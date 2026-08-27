//! Authenticated workspace-scoped proxy for the managed cloud runtime fleet.
//!
//! The upstream owns fleet state and node credentials. This boundary only
//! validates the small HTTP envelope, stamps the authenticated user identity,
//! and preserves the upstream status/body contract.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::body::{Body, Bytes};
use axum::extract::Extension;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures_util::StreamExt;
use url::Url;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const MAX_REQUEST_BODY_SIZE: usize = 1 << 20;
const MAX_RESPONSE_BODY_SIZE: usize = 1 << 20;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(35);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloudRuntimeRequest {
    pub method: Method,
    pub path: String,
    /// Optional high-level metric operation. An empty value falls back to
    /// path/method inference, while a non-empty value is normalized and used
    /// as the operation bucket (the Go cloudruntime contract).
    pub op: String,
    pub query: Option<String>,
    pub body: Vec<u8>,
    pub headers: HeaderMap,
    pub user_id: String,
    pub request_id: String,
}

#[derive(Clone, Debug)]
pub struct CloudRuntimeResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Debug)]
pub enum CloudRuntimeError {
    Disabled,
    InvalidBaseUrl,
    Timeout,
    ResponseTooLarge,
    Transport(reqwest::Error),
}

impl std::fmt::Display for CloudRuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => formatter.write_str("cloud runtime fleet URL is not configured"),
            Self::InvalidBaseUrl => formatter.write_str("cloud runtime fleet URL is invalid"),
            Self::Timeout => formatter.write_str("cloud runtime request timed out"),
            Self::ResponseTooLarge => write!(
                formatter,
                "cloud runtime response exceeds {MAX_RESPONSE_BODY_SIZE} bytes"
            ),
            Self::Transport(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CloudRuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            _ => None,
        }
    }
}

#[async_trait]
pub trait CloudRuntimeProxy: Send + Sync {
    fn enabled(&self) -> bool;

    async fn execute(
        &self,
        request: CloudRuntimeRequest,
    ) -> Result<CloudRuntimeResponse, CloudRuntimeError>;
}

/// Production HTTP implementation of the cloud-runtime proxy boundary.
#[derive(Clone)]
pub struct HttpCloudRuntimeProxy {
    base_url: String,
    client: reqwest::Client,
    timeout: Duration,
    metrics: Option<Arc<cordy_metrics::BusinessMetrics>>,
}

impl HttpCloudRuntimeProxy {
    pub fn new(base_url: impl Into<String>, client: reqwest::Client) -> Self {
        Self {
            base_url: base_url.into().trim().trim_end_matches('/').to_string(),
            client,
            timeout: DEFAULT_TIMEOUT,
            metrics: None,
        }
    }

    pub fn from_env() -> Self {
        let mut proxy = Self::new(fleet_url_from_env(), reqwest::Client::new());
        proxy.timeout = fleet_timeout_from_env();
        proxy
    }

    pub fn with_metrics(mut self, metrics: Option<Arc<cordy_metrics::BusinessMetrics>>) -> Self {
        self.metrics = metrics;
        self
    }

    #[cfg(test)]
    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn target_url(&self, path: &str, query: Option<&str>) -> Result<Url, CloudRuntimeError> {
        if self.base_url.is_empty() {
            return Err(CloudRuntimeError::Disabled);
        }
        let mut url = Url::parse(&self.base_url).map_err(|_| CloudRuntimeError::InvalidBaseUrl)?;
        if url.scheme().is_empty() || url.host_str().is_none() {
            return Err(CloudRuntimeError::InvalidBaseUrl);
        }
        let base_path = url.path().trim_end_matches('/');
        url.set_path(&format!("{base_path}{path}"));
        url.set_query(query.filter(|value| !value.is_empty()));
        Ok(url)
    }
}

