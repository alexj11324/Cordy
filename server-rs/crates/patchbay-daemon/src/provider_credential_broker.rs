//! Loopback provider credential broker.
//!
//! The daemon retains long-lived provider credentials. A task receives only a
//! random bearer accepted by this task-owned loopback listener. Every request
//! revalidates the server-side task/grant lease before the broker substitutes
//! the host credential and forwards upstream.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context};
use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::Response;
use axum::Router;
use chrono::{DateTime, Utc};
use rand::RngCore as _;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::client::Client;
use crate::repocache::Ctx;
use crate::types::{ProviderAuthorization, Task};

const MAX_PROVIDER_REQUEST_BYTES: usize = 16 * 1024 * 1024;

pub(crate) struct ProviderCredentialBroker {
    pub base_url: String,
    pub task_bearer: String,
    cancellation: CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for ProviderCredentialBroker {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.server.abort();
    }
}

impl ProviderCredentialBroker {
    pub(crate) async fn start(
        client: Arc<Client>,
        ctx: Ctx,
        task: &Task,
        authorization: &ProviderAuthorization,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            authorization.task_id == task.id
                && authorization.agent_id == task.agent_id
                && authorization.runtime_id == task.runtime_id
                && authorization.device_id == task.runtime_id
                && authorization.action == "provider.invoke",
            "provider authorization does not match claimed task"
        );
        anyhow::ensure!(
            authorization.provider == "codex" || authorization.provider == "claude",
            "provider credential broker does not support {}",
            authorization.provider
        );
        let expires_at = DateTime::parse_from_rfc3339(&authorization.expires_at)
            .context("parse provider authorization expiry")?
            .with_timezone(&Utc);
        anyhow::ensure!(expires_at > Utc::now(), "provider authorization is expired");
        let credential_mode = load_host_credential(&authorization.provider)?.mode();
        let mut random = [0_u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut random);
        let task_bearer = format!("pbpl_{}", hex::encode(random));
        let cancellation = CancellationToken::new();
        let state = Arc::new(BrokerState {
            client,
            ctx,
            task_token: task.auth_token.clone(),
            task_bearer: task_bearer.clone(),
            authorization: authorization.clone(),
            expires_at,
            credential_mode,
            used_tokens: AtomicU64::new(0),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .context("build provider broker client")?,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind provider credential broker")?;
        let address = listener.local_addr().context("read provider broker address")?;
        let app = Router::new().fallback(proxy_provider_request).with_state(state);
        let shutdown = cancellation.clone();
        let server = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, app)
                .with_graceful_shutdown(async move { shutdown.cancelled().await })
                .await
            {
                tracing::warn!(%error, "provider credential broker stopped");
            }
        });
        let path = match (authorization.provider.as_str(), credential_mode) {
            ("codex", CredentialMode::OpenAiApiKey) => "/openai/v1",
            ("codex", CredentialMode::CodexOauth) => "/openai",
            ("claude", _) => "/anthropic",
            _ => unreachable!("provider checked above"),
        };
        Ok(Self {
            base_url: format!("http://{address}{path}"),
            task_bearer,
            cancellation,
            server,
        })
    }

    pub(crate) fn configure_child_environment(
        &self,
        provider: &str,
        env: &mut BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        match provider {
            "codex" => {
                env.insert("OPENAI_BASE_URL".into(), self.base_url.clone());
                env.insert("OPENAI_API_KEY".into(), self.task_bearer.clone());
            }
            "claude" => {
                env.insert("ANTHROPIC_BASE_URL".into(), self.base_url.clone());
                env.insert("ANTHROPIC_API_KEY".into(), self.task_bearer.clone());
            }
            _ => anyhow::bail!("provider credential broker does not support {provider}"),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CredentialMode {
    OpenAiApiKey,
    CodexOauth,
    AnthropicApiKey,
    ClaudeOauth,
}

enum HostCredential {
    OpenAiApiKey(String),
    CodexOauth {
        access_token: String,
        account_id: String,
    },
    AnthropicApiKey(String),
    ClaudeOauth(String),
}

impl HostCredential {
    fn mode(&self) -> CredentialMode {
        match self {
            Self::OpenAiApiKey(_) => CredentialMode::OpenAiApiKey,
            Self::CodexOauth { .. } => CredentialMode::CodexOauth,
            Self::AnthropicApiKey(_) => CredentialMode::AnthropicApiKey,
            Self::ClaudeOauth(_) => CredentialMode::ClaudeOauth,
        }
    }
}

struct BrokerState {
    client: Arc<Client>,
    ctx: Ctx,
    task_token: String,
    task_bearer: String,
    authorization: ProviderAuthorization,
    expires_at: DateTime<Utc>,
    credential_mode: CredentialMode,
    used_tokens: AtomicU64,
    http: reqwest::Client,
}

async fn proxy_provider_request(
    State(state): State<Arc<BrokerState>>,
    request: Request,
) -> Response {
    match proxy_provider_request_inner(state, request).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(reason = %error, "provider broker denied request");
            Response::builder()
                .status(StatusCode::FORBIDDEN)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"error":{"message":"provider capability denied"}}"#))
                .unwrap_or_else(|_| Response::new(Body::empty()))
        }
    }
}

