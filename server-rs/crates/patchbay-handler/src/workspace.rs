//! Workspace domain handlers — first slice of the route port (S8).
//!
//! Implements workspace listing/detail and share-link lookup. Wire shapes match the
//! Go structs field-for-field: UUIDs as hyphenated strings, timestamps as
//! RFC3339, nullable columns as absent-or-null JSON.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use patchbay_db::models::{Agent, AgentTaskQueue, Member, User, Workspace};
use patchbay_db::queries::{member, share_link, user, workspace};
use patchbay_middleware::workspace::WorkspaceContext;
use rand::RngCore;
use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn public_router() -> Router<HandlerState> {
    Router::new().route("/api/share-links/{code}", get(get_share_link_info))
}

/// Authenticated workspace routes from router.go. The collection is user
/// scoped; the item route additionally requires membership in the workspace
/// named by `{id}`.
pub fn authenticated_router() -> Router<HandlerState> {
    Router::new()
        .route(
            "/api/workspaces",
            get(list_workspaces).post(create_workspace),
        )
        .route(
            "/api/workspaces/",
            get(list_workspaces).post(create_workspace),
        )
        .route("/api/workspaces/{id}", get(get_workspace))
        .route("/api/workspaces/{id}/", get(get_workspace))
        .route("/api/share-links/join", post(join_by_share_link))
}

pub fn member_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/workspaces/{id}/members", get(list_members))
        .route("/api/workspaces/{id}/leave", post(leave_workspace))
}

pub fn admin_router() -> Router<HandlerState> {
    Router::new()
        .route(
            "/api/workspaces/{id}",
            put(update_workspace)
                .patch(update_workspace)
                .delete(delete_workspace),
        )
        .route(
            "/api/workspaces/{id}/members/{member_id}",
            patch(update_member).delete(delete_member),
        )
        .route(
            "/api/workspaces/{id}/share-links",
            get(list_share_links).post(create_share_link),
        )
        .route(
            "/api/workspaces/{id}/share-links/{link_id}",
            delete(revoke_share_link),
        )
}

async fn accept_avatar_url(
    state: &HandlerState,
    raw: &str,
    current: &str,
) -> Result<String, Response> {
    crate::avatar::accept_url(state, raw, Some(current))
        .await
        .map_err(|message| error_response(StatusCode::FORBIDDEN, message))
}

fn resolve_avatar_url(state: &HandlerState, raw: Option<String>) -> Option<String> {
    raw.map(|value| crate::avatar::resolve_url(state, &value))
}

/// GET /api/share-links/{code} — public preview of a workspace share link.
async fn get_share_link_info(
    State(state): State<HandlerState>,
    Path(code): Path<String>,
) -> Response {
    let code = code.trim();
    if code.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "code is required");
    }
    let Some(row) = share_link::get_share_link_info_by_code(&state.pool, code)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "share link lookup failed");
            None
        })
    else {
        return error_response(StatusCode::NOT_FOUND, "share link not found or expired");
    };
    Json(ShareLinkInfoResponse {
        workspace_name: row.workspace_name,
        workspace_slug: row.workspace_slug,
        creator_name: row.creator_name,
        role: row.role,
    })
    .into_response()
}

#[derive(Serialize)]
struct ShareLinkInfoResponse {
    workspace_name: String,
    workspace_slug: String,
    creator_name: String,
    role: String,
}

/// GET /api/workspaces — list the workspaces visible to the authenticated
/// user. Authentication stamps `x-user-id`; never trust a client-provided
/// workspace id for this user-scoped collection.
async fn list_workspaces(
    State(state): State<HandlerState>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Some(user_id) = header_uuid(&headers, "x-user-id") else {
        return error_response(StatusCode::UNAUTHORIZED, "user not authenticated");
    };

    match workspace::list_workspaces(&state.pool, user_id).await {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|workspace| workspace_response(&state, workspace))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to list workspaces");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list workspaces",
            )
        }
    }
}

/// GET /api/workspaces/{id} — resolve membership before returning the row.
/// Returning 404 for non-members preserves the Go guard's non-enumeration
/// contract.
async fn get_workspace(
    State(state): State<HandlerState>,
    Path(raw_id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Response {
    let Ok(id) = Uuid::parse_str(raw_id.trim()) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid workspace_id");
    };
    if headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        == Some("task_token")
        && header_uuid(&headers, "x-workspace-id") != Some(id)
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "task token is bound to a different workspace",
        );
    }
    let Some(user_id) = header_uuid(&headers, "x-user-id") else {
        return error_response(StatusCode::UNAUTHORIZED, "user not authenticated");
    };
    let is_member = member::get_member_by_user_and_workspace(&state.pool, user_id, id)
        .await
        .ok()
        .flatten()
        .is_some();
    if !is_member {
        return error_response(StatusCode::NOT_FOUND, "workspace not found");
    }

    match workspace::get_workspace(&state.pool, id).await {
        Ok(Some(row)) => Json(workspace_response(&state, row)).into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "workspace not found"),
        Err(error) => {
            tracing::warn!(%error, workspace_id = %id, "failed to get workspace");
            error_response(StatusCode::NOT_FOUND, "workspace not found")
        }
    }
}

#[derive(Debug, Deserialize)]
struct JoinByShareLinkRequest {
    code: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct MemberWithUserResponse {
    id: String,
    workspace_id: String,
    user_id: String,
    role: String,
    created_at: String,
    name: String,
    email: String,
    avatar_url: Option<String>,
}

impl MemberWithUserResponse {
    pub(crate) fn new(member: &Member, user: &User) -> Self {
        Self {
            id: member.id.to_string(),
            workspace_id: member.workspace_id.to_string(),
            user_id: member.user_id.to_string(),
            role: member.role.clone(),
            created_at: crate::timefmt::rfc3339(member.created_at),
            name: user.name.clone(),
            email: user.email.clone(),
            avatar_url: user.avatar_url.clone(),
        }
    }

