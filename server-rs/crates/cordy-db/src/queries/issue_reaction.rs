//! Typed SQL queries for issue_reaction records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct AddIssueReactionRow {
    pub id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub actor_type: String,
    pub actor_id: Option<Uuid>,
    pub emoji: String,
    pub created_at: Option<DateTime<Utc>>,
    pub issue_revision: i64,
}

pub async fn add_issue_reaction(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    workspace_id: Uuid,
    actor_type: &str,
    actor_id: Uuid,
    emoji: &str,
) -> anyhow::Result<Option<AddIssueReactionRow>> {
    let row = sqlx::query(
        r#"WITH inserted AS (
    INSERT INTO issue_reaction (issue_id, workspace_id, actor_type, actor_id, emoji)
    VALUES ($1, $2, $3, $4, $5)
    ON CONFLICT (issue_id, actor_type, actor_id, emoji) DO NOTHING
    RETURNING id, issue_id, workspace_id, actor_type, actor_id, emoji, created_at
), bumped AS (
    UPDATE issue
    SET revision = revision + 1
    WHERE id IN (SELECT issue_id FROM inserted)
    RETURNING revision
)
SELECT reaction.id, reaction.issue_id, reaction.workspace_id, reaction.actor_type, reaction.actor_id, reaction.emoji, reaction.created_at, COALESCE((SELECT revision FROM bumped), 0)::bigint AS issue_revision
FROM inserted reaction
UNION ALL
SELECT reaction.id, reaction.issue_id, reaction.workspace_id, reaction.actor_type, reaction.actor_id, reaction.emoji, reaction.created_at, 0::bigint AS issue_revision
FROM issue_reaction reaction
WHERE reaction.issue_id = $1
  AND reaction.actor_type = $3
  AND reaction.actor_id = $4
  AND reaction.emoji = $5
  AND NOT EXISTS (SELECT 1 FROM inserted)
LIMIT 1"#
    )
        .bind(issue_id)
        .bind(workspace_id)
        .bind(actor_type)
        .bind(actor_id)
        .bind(emoji)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AddIssueReactionRow {
        id: row.try_get(0)?,
        issue_id: row.try_get(1)?,
        workspace_id: row.try_get(2)?,
        actor_type: row.try_get(3)?,
        actor_id: row.try_get(4)?,
        emoji: row.try_get(5)?,
        created_at: row.try_get(6)?,
        issue_revision: row.try_get(7)?,
    }))
}

pub async fn list_issue_reactions(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Vec<IssueReaction>> {
    let rows = sqlx::query(
        r#"SELECT id, issue_id, workspace_id, actor_type, actor_id, emoji, created_at FROM issue_reaction
WHERE issue_id = $1
ORDER BY created_at ASC"#
    )
        .bind(issue_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(IssueReaction {
            id: row.try_get(0)?,
            issue_id: row.try_get(1)?,
            workspace_id: row.try_get(2)?,
            actor_type: row.try_get(3)?,
            actor_id: row.try_get(4)?,
            emoji: row.try_get(5)?,
            created_at: row.try_get(6)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RemoveIssueReactionRow {
    pub changed: bool,
    pub issue_revision: i64,
}

pub async fn remove_issue_reaction(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    actor_type: &str,
    actor_id: Uuid,
    emoji: &str,
) -> anyhow::Result<Option<RemoveIssueReactionRow>> {
    let row = sqlx::query(
        r#"WITH deleted AS (
    DELETE FROM issue_reaction
    WHERE issue_id = $1 AND actor_type = $2 AND actor_id = $3 AND emoji = $4
    RETURNING issue_id
), bumped AS (
    UPDATE issue
    SET revision = revision + 1
    WHERE id IN (SELECT issue_id FROM deleted)
    RETURNING revision
)
SELECT EXISTS(SELECT 1 FROM deleted) AS changed,
       COALESCE((SELECT revision FROM bumped), 0)::bigint AS issue_revision"#,
    )
    .bind(issue_id)
    .bind(actor_type)
    .bind(actor_id)
    .bind(emoji)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(RemoveIssueReactionRow {
        changed: row.try_get(0)?,
        issue_revision: row.try_get(1)?,
    }))
}
