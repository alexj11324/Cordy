//! Core issue routes — S8 authenticated issue-domain slice.
//!
//! Ports the stable list/query, detail, create/update/batch-update, children,
//! and issue-label contracts. The
//! workspace middleware resolves slugs/ids, verifies membership, and stamps a
//! `WorkspaceContext` before these handlers run.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{NaiveDate, SecondsFormat};
use patchbay_db::models::{
    AgentTaskQueue, Attachment, Issue, IssueLabel, IssueReaction, IssueSubscriber,
};
use patchbay_db::queries::issue_reaction::AddIssueReactionRow;
use patchbay_db::queries::{
    activity, agent, agent_invocation_target, attachment, autopilot, comment as comment_q,
    issue as issue_q, issue_label, issue_property, issue_reaction, member, quick_action, runtime,
    subscriber, task_usage, team, user, workspace,
};
use patchbay_middleware::workspace::{WorkspaceContext, WorkspaceGuardState};
use patchbay_service::issue_service::{
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
        .route("/api/assignee-frequency", get(get_assignee_frequency))
        .route("/api/issues", get(list_issues).post(create_issue))
        .route("/api/issues/", get(list_issues).post(create_issue))
        .route("/api/issues/query", post(query_issues))
        .route("/api/issues/search", get(search_issues))
        .route("/api/issues/grouped", get(grouped_issues))
        .route("/api/issues/table/rows", post(table_rows))
        .route("/api/issues/table/groups", post(table_groups))
        .route("/api/issues/table/facets", post(table_facets))
        .route("/api/issues/quick-create", post(quick_create_issue))
        .route("/api/issues/preview-trigger", post(preview_trigger))
        .route("/api/issues/batch-delete", post(batch_delete_issues))
        .route("/api/issues/child-progress", get(child_issue_progress))
        .route("/api/issues/children", get(list_children_by_parents))
        .route("/api/issues/batch-update", post(batch_update_issues))
        .route(
            "/api/issues/{id}",
            get(get_issue).put(update_issue).delete(delete_issue),
        )
        .route(
            "/api/issues/{id}/",
            get(get_issue).put(update_issue).delete(delete_issue),
        )
        .route("/api/issues/{id}/move", post(move_issue))
        .route("/api/issues/{id}/children", get(list_child_issues))
        .route("/api/issues/{id}/usage", get(get_issue_usage))
        .route("/api/issues/{id}/attachments", get(list_attachments))
        .route("/api/issues/{id}/active-task", get(get_active_tasks))
        .route("/api/issues/{id}/task-runs", get(list_task_runs))
        .route("/api/issues/{id}/timeline", get(issue_timeline))
        .route("/api/issues/{id}/rerun", post(rerun_issue))
        .route(
            "/api/issues/{id}/quick-actions/{quick_action_id}/render",
            post(render_quick_action),
        )
        .route(
            "/api/issues/{id}/quick-actions/{quick_action_id}/run",
            post(run_quick_action),
        )
        .route(
            "/api/issues/{id}/team-evaluated",
            post(record_team_evaluated),
        )
        .route(
            "/api/issues/{id}/pull-requests",
            get(crate::issue_pull_request::list)
                .post(crate::issue_pull_request::attach)
                .layer(DefaultBodyLimit::max(4 << 20)),
        )
        .route("/api/issues/{id}/tasks/{task_id}/cancel", post(cancel_task))
        .route("/api/issues/{id}/metadata", get(list_issue_metadata))
        .route(
            "/api/issues/{id}/metadata/{key}",
            axum::routing::put(set_issue_metadata_key).delete(delete_issue_metadata_key),
        )
        .route(
            "/api/issues/{id}/properties/{property_id}",
            axum::routing::put(set_issue_property).delete(unset_issue_property),
        )
        .route(
            "/api/issues/{id}/reactions",
            post(add_issue_reaction).delete(remove_issue_reaction),
        )
        .route("/api/issues/{id}/subscribers", get(list_issue_subscribers))
        .route("/api/issues/{id}/subscribe", post(subscribe_to_issue))
        .route("/api/issues/{id}/unsubscribe", post(unsubscribe_from_issue))
        .route(
            "/api/issues/{id}/unsubscribe/subtree",
            post(unsubscribe_from_issue_subtree),
        )
        .route(
            "/api/issues/{id}/labels",
            get(list_labels_for_issue).post(attach_label),
        )
        .route(
            "/api/issues/{id}/labels/{label_id}",
            axum::routing::delete(detach_label),
        )
}

fn context_workspace(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "workspace not found"))
}

const ISSUE_COLUMNS: &str = "id, workspace_id, title, description, status, priority, assignee_type, assignee_id, creator_type, creator_id, parent_issue_id, acceptance_criteria, context_refs, position, due_date, created_at, updated_at, number, project_id, origin_type, origin_id, first_executed_at, start_date, metadata, stage, properties, revision, last_activity_at, reviewer_type, reviewer_id";

fn search_patterns(raw: &str) -> Vec<String> {
    raw.split_whitespace()
        .filter(|term| !term.is_empty())
        .map(|term| {
            format!(
                "%{}%",
                term.to_lowercase()
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            )
        })
        .collect()
}

fn search_number(raw: &str) -> Option<i32> {
    let raw = raw.trim();
    let parsed = if let Some((prefix, number)) = raw.split_once('-') {
        (!prefix.is_empty() && prefix.chars().all(|ch| ch.is_ascii_alphabetic()))
            .then(|| number.parse::<i32>().ok())
            .flatten()
    } else {
        raw.parse::<i32>().ok()
    };
    parsed.filter(|number| *number > 0)
}

fn push_search_membership(
    query: &mut QueryBuilder<'_, Postgres>,
    workspace_id: Uuid,
    include_closed: bool,
    patterns: &[String],
    number: Option<i32>,
) {
    query.push("i.workspace_id = ").push_bind(workspace_id);
    if !include_closed {
        query.push(
            " AND issue_effective_status(i.workspace_id,i.status) NOT IN ('done','cancelled')",
        );
    }
    query.push(" AND (");
    let mut has_alternative = false;
    if let Some(number) = number {
        query.push("i.number = ").push_bind(number);
        has_alternative = true;
    }
    if !patterns.is_empty() {
        if has_alternative {
            query.push(" OR ");
        }
        query.push("(");
        for (index, pattern) in patterns.iter().enumerate() {
            if index > 0 {
                query.push(" AND ");
            }
            query.push("(LOWER(i.title) LIKE ").push_bind(pattern.clone()).push(" ESCAPE '\\\\' OR LOWER(COALESCE(i.description,'')) LIKE ").push_bind(pattern.clone()).push(" ESCAPE '\\\\' OR EXISTS(SELECT 1 FROM comment c WHERE c.issue_id=i.id AND c.workspace_id=").push_bind(workspace_id).push(" AND LOWER(c.content) LIKE ").push_bind(pattern.clone()).push(" ESCAPE '\\\\'))");
        }
        query.push(")");
    }
    query.push(")");
}

fn search_snippet(raw: &str, query: &str) -> String {
    const MAX: usize = 240;
    let chars = raw.chars().collect::<Vec<_>>();
    if chars.len() <= MAX {
        return raw.to_string();
    }
    // `str::to_lowercase` can change the number of bytes (for example, some
    // Unicode characters expand when case-folded), so an offset in the folded
    // string is not safe to use as a byte offset into `raw`. Keep a mapping for
    // every folded byte back to the original character index instead.
    let mut folded = String::new();
    let mut folded_byte_to_char = Vec::new();
    for (char_index, character) in raw.chars().enumerate() {
        for folded_character in character.to_lowercase() {
            let byte_len = folded_character.len_utf8();
            folded.push(folded_character);
            folded_byte_to_char.extend(std::iter::repeat_n(char_index, byte_len));
        }
    }
    let char_index = folded
        .find(&query.to_lowercase())
        .and_then(|byte_index| folded_byte_to_char.get(byte_index).copied())
        .unwrap_or(0);
    let start = char_index.saturating_sub(60).min(chars.len() - MAX);
    let mut result = chars[start..start + MAX].iter().collect::<String>();
    if start > 0 {
        result.insert(0, '…');
    }
    if start + MAX < chars.len() {
        result.push('…');
    }
    result
}

async fn search_issues(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let query = params
        .get("q")
        .map(|value| value.trim())
        .unwrap_or_default();
    if query.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "q parameter is required");
    }
    let workspace_id = match context_workspace(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let limit = params
        .get("limit")
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(20)
        .min(50);
    let offset = params
        .get("offset")
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(0);
    let include_closed = params
        .get("include_closed")
        .is_some_and(|value| value == "true");
    let patterns = search_patterns(query);
    let number = search_number(query).filter(|number| *number > 0);
    let phrase = format!(
        "%{}%",
        query
            .to_lowercase()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let mut statement =
        QueryBuilder::<Postgres>::new(format!("SELECT {ISSUE_COLUMNS} FROM issue i WHERE "));
    push_search_membership(
        &mut statement,
        workspace_id,
        include_closed,
        &patterns,
        number,
    );
    statement.push(" ORDER BY CASE ");
    if let Some(number) = number {
        statement
            .push("WHEN i.number = ")
            .push_bind(number)
            .push(" THEN 0 ");
    }
    statement.push("WHEN LOWER(i.title) = ").push_bind(query.to_lowercase()).push(" THEN 1 WHEN LOWER(i.title) LIKE ").push_bind(phrase.clone()).push(" ESCAPE '\\\\' THEN 2 WHEN LOWER(COALESCE(i.description,'')) LIKE ").push_bind(phrase.clone()).push(" ESCAPE '\\\\' THEN 3 ELSE 4 END, CASE i.status WHEN 'in_progress' THEN 0 WHEN 'in_review' THEN 1 WHEN 'todo' THEN 2 ELSE 3 END, i.updated_at DESC, i.id DESC LIMIT ").push_bind(limit).push(" OFFSET ").push_bind(offset);
    let issues = match statement
        .build_query_as::<Issue>()
        .fetch_all(&state.pool)
        .await
    {
        Ok(issues) => issues,
        Err(error) => {
            tracing::warn!(%error, "failed to search issues");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to search issues");
        }
    };
    let mut count = QueryBuilder::<Postgres>::new("SELECT count(*) FROM issue i WHERE ");
    push_search_membership(&mut count, workspace_id, include_closed, &patterns, number);
    let total = match count
        .build_query_scalar::<i64>()
        .fetch_one(&state.pool)
        .await
    {
        Ok(total) => total,
        Err(error) => {
            tracing::warn!(%error, "failed to count search results");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to count search results",
            );
        }
    };
    let prefix = issue_prefix(&state, workspace_id).await;
    let mut response = Vec::with_capacity(issues.len());
    let terms = query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    for issue in issues {
        let source = if terms
            .iter()
            .all(|term| issue.title.to_lowercase().contains(term))
        {
            "title"
        } else if issue
            .description
            .as_deref()
            .is_some_and(|value| terms.iter().all(|term| value.to_lowercase().contains(term)))
        {
            "description"
        } else {
            "comment"
        };
        let matching_comment = if source == "comment" || patterns.len() > 1 {
            let mut comment =
                QueryBuilder::<Postgres>::new("SELECT content FROM comment c WHERE c.issue_id=");
            comment
                .push_bind(issue.id)
                .push(" AND c.workspace_id=")
                .push_bind(workspace_id)
                .push(" AND (");
            for (index, pattern) in patterns.iter().enumerate() {
                if index > 0 {
                    comment.push(" AND ");
                }
                comment
                    .push("LOWER(c.content) LIKE ")
                    .push_bind(pattern.clone())
                    .push(" ESCAPE '\\\\'");
            }
            comment.push(") ORDER BY c.created_at DESC LIMIT 1");
            comment
                .build_query_scalar::<String>()
                .fetch_optional(&state.pool)
                .await
                .unwrap_or(None)
        } else {
            None
        };
        let mut value = serde_json::to_value(IssueResponse::from_issue(&issue, &prefix))
            .unwrap_or_else(|_| json!({}));
        if let Some(object) = value.as_object_mut() {
            object.insert("match_source".into(), json!(source));
            if let Some(content) = matching_comment {
                let snippet = search_snippet(&content, query);
                object.insert("matched_comment_snippet".into(), json!(snippet));
                if source == "comment" {
                    object.insert("matched_snippet".into(), json!(snippet));
                }
            }
            if issue
                .description
                .as_deref()
                .is_some_and(|text| terms.iter().all(|term| text.to_lowercase().contains(term)))
            {
                object.insert(
                    "matched_description_snippet".into(),
                    json!(search_snippet(
                        issue.description.as_deref().unwrap_or_default(),
                        query
                    )),
                );
            }
        }
        response.push(value);
    }
    let mut http = Json(json!({ "issues": response, "total": total })).into_response();
    if let Ok(value) = total.to_string().parse() {
        http.headers_mut().insert("x-total-count", value);
    }
    http
}

async fn grouped_issues(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<ListParams>,
) -> Response {
    if params.group_by.as_deref().unwrap_or("assignee") != "assignee" {
        return error_response(StatusCode::BAD_REQUEST, "unsupported group_by");
    }
    let workspace_id = match context_workspace(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let limit = params
        .limit
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(50)
        .clamp(1, 100);
    let offset = params
        .offset
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0)
        .max(0);
    let mut filters = serde_json::Map::new();
    let list = |raw: Option<&str>| {
        raw.unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(|v| json!(v))
            .collect::<Vec<_>>()
    };
    let statuses = list(params.statuses.as_deref().or(params.status.as_deref()));
    if !statuses.is_empty() {
        filters.insert("statuses".into(), Value::Array(statuses));
    }
    let priorities = list(params.priorities.as_deref().or(params.priority.as_deref()));
    if !priorities.is_empty() {
        filters.insert("priorities".into(), Value::Array(priorities));
    }
    if let Some(raw) = params.assignee_filters.as_deref() {
        let actors = raw
            .split(',')
            .filter_map(|entry| entry.split_once(':'))
            .map(|(kind, id)| json!({"type":kind,"id":id}))
            .collect::<Vec<_>>();
        filters.insert("assignees".into(), Value::Array(actors));
    }
    if params.include_no_assignee.as_deref() == Some("true") {
        filters.insert("include_no_assignee".into(), Value::Bool(true));
    }
    let projects = list(
        params
            .project_ids
            .as_deref()
            .or(params.project_id.as_deref()),
    );
    if !projects.is_empty() {
        filters.insert("project_ids".into(), Value::Array(projects));
    }
    if params.include_no_project.as_deref() == Some("true") {
        filters.insert("include_no_project".into(), Value::Bool(true));
    }
    let labels = list(params.label_ids.as_deref());
    if !labels.is_empty() {
        filters.insert("label_ids".into(), Value::Array(labels));
    }
    if params.top_level_only.as_deref() == Some("true") {
        filters.insert("include_sub_issues".into(), Value::Bool(false));
    }
    let request = TableRequest {
        query: json!({"scope":{"kind":"workspace"},"filters":filters,"search":params.q.clone().unwrap_or_default(),"sort":{"field":params.sort.clone().unwrap_or_else(|| "position".into()),"direction":params.direction.clone().unwrap_or_else(|| "asc".into())}}),
        group: Value::Null,
        group_key: None,
        hierarchy: Value::Null,
        parent_id: None,
        facets: Vec::new(),
        page: Value::Null,
    };
    let rows = match table_all_rows(&state, workspace_id, context.member.user_id, &request).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(status = %error.status(), "failed to list grouped issues");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list grouped issues",
            );
        }
    };
    let prefix = issue_prefix(&state, workspace_id).await;
    type IssueGroup = (Option<String>, Option<Uuid>, Vec<IssueResponse>);
    let mut grouped: std::collections::BTreeMap<String, IssueGroup> =
        std::collections::BTreeMap::new();
    for row in rows {
        let key = match (&row.assignee_type, row.assignee_id) {
            (Some(kind), Some(id)) => format!("{kind}:{id}"),
            _ => "none".to_string(),
        };
        grouped
            .entry(key)
            .or_insert_with(|| (row.assignee_type.clone(), row.assignee_id, Vec::new()))
            .2
            .push(IssueResponse::from_issue(&row, &prefix));
    }
    let groups = grouped
        .into_iter()
        .map(|(id, (kind, actor_id, issues))| {
            let total = issues.len();
            let issues = issues
                .into_iter()
                .skip(offset as usize)
                .take(limit as usize)
                .collect::<Vec<_>>();
            json!({
                "id": id, "assignee_type": kind, "assignee_id": actor_id,
                "total": total, "issues": issues,
            })
        })
        .collect::<Vec<_>>();
    Json(json!({ "groups": groups })).into_response()
}

#[derive(Debug, Default, Clone, Deserialize)]
struct TableRequest {
    #[serde(default)]
    query: Value,
    #[serde(default)]
    group: Value,
    #[serde(default)]
    group_key: Option<String>,
    #[serde(default)]
    hierarchy: Value,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    facets: Vec<Value>,
    #[serde(default)]
    page: Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct TableCursor {
    v: u8,
    query: String,
    offset: i64,
    #[serde(default)]
    last_id: Option<String>,
    #[serde(default)]
    group_key: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
}

fn table_fingerprint(request: &TableRequest) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&request.query).unwrap_or_default();
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn table_cursor(request: &TableRequest, fingerprint: &str) -> Result<(i64, i64), Response> {
    let limit = request
        .page
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "page.limit must be between 1 and 100",
        ));
    }
    let Some(raw) = request
        .page
        .get("cursor")
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
    else {
        return Ok((limit, 0));
    };
    let decoded = URL_SAFE_NO_PAD
        .decode(raw)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<TableCursor>(&bytes).ok())
        .filter(|cursor| cursor.v == 1)
        .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "invalid cursor"))?;
    let group_binding = request.group_key.clone().or_else(|| {
        request
            .group
            .get("kind")
            .and_then(Value::as_str)
            .map(|kind| {
                format!(
                    "group:{kind}:{}",
                    request
                        .group
                        .get("property_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                )
            })
    });
    if decoded.query != fingerprint
        || decoded.group_key != group_binding
        || decoded.parent_id != request.parent_id
    {
        return Err(error_response(
            StatusCode::CONFLICT,
            "cursor does not belong to this table query",
        ));
    }
    Ok((limit, decoded.offset.max(0)))
}

fn table_row_cursor(
    request: &TableRequest,
    fingerprint: &str,
) -> Result<(i64, Option<Uuid>), Response> {
    let limit = request
        .page
        .get("limit")
        .and_then(Value::as_i64)
        .unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "page.limit must be between 1 and 100",
        ));
    }
    let Some(raw) = request
        .page
        .get("cursor")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok((limit, None));
    };
    let decoded = URL_SAFE_NO_PAD
        .decode(raw)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<TableCursor>(&bytes).ok())
        .filter(|cursor| cursor.v == 1)
        .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "invalid cursor"))?;
    let group_binding = request.group_key.clone().or_else(|| {
        request
            .group
            .get("kind")
            .and_then(Value::as_str)
            .map(|kind| {
                format!(
                    "group:{kind}:{}",
                    request
                        .group
                        .get("property_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                )
            })
    });
    if decoded.query != fingerprint
        || decoded.group_key != group_binding
        || decoded.parent_id != request.parent_id
    {
        return Err(error_response(
            StatusCode::CONFLICT,
            "cursor does not belong to this table query",
        ));
    }
    let last_id = decoded
        .last_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid cursor"))?;
    if last_id.is_none() {
        return Err(error_response(
            StatusCode::CONFLICT,
            "cursor requires a fresh table query",
        ));
    }
    Ok((limit, last_id))
}

fn encode_table_cursor(request: &TableRequest, fingerprint: &str, offset: i64) -> String {
    let group_key = request.group_key.clone().or_else(|| {
        request
            .group
            .get("kind")
            .and_then(Value::as_str)
            .map(|kind| {
                format!(
                    "group:{kind}:{}",
                    request
                        .group
                        .get("property_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                )
            })
    });
    URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&TableCursor {
            v: 1,
            query: fingerprint.into(),
            offset,
            last_id: None,
            group_key,
            parent_id: request.parent_id.clone(),
        })
        .unwrap_or_default(),
    )
}

fn encode_table_row_cursor(request: &TableRequest, fingerprint: &str, last_id: Uuid) -> String {
    let group_key = request.group_key.clone().or_else(|| {
        request
            .group
            .get("kind")
            .and_then(Value::as_str)
            .map(|kind| {
                format!(
                    "group:{kind}:{}",
                    request
                        .group
                        .get("property_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                )
            })
    });
    URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&TableCursor {
            v: 1,
            query: fingerprint.into(),
            offset: 0,
            last_id: Some(last_id.to_string()),
            group_key,
            parent_id: request.parent_id.clone(),
        })
        .unwrap_or_default(),
    )
}

fn parse_table_uuid(raw: &Value, field: &str) -> Result<Uuid, Response> {
    raw.as_str()
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, &format!("invalid {field}")))
}

