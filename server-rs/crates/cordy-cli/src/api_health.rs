use reqwest::{Client, StatusCode};
use std::time::Duration;
use url::Url;

use super::api::{classify_network_error, ApiClient, ErrorKind, HealthProbeError};

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_unsafe_or_unbounded_inputs_before_network() {
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
