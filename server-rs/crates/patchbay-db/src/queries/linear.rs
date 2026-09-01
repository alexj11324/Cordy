//! SQL queries for the Linear installation foundation.
//!
//! This module owns only persistence. OAuth protocol handling and Webhook
//! verification remain in `patchbay-handler`, so database code cannot
//! accidentally accept an unverified provider payload.

use crate::models::{LinearConnection, LinearOAuthState, LinearSyncInbox};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

pub struct OAuthStateInput<'a> {
    pub id: Uuid,
    pub state_hash: &'a str,
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub code_verifier_encrypted: &'a str,
    pub redirect_uri: &'a str,
    pub expires_at: DateTime<Utc>,
}

pub async fn insert_oauth_state(
    executor: impl Executor<'_, Database = Postgres>,
    state: &OAuthStateInput<'_>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO linear_oauth_state
           (id, state_hash, workspace_id, user_id, code_verifier_encrypted,
            redirect_uri, expires_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
    )
    .bind(state.id)
    .bind(state.state_hash)
    .bind(state.workspace_id)
    .bind(state.user_id)
    .bind(state.code_verifier_encrypted)
    .bind(state.redirect_uri)
    .bind(state.expires_at)
    .execute(executor)
    .await?;
    Ok(())
}

/// Atomically consumes an unexpired state. The unique state-hash index makes
/// replay attempts deterministic even when callbacks race.
pub async fn consume_oauth_state(
    executor: impl Executor<'_, Database = Postgres>,
    state_hash: &str,
) -> anyhow::Result<Option<LinearOAuthState>> {
    Ok(sqlx::query_as::<_, LinearOAuthState>(
        r#"UPDATE linear_oauth_state
           SET consumed_at = now()
           WHERE state_hash = $1
             AND consumed_at IS NULL
             AND expires_at > now()
           RETURNING id, state_hash, workspace_id, user_id,
                     code_verifier_encrypted, redirect_uri, expires_at,
                     consumed_at, created_at"#,
    )
    .bind(state_hash)
    .fetch_optional(executor)
    .await?)
}

/// Reclaims a bounded batch of PKCE state rows that can no longer be used.
/// The expired branch is shaped to use `idx_linear_oauth_state_expiry`; the
/// consumed branch keeps successful and denied OAuth attempts from retaining
/// encrypted verifiers indefinitely.
pub async fn cleanup_oauth_states(
    executor: impl Executor<'_, Database = Postgres>,
    limit: i64,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"WITH expired AS (
    SELECT id
    FROM linear_oauth_state
    WHERE consumed_at IS NULL AND expires_at <= now()
    ORDER BY expires_at, id
    LIMIT $1
), consumed AS (
    SELECT id
    FROM linear_oauth_state
    WHERE consumed_at IS NOT NULL
    ORDER BY consumed_at, id
    LIMIT $1
), candidates AS (
    SELECT id FROM expired
    UNION ALL
    SELECT id FROM consumed
    LIMIT $1
)
DELETE FROM linear_oauth_state
WHERE id IN (SELECT id FROM candidates)"#,
    )
    .bind(limit.clamp(1, 500))
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

pub struct LinearConnectionInput<'a> {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub organization_id: &'a str,
    pub organization_name: &'a str,
    pub actor_id: &'a str,
    pub access_token_encrypted: &'a str,
    pub refresh_token_encrypted: &'a str,
    pub token_expires_at: DateTime<Utc>,
    pub scopes: &'a Value,
    pub created_by_id: Uuid,
}

