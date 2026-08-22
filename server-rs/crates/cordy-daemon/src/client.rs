//! Port of `server/internal/daemon/client.go` (lines 1–1184) — HTTP
//! communication with the Cordy server daemon API.
//!
//! Symbol map (Go → Rust):
//! - `requestError` → [`RequestError`] (`errors.As` → `err.chain().downcast_ref`)
//! - `isWorkspaceNotFoundError` / `isTaskNotFoundError` /
//!   `isUnauthorizedError` / `isRuntimeNotFoundError` /
//!   `isBatchClaimUnsupported` / `isIssueGCBatchUnsupported` → same-named fns
//! - `Client` / `NewClient` → [`Client`] / [`Client::new`]
//! - `CloseIdleConnections` → S9-integration no-op (reqwest exposes no pool
//!   flush; revisit when the daemon lane wires transport recovery)
//! - `normalizeGOOS` → [`normalize_goos`]
//! - `SetVersion` / `SetToken` / `Token` → same-named methods
//! - `setIdentityHeaders` / `daemonClientCapabilities` → private equivalents
//! - `postJSON` / `postJSONVia` / `getJSON` / `getJSONWithToken` /
//!   `postJSONWithToken` → [`Client::post_json`] (discard) +
//!   [`Client::post_json_decode`] (typed) + get/post token variants
//! - `postJSONWithRetry` / `postJSONViaWithRetry` → retry wrappers over the
//!   same core; `retrySleep` → `crate::helpers::sleep_with_context`
//! - `defaultTerminalRetrySchedule` / `skillBundleResolveRetrySchedule` →
//!   [`DEFAULT_TERMINAL_RETRY_SCHEDULE`] / [`SKILL_BUNDLE_RESOLVE_RETRY_SCHEDULE`]
//! - `batchClaimRequestTimeout` → [`BATCH_CLAIM_REQUEST_TIMEOUT`]
//! - Endpoint methods ported one-to-one: `ClaimTask`, `ResolveRemoteMCPCredential`,
//!   `ClaimTasks`, `claimTasksLegacy`, `ResolveSkillBundle`,
//!   `ExtendTaskPrepareLease`, `StartTask`, `MarkTaskWaitingLocalDirectory`,
//!   `AckTaskCancelled`, `ReportProgress`, `ReportTaskMessages`, `CompleteTask`,
//!   `ReportTaskUsage`, `FailTask`, `PinTaskSession`, `RecoverOrphans`,
//!   `GetTaskStatus`, `SendHeartbeat`, `ReportUpdateResult`,
//!   `ReportModelListResult`, `ReportLocalSkillListResult`,
//!   `ReportLocalSkillImportResult`, `ListWorkspaces`, `RenewToken`,
//!   `GetIssueGCChecks`, `GetIssueGCCheck`, `GetChatSessionGCCheck`,
//!   `GetAutopilotRunGCCheck`, `GetTaskGCCheck`, `Deregister`, `Register`,
//!   `GetWorkspaceRepos`, `GetRuntimeProfiles`, `InvokeAgentPluginHook`
//! - Wire structs: `WorkspaceInfo`, `RenewTokenResponse`, `RegisterResponse`,
//!   `WorkspaceReposResponse`, `RuntimeProfile`, `RuntimeProfilesResponse`,
//!   `TaskCancelAck`, `TaskMessageData`, `RuntimeOfflineReason`
//! - `HeartbeatResponse = protocol.DaemonHeartbeatAckPayload` (+ siblings) →
//!   direct use of `cordy_protocol::messages` types (aliases unnecessary)
//! - `IssueGCStatus` / `IssueGCCheckResult` → reuse `crate::gc`'s
//!   [`gc::IssueGCCheckStatus`] / [`gc::IssueGCCheckResult`] (the GC lane
//!   already owns these shapes); `ChatSessionGCStatus` / `AutopilotRunGCStatus`
//!   / `TaskGCStatus` collapse onto [`gc::IssueGCCheckStatus`] there too
//! - `agent.ExecFormatRepair` → [`ExecFormatRepair`] stand-in
//!   (S9-integration: pkg/agent belongs to another lane)
//!
//! Port notes:
//! - Go's nil-vs-pointer `respBody any` split becomes discard vs typed method
//!   pairs; discards never parse the body (Go's `io.Copy(io.Discard)`).
//! - `context.WithTimeout(ctx, batchClaimRequestTimeout)` in ClaimTasks is a
//!   child [`Ctx`] cancelled by a timer task with
//!   `CancelCause::DeadlineExceeded`; cancellation is client-side only (Go's
//!   deadline also propagates to the server query).
//! - `workspaceMu` / `issueGCBatchMu` are held across network I/O in Go, so
//!   they map to `tokio::sync::Mutex` (std guards would poison Send-ness).

// S9-integration: dead_code is expected until the Daemon core (daemon.go
// port) wires these symbols; remove this allow when that lane lands.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};
use cordy_protocol::messages::DaemonHeartbeatAckPayload;
use cordy_protocol::{
    DAEMON_CAPABILITY_AGENT_SKILL_V1, DAEMON_CAPABILITY_COALESCED_COMMENTS_V1,
    DAEMON_CAPABILITY_EXECUTION_MANIFEST_V1, DAEMON_CAPABILITY_LOCAL_WORKTREE_V1,
    DAEMON_CAPABILITY_REMOTE_MCP_V1, DAEMON_CAPABILITY_RPC_V1, DAEMON_CAPABILITY_SKILL_BUNDLES_V1,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::gc::{IssueGCCheckResult, IssueGCCheckStatus};
use crate::helpers::sleep_with_context;
use crate::repocache::{CancelCause, Ctx};
use crate::types::{RepoData, Runtime, SkillData, SkillRefData, Task, TaskUsageEntry};

/// `PLUGIN_CONTRIBUTION_PREFIX` (server/pkg/remotemcp/types.go:49). A
/// Plugin-contributed connection keeps its credential in the Plugin's own
/// secret storage, which a different route serves.
const PLUGIN_CONTRIBUTION_PREFIX: &str = "plugin:";

// ---------------------------------------------------------------------------
// requestError + classifiers (client.go:23–91)
// ---------------------------------------------------------------------------

/// `requestError`: returned by post/get helpers when the server responds with
/// an error status. Display matches Go byte-for-byte.
#[derive(Debug, thiserror::Error)]
#[error("{method} {path} returned {status_code}: {body}")]
pub(crate) struct RequestError {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) status_code: i32,
    pub(crate) body: String,
}

fn request_error_of(err: &anyhow::Error) -> Option<&RequestError> {
    err.chain().find_map(|c| c.downcast_ref::<RequestError>())
}

/// `isWorkspaceNotFoundError` (client.go:35–44): 404 with "workspace not found"
/// body.
pub(crate) fn is_workspace_not_found_error(err: &anyhow::Error) -> bool {
    matches!(request_error_of(err), Some(req_err) if req_err.status_code == 404)
        && request_error_of(err)
            .map(|e| e.body.to_lowercase().contains("workspace not found"))
            .unwrap_or(false)
}

/// `isTaskNotFoundError` (client.go:51–60): 404 with "task not found" body —
/// the task was deleted server-side while the local agent was still running.
pub(crate) fn is_task_not_found_error(err: &anyhow::Error) -> bool {
    let Some(req_err) = request_error_of(err) else {
        return false;
    };
    req_err.status_code == 404 && req_err.body.to_lowercase().contains("task not found")
}

/// `isUnauthorizedError` (client.go:65–71): 401 from the server.
pub(crate) fn is_unauthorized_error(err: &anyhow::Error) -> bool {
    request_error_of(err).is_some_and(|e| e.status_code == 401)
}

/// `isRuntimeNotFoundError` (client.go:82–91): 404 with "runtime not found"
/// body — the runtime row was deleted server-side while the daemon was still
/// heartbeating against the dead UUID.
pub(crate) fn is_runtime_not_found_error(err: &anyhow::Error) -> bool {
    let Some(req_err) = request_error_of(err) else {
        return false;
    };
    req_err.status_code == 404 && req_err.body.to_lowercase().contains("runtime not found")
}

/// `isBatchClaimUnsupported` (client.go:288–294): 404 from the batch claim
/// endpoint — the server predates /api/daemon/tasks/claim and the daemon must
/// fall back to the legacy per-runtime claim (MUL-4257).
pub(crate) fn is_batch_claim_unsupported(err: &anyhow::Error) -> bool {
    request_error_of(err).is_some_and(|e| e.status_code == 404)
}

/// `isIssueGCBatchUnsupported` (client.go:708–713): distinguishes chi's
/// unmatched-route response on an older server ("404 page not found") from the
/// JSON 404 returned by a current server on authorization failure. Only the
/// former is a compatibility signal.
fn is_issue_gc_batch_unsupported(err: &anyhow::Error) -> bool {
    matches!(request_error_of(err), Some(req_err) if req_err.status_code == 404 && req_err.body.trim() == "404 page not found")
}

