//! Loopback provider credential broker.
//!
//! The daemon retains long-lived provider credentials. A task receives only a
//! random bearer accepted by this task-owned loopback listener. Every request
//! revalidates the server-side task/grant lease before the broker substitutes
//! the host credential and forwards upstream.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, Context};
use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::Response;
use axum::Router;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use rand::RngCore as _;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::client::Client;
use crate::repocache::Ctx;
use crate::types::{ProviderAuthorization, Task};

const MAX_PROVIDER_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const OAUTH_REFRESH_WINDOW_SECONDS: i64 = 5 * 60;
const CODEX_OAUTH_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const CLAUDE_OAUTH_TOKEN_ENDPOINT: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_DEFAULT_SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

static PROVIDER_REFRESH_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

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
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("build provider broker client")?;
        let credential_mode = current_host_credential(&authorization.provider, &http, None)
            .await?
            .mode();
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
            http,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("bind provider credential broker")?;
        let address = listener
            .local_addr()
            .context("read provider broker address")?;
        let app = Router::new()
            .fallback(proxy_provider_request)
            .with_state(state);
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
        source: Option<CodexOauthSource>,
    },
    AnthropicApiKey(String),
    ClaudeOauth {
        access_token: String,
        source: Option<ClaudeOauthSource>,
    },
}

struct CodexOauthSource {
    path: PathBuf,
    refresh_token: String,
    expires_at: Option<DateTime<Utc>>,
}

struct ClaudeOauthSource {
    path: PathBuf,
    refresh_token: String,
    expires_at: Option<DateTime<Utc>>,
    scopes: String,
}

impl HostCredential {
    fn mode(&self) -> CredentialMode {
        match self {
            Self::OpenAiApiKey(_) => CredentialMode::OpenAiApiKey,
            Self::CodexOauth { .. } => CredentialMode::CodexOauth,
            Self::AnthropicApiKey(_) => CredentialMode::AnthropicApiKey,
            Self::ClaudeOauth { .. } => CredentialMode::ClaudeOauth,
        }
    }

    fn refreshable_access_token(&self) -> Option<&str> {
        match self {
            Self::CodexOauth {
                access_token,
                source: Some(_),
                ..
            }
            | Self::ClaudeOauth {
                access_token,
                source: Some(_),
            } => Some(access_token),
            _ => None,
        }
    }

    fn needs_refresh(&self, now: DateTime<Utc>) -> bool {
        let expires_at = match self {
            Self::CodexOauth {
                source: Some(source),
                ..
            } => source.expires_at,
            Self::ClaudeOauth {
                source: Some(source),
                ..
            } => source.expires_at,
            _ => None,
        };
        expires_at.is_some_and(|expiry| {
            expiry <= now + chrono::Duration::seconds(OAUTH_REFRESH_WINDOW_SECONDS)
        })
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
                .body(Body::from(
                    r#"{"error":{"message":"provider capability denied"}}"#,
                ))
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
            state
                .authorization
                .models
                .iter()
                .any(|allowed| allowed == &model),
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

    let credential =
        current_host_credential(&state.authorization.provider, &state.http, None).await?;
    anyhow::ensure!(
        credential.mode() == state.credential_mode,
        "host provider login mode changed"
    );
    let local_path = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let rejected_access_token = credential.refreshable_access_token().map(str::to_string);
    let mut upstream_response = send_upstream_request(
        &state.http,
        &parts.method,
        &parts.headers,
        local_path,
        body.clone(),
        &credential,
    )
    .await?;
    if upstream_response.status() == StatusCode::UNAUTHORIZED {
        if let Some(rejected_access_token) = rejected_access_token {
            drop(upstream_response);
            let refreshed = current_host_credential(
                &state.authorization.provider,
                &state.http,
                Some(&rejected_access_token),
            )
            .await?;
            anyhow::ensure!(
                refreshed.mode() == state.credential_mode,
                "host provider login mode changed during refresh"
            );
            upstream_response = send_upstream_request(
                &state.http,
                &parts.method,
                &parts.headers,
                local_path,
                body,
                &refreshed,
            )
            .await?;
        }
    }
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

async fn send_upstream_request(
    http: &reqwest::Client,
    method: &axum::http::Method,
    headers: &HeaderMap,
    local_path: &str,
    body: axum::body::Bytes,
    credential: &HostCredential,
) -> anyhow::Result<reqwest::Response> {
    let upstream = upstream_url(credential, local_path)?;
    let mut request = http.request(method.clone(), upstream).body(body);
    for (name, value) in headers {
        if is_forwardable_request_header(name) {
            request = request.header(name, value);
        }
    }
    apply_host_credential(request, credential)
        .send()
        .await
        .context("provider upstream request")
}

fn authenticate_task_bearer(headers: &HeaderMap, expected: &str) -> anyhow::Result<()> {
    let supplied = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .or_else(|| {
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok())
        })
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
    let result = state
        .used_tokens
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
            used.checked_add(requested).filter(|next| *next <= maximum)
        });
    anyhow::ensure!(result.is_ok(), "provider token budget exceeded");
    Ok(())
}

