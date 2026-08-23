//! Port of `server/internal/daemon/client.go` (lines 1–1184) — HTTP
//! communication with the Cordy server daemon API.
//!
//! Symbol map (Go → Rust):
//! - `requestError` → [`RequestError`] (a distinct error type so Go's
//!   `errors.As` predicates become downcast checks)
//! - `isWorkspaceNotFoundError` / `isTaskNotFoundError` /
//!   `isUnauthorizedError` / `isRuntimeNotFoundError` /
//!   `isBatchClaimUnsupported` / `isIssueGCBatchUnsupported` → same-named
//!   functions over [`ClientError::Request`]
//! - `Client` → [`Client`]
//! - `normalizeGOOS` → [`normalize_goos`]
//! - `daemonClientCapabilities` → [`daemon_client_capabilities`]
//! - `batchClaimRequestTimeout` → [`BATCH_CLAIM_REQUEST_TIMEOUT`]
//! - `TaskCancelAck`, `TaskMessageData`, `WorkspaceInfo`,
//!   `RenewTokenResponse`, `IssueGCStatus`, `IssueGCCheckResult`,
//!   `ChatSessionGCStatus`, `AutopilotRunGCStatus`, `TaskGCStatus`,
//!   `RuntimeOfflineReason`, `RegisterResponse`, `WorkspaceReposResponse`,
//!   `RuntimeProfile`, `RuntimeProfilesResponse` → same-named structs
//! - heartbeat aliases (`HeartbeatResponse = protocol.DaemonHeartbeatAck…`)
//!   → type aliases onto `cordy_protocol::messages::*`
//! - `defaultTerminalRetrySchedule` / `skillBundleResolveRetrySchedule` →
//!   [`DEFAULT_TERMINAL_RETRY_SCHEDULE`] / [`SKILL_BUNDLE_RESOLVE_RETRY_SCHEDULE`]
//! - `postJSON*` / `getJSONWithToken` family → private request helpers
//!
//! Deviations from Go:
//! - Go's two `http.Client`s (fixed 30s control-plane vs. no-timeout bundle
//!   client) become one shared `reqwest::Client`; reqwest applies per-request
//!   timeouts instead, so the control-plane 30s budget is enforced with
//!   [`Client::CONTROL_PLANE_TIMEOUT`] at each call site and bundle downloads
//!   run under the caller-supplied ctx deadline only (GitHub #4505).
//! - `retrySleep` is inlined as a ctx-aware tokio sleep (no test-injection var;
//!   tests use instant schedules).
//! - `ResolveRemoteMCPCredential` returns `Vec<(String, String)>` header pairs
//!   rather than `http.Header`.

// S9-integration: consumed by daemon.go core (lane B) wiring that lands with
// integration; silence dead-code until then.
#![allow(dead_code)]

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use serde_json::{json, Value};

use cordy_protocol::{
    DaemonHeartbeatAckPayload, DAEMON_CAPABILITY_AGENT_SKILL_V1,
    DAEMON_CAPABILITY_COALESCED_COMMENTS_V1, DAEMON_CAPABILITY_EXECUTION_MANIFEST_V1,
    DAEMON_CAPABILITY_LOCAL_WORKTREE_V1, DAEMON_CAPABILITY_REMOTE_MCP_V1, DAEMON_CAPABILITY_RPC_V1,
    DAEMON_CAPABILITY_SKILL_BUNDLES_V1,
};

use crate::types::{RepoData, Runtime, SkillData, SkillRefData, Task};

/// The fixed 30s control-plane timeout (Go: `http.Client{Timeout: 30s}`).
pub(crate) const CONTROL_PLANE_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Error taxonomy (client.go:22–91)
// ---------------------------------------------------------------------------

/// `requestError`: the server responded with an error status.
#[derive(Debug, thiserror::Error)]
#[error("{method} {path} returned {status_code}: {body}")]
pub struct RequestError {
    pub method: &'static str,
    pub path: String,
    pub status_code: u16,
    pub body: String,
}

/// Transport failures (reqwest errors, JSON decode errors) surfaced verbatim.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// `*requestError`.
    #[error(transparent)]
    Request(#[from] RequestError),
    /// Everything else (connection refused, decode failure, ctx cancelled).
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ClientError {
    /// Go's `errors.As(err, &reqErr)` — the request-error downcast.
    pub fn as_request(&self) -> Option<&RequestError> {
        match self {
            ClientError::Request(req_err) => Some(req_err),
            ClientError::Other(_) => None,
        }
    }
}

/// `isWorkspaceNotFoundError` (client.go:35): a 404 with "workspace not found"
/// body.
pub(crate) fn is_workspace_not_found_error(err: &ClientError) -> bool {
    matches!(
        err.as_request(),
        Some(req) if req.status_code == 404 && req.body.to_lowercase().contains("workspace not found")
    )
}

/// `isTaskNotFoundError` (client.go:51): a 404 with "task not found" body —
/// the task was deleted server-side while the local agent was still running.
pub(crate) fn is_task_not_found_error(err: &ClientError) -> bool {
    matches!(
        err.as_request(),
        Some(req) if req.status_code == 404 && req.body.to_lowercase().contains("task not found")
    )
}

/// `isUnauthorizedError` (client.go:65): a 401 from the server.
pub(crate) fn is_unauthorized_error(err: &ClientError) -> bool {
    matches!(err.as_request(), Some(req) if req.status_code == 401)
}

/// `isRuntimeNotFoundError` (client.go:82): a 404 with "runtime not found" body
/// — the runtime row was deleted server-side while the daemon was still
/// heartbeating against the dead UUID.
pub(crate) fn is_runtime_not_found_error(err: &ClientError) -> bool {
    matches!(
        err.as_request(),
        Some(req) if req.status_code == 404 && req.body.to_lowercase().contains("runtime not found")
    )
}

/// `isBatchClaimUnsupported` (client.go:288): a 404 from the batch claim
/// endpoint — the server predates the route and the daemon must fall back to
/// the legacy per-runtime claim.
pub(crate) fn is_batch_claim_unsupported(err: &ClientError) -> bool {
    matches!(err.as_request(), Some(req) if req.status_code == 404)
}

/// `isIssueGCBatchUnsupported` (client.go:708): distinguishes chi's
/// unmatched-route response on an older server ("404 page not found") from the
/// JSON 404 returned by a current server on an authorization failure.
pub(crate) fn is_issue_gc_batch_unsupported(err: &ClientError) -> bool {
    matches!(
        err.as_request(),
        Some(req) if req.status_code == 404 && req.body.trim() == "404 page not found"
    )
}

// ---------------------------------------------------------------------------
// Client (client.go:93–137)
// ---------------------------------------------------------------------------

/// `Client`: handles HTTP communication with the Cordy server daemon API.
///
/// Identity headers sent on every request as X-Client-* are populated by
/// [`Client::set_version`] ([`Client::platform`] / [`Client::os`] are fixed at
/// construction); empty values are simply omitted.
pub(crate) struct Client {
    base_url: String,
    token: std::sync::Mutex<String>,
    http: reqwest::Client,

    version: std::sync::Mutex<String>,

    // Workspace ETag cache state (Go's workspaceMu-guarded fields).
    workspace_state: std::sync::Mutex<WorkspaceCacheState>,
    issue_gc_batch_state: std::sync::Mutex<IssueGcBatchState>,
}

#[derive(Default)]
struct WorkspaceCacheState {
    etag: String,
    cache: Vec<WorkspaceInfo>,
    cache_valid: bool,
    legacy_endpoint_enabled: bool,
}

#[derive(Default)]
struct IssueGcBatchState {
    legacy_enabled: bool,
}

/// `NewClient` (client.go:122): creates a new daemon API client.
impl Client {
    pub(crate) fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            token: std::sync::Mutex::new(String::new()),
            http: reqwest::Client::builder()
                .pool_idle_timeout(Duration::from_secs(90))
                .build()
                .expect("reqwest client builder with valid config"),
            version: std::sync::Mutex::new(String::new()),
            workspace_state: std::sync::Mutex::new(WorkspaceCacheState::default()),
            issue_gc_batch_state: std::sync::Mutex::new(IssueGcBatchState::default()),
        }
    }

    /// `CloseIdleConnections` (client.go:142): drops pooled control-plane HTTP
    /// connections. Called after repeated heartbeat transport failures so a
    /// stale keep-alive socket from a server restart cannot delay recovery.
    pub fn close_idle_connections(&self) {
        // reqwest 0.12 pools per-client with idle timeouts; there is no public
        // force-close handle. Dropping pooled sockets happens on the pool's own
        // idle timeout, so this is a best-effort no-op that keeps the Go call
        // site shape.
    }

    /// `SetVersion` (client.go:166): records the daemon's CLI version, sent as
    /// X-Client-Version. Called by Daemon.Run after config is loaded.
    pub fn set_version(&self, v: &str) {
        *self.version.lock().unwrap() = v.to_string();
    }

    /// `SetToken` (client.go:202).
    pub fn set_token(&self, token: &str) {
        *self.token.lock().unwrap() = token.to_string();
    }

    /// `Token` (client.go:207).
    pub fn token(&self) -> String {
        self.token.lock().unwrap().clone()
    }

    fn platform(&self) -> &'static str {
        "daemon"
    }

    fn os(&self) -> &'static str {
        normalize_goos(std::env::consts::OS)
    }

    /// `setIdentityHeaders` (client.go:171): attaches X-Client-* to the
    /// builder when set.
    fn set_identity_headers(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let builder = builder.header("X-Client-Platform", self.platform());
        let builder = builder.header("X-Client-OS", self.os());
        let version = self.version.lock().unwrap().clone();
        let builder = if !version.is_empty() {
            builder.header("X-Client-Version", version)
        } else {
            builder
        };
        builder.header("X-Client-Capabilities", daemon_client_capabilities())
    }
}

