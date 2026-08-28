//! Process bootstrap primitives for the future Cordy CLI binary.
//!
//! This module owns host-process concerns only: profile state paths, exclusive
//! PID ownership, logging, shutdown signals, graceful stack completion, and
//! successor release. The caller must provide a real daemon stack callback;
//! there is no default service implementation or pretend-success path.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use flate2::write::GzEncoder;
use flate2::Compression;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::EnvFilter;

use crate::identity::profile_dir;
use crate::update_executor::{
    is_access_denied_spawn_error, restart_command, restart_command_after_access_denied,
};

const PID_FILE_NAME: &str = "daemon.pid";
const PID_LOCK_FILE_NAME: &str = "daemon.pid.lock";
const STRUCTURED_LOG_FILE_NAME: &str = "daemon.log";
const CRASH_LOG_FILE_NAME: &str = "daemon.err.log";
const RESTART_HANDOFF_ENV: &str = "CORDY_DAEMON_RESTART_HANDOFF";
const RESTART_LOCK_WAIT: Duration = Duration::from_secs(5);
const DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(45);
const DEFAULT_LOG_MAX_SIZE_MB: u64 = 20;
const DEFAULT_LOG_MAX_BACKUPS: usize = 5;
const DEFAULT_LOG_MAX_AGE_DAYS: u64 = 30;
const CRASH_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
pub(crate) const DAEMON_LOG_COMPONENT: &str = "daemon";

