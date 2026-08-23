//! Health / readiness endpoints — port of router.go's
//! `/health`, `/readyz`, `/healthz` (newServerHealth in Go).

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/health", get(live))
        .route("/readyz", get(ready))
        .route("/healthz", get(ready))
}

/// Liveness — process is up; no DB dependency.
async fn live() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

/// Readiness — reports DB state (K8s readiness semantics): 200 when the DB
/// answers, 503 with an error body when not.
async fn ready(State(state): State<HandlerState>) -> Response {
    match sqlx::query_scalar::<_, i32>("select 1")
        .fetch_one(&state.pool)
        .await
    {
        Ok(_) => (StatusCode::OK, Json(json!({ "status": "ready" }))).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "readyz: db ping failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "status": "unavailable", "error": e.to_string() })),
            )
                .into_response()
        }
    }
}
