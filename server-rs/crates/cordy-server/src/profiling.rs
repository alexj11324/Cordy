//! Loopback-only profiling for the Rust server.
//!
//! CPU and Linux allocation profiles stay on the existing pprof listener.
//! Tokio task/resource/operation telemetry is exported on a second fixed
//! loopback address by the process tracing subscriber.

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderValue, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use hyper::{body::Incoming, server::conn::http1, service::service_fn, Request};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde::Deserialize;
use std::time::Duration;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

pub const ADDR: &str = "127.0.0.1:6060";
pub const TOKIO_CONSOLE_ADDR: &str = "127.0.0.1:6669";
const DEFAULT_PROFILE_SECONDS: u64 = 30;
const PROFILE_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);
const CAPTURE_CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(100);
const INDEX: &str = "<!doctype html><html><head><title>pprof</title></head><body>\
<h1>pprof</h1><ul>\
<li><a href=\"/debug/pprof/profile\">profile</a></li>\
<li><a href=\"/debug/pprof/heap\">heap</a></li>\
<li><a href=\"/debug/pprof/cmdline\">cmdline</a></li>\
<li><a href=\"/debug/pprof/symbol\">symbol</a></li>\
</ul><p>Async runtime diagnostics are continuously exported to the tokio-console \
gRPC endpoint at 127.0.0.1:6669.</p>\
</body></html>\n";

#[derive(Debug, Deserialize)]
struct ProfileQuery {
    seconds: Option<String>,
}

pub async fn serve(shutdown: CancellationToken) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(ADDR).await?;
    tracing::info!(addr = ADDR, "pprof server listening");

    serve_listener(listener, shutdown).await
}

async fn serve_listener(
    listener: tokio::net::TcpListener,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let app = router(shutdown.clone());
    let mut connections = JoinSet::new();

    loop {
        let accepted = tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        let (stream, peer_addr) = accepted?;
        let app = app.clone();
        let connection_shutdown = shutdown.clone();
        connections.spawn(async move {
            if let Err(error) = serve_connection(stream, app, connection_shutdown).await {
                tracing::debug!(%peer_addr, %error, "pprof connection closed with an error");
            }
        });
    }

    // Closing every connection after one response is stricter than the legacy
    // 30-second idle timeout and, unlike a whole-connection timeout, does not
    // interrupt a profile response that legitimately takes longer than 30s.
    shutdown.cancel();
    while let Some(result) = connections.join_next().await {
        if let Err(error) = result {
            tracing::warn!(%error, "pprof connection task failed");
        }
    }
    Ok(())
}

async fn serve_connection(
    stream: tokio::net::TcpStream,
    app: Router,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let service = service_fn(move |request: Request<Incoming>| {
        let app = app.clone();
        async move { app.oneshot(request.map(Body::new)).await }
    });
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(PROFILE_HEADER_READ_TIMEOUT)
        .keep_alive(false);
    let connection = builder.serve_connection(TokioIo::new(stream), service);
    tokio::pin!(connection);

    tokio::select! {
        result = connection.as_mut() => result.map_err(Into::into),
        () = shutdown.cancelled() => {
            connection.as_mut().graceful_shutdown();
            connection.await.map_err(Into::into)
        }
    }
}

fn router(shutdown: CancellationToken) -> Router {
    Router::new()
        .route("/debug/pprof", get(redirect_to_index))
        .route("/debug/pprof/", get(index))
        .route("/debug/pprof/cmdline", get(cmdline))
        .route("/debug/pprof/profile", get(profile))
        .route("/debug/pprof/symbol", get(symbol_get).post(symbol_post))
        .route("/debug/pprof/heap", get(heap_profile))
        .route("/debug/pprof/allocs", get(retired_go_profile))
        .route("/debug/pprof/block", get(retired_go_profile))
        .route("/debug/pprof/goroutine", get(retired_go_profile))
        .route("/debug/pprof/mutex", get(retired_go_profile))
        .route("/debug/pprof/threadcreate", get(retired_go_profile))
        .route("/debug/pprof/trace", get(runtime_trace_replacement))
        .with_state(shutdown)
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
    html_response(StatusCode::OK, INDEX)
}

