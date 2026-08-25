//! Daemon lifecycle and diagnostics command dispatch.
//!
//! Start/stop/restart, status/logs, and diagnostics stay behind one daemon
//! boundary while preserving lifecycle argument and probe behavior.

use super::*;

pub(super) async fn run_daemon_command(
    cli: &Cli,
    environment: &Environment,
    args: &DaemonArgs,
) -> Result<RunOutput> {
    match args {
        DaemonArgs {
            command: DaemonCommand::Start(args),
        } => run_daemon_start(cli, environment, args).await,
        DaemonArgs {
            command: DaemonCommand::Status(args),
        } => run_daemon_status(cli, environment, args).await,
        DaemonArgs {
            command: DaemonCommand::Logs(args),
        } => run_daemon_logs(cli, environment, args).await,
        DaemonArgs {
            command: DaemonCommand::Restart(args),
        } => run_daemon_restart(cli, environment, args).await,
        DaemonArgs {
            command: DaemonCommand::Stop,
        } => run_daemon_stop(cli, environment).await,
        DaemonArgs {
            command: DaemonCommand::ProbeRuntimes,
        } => run_daemon_probe_runtimes(cli, environment),
        DaemonArgs {
            command: DaemonCommand::DiskUsage(args),
        } => run_daemon_disk_usage(cli, environment, args).await,
    }
}
