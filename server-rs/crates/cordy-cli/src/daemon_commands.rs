//! Command-facing daemon lifecycle operations.
//!
//! The command parser and setup policy stay in `lib.rs`; this module owns the
//! side-effecting lifecycle handoff so background and foreground execution use
//! the same typed `DaemonStartAssembly` snapshot.

use anyhow::{bail, Context, Result};
use std::ffi::OsString;
use std::io::{Read, Write as IoWrite};
use std::sync::Arc;
use std::time::Duration;

use super::config::Environment;
use super::{
    config, daemon, dispatch_daemon_after_setup, Cli, DaemonLaunchArgs, DaemonRestartArgs,
    DaemonStartArgs, RunOutput, CLIENT_VERSION,
};

impl super::DaemonLaunchArgs {
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



fn render_daemon_start_outcome(
    outcome: cordy_daemon::process_control::DaemonStartOutcome,
) -> Result<RunOutput> {
    let stdout = match outcome {
        cordy_daemon::process_control::DaemonStartOutcome::AlreadyRunning(snapshot) => {
            format!("daemon already running (pid {})\n", snapshot.response.pid)
        }
        cordy_daemon::process_control::DaemonStartOutcome::Launch(startup) => {
            return render_daemon_startup(startup, "started")
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn render_daemon_restart_outcome(
    outcome: cordy_daemon::process_control::DaemonRestartOutcome,
) -> Result<RunOutput> {
    match outcome {
        cordy_daemon::process_control::DaemonRestartOutcome::StopIncomplete(stop) => match stop {
            cordy_daemon::process_control::DaemonStopOutcome::StillStopping { pid, .. } => {
                bail!("daemon is still stopping (pid {pid}); refusing to launch a replacement")
            }
            _ => bail!("daemon restart did not complete its stop phase"),
        },
        cordy_daemon::process_control::DaemonRestartOutcome::Launch { startup, .. } => {
            render_daemon_startup(startup, "restarted")
        }
    }
}

fn render_daemon_startup(
    startup: cordy_daemon::process_control::BackgroundStartupOutcome,
    verb: &str,
) -> Result<RunOutput> {
    match startup {
        cordy_daemon::process_control::BackgroundStartupOutcome::Ready { pid, .. } => {
            Ok(RunOutput {
                stdout: format!("daemon {verb} (pid {pid})\n"),
                stderr: String::new(),
            })
        }
        cordy_daemon::process_control::BackgroundStartupOutcome::Exited { pid, status, logs } => {
            let evidence = logs.failure_evidence(8);
            let detail = evidence
                .structured_lines
                .into_iter()
                .chain(evidence.crash_lines)
                .collect::<Vec<_>>()
                .join("; ");
            if detail.is_empty() {
                bail!("daemon {verb} child exited before readiness (pid {pid}, status {status})")
            }
            bail!(
                "daemon {verb} child exited before readiness (pid {pid}, status {status}): {detail}"
            )
        }
        cordy_daemon::process_control::BackgroundStartupOutcome::TimedOut {
            pid,
            last_status,
            ..
        } => {
            let status = last_status.unwrap_or_else(|| "unknown".to_string());
            bail!("daemon {verb} timed out before readiness (pid {pid}, status {status})")
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
/// Handles the daemon's private execution-environment helper mode before
/// normal CLI parsing or profile loading. The protocol never places task
/// configuration or gateway credentials in argv; all payload data stays on
/// the inherited stdin/stdout pipes.
pub async fn run_private_helper<I, O>(args: &[OsString], input: I, output: &mut O) -> Result<bool>
where
    I: Read,
    O: IoWrite,
{
    if args.len() != 2
        || args[1] != OsString::from(cordy_daemon::execenv::isolation::PREPARATION_HELPER_ARG)
    {
        return Ok(false);
    }
    cordy_daemon::execenv::isolation::run_preparation_helper(input, output).await?;
    Ok(true)
}
