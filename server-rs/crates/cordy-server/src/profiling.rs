//! Local CPU profiling endpoint, ported from `server/internal/profiling`.

use std::sync::OnceLock;
use std::time::Duration;

use axum::body::Body;
use axum::extract::Query;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use flate2::write::GzEncoder;
use flate2::Compression;
use pprof::protos::Message;
use serde::Deserialize;
use std::io::Write;

pub const ADDR: &str = "127.0.0.1:6060";

const DEFAULT_PROFILE_SECONDS: u64 = 30;
const PROFILE_FREQUENCY_HZ: i32 = 100;

#[derive(Debug, Deserialize)]
struct ProfileQuery {
    seconds: Option<u64>,
}

pub fn router() -> Router {
    Router::new()
        .route("/debug/pprof/", get(index))
        .route("/debug/pprof/cmdline", get(cmdline))
        .route("/debug/pprof/profile", get(profile))
}

pub async fn serve() -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(ADDR).await?;
    axum::serve(listener, router()).await?;
    Ok(())
}

async fn index() -> impl IntoResponse {
    "<html><head><title>pprof</title></head><body>\
        <a href=\"/debug/pprof/profile\">profile</a><br>\
        <a href=\"/debug/pprof/cmdline\">cmdline</a>\
    </body></html>"
}

async fn cmdline() -> Response {
    let body = std::env::args_os()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\0");
    (
        [(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"))],
        body,
    )
        .into_response()
}

async fn profile(Query(query): Query<ProfileQuery>) -> Response {
    let seconds = profile_seconds(query.seconds);
    let lock = profile_lock();
    // ponytail: one global profile at a time; pprof itself is process-global,
    // so concurrent captures cannot provide independent samples.
    let Ok(_profile_lock) = lock.try_lock() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "another CPU profile is already running",
        );
    };

    let guard = match pprof::ProfilerGuard::new(PROFILE_FREQUENCY_HZ) {
        Ok(guard) => guard,
        Err(error) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("start CPU profiler: {error}"),
            )
        }
    };
    tokio::time::sleep(Duration::from_secs(seconds)).await;

    let report = match guard.report().build() {
        Ok(report) => report,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("build CPU profile: {error}"),
            )
        }
    };
    let profile = match report.pprof() {
        Ok(profile) => profile,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("encode CPU profile: {error}"),
            )
        }
    };
    let body = match profile.write_to_bytes() {
        Ok(body) => body,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("serialize CPU profile: {error}"),
            )
        }
    };
    let mut compressed = GzEncoder::new(Vec::new(), Compression::default());
    if let Err(error) = compressed.write_all(&body) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("compress CPU profile: {error}"),
        );
    }
    let body = match compressed.finish() {
        Ok(body) => body,
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("finish CPU profile: {error}"),
            )
        }
    };

    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            ),
            (header::CONTENT_ENCODING, HeaderValue::from_static("gzip")),
        ],
        Body::from(body),
    )
        .into_response()
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({"error": message}))).into_response()
}

fn profile_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_seconds_defaults_for_missing_and_zero() {
        assert_eq!(profile_seconds(None), DEFAULT_PROFILE_SECONDS);
        assert_eq!(profile_seconds(Some(0)), DEFAULT_PROFILE_SECONDS);
        assert_eq!(profile_seconds(Some(7)), 7);
    }
}

fn profile_seconds(seconds: Option<u64>) -> u64 {
    seconds
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_PROFILE_SECONDS)
}
