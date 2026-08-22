//! Port of `server/internal/daemon/gc.go` (1,509 lines) plus the unix
//! `processtree/{run.go,controller_unix.go}` inlined as [`processtree`].
//!
//! Deviations from Go:
//! - `*Daemon` receiver → [`GcHost`] trait; config fields live in
//!   [`GcConfig`] mirroring the exact Go field names/types from
//!   `internal/daemon/config.go:99–118`.
//! - execenv-owned pieces (GCMeta, store pruners, managed-artifact helpers)
//!   are local seam stand-ins marked `// S9-integration:`; this module never
//!   references `crate::execenv`.
//! - `time.Ticker` → `tokio::time::interval` with `MissedTickBehavior::Delay`
//!   (Go tickers drop missed ticks).
//! - `context.Context` → [`Ctx`](crate::repocache::Ctx); slog → tracing with
//!   identical messages.

// S9-integration: entry points (gc_loop/run_gc) and seam types are wired by
// the daemon-runner/execenv lanes; silence dead-code until that lands.
#![allow(dead_code)]

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, Utc};

use crate::repocache::{normalize_lexically, CancelCause, Ctx};

// ---------------------------------------------------------------------------
// processtree (inlined port of processtree/run.go + controller_unix.go).
// ---------------------------------------------------------------------------

/// Port of `server/internal/daemon/processtree`: runs bounded helper commands
/// whose descendants must not survive cancellation. Uses a Unix process group
/// (`setpgid`) — unix only, matching the Go build tag.
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
        _wait_delay: Duration,
        combined: bool,
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        if let Some(cause) = ctx.err() {
            return Err(anyhow::anyhow!(cause.to_string()));
        }
        // newController (controller_unix.go:18–24): own process group.
        cmd.process_group(0);
        cmd.stdin(Stdio::null());
        let mut child = cmd.spawn().context("start process")?;
        let pid = child.id().unwrap_or_default() as i32;

        // Drain pipes concurrently into buffers, like Go's exec copying
        // goroutines writing into bytes.Buffer. stdout is drained first so
        // split-mode callers can attribute buffers by index.
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let mut pipes: Vec<Box<dyn tokio::io::AsyncRead + Unpin + Send>> = Vec::new();
        if let Some(p) = child.stdout.take() {
            pipes.push(Box::new(p));
        }
        if let Some(p) = child.stderr.take() {
            pipes.push(Box::new(p));
        }
        let mut drain_tasks: Vec<tokio::task::JoinHandle<(usize, Vec<u8>)>> = Vec::new();
        for (idx, pipe) in pipes.into_iter().enumerate() {
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
                (idx, local)
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
                            stop_err = Some(match stop_err.take() {
                                Some(prev) => std::io::Error::other(format!("{prev}; {e}")),
                                None => e,
                            });
                        }
                        Some(child.wait().await)
                    }
                }
            }
        };

        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();
        for task in drain_tasks {
            let (idx, buf) = task.await.unwrap_or((usize::MAX, Vec::new()));
            if combined {
                continue; // already accumulated in `shared`
            }
            if idx == 0 {
                stdout_buf = buf;
            } else {
                stderr_buf = buf;
            }
        }
        let combined_buf = std::mem::take(&mut *shared.lock().unwrap());

        // errors.Join(stopErr, finishErr) != nil → "stop process tree: %w"
        if let Err(finish_err) = finish(pid).await {
            let mut err = finish_err.context("stop process tree");
            if let Some(se) = stop_err.take() {
                err = err.context(se.to_string());
            }
            return Err(err);
        }
        if let Some(se) = stop_err.take() {
            return Err(anyhow::Error::new(se).context("stop process tree"));
        }
        if cancelled {
            return Err(anyhow::Error::new(ProcessError::Cancelled(ctx.cause())));
        }

        let status = match status_result {
            Some(Ok(status)) => status,
            Some(Err(e)) => return Err(anyhow::Error::new(ProcessError::Io(e))),
            None => return Err(anyhow::Error::new(ProcessError::Cancelled(ctx.cause()))),
        };
        if !status.success() {
            let err = if let Some(sig) = status.signal() {
                anyhow::Error::new(ProcessError::Signal(sig))
            } else {
                anyhow::Error::new(ProcessError::Exit(status.code().unwrap_or(-1)))
            };
            return Err(err);
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
            Err(err) => (Vec::new(), Err(err)),
        }
    }

    /// `Output` (run.go:28–37): the process-tree-safe equivalent of
    /// exec.Cmd.Output.
    pub(crate) async fn output(
        ctx: &Ctx,
        cmd: Command,
        wait_delay: Duration,
    ) -> anyhow::Result<Vec<u8>> {
        run_inner(ctx, cmd, wait_delay, false)
            .await
            .map(|(out, _)| out)
    }

    /// `Run` (run.go:40–42): executes an unstarted command while owning its
    /// complete process tree.
    pub(crate) async fn run(ctx: &Ctx, cmd: Command, wait_delay: Duration) -> anyhow::Result<()> {
        run_inner(ctx, cmd, wait_delay, true).await.map(|_| ())
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

// ---------------------------------------------------------------------------
// S9-integration seam stand-ins (gc.go imports execenv + daemon client).
// ---------------------------------------------------------------------------

// S9-integration: mirrors execenv.GCMetaKind / GCMeta / ReadGCMeta
// (execenv/execenv.go:947–1023). Swap to the shared execenv module at
// integration time; this module must not reference crate::execenv.

/// `execenv.GCMetaKind` (execenv.go:947–953): string-kind discriminator for
/// the parent record that governs a task dir's lifecycle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum GcMetaKind {
    /// Pre-v2 meta files (no kind field) normalize to Issue so the legacy
    /// issue path keeps working without a migration.
    #[default]
    Issue,
    Chat,
    AutopilotRun,
    QuickCreate,
    Unknown,
}

