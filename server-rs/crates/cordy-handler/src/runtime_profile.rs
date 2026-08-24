//! Workspace custom runtime-profile handlers.
//!
//! This is the HTTP counterpart of `server/internal/handler/runtime_profile.go`.
//! Profile mutations are mounted behind the workspace admin guard; reads are
//! mounted behind the member guard.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cordy_db::models::{Agent, AgentTaskQueue, Autopilot, RuntimeProfile};
use cordy_db::queries::{
    agent, agent_invocation_target, autopilot, channel, chat, chat_pinned_agent, issue_label,
    runtime, runtime_profile,
};
use cordy_middleware::workspace::WorkspaceContext;
use cordy_protocol::{EVENT_AGENT_STATUS, EVENT_AUTOPILOT_UPDATED, EVENT_DAEMON_REGISTER};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const DEFAULT_VISIBILITY: &str = "workspace";
const SUPPORTED_PROTOCOL_FAMILIES: &[&str] = &[
    "claude",
    "codebuddy",
    "codex",
    "copilot",
    "opencode",
    "deveco",
    "openclaw",
    "hermes",
    "pi",
    "cursor",
    "kimi",
    "reasonix",
    "dsh",
    "kiro",
    "antigravity",
    "qoder",
    "qoderclicn",
    "traecli",
    "grok",
    "qwen",
    "qwenpaw",
    "mcode",
    "dim",
];

pub fn member_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/workspaces/{id}/runtime-profiles", get(list))
        .route(
            "/api/workspaces/{id}/runtime-profiles/{profile_id}",
            get(get_one),
        )
}

pub fn admin_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/workspaces/{id}/runtime-profiles", post(create))
        .route(
            "/api/workspaces/{id}/runtime-profiles/{profile_id}",
            axum::routing::put(update)
                .patch(update)
                .delete(delete_profile),
        )
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace id"))
}

fn profile_id(raw: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(raw).map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid profile id"))
}

fn unique_violation(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23505")
}

fn sqlx_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23505")
}

fn decode_first<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, Response> {
    let mut decoder = serde_json::Deserializer::from_slice(body);
    T::deserialize(&mut decoder)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid request body"))
}

fn validate_command_name(raw: &str) -> Result<String, Response> {
    let command = raw.trim();
    if command.is_empty() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "command_name is required",
        ));
    }
    if command
        .chars()
        .any(|character| matches!(character, ' ' | '\t' | '\r' | '\n'))
    {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "command_name must be a single executable token; put arguments in fixed_args",
        ));
    }
    if command.contains('\0') {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "command_name cannot contain NUL bytes",
        ));
    }
    Ok(command.to_string())
}

fn fixed_args_value(args: Vec<String>) -> Result<Value, Response> {
    if args.iter().any(|arg| arg.trim().is_empty()) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "fixed_args entries must be non-empty",
        ));
    }
    if args.iter().any(|arg| arg.contains('\0')) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "fixed_args entries cannot contain NUL bytes",
        ));
    }
    Ok(json!(args))
}

fn profile_response(profile: &RuntimeProfile) -> Value {
    crate::profile_json::profile_to_map(profile)
}

async fn notify_profile_changed(state: &HandlerState, workspace_id: Uuid, profile_id: Uuid) {
    state
        .daemon_notifier
        .notify_runtime_profiles_changed(&workspace_id.to_string(), &profile_id.to_string())
        .await;
}

fn publish_daemon_register(
    state: &HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    payload: Value,
) {
    state.bus.publish(&cordy_events::Event {
        event_type: EVENT_DAEMON_REGISTER.to_string(),
        workspace_id: workspace_id.to_string(),
        actor_type: "member".into(),
        actor_id: user_id.to_string(),
        payload,
        ..Default::default()
    });
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    match runtime_profile::list_runtime_profiles(&state.pool, workspace_id).await {
        Ok(profiles) => Json(json!({
            "runtime_profiles": profiles.iter().map(profile_response).collect::<Vec<_>>()
        }))
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, %workspace_id, "failed to list runtime profiles");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list runtime profiles",
            )
        }
    }
}

async fn get_one(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, raw_profile_id)): Path<(String, String)>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let profile_id = match profile_id(&raw_profile_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    match runtime_profile::get_runtime_profile_for_workspace(&state.pool, profile_id, workspace_id)
        .await
    {
        Ok(Some(profile)) => Json(profile_response(&profile)).into_response(),
        Ok(None) | Err(_) => error_response(StatusCode::NOT_FOUND, "runtime profile not found"),
    }
}

