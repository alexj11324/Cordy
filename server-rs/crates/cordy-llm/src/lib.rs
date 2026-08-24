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

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub api_key: String,
    pub base_url: String,
    pub default_model: String,
    /// `None` uses [`DEFAULT_MAX_RETRIES`]; `Some(0)` disables retries.
    pub max_retries: Option<u32>,
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
    #[error("llm: upstream returned HTTP {0}")]
    Upstream(StatusCode),
    #[error("llm: upstream returned no choices")]
    NoChoices,
}

#[derive(Clone)]
pub struct Client {
    transport: Option<Arc<dyn Transport>>,
    api_key: String,
    endpoint: String,
    default_model: String,
    max_retries: u32,
    enabled: bool,
}

impl Client {
    pub fn new(config: Config) -> Self {
        let api_key = config.api_key.trim().to_owned();
        let configured_base_url = config.base_url.trim();
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
        let transport = reqwest::Client::builder()
            .timeout(DEFAULT_REQUEST_TIMEOUT)
            .build()
            .ok()
            .map(|client| Arc::new(ReqwestTransport(client)) as Arc<dyn Transport>);
        let default_model = match config.default_model.trim() {
            "" => FALLBACK_MODEL.to_owned(),
            model => model.to_owned(),
        };
        Self {
            transport,
            api_key,
            endpoint,
            default_model,
            max_retries: config.max_retries.unwrap_or(DEFAULT_MAX_RETRIES),
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
        self.max_retries
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
        let request = CompletionRequest {
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
        };

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
            let response = self.post_with_retries(&request).await?;
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
        request: &CompletionRequest<'_>,
    ) -> Result<TransportResponse, Error> {
        let transport = self.transport.as_ref().ok_or(Error::ClientUnavailable)?;

        for attempt in 0..=self.max_retries {
            match transport
                .post(&self.endpoint, &self.api_key, &request)
                .await
            {
                Ok(response) if response.status.is_success() => return Ok(response),
                Ok(response) => {
                    let status = response.status;
                    let retry_after = retry_after(&response.headers);
                    let should_retry = retry_directive(&response.headers)
                        .unwrap_or_else(|| retryable_status(status));
                    // Never retain or expose the response body: gateways can
                    // echo private prompts or sensitive diagnostics there.
                    if attempt == self.max_retries || !should_retry {
                        return Ok(response);
                    }
                    tokio::time::sleep(retry_after.unwrap_or_else(|| retry_delay(attempt))).await;
                }
                Err(error) => {
                    if attempt == self.max_retries || !retryable_error(&error) {
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
        request: &CompletionRequest<'_>,
    ) -> Result<TransportResponse, reqwest::Error>;
}

struct ReqwestTransport(reqwest::Client);

#[async_trait::async_trait]
impl Transport for ReqwestTransport {
    async fn post(
        &self,
        endpoint: &str,
        api_key: &str,
        request: &CompletionRequest<'_>,
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
            request: &CompletionRequest<'_>,
        ) -> Result<TransportResponse, reqwest::Error> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut captured) = self.endpoint.lock() {
                *captured = endpoint.to_owned();
            }
            if let Ok(mut captured) = self.api_key.lock() {
                *captured = api_key.to_owned();
            }
            if let (Ok(value), Ok(mut captured)) = (serde_json::to_value(request), self.body.lock())
            {
                *captured = Some(value);
            }
            Ok(TransportResponse {
                status: StatusCode::OK,
                headers: header::HeaderMap::new(),
                body: br#"{"choices":[{"message":{"content":"Semantic title"}}]}"#.to_vec(),
            })
        }
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
        assert_eq!(transport.requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn sends_openai_compatible_request_with_default_model() {
        let transport = Arc::new(RecordingTransport::default());
        let mut client = Client::new(Config {
            api_key: "test-key".into(),
            base_url: "https://gateway.example/v1".into(),
            default_model: "configured-model".into(),
            max_retries: Some(0),
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

    #[test]
    fn defaults_match_assist_contract() {
        let client = Client::new(Config::default());
        assert_eq!(client.default_model(), FALLBACK_MODEL);
        assert_eq!(client.max_retries(), DEFAULT_MAX_RETRIES);
    }

    #[tokio::test]
    async fn malformed_config_never_falls_back_to_openai() {
        let client = Client::new(Config {
            base_url: "not a URL".into(),
            max_retries: Some(0),
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
