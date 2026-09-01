//! Profile configuration and task-context isolation for CLI commands.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

pub const TASK_CONFIG_ROOT_ENV: &str = "PATCHBAY_TASK_CONFIG_ROOT";
const TASK_CONTEXT_MARKER_REL_PATH: &str = ".patchbay/daemon_task_context.json";
const LEGACY_TASK_CONTEXT_MARKER_REL_PATH: &str = ".cordy/daemon_task_context.json"; // legacy-brand-compat
const TASK_CONTEXT_MARKER_MANAGED_BY: &str = "patchbay-daemon-task";

const CAPTURED_ENV_KEYS: &[&str] = &[
    "PATCHBAY_AGENT_ID",
    "PATCHBAY_AGENT_NAME",
    "PATCHBAY_TASK_ID",
    "PATCHBAY_TOKEN",
    "PATCHBAY_DAEMON_PORT",
    "PATCHBAY_WORKSPACE_ID",
    "PATCHBAY_SERVER_URL",
    "PATCHBAY_APP_URL",
    "FRONTEND_ORIGIN",
    "PATCHBAY_HTTP_TIMEOUT",
    "PATCHBAY_DEBUG",
    "PATCHBAY_REPO_CHECKOUT_MODE",
    "PATCHBAY_DAEMON_ID",
    "PATCHBAY_DAEMON_DEVICE_NAME",
    "PATCHBAY_AGENT_RUNTIME_NAME",
    "PATCHBAY_WORKSPACES_ROOT",
    "PATCHBAY_TASK_WORKSPACES_ROOT",
    "PATCHBAY_DAEMON_MAX_CONCURRENT_TASKS",
    "PATCHBAY_DAEMON_POLL_INTERVAL",
    "PATCHBAY_DAEMON_HEARTBEAT_INTERVAL",
    "PATCHBAY_AGENT_TIMEOUT",
    "PATCHBAY_CODEX_SEMANTIC_INACTIVITY_TIMEOUT",
    "PATCHBAY_CODEX_HANDSHAKE_TIMEOUT",
    "PATCHBAY_DAEMON_AUTO_UPDATE",
    "PATCHBAY_DAEMON_AUTO_UPDATE_INTERVAL",
    "PATCHBAY_DAEMON_AUTO_RELOAD",
    "SSH_CONNECTION",
    "SSH_CLIENT",
    "SSH_TTY",
    "PATCHBAY_QUICK_CREATE_TASK_ID",
    "PATCHBAY_QUICK_CREATE_ATTACHMENT_IDS",
    TASK_CONFIG_ROOT_ENV,
];

fn capture_environment_values(
    mut read: impl FnMut(&str) -> Option<String>,
) -> HashMap<String, String> {
    CAPTURED_ENV_KEYS
        .iter()
        .filter_map(|key| read(key).map(|value| ((*key).into(), value)))
        .collect()
}
#[derive(Clone, Debug)]
pub struct Environment {
    values: HashMap<String, String>,
    current_dir: PathBuf,
    home_dir: Option<PathBuf>,
}

impl Environment {
    pub fn from_process() -> Result<Self> {
        let values = capture_environment_values(|key| std::env::var(key).ok());
        let home_dir = dirs::home_dir();
        if let Some(home) = home_dir.as_deref() {
            migrate_legacy_config_dir(home)?;
        }
        Ok(Self {
            values,
            current_dir: std::env::current_dir().context("resolve current directory")?,
            home_dir,
        })
    }

    #[cfg(test)]
    pub fn for_test(home_dir: PathBuf, current_dir: PathBuf) -> Self {
        Self {
            values: HashMap::new(),
            current_dir,
            home_dir: Some(home_dir),
        }
    }