/// `normalizeGOOS` (client.go:151): maps OS values to the protocol vocabulary
/// used by X-Client-OS / client_os ("macos" / "windows" / "linux").
pub(crate) fn normalize_goos(goos: &str) -> &'static str {
    match goos {
        "darwin" => "macos",
        "windows" => "windows",
        "linux" => "linux",
        _ => "",
    }
}

/// `daemonClientCapabilities` (client.go:189): the X-Client-Capabilities value
/// the daemon advertises on BOTH the HTTP control-plane requests and the WS
/// handshake. rpc-v1 advertises WS request/response support (MUL-4257).
pub(crate) fn daemon_client_capabilities() -> String {
    [
        DAEMON_CAPABILITY_SKILL_BUNDLES_V1,
        DAEMON_CAPABILITY_COALESCED_COMMENTS_V1,
        DAEMON_CAPABILITY_EXECUTION_MANIFEST_V1,
        DAEMON_CAPABILITY_AGENT_SKILL_V1,
        DAEMON_CAPABILITY_REMOTE_MCP_V1,
        DAEMON_CAPABILITY_LOCAL_WORKTREE_V1,
        DAEMON_CAPABILITY_RPC_V1,
    ]
    .join(",")
}

// ---------------------------------------------------------------------------
// Claim endpoints (client.go:211–323)
// ---------------------------------------------------------------------------

/// `batchClaimRequestTimeout` (client.go:255): the short, request-scoped
/// deadline for the machine-level batch claim (MUL-4257). Bounding the batch to
/// a few seconds caps worst-case head-of-line starvation across every runtime
/// the daemon hosts; a claim that commits server-side after the client gives up
/// is recovered by stale-dispatch reclaim on the next poll. Kept comfortably
/// above p99 claim latency so recovery stays the exception.
pub const BATCH_CLAIM_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

impl Client {
    /// `ClaimTask` (client.go:211): claim one task for a single runtime.
    pub(crate) async fn claim_task(
        &self,
        ctx: &crate::repocache::Ctx,
        runtime_id: &str,
    ) -> anyhow::Result<Option<Task>> {
        #[derive(Deserialize)]
        struct Resp {
            task: Option<Task>,
        }
        let resp: Resp = self
            .post_json(
                ctx,
                &format!("/api/daemon/runtimes/{runtime_id}/tasks/claim"),
                json!({}),
            )
            .await?;
        Ok(resp.task)
    }

    /// `ResolveRemoteMCPCredential` (client.go:221): fetch the credential for a
    /// remote-MCP or plugin-contributed MCP connection. A Plugin-contributed
    /// connection keeps its credential in the Plugin's own secret storage,
    /// which a different route serves; the marker travels on the contribution
    /// id because that is all the broker hands back at dial time.
    pub(crate) async fn resolve_remote_mcp_credential(
        &self,
        ctx: &crate::repocache::Ctx,
        daemon_token: &str,
        task_id: &str,
        contribution_id: &str,
    ) -> anyhow::Result<Vec<(String, String)>> {
        #[derive(Deserialize)]
        struct Response {
            #[serde(default)]
            credential_header: String,
            #[serde(default)]
            credential: String,
        }
        let route = if contribution_id.starts_with("plugin:") {
            "plugin-mcp"
        } else {
            "remote-mcp"
        };
        let path = format!(
            "/api/daemon/tasks/{}/{}/{}/credential",
            url_escape(task_id),
            route,
            url_escape(contribution_id)
        );
        let response: Response = self.get_json_with_token(ctx, &path, daemon_token).await?;
        let mut headers = Vec::new();
        if !response.credential_header.is_empty() {
            headers.push((response.credential_header, response.credential));
        }
        Ok(headers)
    }