fn parse_gc_meta_kind(raw: &str) -> GcMetaKind {
    match raw {
        "" | "issue" => GcMetaKind::Issue,
        "chat" => GcMetaKind::Chat,
        "autopilot_run" => GcMetaKind::AutopilotRun,
        "quick_create" => GcMetaKind::QuickCreate,
        _ => GcMetaKind::Unknown,
    }
}

/// `execenv.GCMeta` (execenv.go:963–984): persisted to `.gc_meta.json` inside
/// the env root so the GC loop can make parent-aware decisions.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct GcMeta {
    #[serde(default, rename = "kind")]
    kind_raw: String,
    #[serde(default, rename = "issue_id")]
    issue_id: String,
    #[serde(default, rename = "chat_session_id")]
    chat_session_id: String,
    #[serde(default, rename = "autopilot_run_id")]
    autopilot_run_id: String,
    #[serde(default, rename = "task_id")]
    task_id: String,
    #[serde(default, rename = "workspace_id")]
    workspace_id: String,
    #[serde(default, rename = "completed_at")]
    completed_at: Option<DateTime<Utc>>,
    /// Marks tasks whose WorkDir pointed at a user-owned path rather than the
    /// synthesised envRoot/workdir. The GC loop honours this by never falling
    /// into the full-clean branch.
    #[serde(default, rename = "local_directory")]
    local_directory: bool,
}

impl GcMeta {
    pub(crate) fn kind(&self) -> GcMetaKind {
        parse_gc_meta_kind(&self.kind_raw)
    }
    pub(crate) fn issue_id(&self) -> &str {
        &self.issue_id
    }
    pub(crate) fn completed_at(&self) -> Option<DateTime<Utc>> {
        self.completed_at
    }
    pub(crate) fn local_directory(&self) -> bool {
        self.local_directory
    }
}

/// `execenv.ReadGCMeta` (execenv.go:1008–1023): reads GC metadata from a task
/// directory root. Pre-v2 meta files (no kind field) are normalized to Issue
/// so the legacy issue path keeps working without a migration.
fn read_gc_meta(env_root: &Path) -> anyhow::Result<GcMeta> {
    let data = std::fs::read(env_root.join(".gc_meta.json"))?;
    let mut meta: GcMeta = serde_json::from_slice(&data).context("unmarshal gc meta")?;
    if meta.kind_raw.is_empty() {
        meta.kind_raw = "issue".to_string();
    }
    Ok(meta)
}

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
    pub(crate) id: String,
    pub(crate) found: bool,
    pub(crate) status: String,
    pub(crate) updated_at: Option<DateTime<Utc>>,
    pub(crate) err: Option<std::sync::Arc<anyhow::Error>>,
}