    pub(crate) fn new_resolved(state: &HandlerState, member: &Member, user: &User) -> Self {
        let mut response = Self::new(member, user);
        response.avatar_url = resolve_avatar_url(state, response.avatar_url);
        response
    }
}

#[derive(Debug, Serialize)]
struct JoinByShareLinkResponse {
    member: MemberWithUserResponse,
    workspace_id: String,
    workspace_slug: String,
}

async fn join_by_share_link(
    State(state): State<HandlerState>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    let Some(user_id) = header_uuid(&headers, "x-user-id") else {
        return error_response(StatusCode::UNAUTHORIZED, "user not authenticated");
    };
    let request: JoinByShareLinkRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let code = request.code.trim();
    if code.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "code is required");
    }
    let current_user = match user::get_user(&state.pool, user_id).await {
        Ok(Some(user)) => user,
        Ok(None) | Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load user")
        }
    };

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "failed to begin share-link join transaction");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to join workspace",
            );
        }
    };
    let link = match share_link::claim_share_link_by_code(&mut *transaction, code).await {
        Ok(Some(link)) => link,
        Ok(None) | Err(_) => {
            return error_response(StatusCode::NOT_FOUND, "share link not found or expired")
        }
    };
    match member::get_member_by_user_and_workspace(
        &mut *transaction,
        current_user.id,
        link.workspace_id,
    )
    .await
    {
        Ok(Some(_)) => {
            return error_response(
                StatusCode::CONFLICT,
                "you are already a member of this workspace",
            )
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(%error, "failed to check share-link membership");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create membership",
            );
        }
    }
    let joined_member = match member::create_member(
        &mut *transaction,
        link.workspace_id,
        current_user.id,
        &link.role,
    )
    .await
    {
        Ok(Some(member)) => member,
        Err(error) if unique_violation(&error) => {
            return error_response(
                StatusCode::CONFLICT,
                "you are already a member of this workspace",
            )
        }
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create membership",
            )
        }
    };
    if user::mark_user_onboarded(&mut *transaction, current_user.id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to finalize onboarding",
        );
    }
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, "failed to commit share-link join");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to join workspace",
        );
    }

    let workspace_id = link.workspace_id.to_string();
    let workspace_slug = workspace::get_workspace(&state.pool, link.workspace_id)
        .await
        .ok()
        .flatten()
        .map(|workspace| workspace.slug)
        .unwrap_or_default();
    let member_response =
        MemberWithUserResponse::new_resolved(&state, &joined_member, &current_user);
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::events::EVENT_MEMBER_ADDED.to_string(),
        workspace_id: workspace_id.clone(),
        actor_type: "member".to_string(),
        actor_id: user_id.to_string(),
        payload: serde_json::json!({"member": &member_response}),
        ..Default::default()
    });
    state
        .daemon_notifier
        .notify_workspaces_changed(&user_id.to_string())
        .await;

    Json(JoinByShareLinkResponse {
        member: member_response,
        workspace_id,
        workspace_slug,
    })
    .into_response()
}

pub(crate) fn unique_violation(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(|error| error.as_database_error())
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23505")
}

fn header_uuid(headers: &axum::http::HeaderMap, name: &str) -> Option<Uuid> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
}

#[derive(Debug, Serialize)]
struct WorkspaceResponse {
    id: String,
    name: String,
    slug: String,
    description: Option<String>,
    context: Option<String>,
    settings: serde_json::Value,
    repos: serde_json::Value,
    issue_prefix: String,
    avatar_url: Option<String>,
    created_at: String,
    updated_at: String,
}

impl From<Workspace> for WorkspaceResponse {
    fn from(workspace: Workspace) -> Self {
        Self {
            id: workspace.id.to_string(),
            name: workspace.name,
            slug: workspace.slug,
            description: workspace.description,
            context: workspace.context,
            settings: workspace.settings,
            repos: workspace.repos,
            issue_prefix: workspace.issue_prefix,
            avatar_url: workspace.avatar_url,
            created_at: crate::timefmt::rfc3339(workspace.created_at),
            updated_at: crate::timefmt::rfc3339(workspace.updated_at),
        }
    }
}

fn workspace_response(state: &HandlerState, workspace: Workspace) -> WorkspaceResponse {
    let mut response = WorkspaceResponse::from(workspace);
    response.avatar_url = resolve_avatar_url(state, response.avatar_url);
    response
}

#[derive(Deserialize)]
struct CreateWorkspaceRequest {
    name: String,
    slug: String,
    description: Option<String>,
    context: Option<String>,
    issue_prefix: Option<String>,
}

fn normalize_issue_prefix(raw: &str) -> Result<Option<String>, Response> {
    let prefix = raw.trim().to_ascii_uppercase();
    if prefix.is_empty() {
        return Ok(None);
    }
    if prefix.len() > 10
        || !prefix
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "issue prefix must be 1-10 uppercase letters or digits",
        ));
    }
    Ok(Some(prefix))
}

fn default_issue_prefix(slug: &str) -> String {
    slug.bytes()
        .filter(u8::is_ascii_alphanumeric)
        .take(4)
        .map(|byte| (byte as char).to_ascii_uppercase())
        .collect()
}

fn reserved_slug(slug: &str) -> bool {
    #[derive(Deserialize)]
    struct File {
        groups: Vec<Group>,
    }
    #[derive(Deserialize)]
    struct Group {
        slugs: Vec<String>,
    }
    let file: File = serde_json::from_str(include_str!("../assets/reserved_slugs.json"))
        .expect("reserved_slugs.json must be valid");
    file.groups
        .iter()
        .any(|group| group.slugs.iter().any(|item| item == slug))
}

