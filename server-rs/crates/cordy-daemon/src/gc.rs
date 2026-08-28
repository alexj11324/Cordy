//! Workspace and repository-cache garbage collection with process-tree-safe
//! command execution in [`processtree`].
//!
//! Deviations from Go:
//! - `*Daemon` receiver → [`GcHost`] trait; config fields live in
//!   [`GcConfig`] mirroring the exact Go field names/types from
//!   `internal/daemon/config.go:99–118`.
//! - execenv-owned metadata and managed-artifact helpers are reused directly;
//!   store pruning remains here because it is part of this production loop.
//! - `time.Ticker` → `tokio::time::interval` with `MissedTickBehavior::Delay`
//!   (Go tickers drop missed ticks).
//! - `context.Context` → [`Ctx`](crate::repocache::Ctx); slog → tracing with
//!   identical messages.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::Future;
use sha2::{Digest, Sha256};

use crate::activity::DaemonActivity;
use crate::artifact_matcher::{
    safe_relative_path, ArtifactMatcher, MANAGED_ARTIFACT_PATTERN_PREFIX,
};
use crate::execenv::execenv::{read_gc_meta, GCMetaKind, GcMeta};
use crate::repocache::{CancelCause, Ctx};

// ---------------------------------------------------------------------------
// processtree (inlined port of processtree/run.go + controller_unix.go).
// ---------------------------------------------------------------------------

/// Runs bounded helper commands
/// whose descendants must not survive cancellation. Uses a Unix process group
/// (`setpgid`) — unix only, matching the Go build tag.
#[cfg(unix)]
pub(crate) mod processtree {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Stdio;
    use std::time::Duration;

    use anyhow::Context as _;
    use tokio::io::AsyncReadExt;
    use tokio::process::Command;
    use tokio::select;

    use crate::repocache::{CancelCause, Ctx};

    /// `gracefulStopTimeout` (run.go:14).
    const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(1);
    /// `processTreeFinishTimeout` (controller_unix.go:14).
    const PROCESS_TREE_FINISH_TIMEOUT: Duration = Duration::from_secs(5);

    /// Exit-status error carrying the process exit code, standing in for Go's
    /// `exec.ExitError` (`Error()` renders "exit status N").
    #[derive(Debug, thiserror::Error)]
    pub(crate) enum ProcessError {
        #[error("exit status {0}")]
        Exit(i32),
        #[error("signal: {0}")]
        Signal(i32),
        #[error("{0}")]
        Cancelled(CancelCause),
        #[error(transparent)]
        Io(#[from] std::io::Error),
    }

    fn run_failure(
        combined: bool,
        combined_output: Vec<u8>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        err: anyhow::Error,
    ) -> (Vec<u8>, Vec<u8>, anyhow::Error) {
        if combined {
            (combined_output, Vec::new(), err)
        } else {
            (stdout, stderr, err)
        }
    }

    /// Signals a process group; Ok(false) means the group no longer exists
    /// (ESRCH), mirroring the controller's ESRCH tolerance.
    fn kill_group(pid: i32, sig: i32) -> std::io::Result<bool> {
        let rc = unsafe { libc::kill(-pid, sig) };
        if rc == 0 {
            return Ok(true);
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::ESRCH) => Ok(false),
            _ => Err(err),
        }
    }

    /// `controller.interrupt` (controller_unix.go:28–39): SIGTERM the group.
    fn interrupt(pid: i32) -> std::io::Result<()> {
        kill_group(pid, libc::SIGTERM).map(|_| ())
    }

    /// `controller.stop` (controller_unix.go:41–52): SIGKILL the group.
    fn stop(pid: i32) -> std::io::Result<()> {
        kill_group(pid, libc::SIGKILL).map(|_| ())
    }

    /// `controller.finish` (controller_unix.go:54–76): a normally-exited
    /// leader can still leave a descendant holding inherited pipes or
    /// repository locks. Kill the remaining group before returning ownership
    /// of the repository to another operation.
    async fn finish(pid: i32) -> anyhow::Result<()> {
        if !kill_group(pid, 0).unwrap_or(false) {
            return Ok(());
        }
        let _ = kill_group(pid, libc::SIGKILL);
        let deadline = tokio::time::Instant::now() + PROCESS_TREE_FINISH_TIMEOUT;
        while tokio::time::Instant::now() < deadline {
            if !kill_group(pid, 0).unwrap_or(false) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::bail!(
            "process group {} still active after {}s",
            pid,
            PROCESS_TREE_FINISH_TIMEOUT.as_secs()
        )
    }

    /// Core of `run` (run.go:44–92): spawns the command in its own process
    /// group, drains stdout/stderr concurrently, waits under cancellation,
    /// and tears down the tree on cancel (SIGTERM → 1s → SIGKILL → finish).
    /// Returns `(stdout, stderr)`; in combined mode both land in `.0`.
    async fn run_inner(
        ctx: &Ctx,
        mut cmd: Command,
        wait_delay: Duration,
        combined: bool,
    ) -> Result<(Vec<u8>, Vec<u8>), (Vec<u8>, Vec<u8>, anyhow::Error)> {
        if let Some(cause) = ctx.err() {
            return Err((Vec::new(), Vec::new(), anyhow::anyhow!(cause.to_string())));
        }
        // newController (controller_unix.go:18–24): own process group.
        cmd.process_group(0);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .context("start process")
            .map_err(|err| (Vec::new(), Vec::new(), err))?;
        let pid = child.id().unwrap_or_default() as i32;

        // Drain pipes concurrently into buffers, like Go's exec copying
        // goroutines writing into bytes.Buffer.
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        // The two pipes have different concrete types (ChildStdout /
        // ChildStderr); box them as trait objects so one drain task body
        // serves both, mirroring Go's uniform io.Copy goroutines.
        let pipes: Vec<Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>> = vec![
            stdout_pipe.map(|p| Box::new(p) as _),
            stderr_pipe.map(|p| Box::new(p) as _),
        ];
        let mut drain_tasks: Vec<tokio::task::JoinHandle<Vec<u8>>> = Vec::new();
        for pipe in pipes {
            let Some(pipe) = pipe else { continue };
            let sink = if combined { Some(shared.clone()) } else { None };
            drain_tasks.push(tokio::spawn(async move {
                let mut reader = pipe;
                let mut local = Vec::new();
                let mut chunk = [0u8; 8192];
                loop {
                    match reader.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if let Some(sink) = &sink {
                                sink.lock().unwrap().extend_from_slice(&chunk[..n]);
                            } else {
                                local.extend_from_slice(&chunk[..n]);
                            }
                        }
                    }
                }
                local
            }));
        }

        // select { case err = <-waitDone; case <-ctx.Done(): ... } (run.go:70–83)
        let waited = select! {
            r = child.wait() => Some(r),
            _ = ctx.cancelled() => None,
        };

        let cancelled = waited.is_none();
        let mut stop_err: Option<std::io::Error> = None;
        let status_result = match waited {
            Some(r) => Some(r),
            None => {
                if let Err(e) = interrupt(pid) {
                    stop_err = Some(e);
                }
                match tokio::time::timeout(GRACEFUL_STOP_TIMEOUT, child.wait()).await {
                    Ok(r) => Some(r),
                    Err(_elapsed) => {
                        if let Err(e) = stop(pid) {
                            let prev = stop_err.take();
                            stop_err = Some(match prev {
                                Some(prev) => std::io::Error::other(format!("{prev}; {e}")),
                                None => e,
                            });
                        }
                        Some(child.wait().await)
                    }
                }
            }
        };

        // Stop any descendants before waiting for pipe EOF: a helper may keep
        // an inherited pipe open after the leader exits. The bounded join is
        // the Rust equivalent of Go's Cmd.WaitDelay.
        let finish_result = finish(pid).await;
        let abort_handles: Vec<_> = drain_tasks.iter().map(|task| task.abort_handle()).collect();
        let drained =
            match tokio::time::timeout(wait_delay, futures_util::future::join_all(drain_tasks))
                .await
            {
                Ok(results) => Ok(results),
                Err(_) => {
                    for handle in abort_handles {
                        handle.abort();
                    }
                    Err(anyhow::anyhow!(
                        "wait for process output exceeded {}s",
                        wait_delay.as_secs()
                    ))
                }
            };

        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();
        for (i, result) in drained
            .as_ref()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let buf = result.as_ref().cloned().unwrap_or_default();
            if combined {
                continue; // already accumulated in `shared`
            }
            if i == 0 {
                stdout_buf = buf;
            } else {
                stderr_buf = buf;
            }
        }
        let combined_buf = std::mem::take(&mut *shared.lock().unwrap());

        // errors.Join(stopErr, finishErr) != nil → "stop process tree: %w"
        let lifecycle_err = match (stop_err, finish_result) {
            (None, Ok(())) => None,
            (Some(stop), Ok(())) => Some(anyhow::Error::new(stop)),
            (None, Err(finish)) => Some(finish),
            (Some(stop), Err(finish)) => Some(finish.context(format!("stop process tree: {stop}"))),
        };
        if let Some(e) = lifecycle_err {
            let err = if format!("{e:#}").contains("stop process tree") {
                e
            } else {
                e.context("stop process tree")
            };
            return Err(run_failure(
                combined,
                combined_buf,
                stdout_buf,
                stderr_buf,
                err,
            ));
        }
        if let Err(err) = drained {
            return Err(run_failure(
                combined,
                combined_buf,
                stdout_buf,
                stderr_buf,
                err,
            ));
        }
        if cancelled {
            return Err(run_failure(
                combined,
                combined_buf,
                stdout_buf,
                stderr_buf,
                anyhow::Error::new(ProcessError::Cancelled(ctx.cause())),
            ));
        }

        let status = match status_result {
            Some(Ok(status)) => status,
            Some(Err(e)) => {
                return Err(run_failure(
                    combined,
                    combined_buf,
                    stdout_buf,
                    stderr_buf,
                    anyhow::Error::new(ProcessError::Io(e)),
                ));
            }
            None => unreachable!("status_result is None only when cancelled"),
        };
        if !status.success() {
            let err = if let Some(sig) = status.signal() {
                anyhow::Error::new(ProcessError::Signal(sig))
            } else {
                anyhow::Error::new(ProcessError::Exit(status.code().unwrap_or(-1)))
            };
            return Err(run_failure(
                combined,
                combined_buf,
                stdout_buf,
                stderr_buf,
                err,
            ));
        }
        if combined {
            Ok((combined_buf, Vec::new()))
        } else {
            Ok((stdout_buf, stderr_buf))
        }
    }

    /// `CombinedOutput` (run.go:19–25): runs an unstarted command and returns
    /// its combined output. Cancellation terminates the entire process tree,
    /// waits for it to disappear, and returns the context cause rather than a
    /// platform-specific exit status.
    pub(crate) async fn combined_output(
        ctx: &Ctx,
        cmd: Command,
        wait_delay: Duration,
    ) -> (Vec<u8>, anyhow::Result<()>) {
        match run_inner(ctx, cmd, wait_delay, true).await {
            Ok((out, _)) => (out, Ok(())),
            Err((out, _, err)) => (out, Err(err)),
        }
    }

    /// `Output` (run.go:28–37): the process-tree-safe equivalent of
    /// exec.Cmd.Output.
    pub(crate) async fn output(
        ctx: &Ctx,
        cmd: Command,
        wait_delay: Duration,
    ) -> anyhow::Result<Vec<u8>> {
        match run_inner(ctx, cmd, wait_delay, false).await {
            Ok((out, _)) => Ok(out),
            Err((_, _, err)) => Err(err),
        }
    }

    /// `Run` (run.go:40–42): executes an unstarted command while owning its
    /// complete process tree.
    pub(crate) async fn run(ctx: &Ctx, cmd: Command, wait_delay: Duration) -> anyhow::Result<()> {
        match run_inner(ctx, cmd, wait_delay, true).await {
            Ok(_) => Ok(()),
            Err((_, _, err)) => Err(err),
        }
    }
}

