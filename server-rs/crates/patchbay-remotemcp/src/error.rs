//! Error taxonomy for the remote MCP security boundary.
//!
//! Messages remain stable so service and client logs can be correlated.

use std::net::IpAddr;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("parse endpoint: {0}")]
    ParseEndpoint(String),

    #[error("endpoint must be a public HTTPS URL without userinfo, query, or fragment")]
    NotPublicHttps,

    #[error("endpoint host is not public")]
    HostNotPublic,

    #[error("endpoint host is outside the Plugin endpoint policy")]
    HostOutsidePolicy,

    #[error("resolve endpoint host: {0}")]
    Resolve(String),

    #[error("endpoint host resolves to non-public address {0}")]
    NonPublicResolved(IpAddr),

    #[error("remote MCP redirect changed endpoint host")]
    RedirectChangedHost,

    #[error("remote MCP endpoint resolved to a non-public address")]
    DialNonPublic,

    #[error("connect to remote MCP endpoint failed")]
    ConnectFailed,

    #[error("remote MCP call timed out")]
    CallTimeout,

    #[error("TLS configuration error: {0}")]
    Tls(String),

    #[error("request failed: {0}")]
    Request(String),

    #[error("invalid request URI: {0}")]
    InvalidUri(String),

    #[error("remote MCP response exceeds size limit")]
    ResponseTooLarge,
}
