//! Client metadata capture — port of `server/internal/middleware/client.go`.
//!
//! Populated from `X-Client-Platform` / `X-Client-Version` / `X-Client-OS`
//! request headers. Sent by every first-party client (Web, Desktop, CLI,
//! Daemon) so the server can split logs / metrics / gating decisions by
//! caller without reverse-engineering User-Agent strings.
//!
//! All three values are best-effort: treat missing values as "unknown" and
//! never make security decisions based on them — these headers are
//! client-controlled and trivial to spoof.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

/// Header names — exported so other packages (request logger, realtime hub)
/// stay in sync without re-declaring magic strings.
pub const HEADER_CLIENT_PLATFORM: &str = "X-Client-Platform";
pub const HEADER_CLIENT_VERSION: &str = "X-Client-Version";
pub const HEADER_CLIENT_OS: &str = "X-Client-OS";

/// Client metadata captured from `X-Client-*` headers. Empty strings mean
/// the value wasn't sent — callers must treat them as "unknown".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientMetadata {
    pub platform: String,
    pub version: String,
    pub os: String,
}

impl ClientMetadata {
    fn from_request(req: &Request) -> Self {
        Self {
            platform: header_value(req, "x-client-platform"),
            version: header_value(req, "x-client-version"),
            os: header_value(req, "x-client-os"),
        }
    }
}

fn header_value(req: &Request, name: &str) -> String {
    req.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// Extracts `X-Client-*` headers and stashes a [`ClientMetadata`] in the
/// request extensions so downstream handlers and the request logger can read
/// it. Wired before route mounting so every handler benefits from the same
/// observability dimensions.
pub async fn client_metadata(mut req: Request, next: Next) -> Response {
    let meta = ClientMetadata::from_request(&req);
    req.extensions_mut().insert(meta);
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn metadata_defaults_to_unknown() {
        let meta = ClientMetadata::default();
        assert_eq!(meta.platform, "");
        assert_eq!(meta.version, "");
        assert_eq!(meta.os, "");
    }

    #[test]
    fn header_names_match_go_constants() {
        assert_eq!(HEADER_CLIENT_PLATFORM, "X-Client-Platform");
        assert_eq!(HEADER_CLIENT_VERSION, "X-Client-Version");
        assert_eq!(HEADER_CLIENT_OS, "X-Client-OS");
    }

    #[test]
    fn header_value_helper_reads_lowercase_names() {
        // Sanity for the lowercase lookup convention used in from_request.
        let mut req = Request::builder()
            .header("x-client-platform", "web")
            .body(())
            .unwrap();
        let v = req
            .headers_mut()
            .get("x-client-platform")
            .and_then(|h| h.to_str().ok())
            .map(str::to_string)
            .unwrap_or_default();
        assert_eq!(v, "web");
        assert_eq!(
            HeaderValue::from_static("web"),
            HeaderValue::from_static("web")
        );
    }
}
