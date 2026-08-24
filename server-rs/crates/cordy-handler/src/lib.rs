//! cordy-handler — HTTP handler layer (S8).
//!
//! Port of `server/cmd/server/router.go` + `server/internal/handler` +
//! `server/internal/realtime` WS pump (HandleWebSocket/readPump/writePump),
//! on axum. Routes are ported domain-by-domain; each domain module exposes a
//! `router()` merged into the app router in this file.
//!
//! Handler validation helpers intentionally return complete Axum responses so
//! every rejection preserves the Go wire shape at its source.

#![allow(clippy::result_large_err)]

pub mod agent_aggregation;
pub mod agent_builder;
pub mod attachment;
pub mod attachment_access;
pub mod attachment_storage;
pub mod auth;
pub mod avatar;
pub mod claim_comments;
pub mod claim_response;
pub mod cli_token;
pub mod client_usage;
pub mod cloudfront;
pub mod comment;
pub mod comment_list;
pub mod config;
pub mod contact_sales;
pub mod daemon;
pub mod daemon_ws;
pub mod dashboard;
pub mod error;
pub mod feedback;
pub mod health;
pub mod inbox;
pub mod invitation;
pub mod issue;
pub mod issue_property_value;
pub mod issue_pull_request;
pub mod issue_status;
pub mod issue_view;
pub mod issue_view_preference;
pub mod label;
pub mod mcp_merge;
pub mod me;
pub mod notification;
pub mod pat;
pub mod pending_store;
pub mod pin;
pub mod plugin_action;
pub mod plugin_admin;
pub mod profile_json;
pub mod project;
pub mod property;
pub mod quick_action;
pub mod runtime;
pub mod runtime_requests;
pub mod runtime_usage;
pub mod session;
pub mod skill;
mod skill_import;
pub mod squad;
pub mod squad_briefing;
pub mod state;
pub mod task;
pub mod task_json;
pub mod timefmt;
pub mod vcs_webhook;
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
        HeaderName::from_static("x-cordy-next-before"),
        HeaderName::from_static("x-cordy-next-before-id"),
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

    build_router_from_state(state)
}

