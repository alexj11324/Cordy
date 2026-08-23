//! Workspace project read handlers.

use std::collections::HashMap;

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::models::Project;
use cordy_db::queries::{project, project_resource};
use cordy_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const SEARCH_STATEMENT_TIMEOUT_MS: i64 = 3_000;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/projects/search", get(search))
        .route("/api/projects", get(list))
        .route("/api/projects/", get(list))
        .route("/api/projects/{id}", get(get_one))
        .route("/api/projects/{id}/", get(get_one))
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
    fn search_parsing_matches_go_defaults_and_caps() {
        assert_eq!(parse_positive(&None, 20), 20);
        assert_eq!(parse_positive(&Some("0".into()), 20), 20);
        assert_eq!(parse_positive(&Some("75".into()), 20).min(50), 50);
        assert_eq!(parse_non_negative(&Some("-1".into()), 0), 0);
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
