//! Workspace project read handlers.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::models::{Project, ProjectResource};
use cordy_db::queries::{project, project_resource};
use cordy_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const SEARCH_STATEMENT_TIMEOUT_MS: i64 = 3_000;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/projects/search", get(search))
        .route("/api/projects", get(list))
        .route("/api/projects/", get(list))
        .route(
            "/api/projects/{id}",
            get(get_one).put(update).delete(remove),
        )
        .route(
            "/api/projects/{id}/",
            get(get_one).put(update).delete(remove),
        )
        .route("/api/projects/{id}/resources", get(list_resources))
        .route("/api/projects/{id}/resources/", get(list_resources))
        .route(
            "/api/projects/{id}/resources/{resource_id}",
            axum::routing::delete(remove_resource),
        )
        .route(
            "/api/projects/{id}/resources/{resource_id}/",
            axum::routing::delete(remove_resource),
        )
}

#[derive(Debug, Serialize)]
struct ProjectResponse {
    id: String,
    workspace_id: String,
    title: String,
    description: Option<String>,
    icon: Option<String>,
    status: String,
    priority: String,
    lead_type: Option<String>,
    lead_id: Option<String>,
    start_date: Option<String>,
    due_date: Option<String>,
    created_at: String,
    updated_at: String,
    issue_count: i64,
    done_count: i64,
    resource_count: i64,
}

impl From<Project> for ProjectResponse {
    fn from(project: Project) -> Self {
        Self {
            id: project.id.to_string(),
            workspace_id: project.workspace_id.to_string(),
            title: project.title,
            description: project.description,
            icon: project.icon,
            status: project.status,
            priority: project.priority,
            lead_type: project.lead_type,
            lead_id: project.lead_id.map(|id| id.to_string()),
            start_date: project
                .start_date
                .map(|date| date.format("%Y-%m-%d").to_string()),
            due_date: project
                .due_date
                .map(|date| date.format("%Y-%m-%d").to_string()),
            created_at: crate::timefmt::rfc3339(project.created_at),
            updated_at: crate::timefmt::rfc3339(project.updated_at),
            issue_count: 0,
            done_count: 0,
            resource_count: 0,
        }
    }
}

#[derive(Debug, Serialize)]
struct ProjectResourceResponse {
    id: String,
    project_id: String,
    workspace_id: String,
    resource_type: String,
    resource_ref: Value,
    label: Option<String>,
    position: i32,
    created_at: String,
    created_by: Option<String>,
}

impl From<ProjectResource> for ProjectResourceResponse {
    fn from(resource: ProjectResource) -> Self {
        Self {
            id: resource.id.to_string(),
            project_id: resource.project_id.to_string(),
            workspace_id: resource.workspace_id.to_string(),
            resource_type: resource.resource_type,
            resource_ref: resource.resource_ref,
            label: resource.label,
            position: resource.position,
            created_at: crate::timefmt::rfc3339(resource.created_at),
            created_by: resource.created_by.map(|id| id.to_string()),
        }
    }
}

#[derive(Default, Deserialize)]
struct ListQuery {
    status: Option<String>,
    priority: Option<String>,
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(query): Query<ListQuery>,
) -> Response {
    let projects = match project::list_projects(
        &state.pool,
        context.member.workspace_id,
        query.status.as_deref().filter(|value| !value.is_empty()),
        query.priority.as_deref().filter(|value| !value.is_empty()),
    )
    .await
    {
        Ok(projects) => projects,
        Err(error) => {
            tracing::warn!(%error, workspace_id = %context.workspace_id, "failed to list projects");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list projects");
        }
    };
    let ids = projects
        .iter()
        .map(|project| project.id)
        .collect::<Vec<_>>();
    let (stats, counts) = project_enrichment(&state, &ids).await;
    let response = projects
        .into_iter()
        .map(|project| enrich(ProjectResponse::from(project), &stats, &counts))
        .collect::<Vec<_>>();
    Json(serde_json::json!({ "projects": response, "total": response.len() })).into_response()
}

async fn get_one(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let id = match Uuid::parse_str(raw_id.trim()) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid project id"),
    };
    let found =
        match project::get_project_in_workspace(&state.pool, id, context.member.workspace_id).await
        {
            Ok(Some(project)) => project,
            Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "project not found"),
        };
    let (stats, counts) = project_enrichment(&state, &[found.id]).await;
    Json(enrich(ProjectResponse::from(found), &stats, &counts)).into_response()
}

const PROJECT_STATUSES: &[&str] = &["planned", "in_progress", "paused", "completed", "cancelled"];
const PROJECT_PRIORITIES: &[&str] = &["urgent", "high", "medium", "low", "none"];

