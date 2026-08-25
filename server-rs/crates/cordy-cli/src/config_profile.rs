//! Profile schema and daemon launch resolution.
//!
//! This module owns the persisted profile shape and the CLI's
//! flag > environment > profile > daemon-default resolution layer. Keeping
//! those concerns together makes the precedence contract reviewable without
//! coupling it to filesystem locking or environment capture.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration;

use super::Environment;

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

/// Resolves the CLI-owned `flag > env > profile > daemon default` layer.
///
/// Most environment values are intentionally represented by an empty/zero
/// output: [`cordy_daemon::assembly::DaemonProductionInputs`] reads the same
/// process environment through the authoritative daemon config loader. The
/// server URL is the exception because background authenticated preflight
/// consumes it before the foreground loader runs.
pub fn resolve_daemon_launch_overrides(
    profile: &str,
    flags: &DaemonLaunchFlags,
    environment: &Environment,
    config: &CliConfig,
) -> Result<cordy_daemon::assembly::DaemonLaunchOverrides> {
    let poll_interval = resolve_positive_duration(
        flags.poll_interval,
        environment,
        "CORDY_DAEMON_POLL_INTERVAL",
        &config.poll_interval,
    )?;
    let heartbeat_interval = resolve_positive_duration(
        flags.heartbeat_interval,
        environment,
        "CORDY_DAEMON_HEARTBEAT_INTERVAL",
        &config.heartbeat_interval,
    )?;
    let codex_semantic_inactivity_timeout = resolve_positive_duration(
        flags.codex_semantic_inactivity_timeout,
        environment,
        "CORDY_CODEX_SEMANTIC_INACTIVITY_TIMEOUT",
        &config.codex_semantic_inactivity_timeout,
    )?;
    let codex_handshake_timeout = resolve_positive_duration(
        flags.codex_handshake_timeout,
        environment,
        "CORDY_CODEX_HANDSHAKE_TIMEOUT",
        &config.codex_handshake_timeout,
    )?;
    let auto_update_check_interval = resolve_positive_duration(
        flags.auto_update_check_interval,
        environment,
        "CORDY_DAEMON_AUTO_UPDATE_INTERVAL",
        &config.auto_update_check_interval,
    )?;
    let agent_timeout = resolve_agent_timeout(flags.agent_timeout, environment, config)?;

    Ok(cordy_daemon::assembly::DaemonLaunchOverrides {
        // Unlike every other environment-owned field, the server URL is also
        // consumed by the background lifecycle preflight before the child
        // exists. Carry its effective value so preflight and foreground never
        // target different servers.
        server_url: resolve_effective_string(
            flags.server_url.as_deref(),
            environment,
            "CORDY_SERVER_URL",
            &config.server_url,
        ),
        workspaces_root: resolve_string(
            flags.workspaces_root.as_deref(),
            environment,
            "CORDY_WORKSPACES_ROOT",
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
            "CORDY_DAEMON_MAX_CONCURRENT_TASKS",
            config.max_concurrent_tasks,
        ),
        daemon_id: resolve_string(
            flags.daemon_id.as_deref(),
            environment,
            "CORDY_DAEMON_ID",
            "",
        ),
        device_name: resolve_string(
            flags.device_name.as_deref(),
            environment,
            "CORDY_DAEMON_DEVICE_NAME",
            &config.device_name,
        ),
        runtime_name: resolve_string(
            flags.runtime_name.as_deref(),
            environment,
            "CORDY_AGENT_RUNTIME_NAME",
            &config.runtime_name,
        ),
        profile: profile.to_string(),
        health_port: i32::from(cordy_daemon::control_client::health_port_for_profile(
            profile,
        )),
        allow_no_agents: false,
        disable_auto_update: resolve_disable_signal(
            flags.disable_auto_update,
            environment,
            "CORDY_DAEMON_AUTO_UPDATE",
            config.disable_auto_update,
        ),
        auto_update_check_interval,
        disable_auto_reload: resolve_disable_signal(
            flags.disable_auto_reload,
            environment,
            "CORDY_DAEMON_AUTO_RELOAD",
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
    let parsed = cordy_daemon::helpers::parse_go_duration(persisted).with_context(|| {
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
    const ENV_KEY: &str = "CORDY_AGENT_TIMEOUT";
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
    cordy_daemon::helpers::parse_go_duration(persisted)
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
