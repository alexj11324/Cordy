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
    if matches!(
        headers
            .get("x-actor-source")
            .and_then(|value| value.to_str().ok()),
        Some("task_token" | "cloud_pat")
    ) {
        return error_response(
            StatusCode::FORBIDDEN,
            "this endpoint is only available to human actors",
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn machine_credentials_are_rejected_before_user_lookup() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let state = HandlerState::new(pool, cordy_auth::pat_cache::PatCache::disabled(), None);
        let app = router().with_state(state);

        for source in ["task_token", "cloud_pat"] {
            let response = app
                .clone()
                .oneshot(
                    Request::post("/api/cli-token")
                        .header("x-actor-source", source)
                        .header("x-user-id", Uuid::nil().to_string())
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{source}");
        }
    }
}
