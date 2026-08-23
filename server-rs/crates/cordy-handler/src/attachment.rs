use axum::{
    body::{to_bytes, Body},
    extract::{DefaultBodyLimit, Extension, Multipart, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use cordy_db::{
    models::Attachment,
    queries::{
        agent, agent_invocation_target, attachment, chat, comment, issue, member, workspace,
    },
};
use cordy_middleware::workspace::WorkspaceContext;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    attachment_storage::{content_disposition, StorageGetError},
    error::error_response,
    state::{AttachmentDownloadMode, HandlerState},
};

const MAX_UPLOAD: usize = 100 << 20;
const MAX_PREVIEW: usize = 2 << 20;
const CAP_TTL: i64 = 60;

pub fn public_router() -> Router<HandlerState> {
    Router::new()
        .route("/uploads/{*key}", get(serve_local_upload))
        .route(
            "/api/attachments/{id}/signed-download",
            get(signed_download),
        )
}
pub fn authenticated_router() -> Router<HandlerState> {
    Router::new()
        .route(
            "/api/upload-file",
            post(upload).layer(DefaultBodyLimit::max(MAX_UPLOAD)),
        )
        .route("/api/attachments/{id}/download", get(download))
}
pub fn workspace_router() -> Router<HandlerState> {
    Router::new()
        .route(
            "/api/attachments/{id}",
            get(metadata).delete(delete_attachment),
        )
        .route("/api/attachments/{id}/content", get(content))
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace id"))
}
fn parse_id(raw: &str, label: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(raw)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, &format!("invalid {label}")))
}
fn user_id(headers: &HeaderMap) -> Result<Uuid, Response> {
    headers
        .get("x-user-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or_else(|| error_response(StatusCode::UNAUTHORIZED, "authentication required"))
}

async fn load(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw: &str,
) -> Result<Attachment, Response> {
    let id = parse_id(raw, "attachment id")?;
    let ws = workspace_id(context)?;
    attachment::get_attachment(&state.pool, id, ws)
        .await
        .map_err(|_| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get attachment",
            )
        })?
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "attachment not found"))
}

async fn response_json(state: &HandlerState, att: &Attachment, capability: bool) -> Value {
    let stable = format!("/api/attachments/{}/download", att.id);
    let mut download_url = stable.clone();
    let mut attachment_download_url = None;
    let mode = resolved_download_mode(state);
    match mode {
        AttachmentDownloadMode::CloudFront => {
            if let Some(signer) = &state.attachment_download.cloudfront_signer {
                match signer.signed_url(&att.url, state.attachment_download.ttl, None) {
                    Ok(url) => download_url = url,
                    Err(error) => {
                        tracing::warn!(%error, attachment_id = %att.id, "failed to sign CloudFront attachment URL")
                    }
                }
                if capability {
                    match signer.signed_url(
                        &att.url,
                        state.attachment_download.ttl,
                        Some(&content_disposition(&att.content_type, &att.filename, true)),
                    ) {
                        Ok(url) => attachment_download_url = Some(url),
                        Err(error) => {
                            tracing::warn!(%error, attachment_id = %att.id, "failed to sign CloudFront attachment download URL")
                        }
                    }
                }
            }
        }
        AttachmentDownloadMode::Presign if capability => {
            if let Some(storage) = &state.attachment_storage {
                let key = storage.key_from_url(&att.url);
                if let Some(key) = key {
                    match storage
                        .presign_get(&key, state.attachment_download.ttl, None)
                        .await
                    {
                        Ok(Some(url)) => download_url = url,
                        Ok(None) => {}
                        Err(error) => {
                            tracing::warn!(%error, attachment_id = %att.id, "failed to presign attachment URL")
                        }
                    }
                    match storage
                        .presign_get(
                            &key,
                            state.attachment_download.ttl,
                            Some(&content_disposition(&att.content_type, &att.filename, true)),
                        )
                        .await
                    {
                        Ok(Some(url)) => attachment_download_url = Some(url),
                        Ok(None) => {}
                        Err(error) => {
                            tracing::warn!(%error, attachment_id = %att.id, "failed to presign attachment download URL")
                        }
                    }
                }
            }
        }
        AttachmentDownloadMode::Proxy if capability => {
            download_url = capability_path(att.id, false);
            attachment_download_url = Some(capability_path(att.id, true));
        }
        _ => {}
    }
    let markdown_url = markdown_url(state, att, &stable);
    let mut value = json!({"id":att.id,"workspace_id":att.workspace_id,"issue_id":att.issue_id,"comment_id":att.comment_id,"chat_session_id":att.chat_session_id,"chat_message_id":att.chat_message_id,"uploader_type":att.uploader_type,"uploader_id":att.uploader_id,"filename":att.filename,"url":att.url,"download_url":download_url,"markdown_url":markdown_url,"content_type":att.content_type,"size_bytes":att.size_bytes,"created_at":crate::timefmt::rfc3339(att.created_at)});
    if let Some(url) = attachment_download_url {
        value
            .as_object_mut()
            .expect("object")
            .insert("attachment_download_url".into(), json!(url));
    }
    value
}

