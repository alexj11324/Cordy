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

use crate::error::error_response;
use crate::state::{AttachmentDownloadMode, HandlerState};

type HmacSha256 = Hmac<Sha256>;

pub fn router() -> Router<HandlerState> {
    Router::new().route("/api/avatars/{sig}/{*key}", get(serve))
}

fn signing_key() -> [u8; 32] {
    Sha256::digest(format!("avatar-url:{}", cordy_auth::jwt::jwt_secret()).as_bytes()).into()
}

pub(crate) fn sign_key(key: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(&signing_key()).expect("HMAC accepts any key length");
    mac.update(key.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

fn key_from_served_url(raw: &str) -> Option<String> {
    let path = url::Url::parse(raw)
        .ok()
        .map(|url| url.path().to_string())
        .unwrap_or_else(|| raw.to_string());
    let rest = path.strip_prefix("/api/avatars/")?;
    let (signature, key) = rest.split_once('/')?;
    (!key.is_empty() && signature_valid(key, signature)).then(|| key.to_string())
}

fn served_url(state: &HandlerState, key: &str) -> String {
    let path = format!("/api/avatars/{}/{key}", sign_key(key));
    let base = &state.attachment_download.public_url;
    if base.is_empty() {
        path
    } else {
        format!("{base}{path}")
    }
}

/// Resolves a durable avatar object URL into the stable capability endpoint
/// used by private object stores. Foreign URLs and local public uploads are
/// deliberately passed through unchanged.
pub fn resolve_url(state: &HandlerState, raw: &str) -> String {
    let Some(storage) = state.attachment_storage.as_ref() else {
        return raw.to_string();
    };
    if let Some(key) = key_from_served_url(raw) {
        return served_url(state, &key);
    }
    let Some(key) = storage.key_from_url(raw) else {
        return raw.to_string();
    };
    if storage.object_url(&key) != raw || content_type(&key).is_none() || storage.is_local() {
        return raw.to_string();
    }
    if storage.has_public_base_url()
        && state.attachment_download.cloudfront_signer.is_none()
        && crate::attachment_access::durable_public_url(raw)
    {
        return raw.to_string();
    }
    served_url(state, &key)
}

/// Normalizes and validates a client-supplied avatar URL before persistence.
/// A private workspace attachment can never be promoted into a public avatar
/// capability simply by copying its object URL into an avatar field.
pub async fn accept_url(
    state: &HandlerState,
    raw: &str,
    current: Option<&str>,
) -> Result<String, &'static str> {
    let Some(storage) = state.attachment_storage.as_ref() else {
        return Ok(raw.trim().to_string());
    };
    let trimmed = raw.trim();
    let normalized = key_from_served_url(trimmed)
        .map(|key| storage.object_url(&key))
        .unwrap_or_else(|| trimmed.to_string());
    if current.is_some_and(|value| value.trim() == normalized) {
        return Ok(normalized);
    }
    let Some(key) = storage.key_from_url(&normalized) else {
        return Ok(normalized);
    };
    if storage.object_url(&key) != normalized {
        return Ok(normalized);
    }
    if !publishable(state, &key).await {
        return Err("avatar_url must reference a standalone image upload, not a file attached to an issue, comment, or chat");
    }
    Ok(normalized)
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
    let object_url = storage.object_url(&key);
    match crate::attachment_access::resolved_download_mode(&state, &object_url) {
        AttachmentDownloadMode::CloudFront => {
            let Some(signer) = state.attachment_download.cloudfront_signer.as_ref() else {
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "cloudfront avatar downloads are not configured",
                );
            };
            return match signer.signed_url(&object_url, state.attachment_download.ttl, None) {
                Ok(url) => avatar_redirect(&url, avatar_redirect_max_age(&state)),
                Err(error) => {
                    tracing::warn!(%error, %key, "failed to sign avatar URL");
                    error_response(StatusCode::BAD_GATEWAY, "failed to create avatar URL")
                }
            };
        }
        AttachmentDownloadMode::Presign => {
            return match storage
                .presign_get(&key, state.attachment_download.ttl, None)
                .await
            {
                Ok(Some(url)) => avatar_redirect(&url, avatar_redirect_max_age(&state)),
                Ok(None) => error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "avatar storage does not support presigned downloads",
                ),
                Err(error) => {
                    tracing::warn!(%error, %key, "failed to presign avatar URL");
                    error_response(StatusCode::BAD_GATEWAY, "failed to create avatar URL")
                }
            };
        }
        AttachmentDownloadMode::Proxy | AttachmentDownloadMode::Auto => {}
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

fn avatar_redirect(url: &str, max_age: u64) -> Response {
    let Ok(location) = HeaderValue::from_str(url) else {
        return error_response(StatusCode::BAD_GATEWAY, "invalid avatar URL");
    };
    let mut response = StatusCode::FOUND.into_response();
    response.headers_mut().insert(header::LOCATION, location);
    let cache_control = if max_age == 0 {
        HeaderValue::from_static("no-store")
    } else {
        HeaderValue::from_str(&format!("private, max-age={max_age}"))
            .unwrap_or_else(|_| HeaderValue::from_static("no-store"))
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, cache_control);
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
    response
}

fn avatar_redirect_max_age(state: &HandlerState) -> u64 {
    (state.attachment_download.ttl.as_secs() / 2).min(60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachment_storage::{AttachmentStorage, StoredObject};
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

    struct PresignStorage {
        public: bool,
    }

    #[async_trait]
    impl AttachmentStorage for PresignStorage {
        async fn upload(
            &self,
            _key: &str,
            _body: Vec<u8>,
            _content_type: &str,
            _filename: &str,
        ) -> anyhow::Result<String> {
            anyhow::bail!("not used")
        }

        async fn get(&self, _key: &str, _range: Option<&str>) -> anyhow::Result<StoredObject> {
            anyhow::bail!("not used")
        }

        async fn delete(&self, _key: &str) -> anyhow::Result<()> {
            anyhow::bail!("not used")
        }

        async fn presign_get(
            &self,
            key: &str,
            _ttl: Duration,
            _content_disposition: Option<&str>,
        ) -> anyhow::Result<Option<String>> {
            Ok(Some(format!("https://signed.example/{key}?sig=fresh")))
        }

        fn key_from_url(&self, raw: &str) -> Option<String> {
            raw.strip_prefix("https://objects.example/")
                .map(str::to_string)
        }

        fn object_url(&self, key: &str) -> String {
            format!("https://objects.example/{key}")
        }

        fn has_public_base_url(&self) -> bool {
            self.public
        }

        fn supports_presign(&self) -> bool {
            true
        }
    }

    fn state(public: bool) -> HandlerState {
        let mut settings = crate::state::AttachmentDownloadSettings::default();
        settings.public_url = "https://api.example".into();
        HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        )
        .with_attachment_storage(Arc::new(PresignStorage { public }), settings)
    }

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

    #[test]
    fn forged_served_urls_are_not_normalized() {
        assert!(key_from_served_url("/api/avatars/not-valid/users/u/avatar.png").is_none());
    }

    #[tokio::test]
    async fn private_s3_avatar_resolves_through_stable_route_then_presigns() {
        let state = state(false);
        let raw = "https://objects.example/users/u/avatar.png";
        let resolved = resolve_url(&state, raw);
        assert!(resolved.starts_with("https://api.example/api/avatars/"));

        let path = resolved.strip_prefix("https://api.example").unwrap();
        let response = router()
            .with_state(state)
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "https://signed.example/users/u/avatar.png?sig=fresh"
        );
    }

    #[tokio::test]
    async fn public_s3_avatar_stays_on_durable_object_url() {
        let state = state(true);
        let raw = "https://objects.example/users/u/avatar.png";
        assert_eq!(resolve_url(&state, raw), raw);
    }
}
