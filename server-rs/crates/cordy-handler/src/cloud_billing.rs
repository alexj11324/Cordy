//! Human-only owner-credit billing and workspace subscription proxies.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, Extension, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{middleware, Router};
use cordy_middleware::workspace::WorkspaceContext;
use futures_util::StreamExt;
use ipnetwork::IpNetwork;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::cloud_runtime::{
    CloudRuntimeError, CloudRuntimeProxy, CloudRuntimeRequest, CloudRuntimeResponse,
};
use crate::error::error_response;
use crate::state::HandlerState;

const MAX_BODY_SIZE: usize = 1 << 20;
const MAX_IDEMPOTENCY_KEY_SIZE: usize = 255;

pub fn billing_router(proxy: Arc<dyn CloudRuntimeProxy>) -> Router<HandlerState> {
    Router::new()
        .route("/api/cloud-billing/balance", get(balance))
        .route("/api/cloud-billing/transactions", get(transactions))
        .route("/api/cloud-billing/batches", get(batches))
        .route("/api/cloud-billing/topups", get(topups))
        .route("/api/cloud-billing/price-tiers", get(price_tiers))
        .route(
            "/api/cloud-billing/checkout-sessions",
            post(create_billing_checkout),
        )
        .route(
            "/api/cloud-billing/checkout-sessions/{session_id}",
            get(get_billing_checkout),
        )
        .route(
            "/api/cloud-billing/portal-sessions",
            post(create_billing_portal),
        )
        .layer(Extension(proxy))
}

/// Public Stripe ingress. The fleet verifies Stripe's signature; this edge
/// preserves the exact signed bytes and signature header and never attaches a
/// Cordy user identity.
pub fn stripe_webhook_router(proxy: Arc<dyn CloudRuntimeProxy>) -> Router<HandlerState> {
    stripe_webhook_router_with_limiter(proxy, StripeIpLimiter::from_env())
}

fn stripe_webhook_router_with_limiter(
    proxy: Arc<dyn CloudRuntimeProxy>,
    limiter: StripeIpLimiter,
) -> Router<HandlerState> {
    Router::new()
        .route("/api/webhooks/stripe", post(stripe_webhook))
        .route_layer(middleware::from_fn_with_state(limiter, stripe_ip_limit))
        .layer(Extension(proxy))
}

#[derive(Clone)]
struct StripeIpLimiter {
    hits: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
    limit: usize,
    window: Duration,
    trusted_proxies: Vec<IpNetwork>,
}

impl StripeIpLimiter {
    fn from_env() -> Self {
        Self {
            hits: Arc::new(Mutex::new(HashMap::new())),
            limit: 30,
            window: Duration::from_secs(60),
            trusted_proxies: cordy_middleware::ratelimit::parse_trusted_proxies(
                &std::env::var("RATE_LIMIT_TRUSTED_PROXIES").unwrap_or_default(),
            ),
        }
    }

    #[cfg(test)]
    fn test(limit: usize) -> Self {
        Self {
            hits: Arc::new(Mutex::new(HashMap::new())),
            limit,
            window: Duration::from_secs(60),
            trusted_proxies: Vec::new(),
        }
    }

    async fn allow(&self, ip: &str) -> bool {
        if self.limit == 0 || ip.is_empty() {
            return true;
        }
        let now = Instant::now();
        let cutoff = now - self.window;
        let mut hits = self.hits.lock().await;
        hits.retain(|_, entries| {
            entries.retain(|seen| *seen > cutoff);
            !entries.is_empty()
        });
        let entries = hits.entry(ip.to_string()).or_default();
        if entries.len() >= self.limit {
            return false;
        }
        entries.push(now);
        true
    }
}

fn stripe_client_ip(request: &Request, trusted_proxies: &[IpNetwork]) -> String {
    let remote = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    if remote.is_some_and(|ip| trusted_proxies.iter().any(|network| network.contains(ip))) {
        if let Some(forwarded) = request
            .headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
        {
            for candidate in forwarded.rsplit(',').map(str::trim) {
                if let Ok(ip) = candidate.parse::<IpAddr>() {
                    if !trusted_proxies.iter().any(|network| network.contains(ip)) {
                        return ip.to_string();
                    }
                }
            }
        }
    }
    remote.map(|ip| ip.to_string()).unwrap_or_default()
}

