//! Persistence helpers for server-backed guest sessions.
//!
//! Raw bearer tokens never leave this module as persisted data; callers pass
//! SHA-256 hashes and only receive the associated session identity.

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
    let row = sqlx::query(
        r#"SELECT user_id
           FROM guest_session
           WHERE token_hash = $1 AND status = 'active'"#,
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
