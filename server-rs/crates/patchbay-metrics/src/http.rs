//! HTTP instrumentation.
//!
//! The Go version is chi middleware; the Rust port is a tower/axum middleware
//! that records request count, duration, and in-flight gauges. Route labels
//! come from the matched route pattern (`MatchedPath`), falling back to
//! "unmatched" exactly like chi's RoutePattern.

use std::time::Instant;

use axum::body::HttpBody;
use axum::extract::MatchedPath;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use prometheus::{CounterVec, Gauge, HistogramOpts, HistogramVec, Opts};

const DAEMON_WORKSPACE_ROUTE_PATTERN: &str = "/api/daemon/workspaces";

pub struct HttpMetrics {
    requests: CounterVec,
    duration: HistogramVec,
    daemon_workspace_response_size: HistogramVec,
    in_flight: Gauge,
}

impl HttpMetrics {
    pub fn new() -> Self {
        Self {
            requests: CounterVec::new(
                Opts::new(
                    "patchbay_http_requests_total",
                    "Total HTTP requests served by the API server.",
                ),
                &["method", "route", "status"],
            )
            .expect("valid counter vec"),
            duration: HistogramVec::new(
                HistogramOpts::new(
                    "patchbay_http_request_duration_seconds",
                    "HTTP request duration observed by the API server.",
                )
                .buckets(vec![
                    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
                ]),
                &["method", "route", "status"],
            )
            .expect("valid histogram vec"),
            daemon_workspace_response_size: HistogramVec::new(
                HistogramOpts::new(
                    "patchbay_http_daemon_workspace_response_size_bytes",
                    "Response bytes written by the daemon workspace-set endpoint.",
                )
                .buckets(prometheus::exponential_buckets(128.0, 4.0, 9).expect("valid buckets")),
                &["status"],
            )
            .expect("valid histogram vec"),
            in_flight: Gauge::new(
                "patchbay_http_in_flight_requests",
                "Current number of in-flight HTTP requests served by the API server.",
            )
            .expect("valid gauge"),
        }
    }

    pub fn collectors(&self) -> Vec<Box<dyn prometheus::core::Collector>> {
        vec![
            Box::new(self.requests.clone()),
            Box::new(self.duration.clone()),
            Box::new(self.daemon_workspace_response_size.clone()),
            Box::new(self.in_flight.clone()),
        ]
    }

    /// Records one served request. Called from the axum middleware below;
    /// split out as a plain function so tests can drive it directly.
    pub fn record(
        &self,
        method: &str,
        route: &str,
        status: u16,
        bytes_written: usize,
        elapsed_secs: f64,
    ) {
        let status_label = status.to_string();
        let labels = [method, route, status_label.as_str()];
        self.requests.with_label_values(&labels).inc();
        self.duration
            .with_label_values(&labels)
            .observe(elapsed_secs);
        if route == DAEMON_WORKSPACE_ROUTE_PATTERN {
            self.daemon_workspace_response_size
                .with_label_values(&[status_label.as_str()])
                .observe(bytes_written as f64);
        }
    }

    pub fn inc_in_flight(&self) {
        self.in_flight.inc();
    }

    pub fn dec_in_flight(&self) {
        self.in_flight.dec();
    }
}

impl Default for HttpMetrics {
    fn default() -> Self {
        Self::new()
    }
}

pub fn is_health_probe_path(path: &str) -> bool {
    matches!(path, "/health" | "/healthz" | "/readyz")
}

/// Axum middleware wrapping [`HttpMetrics`]. Health probes bypass
/// instrumentation so scrape traffic does not pollute the request series.
pub async fn middleware(
    axum::extract::State(metrics): axum::extract::State<std::sync::Arc<HttpMetrics>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if is_health_probe_path(req.uri().path()) {
        return next.run(req).await;
    }

    metrics.inc_in_flight();
    let start = Instant::now();
    let method = req.method().to_string();
    // MatchedPath resolves before the handler runs; reading it afterwards
    // yields None because the task-local route context is consumed.
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "unmatched".to_string());

    let res = next.run(req).await;

    // axum Body has no cheap exact size; the Go version counted bytes
    // written by the wrapper. Use the body size hint when known, else 0 —
    // the daemon workspace histogram tolerates the approximation.
    let bytes = res.body().size_hint().exact().unwrap_or(0) as usize;
    metrics.record(
        &method,
        &route,
        res.status().as_u16(),
        bytes,
        start.elapsed().as_secs_f64(),
    );
    metrics.dec_in_flight();
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_probe_paths_match_go_set() {
        assert!(is_health_probe_path("/health"));
        assert!(is_health_probe_path("/healthz"));
        assert!(is_health_probe_path("/readyz"));
        assert!(!is_health_probe_path("/api/issues"));
        assert!(!is_health_probe_path("/healthz/extra"));
    }

    #[test]
    fn record_counts_and_observes_with_labels() {
        let m = HttpMetrics::new();
        m.record("GET", "/api/issues", 200, 0, 0.01);
        m.record("GET", "/api/issues", 200, 0, 0.02);
        m.record("POST", "/api/issues", 500, 0, 0.3);
        assert_eq!(
            m.requests
                .with_label_values(&["GET", "/api/issues", "200"])
                .get(),
            2.0
        );
        assert_eq!(
            m.requests
                .with_label_values(&["POST", "/api/issues", "500"])
                .get(),
            1.0
        );
        assert_eq!(
            m.duration
                .with_label_values(&["GET", "/api/issues", "200"])
                .get_sample_count(),
            2
        );
    }

    #[test]
    fn daemon_workspace_route_also_records_response_size() {
        let m = HttpMetrics::new();
        m.record("PUT", DAEMON_WORKSPACE_ROUTE_PATTERN, 204, 4096, 0.05);
        m.record("PUT", "/api/other", 204, 4096, 0.05);
        assert_eq!(
            m.daemon_workspace_response_size
                .with_label_values(&["204"])
                .get_sample_count(),
            1,
            "only the daemon workspace endpoint records response size"
        );
    }

    #[test]
    fn in_flight_gauge_moves_up_and_down() {
        let m = HttpMetrics::new();
        m.inc_in_flight();
        m.inc_in_flight();
        assert_eq!(m.in_flight.get(), 2.0);
        m.dec_in_flight();
        assert_eq!(m.in_flight.get(), 1.0);
    }
}
