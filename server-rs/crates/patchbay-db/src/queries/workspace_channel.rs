//! Typed SQL queries for in-app workspace channels.
//!
//! Channel membership is intentionally workspace-scoped in v1: every workspace
//! member can read and post, while agents appear as authors when an
//! authenticated task-token client posts on their behalf. The references to
//! members, agents, parents, and quotes are validated by the handler because
//! this repository does not add foreign keys for application-owned relations.

#![allow(clippy::too_many_arguments)]

use crate::models::{WorkspaceChannel, WorkspaceChannelMessage, WorkspaceChannelQuotedMessage};
use sqlx::Row;
use uuid::Uuid;

pub async fn list_channels(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<WorkspaceChannel>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, name, slug, description, created_by,
    archived_at, created_at, updated_at
FROM workspace_channel
WHERE workspace_id = $1 AND archived_at IS NULL
ORDER BY created_at ASC, id ASC"#,
    )
    .bind(workspace_id)
    .fetch_all(executor)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(WorkspaceChannel {
                id: row.try_get(0)?,
                workspace_id: row.try_get(1)?,
                name: row.try_get(2)?,
                slug: row.try_get(3)?,
                description: row.try_get(4)?,
                created_by: row.try_get(5)?,
                archived_at: row.try_get(6)?,
                created_at: row.try_get(7)?,
                updated_at: row.try_get(8)?,
            })
        })
        .collect()
}

pub async fn get_channel(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<WorkspaceChannel>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, name, slug, description, created_by,
    archived_at, created_at, updated_at
FROM workspace_channel
WHERE id = $1 AND workspace_id = $2 AND archived_at IS NULL"#,
    )
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WorkspaceChannel {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        slug: row.try_get(3)?,
        description: row.try_get(4)?,
        created_by: row.try_get(5)?,
        archived_at: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
    }))
}

pub async fn create_channel(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    name: &str,
    slug: &str,
    description: &str,
    created_by: Uuid,
) -> anyhow::Result<Option<WorkspaceChannel>> {
    let row = sqlx::query(
        r#"INSERT INTO workspace_channel
    (id, workspace_id, name, slug, description, created_by)
VALUES ($1, $2, $3, $4, $5, $6)
RETURNING id, workspace_id, name, slug, description, created_by,
    archived_at, created_at, updated_at"#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(name)
    .bind(slug)
    .bind(description)
    .bind(created_by)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WorkspaceChannel {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        name: row.try_get(2)?,
        slug: row.try_get(3)?,
        description: row.try_get(4)?,
        created_by: row.try_get(5)?,
        archived_at: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
    }))
}

pub async fn get_message(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    channel_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<WorkspaceChannelMessage>> {
    let query = message_select(Some(
        "m.id = $1 AND m.channel_id = $2 AND m.workspace_id = $3",
    ));
    let row = sqlx::query(&query)
        .bind(id)
        .bind(channel_id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    row.map(message_from_row).transpose()
}

pub async fn list_messages(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    channel_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<WorkspaceChannelMessage>> {
    let query = message_select(Some(
        "m.channel_id = $1 AND m.workspace_id = $2",
    ));
    let rows = sqlx::query(&query)
        .bind(channel_id)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;

    let mut messages = rows
        .into_iter()
        .map(message_from_row)
        .collect::<anyhow::Result<Vec<_>>>()?;
    messages.reverse();
    Ok(messages)
}

pub async fn create_message(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    channel_id: Uuid,
    author_type: &str,
    author_id: Uuid,
    content: &str,
    parent_id: Option<Uuid>,
    quoted_message_id: Option<Uuid>,
) -> anyhow::Result<Option<Uuid>> {
    let row = sqlx::query(
        r#"INSERT INTO workspace_channel_message
    (id, workspace_id, channel_id, author_type, author_id, content,
     parent_id, quoted_message_id)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
RETURNING id"#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(channel_id)
    .bind(author_type)
    .bind(author_id)
    .bind(content)
    .bind(parent_id)
    .bind(quoted_message_id)
    .fetch_optional(executor)
    .await?;
    row.map(|value| value.try_get(0))
        .transpose()
        .map_err(Into::into)
}

fn message_select(predicate: Option<&str>) -> String {
    let predicate = predicate.unwrap_or("TRUE");
    format!(
        r#"SELECT
    m.id, m.workspace_id, m.channel_id, m.author_type, m.author_id,
    m.content, m.parent_id, m.quoted_message_id, m.created_at, m.updated_at,
    COALESCE(u.name, a.name, 'Unknown') AS author_name,
    CASE WHEN m.author_type = 'member' THEN u.avatar_url ELSE a.avatar_url END AS author_avatar_url,
    CASE WHEN m.author_type = 'agent' THEN a.status ELSE NULL END AS author_status,
    q.id AS quoted_id,
    q.author_type AS quoted_author_type,
    q.author_id AS quoted_author_id,
    q.content AS quoted_content,
    COALESCE(qu.name, qa.name, 'Unknown') AS quoted_author_name
FROM workspace_channel_message AS m
LEFT JOIN "user" AS u
    ON m.author_type = 'member' AND u.id = m.author_id
LEFT JOIN agent AS a
    ON m.author_type = 'agent' AND a.id = m.author_id
LEFT JOIN workspace_channel_message AS q
    ON q.id = m.quoted_message_id
   AND q.channel_id = m.channel_id
   AND q.workspace_id = m.workspace_id
LEFT JOIN "user" AS qu
    ON q.author_type = 'member' AND qu.id = q.author_id
LEFT JOIN agent AS qa
    ON q.author_type = 'agent' AND qa.id = q.author_id
WHERE {predicate}
ORDER BY m.created_at DESC, m.id DESC
LIMIT 200"#,
    )
}

fn message_from_row(row: sqlx::postgres::PgRow) -> anyhow::Result<WorkspaceChannelMessage> {
    let quoted_id: Option<Uuid> = row.try_get(13)?;
    Ok(WorkspaceChannelMessage {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        channel_id: row.try_get(2)?,
        author_type: row.try_get(3)?,
        author_id: row.try_get(4)?,
        content: row.try_get(5)?,
        parent_id: row.try_get(6)?,
        quoted_message_id: row.try_get(7)?,
        created_at: row.try_get(8)?,
        updated_at: row.try_get(9)?,
        author_name: row.try_get(10)?,
        author_avatar_url: row.try_get(11)?,
        author_status: row.try_get(12)?,
        quoted_message: if let Some(id) = quoted_id {
            Some(WorkspaceChannelQuotedMessage {
                id,
                author_type: row.try_get(14)?,
                author_id: row.try_get(15)?,
                content: row.try_get(16)?,
                author_name: row.try_get(17)?,
            })
        } else {
            None
        },
    })
}
