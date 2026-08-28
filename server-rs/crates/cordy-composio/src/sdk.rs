//! A small, standalone SDK for the Composio v3.1 REST API.
//!
//! Covers authentication configs, connected accounts, sessions, toolkits, and
//! webhooks. Tool execution is intentionally omitted because no caller uses it.
//!
//! Wire notes: every non-2xx response surfaces as [`ApiError`] parsed from
//! the upstream `{"error": {...}}` envelope; transport failures are plain
//! reqwest errors wrapped in the request context.

use std::time::Duration;

use hmac::Mac as _;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The canonical Composio v3.1 REST root.
pub const DEFAULT_BASE_URL: &str = "https://backend.composio.dev/api/v3.1";

/// Sent on every request unless overridden via [`ClientBuilder::user_agent`].
pub const DEFAULT_USER_AGENT: &str = "cordy-composio-rs/0.1";

/// Per-request timeout applied when no explicit timeout is set.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

// ── errors ───────────────────────────────────────────────────────────────

/// All non-2xx responses come back as this error carrying the upstream
/// status, slug, and message. Field names match the upstream JSON envelope
/// (`{"error": {...}}`), so the parse mirrors Go's `parseAPIError`.
#[derive(Debug, Clone, Default, PartialEq, Error, serde::Serialize, serde::Deserialize)]
#[error("composio: {http_status} {message}")]
pub struct ApiError {
    pub http_status: u16,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub code: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub slug: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub status: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub suggested_fix: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    /// Raw body bytes retained for callers that need the exact wire shape.
    #[serde(skip)]
    pub raw_body: Vec<u8>,
}

fn is_zero(v: &i64) -> bool {
    *v == 0
}

impl ApiError {
    pub fn is_not_found(&self) -> bool {
        self.http_status == 404
    }

    pub fn is_unauthorized(&self) -> bool {
        self.http_status == 401
    }

    pub fn is_rate_limited(&self) -> bool {
        self.http_status == 429
    }
}

/// Parses an upstream error body into an [`ApiError`]. A body that is not
/// the expected envelope keeps the raw body with an empty message — the
/// same degradation Go's parser applies.
pub fn parse_api_error(status: u16, body: &[u8]) -> ApiError {
    let mut out = ApiError {
        http_status: status,
        ..Default::default()
    };
    out.raw_body = body.to_vec();
    if body.is_empty() {
        return out;
    }
    #[derive(Deserialize)]
    struct Wire {
        #[serde(default)]
        error: ApiErrorWireBody,
    }
    #[derive(Deserialize, Default)]
    struct ApiErrorWireBody {
        #[serde(default)]
        message: String,
        #[serde(default)]
        code: i64,
        #[serde(default)]
        slug: String,
        #[serde(default)]
        status: i64,
        #[serde(default, rename = "request_id")]
        _request_id: String,
        #[serde(default, rename = "suggested_fix")]
        _suggested_fix: String,
        #[serde(default)]
        errors: Vec<String>,
    }
    let Ok(wire) = serde_json::from_slice::<Wire>(body) else {
        // Body is not the expected envelope — leave RawBody set, message empty.
        return out;
    };
    out.message = wire.error.message;
    out.code = wire.error.code;
    out.slug = wire.error.slug;
    out.status = wire.error.status;
    out.errors = wire.error.errors;
    out
}

// ── shared types ─────────────────────────────────────────────────────────

/// Mirrors a subset of a Composio auth config — the project-level record
/// that defines HOW users authenticate with a toolkit. The connect-link
/// flow needs its opaque `id` (ac_…); the other fields drive selection when
/// a toolkit has more than one config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Carries at least the slug (and a logo) the config belongs to.
    #[serde(default)]
    pub toolkit: Toolkit,
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        rename = "auth_scheme"
    )]
    pub auth_scheme: String,
    /// True for Composio's managed OAuth app; false for a custom
    /// (bring-your-own client_id/secret) config — the white-label case.
    #[serde(default, rename = "is_composio_managed")]
    pub is_composio_managed: bool,
    /// "ENABLED" or "DISABLED". The list endpoint hides disabled configs by
    /// default (show_disabled=false).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        rename = "created_at"
    )]
    pub created_at: String,
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        rename = "last_updated_at"
    )]
    pub last_updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Toolkit {
    pub slug: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty", rename = "logo")]
    pub logo_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        rename = "auth_schemes"
    )]
    pub auth_schemes: Vec<String>,
}

