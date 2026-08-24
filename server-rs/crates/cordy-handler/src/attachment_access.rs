//! Shared attachment read policy and routes.
//!
//! Bulk responses stay durable: only CloudFront mode emits a signed URL;
//! presign/proxy deployments keep the stable authenticated path. The
//! single-attachment metadata endpoint is the fresh-URL exchange point.

use std::net::IpAddr;

use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::models::Attachment;
use cordy_middleware::workspace::WorkspaceContext;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

use crate::attachment_storage::{content_disposition, StorageGetError};
use crate::error::error_response;
use crate::state::{AttachmentDownloadMode, HandlerState};

type HmacSha256 = Hmac<Sha256>;

const CAPABILITY_TTL: i64 = 60;
const CAPABILITY_VERSION: &str = "v1";
const CAPABILITY_KEY_DOMAIN: &str = "attachment-download-capability:";
const DOWNLOAD_INTENT: &str = "attachment";

pub fn public_router() -> Router<HandlerState> {
    Router::new()
        .route("/uploads/{*key}", get(serve_local_upload))
        .route(
            "/api/attachments/{id}/signed-download",
            get(signed_download),
        )
}

pub fn authenticated_router() -> Router<HandlerState> {
    Router::new().route("/api/attachments/{id}/download", get(download))
}

pub fn workspace_router() -> Router<HandlerState> {
    Router::new().route("/api/attachments/{id}", get(metadata))
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct AttachmentUrls {
    pub download_url: String,
    pub markdown_url: String,
}

pub(crate) fn response_urls(
    state: &HandlerState,
    headers: &HeaderMap,
    attachment: &Attachment,
) -> AttachmentUrls {
    let stable = stable_path(attachment.id);
    let download_url = if request_has_stable_attachment_urls(headers) {
        stable.clone()
    } else if let Some(signer) = state.attachment_download.cloudfront_signer.as_ref() {
        signer
            .signed_url(&attachment.url, state.attachment_download.ttl, None)
            .unwrap_or_else(|error| {
                tracing::warn!(%error, attachment_id = %attachment.id, "failed to sign bulk attachment URL");
                stable.clone()
            })
    } else {
        // Canonical Go behavior: bulk/list URLs must not carry a short TTL.
        // Presign and proxy clients exchange this path at metadata/download.
        stable.clone()
    };
    AttachmentUrls {
        download_url,
        markdown_url: markdown_url(state, attachment, &stable),
    }
}

fn request_has_stable_attachment_urls(headers: &HeaderMap) -> bool {
    headers
        .get_all("x-client-capabilities")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|value| value.trim() == "stable_attachment_urls")
}

fn stable_path(id: Uuid) -> String {
    format!("/api/attachments/{id}/download")
}

fn markdown_url(state: &HandlerState, attachment: &Attachment, stable: &str) -> String {
    let publicly_readable = state
        .attachment_storage
        .as_ref()
        .is_some_and(|storage| storage.has_public_base_url())
        && state.attachment_download.cloudfront_signer.is_none()
        && durable_public_url(&attachment.url);
    if publicly_readable {
        return attachment.url.clone();
    }
    if state.attachment_download.public_url.is_empty() {
        stable.to_string()
    } else {
        format!("{}{}", state.attachment_download.public_url, stable)
    }
}

pub(crate) fn durable_public_url(raw: &str) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return false;
    }
    let expiring = [
        "signature",
        "x-amz-signature",
        "key-pair-id",
        "expires",
        "x-amz-expires",
    ];
    !url.query_pairs().any(|(key, _)| {
        expiring
            .iter()
            .any(|candidate| key.eq_ignore_ascii_case(candidate))
    })
}

pub(crate) fn resolved_download_mode(
    state: &HandlerState,
    raw_url: &str,
) -> AttachmentDownloadMode {
    match state.attachment_download.mode {
        AttachmentDownloadMode::Auto => {
            if state.attachment_download.cloudfront_signer.is_some() {
                AttachmentDownloadMode::CloudFront
            } else if should_proxy_url(raw_url) {
                AttachmentDownloadMode::Proxy
            } else if state
                .attachment_storage
                .as_ref()
                .is_some_and(|storage| storage.supports_presign())
            {
                AttachmentDownloadMode::Presign
            } else {
                AttachmentDownloadMode::Proxy
            }
        }
        mode => mode,
    }
}

