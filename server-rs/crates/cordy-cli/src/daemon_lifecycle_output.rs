//! User-facing rendering for daemon lifecycle outcomes.
//!
//! Process start/stop/restart orchestration remains in
//! `daemon_lifecycle_commands`; this module keeps status/error wording
//! independent from the daemon control client.

use anyhow::{bail, Result};

use super::RunOutput;

pub(super) fn render_daemon_start_outcome(
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

pub(super) fn render_daemon_restart_outcome(
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
