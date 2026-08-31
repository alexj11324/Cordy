//! Linear installation foundation.
//!
//! This module owns Linear installation, catalog/binding administration, and
//! the verified Webhook receipt. Issue mutations still go through the
//! IssueService domain boundary and durable sync queues.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use chrono::{Duration, NaiveDate, Utc};
use hmac::{Hmac, Mac};
use patchbay_db::models::{LinearConnection, LinearProjectBinding};
use patchbay_db::queries::issue as issue_q;
use patchbay_db::queries::linear as linear_q;
use patchbay_db::queries::member as member_q;
use patchbay_middleware::workspace::WorkspaceContext;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use url::Url;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;
use patchbay_service::issue_service::{
    ExternalIssueError, ExternalIssuePatch, ExternalSource, IssueCommand,
};

type HmacSha256 = Hmac<Sha256>;

const LINEAR_AUTH_URL: &str = "https://linear.app/oauth/authorize";
const LINEAR_TOKEN_URL: &str = "https://api.linear.app/oauth/token";
const LINEAR_REVOKE_URL: &str = "https://api.linear.app/oauth/revoke";
const LINEAR_GRAPHQL_URL: &str = "https://api.linear.app/graphql";
const LINEAR_OAUTH_SCOPE: &str = "read,write,issues:create,app:assignable";
const WEBHOOK_MAX_AGE_MS: i128 = 60_000;
const TOKEN_REFRESH_SKEW: Duration = Duration::minutes(5);
const MAX_WEBHOOK_BODY_BYTES: usize = 2 * 1024 * 1024;
const LINEAR_HTTP_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const LINEAR_HTTP_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub fn member_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/workspaces/{id}/linear", get(get_connection))
        .route("/api/workspaces/{id}/linear/catalog", get(get_catalog))
        .route("/api/workspaces/{id}/linear/bindings", get(list_bindings))
        .route(
            "/api/workspaces/{id}/linear/members",
            get(list_member_bindings),
        )
        .route("/api/workspaces/{id}/linear/conflicts", get(list_conflicts))
}

pub fn admin_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/workspaces/{id}/linear/connect", post(start_oauth))
        .route("/api/workspaces/{id}/linear", delete(disconnect))
        .route("/api/workspaces/{id}/linear/bindings", post(create_binding))
        .route(
            "/api/workspaces/{id}/linear/members",
            put(save_member_binding),
        )
        .route(
            "/api/workspaces/{id}/linear/members/{user_id}",
            delete(delete_member_binding),
        )
        .route(
            "/api/workspaces/{id}/linear/conflicts/{conflict_id}",
            patch(resolve_conflict),
        )
        .route("/api/workspaces/{id}/linear/dry-run", post(dry_run_binding))
        .route(
            "/api/workspaces/{id}/linear/bindings/{binding_id}/import",
            post(enqueue_initial_import),
        )
        .route(
            "/api/workspaces/{id}/linear/bindings/{binding_id}",
            patch(update_binding).delete(delete_binding),
        )
}

pub fn public_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/linear/oauth/callback", get(oauth_callback))
        .route("/api/webhooks/linear", post(linear_webhook))
        .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY_BYTES))
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace id"))
}

fn configured_value(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn env_value(name: &str) -> Option<String> {
    configured_value(std::env::var(name).ok().as_deref())
}

fn linear_redirect_uri(state: &HandlerState) -> Option<String> {
    state
        .integrations
        .linear_redirect_uri
        .as_deref()
        .and_then(|value| configured_value(Some(value)))
        .or_else(|| {
            configured_value(Some(&state.public_config.public_url))
                .map(|base| format!("{}/api/linear/oauth/callback", base.trim_end_matches('/')))
        })
}

fn frontend_origin(state: &HandlerState) -> String {
    let raw = configured_value(Some(&state.public_config.daemon_app_url))
        .or_else(|| env_value("FRONTEND_ORIGIN"))
        .unwrap_or_else(|| "http://localhost:3000".to_string());
    let Ok(mut url) = Url::parse(&raw) else {
        return "http://localhost:3000".to_string();
    };
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return "http://localhost:3000".to_string();
    }
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    url.to_string().trim_end_matches('/').to_string()
}

fn linear_callback_redirect(state: &HandlerState, outcome: &str) -> Response {
    Redirect::temporary(&format!(
        "{}/settings?tab=integrations&linear_{}=1",
        frontend_origin(state),
        outcome
    ))
    .into_response()
}

fn build_authorization_url(
    auth_url: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    verifier: &str,
) -> Result<String, url::ParseError> {
    let mut url = Url::parse(auth_url)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("state", state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("scope", LINEAR_OAUTH_SCOPE)
        .append_pair("actor", "app");
    Ok(url.to_string())
}

fn connection_json(connection: LinearConnection) -> Value {
    json!({
        "id": connection.id,
        "workspace_id": connection.workspace_id,
        "organization_id": connection.organization_id,
        "organization_name": connection.organization_name,
        "actor_id": connection.actor_id,
        "scopes": connection.scopes,
        "webhook_id": connection.webhook_id,
        "status": connection.status,
        "token_expires_at": connection.token_expires_at,
        "last_success_at": connection.last_success_at,
        "last_error": connection.last_error,
        "created_at": connection.created_at,
        "updated_at": connection.updated_at,
    })
}

fn integration_disabled(state: &HandlerState) -> Option<Response> {
    (!state.linear_integration_enabled).then(|| {
        error_response(
            StatusCode::NOT_FOUND,
            "Linear integration is not configured",
        )
    })
}

async fn get_connection(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    match linear_q::get_connection_for_workspace(&state.pool, workspace_id).await {
        Ok(Some(connection)) => Json(json!({
            "configured": true,
            "connected": connection.status == "active",
            "pull_import_enabled": state.linear_pull_import_enabled(workspace_id),
            "push_enabled": state.linear_push_enabled(workspace_id),
            "connection": connection_json(connection),
        }))
        .into_response(),
        Ok(None) => Json(json!({
            "configured": true,
            "connected": false,
            "pull_import_enabled": state.linear_pull_import_enabled(workspace_id),
            "push_enabled": state.linear_push_enabled(workspace_id),
            "connection": Value::Null,
        }))
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, "Linear connection lookup failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load Linear connection",
            )
        }
    }
}

fn linear_token_error_response(error: LinearTokenError) -> Response {
    match error {
        LinearTokenError::InvalidGrant | LinearTokenError::ReauthorizationRequired => {
            error_response(
                StatusCode::CONFLICT,
                "Linear authorization requires reauthorization",
            )
        }
        LinearTokenError::NotConfigured => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Linear OAuth is not configured",
        ),
        LinearTokenError::Provider => {
            error_response(StatusCode::BAD_GATEWAY, "Linear provider request failed")
        }
        LinearTokenError::InvalidResponse => error_response(
            StatusCode::BAD_GATEWAY,
            "Linear provider returned an invalid response",
        ),
        LinearTokenError::MutationRejected(message) => {
            tracing::warn!(%message, "Linear mutation was rejected");
            error_response(
                StatusCode::BAD_REQUEST,
                "Linear rejected the requested mutation",
            )
        }
        LinearTokenError::RateLimited => error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "Linear rate limit reached; retry later",
        ),
        LinearTokenError::Storage(error) => {
            tracing::warn!(%error, "Linear storage operation failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Linear integration storage failed",
            )
        }
        LinearTokenError::Secret(error) => {
            tracing::warn!(%error, "Linear secret storage operation failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Linear integration secret storage failed",
            )
        }
    }
}

async fn get_catalog(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let connection = match linear_q::get_connection_for_workspace(&state.pool, workspace_id).await {
        Ok(Some(connection)) => connection,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "Linear is not connected"),
        Err(error) => {
            tracing::warn!(%error, "Linear catalog connection lookup failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load Linear connection",
            );
        }
    };
    if connection.status == "reauthorization_required" {
        return error_response(
            StatusCode::CONFLICT,
            "Linear authorization requires reauthorization",
        );
    }
    if connection.status == "revoked" {
        return error_response(StatusCode::NOT_FOUND, "Linear is not connected");
    }
    let manager = match LinearTokenManager::from_state(&state) {
        Ok(manager) => manager,
        Err(error) => return linear_token_error_response(error),
    };
    match manager.catalog(connection.id).await {
        Ok(catalog) => Json(catalog).into_response(),
        Err(error) => linear_token_error_response(error),
    }
}

async fn dry_run_binding(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<SaveLinearProjectBindingRequest>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let request = match validate_binding_request(request) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let connection = match connection_for_binding(&state, workspace_id, request.connection_id).await
    {
        Ok(connection) => connection,
        Err(response) => return response,
    };
    if let Err(response) = validate_remote_binding(&state, &connection, &request).await {
        return response;
    }
    match linear_q::project_belongs_to_workspace(
        &state.pool,
        workspace_id,
        request.patchbay_project_id,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => return error_response(StatusCode::NOT_FOUND, "Patchbay project not found"),
        Err(error) => {
            tracing::warn!(%error, "Patchbay project lookup for Linear dry run failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load Patchbay project",
            );
        }
    }
    let local_issue_count = match linear_q::count_issues_in_project(
        &state.pool,
        workspace_id,
        request.patchbay_project_id,
    )
    .await
    {
        Ok(count) => count,
        Err(error) => {
            tracing::warn!(%error, "Patchbay issue count for Linear dry run failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to count Patchbay issues",
            );
        }
    };

    let remote_counts = if request.sync_mode == "not_synced" {
        RemoteDryRunCounts {
            issue_count: 0,
            unmapped_status_count: 0,
            truncated: false,
        }
    } else {
        let manager = match LinearTokenManager::from_state(&state) {
            Ok(manager) => manager,
            Err(error) => return linear_token_error_response(error),
        };
        match manager
            .dry_run_counts(
                request.connection_id,
                request.linear_project_id.trim(),
                &request.status_mapping,
            )
            .await
        {
            Ok(counts) => counts,
            Err(error) => return linear_token_error_response(error),
        }
    };

    let candidate_import_count = if request.sync_mode == "import"
        || (request.sync_mode == "two_way"
            && request.initial_source_of_truth.as_deref() == Some("linear"))
    {
        remote_counts.issue_count
    } else {
        0
    };
    let candidate_publish_count = if request.sync_mode == "publish"
        || (request.sync_mode == "two_way"
            && request.initial_source_of_truth.as_deref() == Some("patchbay"))
    {
        local_issue_count
    } else {
        0
    };
    Json(LinearDryRunResponse {
        patchbay_project_id: request.patchbay_project_id,
        linear_project_id: request.linear_project_id.trim().to_string(),
        sync_mode: request.sync_mode,
        initial_source_of_truth: request.initial_source_of_truth,
        local_issue_count,
        remote_issue_count: remote_counts.issue_count,
        remote_issue_count_truncated: remote_counts.truncated,
        candidate_import_count,
        candidate_publish_count,
        unmapped_remote_status_count: remote_counts.unmapped_status_count,
        exact_link_counts_available: false,
    })
    .into_response()
}

async fn list_bindings(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    match linear_q::list_project_bindings(&state.pool, workspace_id).await {
        Ok(bindings) => Json(json!({ "bindings": bindings })).into_response(),
        Err(error) => {
            tracing::warn!(%error, "Linear project binding lookup failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load Linear project bindings",
            )
        }
    }
}

