//! Dial-time pinned connector for the secure remote MCP HTTP client.
//!
//! Ported from the transport half of `server/pkg/remotemcp/client.go`
//! (`NewSecureHTTPClient`'s `DialContext`). Every connection is re-resolved
//! and re-checked at dial time and pinned to an address that passed the
//! public-address gate — the DNS-rebinding defense.
//!
//! Deviations from Go, both cosmetic: TCP keepalive is dropped (needs
//! socket2; connections live seconds), and hyper never follows redirects so
//! Go's `CheckRedirect` refusal needs no port.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use http::Uri;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::TokioIo;
use rustls::ClientConfig;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;
use tower_service::Service;
use url::Url;

use crate::error::Error;
use crate::validate::{is_public_address, normalize_host, Resolver, SystemResolver};

/// Connect timeout, mirroring Go's `ConnectTimeout`.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-call timeout, mirroring Go's `CallTimeout`.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// TCP stream after scheme-specific wrapping: plain for a dev origin's
/// `http://` endpoint, TLS otherwise.
pub(crate) enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

impl Connection for Stream {
    /// Parity with Go's `Transport.Proxy: nil` — never proxied.
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

#[derive(Clone)]
pub(crate) struct PinnedConnector {
    /// Normalized endpoint host (lowercase, brackets/trailing dot stripped).
    /// The pin: any dial whose URI host differs is refused before it can
    /// resolve, so a redirect pointing somewhere else dies here.
    endpoint_host: Arc<str>,
    dev_origin: bool,
    tls: Arc<ClientConfig>,
}

impl PinnedConnector {
    pub(crate) fn new(endpoint: &Url, tls: Arc<ClientConfig>) -> Self {
        let host = normalize_host(endpoint.host_str().unwrap_or_default());
        Self {
            endpoint_host: Arc::from(host.as_str()),
            // Decided once, here, rather than inside the dialer: the endpoint
            // is fixed for this client's lifetime, so a per-dial lookup could
            // only differ if the environment changed mid-task, and honouring
            // that would be a way to widen the guard after the connection was
            // approved.
            dev_origin: crate::devorigin::is_dev_origin(endpoint),
            tls,
        }
    }

    async fn connect(self, uri: Uri) -> Result<TokioIo<Stream>, Error> {
        let Some(host) = uri.host() else {
            return Err(Error::InvalidUri(uri.to_string()));
        };
        let host = normalize_host(host);
        if host != *self.endpoint_host {
            return Err(Error::RedirectChangedHost);
        }
        let scheme = uri.scheme().map(http::uri::Scheme::as_str);
        let is_https = scheme == Some("https");
        let is_http = scheme == Some("http");
        if !is_https && !is_http {
            return Err(Error::InvalidUri(uri.to_string()));
        }
        if is_http && !self.dev_origin {
            // Validated endpoints are always https; plain http reaching the
            // connector for a non-dev origin means something upstream broke.
            // Fail closed.
            return Err(Error::NotPublicHttps);
        }
        let port = uri.port_u16().unwrap_or(if is_https { 443 } else { 80 });
        let addresses = SystemResolver.lookup_net_ip(&host).await?;
        if addresses.is_empty() {
            return Err(Error::Resolve("no addresses".to_string()));
        }
        let mut last_error: Option<Error> = None;
        for candidate in addresses {
            // The host pin above still holds for a dev origin, so a redirect
            // pointing somewhere else is refused either way. What this skips
            // is only "the address must be on the public internet".
            if !self.dev_origin && !is_public_address(candidate) {
                return Err(Error::DialNonPublic);
            }
            match dial(candidate, port, is_https, &host, &self.tls).await {
                Ok(stream) => return Ok(TokioIo::new(stream)),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or(Error::ConnectFailed))
    }
}

impl Service<Uri> for PinnedConnector {
    type Response = TokioIo<Stream>;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let connector = self.clone();
        Box::pin(async move { connector.connect(uri).await })
    }
}

async fn dial(
    ip: IpAddr,
    port: u16,
    tls: bool,
    sni_host: &str,
    config: &Arc<ClientConfig>,
) -> Result<Stream, Error> {
    let target = SocketAddr::new(ip, port);
    let tcp = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target)).await {
        Ok(Ok(tcp)) => tcp,
        Ok(Err(error)) => return Err(Error::Request(format!("dial {target}: {error}"))),
        Err(_) => return Err(Error::Request(format!("dial {target}: timed out"))),
    };
    if !tls {
        return Ok(Stream::Plain(tcp));
    }
    let name = server_name(sni_host)?;
    let connector = TlsConnector::from(config.clone());
    let stream = connector
        .connect(name, tcp)
        .await
        .map_err(|error| Error::Tls(format!("handshake with {target}: {error}")))?;
    Ok(Stream::Tls(Box::new(stream)))
}

fn server_name(host: &str) -> Result<rustls::pki_types::ServerName<'static>, Error> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(rustls::pki_types::ServerName::IpAddress(ip.into()));
    }
    rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|error| Error::Tls(format!("invalid TLS server name {host}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::default_tls_config;
    use url::Url;

    #[tokio::test]
    async fn refuses_foreign_host_before_resolving() {
        let endpoint = Url::parse("https://mcp.example.com/mcp").unwrap();
        let connector = PinnedConnector::new(&endpoint, default_tls_config());
        let uri: Uri = "https://evil.example.net/mcp".parse().unwrap();
        let error = connector.connect(uri).await.err().expect("must refuse");
        assert!(matches!(error, Error::RedirectChangedHost));
    }

    #[tokio::test]
    async fn refuses_plain_http_for_non_dev_origins() {
        let endpoint = Url::parse("https://mcp.example.com/mcp").unwrap();
        let connector = PinnedConnector::new(&endpoint, default_tls_config());
        let uri: Uri = "http://mcp.example.com/mcp".parse().unwrap();
        let error = connector.connect(uri).await.err().expect("must refuse");
        assert!(matches!(error, Error::NotPublicHttps));
    }
}
