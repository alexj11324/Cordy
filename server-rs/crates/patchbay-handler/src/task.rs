//! User-authenticated task endpoints.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use patchbay_authorization::{
    Action, AuthorizationContext, AuthorizationRequest, Principal, PrincipalType, Resource,
    ResourceType,
};
use patchbay_db::models::{Agent, AgentInvocationTarget};
use patchbay_db::queries::{agent, agent_invocation_target, chat, task_message};
use patchbay_middleware::workspace::WorkspaceContext;
use patchbay_service::task_service::{CancelTaskOptions, TaskServiceError};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/tasks/{task_id}/messages", get(list_messages))
        .route(
            "/api/tasks/{task_id}/message-bus",
            post(send_message_to_main_task),
        )
        .route("/api/tasks/{task_id}/cancel", post(cancel_task))
}

#[derive(Debug, Default, Deserialize)]
struct MessageQuery {
    since: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CancelQuery {
    #[serde(default)]
    expected_status: String,
    #[serde(default)]
    chat_session_id: String,
    #[serde(default)]
    queue_action: String,
}

#[derive(Debug, Default, Deserialize)]
struct TaskMessageBusRequest {
    #[serde(default)]
    content: String,
}

/// A Side Chat may address only the exact main task recorded in its immutable
/// task context. The task-token identity is authoritative; member/PAT callers
/// cannot impersonate an Agent or manually relay this action.
async fn send_message_to_main_task(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(parent_task_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<TaskMessageBusRequest>,
) -> Response {
    if headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        != Some("task_token")
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "only a Side Chat Agent can use the task Message Bus",
        );
    }
    let Some(source_task_id) = headers
        .get("x-task-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
    else {
        return error_response(StatusCode::FORBIDDEN, "invalid Side Chat task identity");
    };
    if !task_lease_allows(
        &state,
        &headers,
        context.member.workspace_id,
        source_task_id,
        Action::TASK_UPDATE,
    )
    .await
    {
        return error_response(StatusCode::FORBIDDEN, "task capability does not allow update");
    }
    let Some(actor_agent_id) = headers
        .get("x-agent-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
    else {
        return error_response(StatusCode::FORBIDDEN, "invalid Side Chat Agent identity");
    };
    let Ok(parent_task_id) = Uuid::parse_str(parent_task_id.trim()) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid main task id");
    };

    let parent = match agent::get_agent_task_in_workspace(
        &state.pool,
        parent_task_id,
        context.member.workspace_id,
    )
    .await
    {
        Ok(Some(parent)) if parent.agent_id == actor_agent_id => parent,
        Ok(Some(_)) => {
            return error_response(
                StatusCode::FORBIDDEN,
                "the main task belongs to a different Agent",
            )
        }
        Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "main task not found"),
    };

    match state
        .tasks
        .send_side_chat_message_to_main(source_task_id, parent.id, &request.content)
        .await
    {
        Ok(receipt) => Json(json!({
            "status": if receipt.coalesced { "coalesced" } else { "deferred" },
            "continuation_task_id": receipt.continuation_task_id,
            "main_task_id": parent.id,
            "agent_id": parent.agent_id,
        }))
        .into_response(),
        Err(TaskServiceError::Sql(error)) => {
            tracing::warn!(%error, %source_task_id, %parent_task_id, "task Message Bus write failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to send the Side Chat instruction",
            )
        }
        Err(error) => error_response(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

async fn list_messages(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(task_id): Path<String>,
    Query(query): Query<MessageQuery>,
    headers: HeaderMap,
) -> Response {
    let task_id = match Uuid::parse_str(task_id.trim()) {
        Ok(task_id) => task_id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid task_id"),
    };
    if !task_lease_allows(
        &state,
        &headers,
        context.member.workspace_id,
        task_id,
        Action::TASK_READ,
    )
    .await
    {
        return error_response(StatusCode::FORBIDDEN, "task capability does not allow read");
    }
    let task = match agent::get_agent_task(&state.pool, task_id).await {
        Ok(Some(task)) => task,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "task not found"),
        Err(error) => {
            tracing::warn!(%error, %task_id, "failed to load task messages owner");
            return error_response(StatusCode::NOT_FOUND, "task not found");
        }
    };
    let task_workspace = state.tasks.resolve_task_workspace_id(&task).await;
    if task_workspace.as_deref() != Some(context.workspace_id.as_str()) {
        return error_response(StatusCode::NOT_FOUND, "task not found");
    }

    let messages = match query.since {
        Some(since) => {
            let seq = match since.parse::<i32>() {
                Ok(seq) => seq,
                Err(_) => {
                    return error_response(StatusCode::BAD_REQUEST, "invalid since parameter")
                }
            };
            task_message::list_task_messages_since(&state.pool, task_id, seq).await
        }
        None => task_message::list_task_messages(&state.pool, task_id).await,
    };
    match messages {
        Ok(messages) => Json(
            messages
                .iter()
                .map(|message| crate::daemon::task_message_payload(message, task.issue_id))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, %task_id, "failed to list task messages");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list task messages",
            )
        }
    }
}

