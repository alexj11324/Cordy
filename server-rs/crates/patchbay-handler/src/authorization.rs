//! Actor-scoped authorization decision explanations.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use patchbay_middleware::workspace::WorkspaceContext;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new().route("/api/authorization/decisions/{decision_id}", get(explain))
}

async fn explain(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let decision_id = match Uuid::parse_str(&raw_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid decision id"),
    };
    let event = match state
        .authorization
        .explain(decision_id, context.member.workspace_id)
        .await
    {
        Ok(Some(event)) => event,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "decision not found"),
        Err(error) => {
            tracing::error!(%error, %decision_id, "failed to explain authorization decision");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to explain authorization decision",
            );
        }
    };
    let actor_id = context.member.user_id;
    let actor_can_read = event.principal_id == Some(actor_id)
        || event.on_behalf_of_user_id == Some(actor_id)
        || context.member.role == "owner";
    if !actor_can_read {
        // Do not reveal that another principal's decision exists.
        return error_response(StatusCode::NOT_FOUND, "decision not found");
    }
    Json(event).into_response()
}