/// A daemon owner may outlive the task that spawned it, so it cannot rely on
/// implicit span inheritance. ERROR is deliberate: the span itself emits no
/// event, and this keeps its component/owner fields available to every enabled
/// warning or error even when lower log levels are filtered out.
pub(crate) fn daemon_owner_span(owner: &'static str) -> tracing::Span {
    tracing::error_span!(
        "daemon_owner",
        component = DAEMON_LOG_COMPONENT,
        owner = owner
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileStatePaths {
    pub directory: PathBuf,
    pub pid: PathBuf,
    pub pid_lock: PathBuf,
    pub structured_log: PathBuf,
    pub crash_log: PathBuf,
}

impl ProfileStatePaths {
    pub fn resolve(profile: &str) -> anyhow::Result<Self> {
        let directory = profile_dir(profile)?;
        Ok(Self {
            pid: directory.join(PID_FILE_NAME),
            pid_lock: directory.join(PID_LOCK_FILE_NAME),
            structured_log: directory.join(STRUCTURED_LOG_FILE_NAME),
            crash_log: directory.join(CRASH_LOG_FILE_NAME),
            directory,
        })
    }
}

#[derive(Debug, Clone)]
pub struct BootstrapOptions {
    pub profile: String,
    pub successor_args: Vec<OsString>,
    pub log_filter: Option<String>,
    pub graceful_shutdown_timeout: Duration,
}

impl BootstrapOptions {
    pub fn new(profile: impl Into<String>, successor_args: Vec<OsString>) -> Self {
        Self {
            profile: profile.into(),
            successor_args,
            log_filter: None,
            graceful_shutdown_timeout: DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BootstrapContext {
    pub paths: ProfileStatePaths,
    pub launched_by: String,
    pub shutdown: CancellationToken,
}

#[derive(Debug, Clone, Default)]
pub struct DaemonStackExit {
    /// Set only after the daemon core has deregistered runtimes and drained
    /// owned task/control lifecycles.
    pub successor_binary: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapOutcome {
    pub successor_pid: Option<u32>,
}

pub trait BootstrapClock: Send + Sync + 'static {
    fn now(&self) -> SystemTime;
}

#[derive(Debug)]
pub struct SystemBootstrapClock;

impl BootstrapClock for SystemBootstrapClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Production entrypoint used by the future CLI binary. The supplied stack
/// owns daemon services and must return only after graceful drain completes.
pub async fn run_once<Run, RunFuture>(
    options: BootstrapOptions,
    run_stack: Run,
) -> anyhow::Result<BootstrapOutcome>
where
    Run: FnOnce(BootstrapContext) -> RunFuture,
    RunFuture: Future<Output = anyhow::Result<DaemonStackExit>>,
{
    run_once_with_signal(
        options,
        Arc::new(SystemBootstrapClock),
        wait_for_shutdown_signal(),
        run_stack,
    )
    .await
}

/// Injectable run-once boundary for deterministic lifecycle tests. A signal
/// cancels the root token; this function then keeps awaiting `run_stack`, so
/// task/control owners retain their full graceful-drain contract.
pub async fn run_once_with_signal<Run, RunFuture, Signal>(
    options: BootstrapOptions,
    clock: Arc<dyn BootstrapClock>,
    shutdown_signal: Signal,
    run_stack: Run,
) -> anyhow::Result<BootstrapOutcome>
where
    Run: FnOnce(BootstrapContext) -> RunFuture,
    RunFuture: Future<Output = anyhow::Result<DaemonStackExit>>,
    Signal: Future<Output = io::Result<()>>,
{
    anyhow::ensure!(
        !options.graceful_shutdown_timeout.is_zero(),
        "graceful shutdown timeout must be greater than zero"
    );
    let paths = ProfileStatePaths::resolve(&options.profile)?;
    fs::create_dir_all(&paths.directory).context("create daemon profile directory")?;
    let mut pid_owner = PidOwner::acquire(&paths).await?;
    let mut logs = DaemonLogs::install(&paths, options.log_filter.as_deref(), clock)?;
    let shutdown = CancellationToken::new();
    let context = BootstrapContext {
        paths: paths.clone(),
        launched_by: std::env::var("CORDY_LAUNCHED_BY").unwrap_or_default(),
        shutdown: shutdown.clone(),
    };
    let stack = run_stack(context).instrument(daemon_owner_span("stack"));
    let stack_result = drive_stack(
        shutdown,
        options.graceful_shutdown_timeout,
        shutdown_signal,
        stack,
    )
    .await;
    let stack_exit = stack_result.context("daemon stack failed")?;

    let successor_pid = if let Some(binary) = stack_exit.successor_binary {
        // The successor owns daemon.log through a fresh rotating writer. Drop
        // the current handle before spawn, and use only the bounded raw crash
        // sink for inherited stdout/stderr.
        logs.close_structured()?;
        let crash = logs.crash_sink_for_handoff(&paths.crash_log)?;
        let child = spawn_successor(&binary, &options.successor_args, &crash)?;
        let pid = child.id();
        drop(child); // Closing the process handle releases; it does not kill.
        pid_owner.handoff(pid)?;
        Some(pid)
    } else {
        None
    };
    Ok(BootstrapOutcome { successor_pid })
}

async fn drive_stack<RunFuture, Signal>(
    shutdown: CancellationToken,
    graceful_shutdown_timeout: Duration,
    shutdown_signal: Signal,
    stack: RunFuture,
) -> anyhow::Result<DaemonStackExit>
where
    RunFuture: Future<Output = anyhow::Result<DaemonStackExit>>,
    Signal: Future<Output = io::Result<()>>,
{
    tokio::pin!(stack);
    tokio::pin!(shutdown_signal);
    let stack_result = tokio::select! {
        result = &mut stack => result,
        signal = &mut shutdown_signal => {
            if let Err(error) = signal {
                tracing::error!(component = "daemon", %error, "daemon shutdown signal listener failed");
            }
            shutdown.cancel();
            tokio::time::timeout(graceful_shutdown_timeout, &mut stack)
                .await
                .map_err(|_| anyhow::anyhow!(
                    "daemon stack did not drain within {:?}",
                    graceful_shutdown_timeout
                ))?
        }
    };
    shutdown.cancel();
    stack_result
}

async fn wait_for_shutdown_signal() -> io::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            _ = interrupt.recv() => Ok(()),
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(windows)]
    {
        let mut interrupt = tokio::signal::windows::ctrl_c()?;
        let mut ctrl_break = tokio::signal::windows::ctrl_break()?;
        tokio::select! {
            _ = interrupt.recv() => Ok(()),
            _ = ctrl_break.recv() => Ok(()),
        }
    }
}

struct PidOwner {
    lock: File,
    pid_path: PathBuf,
    remove_on_drop: bool,
}

impl PidOwner {
    async fn acquire(paths: &ProfileStatePaths) -> anyhow::Result<Self> {
        let handoff = std::env::var_os(RESTART_HANDOFF_ENV).is_some();
        let deadline = tokio::time::Instant::now() + RESTART_LOCK_WAIT;
        loop {
            match Self::try_acquire(paths) {
                Ok(owner) => {
                    if handoff {
                        std::env::remove_var(RESTART_HANDOFF_ENV);
                    }
                    return Ok(owner);
                }
                Err(_) if handoff && tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn try_acquire(paths: &ProfileStatePaths) -> anyhow::Result<Self> {
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&paths.pid_lock)
            .context("open daemon PID lock")?;
        try_lock_pid_file(&lock).map_err(|error| {
            let observed = fs::read_to_string(&paths.pid)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            anyhow::anyhow!("daemon is already running (pid {observed}): {error}")
        })?;
        // Owning the kernel lock proves any previous writer is gone; stale or
        // malformed PID content is replaced without process-name guesswork.
        atomic_write_pid(&paths.pid, std::process::id())?;
        Ok(Self {
            lock,
            pid_path: paths.pid.clone(),
            remove_on_drop: true,
        })
    }

    fn handoff(&mut self, successor_pid: u32) -> anyhow::Result<()> {
        atomic_write_pid(&self.pid_path, successor_pid)?;
        self.remove_on_drop = false;
        Ok(())
    }
}

impl Drop for PidOwner {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = fs::remove_file(&self.pid_path);
        }
        unlock_pid_file(&self.lock);
    }
}

fn atomic_write_pid(path: &Path, pid: u32) -> anyhow::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("PID path has no parent directory"))?;
    let mut temporary = tempfile::Builder::new()
        .prefix("daemon.pid-")
        .tempfile_in(directory)
        .context("create temporary PID file")?;
    write!(temporary, "{pid}").context("write temporary PID file")?;
    temporary.flush().context("flush temporary PID file")?;
    temporary
        .as_file()
        .sync_all()
        .context("sync temporary PID file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o644))
            .context("set PID file permissions")?;
    }
    let temporary = temporary
        .into_temp_path()
        .keep()
        .map_err(|error| error.error)
        .context("retain temporary PID file")?;
    if let Err(error) = atomic_replace(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error).context("atomically replace PID file");
    }
    Ok(())
}