async fn create_workspace(
    State(state): State<HandlerState>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    let Some(user_id) = header_uuid(&headers, "x-user-id") else {
        return error_response(StatusCode::UNAUTHORIZED, "user not authenticated");
    };
    if crate::config::workspace_creation_disabled() {
        return error_response(
            StatusCode::FORBIDDEN,
            "workspace creation is disabled for this instance",
        );
    }
    let mut request: CreateWorkspaceRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    request.name = request.name.trim().to_string();
    request.slug = request.slug.trim().to_ascii_lowercase();
    if request.name.is_empty() || request.slug.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "name and slug are required");
    }
    let slug_pattern = Regex::new(r"^[a-z0-9]+(?:-[a-z0-9]+)*$").expect("valid regex");
    if !slug_pattern.is_match(&request.slug) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "slug must contain only lowercase letters, numbers, and hyphens",
        );
    }
    if reserved_slug(&request.slug) {
        return error_response(StatusCode::BAD_REQUEST, "slug is reserved");
    }
    let issue_prefix = match request.issue_prefix.as_deref() {
        Some(raw) => match normalize_issue_prefix(raw) {
            Ok(Some(prefix)) => prefix,
            Ok(None) => default_issue_prefix(&request.slug),
            Err(response) => return response,
        },
        None => default_issue_prefix(&request.slug),
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create workspace",
            )
        }
    };
    // Guest sessions get one workspace. Locking their user row for the
    // duration of this transaction makes the quota atomic across concurrent
    // create requests, while formal users retain the existing behavior.
    let is_guest = headers
        .get("x-guest-user")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == "true");
    if is_guest {
        match user::get_user_for_update(&mut *transaction, user_id).await {
            Ok(Some(guest_user)) if guest_user.is_guest => {}
            Ok(Some(_)) => return error_response(StatusCode::FORBIDDEN, "formal login required"),
            Ok(None) => return error_response(StatusCode::UNAUTHORIZED, "user not found"),
            Err(error) => {
                tracing::warn!(%error, %user_id, "failed to lock guest workspace quota");
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "guest workspace quota unavailable",
                );
            }
        }
        match workspace::list_workspaces(&mut *transaction, user_id).await {
            Ok(workspaces) if !workspaces.is_empty() => {
                return error_response(
                    StatusCode::FORBIDDEN,
                    "guest workspace limit reached; formal login required",
                )
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, %user_id, "failed to read guest workspace quota");
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "guest workspace quota unavailable",
                );
            }
        }
    }
    let created = match workspace::create_workspace(
        &mut *transaction,
        &request.name,
        &request.slug,
        request.description.as_deref(),
        request.context.as_deref(),
        &issue_prefix,
    )
    .await
    {
        Ok(Some(created)) => created,
        Err(error) if unique_violation(&error) => {
            return error_response(StatusCode::CONFLICT, "workspace slug already exists")
        }
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create workspace",
            )
        }
    };
    if member::create_member(&mut *transaction, created.id, user_id, "owner")
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to add owner");
    }
    if patchbay_service::issue_status::ensure(&mut *transaction, created.id)
        .await
        .is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to seed issue statuses",
        );
    }
    if transaction.commit().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create workspace",
        );
    }
    let workspace_id = created.id.to_string();
    let event = patchbay_analytics::workspace_created(&user_id.to_string(), &workspace_id);
    state.analytics.capture(event.clone());
    if let Some(metrics) = state.business_metrics.as_deref() {
        metrics.inc_for_event(&event);
    }
    state
        .daemon_notifier
        .notify_workspaces_changed(&user_id.to_string())
        .await;
    (
        StatusCode::CREATED,
        Json(workspace_response(&state, created)),
    )
        .into_response()
}

#[derive(Default, Deserialize)]
struct UpdateWorkspaceRequest {
    name: Option<String>,
    description: Option<String>,
    context: Option<String>,
    settings: Option<serde_json::Value>,
    repos: Option<serde_json::Value>,
    issue_prefix: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct WorkspaceRepoRef {
    url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
}

fn valid_git_url(value: &str) -> bool {
    if let Ok(url) = url::Url::parse(value) {
        if url.host_str().is_some() && matches!(url.scheme(), "http" | "https" | "ssh" | "git") {
            return true;
        }
    }
    if value.contains(' ') || value.contains("://") {
        return false;
    }
    let Some(colon) = value.find(':') else {
        return false;
    };
    if colon == 0 || colon + 1 == value.len() {
        return false;
    }
    !value.find('@').is_some_and(|at| at >= colon)
}

fn normalize_repos(value: serde_json::Value) -> Result<serde_json::Value, Response> {
    let repos: Vec<WorkspaceRepoRef> = serde_json::from_value(value).map_err(|_| {
        error_response(
            StatusCode::BAD_REQUEST,
            "repos must be an array of repository objects",
        )
    })?;
    let mut seen = std::collections::HashSet::new();
    let mut normalized = Vec::new();
    for (index, mut repo) in repos.into_iter().enumerate() {
        repo.url = repo.url.trim().to_string();
        repo.description = repo.description.trim().to_string();
        if repo.url.is_empty() {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                &format!("repos[{index}]: url is required"),
            ));
        }
        if !valid_git_url(&repo.url) {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                &format!("repos[{index}]: url must be a valid http(s) or ssh git URL"),
            ));
        }
        if seen.insert(repo.url.clone()) {
            normalized.push(repo);
        }
    }
    Ok(serde_json::to_value(normalized).expect("serializable repos"))
}

async fn update_workspace(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    body: Bytes,
) -> Response {
    let mut request: UpdateWorkspaceRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if let Some(name) = request.name.as_mut() {
        *name = name.trim().to_string();
        if name.is_empty() {
            return error_response(StatusCode::BAD_REQUEST, "name is required");
        }
    }
    let prefix = match request.issue_prefix.as_deref() {
        Some(raw) => match normalize_issue_prefix(raw) {
            Ok(value) => value,
            Err(response) => return response,
        },
        None => None,
    };
    let repos = match request.repos.take() {
        Some(value) => match normalize_repos(value) {
            Ok(value) => Some(value),
            Err(response) => return response,
        },
        None => None,
    };
    let settings = request.settings;
    let avatar = match request.avatar_url.as_deref() {
        Some(raw) => {
            let current = workspace::get_workspace(&state.pool, context.member.workspace_id)
                .await
                .ok()
                .flatten()
                .and_then(|workspace| workspace.avatar_url)
                .unwrap_or_default();
            match accept_avatar_url(&state, raw, &current).await {
                Ok(value) => Some(value),
                Err(response) => return response,
            }
        }
        None => None,
    };
    let updated = match workspace::update_workspace(
        &state.pool,
        context.member.workspace_id,
        request.name.as_deref(),
        request.description.as_deref(),
        request.context.as_deref(),
        settings.as_ref(),
        repos.as_ref(),
        prefix.as_deref(),
        avatar.as_deref(),
    )
    .await
    {
        Ok(Some(updated)) => updated,
        _ => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update workspace",
            )
        }
    };
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::events::EVENT_WORKSPACE_UPDATED.into(),
        workspace_id: updated.id.to_string(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload: serde_json::json!({"workspace": workspace_response(&state, updated.clone())}),
        ..Default::default()
    });
    if request.name.is_some() {
        if let Ok(members) = member::list_members(&state.pool, updated.id).await {
            for member in members {
                state
                    .daemon_notifier
                    .notify_workspaces_changed(&member.user_id.to_string())
                    .await;
            }
        }
    }
    Json(workspace_response(&state, updated)).into_response()
}