fn push_table_filters(
    builder: &mut QueryBuilder<'_, Postgres>,
    request: &TableRequest,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), Response> {
    builder.push("i.workspace_id=").push_bind(workspace_id);
    let scope = request.query.get("scope").unwrap_or(&Value::Null);
    match scope
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("workspace")
    {
        "workspace" => {}
        "project" => {
            builder
                .push(" AND i.project_id=")
                .push_bind(parse_table_uuid(&scope["project_id"], "scope.project_id")?);
        }
        "assignee" | "creator" => {
            let kind = scope["kind"].as_str().unwrap();
            let actor = scope.get("actor").ok_or_else(|| {
                error_response(StatusCode::BAD_REQUEST, "scope.actor is required")
            })?;
            let actor_type = actor
                .get("type")
                .and_then(Value::as_str)
                .filter(|v| matches!(*v, "member" | "agent" | "team"))
                .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "invalid scope.actor"))?;
            let id = parse_table_uuid(&actor["id"], "scope.actor")?;
            builder
                .push(" AND i.")
                .push(kind)
                .push("_type=")
                .push_bind(actor_type.to_string())
                .push(" AND i.")
                .push(kind)
                .push("_id=")
                .push_bind(id);
        }
        "my" => match scope
            .get("relation")
            .and_then(Value::as_str)
            .unwrap_or("any")
        {
            "assigned" => {
                builder
                    .push(" AND i.assignee_type='member' AND i.assignee_id=")
                    .push_bind(user_id);
            }
            "created" => {
                builder
                    .push(" AND i.creator_type='member' AND i.creator_id=")
                    .push_bind(user_id);
            }
            "involved" => {
                builder.push(" AND ((i.assignee_type='agent' AND i.assignee_id IN(SELECT id FROM agent WHERE workspace_id=").push_bind(workspace_id).push(" AND owner_id=").push_bind(user_id).push(")) OR (i.assignee_type='team' AND i.assignee_id IN(SELECT sm.team_id FROM team_member sm JOIN team s ON s.id=sm.team_id WHERE s.workspace_id=").push_bind(workspace_id).push(" AND ((sm.member_type='member' AND sm.member_id=").push_bind(user_id).push(") OR (sm.member_type='agent' AND sm.member_id IN(SELECT id FROM agent WHERE workspace_id=").push_bind(workspace_id).push(" AND owner_id=").push_bind(user_id).push(")))))");
            }
            "any" => {
                builder.push(" AND ((i.assignee_type='member' AND i.assignee_id=").push_bind(user_id).push(") OR (i.creator_type='member' AND i.creator_id=").push_bind(user_id).push(") OR (i.assignee_type='agent' AND i.assignee_id IN(SELECT id FROM agent WHERE workspace_id=").push_bind(workspace_id).push(" AND owner_id=").push_bind(user_id).push(")) OR (i.assignee_type='team' AND i.assignee_id IN(SELECT sm.team_id FROM team_member sm JOIN team s ON s.id=sm.team_id WHERE s.workspace_id=").push_bind(workspace_id).push(" AND ((sm.member_type='member' AND sm.member_id=").push_bind(user_id).push(") OR (sm.member_type='agent' AND sm.member_id IN(SELECT id FROM agent WHERE workspace_id=").push_bind(workspace_id).push(" AND owner_id=").push_bind(user_id).push("))))))");
            }
            _ => {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid scope.relation",
                ));
            }
        },
        other => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                &format!("unsupported scope.kind: {other}"),
            ));
        }
    }
    if let Some(types) = scope.get("assignee_types").and_then(Value::as_array) {
        let values = types
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        if values
            .iter()
            .any(|v| !matches!(v.as_str(), "member" | "agent" | "team"))
        {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid scope.assignee_types",
            ));
        }
        if !values.is_empty() {
            builder
                .push(" AND i.assignee_type=ANY(")
                .push_bind(values)
                .push(")");
        }
    }
    let filters = request.query.get("filters").unwrap_or(&Value::Null);
    for (field, column) in [("statuses", "status"), ("priorities", "priority")] {
        if let Some(values) = filters.get(field).and_then(Value::as_array) {
            let values = values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>();
            if !values.is_empty() {
                builder
                    .push(" AND i.")
                    .push(column)
                    .push("=ANY(")
                    .push_bind(values)
                    .push(")");
            }
        }
    }
    for (field, type_col, id_col) in [
        ("assignees", "assignee_type", "assignee_id"),
        ("creators", "creator_type", "creator_id"),
    ] {
        if let Some(actors) = filters.get(field).and_then(Value::as_array) {
            if actors.is_empty() && field == "assignees" {
                builder.push(" AND FALSE");
                continue;
            }
            if !actors.is_empty() {
                builder.push(" AND (");
                for (index, actor) in actors.iter().enumerate() {
                    if index > 0 {
                        builder.push(" OR ");
                    }
                    let actor_type = actor
                        .get("type")
                        .and_then(Value::as_str)
                        .filter(|v| matches!(*v, "member" | "agent" | "team"))
                        .ok_or_else(|| {
                            error_response(
                                StatusCode::BAD_REQUEST,
                                &format!("invalid filters.{field}"),
                            )
                        })?;
                    let id = parse_table_uuid(&actor["id"], &format!("filters.{field}"))?;
                    builder
                        .push("(i.")
                        .push(type_col)
                        .push("=")
                        .push_bind(actor_type.to_string())
                        .push(" AND i.")
                        .push(id_col)
                        .push("=")
                        .push_bind(id)
                        .push(")");
                }
                if field == "assignees"
                    && filters
                        .get("include_no_assignee")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                {
                    builder.push(" OR (i.assignee_type IS NULL AND i.assignee_id IS NULL)");
                }
                builder.push(")");
            }
        } else if field == "assignees"
            && filters
                .get("include_no_assignee")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            builder.push(" AND i.assignee_id IS NULL");
        }
    }
    if let Some(values) = filters.get("project_ids").and_then(Value::as_array) {
        let ids = values
            .iter()
            .map(|v| parse_table_uuid(v, "filters.project_ids"))
            .collect::<Result<Vec<_>, _>>()?;
        let include_none = filters
            .get("include_no_project")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let has_ids = !ids.is_empty();
        if has_ids || include_none {
            builder.push(" AND (");
            if has_ids {
                builder.push("i.project_id=ANY(").push_bind(ids).push(")");
            }
            if include_none {
                if has_ids {
                    builder.push(" OR ");
                }
                builder.push("i.project_id IS NULL");
            }
            builder.push(")");
        }
    }
    if let Some(values) = filters.get("label_ids").and_then(Value::as_array) {
        let ids = values
            .iter()
            .map(|v| parse_table_uuid(v, "filters.label_ids"))
            .collect::<Result<Vec<_>, _>>()?;
        if !ids.is_empty() {
            builder.push(" AND EXISTS(SELECT 1 FROM issue_to_label itl WHERE itl.issue_id=i.id AND itl.label_id=ANY(").push_bind(ids).push("))");
        }
    }
    if let Some(properties) = filters.get("properties").and_then(Value::as_object) {
        let mut alternatives = 0usize;
        for (definition_id, values) in properties {
            Uuid::parse_str(definition_id).map_err(|_| {
                error_response(StatusCode::BAD_REQUEST, "invalid filters.properties")
            })?;
            let values = values.as_array().ok_or_else(|| {
                error_response(StatusCode::BAD_REQUEST, "invalid filters.properties")
            })?;
            if values.is_empty() {
                continue;
            }
            alternatives += values.len();
            if alternatives > 256 {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "properties filter is too large",
                ));
            }
            builder.push(" AND (");
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    builder.push(" OR ");
                }
                if value
                    .as_str()
                    .is_some_and(|value| matches!(value, "__none__" | "unset:"))
                {
                    builder
                        .push("NOT(i.properties ? ")
                        .push_bind(definition_id.clone())
                        .push(")");
                } else {
                    let alternatives = property_filter_containment(definition_id, value);
                    builder.push("(");
                    for (alt_index, alternative) in alternatives.into_iter().enumerate() {
                        if alt_index > 0 {
                            builder.push(" OR ");
                        }
                        builder.push("i.properties @> ").push_bind(alternative);
                    }
                    builder.push(")");
                }
            }
            builder.push(")");
        }
    }
    if let Some(values) = filters.get("working_issue_ids").and_then(Value::as_array) {
        let ids = values
            .iter()
            .map(|v| parse_table_uuid(v, "filters.working_issue_ids"))
            .collect::<Result<Vec<_>, _>>()?;
        if ids.is_empty() {
            builder.push(" AND FALSE");
        } else {
            builder.push(" AND i.id=ANY(").push_bind(ids).push(")");
        }
    }
    if let Some(date) = filters.get("date").and_then(Value::as_object) {
        let field = date
            .get("field")
            .and_then(Value::as_str)
            .filter(|field| matches!(*field, "created_at" | "updated_at"))
            .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "invalid filters.date.field"))?;
        let start = date
            .get("start")
            .and_then(Value::as_str)
            .and_then(|raw| raw.parse::<chrono::DateTime<chrono::Utc>>().ok())
            .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "invalid filters.date range"))?;
        let end = date
            .get("end")
            .and_then(Value::as_str)
            .and_then(|raw| raw.parse::<chrono::DateTime<chrono::Utc>>().ok())
            .filter(|end| start < *end)
            .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "invalid filters.date range"))?;
        builder
            .push(" AND i.")
            .push(field)
            .push(">=")
            .push_bind(start)
            .push(" AND i.")
            .push(field)
            .push("<")
            .push_bind(end);
    }
    if filters.get("include_sub_issues").and_then(Value::as_bool) == Some(false) {
        builder.push(" AND i.parent_issue_id IS NULL");
    }
    if filters.get("working_only").and_then(Value::as_bool) == Some(true) {
        builder.push(" AND EXISTS(SELECT 1 FROM agent_task_queue atq WHERE atq.issue_id=i.id AND atq.status='running')");
    }
    if let Some(raw) = request
        .query
        .get("search")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        let patterns = search_patterns(raw);
        let number = search_number(raw).filter(|v| *v > 0);
        builder.push(" AND (");
        if let Some(number) = number {
            builder.push("i.number=").push_bind(number).push(" OR ");
        }
        for (index, pattern) in patterns.iter().enumerate() {
            if index > 0 {
                builder.push(" AND ");
            }
            builder.push("(LOWER(i.title) LIKE ").push_bind(pattern.clone()).push(" ESCAPE '\\\\' OR LOWER(COALESCE(i.description,'')) LIKE ").push_bind(pattern.clone()).push(" ESCAPE '\\\\' OR EXISTS(SELECT 1 FROM comment c WHERE c.issue_id=i.id AND c.workspace_id=").push_bind(workspace_id).push(" AND LOWER(c.content) LIKE ").push_bind(pattern.clone()).push(" ESCAPE '\\\\'))");
        }
        builder.push(")");
    }
    Ok(())
}

fn property_filter_containment(definition_id: &str, value: &Value) -> Vec<Value> {
    let mut alternatives = vec![json!({ definition_id: value })];
    if let Some(raw) = value.as_str() {
        alternatives.push(json!({ definition_id: [raw] }));
        if raw == "true" {
            alternatives.push(json!({ definition_id: true }));
        } else if raw == "false" {
            alternatives.push(json!({ definition_id: false }));
        }
    }
    alternatives
}

fn push_table_branch(
    builder: &mut QueryBuilder<'_, Postgres>,
    request: &TableRequest,
) -> Result<(), Response> {
    let hierarchy = request
        .hierarchy
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if request.parent_id.is_some() && !hierarchy {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "parent_id requires hierarchy.enabled=true",
        ));
    }
    if hierarchy {
        if let Some(parent) = request.parent_id.as_deref() {
            builder.push(" AND i.parent_issue_id=").push_bind(
                Uuid::parse_str(parent)
                    .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid parent_id"))?,
            );
        } else {
            builder.push(" AND i.parent_issue_id IS NULL");
        }
    }
    let Some(raw_key) = request.group_key.as_deref() else {
        return Ok(());
    };
    let kind = request
        .group
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("none");
    if kind == "none" {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "group_key requires a group",
        ));
    }
    let value = raw_key.strip_prefix(&format!("{kind}:")).unwrap_or(raw_key);
    match kind {
        "status" => {
            builder.push(" AND i.status=").push_bind(value.to_string());
        }
        "status_category" => {
            builder
                .push(" AND issue_effective_status(i.workspace_id,i.status)=")
                .push_bind(value.to_string());
        }
        "assignee" => {
            if matches!(value, "unassigned" | "__none__") {
                builder.push(" AND i.assignee_id IS NULL");
            } else {
                let (actor_type, id) = value
                    .split_once(':')
                    .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "invalid group_key"))?;
                builder
                    .push(" AND i.assignee_type=")
                    .push_bind(actor_type.to_string())
                    .push(" AND i.assignee_id=")
                    .push_bind(Uuid::parse_str(id).map_err(|_| {
                        error_response(StatusCode::BAD_REQUEST, "invalid group_key")
                    })?);
            }
        }
        "project" => {
            if matches!(value, "unassigned" | "__none__") {
                builder.push(" AND i.project_id IS NULL");
            } else {
                builder.push(" AND i.project_id=").push_bind(
                    Uuid::parse_str(value).map_err(|_| {
                        error_response(StatusCode::BAD_REQUEST, "invalid group_key")
                    })?,
                );
            }
        }
        "parent" => {
            if value == "root" {
                builder.push(" AND i.parent_issue_id IS NULL");
            } else {
                builder.push(" AND i.parent_issue_id=").push_bind(
                    Uuid::parse_str(value).map_err(|_| {
                        error_response(StatusCode::BAD_REQUEST, "invalid group_key")
                    })?,
                );
            }
        }
        "property" => {
            let property_id = request
                .group
                .get("property_id")
                .and_then(Value::as_str)
                .filter(|id| Uuid::parse_str(id).is_ok())
                .ok_or_else(|| {
                    error_response(StatusCode::BAD_REQUEST, "invalid group.property_id")
                })?;
            if value == "unset:" {
                builder
                    .push(" AND (NOT(i.properties ? ")
                    .push_bind(property_id.to_string())
                    .push(") OR i.properties->")
                    .push_bind(property_id.to_string())
                    .push(" = 'null'::jsonb OR CASE WHEN jsonb_typeof(i.properties->")
                    .push_bind(property_id.to_string())
                    .push(") = 'array' THEN jsonb_array_length(i.properties->")
                    .push_bind(property_id.to_string())
                    .push(") = 0 ELSE FALSE END)");
            } else {
                let value = value.trim_start_matches("value:").to_string();
                builder
                    .push(" AND (i.properties->>")
                    .push_bind(property_id.to_string())
                    .push("=")
                    .push_bind(value.clone())
                    .push(" OR EXISTS (SELECT 1 FROM jsonb_array_elements_text(CASE WHEN jsonb_typeof(i.properties->")
                    .push_bind(property_id.to_string())
                    .push(") = 'array' THEN i.properties->")
                    .push_bind(property_id.to_string())
                    .push(" ELSE '[]'::jsonb END) AS property_value(value) WHERE property_value.value=")
                    .push_bind(value)
                    .push("))");
            }
        }
        "compound" => push_compound_group_predicate(builder, request, value)?,
        _ => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "unsupported table group",
            ));
        }
    }
    Ok(())
}

fn push_compound_group_predicate(
    builder: &mut QueryBuilder<'_, Postgres>,
    request: &TableRequest,
    encoded_and_status: &str,
) -> Result<(), Response> {
    let secondary = request
        .group
        .get("secondary")
        .and_then(Value::as_str)
        .unwrap_or("status");
    let axis = match secondary {
        "status" => ":status:",
        "status_category" => ":status_category:",
        _ => return Err(error_response(StatusCode::BAD_REQUEST, "invalid group_key")),
    };
    let Some((encoded, status)) = encoded_and_status.split_once(axis) else {
        return Err(error_response(StatusCode::BAD_REQUEST, "invalid group_key"));
    };
    if status.is_empty() || status.len() > 64 {
        return Err(error_response(StatusCode::BAD_REQUEST, "invalid group_key"));
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "invalid group_key"))?;
    let primary = request
        .group
        .get("primary")
        .and_then(Value::as_str)
        .unwrap_or("assignee");
    push_group_dimension_predicate(builder, primary, &decoded)?;
    match secondary {
        "status_category" => {
            builder
                .push(" AND issue_effective_status(i.workspace_id,i.status)=")
                .push_bind(status.to_string());
        }
        _ => {
            builder.push(" AND i.status=").push_bind(status.to_string());
        }
    }
    Ok(())
}

fn push_group_dimension_predicate(
    builder: &mut QueryBuilder<'_, Postgres>,
    kind: &str,
    raw_key: &str,
) -> Result<(), Response> {
    let value = raw_key.strip_prefix(&format!("{kind}:")).unwrap_or(raw_key);
    match kind {
        "assignee" => {
            if matches!(value, "unassigned" | "__none__" | "__unassigned__") {
                builder.push(" AND i.assignee_id IS NULL");
            } else {
                let (actor_type, id) = value
                    .split_once(':')
                    .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "invalid group_key"))?;
                builder
                    .push(" AND i.assignee_type=")
                    .push_bind(actor_type.to_string())
                    .push(" AND i.assignee_id=")
                    .push_bind(Uuid::parse_str(id).map_err(|_| {
                        error_response(StatusCode::BAD_REQUEST, "invalid group_key")
                    })?);
            }
        }
        "project" => {
            if matches!(value, "unassigned" | "__none__" | "none" | "__no_project__") {
                builder.push(" AND i.project_id IS NULL");
            } else {
                builder.push(" AND i.project_id=").push_bind(
                    Uuid::parse_str(value).map_err(|_| {
                        error_response(StatusCode::BAD_REQUEST, "invalid group_key")
                    })?,
                );
            }
        }
        "parent" => {
            if matches!(value, "root" | "none" | "__no_parent__") {
                builder.push(" AND i.parent_issue_id IS NULL");
            } else {
                builder.push(" AND i.parent_issue_id=").push_bind(
                    Uuid::parse_str(value).map_err(|_| {
                        error_response(StatusCode::BAD_REQUEST, "invalid group_key")
                    })?,
                );
            }
        }
        _ => {
            return Err(error_response(StatusCode::BAD_REQUEST, "invalid group_key"));
        }
    }
    Ok(())
}

fn table_sort_column(field: &str) -> Option<&'static str> {
    match field {
        "position" => Some("i.position"),
        "title" => Some("i.title"),
        "created_at" => Some("i.created_at"),
        "updated_at" => Some("i.updated_at"),
        "last_activity" | "last_activity_at" => Some("i.last_activity_at"),
        "start_date" => Some("i.start_date"),
        "due_date" => Some("i.due_date"),
        "priority" => Some("i.priority"),
        "status" => Some("i.status"),
        "number" => Some("i.number"),
        _ => None,
    }
}

async fn table_sort_expression(
    state: &HandlerState,
    workspace_id: Uuid,
    request: &TableRequest,
) -> Result<(String, bool), Response> {
    let sort = request.query.get("sort").unwrap_or(&Value::Null);
    let field = sort
        .get("field")
        .and_then(Value::as_str)
        .unwrap_or("position");
    let column = if let Some(column) = table_sort_column(field) {
        column.to_string()
    } else if let Some(raw_id) = field.strip_prefix("property:") {
        let property_id = Uuid::parse_str(raw_id)
            .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid sort.field"))?;
        match issue_property::get_issue_property(&state.pool, property_id, workspace_id).await {
            Ok(Some(definition)) if definition.archived_at.is_none() => {
                let id = definition.id;
                match definition.type_.as_str() {
                    "number" => format!(
                        "CASE WHEN jsonb_typeof(i.properties->'{id}') = 'number' THEN (i.properties->>'{id}')::numeric END"
                    ),
                    "date" | "text" | "url" | "select" => {
                        format!("NULLIF(i.properties->>'{id}', '')")
                    }
                    _ => "i.position".into(),
                }
            }
            Ok(_) => "i.position".into(),
            Err(error) => {
                tracing::warn!(%error, "failed to resolve table sort property");
                return Err(error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to resolve table sort",
                ));
            }
        }
    } else {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid sort.field",
        ));
    };
    let descending = match sort
        .get("direction")
        .and_then(Value::as_str)
        .unwrap_or("asc")
    {
        "asc" => false,
        "desc" => true,
        _ => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid sort.direction",
            ));
        }
    };
    Ok((column, descending))
}

async fn table_order(
    state: &HandlerState,
    workspace_id: Uuid,
    request: &TableRequest,
) -> Result<String, Response> {
    let (column, descending) = table_sort_expression(state, workspace_id, request).await?;
    let direction = if descending { "DESC" } else { "ASC" };
    Ok(format!(
        "{column} {direction} NULLS LAST, i.created_at DESC, i.id DESC"
    ))
}

fn push_table_keyset(
    builder: &mut QueryBuilder<'_, Postgres>,
    expression: &str,
    descending: bool,
    last_id: Uuid,
) {
    // The cursor stores the last row id, so the database can read the exact
    // typed sort value from the same row. This preserves numeric/date ordering
    // for custom properties without serializing values into an untyped cursor.
    let last_expression = expression.replace("i.", "last_i.");
    let primary_comparison = if descending { "<" } else { ">" };
    builder.push(" AND (");
    builder.push("(").push(expression).push(" IS NULL AND ");
    push_table_last_value(builder, &last_expression, last_id);
    builder.push(" IS NOT NULL)");
    builder.push(" OR (");
    builder.push(expression).push(" IS NOT DISTINCT FROM ");
    push_table_last_value(builder, &last_expression, last_id);
    builder.push(" AND (");
    builder
        .push("i.created_at < (SELECT last_i.created_at FROM issue AS last_i WHERE last_i.id = ");
    builder.push_bind(last_id).push(")");
    builder.push(" OR (i.created_at IS NOT DISTINCT FROM (SELECT last_i.created_at FROM issue AS last_i WHERE last_i.id = ");
    builder.push_bind(last_id).push(") AND i.id < ");
    builder.push_bind(last_id).push(")");
    builder.push(")");
    builder.push(")");
    builder.push(" OR (");
    builder.push(expression).push(" IS NOT NULL AND ");
    push_table_last_value(builder, &last_expression, last_id);
    builder.push(" IS NOT NULL AND ");
    builder
        .push(expression)
        .push(' ')
        .push(primary_comparison)
        .push(' ');
    push_table_last_value(builder, &last_expression, last_id);
    builder.push(")");
    builder.push(")");
}

fn push_table_last_value(
    builder: &mut QueryBuilder<'_, Postgres>,
    expression: &str,
    last_id: Uuid,
) {
    builder
        .push("(SELECT ")
        .push(expression)
        .push(" FROM issue AS last_i WHERE last_i.id = ")
        .push_bind(last_id)
        .push(")");
}

async fn table_base_rows(
    state: &HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    request: &TableRequest,
) -> Result<(Vec<Issue>, i64, Option<String>), Response> {
    let fingerprint = table_fingerprint(request);
    let (limit, last_id) = table_row_cursor(request, &fingerprint)?;
    let order = table_order(state, workspace_id, request).await?;
    let (sort_expression, descending) = table_sort_expression(state, workspace_id, request).await?;
    let mut count = QueryBuilder::<Postgres>::new("SELECT count(*) FROM issue i WHERE ");
    push_table_filters(&mut count, request, workspace_id, user_id)?;
    push_table_branch(&mut count, request)?;
    let total = count
        .build_query_scalar::<i64>()
        .fetch_one(&state.pool)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to count issue table");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to query issue table",
            )
        })?;
    let mut query =
        QueryBuilder::<Postgres>::new(format!("SELECT {ISSUE_COLUMNS} FROM issue i WHERE "));
    push_table_filters(&mut query, request, workspace_id, user_id)?;
    push_table_branch(&mut query, request)?;
    if let Some(last_id) = last_id {
        push_table_keyset(&mut query, &sort_expression, descending, last_id);
    }
    query
        .push(" ORDER BY ")
        .push(order)
        .push(" LIMIT ")
        .push_bind(limit);
    let rows = query
        .build_query_as::<Issue>()
        .fetch_all(&state.pool)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to query issue table");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to query issue table",
            )
        })?;
    let next = (rows.len() as i64 == limit)
        .then(|| {
            rows.last()
                .map(|row| encode_table_row_cursor(request, &fingerprint, row.id))
        })
        .flatten();
    Ok((rows, total, next))
}

async fn table_all_rows(
    state: &HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    request: &TableRequest,
) -> Result<Vec<Issue>, Response> {
    let order = table_order(state, workspace_id, request).await?;
    let mut query =
        QueryBuilder::<Postgres>::new(format!("SELECT {ISSUE_COLUMNS} FROM issue i WHERE "));
    push_table_filters(&mut query, request, workspace_id, user_id)?;
    query.push(" ORDER BY ").push(order);
    query
        .build_query_as::<Issue>()
        .fetch_all(&state.pool)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to query issue table");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to query issue table",
            )
        })
}

async fn table_rows(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<TableRequest>,
) -> Response {
    let workspace_id = match context_workspace(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let (rows, total, next_cursor) =
        match table_base_rows(&state, workspace_id, context.member.user_id, &request).await {
            Ok(rows) => rows,
            Err(response) => return response,
        };
    let prefix = issue_prefix(&state, workspace_id).await;
    let ids = rows.iter().map(|issue| issue.id).collect::<Vec<_>>();
    let child_counts = match sqlx::query_as::<_, (Uuid, i64)>("SELECT parent_issue_id, count(*)::bigint FROM issue WHERE workspace_id=$1 AND parent_issue_id=ANY($2) GROUP BY parent_issue_id")
        .bind(workspace_id).bind(&ids).fetch_all(&state.pool).await {
            Ok(rows) => rows.into_iter().collect::<HashMap<_, _>>(),
            Err(error) => {
                tracing::warn!(%error, "failed to count issue table children");
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to query issue table");
            }
        };
    let mut labels = labels_for_issues(&state, workspace_id, &ids)
        .await
        .unwrap_or_default();
    let mut status_resolver = patchbay_service::issue_status::Resolver::new(workspace_id);
    let mut response_rows = Vec::with_capacity(rows.len());
    for issue in &rows {
        let mut response = IssueResponse::from_issue(issue, &prefix);
        response.status_category =
            Some(status_resolver.effective(&state.pool, &issue.status).await);
        response.labels = Some(labels.remove(&issue.id).unwrap_or_default());
        response_rows.push(json!({
            "issue": response,
            "direct_child_count": child_counts.get(&issue.id).copied().unwrap_or(0),
        }));
    }
    Json(json!({
        "query_fingerprint": table_fingerprint(&request), "group_key": request.group_key,
        "parent_id": request.parent_id, "total": total, "rows": response_rows,
        "branch_total": total, "next_cursor": next_cursor,
    }))
    .into_response()
}

fn table_dimension_expr(kind: &str, property_id: Option<&str>) -> Result<String, Response> {
    Ok(match kind {
        "status" => "i.status".into(),
        "status_category" => "issue_effective_status(i.workspace_id, i.status)".into(),
        "priority" => "i.priority".into(),
        "assignee" => "CASE WHEN i.assignee_id IS NULL THEN 'unassigned' ELSE i.assignee_type || ':' || i.assignee_id::text END".into(),
        "creator" => "i.creator_type || ':' || i.creator_id::text".into(),
        "project" => "COALESCE(i.project_id::text, 'unassigned')".into(),
        "parent" => "COALESCE(i.parent_issue_id::text, 'root')".into(),
        "property" => {
            let id = property_id
                .filter(|id| Uuid::parse_str(id).is_ok())
                .ok_or_else(|| {
                    error_response(StatusCode::BAD_REQUEST, "invalid group.property_id")
                })?;
            format!(
                "CASE WHEN NOT (i.properties ? '{id}') THEN 'unset:' ELSE COALESCE(i.properties->>'{id}', 'unset:') END"
            )
        }
        _ => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "unsupported table group",
            ))
        }
    })
}

