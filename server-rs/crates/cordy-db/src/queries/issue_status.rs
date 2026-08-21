//! Port of server/pkg/db/queries/issue_status.sql (generated issue_status.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn archive_issue_status_entry(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<IssueStatus>> {
    let row = sqlx::query(
        r#"UPDATE issue_status SET
    archived_at = now(),
    updated_at = now()
WHERE id = $1::uuid
  AND workspace_id = $2::uuid
  AND is_system = FALSE
  AND archived_at IS NULL
RETURNING id, workspace_id, key, name, description, category, color, is_system, position, archived_at, created_at, updated_at"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueStatus {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        key: row.try_get(2)?,
        name: row.try_get(3)?,
        description: row.try_get(4)?,
        category: row.try_get(5)?,
        color: row.try_get(6)?,
        is_system: row.try_get(7)?,
        position: row.try_get(8)?,
        archived_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

pub async fn count_issues_using_status_key(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    key: &str,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT COUNT(*)::bigint FROM issue
WHERE workspace_id = $1::uuid
  AND status = $2::text"#,
    )
    .bind(workspace_id)
    .bind(key)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn create_issue_status_entry(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    key: &str,
    name: &str,
    description: &str,
    category: &str,
    color: &str,
) -> anyhow::Result<Option<IssueStatus>> {
    let row = sqlx::query(
        r#"INSERT INTO issue_status (workspace_id, key, name, description, category, color, position)
VALUES (
    $1::uuid,
    $2::text,
    $3::text,
    $4::text,
    $5::text,
    $6::text,
    COALESCE(
        (SELECT MAX(position) + 1 FROM issue_status
         WHERE workspace_id = $1::uuid
           AND category = $5::text),
        0
    )
)
RETURNING id, workspace_id, key, name, description, category, color, is_system, position, archived_at, created_at, updated_at"#
    )
        .bind(workspace_id)
        .bind(key)
        .bind(name)
        .bind(description)
        .bind(category)
        .bind(color)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueStatus {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        key: row.try_get(2)?,
        name: row.try_get(3)?,
        description: row.try_get(4)?,
        category: row.try_get(5)?,
        color: row.try_get(6)?,
        is_system: row.try_get(7)?,
        position: row.try_get(8)?,
        archived_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

pub async fn delete_issue_status_entries_for_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM issue_status WHERE workspace_id = $1::uuid"#)
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn get_issue_status_entry_by_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<IssueStatus>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, key, name, description, category, color, is_system, position, archived_at, created_at, updated_at FROM issue_status
WHERE id = $1::uuid
  AND workspace_id = $2::uuid"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueStatus {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        key: row.try_get(2)?,
        name: row.try_get(3)?,
        description: row.try_get(4)?,
        category: row.try_get(5)?,
        color: row.try_get(6)?,
        is_system: row.try_get(7)?,
        position: row.try_get(8)?,
        archived_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

pub async fn get_issue_status_entry_by_key(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    key: &str,
) -> anyhow::Result<Option<IssueStatus>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, key, name, description, category, color, is_system, position, archived_at, created_at, updated_at FROM issue_status
WHERE workspace_id = $1::uuid
  AND key = $2::text"#
    )
        .bind(workspace_id)
        .bind(key)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueStatus {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        key: row.try_get(2)?,
        name: row.try_get(3)?,
        description: row.try_get(4)?,
        category: row.try_get(5)?,
        color: row.try_get(6)?,
        is_system: row.try_get(7)?,
        position: row.try_get(8)?,
        archived_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

pub async fn list_active_custom_issue_status_entries(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    category: &str,
) -> anyhow::Result<Vec<IssueStatus>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, key, name, description, category, color, is_system, position, archived_at, created_at, updated_at FROM issue_status
WHERE workspace_id = $1::uuid
  AND category = $2::text
  AND is_system = FALSE
  AND archived_at IS NULL
ORDER BY position, key"#
    )
        .bind(workspace_id)
        .bind(category)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(IssueStatus {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            key: row.try_get(2)?,
            name: row.try_get(3)?,
            description: row.try_get(4)?,
            category: row.try_get(5)?,
            color: row.try_get(6)?,
            is_system: row.try_get(7)?,
            position: row.try_get(8)?,
            archived_at: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
        });
    }
    Ok(out)
}

