//! Runtime collection and editable metadata handlers.

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use patchbay_authorization::{
    Action, AuthorizationContext, AuthorizationRequest, Principal, PrincipalType, Resource,
    ResourceType, WorkspaceRole,
};
use patchbay_db::models::{AgentRuntime, Member};
use patchbay_db::queries::{agent, runtime, runtime_profile};
use patchbay_middleware::workspace::WorkspaceContext;
use patchbay_protocol::EVENT_DAEMON_REGISTER;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const MAX_CUSTOM_NAME_CHARS: usize = 100;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/runtimes", get(list))
        .route("/api/runtimes/", get(list))
        .route("/api/runtimes/{runtime_id}", patch(update))
        .route("/api/runtimes/{runtime_id}/", patch(update))
        .route("/api/runtimes/{runtime_id}", delete(delete_runtime))
        .route(
            "/api/runtimes/{runtime_id}/unbind-agents-and-delete",
            post(delete_runtime_confirmed),
        )
        // Compatibility boundary for installed clients. It deliberately runs
        // the current unbind semantics; no user agent is archived or deleted.
        .route(
            "/api/runtimes/{runtime_id}/archive-agents-and-delete",
            post(delete_runtime_confirmed),
        )
}

fn runtime_profile_conflict() -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "cannot delete a custom runtime instance directly; delete its runtime profile instead.",
            "code": "runtime_profile_instance_delete_unsupported"
        })),
    )
        .into_response()
}

fn redact_gateway_token(mut config: Value) -> Value {
    let has_token = config
        .get_mut("gateway")
        .and_then(Value::as_object_mut)
        .and_then(|gateway| gateway.get_mut("token"))
        .and_then(|value| value.as_str())
        .filter(|token| !token.is_empty())
        .is_some();
    if has_token {
        config["gateway"]["token"] = Value::String("***".into());
    }
    config
}

fn workspace_role(member: &Member) -> Option<WorkspaceRole> {
    match member.role.as_str() {
        "owner" => Some(WorkspaceRole::Owner),
        "admin" => Some(WorkspaceRole::Admin),
        "member" => Some(WorkspaceRole::Member),
        _ => None,
    }
}

