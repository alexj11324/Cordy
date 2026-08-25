//! Port of server/pkg/db/queries/invitation.sql (generated invitation.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

const ACCEPT_INVITATION_SQL: &str = r#"UPDATE workspace_invitation
SET status = 'accepted', updated_at = now()
WHERE id = $1 AND status = 'pending' AND expires_at > now()
RETURNING id, workspace_id, inviter_id, invitee_email, invitee_user_id, role, status, created_at, updated_at, expires_at"#;

pub async fn accept_invitation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<WorkspaceInvitation>> {
    let row = sqlx::query(ACCEPT_INVITATION_SQL)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WorkspaceInvitation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        inviter_id: row.try_get(2)?,
        invitee_email: row.try_get(3)?,
        invitee_user_id: row.try_get(4)?,
        role: row.try_get(5)?,
        status: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
        expires_at: row.try_get(9)?,
    }))
}

pub async fn create_invitation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    inviter_id: Uuid,
    invitee_email: &str,
    invitee_user_id: Option<Uuid>,
    role: &str,
) -> anyhow::Result<Option<WorkspaceInvitation>> {
    let row = sqlx::query(
        r#"INSERT INTO workspace_invitation (workspace_id, inviter_id, invitee_email, invitee_user_id, role)
VALUES ($1, $2, $3, $4, $5)
RETURNING id, workspace_id, inviter_id, invitee_email, invitee_user_id, role, status, created_at, updated_at, expires_at"#
    )
        .bind(workspace_id)
        .bind(inviter_id)
        .bind(invitee_email)
        .bind(invitee_user_id)
        .bind(role)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WorkspaceInvitation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        inviter_id: row.try_get(2)?,
        invitee_email: row.try_get(3)?,
        invitee_user_id: row.try_get(4)?,
        role: row.try_get(5)?,
        status: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
        expires_at: row.try_get(9)?,
    }))
}

pub async fn decline_invitation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<WorkspaceInvitation>> {
    let row = sqlx::query(
        r#"UPDATE workspace_invitation
SET status = 'declined', updated_at = now()
WHERE id = $1 AND status = 'pending'
RETURNING id, workspace_id, inviter_id, invitee_email, invitee_user_id, role, status, created_at, updated_at, expires_at"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WorkspaceInvitation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        inviter_id: row.try_get(2)?,
        invitee_email: row.try_get(3)?,
        invitee_user_id: row.try_get(4)?,
        role: row.try_get(5)?,
        status: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
        expires_at: row.try_get(9)?,
    }))
}

pub async fn expire_stale_pending_invitations(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    invitee_email: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE workspace_invitation
SET status = 'expired', updated_at = now()
WHERE workspace_id = $1
  AND invitee_email = $2
  AND status = 'pending'
  AND expires_at <= now()"#,
    )
    .bind(workspace_id)
    .bind(invitee_email)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn get_invitation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<WorkspaceInvitation>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, inviter_id, invitee_email, invitee_user_id, role, status, created_at, updated_at, expires_at FROM workspace_invitation
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WorkspaceInvitation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        inviter_id: row.try_get(2)?,
        invitee_email: row.try_get(3)?,
        invitee_user_id: row.try_get(4)?,
        role: row.try_get(5)?,
        status: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
        expires_at: row.try_get(9)?,
    }))
}

pub async fn get_pending_invitation_by_email(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    invitee_email: &str,
) -> anyhow::Result<Option<WorkspaceInvitation>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, inviter_id, invitee_email, invitee_user_id, role, status, created_at, updated_at, expires_at FROM workspace_invitation
WHERE workspace_id = $1 AND invitee_email = $2 AND status = 'pending' AND expires_at > now()"#
    )
        .bind(workspace_id)
        .bind(invitee_email)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WorkspaceInvitation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        inviter_id: row.try_get(2)?,
        invitee_email: row.try_get(3)?,
        invitee_user_id: row.try_get(4)?,
        role: row.try_get(5)?,
        status: row.try_get(6)?,
        created_at: row.try_get(7)?,
        updated_at: row.try_get(8)?,
        expires_at: row.try_get(9)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListPendingInvitationsByWorkspaceRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub inviter_id: Option<Uuid>,
    pub invitee_email: String,
    pub invitee_user_id: Option<Uuid>,
    pub role: String,
    pub status: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub inviter_name: String,
    pub inviter_email: String,
}