async fn cancel_task(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(task_id): Path<String>,
    Query(query): Query<CancelQuery>,
    headers: HeaderMap,
) -> Response {
    let task_id = match Uuid::parse_str(task_id.trim()) {
        Ok(task_id) => task_id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid task id"),
    };
    if !task_lease_allows(
        &state,
        &headers,
        context.member.workspace_id,
        task_id,
        Action::TASK_UPDATE,
    )
    .await
    {
        return error_response(StatusCode::FORBIDDEN, "task capability does not allow update");
    }
    let task =
        match agent::get_agent_task_in_workspace(&state.pool, task_id, context.member.workspace_id)
            .await
        {
            Ok(Some(task)) => task,
            Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "task not found"),
        };
    let options = match cancel_options(
        task.chat_session_id,
        &query,
        crate::claim_response::request_has_client_capability(
            &headers,
            patchbay_protocol::APP_CAPABILITY_CHAT_DRAFT_RESTORE_V1,
        ),
    ) {
        Ok(options) => options,
        Err((status, message)) => return error_response(status, message),
    };

    if let Some(chat_session_id) = task.chat_session_id {
        let session = match chat::get_chat_session_in_workspace(
            &state.pool,
            chat_session_id,
            context.member.workspace_id,
        )
        .await
        {
            Ok(Some(session)) => session,
            Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "task not found"),
        };
        if session.creator_id != context.member.user_id {
            return error_response(StatusCode::FORBIDDEN, "not your task");
        }
    } else {
        let target = match agent::get_agent_in_workspace(
            &state.pool,
            task.agent_id,
            context.member.workspace_id,
        )
        .await
        {
            Ok(Some(agent)) => agent,
            Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "task not found"),
        };
        let (actor_type, actor_id, _) =
            crate::issue::mutation_actor(&state, &context, &headers).await;
        if !can_access_agent(&state, &context, &target, &actor_type, actor_id).await {
            return error_response(
                StatusCode::FORBIDDEN,
                "you do not have access to this agent",
            );
        }
    }

    let cancelled = match state.tasks.cancel_task_with_result(task_id, options).await {
        Ok(cancelled) => cancelled,
        Err(TaskServiceError::NoLongerQueued(_)) => {
            return error_response(StatusCode::CONFLICT, "task is no longer queued")
        }
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let mut response = crate::issue::task_maps(
        &state,
        std::slice::from_ref(&cancelled.task),
        &context.workspace_id,
    )
    .await
    .pop()
    .unwrap_or_else(|| json!({}));
    if let Some(message) = cancelled.cancelled_chat_message {
        let attachments = message
            .attachments
            .iter()
            .map(crate::issue::AttachmentResponse::from)
            .collect::<Vec<_>>();
        let mut value = serde_json::Map::new();
        value.insert("chat_session_id".into(), json!(message.chat_session_id));
        value.insert("message_id".into(), json!(message.message_id));
        value.insert("content".into(), json!(message.content));
        value.insert("restore_to_input".into(), json!(message.restore_to_input));
        if !attachments.is_empty() {
            value.insert(
                "attachments".into(),
                serde_json::to_value(attachments).unwrap_or_else(|_| Value::Array(Vec::new())),
            );
        }
        response["cancelled_chat_message"] = Value::Object(value);
    }
    Json(response).into_response()
}

