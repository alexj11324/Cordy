//! Loopback-only profiling for the Rust server.
//!
//! CPU and Linux allocation profiles stay on the existing pprof listener.
//! Tokio task/resource/operation telemetry is exported on a second fixed
//! loopback address by the process tracing subscriber.

use axum::{
    body::Body,
    extract::Query,
    http::{header, HeaderValue, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;

pub const ADDR: &str = "127.0.0.1:6060";
pub const TOKIO_CONSOLE_ADDR: &str = "127.0.0.1:6669";
pub const TOKIO_CONSOLE_ENV: &str = "CORDY_TOKIO_CONSOLE";
const TOKIO_CONSOLE_RETENTION: std::time::Duration = std::time::Duration::from_secs(60);
const TOKIO_CONSOLE_EVENT_BUFFER_CAPACITY: usize = 1024;
const DEFAULT_PROFILE_SECONDS: u64 = 30;
const INDEX: &str = "<!doctype html><html><head><title>pprof</title></head><body>\
<h1>pprof</h1><ul>\
<li><a href=\"/debug/pprof/profile\">profile</a></li>\
<li><a href=\"/debug/pprof/heap\">heap</a></li>\
<li><a href=\"/debug/pprof/cmdline\">cmdline</a></li>\
<li><a href=\"/debug/pprof/symbol\">symbol</a></li>\
</ul><p>Async runtime diagnostics (when CORDY_TOKIO_CONSOLE=1): tokio-console http://127.0.0.1:6669</p>\
</body></html>\n";

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

pub fn console_enabled(raw: Option<&str>) -> bool {
    raw == Some("1")
}

pub fn build_console() -> (console_subscriber::ConsoleLayer, console_subscriber::Server) {
    let addr = TOKIO_CONSOLE_ADDR
        .parse::<std::net::SocketAddr>()
        .unwrap_or_else(|_| unreachable!("fixed Tokio console address must parse"));
    console_subscriber::ConsoleLayer::builder()
        .server_addr(addr)
        .retention(TOKIO_CONSOLE_RETENTION)
        .event_buffer_capacity(TOKIO_CONSOLE_EVENT_BUFFER_CAPACITY)
        .build()
}

pub async fn bind_console() -> std::io::Result<TcpListener> {
    bind_console_at(
        TOKIO_CONSOLE_ADDR
            .parse()
            .unwrap_or_else(|_| unreachable!("fixed Tokio console address must parse")),
    )
    .await
}

async fn bind_console_at(addr: std::net::SocketAddr) -> std::io::Result<TcpListener> {
    TcpListener::bind(addr).await
}

pub async fn serve_console(
    console: console_subscriber::Server,
    listener: TcpListener,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let console_subscriber::ServerParts {
        instrument_server,
        aggregator,
        ..
    } = console.into_parts();
    let aggregate = tokio::spawn(aggregator.run());
    let result = Server::builder()
        .add_service(instrument_server)
        .serve_with_incoming_shutdown(
            tokio_stream::wrappers::TcpListenerStream::new(listener),
            shutdown.cancelled_owned(),
        )
        .await;
    aggregate.abort();
    let _ = aggregate.await;
    result.map_err(Into::into)
}

fn router() -> Router {
    Router::new()
        .route("/debug/pprof", get(redirect_to_index))
        .route("/debug/pprof/", get(index))
        .route("/debug/pprof/cmdline", get(cmdline))
        .route("/debug/pprof/profile", get(profile))
        .route("/debug/pprof/symbol", get(symbol_get).post(symbol_post))
        .route("/debug/pprof/heap", get(heap_profile))
        .route("/debug/pprof/trace", get(runtime_trace_replacement))
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

#[cfg(target_os = "linux")]
async fn heap_profile() -> Response {
    enum Capture {
        Unavailable,
        Inactive,
        Profile(Vec<u8>),
        Empty,
        Failed(anyhow::Error),
    }

    let capture = tokio::task::spawn_blocking(|| {
        let Some(profiler) = jemalloc_pprof::PROF_CTL.as_ref() else {
            return Capture::Unavailable;
        };
        let mut profiler = profiler.blocking_lock();
        if !profiler.activated() {
            return Capture::Inactive;
        }
        match profiler.dump_pprof() {
            Ok(profile) if !profile.is_empty() => Capture::Profile(profile),
            Ok(_) => Capture::Empty,
            Err(error) => Capture::Failed(error),
        }
    })
    .await;

    match capture {
        Ok(Capture::Unavailable) => text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "jemalloc heap profiling is unavailable\n",
        ),
        Ok(Capture::Inactive) => text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "jemalloc heap profiling is not active\n",
        ),
        Ok(Capture::Profile(profile)) => binary_response(profile),
        Ok(Capture::Empty) => text_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "heap profile capture returned no data\n",
        ),
        Ok(Capture::Failed(error)) => {
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
        "Go runtime trace is retired; set CORDY_TOKIO_CONSOLE=1 and use tokio-console http://127.0.0.1:6669 for Rust task/resource/operation diagnostics\n",
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
        assert_eq!(TOKIO_CONSOLE_ADDR, "127.0.0.1:6669");
    }

    #[test]
    fn tokio_console_is_explicitly_opt_in() {
        assert!(!console_enabled(None));
        assert!(!console_enabled(Some("true")));
        assert!(!console_enabled(Some(" 1 ")));
        assert!(console_enabled(Some("1")));
    }

    #[tokio::test]
    async fn occupied_console_address_is_rejected_before_startup() {
        let occupied = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|_| unreachable!());
        let address = occupied.local_addr().unwrap_or_else(|_| unreachable!());
        let error = bind_console_at(address)
            .await
            .expect_err("occupied address must fail closed");
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
    }

    #[tokio::test]
    async fn console_server_joins_after_cancellation() {
        let (_, server) = build_console();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|_| unreachable!());
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(serve_console(server, listener, shutdown.clone()));
        tokio::task::yield_now().await;
        shutdown.cancel();

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .expect("console shutdown must be bounded")
            .expect("console task must join");
        assert!(result.is_ok());
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
    async fn legacy_runtime_trace_points_to_live_rust_diagnostics() {
        let response = router()
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
        use flate2::read::GzDecoder;
        use pprof::protos::Message;
        use std::io::Read;

        let response = router()
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
        let mut protobuf = Vec::new();
        GzDecoder::new(body.as_ref())
            .read_to_end(&mut protobuf)
            .expect("heap profile must be valid gzip");
        let profile = pprof::protos::Profile::parse_from_bytes(&protobuf)
            .expect("heap profile must be a valid pprof protobuf");
        assert!(!profile.string_table.is_empty());
    }
}
