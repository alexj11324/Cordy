//! Core issue routes — S8 authenticated issue-domain slice.
//!
//! Ports the stable list/query, detail, create/update/batch-update, children,
//! and issue-label contracts from `server/internal/handler/issue.go` and `label.go`. The
//! workspace middleware resolves slugs/ids, verifies membership, and stamps a
//! `WorkspaceContext` before these handlers run.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{NaiveDate, SecondsFormat};
use cordy_db::models::{Attachment, Issue, IssueLabel, IssueReaction};
use cordy_db::queries::{
    agent, agent_invocation_target, attachment, issue as issue_q, issue_label, issue_reaction,
    member, squad, workspace,
};
use cordy_middleware::workspace::{WorkspaceContext, WorkspaceGuardState};
use cordy_service::issue_service::{
    IssueCreateError, IssueCreateOpts, IssueCreateParams, IssueTriggerInput, IssueTriggerProbe,
};
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
        .route("/api/issues/batch-update", post(batch_update_issues))
        .route("/api/issues/{id}", get(get_issue).put(update_issue))
        .route("/api/issues/{id}/", get(get_issue).put(update_issue))
        .route("/api/issues/{id}/children", get(list_child_issues))
        .route(
            "/api/issues/{id}/labels",
            get(list_labels_for_issue).post(attach_label),
        )
        .route(
            "/api/issues/{id}/labels/{label_id}",
            axum::routing::delete(detach_label),
        )
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
    let slug = query_owned(&request, "workspace_slug")
        .or_else(|| header_owned(&request, "x-workspace-slug"));
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
    assignee_filters: Option<String>,
    creator_filters: Option<String>,
    include_no_assignee: Option<String>,
    include_no_project: Option<String>,
    label_ids: Option<String>,
    involves_user_id: Option<String>,
    metadata: Option<String>,
    properties: Option<String>,
    date_field: Option<String>,
    date_start: Option<String>,
    date_end: Option<String>,
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

#[derive(Debug, Clone)]
struct ActorFilter {
    actor_type: String,
    actor_id: Uuid,
}