fn should_proxy_url(raw_url: &str) -> bool {
    let Ok(url) = Url::parse(raw_url) else {
        return true;
    };
    let Some(host) = url.host_str() else {
        return true;
    };
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty()
        || host == "localhost"
        || host.ends_with(".localhost")
        || [
            ".local",
            ".localdomain",
            ".internal",
            ".lan",
            ".home",
            ".docker",
        ]
        .iter()
        .any(|suffix| host.ends_with(suffix))
        || !host.contains('.')
    {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|address| match address {
        IpAddr::V4(address) => {
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_unspecified()
        }
        IpAddr::V6(address) => {
            address.is_loopback()
                || address.is_unspecified()
                || (address.segments()[0] & 0xfe00) == 0xfc00
                || (address.segments()[0] & 0xffc0) == 0xfe80
        }
    })
}

async fn metadata(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let id = match Uuid::parse_str(&raw_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid attachment id"),
    };
    let workspace_id = match Uuid::parse_str(&context.workspace_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace id"),
    };
    let attachment =
        match cordy_db::queries::attachment::get_attachment(&state.pool, id, workspace_id).await {
            Ok(Some(attachment)) => attachment,
            Ok(None) => return error_response(StatusCode::NOT_FOUND, "attachment not found"),
            Err(error) => {
                tracing::warn!(%error, attachment_id = %id, "failed to load attachment metadata");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to get attachment",
                );
            }
        };
    match metadata_json(&state, &attachment).await {
        Ok(value) => Json(value).into_response(),
        Err(response) => response,
    }
}

async fn metadata_json(state: &HandlerState, attachment: &Attachment) -> Result<Value, Response> {
    let stable = stable_path(attachment.id);
    let (download_url, attachment_download_url) = match resolved_download_mode(
        state,
        &attachment.url,
    ) {
        AttachmentDownloadMode::CloudFront => {
            let signer = state
                .attachment_download
                .cloudfront_signer
                .as_ref()
                .ok_or_else(|| {
                    error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "cloudfront attachment downloads are not configured",
                    )
                })?;
            let download_url = signer
                .signed_url(&attachment.url, state.attachment_download.ttl, None)
                .map_err(|error| {
                    tracing::warn!(%error, attachment_id = %attachment.id, "failed to sign attachment URL");
                    error_response(StatusCode::BAD_GATEWAY, "failed to create download URL")
                })?;
            let attachment_download_url = Some(
                signer
                    .signed_url(
                        &attachment.url,
                        state.attachment_download.ttl,
                        Some(&content_disposition(
                            &attachment.content_type,
                            &attachment.filename,
                            true,
                        )),
                    )
                    .map_err(|error| {
                        tracing::warn!(%error, attachment_id = %attachment.id, "failed to sign forced attachment URL");
                        error_response(StatusCode::BAD_GATEWAY, "failed to create download URL")
                    })?,
            );
            (download_url, attachment_download_url)
        }
        AttachmentDownloadMode::Presign => {
            let storage = configured_storage(state)?;
            let key = storage.key_from_url(&attachment.url).ok_or_else(|| {
                error_response(StatusCode::NOT_FOUND, "attachment object not found")
            })?;
            let download_url = storage
                .presign_get(&key, state.attachment_download.ttl, None)
                .await
                .map_err(|error| {
                    tracing::warn!(%error, attachment_id = %attachment.id, "failed to presign attachment URL");
                    error_response(StatusCode::BAD_GATEWAY, "failed to create download URL")
                })?
                .ok_or_else(|| {
                    error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "attachment storage does not support presigned downloads",
                    )
                })?;
            let attachment_download_url = storage
                .presign_get(
                    &key,
                    state.attachment_download.ttl,
                    Some(&content_disposition(
                        &attachment.content_type,
                        &attachment.filename,
                        true,
                    )),
                )
                .await
                .map_err(|error| {
                    tracing::warn!(%error, attachment_id = %attachment.id, "failed to presign forced attachment URL");
                    error_response(StatusCode::BAD_GATEWAY, "failed to create download URL")
                })?;
            (download_url, attachment_download_url)
        }
        AttachmentDownloadMode::Proxy | AttachmentDownloadMode::Auto => {
            configured_storage(state)?;
            (
                capability_path(attachment.id, false),
                Some(capability_path(attachment.id, true)),
            )
        }
    };
    let mut value = attachment_json(
        attachment,
        download_url,
        markdown_url(state, attachment, &stable),
    );
    if let Some(url) = attachment_download_url {
        value
            .as_object_mut()
            .expect("attachment response is an object")
            .insert("attachment_download_url".into(), json!(url));
    }
    Ok(value)
}

