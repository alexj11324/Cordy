//! Profile configuration and task-context isolation ported from
//! `server/internal/cli/config.go` and the resolvers in `cmd/cordy`.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const TASK_CONFIG_ROOT_ENV: &str = "CORDY_TASK_CONFIG_ROOT";
const TASK_CONTEXT_MARKER_REL_PATH: &str = ".cordy/daemon_task_context.json";
const TASK_CONTEXT_MARKER_MANAGED_BY: &str = "cordy-daemon-task";

const CAPTURED_ENV_KEYS: &[&str] = &[
    "CORDY_AGENT_ID",
    "CORDY_TASK_ID",
    "CORDY_TOKEN",
    "CORDY_DAEMON_PORT",
    "CORDY_WORKSPACE_ID",
    "CORDY_SERVER_URL",
    "CORDY_HTTP_TIMEOUT",
    "CORDY_DEBUG",
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
    fs::create_dir_all(directory).context("create CLI config directory")?;
    let Some(task_root) = task_root else {
        return Ok(());
    };
    let task_root = Path::new(task_root);
    let mut current = directory;
    loop {
        restrict_directory_permissions(current)?;
        if current == task_root {
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

#[derive(Clone, Debug, Default, Deserialize)]
pub struct CliConfig {
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub token: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_paths_match_go_layouts() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let mut env = Environment::for_test(home.path().into(), cwd.path().into());

        assert_eq!(
            env.config_path("").expect("default path"),
            home.path().join(".cordy/config.json")
        );
        assert_eq!(
            env.config_path("dev").expect("profile path"),
            home.path().join(".cordy/profiles/dev/config.json")
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
    fn task_marker_is_fail_closed_and_actionable_only_when_task_scoped() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let marker_dir = cwd.path().join(".cordy");
        fs::create_dir_all(&marker_dir).expect("marker dir");
        let marker_path = marker_dir.join("daemon_task_context.json");
        fs::write(
            &marker_path,
            r#"{"managed_by":"cordy-daemon-task","agent_id":"agent-1"}"#,
        )
        .expect("marker");

        let env = Environment::for_test(home.path().into(), cwd.path().into());
        assert!(env.in_daemon_managed_execution_context());
        assert!(env
            .leftover_marker_suffix()
            .expect("leftover suffix")
            .contains(marker_path.to_string_lossy().as_ref()));

        fs::write(&marker_path, r#"{"managed_by":"cordy-daemon-task"}"#).expect("root marker");
        assert!(env.leftover_marker_suffix().is_none());
    }

    #[test]
    fn clear_profile_token_is_locked_atomic_and_preserves_other_fields() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let profile_dir = home.path().join(".cordy/profiles/dev");
        fs::create_dir_all(&profile_dir).expect("profile dir");
        fs::write(
            profile_dir.join("config.json"),
            r#"{"server_url":"https://dev.example","token":"mul_dev","future":{"enabled":true}}"#,
        )
        .expect("profile config");
        let default_path = home.path().join(".cordy/config.json");
        fs::create_dir_all(default_path.parent().expect("default dir")).expect("default dir");
        let default_bytes = br#"{"token":"mul_default","workspace_id":"default-workspace"}"#;
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
            fs::read(&default_path).expect("default unchanged"),
            default_bytes
        );
        assert!(!environment
            .clear_profile_token("missing")
            .expect("missing profile is idempotent"));
    }
}