async fn runtime_allowed(
    state: &HandlerState,
    member: &Member,
    headers: &HeaderMap,
    runtime: &AgentRuntime,
    action: &'static str,
) -> Result<bool, Response> {
    let task_principal = headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        == Some("task_token");
    let task_id = headers
        .get("x-task-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok());
    let lease_id = headers
        .get("x-capability-lease-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok());
    let on_behalf_of_user_id = headers
        .get("x-on-behalf-of-user-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok());
    let device_id = headers
        .get("x-device-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok());
    let via_agent_id = headers
        .get("x-agent-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok());
    let decision = state
        .authorization
        .authorize(AuthorizationRequest {
            principal: Principal {
                principal_type: if task_principal {
                    PrincipalType::TaskRun
                } else {
                    PrincipalType::User
                },
                id: if task_principal { task_id } else { Some(member.user_id) },
            },
            action: Action::new(action),
            resource: Resource {
                resource_type: ResourceType::new(ResourceType::RUNTIME),
                id: Some(runtime.id),
                workspace_id: runtime.workspace_id,
                owner_id: runtime.owner_id,
                attributes: json!({
                    "private": runtime.visibility != "public",
                    "local_device": runtime.runtime_mode == "local",
                }),
            },
            context: AuthorizationContext {
                workspace_role: workspace_role(member),
                on_behalf_of_user_id: task_principal.then_some(on_behalf_of_user_id).flatten(),
                via_agent_id: task_principal.then_some(via_agent_id).flatten(),
                device_id,
                task_id: task_principal.then_some(task_id).flatten(),
                lease_id: task_principal.then_some(lease_id).flatten(),
                ..Default::default()
            },
            delegation_chain: Vec::new(),
        })
        .await
        .map_err(|error| {
            tracing::error!(%error, runtime_id = %runtime.id, action, "runtime authorization failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to authorize runtime access",
            )
        })?;
    Ok(decision.is_allowed())
}

fn safe_agent_response(state: &HandlerState, agent: &patchbay_db::models::Agent) -> Value {
    let env_count = agent.custom_env.as_object().map_or(0, serde_json::Map::len);
    let runtime_id = agent
        .runtime_id
        .map(|id| id.to_string())
        .unwrap_or_default();
    json!({
        "id": agent.id,
        "workspace_id": agent.workspace_id,
        "runtime_id": runtime_id,
        "runtime_bound": agent.runtime_id.is_some(),
        "name": agent.name,
        "description": agent.description,
        "instructions": agent.instructions,
        "system_key": agent.system_key,
        "avatar_url": agent.avatar_url.as_deref().map(|raw| crate::avatar::resolve_url(state, raw)),
        "runtime_mode": agent.runtime_mode,
        "runtime_config": redact_gateway_token(agent.runtime_config.clone()),
        "custom_args": agent.custom_args,
        "mcp_config": Value::Null,
        "mcp_config_redacted": agent.mcp_config.is_some(),
        "has_custom_env": env_count > 0,
        "custom_env_key_count": env_count,
        "visibility": agent.visibility,
        "permission_mode": agent.permission_mode,
        "invocation_targets": [],
        "status": agent.status,
        "max_concurrent_tasks": agent.max_concurrent_tasks,
        "model": agent.model.as_deref().unwrap_or_default(),
        "thinking_level": agent.thinking_level.as_deref().unwrap_or_default(),
        "session_mode": agent.session_mode.as_deref().unwrap_or_default(),
        "service_tier": agent.service_tier.as_deref().unwrap_or_default(),
        "composio_toolkit_allowlist": Value::Null,
        "composio_toolkit_allowlist_redacted": agent.composio_toolkit_allowlist.is_some(),
        "owner_id": agent.owner_id,
        "skills": [],
        "disabled_runtime_skills": agent.disabled_runtime_skills,
        "created_at": crate::timefmt::rfc3339(agent.created_at),
        "updated_at": crate::timefmt::rfc3339(agent.updated_at),
        "archived_at": agent.archived_at.map(crate::timefmt::rfc3339),
        "archived_by": agent.archived_by,
    })
}

fn active_agents_conflict(
    state: &HandlerState,
    agents: &[patchbay_db::models::Agent],
    plan_changed: bool,
) -> Response {
    let (message, code) = if plan_changed {
        (
            "the active agent set changed; please review and confirm again.",
            "runtime_delete_plan_changed",
        )
    } else {
        (
            "cannot delete runtime: it has active agents bound to it. Reassign them or confirm unbinding them first.",
            "runtime_has_active_agents",
        )
    };
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": message,
            "code": code,
            "active_agents": agents.iter().map(|agent| safe_agent_response(state, agent)).collect::<Vec<_>>()
        })),
    )
        .into_response()
}

async fn load_deletable_runtime(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    raw_id: &str,
) -> Result<AgentRuntime, Response> {
    let runtime_id = Uuid::parse_str(raw_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"))?;
    let found = runtime::get_agent_runtime(&state.pool, runtime_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "runtime not found"))?;
    if found.workspace_id != context.member.workspace_id {
        return Err(error_response(StatusCode::NOT_FOUND, "runtime not found"));
    }
    if !runtime_allowed(
        state,
        &context.member,
        headers,
        &found,
        Action::RUNTIME_UPDATE,
    )
    .await?
    {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "you can only delete your own runtimes",
        ));
    }
    if let Some(profile_id) = found.profile_id {
        match runtime_profile::get_runtime_profile_for_workspace(
            &state.pool,
            profile_id,
            found.workspace_id,
        )
        .await
        {
            Ok(Some(_)) => return Err(runtime_profile_conflict()),
            Ok(None) => {
                tracing::warn!(%runtime_id, %profile_id, "deleting orphaned profile runtime")
            }
            Err(_) => {
                return Err(error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to check runtime profile",
                ))
            }
        }
    }
    Ok(found)
}