async fn proxy_provider_request_inner(
    state: Arc<BrokerState>,
    request: Request,
) -> anyhow::Result<Response> {
    anyhow::ensure!(Utc::now() < state.expires_at, "provider capability expired");
    authenticate_task_bearer(request.headers(), &state.task_bearer)?;
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, MAX_PROVIDER_REQUEST_BYTES)
        .await
        .context("read provider request")?;
    let payload: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !state.authorization.models.is_empty() {
        anyhow::ensure!(
            state.authorization.models.iter().any(|allowed| allowed == &model),
            "provider model is outside grant"
        );
    }
    let requested_tokens = payload
        .get("max_output_tokens")
        .or_else(|| payload.get("max_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(4096);
    reserve_token_budget(&state, requested_tokens)?;
    state
        .client
        .validate_provider_lease(
            &state.ctx,
            &state.task_token,
            &state.authorization.runtime_id,
            &state.authorization.provider,
            &model,
            requested_tokens,
        )
        .await
        .context("revalidate provider capability")?;

    let credential = load_host_credential(&state.authorization.provider)?;
    anyhow::ensure!(
        credential.mode() == state.credential_mode,
        "host provider login mode changed"
    );
    let upstream = upstream_url(
        &credential,
        parts
            .uri
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or("/"),
    )?;
    let mut upstream_request = state.http.request(parts.method, upstream).body(body);
    for (name, value) in &parts.headers {
        if is_forwardable_request_header(name) {
            upstream_request = upstream_request.header(name, value);
        }
    }
    upstream_request = apply_host_credential(upstream_request, &credential);
    let upstream_response = upstream_request.send().await.context("provider upstream request")?;
    let status = upstream_response.status();
    let headers = upstream_response.headers().clone();
    let stream = upstream_response.bytes_stream();
    let mut response = Response::builder().status(status);
    for (name, value) in &headers {
        if is_forwardable_response_header(name) {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from_stream(stream))
        .map_err(|error| anyhow!("build provider response: {error}"))
}

fn authenticate_task_bearer(headers: &HeaderMap, expected: &str) -> anyhow::Result<()> {
    let supplied = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-api-key").and_then(|value| value.to_str().ok()))
        .unwrap_or_default();
    anyhow::ensure!(
        constant_time_equal(supplied.as_bytes(), expected.as_bytes()),
        "invalid broker bearer"
    );
    Ok(())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn reserve_token_budget(state: &BrokerState, requested: u64) -> anyhow::Result<()> {
    let Some(maximum) = state.authorization.max_tokens else {
        return Ok(());
    };
    let result =
        state
            .used_tokens
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(requested).filter(|next| *next <= maximum)
            });
    anyhow::ensure!(result.is_ok(), "provider token budget exceeded");
    Ok(())
}

fn load_host_credential(provider: &str) -> anyhow::Result<HostCredential> {
    match provider {
        "codex" => load_codex_credential(),
        "claude" => load_claude_credential(),
        _ => anyhow::bail!("unsupported provider credential {provider}"),
    }
}

fn load_codex_credential() -> anyhow::Result<HostCredential> {
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        if !key.trim().is_empty() && !key.starts_with("pbpl_") {
            return Ok(HostCredential::OpenAiApiKey(key));
        }
    }
    let path = std::path::Path::new(&crate::execenv::codex_home::resolve_shared_codex_home())
        .join("auth.json");
    let document: Value = serde_json::from_slice(
        &std::fs::read(&path).with_context(|| format!("read host Codex login at {}", path.display()))?,
    )
    .context("parse host Codex login")?;
    if let Some(key) = document.get("OPENAI_API_KEY").and_then(Value::as_str) {
        if !key.trim().is_empty() {
            return Ok(HostCredential::OpenAiApiKey(key.to_string()));
        }
    }
    let tokens = document.get("tokens").and_then(Value::as_object);
    let access_token = tokens
        .and_then(|tokens| tokens.get("access_token"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("host Codex login has no usable access token"))?;
    let account_id = tokens
        .and_then(|tokens| tokens.get("account_id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(HostCredential::CodexOauth {
        access_token: access_token.to_string(),
        account_id: account_id.to_string(),
    })
}

fn load_claude_credential() -> anyhow::Result<HostCredential> {
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.trim().is_empty() && !key.starts_with("pbpl_") {
            return Ok(HostCredential::AnthropicApiKey(key));
        }
    }
    if let Ok(token) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        if !token.trim().is_empty() {
            return Ok(HostCredential::ClaudeOauth(token));
        }
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow!("host HOME is unavailable"))?;
    let path = home.join(".claude").join(".credentials.json");
    let document: Value = serde_json::from_slice(
        &std::fs::read(&path).with_context(|| format!("read host Claude login at {}", path.display()))?,
    )
    .context("parse host Claude login")?;
    let access_token = document
        .pointer("/claudeAiOauth/accessToken")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("host Claude login has no usable access token"))?;
    Ok(HostCredential::ClaudeOauth(access_token.to_string()))
}

