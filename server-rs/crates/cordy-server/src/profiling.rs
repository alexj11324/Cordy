//! Loopback-only CPU profiling for the Rust server.
//!
//! This is the CPU-profile part of Go's `internal/profiling` contract. The
//! listener stays separate from the public API and is intentionally fixed to
//! the loopback address, matching the Go server's security boundary.

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderValue, StatusCode, Uri},
    response::Response,
    routing::get,
    Router,
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

pub const ADDR: &str = "127.0.0.1:6060";
const DEFAULT_PROFILE_SECONDS: u64 = 30;
const MAX_PROFILE_SECONDS: u64 = 60;
const MAX_SYMBOL_REQUEST_BYTES: usize = 64 * 1024;
const INDEX: &str = "<!doctype html><html><head><title>pprof</title></head><body>\
<h1>pprof</h1><ul>\
<li><a href=\"/debug/pprof/profile\">profile</a></li>\
<li><a href=\"/debug/pprof/cmdline\">cmdline</a></li>\
<li><a href=\"/debug/pprof/symbol\">symbol</a></li>\
</ul></body></html>\n";

#[derive(Debug, Deserialize)]
struct ProfileQuery {
    seconds: Option<String>,
}

pub async fn serve(shutdown: CancellationToken) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(ADDR).await?;
    tracing::info!(addr = ADDR, "pprof server listening");

    axum::serve(listener, router_with_shutdown(shutdown.clone()))
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await?;
    Ok(())
}

fn router() -> Router {
    router_with_shutdown(CancellationToken::new())
}

fn router_with_shutdown(shutdown: CancellationToken) -> Router {
    Router::new()
        .route("/debug/pprof", get(redirect_to_index))
        .route("/debug/pprof/", get(index))
        .route("/debug/pprof/cmdline", get(cmdline))
        .route("/debug/pprof/profile", get(profile))
        .route("/debug/pprof/symbol", get(symbol_get).post(symbol_post))
        .route("/debug/pprof/trace", get(trace))
        .with_state(shutdown)
}

async fn redirect_to_index() -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::MOVED_PERMANENTLY;
    response
        .headers_mut()
        .insert(header::LOCATION, HeaderValue::from_static("/debug/pprof/"));
    response
}

async fn index() -> Response {
    html_response(StatusCode::OK, INDEX)
}

async fn cmdline() -> Response {
    plain_bytes_response(command_line())
}

async fn profile(
    State(shutdown): State<CancellationToken>,
    Query(query): Query<ProfileQuery>,
) -> Response {
    let seconds = profile_seconds(query.seconds.as_deref());
    if shutdown.is_cancelled() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "CPU profile server is shutting down\n",
        );
    }

    let capture_shutdown = shutdown.clone();
    let mut worker =
        tokio::task::spawn_blocking(move || capture_cpu_profile(seconds, capture_shutdown));
    let result = tokio::select! {
        result = &mut worker => result,
        _ = shutdown.cancelled() => {
            let _ = worker.await;
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "CPU profile capture cancelled\n",
            );
        }
    };

    match result {
        Ok(Ok(profile)) => binary_response(profile),
        Ok(Err(error)) => {
            if shutdown.is_cancelled() {
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "CPU profile capture cancelled\n",
                );
            }
            tracing::warn!(%error, "CPU profile capture failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "CPU profile capture failed\n",
            )
        }
        Err(error) => {
            tracing::warn!(%error, "CPU profile worker failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "CPU profile worker failed\n",
            )
        }
    }
}

async fn symbol_get(uri: Uri) -> Response {
    let query = uri.query().unwrap_or_default();
    text_response(StatusCode::OK, &resolve_symbols(query.as_bytes()))
}

async fn symbol_post(body: axum::body::Bytes) -> Response {
    if body.len() > MAX_SYMBOL_REQUEST_BYTES {
        return error_response(StatusCode::PAYLOAD_TOO_LARGE, "symbol request too large\n");
    }
    let output = tokio::task::spawn_blocking(move || resolve_symbols(&body)).await;
    match output {
        Ok(output) => text_response(StatusCode::OK, &output),
        Err(error) => {
            tracing::warn!(%error, "pprof symbol worker failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "pprof symbol worker failed\n",
            )
        }
    }
}