#[cfg(windows)]
pub(crate) mod processtree {
    use std::process::Stdio;
    use std::time::Duration;

    use tokio::io::AsyncReadExt;
    use tokio::process::Command;

    use crate::repocache::{CancelCause, Ctx};

    const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(1);
    const PROCESS_TREE_FINISH_TIMEOUT: Duration = Duration::from_secs(5);

    #[derive(Debug, thiserror::Error)]
    pub(crate) enum ProcessError {
        #[error("exit status {0}")]
        Exit(i32),
        #[error("{0}")]
        Cancelled(CancelCause),
        #[error(transparent)]
        Io(#[from] std::io::Error),
    }

    /// Windows counterpart to the Unix process-group runner. `OwnedProcessTree`
    /// starts the helper suspended, assigns its complete descendant tree to a
    /// kill-on-close Job Object, and only then lets it run.
    async fn run_inner(
        ctx: &Ctx,
        mut cmd: Command,
        wait_delay: Duration,
    ) -> (Vec<u8>, Vec<u8>, anyhow::Result<()>) {
        if let Some(cause) = ctx.err() {
            return (
                Vec::new(),
                Vec::new(),
                Err(anyhow::Error::new(ProcessError::Cancelled(cause))),
            );
        }
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut tree = match cordy_agent::OwnedProcessTree::spawn(&mut cmd).await {
            Ok(tree) => tree,
            Err(error) => {
                return (
                    Vec::new(),
                    Vec::new(),
                    Err(anyhow::Error::new(ProcessError::Io(error))),
                )
            }
        };
        let stdout = tree.child_mut().stdout.take();
        let stderr = tree.child_mut().stderr.take();
        let drain = |pipe: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>| {
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.read_to_end(&mut bytes).await;
                }
                bytes
            })
        };
        let stdout_task = drain(stdout.map(|pipe| Box::new(pipe) as _));
        let stderr_task = drain(stderr.map(|pipe| Box::new(pipe) as _));

        let status = tokio::select! {
            result = tree.wait() => Some(result),
            _ = ctx.cancelled() => None,
        };
        let result = match status {
            None => {
                let _ = tree
                    .shutdown(GRACEFUL_STOP_TIMEOUT, PROCESS_TREE_FINISH_TIMEOUT)
                    .await;
                Err(anyhow::Error::new(ProcessError::Cancelled(ctx.cause())))
            }
            Some(Err(error)) => Err(anyhow::Error::new(ProcessError::Io(error))),
            Some(Ok(status)) => {
                let _ = tree.kill();
                if !tree.wait_tree_gone(PROCESS_TREE_FINISH_TIMEOUT).await {
                    Err(anyhow::anyhow!("process job still active after 5s"))
                } else if status.success() {
                    Ok(())
                } else {
                    Err(anyhow::Error::new(ProcessError::Exit(
                        status.code().unwrap_or(-1),
                    )))
                }
            }
        };

        let drained = tokio::time::timeout(wait_delay, async {
            (
                stdout_task.await.unwrap_or_default(),
                stderr_task.await.unwrap_or_default(),
            )
        })
        .await;
        let (stdout, stderr) = match drained {
            Ok(output) => output,
            Err(_) => {
                if result.is_err() {
                    return (Vec::new(), Vec::new(), result);
                }
                return (
                    Vec::new(),
                    Vec::new(),
                    Err(anyhow::anyhow!(
                        "wait for process output exceeded {}s",
                        wait_delay.as_secs()
                    )),
                );
            }
        };
        (stdout, stderr, result)
    }

    pub(crate) async fn combined_output(
        ctx: &Ctx,
        cmd: Command,
        wait_delay: Duration,
    ) -> (Vec<u8>, anyhow::Result<()>) {
        let (mut stdout, stderr, result) = run_inner(ctx, cmd, wait_delay).await;
        stdout.extend(stderr);
        (stdout, result)
    }

    pub(crate) async fn output(
        ctx: &Ctx,
        cmd: Command,
        wait_delay: Duration,
    ) -> anyhow::Result<Vec<u8>> {
        let (stdout, _stderr, result) = run_inner(ctx, cmd, wait_delay).await;
        result.map(|()| stdout)
    }

    pub(crate) async fn run(ctx: &Ctx, cmd: Command, wait_delay: Duration) -> anyhow::Result<()> {
        let (_stdout, _stderr, result) = run_inner(ctx, cmd, wait_delay).await;
        result
    }
}

// ---------------------------------------------------------------------------
// gc.go lines 18–21: reposDirName.
// ---------------------------------------------------------------------------

/// `reposDirName` (gc.go:21): the bare-repo cache directory inside the
/// workspaces root. It is a sibling of the per-workspace task directories
/// rather than one of them, so every walk over the root has to decide
/// explicitly what to do with it.
pub(crate) const REPOS_DIR_NAME: &str = ".repos";

// S9-integration: mirrors daemon client requestError + isAccessNotFound
// (gc.go:472–475). The real client lives behind GcHost.

/// Stand-in for the daemon client's `requestError`.
#[derive(Debug, thiserror::Error)]
#[error("request failed with status {status_code}")]
pub(crate) struct RequestError {
    pub(crate) status_code: u16,
}

/// `isAccessNotFound` (gc.go:472–475): detects the 404 returned by gc-check
/// endpoints. The same status covers "row deleted" and "daemon token can't
/// see this workspace", so callers can't tell the two apart from the response
/// alone.
fn is_access_not_found(err: &anyhow::Error) -> bool {
    err.chain().any(|c| {
        c.downcast_ref::<RequestError>()
            .is_some_and(|e| e.status_code == 404)
    })
}

/// `IssueGCCheckResult` (daemon client): outcome of one issue GC check.
#[derive(Debug, Clone, Default)]
pub(crate) struct IssueGCCheckResult {
    pub(crate) found: bool,
    pub(crate) status: String,
    pub(crate) updated_at: Option<DateTime<Utc>>,
    /// Go stores `err error`; we keep the rendered message (Clone needed for
    /// the batch-check map fan-out).
    pub(crate) err: Option<String>,
}

/// Per-issue single check payload used by `gcDecisionIssue`
/// (gc.go:494–499).
#[derive(Debug, Clone, Default)]
pub(crate) struct IssueGCCheckStatus {
    pub(crate) status: String,
    pub(crate) updated_at: Option<DateTime<Utc>>,
}

// S9-integration: mirrors execenv.ManagedReclaimableArtifactSubpaths and the
// hasManagedArtifact probe (codex-home/.sandbox-bin under the env root).

/// `execenv.ManagedReclaimableArtifactSubpaths`: labels logged at GC startup.
fn managed_reclaimable_artifact_subpaths() -> Vec<String> {
    crate::execenv::reclaimable::managed_reclaimable_artifact_subpaths()
}

// S9-integration: mirrors execenv.PruneCodexSessionStores /
// PruneHermesMemoryStores / PruneHermesSessionStores. Each returns
// (storesRemoved, bytesReclaimed); the real implementations live in execenv
// and receive `reserve_store_for_deletion` as a reservation callback.

/// Reservation callback handed to store pruners (`d.reserveStoreForDeletion`).
pub(crate) type ReserveStoreForDeletion<'a> =
    &'a (dyn Fn(&Path) -> Option<crate::activity::StoreGcReservation> + Send + Sync);

fn prune_codex_session_stores(
    profile: &str,
    ttl: Duration,
    now: DateTime<Utc>,
    reserve: ReserveStoreForDeletion<'_>,
) -> (usize, i64) {
    let namespace = if profile.is_empty() {
        "default".to_string()
    } else {
        format!("p_{}", hex::encode(Sha256::digest(profile.as_bytes())))
    };
    let Some(root) = shared_codex_home().map(|home| home.join("cordy-sessions").join(namespace))
    else {
        return (0, 0);
    };
    prune_store_tree(&root, 2, ttl, now, reserve)
}

fn prune_hermes_memory_stores(
    profile: &str,
    ttl: Duration,
    now: DateTime<Utc>,
    reserve: ReserveStoreForDeletion<'_>,
) -> (usize, i64) {
    let Some(root) = profile_dir(profile).map(|dir| dir.join("hermes-state")) else {
        return (0, 0);
    };
    prune_store_tree(&root, 2, ttl, now, reserve)
}

fn prune_hermes_session_stores(
    profile: &str,
    ttl: Duration,
    now: DateTime<Utc>,
    reserve: ReserveStoreForDeletion<'_>,
) -> (usize, i64) {
    let Some(root) = profile_dir(profile).map(|dir| dir.join("hermes-sessions")) else {
        return (0, 0);
    };
    prune_store_tree(&root, 3, ttl, now, reserve)
}

fn profile_dir(profile: &str) -> Option<PathBuf> {
    if profile.contains(['/', '\\']) || profile == "." || profile == ".." {
        return None;
    }
    if let Some(root) = std::env::var_os("CORDY_TASK_CONFIG_ROOT").filter(|v| !v.is_empty()) {
        let root = PathBuf::from(root);
        if !root.is_absolute() {
            return None;
        }
        return Some(if profile.is_empty() {
            root
        } else {
            root.join("profiles").join(profile)
        });
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(if profile.is_empty() {
        home.join(".cordy")
    } else {
        home.join(".cordy").join("profiles").join(profile)
    })
}

fn shared_codex_home() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(raw);
        return if path.is_absolute() {
            Some(path)
        } else {
            std::env::current_dir().ok().map(|cwd| cwd.join(path))
        };
    }
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".codex"))
}

fn prune_store_tree(
    root: &Path,
    leaf_depth: usize,
    ttl: Duration,
    now: DateTime<Utc>,
    reserve: ReserveStoreForDeletion<'_>,
) -> (usize, i64) {
    if ttl.is_zero() {
        return (0, 0);
    }
    let Ok(retention) = chrono::Duration::from_std(ttl) else {
        return (0, 0);
    };
    let Some(cutoff) = now.checked_sub_signed(retention) else {
        return (0, 0);
    };
    let mut level = vec![root.to_path_buf()];
    for _ in 0..leaf_depth {
        let mut next = Vec::new();
        for parent in level {
            let Ok(entries) = std::fs::read_dir(parent) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                    next.push(entry.path());
                }
            }
        }
        level = next;
    }

    let mut removed = 0usize;
    let mut bytes_freed = 0i64;
    for store in level {
        let (newest, size) = store_stat(&store);
        let Some(newest) = newest else { continue };
        let newest: DateTime<Utc> = newest.into();
        if newest >= cutoff {
            continue;
        }
        let Some(_reservation) = reserve(&store) else {
            continue;
        };
        match std::fs::remove_dir_all(&store) {
            Ok(()) => {
                removed += 1;
                bytes_freed += size;
                remove_empty_ancestors(store.parent(), root);
            }
            Err(err) => {
                tracing::warn!(store = %store.display(), error = %err, "gc: prune shared store failed")
            }
        }
    }
    (removed, bytes_freed)
}

fn store_stat(root: &Path) -> (Option<std::time::SystemTime>, i64) {
    let mut newest = None;
    let mut size = 0i64;
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let Ok(entry) = entry else { continue };
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_file() {
            size = size.saturating_add(i64::try_from(metadata.len()).unwrap_or(i64::MAX));
        }
        if let Ok(modified) = metadata.modified() {
            newest = Some(newest.map_or(modified, |current| std::cmp::max(current, modified)));
        }
    }
    (newest, size)
}

fn remove_empty_ancestors(mut current: Option<&Path>, root: &Path) {
    while let Some(dir) = current {
        if dir == root || !dir.starts_with(root) || std::fs::remove_dir(dir).is_err() {
            break;
        }
        current = dir.parent();
    }
}

