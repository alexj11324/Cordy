//! Port of server/pkg/db/queries/comment.sql (generated comment.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn bump_comment_revision(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Comment>> {
    let row = sqlx::query(
        r#"UPDATE comment
SET revision = revision + 1,
    updated_at = now()
WHERE id = $1
  AND workspace_id = $2
RETURNING id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Comment {
        id: row.try_get(0)?,
        issue_id: row.try_get(1)?,
        author_type: row.try_get(2)?,
        author_id: row.try_get(3)?,
        content: row.try_get(4)?,
        type_: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        parent_id: row.try_get(8)?,
        workspace_id: row.try_get(9)?,
        resolved_at: row.try_get(10)?,
        resolved_by_type: row.try_get(11)?,
        resolved_by_id: row.try_get(12)?,
        source_task_id: row.try_get(13)?,
        quick_action_id: row.try_get(14)?,
        via_plugin_id: row.try_get(15)?,
        revision: row.try_get(16)?,
    }))
}

pub async fn clear_other_thread_resolutions(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    target_id: Uuid,
    issue_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Comment>> {
    let rows = sqlx::query(
        r#"WITH RECURSIVE root_of AS (
    -- Walk up from the target to its thread root.
    SELECT c.id, c.parent_id
    FROM comment c
    WHERE c.id = $1 AND c.issue_id = $2 AND c.workspace_id = $3
    UNION ALL
    SELECT p.id, p.parent_id
    FROM comment p
    JOIN root_of r ON p.id = r.parent_id
),
thread_root AS (
    SELECT id FROM root_of WHERE parent_id IS NULL LIMIT 1
),
descendants AS (
    -- Expand back down from the root over the whole subtree. Cycle-safe under
    -- the PK constraint (a comment cannot be its own ancestor).
    SELECT c.id
    FROM comment c
    JOIN thread_root tr ON c.id = tr.id
    UNION
    SELECT c.id
    FROM comment c
    JOIN descendants d ON c.parent_id = d.id
    WHERE c.issue_id = $2 AND c.workspace_id = $3
)
UPDATE comment SET
    resolved_at = NULL,
    resolved_by_type = NULL,
    resolved_by_id = NULL,
    revision = revision + 1,
    updated_at = now()
WHERE comment.id IN (SELECT id FROM descendants)
  AND comment.id <> $1
  AND comment.resolved_at IS NOT NULL
RETURNING id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision"#
    )
        .bind(target_id)
        .bind(issue_id)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Comment {
            id: row.try_get(0)?,
            issue_id: row.try_get(1)?,
            author_type: row.try_get(2)?,
            author_id: row.try_get(3)?,
            content: row.try_get(4)?,
            type_: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            parent_id: row.try_get(8)?,
            workspace_id: row.try_get(9)?,
            resolved_at: row.try_get(10)?,
            resolved_by_type: row.try_get(11)?,
            resolved_by_id: row.try_get(12)?,
            source_task_id: row.try_get(13)?,
            quick_action_id: row.try_get(14)?,
            via_plugin_id: row.try_get(15)?,
            revision: row.try_get(16)?,
        });
    }
    Ok(out)
}