pub async fn upsert_connection(
    executor: impl Executor<'_, Database = Postgres>,
    connection: &LinearConnectionInput<'_>,
) -> anyhow::Result<LinearConnection> {
    Ok(sqlx::query_as::<_, LinearConnection>(
        r#"INSERT INTO linear_connection
           (id, workspace_id, organization_id, organization_name, actor_id,
            access_token_encrypted, refresh_token_encrypted, token_expires_at,
            scopes, created_by_id)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
           ON CONFLICT (workspace_id) DO UPDATE SET
             organization_id = EXCLUDED.organization_id,
             organization_name = EXCLUDED.organization_name,
             actor_id = EXCLUDED.actor_id,
             access_token_encrypted = EXCLUDED.access_token_encrypted,
             refresh_token_encrypted = EXCLUDED.refresh_token_encrypted,
             token_expires_at = EXCLUDED.token_expires_at,
             scopes = EXCLUDED.scopes,
             webhook_id = CASE
               WHEN linear_connection.organization_id = EXCLUDED.organization_id
               THEN linear_connection.webhook_id
               ELSE NULL
             END,
             status = 'active',
             last_error = NULL,
             updated_at = now()
           RETURNING id, workspace_id, organization_id, organization_name,
                     actor_id, access_token_encrypted, refresh_token_encrypted,
                     token_expires_at, scopes, webhook_id, status,
                     last_success_at, last_error, created_by_id, created_at,
                     updated_at"#,
    )
    .bind(connection.id)
    .bind(connection.workspace_id)
    .bind(connection.organization_id)
    .bind(connection.organization_name)
    .bind(connection.actor_id)
    .bind(connection.access_token_encrypted)
    .bind(connection.refresh_token_encrypted)
    .bind(connection.token_expires_at)
    .bind(connection.scopes)
    .bind(connection.created_by_id)
    .fetch_one(executor)
    .await?)
}

pub async fn get_connection_for_workspace(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Option<LinearConnection>> {
    Ok(sqlx::query_as::<_, LinearConnection>(
        r#"SELECT id, workspace_id, organization_id, organization_name,
                  actor_id, access_token_encrypted, refresh_token_encrypted,
                  token_expires_at, scopes, webhook_id, status,
                  last_success_at, last_error, created_by_id, created_at,
                  updated_at
           FROM linear_connection
           WHERE workspace_id = $1"#,
    )
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?)
}

pub async fn get_connection_for_workspace_for_update(
    executor: &mut sqlx::PgConnection,
    workspace_id: Uuid,
) -> anyhow::Result<Option<LinearConnection>> {
    Ok(sqlx::query_as::<_, LinearConnection>(
        r#"SELECT id, workspace_id, organization_id, organization_name,
                  actor_id, access_token_encrypted, refresh_token_encrypted,
                  token_expires_at, scopes, webhook_id, status,
                  last_success_at, last_error, created_by_id, created_at,
                  updated_at
           FROM linear_connection
           WHERE workspace_id = $1
           FOR UPDATE"#,
    )
    .bind(workspace_id)
    .fetch_optional(&mut *executor)
    .await?)
}

pub async fn get_connection_by_id(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    id: Uuid,
) -> anyhow::Result<Option<LinearConnection>> {
    Ok(sqlx::query_as::<_, LinearConnection>(
        r#"SELECT id, workspace_id, organization_id, organization_name,
                  actor_id, access_token_encrypted, refresh_token_encrypted,
                  token_expires_at, scopes, webhook_id, status,
                  last_success_at, last_error, created_by_id, created_at,
                  updated_at
           FROM linear_connection
           WHERE workspace_id = $1 AND id = $2"#,
    )
    .bind(workspace_id)
    .bind(id)
    .fetch_optional(executor)
    .await?)
}

pub async fn get_connection_for_update(
    executor: &mut sqlx::PgConnection,
    id: Uuid,
) -> anyhow::Result<Option<LinearConnection>> {
    Ok(sqlx::query_as::<_, LinearConnection>(
        r#"SELECT id, workspace_id, organization_id, organization_name,
                  actor_id, access_token_encrypted, refresh_token_encrypted,
                  token_expires_at, scopes, webhook_id, status,
                  last_success_at, last_error, created_by_id, created_at,
                  updated_at
           FROM linear_connection
           WHERE id = $1
           FOR UPDATE"#,
    )
    .bind(id)
    .fetch_optional(&mut *executor)
    .await?)
}

/// Returns exact webhook bindings first. Unbound connections are considered
/// only when no exact binding exists, while holding the selected rows for the
/// caller's routing/bind transaction.
pub async fn find_connections_for_webhook(
    executor: &mut sqlx::PgConnection,
    organization_id: &str,
    webhook_id: &str,
) -> anyhow::Result<Vec<LinearConnection>> {
    let exact = sqlx::query_as::<_, LinearConnection>(
        r#"SELECT id, workspace_id, organization_id, organization_name,
                  actor_id, access_token_encrypted, refresh_token_encrypted,
                  token_expires_at, scopes, webhook_id, status,
                  last_success_at, last_error, created_by_id, created_at,
                  updated_at
           FROM linear_connection
           WHERE organization_id = $1
             AND status <> 'revoked'
             AND webhook_id = $2
           FOR UPDATE"#,
    )
    .bind(organization_id)
    .bind(webhook_id)
    .fetch_all(&mut *executor)
    .await?;
    if !exact.is_empty() {
        return Ok(exact);
    }

    Ok(sqlx::query_as::<_, LinearConnection>(
        r#"SELECT id, workspace_id, organization_id, organization_name,
                  actor_id, access_token_encrypted, refresh_token_encrypted,
                  token_expires_at, scopes, webhook_id, status,
                  last_success_at, last_error, created_by_id, created_at,
                  updated_at
           FROM linear_connection
           WHERE organization_id = $1
             AND status <> 'revoked'
             AND webhook_id IS NULL
           FOR UPDATE"#,
    )
    .bind(organization_id)
    .fetch_all(&mut *executor)
    .await?)
}

pub async fn bind_webhook(
    executor: &mut sqlx::PgConnection,
    connection_id: Uuid,
    webhook_id: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_connection
           SET webhook_id = $2, updated_at = now()
           WHERE id = $1 AND webhook_id IS NULL"#,
    )
    .bind(connection_id)
    .bind(webhook_id)
    .execute(&mut *executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn mark_webhook_accepted(
    executor: &mut sqlx::PgConnection,
    connection_id: Uuid,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"UPDATE linear_connection
           SET last_success_at = now(), last_error = NULL, updated_at = now()
           WHERE id = $1"#,
    )
    .bind(connection_id)
    .execute(&mut *executor)
    .await?;
    Ok(())
}

pub async fn mark_reauthorization_required(
    executor: &mut sqlx::PgConnection,
    connection_id: Uuid,
    error: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"UPDATE linear_connection
           SET status = 'reauthorization_required', last_error = $2,
               updated_at = now()
           WHERE id = $1"#,
    )
    .bind(connection_id)
    .bind(error)
    .execute(&mut *executor)
    .await?;
    Ok(())
}