fn upstream_url(credential: &HostCredential, local_path: &str) -> anyhow::Result<String> {
    let (prefix, base) = match credential {
        HostCredential::OpenAiApiKey(_) => ("/openai/v1", "https://api.openai.com/v1"),
        HostCredential::CodexOauth { .. } => {
            ("/openai", "https://chatgpt.com/backend-api/codex")
        }
        HostCredential::AnthropicApiKey(_) | HostCredential::ClaudeOauth(_) => {
            ("/anthropic", "https://api.anthropic.com")
        }
    };
    let suffix = local_path
        .strip_prefix(prefix)
        .ok_or_else(|| anyhow!("provider request path is outside broker route"))?;
    anyhow::ensure!(suffix.starts_with('/'), "invalid provider request path");
    let path_only = suffix.split('?').next().unwrap_or(suffix);
    anyhow::ensure!(
        !path_only.split('/').any(|segment| segment == ".."),
        "provider request path traversal is forbidden"
    );
    Ok(format!("{base}{suffix}"))
}

fn apply_host_credential(
    request: reqwest::RequestBuilder,
    credential: &HostCredential,
) -> reqwest::RequestBuilder {
    match credential {
        HostCredential::OpenAiApiKey(key) => request.bearer_auth(key),
        HostCredential::CodexOauth {
            access_token,
            account_id,
        } => {
            let request = request.bearer_auth(access_token);
            if account_id.is_empty() {
                request
            } else {
                request.header("ChatGPT-Account-Id", account_id)
            }
        }
        HostCredential::AnthropicApiKey(key) => request.header("x-api-key", key),
        HostCredential::ClaudeOauth(token) => request
            .bearer_auth(token)
            .header("anthropic-beta", "oauth-2025-04-20"),
    }
}

fn is_forwardable_request_header(name: &HeaderName) -> bool {
    !matches!(
        name.as_str(),
        "authorization" | "x-api-key" | "host" | "content-length" | "cookie"
    )
}

fn is_forwardable_response_header(name: &HeaderName) -> bool {
    !matches!(
        name.as_str(),
        "set-cookie" | "transfer-encoding" | "connection" | "content-length"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_bearer_comparison_is_exact() {
        assert!(constant_time_equal(b"pbpl_same", b"pbpl_same"));
        assert!(!constant_time_equal(b"pbpl_same", b"pbpl_other"));
        assert!(!constant_time_equal(b"short", b"longer"));
    }

    #[test]
    fn upstream_paths_cannot_escape_provider_route() {
        let credential = HostCredential::OpenAiApiKey("secret".into());
        assert_eq!(
            upstream_url(&credential, "/openai/v1/responses").unwrap(),
            "https://api.openai.com/v1/responses"
        );
        assert!(upstream_url(&credential, "/anthropic/v1/messages").is_err());
    }
}
