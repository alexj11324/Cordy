//! Loopback-only CPU profiling for the Rust server.
//!
//! This is the CPU-profile part of Go's `internal/profiling` contract. The
//! listener stays separate from the public API and is intentionally fixed to
//! the loopback address, matching the Go server's security boundary.

use axum::{
    body::Body,
    extract::Query,
    http::{header, HeaderValue, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

pub const ADDR: &str = "127.0.0.1:6060";
const DEFAULT_PROFILE_SECONDS: u64 = 30;
const INDEX: &str = "<!doctype html><html><head><title>pprof</title></head><body>\
<h1>pprof</h1><ul>\
<li><a href=\"/debug/pprof/profile\">profile</a></li>\
<li><a href=\"/debug/pprof/cmdline\">cmdline</a></li>\
<li><a href=\"/debug/pprof/symbol\">symbol</a></li>\
</ul></body></html>\n";

#[derive(Debug, Deserialize)]
struct ProfileQuery {
    seconds: Option<u64>,
}

pub async fn serve(shutdown: CancellationToken) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(ADDR).await?;
    tracing::info!(addr = ADDR, "pprof server listening");

    axum::serve(listener, router())
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await?;
    Ok(())
}

fn router() -> Router {
    Router::new()
        .route("/debug/pprof", get(redirect_to_index))
        .route("/debug/pprof/", get(index))
        .route("/debug/pprof/cmdline", get(cmdline))
        .route("/debug/pprof/profile", get(profile))
        .route("/debug/pprof/symbol", get(symbol_get).post(symbol_post))
        .route("/debug/pprof/trace", get(trace))
}

async fn redirect_to_index() -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::TEMPORARY_REDIRECT;
    response
        .headers_mut()
        .insert(header::LOCATION, HeaderValue::from_static("/debug/pprof/"));
    response
}

async fn index() -> Response {
    text_response(StatusCode::OK, INDEX)
}

async fn cmdline() -> Response {
    let mut response = Response::new(Body::from(command_line()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

async fn profile(Query(query): Query<ProfileQuery>) -> Response {
    let seconds = query.seconds.unwrap_or(DEFAULT_PROFILE_SECONDS);
    let result = tokio::task::spawn_blocking(move || capture_cpu_profile(seconds)).await;

    match result {
        Ok(Ok(profile)) => binary_response(profile),
        Ok(Err(error)) => {
            tracing::warn!(%error, "CPU profile capture failed");
            text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "CPU profile capture failed\n",
            )
        }
        Err(error) => {
            tracing::warn!(%error, "CPU profile worker failed");
            text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "CPU profile worker failed\n",
            )
        }
    }
}

async fn symbol_get() -> Response {
    text_response(StatusCode::OK, "num_symbols: 1\n")
}

async fn symbol_post(body: axum::body::Bytes) -> Response {
    let output = tokio::task::spawn_blocking(move || resolve_symbols(&body)).await;
    match output {
        Ok(output) => text_response(StatusCode::OK, &output),
        Err(error) => {
            tracing::warn!(%error, "pprof symbol worker failed");
            text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "pprof symbol worker failed\n",
            )
        }
    }
}

async fn trace() -> Response {
    text_response(
        StatusCode::NOT_IMPLEMENTED,
        "runtime trace is not available in the Rust server\n",
    )
}

fn text_response(status: StatusCode, body: &str) -> Response {
    let mut response = Response::new(Body::from(body.to_owned()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

fn binary_response(body: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response
        .headers_mut()
        .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    response
}

fn command_line() -> Vec<u8> {
    let mut command = Vec::new();
    for (index, argument) in std::env::args_os().enumerate() {
        if index > 0 {
            command.push(0);
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            command.extend_from_slice(argument.as_os_str().as_bytes());
        }
        #[cfg(not(unix))]
        command.extend_from_slice(argument.to_string_lossy().as_bytes());
    }
    command
}

#[cfg(unix)]
fn resolve_symbols(body: &[u8]) -> String {
    use std::ffi::c_void;

    let mut output = String::new();
    for raw_line in body.split(|byte| *byte == b'\n') {
        let line = String::from_utf8_lossy(raw_line).trim().to_owned();
        if line.is_empty() {
            continue;
        }

        let address = line
            .strip_prefix("0x")
            .or_else(|| line.strip_prefix("0X"))
            .and_then(|value| usize::from_str_radix(value, 16).ok());
        let mut name = None;
        if let Some(address) = address {
            backtrace::resolve(address as *mut c_void, |symbol| {
                if name.is_none() {
                    name = symbol.name().map(|value| value.to_string());
                }
            });
        }
        output.push_str(&line);
        output.push(' ');
        output.push_str(name.as_deref().unwrap_or("unknown"));
        output.push('\n');
    }
    output
}

#[cfg(not(unix))]
fn resolve_symbols(body: &[u8]) -> String {
    body.split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| format!("{} unknown\n", String::from_utf8_lossy(line).trim()))
        .collect()
}

#[cfg(unix)]
fn capture_cpu_profile(seconds: u64) -> anyhow::Result<Vec<u8>> {
    use flate2::{write::GzEncoder, Compression};
    use pprof::{protos::Message, ProfilerGuardBuilder};
    use std::io::Write;
    use std::time::Duration;

    let guard = ProfilerGuardBuilder::default().frequency(99).build()?;
    std::thread::sleep(Duration::from_secs(seconds));

    let profile = guard.report().build()?.pprof()?;
    let mut encoded = Vec::new();
    profile.write_to_vec(&mut encoded)?;

    let mut compressed = GzEncoder::new(Vec::new(), Compression::default());
    compressed.write_all(&encoded)?;
    Ok(compressed.finish()?)
}

#[cfg(not(unix))]
fn capture_cpu_profile(_: u64) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("CPU profiling is unavailable on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[test]
    fn uses_the_fixed_loopback_management_address() {
        assert_eq!(ADDR, "127.0.0.1:6060");
    }

    #[test]
    fn symbol_protocol_returns_unknown_for_unresolved_addresses() {
        assert_eq!(
            resolve_symbols(b"0xnot-an-address\n"),
            "0xnot-an-address unknown\n"
        );
    }

    #[tokio::test]
    async fn index_is_available_only_on_the_profiling_router() {
        let response = router()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/debug/pprof/")
                    .body(Body::empty())
                    .unwrap_or_else(|_| unreachable!()),
            )
            .await
            .unwrap_or_else(|_| unreachable!());

        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|_| unreachable!())
            .to_bytes();
        assert!(body
            .windows(b"/debug/pprof/profile".len())
            .any(|window| window == b"/debug/pprof/profile"));
    }

    #[tokio::test]
    async fn heap_is_not_accidentally_exposed_by_the_cpu_slice() {
        let response = router()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/debug/pprof/heap")
                    .body(Body::empty())
                    .unwrap_or_else(|_| unreachable!()),
            )
            .await
            .unwrap_or_else(|_| unreachable!());

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
