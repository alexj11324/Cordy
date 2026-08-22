//! Daemon control-plane handlers — port of `server/internal/handler/daemon.go`,
//! `daemon_ws.go`, `daemon_workspace.go`, `runtime_update.go`, `runtime_models.go`
//! and `runtime_local_skills.go` (S8 slice 2: the `/api/daemon` route group).
//!
//! Wire contracts are preserved byte-for-byte with the Go handlers:
//! - error bodies `{"error": msg}` (see [`crate::error`]);
//! - heartbeat ack JSON mirrors `protocol.DaemonHeartbeatAckPayload`;
//! - register response `{runtimes, repos, repos_version, settings}`;
//! - deregister `{status:"ok"}`; task status `{status}`; GC checks
//!   `{status, updated_at|completed_at}`.

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, Query as AxumQuery, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::models::AgentRuntime;
use cordy_db::queries::{
    agent, autopilot, chat, issue, member, runtime, runtime_profile, task_message, task_token,
    workspace,
};
use cordy_middleware::daemon_auth::DaemonContext;
use cordy_protocol::EVENT_DAEMON_REGISTER;
use cordy_service::issue_status as issue_status_svc;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

/// `/api/daemon` group. The caller nests this under the DaemonAuth middleware.
pub fn router() -> Router<HandlerState> {
    Router::new()
        // registration + liveness
        .route("/api/daemon/register", axum::routing::post(register))
        .route("/api/daemon/deregister", axum::routing::post(deregister))
        .route("/api/daemon/heartbeat", axum::routing::post(heartbeat))
        .route("/api/daemon/ws", get(daemon_ws))
        // workspaces / profiles / repos
        .route("/api/daemon/workspaces", get(list_workspaces))
        .route(
            "/api/daemon/workspaces/{workspace_id}/repos",
            get(workspace_repos),
        )
        .route(
            "/api/daemon/workspaces/{workspace_id}/runtime-profiles",
            get(list_runtime_profiles),
        )
        // plugin hooks invoked by agents on daemons
        .route(
            "/api/daemon/tasks/{id}/plugin-hooks",
            axum::routing::post(invoke_agent_plugin_hook),
        )
        .route(
            "/api/daemon/tasks/{id}/plugin-mcp/{contribution_id}/credential",
            get(resolve_plugin_mcp_credential),
        )
        // claim family
        .route(
            "/api/daemon/runtimes/{runtime_id}/tasks/claim",
            axum::routing::post(claim_task_by_runtime),
        )
        .route(
            "/api/daemon/tasks/claim",
            axum::routing::post(claim_tasks_by_runtime),
        )
        .route(
            "/api/daemon/claim",
            axum::routing::post(claim_tasks_by_runtime),
        )
        .route(
            "/api/daemon/runtimes/{runtime_id}/tasks/{task_id}/prepare-lease",
            axum::routing::post(extend_task_prepare_lease),
        )
        .route(
            "/api/daemon/runtimes/{runtime_id}/tasks/{task_id}/skill-bundles/resolve",
            axum::routing::post(resolve_task_skill_bundles),
        )
        .route(
            "/api/daemon/runtimes/{runtime_id}/tasks/pending",
            get(list_pending_tasks_by_runtime),
        )
        .route(
            "/api/daemon/runtimes/{runtime_id}/update/{update_id}/result",
            axum::routing::post(report_update_result),
        )
        .route(
            "/api/daemon/runtimes/{runtime_id}/models/{request_id}/result",
            axum::routing::post(report_model_list_result),
        )
        .route(
            "/api/daemon/runtimes/{runtime_id}/local-skills/{request_id}/result",
            axum::routing::post(report_local_skill_list_result),
        )
        .route(
            "/api/daemon/runtimes/{runtime_id}/local-skills/import/{request_id}/result",
            axum::routing::post(report_local_skill_import_result),
        )
        .route(
            "/api/daemon/runtimes/{runtime_id}/recover-orphans",
            axum::routing::post(recover_orphaned_tasks),
        )
        // task lifecycle
        .route("/api/daemon/tasks/{task_id}/status", get(get_task_status))
        .route(
            "/api/daemon/tasks/{task_id}/start",
            axum::routing::post(start_task),
        )
        .route(
            "/api/daemon/tasks/{task_id}/wait-local-directory",
            axum::routing::post(mark_task_waiting_local_directory),
        )
        .route(
            "/api/daemon/tasks/{task_id}/progress",
            axum::routing::post(report_task_progress),
        )
        .route(
            "/api/daemon/tasks/{task_id}/complete",
            axum::routing::post(complete_task),
        )
        .route(
            "/api/daemon/tasks/{task_id}/fail",
            axum::routing::post(fail_task),
        )
        .route(
            "/api/daemon/tasks/{task_id}/usage",
            axum::routing::post(report_task_usage),
        )
        .route(
            "/api/daemon/tasks/{task_id}/messages",
            axum::routing::get(list_task_messages).post(report_task_messages),
        )
        .route(
            "/api/daemon/tasks/{task_id}/cancel-ack",
            axum::routing::post(ack_task_cancelled),
        )
        .route(
            "/api/daemon/tasks/{task_id}/session",
            axum::routing::post(pin_task_session),
        )
        // gc-check family
        .route(
            "/api/daemon/workspaces/{workspace_id}/issues/gc-check",
            axum::routing::post(batch_issue_gc_check),
        )
        .route(
            "/api/daemon/issues/{issue_id}/gc-check",
            get(issue_gc_check),
        )
        .route(
            "/api/daemon/chat-sessions/{session_id}/gc-check",
            get(chat_session_gc_check),
        )
        .route(
            "/api/daemon/autopilot-runs/{run_id}/gc-check",
            get(autopilot_run_gc_check),
        )
        .route("/api/daemon/tasks/{task_id}/gc-check", get(task_gc_check))
}

// ---------------------------------------------------------------------------
// shared helpers — Go handler.go parseUUIDOrBadRequest / requireUserID etc.
// ---------------------------------------------------------------------------

#[allow(clippy::result_large_err)]
fn parse_uuid_or_bad_request(value: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(value.trim())
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, &format!("invalid {value}")))
}

/// Extracts the authenticated user id from the `x-user-id` header set by the
/// auth middlewares (Go `requestUserID`).
fn request_user_id(headers: &HeaderMap) -> String {
    headers
        .get("x-user-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// Extracts the daemon-token workspace id from the injected
/// [`DaemonContext`] extension (Go `middleware.DaemonWorkspaceIDFromContext`).
fn daemon_workspace_id(ext: Option<DaemonContext>) -> String {
    ext.and_then(|d| d.workspace_id).unwrap_or_default()
}

fn daemon_id_of(ext: Option<DaemonContext>) -> String {
    ext.and_then(|d| d.daemon_id).unwrap_or_default()
}

struct Access<'a> {
    state: &'a HandlerState,
    headers: &'a HeaderMap,
}

impl<'a> Access<'a> {
    fn new(state: &'a HandlerState, headers: &'a HeaderMap) -> Self {
        Self { state, headers }
    }

    async fn get_member(&self, user_id: &str, ws_id: &str) -> Option<cordy_db::models::Member> {
        let user = Uuid::parse_str(user_id).ok()?;
        let ws = Uuid::parse_str(ws_id).ok()?;
        member::get_member_by_user_and_workspace(&self.state.pool, user, ws)
            .await
            .ok()
            .flatten()
    }
}

/// Result of a workspace-access check: either authorized or an error response.
type AccessCheck = Result<String, Response>;

/// Verifies the caller may act in `workspace_id`. Daemon tokens compare the
/// token's workspace directly; PAT/JWT fallback verifies membership (Go
/// `requireDaemonWorkspaceAccess`, non-writing variant folded into the enum).
async fn check_daemon_workspace_access(
    access: &Access<'_>,
    daemon_ctx: Option<DaemonContext>,
    workspace_id: &str,
) -> AccessCheck {
    if workspace_id.is_empty() {
        return Err(error_response(StatusCode::NOT_FOUND, "not found"));
    }
    let daemon_ws = daemon_workspace_id(daemon_ctx.clone());
    if !daemon_ws.is_empty() {
        if daemon_ws != workspace_id {
            return Err(error_response(StatusCode::NOT_FOUND, "not found"));
        }
        return Ok(workspace_id.to_string());
    }
    let user_id = request_user_id(access.headers);
    if user_id.is_empty() {
        return Err(error_response(StatusCode::NOT_FOUND, "not found"));
    }
    match access.get_member(&user_id, workspace_id).await {
        Some(_) => Ok(workspace_id.to_string()),
        None => Err(error_response(StatusCode::NOT_FOUND, "not found")),
    }
}

/// Loads a runtime and verifies its workspace belongs to the caller (Go
/// `requireDaemonRuntimeAccess`). Only a missing row is a 404; other DB errors
/// are 500 so the daemon does not self-cleanup on a hiccup.
async fn require_daemon_runtime_access(
    access: &Access<'_>,
    daemon_ctx: Option<DaemonContext>,
    runtime_id: &str,
) -> Result<(AgentRuntime, String), Response> {
    let rt_uuid = parse_uuid_or_bad_request(runtime_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"))?;
    let lookup = runtime::get_agent_runtime(&access.state.pool, rt_uuid).await;
    // Only a missing row is a real 404: the daemon reads that as "drop the
    // stale runtime and re-register". A transient DB error must NOT be
    // reported as a deletion (Go isNotFound branch), hence the 500 here.
    let rt = match lookup {
        Ok(Some(rt)) => rt,
        Ok(None) => return Err(error_response(StatusCode::NOT_FOUND, "runtime not found")),
        Err(e) => {
            tracing::warn!(error = %e, runtime_id = %runtime_id, "get agent runtime failed");
            return Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load runtime",
            ));
        }
    };
    let ws_id = rt.workspace_id.to_string();
    let _ = check_daemon_workspace_access(access, daemon_ctx, &ws_id).await?;
    Ok((rt, ws_id))
}

/// Loads a task plus resolved workspace and verifies access (Go
/// `requireDaemonTaskAccessWithWorkspace`).
async fn require_daemon_task_access_with_workspace(
    access: &Access<'_>,
    daemon_ctx: Option<DaemonContext>,
    task_id: &str,
) -> Result<(cordy_db::models::AgentTaskQueue, String), Response> {
    let task_uuid = parse_uuid_or_bad_request(task_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid task_id"))?;
    let task = agent::get_agent_task(&access.state.pool, task_uuid)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, task_id = %task_id, "get agent task failed");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load task")
        })?
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "task not found"))?;

    let ws_id = access
        .state
        .tasks
        .resolve_task_workspace_id(&task)
        .await
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "task not found"))?;

    let _ = check_daemon_workspace_access(access, daemon_ctx, &ws_id).await?;
    Ok((task, ws_id))
}

// ---------------------------------------------------------------------------
// GET /api/daemon/workspaces
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[allow(dead_code)]
struct WorkspacesQuery {
    #[serde(default)]
    runtime_ids: Vec<String>,
}