// ── connect links ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateLinkRequest {
    /// The `ac_…` id of an auth config registered in your Composio project.
    #[serde(rename = "auth_config_id")]
    pub auth_config_id: String,
    /// Your own user identifier — Composio scopes the resulting connected
    /// account by it.
    #[serde(rename = "user_id")]
    pub user_id: String,
    /// Where Composio sends the user after the hosted auth flow. Optional;
    /// Composio has a default landing page.
    #[serde(rename = "callback_url", skip_serializing_if = "String::is_empty")]
    pub callback_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub alias: String,
    #[serde(rename = "connection_data", skip_serializing_if = "Option::is_none")]
    pub connection_data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreateLinkResponse {
    #[serde(default, rename = "link_token")]
    pub link_token: String,
    #[serde(default, rename = "redirect_url")]
    pub redirect_url: String,
    #[serde(default, rename = "expires_at")]
    pub expires_at: String,
    #[serde(default, rename = "connected_account_id")]
    pub connected_account_id: String,
}

// ── connected accounts ───────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ListConnectedAccountsRequest {
    pub user_ids: Vec<String>,
    pub toolkit_slugs: Vec<String>,
    pub auth_config_ids: Vec<String>,
    pub connected_account_ids: Vec<String>,
    /// ACTIVE, EXPIRED, INACTIVE, …
    pub statuses: Vec<String>,
    /// "created_at" (default) | "updated_at"
    pub order_by: String,
    /// "asc" | "desc" (default)
    pub order_direction: String,
    /// Experimental: PRIVATE | SHARED | ALL
    pub account_type: String,
    /// 0 = use upstream default
    pub limit: u32,
    pub cursor: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConnectedAccount {
    pub id: String,
    #[serde(default, rename = "user_id")]
    pub user_id: String,
    #[serde(default, rename = "auth_config_id")]
    pub auth_config_id: String,
    #[serde(default, rename = "auth_config")]
    pub auth_config: AuthConfigRef,
    #[serde(default)]
    pub toolkit: Toolkit,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuthConfigRef {
    pub id: String,
    #[serde(default, rename = "auth_scheme")]
    pub auth_scheme: String,
    #[serde(default, rename = "is_composio_managed")]
    pub is_composio_managed: bool,
    #[serde(default, rename = "is_disabled")]
    pub is_disabled: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListConnectedAccountsResponse {
    #[serde(default)]
    pub items: Vec<ConnectedAccount>,
    #[serde(default, rename = "next_cursor")]
    pub next_cursor: String,
    #[serde(default, rename = "total_items")]
    pub total_items: i64,
}

// ── sessions ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateSessionRequest {
    #[serde(rename = "user_id")]
    pub user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toolkits: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "connected_accounts")]
    pub connected_accounts: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpDescriptor {
    #[serde(default, rename = "type")]
    pub r#type: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreateSessionResponse {
    #[serde(default, rename = "session_id")]
    pub session_id: String,
    #[serde(default)]
    pub mcp: McpDescriptor,
}

// ── toolkits ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ListToolkitsRequest {
    pub category: String,
    pub limit: u32,
    pub cursor: String,
    /// Upstream sort order: "usage" | "alphabetically".
    pub sort_by: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListToolkitsResponse {
    #[serde(default)]
    pub items: Vec<Toolkit>,
    #[serde(default, rename = "next_cursor")]
    pub next_cursor: String,
    #[serde(default, rename = "total_items")]
    pub total_items: i64,
}

// ── auth configs list ────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ListAuthConfigsRequest {
    /// Filters to specific toolkits; sent as comma-separated `toolkit_slug`
    /// query param per the v3 spec.
    pub toolkit_slugs: Vec<String>,
    pub show_disabled: bool,
    pub search: String,
    /// Page size (max 1000 upstream). 0 = upstream default.
    pub limit: u32,
    pub cursor: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListAuthConfigsResponse {
    #[serde(default)]
    pub items: Vec<AuthConfig>,
    #[serde(default, rename = "next_cursor")]
    pub next_cursor: String,
    #[serde(default, rename = "total_items")]
    pub total_items: i64,
}

// ── client ───────────────────────────────────────────────────────────────

