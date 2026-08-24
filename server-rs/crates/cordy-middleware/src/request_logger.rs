//! Structured HTTP request logging — port of
//! `server/internal/middleware/request_logger.go`.
//!
//! Port notes (axum vs chi runtime differences):
//! - Body capture for soft-404 classification only intercepts 404 responses
//!   (`to_bytes` with a safety cap); other statuses stream untouched.
//! - The webhook trigger ID rides a RESPONSE header set by the webhook
//!   handler (`x-webhook-trigger-id`); the logger reads and strips it so the
//!   internal header never reaches the client.

use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::client::ClientMetadata;

/// Public webhook ingress path prefix. The path segment after this prefix IS
/// a bearer credential, so the logger must redact it.
pub const WEBHOOK_INGRESS_PATH_PREFIX: &str = "/api/webhooks/autopilots/";

/// Internal response header carrying the resolved webhook trigger ID to the
/// request logger (which strips it before the response leaves the server).
/// Set by the webhook handler right after the trigger row is looked up.
pub const WEBHOOK_TRIGGER_ID_HEADER: &str = "x-webhook-trigger-id";

/// Maximum body bytes inspected when classifying a 404. The JSON error
/// envelope is small — far less is needed to see the "error" field — and the
/// cap means an unbounded handler body cannot blow up logger memory.
const SOFT_NOT_FOUND_BODY_CAPTURE_LIMIT: usize = 256 * 1024;

/// 404 response bodies the daemon emits routinely as part of normal lifecycle
/// events: a runtime deleted from the UI, a task GC'd after an issue was
/// removed, etc. Logging these at Warn turned production stderr into a flood
/// whenever a runtime was deleted (issue #2391). They stay machine-
/// recognizable at Info, while genuine 4xx keep Warn.
const SOFT_NOT_FOUND_MARKERS: [&str; 2] = ["runtime not found", "task not found"];

/// Returns a logger-safe version of a request path. For the autopilot webhook
/// ingress path the trailing token segment is replaced with "[redacted]";
/// every other path passes through untouched.
///
/// Without redaction, every successful webhook delivery prints a replayable
/// URL (`.../awt_<32-byte-base64>`) into the structured log stream.
pub fn redact_webhook_path(path: &str) -> String {
    if !path.starts_with(WEBHOOK_INGRESS_PATH_PREFIX) {
        return path.to_string();
    }
    let rest = &path[WEBHOOK_INGRESS_PATH_PREFIX.len()..];
    if rest.is_empty() {
        return path.to_string();
    }
    // Preserve any sub-path after the token (currently none, but defensive).
    match rest.find('/') {
        Some(slash) => {
            format!("{WEBHOOK_INGRESS_PATH_PREFIX}[redacted]{}", &rest[slash..])
        }
        None => format!("{WEBHOOK_INGRESS_PATH_PREFIX}[redacted]"),
    }
}

/// Reports whether the captured response body matches one of the expected
/// stale-state 404 signals.
fn is_soft_not_found(body: &[u8]) -> bool {
    if body.is_empty() {
        return false;
    }
    let lower = String::from_utf8_lossy(body).to_lowercase();
    SOFT_NOT_FOUND_MARKERS.iter().any(|m| lower.contains(m))
}

