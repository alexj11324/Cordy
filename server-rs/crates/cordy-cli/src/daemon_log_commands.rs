//! Command-facing daemon log inspection.
//!
//! File-tail and follow behavior live together so the CLI keeps one bounded
//! log I/O policy independent of lifecycle and runtime diagnostics.

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::Write as IoWrite;
use std::path::PathBuf;

use super::config::Environment;
use super::daemon_log_io::{follow_daemon_log, read_daemon_log_tail};
use super::{require_known_daemon_profile, Cli, DaemonLogsArgs, RunOutput};

const MAX_LOG_LINES: usize = 100_000;

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