pub async fn count_comments(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT count(*) FROM comment
WHERE issue_id = $1 AND workspace_id = $2"#,
    )
    .bind(issue_id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn count_new_comments_since(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    workspace_id: Uuid,
    since: Option<DateTime<Utc>>,
    anchor_id: Uuid,
    author_id: Uuid,
) -> anyhow::Result<Option<i64>> {
    let row = sqlx::query(
        r#"SELECT count(*) FROM comment
WHERE issue_id = $1
  AND workspace_id = $2
  AND created_at > $3
  AND id <> $4
  AND NOT (author_type = 'agent' AND author_id = $5)"#,
    )
    .bind(issue_id)
    .bind(workspace_id)
    .bind(since)
    .bind(anchor_id)
    .bind(author_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CreateCommentRow {
    pub id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub author_type: String,
    pub author_id: Option<Uuid>,
    pub content: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub parent_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by_type: Option<String>,
    pub resolved_by_id: Option<Uuid>,
    pub source_task_id: Option<Uuid>,
    pub quick_action_id: Option<Uuid>,
    pub via_plugin_id: Option<Uuid>,
    pub revision: i64,
    pub issue_revision: i64,
}

pub async fn create_comment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    workspace_id: Uuid,
    author_type: &str,
    author_id: Uuid,
    content: &str,
    type_: &str,
    parent_id: Option<Uuid>,
    source_task_id: Option<Uuid>,
    quick_action_id: Option<Uuid>,
    via_plugin_id: Option<Uuid>,
    id: Uuid,
) -> anyhow::Result<Option<CreateCommentRow>> {
    let row = sqlx::query(
        r#"WITH touched_issue AS (
    UPDATE issue SET
        updated_at = now(),
        revision = revision + 1,
        last_activity_at = GREATEST(COALESCE(last_activity_at, updated_at), now())
    WHERE issue.id = $1 AND issue.workspace_id = $2
    RETURNING issue.id, issue.workspace_id, issue.revision
), inserted_comment AS (
    INSERT INTO comment (issue_id, workspace_id, author_type, author_id, content, type, parent_id, source_task_id, quick_action_id, via_plugin_id, id)
    SELECT ti.id, ti.workspace_id, $3, $4, $5, $6, $7, $8, $9, $10, COALESCE($11::uuid, gen_random_uuid())
    FROM touched_issue ti
    RETURNING id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision
)
SELECT inserted_comment.id, inserted_comment.issue_id, inserted_comment.author_type, inserted_comment.author_id, inserted_comment.content, inserted_comment.type, inserted_comment.created_at, inserted_comment.updated_at, inserted_comment.parent_id, inserted_comment.workspace_id, inserted_comment.resolved_at, inserted_comment.resolved_by_type, inserted_comment.resolved_by_id, inserted_comment.source_task_id, inserted_comment.quick_action_id, inserted_comment.via_plugin_id, inserted_comment.revision, touched_issue.revision AS issue_revision
FROM inserted_comment
JOIN touched_issue ON touched_issue.id = inserted_comment.issue_id"#
    )
        .bind(issue_id)
        .bind(workspace_id)
        .bind(author_type)
        .bind(author_id)
        .bind(content)
        .bind(type_)
        .bind(parent_id)
        .bind(source_task_id)
        .bind(quick_action_id)
        .bind(via_plugin_id)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(CreateCommentRow {
        id: row.try_get(0)?,
        issue_id: row.try_get(1)?,
        author_type: row.try_get(2)?,
        author_id: row.try_get(3)?,
        content: row.try_get(4)?,
        type_: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        parent_id: row.try_get(8)?,
        workspace_id: row.try_get(9)?,
        resolved_at: row.try_get(10)?,
        resolved_by_type: row.try_get(11)?,
        resolved_by_id: row.try_get(12)?,
        source_task_id: row.try_get(13)?,
        quick_action_id: row.try_get(14)?,
        via_plugin_id: row.try_get(15)?,
        revision: row.try_get(16)?,
        issue_revision: row.try_get(17)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DeleteCommentRow {
    pub changed: bool,
    pub issue_revision: i64,
}

pub async fn delete_comment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<DeleteCommentRow>> {
    let row = sqlx::query(
        r#"WITH locked_issue AS MATERIALIZED (
    -- Lock the aggregate owner before its child so this cannot deadlock with
    -- issue teardown (which takes the same issue -> comment order).
    SELECT issue.id
    FROM issue
    JOIN comment ON comment.issue_id = issue.id
                AND comment.workspace_id = issue.workspace_id
    WHERE comment.id = $1 AND comment.workspace_id = $2
    FOR UPDATE OF issue
), issue_fence AS MATERIALIZED (
    -- The consumed locked_count below is the ordering fence: the issue lock is
    -- acquired before DELETE can lock the comment. MATERIALIZED only prevents
    -- folding/re-evaluation and is not, by itself, a lock-order guarantee.
    SELECT count(*) AS locked_count FROM locked_issue
), deleted_comment AS (
    DELETE FROM comment
    USING issue_fence
    WHERE comment.id = $1 AND comment.workspace_id = $2
      AND issue_fence.locked_count >= 0
    RETURNING issue_id, workspace_id
), touched_issue AS (
    UPDATE issue
    SET revision = issue.revision + 1,
        last_activity_at = GREATEST(COALESCE(issue.last_activity_at, issue.updated_at), now())
    FROM deleted_comment
    WHERE issue.id = deleted_comment.issue_id
      AND issue.workspace_id = deleted_comment.workspace_id
    RETURNING issue.id, issue.revision
)
SELECT EXISTS(SELECT 1 FROM deleted_comment) AS changed,
       COALESCE((SELECT revision FROM touched_issue), 0)::bigint AS issue_revision"#,
    )
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(DeleteCommentRow {
        changed: row.try_get(0)?,
        issue_revision: row.try_get(1)?,
    }))
}

pub async fn get_comment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Comment>> {
    let row = sqlx::query(
        r#"SELECT id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision FROM comment
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Comment {
        id: row.try_get(0)?,
        issue_id: row.try_get(1)?,
        author_type: row.try_get(2)?,
        author_id: row.try_get(3)?,
        content: row.try_get(4)?,
        type_: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        parent_id: row.try_get(8)?,
        workspace_id: row.try_get(9)?,
        resolved_at: row.try_get(10)?,
        resolved_by_type: row.try_get(11)?,
        resolved_by_id: row.try_get(12)?,
        source_task_id: row.try_get(13)?,
        quick_action_id: row.try_get(14)?,
        via_plugin_id: row.try_get(15)?,
        revision: row.try_get(16)?,
    }))
}

pub async fn get_comment_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Comment>> {
    let row = sqlx::query(
        r#"SELECT id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision FROM comment
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Comment {
        id: row.try_get(0)?,
        issue_id: row.try_get(1)?,
        author_type: row.try_get(2)?,
        author_id: row.try_get(3)?,
        content: row.try_get(4)?,
        type_: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        parent_id: row.try_get(8)?,
        workspace_id: row.try_get(9)?,
        resolved_at: row.try_get(10)?,
        resolved_by_type: row.try_get(11)?,
        resolved_by_id: row.try_get(12)?,
        source_task_id: row.try_get(13)?,
        quick_action_id: row.try_get(14)?,
        via_plugin_id: row.try_get(15)?,
        revision: row.try_get(16)?,
    }))
}

pub async fn get_delegated_failure_recovery_comment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    workspace_id: Uuid,
    source_task_id: Uuid,
) -> anyhow::Result<Option<Comment>> {
    let row = sqlx::query(
        r#"SELECT id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision FROM comment
WHERE issue_id = $1
  AND workspace_id = $2
  AND author_type = 'system'
  AND type = 'progress_update'
  AND source_task_id = $3
ORDER BY created_at ASC, id ASC
LIMIT 1"#
    )
        .bind(issue_id)
        .bind(workspace_id)
        .bind(source_task_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Comment {
        id: row.try_get(0)?,
        issue_id: row.try_get(1)?,
        author_type: row.try_get(2)?,
        author_id: row.try_get(3)?,
        content: row.try_get(4)?,
        type_: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        parent_id: row.try_get(8)?,
        workspace_id: row.try_get(9)?,
        resolved_at: row.try_get(10)?,
        resolved_by_type: row.try_get(11)?,
        resolved_by_id: row.try_get(12)?,
        source_task_id: row.try_get(13)?,
        quick_action_id: row.try_get(14)?,
        via_plugin_id: row.try_get(15)?,
        revision: row.try_get(16)?,
    }))
}

