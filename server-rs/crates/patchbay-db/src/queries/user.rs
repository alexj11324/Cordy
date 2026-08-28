//! Typed SQL queries for user records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn create_user(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    name: &str,
    email: &str,
    avatar_url: Option<&str>,
) -> anyhow::Result<Option<User>> {
    let row = sqlx::query(
        r#"INSERT INTO "user" (name, email, avatar_url)
VALUES ($1, $2, $3)
RETURNING id, name, email, avatar_url, created_at, updated_at, onboarded_at, onboarding_questionnaire, cloud_waitlist_email, cloud_waitlist_reason, starter_content_state, language, profile_description, timezone"#
    )
        .bind(name)
        .bind(email)
        .bind(avatar_url)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(User {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        email: row.try_get(2)?,
        avatar_url: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
        onboarded_at: row.try_get(6)?,
        onboarding_questionnaire: row.try_get(7)?,
        cloud_waitlist_email: row.try_get(8)?,
        cloud_waitlist_reason: row.try_get(9)?,
        starter_content_state: row.try_get(10)?,
        language: row.try_get(11)?,
        profile_description: row.try_get(12)?,
        timezone: row.try_get(13)?,
    }))
}

pub async fn get_user(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<User>> {
    let row = sqlx::query(
        r#"SELECT id, name, email, avatar_url, created_at, updated_at, onboarded_at, onboarding_questionnaire, cloud_waitlist_email, cloud_waitlist_reason, starter_content_state, language, profile_description, timezone FROM "user"
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(User {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        email: row.try_get(2)?,
        avatar_url: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
        onboarded_at: row.try_get(6)?,
        onboarding_questionnaire: row.try_get(7)?,
        cloud_waitlist_email: row.try_get(8)?,
        cloud_waitlist_reason: row.try_get(9)?,
        starter_content_state: row.try_get(10)?,
        language: row.try_get(11)?,
        profile_description: row.try_get(12)?,
        timezone: row.try_get(13)?,
    }))
}

pub async fn get_user_by_email(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    email: &str,
) -> anyhow::Result<Option<User>> {
    let row = sqlx::query(
        r#"SELECT id, name, email, avatar_url, created_at, updated_at, onboarded_at, onboarding_questionnaire, cloud_waitlist_email, cloud_waitlist_reason, starter_content_state, language, profile_description, timezone FROM "user"
WHERE email = $1"#
    )
        .bind(email)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(User {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        email: row.try_get(2)?,
        avatar_url: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
        onboarded_at: row.try_get(6)?,
        onboarding_questionnaire: row.try_get(7)?,
        cloud_waitlist_email: row.try_get(8)?,
        cloud_waitlist_reason: row.try_get(9)?,
        starter_content_state: row.try_get(10)?,
        language: row.try_get(11)?,
        profile_description: row.try_get(12)?,
        timezone: row.try_get(13)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetUsersByIDsRow {
    pub id: Option<Uuid>,
    pub name: String,
    pub email: String,
    pub avatar_url: Option<String>,
}

pub async fn get_users_by_i_ds(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    ids: Vec<Uuid>,
) -> anyhow::Result<Vec<GetUsersByIDsRow>> {
    let rows = sqlx::query(
        r#"SELECT id, name, email, avatar_url FROM "user"
WHERE id = ANY($1::uuid[])"#,
    )
    .bind(ids)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(GetUsersByIDsRow {
            id: row.try_get(0)?,
            name: row.try_get(1)?,
            email: row.try_get(2)?,
            avatar_url: row.try_get(3)?,
        });
    }
    Ok(out)
}

pub async fn join_cloud_waitlist(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    cloud_waitlist_email: Option<&str>,
    cloud_waitlist_reason: Option<&str>,
) -> anyhow::Result<Option<User>> {
    let row = sqlx::query(
        r#"UPDATE "user" SET
    cloud_waitlist_email = $2,
    cloud_waitlist_reason = $3,
    updated_at = now()
WHERE id = $1
RETURNING id, name, email, avatar_url, created_at, updated_at, onboarded_at, onboarding_questionnaire, cloud_waitlist_email, cloud_waitlist_reason, starter_content_state, language, profile_description, timezone"#
    )
        .bind(id)
        .bind(cloud_waitlist_email)
        .bind(cloud_waitlist_reason)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(User {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        email: row.try_get(2)?,
        avatar_url: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
        onboarded_at: row.try_get(6)?,
        onboarding_questionnaire: row.try_get(7)?,
        cloud_waitlist_email: row.try_get(8)?,
        cloud_waitlist_reason: row.try_get(9)?,
        starter_content_state: row.try_get(10)?,
        language: row.try_get(11)?,
        profile_description: row.try_get(12)?,
        timezone: row.try_get(13)?,
    }))
}

pub async fn mark_user_onboarded(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<User>> {
    let row = sqlx::query(
        r#"UPDATE "user" SET
    onboarded_at = COALESCE(onboarded_at, now()),
    updated_at = now()
WHERE id = $1
RETURNING id, name, email, avatar_url, created_at, updated_at, onboarded_at, onboarding_questionnaire, cloud_waitlist_email, cloud_waitlist_reason, starter_content_state, language, profile_description, timezone"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(User {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        email: row.try_get(2)?,
        avatar_url: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
        onboarded_at: row.try_get(6)?,
        onboarding_questionnaire: row.try_get(7)?,
        cloud_waitlist_email: row.try_get(8)?,
        cloud_waitlist_reason: row.try_get(9)?,
        starter_content_state: row.try_get(10)?,
        language: row.try_get(11)?,
        profile_description: row.try_get(12)?,
        timezone: row.try_get(13)?,
    }))
}

pub async fn claim_first_onboarding(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<User>> {
    let row = sqlx::query(
        r#"UPDATE "user" SET
    onboarded_at = now(),
    updated_at = now()
WHERE id = $1 AND onboarded_at IS NULL
RETURNING id, name, email, avatar_url, created_at, updated_at, onboarded_at, onboarding_questionnaire, cloud_waitlist_email, cloud_waitlist_reason, starter_content_state, language, profile_description, timezone"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(User {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        email: row.try_get(2)?,
        avatar_url: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
        onboarded_at: row.try_get(6)?,
        onboarding_questionnaire: row.try_get(7)?,
        cloud_waitlist_email: row.try_get(8)?,
        cloud_waitlist_reason: row.try_get(9)?,
        starter_content_state: row.try_get(10)?,
        language: row.try_get(11)?,
        profile_description: row.try_get(12)?,
        timezone: row.try_get(13)?,
    }))
}

