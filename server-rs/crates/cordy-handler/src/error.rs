//! Error → HTTP response mapping. Body shape:
//! `{"error": msg}` or `{"error": msg, "code": code}`, plus a trailing
//! newline matching Go's `json.Encoder.Encode`.

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use cordy_util::Error;
use serde_json::json;

/// Encode a JSON value with the framing and string escaping used by Go's
/// `json.Marshal` + `writeJSON`: HTML-sensitive bytes are escaped, followed by
/// one trailing newline. `serde_json::Value` cannot contain a value that fails
/// serialization, so this stays infallible like the successful Go path.
fn go_json_bytes(value: &serde_json::Value) -> Vec<u8> {
    let bytes = serde_json::to_vec(value).expect("serde_json::Value is serializable");
    let mut out = Vec::with_capacity(bytes.len() + 1);
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'<' => out.extend_from_slice(br#"\u003c"#),
            b'>' => out.extend_from_slice(br#"\u003e"#),
            b'&' => out.extend_from_slice(br#"\u0026"#),
            0xe2 if bytes.get(index + 1) == Some(&0x80) && bytes.get(index + 2) == Some(&0xa8) => {
                out.extend_from_slice(br#"\u2028"#);
                index += 2;
            }
            0xe2 if bytes.get(index + 1) == Some(&0x80) && bytes.get(index + 2) == Some(&0xa9) => {
                out.extend_from_slice(br#"\u2029"#);
                index += 2;
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    out.push(b'\n');
    out
}

fn json_response(status: StatusCode, value: serde_json::Value) -> Response {
    let body = go_json_bytes(&value);
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len().to_string())
        .body(Body::from(body))
        .expect("valid HTTP JSON response")
}

/// `{"error": msg}` — Go `writeError`.
pub fn error_response(status: StatusCode, msg: &str) -> Response {
    json_response(status, json!({ "error": msg }))
}

/// `{"error": msg, "code": code}` — Go `writeErrorCode`.
pub fn error_code_response(status: StatusCode, code: &str, msg: &str) -> Response {
    json_response(status, json!({ "error": msg, "code": code }))
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
        assert_eq!(
            res.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        let content_length = res
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            &bytes[..],
            br#"{"error":"task not found"}
"#
        );
        assert_eq!(content_length, bytes.len());
    }

    #[tokio::test]
    async fn error_code_response_carries_code() {
        let res = error_code_response(StatusCode::CONFLICT, "revision_conflict", "changed");
        assert_eq!(
            res.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );
        let content_length = res
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], "revision_conflict");
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_eq!(content_length, bytes.len());
    }

    #[tokio::test]
    async fn error_response_uses_go_safe_string_escaping() {
        let res = error_response(StatusCode::BAD_REQUEST, "<unsafe> & \u{2028}\u{2029}");
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            &bytes[..],
            br#"{"error":"\u003cunsafe\u003e \u0026 \u2028\u2029"}
"#
        );
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
