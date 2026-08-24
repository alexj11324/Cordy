//! Runtime collection and editable metadata handlers.

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use cordy_db::models::{AgentRuntime, Member};
use cordy_db::queries::{agent, runtime, runtime_profile};
use cordy_middleware::workspace::WorkspaceContext;
use cordy_protocol::EVENT_DAEMON_REGISTER;
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

fn safe_agent_response(state: &HandlerState, agent: &cordy_db::models::Agent) -> Value {
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
    agents: &[cordy_db::models::Agent],
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
    if !can_edit(&context.member, &found) {
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
    Path(raw_id): Path<String>,
) -> Response {
    let found = match load_deletable_runtime(&state, &context, &raw_id).await {
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
    let found = match load_deletable_runtime(&state, &context, &raw_id).await {
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
        Ok(runtimes) => Json(
            runtimes
                .into_iter()
                .map(RuntimeResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
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

fn can_edit(member: &Member, runtime: &AgentRuntime) -> bool {
    matches!(member.role.as_str(), "owner" | "admin") || runtime.owner_id == Some(member.user_id)
}

fn can_set_visibility(member: &Member, runtime: &AgentRuntime) -> bool {
    runtime.owner_id == Some(member.user_id)
}

async fn update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
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
    if !can_edit(&context.member, &found) {
        return error_response(StatusCode::FORBIDDEN, "you can only edit your own runtimes");
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
            let owner_filter = (!matches!(context.member.role.as_str(), "owner" | "admin"))
                .then_some(context.member.user_id);
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
        state.bus.publish(&cordy_events::Event {
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

    fn test_agent() -> cordy_db::models::Agent {
        let now = chrono::Utc::now();
        cordy_db::models::Agent {
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
        let state = HandlerState::new(pool, cordy_auth::pat_cache::PatCache::disabled(), None);
        let response = safe_agent_response(&state, &test_agent());
        assert!(response.get("custom_env").is_none());
        assert_eq!(response["custom_env_key_count"], 1);
        assert_eq!(response["mcp_config"], Value::Null);
        assert_eq!(response["runtime_config"]["gateway"]["token"], "***");
        assert_eq!(response["composio_toolkit_allowlist"], Value::Null);
        assert!(!response.to_string().contains("do-not-leak"));
    }
}