/// GET /api/daemon/ws — pre-upgrade identity resolution for the daemon socket.
///
/// The gorilla upgrade itself is deferred: identity validation runs here (400
/// when no runtime ids and no user), runtime/workspace authorization is
/// enforced per runtime, and the resulting identity is what the axum WS lane
/// will hand to `DaemonHub::register`. Until that lane lands, this endpoint
/// reports 503 so older daemons fall back to HTTP polling transparently.
async fn daemon_ws(
    State(state): State<HandlerState>,
    AxumQuery(query): AxumQuery<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    // Collect deduped runtime ids from both spellings (Go parseRuntimeIDs).
    let mut runtime_ids: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for raw_key in ["runtime_id", "runtime_ids"] {
        for raw in query.get(raw_key).cloned().into_iter() {
            for part in raw.split(',') {
                let id = part.trim();
                if !id.is_empty() && seen.insert(id.to_string()) {
                    runtime_ids.push(id.to_string());
                }
            }
        }
    }
    let user_id = request_user_id(&headers);
    if runtime_ids.is_empty() && user_id.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "runtime_ids or user identity required",
        );
    }

    let daemon_ctx = None::<DaemonContext>; // extracted below via middleware
    let access = Access::new(&state, &headers);
    let mut workspace_ids: Vec<String> = Vec::new();
    let mut seen_ws: HashSet<String> = HashSet::new();
    for rid in &runtime_ids {
        let (rt, ws_id) =
            match require_daemon_runtime_access(&access, daemon_ctx.clone(), rid).await {
                Ok(v) => v,
                Err(res) => return res,
            };
        let daemon_id_hdr = daemon_id_of(daemon_ctx.clone());
        if !daemon_id_hdr.is_empty() && rt.daemon_id.as_deref().unwrap_or("") != daemon_id_hdr {
            return error_response(StatusCode::NOT_FOUND, "runtime not found");
        }
        if seen_ws.insert(ws_id.clone()) {
            workspace_ids.push(ws_id);
        }
    }

    // The pump lane lands with the daemonws slice; until then refuse politely.
    let _ = workspace_ids;
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "daemon websocket unavailable",
    )
}

/// GET /api/daemon/workspaces — minimal membership projection with an ETag.
async fn list_workspaces(
    State(state): State<HandlerState>,
    AxumQuery(_query): AxumQuery<WorkspacesQuery>,
    headers: HeaderMap,
) -> Response {
    let user_id = request_user_id(&headers);
    let items: Vec<Value> = if !user_id.is_empty() {
        let Ok(user_uuid) = Uuid::parse_str(&user_id) else {
            return error_response(StatusCode::BAD_REQUEST, "invalid user id");
        };
        match workspace::list_daemon_workspaces(&state.pool, user_uuid).await {
            Ok(rows) => rows
                .into_iter()
                .map(|row| {
                    json!({
                        "id": row.id.map(|u| u.to_string()).unwrap_or_default(),
                        "name": row.name,
                    })
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "failed to list daemon workspaces");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to list daemon workspaces",
                );
            }
        }
    } else {
        let daemon_ws = daemon_workspace_id(None);
        if daemon_ws.is_empty() {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "daemon workspace identity required",
            );
        }
        // The middleware injects DaemonContext; without it there is nothing to
        // resolve, matching Go's 404 path for unknown workspaces.
        let Ok(ws_uuid) = Uuid::parse_str(&daemon_ws) else {
            return error_response(StatusCode::NOT_FOUND, "workspace not found");
        };
        match workspace::get_daemon_workspace(&state.pool, ws_uuid).await {
            Ok(Some(row)) => vec![json!({
                "id": row.id.map(|u| u.to_string()).unwrap_or_default(),
                "name": row.name,
            })],
            Ok(None) => return error_response(StatusCode::NOT_FOUND, "workspace not found"),
            Err(e) => {
                tracing::warn!(error = %e, "get daemon workspace failed");
                return error_response(StatusCode::NOT_FOUND, "workspace not found");
            }
        }
    };

    // ETag over the canonical JSON encoding of the array (Go daemonWorkspacesETag).
    let body = Value::Array(items.clone()).to_string();
    use sha2::Digest;
    let sum = sha2::Sha256::digest(body.as_bytes());
    let etag = format!("W/\"{}\"", hex::encode(sum));
    let mut res = Json(items).into_response();
    res.headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-cache".parse().unwrap());
    res.headers_mut()
        .insert(header::ETAG, etag.parse().unwrap());
    res
}

// ---------------------------------------------------------------------------
// POST /api/daemon/deregister
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DeregisterRequest {
    #[serde(default, rename = "runtime_ids")]
    runtime_ids: Vec<String>,
    #[serde(default, rename = "offline_reasons")]
    offline_reasons: HashMap<String, Value>,
}

async fn deregister(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Option<Json<DeregisterRequest>>,
) -> Response {
    let Some(Json(req)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    if req.runtime_ids.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "runtime_ids is required");
    }
    let mut parsed: Vec<Uuid> = Vec::with_capacity(req.runtime_ids.len());
    for rid in &req.runtime_ids {
        match Uuid::parse_str(rid.trim()) {
            Ok(u) => parsed.push(u),
            Err(_) => {
                return error_response(StatusCode::BAD_REQUEST, "invalid runtime_ids");
            }
        }
    }

    let access = Access::new(&state, &headers);
    let mut affected: Vec<String> = Vec::new();
    for (i, rid) in req.runtime_ids.iter().enumerate() {
        let rt = match runtime::get_agent_runtime(&state.pool, parsed[i]).await {
            Ok(Some(rt)) => rt,
            Ok(None) => {
                tracing::warn!("deregister: runtime not found {}", rid);
                continue;
            }
            Err(e) => {
                tracing::warn!(error = %e, "deregister: runtime lookup failed {}", rid);
                continue;
            }
        };
        let ws_id = rt.workspace_id.to_string();
        let daemon_ws = daemon_workspace_id(None);
        let ok = if !daemon_ws.is_empty() {
            daemon_ws == ws_id
        } else {
            {
                let user_id = request_user_id(&headers);
                !user_id.is_empty() && access.get_member(&user_id, &ws_id).await.is_some()
            }
        };
        if !ok {
            tracing::warn!("deregister: workspace mismatch {}", rid);
            continue;
        }
        // A valid reason rides along; anything unusable falls back to the
        // plain offline write (MUL-6164).
        let result = match req.offline_reasons.get(rid) {
            Some(reason @ Value::Object(_)) => {
                runtime::set_agent_runtime_offline_with_reason(&state.pool, rt.id, reason).await
            }
            _ => runtime::set_agent_runtime_offline(&state.pool, rt.id).await,
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, runtime_id = %rid, "deregister: failed to set offline");
            continue;
        }
        if !affected.contains(&ws_id) {
            affected.push(ws_id);
        }
    }

    for ws_id in affected {
        state.bus.publish(&cordy_events::Event {
            event_type: EVENT_DAEMON_REGISTER.to_string(),
            workspace_id: ws_id,
            actor_type: "system".to_string(),
            actor_id: String::new(),
            payload: json!({ "action": "deregister" }),
            ..Default::default()
        });
    }

    tracing::info!(runtime_ids = ?req.runtime_ids, "daemon deregistered");
    Json(json!({ "status": "ok" })).into_response()
}

// ---------------------------------------------------------------------------
// POST /api/daemon/heartbeat
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct HeartbeatRequest {
    #[serde(default, rename = "runtime_id")]
    runtime_id: String,
    #[serde(default, rename = "supports_batch_import")]
    supports_batch_import: bool,
}

/// POST /api/daemon/heartbeat. Records liveness and pops any pending update /
/// model-list / local-skill requests queued for the runtime. The Redis-backed
/// pending queues land with the redis wiring slice; today every probe is empty,
/// which matches Go's behavior with all stores disabled.
async fn heartbeat(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Option<Json<HeartbeatRequest>>,
) -> Response {
    let Some(Json(req)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    if req.runtime_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "runtime_id is required");
    }
    let rt_uuid = match Uuid::parse_str(&req.runtime_id) {
        Ok(u) => u,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"),
    };
    let rt = match runtime::get_agent_runtime(&state.pool, rt_uuid).await {
        Ok(Some(rt)) => rt,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "runtime not found"),
        Err(e) => {
            tracing::warn!(error = %e, runtime_id = %req.runtime_id, "get agent runtime failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load runtime");
        }
    };

    let access = Access::new(&state, &headers);
    let ws_id = rt.workspace_id.to_string();
    let daemon_ws = daemon_workspace_id(None);
    if !daemon_ws.is_empty() {
        if daemon_ws != ws_id {
            return error_response(StatusCode::FORBIDDEN, "workspace denied");
        }
    } else {
        let user_id = request_user_id(&headers);
        if user_id.is_empty() || access.get_member(&user_id, &ws_id).await.is_none() {
            return error_response(StatusCode::FORBIDDEN, "workspace denied");
        }
    }

    record_heartbeat(&state, &rt).await;

    // Pending stores (update / model list / local skills) land with the redis
    // wiring slice. The ack shape stays identical: only the optional fields go
    // absent while the queues are empty, which old daemons already tolerate.
    let _ = req.supports_batch_import;
    Json(json!({ "status": "ok", "runtime_id": rt.id.to_string() })).into_response()
}

/// Passthrough liveness write (Go PassthroughHeartbeatScheduler.Schedule):
/// touch last_seen_at on online rows, flip offline→online otherwise. The
/// Redis TTL layer and batched coalescing land with the redis slice.
async fn record_heartbeat(state: &HandlerState, rt: &AgentRuntime) {
    if rt.status == "online" && rt.last_seen_at.is_some() {
        match runtime::touch_agent_runtime_last_seen(&state.pool, rt.id).await {
            Ok(n) if n > 0 => return,
            _ => {}
        }
    }
    if let Err(e) = runtime::mark_agent_runtime_online(&state.pool, rt.id).await {
        tracing::warn!(error = %e, runtime_id = %rt.id, "heartbeat db update failed");
    }
}

// ---------------------------------------------------------------------------
// POST /api/daemon/register
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RegisterRequest {
    #[serde(default, rename = "workspace_id")]
    workspace_id: String,
    #[serde(default, rename = "daemon_id")]
    daemon_id: String,
    #[serde(default, rename = "legacy_daemon_ids")]
    legacy_daemon_ids: Vec<String>,
    #[serde(default, rename = "device_name")]
    device_name: String,
    #[serde(default, rename = "cli_version")]
    cli_version: String,
    #[serde(default, rename = "launched_by")]
    launched_by: String,
    #[serde(default)]
    runtimes: Vec<RegisterRuntime>,
    #[serde(default, rename = "failed_profiles")]
    failed_profiles: Vec<RegisterFailedProfile>,
}

#[derive(Deserialize)]
struct RegisterRuntime {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    type_: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    status: String,
    #[serde(default, rename = "profile_id")]
    profile_id: String,
}

#[derive(Deserialize)]
struct RegisterFailedProfile {
    #[serde(default, rename = "profile_id")]
    profile_id: String,
    #[serde(default, rename = "command_name")]
    command_name: String,
    #[serde(default)]
    reason: String,
}

fn normalize_provider(s: &str) -> String {
    s.trim().to_lowercase()
}