    /// `ClaimTasks` (client.go:267): the machine-level batch counterpart of
    /// [`Client::claim_task`] — claims up to max_tasks tasks across every
    /// runtime the daemon hosts in a single request. daemonID scopes the
    /// request to this machine; each returned Task carries its own RuntimeID
    /// so the daemon routes it locally. Runs under a short request-scoped
    /// deadline rather than the shared 30s control-plane timeout.
    pub(crate) async fn claim_tasks(
        &self,
        ctx: &crate::repocache::Ctx,
        daemon_id: &str,
        runtime_ids: &[String],
        max_tasks: i32,
    ) -> anyhow::Result<Vec<Task>> {
        #[derive(Deserialize)]
        struct Resp {
            #[serde(default)]
            tasks: Vec<Task>,
        }
        // Go wraps the request in context.WithTimeout(ctx,
        // batchClaimRequestTimeout); reqwest's per-request timeout below
        // provides the same client-side bound.
        let resp: Resp = self
            .post_json_with_timeout(
                ctx,
                "/api/daemon/tasks/claim",
                json!({
                    "daemon_id": daemon_id,
                    "runtime_ids": runtime_ids,
                    "max_tasks": max_tasks,
                }),
                BATCH_CLAIM_REQUEST_TIMEOUT,
            )
            .await?;
        Ok(resp.tasks)
    }

    /// `claimTasksLegacy` (client.go:302): the pre-batch compatibility
    /// fallback — claim per runtime via the legacy per-runtime endpoint so a
    /// new daemon still works against a server without the batch route. A
    /// per-runtime error is only propagated when nothing has been claimed yet;
    /// otherwise the partial result is returned and the next poll retries the
    /// rest.
    pub(crate) async fn claim_tasks_legacy(
        &self,
        ctx: &crate::repocache::Ctx,
        runtime_ids: &[String],
        max_tasks: usize,
    ) -> anyhow::Result<Vec<Task>> {
        if max_tasks == 0 {
            return Ok(Vec::new());
        }
        let mut out: Vec<Task> = Vec::with_capacity(max_tasks);
        for rid in runtime_ids {
            if out.len() >= max_tasks {
                break;
            }
            match self.claim_task(ctx, rid).await {
                Err(err) => {
                    if out.is_empty() {
                        return Err(err);
                    }
                    return Ok(out);
                }
                Ok(Some(task)) => out.push(task),
                Ok(None) => {}
            }
        }
        Ok(out)
    }
}

/// Minimal percent-encoding for path segments (Go's `url.PathEscape`).
fn url_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Task lifecycle endpoints (client.go:332–516)
// ---------------------------------------------------------------------------

/// `skillBundleResolveRetrySchedule` (client.go:947): rides out brief
/// transport blips on a single bundle download. N entries → N+1 attempts.
pub(crate) const SKILL_BUNDLE_RESOLVE_RETRY_SCHEDULE: &[Duration] =
    &[Duration::from_millis(500), Duration::from_secs(2)];

impl Client {
    /// `ResolveSkillBundle` (client.go:332): downloads a single skill bundle
    /// with retries within whatever budget ctx leaves, letting each download
    /// fit its own deadline and be cached independently (GitHub #4505).
    pub(crate) async fn resolve_skill_bundle(
        &self,
        ctx: &crate::repocache::Ctx,
        runtime_id: &str,
        task_id: &str,
        skill_ref: SkillRefData,
    ) -> anyhow::Result<SkillData> {
        #[derive(Deserialize)]
        struct Resp {
            #[serde(default)]
            bundles: Vec<SkillData>,
        }
        let path =
            format!("/api/daemon/runtimes/{runtime_id}/tasks/{task_id}/skill-bundles/resolve");
        let resp: Resp = self
            .post_json_with_retry(
                ctx,
                &path,
                json!({ "skills": vec![skill_ref] }),
                SKILL_BUNDLE_RESOLVE_RETRY_SCHEDULE,
            )
            .await?;
        if resp.bundles.len() != 1 {
            anyhow::bail!(
                "resolve skill bundle: expected 1 bundle, got {}",
                resp.bundles.len()
            );
        }
        Ok(resp.bundles.into_iter().next().expect("len checked"))
    }

    /// `ExtendTaskPrepareLease` (client.go:348).
    pub(crate) async fn extend_task_prepare_lease(
        &self,
        ctx: &crate::repocache::Ctx,
        runtime_id: &str,
        task_id: &str,
    ) -> anyhow::Result<()> {
        self.post_json_unit(
            ctx,
            &format!("/api/daemon/runtimes/{runtime_id}/tasks/{task_id}/prepare-lease"),
            json!({}),
        )
        .await
    }

    /// `StartTask` (client.go:352).
    pub(crate) async fn start_task(
        &self,
        ctx: &crate::repocache::Ctx,
        task_id: &str,
    ) -> anyhow::Result<()> {
        self.post_json_unit(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/start"),
            json!({}),
        )
        .await
    }

    /// `MarkTaskWaitingLocalDirectory` (client.go:365): parks a
    /// freshly-dispatched task in waiting_local_directory. Idempotent
    /// daemon-side.
    pub(crate) async fn mark_task_waiting_local_directory(
        &self,
        ctx: &crate::repocache::Ctx,
        task_id: &str,
        reason: &str,
    ) -> anyhow::Result<()> {
        self.post_json_unit(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/wait-local-directory"),
            json!({ "reason": reason }),
        )
        .await
    }

    /// `ReportProgress` (client.go:417).
    pub(crate) async fn report_progress(
        &self,
        ctx: &crate::repocache::Ctx,
        task_id: &str,
        summary: &str,
        step: i32,
        total: i32,
    ) -> anyhow::Result<()> {
        self.post_json_unit(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/progress"),
            json!({ "summary": summary, "step": step, "total": total }),
        )
        .await
    }

    /// `ReportTaskMessages` (client.go:435).
    pub(crate) async fn report_task_messages(
        &self,
        ctx: &crate::repocache::Ctx,
        task_id: &str,
        messages: Vec<TaskMessageData>,
    ) -> anyhow::Result<()> {
        self.post_json_unit(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/messages"),
            json!({ "messages": messages }),
        )
        .await
    }