#[cfg(unix)]
fn try_lock_pid_file(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    // SAFETY: flock operates on this live File's descriptor and stores no
    // pointer beyond the call.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn unlock_pid_file(file: &File) {
    use std::os::fd::AsRawFd;
    // SAFETY: best-effort unlock of this live File descriptor.
    let _ = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
}

/// Returns the PID advertised by the current profile lock owner, or `None`
/// only after proving that no process holds the kernel lock. The control CLI
/// uses this after `/health` closes so restart cannot race the old daemon's
/// bounded drain and PID ownership release.
pub(crate) fn locked_profile_pid(profile: &str) -> anyhow::Result<Option<u32>> {
    let paths = ProfileStatePaths::resolve(profile)?;
    let lock = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(&paths.pid_lock)
    {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("open daemon PID lock for ownership probe"),
    };
    if try_lock_pid_file(&lock).is_ok() {
        unlock_pid_file(&lock);
        return Ok(None);
    }
    let raw = fs::read_to_string(&paths.pid).context("read locked daemon PID")?;
    let pid = raw
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)
        .context("locked daemon PID file is missing or invalid")?;
    Ok(Some(pid))
}

#[cfg(windows)]
fn try_lock_pid_file(file: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    let flags = LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY;
    let result = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            flags,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };
    if result != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn unlock_pid_file(file: &File) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    let _ = unsafe { UnlockFileEx(file.as_raw_handle(), 0, u32::MAX, u32::MAX, &mut overlapped) };
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[derive(Debug, Clone)]
struct LogPolicy {
    max_size_bytes: u64,
    max_backups: usize,
    max_age: Duration,
}

impl LogPolicy {
    fn from_env() -> Self {
        Self {
            max_size_bytes: positive_env_u64(
                "CORDY_DAEMON_LOG_MAX_SIZE_MB",
                DEFAULT_LOG_MAX_SIZE_MB,
            ) * 1024
                * 1024,
            max_backups: positive_env_usize(
                "CORDY_DAEMON_LOG_MAX_BACKUPS",
                DEFAULT_LOG_MAX_BACKUPS,
            ),
            max_age: Duration::from_secs(
                positive_env_u64("CORDY_DAEMON_LOG_MAX_AGE_DAYS", DEFAULT_LOG_MAX_AGE_DAYS)
                    * 24
                    * 60
                    * 60,
            ),
        }
    }
}

fn positive_env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn positive_env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

#[derive(Clone)]
struct RotatingLogWriter {
    inner: Arc<Mutex<RotatingLogState>>,
}

struct RotatingLogState {
    path: PathBuf,
    file: Option<File>,
    length: u64,
    policy: LogPolicy,
    clock: Arc<dyn BootstrapClock>,
}