async fn task_lease_allows(
    state: &HandlerState,
    headers: &HeaderMap,
    workspace_id: Uuid,
    task_id: Uuid,
    action: &'static str,
) -> bool {
    if headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        != Some("task_token")
    {
        return true;
    }
    let header_uuid = |name: &'static str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| Uuid::parse_str(value).ok())
    };
    let Some(current_task_id) = header_uuid("x-task-id") else {
        return false;
    };
    if !task_request_matches(headers, task_id) {
        return false;
    }
    let Some(lease_id) = header_uuid("x-capability-lease-id") else {
        return false;
    };
    state
        .authorization
        .authorize(AuthorizationRequest {
            principal: Principal {
                principal_type: PrincipalType::TaskRun,
                id: Some(current_task_id),
            },
            action: Action::new(action),
            resource: Resource {
                resource_type: ResourceType::new(ResourceType::TASK_RUN),
                id: Some(task_id),
                workspace_id,
                owner_id: header_uuid("x-on-behalf-of-user-id"),
                attributes: json!({"private": true}),
            },
            context: AuthorizationContext {
                on_behalf_of_user_id: header_uuid("x-on-behalf-of-user-id"),
                via_agent_id: header_uuid("x-agent-id"),
                device_id: header_uuid("x-device-id"),
                task_id: Some(current_task_id),
                lease_id: Some(lease_id),
                ..Default::default()
            },
            delegation_chain: Vec::new(),
        })
        .await
        .is_ok_and(|decision| decision.is_allowed())
}

fn task_request_matches(headers: &HeaderMap, requested_task_id: Uuid) -> bool {
    headers
        .get("x-task-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        == Some(requested_task_id)
}

fn cancel_options(
    task_chat_session_id: Option<Uuid>,
    query: &CancelQuery,
    client_supports_draft_restore: bool,
) -> Result<CancelTaskOptions, (StatusCode, &'static str)> {
    let mut options = CancelTaskOptions {
        client_supports_draft_restore,
        user_initiated: true,
        ..CancelTaskOptions::default()
    };
    if query.expected_status.is_empty() {
        return Ok(options);
    }
    if query.expected_status != "queued" {
        return Err((StatusCode::BAD_REQUEST, "expected_status must be queued"));
    }
    let expected_session = Uuid::parse_str(&query.chat_session_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "invalid chat_session_id"))?;
    if task_chat_session_id != Some(expected_session) {
        return Err((
            StatusCode::CONFLICT,
            "task does not belong to the expected chat session",
        ));
    }
    if query.queue_action != "edit" && query.queue_action != "remove" {
        return Err((
            StatusCode::BAD_REQUEST,
            "queue_action must be edit or remove",
        ));
    }
    options.queued_only = true;
    options.expected_chat_session = expected_session;
    options.queue_action.clone_from(&query.queue_action);
    Ok(options)
}

pub(crate) async fn can_access_agent(
    state: &HandlerState,
    context: &WorkspaceContext,
    target: &Agent,
    actor_type: &str,
    actor_id: Uuid,
) -> bool {
    if actor_type == "agent" {
        return true;
    }
    if target.owner_id == Some(actor_id)
        || matches!(context.member.role.as_str(), "owner" | "admin")
    {
        return true;
    }
    if target.permission_mode != "public_to" {
        return false;
    }
    let targets = match agent_invocation_target::list_agent_invocation_targets(
        &state.pool,
        target.id,
    )
    .await
    {
        Ok(targets) => targets,
        Err(_) => return false,
    };
    member_hits_invocation_targets(&targets, actor_id)
}

/// Returns whether a human member may invoke an agent. This is intentionally
/// narrower than [`can_access_agent`]: workspace admins may inspect and wire
/// private agents, but they must not run them with the owner's credentials.
/// It mirrors Go's `canInvokeAgent` member branch.
pub(crate) async fn can_member_invoke_agent(
    state: &HandlerState,
    target: &Agent,
    actor_id: Uuid,
) -> bool {
    if target.owner_id == Some(actor_id) {
        return true;
    }
    if target.permission_mode != "public_to" {
        return false;
    }
    let targets = match agent_invocation_target::list_agent_invocation_targets(
        &state.pool,
        target.id,
    )
    .await
    {
        Ok(targets) => targets,
        Err(_) => return false,
    };
    member_invocation_allowed(target.owner_id, &target.permission_mode, &targets, actor_id)
}