async fn locked_delete(
    state: &HandlerState,
    context: &WorkspaceContext,
    found: &AgentRuntime,
    expected: Option<&[Uuid]>,
) -> Response {
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete runtime",
            )
        }
    };
    if runtime::lock_agent_runtime(&mut *transaction, found.id)
        .await
        .ok()
        .flatten()
        .is_none()
        || agent::list_user_agents_by_runtime_for_update(&mut *transaction, found.id)
            .await
            .is_err()
    {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to lock runtime");
    }
    let active =
        match agent::list_active_agents_by_runtime_for_update(&mut *transaction, found.id).await {
            Ok(value) => value,
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to enumerate active agents",
                )
            }
        };
    let mut current_ids = active.iter().map(|value| value.id).collect::<Vec<_>>();
    current_ids.sort_unstable();
    if let Some(expected) = expected {
        let mut expected = expected.to_vec();
        expected.sort_unstable();
        if expected != current_ids {
            return active_agents_conflict(state, &active, true);
        }
    } else if !active.is_empty() {
        return active_agents_conflict(state, &active, false);
    }
    let teardown = match crate::runtime_profile::teardown_runtime(&mut transaction, found.id).await
    {
        Ok(value) => value,
        Err(error) if error.to_string().contains("runtime_delete_not_drained") => {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "the runtime still has tasks in flight; retry in a moment.",
                    "code": "runtime_delete_not_drained"
                })),
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!(%error, runtime_id = %found.id, "runtime teardown failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete runtime",
            );
        }
    };
    if runtime::delete_agent_runtime(&mut *transaction, found.id)
        .await
        .is_err()
        || transaction.commit().await.is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to delete runtime",
        );
    }
    state
        .tasks
        .broadcast_cancelled_tasks(&found.workspace_id.to_string(), &teardown.cancelled_tasks)
        .await;
    crate::runtime_profile::publish_teardown(
        state,
        found.workspace_id,
        context.member.user_id,
        &teardown,
    );
    Json(json!({"status": "ok"})).into_response()
}

async fn delete_runtime(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let found = match load_deletable_runtime(&state, &context, &headers, &raw_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match agent::list_active_agents_by_runtime(&state.pool, found.id).await {
        Ok(active) if !active.is_empty() => return active_agents_conflict(&state, &active, false),
        Ok(_) => {}
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to check runtime dependencies",
            )
        }
    }
    locked_delete(&state, &context, &found, None).await
}

#[derive(Deserialize)]
struct ConfirmDeleteRequest {
    expected_active_agent_ids: Vec<String>,
}

async fn delete_runtime_confirmed(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
    Json(request): Json<ConfirmDeleteRequest>,
) -> Response {
    let mut expected = Vec::with_capacity(request.expected_active_agent_ids.len());
    for raw in request.expected_active_agent_ids {
        match Uuid::parse_str(&raw) {
            Ok(id) => expected.push(id),
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "expected_active_agent_ids must be a list of valid UUIDs",
                )
            }
        }
    }
    expected.sort_unstable();
    expected.dedup();
    let found = match load_deletable_runtime(&state, &context, &headers, &raw_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    locked_delete(&state, &context, &found, Some(&expected)).await
}

#[derive(Debug, Serialize)]
struct RuntimeResponse {
    id: String,
    workspace_id: String,
    daemon_id: Option<String>,
    name: String,
    custom_name: Option<String>,
    runtime_mode: String,
    provider: String,
    launch_header: &'static str,
    status: String,
    device_info: String,
    metadata: Value,
    owner_id: Option<String>,
    visibility: String,
    profile_id: Option<String>,
    last_seen_at: Option<String>,
    created_at: String,
    updated_at: String,
}

