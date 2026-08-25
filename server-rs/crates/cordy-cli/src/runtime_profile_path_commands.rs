use anyhow::{bail, Context, Result};
use std::path::Path;

use super::{require_human_local_command, Cli, Environment, RunOutput};

pub(super) fn run_runtime_profile_set_path(
    cli: &Cli,
    environment: &Environment,
    profile_id: &str,
    path: Option<&str>,
) -> Result<RunOutput> {
    require_human_local_command(environment, "runtime profile set-path")?;
    let path = path
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .context("--path is required")?;
    if !Path::new(path).is_absolute() {
        bail!("--path must be an absolute path, got {path:?}");
    }
    environment
        .set_profile_command_override(&cli.profile, profile_id, Some(path))
        .context("save CLI config")?;
    Ok(RunOutput {
        stdout: format!(
            "Pinned runtime profile {profile_id} to {path} on this machine.\nRestart the daemon for the change to take effect.\n"
        ),
        stderr: String::new(),
    })
}

pub(super) fn run_runtime_profile_unset_path(
    cli: &Cli,
    environment: &Environment,
    profile_id: &str,
) -> Result<RunOutput> {
    require_human_local_command(environment, "runtime profile unset-path")?;
    let changed = environment
        .set_profile_command_override(&cli.profile, profile_id, None)
        .context("save CLI config")?;
    Ok(RunOutput {
        stdout: if changed {
            format!(
                "Removed per-machine path override for runtime profile {profile_id}.\nRestart the daemon for the change to take effect.\n"
            )
        } else {
            format!("No per-machine path override set for runtime profile {profile_id}.\n")
        },
        stderr: String::new(),
    })
}