async fn stripe_ip_limit(
    State(limiter): State<StripeIpLimiter>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let ip = stripe_client_ip(&request, &limiter.trusted_proxies);
    if !limiter.allow(&ip).await {
        return error_response(StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded");
    }
    next.run(request).await
}

async fn stripe_webhook(
    Extension(cloud): Extension<Arc<dyn CloudRuntimeProxy>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !cloud.enabled() {
        return unavailable();
    }
    let Some(signature) = headers.get("stripe-signature").cloned() else {
        return error_response(StatusCode::UNAUTHORIZED, "missing Stripe-Signature header");
    };
    let body = match read_raw_body(body).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let mut forwarded = HeaderMap::new();
    forwarded.insert("stripe-signature", signature);
    if let Some(content_type) = headers.get(header::CONTENT_TYPE).cloned() {
        forwarded.insert(header::CONTENT_TYPE, content_type);
    }
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    match cloud
        .execute(CloudRuntimeRequest {
            method: Method::POST,
            path: "/api/v1/webhooks/stripe".into(),
            op: "billing".into(),
            query: None,
            body,
            headers: forwarded,
            user_id: String::new(),
            request_id,
        })
        .await
    {
        Ok(response) => upstream_response(response),
        Err(CloudRuntimeError::Disabled) => unavailable(),
        Err(CloudRuntimeError::InvalidBaseUrl) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "cloud runtime is misconfigured",
        ),
        Err(CloudRuntimeError::Timeout) => error_response(
            StatusCode::GATEWAY_TIMEOUT,
            "cloud runtime request timed out",
        ),
        Err(error) => {
            tracing::warn!(%error, "Stripe webhook proxy failed");
            error_response(StatusCode::BAD_GATEWAY, "cloud runtime request failed")
        }
    }
}

/// Member-readable subscription routes. Mount behind `RequireWorkspaceMember`.
pub fn subscription_member_router(proxy: Arc<dyn CloudRuntimeProxy>) -> Router<HandlerState> {
    Router::new()
        .route("/api/cloud-subscriptions/entitlements", get(entitlements))
        .route("/api/cloud-subscriptions/summary", get(summary))
        .route("/api/cloud-subscriptions/prices", get(prices))
        .layer(Extension(proxy))
}

/// Subscription mutations. Mount behind owner/admin workspace middleware.
pub fn subscription_admin_router(proxy: Arc<dyn CloudRuntimeProxy>) -> Router<HandlerState> {
    Router::new()
        .route(
            "/api/cloud-subscriptions/checkout-sessions",
            post(create_subscription_checkout),
        )
        .route(
            "/api/cloud-subscriptions/seats/reconcile",
            post(reconcile_seats),
        )
        .route(
            "/api/cloud-subscriptions/portal-sessions",
            post(create_subscription_portal),
        )
        .layer(Extension(proxy))
}

macro_rules! billing_get {
    ($name:ident, $path:literal, $query:expr) => {
        async fn $name(
            Extension(proxy): Extension<Arc<dyn CloudRuntimeProxy>>,
            headers: HeaderMap,
            uri: Uri,
        ) -> Response {
            if let Err(response) = require_human(&headers) {
                return response;
            }
            proxy_call(
                proxy,
                &headers,
                Method::GET,
                $path.to_string(),
                if $query {
                    uri.query().map(str::to_string)
                } else {
                    None
                },
                Vec::new(),
                HeaderMap::new(),
                required_user(&headers),
            )
            .await
        }
    };
}

billing_get!(balance, "/api/v1/billing/balance", false);
billing_get!(transactions, "/api/v1/billing/transactions", true);
billing_get!(batches, "/api/v1/billing/batches", true);
billing_get!(topups, "/api/v1/billing/topups", true);
billing_get!(price_tiers, "/api/v1/billing/price-tiers", false);

async fn create_billing_checkout(
    Extension(cloud): Extension<Arc<dyn CloudRuntimeProxy>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if let Err(response) = require_human(&headers) {
        return response;
    }
    if !cloud.enabled() {
        return unavailable();
    }
    let body = match read_json_body(body).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    proxy_call(
        cloud,
        &headers,
        Method::POST,
        "/api/v1/billing/checkout-sessions".to_string(),
        None,
        body,
        HeaderMap::new(),
        required_user(&headers),
    )
    .await
}