async fn table_filtered_count(
    state: &HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    request: &TableRequest,
) -> Result<i64, Response> {
    let mut count = QueryBuilder::<Postgres>::new("SELECT count(*) FROM issue i WHERE ");
    push_table_filters(&mut count, request, workspace_id, user_id)?;
    count
        .build_query_scalar::<i64>()
        .fetch_one(&state.pool)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to count issue table");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to query issue table",
            )
        })
}

async fn table_grouped_counts(
    state: &HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    request: &TableRequest,
    expr: &str,
) -> Result<Vec<(String, i64)>, Response> {
    let mut query = QueryBuilder::<Postgres>::new(format!(
        "SELECT {expr} AS group_value, count(*)::bigint FROM issue i WHERE "
    ));
    push_table_filters(&mut query, request, workspace_id, user_id)?;
    query.push(" GROUP BY 1");
    query
        .build_query_as::<(Option<String>, i64)>()
        .fetch_all(&state.pool)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|(key, count)| (key.unwrap_or_default(), count))
                .collect()
        })
        .map_err(|error| {
            tracing::warn!(%error, "failed to aggregate issue table groups");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to query issue table",
            )
        })
}

async fn table_property_grouped_counts(
    state: &HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    request: &TableRequest,
    property_id: &str,
    group_keys: bool,
) -> Result<Vec<(String, i64)>, Response> {
    let property_id = Uuid::parse_str(property_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid group.property_id"))?
        .to_string();
    // The property id has been parsed as a UUID before interpolation. Values
    // are expanded in SQL so an issue contributes once to every selected
    // option, rather than being grouped under the JSON array's serialization.
    let value_expression = if group_keys {
        "'value:' || property_element.value"
    } else {
        "property_element.value"
    };
    let empty_expression = if group_keys { "'unset:'" } else { "'__none__'" };
    let mut query = QueryBuilder::<Postgres>::new(format!(
        "SELECT property_group.group_value, count(DISTINCT i.id)::bigint FROM issue i CROSS JOIN LATERAL (SELECT {empty_expression}::text AS group_value WHERE NOT (i.properties ? '{property_id}') OR i.properties -> '{property_id}' = 'null'::jsonb OR CASE WHEN jsonb_typeof(i.properties -> '{property_id}') = 'array' THEN jsonb_array_length(i.properties -> '{property_id}') = 0 ELSE FALSE END UNION ALL SELECT {value_expression} AS group_value FROM jsonb_array_elements_text(CASE WHEN jsonb_typeof(i.properties -> '{property_id}') = 'array' THEN i.properties -> '{property_id}' ELSE jsonb_build_array(i.properties ->> '{property_id}') END) AS property_element(value) WHERE i.properties ? '{property_id}' AND i.properties -> '{property_id}' <> 'null'::jsonb AND CASE WHEN jsonb_typeof(i.properties -> '{property_id}') = 'array' THEN jsonb_array_length(i.properties -> '{property_id}') > 0 ELSE TRUE END) AS property_group WHERE "
    ));
    push_table_filters(&mut query, request, workspace_id, user_id)?;
    query.push(" GROUP BY 1");
    query
        .build_query_as::<(Option<String>, i64)>()
        .fetch_all(&state.pool)
        .await
        .map(|rows| {
            rows.into_iter()
                .filter_map(|(key, count)| key.map(|key| (key, count)))
                .collect()
        })
        .map_err(|error| {
            tracing::warn!(%error, "failed to aggregate issue property groups");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to query issue table",
            )
        })
}

async fn table_compound_counts(
    state: &HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    request: &TableRequest,
    primary_expr: &str,
    secondary_expr: &str,
) -> Result<Vec<(String, String, i64)>, Response> {
    let mut query = QueryBuilder::<Postgres>::new(format!(
        "SELECT {primary_expr} AS primary_value, {secondary_expr} AS secondary_value, count(*)::bigint FROM issue i WHERE "
    ));
    push_table_filters(&mut query, request, workspace_id, user_id)?;
    query.push(" GROUP BY 1, 2");
    query
        .build_query_as::<(Option<String>, Option<String>, i64)>()
        .fetch_all(&state.pool)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|(primary, secondary, count)| {
                    (
                        primary.unwrap_or_default(),
                        secondary.unwrap_or_default(),
                        count,
                    )
                })
                .collect()
        })
        .map_err(|error| {
            tracing::warn!(%error, "failed to aggregate compound issue table groups");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to query issue table",
            )
        })
}

fn compound_cell_group_key(primary_key: &str, status: &str, category: bool) -> String {
    let axis = if category {
        ":status_category:"
    } else {
        ":status:"
    };
    format!(
        "compound:{}{}{}",
        URL_SAFE_NO_PAD.encode(primary_key.as_bytes()),
        axis,
        status
    )
}

fn table_group_descriptor(kind: &str, raw: &str, count: i64, property_id: Option<&str>) -> Value {
    match kind {
        "status" | "status_category" => json!({
            "key": format!("status:{raw}"),
            "value": { "kind": "status", "status": raw },
            "count": count,
        }),
        "assignee" => {
            if matches!(raw, "unassigned" | "__unassigned__" | "__none__") {
                json!({
                    "key": "assignee:unassigned",
                    "value": { "kind": "assignee", "actor": Value::Null },
                    "count": count,
                })
            } else if let Some((actor_type, id)) = raw.split_once(':') {
                json!({
                    "key": format!("assignee:{raw}"),
                    "value": { "kind": "assignee", "actor": { "type": actor_type, "id": id } },
                    "count": count,
                })
            } else {
                json!({
                    "key": format!("assignee:{raw}"),
                    "value": { "kind": "assignee", "actor": Value::Null },
                    "count": count,
                })
            }
        }
        "project" => {
            if matches!(raw, "unassigned" | "__none__" | "none" | "__no_project__") {
                json!({
                    "key": "project:none",
                    "value": { "kind": "project", "project_id": Value::Null },
                    "count": count,
                })
            } else {
                json!({
                    "key": format!("project:{raw}"),
                    "value": { "kind": "project", "project_id": raw },
                    "count": count,
                })
            }
        }
        "parent" => {
            if matches!(raw, "root" | "none" | "__no_parent__") {
                json!({
                    "key": "parent:none",
                    "value": {
                        "kind": "parent",
                        "parent_id": Value::Null,
                        "parent": Value::Null,
                        "value_state": "unset",
                    },
                    "count": count,
                })
            } else {
                json!({
                    "key": format!("parent:{raw}"),
                    "value": {
                        "kind": "parent",
                        "parent_id": raw,
                        "parent": Value::Null,
                        "value_state": "unavailable",
                    },
                    "count": count,
                })
            }
        }
        "property" => {
            let property_id = property_id.unwrap_or_default();
            let (state, value) = raw.split_once(':').unwrap_or(("unset", ""));
            if state == "unset" || value.is_empty() && state != "value" {
                json!({
                    "key": format!("property:{property_id}:unset:"),
                    "value": {
                        "kind": "property",
                        "property_id": property_id,
                        "value_state": "unset",
                    },
                    "count": count,
                })
            } else {
                json!({
                    "key": format!("property:{property_id}:value:{value}"),
                    "value": {
                        "kind": "property",
                        "property_id": property_id,
                        "value": value,
                        "value_state": "value",
                    },
                    "count": count,
                })
            }
        }
        _ => json!({
            "key": format!("{kind}:{raw}"),
            "value": { "kind": kind, "value": raw },
            "count": count,
        }),
    }
}

async fn table_groups(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<TableRequest>,
) -> Response {
    let workspace_id = match context_workspace(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let kind = request
        .group
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("status");
    if !matches!(
        kind,
        "status" | "status_category" | "assignee" | "project" | "parent" | "property" | "compound"
    ) {
        return error_response(StatusCode::BAD_REQUEST, "unsupported table group");
    }
    let property_id = request.group.get("property_id").and_then(Value::as_str);
    if kind == "property"
        && property_id
            .and_then(|id| Uuid::parse_str(id).ok())
            .is_none()
    {
        return error_response(StatusCode::BAD_REQUEST, "invalid group.property_id");
    }
    let primary = request
        .group
        .get("primary")
        .and_then(Value::as_str)
        .unwrap_or("assignee");
    let secondary = request
        .group
        .get("secondary")
        .and_then(Value::as_str)
        .unwrap_or("status");
    let total =
        match table_filtered_count(&state, workspace_id, context.member.user_id, &request).await {
            Ok(total) => total,
            Err(response) => return response,
        };
    let mut counts =
        std::collections::BTreeMap::<String, (i64, std::collections::BTreeMap<String, i64>)>::new();
    if kind == "compound" {
        let primary_expr = match table_dimension_expr(primary, None) {
            Ok(expr) => expr,
            Err(response) => return response,
        };
        let secondary_expr = match table_dimension_expr(secondary, None) {
            Ok(expr) => expr,
            Err(response) => return response,
        };
        let rows = match table_compound_counts(
            &state,
            workspace_id,
            context.member.user_id,
            &request,
            &primary_expr,
            &secondary_expr,
        )
        .await
        {
            Ok(rows) => rows,
            Err(response) => return response,
        };
        for (key, secondary_key, count) in rows {
            let entry = counts.entry(key).or_default();
            entry.0 += count;
            *entry.1.entry(secondary_key).or_default() += count;
        }
    } else {
        let expr = match table_dimension_expr(kind, property_id) {
            Ok(expr) => expr,
            Err(response) => return response,
        };
        let rows = match if kind == "property" {
            table_property_grouped_counts(
                &state,
                workspace_id,
                context.member.user_id,
                &request,
                property_id.unwrap_or_default(),
                true,
            )
            .await
        } else {
            table_grouped_counts(
                &state,
                workspace_id,
                context.member.user_id,
                &request,
                &expr,
            )
            .await
        } {
            Ok(rows) => rows,
            Err(response) => return response,
        };
        for (key, count) in rows {
            counts.insert(key, (count, std::collections::BTreeMap::new()));
        }
    }
    let fingerprint = table_fingerprint(&request);
    let (limit, offset) = match table_cursor(&request, &fingerprint) {
        Ok(page) => page,
        Err(response) => return response,
    };
    let category = secondary == "status_category";
    let all_groups = counts
        .into_iter()
        .map(|(key, (count, secondary_counts))| {
            let descriptor_kind = if kind == "compound" { primary } else { kind };
            let mut descriptor = table_group_descriptor(descriptor_kind, &key, count, property_id);
            if kind == "compound" {
                let primary_key = descriptor
                    .get("key")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let secondary_groups = patchbay_service::issue_status::CANONICAL_ORDER
                    .iter()
                    .map(|status| {
                        json!({
                            "key": compound_cell_group_key(&primary_key, status, category),
                            "value": { "kind": "status", "status": *status },
                            "count": secondary_counts.get(*status).copied().unwrap_or(0),
                        })
                    })
                    .collect::<Vec<_>>();
                if let Some(object) = descriptor.as_object_mut() {
                    object.insert("secondary_groups".into(), json!(secondary_groups));
                }
            }
            descriptor
        })
        .collect::<Vec<_>>();
    let next_offset = offset + limit;
    let next_cursor = (next_offset < all_groups.len() as i64)
        .then(|| encode_table_cursor(&request, &fingerprint, next_offset));
    let groups = all_groups
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect::<Vec<_>>();
    Json(json!({ "query_fingerprint": fingerprint, "total": total, "groups": groups, "next_cursor": next_cursor })).into_response()
}

async fn table_facets(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<TableRequest>,
) -> Response {
    if request.facets.is_empty() || request.facets.len() > 32 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "facets must contain between 1 and 32 entries",
        );
    }
    let workspace_id = match context_workspace(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let total =
        match table_filtered_count(&state, workspace_id, context.member.user_id, &request).await {
            Ok(total) => total,
            Err(response) => return response,
        };
    let mut facets = Vec::with_capacity(request.facets.len());
    for facet in &request.facets {
        let kind = facet
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(
            kind,
            "status"
                | "priority"
                | "assignee"
                | "creator"
                | "project"
                | "label"
                | "property"
                | "working_agents"
        ) {
            return error_response(StatusCode::BAD_REQUEST, "unsupported table facet");
        }
        let mut facet_request = request.clone();
        if let Some(filters) = facet_request
            .query
            .get_mut("filters")
            .and_then(Value::as_object_mut)
        {
            match kind {
                "status" => {
                    filters.remove("statuses");
                }
                "priority" => {
                    filters.remove("priorities");
                }
                "assignee" => {
                    filters.remove("assignees");
                    filters.remove("include_no_assignee");
                }
                "creator" => {
                    filters.remove("creators");
                }
                "project" => {
                    filters.remove("project_ids");
                    filters.remove("include_no_project");
                }
                "label" => {
                    filters.remove("label_ids");
                }
                "working_agents" => {
                    filters.remove("working_only");
                    filters.remove("working_issue_ids");
                }
                "property" => {
                    if let Some(id) = facet.get("property_id").and_then(Value::as_str) {
                        if let Some(properties) =
                            filters.get_mut("properties").and_then(Value::as_object_mut)
                        {
                            properties.remove(id);
                        }
                    }
                }
                _ => {}
            }
        }
        let counts = match kind {
            "label" => {
                let mut query = QueryBuilder::<Postgres>::new(
                    "SELECT itl.label_id::text, count(DISTINCT i.id)::bigint FROM issue i JOIN issue_to_label itl ON itl.issue_id=i.id WHERE ",
                );
                if let Err(response) = push_table_filters(
                    &mut query,
                    &facet_request,
                    workspace_id,
                    context.member.user_id,
                ) {
                    return response;
                }
                query.push(" GROUP BY 1");
                match query
                    .build_query_as::<(String, i64)>()
                    .fetch_all(&state.pool)
                    .await
                {
                    Ok(rows) => rows
                        .into_iter()
                        .collect::<std::collections::BTreeMap<_, _>>(),
                    Err(error) => {
                        tracing::warn!(%error, "failed to query issue label facets");
                        return error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "failed to query issue facets",
                        );
                    }
                }
            }
            "working_agents" => {
                let mut query = QueryBuilder::<Postgres>::new(
                    "SELECT atq.agent_id::text, count(DISTINCT i.id)::bigint FROM issue i JOIN agent_task_queue atq ON atq.issue_id=i.id AND atq.workspace_id=i.workspace_id AND atq.status='running' WHERE ",
                );
                if let Err(response) = push_table_filters(
                    &mut query,
                    &facet_request,
                    workspace_id,
                    context.member.user_id,
                ) {
                    return response;
                }
                query.push(" GROUP BY 1");
                match query
                    .build_query_as::<(String, i64)>()
                    .fetch_all(&state.pool)
                    .await
                {
                    Ok(rows) => rows
                        .into_iter()
                        .collect::<std::collections::BTreeMap<_, _>>(),
                    Err(error) => {
                        tracing::warn!(%error, "failed to query working-agent facets");
                        return error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "failed to query issue facets",
                        );
                    }
                }
            }
            _ => {
                let property_id = facet.get("property_id").and_then(Value::as_str);
                let expr = match table_dimension_expr(kind, property_id) {
                    Ok(expr) => expr,
                    Err(response) => return response,
                };
                match if kind == "property" {
                    table_property_grouped_counts(
                        &state,
                        workspace_id,
                        context.member.user_id,
                        &facet_request,
                        property_id.unwrap_or_default(),
                        false,
                    )
                    .await
                } else {
                    table_grouped_counts(
                        &state,
                        workspace_id,
                        context.member.user_id,
                        &facet_request,
                        &expr,
                    )
                    .await
                } {
                    Ok(rows) => rows.into_iter().collect(),
                    Err(response) => return response,
                }
            }
        };
        facets.push(json!({
            "kind": kind,
            "property_id": facet.get("property_id"),
            "values": counts.into_iter().map(|(key, count)| json!({"key": key, "count": count})).collect::<Vec<_>>(),
        }));
    }
    Json(json!({
        "query_fingerprint": table_fingerprint(&request),
        "total": total,
        "facets": facets,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct BatchDeleteRequest {
    issue_ids: Vec<String>,
}

async fn delete_issue_and_collect_attachment_urls(
    state: &HandlerState,
    issue: &Issue,
) -> anyhow::Result<Vec<String>> {
    let mut tx = state.pool.begin().await?;
    issue_q::lock_issue_for_delete(&mut *tx, issue.id, issue.workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("issue not found while locking for delete"))?;
    let urls = attachment::list_attachment_ur_ls_by_issue_or_comments(&mut *tx, issue.id).await?;
    if issue_q::delete_issue(&mut *tx, issue.id, issue.workspace_id).await? != 1 {
        anyhow::bail!("issue disappeared while deleting");
    }
    tx.commit().await?;
    Ok(urls)
}

pub(crate) async fn delete_attachment_objects(state: &HandlerState, urls: Vec<String>) {
    let Some(storage) = state.attachment_storage.as_ref() else {
        return;
    };
    for url in urls {
        let Some(key) = storage.key_from_url(&url) else {
            tracing::warn!(%url, "skipping issue attachment URL outside configured storage");
            continue;
        };
        if let Err(error) = storage.delete(&key).await {
            tracing::warn!(%error, %key, "failed to delete issue attachment object");
        }
    }
}

async fn batch_delete_issues(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Json(request): Json<BatchDeleteRequest>,
) -> Response {
    if request.issue_ids.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "issue_ids is required");
    }
    let workspace_id = match context_workspace(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let (actor_type, actor_id, _) = mutation_actor(&state, &context, &headers).await;
    let mut deleted = 0usize;
    for raw_id in request.issue_ids {
        let Ok(id) = Uuid::parse_str(&raw_id) else {
            continue;
        };
        let Ok(Some(issue)) = issue_q::get_issue_in_workspace(&state.pool, id, workspace_id).await
        else {
            continue;
        };
        if state.tasks.cancel_tasks_for_issue(issue.id).await.is_err() {
            continue;
        }
        let _ = autopilot::fail_autopilot_runs_by_issue(&state.pool, issue.id).await;
        let Ok(attachment_urls) = delete_issue_and_collect_attachment_urls(&state, &issue).await
        else {
            continue;
        };
        delete_attachment_objects(&state, attachment_urls).await;
        state.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_ISSUE_DELETED.into(),
            workspace_id: workspace_id.to_string(),
            actor_type: actor_type.clone(),
            actor_id: actor_id.to_string(),
            payload: json!({"issue_id": issue.id}),
            ..Default::default()
        });
        deleted += 1;
    }
    Json(json!({ "deleted": deleted })).into_response()
}

async fn delete_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    if let Err(error) = state.tasks.cancel_tasks_for_issue(issue.id).await {
        tracing::warn!(%error, issue_id=%issue.id, "failed to cancel issue tasks");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to delete issue");
    }
    let _ = autopilot::fail_autopilot_runs_by_issue(&state.pool, issue.id).await;
    match delete_issue_and_collect_attachment_urls(&state, &issue).await {
        Ok(attachment_urls) => {
            delete_attachment_objects(&state, attachment_urls).await;
            let (actor_type, actor_id, _) = mutation_actor(&state, &context, &headers).await;
            state.bus.publish(&patchbay_events::Event {
                event_type: patchbay_protocol::EVENT_ISSUE_DELETED.into(),
                workspace_id: issue.workspace_id.to_string(),
                actor_type,
                actor_id: actor_id.to_string(),
                payload: json!({"issue_id":issue.id}),
                ..Default::default()
            });
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => {
            tracing::warn!(%error, issue_id=%issue.id, "failed to delete issue");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to delete issue")
        }
    }
}

#[derive(Debug, Deserialize)]
struct QuickCreateRequest {
    #[serde(default)]
    agent_id: String,
    #[serde(default)]
    team_id: String,
    prompt: String,
    #[serde(default)]
    priority: String,
    #[serde(default)]
    due_date: String,
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    parent_issue_id: String,
    #[serde(default)]
    attachment_ids: Vec<String>,
}

