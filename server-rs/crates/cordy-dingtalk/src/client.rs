//! Port of `client.go` + `token.go`: the outbound DingTalk Open-API client.
//!
//! This is the thin DingTalk Open-API REST seam the install + outbound paths
//! share: minting an access_token from AppKey/AppSecret, plus a JSON POST
//! helper. It is deliberately hand-rolled over reqwest (not the Stream SDK)
//! because the only REST calls the server makes outside the Stream connection
//! are the token mint plus the message send; keeping it here makes both
//! trivially testable against a local server via the api_base override.
//!
//! DingTalk's access_token expires (~2h), unlike Slack's static bot token, so
//! it is cached in-process keyed by AppKey and refreshed before expiry. The
//! mint collapses concurrent cache misses for the same AppKey into a single
//! in-flight token request (Go's singleflight): a burst of outbound sends
//! (ack + reply) sharing an expired token would otherwise each fire a redundant
//! token request and race to overwrite the cache, and DingTalk rate-limits
//! token issuance.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;

/// Returned by [`Client::post_json`] on an HTTP 401 so the outbound sender can
/// drop the cached access_token and retry once with a fresh one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("dingtalk: unauthorized (access token expired or invalid)")]
pub struct Unauthorized;

/// Reports whether an error chain carries the 401 sentinel.
pub fn is_unauthorized(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.downcast_ref::<Unauthorized>().is_some())
}

/// The DingTalk Open-API host. The mainland cloud is the only region DingTalk
/// exposes for these endpoints, so unlike Feishu there is no per-installation
/// region split.
pub const DEFAULT_API_BASE: &str = "https://api.dingtalk.com";

/// Mints an enterprise-internal-app access_token from the app's
/// AppKey/AppSecret. The response carries the token and its lifetime in
/// seconds.
pub const ACCESS_TOKEN_PATH: &str = "/v1.0/oauth2/accessToken";

/// Resolves a robot message downloadCode to a short-lived download URL. Both
/// the code and the returned URL are temporary (DingTalk documents no exact
/// TTL) — resolve and fetch immediately, and never persist or log either value.
pub const MESSAGE_FILES_DOWNLOAD_PATH: &str = "/v1.0/robot/messageFiles/download";

/// Subtracted from DingTalk's expireIn so a token is refreshed before it
/// actually expires, absorbing clock skew and in-flight use.
const TOKEN_SAFETY_MARGIN: Duration = Duration::from_secs(5 * 60);

/// Bounds the shared mint independently of any one caller.
const TOKEN_MINT_TIMEOUT: Duration = Duration::from_secs(10);

/// Cap on how much of any response body we read (mirrors Go's LimitReader).
const MAX_RESPONSE_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct AccessTokenResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    expire_in: i64,
}

