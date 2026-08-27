//! OpenAI-compatible client for model calls made by the API process itself.
//!
//! This is deliberately separate from agent execution. Agent runtimes use
//! their own credentials and process boundary; this client is the single
//! outbound entry point for small server-internal assist features such as chat
//! auto-titling. A client with neither an API key nor a base URL is disabled
//! and constructs no request, preserving the self-hosted zero-egress contract.

use std::{sync::Arc, time::Duration};

use reqwest::{header, StatusCode};
use serde::{Deserialize, Serialize};

pub const FALLBACK_MODEL: &str = "gpt-5.6-luna";
pub const DEFAULT_MAX_RETRIES: u32 = 2;
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// A validated transport retry budget.
///
/// Keeping the value private makes the unset-versus-explicit-zero distinction
/// part of [`Config`] rather than something callers can accidentally erase by
/// constructing an invalid value directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryOverride {
    value: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RetryConfigError {
    #[error("llm: max retries must not be negative, got {0} (use 0 to disable retries)")]
    Negative(i64),
    #[error("llm: max retries must fit in an unsigned 32-bit integer, got {0}")]
    TooLarge(i64),
}

/// Builds an explicit retry override. `retries(0)` disables transport retries;
/// a negative value is rejected instead of being silently corrected.
pub fn retries(value: i64) -> Result<RetryOverride, RetryConfigError> {
    if value < 0 {
        return Err(RetryConfigError::Negative(value));
    }
    let value = u32::try_from(value).map_err(|_| RetryConfigError::TooLarge(value))?;
    Ok(RetryOverride { value })
}

impl RetryOverride {
    pub fn value(self) -> u32 {
        self.value
    }
}

/// Identifies where the effective retry budget came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RetrySource {
    #[default]
    Default,
    Config,
}

impl RetrySource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Config => "config",
        }
    }
}

/// The effective transport retry policy used by a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RetryBudget {
    pub max_retries: u32,
    pub source: RetrySource,
    pub request_timeout: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub api_key: String,
    pub base_url: String,
    pub default_model: String,
    /// `None` uses [`DEFAULT_MAX_RETRIES`]; `Some(retries(0)?)` disables
    /// retries while preserving that it was explicitly configured.
    pub max_retries: Option<RetryOverride>,
    /// Replaces the default request transport. This is primarily a test seam,
    /// but also preserves callers' ability to supply a configured reqwest
    /// client with custom proxies, certificates, or timeouts.
    pub http_client: Option<reqwest::Client>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("llm: no API key or base URL configured")]
    NotConfigured,
    #[error("llm: request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("llm: HTTP client unavailable")]
    ClientUnavailable,
    #[error("llm: request timed out")]
    Timeout,
    #[error("llm: invalid upstream response: {0}")]
    InvalidResponse(String),
    #[error("llm: invalid request: {0}")]
    InvalidRequest(String),
    #[error("llm: upstream returned HTTP {0}")]
    Upstream(StatusCode),
    #[error("llm: upstream returned no choices")]
    NoChoices,
}

#[derive(Clone)]
pub struct Client {
    transport: Option<Arc<dyn Transport>>,
    stream_client: Option<reqwest::Client>,
    api_key: String,
    endpoint: String,
    default_model: String,
    retry: RetryBudget,
    enabled: bool,
}