#[derive(Debug, Clone)]
struct DateFilter {
    column: &'static str,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
enum PropertyAlternative {
    Missing(String),
    Contains(Value),
}

#[derive(Debug, Clone)]
struct IssueFilters {
    workspace_id: Uuid,
    statuses: Vec<String>,
    category_statuses: Option<Vec<String>>,
    closed_statuses: Vec<String>,
    priorities: Vec<String>,
    assignee_id: Option<Uuid>,
    assignee_ids: Vec<Uuid>,
    assignee_types: Vec<String>,
    creator_id: Option<Uuid>,
    project_id: Option<Uuid>,
    project_ids: Vec<Uuid>,
    ids: Option<Vec<Uuid>>,
    assignee_filters: Vec<ActorFilter>,
    creator_filters: Vec<ActorFilter>,
    include_no_assignee: bool,
    include_no_project: bool,
    label_ids: Vec<Uuid>,
    involves_user_id: Option<Uuid>,
    metadata: Option<Value>,
    properties: Vec<Vec<PropertyAlternative>>,
    date_filter: Option<DateFilter>,
    search_terms: Vec<String>,
    search_number: Option<i32>,
    top_level_only: bool,
    scheduled: bool,
}

fn push_issue_filters(query: &mut QueryBuilder<'_, Postgres>, filters: &IssueFilters) {
    query
        .push("i.workspace_id = ")
        .push_bind(filters.workspace_id);
    if !filters.statuses.is_empty() {
        query
            .push(" AND i.status = ANY(")
            .push_bind(filters.statuses.clone())
            .push(")");
    }
    if let Some(category_statuses) = &filters.category_statuses {
        query
            .push(" AND i.status = ANY(")
            .push_bind(category_statuses.clone())
            .push(")");
    }
    if !filters.closed_statuses.is_empty() {
        query
            .push(" AND NOT (i.status = ANY(")
            .push_bind(filters.closed_statuses.clone())
            .push("))");
    }
    if !filters.priorities.is_empty() {
        query
            .push(" AND i.priority = ANY(")
            .push_bind(filters.priorities.clone())
            .push(")");
    }
    if let Some(id) = filters.assignee_id {
        query.push(" AND i.assignee_id = ").push_bind(id);
    }
    if !filters.assignee_ids.is_empty() {
        query
            .push(" AND i.assignee_id = ANY(")
            .push_bind(filters.assignee_ids.clone())
            .push(")");
    }
    if !filters.assignee_types.is_empty() {
        query
            .push(" AND i.assignee_type = ANY(")
            .push_bind(filters.assignee_types.clone())
            .push(")");
    }
    if let Some(id) = filters.creator_id {
        query.push(" AND i.creator_id = ").push_bind(id);
    }
    if let Some(id) = filters.project_id {
        query.push(" AND i.project_id = ").push_bind(id);
    }
    if !filters.project_ids.is_empty() || filters.include_no_project {
        query.push(" AND (");
        if !filters.project_ids.is_empty() {
            query
                .push("i.project_id = ANY(")
                .push_bind(filters.project_ids.clone())
                .push(")");
            if filters.include_no_project {
                query.push(" OR ");
            }
        }
        if filters.include_no_project {
            query.push("i.project_id IS NULL");
        }
        query.push(")");
    }
    if let Some(ids) = &filters.ids {
        query
            .push(" AND i.id = ANY(")
            .push_bind(ids.clone())
            .push(")");
    }
    if !filters.assignee_filters.is_empty() || filters.include_no_assignee {
        query.push(" AND (");
        let mut separated = query.separated(" OR ");
        for actor in &filters.assignee_filters {
            separated
                .push("(i.assignee_type = ")
                .push_bind(actor.actor_type.clone())
                .push(" AND i.assignee_id = ")
                .push_bind(actor.actor_id)
                .push(")");
        }
        if filters.include_no_assignee {
            separated.push("(i.assignee_type IS NULL AND i.assignee_id IS NULL)");
        }
        separated.push_unseparated(")");
    }
    if !filters.creator_filters.is_empty() {
        query.push(" AND (");
        let mut separated = query.separated(" OR ");
        for actor in &filters.creator_filters {
            separated
                .push("(i.creator_type = ")
                .push_bind(actor.actor_type.clone())
                .push(" AND i.creator_id = ")
                .push_bind(actor.actor_id)
                .push(")");
        }
        separated.push_unseparated(")");
    }
    if !filters.label_ids.is_empty() {
        query.push(" AND EXISTS (SELECT 1 FROM issue_to_label itl WHERE itl.issue_id = i.id AND itl.label_id = ANY(")
            .push_bind(filters.label_ids.clone()).push("))");
    }
    if let Some(user_id) = filters.involves_user_id {
        query.push(" AND ((i.assignee_type = 'agent' AND i.assignee_id IN (SELECT a.id FROM agent a WHERE a.workspace_id = ")
            .push_bind(filters.workspace_id).push(" AND a.owner_id = ").push_bind(user_id)
            .push(")) OR (i.assignee_type = 'squad' AND i.assignee_id IN (SELECT sm.squad_id FROM squad_member sm JOIN squad s ON s.id = sm.squad_id WHERE s.workspace_id = ")
            .push_bind(filters.workspace_id).push(" AND sm.member_type = 'member' AND sm.member_id = ").push_bind(user_id)
            .push(" UNION SELECT s.id FROM squad s JOIN agent a ON a.id = s.leader_id WHERE s.workspace_id = ")
            .push_bind(filters.workspace_id).push(" AND a.workspace_id = ").push_bind(filters.workspace_id).push(" AND a.owner_id = ").push_bind(user_id)
            .push(" UNION SELECT sm.squad_id FROM squad_member sm JOIN squad s ON s.id = sm.squad_id JOIN agent a ON a.id = sm.member_id WHERE s.workspace_id = ")
            .push_bind(filters.workspace_id).push(" AND sm.member_type = 'agent' AND a.workspace_id = ").push_bind(filters.workspace_id).push(" AND a.owner_id = ").push_bind(user_id)
            .push(")))");
    }
    if let Some(metadata) = &filters.metadata {
        query
            .push(" AND i.metadata @> ")
            .push_bind(metadata.clone());
    }
    for alternatives in &filters.properties {
        query.push(" AND (");
        let mut separated = query.separated(" OR ");
        for alternative in alternatives {
            match alternative {
                PropertyAlternative::Missing(definition_id) => {
                    separated
                        .push("NOT (i.properties ? ")
                        .push_bind(definition_id.clone())
                        .push(")");
                }
                PropertyAlternative::Contains(value) => {
                    separated.push("i.properties @> ").push_bind(value.clone());
                }
            }
        }
        separated.push_unseparated(")");
    }
    if let Some(date) = &filters.date_filter {
        query
            .push(" AND i.")
            .push(date.column)
            .push(" >= ")
            .push_bind(date.start)
            .push(" AND i.")
            .push(date.column)
            .push(" < ")
            .push_bind(date.end);
    }
    if !filters.search_terms.is_empty() || filters.search_number.is_some() {
        query.push(" AND (");
        if !filters.search_terms.is_empty() {
            query.push("(");
            let mut separated = query.separated(" AND ");
            for pattern in &filters.search_terms {
                separated
                    .push("LOWER(i.title) LIKE ")
                    .push_bind(pattern.clone())
                    .push(" ESCAPE '\\\\'");
            }
            separated.push_unseparated(")");
            if filters.search_number.is_some() {
                query.push(" OR ");
            }
        }
        if let Some(number) = filters.search_number {
            query.push("i.number = ").push_bind(number);
        }
        query.push(")");
    }
    if filters.top_level_only {
        query.push(" AND i.parent_issue_id IS NULL");
    }
    if filters.scheduled {
        query.push(" AND (i.start_date IS NOT NULL OR i.due_date IS NOT NULL)");
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
    let open_only = params.open_only.as_deref() == Some("true");
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

    let statuses = comma_list(params.statuses.as_deref().or(params.status.as_deref()));
    let categories = comma_list(
        params
            .status_categories
            .as_deref()
            .or(params.status_category.as_deref()),
    );
    let category_statuses = match expand_status_categories(state, workspace_id, &categories).await {
        Ok(values) => (!categories.is_empty()).then_some(values),
        Err(response) => return response,
    };
    let closed_statuses = if open_only {
        match expand_status_categories(
            state,
            workspace_id,
            &["done".to_string(), "cancelled".to_string()],
        )
        .await
        {
            Ok(values) => values,
            Err(response) => return response,
        }
    } else {
        Vec::new()
    };
    let priorities = comma_list(params.priorities.as_deref().or(params.priority.as_deref()));
    let assignee_filters =
        match actor_filters(params.assignee_filters.as_deref(), "assignee_filters") {
            Ok(value) => value,
            Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
        };
    let creator_filters = match actor_filters(params.creator_filters.as_deref(), "creator_filters")
    {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let label_ids = match uuid_list(params.label_ids.as_deref(), "label_ids") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let involves_user_id =
        match optional_uuid(params.involves_user_id.as_deref(), "involves_user_id") {
            Ok(value) => value,
            Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
        };
    let metadata = match json_object_filter(params.metadata.as_deref(), "metadata") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let properties = match properties_filter(params.properties.as_deref()) {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let date_filter = match parse_date_filter(
        params.date_field.as_deref(),
        params.date_start.as_deref(),
        params.date_end.as_deref(),
    ) {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let (search_terms, search_number) = search_filter(params.q.as_deref());
    let filters = IssueFilters {
        workspace_id,
        statuses,
        category_statuses,
        closed_statuses,
        priorities,
        assignee_id,
        assignee_ids,
        assignee_types,
        creator_id,
        project_id,
        project_ids,
        ids: params.ids.is_some().then_some(ids),
        assignee_filters,
        creator_filters,
        include_no_assignee: params.include_no_assignee.as_deref() == Some("true"),
        include_no_project: params.include_no_project.as_deref() == Some("true"),
        label_ids,
        involves_user_id,
        metadata,
        properties,
        date_filter,
        search_terms,
        search_number,
        top_level_only: params.top_level_only.as_deref() == Some("true"),
        scheduled: params.scheduled.as_deref() == Some("true"),
    };

    let mut count_query = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM issue i WHERE ");
    push_issue_filters(&mut count_query, &filters);
    let total = match count_query
        .build_query_scalar::<i64>()
        .fetch_one(&state.pool)
        .await
    {
        Ok(total) => total,
        Err(error) => {
            tracing::warn!(%error, "failed to count issues");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list issues");
        }
    };

    let mut query = QueryBuilder::<Postgres>::new("SELECT i.* FROM issue i WHERE ");
    push_issue_filters(&mut query, &filters);

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
    if !open_only {
        query
            .push(" LIMIT ")
            .push_bind(limit)
            .push(" OFFSET ")
            .push_bind(offset);
    }

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
    let issue_id = issue.id;
    let workspace_id = issue.workspace_id;
    let mut responses = enrich_issue_list(&state, &context, vec![issue]).await;
    let mut response = responses.remove(0);
    response.reactions = issue_reaction::list_issue_reactions(&state.pool, issue_id)
        .await
        .unwrap_or_default()
        .iter()
        .map(IssueReactionResponse::from)
        .collect();
    response.attachments =
        attachment::list_attachments_by_issue(&state.pool, issue_id, workspace_id)
            .await
            .unwrap_or_default()
            .iter()
            .map(AttachmentResponse::from)
            .collect();
    Json(response).into_response()
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

#[derive(Debug, Clone)]
enum UpdateField<T> {
    Missing,
    Null,
    Value(T),
}

impl<T> UpdateField<T> {
    fn is_present(&self) -> bool {
        !matches!(self, Self::Missing)
    }
}

fn update_field<T: serde::de::DeserializeOwned>(
    fields: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<UpdateField<T>, Response> {
    let Some(value) = fields.get(name) else {
        return Ok(UpdateField::Missing);
    };
    if value.is_null() {
        return Ok(UpdateField::Null);
    }
    serde_json::from_value(value.clone())
        .map(UpdateField::Value)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid request body"))
}

fn update_object(body: &[u8]) -> Result<serde_json::Map<String, Value>, Response> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(fields)) => Ok(fields),
        _ => Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid request body",
        )),
    }
}

async fn update_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let previous = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let fields = match update_object(&body) {
        Ok(fields) => fields,
        Err(response) => return response,
    };
    match apply_issue_update(&state, &context, previous, &fields).await {
        Ok(issue) => issue_response(&state, issue).await,
        Err(response) => response,
    }
}

async fn apply_issue_update(
    state: &HandlerState,
    context: &WorkspaceContext,
    previous: Issue,
    fields: &serde_json::Map<String, Value>,
) -> Result<Issue, Response> {
    let expected_revision = match update_field::<i64>(fields, "expected_revision")? {
        UpdateField::Value(value) if value > 0 => Some(value),
        UpdateField::Missing | UpdateField::Null => None,
        UpdateField::Value(_) => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "expected_revision must be a positive integer",
            ))
        }
    };
    if let Some(expected) = expected_revision {
        if expected != previous.revision {
            return Err(revision_conflict(&previous, expected, previous.revision));
        }
    }
    let suppress_run = match update_field::<bool>(fields, "suppress_run")? {
        UpdateField::Value(value) => value,
        UpdateField::Missing | UpdateField::Null => false,
    };
    let handoff_note = match update_field::<String>(fields, "handoff_note")? {
        UpdateField::Value(value) => value,
        UpdateField::Missing | UpdateField::Null => String::new(),
    };
    let attachment_ids = match update_field::<Vec<String>>(fields, "attachment_ids")? {
        UpdateField::Value(values) => uuid_strings(&values, "attachment_ids")
            .map_err(|message| error_response(StatusCode::BAD_REQUEST, &message))?,
        UpdateField::Missing | UpdateField::Null => Vec::new(),
    };

    let mut next = previous.clone();
    if let UpdateField::Value(value) = update_field::<String>(fields, "title")? {
        next.title = value;
    }
    if let UpdateField::Value(value) = update_field::<String>(fields, "description")? {
        next.description = Some(value);
    }
    if let UpdateField::Value(value) = update_field::<String>(fields, "status")? {
        next.status =
            match cordy_service::issue_status::resolve(&state.pool, previous.workspace_id, &value)
                .await
            {
                Ok(entry) => entry.key,
                Err(_) => return Err(invalid_status(state, previous.workspace_id, &value).await),
            };
    }
    if let UpdateField::Value(value) = update_field::<String>(fields, "priority")? {
        if !PRIORITIES.contains(&value.as_str()) {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                &format!(
                    "invalid priority: {value} (must be one of: {})",
                    PRIORITIES.join(", ")
                ),
            ));
        }
        next.priority = value;
    }
    if let UpdateField::Value(value) = update_field::<f64>(fields, "position")? {
        if !value.is_finite() {
            return Err(error_response(StatusCode::BAD_REQUEST, "invalid position"));
        }
        next.position = value;
    }

    let assignee_type = update_field::<String>(fields, "assignee_type")?;
    let assignee_id = update_field::<String>(fields, "assignee_id")?;
    let assignee_touched = assignee_type.is_present() || assignee_id.is_present();
    match assignee_type {
        UpdateField::Missing => {}
        UpdateField::Null => next.assignee_type = None,
        UpdateField::Value(value) => next.assignee_type = Some(value),
    }
    match assignee_id {
        UpdateField::Missing => {}
        UpdateField::Null => next.assignee_id = None,
        UpdateField::Value(value) => {
            next.assignee_id = Some(
                Uuid::parse_str(&value)
                    .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid assignee_id"))?,
            )
        }
    }
    if assignee_touched {
        validate_assignee(state, previous.workspace_id, &next).await?;
    }

    let start_date = update_field::<String>(fields, "start_date")?;
    if start_date.is_present() {
        next.start_date = match start_date {
            UpdateField::Missing | UpdateField::Null => None,
            UpdateField::Value(value) if value.is_empty() => None,
            UpdateField::Value(value) => {
                Some(NaiveDate::parse_from_str(&value, "%Y-%m-%d").map_err(|_| {
                    error_response(
                        StatusCode::BAD_REQUEST,
                        "invalid start_date format, expected YYYY-MM-DD",
                    )
                })?)
            }
        };
    }
    let due_date = update_field::<String>(fields, "due_date")?;
    if due_date.is_present() {
        next.due_date = match due_date {
            UpdateField::Missing | UpdateField::Null => None,
            UpdateField::Value(value) if value.is_empty() => None,
            UpdateField::Value(value) => {
                Some(NaiveDate::parse_from_str(&value, "%Y-%m-%d").map_err(|_| {
                    error_response(
                        StatusCode::BAD_REQUEST,
                        "invalid due_date format, expected YYYY-MM-DD",
                    )
                })?)
            }
        };
    }

    let parent = update_field::<String>(fields, "parent_issue_id")?;
    if parent.is_present() {
        next.parent_issue_id = match parent {
            UpdateField::Missing | UpdateField::Null => None,
            UpdateField::Value(value) => {
                let parent_id = Uuid::parse_str(&value).map_err(|_| {
                    error_response(StatusCode::BAD_REQUEST, "invalid parent_issue_id")
                })?;
                validate_parent(state, &previous, parent_id).await?;
                Some(parent_id)
            }
        };
    }

    let project = update_field::<String>(fields, "project_id")?;
    if project.is_present() {
        next.project_id = match project {
            UpdateField::Missing | UpdateField::Null => None,
            UpdateField::Value(value) => {
                let project_id = Uuid::parse_str(&value)
                    .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid project_id"))?;
                match cordy_db::queries::project::get_project_in_workspace(
                    &state.pool,
                    project_id,
                    previous.workspace_id,
                )
                .await
                {
                    Ok(Some(_)) => Some(project_id),
                    Ok(None) => {
                        return Err(error_response(
                            StatusCode::BAD_REQUEST,
                            "project not found in this workspace",
                        ))
                    }
                    Err(error) => {
                        tracing::warn!(%error, %project_id, "failed to validate issue project");
                        return Err(error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "failed to validate project",
                        ));
                    }
                }
            }
        };
    }

    let stage = update_field::<i32>(fields, "stage")?;
    if stage.is_present() {
        next.stage = match stage {
            UpdateField::Missing | UpdateField::Null => None,
            UpdateField::Value(value) if value >= 1 => Some(value),
            UpdateField::Value(_) => {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "stage must be >= 1",
                ))
            }
        };
    }

    let did_change = issue_mutable_fields_differ(&previous, &next);
    if !did_change && attachment_ids.is_empty() {
        return Ok(previous);
    }
    let did_activity = issue_activity_fields_differ(&previous, &next);
    let mut updated = if did_change {
        let updated = sqlx::query_as::<_, Issue>(
        r#"UPDATE issue SET
title = $3, description = $4, status = $5, priority = $6,
assignee_type = $7, assignee_id = $8, position = $9, start_date = $10,
due_date = $11, parent_issue_id = $12, project_id = $13, stage = $14,
revision = revision + 1, updated_at = now(),
last_activity_at = CASE WHEN $15 THEN GREATEST(COALESCE(last_activity_at, updated_at), now()) ELSE last_activity_at END
WHERE id = $1 AND workspace_id = $2
  AND ($16::bigint IS NULL OR revision = $16)
RETURNING *"#,
    )
        .bind(previous.id)
        .bind(previous.workspace_id)
        .bind(&next.title)
        .bind(&next.description)
        .bind(&next.status)
        .bind(&next.priority)
        .bind(&next.assignee_type)
        .bind(next.assignee_id)
        .bind(next.position)
        .bind(next.start_date)
        .bind(next.due_date)
        .bind(next.parent_issue_id)
        .bind(next.project_id)
        .bind(next.stage)
        .bind(did_activity)
        .bind(expected_revision)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| {
            tracing::warn!(%error, issue_id = %previous.id, "failed to update issue");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
        })?;
        let Some(updated) = updated else {
            let actual =
                issue_q::get_issue_in_workspace(&state.pool, previous.id, previous.workspace_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|issue| issue.revision)
                    .unwrap_or(previous.revision);
            return Err(revision_conflict(
                &previous,
                expected_revision.unwrap_or(previous.revision),
                actual,
            ));
        };
        updated
    } else {
        previous.clone()
    };
    if !attachment_ids.is_empty() {
        let linked = cordy_db::queries::attachment::link_attachments_to_issue(
            &state.pool,
            previous.id,
            previous.workspace_id,
            attachment_ids,
            !did_change,
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, issue_id = %previous.id, "failed to link issue attachments");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to link issue attachments",
            )
        })?;
        if linked.is_some_and(|result| result.linked_count > 0) {
            if let Ok(Some(current)) =
                issue_q::get_issue_in_workspace(&state.pool, previous.id, previous.workspace_id)
                    .await
            {
                updated = current;
            }
        }
    }
    publish_issue_updated(state, context, &previous, &updated).await;
    let assignee_changed = previous.assignee_type != updated.assignee_type
        || previous.assignee_id != updated.assignee_id;
    let status_changed = previous.status != updated.status;
    if !suppress_run {
        let trigger = state
            .issues
            .will_enqueue_run(
                IssueTriggerInput {
                    issue: updated.clone(),
                    prev_status: previous.status.clone(),
                    is_create: false,
                    assignee_changed,
                    status_changed,
                },
                IssueTriggerProbe {
                    can_access_agent: None,
                    is_self_loop: None,
                    suppress_active_self_assignment: None,
                },
            )
            .await;
        if trigger.is_some() {
            if let Err(error) = state
                .tasks
                .enqueue_task_for_issue_with_handoff(
                    &updated,
                    &handoff_note,
                    Some(context.member.user_id),
                )
                .await
            {
                tracing::warn!(%error, issue_id = %updated.id, "failed to enqueue updated issue");
            }
        }
    }
    Ok(updated)
}

