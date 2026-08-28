//! Typed SQL queries for daemon_token records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn create_daemon_token(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
    workspace_id: Uuid,
    daemon_id: &str,
    expires_at: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<DaemonToken>> {
    let row = sqlx::query(
        r#"INSERT INTO daemon_token (token_hash, workspace_id, daemon_id, expires_at)
VALUES ($1, $2, $3, $4)
RETURNING id, token_hash, workspace_id, daemon_id, expires_at, created_at"#,
    )
    .bind(token_hash)
    .bind(workspace_id)
    .bind(daemon_id)
    .bind(expires_at)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(DaemonToken {
        id: row.try_get(0)?,
        token_hash: row.try_get(1)?,
        workspace_id: row.try_get(2)?,
        daemon_id: row.try_get(3)?,
        expires_at: row.try_get(4)?,
        created_at: row.try_get(5)?,
    }))
}

pub async fn delete_daemon_tokens_by_workspace_and_daemons(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    daemon_ids: &[String],
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query(
        r#"DELETE FROM daemon_token
WHERE workspace_id = $1
  AND daemon_id = ANY($2::text[])
RETURNING token_hash"#,
    )
    .bind(workspace_id)
    .bind(daemon_ids)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn delete_expired_daemon_tokens(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM daemon_token
WHERE expires_at <= now()"#,
    )
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn get_daemon_token_by_hash(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
) -> anyhow::Result<Option<DaemonToken>> {
    let row = sqlx::query(
        r#"SELECT id, token_hash, workspace_id, daemon_id, expires_at, created_at FROM daemon_token
WHERE token_hash = $1 AND expires_at > now()"#,
    )
    .bind(token_hash)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(DaemonToken {
        id: row.try_get(0)?,
        token_hash: row.try_get(1)?,
        workspace_id: row.try_get(2)?,
        daemon_id: row.try_get(3)?,
        expires_at: row.try_get(4)?,
        created_at: row.try_get(5)?,
    }))
}
