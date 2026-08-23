//! HTTP client foundation ported from `server/internal/cli/client.go`.

use anyhow::{Context, Result};
use reqwest::{Client, Method, RequestBuilder, Response};
use serde::{de::DeserializeOwned, Serialize};
use std::fmt;
use std::time::Duration;
use thiserror::Error;

const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const ERROR_BODY_LIMIT: usize = 4096;
const CLIENT_CAPABILITIES: &str = "stable_attachment_urls";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    NetworkTimeout,
    NetworkDns,
    NetworkRefused,
    NetworkTls,
    NetworkOffline,
    AuthRequired,
    Forbidden,
    NotFound,
    Conflict,
    Validation,
    RateLimited,
    Server,
    Unknown,
}

#[derive(Debug, Error)]
#[error("{method} {path} returned {status_code}: {body}")]
pub struct HttpError {
    pub method: Method,
    pub path: String,
    pub status_code: u16,
    pub body: String,
}

impl HttpError {
    pub fn kind(&self) -> ErrorKind {
        match self.status_code {
            401 => ErrorKind::AuthRequired,
            403 => ErrorKind::Forbidden,
            404 => ErrorKind::NotFound,
            409 => ErrorKind::Conflict,
            400 | 422 => ErrorKind::Validation,
            429 => ErrorKind::RateLimited,
            500..=599 => ErrorKind::Server,
            _ => ErrorKind::Unknown,
        }
    }
}

#[derive(Debug, Error)]
#[error("{op}: {source}")]
pub struct NetworkError {
    pub kind: ErrorKind,
    pub op: String,
    #[source]
    pub source: reqwest::Error,
}

#[derive(Debug)]
pub struct ApiClient {
    base_url: String,
    workspace_id: String,
    token: String,
    agent_id: String,
    task_id: String,
    version: &'static str,
    client: Client,
}

impl ApiClient {
    pub fn new(
        base_url: String,
        workspace_id: String,
        token: String,
        agent_id: String,
        task_id: String,
        timeout: Duration,
        version: &'static str,
    ) -> Result<Self> {
        Ok(Self {
            base_url: base_url.trim_end_matches('/').into(),
            workspace_id,
            token,
            agent_id,
            task_id,
            version,
            client: Client::builder()
                .timeout(timeout)
                .build()
                .context("build HTTP client")?,
        })
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send_json(Method::GET, path, self.request(Method::GET, path))
            .await
    }

    pub async fn patch_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.send_json(
            Method::PATCH,
            path,
            self.request(Method::PATCH, path).json(body),
        )
        .await
    }

    pub async fn post_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.send_json(
            Method::POST,
            path,
            self.request(Method::POST, path).json(body),
        )
        .await
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base_url))
            .header("X-Client-Capabilities", CLIENT_CAPABILITIES)
            .header("X-Client-Platform", "cli")
            .header("X-Client-Version", self.version)
            .header("X-Client-OS", normalized_os());
        if !self.token.is_empty() {
            request = request.bearer_auth(&self.token);
        }
        if !self.workspace_id.is_empty() {
            request = request.header("X-Workspace-ID", &self.workspace_id);
        }
        if !self.agent_id.is_empty() {
            request = request.header("X-Agent-ID", &self.agent_id);
        }
        if !self.task_id.is_empty() {
            request = request.header("X-Task-ID", &self.task_id);
        }

        request
    }

    async fn send_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        request: RequestBuilder,
    ) -> Result<T> {
        let response = request.send().await.map_err(|source| NetworkError {
            kind: classify_network_error(&source),
            op: format!("{method} {path}"),
            source,
        })?;
        if response.status().is_client_error() || response.status().is_server_error() {
            return Err(read_http_error(method, path, response).await.into());
        }
        response.json().await.context("decode API response")
    }
}

pub fn http_timeout(raw: Option<&str>) -> Duration {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return DEFAULT_HTTP_TIMEOUT;
    };
    parse_go_duration(raw)
        .or_else(|| raw.parse::<u64>().ok().map(Duration::from_secs))
        .filter(|duration| !duration.is_zero())
        .unwrap_or(DEFAULT_HTTP_TIMEOUT)
}

async fn read_http_error(method: Method, path: &str, mut response: Response) -> HttpError {
    let status_code = response.status().as_u16();
    let mut body = Vec::with_capacity(ERROR_BODY_LIMIT);
    while body.len() < ERROR_BODY_LIMIT {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = ERROR_BODY_LIMIT - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    HttpError {
        method,
        path: path.into(),
        status_code,
        body: String::from_utf8_lossy(&body).trim().into(),
    }
}

fn classify_network_error(error: &reqwest::Error) -> ErrorKind {
    if error.is_timeout() {
        return ErrorKind::NetworkTimeout;
    }
    let message = error.to_string().to_lowercase();
    match () {
        () if message.contains("dns")
            || message.contains("no such host")
            || message.contains("name resolution") =>
        {
            ErrorKind::NetworkDns
        }
        () if message.contains("connection refused") => ErrorKind::NetworkRefused,
        () if message.contains("tls")
            || message.contains("certificate")
            || message.contains("x509") =>
        {
            ErrorKind::NetworkTls
        }
        () => ErrorKind::NetworkOffline,
    }
}

fn parse_go_duration(raw: &str) -> Option<Duration> {
    if raw.is_empty() || raw.starts_with('-') {
        return None;
    }
    let mut rest = raw;
    let mut seconds = 0.0_f64;
    while !rest.is_empty() {
        let number_len = rest
            .char_indices()
            .take_while(|(_, character)| character.is_ascii_digit() || *character == '.')
            .map(|(index, character)| index + character.len_utf8())
            .last()?;
        let value = rest[..number_len].parse::<f64>().ok()?;
        rest = &rest[number_len..];
        let (unit, multiplier) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ]
        .into_iter()
        .find(|(unit, _)| rest.starts_with(unit))?;
        rest = &rest[unit.len()..];
        seconds += value * multiplier;
    }
    (seconds.is_finite() && seconds >= 0.0 && seconds < Duration::MAX.as_secs_f64())
        .then(|| Duration::from_secs_f64(seconds))
}

fn normalized_os() -> &'static str {
    std::env::consts::OS
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_accepts_go_duration_and_bare_seconds() {
        assert_eq!(http_timeout(Some("1m30s")), Duration::from_secs(90));
        assert_eq!(http_timeout(Some("45")), Duration::from_secs(45));
        assert_eq!(http_timeout(Some("0s")), DEFAULT_HTTP_TIMEOUT);
        assert_eq!(http_timeout(Some("nonsense")), DEFAULT_HTTP_TIMEOUT);
    }
}
