//! Bounded attachment download transport.
//!
//! Upload multipart construction stays in `api_attachments`; this module
//! owns only relative/absolute URL selection and the download size bound.

use anyhow::{Context, Result};
use reqwest::Method;
use std::time::Duration;

use super::api::{read_http_error, ApiClient, NetworkError};

impl ApiClient {
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