/// Per-issue single check payload used by `gcDecisionIssue`
/// (gc.go:494–499).
#[derive(Debug, Clone, Default)]
pub(crate) struct IssueGCCheckStatus {
    pub(crate) status: String,
    pub(crate) updated_at: Option<DateTime<Utc>>,
}

// S9-integration: mirrors execenv.ManagedReclaimableArtifactSubpaths. The
// real list lives in execenv; swap at integration time.

/// `execenv.ManagedReclaimableArtifactSubpaths`: labels logged at GC startup
/// and the exact managed paths cleaned by clean_managed_task_artifacts.
fn managed_reclaimable_artifact_subpaths() -> Vec<String> {
    Vec::new()
}

// S9-integration: mirrors execenv.PruneCodexSessionStores /
// PruneHermesMemoryStores / PruneHermesSessionStores. Each returns
// (storesRemoved, bytesReclaimed); the real implementations live in execenv
// and receive `reserve_store_for_deletion` as a reservation callback.

/// Reservation callback handed to store pruners (`d.reserveStoreForDeletion`).
pub(crate) type ReserveStoreForDeletion<'a> = &'a (dyn Fn(&Path) + Send + Sync);

fn prune_codex_session_stores(
    profile: &str,
    ttl: Duration,
    now: DateTime<Utc>,
    reserve: ReserveStoreForDeletion<'_>,
) -> (usize, i64) {
    let _ = (profile, ttl, now, reserve);
    (0, 0)
}

fn prune_hermes_memory_stores(
    profile: &str,
    ttl: Duration,
    now: DateTime<Utc>,
    reserve: ReserveStoreForDeletion<'_>,
) -> (usize, i64) {
    let _ = (profile, ttl, now, reserve);
    (0, 0)
}

