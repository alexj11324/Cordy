//! Bounded daemon log tailing and follow I/O.
//!
//! The command layer owns profile/path policy; this module owns only the
//! bounded file reads and cancellable follow loop.

use anyhow::{Context, Result};
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_LOG_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

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

pub(crate) async fn follow_daemon_log(path: PathBuf, lines: usize) -> Result<()> {
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
