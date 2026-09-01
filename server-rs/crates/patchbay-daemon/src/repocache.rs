//! Manages bare git clone caches for workspace
//! repositories; the daemon uses these caches as the source for creating
//! per-task worktrees.
//!
//! Deviations from Go:
//! - `sync.Map` → `Mutex<HashMap>` (task requirement).
//! - `context.Context` → [`Ctx`] (CancellationToken + explicit cause enum),
//!   mirroring `context.WithCancelCause`.
//! - `log/slog` → `tracing` with identical message text.
//! - Blocking channel-based condition variable → `tokio::sync::Notify` with
//!   pre-registration (`enable`) to preserve the Go happens-before of
//!   `close(changed)` under the mutex.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use tokio_util::sync::CancellationToken;

use crate::gc::processtree;

// ---------------------------------------------------------------------------
// Cause-aware cancellation (mirrors context.Context + context.Cause).
// ---------------------------------------------------------------------------

/// Cancellation causes carried alongside a [`CancellationToken`], preserving
/// who-cancelled-why semantics of Go's `context.WithCancelCause`.
///
/// - `Cancelled` corresponds to Go's bare `context.Canceled` (nil cause).
/// - `Preempted` corresponds to `repocache.ErrMaintenancePreempted`.
/// - `Shutdown` is the daemon-wide cancellation cause used by callers that
///   previously passed `context.Background()` plus their own lifetimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelCause {
    Cancelled,
    Preempted,
    Shutdown,
    DeadlineExceeded,
}

impl std::fmt::Display for CancelCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Matches Go's context.Canceled text.
            CancelCause::Cancelled => write!(f, "context canceled"),
            // Matches repocache.ErrMaintenancePreempted text.
            CancelCause::Preempted => {
                write!(f, "repository maintenance preempted by foreground work")
            }
            CancelCause::Shutdown => write!(f, "daemon shutting down"),
            // Matches Go's context.DeadlineExceeded text.
            CancelCause::DeadlineExceeded => write!(f, "context deadline exceeded"),
        }
    }
}

/// A cancellation token paired with an optional cause, mirroring Go's
/// `context.Context` created via `context.WithCancelCause`.
#[derive(Debug, Clone)]
pub struct Ctx {
    token: CancellationToken,
    cause: Arc<Mutex<Option<CancelCause>>>,
}

impl Default for Ctx {
    fn default() -> Self {
        Self::new()
    }
}

impl Ctx {
    /// Equivalent of `context.Background()` wrapped for cause tracking.
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            cause: Arc::new(Mutex::new(None)),
        }
    }

    /// Equivalent of `context.WithCancelCause(parent)`: a child token that is
    /// cancelled when either the parent or this child is cancelled.
    pub fn child(&self) -> Self {
        Self {
            token: self.token.child_token(),
            cause: Arc::new(Mutex::new(None)),
        }
    }

    /// Cancels the token recording `cause`, mirroring `cancel(cause)`.
    pub fn cancel_with(&self, cause: CancelCause) {
        {
            let mut slot = self.cause.lock().unwrap();
            if slot.is_none() {
                *slot = Some(cause);
            }
        }
        self.token.cancel();
    }

    /// Non-blocking `ctx.Err()` check; `Some` means cancelled.
    pub fn err(&self) -> Option<CancelCause> {
        if self.token.is_cancelled() {
            Some(self.cause())
        } else {
            None
        }
    }

    /// Resolved cause: the recorded cause if any, otherwise plain cancelled
    /// (parent-initiated cancellation without a local cause).
    pub fn cause(&self) -> CancelCause {
        self.cause.lock().unwrap().unwrap_or(CancelCause::Cancelled)
    }

    /// Future resolving on cancellation (`<-ctx.Done()`).
    pub async fn cancelled(&self) {
        self.token.cancelled().await;
    }

    pub(crate) fn token(&self) -> &CancellationToken {
        &self.token
    }
}

// ---------------------------------------------------------------------------
// git subprocess plumbing (cache.go lines 24–147).
// ---------------------------------------------------------------------------

/// `gitEnv` (cache.go:24–56): environment for git subprocesses that contact
/// remotes. Passes the full daemon environment so credential helpers (e.g.
/// gh) can locate their config, disables TTY prompting, and sets
/// `safe.directory=*` via GIT_CONFIG_* env vars appended after any existing
/// `GIT_CONFIG_COUNT` block.
fn git_env() -> Vec<(OsString, OsString)> {
    let mut base: Vec<(OsString, OsString)> = std::env::vars_os().collect();

    // Find the existing GIT_CONFIG_COUNT so we append at the next index
    // rather than overwriting any env-scoped git config (auth, URL
    // rewrites, extra headers, etc.).
    let mut existing: i64 = 0;
    for (k, v) in &base {
        if k.to_string_lossy() == "GIT_CONFIG_COUNT" {
            if let Ok(n) = v.to_string_lossy().parse::<i64>() {
                existing = n;
            }
        }
    }

    let idx = existing.to_string();
    base.push(("GIT_TERMINAL_PROMPT".into(), "0".into()));
    base.push(("GIT_CONFIG_COUNT".into(), (existing + 1).to_string().into()));
    base.push((
        format!("GIT_CONFIG_KEY_{idx}").into(),
        "safe.directory".into(),
    ));
    base.push((format!("GIT_CONFIG_VALUE_{idx}").into(), "*".into()));
    base
}

/// `agentGitExcludePatterns` (cache.go:58–67).
pub(crate) const AGENT_GIT_EXCLUDE_PATTERNS: &[&str] = &[
    ".agent_context",
    "CLAUDE.md",
    "AGENTS.md",
    ".claude",
    ".opencode",
    ".deveco",
    "CODEBUDDY.md",
    ".codebuddy",
];

/// `repoCacheGitTimeout` (cache.go:69).
const REPO_CACHE_GIT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Formats a Duration the way Go's `time.Duration.String` renders whole
/// minute/second values (e.g. `10m0s`), used inside timeout error messages.
fn go_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 && secs.is_multiple_of(60) {
        format!("{}m0s", secs / 60)
    } else if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// `newGitCommand` (cache.go:71–75).
fn new_git_command(args: &[&str]) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args);
    apply_env(&mut cmd);
    cmd
}

fn apply_env(cmd: &mut tokio::process::Command) {
    cmd.env_clear();
    for (k, v) in git_env() {
        cmd.env(k, v);
    }
}

/// Runs `f` under a derived deadline context, mirroring
/// `context.WithTimeout(parent, timeout)` around every git invocation
/// (cache.go:89–99/113–123/137–147). The timer cancels the derived token so
/// the process tree is torn down exactly as Go's ctx cancellation does;
/// `Err(DeadlineExceeded)` is returned only when the timer fired.
async fn with_git_deadline<T, F, Fut>(
    parent: &Ctx,
    timeout: Duration,
    f: F,
) -> Result<T, CancelCause>
where
    F: FnOnce(Ctx) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let tctx = parent.child();
    let token = tctx.token().clone();
    let timer = tokio::spawn(async move {
        tokio::time::sleep(timeout).await;
        token.cancel();
    });
    let out = f(tctx.clone()).await;
    timer.abort();
    if tctx.err() == Some(CancelCause::DeadlineExceeded) {
        Err(CancelCause::DeadlineExceeded)
    } else {
        Ok(out)
    }
}

fn git_deadline_error(timeout: Duration) -> anyhow::Error {
    // cache.go:96/120/144 — "git command timed out after %s: %w" where %w is
    // context.DeadlineExceeded.
    anyhow::anyhow!(
        "git command timed out after {}: context deadline exceeded",
        go_duration(timeout)
    )
}

/// `runGitCombinedOutputContext` (cache.go:81–83).
pub(crate) async fn run_git_combined_output(ctx: &Ctx, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    run_git_combined_output_with_timeout(ctx, REPO_CACHE_GIT_TIMEOUT, args).await
}

/// `runGitCombinedOutputWithTimeoutContext` (cache.go:89–99).
pub(crate) async fn run_git_combined_output_with_timeout(
    ctx: &Ctx,
    timeout: Duration,
    args: &[&str],
) -> anyhow::Result<Vec<u8>> {
    // processtree.CombinedOutput(ctx, cmd, 5*time.Second)
    let outcome = with_git_deadline(ctx, timeout, |c| {
        let cmd = new_git_command(args);
        async move { processtree::combined_output(&c, cmd, Duration::from_secs(5)).await }
    })
    .await;
    match outcome {
        Ok((out, Ok(()))) => Ok(out),
        Ok((_out, Err(err))) => {
            // Embed the trimmed combined output so outer fmt.Errorf("%s: %w")
            // layers reproduce Go's message shape.
            Err(err.context(CombinedOutputFailure {
                output: String::from_utf8_lossy(&_out).trim().to_string(),
            }))
        }
        Err(_cause) => Err(git_deadline_error(timeout)),
    }
}

/// `runGitOutputContext` (cache.go:105–107).
pub(crate) async fn run_git_output(ctx: &Ctx, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    run_git_output_with_timeout(ctx, REPO_CACHE_GIT_TIMEOUT, args).await
}

/// `runGitOutputWithTimeoutContext` (cache.go:113–123).
pub(crate) async fn run_git_output_with_timeout(
    ctx: &Ctx,
    timeout: Duration,
    args: &[&str],
) -> anyhow::Result<Vec<u8>> {
    let outcome = with_git_deadline(ctx, timeout, |c| {
        let cmd = new_git_command(args);
        async move { processtree::output(&c, cmd, Duration::from_secs(5)).await }
    })
    .await;
    match outcome {
        Ok(res) => res,
        Err(_cause) => Err(git_deadline_error(timeout)),
    }
}

/// `runGitContext` (cache.go:129–131).
pub(crate) async fn run_git(ctx: &Ctx, args: &[&str]) -> anyhow::Result<()> {
    run_git_with_timeout(ctx, REPO_CACHE_GIT_TIMEOUT, args).await
}

/// `runGitWithTimeoutContext` (cache.go:137–147).
pub(crate) async fn run_git_with_timeout(
    ctx: &Ctx,
    timeout: Duration,
    args: &[&str],
) -> anyhow::Result<()> {
    let outcome = with_git_deadline(ctx, timeout, |c| {
        let cmd = new_git_command(args);
        async move { processtree::run(&c, cmd, Duration::from_secs(5)).await }
    })
    .await;
    match outcome {
        Ok(res) => res,
        Err(_cause) => Err(git_deadline_error(timeout)),
    }
}

// ---------------------------------------------------------------------------
// Public data types (cache.go lines 149–183).
// ---------------------------------------------------------------------------

/// `RepoInfo` (cache.go:150–152): describes a repository to cache.
#[derive(Debug, Clone)]
pub(crate) struct RepoInfo {
    pub url: String,
}