async fn quick_create_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<QuickCreateRequest>,
) -> Response {
    let prompt = request.prompt.trim();
    if prompt.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "prompt is required");
    }
    let has_agent = !request.agent_id.trim().is_empty();
    let has_team = !request.team_id.trim().is_empty();
    if has_agent == has_team {
        return error_response(
            StatusCode::BAD_REQUEST,
            "exactly one of agent_id or team_id is required",
        );
    }
    let priority = request.priority.trim().to_lowercase();
    if !priority.is_empty() && !matches!(priority.as_str(), "urgent" | "high" | "medium" | "low") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "priority must be one of: urgent, high, medium, low",
        );
    }
    if !request.due_date.is_empty()
        && NaiveDate::parse_from_str(&request.due_date, "%Y-%m-%d").is_err()
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "invalid due_date format, expected YYYY-MM-DD",
        );
    }
    let workspace_id = match context_workspace(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let requested_assignee = if has_team {
        ("team", request.team_id.trim())
    } else {
        ("agent", request.agent_id.trim())
    };
    let requested_id = match Uuid::parse_str(requested_assignee.1) {
        Ok(id) => id,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                if has_team {
                    "invalid team_id"
                } else {
                    "invalid agent_id"
                },
            );
        }
    };
    if let Err(message) =
        validate_assignee(&state, &context, requested_assignee.0, requested_id).await
    {
        return error_response(StatusCode::FORBIDDEN, &message);
    }
    let mut team_id = None;
    let agent_id = if has_team {
        let id = match Uuid::parse_str(request.team_id.trim()) {
            Ok(id) => id,
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid team_id"),
        };
        let selected = match team::get_team_in_workspace(&state.pool, id, workspace_id).await {
            Ok(Some(value)) if value.archived_at.is_none() => value,
            Ok(_) => return error_response(StatusCode::NOT_FOUND, "team not found"),
            Err(_) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load team");
            }
        };
        team_id = Some(id);
        selected.leader_id
    } else {
        match Uuid::parse_str(request.agent_id.trim()) {
            Ok(id) => id,
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid agent_id"),
        }
    };
    let selected_agent = match agent::get_agent_in_workspace(&state.pool, agent_id, workspace_id)
        .await
    {
        Ok(Some(value)) if value.archived_at.is_none() => value,
        Ok(_) => return error_response(StatusCode::NOT_FOUND, "agent not found"),
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load agent"),
    };
    if selected_agent.runtime_id.is_none() {
        return error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "agent runtime is required",
        );
    }
    let project_id = match optional_uuid(Some(request.project_id.trim()), "project_id") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    if let Some(id) = project_id {
        if !matches!(
            patchbay_db::queries::project::get_project_in_workspace(&state.pool, id, workspace_id)
                .await,
            Ok(Some(_))
        ) {
            return error_response(StatusCode::BAD_REQUEST, "project not found");
        }
    }
    let parent_issue_id =
        match optional_uuid(Some(request.parent_issue_id.trim()), "parent_issue_id") {
            Ok(value) => value,
            Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
        };
    if let Some(id) = parent_issue_id {
        if !matches!(
            issue_q::get_issue_in_workspace(&state.pool, id, workspace_id).await,
            Ok(Some(_))
        ) {
            return error_response(
                StatusCode::BAD_REQUEST,
                "parent issue not found in this workspace",
            );
        }
    }
    let attachment_ids = match uuid_strings(&request.attachment_ids, "attachment_ids") {
        Ok(ids) => ids,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    match state
        .tasks
        .enqueue_quick_create_task(
            workspace_id,
            context.member.user_id,
            agent_id,
            team_id,
            prompt,
            &priority,
            request.due_date.trim(),
            project_id,
            parent_issue_id,
            attachment_ids,
        )
        .await
    {
        Ok(task) => (StatusCode::ACCEPTED, Json(json!({"task_id":task.id}))).into_response(),
        Err(error) => {
            tracing::warn!(%error, "quick-create enqueue failed");
            error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                "failed to enqueue quick-create task",
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct TriggerPreviewRequest {
    #[serde(default)]
    issue_ids: Vec<String>,
    #[serde(default)]
    is_create: bool,
    assignee_type: Option<String>,
    assignee_id: Option<String>,
    status: Option<String>,
}

async fn preview_trigger(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<TriggerPreviewRequest>,
) -> Response {
    if request.issue_ids.len() > 500 {
        return error_response(StatusCode::BAD_REQUEST, "too many issue_ids");
    }
    let workspace_id = match context_workspace(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let prospective_id = match request
        .assignee_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(Uuid::parse_str)
        .transpose()
    {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid assignee_id"),
    };
    let mut candidates = Vec::new();
    if request.is_create {
        let now = chrono::Utc::now();
        candidates.push((
            Issue {
                id: Uuid::nil(),
                workspace_id,
                title: String::new(),
                description: None,
                status: request
                    .status
                    .clone()
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| "todo".into()),
                priority: "none".into(),
                assignee_type: request.assignee_type.clone(),
                assignee_id: prospective_id,
                creator_type: "member".into(),
                creator_id: context.member.user_id,
                parent_issue_id: None,
                acceptance_criteria: json!([]),
                context_refs: json!([]),
                position: 0.0,
                due_date: None,
                created_at: now,
                updated_at: now,
                number: 0,
                project_id: None,
                origin_type: None,
                origin_id: None,
                first_executed_at: None,
                start_date: None,
                metadata: json!({}),
                stage: None,
                properties: json!({}),
                revision: 1,
                last_activity_at: None,
                reviewer_id: None,
                reviewer_type: None,
            },
            String::new(),
            true,
        ));
    } else {
        for raw in request.issue_ids {
            let Ok(id) = Uuid::parse_str(&raw) else {
                continue;
            };
            let Ok(Some(mut issue)) =
                issue_q::get_issue_in_workspace(&state.pool, id, workspace_id).await
            else {
                continue;
            };
            let previous = issue.status.clone();
            if prospective_id.is_some() {
                issue.assignee_id = prospective_id;
                issue.assignee_type = request.assignee_type.clone();
            }
            if let Some(status) = request.status.as_ref().filter(|value| !value.is_empty()) {
                issue.status = status.clone();
            }
            candidates.push((issue, previous, false));
        }
    }
    let mut allowed_agents = HashSet::new();
    for (issue, _, _) in &candidates {
        let target_id = match (issue.assignee_type.as_deref(), issue.assignee_id) {
            (Some("agent"), Some(agent_id)) => Some(agent_id),
            (Some("team"), Some(team_id)) => {
                team::get_team_in_workspace(&state.pool, team_id, workspace_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|team| team.leader_id)
            }
            _ => None,
        };
        let Some(agent_id) = target_id else {
            continue;
        };
        if let Ok(Some(agent)) =
            agent::get_agent_in_workspace(&state.pool, agent_id, workspace_id).await
        {
            if can_member_invoke_agent(&state, context.member.user_id, workspace_id, &agent).await {
                allowed_agents.insert(agent.id);
            }
        }
    }
    let mut triggers = Vec::new();
    for (issue, previous, is_create) in candidates {
        let input = IssueTriggerInput {
            assignee_changed: prospective_id.is_some(),
            status_changed: issue.status != previous,
            prev_status: previous,
            is_create,
            issue,
        };
        let allowed = allowed_agents.clone();
        if let Some(trigger) = state
            .issues
            .will_enqueue_run(
                input,
                IssueTriggerProbe {
                    can_access_agent: Some(Box::new(move |agent| allowed.contains(&agent.id))),
                    is_self_loop: None,
                    suppress_active_self_assignment: None,
                },
            )
            .await
        {
            let handoff_supported = runtime_supports_handoff(&state, trigger.agent_id).await;
            triggers.push(json!({ "issue_id": trigger.issue_id, "agent_id": trigger.agent_id, "source": trigger.source.as_str(), "handoff_supported": handoff_supported }));
        }
    }
    Json(json!({ "total_count": triggers.len(), "triggers": triggers })).into_response()
}

async fn runtime_supports_handoff(state: &HandlerState, agent_id: Uuid) -> bool {
    let Ok(Some(agent)) = agent::get_agent(&state.pool, agent_id).await else {
        return false;
    };
    let Some(runtime_id) = agent.runtime_id else {
        return false;
    };
    let Ok(Some(runtime)) = runtime::get_agent_runtime(&state.pool, runtime_id).await else {
        return false;
    };
    runtime
        .metadata
        .get("cli_version")
        .and_then(Value::as_str)
        .is_some_and(patchbay_agent::version::handoff_supported)
}

async fn issue_timeline(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let mut comments =
        match comment_q::list_comments_for_issue(&state.pool, issue.id, issue.workspace_id, 2001)
            .await
        {
            Ok(rows) => rows,
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to list comments",
                );
            }
        };
    let mut activities =
        match activity::list_activities_for_issue(&state.pool, issue.id, 2001).await {
            Ok(rows) => rows,
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to list activities",
                );
            }
        };
    let truncated_comments = comments.len() > 2000;
    let truncated_activities = activities.len() > 2000;
    if truncated_comments {
        comments.remove(0);
        match crate::comment_list::complete_comment_threads(
            &state,
            issue.id,
            issue.workspace_id,
            comments,
        )
        .await
        {
            Ok(completed) => comments = completed,
            Err(error) => {
                tracing::warn!(%error, "failed to complete truncated timeline threads");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to complete comment threads",
                );
            }
        }
    }
    if truncated_activities {
        activities = activities.split_off(activities.len().saturating_sub(2000));
    }
    let comment_ids = comments
        .iter()
        .map(|comment| comment.id)
        .collect::<Vec<_>>();
    let mut reactions_by_comment = HashMap::<Uuid, Vec<Value>>::new();
    if !comment_ids.is_empty() {
        if let Ok(rows) = patchbay_db::queries::reaction::list_reactions_by_comment_i_ds(
            &state.pool,
            comment_ids.clone(),
        )
        .await
        {
            for reaction in rows {
                reactions_by_comment
                    .entry(reaction.comment_id)
                    .or_default()
                    .push(crate::comment::reaction_json(&reaction));
            }
        }
    }
    let mut attachments_by_comment = HashMap::<Uuid, Vec<crate::issue::AttachmentResponse>>::new();
    if !comment_ids.is_empty() {
        if let Ok(rows) = attachment::list_attachments_by_comment_i_ds(
            &state.pool,
            comment_ids,
            issue.workspace_id,
        )
        .await
        {
            for item in rows {
                if let Some(comment_id) = item.comment_id {
                    attachments_by_comment
                        .entry(comment_id)
                        .or_default()
                        .push(AttachmentResponse::for_request(&state, &item, &headers));
                }
            }
        }
    }
    let mut entries = Vec::new();
    for comment in comments {
        let reactions = reactions_by_comment.remove(&comment.id).unwrap_or_default();
        let attachments = attachments_by_comment
            .remove(&comment.id)
            .unwrap_or_default();
        entries.push(json!({
            "type":"comment",
            "id":comment.id,
            "actor_type":comment.author_type,
            "actor_id":comment.author_id,
            "created_at":crate::timefmt::rfc3339(comment.created_at),
            "content":comment.content,
            "parent_id":comment.parent_id,
            "updated_at":crate::timefmt::rfc3339(comment.updated_at),
            "revision":comment.revision,
            "comment_type":comment.type_,
            "quick_action_id":comment.quick_action_id,
            "resolved_at":comment.resolved_at.map(crate::timefmt::rfc3339),
            "resolved_by_type":comment.resolved_by_type,
            "resolved_by_id":comment.resolved_by_id,
            "source_task_id":comment.source_task_id,
            "reactions": reactions,
            "attachments": attachments,
        }));
    }
    for row in activities {
        entries.push(json!({ "type":"activity", "id":row.id, "actor_type":row.actor_type, "actor_id":row.actor_id, "created_at":crate::timefmt::rfc3339(row.created_at), "action":row.action, "details":row.details }));
    }
    entries.sort_by_key(|entry| {
        (
            entry["created_at"].as_str().unwrap_or_default().to_string(),
            entry["id"].as_str().unwrap_or_default().to_string(),
        )
    });
    let wrapped = ["limit", "before", "after", "around"]
        .iter()
        .any(|key| params.contains_key(*key));
    if wrapped {
        entries.reverse();
    }
    let body = if wrapped {
        let target_index = params.get("around").and_then(|anchor| {
            entries
                .iter()
                .position(|entry| entry["id"].as_str() == Some(anchor.as_str()))
        });
        json!({ "entries":entries, "next_cursor":Value::Null, "prev_cursor":Value::Null, "has_more_before":truncated_comments || truncated_activities, "has_more_after":false, "target_index":target_index })
    } else {
        json!(entries)
    };
    let mut response = Json(body).into_response();
    let truncated = match (truncated_comments, truncated_activities) {
        (true, true) => Some("activity,comment"),
        (true, false) => Some("comment"),
        (false, true) => Some("activity"),
        _ => None,
    };
    if let Some(value) = truncated {
        response.headers_mut().insert(
            "x-timeline-truncated",
            value.parse().expect("static header"),
        );
    }
    response
}

#[derive(Debug, Default, Deserialize)]
struct RerunIssueRequest {
    task_id: Option<String>,
}

async fn rerun_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let request = if body.is_empty() {
        RerunIssueRequest::default()
    } else {
        match serde_json::from_slice::<RerunIssueRequest>(&body) {
            Ok(request) => request,
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
        }
    };
    let source_task_id = match request.task_id {
        Some(raw) if !raw.is_empty() => match Uuid::parse_str(&raw) {
            Ok(id) => Some(id),
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid task_id"),
        },
        _ => None,
    };

    // Resolve and authorize the actual rerun target before the service clears
    // or enqueues anything. A historical task may belong to an agent that is
    // no longer the issue assignee, so checking only the current assignee would
    // cross the private-agent invocation boundary.
    let target_agent_id = if let Some(task_id) = source_task_id {
        match agent::get_agent_task(&state.pool, task_id).await {
            Ok(Some(task)) if task.issue_id == Some(issue.id) => task.agent_id,
            Ok(Some(_)) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "source task does not belong to this issue",
                );
            }
            Ok(None) => return error_response(StatusCode::BAD_REQUEST, "source task not found"),
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load source task",
                );
            }
        }
    } else {
        match (issue.assignee_type.as_deref(), issue.assignee_id) {
            (Some("agent"), Some(agent_id)) => agent_id,
            (Some("team"), Some(team_id)) => {
                match team::get_team_in_workspace(&state.pool, team_id, issue.workspace_id).await {
                    Ok(Some(team)) => team.leader_id,
                    _ => {
                        return error_response(
                            StatusCode::BAD_REQUEST,
                            "issue is assigned to a team but team not found",
                        );
                    }
                }
            }
            _ => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "issue is not assigned to an agent or team",
                );
            }
        }
    };
    let target = match agent::get_agent(&state.pool, target_agent_id).await {
        Ok(Some(target)) if target.workspace_id == issue.workspace_id => target,
        Ok(_) => return error_response(StatusCode::BAD_REQUEST, "target agent not found"),
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load target agent",
            );
        }
    };
    if !can_member_invoke_agent(&state, context.member.user_id, issue.workspace_id, &target).await {
        return error_response(StatusCode::FORBIDDEN, "agent invocation is not allowed");
    }

    let authorized_agent_id = target.id;
    match state
        .tasks
        .rerun_issue(
            issue.id,
            source_task_id,
            None,
            Some(context.member.user_id),
            Some(&move |agent: &patchbay_db::models::Agent| agent.id == authorized_agent_id),
        )
        .await
    {
        Ok(task) => (
            StatusCode::ACCEPTED,
            Json(crate::task_json::task_to_map(
                &task,
                &issue.workspace_id.to_string(),
            )),
        )
            .into_response(),
        Err(patchbay_service::task_service::TaskServiceError::RerunInvokeNotAllowed(_)) => {
            error_response(StatusCode::FORBIDDEN, "agent invocation is not allowed")
        }
        Err(error) => error_response(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

async fn quick_action_for_issue(
    state: &HandlerState,
    context: &WorkspaceContext,
    issue_id: &str,
    action_id: &str,
) -> Result<
    (
        Issue,
        patchbay_db::models::QuickAction,
        String,
        patchbay_db::models::Agent,
    ),
    Response,
> {
    let issue = resolve_issue(state, context, issue_id).await?;
    let id = Uuid::parse_str(action_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid quick action id"))?;
    let action = quick_action::get_quick_action(&state.pool, id, issue.workspace_id)
        .await
        .map_err(|_| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load quick action",
            )
        })?
        .filter(|action| {
            action.visibility == "public" || action.created_by_id == context.member.user_id
        })
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "quick action not found"))?;
    let (name, agent, _) = crate::quick_action::target(
        state,
        issue.workspace_id,
        &action.assignee_type,
        action.assignee_id,
    )
    .await?;
    if !can_member_invoke_agent(state, context.member.user_id, issue.workspace_id, &agent).await {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "agent invocation is not allowed",
        ));
    }
    Ok((issue, action, name, agent))
}

fn quick_action_body(action: &patchbay_db::models::QuickAction, name: &str) -> String {
    format!(
        "[@{name}](mention://{}/{})\n\n{}",
        action.assignee_type, action.assignee_id, action.prompt
    )
}

async fn render_quick_action(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((issue_id, action_id)): Path<(String, String)>,
) -> Response {
    match quick_action_for_issue(&state, &context, &issue_id, &action_id).await {
        Ok((_issue, action, name, _)) => {
            Json(json!({"content":quick_action_body(&action,&name)})).into_response()
        }
        Err(response) => response,
    }
}

async fn run_quick_action(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((issue_id, action_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (issue, action, name, target_agent) =
        match quick_action_for_issue(&state, &context, &issue_id, &action_id).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    if action.status != "active" {
        return error_response(StatusCode::BAD_REQUEST, "quick action is archived");
    }
    let (actor_type, actor_id, source_task_id) = mutation_actor(&state, &context, &headers).await;
    let row = match comment_q::create_comment(
        &state.pool,
        issue.id,
        issue.workspace_id,
        &actor_type,
        actor_id,
        &quick_action_body(&action, &name).replace('\0', ""),
        "comment",
        None,
        source_task_id,
        Some(action.id),
        None,
        patchbay_db::dbid::new_v7(),
    )
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "issue not found"),
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to run quick action",
            );
        }
    };
    let comment = comment_q::get_comment_in_workspace(
        &state.pool,
        row.id.unwrap_or_default(),
        issue.workspace_id,
    )
    .await
    .ok()
    .flatten()
    .expect("created comment");
    let trigger = if action.assignee_type == "team" {
        state
            .tasks
            .enqueue_task_for_team_leader(
                &issue,
                target_agent.id,
                action.assignee_id,
                Some(comment.id),
            )
            .await
    } else {
        state
            .tasks
            .enqueue_task_for_mention(&issue, target_agent.id, Some(comment.id))
            .await
    };
    let trigger_outcomes = match trigger {
        Ok(task) => vec![json!({"agent_id":target_agent.id,"status":"queued","task_id":task.id})],
        Err(error) => {
            vec![json!({"agent_id":target_agent.id,"status":"blocked","reason":error.to_string()})]
        }
    };
    let _ =
        quick_action::touch_quick_action_usage(&state.pool, action.id, issue.workspace_id).await;
    let mut value = crate::comment::comment_json(&state, &comment, &headers).await;
    if let Some(object) = value.as_object_mut() {
        object.insert("issue_revision".into(), json!(row.issue_revision));
        object.insert("trigger_outcomes".into(), json!(trigger_outcomes));
    }
    crate::comment::publish(
        &state,
        &context,
        patchbay_protocol::EVENT_COMMENT_CREATED,
        &actor_type,
        actor_id,
        value.clone(),
    );
    (StatusCode::CREATED, Json(value)).into_response()
}

#[derive(Debug, Deserialize)]
struct TeamEvaluationRequest {
    outcome: String,
    #[serde(default)]
    reason: String,
}
async fn record_team_evaluated(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<TeamEvaluationRequest>,
) -> Response {
    if !matches!(request.outcome.as_str(), "action" | "no_action" | "failed") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "outcome must be 'action', 'no_action', or 'failed'",
        );
    }
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let Some(team_id) = issue
        .assignee_id
        .filter(|_| issue.assignee_type.as_deref() == Some("team"))
    else {
        return error_response(StatusCode::BAD_REQUEST, "issue is not assigned to a team");
    };
    let selected = match team::get_team_in_workspace(&state.pool, team_id, issue.workspace_id).await
    {
        Ok(Some(value)) => value,
        _ => return error_response(StatusCode::NOT_FOUND, "team not found"),
    };
    let (actor_type, actor_id, task_id) = mutation_actor(&state, &context, &headers).await;
    if actor_type != "agent" || actor_id != selected.leader_id {
        return error_response(
            StatusCode::FORBIDDEN,
            "only the team leader agent can record evaluations",
        );
    }
    let Some(task_id) = task_id else {
        return error_response(StatusCode::BAD_REQUEST, "invalid task id");
    };
    let task = match agent::get_agent_task(&state.pool, task_id).await {
        Ok(Some(task)) if task.issue_id == Some(issue.id) => task,
        _ => return error_response(StatusCode::BAD_REQUEST, "task does not belong to issue"),
    };
    let details = json!({"team_id":selected.id,"task_id":task.id,"outcome":request.outcome,"reason":request.reason});
    match activity::create_activity(
        &state.pool,
        issue.workspace_id,
        issue.id,
        Some("agent"),
        Some(actor_id),
        "team_leader_evaluated",
        &details,
        patchbay_db::dbid::new_v7(),
    )
    .await
    {
        Ok(Some(row)) => {
            state.bus.publish(&patchbay_events::Event {
                event_type: patchbay_protocol::EVENT_ACTIVITY_CREATED.into(),
                workspace_id: issue.workspace_id.to_string(),
                actor_type: "agent".into(),
                actor_id: actor_id.to_string(),
                payload: json!({
                    "issue_id": issue.id,
                    "entry": {
                        "type": "activity", "id": row.id, "actor_type": "agent",
                        "actor_id": actor_id, "action": row.action, "details": details,
                        "created_at": crate::timefmt::rfc3339(row.created_at),
                    }
                }),
                ..Default::default()
            });
            (
                StatusCode::CREATED,
                Json(json!({"id":row.id,"action":row.action})),
            )
                .into_response()
        }
        _ => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to record evaluation",
        ),
    }
}

async fn move_anchor_position(
    state: &HandlerState,
    workspace_id: Uuid,
    id: Option<Uuid>,
) -> Result<Option<f64>, Response> {
    let Some(id) = id else { return Ok(None) };
    sqlx::query_scalar::<_, f64>("SELECT position FROM issue WHERE workspace_id=$1 AND id=$2")
        .bind(workspace_id)
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to resolve move anchor");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to resolve move anchor",
            )
        })?
        .map(Some)
        .ok_or_else(|| {
            error_response(
                StatusCode::BAD_REQUEST,
                "move anchor not found in this workspace",
            )
        })
}

fn move_position(
    current: f64,
    before: Option<f64>,
    after: Option<f64>,
) -> Result<f64, &'static str> {
    let position = match (before, after) {
        (Some(before), Some(after)) if before < after => before + (after - before) / 2.0,
        (Some(_), Some(_)) => return Err("move anchors are stale or out of order"),
        (Some(before), None) => before + 1.0,
        (None, Some(after)) => after - 1.0,
        (None, None) => current,
    };
    if !position.is_finite()
        || before.is_some_and(|value| position <= value)
        || after.is_some_and(|value| position >= value)
    {
        return Err("move anchors are too close; refresh and retry");
    }
    Ok(position)
}

const MOVE_ALLOWED_FIELDS: &[&str] = &[
    "status",
    "assignee_type",
    "assignee_id",
    "parent_issue_id",
    "project_id",
    "before_id",
    "after_id",
    "expected_revision",
    "suppress_run",
    "handoff_note",
];

fn is_allowed_move_field(field: &str) -> bool {
    MOVE_ALLOWED_FIELDS.contains(&field)
}

async fn move_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let current = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let mut fields = match update_object(&body) {
        Ok(fields) => fields,
        Err(response) => return response,
    };
    if let Some(field) = fields.keys().find(|field| !is_allowed_move_field(field)) {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!("unsupported move field: {field}"),
        );
    }
    if !fields.contains_key("before_id") {
        return error_response(StatusCode::BAD_REQUEST, "before_id is required");
    }
    if !fields.contains_key("after_id") {
        return error_response(StatusCode::BAD_REQUEST, "after_id is required");
    }
    let decode = |name: &str| -> Result<Option<Uuid>, Response> {
        match fields.get(name) {
            Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => Uuid::parse_str(value)
                .map(Some)
                .map_err(|_| error_response(StatusCode::BAD_REQUEST, &format!("invalid {name}"))),
            _ => Err(error_response(
                StatusCode::BAD_REQUEST,
                &format!("{name} must be a UUID or null"),
            )),
        }
    };
    let before_id = match decode("before_id") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let after_id = match decode("after_id") {
        Ok(value) => value,
        Err(response) => return response,
    };
    if before_id == Some(current.id) || after_id == Some(current.id) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "move anchor cannot be the moved issue",
        );
    }
    if before_id.is_some() && before_id == after_id {
        return error_response(StatusCode::BAD_REQUEST, "move anchors must be distinct");
    }
    let before = match move_anchor_position(&state, current.workspace_id, before_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let after = match move_anchor_position(&state, current.workspace_id, after_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let position = match move_position(current.position, before, after) {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::CONFLICT, message),
    };
    fields.remove("before_id");
    fields.remove("after_id");
    fields.insert("position".into(), json!(position));
    match apply_issue_update(&state, &context, &headers, current, &fields, true).await {
        Ok(issue) => issue_response(&state, issue).await,
        Err(response) => response,
    }
}

async fn list_issue_subscribers(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    match subscriber::list_issue_subscribers(&state.pool, issue.id).await {
        Ok(subscribers) => Json(
            subscribers
                .iter()
                .map(SubscriberResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to list subscribers");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list subscribers",
            )
        }
    }
}

#[derive(Default, Deserialize)]
struct SubscriberRequest {
    user_id: Option<Uuid>,
    user_type: Option<String>,
}

async fn subscriber_target(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    request: SubscriberRequest,
) -> Result<(String, Uuid), Response> {
    let (caller_type, caller_id) = request_actor(headers, context);
    let user_type = request.user_type.unwrap_or_else(|| caller_type.into());
    let user_id = request.user_id.unwrap_or(caller_id);
    if !matches!(user_type.as_str(), "member" | "agent") {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "target user is not a member of this workspace",
        ));
    }
    let table = if user_type == "member" {
        "member"
    } else {
        "agent"
    };
    let key = if user_type == "member" {
        "user_id"
    } else {
        "id"
    };
    let statement = format!("SELECT 1 FROM {table} WHERE {key}=$1 AND workspace_id=$2");
    let exists = sqlx::query(&statement)
        .bind(user_id)
        .bind(context.member.workspace_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to verify subscriber target");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to verify subscriber",
            )
        })?;
    if exists.is_none() {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "target user is not a member of this workspace",
        ));
    }
    Ok((user_type, user_id))
}

