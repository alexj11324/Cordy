//! Port of server/pkg/db/queries/issue_view_preference.sql (generated issue_view_preference.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn get_issue_view_preference(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    scope_type: &str,
    scope_id: Uuid,
) -> anyhow::Result<Option<IssueViewPreference>> {
    let row = sqlx::query(
        r#"SELECT workspace_id, user_id, scope_type, scope_id, prefs, updated_at FROM issue_view_preference
WHERE workspace_id = $1 AND user_id = $2 AND scope_type = $3 AND scope_id = $4"#
    )
        .bind(workspace_id)
        .bind(user_id)
        .bind(scope_type)
        .bind(scope_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueViewPreference {
        workspace_id: row.try_get(0)?,
        user_id: row.try_get(1)?,
        scope_type: row.try_get(2)?,
        scope_id: row.try_get(3)?,
        prefs: row.try_get(4)?,
        updated_at: row.try_get(5)?,
    }))
}

pub async fn upsert_issue_view_preference(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    scope_type: &str,
    scope_id: Uuid,
    prefs: &serde_json::Value,
) -> anyhow::Result<Option<IssueViewPreference>> {
    let row = sqlx::query(
        r#"INSERT INTO issue_view_preference (workspace_id, user_id, scope_type, scope_id, prefs)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (workspace_id, user_id, scope_type, scope_id)
DO UPDATE SET prefs = EXCLUDED.prefs, updated_at = now()
RETURNING workspace_id, user_id, scope_type, scope_id, prefs, updated_at"#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(scope_type)
    .bind(scope_id)
    .bind(prefs)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(IssueViewPreference {
        workspace_id: row.try_get(0)?,
        user_id: row.try_get(1)?,
        scope_type: row.try_get(2)?,
        scope_id: row.try_get(3)?,
        prefs: row.try_get(4)?,
        updated_at: row.try_get(5)?,
    }))
}