fn member_invocation_allowed(
    owner_id: Option<Uuid>,
    permission_mode: &str,
    targets: &[AgentInvocationTarget],
    actor_id: Uuid,
) -> bool {
    owner_id == Some(actor_id)
        || (permission_mode == "public_to" && member_hits_invocation_targets(targets, actor_id))
}

fn member_hits_invocation_targets(targets: &[AgentInvocationTarget], actor_id: Uuid) -> bool {
    targets.iter().any(|target| {
        target.target_type == "workspace"
            || (target.target_type == "member" && target.target_id == actor_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued_query(session_id: Uuid) -> CancelQuery {
        CancelQuery {
            expected_status: "queued".into(),
            chat_session_id: session_id.to_string(),
            queue_action: "edit".into(),
        }
    }

    fn uuid_at(last: char) -> Uuid {
        Uuid::parse_str(&format!("018f03a0-c4d2-7a37-ae4d-5aa45de12f1{last}")).unwrap()
    }

    #[test]
    fn ordinary_cancel_ignores_queue_fields_and_marks_user_initiated() {
        let options = cancel_options(
            None,
            &CancelQuery {
                expected_status: String::new(),
                chat_session_id: "not-a-uuid".into(),
                queue_action: "invalid".into(),
            },
            true,
        )
        .unwrap();
        assert!(!options.queued_only);
        assert!(options.user_initiated);
        assert!(options.client_supports_draft_restore);
    }

    #[test]
    fn queued_cancel_is_a_session_scoped_compare_and_set() {
        let session_id = uuid_at('1');
        let options = cancel_options(Some(session_id), &queued_query(session_id), false).unwrap();
        assert!(options.queued_only);
        assert_eq!(options.expected_chat_session, session_id);
        assert_eq!(options.queue_action, "edit");

        let other_session = uuid_at('2');
        assert_eq!(
            cancel_options(Some(other_session), &queued_query(session_id), false)
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
    }

    #[test]
    fn queued_cancel_rejects_invalid_status_session_and_action() {
        let session_id = uuid_at('3');
        let mut query = queued_query(session_id);
        query.expected_status = "running".into();
        assert_eq!(
            cancel_options(Some(session_id), &query, false)
                .unwrap_err()
                .1,
            "expected_status must be queued"
        );

        query = queued_query(session_id);
        query.chat_session_id = "bad".into();
        assert_eq!(
            cancel_options(Some(session_id), &query, false)
                .unwrap_err()
                .1,
            "invalid chat_session_id"
        );

        query = queued_query(session_id);
        query.queue_action = "replace".into();
        assert_eq!(
            cancel_options(Some(session_id), &query, false)
                .unwrap_err()
                .1,
            "queue_action must be edit or remove"
        );
    }

    #[test]
    fn regular_member_access_matches_public_to_targets() {
        let member_id = uuid_at('4');
        let other_id = uuid_at('5');
        let target = |target_type: &str, target_id: Uuid| AgentInvocationTarget {
            id: uuid_at('6'),
            agent_id: uuid_at('7'),
            target_type: target_type.into(),
            target_id,
            created_by: None,
            created_at: chrono::Utc::now(),
        };
        assert!(member_hits_invocation_targets(
            &[target("workspace", other_id)],
            member_id
        ));
        assert!(member_hits_invocation_targets(
            &[target("member", member_id)],
            member_id
        ));
        assert!(!member_hits_invocation_targets(
            &[target("member", other_id), target("team", member_id)],
            member_id
        ));
    }

    #[test]
    fn private_agent_never_gets_an_admin_invoke_bypass() {
        let owner = uuid_at('8');
        let admin = uuid_at('9');
        assert!(!member_invocation_allowed(
            Some(owner),
            "private",
            &[],
            admin
        ));
        assert!(member_invocation_allowed(
            Some(owner),
            "private",
            &[],
            owner
        ));
    }

    #[test]
    fn task_lease_cannot_read_or_cancel_another_task() {
        let own_task = uuid_at('a');
        let other_task = uuid_at('b');
        let mut headers = HeaderMap::new();
        headers.insert("x-task-id", own_task.to_string().parse().unwrap());
        assert!(task_request_matches(&headers, own_task));
        assert!(!task_request_matches(&headers, other_task));
    }
}