async fn current_host_credential(
    provider: &str,
    http: &reqwest::Client,
    rejected_access_token: Option<&str>,
) -> anyhow::Result<HostCredential> {
    let initial = read_host_credential(provider)?;
    if !credential_requires_refresh(&initial, rejected_access_token) {
        return Ok(initial);
    }

    let lock = PROVIDER_REFRESH_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
    let _guard = lock.lock().await;
    // A sibling broker may have refreshed the shared host login while this
    // request waited. Re-read under the lock before rotating a refresh token.
    let current = read_host_credential(provider)?;
    if !credential_requires_refresh(&current, rejected_access_token) {
        return Ok(current);
    }
    refresh_host_credential(http, current).await
}

fn credential_requires_refresh(
    credential: &HostCredential,
    rejected_access_token: Option<&str>,
) -> bool {
    if let Some(rejected) = rejected_access_token {
        return credential.refreshable_access_token() == Some(rejected);
    }
    credential.needs_refresh(Utc::now())
}

fn read_host_credential(provider: &str) -> anyhow::Result<HostCredential> {
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
        &std::fs::read(&path)
            .with_context(|| format!("read host Codex login at {}", path.display()))?,
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
    let refresh_token = tokens
        .and_then(|tokens| tokens.get("refresh_token"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    Ok(HostCredential::CodexOauth {
        access_token: access_token.to_string(),
        account_id: account_id.to_string(),
        source: refresh_token.map(|refresh_token| CodexOauthSource {
            path,
            refresh_token: refresh_token.to_string(),
            expires_at: jwt_expiry(access_token),
        }),
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
            return Ok(HostCredential::ClaudeOauth {
                access_token: token,
                source: None,
            });
        }
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow!("host HOME is unavailable"))?;
    let path = home.join(".claude").join(".credentials.json");
    let document: Value = serde_json::from_slice(
        &std::fs::read(&path)
            .with_context(|| format!("read host Claude login at {}", path.display()))?,
    )
    .context("parse host Claude login")?;
    let access_token = document
        .pointer("/claudeAiOauth/accessToken")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("host Claude login has no usable access token"))?;
    let oauth = document
        .get("claudeAiOauth")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("host Claude login is malformed"))?;
    let refresh_token = oauth
        .get("refreshToken")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let expires_at = oauth
        .get("expiresAt")
        .and_then(Value::as_i64)
        .and_then(unix_timestamp);
    let scopes = oauth
        .get("scopes")
        .and_then(|value| match value {
            Value::Array(values) => Some(
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            Value::String(value) => Some(value.clone()),
            _ => None,
        })
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| CLAUDE_DEFAULT_SCOPES.to_string());
    Ok(HostCredential::ClaudeOauth {
        access_token: access_token.to_string(),
        source: refresh_token.map(|refresh_token| ClaudeOauthSource {
            path,
            refresh_token: refresh_token.to_string(),
            expires_at,
            scopes,
        }),
    })
}

