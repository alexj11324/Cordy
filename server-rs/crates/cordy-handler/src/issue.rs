//! Core issue routes — S8 authenticated issue-domain slice.
//!
//! Ports the stable list/query, detail, create, children, and label-read
//! contracts from `server/internal/handler/issue.go` and `label.go`. The
//! workspace middleware resolves slugs/ids, verifies membership, and stamps a
//! `WorkspaceContext` before these handlers run.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{NaiveDate, SecondsFormat};
use cordy_db::models::{Issue, IssueLabel};
use cordy_db::queries::{issue as issue_q, issue_label, member, workspace};
use cordy_middleware::workspace::{WorkspaceContext, WorkspaceGuardState};
use cordy_service::issue_service::{IssueCreateError, IssueCreateOpts, IssueCreateParams};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const PRIORITIES: &[&str] = &["urgent", "high", "medium", "low", "none"];

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/issues", get(list_issues).post(create_issue))
        .route("/api/issues/", get(list_issues).post(create_issue))
        .route("/api/issues/query", post(query_issues))
        .route("/api/issues/{id}", get(get_issue))
        .route("/api/issues/{id}/", get(get_issue))
        .route("/api/issues/{id}/children", get(list_child_issues))
        .route("/api/issues/{id}/labels", get(list_labels_for_issue))
}

/// Workspace guard for the issue group. Kept here because this slice needs a
/// JSON `Response` on every failure path; it uses the shared resolver and the
/// same `WorkspaceContext` type as `cordy-middleware`.
pub async fn require_issue_workspace(
    State(state): State<WorkspaceGuardState>,
    mut request: Request,
    next: Next,
) -> Response {
    let actor_source = header_owned(&request, "x-actor-source");
    let workspace_header = header_owned(&request, "x-workspace-id");
    let slug = header_owned(&request, "x-workspace-slug")
        .or_else(|| query_owned(&request, "workspace_slug"));
    let workspace_query = query_owned(&request, "workspace_id");
    let user_id =
        header_owned(&request, "x-user-id").and_then(|value| Uuid::parse_str(&value).ok());

    let raw_workspace_id = if actor_source.as_deref() == Some("task_token") {
        workspace_header
    } else if let Some(slug) = slug {
        workspace::get_workspace_by_slug(&state.pool, &slug)
            .await
            .ok()
            .flatten()
            .map(|workspace| workspace.id.to_string())
    } else {
        workspace_header.or(workspace_query)
    };
    let Some(raw_workspace_id) = raw_workspace_id else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "workspace_id or workspace_slug is required",
        );
    };
    let Ok(workspace_id) = Uuid::parse_str(&raw_workspace_id) else {
        return error_response(StatusCode::NOT_FOUND, "workspace not found");
    };
    let Some(user_id) = user_id else {
        return error_response(StatusCode::UNAUTHORIZED, "user not authenticated");
    };
    let member =
        match member::get_member_by_user_and_workspace(&state.pool, user_id, workspace_id).await {
            Ok(Some(member)) => member,
            Ok(None) => return error_response(StatusCode::NOT_FOUND, "workspace not found"),
            Err(error) => {
                tracing::warn!(%error, %workspace_id, "workspace membership lookup failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to verify workspace",
                );
            }
        };
    request.extensions_mut().insert(WorkspaceContext {
        workspace_id: workspace_id.to_string(),
        member,
    });
    next.run(request).await
}

fn header_owned(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn query_owned(request: &Request, name: &str) -> Option<String> {
    request.uri().query()?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name && !value.is_empty()).then(|| value.to_string())
    })
}

#[derive(Debug, Default, Deserialize)]
struct ListParams {
    limit: Option<String>,
    offset: Option<String>,
    status: Option<String>,
    statuses: Option<String>,
    status_category: Option<String>,
    status_categories: Option<String>,
    priority: Option<String>,
    priorities: Option<String>,
    assignee_id: Option<String>,
    assignee_ids: Option<String>,
    assignee_types: Option<String>,
    creator_id: Option<String>,
    project_id: Option<String>,
    project_ids: Option<String>,
    ids: Option<String>,
    q: Option<String>,
    top_level_only: Option<String>,
    scheduled: Option<String>,
    open_only: Option<String>,
    sort: Option<String>,
    direction: Option<String>,
}

