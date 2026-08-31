//! SQL queries for the Linear installation foundation.
//!
//! This module owns only persistence. OAuth protocol handling and Webhook
//! verification remain in `patchbay-handler`, so database code cannot
//! accidentally accept an unverified provider payload.

use crate::models::{
    Issue, LinearConnection, LinearIssueLink, LinearMemberBinding, LinearOAuthState,
    LinearProjectBinding, LinearSyncConflict, LinearSyncInbox, LinearSyncOutbox,
};
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
        r#"WITH deleted_member_bindings AS (
               DELETE FROM linear_member_binding
               WHERE workspace_id = $2
                 AND connection_id IN (
                     SELECT id
                     FROM linear_connection
                     WHERE workspace_id = $2 AND organization_id <> $3
                 )
           ), changed_bindings AS (
               UPDATE linear_project_binding
               SET status = 'tombstone',
                   paused_at = COALESCE(paused_at, now()),
                   updated_at = now()
               WHERE workspace_id = $2
                 AND status <> 'tombstone'
                 AND EXISTS (
                     SELECT 1
                     FROM linear_connection
                     WHERE workspace_id = $2 AND organization_id <> $3
                 )
           )
           INSERT INTO linear_connection
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

pub async fn get_connection_by_id_unscoped(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<LinearConnection>> {
    Ok(sqlx::query_as::<_, LinearConnection>(
        r#"SELECT id, workspace_id, organization_id, organization_name,
                  actor_id, access_token_encrypted, refresh_token_encrypted,
                  token_expires_at, scopes, webhook_id, status,
                  last_success_at, last_error, created_by_id, created_at,
                  updated_at
           FROM linear_connection
           WHERE id = $1"#,
    )
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

fn member_binding_columns() -> &'static str {
    "id, workspace_id, connection_id, patchbay_user_id, linear_user_id, created_at, updated_at"
}

pub async fn list_linear_member_bindings(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
) -> anyhow::Result<Vec<LinearMemberBinding>> {
    let query = format!(
        "SELECT {columns} FROM linear_member_binding\
         WHERE workspace_id = $1 AND connection_id = $2\
         ORDER BY updated_at DESC, id DESC",
        columns = member_binding_columns(),
    );
    Ok(sqlx::query_as::<_, LinearMemberBinding>(&query)
        .bind(workspace_id)
        .bind(connection_id)
        .fetch_all(executor)
        .await?)
}

pub async fn get_linear_member_binding(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
    patchbay_user_id: Uuid,
) -> anyhow::Result<Option<LinearMemberBinding>> {
    let query = format!(
        "SELECT {columns} FROM linear_member_binding\
         WHERE workspace_id = $1 AND connection_id = $2 AND patchbay_user_id = $3",
        columns = member_binding_columns(),
    );
    Ok(sqlx::query_as::<_, LinearMemberBinding>(&query)
        .bind(workspace_id)
        .bind(connection_id)
        .bind(patchbay_user_id)
        .fetch_optional(executor)
        .await?)
}

pub async fn get_linear_member_binding_by_linear_user(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_user_id: &str,
) -> anyhow::Result<Option<LinearMemberBinding>> {
    let query = format!(
        "SELECT {columns} FROM linear_member_binding\
         WHERE workspace_id = $1 AND connection_id = $2 AND linear_user_id = $3",
        columns = member_binding_columns(),
    );
    Ok(sqlx::query_as::<_, LinearMemberBinding>(&query)
        .bind(workspace_id)
        .bind(connection_id)
        .bind(linear_user_id)
        .fetch_optional(executor)
        .await?)
}

pub async fn upsert_linear_member_binding(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    connection_id: Uuid,
    patchbay_user_id: Uuid,
    linear_user_id: &str,
) -> anyhow::Result<LinearMemberBinding> {
    let query = format!(
        "INSERT INTO linear_member_binding\
         (id, workspace_id, connection_id, patchbay_user_id, linear_user_id)\
         VALUES ($1, $2, $3, $4, $5)\
         ON CONFLICT (workspace_id, connection_id, patchbay_user_id) DO UPDATE\
         SET linear_user_id = EXCLUDED.linear_user_id, updated_at = now()\
         RETURNING {columns}",
        columns = member_binding_columns(),
    );
    Ok(sqlx::query_as::<_, LinearMemberBinding>(&query)
        .bind(id)
        .bind(workspace_id)
        .bind(connection_id)
        .bind(patchbay_user_id)
        .bind(linear_user_id)
        .fetch_one(executor)
        .await?)
}

pub async fn delete_linear_member_binding(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
    patchbay_user_id: Uuid,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"DELETE FROM linear_member_binding
           WHERE workspace_id = $1 AND connection_id = $2 AND patchbay_user_id = $3"#,
    )
    .bind(workspace_id)
    .bind(connection_id)
    .bind(patchbay_user_id)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
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
                  received_at, processed_at, attempts, last_error,
                  available_at, locked_by, locked_until, max_attempts,
                  dead_lettered_at
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

/// Claims pending Inbox rows with PostgreSQL row locks. The lease is the only
/// authority for completing or retrying a row; Notify remains merely a
/// low-latency hint for the worker.
pub async fn claim_sync_inbox(
    executor: &sqlx::PgPool,
    worker_id: &str,
    limit: i64,
    lease_seconds: i64,
    workspace_ids: Option<&[Uuid]>,
    include_issue_events: bool,
) -> anyhow::Result<Vec<LinearSyncInbox>> {
    Ok(sqlx::query_as::<_, LinearSyncInbox>(
        r#"WITH picked AS (
               SELECT id
               FROM linear_sync_inbox
               WHERE processed_at IS NULL
                 AND dead_lettered_at IS NULL
                 AND available_at <= now()
                 AND attempts < max_attempts
                 AND (locked_until IS NULL OR locked_until < now())
                 AND (
                     ($4::uuid[] IS NULL
                      OR EXISTS (
                          SELECT 1
                          FROM linear_connection
                          WHERE linear_connection.id = linear_sync_inbox.connection_id
                            AND linear_connection.workspace_id = ANY($4::uuid[])
                      ))
                     AND (
                         (
                             $5
                             AND replace(lower(event_type), '_', '') NOT LIKE '%agentsession%'
                             AND NOT (payload ? 'agentSession')
                             AND NOT (payload ? 'agentSessionEvent')
                             AND NOT (payload->'data' ? 'agentSession')
                             AND NOT (payload->'data' ? 'agentSessionEvent')
                         )
                         OR (
                             NOT $5
                             AND (
                                 replace(lower(event_type), '_', '') LIKE '%agentsession%'
                                 OR payload ? 'agentSession'
                                 OR payload ? 'agentSessionEvent'
                                 OR payload->'data' ? 'agentSession'
                                 OR payload->'data' ? 'agentSessionEvent'
                             )
                         )
                     )
                 )
               ORDER BY available_at, received_at, id
               FOR UPDATE SKIP LOCKED
               LIMIT $1
           )
           UPDATE linear_sync_inbox AS inbox
           SET locked_by = $2,
               locked_until = now() + ($3 * interval '1 second'),
               attempts = inbox.attempts + 1
           FROM picked
           WHERE inbox.id = picked.id
           RETURNING inbox.id, inbox.connection_id, inbox.delivery_id,
                     inbox.event_type, inbox.payload, inbox.received_at,
                     inbox.processed_at, inbox.attempts, inbox.last_error,
                     inbox.available_at, inbox.locked_by, inbox.locked_until,
                     inbox.max_attempts, inbox.dead_lettered_at"#,
    )
    .bind(limit.clamp(1, 100))
    .bind(worker_id)
    .bind(lease_seconds.max(1))
    .bind(workspace_ids.map(|ids| ids.to_vec()))
    .bind(include_issue_events)
    .fetch_all(executor)
    .await?)
}

pub async fn renew_claimed_sync_inbox(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    worker_id: &str,
    lease_seconds: i64,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_sync_inbox
           SET locked_until = now() + ($3 * interval '1 second')
           WHERE id = $1
             AND processed_at IS NULL
             AND dead_lettered_at IS NULL
             AND locked_by = $2
             AND locked_until > now()"#,
    )
    .bind(id)
    .bind(worker_id)
    .bind(lease_seconds.max(1))
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Rows whose last worker died after claim are dead-lettered once their retry
/// budget is exhausted. This prevents a permanent protocol error from being
/// selected forever after its lease expires.
pub async fn dead_letter_exhausted_sync_inbox(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_ids: Option<&[Uuid]>,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"UPDATE linear_sync_inbox
           SET dead_lettered_at = now(),
               locked_by = NULL,
               locked_until = NULL,
               last_error = COALESCE(last_error, 'maximum attempts exceeded')
           WHERE processed_at IS NULL
             AND dead_lettered_at IS NULL
             AND attempts >= max_attempts
             AND (locked_until IS NULL OR locked_until < now())
             AND (
                 $1::uuid[] IS NULL
                 OR EXISTS (
                     SELECT 1
                     FROM linear_connection
                     WHERE linear_connection.id = linear_sync_inbox.connection_id
                       AND linear_connection.workspace_id = ANY($1::uuid[])
                 )
             )"#,
    )
    .bind(workspace_ids.map(|ids| ids.to_vec()))
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

pub async fn complete_claimed_sync_inbox(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    worker_id: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_sync_inbox
           SET processed_at = now(),
               locked_by = NULL,
               locked_until = NULL,
               last_error = NULL
           WHERE id = $1
             AND processed_at IS NULL
             AND locked_by = $2
             AND locked_until > now()"#,
    )
    .bind(id)
    .bind(worker_id)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn retry_claimed_sync_inbox(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    worker_id: &str,
    available_at: DateTime<Utc>,
    error: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_sync_inbox
           SET available_at = $3,
               locked_by = NULL,
               locked_until = NULL,
               last_error = $4
           WHERE id = $1
             AND processed_at IS NULL
             AND dead_lettered_at IS NULL
             AND locked_by = $2
             AND locked_until > now()
             AND attempts < max_attempts"#,
    )
    .bind(id)
    .bind(worker_id)
    .bind(available_at)
    .bind(error)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn dead_letter_claimed_sync_inbox(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    worker_id: &str,
    error: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_sync_inbox
           SET dead_lettered_at = now(),
               locked_by = NULL,
               locked_until = NULL,
               last_error = $3
           WHERE id = $1
             AND processed_at IS NULL
             AND dead_lettered_at IS NULL
             AND locked_by = $2
             AND locked_until > now()"#,
    )
    .bind(id)
    .bind(worker_id)
    .bind(error)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

fn outbox_columns() -> &'static str {
    "id, workspace_id, binding_id, issue_id, event_key, event_type, payload,\
     available_at, locked_by, locked_until, max_attempts, attempts, last_error,\
     processed_at, dead_lettered_at, created_at, updated_at"
}

