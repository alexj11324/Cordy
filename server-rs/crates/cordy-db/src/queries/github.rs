//! Port of server/pkg/db/queries/github.sql (generated github.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn create_git_hub_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    installation_id: i64,
    account_login: &str,
    account_type: &str,
    account_avatar_url: Option<&str>,
    connected_by_id: Option<Uuid>,
) -> anyhow::Result<Option<GithubInstallation>> {
    let row = sqlx::query(
        r#"INSERT INTO github_installation (
    workspace_id, installation_id, account_login, account_type, account_avatar_url, connected_by_id
) VALUES (
    $1, $2, $3, $4, $5, $6
)
ON CONFLICT (workspace_id, installation_id) DO UPDATE SET
    account_login = EXCLUDED.account_login,
    account_type = EXCLUDED.account_type,
    account_avatar_url = EXCLUDED.account_avatar_url,
    connected_by_id = EXCLUDED.connected_by_id,
    updated_at = now()
RETURNING id, workspace_id, installation_id, account_login, account_type, account_avatar_url, connected_by_id, created_at, updated_at"#
    )
        .bind(workspace_id)
        .bind(installation_id)
        .bind(account_login)
        .bind(account_type)
        .bind(account_avatar_url)
        .bind(connected_by_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GithubInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        account_login: row.try_get(3)?,
        account_type: row.try_get(4)?,
        account_avatar_url: row.try_get(5)?,
        connected_by_id: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
    }))
}

pub async fn delete_git_hub_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM github_installation WHERE id = $1 AND workspace_id = $2"#)
        .bind(id)
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DeleteGitHubInstallationByInstallationIDRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
}

pub async fn delete_git_hub_installation_by_installation_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: i64,
) -> anyhow::Result<Vec<DeleteGitHubInstallationByInstallationIDRow>> {
    let rows = sqlx::query(
        r#"DELETE FROM github_installation WHERE installation_id = $1
RETURNING id, workspace_id"#,
    )
    .bind(installation_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(DeleteGitHubInstallationByInstallationIDRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
        });
    }
    Ok(out)
}

pub async fn delete_pending_git_hub_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: i64,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM github_pending_installation WHERE installation_id = $1"#)
        .bind(installation_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn get_git_hub_installation_by_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<GithubInstallation>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, installation_id, account_login, account_type, account_avatar_url, connected_by_id, created_at, updated_at FROM github_installation
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GithubInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        account_login: row.try_get(3)?,
        account_type: row.try_get(4)?,
        account_avatar_url: row.try_get(5)?,
        connected_by_id: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
    }))
}

pub async fn get_git_hub_pull_request(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    repo_owner: &str,
    repo_name: &str,
    pr_number: i32,
) -> anyhow::Result<Option<GithubPullRequest>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, installation_id, repo_owner, repo_name, pr_number, title, state, html_url, branch, author_login, author_avatar_url, merged_at, closed_at, pr_created_at, pr_updated_at, created_at, updated_at, head_sha, mergeable_state, additions, deletions, changed_files, api_mergeable, api_merge_state_status, checks_rollup_state, snapshot_head_sha, snapshot_fetched_at FROM github_pull_request
WHERE workspace_id = $1 AND repo_owner = $2 AND repo_name = $3 AND pr_number = $4"#
    )
        .bind(workspace_id)
        .bind(repo_owner)
        .bind(repo_name)
        .bind(pr_number)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GithubPullRequest {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        repo_owner: row.try_get(3)?,
        repo_name: row.try_get(4)?,
        pr_number: row.try_get(5)?,
        title: row.try_get(6)?,
        state: row.try_get(7)?,
        html_url: row.try_get(8)?,
        branch: row.try_get(9)?,
        author_login: row.try_get(10)?,
        author_avatar_url: row.try_get(11)?,
        merged_at: row.try_get(12)?,
        closed_at: row.try_get(13)?,
        pr_created_at: row.try_get(14)?,
        pr_updated_at: row.try_get(15)?,
        created_at: row.try_get(16)?,
        updated_at: row.try_get(17)?,
        head_sha: row.try_get(18)?,
        mergeable_state: row.try_get(19)?,
        additions: row.try_get(20)?,
        deletions: row.try_get(21)?,
        changed_files: row.try_get(22)?,
        api_mergeable: row.try_get(23)?,
        api_merge_state_status: row.try_get(24)?,
        checks_rollup_state: row.try_get(25)?,
        snapshot_head_sha: row.try_get(26)?,
        snapshot_fetched_at: row.try_get(27)?,
    }))
}

