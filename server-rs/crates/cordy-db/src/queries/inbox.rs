//! Port of server/pkg/db/queries/inbox.sql (generated inbox.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn archive_all_inbox(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    recipient_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE inbox_item SET archived = true
WHERE workspace_id = $1 AND recipient_type = 'member' AND recipient_id = $2 AND archived = false"#,
    )
    .bind(workspace_id)
    .bind(recipient_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn archive_all_read_inbox(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    recipient_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE inbox_item SET archived = true
WHERE workspace_id = $1 AND recipient_type = 'member' AND recipient_id = $2 AND read = true AND archived = false"#
    )
        .bind(workspace_id)
        .bind(recipient_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn archive_completed_inbox(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    recipient_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE inbox_item i SET archived = true
WHERE i.workspace_id = $1 AND i.recipient_type = 'member' AND i.recipient_id = $2 AND i.archived = false
  AND i.issue_id IN (
    SELECT id FROM issue
    WHERE workspace_id = $1
      AND issue_effective_status(workspace_id, status) IN ('done', 'cancelled')
  )"#
    )
        .bind(workspace_id)
        .bind(recipient_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn archive_inbox_by_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    recipient_type: &str,
    recipient_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE inbox_item SET archived = true
WHERE workspace_id = $1 AND recipient_type = $2 AND recipient_id = $3 AND issue_id = $4 AND archived = false"#
    )
        .bind(workspace_id)
        .bind(recipient_type)
        .bind(recipient_id)
        .bind(issue_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ArchiveInboxByIssueAndTypeRow {
    pub recipient_type: String,
    pub recipient_id: Option<Uuid>,
}

pub async fn archive_inbox_by_issue_and_type(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    issue_id: Uuid,
    type_: &str,
) -> anyhow::Result<Vec<ArchiveInboxByIssueAndTypeRow>> {
    let rows = sqlx::query(
        r#"UPDATE inbox_item SET archived = true
WHERE workspace_id = $1 AND issue_id = $2 AND type = $3 AND archived = false
RETURNING recipient_type, recipient_id"#,
    )
    .bind(workspace_id)
    .bind(issue_id)
    .bind(type_)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ArchiveInboxByIssueAndTypeRow {
            recipient_type: row.try_get(0)?,
            recipient_id: row.try_get(1)?,
        });
    }
    Ok(out)
}

pub async fn archive_inbox_item(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<InboxItem>> {
    let row = sqlx::query(
        r#"UPDATE inbox_item SET archived = true
WHERE id = $1
RETURNING id, workspace_id, recipient_type, recipient_id, type, severity, issue_id, title, body, read, archived, created_at, actor_type, actor_id, details"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(InboxItem {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        recipient_type: row.try_get(2)?,
        recipient_id: row.try_get(3)?,
        type_: row.try_get(4)?,
        severity: row.try_get(5)?,
        issue_id: row.try_get(6)?,
        title: row.try_get(7)?,
        body: row.try_get(8)?,
        read: row.try_get(9)?,
        archived: row.try_get(10)?,
        created_at: row.try_get(11)?,
        actor_type: row.try_get(12)?,
        actor_id: row.try_get(13)?,
        details: row.try_get(14)?,
    }))
}

pub async fn count_unread_inbox(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    recipient_type: &str,
    recipient_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT count(*) FROM inbox_item
WHERE workspace_id = $1 AND recipient_type = $2 AND recipient_id = $3 AND read = false AND archived = false"#
    )
        .bind(workspace_id)
        .bind(recipient_type)
        .bind(recipient_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CountUnreadInboxByWorkspaceRow {
    pub workspace_id: Option<Uuid>,
    pub count: i64,
}

pub async fn count_unread_inbox_by_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    recipient_id: Uuid,
) -> anyhow::Result<Vec<CountUnreadInboxByWorkspaceRow>> {
    let rows = sqlx::query(
        r#"SELECT newest.workspace_id, count(*) AS count
FROM (
    SELECT DISTINCT ON (i.workspace_id, COALESCE(i.issue_id, i.id))
        i.workspace_id, i.read
    FROM inbox_item i
    JOIN member m ON m.workspace_id = i.workspace_id AND m.user_id = i.recipient_id
    WHERE i.recipient_type = 'member'
      AND i.recipient_id = $1
      AND i.archived = false
    ORDER BY i.workspace_id, COALESCE(i.issue_id, i.id), i.created_at DESC
) newest
WHERE newest.read = false
GROUP BY newest.workspace_id"#,
    )
    .bind(recipient_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(CountUnreadInboxByWorkspaceRow {
            workspace_id: row.try_get(0)?,
            count: row.try_get(1)?,
        });
    }
    Ok(out)
}

