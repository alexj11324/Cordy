//! Workspace issue-status catalog handlers.

use std::collections::HashSet;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::{Json, Router};
use cordy_db::queries::issue_status as status_q;
use cordy_middleware::workspace::WorkspaceContext;
use cordy_service::issue_status;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const CATEGORIES: [&str; 7] = [
    "backlog",
    "todo",
    "in_progress",
    "in_review",
    "done",
    "blocked",
    "cancelled",
];

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/issue-statuses", get(list).post(create))
        .route("/api/issue-statuses/", get(list).post(create))
        .route("/api/issue-statuses/reorder", patch(reorder))
        .route("/api/issue-statuses/{id}", patch(update).delete(archive))
        .route("/api/issue-statuses/{id}/", patch(update).delete(archive))
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "workspace not found"))
}

fn require_admin(context: &WorkspaceContext) -> Result<(), Response> {
    match context.member.role.as_str() {
        "owner" | "admin" => Ok(()),
        _ => Err(error_response(
            StatusCode::FORBIDDEN,
            "insufficient permissions",
        )),
    }
}

fn db_error(error: anyhow::Error, message: &'static str) -> Response {
    tracing::warn!(%error, "{message}");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, message)
}

fn unique_violation(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(|e| e.as_database_error())
        .and_then(|e| e.code())
        .is_some_and(|code| code == "23505")
}

fn normalize_color(raw: &str) -> Result<String, Response> {
    let value = raw.trim();
    if value.len() != 7
        || !value.starts_with('#')
        || !value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "color must be a hex color like #3b82f6",
        ));
    }
    Ok(value.to_ascii_lowercase())
}

fn publish(state: &HandlerState, context: &WorkspaceContext, action: &str) {
    state.bus.publish(&cordy_events::Event {
        event_type: "issue_status:changed".into(),
        workspace_id: context.workspace_id.clone(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload: json!({ "action": action }),
        ..Default::default()
    });
}

#[derive(Default, Deserialize)]
struct ListQuery {
    include_archived: Option<bool>,
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(query): Query<ListQuery>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    if let Err(error) = issue_status::ensure(&state.pool, workspace_id).await {
        tracing::warn!(%error, "failed to ensure issue status catalog");
    }
    match status_q::list_issue_status_entries(
        &state.pool,
        workspace_id,
        query.include_archived.unwrap_or(false),
    )
    .await
    {
        Ok(statuses) => Json(json!({
            "total": statuses.len(),
            "statuses": statuses,
            "categories": CATEGORIES,
        }))
        .into_response(),
        Err(error) => db_error(error, "failed to list issue statuses"),
    }
}

#[derive(Deserialize)]
struct CreateRequest {
    #[serde(default)]
    key: String,
    name: String,
    #[serde(default)]
    description: String,
    category: String,
    color: String,
}

async fn create(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<CreateRequest>,
) -> Response {
    if let Err(response) = require_admin(&context) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return error_response(StatusCode::BAD_REQUEST, "name must be 1-64 characters");
    }
    if request.description.chars().count() > 256 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "description must be at most 256 characters",
        );
    }
    if !issue_status::is_category(&request.category) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "category must be one of: backlog, todo, in_progress, in_review, done, blocked, cancelled",
        );
    }
    let color = match normalize_color(&request.color) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let key = if request.key.trim().is_empty() {
        issue_status::slugify_key(name)
    } else {
        issue_status::validate_key(&request.key)
    };
    let key = match key {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    match status_q::create_issue_status_entry(
        &state.pool,
        workspace_id,
        &key,
        name,
        &request.description,
        &request.category,
        &color,
    )
    .await
    {
        Ok(Some(status)) => {
            publish(&state, &context, "created");
            (StatusCode::CREATED, Json(status)).into_response()
        }
        Ok(None) => db_error(
            anyhow::anyhow!("missing returned row"),
            "failed to create issue status",
        ),
        Err(error) if unique_violation(&error) => error_response(
            StatusCode::CONFLICT,
            "a status with this key or name already exists",
        ),
        Err(error) => db_error(error, "failed to create issue status"),
    }
}

#[derive(Deserialize)]
struct UpdateRequest {
    name: Option<String>,
    description: Option<String>,
    color: Option<String>,
    position: Option<f64>,
}

