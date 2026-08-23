//! Error → HTTP response mapping, port of `writeError` / `writeErrorCode`
//! (server/internal/handler/handler.go). Body shape:
//! `{"error": msg}` or `{"error": msg, "code": code}`, plus a trailing
//! newline matching Go's `json.Encoder.Encode`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use cordy_util::Error;
use serde_json::json;

/// `{"error": msg}` — Go `writeError`.
pub fn error_response(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

/// `{"error": msg, "code": code}` — Go `writeErrorCode`.
pub fn error_code_response(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(json!({ "error": msg, "code": code }))).into_response()
}

/// Maps the shared domain [`cordy_util::Error`] onto HTTP status codes in one
/// place, mirroring the taxonomy the Go handlers apply per-call-site.
pub fn domain_error(e: Error) -> Response {
    let status = match &e {
        Error::NotFound(_) => StatusCode::NOT_FOUND,
        Error::Unauthorized => StatusCode::UNAUTHORIZED,
        Error::Forbidden => StatusCode::FORBIDDEN,
        Error::Invalid(_) => StatusCode::BAD_REQUEST,
        Error::Conflict(_) => StatusCode::CONFLICT,
        Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let msg = match &e {
        Error::NotFound(m) => (*m).to_string(),
        Error::Invalid(m) | Error::Conflict(m) => m.clone(),
        _ => e.to_string(),
    };
    if let Error::Internal(inner) = &e {
        tracing::error!(error = %inner, "handler: internal error");
    }
    error_response(status, &msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt as _;

    #[tokio::test]
    async fn error_response_matches_go_shape() {
        let res = error_response(StatusCode::NOT_FOUND, "task not found");
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], br#"{"error":"task not found"}"#);
    }

    #[tokio::test]
    async fn error_code_response_carries_code() {
        let res = error_code_response(StatusCode::CONFLICT, "revision_conflict", "changed");
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], "revision_conflict");
    }

    #[test]
    fn domain_error_maps_statuses() {
        assert_eq!(
            domain_error(Error::NotFound("x")).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            domain_error(Error::Invalid("bad".to_string())).status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            domain_error(Error::Conflict("dup".to_string())).status(),
            StatusCode::CONFLICT
        );
    }
}
