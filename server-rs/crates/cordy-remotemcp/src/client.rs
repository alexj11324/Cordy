//! Secure HTTP client construction for validated remote MCP endpoints.
//!
//! Ported from `NewSecureHTTPClient` in `server/pkg/remotemcp/client.go`:
//! no proxy, no redirects (hyper follows none), 30s overall call timeout,
//! and every connection dialed through [`PinnedConnector`] — re-resolved at
//! dial time, pinned to the endpoint host, re-checked against the
//! public-address rule unless the endpoint is an operator-named dev origin.
//!
//! The OAuth discovery/token flows and MCP discovery layer live in sibling
//! modules in this crate; both use this same pinned transport boundary.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use http::{Request, Response};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioTimer};
use rustls::{ClientConfig, RootCertStore};
use url::Url;

use crate::connector::{PinnedConnector, CALL_TIMEOUT};
use crate::error::Error;

/// Maximum response body bytes retained, mirroring Go's `MaxResponseBytes`.
pub const MAX_RESPONSE_BYTES: usize = 4 << 20;

/// Buffered request body accepted by [`SecureHttpClient::send`].
pub type RequestBody = Full<Bytes>;

type LegacyClient = Client<PinnedConnector, RequestBody>;

static DEFAULT_TLS: OnceLock<Arc<ClientConfig>> = OnceLock::new();

/// HTTPS client bound to one validated endpoint.
pub struct SecureHttpClient {
    endpoint: Url,
    inner: LegacyClient,
}

impl SecureHttpClient {
    /// The validated endpoint this client is pinned to.
    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    /// Executes one request against the endpoint.
    ///
    /// Applies the 30s call timeout across send and body read (Go's
    /// `http.Client.Timeout` semantics) and buffers the response body up to
    /// [`MAX_RESPONSE_BYTES`] — bounded memory, like Go's LimitReader.
    pub async fn send(&self, request: Request<RequestBody>) -> Result<Response<Vec<u8>>, Error> {
        tokio::time::timeout(CALL_TIMEOUT, async {
            let response = self
                .inner
                .request(request)
                .await
                .map_err(|error| Error::Request(error.to_string()))?;
            let (parts, body) = response.into_parts();
            let buffered = read_limited(body).await?;
            Ok(Response::from_parts(parts, buffered))
        })
        .await
        .map_err(|_| Error::CallTimeout)?
    }
}

async fn read_limited(mut body: Incoming) -> Result<Vec<u8>, Error> {
    let mut buffered = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| Error::Request(error.to_string()))?;
        if let Ok(data) = frame.into_data() {
            if buffered.len().saturating_add(data.len()) > MAX_RESPONSE_BYTES {
                return Err(Error::ResponseTooLarge);
            }
            buffered.extend_from_slice(&data);
        }
    }
    Ok(buffered)
}

/// Builds the secure client for `endpoint`.
pub fn new_secure_http_client(endpoint: &Url) -> SecureHttpClient {
    // A dev origin trusts an extra CA for this origin, never skips
    // verification: a plugin author's local MCP server still presents a
    // certificate. An unreadable or unparseable CA file falls back to the
    // default roots — failing closed to a certificate error.
    let tls = if crate::devorigin::is_dev_origin(endpoint) {
        crate::devorigin::dev_tls_config().unwrap_or_else(default_tls_config)
    } else {
        default_tls_config()
    };
    let connector = PinnedConnector::new(endpoint, tls);
    let inner = Client::builder(TokioExecutor::new())
        .timer(TokioTimer::new())
        .pool_idle_timeout(Duration::from_secs(30))
        .build(connector);
    SecureHttpClient {
        endpoint: endpoint.clone(),
        inner,
    }
}

pub(crate) fn default_tls_config() -> Arc<ClientConfig> {
    DEFAULT_TLS.get_or_init(build_default_tls_config).clone()
}

fn build_default_tls_config() -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    // Both crypto providers land in the graph (aws-lc-rs via rustls defaults,
    // ring via reqwest's hyper-rustls), so the process-default lookup would be
    // ambiguous and panic. Pin one explicitly.
    let mut config = ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("default protocol versions are always supported")
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_endpoint_and_uses_response_byte_cap() {
        let endpoint = Url::parse("https://mcp.example.com/mcp").unwrap();
        let client = new_secure_http_client(&endpoint);
        assert_eq!(client.endpoint(), &endpoint);
        assert_eq!(MAX_RESPONSE_BYTES, 4 << 20);
    }
}
