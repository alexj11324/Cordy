//! Pull-request cards attached to an issue.

use std::sync::LazyLock;

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use patchbay_db::models::{GithubPullRequest, VcsPullRequest};
use patchbay_db::queries::{github, vcs};
use patchbay_middleware::workspace::WorkspaceContext;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::error_response;
use crate::state::HandlerState;

static GITHUB_PR_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^https?://github\.com/([A-Za-z0-9][A-Za-z0-9-_.]*)/([A-Za-z0-9][A-Za-z0-9-_.]*)/pull/(\d+)(?:[/?#].*)?$",
    )
    .expect("GitHub pull request URL regex is valid")
});

#[derive(Debug, Deserialize)]
struct AttachRequest {
    url: String,
    title: Option<String>,
    state: Option<String>,
    branch: Option<String>,
    head_ref_name: Option<String>,
    #[allow(dead_code)]
    head_sha: Option<String>,
    author_login: Option<String>,
}

fn decode_attach_request(body: &[u8]) -> Result<AttachRequest, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    AttachRequest::deserialize(&mut deserializer)
}

fn parse_github_pr_url(raw: &str) -> Result<(String, String, i32), String> {
    let Some(captures) = GITHUB_PR_URL.captures(raw.trim()) else {
        return Err(format!("not a GitHub pull request URL: {raw:?}"));
    };
    let number = captures[3]
        .parse::<i32>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| format!("invalid pull request number in {raw:?}"))?;
    Ok((
        captures[1].to_ascii_lowercase(),
        captures[2].to_ascii_lowercase(),
        number,
    ))
}

fn normalize_attach_state(raw: Option<&str>) -> Result<String, String> {
    let Some(raw) = raw.filter(|value| !value.trim().is_empty()) else {
        return Ok("open".into());
    };
    let state = raw.trim().to_ascii_lowercase();
    match state.as_str() {
        "open" | "closed" | "merged" | "draft" => Ok(state),
        _ => Err(format!(
            "invalid state {raw:?}: expected open, closed, merged, or draft"
        )),
    }
}

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