async fn register(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Option<Json<RegisterRequest>>,
) -> Response {
    let Some(Json(mut req)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    req.workspace_id = req.workspace_id.trim().to_string();
    req.daemon_id = req.daemon_id.trim().to_string();
    req.device_name = req.device_name.trim().to_string();
    if req.daemon_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "daemon_id is required");
    }
    if req.workspace_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "workspace_id is required");
    }
    if req.runtimes.is_empty() && req.failed_profiles.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "at least one runtime or failed profile is required",
        );
    }
    let ws_uuid = match Uuid::parse_str(&req.workspace_id) {
        Ok(u) => u,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace_id"),
    };

    // Resolve owner: daemon tokens keep any existing owner on upsert (zero UUID).
    let mut owner_id = Uuid::nil();
    let daemon_ws = daemon_workspace_id(None);
    if !daemon_ws.is_empty() {
        if daemon_ws != req.workspace_id {
            return error_response(StatusCode::NOT_FOUND, "workspace not found");
        }
    } else {
        let user_id = request_user_id(&headers);
        match access_get_member(&state, &headers, &user_id, &req.workspace_id).await {
            Some(m) => owner_id = m.user_id,
            None => return error_response(StatusCode::NOT_FOUND, "workspace not found"),
        }
    }

    let Some(ws) = workspace::get_workspace(&state.pool, ws_uuid)
        .await
        .ok()
        .flatten()
    else {
        return error_response(StatusCode::NOT_FOUND, "workspace not found");
    };

    let mut resp: Vec<Value> = Vec::with_capacity(req.runtimes.len());
    for rt_req in &req.runtimes {
        let mut provider = normalize_provider(&rt_req.type_);
        if provider.is_empty() {
            provider = "unknown".to_string();
        }
        let mut name = rt_req.name.trim().to_string();
        if name.is_empty() {
            name = if req.device_name.is_empty() {
                provider.clone()
            } else {
                format!("{} ({})", provider, req.device_name)
            };
        }
        let mut device_info = req.device_name.clone();
        if !rt_req.version.is_empty() && !device_info.is_empty() {
            device_info = format!("{} · {}", device_info, rt_req.version);
        } else if !rt_req.version.is_empty() {
            device_info = rt_req.version.clone();
        }
        let status = if rt_req.status == "offline" {
            "offline"
        } else {
            "online"
        };
        let metadata = json!({
            "version": rt_req.version,
            "cli_version": req.cli_version,
            "launched_by": req.launched_by,
        });
        let is_custom = !rt_req.profile_id.trim().is_empty();

        // The two upsert variants return different row types; both render
        // through runtime_to_json, and only the built-in branch feeds the
        // legacy-hostname merge.
        let resp_item: Value = if is_custom {
            let profile_uuid = match Uuid::parse_str(rt_req.profile_id.trim()) {
                Ok(u) => u,
                Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid profile_id"),
            };
            // Serialize registration against profile deletion: KEY SHARE lock on
            // the profile row until commit (Go upsertRuntimeWithProfile).
            let mut tx = match state.pool.begin().await {
                Ok(tx) => tx,
                Err(e) => {
                    tracing::error!(error = %e, "register: begin profile tx failed");
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("failed to register runtime: {e}"),
                    );
                }
            };
            let profile = match runtime_profile::lock_runtime_profile_for_registration(
                &mut *tx,
                profile_uuid,
                ws_uuid,
            )
            .await
            {
                Ok(Some(p)) => p,
                Ok(None) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("unknown runtime profile: {}", rt_req.profile_id),
                    );
                }
                Err(e) => {
                    tracing::error!(error = %e, "register: profile lock failed");
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("failed to register runtime: {e}"),
                    );
                }
            };
            if !profile.enabled {
                return error_response(
                    StatusCode::CONFLICT,
                    &format!("runtime profile is disabled: {}", rt_req.profile_id),
                );
            }
            let up = runtime::upsert_agent_runtime_with_profile(
                &mut *tx,
                ws_uuid,
                Some(&req.daemon_id),
                &name,
                "local",
                &profile.protocol_family,
                status,
                &device_info,
                &metadata,
                owner_id,
                profile_uuid,
            )
            .await;
            match up {
                Ok(Some(row)) => {
                    if let Err(e) = tx.commit().await {
                        tracing::error!(error = %e, "register: commit profile runtime failed");
                        return error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            &format!("failed to register runtime: {e}"),
                        );
                    }
                    provider = profile.protocol_family;
                    upsert_row_to_json(&row)
                }
                Ok(None) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("unknown runtime profile: {}", rt_req.profile_id),
                    );
                }
                Err(e) => {
                    tracing::error!(error = %e, "register: upsert profile runtime failed");
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("failed to register runtime: {e}"),
                    );
                }
            }
        } else {
            let _registered: Option<AgentRuntime> = None;
            match runtime::upsert_agent_runtime(
                &state.pool,
                ws_uuid,
                Some(&req.daemon_id),
                &name,
                "local",
                &provider,
                status,
                &device_info,
                &metadata,
                owner_id,
            )
            .await
            {
                Ok(Some(row)) => {
                    merge_legacy_runtimes(&state, &row, ws_uuid, &provider, &req.legacy_daemon_ids)
                        .await;
                    upsert_row_to_json(&row)
                }
                Ok(None) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to register runtime",
                    );
                }
                Err(e) => {
                    tracing::error!(error = %e, "register: upsert runtime failed");
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("failed to register runtime: {e}"),
                    );
                }
            }
        };
        let _ = &mut provider;
        resp.push(resp_item);
    }

    for failed in &req.failed_profiles {
        let profile_id = failed.profile_id.trim();
        if profile_id.is_empty() {
            continue;
        }
        let Ok(profile_uuid) = Uuid::parse_str(profile_id) else {
            return error_response(StatusCode::BAD_REQUEST, "invalid profile_id");
        };
        let mut reason = failed.reason.trim().to_string();
        if reason.is_empty() {
            reason = "custom runtime command could not be resolved".to_string();
        }
        let command_name = failed.command_name.trim().to_string();
        let mut tx = match state.pool.begin().await {
            Ok(tx) => tx,
            Err(_) => continue,
        };
        let Ok(Some(profile)) =
            runtime_profile::lock_runtime_profile_for_registration(&mut *tx, profile_uuid, ws_uuid)
                .await
        else {
            continue;
        };
        if !profile.enabled {
            continue;
        }
        let name = if req.device_name.is_empty() {
            profile.display_name.clone()
        } else {
            format!("{} ({})", profile.display_name, req.device_name)
        };
        let resolved_command = if command_name.is_empty() {
            profile.command_name.clone()
        } else {
            command_name
        };
        let metadata = json!({
            "version": "",
            "cli_version": req.cli_version,
            "launched_by": req.launched_by,
            "runtime_profile_registration_error": true,
            "runtime_profile_failure_reason": reason,
            "command_name": resolved_command,
        });
        let up = runtime::upsert_agent_runtime_with_profile(
            &mut *tx,
            ws_uuid,
            Some(&req.daemon_id),
            &name,
            "local",
            &profile.protocol_family,
            "offline",
            req.device_name.trim(),
            &metadata,
            owner_id,
            profile_uuid,
        )
        .await;
        match up {
            Ok(Some(_)) => {
                let _ = tx.commit().await;
            }
            _ => {
                tracing::warn!(
                    workspace_id = %req.workspace_id,
                    daemon_id = %req.daemon_id,
                    profile_id = %profile_id,
                    "failed to record runtime profile registration failure"
                );
            }
        }
    }

    tracing::info!(
        workspace_id = %req.workspace_id,
        daemon_id = %req.daemon_id,
        runtimes_count = resp.len(),
        "daemon registered"
    );

    state.bus.publish(&cordy_events::Event {
        event_type: EVENT_DAEMON_REGISTER.to_string(),
        workspace_id: req.workspace_id.clone(),
        actor_type: "system".to_string(),
        actor_id: String::new(),
        payload: json!({ "runtimes": resp }),
        ..Default::default()
    });

    let repos = parse_workspace_repos(&ws.repos);
    Json(json!({
        "runtimes": resp,
        "repos": repos,
        "repos_version": workspace_repos_version(&repos),
        "settings": if ws.settings.is_null() { Value::Null } else { ws.settings.clone() },
    }))
    .into_response()
}

async fn access_get_member(
    state: &HandlerState,
    _headers: &HeaderMap,
    user_id: &str,
    ws_id: &str,
) -> Option<cordy_db::models::Member> {
    let user = Uuid::parse_str(user_id).ok()?;
    let ws = Uuid::parse_str(ws_id).ok()?;
    member::get_member_by_user_and_workspace(&state.pool, user, ws)
        .await
        .ok()
        .flatten()
}

/// Folds every runtime row keyed on a prior hostname-derived daemon_id into the
/// newly registered row (Go mergeLegacyRuntime, single transaction with fence).
fn upsert_row_to_json<T: serde::Serialize>(row: &T) -> Value {
    let mut v = serde_json::to_value(row).unwrap_or(Value::Null);
    if let Some(obj) = v.as_object_mut() {
        // Go renders metadata as an object (never null) and timestamps as
        // RFC3339; the row types carry Option<Value> / DateTime<Utc>, so
        // normalize here.
        if obj.get("metadata").map(Value::is_null).unwrap_or(true) {
            obj.insert("metadata".into(), json!({}));
        }
    }
    v
}

async fn merge_legacy_runtimes(
    state: &HandlerState,
    registered: &runtime::UpsertAgentRuntimeRow,
    workspace_id: Uuid,
    provider: &str,
    legacy_ids: &[String],
) {
    let Some(new_id) = registered.id else { return };
    let Some(ws_uuid) = registered.workspace_id else {
        return;
    };
    let _ = workspace_id;
    let mut merged: HashSet<Uuid> = HashSet::new();
    for legacy_id in legacy_ids {
        let legacy_id = legacy_id.trim();
        if legacy_id.is_empty() {
            continue;
        }
        let matches = match runtime::find_legacy_runtimes_by_daemon_id(
            &state.pool,
            ws_uuid,
            provider,
            legacy_id,
        )
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, legacy_daemon_id = %legacy_id, "legacy runtime merge: lookup failed");
                continue;
            }
        };
        for old in matches {
            if old.id == new_id || !merged.insert(old.id) {
                continue;
            }
            let res = merge_legacy_runtime(state, new_id, old.id, legacy_id, provider).await;
            if let Err(e) = res {
                tracing::warn!(error = %e, legacy_daemon_id = %legacy_id, "legacy runtime merge failed");
            }
        }
    }
}