// ---------------------------------------------------------------------------
// GcConfig (mirrors internal/daemon/config.go:99–118 field names/types).
// ---------------------------------------------------------------------------

/// The subset of `Daemon.Config` consumed by the GC loop, with exact Go field
/// names preserved.
#[derive(Debug, Clone)]
pub(crate) struct GcConfig {
    /// profile name (empty = default)
    pub profile: String,
    /// base path for execution envs (default: ~/cordy_workspaces)
    pub workspaces_root: PathBuf,
    /// enable periodic workspace garbage collection (default: true)
    pub gc_enabled: bool,
    /// how often the GC loop runs (default: 2h)
    pub gc_interval: Duration,
    /// clean dirs whose issue is done/cancelled and updated_at < now()-TTL
    /// (default: 24h)
    pub gc_ttl: Duration,
    /// fully clean inactive issue-task envs completed at least this long ago,
    /// regardless of parent issue status (default: 14d on Cordy Cloud,
    /// 0/disabled elsewhere; local_directory envs are never fully removed)
    pub gc_completed_task_ttl: Duration,
    /// clean orphan dirs with no meta, or dirs whose issue gc-check returns
    /// 404, once they exceed this age (default: 72h)
    pub gc_orphan_ttl: Duration,
    /// once a task has been completed for at least this long, drop
    /// regenerable artifacts (default: 12h, set 0 to disable both)
    pub gc_artifact_ttl: Duration,
    /// reclaim per-issue Codex session stores untouched this long (14d)
    pub gc_codex_session_ttl: Duration,
    /// reclaim per-agent Hermes memory stores untouched this long (90d)
    pub gc_hermes_memory_ttl: Duration,
    /// reclaim per-conversation Hermes session stores untouched this long (14d)
    pub gc_hermes_session_ttl: Duration,
    /// evict a bare repo cache no task has checked out this long (30d)
    pub gc_repo_ttl: Duration,
    /// prune stale worktree refs + evict repo caches during GC cycles
    pub gc_repo_maintenance_enabled: bool,
    /// basename matches treated as regenerable build outputs
    pub gc_artifact_patterns: Vec<String>,
}

// ---------------------------------------------------------------------------
// GcHost: the *Daemon surface gc.go touches (trait seam).
// ---------------------------------------------------------------------------

/// Host operations the GC loop needs from the daemon. Integration wires this
/// to the Daemon struct; unit tests supply fakes.
pub(crate) trait GcHost: Send + Sync {
    fn config(&self) -> &GcConfig;

    /// `d.client.GetIssueGCCheck` (gc.go:482).
    fn get_issue_gc_check(
        &self,
        ctx: &Ctx,
        issue_id: &str,
    ) -> impl Future<Output = anyhow::Result<IssueGCCheckStatus>> + Send;

    /// `d.client.GetIssueGCChecks` (gc.go:228).
    fn get_issue_gc_checks(
        &self,
        ctx: &Ctx,
        workspace_id: &str,
        issue_ids: &[String],
    ) -> impl Future<Output = anyhow::Result<HashMap<String, IssueGCCheckResult>>> + Send;

    /// `d.client.GetChatSessionGCCheck` (gc.go:611).
    fn get_chat_session_gc_check(
        &self,
        ctx: &Ctx,
        chat_session_id: &str,
    ) -> impl Future<Output = anyhow::Result<IssueGCCheckStatus>> + Send;

    /// `d.client.GetAutopilotRunGCCheck` (gc.go:669).
    fn get_autopilot_run_gc_check(
        &self,
        ctx: &Ctx,
        autopilot_run_id: &str,
    ) -> impl Future<Output = anyhow::Result<IssueGCCheckStatus>> + Send;

    /// `d.client.GetTaskGCCheck` (gc.go:724).
    fn get_task_gc_check(
        &self,
        ctx: &Ctx,
        task_id: &str,
    ) -> impl Future<Output = anyhow::Result<IssueGCCheckStatus>> + Send;

    /// Shared task/root state used for active checks and atomic GC deletion
    /// reservations.
    fn activity(&self) -> &Arc<DaemonActivity>;

    /// `d.repoBarePathIsLive`: whether any watched workspace still claims
    /// this bare repo path (gc.go:1141/1183).
    fn repo_bare_path_is_live(&self, bare_path: &Path) -> bool;

    /// Repo-cache gate for maintenance (`d.repoCache`, gc.go:1075/1095);
    /// None mirrors a daemon built without a repo cache.
    fn repo_cache_for_gc(&self) -> Option<&crate::repocache::Cache>;
}

// ---------------------------------------------------------------------------
// gcStats / gcAction (gc.go lines 61–77, 318–326).
// ---------------------------------------------------------------------------

/// `gcStats` (gc.go:62–77): accumulates byte counts and per-pattern hit
/// counts for one GC cycle.
#[derive(Debug, Default)]
pub(crate) struct GcStats {
    /// whole task dirs removed by a parent-lifecycle or completed-task policy
    cleaned: i32,
    /// whole task dirs removed (no meta / unreachable issue)
    orphaned: i32,
    /// task dirs left untouched
    skipped: i32,
    /// task dirs that had at least one artifact reclaimed
    artifact_dirs: i32,
    /// count of removed artifact subdirs
    artifact_removed: i32,
    /// per-conversation Codex session stores reclaimed past their TTL
    stores_reclaimed: i32,
    /// counted separately from stores_reclaimed: the two stores hold
    /// different things on different TTLs, so folding them into one number
    /// would make either figure unreadable for an operator.
    hermes_memory_stores_reclaimed: i32,
    /// per-conversation Hermes session stores reclaimed past their TTL
    hermes_session_stores_reclaimed: i32,
    /// bare repo caches under .repos evicted past their TTL
    repo_caches_reclaimed: i32,
    /// total bytes freed in this cycle
    bytes_reclaimed: i64,
    /// configured basename or managed path label -> reclaim count
    by_pattern: HashMap<String, i32>,
}

impl GcStats {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

/// `gcAction` (gc.go:318–326).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GcAction {
    Skip,
    /// a parent-lifecycle or completed-task policy selected full cleanup
    Clean,
    /// no meta or unknown issue and dir is old
    Orphan,
    /// task completed long enough ago; drop regenerable artifacts only
    CleanArtifacts,
    /// preserve the task and drop exact daemon-managed artifacts only
    CleanManagedArtifacts,
}

// ---------------------------------------------------------------------------
// gcLoop / runGC (gc.go lines 23–152).
// ---------------------------------------------------------------------------

/// `sleepWithContext` equivalent: resolves Err(cause) on cancellation.
async fn sleep_with_ctx(ctx: &Ctx, dur: Duration) -> Result<(), CancelCause> {
    tokio::select! {
        _ = ctx.cancelled() => Err(ctx.cause()),
        _ = tokio::time::sleep(dur) => Ok(()),
    }
}

/// `gcLoop` (gc.go:25–59): periodically scans local workspace directories and
/// applies the configured retention policies.
pub(crate) async fn gc_loop<H: GcHost>(host: &H, ctx: &Ctx) {
    let cfg = host.config();
    if !cfg.gc_enabled {
        tracing::info!("gc: disabled");
        // This is still an owned production loop. Remain attached to the
        // daemon root so the supervisor cannot mistake a supported disabled
        // configuration for an unexpected owner exit.
        ctx.cancelled().await;
        return;
    }
    tracing::info!(
        interval = go_duration(cfg.gc_interval),
        ttl = go_duration(cfg.gc_ttl),
        completed_task_ttl = go_duration(cfg.gc_completed_task_ttl),
        orphan_ttl = go_duration(cfg.gc_orphan_ttl),
        artifact_ttl = go_duration(cfg.gc_artifact_ttl),
        repo_ttl = go_duration(cfg.gc_repo_ttl),
        repo_maintenance_enabled = cfg.gc_repo_maintenance_enabled,
        artifact_patterns = ?cfg.gc_artifact_patterns,
        managed_artifact_subpaths = ?managed_reclaimable_artifact_subpaths(),
        "gc: started"
    );

    // Run once at startup after a short delay (let the daemon finish
    // initializing).
    if sleep_with_ctx(ctx, Duration::from_secs(30)).await.is_err() {
        return;
    }
    run_gc(host, ctx).await;

    // time.NewTicker fires after a full interval; consume tokio's immediate
    // first tick so behaviour matches.
    let mut ticker = tokio::time::interval(cfg.gc_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await;

    loop {
        tokio::select! {
            _ = ctx.cancelled() => return,
            _ = ticker.tick() => run_gc(host, ctx).await,
        }
    }
}

/// Formats a Duration like Go's time.Duration.String for whole-second values
/// ("2h0m0s", "30m0s", "45s"), used in startup log fields.
fn go_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    let mut out = String::new();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if m > 0 || h > 0 {
        out.push_str(&format!("{m}m"));
    }
    out.push_str(&format!("{s}s"));
    out
}

/// `runGC` (gc.go:80–152): performs a single GC scan across all workspace
/// directories.
pub(crate) async fn run_gc<H: GcHost>(host: &H, ctx: &Ctx) {
    let cfg = host.config();
    let root = &cfg.workspaces_root;
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => {
            tracing::warn!(error = %err, "gc: read workspaces root failed");
            return;
        }
    };

    let mut stats = GcStats::new();
    let mut ws_entries: Vec<_> = entries.flatten().collect();
    ws_entries.sort_by_key(|entry| entry.path());
    for ws_entry in ws_entries {
        let file_type = match ws_entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        let ws_entry = ws_entry.path();
        // Skip every daemon-internal dot directory, not just .repos. A
        // workspace directory is always a UUID, so a dot-prefixed entry is one
        // of our own caches. Walking .skill-cache as if it were a workspace
        // made its `v1` directory look like a task dir with no .gc_meta.json,
        // so the orphan path would delete the entire bundle cache once its
        // mtime went 72h without a new bundle.
        let name = ws_entry
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if !file_type.is_dir() || name.starts_with('.') {
            continue;
        }
        gc_workspace(host, ctx, &ws_entry, &mut stats).await;
    }

    // Prune stale worktree references from all bare repo caches, then evict
    // the caches nothing needs anymore. These live outside any workspace
    // directory and are never reclaimed by the task walk above.
    prune_repo_worktrees_ctx(host, ctx, root, &mut stats).await;

    // Reclaim per-issue Codex session stores idle past their TTL (MUL-4424).
    let now = Utc::now();
    let reserve = |p: &Path| host.activity().reserve_store_for_gc(p);
    let (stores_removed, store_bytes) =
        prune_codex_session_stores(&cfg.profile, cfg.gc_codex_session_ttl, now, &reserve);
    if stores_removed > 0 {
        stats.stores_reclaimed += stores_removed as i32;
        stats.bytes_reclaimed += store_bytes;
    }

    // Same for per-agent Hermes memory stores (#6638).
    let (stores_removed, store_bytes) =
        prune_hermes_memory_stores(&cfg.profile, cfg.gc_hermes_memory_ttl, now, &reserve);
    if stores_removed > 0 {
        stats.hermes_memory_stores_reclaimed += stores_removed as i32;
        stats.bytes_reclaimed += store_bytes;
    }

    // And per-conversation Hermes session stores (#6806).
    let (stores_removed, store_bytes) =
        prune_hermes_session_stores(&cfg.profile, cfg.gc_hermes_session_ttl, now, &reserve);
    if stores_removed > 0 {
        stats.hermes_session_stores_reclaimed += stores_removed as i32;
        stats.bytes_reclaimed += store_bytes;
    }

    if stats.cleaned > 0
        || stats.orphaned > 0
        || stats.artifact_dirs > 0
        || stats.stores_reclaimed > 0
        || stats.hermes_memory_stores_reclaimed > 0
        || stats.hermes_session_stores_reclaimed > 0
        || stats.repo_caches_reclaimed > 0
    {
        tracing::info!(
            cleaned = stats.cleaned,
            orphaned = stats.orphaned,
            skipped = stats.skipped,
            artifact_dirs = stats.artifact_dirs,
            artifact_removed = stats.artifact_removed,
            codex_session_stores_reclaimed = stats.stores_reclaimed,
            hermes_memory_stores_reclaimed = stats.hermes_memory_stores_reclaimed,
            hermes_session_stores_reclaimed = stats.hermes_session_stores_reclaimed,
            repo_caches_reclaimed = stats.repo_caches_reclaimed,
            bytes_reclaimed = stats.bytes_reclaimed,
            by_pattern = ?stats.by_pattern,
            "gc: cycle complete"
        );
    }
}