async fn update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    Json(request): Json<UpdateRequest>,
) -> Response {
    if let Err(response) = require_admin(&context) {
        return response;
    }
    let Ok(id) = Uuid::parse_str(&id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid issue status id");
    };
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let Some(current) =
        (match status_q::get_issue_status_entry_by_id(&state.pool, id, workspace_id).await {
            Ok(value) => value,
            Err(error) => return db_error(error, "failed to load issue status"),
        })
    else {
        return error_response(StatusCode::NOT_FOUND, "issue status not found");
    };
    if current.is_system {
        return error_response(
            StatusCode::FORBIDDEN,
            "built-in statuses cannot be modified",
        );
    }
    if current.archived_at.is_some() {
        return error_response(StatusCode::CONFLICT, "archived statuses cannot be modified");
    }
    let name = match request.name {
        Some(value) if value.trim().is_empty() || value.trim().chars().count() > 64 => {
            return error_response(StatusCode::BAD_REQUEST, "name must be 1-64 characters");
        }
        Some(value) => Some(value.trim().to_string()),
        None => None,
    };
    if request
        .description
        .as_ref()
        .is_some_and(|v| v.chars().count() > 256)
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "description must be at most 256 characters",
        );
    }
    let color = match request.color.as_deref().map(normalize_color).transpose() {
        Ok(value) => value,
        Err(response) => return response,
    };
    match status_q::update_issue_status_entry(
        &state.pool,
        name.as_deref(),
        request.description.as_deref(),
        color.as_deref(),
        request.position,
        id,
        workspace_id,
    )
    .await
    {
        Ok(Some(status)) => {
            publish(&state, &context, "updated");
            Json(status).into_response()
        }
        Ok(None) => error_response(StatusCode::NOT_FOUND, "issue status not found"),
        Err(error) if unique_violation(&error) => error_response(
            StatusCode::CONFLICT,
            "a status with this name already exists",
        ),
        Err(error) => db_error(error, "failed to update issue status"),
    }
}

async fn archive(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = require_admin(&context) {
        return response;
    }
    let Ok(id) = Uuid::parse_str(&id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid issue status id");
    };
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let Some(current) =
        (match status_q::get_issue_status_entry_by_id(&state.pool, id, workspace_id).await {
            Ok(value) => value,
            Err(error) => return db_error(error, "failed to load issue status"),
        })
    else {
        return error_response(StatusCode::NOT_FOUND, "issue status not found");
    };
    if current.is_system {
        return error_response(
            StatusCode::FORBIDDEN,
            "built-in statuses cannot be archived",
        );
    }
    if current.archived_at.is_some() {
        return Json(current).into_response();
    }
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(error) => return db_error(error.into(), "failed to archive issue status"),
    };
    let result = async {
        status_q::lock_issue_status_catalog(&mut *transaction, workspace_id).await?;
        let archived =
            status_q::archive_issue_status_entry(&mut *transaction, id, workspace_id).await?;
        transaction.commit().await?;
        anyhow::Ok(archived)
    }
    .await;
    match result {
        Ok(Some(status)) => {
            publish(&state, &context, "archived");
            Json(status).into_response()
        }
        Ok(None) => error_response(StatusCode::CONFLICT, "status is no longer archivable"),
        Err(error) => db_error(error, "failed to archive issue status"),
    }
}

#[derive(Deserialize)]
struct ReorderRequest {
    category: String,
    ids: Vec<Uuid>,
}

async fn reorder(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<ReorderRequest>,
) -> Response {
    if let Err(response) = require_admin(&context) {
        return response;
    }
    if !issue_status::is_category(&request.category) {
        return error_response(StatusCode::BAD_REQUEST, "invalid category");
    }
    if request.ids.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "ids must not be empty");
    }
    if request.ids.iter().copied().collect::<HashSet<_>>().len() != request.ids.len() {
        return error_response(StatusCode::BAD_REQUEST, "duplicate ids");
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(error) => return db_error(error.into(), "failed to reorder issue statuses"),
    };
    let result = async {
        status_q::lock_issue_status_catalog_shared(&mut *transaction, workspace_id).await?;
        let active = status_q::list_active_custom_issue_status_entries(
            &mut *transaction,
            workspace_id,
            &request.category,
        )
        .await?;
        let wanted = request.ids.iter().copied().collect::<HashSet<_>>();
        let actual = active.iter().map(|entry| entry.id).collect::<HashSet<_>>();
        if wanted != actual {
            anyhow::bail!("catalog_changed");
        }
        let affected =
            status_q::reorder_issue_status_entries(&mut *transaction, workspace_id, request.ids)
                .await?;
        if affected as usize != active.len() {
            anyhow::bail!("catalog_changed");
        }
        let statuses =
            status_q::list_issue_status_entries(&mut *transaction, workspace_id, true).await?;
        transaction.commit().await?;
        anyhow::Ok(statuses)
    }
    .await;
    match result {
        Ok(statuses) => {
            publish(&state, &context, "reordered");
            Json(json!({ "total": statuses.len(), "statuses": statuses, "categories": CATEGORIES }))
                .into_response()
        }
        Err(error) if error.to_string() == "catalog_changed" => error_response(
            StatusCode::CONFLICT,
            "ids must name every active custom status in the category",
        ),
        Err(error) => db_error(error, "failed to reorder issue statuses"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_colors() {
        assert_eq!(normalize_color("#3B82F6").unwrap(), "#3b82f6");
        assert!(normalize_color("blue").is_err());
    }
}
