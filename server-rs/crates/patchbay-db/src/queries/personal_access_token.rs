//! Typed SQL queries for personal_access_token records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn create_personal_access_token(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
    name: &str,
    token_hash: &str,
    token_prefix: &str,
    expires_at: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<PersonalAccessToken>> {
    let row = sqlx::query(
        r#"INSERT INTO personal_access_token (user_id, name, token_hash, token_prefix, expires_at)
VALUES ($1, $2, $3, $4, $5)
RETURNING id, user_id, name, token_hash, token_prefix, expires_at, last_used_at, revoked, created_at"#
    )
        .bind(user_id)
        .bind(name)
        .bind(token_hash)
        .bind(token_prefix)
        .bind(expires_at)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PersonalAccessToken {
        id: row.try_get(0)?,
        user_id: row.try_get(1)?,
        name: row.try_get(2)?,
        token_hash: row.try_get(3)?,
        token_prefix: row.try_get(4)?,
        expires_at: row.try_get(5)?,
        last_used_at: row.try_get(6)?,
        revoked: row.try_get(7)?,
        created_at: row.try_get(8)?,
    }))
}

pub async fn extend_personal_access_token_expiry(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    new_expires_at: Option<DateTime<Utc>>,
    id: Uuid,
    renew_threshold_at: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<Option<DateTime<Utc>>>> {
    let row = sqlx::query(
        r#"UPDATE personal_access_token
SET expires_at = $1
WHERE id = $2
  AND revoked = FALSE
  AND expires_at IS NOT NULL
  AND expires_at > now()
  AND expires_at <= $3
RETURNING expires_at"#,
    )
    .bind(new_expires_at)
    .bind(id)
    .bind(renew_threshold_at)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn get_personal_access_token_by_hash(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
) -> anyhow::Result<Option<PersonalAccessToken>> {
    let row = sqlx::query(
        r#"SELECT id, user_id, name, token_hash, token_prefix, expires_at, last_used_at, revoked, created_at FROM personal_access_token
WHERE token_hash = $1
  AND revoked = FALSE
  AND (expires_at IS NULL OR expires_at > now())"#
    )
        .bind(token_hash)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(PersonalAccessToken {
        id: row.try_get(0)?,
        user_id: row.try_get(1)?,
        name: row.try_get(2)?,
        token_hash: row.try_get(3)?,
        token_prefix: row.try_get(4)?,
        expires_at: row.try_get(5)?,
        last_used_at: row.try_get(6)?,
        revoked: row.try_get(7)?,
        created_at: row.try_get(8)?,
    }))
}

pub async fn list_personal_access_tokens_by_user(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: Uuid,
) -> anyhow::Result<Vec<PersonalAccessToken>> {
    let rows = sqlx::query(
        r#"SELECT id, user_id, name, token_hash, token_prefix, expires_at, last_used_at, revoked, created_at FROM personal_access_token
WHERE user_id = $1
  AND revoked = FALSE
ORDER BY created_at DESC"#
    )
        .bind(user_id)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(PersonalAccessToken {
            id: row.try_get(0)?,
            user_id: row.try_get(1)?,
            name: row.try_get(2)?,
            token_hash: row.try_get(3)?,
            token_prefix: row.try_get(4)?,
            expires_at: row.try_get(5)?,
            last_used_at: row.try_get(6)?,
            revoked: row.try_get(7)?,
            created_at: row.try_get(8)?,
        });
    }
    Ok(out)
}

pub async fn revoke_personal_access_token(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    user_id: Uuid,
) -> anyhow::Result<Option<String>> {
    let row = sqlx::query(
        r#"UPDATE personal_access_token
SET revoked = TRUE
WHERE id = $1 AND user_id = $2
RETURNING token_hash"#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn update_personal_access_token_last_used(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE personal_access_token
SET last_used_at = now()
WHERE id = $1"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}