pub async fn attach_git_hub_pull_request(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    installation_id: Option<i64>,
    repo_owner: &str,
    repo_name: &str,
    pr_number: i32,
    title: &str,
    state: &str,
    html_url: &str,
    pr_created_at: Option<DateTime<Utc>>,
    pr_updated_at: Option<DateTime<Utc>>,
    head_sha: &str,
    additions: i32,
    deletions: i32,
    changed_files: i32,
    branch: Option<&str>,
    author_login: Option<&str>,
    author_avatar_url: Option<&str>,
    merged_at: Option<DateTime<Utc>>,
    closed_at: Option<DateTime<Utc>>,
    metadata_complete: bool,
) -> anyhow::Result<Option<GithubPullRequest>> {
    let row = sqlx::query(
        r#"INSERT INTO github_pull_request (
    workspace_id, installation_id, repo_owner, repo_name, pr_number,
    title, state, html_url, branch, author_login, author_avatar_url,
    merged_at, closed_at, pr_created_at, pr_updated_at,
    head_sha, mergeable_state, additions, deletions, changed_files
) VALUES (
    $1, $2, $3, $4, $5, $6, $7, $8, $15, $16, $17,
    $18, $19, $9, $10, $11, NULL, $12, $13, $14
)
ON CONFLICT (workspace_id, repo_owner, repo_name, pr_number) DO UPDATE SET
    installation_id = CASE WHEN $20::boolean THEN COALESCE(EXCLUDED.installation_id, github_pull_request.installation_id) ELSE github_pull_request.installation_id END,
    title = CASE WHEN $20::boolean THEN EXCLUDED.title ELSE github_pull_request.title END,
    state = CASE WHEN $20::boolean THEN EXCLUDED.state ELSE github_pull_request.state END,
    html_url = CASE WHEN $20::boolean THEN EXCLUDED.html_url ELSE github_pull_request.html_url END,
    branch = CASE WHEN $20::boolean THEN EXCLUDED.branch ELSE github_pull_request.branch END,
    author_login = CASE WHEN $20::boolean THEN EXCLUDED.author_login ELSE github_pull_request.author_login END,
    author_avatar_url = CASE WHEN $20::boolean THEN EXCLUDED.author_avatar_url ELSE github_pull_request.author_avatar_url END,
    merged_at = CASE WHEN $20::boolean THEN EXCLUDED.merged_at ELSE github_pull_request.merged_at END,
    closed_at = CASE WHEN $20::boolean THEN EXCLUDED.closed_at ELSE github_pull_request.closed_at END,
    pr_updated_at = CASE WHEN $20::boolean THEN EXCLUDED.pr_updated_at ELSE github_pull_request.pr_updated_at END,
    head_sha = CASE WHEN $20::boolean THEN EXCLUDED.head_sha ELSE github_pull_request.head_sha END,
    additions = CASE WHEN $20::boolean THEN EXCLUDED.additions ELSE github_pull_request.additions END,
    deletions = CASE WHEN $20::boolean THEN EXCLUDED.deletions ELSE github_pull_request.deletions END,
    changed_files = CASE WHEN $20::boolean THEN EXCLUDED.changed_files ELSE github_pull_request.changed_files END,
    updated_at = now()
RETURNING id, workspace_id, installation_id, repo_owner, repo_name, pr_number, title, state, html_url, branch, author_login, author_avatar_url, merged_at, closed_at, pr_created_at, pr_updated_at, created_at, updated_at, head_sha, mergeable_state, additions, deletions, changed_files, api_mergeable, api_merge_state_status, checks_rollup_state, snapshot_head_sha, snapshot_fetched_at"#,
    )
    .bind(workspace_id)
    .bind(installation_id)
    .bind(repo_owner)
    .bind(repo_name)
    .bind(pr_number)
    .bind(title)
    .bind(state)
    .bind(html_url)
    .bind(pr_created_at)
    .bind(pr_updated_at)
    .bind(head_sha)
    .bind(additions)
    .bind(deletions)
    .bind(changed_files)
    .bind(branch)
    .bind(author_login)
    .bind(author_avatar_url)
    .bind(merged_at)
    .bind(closed_at)
    .bind(metadata_complete)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GithubPullRequest {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        repo_owner: row.try_get(3)?,
        repo_name: row.try_get(4)?,
        pr_number: row.try_get(5)?,
        title: row.try_get(6)?,
        state: row.try_get(7)?,
        html_url: row.try_get(8)?,
        branch: row.try_get(9)?,
        author_login: row.try_get(10)?,
        author_avatar_url: row.try_get(11)?,
        merged_at: row.try_get(12)?,
        closed_at: row.try_get(13)?,
        pr_created_at: row.try_get(14)?,
        pr_updated_at: row.try_get(15)?,
        created_at: row.try_get(16)?,
        updated_at: row.try_get(17)?,
        head_sha: row.try_get(18)?,
        mergeable_state: row.try_get(19)?,
        additions: row.try_get(20)?,
        deletions: row.try_get(21)?,
        changed_files: row.try_get(22)?,
        api_mergeable: row.try_get(23)?,
        api_merge_state_status: row.try_get(24)?,
        checks_rollup_state: row.try_get(25)?,
        snapshot_head_sha: row.try_get(26)?,
        snapshot_fetched_at: row.try_get(27)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetIssuePullRequestCloseAggregateRow {
    pub open_count: i64,
    pub merged_with_close_intent_count: i64,
}

pub async fn get_issue_pull_request_close_aggregate(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Option<GetIssuePullRequestCloseAggregateRow>> {
    let row = sqlx::query(
        r#"SELECT
    COALESCE(SUM(CASE WHEN pr.state IN ('open', 'draft') THEN 1 ELSE 0 END), 0)::bigint AS open_count,
    COALESCE(SUM(CASE WHEN pr.state = 'merged' AND ipr.close_intent THEN 1 ELSE 0 END), 0)::bigint AS merged_with_close_intent_count
FROM github_pull_request pr
JOIN issue_pull_request ipr ON ipr.pull_request_id = pr.id
WHERE ipr.issue_id = $1 AND NOT ipr.reference_only"#
    )
        .bind(issue_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetIssuePullRequestCloseAggregateRow {
        open_count: row.try_get(0)?,
        merged_with_close_intent_count: row.try_get(1)?,
    }))
}

pub async fn get_issue_review_head_sha(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Option<String>> {
    let row = sqlx::query(
        r#"SELECT head_sha FROM (
    SELECT pr.head_sha AS head_sha, pr.state AS state, pr.pr_updated_at AS pr_updated_at
    FROM github_pull_request pr
    JOIN issue_pull_request ipr ON ipr.pull_request_id = pr.id
    WHERE ipr.issue_id = $1 AND pr.head_sha <> '' AND NOT ipr.reference_only
    UNION ALL
    SELECT pr.head_sha AS head_sha, pr.state AS state, pr.pr_updated_at AS pr_updated_at
    FROM vcs_pull_request pr
    JOIN issue_vcs_pull_request ipr ON ipr.pull_request_id = pr.id
    WHERE ipr.issue_id = $1 AND pr.head_sha <> '' AND NOT ipr.reference_only
) combined
ORDER BY (state IN ('open', 'draft')) DESC, pr_updated_at DESC
LIMIT 1"#,
    )
    .bind(issue_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn get_pending_git_hub_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: i64,
) -> anyhow::Result<Option<GithubPendingInstallation>> {
    let row = sqlx::query(
        r#"SELECT installation_id, account_login, account_type, account_avatar_url, received_at, updated_at FROM github_pending_installation WHERE installation_id = $1"#
    )
        .bind(installation_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GithubPendingInstallation {
        installation_id: row.try_get(0)?,
        account_login: row.try_get(1)?,
        account_type: row.try_get(2)?,
        account_avatar_url: row.try_get(3)?,
        received_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
    }))
}

pub async fn link_issue_to_pull_request(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    pull_request_id: Uuid,
    close_intent: bool,
    linked_by_type: Option<&str>,
    linked_by_id: Option<Uuid>,
    reference_only: bool,
    preserve_close_intent: bool,
    preserve_reference_only: bool,
    preserve_linked_by: bool,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO issue_pull_request (
    issue_id, pull_request_id, linked_by_type, linked_by_id, close_intent, reference_only
) VALUES (
    $1, $2, $4, $5, $3, $6
)
ON CONFLICT (issue_id, pull_request_id) DO UPDATE SET
    linked_by_type = CASE
        WHEN $9 THEN issue_pull_request.linked_by_type
        ELSE EXCLUDED.linked_by_type
    END,
    linked_by_id = CASE
        WHEN $9 THEN issue_pull_request.linked_by_id
        ELSE EXCLUDED.linked_by_id
    END,
    close_intent = CASE
        WHEN $7 THEN issue_pull_request.close_intent
        ELSE EXCLUDED.close_intent
    END,
    reference_only = CASE
        WHEN $8 THEN issue_pull_request.reference_only
        ELSE EXCLUDED.reference_only
    END"#,
    )
    .bind(issue_id)
    .bind(pull_request_id)
    .bind(close_intent)
    .bind(linked_by_type)
    .bind(linked_by_id)
    .bind(reference_only)
    .bind(preserve_close_intent)
    .bind(preserve_reference_only)
    .bind(preserve_linked_by)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn list_git_hub_installations_by_installation_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: i64,
) -> anyhow::Result<Vec<GithubInstallation>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, installation_id, account_login, account_type, account_avatar_url, connected_by_id, created_at, updated_at FROM github_installation
WHERE installation_id = $1
ORDER BY created_at ASC, id ASC"#
    )
        .bind(installation_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(GithubInstallation {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            installation_id: row.try_get(2)?,
            account_login: row.try_get(3)?,
            account_type: row.try_get(4)?,
            account_avatar_url: row.try_get(5)?,
            connected_by_id: row.try_get(6)?,
            created_at: row.try_get(7)?,
            updated_at: row.try_get(8)?,
        });
    }
    Ok(out)
}

