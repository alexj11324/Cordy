//! CLI flag/environment/profile precedence and daemon launch validation.

use anyhow::{Context, Result};
use std::time::Duration;

use crate::{config::Environment, config_profile_schema::*};

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