async fn subscribe_to_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    request: Option<Json<SubscriberRequest>>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let target = subscriber_target(
        &state,
        &context,
        &headers,
        request.map(|Json(value)| value).unwrap_or_default(),
    )
    .await;
    let (user_type, user_id) = match target {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = subscriber::subscribe_to_issue_explicitly(
        &state.pool,
        issue.id,
        &user_type,
        user_id,
        "manual",
    )
    .await
    {
        tracing::warn!(%error, "failed to subscribe");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to subscribe");
    }
    let (actor_type, actor_id) = request_actor(&headers, &context);
    state.bus.publish(&patchbay_events::Event {
        event_type: "subscriber:added".into(), workspace_id: context.workspace_id.clone(),
        actor_type: actor_type.into(), actor_id: actor_id.to_string(),
        payload: json!({ "issue_id": issue.id, "user_type": user_type, "user_id": user_id, "reason": "manual" }),
        ..Default::default()
    });
    Json(json!({ "subscribed": true })).into_response()
}

async fn unsubscribe_from_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    request: Option<Json<SubscriberRequest>>,
) -> Response {
    unsubscribe(
        &state,
        &context,
        &id,
        &headers,
        request.map(|Json(v)| v).unwrap_or_default(),
        false,
    )
    .await
}

async fn unsubscribe_from_issue_subtree(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    request: Option<Json<SubscriberRequest>>,
) -> Response {
    unsubscribe(
        &state,
        &context,
        &id,
        &headers,
        request.map(|Json(v)| v).unwrap_or_default(),
        true,
    )
    .await
}

async fn unsubscribe(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_id: &str,
    headers: &HeaderMap,
    request: SubscriberRequest,
    subtree: bool,
) -> Response {
    let issue = match resolve_issue(state, context, raw_id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let (user_type, user_id) = match subscriber_target(state, context, headers, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let removed = if subtree {
        let mut transaction = match state.pool.begin().await {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "failed to unsubscribe");
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to unsubscribe");
            }
        };
        if let Err(error) =
            subscriber::lock_subscriber_writes(&mut *transaction, issue.workspace_id, user_id).await
        {
            tracing::warn!(%error, "failed to lock subscriber writes");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to unsubscribe");
        }
        if user_type == "member" {
            match subscriber::lock_active_member(&mut *transaction, user_id, issue.workspace_id)
                .await
            {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return error_response(
                        StatusCode::FORBIDDEN,
                        "target user is not a member of this workspace",
                    );
                }
                Err(error) => {
                    tracing::warn!(%error, "failed to recheck subscriber membership");
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to unsubscribe",
                    );
                }
            }
        }
        match subscriber::unsubscribe_from_issue_subtree(
            &mut *transaction,
            issue.id,
            &user_type,
            user_id,
        )
        .await
        {
            Ok(ids) => {
                if let Err(error) = transaction.commit().await {
                    tracing::warn!(%error, "failed to commit unsubscribe");
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to unsubscribe",
                    );
                }
                ids.into_iter().flatten().collect::<Vec<_>>()
            }
            Err(error) => {
                tracing::warn!(%error, "failed to unsubscribe subtree");
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to unsubscribe");
            }
        }
    } else {
        if let Err(error) =
            subscriber::remove_issue_subscriber(&state.pool, issue.id, &user_type, user_id).await
        {
            tracing::warn!(%error, "failed to unsubscribe");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to unsubscribe");
        }
        vec![issue.id]
    };
    let (actor_type, actor_id) = request_actor(headers, context);
    for issue_id in removed {
        state.bus.publish(&patchbay_events::Event {
            event_type: "subscriber:removed".into(),
            workspace_id: context.workspace_id.clone(),
            actor_type: actor_type.into(),
            actor_id: actor_id.to_string(),
            payload: json!({ "issue_id": issue_id, "user_type": user_type, "user_id": user_id }),
            ..Default::default()
        });
    }
    Json(json!({ "subscribed": false })).into_response()
}

fn request_actor(headers: &HeaderMap, context: &WorkspaceContext) -> (&'static str, Uuid) {
    if headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        == Some("task_token")
    {
        if let Some(id) = headers
            .get("x-agent-id")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| Uuid::parse_str(value).ok())
        {
            return ("agent", id);
        }
    }
    ("member", context.member.user_id)
}

#[derive(Deserialize)]
struct ReactionRequest {
    emoji: String,
}

async fn add_issue_reaction(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ReactionRequest>,
) -> Response {
    if request.emoji.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "emoji is required");
    }
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let (actor_type, actor_id) = request_actor(&headers, &context);
    match issue_reaction::add_issue_reaction(
        &state.pool,
        issue.id,
        issue.workspace_id,
        actor_type,
        actor_id,
        &request.emoji,
    )
    .await
    {
        Ok(Some(reaction)) => {
            let Some(response) = IssueReactionResponse::from_added(&reaction) else {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to add reaction");
            };
            if reaction.issue_revision > 0 {
                state.bus.publish(&patchbay_events::Event {
                    event_type: patchbay_protocol::EVENT_ISSUE_REACTION_ADDED.into(),
                    workspace_id: context.workspace_id.clone(),
                    actor_type: actor_type.into(),
                    actor_id: actor_id.to_string(),
                    payload: json!({
                        "reaction": response,
                        "issue_id": issue.id,
                        "issue_title": issue.title,
                        "issue_status": issue.status,
                        "creator_type": issue.creator_type,
                        "creator_id": issue.creator_id,
                        "issue_revision": reaction.issue_revision,
                    }),
                    ..Default::default()
                });
            }
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Ok(None) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to add reaction"),
        Err(error) => {
            tracing::warn!(%error, "failed to add issue reaction");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to add reaction")
        }
    }
}

async fn remove_issue_reaction(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ReactionRequest>,
) -> Response {
    if request.emoji.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "emoji is required");
    }
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let (actor_type, actor_id) = request_actor(&headers, &context);
    match issue_reaction::remove_issue_reaction(
        &state.pool,
        issue.id,
        actor_type,
        actor_id,
        &request.emoji,
    )
    .await
    {
        Ok(Some(removed)) => {
            if removed.changed {
                state.bus.publish(&patchbay_events::Event {
                    event_type: "issue:reaction_removed".into(),
                    workspace_id: context.workspace_id.clone(),
                    actor_type: actor_type.into(),
                    actor_id: actor_id.to_string(),
                    payload: json!({
                        "issue_id": issue.id,
                        "emoji": request.emoji,
                        "actor_type": actor_type,
                        "actor_id": actor_id,
                        "issue_revision": removed.issue_revision,
                    }),
                    ..Default::default()
                });
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to remove reaction",
        ),
        Err(error) => {
            tracing::warn!(%error, "failed to remove issue reaction");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to remove reaction",
            )
        }
    }
}

fn valid_metadata_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(first) if first == '_' || first.is_ascii_alphabetic())
        && key.len() <= 64
        && chars.all(|character| {
            character == '_'
                || character == '.'
                || character == '-'
                || character.is_ascii_alphanumeric()
        })
}

async fn list_issue_metadata(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    match resolve_issue(&state, &context, &id).await {
        Ok(issue) => Json(json!({ "metadata": issue.metadata })).into_response(),
        Err(response) => response,
    }
}

fn decode_property_value(body: &[u8]) -> Result<Option<Value>, ()> {
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let request = Value::deserialize(&mut deserializer).map_err(|_| ())?;
    let fields = request.as_object().ok_or(())?;
    Ok(fields.get("value").cloned())
}

async fn set_issue_property(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_issue, raw_property)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let property_id = match Uuid::parse_str(raw_property.trim()) {
        Ok(property_id) => property_id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid property id"),
    };
    let value = match decode_property_value(&body) {
        Ok(Some(value)) => value,
        Ok(None) => return error_response(StatusCode::BAD_REQUEST, "value is required"),
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let issue = match resolve_issue(&state, &context, &raw_issue).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, %property_id, "failed to begin property write");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to set property");
        }
    };
    if let Err(error) = sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("prop:{property_id}"))
        .execute(&mut *transaction)
        .await
    {
        tracing::warn!(%error, %property_id, "failed to lock property definition");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to set property");
    }
    let definition = match issue_property::get_issue_property(
        &mut *transaction,
        property_id,
        issue.workspace_id,
    )
    .await
    {
        Ok(Some(definition)) => definition,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "property not found"),
        Err(error) => {
            tracing::warn!(%error, %property_id, "failed to resolve property before set");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to set property");
        }
    };
    if definition.archived_at.is_some() {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!(
                "property {:?} is archived and cannot receive new values",
                definition.name
            ),
        );
    }
    let (stored, actor_refs) = match crate::issue_property_value::validate(&definition, &value) {
        Ok(validated) => validated,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    for actor in actor_refs {
        match member::get_member_by_user_and_workspace(
            &mut *transaction,
            actor.user_id,
            issue.workspace_id,
        )
        .await
        {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!(
                        "{:?} does not refer to a member of this workspace",
                        actor.value
                    ),
                );
            }
        }
    }
    let updated = match issue_property::set_issue_property_value(
        &mut *transaction,
        &property_id.to_string(),
        &stored,
        issue.id,
        issue.workspace_id,
    )
    .await
    {
        Ok(Some(updated)) => updated,
        Ok(None) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to set property");
        }
        Err(error)
            if error
                .downcast_ref::<sqlx::Error>()
                .and_then(sqlx::Error::as_database_error)
                .and_then(|error| error.code())
                .is_some_and(|code| code == "23514") =>
        {
            return error_response(
                StatusCode::BAD_REQUEST,
                "issue properties exceed the 16KB size limit",
            );
        }
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, %property_id, "failed to set property");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to set property");
        }
    };
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, issue_id = %issue.id, %property_id, "failed to commit property write");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to set property");
    }
    let properties = object_or_empty(updated.properties.clone());
    let (actor_type, actor_id, task_id) = mutation_actor(&state, &context, &headers).await;
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::EVENT_ISSUE_PROPERTIES_CHANGED.into(),
        workspace_id: updated.workspace_id.to_string(),
        actor_type,
        actor_id: actor_id.to_string(),
        payload: json!({
            "issue_id": updated.id,
            "properties": properties.clone(),
            "issue_revision": updated.revision,
        }),
        task_id: task_id.map(|id| id.to_string()).unwrap_or_default(),
        chat_session_id: String::new(),
    });
    Json(json!({
        "properties": properties,
        "issue_revision": updated.revision,
    }))
    .into_response()
}

async fn unset_issue_property(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_issue, raw_property)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let property_id = match Uuid::parse_str(raw_property.trim()) {
        Ok(property_id) => property_id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid property id"),
    };
    let issue = match resolve_issue(&state, &context, &raw_issue).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    match issue_property::get_issue_property(&state.pool, property_id, issue.workspace_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "property not found"),
        Err(error) => {
            tracing::warn!(%error, %property_id, "failed to resolve property before unset");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to unset property",
            );
        }
    }
    let updated = match issue_property::delete_issue_property_value(
        &state.pool,
        &property_id.to_string(),
        issue.id,
        issue.workspace_id,
    )
    .await
    {
        Ok(Some(updated)) => updated,
        Ok(None) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to unset property",
            );
        }
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, %property_id, "failed to unset property");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to unset property",
            );
        }
    };
    let properties = object_or_empty(updated.properties.clone());
    let (actor_type, actor_id, task_id) = mutation_actor(&state, &context, &headers).await;
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::EVENT_ISSUE_PROPERTIES_CHANGED.into(),
        workspace_id: updated.workspace_id.to_string(),
        actor_type,
        actor_id: actor_id.to_string(),
        payload: json!({
            "issue_id": updated.id,
            "properties": properties.clone(),
            "issue_revision": updated.revision,
        }),
        task_id: task_id.map(|id| id.to_string()).unwrap_or_default(),
        chat_session_id: String::new(),
    });
    Json(json!({
        "properties": properties,
        "issue_revision": updated.revision,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct SetMetadataRequest {
    value: Value,
}

async fn set_issue_metadata_key(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((id, key)): Path<(String, String)>,
    Json(request): Json<SetMetadataRequest>,
) -> Response {
    if !valid_metadata_key(&key) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "key must match ^[a-zA-Z_][a-zA-Z0-9_.-]{0,63}$",
        );
    }
    if !matches!(
        request.value,
        Value::String(_) | Value::Number(_) | Value::Bool(_)
    ) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "value must be a primitive: string, number, or bool",
        );
    }
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let count = issue.metadata.as_object().map_or(0, serde_json::Map::len);
    if issue.metadata.get(&key).is_none() && count >= 50 {
        return error_response(StatusCode::BAD_REQUEST, "metadata cannot exceed 50 keys");
    }
    match issue_q::set_issue_metadata_key(
        &state.pool,
        &key,
        &request.value,
        issue.id,
        issue.workspace_id,
    )
    .await
    {
        Ok(Some(updated)) => metadata_response(&state, &context, updated),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "issue not found"),
        Err(error)
            if error
                .downcast_ref::<sqlx::Error>()
                .and_then(|e| e.as_database_error())
                .and_then(|e| e.code())
                .is_some_and(|code| code == "23514") =>
        {
            error_response(
                StatusCode::BAD_REQUEST,
                "metadata exceeds the 8KB size limit",
            )
        }
        Err(error) => {
            tracing::warn!(%error, "failed to set metadata key");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to set metadata key",
            )
        }
    }
}

async fn delete_issue_metadata_key(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((id, key)): Path<(String, String)>,
) -> Response {
    if !valid_metadata_key(&key) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "key must match ^[a-zA-Z_][a-zA-Z0-9_.-]{0,63}$",
        );
    }
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    match issue_q::delete_issue_metadata_key(&state.pool, &key, issue.id, issue.workspace_id).await
    {
        Ok(Some(updated)) => metadata_response(&state, &context, updated),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "issue not found"),
        Err(error) => {
            tracing::warn!(%error, "failed to delete metadata key");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete metadata key",
            )
        }
    }
}

fn metadata_response(state: &HandlerState, context: &WorkspaceContext, issue: Issue) -> Response {
    state.bus.publish(&patchbay_events::Event {
        event_type: "issue:metadata_changed".into(),
        workspace_id: context.workspace_id.clone(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload: json!({
            "issue_id": issue.id,
            "metadata": issue.metadata,
            "issue_revision": issue.revision,
        }),
        ..Default::default()
    });
    Json(json!({ "metadata": issue.metadata, "issue_revision": issue.revision })).into_response()
}

/// Workspace guard for the issue group. Kept here because this slice needs a
/// JSON `Response` on every failure path; it uses the shared resolver and the
/// same `WorkspaceContext` type as `patchbay-middleware`.
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
    group_by: Option<String>,
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

#[derive(Debug, Default, Deserialize)]
struct ChildrenByParentsParams {
    parent_ids: Option<String>,
}

const LIST_CHILDREN_BY_PARENTS_LIMIT: usize = 200;

fn parse_parent_ids(raw: &str) -> Result<Vec<Uuid>, &'static str> {
    let parts = raw.split(',').collect::<Vec<_>>();
    if parts.len() > LIST_CHILDREN_BY_PARENTS_LIMIT {
        return Err("too many parent_ids");
    }
    parts
        .into_iter()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| Uuid::parse_str(value).map_err(|_| "invalid parent_ids"))
        .collect()
}

async fn list_children_by_parents(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<ChildrenByParentsParams>,
) -> Response {
    let raw = params.parent_ids.as_deref().unwrap_or_default();
    if raw.is_empty() {
        return Json(json!({ "issues": Vec::<IssueResponse>::new() })).into_response();
    }

    let parent_ids = match parse_parent_ids(raw) {
        Ok(ids) => ids,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };
    if parent_ids.is_empty() {
        return Json(json!({ "issues": Vec::<IssueResponse>::new() })).into_response();
    }

    match issue_q::list_children_by_parents(&state.pool, context.member.workspace_id, parent_ids)
        .await
    {
        Ok(issues) => Json(json!({
            "issues": enrich_issue_list(&state, &context, issues).await,
        }))
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, workspace_id = %context.member.workspace_id, "failed to list child issues");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list child issues",
            )
        }
    }
}

#[derive(Debug, Serialize)]
struct ChildProgressResponse {
    parent_issue_id: String,
    total: i64,
    done: i64,
}

async fn child_issue_progress(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    match issue_q::child_issue_progress(&state.pool, context.member.workspace_id).await {
        Ok(rows) => {
            let progress = rows
                .into_iter()
                .filter_map(|row| {
                    row.parent_issue_id
                        .map(|parent_issue_id| ChildProgressResponse {
                            parent_issue_id: parent_issue_id.to_string(),
                            total: row.total,
                            done: row.done,
                        })
                })
                .collect::<Vec<_>>();
            Json(json!({ "progress": progress })).into_response()
        }
        Err(error) => {
            tracing::warn!(%error, workspace_id = %context.member.workspace_id, "failed to get child issue progress");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get child issue progress",
            )
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct AssigneeFrequencyResponse {
    assignee_type: String,
    assignee_id: String,
    frequency: i64,
}

#[derive(Debug, FromRow)]
struct AssigneeActivityFrequencyRow {
    assignee_type: String,
    assignee_id: String,
    frequency: i64,
}

async fn get_assignee_frequency(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let workspace_id = context.member.workspace_id;
    let user_id = context.member.user_id;
    let (activity_counts, issue_counts) = tokio::join!(
        sqlx::query_as::<_, AssigneeActivityFrequencyRow>(
            r#"SELECT
                  details->>'to_type' AS assignee_type,
                  details->>'to_id' AS assignee_id,
                  COUNT(*)::bigint AS frequency
               FROM activity_log
              WHERE workspace_id = $1
                AND actor_id = $2
                AND actor_type = 'member'
                AND action = 'assignee_changed'
                AND details->>'to_type' IS NOT NULL
                AND details->>'to_id' IS NOT NULL
              GROUP BY details->>'to_type', details->>'to_id'"#,
        )
        .bind(workspace_id)
        .bind(user_id)
        .fetch_all(&state.pool),
        issue_q::count_created_issue_assignees(&state.pool, workspace_id, user_id),
    );
    let (activity_counts, issue_counts) = match (activity_counts, issue_counts) {
        (Ok(activity_counts), Ok(issue_counts)) => (activity_counts, issue_counts),
        (activity_result, issue_result) => {
            tracing::warn!(
                workspace_id = %workspace_id,
                activity_error = ?activity_result.err(),
                issue_error = ?issue_result.err(),
                "failed to get assignee frequency"
            );
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get assignee frequency",
            );
        }
    };

    let mut frequencies = HashMap::<(String, String), i64>::new();
    for row in activity_counts {
        *frequencies
            .entry((row.assignee_type, row.assignee_id))
            .or_default() += row.frequency;
    }
    for row in issue_counts {
        if let (Some(assignee_type), Some(assignee_id)) = (row.assignee_type, row.assignee_id) {
            *frequencies
                .entry((assignee_type, assignee_id.to_string()))
                .or_default() += row.frequency;
        }
    }

    let mut response = frequencies
        .into_iter()
        .map(
            |((assignee_type, assignee_id), frequency)| AssigneeFrequencyResponse {
                assignee_type,
                assignee_id,
                frequency,
            },
        )
        .collect::<Vec<_>>();
    response.sort_by_key(|entry| std::cmp::Reverse(entry.frequency));
    Json(response).into_response()
}

#[derive(Debug, FromRow)]
struct ListRow {
    acceptance_criteria: Value,
    assignee_id: Option<Uuid>,
    assignee_type: Option<String>,
    reviewer_id: Option<Uuid>,
    reviewer_type: Option<String>,
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
            reviewer_id: self.reviewer_id,
            reviewer_type: self.reviewer_type,
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
            .push(")) OR (i.assignee_type = 'team' AND i.assignee_id IN (SELECT sm.team_id FROM team_member sm JOIN team s ON s.id = sm.team_id WHERE s.workspace_id = ")
            .push_bind(filters.workspace_id).push(" AND sm.member_type = 'member' AND sm.member_id = ").push_bind(user_id)
            .push(" UNION SELECT s.id FROM team s JOIN agent a ON a.id = s.leader_id WHERE s.workspace_id = ")
            .push_bind(filters.workspace_id).push(" AND a.workspace_id = ").push_bind(filters.workspace_id).push(" AND a.owner_id = ").push_bind(user_id)
            .push(" UNION SELECT sm.team_id FROM team_member sm JOIN team s ON s.id = sm.team_id JOIN agent a ON a.id = sm.member_id WHERE s.workspace_id = ")
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
        .any(|kind| !matches!(kind.as_str(), "member" | "agent" | "team"))
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
    headers: HeaderMap,
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
            .map(|item| AttachmentResponse::for_request(&state, item, &headers))
            .collect();
    Json(response).into_response()
}

#[derive(Debug, Serialize)]
struct IssueUsageResponse {
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_cache_read_tokens: i64,
    total_cache_write_tokens: i64,
    cost_usd_ticks: i64,
    uncosted_input_tokens: i64,
    uncosted_output_tokens: i64,
    uncosted_cache_read_tokens: i64,
    uncosted_cache_write_tokens: i64,
    task_count: i32,
}

impl From<task_usage::GetIssueUsageSummaryRow> for IssueUsageResponse {
    fn from(row: task_usage::GetIssueUsageSummaryRow) -> Self {
        Self {
            total_input_tokens: row.total_input_tokens,
            total_output_tokens: row.total_output_tokens,
            total_cache_read_tokens: row.total_cache_read_tokens,
            total_cache_write_tokens: row.total_cache_write_tokens,
            cost_usd_ticks: row.total_cost_usd_ticks,
            uncosted_input_tokens: row.uncosted_input_tokens,
            uncosted_output_tokens: row.uncosted_output_tokens,
            uncosted_cache_read_tokens: row.uncosted_cache_read_tokens,
            uncosted_cache_write_tokens: row.uncosted_cache_write_tokens,
            task_count: row.task_count,
        }
    }
}

async fn get_issue_usage(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };

    match task_usage::get_issue_usage_summary(&state.pool, issue.id).await {
        Ok(Some(row)) => Json(IssueUsageResponse::from(row)).into_response(),
        Ok(None) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to get issue usage",
        ),
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to get issue usage");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get issue usage",
            )
        }
    }
}

async fn list_attachments(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };

    match attachment::list_attachments_by_issue(&state.pool, issue.id, issue.workspace_id).await {
        Ok(attachments) => Json(
            attachments
                .iter()
                .map(|item| AttachmentResponse::for_request(&state, item, &headers))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to list attachments");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list attachments",
            )
        }
    }
}

fn hydrate_task_user_ref(
    state: &HandlerState,
    attribution: &mut serde_json::Map<String, Value>,
    key: &str,
    users: &HashMap<Uuid, user::GetUsersByIDsRow>,
) {
    let Some(reference) = attribution.get_mut(key).and_then(Value::as_object_mut) else {
        return;
    };
    let Some(id) = reference
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
    else {
        return;
    };
    let Some(user) = users.get(&id) else { return };
    if !user.name.is_empty() {
        reference.insert("name".into(), Value::String(user.name.clone()));
    }
    if !user.email.is_empty() {
        reference.insert("email".into(), Value::String(user.email.clone()));
    }
    if let Some(avatar_url) = user.avatar_url.as_deref().filter(|url| !url.is_empty()) {
        reference.insert(
            "avatar_url".into(),
            Value::String(crate::avatar::resolve_url(state, avatar_url)),
        );
    }
}