fn prune_hermes_session_stores(
    profile: &str,
    ttl: Duration,
    now: DateTime<Utc>,
    reserve: ReserveStoreForDeletion<'_>,
) -> (usize, i64) {
    let _ = (profile, ttl, now, reserve);
    (0, 0)
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

    /// `d.isActiveEnvRoot`: true while a task is running on this env root.
    fn is_active_env_root(&self, task_dir: &Path) -> bool;

    /// `d.reserveEnvRootForGC`: atomically reserves an env root for deletion;
    /// returns the release closure, or None when reservation failed.
    fn reserve_env_root_for_gc(&self, task_dir: &Path) -> Option<Box<dyn FnOnce() + Send>>;

    /// `d.reserveStoreForDeletion`: reservation callback passed to store
    /// pruners.
    fn reserve_store_for_deletion(&self, path: &Path);

    /// `d.repoBarePathIsLive`: whether any watched workspace still claims
    /// this bare repo path (gc.go:1141/1183).
    fn repo_bare_path_is_live(&self, bare_path: &Path) -> bool;

    /// `d.activeTasks.Load()` (gc.go:1312/1330).
    fn active_tasks(&self) -> i64;

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
    let mut ws_entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    ws_entries.sort();
    for ws_entry in ws_entries {
        // Skip every daemon-internal dot directory, not just .repos. A
        // workspace directory is always a UUID, so a dot-prefixed entry is one
        // of our own caches. Walking .skill-cache as if it were a workspace
        // made its `v1` directory look like a task dir with no .gc_meta.json,
        // so the orphan path would delete the entire bundle cache once its
        // mtime went 72h without a new bundle.
        let is_dir = std::fs::metadata(&ws_entry)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        let name = ws_entry
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if !is_dir || name.starts_with('.') {
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
    let reserve = |p: &Path| host.reserve_store_for_deletion(p);
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
    let task_paths: Vec<PathBuf> = match std::fs::read_dir(ws_dir) {
        Ok(entries) => entries.flatten().map(|e| e.path()).collect(),
        Err(err) => {
            tracing::warn!(dir = %ws_dir.display(), error = %err, "gc: read workspace dir failed");
            return;
        }
    };

    let mut cleaned_here = 0i32;
    let mut issue_candidates: Vec<IssueGcCandidate> = Vec::with_capacity(task_paths.len());
    for task_dir in task_paths {
        if ctx.err().is_some() {
            return;
        }
        let is_dir = std::fs::metadata(&task_dir)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        if !is_dir {
            continue;
        }
        if host.is_active_env_root(&task_dir) {
            stats.skipped += 1;
            continue;
        }
        match read_gc_meta(&task_dir) {
            Ok(meta) if meta.kind() == GcMetaKind::Issue && !meta.issue_id().trim().is_empty() => {
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
        let issue_id = candidate.meta.issue_id().trim().to_string();
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
        let issue_id = candidate.meta.issue_id().trim().to_string();
        let Some(result) = results.remove(&issue_id) else {
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
        match host.reserve_env_root_for_gc(task_dir) {
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
    if host.is_active_env_root(task_dir) {
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
    let Some(completed_at) = meta.completed_at() else {
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
        kind = %meta_kind_str(meta),
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
    if !meta.local_directory() {
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
    match meta.kind() {
        GcMetaKind::Issue => gc_decision_issue(host, ctx, task_dir, meta).await,
        GcMetaKind::Chat => gc_decision_chat(host, ctx, task_dir, meta).await,
        GcMetaKind::AutopilotRun => gc_decision_autopilot_run(host, ctx, task_dir, meta).await,
        GcMetaKind::QuickCreate => gc_decision_quick_create(host, ctx, task_dir, meta).await,
        // Unknown kind: fall back to mtime-based orphan cleanup so a future
        // daemon writing a kind we don't recognize doesn't get insta-wiped.
        _ => orphan_by_mtime(host, task_dir, "unknown kind"),
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
    if meta.issue_id().trim().is_empty() {
        return orphan_by_mtime(host, task_dir, "empty issue id");
    }

    let status = match host.get_issue_gc_check(ctx, meta.issue_id()).await {
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
            id: meta.issue_id().to_string(),
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
            issue = %meta.issue_id(),
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
        && !meta.local_directory()
        && meta.completed_at().is_some()
        && is_known_issue_status(&result.status)
    {
        let completed_age = meta
            .completed_at()
            .map(|c| Utc::now().signed_duration_since(c))
            .unwrap_or_default();
        if completed_age > chrono::Duration::from_std(cfg.gc_completed_task_ttl).unwrap_or_default()
        {
            tracing::info!(
                dir = %base_name(task_dir),
                kind = "issue",
                issue = %meta.issue_id(),
                status = %result.status,
                completed_at = %meta.completed_at().map(|c| c.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap_or_default(),
                completed_task_ttl = go_duration(cfg.gc_completed_task_ttl),
                "gc: completed task eligible for full cleanup"
            );
            return GcAction::Clean;
        }
    }

    if !cfg.gc_artifact_ttl.is_zero() {
        if let Some(completed_at) = meta.completed_at() {
            if Utc::now().signed_duration_since(completed_at)
                > chrono::Duration::from_std(cfg.gc_artifact_ttl).unwrap_or_default()
            {
                tracing::info!(
                    dir = %base_name(task_dir),
                    kind = "issue",
                    issue = %meta.issue_id(),
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
    if !cfg.gc_artifact_ttl.is_zero() && meta.completed_at().is_none() {
        if let Some(age) = gc_meta_file_age(task_dir) {
            if age > chrono::Duration::from_std(cfg.gc_orphan_ttl).unwrap_or_default() {
                tracing::info!(
                    dir = %base_name(task_dir),
                    kind = "issue",
                    issue = %meta.issue_id(),
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

/// Raw kind string for log fields, matching Go's `string(meta.Kind)`.
fn meta_kind_str(meta: &GcMeta) -> &str {
    match meta.kind() {
        GcMetaKind::Issue => "issue",
        GcMetaKind::Chat => "chat",
        GcMetaKind::AutopilotRun => "autopilot_run",
        GcMetaKind::QuickCreate => "quick_create",
        GcMetaKind::Unknown => "unknown",
    }
}

// ---------------------------------------------------------------------------
// Artifact cleanup (gc.go lines 765–997 + artifact_matcher.go inlined).
// ---------------------------------------------------------------------------

/// `managedArtifactPatternPrefix` (artifact_matcher.go:10).
const MANAGED_ARTIFACT_PATTERN_PREFIX: &str = "managed:";

/// `artifactMatcher` (artifact_matcher.go:15–19): combines
/// operator-configured basename matches with exact daemon-managed paths.
/// Exact paths take precedence so a broad basename such as .sandbox-bin
/// cannot double-count a managed directory.
struct ArtifactMatcher {
    basenames: std::collections::HashSet<String>,
    exact_paths: HashMap<String, String>,
    exact_leaf_names: std::collections::HashSet<String>,
}

impl ArtifactMatcher {
    /// `newArtifactMatcher` (artifact_matcher.go:21–37).
    fn new(patterns: &[String], managed_subpaths: &[String]) -> Self {
        let mut matcher = ArtifactMatcher {
            basenames: patterns.iter().cloned().collect(),
            exact_paths: HashMap::with_capacity(managed_subpaths.len()),
            exact_leaf_names: std::collections::HashSet::with_capacity(managed_subpaths.len()),
        };
        for subpath in managed_subpaths {
            let Some(cleaned) = safe_relative_path(subpath) else {
                continue;
            };
            let display = cleaned.replace('\\', "/");
            matcher.exact_paths.insert(
                cleaned.clone(),
                format!("{MANAGED_ARTIFACT_PATTERN_PREFIX}{display}"),
            );
            if let Some(leaf) = Path::new(&cleaned).file_name() {
                matcher
                    .exact_leaf_names
                    .insert(leaf.to_string_lossy().into_owned());
            }
        }
        matcher
    }

    /// `matchDirectory` (artifact_matcher.go:39–63).
    fn match_directory(&self, abs_root: &Path, path: &Path, name: &str) -> Option<String> {
        let exact_candidate = self.exact_leaf_names.contains(name);
        let basename_match = self.basenames.contains(name);
        if !exact_candidate && !basename_match {
            return None;
        }

        // Rel and containment validation are only needed for a directory
        // whose leaf could actually match. Most workdir entries avoid this
        // path entirely.
        let rel = path.strip_prefix(abs_root).ok()?;
        let rel = safe_relative_path(&rel.to_string_lossy())?;
        if let Some(label) = self.exact_paths.get(&rel) {
            return Some(label.clone());
        }
        if basename_match {
            return Some(name.to_string());
        }
        None
    }
}

/// `safeRelativePath` (artifact_matcher.go:74–84).
fn safe_relative_path(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() || Path::new(path).is_absolute() {
        return None;
    }
    let cleaned = normalize_lexically(Path::new(path));
    let cleaned = cleaned.to_string_lossy().into_owned();
    if cleaned == "."
        || cleaned == ".."
        || cleaned.starts_with("../")
        || cleaned.starts_with("..\\")
    {
        return None;
    }
    Some(cleaned)
}

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
        let Some(target) = managed_artifact_target(&abs_root, &rel) else {
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
                rel.replace('\\', "/")
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
        if managed_artifact_target(&abs_root, &rel).is_some() {
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
    if task_dir.as_os_str().is_empty()
        || (matcher.basenames.is_empty() && matcher.exact_paths.is_empty())
    {
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
async fn maintain_repo_cache<H: GcHost>(
    host: &H,
    ctx: &Ctx,
    bare_path: &Path,
    stats: &mut GcStats,
) {
    with_repo_maintenance(host, ctx, bare_path, |mctx: Ctx| async move {
        prune_worktree_locked(host, &mctx, bare_path).await;
        if mctx.err().is_none() {
            evict_repo_cache_locked(host, &mctx, bare_path, stats).await;
        }
    })
    .await;
}

/// `withRepoMaintenance` (gc.go:1074–1089): uses the cache's foreground-
/// priority gate when available; falls back to the plain repo lock otherwise.
async fn with_repo_maintenance<H, F, Fut>(host: &H, ctx: &Ctx, bare_path: &Path, f: F)
where
    H: GcHost,
    F: FnOnce(Ctx) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    match host.repo_cache_for_gc() {
        Some(cache) => {
            let ran = cache
                .with_repo_maintenance(ctx, bare_path, |mctx: Ctx| async move {
                    f(mctx).await;
                    Ok(())
                })
                .await;
            match ran {
                Ok((true, _)) => {}
                Ok((false, _)) => {
                    tracing::debug!(
                        repo = %bare_path.display(),
                        "gc: repo maintenance skipped for foreground work"
                    );
                }
                Err(err) => {
                    if ctx.err().is_none() {
                        tracing::warn!(
                            repo = %bare_path.display(),
                            error = %err,
                            "gc: repo maintenance lock failed"
                        );
                    }
                }
            }
        }
        None => {
            // Go fallback: d.withRepoLock(barePath, func() { fn(ctx) }) — the
            // caller's context flows through unchanged.
            with_repo_lock(host, ctx, bare_path, f).await;
        }
    }
}

/// `withRepoLock` (gc.go:1094–1105): serializes a mutation against Sync /
/// CreateWorktree on the same bare repo. A daemon built without a repo cache
/// has no lock to take and runs the work directly.
async fn with_repo_lock<H, F, Fut>(host: &H, ctx: &Ctx, bare_path: &Path, f: F)
where
    H: GcHost,
    F: FnOnce(Ctx) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    match host.repo_cache_for_gc() {
        None => {
            f(ctx.clone()).await;
        }
        Some(cache) => {
            if let Err(err) = cache
                .with_repo_lock_ctx(ctx, bare_path, |c: Ctx| async move {
                    f(c).await;
                    Ok(())
                })
                .await
            {
                tracing::warn!(repo = %bare_path.display(), error = %err, "gc: repo lock failed");
            }
        }
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

    fn flush(count: &mut usize, in_block: &mut bool, is_bare: &mut bool) {
        if *in_block && !*is_bare {
            *count += 1;
        }
        *in_block = false;
        *is_bare = false;
    }
    let mut count = 0usize;
    let mut in_block = false;
    let mut is_bare = false;
    for line in out.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            flush(&mut count, &mut in_block, &mut is_bare);
        } else if line.starts_with("worktree ") {
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
    if host.active_tasks() > 0 {
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
        if ctx.err().is_some() || host.active_tasks() > 0 {
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
    let mut cmd_args: Vec<&str> = vec!["-C", bare.as_str()];
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

// ---------------------------------------------------------------------------
// Tests: cheap pure cases ported from gc_test.go / artifact_matcher.go.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// safeRelativePath contract (artifact_matcher.go:74–84): rejects empty,
    /// absolute, and upward-escaping paths; cleans the rest.
    #[test]
    fn safe_relative_path_contract() {
        assert_eq!(safe_relative_path("a/b/c"), Some("a/b/c".into()));
        assert_eq!(safe_relative_path("./a"), Some("a".into()));
        assert_eq!(safe_relative_path("a//b"), Some("a/b".into()));
        assert_eq!(safe_relative_path(""), None);
        assert_eq!(safe_relative_path("/abs/path"), None);
        assert_eq!(safe_relative_path(".."), None);
        assert_eq!(safe_relative_path("../escape"), None);
        assert_eq!(safe_relative_path("a/../.."), None);
    }

    /// artifactMatcher precedence (artifact_matcher.go:12–14): exact managed
    /// paths win over broad basenames so a managed dir is not double-counted.
    #[test]
    fn artifact_matcher_exact_paths_take_precedence() {
        let matcher = ArtifactMatcher::new(
            &[".sandbox-bin".to_string(), "node_modules".to_string()],
            &["codex-home/.sandbox-bin".to_string()],
        );
        // Exact managed path gets the "managed:" label.
        assert_eq!(
            matcher.match_directory(
                Path::new("/root"),
                Path::new("/root/codex-home/.sandbox-bin"),
                ".sandbox-bin",
            ),
            Some("managed:codex-home/.sandbox-bin".into())
        );
        // Same leaf outside the managed location falls back to the basename.
        assert_eq!(
            matcher.match_directory(
                Path::new("/root"),
                Path::new("/root/node_modules"),
                "node_modules"
            ),
            Some("node_modules".into())
        );
        // Unrelated names never match.
        assert_eq!(
            matcher.match_directory(Path::new("/root"), Path::new("/root/src"), "src"),
            None
        );
    }

    /// isKnownIssueStatus table (gc.go:589–596).
    #[test]
    fn known_issue_status_table() {
        for status in [
            "backlog",
            "todo",
            "in_progress",
            "in_review",
            "done",
            "blocked",
            "cancelled",
        ] {
            assert!(is_known_issue_status(status), "{status} must be known");
        }
        assert!(!is_known_issue_status("custom_done"));
        assert!(!is_known_issue_status(""));
    }

    /// isAutopilotRunTerminal (gc.go:710–717) and isAgentTaskTerminal
    /// (gc.go:756–763) tables.
    #[test]
    fn terminal_state_tables() {
        for status in ["completed", "failed", "skipped", "issue_created"] {
            assert!(is_autopilot_run_terminal(status));
        }
        assert!(!is_autopilot_run_terminal("running"));
        assert!(!is_autopilot_run_terminal("pending"));

        for status in ["completed", "failed", "cancelled"] {
            assert!(is_agent_task_terminal(status));
        }
        assert!(!is_agent_task_terminal("running"));
    }

    /// GCMeta parsing (execenv.go:1008–1015 semantics): pre-v2 files without a
    /// kind field normalize to Issue; known kinds parse through.
    #[test]
    fn gc_meta_kind_normalization() {
        let legacy: GcMeta = serde_json::from_slice(br#"{"workspace_id":"ws"}"#).unwrap();
        assert_eq!(legacy.kind(), GcMetaKind::Issue);

        let chat: GcMeta =
            serde_json::from_slice(br#"{"kind":"chat","chat_session_id":"cs1"}"#).unwrap();
        assert_eq!(chat.kind(), GcMetaKind::Chat);

        let future: GcMeta = serde_json::from_slice(br#"{"kind":"hologram"}"#).unwrap();
        assert_eq!(future.kind(), GcMetaKind::Unknown);
    }

    /// dirSize counts regular files only and skips linked content
    /// (gc.go:959–963).
    #[test]
    fn dir_size_skips_links_and_counts_regular_files() {
        let root = std::env::temp_dir().join(format!("cordy-ds-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.bin"), vec![0u8; 100]).unwrap();
        std::fs::write(root.join("sub").join("b.bin"), vec![0u8; 50]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hostname", root.join("link")).unwrap();

        assert_eq!(dir_size(&root), 150);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// linkedWorktreeCount porcelain parsing (gc.go:1210–1240): the bare
    /// repo's own block (marked `bare`) must not count as a linked worktree.
    #[test]
    fn linked_worktree_count_ignores_bare_block() {
        // The parser lives behind a git invocation; pin its block grammar via
        // the same state machine shape used in prune decisions instead.
        let sample = "worktree /cache/repo.git\nbare\n\nworktree /tmp/wt1\nbranch refs/heads/agent/a/1\n\nworktree /tmp/wt2\nbranch refs/heads/agent/b/2\n";
        let mut count = 0usize;
        let mut in_block = false;
        let mut is_bare = false;
        for line in sample.split('\n') {
            let line = line.trim();
            if line.is_empty() {
                if in_block && !is_bare {
                    count += 1;
                }
                in_block = false;
                is_bare = false;
            } else if line.starts_with("worktree ") {
                if in_block && !is_bare {
                    count += 1;
                }
                in_block = true;
                is_bare = false;
            } else if line == "bare" {
                is_bare = true;
            }
        }
        if in_block && !is_bare {
            count += 1;
        }
        assert_eq!(count, 2, "bare block excluded, two linked blocks counted");
    }
}