// ---------------------------------------------------------------------------
// gcWorkspace / issue batching (gc.go lines 154–264).
// ---------------------------------------------------------------------------

/// `issueGCBatchSize` (gc.go:195).
const ISSUE_GC_BATCH_SIZE: usize = 500;

/// `issueGCCandidate` (gc.go:197–200).
struct IssueGcCandidate {
    task_dir: PathBuf,
    meta: GcMeta,
}

/// `gcWorkspace` (gc.go:155–193): scans task directories inside a single
/// workspace directory.
async fn gc_workspace<H: GcHost>(host: &H, ctx: &Ctx, ws_dir: &Path, stats: &mut GcStats) {
    let task_entries = match std::fs::read_dir(ws_dir) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(dir = %ws_dir.display(), error = %err, "gc: read workspace dir failed");
            return;
        }
    };

    let mut cleaned_here = 0i32;
    let mut issue_candidates: Vec<IssueGcCandidate> = Vec::with_capacity(task_entries.count());
    let mut task_entries = std::fs::read_dir(ws_dir).ok();
    while let Some(entry) = task_entries.as_mut().and_then(|rd| rd.next()) {
        let Ok(entry) = entry else { continue };
        if ctx.err().is_some() {
            return;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        let task_dir = ws_dir.join(entry.file_name());
        if host.activity().is_active_env_root(&task_dir) {
            stats.skipped += 1;
            continue;
        }
        match read_gc_meta(&task_dir) {
            Ok(meta)
                if meta.kind.as_ref() == Some(&GCMetaKind::Issue)
                    && !meta.issue_id.trim().is_empty() =>
            {
                issue_candidates.push(IssueGcCandidate { task_dir, meta });
                continue;
            }
            _ => {}
        }
        let action = should_clean_task_dir(host, ctx, &task_dir).await;
        cleaned_here += apply_gc_action(host, &task_dir, action, stats);
    }
    let workspace_id = ws_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    cleaned_here += gc_workspace_issues(host, ctx, &workspace_id, issue_candidates, stats).await;

    // Remove the workspace directory itself if it's now empty.
    if cleaned_here > 0 {
        let remaining = std::fs::read_dir(ws_dir).map(|rd| rd.count()).unwrap_or(1);
        if remaining == 0 {
            let _ = std::fs::remove_dir(ws_dir);
        }
    }
}

/// `gcWorkspaceIssues` (gc.go:206–264): resolves all issue-backed task dirs
/// with a bounded number of workspace-level requests. Multiple task dirs for
/// the same issue share one result.
async fn gc_workspace_issues<H: GcHost>(
    host: &H,
    ctx: &Ctx,
    workspace_id: &str,
    candidates: Vec<IssueGcCandidate>,
    stats: &mut GcStats,
) -> i32 {
    if candidates.is_empty() {
        return 0;
    }

    let mut issue_ids: Vec<String> = Vec::with_capacity(candidates.len());
    let mut seen = std::collections::HashSet::new();
    for candidate in &candidates {
        let issue_id = candidate.meta.issue_id.trim().to_string();
        if !seen.insert(issue_id.clone()) {
            continue;
        }
        issue_ids.push(issue_id);
    }

    let mut results: HashMap<String, IssueGCCheckResult> = HashMap::with_capacity(issue_ids.len());
    for start in (0..issue_ids.len()).step_by(ISSUE_GC_BATCH_SIZE) {
        if ctx.err().is_some() {
            break;
        }
        let end = usize::min(start + ISSUE_GC_BATCH_SIZE, issue_ids.len());
        match host
            .get_issue_gc_checks(ctx, workspace_id, &issue_ids[start..end])
            .await
        {
            Ok(chunk_results) => results.extend(chunk_results),
            Err(err) => {
                tracing::warn!(
                    workspace = %workspace_id,
                    count = end - start,
                    error = %err,
                    "gc: batch issue check failed"
                );
                continue;
            }
        }
    }

    let mut cleaned = 0i32;
    let total_candidates = candidates.len() as i32;
    for (i, candidate) in candidates.into_iter().enumerate() {
        if ctx.err().is_some() {
            stats.skipped += total_candidates - i as i32;
            break;
        }
        let issue_id = candidate.meta.issue_id.trim().to_string();
        let Some(result) = results.get(&issue_id) else {
            // No usable answer about the parent issue this cycle, so the task
            // data stays. The regenerable Codex cache is still fair game.
            let action = apply_managed_artifact_fallback(
                host,
                &candidate.task_dir,
                &candidate.meta,
                GcAction::Skip,
            );
            cleaned += apply_gc_action(host, &candidate.task_dir, action, stats);
            continue;
        };
        if result.err.is_some() {
            let action = apply_managed_artifact_fallback(
                host,
                &candidate.task_dir,
                &candidate.meta,
                GcAction::Skip,
            );
            cleaned += apply_gc_action(host, &candidate.task_dir, action, stats);
            continue;
        }
        let result = result.clone();
        let mut action =
            gc_decision_issue_result(host, &candidate.task_dir, &candidate.meta, result);
        action = apply_local_directory_gc_override(host, &candidate.meta, action);
        action =
            apply_managed_artifact_fallback(host, &candidate.task_dir, &candidate.meta, action);
        cleaned += apply_gc_action(host, &candidate.task_dir, action, stats);
    }
    cleaned
}

// ---------------------------------------------------------------------------
// Decision plumbing (gc.go lines 266–350).
// ---------------------------------------------------------------------------

/// `applyGCAction` (gc.go:269–301): performs one decision and updates cycle
/// stats. Each mutation atomically reserves the env root because a task can
/// start while the server reconciliation request is in flight.
fn apply_gc_action<H: GcHost>(
    host: &H,
    task_dir: &Path,
    action: GcAction,
    stats: &mut GcStats,
) -> i32 {
    let _release = if action != GcAction::Skip {
        match host.activity().reserve_env_root_for_gc(task_dir) {
            Some(release) => Some(release),
            None => {
                stats.skipped += 1;
                return 0;
            }
        }
    } else {
        None
    };
    match action {
        GcAction::Clean => {
            let bytes = clean_task_dir(task_dir);
            stats.cleaned += 1;
            stats.bytes_reclaimed += bytes;
            1
        }
        GcAction::Orphan => {
            let bytes = clean_task_dir(task_dir);
            stats.orphaned += 1;
            stats.bytes_reclaimed += bytes;
            1
        }
        GcAction::CleanArtifacts => {
            let cfg = host.config();
            let (removed, bytes, per_pattern) =
                clean_task_artifacts(task_dir, &cfg.gc_artifact_patterns);
            record_artifact_cleanup(stats, removed, bytes, per_pattern);
            stats.skipped += 1; // task dir itself preserved
            0
        }
        GcAction::CleanManagedArtifacts => {
            let (removed, bytes, per_pattern) = clean_managed_task_artifacts(task_dir);
            record_artifact_cleanup(stats, removed, bytes, per_pattern);
            stats.skipped += 1; // task dir itself preserved
            0
        }
        GcAction::Skip => {
            stats.skipped += 1;
            0
        }
    }
}

/// `recordArtifactCleanup` (gc.go:303–316).
fn record_artifact_cleanup(
    stats: &mut GcStats,
    removed: i32,
    bytes: i64,
    per_pattern: HashMap<String, i32>,
) {
    if removed == 0 {
        return;
    }
    stats.artifact_dirs += 1;
    stats.artifact_removed += removed;
    stats.bytes_reclaimed += bytes;
    for (pattern, count) in per_pattern {
        *stats.by_pattern.entry(pattern).or_insert(0) += count;
    }
}

/// `shouldCleanTaskDir` (gc.go:331–350): decides whether a task directory
/// should be removed. Dispatches on meta.Kind so chat / autopilot /
/// quick-create tasks each follow the parent record that actually governs
/// their lifecycle.
async fn should_clean_task_dir<H: GcHost>(host: &H, ctx: &Ctx, task_dir: &Path) -> GcAction {
    // A task currently running on this env root must never be reclaimed —
    // not even on the done/cancelled or orphan-404 paths.
    if host.activity().is_active_env_root(task_dir) {
        return GcAction::Skip;
    }

    let meta = match read_gc_meta(task_dir) {
        Ok(meta) => meta,
        Err(_) => return orphan_by_mtime(host, task_dir, "no meta"),
    };

    let action = should_clean_task_dir_for_kind(host, ctx, task_dir, &meta).await;
    let action = apply_local_directory_gc_override(host, &meta, action);
    apply_managed_artifact_fallback(host, task_dir, &meta, action)
}

/// `applyManagedArtifactFallback` (gc.go:375–398): upgrades a skip into
/// managed-artifact cleanup once the task's own regenerable Codex cache is
/// past GCArtifactTTL. Deliberately a one-way upgrade from Skip.
fn apply_managed_artifact_fallback<H: GcHost>(
    host: &H,
    task_dir: &Path,
    meta: &GcMeta,
    action: GcAction,
) -> GcAction {
    if action != GcAction::Skip || host.config().gc_artifact_ttl.is_zero() {
        return action;
    }
    // A zero CompletedAt means the task never reported completion through
    // WriteGCMeta. Leave those to the per-kind legacy handling rather than
    // guessing from an unrelated clock.
    let Some(completed_at) = meta.completed_at else {
        return action;
    };
    let age = Utc::now().signed_duration_since(completed_at);
    if age <= chrono::Duration::from_std(host.config().gc_artifact_ttl).unwrap_or_default() {
        return action;
    }
    // completed_at never moves again for a task that stays non-terminal, so
    // without this the decision stays "reclaim" forever and every cycle pays
    // for a reservation and a removal pass that finds nothing.
    if !has_managed_artifact(task_dir) {
        return action;
    }
    tracing::info!(
        dir = %task_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        kind = %meta.kind.as_ref().map(GCMetaKind::as_str).unwrap_or(""),
        completed_at = %completed_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "gc: eligible for managed artifact cleanup"
    );
    GcAction::CleanManagedArtifacts
}

/// `applyLocalDirectoryGCOverride` (gc.go:400–431): local_directory tasks keep
/// their envRoot indefinitely so the user can inspect output/ and logs/ for
/// forensic context. Clean demotes to artifact-pattern cleanup; Orphan demotes
/// to exact managed-artifact cleanup only.
fn apply_local_directory_gc_override<H: GcHost>(
    host: &H,
    meta: &GcMeta,
    action: GcAction,
) -> GcAction {
    if !meta.local_directory {
        return action;
    }
    if host.config().gc_artifact_ttl.is_zero() {
        return GcAction::Skip;
    }
    match action {
        GcAction::Clean => GcAction::CleanArtifacts,
        GcAction::Orphan => GcAction::CleanManagedArtifacts,
        other => other,
    }
}