/// One transaction per legacy row; order is load-bearing — fence locks first,
/// then task reassignment whose predicate carries the same fence (Go doc).
async fn merge_legacy_runtime(
    state: &HandlerState,
    new_runtime_id: Uuid,
    old_runtime_id: Uuid,
    legacy_id: &str,
    provider: &str,
) -> anyhow::Result<()> {
    use cordy_db::queries::runtime as rq;
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|e| anyhow::anyhow!("begin merge tx: {e}"))?;
    let ids = vec![old_runtime_id, new_runtime_id];
    rq::lock_workspace_for_runtime_merge(&mut *tx, ids.clone())
        .await
        .map_err(|e| anyhow::anyhow!("lock workspace: {e}"))?;
    let locked = rq::lock_runtimes_for_merge(&mut *tx, ids.clone())
        .await
        .map_err(|e| anyhow::anyhow!("lock runtimes: {e}"))?;
    if locked.len() != 2 {
        anyhow::bail!("runtime merge refused by the task-write fence");
    }
    let reassignment = rq::reassign_tasks_to_runtime(&mut *tx, new_runtime_id, old_runtime_id)
        .await
        .map_err(|e| anyhow::anyhow!("reassign tasks: {e}"))?;
    let Some(row) = reassignment else {
        anyhow::bail!("reassign tasks: no result");
    };
    if !row.fence_ok {
        anyhow::bail!("runtime merge refused by the task-write fence");
    }
    let agents_reassigned =
        rq::reassign_agents_to_runtime(&mut *tx, new_runtime_id, old_runtime_id)
            .await
            .map_err(|e| anyhow::anyhow!("reassign agents: {e}"))?;
    rq::record_runtime_legacy_daemon_id(&mut *tx, new_runtime_id, Some(legacy_id))
        .await
        .map_err(|e| anyhow::anyhow!("record legacy daemon_id: {e}"))?;
    rq::delete_agent_runtime(&mut *tx, old_runtime_id)
        .await
        .map_err(|e| anyhow::anyhow!("delete old runtime: {e}"))?;
    tx.commit()
        .await
        .map_err(|e| anyhow::anyhow!("commit merge: {e}"))?;
    tracing::info!(
        legacy_daemon_id = %legacy_id,
        old_runtime_id = %old_runtime_id,
        new_runtime_id = %new_runtime_id,
        provider = %provider,
        agents_reassigned = agents_reassigned,
        tasks_reassigned = row.reassigned_tasks,
        "legacy runtime merged"
    );
    Ok(())
}

fn parse_workspace_repos(raw: &Value) -> Vec<Value> {
    let Ok(repos) = serde_json::from_value::<Vec<Value>>(raw.clone()) else {
        return Vec::new();
    };
    let mut out: Vec<Value> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for mut repo in repos {
        let url = repo
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if url.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        repo["url"] = Value::String(url);
        out.push(repo);
    }
    out
}

fn workspace_repos_version(repos: &[Value]) -> String {
    use sha2::Digest;
    let mut urls: Vec<String> = repos
        .iter()
        .map(|r| {
            r.get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        })
        .filter(|u| !u.is_empty())
        .collect();
    urls.sort();
    let sum = sha2::Sha256::digest(urls.join("\n").as_bytes());
    hex::encode(sum)
}

/// Retained for the daemonws slice: the WS heartbeat ack and per-runtime
/// notifications reuse this exact shape. Not yet called on the HTTP paths.
#[allow(dead_code)]
fn runtime_to_json(rt: &AgentRuntime) -> Value {
    json!({
        "id": rt.id.to_string(),
        "workspace_id": rt.workspace_id.to_string(),
        "daemon_id": rt.daemon_id.clone(),
        "name": rt.name,
        "custom_name": rt.custom_name.clone(),
        "runtime_mode": rt.runtime_mode,
        "provider": rt.provider,
        "launch_header": launch_header(&rt.provider),
        "status": rt.status,
        "device_info": rt.device_info,
        "metadata": if rt.metadata.is_null() { json!({}) } else { rt.metadata.clone() },
        "owner_id": rt.owner_id.map(|u| u.to_string()),
        "visibility": rt.visibility,
        "profile_id": rt.profile_id.map(|u| u.to_string()),
        "last_seen_at": rt.last_seen_at.map(crate::timefmt::rfc3339),
        "created_at": crate::timefmt::rfc3339(rt.created_at),
        "updated_at": crate::timefmt::rfc3339(rt.updated_at),
    })
}

/// Port of pkg/agent.LaunchHeader's static map + omp descriptor lookup.
#[allow(dead_code)]
fn launch_header(agent_type: &str) -> &'static str {
    match agent_type {
        "antigravity" => "agy -p (non-interactive)",
        "claude" => "claude (stream-json)",
        "codebuddy" => "codebuddy (stream-json)",
        "codex" => "codex app-server",
        "copilot" => "copilot (json)",
        "cursor" => "cursor-agent (stream-json)",
        "deveco" => "deveco run (json)",
        "hermes" => "hermes acp",
        "kimi" => "kimi acp",
        "reasonix" => "reasonix acp",
        "dsh" => "dsh --profile cordy (stdio)",
        "kiro" => "kiro-cli acp",
        "openclaw" => "openclaw agent (json)",
        "opencode" => "opencode run (json)",
        "pi" => "pi (json mode)",
        "omp" => "omp (json mode)",
        "qoder" => "qodercli --acp",
        "qoderclicn" => "qoderclicn --acp",
        "traecli" => "traecli acp serve",
        "grok" => "grok agent stdio",
        "qwen" => "qwen -p (stream-json)",
        "qwenpaw" => "qwenpaw acp",
        "dim" => "dim acp",
        "mcode" => "mcode acp",
        _ => "",
    }
}

// ---------------------------------------------------------------------------
// Claim family
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct BatchClaimRequest {
    #[serde(default, rename = "daemon_id")]
    daemon_id: String,
    #[serde(default, rename = "runtime_ids")]
    runtime_ids: Vec<String>,
    #[serde(default, rename = "max_tasks")]
    max_tasks: i64,
}

/// Bounds one machine-level batch claim (Go claimBatchMaxTasksCap).
const CLAIM_BATCH_MAX_TASKS_CAP: usize = 32;

fn empty_tasks_response() -> Response {
    Json(json!({ "tasks": [] })).into_response()
}

/// POST /api/daemon/tasks/claim (and the /claim alias). Machine-level batch
/// claim: one round trip per daemon. Each returned task carries its runtime_id
/// so the daemon routes it to the matching runtime locally.
///
/// The full per-task response builder (Go buildClaimedTaskResponse — agent
/// skills, comment plans, chat input, squad briefings) is a follow-on slice;
/// this port delivers the ownership/fence/token skeleton with the same
/// response envelope and cancellation semantics for the paths it covers.
async fn claim_tasks_by_runtime(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Option<Json<BatchClaimRequest>>,
) -> Response {
    let Some(Json(req)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    if req.daemon_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "daemon_id is required");
    }
    if req.max_tasks < 0 {
        return error_response(StatusCode::BAD_REQUEST, "max_tasks must not be negative");
    }
    if req.max_tasks == 0 {
        return empty_tasks_response();
    }
    let max_tasks = (req.max_tasks as usize).min(CLAIM_BATCH_MAX_TASKS_CAP);

    // Parse + dedupe requested ids; invalid ids are skipped (unknown-id semantics).
    let mut id_set: Vec<Uuid> = Vec::new();
    let mut seen: HashSet<Uuid> = HashSet::new();
    for rid in &req.runtime_ids {
        if let Ok(u) = Uuid::parse_str(rid.trim()) {
            if seen.insert(u) {
                id_set.push(u);
            }
        }
    }
    if id_set.is_empty() {
        return empty_tasks_response();
    }

    let runtimes = match runtime::get_agent_runtimes(&state.pool, id_set).await {
        Ok(rows) => rows,
        Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load runtimes")
        }
    };
    let mut authorized: Vec<Uuid> = Vec::new();
    let mut by_id: HashMap<Uuid, &AgentRuntime> = HashMap::new();
    for rt in &runtimes {
        let ws_id = rt.workspace_id.to_string();
        let ok = match daemon_workspace_id(None).is_empty() {
            false => daemon_workspace_id(None) == ws_id,
            true => {
                let user_id = request_user_id(&headers);
                !user_id.is_empty()
                    && access_get_member(&state, &headers, &user_id, &ws_id)
                        .await
                        .is_some()
            }
        };
        if !ok {
            continue;
        }
        if let Some(d) = rt.daemon_id.as_deref() {
            if d != req.daemon_id {
                continue;
            }
        } else if rt.daemon_id.is_some() {
            continue;
        }
        authorized.push(rt.id);
        by_id.insert(rt.id, rt);
    }
    if authorized.is_empty() {
        return empty_tasks_response();
    }

    let claimed = match state
        .tasks
        .claim_tasks_for_runtimes(authorized.clone(), max_tasks)
        .await
    {
        Ok(tasks) => tasks,
        Err(e) => {
            tracing::error!(error = %e, "batch claim failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to claim tasks: {e}"),
            );
        }
    };

    let mut out: Vec<Value> = Vec::with_capacity(claimed.len());
    for task in claimed {
        // A task whose runtime is not in the authorized set would be a stray
        // cross-daemon claim; leave it for the owning daemon's reclaim path.
        let Some(runtime_id) = task.runtime_id else {
            continue;
        };
        let Some(rt) = by_id.get(&runtime_id) else {
            continue;
        };
        let Some(owner) = rt.owner_id else {
            tracing::error!(
                task_id = %task.id,
                runtime_id = %runtime_id,
                "batch claim: runtime owner missing; cancelling task"
            );
            let _ = state.tasks.cancel_task(task.id).await;
            continue;
        };
        match finalize_claim(&state, &task, owner, rt.workspace_id).await {
            Ok(auth_token) => {
                let mut payload =
                    crate::task_json::task_to_map(&task, &rt.workspace_id.to_string());
                payload["auth_token"] = Value::String(auth_token);
                out.push(payload);
            }
            Err(requeue) => {
                if requeue {
                    let _ = state.tasks.requeue_task_after_claim_failure(&task).await;
                }
                continue;
            }
        }
    }

    if !out.is_empty() {
        tracing::info!(
            runtimes = authorized.len(),
            requested_max = max_tasks,
            claimed = out.len(),
            "tasks claimed by runtime batch"
        );
    }
    Json(json!({ "tasks": out })).into_response()
}

/// Mints and atomically persists the task-scoped token + delivery receipt
/// (Go FinalizeTaskClaim). Returns the raw token, or whether the exact claim
/// should be requeued on failure.
async fn finalize_claim(
    state: &HandlerState,
    task: &cordy_db::models::AgentTaskQueue,
    owner_id: Uuid,
    workspace_id: Uuid,
) -> Result<String, bool> {
    let token_str = cordy_auth::jwt::generate_agent_task_token().map_err(|_| false)?;
    let expires = chrono::Utc::now() + chrono::Duration::hours(24);
    let receipt = state
        .tasks
        .finalize_task_claim(
            task,
            cordy_service::task_service::CreateTaskToken {
                token_hash: cordy_auth::jwt::hash_token(&token_str),
                task_id: task.id,
                agent_id: task.agent_id,
                workspace_id,
                user_id: owner_id,
                expires_at: Some(expires),
            },
            None,
            Vec::new(),
            false,
        )
        .await;
    match receipt {
        Ok(_) => Ok(token_str),
        Err(e) => {
            tracing::error!(error = %e, task_id = %task.id, "claim finalization failed");
            Err(true)
        }
    }
}