fn revision_conflict(issue: &Issue, expected: i64, actual: i64) -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "resource changed since it was loaded",
            "code": "revision_conflict",
            "resource_type": "issue",
            "resource_id": issue.id.to_string(),
            "expected_revision": expected,
            "actual_revision": actual,
        })),
    )
        .into_response()
}

fn issue_mutable_fields_differ(left: &Issue, right: &Issue) -> bool {
    left.title != right.title
        || left.description != right.description
        || left.status != right.status
        || left.priority != right.priority
        || left.assignee_type != right.assignee_type
        || left.assignee_id != right.assignee_id
        || left.position != right.position
        || left.start_date != right.start_date
        || left.due_date != right.due_date
        || left.parent_issue_id != right.parent_issue_id
        || left.project_id != right.project_id
        || left.stage != right.stage
}

fn issue_activity_fields_differ(left: &Issue, right: &Issue) -> bool {
    left.title != right.title
        || left.description != right.description
        || left.status != right.status
        || left.priority != right.priority
        || left.assignee_type != right.assignee_type
        || left.assignee_id != right.assignee_id
        || left.start_date != right.start_date
        || left.due_date != right.due_date
        || left.parent_issue_id != right.parent_issue_id
        || left.project_id != right.project_id
        || left.stage != right.stage
}

