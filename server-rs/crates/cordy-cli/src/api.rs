//! HTTP client foundation ported from `server/internal/cli/client.go`.

use anyhow::{Context, Result};
use reqwest::{header::HeaderMap, Client, Method, RequestBuilder, Response, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fmt;
use std::time::Duration;
use thiserror::Error;
use url::Url;

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

/// Errors returned by the unauthenticated setup preflight.  The probe must
/// not leak a bearer token, response body, or an untrusted redirect target in
/// the command error: setup runs before a new profile is persisted.
#[derive(Debug, Error)]
pub enum HealthProbeError {
    #[error("health probe URL is invalid")]
    InvalidUrl,
    #[error("health probe only supports http(s) URLs")]
    UnsupportedScheme,
    #[error("health probe timeout")]
    Timeout,
    #[error("health probe request failed ({kind})")]
    Request { kind: ErrorKind },
    #[error("health endpoint returned HTTP {status_code}")]
    Unhealthy { status_code: u16 },
}

#[derive(Debug)]
pub struct ApiClient {
    base_url: String,
    workspace_id: String,
    token: String,
    agent_id: String,
    task_id: String,
    version: &'static str,
    request_timeout: Option<Duration>,
    client: Client,
}

#[derive(Debug, Deserialize)]
pub struct AttachmentUploadResponse {
    pub id: String,
    #[serde(default)]
    pub markdown_url: String,
    #[serde(default)]
    pub content_type: String,
}

#[derive(Debug, Deserialize)]
pub struct FileUploadResponse {
    pub id: String,
    pub url: String,
}

impl ApiClient {
    /// Probe a deployment before setup changes the persisted profile.
    ///
    /// This is deliberately unauthenticated and bounded by both reqwest's
    /// request timeout and an outer future timeout. Redirects are disabled so
    /// setup cannot silently validate a different host than the one supplied
    /// by the user.
    pub async fn probe_health(
        base_url: &str,
        timeout: Duration,
    ) -> std::result::Result<(), HealthProbeError> {
        if timeout.is_zero() {
            return Err(HealthProbeError::Timeout);
        }
        let mut base = Url::parse(base_url.trim()).map_err(|_| HealthProbeError::InvalidUrl)?;
        match base.scheme() {
            "http" | "https" => {}
            _ => return Err(HealthProbeError::UnsupportedScheme),
        }
        if base.query().is_some() || base.fragment().is_some() {
            return Err(HealthProbeError::InvalidUrl);
        }
        let path = base.path().trim_end_matches('/');
        base.set_path(&format!("{path}/health"));

        let client = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| HealthProbeError::Request {
                kind: ErrorKind::Unknown,
            })?;
        let request = client.get(base);
        let response = tokio::time::timeout(timeout, request.send())
            .await
            .map_err(|_| HealthProbeError::Timeout)?
            .map_err(|source| HealthProbeError::Request {
                kind: classify_network_error(&source),
            })?;
        if response.status() != StatusCode::OK {
            return Err(HealthProbeError::Unhealthy {
                status_code: response.status().as_u16(),
            });
        }
        Ok(())
    }

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
            request_timeout: None,
            client: Client::builder()
                .timeout(timeout)
                .build()
                .context("build HTTP client")?,
        })
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = Some(timeout);
        self
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send_json(Method::GET, path, self.request(Method::GET, path))
            .await
    }

    pub async fn get_json_with_headers<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<(T, HeaderMap)> {
        let response = self
            .request(Method::GET, path)
            .send()
            .await
            .map_err(|source| NetworkError {
                kind: classify_network_error(&source),
                op: format!("GET {path}"),
                source,
            })?;
        if response.status().is_client_error() || response.status().is_server_error() {
            return Err(read_http_error(Method::GET, path, response).await.into());
        }
        let headers = response.headers().clone();
        let value = response.json().await.context("decode API response")?;
        Ok((value, headers))
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

    pub async fn put_json<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        self.send_json(
            Method::PUT,
            path,
            self.request(Method::PUT, path).json(body),
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

    /// Import a local skill archive through the same multipart endpoint used
    /// by the Go CLI. Archive parsing and all decompression/path limits remain
    /// server-side; this method only builds the authenticated request.
    pub async fn import_skill_file<T: DeserializeOwned>(
        &self,
        file_data: Vec<u8>,
        filename: &str,
        on_conflict: &str,
    ) -> Result<T> {
        let filename = std::path::Path::new(filename)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("skill.zip")
            .to_owned();
        let mut form = reqwest::multipart::Form::new().part(
            "file",
            reqwest::multipart::Part::bytes(file_data).file_name(filename),
        );
        if !on_conflict.is_empty() {
            form = form.text("on_conflict", on_conflict.to_owned());
        }
        self.send_json(
            Method::POST,
            "/api/skills/import",
            self.request(Method::POST, "/api/skills/import")
                .multipart(form),
        )
        .await
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        let response = self
            .request(Method::DELETE, path)
            .send()
            .await
            .map_err(|source| NetworkError {
                kind: classify_network_error(&source),
                op: format!("DELETE {path}"),
                source,
            })?;
        if response.status().is_client_error() || response.status().is_server_error() {
            return Err(read_http_error(Method::DELETE, path, response).await.into());
        }
        Ok(())
    }

    pub async fn delete_with_body<B: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<()> {
        let response = self
            .request(Method::DELETE, path)
            .json(body)
            .send()
            .await
            .map_err(|source| NetworkError {
                kind: classify_network_error(&source),
                op: format!("DELETE {path}"),
                source,
            })?;
        if response.status().is_client_error() || response.status().is_server_error() {
            return Err(read_http_error(Method::DELETE, path, response).await.into());
        }
        Ok(())
    }

    pub async fn delete_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send_json(Method::DELETE, path, self.request(Method::DELETE, path))
            .await
    }

    pub async fn delete_json_with_body<B: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<()> {
        let response = self
            .request(Method::DELETE, path)
            .json(body)
            .send()
            .await
            .map_err(|source| NetworkError {
                kind: classify_network_error(&source),
                op: format!("DELETE {path}"),
                source,
            })?;
        if response.status().is_client_error() || response.status().is_server_error() {
            return Err(read_http_error(Method::DELETE, path, response).await.into());
        }
        Ok(())
    }

    pub async fn upload_file(
        &self,
        file_data: Vec<u8>,
        filename: &str,
        issue_id: &str,
    ) -> Result<String> {
        let filename = std::path::Path::new(filename)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(filename)
            .to_string();
        let part = reqwest::multipart::Part::bytes(file_data).file_name(filename);
        let mut form = reqwest::multipart::Form::new().part("file", part);
        if !issue_id.is_empty() {
            form = form.text("issue_id", issue_id.to_string());
        }
        let result: serde_json::Value = self
            .send_json(
                Method::POST,
                "/api/upload-file",
                self.request(Method::POST, "/api/upload-file")
                    .multipart(form),
            )
            .await?;
        let id = result
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if id.is_empty() {
            anyhow::bail!("upload response missing attachment id");
        }
        Ok(id.into())
    }

    pub async fn upload_file_with_url(
        &self,
        file_data: Vec<u8>,
        filename: &str,
    ) -> Result<FileUploadResponse> {
        let filename = std::path::Path::new(filename)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(filename)
            .to_string();
        let form = reqwest::multipart::Form::new().part(
            "file",
            reqwest::multipart::Part::bytes(file_data).file_name(filename),
        );
        let result: FileUploadResponse = self
            .send_json(
                Method::POST,
                "/api/upload-file",
                self.request(Method::POST, "/api/upload-file")
                    .multipart(form),
            )
            .await?;
        if result.id.is_empty() {
            anyhow::bail!("upload response missing attachment id");
        }
        Ok(result)
    }

    pub async fn upload_chat_attachment(
        &self,
        file_data: Vec<u8>,
        filename: &str,
        task_id: &str,
    ) -> Result<AttachmentUploadResponse> {
        let filename = std::path::Path::new(filename)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(filename)
            .to_string();
        let form = reqwest::multipart::Form::new()
            .part(
                "file",
                reqwest::multipart::Part::bytes(file_data).file_name(filename),
            )
            .text("task_id", task_id.to_string());
        let result: AttachmentUploadResponse = self
            .send_json(
                Method::POST,
                "/api/upload-file",
                self.request(Method::POST, "/api/upload-file")
                    .multipart(form),
            )
            .await?;
        if result.id.is_empty() {
            anyhow::bail!("upload response missing attachment id");
        }
        Ok(result)
    }

    pub async fn download_file(&self, download_url: &str) -> Result<Vec<u8>> {
        let relative =
            !download_url.starts_with("http://") && !download_url.starts_with("https://");
        let (url, request) = if relative {
            if self.base_url.is_empty() {
                anyhow::bail!(
                    "download URL {download_url:?} is relative but client has no BaseURL"
                );
            }
            let url = format!("{}{download_url}", self.base_url);
            (url, self.request(Method::GET, download_url))
        } else {
            (
                download_url.to_string(),
                self.client.get(download_url).timeout(
                    self.request_timeout
                        .unwrap_or_else(|| Duration::from_secs(60)),
                ),
            )
        };
        let mut response = request.send().await.map_err(|source| NetworkError {
            kind: classify_network_error(&source),
            op: format!("GET {url}"),
            source,
        })?;
        if response.status().is_client_error() || response.status().is_server_error() {
            return Err(read_http_error(Method::GET, &url, response).await.into());
        }
        const MAX_DOWNLOAD_SIZE: usize = 100 << 20;
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await.context("read download response")? {
            let remaining = MAX_DOWNLOAD_SIZE.saturating_sub(data.len());
            if remaining == 0 {
                break;
            }
            data.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        }
        Ok(data)
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

        if let Some(timeout) = self.request_timeout {
            request = request.timeout(timeout);
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
    classify_network_message(&error_chain_text(error))
}

fn error_chain_text(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = String::new();
    let mut current = Some(error);
    while let Some(err) = current {
        if !message.is_empty() {
            message.push(' ');
        }
        message.push_str(&err.to_string());
        current = err.source();
    }
    message.to_ascii_lowercase()
}

fn classify_network_message(message: &str) -> ErrorKind {
    if message.contains("dns")
        || message.contains("no such host")
        || message.contains("name resolution")
    {
        ErrorKind::NetworkDns
    } else if message.contains("connection refused") {
        ErrorKind::NetworkRefused
    } else if message.contains("tls") || message.contains("certificate") || message.contains("x509")
    {
        ErrorKind::NetworkTls
    } else {
        ErrorKind::NetworkOffline
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

    #[test]
    fn network_classification_inspects_the_source_chain() {
        #[derive(Debug)]
        struct Outer(Inner);
        impl std::fmt::Display for Outer {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("error sending request for url (https://example.test/)")
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        #[derive(Debug)]
        struct Inner;
        impl std::fmt::Display for Inner {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("dns error: no such host")
            }
        }
        impl std::error::Error for Inner {}

        assert_eq!(
            classify_network_message(&error_chain_text(&Outer(Inner))),
            ErrorKind::NetworkDns
        );
        assert_eq!(
            classify_network_message("error sending request for url (https://example.test/)"),
            ErrorKind::NetworkOffline
        );
    }

    #[tokio::test]
    async fn health_probe_rejects_unsafe_or_unbounded_inputs_before_network() {
        assert!(matches!(
            ApiClient::probe_health("ftp://example.test", Duration::from_secs(2)).await,
            Err(HealthProbeError::UnsupportedScheme)
        ));
        assert!(matches!(
            ApiClient::probe_health(
                "https://example.test/health?token=secret",
                Duration::from_secs(2)
            )
            .await,
            Err(HealthProbeError::InvalidUrl)
        ));
        assert!(matches!(
            ApiClient::probe_health("https://example.test", Duration::ZERO).await,
            Err(HealthProbeError::Timeout)
        ));
    }
}