/// Finds the one active binding that is allowed to publish a local Issue.
/// Project bindings are unique for a live local project, but the query still
/// orders deterministically so a partially migrated database cannot make the
/// worker choose randomly.
pub async fn get_push_binding_for_project(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> anyhow::Result<Option<LinearProjectBinding>> {
    let columns = binding_columns();
    let query = format!(
        "SELECT {columns} FROM linear_project_binding\
         WHERE workspace_id = $1 AND patchbay_project_id = $2\
           AND status = 'active'\
           AND sync_mode IN ('publish', 'two_way')\
           AND linear_team_id IS NOT NULL\
         ORDER BY updated_at DESC, id DESC LIMIT 1"
    );
    Ok(sqlx::query_as::<_, LinearProjectBinding>(&query)
        .bind(workspace_id)
        .bind(project_id)
        .fetch_optional(executor)
        .await?)
}

/// Appends an outbound event only when the Issue's Project has an active
/// publish-capable binding. The unique `(binding_id, event_key)` index makes
/// repeated domain-event delivery idempotent.
pub async fn enqueue_issue_outbox(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    issue_id: Uuid,
    event_key: &str,
    event_type: &str,
    payload: &Value,
) -> anyhow::Result<Option<LinearSyncOutbox>> {
    let Some(project_id) = project_id else {
        return Ok(None);
    };
    let columns = outbox_columns();
    let query = format!(
        "INSERT INTO linear_sync_outbox \
         (id, workspace_id, binding_id, issue_id, event_key, event_type, payload) \
         SELECT $1, $2, binding.id, $4, $5, $6, $7 \
         FROM linear_project_binding AS binding \
         WHERE binding.workspace_id = $2 \
           AND binding.patchbay_project_id = $3 \
           AND binding.status = 'active' \
           AND binding.sync_mode IN ('publish', 'two_way') \
           AND binding.linear_team_id IS NOT NULL \
         ORDER BY binding.updated_at DESC, binding.id DESC \
         LIMIT 1 \
         ON CONFLICT (binding_id, event_key) DO NOTHING \
         RETURNING {columns}"
    );
    Ok(sqlx::query_as::<_, LinearSyncOutbox>(&query)
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(project_id)
        .bind(issue_id)
        .bind(event_key)
        .bind(event_type)
        .bind(payload)
        .fetch_optional(executor)
        .await?)
}

/// Appends an outbound event for a specific binding. Activation seeding uses
/// the binding id explicitly so a partially migrated workspace cannot enqueue
/// an Issue into a different binding selected by project ordering.
pub async fn enqueue_issue_outbox_for_binding(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    binding_id: Uuid,
    issue_id: Uuid,
    event_key: &str,
    event_type: &str,
    payload: &Value,
) -> anyhow::Result<Option<LinearSyncOutbox>> {
    let columns = outbox_columns();
    let query = format!(
        "INSERT INTO linear_sync_outbox \
         (id, workspace_id, binding_id, issue_id, event_key, event_type, payload) \
         SELECT $1, $2, binding.id, $4, $5, $6, $7 \
         FROM linear_project_binding AS binding \
         WHERE binding.id = $3 \
           AND binding.workspace_id = $2 \
           AND binding.status = 'active' \
           AND binding.sync_mode IN ('publish', 'two_way') \
           AND binding.linear_team_id IS NOT NULL \
         ON CONFLICT (binding_id, event_key) DO NOTHING \
         RETURNING {columns}"
    );
    Ok(sqlx::query_as::<_, LinearSyncOutbox>(&query)
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(binding_id)
        .bind(issue_id)
        .bind(event_key)
        .bind(event_type)
        .bind(payload)
        .fetch_optional(executor)
        .await?)
}

/// Claims outbound rows with a PostgreSQL lease. A separate Outbox lease is
/// used even though Inbox has the same shape: inbound and outbound pressure
/// must not starve each other.
pub async fn claim_sync_outbox(
    executor: &sqlx::PgPool,
    worker_id: &str,
    limit: i64,
    lease_seconds: i64,
    workspace_ids: Option<&[Uuid]>,
) -> anyhow::Result<Vec<LinearSyncOutbox>> {
    let columns = outbox_columns();
    let query = format!(
        "WITH picked AS ( \
             SELECT outbox.id FROM linear_sync_outbox AS outbox \
             JOIN linear_project_binding AS binding ON binding.id = outbox.binding_id \
             JOIN linear_connection AS connection ON connection.id = binding.connection_id \
             WHERE outbox.processed_at IS NULL \
               AND outbox.dead_lettered_at IS NULL \
               AND outbox.available_at <= now() \
               AND outbox.attempts < outbox.max_attempts \
               AND (outbox.locked_until IS NULL OR outbox.locked_until < now()) \
               AND binding.status = 'active' \
               AND binding.sync_mode IN ('publish', 'two_way') \
               AND binding.linear_team_id IS NOT NULL \
               AND connection.status = 'active' \
               AND (\
                   $4::uuid[] IS NULL \
                   OR connection.workspace_id = ANY($4::uuid[]) \
               ) \
               AND NOT EXISTS (\
                   SELECT 1 \
                   FROM linear_sync_outbox AS earlier \
                   WHERE earlier.workspace_id = outbox.workspace_id \
                     AND earlier.issue_id = outbox.issue_id \
                     AND earlier.processed_at IS NULL \
                     AND earlier.dead_lettered_at IS NULL \
                     AND EXISTS (\
                         SELECT 1 \
                         FROM linear_project_binding AS earlier_binding \
                         JOIN linear_connection AS earlier_connection \
                           ON earlier_connection.id = earlier_binding.connection_id \
                         WHERE earlier_binding.id = earlier.binding_id \
                           AND earlier_binding.status = 'active' \
                           AND earlier_binding.sync_mode IN ('publish', 'two_way') \
                           AND earlier_binding.linear_team_id IS NOT NULL \
                           AND earlier_connection.status = 'active' \
                     ) \
                     AND (earlier.created_at, earlier.id) < (outbox.created_at, outbox.id) \
               ) \
             ORDER BY outbox.available_at, outbox.created_at, outbox.id \
             FOR UPDATE SKIP LOCKED \
             LIMIT $1 \
         ) \
         UPDATE linear_sync_outbox AS outbox \
         SET locked_by = $2, \
             locked_until = now() + ($3 * interval '1 second'), \
             attempts = outbox.attempts + 1, \
             updated_at = now() \
         FROM picked \
         WHERE outbox.id = picked.id \
         RETURNING {columns}"
    );
    Ok(sqlx::query_as::<_, LinearSyncOutbox>(&query)
        .bind(limit.clamp(1, 100))
        .bind(worker_id)
        .bind(lease_seconds.max(1))
        .bind(workspace_ids.map(|ids| ids.to_vec()))
        .fetch_all(executor)
        .await?)
}

pub async fn dead_letter_exhausted_sync_outbox(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_ids: Option<&[Uuid]>,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"UPDATE linear_sync_outbox
           SET dead_lettered_at = now(), locked_by = NULL, locked_until = NULL,
               last_error = COALESCE(last_error, 'maximum attempts exceeded'),
               updated_at = now()
           WHERE processed_at IS NULL AND dead_lettered_at IS NULL
             AND attempts >= max_attempts
             AND (locked_until IS NULL OR locked_until < now())
             AND (
                 $1::uuid[] IS NULL
                 OR EXISTS (
                     SELECT 1
                     FROM linear_project_binding
                     JOIN linear_connection
                       ON linear_connection.id = linear_project_binding.connection_id
                     WHERE linear_project_binding.id = linear_sync_outbox.binding_id
                       AND linear_connection.workspace_id = ANY($1::uuid[])
                 )
             )"#,
    )
    .bind(workspace_ids.map(|ids| ids.to_vec()))
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

pub async fn renew_claimed_sync_outbox(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    worker_id: &str,
    lease_seconds: i64,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_sync_outbox
           SET locked_until = now() + ($3 * interval '1 second'), updated_at = now()
           WHERE id = $1
             AND processed_at IS NULL
             AND dead_lettered_at IS NULL
             AND locked_by = $2
             AND locked_until > now()"#,
    )
    .bind(id)
    .bind(worker_id)
    .bind(lease_seconds.max(1))
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn complete_claimed_sync_outbox(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    worker_id: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_sync_outbox
           SET processed_at = now(), locked_by = NULL, locked_until = NULL,
               last_error = NULL, updated_at = now()
           WHERE id = $1 AND processed_at IS NULL AND locked_by = $2
             AND locked_until > now()"#,
    )
    .bind(id)
    .bind(worker_id)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn retry_claimed_sync_outbox(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    worker_id: &str,
    available_at: DateTime<Utc>,
    error: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_sync_outbox
           SET available_at = $3, locked_by = NULL, locked_until = NULL,
               last_error = $4, updated_at = now()
           WHERE id = $1 AND processed_at IS NULL AND dead_lettered_at IS NULL
             AND locked_by = $2 AND locked_until > now()
             AND attempts < max_attempts"#,
    )
    .bind(id)
    .bind(worker_id)
    .bind(available_at)
    .bind(error)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn dead_letter_claimed_sync_outbox(
    executor: impl Executor<'_, Database = Postgres>,
    id: Uuid,
    worker_id: &str,
    error: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_sync_outbox
           SET dead_lettered_at = now(), locked_by = NULL, locked_until = NULL,
               last_error = $3, updated_at = now()
           WHERE id = $1 AND processed_at IS NULL AND dead_lettered_at IS NULL
             AND locked_by = $2 AND locked_until > now()"#,
    )
    .bind(id)
    .bind(worker_id)
    .bind(error)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn find_linear_issue_link(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_issue_id: &str,
) -> anyhow::Result<Option<LinearIssueLink>> {
    Ok(sqlx::query_as::<_, LinearIssueLink>(
        r#"SELECT link.id, link.workspace_id, link.binding_id,
                  link.patchbay_issue_id, link.linear_issue_id,
                  link.linear_identifier, link.last_common_snapshot,
                  link.remote_updated_at, link.last_remote_event_at_ms,
                  link.last_remote_event_id, link.sync_status,
                  link.created_at, link.updated_at
           FROM linear_issue_link AS link
           JOIN linear_project_binding AS binding
             ON binding.id = link.binding_id
           WHERE link.workspace_id = $1
             AND binding.connection_id = $2
             AND link.linear_issue_id = $3
             AND link.sync_status <> 'deleted'
             AND binding.status <> 'tombstone'
           ORDER BY link.updated_at DESC, link.id DESC
           LIMIT 1"#,
    )
    .bind(workspace_id)
    .bind(connection_id)
    .bind(linear_issue_id)
    .fetch_optional(executor)
    .await?)
}

pub async fn find_issue_by_linear_marker(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    linear_issue_id: &str,
) -> anyhow::Result<Option<Issue>> {
    Ok(sqlx::query_as::<_, Issue>(
        r#"SELECT *
           FROM issue
           WHERE workspace_id = $1
             AND metadata->'linear'->>'issue_id' = $2
           ORDER BY updated_at DESC, id DESC
           LIMIT 1"#,
    )
    .bind(workspace_id)
    .bind(linear_issue_id)
    .fetch_optional(executor)
    .await?)
}

pub async fn get_binding_for_remote_project(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    connection_id: Uuid,
    linear_project_id: &str,
) -> anyhow::Result<Option<LinearProjectBinding>> {
    let columns = binding_columns();
    let query = format!(
        "SELECT {columns} FROM linear_project_binding\
         WHERE workspace_id = $1 AND connection_id = $2\
           AND linear_project_id = $3 AND status IN ('active', 'paused')\
         ORDER BY updated_at DESC, id DESC LIMIT 1"
    );
    Ok(sqlx::query_as::<_, LinearProjectBinding>(&query)
        .bind(workspace_id)
        .bind(connection_id)
        .bind(linear_project_id)
        .fetch_optional(executor)
        .await?)
}

pub async fn get_linear_issue_link_by_patchbay_issue(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    patchbay_issue_id: Uuid,
) -> anyhow::Result<Option<LinearIssueLink>> {
    Ok(sqlx::query_as::<_, LinearIssueLink>(
        r#"SELECT id, workspace_id, binding_id, patchbay_issue_id,
                  linear_issue_id, linear_identifier, last_common_snapshot,
                  remote_updated_at, last_remote_event_at_ms,
                  last_remote_event_id, sync_status, created_at, updated_at
           FROM linear_issue_link
           WHERE workspace_id = $1
             AND patchbay_issue_id = $2
             AND sync_status <> 'deleted'
           ORDER BY updated_at DESC, id DESC
           LIMIT 1"#,
    )
    .bind(workspace_id)
    .bind(patchbay_issue_id)
    .fetch_optional(executor)
    .await?)
}

pub struct LinearIssueLinkInput<'a> {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub binding_id: Uuid,
    pub patchbay_issue_id: Uuid,
    pub linear_issue_id: &'a str,
    pub linear_identifier: &'a str,
    pub last_common_snapshot: &'a Value,
    pub remote_updated_at: Option<DateTime<Utc>>,
    pub last_remote_event_at_ms: Option<i64>,
    pub last_remote_event_id: Option<&'a str>,
}

