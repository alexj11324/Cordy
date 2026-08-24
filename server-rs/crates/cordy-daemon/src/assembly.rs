//! Production input assembly for the daemon binary.
//!
//! CLI profile parsing stays owned by `cordy-cli`; this module accepts the
//! resolved, non-secret values and turns them into the daemon's authoritative
//! [`Config`], authenticated [`Client`], and repository [`Cache`]. Keeping the
//! boundary here prevents a binary from reimplementing configuration
//! precedence or accidentally starting the stack before authentication and
//! workspace-root safety have been established.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::client::Client;
use crate::config::{self, CliProfileConfig, Config, Overrides};
use crate::execenv::context::ensure_workspaces_root_marker;
use crate::repocache::Cache;
use crate::types::AgentEntry;

/// Resolved daemon flags and persisted settings supplied by the CLI layer.
/// Environment/default precedence remains implemented by `config::load_config`.
#[derive(Debug, Clone, Default)]
pub struct DaemonLaunchOverrides {
    pub server_url: String,
    pub workspaces_root: String,
    pub poll_interval: Duration,
    pub heartbeat_interval: Duration,
    pub agent_timeout: Option<Duration>,
    pub codex_semantic_inactivity_timeout: Duration,
    pub codex_handshake_timeout: Duration,
    pub max_concurrent_tasks: i64,
    pub daemon_id: String,
    pub device_name: String,
    pub runtime_name: String,
    pub profile: String,
    pub health_port: i32,
    pub allow_no_agents: bool,
    pub disable_auto_update: bool,
    pub auto_update_check_interval: Duration,
    pub disable_auto_reload: bool,
}

/// Profile data read by `cordy-cli` and consumed during daemon assembly.
///
/// This type intentionally does not implement `Debug`: the bearer token must
/// never be emitted by derived diagnostics or structured startup logs.
#[derive(Clone, Default)]
pub struct DaemonProfileInput {
    pub token: String,
    pub profile_command_overrides: BTreeMap<String, String>,
    pub openclaw_binary_path: String,
    pub openclaw_state_dir: String,
    pub openclaw_cli_timeout: String,
}

/// Fully initialized daemon-owned inputs. Constructing this value proves that
/// configuration, authentication, the root marker, and cache location have
/// all passed their production checks.
pub struct DaemonProductionInputs {
    pub config: Config,
    pub client: Arc<Client>,
    pub repo_cache: Arc<Cache>,
}

impl DaemonProductionInputs {
    /// Production constructor used after the CLI has loaded the active
    /// profile. Agent discovery is real and startup fails when no supported
    /// executable is present unless the explicit probe-only escape hatch is
    /// selected.
    pub fn resolve(
        launch: DaemonLaunchOverrides,
        profile: DaemonProfileInput,
        cli_version: impl Into<String>,
        launched_by: impl Into<String>,
    ) -> anyhow::Result<Self> {
        Self::resolve_with_probe(
            launch,
            profile,
            cli_version.into(),
            launched_by.into(),
            &crate::agents_probe::probe_agent_clis,
        )
    }

    fn resolve_with_probe(
        launch: DaemonLaunchOverrides,
        profile: DaemonProfileInput,
        cli_version: String,
        launched_by: String,
        probe_agents: &dyn Fn() -> BTreeMap<String, AgentEntry>,
    ) -> anyhow::Result<Self> {
        if profile.token.is_empty() {
            let login_hint = if launch.profile.is_empty() {
                "'cordy login'".to_string()
            } else {
                format!("'cordy login --profile {}'", launch.profile)
            };
            anyhow::bail!("not authenticated: run {login_hint} first");
        }

        let overrides = Overrides {
            server_url: launch.server_url,
            workspaces_root: launch.workspaces_root,
            poll_interval: launch.poll_interval,
            heartbeat_interval: launch.heartbeat_interval,
            agent_timeout: launch.agent_timeout,
            codex_semantic_inactivity_timeout: launch.codex_semantic_inactivity_timeout,
            codex_handshake_timeout: launch.codex_handshake_timeout,
            max_concurrent_tasks: launch.max_concurrent_tasks,
            daemon_id: launch.daemon_id,
            device_name: launch.device_name,
            runtime_name: launch.runtime_name,
            profile: launch.profile,
            health_port: launch.health_port,
            allow_no_agents: launch.allow_no_agents,
            disable_auto_update: launch.disable_auto_update,
            auto_update_check_interval: launch.auto_update_check_interval,
            disable_auto_reload: launch.disable_auto_reload,
            cli_profile_overrides: Some(CliProfileConfig {
                profile_command_overrides: profile.profile_command_overrides,
                openclaw_binary_path: profile.openclaw_binary_path,
                openclaw_state_dir: profile.openclaw_state_dir,
                openclaw_cli_timeout: profile.openclaw_cli_timeout,
            }),
            cli_version,
            launched_by,
        };
        let config = config::load_config(overrides, probe_agents)?;

        // Fail closed across the entire daemon-owned tree before any provider
        // process can inherit a working directory below it.
        ensure_workspaces_root_marker(&config.workspaces_root)?;

        let client = Arc::new(Client::new(&config.server_base_url));
        client.set_token(&profile.token);
        client.set_version(&config.cli_version);
        let repo_cache = Arc::new(Cache::new(
            std::path::Path::new(&config.workspaces_root).join(".repos"),
        ));
        Ok(Self {
            config,
            client,
            repo_cache,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_agent() -> BTreeMap<String, AgentEntry> {
        BTreeMap::from([(
            "codex".to_string(),
            AgentEntry {
                path: "/bin/codex".to_string(),
                command: "codex".to_string(),
                model: String::new(),
            },
        )])
    }

    #[test]
    fn rejects_missing_auth_before_touching_workspace_root() {
        let root = tempfile::tempdir().unwrap().path().join("not-created");
        let result = DaemonProductionInputs::resolve_with_probe(
            DaemonLaunchOverrides {
                workspaces_root: root.display().to_string(),
                ..DaemonLaunchOverrides::default()
            },
            DaemonProfileInput::default(),
            "1.2.3".to_string(),
            String::new(),
            &one_agent,
        );
        let error = match result {
            Ok(_) => panic!("missing auth unexpectedly assembled production inputs"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("cordy login"));
        assert!(!root.exists());
    }

    #[test]
    fn initializes_authenticated_client_and_daemon_cache() {
        let root = tempfile::tempdir().unwrap().path().join("workspaces");
        let inputs = DaemonProductionInputs::resolve_with_probe(
            DaemonLaunchOverrides {
                server_url: "https://example.test".to_string(),
                workspaces_root: root.display().to_string(),
                allow_no_agents: false,
                ..DaemonLaunchOverrides::default()
            },
            DaemonProfileInput {
                token: "secret-token".to_string(),
                ..DaemonProfileInput::default()
            },
            "1.2.3".to_string(),
            "desktop".to_string(),
            &one_agent,
        )
        .unwrap();

        assert_eq!(inputs.client.token(), "secret-token");
        assert_eq!(inputs.config.cli_version, "1.2.3");
        assert_eq!(inputs.config.launched_by, "desktop");
        assert_eq!(inputs.config.server_base_url, "https://example.test");
        assert!(root.join(".cordy/daemon_task_context.json").is_file());
    }
}