fn attachment_json(attachment: &Attachment, download_url: String, markdown_url: String) -> Value {
    json!({
        "id": attachment.id,
        "workspace_id": attachment.workspace_id,
        "issue_id": attachment.issue_id,
        "comment_id": attachment.comment_id,
        "chat_session_id": attachment.chat_session_id,
        "chat_message_id": attachment.chat_message_id,
        "uploader_type": attachment.uploader_type,
        "uploader_id": attachment.uploader_id,
        "filename": attachment.filename,
        "url": attachment.url,
        "download_url": download_url,
        "markdown_url": markdown_url,
        "content_type": attachment.content_type,
        "size_bytes": attachment.size_bytes,
        "created_at": crate::timefmt::rfc3339(attachment.created_at),
    })
}

fn configured_storage(
    state: &HandlerState,
) -> Result<&dyn crate::attachment_storage::AttachmentStorage, Response> {
    state
        .attachment_storage
        .as_deref()
        .ok_or_else(|| error_response(StatusCode::SERVICE_UNAVAILABLE, "storage not configured"))
}

async fn download(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
) -> Response {
    let user_id = match header_uuid(&headers, "x-user-id") {
        Some(id) => id,
        None => return error_response(StatusCode::UNAUTHORIZED, "authentication required"),
    };
    let id = match Uuid::parse_str(&raw_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid attachment id"),
    };
    let attachment =
        match cordy_db::queries::attachment::get_attachment_by_id_only(&state.pool, id).await {
            Ok(Some(attachment)) => attachment,
            _ => return error_response(StatusCode::NOT_FOUND, "attachment not found"),
        };
    let is_member = cordy_db::queries::member::get_member_by_user_and_workspace(
        &state.pool,
        user_id,
        attachment.workspace_id,
    )
    .await
    .ok()
    .flatten()
    .is_some();
    if !is_member {
        return error_response(StatusCode::NOT_FOUND, "attachment not found");
    }
    dispatch_download(&state, &headers, &attachment, false).await
}

fn header_uuid(headers: &HeaderMap, name: &str) -> Option<Uuid> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
}

async fn dispatch_download(
    state: &HandlerState,
    headers: &HeaderMap,
    attachment: &Attachment,
    capability_force: bool,
) -> Response {
    match resolved_download_mode(state, &attachment.url) {
        AttachmentDownloadMode::CloudFront => {
            let Some(signer) = state.attachment_download.cloudfront_signer.as_ref() else {
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "cloudfront attachment downloads are not configured",
                );
            };
            match signer.signed_url(
                &attachment.url,
                state.attachment_download.ttl,
                Some(&content_disposition(
                    &attachment.content_type,
                    &attachment.filename,
                    true,
                )),
            ) {
                Ok(url) => redirect(&url),
                Err(error) => {
                    tracing::warn!(%error, attachment_id = %attachment.id, "failed to sign attachment download");
                    error_response(StatusCode::BAD_GATEWAY, "failed to create download URL")
                }
            }
        }
        AttachmentDownloadMode::Presign => {
            let storage = match configured_storage(state) {
                Ok(storage) => storage,
                Err(response) => return response,
            };
            let Some(key) = storage.key_from_url(&attachment.url) else {
                return error_response(StatusCode::NOT_FOUND, "attachment object not found");
            };
            match storage
                .presign_get(
                    &key,
                    state.attachment_download.ttl,
                    Some(&content_disposition(
                        &attachment.content_type,
                        &attachment.filename,
                        true,
                    )),
                )
                .await
            {
                Ok(Some(url)) => redirect(&url),
                Ok(None) => error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "attachment storage does not support presigned downloads",
                ),
                Err(error) => {
                    tracing::warn!(%error, attachment_id = %attachment.id, "failed to presign attachment download");
                    error_response(StatusCode::BAD_GATEWAY, "failed to create download URL")
                }
            }
        }
        AttachmentDownloadMode::Proxy | AttachmentDownloadMode::Auto => {
            stream(
                state,
                attachment,
                headers
                    .get(header::RANGE)
                    .and_then(|value| value.to_str().ok()),
                capability_force,
            )
            .await
        }
    }
}

