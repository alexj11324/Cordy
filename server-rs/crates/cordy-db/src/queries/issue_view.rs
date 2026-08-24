//! Port of server/pkg/db/queries/issue_view.sql (generated issue_view.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn count_issue_views_by_owner(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    owner_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT COUNT(*) FROM issue_view
WHERE workspace_id = $1 AND owner_id = $2"#,
    )
    .bind(workspace_id)
    .bind(owner_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn create_issue_view(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    owner_id: Uuid,
    name: &str,
    scope_type: &str,
    scope_id: Option<Uuid>,
    scope_variant: Option<&str>,
    visibility: &str,
    definition_version: i32,
    query: &serde_json::Value,
    display: &serde_json::Value,
) -> anyhow::Result<Option<IssueView>> {
    let row = sqlx::query(
        r#"INSERT INTO issue_view (
    workspace_id, owner_id, name, scope_type, scope_id, scope_variant,
    visibility, definition_version, query, display
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
RETURNING id, workspace_id, owner_id, name, scope_type, scope_id, scope_variant, visibility, definition_version, query, display, revision, created_at, updated_at"#
    )
        .bind(workspace_id)
        .bind(owner_id)
        .bind(name)
        .bind(scope_type)
        .bind(scope_id)
        .bind(scope_variant)
        .bind(visibility)
        .bind(definition_version)
        .bind(query)
        .bind(display)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueView {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        owner_id: row.try_get(2)?,
        name: row.try_get(3)?,
        scope_type: row.try_get(4)?,
        scope_id: row.try_get(5)?,
        scope_variant: row.try_get(6)?,
        visibility: row.try_get(7)?,
        definition_version: row.try_get(8)?,
        query: row.try_get(9)?,
        display: row.try_get(10)?,
        revision: row.try_get(11)?,
        created_at: row.try_get(12)?,
        updated_at: row.try_get(13)?,
    }))
}

pub async fn delete_issue_view(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"WITH deleted AS (
    DELETE FROM issue_view
    WHERE issue_view.id = $1 AND issue_view.workspace_id = $2
    RETURNING issue_view.id
),
swept_pins AS (
    DELETE FROM pinned_item
    WHERE pinned_item.item_type = 'view'
      AND pinned_item.workspace_id = $2
      AND pinned_item.item_id IN (SELECT deleted.id FROM deleted)
)
SELECT deleted.id FROM deleted"#,
    )
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn delete_issue_view_preferences_by_user(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM issue_view_preference
WHERE workspace_id = $1 AND user_id = $2"#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_issue_views_by_project_scope(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    scope_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH deleted AS (
    DELETE FROM issue_view
    WHERE issue_view.workspace_id = $1 AND issue_view.scope_type = 'project' AND issue_view.scope_id = $2
    RETURNING issue_view.id
)
DELETE FROM pinned_item
WHERE pinned_item.item_type = 'view'
  AND pinned_item.workspace_id = $1
  AND pinned_item.item_id IN (SELECT deleted.id FROM deleted)"#
    )
        .bind(workspace_id)
        .bind(scope_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_private_issue_views_by_owner(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    owner_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH deleted AS (
    DELETE FROM issue_view
    WHERE issue_view.workspace_id = $1 AND issue_view.owner_id = $2 AND issue_view.visibility = 'private'
    RETURNING issue_view.id
)
DELETE FROM pinned_item
WHERE pinned_item.item_type = 'view'
  AND pinned_item.workspace_id = $1
  AND pinned_item.item_id IN (SELECT deleted.id FROM deleted)"#
    )
        .bind(workspace_id)
        .bind(owner_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn get_issue_view(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<IssueView>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, owner_id, name, scope_type, scope_id, scope_variant, visibility, definition_version, query, display, revision, created_at, updated_at FROM issue_view
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueView {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        owner_id: row.try_get(2)?,
        name: row.try_get(3)?,
        scope_type: row.try_get(4)?,
        scope_id: row.try_get(5)?,
        scope_variant: row.try_get(6)?,
        visibility: row.try_get(7)?,
        definition_version: row.try_get(8)?,
        query: row.try_get(9)?,
        display: row.try_get(10)?,
        revision: row.try_get(11)?,
        created_at: row.try_get(12)?,
        updated_at: row.try_get(13)?,
    }))
}

pub async fn list_issue_views_for_user(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    scope_type: &str,
    owner_id: Uuid,
    scope_id: Option<Uuid>,
) -> anyhow::Result<Vec<IssueView>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, owner_id, name, scope_type, scope_id, scope_variant, visibility, definition_version, query, display, revision, created_at, updated_at FROM issue_view
WHERE workspace_id = $1
  AND scope_type = $2
  AND scope_id IS NOT DISTINCT FROM $4::uuid
  AND (owner_id = $3 OR visibility = 'workspace')
ORDER BY created_at ASC
LIMIT 200"#
    )
        .bind(workspace_id)
        .bind(scope_type)
        .bind(owner_id)
        .bind(scope_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(IssueView {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            owner_id: row.try_get(2)?,
            name: row.try_get(3)?,
            scope_type: row.try_get(4)?,
            scope_id: row.try_get(5)?,
            scope_variant: row.try_get(6)?,
            visibility: row.try_get(7)?,
            definition_version: row.try_get(8)?,
            query: row.try_get(9)?,
            display: row.try_get(10)?,
            revision: row.try_get(11)?,
            created_at: row.try_get(12)?,
            updated_at: row.try_get(13)?,
        });
    }
    Ok(out)
}

pub async fn update_issue_view(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    name: &str,
    visibility: &str,
    scope_variant: Option<&str>,
    query: &serde_json::Value,
    display: &serde_json::Value,
    revision: i32,
) -> anyhow::Result<Option<IssueView>> {
    let row = sqlx::query(
        r#"UPDATE issue_view SET
    name = $3,
    visibility = $4,
    scope_variant = $5,
    query = $6,
    display = $7,
    revision = revision + 1,
    updated_at = now()
WHERE id = $1 AND workspace_id = $2 AND revision = $8
RETURNING id, workspace_id, owner_id, name, scope_type, scope_id, scope_variant, visibility, definition_version, query, display, revision, created_at, updated_at"#
    )
        .bind(id)
        .bind(workspace_id)
        .bind(name)
        .bind(visibility)
        .bind(scope_variant)
        .bind(query)
        .bind(display)
        .bind(revision)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueView {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        owner_id: row.try_get(2)?,
        name: row.try_get(3)?,
        scope_type: row.try_get(4)?,
        scope_id: row.try_get(5)?,
        scope_variant: row.try_get(6)?,
        visibility: row.try_get(7)?,
        definition_version: row.try_get(8)?,
        query: row.try_get(9)?,
        display: row.try_get(10)?,
        revision: row.try_get(11)?,
        created_at: row.try_get(12)?,
        updated_at: row.try_get(13)?,
    }))
}
