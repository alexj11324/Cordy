//! Cross-platform background process ownership for CLI daemon start.
//!
//! This is deliberately separate from the foreground bootstrap owner: the
//! child acquires and writes the authoritative PID lock itself. The launcher
//! only keeps the process handle long enough to detect pre-readiness exits.

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Seek};
use std::path::PathBuf;
use std::process::{Child, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::Context;

use crate::bootstrap::{open_bounded_crash_log, ProfileStatePaths};
use crate::control_client::{
    DaemonHealthSnapshot, LocalDaemonControl, LocalDaemonHealth, LocalDaemonProbe,
};
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

const STARTUP_LOG_READ_CAP: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupFailureKind {
    AuthenticationRejected,
    ServerUnreachable,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupFailureEvidence {
    pub kind: StartupFailureKind,
    pub structured_lines: Vec<String>,
    pub crash_lines: Vec<String>,
}

impl StartupLogCursor {
    /// Reads only bounded content appended by this launch attempt. Structured
    /// DBG/INF noise is omitted from the user-facing excerpt; raw crash lines
    /// are retained because they contain pre-subscriber failures and panics.
    pub fn failure_evidence(&self, max_lines: usize) -> StartupFailureEvidence {
        let structured = read_lines_since(&self.structured_log, self.structured_offset);
        let crash = read_lines_since(&self.crash_log, self.crash_offset);
        let combined = structured
            .iter()
            .chain(&crash)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n");
        let kind = classify_startup_failure(&combined);
        let structured_lines = tail_lines(
            structured
                .into_iter()
                .filter(|line| !line.contains(" DBG ") && !line.contains(" INF "))
                .collect(),
            max_lines,
        );
        let crash_lines = tail_lines(crash, max_lines);
        StartupFailureEvidence {
            kind,
            structured_lines,
            crash_lines,
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonStopOutcome {
    AlreadyStopped,
    Stopped { pid: u32, forced: bool },
    StillStopping { pid: u32, forced: bool },
}

#[async_trait::async_trait]
pub trait DaemonStartPreflight: Send + Sync {
    async fn check(&self) -> anyhow::Result<()>;
}

pub struct DaemonStartRequest {
    pub launch: BackgroundLaunchOptions,
    pub port: u16,
    pub startup_timeout: Duration,
}

#[derive(Debug)]
pub enum DaemonStartOutcome {
    AlreadyRunning(DaemonHealthSnapshot),
    Launch(BackgroundStartupOutcome),
}

/// Starts a background daemon only after proving that the profile is
/// authenticated. An existing live daemon is identity-checked before being
/// reported as already running because profile names can hash to one port.
pub async fn start_daemon<C, K, P>(
    control: &C,
    clock: &K,
    preflight: &P,
    request: DaemonStartRequest,
) -> anyhow::Result<DaemonStartOutcome>
where
    C: LocalDaemonProbe,
    K: StartupClock,
    P: DaemonStartPreflight,
{
    if let LocalDaemonHealth::Live(snapshot) = control.health(request.port).await {
        snapshot.confirm_profile(&request.launch.profile, request.port)?;
        return Ok(DaemonStartOutcome::AlreadyRunning(snapshot));
    }
    anyhow::ensure!(
        !request.startup_timeout.is_zero(),
        "daemon startup timeout is zero"
    );
    validate_background_launch(&request.launch)?;
    preflight
        .check()
        .await
        .context("daemon start preflight failed")?;
    let profile = request.launch.profile.clone();
    let daemon = BackgroundDaemon::spawn(request.launch)?;
    let startup = daemon
        .wait_until_ready(
            control,
            clock,
            &profile,
            request.port,
            request.startup_timeout,
        )
        .await?;
    Ok(DaemonStartOutcome::Launch(startup))
}

#[async_trait::async_trait]
pub trait DaemonRestartPreflight: Send + Sync {
    async fn check(&self) -> anyhow::Result<()>;
}

pub struct DaemonRestartRequest {
    pub launch: BackgroundLaunchOptions,
    pub port: u16,
    pub stop_timeout: Duration,
    pub startup_timeout: Duration,
}

#[derive(Debug)]
pub enum DaemonRestartOutcome {
    StopIncomplete(DaemonStopOutcome),
    Launch {
        stop: DaemonStopOutcome,
        startup: BackgroundStartupOutcome,
    },
}

/// Restarts without sacrificing a healthy daemon when the replacement cannot
/// pass authentication/server preflight. The expensive preflight is required
/// only when a live daemon would otherwise be stopped; starting from a stopped
/// state retains the normal child-preflight/early-exit path.
pub async fn restart_daemon<C, K, T, P>(
    control: &C,
    clock: &K,
    terminator: &T,
    preflight: &P,
    request: DaemonRestartRequest,
) -> anyhow::Result<DaemonRestartOutcome>
where
    C: LocalDaemonControl,
    K: StartupClock,
    T: ProcessTerminator,
    P: DaemonRestartPreflight,
{
    anyhow::ensure!(
        !request.startup_timeout.is_zero(),
        "daemon startup timeout is zero"
    );
    validate_background_launch(&request.launch)?;
    let stop = stop_daemon_with_preflight(
        control,
        clock,
        terminator,
        &request.launch.profile,
        request.port,
        request.stop_timeout,
        Some(preflight),
    )
    .await?;
    if matches!(stop, DaemonStopOutcome::StillStopping { .. }) {
        return Ok(DaemonRestartOutcome::StopIncomplete(stop));
    }
    let profile = request.launch.profile.clone();
    let daemon = BackgroundDaemon::spawn(request.launch)?;
    let startup = daemon
        .wait_until_ready(
            control,
            clock,
            &profile,
            request.port,
            request.startup_timeout,
        )
        .await?;
    Ok(DaemonRestartOutcome::Launch { stop, startup })
}

/// Executes the identity-safe cross-platform stop transaction used by both
/// `daemon stop` and the stop phase of `daemon restart`.
pub async fn stop_daemon<C, K, T>(
    control: &C,
    clock: &K,
    terminator: &T,
    profile: &str,
    port: u16,
    wait_timeout: Duration,
) -> anyhow::Result<DaemonStopOutcome>
where
    C: LocalDaemonControl,
    K: StartupClock,
    T: ProcessTerminator,
{
    stop_daemon_with_preflight(
        control,
        clock,
        terminator,
        profile,
        port,
        wait_timeout,
        None,
    )
    .await
}

async fn stop_daemon_with_preflight<C, K, T>(
    control: &C,
    clock: &K,
    terminator: &T,
    profile: &str,
    port: u16,
    wait_timeout: Duration,
    preflight: Option<&dyn DaemonRestartPreflight>,
) -> anyhow::Result<DaemonStopOutcome>
where
    C: LocalDaemonControl,
    K: StartupClock,
    T: ProcessTerminator,
{
    let snapshot = match control.health(port).await {
        LocalDaemonHealth::Stopped => return Ok(DaemonStopOutcome::AlreadyStopped),
        LocalDaemonHealth::Live(snapshot) => snapshot,
    };
    snapshot.confirm_profile(profile, port)?;
    if let Some(preflight) = preflight {
        preflight
            .check()
            .await
            .context("daemon restart preflight failed; running daemon was left untouched")?;
    }
    let pid = u32::try_from(snapshot.response.pid)
        .ok()
        .filter(|pid| *pid > 0)
        .context("daemon health response has no valid PID")?;

    let forced = match control.request_shutdown(port).await {
        Ok(()) => false,
        Err(shutdown_error) => {
            terminator
                .force_kill(pid)
                .map_err(|kill_error| ForcedTerminationError {
                    shutdown_error,
                    kill_error,
                })?;
            true
        }
    };
    if wait_timeout.is_zero() {
        return Ok(DaemonStopOutcome::StillStopping { pid, forced });
    }
    let deadline = clock
        .now()
        .checked_add(wait_timeout)
        .context("daemon stop deadline overflow")?;
    loop {
        let now = clock.now();
        if now >= deadline {
            return Ok(DaemonStopOutcome::StillStopping { pid, forced });
        }
        clock.sleep(STARTUP_POLL_INTERVAL.min(deadline - now)).await;
        match control.health(port).await {
            LocalDaemonHealth::Stopped => {
                return Ok(DaemonStopOutcome::Stopped { pid, forced });
            }
            LocalDaemonHealth::Live(current) => current.confirm_profile(profile, port)?,
        }
    }
}

#[derive(Debug)]
struct ForcedTerminationError {
    shutdown_error: anyhow::Error,
    kill_error: anyhow::Error,
}

impl std::fmt::Display for ForcedTerminationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "graceful daemon shutdown failed ({}); forced termination also failed: {}",
            self.shutdown_error, self.kill_error
        )
    }
}

impl std::error::Error for ForcedTerminationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.kill_error.as_ref())
    }
}

