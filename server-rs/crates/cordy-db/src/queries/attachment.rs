//! Typed SQL queries for attachment records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn bind_chat_attachments_to_message(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_message_id: Uuid,
    workspace_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"UPDATE attachment
SET chat_message_id = $1
WHERE workspace_id = $2
  AND task_id = $3
  AND issue_id IS NULL
  AND comment_id IS NULL
  AND chat_message_id IS NULL
RETURNING id"#,
    )
    .bind(chat_message_id)
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn count_unbound_chat_attachments_for_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT COUNT(*) FROM attachment
WHERE workspace_id = $1
  AND task_id = $2
  AND issue_id IS NULL
  AND comment_id IS NULL
  AND chat_message_id IS NULL"#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CreateAttachmentRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub comment_id: Option<Uuid>,
    pub uploader_type: String,
    pub uploader_id: Option<Uuid>,
    pub filename: String,
    pub url: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub created_at: Option<DateTime<Utc>>,
    pub chat_session_id: Option<Uuid>,
    pub chat_message_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub issue_revision: i64,
    pub comment_revision: i64,
}

pub async fn create_attachment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    uploader_type: &str,
    uploader_id: Uuid,
    filename: &str,
    url: &str,
    content_type: &str,
    size_bytes: i64,
    // Attachment foreign keys are nullable because a record may be bound to
    // any one of these owners.
    issue_id: Option<Uuid>,
    comment_id: Option<Uuid>,
    chat_session_id: Option<Uuid>,
    task_id: Option<Uuid>,
) -> anyhow::Result<Option<CreateAttachmentRow>> {
    let row = sqlx::query(
        r#"WITH inserted AS (
  INSERT INTO attachment (
    id, workspace_id, issue_id, comment_id, chat_session_id, task_id,
    uploader_type, uploader_id, filename, url, content_type, size_bytes
  )
  VALUES (
    $1, $2, $9, $10, $11, $12,
    $3, $4, $5, $6, $7, $8
  )
  RETURNING id, workspace_id, issue_id, comment_id, uploader_type, uploader_id, filename, url, content_type, size_bytes, created_at, chat_session_id, chat_message_id, task_id
), bumped_issue AS (
  UPDATE issue
  SET revision = revision + 1
  WHERE id IN (SELECT issue_id FROM inserted WHERE issue_id IS NOT NULL)
  RETURNING revision
), bumped_comment AS (
  UPDATE comment
  SET revision = revision + 1
  WHERE id IN (SELECT comment_id FROM inserted WHERE comment_id IS NOT NULL)
  RETURNING revision
)
SELECT inserted.id, inserted.workspace_id, inserted.issue_id, inserted.comment_id, inserted.uploader_type, inserted.uploader_id, inserted.filename, inserted.url, inserted.content_type, inserted.size_bytes, inserted.created_at, inserted.chat_session_id, inserted.chat_message_id, inserted.task_id,
       COALESCE((SELECT revision FROM bumped_issue), 0)::bigint AS issue_revision,
       COALESCE((SELECT revision FROM bumped_comment), 0)::bigint AS comment_revision
FROM inserted"#
    )
        .bind(id)
        .bind(workspace_id)
        .bind(uploader_type)
        .bind(uploader_id)
        .bind(filename)
        .bind(url)
        .bind(content_type)
        .bind(size_bytes)
        .bind(issue_id)
        .bind(comment_id)
        .bind(chat_session_id)
        .bind(task_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(CreateAttachmentRow {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        issue_id: row.try_get(2)?,
        comment_id: row.try_get(3)?,
        uploader_type: row.try_get(4)?,
        uploader_id: row.try_get(5)?,
        filename: row.try_get(6)?,
        url: row.try_get(7)?,
        content_type: row.try_get(8)?,
        size_bytes: row.try_get(9)?,
        created_at: row.try_get(10)?,
        chat_session_id: row.try_get(11)?,
        chat_message_id: row.try_get(12)?,
        task_id: row.try_get(13)?,
        issue_revision: row.try_get(14)?,
        comment_revision: row.try_get(15)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DeleteAttachmentRow {
    pub changed: bool,
    pub issue_revision: i64,
    pub comment_revision: i64,
}

pub async fn delete_attachment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<DeleteAttachmentRow>> {
    let row = sqlx::query(
        r#"WITH deleted AS (
  DELETE FROM attachment
  WHERE attachment.id = $1 AND attachment.workspace_id = $2
  RETURNING issue_id, comment_id
), bumped_issue AS (
  UPDATE issue
  SET revision = revision + 1
  WHERE id IN (SELECT issue_id FROM deleted WHERE issue_id IS NOT NULL)
  RETURNING revision
), bumped_comment AS (
  UPDATE comment
  SET revision = revision + 1
  WHERE id IN (SELECT comment_id FROM deleted WHERE comment_id IS NOT NULL)
  RETURNING revision
)
SELECT EXISTS(SELECT 1 FROM deleted) AS changed,
       COALESCE((SELECT revision FROM bumped_issue), 0)::bigint AS issue_revision,
       COALESCE((SELECT revision FROM bumped_comment), 0)::bigint AS comment_revision"#,
    )
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(DeleteAttachmentRow {
        changed: row.try_get(0)?,
        issue_revision: row.try_get(1)?,
        comment_revision: row.try_get(2)?,
    }))
}

pub async fn detach_attachments_from_user_chat_message_by_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
) -> anyhow::Result<Vec<Attachment>> {
    let rows = sqlx::query(
        r#"UPDATE attachment
SET chat_message_id = NULL
WHERE chat_message_id IN (
  SELECT id FROM chat_message WHERE chat_message.task_id = $1 AND role = 'user'
)
RETURNING id, workspace_id, issue_id, comment_id, uploader_type, uploader_id, filename, url, content_type, size_bytes, created_at, chat_session_id, chat_message_id, task_id"#
    )
        .bind(task_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Attachment {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            issue_id: row.try_get(2)?,
            comment_id: row.try_get(3)?,
            uploader_type: row.try_get(4)?,
            uploader_id: row.try_get(5)?,
            filename: row.try_get(6)?,
            url: row.try_get(7)?,
            content_type: row.try_get(8)?,
            size_bytes: row.try_get(9)?,
            created_at: row.try_get(10)?,
            chat_session_id: row.try_get(11)?,
            chat_message_id: row.try_get(12)?,
            task_id: row.try_get(13)?,
        });
    }
    Ok(out)
}