pub(crate) fn fleet_url_from_env() -> String {
    select_fleet_url(
        std::env::var("CORDY_CLOUD_FLEET_URL").ok().as_deref(),
        std::env::var("CORDY_FLEET_URL").ok().as_deref(),
    )
}

fn select_fleet_url(primary: Option<&str>, legacy: Option<&str>) -> String {
    primary
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| legacy.map(str::trim).filter(|value| !value.is_empty()))
        .unwrap_or_default()
        .to_string()
}

fn fleet_timeout_from_env() -> Duration {
    let Ok(raw) = std::env::var("CORDY_CLOUD_FLEET_TIMEOUT") else {
        return DEFAULT_TIMEOUT;
    };
    match parse_go_duration(&raw).filter(|duration| !duration.is_zero()) {
        Some(duration) => duration,
        None => {
            tracing::warn!(
                value = raw,
                default_seconds = DEFAULT_TIMEOUT.as_secs(),
                "invalid CORDY_CLOUD_FLEET_TIMEOUT; using default"
            );
            DEFAULT_TIMEOUT
        }
    }
}

fn parse_go_duration(raw: &str) -> Option<Duration> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with('-') {
        return None;
    }
    if raw == "0" {
        return Some(Duration::ZERO);
    }
    let bytes = raw.as_bytes();
    let mut cursor = 0;
    let mut seconds = 0.0_f64;
    while cursor < bytes.len() {
        let number_start = cursor;
        while cursor < bytes.len() && (bytes[cursor].is_ascii_digit() || bytes[cursor] == b'.') {
            cursor += 1;
        }
        if cursor == number_start {
            return None;
        }
        let value = raw[number_start..cursor].parse::<f64>().ok()?;
        let units = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ];
        let (unit, multiplier) = units
            .into_iter()
            .find(|(unit, _)| raw[cursor..].starts_with(unit))?;
        cursor += unit.len();
        seconds += value * multiplier;
    }
    (seconds.is_finite() && seconds >= 0.0 && seconds < Duration::MAX.as_secs_f64())
        .then(|| Duration::from_secs_f64(seconds))
}

#[async_trait]
impl CloudRuntimeProxy for HttpCloudRuntimeProxy {
    fn enabled(&self) -> bool {
        !self.base_url.is_empty()
    }