#[derive(Debug, Default, Deserialize)]
struct CreateRequest {
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    protocol_family: String,
    #[serde(default)]
    command_name: String,
    description: Option<String>,
    #[serde(default)]
    fixed_args: Vec<String>,
    enabled: Option<bool>,
}

async fn create(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    body: Bytes,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let request = match decode_first::<Option<CreateRequest>>(&body) {
        Ok(request) => request.unwrap_or_default(),
        Err(response) => return response,
    };
    let display_name = request.display_name.trim();
    if display_name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "display_name is required");
    }
    let protocol_family = request.protocol_family.trim();
    if !SUPPORTED_PROTOCOL_FAMILIES.contains(&protocol_family) {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!(
                "unsupported protocol_family: must be one of {}",
                SUPPORTED_PROTOCOL_FAMILIES.join(", ")
            ),
        );
    }
    let command_name = match validate_command_name(&request.command_name) {
        Ok(command) => command,
        Err(response) => return response,
    };
    let fixed_args = match fixed_args_value(request.fixed_args) {
        Ok(args) => args,
        Err(response) => return response,
    };
    match runtime_profile::create_runtime_profile(
        &state.pool,
        workspace_id,
        display_name,
        protocol_family,
        &command_name,
        request.description.as_deref(),
        &fixed_args,
        DEFAULT_VISIBILITY,
        context.member.user_id,
        request.enabled.unwrap_or(true),
    )
    .await
    {
        Ok(Some(profile)) => {
            notify_profile_changed(&state, workspace_id, profile.id).await;
            publish_daemon_register(
                &state,
                workspace_id,
                context.member.user_id,
                json!({ "runtime_profile_id": profile.id.to_string() }),
            );
            (StatusCode::CREATED, Json(profile_response(&profile))).into_response()
        }
        Err(error) if unique_violation(&error) => error_response(
            StatusCode::CONFLICT,
            "a runtime profile with this display_name already exists",
        ),
        Err(error) => {
            tracing::error!(%error, %workspace_id, "failed to create runtime profile");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create runtime profile",
            )
        }
        Ok(None) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create runtime profile",
        ),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateRequest {
    display_name: Option<String>,
    command_name: Option<String>,
    description: Option<String>,
    fixed_args: Option<Vec<String>>,
    enabled: Option<bool>,
}

async fn update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, raw_profile_id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let profile_id = match profile_id(&raw_profile_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let request = match decode_first::<UpdateRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let display_name = match request.display_name.as_deref() {
        Some(raw) if raw.trim().is_empty() => {
            return error_response(StatusCode::BAD_REQUEST, "display_name cannot be empty")
        }
        Some(raw) => Some(raw.trim()),
        None => None,
    };
    let command_name = match request.command_name.as_deref() {
        Some(raw) => match validate_command_name(raw) {
            Ok(command) => Some(command),
            Err(response) => return response,
        },
        None => None,
    };
    let fixed_args = match request.fixed_args {
        Some(args) => match fixed_args_value(args) {
            Ok(args) => Some(args),
            Err(response) => return response,
        },
        None => None,
    };

    // The generated query takes `&Value`, which cannot represent SQL NULL and
    // would overwrite an omitted fixed_args with JSON null. Keep the same SQL
    // contract here while binding an optional JSON value.
    let updated = sqlx::query_as::<_, RuntimeProfile>(
        r#"UPDATE runtime_profile
SET display_name = COALESCE($1, display_name),
    command_name = COALESCE($2, command_name),
    description  = COALESCE($3, description),
    fixed_args   = COALESCE($4, fixed_args),
    enabled      = COALESCE($5, enabled),
    updated_at   = now()
WHERE id = $6 AND workspace_id = $7
RETURNING command_name, created_at, created_by, description, display_name,
          enabled, fixed_args, id, protocol_family, updated_at, visibility,
          workspace_id"#,
    )
    .bind(display_name)
    .bind(command_name.as_deref())
    .bind(request.description.as_deref())
    .bind(fixed_args.as_ref())
    .bind(request.enabled)
    .bind(profile_id)
    .bind(workspace_id)
    .fetch_optional(&state.pool)
    .await;

    match updated {
        Ok(Some(profile)) => {
            notify_profile_changed(&state, workspace_id, profile.id).await;
            publish_daemon_register(
                &state,
                workspace_id,
                context.member.user_id,
                json!({ "runtime_profile_id": profile.id.to_string() }),
            );
            Json(profile_response(&profile)).into_response()
        }
        Ok(None) => error_response(StatusCode::NOT_FOUND, "runtime profile not found"),
        Err(error) if sqlx_unique_violation(&error) => error_response(
            StatusCode::CONFLICT,
            "a runtime profile with this display_name already exists",
        ),
        Err(error) => {
            tracing::error!(%error, %profile_id, "failed to update runtime profile");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update runtime profile",
            )
        }
    }
}