/// `ErrRepoBusy` (cache.go:176): means a foreground checkout could not
/// acquire its repository within the caller's bounded wait.
#[derive(Debug, thiserror::Error)]
#[error("repository is busy")]
pub(crate) struct RepoBusyError;

/// Convenience constructor matching Go's sentinel comparison
/// (`errors.Is(err, ErrRepoBusy)`).
pub(crate) fn err_repo_busy() -> anyhow::Error {
    anyhow::Error::new(RepoBusyError)
}

/// `Activity` (cache.go:180–183): path-free repository coordination state
/// exposed through the daemon health endpoint. Diagnostic only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Activity {
    pub maintenance_active: i32,
    pub foreground_waiters: i32,
}

// ---------------------------------------------------------------------------
// repoLock: foreground-priority mutex (cache.go lines 185–293).
// ---------------------------------------------------------------------------

/// True when `err`'s chain contains the repo-busy sentinel
/// (`errors.Is(err, ErrRepoBusy)`).
pub(crate) fn is_repo_busy(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|c| c.downcast_ref::<RepoBusyError>().is_some())
}

#[derive(Default)]
struct LockState {
    held: bool,
    maintenance: bool,
    /// Child [`Ctx`] handed to the active maintenance holder; cancelling it
    /// with [`CancelCause::Preempted`] mirrors `maintenanceCancel(cause)`.
    maintenance_cancel: Option<Ctx>,
    foreground_waiters: i32,
}

/// `repoLock` (cache.go:190–197): a foreground-priority mutex. Ordinary cache
/// mutations serialize exactly as they did with sync.Mutex. Low-priority
/// maintenance only starts on an idle repository and receives a context that
/// is cancelled as soon as a foreground operation queues. The maintenance
/// holder remains responsible for stopping its Git process tree before
/// unlocking.
/// Unlocks the repo lock on drop — the Rust analogue of Go's
/// `defer repoLock.Unlock()` inside WithRepoMaintenance.
pub(crate) struct MaintenanceGuard {
    repo_lock: Arc<RepoLock>,
}

impl Drop for MaintenanceGuard {
    fn drop(&mut self) {
        self.repo_lock.unlock();
    }
}

struct ForegroundGuard {
    repo_lock: Arc<RepoLock>,
}

impl Drop for ForegroundGuard {
    fn drop(&mut self) {
        self.repo_lock.unlock();
    }
}

#[derive(Default)]
pub(crate) struct RepoLock {
    state: Mutex<LockState>,
    notify: tokio::sync::Notify,
}

impl RepoLock {
    /// `newRepoLock` (cache.go:203–205).
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// `LockContext` (cache.go:207–235).
    pub(crate) async fn lock(&self, ctx: &Ctx) -> Result<(), CancelCause> {
        {
            let mut st = self.state.lock().unwrap();
            st.foreground_waiters += 1;
        }
        loop {
            // Pre-register interest in the notification while still holding
            // the state mutex so a concurrent signal cannot be missed
            // (Go relies on close(changed) under l.mu for the same guarantee).
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            {
                let mut st = self.state.lock().unwrap();
                if let Some(cause) = ctx.err() {
                    st.foreground_waiters -= 1;
                    drop(st);
                    self.notify.notify_waiters();
                    return Err(cause);
                }
                if let Some(m) = &st.maintenance_cancel {
                    // l.maintenanceCancel(ErrMaintenancePreempted)
                    m.cancel_with(CancelCause::Preempted);
                }
                if !st.held {
                    st.held = true;
                    st.maintenance = false;
                    st.foreground_waiters -= 1;
                    return Ok(());
                }
            }

            tokio::select! {
                _ = ctx.cancelled() => {}
                _ = notified => {}
            }
        }
    }

    /// `Unlock` (cache.go:241–255).
    ///
    /// # Panics
    /// Panics on unlocking an unlocked repository, matching Go.
    pub(crate) fn unlock(&self) {
        {
            let mut st = self.state.lock().unwrap();
            if !st.held {
                panic!("repocache: unlock of unlocked repository");
            }
            if let Some(m) = st.maintenance_cancel.take() {
                // maintenanceCancel(nil) — plain cancellation, no cause.
                m.cancel_with(CancelCause::Cancelled);
            }
            st.held = false;
            st.maintenance = false;
        }
        self.notify.notify_waiters();
    }

    /// `tryLockMaintenance` (cache.go:257–268). Returns the maintenance child
    /// context on success.
    pub(crate) fn try_lock_maintenance(&self, parent: &Ctx) -> Option<Ctx> {
        let mut st = self.state.lock().unwrap();
        if parent.err().is_some() || st.held || st.foreground_waiters > 0 {
            return None;
        }
        let mctx = parent.child();
        st.held = true;
        st.maintenance = true;
        st.maintenance_cancel = Some(mctx.clone());
        Some(mctx)
    }

    /// `activity` (cache.go:275–279).
    pub(crate) fn activity(&self) -> (bool, i32) {
        let st = self.state.lock().unwrap();
        (st.held && st.maintenance, st.foreground_waiters)
    }

    /// `cancelMaintenanceAndWait` (cache.go:281–293).
    pub(crate) async fn cancel_maintenance_and_wait(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            {
                let st = self.state.lock().unwrap();
                if !st.maintenance {
                    return;
                }
                if let Some(m) = &st.maintenance_cancel {
                    m.cancel_with(CancelCause::Preempted);
                }
            }
            notified.await;
        }
    }
}

// ---------------------------------------------------------------------------
// Cache (cache.go lines 160–170, 296–337).
// ---------------------------------------------------------------------------

/// `Cache` (cache.go:161–170): manages bare git clones for workspace
/// repositories.
pub struct Cache {
    /// base directory for all caches (e.g. ~/patchbay_workspaces/.repos)
    root: PathBuf,
    /// `repoLocks` maps bare repo path → dedicated mutex. Any mutating
    /// operation on a given bare repo (clone, fetch, worktree add, ref
    /// update) must hold its lock — git's own lockfiles (packed-refs.lock,
    /// config.lock, worktree admin dirs) don't tolerate parallel mutations
    /// on the same repo. Separate repos are independent and run concurrently.
    /// (Go used sync.Map; Mutex<HashMap> per task requirements.)
    repo_locks: Mutex<HashMap<PathBuf, Arc<RepoLock>>>,
}