async fn refresh_host_credential(
    http: &reqwest::Client,
    credential: HostCredential,
) -> anyhow::Result<HostCredential> {
    match credential {
        HostCredential::CodexOauth {
            source: Some(source),
            ..
        } => refresh_codex_credential(http, source).await,
        HostCredential::ClaudeOauth {
            source: Some(source),
            ..
        } => refresh_claude_credential(http, source).await,
        _ => anyhow::bail!("host provider login cannot be refreshed"),
    }
}

#[derive(Deserialize)]
struct CodexRefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
}

async fn refresh_codex_credential(
    http: &reqwest::Client,
    source: CodexOauthSource,
) -> anyhow::Result<HostCredential> {
    let refreshed: CodexRefreshResponse = http
        .post(CODEX_OAUTH_TOKEN_ENDPOINT)
        .json(&json!({
            "client_id": CODEX_OAUTH_CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": &source.refresh_token,
        }))
        .send()
        .await
        .context("refresh host Codex login")?
        .error_for_status()
        .context("host Codex login refresh was rejected")?
        .json()
        .await
        .context("decode host Codex login refresh")?;
    anyhow::ensure!(
        !refreshed.access_token.trim().is_empty(),
        "host Codex login refresh returned no access token"
    );
    let mut document = read_json_document(&source.path, "Codex")?;
    let tokens = document
        .get_mut("tokens")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("host Codex login is malformed"))?;
    tokens.insert("access_token".into(), Value::String(refreshed.access_token));
    tokens.insert(
        "refresh_token".into(),
        Value::String(refreshed.refresh_token.unwrap_or(source.refresh_token)),
    );
    if let Some(id_token) = refreshed.id_token {
        tokens.insert("id_token".into(), Value::String(id_token));
    }
    document["last_refresh"] = Value::String(Utc::now().to_rfc3339());
    atomic_write_private_json(&source.path, &document)?;
    load_codex_credential()
}

#[derive(Deserialize)]
struct ClaudeRefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
    refresh_token_expires_in: Option<i64>,
    scope: Option<String>,
}

async fn refresh_claude_credential(
    http: &reqwest::Client,
    source: ClaudeOauthSource,
) -> anyhow::Result<HostCredential> {
    let refreshed: ClaudeRefreshResponse = http
        .post(CLAUDE_OAUTH_TOKEN_ENDPOINT)
        .json(&json!({
            "client_id": CLAUDE_OAUTH_CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": &source.refresh_token,
            "scope": &source.scopes,
        }))
        .send()
        .await
        .context("refresh host Claude login")?
        .error_for_status()
        .context("host Claude login refresh was rejected")?
        .json()
        .await
        .context("decode host Claude login refresh")?;
    anyhow::ensure!(
        !refreshed.access_token.trim().is_empty(),
        "host Claude login refresh returned no access token"
    );
    anyhow::ensure!(
        refreshed.expires_in > 0,
        "host Claude login refresh returned an invalid expiry"
    );
    let mut document = read_json_document(&source.path, "Claude")?;
    let oauth = document
        .get_mut("claudeAiOauth")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("host Claude login is malformed"))?;
    oauth.insert("accessToken".into(), Value::String(refreshed.access_token));
    oauth.insert(
        "refreshToken".into(),
        Value::String(refreshed.refresh_token.unwrap_or(source.refresh_token)),
    );
    oauth.insert(
        "expiresAt".into(),
        Value::from(future_epoch_millis(refreshed.expires_in)?),
    );
    if let Some(refresh_expires_in) = refreshed.refresh_token_expires_in {
        oauth.insert(
            "refreshTokenExpiresAt".into(),
            Value::from(future_epoch_millis(refresh_expires_in)?),
        );
    }
    if let Some(scope) = refreshed.scope {
        oauth.insert(
            "scopes".into(),
            Value::Array(
                scope
                    .split_whitespace()
                    .map(|value| Value::String(value.to_string()))
                    .collect(),
            ),
        );
    }
    atomic_write_private_json(&source.path, &document)?;
    load_claude_credential()
}