/// Return a user id only when the logger can independently verify a JWT.
///
/// The request logger sits outside authentication so it must never trust
/// client-supplied `X-User-ID`: a rejected request could otherwise attribute
/// its warning to an arbitrary victim. JWT bearer/cookie requests can be
/// verified locally with the same HS256 decoder as auth. PAT, task-token and
/// managed-proxy requests deliberately remain unattributed here; their raw
/// forwarding headers are not evidence of identity.
fn verified_jwt_user_id(req: &Request) -> String {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let cookies = req
                .headers()
                .get(header::COOKIE)
                .and_then(|value| value.to_str().ok())?;
            cookies.split(';').find_map(|pair| {
                let (name, value) = pair.trim().split_once('=')?;
                (name == cordy_auth::cookie::AUTH_COOKIE_NAME && !value.is_empty())
                    .then(|| value.to_string())
            })
        });
    let Some(token) = token else {
        return String::new();
    };
    crate::auth::decode_jwt_claims(&token)
        .and_then(|claims| {
            claims
                .get("sub")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// Structured HTTP request logger. Skips the hot liveness endpoint to keep
/// logs readable.
pub async fn request_logger(req: Request, next: Next) -> Response {
    // Skip the hot liveness endpoint.
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }

    let start = std::time::Instant::now();
    let method = req.method().to_string();
    let raw_path = req.uri().path().to_string();
    let request_id = header_str(&req, "x-request-id");
    let user_id = verified_jwt_user_id(&req);
    let meta = req.extensions().get::<ClientMetadata>().cloned();

    let mut res = next.run(req).await;
    let duration = start.elapsed();
    let status = res.status().as_u16();

    // Read + strip the internal trigger-ID header (server-set only).
    let mut trigger_id = String::new();
    if let Some(v) = res.headers_mut().remove(WEBHOOK_TRIGGER_ID_HEADER) {
        if let Ok(s) = v.to_str() {
            trigger_id = s.to_string();
        }
    }

    // Soft-404 classification needs the body — only intercept on 404.
    let mut soft = false;
    if status == StatusCode::NOT_FOUND.as_u16() {
        let (parts, body) = res.into_parts();
        match axum::body::to_bytes(body, SOFT_NOT_FOUND_BODY_CAPTURE_LIMIT).await {
            Ok(bytes) => {
                soft = is_soft_not_found(&bytes);
                res = Response::from_parts(parts, Body::from(bytes));
            }
            Err(e) => {
                // Body exceeded the capture cap — degrade to a generic 404
                // rather than buffer unbounded data. Real 404s are tiny JSON.
                tracing::warn!(error = %e, "request_logger: 404 body exceeded capture cap");
                res = (StatusCode::NOT_FOUND, r#"{"error":"not found"}"#).into_response();
            }
        }
    }

    let default_meta = ClientMetadata::default();
    let meta = meta.as_ref().unwrap_or(&default_meta);
    let path = redact_webhook_path(&raw_path);

    // The event! macro requires a static level path, so branch explicitly;
    // the local macro keeps the field list in one place.
    macro_rules! log_http {
        ($level:path) => {
            tracing::event!(
                $level,
                method = %method,
                path = %path,
                status = status,
                duration = ?duration,
                request_id = %request_id,
                user_id = %user_id,
                webhook_trigger_id = %trigger_id,
                client_platform = %meta.platform,
                client_version = %meta.version,
                client_os = %meta.os,
                "http request",
            );
        };
    }

    if status >= 500 {
        log_http!(tracing::Level::ERROR);
    } else if status == StatusCode::NOT_FOUND.as_u16() && soft {
        // Lifecycle 404 — runtime/task was deleted server-side. The daemon
        // self-heals on this exact body, so it is neither noise nor a bug.
        log_http!(tracing::Level::INFO);
    } else if status >= 400 {
        log_http!(tracing::Level::WARN);
    } else {
        log_http!(tracing::Level::INFO);
    }

    res
}

fn header_str(req: &Request, name: &str) -> String {
    req.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use serde_json::json;

    #[test]
    fn webhook_paths_redact_token_segment() {
        assert_eq!(
            redact_webhook_path("/api/webhooks/autopilots/awt_secrettoken123"),
            "/api/webhooks/autopilots/[redacted]"
        );
        // Sub-path after the token is preserved defensively.
        assert_eq!(
            redact_webhook_path("/api/webhooks/autopilots/awt_x/retry"),
            "/api/webhooks/autopilots/[redacted]/retry"
        );
        // Prefix alone stays intact.
        assert_eq!(
            redact_webhook_path("/api/webhooks/autopilots/"),
            "/api/webhooks/autopilots/"
        );
    }

    #[test]
    fn non_webhook_paths_pass_through() {
        assert_eq!(redact_webhook_path("/api/issues"), "/api/issues");
        assert_eq!(redact_webhook_path("/"), "/");
        assert_eq!(redact_webhook_path(""), "");
    }

    #[test]
    fn soft_not_found_matches_markers_case_insensitively() {
        assert!(is_soft_not_found(b"{\"error\":\"Runtime Not Found\"}"));
        assert!(is_soft_not_found(b"{\"error\":\"task not found\"}"));
        assert!(!is_soft_not_found(b"{\"error\":\"wrong path\"}"));
        assert!(!is_soft_not_found(b""));
    }

    #[test]
    fn spoofed_user_header_is_never_logged_as_identity() {
        let request = Request::builder()
            .uri("/api/me")
            .header("x-user-id", "victim")
            .body(Body::empty())
            .unwrap();
        assert_eq!(verified_jwt_user_id(&request), "");
    }

    #[test]
    fn valid_jwt_identity_is_available_to_outer_logger() {
        let token = encode(
            &Header::new(Algorithm::HS256),
            &json!({"sub":"user-1","exp":chrono::Utc::now().timestamp()+60}),
            &EncodingKey::from_secret(cordy_auth::jwt::jwt_secret().as_bytes()),
        )
        .unwrap();
        let request = Request::builder()
            .uri("/api/me")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(verified_jwt_user_id(&request), "user-1");
    }
}
