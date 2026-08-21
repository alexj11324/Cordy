//! Port of server/pkg/db/queries/pinned_item.sql (generated pinned_item.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn create_pinned_item(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    item_type: &str,
    item_id: Uuid,
    position: f64,
) -> anyhow::Result<Option<PinnedItem>> {
    let row = sqlx::query(
        r#"INSERT INTO pinned_item (workspace_id, user_id, item_type, item_id, position)
VALUES ($1, $2, $3, $4, $5)
RETURNING id, workspace_id, user_id, item_type, item_id, position, created_at"#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(item_type)
    .bind(item_id)
    .bind(position)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PinnedItem {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        user_id: row.try_get(2)?,
        item_type: row.try_get(3)?,
        item_id: row.try_get(4)?,
        position: row.try_get(5)?,
        created_at: row.try_get(6)?,
    }))
}

pub async fn delete_pinned_item(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    item_type: &str,
    item_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM pinned_item
WHERE workspace_id = $1 AND user_id = $2 AND item_type = $3 AND item_id = $4"#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(item_type)
    .bind(item_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_pinned_items_by_item(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    item_type: &str,
    item_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM pinned_item
WHERE item_type = $1 AND item_id = $2"#,
    )
    .bind(item_type)
    .bind(item_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn get_max_pinned_item_position(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> anyhow::Result<Option<f64>> {
    let row = sqlx::query(
        r#"SELECT COALESCE(MAX(position), 0)::float8 AS max_position
FROM pinned_item
WHERE workspace_id = $1 AND user_id = $2"#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn list_pinned_items(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> anyhow::Result<Vec<PinnedItem>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, user_id, item_type, item_id, position, created_at FROM pinned_item
WHERE workspace_id = $1 AND user_id = $2
ORDER BY position ASC, created_at ASC"#
    )
        .bind(workspace_id)
        .bind(user_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(PinnedItem {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            user_id: row.try_get(2)?,
            item_type: row.try_get(3)?,
            item_id: row.try_get(4)?,
            position: row.try_get(5)?,
            created_at: row.try_get(6)?,
        });
    }
    Ok(out)
}

pub async fn update_pinned_item_position(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    position: f64,
    id: Uuid,
    workspace_id: Uuid,
    user_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE pinned_item SET position = $1
WHERE id = $2 AND workspace_id = $3 AND user_id = $4"#,
    )
    .bind(position)
    .bind(id)
    .bind(workspace_id)
    .bind(user_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}