pub async fn get_attachment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Attachment>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, issue_id, comment_id, uploader_type, uploader_id, filename, url, content_type, size_bytes, created_at, chat_session_id, chat_message_id, task_id FROM attachment
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Attachment {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        issue_id: row.try_get(2)?,
        comment_id: row.try_get(3)?,
        uploader_type: row.try_get(4)?,
        uploader_id: row.try_get(5)?,
        filename: row.try_get(6)?,
        url: row.try_get(7)?,
        content_type: row.try_get(8)?,
        size_bytes: row.try_get(9)?,
        created_at: row.try_get(10)?,
        chat_session_id: row.try_get(11)?,
        chat_message_id: row.try_get(12)?,
        task_id: row.try_get(13)?,
    }))
}

pub async fn get_attachment_by_id_only(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Attachment>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, issue_id, comment_id, uploader_type, uploader_id, filename, url, content_type, size_bytes, created_at, chat_session_id, chat_message_id, task_id FROM attachment
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Attachment {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        issue_id: row.try_get(2)?,
        comment_id: row.try_get(3)?,
        uploader_type: row.try_get(4)?,
        uploader_id: row.try_get(5)?,
        filename: row.try_get(6)?,
        url: row.try_get(7)?,
        content_type: row.try_get(8)?,
        size_bytes: row.try_get(9)?,
        created_at: row.try_get(10)?,
        chat_session_id: row.try_get(11)?,
        chat_message_id: row.try_get(12)?,
        task_id: row.try_get(13)?,
    }))
}

pub async fn link_attachments_to_chat_message(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_message_id: Uuid,
    chat_session_id: Uuid,
    workspace_id: Uuid,
    uploader_type: &str,
    uploader_id: Uuid,
    attachment_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"UPDATE attachment
SET chat_message_id = $1,
    chat_session_id = $2
WHERE workspace_id = $3
  AND issue_id IS NULL
  AND comment_id IS NULL
  AND chat_message_id IS NULL
  AND (
    chat_session_id IS NULL
    OR chat_session_id = $2
  )
  AND uploader_type = $4
  AND uploader_id = $5
  AND id = ANY($6::uuid[])
RETURNING id"#,
    )
    .bind(chat_message_id)
    .bind(chat_session_id)
    .bind(workspace_id)
    .bind(uploader_type)
    .bind(uploader_id)
    .bind(attachment_ids)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn link_attachments_to_comment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    comment_id: Uuid,
    issue_id: Uuid,
    column3: Vec<Uuid>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE attachment