pub async fn get_user_for_update(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<User>> {
    let row = sqlx::query(
        r#"SELECT id, name, email, avatar_url, created_at, updated_at, onboarded_at, onboarding_questionnaire, cloud_waitlist_email, cloud_waitlist_reason, starter_content_state, language, profile_description, timezone FROM "user"
WHERE id = $1
FOR UPDATE"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(User {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        email: row.try_get(2)?,
        avatar_url: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
        onboarded_at: row.try_get(6)?,
        onboarding_questionnaire: row.try_get(7)?,
        cloud_waitlist_email: row.try_get(8)?,
        cloud_waitlist_reason: row.try_get(9)?,
        starter_content_state: row.try_get(10)?,
        language: row.try_get(11)?,
        profile_description: row.try_get(12)?,
        timezone: row.try_get(13)?,
    }))
}

pub async fn patch_user_onboarding(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    questionnaire: Option<&serde_json::Value>,
    id: Uuid,
) -> anyhow::Result<Option<User>> {
    let row = sqlx::query(
        r#"UPDATE "user" SET
    onboarding_questionnaire = COALESCE($1, onboarding_questionnaire),
    updated_at = now()
WHERE id = $2
RETURNING id, name, email, avatar_url, created_at, updated_at, onboarded_at, onboarding_questionnaire, cloud_waitlist_email, cloud_waitlist_reason, starter_content_state, language, profile_description, timezone"#
    )
        .bind(questionnaire)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(User {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        email: row.try_get(2)?,
        avatar_url: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
        onboarded_at: row.try_get(6)?,
        onboarding_questionnaire: row.try_get(7)?,
        cloud_waitlist_email: row.try_get(8)?,
        cloud_waitlist_reason: row.try_get(9)?,
        starter_content_state: row.try_get(10)?,
        language: row.try_get(11)?,
        profile_description: row.try_get(12)?,
        timezone: row.try_get(13)?,
    }))
}

pub async fn set_starter_content_state(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    starter_content_state: Option<&str>,
) -> anyhow::Result<Option<User>> {
    let row = sqlx::query(
        r#"UPDATE "user" SET
    starter_content_state = $2,
    updated_at = now()
WHERE id = $1
RETURNING id, name, email, avatar_url, created_at, updated_at, onboarded_at, onboarding_questionnaire, cloud_waitlist_email, cloud_waitlist_reason, starter_content_state, language, profile_description, timezone"#
    )
        .bind(id)
        .bind(starter_content_state)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(User {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        email: row.try_get(2)?,
        avatar_url: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
        onboarded_at: row.try_get(6)?,
        onboarding_questionnaire: row.try_get(7)?,
        cloud_waitlist_email: row.try_get(8)?,
        cloud_waitlist_reason: row.try_get(9)?,
        starter_content_state: row.try_get(10)?,
        language: row.try_get(11)?,
        profile_description: row.try_get(12)?,
        timezone: row.try_get(13)?,
    }))
}

pub async fn update_user(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    name: &str,
    avatar_url: Option<&str>,
    language: Option<&str>,
    profile_description: Option<&str>,
    timezone: Option<&str>,
) -> anyhow::Result<Option<User>> {
    let row = sqlx::query(
        r#"UPDATE "user" SET
    name = COALESCE($2, name),
    avatar_url = COALESCE($3, avatar_url),
    language = COALESCE($4, language),
    profile_description = COALESCE($5, profile_description),
    timezone = CASE
        WHEN $6::text IS NULL THEN timezone
        WHEN $6::text = ''    THEN NULL
        ELSE $6::text
    END,
    updated_at = now()
WHERE id = $1
RETURNING id, name, email, avatar_url, created_at, updated_at, onboarded_at, onboarding_questionnaire, cloud_waitlist_email, cloud_waitlist_reason, starter_content_state, language, profile_description, timezone"#
    )
        .bind(id)
        .bind(name)
        .bind(avatar_url)
        .bind(language)
        .bind(profile_description)
        .bind(timezone)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(User {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        email: row.try_get(2)?,
        avatar_url: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
        onboarded_at: row.try_get(6)?,
        onboarding_questionnaire: row.try_get(7)?,
        cloud_waitlist_email: row.try_get(8)?,
        cloud_waitlist_reason: row.try_get(9)?,
        starter_content_state: row.try_get(10)?,
        language: row.try_get(11)?,
        profile_description: row.try_get(12)?,
        timezone: row.try_get(13)?,
    }))
}