fn resolved_download_mode(state: &HandlerState) -> AttachmentDownloadMode {
    match state.attachment_download.mode {
        AttachmentDownloadMode::Auto => {
            if state.attachment_download.cloudfront_signer.is_some() {
                AttachmentDownloadMode::CloudFront
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

fn markdown_url(state: &HandlerState, att: &Attachment, stable: &str) -> String {
    let publicly_readable = state
        .attachment_storage
        .as_ref()
        .is_some_and(|storage| storage.has_public_base_url())
        && state.attachment_download.cloudfront_signer.is_none()
        && durable_public_url(&att.url);
    if publicly_readable {
        return att.url.clone();
    }
    if state.attachment_download.public_url.is_empty() {
        stable.to_string()
    } else {
        format!("{}{}", state.attachment_download.public_url, stable)
    }
}

fn durable_public_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return false;
    }
    ![
        "Signature",
        "X-Amz-Signature",
        "Key-Pair-Id",
        "Expires",
        "X-Amz-Expires",
    ]
    .iter()
    .any(|key| url.query_pairs().any(|(candidate, _)| candidate == *key))
}

async fn metadata(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    match load(&state, &context, &id).await {
        Ok(att) => Json(response_json(&state, &att, true).await).into_response(),
        Err(r) => r,
    }
}

#[derive(Default)]
struct UploadForm {
    filename: String,
    bytes: Vec<u8>,
    issue_id: Option<String>,
    comment_id: Option<String>,
    chat_session_id: Option<String>,
    task_id: Option<String>,
}

async fn upload(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Query(query): Query<UploadQuery>,
    mut multipart: Multipart,
) -> Response {
    let uid = match user_id(&headers) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut form = UploadForm::default();
    loop {
        let field = match multipart.next_field().await {
            Ok(v) => v,
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "file too large or invalid multipart form",
                )
            }
        };
        let Some(field) = field else { break };
        let name = field.name().unwrap_or("").to_string();
        if name == "file" {
            form.filename = field.file_name().unwrap_or("").to_string();
            form.bytes = match field.bytes().await {
                Ok(v) if v.len() <= MAX_UPLOAD => v.to_vec(),
                _ => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "file too large or invalid multipart form",
                    )
                }
            };
        } else {
            let value = match field.text().await {
                Ok(v) => v,
                Err(_) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        "file too large or invalid multipart form",
                    )
                }
            };
            match name.as_str() {
                "issue_id" => form.issue_id = Some(value),
                "comment_id" => form.comment_id = Some(value),
                "chat_session_id" => form.chat_session_id = Some(value),
                "task_id" => form.task_id = Some(value),
                _ => {}
            }
        }
    }
    if form.filename.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "missing file field: http: no such file",
        );
    }
    let Some(storage) = &state.attachment_storage else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "file upload not configured",
        );
    };
    let workspace = resolve_upload_workspace(&state, &headers, &query).await;
    let id = Uuid::now_v7();
    let ext = std::path::Path::new(&form.filename)
        .extension()
        .and_then(|v| v.to_str())
        .filter(|v| v.len() <= 16)
        .map(|v| format!(".{v}"))
        .unwrap_or_default();
    let key = workspace.map_or_else(
        || format!("users/{uid}/{id}{ext}"),
        |ws| format!("workspaces/{ws}/{id}{ext}"),
    );
    let content_type = sniff(&form.bytes, &form.filename);
    if let Some(ws) = workspace {
        let Some(mem) = member::get_member_by_user_and_workspace(&state.pool, uid, ws)
            .await
            .ok()
            .flatten()
        else {
            return error_response(StatusCode::FORBIDDEN, "not a member of this workspace");
        };
        let (actor_type, actor_id, bound_task) = upload_actor(&state, &headers, ws, uid).await;
        let issue_id = match optional_id(form.issue_id.as_deref(), "issue_id") {
            Ok(v) => v,
            Err(r) => return r,
        };
        if let Some(value) = issue_id {
            if issue::get_issue_in_workspace(&state.pool, value, ws)
                .await
                .ok()
                .flatten()
                .is_none()
            {
                return error_response(StatusCode::FORBIDDEN, "invalid issue_id");
            }
        }
        let comment_id = match optional_id(form.comment_id.as_deref(), "comment_id") {
            Ok(v) => v,
            Err(r) => return r,
        };
        if let Some(value) = comment_id {
            if comment::get_comment_in_workspace(&state.pool, value, ws)
                .await
                .ok()
                .flatten()
                .is_none()
            {
                return error_response(StatusCode::FORBIDDEN, "invalid comment_id");
            }
        }
        let mut chat_session_id =
            match optional_id(form.chat_session_id.as_deref(), "chat_session_id") {
                Ok(v) => v,
                Err(r) => return r,
            };
        if let Some(value) = chat_session_id {
            if let Err(response) =
                gate_public_chat_session(&state, value, ws, uid, &actor_type, actor_id, &mem).await
            {
                return response;
            }
        }
        let task_id = match optional_id(form.task_id.as_deref(), "task_id") {
            Ok(v) => v,
            Err(r) => return r,
        };
        if let Some(value) = task_id {
            if headers.get("x-actor-source").and_then(|v| v.to_str().ok()) != Some("task_token")
                || bound_task != Some(value)
            {
                return error_response(
                    StatusCode::FORBIDDEN,
                    "task_id must match the request's task token",
                );
            }
            let Some(task) = agent::get_agent_task_in_workspace(&state.pool, value, ws)
                .await
                .ok()
                .flatten()
            else {
                return error_response(StatusCode::FORBIDDEN, "invalid task_id");
            };
            if actor_type != "agent" || task.agent_id != actor_id {
                return error_response(
                    StatusCode::FORBIDDEN,
                    "task_id upload requires the task's own agent",
                );
            }
            let Some(session) = task.chat_session_id else {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "task_id upload requires a chat task",
                );
            };
            chat_session_id = Some(session);
        }
        let size_bytes = form.bytes.len() as i64;
        let url = match storage
            .upload(&key, form.bytes, &content_type, &form.filename)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e,"file upload failed");
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "upload failed");
            }
        };
        let row = match attachment::create_attachment(
            &state.pool,
            id,
            ws,
            &actor_type,
            actor_id,
            &form.filename,
            &url,
            &content_type,
            size_bytes,
            issue_id,
            comment_id,
            chat_session_id,
            task_id,
        )
        .await
        {
            Ok(Some(v)) => v,
            _ => return Json(json!({"id":"","url":url,"filename":form.filename})).into_response(),
        };
        let att = Attachment {
            id: row.id.unwrap_or(id),
            workspace_id: row.workspace_id.unwrap_or(ws),
            issue_id: row.issue_id,
            comment_id: row.comment_id,
            uploader_type: row.uploader_type,
            uploader_id: row.uploader_id.unwrap_or(actor_id),
            filename: row.filename,
            url: row.url,
            content_type: row.content_type,
            size_bytes: row.size_bytes,
            created_at: row.created_at.unwrap_or_else(chrono::Utc::now),
            chat_session_id: row.chat_session_id,
            chat_message_id: row.chat_message_id,
            task_id: row.task_id,
        };
        publish_changes(
            &state,
            &att,
            row.issue_revision,
            row.comment_revision,
            &actor_type,
            actor_id,
        )
        .await;
        let _ = mem;
        return Json(response_json(&state, &att, false).await).into_response();
    }
    let url = match storage
        .upload(&key, form.bytes, &content_type, &form.filename)
        .await
    {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "upload failed"),
    };
    Json(json!({"id":id,"url":url,"filename":form.filename})).into_response()
}