/// `shouldCleanTaskDirForKind` (gc.go:436–451): runs the per-Kind dispatch
/// without applying the local_directory override.
async fn should_clean_task_dir_for_kind<H: GcHost>(
    host: &H,
    ctx: &Ctx,
    task_dir: &Path,
    meta: &GcMeta,
) -> GcAction {
    match meta.kind.as_ref() {
        Some(GCMetaKind::Issue) => gc_decision_issue(host, ctx, task_dir, meta).await,
        Some(GCMetaKind::Chat) => gc_decision_chat(host, ctx, task_dir, meta).await,
        Some(GCMetaKind::AutopilotRun) => {
            gc_decision_autopilot_run(host, ctx, task_dir, meta).await
        }
        Some(GCMetaKind::QuickCreate) => gc_decision_quick_create(host, ctx, task_dir, meta).await,
        // Unknown or absent kind: fall back to mtime-based
        // orphan cleanup so a future daemon writing a kind we don't recognize
        // doesn't get insta-wiped.
        Some(GCMetaKind::Other(_)) | None => orphan_by_mtime(host, task_dir, "unknown kind"),
    }
}

/// `orphanByMTime` (gc.go:456–466): Orphan when the directory is older than
/// GCOrphanTTL, Skip otherwise. Centralizes the "we have no parent record
/// signal so just look at the disk" fallback used by every kind.
fn orphan_by_mtime<H: GcHost>(host: &H, task_dir: &Path, reason: &str) -> GcAction {
    let Ok(info) = std::fs::metadata(task_dir) else {
        return GcAction::Skip;
    };
    let Ok(modified) = info.modified() else {
        return GcAction::Skip;
    };
    let age = modified.elapsed().unwrap_or_default();
    if age > host.config().gc_orphan_ttl {
        tracing::info!(
            dir = %task_dir.display(),
            reason = %reason,
            age = format!("{}h0m0s", age.as_secs() / 3600),
            "gc: orphan directory"
        );
        return GcAction::Orphan;
    }
    GcAction::Skip
}

// ---------------------------------------------------------------------------
// Per-kind decisions (gc.go lines 477–763).
// ---------------------------------------------------------------------------

/// `gcDecisionIssue` (gc.go:477–500).
async fn gc_decision_issue<H: GcHost>(
    host: &H,
    ctx: &Ctx,
    task_dir: &Path,
    meta: &GcMeta,
) -> GcAction {
    if meta.issue_id.trim().is_empty() {
        return orphan_by_mtime(host, task_dir, "empty issue id");
    }

    let status = match host.get_issue_gc_check(ctx, &meta.issue_id).await {
        Ok(status) => status,
        Err(err) => {
            if is_access_not_found(&err) {
                // 404 is ambiguous: server returns it for both "issue deleted"
                // and "daemon token has no access to the workspace". Fall back
                // to the mtime-gated orphan cleanup so a scoped-down token
                // can't instantly wipe dirs whose issues are still live.
                return orphan_by_mtime(host, task_dir, "issue not accessible");
            }
            return GcAction::Skip;
        }
    };

    gc_decision_issue_result(
        host,
        task_dir,
        meta,
        IssueGCCheckResult {
            found: true,
            status: status.status,
            updated_at: status.updated_at,
            err: None,
        },
    )
}

/// `gcDecisionIssueResult` (gc.go:502–576).
fn gc_decision_issue_result<H: GcHost>(
    host: &H,
    task_dir: &Path,
    meta: &GcMeta,
    result: IssueGCCheckResult,
) -> GcAction {
    if !result.found {
        return orphan_by_mtime(host, task_dir, "issue not accessible");
    }

    // result.Status is a CATEGORY, normalized server-side, so this literal
    // comparison covers custom statuses too — an issue on a `done`-category
    // custom status is terminal here. (MUL-6243)
    let stale = result
        .updated_at
        .map(|u| {
            Utc::now().signed_duration_since(u)
                > chrono::Duration::from_std(host.config().gc_ttl).unwrap_or_default()
        })
        .unwrap_or(false);
    if (result.status == "done" || result.status == "cancelled") && stale {
        tracing::info!(
            dir = %base_name(task_dir),
            kind = "issue",
            issue = %meta.issue_id,
            status = %result.status,
            updated_at = %result.updated_at.map(|u| u.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap_or_default(),
            "gc: eligible for cleanup"
        );
        return GcAction::Clean;
    }

    // Operators may opt into a hard retention bound for completed issue-task
    // environments even while the parent issue stays open (gc.go:522–544).
    let cfg = host.config();
    if !cfg.gc_completed_task_ttl.is_zero()
        && !meta.local_directory
        && meta.completed_at.is_some()
        && is_known_issue_status(&result.status)
    {
        let completed_age = meta
            .completed_at
            .map(|c| Utc::now().signed_duration_since(c))
            .unwrap_or_default();
        if completed_age > chrono::Duration::from_std(cfg.gc_completed_task_ttl).unwrap_or_default()
        {
            tracing::info!(
                dir = %base_name(task_dir),
                kind = "issue",
                issue = %meta.issue_id,
                status = %result.status,
                completed_at = %meta.completed_at.map(|c| c.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap_or_default(),
                completed_task_ttl = go_duration(cfg.gc_completed_task_ttl),
                "gc: completed task eligible for full cleanup"
            );
            return GcAction::Clean;
        }
    }

    if !cfg.gc_artifact_ttl.is_zero() {
        if let Some(completed_at) = meta.completed_at {
            if Utc::now().signed_duration_since(completed_at)
                > chrono::Duration::from_std(cfg.gc_artifact_ttl).unwrap_or_default()
            {
                tracing::info!(
                    dir = %base_name(task_dir),
                    kind = "issue",
                    issue = %meta.issue_id,
                    status = %result.status,
                    completed_at = %completed_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "gc: eligible for artifact cleanup"
                );
                return GcAction::CleanArtifacts;
            }
        }
    }

    // Old metadata may not have completed_at. Keep that case conservative:
    // after the metadata file itself has been idle for the longer orphan TTL,
    // reclaim only the exact daemon-managed cache (gc.go:557–573).
    if !cfg.gc_artifact_ttl.is_zero() && meta.completed_at.is_none() {
        if let Some(age) = gc_meta_file_age(task_dir) {
            if age > chrono::Duration::from_std(cfg.gc_orphan_ttl).unwrap_or_default() {
                tracing::info!(
                    dir = %base_name(task_dir),
                    kind = "issue",
                    issue = %meta.issue_id,
                    status = %result.status,
                    age = format!("{}h0m0s", age.num_hours()),
                    "gc: legacy task eligible for managed artifact cleanup"
                );
                return GcAction::CleanManagedArtifacts;
            }
        }
    }

    GcAction::Skip
}

/// `isKnownIssueStatus` (gc.go:589–596): mirrors the issue status constraint
/// enforced by the server. The gc-check endpoints answer with the status's
/// CATEGORY, not the stored key. Do not teach this function about custom
/// statuses: an installed daemon has no catalog to resolve them against.
fn is_known_issue_status(status: &str) -> bool {
    matches!(
        status,
        "backlog" | "todo" | "in_progress" | "in_review" | "done" | "blocked" | "cancelled"
    )
}

/// `gcMetaFileAge` (gc.go:598–604).
fn gc_meta_file_age(task_dir: &Path) -> Option<chrono::Duration> {
    let info = std::fs::metadata(task_dir.join(".gc_meta.json")).ok()?;
    let modified = info.modified().ok()?;
    Some(Utc::now().signed_duration_since(DateTime::<Utc>::from(modified)))
}

/// `gcDecisionChat` (gc.go:606–662).
async fn gc_decision_chat<H: GcHost>(
    host: &H,
    ctx: &Ctx,
    task_dir: &Path,
    meta: &GcMeta,
) -> GcAction {
    if meta.chat_session_id.trim().is_empty() {
        return orphan_by_mtime(host, task_dir, "empty chat session id");
    }

    let status = match host
        .get_chat_session_gc_check(ctx, &meta.chat_session_id)
        .await
    {
        Ok(status) => status,
        Err(err) => {
            if is_access_not_found(&err) {
                // 404 means the chat_session row is gone — DeleteChatSession
                // is a real DELETE, so a hard delete propagates here as soon
                // as the user clicks the button. We don't gate on mtime: every
                // chat_session_id in a meta file was written by this daemon
                // under its current token.
                tracing::info!(
                    dir = %base_name(task_dir),
                    kind = "chat",
                    chat_session = %meta.chat_session_id,
                    reason = "session not accessible (hard-deleted)",
                    "gc: eligible for cleanup"
                );
                return GcAction::Clean;
            }
            return GcAction::Skip;
        }
    };

    match status.status.as_str() {
        // An active chat session's directory must never be reclaimed by mtime
        // — that would silently kill a user's idle session (#6782).
        "active" => GcAction::Skip,
        "archived" => {
            let stale = status
                .updated_at
                .map(|u| {
                    Utc::now().signed_duration_since(u)
                        > chrono::Duration::from_std(host.config().gc_ttl).unwrap_or_default()
                })
                .unwrap_or(false);
            if stale {
                tracing::info!(
                    dir = %base_name(task_dir),
                    kind = "chat",
                    chat_session = %meta.chat_session_id,
                    status = %status.status,
                    updated_at = %status.updated_at.map(|u| u.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap_or_default(),
                    "gc: eligible for cleanup"
                );
                return GcAction::Clean;
            }
            GcAction::Skip
        }
        _ => GcAction::Skip,
    }
}

/// `gcDecisionAutopilotRun` (gc.go:664–704).
async fn gc_decision_autopilot_run<H: GcHost>(
    host: &H,
    ctx: &Ctx,
    task_dir: &Path,
    meta: &GcMeta,
) -> GcAction {
    if meta.autopilot_run_id.trim().is_empty() {
        return orphan_by_mtime(host, task_dir, "empty autopilot run id");
    }

    let status = match host
        .get_autopilot_run_gc_check(ctx, &meta.autopilot_run_id)
        .await
    {
        Ok(status) => status,
        Err(err) => {
            if is_access_not_found(&err) {
                return orphan_by_mtime(host, task_dir, "autopilot run not accessible");
            }
            return GcAction::Skip;
        }
    };

    // Terminal states per the autopilot_run CHECK constraint: completed,
    // failed, skipped, issue_created. The moment the run reaches a terminal
    // state the directory is dead weight and we reclaim it immediately,
    // without waiting out GCTTL.
    if is_autopilot_run_terminal(&status.status) {
        tracing::info!(
            dir = %base_name(task_dir),
            kind = "autopilot_run",
            autopilot_run = %meta.autopilot_run_id,
            status = %status.status,
            "gc: eligible for cleanup"
        );
        return GcAction::Clean;
    }
    GcAction::Skip
}

/// `isAutopilotRunTerminal` (gc.go:710–717): mirrors the run.status CHECK in
/// migrations/042_autopilot.up.sql.
fn is_autopilot_run_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "skipped" | "issue_created")
}

/// `gcDecisionQuickCreate` (gc.go:719–751).
async fn gc_decision_quick_create<H: GcHost>(
    host: &H,
    ctx: &Ctx,
    task_dir: &Path,
    meta: &GcMeta,
) -> GcAction {
    if meta.task_id.trim().is_empty() {
        return orphan_by_mtime(host, task_dir, "empty task id");
    }

    let status = match host.get_task_gc_check(ctx, &meta.task_id).await {
        Ok(status) => status,
        Err(err) => {
            if is_access_not_found(&err) {
                // Task row was hard-deleted, or token can't see it. Either
                // way, fall back to mtime-gated orphan to stay safe across
                // scoped tokens.
                return orphan_by_mtime(host, task_dir, "task not accessible");
            }
            return GcAction::Skip;
        }
    };

    // Quick-create workdirs are not reused by the issue task that
    // LinkTaskToIssue eventually attaches, so as soon as the quick-create
    // task reaches a terminal state we can reclaim immediately.
    if is_agent_task_terminal(&status.status) {
        tracing::info!(
            dir = %base_name(task_dir),
            kind = "quick_create",
            task = %meta.task_id,
            status = %status.status,
            "gc: eligible for cleanup"
        );
        return GcAction::Clean;
    }
    GcAction::Skip
}

/// `isAgentTaskTerminal` (gc.go:756–763): reports whether a value of
/// agent_task_queue.status represents a final state.
fn is_agent_task_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled")
}