#[derive(Debug, FromRow)]
struct ListRow {
    acceptance_criteria: Value,
    assignee_id: Option<Uuid>,
    assignee_type: Option<String>,
    context_refs: Value,
    created_at: chrono::DateTime<chrono::Utc>,
    creator_id: Uuid,
    creator_type: String,
    description: Option<String>,
    due_date: Option<NaiveDate>,
    first_executed_at: Option<chrono::DateTime<chrono::Utc>>,
    id: Uuid,
    last_activity_at: Option<chrono::DateTime<chrono::Utc>>,
    metadata: Value,
    number: i32,
    origin_id: Option<Uuid>,
    origin_type: Option<String>,
    parent_issue_id: Option<Uuid>,
    position: f64,
    priority: String,
    project_id: Option<Uuid>,
    properties: Value,
    revision: i64,
    stage: Option<i32>,
    start_date: Option<NaiveDate>,
    status: String,
    title: String,
    updated_at: chrono::DateTime<chrono::Utc>,
    workspace_id: Uuid,
    total_count: i64,
}

impl ListRow {
    fn into_issue(self) -> Issue {
        Issue {
            acceptance_criteria: self.acceptance_criteria,
            assignee_id: self.assignee_id,
            assignee_type: self.assignee_type,
            context_refs: self.context_refs,
            created_at: self.created_at,
            creator_id: self.creator_id,
            creator_type: self.creator_type,
            description: self.description,
            due_date: self.due_date,
            first_executed_at: self.first_executed_at,
            id: self.id,
            last_activity_at: self.last_activity_at,
            metadata: self.metadata,
            number: self.number,
            origin_id: self.origin_id,
            origin_type: self.origin_type,
            parent_issue_id: self.parent_issue_id,
            position: self.position,
            priority: self.priority,
            project_id: self.project_id,
            properties: self.properties,
            revision: self.revision,
            stage: self.stage,
            start_date: self.start_date,
            status: self.status,
            title: self.title,
            updated_at: self.updated_at,
            workspace_id: self.workspace_id,
        }
    }
}

async fn list_issues(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<ListParams>,
) -> Response {
    list_issues_with_params(&state, &context, params).await
}