/// Assemble the HTTP router from fully wired state. Production uses this
/// entry point so observability, storage, and download policy all come from
/// the same loaded configuration; tests retain [`build_router`].
pub fn build_router_from_state(state: HandlerState) -> Router {
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
    let public_auth = auth::public_router(
        state.auth_rate_limit.clone(),
        state.auth_verify_rate_limit.clone(),
    );

    let issue_routes = issue::router().route_layer(middleware::from_fn_with_state(
        WorkspaceGuardState::member_only(state.pool.clone()),
        issue::require_issue_workspace,
    ));
    let task_routes = task::router().route_layer(middleware::from_fn_with_state(
        WorkspaceGuardState::member_only(state.pool.clone()),
        issue::require_issue_workspace,
    ));
    let attachment_routes =
        attachment_access::workspace_router().route_layer(middleware::from_fn_with_state(
            WorkspaceGuardState::member_only(state.pool.clone()),
            issue::require_issue_workspace,
        ));
    let attachment_mutation_routes =
        attachment::workspace_router().route_layer(middleware::from_fn_with_state(
            WorkspaceGuardState::member_only(state.pool.clone()),
            cordy_middleware::workspace::require_workspace,
        ));
    let comment_routes = comment::router().route_layer(middleware::from_fn_with_state(
        WorkspaceGuardState::member_only(state.pool.clone()),
        issue::require_issue_workspace,
    ));
    let authenticated = workspace::authenticated_router()
        .merge(
            agent_aggregation::router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::member_only(state.pool.clone()),
                issue::require_issue_workspace,
            )),
        )
        .merge(
            agent_builder::router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::member_only(state.pool.clone()),
                cordy_middleware::workspace::require_workspace,
            )),
        )
        .merge(cli_token::router())
        .merge(client_usage::router())
        .merge(feedback::router())
        .merge(invitation::router())
        .merge(
            invitation::workspace_member_router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::from_url(state.pool.clone(), "id"),
                cordy_middleware::workspace::require_workspace,
            )),
        )
        .merge(
            invitation::workspace_admin_router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::from_url_with_roles(
                    state.pool.clone(),
                    "id",
                    vec!["owner".into(), "admin".into()],
                ),
                cordy_middleware::workspace::require_workspace,
            )),
        )
        .merge(me::router())
        .merge(
            dashboard::router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::member_only(state.pool.clone()),
                cordy_middleware::workspace::require_workspace,
            )),
        )
        .merge(inbox::router().route_layer(middleware::from_fn_with_state(
            WorkspaceGuardState::member_only(state.pool.clone()),
            cordy_middleware::workspace::require_workspace,
        )))
        .merge(
            runtime::router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::member_only(state.pool.clone()),
                cordy_middleware::workspace::require_workspace,
            )),
        )
        .merge(
            runtime_requests::router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::member_only(state.pool.clone()),
                cordy_middleware::workspace::require_workspace,
            )),
        )
        .merge(
            runtime_usage::router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::member_only(state.pool.clone()),
                cordy_middleware::workspace::require_workspace,
            )),
        )
        .merge(pat::router())
        .merge(attachment::authenticated_router())
        .merge(attachment_access::authenticated_router())
        .merge(attachment_routes)
        .merge(attachment_mutation_routes)
        .merge(
            project::router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::member_only(state.pool.clone()),
                cordy_middleware::workspace::require_workspace,
            )),
        )
        .merge(squad::router().route_layer(middleware::from_fn_with_state(
            WorkspaceGuardState::member_only(state.pool.clone()),
            cordy_middleware::workspace::require_workspace,
        )))
        .merge(skill::router().route_layer(middleware::from_fn_with_state(
            WorkspaceGuardState::member_only(state.pool.clone()),
            cordy_middleware::workspace::require_workspace,
        )))
        .merge(issue_routes)
        .merge(task_routes)
        .merge(comment_routes)
        .merge(label::router().route_layer(middleware::from_fn_with_state(
            WorkspaceGuardState::member_only(state.pool.clone()),
            issue::require_issue_workspace,
        )))
        .merge(
            issue_status::router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::member_only(state.pool.clone()),
                issue::require_issue_workspace,
            )),
        )
        .merge(
            issue_view::router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::member_only(state.pool.clone()),
                issue::require_issue_workspace,
            )),
        )
        .merge(
            issue_view_preference::router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::member_only(state.pool.clone()),
                issue::require_issue_workspace,
            )),
        )
        .merge(pin::router().route_layer(middleware::from_fn_with_state(
            WorkspaceGuardState::member_only(state.pool.clone()),
            issue::require_issue_workspace,
        )))
        .merge(
            plugin_admin::member_router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::from_url(state.pool.clone(), "id"),
                cordy_middleware::workspace::require_workspace,
            )),
        )
        .merge(
            plugin_admin::admin_router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::from_url_with_roles(
                    state.pool.clone(),
                    "id",
                    vec!["owner".into(), "admin".into()],
                ),
                cordy_middleware::workspace::require_workspace,
            )),
        )
        .merge(
            property::router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::member_only(state.pool.clone()),
                issue::require_issue_workspace,
            )),
        )
        .merge(
            quick_action::router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::member_only(state.pool.clone()),
                issue::require_issue_workspace,
            )),
        )
        .merge(
            notification::router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::member_only(state.pool.clone()),
                issue::require_issue_workspace,
            )),
        )
        .route_layer(middleware::from_fn_with_state(auth_state, auth_middleware));
    let plugin_action = plugin_action::router().route_layer(middleware::from_fn_with_state(
        AuthState {
            pool: state.pool.clone(),
            pat_cache: state.pat_cache.clone(),
        },
        cordy_middleware::plugin_auth::plugin_auth,
    ));
    let daemon = daemon::router().route_layer(middleware::from_fn_with_state(
        daemon_auth_state,
        daemon_auth_middleware,
    ));
    let contact_sales_limit = std::env::var("RATE_LIMIT_CONTACT_SALES")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(5);
    let contact_sales = contact_sales::router().route_layer(middleware::from_fn_with_state(
        cordy_middleware::ratelimit::RateLimitState {
            client: state.rate_limit_client.clone(),
            conn: Arc::new(tokio::sync::Mutex::new(None)),
            limit: contact_sales_limit,
            window_secs: 60 * 60,
            trusted_proxies: cordy_middleware::ratelimit::parse_trusted_proxies(
                &std::env::var("RATE_LIMIT_TRUSTED_PROXIES").unwrap_or_default(),
            ),
        },
        cordy_middleware::ratelimit::rate_limit,
    ));

    let http_metrics = state.http_metrics.clone();
    let app = Router::new()
        .merge(health::router())
        .merge(public_auth)
        .merge(session::public_router())
        .merge(workspace::public_router())
        .merge(attachment_access::public_router())
        .merge(avatar::router())
        .merge(config::router())
        .merge(contact_sales)
        .merge(vcs_webhook::router())
        .merge(plugin_action)
        .merge(authenticated)
        .merge(daemon)
        .route("/ws", get(ws::ws_handler))
        .with_state(state)
        .layer(cors_layer())
        .layer(middleware::from_fn(
            cordy_middleware::request_logger::request_logger,
        ))
        .layer(middleware::from_fn(
            cordy_middleware::client::client_metadata,
        ));

    match http_metrics {
        Some(metrics) => app.layer(middleware::from_fn_with_state(
            metrics,
            cordy_metrics::http::middleware,
        )),
        None => app,
    }
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
            "/api/issues/CORD-14/usage",
            "/api/issues/CORD-14/attachments",
            "/api/issues/CORD-14/active-task",
            "/api/issues/CORD-14/task-runs",
            "/api/issues/CORD-14/comments",
            "/api/issues/CORD-14/pull-requests",
            "/api/tasks/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/messages",
            "/api/me",
            "/api/invitations",
            "/api/invitations/018f03a0-c4d2-7a37-ae4d-5aa45de12f11",
            "/api/issue-views?scope_type=workspace",
            "/api/issue-views/018f03a0-c4d2-7a37-ae4d-5aa45de12f11",
            "/api/issue-view-preferences?scope_type=workspace",
            "/api/agent-activity-30d",
            "/api/agent-run-counts",
            "/api/agent-task-snapshot",
            "/api/agent-builder/sessions",
            "/api/skills",
            "/api/skills/018f03a0-c4d2-7a37-ae4d-5aa45de12f11",
            "/api/skills/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/files",
            "/api/skills/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/labels",
            "/api/properties",
            "/api/properties/018f03a0-c4d2-7a37-ae4d-5aa45de12f11",
            "/api/projects",
            "/api/projects/search?q=migration",
            "/api/projects/018f03a0-c4d2-7a37-ae4d-5aa45de12f11",
            "/api/projects/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/resources",
            "/api/squads",
            "/api/squads/018f03a0-c4d2-7a37-ae4d-5aa45de12f11",
            "/api/squads/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/members",
            "/api/squads/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/members/status",
            "/api/inbox",
            "/api/inbox/archived",
            "/api/inbox/unread-count",
            "/api/inbox/unread-summary",
            "/api/dashboard/usage/daily",
            "/api/dashboard/usage/by-agent",
            "/api/dashboard/agent-runtime",
            "/api/dashboard/runtime/daily",
            "/api/dashboard/failures/daily",
            "/api/dashboard/failures/by-agent",
            "/api/runtimes",
            "/api/runtimes/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/usage",
            "/api/runtimes/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/usage/by-agent",
            "/api/runtimes/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/usage/by-hour",
            "/api/runtimes/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/activity",
            "/api/runtimes/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/update/request-1",
            "/api/runtimes/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/models/request-1",
            "/api/runtimes/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/local-skills/request-1",
            "/api/runtimes/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/local-skills/import/request-1",
            "/api/working-agents",
            "/api/v1/plugin/context",
            "/api/v1/plugin/issues/CORD-14",
            "/api/v1/plugin/issues/CORD-14/comments",
            "/api/v1/plugin/storage/workspace",
            "/api/v1/plugin/storage/workspace/key",
            "/api/workspaces/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/plugins",
            "/api/workspaces/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/plugins/018f03a0-c4d2-7a37-ae4d-5aa45de12f12/invocations",
            "/api/workspaces/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/plugins/018f03a0-c4d2-7a37-ae4d-5aa45de12f12/mcp/search/tools",
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
            Request::post("/api/issues/CORD-14/tasks/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/cancel")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/issues/CORD-14/pull-requests")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"url":"https://github.com/alexj11324/Cordy/pull/24"}"#,
                ))
                .unwrap(),
            Request::post("/api/properties")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Severity","type":"text"}"#))
                .unwrap(),
            Request::patch("/api/properties/018f03a0-c4d2-7a37-ae4d-5aa45de12f11")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Impact"}"#))
                .unwrap(),
            Request::put("/api/projects/018f03a0-c4d2-7a37-ae4d-5aa45de12f11")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"status":"in_progress"}"#))
                .unwrap(),
            Request::post("/api/projects")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"title":"Migration"}"#))
                .unwrap(),
            Request::delete("/api/projects/018f03a0-c4d2-7a37-ae4d-5aa45de12f11")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/projects/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/resources")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"resource_type":"github_repo","resource_ref":{"url":"https://github.com/alexj11324/Cordy"}}"#,
                ))
                .unwrap(),
            Request::put("/api/projects/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/resources/018f03a0-c4d2-7a37-ae4d-5aa45de12f12")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"label":"Backend"}"#))
                .unwrap(),
            Request::delete("/api/projects/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/resources/018f03a0-c4d2-7a37-ae4d-5aa45de12f12")
                .body(Body::empty())
                .unwrap(),
            Request::delete("/api/squads/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/members")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"member_type":"agent","member_id":"018f03a0-c4d2-7a37-ae4d-5aa45de12f12"}"#))
                .unwrap(),
            Request::post("/api/squads/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/members")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"member_type":"agent","member_id":"018f03a0-c4d2-7a37-ae4d-5aa45de12f12","role":"worker"}"#))
                .unwrap(),
            Request::post("/api/squads")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Migration","leader_id":"018f03a0-c4d2-7a37-ae4d-5aa45de12f12"}"#))
                .unwrap(),
            Request::put("/api/squads/018f03a0-c4d2-7a37-ae4d-5aa45de12f11")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Migration Core"}"#))
                .unwrap(),
            Request::delete("/api/squads/018f03a0-c4d2-7a37-ae4d-5aa45de12f11")
                .body(Body::empty())
                .unwrap(),
            Request::patch("/api/squads/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/members/role")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"member_type":"agent","member_id":"018f03a0-c4d2-7a37-ae4d-5aa45de12f12","role":"reviewer"}"#))
                .unwrap(),
            Request::post("/api/inbox/mark-all-read")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/inbox/archive-all")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/inbox/archive-all-read")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/inbox/archive-completed")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/inbox/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/read")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/inbox/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/unread")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/inbox/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/archive")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/inbox/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/unarchive")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/runtimes/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/update")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"target_version":"v1.2.3"}"#))
                .unwrap(),
            Request::post("/api/runtimes/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/models")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/runtimes/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/local-skills")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/runtimes/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/local-skills/import")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"skill_key":"review-helper"}"#))
                .unwrap(),
            Request::post("/api/agent-builder/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"runtime_id":"018f03a0-c4d2-7a37-ae4d-5aa45de12f11"}"#,
                ))
                .unwrap(),
            Request::patch("/api/agent-builder/sessions/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/runtime")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"runtime_id":"018f03a0-c4d2-7a37-ae4d-5aa45de12f12"}"#,
                ))
                .unwrap(),
            Request::put("/api/agent-builder/sessions/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/draft")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"draft":{"name":"Migration"}}"#))
                .unwrap(),
            Request::post("/api/skills")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"review"}"#))
                .unwrap(),
            Request::put("/api/skills/018f03a0-c4d2-7a37-ae4d-5aa45de12f11")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"description":"Review changes"}"#))
                .unwrap(),
            Request::delete("/api/skills/018f03a0-c4d2-7a37-ae4d-5aa45de12f11")
                .body(Body::empty())
                .unwrap(),
            Request::put("/api/skills/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/files")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"path":"references/checklist.md"}"#))
                .unwrap(),
            Request::delete("/api/skills/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/files/018f03a0-c4d2-7a37-ae4d-5aa45de12f12")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/skills/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/labels")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"label_id":"018f03a0-c4d2-7a37-ae4d-5aa45de12f12"}"#,
                ))
                .unwrap(),
            Request::delete("/api/skills/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/labels/018f03a0-c4d2-7a37-ae4d-5aa45de12f12")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/issue-views")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Assigned","scope_type":"my","scope_variant":"assigned","query":{}}"#,
                ))
                .unwrap(),
            Request::post("/api/invitations/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/accept")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/invitations/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/decline")
                .body(Body::empty())
                .unwrap(),
            Request::patch("/api/issue-views/018f03a0-c4d2-7a37-ae4d-5aa45de12f11")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Mine","expected_revision":1}"#))
                .unwrap(),
            Request::delete("/api/issue-views/018f03a0-c4d2-7a37-ae4d-5aa45de12f11")
                .body(Body::empty())
                .unwrap(),
            Request::delete("/api/issues/CORD-14/properties/018f03a0-c4d2-7a37-ae4d-5aa45de12f11")
                .body(Body::empty())
                .unwrap(),
            Request::put("/api/issues/CORD-14/properties/018f03a0-c4d2-7a37-ae4d-5aa45de12f11")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"value":"high"}"#))
                .unwrap(),
            Request::post("/api/tasks/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/cancel")
                .body(Body::empty())
                .unwrap(),
            Request::patch("/api/me")
                .body(Body::from(r#"{"name":"Alex"}"#))
                .unwrap(),
            Request::patch("/api/me/onboarding")
                .body(Body::from(r#"{"questionnaire":{}}"#))
                .unwrap(),
            Request::post("/api/me/onboarding/complete")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/me/onboarding/cloud-waitlist")
                .body(Body::from(r#"{"email":"alex@example.com"}"#))
                .unwrap(),
            Request::put("/api/issue-view-preferences")
                .body(Body::from(r#"{"scope_type":"workspace","prefs":{}}"#))
                .unwrap(),
            Request::post("/api/share-links/join")
                .body(Body::from(r#"{"code":"invite"}"#))
                .unwrap(),
            Request::post("/api/feedback")
                .body(Body::from(r#"{"message":"feedback"}"#))
                .unwrap(),
            Request::post("/api/client-usage")
                .body(Body::from(
                    r#"{"install_id":"018f03a0-c4d2-7a37-ae4d-5aa45de12f11"}"#,
                ))
                .unwrap(),
            Request::post("/api/comments/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/resolve")
                .body(Body::empty())
                .unwrap(),
            Request::delete("/api/comments/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/resolve")
                .body(Body::empty())
                .unwrap(),
            Request::post("/api/comments/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/reactions")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"emoji":"👍"}"#))
                .unwrap(),
            Request::post("/api/cli-token").body(Body::empty()).unwrap(),
            Request::delete("/api/comments/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/reactions")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"emoji":"👍"}"#))
                .unwrap(),
            Request::patch("/api/runtimes/018f03a0-c4d2-7a37-ae4d-5aa45de12f11")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"custom_name":"Prod Box"}"#))
                .unwrap(),
            Request::patch("/api/v1/plugin/issues/CORD-14")
                .body(Body::from(r#"{"title":"Updated"}"#))
                .unwrap(),
            Request::post("/api/v1/plugin/issues/CORD-14/comments")
                .body(Body::from(r#"{"content":"hello"}"#))
                .unwrap(),
            Request::post("/api/v1/plugin/hooks/summarize")
                .body(Body::from(r#"{"trigger":"manual"}"#))
                .unwrap(),
            Request::put("/api/v1/plugin/storage/workspace/key")
                .body(Body::from(r#"{"value":"saved"}"#))
                .unwrap(),
            Request::delete("/api/v1/plugin/storage/workspace/key")
                .body(Body::empty())
                .unwrap(),
            Request::post(
                "/api/workspaces/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/plugins/preview",
            )
            .body(Body::from(r#"{"source_url":"https://example.com/plugin.json"}"#))
            .unwrap(),
            Request::put(
                "/api/workspaces/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/plugins/018f03a0-c4d2-7a37-ae4d-5aa45de12f12/config",
            )
            .body(Body::from(r#"{"values":{}}"#))
            .unwrap(),
            Request::post(
                "/api/workspaces/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/plugins/018f03a0-c4d2-7a37-ae4d-5aa45de12f12/enable",
            )
            .body(Body::empty())
            .unwrap(),
            Request::delete(
                "/api/workspaces/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/plugins/018f03a0-c4d2-7a37-ae4d-5aa45de12f12",
            )
            .body(Body::empty())
            .unwrap(),
            Request::post(
                "/api/workspaces/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/plugins/018f03a0-c4d2-7a37-ae4d-5aa45de12f12/token",
            )
            .body(Body::empty())
            .unwrap(),
            Request::delete(
                "/api/workspaces/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/plugins/018f03a0-c4d2-7a37-ae4d-5aa45de12f12/token",
            )
            .body(Body::empty())
            .unwrap(),
            Request::put(
                "/api/workspaces/018f03a0-c4d2-7a37-ae4d-5aa45de12f11/plugins/018f03a0-c4d2-7a37-ae4d-5aa45de12f12/mcp/search/tools",
            )
            .body(Body::from(r#"{"tools":[]}"#))
            .unwrap(),
        ] {
            let response = build_router(None, None).oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn signed_attachment_route_is_public_but_rejects_invalid_capability() {
        let response = build_router(None, None)
            .oneshot(
                Request::get(
                    "/api/attachments/018f03a0-c4d2-7a37-ae4d-5aa45de12f13/signed-download?exp=1&sig=bad",
                )
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
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