SET comment_id = $1
WHERE issue_id = $2
  AND comment_id IS NULL
  AND id = ANY($3::uuid[])"#,
    )
    .bind(comment_id)
    .bind(issue_id)
    .bind(column3)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LinkAttachmentsToIssueRow {
    pub linked_count: i64,
    pub issue_revision: i64,
}

pub async fn link_attachments_to_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    workspace_id: Uuid,
    attachment_ids: Vec<Uuid>,
    bump_revision: bool,
) -> anyhow::Result<Option<LinkAttachmentsToIssueRow>> {
    let row = sqlx::query(
        r#"WITH linked AS (
  UPDATE attachment
  SET issue_id = $1
  WHERE attachment.workspace_id = $2
    AND attachment.issue_id IS NULL
    AND attachment.id = ANY($3::uuid[])
  RETURNING attachment.issue_id
), bumped_issue AS (
  UPDATE issue
  SET revision = revision + 1,
      updated_at = now()
  WHERE id = $1
    AND $4::boolean
    AND EXISTS (SELECT 1 FROM linked)
  RETURNING revision
)
SELECT COUNT(*)::bigint AS linked_count,
       COALESCE((SELECT revision FROM bumped_issue), 0)::bigint AS issue_revision
FROM linked"#,
    )
    .bind(issue_id)
    .bind(workspace_id)
    .bind(attachment_ids)
    .bind(bump_revision)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(LinkAttachmentsToIssueRow {
        linked_count: row.try_get(0)?,
        issue_revision: row.try_get(1)?,
    }))
}

pub async fn list_attachment_ur_ls_by_comment_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    comment_id: Uuid,
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query(
        r#"SELECT url FROM attachment
WHERE comment_id = $1"#,
    )
    .bind(comment_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_attachment_ur_ls_by_issue_or_comments(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query(
        r#"SELECT a.url FROM attachment a
WHERE a.issue_id = $1
   OR a.comment_id IN (SELECT c.id FROM comment c WHERE c.issue_id = $1)"#,
    )
    .bind(issue_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn list_attachments_by_chat_message(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_message_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Attachment>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, issue_id, comment_id, uploader_type, uploader_id, filename, url, content_type, size_bytes, created_at, chat_session_id, chat_message_id, task_id FROM attachment
WHERE chat_message_id = $1 AND workspace_id = $2
ORDER BY created_at ASC"#
    )
        .bind(chat_message_id)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Attachment {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            issue_id: row.try_get(2)?,
            comment_id: row.try_get(3)?,
            uploader_type: row.try_get(4)?,
            uploader_id: row.try_get(5)?,
            filename: row.try_get(6)?,
            url: row.try_get(7)?,
            content_type: row.try_get(8)?,
            size_bytes: row.try_get(9)?,
            created_at: row.try_get(10)?,
            chat_session_id: row.try_get(11)?,
            chat_message_id: row.try_get(12)?,
            task_id: row.try_get(13)?,
        });
    }
    Ok(out)
}

pub async fn list_attachments_by_chat_message_i_ds(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    column1: Vec<Uuid>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Attachment>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, issue_id, comment_id, uploader_type, uploader_id, filename, url, content_type, size_bytes, created_at, chat_session_id, chat_message_id, task_id FROM attachment
WHERE chat_message_id = ANY($1::uuid[]) AND workspace_id = $2
ORDER BY created_at ASC"#
    )
        .bind(column1)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Attachment {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            issue_id: row.try_get(2)?,
            comment_id: row.try_get(3)?,
            uploader_type: row.try_get(4)?,
            uploader_id: row.try_get(5)?,
            filename: row.try_get(6)?,
            url: row.try_get(7)?,
            content_type: row.try_get(8)?,
            size_bytes: row.try_get(9)?,
            created_at: row.try_get(10)?,
            chat_session_id: row.try_get(11)?,
            chat_message_id: row.try_get(12)?,
            task_id: row.try_get(13)?,
        });
    }
    Ok(out)
}