fn base_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Artifact cleanup (gc.go lines 765–997). The artifact matcher lives in
// crate::artifact_matcher — the crate's single copy (CORD-12 consolidation).
// ---------------------------------------------------------------------------

/// `cleanTaskDir` (gc.go:767–776): removes a task directory, logs the
/// reclaimed bytes, and returns that count for the cycle summary. A failed
/// removal reports zero reclaimed.
fn clean_task_dir(task_dir: &Path) -> i64 {
    let bytes = dir_size(task_dir);
    match std::fs::remove_dir_all(task_dir) {
        Err(err) => {
            tracing::warn!(dir = %task_dir.display(), error = %err, "gc: remove task dir failed");
            0
        }
        Ok(()) => {
            tracing::info!(dir = %task_dir.display(), bytes_reclaimed = bytes, "gc: removed");
            bytes
        }
    }
}

/// `linkedDirModes` (gc.go:790) equivalent on unix: a symlinked entry. Every
/// task-directory walk that deletes or measures must refuse to descend
/// through links — the per-task codex-home links the user's real skills,
/// Codex session store and plugin cache into itself, so descending would put
/// the GC inside the user's home.
fn is_linked_entry(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

/// `cleanTaskArtifacts` (gc.go:805–807): walks taskDir and deletes every
/// directory whose basename matches one of patterns, plus exact
/// daemon-managed artifact paths. Returns (removedCount, bytesReclaimed,
/// perPattern).
///
/// Safety contract:
/// - patterns are basename-only; entries with a path separator are dropped.
/// - .git subtrees are never descended into.
/// - linked directories are skipped entirely.
/// - every removal target is verified to live inside taskDir.
fn clean_task_artifacts(task_dir: &Path, patterns: &[String]) -> (i32, i64, HashMap<String, i32>) {
    let matcher = ArtifactMatcher::new(patterns, &managed_reclaimable_artifact_subpaths());
    clean_task_artifacts_matching(task_dir, &matcher)
}

/// `cleanManagedTaskArtifacts` (gc.go:819–848): removes the exact
/// daemon-managed artifact subpaths under taskDir. The managed set is a list
/// of exact relative paths, addressed directly rather than searched for.
fn clean_managed_task_artifacts(task_dir: &Path) -> (i32, i64, HashMap<String, i32>) {
    let mut removed = 0i32;
    let mut bytes = 0i64;
    let mut per_pattern = HashMap::new();
    if task_dir.as_os_str().is_empty() {
        return (removed, bytes, per_pattern);
    }
    let Ok(abs_root) = std::path::absolute(task_dir) else {
        return (removed, bytes, per_pattern);
    };
    for subpath in managed_reclaimable_artifact_subpaths() {
        let Some(rel) = safe_relative_path(&subpath) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().into_owned();
        let Some(target) = managed_artifact_target(&abs_root, &rel_str) else {
            continue;
        };
        let size = dir_size(&target);
        if let Err(err) = std::fs::remove_dir_all(&target) {
            tracing::warn!(path = %target.display(), error = %err, "gc: artifact remove failed");
            continue;
        }
        removed += 1;
        bytes += size;
        *per_pattern
            .entry(format!(
                "{MANAGED_ARTIFACT_PATTERN_PREFIX}{}",
                rel_str.replace('\\', "/")
            ))
            .or_insert(0i32) += 1;
        tracing::info!(path = %target.display(), bytes = size, "gc: artifact removed");
    }
    (removed, bytes, per_pattern)
}

/// `managedArtifactTarget` (gc.go:864–879): resolves one managed relative
/// subpath under absRoot to an absolute path that is safe to remove, None
/// when there is nothing to reclaim. Every component between absRoot and the
/// leaf is re-checked; containment needs no separate check because
/// safe_relative_path has already rejected absolute paths and anything that
/// escapes upward.
fn managed_artifact_target(abs_root: &Path, rel: &str) -> Option<PathBuf> {
    let mut current = abs_root.to_path_buf();
    for part in rel.split(['/', '\\']) {
        current = current.join(part);
        let info = std::fs::symlink_metadata(&current).ok()?;
        // Already reclaimed, never created, or unreadable — all three mean
        // "nothing for this cycle to do".
        if is_linked_entry(&info) || !info.is_dir() {
            return None;
        }
    }
    Some(current)
}

/// `hasManagedArtifact` (gc.go:885–900): reports whether any managed subpath
/// is actually present. Without this the decision layer keeps returning
/// CleanManagedArtifacts for a long-lived task whose completed_at never moves
/// again, so every cycle takes an env-root reservation and logs a reclaim
/// that removes nothing.
fn has_managed_artifact(task_dir: &Path) -> bool {
    let Ok(abs_root) = std::path::absolute(task_dir) else {
        return false;
    };
    for subpath in managed_reclaimable_artifact_subpaths() {
        let Some(rel) = safe_relative_path(&subpath) else {
            continue;
        };
        if managed_artifact_target(&abs_root, rel.to_string_lossy().as_ref()).is_some() {
            return true;
        }
    }
    false
}

/// `cleanTaskArtifactsMatching` (gc.go:902–957). Mirrors filepath.WalkDir
/// with SkipDir semantics: never descends into .git or linked directories,
/// and does not descend into a subtree it just deleted.
fn clean_task_artifacts_matching(
    task_dir: &Path,
    matcher: &ArtifactMatcher,
) -> (i32, i64, HashMap<String, i32>) {
    let mut removed = 0i32;
    let mut bytes = 0i64;
    let mut per_pattern = HashMap::new();
    if task_dir.as_os_str().is_empty() || matcher.is_empty() {
        return (removed, bytes, per_pattern);
    }

    let Ok(abs_root) = std::path::absolute(task_dir) else {
        return (removed, bytes, per_pattern);
    };

    fn walk(
        dir: &Path,
        abs_root: &Path,
        matcher: &ArtifactMatcher,
        removed: &mut i32,
        bytes: &mut i64,
        per_pattern: &mut HashMap<String, i32>,
    ) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return, // best-effort — keep walking
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(info) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !info.is_dir() || info.file_type().is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            // Never descend into .git — preserves agent commits even if a
            // pattern like "objects" would otherwise match.
            if name == ".git" {
                continue;
            }
            // Refuse to follow linked directories.
            if is_linked_entry(&info) {
                continue;
            }
            let Some(pattern) = matcher.match_directory(abs_root, &path, &name) else {
                walk(&path, abs_root, matcher, removed, bytes, per_pattern);
                continue;
            };
            let size = dir_size(&path);
            if let Err(err) = std::fs::remove_dir_all(&path) {
                tracing::warn!(path = %path.display(), error = %err, "gc: artifact remove failed");
                continue;
            }
            *removed += 1;
            *bytes += size;
            *per_pattern.entry(pattern).or_insert(0i32) += 1;
            tracing::info!(path = %path.display(), bytes = size, "gc: artifact removed");
            // Don't descend into the now-deleted subtree.
        }
    }

    walk(
        &abs_root,
        &abs_root,
        matcher,
        &mut removed,
        &mut bytes,
        &mut per_pattern,
    );
    (removed, bytes, per_pattern)
}

/// `dirSize` (gc.go:964–967): total size of all regular files under root, in
/// bytes. Linked content is not counted: RemoveAll would drop the link and
/// leave the target, so counting it would overstate what a removal reclaims.
pub(crate) fn dir_size(root: &Path) -> i64 {
    dir_size_ctx(&Ctx::new(), root).unwrap_or(0)
}

/// `dirSizeContext` (gc.go:969–997): cancellation-aware variant. Non-fatal:
/// errors during the walk are ignored so callers can report a best-effort
/// byte count without aborting the whole GC cycle.
pub(crate) fn dir_size_ctx(ctx: &Ctx, root: &Path) -> anyhow::Result<i64> {
    fn walk(ctx: &Ctx, dir: &Path, total: &mut i64) -> anyhow::Result<()> {
        if ctx.err().is_some() {
            anyhow::bail!("{}", ctx.cause());
        }
        for entry in std::fs::read_dir(dir)?.flatten() {
            if ctx.err().is_some() {
                anyhow::bail!("{}", ctx.cause());
            }
            let Ok(info) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if is_linked_entry(&info) {
                continue; // SkipDir for link-dirs / skip files alike
            }
            if info.is_dir() {
                walk(ctx, &entry.path(), total)?;
            } else if info.is_file() {
                *total += info.len() as i64;
            }
        }
        Ok(())
    }
    let mut total = 0i64;
    walk(ctx, root, &mut total)?;
    Ok(total)
}

// ---------------------------------------------------------------------------
// Repo-cache maintenance (gc.go lines 999–1509).
// ---------------------------------------------------------------------------

/// `gitCmdTimeout` (gc.go:1000).
const GIT_CMD_TIMEOUT: Duration = Duration::from_secs(30);
/// `gitMaintenanceTimeout` (gc.go:1001).
const GIT_MAINTENANCE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// `repoMaintenanceMarker` (gc.go:1002).
const REPO_MAINTENANCE_MARKER: &str = ".cordy-maintenance-pending";

/// `pruneRepoWorktreesContext` (gc.go:1011–1048): runs `git worktree prune`
/// on all bare repos in the cache, then evicts the ones nothing needs
/// anymore.
async fn prune_repo_worktrees_ctx<H: GcHost>(
    host: &H,
    ctx: &Ctx,
    workspaces_root: &Path,
    stats: &mut GcStats,
) {
    let repos_root = workspaces_root.join(REPOS_DIR_NAME);
    let Ok(ws_entries) = std::fs::read_dir(&repos_root) else {
        return;
    };

    let mut ws_names: Vec<PathBuf> = ws_entries.flatten().map(|e| e.path()).collect();
    ws_names.sort();
    for ws_repo_dir in ws_names {
        if ctx.err().is_some() {
            return;
        }
        if !std::fs::metadata(&ws_repo_dir)
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let Ok(repo_entries) = std::fs::read_dir(&ws_repo_dir) else {
            continue;
        };
        let mut repo_paths: Vec<PathBuf> = repo_entries.flatten().map(|e| e.path()).collect();
        repo_paths.sort();
        for bare_path in repo_paths {
            if ctx.err().is_some() {
                return;
            }
            if !std::fs::metadata(&bare_path)
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            if !gc_is_bare_repo(&bare_path) {
                continue;
            }
            maintain_repo_cache(host, ctx, &bare_path, stats).await;
        }
        // Drop the per-workspace directory once its last repo is gone.
        if let Ok(mut remaining) = std::fs::read_dir(&ws_repo_dir) {
            if remaining.next().is_none() {
                let _ = std::fs::remove_dir(&ws_repo_dir);
            }
        }
    }
}

/// `maintainRepoCache` (gc.go:1050–1057).
async fn maintain_repo_cache<'env, H: GcHost>(
    host: &'env H,
    ctx: &'env Ctx,
    bare_path: &'env Path,
    stats: &'env mut GcStats,
) {
    // Go passes a plain func(maintenanceCtx); we box the body once so the
    // wrapper can run it under either the cache gate or the plain lock.
    if let Some(cache) = host.repo_cache_for_gc() {
        let Some((mctx, guard)) = cache.try_begin_maintenance(ctx, bare_path) else {
            tracing::debug!(
                repo = %bare_path.display(),
                "gc: repo maintenance skipped for foreground work"
            );
            return;
        };
        prune_worktree_locked(host, &mctx, bare_path).await;
        if mctx.err().is_none() {
            evict_repo_cache_locked(host, &mctx, bare_path, stats).await;
        }
        drop(guard);
        return;
    }
    // No cache: run directly — gc.go's fallback is d.withRepoLock, which is a
    // no-op lock when the daemon has no cache (withRepoLock returns early
    // when d.cache == nil), so plain sequential execution matches Go.
    prune_worktree_locked(host, ctx, bare_path).await;
    if ctx.err().is_none() {
        evict_repo_cache_locked(host, ctx, bare_path, stats).await;
    }
}