impl Cache {
    /// `New` (cache.go:296–298): creates a new repo cache rooted at the given
    /// directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            repo_locks: Mutex::new(HashMap::new()),
        }
    }

    /// `lockForRepo` (cache.go:302–309): returns the mutex dedicated to the
    /// given bare repo path.
    pub(crate) fn lock_for_repo(&self, bare_path: &Path) -> Arc<RepoLock> {
        let mut locks = self.repo_locks.lock().unwrap();
        locks
            .entry(bare_path.to_path_buf())
            .or_insert_with(|| Arc::new(RepoLock::new()))
            .clone()
    }

    /// `Activity` (cache.go:313–324): reports aggregate repository
    /// coordination without exposing cache paths. Safe to call while
    /// operations are starting or finishing.
    pub(crate) fn activity(&self) -> Activity {
        let mut activity = Activity::default();
        let locks = self.repo_locks.lock().unwrap();
        for lock in locks.values() {
            let (maintenance, waiters) = lock.activity();
            if maintenance {
                activity.maintenance_active += 1;
            }
            activity.foreground_waiters += waiters;
        }
        activity
    }

    /// `CancelMaintenance` (cache.go:332–337): stops every active
    /// low-priority repository operation and waits for it to release
    /// repository ownership. Task dispatch uses this as a barrier before
    /// launching an agent because a task may reuse an existing worktree and
    /// never pass through CreateWorktree's gate. Waiting also ensures
    /// interrupted-maintenance lock cleanup completes before direct agent Git
    /// work can create its own lock files.
    pub(crate) async fn cancel_maintenance(&self) {
        let locks: Vec<Arc<RepoLock>> = self.repo_locks.lock().unwrap().values().cloned().collect();
        for lock in locks {
            lock.cancel_maintenance_and_wait().await;
        }
    }

    /// `SyncContext` (cache.go:353–395): ensures all repos for a workspace
    /// are cloned (or fetched if already cached). Repos no longer in the list
    /// are left in place (cheap to keep, avoids re-cloning if a repo is
    /// temporarily removed and re-added).
    ///
    /// Per-repo mutation serializes against CreateWorktree on the same bare
    /// path via lock_for_repo. Different repos run sequentially within a
    /// single Sync call but concurrent Sync calls do not block each other.
    pub(crate) async fn sync_ctx(
        &self,
        ctx: &Ctx,
        workspace_id: &str,
        repos: &[RepoInfo],
    ) -> anyhow::Result<()> {
        let ws_dir = self.root.join(workspace_id);
        std::fs::create_dir_all(&ws_dir)
            .with_context(|| format!("create workspace cache dir: {}", ws_dir.display()))?;

        let mut first_err: Option<anyhow::Error> = None;
        for repo in repos {
            if let Some(cause) = ctx.err() {
                return Err(anyhow::anyhow!(cause.to_string()));
            }
            if repo.url.is_empty() {
                continue;
            }
            let bare_path = ws_dir.join(bare_dir_name(&repo.url));

            let repo_lock = self.lock_for_repo(&bare_path);
            repo_lock
                .lock(ctx)
                .await
                .map_err(|cause| anyhow::anyhow!(cause.to_string()))?;
            if is_bare_repo(&bare_path) {
                // Already cached — fetch latest.
                tracing::info!(url = %repo.url, path = %bare_path.display(), "repo cache: fetching");
                if let Err(err) = git_fetch_ctx(ctx, &bare_path).await {
                    tracing::warn!(url = %repo.url, error = %err, "repo cache: fetch failed");
                    if first_err.is_none() {
                        first_err = Some(err);
                    }
                }
            } else {
                // Not cached — bare clone.
                tracing::info!(url = %repo.url, path = %bare_path.display(), "repo cache: cloning");
                if let Err(err) = git_clone_bare_ctx(ctx, &repo.url, &bare_path).await {
                    tracing::error!(url = %repo.url, error = %err, "repo cache: clone failed");
                    if first_err.is_none() {
                        first_err = Some(err);
                    }
                }
            }
            repo_lock.unlock();
        }
        match first_err {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// `Lookup` (cache.go:399–405): returns the local bare clone path for a
    /// repo URL within a workspace. Returns `None` if not cached.
    pub(crate) fn lookup(&self, workspace_id: &str, url: &str) -> Option<PathBuf> {
        let bare_path = self.bare_path(workspace_id, url);
        if is_bare_repo(&bare_path) {
            Some(bare_path)
        } else {
            None
        }
    }

    /// `BarePath` (cache.go:411–413): returns where a repo's bare cache
    /// lives, whether or not it exists yet. Lookup is the "is it cached?"
    /// question; this is the "where would it be?" question, which the GC
    /// needs to map a set of live repo URLs onto the directories it is about
    /// to consider evicting.
    pub(crate) fn bare_path(&self, workspace_id: &str, url: &str) -> PathBuf {
        self.root.join(workspace_id).join(bare_dir_name(url))
    }
}

/// `lastUsedFile` (cache.go:424): records the last time a task asked for a
/// worktree from this bare repo. It lives inside the bare repo so it is
/// removed with it.
///
/// Directory mtime cannot answer this question. Every daemon restart re-syncs
/// each registered workspace's full repo list, and that path fetches every
/// cached repo (see Sync), refreshing the mtime of repos no task has checked
/// out in months. atime is worse: noatime is common on Linux and Windows
/// disables it by default. So the signal has to be written explicitly, at the
/// one place that means a repo was really used — CreateWorktree.
pub(crate) const LAST_USED_FILE: &str = ".patchbay_last_used";

/// `MarkUsed` (cache.go:430–438): records that this bare repo was just used
/// for a checkout. Callers must already hold the repo lock. Best-effort: a
/// failed stamp only risks the repo looking idle later, and the GC's own
/// missing-stamp grace period (see LastUsed) absorbs that.
pub(crate) fn mark_used(bare_path: &Path) {
    if bare_path.as_os_str().is_empty() {
        return;
    }
    let stamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    if let Err(err) = std::fs::write(bare_path.join(LAST_USED_FILE), stamp.as_bytes()) {
        tracing::warn!(
            repo = %bare_path.display(),
            error = %err,
            "repo cache: write last-used stamp failed"
        );
    }
}

/// `LastUsed` (cache.go:448–458): reports when this bare repo was last used
/// for a checkout, and whether a stamp existed at all.
///
/// ok=false means "unknown", never "ancient". Every cache created before this
/// stamp existed reports unknown, so treating it as infinitely old would make
/// the first GC cycle after an upgrade wipe every repo cache on the machine
/// and force a full re-clone of each. Callers must stamp an unknown repo and
/// let it age from now.
pub(crate) fn last_used(bare_path: &Path) -> Option<chrono::DateTime<chrono::Utc>> {
    let data = std::fs::read_to_string(bare_path.join(LAST_USED_FILE)).ok()?;
    parse_rfc3339_nano(data.trim())
}

/// Parses RFC3339 with optional nanosecond precision (Go time.RFC3339Nano
/// accepts fractional seconds of arbitrary length).
fn parse_rfc3339_nano(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

// ---------------------------------------------------------------------------
// WithRepoLock / WithRepoMaintenance / Fetch (cache.go lines 460–496).
// ---------------------------------------------------------------------------

impl Cache {
    /// `WithRepoMaintenance` (cache.go:481–489): runs fn only when the
    /// repository is idle. A foreground waiter cancels the context passed to
    /// fn, then waits for fn to stop its process tree and release the
    /// repository. ran=false means maintenance was skipped because foreground
    /// work already owned or was waiting for the repository.
    // The maintenance closure borrows the per-call context; the boxed
    // future is tied to that borrow via an explicit lifetime so callers can
    // capture their own environment in it.
    pub(crate) fn try_begin_maintenance(
        &self,
        ctx: &Ctx,
        bare_path: &Path,
    ) -> Option<(Ctx, MaintenanceGuard)> {
        let repo_lock = self.lock_for_repo(bare_path);
        let maintenance_ctx = repo_lock.try_lock_maintenance(ctx)?;
        Some((maintenance_ctx, MaintenanceGuard { repo_lock }))
    }
}

/// `bareDirName` (cache.go:515–546): returns a filesystem-safe,
/// collision-free directory name for the bare clone of rawURL. The name is
/// built from the host plus each path segment, joined by '+'. '+' is
/// disallowed in GitHub and GitLab path segments, so two URLs produce the
/// same name only if they point at the same repository on the same host.
///
/// Examples:
/// - `https://github.com/org/my-repo.git` → `github.com+org+my-repo.git`
/// - `git@github.com:org/my-repo`         → `github.com+org+my-repo.git`
/// - `ssh://git@gitlab.example.com:22/g/s/r.git` → `gitlab.example.com%3A22+g+s+r.git`
/// - `my-repo` → `my-repo.git` (bare name fallback)
pub(crate) fn bare_dir_name(raw_url: &str) -> String {
    let raw_url = raw_url.trim_end_matches('/');

    let (host, path) = split_host_and_path(raw_url);
    let host = host.trim().to_lowercase();
    // Encode ':' as '%3A' so host:port is lossless. A naive ':'->'-' rewrite
    // would collapse `gitlab.example.com:22` onto a literal hostname
    // `gitlab.example.com-22`, reintroducing the silent wrong-remote class
    // this function exists to prevent.
    let host = host.replace(':', "%3A");

    let mut parts: Vec<&str> = Vec::new();
    if !host.is_empty() {
        parts.push(&host);
    }
    for seg in path.split('/') {
        if !seg.is_empty() {
            parts.push(seg);
        }
    }

    let mut name = parts.join("+");
    if !name.ends_with(".git") {
        name.push_str(".git");
    }
    if name.is_empty() || name == ".git" {
        name = "repo.git".to_string();
    }
    name
}

/// `splitHostAndPath` (cache.go:557–569): extracts the host and
/// path-with-namespace from the supported git URL forms:
/// - URL form (`ssh://user@host[:port]/path`, `https://host/path`) — host
///   verbatim (may include :port), path without leading slash.
/// - scp-style (`[user@]host:path`) — splits on the first ':' after the
///   optional 'user@'.
/// - Anything else (bare repo names, absolute filesystem paths) — empty host
///   and raw input as the path.
fn split_host_and_path(raw_url: &str) -> (String, String) {
    if let Some((host, path)) = parse_url_host_and_path(raw_url) {
        return (host, path);
    }
    let s = raw_url;
    let s = match s.find('@') {
        Some(i) => &s[i + 1..],
        None => s,
    };
    if let Some(i) = s.find(':') {
        return (s[..i].to_string(), s[i + 1..].to_string());
    }
    (String::new(), s.to_string())
}

/// Minimal URL scheme/host/path extraction equivalent to Go's
/// `url.Parse` check `u.Scheme != "" && u.Host != ""` for the git URL shapes
/// this code accepts. Handles `scheme://[user@]host[:port]/path`.
fn parse_url_host_and_path(raw_url: &str) -> Option<(String, String)> {
    let scheme_end = raw_url.find("://")?;
    let scheme = &raw_url[..scheme_end];
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
    {
        return None;
    }
    let rest = &raw_url[scheme_end + 3..];
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].trim_start_matches('/')),
        None => (rest, ""),
    };
    if authority.is_empty() {
        return None;
    }
    // Strip userinfo (user@ / user:pass@) — Go's u.Host excludes it.
    let host = match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), path.to_string()))
}

/// `isBareRepo` (cache.go:572–576): checks if a path looks like a bare git
/// repository (a HEAD file at the root).
pub(crate) fn is_bare_repo(path: &Path) -> bool {
    path.join("HEAD").exists()
}

/// `modernFetchRefspec` (cache.go:583): remote-tracking refspec that keeps
/// fetched heads out of the bare repo's refs/heads/* namespace.
pub(crate) const MODERN_FETCH_REFSPEC: &str = "+refs/heads/*:refs/remotes/origin/*";

/// `gitCloneBareContext` (cache.go:589–604).
async fn git_clone_bare_ctx(ctx: &Ctx, url: &str, dest: &Path) -> anyhow::Result<()> {
    if let Err(err) =
        run_git_combined_output(ctx, &["clone", "--bare", url, &dest.to_string_lossy()]).await
    {
        // Clean up partial clone.
        let _ = std::fs::remove_dir_all(dest);
        let out = extract_combined_output(&err);
        return Err(err.context(format!("git clone --bare: {}", out.trim())));
    }
    // `git clone --bare` populates refs/heads/* as a snapshot and defaults to
    // a mirror-style fetch refspec. Convert the bare repo to the standard
    // remote-tracking layout immediately so subsequent fetches write to
    // refs/remotes/origin/* and can't conflict with worktree-locked heads.
    if let Err(err) = ensure_remote_tracking_layout_ctx(ctx, dest).await {
        let _ = std::fs::remove_dir_all(dest);
        return Err(err.context("configure fetch refspec"));
    }
    Ok(())
}

/// Recovers the combined-output text embedded by run_git_combined_output
/// wrappers so `%s` interpolation in outer wraps matches Go's formatting.
fn extract_combined_output(err: &anyhow::Error) -> String {
    for cause in err.chain() {
        if let Some(co) = cause.downcast_ref::<CombinedOutputFailure>() {
            return co.output.clone();
        }
    }
    String::new()
}

/// Side-channel carrying the trimmed combined output of a failed git command
/// so nested fmt.Errorf("%s: %w") layers can reproduce Go's message shape.
#[derive(Debug, thiserror::Error)]
#[error("{output}")]
pub(crate) struct CombinedOutputFailure {
    pub(crate) output: String,
}

/// `gitFetchContext` (cache.go:619–634): runs `git fetch origin` on a bare
/// cache, migrating its fetch refspec to the remote-tracking layout first if
/// it's still using the legacy mirror-style layout. After a successful fetch
/// it also refreshes refs/remotes/origin/HEAD so a remote default-branch
/// change actually takes effect in getRemoteDefaultBranch.
pub(crate) async fn git_fetch_ctx(ctx: &Ctx, bare_path: &Path) -> anyhow::Result<()> {
    ensure_remote_tracking_layout_ctx(ctx, bare_path)
        .await
        .context("ensure refspec")?;
    run_git_fetch_ctx(ctx, bare_path).await?;
    // Refresh refs/remotes/origin/HEAD after every successful fetch.
    // set-head --auto is lightweight and non-fatal.
    let _ = run_git(
        ctx,
        &[
            "-C",
            &bare_path.to_string_lossy(),
            "remote",
            "set-head",
            "origin",
            "--auto",
        ],
    )
    .await;
    Ok(())
}

/// `runGitFetchContext` (cache.go:642–647): the raw `git fetch origin`
/// wrapper. Callers should go through git_fetch, which migrates legacy caches
/// first.
async fn run_git_fetch_ctx(ctx: &Ctx, bare_path: &Path) -> anyhow::Result<()> {
    if let Err(err) = run_git_combined_output(
        ctx,
        &["-C", &bare_path.to_string_lossy(), "fetch", "origin"],
    )
    .await
    {
        let out = extract_combined_output(&err);
        return Err(err.context(format!("git fetch: {}", out.trim())));
    }
    Ok(())
}

