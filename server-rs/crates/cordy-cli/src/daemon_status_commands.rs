//! Command-facing daemon health/status inspection.
//!
//! Profile resolution and status presentation stay together so lifecycle
//! commands can reuse the same profile validation without owning output code.

use anyhow::{bail, Context, Result};

use super::config::Environment;
use super::daemon_profile_discovery::require_known_daemon_profile;
use super::daemon_status_output::render_daemon_status;
use super::{Cli, DaemonStatusArgs, OutputFormat, RunOutput};

pub(crate) async fn run_daemon_status(
    cli: &Cli,
    environment: &Environment,
    args: &DaemonStatusArgs,
) -> Result<RunOutput> {
    let port = resolve_daemon_status_port(cli, environment)?;
    let control = cordy_daemon::control_client::DaemonControlClient::try_new()
        .context("build daemon health client")?;
    let health = control.health(port).await;
    let conflict = if environment.in_daemon_task_identity_context() {
        None
    } else if let cordy_daemon::control_client::LocalDaemonHealth::Live(snapshot) = &health {
        snapshot.confirm_profile(&cli.profile, port).err()
    } else {
        None
    };
    render_daemon_status(&cli.profile, args.output, health, conflict)
}

pub(crate) fn resolve_daemon_status_port(cli: &Cli, environment: &Environment) -> Result<u16> {
    if !environment.in_daemon_task_identity_context() {
        require_known_daemon_profile(environment, &cli.profile)?;
        return Ok(cordy_daemon::control_client::health_port_for_profile(
            &cli.profile,
        ));
    }

    if !cli.profile.is_empty() {
        bail!("daemon status --profile is not available inside a daemon-managed task");
    }
    let raw = environment.trimmed("CORDY_DAEMON_PORT").context(
        "daemon status inside a daemon-managed task requires the daemon-injected CORDY_DAEMON_PORT",
    )?;
    raw.parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .context("invalid CORDY_DAEMON_PORT inside a daemon-managed task")
}