pub async fn create_linear_issue_link(
    executor: impl Executor<'_, Database = Postgres>,
    input: &LinearIssueLinkInput<'_>,
) -> anyhow::Result<Option<LinearIssueLink>> {
    Ok(sqlx::query_as::<_, LinearIssueLink>(
        r#"INSERT INTO linear_issue_link
           (id, workspace_id, binding_id, patchbay_issue_id, linear_issue_id,
            linear_identifier, last_common_snapshot, remote_updated_at,
            last_remote_event_at_ms, last_remote_event_id)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
           ON CONFLICT DO NOTHING
           RETURNING id, workspace_id, binding_id, patchbay_issue_id,
                     linear_issue_id, linear_identifier, last_common_snapshot,
                     remote_updated_at, last_remote_event_at_ms,
                     last_remote_event_id, sync_status, created_at, updated_at"#,
    )
    .bind(input.id)
    .bind(input.workspace_id)
    .bind(input.binding_id)
    .bind(input.patchbay_issue_id)
    .bind(input.linear_issue_id)
    .bind(input.linear_identifier)
    .bind(input.last_common_snapshot)
    .bind(input.remote_updated_at)
    .bind(input.last_remote_event_at_ms)
    .bind(input.last_remote_event_id)
    .fetch_optional(executor)
    .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn update_linear_issue_link(
    executor: impl Executor<'_, Database = Postgres>,
    link_id: Uuid,
    workspace_id: Uuid,
    last_common_snapshot: &Value,
    remote_updated_at: Option<DateTime<Utc>>,
    last_remote_event_at_ms: Option<i64>,
    last_remote_event_id: Option<&str>,
    sync_status: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_issue_link
           SET last_common_snapshot = $3,
               remote_updated_at = $4,
               last_remote_event_at_ms = $5,
               last_remote_event_id = $6,
               sync_status = $7,
               updated_at = now()
           WHERE id = $1
             AND workspace_id = $2
             AND (
                 $5::bigint IS NULL
                 OR last_remote_event_at_ms IS NULL
                 OR $5::bigint > last_remote_event_at_ms
             )"#,
    )
    .bind(link_id)
    .bind(workspace_id)
    .bind(last_common_snapshot)
    .bind(remote_updated_at)
    .bind(last_remote_event_at_ms)
    .bind(last_remote_event_id)
    .bind(sync_status)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Updates link state after the caller has already established ordering and
/// holds the link row lock. Conflict resolution uses this variant because a
/// user decision must not be rejected merely because it carries the same
/// remote event metadata that is already stored on the link.
#[allow(clippy::too_many_arguments)]
pub async fn set_linear_issue_link_state(
    executor: impl Executor<'_, Database = Postgres>,
    link_id: Uuid,
    workspace_id: Uuid,
    last_common_snapshot: &Value,
    remote_updated_at: Option<DateTime<Utc>>,
    last_remote_event_at_ms: Option<i64>,
    last_remote_event_id: Option<&str>,
    sync_status: &str,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_issue_link
           SET last_common_snapshot = $3,
               remote_updated_at = $4,
               last_remote_event_at_ms = $5,
               last_remote_event_id = $6,
               sync_status = $7,
               updated_at = now()
           WHERE id = $1 AND workspace_id = $2"#,
    )
    .bind(link_id)
    .bind(workspace_id)
    .bind(last_common_snapshot)
    .bind(remote_updated_at)
    .bind(last_remote_event_at_ms)
    .bind(last_remote_event_id)
    .bind(sync_status)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn rebind_linear_issue_link(
    executor: impl Executor<'_, Database = Postgres>,
    link_id: Uuid,
    workspace_id: Uuid,
    binding_id: Uuid,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_issue_link
           SET binding_id = $3, updated_at = now()
           WHERE id = $1 AND workspace_id = $2 AND sync_status <> 'deleted'"#,
    )
    .bind(link_id)
    .bind(workspace_id)
    .bind(binding_id)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn get_linear_issue_link(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    link_id: Uuid,
) -> anyhow::Result<Option<LinearIssueLink>> {
    Ok(sqlx::query_as::<_, LinearIssueLink>(
        r#"SELECT id, workspace_id, binding_id, patchbay_issue_id,
                  linear_issue_id, linear_identifier, last_common_snapshot,
                  remote_updated_at, last_remote_event_at_ms,
                  last_remote_event_id, sync_status, created_at, updated_at
           FROM linear_issue_link
           WHERE id = $1 AND workspace_id = $2"#,
    )
    .bind(link_id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?)
}

pub async fn get_linear_issue_link_for_update(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    link_id: Uuid,
) -> anyhow::Result<Option<LinearIssueLink>> {
    Ok(sqlx::query_as::<_, LinearIssueLink>(
        r#"SELECT id, workspace_id, binding_id, patchbay_issue_id,
                  linear_issue_id, linear_identifier, last_common_snapshot,
                  remote_updated_at, last_remote_event_at_ms,
                  last_remote_event_id, sync_status, created_at, updated_at
           FROM linear_issue_link
           WHERE id = $1 AND workspace_id = $2
           FOR UPDATE"#,
    )
    .bind(link_id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?)
}

fn conflict_columns() -> &'static str {
    "id, workspace_id, binding_id, link_id, patchbay_issue_id, linear_issue_id,\
     field, base_value, local_value, remote_value, source_event_id,\
     source_event_at_ms, status, resolution, resolved_value, resolved_by_id,\
     created_at, updated_at"
}

pub async fn list_linear_sync_conflicts(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    status: Option<&str>,
) -> anyhow::Result<Vec<LinearSyncConflict>> {
    let query = format!(
        "SELECT {columns} FROM linear_sync_conflict\
         WHERE workspace_id = $1 AND ($2::text IS NULL OR status = $2)\
         ORDER BY CASE WHEN status = 'open' THEN 0 ELSE 1 END, updated_at DESC, id DESC",
        columns = conflict_columns(),
    );
    Ok(sqlx::query_as::<_, LinearSyncConflict>(&query)
        .bind(workspace_id)
        .bind(status)
        .fetch_all(executor)
        .await?)
}

pub async fn get_linear_sync_conflict(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    conflict_id: Uuid,
) -> anyhow::Result<Option<LinearSyncConflict>> {
    let query = format!(
        "SELECT {columns} FROM linear_sync_conflict\
         WHERE workspace_id = $1 AND id = $2",
        columns = conflict_columns(),
    );
    Ok(sqlx::query_as::<_, LinearSyncConflict>(&query)
        .bind(workspace_id)
        .bind(conflict_id)
    .fetch_optional(executor)
    .await?)
}

pub async fn get_linear_sync_conflict_for_update(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    conflict_id: Uuid,
) -> anyhow::Result<Option<LinearSyncConflict>> {
    let query = format!(
        "SELECT {columns} FROM linear_sync_conflict\
         WHERE workspace_id = $1 AND id = $2\
         FOR UPDATE",
        columns = conflict_columns(),
    );
    Ok(sqlx::query_as::<_, LinearSyncConflict>(&query)
        .bind(workspace_id)
        .bind(conflict_id)
        .fetch_optional(executor)
        .await?)
}

pub struct LinearSyncConflictInput<'a> {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub binding_id: Uuid,
    pub link_id: Uuid,
    pub patchbay_issue_id: Uuid,
    pub linear_issue_id: &'a str,
    pub field: &'a str,
    pub base_value: &'a Value,
    pub local_value: &'a Value,
    pub remote_value: &'a Value,
    pub source_event_id: &'a str,
    pub source_event_at_ms: Option<i64>,
}

pub async fn create_linear_sync_conflict(
    executor: impl Executor<'_, Database = Postgres>,
    input: &LinearSyncConflictInput<'_>,
) -> anyhow::Result<Option<LinearSyncConflict>> {
    let query = format!(
        "INSERT INTO linear_sync_conflict\
         (id, workspace_id, binding_id, link_id, patchbay_issue_id, linear_issue_id,\
          field, base_value, local_value, remote_value, source_event_id,\
          source_event_at_ms)\
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)\
         ON CONFLICT (link_id, field) WHERE status = 'open' DO UPDATE SET\
             base_value = EXCLUDED.base_value,\
             local_value = EXCLUDED.local_value,\
             remote_value = EXCLUDED.remote_value,\
             source_event_id = EXCLUDED.source_event_id,\
             source_event_at_ms = EXCLUDED.source_event_at_ms,\
             updated_at = now()\
         WHERE EXCLUDED.source_event_at_ms IS NOT NULL\
           AND (linear_sync_conflict.source_event_at_ms IS NULL\
                OR EXCLUDED.source_event_at_ms > linear_sync_conflict.source_event_at_ms)\
         RETURNING {columns}",
        columns = conflict_columns(),
    );
    Ok(sqlx::query_as::<_, LinearSyncConflict>(&query)
        .bind(input.id)
        .bind(input.workspace_id)
        .bind(input.binding_id)
        .bind(input.link_id)
        .bind(input.patchbay_issue_id)
        .bind(input.linear_issue_id)
        .bind(input.field)
        .bind(input.base_value)
        .bind(input.local_value)
        .bind(input.remote_value)
        .bind(input.source_event_id)
        .bind(input.source_event_at_ms)
        .fetch_optional(executor)
        .await?)
}

pub async fn resolve_linear_sync_conflict(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    conflict_id: Uuid,
    resolution: &str,
    resolved_value: &Value,
    resolved_by_id: Uuid,
) -> anyhow::Result<Option<LinearSyncConflict>> {
    let query = format!(
        "UPDATE linear_sync_conflict\
         SET status = 'resolved', resolution = $3, resolved_value = $4,\
             resolved_by_id = $5, updated_at = now()\
         WHERE workspace_id = $1 AND id = $2 AND status = 'open'\
         RETURNING {columns}",
        columns = conflict_columns(),
    );
    Ok(sqlx::query_as::<_, LinearSyncConflict>(&query)
        .bind(workspace_id)
        .bind(conflict_id)
        .bind(resolution)
        .bind(resolved_value)
        .bind(resolved_by_id)
        .fetch_optional(executor)
        .await?)
}

pub async fn count_open_linear_sync_conflicts(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        r#"SELECT COUNT(*)::bigint
           FROM linear_sync_conflict
           WHERE workspace_id = $1 AND status = 'open'"#,
    )
    .bind(workspace_id)
    .fetch_one(executor)
    .await?)
}

