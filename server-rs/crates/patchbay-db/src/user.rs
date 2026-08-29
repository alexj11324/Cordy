//! User queries.
//!
//! Compile-time checked via `query_as!`: every statement is verified against
//! the live schema at build time (or `.sqlx` offline cache in CI), so any
//! schema drift fails the build.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

/// Persisted user model. JSON field names are part of the public API contract.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub is_guest: bool,
    pub name: String,
    pub email: String,
    pub avatar_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub onboarded_at: Option<DateTime<Utc>>,
    pub onboarding_questionnaire: Option<serde_json::Value>,
    pub cloud_waitlist_email: Option<String>,
    pub cloud_waitlist_reason: Option<String>,
    pub starter_content_state: Option<String>,
    pub language: Option<String>,
    pub profile_description: String,
    /// User-preferred IANA timezone for report rendering. NULL = browser-detected.
    pub timezone: Option<String>,
}

pub async fn get_user(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as!(
        User,
        r#"SELECT id, name, email, avatar_url, created_at, updated_at,
                  onboarded_at, onboarding_questionnaire, cloud_waitlist_email,
                  cloud_waitlist_reason, starter_content_state, language,
                  profile_description, timezone, is_guest
           FROM "user" WHERE id = $1"#,
        id
    )
    .fetch_optional(executor)
    .await
}

pub async fn get_user_by_email(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    email: &str,
) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as!(
        User,
        r#"SELECT id, name, email, avatar_url, created_at, updated_at,
                  onboarded_at, onboarding_questionnaire, cloud_waitlist_email,
                  cloud_waitlist_reason, starter_content_state, language,
                  profile_description, timezone, is_guest
           FROM "user" WHERE email = $1"#,
        email
    )
    .fetch_optional(executor)
    .await
}

/// Batch lookup from the GLOBAL user table (not gated on membership, so
/// departed members still render) — PB-4302 §9 N+1 avoidance.
pub async fn get_users_by_ids(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, String, String, Option<String>)>, sqlx::Error> {
    sqlx::query_as(r#"SELECT id, name, email, avatar_url FROM "user" WHERE id = ANY($1::uuid[])"#)
        .bind(ids)
        .fetch_all(executor)
        .await
}

pub async fn create_user(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    name: &str,
    email: &str,
    avatar_url: Option<&str>,
) -> Result<User, sqlx::Error> {
    sqlx::query_as!(
        User,
        r#"INSERT INTO "user" (name, email, avatar_url)
           VALUES ($1, $2, $3)
           RETURNING id, name, email, avatar_url, created_at, updated_at,
                     onboarded_at, onboarding_questionnaire, cloud_waitlist_email,
                     cloud_waitlist_reason, starter_content_state, language,
                     profile_description, timezone, is_guest"#,
        name,
        email,
        avatar_url
    )
    .fetch_one(executor)
    .await
}

pub async fn mark_user_onboarded(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> Result<User, sqlx::Error> {
    sqlx::query_as!(
        User,
        r#"UPDATE "user" SET
               onboarded_at = COALESCE(onboarded_at, now()),
               updated_at = now()
           WHERE id = $1
           RETURNING id, name, email, avatar_url, created_at, updated_at,
                     onboarded_at, onboarding_questionnaire, cloud_waitlist_email,
                     cloud_waitlist_reason, starter_content_state, language,
                     profile_description, timezone, is_guest"#,
        id
    )
    .fetch_one(executor)
    .await
}
