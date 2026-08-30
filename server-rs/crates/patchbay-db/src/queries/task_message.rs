//! Typed SQL queries for task_message records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn create_task_message(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    task_id: Uuid,
    seq: i32,
    type_: &str,
    tool: Option<&str>,
    content: Option<&str>,
    input: &serde_json::Value,
    output: Option<&str>,
) -> anyhow::Result<Option<TaskMessage>> {
    let row = sqlx::query(
        r#"INSERT INTO task_message (id, task_id, seq, type, tool, content, input, output)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
RETURNING id, task_id, seq, type, tool, content, input, output, created_at"#,
    )
    .bind(id)
    .bind(task_id)
    .bind(seq)
    .bind(type_)
    .bind(tool)
    .bind(content)
    .bind(input)
    .bind(output)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(TaskMessage {
        id: row.try_get(0)?,
        task_id: row.try_get(1)?,
        seq: row.try_get(2)?,
        type_: row.try_get(3)?,
        tool: row.try_get(4)?,
        content: row.try_get(5)?,
        input: row.try_get(6)?,
        output: row.try_get(7)?,
        created_at: row.try_get(8)?,
    }))
}

pub async fn delete_task_messages(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM task_message
WHERE task_id = $1"#,
    )
    .bind(task_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn list_task_messages(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
) -> anyhow::Result<Vec<TaskMessage>> {
    let rows = sqlx::query(
        r#"SELECT id, task_id, seq, type, tool, content, input, output, created_at FROM task_message
WHERE task_id = $1
ORDER BY seq ASC"#,
    )
    .bind(task_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(TaskMessage {
            id: row.try_get(0)?,
            task_id: row.try_get(1)?,
            seq: row.try_get(2)?,
            type_: row.try_get(3)?,
            tool: row.try_get(4)?,
            content: row.try_get(5)?,
            input: row.try_get(6)?,
            output: row.try_get(7)?,
            created_at: row.try_get(8)?,
        });
    }
    Ok(out)
}

/// Loads the structured events for a complete Agent thread chain in one
/// round-trip. `array_position` preserves the caller's chain order while
/// keeping each task's event sequence ordered, so the handler does not fall
/// back to one sequential query per child task.
pub async fn list_task_messages_for_tasks(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<TaskMessage>> {
    if task_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        r#"SELECT id, task_id, seq, type, tool, content, input, output, created_at FROM task_message
WHERE task_id = ANY($1::uuid[])
ORDER BY array_position($1::uuid[], task_id), seq ASC"#,
    )
    .bind(task_ids)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(TaskMessage {
            id: row.try_get(0)?,
            task_id: row.try_get(1)?,
            seq: row.try_get(2)?,
            type_: row.try_get(3)?,
            tool: row.try_get(4)?,
            content: row.try_get(5)?,
            input: row.try_get(6)?,
            output: row.try_get(7)?,
            created_at: row.try_get(8)?,
        });
    }
    Ok(out)
}

pub async fn list_task_messages_since(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
    seq: i32,
) -> anyhow::Result<Vec<TaskMessage>> {
    let rows = sqlx::query(
        r#"SELECT id, task_id, seq, type, tool, content, input, output, created_at FROM task_message
WHERE task_id = $1 AND seq > $2
ORDER BY seq ASC"#,
    )
    .bind(task_id)
    .bind(seq)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(TaskMessage {
            id: row.try_get(0)?,
            task_id: row.try_get(1)?,
            seq: row.try_get(2)?,
            type_: row.try_get(3)?,
            tool: row.try_get(4)?,
            content: row.try_get(5)?,
            input: row.try_get(6)?,
            output: row.try_get(7)?,
            created_at: row.try_get(8)?,
        });
    }
    Ok(out)
}
