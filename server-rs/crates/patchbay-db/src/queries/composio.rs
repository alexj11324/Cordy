//! Typed SQL queries for composio records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn get_user_composio_connection(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    user_id: Uuid,
) -> anyhow::Result<Option<UserComposioConnection>> {
    let row = sqlx::query(
        r#"SELECT id, user_id, toolkit_slug, auth_config_id, connected_account_id, composio_user_id, status, connected_at, last_used_at, created_at, updated_at FROM user_composio_connection
WHERE id = $1 AND user_id = $2"#
    )
        .bind(id)
        .bind(user_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(UserComposioConnection {
        id: row.try_get(0)?,
        user_id: row.try_get(1)?,
        toolkit_slug: row.try_get(2)?,
        auth_config_id: row.try_get(3)?,
        connected_account_id: row.try_get(4)?,
        composio_user_id: row.try_get(5)?,
        status: row.try_get(6)?,
        connected_at: row.try_get(7)?,
        last_used_at: row.try_get(8)?,
        created_at: row.try_get(9)?,
        updated_at: row.try_get(10)?,
    }))
}

pub async fn list_active_user_composio_connections(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
) -> anyhow::Result<Vec<UserComposioConnection>> {
    let rows = sqlx::query(
        r#"SELECT id, user_id, toolkit_slug, auth_config_id, connected_account_id, composio_user_id, status, connected_at, last_used_at, created_at, updated_at FROM user_composio_connection
WHERE user_id = $1 AND status = 'active'
ORDER BY connected_at DESC"#
    )
        .bind(user_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(UserComposioConnection {
            id: row.try_get(0)?,
            user_id: row.try_get(1)?,
            toolkit_slug: row.try_get(2)?,
            auth_config_id: row.try_get(3)?,
            connected_account_id: row.try_get(4)?,
            composio_user_id: row.try_get(5)?,
            status: row.try_get(6)?,
            connected_at: row.try_get(7)?,
            last_used_at: row.try_get(8)?,
            created_at: row.try_get(9)?,
            updated_at: row.try_get(10)?,
        });
    }
    Ok(out)
}

pub async fn mark_user_composio_connection_revoked(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    user_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE user_composio_connection
SET status = 'revoked', updated_at = now()
WHERE id = $1 AND user_id = $2"#,
    )
    .bind(id)
    .bind(user_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn upsert_user_composio_connection(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
    toolkit_slug: &str,
    auth_config_id: &str,
    connected_account_id: &str,
    composio_user_id: &str,
) -> anyhow::Result<Option<UserComposioConnection>> {
    let row = sqlx::query(
        r#"INSERT INTO user_composio_connection (
    user_id, toolkit_slug, auth_config_id, connected_account_id, composio_user_id, status
) VALUES (
    $1, $2, $3, $4, $5, 'active'
)
ON CONFLICT (user_id, connected_account_id) DO UPDATE SET
    toolkit_slug     = EXCLUDED.toolkit_slug,
    auth_config_id   = EXCLUDED.auth_config_id,
    composio_user_id = EXCLUDED.composio_user_id,
    status           = 'active',
    updated_at       = now()
RETURNING id, user_id, toolkit_slug, auth_config_id, connected_account_id, composio_user_id, status, connected_at, last_used_at, created_at, updated_at"#
    )
        .bind(user_id)
        .bind(toolkit_slug)
        .bind(auth_config_id)
        .bind(connected_account_id)
        .bind(composio_user_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(UserComposioConnection {
        id: row.try_get(0)?,
        user_id: row.try_get(1)?,
        toolkit_slug: row.try_get(2)?,
        auth_config_id: row.try_get(3)?,
        connected_account_id: row.try_get(4)?,
        composio_user_id: row.try_get(5)?,
        status: row.try_get(6)?,
        connected_at: row.try_get(7)?,
        last_used_at: row.try_get(8)?,
        created_at: row.try_get(9)?,
        updated_at: row.try_get(10)?,
    }))
}