    async fn execute(
        &self,
        request: CloudRuntimeRequest,
    ) -> Result<CloudRuntimeResponse, CloudRuntimeError> {
        let operation = infer_cloud_runtime_op(&request.op, &request.method, &request.path);
        let started = Instant::now();
        let result = async {
            let url = self.target_url(&request.path, request.query.as_deref())?;
            let has_body = !request.body.is_empty();
            let mut forwarded_headers = request.headers;
            forwarded_headers
                .entry(header::ACCEPT)
                .or_insert(HeaderValue::from_static("application/json"));
            if has_body {
                forwarded_headers
                    .entry(header::CONTENT_TYPE)
                    .or_insert(HeaderValue::from_static("application/json"));
            }
            if !request.user_id.is_empty() {
                if let Ok(user_id) = HeaderValue::from_str(&request.user_id) {
                    forwarded_headers.insert("x-user-id", user_id);
                }
            }
            if !request.request_id.is_empty() {
                if let Ok(request_id) = HeaderValue::from_str(&request.request_id) {
                    forwarded_headers.insert("x-request-id", request_id);
                }
            }
            let mut upstream = self
                .client
                .request(request.method, url)
                .timeout(self.timeout)
                .headers(forwarded_headers);
            if has_body {
                upstream = upstream.body(request.body);
            }

            let response = upstream.send().await.map_err(|error| {
                if error.is_timeout() {
                    CloudRuntimeError::Timeout
                } else {
                    CloudRuntimeError::Transport(error)
                }
            })?;
            let status = response.status();
            let headers = response.headers().clone();
            let mut stream = response.bytes_stream();
            let mut body = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|error| {
                    if error.is_timeout() {
                        CloudRuntimeError::Timeout
                    } else {
                        CloudRuntimeError::Transport(error)
                    }
                })?;
                if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BODY_SIZE {
                    return Err(CloudRuntimeError::ResponseTooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(CloudRuntimeResponse {
                status,
                headers,
                body: body.into(),
            })
        }
        .await;

        if let Some(metrics) = self.metrics.as_ref() {
            metrics.record_cloud_runtime_request(
                &operation,
                cloud_runtime_status_bucket(&result),
                started.elapsed().as_secs_f64(),
            );
        }
        result
    }
}

fn infer_cloud_runtime_op(op: &str, method: &Method, path: &str) -> String {
    let op = op.trim().to_ascii_lowercase();
    if !op.is_empty() {
        return op;
    }
    match () {
        _ if path.contains("/billing") => "billing".into(),
        _ if path.contains("/gateway") || path.contains("/proxy") || path.contains("/exec") => {
            "gateway".into()
        }
        _ if path.contains("/start") || path.contains("/provision") => "provision".into(),
        _ if path.contains("/stop") || path.contains("/terminate") || path.contains("/reboot") => {
            "terminate".into()
        }
        _ if path.contains("/status") || path.contains("/health") || path.contains("/ready") => {
            "status".into()
        }
        _ if path.contains("/nodes") && *method == Method::POST => "provision".into(),
        _ if path.contains("/nodes") && *method == Method::DELETE => "terminate".into(),
        _ if path.contains("/nodes") => "status".into(),
        _ => "fleet".into(),
    }
}

fn cloud_runtime_status_bucket(
    result: &Result<CloudRuntimeResponse, CloudRuntimeError>,
) -> &'static str {
    match result {
        Ok(response) if response.status.is_success() || response.status.is_redirection() => "ok",
        Ok(response) if response.status.is_client_error() => "4xx",
        Ok(response) if response.status.is_server_error() => "5xx",
        Ok(_) => "error",
        Err(CloudRuntimeError::Timeout) => "timeout",
        Err(_) => "error",
    }
}

/// Mount with an explicitly constructed proxy. Production should pass
/// `Arc::new(HttpCloudRuntimeProxy::from_env())`; tests can supply a recorder.
pub fn router(proxy: Arc<dyn CloudRuntimeProxy>) -> Router<HandlerState> {
    Router::new()
        .route("/api/cloud-runtime", get(get_service))
        .route("/api/cloud-runtime/healthz", get(get_health))
        .route("/api/cloud-runtime/readyz", get(get_ready))
        .route(
            "/api/cloud-runtime/nodes",
            get(list_nodes).post(create_node).delete(delete_node),
        )
        .route("/api/cloud-runtime/nodes/start", post(start_node))
        .route("/api/cloud-runtime/nodes/stop", post(stop_node))
        .route("/api/cloud-runtime/nodes/reboot", post(reboot_node))
        .route("/api/cloud-runtime/nodes/status", post(node_status))
        .route("/api/cloud-runtime/nodes/exec", post(exec_node))
        .layer(Extension(proxy))
}

async fn get_service(
    Extension(proxy): Extension<Arc<dyn CloudRuntimeProxy>>,
    headers: HeaderMap,
) -> Response {
    proxy_request(proxy, headers, None, Method::GET, "/api/v1/", false, true).await
}

async fn get_health(
    Extension(proxy): Extension<Arc<dyn CloudRuntimeProxy>>,
    headers: HeaderMap,
) -> Response {
    proxy_request(proxy, headers, None, Method::GET, "/healthz", false, false).await
}

async fn get_ready(
    Extension(proxy): Extension<Arc<dyn CloudRuntimeProxy>>,
    headers: HeaderMap,
) -> Response {
    proxy_request(proxy, headers, None, Method::GET, "/readyz", false, false).await
}

