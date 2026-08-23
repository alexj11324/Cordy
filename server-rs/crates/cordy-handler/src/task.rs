//! User-authenticated task endpoints.

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::queries::{agent, task_message};
use cordy_middleware::workspace::WorkspaceContext;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new().route("/api/tasks/{task_id}/messages", get(list_messages))
}

#[derive(Debug, Default, Deserialize)]
struct MessageQuery {
    since: Option<String>,
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
