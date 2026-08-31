//! Persistence helpers for server-backed guest sessions.
//!
//! Raw bearer tokens never leave this module as persisted data; callers pass
//! SHA-256 hashes and only receive the associated session identity.

use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct GuestSession {
    pub user_id: Uuid,
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
    // A bootstrap guest that outlives its five-minute OAuth attempt is no
    // longer useful. Revoke it lazily on the next auth lookup so an abandoned
    // browser flow cannot keep a bearer live indefinitely, including after a
    // process restart where an in-memory timer would be lost.
    let row = sqlx::query(
        r#"WITH revoked AS (
               UPDATE guest_session
               SET status = 'revoked'
               WHERE token_hash = $1
                 AND status = 'active'
                 AND handoff_expires_at IS NOT NULL
                 AND handoff_expires_at <= now()
               RETURNING token_hash
           )
           SELECT user_id
           FROM guest_session
           WHERE token_hash = $1
             AND status = 'active'
             AND (handoff_expires_at IS NULL OR handoff_expires_at > now())"#,
    )
    .bind(token_hash)
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(GuestSession {
            user_id: row.try_get(0)?,
        })
    })
    .transpose()
}

/// Arms the short expiry used only by a Desktop OAuth bootstrap guest. A
/// claimed or revoked session cannot be re-armed, and ordinary guest sessions
/// never call this function.
pub async fn set_handoff_expiry_by_token_hash(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"UPDATE guest_session
           SET handoff_expires_at = $2
           WHERE token_hash = $1 AND status = 'active'"#,
    )
    .bind(token_hash)
    .bind(expires_at)
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Revokes the active session for a bearer token without ever persisting the
/// raw token. A guest user has one session token, so matching by its hash also
/// avoids revoking a different session if the schema gains multi-session
/// support later.
pub async fn revoke_active_by_token_hash(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"UPDATE guest_session
           SET status = 'revoked'
           WHERE token_hash = $1 AND status = 'active'"#,
    )
    .bind(token_hash)
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Marks the bootstrap guest session as claimed by the formal account created
/// by a completed desktop OAuth handoff. The token stops authenticating as soon
/// as it is claimed, while the row remains available for audit.
pub async fn claim_active_by_token_hash(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
    claimed_by: Uuid,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"UPDATE guest_session
           SET status = 'claimed', claimed_at = now(), claimed_by = $2
           WHERE token_hash = $1 AND status = 'active'"#,
    )
    .bind(token_hash)
    .bind(claimed_by)
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}
