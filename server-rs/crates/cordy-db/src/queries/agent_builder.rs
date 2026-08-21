//! Port of server/pkg/db/queries/agent_builder.sql (generated agent_builder.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn delete_agent_builder_draft(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM agent_builder_draft WHERE chat_session_id = $1"#)
        .bind(chat_session_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn delete_agent_builder_drafts_by_system_runtime_agents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM agent_builder_draft
WHERE chat_session_id IN (
    SELECT cs.id FROM chat_session cs
    JOIN agent a ON a.id = cs.agent_id
    WHERE a.runtime_id = $1 AND a.kind = 'system'
)"#,
    )
    .bind(runtime_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn upsert_agent_builder_draft(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
    workspace_id: Uuid,
    draft: &serde_json::Value,
) -> anyhow::Result<Option<AgentBuilderDraft>> {
    let row = sqlx::query(
        r#"INSERT INTO agent_builder_draft (chat_session_id, workspace_id, draft)
VALUES ($1, $2, $3)
ON CONFLICT (chat_session_id) DO UPDATE
SET draft = EXCLUDED.draft,
    updated_at = now()
RETURNING chat_session_id, workspace_id, draft, updated_at"#,
    )
    .bind(chat_session_id)
    .bind(workspace_id)
    .bind(draft)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AgentBuilderDraft {
        chat_session_id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        draft: row.try_get(2)?,
        updated_at: row.try_get(3)?,
    }))
}