#[derive(Default, Deserialize)]
struct UpdateRequest {
    title: Option<String>,
    description: Option<String>,
    icon: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    lead_type: Option<String>,
    lead_id: Option<String>,
    start_date: Option<String>,
    due_date: Option<String>,
}

fn decode_update(body: &[u8]) -> Result<(UpdateRequest, Map<String, Value>), ()> {
    let value = serde_json::from_slice::<Value>(body).map_err(|_| ())?;
    match value {
        Value::Object(fields) => {
            let request = serde_json::from_value(Value::Object(fields.clone())).map_err(|_| ())?;
            Ok((request, fields))
        }
        Value::Null => Ok((UpdateRequest::default(), Map::new())),
        _ => Err(()),
    }
}

fn validate_enum(field: &str, value: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "invalid {field} {value:?}; valid values: {}",
            allowed.join(", ")
        ))
    }
}

fn calendar_date(value: &str, field: &str) -> Result<Option<chrono::NaiveDate>, String> {
    if value.is_empty() {
        return Ok(None);
    }
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(Some)
        .map_err(|_| format!("invalid {field} format, expected YYYY-MM-DD"))
}

fn check_violation(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23514")
}

fn publish_project(
    state: &HandlerState,
    context: &WorkspaceContext,
    event_type: &str,
    payload: Value,
) {
    state.bus.publish(&cordy_events::Event {
        event_type: event_type.into(),
        workspace_id: context.member.workspace_id.to_string(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload,
        ..Default::default()
    });
}

async fn update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    body: Bytes,
) -> Response {
    let id = match Uuid::parse_str(raw_id.trim()) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid project id"),
    };
    let existing =
        match project::get_project_in_workspace(&state.pool, id, context.member.workspace_id).await
        {
            Ok(Some(project)) => project,
            Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "project not found"),
        };
    let (request, fields) = match decode_update(&body) {
        Ok(decoded) => decoded,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if let Some(status) = request.status.as_deref() {
        if let Err(message) = validate_enum("status", status, PROJECT_STATUSES) {
            return error_response(StatusCode::BAD_REQUEST, &message);
        }
    }
    if let Some(priority) = request.priority.as_deref() {
        if let Err(message) = validate_enum("priority", priority, PROJECT_PRIORITIES) {
            return error_response(StatusCode::BAD_REQUEST, &message);
        }
    }
    let description = if fields.contains_key("description") {
        request.description.as_deref()
    } else {
        existing.description.as_deref()
    };
    let icon = if fields.contains_key("icon") {
        request.icon.as_deref()
    } else {
        existing.icon.as_deref()
    };
    let lead_type = if fields.contains_key("lead_type") {
        request.lead_type.as_deref()
    } else {
        existing.lead_type.as_deref()
    };
    let lead_id = if fields.contains_key("lead_id") {
        match request.lead_id.as_deref() {
            Some(raw) => match Uuid::parse_str(raw) {
                Ok(id) => Some(id),
                Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid lead_id"),
            },
            None => None,
        }
    } else {
        existing.lead_id
    };
    let start_date = if fields.contains_key("start_date") {
        match request.start_date.as_deref() {
            Some(value) => match calendar_date(value, "start_date") {
                Ok(date) => date,
                Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
            },
            None => None,
        }
    } else {
        existing.start_date
    };
    let due_date = if fields.contains_key("due_date") {
        match request.due_date.as_deref() {
            Some(value) => match calendar_date(value, "due_date") {
                Ok(date) => date,
                Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
            },
            None => None,
        }
    } else {
        existing.due_date
    };
    let updated = match project::update_project(
        &state.pool,
        existing.id,
        context.member.workspace_id,
        request.title.as_deref(),
        description,
        icon,
        request.status.as_deref(),
        request.priority.as_deref(),
        lead_type,
        lead_id,
        start_date,
        due_date,
    )
    .await
    {
        Ok(Some(project)) => project,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "project not found"),
        Err(error) if check_violation(&error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "project update rejected: a field value failed a database constraint",
            )
        }
        Err(error) => {
            tracing::warn!(%error, %id, "project update failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update project",
            );
        }
    };
    let (stats, counts) = project_enrichment(&state, &[updated.id]).await;
    let response = enrich(ProjectResponse::from(updated), &stats, &counts);
    publish_project(
        &state,
        &context,
        cordy_protocol::EVENT_PROJECT_UPDATED,
        json!({ "project": &response }),
    );
    Json(response).into_response()
}

