//! Typed SQL queries for reaction records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct AddReactionRow {
    pub id: Option<Uuid>,
    pub comment_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub actor_type: String,
    pub actor_id: Option<Uuid>,
    pub emoji: String,
    pub created_at: Option<DateTime<Utc>>,
    pub comment_revision: i64,
}

pub async fn add_reaction(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    comment_id: Uuid,
    workspace_id: Uuid,
    actor_type: &str,
    actor_id: Uuid,
    emoji: &str,
) -> anyhow::Result<Option<AddReactionRow>> {
    let row = sqlx::query(
        r#"WITH inserted AS (
    INSERT INTO comment_reaction (comment_id, workspace_id, actor_type, actor_id, emoji)
    VALUES ($1, $2, $3, $4, $5)
    ON CONFLICT (comment_id, actor_type, actor_id, emoji) DO NOTHING
    RETURNING id, comment_id, workspace_id, actor_type, actor_id, emoji, created_at
), bumped AS (
    UPDATE comment
    SET revision = revision + 1
    WHERE id IN (SELECT comment_id FROM inserted)
    RETURNING revision
)
SELECT reaction.id, reaction.comment_id, reaction.workspace_id, reaction.actor_type, reaction.actor_id, reaction.emoji, reaction.created_at, COALESCE((SELECT revision FROM bumped), 0)::bigint AS comment_revision
FROM inserted reaction
UNION ALL
SELECT reaction.id, reaction.comment_id, reaction.workspace_id, reaction.actor_type, reaction.actor_id, reaction.emoji, reaction.created_at, 0::bigint AS comment_revision
FROM comment_reaction reaction
WHERE reaction.comment_id = $1
  AND reaction.actor_type = $3
  AND reaction.actor_id = $4
  AND reaction.emoji = $5
  AND NOT EXISTS (SELECT 1 FROM inserted)
LIMIT 1"#
    )
        .bind(comment_id)
        .bind(workspace_id)
        .bind(actor_type)
        .bind(actor_id)
        .bind(emoji)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AddReactionRow {
        id: row.try_get(0)?,
        comment_id: row.try_get(1)?,
        workspace_id: row.try_get(2)?,
        actor_type: row.try_get(3)?,
        actor_id: row.try_get(4)?,
        emoji: row.try_get(5)?,
        created_at: row.try_get(6)?,
        comment_revision: row.try_get(7)?,
    }))
}

pub async fn list_reactions_by_comment_i_ds(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    dollar_1: Vec<Uuid>,
) -> anyhow::Result<Vec<CommentReaction>> {
    let rows = sqlx::query(
        r#"SELECT id, comment_id, workspace_id, actor_type, actor_id, emoji, created_at FROM comment_reaction
WHERE comment_id = ANY($1::uuid[])
ORDER BY created_at ASC"#
    )
        .bind(dollar_1)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(CommentReaction {
            id: row.try_get(0)?,
            comment_id: row.try_get(1)?,
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
pub struct RemoveReactionRow {
    pub changed: bool,
    pub comment_revision: i64,
}

pub async fn remove_reaction(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    comment_id: Uuid,
    actor_type: &str,
    actor_id: Uuid,
    emoji: &str,
) -> anyhow::Result<Option<RemoveReactionRow>> {
    let row = sqlx::query(
        r#"WITH deleted AS (
    DELETE FROM comment_reaction
    WHERE comment_id = $1 AND actor_type = $2 AND actor_id = $3 AND emoji = $4
    RETURNING comment_id
), bumped AS (
    UPDATE comment
    SET revision = revision + 1
    WHERE id IN (SELECT comment_id FROM deleted)
    RETURNING revision
)
SELECT EXISTS(SELECT 1 FROM deleted) AS changed,
       COALESCE((SELECT revision FROM bumped), 0)::bigint AS comment_revision"#,
    )
    .bind(comment_id)
    .bind(actor_type)
    .bind(actor_id)
    .bind(emoji)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(RemoveReactionRow {
        changed: row.try_get(0)?,
        comment_revision: row.try_get(1)?,
    }))
}