async fn trace() -> Response {
    error_response(
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
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn html_response(status: StatusCode, body: &str) -> Response {
    let mut response = Response::new(Body::from(body.to_owned()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn plain_bytes_response(body: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn error_response(status: StatusCode, body: &str) -> Response {
    let mut response = text_response(status, body);
    response
        .headers_mut()
        .insert("X-Go-Pprof", HeaderValue::from_static("1"));
    response
}

fn binary_response(body: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"profile\""),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn profile_seconds(raw: Option<&str>) -> u64 {
    raw.and_then(|value| value.parse::<i64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(|seconds| (seconds as u64).min(MAX_PROFILE_SECONDS))
        .unwrap_or(DEFAULT_PROFILE_SECONDS)
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

    let mut output = String::from("num_symbols: 1\n");
    for raw_symbol in body.split(|byte| *byte == b'+') {
        let line = String::from_utf8_lossy(raw_symbol).trim().to_owned();
        let Some(address) = parse_program_counter(&line) else {
            continue;
        };
        if address == 0 {
            continue;
        }

        let mut name = None;
        backtrace::resolve(address as *mut c_void, |symbol| {
            if name.is_none() {
                name = symbol.name().map(|value| value.to_string());
            }
        });
        if let Some(name) = name {
            output.push_str(&format!("{address:#x} {name}\n"));
        }
    }
    output
}

#[cfg(not(unix))]
fn resolve_symbols(body: &[u8]) -> String {
    let _ = body;
    "num_symbols: 1\n".to_owned()
}

#[cfg(unix)]
fn parse_program_counter(value: &str) -> Option<u64> {
    let (digits, radix) = if let Some(value) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        (value, 16)
    } else if let Some(value) = value
        .strip_prefix("0b")
        .or_else(|| value.strip_prefix("0B"))
    {
        (value, 2)
    } else if let Some(value) = value
        .strip_prefix("0o")
        .or_else(|| value.strip_prefix("0O"))
    {
        (value, 8)
    } else if value.len() > 1 && value.starts_with('0') {
        (&value[1..], 8)
    } else {
        (value, 10)
    };
    u64::from_str_radix(digits, radix).ok()
}

#[cfg(unix)]
fn capture_cpu_profile(seconds: u64, shutdown: CancellationToken) -> anyhow::Result<Vec<u8>> {
    use flate2::{write::GzEncoder, Compression};
    use pprof::{protos::Message, ProfilerGuardBuilder};
    use std::io::Write;
    use std::time::{Duration, Instant};

    let guard = ProfilerGuardBuilder::default().frequency(99).build()?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if shutdown.is_cancelled() {
            anyhow::bail!("CPU profile capture cancelled");
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        std::thread::sleep(remaining.min(Duration::from_millis(100)));
    }

    let profile = guard.report().build()?.pprof()?;
    let mut encoded = Vec::new();
    profile.write_to_vec(&mut encoded)?;

    let mut compressed = GzEncoder::new(Vec::new(), Compression::default());
    compressed.write_all(&encoded)?;
    Ok(compressed.finish()?)
}

#[cfg(not(unix))]
fn capture_cpu_profile(_: u64, _: CancellationToken) -> anyhow::Result<Vec<u8>> {
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
    fn symbol_protocol_omits_unresolved_addresses() {
        assert_eq!(resolve_symbols(b"0xnot-an-address+0x0"), "num_symbols: 1\n");
    }

    #[test]
    fn profile_seconds_are_defaulted_and_bounded() {
        assert_eq!(profile_seconds(None), DEFAULT_PROFILE_SECONDS);
        assert_eq!(profile_seconds(Some("")), DEFAULT_PROFILE_SECONDS);
        assert_eq!(profile_seconds(Some("0")), DEFAULT_PROFILE_SECONDS);
        assert_eq!(profile_seconds(Some("-1")), DEFAULT_PROFILE_SECONDS);
        assert_eq!(profile_seconds(Some("15")), 15);
        assert_eq!(profile_seconds(Some("61")), MAX_PROFILE_SECONDS);
        assert_eq!(
            profile_seconds(Some("not-a-number")),
            DEFAULT_PROFILE_SECONDS
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
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/html; charset=utf-8")
        );
        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .and_then(|value| value.to_str().ok()),
            Some("nosniff")
        );
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
    async fn symbol_get_uses_the_pprof_symbol_wire_shape() {
        let response = router()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/debug/pprof/symbol?0xnot-an-address+0x0")
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
        assert_eq!(body.as_ref(), b"num_symbols: 1\n");
    }

    #[test]
    fn profile_response_keeps_the_gzip_payload_without_http_encoding() {
        let response = binary_response(vec![0x1f, 0x8b]);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/octet-stream")
        );
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_DISPOSITION)
                .and_then(|value| value.to_str().ok()),
            Some("attachment; filename=\"profile\"")
        );
        assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
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