impl RotatingLogWriter {
    fn open(path: PathBuf, policy: LogPolicy, clock: Arc<dyn BootstrapClock>) -> io::Result<Self> {
        let file = open_append(&path)?;
        let length = file.metadata()?.len();
        Ok(Self {
            inner: Arc::new(Mutex::new(RotatingLogState {
                path,
                file: Some(file),
                length,
                policy,
                clock,
            })),
        })
    }

    fn close(&self) -> io::Result<()> {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(file) = state.file.take() {
            file.sync_all()?;
        }
        Ok(())
    }
}

impl Write for RotatingLogWriter {
    fn write(&mut self, mut buffer: &[u8]) -> io::Result<usize> {
        let original_length = buffer.len();
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !buffer.is_empty() {
            if state.length >= state.policy.max_size_bytes {
                state.rotate()?;
            }
            let available = (state.policy.max_size_bytes - state.length) as usize;
            let amount = available.min(buffer.len());
            let file = state
                .file
                .as_mut()
                .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "daemon log is closed"))?;
            file.write_all(&buffer[..amount])?;
            state.length += amount as u64;
            buffer = &buffer[amount..];
        }
        Ok(original_length)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut state = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match state.file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for RotatingLogWriter {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

impl RotatingLogState {
    fn rotate(&mut self) -> io::Result<()> {
        if let Some(file) = self.file.take() {
            file.sync_all()?;
        }
        let rotation = (|| {
            self.prune_expired()?;
            let oldest = backup_path(&self.path, self.policy.max_backups);
            let _ = fs::remove_file(oldest);
            for index in (1..self.policy.max_backups).rev() {
                let from = backup_path(&self.path, index);
                let to = backup_path(&self.path, index + 1);
                if from.exists() {
                    fs::rename(from, to)?;
                }
            }
            if self.path.exists() {
                compress_log(&self.path, &backup_path(&self.path, 1))?;
                fs::remove_file(&self.path)?;
            }
            Ok(())
        })();
        self.file = Some(open_append(&self.path)?);
        self.length = self
            .file
            .as_ref()
            .and_then(|file| file.metadata().ok())
            .map_or(0, |metadata| metadata.len());
        rotation
    }

    fn prune_expired(&self) -> io::Result<()> {
        let cutoff = self
            .clock
            .now()
            .checked_sub(self.policy.max_age)
            .unwrap_or(UNIX_EPOCH);
        for index in 1..=self.policy.max_backups {
            let path = backup_path(&self.path, index);
            match fs::metadata(&path).and_then(|metadata| metadata.modified()) {
                Ok(modified) if modified < cutoff => {
                    fs::remove_file(path)?;
                }
                Ok(_) | Err(_) => {}
            }
        }
        Ok(())
    }
}

fn backup_path(path: &Path, index: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}.gz", path.display(), index))
}

fn compress_log(source: &Path, destination: &Path) -> io::Result<()> {
    let directory = destination
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "log path has no parent"))?;
    let mut temporary = tempfile::Builder::new()
        .prefix("daemon-log-")
        .tempfile_in(directory)?;
    {
        let mut input = File::open(source)?;
        let mut encoder = GzEncoder::new(&mut temporary, Compression::default());
        io::copy(&mut input, &mut encoder)?;
        encoder.finish()?;
    }
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    let temporary = temporary
        .into_temp_path()
        .keep()
        .map_err(|error| error.error)?;
    if let Err(error) = atomic_replace(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

struct DaemonLogs {
    rotating: Option<RotatingLogWriter>,
    crash_stdio: Option<CrashStdioGuard>,
}

impl DaemonLogs {
    fn install(
        paths: &ProfileStatePaths,
        filter: Option<&str>,
        clock: Arc<dyn BootstrapClock>,
    ) -> anyhow::Result<Self> {
        let terminal = cordy_util::logging::stderr_is_terminal();
        let crash_stdio = if terminal {
            None
        } else {
            repoint_windows_stdio(&paths.crash_log)?
        };
        let rotating = if terminal {
            None
        } else {
            Some(RotatingLogWriter::open(
                paths.structured_log.clone(),
                LogPolicy::from_env(),
                clock,
            )?)
        };
        let writer = match &rotating {
            Some(writer) => BoxMakeWriter::new(writer.clone()),
            None => BoxMakeWriter::new(io::stderr),
        };
        let filter = filter
            .map(str::to_owned)
            .unwrap_or_else(cordy_util::logging::env_filter);
        let env_filter = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("debug"));
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
                cordy_util::logging::LOCAL_TIME_FORMAT.to_string(),
            ))
            .with_writer(writer)
            .with_ansi(rotating.is_none())
            .try_init()
            .map_err(map_subscriber_install_error)?;
        Ok(Self {
            rotating,
            crash_stdio,
        })
    }

    fn close_structured(&mut self) -> io::Result<()> {
        if let Some(writer) = self.rotating.take() {
            writer.close()?;
        }
        Ok(())
    }

    fn crash_sink_for_handoff(&self, path: &Path) -> io::Result<File> {
        match &self.crash_stdio {
            Some(guard) => guard.file.try_clone(),
            None => open_bounded_crash_log(path),
        }
    }
}