/// `ensureRemoteTrackingLayoutContext` (cache.go:660–681): upgrades a bare
/// repo from the legacy mirror refspec (+refs/heads/*:refs/heads/*) to the
/// standard remote-tracking refspec. Idempotent.
async fn ensure_remote_tracking_layout_ctx(ctx: &Ctx, bare_path: &Path) -> anyhow::Result<()> {
    let cur = read_fetch_refspec_ctx(ctx, bare_path).await?;
    let modern_without_plus = MODERN_FETCH_REFSPEC.trim_start_matches('+');
    if cur == MODERN_FETCH_REFSPEC || cur == modern_without_plus {
        return Ok(()); // already modern
    }
    set_fetch_refspec_ctx(ctx, bare_path, MODERN_FETCH_REFSPEC).await?;
    // Backfill refs/remotes/origin/* by fetching with the new refspec.
    run_git_fetch_ctx(ctx, bare_path)
        .await
        .context("backfill fetch after refspec migration")?;
    // Set refs/remotes/origin/HEAD so getRemoteDefaultBranch can read it.
    // Non-fatal.
    let _ = run_git(
        ctx,
        &[
            "-C",
            &bare_path.to_string_lossy(),
            "remote",
            "set-head",
            "origin",
            "--auto",
        ],
    )
    .await;
    Ok(())
}

/// `readFetchRefspecContext` (cache.go:690–699): returns the current
/// remote.origin.fetch config value, or the empty string if it's not set.
/// Distinguishes "missing" (exit 1) from real git errors.
async fn read_fetch_refspec_ctx(ctx: &Ctx, bare_path: &Path) -> anyhow::Result<String> {
    match run_git_output(
        ctx,
        &[
            "-C",
            &bare_path.to_string_lossy(),
            "config",
            "--get",
            "remote.origin.fetch",
        ],
    )
    .await
    {
        Ok(out) => Ok(String::from_utf8_lossy(&out).trim().to_string()),
        Err(err) => {
            if exit_code_of(&err) == Some(1) {
                return Ok(String::new()); // key missing, not an error
            }
            Err(err.context("read remote.origin.fetch"))
        }
    }
}

/// Extracts an exec exit code from an error chain
/// (`exec.ExitError.ExitCode()` equivalent).
pub(crate) fn exit_code_of(err: &anyhow::Error) -> Option<i32> {
    for cause in err.chain() {
        if let Some(processtree::ProcessError::Exit(code)) =
            cause.downcast_ref::<processtree::ProcessError>()
        {
            return Some(*code);
        }
    }
    None
}

/// `setFetchRefspecContext` (cache.go:705–711).
async fn set_fetch_refspec_ctx(ctx: &Ctx, bare_path: &Path, refspec: &str) -> anyhow::Result<()> {
    if let Err(err) = run_git_combined_output(
        ctx,
        &[
            "-C",
            &bare_path.to_string_lossy(),
            "config",
            "remote.origin.fetch",
            refspec,
        ],
    )
    .await
    {
        let out = extract_combined_output(&err);
        return Err(err.context(format!("set remote.origin.fetch: {}", out.trim())));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Worktree creation (cache.go lines 713–949+).
// ---------------------------------------------------------------------------

/// `WorktreeParams` (cache.go:714–733): inputs for creating a worktree from a
/// cached bare clone.
#[derive(Debug, Clone)]
pub(crate) struct WorktreeParams {
    /// workspace that owns the repo
    pub workspace_id: String,
    /// remote URL to look up in the cache
    pub repo_url: String,
    /// parent directory for the worktree (e.g. task workdir)
    pub work_dir: PathBuf,
    /// optional branch, tag, or commit to base the worktree on
    pub reference: String,
    /// for branch naming
    pub agent_name: String,
    /// for branch naming uniqueness
    pub task_id: String,
    /// install prepare-commit-msg hook for Co-authored-by trailer
    pub co_authored_by_enabled: bool,
    /// Bounds only the wait for another same-repository operation. Zero
    /// preserves the historical unbounded wait for internal and older
    /// callers; retry-aware HTTP checkout requests set a finite value.
    pub lock_wait_timeout: Duration,
    /// Creates a local clone whose .git directory lives inside work_dir
    /// instead of a linked worktree whose gitdir lives under the shared
    /// cache. Codex tasks need this because workspace-write keeps a resolved
    /// external worktree gitdir read-only even when explicitly listed as a
    /// writable root (upstream repository issues 2925 and 6449).
    pub isolated_git_metadata: bool,
}

/// `WorktreeResult` (cache.go:736–739): describes a successfully created
/// worktree.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct WorktreeResult {
    /// absolute path to the worktree
    #[serde(rename = "path")]
    pub path: PathBuf,
    /// git branch created for this worktree
    #[serde(rename = "branch_name")]
    pub branch_name: String,
}

impl Cache {
    /// `CreateWorktreeContext` (cache.go:753–…): CreateWorktree with
    /// cancellation propagated through lock acquisition and every Git
    /// subprocess. Git work begins only after the lock is held, so a client
    /// that times out behind maintenance cannot leave a late, unwanted
    /// checkout.
    pub(crate) async fn create_worktree_ctx(
        &self,
        ctx: &Ctx,
        params: WorktreeParams,
    ) -> anyhow::Result<WorktreeResult> {
        let Some(bare_path) = self.lookup(&params.workspace_id, &params.repo_url) else {
            anyhow::bail!(
                "repo not found in cache: {} (workspace: {})",
                params.repo_url,
                params.workspace_id
            );
        };

        // Serialize concurrent CreateWorktree calls on the same bare repo.
        let repo_lock = self.lock_for_repo(&bare_path);

        // lockCtx := ctx (+ optional WithTimeout); mirrors cache.go:763–769.
        let bounded = params.lock_wait_timeout > Duration::ZERO;
        let lock_ctx = if bounded { ctx.child() } else { ctx.clone() };

        let lock_fut = repo_lock.lock(&lock_ctx);
        tokio::pin!(lock_fut);
        let lock_result = if bounded {
            tokio::select! {
                r = &mut lock_fut => Some(r),
                _ = tokio::time::sleep(params.lock_wait_timeout) => None,
            }
        } else {
            Some((&mut lock_fut).await)
        };
        match lock_result {
            Some(Ok(())) => {}
            Some(Err(cause)) => {
                if ctx.err().is_none() && cause == CancelCause::DeadlineExceeded {
                    // cache.go:771–773: "%w: %s" with ErrRepoBusy.
                    return Err(err_repo_busy().context(params.repo_url.clone()));
                }
                return Err(anyhow::anyhow!(cause.to_string()));
            }
            None => {
                // Timer expired first: cancel like Go's deferred cancel() and
                // let the lock wait unwind. If acquisition won the race we
                // still proceed (same as Go acquiring just before cancel).
                lock_ctx.cancel_with(CancelCause::DeadlineExceeded);
                if let Err(cause) = (&mut lock_fut).await {
                    if ctx.err().is_none() && cause == CancelCause::DeadlineExceeded {
                        return Err(err_repo_busy().context(params.repo_url.clone()));
                    }
                    return Err(anyhow::anyhow!(cause.to_string()));
                }
            }
        }
        let _repo_guard = ForegroundGuard {
            repo_lock: repo_lock.clone(),
        };
        if let Some(cause) = ctx.err() {
            return Err(anyhow::anyhow!(cause.to_string()));
        }

        // Stamp before doing the work, not after: a task asking for a
        // worktree is what "this cache is still wanted" means, whether or not
        // the checkout ultimately succeeds (cache.go:781–785).
        mark_used(&bare_path);

        // Fetch latest from origin. Non-fatal on failure (cache.go:787–802).
        if let Err(err) = git_fetch_ctx(ctx, &bare_path).await {
            if ctx.err().is_some() {
                return Err(anyhow::anyhow!(ctx.cause().to_string()));
            }
            tracing::warn!(
                url = %params.repo_url,
                error = %err,
                "repo checkout: fetch failed, agent will see possibly stale code"
            );
        }
        if let Some(cause) = ctx.err() {
            return Err(anyhow::anyhow!(cause.to_string()));
        }

        // Determine the ref to base the worktree on (cache.go:807–820).
        let base_ref = resolve_base_ref_ctx(ctx, &bare_path, &params.reference).await?;
        if let Some(cause) = ctx.err() {
            return Err(anyhow::anyhow!(cause.to_string()));
        }

        // Empty here means params.reference was unset and
        // getRemoteDefaultBranch couldn't resolve a default (cache.go:822–830).
        if base_ref.is_empty() {
            anyhow::bail!(
                "cannot resolve default branch for {}: bare cache at {} has no usable refs (origin/* is empty or ambiguous and bare HEAD has no match). The cache may be corrupted; delete it and retry",
                params.repo_url,
                bare_path.display()
            );
        }

        // Build branch name: agent/{sanitized-name}/{task-id} (cache.go:833).
        let branch_name = format!(
            "agent/{}/{}",
            sanitize_name(&params.agent_name),
            task_key(&params.task_id)
        );

        // Derive directory name from repo URL (cache.go:836–837).
        let dir_name = repo_name_from_url(&params.repo_url);
        let worktree_path = params.work_dir.join(dir_name);

        // Once a workdir has moved to isolated metadata, keep using that
        // safer shape (cache.go:839–843).
        if params.isolated_git_metadata || is_isolated_checkout_ctx(ctx, &worktree_path).await {
            let actual_branch = self
                .create_or_update_isolated_checkout_ctx(
                    ctx,
                    &bare_path,
                    &params.repo_url,
                    &worktree_path,
                    &branch_name,
                    &base_ref,
                )
                .await
                .context("create isolated checkout")?;

            for pattern in AGENT_GIT_EXCLUDE_PATTERNS {
                let _ = exclude_from_git_ctx(ctx, &worktree_path, pattern).await;
            }
            if params.co_authored_by_enabled {
                if let Err(err) = install_co_authored_by_hook_ctx(ctx, &worktree_path).await {
                    tracing::warn!(
                        error = %err,
                        "repo checkout: install co-authored-by hook failed (non-fatal)"
                    );
                }
            } else if let Err(err) = remove_co_authored_by_hook_ctx(ctx, &worktree_path).await {
                tracing::warn!(
                    error = %err,
                    "repo checkout: remove co-authored-by hook failed (non-fatal)"
                );
            }
            if let Some(cause) = ctx.err() {
                return Err(anyhow::anyhow!(cause.to_string()));
            }

            tracing::info!(
                url = %params.repo_url,
                path = %worktree_path.display(),
                branch = %actual_branch,
                base = %base_ref,
                "repo checkout: isolated checkout ready"
            );
            return Ok(WorktreeResult {
                path: worktree_path,
                branch_name: actual_branch,
            });
        }

        // If worktree already exists (reused environment from a prior task),
        // update it to the latest remote code instead of creating a new one
        // (cache.go:881–922).
        if is_git_worktree(&worktree_path) {
            let actual_branch =
                update_existing_worktree_ctx(ctx, &worktree_path, &branch_name, &base_ref)
                    .await
                    .context("update existing worktree")?;

            for pattern in AGENT_GIT_EXCLUDE_PATTERNS {
                let _ = exclude_from_git_ctx(ctx, &worktree_path, pattern).await;
            }

            if params.co_authored_by_enabled {
                if let Err(err) = install_co_authored_by_hook_ctx(ctx, &worktree_path).await {
                    tracing::warn!(
                        error = %err,
                        "repo checkout: install co-authored-by hook failed (non-fatal)"
                    );
                }
            } else if let Err(err) = remove_co_authored_by_hook_ctx(ctx, &worktree_path).await {
                tracing::warn!(
                    error = %err,
                    "repo checkout: remove co-authored-by hook failed (non-fatal)"
                );
            }
            if let Some(cause) = ctx.err() {
                return Err(anyhow::anyhow!(cause.to_string()));
            }

            tracing::info!(
                url = %params.repo_url,
                path = %worktree_path.display(),
                branch = %actual_branch,
                base = %base_ref,
                "repo checkout: existing worktree updated"
            );

            return Ok(WorktreeResult {
                path: worktree_path,
                branch_name: actual_branch,
            });
        }

        // Create a new worktree. createWorktree may rename the branch to
        // avoid collisions with stale per-task refs left over from previous
        // runs (cache.go:924–934).
        let actual_branch =
            create_worktree_ctx(ctx, &bare_path, &worktree_path, &branch_name, &base_ref)
                .await
                .context("create worktree")?;

        // Exclude agent context files from git tracking.
        for pattern in AGENT_GIT_EXCLUDE_PATTERNS {
            let _ = exclude_from_git_ctx(ctx, &worktree_path, pattern).await;
        }

        if params.co_authored_by_enabled {
            if let Err(err) = install_co_authored_by_hook_ctx(ctx, &worktree_path).await {
                tracing::warn!(
                    error = %err,
                    "repo checkout: install co-authored-by hook failed (non-fatal)"
                );
            }
        } else if let Err(err) = remove_co_authored_by_hook_ctx(ctx, &worktree_path).await {
            tracing::warn!(
                error = %err,
                "repo checkout: remove co-authored-by hook failed (non-fatal)"
            );
        }
        if let Some(cause) = ctx.err() {
            return Err(anyhow::anyhow!(cause.to_string()));
        }

        tracing::info!(
            url = %params.repo_url,
            path = %worktree_path.display(),
            branch = %actual_branch,
            base = %base_ref,
            "repo checkout: worktree created"
        );

        Ok(WorktreeResult {
            path: worktree_path,
            branch_name: actual_branch,
        })
    }
}

// ---------------------------------------------------------------------------
// Isolated checkouts (cache.go lines 965–1150).
// ---------------------------------------------------------------------------

/// `isolatedCheckoutConfigKey` / `isolatedCheckoutConfigValue` /
/// `isolatedCacheRemoteName` (cache.go:966–968).
const ISOLATED_CHECKOUT_CONFIG_KEY: &str = "patchbay.checkout-mode";
const ISOLATED_CHECKOUT_CONFIG_VALUE: &str = "isolated";
const ISOLATED_CACHE_REMOTE_NAME: &str = "patchbay-cache";

impl Cache {
    /// `createOrUpdateIsolatedCheckoutContext` (cache.go:982–1030): keeps Git
    /// metadata inside the task workdir. The fresh path uses a local clone, so
    /// immutable Git objects are hard-linked from the daemon's cache while
    /// refs, index, logs, config, and new objects remain private to the task
    /// checkout. The temporary cache remote is then replaced with the real
    /// repository URL so an agent's normal fetch/push commands still target
    /// GitHub rather than the daemon-owned bare cache.
    async fn create_or_update_isolated_checkout_ctx(
        &self,
        ctx: &Ctx,
        bare_path: &Path,
        repo_url: &str,
        checkout_path: &Path,
        branch_name: &str,
        base_ref: &str,
    ) -> anyhow::Result<String> {
        let base_commit = resolve_commit_ctx(ctx, bare_path, base_ref).await?;

        if is_isolated_checkout_ctx(ctx, checkout_path).await {
            set_isolated_checkout_origin_ctx(ctx, checkout_path, repo_url).await?;
            // Idempotent, and required for a workdir that was first created
            // while the cache was still a full clone: without it, a checkout
            // backed by a blobless cache resolves missing blobs to nothing
            // instead of fetching.
            if is_partial_clone_ctx(ctx, bare_path).await {
                configure_promisor_remote_ctx(ctx, checkout_path).await?;
            }
            sync_isolated_checkout_refs_ctx(ctx, bare_path, checkout_path, base_ref).await?;
            let actual_branch =
                update_existing_worktree_ctx(ctx, checkout_path, branch_name, &base_commit).await?;
            // Drop earlier tasks' agent/* heads so a reused workdir doesn't
            // grow a new local branch on every checkout. Non-fatal.
            if let Err(err) =
                delete_stale_agent_branches_ctx(ctx, checkout_path, &actual_branch).await
            {
                tracing::warn!(error = %err, "repo checkout: prune stale branches failed (non-fatal)");
            }
            return Ok(actual_branch);
        }
        // A daemon upgrade can resume a pre-fix Codex workdir that still has a
        // linked worktree. Remove it through Git (so the shared admin record is
        // cleaned too), then recreate the same checkout path with local
        // metadata.
        if is_git_worktree(checkout_path) {
            remove_linked_worktree_ctx(ctx, bare_path, checkout_path).await?;
        }
        match std::fs::symlink_metadata(checkout_path) {
            Ok(_) => anyhow::bail!(
                "checkout path already exists and is not a Patchbay isolated checkout: {}",
                checkout_path.display()
            ),
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                return Err(anyhow::Error::new(err).context("stat checkout path"));
            }
            Err(_) => {}
        }

        create_isolated_checkout_ctx(
            ctx,
            bare_path,
            repo_url,
            checkout_path,
            branch_name,
            base_ref,
            &base_commit,
        )
        .await
    }
}