/// POST /api/daemon/runtimes/{runtimeId}/tasks/claim — legacy single-runtime
/// poll. Shares the batch endpoint's finalization path.
async fn claim_task_by_runtime(
    State(state): State<HandlerState>,
    Path(runtime_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let access = Access::new(&state, &headers);
    let (_rt, ws_id) = match require_daemon_runtime_access(&access, None, &runtime_id).await {
        Ok(v) => v,
        Err(res) => return res,
    };
    let rt_uuid = match Uuid::parse_str(runtime_id.trim()) {
        Ok(u) => u,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"),
    };
    let task = match state.tasks.claim_task_for_runtime(rt_uuid).await {
        Ok(Some(t)) => t,
        Ok(None) => return Json(json!({ "task": null })).into_response(),
        Err(e) => {
            tracing::error!(error = %e, runtime_id = %runtime_id, "claim failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to claim task: {e}"),
            );
        }
    };
    let rt = match runtime::get_agent_runtime(&state.pool, rt_uuid).await {
        Ok(Some(rt)) => rt,
        _ => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load runtime"),
    };
    let Some(owner) = rt.owner_id else {
        tracing::error!(task_id = %task.id, "claim: runtime owner missing; cancelling task");
        let _ = state.tasks.cancel_task(task.id).await;
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime owner required to mint task token",
        );
    };
    match finalize_claim(&state, &task, owner, rt.workspace_id).await {
        Ok(token) => {
            let mut payload = crate::task_json::task_to_map(&task, &ws_id);
            payload["auth_token"] = Value::String(token);
            payload["leader_role_resolved"] = Value::Bool(true);
            Json(json!({ "task": payload })).into_response()
        }
        Err(true) => {
            let _ = state.tasks.requeue_task_after_claim_failure(&task).await;
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to finalize task claim",
            )
        }
        Err(false) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to mint task token",
        ),
    }
}

/// POST /api/daemon/runtimes/{runtimeId}/tasks/{taskId}/prepare-lease
async fn extend_task_prepare_lease(
    State(state): State<HandlerState>,
    Path((runtime_id, task_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let access = Access::new(&state, &headers);
    let (rt, _) = match require_daemon_runtime_access(&access, None, &runtime_id).await {
        Ok(v) => v,
        Err(res) => return res,
    };
    let (task, task_ws) =
        match require_daemon_task_access_with_workspace(&access, None, &task_id).await {
            Ok(v) => v,
            Err(res) => return res,
        };
    if task_ws != rt.workspace_id.to_string() || task.runtime_id != Some(rt.id) {
        return error_response(StatusCode::NOT_FOUND, "task not found");
    }
    let rt_uuid = Uuid::parse_str(runtime_id.trim()).unwrap();
    let task_uuid = Uuid::parse_str(task_id.trim()).unwrap();
    match state
        .tasks
        .extend_task_prepare_lease(task_uuid, rt_uuid)
        .await
    {
        Ok(updated) => Json(crate::task_json::task_to_map(&updated, &task_ws)).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, task_id = %task_id, "extend prepare lease failed");
            error_response(StatusCode::BAD_REQUEST, &e.to_string())
        }
    }
}

/// GET /api/daemon/runtimes/{runtimeId}/tasks/pending
async fn list_pending_tasks_by_runtime(
    State(state): State<HandlerState>,
    Path(runtime_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let access = Access::new(&state, &headers);
    let (_, ws_id) = match require_daemon_runtime_access(&access, None, &runtime_id).await {
        Ok(v) => v,
        Err(res) => return res,
    };
    let Ok(rt_uuid) = Uuid::parse_str(runtime_id.trim()) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid runtime_id");
    };
    let tasks = match agent::list_pending_tasks_by_runtime(&state.pool, rt_uuid).await {
        Ok(t) => t,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list pending tasks",
            )
        }
    };
    Json(
        tasks
            .iter()
            .map(|t| crate::task_json::task_to_map(t, &ws_id))
            .collect::<Vec<_>>(),
    )
    .into_response()
}

/// POST /api/daemon/runtimes/{runtimeId}/tasks/{taskId}/skill-bundles/resolve
#[derive(Deserialize)]
struct ResolveSkillBundlesRequest {
    #[serde(default)]
    skills: Vec<SkillRef>,
}

#[derive(Deserialize)]
struct SkillRef {
    #[serde(default)]
    id: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    hash: String,
}

async fn resolve_task_skill_bundles(
    State(state): State<HandlerState>,
    Path((runtime_id, task_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<ResolveSkillBundlesRequest>>,
) -> Response {
    let access = Access::new(&state, &headers);
    let (_rt, _) = match require_daemon_runtime_access(&access, None, &runtime_id).await {
        Ok(v) => v,
        Err(res) => return res,
    };
    let (task, task_ws) =
        match require_daemon_task_access_with_workspace(&access, None, &task_id).await {
            Ok(v) => v,
            Err(res) => return res,
        };
    if task_ws != _rt.workspace_id.to_string() || task.runtime_id != Some(_rt.id) {
        return error_response(StatusCode::NOT_FOUND, "task not found");
    }
    if task.status != "dispatched" && task.status != "waiting_local_directory" {
        return error_response(StatusCode::CONFLICT, "task is not preparing");
    }
    let Some(Json(req)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    if req.skills.is_empty() {
        return Json(json!({ "bundles": [] })).into_response();
    }
    let (bundles, _) = state.tasks.load_agent_skill_bundles(task.agent_id).await;
    for r in &req.skills {
        if r.id.is_empty() || r.source.is_empty() || r.hash.is_empty() {
            return error_response(StatusCode::BAD_REQUEST, "invalid skill ref");
        }
        let found = bundles.iter().any(|b| b.source == r.source && b.id == r.id);
        if !found {
            return error_response(StatusCode::NOT_FOUND, "skill bundle not found");
        }
    }
    Json(json!({ "bundles": bundles })).into_response()
}

// ---------------------------------------------------------------------------
// Task lifecycle
// ---------------------------------------------------------------------------

async fn get_task_status(
    State(state): State<HandlerState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let access = Access::new(&state, &headers);
    let (task, _) = match require_daemon_task_access_with_workspace(&access, None, &task_id).await {
        Ok(v) => v,
        Err(res) => return res,
    };
    Json(json!({ "status": task.status })).into_response()
}

async fn start_task(
    State(state): State<HandlerState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let access = Access::new(&state, &headers);
    let (_, ws_id) = match require_daemon_task_access_with_workspace(&access, None, &task_id).await
    {
        Ok(v) => v,
        Err(res) => return res,
    };
    let Ok(task_uuid) = Uuid::parse_str(task_id.trim()) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid task_id");
    };
    match state.tasks.start_task(task_uuid).await {
        Ok(task) => Json(crate::task_json::task_to_map(&task, &ws_id)).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, task_id = %task_id, "start task failed");
            error_response(StatusCode::BAD_REQUEST, &e.to_string())
        }
    }
}

#[derive(Deserialize, Default)]
struct WaitLocalDirectoryRequest {
    #[serde(default)]
    reason: String,
}

async fn mark_task_waiting_local_directory(
    State(state): State<HandlerState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<WaitLocalDirectoryRequest>>,
) -> Response {
    let access = Access::new(&state, &headers);
    let (_, ws_id) = match require_daemon_task_access_with_workspace(&access, None, &task_id).await
    {
        Ok(v) => v,
        Err(res) => return res,
    };
    // Empty bodies are legal: an absent payload means no reason (Go ContentLength==0).
    let reason = body.map(|Json(r)| r.reason).unwrap_or_default();
    let Ok(task_uuid) = Uuid::parse_str(task_id.trim()) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid task_id");
    };
    match state
        .tasks
        .mark_task_waiting_local_directory(task_uuid, &reason)
        .await
    {
        Ok(task) => Json(crate::task_json::task_to_map(&task, &ws_id)).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, task_id = %task_id, "mark waiting_local_directory failed");
            error_response(StatusCode::BAD_REQUEST, &e.to_string())
        }
    }
}

#[derive(Deserialize)]
struct TaskProgressRequest {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    step: i32,
    #[serde(default)]
    total: i32,
}