fn redirect(url: &str) -> Response {
    let Ok(location) = HeaderValue::from_str(url) else {
        return error_response(StatusCode::BAD_GATEWAY, "invalid attachment download URL");
    };
    let mut response = StatusCode::FOUND.into_response();
    response.headers_mut().insert(header::LOCATION, location);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

#[derive(Default, Deserialize)]
struct CapabilityQuery {
    exp: Option<String>,
    sig: Option<String>,
    dl: Option<String>,
}

async fn signed_download(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Path(raw_id): Path<String>,
    Query(query): Query<CapabilityQuery>,
) -> Response {
    let Ok(id) = Uuid::parse_str(&raw_id) else {
        return invalid_capability();
    };
    let force = query.dl.as_deref() == Some("1");
    if !verify_capability(
        id,
        query.exp.as_deref(),
        query.sig.as_deref(),
        force,
        chrono::Utc::now().timestamp(),
    ) {
        return invalid_capability();
    }
    let attachment =
        match cordy_db::queries::attachment::get_attachment_by_id_only(&state.pool, id).await {
            Ok(Some(attachment)) => attachment,
            Ok(None) => return error_response(StatusCode::NOT_FOUND, "attachment not found"),
            Err(error) => {
                tracing::warn!(%error, attachment_id = %id, "failed to load capability attachment");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load attachment",
                );
            }
        };
    // A capability is only minted in proxy mode. Always proxy at redemption
    // so its query credential cannot leak through a cross-origin Referer.
    let mut response = stream(
        &state,
        &attachment,
        headers
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok()),
        force,
    )
    .await;
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn invalid_capability() -> Response {
    error_response(StatusCode::FORBIDDEN, "invalid or expired download link")
}

async fn stream(
    state: &HandlerState,
    attachment: &Attachment,
    range: Option<&str>,
    force: bool,
) -> Response {
    let storage = match configured_storage(state) {
        Ok(storage) => storage,
        Err(response) => return response,
    };
    let Some(key) = storage.key_from_url(&attachment.url) else {
        return error_response(StatusCode::NOT_FOUND, "attachment object not found");
    };
    stream_key(
        storage,
        &key,
        range,
        &attachment.content_type,
        &attachment.filename,
        force,
        attachment.size_bytes,
    )
    .await
}

async fn stream_key(
    storage: &dyn crate::attachment_storage::AttachmentStorage,
    key: &str,
    range: Option<&str>,
    fallback_content_type: &str,
    fallback_filename: &str,
    force: bool,
    fallback_size: i64,
) -> Response {
    let object = match storage.get(key, range).await {
        Ok(object) => object,
        Err(error) => {
            if let Some(StorageGetError::InvalidRange { total }) =
                error.downcast_ref::<StorageGetError>()
            {
                let mut response = error_response(
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    "requested range not satisfiable",
                );
                if let Ok(value) = HeaderValue::from_str(&format!(
                    "bytes */{}",
                    total.unwrap_or_else(|| fallback_size.max(0) as u64)
                )) {
                    response.headers_mut().insert(header::CONTENT_RANGE, value);
                }
                return response;
            }
            if matches!(
                error.downcast_ref::<StorageGetError>(),
                Some(StorageGetError::NotFound)
            ) {
                return error_response(StatusCode::NOT_FOUND, "attachment object not found");
            }
            tracing::warn!(%error, %key, "attachment storage read failed");
            return error_response(StatusCode::BAD_GATEWAY, "attachment storage unavailable");
        }
    };
    let content_type = object
        .content_type
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(if fallback_content_type.trim().is_empty() {
            "application/octet-stream"
        } else {
            fallback_content_type
        });
    let filename = object
        .filename
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback_filename);
    let mut response = Response::new(object.body);
    *response.status_mut() = StatusCode::from_u16(object.status.as_u16()).unwrap_or(StatusCode::OK);
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(content_type, filename, force))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    if let Some(value) = object
        .content_length
        .and_then(|length| HeaderValue::from_str(&length.to_string()).ok())
    {
        headers.insert(header::CONTENT_LENGTH, value);
    }
    if let Some(value) = object
        .content_range
        .and_then(|range| HeaderValue::from_str(&range).ok())
    {
        headers.insert(header::CONTENT_RANGE, value);
    }
    response
}

async fn serve_local_upload(
    State(state): State<HandlerState>,
    Path(key): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(storage) = state.attachment_storage.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !storage.is_local() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let filename = std::path::Path::new(&key)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    stream_key(
        storage,
        &key,
        headers
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok()),
        "application/octet-stream",
        filename,
        false,
        -1,
    )
    .await
}

pub(crate) fn capability_path(id: Uuid, force: bool) -> String {
    capability_path_at(id, force, chrono::Utc::now().timestamp())
}

fn capability_path_at(id: Uuid, force: bool, now: i64) -> String {
    let exp = now + CAPABILITY_TTL;
    let signature = sign_capability(id, exp, force);
    let intent = if force { "&dl=1" } else { "" };
    format!("/api/attachments/{id}/signed-download?exp={exp}&sig={signature}{intent}")
}