async fn list_member_bindings(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let Some(connection) =
        (match linear_q::get_connection_for_workspace(&state.pool, workspace_id).await {
            Ok(connection) => connection,
            Err(error) => {
                tracing::warn!(%error, "Linear member binding connection lookup failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load Linear connection",
                );
            }
        })
    else {
        return error_response(StatusCode::NOT_FOUND, "Linear is not connected");
    };
    match linear_q::list_linear_member_bindings(&state.pool, workspace_id, connection.id).await {
        Ok(bindings) => Json(json!({ "bindings": bindings })).into_response(),
        Err(error) => {
            tracing::warn!(%error, "Linear member binding lookup failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load Linear member mappings",
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct SaveLinearMemberBindingRequest {
    connection_id: Uuid,
    patchbay_user_id: Uuid,
    linear_user_id: String,
}

#[derive(Debug, Deserialize)]
struct LinearMemberBindingPath {
    id: Uuid,
    user_id: Uuid,
}

async fn save_member_binding(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<SaveLinearMemberBindingRequest>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let linear_user_id = request.linear_user_id.trim();
    if request.patchbay_user_id.is_nil() || linear_user_id.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Patchbay and Linear member ids are required",
        );
    }
    if let Err(response) = connection_for_binding(&state, workspace_id, request.connection_id).await
    {
        return response;
    }
    if member_q::get_member_by_user_and_workspace(
        &state.pool,
        request.patchbay_user_id,
        workspace_id,
    )
    .await
    .ok()
    .flatten()
    .is_none()
    {
        return error_response(StatusCode::NOT_FOUND, "Patchbay member not found");
    }
    match linear_q::upsert_linear_member_binding(
        &state.pool,
        Uuid::now_v7(),
        workspace_id,
        request.connection_id,
        request.patchbay_user_id,
        linear_user_id,
    )
    .await
    {
        Ok(binding) => (StatusCode::OK, Json(binding)).into_response(),
        Err(error) => {
            tracing::warn!(%error, "Linear member binding save failed");
            error_response(
                StatusCode::CONFLICT,
                "Linear member is already mapped to another Patchbay member",
            )
        }
    }
}

async fn delete_member_binding(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(path): Path<LinearMemberBindingPath>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    if path.id != workspace_id || path.user_id.is_nil() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid Linear member mapping path",
        );
    }
    let Some(connection) =
        (match linear_q::get_connection_for_workspace(&state.pool, workspace_id).await {
            Ok(connection) => connection,
            Err(error) => {
                tracing::warn!(%error, "Linear member binding connection lookup failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load Linear connection",
                );
            }
        })
    else {
        return error_response(StatusCode::NOT_FOUND, "Linear is not connected");
    };
    match linear_q::delete_linear_member_binding(
        &state.pool,
        workspace_id,
        connection.id,
        path.user_id,
    )
    .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error_response(StatusCode::NOT_FOUND, "Linear member mapping not found"),
        Err(error) => {
            tracing::warn!(%error, "Linear member binding delete failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete Linear member mapping",
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct LinearConflictQuery {
    status: Option<String>,
}

async fn list_conflicts(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(query): Query<LinearConflictQuery>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let status = query
        .status
        .as_deref()
        .filter(|value| matches!(*value, "open" | "resolved" | "dismissed"));
    if query.status.is_some() && status.is_none() {
        return error_response(StatusCode::BAD_REQUEST, "invalid Linear conflict status");
    }
    match linear_q::list_linear_sync_conflicts(&state.pool, workspace_id, status).await {
        Ok(conflicts) => Json(json!({ "conflicts": conflicts })).into_response(),
        Err(error) => {
            tracing::warn!(%error, "Linear conflict lookup failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load Linear conflicts",
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct ResolveLinearConflictRequest {
    resolution: String,
    manual_value: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct LinearConflictPath {
    id: Uuid,
    conflict_id: Uuid,
}

fn conflict_patch(field: &str, value: &Value) -> Result<ExternalIssuePatch, &'static str> {
    match field {
        "title" => Ok(ExternalIssuePatch {
            title: Some(
                value
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or("title resolution must be a non-empty string")?
                    .to_string(),
            ),
            ..ExternalIssuePatch::default()
        }),
        "description" => {
            let description = match value {
                Value::String(value) => Some(Some(value.clone())),
                Value::Null => Some(None),
                _ => return Err("description resolution must be a string or null"),
            };
            Ok(ExternalIssuePatch {
                description,
                ..ExternalIssuePatch::default()
            })
        }
        "priority" => Ok(ExternalIssuePatch {
            priority: Some(
                value
                    .as_str()
                    .filter(|value| matches!(*value, "none" | "urgent" | "high" | "medium" | "low"))
                    .ok_or("priority resolution is invalid")?
                    .to_string(),
            ),
            ..ExternalIssuePatch::default()
        }),
        "status" => Ok(ExternalIssuePatch {
            status: Some(
                value
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or("status resolution must be a non-empty string")?
                    .to_string(),
            ),
            ..ExternalIssuePatch::default()
        }),
        "due_date" => Ok(ExternalIssuePatch {
            due_date: Some(
                value
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| {
                        NaiveDate::parse_from_str(value, "%Y-%m-%d")
                            .map_err(|_| "due date resolution is invalid")
                    })
                    .transpose()?,
            ),
            ..ExternalIssuePatch::default()
        }),
        "owner_id" => {
            let owner_id = value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(|value| {
                    value
                        .parse::<Uuid>()
                        .map_err(|_| "owner resolution is invalid")
                })
                .transpose()?;
            Ok(ExternalIssuePatch {
                owner_type: Some(owner_id.map(|_| "member".to_string())),
                owner_id: Some(owner_id),
                ..ExternalIssuePatch::default()
            })
        }
        _ => Err("this Linear field cannot be resolved here"),
    }
}

fn external_conflict_error_status(error: &ExternalIssueError) -> StatusCode {
    match error {
        ExternalIssueError::RevisionConflict { .. } => StatusCode::CONFLICT,
        ExternalIssueError::NotFound => StatusCode::NOT_FOUND,
        ExternalIssueError::InvalidStatus
        | ExternalIssueError::InvalidPriority
        | ExternalIssueError::InvalidOwner
        | ExternalIssueError::ActiveExecutorRequired
        | ExternalIssueError::ReviewReviewerRequired
        | ExternalIssueError::Internal(_)
        | ExternalIssueError::MissingSourceEvent
        | ExternalIssueError::ExternalOutboxNotSuppressed => StatusCode::BAD_REQUEST,
        ExternalIssueError::Sql(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn resolve_conflict(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(path): Path<LinearConflictPath>,
    Json(request): Json<ResolveLinearConflictRequest>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    if path.id != workspace_id {
        return error_response(
            StatusCode::BAD_REQUEST,
            "workspace id does not match context",
        );
    }
    if !matches!(request.resolution.as_str(), "local" | "remote" | "manual") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "resolution must be local, remote, or manual",
        );
    }
    let Some(conflict) = (match linear_q::get_linear_sync_conflict(
        &state.pool,
        workspace_id,
        path.conflict_id,
    )
    .await
    {
        Ok(conflict) => conflict,
        Err(error) => {
            tracing::warn!(%error, "Linear conflict lookup failed before resolution");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load Linear conflict",
            );
        }
    }) else {
        return error_response(StatusCode::NOT_FOUND, "Linear conflict not found");
    };
    if conflict.status != "open" {
        return error_response(StatusCode::CONFLICT, "Linear conflict is already resolved");
    }
    let selected_value = match request.resolution.as_str() {
        "local" => conflict.local_value.clone(),
        "remote" => conflict.remote_value.clone(),
        "manual" => match request.manual_value {
            Some(value) => value,
            None => return error_response(StatusCode::BAD_REQUEST, "manual_value is required"),
        },
        _ => unreachable!(),
    };
    let patch = match conflict_patch(&conflict.field, &selected_value) {
        Ok(patch) => patch,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };
    let issue = match patchbay_db::queries::issue::get_issue_in_workspace(
        &state.pool,
        conflict.patchbay_issue_id,
        workspace_id,
    )
    .await
    {
        Ok(Some(issue)) => issue,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "Patchbay Issue not found"),
        Err(error) => {
            tracing::warn!(%error, "Linear conflict Issue lookup failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load Issue");
        }
    };
    let updated_issue = match state
        .issues
        .apply_external_patch(
            workspace_id,
            issue.id,
            IssueCommand::ApplyExternalPatch {
                source: ExternalSource::Linear,
                source_event_id: format!("linear-conflict:{}", conflict.id),
                expected_revision: Some(issue.revision),
                suppress_external_outbox: true,
                patch: patch.clone(),
            },
        )
        .await
    {
        Ok(issue) => issue,
        Err(error) => {
            tracing::warn!(%error, conflict_id = %conflict.id, "Linear conflict Issue resolution failed");
            return error_response(external_conflict_error_status(&error), &error.to_string());
        }
    };

    let Some(link) =
        (match linear_q::get_linear_issue_link(&state.pool, workspace_id, conflict.link_id).await {
            Ok(link) => link,
            Err(error) => {
                tracing::warn!(%error, "Linear conflict link lookup failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load Linear link",
                );
            }
        })
    else {
        return error_response(StatusCode::NOT_FOUND, "Linear Issue Link not found");
    };
    let open_conflicts = match linear_q::count_open_linear_sync_conflicts_for_link(
        &state.pool,
        workspace_id,
        link.id,
    )
    .await
    {
        Ok(count) => count,
        Err(error) => {
            tracing::warn!(%error, "Linear conflict count failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update Linear conflict",
            );
        }
    };
    let resolved = match linear_q::resolve_linear_sync_conflict(
        &state.pool,
        workspace_id,
        conflict.id,
        &request.resolution,
        &selected_value,
        context.member.user_id,
    )
    .await
    {
        Ok(Some(conflict)) => conflict,
        Ok(None) => {
            return error_response(StatusCode::CONFLICT, "Linear conflict is already resolved")
        }
        Err(error) => {
            tracing::warn!(%error, "Linear conflict resolution persistence failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to save Linear conflict resolution",
            );
        }
    };
    let mut common_snapshot = link.last_common_snapshot;
    if request.resolution == "remote" {
        if let Some(object) = common_snapshot.as_object_mut() {
            object.insert(conflict.field.clone(), selected_value);
        }
    }
    let link_status = if open_conflicts == 1 {
        "active"
    } else {
        "conflict"
    };
    if let Err(error) = linear_q::update_linear_issue_link(
        &state.pool,
        link.id,
        workspace_id,
        &common_snapshot,
        link.remote_updated_at,
        link.last_remote_event_at_ms,
        link.last_remote_event_id.as_deref(),
        link_status,
    )
    .await
    {
        tracing::warn!(%error, conflict_id = %resolved.id, "Linear conflict link update failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update Linear link",
        );
    }
    if matches!(request.resolution.as_str(), "local" | "manual") {
        if let Err(error) = linear_q::enqueue_issue_outbox(
            &state.pool,
            workspace_id,
            updated_issue.project_id,
            updated_issue.id,
            &format!("conflict:{}:{}", conflict.id, request.resolution),
            "issue_updated",
            &patchbay_service::issue_service::linear_issue_sync_payload(&updated_issue),
        )
        .await
        {
            tracing::warn!(%error, conflict_id = %conflict.id, "Linear conflict outbound resolution enqueue failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to queue Linear conflict resolution",
            );
        }
    }
    state.notify_linear_sync();
    Json(resolved).into_response()
}

#[derive(Debug, Deserialize)]
struct SaveLinearProjectBindingRequest {
    connection_id: Uuid,
    patchbay_project_id: Uuid,
    linear_project_id: String,
    linear_team_id: Option<String>,
    status: Option<String>,
    sync_mode: String,
    initial_source_of_truth: Option<String>,
    #[serde(default = "empty_json_object")]
    status_mapping: Value,
    #[serde(default = "empty_json_object")]
    agent_label_mapping: Value,
}

fn empty_json_object() -> Value {
    json!({})
}

#[derive(Debug, Deserialize)]
struct LinearBindingPath {
    id: Uuid,
    binding_id: Uuid,
}

fn binding_status(status: Option<&str>) -> Result<&str, Response> {
    let status = status.unwrap_or("draft");
    if matches!(status, "draft" | "active" | "paused" | "tombstone") {
        Ok(status)
    } else {
        Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid Linear binding status",
        ))
    }
}

fn binding_sync_mode(mode: &str) -> Result<&str, Response> {
    if matches!(mode, "import" | "publish" | "two_way" | "not_synced") {
        Ok(mode)
    } else {
        Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid Linear sync mode",
        ))
    }
}

fn binding_mapping(value: Value, message: &'static str) -> Result<Value, Response> {
    if value.is_object() {
        Ok(value)
    } else {
        Err(error_response(StatusCode::BAD_REQUEST, message))
    }
}

fn validate_binding_request(
    request: SaveLinearProjectBindingRequest,
) -> Result<SaveLinearProjectBindingRequest, Response> {
    let status = binding_status(request.status.as_deref())?;
    let sync_mode = binding_sync_mode(&request.sync_mode)?;
    if request.linear_project_id.trim().is_empty() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "Linear project id is required",
        ));
    }
    let linear_team_id = request
        .linear_team_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if matches!(status, "active" | "paused") && linear_team_id.is_none() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "Linear team id is required before activation",
        ));
    }
    if status == "active" && sync_mode == "not_synced" {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "a not-synced binding cannot be active",
        ));
    }
    if sync_mode == "import" && request.initial_source_of_truth.as_deref() != Some("linear") {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "import bindings require Linear as the initial source",
        ));
    }
    if sync_mode == "publish" && request.initial_source_of_truth.as_deref() != Some("patchbay") {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "publish bindings require Patchbay as the initial source",
        ));
    }
    if sync_mode == "two_way"
        && !matches!(
            request.initial_source_of_truth.as_deref(),
            Some("linear") | Some("patchbay")
        )
    {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "two-way bindings require an initial source",
        ));
    }
    if !matches!(sync_mode, "two_way" | "import" | "publish")
        && request.initial_source_of_truth.is_some()
    {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "not-synced bindings cannot select an initial source",
        ));
    }
    let status_mapping =
        binding_mapping(request.status_mapping, "status mapping must be an object")?;
    let agent_label_mapping = binding_mapping(
        request.agent_label_mapping,
        "agent label mapping must be an object",
    )?;
    Ok(SaveLinearProjectBindingRequest {
        linear_team_id: linear_team_id.map(str::to_string),
        status: Some(status.to_string()),
        sync_mode: sync_mode.to_string(),
        status_mapping,
        agent_label_mapping,
        ..request
    })
}

async fn connection_for_binding(
    state: &HandlerState,
    workspace_id: Uuid,
    connection_id: Uuid,
) -> Result<LinearConnection, Response> {
    match linear_q::get_connection_by_id(&state.pool, workspace_id, connection_id).await {
        Ok(Some(connection)) if connection.status == "active" => Ok(connection),
        Ok(Some(connection)) if connection.status == "reauthorization_required" => {
            Err(error_response(
                StatusCode::CONFLICT,
                "Linear authorization requires reauthorization",
            ))
        }
        Ok(Some(_)) => Err(error_response(
            StatusCode::NOT_FOUND,
            "Linear is not connected",
        )),
        Ok(None) => Err(error_response(
            StatusCode::NOT_FOUND,
            "Linear connection not found",
        )),
        Err(error) => {
            tracing::warn!(%error, "Linear binding connection lookup failed");
            Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load Linear connection",
            ))
        }
    }
}

async fn validate_remote_binding(
    state: &HandlerState,
    connection: &LinearConnection,
    request: &SaveLinearProjectBindingRequest,
) -> Result<(), Response> {
    if request.status.as_deref() != Some("active") {
        return Ok(());
    }
    let Some(linear_team_id) = request.linear_team_id.as_deref() else {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "Linear team id is required before activation",
        ));
    };
    let manager = LinearTokenManager::from_state(state).map_err(linear_token_error_response)?;
    match manager
        .remote_binding_is_valid(
            connection.id,
            &connection.organization_id,
            request.linear_project_id.trim(),
            linear_team_id,
        )
        .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(error_response(
            StatusCode::BAD_REQUEST,
            "Linear project and team do not belong to the connected organization",
        )),
        Err(error) => Err(linear_token_error_response(error)),
    }
}