pub async fn list_attachments_by_comment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    comment_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Attachment>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, issue_id, comment_id, uploader_type, uploader_id, filename, url, content_type, size_bytes, created_at, chat_session_id, chat_message_id, task_id FROM attachment
WHERE comment_id = $1 AND workspace_id = $2
ORDER BY created_at ASC"#
    )
        .bind(comment_id)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Attachment {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            issue_id: row.try_get(2)?,
            comment_id: row.try_get(3)?,
            uploader_type: row.try_get(4)?,
            uploader_id: row.try_get(5)?,
            filename: row.try_get(6)?,
            url: row.try_get(7)?,
            content_type: row.try_get(8)?,
            size_bytes: row.try_get(9)?,
            created_at: row.try_get(10)?,
            chat_session_id: row.try_get(11)?,
            chat_message_id: row.try_get(12)?,
            task_id: row.try_get(13)?,
        });
    }
    Ok(out)
}

pub async fn list_attachments_by_comment_i_ds(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    column1: Vec<Uuid>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Attachment>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, issue_id, comment_id, uploader_type, uploader_id, filename, url, content_type, size_bytes, created_at, chat_session_id, chat_message_id, task_id FROM attachment
WHERE comment_id = ANY($1::uuid[]) AND workspace_id = $2
ORDER BY created_at ASC"#
    )
        .bind(column1)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Attachment {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            issue_id: row.try_get(2)?,
            comment_id: row.try_get(3)?,
            uploader_type: row.try_get(4)?,
            uploader_id: row.try_get(5)?,
            filename: row.try_get(6)?,
            url: row.try_get(7)?,
            content_type: row.try_get(8)?,
            size_bytes: row.try_get(9)?,
            created_at: row.try_get(10)?,
            chat_session_id: row.try_get(11)?,
            chat_message_id: row.try_get(12)?,
            task_id: row.try_get(13)?,
        });
    }
    Ok(out)
}

pub async fn list_attachments_by_i_ds(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    attachment_ids: Vec<Uuid>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Attachment>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, issue_id, comment_id, uploader_type, uploader_id, filename, url, content_type, size_bytes, created_at, chat_session_id, chat_message_id, task_id FROM attachment
WHERE id = ANY($1::uuid[]) AND workspace_id = $2
ORDER BY created_at ASC"#
    )
        .bind(attachment_ids)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Attachment {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            issue_id: row.try_get(2)?,
            comment_id: row.try_get(3)?,
            uploader_type: row.try_get(4)?,
            uploader_id: row.try_get(5)?,
            filename: row.try_get(6)?,
            url: row.try_get(7)?,
            content_type: row.try_get(8)?,
            size_bytes: row.try_get(9)?,
            created_at: row.try_get(10)?,
            chat_session_id: row.try_get(11)?,
            chat_message_id: row.try_get(12)?,
            task_id: row.try_get(13)?,
        });
    }
    Ok(out)
}

pub async fn list_attachments_by_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Attachment>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, issue_id, comment_id, uploader_type, uploader_id, filename, url, content_type, size_bytes, created_at, chat_session_id, chat_message_id, task_id FROM attachment
WHERE issue_id = $1 AND workspace_id = $2
ORDER BY created_at ASC"#
    )
        .bind(issue_id)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Attachment {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            issue_id: row.try_get(2)?,
            comment_id: row.try_get(3)?,
            uploader_type: row.try_get(4)?,
            uploader_id: row.try_get(5)?,
            filename: row.try_get(6)?,
            url: row.try_get(7)?,
            content_type: row.try_get(8)?,
            size_bytes: row.try_get(9)?,
            created_at: row.try_get(10)?,
            chat_session_id: row.try_get(11)?,
            chat_message_id: row.try_get(12)?,
            task_id: row.try_get(13)?,
        });
    }
    Ok(out)
}

pub async fn lock_attachments_for_issue_link(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    attachment_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<Option<Uuid>>> {
    let rows = sqlx::query(
        r#"SELECT id FROM attachment
WHERE workspace_id = $1
  AND issue_id IS NULL
  AND id = ANY($2::uuid[])
ORDER BY id
FOR UPDATE"#,
    )
    .bind(workspace_id)
    .bind(attachment_ids)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn replace_comment_attachments(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    comment_id: Uuid,
    issue_id: Uuid,
    attachment_ids: Vec<Uuid>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE attachment
SET comment_id = CASE
  WHEN id = ANY($3::uuid[]) THEN $1
  ELSE NULL
END
WHERE issue_id = $2
  AND (
    (comment_id = $1 AND NOT id = ANY($3::uuid[]))
    OR (comment_id IS NULL AND id = ANY($3::uuid[]))
  )"#,
    )
    .bind(comment_id)
    .bind(issue_id)
    .bind(attachment_ids)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}
