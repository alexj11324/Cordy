//! Authenticated CLI bearer-token handoff.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use cordy_db::queries::user;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new().route("/api/cli-token", post(issue))
}

async fn issue(State(state): State<HandlerState>, headers: HeaderMap) -> Response {
    let user_id = match headers
        .get("x-user-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
    {
        Some(user_id) => user_id,
        None => return error_response(StatusCode::UNAUTHORIZED, "user not authenticated"),
    };
    let user = match user::get_user(&state.pool, user_id).await {
        Ok(Some(user)) => user,
        Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "user not found"),
    };
    if cordy_auth::disabled_users::is_temporarily_disabled_user(&user.id.to_string(), &user.email) {
        return error_response(StatusCode::FORBIDDEN, "account disabled");
    }
    match cordy_auth::jwt::issue_user_jwt(&user.id.to_string(), &user.email, &user.name) {
        Ok(token) => Json(serde_json::json!({ "token": token })).into_response(),
        Err(error) => {
            tracing::warn!(%error, %user_id, "cli-token: failed to issue JWT");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to generate token",
            )
        }
    }
}