    #[cfg(test)]
    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        self.values.insert(key.into(), value.into());
    }

    pub fn raw(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    pub(crate) fn current_dir(&self) -> &Path {
        &self.current_dir
    }

    pub fn trimmed(&self, key: &str) -> Option<&str> {
        self.raw(key)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    pub fn config_path(&self, profile: &str) -> Result<PathBuf> {
        if let Some(root) = self.trimmed(TASK_CONFIG_ROOT_ENV) {
            let root = normalize_path(Path::new(root));
            if !root.is_absolute() {
                bail!("{TASK_CONFIG_ROOT_ENV} must be an absolute path");
            }
            validate_task_local_profile(profile)?;
            return Ok(if profile.is_empty() {
                root.join("config.json")
            } else {
                root.join("profiles").join(profile).join("config.json")
            });
        }

        let home = self
            .home_dir
            .as_ref()
            .context("resolve CLI config path: home directory is unavailable")?;
        Ok(if profile.is_empty() {
            home.join(".patchbay/config.json")
        } else {
            home.join(".patchbay/profiles")
                .join(profile)
                .join("config.json")
        })
    }

    pub fn load_config(&self, profile: &str) -> Result<CliConfig> {
        let path = self.config_path(profile)?;
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CliConfig::default())
            }
            Err(err) => return Err(err).context("read CLI config"),
        };
        serde_json::from_slice(&data).context("parse CLI config")
    }

    pub fn clear_profile_token(&self, profile: &str) -> Result<bool> {
        let path = self.config_path(profile)?;
        let directory = path.parent().context("resolve CLI config directory")?;
        if !directory.exists() {
            return Ok(false);
        }
        let lock_path = directory.join(".config.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .context("open CLI config lock")?;
        restrict_file_permissions(&lock_path)?;
        lock.lock().context("lock CLI config")?;

        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error).context("read CLI config"),
        };
        let mut document: Value = serde_json::from_slice(&data).context("parse CLI config")?;
        let object = document
            .as_object_mut()
            .context("parse CLI config: expected a JSON object")?;
        let has_token = object
            .get("token")
            .and_then(Value::as_str)
            .is_some_and(|token| !token.is_empty());
        if !has_token {
            return Ok(false);
        }
        object.remove("token");
        write_json_atomically(&path, &document)?;
        Ok(true)
    }

    /// Apply one Desktop-owned profile mutation under the CLI config lock.
    ///
    /// Desktop sends credentials over stdin to the private CLI helper. Keeping
    /// the server URL, PAT, and owning user id in one locked JSON update avoids
    /// races with terminal CLI writes and prevents a token from being paired
    /// with the previous Desktop account.
    pub(crate) fn update_desktop_profile(
        &self,
        profile: &str,
        server_url: Option<&str>,
        credentials: Option<(&str, &str)>,
        clear_credentials: bool,
    ) -> Result<()> {
        anyhow::ensure!(
            profile == "desktop" || profile.starts_with("desktop-"),
            "private Desktop helper requires a Desktop-owned profile"
        );
        validate_task_local_profile(profile)?;

        let path = self.config_path(profile)?;
        if clear_credentials && server_url.is_none() && credentials.is_none() && !path.exists() {
            return Ok(());
        }
        let directory = path.parent().context("resolve CLI config directory")?;
        ensure_config_directory(directory, self.trimmed(TASK_CONFIG_ROOT_ENV))?;
        let lock_path = directory.join(".config.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .context("open CLI config lock")?;
        restrict_file_permissions(&lock_path)?;
        lock.lock().context("lock CLI config")?;

        let mut document = read_config_document(&path)?;
        let object = document
            .as_object_mut()
            .context("parse CLI config: expected a JSON object")?;
        if let Some(server_url) = server_url {
            object.insert("server_url".into(), Value::String(server_url.into()));
        }
        if let Some((token, user_id)) = credentials {
            object.insert("token".into(), Value::String(token.into()));
            object.insert("desktop_user_id".into(), Value::String(user_id.into()));
        }
        if clear_credentials {
            object.remove("token");
            object.remove("desktop_user_id");
        }
        write_json_atomically(&path, &document)
    }

    /// Persist an authenticated profile in one locked, atomic replacement.
    ///
    /// Authentication changes deployment credentials and workspace identity
    /// together; retaining the old workspace after a login can direct the
    /// daemon at a tenant from the previous account. The token never appears
    /// in an error or diagnostic produced by this method.
    pub fn save_authenticated_profile(
        &self,
        profile: &str,
        server_url: &str,
        app_url: &str,
        token: &str,
        workspace_id: &str,
    ) -> Result<()> {
        let path = self.config_path(profile)?;
        let directory = path.parent().context("resolve CLI config directory")?;
        ensure_config_directory(directory, self.trimmed(TASK_CONFIG_ROOT_ENV))?;
        let lock_path = directory.join(".config.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .context("open CLI config lock")?;
        restrict_file_permissions(&lock_path)?;
        lock.lock().context("lock CLI config")?;

        let mut document = read_config_document(&path)?;
        let object = document
            .as_object_mut()
            .context("parse CLI config: expected a JSON object")?;
        object.insert("server_url".into(), Value::String(server_url.into()));
        object.insert("app_url".into(), Value::String(app_url.into()));
        object.insert("token".into(), Value::String(token.into()));
        object.insert("workspace_id".into(), Value::String(workspace_id.into()));
        write_json_atomically(&path, &document)
    }

    pub fn load_profile_document(&self, profile: &str) -> Result<Value> {
        let path = self.config_path(profile)?;
        read_config_document(&path)
    }

    pub fn set_profile_value(&self, profile: &str, key: &str, value: Option<Value>) -> Result<()> {
        let path = self.config_path(profile)?;
        let directory = path.parent().context("resolve CLI config directory")?;
        ensure_config_directory(directory, self.trimmed(TASK_CONFIG_ROOT_ENV))?;
        let lock_path = directory.join(".config.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .context("open CLI config lock")?;
        restrict_file_permissions(&lock_path)?;
        lock.lock().context("lock CLI config")?;

        let mut document = read_config_document(&path)?;
        let object = document
            .as_object_mut()
            .context("parse CLI config: expected a JSON object")?;
        match value {
            Some(value) => {
                object.insert(key.into(), value);
            }
            None => {
                object.remove(key);
            }
        }
        write_json_atomically(&path, &document)
    }

    /// Atomically persist the credentials established by `login --token`.
    /// Keeping the server URL and token update under one profile lock avoids
    /// leaving a profile half-updated if the second field cannot be written.
    pub fn save_profile_credentials(
        &self,
        profile: &str,
        server_url: &str,
        token: &str,
    ) -> Result<()> {
        self.save_profile_credentials_with_app_url(profile, server_url, None, token)
    }

    pub fn save_profile_credentials_with_app_url(
        &self,
        profile: &str,
        server_url: &str,
        app_url: Option<&str>,
        token: &str,
    ) -> Result<()> {
        let path = self.config_path(profile)?;
        let directory = path.parent().context("resolve CLI config directory")?;
        ensure_config_directory(directory, self.trimmed(TASK_CONFIG_ROOT_ENV))?;
        let lock_path = directory.join(".config.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .context("open CLI config lock")?;
        restrict_file_permissions(&lock_path)?;
        lock.lock().context("lock CLI config")?;

        let mut document = read_config_document(&path)?;
        let object = document
            .as_object_mut()
            .context("parse CLI config: expected a JSON object")?;
        object.insert("server_url".into(), Value::String(server_url.into()));
        if let Some(app_url) = app_url {
            object.insert("app_url".into(), Value::String(app_url.into()));
        }
        object.insert("workspace_id".into(), Value::String(String::new()));
        object.insert("token".into(), Value::String(token.into()));
        write_json_atomically(&path, &document)
    }

    pub fn set_profile_command_override(
        &self,
        profile: &str,
        profile_id: &str,
        executable_path: Option<&str>,
    ) -> Result<bool> {
        let path = self.config_path(profile)?;
        let directory = path.parent().context("resolve CLI config directory")?;
        ensure_config_directory(directory, self.trimmed(TASK_CONFIG_ROOT_ENV))?;
        let lock_path = directory.join(".config.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .context("open CLI config lock")?;
        restrict_file_permissions(&lock_path)?;
        lock.lock().context("lock CLI config")?;

        let mut document = read_config_document(&path)?;
        let root = document
            .as_object_mut()
            .context("parse CLI config: expected a JSON object")?;
        let mut overrides = match root.remove("profile_command_overrides") {
            None => serde_json::Map::new(),
            Some(Value::Object(overrides)) => overrides,
            Some(_) => bail!("parse CLI config: profile_command_overrides must be a JSON object"),
        };
        let changed = match executable_path {
            Some(executable_path) => {
                let value = Value::String(executable_path.into());
                overrides.insert(profile_id.into(), value.clone()).as_ref() != Some(&value)
            }
            None => overrides.remove(profile_id).is_some(),
        };
        if !changed {
            if !overrides.is_empty() {
                root.insert("profile_command_overrides".into(), Value::Object(overrides));
            }
            return Ok(false);
        }
        if !overrides.is_empty() {
            root.insert("profile_command_overrides".into(), Value::Object(overrides));
        }
        write_json_atomically(&path, &document)?;
        Ok(true)
    }

    /// Replace a profile with the minimal configuration produced by `setup`,
    /// but only after the caller's health preflight succeeds.
    ///
    /// Setup is intentionally a whole-profile replacement: an old token,
    /// workspace, or daemon override must not silently survive a switch to a
    /// different deployment. The probe is injected so the command layer can
    /// enforce its two-second `/health` contract without coupling persistence
    /// to an HTTP client (and tests can exercise the no-mutation failure
    /// path). The config lock is acquired only for the eventual write, so an
    /// unreachable target cannot truncate or otherwise alter the existing
    /// profile.
    pub fn replace_profile_for_setup_if_reachable<F>(
        &self,
        profile: &str,
        input: &SetupProfileInput,
        probe: F,
    ) -> Result<bool>
    where
        F: FnOnce(&str) -> bool,
    {
        if !probe(&input.server_url) {
            return Ok(false);
        }
        self.replace_profile_for_setup(profile, input)?;
        Ok(true)
    }

    /// Atomically persist the minimal profile emitted by `setup`.
    ///
    /// This deliberately does not merge with the previous JSON document:
    /// setup is a deployment switch, not a `config set` operation. Unknown
    /// fields and credentials from the old deployment are discarded, while
    /// the existing lock file is preserved and the replacement file is
    /// written with the normal restricted permissions.
    pub fn replace_profile_for_setup(
        &self,
        profile: &str,
        input: &SetupProfileInput,
    ) -> Result<()> {
        let path = self.config_path(profile)?;
        let directory = path.parent().context("resolve CLI config directory")?;
        ensure_config_directory(directory, self.trimmed(TASK_CONFIG_ROOT_ENV))?;
        let lock_path = directory.join(".config.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .context("open CLI config lock")?;
        restrict_file_permissions(&lock_path)?;
        lock.lock().context("lock CLI config")?;

        let document = serde_json::json!({
            "server_url": input.server_url,
            "app_url": input.app_url,
        });
        write_json_atomically(&path, &document)
    }

    pub fn in_agent_execution_context(&self) -> bool {
        self.raw("PATCHBAY_AGENT_ID")
            .is_some_and(|value| !value.is_empty())
            || self
                .raw("PATCHBAY_TASK_ID")
                .is_some_and(|value| !value.is_empty())
    }

    pub fn in_daemon_managed_execution_context(&self) -> bool {
        self.in_agent_execution_context()
            || self
                .raw("PATCHBAY_DAEMON_PORT")
                .is_some_and(|value| !value.is_empty())
            || self.daemon_task_context_marker().is_some()
    }

    pub fn in_daemon_task_identity_context(&self) -> bool {
        self.in_agent_execution_context()
            || self.trimmed(TASK_CONFIG_ROOT_ENV).is_some()
            || self.daemon_task_context_marker().is_some()
    }

    pub fn daemon_port_only_context_hint(&self) -> &'static str {
        if self.trimmed("PATCHBAY_DAEMON_PORT").is_some()
            && !self.in_agent_execution_context()
            && self.trimmed(TASK_CONFIG_ROOT_ENV).is_none()
            && self.daemon_task_context_marker().is_none()
        {
            "; PATCHBAY_DAEMON_PORT is set without task identity — if this is a host or container startup shell, remove that variable and retry"
        } else {
            ""
        }
    }

    pub fn leftover_marker_suffix(&self) -> Option<String> {
        if self.in_agent_execution_context()
            || self.trimmed(TASK_CONFIG_ROOT_ENV).is_some()
            || self.trimmed("PATCHBAY_DAEMON_PORT").is_some()
        {
            return None;
        }
        let marker_path = self.daemon_task_context_marker()?;
        let data = fs::read(&marker_path).ok()?;
        let marker: TaskContextMarker = serde_json::from_slice(&data).ok()?;
        if marker.agent_id.trim().is_empty() && marker.issue_id.trim().is_empty() {
            return None;
        }
        Some(format!(
            "; detected a daemon task marker at {} — if you are not running inside an agent task this is likely a leftover, remove it and retry",
            marker_path.display()
        ))
    }

    fn daemon_task_context_marker(&self) -> Option<PathBuf> {
        self.current_dir.ancestors().find_map(|dir| {
            [
                TASK_CONTEXT_MARKER_REL_PATH,
                LEGACY_TASK_CONTEXT_MARKER_REL_PATH,
            ]
            .into_iter()
            .find_map(|marker_path| {
                let path = dir.join(marker_path);
                let data = fs::read(&path).ok()?;
                let marker: TaskContextMarker = serde_json::from_slice(&data).ok()?;
                (marker.managed_by == TASK_CONTEXT_MARKER_MANAGED_BY).then_some(path)
            })
        })
    }
}

fn migrate_legacy_config_dir(home: &Path) -> Result<()> {
    let destination = home.join(".patchbay");
    if destination.exists() {
        return Ok(());
    }
    let source = home.join(".cordy"); // legacy-brand-compat
    if !source.exists() {
        return Ok(());
    }
    fs::rename(&source, &destination).with_context(|| {
        format!(
            "migrate legacy CLI configuration from {} to {}",
            source.display(),
            destination.display()
        )
    })
}

/// The only values setup is allowed to write before authentication. Keeping
/// this separate from [`CliConfig`] makes it impossible for a setup caller to
/// accidentally persist an old token, workspace, or daemon execution state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetupProfileInput {
    pub server_url: String,
    pub app_url: String,
}

impl SetupProfileInput {
    pub fn new(server_url: impl Into<String>, app_url: impl Into<String>) -> Result<Self> {
        let server_url = server_url.into();
        let app_url = app_url.into();
        anyhow::ensure!(
            !server_url.trim().is_empty(),
            "setup server URL must not be empty"
        );
        anyhow::ensure!(
            !app_url.trim().is_empty(),
            "setup app URL must not be empty"
        );
        Ok(Self {
            server_url,
            app_url,
        })
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    let _ = normalized.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ if !normalized.has_root() => normalized.push(component.as_os_str()),
                _ => {}
            },
            _ => normalized.push(component.as_os_str()),
        }
    }
    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

fn read_config_document(path: &Path) -> Result<Value> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Value::Object(serde_json::Map::new()))
        }
        Err(error) => return Err(error).context("read CLI config"),
    };
    let document: Value = serde_json::from_slice(&data).context("parse CLI config")?;
    if !document.is_object() {
        bail!("parse CLI config: expected a JSON object");
    }
    Ok(document)
}