async fn cmdline() -> Response {
    let mut response = Response::new(Body::from(command_line()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

async fn profile(
    State(shutdown): State<CancellationToken>,
    Query(query): Query<ProfileQuery>,
) -> Response {
    let duration = profile_duration(&query);
    let capture_cancel = CancellationToken::new();
    let _cancel_on_drop = CancelOnDrop(capture_cancel.clone());
    let worker_cancel = capture_cancel.clone();
    let mut worker =
        tokio::task::spawn_blocking(move || capture_cpu_profile(duration, &worker_cancel));
    let result = tokio::select! {
        result = &mut worker => result,
        () = shutdown.cancelled() => {
            capture_cancel.cancel();
            if let Err(error) = worker.await {
                tracing::warn!(%error, "CPU profile worker failed during shutdown");
            }
            return text_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "CPU profile capture cancelled during server shutdown\n",
            );
        }
    };

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

#[cfg(target_os = "linux")]
async fn heap_profile() -> Response {
    let Some(profiler) = jemalloc_pprof::PROF_CTL.as_ref() else {
        return text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "jemalloc heap profiling is unavailable\n",
        );
    };
    let profiler = profiler.clone();
    let output = tokio::task::spawn_blocking(move || {
        // The singleton lock preserves serialized jemalloc dump access while
        // the synchronous dump/parsing work stays off the async executor.
        let mut profiler = profiler.blocking_lock();
        if !profiler.activated() {
            return Ok(None);
        }
        profiler.dump_pprof().map(Some)
    })
    .await;

    match output {
        Ok(Ok(Some(profile))) if !profile.is_empty() => binary_response(profile),
        Ok(Ok(Some(_))) => text_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "heap profile capture returned no data\n",
        ),
        Ok(Ok(None)) => text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "jemalloc heap profiling is not active\n",
        ),
        Ok(Err(error)) => {
            tracing::warn!(%error, "heap profile capture failed");
            text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "heap profile capture failed\n",
            )
        }
        Err(error) => {
            tracing::warn!(%error, "heap profile worker failed");
            text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "heap profile worker failed\n",
            )
        }
    }
}

#[cfg(not(target_os = "linux"))]
async fn heap_profile() -> Response {
    text_response(
        StatusCode::NOT_IMPLEMENTED,
        "allocation-stack heap profiling is available only on Linux\n",
    )
}

async fn runtime_trace_replacement() -> Response {
    text_response(
        StatusCode::GONE,
        "Go runtime trace is retired; the server continuously exports Rust task/resource/operation diagnostics through the tokio-console gRPC endpoint at 127.0.0.1:6669\n",
    )
}

async fn retired_go_profile() -> Response {
    text_response(
        StatusCode::GONE,
        "This Go runtime profile was retired by the Rust server migration; use /debug/pprof/profile, /debug/pprof/heap, or tokio-console as appropriate\n",
    )
}

