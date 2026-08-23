//! Port of server/pkg/db/queries/github_snapshot.sql (generated github_snapshot.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn delete_git_hub_pr_check_runs(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    pr_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM github_pull_request_check_run WHERE pr_id = $1"#)
        .bind(pr_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn get_git_hub_pull_request_by_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<GithubPullRequest>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, installation_id, repo_owner, repo_name, pr_number, title, state, html_url, branch, author_login, author_avatar_url, merged_at, closed_at, pr_created_at, pr_updated_at, created_at, updated_at, head_sha, mergeable_state, additions, deletions, changed_files, api_mergeable, api_merge_state_status, checks_rollup_state, snapshot_head_sha, snapshot_fetched_at FROM github_pull_request WHERE id = $1"#
    )
        .bind(id)
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

pub async fn insert_git_hub_pr_check_run(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    pr_id: Uuid,
    head_sha: &str,
    ordinal: i32,
    name: &str,
    status: &str,
    is_status_context: bool,
    conclusion: Option<&str>,
    details_url: Option<&str>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO github_pull_request_check_run (
    pr_id, head_sha, ordinal, name, status, conclusion, details_url, is_status_context
) VALUES (
    $1, $2, $3, $4, $5, $7, $8, $6
)"#,
    )
    .bind(pr_id)
    .bind(head_sha)
    .bind(ordinal)
    .bind(name)
    .bind(status)
    .bind(is_status_context)
    .bind(conclusion)
    .bind(details_url)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn list_git_hub_pr_numbers_by_head_sha(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: i64,
    repo_owner: &str,
    repo_name: &str,
    head_sha: &str,
) -> anyhow::Result<Vec<i32>> {
    let rows = sqlx::query(
        r#"SELECT DISTINCT pr_number
FROM github_pull_request
WHERE installation_id = $1 AND repo_owner = $2 AND repo_name = $3 AND head_sha = $4"#,
    )
    .bind(installation_id)
    .bind(repo_owner)
    .bind(repo_name)
    .bind(head_sha)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListGitHubPRRowsByAddressRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub head_sha: String,
    pub state: String,
}

pub async fn list_git_hub_pr_rows_by_address(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: i64,
    repo_owner: &str,
    repo_name: &str,
    pr_number: i32,
) -> anyhow::Result<Vec<ListGitHubPRRowsByAddressRow>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, head_sha, state
FROM github_pull_request
WHERE installation_id = $1 AND repo_owner = $2 AND repo_name = $3 AND pr_number = $4"#,
    )
    .bind(installation_id)
    .bind(repo_owner)
    .bind(repo_name)
    .bind(pr_number)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListGitHubPRRowsByAddressRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            head_sha: row.try_get(2)?,
            state: row.try_get(3)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListStaleUndecidedGitHubPRsRow {
    pub installation_id: i64,
    pub repo_owner: String,
    pub repo_name: String,
    pub pr_number: i32,
}

pub async fn list_stale_undecided_git_hub_p_rs(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    after_installation_id: i64,
    after_repo_owner: &str,
    after_repo_name: &str,
    after_pr_number: i32,
    max_rows: i32,
    older_than: Option<DateTime<Utc>>,
) -> anyhow::Result<Vec<ListStaleUndecidedGitHubPRsRow>> {
    let rows = sqlx::query(
        r#"WITH candidates AS (
    SELECT installation_id, repo_owner, repo_name, pr_number
    FROM github_pull_request AS pr
    WHERE state IN ('open', 'draft')
      AND installation_id IS NOT NULL
      AND (snapshot_fetched_at IS NULL OR snapshot_fetched_at < $6)
      AND (
          snapshot_fetched_at IS NULL
          OR api_mergeable IS NULL
          OR api_mergeable = 'UNKNOWN'
          OR checks_rollup_state IN ('PENDING', 'EXPECTED')
          OR EXISTS (
              SELECT 1
              FROM github_pull_request_check_run AS cr
              WHERE cr.pr_id = pr.id AND cr.status <> 'completed'
          )
      )
    GROUP BY installation_id, repo_owner, repo_name, pr_number
)
SELECT installation_id, repo_owner, repo_name, pr_number
FROM candidates
ORDER BY (
    ROW(installation_id, repo_owner, repo_name, pr_number) >
    ROW(
        $1::BIGINT,
        $2::TEXT,
        $3::TEXT,
        $4::INTEGER
    )
) DESC,
installation_id, repo_owner, repo_name, pr_number
LIMIT $5"#,
    )
    .bind(after_installation_id)
    .bind(after_repo_owner)
    .bind(after_repo_name)
    .bind(after_pr_number)
    .bind(max_rows)
    .bind(older_than)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListStaleUndecidedGitHubPRsRow {
            installation_id: row.try_get(0)?,
            repo_owner: row.try_get(1)?,
            repo_name: row.try_get(2)?,
            pr_number: row.try_get(3)?,
        });
    }
    Ok(out)
}

pub async fn update_git_hub_pr_snapshot(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    api_mergeable: Option<&str>,
    api_merge_state_status: Option<&str>,
    checks_rollup_state: Option<&str>,
    head_sha: &str,
    fetched_at: Option<DateTime<Utc>>,
    pr_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE github_pull_request
SET api_mergeable          = $1,
    api_merge_state_status = $2,
    checks_rollup_state    = $3,
    snapshot_head_sha      = $4,
    snapshot_fetched_at    = $5,
    updated_at             = now()
WHERE id = $6 AND head_sha = $4"#,
    )
    .bind(api_mergeable)
    .bind(api_merge_state_status)
    .bind(checks_rollup_state)
    .bind(head_sha)
    .bind(fetched_at)
    .bind(pr_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}
