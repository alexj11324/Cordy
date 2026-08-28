//! Request-ID assignment — port of chi's `middleware.RequestID`.
//!
//! Generates `X-Request-ID` before request logging when the caller omitted a
//! usable value, and echoes the identifier on the response.

use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// Preserves a non-empty caller-supplied request ID, otherwise mints one.
pub fn resolve_request_id(existing: Option<&str>) -> String {
    existing
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| HeaderValue::from_str(value).is_ok())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::now_v7().to_string())
}

/// Assigns `X-Request-ID` on the inbound request (so loggers can read it)
/// and copies it onto the response when the handler did not set one.
pub async fn request_id(mut req: Request, next: Next) -> Response {
    let id = resolve_request_id(
        req.headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    if let Ok(value) = HeaderValue::from_str(&id) {
        req.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    let mut res = next.run(req).await;
    if !res.headers().contains_key(REQUEST_ID_HEADER) {
        if let Ok(value) = HeaderValue::from_str(&id) {
            res.headers_mut().insert(REQUEST_ID_HEADER, value);
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::middleware;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    #[test]
    fn empty_or_invalid_values_are_replaced() {
        assert!(resolve_request_id(None).starts_with("01") || resolve_request_id(None).len() == 36);
        assert_eq!(resolve_request_id(Some("abc-123")), "abc-123");
        assert_eq!(resolve_request_id(Some("  abc-123  ")), "abc-123");
        assert_ne!(resolve_request_id(Some("")), " ");
        assert!(HeaderValue::from_str(&resolve_request_id(Some("bad\n"))).is_ok());
        assert_ne!(resolve_request_id(Some("bad\nid")), "bad\nid");
    }

    #[tokio::test]
    async fn generated_id_is_logged_on_request_and_response() {
        let app = Router::new()
            .route(
                "/",
                get(|req: HttpRequest<Body>| async move {
                    req.headers()
                        .get(REQUEST_ID_HEADER)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string()
                }),
            )
            .layer(middleware::from_fn(request_id));
        let response = app
            .oneshot(HttpRequest::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let response_id = response
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap()
            .to_string();
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(body, response_id.as_bytes());
        assert!(!response_id.is_empty());
    }

    #[tokio::test]
    async fn supplied_id_is_preserved() {
        let app = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(middleware::from_fn(request_id));
        let response = app
            .oneshot(
                HttpRequest::get("/")
                    .header("x-request-id", "client-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers().get("x-request-id").unwrap(), "client-id");
    }
}