pub async fn mark_revoked(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_connection
           SET status = 'revoked', last_error = NULL, updated_at = now()
           WHERE workspace_id = $1 AND id = $2 AND status <> 'revoked'"#,
    )
    .bind(workspace_id)
    .bind(connection_id)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn update_tokens(
    executor: &mut sqlx::PgConnection,
    connection_id: Uuid,
    access_token_encrypted: &str,
    refresh_token_encrypted: &str,
    token_expires_at: DateTime<Utc>,
    scopes: &Value,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"UPDATE linear_connection
           SET access_token_encrypted = $2,
               refresh_token_encrypted = $3,
               token_expires_at = $4,
               scopes = $5,
               status = 'active',
               last_error = NULL,
               updated_at = now()
           WHERE id = $1"#,
    )
    .bind(connection_id)
    .bind(access_token_encrypted)
    .bind(refresh_token_encrypted)
    .bind(token_expires_at)
    .bind(scopes)
    .execute(&mut *executor)
    .await?;
    Ok(())
}

pub async fn insert_sync_inbox(
    executor: &mut sqlx::PgConnection,
    id: Uuid,
    connection_id: Uuid,
    delivery_id: &str,
    event_type: &str,
    payload: &Value,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        r#"INSERT INTO linear_sync_inbox
           (id, connection_id, delivery_id, event_type, payload)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (connection_id, delivery_id) DO NOTHING
           RETURNING id"#,
    )
    .bind(id)
    .bind(connection_id)
    .bind(delivery_id)
    .bind(event_type)
    .bind(payload)
    .fetch_optional(&mut *executor)
    .await?;
    Ok(row.is_some())
}

pub async fn list_inbox(
    executor: impl Executor<'_, Database = Postgres>,
    connection_id: Uuid,
    limit: i64,
) -> anyhow::Result<Vec<LinearSyncInbox>> {
    Ok(sqlx::query_as::<_, LinearSyncInbox>(
        r#"SELECT id, connection_id, delivery_id, event_type, payload,
                  received_at, processed_at, attempts, last_error
           FROM linear_sync_inbox
           WHERE connection_id = $1
           ORDER BY received_at, id
           LIMIT $2"#,
    )
    .bind(connection_id)
    .bind(limit.clamp(1, 500))
    .fetch_all(executor)
    .await?)
}