fn is_linear_binding_unique_conflict(error: &anyhow::Error) -> bool {
    error.downcast_ref::<sqlx::Error>().is_some_and(|error| {
        matches!(
            error,
            sqlx::Error::Database(database)
                if database.code().as_deref() == Some("23505")
                    && matches!(
                        database.constraint(),
                        Some("uq_linear_project_binding_remote" | "uq_linear_project_binding_local")
                    )
        )
    })
}

fn binding_is_publishable(binding: &LinearProjectBinding) -> bool {
    binding.status == "active"
        && binding.linear_team_id.is_some()
        && (binding.sync_mode == "publish"
            || (binding.sync_mode == "two_way"
                && binding.initial_source_of_truth.as_deref() == Some("patchbay")))
}

fn binding_needs_outbox_seed(
    previous: Option<&LinearProjectBinding>,
    next: &LinearProjectBinding,
) -> bool {
    if !binding_is_publishable(next) {
        return false;
    }
    let Some(previous) = previous else {
        return true;
    };
    !binding_is_publishable(previous)
        || (next.sync_mode == "two_way"
            && next.initial_source_of_truth.as_deref() == Some("patchbay")
            && previous.initial_source_of_truth.as_deref() != Some("patchbay"))
}

async fn seed_binding_outbox(
    executor: &mut sqlx::PgConnection,
    binding: &LinearProjectBinding,
) -> anyhow::Result<u64> {
    if !binding_is_publishable(binding) {
        return Ok(0);
    }
    let issues = issue_q::list_issues_in_project(
        &mut *executor,
        binding.workspace_id,
        binding.patchbay_project_id,
    )
    .await?;
    let mut inserted = 0;
    for issue in issues {
        let event_key = format!("issue:{}:revision:{}", issue.id, issue.revision);
        if linear_q::enqueue_issue_outbox_for_binding(
            &mut *executor,
            binding.workspace_id,
            binding.id,
            issue.id,
            &event_key,
            "issue_updated",
            &patchbay_service::issue_service::linear_issue_sync_payload(&issue),
        )
        .await?
        .is_some()
        {
            inserted += 1;
        }
    }
    Ok(inserted)
}

fn binding_is_publishable(binding: &LinearProjectBinding) -> bool {
    binding.status == "active"
        && binding.linear_team_id.is_some()
        && (binding.sync_mode == "publish"
            || (binding.sync_mode == "two_way"
                && binding.initial_source_of_truth.as_deref() == Some("patchbay")))
}

fn binding_needs_outbox_seed(
    previous: Option<&LinearProjectBinding>,
    next: &LinearProjectBinding,
) -> bool {
    if !binding_is_publishable(next) {
        return false;
    }
    let Some(previous) = previous else {
        return true;
    };
    !binding_is_publishable(previous)
        || (next.sync_mode == "two_way"
            && next.initial_source_of_truth.as_deref() == Some("patchbay")
            && previous.initial_source_of_truth.as_deref() != Some("patchbay"))
}

async fn seed_binding_outbox(
    executor: &mut sqlx::PgConnection,
    binding: &LinearProjectBinding,
) -> anyhow::Result<u64> {
    if !binding_is_publishable(binding) {
        return Ok(0);
    }
    let issues = issue_q::list_issues_in_project(
        &mut *executor,
        binding.workspace_id,
        binding.patchbay_project_id,
    )
    .await?;
    let mut inserted = 0;
    for issue in issues {
        let event_key = format!("issue:{}:revision:{}", issue.id, issue.revision);
        if linear_q::enqueue_issue_outbox_for_binding(
            &mut *executor,
            binding.workspace_id,
            binding.id,
            issue.id,
            &event_key,
            "issue_updated",
            &patchbay_service::issue_service::linear_issue_sync_payload(&issue),
        )
        .await?
        .is_some()
        {
            inserted += 1;
        }
    }
    Ok(inserted)
}

async fn create_binding(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<SaveLinearProjectBindingRequest>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let request = match validate_binding_request(request) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let connection = match connection_for_binding(&state, workspace_id, request.connection_id).await
    {
        Ok(connection) => connection,
        Err(response) => return response,
    };
    if let Err(response) = validate_remote_binding(&state, &connection, &request).await {
        return response;
    }
    match linear_q::project_belongs_to_workspace(
        &state.pool,
        workspace_id,
        request.patchbay_project_id,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => return error_response(StatusCode::NOT_FOUND, "Patchbay project not found"),
        Err(error) => {
            tracing::warn!(%error, "Patchbay project lookup for Linear binding failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load Patchbay project",
            );
        }
    }
    let input = linear_q::LinearProjectBindingInput {
        id: Uuid::now_v7(),
        workspace_id,
        connection_id: request.connection_id,
        patchbay_project_id: request.patchbay_project_id,
        linear_project_id: request.linear_project_id.trim(),
        linear_team_id: request.linear_team_id.as_deref(),
        status: request.status.as_deref().unwrap_or("draft"),
        sync_mode: &request.sync_mode,
        initial_source_of_truth: request.initial_source_of_truth.as_deref(),
        status_mapping: &request.status_mapping,
        agent_label_mapping: &request.agent_label_mapping,
        created_by_id: context.member.user_id,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "Linear project binding transaction failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create Linear project binding",
            );
        }
    };
    match linear_q::create_project_binding(&mut *transaction, &input).await {
        Ok(binding) => {
            if binding_needs_outbox_seed(None, &binding) {
                if let Err(error) = seed_binding_outbox(&mut *transaction, &binding).await {
                    tracing::warn!(%error, binding_id = %binding.id, "Linear binding Outbox seed failed");
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to seed Linear publication events",
                    );
                }
            }
            if let Err(error) = transaction.commit().await {
                tracing::warn!(%error, binding_id = %binding.id, "Linear project binding commit failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create Linear project binding",
                );
            }
            (StatusCode::CREATED, Json(binding)).into_response()
        }
        Err(error) => {
            tracing::warn!(%error, "Linear project binding creation failed");
            if is_linear_binding_unique_conflict(&error) {
                error_response(
                    StatusCode::CONFLICT,
                    "Linear project binding already exists",
                )
            } else {
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create Linear project binding",
                )
            }
        }
    }
}

async fn update_binding(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(path): Path<LinearBindingPath>,
    Json(request): Json<SaveLinearProjectBindingRequest>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    if path.id != workspace_id {
        return error_response(
            StatusCode::BAD_REQUEST,
            "workspace id does not match context",
        );
    }
    let request = match validate_binding_request(request) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(existing) =
        (match linear_q::get_project_binding(&state.pool, workspace_id, path.binding_id).await {
            Ok(binding) => binding,
            Err(error) => {
                tracing::warn!(%error, "Linear project binding lookup failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load Linear project binding",
                );
            }
        })
    else {
        return error_response(StatusCode::NOT_FOUND, "Linear project binding not found");
    };
    if request.connection_id != existing.connection_id
        || request.patchbay_project_id != existing.patchbay_project_id
        || request.linear_project_id.trim() != existing.linear_project_id
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Linear binding connection, Patchbay project, and Linear project are immutable",
        );
    }
    let connection = match connection_for_binding(&state, workspace_id, request.connection_id).await
    {
        Ok(connection) => connection,
        Err(response) => return response,
    };
    if let Err(response) = validate_remote_binding(&state, &connection, &request).await {
        return response;
    }
    let input = linear_q::LinearProjectBindingInput {
        id: path.binding_id,
        workspace_id,
        connection_id: request.connection_id,
        patchbay_project_id: request.patchbay_project_id,
        linear_project_id: request.linear_project_id.trim(),
        linear_team_id: request.linear_team_id.as_deref(),
        status: request.status.as_deref().unwrap_or("draft"),
        sync_mode: &request.sync_mode,
        initial_source_of_truth: request.initial_source_of_truth.as_deref(),
        status_mapping: &request.status_mapping,
        agent_label_mapping: &request.agent_label_mapping,
        created_by_id: existing.created_by_id,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "Linear project binding transaction failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update Linear project binding",
            );
        }
    };
    match linear_q::update_project_binding(&mut *transaction, &input).await {
        Ok(Some(binding)) => {
            if binding_needs_outbox_seed(Some(&existing), &binding) {
                if let Err(error) = seed_binding_outbox(&mut *transaction, &binding).await {
                    tracing::warn!(%error, binding_id = %binding.id, "Linear binding Outbox seed failed");
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to seed Linear publication events",
                    );
                }
            }
            if let Err(error) = transaction.commit().await {
                tracing::warn!(%error, binding_id = %binding.id, "Linear project binding commit failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to update Linear project binding",
                );
            }
            Json(binding).into_response()
        }
        Ok(None) => error_response(StatusCode::NOT_FOUND, "Linear project binding not found"),
        Err(error) => {
            tracing::warn!(%error, "Linear project binding update failed");
            if is_linear_binding_unique_conflict(&error) {
                error_response(
                    StatusCode::CONFLICT,
                    "Linear project binding already exists",
                )
            } else {
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to update Linear project binding",
                )
            }
        }
    }
}

async fn delete_binding(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(path): Path<LinearBindingPath>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    if path.id != workspace_id {
        return error_response(
            StatusCode::BAD_REQUEST,
            "workspace id does not match context",
        );
    }
    match linear_q::tombstone_project_binding(&state.pool, workspace_id, path.binding_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error_response(StatusCode::NOT_FOUND, "Linear project binding not found"),
        Err(error) => {
            tracing::warn!(%error, "Linear project binding tombstone failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to remove Linear project binding",
            )
        }
    }
}

async fn enqueue_initial_import(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(path): Path<LinearBindingPath>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    if path.id != workspace_id {
        return error_response(
            StatusCode::BAD_REQUEST,
            "workspace id does not match context",
        );
    }
    if !state.linear_pull_import_enabled(workspace_id) {
        return error_response(StatusCode::NOT_FOUND, "Linear pull/import is not enabled");
    }
    let Some(binding) =
        (match linear_q::get_project_binding(&state.pool, workspace_id, path.binding_id).await {
            Ok(binding) => binding,
            Err(error) => {
                tracing::warn!(%error, "Linear initial import binding lookup failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load Linear project binding",
                );
            }
        })
    else {
        return error_response(StatusCode::NOT_FOUND, "Linear project binding not found");
    };
    if binding.status != "active" || !matches!(binding.sync_mode.as_str(), "import" | "two_way") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Linear binding is not configured for import",
        );
    }
    if let Err(response) = connection_for_binding(&state, workspace_id, binding.connection_id).await
    {
        return response;
    }
    let job_id = Uuid::now_v7();
    let delivery_id = format!("linear-initial-import-{job_id}");
    let source_event_id = format!("linear-import:{job_id}");
    let payload = json!({
        "kind": "initial_import",
        "binding_id": binding.id,
        "source_event_id": source_event_id,
    });
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "Linear initial import transaction failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Linear import queue is unavailable",
            );
        }
    };
    let inserted = match linear_q::insert_sync_inbox(
        &mut *transaction,
        job_id,
        binding.connection_id,
        &delivery_id,
        "linear.initial_import",
        &payload,
    )
    .await
    {
        Ok(inserted) => inserted,
        Err(error) => {
            tracing::warn!(%error, "Linear initial import enqueue failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Linear import queue is unavailable",
            );
        }
    };
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, "Linear initial import commit failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Linear import queue is unavailable",
        );
    }
    state.notify_linear_sync();
    (
        StatusCode::ACCEPTED,
        Json(json!({ "queued": inserted, "inbox_id": job_id })),
    )
        .into_response()
}

