//! Authenticated workspace-scoped proxy for the managed cloud runtime fleet.
//!
//! The upstream owns fleet state and node credentials. This boundary only
//! validates the small HTTP envelope, stamps the authenticated user identity,
//! and preserves the upstream status/body contract.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::{Body, Bytes};
use axum::extract::Extension;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures_util::StreamExt;
use url::Url;

use crate::error::error_response;
use crate::state::HandlerState;

const MAX_REQUEST_BODY_SIZE: usize = 1 << 20;
const MAX_RESPONSE_BODY_SIZE: usize = 1 << 20;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(35);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloudRuntimeRequest {
    pub method: Method,
    pub path: &'static str,
    pub query: Option<String>,
    pub body: Vec<u8>,
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
}

impl HttpCloudRuntimeProxy {
    pub fn new(base_url: impl Into<String>, client: reqwest::Client) -> Self {
        Self {
            base_url: base_url.into().trim().trim_end_matches('/').to_string(),
            client,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn from_env() -> Self {
        Self::new(fleet_url_from_env(), reqwest::Client::new())
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

fn fleet_url_from_env() -> String {
    select_fleet_url(
        std::env::var("CORDY_CLOUD_FLEET_URL").ok().as_deref(),
        std::env::var("CORDY_FLEET_URL").ok().as_deref(),
    )
}

fn select_fleet_url(primary: Option<&str>, fallback: Option<&str>) -> String {
    primary
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .or_else(|| fallback.map(str::trim))
        .unwrap_or_default()
        .to_string()
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
        let url = self.target_url(request.path, request.query.as_deref())?;
        let has_body = !request.body.is_empty();
        let mut upstream = self
            .client
            .request(request.method, url)
            .timeout(self.timeout)
            .header(header::ACCEPT, "application/json");
        if has_body {
            upstream = upstream
                .header(header::CONTENT_TYPE, "application/json")
                .body(request.body);
        }
        if !request.user_id.is_empty() {
            upstream = upstream.header("X-User-ID", request.user_id);
        }
        if !request.request_id.is_empty() {
            upstream = upstream.header("X-Request-ID", request.request_id);
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
    let request_id = optional_header(&headers, "x-request-id");
    match proxy
        .execute(CloudRuntimeRequest {
            method,
            path,
            query,
            body,
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
    fn fleet_url_selection_matches_go_environment_precedence() {
        assert_eq!(
            select_fleet_url(
                Some(" https://cloud-fleet.test "),
                Some("https://legacy-fleet.test")
            ),
            "https://cloud-fleet.test"
        );
        assert_eq!(
            select_fleet_url(Some("  "), Some(" https://legacy-fleet.test ")),
            "https://legacy-fleet.test"
        );
        assert_eq!(
            select_fleet_url(None, Some(" https://legacy-fleet.test ")),
            "https://legacy-fleet.test"
        );
        assert_eq!(select_fleet_url(None, Some("  ")), "");
    }

    fn test_state() -> HandlerState {
        HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        )
    }
}