pub(crate) fn vcs_model_response(row: VcsPullRequest) -> serde_json::Value {
    serde_json::json!(PullRequestResponse {
        id: row.id.to_string(),
        provider: row.provider,
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
        mergeable_state: None,
        mergeable: None,
        merge_state_status: None,
        snapshot_available: None,
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

pub(crate) async fn attach(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_issue): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let issue = match crate::issue::resolve_issue(&state, &context, &raw_issue).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let request = match decode_attach_request(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let (owner, repo, number) = match parse_github_pr_url(&request.url) {
        Ok(parts) => parts,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let mut pr_state = match normalize_attach_state(request.state.as_deref()) {
        Ok(state) => state,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };

    let mut metadata = None;
    if let (Some(client), Ok(installations)) = (
        state.github_snapshots.client(),
        github::list_git_hub_installations_by_workspace(&state.pool, issue.workspace_id).await,
    ) {
        for installation in installations {
            if let Ok(value) = client
                .pull_request_metadata(installation.installation_id, &owner, &repo, number)
                .await
            {
                metadata = Some((installation.installation_id, value));
                break;
            }
        }
    }

    let now = Utc::now();
    let mut installation_id = None;
    let mut title = format!("{owner}/{repo}#{number}");
    let mut branch = request.branch.or(request.head_ref_name);
    let mut head_sha = String::new();
    let mut author_login = request
        .author_login
        .filter(|value| !value.is_empty())
        .map(|value| value.trim().to_string());
    let mut author_avatar_url = None;
    let mut pr_created_at = now;
    let mut pr_updated_at = now;
    let mut merged_at = None;
    let mut closed_at = None;
    let mut additions = 0;
    let mut deletions = 0;
    let mut changed_files = 0;
    if let Some((served_by, metadata)) = metadata.as_ref() {
        installation_id = Some(*served_by);
        if !metadata.title.is_empty() {
            title.clone_from(&metadata.title);
        }
        pr_state.clone_from(&metadata.state);
        if !metadata.branch.is_empty() {
            branch = Some(metadata.branch.clone());
        }
        head_sha.clone_from(&metadata.head_sha);
        author_login = (!metadata.author_login.is_empty()).then(|| metadata.author_login.clone());
        author_avatar_url =
            (!metadata.author_avatar_url.is_empty()).then(|| metadata.author_avatar_url.clone());
        pr_created_at = metadata.created_at;
        pr_updated_at = metadata.updated_at;
        merged_at = metadata.merged_at;
        closed_at = metadata.closed_at;
        additions = metadata.additions;
        deletions = metadata.deletions;
        changed_files = metadata.changed_files;
    } else if let Some(request_title) = request.title.as_deref().map(str::trim) {
        if !request_title.is_empty() {
            title = request_title.to_string();
        }
    }
    branch = branch.filter(|value| !value.is_empty());

    let is_new = match github::get_git_hub_pull_request(
        &state.pool,
        issue.workspace_id,
        &owner,
        &repo,
        number,
    )
    .await
    {
        Ok(row) => row.is_none(),
        Err(error) => {
            tracing::warn!(%error, "github: attach lookup pr failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to attach pull request",
            );
        }
    };
    let canonical_url = format!("https://github.com/{owner}/{repo}/pull/{number}");
    let pull_request = match github::attach_git_hub_pull_request(
        &state.pool,
        issue.workspace_id,
        installation_id,
        &owner,
        &repo,
        number,
        &title,
        &pr_state,
        &canonical_url,
        Some(pr_created_at),
        Some(pr_updated_at),
        &head_sha,
        additions,
        deletions,
        changed_files,
        branch.as_deref(),
        author_login.as_deref(),
        author_avatar_url.as_deref(),
        merged_at,
        closed_at,
        metadata.is_some(),
    )
    .await
    {
        Ok(Some(pull_request)) => pull_request,
        Ok(None) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to attach pull request",
            )
        }
        Err(error) => {
            tracing::warn!(%error, "github: attach upsert pr failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to attach pull request",
            );
        }
    };

    let (actor_type, actor_id, task_id) =
        crate::issue::mutation_actor(&state, &context, &headers).await;
    let linked_by_type = if actor_type == "member" {
        "user"
    } else {
        actor_type.as_str()
    };
    if let Err(error) = github::link_issue_to_pull_request(
        &state.pool,
        issue.id,
        pull_request.id,
        false,
        Some(linked_by_type),
        Some(actor_id),
        false,
        true,
        false,
        false,
    )
    .await
    {
        tracing::warn!(%error, "github: attach link failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to link pull request",
        );
    }

    let response = github_model_response(pull_request, state.github_snapshots.enabled());
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::EVENT_PULL_REQUEST_UPDATED.into(),
        workspace_id: issue.workspace_id.to_string(),
        actor_type,
        actor_id: actor_id.to_string(),
        payload: serde_json::json!({
            "pull_request": response.clone(),
            "linked_issue_ids": [issue.id.to_string()],
        }),
        task_id: task_id.map(|id| id.to_string()).unwrap_or_default(),
        chat_session_id: String::new(),
    });
    let status = if is_new {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    (
        status,
        Json(serde_json::json!({ "pull_request": response })),
    )
        .into_response()
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
    fn github_pull_request_urls_match_go_contract() {
        assert_eq!(
            parse_github_pr_url(" https://github.com/Example-Org/Example-Repo/pull/24/files "),
            Ok(("example-org".into(), "example-repo".into(), 24))
        );
        assert_eq!(
            parse_github_pr_url("http://github.com/Owner/repo.name/pull/7?diff=split"),
            Ok(("owner".into(), "repo.name".into(), 7))
        );
        assert!(parse_github_pr_url("https://github.com/owner/repo/issues/24").is_err());
        assert!(parse_github_pr_url("https://example.com/owner/repo/pull/24").is_err());
        assert!(parse_github_pr_url("https://github.com/owner/repo/pull/0").is_err());
        assert!(parse_github_pr_url("https://github.com/owner/repo/pull/2147483648").is_err());
    }

    #[test]
    fn attach_state_defaults_normalizes_and_rejects_unknown_values() {
        assert_eq!(normalize_attach_state(None).as_deref(), Ok("open"));
        assert_eq!(normalize_attach_state(Some("  ")).as_deref(), Ok("open"));
        assert_eq!(
            normalize_attach_state(Some(" MeRgEd ")).as_deref(),
            Ok("merged")
        );
        assert!(normalize_attach_state(Some("ready")).is_err());
    }

    #[test]
    fn attach_decoder_matches_go_first_value_and_unknown_field_contract() {
        let request = decode_attach_request(
            br#"{"url":"https://github.com/o/r/pull/1","unknown":true} trailing"#,
        )
        .unwrap();
        assert_eq!(request.url, "https://github.com/o/r/pull/1");
        assert!(decode_attach_request(b"").is_err());
    }

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