async fn remove(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let id = match Uuid::parse_str(raw_id.trim()) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid project id"),
    };
    match project::get_project_in_workspace(&state.pool, id, context.member.workspace_id).await {
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "project not found"),
    }
    if !matches!(context.member.role.as_str(), "owner" | "admin") {
        return error_response(StatusCode::FORBIDDEN, "insufficient workspace role");
    }
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, %id, "failed to begin project delete");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to start transaction",
            );
        }
    };
    match project::lock_project_for_delete(&mut *transaction, id, context.member.workspace_id).await
    {
        Ok(Some(_)) => {}
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "project not found"),
        Err(error) => {
            tracing::warn!(%error, %id, "failed to lock project");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to lock project");
        }
    }
    if let Err(error) = cordy_db::queries::chat::clear_chat_session_project_by_project(
        &mut *transaction,
        id,
        context.member.workspace_id,
    )
    .await
    {
        tracing::warn!(%error, %id, "failed to clear project chat context");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to clear project chat context",
        );
    }
    if let Err(error) = cordy_db::queries::issue_view::delete_issue_views_by_project_scope(
        &mut *transaction,
        context.member.workspace_id,
        id,
    )
    .await
    {
        tracing::warn!(%error, %id, "failed to delete project views");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to delete project views",
        );
    }
    match project::delete_project(&mut *transaction, id, context.member.workspace_id).await {
        Ok(1) => {}
        Ok(_) => return error_response(StatusCode::NOT_FOUND, "project not found"),
        Err(error) => {
            tracing::warn!(%error, %id, "failed to delete project");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete project",
            );
        }
    }
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, %id, "failed to commit project delete");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to commit project delete",
        );
    }
    publish_project(
        &state,
        &context,
        cordy_protocol::EVENT_PROJECT_DELETED,
        json!({ "project_id": id.to_string() }),
    );
    StatusCode::NO_CONTENT.into_response()
}

async fn load_project_for_resource(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_id: &str,
) -> Result<Project, Response> {
    let id = Uuid::parse_str(raw_id.trim())
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid project id"))?;
    match project::get_project_in_workspace(&state.pool, id, context.member.workspace_id).await {
        Ok(Some(project)) => Ok(project),
        Ok(None) | Err(_) => Err(error_response(StatusCode::NOT_FOUND, "project not found")),
    }
}

async fn list_resources(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let project = match load_project_for_resource(&state, &context, &raw_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let resources = match project_resource::list_project_resources(&state.pool, project.id).await {
        Ok(resources) => resources,
        Err(error) => {
            tracing::warn!(%error, project_id = %project.id, "failed to list project resources");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list project resources",
            );
        }
    };
    let response = resources
        .into_iter()
        .map(ProjectResourceResponse::from)
        .collect::<Vec<_>>();
    Json(json!({ "resources": response, "total": response.len() })).into_response()
}

async fn remove_resource(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_project_id, raw_resource_id)): Path<(String, String)>,
) -> Response {
    let project = match load_project_for_resource(&state, &context, &raw_project_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let resource_id = match Uuid::parse_str(raw_resource_id.trim()) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid resource id"),
    };
    let resource = match project_resource::get_project_resource_in_workspace(
        &state.pool,
        resource_id,
        context.member.workspace_id,
    )
    .await
    {
        Ok(Some(resource)) if resource.project_id == project.id => resource,
        Ok(Some(_)) | Ok(None) | Err(_) => {
            return error_response(StatusCode::NOT_FOUND, "project resource not found")
        }
    };
    match project_resource::delete_project_resource(&state.pool, resource.id).await {
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, %resource_id, "failed to delete project resource");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete project resource",
            );
        }
    }
    publish_project(
        &state,
        &context,
        cordy_protocol::EVENT_PROJECT_RESOURCE_DELETED,
        json!({
            "project_id": project.id.to_string(),
            "resource_id": resource.id.to_string(),
        }),
    );
    StatusCode::NO_CONTENT.into_response()
}

async fn project_enrichment(
    state: &HandlerState,
    ids: &[Uuid],
) -> (HashMap<Uuid, (i64, i64)>, HashMap<Uuid, i64>) {
    if ids.is_empty() {
        return (HashMap::new(), HashMap::new());
    }
    let stats = project::get_project_issue_stats(&state.pool, ids.to_vec())
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            row.project_id
                .map(|id| (id, (row.total_count, row.done_count)))
        })
        .collect();
    let counts = project_resource::get_project_resource_counts(&state.pool, ids.to_vec())
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| row.project_id.map(|id| (id, row.resource_count)))
        .collect();
    (stats, counts)
}