#[derive(Default, Deserialize)]
struct UploadQuery {
    workspace_id: Option<String>,
    workspace_slug: Option<String>,
}

async fn resolve_upload_workspace(
    state: &HandlerState,
    headers: &HeaderMap,
    query: &UploadQuery,
) -> Option<Uuid> {
    if headers.get("x-actor-source").and_then(|v| v.to_str().ok()) == Some("task_token") {
        return header_id(headers, "x-workspace-id");
    }
    let slug = headers
        .get("x-workspace-slug")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| query.workspace_slug.clone());
    if let Some(slug) = slug {
        if let Some(value) = workspace::get_workspace_by_slug(&state.pool, &slug)
            .await
            .ok()
            .flatten()
        {
            return Some(value.id);
        }
    }
    if let Some(id) = headers
        .get("x-workspace-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
    {
        return Some(id);
    }
    query
        .workspace_id
        .as_deref()
        .and_then(|v| Uuid::parse_str(v).ok())
}
async fn gate_public_chat_session(
    state: &HandlerState,
    id: Uuid,
    ws: Uuid,
    uid: Uuid,
    actor_type: &str,
    actor_id: Uuid,
    mem: &cordy_db::models::Member,
) -> Result<(), Response> {
    let session = chat::get_chat_session_in_workspace(&state.pool, id, ws)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "chat session not found"))?;
    if session.creator_id != uid {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "not your chat session",
        ));
    }
    let agent = agent::get_agent_in_workspace(&state.pool, session.agent_id, ws)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "agent not found"))?;
    let access = actor_type == "agent"
        || agent.owner_id == Some(actor_id)
        || matches!(mem.role.as_str(), "owner" | "admin")
        || if agent.permission_mode == "public_to" {
            agent_invocation_target::list_agent_invocation_targets(&state.pool, agent.id)
                .await
                .ok()
                .is_some_and(|targets| {
                    targets.iter().any(|target| {
                        target.target_type == "workspace"
                            || (target.target_type == "member" && target.target_id == uid)
                    })
                })
        } else {
            false
        };
    if !access {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "you do not have access to this agent",
        ));
    }
    if chat::get_public_chat_session_in_workspace(&state.pool, id, ws)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return Err(error_response(
            StatusCode::NOT_FOUND,
            "chat session not found",
        ));
    }
    Ok(())
}