/// POST twin of GET /api/issues. Values intentionally stay strings so the two
/// transports share parsing and validation exactly as in Go.
async fn query_issues(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(values): Json<HashMap<String, String>>,
) -> Response {
    let value = match serde_json::to_value(values) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let params = match serde_json::from_value(value) {
        Ok(params) => params,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    list_issues_with_params(&state, &context, params).await
}

async fn list_issues_with_params(
    state: &HandlerState,
    context: &WorkspaceContext,
    params: ListParams,
) -> Response {
    let workspace_id = context.member.workspace_id;
    let limit = params
        .limit
        .as_deref()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(100)
        .min(100);
    let offset = params
        .offset
        .as_deref()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v >= 0)
        .unwrap_or(0);

    let assignee_id = match optional_uuid(params.assignee_id.as_deref(), "assignee_id") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let creator_id = match optional_uuid(params.creator_id.as_deref(), "creator_id") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let project_id = match optional_uuid(params.project_id.as_deref(), "project_id") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let assignee_ids = match uuid_list(params.assignee_ids.as_deref(), "assignee_ids") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let project_ids = match uuid_list(params.project_ids.as_deref(), "project_ids") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let ids = match uuid_list(params.ids.as_deref(), "ids") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let assignee_types = comma_list(params.assignee_types.as_deref());
    if assignee_types
        .iter()
        .any(|kind| !matches!(kind.as_str(), "member" | "agent" | "squad"))
    {
        return error_response(StatusCode::BAD_REQUEST, "invalid assignee_types");
    }

    let mut statuses = comma_list(params.statuses.as_deref().or(params.status.as_deref()));
    let categories = comma_list(
        params
            .status_categories
            .as_deref()
            .or(params.status_category.as_deref()),
    );
    if !categories.is_empty() {
        let entries = match cordy_db::queries::issue_status::list_issue_status_entries(
            &state.pool,
            workspace_id,
            true,
        )
        .await
        {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(%error, "failed to expand issue status categories");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to resolve status categories",
                );
            }
        };
        for category in &categories {
            if cordy_service::issue_status::is_built_in(category) {
                statuses.push(category.clone());
            }
            statuses.extend(
                entries
                    .iter()
                    .filter(|entry| entry.category == *category)
                    .map(|entry| entry.key.clone()),
            );
        }
        statuses.sort();
        statuses.dedup();
    }
    let priorities = comma_list(params.priorities.as_deref().or(params.priority.as_deref()));

    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT i.*, COUNT(*) OVER() AS total_count FROM issue i WHERE i.workspace_id = ",
    );
    query.push_bind(workspace_id);
    if !statuses.is_empty() {
        query
            .push(" AND i.status = ANY(")
            .push_bind(statuses)
            .push(")");
    }
    if !priorities.is_empty() {
        query
            .push(" AND i.priority = ANY(")
            .push_bind(priorities)
            .push(")");
    }
    if let Some(id) = assignee_id {
        query.push(" AND i.assignee_id = ").push_bind(id);
    }
    if !assignee_ids.is_empty() {
        query
            .push(" AND i.assignee_id = ANY(")
            .push_bind(assignee_ids)
            .push(")");
    }
    if !assignee_types.is_empty() {
        query
            .push(" AND i.assignee_type = ANY(")
            .push_bind(assignee_types)
            .push(")");
    }
    if let Some(id) = creator_id {
        query.push(" AND i.creator_id = ").push_bind(id);
    }
    if let Some(id) = project_id {
        query.push(" AND i.project_id = ").push_bind(id);
    }
    if !project_ids.is_empty() {
        query
            .push(" AND i.project_id = ANY(")
            .push_bind(project_ids)
            .push(")");
    }
    if params.ids.is_some() {
        query.push(" AND i.id = ANY(").push_bind(ids).push(")");
    }
    if params.top_level_only.as_deref() == Some("true") {
        query.push(" AND i.parent_issue_id IS NULL");
    }
    if params.scheduled.as_deref() == Some("true") {
        query.push(" AND (i.start_date IS NOT NULL OR i.due_date IS NOT NULL)");
    }
    if params.open_only.as_deref() == Some("true") {
        query.push(" AND i.status NOT IN ('done', 'cancelled')");
    }
    if let Some(term) = params.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        let pattern = format!("%{term}%");
        query
            .push(" AND (i.title ILIKE ")
            .push_bind(pattern.clone())
            .push(" OR COALESCE(i.description, '') ILIKE ")
            .push_bind(pattern)
            .push(")");
    }

    let sort = params.sort.as_deref().unwrap_or("position");
    let direction = params
        .direction
        .as_deref()
        .unwrap_or(if sort == "last_activity" {
            "desc"
        } else {
            "asc"
        });
    if !matches!(direction.to_ascii_lowercase().as_str(), "asc" | "desc") {
        return error_response(StatusCode::BAD_REQUEST, "invalid direction value");
    }
    let direction = direction.to_ascii_uppercase();
    match sort {
        "position" => query.push(" ORDER BY i.position ASC, i.created_at DESC, i.id DESC"),
        "title" | "created_at" | "updated_at" | "start_date" | "due_date" => query
            .push(" ORDER BY i.")
            .push(sort)
            .push(" ")
            .push(direction)
            .push(" NULLS LAST, i.created_at DESC, i.id DESC"),
        "last_activity" => query
            .push(" ORDER BY i.last_activity_at ")
            .push(direction)
            .push(" NULLS LAST, i.id DESC"),
        "priority" => query
            .push(" ORDER BY CASE i.priority WHEN 'urgent' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 WHEN 'low' THEN 3 ELSE 4 END ")
            .push(direction)
            .push(", i.created_at DESC, i.id DESC"),
        "status" => query
            .push(" ORDER BY CASE i.status WHEN 'backlog' THEN 0 WHEN 'todo' THEN 1 WHEN 'in_progress' THEN 2 WHEN 'in_review' THEN 3 WHEN 'done' THEN 4 WHEN 'blocked' THEN 5 WHEN 'cancelled' THEN 6 ELSE 7 END ")
            .push(direction)
            .push(", i.created_at DESC, i.id DESC"),
        _ => return error_response(StatusCode::BAD_REQUEST, "invalid sort value"),
    };
    query
        .push(" LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);

    let rows = match query
        .build_query_as::<ListRow>()
        .fetch_all(&state.pool)
        .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, "failed to list issues");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list issues");
        }
    };
    let total = rows.first().map(|row| row.total_count).unwrap_or(0);
    let issues = rows
        .into_iter()
        .map(ListRow::into_issue)
        .collect::<Vec<_>>();
    let responses = enrich_issue_list(state, context, issues).await;
    Json(json!({ "issues": responses, "total": total })).into_response()
}