async fn start_oauth(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let Some(client_id) = state
        .integrations
        .linear_client_id
        .as_deref()
        .and_then(|value| configured_value(Some(value)))
    else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Linear OAuth is not configured",
        );
    };
    if state
        .integrations
        .linear_client_secret
        .as_deref()
        .and_then(|value| configured_value(Some(value)))
        .is_none()
    {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Linear OAuth is not configured",
        );
    }
    let Some(redirect_uri) = linear_redirect_uri(&state) else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Linear OAuth redirect URI is not configured",
        );
    };
    if state.linear_secret_box.is_none() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Linear encrypted secret storage is not configured",
        );
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    match linear_q::get_connection_for_workspace(&state.pool, workspace_id).await {
        Ok(Some(connection)) if connection.status == "active" => {
            return error_response(
                StatusCode::CONFLICT,
                "Linear is already connected; disconnect before reconnecting",
            );
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, "Linear connection lookup before OAuth failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to prepare Linear authorization",
            );
        }
    }
    if let Err(error) = linear_q::cleanup_oauth_states(&state.pool, 100).await {
        tracing::warn!(%error, "Linear OAuth state cleanup failed");
    }

    let state_token = random_token(32);
    let verifier = random_token(48);
    let verifier_encrypted = match seal(&state, &verifier) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "Linear OAuth verifier encryption failed");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Linear encrypted secret storage is unavailable",
            );
        }
    };
    let expires_at = Utc::now() + Duration::minutes(10);
    let state_hash = sha256_hex(&state_token);
    if let Err(error) = linear_q::insert_oauth_state(
        &state.pool,
        &linear_q::OAuthStateInput {
            id: Uuid::now_v7(),
            state_hash: &state_hash,
            workspace_id,
            user_id: context.member.user_id,
            code_verifier_encrypted: &verifier_encrypted,
            redirect_uri: &redirect_uri,
            expires_at,
        },
    )
    .await
    {
        tracing::warn!(%error, "Linear OAuth state persistence failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to prepare Linear authorization",
        );
    }

    let auth_url = state
        .integrations
        .linear_auth_url
        .as_deref()
        .and_then(|value| configured_value(Some(value)))
        .unwrap_or_else(|| LINEAR_AUTH_URL.to_string());
    let authorization_url = match build_authorization_url(
        &auth_url,
        &client_id,
        &redirect_uri,
        &state_token,
        &verifier,
    ) {
        Ok(url) => url,
        Err(error) => {
            tracing::warn!(%error, "Linear OAuth authorization URL is invalid");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Linear OAuth authorization URL is invalid",
            );
        }
    };
    Json(json!({
        "authorization_url": authorization_url,
        "state_expires_at": expires_at,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct OAuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// The OAuth token response intentionally contains only documented token
/// fields. Installation identity comes from the authenticated GraphQL API,
/// never from undocumented token-response extensions.
#[derive(Debug, Deserialize)]
struct LinearTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    scope: Option<LinearScopeResponse>,
}

#[derive(Debug, Deserialize)]
struct GraphQlResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdentityData {
    viewer: IdentityViewer,
    organization: Option<IdentityOrganization>,
}

#[derive(Debug, Deserialize)]
struct IdentityViewer {
    id: String,
}

#[derive(Debug, Deserialize)]
struct IdentityOrganization {
    id: String,
    name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct LinearCatalogPage<T> {
    nodes: Vec<T>,
    #[serde(rename = "pageInfo")]
    page_info: LinearCatalogPageInfo,
}

#[derive(Debug, Deserialize)]
struct LinearCatalogPageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LinearCatalogData {
    teams: LinearCatalogPage<LinearCatalogTeam>,
    projects: LinearCatalogPage<LinearCatalogProject>,
    #[serde(rename = "workflowStates")]
    workflow_states: LinearCatalogPage<LinearCatalogState>,
    users: LinearCatalogPage<LinearCatalogUser>,
    #[serde(rename = "issueLabels")]
    issue_labels: LinearCatalogPage<LinearCatalogLabel>,
}

#[derive(Debug, Deserialize)]
struct LinearCatalogPageResponse {
    teams: Option<LinearCatalogPage<LinearCatalogTeam>>,
    projects: Option<LinearCatalogPage<LinearCatalogProject>>,
    #[serde(rename = "workflowStates")]
    workflow_states: Option<LinearCatalogPage<LinearCatalogState>>,
    users: Option<LinearCatalogPage<LinearCatalogUser>>,
    #[serde(rename = "issueLabels")]
    issue_labels: Option<LinearCatalogPage<LinearCatalogLabel>>,
}

#[derive(Debug, Deserialize)]
struct LinearBindingValidationData {
    project: Option<LinearBindingValidationProject>,
    team: Option<LinearBindingValidationTeam>,
}

#[derive(Debug, Deserialize)]
struct LinearBindingValidationProject {
    id: String,
    teams: LinearBindingValidationTeamPage,
}

#[derive(Debug, Deserialize)]
struct LinearBindingValidationTeamPage {
    nodes: Vec<LinearBindingValidationTeamId>,
}

#[derive(Debug, Deserialize)]
struct LinearBindingValidationTeamId {
    id: String,
}

#[derive(Debug, Deserialize)]
struct LinearBindingValidationTeam {
    id: String,
    organization: LinearBindingValidationOrganization,
}

#[derive(Debug, Deserialize)]
struct LinearBindingValidationOrganization {
    id: String,
}

#[derive(Debug, Default)]
struct CatalogCursor {
    after: Option<String>,
    done: bool,
}

#[derive(Debug, Default)]
struct CatalogCursors {
    teams: CatalogCursor,
    projects: CatalogCursor,
    workflow_states: CatalogCursor,
    users: CatalogCursor,
    issue_labels: CatalogCursor,
}

#[derive(Debug, Deserialize, Serialize)]
struct LinearCatalogTeam {
    id: String,
    key: String,
    name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct LinearCatalogProject {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct LinearCatalogState {
    id: String,
    name: String,
    #[serde(rename = "type")]
    state_type: String,
    color: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct LinearCatalogUser {
    id: String,
    name: String,
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LinearCatalogLabel {
    id: String,
    name: String,
    color: String,
    #[serde(rename = "isGroup")]
    is_group: bool,
    parent: Option<LinearCatalogLabelParent>,
    team: Option<LinearCatalogLabelTeam>,
}

#[derive(Debug, Deserialize)]
struct LinearCatalogLabelParent {
    id: String,
}

#[derive(Debug, Deserialize)]
struct LinearCatalogLabelTeam {
    id: String,
}

#[derive(Debug, Serialize)]
struct LinearCatalogLabelResponse {
    id: String,
    name: String,
    color: String,
    is_group: bool,
    parent_id: Option<String>,
    team_id: Option<String>,
}

impl From<LinearCatalogLabel> for LinearCatalogLabelResponse {
    fn from(label: LinearCatalogLabel) -> Self {
        Self {
            id: label.id,
            name: label.name,
            color: label.color,
            is_group: label.is_group,
            parent_id: label.parent.map(|parent| parent.id),
            team_id: label.team.map(|team| team.id),
        }
    }
}

#[derive(Debug, Serialize)]
struct LinearCatalogResponse {
    teams: Vec<LinearCatalogTeam>,
    projects: Vec<LinearCatalogProject>,
    states: Vec<LinearCatalogState>,
    users: Vec<LinearCatalogUser>,
    labels: Vec<LinearCatalogLabelResponse>,
}

#[derive(Debug, Deserialize)]
struct LinearIssuePreviewPage {
    nodes: Vec<LinearIssuePreview>,
    #[serde(rename = "pageInfo")]
    page_info: LinearIssuePreviewPageInfo,
}

#[derive(Debug, Deserialize)]
struct LinearIssuePreview {
    state: Option<LinearIssuePreviewState>,
}

#[derive(Debug, Deserialize)]
struct LinearIssuePreviewState {
    id: String,
}

#[derive(Debug, Deserialize)]
struct LinearIssuePreviewPageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LinearIssuePreviewData {
    issues: LinearIssuePreviewPage,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct LinearRemoteIssue {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: i64,
    pub state: Option<LinearRemoteState>,
    #[serde(rename = "dueDate")]
    pub due_date: Option<String>,
    pub project: Option<LinearRemoteProject>,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    pub team: Option<LinearRemoteTeam>,
    pub assignee: Option<LinearRemoteUser>,
    pub labels: LinearCatalogPage<LinearRemoteLabel>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct LinearRemoteState {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub state_type: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct LinearRemoteProject {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct LinearRemoteTeam {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct LinearRemoteUser {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct LinearRemoteLabel {
    pub id: String,
}

const PATCHBAY_ISSUE_MARKER_PREFIX: &str = "<!-- patchbay:issue:";

/// Stable, hidden marker used to reconcile a successful Linear create when
/// the local transaction crashes before the link is committed. It is not a
/// title search and is never exposed as a user-facing field.
pub(crate) fn patchbay_issue_marker(issue_id: Uuid) -> String {
    format!("{PATCHBAY_ISSUE_MARKER_PREFIX}{issue_id} -->")
}

pub(crate) fn description_with_patchbay_marker(
    description: Option<&str>,
    issue_id: Uuid,
) -> String {
    let marker = patchbay_issue_marker(issue_id);
    let human = description.unwrap_or_default().trim();
    if human.is_empty() {
        marker
    } else {
        format!("{human}\n\n{marker}")
    }
}

/// Removes Patchbay's reconciliation marker before a remote description enters
/// the local human-authored field.
pub(crate) fn strip_patchbay_issue_marker(description: Option<&str>) -> Option<String> {
    let description = description?;
    let Some(start) = description.find(PATCHBAY_ISSUE_MARKER_PREFIX) else {
        return Some(description.to_string());
    };
    let end = description[start..].find("-->")? + start + 3;
    let mut human = String::new();
    let before = description[..start].trim();
    let after = description[end..].trim();
    if !before.is_empty() {
        human.push_str(before);
    }
    if !after.is_empty() {
        if !human.is_empty() {
            human.push_str("\n\n");
        }
        human.push_str(after);
    }
    Some(human).filter(|value| !value.is_empty())
}

pub(crate) fn patchbay_issue_id_from_description(description: Option<&str>) -> Option<Uuid> {
    let description = description?;
    let start =
        description.find(PATCHBAY_ISSUE_MARKER_PREFIX)? + PATCHBAY_ISSUE_MARKER_PREFIX.len();
    let end = description[start..].find("-->")? + start;
    Uuid::parse_str(description[start..end].trim()).ok()
}

#[derive(Debug, Deserialize)]
struct LinearRemoteIssueData {
    issue: Option<LinearRemoteIssue>,
}

#[derive(Debug, Deserialize)]
struct LinearRemoteIssuePage {
    nodes: Vec<LinearRemoteIssue>,
    #[serde(rename = "pageInfo")]
    page_info: LinearIssuePreviewPageInfo,
}

#[derive(Debug, Deserialize)]
struct LinearRemoteIssueListData {
    issues: LinearRemoteIssuePage,
}

#[derive(Debug, Serialize)]
struct LinearDryRunResponse {
    patchbay_project_id: Uuid,
    linear_project_id: String,
    sync_mode: String,
    initial_source_of_truth: Option<String>,
    local_issue_count: i64,
    remote_issue_count: i64,
    remote_issue_count_truncated: bool,
    candidate_import_count: i64,
    candidate_publish_count: i64,
    unmapped_remote_status_count: i64,
    exact_link_counts_available: bool,
}

#[derive(Debug)]
struct RemoteDryRunCounts {
    issue_count: i64,
    unmapped_status_count: i64,
    truncated: bool,
}

#[derive(Debug)]
struct LinearIdentity {
    actor_id: String,
    organization_id: String,
    organization_name: String,
}

fn advance_catalog_page<T>(
    cursor: CatalogCursor,
    page: Option<LinearCatalogPage<T>>,
) -> Result<(Vec<T>, CatalogCursor), LinearTokenError> {
    if cursor.done {
        return Ok((Vec::new(), cursor));
    }
    let Some(page) = page else {
        return Err(LinearTokenError::InvalidResponse);
    };
    if !page.page_info.has_next_page {
        return Ok((
            page.nodes,
            CatalogCursor {
                after: None,
                done: true,
            },
        ));
    }
    let Some(next) = page.page_info.end_cursor else {
        return Err(LinearTokenError::InvalidResponse);
    };
    if cursor.after.as_deref() == Some(next.as_str()) {
        return Err(LinearTokenError::InvalidResponse);
    }
    Ok((
        page.nodes,
        CatalogCursor {
            after: Some(next),
            done: false,
        },
    ))
}

#[derive(Debug, thiserror::Error)]
pub enum LinearTokenError {
    #[error("Linear authorization requires reauthorization")]
    InvalidGrant,
    #[error("Linear authorization requires reauthorization")]
    ReauthorizationRequired,
    #[error("Linear provider returned an invalid response")]
    InvalidResponse,
    #[error("Linear provider request failed")]
    Provider,
    #[error("Linear mutation was rejected: {0}")]
    MutationRejected(String),
    #[error("Linear provider rate limit reached")]
    RateLimited,
    #[error("Linear integration is not configured")]
    NotConfigured,
    #[error("Linear storage operation failed: {0}")]
    Storage(#[source] anyhow::Error),
    #[error("Linear secret storage operation failed: {0}")]
    Secret(#[source] anyhow::Error),
}

#[derive(Clone)]
pub struct LinearTokenManager {
    pool: PgPool,
    secret_box: patchbay_util::secretbox::SecretBox,
    client: reqwest::Client,
    client_id: String,
    client_secret: String,
    token_url: String,
    revoke_url: String,
    graphql_url: String,
}

impl LinearTokenManager {
    pub fn from_state(state: &HandlerState) -> Result<Self, LinearTokenError> {
        let Some(secret_box) = state.linear_secret_box.clone() else {
            return Err(LinearTokenError::NotConfigured);
        };
        let Some(client_id) = state
            .integrations
            .linear_client_id
            .as_deref()
            .and_then(|value| configured_value(Some(value)))
        else {
            return Err(LinearTokenError::NotConfigured);
        };
        let Some(client_secret) = state
            .integrations
            .linear_client_secret
            .as_deref()
            .and_then(|value| configured_value(Some(value)))
        else {
            return Err(LinearTokenError::NotConfigured);
        };
        let client = reqwest::Client::builder()
            .connect_timeout(LINEAR_HTTP_CONNECT_TIMEOUT)
            .timeout(LINEAR_HTTP_REQUEST_TIMEOUT)
            .build()
            .map_err(|error| {
                tracing::warn!(%error, "Linear HTTP client configuration failed");
                LinearTokenError::Provider
            })?;
        Ok(Self {
            pool: state.pool.clone(),
            secret_box,
            client,
            client_id,
            client_secret,
            token_url: state
                .integrations
                .linear_token_url
                .as_deref()
                .and_then(|value| configured_value(Some(value)))
                .unwrap_or_else(|| LINEAR_TOKEN_URL.to_string()),
            revoke_url: state
                .integrations
                .linear_revoke_url
                .as_deref()
                .and_then(|value| configured_value(Some(value)))
                .unwrap_or_else(|| LINEAR_REVOKE_URL.to_string()),
            graphql_url: state
                .integrations
                .linear_graphql_url
                .as_deref()
                .and_then(|value| configured_value(Some(value)))
                .unwrap_or_else(|| LINEAR_GRAPHQL_URL.to_string()),
        })
    }

    async fn exchange_authorization_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<LinearTokenResponse, LinearTokenError> {
        self.request_token(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("code_verifier", verifier),
        ])
        .await
    }

    async fn refresh_token(
        &self,
        refresh_token: &str,
    ) -> Result<LinearTokenResponse, LinearTokenError> {
        self.request_token(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
        ])
        .await
    }

    async fn request_token(
        &self,
        form: &[(&str, &str)],
    ) -> Result<LinearTokenResponse, LinearTokenError> {
        let response = self
            .client
            .post(&self.token_url)
            .form(form)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear token endpoint request failed");
                LinearTokenError::Provider
            })?;
        let status = response.status();
        let payload = response.json::<Value>().await.map_err(|error| {
            tracing::warn!(%error, "Linear token endpoint returned invalid JSON");
            LinearTokenError::InvalidResponse
        })?;
        if !status.is_success() {
            if payload.get("error").and_then(Value::as_str) == Some("invalid_grant") {
                return Err(LinearTokenError::InvalidGrant);
            }
            tracing::warn!(%status, "Linear token endpoint rejected request");
            return Err(LinearTokenError::Provider);
        }
        let token: LinearTokenResponse = serde_json::from_value(payload).map_err(|error| {
            tracing::warn!(%error, "Linear token response shape is invalid");
            LinearTokenError::InvalidResponse
        })?;
        if token.access_token.trim().is_empty()
            || token.refresh_token.as_deref().is_none_or(str::is_empty)
            || token.expires_in.is_none_or(|value| value <= 0)
        {
            return Err(LinearTokenError::InvalidResponse);
        }
        Ok(token)
    }

    async fn discover_identity(
        &self,
        access_token: &str,
    ) -> Result<LinearIdentity, LinearTokenError> {
        let response = self
            .client
            .post(&self.graphql_url)
            .bearer_auth(access_token)
            .json(&json!({
                "query": "query LinearInstallationIdentity { viewer { id } organization { id name } }"
            }))
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear identity request failed");
                LinearTokenError::Provider
            })?;
        let status = response.status();
        let payload = response
            .json::<GraphQlResponse<IdentityData>>()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear identity response is invalid JSON");
                LinearTokenError::InvalidResponse
            })?;
        if !status.is_success() {
            tracing::warn!(%status, "Linear identity request returned an error");
            return Err(LinearTokenError::Provider);
        }
        if let Some(errors) = payload.errors.as_ref().filter(|errors| !errors.is_empty()) {
            let message = errors
                .first()
                .and_then(|error| error.message.as_deref())
                .unwrap_or("unknown GraphQL error");
            tracing::warn!(message, "Linear identity GraphQL request failed");
            return Err(LinearTokenError::Provider);
        }
        let Some(data) = payload.data else {
            return Err(LinearTokenError::InvalidResponse);
        };
        let Some(organization) = data.organization else {
            return Err(LinearTokenError::InvalidResponse);
        };
        if data.viewer.id.trim().is_empty()
            || organization.id.trim().is_empty()
            || organization.name.trim().is_empty()
        {
            return Err(LinearTokenError::InvalidResponse);
        }
        Ok(LinearIdentity {
            actor_id: data.viewer.id,
            organization_id: organization.id,
            organization_name: organization.name,
        })
    }

    async fn query_catalog_page(
        &self,
        access_token: &str,
        cursors: &CatalogCursors,
    ) -> Result<LinearCatalogPageResponse, LinearTokenError> {
        let response = self
            .client
            .post(&self.graphql_url)
            .bearer_auth(access_token)
            .json(&json!({
                "query": "query LinearCatalog($teamsAfter: String, $teamsDone: Boolean!, $projectsAfter: String, $projectsDone: Boolean!, $statesAfter: String, $statesDone: Boolean!, $usersAfter: String, $usersDone: Boolean!, $labelsAfter: String, $labelsDone: Boolean!) { teams(first: 250, after: $teamsAfter) @skip(if: $teamsDone) { nodes { id name key } pageInfo { hasNextPage endCursor } } projects(first: 250, after: $projectsAfter) @skip(if: $projectsDone) { nodes { id name } pageInfo { hasNextPage endCursor } } workflowStates(first: 250, after: $statesAfter) @skip(if: $statesDone) { nodes { id name type color } pageInfo { hasNextPage endCursor } } users(first: 250, after: $usersAfter) @skip(if: $usersDone) { nodes { id name email } pageInfo { hasNextPage endCursor } } issueLabels(first: 250, after: $labelsAfter) @skip(if: $labelsDone) { nodes { id name color isGroup parent { id } team { id } } pageInfo { hasNextPage endCursor } } }",
                "variables": {
                    "teamsAfter": cursors.teams.after,
                    "teamsDone": cursors.teams.done,
                    "projectsAfter": cursors.projects.after,
                    "projectsDone": cursors.projects.done,
                    "statesAfter": cursors.workflow_states.after,
                    "statesDone": cursors.workflow_states.done,
                    "usersAfter": cursors.users.after,
                    "usersDone": cursors.users.done,
                    "labelsAfter": cursors.issue_labels.after,
                    "labelsDone": cursors.issue_labels.done,
                },
            }))
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear catalog request failed");
                LinearTokenError::Provider
            })?;
        let status = response.status();
        let payload = response
            .json::<GraphQlResponse<LinearCatalogPageResponse>>()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear catalog response is invalid JSON");
                LinearTokenError::InvalidResponse
            })?;
        if !status.is_success() {
            tracing::warn!(%status, "Linear catalog request returned an error");
            return Err(LinearTokenError::Provider);
        }
        if let Some(errors) = payload.errors.as_ref().filter(|errors| !errors.is_empty()) {
            let message = errors
                .first()
                .and_then(|error| error.message.as_deref())
                .unwrap_or("unknown GraphQL error");
            tracing::warn!(message, "Linear catalog GraphQL request failed");
            return Err(LinearTokenError::Provider);
        }
        payload.data.ok_or(LinearTokenError::InvalidResponse)
    }

    async fn query_catalog(
        &self,
        access_token: &str,
    ) -> Result<LinearCatalogData, LinearTokenError> {
        let mut cursors = CatalogCursors::default();
        let mut data = LinearCatalogData {
            teams: LinearCatalogPage {
                nodes: Vec::new(),
                page_info: LinearCatalogPageInfo {
                    has_next_page: false,
                    end_cursor: None,
                },
            },
            projects: LinearCatalogPage {
                nodes: Vec::new(),
                page_info: LinearCatalogPageInfo {
                    has_next_page: false,
                    end_cursor: None,
                },
            },
            workflow_states: LinearCatalogPage {
                nodes: Vec::new(),
                page_info: LinearCatalogPageInfo {
                    has_next_page: false,
                    end_cursor: None,
                },
            },
            users: LinearCatalogPage {
                nodes: Vec::new(),
                page_info: LinearCatalogPageInfo {
                    has_next_page: false,
                    end_cursor: None,
                },
            },
            issue_labels: LinearCatalogPage {
                nodes: Vec::new(),
                page_info: LinearCatalogPageInfo {
                    has_next_page: false,
                    end_cursor: None,
                },
            },
        };

        loop {
            let page = self.query_catalog_page(access_token, &cursors).await?;
            let (teams, teams_cursor) = advance_catalog_page(cursors.teams, page.teams)?;
            let (projects, projects_cursor) =
                advance_catalog_page(cursors.projects, page.projects)?;
            let (workflow_states, workflow_states_cursor) =
                advance_catalog_page(cursors.workflow_states, page.workflow_states)?;
            let (users, users_cursor) = advance_catalog_page(cursors.users, page.users)?;
            let (issue_labels, issue_labels_cursor) =
                advance_catalog_page(cursors.issue_labels, page.issue_labels)?;

            data.teams.nodes.extend(teams);
            data.projects.nodes.extend(projects);
            data.workflow_states.nodes.extend(workflow_states);
            data.users.nodes.extend(users);
            data.issue_labels.nodes.extend(issue_labels);
            cursors = CatalogCursors {
                teams: teams_cursor,
                projects: projects_cursor,
                workflow_states: workflow_states_cursor,
                users: users_cursor,
                issue_labels: issue_labels_cursor,
            };
            if cursors.teams.done
                && cursors.projects.done
                && cursors.workflow_states.done
                && cursors.users.done
                && cursors.issue_labels.done
            {
                break;
            }
        }
        Ok(data)
    }

    async fn remote_binding_is_valid(
        &self,
        connection_id: Uuid,
        organization_id: &str,
        linear_project_id: &str,
        linear_team_id: &str,
    ) -> Result<bool, LinearTokenError> {
        let access_token = self.access_token(connection_id).await?;
        let response = self
            .client
            .post(&self.graphql_url)
            .bearer_auth(access_token)
            .json(&json!({
                "query": "query LinearBindingValidation($projectId: ID!, $teamId: ID!) { project(id: $projectId) { id teams(first: 1, filter: { id: { eq: $teamId } }) { nodes { id } } } team(id: $teamId) { id organization { id } } }",
                "variables": {
                    "projectId": linear_project_id,
                    "teamId": linear_team_id,
                },
            }))
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear binding validation request failed");
                LinearTokenError::Provider
            })?;
        let status = response.status();
        let payload = response
            .json::<GraphQlResponse<LinearBindingValidationData>>()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear binding validation response is invalid JSON");
                LinearTokenError::InvalidResponse
            })?;
        if !status.is_success() {
            tracing::warn!(%status, "Linear binding validation request returned an error");
            return Err(LinearTokenError::Provider);
        }
        if let Some(errors) = payload.errors.as_ref().filter(|errors| !errors.is_empty()) {
            let message = errors
                .first()
                .and_then(|error| error.message.as_deref())
                .unwrap_or("unknown GraphQL error");
            tracing::warn!(message, "Linear binding validation GraphQL request failed");
            return Err(LinearTokenError::Provider);
        }
        let Some(data) = payload.data else {
            return Err(LinearTokenError::InvalidResponse);
        };
        Ok(data.project.is_some_and(|project| {
            project.id == linear_project_id
                && project
                    .teams
                    .nodes
                    .iter()
                    .any(|team| team.id == linear_team_id)
        }) && data.team.is_some_and(|team| {
            team.id == linear_team_id && team.organization.id == organization_id
        }))
    }

    async fn query_issue_preview_page(
        &self,
        access_token: &str,
        linear_project_id: &str,
        after: Option<String>,
    ) -> Result<LinearIssuePreviewData, LinearTokenError> {
        let response = self
            .client
            .post(&self.graphql_url)
            .bearer_auth(access_token)
            .json(&json!({
                "query": "query LinearProjectIssuePreview($projectId: ID!, $after: String) { issues(first: 250, after: $after, filter: { project: { id: { eq: $projectId } } }) { nodes { state { id } } pageInfo { hasNextPage endCursor } } }",
                "variables": {
                    "projectId": linear_project_id,
                    "after": after,
                },
            }))
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear issue preview request failed");
                LinearTokenError::Provider
            })?;
        let status = response.status();
        let payload = response
            .json::<GraphQlResponse<LinearIssuePreviewData>>()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear issue preview response is invalid JSON");
                LinearTokenError::InvalidResponse
            })?;
        if !status.is_success() {
            tracing::warn!(%status, "Linear issue preview request returned an error");
            return Err(LinearTokenError::Provider);
        }
        if let Some(errors) = payload.errors.as_ref().filter(|errors| !errors.is_empty()) {
            let message = errors
                .first()
                .and_then(|error| error.message.as_deref())
                .unwrap_or("unknown GraphQL error");
            tracing::warn!(message, "Linear issue preview GraphQL request failed");
            return Err(LinearTokenError::Provider);
        }
        payload.data.ok_or(LinearTokenError::InvalidResponse)
    }

    async fn query_remote_issue(
        &self,
        access_token: &str,
        linear_issue_id: &str,
    ) -> Result<Option<LinearRemoteIssue>, LinearTokenError> {
        let response = self
            .client
            .post(&self.graphql_url)
            .bearer_auth(access_token)
            .json(&json!({
                "query": "query LinearIssue($issueId: ID!) { issue(id: $issueId) { id identifier title description priority state { id name type } dueDate project { id } updatedAt team { id } assignee { id } labels { nodes { id } } } }",
                "variables": { "issueId": linear_issue_id },
            }))
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear issue request failed");
                LinearTokenError::Provider
            })?;
        let status = response.status();
        let payload = response
            .json::<GraphQlResponse<LinearRemoteIssueData>>()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear issue response is invalid JSON");
                LinearTokenError::InvalidResponse
            })?;
        if !status.is_success() {
            tracing::warn!(%status, "Linear issue request returned an error");
            return Err(LinearTokenError::Provider);
        }
        if let Some(errors) = payload.errors.as_ref().filter(|errors| !errors.is_empty()) {
            let message = errors
                .first()
                .and_then(|error| error.message.as_deref())
                .unwrap_or("unknown GraphQL error");
            tracing::warn!(message, "Linear issue GraphQL request failed");
            return Err(LinearTokenError::Provider);
        }
        Ok(payload.data.ok_or(LinearTokenError::InvalidResponse)?.issue)
    }

    async fn query_remote_issue_page(
        &self,
        access_token: &str,
        linear_project_id: &str,
        after: Option<&str>,
    ) -> Result<LinearRemoteIssuePage, LinearTokenError> {
        let response = self
            .client
            .post(&self.graphql_url)
            .bearer_auth(access_token)
            .json(&json!({
                "query": "query LinearProjectIssues($projectId: ID!, $after: String) { issues(first: 100, after: $after, filter: { project: { id: { eq: $projectId } } }) { nodes { id identifier title description priority state { id name type } dueDate project { id } updatedAt team { id } assignee { id } labels { nodes { id } } } pageInfo { hasNextPage endCursor } } }",
                "variables": {
                    "projectId": linear_project_id,
                    "after": after,
                },
            }))
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear project issue request failed");
                LinearTokenError::Provider
            })?;
        let status = response.status();
        let payload = response
            .json::<GraphQlResponse<LinearRemoteIssueListData>>()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear project issue response is invalid JSON");
                LinearTokenError::InvalidResponse
            })?;
        if !status.is_success() {
            tracing::warn!(%status, "Linear project issue request returned an error");
            return Err(LinearTokenError::Provider);
        }
        if let Some(errors) = payload.errors.as_ref().filter(|errors| !errors.is_empty()) {
            let message = errors
                .first()
                .and_then(|error| error.message.as_deref())
                .unwrap_or("unknown GraphQL error");
            tracing::warn!(message, "Linear project issue GraphQL request failed");
            return Err(LinearTokenError::Provider);
        }
        Ok(payload
            .data
            .ok_or(LinearTokenError::InvalidResponse)?
            .issues)
    }

    async fn mutate_issue(
        &self,
        access_token: &str,
        operation: &str,
        query: &str,
        variables: Value,
    ) -> Result<LinearRemoteIssue, LinearTokenError> {
        let response = self
            .client
            .post(&self.graphql_url)
            .bearer_auth(access_token)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear issue mutation request failed");
                LinearTokenError::Provider
            })?;
        let status = response.status();
        if status.as_u16() == 429 {
            tracing::warn!(%status, "Linear issue mutation was rate limited");
            return Err(LinearTokenError::RateLimited);
        }
        let payload = response
            .json::<GraphQlResponse<Value>>()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear issue mutation response is invalid JSON");
                LinearTokenError::InvalidResponse
            })?;
        if !status.is_success() {
            if status.is_client_error() {
                return Err(LinearTokenError::MutationRejected(format!(
                    "Linear issue mutation returned HTTP {status}"
                )));
            }
            return Err(LinearTokenError::Provider);
        }
        if let Some(errors) = payload.errors.as_ref().filter(|errors| !errors.is_empty()) {
            let message = errors
                .iter()
                .filter_map(|error| error.message.as_deref())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(LinearTokenError::MutationRejected(if message.is_empty() {
                "GraphQL mutation failed".to_string()
            } else {
                message
            }));
        }
        let result = payload
            .data
            .as_ref()
            .and_then(|data| data.get(operation))
            .ok_or(LinearTokenError::InvalidResponse)?;
        let success = result
            .get("success")
            .and_then(Value::as_bool)
            .ok_or(LinearTokenError::InvalidResponse)?;
        if !success {
            let message = result
                .get("userErrors")
                .and_then(Value::as_array)
                .map(|errors| {
                    errors
                        .iter()
                        .filter_map(|error| error.get("message").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| "Linear rejected the issue mutation".to_string());
            return Err(LinearTokenError::MutationRejected(message));
        }
        let issue_value = result
            .get("issue")
            .cloned()
            .ok_or(LinearTokenError::InvalidResponse)?;
        let issue = serde_json::from_value::<LinearRemoteIssue>(issue_value)
            .map_err(|_| LinearTokenError::InvalidResponse)?;
        if issue.id.trim().is_empty()
            || issue.identifier.trim().is_empty()
            || issue.updated_at.trim().is_empty()
        {
            return Err(LinearTokenError::InvalidResponse);
        }
        Ok(issue)
    }

    pub(crate) async fn create_issue(
        &self,
        connection_id: Uuid,
        team_id: &str,
        project_id: &str,
        issue_id: Uuid,
        title: &str,
        description: Option<&str>,
        priority: i64,
        state_id: Option<&str>,
        due_date: Option<&str>,
        assignee_id: Option<&str>,
    ) -> Result<LinearRemoteIssue, LinearTokenError> {
        let access_token = self.access_token(connection_id).await?;
        let mut input = serde_json::Map::new();
        input.insert("teamId".to_string(), json!(team_id));
        input.insert("projectId".to_string(), json!(project_id));
        input.insert("title".to_string(), json!(title));
        input.insert(
            "description".to_string(),
            json!(description_with_patchbay_marker(description, issue_id)),
        );
        input.insert("priority".to_string(), json!(priority));
        if let Some(state_id) = state_id {
            input.insert("stateId".to_string(), json!(state_id));
        }
        if let Some(due_date) = due_date {
            input.insert("dueDate".to_string(), json!(due_date));
        }
        if let Some(assignee_id) = assignee_id {
            input.insert("assigneeId".to_string(), json!(assignee_id));
        }
        self.mutate_issue(
            &access_token,
            "issueCreate",
            "mutation LinearIssueCreate($input: IssueCreateInput!) { issueCreate(input: $input) { success issue { id identifier title description priority state { id name type } dueDate project { id } updatedAt team { id } assignee { id } labels { nodes { id } } } userErrors { message } } }",
            json!({ "input": Value::Object(input) }),
        )
        .await
    }

    pub(crate) async fn update_issue(
        &self,
        connection_id: Uuid,
        linear_issue_id: &str,
        patchbay_issue_id: Uuid,
        title: &str,
        description: Option<&str>,
        priority: i64,
        state_id: Option<&str>,
        due_date: Option<&str>,
        assignee_id: Option<Option<&str>>,
    ) -> Result<LinearRemoteIssue, LinearTokenError> {
        let access_token = self.access_token(connection_id).await?;
        let mut input = serde_json::Map::new();
        input.insert("title".to_string(), json!(title));
        input.insert(
            "description".to_string(),
            json!(description_with_patchbay_marker(
                description,
                patchbay_issue_id
            )),
        );
        input.insert("priority".to_string(), json!(priority));
        input.insert("dueDate".to_string(), json!(due_date));
        if let Some(state_id) = state_id {
            input.insert("stateId".to_string(), json!(state_id));
        }
        if let Some(assignee_id) = assignee_id {
            input.insert("assigneeId".to_string(), json!(assignee_id));
        }
        self.mutate_issue(
            &access_token,
            "issueUpdate",
            "mutation LinearIssueUpdate($issueId: ID!, $input: IssueUpdateInput!) { issueUpdate(id: $issueId, input: $input) { success issue { id identifier title description priority state { id name type } dueDate project { id } updatedAt team { id } assignee { id } labels { nodes { id } } } userErrors { message } } }",
            json!({ "issueId": linear_issue_id, "input": Value::Object(input) }),
        )
        .await
    }

    /// Lists the bound project and searches the hidden marker used by a
    /// previous create attempt. This is bounded by the same import cap as the
    /// initial importer and is the only crash-reconciliation lookup.
    pub(crate) async fn find_issue_by_marker(
        &self,
        connection_id: Uuid,
        project_id: &str,
        patchbay_issue_id: Uuid,
    ) -> Result<Option<LinearRemoteIssue>, LinearTokenError> {
        let issues = self.list_project_issues(connection_id, project_id).await?;
        Ok(issues.into_iter().find(|issue| {
            patchbay_issue_id_from_description(issue.description.as_deref())
                == Some(patchbay_issue_id)
        }))
    }

    async fn dry_run_counts(
        &self,
        connection_id: Uuid,
        linear_project_id: &str,
        status_mapping: &Value,
    ) -> Result<RemoteDryRunCounts, LinearTokenError> {
        const MAX_PREVIEW_ISSUES: i64 = 10_000;

        let access_token = self.access_token(connection_id).await?;
        let mut after = None;
        let mut issue_count = 0;
        let mut unmapped_status_count = 0;
        let mut truncated = false;
        loop {
            let page = self
                .query_issue_preview_page(&access_token, linear_project_id, after.clone())
                .await?;
            for issue in page.issues.nodes {
                if issue_count >= MAX_PREVIEW_ISSUES {
                    truncated = true;
                    break;
                }
                issue_count += 1;
                let is_mapped = issue.state.as_ref().is_some_and(|state| {
                    if state.id.trim().is_empty() {
                        return false;
                    }
                    status_mapping
                        .as_object()
                        .and_then(|mapping| mapping.get(&state.id))
                        .is_some_and(|value| match value {
                            Value::String(value) => !value.trim().is_empty(),
                            Value::Null => false,
                            _ => true,
                        })
                });
                if !is_mapped {
                    unmapped_status_count += 1;
                }
            }
            if truncated || !page.issues.page_info.has_next_page {
                break;
            }
            let Some(next_cursor) = page.issues.page_info.end_cursor else {
                return Err(LinearTokenError::InvalidResponse);
            };
            if after.as_deref() == Some(next_cursor.as_str()) {
                return Err(LinearTokenError::InvalidResponse);
            }
            after = Some(next_cursor);
        }
        Ok(RemoteDryRunCounts {
            issue_count,
            unmapped_status_count,
            truncated,
        })
    }

    pub async fn catalog(
        &self,
        connection_id: Uuid,
    ) -> Result<LinearCatalogResponse, LinearTokenError> {
        let access_token = self.access_token(connection_id).await?;
        let catalog = self.query_catalog(&access_token).await?;
        if catalog.teams.nodes.iter().any(|team| {
            team.id.trim().is_empty() || team.name.trim().is_empty() || team.key.trim().is_empty()
        }) || catalog
            .projects
            .nodes
            .iter()
            .any(|project| project.id.trim().is_empty() || project.name.trim().is_empty())
            || catalog.workflow_states.nodes.iter().any(|state| {
                state.id.trim().is_empty()
                    || state.name.trim().is_empty()
                    || state.state_type.trim().is_empty()
                    || state.color.trim().is_empty()
            })
            || catalog.users.nodes.iter().any(|user| {
                user.id.trim().is_empty()
                    || user.name.trim().is_empty()
                    || user
                        .email
                        .as_deref()
                        .map_or(false, |email| email.trim().is_empty())
            })
            || catalog.issue_labels.nodes.iter().any(|label| {
                label.id.trim().is_empty()
                    || label.name.trim().is_empty()
                    || label.color.trim().is_empty()
            })
        {
            return Err(LinearTokenError::InvalidResponse);
        }
        Ok(LinearCatalogResponse {
            teams: catalog.teams.nodes,
            projects: catalog.projects.nodes,
            states: catalog.workflow_states.nodes,
            users: catalog.users.nodes,
            labels: catalog
                .issue_labels
                .nodes
                .into_iter()
                .map(LinearCatalogLabelResponse::from)
                .collect(),
        })
    }

    /// Fetches a complete Issue after a Webhook notification. Webhook data is
    /// intentionally not treated as a durable snapshot because nested fields
    /// may be omitted by Linear.
    pub(crate) async fn fetch_issue(
        &self,
        connection_id: Uuid,
        linear_issue_id: &str,
    ) -> Result<Option<LinearRemoteIssue>, LinearTokenError> {
        let access_token = self.access_token(connection_id).await?;
        self.query_remote_issue(&access_token, linear_issue_id)
            .await
    }

    /// Enumerates every Issue in a bound Linear Project for initial import.
    /// A hard cap protects the worker from an unexpectedly large project; the
    /// worker treats crossing it as a retryable provider response rather than
    /// silently importing a partial project.
    pub(crate) async fn list_project_issues(
        &self,
        connection_id: Uuid,
        linear_project_id: &str,
    ) -> Result<Vec<LinearRemoteIssue>, LinearTokenError> {
        const MAX_IMPORT_ISSUES: usize = 50_000;
        let access_token = self.access_token(connection_id).await?;
        let mut after = None;
        let mut issues = Vec::new();
        loop {
            let page = self
                .query_remote_issue_page(&access_token, linear_project_id, after.as_deref())
                .await?;
            issues.extend(page.nodes);
            if issues.len() > MAX_IMPORT_ISSUES {
                return Err(LinearTokenError::InvalidResponse);
            }
            if !page.page_info.has_next_page {
                return Ok(issues);
            }
            let Some(next_cursor) = page.page_info.end_cursor else {
                return Err(LinearTokenError::InvalidResponse);
            };
            if after.as_deref() == Some(next_cursor.as_str()) {
                return Err(LinearTokenError::InvalidResponse);
            }
            after = Some(next_cursor);
        }
    }

    /// Returns an access token and refreshes it while holding the connection
    /// row lock. The refresh response must contain the rotated refresh token;
    /// both encrypted values are replaced in one database transaction.
    pub async fn access_token(&self, connection_id: Uuid) -> Result<String, LinearTokenError> {
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let Some(connection) = linear_q::get_connection_for_update(&mut transaction, connection_id)
            .await
            .map_err(storage_error)?
        else {
            return Err(LinearTokenError::InvalidResponse);
        };
        match connection.status.as_str() {
            "active" => {}
            "reauthorization_required" => return Err(LinearTokenError::ReauthorizationRequired),
            _ => return Err(LinearTokenError::InvalidResponse),
        }
        let access_token = open_secret(&self.secret_box, &connection.access_token_encrypted)?;
        if connection.token_expires_at > Utc::now() + TOKEN_REFRESH_SKEW {
            transaction.commit().await.map_err(storage_error)?;
            return Ok(access_token);
        }
        let refresh_token = open_secret(&self.secret_box, &connection.refresh_token_encrypted)?;
        let refreshed = match self.refresh_token(&refresh_token).await {
            Ok(token) => token,
            Err(LinearTokenError::InvalidGrant) => {
                linear_q::mark_reauthorization_required(
                    &mut transaction,
                    connection_id,
                    "invalid_grant",
                )
                .await
                .map_err(storage_error)?;
                transaction.commit().await.map_err(storage_error)?;
                return Err(LinearTokenError::InvalidGrant);
            }
            Err(error) => return Err(error),
        };
        let rotated_refresh = refreshed
            .refresh_token
            .as_deref()
            .ok_or(LinearTokenError::InvalidResponse)?;
        let expires_in = refreshed
            .expires_in
            .ok_or(LinearTokenError::InvalidResponse)?;
        let access_encrypted = seal_secret(&self.secret_box, &refreshed.access_token)?;
        let refresh_encrypted = seal_secret(&self.secret_box, rotated_refresh)?;
        let scopes = refreshed
            .scope
            .map(LinearScopeResponse::into_json)
            .unwrap_or_else(|| connection.scopes.clone());
        linear_q::update_tokens(
            &mut transaction,
            connection_id,
            &access_encrypted,
            &refresh_encrypted,
            Utc::now() + Duration::seconds(expires_in),
            &scopes,
        )
        .await
        .map_err(storage_error)?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(refreshed.access_token)
    }

    async fn revoke_connection(
        &self,
        workspace_id: Uuid,
        connection: &LinearConnection,
    ) -> Result<(), LinearTokenError> {
        match connection.status.as_str() {
            "revoked" => return Ok(()),
            "reauthorization_required" => return Err(LinearTokenError::ReauthorizationRequired),
            _ => {}
        }
        let access_token = self.access_token(connection.id).await?;
        let response = self
            .client
            .post(&self.revoke_url)
            .form(&[
                ("token", access_token.as_str()),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear revoke request failed");
                LinearTokenError::Provider
            })?;
        if !response.status().is_success() {
            tracing::warn!(status = %response.status(), "Linear revoke request rejected");
            return Err(LinearTokenError::Provider);
        }
        let marked = linear_q::mark_revoked(&self.pool, workspace_id, connection.id)
            .await
            .map_err(storage_error)?;
        if !marked {
            return Err(LinearTokenError::InvalidResponse);
        }
        Ok(())
    }
}

