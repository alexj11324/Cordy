//! User-authenticated task endpoints.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use patchbay_db::models::{Agent, AgentInvocationTarget};
use patchbay_db::queries::{
    agent, agent_invocation_target, automation as automation_query, chat, issue as issue_query,
    task_message,
};
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
            "/api/tasks/{task_id}/agent-thread",
            get(get_agent_thread),
        )
        .route(
            "/api/tasks/{task_id}/agent-thread/continue",
            post(continue_agent_thread),
        )
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

#[derive(Debug, Default, Deserialize)]
struct ContinueAgentThreadRequest {
    #[serde(default)]
    content: String,
    #[serde(default)]
    idempotency_key: String,
}

struct AgentThreadAccess {
    task: patchbay_db::models::AgentTaskQueue,
    agent: Agent,
}

fn unavailable_reason(
    reason: patchbay_service::task_service::AgentThreadUnavailableReason,
) -> (&'static str, &'static str) {
    use patchbay_service::task_service::AgentThreadUnavailableReason as Reason;
    match reason {
        Reason::RetiredSession => (
            "provider_session_retired",
            "The provider session was deleted or retired and cannot be restored.",
        ),
        Reason::SessionRolloutMissing => (
            "provider_session_missing",
            "The provider session data is missing, so this Agent thread cannot continue safely.",
        ),
        Reason::FreshSessionRequired => (
            "fresh_session_required",
            "This run was explicitly started with a fresh provider session and cannot continue the previous thread.",
        ),
        Reason::SessionNotEstablished => (
            "provider_session_not_established",
            "The provider has not established a session for this run yet.",
        ),
    }
}

async fn load_agent_thread_access(
    state: &HandlerState,
    context: &WorkspaceContext,
    task_id: Uuid,
) -> Result<AgentThreadAccess, Response> {
    let task = match agent::get_agent_task_in_workspace(
        &state.pool,
        task_id,
        context.member.workspace_id,
    )
    .await
    {
        Ok(Some(task)) => task,
        Ok(None) | Err(_) => return Err(error_response(StatusCode::NOT_FOUND, "task not found")),
    };
    let target = match agent::get_agent_in_workspace(
        &state.pool,
        task.agent_id,
        context.member.workspace_id,
    )
    .await
    {
        Ok(Some(agent)) => agent,
        Ok(None) | Err(_) => return Err(error_response(StatusCode::NOT_FOUND, "task not found")),
    };

    if let Some(issue_id) = task.issue_id {
        match issue_query::get_issue(&state.pool, issue_id).await {
            Ok(Some(issue)) if issue.workspace_id == context.member.workspace_id => {}
            Ok(Some(_)) | Ok(None) | Err(_) => {
                return Err(error_response(StatusCode::NOT_FOUND, "task not found"))
            }
        }
    }
    if let Some(chat_session_id) = task.chat_session_id {
        let session = match chat::get_chat_session_in_workspace(
            &state.pool,
            chat_session_id,
            context.member.workspace_id,
        )
        .await
        {
            Ok(Some(session)) => session,
            Ok(None) | Err(_) => {
                return Err(error_response(StatusCode::NOT_FOUND, "task not found"))
            }
        };
        if session.creator_id != context.member.user_id {
            return Err(error_response(
                StatusCode::FORBIDDEN,
                "you do not have access to this Agent thread",
            ));
        }
    }
    if let Some(run_id) = task.automation_run_id {
        let run = match automation_query::get_automation_run(&state.pool, run_id).await {
            Ok(Some(run)) => run,
            Ok(None) | Err(_) => {
                return Err(error_response(StatusCode::NOT_FOUND, "task not found"))
            }
        };
        let automation = match automation_query::get_automation(&state.pool, run.automation_id).await
        {
            Ok(Some(automation)) if automation.workspace_id == context.member.workspace_id => {
                automation
            }
            Ok(Some(_)) | Ok(None) | Err(_) => {
                return Err(error_response(StatusCode::NOT_FOUND, "task not found"))
            }
        };
        let owns_automation = matches!(context.member.role.as_str(), "owner" | "admin")
            || (automation.created_by_type == "member"
                && automation.created_by_id == context.member.user_id);
        let collaborates = matches!(
            automation_query::is_automation_collaborator(
                &state.pool,
                automation.id,
                context.member.user_id,
            )
            .await,
            Ok(Some(true))
        );
        if !owns_automation && !collaborates {
            return Err(error_response(
                StatusCode::FORBIDDEN,
                "you do not have access to this Automation run",
            ));
        }
    }

    if !can_member_invoke_agent(state, &target, context.member.user_id).await {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "you do not have permission to continue this Agent thread",
        ));
    }
    Ok(AgentThreadAccess {
        task,
        agent: target,
    })
}

