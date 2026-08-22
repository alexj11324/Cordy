//! Port of `server/internal/daemon/health.go` (lines 1–406).
//!
//! The daemon's localhost-only health surface: `/health` (liveness/readiness
//! split plus operator diagnostics), `/shutdown` (graceful stop without OS
//! signals), and `/repo/checkout` (token-bound worktree checkout for the
//! active task).
//!
//! Deviations from Go:
//! - Go binds a `net/http` server; this crate has no HTTP-server dependency,
//!   so the handlers are pure functions over request inputs returning an
//!   [`HandlerReply`] (status + headers + body). Binding them to a listener is
//!   S9-integration work once the HTTP stack lands. All business logic —
//!   auth, validation, status codes, headers, JSON shapes — lives here.
//! - `*Daemon` fields/methods → [`HealthHost`] trait seam (same pattern as
//!   gc.rs's GcHost); integration wires it to the Daemon struct.
//! - `d.repoCache.CreateWorktreeContext` → [`HealthHost::create_worktree`]
//!   seam; production forwards to `repocache::Cache::create_worktree_ctx`.
//! - `runtime.GOOS` → wire-compatible mapping (`macos` → `"darwin"`); the
//!   desktop app compares this against its own host OS (#3916).
//! - `ErrRepoNotConfigured` (daemon.go:36–39, lane B) → local sentinel with
//!   the same message and an `errors.Is`-style chain check.
//! - A cancelled request writes nothing in Go (client sees EOF); modelled as
//!   [`HandlerReply::CANCELLED`] status 0.

// S9-integration: consumed by daemon HTTP wiring that lands with
// integration; silence dead-code until then.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::repocache::{self, Ctx, WorktreeParams, WorktreeResult};

/// `repoCheckoutModeIsolated` (daemon.go:72).
pub(crate) const REPO_CHECKOUT_MODE_ISOLATED: &str = "isolated";

/// `repoCheckoutLockWaitTimeout` (health.go:180).
pub(crate) const REPO_CHECKOUT_LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
/// `repoCheckoutRetryAfter` (health.go:181).
pub(crate) const REPO_CHECKOUT_RETRY_AFTER: Duration = Duration::from_secs(2);
/// `repoCheckoutRetryHeader` (health.go:182).
pub(crate) const REPO_CHECKOUT_RETRY_HEADER: &str = "X-Cordy-Retryable";
/// `repoCheckoutRetryValueBusy` (health.go:183).
pub(crate) const REPO_CHECKOUT_RETRY_VALUE_BUSY: &str = "repo-busy";

// ---------------------------------------------------------------------------
// Response types (health.go:20–84).
// ---------------------------------------------------------------------------

/// `HealthResponse` (health.go:20–79).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct HealthResponse {
    #[serde(rename = "status")]
    pub status: String,
    #[serde(rename = "pid")]
    pub pid: i32,
    /// runtime.GOOS of the daemon host; lets the desktop detect a daemon it
    /// cannot manage (e.g. Linux-in-WSL behind Windows, #3916).
    #[serde(rename = "os")]
    pub os: String,
    #[serde(rename = "uptime")]
    pub uptime: String,
    /// Deliberately NOT omitempty: "" means "default profile's daemon" and
    /// must stay distinguishable from a pre-#6694 daemon that cannot identify
    /// itself at all.
    #[serde(rename = "profile")]
    pub profile: String,
    #[serde(rename = "daemon_id")]
    pub daemon_id: String,
    #[serde(rename = "device_name")]
    pub device_name: String,
    #[serde(rename = "server_url")]
    pub server_url: String,
    #[serde(rename = "cli_version")]
    pub cli_version: String,
    /// "desktop" when the Electron app spawned this daemon, empty otherwise.
    #[serde(
        rename = "launched_by",
        skip_serializing_if = "String::is_empty"
    )]
    pub launched_by: String,
    #[serde(rename = "active_task_count")]
    pub active_task_count: i64,
    #[serde(rename = "running_task_count")]
    pub running_task_count: i64,
    #[serde(rename = "resource_wait_task_count")]
    pub resource_wait_task_count: i64,
    #[serde(
        rename = "repo_maintenance_active",
        skip_serializing_if = "is_zero_i32"
    )]
    pub repo_maintenance_active: i32,
    #[serde(
        rename = "repo_checkout_waiters",
        skip_serializing_if = "is_zero_i32"
    )]
    pub repo_checkout_waiters: i32,
    #[serde(rename = "agents")]
    pub agents: Vec<String>,
    /// Provider discovered locally but dropped by the last registration round
    /// → reason (MUL-5439). Omitted when empty.
    #[serde(
        rename = "skipped_agents",
        skip_serializing_if = "HashMap::is_empty"
    )]
    pub skipped_agents: HashMap<String, String>,
    /// Why a confirmed cordy version change hasn't restarted yet. Omitted when
    /// empty so older consumers see no change.
    #[serde(
        rename = "reload_pending_reason",
        skip_serializing_if = "String::is_empty"
    )]
    pub reload_pending_reason: String,
    #[serde(rename = "workspaces")]
    pub workspaces: Vec<HealthWorkspace>,
}

fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}

/// `healthWorkspace` (health.go:81–84).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct HealthWorkspace {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "runtimes")]
    pub runtimes: Vec<String>,
}

/// `repoCheckoutRequest` (health.go:98–109): body of POST /repo/checkout.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(crate) struct RepoCheckoutRequest {
    #[serde(default, rename = "url")]
    pub url: String,
    #[serde(default, rename = "workspace_id")]
    pub workspace_id: String,
    #[serde(default, rename = "workdir")]
    pub work_dir: String,
    #[serde(default, rename = "ref")]
    pub reference: String,
    #[serde(default, rename = "agent_name")]
    pub agent_name: String,
    #[serde(default, rename = "task_id")]
    pub task_id: String,
    #[serde(default, rename = "checkout_mode")]
    pub checkout_mode: String,
    /// Sent by clients that understand 503 + Retry-After.
    #[serde(default, rename = "retry_busy")]
    pub retry_busy: bool,
}

/// `activeRepoCheckoutTask` (health.go:111–117).
#[derive(Debug, Clone, Default)]
pub(crate) struct ActiveRepoCheckoutTask {
    pub workspace_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub work_dir: String,
}

// ---------------------------------------------------------------------------
// Token-bound active-task registry (health.go:119–153).
// ---------------------------------------------------------------------------

/// The `d.repoCheckoutTasks` map + mutex from health.go:124–137, extracted so
/// the registry is usable without the full Daemon struct.
#[derive(Default)]
pub(crate) struct RepoCheckoutTasks {
    inner: Mutex<HashMap<String, ActiveRepoCheckoutTask>>,
}

impl RepoCheckoutTasks {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// `registerActiveRepoCheckoutTask` (health.go:124–131): binds checkout
    /// identity to the active task. Not an OS-user isolation boundary.
    pub(crate) fn register(&self, token: &str, task: ActiveRepoCheckoutTask) {
        self.inner.lock().unwrap().insert(token.to_string(), task);
    }

    /// `clearActiveRepoCheckoutTask` (health.go:133–137).
    pub(crate) fn clear(&self, token: &str) {
        self.inner.lock().unwrap().remove(token);
    }

    /// `activeRepoCheckoutTask` (health.go:139–153): resolve the Bearer token
    /// from an Authorization header value to its bound task.
    pub(crate) fn lookup_bearer(&self, authorization_header: &str) -> Option<ActiveRepoCheckoutTask> {
        const BEARER: &str = "Bearer ";
        let header = authorization_header.trim();
        let token = header.strip_prefix(BEARER)?.trim();
        if token.is_empty() {
            return None;
        }
        self.inner.lock().unwrap().get(token).cloned()
    }
}

// ---------------------------------------------------------------------------
// Workdir authorization (health.go:155–177).
// ---------------------------------------------------------------------------

/// `filepath.IsLocal` (unix semantics): non-empty, not absolute, no `..`
/// escape. Windows volume-name cases are unreachable on unix targets.
fn filepath_is_local(rel: &str) -> bool {
    if rel.is_empty() || rel == ".." || rel.starts_with("../") || rel.starts_with('/') {
        return false;
    }
    true
}