fn ensure_config_directory(directory: &Path, task_root: Option<&str>) -> Result<()> {
    let Some(task_root) = task_root else {
        fs::create_dir_all(directory).context("create CLI config directory")?;
        return Ok(());
    };
    let task_root = normalize_path(Path::new(task_root));
    if !directory.starts_with(&task_root) {
        bail!(
            "task-local CLI config directory {:?} escapes root {:?}",
            directory,
            task_root
        );
    }
    fs::create_dir_all(directory).context("create CLI config directory")?;
    let mut current = directory;
    loop {
        restrict_directory_permissions(current)?;
        if current == task_root.as_path() {
            return Ok(());
        }
        current = current.parent().with_context(|| {
            format!(
                "task-local CLI config directory {:?} escapes root {:?}",
                directory, task_root
            )
        })?;
    }
}

fn write_json_atomically(path: &Path, document: &Value) -> Result<()> {
    let directory = path.parent().context("resolve CLI config directory")?;
    let mut data = serde_json::to_vec_pretty(document).context("encode CLI config")?;
    data.push(b'\n');
    let (mut temporary, temporary_path) = create_config_temp_file(directory)?;
    let result = (|| -> Result<()> {
        temporary
            .write_all(&data)
            .context("write temp config file")?;
        temporary.sync_all().context("sync temp config file")?;
        drop(temporary);
        restrict_file_permissions(&temporary_path)?;
        fs::rename(&temporary_path, path).context("rename config file")?;
        sync_directory(directory)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn create_config_temp_file(directory: &Path) -> Result<(File, PathBuf)> {
    for attempt in 0..100_u8 {
        let path = directory.join(format!(".config-{}-{attempt}.json.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create temp config file"),
        }
    }
    bail!("create temp config file: exhausted unique names")
}

#[cfg(unix)]
fn restrict_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).context("chmod CLI config file")
}

#[cfg(unix)]
fn restrict_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .context("restrict task-local CLI config directory")
}

