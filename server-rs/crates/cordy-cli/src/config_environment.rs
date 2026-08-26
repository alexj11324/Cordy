//! Environment capture and atomic profile persistence.
//!
//! This module owns task-context isolation, lock files, and restricted atomic
//! writes; profile schema and daemon launch resolution stay in the parent
//! config module.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use crate::config_persistence::{
    ensure_config_directory, read_config_document, restrict_file_permissions, write_json_atomically,
};
use crate::config::CliConfig;

pub const TASK_CONFIG_ROOT_ENV: &str = "CORDY_TASK_CONFIG_ROOT";
const TASK_CONTEXT_MARKER_REL_PATH: &str = ".cordy/daemon_task_context.json";
const TASK_CONTEXT_MARKER_MANAGED_BY: &str = "cordy-daemon-task";

const CAPTURED_ENV_KEYS: &[&str] = &[
    "CORDY_AGENT_ID",
    "CORDY_AGENT_NAME",
    "CORDY_TASK_ID",
    "CORDY_TOKEN",
    "CORDY_DAEMON_PORT",
    "CORDY_WORKSPACE_ID",
    "CORDY_SERVER_URL",
    "CORDY_APP_URL",
    "CORDY_HTTP_TIMEOUT",
    "CORDY_DEBUG",
    "CORDY_REPO_CHECKOUT_MODE",
    "CORDY_DAEMON_ID",
    "CORDY_DAEMON_DEVICE_NAME",
    "CORDY_AGENT_RUNTIME_NAME",
    "CORDY_WORKSPACES_ROOT",
    "CORDY_TASK_WORKSPACES_ROOT",
    "CORDY_DAEMON_MAX_CONCURRENT_TASKS",
    "CORDY_DAEMON_POLL_INTERVAL",
    "CORDY_DAEMON_HEARTBEAT_INTERVAL",
    "CORDY_AGENT_TIMEOUT",
    "CORDY_CODEX_SEMANTIC_INACTIVITY_TIMEOUT",
    "CORDY_CODEX_HANDSHAKE_TIMEOUT",
    "CORDY_DAEMON_AUTO_UPDATE",
    "CORDY_DAEMON_AUTO_UPDATE_INTERVAL",
    "CORDY_DAEMON_AUTO_RELOAD",
    TASK_CONFIG_ROOT_ENV,
];

#[derive(Clone, Debug)]
pub struct Environment {
    values: HashMap<String, String>,
    current_dir: PathBuf,
    home_dir: Option<PathBuf>,
}

impl Environment {
    pub fn from_process() -> Result<Self> {
        let values = CAPTURED_ENV_KEYS
            .iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| ((*key).into(), value)))
            .collect();
        Ok(Self {
            values,
            current_dir: std::env::current_dir().context("resolve current directory")?,
            home_dir: dirs::home_dir(),
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
            home.join(".cordy/config.json")
        } else {
            home.join(".cordy/profiles")
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
        self.raw("CORDY_AGENT_ID")
            .is_some_and(|value| !value.is_empty())
            || self
                .raw("CORDY_TASK_ID")
                .is_some_and(|value| !value.is_empty())
    }

    pub fn in_daemon_managed_execution_context(&self) -> bool {
        self.in_agent_execution_context()
            || self
                .raw("CORDY_DAEMON_PORT")
                .is_some_and(|value| !value.is_empty())
            || self.daemon_task_context_marker().is_some()
    }

    pub fn in_daemon_task_identity_context(&self) -> bool {
        self.in_agent_execution_context()
            || self.trimmed(TASK_CONFIG_ROOT_ENV).is_some()
            || self.daemon_task_context_marker().is_some()
    }

    pub fn daemon_port_only_context_hint(&self) -> &'static str {
        if self.trimmed("CORDY_DAEMON_PORT").is_some()
            && !self.in_agent_execution_context()
            && self.trimmed(TASK_CONFIG_ROOT_ENV).is_none()
            && self.daemon_task_context_marker().is_none()
        {
            "; CORDY_DAEMON_PORT is set without task identity — if this is a host or container startup shell, remove that variable and retry"
        } else {
            ""
        }
    }

    pub fn leftover_marker_suffix(&self) -> Option<String> {
        if self.in_agent_execution_context()
            || self.trimmed(TASK_CONFIG_ROOT_ENV).is_some()
            || self.trimmed("CORDY_DAEMON_PORT").is_some()
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
            let path = dir.join(TASK_CONTEXT_MARKER_REL_PATH);
            let data = fs::read(&path).ok()?;
            let marker: TaskContextMarker = serde_json::from_slice(&data).ok()?;
            (marker.managed_by == TASK_CONTEXT_MARKER_MANAGED_BY).then_some(path)
        })
    }
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
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
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
        bail!("invalid task-local Cordy profile name {profile:?}");
    }
    Ok(())
}