pub async fn create_inbox_item(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    recipient_type: &str,
    recipient_id: Uuid,
    type_: &str,
    severity: &str,
    issue_id: Option<Uuid>,
    title: &str,
    body: Option<&str>,
    actor_type: Option<&str>,
    actor_id: impl Into<Option<Uuid>>,
    details: &serde_json::Value,
    id: Uuid,
) -> anyhow::Result<Option<InboxItem>> {
    let row = sqlx::query(
        r#"INSERT INTO inbox_item (
    workspace_id, recipient_type, recipient_id,
    type, severity, issue_id, title, body,
    actor_type, actor_id, details, id
) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, COALESCE($12::uuid, gen_random_uuid()))
RETURNING id, workspace_id, recipient_type, recipient_id, type, severity, issue_id, title, body, read, archived, created_at, actor_type, actor_id, details"#
    )
        .bind(workspace_id)
        .bind(recipient_type)
        .bind(recipient_id)
        .bind(type_)
        .bind(severity)
        .bind(issue_id)
        .bind(title)
        .bind(body)
        .bind(actor_type)
        .bind(actor_id.into())
        .bind(details)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(InboxItem {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        recipient_type: row.try_get(2)?,
        recipient_id: row.try_get(3)?,
        type_: row.try_get(4)?,
        severity: row.try_get(5)?,
        issue_id: row.try_get(6)?,
        title: row.try_get(7)?,
        body: row.try_get(8)?,
        read: row.try_get(9)?,
        archived: row.try_get(10)?,
        created_at: row.try_get(11)?,
        actor_type: row.try_get(12)?,
        actor_id: row.try_get(13)?,
        details: row.try_get(14)?,
    }))
}

pub async fn get_inbox_item(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<InboxItem>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, recipient_type, recipient_id, type, severity, issue_id, title, body, read, archived, created_at, actor_type, actor_id, details FROM inbox_item
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(InboxItem {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        recipient_type: row.try_get(2)?,
        recipient_id: row.try_get(3)?,
        type_: row.try_get(4)?,
        severity: row.try_get(5)?,
        issue_id: row.try_get(6)?,
        title: row.try_get(7)?,
        body: row.try_get(8)?,
        read: row.try_get(9)?,
        archived: row.try_get(10)?,
        created_at: row.try_get(11)?,
        actor_type: row.try_get(12)?,
        actor_id: row.try_get(13)?,
        details: row.try_get(14)?,
    }))
}

pub async fn get_inbox_item_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<InboxItem>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, recipient_type, recipient_id, type, severity, issue_id, title, body, read, archived, created_at, actor_type, actor_id, details FROM inbox_item
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(InboxItem {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        recipient_type: row.try_get(2)?,
        recipient_id: row.try_get(3)?,
        type_: row.try_get(4)?,
        severity: row.try_get(5)?,
        issue_id: row.try_get(6)?,
        title: row.try_get(7)?,
        body: row.try_get(8)?,
        read: row.try_get(9)?,
        archived: row.try_get(10)?,
        created_at: row.try_get(11)?,
        actor_type: row.try_get(12)?,
        actor_id: row.try_get(13)?,
        details: row.try_get(14)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListArchivedInboxItemsRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub recipient_type: String,
    pub recipient_id: Option<Uuid>,
    #[serde(rename = "type")]
    pub type_: String,
    pub severity: String,
    pub issue_id: Option<Uuid>,
    pub title: String,
    pub body: Option<String>,
    pub read: bool,
    pub archived: bool,
    pub created_at: Option<DateTime<Utc>>,
    pub actor_type: Option<String>,
    pub actor_id: Option<Uuid>,
    pub details: Option<serde_json::Value>,
    pub issue_status: Option<String>,
}

pub async fn list_archived_inbox_items(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    recipient_type: &str,
    recipient_id: Uuid,
) -> anyhow::Result<Vec<ListArchivedInboxItemsRow>> {
    let rows = sqlx::query(
        r#"SELECT i.id, i.workspace_id, i.recipient_type, i.recipient_id, i.type, i.severity, i.issue_id, i.title, i.body, i.read, i.archived, i.created_at, i.actor_type, i.actor_id, i.details,
       iss.status as issue_status
FROM inbox_item i
LEFT JOIN issue iss ON iss.id = i.issue_id
WHERE i.workspace_id = $1 AND i.recipient_type = $2 AND i.recipient_id = $3 AND i.archived = true
  AND (i.issue_id IS NULL OR NOT EXISTS (
      SELECT 1
      FROM inbox_item active
      WHERE active.workspace_id = i.workspace_id
        AND active.recipient_type = i.recipient_type
        AND active.recipient_id = i.recipient_id
        AND active.issue_id = i.issue_id
        AND active.archived = false
  ))
ORDER BY i.created_at DESC
LIMIT 200"#
    )
        .bind(workspace_id)
        .bind(recipient_type)
        .bind(recipient_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListArchivedInboxItemsRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            recipient_type: row.try_get(2)?,
            recipient_id: row.try_get(3)?,
            type_: row.try_get(4)?,
            severity: row.try_get(5)?,
            issue_id: row.try_get(6)?,
            title: row.try_get(7)?,
            body: row.try_get(8)?,
            read: row.try_get(9)?,
            archived: row.try_get(10)?,
            created_at: row.try_get(11)?,
            actor_type: row.try_get(12)?,
            actor_id: row.try_get(13)?,
            details: row.try_get(14)?,
            issue_status: row.try_get(15)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListInboxItemsRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub recipient_type: String,
    pub recipient_id: Option<Uuid>,
    #[serde(rename = "type")]
    pub type_: String,
    pub severity: String,
    pub issue_id: Option<Uuid>,
    pub title: String,
    pub body: Option<String>,
    pub read: bool,
    pub archived: bool,
    pub created_at: Option<DateTime<Utc>>,
    pub actor_type: Option<String>,
    pub actor_id: Option<Uuid>,
    pub details: Option<serde_json::Value>,
    pub issue_status: Option<String>,
}

pub async fn list_inbox_items(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    recipient_type: &str,
    recipient_id: Uuid,
) -> anyhow::Result<Vec<ListInboxItemsRow>> {
    let rows = sqlx::query(
        r#"SELECT i.id, i.workspace_id, i.recipient_type, i.recipient_id, i.type, i.severity, i.issue_id, i.title, i.body, i.read, i.archived, i.created_at, i.actor_type, i.actor_id, i.details,
       iss.status as issue_status
FROM inbox_item i
LEFT JOIN issue iss ON iss.id = i.issue_id
WHERE i.workspace_id = $1 AND i.recipient_type = $2 AND i.recipient_id = $3 AND i.archived = false
ORDER BY i.created_at DESC"#
    )
        .bind(workspace_id)
        .bind(recipient_type)
        .bind(recipient_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListInboxItemsRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            recipient_type: row.try_get(2)?,
            recipient_id: row.try_get(3)?,
            type_: row.try_get(4)?,
            severity: row.try_get(5)?,
            issue_id: row.try_get(6)?,
            title: row.try_get(7)?,
            body: row.try_get(8)?,
            read: row.try_get(9)?,
            archived: row.try_get(10)?,
            created_at: row.try_get(11)?,
            actor_type: row.try_get(12)?,
            actor_id: row.try_get(13)?,
            details: row.try_get(14)?,
            issue_status: row.try_get(15)?,
        });
    }
    Ok(out)
}