/// `removeLinkedWorktreeContext` (cache.go:1036–1052).
async fn remove_linked_worktree_ctx(
    ctx: &Ctx,
    bare_path: &Path,
    checkout_path: &Path,
) -> anyhow::Result<()> {
    let out = run_git_output(
        ctx,
        &[
            "-C",
            &checkout_path.to_string_lossy(),
            "rev-parse",
            "--git-common-dir",
        ],
    )
    .await
    .context("resolve linked worktree common dir")?;
    let mut common_dir = PathBuf::from(String::from_utf8_lossy(&out).trim());
    if !common_dir.is_absolute() {
        common_dir = checkout_path.join(common_dir);
    }
    if !same_resolved_path(&common_dir, bare_path) {
        anyhow::bail!(
            "linked worktree common dir {} does not match cache {}",
            common_dir.display(),
            bare_path.display()
        );
    }
    if let Err(err) = run_git_combined_output(
        ctx,
        &[
            "-C",
            &bare_path.to_string_lossy(),
            "worktree",
            "remove",
            "--force",
            &checkout_path.to_string_lossy(),
        ],
    )
    .await
    {
        let out_text = extract_combined_output(&err);
        return Err(err.context(format!("remove linked worktree: {}", out_text.trim())));
    }
    Ok(())
}

/// `sameResolvedPath` (cache.go:1059–1071): reports whether a and b denote
/// the same location after resolving to absolute, symlink-free, cleaned form.
/// It is a path-equality check (not a same-device check).
pub(crate) fn same_resolved_path(a: &Path, b: &Path) -> bool {
    fn clean(path: &Path) -> PathBuf {
        match std::fs::canonicalize(path) {
            Ok(resolved) => resolved,
            Err(_) => {
                // Fall back to absolute + lexical clean when the target does
                // not exist or cannot be resolved.
                match std::path::absolute(path) {
                    Ok(abs) => normalize_lexically(&abs),
                    Err(_) => normalize_lexically(path),
                }
            }
        }
    }
    clean(a) == clean(b)
}

/// Lexical `filepath.Clean` equivalent for absolute paths: collapses `.`,
/// `..`, and duplicate separators without touching the filesystem.
pub(crate) fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::from("/");
    for comp in path.components() {
        use std::path::Component;
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `localCloneArgs` (cache.go:1083–1089): builds the `git clone --local`
/// invocation that seeds a fresh isolated checkout from the shared bare cache.
/// On Windows the clone also passes --no-hardlinks so a cache and workdir on
/// different drives keep working and sandboxed checkouts cannot re-permission
/// the daemon-owned cache's object files.
fn local_clone_args(goos: &str, bare_path: &Path, checkout_path: &Path) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "clone".into(),
        "--local".into(),
        "--no-checkout".into(),
        "--no-tags".into(),
    ];
    if goos == "windows" {
        args.push("--no-hardlinks".into());
    }
    args.push("--origin".into());
    args.push(ISOLATED_CACHE_REMOTE_NAME.into());
    args.push(bare_path.to_string_lossy().into_owned());
    args.push(checkout_path.to_string_lossy().into_owned());
    args
}