pub async fn count_open_linear_sync_conflicts_for_link(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    link_id: Uuid,
) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        r#"SELECT COUNT(*)::bigint
           FROM linear_sync_conflict
           WHERE workspace_id = $1 AND link_id = $2 AND status = 'open'"#,
    )
    .bind(workspace_id)
    .bind(link_id)
    .fetch_one(executor)
    .await?)
}

pub async fn mark_linear_issue_link_deleted(
    executor: impl Executor<'_, Database = Postgres>,
    link_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_issue_link
           SET sync_status = 'deleted', updated_at = now()
           WHERE id = $1 AND workspace_id = $2 AND sync_status <> 'deleted'"#,
    )
    .bind(link_id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn mark_linear_issue_link_conflict(
    executor: impl Executor<'_, Database = Postgres>,
    link_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<bool> {
    let result = sqlx::query(
        r#"UPDATE linear_issue_link
           SET sync_status = 'conflict', updated_at = now()
           WHERE id = $1 AND workspace_id = $2 AND sync_status <> 'deleted'"#,
    )
    .bind(link_id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn project_belongs_to_workspace(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar(
        r#"SELECT EXISTS(
               SELECT 1 FROM project WHERE workspace_id = $1 AND id = $2
           )"#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(executor)
    .await?)
}

pub async fn count_issues_in_project(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        r#"SELECT COUNT(*)::bigint
           FROM issue
           WHERE workspace_id = $1 AND project_id = $2"#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(executor)
    .await?)
}

pub struct LinearProjectBindingInput<'a> {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub connection_id: Uuid,
    pub patchbay_project_id: Uuid,
    pub linear_project_id: &'a str,
    pub linear_team_id: Option<&'a str>,
    pub status: &'a str,
    pub sync_mode: &'a str,
    pub initial_source_of_truth: Option<&'a str>,
    pub status_mapping: &'a Value,
    pub agent_label_mapping: &'a Value,
    pub created_by_id: Uuid,
}

fn binding_columns() -> &'static str {
    "id, workspace_id, connection_id, patchbay_project_id, linear_project_id,\
     linear_team_id, status, sync_mode, initial_source_of_truth, status_mapping,\
     agent_label_mapping, activated_at, paused_at, created_by_id, created_at,\
     updated_at"
}