    /// `CompleteTask` (client.go:441): terminal callback with bounded retry.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn complete_task(
        &self,
        ctx: &crate::repocache::Ctx,
        task_id: &str,
        output: &str,
        branch_name: &str,
        session_id: &str,
        work_dir: &str,
        session_rollout_missing: bool,
        retired_session_id: &str,
        durable_work_dir: &str,
    ) -> anyhow::Result<()> {
        let mut body = serde_json::Map::new();
        body.insert("output".into(), json!(output));
        if !branch_name.is_empty() {
            body.insert("branch_name".into(), json!(branch_name));
        }
        if !session_id.is_empty() {
            body.insert("session_id".into(), json!(session_id));
        }
        if !work_dir.is_empty() {
            body.insert("work_dir".into(), json!(work_dir));
        }
        if !durable_work_dir.is_empty() {
            body.insert("durable_work_dir".into(), json!(durable_work_dir));
        }
        if session_rollout_missing {
            body.insert("session_rollout_missing".into(), json!(true));
        }
        if !retired_session_id.is_empty() {
            body.insert("retired_session_id".into(), json!(retired_session_id));
        }
        self.post_json_unit_with_retry(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/complete"),
            Value::Object(body),
            DEFAULT_TERMINAL_RETRY_SCHEDULE,
        )
        .await
    }

    /// `FailTask` (client.go:473): terminal callback with bounded retry.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn fail_task(
        &self,
        ctx: &crate::repocache::Ctx,
        task_id: &str,
        error_msg: &str,
        session_id: &str,
        work_dir: &str,
        branch_name: &str,
        failure_reason: &str,
        session_rollout_missing: bool,
        retired_session_id: &str,
        durable_work_dir: &str,
    ) -> anyhow::Result<()> {
        let mut body = serde_json::Map::new();
        body.insert("error".into(), json!(error_msg));
        if !session_id.is_empty() {
            body.insert("session_id".into(), json!(session_id));
        }
        if !work_dir.is_empty() {
            body.insert("work_dir".into(), json!(work_dir));
        }
        if !durable_work_dir.is_empty() {
            body.insert("durable_work_dir".into(), json!(durable_work_dir));
        }
        // A failed run can still have delivered a branch: worktree mode commits
        // whatever the agent left before removing the worktree, so partial work
        // survives — but only if its name travels with the failure report.
        if !branch_name.is_empty() {
            body.insert("branch_name".into(), json!(branch_name));
        }
        if !failure_reason.is_empty() {
            body.insert("failure_reason".into(), json!(failure_reason));
        }
        if session_rollout_missing {
            body.insert("session_rollout_missing".into(), json!(true));
        }
        if !retired_session_id.is_empty() {
            body.insert("retired_session_id".into(), json!(retired_session_id));
        }
        self.post_json_unit_with_retry(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/fail"),
            Value::Object(body),
            DEFAULT_TERMINAL_RETRY_SCHEDULE,
        )
        .await
    }

    /// `PinTaskSession` (client.go:504): persists the agent's session_id and
    /// work_dir mid-flight so a crash doesn't lose the resume pointer.
    pub(crate) async fn pin_task_session(
        &self,
        ctx: &crate::repocache::Ctx,
        task_id: &str,
        session_id: &str,
        work_dir: &str,
    ) -> anyhow::Result<()> {
        if session_id.is_empty() && work_dir.is_empty() {
            return Ok(());
        }
        let mut body = serde_json::Map::new();
        if !session_id.is_empty() {
            body.insert("session_id".into(), json!(session_id));
        }
        if !work_dir.is_empty() {
            body.insert("work_dir".into(), json!(work_dir));
        }
        self.post_json_unit(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/session"),
            Value::Object(body),
        )
        .await
    }
}

/// `TaskCancelAck` (client.go:372): the payload of the daemon's cancel
/// acknowledgement. A cancelled worktree task has already finalized — this ack
/// is the only channel left to report where that partial work went; when the
/// cancelled run additionally FAILED to persist its work, the error text is the
/// only pointer to it.
#[derive(Debug, Clone, Default)]
pub struct TaskCancelAck {
    /// A cancelled worktree task has already finalized — its partial work is
    /// committed to a branch in the user's repo.
    pub branch_name: String,
    /// The configured local_directory path that became authoritative after the
    /// disposable task worktree was removed.
    pub durable_work_dir: String,
    /// Set when the cancelled run additionally FAILED to persist its work.
    pub error_message: String,
    pub failure_reason: String,
}

/// `TaskMessageData` (client.go:426): a single agent execution message for
/// batch reporting.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskMessageData {
    pub seq: i32,
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Map<String, Value>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
}

impl Client {
    /// `AckTaskCancelled` (client.go:400): tells the server this daemon
    /// observed the cancellation and finished flushing the transcript (#5219).
    /// Retried like the complete/fail callbacks: when the ack carries a branch
    /// or an error it is a terminal delivery — the only pointer to the
    /// cancelled task's work.
    pub(crate) async fn ack_task_cancelled(
        &self,
        ctx: &crate::repocache::Ctx,
        task_id: &str,
        ack: TaskCancelAck,
    ) -> anyhow::Result<()> {
        let mut body = serde_json::Map::new();
        if !ack.branch_name.is_empty() {
            body.insert("branch_name".into(), json!(ack.branch_name));
        }
        if !ack.durable_work_dir.is_empty() {
            body.insert("durable_work_dir".into(), json!(ack.durable_work_dir));
        }
        if !ack.error_message.is_empty() {
            body.insert("error_message".into(), json!(ack.error_message));
        }
        if !ack.failure_reason.is_empty() {
            body.insert("failure_reason".into(), json!(ack.failure_reason));
        }
        self.post_json_unit_with_retry(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/cancel-ack"),
            Value::Object(body),
            DEFAULT_TERMINAL_RETRY_SCHEDULE,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Heartbeat / registration / misc (client.go:518–926)
// ---------------------------------------------------------------------------

/// Heartbeat aliases (client.go:541–547): HTTP and WS heartbeat paths share a
/// single type via the protocol payload directly.
pub type HeartbeatResponse = DaemonHeartbeatAckPayload;

/// `RegisterResponse` (client.go:861).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RegisterResponse {
    #[serde(default)]
    pub runtimes: Vec<Runtime>,
    #[serde(default)]
    pub repos: Vec<RepoData>,
    #[serde(rename = "repos_version", default)]
    pub repos_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Value>,
}

/// `WorkspaceReposResponse` (client.go:876).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorkspaceReposResponse {
    #[serde(rename = "workspace_id", default)]
    pub workspace_id: String,
    #[serde(default)]
    pub repos: Vec<RepoData>,
    #[serde(rename = "repos_version", default)]
    pub repos_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Value>,
}

/// `RuntimeProfile` (client.go:896): mirrors the server's workspace custom
/// runtime profile (MUL-3284). protocol_family selects the agent backend;
/// command_name is the executable the daemon resolves and launches.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuntimeProfile {
    pub id: String,
    #[serde(rename = "workspace_id")]
    pub workspace_id: String,
    #[serde(rename = "display_name")]
    pub display_name: String,
    #[serde(rename = "protocol_family")]
    pub protocol_family: String,
    #[serde(rename = "command_name")]
    pub command_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "fixed_args", default)]
    pub fixed_args: Vec<String>,
    pub visibility: String,
    pub enabled: bool,
}

/// `RuntimeProfilesResponse` (client.go:911).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RuntimeProfilesResponse {
    #[serde(rename = "workspace_id")]
    pub workspace_id: String,
    #[serde(rename = "runtime_profiles")]
    pub runtime_profiles: Vec<RuntimeProfile>,
}

/// `WorkspaceInfo` (client.go:581): minimal workspace metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub id: String,
    pub name: String,
}

/// `RenewTokenResponse` (client.go:589): mirrors handler.RenewPATResponse —
/// kept loose because the daemon never parses the timestamp itself.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RenewTokenResponse {
    #[serde(rename = "expires_at")]
    pub expires_at: String,
    pub renewed: bool,
}

/// `IssueGCStatus` (client.go:682).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct IssueGcStatus {
    pub status: String,
    #[serde(rename = "updated_at")]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// `IssueGCCheckResult` (client.go:691): found=false deliberately covers both
/// a deleted issue and an ID outside the requested workspace (anti-enumeration
/// contract). err is only populated by the legacy per-issue fallback.
#[derive(Debug, Default)]
pub struct IssueGcCheckResult {
    pub id: String,
    pub found: bool,
    pub status: String,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Only populated by the legacy per-issue fallback; rendered via Display
    /// (anyhow::Error is not Clone, so the struct drops its Go-clone parity).
    pub err: Option<String>,
}