fn storage_error<E>(error: E) -> LinearTokenError
where
    E: Into<anyhow::Error>,
{
    LinearTokenError::Storage(error.into())
}

fn seal_secret(
    secret_box: &patchbay_util::secretbox::SecretBox,
    plaintext: &str,
) -> Result<String, LinearTokenError> {
    secret_box
        .seal(plaintext.as_bytes())
        .map(|value| STANDARD.encode(value))
        .map_err(|error| LinearTokenError::Secret(anyhow::Error::from(error)))
}

fn open_secret(
    secret_box: &patchbay_util::secretbox::SecretBox,
    ciphertext: &str,
) -> Result<String, LinearTokenError> {
    let decoded = STANDARD
        .decode(ciphertext)
        .map_err(|error| LinearTokenError::Secret(anyhow::Error::from(error)))?;
    let plaintext = secret_box
        .open(&decoded)
        .map_err(|error| LinearTokenError::Secret(anyhow::Error::from(error)))?;
    String::from_utf8(plaintext)
        .map_err(|error| LinearTokenError::Secret(anyhow::Error::from(error)))
}

fn seal(state: &HandlerState, plaintext: &str) -> anyhow::Result<String> {
    let secret_box = state
        .linear_secret_box
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Linear secret storage is not configured"))?;
    seal_secret(secret_box, plaintext).map_err(|error| anyhow::anyhow!(error.to_string()))
}