pub async fn create_project_binding(
    executor: impl Executor<'_, Database = Postgres>,
    input: &LinearProjectBindingInput<'_>,
) -> anyhow::Result<LinearProjectBinding> {
    let query = format!(
        "INSERT INTO linear_project_binding\
         (id, workspace_id, connection_id, patchbay_project_id, linear_project_id,\
          linear_team_id, status, sync_mode, initial_source_of_truth,\
          status_mapping, agent_label_mapping, activated_at, paused_at, created_by_id)\
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,\
                 CASE WHEN $7 = 'active' THEN now() ELSE NULL END,\
                 CASE WHEN $7 = 'paused' THEN now() ELSE NULL END, $12)\
         RETURNING {columns}",
        columns = binding_columns(),
    );
    Ok(sqlx::query_as::<_, LinearProjectBinding>(&query)
        .bind(input.id)
        .bind(input.workspace_id)
        .bind(input.connection_id)
        .bind(input.patchbay_project_id)
        .bind(input.linear_project_id)
        .bind(input.linear_team_id)
        .bind(input.status)
        .bind(input.sync_mode)
        .bind(input.initial_source_of_truth)
        .bind(input.status_mapping)
        .bind(input.agent_label_mapping)
        .bind(input.created_by_id)
        .fetch_one(executor)
        .await?)
}

