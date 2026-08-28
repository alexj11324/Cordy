//! Typed SQL queries for subscriber records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn add_delegated_subscriber(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    user_id: Uuid,
    reason: &str,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH RECURSIVE ancestors(node_id, parent_id, depth) AS (
    SELECT root.id, root.parent_issue_id, 0 FROM issue root WHERE root.id = $1
    UNION ALL
    SELECT i.id, i.parent_issue_id, a.depth + 1
    FROM issue i JOIN ancestors a ON i.id = a.parent_id
),
active_member AS (
    SELECT m.user_id FROM member m
    WHERE m.user_id = $2 AND m.workspace_id = $4
    FOR SHARE
)
INSERT INTO issue_subscriber (issue_id, user_type, user_id, reason)
SELECT $1, 'member', am.user_id, $3
FROM active_member am
WHERE NOT EXISTS (
    SELECT 1
    FROM issue_subscriber s
    JOIN ancestors a ON a.node_id = s.issue_id
    WHERE s.user_type = 'member' AND s.user_id = $2
      AND s.unsubscribed_at IS NOT NULL
      AND (a.depth = 0 OR s.opt_out_scope = 'subtree')
)
ON CONFLICT (issue_id, user_type, user_id) DO UPDATE
SET reason = EXCLUDED.reason
WHERE issue_subscriber.unsubscribed_at IS NULL
  AND issue_subscriber.reason = 'delegated'
  AND EXCLUDED.reason <> 'delegated'"#,
    )
    .bind(issue_id)
    .bind(user_id)
    .bind(reason)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn add_issue_subscriber(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    user_type: &str,
    user_id: Uuid,
    reason: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO issue_subscriber (issue_id, user_type, user_id, reason)
VALUES ($1, $2, $3, $4)
ON CONFLICT (issue_id, user_type, user_id) DO UPDATE
SET reason = EXCLUDED.reason
WHERE issue_subscriber.unsubscribed_at IS NULL
  AND issue_subscriber.reason = 'delegated'
  AND EXCLUDED.reason <> 'delegated'"#,
    )
    .bind(issue_id)
    .bind(user_type)
    .bind(user_id)
    .bind(reason)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_subscriptions_by_member(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM issue_subscriber s
USING issue i
WHERE s.issue_id = i.id
  AND i.workspace_id = $1
  AND s.user_type = 'member'
  AND s.user_id = $2"#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn has_ancestor_opt_out(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    user_type: &str,
    user_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"WITH RECURSIVE ancestors(node_id, parent_id, depth) AS (
    SELECT root.id, root.parent_issue_id, 0 FROM issue root WHERE root.id = $1
    UNION ALL
    SELECT i.id, i.parent_issue_id, a.depth + 1 FROM issue i JOIN ancestors a ON i.id = a.parent_id
)
SELECT EXISTS(
    SELECT 1
    FROM issue_subscriber s
    JOIN ancestors a ON a.node_id = s.issue_id
    WHERE s.user_type = $2 AND s.user_id = $3
      AND s.unsubscribed_at IS NOT NULL
      AND (a.depth = 0 OR s.opt_out_scope = 'subtree')
) AS opted_out"#,
    )
    .bind(id)
    .bind(user_type)
    .bind(user_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn is_issue_subscriber(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    user_type: &str,
    user_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT EXISTS(
    SELECT 1 FROM issue_subscriber
    WHERE issue_id = $1 AND user_type = $2 AND user_id = $3
      AND unsubscribed_at IS NULL
) AS subscribed"#,
    )
    .bind(issue_id)
    .bind(user_type)
    .bind(user_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn list_issue_subscribers(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Vec<IssueSubscriber>> {
    let rows = sqlx::query(
        r#"SELECT issue_id, user_type, user_id, reason, created_at, unsubscribed_at, opt_out_scope FROM issue_subscriber
WHERE issue_id = $1 AND unsubscribed_at IS NULL
ORDER BY created_at"#
    )
        .bind(issue_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(IssueSubscriber {
            issue_id: row.try_get(0)?,
            user_type: row.try_get(1)?,
            user_id: row.try_get(2)?,
            reason: row.try_get(3)?,
            created_at: row.try_get(4)?,
            unsubscribed_at: row.try_get(5)?,
            opt_out_scope: row.try_get(6)?,
        });
    }
    Ok(out)
}

pub async fn lock_active_member(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"SELECT id FROM member
WHERE user_id = $1 AND workspace_id = $2
FOR SHARE"#,
    )
    .bind(user_id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn lock_subscriber_writes(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"SELECT pg_advisory_xact_lock(
    hashtext(($1::uuid)::text),
    hashtext(($2::uuid)::text)
)"#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn remove_issue_subscriber(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    user_type: &str,
    user_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE issue_subscriber
SET unsubscribed_at = now(), opt_out_scope = 'issue'
WHERE issue_id = $1 AND user_type = $2 AND user_id = $3 AND unsubscribed_at IS NULL"#,
    )
    .bind(issue_id)
    .bind(user_type)
    .bind(user_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn subscribe_to_issue_explicitly(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    user_type: &str,
    user_id: Uuid,
    reason: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO issue_subscriber (issue_id, user_type, user_id, reason)
VALUES ($1, $2, $3, $4)
ON CONFLICT (issue_id, user_type, user_id)
DO UPDATE SET unsubscribed_at = NULL, opt_out_scope = NULL, reason = EXCLUDED.reason"#,
    )
    .bind(issue_id)
    .bind(user_type)
    .bind(user_id)
    .bind(reason)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn unsubscribe_from_issue_subtree(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    user_type: &str,
    user_id: Uuid,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"WITH RECURSIVE subtree(node_id) AS (
    SELECT root.id FROM issue root WHERE root.id = $1
    UNION ALL
    SELECT i.id FROM issue i JOIN subtree s ON i.parent_issue_id = s.node_id
),
retire_descendants AS (
    UPDATE issue_subscriber sub
    SET unsubscribed_at = now(), opt_out_scope = 'subtree'
    WHERE sub.issue_id IN (SELECT node_id FROM subtree WHERE node_id <> $1)
      AND sub.user_type = $2 AND sub.user_id = $3
      AND sub.unsubscribed_at IS NULL
    RETURNING sub.issue_id
),
retire_root AS (
    INSERT INTO issue_subscriber (issue_id, user_type, user_id, reason, unsubscribed_at, opt_out_scope)
    VALUES ($1, $2, $3, 'manual', now(), 'subtree')
    ON CONFLICT (issue_id, user_type, user_id)
    DO UPDATE SET unsubscribed_at = now(), opt_out_scope = 'subtree'
    RETURNING issue_id
)
SELECT issue_id FROM retire_descendants
UNION ALL
SELECT issue_id FROM retire_root"#
    )
        .bind(id)
        .bind(user_type)
        .bind(user_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}