fn enrich(
    mut response: ProjectResponse,
    stats: &HashMap<Uuid, (i64, i64)>,
    counts: &HashMap<Uuid, i64>,
) -> ProjectResponse {
    let id = Uuid::parse_str(&response.id).ok();
    if let Some((total, done)) = id.and_then(|id| stats.get(&id)) {
        response.issue_count = *total;
        response.done_count = *done;
    }
    response.resource_count = id.and_then(|id| counts.get(&id).copied()).unwrap_or(0);
    response
}

#[derive(Default, Deserialize)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
    include_closed: Option<String>,
}

#[derive(Serialize)]
struct SearchProjectResponse {
    #[serde(flatten)]
    project: ProjectResponse,
    match_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    matched_snippet: Option<String>,
}

async fn search(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(query): Query<SearchQuery>,
) -> Response {
    let phrase = match query.q {
        Some(phrase) if !phrase.is_empty() => phrase,
        _ => return error_response(StatusCode::BAD_REQUEST, "q parameter is required"),
    };
    let limit = parse_positive(&query.limit, 20).min(50);
    let offset = parse_non_negative(&query.offset, 0);
    let include_closed = query.include_closed.as_deref() == Some("true");
    let escaped_phrase = escape_like(&simple_lowercase(&phrase));
    let terms = phrase
        .split_whitespace()
        .map(|term| escape_like(&simple_lowercase(term)))
        .collect::<Vec<_>>();

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "failed to begin project search");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to search projects",
            );
        }
    };
    if sqlx::query(&format!(
        "SET LOCAL statement_timeout = {SEARCH_STATEMENT_TIMEOUT_MS}"
    ))
    .execute(&mut *transaction)
    .await
    .is_err()
        || sqlx::query("SET LOCAL transaction_read_only = on")
            .execute(&mut *transaction)
            .await
            .is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to search projects",
        );
    }
    let rows = match project::search_projects(
        &mut *transaction,
        context.member.workspace_id,
        &escaped_phrase,
        &terms,
        include_closed,
        limit,
        offset,
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) if statement_timeout(&error) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "search timed out; please refine your query or try again",
            )
        }
        Err(error) => {
            tracing::warn!(%error, workspace_id = %context.workspace_id, query = %phrase, "search projects failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to search projects",
            );
        }
    };
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, "failed to commit project search");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to search projects",
        );
    }

    let total = rows.first().map(|row| row.total_count).unwrap_or_default();
    let ids = rows.iter().map(|row| row.project.id).collect::<Vec<_>>();
    let (stats, counts) = project_enrichment(&state, &ids).await;
    let response = rows
        .into_iter()
        .map(|row| {
            let matched_snippet = (row.match_source == "description")
                .then_some(row.project.description.as_deref())
                .flatten()
                .filter(|description| !description.is_empty())
                .map(|description| extract_snippet(description, &phrase));
            SearchProjectResponse {
                project: enrich(ProjectResponse::from(row.project), &stats, &counts),
                match_source: row.match_source,
                matched_snippet,
            }
        })
        .collect::<Vec<_>>();
    let mut result =
        Json(serde_json::json!({ "projects": response, "total": total })).into_response();
    if let Ok(value) = HeaderValue::from_str(&total.to_string()) {
        result.headers_mut().insert("x-total-count", value);
    }
    result
}