async fn get_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let mut responses = enrich_issue_list(&state, &context, vec![issue]).await;
    Json(responses.remove(0)).into_response()
}

async fn list_child_issues(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    match issue_q::list_child_issues(&state.pool, issue.id).await {
        Ok(children) => {
            let issues = enrich_issue_list(&state, &context, children).await;
            Json(json!({ "issues": issues })).into_response()
        }
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to list child issues");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list child issues",
            )
        }
    }
}

async fn list_labels_for_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    match labels_for_issues(&state, issue.workspace_id, &[issue.id]).await {
        Ok(mut labels) => Json(json!({
            "labels": labels.remove(&issue.id).unwrap_or_default(),
            "issue_revision": issue.revision,
        }))
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to list issue labels");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list labels")
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreateIssueRequest {
    title: String,
    description: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    priority: String,
    assignee_type: Option<String>,
    assignee_id: Option<String>,
    parent_issue_id: Option<String>,
    project_id: Option<String>,
    stage: Option<i32>,
    start_date: Option<String>,
    due_date: Option<String>,
    #[serde(default)]
    attachment_ids: Vec<String>,
    #[serde(default)]
    label_ids: Vec<String>,
    origin_type: Option<String>,
    origin_id: Option<String>,
    #[serde(default)]
    allow_duplicate: bool,
}

async fn create_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Json(request): Json<CreateIssueRequest>,
) -> Response {
    if request.title.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "title is required");
    }
    let workspace_id = context.member.workspace_id;
    let status = if request.status.is_empty() {
        "todo".to_string()
    } else {
        request.status
    };
    let status =
        match cordy_service::issue_status::resolve(&state.pool, workspace_id, &status).await {
            Ok(entry) => entry.key,
            Err(_) => return invalid_status(&state, workspace_id, &status).await,
        };
    let priority = if request.priority.is_empty() {
        "none".to_string()
    } else {
        request.priority
    };
    if !PRIORITIES.contains(&priority.as_str()) {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!(
                "invalid priority {:?}; valid values: {}",
                priority,
                PRIORITIES.join(", ")
            ),
        );
    }
    if request.stage.is_some_and(|stage| stage < 1) {
        return error_response(StatusCode::BAD_REQUEST, "stage must be >= 1");
    }
    let assignee_id = match optional_uuid(request.assignee_id.as_deref(), "assignee_id") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    if request.assignee_type.is_some() != assignee_id.is_some() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "assignee_type and assignee_id must be provided together",
        );
    }
    if request
        .assignee_type
        .as_deref()
        .is_some_and(|kind| !matches!(kind, "member" | "agent" | "squad"))
    {
        return error_response(StatusCode::BAD_REQUEST, "invalid assignee_type");
    }
    let parent_issue_id = match optional_uuid(request.parent_issue_id.as_deref(), "parent_issue_id")
    {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let project_id = match optional_uuid(request.project_id.as_deref(), "project_id") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let start_date = match optional_date(request.start_date.as_deref(), "start_date") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let due_date = match optional_date(request.due_date.as_deref(), "due_date") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let attachment_ids = match uuid_strings(&request.attachment_ids, "attachment_ids") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let label_ids = match uuid_strings(&request.label_ids, "label_ids") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    if request.origin_type.is_some() != request.origin_id.is_some() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "origin_type and origin_id must be provided together",
        );
    }
    if request
        .origin_type
        .as_deref()
        .is_some_and(|kind| kind != "quick_create")
    {
        return error_response(StatusCode::BAD_REQUEST, "unsupported origin_type");
    }
    let origin_id = match optional_uuid(request.origin_id.as_deref(), "origin_id") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let agent_id = headers
        .get("x-agent-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok());
    let (creator_type, creator_id) = agent_id
        .map(|id| ("agent".to_string(), id))
        .unwrap_or_else(|| ("member".to_string(), context.member.id));

    let prefix = issue_prefix(&state, workspace_id).await;
    let broadcast_prefix = prefix.clone();
    let result = state
        .issues
        .create(
            IssueCreateParams {
                workspace_id,
                title: request.title,
                description: request.description,
                status,
                priority,
                assignee_type: request.assignee_type,
                assignee_id,
                creator_type,
                creator_id,
                parent_issue_id,
                project_id,
                start_date,
                due_date,
                origin_type: request.origin_type,
                origin_id,
                attachment_ids,
                label_ids,
                allow_duplicate: request.allow_duplicate,
                stage: request.stage,
            },
            IssueCreateOpts {
                actor_id: creator_id.to_string(),
                platform: headers
                    .get("x-client-platform")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string(),
                broadcast_payload: Some(Arc::new(move |issue, _, labels| {
                    let mut response = IssueResponse::from_issue(issue, &broadcast_prefix);
                    response.labels = Some(labels.iter().map(LabelResponse::from).collect());
                    json!({ "issue": response })
                })),
                ..IssueCreateOpts::default()
            },
        )
        .await;

    match result {
        Ok(result) => {
            let Some(issue) = result.issue else {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create issue",
                );
            };
            let mut response = IssueResponse::from_issue(&issue, &prefix);
            response.status_category = Some(
                cordy_service::issue_status::effective(&state.pool, workspace_id, &issue.status)
                    .await,
            );
            response.labels = Some(result.labels.iter().map(LabelResponse::from).collect());
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(IssueCreateError::ActiveDuplicate { duplicate }) => {
            let duplicate = duplicate.map(|issue| IssueResponse::from_issue(&issue, &prefix));
            (StatusCode::CONFLICT, Json(json!({
                "code": "active_duplicate_issue",
                "error": "an active duplicate issue already exists",
                "issue": duplicate,
            })))
                .into_response()
        }
        Err(IssueCreateError::ParentIssueNotFound) => error_response(
            StatusCode::BAD_REQUEST,
            "parent issue not found in this workspace",
        ),
        Err(IssueCreateError::ProjectNotFound) => error_response(
            StatusCode::BAD_REQUEST,
            "project not found in this workspace",
        ),
        Err(IssueCreateError::LabelNotFound) => error_response(
            StatusCode::BAD_REQUEST,
            "one or more labels not found in this workspace",
        ),
        Err(IssueCreateError::StatusUnavailable) => error_response(
            StatusCode::CONFLICT,
            "the target status was archived while this request was in flight; reload the status list and retry",
        ),
        Err(error) => {
            tracing::warn!(%error, %workspace_id, "failed to create issue");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to create issue")
        }
    }
}

