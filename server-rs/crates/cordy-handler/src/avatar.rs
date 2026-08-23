//! Public, capability-signed avatar object serving.
//!
//! The HMAC authorizes one immutable storage key. Workspace uploads are also
//! re-checked on every read so a file later bound to an issue/comment/chat can
//! never remain publicly readable through an old avatar URL.

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::path::Path as FsPath;
use uuid::Uuid;

use crate::state::HandlerState;

type HmacSha256 = Hmac<Sha256>;

pub fn router() -> Router<HandlerState> {
    Router::new().route("/api/avatars/{sig}/{*key}", get(serve))
}

fn signing_key() -> [u8; 32] {
    Sha256::digest(format!("avatar-url:{}", cordy_auth::jwt::jwt_secret()).as_bytes()).into()
}

#[cfg(test)]
fn sign_key(key: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(&signing_key()).expect("HMAC accepts any key length");
    mac.update(key.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn signature_valid(key: &str, signature: &str) -> bool {
    let Ok(signature) = URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(&signing_key()).expect("HMAC accepts any key length");
    mac.update(key.as_bytes());
    mac.verify_slice(&signature).is_ok()
}

fn content_type(key: &str) -> Option<&'static str> {
    match FsPath::new(key)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("avif") => Some("image/avif"),
        Some("bmp") => Some("image/bmp"),
        Some("ico") => Some("image/x-icon"),
        _ => None,
    }
}

fn attachment_id(key: &str) -> Option<Uuid> {
    let name = FsPath::new(key).file_name()?.to_str()?;
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    Uuid::parse_str(stem).ok()
}

async fn publishable(state: &HandlerState, key: &str) -> bool {
    if content_type(key).is_none() {
        return false;
    }
    if !key.starts_with("workspaces/") {
        return true;
    }
    let Some(id) = attachment_id(key) else {
        return false;
    };
    let Some(attachment) =
        cordy_db::queries::attachment::get_attachment_by_id_only(&state.pool, id)
            .await
            .ok()
            .flatten()
    else {
        return false;
    };
    attachment
        .content_type
        .to_ascii_lowercase()
        .starts_with("image/")
        && attachment.issue_id.is_none()
        && attachment.comment_id.is_none()
        && attachment.chat_session_id.is_none()
        && attachment.chat_message_id.is_none()
        && attachment.task_id.is_none()
}

async fn serve(
    State(state): State<HandlerState>,
    Path((signature, key)): Path<(String, String)>,
) -> Response {
    let Some(storage) = state.attachment_storage.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(expected_type) = content_type(&key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !signature_valid(&key, &signature) || !publishable(&state, &key).await {
        // Deliberately identical for invalid signature, missing row, bound
        // attachment, and disallowed type: this endpoint is not an oracle.
        return StatusCode::NOT_FOUND.into_response();
    }
    let object = match storage.get(&key, None).await {
        Ok(value) => value,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let mut response = Response::new(object.body);
    *response.status_mut() = StatusCode::from_u16(object.status.as_u16()).unwrap_or(StatusCode::OK);
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(expected_type),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("inline"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=300"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    if let Some(length) = object
        .content_length
        .and_then(|value| HeaderValue::from_str(&value.to_string()).ok())
    {
        headers.insert(header::CONTENT_LENGTH, length);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_are_key_bound_and_url_safe() {
        let signature = sign_key("users/u/avatar.png");
        assert!(signature_valid("users/u/avatar.png", &signature));
        assert!(!signature_valid("users/u/other.png", &signature));
        assert!(!signature.contains(['+', '/', '=']));
    }

    #[test]
    fn svg_and_non_images_are_never_avatar_class() {
        assert_eq!(content_type("a.PNG"), Some("image/png"));
        assert_eq!(content_type("a.svg"), None);
        assert_eq!(content_type("a.txt"), None);
    }
}
