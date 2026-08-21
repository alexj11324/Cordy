//! Port of server/pkg/db/queries/vcs.sql (generated vcs.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn delete_vcs_connection(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH target AS (
    SELECT vcs_connection.id FROM vcs_connection WHERE vcs_connection.id = $1 AND vcs_connection.workspace_id = $2
),
cleared_links AS (
    DELETE FROM issue_vcs_pull_request
    WHERE pull_request_id IN (
        SELECT vcs_pull_request.id FROM vcs_pull_request
        WHERE vcs_pull_request.connection_id IN (SELECT target.id FROM target)
    )
),
cleared_statuses AS (
    DELETE FROM vcs_commit_status WHERE connection_id IN (SELECT target.id FROM target)
),
cleared_prs AS (
    DELETE FROM vcs_pull_request WHERE connection_id IN (SELECT target.id FROM target)
)
DELETE FROM vcs_connection WHERE vcs_connection.id = $1 AND vcs_connection.workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetIssueCombinedPullRequestCloseAggregateRow {
    pub open_count: i64,
    pub merged_with_close_intent_count: i64,
}

pub async fn get_issue_combined_pull_request_close_aggregate(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Option<GetIssueCombinedPullRequestCloseAggregateRow>> {
    let row = sqlx::query(
        r#"WITH combined AS (
    SELECT pr.state AS state, ipr.close_intent AS close_intent
    FROM github_pull_request pr
    JOIN issue_pull_request ipr ON ipr.pull_request_id = pr.id
    WHERE ipr.issue_id = $1 AND NOT ipr.reference_only
    UNION ALL
    SELECT pr.state AS state, ipr.close_intent AS close_intent
    FROM vcs_pull_request pr
    JOIN issue_vcs_pull_request ipr ON ipr.pull_request_id = pr.id
    WHERE ipr.issue_id = $1 AND NOT ipr.reference_only
)
SELECT
    COALESCE(SUM(CASE WHEN state IN ('open', 'draft') THEN 1 ELSE 0 END), 0)::bigint AS open_count,
    COALESCE(SUM(CASE WHEN state = 'merged' AND close_intent THEN 1 ELSE 0 END), 0)::bigint AS merged_with_close_intent_count
FROM combined"#
    )
        .bind(issue_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetIssueCombinedPullRequestCloseAggregateRow {
        open_count: row.try_get(0)?,
        merged_with_close_intent_count: row.try_get(1)?,
    }))
}

pub async fn get_vcs_connection_by_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<VcsConnection>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, provider, instance_url, account_login, access_token_encrypted, webhook_secret_encrypted, connected_by_id, created_at, updated_at FROM vcs_connection
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(VcsConnection {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        provider: row.try_get(2)?,
        instance_url: row.try_get(3)?,
        account_login: row.try_get(4)?,
        access_token_encrypted: row.try_get(5)?,
        webhook_secret_encrypted: row.try_get(6)?,
        connected_by_id: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
    }))
}

