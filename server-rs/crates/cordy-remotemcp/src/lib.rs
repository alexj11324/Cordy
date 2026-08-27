//! Remote MCP client primitives: public-endpoint validation (the SSRF guard)
//! and secure HTTPS client construction.
//!
//! Rust port of the service-layer subset of `server/pkg/remotemcp/`.
//! Consumers: the cordy-service plugin hook engine making outbound
//! HMAC-signed POSTs to manifest-declared hosts.
//!
mod client;
mod connector;
mod devorigin;
mod discover;
mod error;
mod oauth;
mod types;
mod validate;

pub use client::{new_secure_http_client, RequestBody, SecureHttpClient, MAX_RESPONSE_BYTES};
pub use connector::{CALL_TIMEOUT, CONNECT_TIMEOUT};
pub use devorigin::{dev_tls_config, is_dev_origin, DEV_CA_ENV, DEV_ORIGINS_ENV};
pub use discover::{
    contains_string, discover, supported_protocol_versions, tool_set_digest, ExtraHeaders,
};
pub use error::Error;
pub use oauth::{
    build_authorization_url, discover_oauth, exchange_oauth_code, oauth_expiry,
    refresh_oauth_token, register_oauth_client, validate_oauth_metadata, OAuthClientRegistration,
    OAuthMetadata, OAuthRegistration, OAuthToken, OAuthTokenResponse, Registration, Token,
};
pub use types::{digest_bytes, Connection, Tool, PLUGIN_CONTRIBUTION_PREFIX};
pub use validate::{
    host_allowed, is_public_address, validate_public_https_endpoint, Resolver, SystemResolver,
};
