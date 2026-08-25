use anyhow::{Context, Result};
use reqwest::Method;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

use super::api::{read_http_error, ApiClient, NetworkError};

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
        let result: Value = self
            .send_json(
                Method::POST,
                "/api/upload-file",
                self.request(Method::POST, "/api/upload-file")
                    .multipart(form),
            )
            .await?;
        let id = result.get("id").and_then(Value::as_str).unwrap_or_default();
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
            kind: super::api::classify_network_error(&source),
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
}