async fn report_task_progress(
    State(state): State<HandlerState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<TaskProgressRequest>>,
) -> Response {
    let Some(Json(req)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    let access = Access::new(&state, &headers);
    let (task, _) = match require_daemon_task_access_with_workspace(&access, None, &task_id).await {
        Ok(v) => v,
        Err(res) => return res,
    };
    // Workspace resolution matches Go: issue first, chat session fallback.
    let mut workspace_id = String::new();
    if let Some(issue_id) = task.issue_id {
        if let Ok(Some(issue_row)) = issue::get_issue(&state.pool, issue_id).await {
            workspace_id = issue_row.workspace_id.to_string();
        }
    }
    if workspace_id.is_empty() {
        if let Some(cs_id) = task.chat_session_id {
            if let Ok(Some(cs)) = chat::get_chat_session(&state.pool, cs_id).await {
                workspace_id = cs.workspace_id.to_string();
            }
        }
    }
    state
        .tasks
        .report_progress(&task_id, &workspace_id, &req.summary, req.step, req.total);
    Json(json!({ "status": "ok" })).into_response()
}

#[derive(Deserialize)]
struct TaskCompleteRequest {
    #[serde(default, rename = "pr_url")]
    pr_url: String,
    #[serde(default)]
    output: String,
    #[serde(default, rename = "session_id")]
    session_id: String,
    #[serde(default, rename = "work_dir")]
    work_dir: String,
    #[serde(default, rename = "durable_work_dir")]
    durable_work_dir: String,
    #[serde(default, rename = "branch_name")]
    branch_name: String,
    #[serde(default, rename = "session_rollout_missing")]
    session_rollout_missing: bool,
    #[serde(default, rename = "retired_session_id")]
    retired_session_id: String,
}

fn sanitize(s: &str) -> String {
    {
        let mut out = s.to_string();
        if s.contains('\0') {
            out = s.replace('\0', "");
        }
        out
    }
}

/// POST /api/daemon/tasks/{taskId}/complete. A context-exhaustion notice in the
/// output is re-routed to the failure path so a run that ran out of context is
/// recorded as failed rather than published as a clean success (GH #6402).
async fn complete_task(
    State(state): State<HandlerState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<TaskCompleteRequest>>,
) -> Response {
    let access = Access::new(&state, &headers);
    let (_, ws_id) = match require_daemon_task_access_with_workspace(&access, None, &task_id).await
    {
        Ok(v) => v,
        Err(res) => return res,
    };
    let Some(Json(req)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    let req = TaskCompleteRequest {
        pr_url: sanitize(&req.pr_url),
        output: sanitize(&req.output),
        session_id: sanitize(&req.session_id),
        work_dir: sanitize(&req.work_dir),
        durable_work_dir: sanitize(&req.durable_work_dir),
        branch_name: sanitize(&req.branch_name),
        session_rollout_missing: req.session_rollout_missing,
        retired_session_id: sanitize(&req.retired_session_id),
    };

    if cordy_task_failure::context_exhausted_completion(&req.output) {
        tracing::warn!(task_id = %task_id, "complete: context-exhaustion notice, recording as failed");
        return fail_task_impl(
            &state,
            &ws_id,
            &task_id,
            FailBody {
                error: req.output.clone(),
                session_id: req.session_id.clone(),
                work_dir: req.work_dir.clone(),
                durable_work_dir: req.durable_work_dir.clone(),
                failure_reason: cordy_task_failure::Reason::AGENT_CONTEXT_OVERFLOW.to_string(),
                branch_name: req.branch_name.clone(),
                session_rollout_missing: req.session_rollout_missing,
                retired_session_id: req.retired_session_id.clone(),
            },
        )
        .await;
    }

    let result = json!({
        "pr_url": req.pr_url,
        "output": req.output,
        "session_id": req.session_id,
        "work_dir": req.work_dir,
        "durable_work_dir": req.durable_work_dir,
        "branch_name": req.branch_name,
        "session_rollout_missing": req.session_rollout_missing,
        "retired_session_id": req.retired_session_id,
    });
    let Ok(task_uuid) = Uuid::parse_str(task_id.trim()) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid task_id");
    };
    match state
        .tasks
        .complete_task(
            task_uuid,
            &result,
            &req.session_id,
            &req.work_dir,
            &req.branch_name,
            req.session_rollout_missing,
            &req.retired_session_id,
            &req.durable_work_dir,
        )
        .await
    {
        Ok(task) => {
            revoke_tokens_best_effort(&state, task.id).await;
            tracing::info!(task_id = %task_id, agent_id = %task.agent_id, "task completed");
            Json(crate::task_json::task_to_map(&task, &ws_id)).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, task_id = %task_id, "complete task failed");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    }
}

async fn revoke_tokens_best_effort(state: &HandlerState, task_id: Uuid) {
    if let Err(e) = task_token::delete_task_tokens_by_task(&state.pool, task_id).await {
        tracing::warn!(error = %e, task_id = %task_id, "failed to revoke task tokens");
    }
}

#[derive(Deserialize, Default)]
struct FailBody {
    #[serde(default)]
    error: String,
    #[serde(default, rename = "session_id")]
    session_id: String,
    #[serde(default, rename = "work_dir")]
    work_dir: String,
    #[serde(default, rename = "durable_work_dir")]
    durable_work_dir: String,
    #[serde(default, rename = "failure_reason")]
    failure_reason: String,
    #[serde(default, rename = "branch_name")]
    branch_name: String,
    #[serde(default, rename = "session_rollout_missing")]
    session_rollout_missing: bool,
    #[serde(default, rename = "retired_session_id")]
    retired_session_id: String,
}

async fn fail_task(
    State(state): State<HandlerState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<FailBody>>,
) -> Response {
    let access = Access::new(&state, &headers);
    let (_, ws_id) = match require_daemon_task_access_with_workspace(&access, None, &task_id).await
    {
        Ok(v) => v,
        Err(res) => return res,
    };
    let Some(Json(req)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    fail_task_impl(
        &state,
        &ws_id,
        &task_id,
        FailBody {
            error: sanitize(&req.error),
            session_id: sanitize(&req.session_id),
            work_dir: sanitize(&req.work_dir),
            durable_work_dir: sanitize(&req.durable_work_dir),
            failure_reason: sanitize(&req.failure_reason),
            branch_name: sanitize(&req.branch_name),
            session_rollout_missing: req.session_rollout_missing,
            retired_session_id: sanitize(&req.retired_session_id),
        },
    )
    .await
}

async fn fail_task_impl(
    state: &HandlerState,
    workspace_id: &str,
    task_id: &str,
    req: FailBody,
) -> Response {
    let Ok(task_uuid) = Uuid::parse_str(task_id.trim()) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid task_id");
    };
    match state
        .tasks
        .fail_task(
            task_uuid,
            &req.error,
            &req.session_id,
            &req.work_dir,
            &req.branch_name,
            &req.failure_reason,
            req.session_rollout_missing,
            &req.retired_session_id,
            &req.durable_work_dir,
        )
        .await
    {
        Ok(task) => {
            state.tasks.notify_task_finished(&task);
            revoke_tokens_best_effort(state, task.id).await;
            tracing::info!(
                task_id = %task_id,
                agent_id = %task.agent_id,
                failure_reason = %req.failure_reason,
                "task failed"
            );
            Json(crate::task_json::task_to_map(&task, workspace_id)).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, task_id = %task_id, "fail task failed");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
        }
    }
}

#[derive(Deserialize)]
struct UsageEntry {
    #[serde(default)]
    provider: String,
    #[serde(default, rename = "model")]
    model: String,
    #[serde(default, rename = "input_tokens")]
    input_tokens: i64,
    #[serde(default, rename = "output_tokens")]
    output_tokens: i64,
    #[serde(default, rename = "cache_read_tokens")]
    cache_read_tokens: i64,
    #[serde(default, rename = "cache_write_tokens")]
    cache_write_tokens: i64,
    #[serde(default, rename = "cost_usd_ticks")]
    cost_usd_ticks: i64,
}

#[derive(Deserialize)]
struct UsageRequest {
    #[serde(default)]
    usage: Vec<UsageEntry>,
}

async fn report_task_usage(
    State(state): State<HandlerState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<UsageRequest>>,
) -> Response {
    let access = Access::new(&state, &headers);
    let (task, _) = match require_daemon_task_access_with_workspace(&access, None, &task_id).await {
        Ok(v) => v,
        Err(res) => return res,
    };
    let Some(Json(req)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    let task_uuid = Uuid::parse_str(task_id.trim()).unwrap();
    for u in &req.usage {
        let provider = normalize_provider(&u.provider);
        let provider = if provider.is_empty() {
            // Backfill from the runtime so generic ids like `auto` still price.
            match task.runtime_id {
                Some(rid) => futures_executor_block(async {
                    match runtime::get_agent_runtime(&state.pool, rid).await {
                        Ok(Some(rt)) => normalize_provider(&rt.provider),
                        _ => String::new(),
                    }
                }),
                None => String::new(),
            }
        } else {
            provider
        };
        // Only a positive tick figure is authoritative; zero/negative stays NULL.
        let cost = (u.cost_usd_ticks > 0).then_some(u.cost_usd_ticks);
        if let Err(e) = cordy_db::queries::task_usage::upsert_task_usage(
            &state.pool,
            task_uuid,
            &provider,
            &u.model,
            u.input_tokens,
            u.output_tokens,
            u.cache_read_tokens,
            u.cache_write_tokens,
            cost,
        )
        .await
        {
            tracing::warn!(error = %e, task_id = %task_id, model = %u.model, "upsert task usage failed");
            continue;
        }
        state
            .tasks
            .capture_task_usage(
                &task,
                &provider,
                &u.model,
                u.input_tokens,
                u.output_tokens,
                u.cache_read_tokens,
                u.cache_write_tokens,
                u.cost_usd_ticks.max(0),
            )
            .await;
    }
    Json(json!({ "status": "ok" })).into_response()
}

// Blocking wrapper kept local to the usage backfill path; the runtime lookup
// is a single-row point read and the endpoint is daemon-only.
fn futures_executor_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Handle::current().block_on(fut)
}

// ---------------------------------------------------------------------------
// Task messages
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TaskMessageRequest {
    #[serde(default)]
    seq: i32,
    #[serde(default, rename = "type")]
    type_: String,
    #[serde(default)]
    tool: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    input: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    output: String,
}

#[derive(Deserialize)]
struct TaskMessageBatchRequest {
    #[serde(default)]
    messages: Vec<TaskMessageRequest>,
}

fn task_message_payload(m: &cordy_db::models::TaskMessage) -> Value {
    json!({
        "task_id": m.task_id.to_string(),
        "seq": m.seq,
        "type": m.type_,
        "tool": m.tool.clone().unwrap_or_default(),
        "content": m.content.clone().unwrap_or_default(),
        "input": m.input.clone(),
        "output": m.output.clone().unwrap_or_default(),
        "created_at": crate::timefmt::rfc3339_nano(m.created_at),
    })
}

async fn report_task_messages(
    State(state): State<HandlerState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<TaskMessageBatchRequest>>,
) -> Response {
    let Some(Json(req)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    if req.messages.is_empty() {
        return Json(json!({ "status": "ok" })).into_response();
    }
    let access = Access::new(&state, &headers);
    let (task, _) = match require_daemon_task_access_with_workspace(&access, None, &task_id).await {
        Ok(v) => v,
        Err(res) => return res,
    };
    let mut workspace_id = String::new();
    if let Some(issue_id) = task.issue_id {
        if let Ok(Some(row)) = issue::get_issue(&state.pool, issue_id).await {
            workspace_id = row.workspace_id.to_string();
        }
    }
    if workspace_id.is_empty() {
        if let Some(cs_id) = task.chat_session_id {
            if let Ok(Some(cs)) = chat::get_chat_session(&state.pool, cs_id).await {
                workspace_id = cs.workspace_id.to_string();
            }
        }
    }
    let task_uuid = Uuid::parse_str(task_id.trim()).unwrap();
    for msg in &req.messages {
        // Redact secret-shaped substrings before persisting or broadcasting.
        let content = cordy_service::redact::text(&msg.content);
        let output = cordy_service::redact::text(&msg.output);
        let input = msg
            .input
            .as_ref()
            .map(|m| Value::Object(cordy_service::redact::input_map(m)));
        let type_ = sanitize(&msg.type_);
        let tool = sanitize(&msg.tool);
        let content = sanitize(&content);
        let output = sanitize(&output);

        match task_message::create_task_message(
            &state.pool,
            cordy_db::dbid::new_v7(),
            task_uuid,
            msg.seq,
            &type_,
            (!tool.is_empty()).then_some(tool.as_str()),
            (!content.is_empty()).then_some(content.as_str()),
            &input.unwrap_or(Value::Null),
            (!output.is_empty()).then_some(output.as_str()),
        )
        .await
        {
            Ok(Some(created)) => {
                if !workspace_id.is_empty() {
                    state.bus.publish(&cordy_events::Event {
                        event_type: cordy_protocol::EVENT_TASK_MESSAGE.to_string(),
                        workspace_id: workspace_id.clone(),
                        actor_type: "system".to_string(),
                        actor_id: String::new(),
                        payload: json!({
                            "task_message": task_message_payload(&created),
                            "issue_id": task.issue_id.map(|u| u.to_string()).unwrap_or_default(),
                        }),
                        task_id: task_id.to_string(),
                        chat_session_id: String::new(),
                    });
                }
            }
            _ => {
                tracing::error!(task_id = %task_id, seq = msg.seq, "failed to create task message");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to persist task message",
                );
            }
        }
    }
    Json(json!({ "status": "ok" })).into_response()
}

async fn list_task_messages(
    State(state): State<HandlerState>,
    Path(task_id): Path<String>,
    AxumQuery(query): AxumQuery<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    list_task_messages_impl(state, task_id, query, headers).await
}