impl BackgroundDaemon {
    pub fn spawn(options: BackgroundLaunchOptions) -> anyhow::Result<Self> {
        validate_background_launch(&options)?;
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

fn validate_background_launch(options: &BackgroundLaunchOptions) -> anyhow::Result<()> {
    anyhow::ensure!(
        !options.binary.as_os_str().is_empty(),
        "daemon executable path is empty"
    );
    anyhow::ensure!(
        options.binary.is_absolute(),
        "daemon executable path must be absolute"
    );
    let metadata = fs::metadata(&options.binary)
        .with_context(|| format!("inspect daemon executable: {}", options.binary.display()))?;
    anyhow::ensure!(metadata.is_file(), "daemon executable is not a file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        anyhow::ensure!(
            metadata.permissions().mode() & 0o111 != 0,
            "daemon executable is not executable"
        );
    }
    Ok(())
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

fn read_lines_since(path: &std::path::Path, offset: u64) -> Vec<String> {
    let Ok(mut file) = fs::File::open(path) else {
        return Vec::new();
    };
    let length = file.metadata().map_or(0, |metadata| metadata.len());
    let attempt_start = offset.min(length);
    let available = length.saturating_sub(attempt_start);
    let read_start = if available > STARTUP_LOG_READ_CAP {
        length - STARTUP_LOG_READ_CAP
    } else {
        attempt_start
    };
    if file.seek(std::io::SeekFrom::Start(read_start)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::with_capacity((length - read_start) as usize);
    if file
        .take(STARTUP_LOG_READ_CAP)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Vec::new();
    }
    String::from_utf8_lossy(&bytes)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn tail_lines(mut lines: Vec<String>, maximum: usize) -> Vec<String> {
    if lines.len() > maximum {
        lines.drain(..lines.len() - maximum);
    }
    lines
}

fn classify_startup_failure(logs: &str) -> StartupFailureKind {
    if ["auth token rejected", "returned 401", "not authenticated"]
        .iter()
        .any(|needle| logs.contains(needle))
    {
        StartupFailureKind::AuthenticationRejected
    } else if [
        "connection refused",
        "no such host",
        "i/o timeout",
        "network is unreachable",
    ]
    .iter()
    .any(|needle| logs.contains(needle))
    {
        StartupFailureKind::ServerUnreachable
    } else {
        StartupFailureKind::Other
    }
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

    struct StoppedControl;

    #[async_trait::async_trait]
    impl LocalDaemonProbe for StoppedControl {
        async fn health(&self, _port: u16) -> LocalDaemonHealth {
            LocalDaemonHealth::Stopped
        }
    }

    #[async_trait::async_trait]
    impl LocalDaemonControl for StoppedControl {
        async fn request_shutdown(&self, _port: u16) -> anyhow::Result<()> {
            panic!("shutdown must not be requested for a stopped daemon")
        }
    }

    struct UnusedClock;

    #[async_trait::async_trait]
    impl StartupClock for UnusedClock {
        fn now(&self) -> Instant {
            panic!("clock must not be read for a stopped daemon")
        }

        async fn sleep(&self, _duration: Duration) {
            panic!("clock must not sleep for a stopped daemon")
        }
    }

    struct UnusedTerminator;

    impl ProcessTerminator for UnusedTerminator {
        fn force_kill(&self, _pid: u32) -> anyhow::Result<()> {
            panic!("a stopped daemon must not be killed")
        }
    }

    struct LiveControl;

    #[async_trait::async_trait]
    impl LocalDaemonProbe for LiveControl {
        async fn health(&self, _port: u16) -> LocalDaemonHealth {
            crate::control_client::parse_health(serde_json::json!({
                "status": "running",
                "pid": 42,
                "profile": "profile"
            }))
            .unwrap()
        }
    }

    #[async_trait::async_trait]
    impl LocalDaemonControl for LiveControl {
        async fn request_shutdown(&self, _port: u16) -> anyhow::Result<()> {
            panic!("failed restart preflight must not stop the running daemon")
        }
    }

    struct FailingPreflight;

    #[async_trait::async_trait]
    impl DaemonRestartPreflight for FailingPreflight {
        async fn check(&self) -> anyhow::Result<()> {
            anyhow::bail!("server rejected token")
        }
    }

    struct PanicStartPreflight;

    #[async_trait::async_trait]
    impl DaemonStartPreflight for PanicStartPreflight {
        async fn check(&self) -> anyhow::Result<()> {
            panic!("already-running start must not perform auth preflight")
        }
    }

    #[test]
    fn missing_logs_start_at_zero() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(file_length(&directory.path().join("missing.log")), 0);
    }

    #[test]
    fn startup_evidence_is_bounded_to_attempt_and_filters_noise() {
        let directory = tempfile::tempdir().unwrap();
        let structured = directory.path().join("daemon.log");
        let crash = directory.path().join("daemon.err.log");
        fs::write(&structured, "old secret\n").unwrap();
        fs::write(&crash, "old crash\n").unwrap();
        let cursor = StartupLogCursor {
            structured_log: structured.clone(),
            structured_offset: file_length(&structured),
            crash_log: crash.clone(),
            crash_offset: file_length(&crash),
        };
        fs::write(
            &structured,
            "old secret\n2026 INF routine\n2026 ERR auth token rejected\n",
        )
        .unwrap();
        fs::write(&crash, "old crash\nchild exited\n").unwrap();

        let evidence = cursor.failure_evidence(5);
        assert_eq!(evidence.kind, StartupFailureKind::AuthenticationRejected);
        assert_eq!(
            evidence.structured_lines,
            vec!["2026 ERR auth token rejected"]
        );
        assert_eq!(evidence.crash_lines, vec!["child exited"]);
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

    #[tokio::test]
    async fn stop_is_idempotent_without_touching_process_or_clock() {
        let outcome = stop_daemon(
            &StoppedControl,
            &UnusedClock,
            &UnusedTerminator,
            "profile",
            19515,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(outcome, DaemonStopOutcome::AlreadyStopped);
    }

    #[tokio::test]
    async fn restart_preflight_failure_leaves_running_daemon_untouched() {
        let result = restart_daemon(
            &LiveControl,
            &UnusedClock,
            &UnusedTerminator,
            &FailingPreflight,
            DaemonRestartRequest {
                launch: BackgroundLaunchOptions {
                    profile: "profile".to_string(),
                    binary: std::env::current_exe().unwrap(),
                    args: Vec::new(),
                },
                port: 19515,
                stop_timeout: Duration::from_secs(5),
                startup_timeout: Duration::from_secs(45),
            },
        )
        .await;
        let error = result.unwrap_err();
        assert!(error
            .to_string()
            .contains("running daemon was left untouched"));
        assert!(error
            .chain()
            .any(|cause| cause.to_string() == "server rejected token"));
    }

    #[tokio::test]
    async fn start_reports_identity_checked_existing_daemon_without_spawn() {
        let outcome = start_daemon(
            &LiveControl,
            &UnusedClock,
            &PanicStartPreflight,
            DaemonStartRequest {
                launch: BackgroundLaunchOptions {
                    profile: "profile".to_string(),
                    binary: PathBuf::from("/must/not/spawn"),
                    args: Vec::new(),
                },
                port: 19515,
                startup_timeout: Duration::from_secs(45),
            },
        )
        .await
        .unwrap();
        match outcome {
            DaemonStartOutcome::AlreadyRunning(snapshot) => {
                assert_eq!(snapshot.response.pid, 42);
            }
            DaemonStartOutcome::Launch(_) => panic!("existing daemon unexpectedly spawned child"),
        }
    }
}
