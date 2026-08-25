//! Daemon launch argument conversion and validation.
//!
//! Lifecycle commands consume these typed inputs; process handoff and output
//! rendering remain in their respective modules.

use anyhow::Result;
use std::time::Duration;

use super::{config, DaemonLaunchArgs};

impl DaemonLaunchArgs {
    pub(super) fn to_launch_flags(&self, server_url: Option<String>) -> config::DaemonLaunchFlags {
        config::DaemonLaunchFlags {
            server_url,
            daemon_id: self.daemon_id.clone(),
            device_name: self.device_name.clone(),
            runtime_name: self.runtime_name.clone(),
            workspaces_root: self.workspaces_root.clone(),
            poll_interval: self.poll_interval,
            heartbeat_interval: self.heartbeat_interval,
            agent_timeout: self.agent_timeout,
            codex_semantic_inactivity_timeout: self.codex_semantic_inactivity_timeout,
            codex_handshake_timeout: self.codex_handshake_timeout,
            max_concurrent_tasks: self.max_concurrent_tasks,
            disable_auto_update: self.disable_auto_update,
            auto_update_check_interval: self.auto_update_interval,
            disable_auto_reload: self.disable_auto_reload,
        }
    }
}

pub(super) fn ensure_restart_is_background(launch: &DaemonLaunchArgs) -> Result<()> {
    anyhow::ensure!(
        !launch.foreground,
        "daemon restart does not support --foreground; use 'daemon start --foreground'"
    );
    Ok(())
}

pub(super) fn validate_daemon_health_port(
    requested: Option<u16>,
    resolved: &cordy_daemon::assembly::DaemonLaunchOverrides,
) -> Result<()> {
    if let Some(health_port) = requested {
        anyhow::ensure!(
            i32::from(health_port) == resolved.health_port,
            "--health-port must match the profile-derived daemon health port ({})",
            resolved.health_port
        );
    }
    Ok(())
}

pub(super) fn parse_cli_duration(value: &str) -> std::result::Result<Duration, String> {
    cordy_daemon::helpers::parse_go_duration(value).map_err(|error| error.to_string())
}