async fn oauth_callback(
    State(state): State<HandlerState>,
    Query(query): Query<OAuthCallbackQuery>,
) -> Response {
    if !state.linear_integration_enabled {
        return linear_callback_redirect(&state, "not_configured");
    }
    let Some(state_token) = query.state.filter(|value| !value.trim().is_empty()) else {
        return linear_callback_redirect(&state, "invalid_request");
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "Linear OAuth callback transaction failed");
            return linear_callback_redirect(&state, "error");
        }
    };
    let oauth_state =
        match linear_q::consume_oauth_state(&mut *transaction, &sha256_hex(&state_token)).await {
            Ok(Some(value)) => value,
            Ok(None) => return linear_callback_redirect(&state, "invalid_state"),
            Err(error) => {
                tracing::warn!(%error, "Linear OAuth state lookup failed");
                return linear_callback_redirect(&state, "error");
            }
        };
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, "Linear OAuth state commit failed");
        return linear_callback_redirect(&state, "error");
    }
    if query.error.is_some() {
        return linear_callback_redirect(&state, "denied");
    }
    let Some(code) = query.code.filter(|value| !value.trim().is_empty()) else {
        return linear_callback_redirect(&state, "invalid_request");
    };
    // Avoid issuing a second provider grant when another connect completed
    // while this callback was in flight. The locked check immediately before
    // the upsert below closes the remaining race with a concurrent callback.
    match linear_q::get_connection_for_workspace(&state.pool, oauth_state.workspace_id).await {
        Ok(Some(connection)) if connection.status == "active" => {
            return linear_callback_redirect(&state, "already_connected");
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, "Linear existing connection check failed");
            return linear_callback_redirect(&state, "error");
        }
    }
    let verifier = match open(&state, &oauth_state.code_verifier_encrypted) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "Linear OAuth verifier decryption failed");
            return linear_callback_redirect(&state, "error");
        }
    };
    let manager = match LinearTokenManager::from_state(&state) {
        Ok(manager) => manager,
        Err(error) => {
            tracing::warn!(%error, "Linear OAuth configuration is incomplete");
            return linear_callback_redirect(&state, "not_configured");
        }
    };
    let token = match manager
        .exchange_authorization_code(&code, &verifier, &oauth_state.redirect_uri)
        .await
    {
        Ok(token) => token,
        Err(error) => {
            tracing::warn!(%error, "Linear OAuth token exchange failed");
            return linear_callback_redirect(&state, "error");
        }
    };
    let identity = match manager.discover_identity(&token.access_token).await {
        Ok(identity) => identity,
        Err(error) => {
            tracing::warn!(%error, "Linear installation identity discovery failed");
            return linear_callback_redirect(&state, "error");
        }
    };
    let refresh_token = match token.refresh_token.as_deref() {
        Some(value) if !value.trim().is_empty() => value,
        _ => return linear_callback_redirect(&state, "error"),
    };
    let expires_in = match token.expires_in {
        Some(value) if value > 0 => value,
        _ => return linear_callback_redirect(&state, "error"),
    };
    let access_encrypted = match seal_secret(&manager.secret_box, &token.access_token) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "Linear access token encryption failed");
            return linear_callback_redirect(&state, "error");
        }
    };
    let refresh_encrypted = match seal_secret(&manager.secret_box, refresh_token) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "Linear refresh token encryption failed");
            return linear_callback_redirect(&state, "error");
        }
    };
    let scopes = token
        .scope
        .map(LinearScopeResponse::into_json)
        .unwrap_or_else(|| json!([]));
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "Linear connection transaction failed");
            return linear_callback_redirect(&state, "error");
        }
    };
    let member = match patchbay_db::queries::member::lock_member_by_user_and_workspace(
        &mut *transaction,
        oauth_state.user_id,
        oauth_state.workspace_id,
    )
    .await
    {
        Ok(Some(member)) => member,
        Ok(None) => return linear_callback_redirect(&state, "unauthorized"),
        Err(error) => {
            tracing::warn!(%error, "Linear OAuth membership revalidation failed");
            return linear_callback_redirect(&state, "error");
        }
    };
    if !matches!(member.role.as_str(), "owner" | "admin") {
        return linear_callback_redirect(&state, "unauthorized");
    }
    match linear_q::get_connection_for_workspace_for_update(
        &mut transaction,
        oauth_state.workspace_id,
    )
    .await
    {
        Ok(Some(connection)) if connection.status != "revoked" => {
            return linear_callback_redirect(&state, "already_connected");
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, "Linear existing connection check failed");
            return linear_callback_redirect(&state, "error");
        }
    }
    if let Err(error) = linear_q::upsert_connection(
        &mut *transaction,
        &linear_q::LinearConnectionInput {
            id: Uuid::now_v7(),
            workspace_id: oauth_state.workspace_id,
            organization_id: &identity.organization_id,
            organization_name: &identity.organization_name,
            actor_id: &identity.actor_id,
            access_token_encrypted: &access_encrypted,
            refresh_token_encrypted: &refresh_encrypted,
            token_expires_at: Utc::now() + Duration::seconds(expires_in),
            scopes: &scopes,
            created_by_id: oauth_state.user_id,
        },
    )
    .await
    {
        tracing::warn!(%error, "Linear connection persistence failed");
        return linear_callback_redirect(&state, "error");
    }
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, "Linear connection transaction commit failed");
        return linear_callback_redirect(&state, "error");
    }
    linear_callback_redirect(&state, "connected")
}