#[derive(Debug)]
struct SubscriberInstallError {
    source: Box<dyn std::error::Error + Send + Sync + 'static>,
}

impl std::fmt::Display for SubscriberInstallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("tracing subscriber initialization failed")
    }
}

impl std::error::Error for SubscriberInstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn map_subscriber_install_error(
    source: Box<dyn std::error::Error + Send + Sync + 'static>,
) -> anyhow::Error {
    anyhow::Error::new(SubscriberInstallError { source })
        .context("install daemon tracing subscriber")
}

pub(crate) fn open_bounded_crash_log(path: &Path) -> io::Result<File> {
    if fs::metadata(path).is_ok_and(|metadata| metadata.len() >= CRASH_LOG_MAX_BYTES) {
        let backup = PathBuf::from(format!("{}.1", path.display()));
        let _ = fs::remove_file(&backup);
        let _ = fs::rename(path, backup);
    }
    OpenOptions::new().create(true).append(true).open(path)
}

struct CrashStdioGuard {
    file: File,
}

#[cfg(not(windows))]
fn repoint_windows_stdio(_path: &Path) -> io::Result<Option<CrashStdioGuard>> {
    Ok(None)
}

#[cfg(windows)]
fn repoint_windows_stdio(path: &Path) -> io::Result<Option<CrashStdioGuard>> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{
        GetStdHandle, SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };

    let file = open_bounded_crash_log(path)?;
    let new_handle = file.as_raw_handle();
    let old_stdout = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let old_stderr = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    if unsafe { SetStdHandle(STD_OUTPUT_HANDLE, new_handle) } == 0
        || unsafe { SetStdHandle(STD_ERROR_HANDLE, new_handle) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    for (index, old) in [old_stdout, old_stderr].into_iter().enumerate() {
        if !old.is_null() && old != INVALID_HANDLE_VALUE && old != new_handle {
            if index == 0 || old != old_stdout {
                let _ = unsafe { CloseHandle(old) };
            }
        }
    }
    Ok(Some(CrashStdioGuard { file }))
}