fn parse_positive(value: &Option<String>, default: i64) -> i64 {
    value
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_non_negative(value: &Option<String>, default: i64) -> i64 {
    value
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(default)
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn simple_lower_chars(value: &str) -> Vec<char> {
    value
        .chars()
        .map(|character| character.to_lowercase().next().unwrap_or(character))
        .collect()
}

fn simple_lowercase(value: &str) -> String {
    simple_lower_chars(value).into_iter().collect()
}

fn statement_timeout(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(|error| error.as_database_error())
        .and_then(|error| error.code())
        .is_some_and(|code| code == "57014")
}

fn extract_snippet(content: &str, query: &str) -> String {
    let content_chars = content.chars().collect::<Vec<_>>();
    let lower_chars = simple_lower_chars(content);
    let query_chars = simple_lower_chars(query);
    let mut found = find_chars(&lower_chars, &query_chars).map(|index| (index, query_chars.len()));
    if found.is_none() {
        found = query
            .split_whitespace()
            .filter_map(|term| {
                let chars = simple_lower_chars(term);
                find_chars(&lower_chars, &chars).map(|index| (index, chars.len()))
            })
            .min_by_key(|(index, _)| *index);
    }
    let Some((index, match_len)) = found else {
        return if content_chars.len() > 120 {
            format!("{}...", content_chars[..120].iter().collect::<String>())
        } else {
            content.to_string()
        };
    };
    let start = index.saturating_sub(40);
    let end = (index + match_len + 80).min(content_chars.len());
    let mut snippet = content_chars[start..end].iter().collect::<String>();
    if start > 0 {
        snippet.insert_str(0, "...");
    }
    if end < content_chars.len() {
        snippet.push_str("...");
    }
    snippet
}

fn find_chars(haystack: &[char], needle: &[char]) -> Option<usize> {
    (!needle.is_empty() && needle.len() <= haystack.len())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_wire_uses_calendar_dates_and_nullable_fields() {
        let now = chrono::Utc::now();
        let response = ProjectResponse::from(Project {
            id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            title: "Migration".into(),
            description: None,
            icon: None,
            status: "planned".into(),
            priority: "none".into(),
            lead_type: None,
            lead_id: None,
            start_date: chrono::NaiveDate::from_ymd_opt(2026, 8, 23),
            due_date: None,
            created_at: now,
            updated_at: now,
        });
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["start_date"], "2026-08-23");
        assert_eq!(value["description"], serde_json::Value::Null);
        assert_eq!(value["issue_count"], 0);
    }

    #[test]
    fn project_resource_wire_preserves_json_and_nullable_creator() {
        let now = chrono::Utc::now();
        let response = ProjectResourceResponse::from(ProjectResource {
            id: Uuid::nil(),
            project_id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            resource_type: "github_repo".into(),
            resource_ref: json!({"url": "git@github.com:alexj11324/Cordy.git"}),
            label: None,
            position: 2,
            created_at: now,
            created_by: None,
        });
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(
            value["resource_ref"]["url"],
            "git@github.com:alexj11324/Cordy.git"
        );
        assert_eq!(value["created_by"], Value::Null);
        assert_eq!(value["position"], 2);
    }

    #[test]
    fn search_parsing_matches_go_defaults_and_caps() {
        assert_eq!(parse_positive(&None, 20), 20);
        assert_eq!(parse_positive(&Some("0".into()), 20), 20);
        assert_eq!(parse_positive(&Some("75".into()), 20).min(50), 50);
        assert_eq!(parse_non_negative(&Some("-1".into()), 0), 0);
    }

    #[test]
    fn update_decoder_preserves_nullable_field_presence() {
        let (request, fields) = decode_update(
            br#"{"description":null,"icon":"rocket","start_date":"","unknown":true}"#,
        )
        .unwrap();
        assert!(fields.contains_key("description"));
        assert!(request.description.is_none());
        assert_eq!(request.icon.as_deref(), Some("rocket"));
        assert_eq!(request.start_date.as_deref(), Some(""));
        assert!(decode_update(br#"{"status":"planned"} trailing"#).is_err());
        let (request, fields) = decode_update(b"null").unwrap();
        assert!(fields.is_empty());
        assert!(request.title.is_none());
        assert!(decode_update(b"[]").is_err());
    }

    #[test]
    fn project_update_validation_matches_go_contract() {
        assert!(validate_enum("status", "in_progress", PROJECT_STATUSES).is_ok());
        assert!(validate_enum("status", "active", PROJECT_STATUSES)
            .unwrap_err()
            .contains("planned, in_progress, paused, completed, cancelled"));
        assert_eq!(
            calendar_date("2026-08-23", "start_date").unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 8, 23)
        );
        assert_eq!(calendar_date("", "due_date").unwrap(), None);
        assert_eq!(
            calendar_date("08/23/2026", "due_date").unwrap_err(),
            "invalid due_date format, expected YYYY-MM-DD"
        );
    }

    #[test]
    fn snippet_preserves_unicode_and_falls_back_to_terms() {
        let content =
            "这是一段很长的中文内容，包含了搜索关键词测试用例，用来验证多字节字符不会被截断";
        assert!(extract_snippet(content, "搜索关键词").contains("搜索关键词"));
        assert!(
            extract_snippet("deploy now, kubernetes later", "deploy kubernetes").contains("deploy")
        );
    }

    #[test]
    fn snippet_indices_remain_aligned_for_expanding_unicode_lowercase() {
        let content = format!("{}x", "İ".repeat(100));
        let snippet = extract_snippet(&content, "x");
        assert!(snippet.contains('x'));
        assert!(snippet.chars().count() <= 124);
    }

    #[test]
    fn like_escaping_matches_go() {
        assert_eq!(escape_like(r"a%b_c\d"), r"a\%b\_c\\d");
    }
}