pub async fn list_project_bindings(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<LinearProjectBinding>> {
    let query = format!(
        "SELECT {columns} FROM linear_project_binding\
         WHERE workspace_id = $1 AND status <> 'tombstone'\
         ORDER BY created_at, id",
        columns = binding_columns(),
    );
    Ok(sqlx::query_as::<_, LinearProjectBinding>(&query)
        .bind(workspace_id)
        .fetch_all(executor)
        .await?)
}

pub async fn get_project_binding(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    binding_id: Uuid,
) -> anyhow::Result<Option<LinearProjectBinding>> {
    let query = format!(
        "SELECT {columns} FROM linear_project_binding\
         WHERE workspace_id = $1 AND id = $2 AND status <> 'tombstone'",
        columns = binding_columns(),
    );
    Ok(sqlx::query_as::<_, LinearProjectBinding>(&query)
        .bind(workspace_id)
        .bind(binding_id)
    .fetch_optional(executor)
    .await?)
}

pub async fn get_project_binding_for_update(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    binding_id: Uuid,
) -> anyhow::Result<Option<LinearProjectBinding>> {
    let query = format!(
        "SELECT {columns} FROM linear_project_binding\
         WHERE workspace_id = $1 AND id = $2 AND status <> 'tombstone'\
         FOR UPDATE",
        columns = binding_columns(),
    );
    Ok(sqlx::query_as::<_, LinearProjectBinding>(&query)
        .bind(workspace_id)
        .bind(binding_id)
        .fetch_optional(executor)
        .await?)
}

pub async fn update_project_binding(
    executor: impl Executor<'_, Database = Postgres>,
    input: &LinearProjectBindingInput<'_>,
) -> anyhow::Result<Option<LinearProjectBinding>> {
    let query = format!(
        "UPDATE linear_project_binding\
         SET linear_project_id = $3, linear_team_id = $4, status = $5,\
             sync_mode = $6, initial_source_of_truth = $7, status_mapping = $8,\
             agent_label_mapping = $9,\
             activated_at = CASE WHEN $5 = 'active'\
                                 THEN COALESCE(activated_at, now()) ELSE activated_at END,\
             paused_at = CASE WHEN $5 = 'paused' THEN COALESCE(paused_at, now())\
                              WHEN $5 = 'active' THEN NULL ELSE paused_at END,\
             updated_at = now()\
         WHERE workspace_id = $1 AND id = $2 AND status <> 'tombstone'\
         RETURNING {columns}",
        columns = binding_columns(),
    );
    Ok(sqlx::query_as::<_, LinearProjectBinding>(&query)
        .bind(input.workspace_id)
        .bind(input.id)
        .bind(input.linear_project_id)
        .bind(input.linear_team_id)
        .bind(input.status)
        .bind(input.sync_mode)
        .bind(input.initial_source_of_truth)
        .bind(input.status_mapping)
        .bind(input.agent_label_mapping)
        .fetch_optional(executor)
        .await?)
}

pub async fn tombstone_project_binding(
    executor: impl Executor<'_, Database = Postgres>,
    workspace_id: Uuid,
    binding_id: Uuid,
) -> anyhow::Result<bool> {
    let result = sqlx::query_scalar::<_, i64>(
        r#"WITH target_binding AS (
               SELECT id
               FROM linear_project_binding
               WHERE workspace_id = $1 AND id = $2
           ), tombstoned_binding AS (
               UPDATE linear_project_binding
               SET status = 'tombstone',
                   paused_at = COALESCE(paused_at, now()),
                   updated_at = now()
               WHERE id IN (SELECT id FROM target_binding) AND status <> 'tombstone'
               RETURNING id
           ), deleted_links AS (
               UPDATE linear_issue_link
               SET sync_status = 'deleted', updated_at = now()
               WHERE workspace_id = $1
                 AND binding_id IN (SELECT id FROM target_binding)
                 AND sync_status <> 'deleted'
               RETURNING id
           )
           SELECT COUNT(*)::bigint FROM target_binding"#,
    )
    .bind(workspace_id)
    .bind(binding_id)
    .fetch_one(executor)
    .await?;
    Ok(result == 1)
}