pub async fn get_delegated_failure_recovery_exhaustion_comment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    workspace_id: Uuid,
    source_task_id: Uuid,
) -> anyhow::Result<Option<Comment>> {
    let row = sqlx::query(
        r#"SELECT id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision FROM comment
WHERE issue_id = $1
  AND workspace_id = $2
  AND author_type = 'system'
  AND type = 'system'
  AND source_task_id = $3
ORDER BY created_at ASC, id ASC
LIMIT 1"#
    )
        .bind(issue_id)
        .bind(workspace_id)
        .bind(source_task_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Comment {
        id: row.try_get(0)?,
        issue_id: row.try_get(1)?,
        author_type: row.try_get(2)?,
        author_id: row.try_get(3)?,
        content: row.try_get(4)?,
        type_: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        parent_id: row.try_get(8)?,
        workspace_id: row.try_get(9)?,
        resolved_at: row.try_get(10)?,
        resolved_by_type: row.try_get(11)?,
        resolved_by_id: row.try_get(12)?,
        source_task_id: row.try_get(13)?,
        quick_action_id: row.try_get(14)?,
        via_plugin_id: row.try_get(15)?,
        revision: row.try_get(16)?,
    }))
}

pub async fn get_latest_member_comment_for_issue_since(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    since: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<Comment>> {
    let row = sqlx::query(
        r#"SELECT id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision FROM comment
WHERE issue_id = $1
  AND author_type = 'member'
  AND created_at > $2
ORDER BY created_at DESC
LIMIT 1"#
    )
        .bind(issue_id)
        .bind(since)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Comment {
        id: row.try_get(0)?,
        issue_id: row.try_get(1)?,
        author_type: row.try_get(2)?,
        author_id: row.try_get(3)?,
        content: row.try_get(4)?,
        type_: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        parent_id: row.try_get(8)?,
        workspace_id: row.try_get(9)?,
        resolved_at: row.try_get(10)?,
        resolved_by_type: row.try_get(11)?,
        resolved_by_id: row.try_get(12)?,
        source_task_id: row.try_get(13)?,
        quick_action_id: row.try_get(14)?,
        via_plugin_id: row.try_get(15)?,
        revision: row.try_get(16)?,
    }))
}

pub async fn get_thread_root(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    comment_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Comment>> {
    let row = sqlx::query(
        r#"WITH RECURSIVE root_of AS (
    SELECT c.id, c.parent_id
    FROM comment c
    WHERE c.id = $1 AND c.workspace_id = $2
    UNION ALL
    SELECT p.id, p.parent_id
    FROM comment p
    JOIN root_of r ON p.id = r.parent_id
)
SELECT c.id, c.issue_id, c.author_type, c.author_id, c.content, c.type, c.created_at, c.updated_at, c.parent_id, c.workspace_id, c.resolved_at, c.resolved_by_type, c.resolved_by_id, c.source_task_id, c.quick_action_id, c.via_plugin_id, c.revision FROM comment c
WHERE c.id = (SELECT id FROM root_of WHERE parent_id IS NULL LIMIT 1)"#
    )
        .bind(comment_id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Comment {
        id: row.try_get(0)?,
        issue_id: row.try_get(1)?,
        author_type: row.try_get(2)?,
        author_id: row.try_get(3)?,
        content: row.try_get(4)?,
        type_: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        parent_id: row.try_get(8)?,
        workspace_id: row.try_get(9)?,
        resolved_at: row.try_get(10)?,
        resolved_by_type: row.try_get(11)?,
        resolved_by_id: row.try_get(12)?,
        source_task_id: row.try_get(13)?,
        quick_action_id: row.try_get(14)?,
        via_plugin_id: row.try_get(15)?,
        revision: row.try_get(16)?,
    }))
}

pub async fn has_agent_commented_since(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    author_id: Uuid,
    since: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT EXISTS (
    SELECT 1 FROM comment
    WHERE issue_id = $1
      AND author_type = 'agent'
      AND author_id = $2
      AND created_at >= $3
) AS commented"#,
    )
    .bind(issue_id)
    .bind(author_id)
    .bind(since)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn has_agent_replied_in_thread(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    parent_id: Uuid,
    agent_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT count(*) > 0 AS has_replied FROM comment
WHERE parent_id = $1 AND author_type = 'agent' AND author_id = $2"#,
    )
    .bind(parent_id)
    .bind(agent_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn list_child_comments_for_parents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    parent_ids: Vec<Uuid>,
    issue_id: Uuid,
    workspace_id: Uuid,
    through_at: Option<DateTime<Utc>>,
    through_id: Uuid,
    row_limit: i32,
) -> anyhow::Result<Vec<Comment>> {
    let rows = sqlx::query(
        r#"SELECT id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision FROM comment
WHERE parent_id = ANY($1::uuid[])
  AND issue_id = $2
  AND workspace_id = $3
  AND (created_at, id) <= ($4::timestamptz, $5::uuid)
ORDER BY parent_id ASC, created_at ASC, id ASC
LIMIT $6"#
    )
        .bind(parent_ids)
        .bind(issue_id)
        .bind(workspace_id)
        .bind(through_at)
        .bind(through_id)
        .bind(row_limit)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Comment {
            id: row.try_get(0)?,
            issue_id: row.try_get(1)?,
            author_type: row.try_get(2)?,
            author_id: row.try_get(3)?,
            content: row.try_get(4)?,
            type_: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            parent_id: row.try_get(8)?,
            workspace_id: row.try_get(9)?,
            resolved_at: row.try_get(10)?,
            resolved_by_type: row.try_get(11)?,
            resolved_by_id: row.try_get(12)?,
            source_task_id: row.try_get(13)?,
            quick_action_id: row.try_get(14)?,
            via_plugin_id: row.try_get(15)?,
            revision: row.try_get(16)?,
        });
    }
    Ok(out)
}

