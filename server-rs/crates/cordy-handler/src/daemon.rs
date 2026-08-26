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
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use axum::extract::{Path, Query as AxumQuery, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_daemon::hub::{
    ClientIdentity, HeartbeatHandler, RpcHandler, RpcHandlerError, RpcOutcome,
};
use cordy_db::models::AgentRuntime;
use cordy_db::queries::{
    agent, autopilot, chat, comment as comment_q, issue, member, runtime, runtime_profile,
    task_message, task_token, workspace,
};
use cordy_middleware::daemon_auth::DaemonContext;
use cordy_protocol::{
    DaemonHeartbeatAckPayload, DaemonHeartbeatPendingLocalSkillImport,
    DaemonHeartbeatPendingLocalSkills, DaemonHeartbeatPendingModelList,
    DaemonHeartbeatPendingUpdate, DAEMON_CAPABILITY_RPC_V1, EVENT_DAEMON_REGISTER,
    HEARTBEAT_STATUS_RUNTIME_GONE,
};
use cordy_service::issue_status as issue_status_svc;
use cordy_service::plugin::PluginService;
use cordy_service::task_service::TaskService;
use cordy_util::text::{sanitize_json_for_postgres, sanitize_text_for_postgres as sanitize};
use http_body_util::BodyExt as _;
use serde::Deserialize;
use serde_json::{json, Map, Value};
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
        .route("/api/daemon/ws", get(crate::daemon_ws::daemon_ws_handler))
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
pub(crate) fn request_user_id(headers: &HeaderMap) -> String {
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

pub(crate) fn daemon_id_of(ext: Option<DaemonContext>) -> String {
    ext.and_then(|d| d.daemon_id).unwrap_or_default()
}

pub(crate) fn daemon_context_from_headers(headers: &HeaderMap) -> Option<DaemonContext> {
    let workspace_id = headers
        .get(cordy_middleware::daemon_auth::DAEMON_WORKSPACE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let daemon_id = headers
        .get(cordy_middleware::daemon_auth::DAEMON_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    if workspace_id.is_none() && daemon_id.is_none() {
        return None;
    }
    Some(DaemonContext {
        workspace_id,
        daemon_id,
        auth_path: cordy_middleware::daemon_auth::DAEMON_AUTH_PATH_DAEMON_TOKEN,
    })
}

pub(crate) struct Access<'a> {
    state: &'a HandlerState,
    headers: &'a HeaderMap,
}

impl<'a> Access<'a> {
    pub(crate) fn new(state: &'a HandlerState, headers: &'a HeaderMap) -> Self {
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
    let daemon_ctx = daemon_ctx.or_else(|| daemon_context_from_headers(access.headers));
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
    if access
        .state
        .membership_cache
        .get(&user_id, workspace_id)
        .await
    {
        return Ok(workspace_id.to_string());
    }
    match access.get_member(&user_id, workspace_id).await {
        Some(_) => {
            access
                .state
                .membership_cache
                .set(&user_id, workspace_id)
                .await;
            Ok(workspace_id.to_string())
        }
        None => Err(error_response(StatusCode::NOT_FOUND, "not found")),
    }
}

/// Loads a runtime and verifies its workspace belongs to the caller (Go
/// `requireDaemonRuntimeAccess`). Only a missing row is a 404; other DB errors
/// are 500 so the daemon does not self-cleanup on a hiccup.
pub(crate) async fn require_daemon_runtime_access(
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
        let daemon_ws = daemon_workspace_id(daemon_context_from_headers(&headers));
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
        let daemon_ws = daemon_workspace_id(daemon_context_from_headers(&headers));
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
        state.liveness_store.forget(&rt.id.to_string()).await;
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

/// Production implementation of the daemon WebSocket heartbeat seam. It owns
/// only the dependencies used by `processHeartbeat`, deliberately excluding
/// `DaemonHub` so installing it on the hub cannot create a strong-reference
/// cycle through `HandlerState`.
pub(crate) struct DaemonHeartbeatProcessor {
    pool: sqlx::PgPool,
    heartbeat_scheduler: Arc<dyn crate::heartbeat_scheduler::HeartbeatScheduler>,
    liveness_store: Arc<dyn crate::runtime_liveness::LivenessStore>,
    update_store: Option<Arc<crate::pending_store::UpdateStore>>,
    model_list_store: Option<Arc<crate::pending_store::ModelListStore>>,
    local_skill_list_store: Option<Arc<crate::pending_store::LocalSkillListStore>>,
    local_skill_import_store: Option<Arc<crate::pending_store::LocalSkillImportStore>>,
}

impl DaemonHeartbeatProcessor {
    pub(crate) fn from_state(state: &HandlerState) -> Self {
        Self {
            pool: state.pool.clone(),
            heartbeat_scheduler: state.heartbeat_scheduler.clone(),
            liveness_store: state.liveness_store.clone(),
            update_store: state.update_store.clone(),
            model_list_store: state.model_list_store.clone(),
            local_skill_list_store: state.local_skill_list_store.clone(),
            local_skill_import_store: state.local_skill_import_store.clone(),
        }
    }

    async fn process(
        &self,
        rt: &AgentRuntime,
        supports_batch_import: bool,
    ) -> anyhow::Result<DaemonHeartbeatAckPayload> {
        record_heartbeat(
            self.liveness_store.as_ref(),
            self.heartbeat_scheduler.as_ref(),
            rt,
        )
        .await?;

        let runtime_id = rt.id.to_string();
        let mut ack = DaemonHeartbeatAckPayload {
            runtime_id: runtime_id.clone(),
            status: "ok".to_string(),
            server_capabilities: vec![DAEMON_CAPABILITY_RPC_V1.to_string()],
            runtime_gone: false,
            pending_update: None,
            pending_model_list: None,
            pending_local_skills: None,
            pending_local_skill_import: None,
            pending_local_skill_imports: Vec::new(),
        };

        if let Some(store) = &self.update_store {
            match tokio::time::timeout(
                crate::pending_store::HEARTBEAT_HAS_PENDING_TIMEOUT,
                store.has_pending(&runtime_id),
            )
            .await
            {
                Ok(Ok(true)) => match store.pop_pending(&runtime_id).await {
                    Ok(Some(pending)) => {
                        ack.pending_update = Some(DaemonHeartbeatPendingUpdate {
                            id: pending.id,
                            target_version: pending.target_version,
                        });
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(%error, %runtime_id, "update PopPending failed")
                    }
                },
                Ok(Ok(false)) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, %runtime_id, "update HasPending failed")
                }
                Err(_) => tracing::warn!(%runtime_id, "update HasPending timed out"),
            }
        }

        if let Some(store) = &self.model_list_store {
            match tokio::time::timeout(
                crate::pending_store::HEARTBEAT_HAS_PENDING_TIMEOUT,
                store.has_pending(&runtime_id),
            )
            .await
            {
                Ok(Ok(true)) => match store.pop_pending(&runtime_id).await {
                    Ok(Some(pending)) => {
                        ack.pending_model_list =
                            Some(DaemonHeartbeatPendingModelList { id: pending.id });
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(%error, %runtime_id, "model list PopPending failed")
                    }
                },
                Ok(Ok(false)) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, %runtime_id, "model list HasPending failed")
                }
                Err(_) => tracing::warn!(%runtime_id, "model list HasPending timed out"),
            }
        }

        if let Some(store) = &self.local_skill_list_store {
            match tokio::time::timeout(
                crate::pending_store::HEARTBEAT_HAS_PENDING_TIMEOUT,
                store.has_pending(&runtime_id),
            )
            .await
            {
                Ok(Ok(true)) => match store.pop_pending(&runtime_id).await {
                    Ok(Some(pending)) => {
                        ack.pending_local_skills =
                            Some(DaemonHeartbeatPendingLocalSkills { id: pending.id });
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(%error, %runtime_id, "local skill list PopPending failed")
                    }
                },
                Ok(Ok(false)) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, %runtime_id, "local skill list HasPending failed")
                }
                Err(_) => tracing::warn!(%runtime_id, "local skill list HasPending timed out"),
            }
        }

        if let Some(store) = &self.local_skill_import_store {
            match tokio::time::timeout(
                crate::pending_store::HEARTBEAT_HAS_PENDING_TIMEOUT,
                store.has_pending(&runtime_id),
            )
            .await
            {
                Ok(Ok(true)) if supports_batch_import => match store
                    .pop_pending_batch(
                        &runtime_id,
                        crate::claim_response::MAX_LOCAL_SKILL_IMPORT_BATCH,
                    )
                    .await
                {
                    Ok(pending) if !pending.is_empty() => {
                        ack.pending_local_skill_import =
                            Some(DaemonHeartbeatPendingLocalSkillImport {
                                id: pending[0].id.clone(),
                                skill_key: pending[0].skill_key.clone(),
                            });
                        ack.pending_local_skill_imports = pending
                            .into_iter()
                            .map(|pending| DaemonHeartbeatPendingLocalSkillImport {
                                id: pending.id,
                                skill_key: pending.skill_key,
                            })
                            .collect();
                    }
                    Ok(_) => {}
                    Err(error) => tracing::warn!(
                        %error,
                        %runtime_id,
                        "local skill import PopPendingBatch failed"
                    ),
                },
                Ok(Ok(true)) => match store.pop_pending(&runtime_id).await {
                    Ok(Some(pending)) => {
                        ack.pending_local_skill_import =
                            Some(DaemonHeartbeatPendingLocalSkillImport {
                                id: pending.id,
                                skill_key: pending.skill_key,
                            });
                    }
                    Ok(None) => {}
                    Err(error) => tracing::warn!(
                        %error,
                        %runtime_id,
                        "local skill import PopPending failed"
                    ),
                },
                Ok(Ok(false)) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, %runtime_id, "local skill import HasPending failed")
                }
                Err(_) => tracing::warn!(%runtime_id, "local skill import HasPending timed out"),
            }
        }

        Ok(ack)
    }
}

