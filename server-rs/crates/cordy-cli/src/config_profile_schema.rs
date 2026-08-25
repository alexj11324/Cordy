//! Persisted profile data and raw daemon launch flags.
//!
//! Precedence and validation live in `config_profile_resolution`; this module
//! contains only the stable data shapes shared by config, setup, and daemon
//! assembly.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration;

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
    pub fn daemon_profile_input(&self) -> cordy_daemon::assembly::DaemonProfileInput {
        let openclaw = self
            .backends
            .as_ref()
            .and_then(|backends| backends.openclaw.as_ref());
        cordy_daemon::assembly::DaemonProfileInput {
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
    ) -> cordy_daemon::runtime_probe::RuntimeProbeOptions {
        let openclaw = self
            .backends
            .as_ref()
            .and_then(|backends| backends.openclaw.as_ref());
        cordy_daemon::runtime_probe::RuntimeProbeOptions {
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
