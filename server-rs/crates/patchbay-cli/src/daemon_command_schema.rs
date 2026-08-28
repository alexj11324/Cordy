use clap::{Args, Subcommand};

use super::*;

#[derive(Debug, Args)]
pub(super) struct DaemonArgs {
    #[command(subcommand)]
    pub(super) command: DaemonCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum DaemonCommand {
    #[command(about = "Start the production daemon")]
    Start(DaemonStartArgs),
    #[command(about = "Show daemon status")]
    Status(DaemonStatusArgs),
    #[command(about = "Show daemon logs")]
    Logs(DaemonLogsArgs),
    #[command(about = "Restart the production daemon")]
    Restart(DaemonRestartArgs),
    #[command(about = "Stop the production daemon")]
    Stop,
    #[command(
        name = "probe-runtimes",
        about = "Probe locally configured runtimes",
        hide = true
    )]
    ProbeRuntimes,
    #[command(about = "Show local daemon workspace disk usage")]
    DiskUsage(DaemonDiskUsageArgs),
}

#[derive(Debug, Args)]
pub(super) struct DaemonStartArgs {
    #[command(flatten)]
    pub(super) launch: DaemonLaunchArgs,
}

#[derive(Debug, Args)]
pub(super) struct DaemonStatusArgs {
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
}

#[derive(Debug, Args)]
pub(super) struct DaemonLogsArgs {
    #[arg(short = 'f', long, help = "Follow the log file as it grows")]
    pub(super) follow: bool,
    #[arg(
        short = 'n',
        long,
        default_value_t = 50,
        value_parser = parse_log_lines,
        help = "Number of recent log lines to show"
    )]
    pub(super) lines: usize,
}

#[derive(Debug, Args)]
pub(super) struct DaemonRestartArgs {
    #[command(flatten)]
    pub(super) launch: DaemonLaunchArgs,
}

#[derive(Debug, Args)]
pub(super) struct DaemonDiskUsageArgs {
    #[arg(long, help = "Aggregate output by workspace instead of by task")]
    pub(super) by_workspace: bool,
    #[arg(long, help = "Use the per-task view (default)")]
    pub(super) by_task: bool,
    #[arg(long, default_value_t = 0, help = "Keep only the largest N entries")]
    pub(super) top: i64,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(super) output: OutputFormat,
    #[arg(long, help = "Override the workspaces root path")]
    pub(super) workspaces_root: Option<String>,
    #[arg(long, help = "Scan the default root and every named profile root")]
    pub(super) all_profiles: bool,
}

/// Launch flags shared by `daemon start` and `daemon restart`.
///
/// Restart remains a background lifecycle operation, but it must resolve the
/// same launch contract as start so the replacement process cannot silently
/// inherit a different daemon identity, workspace root, timeout, or reload
/// policy. The root `--server-url`/`--profile` options remain global and are
/// included by `to_launch_flags` below.
#[derive(Debug, Args)]
pub(super) struct DaemonLaunchArgs {
    /// Run the daemon in the current process. Without this flag the command
    /// uses the typed lifecycle owner to launch the real foreground child.
    #[arg(long)]
    pub(super) foreground: bool,
    #[arg(long)]
    pub(super) daemon_id: Option<String>,
    #[arg(long)]
    pub(super) device_name: Option<String>,
    #[arg(long)]
    pub(super) runtime_name: Option<String>,
    #[arg(long)]
    pub(super) workspaces_root: Option<String>,
    #[arg(long, value_parser = parse_cli_duration)]
    pub(super) poll_interval: Option<Duration>,
    #[arg(long, value_parser = parse_cli_duration)]
    pub(super) heartbeat_interval: Option<Duration>,
    #[arg(long, value_parser = parse_cli_duration)]
    pub(super) agent_timeout: Option<Duration>,
    #[arg(long, value_parser = parse_cli_duration)]
    pub(super) codex_semantic_inactivity_timeout: Option<Duration>,
    #[arg(long, value_parser = parse_cli_duration)]
    pub(super) codex_handshake_timeout: Option<Duration>,
    #[arg(long)]
    pub(super) max_concurrent_tasks: Option<i64>,
    /// Successor invocations include the profile-derived health port. It is
    /// validated against the canonical resolver rather than becoming a
    /// second source of daemon configuration.
    #[arg(long)]
    pub(super) health_port: Option<u16>,
    #[arg(long = "no-auto-update")]
    pub(super) disable_auto_update: bool,
    #[arg(long, value_parser = parse_cli_duration)]
    pub(super) auto_update_interval: Option<Duration>,
    #[arg(long = "no-auto-reload")]
    pub(super) disable_auto_reload: bool,
}