impl Client {
    pub fn new(config: Config) -> Self {
        let Config {
            api_key: configured_api_key,
            base_url: configured_base_url,
            default_model: configured_default_model,
            max_retries,
            http_client,
        } = config;
        let api_key = configured_api_key.trim().to_owned();
        let configured_base_url = configured_base_url.trim();
        let enabled = !api_key.is_empty() || !configured_base_url.is_empty();
        let base_url = if configured_base_url.is_empty() {
            DEFAULT_BASE_URL
        } else {
            configured_base_url
        };
        let endpoint = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        // Construction remains infallible at startup, matching the Go layer.
        // A malformed configured URL remains configured but fails its calls;
        // it is never replaced with the default OpenAI endpoint.
        // The non-streaming methods apply DEFAULT_REQUEST_TIMEOUT around
        // their complete retry/response lifecycle. A total reqwest timeout
        // would also expire a successfully opened stream while its caller is
        // still consuming the body, so the streaming client deliberately has
        // no total timeout; the caller owns that lifetime and can cancel the
        // future or response when its ChatStream context equivalent expires.
        let stream_client = http_client.or_else(|| reqwest::Client::builder().build().ok());
        let transport = stream_client
            .clone()
            .map(|client| Arc::new(ReqwestTransport(client)) as Arc<dyn Transport>);
        let default_model = match configured_default_model.trim() {
            "" => FALLBACK_MODEL.to_owned(),
            model => model.to_owned(),
        };
        let retry = RetryBudget {
            max_retries: max_retries
                .map(RetryOverride::value)
                .unwrap_or(DEFAULT_MAX_RETRIES),
            source: if max_retries.is_some() {
                RetrySource::Config
            } else {
                RetrySource::Default
            },
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        };
        Self {
            transport,
            stream_client,
            api_key,
            endpoint,
            default_model,
            retry,
            enabled,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn default_model(&self) -> &str {
        &self.default_model
    }

    pub fn max_retries(&self) -> u32 {
        self.retry.max_retries
    }

    pub fn retry_budget(&self) -> RetryBudget {
        self.retry
    }

    /// Sends a raw OpenAI-compatible chat completion request and returns the
    /// decoded JSON response unchanged. The object form intentionally keeps
    /// the request extensible for tools, response formats, and gateway
    /// parameters that evolve independently of this crate.
    ///
    /// If `model` is absent, null, or an empty string, the configured default
    /// model is inserted. Other fields are passed through unchanged.
    pub async fn chat(&self, request: serde_json::Value) -> Result<serde_json::Value, Error> {
        if !self.enabled {
            return Err(Error::NotConfigured);
        }
        let request = self.prepare_chat_request(request)?;

        match tokio::time::timeout(DEFAULT_REQUEST_TIMEOUT, self.chat_inner(request)).await {
            Ok(result) => result,
            Err(_) => Err(Error::Timeout),
        }
    }

    async fn chat_inner(&self, request: serde_json::Value) -> Result<serde_json::Value, Error> {
        let response = self.post_with_retries(&request).await?;
        if !response.status.is_success() {
            return Err(Error::Upstream(response.status));
        }
        serde_json::from_slice(&response.body)
            .map_err(|error| Error::InvalidResponse(error.to_string()))
    }

    /// Opens an OpenAI-compatible streaming chat completion. The response is
    /// returned after the upstream accepts the request; callers own the
    /// response body and can consume its SSE bytes with `bytes_stream()`.
    /// Unlike [`Client::chat`], no implicit deadline is imposed because the
    /// caller owns the stream lifetime and should bound both this future and
    /// the returned body according to its request lifecycle.
    pub async fn chat_stream(
        &self,
        request: serde_json::Value,
    ) -> Result<reqwest::Response, Error> {
        if !self.enabled {
            return Err(Error::NotConfigured);
        }
        let mut request = self.prepare_chat_request(request)?;
        let Some(object) = request.as_object_mut() else {
            return Err(Error::InvalidRequest(
                "chat request must be a JSON object".into(),
            ));
        };
        object.insert("stream".into(), serde_json::Value::Bool(true));

        let http = self
            .stream_client
            .as_ref()
            .ok_or(Error::ClientUnavailable)?;
        self.post_stream_with_retries(http, &request).await
    }

    async fn post_stream_with_retries(
        &self,
        http: &reqwest::Client,
        request: &serde_json::Value,
    ) -> Result<reqwest::Response, Error> {
        // The retry budget applies only until the upstream returns a
        // successful response. Once headers are returned, the response body
        // belongs to the caller and cannot be replayed safely here.
        for attempt in 0..=self.retry.max_retries {
            let mut builder = http.post(&self.endpoint).json(request);
            if !self.api_key.is_empty() {
                builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", self.api_key));
            }

            match builder.send().await {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response) => {
                    let status = response.status();
                    let retry_after = retry_after(response.headers());
                    let should_retry = retry_directive(response.headers())
                        .unwrap_or_else(|| retryable_status(status));
                    if attempt == self.retry.max_retries || !should_retry {
                        return Err(Error::Upstream(status));
                    }
                    // Drop the failed response before waiting and opening the
                    // next connection; its body is intentionally not exposed.
                    drop(response);
                    tokio::time::sleep(retry_after.unwrap_or_else(|| retry_delay(attempt))).await;
                }
                Err(error) => {
                    if attempt == self.retry.max_retries || !retryable_error(&error) {
                        return Err(Error::Request(error));
                    }
                    tokio::time::sleep(retry_delay(attempt)).await;
                }
            }
        }
        unreachable!("inclusive streaming retry loop always returns")
    }

    fn prepare_chat_request(
        &self,
        mut request: serde_json::Value,
    ) -> Result<serde_json::Value, Error> {
        let Some(object) = request.as_object_mut() else {
            return Err(Error::InvalidRequest(
                "chat request must be a JSON object".into(),
            ));
        };
        if matches!(object.get("stream"), Some(serde_json::Value::Bool(true))) {
            return Err(Error::InvalidRequest(
                "chat request must be non-streaming; use the streaming chat API".into(),
            ));
        }
        let needs_model = match object.get("model") {
            None | Some(serde_json::Value::Null) => true,
            Some(serde_json::Value::String(model)) => model.trim().is_empty(),
            Some(_) => false,
        };
        if needs_model {
            object.insert(
                "model".into(),
                serde_json::Value::String(self.default_model.clone()),
            );
        }
        Ok(request)
    }

    /// Sends one system/user chat completion and returns the first choice.
    /// The caller's deadline bounds the entire retry sequence.
    pub async fn generate_text(
        &self,
        model: &str,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, Error> {
        if !self.enabled {
            return Err(Error::NotConfigured);
        }
        match tokio::time::timeout(
            DEFAULT_REQUEST_TIMEOUT,
            self.generate_text_inner(model, system_prompt, user_prompt),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(Error::Timeout),
        }
    }

    async fn generate_text_inner(
        &self,
        model: &str,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, Error> {
        let mut messages = Vec::with_capacity(2);
        if !system_prompt.trim().is_empty() {
            messages.push(Message {
                role: "system",
                content: system_prompt,
            });
        }
        messages.push(Message {
            role: "user",
            content: user_prompt,
        });
        let request = serde_json::to_value(CompletionRequest {
            model: if model.trim().is_empty() {
                &self.default_model
            } else {
                model.trim()
            },
            messages,
            response_format: None,
            temperature: None,
            max_completion_tokens: None,
            max_tokens: None,
            reasoning_effort: None,
        })
        .map_err(|error| Error::InvalidRequest(error.to_string()))?;

        let response = self.post_with_retries(&request).await?;
        if !response.status.is_success() {
            return Err(Error::Upstream(response.status));
        }
        first_choice(&response.body)
    }

    /// Structured sibling of [`Client::generate_text`]. The preferred request
    /// matches Go's quick-actions contract and narrowly negotiates legacy
    /// gateways only when their 400 response identifies an unsupported field.
    pub async fn generate_json(
        &self,
        model: &str,
        system_prompt: &str,
        user_prompt: &str,
        temperature: f64,
        max_completion_tokens: i64,
    ) -> Result<String, Error> {
        if !self.enabled {
            return Err(Error::NotConfigured);
        }
        match tokio::time::timeout(
            DEFAULT_REQUEST_TIMEOUT,
            self.generate_json_inner(
                model,
                system_prompt,
                user_prompt,
                temperature,
                max_completion_tokens,
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(Error::Timeout),
        }
    }

    async fn generate_json_inner(
        &self,
        model: &str,
        system_prompt: &str,
        user_prompt: &str,
        temperature: f64,
        max_completion_tokens: i64,
    ) -> Result<String, Error> {
        let mut messages = Vec::with_capacity(2);
        if !system_prompt.trim().is_empty() {
            messages.push(Message {
                role: "system",
                content: system_prompt,
            });
        }
        messages.push(Message {
            role: "user",
            content: user_prompt,
        });
        let effective_model = if model.trim().is_empty() {
            self.default_model.as_str()
        } else {
            model.trim()
        };
        let mut request = CompletionRequest {
            model: effective_model,
            messages,
            response_format: Some(ResponseFormat {
                type_: "json_object",
            }),
            temperature: (temperature > 0.0 && !is_gpt_56_family(effective_model))
                .then_some(temperature),
            max_completion_tokens: (max_completion_tokens > 0).then_some(max_completion_tokens),
            max_tokens: None,
            reasoning_effort: is_gpt_56_family(effective_model).then_some("none"),
        };

        for compatibility_retries in 0..=2 {
            let request_value = serde_json::to_value(&request)
                .map_err(|error| Error::InvalidRequest(error.to_string()))?;
            let response = self.post_with_retries(&request_value).await?;
            if response.status.is_success() {
                let completion: CompletionResponse = serde_json::from_slice(&response.body)
                    .map_err(|error| Error::InvalidResponse(error.to_string()))?;
                let choice = completion
                    .choices
                    .into_iter()
                    .next()
                    .ok_or(Error::NoChoices)?;
                if choice.finish_reason.as_deref() == Some("length") {
                    return Err(Error::InvalidResponse(
                        "upstream reached the max completion token limit before producing complete JSON"
                            .into(),
                    ));
                }
                if choice.message.content.trim().is_empty() {
                    return Err(Error::InvalidResponse(
                        "upstream returned empty JSON content".into(),
                    ));
                }
                return Ok(choice.message.content);
            }

            if compatibility_retries < 2
                && request.max_completion_tokens.is_some()
                && is_unsupported_parameter(&response.body, "max_completion_tokens")
            {
                request.max_completion_tokens = None;
                request.max_tokens = (max_completion_tokens > 0).then_some(max_completion_tokens);
                continue;
            }
            if compatibility_retries < 2
                && request.reasoning_effort.is_some()
                && is_unsupported_parameter(&response.body, "reasoning_effort")
            {
                request.reasoning_effort = None;
                continue;
            }
            return Err(Error::Upstream(response.status));
        }
        unreachable!("bounded compatibility loop always returns")
    }

    async fn post_with_retries(
        &self,
        request: &serde_json::Value,
    ) -> Result<TransportResponse, Error> {
        let transport = self.transport.as_ref().ok_or(Error::ClientUnavailable)?;

        for attempt in 0..=self.retry.max_retries {
            match transport.post(&self.endpoint, &self.api_key, request).await {
                Ok(response) if response.status.is_success() => return Ok(response),
                Ok(response) => {
                    let status = response.status;
                    let retry_after = retry_after(&response.headers);
                    let should_retry = retry_directive(&response.headers)
                        .unwrap_or_else(|| retryable_status(status));
                    // Never retain or expose the response body: gateways can
                    // echo private prompts or sensitive diagnostics there.
                    if attempt == self.retry.max_retries || !should_retry {
                        return Ok(response);
                    }
                    tokio::time::sleep(retry_after.unwrap_or_else(|| retry_delay(attempt))).await;
                }
                Err(error) => {
                    if attempt == self.retry.max_retries || !retryable_error(&error) {
                        return Err(Error::Request(error));
                    }
                    tokio::time::sleep(retry_delay(attempt)).await;
                }
            }
        }
        unreachable!("inclusive retry loop always returns")
    }
}

fn first_choice(body: &[u8]) -> Result<String, Error> {
    let completion: CompletionResponse =
        serde_json::from_slice(body).map_err(|error| Error::InvalidResponse(error.to_string()))?;
    completion
        .choices
        .into_iter()
        .next()
        .map(|choice| choice.message.content)
        .ok_or(Error::NoChoices)
}

struct TransportResponse {
    status: StatusCode,
    headers: header::HeaderMap,
    body: Vec<u8>,
}

#[async_trait::async_trait]
trait Transport: Send + Sync {
    async fn post(
        &self,
        endpoint: &str,
        api_key: &str,
        request: &serde_json::Value,
    ) -> Result<TransportResponse, reqwest::Error>;
}

struct ReqwestTransport(reqwest::Client);

#[async_trait::async_trait]
impl Transport for ReqwestTransport {
    async fn post(
        &self,
        endpoint: &str,
        api_key: &str,
        request: &serde_json::Value,
    ) -> Result<TransportResponse, reqwest::Error> {
        let mut builder = self.0.post(endpoint).json(request);
        if !api_key.is_empty() {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {api_key}"));
        }
        let response = builder.send().await?;
        let status = response.status();
        let headers = response.headers().clone();
        // Error bodies are retained only long enough to inspect the structured
        // `param`/`code` compatibility signal. They are never exposed through
        // [`Error`] or logs because gateways can echo private prompts.
        let body = response.bytes().await?.to_vec();
        Ok(TransportResponse {
            status,
            headers,
            body,
        })
    }
}

fn retryable_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT | StatusCode::CONFLICT | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
}

fn retry_directive(headers: &header::HeaderMap) -> Option<bool> {
    headers
        .get("x-should-retry")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        })
}

fn retry_after(headers: &header::HeaderMap) -> Option<Duration> {
    if let Some(milliseconds) = headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
    {
        return Some(Duration::from_millis(milliseconds));
    }
    let value = headers.get(header::RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let deadline = httpdate::parse_http_date(value).ok()?;
    deadline.duration_since(std::time::SystemTime::now()).ok()
}

fn retryable_error(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_timeout() || error.is_request()
}

fn retry_delay(attempt: u32) -> Duration {
    // Matches the OpenAI SDK's documented 0.5s exponential curve and 8s cap.
    use rand::Rng;

    let base = 500_u64.saturating_mul(1_u64 << attempt.min(4));
    let jitter = rand::thread_rng().gen_range(0..=(base / 4));
    Duration::from_millis(base.saturating_sub(jitter))
}

#[derive(Serialize)]
struct CompletionRequest<'a> {
    model: &'a str,
    messages: Vec<Message<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'static str>,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    type_: &'static str,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct CompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: CompletionMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct CompletionMessage {
    content: String,
}

#[derive(Deserialize)]
struct UpstreamErrorEnvelope {
    error: UpstreamErrorDetail,
}

#[derive(Deserialize)]
struct UpstreamErrorDetail {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    param: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

fn is_unsupported_parameter(body: &[u8], parameter: &str) -> bool {
    let Ok(envelope) = serde_json::from_slice::<UpstreamErrorEnvelope>(body) else {
        return false;
    };
    envelope.error.param.as_deref() == Some(parameter)
        && (envelope.error.code.as_deref() == Some("unsupported_parameter")
            || (envelope
                .error
                .code
                .as_deref()
                .unwrap_or_default()
                .is_empty()
                && envelope
                    .error
                    .message
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase()
                    .starts_with("unsupported parameter")))
}

fn is_gpt_56_family(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model == "gpt-5.6" || model.starts_with("gpt-5.6-")
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };

    use serde_json::Value;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;

    #[derive(Default)]
    struct RecordingTransport {
        requests: AtomicUsize,
        endpoint: Mutex<String>,
        api_key: Mutex<String>,
        body: Mutex<Option<Value>>,
    }

    #[async_trait::async_trait]
    impl Transport for RecordingTransport {
        async fn post(
            &self,
            endpoint: &str,
            api_key: &str,
            request: &serde_json::Value,
        ) -> Result<TransportResponse, reqwest::Error> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut captured) = self.endpoint.lock() {
                *captured = endpoint.to_owned();
            }
            if let Ok(mut captured) = self.api_key.lock() {
                *captured = api_key.to_owned();
            }
            if let Ok(mut captured) = self.body.lock() {
                *captured = Some(request.clone());
            }
            Ok(TransportResponse {
                status: StatusCode::OK,
                headers: header::HeaderMap::new(),
                body: br#"{"choices":[{"message":{"content":"Semantic title"}}]}"#.to_vec(),
            })
        }
    }