async fn get_billing_checkout(
    Extension(cloud): Extension<Arc<dyn CloudRuntimeProxy>>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_human(&headers) {
        return response;
    }
    if session_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "session_id is required");
    }
    if !session_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return error_response(StatusCode::BAD_REQUEST, "invalid session_id");
    }
    proxy_call(
        cloud,
        &headers,
        Method::GET,
        format!("/api/v1/billing/checkout-sessions/{session_id}"),
        None,
        Vec::new(),
        HeaderMap::new(),
        required_user(&headers),
    )
    .await
}

async fn create_billing_portal(
    Extension(cloud): Extension<Arc<dyn CloudRuntimeProxy>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_human(&headers) {
        return response;
    }
    proxy_call(
        cloud,
        &headers,
        Method::POST,
        "/api/v1/billing/portal-sessions".to_string(),
        None,
        Vec::new(),
        HeaderMap::new(),
        required_user(&headers),
    )
    .await
}

macro_rules! subscription_get {
    ($name:ident, $suffix:literal) => {
        async fn $name(
            State(state): State<HandlerState>,
            Extension(cloud): Extension<Arc<dyn CloudRuntimeProxy>>,
            context: Option<Extension<WorkspaceContext>>,
            headers: HeaderMap,
        ) -> Response {
            let (workspace_id, user_id) = match subscription_context(
                &state,
                &headers,
                context.as_ref().map(|value| &value.0),
                false,
            ) {
                Ok(value) => value,
                Err(response) => return response,
            };
            proxy_call(
                cloud,
                &headers,
                Method::GET,
                format!($suffix, workspace_id),
                None,
                Vec::new(),
                HeaderMap::new(),
                Ok(user_id),
            )
            .await
        }
    };
}

subscription_get!(entitlements, "/api/v1/entitlements/{}");
subscription_get!(summary, "/api/v1/subscriptions/{}/summary");
subscription_get!(prices, "/api/v1/subscriptions/{}/prices");

#[derive(Default, Deserialize)]
struct CheckoutInput {
    interval: Option<String>,
    idempotency_key: Option<String>,
    customer_email: Option<String>,
}

#[derive(Serialize)]
struct CheckoutUpstream {
    workspace_id: String,
    interval: String,
    idempotency_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    customer_email: Option<String>,
}

async fn create_subscription_checkout(
    State(state): State<HandlerState>,
    Extension(cloud): Extension<Arc<dyn CloudRuntimeProxy>>,
    context: Option<Extension<WorkspaceContext>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let (workspace_id, user_id) = match subscription_context(
        &state,
        &headers,
        context.as_ref().map(|value| &value.0),
        true,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let body = match read_json_body(body).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let input: CheckoutInput = match serde_json::from_slice(&body) {
        Ok(input) => input,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let interval = input.interval.unwrap_or_default();
    if interval != "month" && interval != "year" {
        return error_response(StatusCode::BAD_REQUEST, "interval must be month or year");
    }
    let idempotency_key = match idempotency_key(&headers, input.idempotency_key.as_deref()) {
        Ok(key) => key,
        Err(response) => return response,
    };
    let upstream = CheckoutUpstream {
        workspace_id,
        interval,
        idempotency_key,
        customer_email: input.customer_email.filter(|value| !value.is_empty()),
    };
    let body = match serde_json::to_vec(&upstream) {
        Ok(body) => body,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to build subscription request",
            )
        }
    };
    proxy_call(
        cloud,
        &headers,
        Method::POST,
        "/api/v1/subscriptions/checkout-sessions".to_string(),
        None,
        body,
        idempotency_headers(&headers),
        Ok(user_id),
    )
    .await
}