/// `authorizeRepoCheckoutWorkDir` (health.go:155–177): both sides are
/// absolutized and symlink-resolved before the containment check, so a
/// symlinked path cannot smuggle a directory outside the active task root.
pub(crate) fn authorize_repo_checkout_work_dir(
    active_root: &str,
    requested: &str,
) -> anyhow::Result<String> {
    use std::path::Path;

    let abs = |p: &str| -> anyhow::Result<std::path::PathBuf> {
        let p = Path::new(p);
        Ok(if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir()?.join(p)
        })
    };
    let eval = |p: std::path::PathBuf| -> anyhow::Result<std::path::PathBuf> {
        std::fs::canonicalize(&p).map_err(|e| anyhow::Error::new(e).context("evalsymlinks"))
    };

    let root = eval(abs(active_root)?)?;
    let workdir = eval(abs(requested)?)?;
    let rel = match workdir.strip_prefix(&root) {
        // filepath.Rel(root, root) == "." in Go.
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => String::new(),
    };
    if !filepath_is_local(&rel) {
        anyhow::bail!("workdir is outside the active task workdir");
    }
    Ok(workdir.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Host seam (the *Daemon surface health.go touches).
// ---------------------------------------------------------------------------

/// Daemon operations the health handlers need. Integration wires this to the
/// Daemon struct; unit tests supply fakes.
///
/// `#[async_trait]` for object safety: handlers take `&dyn HealthHost`
/// (gc.rs's GcHost instead stays static-generic via RPITIT).
#[async_trait::async_trait]
pub(crate) trait HealthHost: Send + Sync {
    // --- identity (Config fields) ---
    fn profile(&self) -> &str;
    fn launched_by(&self) -> &str;
    fn daemon_id(&self) -> &str;
    fn device_name(&self) -> &str;
    fn server_base_url(&self) -> &str;
    fn cli_version(&self) -> &str;

    // --- live state ---
    /// Snapshot of `{id: runtimeIDs}` under d.mu (health.go:190–198).
    fn workspaces_snapshot(&self) -> Vec<HealthWorkspace>;
    /// Names of `d.agents()` (health.go:200–203).
    fn agent_names(&self) -> Vec<String>;
    /// `d.ready.Load()` (health.go:212).
    fn is_ready(&self) -> bool;
    fn pid(&self) -> i32;
    fn active_task_count(&self) -> i64;
    fn running_task_count(&self) -> i64;
    fn resource_wait_task_count(&self) -> i64;
    /// `d.skippedAgentsSnapshot()`.
    fn skipped_agents_snapshot(&self) -> HashMap<String, String>;
    /// `d.reloadPending()`.
    fn reload_pending(&self) -> String;
    /// The `d.repoCache.(interface{ Activity() })` type assertion
    /// (health.go:236–240); None mirrors a daemon without a repo cache.
    fn repo_cache_activity(&self) -> Option<repocache::Activity>;

    // --- repo checkout collaborators ---
    /// `d.ensureRepoReady(ctx, workspaceID, url)` (health.go:346).
    async fn ensure_repo_ready(
        &self,
        ctx: &Ctx,
        workspace_id: &str,
        url: &str,
    ) -> anyhow::Result<()>;
    /// `d.taskRepoDefaultRef(workspaceID, taskID, url)` (health.go:362).
    fn task_repo_default_ref(&self, workspace_id: &str, task_id: &str, url: &str) -> String;
    /// `d.workspaceCoAuthoredByEnabled(workspaceID)` (health.go:372).
    fn workspace_co_authored_by_enabled(&self, workspace_id: &str) -> bool;
    /// `d.repoCache.CreateWorktreeContext(...)` (health.go:380–386).
    /// S9-integration: production forwards to repocache::Cache.
    async fn create_worktree(
        &self,
        ctx: &Ctx,
        params: WorktreeParams,
    ) -> anyhow::Result<WorktreeResult>;
}

// ---------------------------------------------------------------------------
// Handler plumbing: replies instead of http.ResponseWriter.
// ---------------------------------------------------------------------------

/// What a handler would write over the wire. Status 0 means Go wrote nothing
/// because the request context was cancelled (client sees EOF).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HandlerReply {
    pub status: u16,
    pub content_type: Option<&'static str>,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HandlerReply {
    fn json(status: u16, body: String) -> Self {
        Self {
            status,
            content_type: Some("application/json"),
            headers: Vec::new(),
            body,
        }
    }

    /// `http.Error(w, msg, code)` — plain-text body + trailing newline.
    fn error(status: u16, msg: &str) -> Self {
        Self {
            status,
            content_type: None,
            headers: Vec::new(),
            body: format!("{msg}\n"),
        }
    }

    /// No response written (request cancelled mid-flight).
    pub(crate) fn is_cancelled(&self) -> bool {
        self.status == Self::CANCELLED
    }

    pub(crate) const CANCELLED: u16 = 0;
}

/// `runtime.GOOS` wire values (std::env::consts::OS differs on macOS).
pub(crate) fn goos() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

/// `time.Duration.Truncate(time.Second).String()` for non-negative durations:
/// hours accumulate past 24 ("72h3m0s"), zero-suppressed lower units.
pub(crate) fn go_duration_string(total_seconds: i64) -> String {
    let total_seconds = total_seconds.max(0);
    let h = total_seconds / 3600;
    let m = (total_seconds % 3600) / 60;
    let s = total_seconds % 60;
    if h > 0 {
        format!("{h}h{m}m{s}s")
    } else if m > 0 {
        format!("{m}m{s}s")
    } else {
        format!("{s}s")
    }
}

// ---------------------------------------------------------------------------
// /health (health.go:188–245).
// ---------------------------------------------------------------------------

