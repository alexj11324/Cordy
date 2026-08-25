//! Command-facing daemon log inspection.
//!
//! File-tail and follow behavior live together so the CLI keeps one bounded
//! log I/O policy independent of lifecycle and runtime diagnostics.

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::config::Environment;
use super::{require_known_daemon_profile, Cli, DaemonLogsArgs, RunOutput};

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
