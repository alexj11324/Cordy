//! Command-facing daemon lifecycle operations.
//!
//! The command parser and setup policy stay in `lib.rs`; this module owns the
//! side-effecting lifecycle handoff so background and foreground execution use
//! the same typed `DaemonStartAssembly` snapshot.

use anyhow::{bail, Context, Result};
use std::sync::Arc;

use super::config::Environment;
use super::daemon_launch_inputs::{
    ensure_restart_is_background, parse_cli_duration, validate_daemon_health_port,
};
use super::daemon_lifecycle_output::{render_daemon_restart_outcome, render_daemon_start_outcome};
use super::{
    config, daemon, dispatch_daemon_after_setup, Cli, DaemonRestartArgs, DaemonStartArgs,
    RunOutput, CLIENT_VERSION,
};

pub(crate) async fn run_daemon_after_setup(
    cli: &Cli,
    environment: &Environment,
) -> Result<RunOutput> {
    let flags = config::DaemonLaunchFlags {
        server_url: cli.server_url.clone(),
        ..config::DaemonLaunchFlags::default()
    };
    let start = daemon::DaemonStartAssembly::load(&cli.profile, &flags, environment)
        .context("load setup daemon profile")?;
    let executable = std::env::current_exe().context("resolve cordy executable")?;
    let lifecycle = cordy_daemon::lifecycle::DaemonLifecycle::assemble(
        start.lifecycle_options(executable, CLIENT_VERSION),
        &start.profile_input,
    )
    .context("assemble setup daemon lifecycle")?;
    let control = cordy_daemon::control_client::DaemonControlClient::try_new()
        .context("build setup daemon health client")?;
    let health = control.health(lifecycle.port()).await;
    let (daemon_running, active_task_count) = match health {
        cordy_daemon::control_client::LocalDaemonHealth::Stopped => (false, 0),
        cordy_daemon::control_client::LocalDaemonHealth::Live(snapshot) => {
            snapshot
                .confirm_profile(&cli.profile, lifecycle.port())
                .context("setup daemon health profile mismatch")?;
            (true, snapshot.response.active_task_count)
        }
    };
    let action = super::setup_daemon_action(daemon_running, active_task_count);
    dispatch_daemon_after_setup(
        action,
        || async {
            let outcome = lifecycle
                .start()
                .await
                .context("start daemon after setup")?;
            render_daemon_start_outcome(outcome)
        },
        || async {
            let outcome = lifecycle
                .restart()
                .await
                .context("restart daemon after setup")?;
            render_daemon_restart_outcome(outcome)
        },
    )
    .await
}

pub(crate) async fn run_daemon_start(
    cli: &Cli,
    environment: &Environment,
    args: &DaemonStartArgs,
) -> Result<RunOutput> {
    let launch = &args.launch;
    let launch_flags = launch.to_launch_flags(cli.server_url.clone());
    let start = daemon::DaemonStartAssembly::load(&cli.profile, &launch_flags, environment)
        .context("load daemon start profile")?;
    validate_daemon_health_port(launch.health_port, &start.launch)?;

    if !launch.foreground {
        let executable = std::env::current_exe().context("resolve cordy executable")?;
        let lifecycle = cordy_daemon::lifecycle::DaemonLifecycle::assemble(
            start.lifecycle_options(executable, CLIENT_VERSION),
            &start.profile_input,
        )
        .context("assemble background daemon lifecycle")?;
        let outcome = lifecycle.start().await.context("start daemon")?;
        return render_daemon_start_outcome(outcome);
    }

    let options = start.bootstrap_options();
    let checkout_registry = Arc::new(cordy_daemon::health::RepoCheckoutRegistry::default());
    cordy_daemon::assembly::run_production_daemon(options, move |context| {
        start.production_assembly_with_local_catalog(&context, CLIENT_VERSION, checkout_registry)
    })
    .await
    .context("run foreground daemon")?;

    Ok(RunOutput {
        stdout: String::new(),
        stderr: String::new(),
    })
}

pub(crate) async fn run_daemon_restart(
    cli: &Cli,
    environment: &Environment,
    args: &DaemonRestartArgs,
) -> Result<RunOutput> {
    super::require_human_local_command(environment, "daemon restart")?;
    ensure_restart_is_background(&args.launch)?;
    let flags = args.launch.to_launch_flags(cli.server_url.clone());
    let start = daemon::DaemonStartAssembly::load(&cli.profile, &flags, environment)
        .context("load daemon restart profile")?;
    validate_daemon_health_port(args.launch.health_port, &start.launch)?;
    let executable = std::env::current_exe().context("resolve cordy executable")?;
    let lifecycle = cordy_daemon::lifecycle::DaemonLifecycle::assemble(
        start.lifecycle_options(executable, CLIENT_VERSION),
        &start.profile_input,
    )
    .context("assemble daemon restart lifecycle")?;
    let outcome = lifecycle.restart().await.context("restart daemon")?;
    render_daemon_restart_outcome(outcome)
}

pub(crate) async fn run_daemon_stop(cli: &Cli, environment: &Environment) -> Result<RunOutput> {
    super::require_human_local_command(environment, "daemon stop")?;
    let flags = config::DaemonLaunchFlags {
        server_url: cli.server_url.clone(),
        ..config::DaemonLaunchFlags::default()
    };
    let start = daemon::DaemonStartAssembly::load_for_control(&cli.profile, &flags, environment)
        .context("load daemon stop profile")?;
    let executable = std::env::current_exe().context("resolve cordy executable")?;
    let lifecycle = cordy_daemon::lifecycle::DaemonLifecycle::assemble(
        start.lifecycle_options(executable, CLIENT_VERSION),
        &start.profile_input,
    )
    .context("assemble daemon stop lifecycle")?;
    let outcome = lifecycle.stop().await.context("stop daemon")?;
    match outcome {
        cordy_daemon::process_control::DaemonStopOutcome::AlreadyStopped => Ok(RunOutput {
            stdout: "daemon already stopped\n".to_string(),
            stderr: String::new(),
        }),
        cordy_daemon::process_control::DaemonStopOutcome::Stopped { .. } => Ok(RunOutput {
            stdout: "daemon stopped\n".to_string(),
            stderr: String::new(),
        }),
        cordy_daemon::process_control::DaemonStopOutcome::StillStopping { pid, .. } => {
            bail!("daemon is still stopping (pid {pid}); refusing to report success")
        }
    }
}