async fn reconcile_seats(
    State(state): State<HandlerState>,
    Extension(cloud): Extension<Arc<dyn CloudRuntimeProxy>>,
    context: Option<Extension<WorkspaceContext>>,
    headers: HeaderMap,
) -> Response {
    let (workspace_id, user_id) = match subscription_context(
        &state,
        &headers,
        context.as_ref().map(|value| &value.0),
        true,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    proxy_call(
        cloud,
        &headers,
        Method::POST,
        format!("/api/v1/subscriptions/{workspace_id}/seats/reconcile"),
        None,
        Vec::new(),
        HeaderMap::new(),
        Ok(user_id),
    )
    .await
}

async fn create_subscription_portal(
    State(state): State<HandlerState>,
    Extension(cloud): Extension<Arc<dyn CloudRuntimeProxy>>,
    context: Option<Extension<WorkspaceContext>>,
    headers: HeaderMap,
) -> Response {
    let (workspace_id, user_id) = match subscription_context(
        &state,
        &headers,
        context.as_ref().map(|value| &value.0),
        true,
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = idempotency_key(&headers, None) {
        return response;
    }
    proxy_call(
        cloud,
        &headers,
        Method::POST,
        format!("/api/v1/subscriptions/{workspace_id}/portal-sessions"),
        None,
        Vec::new(),
        idempotency_headers(&headers),
        Ok(user_id),
    )
    .await
}

fn subscription_context(
    state: &HandlerState,
    headers: &HeaderMap,
    context: Option<&WorkspaceContext>,
    manager: bool,
) -> Result<(String, String), Response> {
    require_human(headers)?;
    let enabled = state
        .feature_flags
        .as_deref()
        .is_some_and(cordy_service::feature_flags::billing_workspace_subscriptions_enabled);
    if !enabled {
        return Err(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "workspace subscriptions are not enabled",
        ));
    }
    let Some(context) = context else {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "workspace membership required",
        ));
    };
    let workspace_id = Uuid::parse_str(&context.workspace_id)
        .map_err(|_| {
            error_response(
                StatusCode::BAD_REQUEST,
                "workspace_id or workspace_slug is required",
            )
        })?
        .to_string();
    if manager && context.member.role != "owner" && context.member.role != "admin" {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "insufficient permissions",
        ));
    }
    let user_id = required_user(headers)?;
    Ok((workspace_id, user_id))
}

fn require_human(headers: &HeaderMap) -> Result<(), Response> {
    if matches!(
        headers
            .get("x-actor-source")
            .and_then(|value| value.to_str().ok()),
        Some("task_token" | "cloud_pat")
    ) {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "this endpoint is only available to human actors",
        ));
    }
    Ok(())
}

fn required_user(headers: &HeaderMap) -> Result<String, Response> {
    headers
        .get("x-user-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| error_response(StatusCode::UNAUTHORIZED, "user not authenticated"))
}

fn idempotency_key(headers: &HeaderMap, body_key: Option<&str>) -> Result<String, Response> {
    let key = body_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .ok_or_else(|| {
            error_response(
                StatusCode::BAD_REQUEST,
                "Idempotency-Key or idempotency_key is required",
            )
        })?;
    if key.len() > MAX_IDEMPOTENCY_KEY_SIZE {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "idempotency key must be at most 255 bytes",
        ));
    }
    Ok(key.to_string())
}

fn idempotency_headers(headers: &HeaderMap) -> HeaderMap {
    let mut output = HeaderMap::new();
    if let Some(value) = headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| HeaderValue::from_str(value).ok())
    {
        output.insert("idempotency-key", value);
    }
    output
}

async fn read_json_body(body: Body) -> Result<Vec<u8>, Response> {
    let output = read_raw_body(body).await?;
    if output.iter().all(u8::is_ascii_whitespace) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "request body is required",
        ));
    }
    if serde_json::from_slice::<serde_json::Value>(&output).is_err() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid request body",
        ));
    }
    Ok(output)
}

