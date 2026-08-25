//! Port of server/pkg/db/queries/member.sql (generated member.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn create_member(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    role: &str,
) -> anyhow::Result<Option<Member>> {
    let row = sqlx::query(
        r#"INSERT INTO member (workspace_id, user_id, role)
VALUES ($1, $2, $3)
RETURNING id, workspace_id, user_id, role, created_at"#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(role)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Member {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        user_id: row.try_get(2)?,
        role: row.try_get(3)?,
        created_at: row.try_get(4)?,
    }))
}

pub async fn delete_member(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(r#"DELETE FROM member WHERE id = $1"#)
        .bind(id)
        .execute(executor)
        .await?;
    Ok(r.rows_affected())
}

pub async fn get_member(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<Member>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, user_id, role, created_at FROM member
WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Member {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        user_id: row.try_get(2)?,
        role: row.try_get(3)?,
        created_at: row.try_get(4)?,
    }))
}

pub async fn lock_member_by_user_and_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Member>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, user_id, role, created_at FROM member
WHERE user_id = $1 AND workspace_id = $2
FOR UPDATE"#,
    )
    .bind(user_id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Member {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        user_id: row.try_get(2)?,
        role: row.try_get(3)?,
        created_at: row.try_get(4)?,
    }))
}

pub async fn get_member_by_user_and_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<Member>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, user_id, role, created_at FROM member
WHERE user_id = $1 AND workspace_id = $2"#,
    )
    .bind(user_id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Member {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        user_id: row.try_get(2)?,
        role: row.try_get(3)?,
        created_at: row.try_get(4)?,
    }))
}

pub async fn list_members(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Member>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, user_id, role, created_at FROM member
WHERE workspace_id = $1
ORDER BY created_at ASC"#,
    )
    .bind(workspace_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Member {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            user_id: row.try_get(2)?,
            role: row.try_get(3)?,
            created_at: row.try_get(4)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListMembersWithUserRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub role: String,
    pub created_at: Option<DateTime<Utc>>,
    pub user_name: String,
    pub user_email: String,
    pub user_avatar_url: Option<String>,
}

pub async fn list_members_with_user(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<ListMembersWithUserRow>> {
    let rows = sqlx::query(
        r#"SELECT m.id, m.workspace_id, m.user_id, m.role, m.created_at,
       u.name as user_name, u.email as user_email, u.avatar_url as user_avatar_url
FROM member m
JOIN "user" u ON u.id = m.user_id
WHERE m.workspace_id = $1
ORDER BY m.created_at ASC"#,
    )
    .bind(workspace_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListMembersWithUserRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            user_id: row.try_get(2)?,
            role: row.try_get(3)?,
            created_at: row.try_get(4)?,
            user_name: row.try_get(5)?,
            user_email: row.try_get(6)?,
            user_avatar_url: row.try_get(7)?,
        });
    }
    Ok(out)
}

pub async fn update_member_role(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    role: &str,
) -> anyhow::Result<Option<Member>> {
    let row = sqlx::query(
        r#"UPDATE member SET role = $2
WHERE id = $1
RETURNING id, workspace_id, user_id, role, created_at"#,
    )
    .bind(id)
    .bind(role)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Member {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        user_id: row.try_get(2)?,
        role: row.try_get(3)?,
        created_at: row.try_get(4)?,
    }))
}