async fn upload_actor(
    state: &HandlerState,
    headers: &HeaderMap,
    ws: Uuid,
    uid: Uuid,
) -> (String, Uuid, Option<Uuid>) {
    if headers.get("x-actor-source").and_then(|v| v.to_str().ok()) == Some("task_token") {
        if let (Some(task_id), Some(agent_id)) = (
            header_id(headers, "x-task-id"),
            header_id(headers, "x-agent-id"),
        ) {
            if agent::get_agent_task_in_workspace(&state.pool, task_id, ws)
                .await
                .ok()
                .flatten()
                .filter(|t| t.agent_id == agent_id)
                .is_some()
            {
                return ("agent".into(), agent_id, Some(task_id));
            }
        }
    }
    ("member".into(), uid, None)
}
fn header_id(h: &HeaderMap, n: &str) -> Option<Uuid> {
    h.get(n)?
        .to_str()
        .ok()
        .and_then(|v| Uuid::parse_str(v).ok())
}
fn optional_id(raw: Option<&str>, label: &str) -> Result<Option<Uuid>, Response> {
    raw.filter(|v| !v.is_empty())
        .map(|v| parse_id(v, label))
        .transpose()
}

async fn download(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let uid = match user_id(&headers) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let aid = match parse_id(&id, "attachment id") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(att) = attachment::get_attachment_by_id_only(&state.pool, aid)
        .await
        .ok()
        .flatten()
    else {
        return error_response(StatusCode::NOT_FOUND, "attachment not found");
    };
    if member::get_member_by_user_and_workspace(&state.pool, uid, att.workspace_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return error_response(StatusCode::NOT_FOUND, "attachment not found");
    }
    match resolved_download_mode(&state) {
        AttachmentDownloadMode::CloudFront => {
            let Some(signer) = &state.attachment_download.cloudfront_signer else {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "cloudfront attachment downloads are not configured",
                );
            };
            match signer.signed_url(
                &att.url,
                state.attachment_download.ttl,
                Some(&content_disposition(&att.content_type, &att.filename, true)),
            ) {
                Ok(url) => attachment_redirect(&state, &url),
                Err(error) => {
                    tracing::error!(%error, attachment_id = %att.id, "failed to sign CloudFront attachment download");
                    error_response(StatusCode::BAD_GATEWAY, "failed to create download URL")
                }
            }
        }
        AttachmentDownloadMode::Presign => {
            let Some(storage) = &state.attachment_storage else {
                return error_response(StatusCode::SERVICE_UNAVAILABLE, "storage not configured");
            };
            let Some(key) = storage.key_from_url(&att.url) else {
                return error_response(StatusCode::NOT_FOUND, "attachment object not found");
            };
            match storage
                .presign_get(
                    &key,
                    state.attachment_download.ttl,
                    Some(&content_disposition(&att.content_type, &att.filename, true)),
                )
                .await
            {
                Ok(Some(url)) => attachment_redirect(&state, &url),
                Ok(None) => error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "attachment storage does not support presigned downloads",
                ),
                Err(error) => {
                    tracing::error!(%error, attachment_id = %att.id, "failed to presign attachment download");
                    error_response(StatusCode::BAD_GATEWAY, "failed to create download URL")
                }
            }
        }
        AttachmentDownloadMode::Proxy | AttachmentDownloadMode::Auto => {
            stream(
                &state,
                &att,
                headers.get(header::RANGE).and_then(|v| v.to_str().ok()),
                false,
            )
            .await
        }
    }
}