#[derive(Serialize)]
struct MemberListResponse {
    id: String,
    workspace_id: String,
    user_id: String,
    role: String,
    created_at: String,
    name: String,
    email: String,
    avatar_url: Option<String>,
}

async fn list_members(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    match member::list_members_with_user(&state.pool, context.member.workspace_id).await {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| MemberListResponse {
                    id: row.id.map(|id| id.to_string()).unwrap_or_default(),
                    workspace_id: row
                        .workspace_id
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                    user_id: row.user_id.map(|id| id.to_string()).unwrap_or_default(),
                    role: row.role,
                    created_at: row
                        .created_at
                        .map(crate::timefmt::rfc3339)
                        .unwrap_or_default(),
                    name: row.user_name,
                    email: row.user_email,
                    avatar_url: resolve_avatar_url(&state, row.user_avatar_url),
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list members"),
    }
}

fn normalized_member_role(role: &str, allow_default: bool) -> Option<&str> {
    let role = role.trim();
    if role.is_empty() && allow_default {
        return Some("member");
    }
    matches!(role, "owner" | "admin" | "member").then_some(role)
}

#[derive(Deserialize)]
struct MemberPath {
    member_id: String,
}

#[derive(Deserialize)]
struct UpdateMemberRequest {
    role: String,
}

async fn update_member(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(path): Path<MemberPath>,
    body: Bytes,
) -> Response {
    let Ok(member_id) = Uuid::parse_str(&path.member_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid member id");
    };
    let target = match member::get_member(&state.pool, member_id).await {
        Ok(Some(target)) if target.workspace_id == context.member.workspace_id => target,
        _ => return error_response(StatusCode::NOT_FOUND, "member not found"),
    };
    let request: UpdateMemberRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.role.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "role is required");
    }
    let Some(role) = normalized_member_role(&request.role, false) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid member role");
    };
    if (target.role == "owner" || role == "owner") && context.member.role != "owner" {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if target.role == "owner" && role != "owner" {
        match member::list_members(&state.pool, target.workspace_id).await {
            Ok(rows) if rows.iter().filter(|member| member.role == "owner").count() <= 1 => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "workspace must have at least one owner",
                )
            }
            Err(_) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update member")
            }
            _ => {}
        }
    }
    let updated = match member::update_member_role(&state.pool, target.id, role).await {
        Ok(Some(updated)) => updated,
        _ => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update member"),
    };
    state
        .membership_cache
        .invalidate(
            &target.user_id.to_string(),
            &target.workspace_id.to_string(),
        )
        .await;
    let found_user = match user::get_user(&state.pool, updated.user_id).await {
        Ok(Some(user)) => user,
        _ => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load member"),
    };
    let response = MemberWithUserResponse::new_resolved(&state, &updated, &found_user);
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::events::EVENT_MEMBER_UPDATED.into(),
        workspace_id: updated.workspace_id.to_string(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload: serde_json::json!({"member": &response}),
        ..Default::default()
    });
    Json(response).into_response()
}

async fn revoke_and_remove_member(
    state: &HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    member_id: Uuid,
    archived_by: Uuid,
) -> anyhow::Result<MemberRevocation> {
    let mut transaction = state.pool.begin().await?;
    patchbay_db::queries::subscriber::lock_subscriber_writes(
        &mut *transaction,
        workspace_id,
        user_id,
    )
    .await?;
    let runtimes = patchbay_db::queries::runtime::list_agent_runtimes_by_owner(
        &mut *transaction,
        workspace_id,
        user_id,
    )
    .await?;
    let runtime_ids: Vec<Uuid> = runtimes.iter().map(|runtime| runtime.id).collect();
    let mut result = MemberRevocation::default();
    if !runtime_ids.is_empty() {
        result.archived_agents = patchbay_db::queries::agent::archive_agents_by_runtime(
            &mut *transaction,
            archived_by,
            runtime_ids.clone(),
        )
        .await?;
        let agent_ids: Vec<Uuid> = result
            .archived_agents
            .iter()
            .map(|agent| agent.id)
            .collect();
        result.cancelled_tasks =
            patchbay_db::queries::runtime::cancel_agent_tasks_by_runtime_or_agent(
                &mut *transaction,
                runtime_ids.clone(),
                agent_ids,
            )
            .await?;
        result.offline_runtime_ids = patchbay_db::queries::runtime::force_offline_runtimes_by_i_ds(
            &mut *transaction,
            runtime_ids,
        )
        .await?
        .into_iter()
        .filter_map(|runtime| runtime.id)
        .collect();
        let daemon_ids: Vec<String> = runtimes
            .into_iter()
            .filter_map(|runtime| runtime.daemon_id)
            .filter(|id| !id.is_empty())
            .collect();
        if !daemon_ids.is_empty() {
            result.revoked_token_hashes =
                patchbay_db::queries::daemon_token::delete_daemon_tokens_by_workspace_and_daemons(
                    &mut *transaction,
                    workspace_id,
                    &daemon_ids,
                )
                .await?;
        }
    }
    patchbay_db::queries::channel::delete_channel_user_bindings_by_workspace_member(
        &mut *transaction,
        workspace_id,
        user_id,
    )
    .await?;
    patchbay_db::queries::agent_invocation_target::delete_agent_invocation_targets_by_member(
        &mut *transaction,
        workspace_id,
        user_id,
    )
    .await?;
    patchbay_db::queries::quick_action::delete_private_quick_actions_by_creator(
        &mut *transaction,
        workspace_id,
        user_id,
    )
    .await?;
    patchbay_db::queries::issue_view::delete_private_issue_views_by_owner(
        &mut *transaction,
        workspace_id,
        user_id,
    )
    .await?;
    patchbay_db::queries::issue_view::delete_issue_view_preferences_by_user(
        &mut *transaction,
        workspace_id,
        user_id,
    )
    .await?;
    patchbay_db::queries::subscriber::delete_subscriptions_by_member(
        &mut *transaction,
        workspace_id,
        user_id,
    )
    .await?;
    member::delete_member(&mut *transaction, member_id).await?;
    transaction.commit().await?;
    Ok(result)
}

#[derive(Default)]
struct MemberRevocation {
    archived_agents: Vec<Agent>,
    cancelled_tasks: Vec<AgentTaskQueue>,
    offline_runtime_ids: Vec<Uuid>,
    revoked_token_hashes: Vec<String>,
}

