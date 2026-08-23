//! Profile configuration and task-context isolation ported from
//! `server/internal/cli/config.go` and the resolvers in `cmd/cordy`.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
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

    pub fn trimmed(&self, key: &str) -> Option<&str> {
        self.raw(key)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }

    pub fn config_path(&self, profile: &str) -> Result<PathBuf> {
        if let Some(root) = self.trimmed(TASK_CONFIG_ROOT_ENV) {
            let root = PathBuf::from(root);
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
}