/// The Composio REST client. Cheap to clone conceptually; safe for
/// concurrent use (reqwest clients share a pool).
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

/// Builder mirroring Go's `Options`.
pub struct ClientBuilder {
    api_key: String,
    base_url: String,
    user_agent: String,
    timeout: Option<Duration>,
}

impl ClientBuilder {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_string(),
            user_agent: DEFAULT_USER_AGENT.to_string(),
            timeout: None,
        }
    }

    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = ua.into();
        self
    }

    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = Some(t);
        self
    }

    pub fn build(self) -> anyhow::Result<Client> {
        if self.api_key.trim().is_empty() {
            anyhow::bail!("composio: APIKey is required");
        }
        let base_url = self.base_url.trim_end_matches('/').to_string();
        let mut builder = reqwest::Client::builder()
            .timeout(self.timeout.unwrap_or(DEFAULT_TIMEOUT))
            .user_agent(self.user_agent.clone());
        builder = builder.default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert("Content-Type", "application/json".parse().unwrap());
            h.insert("Accept", "application/json".parse().unwrap());
            h.insert(
                "x-api-key",
                self.api_key.parse().map_err(|e| anyhow::anyhow!("{e}"))?,
            );
            h
        });
        let http = builder.build().map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Client {
            http,
            base_url,
            api_key: self.api_key,
        })
    }
}

impl Client {
    /// The header pair callers should attach to MCP streaming clients or
    /// any other Composio request made outside the SDK.
    pub fn api_key_header(&self) -> Vec<(String, String)> {
        vec![("x-api-key".to_string(), self.api_key.clone())]
    }

    /// Alias matching Go's `MCPAuthHeaders` helper name.
    pub fn mcp_auth_headers(&self) -> Vec<(String, String)> {
        self.api_key_header()
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<reqwest::Response, Error> {
        let url = format!("{}{path}", self.base_url);
        self.http
            .get(&url)
            .query(query)
            .send()
            .await
            .map_err(|e| Error::Transport(format!("composio: GET {path}: {e}")))
    }

    async fn post_json<T: Serialize>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<reqwest::Response, Error> {
        let url = format!("{}{path}", self.base_url);
        self.http
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(|e| Error::Transport(format!("composio: POST {path}: {e}")))
    }

    async fn send_empty(&self, method: reqwest::Method, path: &str) -> Result<(), Error> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .http
            .request(method.clone(), &url)
            .send()
            .await
            .map_err(|e| Error::Transport(format!("composio: {method} {path}: {e}")))?;
        self.check(resp).await.map(|_| ())
    }

    async fn check(&self, resp: reqwest::Response) -> Result<reqwest::Response, Error> {
        if resp.status().is_client_error() || resp.status().is_server_error() {
            let status = resp.status().as_u16();
            let body = resp.bytes().await.unwrap_or_default();
            return Err(parse_api_error(status, &body).into());
        }
        Ok(resp)
    }

    async fn decode<T: for<'de> Deserialize<'de>>(
        &self,
        resp: reqwest::Response,
    ) -> Result<T, Error> {
        let resp = self.check(resp).await?;
        resp.json::<T>()
            .await
            .map_err(|e| Error::Transport(format!("composio: decode response: {e}")))
    }

    /// POST /connected_accounts/link
    pub async fn create_link(&self, req: CreateLinkRequest) -> Result<CreateLinkResponse, Error> {
        if req.auth_config_id.is_empty() {
            return Err(Error::Other(
                "composio: CreateLink: AuthConfigID is required".into(),
            ));
        }
        if req.user_id.is_empty() {
            return Err(Error::Other(
                "composio: CreateLink: UserID is required".into(),
            ));
        }
        self.decode(self.post_json("/connected_accounts/link", &req).await?)
            .await
    }