async fn issue_task_maps(
    state: &HandlerState,
    issue: &Issue,
    tasks: &[AgentTaskQueue],
    include_usage: bool,
) -> Vec<Value> {
    let workspace_id = issue.workspace_id.to_string();
    let mut maps = task_maps(state, tasks, &workspace_id).await;

    if include_usage {
        if let Ok(rows) = task_usage::list_issue_task_usage(&state.pool, issue.id).await {
            let mut by_task = HashMap::<Uuid, Vec<Value>>::new();
            for row in rows {
                let Some(task_id) = row.task_id else { continue };
                let mut usage = serde_json::Map::new();
                if !row.provider.is_empty() {
                    usage.insert("provider".into(), Value::String(row.provider));
                }
                usage.insert("model".into(), Value::String(row.model));
                usage.insert("input_tokens".into(), json!(row.input_tokens));
                usage.insert("output_tokens".into(), json!(row.output_tokens));
                usage.insert("cache_read_tokens".into(), json!(row.cache_read_tokens));
                usage.insert("cache_write_tokens".into(), json!(row.cache_write_tokens));
                if let Some(cost) = row.cost_usd_ticks {
                    usage.insert("cost_usd_ticks".into(), json!(cost));
                }
                by_task
                    .entry(task_id)
                    .or_default()
                    .push(Value::Object(usage));
            }
            for (task, map) in tasks.iter().zip(&mut maps) {
                if let Some(usage) = by_task.remove(&task.id) {
                    if let Some(map) = map.as_object_mut() {
                        map.insert("usage".into(), Value::Array(usage));
                    }
                }
            }
        }
    }

    maps
}

pub(crate) async fn task_maps(
    state: &HandlerState,
    tasks: &[AgentTaskQueue],
    workspace_id: &str,
) -> Vec<Value> {
    let mut maps = tasks
        .iter()
        .map(|task| crate::task_json::task_to_map(task, workspace_id))
        .collect::<Vec<_>>();

    let mut user_ids = tasks
        .iter()
        .flat_map(|task| [task.accountable_user_id, task.originator_user_id])
        .flatten()
        .collect::<Vec<_>>();
    user_ids.sort_unstable();
    user_ids.dedup();
    if !user_ids.is_empty() {
        if let Ok(rows) = user::get_users_by_i_ds(&state.pool, user_ids).await {
            let users = rows
                .into_iter()
                .filter_map(|row| row.id.map(|id| (id, row)))
                .collect::<HashMap<_, _>>();
            for map in &mut maps {
                let Some(attribution) = map.get_mut("attribution").and_then(Value::as_object_mut)
                else {
                    continue;
                };
                hydrate_task_user_ref(state, attribution, "initiator", &users);
                hydrate_task_user_ref(state, attribution, "originator", &users);
            }
        }
    }

    maps
}

async fn get_active_tasks(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let tasks = agent::list_active_tasks_by_issue(&state.pool, issue.id)
        .await
        .unwrap_or_default();
    let tasks = issue_task_maps(&state, &issue, &tasks, false).await;
    Json(json!({ "tasks": tasks })).into_response()
}

async fn list_task_runs(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let tasks = match agent::list_tasks_by_issue(&state.pool, issue.id).await {
        Ok(tasks) => tasks,
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to list tasks");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list tasks");
        }
    };
    Json(issue_task_maps(&state, &issue, &tasks, true).await).into_response()
}

async fn cancel_task(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((id, task_id)): Path<(String, String)>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let task_id = match Uuid::parse_str(&task_id) {
        Ok(task_id) => task_id,
        Err(_) => return error_response(StatusCode::NOT_FOUND, "task not found"),
    };
    let existing = match agent::get_agent_task(&state.pool, task_id).await {
        Ok(Some(task)) if task.issue_id == Some(issue.id) => task,
        Ok(_) => return error_response(StatusCode::NOT_FOUND, "task not found"),
        Err(error) => {
            tracing::warn!(%error, %task_id, "failed to load task for cancellation");
            return error_response(StatusCode::NOT_FOUND, "task not found");
        }
    };
    let task = match state.tasks.cancel_task_by_user(existing.id).await {
        Ok(task) => task,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let mut tasks = issue_task_maps(&state, &issue, &[task], false).await;
    Json(tasks.remove(0)).into_response()
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

#[allow(clippy::result_large_err)]
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

#[allow(clippy::result_large_err)]
fn update_object(body: &[u8]) -> Result<serde_json::Map<String, Value>, Response> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(fields)) => Ok(fields),
        _ => Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid request body",
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IssueWorkflowViolation {
    ActiveAssigneeRequired,
    ReviewHandoffRequired,
}

fn issue_owner(issue: &Issue) -> Option<(&str, Uuid)> {
    issue.assignee_type.as_deref().zip(issue.assignee_id)
}

fn issue_reviewer(issue: &Issue) -> Option<(&str, Uuid)> {
    issue.reviewer_type.as_deref().zip(issue.reviewer_id)
}

fn leaves_review_for_implementation(previous_category: &str, next_category: &str) -> bool {
    previous_category == patchbay_service::issue_status::IN_REVIEW
        && next_category == patchbay_service::issue_status::IN_PROGRESS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReviewReturnActions {
    retire_reviewer_tasks: bool,
    record_executor_handoff: bool,
}

fn review_return_actions(leaving_review: bool, suppress_run: bool) -> ReviewReturnActions {
    ReviewReturnActions {
        retire_reviewer_tasks: leaving_review,
        record_executor_handoff: leaving_review && !suppress_run,
    }
}

struct LegacyReviewRemap<'a> {
    previous_category: &'a str,
    next_category: &'a str,
    previous_owner: Option<(&'a str, Uuid)>,
    reviewer_in_request: bool,
    assignee_touched: bool,
    next_owner_type: &'a mut Option<String>,
    next_owner_id: &'a mut Option<Uuid>,
    next_reviewer_type: &'a mut Option<String>,
    next_reviewer_id: &'a mut Option<Uuid>,
}

/// Older clients entered `in_review` by swapping the assignee to the
/// reviewer. The worker stays on `assignee_*`; the incoming assignee
/// becomes `reviewer_*` when the request did not set a reviewer.
fn remap_legacy_review_assignee(args: LegacyReviewRemap<'_>) -> bool {
    if args.reviewer_in_request
        || args.next_reviewer_type.is_some()
        || args.next_reviewer_id.is_some()
        || !args.assignee_touched
        || args.previous_category == patchbay_service::issue_status::IN_REVIEW
        || args.next_category != patchbay_service::issue_status::IN_REVIEW
    {
        return false;
    }
    let Some((prev_type, prev_id)) = args.previous_owner else {
        return false;
    };
    let Some(new_type) = args.next_owner_type.as_deref() else {
        return false;
    };
    let Some(new_id) = *args.next_owner_id else {
        return false;
    };
    if new_type == prev_type && new_id == prev_id {
        return false;
    }
    *args.next_reviewer_type = Some(new_type.to_string());
    *args.next_reviewer_id = Some(new_id);
    *args.next_owner_type = Some(prev_type.to_string());
    *args.next_owner_id = Some(prev_id);
    true
}

fn reviewer_cannot_clear_response() -> Response {
    issue_workflow_error(
        "reviewer_cannot_clear",
        "a reviewer cannot be removed once set",
    )
}

fn issue_workflow_violation(
    previous_category: &str,
    next_category: &str,
    previous_owner: Option<(&str, Uuid)>,
    next_owner: Option<(&str, Uuid)>,
    next_reviewer: Option<(&str, Uuid)>,
) -> Option<IssueWorkflowViolation> {
    let _ = previous_owner;
    if patchbay_service::issue_status::requires_assignee(next_category) && next_owner.is_none() {
        return Some(IssueWorkflowViolation::ActiveAssigneeRequired);
    }
    if previous_category != patchbay_service::issue_status::IN_REVIEW
        && next_category == patchbay_service::issue_status::IN_REVIEW
        && (next_reviewer.is_none() || next_reviewer == next_owner)
    {
        return Some(IssueWorkflowViolation::ReviewHandoffRequired);
    }
    None
}

fn issue_workflow_error(code: &str, message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "code": code, "error": message })),
    )
        .into_response()
}

fn issue_workflow_violation_response(violation: IssueWorkflowViolation) -> Response {
    match violation {
        IssueWorkflowViolation::ActiveAssigneeRequired => issue_workflow_error(
            "active_issue_requires_assignee",
            "issues in progress, in review, or blocked must have an assignee",
        ),
        IssueWorkflowViolation::ReviewHandoffRequired => issue_workflow_error(
            "review_handoff_required",
            "moving an issue into review requires assigning a different reviewer in the same update",
        ),
    }
}

async fn prevalidate_issue_workflow_update(
    state: &HandlerState,
    previous: &Issue,
    fields: &serde_json::Map<String, Value>,
) -> Result<(), Response> {
    let next_status = match update_field::<String>(fields, "status")? {
        UpdateField::Value(value) => {
            patchbay_service::issue_status::resolve(&state.pool, previous.workspace_id, &value)
                .await
                .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid status"))?
                .key
        }
        UpdateField::Missing | UpdateField::Null => previous.status.clone(),
    };
    let mut next_type = previous.assignee_type.clone();
    let mut next_id = previous.assignee_id;
    match update_field::<String>(fields, "assignee_type")? {
        UpdateField::Missing => {}
        UpdateField::Null => next_type = None,
        UpdateField::Value(value) => next_type = Some(value),
    }
    match update_field::<String>(fields, "assignee_id")? {
        UpdateField::Missing => {}
        UpdateField::Null => next_id = None,
        UpdateField::Value(value) => {
            next_id = Some(
                Uuid::parse_str(&value)
                    .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid assignee_id"))?,
            );
        }
    }
    if next_type.is_some() != next_id.is_some() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "assignee_type and assignee_id must be set together",
        ));
    }
    let assignee_touched = update_field::<String>(fields, "assignee_type")?.is_present()
        || update_field::<String>(fields, "assignee_id")?.is_present();
    let reviewer_type_field = update_field::<String>(fields, "reviewer_type")?;
    let reviewer_id_field = update_field::<String>(fields, "reviewer_id")?;
    let reviewer_in_request = reviewer_type_field.is_present() || reviewer_id_field.is_present();
    let mut next_reviewer_type = previous.reviewer_type.clone();
    let mut next_reviewer_id = previous.reviewer_id;
    match reviewer_type_field {
        UpdateField::Missing => {}
        UpdateField::Null => next_reviewer_type = None,
        UpdateField::Value(value) => next_reviewer_type = Some(value),
    }
    match reviewer_id_field {
        UpdateField::Missing => {}
        UpdateField::Null => next_reviewer_id = None,
        UpdateField::Value(value) => {
            next_reviewer_id = Some(
                Uuid::parse_str(&value)
                    .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid reviewer_id"))?,
            );
        }
    }
    if next_reviewer_type.is_some() != next_reviewer_id.is_some() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "reviewer_type and reviewer_id must be set together",
        ));
    }
    if issue_reviewer(previous).is_some() && next_reviewer_type.is_none() {
        return Err(reviewer_cannot_clear_response());
    }
    let previous_category = patchbay_service::issue_status::effective(
        &state.pool,
        previous.workspace_id,
        &previous.status,
    )
    .await;
    let next_category =
        patchbay_service::issue_status::effective(&state.pool, previous.workspace_id, &next_status)
            .await;
    remap_legacy_review_assignee(LegacyReviewRemap {
        previous_category: &previous_category,
        next_category: &next_category,
        previous_owner: issue_owner(previous),
        reviewer_in_request,
        assignee_touched,
        next_owner_type: &mut next_type,
        next_owner_id: &mut next_id,
        next_reviewer_type: &mut next_reviewer_type,
        next_reviewer_id: &mut next_reviewer_id,
    });
    if let Some(violation) = issue_workflow_violation(
        &previous_category,
        &next_category,
        issue_owner(previous),
        next_type.as_deref().zip(next_id),
        next_reviewer_type.as_deref().zip(next_reviewer_id),
    ) {
        return Err(issue_workflow_violation_response(violation));
    }
    Ok(())
}

async fn update_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
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
    match apply_issue_update(&state, &context, &headers, previous, &fields, true).await {
        Ok(issue) => issue_response(&state, issue).await,
        Err(response) => response,
    }
}

async fn apply_issue_update(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    previous: Issue,
    fields: &serde_json::Map<String, Value>,
    notify_parent: bool,
) -> Result<Issue, Response> {
    let expected_revision = match update_field::<i64>(fields, "expected_revision")? {
        UpdateField::Value(value) if value > 0 => Some(value),
        UpdateField::Missing | UpdateField::Null => None,
        UpdateField::Value(_) => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "expected_revision must be a positive integer",
            ));
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
        next.status = match patchbay_service::issue_status::resolve(
            &state.pool,
            previous.workspace_id,
            &value,
        )
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
        match (next.assignee_type.as_deref(), next.assignee_id) {
            (None, None) => {}
            (Some(kind), Some(id)) => validate_assignee(state, context, kind, id)
                .await
                .map_err(|message| error_response(StatusCode::BAD_REQUEST, &message))?,
            _ => {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "assignee_type and assignee_id must be set together",
                ));
            }
        }
    }

    let reviewer_type = update_field::<String>(fields, "reviewer_type")?;
    let reviewer_id = update_field::<String>(fields, "reviewer_id")?;
    let reviewer_in_request = reviewer_type.is_present() || reviewer_id.is_present();
    match reviewer_type {
        UpdateField::Missing => {}
        UpdateField::Null => next.reviewer_type = None,
        UpdateField::Value(value) => next.reviewer_type = Some(value),
    }
    match reviewer_id {
        UpdateField::Missing => {}
        UpdateField::Null => next.reviewer_id = None,
        UpdateField::Value(value) => {
            next.reviewer_id = Some(
                Uuid::parse_str(&value)
                    .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid reviewer_id"))?,
            )
        }
    }
    if reviewer_in_request {
        match (next.reviewer_type.as_deref(), next.reviewer_id) {
            (None, None) => {}
            (Some(kind), Some(id)) => validate_assignee(state, context, kind, id)
                .await
                .map_err(|message| error_response(StatusCode::BAD_REQUEST, &message))?,
            _ => {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "reviewer_type and reviewer_id must be set together",
                ));
            }
        }
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
                match patchbay_db::queries::project::get_project_in_workspace(
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
                        ));
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
                ));
            }
        };
    }

    let prelock_previous_category = patchbay_service::issue_status::effective(
        &state.pool,
        previous.workspace_id,
        &previous.status,
    )
    .await;
    let prelock_next_category =
        patchbay_service::issue_status::effective(&state.pool, next.workspace_id, &next.status)
            .await;
    let should_lock_reviewer_tasks =
        leaves_review_for_implementation(&prelock_previous_category, &prelock_next_category);

    let mut tx = state.pool.begin().await.map_err(|error| {
        tracing::warn!(%error, issue_id = %previous.id, "failed to begin issue update");
        error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
    })?;
    if fields.contains_key("status") {
        patchbay_db::queries::issue_status::lock_issue_status_catalog_shared(
            &mut *tx,
            previous.workspace_id,
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to lock issue status catalog");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
        })?;
        next.status = patchbay_service::issue_status::resolve(
            &mut *tx,
            previous.workspace_id,
            &next.status,
        )
        .await
        .map_err(|_| {
            error_response(
                StatusCode::CONFLICT,
                "the target status was archived while this request was in flight; reload the status list and retry",
            )
        })?
        .key;
    }
    if !attachment_ids.is_empty() {
        attachment::lock_attachments_for_issue_link(
            &mut *tx,
            previous.workspace_id,
            attachment_ids.clone(),
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, issue_id = %previous.id, "failed to lock issue attachments");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
        })?;
    }
    let locked_reviewer_task_ids = if should_lock_reviewer_tasks {
        patchbay_service::coordination::lock_active_reviewer_tasks_for_review_return(
            &mut tx,
            previous.id,
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, issue_id = %previous.id, "failed to lock reviewer tasks for review return");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
        })?
    } else {
        Vec::new()
    };
    let locked = sqlx::query_as::<_, Issue>(
        "SELECT * FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
    )
    .bind(previous.id)
    .bind(previous.workspace_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| {
        tracing::warn!(%error, issue_id = %previous.id, "failed to lock issue");
        error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
    })?
    .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "issue not found"))?;
    if let Some(expected) = expected_revision {
        if locked.revision != expected {
            return Err(revision_conflict(&locked, expected, locked.revision));
        }
    }

    refresh_untouched_fields(&mut next, &locked, fields);
    if let (UpdateField::Value(incoming), UpdateField::Value(base)) = (
        update_field::<String>(fields, "title")?,
        update_field::<String>(fields, "title_base")?,
    ) {
        if locked.title != base && locked.title != incoming {
            return Err(edit_conflict(&locked));
        }
    }
    if let UpdateField::Value(incoming) = update_field::<String>(fields, "description")? {
        let base = match update_field::<String>(fields, "description_base")? {
            UpdateField::Value(value) => Some(value),
            UpdateField::Missing | UpdateField::Null => None,
        };
        let attachments = attachment::list_attachments_by_issue(
            &mut *tx,
            locked.id,
            locked.workspace_id,
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, issue_id = %locked.id, "failed to load description attachments");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
        })?;
        let current = locked.description.clone().unwrap_or_default();
        if let Some(base) = base.as_deref() {
            if current != base && current != incoming {
                let base_with_late_media =
                    merge_channel_media_description(&current, base, Some(base), &attachments);
                if current != base_with_late_media {
                    return Err(edit_conflict(&locked));
                }
            }
        }
        next.description = Some(merge_channel_media_description(
            &current,
            &incoming,
            base.as_deref(),
            &attachments,
        ));
    }

    let previous_category =
        patchbay_service::issue_status::effective(&mut *tx, locked.workspace_id, &locked.status)
            .await;
    let next_category =
        patchbay_service::issue_status::effective(&mut *tx, next.workspace_id, &next.status).await;
    let leaving_review = leaves_review_for_implementation(&previous_category, &next_category);
    if leaving_review && !should_lock_reviewer_tasks {
        return Err(revision_conflict(
            &locked,
            previous.revision,
            locked.revision,
        ));
    }
    let remapped = remap_legacy_review_assignee(LegacyReviewRemap {
        previous_category: &previous_category,
        next_category: &next_category,
        previous_owner: issue_owner(&locked),
        reviewer_in_request,
        assignee_touched,
        next_owner_type: &mut next.assignee_type,
        next_owner_id: &mut next.assignee_id,
        next_reviewer_type: &mut next.reviewer_type,
        next_reviewer_id: &mut next.reviewer_id,
    });
    if issue_reviewer(&locked).is_some() && next.reviewer_type.is_none() {
        return Err(reviewer_cannot_clear_response());
    }
    if remapped {
        if let (Some(kind), Some(id)) = (next.reviewer_type.as_deref(), next.reviewer_id) {
            validate_assignee(state, context, kind, id)
                .await
                .map_err(|message| error_response(StatusCode::BAD_REQUEST, &message))?;
        }
    }
    if let Some(violation) = issue_workflow_violation(
        &previous_category,
        &next_category,
        issue_owner(&locked),
        issue_owner(&next),
        issue_reviewer(&next),
    ) {
        return Err(issue_workflow_violation_response(violation));
    }

    let previous = locked;
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
reviewer_type = $15, reviewer_id = $16,
revision = revision + 1, updated_at = now(),
last_activity_at = CASE WHEN $17 THEN GREATEST(COALESCE(last_activity_at, updated_at), now()) ELSE last_activity_at END
WHERE id = $1 AND workspace_id = $2
  AND ($18::bigint IS NULL OR revision = $18)
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
        .bind(&next.reviewer_type)
        .bind(next.reviewer_id)
        .bind(did_activity)
        .bind(expected_revision)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| {
            tracing::warn!(%error, issue_id = %previous.id, "failed to update issue");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
        })?;
        let Some(updated) = updated else {
            let actual =
                issue_q::get_issue_in_workspace(&mut *tx, previous.id, previous.workspace_id)
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
    let mut attachments_changed = false;
    if !attachment_ids.is_empty() {
        let linked = patchbay_db::queries::attachment::link_attachments_to_issue(
            &mut *tx,
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
            attachments_changed = true;
            if let Ok(Some(current)) =
                issue_q::get_issue_in_workspace(&mut *tx, previous.id, previous.workspace_id).await
            {
                updated = current;
            }
        }
    }
    let review_return_actions = review_return_actions(leaving_review, suppress_run);
    let retired_reviewer_tasks = if review_return_actions.retire_reviewer_tasks {
        patchbay_service::coordination::retire_locked_reviewer_tasks_for_review_return(
            &mut tx,
            &locked_reviewer_task_ids,
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, issue_id = %previous.id, "failed to retire reviewer tasks for review return");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
        })?
    } else {
        Vec::new()
    };
    // `suppress_run` is an explicit request to leave returned work idle. The
    // owner restoration and reviewer retirement still belong to the atomic
    // issue update, but it must not create a durable executor handoff.
    let should_record_review_return = review_return_actions.record_executor_handoff;
    if should_record_review_return {
        // Persist the executor handoff in the same transaction as the review
        // return. The coordinator's PostgreSQL outbox is authoritative; the
        // in-memory notification below is only a latency hint.
        let handoff_note = (!handoff_note.is_empty()).then_some(handoff_note.as_str());
        patchbay_service::coordination::record_review_return(
            &mut tx,
            &updated,
            retired_reviewer_tasks.first().map(|task| task.id),
            handoff_note,
        )
            .await
            .map_err(|error| {
                tracing::warn!(%error, issue_id = %previous.id, "failed to record review return handoff");
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to update issue",
                )
            })?;
    }
    tx.commit().await.map_err(|error| {
        tracing::warn!(%error, issue_id = %previous.id, "failed to commit issue update");
        error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
    })?;
    if should_record_review_return {
        state.coordinator.notify();
    }
    if !retired_reviewer_tasks.is_empty() {
        state
            .tasks
            .publish_transactional_cancellations(&retired_reviewer_tasks)
            .await;
    }
    let (actor_type, actor_id, task_id) = mutation_actor(state, context, headers).await;
    publish_issue_updated(state, &previous, &updated, &actor_type, actor_id, task_id).await;
    if attachments_changed {
        publish_issue_attachments_changed(state, &updated, &actor_type, actor_id, task_id);
    }
    let assignee_changed = previous.assignee_type != updated.assignee_type
        || previous.assignee_id != updated.assignee_id;
    let status_changed = previous.status != updated.status;
    if !suppress_run && !leaving_review {
        let is_self_loop = if let Some(task_id) = task_id {
            agent::get_agent_task(&state.pool, task_id)
                .await
                .ok()
                .flatten()
                .is_some_and(|task| task.issue_id == Some(updated.id))
        } else {
            false
        };
        let suppress_active_self_assignment = if actor_type == "agent"
            && updated.assignee_type.as_deref() == Some("agent")
            && updated.assignee_id == Some(actor_id)
        {
            agent::has_active_task_for_issue_and_agent(&state.pool, updated.id, actor_id)
                .await
                .map(|active| active.unwrap_or(true))
                .unwrap_or(true)
        } else {
            false
        };
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
                    is_self_loop: Some(Box::new(move |_| is_self_loop)),
                    suppress_active_self_assignment: Some(Box::new(move |_| {
                        suppress_active_self_assignment
                    })),
                },
            )
            .await;
        if let Some(trigger) = trigger {
            let actor_user_id = (actor_type == "member").then_some(actor_id);
            let result = if trigger.assignee_type == "team" {
                state
                    .tasks
                    .enqueue_task_for_team_leader_with_handoff(
                        &updated,
                        trigger.agent_id,
                        updated.assignee_id.unwrap_or_default(),
                        &handoff_note,
                        actor_user_id,
                    )
                    .await
            } else {
                state
                    .tasks
                    .enqueue_task_for_issue_with_handoff(&updated, &handoff_note, actor_user_id)
                    .await
            };
            if let Err(error) = result {
                tracing::warn!(%error, issue_id = %updated.id, "failed to enqueue updated issue");
            }
        }
    }
    if notify_parent && status_changed {
        notify_parent_of_child_done(state, &previous, &updated).await;
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
        || left.reviewer_type != right.reviewer_type
        || left.reviewer_id != right.reviewer_id
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
        || left.reviewer_type != right.reviewer_type
        || left.reviewer_id != right.reviewer_id
        || left.start_date != right.start_date
        || left.due_date != right.due_date
        || left.parent_issue_id != right.parent_issue_id
        || left.project_id != right.project_id
        || left.stage != right.stage
}