/// `isTransientError` (client.go:977–992): connection/TLS/I/O errors at the
/// transport layer, 5xx server responses, and 408/429 rate-limit-style codes.
/// Other 4xx codes are permanent. The caller separately bails on parent-ctx
/// cancellation; this predicate cannot distinguish shutdown from a per-attempt
/// timeout because both arrive as context errors wrapped by net/http.
pub(crate) fn is_transient_error(err: &anyhow::Error) -> bool {
    match request_error_of(err) {
        Some(req_err) => {
            req_err.status_code >= 500 || req_err.status_code == 408 || req_err.status_code == 429
        }
        None => true,
    }
}

// ---------------------------------------------------------------------------
// Client (client.go:94–137)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct IdentityState {
    platform: String,
    version: String,
    os: String,
}

#[derive(Default)]
struct WorkspaceCacheState {
    etag: String,
    cache: Vec<WorkspaceInfo>,
    valid: bool,
    legacy_endpoint_enabled: bool,
}

#[derive(Default)]
struct IssueGcBatchState {
    legacy_enabled: bool,
}

/// `Client` handles HTTP communication with the Cordy server daemon API.
pub(crate) struct Client {
    base_url: String,
    token: RwLock<String>,
    /// Control-plane client with the fixed 30s timeout (Go `c.client`).
    http: reqwest::Client,
    /// Bundle downloader without a fixed Timeout: bundles can be large and
    /// slow on jittery links, so the caller supplies a per-request,
    /// size-scaled deadline via ctx instead of being capped by the 30s
    /// control-plane timeout (GitHub #4505).
    bundle_http: reqwest::Client,

    identity: RwLock<IdentityState>,

    workspace: AsyncMutex<WorkspaceCacheState>,
    issue_gc_batch: AsyncMutex<IssueGcBatchState>,
}

impl Client {
    /// `NewClient` (client.go:122–130): creates a new daemon API client.
    pub(crate) fn new(base_url: String) -> Self {
        Self {
            base_url,
            token: RwLock::new(String::new()),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            bundle_http: reqwest::Client::new(),
            identity: RwLock::new(IdentityState {
                platform: "daemon".to_string(),
                version: String::new(),
                os: normalize_goos(runtime_goos()).to_string(),
            }),
            workspace: AsyncMutex::new(WorkspaceCacheState::default()),
            issue_gc_batch: AsyncMutex::new(IssueGcBatchState::default()),
        }
    }

    /// `CloseIdleConnections` (client.go:142–147). S9-integration: reqwest
    /// exposes no pooled-connection flush; the daemon's stale keep-alive
    /// recovery calls this after repeated heartbeat transport failures and
    /// currently no-ops.
    pub(crate) fn close_idle_connections(&self) {}

    /// `SetVersion` (client.go:166–168): records the daemon's CLI version,
    /// sent as X-Client-Version. Called by Daemon.Run after config is loaded.
    pub(crate) fn set_version(&self, v: &str) {
        self.identity
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .version = v.to_string();
    }

    /// `SetToken` (client.go:202–204): sets the auth token for authenticated
    /// requests.
    pub(crate) fn set_token(&self, token: &str) {
        *self.token.write().unwrap_or_else(|e| e.into_inner()) = token.to_string();
    }