async fn resolve_issue(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw: &str,
) -> Result<Issue, Response> {
    let workspace_id = context.member.workspace_id;
    let result = if let Ok(id) = Uuid::parse_str(raw) {
        issue_q::get_issue_in_workspace(&state.pool, id, workspace_id).await
    } else {
        let Some((prefix, number)) = raw.rsplit_once('-') else {
            return Err(error_response(StatusCode::NOT_FOUND, "issue not found"));
        };
        let expected_prefix = issue_prefix(state, workspace_id).await;
        let Ok(number) = number.parse::<i32>() else {
            return Err(error_response(StatusCode::NOT_FOUND, "issue not found"));
        };
        if !prefix.eq_ignore_ascii_case(&expected_prefix) {
            return Err(error_response(StatusCode::NOT_FOUND, "issue not found"));
        }
        issue_q::get_issue_by_number(&state.pool, workspace_id, number).await
    };
    match result {
        Ok(Some(issue)) => Ok(issue),
        Ok(None) => Err(error_response(StatusCode::NOT_FOUND, "issue not found")),
        Err(error) => {
            tracing::warn!(%error, issue = raw, "failed to load issue");
            Err(error_response(StatusCode::NOT_FOUND, "issue not found"))
        }
    }
}

async fn enrich_issue_list(
    state: &HandlerState,
    context: &WorkspaceContext,
    issues: Vec<Issue>,
) -> Vec<IssueResponse> {
    let prefix = issue_prefix(state, context.member.workspace_id).await;
    let ids = issues.iter().map(|issue| issue.id).collect::<Vec<_>>();
    let mut labels = labels_for_issues(state, context.member.workspace_id, &ids)
        .await
        .unwrap_or_default();
    let mut responses = Vec::with_capacity(issues.len());
    for issue in issues {
        let category = cordy_service::issue_status::effective(
            &state.pool,
            context.member.workspace_id,
            &issue.status,
        )
        .await;
        let mut response = IssueResponse::from_issue(&issue, &prefix);
        response.status_category = Some(category);
        response.labels = Some(labels.remove(&issue.id).unwrap_or_default());
        responses.push(response);
    }
    responses
}