fn attachment_redirect(state: &HandlerState, url: &str) -> Response {
    let Ok(location) = HeaderValue::from_str(url) else {
        return error_response(StatusCode::BAD_GATEWAY, "invalid attachment download URL");
    };
    let mut response = StatusCode::FOUND.into_response();
    response.headers_mut().insert(header::LOCATION, location);
    preview_headers(response.headers_mut(), &state.attachment_frame_ancestors);
    response
}
async fn signed_download(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<CapabilityQuery>,
) -> Response {
    let aid = match Uuid::parse_str(&id) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::FORBIDDEN, "invalid or expired download link"),
    };
    let force = q.dl.as_deref() == Some("1");
    if !verify_capability(aid, q.exp, &q.sig, force) {
        return error_response(StatusCode::FORBIDDEN, "invalid or expired download link");
    }
    let Some(att) = attachment::get_attachment_by_id_only(&state.pool, aid)
        .await
        .ok()
        .flatten()
    else {
        return error_response(StatusCode::NOT_FOUND, "attachment not found");
    };
    let mut r = stream(
        &state,
        &att,
        headers.get(header::RANGE).and_then(|v| v.to_str().ok()),
        force,
    )
    .await;
    r.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    r
}
#[derive(Deserialize)]
struct CapabilityQuery {
    exp: i64,
    sig: String,
    dl: Option<String>,
}