pub async fn list_issue_status_entries(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    include_archived: bool,
) -> anyhow::Result<Vec<IssueStatus>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, key, name, description, category, color, is_system, position, archived_at, created_at, updated_at FROM issue_status
WHERE workspace_id = $1::uuid
  AND ($2::bool OR archived_at IS NULL)
ORDER BY
    CASE category
        WHEN 'backlog' THEN 0
        WHEN 'todo' THEN 1
        WHEN 'in_progress' THEN 2
        WHEN 'in_review' THEN 3
        WHEN 'done' THEN 4
        WHEN 'blocked' THEN 5
        WHEN 'cancelled' THEN 6
        ELSE 7
    END,
    position,
    key"#
    )
        .bind(workspace_id)
        .bind(include_archived)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(IssueStatus {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            key: row.try_get(2)?,
            name: row.try_get(3)?,
            description: row.try_get(4)?,
            category: row.try_get(5)?,
            color: row.try_get(6)?,
            is_system: row.try_get(7)?,
            position: row.try_get(8)?,
            archived_at: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
        });
    }
    Ok(out)
}

pub async fn list_issue_status_keys_by_categories(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    categories: &[String],
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query(
        r#"SELECT key FROM issue_status
WHERE workspace_id = $1::uuid
  AND category = ANY($2::text[])"#,
    )
    .bind(workspace_id)
    .bind(categories)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn lock_issue_status_catalog(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"SELECT pg_advisory_xact_lock(hashtextextended($1::uuid::text || ':issue_status', 0))"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn lock_issue_status_catalog_shared(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"SELECT pg_advisory_xact_lock_shared(hashtextextended($1::uuid::text || ':issue_status', 0))"#
    )
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn reorder_issue_status_entries(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    ids: Vec<Uuid>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE issue_status s
SET position = v.ordinality::int,
    updated_at = now()
FROM unnest($2::uuid[]) WITH ORDINALITY AS v(id, ordinality)
WHERE s.id = v.id
  AND s.workspace_id = $1::uuid
  AND s.is_system = FALSE
  AND s.archived_at IS NULL"#,
    )
    .bind(workspace_id)
    .bind(ids)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn seed_issue_status_entries(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO issue_status (workspace_id, key, name, description, category, color, is_system, position)
VALUES
    ($1::uuid, 'backlog', 'Backlog', 'Parked. Assigning an issue here never starts an agent run.', 'backlog', '#6b7280', TRUE, 0),
    ($1::uuid, 'todo', 'Todo', 'Queued for work. Moving an issue here starts the assigned agent.', 'todo', '#6b7280', TRUE, 0),
    ($1::uuid, 'in_progress', 'In Progress', 'Actively being worked on.', 'in_progress', '#f59e0b', TRUE, 0),
    ($1::uuid, 'in_review', 'In Review', 'Work delivered, waiting on human review. Finalizes the autopilot run.', 'in_review', '#22c55e', TRUE, 0),
    ($1::uuid, 'done', 'Done', 'Completed.', 'done', '#3b82f6', TRUE, 0),
    ($1::uuid, 'blocked', 'Blocked', 'Stalled on an external dependency.', 'blocked', '#ef4444', TRUE, 0),
    ($1::uuid, 'cancelled', 'Cancelled', 'Decided not to do.', 'cancelled', '#6b7280', TRUE, 0)
ON CONFLICT DO NOTHING"#
    )
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn update_issue_status_entry(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    name: Option<&str>,
    description: Option<&str>,
    color: Option<&str>,
    position: Option<f64>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<IssueStatus>> {
    let row = sqlx::query(
        r#"UPDATE issue_status SET
    name = COALESCE($1, name),
    description = COALESCE($2, description),
    color = COALESCE($3, color),
    position = COALESCE($4, position),
    updated_at = now()
WHERE id = $5::uuid
  AND workspace_id = $6::uuid
  AND is_system = FALSE
  AND archived_at IS NULL
RETURNING id, workspace_id, key, name, description, category, color, is_system, position, archived_at, created_at, updated_at"#
    )
        .bind(name)
        .bind(description)
        .bind(color)
        .bind(position)
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueStatus {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        key: row.try_get(2)?,
        name: row.try_get(3)?,
        description: row.try_get(4)?,
        category: row.try_get(5)?,
        color: row.try_get(6)?,
        is_system: row.try_get(7)?,
        position: row.try_get(8)?,
        archived_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}