#[derive(Default)]
pub(crate) struct Teardown {
    pub(crate) unbound_agents: Vec<Agent>,
    pub(crate) cancelled_tasks: Vec<AgentTaskQueue>,
    pub(crate) paused_autopilots: Vec<Autopilot>,
}

pub(crate) async fn teardown_runtime(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<Teardown> {
    let unbound_agents =
        runtime::unbind_user_agents_from_runtime(&mut **transaction, runtime_id).await?;
    let agent_ids = unbound_agents
        .iter()
        .map(|agent| agent.id)
        .collect::<Vec<_>>();
    let paused_autopilots =
        autopilot::pause_autopilots_by_unbound_agents(&mut **transaction, agent_ids.clone())
            .await?;
    let cancelled_tasks = runtime::cancel_agent_tasks_by_runtime_or_agent(
        &mut **transaction,
        vec![runtime_id],
        agent_ids.clone(),
    )
    .await?;
    let undrained = runtime::count_undrained_tasks_by_runtime_or_agent(
        &mut **transaction,
        vec![runtime_id],
        agent_ids,
    )
    .await?
    .unwrap_or_default();
    if undrained > 0 {
        anyhow::bail!("runtime_delete_not_drained");
    }
    runtime::unbind_tasks_from_runtime(&mut **transaction, runtime_id).await?;
    agent_invocation_target::delete_agent_invocation_targets_by_system_runtime_agents(
        &mut **transaction,
        runtime_id,
    )
    .await?;
    channel::delete_channel_installations_by_system_runtime_agents(&mut **transaction, runtime_id)
        .await?;
    chat_pinned_agent::delete_chat_pinned_agents_by_system_runtime_agents(
        &mut **transaction,
        runtime_id,
    )
    .await?;
    issue_label::delete_agent_label_assignments_by_system_runtime_agents(
        &mut **transaction,
        runtime_id,
    )
    .await?;
    chat::delete_chat_draft_restores_by_system_runtime_agents(&mut **transaction, runtime_id)
        .await?;
    runtime::delete_system_agents_by_runtime(&mut **transaction, runtime_id).await?;
    Ok(Teardown {
        unbound_agents,
        cancelled_tasks,
        paused_autopilots,
    })
}

pub(crate) fn publish_teardown(
    state: &HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    teardown: &Teardown,
) {
    for agent in &teardown.unbound_agents {
        state.bus.publish(&cordy_events::Event {
            event_type: EVENT_AGENT_STATUS.to_string(),
            workspace_id: workspace_id.to_string(),
            actor_type: "member".into(),
            actor_id: user_id.to_string(),
            payload: json!({ "agent": crate::agent_api::agent_event_response(state, agent) }),
            ..Default::default()
        });
    }
    for autopilot in &teardown.paused_autopilots {
        state.bus.publish(&cordy_events::Event {
            event_type: EVENT_AUTOPILOT_UPDATED.to_string(),
            workspace_id: workspace_id.to_string(),
            actor_type: "member".into(),
            actor_id: user_id.to_string(),
            payload: json!({ "autopilot": autopilot }),
            ..Default::default()
        });
    }
    publish_daemon_register(state, workspace_id, user_id, json!({ "action": "delete" }));
}

async fn delete_profile(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, raw_profile_id)): Path<(String, String)>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let profile_id = match profile_id(&raw_profile_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to begin transaction",
            )
        }
    };
    let profile_missing = match runtime_profile::lock_runtime_profile_for_delete(
        &mut *transaction,
        profile_id,
        workspace_id,
    )
    .await
    {
        Ok(Some(_)) => false,
        Ok(None) => true,
        Err(error) => {
            tracing::error!(%error, %profile_id, "failed to load runtime profile");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load runtime profile",
            );
        }
    };
    let runtime_ids = match runtime_profile::list_agent_runtime_i_ds_by_profile(
        &mut *transaction,
        profile_id,
        workspace_id,
    )
    .await
    {
        Ok(ids) => ids.into_iter().flatten().collect::<Vec<_>>(),
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to enumerate profile runtimes",
            )
        }
    };
    if profile_missing && runtime_ids.is_empty() {
        return error_response(StatusCode::NOT_FOUND, "runtime profile not found");
    }
    for runtime_id in &runtime_ids {
        if agent::list_user_agents_by_runtime_for_update(&mut *transaction, *runtime_id)
            .await
            .is_err()
        {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to lock profile dependencies",
            );
        }
    }
    match runtime_profile::count_agents_by_profile(&mut *transaction, profile_id, workspace_id)
        .await
    {
        Ok(Some(count)) if count > 0 => {
            return error_response(
                StatusCode::CONFLICT,
                "cannot delete runtime profile: active agents are still bound to its runtimes",
            )
        }
        Ok(_) => {}
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to check profile usage",
            )
        }
    }
    let mut teardowns = Vec::with_capacity(runtime_ids.len());
    for runtime_id in &runtime_ids {
        match teardown_runtime(&mut transaction, *runtime_id).await {
            Ok(teardown) => teardowns.push(teardown),
            Err(error) if error.to_string().contains("runtime_delete_not_drained") => {
                return (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "error": "a runtime of this profile still has tasks in flight; retry in a moment.",
                        "code": "runtime_delete_not_drained"
                    })),
                )
                    .into_response();
            }
            Err(error) => {
                tracing::error!(%error, %runtime_id, %profile_id, "runtime profile teardown failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to unbind agents",
                );
            }
        }
    }
    if runtime_profile::delete_agent_runtimes_by_profile(
        &mut *transaction,
        profile_id,
        workspace_id,
    )
    .await
    .is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to clean up runtime instances",
        );
    }
    if !profile_missing
        && runtime_profile::delete_runtime_profile(&mut *transaction, profile_id, workspace_id)
            .await
            .is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to delete runtime profile",
        );
    }
    if transaction.commit().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to commit transaction",
        );
    }

    for teardown in &teardowns {
        state
            .tasks
            .broadcast_cancelled_tasks(&workspace_id.to_string(), &teardown.cancelled_tasks)
            .await;
        publish_teardown(&state, workspace_id, context.member.user_id, teardown);
    }
    notify_profile_changed(&state, workspace_id, profile_id).await;
    publish_daemon_register(
        &state,
        workspace_id,
        context.member.user_id,
        json!({ "deleted_runtime_profile_id": profile_id.to_string() }),
    );
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_name_validation_matches_go_boundary() {
        assert_eq!(
            validate_command_name("  codex-wrapper  ").unwrap(),
            "codex-wrapper"
        );
        assert!(validate_command_name("codex --flag").is_err());
        assert!(validate_command_name("codex\0wrapper").is_err());
        assert!(validate_command_name("   ").is_err());
    }

    #[test]
    fn fixed_args_reject_blank_and_nul_but_preserve_values() {
        assert_eq!(
            fixed_args_value(vec!["--mode".into(), "fast path".into()]).unwrap(),
            json!(["--mode", "fast path"])
        );
        assert!(fixed_args_value(vec![" \t".into()]).is_err());
        assert!(fixed_args_value(vec!["bad\0arg".into()]).is_err());
    }

    #[test]
    fn decoder_matches_go_first_json_value_behavior() {
        let request = decode_first::<UpdateRequest>(
            br#"{"display_name":"first"} {"display_name":"ignored"}"#,
        )
        .unwrap();
        assert_eq!(request.display_name.as_deref(), Some("first"));
    }

    #[test]
    fn protocol_whitelist_matches_current_go_source_of_truth() {
        assert!(SUPPORTED_PROTOCOL_FAMILIES.contains(&"codex"));
        assert!(SUPPORTED_PROTOCOL_FAMILIES.contains(&"dim"));
        assert!(!SUPPORTED_PROTOCOL_FAMILIES.contains(&"gemini"));
        assert_eq!(SUPPORTED_PROTOCOL_FAMILIES.len(), 23);
    }
}