    /// GET /connected_accounts
    pub async fn list_connected_accounts(
        &self,
        req: ListConnectedAccountsRequest,
    ) -> Result<ListConnectedAccountsResponse, Error> {
        let mut q: Vec<(&str, String)> = Vec::new();
        for v in &req.user_ids {
            if !v.is_empty() {
                q.push(("user_ids", v.clone()));
            }
        }
        for v in &req.toolkit_slugs {
            if !v.is_empty() {
                q.push(("toolkit_slugs", v.clone()));
            }
        }
        for v in &req.auth_config_ids {
            if !v.is_empty() {
                q.push(("auth_config_ids", v.clone()));
            }
        }
        for v in &req.connected_account_ids {
            if !v.is_empty() {
                q.push(("connected_account_ids", v.clone()));
            }
        }
        for v in &req.statuses {
            if !v.is_empty() {
                q.push(("statuses", v.clone()));
            }
        }
        if !req.order_by.is_empty() {
            q.push(("order_by", req.order_by.clone()));
        }
        if !req.order_direction.is_empty() {
            q.push(("order_direction", req.order_direction.clone()));
        }
        if !req.account_type.is_empty() {
            q.push(("account_type", req.account_type.clone()));
        }
        if req.limit > 0 {
            q.push(("limit", req.limit.to_string()));
        }
        if !req.cursor.is_empty() {
            q.push(("cursor", req.cursor.clone()));
        }
        self.decode(self.get("/connected_accounts", &q).await?)
            .await
    }

    /// GET /auth_configs
    pub async fn list_auth_configs(
        &self,
        req: ListAuthConfigsRequest,
    ) -> Result<ListAuthConfigsResponse, Error> {
        let mut q: Vec<(&str, String)> = Vec::new();
        if !req.toolkit_slugs.is_empty() {
            q.push(("toolkit_slug", req.toolkit_slugs.join(",")));
        }
        if req.show_disabled {
            q.push(("show_disabled", "true".to_string()));
        }
        if !req.search.is_empty() {
            q.push(("search", req.search.clone()));
        }
        if req.limit > 0 {
            q.push(("limit", req.limit.to_string()));
        }
        if !req.cursor.is_empty() {
            q.push(("cursor", req.cursor.clone()));
        }
        self.decode(self.get("/auth_configs", &q).await?).await
    }

    /// GET /toolkits
    pub async fn list_toolkits(
        &self,
        req: ListToolkitsRequest,
    ) -> Result<ListToolkitsResponse, Error> {
        let mut q: Vec<(&str, String)> = Vec::new();
        if !req.category.is_empty() {
            q.push(("category", req.category.clone()));
        }
        if req.limit > 0 {
            q.push(("limit", req.limit.to_string()));
        }
        if !req.cursor.is_empty() {
            q.push(("cursor", req.cursor.clone()));
        }
        if !req.sort_by.is_empty() {
            q.push(("sort_by", req.sort_by.clone()));
        }
        self.decode(self.get("/toolkits", &q).await?).await
    }

    /// POST /tool_router/session
    pub async fn create_session(
        &self,
        req: CreateSessionRequest,
    ) -> Result<CreateSessionResponse, Error> {
        if req.user_id.is_empty() {
            return Err(Error::Other(
                "composio: CreateSession: UserID is required".into(),
            ));
        }
        self.decode(self.post_json("/tool_router/session", &req).await?)
            .await
    }

    /// POST /connected_accounts/{id}/revoke
    pub async fn revoke_connection(&self, connected_account_id: &str) -> Result<(), Error> {
        if connected_account_id.is_empty() {
            return Err(Error::Other(
                "composio: RevokeConnection: connectedAccountID is required".into(),
            ));
        }
        let path = format!(
            "/connected_accounts/{}/revoke",
            urlencode_component(connected_account_id)
        );
        self.send_empty(reqwest::Method::POST, &path).await
    }

    /// DELETE /connected_accounts/{id}. A 404 is swallowed so repeated
    /// disconnects stay idempotent (matching Go's DeleteConnectedAccount).
    pub async fn delete_connected_account(&self, connected_account_id: &str) -> Result<(), Error> {
        if connected_account_id.is_empty() {
            return Err(Error::Other(
                "composio: DeleteConnectedAccount: connectedAccountID is required".into(),
            ));
        }
        let path = format!(
            "/connected_accounts/{}",
            urlencode_component(connected_account_id)
        );
        match self.send_empty(reqwest::Method::DELETE, &path).await {
            Ok(()) => Ok(()),
            Err(Error::Api(api)) if api.is_not_found() => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Transport vs API error split, mirroring how Go callers `errors.As` an
/// `*APIError` out of any error chain.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Api(#[source] Box<ApiError>),
    #[error("{0}")]
    Transport(String),
    #[error("{0}")]
    Other(String),
}

impl From<ApiError> for Error {
    fn from(error: ApiError) -> Self {
        Self::Api(Box::new(error))
    }
}

impl Error {
    /// Reports whether the error is a Composio 404 API error, used to make
    /// revoke/delete idempotent.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Error::Api(a) if a.is_not_found())
    }
}

fn urlencode_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ── webhook verification ─────────────────────────────────────────────────

pub const HEADER_WEBHOOK_ID: &str = "webhook-id";
pub const HEADER_WEBHOOK_TIMESTAMP: &str = "webhook-timestamp";
pub const HEADER_WEBHOOK_SIGNATURE: &str = "webhook-signature";

pub const DEFAULT_WEBHOOK_TOLERANCE: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("composio: missing webhook headers")]
pub struct MissingWebhookHeadersError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("composio: invalid webhook signature")]
pub struct InvalidWebhookSignatureError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("composio: webhook timestamp outside tolerance")]
pub struct WebhookTimestampStaleError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("composio: webhook secret is empty")]
pub struct WebhookSecretMissingError;