/// `createIsolatedCheckoutContext` (cache.go:1095–1150).
async fn create_isolated_checkout_ctx(
    ctx: &Ctx,
    bare_path: &Path,
    repo_url: &str,
    checkout_path: &Path,
    branch_name: &str,
    base_ref: &str,
    base_commit: &str,
) -> anyhow::Result<String> {
    let goos = if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unix"
    };
    let clone_args_vec = local_clone_args(goos, bare_path, checkout_path);
    let clone_args: Vec<&str> = clone_args_vec.iter().map(String::as_str).collect();
    if let Err(err) = run_git_combined_output(ctx, &clone_args).await {
        // Do not remove checkoutPath here. A different repository with the
        // same basename could have won the path race after our pre-check; Git
        // then fails safely, and deleting the path would destroy its checkout.
        let out_text = extract_combined_output(&err);
        return Err(err.context(format!("git clone --local: {}", out_text.trim())));
    }

    // cleanup flag mirrors Go's named-return defer (cache.go:1105–1110).
    struct CleanupOnDrop<'a> {
        path: &'a Path,
        active: bool,
    }
    impl Drop for CleanupOnDrop<'_> {
        fn drop(&mut self) {
            if self.active {
                let _ = std::fs::remove_dir_all(self.path);
            }
        }
    }
    let mut cleanup = CleanupOnDrop {
        path: checkout_path,
        active: true,
    };

    // The origin swap has to happen before the first checkout when the cache
    // is a partial clone. `git clone --local` hardlinks the objects it can
    // see and does NOT inherit the promisor configuration, so a blobless
    // cache yields a checkout whose blobs are unreachable — and git reports
    // that as success with every file "deleted" rather than as an error.
    if let Err(err) = run_git_combined_output(
        ctx,
        &[
            "-C",
            &checkout_path.to_string_lossy(),
            "remote",
            "remove",
            ISOLATED_CACHE_REMOTE_NAME,
        ],
    )
    .await
    {
        let out_text = extract_combined_output(&err);
        return Err(err.context(format!("remove cache remote: {}", out_text.trim())));
    }
    if let Err(err) = run_git_combined_output(
        ctx,
        &[
            "-C",
            &checkout_path.to_string_lossy(),
            "remote",
            "add",
            "origin",
            repo_url,
        ],
    )
    .await
    {
        let out_text = extract_combined_output(&err);
        return Err(err.context(format!("add origin remote: {}", out_text.trim())));
    }
    if is_partial_clone_ctx(ctx, bare_path).await {
        configure_promisor_remote_ctx(ctx, checkout_path).await?;
    }

    if let Err(err) = run_git_combined_output(
        ctx,
        &[
            "-C",
            &checkout_path.to_string_lossy(),
            "checkout",
            "--detach",
            base_commit,
        ],
    )
    .await
    {
        let out_text = extract_combined_output(&err);
        return Err(err.context(format!("git checkout --detach: {}", out_text.trim())));
    }
    delete_all_local_branches_ctx(ctx, checkout_path).await?;
    sync_isolated_checkout_refs_ctx(ctx, bare_path, checkout_path, base_ref).await?;
    if let Err(err) = run_git_combined_output(
        ctx,
        &[
            "-C",
            &checkout_path.to_string_lossy(),
            "config",
            ISOLATED_CHECKOUT_CONFIG_KEY,
            ISOLATED_CHECKOUT_CONFIG_VALUE,
        ],
    )
    .await
    {
        let out_text = extract_combined_output(&err);
        return Err(err.context(format!("mark isolated checkout: {}", out_text.trim())));
    }

    let actual_branch =
        checkout_new_branch_ctx(ctx, checkout_path, branch_name, base_commit).await?;
    cleanup.active = false;
    Ok(actual_branch)
}

/// `resolveCommitContext` (cache.go:1156–1166).
async fn resolve_commit_ctx(
    ctx: &Ctx,
    repo_path: &Path,
    reference: &str,
) -> anyhow::Result<String> {
    let out = run_git_output(
        ctx,
        &[
            "-C",
            &repo_path.to_string_lossy(),
            "rev-parse",
            "--verify",
            &format!("{reference}^{{commit}}"),
        ],
    )
    .await
    .with_context(|| format!("resolve checkout base {:?}", reference))?;
    let commit = String::from_utf8_lossy(&out).trim().to_string();
    if commit.is_empty() {
        anyhow::bail!("resolve checkout base {:?}: empty commit", reference);
    }
    Ok(commit)
}

/// `isIsolatedCheckoutContext` (cache.go:1172–1179).
async fn is_isolated_checkout_ctx(ctx: &Ctx, path: &Path) -> bool {
    let git_dir = path.join(".git");
    match std::fs::metadata(&git_dir) {
        Ok(info) if info.is_dir() => {}
        _ => return false,
    }
    matches!(
        run_git_output(
            ctx,
            &["-C", &path.to_string_lossy(), "config", "--get", ISOLATED_CHECKOUT_CONFIG_KEY]
        )
        .await,
        Ok(out) if String::from_utf8_lossy(&out).trim() == ISOLATED_CHECKOUT_CONFIG_VALUE
    )
}

/// `partialCloneFilter` (cache.go:1184): the object filter a blobless partial
/// clone is created with, and the value that has to be restored on any
/// repository that inherits such a clone's incomplete object store.
const PARTIAL_CLONE_FILTER: &str = "blob:none";

/// `isPartialCloneContext` (cache.go:1192–1198): reports whether a repository
/// was created as a partial clone, i.e. whether git will lazily fetch missing
/// objects from its promisor remote.
async fn is_partial_clone_ctx(ctx: &Ctx, repo_path: &Path) -> bool {
    match tokio::time::timeout(
        Duration::from_secs(30),
        run_git_output(
            ctx,
            &[
                "-C",
                &repo_path.to_string_lossy(),
                "config",
                "--get",
                "remote.origin.promisor",
            ],
        ),
    )
    .await
    {
        Ok(Ok(out)) => String::from_utf8_lossy(&out).trim() == "true",
        _ => false,
    }
}

/// `configurePromisorRemoteContext` (cache.go:1209–1220): marks origin as the
/// promisor remote for a repository whose object store is incomplete, so git
/// lazily fetches missing blobs from the real remote instead of failing. It
/// mirrors the two config keys `git clone --filter=blob:none` writes;
/// `git clone --local` does not copy them across, which is why they have to
/// be restored by hand.
async fn configure_promisor_remote_ctx(ctx: &Ctx, repo_path: &Path) -> anyhow::Result<()> {
    let settings: [(&str, &str); 2] = [
        ("remote.origin.promisor", "true"),
        ("remote.origin.partialclonefilter", PARTIAL_CLONE_FILTER),
    ];
    for (key, value) in settings {
        if let Err(err) = run_git_combined_output(
            ctx,
            &["-C", &repo_path.to_string_lossy(), "config", key, value],
        )
        .await
        {
            let out_text = extract_combined_output(&err);
            return Err(err.context(format!("set {}: {}", key, out_text.trim())));
        }
    }
    Ok(())
}

/// `setIsolatedCheckoutOriginContext` (cache.go:1226–1232).
async fn set_isolated_checkout_origin_ctx(
    ctx: &Ctx,
    path: &Path,
    repo_url: &str,
) -> anyhow::Result<()> {
    if let Err(err) = run_git_combined_output(
        ctx,
        &[
            "-C",
            &path.to_string_lossy(),
            "remote",
            "set-url",
            "origin",
            repo_url,
        ],
    )
    .await
    {
        let out_text = extract_combined_output(&err);
        return Err(err.context(format!("set origin remote: {}", out_text.trim())));
    }
    Ok(())
}

/// `syncIsolatedCheckoutRefsContext` (cache.go:1242–1256): mirrors the
/// cache's real origin/* and tag refs into the task-local repository. Also
/// fetches the selected base ref directly so a reused checkout can move to a
/// newly-fetched commit without depending on the cache after this returns.
async fn sync_isolated_checkout_refs_ctx(
    ctx: &Ctx,
    bare_path: &Path,
    checkout_path: &Path,
    base_ref: &str,
) -> anyhow::Result<()> {
    let refspecs = [
        "+refs/remotes/origin/*:refs/remotes/origin/*",
        "+refs/tags/*:refs/tags/*",
    ];
    let mut args: Vec<String> = vec![
        "-C".into(),
        checkout_path.to_string_lossy().into_owned(),
        "fetch".into(),
        "--force".into(),
        "--no-tags".into(),
        bare_path.to_string_lossy().into_owned(),
    ];
    args.extend(refspecs.iter().map(|s| s.to_string()));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    if let Err(err) = run_git_combined_output(ctx, &arg_refs).await {
        let out_text = extract_combined_output(&err);
        return Err(err.context(format!("sync cache refs: {}", out_text.trim())));
    }
    if let Err(err) = run_git_combined_output(
        ctx,
        &[
            "-C",
            &checkout_path.to_string_lossy(),
            "fetch",
            "--force",
            "--no-tags",
            &bare_path.to_string_lossy(),
            base_ref,
        ],
    )
    .await
    {
        let out_text = extract_combined_output(&err);
        return Err(err.context(format!("fetch checkout base: {}", out_text.trim())));
    }
    Ok(())
}

/// `deleteAllLocalBranchesContext` (cache.go:1265–1267): removes the heads
/// copied from the bare cache into a fresh local clone. The clone is detached
/// and has no task-created branches to preserve yet.
async fn delete_all_local_branches_ctx(ctx: &Ctx, repo_path: &Path) -> anyhow::Result<()> {
    delete_local_branches_under_ctx(ctx, repo_path, "refs/heads/", "").await
}

/// `deleteStaleAgentBranchesContext` (cache.go:1275–1277): prunes branches
/// left by earlier Patchbay tasks while preserving the current task branch and
/// every user-created local branch.
async fn delete_stale_agent_branches_ctx(
    ctx: &Ctx,
    repo_path: &Path,
    keep_branch: &str,
) -> anyhow::Result<()> {
    delete_local_branches_under_ctx(
        ctx,
        repo_path,
        "refs/heads/agent/",
        &format!("refs/heads/{keep_branch}"),
    )
    .await
}

/// `deleteLocalBranchesUnderContext` (cache.go:1283–1298).
async fn delete_local_branches_under_ctx(
    ctx: &Ctx,
    repo_path: &Path,
    namespace: &str,
    keep_ref: &str,
) -> anyhow::Result<()> {
    let out = run_git_output(
        ctx,
        &[
            "-C",
            &repo_path.to_string_lossy(),
            "for-each-ref",
            "--format=%(refname)",
            namespace,
        ],
    )
    .await
    .context("list local branches")?;
    for line in String::from_utf8_lossy(&out).trim().split('\n') {
        let reference = line.trim();
        if reference.is_empty() || reference == keep_ref {
            continue;
        }
        if let Err(err) = run_git_combined_output(
            ctx,
            &[
                "-C",
                &repo_path.to_string_lossy(),
                "update-ref",
                "-d",
                reference,
            ],
        )
        .await
        {
            let out_text = extract_combined_output(&err);
            return Err(err.context(format!(
                "delete local branch {}: {}",
                reference,
                out_text.trim()
            )));
        }
    }
    Ok(())
}

/// `checkoutNewBranchContext` (cache.go:1304–1318).
async fn checkout_new_branch_ctx(
    ctx: &Ctx,
    repo_path: &Path,
    branch_name: &str,
    base_ref: &str,
) -> anyhow::Result<String> {
    let first = run_git_combined_output(
        ctx,
        &[
            "-C",
            &repo_path.to_string_lossy(),
            "checkout",
            "-b",
            branch_name,
            base_ref,
        ],
    )
    .await;
    match first {
        Ok(_) => return Ok(branch_name.to_string()),
        Err(err) if !is_branch_collision_error(&err) => {
            let out_text = extract_combined_output(&err);
            return Err(err.context(format!("git checkout -b: {}", out_text.trim())));
        }
        Err(_) => {}
    }
    // Branch collision fallback: retry once with a Unix-timestamp suffix.
    let retried_name = format!("{}-{}", branch_name, chrono::Utc::now().timestamp());
    if let Err(err) = run_git_combined_output(
        ctx,
        &[
            "-C",
            &repo_path.to_string_lossy(),
            "checkout",
            "-b",
            &retried_name,
            base_ref,
        ],
    )
    .await
    {
        let out_text = extract_combined_output(&err);
        return Err(err.context(format!("git checkout -b (retry): {}", out_text.trim())));
    }
    Ok(retried_name)
}