fn read_json_document(path: &Path, provider: &str) -> anyhow::Result<Value> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("read host {provider} login"))?)
        .with_context(|| format!("parse host {provider} login"))
}

fn atomic_write_private_json(path: &Path, document: &Value) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("host credential path has no parent"))?;
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).context("create host credential refresh file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .context("protect host credential refresh file")?;
    }
    serde_json::to_writer_pretty(temp.as_file_mut(), document)
        .context("encode host credential refresh")?;
    temp.as_file_mut()
        .write_all(b"\n")
        .context("finish host credential refresh")?;
    temp.as_file()
        .sync_all()
        .context("sync host credential refresh")?;
    temp.persist(path)
        .map_err(|error| anyhow!(error))
        .context("replace host credential login")?;
    Ok(())
}

fn jwt_expiry(token: &str) -> Option<DateTime<Utc>> {
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let document: Value = serde_json::from_slice(&decoded).ok()?;
    document
        .get("exp")
        .and_then(Value::as_i64)
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
}

fn unix_timestamp(value: i64) -> Option<DateTime<Utc>> {
    if value.abs() >= 10_000_000_000 {
        DateTime::from_timestamp_millis(value)
    } else {
        DateTime::from_timestamp(value, 0)
    }
}

fn future_epoch_millis(delta_seconds: i64) -> anyhow::Result<i64> {
    anyhow::ensure!(delta_seconds > 0, "provider OAuth expiry must be positive");
    Utc::now()
        .timestamp()
        .checked_add(delta_seconds)
        .and_then(|seconds| seconds.checked_mul(1000))
        .ok_or_else(|| anyhow!("provider OAuth expiry is out of range"))
}

fn upstream_url(credential: &HostCredential, local_path: &str) -> anyhow::Result<String> {
    let (prefix, base) = match credential {
        HostCredential::OpenAiApiKey(_) => ("/openai/v1", "https://api.openai.com/v1"),
        HostCredential::CodexOauth { .. } => ("/openai", "https://chatgpt.com/backend-api/codex"),
        HostCredential::AnthropicApiKey(_) | HostCredential::ClaudeOauth { .. } => {
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
            ..
        } => {
            let request = request.bearer_auth(access_token);
            if account_id.is_empty() {
                request
            } else {
                request.header("ChatGPT-Account-Id", account_id)
            }
        }
        HostCredential::AnthropicApiKey(key) => request.header("x-api-key", key),
        HostCredential::ClaudeOauth { access_token, .. } => request
            .bearer_auth(access_token)
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

    #[test]
    fn oauth_expiry_parsers_accept_jwt_seconds_and_claude_millis() {
        let expiry = Utc::now().timestamp() + 3600;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(json!({ "exp": expiry }).to_string());
        let token = format!("header.{payload}.signature");
        assert_eq!(
            jwt_expiry(&token).map(|value| value.timestamp()),
            Some(expiry)
        );
        assert_eq!(
            unix_timestamp(expiry * 1000).map(|value| value.timestamp()),
            Some(expiry)
        );
    }

    #[test]
    fn oauth_refresh_is_scheduled_before_expiry_only_for_refreshable_logins() {
        let expiring = HostCredential::ClaudeOauth {
            access_token: "access".into(),
            source: Some(ClaudeOauthSource {
                path: PathBuf::from("credentials.json"),
                refresh_token: "refresh".into(),
                expires_at: Some(Utc::now() + chrono::Duration::seconds(30)),
                scopes: CLAUDE_DEFAULT_SCOPES.into(),
            }),
        };
        let fixed = HostCredential::ClaudeOauth {
            access_token: "access".into(),
            source: None,
        };
        assert!(expiring.needs_refresh(Utc::now()));
        assert!(!fixed.needs_refresh(Utc::now()));
    }
}
