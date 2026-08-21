//! Standalone /metrics HTTP server — port of `server/internal/metrics/server.go`.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::routing::get;
use prometheus::Encoder;

/// Serves the gathered metric families in Prometheus text format. The Go
/// version's http.Server timeouts (read/write/idle) have no direct axum
/// equivalent; connection hygiene is delegated to the surrounding runtime.
pub async fn serve(addr: SocketAddr, registry: Arc<prometheus::Registry>) -> anyhow::Result<()> {
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

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "metrics server listening");
    Ok(axum::serve(listener, app).await?)
}