fn html_response(status: StatusCode, body: &str) -> Response {
    let mut response = Response::new(Body::from(body.to_owned()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
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

fn profile_duration(query: &ProfileQuery) -> Duration {
    let seconds = query
        .seconds
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_PROFILE_SECONDS);
    Duration::from_secs(seconds)
}

struct CancelOnDrop(CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn wait_for_capture_window(
    duration: Duration,
    cancellation: &CancellationToken,
) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    while started.elapsed() < duration {
        if cancellation.is_cancelled() {
            anyhow::bail!("CPU profile capture cancelled");
        }
        let remaining = duration.saturating_sub(started.elapsed());
        std::thread::sleep(remaining.min(CAPTURE_CANCEL_POLL_INTERVAL));
    }
    if cancellation.is_cancelled() {
        anyhow::bail!("CPU profile capture cancelled");
    }
    Ok(())
}

#[cfg(unix)]
fn resolve_symbols(body: &[u8]) -> String {
    use std::ffi::c_void;

    let mut output = String::new();
    for raw_line in body.split(|byte| matches!(*byte, b'\n' | b'+')) {
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
    body.split(|byte| matches!(*byte, b'\n' | b'+'))
        .filter(|line| !line.is_empty())
        .map(|line| format!("{} unknown\n", String::from_utf8_lossy(line).trim()))
        .collect()
}

#[cfg(unix)]
fn capture_cpu_profile(
    duration: Duration,
    cancellation: &CancellationToken,
) -> anyhow::Result<Vec<u8>> {
    use flate2::{write::GzEncoder, Compression};
    use pprof::{protos::Message, ProfilerGuardBuilder};
    use std::io::Write;

    if cancellation.is_cancelled() {
        anyhow::bail!("CPU profile capture cancelled");
    }
    let guard = ProfilerGuardBuilder::default().frequency(99).build()?;
    wait_for_capture_window(duration, cancellation)?;

    let profile = guard.report().build()?.pprof()?;
    let mut encoded = Vec::new();
    profile.write_to_vec(&mut encoded)?;

    let mut compressed = GzEncoder::new(Vec::new(), Compression::default());
    compressed.write_all(&encoded)?;
    Ok(compressed.finish()?)
}

#[cfg(not(unix))]
fn capture_cpu_profile(_: Duration, _: &CancellationToken) -> anyhow::Result<Vec<u8>> {
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
        assert_eq!(TOKIO_CONSOLE_ADDR, "127.0.0.1:6669");
    }

    #[test]
    fn symbol_protocol_returns_unknown_for_unresolved_addresses() {
        assert_eq!(
            resolve_symbols(b"0xnot-an-address\n"),
            "0xnot-an-address unknown\n"
        );
        assert_eq!(
            resolve_symbols(b"0xfirst+0xsecond\n0xthird"),
            "0xfirst unknown\n0xsecond unknown\n0xthird unknown\n"
        );
    }

    #[test]
    fn malformed_and_non_positive_profile_seconds_use_the_go_default() {
        for seconds in [None, Some(""), Some("invalid"), Some("-1"), Some("0")] {
            let query = ProfileQuery {
                seconds: seconds.map(str::to_owned),
            };
            assert_eq!(
                profile_duration(&query),
                Duration::from_secs(DEFAULT_PROFILE_SECONDS),
                "{seconds:?}"
            );
        }

        assert_eq!(
            profile_duration(&ProfileQuery {
                seconds: Some("7".to_owned()),
            }),
            Duration::from_secs(7)
        );
    }

    #[test]
    fn capture_wait_observes_cancellation_during_a_short_window() {
        let cancellation = CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let worker = std::thread::spawn(move || {
            wait_for_capture_window(Duration::from_millis(250), &worker_cancellation)
        });

        std::thread::sleep(Duration::from_millis(20));
        cancellation.cancel();

        assert!(worker.join().unwrap_or_else(|_| unreachable!()).is_err());
    }

    #[test]
    fn dropping_the_capture_guard_cancels_its_worker_token() {
        let cancellation = CancellationToken::new();
        {
            let _guard = CancelOnDrop(cancellation.clone());
            assert!(!cancellation.is_cancelled());
        }
        assert!(cancellation.is_cancelled());
    }

    #[tokio::test]
    async fn index_is_available_only_on_the_profiling_router() {
        let response = router(CancellationToken::new())
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
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/html; charset=utf-8"))
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
    async fn retired_named_go_profiles_return_gone_instead_of_not_found() {
        for name in ["allocs", "block", "goroutine", "mutex", "threadcreate"] {
            let response = router(CancellationToken::new())
                .oneshot(
                    axum::http::Request::builder()
                        .uri(format!("/debug/pprof/{name}"))
                        .body(Body::empty())
                        .unwrap_or_else(|_| unreachable!()),
                )
                .await
                .unwrap_or_else(|_| unreachable!());

            assert_eq!(response.status(), StatusCode::GONE, "{name}");
        }
    }

    #[tokio::test]
    async fn legacy_runtime_trace_points_to_live_rust_diagnostics() {
        let response = router(CancellationToken::new())
            .oneshot(
                axum::http::Request::builder()
                    .uri("/debug/pprof/trace")
                    .body(Body::empty())
                    .unwrap_or_else(|_| unreachable!()),
            )
            .await
            .unwrap_or_else(|_| unreachable!());

        assert_eq!(response.status(), StatusCode::GONE);
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|_| unreachable!())
            .to_bytes();
        assert!(String::from_utf8_lossy(&body).contains(TOKIO_CONSOLE_ADDR));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn heap_endpoint_returns_a_real_gzipped_pprof_profile() {
        let response = router(CancellationToken::new())
            .oneshot(
                axum::http::Request::builder()
                    .uri("/debug/pprof/heap")
                    .body(Body::empty())
                    .unwrap_or_else(|_| unreachable!()),
            )
            .await
            .unwrap_or_else(|_| unreachable!());

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_ENCODING),
            Some(&HeaderValue::from_static("gzip"))
        );
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap_or_else(|_| unreachable!())
            .to_bytes();
        assert!(body.starts_with(&[0x1f, 0x8b]));
    }
}