    /// `Token` (client.go:207–209): returns the current auth token.
    pub(crate) fn token(&self) -> String {
        self.token.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    // ---- identity headers (client.go:171–199) ------------------------------

    fn apply_identity_headers(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let identity = self.identity.read().unwrap_or_else(|e| e.into_inner());
        let mut builder = builder;
        if !identity.platform.is_empty() {
            builder = builder.header("X-Client-Platform", &identity.platform);
        }
        if !identity.version.is_empty() {
            builder = builder.header("X-Client-Version", &identity.version);
        }
        if !identity.os.is_empty() {
            builder = builder.header("X-Client-OS", &identity.os);
        }
        builder.header("X-Client-Capabilities", daemon_client_capabilities())
    }

    // ---- core plumbing (client.go:1009–1152) -------------------------------

    /// Sends one request with ctx cancellation (Go's
    /// `http.NewRequestWithContext` + `httpClient.Do`). No status checking —
    /// callers layer that via [`check_status`] so special statuses (304/404
    /// fallbacks) stay inspectable.
    #[allow(clippy::too_many_arguments)]
    async fn send_request(
        &self,
        ctx: &Ctx,
        client: &reqwest::Client,
        method: reqwest::Method,
        path: &str,
        token_override: Option<&str>,
        body: Option<serde_json::Value>,
        extra_headers: &[(&str, &str)],
    ) -> anyhow::Result<reqwest::Response> {
        let url = format!("{}{}", self.base_url, path);
        let mut builder = client.request(method.clone(), &url);
        if let Some(body) = body {
            builder = builder.json(&body);
        }
        let token = token_override
            .map(str::to_string)
            .unwrap_or_else(|| self.token());
        if !token.is_empty() {
            builder = builder.bearer_auth(&token);
        }
        builder = self.apply_identity_headers(builder);
        for (name, value) in extra_headers {
            builder = builder.header(*name, *value);
        }
        let request = builder.build().context("build daemon api request")?;

        tokio::select! {
            _ = ctx.cancelled() => Err(anyhow!("{}", ctx.cause())),
            resp = client.execute(request) => Ok(resp.context(format!("{method} {path}"))?),
        }
    }

    /// Converts a `>= 400` response into a [`RequestError`] after reading up to
    /// 4096 bytes of body (Go's `io.LimitReader(resp.Body, 4096)`).
    async fn check_status(
        method: &reqwest::Method,
        path: &str,
        resp: reqwest::Response,
    ) -> anyhow::Result<reqwest::Response> {
        let status = resp.status().as_u16();
        if status < 400 {
            return Ok(resp);
        }
        let data = resp.bytes().await.unwrap_or_default();
        let limit = data.len().min(4096);
        let body_text = String::from_utf8_lossy(&data[..limit]).trim().to_string();
        Err(anyhow::Error::new(RequestError {
            method: method.as_str().to_string(),
            path: path.to_string(),
            status_code: status as i32,
            body: body_text,
        }))
    }

    /// Shared JSON round-trip behind postJSON/getJSON/postJSONVia. `Discard`
    /// mirrors Go's nil `respBody` (drain, never parse); `Decode` parses into
    /// a Value for typed conversion by the wrappers.
    #[allow(clippy::too_many_arguments)]
    async fn do_json(
        &self,
        ctx: &Ctx,
        client: &reqwest::Client,
        method: reqwest::Method,
        path: &str,
        token_override: Option<&str>,
        body: Option<serde_json::Value>,
        decode: bool,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        let resp = self
            .send_request(ctx, client, method.clone(), path, token_override, body, &[])
            .await?;
        let resp = Self::check_status(&method, path, resp).await?;
        if !decode {
            // io.Copy(io.Discard, resp.Body)
            return Ok(None);
        }
        let bytes = tokio::select! {
            _ = ctx.cancelled() => return Err(anyhow!("{}", ctx.cause())),
            bytes = resp.bytes() => bytes.context("read daemon api response")?,
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .with_context(|| format!("decode response from {path}"))
    }

    /// `postJSON` with nil `respBody`.
    pub(crate) async fn post_json(
        &self,
        ctx: &Ctx,
        path: &str,
        req_body: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.do_json(
            ctx,
            &self.http,
            reqwest::Method::POST,
            path,
            None,
            Some(req_body),
            false,
        )
        .await?;
        Ok(())
    }

    /// `postJSON` decoding into `T`.
    pub(crate) async fn post_json_decode<T: DeserializeOwned>(
        &self,
        ctx: &Ctx,
        path: &str,
        req_body: serde_json::Value,
    ) -> anyhow::Result<T> {
        let value = self
            .do_json(
                ctx,
                &self.http,
                reqwest::Method::POST,
                path,
                None,
                Some(req_body),
                true,
            )
            .await?;
        serde_json::from_value(value.unwrap_or(serde_json::Value::Null))
            .with_context(|| format!("decode response from {path}"))
    }

    /// `getJSON` (client.go:1086–1088).
    pub(crate) async fn get_json<T: DeserializeOwned>(
        &self,
        ctx: &Ctx,
        path: &str,
    ) -> anyhow::Result<T> {
        self.get_json_with_token_inner(ctx, path, &self.token(), true)
            .await
    }

    /// `getJSONWithToken` (client.go:1093–1118): performs one GET with an
    /// explicit credential, used by the Remote MCP broker so its short-lived
    /// daemon token cannot replace or race the client's long-lived PAT.
    pub(crate) async fn get_json_with_token<T: DeserializeOwned>(
        &self,
        ctx: &Ctx,
        path: &str,
        token: &str,
    ) -> anyhow::Result<T> {
        self.get_json_with_token_inner(ctx, path, token, true).await
    }

    async fn get_json_with_token_inner<T: DeserializeOwned>(
        &self,
        ctx: &Ctx,
        path: &str,
        token: &str,
        decode: bool,
    ) -> anyhow::Result<T> {
        let value = self
            .do_json(
                ctx,
                &self.http,
                reqwest::Method::GET,
                path,
                Some(token),
                None,
                decode,
            )
            .await?;
        serde_json::from_value(value.unwrap_or(serde_json::Value::Null))
            .with_context(|| format!("decode response from {path}"))
    }

    /// `postJSONWithToken` (client.go:1122–1152): getJSONWithToken's write
    /// counterpart, for the daemon's task-scoped calls that carry a body.
    pub(crate) async fn post_json_with_token<T: DeserializeOwned>(
        &self,
        ctx: &Ctx,
        path: &str,
        token: &str,
        req_body: serde_json::Value,
    ) -> anyhow::Result<T> {
        let value = self
            .do_json(
                ctx,
                &self.http,
                reqwest::Method::POST,
                path,
                Some(token),
                Some(req_body),
                true,
            )
            .await?;
        serde_json::from_value(value.unwrap_or(serde_json::Value::Null))
            .with_context(|| format!("decode response from {path}"))
    }

    /// `postJSONWithRetry` (client.go:1009–1011): bounded exponential backoff
    /// for "must reach the server" terminal callbacks. Retries transient
    /// errors per [`is_transient_error`] and stops immediately on permanent
    /// 4xx responses.
    pub(crate) async fn post_json_with_retry(
        &self,
        ctx: &Ctx,
        path: &str,
        req_body: serde_json::Value,
        schedule: &[Duration],
    ) -> anyhow::Result<()> {
        self.post_json_via_with_retry(ctx, false, path, req_body, schedule, false)
            .await?;
        Ok(())
    }

    /// `postJSONViaWithRetry` (client.go:1016–1040): retry loop over an
    /// explicit client choice (`bundle_client=true` selects the no-timeout
    /// bundle client). With N schedule entries it performs N+1 attempts in the
    /// worst case; the returned error is the last server response so callers
    /// can still inspect it with [`is_transient_error`].
    ///
    /// The server-side CompleteTask/FailTask treat "already terminal" as an
    /// idempotent success, so duplicate replays from a retry are safe even if
    /// the prior response was lost in transit.
    pub(crate) async fn post_json_via_with_retry_decode<T: DeserializeOwned>(
        &self,
        ctx: &Ctx,
        bundle_client: bool,
        path: &str,
        req_body: serde_json::Value,
        schedule: &[Duration],
    ) -> anyhow::Result<T> {
        let value = self
            .post_json_via_with_retry(ctx, bundle_client, path, req_body, schedule, true)
            .await?;
        serde_json::from_value(value.unwrap_or(serde_json::Value::Null))
            .with_context(|| format!("decode response from {path}"))
    }

    async fn post_json_via_with_retry(
        &self,
        ctx: &Ctx,
        bundle_client: bool,
        path: &str,
        req_body: serde_json::Value,
        schedule: &[Duration],
        decode: bool,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        let client = if bundle_client {
            &self.bundle_http
        } else {
            &self.http
        };
        let mut last_err: Option<anyhow::Error> = None;
        let mut attempt = 0usize;
        loop {
            if let Some(cause) = ctx.err() {
                if let Some(err) = last_err {
                    return Err(err);
                }
                return Err(anyhow!("{cause}"));
            }
            let result = self
                .do_json(
                    ctx,
                    client,
                    reqwest::Method::POST,
                    path,
                    None,
                    Some(req_body.clone()),
                    decode,
                )
                .await;
            match result {
                Ok(value) => return Ok(value),
                Err(err) => {
                    if !is_transient_error(&err) {
                        return Err(err);
                    }
                    last_err = Some(err);
                    if attempt >= schedule.len() {
                        return Err(last_err.unwrap_or_else(|| anyhow!("retry failed")));
                    }
                    if sleep_with_context(ctx, schedule[attempt]).await.is_err() {
                        return Err(last_err.unwrap_or_else(|| anyhow!("retry failed")));
                    }
                    attempt += 1;
                }
            }
        }
    }

    // ---- claim endpoints (client.go:211–323) -------------------------------

    /// `ClaimTask` (client.go:211–219).
    pub(crate) async fn claim_task(
        &self,
        ctx: &Ctx,
        runtime_id: &str,
    ) -> anyhow::Result<Option<Task>> {
        #[derive(Deserialize)]
        struct Resp {
            #[serde(default)]
            task: Option<Task>,
        }
        let resp: Resp = self
            .post_json_decode(
                ctx,
                &format!("/api/daemon/runtimes/{runtime_id}/tasks/claim"),
                serde_json::json!({}),
            )
            .await?;
        Ok(resp.task)
    }

    /// `ResolveRemoteMCPCredential` (client.go:221–243): resolves one remote
    /// MCP credential through the server broker. Returns the single header to
    /// attach (Go builds an http.Header with at most one entry).
    pub(crate) async fn resolve_remote_mcp_credential(
        &self,
        ctx: &Ctx,
        daemon_token: &str,
        task_id: &str,
        contribution_id: &str,
    ) -> anyhow::Result<HashMap<String, String>> {
        #[derive(Deserialize)]
        struct Response {
            #[serde(rename = "credential_header", default)]
            credential_header: String,
            #[serde(default)]
            credential: String,
        }
        let route = if contribution_id.starts_with(PLUGIN_CONTRIBUTION_PREFIX) {
            "plugin-mcp"
        } else {
            "remote-mcp"
        };
        let path = format!(
            "/api/daemon/tasks/{}/{}/{}/credential",
            path_escape(task_id),
            route,
            path_escape(contribution_id)
        );
        let response: Response = self.get_json_with_token(ctx, &path, daemon_token).await?;
        let mut headers = HashMap::new();
        if !response.credential_header.is_empty() {
            headers.insert(response.credential_header, response.credential);
        }
        Ok(headers)
    }

    /// `ClaimTasks` (client.go:267–281): machine-level (MUL-4257) batch
    /// counterpart of ClaimTask — claims up to maxTasks tasks across every
    /// runtime the daemon hosts in one request. Runs under a short,
    /// request-scoped deadline rather than the shared 30s control-plane
    /// timeout so one slow claim cannot stall the whole batch; the deadline
    /// propagates to the server and cancels the in-flight query there too.
    pub(crate) async fn claim_tasks(
        &self,
        ctx: &Ctx,
        daemon_id: &str,
        runtime_ids: &[String],
        max_tasks: usize,
    ) -> anyhow::Result<Vec<Task>> {
        #[derive(Deserialize)]
        struct Resp {
            #[serde(default)]
            tasks: Vec<Task>,
        }
        let cctx = ctx.child();
        let timer_ctx = cctx.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(BATCH_CLAIM_REQUEST_TIMEOUT).await;
            timer_ctx.cancel_with(CancelCause::DeadlineExceeded);
        });
        let result = self
            .post_json_decode::<Resp>(
                &cctx,
                "/api/daemon/tasks/claim",
                serde_json::json!({
                    "daemon_id": daemon_id,
                    "runtime_ids": runtime_ids,
                    "max_tasks": max_tasks,
                }),
            )
            .await;
        timer.abort();
        Ok(result?.tasks)
    }