impl From<AgentRuntime> for RuntimeResponse {
    fn from(runtime: AgentRuntime) -> Self {
        Self {
            id: runtime.id.to_string(),
            workspace_id: runtime.workspace_id.to_string(),
            daemon_id: runtime.daemon_id,
            name: runtime.name,
            custom_name: runtime.custom_name,
            runtime_mode: runtime.runtime_mode,
            launch_header: crate::daemon::launch_header(&runtime.provider),
            provider: runtime.provider,
            status: runtime.status,
            device_info: runtime.device_info,
            metadata: if runtime.metadata.is_null() {
                json!({})
            } else {
                runtime.metadata
            },
            owner_id: runtime.owner_id.map(|id| id.to_string()),
            visibility: runtime.visibility,
            profile_id: runtime.profile_id.map(|id| id.to_string()),
            last_seen_at: runtime.last_seen_at.map(crate::timefmt::rfc3339),
            created_at: crate::timefmt::rfc3339(runtime.created_at),
            updated_at: crate::timefmt::rfc3339(runtime.updated_at),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ListParams {
    owner: Option<String>,
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Query(params): Query<ListParams>,
) -> Response {
    let workspace_id = match Uuid::parse_str(&context.workspace_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace_id"),
    };
    let result = if params.owner.as_deref() == Some("me") {
        runtime::list_agent_runtimes_by_owner(&state.pool, workspace_id, context.member.user_id)
            .await
    } else {
        runtime::list_agent_runtimes(&state.pool, workspace_id).await
    };
    match result {
        Ok(runtimes) => {
            let mut visible = Vec::with_capacity(runtimes.len());
            for runtime in runtimes {
                match runtime_allowed(
                    &state,
                    &context.member,
                    &headers,
                    &runtime,
                    Action::RUNTIME_READ,
                )
                .await
                {
                    Ok(true) => visible.push(RuntimeResponse::from(runtime)),
                    Ok(false) => {}
                    Err(response) => return response,
                }
            }
            Json(visible).into_response()
        }
        Err(error) => {
            tracing::error!(%error, "failed to list runtimes");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list runtimes")
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct UpdateRequest {
    visibility: Option<String>,
    custom_name: Option<String>,
    #[serde(deserialize_with = "null_default")]
    apply_to_machine: bool,
}

fn decode_update(body: &[u8]) -> Result<UpdateRequest, ()> {
    let mut decoder = serde_json::Deserializer::from_slice(body);
    match Value::deserialize(&mut decoder).map_err(|_| ())? {
        Value::Null => Ok(UpdateRequest::default()),
        value => serde_json::from_value(value).map_err(|_| ()),
    }
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn can_set_visibility(member: &Member, runtime: &AgentRuntime) -> bool {
    runtime.owner_id == Some(member.user_id)
}

async fn update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
    body: Bytes,
) -> Response {
    let runtime_id = match Uuid::parse_str(&raw_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"),
    };
    let mut found = match runtime::get_agent_runtime(&state.pool, runtime_id).await {
        Ok(Some(found)) => found,
        Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "runtime not found"),
    };
    if found.workspace_id != context.member.workspace_id {
        return error_response(StatusCode::NOT_FOUND, "runtime not found");
    }
    match runtime_allowed(
        &state,
        &context.member,
        &headers,
        &found,
        Action::RUNTIME_UPDATE,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => {
            return error_response(StatusCode::FORBIDDEN, "you can only edit your own runtimes")
        }
        Err(response) => return response,
    }

    let request = match decode_update(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid JSON body"),
    };

    let mut visibility_change = None;
    if let Some(visibility) = request.visibility.as_deref() {
        if !matches!(visibility, "private" | "public") {
            return error_response(
                StatusCode::BAD_REQUEST,
                "visibility must be 'private' or 'public'",
            );
        }
        if visibility != found.visibility {
            if !can_set_visibility(&context.member, &found) {
                return error_response(
                    StatusCode::FORBIDDEN,
                    "only the runtime owner can change its visibility",
                );
            }
            visibility_change = Some(visibility);
        }
    }

    let custom_name = request.custom_name.as_deref().map(str::trim);
    if custom_name.is_some_and(|name| name.chars().count() > MAX_CUSTOM_NAME_CHARS) {
        return error_response(StatusCode::BAD_REQUEST, "custom name is too long");
    }

    let mut changed = false;
    if let Some(visibility) = visibility_change {
        found = match runtime::update_agent_runtime_visibility(&state.pool, visibility, found.id)
            .await
        {
            Ok(Some(updated)) => updated,
            Ok(None) | Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to update runtime",
                )
            }
        };
        changed = true;
    }

    if let Some(name) = custom_name {
        let stored = (!name.is_empty()).then_some(name);
        if request.apply_to_machine && found.daemon_id.is_some() {
            // Machine-wide rename remains actor-owned even when a standing
            // grant authorizes one exact runtime. A single grant must not fan
            // out across another user's runtimes on the same daemon.
            let owner_filter = Some(context.member.user_id);
            let rows = match runtime::update_agent_runtime_custom_name_by_daemon(
                &state.pool,
                stored,
                found.workspace_id,
                found.daemon_id.as_deref(),
                owner_filter,
            )
            .await
            {
                Ok(rows) => rows,
                Err(error) => {
                    tracing::error!(%error, runtime_id = %found.id, "failed to rename runtime machine");
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to update runtime",
                    );
                }
            };
            if let Some(updated) = rows.into_iter().find(|row| row.id == found.id) {
                found = updated;
            }
        } else {
            found = match runtime::update_agent_runtime_custom_name(&state.pool, stored, found.id)
                .await
            {
                Ok(Some(updated)) => updated,
                Ok(None) | Err(_) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to update runtime",
                    )
                }
            };
        }
        changed = true;
    }