fn capability_signing_key() -> [u8; 32] {
    Sha256::digest(format!("{CAPABILITY_KEY_DOMAIN}{}", cordy_auth::jwt::jwt_secret()).as_bytes())
        .into()
}

fn capability_message(id: Uuid, exp: i64, force: bool) -> String {
    if force {
        format!("{CAPABILITY_VERSION}|{id}|{exp}|{DOWNLOAD_INTENT}")
    } else {
        format!("{CAPABILITY_VERSION}|{id}|{exp}")
    }
}

fn sign_capability(id: Uuid, exp: i64, force: bool) -> String {
    let mut mac = HmacSha256::new_from_slice(&capability_signing_key())
        .unwrap_or_else(|_| unreachable!("SHA-256 always yields a valid HMAC key"));
    mac.update(capability_message(id, exp, force).as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

fn verify_capability(
    id: Uuid,
    raw_exp: Option<&str>,
    signature: Option<&str>,
    force: bool,
    now: i64,
) -> bool {
    let (Some(raw_exp), Some(signature)) = (raw_exp, signature) else {
        return false;
    };
    let Ok(exp) = raw_exp.parse::<i64>() else {
        return false;
    };
    if now > exp {
        return false;
    }
    let Ok(signature) = hex::decode(signature) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(&capability_signing_key())
        .unwrap_or_else(|_| unreachable!("SHA-256 always yields a valid HMAC key"));
    mac.update(capability_message(id, exp, force).as_bytes());
    mac.verify_slice(&signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(url: &str) -> Attachment {
        Attachment {
            id: Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f13").unwrap(),
            workspace_id: Uuid::nil(),
            issue_id: None,
            comment_id: None,
            chat_session_id: None,
            chat_message_id: None,
            task_id: None,
            uploader_type: "member".into(),
            uploader_id: Uuid::nil(),
            filename: "diagram.png".into(),
            url: url.into(),
            content_type: "image/png".into(),
            size_bytes: 42,
            created_at: chrono::Utc::now(),
        }
    }

    fn state() -> HandlerState {
        HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        )
    }

    #[tokio::test]
    async fn bulk_presign_and_proxy_shapes_stay_stable_and_capability_is_relative() {
        let mut state = state();
        state.attachment_download.public_url = "https://api.example".into();
        let attachment = attachment("https://private.example/object.png");
        let default_urls = response_urls(&state, &HeaderMap::new(), &attachment);
        assert_eq!(default_urls.download_url, stable_path(attachment.id));
        assert_eq!(
            default_urls.markdown_url,
            format!("https://api.example{}", stable_path(attachment.id))
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-client-capabilities",
            HeaderValue::from_static("stable_attachment_urls"),
        );
        let stable = response_urls(&state, &headers, &attachment);
        assert_eq!(stable.download_url, stable_path(attachment.id));
        assert!(stable.download_url.starts_with('/'));
        assert!(capability_path(attachment.id, false).starts_with(&format!(
            "/api/attachments/{}/signed-download?",
            attachment.id
        )));
    }

    #[test]
    fn capabilities_are_id_intent_and_expiry_bound() {
        let id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f13").unwrap();
        let exp = 2_000_000_000;
        let signature = sign_capability(id, exp, false);
        assert!(verify_capability(
            id,
            Some(&exp.to_string()),
            Some(&signature),
            false,
            exp,
        ));
        assert!(!verify_capability(
            Uuid::nil(),
            Some(&exp.to_string()),
            Some(&signature),
            false,
            exp,
        ));
        assert!(!verify_capability(
            id,
            Some(&exp.to_string()),
            Some(&signature),
            true,
            exp,
        ));
        assert!(!verify_capability(
            id,
            Some(&exp.to_string()),
            Some(&signature),
            false,
            exp + 1,
        ));
    }

    #[test]
    fn private_and_expiring_urls_are_not_durable_markdown_urls() {
        assert!(!durable_public_url("/uploads/a.png"));
        assert!(!durable_public_url(
            "https://cdn.example/a.png?X-Amz-Signature=secret"
        ));
        assert!(durable_public_url("https://cdn.example/a.png"));
    }

    #[test]
    fn internal_origins_resolve_to_proxy_mode() {
        for value in [
            "/uploads/a.png",
            "http://minio:9000/a.png",
            "http://127.0.0.1/a.png",
            "http://10.0.0.1/a.png",
            "http://objects.internal/a.png",
        ] {
            assert!(should_proxy_url(value), "{value}");
        }
        assert!(!should_proxy_url("https://s3.us-west-2.amazonaws.com/a"));
    }
}