/// `resolveBaseRefContext` (cache.go:1324–1343).
async fn resolve_base_ref_ctx(
    ctx: &Ctx,
    bare_path: &Path,
    requested_ref: &str,
) -> anyhow::Result<String> {
    let reference = requested_ref.trim();
    if reference.is_empty() {
        return Ok(get_remote_default_branch_ctx(ctx, bare_path).await);
    }

    // Prefer remote-tracking branches for human branch names. Then allow full
    // local refs, tags, and raw commits that exist in the fetched bare cache.
    let candidates = [
        format!("refs/remotes/origin/{reference}"),
        format!("refs/tags/{reference}"),
        reference.to_string(),
    ];
    for candidate in &candidates {
        let probe = format!("{candidate}^{{commit}}");
        if git_ref_exists_ctx(ctx, bare_path, &probe).await {
            return Ok(candidate.clone());
        }
    }
    anyhow::bail!(
        "cannot resolve requested ref {:?} in repo cache at {}",
        reference,
        bare_path.display()
    )
}

/// `gitRefExistsContext` (cache.go:1349–1351).
async fn git_ref_exists_ctx(ctx: &Ctx, repo_path: &Path, reference: &str) -> bool {
    run_git(
        ctx,
        &[
            "-C",
            &repo_path.to_string_lossy(),
            "rev-parse",
            "--verify",
            "--quiet",
            reference,
        ],
    )
    .await
    .is_ok()
}

/// `createWorktreeContext` (cache.go:1360–1380): creates a git worktree at
/// the given path with a new branch. Returns the actual branch name used —
/// which may differ from the requested branchName if a collision was resolved
/// by appending a timestamp suffix.
async fn create_worktree_ctx(
    ctx: &Ctx,
    git_root: &Path,
    worktree_path: &Path,
    branch_name: &str,
    base_ref: &str,
) -> anyhow::Result<String> {
    // Pre-check: if the worktree path already exists we would get a confusing
    // "already exists" error from `git worktree add` — which used to be
    // misclassified as a branch collision, causing the retry to leak branches
    // into the bare repo. Fail cleanly here instead.
    if worktree_path.exists() {
        anyhow::bail!(
            "worktree path already exists and is not a valid git worktree: {}",
            worktree_path.display()
        );
    }

    let mut name = branch_name.to_string();
    let mut err = run_worktree_add_ctx(ctx, git_root, worktree_path, &name, base_ref).await;
    if err.is_err() && is_branch_collision_error(err.as_ref().unwrap_err()) {
        // Branch name collision: append timestamp and retry once.
        name = format!("{}-{}", branch_name, chrono::Utc::now().timestamp());
        err = run_worktree_add_ctx(ctx, git_root, worktree_path, &name, base_ref).await;
    }
    err?;
    Ok(name)
}

/// `runWorktreeAddContext` (cache.go:1386–1391).
async fn run_worktree_add_ctx(
    ctx: &Ctx,
    git_root: &Path,
    worktree_path: &Path,
    branch_name: &str,
    base_ref: &str,
) -> anyhow::Result<()> {
    if let Err(err) = run_git_combined_output(
        ctx,
        &[
            "-C",
            &git_root.to_string_lossy(),
            "worktree",
            "add",
            "-b",
            branch_name,
            &worktree_path.to_string_lossy(),
            base_ref,
        ],
    )
    .await
    {
        let out_text = extract_combined_output(&err);
        return Err(err.context(format!("git worktree add: {}", out_text.trim())));
    }
    Ok(())
}

/// `isBranchCollisionError` (cache.go:1398–1405): true only when the error is
/// specifically about a branch name already existing. Git's other "already
/// exists" messages (notably path collisions from `git worktree add`) must
/// NOT be treated as branch collisions, or the retry-with-timestamp logic
/// will leak branches while still failing on the original path collision.
fn is_branch_collision_error(err: &anyhow::Error) -> bool {
    // Git's message is "fatal: a branch named 'X' already exists".
    err.to_string().to_lowercase().contains("a branch named")
}

/// `isGitWorktree` (cache.go:1407–1412): checks if a path is an existing git
/// worktree. Worktrees have a .git *file* (not directory) that points to the
/// main repo.
pub(crate) fn is_git_worktree(path: &Path) -> bool {
    match std::fs::metadata(path.join(".git")) {
        Ok(info) => !info.is_dir(),
        Err(_) => false,
    }
}

/// `updateExistingWorktreeContext` (cache.go:1422–1452): resets the worktree
/// to a clean state and checks out a new branch from the default branch. The
/// caller is responsible for fetching the bare cache beforehand (worktrees
/// share the same object store). Returns the actual branch name used (may
/// differ from input on collision).
async fn update_existing_worktree_ctx(
    ctx: &Ctx,
    worktree_path: &Path,
    branch_name: &str,
    base_ref: &str,
) -> anyhow::Result<String> {
    // Discard any leftover uncommitted changes from the previous task.
    if let Err(err) = run_git_combined_output(
        ctx,
        &["-C", &worktree_path.to_string_lossy(), "reset", "--hard"],
    )
    .await
    {
        let out_text = extract_combined_output(&err);
        return Err(err.context(format!("git reset --hard: {}", out_text.trim())));
    }

    // Clean untracked files (e.g. build artifacts from previous task).
    if let Err(err) = run_git_combined_output(
        ctx,
        &["-C", &worktree_path.to_string_lossy(), "clean", "-fd"],
    )
    .await
    {
        let out_text = extract_combined_output(&err);
        return Err(err.context(format!("git clean -fd: {}", out_text.trim())));
    }

    // Create a new branch from the resolved default-branch ref and switch to
    // it. baseRef is a ref path returned by getRemoteDefaultBranch — usually
    // "refs/remotes/origin/<branch>" but may be "refs/heads/<branch>" on a
    // legacy/migration-pending cache. Either form is valid as a checkout
    // startpoint.
    let first = run_git_combined_output(
        ctx,
        &[
            "-C",
            &worktree_path.to_string_lossy(),
            "checkout",
            "-b",
            branch_name,
            base_ref,
        ],
    )
    .await;
    match first {
        Ok(_) => return Ok(branch_name.to_string()),
        Err(err) if !is_branch_collision_error(&err) => {
            let out_text = extract_combined_output(&err);
            return Err(err.context(format!("git checkout -b: {}", out_text.trim())));
        }
        Err(_) => {}
    }
    // Branch collision fallback mirrors checkoutNewBranchContext
    // (cache.go:1442–1451): retry once with a Unix-timestamp suffix.
    let retried_name = format!("{}-{}", branch_name, chrono::Utc::now().timestamp());
    if let Err(err) = run_git_combined_output(
        ctx,
        &[
            "-C",
            &worktree_path.to_string_lossy(),
            "checkout",
            "-b",
            &retried_name,
            base_ref,
        ],
    )
    .await
    {
        let out_text = extract_combined_output(&err);
        return Err(err.context(format!("git checkout -b (retry): {}", out_text.trim())));
    }
    Ok(retried_name)
}

/// `getRemoteDefaultBranchContext` (cache.go:1483–1550): returns a ref path
/// (e.g. "refs/remotes/origin/main") that points at the remote's default
/// branch in a bare cache, usable directly as a `git worktree add` /
/// `git checkout -b` startpoint.
///
/// Resolution order:
/// 1. refs/remotes/origin/HEAD (verified)
/// 2. refs/remotes/origin/main, refs/remotes/origin/master
/// 3. bare HEAD mapped into refs/remotes/origin/<same name>
/// 4. scan refs/remotes/origin/* — only when exactly one non-HEAD ref exists
/// 5. legacy last-resort: bare HEAD as plain refs/heads/* ref, gated on
///    origin/* being completely empty
///
/// Returns "" only when none of the above resolve — which the caller treats
/// as a hard error with a clear "cache has no usable refs" message.
async fn get_remote_default_branch_ctx(ctx: &Ctx, bare_path: &Path) -> String {
    let p = bare_path.to_string_lossy().into_owned();
    // 1) Primary: refs/remotes/origin/HEAD set by `git remote set-head
    //    origin --auto` during ensureRemoteTrackingLayout. Verify the target
    //    actually exists — a partial set-head or a manually-broken repo can
    //    leave a symref pointing at a deleted ref.
    if let Ok(out) =
        run_git_output(ctx, &["-C", &p, "symbolic-ref", "refs/remotes/origin/HEAD"]).await
    {
        let reference = String::from_utf8_lossy(&out).trim().to_string();
        if !reference.is_empty()
            && run_git(ctx, &["-C", &p, "rev-parse", "--verify", &reference])
                .await
                .is_ok()
        {
            return reference;
        }
    }
    // 2) Common default branch names under the origin namespace.
    for candidate in ["refs/remotes/origin/main", "refs/remotes/origin/master"] {
        if run_git(ctx, &["-C", &p, "rev-parse", "--verify", candidate])
            .await
            .is_ok()
        {
            return candidate.to_string();
        }
    }
    // 3) Use the bare repo's own HEAD as a hint. We only return when the
    //    matching origin/<name> exists, so we still pick up up-to-date code
    //    rather than a stale local head.
    let bare_ref = bare_head_branch_ctx(ctx, bare_path).await;
    if !bare_ref.is_empty() {
        let origin_ref = format!(
            "refs/remotes/origin/{}",
            bare_ref.strip_prefix("refs/heads/").unwrap_or(&bare_ref)
        );
        if run_git(ctx, &["-C", &p, "rev-parse", "--verify", &origin_ref])
            .await
            .is_ok()
        {
            return origin_ref;
        }
    }
    // 4) Scan refs/remotes/origin/* — return a result ONLY when there's
    //    exactly one non-HEAD candidate. Count entries so step 5 can tell
    //    "legacy empty" apart from "ambiguous".
    let mut origin_count = 0usize;
    let mut singleton = String::new();
    if let Ok(out) = run_git_output(
        ctx,
        &[
            "-C",
            &p,
            "for-each-ref",
            "--format=%(refname)",
            "refs/remotes/origin/",
        ],
    )
    .await
    {
        for line in String::from_utf8_lossy(&out).trim().split('\n') {
            let line = line.trim();
            if line.is_empty() || line == "refs/remotes/origin/HEAD" {
                continue;
            }
            origin_count += 1;
            if singleton.is_empty() {
                singleton = line.to_string();
            }
        }
        if origin_count == 1 {
            return singleton;
        }
    }
    // 5) Last-resort fallback gated on refs/remotes/origin/* being completely
    //    empty — an ambiguous cache must fail loudly instead of silently
    //    basing agent work on a stale snapshot.
    if origin_count == 0 && !bare_ref.is_empty() {
        return bare_ref;
    }
    String::new()
}

