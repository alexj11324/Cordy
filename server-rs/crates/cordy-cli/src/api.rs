//! HTTP client foundation ported from `server/internal/cli/client.go`.

mod api_error;

pub(crate) use api_error::{
    classify_network_error, normalized_os, read_http_error, DEFAULT_HTTP_TIMEOUT,
};
pub use api_error::{http_timeout, ErrorKind, HealthProbeError, HttpError, NetworkError};

use anyhow::{Context, Result};
use reqwest::Client;
use std::time::Duration;

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
