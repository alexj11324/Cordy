use anyhow::{Context, Result};
use reqwest::{header::HeaderMap, Method, RequestBuilder};
use serde::{de::DeserializeOwned, Serialize};

use super::api::{classify_network_error, normalized_os, read_http_error, ApiClient, NetworkError};

const CLIENT_CAPABILITIES: &str = "stable_attachment_urls";

impl ApiClient {
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
