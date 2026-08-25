//! HTTP client foundation ported from `server/internal/cli/client.go`.

pub(crate) use super::api_error::{
    classify_network_error, normalized_os, read_http_error, DEFAULT_HTTP_TIMEOUT,
};
pub use super::api_error::{http_timeout, ErrorKind, HealthProbeError, HttpError, NetworkError};

use anyhow::{Context, Result};
use reqwest::{header::HeaderMap, Client, Method, RequestBuilder, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::time::Duration;
use url::Url;

const CLIENT_CAPABILITIES: &str = "stable_attachment_urls";

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