async fn stream(
    state: &HandlerState,
    att: &Attachment,
    range: Option<&str>,
    force: bool,
) -> Response {
    let Some(storage) = &state.attachment_storage else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "storage not configured");
    };
    let Some(key) = storage.key_from_url(&att.url) else {
        return error_response(StatusCode::NOT_FOUND, "attachment object not found");
    };
    let object = match storage.get(&key, range).await {
        Ok(v) => v,
        Err(error) => {
            if let Some(StorageGetError::InvalidRange { total }) =
                error.downcast_ref::<StorageGetError>()
            {
                let mut r = error_response(
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    "requested range not satisfiable",
                );
                r.headers_mut().insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!(
                        "bytes */{}",
                        total.unwrap_or_else(|| att.size_bytes.max(0) as u64)
                    ))
                    .unwrap(),
                );
                return r;
            }
            if matches!(
                error.downcast_ref::<StorageGetError>(),
                Some(StorageGetError::NotFound)
            ) {
                return error_response(StatusCode::NOT_FOUND, "attachment object not found");
            }
            tracing::error!(%error, attachment_id = %att.id, %key, "attachment storage read failed");
            return error_response(StatusCode::BAD_GATEWAY, "attachment storage unavailable");
        }
    };
    let content_type = object
        .content_type
        .as_deref()
        .filter(|_| att.id.is_nil())
        .unwrap_or(if att.content_type.is_empty() {
            "application/octet-stream"
        } else {
            &att.content_type
        });
    let filename = object
        .filename
        .as_deref()
        .filter(|_| att.id.is_nil())
        .unwrap_or(&att.filename);
    let mut response = Response::new(object.body);
    *response.status_mut() = StatusCode::from_u16(object.status.as_u16()).unwrap_or(StatusCode::OK);
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(content_type, filename, force)).unwrap(),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Some(v) = object
        .content_length
        .and_then(|v| HeaderValue::from_str(&v.to_string()).ok())
    {
        h.insert(header::CONTENT_LENGTH, v);
    }
    if let Some(v) = object
        .content_range
        .and_then(|v| HeaderValue::from_str(&v).ok())
    {
        h.insert(header::CONTENT_RANGE, v);
    }
    preview_headers(h, &state.attachment_frame_ancestors);
    response
}

async fn serve_local_upload(
    State(state): State<HandlerState>,
    Path(key): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(storage) = &state.attachment_storage else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !storage.is_local() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let fake = Attachment {
        id: Uuid::nil(),
        workspace_id: Uuid::nil(),
        issue_id: None,
        comment_id: None,
        uploader_type: "system".into(),
        uploader_id: Uuid::nil(),
        filename: std::path::Path::new(&key)
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("download")
            .into(),
        url: format!("/uploads/{key}"),
        content_type: sniff(&[], &key),
        size_bytes: -1,
        created_at: chrono::Utc::now(),
        chat_session_id: None,
        chat_message_id: None,
        task_id: None,
    };
    stream(
        &state,
        &fake,
        headers.get(header::RANGE).and_then(|v| v.to_str().ok()),
        false,
    )
    .await
}

