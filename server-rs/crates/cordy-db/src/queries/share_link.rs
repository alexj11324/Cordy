//! Port of server/pkg/db/queries/share_link.sql (generated share_link.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn claim_share_link_by_code(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    code: &str,
) -> anyhow::Result<Option<WorkspaceShareLink>> {
    let row = sqlx::query(
        r#"UPDATE workspace_share_link
SET use_count = use_count + 1
WHERE code = $1
  AND is_active = true
  AND (expires_at IS NULL OR expires_at > now())
  AND (max_uses IS NULL OR use_count < max_uses)
RETURNING id, workspace_id, code, created_by, role, expires_at, max_uses, use_count, is_active, created_at"#
    )
        .bind(code)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WorkspaceShareLink {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        code: row.try_get(2)?,
        created_by: row.try_get(3)?,
        role: row.try_get(4)?,
        expires_at: row.try_get(5)?,
        max_uses: row.try_get(6)?,
        use_count: row.try_get(7)?,
        is_active: row.try_get(8)?,
        created_at: row.try_get(9)?,
    }))
}

pub async fn create_share_link(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    code: &str,
    created_by: Uuid,
    role: &str,
    expires_at: Option<DateTime<Utc>>,
    max_uses: Option<i32>,
) -> anyhow::Result<Option<WorkspaceShareLink>> {
    let row = sqlx::query(
        r#"INSERT INTO workspace_share_link (workspace_id, code, created_by, role, expires_at, max_uses)
VALUES ($1, $2, $3, $4, $5, $6)
RETURNING id, workspace_id, code, created_by, role, expires_at, max_uses, use_count, is_active, created_at"#
    )
        .bind(workspace_id)
        .bind(code)
        .bind(created_by)
        .bind(role)
        .bind(expires_at)
        .bind(max_uses)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WorkspaceShareLink {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        code: row.try_get(2)?,
        created_by: row.try_get(3)?,
        role: row.try_get(4)?,
        expires_at: row.try_get(5)?,
        max_uses: row.try_get(6)?,
        use_count: row.try_get(7)?,
        is_active: row.try_get(8)?,
        created_at: row.try_get(9)?,
    }))
}

pub async fn deactivate_workspace_share_links(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE workspace_share_link
SET is_active = false
WHERE workspace_id = $1 AND is_active = true"#,
    )
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetShareLinkInfoByCodeRow {
    pub role: String,
    pub workspace_name: String,
    pub workspace_slug: String,
    pub creator_name: String,
}

pub async fn get_share_link_info_by_code(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    code: &str,
) -> anyhow::Result<Option<GetShareLinkInfoByCodeRow>> {
    let row = sqlx::query(
        r#"SELECT wsl.role,
       w.name  AS workspace_name,
       w.slug  AS workspace_slug,
       u.name  AS creator_name
FROM workspace_share_link wsl
JOIN workspace w ON w.id = wsl.workspace_id
JOIN "user" u ON u.id = wsl.created_by
WHERE wsl.code = $1 AND wsl.is_active = true
  AND (wsl.expires_at IS NULL OR wsl.expires_at > now())
  AND (wsl.max_uses IS NULL OR wsl.use_count < wsl.max_uses)"#,
    )
    .bind(code)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetShareLinkInfoByCodeRow {
        role: row.try_get(0)?,
        workspace_name: row.try_get(1)?,
        workspace_slug: row.try_get(2)?,
        creator_name: row.try_get(3)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListShareLinksByWorkspaceRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub code: String,
    pub created_by: Option<Uuid>,
    pub role: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: Option<i32>,
    pub use_count: i32,
    pub is_active: bool,
    pub created_at: Option<DateTime<Utc>>,
    pub creator_name: String,
    pub creator_email: String,
}

pub async fn list_share_links_by_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<ListShareLinksByWorkspaceRow>> {
    let rows = sqlx::query(
        r#"SELECT wsl.id, wsl.workspace_id, wsl.code, wsl.created_by, wsl.role, wsl.expires_at, wsl.max_uses, wsl.use_count, wsl.is_active, wsl.created_at,
       u.name  AS creator_name,
       u.email AS creator_email
FROM workspace_share_link wsl
JOIN "user" u ON u.id = wsl.created_by
WHERE wsl.workspace_id = $1 AND wsl.is_active = true
ORDER BY wsl.created_at DESC"#
    )
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListShareLinksByWorkspaceRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            code: row.try_get(2)?,
            created_by: row.try_get(3)?,
            role: row.try_get(4)?,
            expires_at: row.try_get(5)?,
            max_uses: row.try_get(6)?,
            use_count: row.try_get(7)?,
            is_active: row.try_get(8)?,
            created_at: row.try_get(9)?,
            creator_name: row.try_get(10)?,
            creator_email: row.try_get(11)?,
        });
    }
    Ok(out)
}

pub async fn revoke_share_link(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE workspace_share_link
SET is_active = false
WHERE id = $1 AND workspace_id = $2"#,
    )
    .bind(id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}