fn refresh_untouched_fields(
    next: &mut Issue,
    current: &Issue,
    fields: &serde_json::Map<String, Value>,
) {
    if !fields.contains_key("title") {
        next.title = current.title.clone();
    }
    if !fields.contains_key("description") {
        next.description = current.description.clone();
    }
    if !fields.contains_key("status") {
        next.status = current.status.clone();
    }
    if !fields.contains_key("priority") {
        next.priority = current.priority.clone();
    }
    if !fields.contains_key("position") {
        next.position = current.position;
    }
    if !fields.contains_key("assignee_type") && !fields.contains_key("assignee_id") {
        next.assignee_type = current.assignee_type.clone();
        next.assignee_id = current.assignee_id;
    }
    if !fields.contains_key("reviewer_type") && !fields.contains_key("reviewer_id") {
        next.reviewer_type = current.reviewer_type.clone();
        next.reviewer_id = current.reviewer_id;
    }
    if !fields.contains_key("start_date") {
        next.start_date = current.start_date;
    }
    if !fields.contains_key("due_date") {
        next.due_date = current.due_date;
    }
    if !fields.contains_key("parent_issue_id") {
        next.parent_issue_id = current.parent_issue_id;
    }
    if !fields.contains_key("project_id") {
        next.project_id = current.project_id;
    }
    if !fields.contains_key("stage") {
        next.stage = current.stage;
    }
}

fn edit_conflict(issue: &Issue) -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "resource changed since it was loaded",
            "code": "edit_conflict",
            "resource_type": "issue",
            "resource_id": issue.id.to_string(),
        })),
    )
        .into_response()
}

fn marked_media_ids(markdown: &str) -> Vec<Uuid> {
    const PREFIX: &str = "<!-- patchbay:channel-media:";
    let mut ids = Vec::new();
    let mut remaining = markdown;
    while let Some(index) = remaining.find(PREFIX) {
        remaining = &remaining[index + PREFIX.len()..];
        let Some(raw) = remaining.get(..36) else {
            break;
        };
        if remaining.get(36..40) == Some(" -->") {
            if let Ok(id) = Uuid::parse_str(raw) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        remaining = remaining.get(36..).unwrap_or_default();
    }
    ids
}

fn media_marker(id: Uuid) -> String {
    format!("<!-- patchbay:channel-media:{id} -->")
}

fn append_markdown(markdown: &str, block: &str) -> String {
    if markdown.is_empty() {
        block.to_string()
    } else {
        format!("{markdown}\n\n{block}")
    }
}

fn merge_channel_media_description(
    current: &str,
    incoming: &str,
    base: Option<&str>,
    attachments: &[Attachment],
) -> String {
    let current_ids = marked_media_ids(current);
    if current_ids.is_empty() {
        return incoming.to_string();
    }
    let base_ids = base.map(marked_media_ids).unwrap_or_default();
    let mut merged = incoming.to_string();
    for id in current_ids {
        let Some(attachment) = attachments.iter().find(|attachment| attachment.id == id) else {
            continue;
        };
        let path = format!("/api/attachments/{id}/download");
        let has_link = merged.contains(&path);
        if base.is_some() && base_ids.contains(&id) && !has_link {
            continue;
        }
        if !has_link {
            let block = if attachment.content_type.starts_with("image/") {
                format!("![]({path})\n\n{}", media_marker(id))
            } else {
                let label = attachment
                    .filename
                    .replace('\\', "\\\\")
                    .replace('[', "\\[")
                    .replace(']', "\\]")
                    .replace(['\r', '\n'], " ");
                format!(
                    "[{}]({path})\n\n{}",
                    if label.is_empty() {
                        "attachment"
                    } else {
                        &label
                    },
                    media_marker(id)
                )
            };
            merged = append_markdown(&merged, &block);
        } else if !merged.contains(&media_marker(id)) {
            merged = append_markdown(&merged, &media_marker(id));
        }
    }
    merged
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
    Json(issue_response_projection(state, &issue).await).into_response()
}

pub(crate) async fn issue_response_projection(
    state: &HandlerState,
    issue: &Issue,
) -> IssueResponse {
    let mut response =
        IssueResponse::from_issue(issue, &issue_prefix(state, issue.workspace_id).await);
    response.status_category = Some(
        patchbay_service::issue_status::effective(&state.pool, issue.workspace_id, &issue.status)
            .await,
    );
    response
}

pub(crate) async fn mutation_actor(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
) -> (String, Uuid, Option<Uuid>) {
    if let Some((task_id, agent_id)) = trusted_agent_task(state, context, headers).await {
        ("agent".to_string(), agent_id, Some(task_id))
    } else {
        ("member".to_string(), context.member.user_id, None)
    }
}

pub(crate) async fn publish_issue_updated(
    state: &HandlerState,
    previous: &Issue,
    issue: &Issue,
    actor_type: &str,
    actor_id: Uuid,
    task_id: Option<Uuid>,
) {
    let prefix = issue_prefix(state, issue.workspace_id).await;
    let category =
        patchbay_service::issue_status::effective(&state.pool, issue.workspace_id, &issue.status)
            .await;
    let previous_category = patchbay_service::issue_status::effective(
        &state.pool,
        previous.workspace_id,
        &previous.status,
    )
    .await;
    let assignee_changed =
        previous.assignee_type != issue.assignee_type || previous.assignee_id != issue.assignee_id;
    let review_handoff = previous_category != patchbay_service::issue_status::IN_REVIEW
        && category == patchbay_service::issue_status::IN_REVIEW
        && issue_reviewer(issue).is_some();
    let mut response = IssueResponse::from_issue(issue, &prefix);
    response.status_category = Some(category);
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::EVENT_ISSUE_UPDATED.to_string(),
        workspace_id: issue.workspace_id.to_string(),
        actor_type: actor_type.to_string(),
        actor_id: actor_id.to_string(),
        payload: json!({
            "issue": response,
            "assignee_changed": assignee_changed,
            "status_changed": previous.status != issue.status,
            "review_handoff": review_handoff,
            "priority_changed": previous.priority != issue.priority,
            "project_changed": previous.project_id != issue.project_id,
            "start_date_changed": previous.start_date != issue.start_date,
            "due_date_changed": previous.due_date != issue.due_date,
            "description_changed": previous.description != issue.description,
            "title_changed": previous.title != issue.title,
            "prev_title": previous.title,
            "prev_assignee_type": previous.assignee_type,
            "prev_assignee_id": previous.assignee_id.map(|id| id.to_string()),
            "prev_status": previous.status,
            "prev_priority": previous.priority,
            "prev_start_date": previous.start_date.map(|date| date.format("%Y-%m-%d").to_string()),
            "prev_due_date": previous.due_date.map(|date| date.format("%Y-%m-%d").to_string()),
            "prev_description": previous.description,
            "creator_type": previous.creator_type,
            "creator_id": previous.creator_id.to_string(),
        }),
        task_id: task_id.map(|id| id.to_string()).unwrap_or_default(),
        chat_session_id: String::new(),
    });
}

/// Applies the PR-merge auto-completion path without bypassing issue-domain
/// side effects. Both GitHub and token-based VCS webhooks use the same
/// combined sibling barrier before calling this helper.
pub(crate) async fn advance_issue_to_done_from_pr(
    state: &HandlerState,
    previous: &Issue,
    source: &str,
) -> Option<Issue> {
    let current_category = patchbay_service::issue_status::effective(
        &state.pool,
        previous.workspace_id,
        &previous.status,
    )
    .await;
    if terminal_category(&current_category) {
        return None;
    }
    let updated = match issue_q::update_issue_status(
        &state.pool,
        previous.id,
        "done",
        previous.workspace_id,
    )
    .await
    {
        Ok(Some(issue)) => issue,
        Ok(None) => return None,
        Err(error) => {
            tracing::warn!(%error, issue_id = %previous.id, "failed to complete issue from pull request");
            return None;
        }
    };
    notify_parent_of_child_done(state, previous, &updated).await;
    let prefix = issue_prefix(state, updated.workspace_id).await;
    let mut response = IssueResponse::from_issue(&updated, &prefix);
    response.status_category = Some("done".into());
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::EVENT_ISSUE_UPDATED.into(),
        workspace_id: updated.workspace_id.to_string(),
        actor_type: "system".into(),
        payload: pr_completion_event_payload(previous, response, source),
        ..Default::default()
    });
    Some(updated)
}

fn pr_completion_event_payload(previous: &Issue, response: IssueResponse, source: &str) -> Value {
    json!({
        "issue": response,
        "assignee_changed": false,
        "status_changed": true,
        "priority_changed": false,
        "project_changed": false,
        "prev_status": previous.status,
        "creator_type": previous.creator_type,
        "creator_id": previous.creator_id.to_string(),
        "source": source,
    })
}

fn publish_issue_attachments_changed(
    state: &HandlerState,
    issue: &Issue,
    actor_type: &str,
    actor_id: Uuid,
    task_id: Option<Uuid>,
) {
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::EVENT_ISSUE_ATTACHMENTS_CHANGED.to_string(),
        workspace_id: issue.workspace_id.to_string(),
        actor_type: actor_type.to_string(),
        actor_id: actor_id.to_string(),
        payload: json!({
            "issue_id": issue.id.to_string(),
            "issue_revision": issue.revision,
        }),
        task_id: task_id.map(|id| id.to_string()).unwrap_or_default(),
        chat_session_id: String::new(),
    });
}

fn terminal_category(category: &str) -> bool {
    matches!(category, "done" | "cancelled")
}

async fn notify_parent_of_child_done(state: &HandlerState, previous: &Issue, issue: &Issue) {
    let Some(parent_id) = issue.parent_issue_id else {
        return;
    };
    let mut resolver = patchbay_service::issue_status::Resolver::new(issue.workspace_id);
    let previous_category = resolver.effective(&state.pool, &previous.status).await;
    let current_category = resolver.effective(&state.pool, &issue.status).await;
    if terminal_category(&previous_category) || !terminal_category(&current_category) {
        return;
    }
    let Some(parent) = issue_q::get_issue_in_workspace(&state.pool, parent_id, issue.workspace_id)
        .await
        .ok()
        .flatten()
    else {
        return;
    };
    let parent_category = resolver.effective(&state.pool, &parent.status).await;
    if matches!(parent_category.as_str(), "backlog" | "done" | "cancelled")
        || parent.assignee_type.as_deref() == Some("member")
    {
        return;
    }
    let children = match issue_q::list_child_issues(&state.pool, parent.id).await {
        Ok(children) => children,
        Err(error) => {
            tracing::warn!(%error, parent_id = %parent.id, "failed to inspect child completion barrier");
            return;
        }
    };
    let staged = children.iter().any(|child| child.stage.is_some());
    if staged && issue.stage.is_none() {
        return;
    }
    let closed_stage = issue.stage;
    for child in &children {
        if staged {
            let Some(child_stage) = child.stage else {
                continue;
            };
            if child_stage > closed_stage.unwrap_or_default() {
                continue;
            }
        }
        let category = resolver.effective(&state.pool, &child.status).await;
        if !terminal_category(&category) {
            return;
        }
    }

    let (mention, target_agent, team_id) =
        match (parent.assignee_type.as_deref(), parent.assignee_id) {
            (Some("agent"), Some(agent_id)) => (
                format!("[@assignee](mention://agent/{agent_id}) "),
                Some(agent_id),
                None,
            ),
            (Some("team"), Some(team_id)) => {
                let leader = team::get_team_in_workspace(&state.pool, team_id, parent.workspace_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|team| team.leader_id);
                (
                    format!("[@team](mention://team/{team_id}) "),
                    leader,
                    Some(team_id),
                )
            }
            _ => (String::new(), None, None),
        };
    let identifier = format!(
        "{}-{}",
        issue_prefix(state, issue.workspace_id).await,
        issue.number
    );
    let progress = if let Some(stage) = closed_stage {
        format!("Stage {stage} is complete")
    } else {
        "All sub-issues are complete".to_string()
    };
    let content = format!(
        "{mention}{progress} — the last sub-issue [{identifier}](mention://issue/{}) — \"{}\" — just finished. Continue the parent or move it to review when complete.",
        issue.id,
        issue.title.replace(['\r', '\n'], " ")
    );
    let created = match patchbay_db::queries::comment::create_comment(
        &state.pool,
        parent.id,
        parent.workspace_id,
        "system",
        Uuid::nil(),
        &content,
        "system",
        None,
        None,
        None,
        None,
        patchbay_db::dbid::new_v7(),
    )
    .await
    {
        Ok(Some(created)) => created,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%error, parent_id = %parent.id, "failed to create child completion comment");
            return;
        }
    };
    let comment_id = created.id;
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::EVENT_COMMENT_CREATED.to_string(),
        workspace_id: parent.workspace_id.to_string(),
        actor_type: "system".to_string(),
        actor_id: String::new(),
        payload: json!({
            "comment": {
                "id": created.id.map(|id| id.to_string()),
                "issue_id": created.issue_id.map(|id| id.to_string()),
                "author_type": created.author_type,
                "author_id": created.author_id.map(|id| id.to_string()),
                "content": created.content,
                "type": created.type_,
                "revision": created.revision,
            },
            "issue_title": parent.title,
            "issue_revision": created.issue_revision,
        }),
        task_id: String::new(),
        chat_session_id: String::new(),
    });
    if let (Some(agent_id), Some(comment_id)) = (target_agent, comment_id) {
        let result = if let Some(team_id) = team_id {
            state
                .tasks
                .enqueue_task_for_team_leader(&parent, agent_id, team_id, Some(comment_id))
                .await
        } else {
            state
                .tasks
                .enqueue_task_for_mention(&parent, agent_id, Some(comment_id))
                .await
        };
        if let Err(error) = result {
            tracing::warn!(%error, parent_id = %parent.id, "failed to wake parent assignee");
        }
    }
}

