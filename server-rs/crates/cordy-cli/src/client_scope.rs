//! Workspace identity and task-context scope resolution.
//!
//! This module owns the CLI's flag/environment/profile precedence for
//! workspace identity and the fail-closed task-context requirements.

use anyhow::{bail, Result};

use crate::{config, Cli, Environment};

pub(super) fn resolve_current_workspace_id(cli: &Cli, environment: &Environment) -> String {
    let task_context = environment.in_daemon_managed_execution_context();
    let may_read_config =
        !task_context || environment.trimmed(config::TASK_CONFIG_ROOT_ENV).is_some();
    let config = if may_read_config {
        environment.load_config(&cli.profile).unwrap_or_default()
    } else {
        config::CliConfig::default()
    };
    resolve_workspace_id(cli, environment, task_context, &config)
}

pub(super) fn required_workspace_id(cli: &Cli, environment: &Environment) -> Result<String> {
    let workspace_id = resolve_current_workspace_id(cli, environment);
    if workspace_id.is_empty() {
        if environment.in_daemon_managed_execution_context() {
            bail!(
                "workspace_id is required: CORDY_WORKSPACE_ID must be set by the daemon in agent execution context (no fallback to user config)"
            );
        }
        bail!(
            "workspace_id is required: use --workspace-id flag, set CORDY_WORKSPACE_ID env, or run 'cordy config set workspace_id <id>'"
        );
    }
    Ok(workspace_id)
}

pub(super) fn resolve_workspace_id(
    cli: &Cli,
    environment: &Environment,
    task_context: bool,
    config: &config::CliConfig,
) -> String {
    match cli.workspace_id.as_deref() {
        Some(value) if !value.is_empty() => value.into(),
        // An explicitly empty flag suppresses the environment, just like
        // Cobra's Changed branch, then falls through to profile config.
        Some(_) => {
            if task_context {
                String::new()
            } else {
                config.workspace_id.clone()
            }
        }
        None => environment
            .trimmed("CORDY_WORKSPACE_ID")
            .map(Into::into)
            .or_else(|| (!task_context).then(|| config.workspace_id.clone()))
            .unwrap_or_default(),
    }
}
