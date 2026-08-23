//! cordy-handler — HTTP handler layer (S8).
//!
//! Port of `server/cmd/server/router.go` + `server/internal/handler` +
//! `server/internal/realtime` WS pump (HandleWebSocket/readPump/writePump),
//! on axum. Routes are ported domain-by-domain; each domain module exposes a
//! `router()` merged into the app router in this file.

pub mod claim_comments;
pub mod claim_response;
pub mod daemon;
pub mod daemon_ws;
pub mod error;
pub mod health;
pub mod mcp_merge;
pub mod pending_store;
pub mod profile_json;
pub mod squad_briefing;
pub mod state;
pub mod task_json;
pub mod timefmt;
pub mod workspace;
pub mod ws;

use std::sync::Arc;

use axum::routing::get;
use axum::{middleware, Router};
use cordy_auth::daemon_token_cache::DaemonTokenCache;
use cordy_middleware::auth::{auth_middleware, AuthState};
use cordy_middleware::daemon_auth::{daemon_auth_middleware, DaemonAuthState};
use cordy_realtime::hub::Hub;

pub use state::HandlerState;

/// Build the application router. Mirrors router.go's assembly order:
/// global middleware → health → WS → per-domain route groups (auth'd groups
/// mount `cordy_middleware::auth::auth_middleware`).
///
/// DB pool is optional so tests can exercise the router without Postgres.
pub fn build_router(db: Option<sqlx::PgPool>, hub: Option<Arc<Hub>>) -> Router {
    let state = match db {
        Some(pool) => HandlerState::new(
            pool,
            // Redis-backed PAT cache lands with the redis wiring slice; the
            // disabled cache degrades to direct DB lookups exactly like Go's
            // nil-cache path.
            cordy_auth::pat_cache::PatCache::disabled(),
            hub,
        ),
        None => HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap_or_else(|_| {
                sqlx::PgPool::connect_lazy("postgres://invalid/invalid")
                    .unwrap_or_else(|_| unreachable!())
            }),
            cordy_auth::pat_cache::PatCache::disabled(),
            hub,
        ),
    };

    if let Some(hub) = state.hub.as_ref() {
        hub.set_authorizer(Arc::new(ws::DbScopeAuthorizer::new(state.tasks.clone())));
    }

    let auth_state = AuthState {
        pool: state.pool.clone(),
        pat_cache: state.pat_cache.clone(),
    };
    let daemon_auth_state = DaemonAuthState {
        pool: state.pool.clone(),
        pat_cache: state.pat_cache.clone(),
        daemon_cache: DaemonTokenCache::disabled(),
    };

    let authenticated = workspace::authenticated_router()
        .route_layer(middleware::from_fn_with_state(auth_state, auth_middleware));
    let daemon = daemon::router().route_layer(middleware::from_fn_with_state(
        daemon_auth_state,
        daemon_auth_middleware,
    ));

    Router::new()
        .merge(health::router())
        .merge(workspace::public_router())
        .merge(authenticated)
        .merge(daemon)
        .route("/ws", get(ws::ws_handler))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn daemon_routes_are_mounted_behind_daemon_auth() {
        let response = build_router(None, None)
            .oneshot(
                Request::post("/api/daemon/heartbeat")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authenticated_workspace_collection_rejects_anonymous_requests() {
        let response = build_router(None, None)
            .oneshot(Request::get("/api/workspaces").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn daemon_routes_are_mounted_and_protected() {
        let response = build_router(None, None)
            .oneshot(
                Request::get("/api/daemon/workspaces")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