async fn publish_member_revocation(
    state: &HandlerState,
    workspace_id: Uuid,
    actor_id: Uuid,
    result: &MemberRevocation,
) {
    for hash in &result.revoked_token_hashes {
        state.daemon_token_cache.invalidate(hash).await;
    }
    state
        .tasks
        .broadcast_cancelled_tasks(&workspace_id.to_string(), &result.cancelled_tasks)
        .await;
    for agent in &result.archived_agents {
        let mut response = serde_json::to_value(agent).unwrap_or_default();
        if let Some(object) = response.as_object_mut() {
            let env_count = object
                .remove("custom_env")
                .and_then(|value| value.as_object().map(|env| env.len()))
                .unwrap_or_default();
            object.remove("mcp_config");
            object.remove("composio_toolkit_allowlist");
            object.insert("has_custom_env".into(), serde_json::json!(env_count > 0));
            object.insert("custom_env_key_count".into(), serde_json::json!(env_count));
            object.insert("mcp_config".into(), serde_json::json!({}));
            object.insert("mcp_config_redacted".into(), serde_json::json!(true));
        }
        state.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_AGENT_ARCHIVED.into(),
            workspace_id: workspace_id.to_string(),
            actor_type: "member".into(),
            actor_id: actor_id.to_string(),
            payload: serde_json::json!({"agent": response}),
            ..Default::default()
        });
    }
    if !result.offline_runtime_ids.is_empty() {
        state.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_DAEMON_REGISTER.into(),
            workspace_id: workspace_id.to_string(),
            actor_type: "member".into(),
            actor_id: actor_id.to_string(),
            payload: serde_json::json!({"action": "revoke"}),
            ..Default::default()
        });
    }
}

async fn remove_member_common(
    state: &HandlerState,
    context: &WorkspaceContext,
    target: Member,
) -> Response {
    if target.role == "owner" && context.member.role != "owner" {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if target.role == "owner" {
        match member::list_members(&state.pool, target.workspace_id).await {
            Ok(rows) if rows.iter().filter(|member| member.role == "owner").count() <= 1 => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "workspace must have at least one owner",
                )
            }
            Err(_) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to delete member")
            }
            _ => {}
        }
    }
    let revocation = match revoke_and_remove_member(
        state,
        target.workspace_id,
        target.user_id,
        target.id,
        context.member.user_id,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(%error, member_id = %target.id, "failed to revoke member");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to delete member");
        }
    };
    state
        .membership_cache
        .invalidate(
            &target.user_id.to_string(),
            &target.workspace_id.to_string(),
        )
        .await;
    publish_member_revocation(
        state,
        target.workspace_id,
        context.member.user_id,
        &revocation,
    )
    .await;
    let workspace_id = target.workspace_id.to_string();
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::events::EVENT_MEMBER_REMOVED.into(), workspace_id: workspace_id.clone(),
        actor_type: "member".into(), actor_id: context.member.user_id.to_string(),
        payload: serde_json::json!({"member_id": target.id, "workspace_id": workspace_id, "user_id": target.user_id}),
        ..Default::default()
    });
    state
        .daemon_notifier
        .notify_workspaces_changed(&target.user_id.to_string())
        .await;
    StatusCode::NO_CONTENT.into_response()
}

async fn delete_member(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(path): Path<MemberPath>,
) -> Response {
    let Ok(member_id) = Uuid::parse_str(&path.member_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid member id");
    };
    let target = match member::get_member(&state.pool, member_id).await {
        Ok(Some(target)) if target.workspace_id == context.member.workspace_id => target,
        _ => return error_response(StatusCode::NOT_FOUND, "member not found"),
    };
    remove_member_common(&state, &context, target).await
}

async fn leave_workspace(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let response = remove_member_common(&state, &context, context.member.clone()).await;
    if response.status() == StatusCode::INTERNAL_SERVER_ERROR {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to leave workspace",
        );
    }
    response
}

#[derive(Serialize)]
struct ShareLinkResponse {
    id: String,
    workspace_id: String,
    code: String,
    created_by: String,
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_uses: Option<i32>,
    use_count: i32,
    is_active: bool,
    created_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    creator_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    creator_email: String,
}

impl From<patchbay_db::models::WorkspaceShareLink> for ShareLinkResponse {
    fn from(link: patchbay_db::models::WorkspaceShareLink) -> Self {
        Self {
            id: link.id.to_string(),
            workspace_id: link.workspace_id.to_string(),
            code: link.code,
            created_by: link.created_by.to_string(),
            role: link.role,
            expires_at: link.expires_at.map(crate::timefmt::rfc3339),
            max_uses: link.max_uses,
            use_count: link.use_count,
            is_active: link.is_active,
            created_at: crate::timefmt::rfc3339(link.created_at),
            creator_name: String::new(),
            creator_email: String::new(),
        }
    }
}

#[derive(Deserialize)]
struct CreateShareLinkRequest {
    #[serde(default)]
    role: String,
    expires_in: Option<i64>,
    max_uses: Option<i64>,
}

async fn create_share_link(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    body: Bytes,
) -> Response {
    let request: CreateShareLinkRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let role = match request.role.trim().to_ascii_lowercase().as_str() {
        "" | "member" => "member",
        "admin" => "admin",
        _ => return error_response(StatusCode::BAD_REQUEST, "invalid role"),
    };
    const MAX_HOURS: i64 = i64::MAX / 3_600_000_000_000;
    let expires_at = match request.expires_in {
        Some(hours) if !(1..=MAX_HOURS).contains(&hours) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("expires_in must be between 1 and {MAX_HOURS} hours"),
            )
        }
        Some(hours) => chrono::Utc::now().checked_add_signed(chrono::Duration::hours(hours)),
        None => None,
    };
    let max_uses = match request.max_uses {
        Some(value) if !(1..=i32::MAX as i64).contains(&value) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "max_uses must be between 1 and 2147483647",
            )
        }
        Some(value) => Some(value as i32),
        None => None,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create share link",
            )
        }
    };
    if share_link::deactivate_workspace_share_links(&mut *transaction, context.member.workspace_id)
        .await
        .is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create share link",
        );
    }
    let mut bytes = [0_u8; 12];
    rand::thread_rng().fill_bytes(&mut bytes);
    let code = hex::encode(bytes);
    let link = match share_link::create_share_link(
        &mut *transaction,
        context.member.workspace_id,
        &code,
        context.member.user_id,
        role,
        expires_at,
        max_uses,
    )
    .await
    {
        Ok(Some(link)) => link,
        Err(error) if unique_violation(&error) => {
            return error_response(
                StatusCode::CONFLICT,
                "a share link is already active for this workspace",
            )
        }
        _ => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create share link",
            )
        }
    };
    if transaction.commit().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create share link",
        );
    }
    (StatusCode::CREATED, Json(ShareLinkResponse::from(link))).into_response()
}