async fn batch_update_issues(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
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

    if let Some(Value::String(status)) = updates.get("status") {
        if patchbay_service::issue_status::resolve(&state.pool, context.member.workspace_id, status)
            .await
            .is_err()
        {
            return invalid_status(&state, context.member.workspace_id, status).await;
        }
    } else if updates.contains_key("status") {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    }
    if let Some(Value::String(priority)) = updates.get("priority") {
        if !PRIORITIES.contains(&priority.as_str()) {
            return error_response(StatusCode::BAD_REQUEST, "invalid priority");
        }
    } else if updates.contains_key("priority") {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    }
    if let Some(project) = updates.get("project_id") {
        if !project.is_null() {
            let Some(raw) = project.as_str() else {
                return error_response(StatusCode::BAD_REQUEST, "invalid project_id");
            };
            let Ok(project_id) = Uuid::parse_str(raw) else {
                return error_response(StatusCode::BAD_REQUEST, "invalid project_id");
            };
            if !matches!(
                patchbay_db::queries::project::get_project_in_workspace(
                    &state.pool,
                    project_id,
                    context.member.workspace_id,
                )
                .await,
                Ok(Some(_))
            ) {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "project not found in this workspace",
                );
            }
        }
    }

    let mut pending = Vec::new();
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
        pending.push(previous);
    }
    for previous in &pending {
        if let Err(response) = prevalidate_issue_workflow_update(&state, previous, updates).await {
            return response;
        }
    }

    let mut updated = 0usize;
    let mut parent_notifications = HashMap::<Uuid, (Issue, Issue)>::new();
    for previous in pending {
        let previous_snapshot = previous.clone();
        if let Ok(issue) =
            apply_issue_update(&state, &context, &headers, previous, updates, false).await
        {
            if previous_snapshot.status != issue.status {
                if let Some(parent_id) = issue.parent_issue_id {
                    let replace = parent_notifications
                        .get(&parent_id)
                        .is_none_or(|(_, current)| issue.stage > current.stage);
                    if replace {
                        parent_notifications.insert(parent_id, (previous_snapshot, issue));
                    }
                }
            }
            updated += 1;
        }
    }
    for (_, (previous, issue)) in parent_notifications {
        notify_parent_of_child_done(&state, &previous, &issue).await;
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
        state.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_ISSUE_LABELS_CHANGED.to_string(),
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
        match patchbay_service::issue_status::resolve(&state.pool, workspace_id, &status).await {
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
        .is_some_and(|kind| !matches!(kind, "member" | "agent" | "team"))
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
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to create issue");
            };
            let mut response = IssueResponse::from_issue(&issue, &prefix);
            response.status_category = Some(status_category);
            response.labels = Some(result.labels.iter().map(LabelResponse::from).collect());
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(IssueCreateError::ActiveDuplicate { duplicate }) => {
            let duplicate = duplicate.map(|issue| IssueResponse::from_issue(&issue, &prefix));
            (
                StatusCode::CONFLICT,
                Json(json!({
                    "code": "active_duplicate_issue",
                    "error": "an active duplicate issue already exists",
                    "issue": duplicate,
                })),
            )
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
        Err(IssueCreateError::ActiveAssigneeRequired) => issue_workflow_error(
            "active_issue_requires_assignee",
            "issues in progress, in review, or blocked must have an assignee",
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
    let (task_id, agent_id) = authoritative_task_actor_headers(headers)?;
    // Resolve through the workspace-scoped join before trusting the task.
    // Callers that bind a mutation to one issue retain their stricter
    // task.issue_id checks after this shared actor-resolution boundary.
    let task =
        agent::get_agent_task_in_workspace(&state.pool, task_id, context.member.workspace_id)
            .await
            .ok()
            .flatten()?;
    if !task_belongs_to_claimed_agent(task.agent_id, agent_id) {
        return None;
    }
    agent::get_agent_in_workspace(&state.pool, agent_id, context.member.workspace_id)
        .await
        .ok()
        .flatten()
        .filter(|agent| agent.archived_at.is_none())?;
    Some((task_id, agent_id))
}

fn authoritative_task_actor_headers(headers: &HeaderMap) -> Option<(Uuid, Uuid)> {
    if headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        != Some("task_token")
    {
        return None;
    }
    Some((
        header_uuid(headers, "x-task-id")?,
        header_uuid(headers, "x-agent-id")?,
    ))
}

fn task_belongs_to_claimed_agent(task_agent_id: Uuid, claimed_agent_id: Uuid) -> bool {
    task_agent_id == claimed_agent_id
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
        "team" => {
            let target = team::get_team_in_workspace(&state.pool, id, workspace_id)
                .await
                .ok()
                .flatten()
                .filter(|team| team.archived_at.is_none())
                .ok_or_else(|| "assignee team not found in this workspace".to_string())?;
            let leader = agent::get_agent_in_workspace(&state.pool, target.leader_id, workspace_id)
                .await
                .ok()
                .flatten()
                .filter(|agent| agent.archived_at.is_none())
                .ok_or_else(|| "team leader is unavailable".to_string())?;
            if !can_member_invoke_agent(state, context.member.user_id, workspace_id, &leader).await
            {
                return Err("you do not have permission to invoke this team".to_string());
            }
        }
        _ => return Err("invalid assignee_type".to_string()),
    }
    Ok(())
}

pub(crate) async fn can_member_invoke_agent(
    state: &HandlerState,
    user_id: Uuid,
    workspace_id: Uuid,
    target: &patchbay_db::models::Agent,
) -> bool {
    can_invoke_agent(state, "member", Some(user_id), workspace_id, target).await
}

/// Invoke gate keyed on the human originator, not the speaking agent owner.
/// Agent/system actors with no originator may only hit a `public_to workspace`
/// target; member-scoped grants stay fail-closed.
pub(crate) async fn can_invoke_agent(
    state: &HandlerState,
    actor_type: &str,
    originator_user_id: Option<Uuid>,
    workspace_id: Uuid,
    target: &patchbay_db::models::Agent,
) -> bool {
    if originator_user_id.is_some_and(|user_id| target.owner_id == Some(user_id)) {
        return true;
    }
    if target.permission_mode != "public_to" {
        return false;
    }
    let workspace_broad = matches!(actor_type, "agent" | "system");
    let is_member = match originator_user_id {
        Some(user_id) => {
            member::get_member_by_user_and_workspace(&state.pool, user_id, workspace_id)
                .await
                .ok()
                .flatten()
                .is_some()
        }
        None => false,
    };
    agent_invocation_target::list_agent_invocation_targets(&state.pool, target.id)
        .await
        .unwrap_or_default()
        .iter()
        .any(|entry| match entry.target_type.as_str() {
            "workspace" => is_member || workspace_broad,
            "member" => originator_user_id == Some(entry.target_id),
            _ => false,
        })
}

pub(crate) async fn resolve_issue(
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
        patchbay_service::issue_status::Resolver::new(context.member.workspace_id);
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

pub(crate) async fn issue_prefix(state: &HandlerState, workspace_id: Uuid) -> String {
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

pub(crate) fn legacy_issue_prefix(name: &str) -> String {
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
    let allowed = patchbay_service::issue_status::active_keys(&state.pool, workspace_id)
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
    let entries = patchbay_db::queries::issue_status::list_issue_status_entries(
        &state.pool,
        workspace_id,
        true,
    )
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
        if patchbay_service::issue_status::is_built_in(category) {
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
            if !matches!(actor_type, "member" | "agent" | "team") || id.trim().is_empty() {
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
pub(crate) struct IssueResponse {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    reviewer_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reviewer_id: Option<String>,
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
    pub(crate) fn from_issue(issue: &Issue, prefix: &str) -> Self {
        Self {
            id: issue.id.to_string(),
            workspace_id: issue.workspace_id.to_string(),
            number: issue.number,
            identifier: format!("{prefix}-{}", issue.number),
            title: issue.title.clone(),
            description: issue.description.clone(),
            status: issue.status.clone(),
            status_category: patchbay_service::issue_status::is_built_in(&issue.status)
                .then(|| issue.status.clone()),
            priority: issue.priority.clone(),
            assignee_type: issue.assignee_type.clone(),
            assignee_id: issue.assignee_id.map(|id| id.to_string()),
            reviewer_type: issue.reviewer_type.clone(),
            reviewer_id: issue.reviewer_id.map(|id| id.to_string()),
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
            last_activity_at: issue.last_activity_at.map(patchbay_util::rfc3339_nano),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    issue_revision: Option<i64>,
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
            issue_revision: None,
        }
    }
}

impl IssueReactionResponse {
    fn from_added(reaction: &AddIssueReactionRow) -> Option<Self> {
        Some(Self {
            id: reaction.id?.to_string(),
            issue_id: reaction.issue_id?.to_string(),
            actor_type: reaction.actor_type.clone(),
            actor_id: reaction.actor_id?.to_string(),
            emoji: reaction.emoji.clone(),
            created_at: timestamp(reaction.created_at?),
            issue_revision: (reaction.issue_revision > 0).then_some(reaction.issue_revision),
        })
    }
}

#[derive(Debug, Serialize)]
struct SubscriberResponse {
    issue_id: String,
    user_type: String,
    user_id: String,
    reason: String,
    created_at: String,
}

impl From<&IssueSubscriber> for SubscriberResponse {
    fn from(subscriber: &IssueSubscriber) -> Self {
        Self {
            issue_id: subscriber.issue_id.to_string(),
            user_type: subscriber.user_type.clone(),
            user_id: subscriber.user_id.to_string(),
            reason: subscriber.reason.clone(),
            created_at: timestamp(subscriber.created_at),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct AttachmentResponse {
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

impl AttachmentResponse {
    pub(crate) fn for_request(
        state: &HandlerState,
        attachment: &Attachment,
        headers: &HeaderMap,
    ) -> Self {
        let mut response = Self::from(attachment);
        response.download_url = crate::attachment::bulk_download_url(state, attachment, headers);
        response
    }
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
    use http_body_util::BodyExt as _;
    use patchbay_auth::pat_cache::PatCache;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt as _;

    #[test]
    fn table_fingerprint_is_stable_for_the_same_query() {
        let left = TableRequest {
            query: json!({"filters":{"statuses":["todo"]}}),
            ..Default::default()
        };
        let right = TableRequest {
            query: json!({"filters":{"statuses":["todo"]}}),
            ..Default::default()
        };
        assert_eq!(table_fingerprint(&left), table_fingerprint(&right));
    }

    fn fixture_issue() -> Issue {
        let timestamp = Utc.with_ymd_and_hms(2026, 8, 23, 3, 30, 0).unwrap();
        let last_activity_at = chrono::DateTime::parse_from_rfc3339("2026-08-23T03:30:00.123400Z")
            .unwrap()
            .to_utc();
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
            last_activity_at: Some(last_activity_at),
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
            reviewer_id: None,
            reviewer_type: None,
            stage: Some(4),
            start_date: None,
            status: "in_progress".into(),
            title: "Port handlers".into(),
            updated_at: timestamp,
            workspace_id: Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f10").unwrap(),
        }
    }

    fn fixture_attachment(id: Uuid) -> Attachment {
        Attachment {
            chat_message_id: None,
            chat_session_id: None,
            comment_id: None,
            content_type: "image/png".into(),
            created_at: Utc.with_ymd_and_hms(2026, 8, 23, 3, 30, 0).unwrap(),
            filename: "diagram.png".into(),
            id,
            issue_id: Some(fixture_issue().id),
            size_bytes: 42,
            task_id: None,
            uploader_id: fixture_issue().creator_id,
            uploader_type: "member".into(),
            url: "https://static.example.test/workspaces/w/diagram.png".into(),
            workspace_id: fixture_issue().workspace_id,
        }
    }

    #[test]
    fn issue_response_matches_go_wire_shape() {
        let value =
            serde_json::to_value(IssueResponse::from_issue(&fixture_issue(), "CORD")).unwrap();
        assert_eq!(value["identifier"], "CORD-14");
        assert_eq!(value["status_category"], "in_progress");
        assert_eq!(value["created_at"], "2026-08-23T03:30:00Z");
        assert_eq!(value["last_activity_at"], "2026-08-23T03:30:00.1234Z");
        assert!(value.get("description").is_some_and(Value::is_null));
        assert!(value.get("assignee_id").is_some_and(Value::is_null));
        assert!(value.get("parent_issue_id").is_some_and(Value::is_null));
        assert!(value.get("project_id").is_some_and(Value::is_null));
        assert!(value.get("start_date").is_some_and(Value::is_null));
        assert!(value.get("due_date").is_some_and(Value::is_null));
        assert_eq!(value["metadata"], json!({}));
        assert_eq!(value["properties"], json!({}));
        assert!(value.get("reactions").is_none());
        assert!(value.get("attachments").is_none());
        assert!(value.get("labels").is_none());
    }

    #[test]
    fn pr_completion_event_matches_go_wire_shape() {
        let previous = fixture_issue();
        let mut updated = previous.clone();
        updated.status = "done".into();
        updated.revision += 1;
        let mut response = IssueResponse::from_issue(&updated, "CORD");
        response.status_category = Some("done".into());
        let issue = serde_json::to_value(&response).expect("issue response");

        assert_eq!(
            pr_completion_event_payload(&previous, response, "github_pr_merged"),
            json!({
                "issue": issue,
                "assignee_changed": false,
                "status_changed": true,
                "priority_changed": false,
                "project_changed": false,
                "prev_status": "in_progress",
                "creator_type": "member",
                "creator_id": "018f03a0-c4d2-7a37-ae4d-5aa45de12f12",
                "source": "github_pr_merged",
            })
        );
    }

    #[test]
    fn list_parameter_validation_rejects_malformed_ids() {
        assert!(optional_uuid(Some("not-a-uuid"), "assignee_id").is_err());
        assert!(uuid_list(Some("not-a-uuid"), "ids").is_err());
        assert!(uuid_list(Some(""), "ids").unwrap().is_empty());
    }

    #[test]
    fn children_by_parents_enforces_uuid_and_fanout_limits() {
        assert!(parse_parent_ids("").unwrap().is_empty());
        assert_eq!(
            parse_parent_ids("018f03a0-c4d2-7a37-ae4d-5aa45de12f11")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            parse_parent_ids("not-a-uuid").unwrap_err(),
            "invalid parent_ids"
        );
        assert_eq!(
            parse_parent_ids(&vec![Uuid::nil().to_string(); 201].join(",")).unwrap_err(),
            "too many parent_ids"
        );
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
    fn property_value_decoder_preserves_missing_null_and_go_first_value_behavior() {
        assert_eq!(decode_property_value(br#"{"other":1}"#), Ok(None));
        assert_eq!(
            decode_property_value(br#"{"value":null}"#),
            Ok(Some(Value::Null))
        );
        assert_eq!(
            decode_property_value(br#"{"value":"high","unknown":true} trailing"#),
            Ok(Some(json!("high")))
        );
        assert!(decode_property_value(b"[]").is_err());
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
    fn move_request_allows_review_run_controls() {
        assert!(is_allowed_move_field("suppress_run"));
        assert!(is_allowed_move_field("handoff_note"));
        assert!(!is_allowed_move_field("move_intent"));
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

    #[test]
    fn active_workflow_requires_an_owner_and_review_requires_a_reviewer() {
        let owner_a = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f21").unwrap();
        let owner_b = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f22").unwrap();

        assert_eq!(
            issue_workflow_violation("todo", "in_progress", None, None, None),
            Some(IssueWorkflowViolation::ActiveAssigneeRequired)
        );
        assert_eq!(
            issue_workflow_violation(
                "in_progress",
                "in_review",
                Some(("agent", owner_a)),
                Some(("agent", owner_a)),
                None,
            ),
            Some(IssueWorkflowViolation::ReviewHandoffRequired)
        );
        assert_eq!(
            issue_workflow_violation(
                "in_progress",
                "in_review",
                Some(("agent", owner_a)),
                Some(("agent", owner_a)),
                Some(("agent", owner_a)),
            ),
            Some(IssueWorkflowViolation::ReviewHandoffRequired)
        );
        assert_eq!(
            issue_workflow_violation(
                "in_progress",
                "in_review",
                Some(("agent", owner_a)),
                Some(("agent", owner_b)),
                None,
            ),
            Some(IssueWorkflowViolation::ReviewHandoffRequired)
        );
        assert_eq!(
            issue_workflow_violation(
                "in_progress",
                "in_review",
                Some(("agent", owner_a)),
                Some(("agent", owner_a)),
                Some(("agent", owner_b)),
            ),
            None
        );
        assert_eq!(
            issue_workflow_violation("todo", "done", None, None, None),
            None
        );
        assert_eq!(
            issue_workflow_violation("in_progress", "cancelled", None, None, None),
            None
        );
    }

    #[test]
    fn legacy_in_review_assignee_swap_becomes_the_reviewer() {
        let owner_a = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f21").unwrap();
        let owner_b = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f22").unwrap();
        let mut next_type = Some("agent".to_string());
        let mut next_id = Some(owner_b);
        let mut reviewer_type = None;
        let mut reviewer_id = None;
        assert!(remap_legacy_review_assignee(LegacyReviewRemap {
            previous_category: "in_progress",
            next_category: "in_review",
            previous_owner: Some(("agent", owner_a)),
            reviewer_in_request: false,
            assignee_touched: true,
            next_owner_type: &mut next_type,
            next_owner_id: &mut next_id,
            next_reviewer_type: &mut reviewer_type,
            next_reviewer_id: &mut reviewer_id,
        }));
        assert_eq!(next_type.as_deref(), Some("agent"));
        assert_eq!(next_id, Some(owner_a));
        assert_eq!(reviewer_type.as_deref(), Some("agent"));
        assert_eq!(reviewer_id, Some(owner_b));
    }

    #[test]
    fn every_review_to_implementation_transition_is_a_review_return() {
        assert!(leaves_review_for_implementation("in_review", "in_progress"));
        assert!(!leaves_review_for_implementation(
            "in_progress",
            "in_review"
        ));
    }

    #[test]
    fn suppress_run_still_retires_reviewer_without_executor_handoff() {
        assert_eq!(
            review_return_actions(true, true),
            ReviewReturnActions {
                retire_reviewer_tasks: true,
                record_executor_handoff: false,
            }
        );
        assert_eq!(
            review_return_actions(true, false),
            ReviewReturnActions {
                retire_reviewer_tasks: true,
                record_executor_handoff: true,
            }
        );
        assert_eq!(
            review_return_actions(false, false),
            ReviewReturnActions {
                retire_reviewer_tasks: false,
                record_executor_handoff: false,
            }
        );
    }

    #[test]
    fn reassignment_while_leaving_review_uses_the_coordinator_handoff() {
        assert!(leaves_review_for_implementation("in_review", "in_progress"));
        assert_eq!(
            review_return_actions(true, false),
            ReviewReturnActions {
                retire_reviewer_tasks: true,
                record_executor_handoff: true,
            }
        );
        let source = include_str!("issue.rs");
        assert!(
            source.contains("record_review_return(\n            &mut tx,\n            &updated")
        );
        assert!(source.contains("if !suppress_run && !leaving_review"));
    }

    #[test]
    fn review_return_locks_reviewer_tasks_before_issue() {
        let source = include_str!("issue.rs");
        let issue_lock = source
            .find("SELECT * FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE")
            .expect("issue row lock");
        let reviewer_task_lock = source
            .find("lock_active_reviewer_tasks_for_review_return")
            .expect("reviewer task lock");
        assert!(reviewer_task_lock < issue_lock);
        assert!(source.contains("prelock_previous_category"));
    }

    #[test]
    fn description_merge_preserves_only_late_channel_media() {
        let id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f13").unwrap();
        let path = format!("/api/attachments/{id}/download");
        let block = format!("![]({path})\n\n{}", media_marker(id));
        let current = append_markdown("Original", &block);
        let attachments = vec![fixture_attachment(id)];

        let merged = merge_channel_media_description(
            &current,
            "Original with local edit",
            Some("Original"),
            &attachments,
        );
        assert!(merged.contains("Original with local edit"));
        assert!(merged.contains(&path));
        assert!(merged.contains(&media_marker(id)));

        let deleted = merge_channel_media_description(
            &current,
            "Original with local edit",
            Some(&current),
            &attachments,
        );
        assert!(!deleted.contains(&path));
    }

    #[test]
    fn locked_snapshot_refreshes_fields_the_request_did_not_touch() {
        let mut next = fixture_issue();
        let mut current = next.clone();
        current.priority = "urgent".into();
        current.title = "concurrent title".into();
        let fields = update_object(br#"{"title":"local title"}"#).unwrap();
        next.title = "local title".into();
        refresh_untouched_fields(&mut next, &current, &fields);
        assert_eq!(next.title, "local title");
        assert_eq!(next.priority, "urgent");
    }

    #[test]
    fn reaction_and_subscriber_responses_match_go_wire_contracts() {
        let id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f11").unwrap();
        let actor_id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f12").unwrap();
        let created_at = chrono::DateTime::parse_from_rfc3339("2026-08-23T12:34:56.789Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let reaction = IssueReactionResponse::from_added(&AddIssueReactionRow {
            id: Some(id),
            issue_id: Some(id),
            workspace_id: Some(id),
            actor_type: "member".into(),
            actor_id: Some(actor_id),
            emoji: "👍".into(),
            created_at: Some(created_at),
            issue_revision: 7,
        })
        .unwrap();
        let reaction = serde_json::to_value(reaction).unwrap();
        assert_eq!(reaction["created_at"], "2026-08-23T12:34:56Z");
        assert_eq!(reaction["issue_revision"], 7);
        assert!(reaction.get("workspace_id").is_none());

        let subscriber = serde_json::to_value(SubscriberResponse::from(&IssueSubscriber {
            created_at,
            issue_id: id,
            opt_out_scope: Some("subtree".into()),
            reason: "manual".into(),
            unsubscribed_at: Some(created_at),
            user_id: actor_id,
            user_type: "member".into(),
        }))
        .unwrap();
        assert_eq!(subscriber["created_at"], "2026-08-23T12:34:56Z");
        assert!(subscriber.get("opt_out_scope").is_none());
        assert!(subscriber.get("unsubscribed_at").is_none());
    }

    #[test]
    fn actor_headers_require_server_stamped_task_token_source() {
        let user_id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f11").unwrap();
        let agent_id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f12").unwrap();
        let workspace_id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f13").unwrap();
        let context = WorkspaceContext {
            workspace_id: workspace_id.to_string(),
            member: patchbay_db::models::Member {
                created_at: chrono::Utc::now(),
                id: Uuid::nil(),
                role: "member".into(),
                user_id,
                workspace_id,
            },
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-agent-id", agent_id.to_string().parse().unwrap());
        headers.insert("x-task-id", Uuid::new_v4().to_string().parse().unwrap());
        assert!(authoritative_task_actor_headers(&headers).is_none());
        assert_eq!(request_actor(&headers, &context), ("member", user_id));

        headers.insert("x-actor-source", "task_token".parse().unwrap());
        assert!(authoritative_task_actor_headers(&headers).is_some());
        assert_eq!(request_actor(&headers, &context), ("agent", agent_id));
        assert!(task_belongs_to_claimed_agent(agent_id, agent_id));
        assert!(!task_belongs_to_claimed_agent(Uuid::new_v4(), agent_id));
    }

    #[test]
    fn issue_usage_response_matches_go_wire_contract() {
        let response = IssueUsageResponse::from(task_usage::GetIssueUsageSummaryRow {
            total_input_tokens: 1,
            total_output_tokens: 2,
            total_cache_read_tokens: 3,
            total_cache_write_tokens: 4,
            total_cost_usd_ticks: 5,
            uncosted_input_tokens: 6,
            uncosted_output_tokens: 7,
            uncosted_cache_read_tokens: 8,
            uncosted_cache_write_tokens: 9,
            task_count: 10,
        });
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "total_input_tokens": 1,
                "total_output_tokens": 2,
                "total_cache_read_tokens": 3,
                "total_cache_write_tokens": 4,
                "cost_usd_ticks": 5,
                "uncosted_input_tokens": 6,
                "uncosted_output_tokens": 7,
                "uncosted_cache_read_tokens": 8,
                "uncosted_cache_write_tokens": 9,
                "task_count": 10,
            })
        );
    }

    #[tokio::test]
    async fn attachment_list_response_matches_go_stable_url_contract() {
        let attachment =
            fixture_attachment(Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f13").unwrap());
        let response = serde_json::to_value(AttachmentResponse::from(&attachment)).unwrap();
        let stable_url = format!("/api/attachments/{}/download", attachment.id);
        assert_eq!(response["download_url"], stable_url);
        assert_eq!(response["markdown_url"], stable_url);
        assert_eq!(response["created_at"], "2026-08-23T03:30:00Z");
        assert_eq!(response["issue_id"], fixture_issue().id.to_string());
        assert!(response.get("task_id").is_none());

        let state = HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            patchbay_auth::pat_cache::PatCache::disabled(),
            None,
        );
        let negotiated = serde_json::to_value(AttachmentResponse::for_request(
            &state,
            &attachment,
            &HeaderMap::new(),
        ))
        .unwrap();
        assert_eq!(negotiated["download_url"], stable_url);

        let mut signing_state = HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            patchbay_auth::pat_cache::PatCache::disabled(),
            None,
        );
        signing_state.attachment_download.cloudfront_signer = Some(std::sync::Arc::new(
            crate::cloudfront::CloudFrontSigner::test_signer(),
        ));
        let signed =
            AttachmentResponse::for_request(&signing_state, &attachment, &HeaderMap::new());
        assert!(
            signed.download_url.contains("Policy="),
            "{}",
            signed.download_url
        );
        assert_eq!(signed.markdown_url, stable_url);

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-client-capabilities",
            "stable_attachment_urls".parse().unwrap(),
        );
        let stable = AttachmentResponse::for_request(&signing_state, &attachment, &headers);
        assert_eq!(stable.download_url, stable_url);
    }

    #[test]
    fn search_parser_matches_identifier_and_multi_term_contract() {
        assert_eq!(search_number("CORD-42"), Some(42));
        assert_eq!(search_number("42"), Some(42));
        assert_eq!(search_number("CORD-extra-42"), None);
        assert_eq!(search_number("bad-0"), None);
        assert_eq!(
            search_patterns("  Alpha   beta "),
            vec!["%alpha%", "%beta%"]
        );
        assert_eq!(
            search_patterns(r"100% _done"),
            vec![r"%100\%%", r"%\_done%"]
        );
    }

    #[test]
    fn search_snippet_handles_casefolded_unicode_without_slicing_mid_codepoint() {
        let raw = format!("İ{}目标", "x".repeat(240));
        let snippet = search_snippet(&raw, "目标");
        assert!(snippet.chars().count() <= 242);
        assert!(snippet.is_char_boundary(snippet.len()));
    }

    #[test]
    fn table_cursor_is_bound_to_query_group_and_parent() {
        let mut request = TableRequest {
            query: json!({"scope":{"kind":"workspace"},"filters":{},"sort":{"field":"position","direction":"asc"}}),
            group: json!({"kind":"status"}),
            group_key: Some("status:todo".into()),
            hierarchy: json!({"enabled":true}),
            parent_id: Some("018f03a0-c4d2-7a37-ae4d-5aa45de12f11".into()),
            facets: Vec::new(),
            page: json!({"limit":25}),
        };
        let fingerprint = table_fingerprint(&request);
        let cursor = encode_table_cursor(&request, &fingerprint, 25);
        request.page = json!({"limit":25,"cursor":cursor});
        assert_eq!(table_cursor(&request, &fingerprint).unwrap(), (25, 25));
        request.group_key = Some("status:done".into());
        assert!(table_cursor(&request, &fingerprint).is_err());
    }

    #[tokio::test]
    async fn production_create_route_returns_duplicate_contract_and_top_positions() {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required for issue create HTTP contract");
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect contract PostgreSQL");
        let slug = format!("issue-create-http-{}", Uuid::now_v7().simple());
        let workspace_id: Uuid = sqlx::query_scalar(
            "INSERT INTO workspace (name, slug) VALUES ('issue create HTTP', $1) RETURNING id",
        )
        .bind(slug)
        .fetch_one(&pool)
        .await
        .expect("create workspace");
        let user_id = Uuid::now_v7();
        let context = WorkspaceContext {
            workspace_id: workspace_id.to_string(),
            member: patchbay_db::models::Member {
                created_at: Utc::now(),
                id: Uuid::now_v7(),
                role: "member".into(),
                user_id,
                workspace_id,
            },
        };
        let app = router()
            .with_state(HandlerState::new(pool.clone(), PatCache::disabled(), None))
            .layer(Extension(context));

        async fn post(app: &Router, body: Value) -> (StatusCode, Value) {
            let response = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method(axum::http::Method::POST)
                        .uri("/api/issues")
                        .header(axum::http::header::CONTENT_TYPE, "application/json")
                        .body(axum::body::Body::from(body.to_string()))
                        .expect("create request"),
                )
                .await
                .expect("create response");
            let status = response.status();
            let bytes = response
                .into_body()
                .collect()
                .await
                .expect("response body")
                .to_bytes();
            (status, serde_json::from_slice(&bytes).expect("JSON body"))
        }

        let (status, first) = post(
            &app,
            json!({"title": "  HTTP\tDuplicate  ", "status": "todo"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(first["position"], -1.0);

        let (status, duplicate) =
            post(&app, json!({"title": "http duplicate", "status": "todo"})).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(duplicate["code"], "active_duplicate_issue");
        assert_eq!(
            duplicate["error"],
            "an active duplicate issue already exists"
        );
        assert_eq!(duplicate["issue"]["id"], first["id"]);
        assert_eq!(duplicate["issue"]["identifier"], first["identifier"]);
        assert_eq!(duplicate["issue"]["title"], first["title"]);
        assert_eq!(duplicate["issue"]["status"], first["status"]);

        let (status, allowed) = post(
            &app,
            json!({
                "title": "http duplicate",
                "status": "todo",
                "allow_duplicate": true,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(allowed["position"], -2.0);
        assert_eq!(first["number"], 1);
        assert_eq!(allowed["number"], 2);

        sqlx::query("DELETE FROM issue WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("delete issues");
        sqlx::query("DELETE FROM workspace WHERE id = $1")
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("delete workspace");
    }

    #[test]
    fn table_sort_fields_include_frontend_contract() {
        assert_eq!(table_sort_column("title"), Some("i.title"));
        assert_eq!(
            table_sort_column("last_activity"),
            Some("i.last_activity_at")
        );
        assert_eq!(
            table_sort_column("last_activity_at"),
            Some("i.last_activity_at")
        );
        assert_eq!(table_sort_column("start_date"), Some("i.start_date"));
        assert_eq!(table_sort_column("due_date"), Some("i.due_date"));
        assert_eq!(table_sort_column("position"), Some("i.position"));
        assert!(table_sort_column("property:018f03a0-c4d2-7a37-ae4d-5aa45de12f10").is_none());
        assert!(table_sort_column("unknown").is_none());
    }

    #[test]
    fn property_filters_expand_array_and_boolean_containment() {
        let id = "018f03a0-c4d2-7a37-ae4d-5aa45de12f10";
        let multi = property_filter_containment(id, &json!("opt-a"));
        assert!(multi.contains(&json!({ id: "opt-a" })));
        assert!(multi.contains(&json!({ id: ["opt-a"] })));
        let checkbox = property_filter_containment(id, &json!("true"));
        assert!(checkbox.contains(&json!({ id: true })));
        let unset = property_filter_containment(id, &json!("false"));
        assert!(unset.contains(&json!({ id: false })));
    }

    #[test]
    fn group_descriptors_include_required_discriminator_fields() {
        let assignee = table_group_descriptor(
            "assignee",
            "member:018f03a0-c4d2-7a37-ae4d-5aa45de12f12",
            3,
            None,
        );
        assert_eq!(assignee["value"]["kind"], "assignee");
        assert_eq!(assignee["value"]["actor"]["type"], "member");
        assert_eq!(
            assignee["value"]["actor"]["id"],
            "018f03a0-c4d2-7a37-ae4d-5aa45de12f12"
        );

        let unassigned = table_group_descriptor("assignee", "unassigned", 1, None);
        assert_eq!(unassigned["value"]["actor"], Value::Null);

        let project =
            table_group_descriptor("project", "018f03a0-c4d2-7a37-ae4d-5aa45de12f11", 2, None);
        assert_eq!(
            project["value"]["project_id"],
            "018f03a0-c4d2-7a37-ae4d-5aa45de12f11"
        );
        assert_eq!(
            table_group_descriptor("project", "unassigned", 0, None)["value"]["project_id"],
            Value::Null
        );

        let parent =
            table_group_descriptor("parent", "018f03a0-c4d2-7a37-ae4d-5aa45de12f11", 4, None);
        assert_eq!(parent["value"]["kind"], "parent");
        assert_eq!(parent["value"]["value_state"], "unavailable");
        assert_eq!(
            table_group_descriptor("parent", "root", 1, None)["value"]["value_state"],
            "unset"
        );

        let property_id = "018f03a0-c4d2-7a37-ae4d-5aa45de12f13";
        let property = table_group_descriptor("property", "value:red", 5, Some(property_id));
        assert_eq!(property["value"]["kind"], "property");
        assert_eq!(property["value"]["property_id"], property_id);
        assert_eq!(property["value"]["value_state"], "value");
        assert_eq!(property["value"]["value"], "red");
    }

    #[test]
    fn compound_group_keys_round_trip_primary_and_status() {
        let key = compound_cell_group_key("assignee:member:abc", "todo", false);
        assert!(key.starts_with("compound:"));
        assert!(key.contains(":status:todo"));
        let category = compound_cell_group_key("project:none", "in_progress", true);
        assert!(category.contains(":status_category:in_progress"));
    }
}