fn open(state: &HandlerState, ciphertext: &str) -> anyhow::Result<String> {
    let secret_box = state
        .linear_secret_box
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Linear secret storage is not configured"))?;
    let decoded = STANDARD.decode(ciphertext)?;
    Ok(String::from_utf8(secret_box.open(&decoded)?)?)
}

async fn disconnect(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let connection = match linear_q::get_connection_for_workspace(&state.pool, workspace_id).await {
        Ok(Some(connection)) => connection,
        Ok(None) => return StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::warn!(%error, "Linear disconnect lookup failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load Linear connection",
            );
        }
    };
    if connection.status == "revoked" {
        return StatusCode::NO_CONTENT.into_response();
    }
    let manager = match LinearTokenManager::from_state(&state) {
        Ok(manager) => manager,
        Err(error) => {
            tracing::warn!(%error, "Linear disconnect configuration is incomplete");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Linear OAuth is not configured",
            );
        }
    };
    match manager.revoke_connection(workspace_id, &connection).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(LinearTokenError::InvalidGrant | LinearTokenError::ReauthorizationRequired) => {
            match linear_q::mark_revoked(&state.pool, workspace_id, connection.id).await {
                Ok(_) => StatusCode::NO_CONTENT.into_response(),
                Err(error) => {
                    tracing::warn!(%error, "Linear local disconnect fallback failed");
                    error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to disconnect Linear",
                    )
                }
            }
        }
        Err(LinearTokenError::Provider) => {
            error_response(StatusCode::BAD_GATEWAY, "Linear revoke request failed")
        }
        Err(error) => {
            tracing::warn!(%error, "Linear disconnect failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to disconnect Linear",
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct LinearWebhookEnvelope {
    #[serde(rename = "id")]
    event_id: Option<String>,
    #[serde(rename = "organizationId")]
    organization_id: Option<String>,
    #[serde(rename = "webhookId")]
    webhook_id: Option<String>,
    #[serde(rename = "webhookTimestamp")]
    webhook_timestamp: Option<i64>,
    #[serde(rename = "type")]
    event_type: Option<String>,
}

#[derive(Debug, PartialEq)]
struct VerifiedWebhook {
    organization_id: String,
    webhook_id: String,
    delivery_id: String,
    event_type: String,
    payload: Value,
}

#[derive(Debug, PartialEq, Eq)]
enum WebhookValidationError {
    MissingSecret,
    MissingSignature,
    InvalidSignature,
    InvalidPayload,
    MissingOrganization,
    MissingWebhook,
    MissingTimestamp,
    InvalidTimestampHeader,
    TimestampMismatch,
    ExpiredTimestamp,
}

fn validate_webhook(
    secret: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
    now_ms: i64,
) -> Result<VerifiedWebhook, WebhookValidationError> {
    let secret = secret
        .and_then(|value| configured_value(Some(value)))
        .ok_or(WebhookValidationError::MissingSecret)?;
    let signature = header_value(headers, "linear-signature")
        .ok_or(WebhookValidationError::MissingSignature)?;
    if !verify_signature(&secret, &signature, body) {
        return Err(WebhookValidationError::InvalidSignature);
    }
    let payload = serde_json::from_slice::<Value>(body)
        .map_err(|_| WebhookValidationError::InvalidPayload)?;
    let envelope = serde_json::from_value::<LinearWebhookEnvelope>(payload.clone())
        .map_err(|_| WebhookValidationError::InvalidPayload)?;
    let organization_id = envelope
        .organization_id
        .as_deref()
        .and_then(|value| configured_value(Some(value)))
        .ok_or(WebhookValidationError::MissingOrganization)?;
    let webhook_id = envelope
        .webhook_id
        .as_deref()
        .and_then(|value| configured_value(Some(value)))
        .ok_or(WebhookValidationError::MissingWebhook)?;
    let webhook_timestamp = envelope
        .webhook_timestamp
        .ok_or(WebhookValidationError::MissingTimestamp)?;
    if let Some(header_timestamp) = header_value(headers, "linear-timestamp") {
        let header_timestamp = header_timestamp
            .parse::<i64>()
            .map_err(|_| WebhookValidationError::InvalidTimestampHeader)?;
        if header_timestamp != webhook_timestamp {
            return Err(WebhookValidationError::TimestampMismatch);
        }
    }
    if !timestamp_is_fresh(webhook_timestamp, now_ms) {
        return Err(WebhookValidationError::ExpiredTimestamp);
    }
    // Linear's delivery header is the retry identity. The body id is retained
    // as a compatibility fallback for fixtures/providers that omit the
    // header; never let an entity id collapse distinct deliveries when the
    // provider supplies the documented delivery key.
    let delivery_id = header_value(headers, "linear-delivery")
        .or_else(|| {
            envelope
                .event_id
                .as_deref()
                .and_then(|value| configured_value(Some(value)))
        })
        .unwrap_or_else(|| sha256_hex_bytes(body));
    let event_type = header_value(headers, "linear-event")
        .or_else(|| configured_value(envelope.event_type.as_deref()))
        .unwrap_or_else(|| "unknown".to_string());
    Ok(VerifiedWebhook {
        organization_id,
        webhook_id,
        delivery_id,
        event_type,
        payload,
    })
}

fn webhook_validation_response(error: WebhookValidationError) -> Response {
    let (status, message) = match error {
        WebhookValidationError::MissingSecret => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Linear Webhook secret is not configured",
        ),
        WebhookValidationError::MissingSignature => {
            (StatusCode::UNAUTHORIZED, "missing Linear signature")
        }
        WebhookValidationError::InvalidSignature => {
            (StatusCode::UNAUTHORIZED, "invalid Linear signature")
        }
        WebhookValidationError::InvalidPayload => {
            (StatusCode::BAD_REQUEST, "invalid Linear Webhook payload")
        }
        WebhookValidationError::MissingOrganization => (
            StatusCode::BAD_REQUEST,
            "Linear Webhook organizationId is required",
        ),
        WebhookValidationError::MissingWebhook => (
            StatusCode::BAD_REQUEST,
            "Linear Webhook webhookId is required",
        ),
        WebhookValidationError::MissingTimestamp => (
            StatusCode::BAD_REQUEST,
            "Linear Webhook webhookTimestamp is required",
        ),
        WebhookValidationError::InvalidTimestampHeader => (
            StatusCode::BAD_REQUEST,
            "Linear Webhook timestamp header is invalid",
        ),
        WebhookValidationError::TimestampMismatch => (
            StatusCode::BAD_REQUEST,
            "Linear Webhook timestamp header does not match webhookTimestamp",
        ),
        WebhookValidationError::ExpiredTimestamp => (
            StatusCode::BAD_REQUEST,
            "Linear Webhook timestamp is expired",
        ),
    };
    error_response(status, message)
}

async fn linear_webhook(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let verified = match validate_webhook(
        state.integrations.linear_webhook_secret.as_deref(),
        &headers,
        &body,
        current_time_millis(),
    ) {
        Ok(verified) => verified,
        Err(error) => return webhook_validation_response(error),
    };

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "Linear Webhook transaction failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Linear Webhook persistence unavailable",
            );
        }
    };
    let candidates = match linear_q::find_connections_for_webhook(
        &mut transaction,
        &verified.organization_id,
        &verified.webhook_id,
    )
    .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(%error, "Linear Webhook installation lookup failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Linear Webhook persistence unavailable",
            );
        }
    };
    if candidates.len() != 1 {
        return error_response(StatusCode::NOT_FOUND, "unknown Linear Webhook installation");
    }
    let connection = &candidates[0];
    if connection.webhook_id.is_none() {
        match linear_q::bind_webhook(&mut transaction, connection.id, &verified.webhook_id).await {
            Ok(true) => {}
            Ok(false) => {
                return error_response(StatusCode::CONFLICT, "Linear Webhook installation changed")
            }
            Err(error) => {
                tracing::warn!(%error, "Linear Webhook identity binding failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Linear Webhook persistence unavailable",
                );
            }
        }
    }
    let inserted = match linear_q::insert_sync_inbox(
        &mut transaction,
        Uuid::now_v7(),
        connection.id,
        &verified.delivery_id,
        &verified.event_type,
        &verified.payload,
    )
    .await
    {
        Ok(inserted) => inserted,
        Err(error) => {
            tracing::warn!(%error, "Linear Webhook Inbox insert failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Linear Webhook persistence unavailable",
            );
        }
    };
    if let Err(error) = linear_q::mark_webhook_accepted(&mut transaction, connection.id).await {
        tracing::warn!(%error, "Linear Webhook health update failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Linear Webhook persistence unavailable",
        );
    }
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, "Linear Webhook transaction commit failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Linear Webhook persistence unavailable",
        );
    }
    if inserted {
        state.notify_linear_sync();
    }
    (
        StatusCode::OK,
        Json(json!({ "accepted": true, "duplicate": !inserted })),
    )
        .into_response()
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| configured_value(Some(value)))
}