#[async_trait::async_trait]
impl HeartbeatHandler for DaemonHeartbeatProcessor {
    async fn handle_heartbeat(
        &self,
        identity: &ClientIdentity,
        runtime_id: &str,
        supports_batch_import: bool,
    ) -> anyhow::Result<Option<DaemonHeartbeatAckPayload>> {
        let runtime_uuid = Uuid::parse_str(runtime_id).context("invalid runtime_id")?;
        let Some(rt) = runtime::get_agent_runtime(&self.pool, runtime_uuid)
            .await
            .context("get agent runtime")?
        else {
            return Ok(Some(DaemonHeartbeatAckPayload {
                runtime_id: runtime_id.to_string(),
                status: HEARTBEAT_STATUS_RUNTIME_GONE.to_string(),
                server_capabilities: Vec::new(),
                runtime_gone: true,
                pending_update: None,
                pending_model_list: None,
                pending_local_skills: None,
                pending_local_skill_import: None,
                pending_local_skill_imports: Vec::new(),
            }));
        };
        anyhow::ensure!(
            identity.allows_workspace(&rt.workspace_id.to_string()),
            "runtime not in connection workspace"
        );
        Ok(Some(self.process(&rt, supports_batch_import).await?))
    }
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
    let daemon_ws = daemon_workspace_id(daemon_context_from_headers(&headers));
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