/// `healthHandler` body (health.go:188–245): build the response payload.
///
/// Status is "starting" until preflight completes, then "running" — callers
/// gate readiness on this field, never on endpoint reachability.
pub(crate) fn build_health_response(host: &dyn HealthHost, started_at: DateTime<Utc>) -> HealthResponse {
    let ws_list = host.workspaces_snapshot();
    let agents = host.agent_names();

    let status = if host.is_ready() { "running" } else { "starting" };
    let uptime_seconds = Utc::now()
        .signed_duration_since(started_at)
        .num_seconds();

    let mut resp = HealthResponse {
        status: status.to_string(),
        pid: host.pid(),
        os: goos().to_string(),
        uptime: go_duration_string(uptime_seconds),
        profile: host.profile().to_string(),
        launched_by: host.launched_by().to_string(),
        daemon_id: host.daemon_id().to_string(),
        device_name: host.device_name().to_string(),
        server_url: host.server_base_url().to_string(),
        cli_version: host.cli_version().to_string(),
        active_task_count: host.active_task_count(),
        running_task_count: host.running_task_count(),
        resource_wait_task_count: host.resource_wait_task_count(),
        repo_maintenance_active: 0,
        repo_checkout_waiters: 0,
        agents,
        skipped_agents: host.skipped_agents_snapshot(),
        reload_pending_reason: host.reload_pending(),
        workspaces: ws_list,
    };
    if let Some(activity) = host.repo_cache_activity() {
        resp.repo_maintenance_active = activity.maintenance_active;
        resp.repo_checkout_waiters = activity.foreground_waiters;
    }
    resp
}