/// `evictRepoCacheLocked` (gc.go:1135–1200): removes a bare repo cache that
/// nothing needs anymore. The caller must hold the repo lock, so this cannot
/// race a Sync or a CreateWorktree on the same repo.
///
/// All four conditions are required: GCRepoTTL > 0; no watched workspace
/// still claims the repo (a RETAIN predicate); no worktrees left after prune;
/// no task has created a worktree from it within GCRepoTTL (an unknown stamp
/// is stamped and skipped, never treated as ancient).
async fn evict_repo_cache_locked<H: GcHost>(
    host: &H,
    ctx: &Ctx,
    bare_path: &Path,
    stats: &mut GcStats,
) {
    let cfg = host.config();
    if cfg.gc_repo_ttl.is_zero() {
        return;
    }
    // Cheap early-out so an attached repo — the common case — never pays for
    // the git and filesystem work below.
    if host.repo_bare_path_is_live(bare_path) {
        return;
    }

    let worktrees = match linked_worktree_count_ctx(ctx, bare_path).await {
        Ok(count) => count,
        Err(err) => {
            if ctx.err().is_some() {
                return;
            }
            tracing::warn!(repo = %bare_path.display(), error = %err, "gc: worktree count failed");
            return;
        }
    };
    if worktrees > 0 {
        return;
    }

    let last_used = match crate::repocache::last_used(bare_path) {
        Some(stamp) => stamp,
        None => {
            // A cache created before the stamp existed. Start its clock now;
            // the alternative reading of "unknown" would evict every
            // pre-upgrade cache on the machine in the first cycle after an
            // upgrade.
            crate::repocache::mark_used(bare_path);
            return;
        }
    };
    let idle = Utc::now().signed_duration_since(last_used);
    if idle <= chrono::Duration::from_std(cfg.gc_repo_ttl).unwrap_or_default() {
        return;
    }

    // Measure before the final check, not after: dirSize walks every file in
    // the repo, which on a multi-GiB cache takes long enough for a workspace
    // to re-attach underneath us.
    let Ok(bytes) = dir_size_ctx(ctx, bare_path) else {
        return;
    };

    // Ask again immediately before deleting; re-reading in-memory state costs
    // one mutex and no network.
    if host.repo_bare_path_is_live(bare_path) {
        return;
    }

    if let Err(err) = std::fs::remove_dir_all(bare_path) {
        tracing::warn!(repo = %bare_path.display(), error = %err, "gc: repo cache remove failed");
        return;
    }
    stats.repo_caches_reclaimed += 1;
    stats.bytes_reclaimed += bytes;
    tracing::info!(
        repo = %base_name(bare_path),
        workspace = %base_name(bare_path.parent().unwrap_or(Path::new(""))),
        last_used = %last_used.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        idle = format!("{}h0m0s", idle.num_hours()),
        bytes_reclaimed = bytes,
        "gc: repo cache evicted"
    );
}

/// `linkedWorktreeCountContext` (gc.go:1210–1240): how many linked worktrees
/// a bare repo still has. `git worktree list --porcelain` emits one
/// blank-line-separated block per worktree and marks the bare repo's own
/// block with a `bare` line.
async fn linked_worktree_count_ctx(ctx: &Ctx, bare_path: &Path) -> anyhow::Result<usize> {
    let out = run_git_gc_command_ctx(ctx, bare_path, &["worktree", "list", "--porcelain"]).await?;

    let mut count = 0usize;
    let mut in_block = false;
    let mut is_bare = false;
    let flush = |count: &mut usize, in_block: &mut bool, is_bare: &mut bool| {
        if *in_block && !*is_bare {
            *count += 1;
        }
        *in_block = false;
        *is_bare = false;
    };
    for line in out.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            flush(&mut count, &mut in_block, &mut is_bare);
        } else if let Some(_rest) = line.strip_prefix("worktree ") {
            flush(&mut count, &mut in_block, &mut is_bare);
            in_block = true;
        } else if line == "bare" {
            is_bare = true;
        }
    }
    flush(&mut count, &mut in_block, &mut is_bare);
    Ok(count)
}

/// `pruneWorktreeLocked` (gc.go:1242–1359): prunes stale worktrees and agent
/// branches, then optionally runs heavier git maintenance when refs were
/// removed or a pending marker exists.
async fn prune_worktree_locked<H: GcHost>(host: &H, ctx: &Ctx, bare_path: &Path) {
    if let Err(err) = run_git_gc_command_ctx(ctx, bare_path, &["worktree", "prune"]).await {
        if ctx.err().is_some() {
            return;
        }
        tracing::warn!(repo = %bare_path.display(), error = %err, "gc: worktree prune failed");
    }

    let active_branches = match agent_worktree_branches_ctx(ctx, bare_path).await {
        Ok(branches) => branches,
        Err(err) => {
            if ctx.err().is_some() {
                return;
            }
            tracing::warn!(repo = %bare_path.display(), error = %err, "gc: worktree branch scan failed");
            return;
        }
    };

    let agent_branches = match list_agent_branches_ctx(ctx, bare_path).await {
        Ok(branches) => branches,
        Err(err) => {
            if ctx.err().is_some() {
                return;
            }
            tracing::warn!(repo = %bare_path.display(), error = %err, "gc: agent branch scan failed");
            return;
        }
    };

    let mut deleted = 0i32;
    for branch in agent_branches {
        if active_branches.contains(&branch) {
            continue;
        }
        if let Err(err) =
            run_git_gc_command_ctx(ctx, bare_path, &["branch", "-D", "--", &branch]).await
        {
            if ctx.err().is_some() {
                return;
            }
            tracing::warn!(
                repo = %bare_path.display(),
                branch = %branch,
                error = %err,
                "gc: agent branch delete failed"
            );
            continue;
        }
        deleted += 1;
    }
    let marker_path = bare_path.join(REPO_MAINTENANCE_MARKER);
    let mut pending = deleted > 0;
    if pending {
        let stamp = format!(
            "{}\n",
            Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
        );
        if let Err(err) = std::fs::write(&marker_path, stamp) {
            tracing::warn!(repo = %bare_path.display(), error = %err, "gc: record pending repo maintenance failed");
        }
        tracing::info!(repo = %bare_path.display(), count = deleted, "gc: deleted stale agent branches");
    } else if marker_path.exists() {
        pending = true;
    }
    if !pending {
        return;
    }
    if !host.config().gc_repo_maintenance_enabled {
        tracing::debug!(repo = %bare_path.display(), "gc: heavy repo maintenance disabled");
        return;
    }
    // Agent CLIs can mutate linked-worktree refs directly, outside the
    // daemon's in-process repository gate. Do not start heavy maintenance
    // while any task is active.
    if host.activity().active_tasks() > 0 {
        tracing::debug!(repo = %bare_path.display(), "gc: heavy repo maintenance deferred while tasks are active");
        return;
    }

    // Heavier maintenance only runs when we actually removed refs, so we
    // don't turn every GC tick into a full `git gc --prune` on every cached
    // repo. The prune step gets its own longer timeout because it can take
    // minutes on a real bare cache.
    let maintenance: [(&[&str], Duration); 2] = [
        (
            &["reflog", "expire", "--expire=30.days", "--all"],
            GIT_CMD_TIMEOUT,
        ),
        (&["gc", "--prune=30.days"], GIT_MAINTENANCE_TIMEOUT),
    ];
    let mut completed = true;
    for (args, timeout) in maintenance {
        if ctx.err().is_some() || host.activity().active_tasks() > 0 {
            return;
        }
        let before = snapshot_repo_maintenance_locks(bare_path);
        if let Err(err) = run_git_command_ctx(ctx, bare_path, timeout, args).await {
            completed = false;
            let cancelled_like =
                err_is_cancellation(&err) || cancel_cause_of(&err) == Some(CancelCause::Preempted);
            if cancelled_like {
                cleanup_new_repo_maintenance_locks(host, bare_path, &before);
            }
            if ctx.cause() == CancelCause::Preempted {
                tracing::info!(
                    repo = %bare_path.display(),
                    command = args.join(" "),
                    "gc: git maintenance preempted for foreground work"
                );
                return;
            }
            tracing::warn!(
                repo = %bare_path.display(),
                command = args.join(" "),
                error = %err,
                "gc: git maintenance failed"
            );
        }
    }
    if completed {
        if let Err(err) = std::fs::remove_file(&marker_path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(repo = %bare_path.display(), error = %err, "gc: clear pending repo maintenance failed");
            }
        }
    }
}

/// True when the error chain carries a process-tree cancellation
/// (`errors.Is(err, context.Canceled/DeadlineExceeded)` shape).
fn err_is_cancellation(err: &anyhow::Error) -> bool {
    cancel_cause_of(err).is_some()
}

/// Recovers a [`CancelCause`] embedded by processtree when the command was
/// cancelled (`context.Cause(ctx)` equivalent).
fn cancel_cause_of(err: &anyhow::Error) -> Option<CancelCause> {
    for cause in err.chain() {
        if let Some(processtree::ProcessError::Cancelled(c)) =
            cause.downcast_ref::<processtree::ProcessError>()
        {
            return Some(*c);
        }
    }
    None
}

/// `runGitGCCommandContext` (gc.go:1369–1371).
async fn run_git_gc_command_ctx(
    ctx: &Ctx,
    bare_path: &Path,
    args: &[&str],
) -> anyhow::Result<String> {
    run_git_command_ctx(ctx, bare_path, GIT_CMD_TIMEOUT, args).await
}

/// `runGitCommandContext` (gc.go:1373–1381): unlike repocache's helpers these
/// build a plain git command without the credential/safe.directory env block,
/// matching Go exactly.
async fn run_git_command_ctx(
    parent: &Ctx,
    bare_path: &Path,
    timeout: Duration,
    args: &[&str],
) -> anyhow::Result<String> {
    let bare = bare_path.to_string_lossy().into_owned();
    let mut cmd_args: Vec<&str> = vec!["-C", &bare];
    cmd_args.extend_from_slice(args);
    let outcome = git_deadline(parent, timeout, |c| {
        let mut cmd = tokio::process::Command::new("git");
        cmd.args(&cmd_args);
        async move { processtree::combined_output(&c, cmd, Duration::from_secs(5)).await }
    })
    .await;
    match outcome {
        Ok((out, Ok(()))) => Ok(String::from_utf8_lossy(&out).trim().to_string()),
        Ok((_out, Err(err))) => Err(err),
        Err(_cause) => Err(anyhow::anyhow!(
            "git command timed out after {}: context deadline exceeded",
            go_duration(timeout)
        )),
    }
}

/// Local mirror of repocache's deadline helper (context.WithTimeout around a
/// processtree call, gc.go:1374).
async fn git_deadline<T, F, Fut>(parent: &Ctx, timeout: Duration, f: F) -> Result<T, CancelCause>
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

/// `repoMaintenanceLockSnapshot` (gc.go:1383): records only lock paths known
/// to be produced by the maintenance commands. Cleanup later removes a path
/// only if it did not exist in this snapshot and the process tree is
/// confirmed gone.
type RepoMaintenanceLockSnapshot = std::collections::HashSet<PathBuf>;

/// `snapshotRepoMaintenanceLocks` (gc.go:1390–1396).
fn snapshot_repo_maintenance_locks(bare_path: &Path) -> RepoMaintenanceLockSnapshot {
    repo_maintenance_lock_paths(bare_path).into_iter().collect()
}