async fn validate_assignee(
    state: &HandlerState,
    workspace_id: Uuid,
    issue: &Issue,
) -> Result<(), Response> {
    let (Some(kind), Some(id)) = (issue.assignee_type.as_deref(), issue.assignee_id) else {
        if issue.assignee_type.is_none() && issue.assignee_id.is_none() {
            return Ok(());
        }
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "assignee_type and assignee_id must be set together",
        ));
    };
    let table = match kind {
        "member" => "member",
        "agent" => "agent",
        "squad" => "squad",
        _ => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid assignee_type",
            ))
        }
    };
    let query = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = $1 AND workspace_id = $2)");
    match sqlx::query_scalar::<_, bool>(&query)
        .bind(id)
        .bind(workspace_id)
        .fetch_one(&state.pool)
        .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(error_response(
            StatusCode::BAD_REQUEST,
            "assignee not found in this workspace",
        )),
        Err(error) => {
            tracing::warn!(%error, %id, kind, "failed to validate issue assignee");
            Err(error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to validate assignee",
            ))
        }
    }
}

async fn validate_parent(
    state: &HandlerState,
    issue: &Issue,
    parent_id: Uuid,
) -> Result<(), Response> {
    if parent_id == issue.id {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "an issue cannot be its own parent",
        ));
    }
    let mut cursor = parent_id;
    for _ in 0..10 {
        let parent = issue_q::get_issue_in_workspace(&state.pool, cursor, issue.workspace_id)
            .await
            .map_err(|error| {
                tracing::warn!(%error, %parent_id, "failed to validate parent issue");
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to validate parent issue",
                )
            })?;
        let Some(parent) = parent else {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "parent issue not found in this workspace",
            ));
        };
        let Some(ancestor) = parent.parent_issue_id else {
            return Ok(());
        };
        if ancestor == issue.id {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "circular parent relationship detected",
            ));
        }
        cursor = ancestor;
    }
    Ok(())
}