async fn read_raw_body(body: Body) -> Result<Vec<u8>, Response> {
    let mut stream = body.into_data_stream();
    let mut output = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid request body"))?;
        if output.len().saturating_add(chunk.len()) > MAX_BODY_SIZE {
            return Err(error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body is too large",
            ));
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
async fn proxy_call(
    cloud: Arc<dyn CloudRuntimeProxy>,
    headers: &HeaderMap,
    method: Method,
    path: String,
    query: Option<String>,
    body: Vec<u8>,
    forwarded_headers: HeaderMap,
    user_id: Result<String, Response>,
) -> Response {
    if !cloud.enabled() {
        return unavailable();
    }
    let user_id = match user_id {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    match cloud
        .execute(CloudRuntimeRequest {
            method,
            path,
            op: "billing".into(),
            query,
            body,
            headers: forwarded_headers,
            user_id,
            request_id,
        })
        .await
    {
        Ok(response) => upstream_response(response),
        Err(CloudRuntimeError::Disabled) => unavailable(),
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

fn unavailable() -> Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "cloud runtime is not configured",
    )
}

fn upstream_response(response: CloudRuntimeResponse) -> Response {
    let request_id = response.headers.get("x-request-id").cloned();
    let body = trim_ascii(&response.body);
    let mut output = if body.is_empty() {
        response.status.into_response()
    } else if serde_json::from_slice::<serde_json::Value>(body).is_ok() {
        (
            response.status,
            [(header::CONTENT_TYPE, "application/json")],
            Bytes::copy_from_slice(body),
        )
            .into_response()
    } else {
        (
            response.status,
            axum::Json(serde_json::json!({ "error": String::from_utf8_lossy(body) })),
        )
            .into_response()
    };
    if let Some(request_id) = request_id {
        output.headers_mut().insert("x-request-id", request_id);
    }
    output
}

fn trim_ascii(mut body: &[u8]) -> &[u8] {
    while body.first().is_some_and(u8::is_ascii_whitespace) {
        body = &body[1..];
    }
    while body.last().is_some_and(u8::is_ascii_whitespace) {
        body = &body[..body.len() - 1];
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::http::Request;
    use chrono::Utc;
    use tokio::sync::Mutex;
    use tower::ServiceExt as _;

    #[derive(Default)]
    struct FakeProxy {
        request: Mutex<Option<CloudRuntimeRequest>>,
    }

    #[async_trait]
    impl CloudRuntimeProxy for FakeProxy {
        fn enabled(&self) -> bool {
            true
        }

        async fn execute(
            &self,
            request: CloudRuntimeRequest,
        ) -> Result<CloudRuntimeResponse, CloudRuntimeError> {
            *self.request.lock().await = Some(request);
            Ok(CloudRuntimeResponse {
                status: StatusCode::OK,
                headers: HeaderMap::new(),
                body: Bytes::from_static(br#"{"ok":true}"#),
            })
        }
    }

    struct EnabledFlags;

    impl cordy_service::feature_flags::FlagSource for EnabledFlags {
        fn is_enabled(&self, key: &str, default: bool) -> bool {
            if key == cordy_service::feature_flags::BILLING_WORKSPACE_SUBSCRIPTIONS {
                true
            } else {
                default
            }
        }
    }

    fn test_state(flags: bool) -> HandlerState {
        let state = HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        );
        if flags {
            state.with_feature_flags(Arc::new(EnabledFlags))
        } else {
            state
        }
    }

    fn context(workspace_id: Uuid, user_id: Uuid, role: &str) -> WorkspaceContext {
        WorkspaceContext {
            workspace_id: workspace_id.to_string(),
            member: cordy_db::models::Member {
                id: Uuid::new_v4(),
                workspace_id,
                user_id,
                role: role.to_string(),
                created_at: Utc::now(),
            },
        }
    }

    #[tokio::test]
    async fn billing_list_forwards_query_and_human_identity() {
        let cloud = Arc::new(FakeProxy::default());
        let result = billing_router(cloud.clone())
            .with_state(test_state(false))
            .oneshot(
                Request::get("/api/cloud-billing/transactions?page=2&page_size=50")
                    .header("x-user-id", "user-1")
                    .header("x-request-id", "request-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::OK);
        let request = cloud.request.lock().await.take().unwrap();
        assert_eq!(request.path, "/api/v1/billing/transactions");
        assert_eq!(request.op, "billing");
        assert_eq!(request.query.as_deref(), Some("page=2&page_size=50"));
        assert_eq!(request.user_id, "user-1");
        assert_eq!(request.request_id, "request-1");
    }

    #[tokio::test]
    async fn stripe_webhook_preserves_signed_bytes_and_has_no_user() {
        let cloud = Arc::new(FakeProxy::default());
        let body = b" {\"id\":\"evt_1\"}\n";
        let result = stripe_webhook_router(cloud.clone())
            .with_state(test_state(false))
            .oneshot(
                Request::post("/api/webhooks/stripe")
                    .header("stripe-signature", "t=1,v1=abc")
                    .header("content-type", "application/json; charset=utf-8")
                    .body(Body::from(body.as_slice()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::OK);
        let request = cloud.request.lock().await.take().unwrap();
        assert_eq!(request.body, body);
        assert_eq!(request.op, "billing");
        assert!(request.user_id.is_empty());
        assert_eq!(
            request
                .headers
                .get("stripe-signature")
                .and_then(|value| value.to_str().ok()),
            Some("t=1,v1=abc")
        );
    }

    #[tokio::test]
    async fn stripe_webhook_rejects_missing_signature_before_proxy() {
        let cloud = Arc::new(FakeProxy::default());
        let result = stripe_webhook_router(cloud.clone())
            .with_state(test_state(false))
            .oneshot(
                Request::post("/api/webhooks/stripe")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::UNAUTHORIZED);
        assert!(cloud.request.lock().await.is_none());
    }

    #[tokio::test]
    async fn stripe_webhook_is_rate_limited_per_remote_ip_before_proxy() {
        let cloud = Arc::new(FakeProxy::default());
        let app = stripe_webhook_router_with_limiter(cloud, StripeIpLimiter::test(1))
            .with_state(test_state(false));
        for expected in [StatusCode::OK, StatusCode::TOO_MANY_REQUESTS] {
            let mut request = Request::post("/api/webhooks/stripe")
                .header("stripe-signature", "t=1,v1=abc")
                .body(Body::from("{}"))
                .unwrap();
            request
                .extensions_mut()
                .insert(ConnectInfo(SocketAddr::from(([203, 0, 113, 7], 1234))));
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), expected);
        }
    }

    #[tokio::test]
    async fn stripe_limiter_evicts_expired_client_entries() {
        let limiter = StripeIpLimiter::test(2);
        limiter.hits.lock().await.insert(
            "198.51.100.9".into(),
            vec![Instant::now() - Duration::from_secs(120)],
        );

        assert!(limiter.allow("203.0.113.7").await);

        let hits = limiter.hits.lock().await;
        assert!(!hits.contains_key("198.51.100.9"));
        assert_eq!(hits.len(), 1);
    }

    #[tokio::test]
    async fn subscription_checkout_injects_workspace_and_preserves_idempotency() {
        let cloud = Arc::new(FakeProxy::default());
        let workspace_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let mut request = Request::post("/api/cloud-subscriptions/checkout-sessions")
            .header("x-user-id", user_id.to_string())
            .header("idempotency-key", "header-key")
            .body(Body::from(
                br#"{"workspace_id":"00000000-0000-0000-0000-000000000001","interval":"year","idempotency_key":"body-key","customer_email":"payer@example.com"}"#
                    .as_slice(),
            ))
            .unwrap();
        request
            .extensions_mut()
            .insert(context(workspace_id, user_id, "owner"));
        let result = subscription_admin_router(cloud.clone())
            .with_state(test_state(true))
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::OK);
        let request = cloud.request.lock().await.take().unwrap();
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(body["workspace_id"], workspace_id.to_string());
        assert_eq!(body["idempotency_key"], "body-key");
        assert_eq!(
            request
                .headers
                .get("idempotency-key")
                .and_then(|value| value.to_str().ok()),
            Some("header-key")
        );
    }

    #[test]
    fn stripe_checkout_ids_reject_path_retargeting() {
        for id in ["cs_test/../admin", "cs?inject=1", "cs#frag"] {
            assert!(!id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'));
        }
        assert!("cs_test_abc"
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'));
    }

    #[test]
    fn idempotency_prefers_body_and_caps_bytes() {
        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", "header-key".parse().unwrap());
        assert_eq!(
            idempotency_key(&headers, Some(" body-key ")).unwrap(),
            "body-key"
        );
        headers.insert(
            "idempotency-key",
            "a".repeat(MAX_IDEMPOTENCY_KEY_SIZE + 1).parse().unwrap(),
        );
        assert_eq!(
            idempotency_key(&headers, None).unwrap_err().status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn machine_actor_gate_is_explicit() {
        for source in ["task_token", "cloud_pat"] {
            let mut headers = HeaderMap::new();
            headers.insert("x-actor-source", source.parse().unwrap());
            assert_eq!(
                require_human(&headers).unwrap_err().status(),
                StatusCode::FORBIDDEN
            );
        }
        let mut headers = HeaderMap::new();
        headers.insert("x-actor-source", "future_actor".parse().unwrap());
        assert!(require_human(&headers).is_ok());
    }
}