pub async fn link_issue_to_vcs_pull_request(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    pull_request_id: Uuid,
    close_intent: bool,
    linked_by_type: Option<&str>,
    linked_by_id: Uuid,
    reference_only: bool,
    preserve_close_intent: bool,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO issue_vcs_pull_request (
    issue_id, pull_request_id, linked_by_type, linked_by_id, close_intent, reference_only
) VALUES (
    $1, $2, $4, $5, $3, $6
)
ON CONFLICT (issue_id, pull_request_id) DO UPDATE SET
    close_intent = CASE
        WHEN $7 THEN issue_vcs_pull_request.close_intent
        ELSE EXCLUDED.close_intent
    END,
    reference_only = CASE
        WHEN $7 THEN issue_vcs_pull_request.reference_only
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
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn list_issue_i_ds_for_vcspr_head(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    connection_id: Uuid,
    head_sha: &str,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT DISTINCT ipr.issue_id
FROM vcs_pull_request pr
JOIN issue_vcs_pull_request ipr ON ipr.pull_request_id = pr.id
WHERE pr.connection_id = $1 AND pr.head_sha = $2 AND pr.head_sha <> ''"#,
    )
    .bind(connection_id)
    .bind(head_sha)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_vcs_connections_by_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<VcsConnection>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, provider, instance_url, account_login, access_token_encrypted, webhook_secret_encrypted, connected_by_id, created_at, updated_at FROM vcs_connection
WHERE workspace_id = $1
ORDER BY created_at ASC"#
    )
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(VcsConnection {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            provider: row.try_get(2)?,
            instance_url: row.try_get(3)?,
            account_login: row.try_get(4)?,
            access_token_encrypted: row.try_get(5)?,
            webhook_secret_encrypted: row.try_get(6)?,
            connected_by_id: row.try_get(7)?,
            created_at: row.try_get(8)?,
            updated_at: row.try_get(9)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListVCSPullRequestsByIssueRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub connection_id: Option<Uuid>,
    pub provider: String,
    pub repo_owner: String,
    pub repo_name: String,
    pub pr_number: i32,
    pub title: String,
    pub state: String,
    pub html_url: String,
    pub branch: Option<String>,
    pub head_sha: String,
    pub author_login: Option<String>,
    pub author_avatar_url: Option<String>,
    pub merged_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub pr_created_at: Option<DateTime<Utc>>,
    pub pr_updated_at: Option<DateTime<Utc>>,
    pub additions: i32,
    pub deletions: i32,
    pub changed_files: i32,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub checks_total: i64,
    pub checks_passed: i64,
    pub checks_failed: i64,
    pub checks_pending: i64,
}

pub async fn list_vcs_pull_requests_by_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Vec<ListVCSPullRequestsByIssueRow>> {
    let rows = sqlx::query(
        r#"WITH checks AS (
    SELECT
        pr.id AS pr_id,
        COUNT(*)::bigint AS total,
        SUM(CASE WHEN cs.state = 'failed'  THEN 1 ELSE 0 END)::bigint AS failed,
        SUM(CASE WHEN cs.state = 'passed'  THEN 1 ELSE 0 END)::bigint AS passed,
        SUM(CASE WHEN cs.state = 'pending' THEN 1 ELSE 0 END)::bigint AS pending
    FROM vcs_pull_request pr
    JOIN issue_vcs_pull_request ipr ON ipr.pull_request_id = pr.id
    JOIN vcs_commit_status cs
        ON cs.connection_id = pr.connection_id
       AND cs.sha = pr.head_sha
       AND pr.head_sha <> ''
    WHERE ipr.issue_id = $1 AND NOT ipr.reference_only
    GROUP BY pr.id
)
SELECT
    pr.id, pr.workspace_id, pr.connection_id, pr.provider, pr.repo_owner, pr.repo_name, pr.pr_number, pr.title, pr.state, pr.html_url, pr.branch, pr.head_sha, pr.author_login, pr.author_avatar_url, pr.merged_at, pr.closed_at, pr.pr_created_at, pr.pr_updated_at, pr.additions, pr.deletions, pr.changed_files, pr.created_at, pr.updated_at,
    COALESCE(c.total, 0)::bigint   AS checks_total,
    COALESCE(c.passed, 0)::bigint  AS checks_passed,
    COALESCE(c.failed, 0)::bigint  AS checks_failed,
    COALESCE(c.pending, 0)::bigint AS checks_pending
FROM vcs_pull_request pr
JOIN issue_vcs_pull_request ipr ON ipr.pull_request_id = pr.id
LEFT JOIN checks c ON c.pr_id = pr.id
WHERE ipr.issue_id = $1 AND NOT ipr.reference_only
ORDER BY pr.pr_created_at DESC"#
    )
        .bind(issue_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListVCSPullRequestsByIssueRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            connection_id: row.try_get(2)?,
            provider: row.try_get(3)?,
            repo_owner: row.try_get(4)?,
            repo_name: row.try_get(5)?,
            pr_number: row.try_get(6)?,
            title: row.try_get(7)?,
            state: row.try_get(8)?,
            html_url: row.try_get(9)?,
            branch: row.try_get(10)?,
            head_sha: row.try_get(11)?,
            author_login: row.try_get(12)?,
            author_avatar_url: row.try_get(13)?,
            merged_at: row.try_get(14)?,
            closed_at: row.try_get(15)?,
            pr_created_at: row.try_get(16)?,
            pr_updated_at: row.try_get(17)?,
            additions: row.try_get(18)?,
            deletions: row.try_get(19)?,
            changed_files: row.try_get(20)?,
            created_at: row.try_get(21)?,
            updated_at: row.try_get(22)?,
            checks_total: row.try_get(23)?,
            checks_passed: row.try_get(24)?,
            checks_failed: row.try_get(25)?,
            checks_pending: row.try_get(26)?,
        });
    }
    Ok(out)
}