#[cfg(not(unix))]
fn restrict_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<()> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .context("sync CLI config directory")
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> Result<()> {
    Ok(())
}

#[derive(Clone, Default, Deserialize)]
pub struct CliConfig {
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub app_url: String,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub runtime_name: String,
    #[serde(default)]
    pub workspaces_root: String,
    #[serde(default)]
    pub max_concurrent_tasks: i64,
    #[serde(default)]
    pub poll_interval: String,
    #[serde(default)]
    pub heartbeat_interval: String,
    /// `None` means no persisted override; `Some("0s")` explicitly disables
    /// the wall-clock task cap and must survive profile loading.
    #[serde(default)]
    pub agent_timeout: Option<String>,
    #[serde(default)]
    pub codex_semantic_inactivity_timeout: String,
    #[serde(default)]
    pub codex_handshake_timeout: String,
    #[serde(default)]
    pub disable_auto_update: bool,
    #[serde(default)]
    pub auto_update_check_interval: String,
    #[serde(default)]
    pub disable_auto_reload: bool,
    #[serde(default)]
    pub backends: Option<BackendOverrides>,
    #[serde(default)]
    pub profile_command_overrides: BTreeMap<String, String>,
}

impl CliConfig {
    /// Extracts the credential and backend/profile settings consumed by the
    /// production daemon constructor. The returned type deliberately has no
    /// `Debug` implementation so the bearer token cannot enter diagnostics.
    pub fn daemon_profile_input(&self) -> patchbay_daemon::assembly::DaemonProfileInput {
        let openclaw = self
            .backends
            .as_ref()
            .and_then(|backends| backends.openclaw.as_ref());
        patchbay_daemon::assembly::DaemonProfileInput {
            token: self.token.clone(),
            profile_command_overrides: self.profile_command_overrides.clone(),
            openclaw_binary_path: openclaw
                .map(|override_| override_.binary_path.clone())
                .unwrap_or_default(),
            openclaw_state_dir: openclaw
                .map(|override_| override_.state_dir.clone())
                .unwrap_or_default(),
            openclaw_cli_timeout: openclaw
                .map(|override_| override_.cli_timeout.clone())
                .unwrap_or_default(),
        }
    }

