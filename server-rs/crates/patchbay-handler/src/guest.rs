//! Server-backed guest sessions for the Desktop entry screen.
//!
//! A guest is a normal persisted user for the duration of the session. The
//! opaque bearer token is checked against the live session row by auth
//! middleware; no local-only identity is accepted by the API.

use axum::extract::{Json, State};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use patchbay_db::queries::{guest as guest_queries, user};
use uuid::Uuid;

use crate::auth::{LoginResponse, UserResponse};
use crate::error::error_code_response;
use crate::state::HandlerState;

const GUEST_NAME: &str = "Guest";

pub fn public_router(
    auth_limit: patchbay_middleware::ratelimit::RateLimitState,
) -> Router<HandlerState> {
    Router::new()
        .route("/auth/guest", post(create_guest))
        .route_layer(axum::middleware::from_fn_with_state(
            auth_limit,
            patchbay_middleware::ratelimit::rate_limit,
        ))
}

async fn create_guest(State(state): State<HandlerState>) -> Response {
    let user_id = Uuid::now_v7();
    let session_id = Uuid::now_v7();
    let token = match patchbay_auth::jwt::generate_guest_token() {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(%error, "guest auth: failed to generate session token");
            return error_code_response(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "guest_unavailable",
                "guest session unavailable",
            );
        }
    };
    let email = format!("guest+{}@guest.patchbay.invalid", user_id.simple());
    let token_hash = patchbay_auth::jwt::hash_token(&token);
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, "guest auth: failed to start session transaction");
            return error_code_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "guest_unavailable",
                "guest session unavailable",
            );
        }
    };
    let guest_user = match user::create_guest_user(&mut *tx, user_id, GUEST_NAME, &email).await {
        Ok(Some(user)) => user,
        Ok(None) | Err(_) => {
            return error_code_response(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "guest_unavailable",
                "guest session unavailable",
            );
        }
    };
    if let Err(error) = guest_queries::create_guest_session(
        &mut *tx,
        session_id,
        user_id,
        &token_hash,
    )
    .await
    {
        tracing::error!(%error, "guest auth: failed to persist session");
        return error_code_response(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "guest_unavailable",
            "guest session unavailable",
        );
    }
    if let Err(error) = tx.commit().await {
        tracing::error!(%error, "guest auth: failed to commit session");
        return error_code_response(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "guest_unavailable",
            "guest session unavailable",
        );
    }
    Json(LoginResponse {
        token,
        user: UserResponse::from_user(&state, &guest_user),
    })
    .into_response()
}