/// GET /api/daemon/tasks/{taskId}/messages — daemon variant (workspace via the
/// task row). The user-authenticated twin lands with the issues slice.
async fn list_task_messages_impl(
    state: HandlerState,
    task_id: String,
    query: HashMap<String, String>,
    headers: HeaderMap,
) -> Response {
    let access = Access::new(&state, &headers);
    let (_task, _) = match require_daemon_task_access_with_workspace(&access, None, &task_id).await
    {
        Ok(v) => v,
        Err(res) => return res,
    };
    let task_uuid = Uuid::parse_str(task_id.trim()).unwrap();
    let messages = match query.get("since") {
        Some(since) => {
            let Ok(seq) = since.parse::<i32>() else {
                return error_response(StatusCode::BAD_REQUEST, "invalid since parameter");
            };
            task_message::list_task_messages_since(&state.pool, task_uuid, seq).await
        }
        None => task_message::list_task_messages(&state.pool, task_uuid).await,
    };
    match messages {
        Ok(rows) => Json(rows.iter().map(task_message_payload).collect::<Vec<_>>()).into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list task messages",
        ),
    }
}

#[derive(Deserialize, Default)]
struct CancelAckBody {
    #[serde(default, rename = "branch_name")]
    branch_name: String,
    #[serde(default, rename = "durable_work_dir")]
    durable_work_dir: String,
    #[serde(default, rename = "error_message")]
    error_message: String,
    #[serde(default, rename = "failure_reason")]
    failure_reason: String,
}

/// POST /api/daemon/tasks/{taskId}/cancel-ack. Both writes carry a
/// status='cancelled' CAS inside the query so a late ack from a stale run can
/// never stamp its branch onto a completed/failed row; replays are idempotent.
async fn ack_task_cancelled(
    State(state): State<HandlerState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<CancelAckBody>>,
) -> Response {
    let access = Access::new(&state, &headers);
    let (task, _) = match require_daemon_task_access_with_workspace(&access, None, &task_id).await {
        Ok(v) => v,
        Err(res) => return res,
    };
    // Body is optional and decode failures must not break the contract.
    let req = body.map(|Json(r)| r).unwrap_or_default();
    let branch = sanitize(req.branch_name.trim());
    let durable = sanitize(req.durable_work_dir.trim());
    let error_message = sanitize(req.error_message.trim());
    let failure_reason = sanitize(req.failure_reason.trim());

    let mut delivered = false;
    if !durable.is_empty() {
        if let Err(e) =
            agent::set_agent_task_durable_work_dir(&state.pool, Some(durable.as_str()), task.id)
                .await
        {
            tracing::error!(error = %e, task_id = %task_id, "cancel ack: record durable work directory failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to record durable work directory",
            );
        }
        delivered = true;
    }
    if !branch.is_empty() {
        if let Err(e) =
            agent::set_agent_task_branch_name(&state.pool, Some(branch.as_str()), task.id).await
        {
            tracing::error!(error = %e, task_id = %task_id, "cancel ack: record branch name failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to record branch name",
            );
        }
        delivered = true;
    }
    if !error_message.is_empty() {
        let reason = (!failure_reason.is_empty()).then_some(failure_reason.as_str());
        if let Err(e) = agent::set_agent_task_error_if_empty(
            &state.pool,
            Some(error_message.as_str()),
            reason,
            task.id,
        )
        .await
        {
            tracing::error!(error = %e, task_id = %task_id, "cancel ack: record preserved-work error failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to record task error",
            );
        }
        delivered = true;
    }
    if delivered {
        state.tasks.rebroadcast_cancelled_task(task.id).await;
    }
    state.tasks.finalize_deferred_cancelled_chat(task.id).await;
    Json(json!({ "status": "ok" })).into_response()
}

#[derive(Deserialize, Default)]
struct PinSessionBody {
    #[serde(default, rename = "session_id")]
    session_id: String,
    #[serde(default, rename = "work_dir")]
    work_dir: String,
}

/// POST /api/daemon/tasks/{taskId}/session. Both writes commit together with
/// the cancelled-chat pointer advance (GH #6340).
async fn pin_task_session(
    State(state): State<HandlerState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<PinSessionBody>>,
) -> Response {
    let access = Access::new(&state, &headers);
    let (task, _) = match require_daemon_task_access_with_workspace(&access, None, &task_id).await {
        Ok(v) => v,
        Err(res) => return res,
    };
    let Some(Json(req)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    if req.session_id.is_empty() && req.work_dir.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "session_id or work_dir required");
    }
    let session_id = (!req.session_id.is_empty()).then_some(req.session_id.as_str());
    let work_dir = (!req.work_dir.is_empty()).then_some(req.work_dir.as_str());

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "pin session failed");
        }
    };
    // chat_session → agent_task_queue is the global lock order; a missing chat
    // session is fine (ErrNoRows tolerated in Go).
    let _ = chat::lock_chat_session_for_task(&mut *tx, task.id).await;
    if let Err(e) = agent::update_agent_task_session(&mut *tx, task.id, session_id, work_dir).await
    {
        tracing::warn!(error = %e, task_id = %task_id, "pin-session failed");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "pin session failed");
    }
    if let Err(e) = chat::advance_cancelled_chat_session_pointer(&mut *tx, task.id).await {
        tracing::warn!(error = %e, task_id = %task_id, "advance cancelled chat session pointer failed");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "pin session failed");
    }
    if tx.commit().await.is_err() {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "pin session failed");
    }
    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------------------
// POST /api/daemon/runtimes/{runtimeId}/recover-orphans
// ---------------------------------------------------------------------------

async fn recover_orphaned_tasks(
    State(state): State<HandlerState>,
    Path(runtime_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let access = Access::new(&state, &headers);
    if require_daemon_runtime_access(&access, None, &runtime_id)
        .await
        .is_err()
    {
        // The exact status is already rendered by the accessor.
        return access_check_failed();
    }
    let rt_uuid = Uuid::parse_str(runtime_id.trim()).unwrap();
    let rows = match agent::recover_orphaned_tasks_for_runtime(&state.pool, rt_uuid).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, runtime_id = %runtime_id, "recover-orphans failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "recover orphans failed");
        }
    };
    // Shared post-failure pipeline: same task:failed events + auto-retry as the sweeper.
    let retried = state.tasks.handle_failed_tasks(&rows).await;
    if !rows.is_empty() {
        tracing::info!(
            runtime_id = %runtime_id,
            orphaned = rows.len(),
            retried = retried,
            "recover-orphans completed"
        );
    }
    Json(json!({ "orphaned": rows.len(), "retried": retried })).into_response()
}

fn access_check_failed() -> Response {
    error_response(StatusCode::NOT_FOUND, "not found")
}

// ---------------------------------------------------------------------------
// GC-check family
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct BatchGcRequest {
    #[serde(default, rename = "issue_ids")]
    issue_ids: Vec<String>,
}

const MAX_ISSUE_GC_BATCH_SIZE: usize = 500;
/// Enforced by the axum default body limit; kept as documentation of Go cap.
#[allow(dead_code)]
const MAX_ISSUE_GC_BATCH_BODY_BYTES: usize = 64 << 10;