    if changed {
        state.bus.publish(&patchbay_events::Event {
            event_type: EVENT_DAEMON_REGISTER.into(),
            workspace_id: found.workspace_id.to_string(),
            actor_type: "member".into(),
            actor_id: context.member.user_id.to_string(),
            payload: json!({ "action": "update" }),
            ..Default::default()
        });
    }

    Json(RuntimeResponse::from(found)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_agent() -> patchbay_db::models::Agent {
        let now = chrono::Utc::now();
        patchbay_db::models::Agent {
            archived_at: None,
            archived_by: None,
            avatar_url: None,
            composio_toolkit_allowlist: Some(vec!["github".into()]),
            created_at: now,
            custom_args: json!(["--safe"]),
            custom_env: json!({"SECRET": "do-not-leak"}),
            description: "description".into(),
            disabled_runtime_skills: json!([]),
            id: Uuid::new_v4(),
            instructions: "instructions".into(),
            kind: "user".into(),
            max_concurrent_tasks: 1,
            mcp_config: Some(json!({"token": "do-not-leak"})),
            model: None,
            name: "agent".into(),
            owner_id: Some(Uuid::new_v4()),
            permission_mode: "private".into(),
            runtime_config: json!({"gateway": {"token": "do-not-leak"}}),
            runtime_id: Some(Uuid::new_v4()),
            runtime_mode: "local".into(),
            service_tier: None,
            session_mode: None,
            status: "active".into(),
            system_key: None,
            thinking_level: None,
            updated_at: now,
            visibility: "workspace".into(),
            workspace_id: Uuid::new_v4(),
        }
    }

    #[test]
    fn update_decoder_matches_go_first_value_and_unknown_field_behavior() {
        let request = decode_update(
            br#"{"custom_name":"  Prod Box  ","timezone":"ignored"} {"custom_name":"later"}"#,
        )
        .unwrap();
        assert_eq!(request.custom_name.as_deref(), Some("  Prod Box  "));
    }

    #[test]
    fn update_decoder_accepts_go_null_zero_values() {
        let top_level = decode_update(b"null").unwrap();
        assert!(top_level.visibility.is_none());
        assert!(top_level.custom_name.is_none());
        assert!(!top_level.apply_to_machine);

        let null_bool = decode_update(br#"{"apply_to_machine":null}"#).unwrap();
        assert!(!null_bool.apply_to_machine);
    }

    #[test]
    fn custom_name_limit_counts_unicode_codepoints() {
        assert_eq!("机".repeat(MAX_CUSTOM_NAME_CHARS).chars().count(), 100);
        assert_eq!("机".repeat(MAX_CUSTOM_NAME_CHARS + 1).chars().count(), 101);
    }

    #[tokio::test]
    async fn runtime_delete_conflict_never_serializes_agent_secrets() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let state = HandlerState::new(pool, patchbay_auth::pat_cache::PatCache::disabled(), None);
        let response = safe_agent_response(&state, &test_agent());
        assert!(response.get("custom_env").is_none());
        assert_eq!(response["custom_env_key_count"], 1);
        assert_eq!(response["mcp_config"], Value::Null);
        assert_eq!(response["runtime_config"]["gateway"]["token"], "***");
        assert_eq!(response["composio_toolkit_allowlist"], Value::Null);
        assert!(!response.to_string().contains("do-not-leak"));
    }

    #[tokio::test]
    async fn foreign_task_cannot_use_public_local_runtime_even_with_grants() {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required for runtime authorization contract");
        let pool = sqlx::PgPool::connect(&url)
            .await
            .expect("connect contract PostgreSQL");
        let workspace_id = Uuid::now_v7();
        let originator = Uuid::now_v7();
        let runtime_owner = Uuid::now_v7();
        let agent_id = Uuid::now_v7();
        let issue_id = Uuid::now_v7();
        let task_id = Uuid::now_v7();
        let runtime_id = Uuid::now_v7();
        let dispatched_at = chrono::Utc::now();

        sqlx::query("INSERT INTO workspace (id, name, slug) VALUES ($1, 'runtime auth', $2)")
            .bind(workspace_id)
            .bind(format!("runtime-auth-{workspace_id}"))
            .execute(&pool)
            .await
            .expect("create workspace");
        for (id, label) in [(originator, "originator"), (runtime_owner, "runtime-owner")] {
            sqlx::query("INSERT INTO \"user\" (id, name, email) VALUES ($1, $2, $3)")
                .bind(id)
                .bind(label)
                .bind(format!("{label}-{id}@example.test"))
                .execute(&pool)
                .await
                .expect("create user");
        }
        sqlx::query("INSERT INTO member (workspace_id, user_id, role) VALUES ($1, $2, 'member')")
            .bind(workspace_id)
            .bind(originator)
            .execute(&pool)
            .await
            .expect("create membership");
        sqlx::query(
            "INSERT INTO agent_runtime (id, workspace_id, daemon_id, name, runtime_mode, provider, status, owner_id, visibility) \
             VALUES ($1, $2, 'runtime-auth-daemon', 'public local runtime', 'local', 'codex', 'online', $3, 'public')",
        )
        .bind(runtime_id)
        .bind(workspace_id)
        .bind(runtime_owner)
        .execute(&pool)
        .await
        .expect("create runtime");
        sqlx::query(
            "INSERT INTO agent (id, workspace_id, name, runtime_mode, status, max_concurrent_tasks, owner_id, runtime_id) \
             VALUES ($1, $2, 'runtime auth agent', 'local', 'idle', 1, $3, $4)",
        )
        .bind(agent_id)
        .bind(workspace_id)
        .bind(originator)
        .bind(runtime_id)
        .execute(&pool)
        .await
        .expect("create agent");
        sqlx::query(
            "INSERT INTO issue (id, workspace_id, title, status, priority, creator_type, creator_id, executor_type, executor_id, number, position) \
             VALUES ($1, $2, 'runtime auth issue', 'in_progress', 'medium', 'member', $3, 'agent', $4, 1, 0)",
        )
        .bind(issue_id)
        .bind(workspace_id)
        .bind(originator)
        .bind(agent_id)
        .execute(&pool)
        .await
        .expect("create issue");
        sqlx::query(
            "INSERT INTO agent_task_queue (id, agent_id, issue_id, status, priority, dispatched_at, originator_user_id, accountable_user_id, runtime_id) \
             VALUES ($1, $2, $3, 'dispatched', 0, $4, $5, $5, $6)",
        )
        .bind(task_id)
        .bind(agent_id)
        .bind(issue_id)
        .bind(dispatched_at)
        .bind(originator)
        .bind(runtime_id)
        .execute(&pool)
        .await
        .expect("create task");
        let lease = patchbay_db::queries::task_token::create_task_token(
            &pool,
            &format!("runtime-auth-lease-{task_id}"),
            task_id,
            agent_id,
            workspace_id,
            originator,
            Some(chrono::Utc::now() + chrono::Duration::minutes(10)),
            &json!([{
                "action": patchbay_authorization::Action::RUNTIME_USE,
                "resource_type": patchbay_authorization::ResourceType::RUNTIME,
                "resource_id": runtime_id,
            }]),
            None,
            Some(dispatched_at),
            1,
            Some(originator),
            Some(runtime_id),
            Uuid::now_v7(),
        )
        .await
        .expect("create lease")
        .expect("lease inserted");
        let member = Member {
            created_at: chrono::Utc::now(),
            id: Uuid::now_v7(),
            role: "member".into(),
            user_id: originator,
            workspace_id,
        };
        let runtime = AgentRuntime {
            created_at: chrono::Utc::now(),
            custom_name: None,
            daemon_id: Some("runtime-auth-daemon".into()),
            device_info: "{}".into(),
            id: runtime_id,
            last_seen_at: None,
            legacy_daemon_id: None,
            metadata: json!({}),
            name: "public local runtime".into(),
            owner_id: Some(runtime_owner),
            profile_id: None,
            provider: "codex".into(),
            runtime_mode: "local".into(),
            status: "online".into(),
            updated_at: chrono::Utc::now(),
            visibility: "public".into(),
            workspace_id,
        };
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("x-actor-source", "task_token".to_string()),
            ("x-task-id", task_id.to_string()),
            ("x-capability-lease-id", lease.id.to_string()),
            ("x-on-behalf-of-user-id", originator.to_string()),
            ("x-agent-id", agent_id.to_string()),
            ("x-device-id", runtime_id.to_string()),
        ] {
            headers.insert(name, value.parse().expect("header value"));
        }
        let state = HandlerState::new(
            pool.clone(),
            patchbay_auth::pat_cache::PatCache::disabled(),
            None,
        );
        assert!(
            !runtime_allowed(&state, &member, &headers, &runtime, Action::RUNTIME_USE)
                .await
                .expect("deny runtime use")
        );

        sqlx::query(
            "INSERT INTO authorization_grant (workspace_id, principal_type, principal_id, action, resource_type, resource_id, effect, created_by) VALUES \
             ($1, 'agent_definition', $2, $4, $5, $3, 'allow', $6), \
             ($1, 'device_runtime', $3, $4, $5, $3, 'allow', $6), \
             ($1, 'task_run', $7, $4, $5, $3, 'allow', $6)",
        )
        .bind(workspace_id)
        .bind(agent_id)
        .bind(runtime_id)
        .bind(Action::RUNTIME_USE)
        .bind(ResourceType::RUNTIME)
        .bind(runtime_owner)
        .bind(task_id)
        .execute(&pool)
        .await
        .expect("grant agent and device runtime use");
        assert!(
            !runtime_allowed(&state, &member, &headers, &runtime, Action::RUNTIME_USE)
                .await
                .expect("agent and device grants are ceilings, not caller authority")
        );

        sqlx::query(
            "INSERT INTO authorization_grant (workspace_id, principal_type, principal_id, action, resource_type, resource_id, effect, created_by) \
             VALUES ($1, 'user', $2, $3, $4, $5, 'allow', $2)",
        )
        .bind(workspace_id)
        .bind(originator)
        .bind(Action::RUNTIME_USE)
        .bind(ResourceType::RUNTIME)
        .bind(runtime_id)
        .execute(&pool)
        .await
        .expect("grant runtime use");
        assert!(
            !runtime_allowed(&state, &member, &headers, &runtime, Action::RUNTIME_USE)
                .await
                .expect("a grant cannot expose another owner's local device")
        );

        sqlx::query(
            "INSERT INTO authorization_grant (workspace_id, principal_type, principal_id, action, resource_type, resource_id, effect, created_by) \
             VALUES ($1, 'user', $2, $3, $4, $5, 'require_approval', $2)",
        )
        .bind(workspace_id)
        .bind(originator)
        .bind(Action::RUNTIME_USE)
        .bind(ResourceType::RUNTIME)
        .bind(runtime_id)
        .execute(&pool)
        .await
        .expect("require runtime approval");
        assert!(
            !runtime_allowed(&state, &member, &headers, &runtime, Action::RUNTIME_USE)
                .await
                .expect("require approval is not allow")
        );

        sqlx::query("DELETE FROM authorization_audit_event WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("delete audit");
        sqlx::query("DELETE FROM authorization_grant WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("delete grants");
        sqlx::query("DELETE FROM task_token WHERE task_id = $1")
            .bind(task_id)
            .execute(&pool)
            .await
            .expect("delete lease");
        sqlx::query("DELETE FROM agent_task_queue WHERE id = $1")
            .bind(task_id)
            .execute(&pool)
            .await
            .expect("delete task");
        sqlx::query("DELETE FROM issue WHERE id = $1")
            .bind(issue_id)
            .execute(&pool)
            .await
            .expect("delete issue");
        sqlx::query("DELETE FROM agent WHERE id = $1")
            .bind(agent_id)
            .execute(&pool)
            .await
            .expect("delete agent");
        sqlx::query("DELETE FROM agent_runtime WHERE id = $1")
            .bind(runtime_id)
            .execute(&pool)
            .await
            .expect("delete runtime");
        sqlx::query("DELETE FROM member WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("delete membership");
        sqlx::query("DELETE FROM workspace WHERE id = $1")
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("delete workspace");
        sqlx::query("DELETE FROM \"user\" WHERE id IN ($1, $2)")
            .bind(originator)
            .bind(runtime_owner)
            .execute(&pool)
            .await
            .expect("delete users");
    }
}