pub async fn list_git_hub_installations_by_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<GithubInstallation>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, installation_id, account_login, account_type, account_avatar_url, connected_by_id, created_at, updated_at FROM github_installation
WHERE workspace_id = $1
ORDER BY created_at ASC"#
    )
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(GithubInstallation {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            installation_id: row.try_get(2)?,
            account_login: row.try_get(3)?,
            account_type: row.try_get(4)?,
            account_avatar_url: row.try_get(5)?,
            connected_by_id: row.try_get(6)?,
            created_at: row.try_get(7)?,
            updated_at: row.try_get(8)?,
        });
    }
    Ok(out)
}

pub async fn list_issue_i_ds_for_pull_request(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    pull_request_id: Uuid,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT issue_id FROM issue_pull_request
WHERE pull_request_id = $1"#,
    )
    .bind(pull_request_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListPullRequestsByIssueRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub installation_id: Option<i64>,
    pub repo_owner: String,
    pub repo_name: String,
    pub pr_number: i32,
    pub title: String,
    pub state: String,
    pub html_url: String,
    pub branch: Option<String>,
    pub author_login: Option<String>,
    pub author_avatar_url: Option<String>,
    pub merged_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub pr_created_at: Option<DateTime<Utc>>,
    pub pr_updated_at: Option<DateTime<Utc>>,
    pub head_sha: String,
    pub mergeable_state: Option<String>,
    pub additions: i32,
    pub deletions: i32,
    pub changed_files: i32,
    pub api_mergeable: Option<String>,
    pub api_merge_state_status: Option<String>,
    pub checks_rollup_state: Option<String>,
    pub snapshot_head_sha: String,
    pub snapshot_fetched_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub checks_total: i64,
    pub checks_passed: i64,
    pub checks_failed: i64,
    pub checks_running: i64,
    pub failed_check_names: Option<Vec<String>>,
}