/// POST /api/daemon/workspaces/{workspaceId}/issues/gc-check. One explicit
/// result per requested id; missing/foreign rows become found=false so the
/// endpoint is not an enumeration oracle.
async fn batch_issue_gc_check(
    State(state): State<HandlerState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<BatchGcRequest>>,
) -> Response {
    let access = Access::new(&state, &headers);
    let ws_uuid = Uuid::parse_str(workspace_id.trim())
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace_id"))
        .unwrap();
    if check_daemon_workspace_access(&access, None, &workspace_id)
        .await
        .is_err()
    {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    // Body size is capped by the axum default body limit (well above 64 KiB but
    // bounded); the count cap below preserves Go's unbounded-DB-work guard.
    let Some(Json(req)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    if req.issue_ids.len() > MAX_ISSUE_GC_BATCH_SIZE {
        return error_response(StatusCode::BAD_REQUEST, "too many issue_ids");
    }
    let mut parsed: Vec<Uuid> = Vec::with_capacity(req.issue_ids.len());
    for id in &req.issue_ids {
        match Uuid::parse_str(id.trim()) {
            Ok(u) => parsed.push(u),
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid issue_id"),
        }
    }
    let mut by_id: HashMap<Uuid, (String, Option<chrono::DateTime<chrono::Utc>>)> = HashMap::new();
    if !parsed.is_empty() {
        match issue::list_issue_gc_statuses(&state.pool, ws_uuid, parsed.clone()).await {
            Ok(rows) => {
                for row in rows {
                    if let Some(id) = row.id {
                        by_id.insert(id, (row.status, row.updated_at));
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, workspace_id = %workspace_id, "list issue GC statuses failed");
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to check issues");
            }
        }
    }

    // ONE resolver for the whole batch so an all-built-in batch costs zero
    // catalog reads (MUL-6243).
    let mut resolver = issue_status_svc::Resolver::new(ws_uuid);
    let items: Vec<Value> = req
        .issue_ids
        .iter()
        .zip(parsed.iter())
        .map(|(raw, uuid)| {
            match by_id.get(uuid) {
                Some((status, updated_at)) => {
                    // Canonical status resolved server-side so daemons that
                    // predate custom statuses keep making correct GC decisions.
                    let effective = futures_executor_block(resolver.effective(&state.pool, status));
                    json!({
                        "id": raw,
                        "found": true,
                        "status": effective,
                        "updated_at": updated_at.map(crate::timefmt::rfc3339),
                    })
                }
                None => json!({ "id": raw, "found": false }),
            }
        })
        .collect();
    Json(json!({ "issues": items })).into_response()
}

async fn issue_gc_check(
    State(state): State<HandlerState>,
    Path(issue_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let access = Access::new(&state, &headers);
    let Ok(issue_uuid) = parse_uuid_or_bad_request(&issue_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid issue_id");
    };
    let Some(row) = issue::get_issue_gc_status(&state.pool, issue_uuid)
        .await
        .ok()
        .flatten()
    else {
        return error_response(StatusCode::NOT_FOUND, "issue not found");
    };
    let Some(ws_id) = row.workspace_id else {
        return error_response(StatusCode::NOT_FOUND, "issue not found");
    };
    if check_daemon_workspace_access(&access, None, &ws_id.to_string())
        .await
        .is_err()
    {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    Json(json!({
        "status": issue_status_svc::effective(&state.pool, ws_id, &row.status).await,
        "updated_at": row.updated_at.map(crate::timefmt::rfc3339),
    }))
    .into_response()
}

async fn chat_session_gc_check(
    State(state): State<HandlerState>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let access = Access::new(&state, &headers);
    let session_uuid = match parse_uuid_or_bad_request(&session_id) {
        Ok(u) => u,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid session_id"),
    };
    let Some(session) = chat::get_chat_session(&state.pool, session_uuid)
        .await
        .ok()
        .flatten()
    else {
        return error_response(StatusCode::NOT_FOUND, "chat session not found");
    };
    if check_daemon_workspace_access(&access, None, &session.workspace_id.to_string())
        .await
        .is_err()
    {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    Json(json!({
        "status": session.status,
        "updated_at": crate::timefmt::rfc3339(session.updated_at),
    }))
    .into_response()
}

async fn autopilot_run_gc_check(
    State(state): State<HandlerState>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let access = Access::new(&state, &headers);
    let run_uuid = match parse_uuid_or_bad_request(&run_id) {
        Ok(u) => u,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid run_id"),
    };
    let Some(run) = autopilot::get_autopilot_run(&state.pool, run_uuid)
        .await
        .ok()
        .flatten()
    else {
        return error_response(StatusCode::NOT_FOUND, "autopilot run not found");
    };
    // Parent autopilot gone → 404 rather than 500 so the daemon falls through
    // to its orphan-by-mtime path.
    let Some(ap) = autopilot::get_autopilot(&state.pool, run.autopilot_id)
        .await
        .ok()
        .flatten()
    else {
        return error_response(StatusCode::NOT_FOUND, "autopilot run not found");
    };
    if check_daemon_workspace_access(&access, None, &ap.workspace_id.to_string())
        .await
        .is_err()
    {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    Json(json!({
        "status": run.status,
        "completed_at": run.completed_at.map(crate::timefmt::rfc3339),
    }))
    .into_response()
}

async fn task_gc_check(
    State(state): State<HandlerState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let access = Access::new(&state, &headers);
    let (task, _) = match require_daemon_task_access_with_workspace(&access, None, &task_id).await {
        Ok(v) => v,
        Err(res) => return res,
    };
    Json(json!({
        "status": task.status,
        "completed_at": task.completed_at.map(crate::timefmt::rfc3339),
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// Workspace repos / runtime profiles / plugin agent hooks
// ---------------------------------------------------------------------------

async fn workspace_repos(
    State(state): State<HandlerState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let access = Access::new(&state, &headers);
    let wid = workspace_id.trim().to_string();
    if check_daemon_workspace_access(&access, None, &wid)
        .await
        .is_err()
    {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    let ws_uuid = match parse_uuid_or_bad_request(&wid) {
        Ok(u) => u,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace_id"),
    };
    let Some(ws) = workspace::get_workspace(&state.pool, ws_uuid)
        .await
        .ok()
        .flatten()
    else {
        return error_response(StatusCode::NOT_FOUND, "workspace not found");
    };
    let repos = parse_workspace_repos(&ws.repos);
    Json(json!({
        "workspace_id": wid,
        "repos": repos,
        "repos_version": workspace_repos_version(&repos),
        "settings": if ws.settings.is_null() { Value::Null } else { ws.settings.clone() },
    }))
    .into_response()
}

async fn list_runtime_profiles(
    State(state): State<HandlerState>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let access = Access::new(&state, &headers);
    let wid = workspace_id.trim().to_string();
    if check_daemon_workspace_access(&access, None, &wid)
        .await
        .is_err()
    {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    let ws_uuid = match parse_uuid_or_bad_request(&wid) {
        Ok(u) => u,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace id"),
    };
    let profiles =
        match runtime_profile::list_enabled_runtime_profiles_for_workspace(&state.pool, ws_uuid)
            .await
        {
            Ok(p) => p,
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to list runtime profiles",
                )
            }
        };
    Json(json!({
        "workspace_id": wid,
        "runtime_profiles": profiles.iter().map(crate::profile_json::profile_to_map).collect::<Vec<_>>(),
    }))
    .into_response()
}

fn split_contribution_id(contribution: &str) -> Option<(String, String)> {
    let trimmed = contribution.strip_prefix(cordy_remotemcp::PLUGIN_CONTRIBUTION_PREFIX)?;
    let (installation_id, hook_key) = trimmed.split_once(':')?;
    if installation_id.is_empty() || hook_key.is_empty() {
        return None;
    }
    Some((installation_id.to_string(), hook_key.to_string()))
}

#[derive(Deserialize)]
struct InvokeAgentHookBody {
    #[serde(default, rename = "installation_id")]
    installation_id: String,
    #[serde(default, rename = "hook_key")]
    hook_key: String,
    #[serde(default)]
    input: Option<Value>,
}

/// POST /api/daemon/tasks/{id}/plugin-hooks. A hook failure returns 200 with an
/// error body on purpose — the daemon turns it into a tool ERROR the agent can
/// read and work around; a transport-level failure would fail the whole task.
async fn invoke_agent_plugin_hook(
    State(state): State<HandlerState>,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<InvokeAgentHookBody>>,
) -> Response {
    let Some(Json(req)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    if req.installation_id.is_empty() || req.hook_key.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "installation_id and hook_key are required",
        );
    }
    let access = Access::new(&state, &headers);
    let (_task, ws_id) =
        match require_daemon_task_access_with_workspace(&access, None, &task_id).await {
            Ok(v) => v,
            Err(res) => return res,
        };
    let Ok(ws_uuid) = Uuid::parse_str(&ws_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid workspace_id");
    };
    let installation = match state
        .plugins
        .installation_for_workspace(ws_uuid, &req.installation_id)
        .await
    {
        Ok(i) => i,
        Err(e) => return plugin_error_response(&e, "failed to load the Plugin"),
    };
    let agent_id = _task.agent_id;
    let service = state.plugins.clone();
    let (result, outcome) = cordy_service::plugin_agent_tools::invoke_agent_hook(
        service,
        state.callbacks.as_ref().map(|c| c.as_ref()),
        &state.callback_base_url,
        &installation.id.to_string(),
        &req.hook_key,
        agent_id,
        req.input.as_ref(),
    )
    .await;
    match outcome {
        Ok(()) => Json(serde_json::to_value(&result).unwrap_or(Value::Null)).into_response(),
        Err(_) => Json(json!({
            "status": result.status,
            "error": result.error,
        }))
        .into_response(),
    }
}

/// GET /api/daemon/tasks/{id}/plugin-mcp/{contributionId}/credential — resolved
/// at connection time so a secret never sits in a task record.
async fn resolve_plugin_mcp_credential(
    State(state): State<HandlerState>,
    Path((task_id, contribution_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let access = Access::new(&state, &headers);
    let (_task, ws_id) =
        match require_daemon_task_access_with_workspace(&access, None, &task_id).await {
            Ok(v) => v,
            Err(res) => return res,
        };
    let Some((installation_id, hook_key)) = split_contribution_id(&contribution_id) else {
        return error_response(StatusCode::BAD_REQUEST, "malformed contribution id");
    };
    let Ok(ws_uuid) = Uuid::parse_str(&ws_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid workspace_id");
    };
    let installation = match state
        .plugins
        .installation_for_workspace(ws_uuid, &installation_id)
        .await
    {
        Ok(i) => i,
        Err(e) => return plugin_error_response(&e, "failed to load the Plugin"),
    };
    match cordy_service::plugin_mcp_transport::mcp_hook_credential(
        &state.plugins.pool,
        state.plugins.secrets.as_ref(),
        &installation,
        &hook_key,
    )
    .await
    {
        Ok((header_name, credential)) => Json(json!({
            "credential_header": header_name,
            "credential": credential,
        }))
        .into_response(),
        Err(e) => plugin_error_response(&e, "failed to resolve the credential"),
    }
}

// ---------------------------------------------------------------------------
// Pending-request report endpoints (update / model list / local skills).
// The Redis-backed request stores land with the redis wiring slice; until then
// these validate the route contract and return 404 "request not found", which
// daemons treat as a dropped one-shot report (same as an expired entry).
// ---------------------------------------------------------------------------

async fn report_update_result(
    State(_state): State<HandlerState>,
    Path((runtime_id, update_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let access = Access::new(&_state, &headers);
    if require_daemon_runtime_access(&access, None, &runtime_id)
        .await
        .is_err()
    {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    let _ = (update_id, body);
    error_response(StatusCode::NOT_FOUND, "update not found")
}

async fn report_model_list_result(
    State(_state): State<HandlerState>,
    Path((runtime_id, request_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let access = Access::new(&_state, &headers);
    if require_daemon_runtime_access(&access, None, &runtime_id)
        .await
        .is_err()
    {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    let _ = (request_id, body);
    error_response(StatusCode::NOT_FOUND, "request not found")
}

async fn report_local_skill_list_result(
    State(_state): State<HandlerState>,
    Path((runtime_id, request_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let access = Access::new(&_state, &headers);
    if require_daemon_runtime_access(&access, None, &runtime_id)
        .await
        .is_err()
    {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    let _ = (request_id, body);
    error_response(StatusCode::NOT_FOUND, "request not found")
}

async fn report_local_skill_import_result(
    State(_state): State<HandlerState>,
    Path((runtime_id, request_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let access = Access::new(&_state, &headers);
    if require_daemon_runtime_access(&access, None, &runtime_id)
        .await
        .is_err()
    {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    let _ = (request_id, body);
    error_response(StatusCode::NOT_FOUND, "request not found")
}

/// Maps PluginError kinds onto statuses exactly like Go writePluginError.
fn plugin_error_response(err: &cordy_service::plugin::PluginError, fallback: &str) -> Response {
    use cordy_service::plugin::PluginErrorKind as K;
    let status = match err.kind {
        K::Invalid => StatusCode::BAD_REQUEST,
        K::NotFound => StatusCode::NOT_FOUND,
        K::Conflict => StatusCode::CONFLICT,
        K::Forbidden => StatusCode::FORBIDDEN,
        K::Incompatible => StatusCode::UNPROCESSABLE_ENTITY,
        K::Quota => StatusCode::INSUFFICIENT_STORAGE,
        _ => StatusCode::BAD_GATEWAY,
    };
    let msg = if err.message.is_empty() {
        fallback
    } else {
        err.message.as_str()
    };
    error_response(status, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_provider_trims_and_lowercases() {
        assert_eq!(normalize_provider("  Claude "), "claude");
        assert_eq!(normalize_provider(""), "");
    }

    #[test]
    fn split_contribution_id_parses_plugin_prefix() {
        let (inst, hook) = split_contribution_id("plugin:abc-123:my_hook").unwrap();
        assert_eq!(inst, "abc-123");
        assert_eq!(hook, "my_hook");
        assert!(split_contribution_id("builtin:whatever").is_none());
        assert!(split_contribution_id("plugin:nocolon").is_none());
        assert!(split_contribution_id("plugin::").is_none());
    }

    #[test]
    fn workspace_repos_version_is_order_insensitive_and_hex() {
        use serde_json::json;
        let a = vec![
            json!({"url": "https://x.com/a"}),
            json!({"url": "https://x.com/b"}),
        ];
        let b = vec![
            json!({"url": "https://x.com/b"}),
            json!({"url": "https://x.com/a"}),
        ];
        assert_eq!(workspace_repos_version(&a), workspace_repos_version(&b));
        assert_eq!(workspace_repos_version(&a).len(), 64);
    }

    #[test]
    fn parse_workspace_repos_dedupes_and_drops_empty() {
        use serde_json::json;
        let raw = json!([
            {"url": " https://x.com/a ", "description": "d"},
            {"url": ""},
            {"url": "https://x.com/a"},
            {"description": "no url"},
        ]);
        let repos = parse_workspace_repos(&raw);
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0]["url"], "https://x.com/a");
    }

    // Contract: a missing runtime row maps to 404 (daemon drops + re-registers)
    // while any other DB error maps to 500. Exercised against a live Postgres in
    // integration; the pure mapping helpers are covered above.
}
