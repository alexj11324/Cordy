//! HTTP client foundation ported from `server/internal/cli/client.go`.

mod api_error;

pub(crate) use api_error::{
    classify_network_error, normalized_os, read_http_error, DEFAULT_HTTP_TIMEOUT,
};
pub use api_error::{http_timeout, ErrorKind, HealthProbeError, HttpError, NetworkError};

use anyhow::{Context, Result};
use reqwest::{header::HeaderMap, Client, Method, RequestBuilder, StatusCode};
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;

const CLIENT_CAPABILITIES: &str = "stable_attachment_urls";

#[derive(Debug)]
pub struct ApiClient {
    pub(super) base_url: String,
    workspace_id: String,
    token: String,
    agent_id: String,
    task_id: String,
    version: &'static str,
    pub(super) request_timeout: Option<Duration>,
    pub(super) client: Client,
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

    pub(super) fn request(&self, method: Method, path: &str) -> RequestBuilder {
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

    pub(super) async fn send_json<T: DeserializeOwned>(
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
}