/// Serialize the /health payload exactly as the handler would.
pub(crate) fn handle_health(host: &dyn HealthHost, started_at: DateTime<Utc>) -> HandlerReply {
    let resp = build_health_response(host, started_at);
    match serde_json::to_string(&resp) {
        Ok(body) => HandlerReply::json(200, body),
        Err(err) => HandlerReply::error(500, &err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// /shutdown (health.go:253–267).
// ---------------------------------------------------------------------------

/// `shutdownHandler` outcome: the caller writes the reply, then cancels the
/// daemon context asynchronously (Go spawns `go d.cancelFunc()` so the
/// response flushes first).
pub(crate) enum ShutdownOutcome {
    /// 200 + `{"status":"shutting down"}`; caller cancels after flushing.
    ShuttingDown(String),
    /// 405 for any non-POST method; no cancellation.
    MethodNotAllowed,
}

/// `shutdownHandler` (health.go:253–267).
pub(crate) fn handle_shutdown(method: &str) -> ShutdownOutcome {
    if method != "POST" {
        return ShutdownOutcome::MethodNotAllowed;
    }
    ShutdownOutcome::ShuttingDown("{\"status\":\"shutting down\"}".to_string())
}

// ---------------------------------------------------------------------------
// /repo/checkout (health.go:290–406).
// ---------------------------------------------------------------------------

/// Local stand-in for `ErrRepoNotConfigured` (daemon.go:36–39).
#[derive(Debug, thiserror::Error)]
#[error("repo is not configured for this workspace")]
pub(crate) struct RepoNotConfiguredError;

/// `errors.Is(err, ErrRepoNotConfigured)`.
pub(crate) fn is_repo_not_configured(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|c| c.downcast_ref::<RepoNotConfiguredError>().is_some())
}

/// `repoCheckoutHandler` (health.go:290–406).
pub(crate) async fn handle_repo_checkout(
    host: &dyn HealthHost,
    tasks: &RepoCheckoutTasks,
    method: &str,
    authorization_header: &str,
    body: &[u8],
    ctx: &Ctx,
) -> HandlerReply {
    if method != "POST" {
        return HandlerReply::error(405, "method not allowed");
    }
    let Some(active_task) = tasks.lookup_bearer(authorization_header) else {
        return HandlerReply::error(401, "repo checkout requires an active task credential");
    };

    let mut req: RepoCheckoutRequest = match serde_json::from_slice(body) {
        Ok(req) => req,
        Err(err) => {
            return HandlerReply::error(400, &format!("invalid request body: {err}"));
        }
    };
    req.url = req.url.trim().to_string();
    if req.url.is_empty() {
        return HandlerReply::error(400, "url is required");
    }
    if req.workspace_id.is_empty() {
        return HandlerReply::error(400, "workspace_id is required");
    }
    if req.work_dir.is_empty() {
        return HandlerReply::error(400, "workdir is required");
    }
    if !req.checkout_mode.is_empty() && req.checkout_mode != REPO_CHECKOUT_MODE_ISOLATED {
        return HandlerReply::error(400, "invalid checkout_mode");
    }
    if req.workspace_id != active_task.workspace_id || req.task_id != active_task.task_id {
        return HandlerReply::error(
            403,
            "repo checkout task context does not match the active task",
        );
    }
    let authorized_work_dir =
        match authorize_repo_checkout_work_dir(&active_task.work_dir, &req.work_dir) {
            Ok(dir) => dir,
            Err(_) => {
                return HandlerReply::error(
                    403,
                    "repo checkout workdir is not owned by the active task",
                );
            }
        };
    // Identity is derived from the token-bound active task; caller-supplied
    // fields are compatibility inputs only and never decide branch ownership.
    req.workspace_id = active_task.workspace_id.clone();
    req.task_id = active_task.task_id.clone();
    req.agent_name = active_task.agent_name.clone();
    req.work_dir = authorized_work_dir;

    if let Err(err) = host.ensure_repo_ready(ctx, &req.workspace_id, &req.url).await {
        if ctx.err().is_some() {
            tracing::debug!(
                url = %req.url,
                error = %err,
                "repo checkout readiness cancelled"
            );
            return HandlerReply {
                status: HandlerReply::CANCELLED,
                content_type: None,
                headers: Vec::new(),
                body: String::new(),
            };
        }
        let status = if is_repo_not_configured(&err) { 400 } else { 500 };
        tracing::error!(
            workspace_id = %req.workspace_id,
            url = %req.url,
            error = %err,
            "repo checkout readiness failed"
        );
        return HandlerReply::error(status, &err.to_string());
    }

    let mut checkout_ref = req.reference.trim().to_string();
    if checkout_ref.is_empty() {
        checkout_ref = host.task_repo_default_ref(&req.workspace_id, &req.task_id, &req.url);
    }

    let params = WorktreeParams {
        workspace_id: req.workspace_id.clone(),
        repo_url: req.url.clone(),
        work_dir: req.work_dir.clone().into(),
        reference: checkout_ref,
        agent_name: req.agent_name.clone(),
        task_id: req.task_id.clone(),
        co_authored_by_enabled: host.workspace_co_authored_by_enabled(&req.workspace_id),
        lock_wait_timeout: if req.retry_busy {
            REPO_CHECKOUT_LOCK_WAIT_TIMEOUT
        } else {
            Duration::ZERO
        },
        isolated_git_metadata: req.checkout_mode == REPO_CHECKOUT_MODE_ISOLATED,
    };
    let result = match host.create_worktree(ctx, params).await {
        Ok(result) => result,
        Err(err) => {
            if repocache::is_repo_busy(&err) && req.retry_busy {
                return HandlerReply {
                    status: 503,
                    content_type: None,
                    headers: vec![
                        (
                            REPO_CHECKOUT_RETRY_HEADER.to_string(),
                            REPO_CHECKOUT_RETRY_VALUE_BUSY.to_string(),
                        ),
                        (
                            "Retry-After".to_string(),
                            format!("{:.0}", REPO_CHECKOUT_RETRY_AFTER.as_secs_f64()),
                        ),
                    ],
                    body: "repository is busy with another operation; retry later\n".to_string(),
                };
            }
            if ctx.err().is_some() {
                tracing::debug!(url = %req.url, error = %err, "repo checkout cancelled");
                return HandlerReply {
                    status: HandlerReply::CANCELLED,
                    content_type: None,
                    headers: Vec::new(),
                    body: String::new(),
                };
            }
            tracing::error!(url = %req.url, error = %err, "repo checkout failed");
            return HandlerReply::error(500, &err.to_string());
        }
    };

    match serde_json::to_string(&result) {
        Ok(body) => HandlerReply::json(200, body),
        Err(err) => HandlerReply::error(500, &err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Tests (health_test.go pure-logic cases).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    use std::sync::Arc;

    /// Fake host mirroring the Go tests' hand-built `&Daemon{...}` fixtures.
    struct FakeHost {
        cli_version: String,
        ready: AtomicBool,
        active_tasks: AtomicI64,
        running_tasks: AtomicI64,
        resource_wait_tasks: AtomicI64,
        reload_pending: Mutex<String>,
        activity: Option<repocache::Activity>,
        default_ref: String,
        ensure_err: Option<anyhow::Error>,
        create_busy: bool,
        created: Mutex<Vec<WorktreeParams>>,
    }

    impl FakeHost {
        fn new() -> Self {
            Self {
                cli_version: "v1.0.0".into(),
                ready: AtomicBool::new(false),
                active_tasks: AtomicI64::new(0),
                running_tasks: AtomicI64::new(0),
                resource_wait_tasks: AtomicI64::new(0),
                reload_pending: Mutex::new(String::new()),
                activity: None,
                default_ref: String::new(),
                ensure_err: None,
                create_busy: false,
                created: Mutex::new(Vec::new()),
            }
        }

        fn last_created(&self) -> Option<WorktreeParams> {
            self.created.lock().unwrap().last().cloned()
        }
    }

    #[async_trait::async_trait]
    impl HealthHost for FakeHost {
        fn profile(&self) -> &str {
            ""
        }
        fn launched_by(&self) -> &str {
            ""
        }
        fn daemon_id(&self) -> &str {
            "daemon-test"
        }
        fn device_name(&self) -> &str {
            "dev"
        }
        fn server_base_url(&self) -> &str {
            "http://localhost:8080"
        }
        fn cli_version(&self) -> &str {
            &self.cli_version
        }
        fn workspaces_snapshot(&self) -> Vec<HealthWorkspace> {
            Vec::new()
        }
        fn agent_names(&self) -> Vec<String> {
            Vec::new()
        }
        fn is_ready(&self) -> bool {
            self.ready.load(Ordering::SeqCst)
        }
        fn pid(&self) -> i32 {
            1234
        }
        fn active_task_count(&self) -> i64 {
            self.active_tasks.load(Ordering::SeqCst)
        }
        fn running_task_count(&self) -> i64 {
            self.running_tasks.load(Ordering::SeqCst)
        }
        fn resource_wait_task_count(&self) -> i64 {
            self.resource_wait_tasks.load(Ordering::SeqCst)
        }
        fn skipped_agents_snapshot(&self) -> HashMap<String, String> {
            HashMap::new()
        }
        fn reload_pending(&self) -> String {
            self.reload_pending.lock().unwrap().clone()
        }
        fn repo_cache_activity(&self) -> Option<repocache::Activity> {
            self.activity
        }
        async fn ensure_repo_ready(&self, _ctx: &Ctx, _ws: &str, _url: &str) -> anyhow::Result<()> {
            match &self.ensure_err {
                Some(err) => Err(anyhow::anyhow!("{err:#}")),
                None => Ok(()),
            }
        }
        fn task_repo_default_ref(&self, _ws: &str, _task: &str, _url: &str) -> String {
            self.default_ref.clone()
        }
        fn workspace_co_authored_by_enabled(&self, _ws: &str) -> bool {
            false
        }
        async fn create_worktree(
            &self,
            _ctx: &Ctx,
            params: WorktreeParams,
        ) -> anyhow::Result<WorktreeResult> {
            self.created.lock().unwrap().push(params.clone());
            if self.create_busy {
                // Preserve the sentinel type so is_repo_busy sees it.
                return Err(repocache::err_repo_busy());
            }
            Ok(WorktreeResult {
                path: "/cache/org/repo.git".into(),
                branch_name: "cordy/task-1".into(),
            })
        }
    }

    fn started_at() -> DateTime<Utc> {
        Utc::now() - chrono::Duration::seconds(90)
    }

    /// TestHealthHandlerReportsCLIVersionAndTaskCounts (health_test.go:19–95).
    #[test]
    fn health_reports_cli_version_and_task_counts() {
        let mut host = FakeHost::new();
        host.cli_version = "v9.9.9".into();
        host.active_tasks.store(2, Ordering::SeqCst);
        host.running_tasks.store(1, Ordering::SeqCst);
        host.resource_wait_tasks.store(1, Ordering::SeqCst);
        host.ready.store(true, Ordering::SeqCst);

        let reply = handle_health(&host, started_at());
        assert_eq!(reply.status, 200);

        let raw: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(raw["cli_version"], "v9.9.9");
        assert_eq!(raw["active_task_count"], 2);
        assert_eq!(raw["running_task_count"], 1);
        assert_eq!(raw["resource_wait_task_count"], 1);
        assert_eq!(raw["status"], "running");
        // The desktop relies on the `os` key to detect an unmanageable daemon
        // (#3916) — lock both key and value.
        assert_eq!(raw["os"], goos());
    }

    /// TestHealthHandlerReportsDeferredReload (health_test.go:102–140).
    #[test]
    fn health_reports_deferred_reload() {
        let mut host = FakeHost::new();
        host.ready.store(true, Ordering::SeqCst);

        // Absent when nothing pending.
        let raw: serde_json::Value =
            serde_json::from_str(&handle_health(&host, started_at()).body).unwrap();
        assert!(
            raw.get("reload_pending_reason").is_none(),
            "reload_pending_reason must be omitted when no restart is pending"
        );

        // Explains a deferred restart.
        *host.reload_pending.lock().unwrap() =
            "cordy binary on disk reports 0.3.8, running 0.3.7".into();
        let raw: serde_json::Value =
            serde_json::from_str(&handle_health(&host, started_at()).body).unwrap();
        let got = raw["reload_pending_reason"].as_str().unwrap_or_default();
        assert!(got.contains("0.3.8"), "reload_pending_reason = {got:?}");
    }

    /// TestHealthHandlerReportsStartingUntilReady (health_test.go:147–176).
    #[test]
    fn health_reports_starting_until_ready() {
        let host = FakeHost::new();
        let read_status = |host: &FakeHost| {
            let raw: serde_json::Value =
                serde_json::from_str(&handle_health(host, started_at()).body).unwrap();
            raw["status"].as_str().unwrap().to_string()
        };
        assert_eq!(read_status(&host), "starting");
        host.ready.store(true, Ordering::SeqCst);
        assert_eq!(read_status(&host), "running");
    }

    /// TestHealthHandlerActiveTaskCountTracksCounter (health_test.go:178–198).
    #[test]
    fn health_active_task_count_tracks_counter() {
        let host = FakeHost::new();
        let count = |host: &FakeHost| {
            let raw: serde_json::Value =
                serde_json::from_str(&handle_health(host, started_at()).body).unwrap();
            raw["active_task_count"].as_i64().unwrap()
        };
        host.active_tasks.fetch_add(1, Ordering::SeqCst);
        host.active_tasks.fetch_add(1, Ordering::SeqCst);
        assert_eq!(count(&host), 2);
        host.active_tasks.fetch_add(-1, Ordering::SeqCst);
        assert_eq!(count(&host), 1);
        host.active_tasks.fetch_add(-1, Ordering::SeqCst);
        assert_eq!(count(&host), 0);
    }

    /// TestHealthHandlerReportsRepoCoordinationActivity (health_test.go:200–222).
    #[test]
    fn health_reports_repo_coordination_activity() {
        let mut host = FakeHost::new();
        host.activity = Some(repocache::Activity {
            maintenance_active: 1,
            foreground_waiters: 3,
        });
        let raw: serde_json::Value =
            serde_json::from_str(&handle_health(&host, started_at()).body).unwrap();
        assert_eq!(raw["repo_maintenance_active"], 1);
        assert_eq!(raw["repo_checkout_waiters"], 3);
    }

    /// TestShutdownHandlerPostCancelsDaemonContext +
    /// TestShutdownHandlerRejectsNonPost (health_test.go:224–265).
    #[test]
    fn shutdown_post_cancels_and_rejects_non_post() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let outcome = handle_shutdown("POST");
        match outcome {
            ShutdownOutcome::ShuttingDown(body) => {
                assert!(body.contains("shutting down"));
                // Caller cancels after flushing the response.
                cancelled.store(true, Ordering::SeqCst);
            }
            ShutdownOutcome::MethodNotAllowed => panic!("POST must shut down"),
        }
        assert!(cancelled.load(Ordering::SeqCst));

        match handle_shutdown("GET") {
            ShutdownOutcome::MethodNotAllowed => {}
            ShutdownOutcome::ShuttingDown(_) => panic!("GET must not trigger shutdown"),
        }
    }

    // ---- /repo/checkout ---------------------------------------------------

    fn setup_checkout(
        host: &FakeHost,
        tasks: &RepoCheckoutTasks,
        work_dir: &std::path::Path,
    ) -> String {
        let token = "test-token";
        tasks.register(
            token,
            ActiveRepoCheckoutTask {
                workspace_id: "ws-checkout".into(),
                task_id: "task-1".into(),
                agent_id: "agent-1".into(),
                agent_name: "Test Agent".into(),
                work_dir: work_dir.to_string_lossy().into_owned(),
            },
        );
        format!("Bearer {token}")
    }

    fn checkout_body(work_dir: &std::path::Path, extra: &str) -> Vec<u8> {
        format!(
            r#"{{"url":"https://github.com/org/repo.git","workspace_id":"ws-checkout","workdir":"{}","task_id":"task-1"{}}}"#,
            work_dir.display(),
            extra
        )
        .into_bytes()
    }

    /// TestRepoCheckoutUsesTaskScopedProjectRefByDefault
    /// (health_test.go:317–340).
    #[tokio::test]
    async fn repo_checkout_uses_task_scoped_project_ref_by_default() {
        let mut host = FakeHost::new();
        host.default_ref = "release/v2".into();
        let tasks = RepoCheckoutTasks::new();
        let work_dir = tempfile::tempdir().unwrap();
        let auth = setup_checkout(&host, &tasks, work_dir.path());

        let reply = handle_repo_checkout(
            &host,
            &tasks,
            "POST",
            &auth,
            &checkout_body(work_dir.path(), r#","agent_name":"Other Agent""#),
            &Ctx::new(),
        )
        .await;
        assert_eq!(reply.status, 200, "{}", reply.body);
        let created = host.last_created().unwrap();
        assert_eq!(created.reference, "release/v2");
        assert_eq!(created.agent_name, "Test Agent", "token-bound active agent wins");
    }

    /// TestRepoCheckoutRejectsMissingTaskCredential (health_test.go:342–361).
    #[tokio::test]
    async fn repo_checkout_rejects_missing_task_credential() {
        let host = FakeHost::new();
        let tasks = RepoCheckoutTasks::new();
        let work_dir = tempfile::tempdir().unwrap();

        let reply = handle_repo_checkout(
            &host,
            &tasks,
            "POST",
            "",
            &checkout_body(work_dir.path(), ""),
            &Ctx::new(),
        )
        .await;
        assert_eq!(reply.status, 401);
        assert!(host.last_created().is_none(), "unauthorized checkout reached repo cache");
    }

    /// TestRepoCheckoutRejectsAnotherTaskWorkdir (health_test.go:363–383).
    #[tokio::test]
    async fn repo_checkout_rejects_another_task_workdir() {
        let host = FakeHost::new();
        let tasks = RepoCheckoutTasks::new();
        let work_dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let auth = setup_checkout(&host, &tasks, work_dir.path());

        let reply = handle_repo_checkout(
            &host,
            &tasks,
            "POST",
            &auth,
            &checkout_body(other.path(), ""),
            &Ctx::new(),
        )
        .await;
        assert_eq!(reply.status, 403);
        assert!(host.last_created().is_none(), "cross-task workdir reached repo cache");
    }

    /// TestRepoCheckoutExplicitRefOverridesProjectDefault
    /// (health_test.go:385–405).
    #[tokio::test]
    async fn repo_checkout_explicit_ref_overrides_project_default() {
        let mut host = FakeHost::new();
        host.default_ref = "release/v2".into();
        let tasks = RepoCheckoutTasks::new();
        let work_dir = tempfile::tempdir().unwrap();
        let auth = setup_checkout(&host, &tasks, work_dir.path());

        let reply = handle_repo_checkout(
            &host,
            &tasks,
            "POST",
            &auth,
            &checkout_body(work_dir.path(), r#","ref":"hotfix""#),
            &Ctx::new(),
        )
        .await;
        assert_eq!(reply.status, 200, "{}", reply.body);
        assert_eq!(host.last_created().unwrap().reference, "hotfix");
    }

    /// TestRepoCheckoutForwardsIsolatedMode (health_test.go:407–426).
    #[tokio::test]
    async fn repo_checkout_forwards_isolated_mode() {
        let host = FakeHost::new();
        let tasks = RepoCheckoutTasks::new();
        let work_dir = tempfile::tempdir().unwrap();
        let auth = setup_checkout(&host, &tasks, work_dir.path());

        let reply = handle_repo_checkout(
            &host,
            &tasks,
            "POST",
            &auth,
            &checkout_body(work_dir.path(), r#","checkout_mode":"isolated""#),
            &Ctx::new(),
        )
        .await;
        assert_eq!(reply.status, 200, "{}", reply.body);
        assert!(host.last_created().unwrap().isolated_git_metadata);
    }

    /// TestRepoCheckoutRejectsUnknownMode (health_test.go:428–447).
    #[tokio::test]
    async fn repo_checkout_rejects_unknown_mode() {
        let host = FakeHost::new();
        let tasks = RepoCheckoutTasks::new();
        let work_dir = tempfile::tempdir().unwrap();
        let auth = setup_checkout(&host, &tasks, work_dir.path());

        let reply = handle_repo_checkout(
            &host,
            &tasks,
            "POST",
            &auth,
            &checkout_body(work_dir.path(), r#","checkout_mode":"unsafe""#),
            &Ctx::new(),
        )
        .await;
        assert_eq!(reply.status, 400);
        assert!(host.last_created().is_none(), "invalid mode reached repo cache");
    }

    /// TestRepoCheckoutReturnsRetryableBusyToCapableClient
    /// (health_test.go:449–474).
    #[tokio::test]
    async fn repo_checkout_returns_retryable_busy_to_capable_client() {
        let mut host = FakeHost::new();
        host.create_busy = true;
        let tasks = RepoCheckoutTasks::new();
        let work_dir = tempfile::tempdir().unwrap();
        let auth = setup_checkout(&host, &tasks, work_dir.path());

        let reply = handle_repo_checkout(
            &host,
            &tasks,
            "POST",
            &auth,
            &checkout_body(work_dir.path(), r#","retry_busy":true"#),
            &Ctx::new(),
        )
        .await;
        assert_eq!(reply.status, 503);
        let header = |name: &str| {
            reply
                .headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(header("Retry-After"), "2");
        assert_eq!(header(REPO_CHECKOUT_RETRY_HEADER), REPO_CHECKOUT_RETRY_VALUE_BUSY);
        assert_eq!(
            host.last_created().unwrap().lock_wait_timeout,
            REPO_CHECKOUT_LOCK_WAIT_TIMEOUT
        );
    }

    /// authorizeRepoCheckoutWorkDir containment (health.go:155–177).
    #[test]
    #[cfg(unix)]
    fn authorize_work_dir_containment() {
        let root = tempfile::tempdir().unwrap();
        let inside = tempfile::tempdir_in(root.path()).unwrap();
        let outside = tempfile::tempdir().unwrap();

        let ok = authorize_repo_checkout_work_dir(
            root.path().to_str().unwrap(),
            inside.path().to_str().unwrap(),
        );
        assert!(ok.is_ok());

        let bad = authorize_repo_checkout_work_dir(
            root.path().to_str().unwrap(),
            outside.path().to_str().unwrap(),
        );
        assert!(bad.is_err(), "workdir outside the active root must be rejected");

        // Symlink escape: a link inside the root pointing outside resolves to
        // the outside target and must be rejected.
        let link = root.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let escaped = authorize_repo_checkout_work_dir(
            root.path().to_str().unwrap(),
            link.to_str().unwrap(),
        );
        assert!(escaped.is_err(), "symlink escape must be rejected");
    }

    /// go_duration_string matches Go's truncated Duration.String().
    #[test]
    fn duration_formatting() {
        assert_eq!(go_duration_string(0), "0s");
        assert_eq!(go_duration_string(45), "45s");
        assert_eq!(go_duration_string(65), "1m5s");
        assert_eq!(go_duration_string(3725), "1h2m5s");
        assert_eq!(go_duration_string(72 * 3600 + 180), "72h3m0s");
    }
}
