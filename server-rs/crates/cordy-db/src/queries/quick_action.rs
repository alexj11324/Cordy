//! Port of server/pkg/db/queries/quick_action.sql (generated quick_action.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn count_active_quick_actions(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT COUNT(*) FROM quick_action
WHERE workspace_id = $1 AND status = 'active'"#,
    )
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn create_quick_action(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    name: &str,
    description: &str,
    assignee_type: &str,
    assignee_id: Uuid,
    prompt: &str,
    visibility: &str,
    created_by_type: &str,
    created_by_id: Uuid,
) -> anyhow::Result<Option<QuickAction>> {
    let row = sqlx::query(
        r#"INSERT INTO quick_action (
    workspace_id, name, description, assignee_type, assignee_id, prompt,
    visibility, created_by_type, created_by_id
) VALUES (
    $1::uuid,
    $2::text,
    $3::text,
    $4::text,
    $5::uuid,
    $6::text,
    $7::text,
    $8::text,
    $9::uuid
)
RETURNING id, workspace_id, name, description, assignee_type, assignee_id, prompt, visibility, status, last_used_at, use_count, created_by_type, created_by_id, created_at, updated_at"#
    )
        .bind(workspace_id)
        .bind(name)
        .bind(description)
        .bind(assignee_type)
        .bind(assignee_id)
        .bind(prompt)
        .bind(visibility)
        .bind(created_by_type)
        .bind(created_by_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(QuickAction {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        assignee_type: row.try_get(4)?,
        assignee_id: row.try_get(5)?,
        prompt: row.try_get(6)?,
        visibility: row.try_get(7)?,
        status: row.try_get(8)?,
        last_used_at: row.try_get(9)?,
        use_count: row.try_get(10)?,
        created_by_type: row.try_get(11)?,
        created_by_id: row.try_get(12)?,
        created_at: row.try_get(13)?,
        updated_at: row.try_get(14)?,
    }))
}

pub async fn delete_private_quick_actions_by_creator(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    created_by_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM quick_action
WHERE workspace_id = $1 AND created_by_id = $2 AND visibility = 'private'"#,
    )
    .bind(workspace_id)
    .bind(created_by_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_quick_action(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM quick_action WHERE id = $1 AND workspace_id = $2"#)
        .bind(id)
        .bind(workspace_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn get_quick_action(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<QuickAction>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, description, assignee_type, assignee_id, prompt, visibility, status, last_used_at, use_count, created_by_type, created_by_id, created_at, updated_at FROM quick_action
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(QuickAction {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        assignee_type: row.try_get(4)?,
        assignee_id: row.try_get(5)?,
        prompt: row.try_get(6)?,
        visibility: row.try_get(7)?,
        status: row.try_get(8)?,
        last_used_at: row.try_get(9)?,
        use_count: row.try_get(10)?,
        created_by_type: row.try_get(11)?,
        created_by_id: row.try_get(12)?,
        created_at: row.try_get(13)?,
        updated_at: row.try_get(14)?,
    }))
}

pub async fn list_quick_actions(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    include_archived: bool,
    viewer_id: Uuid,
) -> anyhow::Result<Vec<QuickAction>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, name, description, assignee_type, assignee_id, prompt, visibility, status, last_used_at, use_count, created_by_type, created_by_id, created_at, updated_at FROM quick_action
WHERE workspace_id = $1::uuid
  AND ($2::bool OR status = 'active')
  AND (visibility = 'public' OR created_by_id = $3::uuid)
ORDER BY use_count DESC, LOWER(name) ASC"#
    )
        .bind(workspace_id)
        .bind(include_archived)
        .bind(viewer_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(QuickAction {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            name: row.try_get(2)?,
            description: row.try_get(3)?,
            assignee_type: row.try_get(4)?,
            assignee_id: row.try_get(5)?,
            prompt: row.try_get(6)?,
            visibility: row.try_get(7)?,
            status: row.try_get(8)?,
            last_used_at: row.try_get(9)?,
            use_count: row.try_get(10)?,
            created_by_type: row.try_get(11)?,
            created_by_id: row.try_get(12)?,
            created_at: row.try_get(13)?,
            updated_at: row.try_get(14)?,
        });
    }
    Ok(out)
}

pub async fn touch_quick_action_usage(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE quick_action
SET use_count = use_count + 1, last_used_at = now()
WHERE id = $1 AND workspace_id = $2"#,
    )
    .bind(id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn update_quick_action(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    assignee_type: Option<&str>,
    assignee_id: Uuid,
    prompt: Option<&str>,
    visibility: Option<&str>,
    status: Option<&str>,
) -> anyhow::Result<Option<QuickAction>> {
    let row = sqlx::query(
        r#"UPDATE quick_action SET
    name = COALESCE($3, name),
    description = COALESCE($4, description),
    assignee_type = COALESCE($5, assignee_type),
    assignee_id = COALESCE($6, assignee_id),
    prompt = COALESCE($7, prompt),
    visibility = COALESCE($8, visibility),
    status = COALESCE($9, status),
    updated_at = now()
WHERE id = $1 AND workspace_id = $2
RETURNING id, workspace_id, name, description, assignee_type, assignee_id, prompt, visibility, status, last_used_at, use_count, created_by_type, created_by_id, created_at, updated_at"#
    )
        .bind(id)
        .bind(workspace_id)
        .bind(name)
        .bind(description)
        .bind(assignee_type)
        .bind(assignee_id)
        .bind(prompt)
        .bind(visibility)
        .bind(status)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(QuickAction {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        description: row.try_get(3)?,
        assignee_type: row.try_get(4)?,
        assignee_id: row.try_get(5)?,
        prompt: row.try_get(6)?,
        visibility: row.try_get(7)?,
        status: row.try_get(8)?,
        last_used_at: row.try_get(9)?,
        use_count: row.try_get(10)?,
        created_by_type: row.try_get(11)?,
        created_by_id: row.try_get(12)?,
        created_at: row.try_get(13)?,
        updated_at: row.try_get(14)?,
    }))
}
