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
pub mod issue;
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
use std::time::Duration;

use axum::http::{header, HeaderName, HeaderValue, Method};
use axum::routing::get;
use axum::{middleware, Router};
use cordy_auth::daemon_token_cache::DaemonTokenCache;
use cordy_middleware::auth::{auth_middleware, AuthState};
use cordy_middleware::daemon_auth::{daemon_auth_middleware, DaemonAuthState};
use cordy_middleware::workspace::WorkspaceGuardState;
use cordy_realtime::hub::Hub;
use tower_http::cors::{AllowOrigin, CorsLayer};

pub use state::HandlerState;

pub(crate) fn allowed_origins() -> Vec<String> {
    let raw = ["CORS_ALLOWED_ORIGINS", "FRONTEND_ORIGIN"]
        .into_iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        });
    raw.map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(str::to_string)
            .collect()
    })
    .filter(|origins: &Vec<_>| !origins.is_empty())
    .unwrap_or_else(|| {
        vec![
            "http://localhost:3000".to_string(),
            "http://localhost:5173".to_string(),
            "http://localhost:5174".to_string(),
        ]
    })
}

fn cors_layer() -> CorsLayer {
    let origins = allowed_origins()
        .into_iter()
        .filter_map(|origin| HeaderValue::from_str(&origin).ok());
    let allowed_headers = [
        header::ACCEPT,
        header::AUTHORIZATION,
        header::CONTENT_TYPE,
        HeaderName::from_static("idempotency-key"),
        HeaderName::from_static("x-workspace-id"),
        HeaderName::from_static("x-workspace-slug"),
        HeaderName::from_static("x-request-id"),
        HeaderName::from_static("x-agent-id"),
        HeaderName::from_static("x-task-id"),
        HeaderName::from_static("x-csrf-token"),
        HeaderName::from_static("x-client-platform"),
        HeaderName::from_static("x-client-version"),
        HeaderName::from_static("x-client-os"),
        HeaderName::from_static("x-client-capabilities"),
        HeaderName::from_static("x-cordy-plugin-installation"),
    ];
    let exposed_headers = [
        HeaderName::from_static("x-comments-truncated"),
        HeaderName::from_static("x-timeline-truncated"),
    ];

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(allowed_headers)
        .expose_headers(exposed_headers)
        .allow_credentials(true)
        .max_age(Duration::from_secs(300))
}

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

    let issue_routes = issue::router().route_layer(middleware::from_fn_with_state(
        WorkspaceGuardState::member_only(state.pool.clone()),
        issue::require_issue_workspace,
    ));
    let authenticated = workspace::authenticated_router()
        .merge(issue_routes)
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
        .layer(cors_layer())
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
    async fn authenticated_issue_collection_rejects_anonymous_requests() {
        for uri in [
            "/api/issues",
            "/api/issues/children?parent_ids=018f03a0-c4d2-7a37-ae4d-5aa45de12f11",
            "/api/issues/child-progress",
            "/api/assignee-frequency",
        ] {
            let response = build_router(None, None)
                .oneshot(Request::get(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{uri}");
        }
    }

    #[tokio::test]
    async fn authenticated_issue_mutations_reject_anonymous_requests() {
        for request in [
            Request::put("/api/issues/CORD-14/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"status":"in_review"}"#))
                .unwrap(),
            Request::post("/api/issues/batch-update")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"issue_ids":[],"updates":{}}"#))
                .unwrap(),
        ] {
            let response = build_router(None, None).oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
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

    #[tokio::test]
    async fn cors_preflight_allows_browser_auth_headers() {
        let origin = allowed_origins().into_iter().next().unwrap();
        let response = build_router(None, None)
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/workspaces")
                    .header("origin", &origin)
                    .header("access-control-request-method", "GET")
                    .header(
                        "access-control-request-headers",
                        "authorization,x-workspace-id,x-client-capabilities",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .and_then(|value| value.to_str().ok()),
            Some(origin.as_str())
        );
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-credentials")
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );
    }
}