/// `bareHeadBranchContext` (cache.go:1564–1577): returns the bare repo's
/// local HEAD ref (e.g. "refs/heads/main") if HEAD is a symbolic ref to an
/// existing branch; "" when detached, missing, or pointing at a non-existent
/// ref. Only used by getRemoteDefaultBranch as a last-resort fallback.
async fn bare_head_branch_ctx(ctx: &Ctx, bare_path: &Path) -> String {
    let p = bare_path.to_string_lossy().into_owned();
    let Ok(out) = run_git_output(ctx, &["-C", &p, "symbolic-ref", "HEAD"]).await else {
        return String::new();
    };
    let reference = String::from_utf8_lossy(&out).trim().to_string();
    if reference.is_empty() {
        return String::new();
    }
    if run_git(ctx, &["-C", &p, "rev-parse", "--verify", &reference])
        .await
        .is_err()
    {
        return String::new();
    }
    reference
}

// ---------------------------------------------------------------------------
// Co-authored-by hook + exclude handling (cache.go lines 1579–1743).
// ---------------------------------------------------------------------------

/// `patchbayHookMarker` (cache.go:1583): sentinel comment embedded in every
/// prepare-commit-msg hook installed by the daemon.
/// removeCoAuthoredByHook uses it to recognize hooks it owns so it never
/// deletes a hook installed by the user or another tool.
const PATCHBAY_HOOK_MARKER: &str = "# patchbay:prepare-commit-msg:co-authored-by";

/// `daemonInstalledHookSignatures` (cache.go:1593–1596): substrings that
/// identify a prepare-commit-msg hook as daemon-installed. Deliberately
/// includes the legacy comment so disabling the toggle on existing
/// installations still cleans up old hooks. Add to this list — never remove
/// from it.
const DAEMON_INSTALLED_HOOK_SIGNATURES: &[&str] =
    &[PATCHBAY_HOOK_MARKER, "# Installed by the Patchbay daemon."];

/// `prepareCommitMsgHook` (cache.go:1600–1622): appends a Co-authored-by
/// trailer for the Patchbay Agent to every commit message.
const PREPARE_COMMIT_MSG_HOOK: &str = r#"#!/bin/sh
# patchbay:prepare-commit-msg:co-authored-by
# Patchbay: add Co-authored-by trailer for the Patchbay Agent.
# Installed by the Patchbay daemon. Do not edit — it will be overwritten.

COMMIT_MSG_FILE="$1"
COMMIT_SOURCE="$2"

# Skip merge and squash commits.
case "$COMMIT_SOURCE" in
  merge|squash) exit 0 ;;
esac

TRAILER="Co-authored-by: patchbay-agent <patchbay-agent@users.noreply.github.com>"

# Don't add if already present.
if grep -qF "$TRAILER" "$COMMIT_MSG_FILE"; then
  exit 0
fi

# Use git interpret-trailers for proper formatting.
git interpret-trailers --in-place --trailer "$TRAILER" "$COMMIT_MSG_FILE"
"#;

/// Resolves the git common directory for a working tree, absolutizing it
/// against `worktree_path` when git reports a relative path
/// (shared by install/remove hook paths, cache.go:1633–1640/1679–1686).
async fn resolve_git_common_dir(ctx: &Ctx, worktree_path: &Path) -> anyhow::Result<PathBuf> {
    let out = run_git_output(
        ctx,
        &[
            "-C",
            &worktree_path.to_string_lossy(),
            "rev-parse",
            "--git-common-dir",
        ],
    )
    .await
    .context("resolve git common dir")?;
    let common_dir = String::from_utf8_lossy(&out).trim().to_string();
    let common_dir = PathBuf::from(common_dir);
    if common_dir.is_absolute() {
        Ok(common_dir)
    } else {
        Ok(worktree_path.join(common_dir))
    }
}

/// `installCoAuthoredByHookContext` (cache.go:1632–1652): installs a
/// prepare-commit-msg git hook in the git common directory (the bare repo for
/// worktrees) so it applies to all worktrees created from this cache.
async fn install_co_authored_by_hook_ctx(ctx: &Ctx, worktree_path: &Path) -> anyhow::Result<()> {
    let common_dir = resolve_git_common_dir(ctx, worktree_path).await?;

    let hooks_dir = common_dir.join("hooks");
    std::fs::create_dir_all(&hooks_dir).context("create hooks dir")?;

    let hook_path = hooks_dir.join("prepare-commit-msg");
    std::fs::write(&hook_path, PREPARE_COMMIT_MSG_HOOK).context("write prepare-commit-msg hook")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755))
            .context("write prepare-commit-msg hook")?;
    }
    Ok(())
}

/// `isDaemonInstalledHook` (cache.go:1658–1666): reports whether a
/// prepare-commit-msg hook on disk was installed by the Patchbay daemon (current
/// or any previously released version). False for hooks without any known
/// daemon signature, so a user-installed hook at the same path is left alone.
fn is_daemon_installed_hook(contents: &[u8]) -> bool {
    let body = String::from_utf8_lossy(contents);
    DAEMON_INSTALLED_HOOK_SIGNATURES
        .iter()
        .any(|sig| body.contains(sig))
}

/// `removeCoAuthoredByHookContext` (cache.go:1678–1704): removes the
/// prepare-commit-msg hook installed by installCoAuthoredByHook. Only deletes
/// the file when the content matches a known daemon signature, so a
/// user-installed prepare-commit-msg hook is never touched. Returns Ok when
/// no hook is present or an unrelated hook occupies the path.
async fn remove_co_authored_by_hook_ctx(ctx: &Ctx, worktree_path: &Path) -> anyhow::Result<()> {
    let common_dir = resolve_git_common_dir(ctx, worktree_path).await?;
    let hook_path = common_dir.join("hooks").join("prepare-commit-msg");
    let contents = match std::fs::read(&hook_path) {
        Ok(c) => c,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(anyhow::Error::new(err).context("read prepare-commit-msg hook")),
    };
    if !is_daemon_installed_hook(&contents) {
        // Unrelated hook (user or third-party): leave it alone.
        return Ok(());
    }
    match std::fs::remove_file(&hook_path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(anyhow::Error::new(err).context("remove prepare-commit-msg hook")),
    }
}

/// `excludeFromGitContext` (cache.go:1711–1743): adds a pattern to the
/// worktree's .git/info/exclude file.
async fn exclude_from_git_ctx(
    ctx: &Ctx,
    worktree_path: &Path,
    pattern: &str,
) -> anyhow::Result<()> {
    let out = run_git_output(
        ctx,
        &[
            "-C",
            &worktree_path.to_string_lossy(),
            "rev-parse",
            "--git-dir",
        ],
    )
    .await
    .context("resolve git dir")?;

    let git_dir = String::from_utf8_lossy(&out).trim().to_string();
    let git_dir = PathBuf::from(git_dir);
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        worktree_path.join(git_dir)
    };

    let exclude_path = git_dir.join("info").join("exclude");

    std::fs::create_dir_all(exclude_path.parent().expect("info dir has parent"))
        .context("create info dir")?;

    let existing = std::fs::read_to_string(&exclude_path).unwrap_or_default();
    if existing.contains(pattern) {
        return Ok(());
    }

    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&exclude_path)
        .context("open exclude file")?;
    write!(f, "\n{pattern}\n").context("write exclude pattern")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Pure helpers (cache.go lines 1745–1811).
// ---------------------------------------------------------------------------

/// `repoNameFromURL` (cache.go:1747–1766): extracts a short directory name
/// from a git remote URL, e.g. "https://github.com/org/my-repo.git" →
/// "my-repo".
pub(crate) fn repo_name_from_url(url: &str) -> String {
    let mut url = url.trim_end_matches('/');
    url = url.strip_suffix(".git").unwrap_or(url);

    if let Some(i) = url.rfind('/') {
        url = &url[i + 1..];
    }
    if let Some(i) = url.rfind(':') {
        url = &url[i + 1..];
        if let Some(j) = url.rfind('/') {
            url = &url[j + 1..];
        }
    }

    let name = url.trim();
    if name.is_empty() {
        return "repo".to_string();
    }
    name.to_string()
}

/// `sanitizeName` (cache.go:1771–1783): produces a git-branch-safe name from
/// a human-readable string.
pub(crate) fn sanitize_name(name: &str) -> String {
    static NON_ALPHANUMERIC: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re =
        NON_ALPHANUMERIC.get_or_init(|| regex::Regex::new(r"[^a-z0-9]+").expect("valid regex"));
    let s = name.trim().to_lowercase();
    let s = re.replace_all(&s, "-").to_string();
    let mut s = s.trim_matches('-').to_string();
    if s.len() > 30 {
        s.truncate(30);
        while s.ends_with('-') {
            s.pop();
        }
    }
    if s.is_empty() {
        s = "agent".to_string();
    }
    s
}

/// `taskKeyLen` (cache.go:1788): mirrors execenv.taskKeyLen — a branch name
/// becomes a path under .git/refs/heads/ inside the task checkout, and
/// Windows enforces MAX_PATH there.
const TASK_KEY_LEN: usize = 12;

/// `taskKey` (cache.go:1795–1801): returns the git-safe branch segment
/// identifying a task: the LAST taskKeyLen hex chars of the id. Mirrors
/// execenv.taskKey — a UUIDv7's LEADING 8 hex chars are timestamp bits that
/// only advance every ~65.5s, so taking the front gave two concurrently
/// created tasks the same branch name (#7326). The tail is random.
pub(crate) fn task_key(uuid: &str) -> String {
    let s: String = uuid.chars().filter(|c| *c != '-').collect();
    if s.len() > TASK_KEY_LEN {
        s[s.len() - TASK_KEY_LEN..].to_string()
    } else {
        s
    }
}

/// `shortID` (cache.go:1805–1811): returns the first 8 characters of a UUID
/// string (dashes stripped). Display and logging only — see task_key for
/// anything that must be unique.
pub(crate) fn short_id(uuid: &str) -> String {
    let s: String = uuid.chars().filter(|c| *c != '-').collect();
    if s.len() > 8 {
        s[..8].to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn foreground_guard_releases_repository_lock_on_drop() {
        let lock = Arc::new(RepoLock::new());
        lock.lock(&Ctx::new()).await.unwrap();
        {
            let _guard = ForegroundGuard {
                repo_lock: lock.clone(),
            };
        }

        tokio::time::timeout(Duration::from_millis(100), lock.lock(&Ctx::new()))
            .await
            .expect("lock should be available after guard drop")
            .unwrap();
        lock.unlock();
    }
}