pub async fn list_comments_by_i_ds_for_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    ids: Vec<Uuid>,
    issue_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Comment>> {
    let rows = sqlx::query(
        r#"SELECT id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision FROM comment
WHERE id = ANY($1::uuid[])
  AND issue_id = $2
  AND workspace_id = $3
ORDER BY created_at ASC, id ASC"#
    )
        .bind(ids)
        .bind(issue_id)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Comment {
            id: row.try_get(0)?,
            issue_id: row.try_get(1)?,
            author_type: row.try_get(2)?,
            author_id: row.try_get(3)?,
            content: row.try_get(4)?,
            type_: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            parent_id: row.try_get(8)?,
            workspace_id: row.try_get(9)?,
            resolved_at: row.try_get(10)?,
            resolved_by_type: row.try_get(11)?,
            resolved_by_id: row.try_get(12)?,
            source_task_id: row.try_get(13)?,
            quick_action_id: row.try_get(14)?,
            via_plugin_id: row.try_get(15)?,
            revision: row.try_get(16)?,
        });
    }
    Ok(out)
}

pub async fn list_comments_for_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    workspace_id: Uuid,
    limit: i32,
) -> anyhow::Result<Vec<Comment>> {
    let rows = sqlx::query(
        r#"SELECT id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision FROM (
    SELECT id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision FROM comment
    WHERE issue_id = $1 AND workspace_id = $2
    ORDER BY created_at DESC, id DESC
    LIMIT $3
) AS recent
ORDER BY created_at ASC, id ASC"#
    )
        .bind(issue_id)
        .bind(workspace_id)
        .bind(limit)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Comment {
            id: row.try_get(0)?,
            issue_id: row.try_get(1)?,
            author_type: row.try_get(2)?,
            author_id: row.try_get(3)?,
            content: row.try_get(4)?,
            type_: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            parent_id: row.try_get(8)?,
            workspace_id: row.try_get(9)?,
            resolved_at: row.try_get(10)?,
            resolved_by_type: row.try_get(11)?,
            resolved_by_id: row.try_get(12)?,
            source_task_id: row.try_get(13)?,
            quick_action_id: row.try_get(14)?,
            via_plugin_id: row.try_get(15)?,
            revision: row.try_get(16)?,
        });
    }
    Ok(out)
}

pub async fn list_comments_since_for_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    workspace_id: Uuid,
    created_at: Option<DateTime<Utc>>,
    limit: i32,
) -> anyhow::Result<Vec<Comment>> {
    let rows = sqlx::query(
        r#"SELECT id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision FROM comment
WHERE issue_id = $1 AND workspace_id = $2 AND created_at > $3
ORDER BY created_at ASC, id ASC
LIMIT $4"#
    )
        .bind(issue_id)
        .bind(workspace_id)
        .bind(created_at)
        .bind(limit)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Comment {
            id: row.try_get(0)?,
            issue_id: row.try_get(1)?,
            author_type: row.try_get(2)?,
            author_id: row.try_get(3)?,
            content: row.try_get(4)?,
            type_: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            parent_id: row.try_get(8)?,
            workspace_id: row.try_get(9)?,
            resolved_at: row.try_get(10)?,
            resolved_by_type: row.try_get(11)?,
            resolved_by_id: row.try_get(12)?,
            source_task_id: row.try_get(13)?,
            quick_action_id: row.try_get(14)?,
            via_plugin_id: row.try_get(15)?,
            revision: row.try_get(16)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListRecentThreadCommentsForIssueRow {
    pub id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub author_type: String,
    pub author_id: Option<Uuid>,
    pub content: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub parent_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by_type: Option<String>,
    pub resolved_by_id: Option<Uuid>,
    pub source_task_id: Option<Uuid>,
    pub quick_action_id: Option<Uuid>,
    pub revision: i64,
    pub thread_root_id: Option<Uuid>,
    pub thread_last_activity_at: Option<DateTime<Utc>>,
}

pub async fn list_recent_thread_comments_for_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    workspace_id: Uuid,
    has_cursor: bool,
    before_at: Option<DateTime<Utc>>,
    before_id: Uuid,
    thread_limit: i32,
) -> anyhow::Result<Vec<ListRecentThreadCommentsForIssueRow>> {
    let rows = sqlx::query(
        r#"WITH RECURSIVE membership(id, root_id, comment_created_at) AS (
    -- Each root maps to itself.
    SELECT c.id, c.id AS root_id, c.created_at
    FROM comment c
    WHERE c.issue_id = $1
      AND c.workspace_id = $2
      AND c.parent_id IS NULL
    UNION ALL
    -- Each descendant inherits its parent's root_id.
    SELECT c.id, m.root_id, c.created_at
    FROM comment c
    JOIN membership m ON c.parent_id = m.id
    WHERE c.issue_id = $1
      AND c.workspace_id = $2
),
thread_stats AS (
    SELECT root_id, MAX(comment_created_at)::timestamptz AS last_activity_at
    FROM membership
    GROUP BY root_id
),
picked AS (
    SELECT ts.root_id, ts.last_activity_at
    FROM thread_stats ts
    WHERE (
        $3::boolean = FALSE
        OR (ts.last_activity_at, ts.root_id) < ($4::timestamptz, $5::uuid)
    )
    ORDER BY ts.last_activity_at DESC, ts.root_id DESC
    LIMIT $6
)
SELECT c.id, c.issue_id, c.author_type, c.author_id, c.content, c.type,
       c.created_at, c.updated_at, c.parent_id, c.workspace_id,
       c.resolved_at, c.resolved_by_type, c.resolved_by_id,
       c.source_task_id, c.quick_action_id, c.revision,
       p.root_id AS thread_root_id,
       p.last_activity_at AS thread_last_activity_at
FROM picked p
JOIN membership m ON m.root_id = p.root_id
JOIN comment c ON c.id = m.id
ORDER BY p.last_activity_at ASC, p.root_id ASC, c.created_at ASC, c.id ASC"#,
    )
    .bind(issue_id)
    .bind(workspace_id)
    .bind(has_cursor)
    .bind(before_at)
    .bind(before_id)
    .bind(thread_limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListRecentThreadCommentsForIssueRow {
            id: row.try_get(0)?,
            issue_id: row.try_get(1)?,
            author_type: row.try_get(2)?,
            author_id: row.try_get(3)?,
            content: row.try_get(4)?,
            type_: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            parent_id: row.try_get(8)?,
            workspace_id: row.try_get(9)?,
            resolved_at: row.try_get(10)?,
            resolved_by_type: row.try_get(11)?,
            resolved_by_id: row.try_get(12)?,
            source_task_id: row.try_get(13)?,
            quick_action_id: row.try_get(14)?,
            revision: row.try_get(15)?,
            thread_root_id: row.try_get(16)?,
            thread_last_activity_at: row.try_get(17)?,
        });
    }
    Ok(out)
}