/// The DingTalk Open-API error envelope (non-2xx responses).
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct ApiError {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct MessageFileDownloadResponse {
    #[serde(default, rename = "downloadUrl")]
    download_url: String,
}

#[derive(Debug, Clone)]
struct CachedToken {
    value: String,
    expires_at: Instant,
}

/// Caches access_tokens and posts robot messages. One instance is shared
/// across installations; the cache is keyed by AppKey so each installation's
/// token is independent. Safe for concurrent use.
pub struct Client {
    http: reqwest::Client,
    api_base: String,
    tokens: Mutex<HashMap<String, CachedToken>>,
    // Per-AppKey mint locks: collapse concurrent cache misses into one in-flight
    // token request (singleflight equivalent). Waiters re-check the cache after
    // acquiring the lock and reuse whatever the flight stored.
    mint_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Client {
    /// Builds the outbound client. `api_base` defaults to the DingTalk
    /// Open-API host; tests point it at a local server.
    pub fn new(http: Option<reqwest::Client>, api_base: &str) -> Self {
        Self {
            http: http.unwrap_or_default(),
            api_base: if api_base.is_empty() {
                DEFAULT_API_BASE.to_string()
            } else {
                api_base.trim_end_matches('/').to_string()
            },
            tokens: Mutex::new(HashMap::new()),
            mint_locks: Mutex::new(HashMap::new()),
        }
    }

    /// The configured API base (the connector dials its Stream open through
    /// the same base).
    pub(crate) fn api_base(&self) -> &str {
        &self.api_base
    }

    /// The shared HTTP client (used by the Stream open call).
    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Returns a usable access_token for (app_key, app_secret), minting and
    /// caching one when the cache is empty or stale.
    pub async fn access_token(&self, app_key: &str, app_secret: &str) -> anyhow::Result<String> {
        if let Some(t) = self.cached_token(app_key) {
            return Ok(t);
        }
        let lock = self.mint_lock(app_key);
        let _guard = lock.lock().await;
        // A mint that finished while we queued behind the lock already
        // refreshed the cache; reuse it instead of fetching again.
        if let Some(t) = self.cached_token(app_key) {
            return Ok(t);
        }
        let mint = fetch_access_token(&self.http, &self.api_base, app_key, app_secret);
        let (token, expire_in) = tokio::time::timeout(TOKEN_MINT_TIMEOUT, mint)
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "dingtalk: access token mint timed out after {TOKEN_MINT_TIMEOUT:?}"
                )
            })??;
        let mut ttl = Duration::from_secs(expire_in.max(0) as u64);
        if ttl < TOKEN_SAFETY_MARGIN * 2 {
            ttl = TOKEN_SAFETY_MARGIN * 2;
        }
        let expires_at = Instant::now() + ttl - TOKEN_SAFETY_MARGIN;
        self.tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                app_key.to_string(),
                CachedToken {
                    value: token.clone(),
                    expires_at,
                },
            );
        Ok(token)
    }

    /// Returns/creates the per-AppKey mint lock.
    fn mint_lock(&self, app_key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.mint_locks.lock().unwrap_or_else(|e| e.into_inner());
        locks
            .entry(app_key.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Returns the cached token for app_key if present and unexpired.
    fn cached_token(&self, app_key: &str) -> Option<String> {
        let tokens = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        tokens
            .get(app_key)
            .filter(|t| t.expires_at > Instant::now())
            .map(|t| t.value.clone())
    }

    /// Drops the cached token for app_key so the next access_token call
    /// refreshes. Used after the API reports an expired/invalid token (HTTP
    /// 401).
    pub fn invalidate(&self, app_key: &str) {
        self.tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(app_key);
    }

    /// Resolves a robot-received message's downloadCode to its temporary
    /// download URL via POST /v1.0/robot/messageFiles/download. It refreshes
    /// the cached token once on a 401 (mirroring sender.send_one). The
    /// returned URL is a short-lived signed link — fetch it immediately.
    pub async fn message_file_download_url(
        &self,
        app_key: &str,
        app_secret: &str,
        robot_code: &str,
        download_code: &str,
    ) -> anyhow::Result<String> {
        let body = serde_json::json!({
            "robotCode": robot_code,
            "downloadCode": download_code,
        });
        let token = self.access_token(app_key, app_secret).await?;
        let result: anyhow::Result<Option<MessageFileDownloadResponse>> = self
            .post_json(MESSAGE_FILES_DOWNLOAD_PATH, &token, &body)
            .await;
        let out = match result {
            Ok(out) => out,
            Err(err) if is_unauthorized(&err) => {
                self.invalidate(app_key);
                let token = self.access_token(app_key, app_secret).await?;
                self.post_json(MESSAGE_FILES_DOWNLOAD_PATH, &token, &body)
                    .await?
            }
            Err(err) => return Err(err),
        };
        let Some(out) = out else {
            anyhow::bail!("dingtalk: messageFiles/download returned empty downloadUrl");
        };
        if out.download_url.is_empty() {
            anyhow::bail!("dingtalk: messageFiles/download returned empty downloadUrl");
        }
        Ok(out.download_url)
    }

    /// Posts body to path with the access token header and decodes a 2xx
    /// response into T. Returns [`Unauthorized`] on HTTP 401 so the caller can
    /// refresh the token and retry.
    pub(crate) async fn post_json<T: DeserializeOwned>(
        &self,
        path: &str,
        access_token: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<Option<T>> {
        let url = format!("{}{}", self.api_base, path);
        let resp = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .header("x-acs-dingtalk-access-token", access_token)
            .json(body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("dingtalk: request {path}: {e}"))?;

        let status = resp.status();
        let resp_body = resp.bytes().await?;
        let resp_body = &resp_body[..resp_body.len().min(MAX_RESPONSE_BYTES)];

        if status.as_u16() == 401 {
            return Err(Unauthorized.into());
        }
        if !status.is_success() {
            let api_err: ApiError = serde_json::from_slice(resp_body).unwrap_or_default();
            if !api_err.message.is_empty() {
                anyhow::bail!(
                    "dingtalk: {path}: code={:?} message={:?}",
                    api_err.code,
                    api_err.message
                );
            }
            anyhow::bail!("dingtalk: {path}: http {}", status.as_u16());
        }
        if resp_body.is_empty() {
            return Ok(None);
        }
        let out = serde_json::from_slice(resp_body)
            .map_err(|e| anyhow::anyhow!("dingtalk: decode {path} response: {e}"))?;
        Ok(Some(out))
    }
}

/// Mints an access_token for (app_key, app_secret). `base_url` defaults to the
/// DingTalk Open-API host; tests point it at a local server. It returns the
/// token and its lifetime in seconds. A non-2xx response or a missing token is
/// an error — the install path uses a failure here as "these credentials are
/// wrong".
pub async fn fetch_access_token(
    http: &reqwest::Client,
    base_url: &str,
    app_key: &str,
    app_secret: &str,
) -> anyhow::Result<(String, i64)> {
    let base = if base_url.is_empty() {
        DEFAULT_API_BASE
    } else {
        base_url.trim_end_matches('/')
    };
    let body = serde_json::json!({"appKey": app_key, "appSecret": app_secret});
    let resp = http
        .post(format!("{base}{ACCESS_TOKEN_PATH}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("dingtalk: access token request: {e}"))?;
    let status = resp.status();
    let bytes = resp.bytes().await?;
    let bytes = &bytes[..bytes.len().min(MAX_RESPONSE_BYTES)];

    if !status.is_success() {
        let api_err: ApiError = serde_json::from_slice(bytes).unwrap_or_default();
        if !api_err.message.is_empty() {
            anyhow::bail!(
                "dingtalk: access token: code={:?} message={:?}",
                api_err.code,
                api_err.message
            );
        }
        anyhow::bail!("dingtalk: access token: http {}", status.as_u16());
    }

    let tok: AccessTokenResponse = serde_json::from_slice(bytes)
        .map_err(|e| anyhow::anyhow!("dingtalk: decode access token response: {e}"))?;
    if tok.access_token.is_empty() {
        anyhow::bail!("dingtalk: access token response missing accessToken");
    }
    Ok((tok.access_token, tok.expire_in))
}