async fn content(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let att = match load(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if !text_previewable(&att.content_type, &att.filename) {
        return error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "preview not supported for this file type",
        );
    }
    let Some(storage) = &state.attachment_storage else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "storage not configured");
    };
    let Some(key) = storage.key_from_url(&att.url) else {
        return error_response(StatusCode::NOT_FOUND, "attachment object not found");
    };
    let object = match storage.get(&key, None).await {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::NOT_FOUND, "attachment object not found"),
    };
    let body = match to_bytes(object.body, MAX_PREVIEW + 1).await {
        Ok(v) => v,
        Err(_) => {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "file too large for inline preview",
            )
        }
    };
    if body.len() > MAX_PREVIEW {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "file too large for inline preview",
        );
    }
    let mut response = Response::new(Body::from(body));
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    if let Ok(v) = HeaderValue::from_str(&att.content_type) {
        h.insert("x-original-content-type", v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    preview_headers(h, &state.attachment_frame_ancestors);
    response
}

async fn delete_attachment(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let att = match load(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let admin = matches!(context.member.role.as_str(), "owner" | "admin");
    if !(att.uploader_type == "member" && att.uploader_id == context.member.user_id) && !admin {
        return error_response(
            StatusCode::FORBIDDEN,
            "not authorized to delete this attachment",
        );
    }
    let deleted = match attachment::delete_attachment(&state.pool, att.id, att.workspace_id).await {
        Ok(Some(v)) => v,
        _ => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete attachment",
            )
        }
    };
    if deleted.changed {
        publish_changes(
            &state,
            &att,
            deleted.issue_revision,
            deleted.comment_revision,
            "member",
            context.member.user_id,
        )
        .await
    }
    if let Some(storage) = &state.attachment_storage {
        if let Some(key) = storage.key_from_url(&att.url) {
            if let Err(e) = storage.delete(&key).await {
                tracing::warn!(%e,%key,"attachment object cleanup failed")
            }
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn publish_changes(
    state: &HandlerState,
    att: &Attachment,
    issue_revision: i64,
    comment_revision: i64,
    actor_type: &str,
    actor_id: Uuid,
) {
    if issue_revision > 0 {
        if let Some(issue_id) = att.issue_id {
            state.bus.publish(&cordy_events::Event {
                event_type: cordy_protocol::EVENT_ISSUE_ATTACHMENTS_CHANGED.into(),
                workspace_id: att.workspace_id.to_string(),
                actor_type: actor_type.into(),
                actor_id: actor_id.to_string(),
                payload: json!({"issue_id":issue_id,"issue_revision":issue_revision}),
                ..Default::default()
            })
        }
    }
    if comment_revision > 0 {
        if let Some(comment_id) = att.comment_id {
            if let Ok(Some(value)) =
                comment::get_comment_in_workspace(&state.pool, comment_id, att.workspace_id).await
            {
                let body = crate::comment::comment_json(state, &value).await;
                state.bus.publish(&cordy_events::Event {
                    event_type: cordy_protocol::EVENT_COMMENT_UPDATED.into(),
                    workspace_id: att.workspace_id.to_string(),
                    actor_type: actor_type.into(),
                    actor_id: actor_id.to_string(),
                    payload: json!({"comment":body}),
                    ..Default::default()
                })
            }
        }
    }
}

fn capability_path(id: Uuid, force: bool) -> String {
    let exp = chrono::Utc::now().timestamp() + CAP_TTL;
    let sig = sign_capability(id, exp, force);
    format!(
        "/api/attachments/{id}/signed-download?exp={exp}&sig={sig}{}",
        if force { "&dl=1" } else { "" }
    )
}
fn sign_capability(id: Uuid, exp: i64, force: bool) -> String {
    let key = Sha256::digest(
        format!(
            "attachment-download-capability:{}",
            cordy_auth::jwt::jwt_secret()
        )
        .as_bytes(),
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).expect("SHA key");
    mac.update(format!("v1|{id}|{exp}{}", if force { "|attachment" } else { "" }).as_bytes());
    hex::encode(mac.finalize().into_bytes())
}
fn verify_capability(id: Uuid, exp: i64, sig: &str, force: bool) -> bool {
    if chrono::Utc::now().timestamp() > exp {
        return false;
    }
    let Ok(got) = hex::decode(sig) else {
        return false;
    };
    let Ok(want) = hex::decode(sign_capability(id, exp, force)) else {
        return false;
    };
    got.len() == want.len() && got.iter().zip(want).fold(0u8, |v, (a, b)| v | (a ^ b)) == 0
}
fn preview_headers(h: &mut HeaderMap, origins: &[String]) {
    let mut a = vec!["'self'".to_string()];
    for raw in origins {
        if let Ok(u) = url::Url::parse(raw) {
            if matches!(u.scheme(), "http" | "https") {
                if let Some(host) = u.host_str() {
                    let value = format!(
                        "{}://{}{}",
                        u.scheme(),
                        host,
                        u.port().map(|p| format!(":{p}")).unwrap_or_default()
                    );
                    if !a.contains(&value) {
                        a.push(value)
                    }
                }
            }
        }
    }
    let value=format!("default-src 'none'; img-src 'self' data:; media-src 'self'; frame-ancestors {}; object-src 'none'; base-uri 'none'; form-action 'none'",a.join(" "));
    if let Ok(v) = HeaderValue::from_str(&value) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
}
fn sniff(body: &[u8], filename: &str) -> String {
    let detected = infer::get(body).map(|kind| kind.mime_type());
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "svg" => "image/svg+xml",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain; charset=utf-8",
        _ => detected.unwrap_or("application/octet-stream"),
    }
    .into()
}
fn text_previewable(content_type: &str, filename: &str) -> bool {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if ct.starts_with("text/")
        || matches!(
            ct.as_str(),
            "application/json"
                | "application/javascript"
                | "application/xml"
                | "application/x-yaml"
                | "application/yaml"
                | "application/toml"
                | "application/x-sh"
                | "application/x-httpd-php"
        )
    {
        return true;
    }
    let name = filename.to_ascii_lowercase();
    let ext = std::path::Path::new(&name)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("");
    matches!(
        ext,
        "md" | "markdown"
            | "txt"
            | "log"
            | "csv"
            | "tsv"
            | "html"
            | "htm"
            | "json"
            | "xml"
            | "yml"
            | "yaml"
            | "toml"
            | "ini"
            | "conf"
            | "sh"
            | "bash"
            | "zsh"
            | "py"
            | "rb"
            | "go"
            | "rs"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "sql"
            | "java"
            | "kt"
            | "swift"
            | "c"
            | "cc"
            | "cpp"
            | "h"
            | "hpp"
            | "cs"
            | "php"
            | "lua"
            | "vim"
            | "dockerfile"
            | "makefile"
            | "gitignore"
    ) || matches!(
        name.as_str(),
        "dockerfile" | "makefile" | ".env" | ".gitignore"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capability_is_id_intent_and_expiry_bound() {
        let id = Uuid::now_v7();
        let exp = chrono::Utc::now().timestamp() + 30;
        let sig = sign_capability(id, exp, false);
        assert!(verify_capability(id, exp, &sig, false));
        assert!(!verify_capability(Uuid::now_v7(), exp, &sig, false));
        assert!(!verify_capability(id, exp, &sig, true));
        assert!(!verify_capability(
            id,
            0,
            &sign_capability(id, 0, false),
            false
        ));
    }
    #[test]
    fn disposition_strips_headers_and_encodes_unicode() {
        let got = content_disposition("application/pdf", "微信\r\nX: y.pdf", true);
        assert!(!got.contains('\r'));
        assert!(!got.contains('\n'));
        assert!(got.contains("filename*=UTF-8''"));
    }

    #[test]
    fn sniffer_recognizes_media_outside_the_extension_overrides() {
        let webp = b"RIFF\x00\x00\x00\x00WEBPVP8 ";
        assert_eq!(sniff(webp, "upload.bin"), "image/webp");
    }

    #[test]
    fn gitignore_is_previewable_without_an_extension() {
        assert!(text_previewable("application/octet-stream", ".gitignore"));
    }

    #[test]
    fn durable_url_rejects_expiring_signatures() {
        assert!(durable_public_url("https://static.example/file.png"));
        assert!(!durable_public_url(
            "https://static.example/file.png?X-Amz-Signature=secret"
        ));
    }
}
