//! Pull-request cards attached to an issue.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use cordy_db::models::GithubPullRequest;
use cordy_db::queries::{github, vcs};
use cordy_middleware::workspace::WorkspaceContext;
use serde::Serialize;

use crate::error::error_response;
use crate::state::HandlerState;

#[derive(Debug, Serialize)]
pub(crate) struct PullRequestResponse {
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

fn rollup_conclusion(
    rollup: Option<&str>,
    failed: i64,
    running: i64,
    passed: i64,
) -> Option<String> {
    let rollup = rollup.filter(|value| !value.is_empty())?;
    match rollup.to_ascii_uppercase().as_str() {
        "FAILURE" | "ERROR" => Some("failed".into()),
        "PENDING" | "EXPECTED" => Some("pending".into()),
        "SUCCESS" => Some("passed".into()),
        _ if failed > 0 => Some("failed".into()),
        _ if running > 0 => Some("pending".into()),
        _ if passed > 0 => Some("passed".into()),
        _ => None,
    }
}

fn github_response(
    row: github::ListPullRequestsByIssueRow,
    snapshot_enabled: bool,
) -> PullRequestResponse {
    let snapshot_available = snapshot_enabled
        && row.snapshot_fetched_at.is_some()
        && !row.snapshot_head_sha.is_empty()
        && row.snapshot_head_sha == row.head_sha;
    let snapshot_stale = snapshot_available
        && matches!(row.state.as_str(), "open" | "draft")
        && row
            .snapshot_fetched_at
            .is_some_and(|at| Utc::now().signed_duration_since(at).num_minutes() > 30);
    let mergeable = snapshot_available
        .then(|| {
            row.api_mergeable
                .as_deref()
                .filter(|value| !value.is_empty())
        })
        .flatten()
        .map(str::to_ascii_lowercase);
    let merge_state_status = snapshot_available
        .then(|| {
            row.api_merge_state_status
                .as_deref()
                .filter(|value| !value.is_empty())
        })
        .flatten()
        .map(str::to_ascii_lowercase);
    let checks_rollup = snapshot_available
        .then(|| {
            row.checks_rollup_state
                .as_deref()
                .filter(|value| !value.is_empty())
        })
        .flatten()
        .map(str::to_ascii_lowercase);
    let checks_conclusion = snapshot_available
        .then(|| {
            rollup_conclusion(
                row.checks_rollup_state.as_deref(),
                row.checks_failed,
                row.checks_running,
                row.checks_passed,
            )
        })
        .flatten();
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
        mergeable,
        merge_state_status,
        snapshot_available: Some(snapshot_available),
        checks_rollup,
        checks_conclusion,
        checks_total: if snapshot_available {
            row.checks_total
        } else {
            0
        },
        checks_passed: if snapshot_available {
            row.checks_passed
        } else {
            0
        },
        checks_failed: if snapshot_available {
            row.checks_failed
        } else {
            0
        },
        checks_running: if snapshot_available {
            row.checks_running
        } else {
            0
        },
        checks_pending: if snapshot_available {
            row.checks_running
        } else {
            0
        },
        failed_check_names: if snapshot_available {
            row.failed_check_names.unwrap_or_default()
        } else {
            Vec::new()
        },
        snapshot_stale,
        snapshot_fetched_at: snapshot_available
            .then(|| optional_timestamp(row.snapshot_fetched_at))
            .flatten(),
        additions: row.additions,
        deletions: row.deletions,
        changed_files: row.changed_files,
    }
}

pub(crate) fn github_model_response(
    row: GithubPullRequest,
    snapshot_enabled: bool,
) -> serde_json::Value {
    let snapshot_available = snapshot_enabled
        && row.snapshot_fetched_at.is_some()
        && !row.snapshot_head_sha.is_empty()
        && row.snapshot_head_sha == row.head_sha;
    serde_json::json!(PullRequestResponse {
        id: row.id.to_string(),
        provider: "github".into(),
        workspace_id: row.workspace_id.to_string(),
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
        pr_created_at: crate::timefmt::rfc3339(row.pr_created_at),
        pr_updated_at: crate::timefmt::rfc3339(row.pr_updated_at),
        mergeable_state: row.mergeable_state,
        mergeable: None,
        merge_state_status: None,
        snapshot_available: Some(snapshot_available),
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
    })
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
    let snapshot_enabled = state.github_snapshots.enabled();
    for row in &github_rows {
        if let Some(installation_id) = row.installation_id {
            let current_snapshot = (row.snapshot_fetched_at.is_some()
                && !row.snapshot_head_sha.is_empty()
                && row.snapshot_head_sha == row.head_sha)
                .then_some(row.snapshot_fetched_at)
                .flatten();
            state.github_snapshots.maybe_enqueue_on_view(
                installation_id,
                row.repo_owner.clone(),
                row.repo_name.clone(),
                row.pr_number,
                current_snapshot,
            );
        }
    }
    let mut pull_requests = github_rows
        .into_iter()
        .map(|row| github_response(row, snapshot_enabled))
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

    #[test]
    fn rollup_mapping_never_treats_absent_checks_as_passed() {
        assert_eq!(rollup_conclusion(None, 0, 0, 4), None);
        assert_eq!(
            rollup_conclusion(Some("SUCCESS"), 0, 0, 4).as_deref(),
            Some("passed")
        );
        assert_eq!(
            rollup_conclusion(Some("OTHER"), 1, 0, 4).as_deref(),
            Some("failed")
        );
    }
}