fn verify_signature(secret: &str, signature: &str, body: &[u8]) -> bool {
    let Ok(signature) = hex::decode(signature) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&signature).is_ok()
}

fn timestamp_is_fresh(timestamp_ms: i64, now_ms: i64) -> bool {
    (i128::from(now_ms) - i128::from(timestamp_ms)).abs() <= WEBHOOK_MAX_AGE_MS
}

fn current_time_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

fn random_token(size: usize) -> String {
    let mut bytes = vec![0_u8; size];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn sha256_hex_bytes(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LinearScopeResponse {
    Text(String),
    List(Vec<String>),
}

impl LinearScopeResponse {
    fn into_json(self) -> Value {
        match self {
            Self::Text(value) => parse_scopes(&value),
            Self::List(values) => Value::Array(
                values
                    .into_iter()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .map(Value::String)
                    .collect(),
            ),
        }
    }
}

fn parse_scopes(scope: &str) -> Value {
    Value::Array(
        scope
            .split(|character: char| character == ',' || character.is_ascii_whitespace())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| Value::String(value.to_string()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn signed_headers(secret: &str, body: &[u8], timestamp: Option<i64>) -> HeaderMap {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let mut headers = HeaderMap::new();
        headers.insert(
            "linear-signature",
            HeaderValue::from_str(&hex::encode(mac.finalize().into_bytes())).unwrap(),
        );
        if let Some(timestamp) = timestamp {
            headers.insert(
                "linear-timestamp",
                HeaderValue::from_str(&timestamp.to_string()).unwrap(),
            );
        }
        headers
    }

    #[test]
    fn official_token_fixture_does_not_require_installation_fields() {
        let token: LinearTokenResponse = serde_json::from_value(json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "expires_in": 86399,
            "scope": "read write"
        }))
        .expect("documented token response should deserialize");
        assert_eq!(token.access_token, "access");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh"));
        let token_with_array_scope: LinearTokenResponse = serde_json::from_value(json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "expires_in": 86399,
            "scope": ["read", "write"]
        }))
        .expect("array scope response should deserialize");
        assert_eq!(
            token_with_array_scope
                .scope
                .expect("scope should be present")
                .into_json(),
            json!(["read", "write"])
        );
    }

    #[test]
    fn scopes_accept_documented_space_format_and_authorization_comma_format() {
        assert_eq!(parse_scopes("read write"), json!(["read", "write"]));
        assert_eq!(
            parse_scopes("read,write,issues:create,app:assignable"),
            json!(["read", "write", "issues:create", "app:assignable"])
        );
        assert_eq!(
            LinearScopeResponse::List(vec!["read".into(), " write ".into()]).into_json(),
            json!(["read", "write"])
        );
    }

    #[test]
    fn authorization_url_uses_pkce_app_actor_and_comma_scopes() {
        let authorization_url = build_authorization_url(
            "https://linear.example/oauth/authorize?existing=1",
            "client-1",
            "https://api.example/api/linear/oauth/callback",
            "state-1",
            "verifier-1",
        )
        .unwrap();
        let url = Url::parse(&authorization_url).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("existing").map(String::as_str), Some("1"));
        assert_eq!(query.get("client_id").map(String::as_str), Some("client-1"));
        assert_eq!(
            query.get("redirect_uri").map(String::as_str),
            Some("https://api.example/api/linear/oauth/callback")
        );
        assert_eq!(query.get("actor").map(String::as_str), Some("app"));
        assert_eq!(
            query.get("scope").map(String::as_str),
            Some("read,write,issues:create,app:assignable")
        );
        assert_eq!(
            query.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        let expected_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(b"verifier-1"));
        assert_eq!(
            query.get("code_challenge").map(String::as_str),
            Some(expected_challenge.as_str())
        );
    }

    #[test]
    fn patchbay_issue_marker_round_trips_without_polluting_human_description() {
        let issue_id = Uuid::now_v7();
        let description = description_with_patchbay_marker(Some("Human text"), issue_id);
        assert_eq!(
            patchbay_issue_id_from_description(Some(&description)),
            Some(issue_id)
        );
        assert_eq!(
            strip_patchbay_issue_marker(Some(&description)).as_deref(),
            Some("Human text")
        );

        let marker_only = description_with_patchbay_marker(None, issue_id);
        assert_eq!(
            patchbay_issue_id_from_description(Some(&marker_only)),
            Some(issue_id)
        );
        assert_eq!(strip_patchbay_issue_marker(Some(&marker_only)), None);
        assert_eq!(
            strip_patchbay_issue_marker(Some("Unmanaged text")),
            Some("Unmanaged text".to_string())
        );
    }

    #[test]
    fn webhook_freshness_uses_milliseconds_and_requires_the_sixty_second_window() {
        assert!(timestamp_is_fresh(
            1_700_000_000_000,
            1_700_000_000_000 + 60_000
        ));
        assert!(!timestamp_is_fresh(
            1_700_000_000_000,
            1_700_000_000_000 + 60_001
        ));
    }

    #[test]
    fn webhook_signature_is_over_the_raw_body() {
        let secret = "webhook-secret";
        let body = br#"{"organizationId":"org"}"#;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let signature = hex::encode(mac.finalize().into_bytes());
        assert!(verify_signature(secret, &signature, body));
        assert!(!verify_signature(
            secret,
            &signature,
            br#"{"organizationId":"other"}"#
        ));
    }

    #[test]
    fn valid_webhook_requires_and_preserves_millisecond_timestamp() {
        let secret = "webhook-secret";
        let timestamp = 1_700_000_000_000;
        let body = br#"{"id":"event-1","organizationId":"org-1","webhookId":"webhook-1","webhookTimestamp":1700000000000,"type":"Issue"}"#;
        let headers = signed_headers(secret, body, None);
        let webhook = validate_webhook(Some(secret), &headers, body, timestamp + 1).unwrap();
        assert_eq!(webhook.organization_id, "org-1");
        assert_eq!(webhook.webhook_id, "webhook-1");
        assert_eq!(webhook.delivery_id, "event-1");
        assert_eq!(webhook.event_type, "Issue");
    }

    #[test]
    fn webhook_delivery_header_is_the_idempotency_key_when_present() {
        let secret = "webhook-secret";
        let timestamp = 1_700_000_000_000;
        let body = br#"{"id":"issue-1","organizationId":"org-1","webhookId":"webhook-1","webhookTimestamp":1700000000000}"#;
        let mut headers = signed_headers(secret, body, None);
        headers.insert("linear-delivery", HeaderValue::from_static("delivery-1"));
        let webhook = validate_webhook(Some(secret), &headers, body, timestamp).unwrap();
        assert_eq!(webhook.delivery_id, "delivery-1");
    }

    #[test]
    fn webhook_timestamp_header_must_match_the_signed_body() {
        let secret = "webhook-secret";
        let timestamp = 1_700_000_000_000;
        let body = br#"{"organizationId":"org-1","webhookId":"webhook-1","webhookTimestamp":1700000000000}"#;
        let headers = signed_headers(secret, body, Some(timestamp + 1));
        assert_eq!(
            validate_webhook(Some(secret), &headers, body, timestamp),
            Err(WebhookValidationError::TimestampMismatch)
        );
    }

    #[test]
    fn webhook_without_timestamp_is_rejected_even_with_a_valid_signature() {
        let secret = "webhook-secret";
        let body = br#"{"organizationId":"org-1","webhookId":"webhook-1"}"#;
        let headers = signed_headers(secret, body, None);
        assert_eq!(
            validate_webhook(Some(secret), &headers, body, 1_700_000_000_000),
            Err(WebhookValidationError::MissingTimestamp)
        );
    }

    #[test]
    fn body_timestamp_is_authoritative_without_provider_timestamp_header() {
        let secret = "webhook-secret";
        let timestamp = 1_700_000_000_000;
        let body = br#"{"organizationId":"org-1","webhookId":"webhook-1","webhookTimestamp":1700000000000}"#;
        let headers = signed_headers(secret, body, None);
        let webhook = validate_webhook(Some(secret), &headers, body, timestamp).unwrap();
        assert_eq!(webhook.delivery_id, sha256_hex_bytes(body));
    }

    #[test]
    fn expired_webhook_is_rejected_after_sixty_seconds() {
        let secret = "webhook-secret";
        let timestamp = 1_700_000_000_000;
        let body = br#"{"organizationId":"org-1","webhookId":"webhook-1","webhookTimestamp":1700000000000}"#;
        let headers = signed_headers(secret, body, None);
        assert_eq!(
            validate_webhook(Some(secret), &headers, body, timestamp + 60_001),
            Err(WebhookValidationError::ExpiredTimestamp)
        );
    }
    #[test]
    fn webhook_timestamp_header_must_match_the_signed_body() {
        let secret = "webhook-secret";
        let timestamp = 1_700_000_000_000;
        let body = br#"{"organizationId":"org-1","webhookId":"webhook-1","webhookTimestamp":1700000000000}"#;
        let headers = signed_headers(secret, body, Some(timestamp + 1));
        assert_eq!(
            validate_webhook(Some(secret), &headers, body, timestamp),
            Err(WebhookValidationError::TimestampMismatch)
        );
    }

    #[test]
    fn catalog_label_mapping_preserves_group_and_team_identity() {
        let response = LinearCatalogLabelResponse::from(LinearCatalogLabel {
            id: "agent-a".to_string(),
            name: "Agent A".to_string(),
            color: "#6E56CF".to_string(),
            is_group: false,
            parent: Some(LinearCatalogLabelParent {
                id: "patchbay-agent-group".to_string(),
            }),
            team: Some(LinearCatalogLabelTeam {
                id: "team-1".to_string(),
            }),
        });

        assert_eq!(response.parent_id.as_deref(), Some("patchbay-agent-group"));
        assert_eq!(response.team_id.as_deref(), Some("team-1"));
        assert!(!response.is_group);
    }
}
