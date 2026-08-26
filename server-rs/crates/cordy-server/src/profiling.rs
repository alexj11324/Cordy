//! Loopback pprof server — port of `server/internal/profiling/server.go`.
//!
//! The Rust profiler can emit the same protobuf profile format consumed by
//! pprof tools. Go's execution trace and runtime symbol lookup have no safe
//! equivalent in `pprof-rs`, so those endpoints remain visible but return an
//! explicit 501 instead of pretending to return a profile.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::{body::Body, Router};
use pprof::protos::Message;
use serde::Deserialize;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub const ADDR: &str = "127.0.0.1:6060";

const DEFAULT_PROFILE_DURATION: Duration = Duration::from_secs(30);
const MAX_PROFILE_DURATION: Duration = Duration::from_secs(60);
const PROFILE_FREQUENCY: i32 = 100;
const PROFILE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const PROFILE_CAPTURE_GRACE: Duration = Duration::from_secs(5);
const PROFILE_CANCEL_TIMEOUT: Duration = Duration::from_secs(1);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone)]
struct ProfilingState {
    shutdown: CancellationToken,
    profile_gate: Arc<Semaphore>,
}

#[derive(Debug, Deserialize)]
struct ProfileQuery {
    seconds: Option<String>,
}

impl ProfileQuery {
    fn duration(&self) -> Result<Duration, String> {
        let Some(raw) = self.seconds.as_deref() else {
            return Ok(DEFAULT_PROFILE_DURATION);
        };
        let seconds = raw
            .parse::<i64>()
            .map_err(|_| "seconds must be an integer number of seconds".to_string())?;
        if seconds <= 0 {
            return Ok(DEFAULT_PROFILE_DURATION);
        }
        let duration = Duration::from_secs(seconds as u64);
        if duration > MAX_PROFILE_DURATION {
            return Err(format!(
                "seconds exceeds the safe maximum of {} seconds",
                MAX_PROFILE_DURATION.as_secs()
            ));
        }
        Ok(duration)
    }
}

/// A separately supervised loopback profiling server.
pub struct Runtime {
    shutdown: CancellationToken,
    task: JoinHandle<()>,
}

impl Runtime {
    pub fn spawn() -> Self {
        let shutdown = CancellationToken::new();
        let serve_shutdown = shutdown.clone();
        let task = tokio::spawn(async move {
            if let Err(error) = serve(serve_shutdown).await {
                tracing::error!(%error, "pprof server disabled after startup error");
            }
        });
        Self { shutdown, task }
    }

    pub async fn shutdown(self) {
        if !self.shutdown_with_timeout(SHUTDOWN_TIMEOUT).await {
            tracing::warn!("pprof server forced to shutdown after timeout");
        }
    }

    async fn shutdown_with_timeout(self, timeout: Duration) -> bool {
        self.shutdown.cancel();
        let mut task = self.task;
        match tokio::time::timeout(timeout, &mut task).await {
            Ok(Ok(())) => true,
            Ok(Err(error)) => {
                tracing::error!(%error, "pprof server task panicked during shutdown");
                true
            }
            Err(_) => {
                task.abort();
                let _ = task.await;
                false
            }
        }
    }
}

pub async fn serve(shutdown: CancellationToken) -> anyhow::Result<()> {
    let serve_shutdown = shutdown.clone();
    let state = Arc::new(ProfilingState {
        shutdown,
        profile_gate: Arc::new(Semaphore::new(1)),
    });
    let listener = tokio::net::TcpListener::bind(ADDR).await?;
    tracing::info!(addr = ADDR, "pprof server starting");

    axum::serve(listener, router(state).into_make_service())
        .with_graceful_shutdown(
            // The profiling state owns the same cancellation token as the
            // server, so in-flight captures stop when graceful shutdown
            // begins instead of extending the process lifetime.
            async move { serve_shutdown.cancelled().await },
        )
        .await?;
    Ok(())
}

fn router(state: Arc<ProfilingState>) -> Router {
    Router::new()
        .route("/debug/pprof/", get(index))
        .route("/debug/pprof/cmdline", get(cmdline))
        .route("/debug/pprof/profile", get(profile))
        .route("/debug/pprof/symbol", get(symbol).post(symbol))
        .route("/debug/pprof/trace", get(trace))
        .with_state(state)
}

async fn index() -> Response {
    text_response(
        StatusCode::OK,
        "text/html; charset=utf-8",
        r#"<!doctype html>
<html><head><title>Cordy pprof</title></head><body>
<h1>Cordy pprof</h1>
<ul>
<li><a href="/debug/pprof/profile">profile</a> (protobuf CPU profile)</li>
<li><a href="/debug/pprof/cmdline">cmdline</a></li>
<li><a href="/debug/pprof/symbol">symbol</a> (unsupported)</li>
<li><a href="/debug/pprof/trace">trace</a> (unsupported)</li>
</ul>
</body></html>
"#,
    )
}

async fn cmdline() -> Response {
    let mut body = Vec::new();
    for (index, argument) in std::env::args_os().enumerate() {
        if index > 0 {
            body.push(0);
        }
        body.extend_from_slice(argument.to_string_lossy().as_bytes());
    }
    response(StatusCode::OK, "text/plain; charset=utf-8", body)
}