async fn list_nodes(
    Extension(proxy): Extension<Arc<dyn CloudRuntimeProxy>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    proxy_request(
        proxy,
        headers,
        Some(uri),
        Method::GET,
        "/api/v1/nodes",
        true,
        true,
    )
    .await
}

macro_rules! body_handler {
    ($name:ident, $method:expr, $path:literal) => {
        async fn $name(
            Extension(proxy): Extension<Arc<dyn CloudRuntimeProxy>>,
            headers: HeaderMap,
            body: Body,
        ) -> Response {
            proxy_request_with_body(proxy, headers, $method, $path, body).await
        }
    };
}

body_handler!(create_node, Method::POST, "/api/v1/nodes");
body_handler!(delete_node, Method::DELETE, "/api/v1/nodes");
body_handler!(start_node, Method::POST, "/api/v1/nodes/start");
body_handler!(stop_node, Method::POST, "/api/v1/nodes/stop");
body_handler!(reboot_node, Method::POST, "/api/v1/nodes/reboot");
body_handler!(node_status, Method::POST, "/api/v1/nodes/status");
body_handler!(exec_node, Method::POST, "/api/v1/nodes/exec");

async fn proxy_request_with_body(
    proxy: Arc<dyn CloudRuntimeProxy>,
    headers: HeaderMap,
    method: Method,
    path: &'static str,
    body: Body,
) -> Response {
    if !proxy.enabled() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "cloud runtime is not configured",
        );
    }
    let body = match read_json_body(body).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    execute(proxy, headers, method, path, None, body, true).await
}

async fn proxy_request(
    proxy: Arc<dyn CloudRuntimeProxy>,
    headers: HeaderMap,
    uri: Option<Uri>,
    method: Method,
    path: &'static str,
    with_query: bool,
    with_user_id: bool,
) -> Response {
    if !proxy.enabled() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "cloud runtime is not configured",
        );
    }
    let query = with_query
        .then(|| {
            uri.as_ref()
                .and_then(Uri::query)
                .unwrap_or_default()
                .to_string()
        })
        .filter(|value| !value.is_empty());
    execute(
        proxy,
        headers,
        method,
        path,
        query,
        Vec::new(),
        with_user_id,
    )
    .await
}

async fn execute(
    proxy: Arc<dyn CloudRuntimeProxy>,
    headers: HeaderMap,
    method: Method,
    path: &'static str,
    query: Option<String>,
    body: Vec<u8>,
    with_user_id: bool,
) -> Response {
    let user_id = if with_user_id {
        match required_header(&headers, "x-user-id") {
            Some(value) => value,
            None => return error_response(StatusCode::UNAUTHORIZED, "user not authenticated"),
        }
    } else {
        String::new()
    };
    let request_id =
        required_header(&headers, "x-request-id").unwrap_or_else(|| Uuid::now_v7().to_string());
    match proxy
        .execute(CloudRuntimeRequest {
            method,
            path: path.to_string(),
            op: String::new(),
            query,
            body,
            headers: HeaderMap::new(),
            user_id,
            request_id,
        })
        .await
    {
        Ok(response) => upstream_response(response),
        Err(CloudRuntimeError::Disabled) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "cloud runtime is not configured",
        ),
        Err(CloudRuntimeError::InvalidBaseUrl) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "cloud runtime is misconfigured",
        ),
        Err(CloudRuntimeError::Timeout) => error_response(
            StatusCode::GATEWAY_TIMEOUT,
            "cloud runtime request timed out",
        ),
        Err(error) => {
            tracing::warn!(%error, "cloud runtime request failed");
            error_response(StatusCode::BAD_GATEWAY, "cloud runtime request failed")
        }
    }
}

fn required_header(headers: &HeaderMap, name: &'static str) -> Option<String> {
    let value = optional_header(headers, name);
    (!value.is_empty()).then_some(value)
}