pub async fn list_pull_requests_by_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Vec<ListPullRequestsByIssueRow>> {
    let rows = sqlx::query(
        r#"WITH issue_prs AS (
    SELECT pr.id, pr.snapshot_head_sha
    FROM github_pull_request pr
    JOIN issue_pull_request ipr ON ipr.pull_request_id = pr.id
    WHERE ipr.issue_id = $1 AND NOT ipr.reference_only
),
checks AS (
    SELECT
        cr.pr_id,
        COUNT(*)::bigint AS total,
        SUM(CASE WHEN cr.status = 'completed' AND cr.conclusion IN
                ('failure','cancelled','timed_out','action_required','startup_failure','stale','error')
            THEN 1 ELSE 0 END)::bigint AS failed,
        SUM(CASE WHEN cr.status = 'completed' AND cr.conclusion IN
                ('success','neutral','skipped')
            THEN 1 ELSE 0 END)::bigint AS passed,
        SUM(CASE WHEN cr.status <> 'completed' OR cr.conclusion IS NULL
            THEN 1 ELSE 0 END)::bigint AS running,
        COALESCE(
            array_agg(cr.name) FILTER (WHERE cr.status = 'completed' AND cr.conclusion IN
                ('failure','cancelled','timed_out','action_required','startup_failure','stale','error')),
            '{}'
        )::text[] AS failed_names
    FROM github_pull_request_check_run cr
    JOIN issue_prs ip ON ip.id = cr.pr_id
    WHERE cr.head_sha = ip.snapshot_head_sha AND ip.snapshot_head_sha <> ''
    GROUP BY cr.pr_id
)
SELECT
    pr.id, pr.workspace_id, pr.installation_id, pr.repo_owner, pr.repo_name,
    pr.pr_number, pr.title, pr.state, pr.html_url, pr.branch, pr.author_login,
    pr.author_avatar_url, pr.merged_at, pr.closed_at, pr.pr_created_at,
    pr.pr_updated_at, pr.head_sha, pr.mergeable_state,
    pr.additions, pr.deletions, pr.changed_files,
    pr.api_mergeable, pr.api_merge_state_status, pr.checks_rollup_state,
    pr.snapshot_head_sha, pr.snapshot_fetched_at,
    pr.created_at, pr.updated_at,
    COALESCE(c.total, 0)::bigint   AS checks_total,
    COALESCE(c.passed, 0)::bigint  AS checks_passed,
    COALESCE(c.failed, 0)::bigint  AS checks_failed,
    COALESCE(c.running, 0)::bigint AS checks_running,
    COALESCE(c.failed_names, '{}')::text[] AS failed_check_names
FROM github_pull_request pr
JOIN issue_pull_request ipr ON ipr.pull_request_id = pr.id
LEFT JOIN checks c ON c.pr_id = pr.id
WHERE ipr.issue_id = $1 AND NOT ipr.reference_only
ORDER BY pr.pr_created_at DESC"#
    )
        .bind(issue_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListPullRequestsByIssueRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            installation_id: row.try_get(2)?,
            repo_owner: row.try_get(3)?,
            repo_name: row.try_get(4)?,
            pr_number: row.try_get(5)?,
            title: row.try_get(6)?,
            state: row.try_get(7)?,
            html_url: row.try_get(8)?,
            branch: row.try_get(9)?,
            author_login: row.try_get(10)?,
            author_avatar_url: row.try_get(11)?,
            merged_at: row.try_get(12)?,
            closed_at: row.try_get(13)?,
            pr_created_at: row.try_get(14)?,
            pr_updated_at: row.try_get(15)?,
            head_sha: row.try_get(16)?,
            mergeable_state: row.try_get(17)?,
            additions: row.try_get(18)?,
            deletions: row.try_get(19)?,
            changed_files: row.try_get(20)?,
            api_mergeable: row.try_get(21)?,
            api_merge_state_status: row.try_get(22)?,
            checks_rollup_state: row.try_get(23)?,
            snapshot_head_sha: row.try_get(24)?,
            snapshot_fetched_at: row.try_get(25)?,
            created_at: row.try_get(26)?,
            updated_at: row.try_get(27)?,
            checks_total: row.try_get(28)?,
            checks_passed: row.try_get(29)?,
            checks_failed: row.try_get(30)?,
            checks_running: row.try_get(31)?,
            failed_check_names: row.try_get(32)?,
        });
    }
    Ok(out)
}