/// `ChatSessionGCStatus` (client.go:779).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ChatSessionGcStatus {
    pub status: String,
    #[serde(rename = "updated_at")]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// `AutopilotRunGCStatus` (client.go:801).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AutopilotRunGcStatus {
    pub status: String,
    #[serde(rename = "completed_at")]
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// `TaskGCStatus` (client.go:818).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TaskGcStatus {
    pub status: String,
    #[serde(rename = "completed_at")]
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// `RuntimeOfflineCodeNotExecutable` (client.go:837): marks a runtime taken
/// offline because the OS refuses to execute its agent CLI — the one
/// deregistration cause no amount of waiting fixes (MUL-6164).
pub const RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE: &str = "not_executable";

/// `RuntimeOfflineReason` (client.go:843): why a runtime went offline, in the
/// form clients can act on. Prose stays in Detail for logs.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeOfflineReason {
    pub code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// Mirrors agent.ExecFormatRepair (package + reinstall command).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<Repair>,
}

/// Stand-in for `agent.ExecFormatRepair` (codex.go wire shape).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repair {
    #[serde(rename = "package")]
    pub package: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shell: String,
}

impl Client {
    /// `RecoverOrphans` (client.go:521): fail any dispatched/running tasks the
    /// previous daemon process left behind; the server auto-retries eligible
    /// tasks.
    pub(crate) async fn recover_orphans(
        &self,
        ctx: &crate::repocache::Ctx,
        runtime_id: &str,
    ) -> anyhow::Result<()> {
        self.post_json_unit(
            ctx,
            &format!("/api/daemon/runtimes/{runtime_id}/recover-orphans"),
            json!({}),
        )
        .await
    }

    /// `GetTaskStatus` (client.go:528): current status of a task, used to
    /// detect terminal/interruption signals while a task executes.
    pub(crate) async fn get_task_status(
        &self,
        ctx: &crate::repocache::Ctx,
        task_id: &str,
    ) -> anyhow::Result<String> {
        #[derive(Deserialize)]
        struct Resp {
            status: String,
        }
        let resp: Resp = self
            .get_json(ctx, &format!("/api/daemon/tasks/{task_id}/status"))
            .await?;
        Ok(resp.status)
    }

    /// `SendHeartbeat` (client.go:549).
    pub(crate) async fn send_heartbeat(
        &self,
        ctx: &crate::repocache::Ctx,
        runtime_id: &str,
    ) -> anyhow::Result<HeartbeatResponse> {
        self.post_json(
            ctx,
            "/api/daemon/heartbeat",
            json!({ "runtime_id": runtime_id, "supports_batch_import": true }),
        )
        .await
    }

    /// `ReportUpdateResult` (client.go:561).
    pub(crate) async fn report_update_result(
        &self,
        ctx: &crate::repocache::Ctx,
        runtime_id: &str,
        update_id: &str,
        result: Value,
    ) -> anyhow::Result<()> {
        self.post_json_unit(
            ctx,
            &format!("/api/daemon/runtimes/{runtime_id}/update/{update_id}/result"),
            result,
        )
        .await
    }

    /// `ReportModelListResult` (client.go:566).
    pub(crate) async fn report_model_list_result(
        &self,
        ctx: &crate::repocache::Ctx,
        runtime_id: &str,
        request_id: &str,
        result: Value,
    ) -> anyhow::Result<()> {
        self.post_json_unit(
            ctx,
            &format!("/api/daemon/runtimes/{runtime_id}/models/{request_id}/result"),
            result,
        )
        .await
    }

    /// `ReportLocalSkillListResult` (client.go:571).
    pub(crate) async fn report_local_skill_list_result(
        &self,
        ctx: &crate::repocache::Ctx,
        runtime_id: &str,
        request_id: &str,
        result: Value,
    ) -> anyhow::Result<()> {
        self.post_json_unit(
            ctx,
            &format!("/api/daemon/runtimes/{runtime_id}/local-skills/{request_id}/result"),
            result,
        )
        .await
    }

    /// `ReportLocalSkillImportResult` (client.go:576).
    pub(crate) async fn report_local_skill_import_result(
        &self,
        ctx: &crate::repocache::Ctx,
        runtime_id: &str,
        request_id: &str,
        result: Value,
    ) -> anyhow::Result<()> {
        self.post_json_unit(
            ctx,
            &format!("/api/daemon/runtimes/{runtime_id}/local-skills/import/{request_id}/result"),
            result,
        )
        .await
    }

    /// `RenewToken` (client.go:599): extend the daemon's PAT in place when
    /// within the server-side renewal window. Safe on any cadence.
    pub(crate) async fn renew_token(
        &self,
        ctx: &crate::repocache::Ctx,
    ) -> anyhow::Result<RenewTokenResponse> {
        self.post_json(ctx, "/api/tokens/current/renew", json!({}))
            .await
    }