async fn issue_response(state: &HandlerState, issue: Issue) -> Response {
    let mut response =
        IssueResponse::from_issue(&issue, &issue_prefix(state, issue.workspace_id).await);
    response.status_category = Some(
        cordy_service::issue_status::effective(&state.pool, issue.workspace_id, &issue.status)
            .await,
    );
    Json(response).into_response()
}

async fn publish_issue_updated(
    state: &HandlerState,
    context: &WorkspaceContext,
    previous: &Issue,
    issue: &Issue,
) {
    let prefix = issue_prefix(state, issue.workspace_id).await;
    let category =
        cordy_service::issue_status::effective(&state.pool, issue.workspace_id, &issue.status)
            .await;
    let mut response = IssueResponse::from_issue(issue, &prefix);
    response.status_category = Some(category);
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_ISSUE_UPDATED.to_string(),
        workspace_id: issue.workspace_id.to_string(),
        actor_type: "member".to_string(),
        actor_id: context.member.user_id.to_string(),
        payload: json!({
            "issue": response,
            "assignee_changed": previous.assignee_type != issue.assignee_type || previous.assignee_id != issue.assignee_id,
            "status_changed": previous.status != issue.status,
            "priority_changed": previous.priority != issue.priority,
            "project_changed": previous.project_id != issue.project_id,
        }),
        task_id: String::new(),
        chat_session_id: String::new(),
    });
}

async fn batch_update_issues(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    body: Bytes,
) -> Response {
    let root = match update_object(&body) {
        Ok(fields) => fields,
        Err(response) => return response,
    };
    let issue_ids = match root.get("issue_ids").cloned() {
        Some(value) => match serde_json::from_value::<Vec<String>>(value) {
            Ok(ids) if !ids.is_empty() => ids,
            _ => return error_response(StatusCode::BAD_REQUEST, "issue_ids is required"),
        },
        None => return error_response(StatusCode::BAD_REQUEST, "issue_ids is required"),
    };
    let updates = match root.get("updates") {
        Some(Value::Object(fields)) => fields,
        _ => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let mutation_keys = [
        "title",
        "description",
        "status",
        "priority",
        "position",
        "assignee_type",
        "assignee_id",
        "start_date",
        "due_date",
        "parent_issue_id",
        "project_id",
        "stage",
    ];
    if !mutation_keys.iter().any(|key| updates.contains_key(*key)) {
        return Json(json!({ "updated": 0 })).into_response();
    }

    let mut updated = 0usize;
    for raw_id in issue_ids {
        let Ok(id) = Uuid::parse_str(&raw_id) else {
            continue;
        };
        let previous =
            match issue_q::get_issue_in_workspace(&state.pool, id, context.member.workspace_id)
                .await
            {
                Ok(Some(issue)) => issue,
                _ => continue,
            };
        if apply_issue_update(&state, &context, previous, updates)
            .await
            .is_ok()
        {
            updated += 1;
        }
    }
    Json(json!({ "updated": updated })).into_response()
}

#[derive(Debug, Deserialize)]
struct AttachLabelRequest {
    label_id: String,
}

async fn attach_label(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    Json(request): Json<AttachLabelRequest>,
) -> Response {
    if request.label_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "label_id is required");
    }
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let label_id = match Uuid::parse_str(&request.label_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid label_id"),
    };
    let label = match issue_label::get_label(&state.pool, label_id, issue.workspace_id).await {
        Ok(Some(label)) if label.resource_type == "issue" => label,
        Ok(Some(_)) => return error_response(StatusCode::NOT_FOUND, "issue label not found"),
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "label not found"),
        Err(error) => {
            tracing::warn!(%error, %label_id, "failed to load issue label");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to attach label");
        }
    };
    let result = match issue_label::attach_label_to_issue(
        &state.pool,
        issue.id,
        label.id,
        issue.workspace_id,
    )
    .await
    {
        Ok(Some(result)) => result,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "label not found"),
        Err(error) => {
            tracing::warn!(%error, %label_id, "failed to attach issue label");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to attach label");
        }
    };
    label_mutation_response(
        &state,
        &context,
        &issue,
        result.changed,
        result.issue_revision,
    )
    .await
}

