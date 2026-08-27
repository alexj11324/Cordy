//! Health / readiness endpoints — port of router.go's
//! `/health`, `/readyz`, `/healthz` (newServerHealth in Go).

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Extension, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::state::HandlerState;

const MAX_MIGRATION_SEARCH_DEPTH: usize = 4;
const MIGRATION_DIR_CANDIDATES: [&str; 2] = ["migrations", "server/migrations"];

/// The migration set required by this checkout. It is discovered once when
/// handler state is built, just like the Go server's `newServerHealth`; the
/// readiness endpoint then verifies the complete set rather than only the
/// lexically latest migration.
#[derive(Clone, Debug)]
pub struct ReadinessState {
    required_migrations: Arc<Vec<String>>,
    init_error: Option<Arc<str>>,
}

impl Default for ReadinessState {
    fn default() -> Self {
        Self::discover()
    }
}

impl ReadinessState {
    pub fn discover() -> Self {
        match all_migration_versions() {
            Ok(versions) if !versions.is_empty() => Self {
                required_migrations: Arc::new(versions),
                init_error: None,
            },
            Ok(_) => Self {
                required_migrations: Arc::new(Vec::new()),
                init_error: Some(Arc::from("no up migrations found")),
            },
            Err(error) => Self {
                required_migrations: Arc::new(Vec::new()),
                init_error: Some(Arc::from(error.to_string())),
            },
        }
    }

    fn required_migrations(&self) -> &[String] {
        self.required_migrations.as_slice()
    }
}

fn all_migration_versions() -> anyhow::Result<Vec<String>> {
    let mut roots = vec![std::env::current_dir()?];
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            roots.push(parent.to_path_buf());
        }
    }

    let mut seen = HashSet::new();
    for root in roots {
        let mut base = root;
        for _ in 0..=MAX_MIGRATION_SEARCH_DEPTH {
            for leaf in MIGRATION_DIR_CANDIDATES {
                let directory = base.join(leaf);
                if !seen.insert(directory.clone()) || !directory.is_dir() {
                    continue;
                }
                let mut files: Vec<PathBuf> = std::fs::read_dir(&directory)?
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.ends_with(".up.sql"))
                    })
                    .collect();
                files.sort_unstable();
                let versions = files
                    .iter()
                    .map(|path| migration_version(path).to_string())
                    .collect::<Vec<_>>();
                if !versions.is_empty() {
                    return Ok(versions);
                }
            }
            base = match base.parent() {
                Some(parent) => parent.to_path_buf(),
                None => break,
            };
        }
    }
    anyhow::bail!("migrations directory not found")
}

fn migration_version(path: &Path) -> &str {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .strip_suffix(".up.sql")
        .unwrap_or_default()
}

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

/// Readiness — reports both DB connectivity and schema currency (K8s
/// readiness semantics): 200 only when every migration required by this
/// binary is recorded in `schema_migrations`.
async fn ready(State(state): State<HandlerState>) -> Response {
    let db_check = tokio::time::timeout(Duration::from_secs(2), cordy_db::ping(&state.pool)).await;
    match db_check {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::error!(%error, "readyz: db ping failed");
            return readiness_response(
                "not_ready",
                "error",
                "unknown",
                StatusCode::SERVICE_UNAVAILABLE,
            );
        }
        Err(_) => {
            tracing::error!("readyz: db ping timed out");
            return readiness_response(
                "not_ready",
                "error",
                "unknown",
                StatusCode::SERVICE_UNAVAILABLE,
            );
        }
    }

    if state.readiness.init_error.is_some() {
        tracing::error!(error = ?state.readiness.init_error, "readyz: migration discovery failed");
        return readiness_response("not_ready", "ok", "error", StatusCode::SERVICE_UNAVAILABLE);
    }

    let required = state.readiness.required_migrations();
    let applied = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = ANY($1)",
        )
        .bind(required)
        .fetch_one(&state.pool),
    )
    .await;
    match applied {
        Ok(Ok(count)) if count >= required.len() as i64 => {
            readiness_response("ok", "ok", "ok", StatusCode::OK)
        }
        Ok(Ok(_)) => readiness_response(
            "not_ready",
            "ok",
            "out_of_date",
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        Ok(Err(error)) => {
            tracing::error!(%error, "readyz: migration query failed");
            readiness_response("not_ready", "ok", "error", StatusCode::SERVICE_UNAVAILABLE)
        }
        Err(_) => {
            tracing::error!("readyz: migration query timed out");
            readiness_response("not_ready", "ok", "error", StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

fn readiness_response(
    status: &'static str,
    db: &'static str,
    migrations: &'static str,
    status_code: StatusCode,
) -> Response {
    (
        status_code,
        Json(json!({
            "status": status,
            "checks": {"db": db, "migrations": migrations},
        })),
    )
        .into_response()
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