pub async fn mark_all_inbox_read(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    recipient_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE inbox_item SET read = true
WHERE workspace_id = $1 AND recipient_type = 'member' AND recipient_id = $2 AND archived = false AND read = false"#
    )
        .bind(workspace_id)
        .bind(recipient_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn mark_inbox_read(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<InboxItem>> {
    let row = sqlx::query(
        r#"UPDATE inbox_item SET read = true
WHERE id = $1
RETURNING id, workspace_id, recipient_type, recipient_id, type, severity, issue_id, title, body, read, archived, created_at, actor_type, actor_id, details"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(InboxItem {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        recipient_type: row.try_get(2)?,
        recipient_id: row.try_get(3)?,
        type_: row.try_get(4)?,
        severity: row.try_get(5)?,
        issue_id: row.try_get(6)?,
        title: row.try_get(7)?,
        body: row.try_get(8)?,
        read: row.try_get(9)?,
        archived: row.try_get(10)?,
        created_at: row.try_get(11)?,
        actor_type: row.try_get(12)?,
        actor_id: row.try_get(13)?,
        details: row.try_get(14)?,
    }))
}

pub async fn mark_inbox_unread(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<InboxItem>> {
    let row = sqlx::query(
        r#"UPDATE inbox_item SET read = false
WHERE id = $1
RETURNING id, workspace_id, recipient_type, recipient_id, type, severity, issue_id, title, body, read, archived, created_at, actor_type, actor_id, details"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(InboxItem {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        recipient_type: row.try_get(2)?,
        recipient_id: row.try_get(3)?,
        type_: row.try_get(4)?,
        severity: row.try_get(5)?,
        issue_id: row.try_get(6)?,
        title: row.try_get(7)?,
        body: row.try_get(8)?,
        read: row.try_get(9)?,
        archived: row.try_get(10)?,
        created_at: row.try_get(11)?,
        actor_type: row.try_get(12)?,
        actor_id: row.try_get(13)?,
        details: row.try_get(14)?,
    }))
}

pub async fn unarchive_inbox_by_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    recipient_type: &str,
    recipient_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE inbox_item SET archived = false
WHERE workspace_id = $1 AND recipient_type = $2 AND recipient_id = $3 AND issue_id = $4 AND archived = true"#
    )
        .bind(workspace_id)
        .bind(recipient_type)
        .bind(recipient_id)
        .bind(issue_id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn unarchive_inbox_item(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<InboxItem>> {
    let row = sqlx::query(
        r#"UPDATE inbox_item SET archived = false
WHERE id = $1
RETURNING id, workspace_id, recipient_type, recipient_id, type, severity, issue_id, title, body, read, archived, created_at, actor_type, actor_id, details"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(InboxItem {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        recipient_type: row.try_get(2)?,
        recipient_id: row.try_get(3)?,
        type_: row.try_get(4)?,
        severity: row.try_get(5)?,
        issue_id: row.try_get(6)?,
        title: row.try_get(7)?,
        body: row.try_get(8)?,
        read: row.try_get(9)?,
        archived: row.try_get(10)?,
        created_at: row.try_get(11)?,
        actor_type: row.try_get(12)?,
        actor_id: row.try_get(13)?,
        details: row.try_get(14)?,
    }))
}