    /// `claimTasksLegacy` (client.go:302–323): pre-batch compatibility
    /// fallback (MUL-4257) — claim per runtime via the legacy POST
    /// /api/daemon/runtimes/{id}/tasks/claim. A per-runtime error is only
    /// propagated when nothing has been claimed yet; otherwise the partial
    /// result is returned and the next poll retries the rest.
    pub(crate) async fn claim_tasks_legacy(
        &self,
        ctx: &Ctx,
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
                Ok(Some(task)) => out.push(task),
                Ok(None) => {}
                Err(err) => {
                    if out.is_empty() {
                        return Err(err);
                    }
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }

    // ---- skill bundles + task lifecycle (client.go:332–536) ----------------

    /// `ResolveSkillBundle` (client.go:332–346): downloads a single skill
    /// bundle over the no-fixed-timeout bundle client, retried per
    /// [`SKILL_BUNDLE_RESOLVE_RETRY_SCHEDULE`]. One skill per request lets each
    /// download fit its own deadline and be cached independently (GitHub
    /// #4505).
    pub(crate) async fn resolve_skill_bundle(
        &self,
        ctx: &Ctx,
        runtime_id: &str,
        task_id: &str,
        reference: SkillRefData,
    ) -> anyhow::Result<SkillData> {
        #[derive(Deserialize)]
        struct Resp {
            #[serde(default)]
            bundles: Vec<SkillData>,
        }
        let path =
            format!("/api/daemon/runtimes/{runtime_id}/tasks/{task_id}/skill-bundles/resolve");
        let resp: Resp = self
            .post_json_via_with_retry_decode(
                ctx,
                true,
                &path,
                serde_json::json!({ "skills": [reference] }),
                SKILL_BUNDLE_RESOLVE_RETRY_SCHEDULE,
            )
            .await?;
        if resp.bundles.len() != 1 {
            return Err(anyhow!(
                "resolve skill bundle: expected 1 bundle, got {}",
                resp.bundles.len()
            ));
        }
        Ok(resp.bundles.into_iter().next().expect("len checked == 1"))
    }

    /// `ExtendTaskPrepareLease` (client.go:348–350).
    pub(crate) async fn extend_task_prepare_lease(
        &self,
        ctx: &Ctx,
        runtime_id: &str,
        task_id: &str,
    ) -> anyhow::Result<()> {
        self.post_json(
            ctx,
            &format!("/api/daemon/runtimes/{runtime_id}/tasks/{task_id}/prepare-lease"),
            serde_json::json!({}),
        )
        .await
    }

    /// `StartTask` (client.go:352–354).
    pub(crate) async fn start_task(&self, ctx: &Ctx, task_id: &str) -> anyhow::Result<()> {
        self.post_json(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/start"),
            serde_json::json!({}),
        )
        .await
    }

    /// `MarkTaskWaitingLocalDirectory` (client.go:365–369): parks a
    /// freshly-dispatched task in waiting_local_directory while another
    /// in-flight task holds the project's path mutex. Idempotent daemon-side.
    pub(crate) async fn mark_task_waiting_local_directory(
        &self,
        ctx: &Ctx,
        task_id: &str,
        reason: &str,
    ) -> anyhow::Result<()> {
        self.post_json(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/wait-local-directory"),
            serde_json::json!({ "reason": reason }),
        )
        .await
    }

    /// `AckTaskCancelled` (client.go:400–415): tells the server this daemon
    /// observed the task's cancellation and finished flushing the transcript
    /// (#5219). Retried like the complete/fail callbacks: when the ack carries
    /// a branch or an error it is a terminal delivery — the only pointer to
    /// the cancelled task's work.
    pub(crate) async fn ack_task_cancelled(
        &self,
        ctx: &Ctx,
        task_id: &str,
        ack: TaskCancelAck,
    ) -> anyhow::Result<()> {
        let mut body = serde_json::Map::new();
        if !ack.branch_name.is_empty() {
            body.insert("branch_name".into(), serde_json::json!(ack.branch_name));
        }
        if !ack.durable_work_dir.is_empty() {
            body.insert(
                "durable_work_dir".into(),
                serde_json::json!(ack.durable_work_dir),
            );
        }
        if !ack.error_message.is_empty() {
            body.insert("error_message".into(), serde_json::json!(ack.error_message));
        }
        if !ack.failure_reason.is_empty() {
            body.insert(
                "failure_reason".into(),
                serde_json::json!(ack.failure_reason),
            );
        }
        self.post_json_with_retry(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/cancel-ack"),
            serde_json::Value::Object(body),
            DEFAULT_TERMINAL_RETRY_SCHEDULE,
        )
        .await
    }

    /// `ReportProgress` (client.go:417–423).
    pub(crate) async fn report_progress(
        &self,
        ctx: &Ctx,
        task_id: &str,
        summary: &str,
        step: i64,
        total: i64,
    ) -> anyhow::Result<()> {
        self.post_json(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/progress"),
            serde_json::json!({ "summary": summary, "step": step, "total": total }),
        )
        .await
    }

    /// `ReportTaskMessages` (client.go:435–439).
    pub(crate) async fn report_task_messages(
        &self,
        ctx: &Ctx,
        task_id: &str,
        messages: Vec<TaskMessageData>,
    ) -> anyhow::Result<()> {
        self.post_json(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/messages"),
            serde_json::json!({ "messages": messages }),
        )
        .await
    }

    /// `CompleteTask` (client.go:441–462).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn complete_task(
        &self,
        ctx: &Ctx,
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
        body.insert("output".into(), serde_json::json!(output));
        if !branch_name.is_empty() {
            body.insert("branch_name".into(), serde_json::json!(branch_name));
        }
        if !session_id.is_empty() {
            body.insert("session_id".into(), serde_json::json!(session_id));
        }
        if !work_dir.is_empty() {
            body.insert("work_dir".into(), serde_json::json!(work_dir));
        }
        if !durable_work_dir.is_empty() {
            body.insert(
                "durable_work_dir".into(),
                serde_json::json!(durable_work_dir),
            );
        }
        if session_rollout_missing {
            body.insert("session_rollout_missing".into(), serde_json::json!(true));
        }
        if !retired_session_id.is_empty() {
            body.insert(
                "retired_session_id".into(),
                serde_json::json!(retired_session_id),
            );
        }
        self.post_json_with_retry(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/complete"),
            serde_json::Value::Object(body),
            DEFAULT_TERMINAL_RETRY_SCHEDULE,
        )
        .await
    }

    /// `ReportTaskUsage` (client.go:464–471).
    pub(crate) async fn report_task_usage(
        &self,
        ctx: &Ctx,
        task_id: &str,
        usage: Vec<TaskUsageEntry>,
    ) -> anyhow::Result<()> {
        if usage.is_empty() {
            return Ok(());
        }
        self.post_json(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/usage"),
            serde_json::json!({ "usage": usage }),
        )
        .await
    }