#[derive(Debug, Clone, Default)]
pub struct WebhookHeaders {
    pub id: String,
    pub timestamp: String,
    pub signature: String,
}

impl WebhookHeaders {
    pub fn from_http(headers: &http::HeaderMap) -> Self {
        Self {
            id: headers
                .get(HEADER_WEBHOOK_ID)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            timestamp: headers
                .get(HEADER_WEBHOOK_TIMESTAMP)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            signature: headers
                .get(HEADER_WEBHOOK_SIGNATURE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string(),
        }
    }
}

/// Verifies the HMAC-SHA256 signature Composio attaches to every webhook
/// delivery: signing string `<id>.<timestamp>.<body>`, standard-base64
/// digest, version-tagged candidates accepted (`v1,<sig>` forms).
///
/// `now` overrides the wall clock for the tolerance check (tests); pass
/// production clocks via [`std::time::SystemTime::now`].
pub fn verify_webhook(
    secret: &str,
    headers: &WebhookHeaders,
    raw_body: &[u8],
    tolerance: Option<Duration>,
    now: std::time::SystemTime,
) -> Result<(), Box<dyn std::error::Error>> {
    if secret.is_empty() {
        return Err(Box::new(WebhookSecretMissingError));
    }
    if headers.id.is_empty() || headers.timestamp.is_empty() || headers.signature.is_empty() {
        return Err(Box::new(MissingWebhookHeadersError));
    }

    let tolerance = tolerance.unwrap_or(DEFAULT_WEBHOOK_TOLERANCE);
    if !tolerance.is_zero() {
        // Unix seconds first; RFC3339 fallback in case future deliveries
        // switch formats.
        let ts: i64 = match headers.timestamp.parse::<i64>() {
            Ok(v) => v,
            Err(_) => {
                let t =
                    chrono::DateTime::parse_from_rfc3339(&headers.timestamp).map_err(|terr| {
                        format!(
                            "composio: invalid webhook-timestamp {}: {terr}",
                            headers.timestamp
                        )
                    })?;
                t.timestamp()
            }
        };
        let unix_now = now
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs() as i64;
        let delta = (unix_now - ts).abs();
        if u64::try_from(delta)
            .map(|d| d > tolerance.as_secs())
            .unwrap_or(true)
        {
            return Err(Box::new(WebhookTimestampStaleError));
        }
    }

    let signing_string = format!(
        "{}.{}.{}",
        headers.id,
        headers.timestamp,
        String::from_utf8_lossy(raw_body)
    );
    let expected = {
        let mut mac = <hmac::Hmac<sha2::Sha256> as hmac::Mac>::new_from_slice(secret.as_bytes())
            .expect("hmac accepts any key length");
        mac.update(signing_string.as_bytes());
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
    };

    // Header takes the form "v1,<sig>[ v2,<sig> ...]" — accept any
    // version-tagged signature plus the bare-base64 form for forward-compat.
    let normalized = headers.signature.replace(',', " ");
    let candidates: Vec<&str> = normalized.split_whitespace().collect();
    if candidates.is_empty() {
        return Err(Box::new(InvalidWebhookSignatureError));
    }
    for cand in candidates {
        // Skip version tags like "v1" / "v2".
        if cand.len() <= 3 && cand.starts_with('v') {
            continue;
        }
        if constant_time_eq(cand.as_bytes(), expected.as_bytes()) {
            return Ok(());
        }
    }
    Err(Box::new(InvalidWebhookSignatureError))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_error_parses_envelope_and_defaults() {
        let e = parse_api_error(
            503,
            br#"{"error":{"message":"upstream down","code":7,"slug":"unavailable","status":503}}"#,
        );
        assert_eq!(e.http_status, 503);
        assert_eq!(e.message, "upstream down");
        assert_eq!(e.code, 7);
        assert_eq!(e.slug, "unavailable");
        assert!(e.raw_body == *br#"{"error":{"message":"upstream down","code":7,"slug":"unavailable","status":503}}"#.as_slice());

        // Non-envelope body degrades to status-only.
        let e = parse_api_error(500, b"<html>oops</html>");
        assert_eq!(e.message, "");
        assert_eq!(e.http_status, 500);
        assert_eq!(e.raw_body, b"<html>oops</html>".to_vec());

        // Empty body.
        let e = parse_api_error(502, b"");
        assert_eq!(e.message, "");

        assert!(!e.is_rate_limited());
        let e = parse_api_error(404, b"");
        assert!(e.is_not_found());
        let e = parse_api_error(401, b"");
        assert!(e.is_unauthorized());
        let e = parse_api_error(429, b"");
        assert!(e.is_rate_limited());
    }

    #[test]
    fn create_link_request_serializes_go_field_names() {
        let req = CreateLinkRequest {
            auth_config_id: "ac_1".into(),
            user_id: "u_1".into(),
            callback_url: "https://cb".into(),
            alias: String::new(),
            connection_data: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["auth_config_id"], "ac_1");
        assert_eq!(json["user_id"], "u_1");
        assert!(json.get("alias").is_none(), "omitempty parity");
        assert!(json.get("connection_data").is_none());
    }

    #[test]
    fn session_request_omits_absent_maps() {
        let req = CreateSessionRequest {
            user_id: "u".into(),
            toolkits: Some(serde_json::json!({"enable": ["github"]})),
            connected_accounts: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["toolkits"]["enable"][0], "github");
        assert!(json.get("connected_accounts").is_none());
    }

    #[test]
    fn connected_account_decodes_camel_case_wire() {
        let acct: ConnectedAccount = serde_json::from_str(
            r#"{"id":"ca_1","user_id":"u","auth_config_id":"ac","auth_config":{"id":"ac"},"toolkit":{"slug":"gh"},"status":"ACTIVE"}"#,
        )
        .unwrap();
        assert_eq!(acct.auth_config.id, "ac");
        assert_eq!(acct.toolkit.slug, "gh");
    }

    #[test]
    fn builder_requires_api_key() {
        assert!(ClientBuilder::new("").build().is_err());
        assert!(ClientBuilder::new("k").build().is_ok());
    }

    #[test]
    fn verify_webhook_accepts_tagged_and_bare_signatures() {
        let secret = "whsec_test";
        let id = "msg_1";
        let ts = "1700000000";
        let body = br#"{"type":"connection"}"#;
        let signing_string = format!("{id}.{ts}.{}", String::from_utf8_lossy(body));
        let mut mac =
            <hmac::Hmac<sha2::Sha256> as hmac::Mac>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(signing_string.as_bytes());
        use base64::Engine as _;
        let sig = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

        let now = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + 10);
        let ok = |sig_header: &str| {
            verify_webhook(
                secret,
                &WebhookHeaders {
                    id: id.into(),
                    timestamp: ts.into(),
                    signature: sig_header.into(),
                },
                body,
                None,
                now,
            )
            .is_ok()
        };
        assert!(ok(&sig));
        assert!(ok(&format!("v1,{sig}")));
        assert!(!ok("v1,wrong"));
        assert!(!ok(""));
        // Missing pieces fail closed.
        assert!(verify_webhook(secret, &WebhookHeaders::default(), body, None, now).is_err());
        assert!(verify_webhook(
            "",
            &WebhookHeaders {
                id: id.into(),
                timestamp: ts.into(),
                signature: sig.clone()
            },
            body,
            None,
            now
        )
        .is_err());
        // Stale timestamp outside tolerance.
        let late = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + 400);
        assert!(verify_webhook(
            secret,
            &WebhookHeaders {
                id: id.into(),
                timestamp: ts.into(),
                signature: sig
            },
            body,
            None,
            late
        )
        .is_err());
    }
}