pub async fn list_pending_invitations_by_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<ListPendingInvitationsByWorkspaceRow>> {
    let rows = sqlx::query(
        r#"SELECT wi.id, wi.workspace_id, wi.inviter_id, wi.invitee_email, wi.invitee_user_id, wi.role, wi.status, wi.created_at, wi.updated_at, wi.expires_at,
       u.name  AS inviter_name,
       u.email AS inviter_email
FROM workspace_invitation wi
JOIN "user" u ON u.id = wi.inviter_id
WHERE wi.workspace_id = $1 AND wi.status = 'pending' AND wi.expires_at > now()
ORDER BY wi.created_at DESC"#
    )
        .bind(workspace_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListPendingInvitationsByWorkspaceRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            inviter_id: row.try_get(2)?,
            invitee_email: row.try_get(3)?,
            invitee_user_id: row.try_get(4)?,
            role: row.try_get(5)?,
            status: row.try_get(6)?,
            created_at: row.try_get(7)?,
            updated_at: row.try_get(8)?,
            expires_at: row.try_get(9)?,
            inviter_name: row.try_get(10)?,
            inviter_email: row.try_get(11)?,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListPendingInvitationsForUserRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub inviter_id: Option<Uuid>,
    pub invitee_email: String,
    pub invitee_user_id: Option<Uuid>,
    pub role: String,
    pub status: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub workspace_name: String,
    pub inviter_name: String,
    pub inviter_email: String,
}

pub async fn list_pending_invitations_for_user(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    invitee_user_id: Uuid,
    invitee_email: &str,
) -> anyhow::Result<Vec<ListPendingInvitationsForUserRow>> {
    let rows = sqlx::query(
        r#"SELECT wi.id, wi.workspace_id, wi.inviter_id, wi.invitee_email, wi.invitee_user_id, wi.role, wi.status, wi.created_at, wi.updated_at, wi.expires_at,
       w.name AS workspace_name,
       u.name AS inviter_name,
       u.email AS inviter_email
FROM workspace_invitation wi
JOIN workspace w ON w.id = wi.workspace_id
JOIN "user" u ON u.id = wi.inviter_id
WHERE wi.status = 'pending'
  AND (wi.invitee_user_id = $1 OR wi.invitee_email = $2)
  AND wi.expires_at > now()
ORDER BY wi.created_at DESC"#
    )
        .bind(invitee_user_id)
        .bind(invitee_email)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListPendingInvitationsForUserRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            inviter_id: row.try_get(2)?,
            invitee_email: row.try_get(3)?,
            invitee_user_id: row.try_get(4)?,
            role: row.try_get(5)?,
            status: row.try_get(6)?,
            created_at: row.try_get(7)?,
            updated_at: row.try_get(8)?,
            expires_at: row.try_get(9)?,
            workspace_name: row.try_get(10)?,
            inviter_name: row.try_get(11)?,
            inviter_email: row.try_get(12)?,
        });
    }
    Ok(out)
}

pub async fn revoke_invitation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM workspace_invitation
WHERE id = $1 AND status = 'pending'"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::ACCEPT_INVITATION_SQL;

    #[test]
    fn accept_invitation_rechecks_expiry_in_the_update() {
        assert!(
            ACCEPT_INVITATION_SQL.contains("expires_at > now()"),
            "{ACCEPT_INVITATION_SQL}"
        );
        assert!(ACCEPT_INVITATION_SQL.contains("status = 'pending'"));
    }
}
