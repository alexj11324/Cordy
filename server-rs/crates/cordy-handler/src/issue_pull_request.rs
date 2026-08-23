//! Pull-request cards attached to an issue.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use cordy_db::queries::{github, vcs};
use cordy_middleware::workspace::WorkspaceContext;
use serde::Serialize;

use crate::error::error_response;
use crate::state::HandlerState;

#[derive(Debug, Serialize)]
struct PullRequestResponse {
    id: String,
    provider: String,
    workspace_id: String,
    repo_owner: String,
    repo_name: String,
    number: i32,
    title: String,
    state: String,
    html_url: String,
    branch: Option<String>,
    author_login: Option<String>,
    author_avatar_url: Option<String>,
    merged_at: Option<String>,
    closed_at: Option<String>,
    pr_created_at: String,
    pr_updated_at: String,
    mergeable_state: Option<String>,
    mergeable: Option<String>,
    merge_state_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_available: Option<bool>,
    checks_rollup: Option<String>,
    checks_conclusion: Option<String>,
    checks_total: i64,
    checks_passed: i64,
    checks_failed: i64,
    checks_running: i64,
    checks_pending: i64,
    failed_check_names: Vec<String>,
    snapshot_stale: bool,
    snapshot_fetched_at: Option<String>,
    additions: i32,
    deletions: i32,
    changed_files: i32,
}

fn id(value: Option<uuid::Uuid>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn timestamp(value: Option<DateTime<Utc>>) -> String {
    value.map(crate::timefmt::rfc3339).unwrap_or_default()
}

fn optional_timestamp(value: Option<DateTime<Utc>>) -> Option<String> {
    value.map(crate::timefmt::rfc3339)
}

fn aggregate_checks(failed: i64, passed: i64, pending: i64, total: i64) -> Option<String> {
    if total == 0 {
        return None;
    }
    match (failed, pending, passed) {
        (value, _, _) if value > 0 => Some("failed".into()),
        (_, value, _) if value > 0 => Some("pending".into()),
        (_, _, value) if value > 0 => Some("passed".into()),
        _ => None,
    }
}

fn github_response(row: github::ListPullRequestsByIssueRow) -> PullRequestResponse {
    // HandlerState does not yet carry the S7 ghsnapshot manager. This is the
    // exact Go PRRefresh-disabled projection: stored snapshot fields remain
    // hidden until the manager wiring slice lands.
    PullRequestResponse {
        id: id(row.id),
        provider: "github".into(),
        workspace_id: id(row.workspace_id),
        repo_owner: row.repo_owner,
        repo_name: row.repo_name,
        number: row.pr_number,
        title: row.title,
        state: row.state,
        html_url: row.html_url,
        branch: row.branch,
        author_login: row.author_login,
        author_avatar_url: row.author_avatar_url,
        merged_at: optional_timestamp(row.merged_at),
        closed_at: optional_timestamp(row.closed_at),
        pr_created_at: timestamp(row.pr_created_at),
        pr_updated_at: timestamp(row.pr_updated_at),
        mergeable_state: row.mergeable_state,
        mergeable: None,
        merge_state_status: None,
        snapshot_available: Some(false),
        checks_rollup: None,
        checks_conclusion: None,
        checks_total: 0,
        checks_passed: 0,
        checks_failed: 0,
        checks_running: 0,
        checks_pending: 0,
        failed_check_names: Vec::new(),
        snapshot_stale: false,
        snapshot_fetched_at: None,
        additions: row.additions,
        deletions: row.deletions,
        changed_files: row.changed_files,
    }
}

fn vcs_response(row: vcs::ListVCSPullRequestsByIssueRow) -> PullRequestResponse {
    let checks_conclusion = aggregate_checks(
        row.checks_failed,
        row.checks_passed,
        row.checks_pending,
        row.checks_total,
    );
    PullRequestResponse {
        id: id(row.id),
        provider: row.provider,
        workspace_id: id(row.workspace_id),
        repo_owner: row.repo_owner,
        repo_name: row.repo_name,
        number: row.pr_number,
        title: row.title,
        state: row.state,
        html_url: row.html_url,
        branch: row.branch,
        author_login: row.author_login,
        author_avatar_url: row.author_avatar_url,
        merged_at: optional_timestamp(row.merged_at),
        closed_at: optional_timestamp(row.closed_at),
        pr_created_at: timestamp(row.pr_created_at),
        pr_updated_at: timestamp(row.pr_updated_at),
        mergeable_state: None,
        mergeable: None,
        merge_state_status: None,
        snapshot_available: None,
        checks_rollup: None,
        checks_conclusion,
        checks_total: row.checks_total,
        checks_passed: row.checks_passed,
        checks_failed: row.checks_failed,
        checks_running: row.checks_pending,
        checks_pending: row.checks_pending,
        failed_check_names: Vec::new(),
        snapshot_stale: false,
        snapshot_fetched_at: None,
        additions: row.additions,
        deletions: row.deletions,
        changed_files: row.changed_files,
    }
}

pub(crate) async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_issue): Path<String>,
) -> Response {
    let issue = match crate::issue::resolve_issue(&state, &context, &raw_issue).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let github_rows = match github::list_pull_requests_by_issue(&state.pool, issue.id).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to list GitHub pull requests");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list pull requests",
            );
        }
    };
    let vcs_rows = match vcs::list_vcs_pull_requests_by_issue(&state.pool, issue.id).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to list VCS pull requests");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list pull requests",
            );
        }
    };
    let mut pull_requests = github_rows
        .into_iter()
        .map(github_response)
        .chain(vcs_rows.into_iter().map(vcs_response))
        .collect::<Vec<_>>();
    pull_requests.sort_by(|left, right| right.pr_created_at.cmp(&left.pr_created_at));
    Json(serde_json::json!({ "pull_requests": pull_requests })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vcs_check_aggregation_matches_failure_pending_pass_priority() {
        assert_eq!(aggregate_checks(1, 4, 2, 7).as_deref(), Some("failed"));
        assert_eq!(aggregate_checks(0, 4, 2, 6).as_deref(), Some("pending"));
        assert_eq!(aggregate_checks(0, 4, 0, 4).as_deref(), Some("passed"));
        assert_eq!(aggregate_checks(0, 0, 0, 0), None);
    }
}