async fn list_share_links(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    match share_link::list_share_links_by_workspace(&state.pool, context.member.workspace_id).await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| ShareLinkResponse {
                    id: row.id.map(|id| id.to_string()).unwrap_or_default(),
                    workspace_id: row
                        .workspace_id
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                    code: row.code,
                    created_by: row.created_by.map(|id| id.to_string()).unwrap_or_default(),
                    role: row.role,
                    expires_at: row.expires_at.map(crate::timefmt::rfc3339),
                    max_uses: row.max_uses,
                    use_count: row.use_count,
                    is_active: row.is_active,
                    created_at: row
                        .created_at
                        .map(crate::timefmt::rfc3339)
                        .unwrap_or_default(),
                    creator_name: row.creator_name,
                    creator_email: row.creator_email,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list share links",
        ),
    }
}

#[derive(Deserialize)]
struct ShareLinkPath {
    link_id: String,
}

async fn revoke_share_link(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(path): Path<ShareLinkPath>,
) -> Response {
    let Ok(link_id) = Uuid::parse_str(&path.link_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid link id");
    };
    match share_link::revoke_share_link(&state.pool, link_id, context.member.workspace_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to revoke share link",
        ),
    }
}

#[derive(Clone, Copy)]
enum TaskOwnerKind {
    Agent,
    Issue,
    Runtime,
}

async fn workspace_owner_page(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    kind: TaskOwnerKind,
    cursor: Option<Uuid>,
) -> anyhow::Result<Vec<Uuid>> {
    const LIMIT: i32 = 500;
    use patchbay_db::queries::workspace_delete as deletion;
    let rows = match (kind, cursor) {
        (TaskOwnerKind::Agent, None) => {
            deletion::list_workspace_agent_id_first_page(&mut **transaction, workspace_id, LIMIT)
                .await?
        }
        (TaskOwnerKind::Agent, Some(cursor)) => {
            deletion::list_workspace_agent_id_page(&mut **transaction, workspace_id, cursor, LIMIT)
                .await?
        }
        (TaskOwnerKind::Issue, None) => {
            deletion::list_workspace_issue_id_first_page(&mut **transaction, workspace_id, LIMIT)
                .await?
        }
        (TaskOwnerKind::Issue, Some(cursor)) => {
            deletion::list_workspace_issue_id_page(&mut **transaction, workspace_id, cursor, LIMIT)
                .await?
        }
        (TaskOwnerKind::Runtime, None) => {
            deletion::list_workspace_runtime_id_first_page(&mut **transaction, workspace_id, LIMIT)
                .await?
        }
        (TaskOwnerKind::Runtime, Some(cursor)) => {
            deletion::list_workspace_runtime_id_page(
                &mut **transaction,
                workspace_id,
                cursor,
                LIMIT,
            )
            .await?
        }
    };
    Ok(rows.into_iter().flatten().collect())
}

async fn task_page(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner_id: Uuid,
    kind: TaskOwnerKind,
    cursor: Option<Uuid>,
) -> anyhow::Result<Vec<Uuid>> {
    const LIMIT: i32 = 1000;
    use patchbay_db::queries::workspace_delete as deletion;
    let rows = match (kind, cursor) {
        (TaskOwnerKind::Agent, None) => {
            deletion::list_task_i_ds_by_agent_first_page(&mut **transaction, owner_id, LIMIT)
                .await?
        }
        (TaskOwnerKind::Agent, Some(cursor)) => {
            deletion::list_task_i_ds_by_agent_page(&mut **transaction, owner_id, cursor, LIMIT)
                .await?
        }
        (TaskOwnerKind::Issue, None) => {
            deletion::list_task_i_ds_by_issue_first_page(&mut **transaction, owner_id, LIMIT)
                .await?
        }
        (TaskOwnerKind::Issue, Some(cursor)) => {
            deletion::list_task_i_ds_by_issue_page(&mut **transaction, owner_id, cursor, LIMIT)
                .await?
        }
        (TaskOwnerKind::Runtime, None) => {
            deletion::list_task_i_ds_by_runtime_first_page(&mut **transaction, owner_id, LIMIT)
                .await?
        }
        (TaskOwnerKind::Runtime, Some(cursor)) => {
            deletion::list_task_i_ds_by_runtime_page(&mut **transaction, owner_id, cursor, LIMIT)
                .await?
        }
    };
    Ok(rows.into_iter().flatten().collect())
}

async fn sweep_tasks_for_owner(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner_id: Uuid,
    kind: TaskOwnerKind,
) -> anyhow::Result<()> {
    use patchbay_db::queries::workspace_delete as deletion;
    for _ in 0..3 {
        let mut cursor = None;
        let mut deleted = 0_usize;
        loop {
            let task_ids = task_page(transaction, owner_id, kind, cursor).await?;
            let Some(last_id) = task_ids.last().copied() else {
                break;
            };
            deletion::detach_task_batch_references(&mut **transaction, task_ids.clone()).await?;
            deletion::delete_task_batch(&mut **transaction, task_ids.clone()).await?;
            deleted += task_ids.len();
            cursor = Some(last_id);
        }
        if deleted == 0 {
            return Ok(());
        }
    }
    anyhow::bail!("tasks kept appearing after the workspace owner was fenced")
}

async fn delete_workspace_tasks(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<()> {
    for kind in [
        TaskOwnerKind::Agent,
        TaskOwnerKind::Issue,
        TaskOwnerKind::Runtime,
    ] {
        let mut cursor = None;
        loop {
            let owners = workspace_owner_page(transaction, workspace_id, kind, cursor).await?;
            let Some(last_id) = owners.last().copied() else {
                break;
            };
            for owner_id in owners {
                sweep_tasks_for_owner(transaction, owner_id, kind).await?;
                if matches!(kind, TaskOwnerKind::Agent) {
                    patchbay_db::queries::workspace_delete::delete_task_tokens_by_agent(
                        &mut **transaction,
                        owner_id,
                    )
                    .await?;
                }
            }
            cursor = Some(last_id);
        }
    }
    Ok(())
}

fn retryable_lock_error(error: &impl std::fmt::Display) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("55p03")
        || message.contains("40p01")
        || message.contains("lock timeout")
        || message.contains("deadlock detected")
}