fn spawn_successor(binary: &Path, args: &[OsString], crash_log: &File) -> anyhow::Result<Child> {
    let mut command = restart_command(binary, args);
    command.env(RESTART_HANDOFF_ENV, "1");
    command.stdout(Stdio::from(crash_log.try_clone()?));
    command.stderr(Stdio::from(crash_log.try_clone()?));
    match command.spawn() {
        Ok(child) => Ok(child),
        Err(error) if is_access_denied_spawn_error(&error) => {
            let mut retry = restart_command_after_access_denied(binary, args);
            retry.env(RESTART_HANDOFF_ENV, "1");
            retry.stdout(Stdio::from(crash_log.try_clone()?));
            retry.stderr(Stdio::from(crash_log.try_clone()?));
            retry
                .spawn()
                .context("spawn daemon successor without job breakaway")
        }
        Err(error) => Err(error).context("spawn daemon successor"),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[derive(Debug)]
    struct FixedClock(SystemTime);

    impl BootstrapClock for FixedClock {
        fn now(&self) -> SystemTime {
            self.0
        }
    }

    fn test_paths(directory: &Path) -> ProfileStatePaths {
        ProfileStatePaths {
            directory: directory.to_path_buf(),
            pid: directory.join(PID_FILE_NAME),
            pid_lock: directory.join(PID_LOCK_FILE_NAME),
            structured_log: directory.join(STRUCTURED_LOG_FILE_NAME),
            crash_log: directory.join(CRASH_LOG_FILE_NAME),
        }
    }

    #[test]
    fn pid_owner_is_exclusive_and_recovers_stale_contents() {
        let directory = tempfile::tempdir().unwrap();
        let paths = test_paths(directory.path());
        fs::write(&paths.pid, "999999").unwrap();

        let owner = PidOwner::try_acquire(&paths).unwrap();
        assert_eq!(
            fs::read_to_string(&paths.pid).unwrap(),
            std::process::id().to_string()
        );
        assert!(PidOwner::try_acquire(&paths).is_err());
        drop(owner);
        assert!(!paths.pid.exists());

        let recovered = PidOwner::try_acquire(&paths).unwrap();
        drop(recovered);
    }

    #[test]
    fn pid_handoff_is_atomic_and_survives_old_owner_drop() {
        let directory = tempfile::tempdir().unwrap();
        let paths = test_paths(directory.path());
        let mut owner = PidOwner::try_acquire(&paths).unwrap();
        owner.handoff(4242).unwrap();
        drop(owner);
        assert_eq!(fs::read_to_string(&paths.pid).unwrap(), "4242");
    }

    #[test]
    fn rotating_writer_compresses_and_bounds_backups() {
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("daemon.log");
        let policy = LogPolicy {
            max_size_bytes: 8,
            max_backups: 2,
            max_age: Duration::from_secs(60),
        };
        let clock = Arc::new(FixedClock(UNIX_EPOCH + Duration::from_secs(120)));
        let mut writer = RotatingLogWriter::open(log.clone(), policy, clock).unwrap();
        writer.write_all(b"12345678abcdef").unwrap();
        writer.flush().unwrap();

        let mut decoded = String::new();
        flate2::read::GzDecoder::new(File::open(backup_path(&log, 1)).unwrap())
            .read_to_string(&mut decoded)
            .unwrap();
        assert_eq!(decoded, "12345678");
        assert_eq!(fs::read_to_string(&log).unwrap(), "abcdef");
        assert!(!backup_path(&log, 3).exists());
    }

    #[test]
    fn crash_log_rolls_at_the_hard_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(CRASH_LOG_FILE_NAME);
        let file = File::create(&path).unwrap();
        file.set_len(CRASH_LOG_MAX_BYTES).unwrap();
        drop(file);

        let mut crash = open_bounded_crash_log(&path).unwrap();
        crash.write_all(b"new crash").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), 9);
        assert_eq!(
            fs::metadata(format!("{}.1", path.display())).unwrap().len(),
            CRASH_LOG_MAX_BYTES
        );
    }

    #[test]
    fn subscriber_install_mapping_preserves_context_and_source() {
        let error = map_subscriber_install_error(Box::new(io::Error::other(
            "subscriber already installed",
        )));
        let chain: Vec<String> = error.chain().map(ToString::to_string).collect();

        assert_eq!(chain[0], "install daemon tracing subscriber");
        assert_eq!(chain[1], "tracing subscriber initialization failed");
        assert_eq!(chain[2], "subscriber already installed");
    }

    #[test]
    fn daemon_owner_span_survives_filtered_info_and_carries_component() {
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::ERROR)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let span = daemon_owner_span("test-owner");
            let metadata = span.metadata().expect("enabled daemon owner span");
            assert_eq!(*metadata.level(), tracing::Level::ERROR);
            assert!(metadata.fields().field("component").is_some());
            assert!(metadata.fields().field("owner").is_some());
            assert_eq!(DAEMON_LOG_COMPONENT, "daemon");
        });
    }

    #[tokio::test]
    async fn signal_cancels_root_and_waits_for_owned_stack_drain() {
        let shutdown = CancellationToken::new();
        let stack_shutdown = shutdown.clone();
        let drained = Arc::new(AtomicBool::new(false));
        let stack_drained = Arc::clone(&drained);
        let stack = async move {
            stack_shutdown.cancelled().await;
            stack_drained.store(true, Ordering::Release);
            Ok(DaemonStackExit::default())
        };

        let exit = drive_stack(
            shutdown,
            Duration::from_secs(1),
            std::future::ready(Ok(())),
            stack,
        )
        .await
        .unwrap();
        assert!(exit.successor_binary.is_none());
        assert!(drained.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn graceful_stack_drain_is_bounded() {
        let error = drive_stack(
            CancellationToken::new(),
            Duration::from_millis(1),
            std::future::ready(Ok(())),
            std::future::pending(),
        )
        .await
        .expect_err("non-draining stack must time out");
        assert!(error.to_string().contains("did not drain"));
    }
}