    /// Extracts only the non-secret profile settings consumed by the local
    /// runtime probe. The probe must not receive the stored bearer token.
    pub fn daemon_runtime_probe_options(
        &self,
        profile: &str,
    ) -> patchbay_daemon::runtime_probe::RuntimeProbeOptions {
        let openclaw = self
            .backends
            .as_ref()
            .and_then(|backends| backends.openclaw.as_ref());
        patchbay_daemon::runtime_probe::RuntimeProbeOptions {
            profile: profile.to_owned(),
            profile_command_overrides: self.profile_command_overrides.clone(),
            openclaw_binary_path: openclaw
                .map(|override_| override_.binary_path.clone())
                .unwrap_or_default(),
            openclaw_state_dir: openclaw
                .map(|override_| override_.state_dir.clone())
                .unwrap_or_default(),
            openclaw_cli_timeout: openclaw
                .map(|override_| override_.cli_timeout.clone())
                .unwrap_or_default(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct BackendOverrides {
    #[serde(default)]
    pub openclaw: Option<OpenClawOverride>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OpenClawOverride {
    #[serde(default)]
    pub binary_path: String,
    #[serde(default)]
    pub state_dir: String,
    #[serde(default)]
    pub cli_timeout: String,
}

/// Raw daemon start/restart flag values. `Option` preserves whether a flag was
/// supplied; that distinction is required for `--agent-timeout 0`.
#[derive(Clone, Debug, Default)]
pub struct DaemonLaunchFlags {
    pub server_url: Option<String>,
    pub daemon_id: Option<String>,
    pub device_name: Option<String>,
    pub runtime_name: Option<String>,
    pub workspaces_root: Option<String>,
    pub poll_interval: Option<Duration>,
    pub heartbeat_interval: Option<Duration>,
    pub agent_timeout: Option<Duration>,
    pub codex_semantic_inactivity_timeout: Option<Duration>,
    pub codex_handshake_timeout: Option<Duration>,
    pub max_concurrent_tasks: Option<i64>,
    pub disable_auto_update: bool,
    pub auto_update_check_interval: Option<Duration>,
    pub disable_auto_reload: bool,
}

/// Resolves the CLI-owned `flag > env > profile > daemon default` layer.
///
/// Most environment values are intentionally represented by an empty/zero
/// output: [`patchbay_daemon::assembly::DaemonProductionInputs`] reads the same
/// process environment through the authoritative daemon config loader. The
/// server URL is the exception because background authenticated preflight
/// consumes it before the foreground loader runs.
pub fn resolve_daemon_launch_overrides(
    profile: &str,
    flags: &DaemonLaunchFlags,
    environment: &Environment,
    config: &CliConfig,
) -> Result<patchbay_daemon::assembly::DaemonLaunchOverrides> {
    let poll_interval = resolve_positive_duration(
        flags.poll_interval,
        environment,
        "PATCHBAY_DAEMON_POLL_INTERVAL",
        &config.poll_interval,
    )?;
    let heartbeat_interval = resolve_positive_duration(
        flags.heartbeat_interval,
        environment,
        "PATCHBAY_DAEMON_HEARTBEAT_INTERVAL",
        &config.heartbeat_interval,
    )?;
    let codex_semantic_inactivity_timeout = resolve_positive_duration(
        flags.codex_semantic_inactivity_timeout,
        environment,
        "PATCHBAY_CODEX_SEMANTIC_INACTIVITY_TIMEOUT",
        &config.codex_semantic_inactivity_timeout,
    )?;
    let codex_handshake_timeout = resolve_positive_duration(
        flags.codex_handshake_timeout,
        environment,
        "PATCHBAY_CODEX_HANDSHAKE_TIMEOUT",
        &config.codex_handshake_timeout,
    )?;
    let auto_update_check_interval = resolve_positive_duration(
        flags.auto_update_check_interval,
        environment,
        "PATCHBAY_DAEMON_AUTO_UPDATE_INTERVAL",
        &config.auto_update_check_interval,
    )?;
    let agent_timeout = resolve_agent_timeout(flags.agent_timeout, environment, config)?;

    Ok(patchbay_daemon::assembly::DaemonLaunchOverrides {
        // Unlike every other environment-owned field, the server URL is also
        // consumed by the background lifecycle preflight before the child
        // exists. Carry its effective value so preflight and foreground never
        // target different servers.
        server_url: resolve_effective_string(
            flags.server_url.as_deref(),
            environment,
            "PATCHBAY_SERVER_URL",
            &config.server_url,
        ),
        workspaces_root: resolve_string(
            flags.workspaces_root.as_deref(),
            environment,
            "PATCHBAY_WORKSPACES_ROOT",
            &config.workspaces_root,
        ),
        poll_interval,
        heartbeat_interval,
        agent_timeout,
        codex_semantic_inactivity_timeout,
        codex_handshake_timeout,
        max_concurrent_tasks: resolve_positive_integer(
            flags.max_concurrent_tasks,
            environment,
            "PATCHBAY_DAEMON_MAX_CONCURRENT_TASKS",
            config.max_concurrent_tasks,
        ),
        daemon_id: resolve_string(
            flags.daemon_id.as_deref(),
            environment,
            "PATCHBAY_DAEMON_ID",
            "",
        ),
        device_name: resolve_string(
            flags.device_name.as_deref(),
            environment,
            "PATCHBAY_DAEMON_DEVICE_NAME",
            &config.device_name,
        ),
        runtime_name: resolve_string(
            flags.runtime_name.as_deref(),
            environment,
            "PATCHBAY_AGENT_RUNTIME_NAME",
            &config.runtime_name,
        ),
        profile: profile.to_string(),
        health_port: i32::from(patchbay_daemon::control_client::health_port_for_profile(
            profile,
        )),
        allow_no_agents: false,
        disable_auto_update: resolve_disable_signal(
            flags.disable_auto_update,
            environment,
            "PATCHBAY_DAEMON_AUTO_UPDATE",
            config.disable_auto_update,
        ),
        auto_update_check_interval,
        disable_auto_reload: resolve_disable_signal(
            flags.disable_auto_reload,
            environment,
            "PATCHBAY_DAEMON_AUTO_RELOAD",
            config.disable_auto_reload,
        ),
    })
}

fn env_has_value(environment: &Environment, key: &str) -> bool {
    environment.trimmed(key).is_some()
}

fn resolve_string(
    flag: Option<&str>,
    environment: &Environment,
    env_key: &str,
    persisted: &str,
) -> String {
    if let Some(flag) = flag.filter(|value| !value.is_empty()) {
        return flag.to_string();
    }
    if env_has_value(environment, env_key) {
        return String::new();
    }
    persisted.to_string()
}

fn resolve_effective_string(
    flag: Option<&str>,
    environment: &Environment,
    env_key: &str,
    persisted: &str,
) -> String {
    if let Some(flag) = flag.filter(|value| !value.is_empty()) {
        return flag.to_string();
    }
    environment
        .trimmed(env_key)
        .unwrap_or(persisted)
        .to_string()
}

fn resolve_positive_duration(
    flag: Option<Duration>,
    environment: &Environment,
    env_key: &str,
    persisted: &str,
) -> Result<Duration> {
    if let Some(flag) = flag.filter(|value| !value.is_zero()) {
        return Ok(flag);
    }
    if env_has_value(environment, env_key) || persisted.is_empty() {
        return Ok(Duration::ZERO);
    }
    let parsed = patchbay_daemon::helpers::parse_go_duration(persisted).with_context(|| {
        format!("config value {persisted:?} for {env_key} is not a valid duration")
    })?;
    anyhow::ensure!(
        !parsed.is_zero(),
        "config value {persisted:?} for {env_key} must be positive"
    );
    Ok(parsed)
}

fn resolve_agent_timeout(
    flag: Option<Duration>,
    environment: &Environment,
    config: &CliConfig,
) -> Result<Option<Duration>> {
    if flag.is_some() {
        return Ok(flag);
    }
    const ENV_KEY: &str = "PATCHBAY_AGENT_TIMEOUT";
    if env_has_value(environment, ENV_KEY) {
        return Ok(None);
    }
    let Some(persisted) = config
        .agent_timeout
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    patchbay_daemon::helpers::parse_go_duration(persisted)
        .map(Some)
        .with_context(|| format!("config value {persisted:?} for {ENV_KEY} is not valid"))
}

fn resolve_positive_integer(
    flag: Option<i64>,
    environment: &Environment,
    env_key: &str,
    persisted: i64,
) -> i64 {
    if let Some(flag) = flag.filter(|value| *value > 0) {
        return flag;
    }
    if env_has_value(environment, env_key) {
        return 0;
    }
    persisted.max(0)
}

fn resolve_disable_signal(
    flag: bool,
    environment: &Environment,
    env_key: &str,
    persisted: bool,
) -> bool {
    if flag {
        return true;
    }
    if let Some(value) = environment.trimmed(env_key) {
        return matches!(
            value.to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        );
    }
    persisted
}

#[derive(Deserialize)]
struct TaskContextMarker {
    #[serde(default)]
    managed_by: String,
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    issue_id: String,
}

fn validate_task_local_profile(profile: &str) -> Result<()> {
    if profile.is_empty() {
        return Ok(());
    }
    let path = Path::new(profile);
    if matches!(profile, "." | "..")
        || path.is_absolute()
        || profile.contains(['/', '\\'])
        || path.components().count() != 1
    {
        bail!("invalid task-local Patchbay profile name {profile:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_profile_schema_preserves_persisted_launch_and_backend_inputs() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let path = environment.config_path("production").expect("config path");
        fs::create_dir_all(path.parent().expect("profile directory")).expect("create profile");
        fs::write(
            &path,
            r#"{
                "server_url":"https://patchbay.example",
                "app_url":"https://app.patchbay.example",
                "workspace_id":"workspace-1",
                "token":"pby_secret",
                "device_name":"build-host",
                "runtime_name":"night-shift",
                "workspaces_root":"/srv/patchbay-workspaces",
                "max_concurrent_tasks":7,
                "poll_interval":"3s",
                "heartbeat_interval":"11s",
                "agent_timeout":"0s",
                "codex_semantic_inactivity_timeout":"17m",
                "codex_handshake_timeout":"42s",
                "disable_auto_update":true,
                "auto_update_check_interval":"4h",
                "disable_auto_reload":true,
                "backends":{"openclaw":{
                    "binary_path":"/opt/openclaw/bin/openclaw",
                    "state_dir":"/srv/openclaw-state",
                    "cli_timeout":"45s"
                }},
                "profile_command_overrides":{
                    "profile-1":"/opt/agents/custom-codex"
                }
            }"#,
        )
        .expect("write profile");

        let config = environment.load_config("production").expect("load profile");
        assert_eq!(config.server_url, "https://patchbay.example");
        assert_eq!(config.app_url, "https://app.patchbay.example");
        assert_eq!(config.workspace_id, "workspace-1");
        assert_eq!(config.device_name, "build-host");
        assert_eq!(config.runtime_name, "night-shift");
        assert_eq!(config.workspaces_root, "/srv/patchbay-workspaces");
        assert_eq!(config.max_concurrent_tasks, 7);
        assert_eq!(config.poll_interval, "3s");
        assert_eq!(config.heartbeat_interval, "11s");
        assert_eq!(config.agent_timeout.as_deref(), Some("0s"));
        assert_eq!(config.codex_semantic_inactivity_timeout, "17m");
        assert_eq!(config.codex_handshake_timeout, "42s");
        assert!(config.disable_auto_update);
        assert_eq!(config.auto_update_check_interval, "4h");
        assert!(config.disable_auto_reload);

        let daemon = config.daemon_profile_input();
        assert_eq!(daemon.token, "pby_secret");
        assert_eq!(
            daemon
                .profile_command_overrides
                .get("profile-1")
                .map(String::as_str),
            Some("/opt/agents/custom-codex")
        );
        assert_eq!(daemon.openclaw_binary_path, "/opt/openclaw/bin/openclaw");
        assert_eq!(daemon.openclaw_state_dir, "/srv/openclaw-state");
        assert_eq!(daemon.openclaw_cli_timeout, "45s");
    }

    #[test]
    fn absent_agent_timeout_and_backend_overrides_remain_unset() {
        let config: CliConfig =
            serde_json::from_str(r#"{"token":"pby_secret"}"#).expect("minimal profile");
        assert!(config.agent_timeout.is_none());
        assert!(config.backends.is_none());

        let daemon = config.daemon_profile_input();
        assert!(daemon.openclaw_binary_path.is_empty());
        assert!(daemon.openclaw_state_dir.is_empty());
        assert!(daemon.openclaw_cli_timeout.is_empty());
    }

    #[test]
    fn daemon_launch_resolver_applies_persisted_values_when_flag_and_env_are_absent() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let config: CliConfig = serde_json::from_str(
            r#"{
                "server_url":"https://profile.example",
                "device_name":"profile-device",
                "runtime_name":"profile-runtime",
                "workspaces_root":"/profile/workspaces",
                "max_concurrent_tasks":9,
                "poll_interval":"3s",
                "heartbeat_interval":"11s",
                "agent_timeout":"0s",
                "codex_semantic_inactivity_timeout":"17m",
                "codex_handshake_timeout":"42s",
                "disable_auto_update":true,
                "auto_update_check_interval":"4h",
                "disable_auto_reload":true
            }"#,
        )
        .expect("profile config");

        let resolved = resolve_daemon_launch_overrides(
            "production",
            &DaemonLaunchFlags::default(),
            &environment,
            &config,
        )
        .expect("resolve launch");
        assert_eq!(resolved.server_url, "https://profile.example");
        assert_eq!(resolved.device_name, "profile-device");
        assert_eq!(resolved.runtime_name, "profile-runtime");
        assert_eq!(resolved.workspaces_root, "/profile/workspaces");
        assert_eq!(resolved.max_concurrent_tasks, 9);
        assert_eq!(resolved.poll_interval, Duration::from_secs(3));
        assert_eq!(resolved.heartbeat_interval, Duration::from_secs(11));
        assert_eq!(resolved.agent_timeout, Some(Duration::ZERO));
        assert_eq!(
            resolved.codex_semantic_inactivity_timeout,
            Duration::from_secs(17 * 60)
        );
        assert_eq!(resolved.codex_handshake_timeout, Duration::from_secs(42));
        assert!(resolved.disable_auto_update);
        assert_eq!(
            resolved.auto_update_check_interval,
            Duration::from_secs(4 * 60 * 60)
        );
        assert!(resolved.disable_auto_reload);
        assert_eq!(resolved.profile, "production");
        assert_eq!(
            resolved.health_port,
            i32::from(patchbay_daemon::control_client::health_port_for_profile(
                "production"
            ))
        );
    }

    #[test]
    fn daemon_launch_resolver_leaves_environment_values_to_daemon_config() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        for (key, value) in [
            ("PATCHBAY_SERVER_URL", "https://env.example"),
            ("PATCHBAY_DAEMON_ID", "env-daemon"),
            ("PATCHBAY_DAEMON_DEVICE_NAME", "env-device"),
            ("PATCHBAY_AGENT_RUNTIME_NAME", "env-runtime"),
            ("PATCHBAY_WORKSPACES_ROOT", "/env/workspaces"),
            ("PATCHBAY_DAEMON_MAX_CONCURRENT_TASKS", "5"),
            ("PATCHBAY_DAEMON_POLL_INTERVAL", "5s"),
            ("PATCHBAY_DAEMON_HEARTBEAT_INTERVAL", "13s"),
            ("PATCHBAY_AGENT_TIMEOUT", "2h"),
            ("PATCHBAY_CODEX_SEMANTIC_INACTIVITY_TIMEOUT", "19m"),
            ("PATCHBAY_CODEX_HANDSHAKE_TIMEOUT", "51s"),
            ("PATCHBAY_DAEMON_AUTO_UPDATE_INTERVAL", "6h"),
            ("PATCHBAY_DAEMON_AUTO_UPDATE", "true"),
            ("PATCHBAY_DAEMON_AUTO_RELOAD", "false"),
        ] {
            environment.set(key, value);
        }
        let config: CliConfig = serde_json::from_str(
            r#"{
                "server_url":"https://profile.example",
                "device_name":"profile-device",
                "runtime_name":"profile-runtime",
                "workspaces_root":"/profile/workspaces",
                "max_concurrent_tasks":9,
                "poll_interval":"3s",
                "heartbeat_interval":"11s",
                "agent_timeout":"0s",
                "codex_semantic_inactivity_timeout":"17m",
                "codex_handshake_timeout":"42s",
                "disable_auto_update":true,
                "auto_update_check_interval":"4h",
                "disable_auto_reload":false
            }"#,
        )
        .expect("profile config");

        let resolved = resolve_daemon_launch_overrides(
            "",
            &DaemonLaunchFlags::default(),
            &environment,
            &config,
        )
        .expect("resolve launch");
        assert_eq!(resolved.server_url, "https://env.example");
        assert!(resolved.daemon_id.is_empty());
        assert!(resolved.device_name.is_empty());
        assert!(resolved.runtime_name.is_empty());
        assert!(resolved.workspaces_root.is_empty());
        assert_eq!(resolved.max_concurrent_tasks, 0);
        assert!(resolved.poll_interval.is_zero());
        assert!(resolved.heartbeat_interval.is_zero());
        assert!(resolved.agent_timeout.is_none());
        assert!(resolved.codex_semantic_inactivity_timeout.is_zero());
        assert!(resolved.codex_handshake_timeout.is_zero());
        assert!(resolved.auto_update_check_interval.is_zero());
        assert!(!resolved.disable_auto_update);
        assert!(resolved.disable_auto_reload);
    }

    #[test]
    fn daemon_launch_flags_win_and_preserve_explicit_zero_agent_timeout() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut environment = Environment::for_test(home.path().into(), cwd.path().into());
        environment.set("PATCHBAY_SERVER_URL", "https://env.example");
        environment.set("PATCHBAY_AGENT_TIMEOUT", "2h");
        environment.set("PATCHBAY_DAEMON_AUTO_UPDATE", "true");
        let flags = DaemonLaunchFlags {
            server_url: Some("https://flag.example".into()),
            daemon_id: Some("flag-daemon".into()),
            device_name: Some("flag-device".into()),
            runtime_name: Some("flag-runtime".into()),
            workspaces_root: Some("/flag/workspaces".into()),
            poll_interval: Some(Duration::from_secs(2)),
            heartbeat_interval: Some(Duration::from_secs(7)),
            agent_timeout: Some(Duration::ZERO),
            codex_semantic_inactivity_timeout: Some(Duration::from_secs(23 * 60)),
            codex_handshake_timeout: Some(Duration::from_secs(61)),
            max_concurrent_tasks: Some(12),
            disable_auto_update: true,
            auto_update_check_interval: Some(Duration::from_secs(8 * 60 * 60)),
            disable_auto_reload: true,
        };
        let resolved = resolve_daemon_launch_overrides(
            "flag-profile",
            &flags,
            &environment,
            &CliConfig::default(),
        )
        .expect("resolve launch");
        assert_eq!(resolved.server_url, "https://flag.example");
        assert_eq!(resolved.daemon_id, "flag-daemon");
        assert_eq!(resolved.device_name, "flag-device");
        assert_eq!(resolved.runtime_name, "flag-runtime");
        assert_eq!(resolved.workspaces_root, "/flag/workspaces");
        assert_eq!(resolved.poll_interval, Duration::from_secs(2));
        assert_eq!(resolved.heartbeat_interval, Duration::from_secs(7));
        assert_eq!(resolved.agent_timeout, Some(Duration::ZERO));
        assert_eq!(resolved.max_concurrent_tasks, 12);
        assert!(resolved.disable_auto_update);
        assert!(resolved.disable_auto_reload);
    }

    #[test]
    fn invalid_persisted_daemon_duration_fails_closed() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let config = CliConfig {
            poll_interval: "eventually".into(),
            ..CliConfig::default()
        };
        let error = resolve_daemon_launch_overrides(
            "",
            &DaemonLaunchFlags::default(),
            &environment,
            &config,
        )
        .expect_err("invalid duration must fail");
        let message = format!("{error:#}");
        assert!(message.contains("PATCHBAY_DAEMON_POLL_INTERVAL"));
        assert!(message.contains("eventually"));
    }