pub async fn rotate_vcs_connection_webhook_secret(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    webhook_secret_encrypted: &str,
) -> anyhow::Result<Option<VcsConnection>> {
    let row = sqlx::query(
        r#"UPDATE vcs_connection
SET webhook_secret_encrypted = $3,
    updated_at = now()
WHERE id = $1 AND workspace_id = $2
RETURNING id, workspace_id, provider, instance_url, account_login, access_token_encrypted, webhook_secret_encrypted, connected_by_id, created_at, updated_at"#
    )
        .bind(id)
        .bind(workspace_id)
        .bind(webhook_secret_encrypted)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(VcsConnection {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        provider: row.try_get(2)?,
        instance_url: row.try_get(3)?,
        account_login: row.try_get(4)?,
        access_token_encrypted: row.try_get(5)?,
        webhook_secret_encrypted: row.try_get(6)?,
        connected_by_id: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
    }))
}

pub async fn upsert_vcs_commit_status(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    connection_id: Uuid,
    sha: &str,
    context: &str,
    state: &str,
    updated_at: Option<DateTime<Utc>>,
    target_url: Option<&str>,
    description: Option<&str>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO vcs_commit_status (
    connection_id, sha, context, state, target_url, description, updated_at
) VALUES (
    $1, $2, $3, $4, $6, $7, $5
)
ON CONFLICT (connection_id, sha, context) DO UPDATE SET
    state       = EXCLUDED.state,
    target_url  = EXCLUDED.target_url,
    description = EXCLUDED.description,
    updated_at  = EXCLUDED.updated_at
WHERE EXCLUDED.updated_at >= vcs_commit_status.updated_at"#,
    )
    .bind(connection_id)
    .bind(sha)
    .bind(context)
    .bind(state)
    .bind(updated_at)
    .bind(target_url)
    .bind(description)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn upsert_vcs_connection(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    provider: &str,
    instance_url: &str,
    account_login: &str,
    access_token_encrypted: &str,
    webhook_secret_encrypted: &str,
    connected_by_id: Uuid,
) -> anyhow::Result<Option<VcsConnection>> {
    let row = sqlx::query(
        r#"INSERT INTO vcs_connection (
    workspace_id, provider, instance_url, account_login,
    access_token_encrypted, webhook_secret_encrypted, connected_by_id
) VALUES (
    $1, $2, $3, $4, $5, $6, $7
)
ON CONFLICT (workspace_id, instance_url) DO UPDATE SET
    provider                 = EXCLUDED.provider,
    account_login            = EXCLUDED.account_login,
    access_token_encrypted   = EXCLUDED.access_token_encrypted,
    webhook_secret_encrypted = EXCLUDED.webhook_secret_encrypted,
    connected_by_id          = EXCLUDED.connected_by_id,
    updated_at               = now()
RETURNING id, workspace_id, provider, instance_url, account_login, access_token_encrypted, webhook_secret_encrypted, connected_by_id, created_at, updated_at"#
    )
        .bind(workspace_id)
        .bind(provider)
        .bind(instance_url)
        .bind(account_login)
        .bind(access_token_encrypted)
        .bind(webhook_secret_encrypted)
        .bind(connected_by_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(VcsConnection {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        provider: row.try_get(2)?,
        instance_url: row.try_get(3)?,
        account_login: row.try_get(4)?,
        access_token_encrypted: row.try_get(5)?,
        webhook_secret_encrypted: row.try_get(6)?,
        connected_by_id: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
    }))
}