    let processor = DaemonHeartbeatProcessor::from_state(&state);
    let ack = match processor.process(&rt, req.supports_batch_import).await {
        Ok(ack) => ack,
        Err(error) => {
            tracing::warn!(%error, runtime_id = %req.runtime_id, "heartbeat failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "heartbeat failed");
        }
    };

    // Preserve the existing HTTP shape while the WebSocket response also
    // advertises its transport capabilities.
    let mut resp = json!({ "status": ack.status, "runtime_id": ack.runtime_id });
    if let Some(pending) = ack.pending_update {
        resp["pending_update"] = json!(pending);
    }
    if let Some(pending) = ack.pending_model_list {
        resp["pending_model_list"] = json!(pending);
    }
    if let Some(pending) = ack.pending_local_skills {
        resp["pending_local_skills"] = json!(pending);
    }
    if let Some(pending) = ack.pending_local_skill_import {
        resp["pending_local_skill_import"] = json!(pending);
    }
    if !ack.pending_local_skill_imports.is_empty() {
        resp["pending_local_skill_imports"] = json!(ack.pending_local_skill_imports);
    }
    Json(resp).into_response()
}

const RUNTIME_LIVENESS_TTL: Duration = Duration::from_secs(90);
const RUNTIME_HEARTBEAT_DB_FLUSH_INTERVAL: Duration = Duration::from_secs(60);

async fn record_heartbeat(
    liveness_store: &dyn crate::runtime_liveness::LivenessStore,
    heartbeat_scheduler: &dyn crate::heartbeat_scheduler::HeartbeatScheduler,
    rt: &AgentRuntime,
) -> anyhow::Result<()> {
    let stale_in_db = rt.last_seen_at.is_none_or(|last_seen| {
        chrono::Utc::now().signed_duration_since(last_seen)
            >= chrono::Duration::from_std(RUNTIME_HEARTBEAT_DB_FLUSH_INTERVAL)
                .unwrap_or_else(|_| chrono::Duration::seconds(60))
    });
    let mut needs_db_write = !liveness_store.available() || rt.status != "online" || stale_in_db;

    if liveness_store.available() {
        if let Err(error) = liveness_store
            .touch(&rt.id.to_string(), RUNTIME_LIVENESS_TTL)
            .await
        {
            tracing::warn!(%error, runtime_id = %rt.id, "liveness touch failed; falling back to DB heartbeat");
            needs_db_write = true;
        }
    }
    if needs_db_write {
        heartbeat_scheduler.schedule(rt).await?;
    }
    Ok(())
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
    let mut owner_id = None;
    let daemon_ws = daemon_workspace_id(daemon_context_from_headers(&headers));
    if !daemon_ws.is_empty() {
        if daemon_ws != req.workspace_id {
            return error_response(StatusCode::NOT_FOUND, "workspace not found");
        }
    } else {
        let user_id = request_user_id(&headers);
        match access_get_member(&state, &headers, &user_id, &req.workspace_id).await {
            Some(m) => {
                state
                    .membership_cache
                    .set(&user_id, &req.workspace_id)
                    .await;
                owner_id = Some(m.user_id);
            }
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
        let capabilities: Vec<&str> = headers
            .get("x-client-capabilities")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect();
        let metadata = json!({
            "version": rt_req.version,
            "cli_version": req.cli_version,
            "launched_by": req.launched_by,
            "capabilities": capabilities,
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
                Ok(Some(mut row)) => {
                    if let Err(e) = tx.commit().await {
                        tracing::error!(error = %e, "register: commit profile runtime failed");
                        return error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            &format!("failed to register runtime: {e}"),
                        );
                    }
                    row.custom_name = inherit_machine_custom_name(
                        &state,
                        row.id,
                        row.workspace_id,
                        row.daemon_id.as_deref(),
                        row.custom_name.as_deref(),
                        row.inserted,
                    )
                    .await;
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
                Ok(Some(mut row)) => {
                    merge_legacy_runtimes(&state, &row, ws_uuid, &provider, &req.legacy_daemon_ids)
                        .await;
                    row.custom_name = inherit_machine_custom_name(
                        &state,
                        row.id,
                        row.workspace_id,
                        row.daemon_id.as_deref(),
                        row.custom_name.as_deref(),
                        row.inserted,
                    )
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
        let capabilities: Vec<&str> = headers
            .get("x-client-capabilities")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect();
        let metadata = json!({
            "version": "",
            "cli_version": req.cli_version,
            "launched_by": req.launched_by,
            "runtime_profile_registration_error": true,
            "runtime_profile_failure_reason": reason,
            "command_name": resolved_command,
            "capabilities": capabilities,
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
            Ok(Some(row)) => {
                if tx.commit().await.is_ok() {
                    let _ = inherit_machine_custom_name(
                        &state,
                        row.id,
                        row.workspace_id,
                        row.daemon_id.as_deref(),
                        row.custom_name.as_deref(),
                        row.inserted,
                    )
                    .await;
                }
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

async fn inherit_machine_custom_name(
    state: &HandlerState,
    runtime_id: Option<Uuid>,
    workspace_id: Option<Uuid>,
    daemon_id: Option<&str>,
    current_name: Option<&str>,
    inserted: bool,
) -> Option<String> {
    if !inserted || current_name.is_some() {
        return current_name.map(str::to_string);
    }
    let (Some(runtime_id), Some(workspace_id), Some(daemon_id)) = (
        runtime_id,
        workspace_id,
        daemon_id.filter(|value| !value.is_empty()),
    ) else {
        return None;
    };
    let Ok(names) =
        runtime::list_daemon_custom_names(&state.pool, workspace_id, Some(daemon_id), runtime_id)
            .await
    else {
        return None;
    };
    let mut shared: Option<&str> = None;
    for name in &names {
        let name = name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())?;
        if shared.is_some_and(|existing| existing != name) {
            return None;
        }
        shared = Some(name);
    }
    let shared = shared?.to_string();
    match runtime::update_agent_runtime_custom_name(&state.pool, Some(&shared), runtime_id).await {
        Ok(Some(_)) => Some(shared),
        _ => None,
    }
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
pub(crate) fn launch_header(agent_type: &str) -> &'static str {
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

/// Exact production dependencies shared by HTTP and WebSocket task claims.
/// Keeping this snapshot independent of `HandlerState` prevents installing an
/// RPC handler from forming `DaemonHub -> handler state -> DaemonHub`.
#[derive(Clone)]
pub(crate) struct DaemonClaimServices {
    pub(crate) pool: sqlx::PgPool,
    pub(crate) tasks: Arc<TaskService>,
    pub(crate) plugins: Arc<PluginService>,
}

impl DaemonClaimServices {
    fn from_state(state: &HandlerState) -> Self {
        Self {
            pool: state.pool.clone(),
            tasks: state.tasks.clone(),
            plugins: state.plugins.clone(),
        }
    }
}

/// Production WS RPC dispatcher. The supported method intentionally stays
/// narrow: every method must reuse the corresponding HTTP domain core rather
/// than grow a second claim/finalization implementation.
pub(crate) struct DaemonRpcProcessor {
    claims: DaemonClaimServices,
}

impl DaemonRpcProcessor {
    pub(crate) fn from_state(state: &HandlerState) -> Self {
        Self {
            claims: DaemonClaimServices::from_state(state),
        }
    }

    fn identity_headers(identity: &ClientIdentity) -> Result<HeaderMap, RpcHandlerError> {
        fn insert(
            headers: &mut HeaderMap,
            name: &'static str,
            value: &str,
        ) -> Result<(), RpcHandlerError> {
            if value.is_empty() {
                return Ok(());
            }
            let value = HeaderValue::from_str(value).map_err(|error| {
                RpcHandlerError::new(
                    StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    anyhow::Error::new(error).context(format!("invalid {name} identity value")),
                )
            })?;
            headers.insert(name, value);
            Ok(())
        }

        let mut headers = HeaderMap::new();
        if identity.daemon_id.is_empty() {
            insert(&mut headers, "x-user-id", &identity.user_id)?;
        } else {
            insert(
                &mut headers,
                cordy_middleware::daemon_auth::DAEMON_WORKSPACE_HEADER,
                &identity.primary_workspace_id(),
            )?;
            insert(
                &mut headers,
                cordy_middleware::daemon_auth::DAEMON_ID_HEADER,
                &identity.daemon_id,
            )?;
        }
        insert(
            &mut headers,
            "x-client-capabilities",
            &identity.capabilities,
        )?;
        insert(&mut headers, "x-client-version", &identity.client_version)?;
        Ok(headers)
    }

    async fn claim_tasks(
        &self,
        ctx: &tokio_util::sync::CancellationToken,
        identity: &ClientIdentity,
        body: Option<&Value>,
    ) -> Result<RpcOutcome, RpcHandlerError> {
        let request =
            serde_json::from_value::<BatchClaimRequest>(body.cloned().unwrap_or_else(|| json!({})))
                .map(Json)
                .map_err(|error| {
                    RpcHandlerError::new(
                        StatusCode::BAD_REQUEST.as_u16(),
                        anyhow::Error::new(error).context("invalid request body"),
                    )
                })?;
        let headers = Self::identity_headers(identity)?;
        let response = tokio::select! {
            biased;
            () = ctx.cancelled() => {
                return Err(RpcHandlerError::new(
                    StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                    anyhow::anyhow!("connection closed"),
                ));
            }
            response = claim_tasks_by_runtime_core(&self.claims, headers, Some(request)) => response,
        };
        let status = response.status().as_u16();
        let bytes = response
            .into_body()
            .collect()
            .await
            .map_err(|error| {
                RpcHandlerError::new(
                    StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    anyhow::Error::new(error).context("collect tasks.claim response"),
                )
            })?
            .to_bytes();
        let body = if bytes.is_empty() {
            None
        } else {
            Some(serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                RpcHandlerError::new(
                    StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                    anyhow::Error::new(error).context("decode tasks.claim response"),
                )
            })?)
        };
        Ok(RpcOutcome { status, body })
    }
}

#[async_trait::async_trait]
impl RpcHandler for DaemonRpcProcessor {
    async fn handle_rpc(
        &self,
        ctx: &tokio_util::sync::CancellationToken,
        identity: &ClientIdentity,
        method: &str,
        body: Option<&Value>,
    ) -> Result<RpcOutcome, RpcHandlerError> {
        match method {
            "tasks.claim" => self.claim_tasks(ctx, identity, body).await,
            _ => Err(RpcHandlerError::new(
                StatusCode::NOT_FOUND.as_u16(),
                anyhow::anyhow!("unknown rpc method {method:?}"),
            )),
        }
    }
}

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
    claim_tasks_by_runtime_core(&DaemonClaimServices::from_state(&state), headers, body).await
}

async fn claim_tasks_by_runtime_core(
    state: &DaemonClaimServices,
    headers: HeaderMap,
    body: Option<Json<BatchClaimRequest>>,
) -> Response {
    let Some(Json(req)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    if req.daemon_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "daemon_id is required");
    }
    let authenticated_daemon_id = daemon_id_of(daemon_context_from_headers(&headers));
    if !authenticated_daemon_id.is_empty() && authenticated_daemon_id != req.daemon_id {
        return error_response(StatusCode::FORBIDDEN, "daemon_id does not match token");
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
        let daemon_workspace = daemon_workspace_id(daemon_context_from_headers(&headers));
        let ok = match daemon_workspace.is_empty() {
            false => daemon_workspace == ws_id,
            true => {
                let user_id = request_user_id(&headers);
                !user_id.is_empty()
                    && claim_access_get_member(state, &user_id, &ws_id)
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
        let rt_workspace = rt.workspace_id.to_string();
        // Stale comment-plan repair: a claimed task whose trigger was cleared
        // (only coalesced survive) must never dispatch as a generic assignment.
        if task.trigger_comment_id.is_none() && !task.coalesced_comment_ids.is_empty() {
            match repair_stale_comment_plan(state, &task, &rt_workspace).await {
                RepairOutcome::NotApplicable => {}
                RepairOutcome::RepairedClean => continue,
                RepairOutcome::Failed(resp) => {
                    let _ = resp;
                    continue;
                }
            }
        }
        // Enriched payload (Go buildClaimedTaskResponse). A failure means the
        // task must not be dispatched; the builder cancelled it where the
        // semantics require it — skip it either way.
        let built = match crate::claim_response::build_claimed_task_response(
            state,
            &headers,
            &task,
            rt,
            &runtime_id.to_string(),
            &rt_workspace,
        )
        .await
        {
            Ok(b) => b,
            Err(failure) => {
                tracing::error!(
                    task_id = %task.id,
                    outcome = %failure.outcome,
                    "batch claim: builder rejected task"
                );
                continue;
            }
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
        match finalize_claim_enriched_with_runtime(state, &task, owner, &built, Some(rt)).await {
            Ok((auth_token, remote_mcp_token, receipt)) => {
                let mut payload = built.payload;
                if let Some(obj) = payload.as_object_mut() {
                    set_claim_tokens(obj, &auth_token, remote_mcp_token.as_deref(), &receipt);
                    insert_delivered_comment_ids(obj, built.delivered_comment_ids.clone());
                }
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

async fn claim_access_get_member(
    state: &DaemonClaimServices,
    user_id: &str,
    workspace_id: &str,
) -> Option<cordy_db::models::Member> {
    let user_id = Uuid::parse_str(user_id).ok()?;
    let workspace_id = Uuid::parse_str(workspace_id).ok()?;
    member::get_member_by_user_and_workspace(&state.pool, user_id, workspace_id)
        .await
        .ok()
        .flatten()
}

/// Outcome of the stale comment-plan repair (Go repairStaleCommentPlanIfNeeded).
enum RepairOutcome {
    /// The guard does not apply; proceed with a normal claim.
    NotApplicable,
    /// Cleanly repaired: survivors replayed through normal routing; report
    /// "no task" for this claim.
    RepairedClean,
    /// Hard failure: render this response and stop.
    Failed(Box<Response>),
}

/// Cancels a claimed task whose trigger_comment_id was cleared but whose
/// coalesced ids survive, then replays the surviving comments through normal
/// routing (which recomputes originator + connected-app context). A task whose
/// issue lives in another workspace is cancelled outright — its overlay still
/// belongs to a deleted author.
///
/// Port of Go `repairStaleCommentPlanIfNeeded`. Returns NotApplicable whenever
/// the guard does not fire so callers can fall through.
async fn repair_stale_comment_plan(
    state: &DaemonClaimServices,
    task: &cordy_db::models::AgentTaskQueue,
    runtime_workspace_id: &str,
) -> RepairOutcome {
    if task.trigger_comment_id.is_some() || task.coalesced_comment_ids.is_empty() {
        return RepairOutcome::NotApplicable;
    }
    let Some(issue_id) = task.issue_id else {
        return RepairOutcome::Failed(Box::new(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "comment task has no issue",
        )));
    };
    let issue = match issue::get_issue(&state.pool, issue_id).await {
        Ok(Some(i)) => i,
        Ok(None) => {
            return RepairOutcome::Failed(Box::new(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to repair stale comment task",
            )));
        }
        Err(_) => {
            return RepairOutcome::Failed(Box::new(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to repair stale comment task",
            )));
        }
    };
    if issue.workspace_id.to_string() != runtime_workspace_id {
        if let Err(e) = state.tasks.cancel_task(task.id).await {
            tracing::error!(
                error = %e,
                task_id = %task.id,
                "task claim: cancel stale cross-workspace task failed"
            );
        }
        return RepairOutcome::Failed(Box::new(error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "task workspace isolation check failed",
        )));
    }
    let mut survivors = Vec::with_capacity(task.coalesced_comment_ids.len());
    for comment_id in &task.coalesced_comment_ids {
        match comment_q::get_comment_in_workspace(&state.pool, *comment_id, issue.workspace_id)
            .await
        {
            Ok(Some(comment)) if comment.issue_id == issue.id => survivors.push(comment),
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, %comment_id, task_id = %task.id, "claim: load stale-plan survivor failed");
            }
        }
    }
    survivors.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });

    match state.tasks.cancel_task(task.id).await {
        Ok(_) => {
            if let Some(trigger) = survivors.pop() {
                let coalesced = survivors.into_iter().map(|comment| comment.id).collect();
                let replayed = state
                    .tasks
                    .enqueue_mention_task(
                        &issue,
                        task.agent_id,
                        Some(trigger.id),
                        coalesced,
                        task.is_leader_task,
                        task.squad_id,
                        task.force_fresh_session,
                        task.handoff_note.as_deref().unwrap_or(""),
                        None,
                        Some(task.id),
                    )
                    .await;
                if let Err(e) = replayed {
                    if !cordy_service::task_service::pending_slot_taken_err(&e) {
                        tracing::error!(error = %e, task_id = %task.id, "claim: replay stale-plan survivors failed");
                        return RepairOutcome::Failed(Box::new(error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "failed to repair stale comment task",
                        )));
                    }
                }
            }
            tracing::info!(
                task_id = %task.id,
                issue_id = %issue_id,
                "claim: repaired stale comment plan; survivors requeued"
            );
            RepairOutcome::RepairedClean
        }
        Err(e) => {
            tracing::warn!(error = %e, task_id = %task.id, "claim: stale comment plan cancel failed");
            RepairOutcome::Failed(Box::new(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to repair stale comment task",
            )))
        }
    }
}

/// Enriched finalization: mints the task token AND the Remote MCP daemon
/// credential when the payload carries remote MCP connections (Go
/// remoteMCPDaemonTokenForClaim + FinalizeTaskClaim), recording the exact
/// comment-delivery receipt for comment-backed tasks.
/// Returns (raw task token, optional raw daemon token, persisted receipt).
/// Runtime-aware variant used by the per-runtime claim path so the Remote MCP
/// daemon token can be bound to the claiming runtime's daemon.
#[allow(clippy::too_many_arguments)]
async fn finalize_claim_enriched_full(
    state: &DaemonClaimServices,
    task: &cordy_db::models::AgentTaskQueue,
    owner_id: Uuid,
    built: &crate::claim_response::BuiltClaim,
    runtime: Option<&AgentRuntime>,
) -> Result<(String, Option<String>, Vec<Uuid>), bool> {
    let token_str = cordy_auth::jwt::generate_agent_task_token().map_err(|_| false)?;
    let expires = chrono::Utc::now() + chrono::Duration::hours(24);
    let workspace_id = built
        .payload
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or(false)?;

    // Remote MCP broker credential: minted only when the claim actually carries
    // remote MCP connections (Go remoteMCPDaemonTokenForClaim). The raw token
    // rides only in this response; its hash commits atomically with the task
    // token below.
    let carries_remote_mcp = built
        .payload
        .get("remote_mcp_connections")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    let mut daemon_token: Option<cordy_service::task_service::CreateDaemonToken> = None;
    let mut raw_daemon_token: Option<String> = None;
    if carries_remote_mcp {
        let Some(runtime) = runtime else {
            tracing::error!(
                task_id = %task.id,
                "remote MCP claim requires a resolved runtime; requeueing"
            );
            return Err(true);
        };
        let Some(daemon_id) = runtime
            .daemon_id
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        else {
            tracing::error!(
                task_id = %task.id,
                "runtime daemon_id is required for Remote MCP; requeueing"
            );
            return Err(true);
        };
        let raw = cordy_auth::jwt::generate_daemon_token().map_err(|_| true)?;
        raw_daemon_token = Some(raw.clone());
        daemon_token = Some(cordy_service::task_service::CreateDaemonToken {
            token_hash: cordy_auth::jwt::hash_token(&raw),
            workspace_id: runtime.workspace_id,
            daemon_id: daemon_id.to_string(),
            expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(24)),
        });
    }

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
            daemon_token,
            built.delivered_comment_ids.clone(),
            built.comment_backed,
        )
        .await;
    match receipt {
        Ok(receipt) => Ok((token_str, raw_daemon_token, receipt)),
        Err(e) => {
            tracing::error!(error = %e, task_id = %task.id, "claim finalization failed");
            Err(true)
        }
    }
}

async fn finalize_claim_enriched_with_runtime(
    state: &DaemonClaimServices,
    task: &cordy_db::models::AgentTaskQueue,
    owner_id: Uuid,
    built: &crate::claim_response::BuiltClaim,
    runtime: Option<&AgentRuntime>,
) -> Result<(String, Option<String>, Vec<Uuid>), bool> {
    finalize_claim_enriched_full(state, task, owner_id, built, runtime).await
}

/// Writes the finalized tokens + receipt into the wire payload (Go sets these
/// after FinalizeTaskCommit succeeds).
fn set_claim_tokens(
    obj: &mut Map<String, Value>,
    auth_token: &str,
    remote_mcp_daemon_token: Option<&str>,
    receipt: &[Uuid],
) {
    obj.insert("auth_token".into(), Value::String(auth_token.to_string()));
    if let Some(token) = remote_mcp_daemon_token {
        obj.insert(
            "remote_mcp_daemon_token".into(),
            Value::String(token.to_string()),
        );
    }
    if !receipt.is_empty() {
        obj.insert(
            "delivered_comment_ids".into(),
            Value::Array(
                receipt
                    .iter()
                    .map(|u| Value::String(u.to_string()))
                    .collect(),
            ),
        );
    }
}

fn insert_delivered_comment_ids(obj: &mut Map<String, Value>, delivered: Vec<Uuid>) {
    if delivered.is_empty() {
        // [] is an authoritative empty receipt — keep it present.
        obj.entry("delivered_comment_ids")
            .or_insert_with(|| Value::Array(Vec::new()));
        return;
    }
    obj.insert(
        "delivered_comment_ids".into(),
        Value::Array(
            delivered
                .into_iter()
                .map(|u| Value::String(u.to_string()))
                .collect(),
        ),
    );
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
    let claim_services = DaemonClaimServices::from_state(&state);
    // Stale comment-plan repair (Go repairStaleCommentPlanIfNeeded): a claimed
    // task whose trigger was cleared must never dispatch as a generic assignment.
    if task.trigger_comment_id.is_none() && !task.coalesced_comment_ids.is_empty() {
        match repair_stale_comment_plan(&claim_services, &task, &ws_id).await {
            RepairOutcome::NotApplicable => {}
            RepairOutcome::RepairedClean => {
                return Json(json!({ "task": null })).into_response();
            }
            RepairOutcome::Failed(resp) => return *resp,
        }
    }
    let built = match crate::claim_response::build_claimed_task_response(
        &claim_services,
        &headers,
        &task,
        &rt,
        runtime_id.trim(),
        &ws_id,
    )
    .await
    {
        Ok(b) => b,
        Err(failure) => return failure.to_response(),
    };
    let Some(owner) = rt.owner_id else {
        tracing::error!(task_id = %task.id, "claim: runtime owner missing; cancelling task");
        let _ = state.tasks.cancel_task(task.id).await;
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime owner required to mint task token",
        );
    };
    match finalize_claim_enriched_with_runtime(&claim_services, &task, owner, &built, Some(&rt))
        .await
    {
        Ok((token, remote_mcp_token, receipt)) => {
            let mut payload = built.payload;
            if let Some(obj) = payload.as_object_mut() {
                set_claim_tokens(obj, &token, remote_mcp_token.as_deref(), &receipt);
                insert_delivered_comment_ids(obj, built.delivered_comment_ids.clone());
            }
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
    let mut selected = Vec::with_capacity(req.skills.len());
    for r in &req.skills {
        if r.id.is_empty() || r.source.is_empty() || r.hash.is_empty() {
            return error_response(StatusCode::BAD_REQUEST, "invalid skill ref");
        }
        let Some(found) = bundles
            .iter()
            .find(|b| b.source == r.source && b.id == r.id)
        else {
            return error_response(StatusCode::NOT_FOUND, "skill bundle not found");
        };
        if r.source == cordy_service::skill_bundle::SOURCE_PLUGIN && found.hash != r.hash {
            return error_response(StatusCode::CONFLICT, "skill bundle changed");
        }
        selected.push(found.clone());
    }
    Json(json!({ "bundles": selected })).into_response()
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
            crate::comment_triggers::reconcile_comments_on_completion(&state, &task).await;
            state.tasks.notify_task_finished(&task).await;
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
            state.tasks.notify_task_finished(&task).await;
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
    let runtime_provider = match task.runtime_id {
        Some(rid) => match runtime::get_agent_runtime(&state.pool, rid).await {
            Ok(Some(rt)) => normalize_provider(&rt.provider),
            _ => String::new(),
        },
        None => String::new(),
    };
    for u in &req.usage {
        let provider = normalize_provider(&u.provider);
        let provider = if provider.is_empty() {
            // Backfill from the runtime so generic ids like `auto` still price.
            runtime_provider.clone()
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

pub(crate) fn task_message_payload(
    m: &cordy_db::models::TaskMessage,
    issue_id: Option<Uuid>,
) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("task_id".into(), Value::String(m.task_id.to_string()));
    payload.insert("seq".into(), json!(m.seq));
    payload.insert("type".into(), Value::String(m.type_.clone()));
    if let Some(issue_id) = issue_id {
        payload.insert("issue_id".into(), Value::String(issue_id.to_string()));
    }
    if let Some(tool) = m.tool.as_deref().filter(|value| !value.is_empty()) {
        payload.insert("tool".into(), Value::String(tool.into()));
    }
    if let Some(content) = m.content.as_deref().filter(|value| !value.is_empty()) {
        payload.insert("content".into(), Value::String(content.into()));
    }
    if let Some(input) = m.input.as_ref().filter(|value| value.is_object()) {
        payload.insert("input".into(), input.clone());
    }
    if let Some(output) = m.output.as_deref().filter(|value| !value.is_empty()) {
        payload.insert("output".into(), Value::String(output.into()));
    }
    payload.insert(
        "created_at".into(),
        Value::String(crate::timefmt::rfc3339_nano(m.created_at)),
    );
    Value::Object(payload)
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
        let input = msg.input.as_ref().map(|m| {
            sanitize_json_for_postgres(Value::Object(cordy_service::redact::input_map(m)))
        });
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
                            "task_message": task_message_payload(&created, task.issue_id),
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
    let (task, _) = match require_daemon_task_access_with_workspace(&access, None, &task_id).await {
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
        Ok(rows) => Json(
            rows.iter()
                .map(|message| task_message_payload(message, task.issue_id))
                .collect::<Vec<_>>(),
        )
        .into_response(),
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
    let Ok(ws_uuid) = Uuid::parse_str(workspace_id.trim()) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid workspace_id");
    };
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
    let mut items: Vec<Value> = Vec::with_capacity(req.issue_ids.len());
    for (raw, uuid) in req.issue_ids.iter().zip(parsed.iter()) {
        match by_id.get(uuid) {
            Some((status, updated_at)) => {
                let effective = resolver.effective(&state.pool, status).await;
                items.push(json!({
                    "id": raw,
                    "found": true,
                    "status": effective,
                    "updated_at": updated_at.map(crate::timefmt::rfc3339),
                }));
            }
            None => items.push(json!({ "id": raw, "found": false })),
        }
    }
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
// Each loads the request from its Redis store, ignores stale reports for
// already-terminal rows (200 ok), and persists the transition. Without a
// wired store these validate the route contract and return 404 "request not
// found", which daemons treat as a dropped one-shot report.
// ---------------------------------------------------------------------------

async fn report_update_result(
    State(state): State<HandlerState>,
    Path((runtime_id, update_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let access = Access::new(&state, &headers);
    if require_daemon_runtime_access(&access, None, &runtime_id)
        .await
        .is_err()
    {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    let Some(store) = &state.update_store else {
        return error_response(StatusCode::NOT_FOUND, "update not found");
    };
    let existing = match store.get(update_id.trim()).await {
        Ok(Some(req)) => req,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "update not found"),
        Err(e) => {
            tracing::warn!(error = %e, update_id = %update_id, "load update failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to load update: {e}"),
            );
        }
    };
    if existing.runtime_id != runtime_id.trim() {
        return error_response(StatusCode::NOT_FOUND, "update not found");
    }
    if existing.status.is_terminal() {
        tracing::debug!(runtime_id = %runtime_id, update_id = %update_id, status = existing.status.as_str(), "ignoring stale update report");
        return Json(json!({ "status": "ok" })).into_response();
    }

    let Some(Json(body)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    let status = body.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let output = body
        .get("output")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let error = body
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    match status {
        "completed" => {
            if let Err(e) = store.complete(update_id.trim(), &output).await {
                tracing::error!(error = %e, update_id = %update_id, "UpdateStore Complete failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to persist completion",
                );
            }
        }
        "failed" => {
            if let Err(e) = store.fail(update_id.trim(), &error).await {
                tracing::error!(error = %e, update_id = %update_id, "UpdateStore Fail failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to persist failure",
                );
            }
        }
        // "running" is a progress signal: PopPending already flipped the row.
        "running" => {}
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid status: {status}"),
            )
        }
    }
    Json(json!({ "status": "ok" })).into_response()
}

async fn report_model_list_result(
    State(state): State<HandlerState>,
    Path((runtime_id, request_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let access = Access::new(&state, &headers);
    if require_daemon_runtime_access(&access, None, &runtime_id)
        .await
        .is_err()
    {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    let Some(store) = &state.model_list_store else {
        return error_response(StatusCode::NOT_FOUND, "request not found");
    };
    let existing = match store.get(request_id.trim()).await {
        Ok(Some(req)) => req,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "request not found"),
        Err(e) => {
            tracing::warn!(error = %e, request_id = %request_id, "load model list request failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to load request: {e}"),
            );
        }
    };
    if existing.runtime_id != runtime_id.trim() {
        return error_response(StatusCode::NOT_FOUND, "request not found");
    }
    if existing.status.is_terminal() {
        tracing::debug!(runtime_id = %runtime_id, request_id = %request_id, "ignoring stale model list report");
        return Json(json!({ "status": "ok" })).into_response();
    }

    let Some(Json(body)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    let body = match decode_model_list_report(body) {
        Ok(body) => body,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let status = body.status.as_str();
    match status {
        "completed" => {
            // Older daemons may omit `supported`; default true keeps the UI usable.
            let supported = body.supported.unwrap_or(true);
            let fallback = body.fallback.unwrap_or(false);
            let models = body.models;
            if let Err(e) = store.complete(request_id.trim(), &models, supported).await {
                tracing::error!(error = %e, request_id = %request_id, "ModelListStore Complete failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to persist completion",
                );
            }
            if let Some(cache) = state.model_catalog_cache.as_ref() {
                use crate::pending_store::{model_catalog_cache_action, ModelCatalogCacheAction};
                let result = match model_catalog_cache_action(&models, supported, fallback) {
                    ModelCatalogCacheAction::Store => {
                        cache.put(runtime_id.trim(), &models, supported).await
                    }
                    ModelCatalogCacheAction::Drop => cache.invalidate(runtime_id.trim()).await,
                    ModelCatalogCacheAction::Keep => Ok(()),
                };
                if let Err(error) = result {
                    tracing::warn!(%error, runtime_id = %runtime_id, "model catalog cache update failed");
                }
            }
        }
        _ => {
            if let Err(e) = store.fail(request_id.trim(), &body.error).await {
                tracing::error!(error = %e, request_id = %request_id, "ModelListStore Fail failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to persist failure",
                );
            }
        }
    }
    tracing::debug!(runtime_id = %runtime_id, request_id = %request_id, status = %status, "model list report");
    Json(json!({ "status": "ok" })).into_response()
}

#[derive(Debug, Default, Deserialize)]
struct ModelListReportBody {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    status: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    models: Vec<crate::pending_store::ModelEntry>,
    #[serde(default)]
    supported: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    error: String,
    #[serde(default)]
    fallback: Option<bool>,
}

fn decode_model_list_report(body: Value) -> Result<ModelListReportBody, serde_json::Error> {
    if body.is_null() {
        Ok(ModelListReportBody::default())
    } else {
        serde_json::from_value(body)
    }
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

async fn report_local_skill_list_result(
    State(state): State<HandlerState>,
    Path((runtime_id, request_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let access = Access::new(&state, &headers);
    if require_daemon_runtime_access(&access, None, &runtime_id)
        .await
        .is_err()
    {
        return error_response(StatusCode::NOT_FOUND, "not found");
    }
    let Some(store) = &state.local_skill_list_store else {
        return error_response(StatusCode::NOT_FOUND, "request not found");
    };
    let existing = match store.get(request_id.trim()).await {
        Ok(Some(req)) => req,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "request not found"),
        Err(e) => {
            tracing::warn!(error = %e, request_id = %request_id, "load local skill list request failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to load request: {e}"),
            );
        }
    };
    if existing.runtime_id != runtime_id.trim() {
        return error_response(StatusCode::NOT_FOUND, "request not found");
    }
    if existing.status.is_terminal() {
        tracing::debug!(runtime_id = %runtime_id, request_id = %request_id, "ignoring stale runtime local skills report");
        return Json(json!({ "status": "ok" })).into_response();
    }

    let Some(Json(body)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    let status = body.get("status").and_then(|v| v.as_str()).unwrap_or("");
    match status {
        "completed" => {
            let supported = body
                .get("supported")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let mcp_supported = body
                .get("mcp_supported")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let skills: Vec<crate::pending_store::RuntimeLocalSkillSummary> = body
                .get("skills")
                .cloned()
                .and_then(|m| serde_json::from_value(m).ok())
                .unwrap_or_default();
            let mcp_servers: Vec<crate::pending_store::RuntimeLocalMcpServerSummary> = body
                .get("mcp_servers")
                .cloned()
                .and_then(|m| serde_json::from_value(m).ok())
                .unwrap_or_default();
            if let Err(e) = store
                .complete(
                    request_id.trim(),
                    &skills,
                    supported,
                    &mcp_servers,
                    mcp_supported,
                )
                .await
            {
                tracing::error!(error = %e, request_id = %request_id, "local skills Complete failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to persist completion",
                );
            }
        }
        _ => {
            let error = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Err(e) = store.fail(request_id.trim(), &error).await {
                tracing::error!(error = %e, request_id = %request_id, "local skills Fail failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to persist failure",
                );
            }
        }
    }
    tracing::debug!(runtime_id = %runtime_id, request_id = %request_id, status = %status, "runtime local skills report");
    Json(json!({ "status": "ok" })).into_response()
}

async fn report_local_skill_import_result(
    State(state): State<HandlerState>,
    Path((runtime_id, request_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let access = Access::new(&state, &headers);
    let (rt, _) = match require_daemon_runtime_access(&access, None, &runtime_id).await {
        Ok(v) => v,
        Err(res) => return res,
    };
    let Some(store) = &state.local_skill_import_store else {
        return error_response(StatusCode::NOT_FOUND, "request not found");
    };
    let existing = match store.get(request_id.trim()).await {
        Ok(Some(req)) => req,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "request not found"),
        Err(e) => {
            tracing::warn!(error = %e, request_id = %request_id, "load import request failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to load request: {e}"),
            );
        }
    };
    if existing.runtime_id != runtime_id.trim() {
        return error_response(StatusCode::NOT_FOUND, "request not found");
    }
    if existing.status.is_terminal() {
        tracing::debug!(runtime_id = %runtime_id, request_id = %request_id, "ignoring stale runtime local skill import report");
        return Json(json!({ "status": "ok" })).into_response();
    }

    let Some(Json(body)) = body else {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    };
    let status = body.get("status").and_then(|v| v.as_str()).unwrap_or("");

    async fn fail_import(
        store: &crate::pending_store::LocalSkillImportStore,
        request_id: &str,
        fail_msg: &str,
    ) -> Response {
        if let Err(e) = store.fail(request_id, fail_msg).await {
            tracing::error!(error = %e, request_id = %request_id, "local skill import Fail failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to persist failure",
            );
        }
        Json(json!({ "status": "ok" })).into_response()
    }

    if status != "completed" {
        return fail_import(
            store,
            request_id.trim(),
            body.get("error").and_then(|v| v.as_str()).unwrap_or(""),
        )
        .await;
    }
    let Some(skill) = body.get("skill") else {
        return fail_import(
            store,
            request_id.trim(),
            "daemon returned an empty skill bundle",
        )
        .await;
    };

    // Persist the imported skill and every supporting file in one Postgres
    // transaction. Redis completion is intentionally outside that transaction;
    // create rolls the inserted skill back on a completion failure, while
    // overwrite is idempotent and is retried without deleting prior state.
    let creator_uuid = match Uuid::parse_str(existing.creator_id.trim()) {
        Ok(u) => u,
        Err(_) => {
            if let Err(e) = store
                .fail(
                    request_id.trim(),
                    "stored local skill import creator_id is invalid",
                )
                .await
            {
                tracing::error!(error = %e, request_id = %request_id, "local skill import Fail failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "stored local skill import creator_id is invalid",
                );
            }
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "stored local skill import creator_id is invalid",
            );
        }
    };
    let name = existing.name.clone().unwrap_or_else(|| {
        skill
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    });
    let description = existing
        .description
        .clone()
        .or_else(|| {
            skill
                .get("description")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let content = skill
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let config = json!({
        "origin": {
            "type": "runtime_local",
            "runtime_id": runtime_id,
            "provider": skill.get("provider").and_then(|v| v.as_str()).unwrap_or(""),
            "source_path": skill.get("source_path").and_then(|v| v.as_str()).unwrap_or(""),
        }
    });
    let files: Vec<(String, String)> = skill
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|file| {
            let path = file.get("path").and_then(Value::as_str).unwrap_or("");
            validate_file_path(path).then(|| {
                (
                    sanitize(path),
                    sanitize(file.get("content").and_then(Value::as_str).unwrap_or("")),
                )
            })
        })
        .collect();
    let sanitized_name = sanitize(&name);
    let sanitized_description = sanitize(&description);
    let sanitized_content = sanitize(&content);
    let is_overwrite = existing.action == "overwrite";

    // Create path: detect a same-name conflict before writing.
    if !is_overwrite {
        match cordy_db::queries::skill::get_skill_by_workspace_and_name(
            &state.pool,
            rt.workspace_id,
            sanitized_name.trim(),
        )
        .await
        {
            Ok(Some(conflicting)) => {
                let conflict = crate::pending_store::LocalSkillImportConflict {
                    existing_skill_id: conflicting.id.to_string(),
                    existing_created_by: conflicting
                        .created_by
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                    can_overwrite: conflicting.created_by == Some(creator_uuid),
                };
                if let Err(e) = store.conflict(request_id.trim(), conflict).await {
                    tracing::error!(error = %e, request_id = %request_id, "local skill import Conflict failed");
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to persist conflict",
                    );
                }
                return Json(json!({ "status": "ok" })).into_response();
            }
            Ok(None) => {}
            Err(e) => {
                return fail_import(
                    store,
                    request_id.trim(),
                    &format!("failed to check for existing skill: {e}"),
                )
                .await;
            }
        }
    }

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            return fail_import(
                store,
                request_id.trim(),
                &format!("failed to begin skill import: {e}"),
            )
            .await
        }
    };
    let persisted = if is_overwrite {
        let target_id = match Uuid::parse_str(existing.target_skill_id.trim()) {
            Ok(id) => id,
            Err(_) => {
                return fail_import(
                    store,
                    request_id.trim(),
                    "stored target_skill_id is invalid",
                )
                .await
            }
        };
        let target = match cordy_db::queries::skill::get_skill_in_workspace(
            &mut *tx,
            target_id,
            rt.workspace_id,
        )
        .await
        {
            Ok(Some(target)) => target,
            Ok(None) => {
                return fail_import(store, request_id.trim(), "target skill no longer exists").await
            }
            Err(e) => {
                return fail_import(
                    store,
                    request_id.trim(),
                    &format!("failed to load target skill: {e}"),
                )
                .await
            }
        };
        if target.created_by != Some(creator_uuid) {
            return fail_import(
                store,
                request_id.trim(),
                "you no longer have permission to overwrite this skill",
            )
            .await;
        }
        if target.name != sanitized_name {
            return fail_import(
                store,
                request_id.trim(),
                "target skill name no longer matches the imported skill",
            )
            .await;
        }
        if let Err(e) =
            cordy_db::queries::skill::delete_skill_files_by_skill(&mut *tx, target.id).await
        {
            return fail_import(
                store,
                request_id.trim(),
                &format!("failed to replace skill files: {e}"),
            )
            .await;
        }
        match cordy_db::queries::skill::update_skill(
            &mut *tx,
            target.id,
            Some(&sanitized_name),
            Some(&sanitized_description),
            Some(&sanitized_content),
            Some(&config),
        )
        .await
        {
            Ok(Some(updated)) => updated,
            Ok(None) => {
                return fail_import(store, request_id.trim(), "target skill no longer exists").await
            }
            Err(e) => {
                return fail_import(
                    store,
                    request_id.trim(),
                    &format!("failed to overwrite skill: {e}"),
                )
                .await
            }
        }
    } else {
        match cordy_db::queries::skill::create_skill(
            &mut *tx,
            rt.workspace_id,
            &sanitized_name,
            &sanitized_description,
            &sanitized_content,
            &config,
            creator_uuid,
        )
        .await
        {
            Ok(Some(created)) => created,
            Ok(None) => {
                return fail_import(
                    store,
                    request_id.trim(),
                    "a skill with this name already exists",
                )
                .await
            }
            Err(e) if is_unique_violation(&e) => {
                return fail_import(
                    store,
                    request_id.trim(),
                    "a skill with this name already exists",
                )
                .await
            }
            Err(e) => {
                return fail_import(
                    store,
                    request_id.trim(),
                    &format!("failed to create skill: {e}"),
                )
                .await
            }
        }
    };

    let mut persisted_files = Vec::with_capacity(files.len());
    for (path, file_content) in &files {
        match cordy_db::queries::skill::upsert_skill_file(
            &mut *tx,
            persisted.id,
            path,
            file_content,
        )
        .await
        {
            Ok(Some(file)) => persisted_files.push(file),
            Ok(None) => {
                return fail_import(store, request_id.trim(), "failed to persist skill file").await
            }
            Err(e) => {
                return fail_import(
                    store,
                    request_id.trim(),
                    &format!("failed to persist skill file: {e}"),
                )
                .await
            }
        }
    }
    if let Err(e) = tx.commit().await {
        return fail_import(
            store,
            request_id.trim(),
            &format!("failed to commit skill import: {e}"),
        )
        .await;
    }

    let resp_skill = json!({
        "id": persisted.id.to_string(),
        "workspace_id": persisted.workspace_id.to_string(),
        "name": persisted.name,
        "description": persisted.description,
        "content": persisted.content,
        "config": persisted.config,
        "created_by": persisted.created_by.map(|u| u.to_string()),
        "created_at": crate::timefmt::rfc3339(persisted.created_at),
        "updated_at": crate::timefmt::rfc3339(persisted.updated_at),
        "files": persisted_files,
    });
    if let Err(e) = store.complete(request_id.trim(), resp_skill.clone()).await {
        // The skill already landed in Postgres; roll it back so the daemon's
        // retry lands on a clean slate instead of hitting the unique-name
        // constraint forever.
        tracing::error!(
            error = %e,
            request_id = %request_id,
            skill_id = %persisted.id,
            "local skill import Complete failed — rolling back created skill"
        );
        if !is_overwrite {
            let _ =
                cordy_db::queries::skill::delete_skill(&state.pool, persisted.id, rt.workspace_id)
                    .await;
        }
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to persist import completion",
        );
    }
    state.bus.publish(&cordy_events::Event {
        event_type: if is_overwrite {
            cordy_protocol::EVENT_SKILL_UPDATED
        } else {
            cordy_protocol::EVENT_SKILL_CREATED
        }
        .to_string(),
        workspace_id: rt.workspace_id.to_string(),
        actor_type: "member".to_string(),
        actor_id: existing.creator_id.clone(),
        payload: json!({ "skill": resp_skill }),
        task_id: String::new(),
        ..Default::default()
    });
    tracing::debug!(runtime_id = %runtime_id, request_id = %request_id, skill_id = %persisted.id, "runtime local skill imported");
    Json(json!({ "status": "ok" })).into_response()
}

fn is_unique_violation(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(|error| error.as_database_error())
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23505")
}

/// Go validateFilePath: rejects absolute paths and `..` escapes so a daemon
/// cannot write outside the skill's file namespace.
fn validate_file_path(p: &str) -> bool {
    if p.is_empty() {
        return false;
    }
    let normalized = p.replace('\\', "/");
    if normalized.starts_with('/') {
        return false;
    }
    !normalized.split('/').any(|seg| seg == "..")
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

    fn lazy_test_state() -> HandlerState {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://cordy:cordy@127.0.0.1/cordy")
            .expect("test database URL is valid");
        HandlerState::new(pool, cordy_auth::pat_cache::PatCache::disabled(), None)
    }

    #[tokio::test]
    async fn websocket_heartbeat_rejects_malformed_runtime_before_database_access() {
        let state = lazy_test_state();
        let processor = DaemonHeartbeatProcessor::from_state(&state);

        let error = processor
            .handle_heartbeat(&ClientIdentity::default(), "not-a-uuid", false)
            .await
            .expect_err("malformed runtime IDs must fail the heartbeat");

        assert!(
            error.to_string().contains("invalid runtime_id"),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn batch_claim_rejects_daemon_token_spoof_before_database_access() {
        let state = lazy_test_state();
        let services = DaemonClaimServices::from_state(&state);
        let mut headers = HeaderMap::new();
        headers.insert(
            cordy_middleware::daemon_auth::DAEMON_ID_HEADER,
            HeaderValue::from_static("authenticated-daemon"),
        );
        headers.insert(
            cordy_middleware::daemon_auth::DAEMON_WORKSPACE_HEADER,
            HeaderValue::from_static("workspace-1"),
        );

        let response = claim_tasks_by_runtime_core(
            &services,
            headers,
            Some(Json(BatchClaimRequest {
                daemon_id: "spoofed-daemon".to_string(),
                runtime_ids: Vec::new(),
                max_tasks: 1,
            })),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"error":"daemon_id does not match token"})
        );
    }

    #[tokio::test]
    async fn websocket_rpc_rejects_unknown_methods_and_cancelled_claims() {
        let state = lazy_test_state();
        let processor = DaemonRpcProcessor::from_state(&state);
        let ctx = tokio_util::sync::CancellationToken::new();
        let unknown = processor
            .handle_rpc(&ctx, &ClientIdentity::default(), "unknown", None)
            .await;
        match unknown {
            Err(error) => assert_eq!(error.status, StatusCode::NOT_FOUND.as_u16()),
            Ok(_) => panic!("unknown RPC method unexpectedly succeeded"),
        }

        ctx.cancel();
        let claim_body = json!({
            "daemon_id": "daemon-1",
            "runtime_ids": [],
            "max_tasks": 1
        });
        let cancelled = processor
            .handle_rpc(
                &ctx,
                &ClientIdentity {
                    daemon_id: "daemon-1".to_string(),
                    workspace_id: "workspace-1".to_string(),
                    ..ClientIdentity::default()
                },
                "tasks.claim",
                Some(&claim_body),
            )
            .await;
        match cancelled {
            Err(error) => assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE.as_u16()),
            Ok(_) => panic!("cancelled RPC claim unexpectedly succeeded"),
        }
    }

    #[test]
    fn model_list_report_rejects_malformed_typed_fields_before_mutation() {
        for body in [
            json!({"status":"completed","models":[{"id":123}],"supported":true}),
            json!({"status":"completed","models":[],"supported":"yes"}),
            json!({"status":"completed","models":[],"fallback":"yes"}),
            json!({
                "status":"completed",
                "models":[{"id":"m","thinking":{"supported_levels":[{"value":7}]}}]
            }),
            json!({
                "status":"completed",
                "models":[{"id":"m","service_tiers":[{"id":7}]}]
            }),
        ] {
            assert!(decode_model_list_report(body).is_err());
        }
    }

    #[test]
    fn model_list_report_preserves_go_null_and_omitted_defaults() {
        let null_report = decode_model_list_report(Value::Null).unwrap();
        assert!(null_report.status.is_empty());
        assert!(null_report.models.is_empty());
        assert!(null_report.error.is_empty());
        assert!(null_report.supported.is_none());
        assert!(null_report.fallback.is_none());

        let report = decode_model_list_report(json!({
            "status": "completed",
            "models": [{
                "id": null,
                "label": null,
                "default": null,
                "thinking": {"supported_levels": null, "default_level": null},
                "service_tiers": null
            }],
            "supported": null,
            "fallback": null
        }))
        .unwrap();
        assert_eq!(report.models.len(), 1);
        assert!(report.models[0].id.is_empty());
        assert!(report.models[0].label.is_empty());
        assert!(!report.models[0].default);
        assert!(report.models[0]
            .thinking
            .as_ref()
            .unwrap()
            .supported_levels
            .is_empty());
        assert!(report.models[0].service_tiers.is_empty());
        assert!(report.supported.unwrap_or(true));
        assert!(!report.fallback.unwrap_or(false));
    }

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

    #[test]
    fn sanitize_json_for_postgres_removes_nested_nuls() {
        let value = json!({
            "outer\0key": ["a\0b", {"inner": "c\0d"}],
            "number": 7,
        });
        assert_eq!(
            sanitize_json_for_postgres(value),
            json!({
                "outerkey": ["ab", {"inner": "cd"}],
                "number": 7,
            })
        );
    }

    #[test]
    fn task_message_payload_matches_go_omitempty_contract() {
        let task_id = Uuid::parse_str("018f946a-1234-7890-abcd-1234567890ab").unwrap();
        let issue_id = Uuid::parse_str("018f946a-5678-7890-abcd-1234567890ab").unwrap();
        let message = cordy_db::models::TaskMessage {
            content: None,
            created_at: "2026-08-23T12:34:56.123Z".parse().unwrap(),
            id: Uuid::parse_str("018f946a-9abc-7890-abcd-1234567890ab").unwrap(),
            input: Some(json!({"path": "README.md"})),
            output: Some(String::new()),
            seq: 7,
            task_id,
            tool: None,
            type_: "tool_call".into(),
        };

        let payload = task_message_payload(&message, Some(issue_id));

        assert_eq!(payload["task_id"], json!(task_id.to_string()));
        assert_eq!(payload["issue_id"], json!(issue_id.to_string()));
        assert_eq!(payload["seq"], json!(7));
        assert_eq!(payload["type"], json!("tool_call"));
        assert_eq!(payload["input"], json!({"path": "README.md"}));
        assert_eq!(payload["created_at"], json!("2026-08-23T12:34:56.123Z"));
        assert!(payload.get("tool").is_none());
        assert!(payload.get("content").is_none());
        assert!(payload.get("output").is_none());
    }

    #[test]
    fn set_claim_tokens_includes_remote_mcp_credential() {
        let mut payload = Map::new();
        set_claim_tokens(&mut payload, "task-token", Some("mcp-token"), &[]);
        assert_eq!(payload["auth_token"], json!("task-token"));
        assert_eq!(payload["remote_mcp_daemon_token"], json!("mcp-token"));
    }

    // Contract: a missing runtime row maps to 404 (daemon drops + re-registers)
    // while any other DB error maps to 500. Exercised against a live Postgres in
    // integration; the pure mapping helpers are covered above.
}