async fn labels_for_issues(
    state: &HandlerState,
    workspace_id: Uuid,
    issue_ids: &[Uuid],
) -> anyhow::Result<HashMap<Uuid, Vec<LabelResponse>>> {
    if issue_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows =
        issue_label::list_labels_for_issues(&state.pool, issue_ids.to_vec(), workspace_id).await?;
    let mut labels = HashMap::<Uuid, Vec<LabelResponse>>::new();
    for row in rows {
        if let (
            Some(issue_id),
            Some(id),
            Some(label_workspace_id),
            Some(created_at),
            Some(updated_at),
        ) = (
            row.issue_id,
            row.id,
            row.workspace_id,
            row.created_at,
            row.updated_at,
        ) {
            labels.entry(issue_id).or_default().push(LabelResponse {
                id: id.to_string(),
                workspace_id: label_workspace_id.to_string(),
                resource_type: row.resource_type,
                name: row.name,
                description: row.description,
                color: row.color,
                usage_count: 0,
                created_at: timestamp(created_at),
                updated_at: timestamp(updated_at),
            });
        }
    }
    Ok(labels)
}

async fn issue_prefix(state: &HandlerState, workspace_id: Uuid) -> String {
    workspace::get_workspace(&state.pool, workspace_id)
        .await
        .ok()
        .flatten()
        .map(|workspace| workspace.issue_prefix)
        .unwrap_or_else(|| "ISSUE".to_string())
}

async fn invalid_status(state: &HandlerState, workspace_id: Uuid, status: &str) -> Response {
    let allowed = cordy_service::issue_status::active_keys(&state.pool, workspace_id)
        .await
        .unwrap_or_else(|_| {
            [
                "backlog",
                "todo",
                "in_progress",
                "in_review",
                "done",
                "blocked",
                "cancelled",
            ]
            .into_iter()
            .map(str::to_string)
            .collect()
        });
    error_response(
        StatusCode::BAD_REQUEST,
        &format!(
            "invalid status {:?}; valid values: {}",
            status,
            allowed.join(", ")
        ),
    )
}

fn optional_uuid(raw: Option<&str>, field: &str) -> Result<Option<Uuid>, String> {
    raw.filter(|value| !value.is_empty())
        .map(|value| Uuid::parse_str(value).map_err(|_| format!("invalid {field}")))
        .transpose()
}

fn uuid_list(raw: Option<&str>, field: &str) -> Result<Vec<Uuid>, String> {
    raw.map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| Uuid::parse_str(value).map_err(|_| format!("invalid {field}")))
            .collect()
    })
    .unwrap_or_else(|| Ok(Vec::new()))
}

fn uuid_strings(raw: &[String], field: &str) -> Result<Vec<Uuid>, String> {
    raw.iter()
        .map(|value| Uuid::parse_str(value).map_err(|_| format!("invalid {field}")))
        .collect()
}

fn comma_list(raw: Option<&str>) -> Vec<String> {
    raw.map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

fn optional_date(raw: Option<&str>, field: &str) -> Result<Option<NaiveDate>, String> {
    raw.filter(|value| !value.is_empty())
        .map(|value| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|_| format!("invalid {field} format, expected YYYY-MM-DD"))
        })
        .transpose()
}

