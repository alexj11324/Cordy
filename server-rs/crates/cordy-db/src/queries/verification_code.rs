//! Port of server/pkg/db/queries/verification_code.sql (generated verification_code.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn create_verification_code(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    email: &str,
    code: &str,
    expires_at: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<VerificationCode>> {
    let row = sqlx::query(
        r#"INSERT INTO verification_code (email, code, expires_at)
VALUES ($1, $2, $3)
RETURNING id, email, code, expires_at, used, created_at, attempts"#,
    )
    .bind(email)
    .bind(code)
    .bind(expires_at)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(VerificationCode {
        id: row.try_get(0)?,
        email: row.try_get(1)?,
        code: row.try_get(2)?,
        expires_at: row.try_get(3)?,
        used: row.try_get(4)?,
        created_at: row.try_get(5)?,
        attempts: row.try_get(6)?,
    }))
}

pub async fn delete_expired_verification_codes(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM verification_code
WHERE expires_at < now() - interval '1 hour'"#,
    )
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn get_latest_code_by_email(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    email: &str,
) -> anyhow::Result<Option<VerificationCode>> {
    let row = sqlx::query(
        r#"SELECT id, email, code, expires_at, used, created_at, attempts FROM verification_code
WHERE email = $1
ORDER BY created_at DESC
LIMIT 1"#,
    )
    .bind(email)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(VerificationCode {
        id: row.try_get(0)?,
        email: row.try_get(1)?,
        code: row.try_get(2)?,
        expires_at: row.try_get(3)?,
        used: row.try_get(4)?,
        created_at: row.try_get(5)?,
        attempts: row.try_get(6)?,
    }))
}

pub async fn get_latest_verification_code(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    email: &str,
) -> anyhow::Result<Option<VerificationCode>> {
    let row = sqlx::query(
        r#"SELECT id, email, code, expires_at, used, created_at, attempts FROM verification_code
WHERE email = $1
  AND used = FALSE
  AND expires_at > now()
  AND attempts < 5
ORDER BY created_at DESC
LIMIT 1"#,
    )
    .bind(email)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(VerificationCode {
        id: row.try_get(0)?,
        email: row.try_get(1)?,
        code: row.try_get(2)?,
        expires_at: row.try_get(3)?,
        used: row.try_get(4)?,
        created_at: row.try_get(5)?,
        attempts: row.try_get(6)?,
    }))
}

pub async fn increment_verification_code_attempts(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE verification_code
SET attempts = attempts + 1
WHERE id = $1"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn mark_verification_code_used(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE verification_code
SET used = TRUE
WHERE id = $1"#,
    )
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}