pub async fn upsert_vcs_pull_request(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
    provider: &str,
    repo_owner: &str,
    repo_name: &str,
    pr_number: i32,
    title: &str,
    state: &str,
    html_url: &str,
    pr_created_at: Option<DateTime<Utc>>,
    pr_updated_at: Option<DateTime<Utc>>,
    additions: i32,
    deletions: i32,
    changed_files: i32,
    head_sha: &str,
    branch: Option<&str>,
    author_login: Option<&str>,
    author_avatar_url: Option<&str>,
    merged_at: Option<DateTime<Utc>>,
    closed_at: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<VcsPullRequest>> {
    let row = sqlx::query(
        r#"INSERT INTO vcs_pull_request (
    workspace_id, connection_id, provider, repo_owner, repo_name, pr_number,
    title, state, html_url, branch, author_login, author_avatar_url,
    merged_at, closed_at, pr_created_at, pr_updated_at,
    additions, deletions, changed_files, head_sha
) VALUES (
    $1, $2, $3, $4, $5, $6,
    $7, $8, $9, $16, $17, $18,
    $19, $20, $10, $11,
    $12, $13, $14, $15
)
ON CONFLICT (connection_id, repo_owner, repo_name, pr_number) DO UPDATE SET
    workspace_id      = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.workspace_id      ELSE vcs_pull_request.workspace_id      END,
    provider          = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.provider          ELSE vcs_pull_request.provider          END,
    title             = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.title             ELSE vcs_pull_request.title             END,
    state             = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.state             ELSE vcs_pull_request.state             END,
    html_url          = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.html_url          ELSE vcs_pull_request.html_url          END,
    branch            = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.branch            ELSE vcs_pull_request.branch            END,
    author_login      = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.author_login      ELSE vcs_pull_request.author_login      END,
    author_avatar_url = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.author_avatar_url ELSE vcs_pull_request.author_avatar_url END,
    merged_at         = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.merged_at         ELSE vcs_pull_request.merged_at         END,
    closed_at         = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.closed_at         ELSE vcs_pull_request.closed_at         END,
    pr_updated_at     = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.pr_updated_at     ELSE vcs_pull_request.pr_updated_at     END,
    additions         = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.additions         ELSE vcs_pull_request.additions         END,
    deletions         = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.deletions         ELSE vcs_pull_request.deletions         END,
    changed_files     = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.changed_files     ELSE vcs_pull_request.changed_files     END,
    head_sha          = CASE WHEN EXCLUDED.pr_updated_at >= vcs_pull_request.pr_updated_at THEN EXCLUDED.head_sha          ELSE vcs_pull_request.head_sha          END,
    updated_at        = now()
RETURNING id, workspace_id, connection_id, provider, repo_owner, repo_name, pr_number, title, state, html_url, branch, head_sha, author_login, author_avatar_url, merged_at, closed_at, pr_created_at, pr_updated_at, additions, deletions, changed_files, created_at, updated_at"#
    )
        .bind(workspace_id)
        .bind(connection_id)
        .bind(provider)
        .bind(repo_owner)
        .bind(repo_name)
        .bind(pr_number)
        .bind(title)
        .bind(state)
        .bind(html_url)
        .bind(pr_created_at)
        .bind(pr_updated_at)
        .bind(additions)
        .bind(deletions)
        .bind(changed_files)
        .bind(head_sha)
        .bind(branch)
        .bind(author_login)
        .bind(author_avatar_url)
        .bind(merged_at)
        .bind(closed_at)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(VcsPullRequest {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        connection_id: row.try_get(2)?,
        provider: row.try_get(3)?,
        repo_owner: row.try_get(4)?,
        repo_name: row.try_get(5)?,
        pr_number: row.try_get(6)?,
        title: row.try_get(7)?,
        state: row.try_get(8)?,
        html_url: row.try_get(9)?,
        branch: row.try_get(10)?,
        head_sha: row.try_get(11)?,
        author_login: row.try_get(12)?,
        author_avatar_url: row.try_get(13)?,
        merged_at: row.try_get(14)?,
        closed_at: row.try_get(15)?,
        pr_created_at: row.try_get(16)?,
        pr_updated_at: row.try_get(17)?,
        additions: row.try_get(18)?,
        deletions: row.try_get(19)?,
        changed_files: row.try_get(20)?,
        created_at: row.try_get(21)?,
        updated_at: row.try_get(22)?,
    }))
}
