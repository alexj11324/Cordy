//! Cross-platform background process ownership for CLI daemon start.
//!
//! This is deliberately separate from the foreground bootstrap owner: the
//! child acquires and writes the authoritative PID lock itself. The launcher
//! only keeps the process handle long enough to detect pre-readiness exits.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, ExitStatus, Stdio};

use anyhow::Context;

use crate::bootstrap::{open_bounded_crash_log, ProfileStatePaths};
use crate::update_executor::{
    is_access_denied_spawn_error, restart_command, restart_command_after_access_denied,
};

#[derive(Debug, Clone)]
pub struct BackgroundLaunchOptions {
    pub profile: String,
    pub binary: PathBuf,
    pub args: Vec<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupLogCursor {
    pub structured_log: PathBuf,
    pub structured_offset: u64,
    pub crash_log: PathBuf,
    pub crash_offset: u64,
}

/// Owned early-start process handle. Dropping or explicitly detaching this
/// value never kills the daemon; `try_wait` is available while readiness is
/// polled so a failed preflight can be reported immediately.
pub struct BackgroundDaemon {
    child: Child,
    logs: StartupLogCursor,
}

impl BackgroundDaemon {
    pub fn spawn(options: BackgroundLaunchOptions) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !options.binary.as_os_str().is_empty(),
            "daemon executable path is empty"
        );
        let paths = ProfileStatePaths::resolve(&options.profile)?;
        fs::create_dir_all(&paths.directory).context("create daemon profile directory")?;
        let structured_offset = file_length(&paths.structured_log);
        let crash = open_bounded_crash_log(&paths.crash_log)
            .with_context(|| format!("open daemon crash log {}", paths.crash_log.display()))?;
        // Read after opening because the bounded sink may have rolled an old
        // file. The cursor must describe only this launch attempt.
        let crash_offset = crash.metadata().map_or(0, |metadata| metadata.len());

        let mut command = restart_command(&options.binary, &options.args);
        configure_stdio(&mut command, &crash)?;
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) if is_access_denied_spawn_error(&error) => {
                let mut retry = restart_command_after_access_denied(&options.binary, &options.args);
                configure_stdio(&mut retry, &crash)?;
                retry
                    .spawn()
                    .context("start daemon without Windows job breakaway")?
            }
            Err(error) => return Err(error).context("start daemon"),
        };
        Ok(Self {
            child,
            logs: StartupLogCursor {
                structured_log: paths.structured_log,
                structured_offset,
                crash_log: paths.crash_log,
                crash_offset,
            },
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn logs(&self) -> &StartupLogCursor {
        &self.logs
    }

    pub fn try_wait(&mut self) -> anyhow::Result<Option<ExitStatus>> {
        self.child.try_wait().context("observe daemon startup")
    }

    /// Releases launcher ownership after health reports `running`. Dropping a
    /// `std::process::Child` handle does not terminate the detached process.
    pub fn detach(self) -> u32 {
        self.child.id()
    }
}

fn configure_stdio(command: &mut std::process::Command, crash: &fs::File) -> anyhow::Result<()> {
    command.stdin(Stdio::null());
    command.stdout(Stdio::from(
        crash.try_clone().context("clone daemon stdout sink")?,
    ));
    command.stderr(Stdio::from(
        crash.try_clone().context("clone daemon stderr sink")?,
    ));
    Ok(())
}

fn file_length(path: &std::path::Path) -> u64 {
    fs::metadata(path).map_or(0, |metadata| metadata.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_logs_start_at_zero() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(file_length(&directory.path().join("missing.log")), 0);
    }
}
