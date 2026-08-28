use anyhow::{bail, Result};

use super::config::Environment;

pub(super) fn require_task_local_config_root(environment: &Environment) -> Result<()> {
    if !environment.in_daemon_managed_execution_context()
        || environment
            .trimmed(super::config::TASK_CONFIG_ROOT_ENV)
            .is_some()
    {
        return Ok(());
    }
    let suffix = environment
        .leftover_marker_suffix()
        .unwrap_or_else(|| environment.daemon_port_only_context_hint().into());
    bail!(
        "daemon-managed task requires a task-local Patchbay config root in {}{suffix}",
        super::config::TASK_CONFIG_ROOT_ENV
    )
}

pub(super) fn require_human_local_command(environment: &Environment, command: &str) -> Result<()> {
    if !environment.in_daemon_task_identity_context() {
        return Ok(());
    }
    let suffix = environment.leftover_marker_suffix().unwrap_or_default();
    bail!("{command} is not available inside a daemon-managed task{suffix}")
}
