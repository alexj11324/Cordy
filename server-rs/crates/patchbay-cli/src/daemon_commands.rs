//! Command-facing daemon lifecycle operations.
//!
//! The command parser and setup policy stay in `lib.rs`; this module owns the
//! side-effecting lifecycle handoff so background and foreground execution use
//! the same typed `DaemonStartAssembly` snapshot.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::ffi::OsString;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::config::Environment;
use super::{
    config, daemon, dispatch_daemon_after_setup, Cli, DaemonDiskUsageArgs, DaemonLaunchArgs,
    DaemonLogsArgs, DaemonRestartArgs, DaemonStartArgs, DaemonStatusArgs, OutputFormat, RunOutput,
    CLIENT_VERSION,
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
    let executable = std::env::current_exe().context("resolve patchbay executable")?;
    let lifecycle = patchbay_daemon::lifecycle::DaemonLifecycle::assemble(
        start.lifecycle_options(executable, CLIENT_VERSION),
        &start.profile_input,
    )
    .context("assemble setup daemon lifecycle")?;
    let control = patchbay_daemon::control_client::DaemonControlClient::try_new()
        .context("build setup daemon health client")?;
    let health = control.health(lifecycle.port()).await;
    let (daemon_running, active_task_count) = match health {
        patchbay_daemon::control_client::LocalDaemonHealth::Stopped => (false, 0),
        patchbay_daemon::control_client::LocalDaemonHealth::Live(snapshot) => {
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
        let executable = std::env::current_exe().context("resolve patchbay executable")?;
        let lifecycle = patchbay_daemon::lifecycle::DaemonLifecycle::assemble(
            start.lifecycle_options(executable, CLIENT_VERSION),
            &start.profile_input,
        )
        .context("assemble background daemon lifecycle")?;
        let outcome = lifecycle.start().await.context("start daemon")?;
        return render_daemon_start_outcome(outcome);
    }

    let options = start.bootstrap_options();
    let checkout_registry = Arc::new(patchbay_daemon::health::RepoCheckoutRegistry::default());
    patchbay_daemon::assembly::run_production_daemon(options, move |context| {
        start.production_assembly_with_local_catalog(context, CLIENT_VERSION, checkout_registry)
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
    let executable = std::env::current_exe().context("resolve patchbay executable")?;
    let lifecycle = patchbay_daemon::lifecycle::DaemonLifecycle::assemble(
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
    let executable = std::env::current_exe().context("resolve patchbay executable")?;
    let lifecycle = patchbay_daemon::lifecycle::DaemonLifecycle::assemble(
        start.lifecycle_options(executable, CLIENT_VERSION),
        &start.profile_input,
    )
    .context("assemble daemon stop lifecycle")?;
    let outcome = lifecycle.stop().await.context("stop daemon")?;
    match outcome {
        patchbay_daemon::process_control::DaemonStopOutcome::AlreadyStopped => Ok(RunOutput {
            stdout: "daemon already stopped\n".to_string(),
            stderr: String::new(),
        }),
        patchbay_daemon::process_control::DaemonStopOutcome::Stopped { .. } => Ok(RunOutput {
            stdout: "daemon stopped\n".to_string(),
            stderr: String::new(),
        }),
        patchbay_daemon::process_control::DaemonStopOutcome::StillStopping { pid, .. } => {
            bail!("daemon is still stopping (pid {pid}); refusing to report success")
        }
    }
}

pub(crate) async fn run_daemon_status(
    cli: &Cli,
    environment: &Environment,
    args: &DaemonStatusArgs,
) -> Result<RunOutput> {
    let port = resolve_daemon_status_port(cli, environment)?;
    let control = patchbay_daemon::control_client::DaemonControlClient::try_new()
        .context("build daemon health client")?;
    let health = control.health(port).await;
    let conflict = if environment.in_daemon_task_identity_context() {
        None
    } else if let patchbay_daemon::control_client::LocalDaemonHealth::Live(snapshot) = &health {
        snapshot.confirm_profile(&cli.profile, port).err()
    } else {
        None
    };
    render_daemon_status(&cli.profile, args.output, health, conflict)
}

pub(crate) fn resolve_daemon_status_port(cli: &Cli, environment: &Environment) -> Result<u16> {
    if !environment.in_daemon_task_identity_context() {
        require_known_daemon_profile(environment, &cli.profile)?;
        return Ok(patchbay_daemon::control_client::health_port_for_profile(
            &cli.profile,
        ));
    }

    if !cli.profile.is_empty() {
        bail!("daemon status --profile is not available inside a daemon-managed task");
    }
    let raw = environment.trimmed("PATCHBAY_DAEMON_PORT").context(
        "daemon status inside a daemon-managed task requires the daemon-injected PATCHBAY_DAEMON_PORT",
    )?;
    raw.parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .context("invalid PATCHBAY_DAEMON_PORT inside a daemon-managed task")
}

pub(crate) fn require_known_daemon_profile(environment: &Environment, profile: &str) -> Result<()> {
    if profile.is_empty() {
        return Ok(());
    }
    let config_path = environment.config_path(profile)?;
    let profile_dir = config_path
        .parent()
        .context("resolve daemon profile directory")?;
    if profile_dir.is_dir() {
        return Ok(());
    }

    let known = known_daemon_profiles(environment);
    if known.is_empty() {
        bail!("unknown profile {profile:?}: no named profiles exist yet");
    }
    bail!(
        "unknown profile {profile:?}\nKnown profiles: {}",
        known.join(", ")
    );
}

pub(crate) fn known_daemon_profiles(environment: &Environment) -> Vec<String> {
    let Ok(config_path) = environment.config_path("") else {
        return Vec::new();
    };
    let Some(config_dir) = config_path.parent() else {
        return Vec::new();
    };
    let profiles_root = config_dir.join("profiles");
    let mut names = Vec::new();
    collect_daemon_profiles(&profiles_root, Path::new(""), &mut names);
    names.sort();
    names
}

fn collect_daemon_profiles(root: &Path, relative: &Path, names: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let child_relative = relative.join(entry.file_name());
        let child = entry.path();
        if child.join("config.json").is_file() {
            names.push(child_relative.to_string_lossy().replace('\\', "/"));
        }
        collect_daemon_profiles(&child, &child_relative, names);
    }
}

pub(crate) fn render_daemon_status(
    profile: &str,
    output: OutputFormat,
    health: patchbay_daemon::control_client::LocalDaemonHealth,
    conflict: Option<patchbay_daemon::control_client::ProfileMismatch>,
) -> Result<RunOutput> {
    if output == OutputFormat::Json {
        let value = if let Some(conflict) = conflict.as_ref() {
            let port_conflict = match &conflict.actual {
                Some(actual) => serde_json::json!({
                    "port": conflict.port,
                    "profile": actual,
                }),
                None => serde_json::json!({
                    "port": conflict.port,
                    "unreadable_identity": true,
                }),
            };
            serde_json::json!({
                "status": "stopped",
                "port_conflict": port_conflict,
            })
        } else {
            match health {
                patchbay_daemon::control_client::LocalDaemonHealth::Stopped => {
                    serde_json::json!({ "status": "stopped" })
                }
                patchbay_daemon::control_client::LocalDaemonHealth::Live(snapshot) => {
                    serde_json::to_value(snapshot.response)?
                }
            }
        };
        return Ok(RunOutput {
            stdout: format!("{}\n", serde_json::to_string_pretty(&value)?),
            stderr: String::new(),
        });
    }

    let label = daemon_status_label(profile);
    let stdout = if let Some(conflict) = conflict.as_ref() {
        format!(
            "{label}: stopped\n{}\n",
            daemon_status_conflict_note(conflict)
        )
    } else {
        match health {
            patchbay_daemon::control_client::LocalDaemonHealth::Stopped => {
                format!("{label}: stopped\n")
            }
            patchbay_daemon::control_client::LocalDaemonHealth::Live(snapshot) => {
                format_daemon_status_table(&label, &snapshot.response)
            }
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn daemon_status_label(profile: &str) -> String {
    if profile.is_empty() {
        "Daemon".to_string()
    } else {
        format!("Daemon [{profile}]")
    }
}

fn daemon_status_conflict_note(
    conflict: &patchbay_daemon::control_client::ProfileMismatch,
) -> String {
    match &conflict.actual {
        Some(actual) => format!(
            "Note: port {} is serving {:?}, which hashes to the same port.",
            conflict.port, actual
        ),
        None => format!(
            "Note: port {} is serving a daemon whose profile identity could not be read.",
            conflict.port
        ),
    }
}

pub(crate) fn format_daemon_status_table(
    label: &str,
    response: &patchbay_daemon::health::HealthResponse,
) -> String {
    let mut rows = vec![(
        label.to_string(),
        format!(
            "{} (pid {}, uptime {})",
            response.status, response.pid, response.uptime
        ),
    )];
    if !response.cli_version.is_empty() {
        rows.push(("Version".to_string(), response.cli_version.clone()));
    }
    if !response.launched_by.is_empty() {
        let manager = if response.launched_by == "desktop" {
            "Patchbay Desktop app (start and stop it from the app)".to_string()
        } else {
            response.launched_by.clone()
        };
        rows.push(("Managed by".to_string(), manager));
    }
    if !response.reload_pending_reason.is_empty() {
        rows.push((
            "Restart pending".to_string(),
            response.reload_pending_reason.clone(),
        ));
    }
    if !response.agents.is_empty() {
        rows.push(("Agents".to_string(), response.agents.join(", ")));
    }
    rows.push((
        "Workspaces".to_string(),
        response.workspaces.len().to_string(),
    ));

    let width = rows.iter().map(|(key, _)| key.len()).max().unwrap_or(0) + 1;
    let mut output = String::new();
    for (key, value) in rows {
        let key = format!("{key}:");
        let _ = writeln!(output, "{key:<width$}  {value}", width = width);
    }
    output
}

const MAX_LOG_LINES: usize = 100_000;
const MAX_LOG_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn parse_log_lines(value: &str) -> std::result::Result<usize, String> {
    let lines = value
        .parse::<usize>()
        .map_err(|_| "--lines must be a non-negative integer".to_string())?;
    if lines > MAX_LOG_LINES {
        return Err(format!("--lines must be at most {MAX_LOG_LINES}"));
    }
    Ok(lines)
}

pub(crate) async fn run_daemon_logs(
    cli: &Cli,
    environment: &Environment,
    args: &DaemonLogsArgs,
) -> Result<RunOutput> {
    super::require_human_local_command(environment, "daemon logs")?;
    require_known_daemon_profile(environment, &cli.profile)?;
    let log_path = resolve_daemon_log_path(environment, &cli.profile)?;
    match fs::metadata(&log_path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => bail!("daemon log path is not a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "no log file found at {}\nThe daemon may not have been started in background mode",
                log_path.display()
            )
        }
        Err(error) => return Err(error).context("inspect daemon log"),
    }

    let notice = format!(
        "Reading {} (profile: {})\n",
        log_path.display(),
        daemon_profile_label(&cli.profile)
    );
    if args.follow {
        {
            let mut stderr = std::io::stderr().lock();
            stderr
                .write_all(notice.as_bytes())
                .context("write daemon log notice")?;
            stderr.flush().context("flush daemon log notice")?;
        }
        follow_daemon_log(log_path, args.lines).await?;
        return Ok(RunOutput {
            stdout: String::new(),
            stderr: String::new(),
        });
    }

    let bytes = read_daemon_log_tail(&log_path, args.lines)?;
    Ok(RunOutput {
        stdout: String::from_utf8_lossy(&bytes).into_owned(),
        stderr: notice,
    })
}

pub(crate) fn resolve_daemon_log_path(environment: &Environment, profile: &str) -> Result<PathBuf> {
    let config_path = environment.config_path(profile)?;
    let state_dir = config_path
        .parent()
        .context("resolve daemon log directory")?;
    anyhow::ensure!(
        state_dir.is_absolute(),
        "cannot resolve an absolute daemon log path"
    );
    Ok(state_dir.join("daemon.log"))
}

fn daemon_profile_label(profile: &str) -> &str {
    if profile.is_empty() {
        "default"
    } else {
        profile
    }
}

pub(crate) fn read_daemon_log_tail(path: &Path, lines: usize) -> Result<Vec<u8>> {
    if lines == 0 {
        return Ok(Vec::new());
    }
    let mut file = fs::File::open(path).context("open daemon log")?;
    let size = file.metadata().context("stat daemon log")?.len();
    if size == 0 {
        return Ok(Vec::new());
    }

    let mut last_byte = [0_u8; 1];
    file.seek(SeekFrom::Start(size - 1))
        .context("seek daemon log")?;
    file.read_exact(&mut last_byte).context("read daemon log")?;
    let needed_newlines = lines.saturating_add(usize::from(last_byte[0] == b'\n'));

    let mut position = size;
    let mut newline_count = 0_usize;
    let mut tail_start = 0_u64;
    let mut buffer = vec![0_u8; 8192];
    'scan: while position > 0 {
        let chunk_len = position.min(buffer.len() as u64) as usize;
        position -= chunk_len as u64;
        file.seek(SeekFrom::Start(position))
            .context("seek daemon log")?;
        file.read_exact(&mut buffer[..chunk_len])
            .context("read daemon log")?;
        for (index, byte) in buffer[..chunk_len].iter().enumerate().rev() {
            if *byte == b'\n' {
                newline_count += 1;
                if newline_count >= needed_newlines {
                    tail_start = position + index as u64 + 1;
                    break 'scan;
                }
            }
        }
    }

    let start = tail_start.max(size.saturating_sub(MAX_LOG_OUTPUT_BYTES));
    file.seek(SeekFrom::Start(start))
        .context("seek daemon log")?;
    let mut output = Vec::with_capacity((size - start).min(MAX_LOG_OUTPUT_BYTES) as usize);
    file.take(MAX_LOG_OUTPUT_BYTES)
        .read_to_end(&mut output)
        .context("read daemon log tail")?;
    Ok(output)
}

async fn follow_daemon_log(path: PathBuf, lines: usize) -> Result<()> {
    let initial = read_daemon_log_tail(&path, lines)?;
    {
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(&initial)
            .context("write daemon log tail")?;
        stdout.flush().context("flush daemon log tail")?;
    }
    let mut offset = fs::metadata(&path).context("stat daemon log")?.len();
    let mut interrupt = Box::pin(tokio::signal::ctrl_c());

    loop {
        tokio::select! {
            result = &mut interrupt => {
                result.context("wait for daemon log follow cancellation")?;
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }

        let size = match fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("stat daemon log while following"),
        };
        if size < offset {
            offset = 0;
        }
        if size == offset {
            continue;
        }
        let mut file = fs::File::open(&path).context("open daemon log while following")?;
        file.seek(SeekFrom::Start(offset))
            .context("seek daemon log while following")?;
        let mut limited = file.take(size - offset);
        {
            let mut stdout = std::io::stdout().lock();
            std::io::copy(&mut limited, &mut stdout).context("write followed daemon log")?;
            stdout.flush().context("flush followed daemon log")?;
        }
        offset = size;
    }
}

pub(crate) fn run_daemon_probe_runtimes(cli: &Cli, environment: &Environment) -> Result<RunOutput> {
    super::require_human_local_command(environment, "daemon probe-runtimes")?;
    let profile = environment
        .load_config(&cli.profile)
        .context("load daemon probe profile")?;
    let options = profile.daemon_runtime_probe_options(&cli.profile);
    let report =
        patchbay_daemon::runtime_probe::probe_runtimes(options).context("probe local runtimes")?;
    Ok(RunOutput {
        stdout: serde_json::to_string(&report)? + "\n",
        stderr: String::new(),
    })
}

/// The CLI owns argument validation and presentation only. Filesystem
/// traversal and parent-status HTTP semantics remain in the existing helpers;
/// this command boundary keeps the daemon diagnostic entry points together.
pub(crate) async fn run_daemon_disk_usage(
    cli: &Cli,
    environment: &Environment,
    args: &DaemonDiskUsageArgs,
) -> Result<RunOutput> {
    let task_context = super::disk_usage_task_context(environment);
    super::validate_disk_usage_args(cli, environment, args, task_context)?;

    let mut stderr = String::new();
    if args.all_profiles {
        let roots = super::enumerate_disk_usage_roots(environment)?;
        let mut aggregate = patchbay_daemon::diskusage::scan_disk_usage_roots(
            &roots,
            &patchbay_daemon::diskusage::artifact_patterns_from_env(),
        )?;
        if !task_context && super::disk_usage_needs_parent_status(args) {
            let cancellation = tokio_util::sync::CancellationToken::new();
            let enrichment = async {
                let mut failed = false;
                for root in &mut aggregate.roots {
                    failed |= super::fill_disk_usage_parent_statuses(
                        cli,
                        environment,
                        &root.profile,
                        &mut root.report,
                        &cancellation,
                    )
                    .await;
                }
                failed
            };
            if super::with_disk_usage_status_deadline(environment, &cancellation, enrichment).await
            {
                super::append_disk_usage_warning(&mut stderr);
            }
        }
        super::limit_disk_usage_aggregate(&mut aggregate, args);
        let stdout = match args.output {
            OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&aggregate)?),
            OutputFormat::Table => {
                super::format_disk_usage_aggregate_table(&aggregate, args.by_workspace)
            }
        };
        return Ok(RunOutput { stdout, stderr });
    }

    let root = super::resolve_disk_usage_root(cli, environment, args, task_context)?;
    let mut report = patchbay_daemon::diskusage::scan_disk_usage(
        &root,
        &patchbay_daemon::diskusage::artifact_patterns_from_env(),
    )?;
    if !task_context && super::disk_usage_needs_parent_status(args) {
        let cancellation = tokio_util::sync::CancellationToken::new();
        let enrichment = super::fill_disk_usage_parent_statuses(
            cli,
            environment,
            &cli.profile,
            &mut report,
            &cancellation,
        );
        if super::with_disk_usage_status_deadline(environment, &cancellation, enrichment).await {
            super::append_disk_usage_warning(&mut stderr);
        }
    }
    super::limit_disk_usage_report(&mut report, args);
    let stdout = match args.output {
        OutputFormat::Json => format!("{}\n", serde_json::to_string_pretty(&report)?),
        OutputFormat::Table => super::format_disk_usage_report_table(&report, args.by_workspace),
    };
    Ok(RunOutput { stdout, stderr })
}
fn render_daemon_start_outcome(
    outcome: patchbay_daemon::process_control::DaemonStartOutcome,
) -> Result<RunOutput> {
    let stdout = match outcome {
        patchbay_daemon::process_control::DaemonStartOutcome::AlreadyRunning(snapshot) => {
            format!("daemon already running (pid {})\n", snapshot.response.pid)
        }
        patchbay_daemon::process_control::DaemonStartOutcome::Launch(startup) => {
            return render_daemon_startup(startup, "started")
        }
    };
    Ok(RunOutput {
        stdout,
        stderr: String::new(),
    })
}

fn render_daemon_restart_outcome(
    outcome: patchbay_daemon::process_control::DaemonRestartOutcome,
) -> Result<RunOutput> {
    match outcome {
        patchbay_daemon::process_control::DaemonRestartOutcome::StopIncomplete(stop) => {
            match stop {
                patchbay_daemon::process_control::DaemonStopOutcome::StillStopping {
                    pid, ..
                } => {
                    bail!("daemon is still stopping (pid {pid}); refusing to launch a replacement")
                }
                _ => bail!("daemon restart did not complete its stop phase"),
            }
        }
        patchbay_daemon::process_control::DaemonRestartOutcome::Launch { startup, .. } => {
            render_daemon_startup(startup, "restarted")
        }
    }
}

fn render_daemon_startup(
    startup: patchbay_daemon::process_control::BackgroundStartupOutcome,
    verb: &str,
) -> Result<RunOutput> {
    match startup {
        patchbay_daemon::process_control::BackgroundStartupOutcome::Ready { pid, .. } => {
            Ok(RunOutput {
                stdout: format!("daemon {verb} (pid {pid})\n"),
                stderr: String::new(),
            })
        }
        patchbay_daemon::process_control::BackgroundStartupOutcome::Exited {
            pid,
            status,
            logs,
        } => {
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
        patchbay_daemon::process_control::BackgroundStartupOutcome::TimedOut {
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
    resolved: &patchbay_daemon::assembly::DaemonLaunchOverrides,
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
    patchbay_daemon::helpers::parse_go_duration(value).map_err(|error| error.to_string())
}

const DESKTOP_PROFILE_HELPER_ARG: &str = "--patchbay-private-desktop-profile";

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum DesktopProfileRequest {
    Configure {
        profile: String,
        server_url: String,
    },
    SetCredentials {
        profile: String,
        server_url: String,
        token: String,
        user_id: String,
    },
    ClearCredentials {
        profile: String,
    },
}

fn apply_desktop_profile_request(
    environment: &Environment,
    request: DesktopProfileRequest,
) -> Result<()> {
    match request {
        DesktopProfileRequest::Configure {
            profile,
            server_url,
        } => environment.update_desktop_profile(&profile, Some(&server_url), None, None),
        DesktopProfileRequest::SetCredentials {
            profile,
            server_url,
            token,
            user_id,
        } => environment.update_desktop_profile(
            &profile,
            Some(&server_url),
            Some(Some(&token)),
            Some(Some(&user_id)),
        ),
        DesktopProfileRequest::ClearCredentials { profile } => {
            environment.update_desktop_profile(&profile, None, Some(None), Some(None))
        }
    }
}

fn run_desktop_profile_helper_with_environment<I, O>(
    input: I,
    output: &mut O,
    environment: &Environment,
) -> Result<()>
where
    I: Read,
    O: IoWrite,
{
    let request: DesktopProfileRequest =
        serde_json::from_reader(input).context("parse Desktop profile request")?;
    apply_desktop_profile_request(environment, request)?;
    serde_json::to_writer(output, &serde_json::json!({ "ok": true }))
        .context("write Desktop profile response")?;
    Ok(())
}

fn run_desktop_profile_helper<I, O>(input: I, output: &mut O) -> Result<()>
where
    I: Read,
    O: IoWrite,
{
    let environment = Environment::from_process_for_desktop_profile()?;
    run_desktop_profile_helper_with_environment(input, output, &environment)
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
    if args.len() != 2 {
        return Ok(false);
    }
    if args[1] == patchbay_daemon::execenv::isolation::PREPARATION_HELPER_ARG {
        patchbay_daemon::execenv::isolation::run_preparation_helper(input, output).await?;
        return Ok(true);
    }
    if args[1] == DESKTOP_PROFILE_HELPER_ARG {
        run_desktop_profile_helper(input, output)?;
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod desktop_profile_helper_tests {
    use super::*;

    #[test]
    fn private_desktop_profile_protocol_sets_and_clears_credentials() {
        let home = tempfile::tempdir().expect("temp home");
        let cwd = tempfile::tempdir().expect("temp cwd");
        let environment = Environment::for_test(home.path().into(), cwd.path().into());
        let profile = "desktop-api.example.com";
        let mut output = Vec::new();

        run_desktop_profile_helper_with_environment(
            serde_json::to_vec(&serde_json::json!({
                "action": "set_credentials",
                "profile": profile,
                "server_url": "https://api.example.com",
                "token": "pby_fixture",
                "user_id": "user-1"
            }))
            .expect("request"),
            &mut output,
            &environment,
        )
        .expect("set credentials");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output).expect("response")["ok"],
            true
        );
        let configured = environment.load_config(profile).expect("configured profile");
        assert_eq!(configured.server_url, "https://api.example.com");
        assert_eq!(configured.token, "pby_fixture");
        assert_eq!(
            environment
                .load_profile_document(profile)
                .expect("profile document")["desktop_user_id"],
            "user-1"
        );

        output.clear();
        run_desktop_profile_helper_with_environment(
            serde_json::to_vec(&serde_json::json!({
                "action": "clear_credentials",
                "profile": profile
            }))
            .expect("request"),
            &mut output,
            &environment,
        )
        .expect("clear credentials");
        assert!(environment
            .load_config(profile)
            .expect("cleared profile")
            .token
            .is_empty());
        assert!(environment
            .load_profile_document(profile)
            .expect("cleared profile document")
            .get("desktop_user_id")
            .is_none());
    }
}