    fn retry_override(value: u32) -> RetryOverride {
        retries(i64::from(value)).expect("test retry budget is valid")
    }

    #[tokio::test]
    async fn disabled_client_makes_zero_outbound_requests() {
        let transport = Arc::new(RecordingTransport::default());
        let mut client = Client::new(Config {
            default_model: "configured-model".into(),
            ..Config::default()
        });
        client.transport = Some(transport.clone());
        assert!(!client.enabled());
        assert!(matches!(
            client.generate_text("", "system", "private opening").await,
            Err(Error::NotConfigured)
        ));
        assert!(matches!(
            client
                .chat_stream(serde_json::json!({
                    "messages": [{"role": "user", "content": "private opening"}]
                }))
                .await,
            Err(Error::NotConfigured)
        ));
        assert_eq!(transport.requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn sends_openai_compatible_request_with_default_model() {
        let transport = Arc::new(RecordingTransport::default());
        let mut client = Client::new(Config {
            api_key: "test-key".into(),
            base_url: "https://gateway.example/v1".into(),
            default_model: "configured-model".into(),
            max_retries: Some(retry_override(0)),
            http_client: None,
        });
        client.transport = Some(transport.clone());
        let result = client.generate_text("", "system", "private opening").await;
        assert!(matches!(result.as_deref(), Ok("Semantic title")));
        assert_eq!(transport.requests.load(Ordering::SeqCst), 1);
        assert_eq!(
            transport.endpoint.lock().map(|v| v.clone()).ok().as_deref(),
            Some("https://gateway.example/v1/chat/completions")
        );
        assert_eq!(
            transport.api_key.lock().map(|v| v.clone()).ok().as_deref(),
            Some("test-key")
        );
        let body = transport.body.lock().ok().and_then(|body| body.clone());
        assert_eq!(
            body.as_ref().map(|body| &body["model"]),
            Some(&Value::String("configured-model".into()))
        );
        assert_eq!(
            body.as_ref().map(|body| &body["messages"][0]["role"]),
            Some(&Value::String("system".into()))
        );
        assert_eq!(
            body.as_ref().map(|body| &body["messages"][1]["content"]),
            Some(&Value::String("private opening".into()))
        );
    }

    #[tokio::test]
    async fn raw_chat_applies_default_model_and_preserves_request_fields() {
        let transport = Arc::new(RecordingTransport::default());
        let mut client = Client::new(Config {
            api_key: "test-key".into(),
            base_url: "https://gateway.example/v1".into(),
            default_model: "configured-model".into(),
            max_retries: Some(retry_override(0)),
            http_client: None,
        });
        client.transport = Some(transport.clone());

        let response = client
            .chat(serde_json::json!({
                "messages": [{"role": "user", "content": "hi"}],
                "response_format": {"type": "json_object"}
            }))
            .await
            .unwrap();
        assert_eq!(
            response["choices"][0]["message"]["content"],
            "Semantic title"
        );

        let body = transport.body.lock().ok().and_then(|body| body.clone());
        assert_eq!(
            body.as_ref().map(|body| &body["model"]),
            Some(&Value::String("configured-model".into()))
        );
        assert_eq!(
            body.as_ref().map(|body| &body["response_format"]["type"]),
            Some(&Value::String("json_object".into()))
        );
    }

    #[tokio::test]
    async fn raw_chat_respects_request_model_and_rejects_non_object() {
        let transport = Arc::new(RecordingTransport::default());
        let mut client = Client::new(Config {
            api_key: "test-key".into(),
            base_url: "https://gateway.example/v1".into(),
            default_model: "configured-model".into(),
            max_retries: Some(retry_override(0)),
            http_client: None,
        });
        client.transport = Some(transport.clone());

        client
            .chat(serde_json::json!({
                "model": "caller-model",
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .await
            .unwrap();
        let body = transport.body.lock().ok().and_then(|body| body.clone());
        assert_eq!(
            body.as_ref().map(|body| &body["model"]),
            Some(&Value::String("caller-model".into()))
        );

        let error = client.chat(serde_json::json!("not-an-object")).await;
        assert!(
            matches!(error, Err(Error::InvalidRequest(message)) if message.contains("JSON object"))
        );
    }

    #[tokio::test]
    async fn raw_chat_rejects_streaming_requests_before_network() {
        let transport = Arc::new(RecordingTransport::default());
        let mut client = Client::new(Config {
            api_key: "test-key".into(),
            base_url: "https://gateway.example/v1".into(),
            max_retries: Some(0),
            ..Config::default()
        });
        client.transport = Some(transport.clone());

        let error = client
            .chat(serde_json::json!({
                "stream": true,
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .await;
        assert!(
            matches!(error, Err(Error::InvalidRequest(message)) if message.contains("non-streaming"))
        );
        assert_eq!(transport.requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn raw_chat_stream_sets_stream_flag_and_returns_sse_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2048];
            for _ in 0..8 {
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request
                    .windows(b"\"stream\":true".len())
                    .any(|window| window == b"\"stream\":true")
                {
                    break;
                }
            }
            assert!(
                request
                    .windows(b"\"stream\":true".len())
                    .any(|window| window == b"\"stream\":true"),
                "stream flag missing from request: {}",
                String::from_utf8_lossy(&request)
            );
            assert!(
                request
                    .windows(b"\"model\":\"stream-model\"".len())
                    .any(|window| window == b"\"model\":\"stream-model\""),
                "model missing from request: {}",
                String::from_utf8_lossy(&request)
            );
            let request_text = String::from_utf8_lossy(&request).to_ascii_lowercase();
            assert!(
                request_text
                    .lines()
                    .any(|line| line == "authorization: bearer test-key"),
                "authorization header missing from request: {}",
                String::from_utf8_lossy(&request)
            );
            socket
                .write_all(
                    concat!(
                        "HTTP/1.1 200 OK\r\n",
                        "Content-Type: text/event-stream\r\n",
                        "\r\n",
                        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\"}\n\n",
                        "data: [DONE]\n\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });

        let client = Client::new(Config {
            api_key: "test-key".into(),
            base_url: format!("http://{address}"),
            default_model: "stream-model".into(),
            max_retries: Some(retry_override(0)),
            http_client: None,
        });
        let response = client
            .chat_stream(serde_json::json!({
                "messages": [{"role": "user", "content": "hi"}],
                "stream": false
            }))
            .await
            .unwrap();
        assert_eq!(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/event-stream")
        );
        let body = response.bytes().await.unwrap();
        assert!(body
            .windows(b"data: [DONE]".len())
            .any(|window| window == b"data: [DONE]"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn configured_http_client_replaces_default_transport() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = socket
                .write_all(
                    concat!(
                        "HTTP/1.1 200 OK\r\n",
                        "Content-Type: application/json\r\n",
                        "\r\n",
                        "{\"choices\":[{\"message\":{\"content\":\"late\"}}]}"
                    )
                    .as_bytes(),
                )
                .await;
        });
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_millis(10))
            .build()
            .unwrap();
        let client = Client::new(Config {
            api_key: "test-key".into(),
            base_url: format!("http://{address}"),
            max_retries: Some(retry_override(0)),
            http_client: Some(http_client),
            ..Config::default()
        });

        let error = client
            .chat(serde_json::json!({
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .await
            .expect_err("the injected ten-millisecond client must time out");
        assert!(matches!(error, Error::Request(error) if error.is_timeout()));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn raw_chat_stream_retries_before_returning_the_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for attempt in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 2048];
                for _ in 0..8 {
                    let read = socket.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if request
                        .windows(b"\"stream\":true".len())
                        .any(|window| window == b"\"stream\":true")
                    {
                        break;
                    }
                }
                assert!(
                    String::from_utf8_lossy(&request).contains("\"stream\":true"),
                    "stream flag missing from attempt {attempt}"
                );
                let response = if attempt == 0 {
                    concat!(
                        "HTTP/1.1 503 Service Unavailable\r\n",
                        "Content-Length: 0\r\n",
                        "Connection: close\r\n",
                        "x-should-retry: true\r\n",
                        "retry-after-ms: 0\r\n",
                        "\r\n"
                    )
                } else {
                    concat!(
                        "HTTP/1.1 200 OK\r\n",
                        "Content-Type: text/event-stream\r\n",
                        "Connection: close\r\n",
                        "\r\n",
                        "data: [DONE]\n\n"
                    )
                };
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let client = Client::new(Config {
            api_key: "test-key".into(),
            base_url: format!("http://{address}"),
            default_model: "stream-model".into(),
            max_retries: Some(retry_override(1)),
            http_client: None,
        });
        let response = client
            .chat_stream(serde_json::json!({
                "messages": [{"role": "user", "content": "hi"}]
            }))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(String::from_utf8(response.bytes().await.unwrap().to_vec())
            .unwrap()
            .contains("data: [DONE]"));
        server.await.unwrap();
    }

    #[test]
    fn defaults_match_assist_contract() {
        let client = Client::new(Config::default());
        assert_eq!(client.default_model(), FALLBACK_MODEL);
        assert_eq!(client.max_retries(), DEFAULT_MAX_RETRIES);
        assert_eq!(
            client.retry_budget(),
            RetryBudget {
                max_retries: DEFAULT_MAX_RETRIES,
                source: RetrySource::Default,
                request_timeout: DEFAULT_REQUEST_TIMEOUT,
            }
        );
    }

    #[test]
    fn retries_validate_values_and_preserve_explicit_zero() {
        assert_eq!(retries(-1), Err(RetryConfigError::Negative(-1)));
        assert_eq!(
            retries(i64::from(u32::MAX) + 1),
            Err(RetryConfigError::TooLarge(i64::from(u32::MAX) + 1))
        );

        let zero = retries(0).expect("zero is a valid explicit override");
        assert_eq!(zero.value(), 0);
        let client = Client::new(Config {
            max_retries: Some(zero),
            ..Config::default()
        });
        assert_eq!(
            client.retry_budget(),
            RetryBudget {
                max_retries: 0,
                source: RetrySource::Config,
                request_timeout: DEFAULT_REQUEST_TIMEOUT,
            }
        );
    }

    #[tokio::test]
    async fn malformed_config_never_falls_back_to_openai() {
        let client = Client::new(Config {
            base_url: "not a URL".into(),
            max_retries: Some(retry_override(0)),
            ..Config::default()
        });
        assert!(client.enabled());
        assert!(matches!(
            client.generate_text("", "system", "private").await,
            Err(Error::Request(_))
        ));
    }

    #[test]
    fn retry_headers_override_status_and_parse_delays() {
        let mut headers = header::HeaderMap::new();
        headers.insert("x-should-retry", header::HeaderValue::from_static("false"));
        assert_eq!(retry_directive(&headers), Some(false));
        headers.insert("x-should-retry", header::HeaderValue::from_static("true"));
        assert_eq!(retry_directive(&headers), Some(true));
        headers.insert("retry-after-ms", header::HeaderValue::from_static("125"));
        assert_eq!(retry_after(&headers), Some(Duration::from_millis(125)));
    }

    #[test]
    fn upstream_error_does_not_expose_response_body() {
        let error = Error::Upstream(StatusCode::TOO_MANY_REQUESTS);
        assert!(!error.to_string().contains("private prompt"));
        assert_eq!(
            error.to_string(),
            "llm: upstream returned HTTP 429 Too Many Requests"
        );
    }
}