fn workspace_delete_error(retryable: bool) -> Response {
    error_response(
        if retryable {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        },
        if retryable {
            "workspace deletion is temporarily blocked by another operation, please try again"
        } else {
            "failed to delete workspace"
        },
    )
}

async fn delete_workspace(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    if context.member.role != "owner" {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    let workspace_id = context.member.workspace_id;
    let affected_users = member::list_members(&state.pool, workspace_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|member| member.user_id)
        .collect::<Vec<_>>();
    for user_id in &affected_users {
        state
            .membership_cache
            .invalidate(&user_id.to_string(), &workspace_id.to_string())
            .await;
    }
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete workspace",
            )
        }
    };
    macro_rules! step {
        ($name:literal, $expr:expr) => {
            if let Err(error) = $expr.await {
                tracing::warn!(%error, workspace_id = %workspace_id, step = $name, "workspace delete failed");
                return workspace_delete_error(retryable_lock_error(&error));
            }
        };
    }
    step!(
        "set lock timeout",
        sqlx::query("SET LOCAL lock_timeout = '10s'").execute(&mut *tx)
    );
    step!(
        "lock workspace",
        workspace::lock_workspace_for_delete(&mut *tx, workspace_id)
    );
    step!(
        "lock work products",
        patchbay_db::queries::workspace_delete::lock_workspace_work_products(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "lock chat sessions",
        patchbay_db::queries::chat::lock_chat_sessions_by_workspace(&mut *tx, workspace_id)
    );
    step!(
        "set teardown mode",
        patchbay_db::queries::workspace_delete::set_workspace_teardown_mode(&mut *tx)
    );
    step!(
        "lock agents",
        patchbay_db::queries::workspace_delete::lock_workspace_task_owner_agents(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "lock issues",
        patchbay_db::queries::workspace_delete::lock_workspace_task_owner_issues(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "lock runtimes",
        patchbay_db::queries::workspace_delete::lock_workspace_task_owner_runtimes(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "prepare links",
        patchbay_db::queries::workspace_delete::prepare_workspace_deletion_links(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete chat pins",
        patchbay_db::queries::chat_pinned_agent::delete_chat_pinned_agents_by_workspace(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "lock usage rollup",
        patchbay_db::queries::workspace_delete::lock_task_usage_rollup_for_workspace_delete(
            &mut *tx
        )
    );
    step!(
        "delete tasks",
        delete_workspace_tasks(&mut tx, workspace_id)
    );
    step!(
        "delete leaf data",
        patchbay_db::queries::workspace_delete::delete_workspace_leaf_data(&mut *tx, workspace_id)
    );
    step!(
        "delete automation runs",
        patchbay_db::queries::workspace_delete::delete_workspace_automation_runs(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete quota reservations",
        patchbay_db::queries::workspace_delete::delete_workspace_automation_quota_reservations(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete quota periods",
        patchbay_db::queries::workspace_delete::delete_workspace_automation_quota_periods(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete chat messages",
        patchbay_db::queries::workspace_delete::delete_workspace_chat_messages(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete communication roots",
        patchbay_db::queries::workspace_delete::delete_workspace_communication_roots(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete comments",
        patchbay_db::queries::workspace_delete::delete_workspace_comments(&mut *tx, workspace_id)
    );
    step!(
        "delete issue roots",
        patchbay_db::queries::workspace_delete::delete_workspace_issue_roots(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete issue statuses",
        patchbay_db::queries::issue_status::delete_issue_status_entries_for_workspace(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete issue category policies",
        patchbay_db::queries::workspace_issue_category_policy::delete_workspace_issue_category_policies(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete automation children",
        patchbay_db::queries::workspace_delete::delete_workspace_automation_children(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete automations",
        patchbay_db::queries::workspace_delete::delete_workspace_automations(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete pull requests",
        patchbay_db::queries::workspace_delete::delete_workspace_pull_requests(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete Linear integration data",
        patchbay_db::queries::workspace_delete::delete_workspace_linear_data(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete integrations",
        patchbay_db::queries::workspace_delete::delete_workspace_connections(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete teams and skills",
        patchbay_db::queries::workspace_delete::delete_workspace_teams_and_skills(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete plugin data",
        patchbay_db::queries::workspace_delete::delete_workspace_plugin_data(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete agents",
        patchbay_db::queries::workspace_delete::delete_workspace_agents(&mut *tx, workspace_id)
    );
    step!(
        "delete runtimes and projects",
        patchbay_db::queries::workspace_delete::delete_workspace_runtimes_and_projects(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete administration",
        patchbay_db::queries::workspace_delete::delete_workspace_administration(
            &mut *tx,
            workspace_id
        )
    );
    step!(
        "delete workspace",
        workspace::delete_workspace(&mut *tx, workspace_id)
    );
    if let Err(error) = tx.commit().await {
        return workspace_delete_error(retryable_lock_error(&error));
    }
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::events::EVENT_WORKSPACE_DELETED.into(),
        workspace_id: workspace_id.to_string(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload: serde_json::json!({"workspace_id": workspace_id}),
        ..Default::default()
    });
    for user_id in affected_users {
        state
            .daemon_notifier
            .notify_workspaces_changed(&user_id.to_string())
            .await;
    }
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::middleware;
    use chrono::{TimeZone, Utc};
    use patchbay_middleware::workspace::WorkspaceGuardState;
    use serde_json::json;
    use tower::ServiceExt;

    fn test_router() -> Router {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let state = HandlerState::new(
            pool.clone(),
            patchbay_auth::pat_cache::PatCache::disabled(),
            None,
        );
        authenticated_router().with_state(state)
    }

    fn guarded_workspace_router() -> Router {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let state = HandlerState::new(
            pool.clone(),
            patchbay_auth::pat_cache::PatCache::disabled(),
            None,
        );
        member_router()
            .route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::from_url(pool.clone(), "id"),
                patchbay_middleware::workspace::require_workspace,
            ))
            .merge(admin_router().route_layer(middleware::from_fn_with_state(
                WorkspaceGuardState::from_url_with_roles(
                    pool.clone(),
                    "id",
                    vec!["owner".into(), "admin".into()],
                ),
                patchbay_middleware::workspace::require_workspace,
            )))
            .merge(crate::invitation::workspace_admin_router().route_layer(
                middleware::from_fn_with_state(
                    WorkspaceGuardState::from_url_with_roles(
                        pool,
                        "id",
                        vec!["owner".into(), "admin".into()],
                    ),
                    patchbay_middleware::workspace::require_workspace,
                ),
            ))
            .with_state(state)
    }

    #[tokio::test]
    async fn workspace_collection_create_requires_authentication() {
        let response = test_router()
            .oneshot(
                Request::post("/api/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Patchbay","slug":"patchbay-team"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn workspace_member_and_admin_routes_require_authentication() {
        let workspace_id = Uuid::new_v4();
        for (method, path) in [
            ("GET", format!("/api/workspaces/{workspace_id}/members")),
            ("POST", format!("/api/workspaces/{workspace_id}/leave")),
            ("POST", format!("/api/workspaces/{workspace_id}/members")),
            ("PATCH", format!("/api/workspaces/{workspace_id}")),
            ("GET", format!("/api/workspaces/{workspace_id}/share-links")),
        ] {
            let response = guarded_workspace_router()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{method}");
        }
    }

    #[tokio::test]
    async fn task_token_cannot_cross_workspace_on_new_routes() {
        let requested = Uuid::new_v4();
        let bound = Uuid::new_v4();
        let response = guarded_workspace_router()
            .oneshot(
                Request::get(format!("/api/workspaces/{requested}/members"))
                    .header("x-actor-source", "task_token")
                    .header("x-workspace-id", bound.to_string())
                    .header("x-user-id", Uuid::new_v4().to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn workspace_input_normalization_matches_go() {
        assert_eq!(default_issue_prefix("front-end"), "FRON");
        assert_eq!(default_issue_prefix("team-2"), "TEAM");
        assert_eq!(
            normalize_issue_prefix(" ab12 ").unwrap(),
            Some("AB12".into())
        );
        assert!(normalize_issue_prefix("bad-prefix").is_err());
        assert!(reserved_slug("login"));
        assert!(!reserved_slug("patchbay-team"));
        assert_eq!(normalized_member_role("", true), Some("member"));
        assert_eq!(normalized_member_role("owner", false), Some("owner"));
        assert_eq!(normalized_member_role(" OWNER ", false), None);
    }

    #[tokio::test]
    async fn malformed_workspace_id_uses_json_error_contract() {
        let response = test_router()
            .oneshot(
                Request::get("/api/workspaces/not-a-uuid")
                    .header("x-user-id", Uuid::nil().to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"{\"error\":\"invalid workspace_id\"}\n");
    }

    #[tokio::test]
    async fn task_token_cannot_cross_its_bound_workspace() {
        let requested = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let bound = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        let response = test_router()
            .oneshot(
                Request::get(format!("/api/workspaces/{requested}"))
                    .header("x-user-id", Uuid::nil().to_string())
                    .header("x-actor-source", "task_token")
                    .header("x-workspace-id", bound.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn workspace_response_matches_go_wire_shape() {
        let id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f11").unwrap();
        let timestamp = Utc.with_ymd_and_hms(2026, 8, 23, 2, 30, 0).unwrap();
        let response = WorkspaceResponse::from(Workspace {
            attribution_fail_closed: false,
            avatar_url: None,
            context: Some("context".into()),
            created_at: timestamp,
            description: None,
            id,
            issue_counter: 7,
            issue_prefix: "CORD".into(),
            name: "Patchbay".into(),
            repos: json!([]),
            settings: json!({}),
            slug: "patchbay".into(),
            updated_at: timestamp,
        });

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "id": id.to_string(),
                "name": "Patchbay",
                "slug": "patchbay",
                "description": null,
                "context": "context",
                "settings": {},
                "repos": [],
                "issue_prefix": "CORD",
                "avatar_url": null,
                "created_at": "2026-08-23T02:30:00Z",
                "updated_at": "2026-08-23T02:30:00Z"
            })
        );
    }

    #[test]
    fn workspace_response_strips_fractional_timestamp_seconds() {
        let timestamp = chrono::DateTime::parse_from_rfc3339("2026-08-23T02:30:00.987Z")
            .unwrap()
            .with_timezone(&Utc);
        let response = WorkspaceResponse::from(Workspace {
            attribution_fail_closed: false,
            avatar_url: None,
            context: None,
            created_at: timestamp,
            description: None,
            id: Uuid::nil(),
            issue_counter: 0,
            issue_prefix: "CORD".into(),
            name: "Patchbay".into(),
            repos: json!([]),
            settings: json!({}),
            slug: "patchbay".into(),
            updated_at: timestamp,
        });

        assert_eq!(response.created_at, "2026-08-23T02:30:00Z");
        assert_eq!(response.updated_at, "2026-08-23T02:30:00Z");
    }

    #[test]
    fn joined_member_response_matches_go_wire_shape() {
        let timestamp = Utc.with_ymd_and_hms(2026, 8, 23, 8, 0, 0).unwrap();
        let workspace_id = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let user_id = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        let member = Member {
            created_at: timestamp,
            id: Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap(),
            role: "member".into(),
            user_id,
            workspace_id,
        };
        let user = User {
            avatar_url: None,
            cloud_waitlist_email: None,
            cloud_waitlist_reason: None,
            created_at: timestamp,
            email: "alex@example.com".into(),
            id: user_id,
            is_guest: false,
            language: None,
            name: "Alex".into(),
            onboarded_at: None,
            onboarding_questionnaire: json!({}),
            profile_description: String::new(),
            starter_content_state: None,
            timezone: None,
            updated_at: timestamp,
        };

        let value = serde_json::to_value(MemberWithUserResponse::new(&member, &user)).unwrap();
        assert_eq!(value["workspace_id"], workspace_id.to_string());
        assert_eq!(value["user_id"], user_id.to_string());
        assert_eq!(value["role"], "member");
        assert_eq!(value["name"], "Alex");
        assert_eq!(value["avatar_url"], serde_json::Value::Null);
        assert_eq!(value["created_at"], "2026-08-23T08:00:00Z");
    }
}