async fn detach_label(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((id, label_id)): Path<(String, String)>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let label_id = match Uuid::parse_str(&label_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid label id"),
    };
    match issue_label::get_label(&state.pool, label_id, issue.workspace_id).await {
        Ok(Some(label)) if label.resource_type == "issue" => {}
        Ok(Some(_)) => return error_response(StatusCode::NOT_FOUND, "issue label not found"),
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "label not found"),
        Err(error) => {
            tracing::warn!(%error, %label_id, "failed to load issue label");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to detach label");
        }
    }
    let result = match issue_label::detach_label_from_issue(
        &state.pool,
        issue.id,
        label_id,
        issue.workspace_id,
    )
    .await
    {
        Ok(Some(result)) => result,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "label not found"),
        Err(error) => {
            tracing::warn!(%error, %label_id, "failed to detach issue label");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to detach label");
        }
    };
    label_mutation_response(
        &state,
        &context,
        &issue,
        result.changed,
        result.issue_revision,
    )
    .await
}

async fn label_mutation_response(
    state: &HandlerState,
    context: &WorkspaceContext,
    issue: &Issue,
    changed: bool,
    revision: i64,
) -> Response {
    let labels = match labels_for_issues(state, issue.workspace_id, &[issue.id]).await {
        Ok(mut labels) => labels.remove(&issue.id).unwrap_or_default(),
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to reload issue labels");
            return Json(json!({})).into_response();
        }
    };
    if changed {
        state.bus.publish(&cordy_events::Event {
            event_type: cordy_protocol::EVENT_ISSUE_LABELS_CHANGED.to_string(),
            workspace_id: issue.workspace_id.to_string(),
            actor_type: "member".to_string(),
            actor_id: context.member.user_id.to_string(),
            payload: json!({
                "issue_id": issue.id.to_string(),
                "labels": labels,
                "issue_revision": revision,
            }),
            task_id: String::new(),
            chat_session_id: String::new(),
        });
    }
    if revision > 0 {
        Json(json!({ "labels": labels, "issue_revision": revision })).into_response()
    } else {
        Json(json!({ "labels": labels })).into_response()
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
    body: Result<Json<CreateIssueRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match body {
        Ok(body) => body,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.title.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "title is required");
    }
    let workspace_id = context.member.workspace_id;
    let status = if request.status.is_empty() {
        "todo".to_string()
    } else {
        request.status
    };
    let (status, status_category) =
        match cordy_service::issue_status::resolve(&state.pool, workspace_id, &status).await {
            Ok(entry) => (entry.key, entry.category),
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
    if let (Some(kind), Some(id)) = (request.assignee_type.as_deref(), assignee_id) {
        if let Err(message) = validate_assignee(&state, &context, kind, id).await {
            return error_response(StatusCode::BAD_REQUEST, &message);
        }
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
    let task_identity = trusted_agent_task(&state, &context, &headers).await;
    let (creator_type, creator_id) = task_identity
        .as_ref()
        .map(|(_, agent_id)| ("agent".to_string(), *agent_id))
        .unwrap_or_else(|| ("member".to_string(), context.member.user_id));
    let (origin_type, origin_id) = if request.origin_type.is_some() {
        (request.origin_type, origin_id)
    } else if let Some((task_id, _)) = task_identity {
        (Some("agent_create".to_string()), Some(task_id))
    } else {
        (None, None)
    };

    let prefix = issue_prefix(&state, workspace_id).await;
    let broadcast_prefix = prefix.clone();
    let broadcast_status_category = status_category.clone();
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
                origin_type,
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
                    response.status_category = Some(broadcast_status_category.clone());
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
            response.status_category = Some(status_category);
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

async fn trusted_agent_task(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
) -> Option<(Uuid, Uuid)> {
    let agent_id = header_uuid(headers, "x-agent-id")?;
    let task_id = header_uuid(headers, "x-task-id")?;
    let task = agent::get_agent_task(&state.pool, task_id)
        .await
        .ok()
        .flatten()?;
    if task.agent_id != agent_id {
        return None;
    }
    agent::get_agent_in_workspace(&state.pool, agent_id, context.member.workspace_id)
        .await
        .ok()
        .flatten()
        .filter(|agent| agent.archived_at.is_none())?;
    Some((task_id, agent_id))
}

fn header_uuid(headers: &HeaderMap, name: &str) -> Option<Uuid> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
}

async fn validate_assignee(
    state: &HandlerState,
    context: &WorkspaceContext,
    kind: &str,
    id: Uuid,
) -> Result<(), String> {
    let workspace_id = context.member.workspace_id;
    match kind {
        "member" => {
            if member::get_member_by_user_and_workspace(&state.pool, id, workspace_id)
                .await
                .ok()
                .flatten()
                .is_none()
            {
                return Err("assignee member not found in this workspace".to_string());
            }
        }
        "agent" => {
            let target = agent::get_agent_in_workspace(&state.pool, id, workspace_id)
                .await
                .ok()
                .flatten()
                .filter(|agent| agent.archived_at.is_none())
                .ok_or_else(|| "assignee agent not found in this workspace".to_string())?;
            if !can_member_invoke_agent(state, context.member.user_id, workspace_id, &target).await
            {
                return Err("you do not have permission to invoke this agent".to_string());
            }
        }
        "squad" => {
            let target = squad::get_squad_in_workspace(&state.pool, id, workspace_id)
                .await
                .ok()
                .flatten()
                .filter(|squad| squad.archived_at.is_none())
                .ok_or_else(|| "assignee squad not found in this workspace".to_string())?;
            let leader = agent::get_agent_in_workspace(&state.pool, target.leader_id, workspace_id)
                .await
                .ok()
                .flatten()
                .filter(|agent| agent.archived_at.is_none())
                .ok_or_else(|| "squad leader is unavailable".to_string())?;
            if !can_member_invoke_agent(state, context.member.user_id, workspace_id, &leader).await
            {
                return Err("you do not have permission to invoke this squad".to_string());
            }
        }
        _ => return Err("invalid assignee_type".to_string()),
    }
    Ok(())
}

async fn can_member_invoke_agent(
    state: &HandlerState,
    user_id: Uuid,
    workspace_id: Uuid,
    target: &cordy_db::models::Agent,
) -> bool {
    if target.owner_id == Some(user_id) {
        return true;
    }
    if target.permission_mode != "public_to" {
        return false;
    }
    let is_member = member::get_member_by_user_and_workspace(&state.pool, user_id, workspace_id)
        .await
        .ok()
        .flatten()
        .is_some();
    agent_invocation_target::list_agent_invocation_targets(&state.pool, target.id)
        .await
        .unwrap_or_default()
        .iter()
        .any(|entry| {
            (entry.target_type == "workspace" && is_member)
                || (entry.target_type == "member" && entry.target_id == user_id)
        })
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
    let mut status_resolver =
        cordy_service::issue_status::Resolver::new(context.member.workspace_id);
    let mut responses = Vec::with_capacity(issues.len());
    for issue in issues {
        let category = status_resolver.effective(&state.pool, &issue.status).await;
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
        .map(|workspace| {
            if workspace.issue_prefix.trim().is_empty() {
                legacy_issue_prefix(&workspace.name)
            } else {
                workspace.issue_prefix
            }
        })
        .unwrap_or_else(|| "ISSUE".to_string())
}

fn legacy_issue_prefix(name: &str) -> String {
    let letters = name
        .chars()
        .filter(|character| character.is_ascii_alphabetic())
        .take(3)
        .collect::<String>()
        .to_ascii_uppercase();
    if letters.is_empty() {
        "WS".to_string()
    } else {
        letters
    }
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

async fn expand_status_categories(
    state: &HandlerState,
    workspace_id: Uuid,
    categories: &[String],
) -> Result<Vec<String>, Response> {
    if categories.is_empty() {
        return Ok(Vec::new());
    }
    let entries =
        cordy_db::queries::issue_status::list_issue_status_entries(&state.pool, workspace_id, true)
            .await
            .map_err(|error| {
                tracing::warn!(%error, "failed to expand issue status categories");
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to resolve status categories",
                )
            })?;
    let mut keys = Vec::new();
    for category in categories {
        if cordy_service::issue_status::is_built_in(category) {
            keys.push(category.clone());
        }
        keys.extend(
            entries
                .iter()
                .filter(|entry| entry.category == *category)
                .map(|entry| entry.key.clone()),
        );
    }
    keys.sort();
    keys.dedup();
    Ok(keys)
}

fn actor_filters(raw: Option<&str>, field: &str) -> Result<Vec<ActorFilter>, String> {
    comma_list(raw)
        .into_iter()
        .map(|value| {
            let (actor_type, id) = value
                .split_once(':')
                .ok_or_else(|| format!("invalid {field}"))?;
            if !matches!(actor_type, "member" | "agent" | "squad") || id.trim().is_empty() {
                return Err(format!("invalid {field}"));
            }
            Ok(ActorFilter {
                actor_type: actor_type.to_string(),
                actor_id: Uuid::parse_str(id.trim()).map_err(|_| format!("invalid {field}"))?,
            })
        })
        .collect()
}

fn json_object_filter(raw: Option<&str>, field: &str) -> Result<Option<Value>, String> {
    let value = json_filter(raw, field)?;
    if value.as_ref().is_some_and(|value| !value.is_object()) {
        return Err(format!("invalid {field}"));
    }
    Ok(value)
}

fn json_filter(raw: Option<&str>, field: &str) -> Result<Option<Value>, String> {
    raw.filter(|raw| !raw.trim().is_empty())
        .map(|raw| serde_json::from_str(raw).map_err(|_| format!("invalid {field}")))
        .transpose()
}

fn properties_filter(raw: Option<&str>) -> Result<Vec<Vec<PropertyAlternative>>, String> {
    let Some(raw) = raw.filter(|raw| !raw.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    let parsed = serde_json::from_str::<HashMap<String, Vec<String>>>(raw).map_err(|_| {
        "properties filter must be a JSON object of {definitionId: [values]}".to_string()
    })?;
    let mut groups = Vec::new();
    for (definition_id, values) in parsed {
        Uuid::parse_str(&definition_id).map_err(|_| {
            format!("properties filter key {definition_id:?} is not a definition id")
        })?;
        if values.is_empty() {
            continue;
        }
        let mut alternatives = Vec::new();
        for value in values {
            if value.is_empty() {
                return Err("properties filter values cannot be empty".to_string());
            }
            if value == "__none__" {
                alternatives.push(PropertyAlternative::Missing(definition_id.clone()));
                continue;
            }
            alternatives.push(PropertyAlternative::Contains(
                json!({ definition_id.clone(): value }),
            ));
            alternatives.push(PropertyAlternative::Contains(
                json!({ definition_id.clone(): [value.clone()] }),
            ));
            if matches!(value.as_str(), "true" | "false") {
                alternatives.push(PropertyAlternative::Contains(
                    json!({ definition_id.clone(): value == "true" }),
                ));
            }
        }
        groups.push(alternatives);
    }
    Ok(groups)
}

fn parse_date_filter(
    field: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
) -> Result<Option<DateFilter>, String> {
    if field.is_none() && start.is_none() && end.is_none() {
        return Ok(None);
    }
    let (Some(field), Some(start), Some(end)) = (field, start, end) else {
        return Err("date_field, date_start, and date_end are required together".to_string());
    };
    let column = match field.trim() {
        "created_at" => "created_at",
        "updated_at" => "updated_at",
        _ => return Err("invalid date_field".to_string()),
    };
    let start = chrono::DateTime::parse_from_rfc3339(start)
        .map_err(|_| "invalid date_start".to_string())?
        .with_timezone(&chrono::Utc);
    let end = chrono::DateTime::parse_from_rfc3339(end)
        .map_err(|_| "invalid date_end".to_string())?
        .with_timezone(&chrono::Utc);
    if start >= end {
        return Err("date_start must be before date_end".to_string());
    }
    Ok(Some(DateFilter { column, start, end }))
}

fn search_filter(raw: Option<&str>) -> (Vec<String>, Option<i32>) {
    let Some(query) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return (Vec::new(), None);
    };
    let terms = query
        .to_lowercase()
        .split_whitespace()
        .map(|term| format!("%{}%", escape_like(term)))
        .collect();
    let numeric_text = if let Some((prefix, number)) = query.split_once('-') {
        (prefix
            .chars()
            .all(|character| character.is_ascii_alphabetic())
            && !prefix.is_empty()
            && !number.contains('-'))
        .then_some(number)
    } else {
        Some(query)
    };
    let numeric = numeric_text
        .and_then(|number| number.parse::<i32>().ok())
        .filter(|number| *number > 0);
    (terms, numeric)
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
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
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reactions: Vec<IssueReactionResponse>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<AttachmentResponse>,
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
            reactions: Vec::new(),
            attachments: Vec::new(),
            labels: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct IssueReactionResponse {
    id: String,
    issue_id: String,
    actor_type: String,
    actor_id: String,
    emoji: String,
    created_at: String,
}

impl From<&IssueReaction> for IssueReactionResponse {
    fn from(reaction: &IssueReaction) -> Self {
        Self {
            id: reaction.id.to_string(),
            issue_id: reaction.issue_id.to_string(),
            actor_type: reaction.actor_type.clone(),
            actor_id: reaction.actor_id.to_string(),
            emoji: reaction.emoji.clone(),
            created_at: timestamp(reaction.created_at),
        }
    }
}

#[derive(Debug, Serialize)]
struct AttachmentResponse {
    id: String,
    workspace_id: String,
    issue_id: Option<String>,
    comment_id: Option<String>,
    chat_session_id: Option<String>,
    chat_message_id: Option<String>,
    uploader_type: String,
    uploader_id: String,
    filename: String,
    url: String,
    download_url: String,
    markdown_url: String,
    content_type: String,
    size_bytes: i64,
    created_at: String,
}

impl From<&Attachment> for AttachmentResponse {
    fn from(attachment: &Attachment) -> Self {
        let stable_url = format!("/api/attachments/{}/download", attachment.id);
        Self {
            id: attachment.id.to_string(),
            workspace_id: attachment.workspace_id.to_string(),
            issue_id: attachment.issue_id.map(|id| id.to_string()),
            comment_id: attachment.comment_id.map(|id| id.to_string()),
            chat_session_id: attachment.chat_session_id.map(|id| id.to_string()),
            chat_message_id: attachment.chat_message_id.map(|id| id.to_string()),
            uploader_type: attachment.uploader_type.clone(),
            uploader_id: attachment.uploader_id.to_string(),
            filename: attachment.filename.clone(),
            url: attachment.url.clone(),
            download_url: stable_url.clone(),
            markdown_url: stable_url,
            content_type: attachment.content_type.clone(),
            size_bytes: attachment.size_bytes,
            created_at: timestamp(attachment.created_at),
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

    #[test]
    fn search_filter_matches_every_escaped_title_term_and_identifiers() {
        let (terms, number) = search_filter(Some("Fix 100%_safe"));
        assert_eq!(terms, vec!["%fix%", "%100\\%\\_safe%"]);
        assert_eq!(number, None);
        assert_eq!(search_filter(Some("CORD-42")).1, Some(42));
        assert_eq!(search_filter(Some("42")).1, Some(42));
        assert_eq!(search_filter(Some("CORD-extra-42")).1, None);
    }

    #[test]
    fn actor_and_property_filters_preserve_table_facet_semantics() {
        let id = "018f03a0-c4d2-7a37-ae4d-5aa45de12f11";
        let actors = actor_filters(Some(&format!("member:{id}")), "assignee_filters").unwrap();
        assert_eq!(actors.len(), 1);
        assert_eq!(actors[0].actor_type, "member");
        assert!(actor_filters(Some("unknown:value"), "assignee_filters").is_err());

        let groups =
            properties_filter(Some(&format!(r#"{{"{id}":["choice","__none__"]}}"#))).unwrap();
        assert_eq!(groups.len(), 1);
        assert!(groups[0].iter().any(
            |alternative| matches!(alternative, PropertyAlternative::Missing(value) if value == id)
        ));
    }

    #[test]
    fn update_parser_distinguishes_missing_null_and_value() {
        let fields = update_object(br#"{"assignee_id":null,"stage":4}"#).unwrap();
        assert!(matches!(
            update_field::<String>(&fields, "assignee_id").unwrap(),
            UpdateField::Null
        ));
        assert!(matches!(
            update_field::<i32>(&fields, "stage").unwrap(),
            UpdateField::Value(4)
        ));
        assert!(matches!(
            update_field::<String>(&fields, "project_id").unwrap(),
            UpdateField::Missing
        ));
    }

    #[test]
    fn legacy_prefix_fallback_matches_frozen_go_rule() {
        assert_eq!(legacy_issue_prefix("Frontend Team"), "FRO");
        assert_eq!(legacy_issue_prefix("前端团队"), "WS");
    }

    #[test]
    fn position_only_update_does_not_count_as_activity() {
        let issue = fixture_issue();
        let mut moved = issue.clone();
        moved.position = issue.position + 1.0;
        assert!(issue_mutable_fields_differ(&issue, &moved));
        assert!(!issue_activity_fields_differ(&issue, &moved));
    }

    #[test]
    fn status_update_counts_as_activity() {
        let issue = fixture_issue();
        let mut updated = issue.clone();
        updated.status = "in_review".into();
        assert!(issue_mutable_fields_differ(&issue, &updated));
        assert!(issue_activity_fields_differ(&issue, &updated));
    }
}
