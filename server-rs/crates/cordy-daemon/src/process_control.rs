//! Cross-platform background process ownership for CLI daemon start.
//!
//! This is deliberately separate from the foreground bootstrap owner: the
//! child acquires and writes the authoritative PID lock itself. The launcher
//! only keeps the process handle long enough to detect pre-readiness exits.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::Context;

use crate::bootstrap::{open_bounded_crash_log, ProfileStatePaths};
use crate::control_client::{DaemonHealthSnapshot, LocalDaemonHealth, LocalDaemonProbe};
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

pub trait ProcessTerminator: Send + Sync {
    fn force_kill(&self, pid: u32) -> anyhow::Result<()>;
}

#[derive(Debug, Default)]
pub struct SystemProcessTerminator;

impl ProcessTerminator for SystemProcessTerminator {
    fn force_kill(&self, pid: u32) -> anyhow::Result<()> {
        anyhow::ensure!(pid > 0, "daemon PID is zero");
        force_kill_pid(pid).with_context(|| format!("kill daemon process {pid}"))
    }
}

const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[async_trait::async_trait]
pub trait StartupClock: Send + Sync {
    fn now(&self) -> Instant;
    async fn sleep(&self, duration: Duration);
}

#[derive(Debug, Default)]
pub struct SystemStartupClock;

#[async_trait::async_trait]
impl StartupClock for SystemStartupClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

#[derive(Debug)]
pub enum BackgroundStartupOutcome {
    Ready {
        pid: u32,
        health: DaemonHealthSnapshot,
    },
    Exited {
        pid: u32,
        status: ExitStatus,
        logs: StartupLogCursor,
    },
    TimedOut {
        pid: u32,
        last_status: Option<String>,
        logs: StartupLogCursor,
    },
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

    /// Waits for production readiness while retaining the child handle for
    /// early-exit observation. Timeout releases ownership without killing the
    /// still-starting daemon, matching the Go CLI's cold-preflight behavior.
    pub async fn wait_until_ready<P: LocalDaemonProbe, C: StartupClock>(
        mut self,
        probe: &P,
        clock: &C,
        profile: &str,
        port: u16,
        timeout: Duration,
    ) -> anyhow::Result<BackgroundStartupOutcome> {
        anyhow::ensure!(!timeout.is_zero(), "daemon startup timeout is zero");
        let deadline = clock
            .now()
            .checked_add(timeout)
            .context("daemon startup deadline overflow")?;
        let mut last_status = None;
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(BackgroundStartupOutcome::Exited {
                    pid: self.pid(),
                    status,
                    logs: self.logs.clone(),
                });
            }
            let now = clock.now();
            if now >= deadline {
                return Ok(BackgroundStartupOutcome::TimedOut {
                    pid: self.pid(),
                    last_status,
                    logs: self.logs.clone(),
                });
            }
            clock.sleep(STARTUP_POLL_INTERVAL.min(deadline - now)).await;
            match probe.health(port).await {
                LocalDaemonHealth::Stopped => {}
                LocalDaemonHealth::Live(snapshot) => {
                    snapshot.confirm_profile(profile, port)?;
                    last_status = Some(snapshot.response.status.clone());
                    if snapshot.response.status == "running" {
                        let pid = self.detach();
                        return Ok(BackgroundStartupOutcome::Ready {
                            pid,
                            health: snapshot,
                        });
                    }
                }
            }
        }
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

#[cfg(unix)]
fn force_kill_pid(pid: u32) -> std::io::Result<()> {
    let pid = i32::try_from(pid)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "PID exceeds i32"))?;
    // SAFETY: `pid` is range-checked and `kill` does not retain pointers.
    if unsafe { libc::kill(pid, libc::SIGKILL) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn force_kill_pid(pid: u32) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    // SAFETY: the numeric PID is supplied by the identity-checked health
    // response; the returned owned handle is closed on every success path.
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `handle` is non-null and was opened with PROCESS_TERMINATE.
    let terminated = unsafe { TerminateProcess(handle, 1) };
    let error = if terminated == 0 {
        Some(std::io::Error::last_os_error())
    } else {
        None
    };
    // SAFETY: this is the single close of the owned handle above.
    unsafe { CloseHandle(handle) };
    match error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_logs_start_at_zero() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(file_length(&directory.path().join("missing.log")), 0);
    }

    #[test]
    fn profile_mismatch_remains_a_typed_startup_error() {
        let error: anyhow::Error = crate::control_client::ProfileMismatch {
            expected: "ab".to_string(),
            actual: Some("ba".to_string()),
            port: 19710,
        }
        .into();
        assert!(error
            .downcast_ref::<crate::control_client::ProfileMismatch>()
            .is_some());
    }

    #[test]
    fn refuses_to_kill_zero_pid() {
        let error = SystemProcessTerminator.force_kill(0).unwrap_err();
        assert!(error.to_string().contains("PID is zero"));
    }
}