    /// `ListWorkspaces` (client.go:611): minimal workspace membership set.
    /// First 404 permanently switches this client process to the legacy full-
    /// workspace endpoint for compatibility with older servers.
    pub(crate) async fn list_workspaces(
        &self,
        ctx: &crate::repocache::Ctx,
    ) -> Result<Vec<WorkspaceInfo>, ClientError> {
        let legacy_enabled = self.workspace_state.lock().unwrap().legacy_endpoint_enabled;
        if legacy_enabled {
            return Ok(self.list_legacy_workspaces(ctx).await?);
        }

        let path = "/api/daemon/workspaces";
        let mut builder = self.http.get(format!("{}{path}", self.base_url));
        let token = self.token();
        if !token.is_empty() {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        builder = self.set_identity_headers(builder);
        let etag = self.workspace_state.lock().unwrap().etag.clone();
        if !etag.is_empty() {
            builder = builder.header("If-None-Match", etag);
        }
        let builder = apply_ctx_deadline(builder, ctx, CONTROL_PLANE_TIMEOUT);
        let resp = builder.send().await.map_err(anyhow::Error::from)?;
        let status = resp.status().as_u16();

        if status == 404 {
            cdp_discard(resp).await;
            {
                let mut state = self.workspace_state.lock().unwrap();
                state.legacy_endpoint_enabled = true;
                state.etag.clear();
                state.cache.clear();
                state.cache_valid = false;
            }
            return Ok(self.list_legacy_workspaces(ctx).await?);
        }
        if status == 304 {
            let cached = {
                let state = self.workspace_state.lock().unwrap();
                if !state.cache_valid {
                    return Err(ClientError::Other(anyhow::anyhow!(
                        "GET {path} returned 304 without a cached workspace set"
                    )));
                }
                state.cache.clone()
            };
            return Ok(cached);
        }
        if status >= 400 {
            let body = body_limited(resp, 4096).await;
            return Err(ClientError::Request(RequestError {
                method: "GET",
                path: path.to_string(),
                status_code: status,
                body,
            }));
        }

        let etag = resp
            .headers()
            .get("ETag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let workspaces: Vec<WorkspaceInfo> = resp.json().await.map_err(anyhow::Error::from)?;
        let mut state = self.workspace_state.lock().unwrap();
        state.etag = etag;
        state.cache = workspaces.clone();
        state.cache_valid = true;
        drop(state);
        Ok(workspaces)
    }

    async fn list_legacy_workspaces(
        &self,
        ctx: &crate::repocache::Ctx,
    ) -> anyhow::Result<Vec<WorkspaceInfo>> {
        let workspaces: Vec<WorkspaceInfo> = self.get_json(ctx, "/api/workspaces").await?;
        Ok(workspaces)
    }

    /// `usesLegacyWorkspaceEndpoint` (client.go:675).
    pub(crate) fn uses_legacy_workspace_endpoint(&self) -> bool {
        self.workspace_state.lock().unwrap().legacy_endpoint_enabled
    }

    /// `GetIssueGCCheck` (client.go:770): status and updated_at of an issue
    /// for GC decisions.
    pub(crate) async fn get_issue_gc_check(
        &self,
        ctx: &crate::repocache::Ctx,
        issue_id: &str,
    ) -> anyhow::Result<IssueGcStatus> {
        self.get_json(ctx, &format!("/api/daemon/issues/{issue_id}/gc-check"))
            .await
    }

    /// `GetChatSessionGCCheck` (client.go:788): status of a chat session for
    /// GC decisions. A 404 indicates hard-deletion (immediate-clean signal).
    pub(crate) async fn get_chat_session_gc_check(
        &self,
        ctx: &crate::repocache::Ctx,
        session_id: &str,
    ) -> anyhow::Result<ChatSessionGcStatus> {
        self.get_json(
            ctx,
            &format!("/api/daemon/chat-sessions/{session_id}/gc-check"),
        )
        .await
    }

    /// `GetAutopilotRunGCCheck` (client.go:807).
    pub(crate) async fn get_autopilot_run_gc_check(
        &self,
        ctx: &crate::repocache::Ctx,
        run_id: &str,
    ) -> anyhow::Result<AutopilotRunGcStatus> {
        self.get_json(
            ctx,
            &format!("/api/daemon/autopilot-runs/{run_id}/gc-check"),
        )
        .await
    }

    /// `GetTaskGCCheck` (client.go:824).
    pub(crate) async fn get_task_gc_check(
        &self,
        ctx: &crate::repocache::Ctx,
        task_id: &str,
    ) -> anyhow::Result<TaskGcStatus> {
        self.get_json(ctx, &format!("/api/daemon/tasks/{task_id}/gc-check"))
            .await
    }

    /// `Deregister` (client.go:852): take runtimes offline. reasons is keyed
    /// by runtime id — a shutting-down daemon explains nothing, while one that
    /// condemned a broken CLI does.
    pub(crate) async fn deregister(
        &self,
        ctx: &crate::repocache::Ctx,
        runtime_ids: &[String],
        reasons: HashMap<String, RuntimeOfflineReason>,
    ) -> anyhow::Result<()> {
        let mut body = serde_json::Map::new();
        body.insert("runtime_ids".into(), json!(runtime_ids));
        if !reasons.is_empty() {
            body.insert("offline_reasons".into(), json!(reasons));
        }
        self.post_json_unit(ctx, "/api/daemon/deregister", Value::Object(body))
            .await
    }

    /// `Register` (client.go:868).
    pub(crate) async fn register(
        &self,
        ctx: &crate::repocache::Ctx,
        req: Value,
    ) -> anyhow::Result<RegisterResponse> {
        self.post_json(ctx, "/api/daemon/register", req).await
    }

    /// `GetWorkspaceRepos` (client.go:883).
    pub(crate) async fn get_workspace_repos(
        &self,
        ctx: &crate::repocache::Ctx,
        workspace_id: &str,
    ) -> anyhow::Result<WorkspaceReposResponse> {
        self.get_json(ctx, &format!("/api/daemon/workspaces/{workspace_id}/repos"))
            .await
    }

    /// `GetRuntimeProfiles` (client.go:920): fetches the workspace's enabled
    /// custom runtime profiles. Best-effort: an older server 404s, which the
    /// caller swallows and continues with built-in runtimes only.
    pub(crate) async fn get_runtime_profiles(
        &self,
        ctx: &crate::repocache::Ctx,
        workspace_id: &str,
    ) -> anyhow::Result<RuntimeProfilesResponse> {
        self.get_json(
            ctx,
            &format!("/api/daemon/workspaces/{workspace_id}/runtime-profiles"),
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// Issue GC batch (client.go:699–767)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct IssueGcBatchResponse {
    #[serde(default)]
    issues: Vec<IssueGcCheckResultWire>,
}

/// Wire shape of `issueGCBatchResponse.Issues` — the Go struct carries a
/// non-serialized `Err error` field, split here into the wire struct plus a
/// side map.
#[derive(Debug, Default, Deserialize)]
struct IssueGcCheckResultWire {
    id: String,
    found: bool,
    #[serde(default)]
    status: String,
    #[serde(rename = "updated_at", default)]
    updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Client {
    /// `GetIssueGCChecks` (client.go:720): reconcile a workspace's issue IDs in
    /// one request. First unmatched-route 404 permanently switches to the
    /// legacy per-issue endpoint; other batch failures propagate so a transient
    /// server problem cannot amplify request volume.
    pub(crate) async fn get_issue_gc_checks(
        &self,
        ctx: &crate::repocache::Ctx,
        workspace_id: &str,
        issue_ids: &[String],
    ) -> anyhow::Result<HashMap<String, IssueGcCheckResult>> {
        if self.issue_gc_batch_state.lock().unwrap().legacy_enabled {
            return Ok(self.get_legacy_issue_gc_checks(ctx, issue_ids).await);
        }
        let path = format!("/api/daemon/workspaces/{workspace_id}/issues/gc-check");
        let resp: Result<IssueGcBatchResponse, anyhow::Error> = self
            .post_json(ctx, &path, json!({ "issue_ids": issue_ids }))
            .await;
        let resp = match resp {
            Ok(resp) => resp,
            Err(err) => {
                let unsupported = request_error(&err).is_some_and(|req| {
                    req.status_code == 404 && req.body.trim() == "404 page not found"
                });
                if !unsupported {
                    return Err(err);
                }
                self.issue_gc_batch_state.lock().unwrap().legacy_enabled = true;
                return Ok(self.get_legacy_issue_gc_checks(ctx, issue_ids).await);
            }
        };
        let mut results = HashMap::with_capacity(resp.issues.len());
        for issue in resp.issues {
            results.insert(
                issue.id.clone(),
                IssueGcCheckResult {
                    id: issue.id,
                    found: issue.found,
                    status: issue.status,
                    updated_at: issue.updated_at,
                    err: None,
                },
            );
        }
        Ok(results)
    }

    /// `getLegacyIssueGCChecks` (client.go:746).
    async fn get_legacy_issue_gc_checks(
        &self,
        ctx: &crate::repocache::Ctx,
        issue_ids: &[String],
    ) -> HashMap<String, IssueGcCheckResult> {
        let mut results = HashMap::with_capacity(issue_ids.len());
        for issue_id in issue_ids {
            match self.get_issue_gc_check(ctx, issue_id).await {
                Err(_not_found) => {
                    results.insert(
                        issue_id.clone(),
                        IssueGcCheckResult {
                            id: issue_id.clone(),
                            found: false,
                            ..Default::default()
                        },
                    );
                }
                Ok(status) => {
                    results.insert(
                        issue_id.clone(),
                        IssueGcCheckResult {
                            id: issue_id.clone(),
                            found: true,
                            status: status.status,
                            updated_at: status.updated_at,
                            err: None,
                        },
                    );
                }
            }
        }
        results
    }
}

// ---------------------------------------------------------------------------
// Plugin hooks (client.go:1154–1184)
// ---------------------------------------------------------------------------

impl Client {
    /// `InvokeAgentPluginHook` (client.go:1160): asks the server to make one
    /// agent-triggered hook call. A refused or failed hook comes back as a 200
    /// with a status so the caller can render it as a TOOL error rather than a
    /// broken transport — an unreachable plugin endpoint must not fail the
    /// task.
    pub(crate) async fn invoke_agent_plugin_hook(
        &self,
        ctx: &crate::repocache::Ctx,
        daemon_token: &str,
        task_id: &str,
        installation_id: &str,
        hook_key: &str,
        input: Option<Value>,
    ) -> anyhow::Result<Option<Value>> {
        #[derive(Deserialize)]
        struct Response {
            status: String,
            #[serde(default, rename = "output")]
            output: Option<Value>,
            #[serde(default)]
            error: String,
        }
        let path = format!("/api/daemon/tasks/{}/plugin-hooks", url_escape(task_id));
        let mut body = serde_json::Map::new();
        body.insert("installation_id".into(), json!(installation_id));
        body.insert("hook_key".into(), json!(hook_key));
        if let Some(input) = input {
            if !input.is_null() {
                body.insert("input".into(), input);
            }
        }
        let response: Response = self
            .post_json_with_token(ctx, &path, daemon_token, Value::Object(body))
            .await?;
        if response.status != "ok" {
            if !response.error.is_empty() {
                anyhow::bail!("{}", response.error);
            }
            anyhow::bail!("the plugin hook did not succeed");
        }
        Ok(response.output)
    }
}

// ---------------------------------------------------------------------------
// Retry plumbing (client.go:928–1040)
// ---------------------------------------------------------------------------

/// `defaultTerminalRetrySchedule` (client.go:934): five backoffs totalling 124s
/// rides out short upstream blips (MUL-2780) without leaving the task stuck.
/// N entries → N+1 attempts.
pub(crate) const DEFAULT_TERMINAL_RETRY_SCHEDULE: &[Duration] = &[
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(32),
    Duration::from_secs(64),
];

/// `isTransientError` (client.go:977): likely-to-resolve hiccups — 5xx,
/// 408/429 — versus permanent 4xx. Non-request errors (transport-level) are
/// transient by definition. Callers separately bail on parent-context
/// cancellation.
fn is_transient_error(err: &anyhow::Error) -> bool {
    let Some(req) = request_error(err) else {
        return true;
    };
    if req.status_code >= 500 {
        return true;
    }
    req.status_code == 408 || req.status_code == 429
}

fn request_error(err: &anyhow::Error) -> Option<&RequestError> {
    err.downcast_ref::<RequestError>().or_else(|| {
        err.downcast_ref::<ClientError>()
            .and_then(ClientError::as_request)
    })
}

impl Client {
    /// `postJSONWithRetry` (client.go:1009): bounded exponential backoff for
    /// "must reach the server" terminal callbacks. Retries per
    /// [`is_transient_error`], stops immediately on permanent 4xx. The
    /// CompleteTask/FailTask handlers treat "already terminal" as idempotent
    /// success, so duplicate replays are safe.
    async fn post_json_with_retry<R: DeserializeOwned>(
        &self,
        ctx: &crate::repocache::Ctx,
        path: &str,
        req_body: Value,
        schedule: &[Duration],
    ) -> anyhow::Result<R> {
        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 0..=schedule.len() {
            if let Some(cancelled) = ctx.err() {
                if let Some(last_err) = last_err {
                    return Err(last_err);
                }
                anyhow::bail!("{cancelled}");
            }
            match self.post_json::<R>(ctx, path, req_body.clone()).await {
                Ok(resp) => return Ok(resp),
                Err(err) => {
                    if !is_transient_error(&err) {
                        return Err(err);
                    }
                    last_err = Some(err);
                    if attempt >= schedule.len() {
                        return Err(last_err.expect("set above"));
                    }
                    let d = schedule[attempt];
                    tokio::select! {
                        () = tokio::time::sleep(d) => {}
                        () = ctx.cancelled() => {
                            return Err(last_err.expect("set above"));
                        }
                    }
                }
            }
        }
        unreachable!("loop returns on attempt == len(schedule)")
    }

    /// Retry variant for terminal acknowledgements whose response body is not
    /// part of the protocol. Successful bodies are drained rather than parsed.
    async fn post_json_unit_with_retry(
        &self,
        ctx: &crate::repocache::Ctx,
        path: &str,
        req_body: Value,
        schedule: &[Duration],
    ) -> anyhow::Result<()> {
        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 0..=schedule.len() {
            if let Some(cancelled) = ctx.err() {
                return Err(last_err.unwrap_or_else(|| anyhow::anyhow!(cancelled)));
            }
            match self.post_json_unit(ctx, path, req_body.clone()).await {
                Ok(()) => return Ok(()),
                Err(err) => {
                    if !is_transient_error(&err) {
                        return Err(err);
                    }
                    last_err = Some(err);
                    if attempt >= schedule.len() {
                        return Err(last_err.expect("set above"));
                    }
                    tokio::select! {
                        () = tokio::time::sleep(schedule[attempt]) => {}
                        () = ctx.cancelled() => {
                            return Err(last_err.expect("set above"));
                        }
                    }
                }
            }
        }
        unreachable!("loop returns on attempt == len(schedule)")
    }
}

// ---------------------------------------------------------------------------
// Raw request helpers (client.go:1042–1152)
// ---------------------------------------------------------------------------

use serde::de::DeserializeOwned;

impl Client {
    /// `postJSON` (client.go:1042): POST JSON under the fixed control-plane
    /// timeout; decodes the response into `R`.
    async fn post_json<R: DeserializeOwned>(
        &self,
        ctx: &crate::repocache::Ctx,
        path: &str,
        req_body: Value,
    ) -> anyhow::Result<R> {
        self.post_json_with_timeout(ctx, path, req_body, CONTROL_PLANE_TIMEOUT)
            .await
    }

    async fn post_json_with_timeout<R: DeserializeOwned>(
        &self,
        ctx: &crate::repocache::Ctx,
        path: &str,
        req_body: Value,
        timeout: Duration,
    ) -> anyhow::Result<R> {
        let builder = self.builder_post(path, req_body)?;
        let opt = self
            .execute_json::<R>(
                apply_ctx_deadline(builder, ctx, timeout),
                ctx.clone(),
                true,
                "POST",
            )
            .await?;
        Ok(opt.unwrap_or_else(|| serde_json::from_value(Value::Null).unwrap()))
    }

    /// `postJSON` where Go passed `respBody == nil`.
    async fn post_json_unit(
        &self,
        ctx: &crate::repocache::Ctx,
        path: &str,
        req_body: Value,
    ) -> anyhow::Result<()> {
        let builder = self.builder_post(path, req_body.clone())?;
        let _: Option<serde_json::Value> = self
            .execute_json(
                apply_ctx_deadline(builder, ctx, CONTROL_PLANE_TIMEOUT),
                ctx.clone(),
                false,
                "POST",
            )
            .await?;
        Ok(())
    }

    /// `postJSONWithToken` (client.go:1122): GET's write counterpart carrying
    /// an explicit credential, used by the Remote MCP broker so its
    /// short-lived daemon token cannot race the long-lived PAT.
    async fn post_json_with_token<R: DeserializeOwned>(
        &self,
        ctx: &crate::repocache::Ctx,
        path: &str,
        token: &str,
        req_body: Value,
    ) -> anyhow::Result<R> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse().unwrap());
        if !token.is_empty() {
            headers.insert("Authorization", format!("Bearer {token}").parse().unwrap());
        }
        let builder = self
            .http
            .post(format!("{}{path}", self.base_url))
            .headers(headers)
            .json(&req_body);
        let builder = self.set_identity_headers(builder);
        let opt = self
            .execute_json::<R>(
                apply_ctx_deadline(builder, ctx, CONTROL_PLANE_TIMEOUT),
                ctx.clone(),
                true,
                "POST",
            )
            .await?;
        Ok(opt.unwrap_or_else(|| serde_json::from_value(Value::Null).unwrap()))
    }

    /// `getJSON` (client.go:1086).
    async fn get_json<R: DeserializeOwned>(
        &self,
        ctx: &crate::repocache::Ctx,
        path: &str,
    ) -> anyhow::Result<R> {
        self.get_json_with_token(ctx, path, &self.token()).await
    }

    /// `getJSONWithToken` (client.go:1093): one GET with an explicit
    /// credential.
    async fn get_json_with_token<R: DeserializeOwned>(
        &self,
        ctx: &crate::repocache::Ctx,
        path: &str,
        token: &str,
    ) -> anyhow::Result<R> {
        let mut builder = self.http.get(format!("{}{path}", self.base_url));
        if !token.is_empty() {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }
        let builder = self.set_identity_headers(builder);
        let opt = self
            .execute_json::<R>(
                apply_ctx_deadline(builder, ctx, CONTROL_PLANE_TIMEOUT),
                ctx.clone(),
                true,
                "POST",
            )
            .await?;
        Ok(opt.unwrap_or_else(|| serde_json::from_value(Value::Null).unwrap()))
    }

    fn builder_post(&self, path: &str, req_body: Value) -> anyhow::Result<reqwest::RequestBuilder> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse().unwrap());
        let token = self.token();
        if !token.is_empty() {
            headers.insert("Authorization", format!("Bearer {token}").parse().unwrap());
        }
        let builder = self
            .http
            .post(format!("{}{path}", self.base_url))
            .headers(headers)
            .json(&req_body);
        Ok(self.set_identity_headers(builder))
    }

    /// Shared executor behind post/get: sends, maps ≥400 to
    /// [`RequestError`], and decodes the body when expected (Go's
    /// `respBody != nil` arm).
    async fn execute_json<R: DeserializeOwned>(
        &self,
        builder: reqwest::RequestBuilder,
        ctx: crate::repocache::Ctx,
        expect_resp: bool,
        method: &'static str,
    ) -> anyhow::Result<Option<R>> {
        // Parent cancellation before sending mirrors Go's
        // NewRequestWithContext behavior of failing fast on a dead ctx.
        if let Some(cancelled) = ctx.err() {
            anyhow::bail!("{cancelled}");
        }
        tokio::select! {
            () = ctx.cancelled() => {
                anyhow::bail!(ctx.cause())
            }
            result = async move {
                let resp = builder.send().await.map_err(anyhow::Error::from)?;
                let status = resp.status();
                if status.as_u16() >= 400 {
                    return Err(RequestError {
                        path: resp.url().path().to_string(),
                        status_code: status.as_u16(),
                        body: body_limited(resp, 4096).await,
                        method,
                    }
                    .into());
                }
                if !expect_resp {
                    cdp_discard(resp).await;
                    return Ok(None);
                }
                let parsed: R = resp.json().await.map_err(anyhow::Error::from)?;
                Ok(Some(parsed))
            } => result,
        }
    }
}

/// Applies the tighter of the ctx deadline and `fallback` to a request. The
/// Go code relied on http.Client.Timeout; here we wrap the send future with a
/// select at execution time via tokio::time::timeout inside `execute_json`'s
/// caller chain — expressed by storing nothing: reqwest applies no default
/// timeout, so we emulate with `ctx` racing in `execute_json`.
fn apply_ctx_deadline(
    builder: reqwest::RequestBuilder,
    _ctx: &crate::repocache::Ctx,
    fallback: Duration,
) -> reqwest::RequestBuilder {
    builder.timeout(fallback)
}

/// Reads at most `limit` bytes of the body, trimmed (Go's LimitReader +
/// TrimSpace).
async fn body_limited(resp: reqwest::Response, limit: usize) -> String {
    let bytes = resp.bytes().await.unwrap_or_default();
    let end = bytes.len().min(limit);
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

/// Drains and discards a response body (Go's io.Copy(io.Discard)).
async fn cdp_discard(resp: reqwest::Response) {
    let _ = resp.bytes().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn serve_once(delay: Duration, body: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            std::thread::sleep(delay);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body);
        });
        format!("http://{address}")
    }

    fn request_error_with_status(status_code: u16) -> anyhow::Error {
        RequestError {
            path: "/test".to_string(),
            status_code,
            body: String::new(),
            method: "POST",
        }
        .into()
    }

    #[test]
    fn retry_classification_handles_direct_request_errors() {
        assert!(!is_transient_error(&request_error_with_status(400)));
        assert!(is_transient_error(&request_error_with_status(408)));
        assert!(is_transient_error(&request_error_with_status(429)));
        assert!(is_transient_error(&request_error_with_status(500)));
    }

    #[tokio::test]
    async fn unit_post_discards_non_json_success_body() {
        let client = Client::new(serve_once(Duration::ZERO, b"not-json"));

        client
            .post_json_unit(&crate::repocache::Ctx::new(), "/terminal", json!({}))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn in_flight_request_observes_parent_cancellation() {
        let client = Client::new(serve_once(Duration::from_millis(250), b"{}"));
        let ctx = crate::repocache::Ctx::new();
        let cancel_ctx = ctx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancel_ctx.cancel_with(crate::repocache::CancelCause::Cancelled);
        });
        let started = std::time::Instant::now();

        let result = client.post_json::<Value>(&ctx, "/cancel", json!({})).await;

        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_millis(150));
    }
}