pub async fn list_reconcilable_comments_for_issue_since(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    since: Option<DateTime<Utc>>,
    planned_comment_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<Comment>> {
    let rows = sqlx::query(
        r#"SELECT id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision FROM comment
WHERE issue_id = $1
  AND (
      (
          author_type IN ('member', 'agent')
          AND (created_at > $2 OR id = ANY($3::uuid[]))
      )
      OR (
          author_type = 'system'
          AND type = 'progress_update'
          AND source_task_id IS NOT NULL
          AND id = ANY($3::uuid[])
      )
  )
ORDER BY created_at ASC, id ASC"#
    )
        .bind(issue_id)
        .bind(since)
        .bind(planned_comment_ids)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Comment {
            id: row.try_get(0)?,
            issue_id: row.try_get(1)?,
            author_type: row.try_get(2)?,
            author_id: row.try_get(3)?,
            content: row.try_get(4)?,
            type_: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            parent_id: row.try_get(8)?,
            workspace_id: row.try_get(9)?,
            resolved_at: row.try_get(10)?,
            resolved_by_type: row.try_get(11)?,
            resolved_by_id: row.try_get(12)?,
            source_task_id: row.try_get(13)?,
            quick_action_id: row.try_get(14)?,
            via_plugin_id: row.try_get(15)?,
            revision: row.try_get(16)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListRootCommentsForIssueRow {
    pub id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub author_type: String,
    pub author_id: Option<Uuid>,
    pub content: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub parent_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by_type: Option<String>,
    pub resolved_by_id: Option<Uuid>,
    pub source_task_id: Option<Uuid>,
    pub quick_action_id: Option<Uuid>,
    pub revision: i64,
    pub reply_count: i32,
    pub last_activity_at: Option<DateTime<Utc>>,
}

pub async fn list_root_comments_for_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    workspace_id: Uuid,
    row_limit: i32,
) -> anyhow::Result<Vec<ListRootCommentsForIssueRow>> {
    let rows = sqlx::query(
        r#"WITH RECURSIVE selected_roots AS (
    SELECT c.id, c.created_at
    FROM comment c
    WHERE c.issue_id = $1
      AND c.workspace_id = $2
      AND c.parent_id IS NULL
    ORDER BY c.created_at DESC, c.id DESC
    LIMIT $3
),
membership(id, root_id, comment_created_at) AS (
    SELECT sr.id, sr.id AS root_id, sr.created_at
    FROM selected_roots sr
    UNION ALL
    SELECT c.id, m.root_id, c.created_at
    FROM comment c
    JOIN membership m ON c.parent_id = m.id
    WHERE c.issue_id = $1
      AND c.workspace_id = $2
),
thread_stats AS (
    SELECT root_id,
           (COUNT(*) - 1)::int AS reply_count,
           MAX(comment_created_at)::timestamptz AS last_activity_at
    FROM membership
    GROUP BY root_id
)
SELECT c.id, c.issue_id, c.author_type, c.author_id, c.content, c.type,
       c.created_at, c.updated_at, c.parent_id, c.workspace_id,
       c.resolved_at, c.resolved_by_type, c.resolved_by_id,
       c.source_task_id, c.quick_action_id, c.revision,
       ts.reply_count AS reply_count,
       ts.last_activity_at AS last_activity_at
FROM selected_roots sr
JOIN comment c ON c.id = sr.id
JOIN thread_stats ts ON ts.root_id = sr.id
ORDER BY c.created_at ASC, c.id ASC"#,
    )
    .bind(issue_id)
    .bind(workspace_id)
    .bind(row_limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListRootCommentsForIssueRow {
            id: row.try_get(0)?,
            issue_id: row.try_get(1)?,
            author_type: row.try_get(2)?,
            author_id: row.try_get(3)?,
            content: row.try_get(4)?,
            type_: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            parent_id: row.try_get(8)?,
            workspace_id: row.try_get(9)?,
            resolved_at: row.try_get(10)?,
            resolved_by_type: row.try_get(11)?,
            resolved_by_id: row.try_get(12)?,
            source_task_id: row.try_get(13)?,
            quick_action_id: row.try_get(14)?,
            revision: row.try_get(15)?,
            reply_count: row.try_get(16)?,
            last_activity_at: row.try_get(17)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListRootCommentsSinceForIssueRow {
    pub id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub author_type: String,
    pub author_id: Option<Uuid>,
    pub content: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub parent_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by_type: Option<String>,
    pub resolved_by_id: Option<Uuid>,
    pub source_task_id: Option<Uuid>,
    pub quick_action_id: Option<Uuid>,
    pub revision: i64,
    pub reply_count: i32,
    pub last_activity_at: Option<DateTime<Utc>>,
}

pub async fn list_root_comments_since_for_issue(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    issue_id: Uuid,
    workspace_id: Uuid,
    since: Option<DateTime<Utc>>,
    row_limit: i32,
) -> anyhow::Result<Vec<ListRootCommentsSinceForIssueRow>> {
    let rows = sqlx::query(
        r#"WITH RECURSIVE selected_roots AS (
    SELECT c.id, c.created_at
    FROM comment c
    WHERE c.issue_id = $1
      AND c.workspace_id = $2
      AND c.parent_id IS NULL
      AND c.created_at > $3
    ORDER BY c.created_at ASC, c.id ASC
    LIMIT $4
),
membership(id, root_id, comment_created_at) AS (
    SELECT sr.id, sr.id AS root_id, sr.created_at
    FROM selected_roots sr
    UNION ALL
    SELECT c.id, m.root_id, c.created_at
    FROM comment c
    JOIN membership m ON c.parent_id = m.id
    WHERE c.issue_id = $1
      AND c.workspace_id = $2
),
thread_stats AS (
    SELECT root_id,
           (COUNT(*) - 1)::int AS reply_count,
           MAX(comment_created_at)::timestamptz AS last_activity_at
    FROM membership
    GROUP BY root_id
)
SELECT c.id, c.issue_id, c.author_type, c.author_id, c.content, c.type,
       c.created_at, c.updated_at, c.parent_id, c.workspace_id,
       c.resolved_at, c.resolved_by_type, c.resolved_by_id,
       c.source_task_id, c.quick_action_id, c.revision,
       ts.reply_count AS reply_count,
       ts.last_activity_at AS last_activity_at
FROM selected_roots sr
JOIN comment c ON c.id = sr.id
JOIN thread_stats ts ON ts.root_id = sr.id
ORDER BY c.created_at ASC, c.id ASC"#,
    )
    .bind(issue_id)
    .bind(workspace_id)
    .bind(since)
    .bind(row_limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListRootCommentsSinceForIssueRow {
            id: row.try_get(0)?,
            issue_id: row.try_get(1)?,
            author_type: row.try_get(2)?,
            author_id: row.try_get(3)?,
            content: row.try_get(4)?,
            type_: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            parent_id: row.try_get(8)?,
            workspace_id: row.try_get(9)?,
            resolved_at: row.try_get(10)?,
            resolved_by_type: row.try_get(11)?,
            resolved_by_id: row.try_get(12)?,
            source_task_id: row.try_get(13)?,
            quick_action_id: row.try_get(14)?,
            revision: row.try_get(15)?,
            reply_count: row.try_get(16)?,
            last_activity_at: row.try_get(17)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListThreadCommentsForIssuePagedRow {
    pub id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub author_type: String,
    pub author_id: Option<Uuid>,
    pub content: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub parent_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by_type: Option<String>,
    pub resolved_by_id: Option<Uuid>,
    pub source_task_id: Option<Uuid>,
    pub quick_action_id: Option<Uuid>,
    pub revision: i64,
}

pub async fn list_thread_comments_for_issue_paged(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    anchor_id: Uuid,
    issue_id: Uuid,
    workspace_id: Uuid,
    has_cursor: bool,
    before_at: Option<DateTime<Utc>>,
    before_id: Uuid,
    reply_limit: i32,
) -> anyhow::Result<Vec<ListThreadCommentsForIssuePagedRow>> {
    let rows = sqlx::query(
        r#"WITH RECURSIVE root_of AS (
    SELECT c.id, c.parent_id
    FROM comment c
    WHERE c.id = $1 AND c.issue_id = $2 AND c.workspace_id = $3
    UNION ALL
    SELECT p.id, p.parent_id
    FROM comment p
    JOIN root_of r ON p.id = r.parent_id
    WHERE p.issue_id = $2 AND p.workspace_id = $3
),
thread_root AS (
    SELECT id FROM root_of WHERE parent_id IS NULL LIMIT 1
),
descendants AS (
    SELECT c.id, c.issue_id, c.author_type, c.author_id, c.content, c.type,
           c.created_at, c.updated_at, c.parent_id, c.workspace_id,
           c.resolved_at, c.resolved_by_type, c.resolved_by_id,
           c.source_task_id, c.quick_action_id, c.revision
    FROM comment c
    JOIN thread_root tr ON c.id = tr.id
    UNION
    SELECT c.id, c.issue_id, c.author_type, c.author_id, c.content, c.type,
           c.created_at, c.updated_at, c.parent_id, c.workspace_id,
           c.resolved_at, c.resolved_by_type, c.resolved_by_id,
           c.source_task_id, c.quick_action_id, c.revision
    FROM comment c
    JOIN descendants d ON c.parent_id = d.id
    WHERE c.issue_id = $2 AND c.workspace_id = $3
),
reply_page AS (
    SELECT d.id, d.issue_id, d.author_type, d.author_id, d.content, d.type,
           d.created_at, d.updated_at, d.parent_id, d.workspace_id,
           d.resolved_at, d.resolved_by_type, d.resolved_by_id,
           d.source_task_id, d.quick_action_id, d.revision
    FROM descendants d
    WHERE d.id NOT IN (SELECT id FROM thread_root)
      AND (
          $4::boolean = FALSE
          OR (d.created_at, d.id) < ($5::timestamptz, $6::uuid)
      )
    ORDER BY d.created_at DESC, d.id DESC
    LIMIT $7
)
SELECT id, issue_id, author_type, author_id, content, type,
       created_at, updated_at, parent_id, workspace_id,
       resolved_at, resolved_by_type, resolved_by_id,
       source_task_id, quick_action_id, revision
FROM (
    SELECT d.id, d.issue_id, d.author_type, d.author_id, d.content, d.type,
           d.created_at, d.updated_at, d.parent_id, d.workspace_id,
           d.resolved_at, d.resolved_by_type, d.resolved_by_id,
           d.source_task_id, d.quick_action_id, d.revision
    FROM descendants d
    JOIN thread_root tr ON d.id = tr.id
    UNION ALL
    SELECT id, issue_id, author_type, author_id, content, type,
           created_at, updated_at, parent_id, workspace_id,
           resolved_at, resolved_by_type, resolved_by_id,
           source_task_id, quick_action_id, revision
    FROM reply_page
) combined
ORDER BY created_at ASC, id ASC"#,
    )
    .bind(anchor_id)
    .bind(issue_id)
    .bind(workspace_id)
    .bind(has_cursor)
    .bind(before_at)
    .bind(before_id)
    .bind(reply_limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListThreadCommentsForIssuePagedRow {
            id: row.try_get(0)?,
            issue_id: row.try_get(1)?,
            author_type: row.try_get(2)?,
            author_id: row.try_get(3)?,
            content: row.try_get(4)?,
            type_: row.try_get(5)?,
            created_at: row.try_get(6)?,
            updated_at: row.try_get(7)?,
            parent_id: row.try_get(8)?,
            workspace_id: row.try_get(9)?,
            resolved_at: row.try_get(10)?,
            resolved_by_type: row.try_get(11)?,
            resolved_by_id: row.try_get(12)?,
            source_task_id: row.try_get(13)?,
            quick_action_id: row.try_get(14)?,
            revision: row.try_get(15)?,
        });
    }
    Ok(out)
}

pub async fn resolve_comment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    resolved_by_type: Option<&str>,
    resolved_by_id: Uuid,
) -> anyhow::Result<Option<Comment>> {
    let row = sqlx::query(
        r#"UPDATE comment SET
    resolved_at = COALESCE(resolved_at, now()),
    resolved_by_type = COALESCE(resolved_by_type, $2),
    resolved_by_id = COALESCE(resolved_by_id, $3),
    revision = revision + CASE WHEN resolved_at IS NULL THEN 1 ELSE 0 END,
    updated_at = CASE WHEN resolved_at IS NULL THEN now() ELSE updated_at END
WHERE id = $1
RETURNING id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision"#
    )
        .bind(id)
        .bind(resolved_by_type)
        .bind(resolved_by_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Comment {
        id: row.try_get(0)?,
        issue_id: row.try_get(1)?,
        author_type: row.try_get(2)?,
        author_id: row.try_get(3)?,
        content: row.try_get(4)?,
        type_: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        parent_id: row.try_get(8)?,
        workspace_id: row.try_get(9)?,
        resolved_at: row.try_get(10)?,
        resolved_by_type: row.try_get(11)?,
        resolved_by_id: row.try_get(12)?,
        source_task_id: row.try_get(13)?,
        quick_action_id: row.try_get(14)?,
        via_plugin_id: row.try_get(15)?,
        revision: row.try_get(16)?,
    }))
}

pub async fn unresolve_comment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Comment>> {
    let row = sqlx::query(
        r#"UPDATE comment SET
    resolved_at = NULL,
    resolved_by_type = NULL,
    resolved_by_id = NULL,
    revision = revision + CASE WHEN resolved_at IS NOT NULL THEN 1 ELSE 0 END,
    updated_at = CASE WHEN resolved_at IS NOT NULL THEN now() ELSE updated_at END
WHERE id = $1
RETURNING id, issue_id, author_type, author_id, content, type, created_at, updated_at, parent_id, workspace_id, resolved_at, resolved_by_type, resolved_by_id, source_task_id, quick_action_id, via_plugin_id, revision"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Comment {
        id: row.try_get(0)?,
        issue_id: row.try_get(1)?,
        author_type: row.try_get(2)?,
        author_id: row.try_get(3)?,
        content: row.try_get(4)?,
        type_: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        parent_id: row.try_get(8)?,
        workspace_id: row.try_get(9)?,
        resolved_at: row.try_get(10)?,
        resolved_by_type: row.try_get(11)?,
        resolved_by_id: row.try_get(12)?,
        source_task_id: row.try_get(13)?,
        quick_action_id: row.try_get(14)?,
        via_plugin_id: row.try_get(15)?,
        revision: row.try_get(16)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UpdateCommentRow {
    pub id: Option<Uuid>,
    pub issue_id: Option<Uuid>,
    pub author_type: String,
    pub author_id: Option<Uuid>,
    pub content: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub parent_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by_type: Option<String>,
    pub resolved_by_id: Option<Uuid>,
    pub source_task_id: Option<Uuid>,
    pub quick_action_id: Option<Uuid>,
    pub via_plugin_id: Option<Uuid>,
    pub revision: i64,
    pub issue_revision: i64,
    pub content_changed: bool,
}

pub async fn update_comment(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    content: &str,
    source_task_id: Uuid,
    expected_revision: Option<i64>,
    content_base: Option<&str>,
) -> anyhow::Result<Option<UpdateCommentRow>> {
    let row = sqlx::query(
        r#"WITH locked_issue AS MATERIALIZED (
    -- Keep the global issue -> child lock order used by issue teardown. The
    -- aggregate below still yields one row when the parent was concurrently
    -- deleted, preserving best-effort edits of an orphaned comment.
    SELECT issue.id
    FROM issue
    JOIN comment ON comment.issue_id = issue.id
                AND comment.workspace_id = issue.workspace_id
    WHERE comment.id = $1
    FOR UPDATE OF issue
), issue_fence AS MATERIALIZED (
    -- The aggregate always emits one row. Consuming locked_count in target's
    -- tautological predicate creates a real data dependency: locked_issue must
    -- acquire the owner lock before target can lock the comment. MATERIALIZED
    -- prevents folding/re-evaluation; it does not itself establish lock order.
    SELECT count(*) AS locked_count FROM locked_issue
), target AS MATERIALIZED (
    SELECT comment.id, comment.issue_id, comment.author_type, comment.author_id, comment.content, comment.type, comment.created_at, comment.updated_at, comment.parent_id, comment.workspace_id, comment.resolved_at, comment.resolved_by_type, comment.resolved_by_id, comment.source_task_id, comment.quick_action_id, comment.via_plugin_id, comment.revision,
           comment.content IS DISTINCT FROM $2 AS content_changed,
           ROW(comment.content, comment.source_task_id) IS DISTINCT FROM
               ROW($2, $3::uuid) AS did_change
    FROM comment
    CROSS JOIN issue_fence
    WHERE comment.id = $1
      AND issue_fence.locked_count >= 0
      AND ($4::bigint IS NULL OR revision = $4::bigint)
      AND (
        $5::text IS NULL
        OR content IS NOT DISTINCT FROM $5::text
        OR content IS NOT DISTINCT FROM $2
      )
    FOR UPDATE OF comment
), updated_comment AS (
    UPDATE comment SET
        content = $2,
        source_task_id = $3::uuid,
        revision = comment.revision + CASE WHEN target.did_change THEN 1 ELSE 0 END,
        updated_at = CASE WHEN target.did_change THEN now() ELSE comment.updated_at END
    FROM target
    WHERE comment.id = target.id
    RETURNING comment.id, comment.issue_id, comment.author_type, comment.author_id,
              comment.content, comment.type, comment.created_at, comment.updated_at,
              comment.parent_id, comment.workspace_id, comment.resolved_at,
              comment.resolved_by_type, comment.resolved_by_id, comment.source_task_id,
              comment.quick_action_id, comment.via_plugin_id, comment.revision,
              target.did_change, target.content_changed
), touched_issue AS (
    UPDATE issue
    SET revision = issue.revision + 1,
        last_activity_at = GREATEST(COALESCE(issue.last_activity_at, issue.updated_at), now())
    FROM updated_comment
    WHERE updated_comment.did_change
      AND issue.id = updated_comment.issue_id
      AND issue.workspace_id = updated_comment.workspace_id
    RETURNING issue.id, issue.revision
)
SELECT updated_comment.id, updated_comment.issue_id, updated_comment.author_type,
       updated_comment.author_id, updated_comment.content, updated_comment.type,
       updated_comment.created_at, updated_comment.updated_at, updated_comment.parent_id,
       updated_comment.workspace_id, updated_comment.resolved_at,
       updated_comment.resolved_by_type, updated_comment.resolved_by_id,
       updated_comment.source_task_id, updated_comment.quick_action_id,
       updated_comment.via_plugin_id, updated_comment.revision,
       COALESCE((SELECT revision FROM touched_issue), 0)::bigint AS issue_revision,
       updated_comment.content_changed
FROM updated_comment"#
    )
        .bind(id)
        .bind(content)
        .bind(source_task_id)
        .bind(expected_revision)
        .bind(content_base)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(UpdateCommentRow {
        id: row.try_get(0)?,
        issue_id: row.try_get(1)?,
        author_type: row.try_get(2)?,
        author_id: row.try_get(3)?,
        content: row.try_get(4)?,
        type_: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
        parent_id: row.try_get(8)?,
        workspace_id: row.try_get(9)?,
        resolved_at: row.try_get(10)?,
        resolved_by_type: row.try_get(11)?,
        resolved_by_id: row.try_get(12)?,
        source_task_id: row.try_get(13)?,
        quick_action_id: row.try_get(14)?,
        via_plugin_id: row.try_get(15)?,
        revision: row.try_get(16)?,
        issue_revision: row.try_get(17)?,
        content_changed: row.try_get(18)?,
    }))
}

/// Bump comment and issue revisions when an edit changes only attachments.
/// The content update query deliberately has no-op semantics for an unchanged
/// body/source pair, so attachment replacement needs its own revision fence.
pub async fn touch_comment_after_attachment_edit(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    comment_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<Option<(i64, i64)>> {
    let row = sqlx::query(
        r#"WITH updated_comment AS (
    UPDATE comment
    SET revision = revision + 1,
        updated_at = now()
    WHERE id = $1 AND issue_id = $2
    RETURNING id, issue_id, workspace_id, revision
), touched_issue AS (
    UPDATE issue
    SET revision = issue.revision + 1,
        last_activity_at = GREATEST(COALESCE(issue.last_activity_at, issue.updated_at), now())
    FROM updated_comment
    WHERE issue.id = updated_comment.issue_id
      AND issue.workspace_id = updated_comment.workspace_id
    RETURNING issue.revision
)
SELECT updated_comment.revision,
       COALESCE((SELECT revision FROM touched_issue), 0)::bigint
FROM updated_comment"#,
    )
    .bind(comment_id)
    .bind(issue_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some((row.try_get(0)?, row.try_get(1)?)))
}