/// `repoMaintenanceLockPaths` (gc.go:1398–1429).
fn repo_maintenance_lock_paths(bare_path: &Path) -> Vec<PathBuf> {
    let mut locks = Vec::new();
    for name in ["gc.pid", "packed-refs.lock"] {
        let path = bare_path.join(name);
        if let Ok(info) = std::fs::symlink_metadata(&path) {
            if info.is_file() {
                locks.push(path);
            }
        }
    }
    for root in [
        bare_path.join("refs"),
        bare_path.join("logs").join("refs"),
        bare_path.join("objects").join("info"),
        bare_path.join("objects").join("pack"),
    ] {
        collect_lock_files(&root, &mut locks);
    }
    locks
}

fn collect_lock_files(dir: &Path, locks: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(info) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if is_linked_entry(&info) {
            continue;
        }
        if info.is_dir() {
            collect_lock_files(&entry.path(), locks);
        } else if entry.file_name().to_string_lossy().ends_with(".lock") {
            locks.push(entry.path());
        }
    }
}

/// `cleanupNewRepoMaintenanceLocks` (gc.go:1431–1447).
fn cleanup_new_repo_maintenance_locks<H: GcHost>(
    _host: &H,
    bare_path: &Path,
    before: &RepoMaintenanceLockSnapshot,
) {
    for path in repo_maintenance_lock_paths(bare_path) {
        if before.contains(&path) {
            continue;
        }
        let Ok(rel) = path.strip_prefix(bare_path) else {
            tracing::warn!(
                repo = %bare_path.display(),
                path = %path.display(),
                "gc: refused maintenance lock cleanup outside repo"
            );
            continue;
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {
                tracing::info!(
                    repo = %bare_path.display(),
                    lock = %rel.display(),
                    "gc: removed lock left by interrupted maintenance"
                );
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                tracing::warn!(
                    repo = %bare_path.display(),
                    lock = %rel.display(),
                    error = %err,
                    "gc: maintenance lock cleanup failed"
                );
            }
        }
    }
}

/// `agentWorktreeBranchesContext` (gc.go:1453–1471).
async fn agent_worktree_branches_ctx(
    ctx: &Ctx,
    bare_path: &Path,
) -> anyhow::Result<std::collections::HashSet<String>> {
    let out = run_git_gc_command_ctx(ctx, bare_path, &["worktree", "list", "--porcelain"]).await?;
    let mut branches = std::collections::HashSet::new();
    for line in out.split('\n') {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("branch refs/heads/") else {
            continue;
        };
        if rest.starts_with("agent/") {
            branches.insert(rest.to_string());
        }
    }
    Ok(branches)
}

/// `listAgentBranchesContext` (gc.go:1477–1498). Trailing slash narrows the
/// pattern to the `agent/` namespace only; without it `for-each-ref` would
/// also return a branch literally named `agent`, which
/// agentWorktreeBranches ignores — that branch would then be deleted.
async fn list_agent_branches_ctx(ctx: &Ctx, bare_path: &Path) -> anyhow::Result<Vec<String>> {
    let out = run_git_gc_command_ctx(
        ctx,
        bare_path,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/agent/",
        ],
    )
    .await?;
    if out.is_empty() {
        return Ok(Vec::new());
    }
    Ok(out
        .split('\n')
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(str::to_string)
        .collect())
}

/// `isBareRepo` (gc.go:1501–1509): checks if a path looks like a bare git
/// repository (HEAD + objects present). Note gc.go's stricter two-file check
/// differs from repocache's HEAD-only probe.
fn gc_is_bare_repo(path: &Path) -> bool {
    path.join("HEAD").exists() && path.join("objects").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DisabledGcHost {
        config: GcConfig,
        activity: Arc<DaemonActivity>,
    }

    impl GcHost for DisabledGcHost {
        fn config(&self) -> &GcConfig {
            &self.config
        }

        async fn get_issue_gc_check(
            &self,
            _ctx: &Ctx,
            _issue_id: &str,
        ) -> anyhow::Result<IssueGCCheckStatus> {
            panic!("disabled GC must not query issue state")
        }

        async fn get_issue_gc_checks(
            &self,
            _ctx: &Ctx,
            _workspace_id: &str,
            _issue_ids: &[String],
        ) -> anyhow::Result<HashMap<String, IssueGCCheckResult>> {
            panic!("disabled GC must not query issue state")
        }

        async fn get_chat_session_gc_check(
            &self,
            _ctx: &Ctx,
            _chat_session_id: &str,
        ) -> anyhow::Result<IssueGCCheckStatus> {
            panic!("disabled GC must not query chat state")
        }

        async fn get_autopilot_run_gc_check(
            &self,
            _ctx: &Ctx,
            _autopilot_run_id: &str,
        ) -> anyhow::Result<IssueGCCheckStatus> {
            panic!("disabled GC must not query autopilot state")
        }

        async fn get_task_gc_check(
            &self,
            _ctx: &Ctx,
            _task_id: &str,
        ) -> anyhow::Result<IssueGCCheckStatus> {
            panic!("disabled GC must not query task state")
        }

        fn activity(&self) -> &Arc<DaemonActivity> {
            &self.activity
        }

        fn repo_bare_path_is_live(&self, _bare_path: &Path) -> bool {
            panic!("disabled GC must not inspect repository liveness")
        }

        fn repo_cache_for_gc(&self) -> Option<&crate::repocache::Cache> {
            panic!("disabled GC must not access the repository cache")
        }
    }

    #[tokio::test]
    async fn disabled_loop_remains_owned_until_cancelled() {
        let host = DisabledGcHost {
            config: GcConfig {
                profile: String::new(),
                workspaces_root: PathBuf::new(),
                gc_enabled: false,
                gc_interval: Duration::from_secs(1),
                gc_ttl: Duration::ZERO,
                gc_completed_task_ttl: Duration::ZERO,
                gc_orphan_ttl: Duration::ZERO,
                gc_artifact_ttl: Duration::ZERO,
                gc_codex_session_ttl: Duration::ZERO,
                gc_hermes_memory_ttl: Duration::ZERO,
                gc_hermes_session_ttl: Duration::ZERO,
                gc_repo_ttl: Duration::ZERO,
                gc_repo_maintenance_enabled: false,
                gc_artifact_patterns: Vec::new(),
            },
            activity: DaemonActivity::new(),
        };
        let ctx = Ctx::new();
        let loop_future = gc_loop(&host, &ctx);
        tokio::pin!(loop_future);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut loop_future)
                .await
                .is_err(),
            "disabled GC owner returned before cancellation"
        );
        ctx.cancel_with(CancelCause::Shutdown);
        tokio::time::timeout(Duration::from_secs(1), &mut loop_future)
            .await
            .expect("disabled GC owner ignored cancellation");
    }

    /// safeRelativePath contract (artifact_matcher.go:74–84): rejects empty,
    /// absolute, and upward-escaping paths; cleans the rest.
    #[test]
    fn safe_relative_path_contract() {
        let p = |s: &str| safe_relative_path(s).map(|v| v.to_string_lossy().into_owned());
        assert_eq!(p("a/b/c"), Some("a/b/c".into()));
        assert_eq!(p("./a"), Some("a".into()));
        assert_eq!(p("a//b"), Some("a/b".into()));
        assert_eq!(p(""), None);
        assert_eq!(p("/abs/path"), None);
        assert_eq!(p(".."), None);
        assert_eq!(p("../escape"), None);
        assert_eq!(p("a/../.."), None);
    }

    #[test]
    fn unknown_local_directory_kind_is_never_fully_removed() {
        let host = DisabledGcHost {
            config: GcConfig {
                profile: String::new(),
                workspaces_root: PathBuf::new(),
                gc_enabled: false,
                gc_interval: Duration::ZERO,
                gc_ttl: Duration::ZERO,
                gc_completed_task_ttl: Duration::ZERO,
                gc_orphan_ttl: Duration::ZERO,
                gc_artifact_ttl: Duration::ZERO,
                gc_codex_session_ttl: Duration::ZERO,
                gc_hermes_memory_ttl: Duration::ZERO,
                gc_hermes_session_ttl: Duration::ZERO,
                gc_repo_ttl: Duration::ZERO,
                gc_repo_maintenance_enabled: false,
                gc_artifact_patterns: Vec::new(),
            },
            activity: DaemonActivity::new(),
        };
        let meta = GcMeta {
            kind: Some(GCMetaKind::Other("future_parent".to_string())),
            local_directory: true,
            ..GcMeta::default()
        };

        assert_eq!(
            apply_local_directory_gc_override(&host, &meta, GcAction::Clean),
            GcAction::Skip
        );
    }

    #[tokio::test]
    async fn process_tree_captures_success_and_failure_output() {
        let ctx = Ctx::new();
        #[cfg(unix)]
        let mut success = tokio::process::Command::new("/bin/sh");
        #[cfg(unix)]
        success.args(["-c", "printf success"]);
        #[cfg(windows)]
        let mut success = tokio::process::Command::new("cmd.exe");
        #[cfg(windows)]
        success.args(["/D", "/C", "echo success"]);
        let (output, result) =
            processtree::combined_output(&ctx, success, Duration::from_secs(1)).await;
        result.unwrap();
        assert_eq!(String::from_utf8_lossy(&output).trim_end(), "success");

        #[cfg(unix)]
        let mut failure = tokio::process::Command::new("/bin/sh");
        #[cfg(unix)]
        failure.args([
            "-c",
            "printf \"fatal: a branch named 'taken' already exists\" >&2; exit 128",
        ]);
        #[cfg(windows)]
        let mut failure = tokio::process::Command::new("cmd.exe");
        #[cfg(windows)]
        failure.args([
            "/D",
            "/C",
            "echo fatal: a branch named 'taken' already exists 1>&2 & exit /b 128",
        ]);
        let (output, result) =
            processtree::combined_output(&ctx, failure, Duration::from_secs(1)).await;
        assert!(result.is_err());
        assert!(String::from_utf8_lossy(&output).contains("a branch named 'taken'"));
    }

    #[test]
    fn prune_store_tree_removes_only_expired_leaf_stores() {
        let temp = tempfile::tempdir().unwrap();
        let old_store = temp.path().join("agent-a/profile-a");
        let fresh_store = temp.path().join("agent-b/profile-b");
        std::fs::create_dir_all(&old_store).unwrap();
        std::fs::create_dir_all(&fresh_store).unwrap();
        std::fs::write(old_store.join("memory.db"), b"old").unwrap();
        std::fs::write(fresh_store.join("memory.db"), b"fresh").unwrap();

        let reserved = std::sync::Mutex::new(Vec::new());
        let activity = DaemonActivity::new();
        let reserve = |path: &Path| {
            reserved.lock().unwrap().push(path.to_path_buf());
            activity.reserve_store_for_gc(path)
        };
        let now = Utc::now() + chrono::Duration::seconds(2);
        let (removed, bytes) =
            prune_store_tree(temp.path(), 2, Duration::from_secs(1), now, &reserve);

        assert_eq!(removed, 2);
        assert_eq!(bytes, 8);
        assert_eq!(reserved.lock().unwrap().len(), 2);
        assert!(!old_store.exists());
        assert!(!fresh_store.exists());
    }

    #[test]
    fn prune_store_tree_is_disabled_by_zero_ttl() {
        let temp = tempfile::tempdir().unwrap();
        let store = temp.path().join("agent/profile");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("memory.db"), b"keep").unwrap();

        let reserve = |_path: &Path| -> Option<crate::activity::StoreGcReservation> {
            panic!("disabled pruning must not reserve a store")
        };
        let result = prune_store_tree(temp.path(), 2, Duration::ZERO, Utc::now(), &reserve);

        assert_eq!(result, (0, 0));
        assert!(store.exists());
    }
}
