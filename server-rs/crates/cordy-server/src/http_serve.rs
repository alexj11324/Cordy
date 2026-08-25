//! Bounded HTTP accept loop that can abort remaining connection tasks.
//!
//! `axum::serve(...).with_graceful_shutdown` waits for handlers to finish, but
//! dropping that future after a drain timeout leaves the detached Tokio tasks
//! running. This loop owns those tasks in a `JoinSet` so a timed-out drain can
//! abort them before channel/maintenance workers stop.

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::Router;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

/// Serves `app` until `shutdown` is cancelled, then drains in-flight
/// connections for `drain_timeout`. Returns `true` when remaining connection
/// tasks had to be aborted.
pub async fn serve_with_bounded_drain(
    listener: TcpListener,
    app: Router,
    shutdown: CancellationToken,
    drain_timeout: Duration,
) -> std::io::Result<bool> {
    let mut connections = JoinSet::new();

    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => break,
            accepted = listener.accept() => {
                let (stream, addr) = accepted?;
                let app = app.clone();
                connections.spawn(async move {
                    let io = TokioIo::new(stream);
                    let service = service_fn(move |request: Request<Incoming>| {
                        let app = app.clone();
                        async move {
                            let mut request = request.map(Body::new);
                            request.extensions_mut().insert(ConnectInfo(addr));
                            app.oneshot(request).await
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service)
                        .with_upgrades()
                        .await;
                });
            }
            Some(_) = connections.join_next() => {}
        }
    }
    drop(listener);

    let drain_deadline = tokio::time::Instant::now() + drain_timeout;
    let mut timed_out = false;
    loop {
        if connections.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            Some(_) = connections.join_next() => {}
            () = tokio::time::sleep_until(drain_deadline) => {
                timed_out = true;
                connections.abort_all();
                while connections.join_next().await.is_some() {}
                break;
            }
        }
    }
    Ok(timed_out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    #[tokio::test]
    async fn drain_timeout_aborts_in_flight_connection_tasks() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let app = {
            let entered = entered.clone();
            let dropped = dropped.clone();
            Router::new().route(
                "/hang",
                get(move || {
                    let entered = entered.clone();
                    let dropped = dropped.clone();
                    async move {
                        struct DropGuard(Arc<AtomicBool>);
                        impl Drop for DropGuard {
                            fn drop(&mut self) {
                                self.0.store(true, Ordering::SeqCst);
                            }
                        }
                        let _guard = DropGuard(dropped);
                        entered.notify_one();
                        std::future::pending::<()>().await;
                        "ok"
                    }
                }),
            )
        };
        let shutdown = CancellationToken::new();
        let server = tokio::spawn(serve_with_bounded_drain(
            listener,
            app,
            shutdown.clone(),
            Duration::from_millis(50),
        ));

        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(b"GET /hang HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .expect("handler should start");
        shutdown.cancel();

        let timed_out = tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("server should stop after aborting the hung connection")
            .expect("server task")
            .expect("serve");
        assert!(timed_out);
        assert!(dropped.load(Ordering::SeqCst));

        let mut buf = [0_u8; 16];
        let _ = tokio::time::timeout(Duration::from_millis(200), client.read(&mut buf)).await;
    }
}
