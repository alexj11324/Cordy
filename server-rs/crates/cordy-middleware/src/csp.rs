//! Content-Security-Policy header — port of `server/internal/middleware/csp.go`.

use axum::extract::Request;
use axum::http::{header, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

/// Standard CSP for all routes.
pub const CSP_HEADER: &str = "default-src 'self'; \
script-src 'self'; \
style-src 'self' 'unsafe-inline'; \
img-src 'self' https: data:; \
connect-src 'self' wss:; \
frame-ancestors 'none'; \
object-src 'none'; \
base-uri 'self'; \
form-action 'self'";

/// Relaxed variant for attachment preview documents, which render inside an
/// iframe and therefore need `frame-ancestors 'self'`.
pub const ATTACHMENT_PREVIEW_CSP_HEADER: &str = "default-src 'self'; \
script-src 'self'; \
style-src 'self' 'unsafe-inline'; \
img-src 'self' https: data:; \
connect-src 'self' wss:; \
frame-ancestors 'self'; \
object-src 'none'; \
base-uri 'self'; \
form-action 'self'";

fn is_attachment_preview_document_path(path: &str) -> bool {
    path.starts_with("/api/attachments/")
        && (path.ends_with("/download") || path.ends_with("/content"))
}

pub fn content_security_policy_for_request(path: &str) -> &'static str {
    if is_attachment_preview_document_path(path) {
        ATTACHMENT_PREVIEW_CSP_HEADER
    } else {
        CSP_HEADER
    }
}

/// Sets Content-Security-Policy on every response.
pub async fn content_security_policy(req: Request, next: Next) -> Response {
    let csp = content_security_policy_for_request(req.uri().path());
    let mut res = next.run(req).await;
    res.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(csp),
    );
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_preview_paths_get_relaxed_csp() {
        assert_eq!(
            content_security_policy_for_request("/api/attachments/abc/download"),
            ATTACHMENT_PREVIEW_CSP_HEADER
        );
        assert_eq!(
            content_security_policy_for_request("/api/attachments/abc/content"),
            ATTACHMENT_PREVIEW_CSP_HEADER
        );
        assert_eq!(
            content_security_policy_for_request("/api/issues"),
            CSP_HEADER
        );
        // Prefix alone is not enough.
        assert_eq!(
            content_security_policy_for_request("/api/attachments/abc"),
            CSP_HEADER
        );
    }

    #[test]
    fn csp_headers_differ_only_in_frame_ancestors() {
        assert!(CSP_HEADER.contains("frame-ancestors 'none'"));
        assert!(ATTACHMENT_PREVIEW_CSP_HEADER.contains("frame-ancestors 'self'"));
    }
}