pub async fn unlink_issue_from_pull_request(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    pull_request_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM issue_pull_request
WHERE issue_id = $1 AND pull_request_id = $2"#,
    )
    .bind(issue_id)
    .bind(pull_request_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn update_git_hub_installation_account_by_installation_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: i64,
    account_login: &str,
    account_type: &str,
    account_avatar_url: Option<&str>,
) -> anyhow::Result<Vec<GithubInstallation>> {
    let rows = sqlx::query(
        r#"UPDATE github_installation
SET account_login = $2,
    account_type = $3,
    account_avatar_url = $4,
    updated_at = now()
WHERE installation_id = $1
RETURNING id, workspace_id, installation_id, account_login, account_type, account_avatar_url, connected_by_id, created_at, updated_at"#
    )
        .bind(installation_id)
        .bind(account_login)
        .bind(account_type)
        .bind(account_avatar_url)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(GithubInstallation {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            installation_id: row.try_get(2)?,
            account_login: row.try_get(3)?,
            account_type: row.try_get(4)?,
            account_avatar_url: row.try_get(5)?,
            connected_by_id: row.try_get(6)?,
            created_at: row.try_get(7)?,
            updated_at: row.try_get(8)?,
        });
    }
    Ok(out)
}

pub async fn upsert_git_hub_pull_request(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    installation_id: i64,
    repo_owner: &str,
    repo_name: &str,
    pr_number: i32,
    title: &str,
    state: &str,
    html_url: &str,
    pr_created_at: Option<DateTime<Utc>>,
    pr_updated_at: Option<DateTime<Utc>>,
    head_sha: &str,
    additions: i32,
    deletions: i32,
    changed_files: i32,
    branch: Option<&str>,
    author_login: Option<&str>,
    author_avatar_url: Option<&str>,
    merged_at: Option<DateTime<Utc>>,
    closed_at: Option<DateTime<Utc>>,
    mergeable_state: Option<&str>,
    clear_mergeable_state: Option<bool>,
) -> anyhow::Result<Option<GithubPullRequest>> {
    let row = sqlx::query(
        r#"INSERT INTO github_pull_request (
    workspace_id, installation_id, repo_owner, repo_name, pr_number,
    title, state, html_url, branch, author_login, author_avatar_url,
    merged_at, closed_at, pr_created_at, pr_updated_at,
    head_sha, mergeable_state,
    additions, deletions, changed_files
) VALUES (
    $1, $2, $3, $4, $5,
    $6, $7, $8, $15, $16, $17,
    $18, $19, $9, $10,
    $11, $20,
    $12, $13, $14
)
ON CONFLICT (workspace_id, repo_owner, repo_name, pr_number) DO UPDATE SET
    installation_id = EXCLUDED.installation_id,
    title = EXCLUDED.title,
    state = EXCLUDED.state,
    html_url = EXCLUDED.html_url,
    branch = EXCLUDED.branch,
    author_login = EXCLUDED.author_login,
    author_avatar_url = EXCLUDED.author_avatar_url,
    merged_at = EXCLUDED.merged_at,
    closed_at = EXCLUDED.closed_at,
    pr_updated_at = EXCLUDED.pr_updated_at,
    head_sha = EXCLUDED.head_sha,
    mergeable_state = CASE
        WHEN COALESCE($21::boolean, FALSE) THEN NULL
        WHEN EXCLUDED.mergeable_state IS NOT NULL THEN EXCLUDED.mergeable_state
        ELSE github_pull_request.mergeable_state
    END,
    additions     = EXCLUDED.additions,
    deletions     = EXCLUDED.deletions,
    changed_files = EXCLUDED.changed_files,
    updated_at = now()
RETURNING id, workspace_id, installation_id, repo_owner, repo_name, pr_number, title, state, html_url, branch, author_login, author_avatar_url, merged_at, closed_at, pr_created_at, pr_updated_at, created_at, updated_at, head_sha, mergeable_state, additions, deletions, changed_files, api_mergeable, api_merge_state_status, checks_rollup_state, snapshot_head_sha, snapshot_fetched_at"#
    )
        .bind(workspace_id)
        .bind(installation_id)
        .bind(repo_owner)
        .bind(repo_name)
        .bind(pr_number)
        .bind(title)
        .bind(state)
        .bind(html_url)
        .bind(pr_created_at)
        .bind(pr_updated_at)
        .bind(head_sha)
        .bind(additions)
        .bind(deletions)
        .bind(changed_files)
        .bind(branch)
        .bind(author_login)
        .bind(author_avatar_url)
        .bind(merged_at)
        .bind(closed_at)
        .bind(mergeable_state)
        .bind(clear_mergeable_state)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GithubPullRequest {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        repo_owner: row.try_get(3)?,
        repo_name: row.try_get(4)?,
        pr_number: row.try_get(5)?,
        title: row.try_get(6)?,
        state: row.try_get(7)?,
        html_url: row.try_get(8)?,
        branch: row.try_get(9)?,
        author_login: row.try_get(10)?,
        author_avatar_url: row.try_get(11)?,
        merged_at: row.try_get(12)?,
        closed_at: row.try_get(13)?,
        pr_created_at: row.try_get(14)?,
        pr_updated_at: row.try_get(15)?,
        created_at: row.try_get(16)?,
        updated_at: row.try_get(17)?,
        head_sha: row.try_get(18)?,
        mergeable_state: row.try_get(19)?,
        additions: row.try_get(20)?,
        deletions: row.try_get(21)?,
        changed_files: row.try_get(22)?,
        api_mergeable: row.try_get(23)?,
        api_merge_state_status: row.try_get(24)?,
        checks_rollup_state: row.try_get(25)?,
        snapshot_head_sha: row.try_get(26)?,
        snapshot_fetched_at: row.try_get(27)?,
    }))
}

pub async fn upsert_pending_git_hub_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: i64,
    account_login: &str,
    account_type: &str,
    account_avatar_url: Option<&str>,
) -> anyhow::Result<Option<GithubPendingInstallation>> {
    let row = sqlx::query(
        r#"INSERT INTO github_pending_installation (
    installation_id, account_login, account_type, account_avatar_url
) VALUES (
    $1, $2, $3, $4
)
ON CONFLICT (installation_id) DO UPDATE SET
    account_login = EXCLUDED.account_login,
    account_type = EXCLUDED.account_type,
    account_avatar_url = EXCLUDED.account_avatar_url,
    updated_at = now()
RETURNING installation_id, account_login, account_type, account_avatar_url, received_at, updated_at"#
    )
        .bind(installation_id)
        .bind(account_login)
        .bind(account_type)
        .bind(account_avatar_url)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GithubPendingInstallation {
        installation_id: row.try_get(0)?,
        account_login: row.try_get(1)?,
        account_type: row.try_get(2)?,
        account_avatar_url: row.try_get(3)?,
        received_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
    }))
}