    /// `FailTask` (client.go:473–500).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn fail_task(
        &self,
        ctx: &Ctx,
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
        body.insert("error".into(), serde_json::json!(error_msg));
        if !session_id.is_empty() {
            body.insert("session_id".into(), serde_json::json!(session_id));
        }
        if !work_dir.is_empty() {
            body.insert("work_dir".into(), serde_json::json!(work_dir));
        }
        if !durable_work_dir.is_empty() {
            body.insert(
                "durable_work_dir".into(),
                serde_json::json!(durable_work_dir),
            );
        }
        // A failed run can still have delivered a branch: worktree mode
        // commits whatever the agent left before removing the worktree, so
        // partial work survives — but only if its name travels with the
        // failure report.
        if !branch_name.is_empty() {
            body.insert("branch_name".into(), serde_json::json!(branch_name));
        }
        if !failure_reason.is_empty() {
            body.insert("failure_reason".into(), serde_json::json!(failure_reason));
        }
        if session_rollout_missing {
            body.insert("session_rollout_missing".into(), serde_json::json!(true));
        }
        if !retired_session_id.is_empty() {
            body.insert(
                "retired_session_id".into(),
                serde_json::json!(retired_session_id),
            );
        }
        self.post_json_with_retry(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/fail"),
            serde_json::Value::Object(body),
            DEFAULT_TERMINAL_RETRY_SCHEDULE,
        )
        .await
    }

    /// `PinTaskSession` (client.go:504–516): persists the agent's session_id
    /// and work_dir mid-flight so a daemon crash doesn't lose the resume
    /// pointer.
    pub(crate) async fn pin_task_session(
        &self,
        ctx: &Ctx,
        task_id: &str,
        session_id: &str,
        work_dir: &str,
    ) -> anyhow::Result<()> {
        if session_id.is_empty() && work_dir.is_empty() {
            return Ok(());
        }
        let mut body = serde_json::Map::new();
        if !session_id.is_empty() {
            body.insert("session_id".into(), serde_json::json!(session_id));
        }
        if !work_dir.is_empty() {
            body.insert("work_dir".into(), serde_json::json!(work_dir));
        }
        self.post_json(
            ctx,
            &format!("/api/daemon/tasks/{task_id}/session"),
            serde_json::Value::Object(body),
        )
        .await
    }

    /// `RecoverOrphans` (client.go:521–523): fails dispatched/running tasks
    /// the previous daemon process left behind; the server auto-retries
    /// eligible ones.
    pub(crate) async fn recover_orphans(&self, ctx: &Ctx, runtime_id: &str) -> anyhow::Result<()> {
        self.post_json(
            ctx,
            &format!("/api/daemon/runtimes/{runtime_id}/recover-orphans"),
            serde_json::json!({}),
        )
        .await
    }

    /// `GetTaskStatus` (client.go:528–536): current status of a task, used to
    /// detect terminal/interruption signals while a task executes.
    pub(crate) async fn get_task_status(&self, ctx: &Ctx, task_id: &str) -> anyhow::Result<String> {
        #[derive(Deserialize)]
        struct Resp {
            #[serde(default)]
            status: String,
        }
        let resp: Resp = self
            .get_json(ctx, &format!("/api/daemon/tasks/{task_id}/status"))
            .await?;
        Ok(resp.status)
    }

    // ---- heartbeat + report-backs (client.go:549–578) ----------------------

    /// `SendHeartbeat` (client.go:549–558).
    pub(crate) async fn send_heartbeat(
        &self,
        ctx: &Ctx,
        runtime_id: &str,
    ) -> anyhow::Result<DaemonHeartbeatAckPayload> {
        self.post_json_decode(
            ctx,
            "/api/daemon/heartbeat",
            serde_json::json!({
                "runtime_id": runtime_id,
                "supports_batch_import": true,
            }),
        )
        .await
    }

    /// `ReportUpdateResult` (client.go:561–563).
    pub(crate) async fn report_update_result(
        &self,
        ctx: &Ctx,
        runtime_id: &str,
        update_id: &str,
        result: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.post_json(
            ctx,
            &format!("/api/daemon/runtimes/{runtime_id}/update/{update_id}/result"),
            result,
        )
        .await
    }

    /// `ReportModelListResult` (client.go:566–568).
    pub(crate) async fn report_model_list_result(
        &self,
        ctx: &Ctx,
        runtime_id: &str,
        request_id: &str,
        result: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.post_json(
            ctx,
            &format!("/api/daemon/runtimes/{runtime_id}/models/{request_id}/result"),
            result,
        )
        .await
    }

    /// `ReportLocalSkillListResult` (client.go:571–573).
    pub(crate) async fn report_local_skill_list_result(
        &self,
        ctx: &Ctx,
        runtime_id: &str,
        request_id: &str,
        result: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.post_json(
            ctx,
            &format!("/api/daemon/runtimes/{runtime_id}/local-skills/{request_id}/result"),
            result,
        )
        .await
    }

    /// `ReportLocalSkillImportResult` (client.go:576–578).
    pub(crate) async fn report_local_skill_import_result(
        &self,
        ctx: &Ctx,
        runtime_id: &str,
        request_id: &str,
        result: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.post_json(
            ctx,
            &format!("/api/daemon/runtimes/{runtime_id}/local-skills/import/{request_id}/result"),
            result,
        )
        .await
    }

    // ---- workspaces + tokens (client.go:581–673) ---------------------------

    /// `ListWorkspaces` (client.go:611–665): fetches the minimal workspace
    /// membership set. New servers expose a daemon-specific endpoint with ETag
    /// support; the first 404 permanently switches this process to the legacy
    /// full-workspace endpoint.
    pub(crate) async fn list_workspaces(&self, ctx: &Ctx) -> anyhow::Result<Vec<WorkspaceInfo>> {
        let mut state = self.workspace.lock().await;
        if state.legacy_endpoint_enabled {
            drop(state);
            return self.list_legacy_workspaces(ctx).await;
        }

        const PATH: &str = "/api/daemon/workspaces";
        let etag = state.etag.clone();
        let mut extra_headers: Vec<(&str, &str)> = Vec::new();
        let etag_header;
        if !etag.is_empty() {
            etag_header = etag;
            extra_headers.push(("If-None-Match", etag_header.as_str()));
        }

        let resp = self
            .send_request(
                ctx,
                &self.http,
                reqwest::Method::GET,
                PATH,
                None,
                None,
                &extra_headers,
            )
            .await?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            state.legacy_endpoint_enabled = true;
            state.etag.clear();
            state.cache.clear();
            state.valid = false;
            drop(state);
            return self.list_legacy_workspaces(ctx).await;
        }
        if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
            if !state.valid {
                return Err(anyhow!(
                    "GET {PATH} returned 304 without a cached workspace set"
                ));
            }
            return Ok(state.cache.clone());
        }
        let etag = resp
            .headers()
            .get("ETag")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .unwrap_or_default();
        let resp = Self::check_status(&reqwest::Method::GET, PATH, resp).await?;

        let bytes = tokio::select! {
            _ = ctx.cancelled() => return Err(anyhow!("{}", ctx.cause())),
            bytes = resp.bytes() => bytes.context("read workspaces response")?,
        };
        let workspaces: Vec<WorkspaceInfo> =
            serde_json::from_slice(&bytes).context("decode workspaces response")?;
        state.etag = etag;
        state.cache = workspaces.clone();
        state.valid = true;
        Ok(workspaces)
    }

    /// `listLegacyWorkspaces` (client.go:667–673).
    async fn list_legacy_workspaces(&self, ctx: &Ctx) -> anyhow::Result<Vec<WorkspaceInfo>> {
        self.get_json(ctx, "/api/workspaces").await
    }

    /// `usesLegacyWorkspaceEndpoint` (client.go:675–679).
    pub(crate) async fn uses_legacy_workspace_endpoint(&self) -> bool {
        self.workspace.lock().await.legacy_endpoint_enabled
    }

    /// `RenewToken` (client.go:599–605): extends the daemon's current PAT in
    /// place when within the server-side renewal window. Safe on any cadence.
    pub(crate) async fn renew_token(&self, ctx: &Ctx) -> anyhow::Result<RenewTokenResponse> {
        self.post_json_decode(ctx, "/api/tokens/current/renew", serde_json::json!({}))
            .await
    }

    // ---- GC checks (client.go:681–830) -------------------------------------

    /// `GetIssueGCChecks` (client.go:720–744): reconciles a workspace's issue
    /// IDs in one request. The first unmatched-route 404 permanently switches
    /// to the legacy per-issue endpoint; other batch failures return without
    /// fan-out so a transient problem cannot amplify request volume.
    pub(crate) async fn get_issue_gc_checks(
        &self,
        ctx: &Ctx,
        workspace_id: &str,
        issue_ids: &[String],
    ) -> anyhow::Result<HashMap<String, IssueGCCheckResult>> {
        let mut state = self.issue_gc_batch.lock().await;
        if state.legacy_enabled {
            drop(state);
            return Ok(self.get_legacy_issue_gc_checks(ctx, issue_ids).await);
        }

        #[derive(Deserialize)]
        struct WireResult {
            id: String,
            #[serde(default)]
            found: bool,
            #[serde(default)]
            status: String,
            #[serde(default)]
            updated_at: Option<DateTime<Utc>>,
        }
        #[derive(Deserialize)]
        struct BatchResp {
            #[serde(default)]
            issues: Vec<WireResult>,
        }

        let path = format!("/api/daemon/workspaces/{workspace_id}/issues/gc-check");
        let resp: anyhow::Result<BatchResp> = self
            .post_json_decode(ctx, &path, serde_json::json!({ "issue_ids": issue_ids }))
            .await;
        match resp {
            Ok(resp) => {
                let mut results = HashMap::with_capacity(resp.issues.len());
                for issue in resp.issues {
                    results.insert(
                        issue.id.clone(),
                        IssueGCCheckResult {
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
            Err(err) => {
                if !is_issue_gc_batch_unsupported(&err) {
                    return Err(err);
                }
                state.legacy_enabled = true;
                drop(state);
                Ok(self.get_legacy_issue_gc_checks(ctx, issue_ids).await)
            }
        }
    }

    /// `getLegacyIssueGCChecks` (client.go:746–767).
    async fn get_legacy_issue_gc_checks(
        &self,
        ctx: &Ctx,
        issue_ids: &[String],
    ) -> HashMap<String, IssueGCCheckResult> {
        let mut results = HashMap::with_capacity(issue_ids.len());
        for issue_id in issue_ids {
            match self.get_issue_gc_check(ctx, issue_id).await {
                Err(err) => {
                    let not_found = request_error_of(&err).is_some_and(|e| e.status_code == 404);
                    if not_found {
                        results.insert(
                            issue_id.clone(),
                            IssueGCCheckResult {
                                id: issue_id.clone(),
                                found: false,
                                ..Default::default()
                            },
                        );
                    } else {
                        results.insert(
                            issue_id.clone(),
                            IssueGCCheckResult {
                                id: issue_id.clone(),
                                err: Some(std::sync::Arc::new(err)),
                                ..Default::default()
                            },
                        );
                    }
                }
                Ok(status) => {
                    results.insert(
                        issue_id.clone(),
                        IssueGCCheckResult {
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

    /// `GetIssueGCCheck` (client.go:770–776): status + updated_at of an issue
    /// for GC decisions.
    pub(crate) async fn get_issue_gc_check(
        &self,
        ctx: &Ctx,
        issue_id: &str,
    ) -> anyhow::Result<IssueGCCheckStatus> {
        self.get_gc_status(ctx, &format!("/api/daemon/issues/{issue_id}/gc-check"))
            .await
    }

    /// `GetChatSessionGCCheck` (client.go:788–794): a 404 here indicates the
    /// session row was hard-deleted, which the caller treats as an
    /// immediate-clean signal.
    pub(crate) async fn get_chat_session_gc_check(
        &self,
        ctx: &Ctx,
        session_id: &str,
    ) -> anyhow::Result<IssueGCCheckStatus> {
        self.get_gc_status(
            ctx,
            &format!("/api/daemon/chat-sessions/{session_id}/gc-check"),
        )
        .await
    }

    /// `GetAutopilotRunGCCheck` (client.go:807–813). Go's AutopilotRunGCStatus
    /// collapses onto gc.rs's IssueGCCheckStatus: the GC loop no longer gates
    /// on CompletedAt, keeping only the wire contract fields it consumes.
    pub(crate) async fn get_autopilot_run_gc_check(
        &self,
        ctx: &Ctx,
        run_id: &str,
    ) -> anyhow::Result<IssueGCCheckStatus> {
        self.get_gc_status(
            ctx,
            &format!("/api/daemon/autopilot-runs/{run_id}/gc-check"),
        )
        .await
    }

    /// `GetTaskGCCheck` (client.go:824–830): agent_task_queue status for
    /// quick-create cleanup.
    pub(crate) async fn get_task_gc_check(
        &self,
        ctx: &Ctx,
        task_id: &str,
    ) -> anyhow::Result<IssueGCCheckStatus> {
        self.get_gc_status(ctx, &format!("/api/daemon/tasks/{task_id}/gc-check"))
            .await
    }

    async fn get_gc_status(&self, ctx: &Ctx, path: &str) -> anyhow::Result<IssueGCCheckStatus> {
        #[derive(Deserialize)]
        struct Wire {
            #[serde(default)]
            status: String,
            #[serde(default)]
            updated_at: Option<DateTime<Utc>>,
        }
        let wire: Wire = self.get_json(ctx, path).await?;
        Ok(IssueGCCheckStatus {
            status: wire.status,
            updated_at: wire.updated_at,
        })
    }

    // ---- registration + profiles (client.go:837–926) -----------------------

    /// `Deregister` (client.go:852–858): takes runtimes offline. `reasons` is
    /// optional and keyed by runtime id.
    pub(crate) async fn deregister(
        &self,
        ctx: &Ctx,
        runtime_ids: &[String],
        reasons: HashMap<String, RuntimeOfflineReason>,
    ) -> anyhow::Result<()> {
        let mut body = serde_json::Map::new();
        body.insert("runtime_ids".into(), serde_json::json!(runtime_ids));
        if !reasons.is_empty() {
            body.insert("offline_reasons".into(), serde_json::json!(reasons));
        }
        self.post_json(
            ctx,
            "/api/daemon/deregister",
            serde_json::Value::Object(body),
        )
        .await
    }

    /// `Register` (client.go:868–874).
    pub(crate) async fn register(
        &self,
        ctx: &Ctx,
        req: serde_json::Value,
    ) -> anyhow::Result<RegisterResponse> {
        self.post_json_decode(ctx, "/api/daemon/register", req)
            .await
    }

    /// `GetWorkspaceRepos` (client.go:883–889).
    pub(crate) async fn get_workspace_repos(
        &self,
        ctx: &Ctx,
        workspace_id: &str,
    ) -> anyhow::Result<WorkspaceReposResponse> {
        self.get_json(ctx, &format!("/api/daemon/workspaces/{workspace_id}/repos"))
            .await
    }

    /// `GetRuntimeProfiles` (client.go:920–926): fetches the workspace's
    /// enabled custom runtime profiles. Best-effort: an older server with no
    /// profiles route returns 404, which the caller swallows and continues
    /// with built-in runtimes only.
    pub(crate) async fn get_runtime_profiles(
        &self,
        ctx: &Ctx,
        workspace_id: &str,
    ) -> anyhow::Result<RuntimeProfilesResponse> {
        self.get_json(
            ctx,
            &format!("/api/daemon/workspaces/{workspace_id}/runtime-profiles"),
        )
        .await
    }

    // ---- plugin hooks (client.go:1160–1184) --------------------------------

    /// `InvokeAgentPluginHook` (client.go:1160–1184): asks the server to make
    /// one agent-triggered hook call. Routing through the server keeps the
    /// rate limit, circuit breaker, `net:` destination check and invocation
    /// record on one code path for every trigger. A refused or failed hook
    /// comes back as a 200 with a status so the caller can render it as a
    /// TOOL error rather than a broken transport.
    pub(crate) async fn invoke_agent_plugin_hook(
        &self,
        ctx: &Ctx,
        daemon_token: &str,
        task_id: &str,
        installation_id: &str,
        hook_key: &str,
        input: Option<serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        #[derive(Deserialize)]
        struct Response {
            #[serde(default)]
            status: String,
            #[serde(rename = "output", default, skip_serializing_if = "Option::is_none")]
            output: Option<serde_json::Value>,
            #[serde(default)]
            error: String,
        }
        let path = format!("/api/daemon/tasks/{}/plugin-hooks", path_escape(task_id));
        let mut body = serde_json::Map::new();
        body.insert("installation_id".into(), serde_json::json!(installation_id));
        body.insert("hook_key".into(), serde_json::json!(hook_key));
        if let Some(input) = input.filter(|v| !v.is_null()) {
            body.insert("input".into(), input);
        }
        let response: Response = self
            .post_json_with_token(ctx, &path, daemon_token, serde_json::Value::Object(body))
            .await?;
        if response.status != "ok" {
            if !response.error.is_empty() {
                return Err(anyhow!(response.error));
            }
            return Err(anyhow!("the plugin hook did not succeed"));
        }
        Ok(response.output.unwrap_or(serde_json::Value::Null))
    }
}

// ---------------------------------------------------------------------------
// Free helpers (client.go:132–162, 189–199, 255, 934–964)
// ---------------------------------------------------------------------------

/// Maps Rust's `std::env::consts::OS` vocabulary onto Go's `runtime.GOOS`
/// values so `normalize_goos` sees the same inputs.
fn runtime_goos() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

/// `normalizeGOOS` (client.go:151–162): maps Go's runtime.GOOS values to the
/// protocol vocabulary used by X-Client-OS / client_os. Unknown values pass
/// through unchanged, matching Go.
pub(crate) fn normalize_goos(goos: &str) -> String {
    match goos {
        "darwin" => "macos".to_string(),
        "windows" => "windows".to_string(),
        "linux" => "linux".to_string(),
        other => other.to_string(),
    }
}

/// `daemonClientCapabilities` (client.go:189–199): the X-Client-Capabilities
/// value advertised on BOTH the HTTP control-plane requests and the WS
/// handshake, so a claim built over WS gets the same capability gating as the
/// HTTP path. rpc-v1 advertises WS request/response support (MUL-4257).
fn daemon_client_capabilities() -> String {
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

/// `batchClaimRequestTimeout` (client.go:255): short request-scoped deadline
/// for the machine-level batch claim (MUL-4257). Bounding the batch caps
/// worst-case starvation across all hosted runtimes; a claim that commits
/// server-side after the client gives up is recovered by
/// ReclaimStaleDispatchedTasksForRuntimes on the next poll. Kept comfortably
/// above p99 claim latency so recovery stays the exception.
pub(crate) const BATCH_CLAIM_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// `defaultTerminalRetrySchedule` (client.go:934–940): backoff used by
/// postJSONWithRetry for terminal task callbacks. N entries → N+1 attempts in
/// the worst case. Five backoffs totalling 124s rides out short upstream blips
/// (MUL-2780).
pub(crate) const DEFAULT_TERMINAL_RETRY_SCHEDULE: &[Duration] = &[
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(32),
    Duration::from_secs(64),
];

/// `skillBundleResolveRetrySchedule` (client.go:947–950): rides out brief
/// transport blips on a single bundle download. Kept short on purpose — the
/// real budget is the size-scaled context deadline per skill (GitHub #4505).
pub(crate) const SKILL_BUNDLE_RESOLVE_RETRY_SCHEDULE: &[Duration] =
    &[Duration::from_millis(500), Duration::from_secs(2)];

/// Escapes one URL path segment exactly like Go's `url.PathEscape`
/// (encodePathSegment mode): unreserved characters plus the RFC-allowed
/// sub-delims `$&+:@=` pass through; `/;,?` and everything else is
/// percent-encoded with uppercase hex.
fn path_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'$'
            | b'&'
            | b'+'
            | b':'
            | b'@'
            | b'=' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Wire structs (client.go:372–388, 426–433, 581–592, 682–701, 843–847,
// 861–881, 896–914)
// ---------------------------------------------------------------------------

/// `TaskCancelAck` (client.go:372–388): payload of the daemon's cancel
/// acknowledgement.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct TaskCancelAck {
    /// A cancelled worktree task has already finalized — its partial work is
    /// committed to a branch in the user's repo. The cancel path discards the
    /// rest of the result, so this ack is the only channel left to report
    /// where that work went.
    pub(crate) branch_name: String,
    /// The configured local_directory path that became authoritative after the
    /// disposable task worktree was removed.
    pub(crate) durable_work_dir: String,
    /// Set when the cancelled run additionally FAILED to persist its work
    /// (worktree Finalize abort); the error text carrying the
    /// preserved-worktree path is the only pointer to the agent's work.
    pub(crate) error_message: String,
    pub(crate) failure_reason: String,
}

/// `TaskMessageData` (client.go:426–433): a single agent execution message for
/// batch reporting.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct TaskMessageData {
    pub seq: i64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "tool", skip_serializing_if = "String::is_empty")]
    pub tool: String,
    #[serde(rename = "content", skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(rename = "input", skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(rename = "output", skip_serializing_if = "String::is_empty")]
    pub output: String,
}

/// `WorkspaceInfo` (client.go:581–584): minimal workspace metadata returned by
/// the API.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct WorkspaceInfo {
    pub id: String,
    pub name: String,
}

/// `RenewTokenResponse` (client.go:589–592): mirrors handler.RenewPATResponse
/// — kept loose because the daemon never parses the timestamp itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct RenewTokenResponse {
    #[serde(rename = "expires_at", default)]
    pub expires_at: String,
    #[serde(default)]
    pub renewed: bool,
}

/// `RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE` (client.go:837): marks a runtime
/// taken offline because the OS refuses to execute its agent CLI — the one
/// deregistration cause no amount of waiting fixes (MUL-6164).
pub(crate) const RUNTIME_OFFLINE_CODE_NOT_EXECUTABLE: &str = "not_executable";

/// S9-integration stand-in for `agent.ExecFormatRepair`
/// (server/pkg/agent/exec_format.go:102–110). Shell names the interpreter
/// Command is written for, so whoever displays it can label the code block
/// truthfully.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ExecFormatRepair {
    #[serde(rename = "package", default, skip_serializing_if = "String::is_empty")]
    pub package: String,
    #[serde(rename = "command", default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(rename = "shell", default, skip_serializing_if = "String::is_empty")]
    pub shell: String,
}

/// `RuntimeOfflineReason` (client.go:843–847): why a runtime went offline, in
/// the form clients can act on — a stable code plus the command that repairs
/// the install. Prose stays in Detail for logs, never as the thing a client
/// parses.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct RuntimeOfflineReason {
    pub code: String,
    #[serde(rename = "detail", skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(rename = "repair", skip_serializing_if = "Option::is_none")]
    pub repair: Option<ExecFormatRepair>,
}

/// `RegisterResponse` (client.go:861–866).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct RegisterResponse {
    #[serde(default)]
    pub runtimes: Vec<Runtime>,
    #[serde(default)]
    pub repos: Vec<RepoData>,
    #[serde(rename = "repos_version", default)]
    pub repos_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
}

/// `WorkspaceReposResponse` (client.go:876–881).
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct WorkspaceReposResponse {
    #[serde(rename = "workspace_id", default)]
    pub workspace_id: String,
    #[serde(default)]
    pub repos: Vec<RepoData>,
    #[serde(rename = "repos_version", default)]
    pub repos_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
}

/// `RuntimeProfile` (client.go:896–906): mirrors the server's workspace custom
/// runtime profile (MUL-3284). protocol_family is the provider used for task
/// routing; command_name is the actual executable the daemon resolves on PATH
/// and launches.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct RuntimeProfile {
    pub id: String,
    #[serde(rename = "workspace_id", default)]
    pub workspace_id: String,
    #[serde(rename = "display_name", default)]
    pub display_name: String,
    #[serde(rename = "protocol_family", default)]
    pub protocol_family: String,
    #[serde(rename = "command_name", default)]
    pub command_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "fixed_args", default)]
    pub fixed_args: Vec<String>,
    #[serde(default)]
    pub visibility: String,
    #[serde(default)]
    pub enabled: bool,
}

