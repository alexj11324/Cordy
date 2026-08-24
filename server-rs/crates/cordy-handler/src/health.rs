//! Health / readiness endpoints — port of router.go's
//! `/health`, `/readyz`, `/healthz` (newServerHealth in Go).

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Extension, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
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
        .route("/health/realtime", get(realtime))
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

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

fn has_bearer_token(headers: &HeaderMap, expected: &str) -> bool {
    let Some(raw) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some((scheme, token)) = raw.split_once(' ') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer")
        && !token.trim().is_empty()
        && constant_time_eq(token.trim().as_bytes(), expected.as_bytes())
}

fn has_forwarding_header(headers: &HeaderMap) -> bool {
    [
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-real-ip",
        "forwarded",
    ]
    .iter()
    .any(|name| {
        headers
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| !value.trim().is_empty())
    })
}

fn is_direct_loopback(headers: &HeaderMap, peer: Option<SocketAddr>) -> bool {
    !has_forwarding_header(headers) && peer.is_some_and(|peer| peer.ip().is_loopback())
}

fn plain_error(status: StatusCode, body: &'static str) -> Response {
    let mut response = (status, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

async fn realtime(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
) -> Response {
    if !state.realtime_metrics_token.is_empty() {
        if !has_bearer_token(&headers, &state.realtime_metrics_token) {
            let mut response = plain_error(StatusCode::UNAUTHORIZED, "unauthorized\n");
            response.headers_mut().insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"metrics\""),
            );
            return response;
        }
    } else if !is_direct_loopback(&headers, peer.map(|Extension(ConnectInfo(peer))| peer)) {
        return plain_error(StatusCode::NOT_FOUND, "404 page not found\n");
    }

    let mut snapshot = cordy_realtime::M.snapshot();
    if let Some(object) = snapshot.as_object_mut() {
        object.insert("daemonws".into(), cordy_daemon::hub::M.snapshot());
    }
    let mut response = Json(snapshot).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_state(token: &str) -> HandlerState {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let mut state = HandlerState::new(pool, cordy_auth::pat_cache::PatCache::disabled(), None);
        state.realtime_metrics_token = token.to_string();
        state
    }

    #[test]
    fn bearer_token_is_case_insensitive_and_constant_time_compared() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "bEaReR secret".parse().unwrap());
        assert!(has_bearer_token(&headers, "secret"));
        assert!(!has_bearer_token(&headers, "secrex"));
        assert!(!has_bearer_token(&headers, "longer-secret"));
    }

    #[test]
    fn loopback_shortcut_rejects_forwarded_requests() {
        let peer: SocketAddr = "127.0.0.1:1234".parse().unwrap();
        let mut headers = HeaderMap::new();
        assert!(is_direct_loopback(&headers, Some(peer)));
        headers.insert("x-forwarded-for", "203.0.113.10".parse().unwrap());
        assert!(!is_direct_loopback(&headers, Some(peer)));
        assert!(!is_direct_loopback(&HeaderMap::new(), None));
    }

    #[tokio::test]
    async fn realtime_route_enforces_bearer_and_returns_both_snapshots() {
        let app = router().with_state(test_state("secret"));
        let denied = app
            .clone()
            .oneshot(
                Request::get("/health/realtime")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            denied.headers()[header::WWW_AUTHENTICATE],
            "Bearer realm=\"metrics\""
        );

        let allowed = app
            .oneshot(
                Request::get("/health/realtime")
                    .header(header::AUTHORIZATION, "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
        assert_eq!(allowed.headers()[header::CACHE_CONTROL], "no-store");
        let body = axum::body::to_bytes(allowed.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(payload.get("connects_total").is_some());
        assert!(payload.get("daemonws").is_some());
    }

    #[tokio::test]
    async fn realtime_route_hides_forwarded_loopback_without_token() {
        let app = router().with_state(test_state(""));
        let peer: SocketAddr = "127.0.0.1:1234".parse().unwrap();
        let allowed = app
            .clone()
            .oneshot(
                Request::get("/health/realtime")
                    .extension(ConnectInfo(peer))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);

        let hidden = app
            .oneshot(
                Request::get("/health/realtime")
                    .header("x-forwarded-for", "203.0.113.10")
                    .extension(ConnectInfo(peer))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    }
}