fn optional_header(headers: &HeaderMap, name: &'static str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

async fn read_json_body(body: Body) -> Result<Vec<u8>, Response> {
    let mut stream = body.into_data_stream();
    let mut data = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid request body"))?;
        if data.len().saturating_add(chunk.len()) > MAX_REQUEST_BODY_SIZE {
            return Err(error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body is too large",
            ));
        }
        data.extend_from_slice(&chunk);
    }
    if data.iter().all(u8::is_ascii_whitespace) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "request body is required",
        ));
    }
    if serde_json::from_slice::<serde_json::Value>(&data).is_err() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid request body",
        ));
    }
    Ok(data)
}

fn upstream_response(response: CloudRuntimeResponse) -> Response {
    let request_id = response.headers.get("x-request-id").cloned();
    let body = trim_ascii(&response.body);
    let mut output = if body.is_empty() {
        response.status.into_response()
    } else if serde_json::from_slice::<serde_json::Value>(body).is_ok() {
        (
            response.status,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            Bytes::copy_from_slice(body),
        )
            .into_response()
    } else {
        (
            response.status,
            axum::Json(serde_json::json!({
                "error": String::from_utf8_lossy(body)
            })),
        )
            .into_response()
    };
    if let Some(request_id) = request_id {
        output.headers_mut().insert("x-request-id", request_id);
    }
    output
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt as _;
    use tokio::sync::Mutex;
    use tower::ServiceExt as _;

    #[derive(Default)]
    struct FakeProxy {
        enabled: bool,
        request: Mutex<Option<CloudRuntimeRequest>>,
        response: Mutex<Option<Result<CloudRuntimeResponse, CloudRuntimeError>>>,
    }

    #[async_trait]
    impl CloudRuntimeProxy for FakeProxy {
        fn enabled(&self) -> bool {
            self.enabled
        }

        async fn execute(
            &self,
            request: CloudRuntimeRequest,
        ) -> Result<CloudRuntimeResponse, CloudRuntimeError> {
            *self.request.lock().await = Some(request);
            self.response.lock().await.take().unwrap()
        }
    }

    fn response(status: StatusCode, body: &'static [u8]) -> CloudRuntimeResponse {
        CloudRuntimeResponse {
            status,
            headers: HeaderMap::new(),
            body: Bytes::from_static(body),
        }
    }

    #[tokio::test]
    async fn create_forwards_identity_request_id_and_original_json() {
        let proxy = Arc::new(FakeProxy {
            enabled: true,
            response: Mutex::new(Some(Ok(response(
                StatusCode::CREATED,
                br#"{"status":"launching"}"#,
            )))),
            ..Default::default()
        });
        let app = router(proxy.clone()).with_state(test_state());
        let result = app
            .oneshot(
                Request::post("/api/cloud-runtime/nodes")
                    .header("x-user-id", "01972f7e-7e8d-77ef-a13d-1b0ce3e9c001")
                    .header("x-request-id", "api-request-id")
                    .body(Body::from(br#"{"instance_type":"g5.xlarge"}"#.as_slice()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::CREATED);
        let request = proxy.request.lock().await.take().unwrap();
        assert_eq!(request.method, Method::POST);
        assert_eq!(request.path, "/api/v1/nodes");
        assert_eq!(request.user_id, "01972f7e-7e8d-77ef-a13d-1b0ce3e9c001");
        assert_eq!(request.request_id, "api-request-id");
        assert_eq!(request.body, br#"{"instance_type":"g5.xlarge"}"#);
    }

    #[tokio::test]
    async fn list_forwards_query_without_reencoding() {
        let proxy = Arc::new(FakeProxy {
            enabled: true,
            response: Mutex::new(Some(Ok(response(StatusCode::OK, b"[]")))),
            ..Default::default()
        });
        let app = router(proxy.clone()).with_state(test_state());
        let result = app
            .oneshot(
                Request::get("/api/cloud-runtime/nodes?limit=10&filter=a%2Fb")
                    .header("x-user-id", "user-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::OK);
        assert_eq!(
            proxy
                .request
                .lock()
                .await
                .as_ref()
                .unwrap()
                .query
                .as_deref(),
            Some("limit=10&filter=a%2Fb")
        );
        let request_id = proxy
            .request
            .lock()
            .await
            .as_ref()
            .unwrap()
            .request_id
            .clone();
        assert!(Uuid::parse_str(&request_id).is_ok());
    }

    #[tokio::test]
    async fn health_needs_no_user_and_wraps_non_json() {
        let proxy = Arc::new(FakeProxy {
            enabled: true,
            response: Mutex::new(Some(Ok(response(
                StatusCode::BAD_GATEWAY,
                b"fleet failed\n",
            )))),
            ..Default::default()
        });
        let result = router(proxy)
            .with_state(test_state())
            .oneshot(
                Request::get("/api/cloud-runtime/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            result.into_body().collect().await.unwrap().to_bytes(),
            br#"{"error":"fleet failed"}"#.as_slice()
        );
    }

    #[tokio::test]
    async fn disabled_proxy_wins_before_body_validation() {
        let proxy = Arc::new(FakeProxy::default());
        let result = router(proxy)
            .with_state(test_state())
            .oneshot(
                Request::post("/api/cloud-runtime/nodes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn body_validation_matches_go_boundary() {
        for (body, status) in [
            (Vec::new(), StatusCode::BAD_REQUEST),
            (b"not-json".to_vec(), StatusCode::BAD_REQUEST),
            (
                vec![b'a'; MAX_REQUEST_BODY_SIZE + 1],
                StatusCode::PAYLOAD_TOO_LARGE,
            ),
        ] {
            let proxy = Arc::new(FakeProxy {
                enabled: true,
                ..Default::default()
            });
            let result = router(proxy)
                .with_state(test_state())
                .oneshot(
                    Request::post("/api/cloud-runtime/nodes")
                        .header("x-user-id", "user-1")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(result.status(), status);
        }
    }

    #[test]
    fn production_url_preserves_base_path_and_query() {
        let proxy = HttpCloudRuntimeProxy::new("https://fleet.test/base/", reqwest::Client::new())
            .with_timeout(Duration::from_millis(10));
        let url = proxy
            .target_url("/api/v1/nodes", Some("limit=20&offset=0"))
            .unwrap();
        assert_eq!(
            url.as_str(),
            "https://fleet.test/base/api/v1/nodes?limit=20&offset=0"
        );
    }

    #[test]
    fn fleet_url_and_timeout_compatibility_helpers_match_go() {
        assert_eq!(
            select_fleet_url(Some(" https://cloud.test/ "), Some("https://legacy.test")),
            "https://cloud.test/"
        );
        assert_eq!(
            select_fleet_url(Some("  "), Some(" https://legacy.test ")),
            "https://legacy.test"
        );
        assert_eq!(parse_go_duration("1m30s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_go_duration("250ms"), Some(Duration::from_millis(250)));
        assert!(parse_go_duration("forever").is_none());
    }

    #[test]
    fn metrics_labels_cover_success_and_failure_paths() {
        assert_eq!(
            infer_cloud_runtime_op("", &Method::POST, "/api/v1/nodes"),
            "provision"
        );
        assert_eq!(
            infer_cloud_runtime_op("", &Method::POST, "/api/v1/nodes/exec"),
            "gateway"
        );
        assert_eq!(
            infer_cloud_runtime_op(" Billing ", &Method::POST, "/api/v1/webhooks/stripe"),
            "billing"
        );
        assert_eq!(
            cloud_runtime_status_bucket(&Ok(response(StatusCode::CREATED, b"{}"))),
            "ok"
        );
        assert_eq!(
            cloud_runtime_status_bucket(&Err(CloudRuntimeError::Timeout)),
            "timeout"
        );
    }

    fn test_state() -> HandlerState {
        HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        )
    }
}
