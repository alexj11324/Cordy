//! Port of server/pkg/db/queries/feedback.sql (generated feedback.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn count_recent_feedback_by_user(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT count(*) FROM feedback
WHERE user_id = $1 AND created_at > now() - interval '1 hour'"#,
    )
    .bind(user_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn create_feedback(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
    message: &str,
    metadata: &serde_json::Value,
    workspace_id: Option<Uuid>,
) -> anyhow::Result<Option<Feedback>> {
    let row = sqlx::query(
        r#"INSERT INTO feedback (user_id, workspace_id, message, metadata)
VALUES ($1, $4, $2, $3)
RETURNING id, user_id, workspace_id, message, metadata, created_at"#,
    )
    .bind(user_id)
    .bind(message)
    .bind(metadata)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Feedback {
        id: row.try_get(0)?,
        user_id: row.try_get(1)?,
        workspace_id: row.try_get(2)?,
        message: row.try_get(3)?,
        metadata: row.try_get(4)?,
        created_at: row.try_get(5)?,
    }))
}