async fn get_agent_thread(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_task_id): Path<String>,
) -> Response {
    let task_id = match Uuid::parse_str(raw_task_id.trim()) {
        Ok(task_id) => task_id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid task id"),
    };
    let access = match load_agent_thread_access(&state, &context, task_id).await {
        Ok(access) => access,
        Err(response) => return response,
    };
    let thread_tasks = match agent::list_agent_thread_tasks(&state.pool, task_id).await {
        Ok(tasks) if !tasks.is_empty() => tasks,
        Ok(_) => vec![access.task.clone()],
        Err(error) => {
            tracing::warn!(%error, %task_id, "failed to load Agent thread tasks");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load Agent thread tasks",
            );
        }
    };
    let current_task = thread_tasks
        .last()
        .cloned()
        .unwrap_or_else(|| access.task.clone());
    let mut events = Vec::new();
    for thread_task in &thread_tasks {
        match task_message::list_task_messages(&state.pool, thread_task.id).await {
            Ok(task_events) => events.extend(task_events.iter().map(|event| {
                crate::daemon::task_message_payload(event, thread_task.issue_id)
            })),
            Err(error) => {
                tracing::warn!(%error, task_id = %thread_task.id, "failed to load Agent thread events");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load Agent thread events",
                );
            }
        }
    }
    let (availability, can_continue) =
        match patchbay_service::task_service::agent_thread_availability(&current_task) {
            Ok(()) => (json!({ "state": "available" }), true),
            Err(reason) => {
                let (code, message) = unavailable_reason(reason);
                (
                    json!({ "state": "unavailable", "reason_code": code, "reason": message }),
                    false,
                )
            }
        };
    Json(json!({
        "task": crate::task_json::task_to_map(&current_task, &context.workspace_id),
        "thread_tasks": thread_tasks
            .iter()
            .map(|task| crate::task_json::task_to_map(task, &context.workspace_id))
            .collect::<Vec<_>>(),
        "current_task_id": current_task.id.to_string(),
        "agent": {
            "id": access.agent.id.to_string(),
            "name": access.agent.name,
            "avatar_url": access.agent.avatar_url,
        },
        "events": events,
        "availability": availability,
        "can_continue": can_continue,
    }))
    .into_response()
}

async fn continue_agent_thread(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_task_id): Path<String>,
    Json(request): Json<ContinueAgentThreadRequest>,
) -> Response {
    let task_id = match Uuid::parse_str(raw_task_id.trim()) {
        Ok(task_id) => task_id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid task id"),
    };
    let access = match load_agent_thread_access(&state, &context, task_id).await {
        Ok(access) => access,
        Err(response) => return response,
    };
    let parent_task = match agent::list_agent_thread_tasks(&state.pool, task_id).await {
        Ok(tasks) => tasks.last().cloned().unwrap_or_else(|| access.task.clone()),
        Err(error) => {
            tracing::warn!(%error, %task_id, "failed to resolve current Agent thread task");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to resolve current Agent thread task",
            );
        }
    };
    if let Err(reason) = patchbay_service::task_service::agent_thread_availability(&parent_task) {
        let (reason_code, message) = unavailable_reason(reason);
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "agent_thread_unavailable",
                "reason_code": reason_code,
                "reason": message,
            })),
        )
            .into_response();
    }
    match state
        .tasks
        .continue_agent_thread(parent_task.id, &request.content, &request.idempotency_key)
        .await
    {
        Ok(receipt) => Json(json!({
            "continuation_task_id": receipt.continuation_task_id,
            "status": if receipt.coalesced { "coalesced" } else { "queued" },
        }))
        .into_response(),
        Err(TaskServiceError::AgentThreadUnavailable(reason)) => {
            let (reason_code, message) = unavailable_reason(reason);
            (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "agent_thread_unavailable",
                    "reason_code": reason_code,
                    "reason": message,
                })),
            )
                .into_response()
        }
        Err(TaskServiceError::AgentThreadIdempotencyConflict) => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "agent_thread_idempotency_conflict",
                "reason": "The idempotency key was already used for different content.",
            })),
        )
            .into_response(),
        Err(TaskServiceError::Sql(error)) => {
            tracing::warn!(%error, %task_id, "Agent thread continuation failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to queue Agent thread continuation",
            )
        }
        Err(error) => error_response(StatusCode::BAD_REQUEST, &error.to_string()),
    }
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
) -> Response {
    let task_id = match Uuid::parse_str(task_id.trim()) {
        Ok(task_id) => task_id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid task_id"),
    };
    let access = match load_agent_thread_access(&state, &context, task_id).await {
        Ok(access) => access,
        Err(response) => return response,
    };
    let task = access.task;

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
}