/// `RuntimeProfilesResponse` (client.go:911–914): body of GET
/// /api/daemon/workspaces/{workspaceID}/runtime-profiles. The server only
/// returns enabled profiles for the workspace.
#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct RuntimeProfilesResponse {
    #[serde(rename = "workspace_id", default)]
    pub workspace_id: String,
    #[serde(rename = "runtime_profiles", default)]
    pub runtime_profiles: Vec<RuntimeProfile>,
}

#[cfg(test)]
mod tests {
    //! Ports of the pure-logic and fake-server cases from client_test.go
    //! (581 lines). Go's httptest.NewServer maps to a minimal TcpListener
    //! responder below; noSleepRetry's instant-sleep swap is unnecessary
    //! because the ported schedules use 1ms entries directly.

    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicI32, Ordering};

    struct FakeRequest {
        method: String,
        path: String,
        body: Vec<u8>,
        headers: HashMap<String, String>,
    }

    impl FakeRequest {
        fn header(&self, name: &str) -> String {
            self.headers
                .get(&name.to_lowercase())
                .cloned()
                .unwrap_or_default()
        }
    }

    type Responder = std::sync::Arc<dyn Fn(&FakeRequest) -> (u16, Vec<u8>) + Send + Sync>;

    /// Minimal HTTP/1.1 server standing in for Go's httptest.NewServer.
    fn spawn_fake_server(responder: Responder) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 4096];
                let header_end;
                loop {
                    let Ok(n) = stream.read(&mut chunk) else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                        header_end = pos + 4;
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let mut lines = head.split("\r\n");
                let request_line = lines.next().unwrap_or_default().to_string();
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                let mut headers = HashMap::new();
                for line in lines {
                    if let Some((name, value)) = line.split_once(':') {
                        headers.insert(name.trim().to_lowercase(), value.trim().to_string());
                    }
                }
                let content_length: usize = headers
                    .get("content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                while buf.len() < header_end + content_length {
                    let Ok(n) = stream.read(&mut chunk) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let req = FakeRequest {
                    method,
                    path,
                    body: buf[header_end..].to_vec(),
                    headers,
                };
                let (status, body) = responder(&req);
                let reason = if status < 400 { "OK" } else { "Error" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    fn request_error(status: i32, body: &str) -> anyhow::Error {
        anyhow::Error::new(RequestError {
            method: "GET".into(),
            path: "/x".into(),
            status_code: status,
            body: body.to_string(),
        })
    }

    // TestIsTransientError (client_test.go:258–281); Go's nil-error row is
    // unrepresentable with anyhow, transport-level errors cover it.
    #[test]
    fn is_transient_error_table() {
        assert!(is_transient_error(&anyhow!("connection reset by peer")));
        assert!(is_transient_error(&request_error(502, "")));
        assert!(is_transient_error(&request_error(503, "")));
        assert!(is_transient_error(&request_error(408, "")));
        assert!(is_transient_error(&request_error(429, "")));
        assert!(!is_transient_error(&request_error(400, "")));
        assert!(!is_transient_error(&request_error(401, "")));
        assert!(!is_transient_error(&request_error(404, "")));
    }

    // TestIsIssueGCBatchUnsupported (client_test.go:283–313)
    #[test]
    fn is_issue_gc_batch_unsupported_table() {
        assert!(is_issue_gc_batch_unsupported(&request_error(
            404,
            "404 page not found"
        )));
        assert!(!is_issue_gc_batch_unsupported(&request_error(
            404,
            r#"{"error":"not found"}"#
        )));
        assert!(!is_issue_gc_batch_unsupported(&request_error(
            500, "failure"
        )));
    }

    // TestDefaultTerminalRetrySchedule_MatchesAgreedPlan (client_test.go:444–457)
    #[test]
    fn default_terminal_retry_schedule_matches_agreed_plan() {
        let want = [4, 8, 16, 32, 64];
        assert_eq!(DEFAULT_TERMINAL_RETRY_SCHEDULE.len(), want.len());
        for (i, secs) in want.iter().enumerate() {
            assert_eq!(
                DEFAULT_TERMINAL_RETRY_SCHEDULE[i],
                Duration::from_secs(*secs)
            );
        }
    }

    // TestNormalizeGOOS (client_test.go:459–471)
    #[test]
    fn normalize_goos_table() {
        assert_eq!(normalize_goos("darwin"), "macos");
        assert_eq!(normalize_goos("windows"), "windows");
        assert_eq!(normalize_goos("linux"), "linux");
        assert_eq!(normalize_goos("freebsd"), "freebsd");
    }

    // TestClient_IdentityHeaders_PostJSON / GetJSON / VersionOmittedWhenUnset
    // (client_test.go:20–90)
    /// (request line, headers) pairs captured by the fake server.
    type SeenLog = Vec<(String, HashMap<String, String>)>;

    #[tokio::test]
    async fn identity_headers_sent_on_post_and_get() {
        let seen: std::sync::Arc<AsyncMutex<SeenLog>> =
            std::sync::Arc::new(AsyncMutex::new(Vec::new()));
        let sink = seen.clone();
        let url = spawn_fake_server(std::sync::Arc::new(move |req| {
            if let Ok(mut guard) = sink.try_lock() {
                guard.push((format!("{} {}", req.method, req.path), req.headers.clone()));
            }
            (200, b"{}".to_vec())
        }));
        let client = Client::new(url);
        client.set_version("v1.2.3");
        let ctx = Ctx::new();
        client
            .post_json(&ctx, "/a", serde_json::json!({}))
            .await
            .unwrap();
        let _: serde_json::Value = client.get_json(&ctx, "/b").await.unwrap();

        let seen = seen.try_lock().unwrap();
        assert_eq!(seen.len(), 2);
        for (_, headers) in seen.iter() {
            assert_eq!(
                headers.get("x-client-platform").map(String::as_str),
                Some("daemon")
            );
            assert_eq!(
                headers.get("x-client-version").map(String::as_str),
                Some("v1.2.3")
            );
            assert!(!headers
                .get("x-client-os")
                .map(String::as_str)
                .unwrap_or_default()
                .is_empty());
            let caps = headers
                .get("x-client-capabilities")
                .map(String::as_str)
                .unwrap_or_default();
            assert!(caps.contains(DAEMON_CAPABILITY_RPC_V1));
            assert!(caps.contains(DAEMON_CAPABILITY_SKILL_BUNDLES_V1));
        }
    }

    #[tokio::test]
    async fn version_omitted_when_unset() {
        let seen: std::sync::Arc<AtomicI32> = std::sync::Arc::new(AtomicI32::new(0));
        let counter = seen.clone();
        let url = spawn_fake_server(std::sync::Arc::new(move |req| {
            counter.fetch_add(1, Ordering::SeqCst);
            assert!(req.header("X-Client-Version").is_empty());
            (200, b"{}".to_vec())
        }));
        let client = Client::new(url);
        client
            .post_json(&Ctx::new(), "/a", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    // TestClient_ResolveRemoteMCPCredential* (client_test.go:91–149)
    #[tokio::test]
    async fn resolve_remote_mcp_credential_uses_explicit_daemon_token() {
        let url = spawn_fake_server(std::sync::Arc::new(|req| {
            assert_eq!(req.method, "GET");
            assert_eq!(
                req.path,
                "/api/daemon/tasks/task-1/remote-mcp/contrib-1/credential"
            );
            assert_eq!(req.header("Authorization"), "Bearer daemon-token");
            (
                200,
                br#"{"credential_header":"X-Cred","credential":"secret"}"#.to_vec(),
            )
        }));
        let client = Client::new(url);
        let headers = client
            .resolve_remote_mcp_credential(&Ctx::new(), "daemon-token", "task-1", "contrib-1")
            .await
            .unwrap();
        assert_eq!(headers.get("X-Cred").map(String::as_str), Some("secret"));
    }

    #[tokio::test]
    async fn resolve_remote_mcp_credential_routes_plugin_contributions() {
        let url = spawn_fake_server(std::sync::Arc::new(|req| {
            assert_eq!(
                req.path,
                // reqwest's URL normalizer decodes %3A back to ':' inside the
                // path segment; the route selection is what matters here.
                "/api/daemon/tasks/task-1/plugin-mcp/plugin:contrib-7/credential"
            );
            (200, br#"{"credential_header":"","credential":""}"#.to_vec())
        }));
        let client = Client::new(url);
        client
            .resolve_remote_mcp_credential(&Ctx::new(), "t", "task-1", "plugin:contrib-7")
            .await
            .unwrap();
    }

    // TestPostJSONWithRetry_TransientThenSuccess (client_test.go:315–337)
    #[tokio::test]
    async fn post_json_with_retry_transient_then_success() {
        let calls = std::sync::Arc::new(AtomicI32::new(0));
        let counter = calls.clone();
        let url = spawn_fake_server(std::sync::Arc::new(move |_| {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            if n < 3 {
                (502, Vec::new())
            } else {
                (200, b"{}".to_vec())
            }
        }));
        let client = Client::new(url);
        client
            .post_json_with_retry(
                &Ctx::new(),
                "/x",
                serde_json::json!({}),
                &[
                    Duration::from_nanos(1),
                    Duration::from_nanos(1),
                    Duration::from_nanos(1),
                ],
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    // TestFailTask_RetriesOnTransient5xxThenSucceeds (client_test.go:345–365)
    #[tokio::test]
    async fn fail_task_retries_on_transient_5xx_then_succeeds() {
        let calls = std::sync::Arc::new(AtomicI32::new(0));
        let counter = calls.clone();
        let url = spawn_fake_server(std::sync::Arc::new(move |_| {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            if n < 3 {
                (500, Vec::new())
            } else {
                (200, b"{}".to_vec())
            }
        }));
        let client = Client::new(url);
        client
            .fail_task(
                &Ctx::new(),
                "task-1",
                "boom",
                "",
                "",
                "",
                "timeout",
                true,
                "",
                "",
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    // TestPostJSONWithRetry_TransientExhausts (client_test.go:367–389)
    #[tokio::test]
    async fn post_json_with_retry_transient_exhausts() {
        let calls = std::sync::Arc::new(AtomicI32::new(0));
        let counter = calls.clone();
        let url = spawn_fake_server(std::sync::Arc::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            (502, Vec::new())
        }));
        let client = Client::new(url);
        let schedule = [Duration::from_nanos(1), Duration::from_nanos(1)];
        let err = client
            .post_json_with_retry(&Ctx::new(), "/x", serde_json::json!({}), &schedule)
            .await
            .unwrap_err();
        assert!(is_transient_error(&err));
        assert_eq!(calls.load(Ordering::SeqCst) as usize, schedule.len() + 1);
    }

    // TestPostJSONWithRetry_PermanentBailsImmediately (client_test.go:391–410)
    #[tokio::test]
    async fn post_json_with_retry_permanent_bails_immediately() {
        let calls = std::sync::Arc::new(AtomicI32::new(0));
        let counter = calls.clone();
        let url = spawn_fake_server(std::sync::Arc::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            (400, Vec::new())
        }));
        let client = Client::new(url);
        let err = client
            .post_json_with_retry(
                &Ctx::new(),
                "/x",
                serde_json::json!({}),
                &[
                    Duration::from_nanos(1),
                    Duration::from_nanos(1),
                    Duration::from_nanos(1),
                ],
            )
            .await
            .unwrap_err();
        assert!(!is_transient_error(&err));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    // TestPostJSONWithRetry_CtxCancelStopsRetries (client_test.go:412–442)
    #[tokio::test]
    async fn post_json_with_retry_ctx_cancel_stops_retries() {
        let calls = std::sync::Arc::new(AtomicI32::new(0));
        let counter = calls.clone();
        let url = spawn_fake_server(std::sync::Arc::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
            (502, Vec::new())
        }));
        let ctx = Ctx::new();
        let cancel_ctx = ctx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_ctx.cancel_with(CancelCause::Cancelled);
        });
        let client = Client::new(url);
        let start = std::time::Instant::now();
        let result = client
            .post_json_with_retry(
                &ctx,
                "/x",
                serde_json::json!({}),
                &[
                    Duration::from_secs(1),
                    Duration::from_secs(1),
                    Duration::from_secs(1),
                ],
            )
            .await;
        assert!(result.is_err());
        assert!(start.elapsed() < Duration::from_millis(750));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    // TestTerminalReportsCarryRetiredSessionID (client_test.go:479–519)
    #[tokio::test]
    async fn terminal_reports_carry_retired_session_id() {
        for endpoint in [
            "/api/daemon/tasks/task-1/complete",
            "/api/daemon/tasks/task-1/fail",
        ] {
            let captured: std::sync::Arc<AsyncMutex<Option<serde_json::Value>>> =
                std::sync::Arc::new(AsyncMutex::new(None));
            let sink = captured.clone();
            let expected = endpoint.to_string();
            let url = spawn_fake_server(std::sync::Arc::new(move |req| {
                assert_eq!(req.path, expected);
                let body = serde_json::from_slice(&req.body).unwrap_or_default();
                if let Ok(mut guard) = sink.try_lock() {
                    *guard = Some(body);
                }
                (200, b"{}".to_vec())
            }));
            let client = Client::new(url);
            if endpoint.ends_with("complete") {
                client
                    .complete_task(
                        &Ctx::new(),
                        "task-1",
                        "done",
                        "",
                        "",
                        "/tmp/wd",
                        false,
                        "POISONED-S",
                        "",
                    )
                    .await
                    .unwrap();
            } else {
                client
                    .fail_task(
                        &Ctx::new(),
                        "task-1",
                        "boom",
                        "",
                        "/tmp/wd",
                        "",
                        "api_invalid_request",
                        false,
                        "POISONED-S",
                        "",
                    )
                    .await
                    .unwrap();
            }
            let body = captured.try_lock().unwrap().clone().unwrap();
            assert_eq!(body["retired_session_id"], "POISONED-S");
        }
    }

    // TestTerminalReportsOmitEmptyRetiredSessionID (client_test.go:524–538)
    #[tokio::test]
    async fn terminal_reports_omit_empty_retired_session_id() {
        let captured: std::sync::Arc<AsyncMutex<Option<serde_json::Value>>> =
            std::sync::Arc::new(AsyncMutex::new(None));
        let sink = captured.clone();
        let url = spawn_fake_server(std::sync::Arc::new(move |req| {
            let body = serde_json::from_slice(&req.body).unwrap_or_default();
            if let Ok(mut guard) = sink.try_lock() {
                *guard = Some(body);
            }
            (200, b"{}".to_vec())
        }));
        let client = Client::new(url);
        client
            .complete_task(
                &Ctx::new(),
                "task-1",
                "done",
                "",
                "sess-1",
                "/tmp/wd",
                false,
                "",
                "",
            )
            .await
            .unwrap();
        let body = captured.try_lock().unwrap().clone().unwrap();
        assert!(body.get("retired_session_id").is_none());
    }

    // TestTerminalReportsCarryDurableWorkDir (client_test.go:540–581)
    #[tokio::test]
    async fn terminal_reports_carry_durable_work_dir() {
        const DURABLE: &str = "/Users/dev/project";
        enum Kind {
            Complete,
            Fail,
            CancelAck,
        }
        for (kind, endpoint) in [
            (Kind::Complete, "/api/daemon/tasks/task-1/complete"),
            (Kind::Fail, "/api/daemon/tasks/task-1/fail"),
            (Kind::CancelAck, "/api/daemon/tasks/task-1/cancel-ack"),
        ] {
            let captured: std::sync::Arc<AsyncMutex<Option<serde_json::Value>>> =
                std::sync::Arc::new(AsyncMutex::new(None));
            let sink = captured.clone();
            let url = spawn_fake_server(std::sync::Arc::new(move |req| {
                let body = serde_json::from_slice(&req.body).unwrap_or_default();
                if let Ok(mut guard) = sink.try_lock() {
                    *guard = Some(body);
                }
                (200, b"{}".to_vec())
            }));
            let client = Client::new(url);
            match kind {
                Kind::Complete => {
                    client
                        .complete_task(
                            &Ctx::new(),
                            "task-1",
                            "done",
                            "",
                            "",
                            "/tmp/wd",
                            false,
                            "",
                            DURABLE,
                        )
                        .await
                        .unwrap();
                }
                Kind::Fail => {
                    client
                        .fail_task(
                            &Ctx::new(),
                            "task-1",
                            "boom",
                            "",
                            "/tmp/wd",
                            "",
                            "agent_error",
                            false,
                            "",
                            DURABLE,
                        )
                        .await
                        .unwrap();
                }
                Kind::CancelAck => {
                    client
                        .ack_task_cancelled(
                            &Ctx::new(),
                            "task-1",
                            TaskCancelAck {
                                durable_work_dir: DURABLE.to_string(),
                                ..Default::default()
                            },
                        )
                        .await
                        .unwrap();
                }
            }
            let _ = endpoint;
            let body = captured.try_lock().unwrap().clone().unwrap();
            assert_eq!(body["durable_work_dir"], DURABLE);
        }
    }
}
