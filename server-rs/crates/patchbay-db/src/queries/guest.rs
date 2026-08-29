//! Persistence helpers for server-backed guest sessions.
//!
//! Raw bearer and transfer tokens never leave this module as persisted data;
//! callers pass SHA-256 hashes and only receive the associated ids/status.

use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct GuestSession {
    pub id: Uuid,
    pub user_id: Uuid,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct GuestTransfer {
    pub id: Uuid,
    pub guest_session_id: Uuid,
    pub guest_user_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub claimed_user_id: Option<Uuid>,
}

pub async fn create_guest_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    user_id: Uuid,
    token_hash: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO guest_session (id, user_id, token_hash)
           VALUES ($1, $2, $3)"#,
    )
    .bind(id)
    .bind(user_id)
    .bind(token_hash)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn find_active_by_token_hash(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
) -> anyhow::Result<Option<GuestSession>> {
    let row = sqlx::query(
        r#"SELECT id, user_id, status
           FROM guest_session
           WHERE token_hash = $1 AND status = 'active'"#,
    )
    .bind(token_hash)
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(GuestSession {
            id: row.try_get(0)?,
            user_id: row.try_get(1)?,
            status: row.try_get(2)?,
        })
    })
    .transpose()
}

pub async fn find_active_by_user_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
) -> anyhow::Result<Option<GuestSession>> {
    let row = sqlx::query(
        r#"SELECT id, user_id, status
           FROM guest_session
           WHERE user_id = $1 AND status = 'active'
           ORDER BY created_at DESC
           LIMIT 1"#,
    )
    .bind(user_id)
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(GuestSession {
            id: row.try_get(0)?,
            user_id: row.try_get(1)?,
            status: row.try_get(2)?,
        })
    })
    .transpose()
}

pub async fn lock_by_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<GuestSession>> {
    let row = sqlx::query(
        r#"SELECT id, user_id, status
           FROM guest_session
           WHERE id = $1
           FOR UPDATE"#,
    )
    .bind(id)
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(GuestSession {
            id: row.try_get(0)?,
            user_id: row.try_get(1)?,
            status: row.try_get(2)?,
        })
    })
    .transpose()
}

pub async fn create_transfer(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    guest_session_id: Uuid,
    guest_user_id: Uuid,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO guest_session_transfer
             (id, guest_session_id, guest_user_id, token_hash, expires_at)
           VALUES ($1, $2, $3, $4, $5)"#,
    )
    .bind(id)
    .bind(guest_session_id)
    .bind(guest_user_id)
    .bind(token_hash)
    .bind(expires_at)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn lock_transfer_by_hash(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
) -> anyhow::Result<Option<GuestTransfer>> {
    let row = sqlx::query(
        r#"SELECT id, guest_session_id, guest_user_id, expires_at,
                  consumed_at, claimed_user_id
           FROM guest_session_transfer
           WHERE token_hash = $1
           FOR UPDATE"#,
    )
    .bind(token_hash)
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(GuestTransfer {
            id: row.try_get(0)?,
            guest_session_id: row.try_get(1)?,
            guest_user_id: row.try_get(2)?,
            expires_at: row.try_get(3)?,
            consumed_at: row.try_get(4)?,
            claimed_user_id: row.try_get(5)?,
        })
    })
    .transpose()
}

pub async fn consume_transfer(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    claimed_user_id: Uuid,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE guest_session_transfer
           SET consumed_at = now(), claimed_user_id = $2
           WHERE id = $1 AND consumed_at IS NULL"#,
    )
    .bind(id)
    .bind(claimed_user_id)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn claim_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    claimed_by: Uuid,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE guest_session
           SET status = 'claimed', claimed_at = now(), claimed_by = $2
           WHERE id = $1 AND status = 'active'"#,
    )
    .bind(id)
    .bind(claimed_by)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}