fn timestamp(value: chrono::DateTime<chrono::Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

#[derive(Debug, Serialize)]
struct IssueResponse {
    id: String,
    workspace_id: String,
    number: i32,
    identifier: String,
    title: String,
    description: Option<String>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_category: Option<String>,
    priority: String,
    assignee_type: Option<String>,
    assignee_id: Option<String>,
    creator_type: String,
    creator_id: String,
    parent_issue_id: Option<String>,
    project_id: Option<String>,
    position: f64,
    stage: Option<i32>,
    start_date: Option<String>,
    due_date: Option<String>,
    created_at: String,
    updated_at: String,
    revision: i64,
    last_activity_at: Option<String>,
    metadata: Value,
    properties: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<Vec<LabelResponse>>,
}

impl IssueResponse {
    fn from_issue(issue: &Issue, prefix: &str) -> Self {
        Self {
            id: issue.id.to_string(),
            workspace_id: issue.workspace_id.to_string(),
            number: issue.number,
            identifier: format!("{prefix}-{}", issue.number),
            title: issue.title.clone(),
            description: issue.description.clone(),
            status: issue.status.clone(),
            status_category: cordy_service::issue_status::is_built_in(&issue.status)
                .then(|| issue.status.clone()),
            priority: issue.priority.clone(),
            assignee_type: issue.assignee_type.clone(),
            assignee_id: issue.assignee_id.map(|id| id.to_string()),
            creator_type: issue.creator_type.clone(),
            creator_id: issue.creator_id.to_string(),
            parent_issue_id: issue.parent_issue_id.map(|id| id.to_string()),
            project_id: issue.project_id.map(|id| id.to_string()),
            position: issue.position,
            stage: issue.stage,
            start_date: issue
                .start_date
                .map(|date| date.format("%Y-%m-%d").to_string()),
            due_date: issue
                .due_date
                .map(|date| date.format("%Y-%m-%d").to_string()),
            created_at: timestamp(issue.created_at),
            updated_at: timestamp(issue.updated_at),
            revision: issue.revision,
            last_activity_at: issue.last_activity_at.map(timestamp),
            metadata: object_or_empty(issue.metadata.clone()),
            properties: object_or_empty(issue.properties.clone()),
            labels: None,
        }
    }
}

fn object_or_empty(value: Value) -> Value {
    if value.is_object() {
        value
    } else {
        json!({})
    }
}

#[derive(Debug, Serialize)]
struct LabelResponse {
    id: String,
    workspace_id: String,
    resource_type: String,
    name: String,
    description: String,
    color: String,
    usage_count: i64,
    created_at: String,
    updated_at: String,
}

impl From<&IssueLabel> for LabelResponse {
    fn from(label: &IssueLabel) -> Self {
        Self {
            id: label.id.to_string(),
            workspace_id: label.workspace_id.to_string(),
            resource_type: label.resource_type.clone(),
            name: label.name.clone(),
            description: label.description.clone(),
            color: label.color.clone(),
            usage_count: 0,
            created_at: timestamp(label.created_at),
            updated_at: timestamp(label.updated_at),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn fixture_issue() -> Issue {
        let timestamp = Utc.with_ymd_and_hms(2026, 8, 23, 3, 30, 0).unwrap();
        Issue {
            acceptance_criteria: json!([]),
            assignee_id: None,
            assignee_type: None,
            context_refs: json!([]),
            created_at: timestamp,
            creator_id: Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f12").unwrap(),
            creator_type: "member".into(),
            description: None,
            due_date: None,
            first_executed_at: None,
            id: Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f11").unwrap(),
            last_activity_at: Some(timestamp),
            metadata: Value::Null,
            number: 14,
            origin_id: None,
            origin_type: None,
            parent_issue_id: None,
            position: -7.0,
            priority: "none".into(),
            project_id: None,
            properties: Value::Null,
            revision: 3,
            stage: Some(4),
            start_date: None,
            status: "in_progress".into(),
            title: "Port handlers".into(),
            updated_at: timestamp,
            workspace_id: Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f10").unwrap(),
        }
    }

    #[test]
    fn issue_response_matches_go_wire_shape() {
        let value =
            serde_json::to_value(IssueResponse::from_issue(&fixture_issue(), "CORD")).unwrap();
        assert_eq!(value["identifier"], "CORD-14");
        assert_eq!(value["status_category"], "in_progress");
        assert_eq!(value["created_at"], "2026-08-23T03:30:00Z");
        assert_eq!(value["metadata"], json!({}));
        assert_eq!(value["properties"], json!({}));
        assert!(value.get("labels").is_none());
    }

    #[test]
    fn list_parameter_validation_rejects_malformed_ids() {
        assert!(optional_uuid(Some("not-a-uuid"), "assignee_id").is_err());
        assert!(uuid_list(Some("not-a-uuid"), "ids").is_err());
        assert!(uuid_list(Some(""), "ids").unwrap().is_empty());
    }

    #[test]
    fn date_parser_preserves_calendar_wire_format() {
        assert_eq!(
            optional_date(Some("2026-08-23"), "due_date").unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 23)
        );
        assert!(optional_date(Some("08/23/2026"), "due_date").is_err());
    }
}
