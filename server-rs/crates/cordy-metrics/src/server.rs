//! Standalone /metrics HTTP server — port of `server/internal/metrics/server.go`.

use std::sync::Arc;

use axum::routing::get;
use prometheus::Encoder;
use tokio_util::sync::CancellationToken;

/// Serves the gathered metric families in Prometheus text format. The Go
/// version's http.Server timeouts (read/write/idle) have no direct axum
/// equivalent; connection hygiene is delegated to the surrounding runtime.
pub async fn serve(
    addr: String,
    registry: Arc<prometheus::Registry>,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let app = axum::Router::new().route(
        "/metrics",
        get(move || async move {
            let mut buf = Vec::new();
            let families = registry.gather();
            let encoder = prometheus::TextEncoder::new();
            match encoder.encode(&families, &mut buf) {
                Ok(()) => (
                    [(
                        axum::http::header::CONTENT_TYPE,
                        "text/plain; version=0.0.4; charset=utf-8",
                    )],
                    String::from_utf8_lossy(&buf).into_owned(),
                ),
                Err(e) => {
                    tracing::error!(error = %e, "/metrics: gather encode failed");
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/plain")],
                        "metrics gather error\n".to_string(),
                    )
                }
            }
        }),
    );

    let bind_addr = normalized_bind_addr(&addr);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(%addr, "metrics server listening");
    Ok(axum::serve(listener, app)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await?)
}

pub fn normalized_bind_addr(addr: &str) -> String {
    if addr.starts_with(':') {
        format!("127.0.0.1{addr}")
    } else {
        addr.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_style_bare_port_is_narrowed_to_loopback() {
        assert_eq!(normalized_bind_addr(":9091"), "127.0.0.1:9091");
        assert_eq!(normalized_bind_addr("127.0.0.1:9091"), "127.0.0.1:9091");
        assert_eq!(normalized_bind_addr("0.0.0.0:9091"), "0.0.0.0:9091");
    }
}
