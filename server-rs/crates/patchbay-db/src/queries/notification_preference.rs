//! Typed SQL queries for notification_preference records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn get_notification_preference(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> anyhow::Result<Option<NotificationPreference>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, user_id, preferences, updated_at FROM notification_preference
WHERE workspace_id = $1 AND user_id = $2"#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(NotificationPreference {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        user_id: row.try_get(2)?,
        preferences: row.try_get(3)?,
        updated_at: row.try_get(4)?,
    }))
}

pub async fn list_notification_preferences_by_users(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<NotificationPreference>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, user_id, preferences, updated_at FROM notification_preference
WHERE workspace_id = $1 AND user_id = ANY($2::uuid[])"#,
    )
    .bind(workspace_id)
    .bind(user_ids)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(NotificationPreference {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            user_id: row.try_get(2)?,
            preferences: row.try_get(3)?,
            updated_at: row.try_get(4)?,
        });
    }
    Ok(out)
}

pub async fn patch_notification_preference(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    preferences: &serde_json::Value,
) -> anyhow::Result<Option<NotificationPreference>> {
    let row = sqlx::query(
        r#"INSERT INTO notification_preference (workspace_id, user_id, preferences)
VALUES ($1, $2, $3)
ON CONFLICT (workspace_id, user_id)
DO UPDATE SET
    preferences = notification_preference.preferences || EXCLUDED.preferences,
    updated_at = now()
RETURNING id, workspace_id, user_id, preferences, updated_at"#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(preferences)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(NotificationPreference {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        user_id: row.try_get(2)?,
        preferences: row.try_get(3)?,
        updated_at: row.try_get(4)?,
    }))
}

pub async fn upsert_notification_preference(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    preferences: &serde_json::Value,
) -> anyhow::Result<Option<NotificationPreference>> {
    let row = sqlx::query(
        r#"INSERT INTO notification_preference (workspace_id, user_id, preferences)
VALUES ($1, $2, $3)
ON CONFLICT (workspace_id, user_id)
DO UPDATE SET preferences = $3, updated_at = now()
RETURNING id, workspace_id, user_id, preferences, updated_at"#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(preferences)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(NotificationPreference {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        user_id: row.try_get(2)?,
        preferences: row.try_get(3)?,
        updated_at: row.try_get(4)?,
    }))
}