    #[test]
    fn profile_paths_match_go_layouts() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut env = Environment::for_test(home.path().into(), cwd.path().into());

        assert_eq!(
            env.config_path("").expect("default path"),
            home.path().join(".patchbay/config.json")
        );
        assert_eq!(
            env.config_path("dev").expect("profile path"),
            home.path().join(".patchbay/profiles/dev/config.json")
        );

        let task_root = home.path().join("task-config");
        env.set(TASK_CONFIG_ROOT_ENV, task_root.display().to_string());
        assert_eq!(
            env.config_path("dev").expect("task profile path"),
            task_root.join("profiles/dev/config.json")
        );
        assert!(env.config_path("../owner").is_err());
    }

    #[test]
    fn task_config_root_with_parent_segments_does_not_escape() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut env = Environment::for_test(home.path().into(), cwd.path().into());
        let canonical = home.path().join("task-config");
        let aliased = home.path().join("tasks").join("..").join("task-config");
        env.set(TASK_CONFIG_ROOT_ENV, aliased.display().to_string());
        env.set_profile_value("dev", "token", Some(Value::String("tok".into())))
            .expect("save under aliased task root");
        assert!(canonical.join("profiles/dev/config.json").is_file());
    }

    #[test]
    fn task_marker_is_fail_closed_and_actionable_only_when_task_scoped() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let marker_dir = cwd.path().join(".patchbay");
        fs::create_dir_all(&marker_dir).expect("marker dir");
        let marker_path = marker_dir.join("daemon_task_context.json");
        fs::write(
            &marker_path,
            r#"{"managed_by":"patchbay-daemon-task","agent_id":"agent-1"}"#,
        )
        .expect("marker");

        let env = Environment::for_test(home.path().into(), cwd.path().into());
        assert!(env.in_daemon_managed_execution_context());
        assert!(env
            .leftover_marker_suffix()
            .expect("leftover suffix")
            .contains(marker_path.to_string_lossy().as_ref()));

        fs::write(&marker_path, r#"{"managed_by":"patchbay-daemon-task"}"#).expect("root marker");
        assert!(env.leftover_marker_suffix().is_none());
    }

    #[test]
    fn clear_profile_token_is_locked_atomic_and_preserves_other_fields() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let profile_dir = home.path().join(".patchbay/profiles/dev");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        fs::write(
            profile_dir.join("config.json"),
            r#"{"server_url":"https://dev.example","token":"pby_dev","future":{"enabled":true}}"#,
        )
        .expect("profile config");
        fs::write(profile_dir.join(".config.lock"), b"lock-sentinel").expect("lock sentinel");
        let default_path = home.path().join(".patchbay/config.json");
        fs::create_dir_all(default_path.parent().expect("default dir")).expect("default dir");
        let default_bytes = br#"{"token":"pby_default","workspace_id":"default-workspace"}"#;
        fs::write(&default_path, default_bytes).expect("default config");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());

        let outcomes = std::thread::scope(|scope| {
            let first = environment.clone();
            let second = environment.clone();
            let first = scope.spawn(move || first.clear_profile_token("dev").expect("first clear"));
            let second =
                scope.spawn(move || second.clear_profile_token("dev").expect("second clear"));
            [
                first.join().expect("first thread"),
                second.join().expect("second thread"),
            ]
        });
        assert_eq!(outcomes.into_iter().filter(|removed| *removed).count(), 1);

        let saved: Value = serde_json::from_slice(
            &fs::read(profile_dir.join("config.json")).expect("saved profile"),
        )
        .expect("saved JSON");
        assert!(saved.get("token").is_none());
        assert_eq!(saved["server_url"], "https://dev.example");
        assert_eq!(saved["future"]["enabled"], true);
        assert_eq!(
            fs::read(profile_dir.join(".config.lock")).expect("lock file"),
            b"lock-sentinel"
        );
        assert_eq!(
            fs::read(&default_path).expect("default unchanged"),
            default_bytes
        );
        assert!(!environment
            .clear_profile_token("missing")
            .expect("missing profile is idempotent"));
    }

    #[test]
    fn set_profile_value_does_not_truncate_existing_lock_file() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let profile_dir = home.path().join(".patchbay/profiles/dev");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        fs::write(profile_dir.join(".config.lock"), b"lock-sentinel").expect("lock sentinel");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());

        environment
            .set_profile_value(
                "dev",
                "workspace_id",
                Some(Value::String("workspace-1".into())),
            )
            .expect("set profile value");

        assert_eq!(
            fs::read(profile_dir.join(".config.lock")).expect("lock file"),
            b"lock-sentinel"
        );
    }

    #[test]
    fn setup_health_failure_does_not_mutate_existing_profile() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let profile_dir = home.path().join(".patchbay/profiles/dev");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        let config_path = profile_dir.join("config.json");
        let old_config = br#"{
            "server_url":"https://old.example",
            "app_url":"https://old-app.example",
            "token":"pby_old",
            "workspace_id":"workspace-old",
            "future":{"kept":true}
        }
        "#;
        fs::write(&config_path, old_config).expect("old config");
        fs::write(profile_dir.join(".config.lock"), b"lock-sentinel").expect("lock sentinel");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let input = SetupProfileInput::new("https://new.example", "https://new-app.example")
            .expect("setup input");

        let persisted = environment
            .replace_profile_for_setup_if_reachable("dev", &input, |_| false)
            .expect("probe failure is not an I/O error");

        assert!(!persisted);
        assert_eq!(fs::read(&config_path).expect("config remains"), old_config);
        assert_eq!(
            fs::read(profile_dir.join(".config.lock")).expect("lock remains"),
            b"lock-sentinel"
        );
    }

    #[test]
    fn setup_success_replaces_whole_profile_atomically() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let profile_dir = home.path().join(".patchbay/profiles/dev");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        fs::write(
            profile_dir.join("config.json"),
            br#"{"server_url":"https://old.example","token":"pby_old","workspace_id":"old","future":{"kept":true}}"#,
        )
        .expect("old config");
        fs::write(profile_dir.join(".config.lock"), b"lock-sentinel").expect("lock sentinel");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let input = SetupProfileInput::new("https://new.example", "https://new-app.example")
            .expect("setup input");

        assert!(environment
            .replace_profile_for_setup_if_reachable("dev", &input, |url| {
                assert_eq!(url, "https://new.example");
                true
            })
            .expect("persist setup profile"));

        let saved = environment
            .load_profile_document("dev")
            .expect("saved config");
        assert_eq!(saved["server_url"], "https://new.example");
        assert_eq!(saved["app_url"], "https://new-app.example");
        assert!(saved.get("token").is_none());
        assert!(saved.get("workspace_id").is_none());
        assert!(saved.get("future").is_none());
        assert_eq!(
            fs::read(profile_dir.join(".config.lock")).expect("lock remains"),
            b"lock-sentinel"
        );
    }

    #[test]
    fn setup_input_rejects_empty_urls() {
        assert!(SetupProfileInput::new("", "https://app.example").is_err());
        assert!(SetupProfileInput::new("https://api.example", " ").is_err());
    }

    #[test]
    fn captured_env_includes_quick_create_task_keys() {
        assert!(CAPTURED_ENV_KEYS.contains(&"PATCHBAY_QUICK_CREATE_TASK_ID"));
        assert!(CAPTURED_ENV_KEYS.contains(&"PATCHBAY_QUICK_CREATE_ATTACHMENT_IDS"));
    }
}
