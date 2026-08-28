//! Typed SQL queries for task_token records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn create_task_token(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
    task_id: Uuid,
    agent_id: Uuid,
    workspace_id: Uuid,
    user_id: Uuid,
    expires_at: Option<DateTime<Utc>>,
    id: Uuid,
) -> anyhow::Result<Option<TaskToken>> {
    let row = sqlx::query(
        r#"INSERT INTO task_token (token_hash, task_id, agent_id, workspace_id, user_id, expires_at, id)
VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7::uuid, gen_random_uuid()))
RETURNING id, token_hash, task_id, agent_id, workspace_id, user_id, expires_at, created_at"#
    )
        .bind(token_hash)
        .bind(task_id)
        .bind(agent_id)
        .bind(workspace_id)
        .bind(user_id)
        .bind(expires_at)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(TaskToken {
        id: row.try_get(0)?,
        token_hash: row.try_get(1)?,
        task_id: row.try_get(2)?,
        agent_id: row.try_get(3)?,
        workspace_id: row.try_get(4)?,
        user_id: row.try_get(5)?,
        expires_at: row.try_get(6)?,
        created_at: row.try_get(7)?,
    }))
}

pub async fn delete_expired_task_tokens(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM task_token WHERE expires_at <= now()"#)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_task_tokens_by_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM task_token WHERE task_id = $1"#)
        .bind(task_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn get_task_token_by_hash(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
) -> anyhow::Result<Option<TaskToken>> {
    let row = sqlx::query(
        r#"SELECT id, token_hash, task_id, agent_id, workspace_id, user_id, expires_at, created_at FROM task_token
WHERE token_hash = $1 AND expires_at > now()"#
    )
        .bind(token_hash)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(TaskToken {
        id: row.try_get(0)?,
        token_hash: row.try_get(1)?,
        task_id: row.try_get(2)?,
        agent_id: row.try_get(3)?,
        workspace_id: row.try_get(4)?,
        user_id: row.try_get(5)?,
        expires_at: row.try_get(6)?,
        created_at: row.try_get(7)?,
    }))
}