async fn profile(
    State(state): State<Arc<ProfilingState>>,
    Query(query): Query<ProfileQuery>,
) -> Response {
    let duration = match query.duration() {
        Ok(duration) => duration,
        Err(error) => {
            return text_response(StatusCode::BAD_REQUEST, "text/plain; charset=utf-8", error)
        }
    };

    let permit = match Arc::clone(&state.profile_gate).try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return text_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "text/plain; charset=utf-8",
                "another CPU profile is already in progress\n",
            )
        }
    };

    let capture_cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&capture_cancel);
    let mut task = tokio::task::spawn_blocking(move || capture_profile(duration, worker_cancel));
    let deadline = duration.saturating_add(PROFILE_CAPTURE_GRACE);
    let mut deadline_sleep = Box::pin(tokio::time::sleep(deadline));

    let result = tokio::select! {
        result = &mut task => match result {
            Ok(result) => result,
            Err(error) => Err(format!("profile worker failed: {error}")),
        },
        _ = state.shutdown.cancelled() => {
            capture_cancel.store(true, Ordering::Release);
            match tokio::time::timeout(PROFILE_CANCEL_TIMEOUT, &mut task).await {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => Err(format!("profile worker failed during shutdown: {error}")),
                Err(_) => Err("profile capture cancelled during shutdown\n".to_string()),
            }
        }
        _ = &mut deadline_sleep => {
            capture_cancel.store(true, Ordering::Release);
            match tokio::time::timeout(PROFILE_CANCEL_TIMEOUT, &mut task).await {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => Err(format!("profile worker failed after timeout: {error}")),
                Err(_) => Err("profile capture exceeded its safety timeout\n".to_string()),
            }
        }
    };
    drop(permit);

    match result {
        Ok(body) => response(StatusCode::OK, "application/octet-stream", body),
        Err(error) if state.shutdown.is_cancelled() => text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "text/plain; charset=utf-8",
            error,
        ),
        Err(error) if error.contains("safety timeout") => text_response(
            StatusCode::GATEWAY_TIMEOUT,
            "text/plain; charset=utf-8",
            error,
        ),
        Err(error) => text_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "text/plain; charset=utf-8",
            format!("unable to capture CPU profile: {error}\n"),
        ),
    }
}

async fn symbol() -> Response {
    unsupported("symbol lookup is not available in the Rust pprof server")
}

async fn trace() -> Response {
    unsupported(
        "runtime execution tracing is not available in the Rust pprof server; use /debug/pprof/profile",
    )
}

fn unsupported(message: &str) -> Response {
    text_response(
        StatusCode::NOT_IMPLEMENTED,
        "text/plain; charset=utf-8",
        format!("{message}\n"),
    )
}

fn capture_profile(duration: Duration, cancelled: Arc<AtomicBool>) -> Result<Vec<u8>, String> {
    let guard = pprof::ProfilerGuardBuilder::default()
        .frequency(PROFILE_FREQUENCY)
        .blocklist(&["libc", "libgcc", "pthread", "vdso"])
        .build()
        .map_err(|error| error.to_string())?;

    let started = Instant::now();
    while started.elapsed() < duration {
        if cancelled.load(Ordering::Acquire) {
            return Err("profile capture cancelled".to_string());
        }
        let remaining = duration.saturating_sub(started.elapsed());
        thread::sleep(remaining.min(PROFILE_POLL_INTERVAL));
    }
    if cancelled.load(Ordering::Acquire) {
        return Err("profile capture cancelled".to_string());
    }

    let report = guard.report().build().map_err(|error| error.to_string())?;
    let profile = report.pprof().map_err(|error| error.to_string())?;
    let mut body = Vec::new();
    profile
        .write_to_vec(&mut body)
        .map_err(|error| error.to_string())?;
    Ok(body)
}

fn response(status: StatusCode, content_type: &'static str, body: Vec<u8>) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

fn text_response<B>(status: StatusCode, content_type: &'static str, body: B) -> Response
where
    B: Into<String>,
{
    response(status, content_type, body.into().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_router() -> Router {
        router(Arc::new(ProfilingState {
            shutdown: CancellationToken::new(),
            profile_gate: Arc::new(Semaphore::new(1)),
        }))
    }

    #[test]
    fn profile_duration_is_bounded() {
        assert_eq!(
            ProfileQuery { seconds: None }.duration().unwrap(),
            DEFAULT_PROFILE_DURATION
        );
        assert_eq!(
            ProfileQuery {
                seconds: Some("0".into())
            }
            .duration()
            .unwrap(),
            DEFAULT_PROFILE_DURATION
        );
        assert!(ProfileQuery {
            seconds: Some("61".into())
        }
        .duration()
        .is_err());
    }

    #[tokio::test]
    async fn index_exposes_only_real_or_explicitly_unsupported_endpoints() {
        let response = test_router()
            .oneshot(Request::get("/debug/pprof/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("/debug/pprof/profile"));
        assert!(body.contains("unsupported"));
    }

    #[tokio::test]
    async fn trace_is_an_explicit_safe_downgrade() {
        let response = test_router()
            .oneshot(
                Request::get("/debug/pprof/trace")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8(body.to_vec())
            .unwrap()
            .contains("not available"));
    }

    #[tokio::test]
    async fn symbol_is_an_explicit_safe_downgrade_for_both_methods() {
        for method in ["GET", "POST"] {
            let response = test_router()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri("/debug/pprof/symbol")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        }
    }
}
