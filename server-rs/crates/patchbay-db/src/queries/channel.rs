//! Typed SQL queries for channel records.
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn acquire_channel_ws_lease(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    new_token: Option<&str>,
    new_expires_at: Option<DateTime<Utc>>,
    id: Uuid,
) -> anyhow::Result<Option<ChannelInstallation>> {
    let row = sqlx::query(
        r#"UPDATE channel_installation
SET ws_lease_token       = $1,
    ws_lease_expires_at  = $2,
    updated_at           = now()
WHERE id = $3
  AND status = 'active'
  AND (
        ws_lease_token IS NULL
        OR ws_lease_expires_at < now()
        OR ws_lease_token = $1
  )
RETURNING id, workspace_id, agent_id, channel_type, config, status, ws_lease_token, ws_lease_expires_at, installer_user_id, installed_at, created_at, updated_at"#
    )
        .bind(new_token)
        .bind(new_expires_at)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        config: row.try_get(4)?,
        status: row.try_get(5)?,
        ws_lease_token: row.try_get(6)?,
        ws_lease_expires_at: row.try_get(7)?,
        installer_user_id: row.try_get(8)?,
        installed_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

pub async fn backfill_channel_installation_region_to_feishu_lark(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE channel_installation
SET config     = jsonb_set(config, '{region}', '"lark"'),
    updated_at = now()
WHERE channel_type = 'feishu'
  AND config ->> 'region' = 'feishu'"#,
    )
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn channel_media_object_is_referenced(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_message_id: Uuid,
    workspace_id: Uuid,
    storage_url: &str,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT EXISTS (
    SELECT 1 FROM attachment
    WHERE chat_message_id = $1
      AND workspace_id = $2
      AND url = $3
) AS referenced"#,
    )
    .bind(chat_message_id)
    .bind(workspace_id)
    .bind(storage_url)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn claim_channel_inbound_dedup(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    message_id: &str,
) -> anyhow::Result<Option<ChannelInboundMessageDedup>> {
    let row = sqlx::query(
        r#"INSERT INTO channel_inbound_message_dedup (installation_id, message_id, claim_token)
VALUES ($1, $2, gen_random_uuid())
ON CONFLICT (installation_id, message_id) DO UPDATE
    SET received_at = now(),
        claim_token = gen_random_uuid()
    WHERE channel_inbound_message_dedup.processed_at IS NULL
      AND channel_inbound_message_dedup.received_at < now() - INTERVAL '60 seconds'
RETURNING installation_id, message_id, received_at, processed_at, claim_token"#,
    )
    .bind(installation_id)
    .bind(message_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelInboundMessageDedup {
        installation_id: row.try_get(0)?,
        message_id: row.try_get(1)?,
        received_at: row.try_get(2)?,
        processed_at: row.try_get(3)?,
        claim_token: row.try_get(4)?,
    }))
}

pub async fn claim_channel_media_pending_objects_for_bind(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    storage_keys: &[String],
    workspace_id: Uuid,
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query(
        r#"DELETE FROM channel_media_pending_object
WHERE storage_key = ANY($1::text[])
  AND workspace_id = $2
  AND state = 'pending'
RETURNING storage_key"#,
    )
    .bind(storage_keys)
    .bind(workspace_id)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(row.try_get(0)?);
    }
    Ok(out)
}

pub async fn claim_next_channel_media_pending_object_for_reconcile(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    lease_token: Uuid,
    lease: sqlx::postgres::types::PgInterval,
    settle_delay: sqlx::postgres::types::PgInterval,
) -> anyhow::Result<Option<ChannelMediaPendingObject>> {
    let row = sqlx::query(
        r#"UPDATE channel_media_pending_object AS obj
SET state = CASE WHEN obj.state = 'tombstoned' THEN 'tombstoned' ELSE 'deleting' END,
    lease_token = $1,
    lease_expires_at = now() + $2::interval,
    attempt = obj.attempt + 1
FROM (
    SELECT cand.storage_key FROM channel_media_pending_object AS cand
    WHERE cand.next_attempt_at <= now()
      AND (
          (cand.state = 'pending' AND cand.created_at <= now() - $3::interval)
          OR (cand.state = 'deleting' AND (cand.lease_expires_at IS NULL OR cand.lease_expires_at <= now()))
          OR (cand.state = 'tombstoned' AND (cand.lease_expires_at IS NULL OR cand.lease_expires_at <= now()))
      )
    ORDER BY cand.next_attempt_at
    LIMIT 1
    FOR UPDATE SKIP LOCKED
) AS due
WHERE obj.storage_key = due.storage_key
RETURNING obj.storage_key, obj.workspace_id, obj.chat_message_id, obj.storage_url, obj.installation_id, obj.state, obj.lease_token, obj.lease_expires_at, obj.attempt, obj.next_attempt_at, obj.last_error, obj.tombstone_pass, obj.created_at"#
    )
        .bind(lease_token)
        .bind(lease)
        .bind(settle_delay)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelMediaPendingObject {
        storage_key: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        chat_message_id: row.try_get(2)?,
        storage_url: row.try_get(3)?,
        installation_id: row.try_get(4)?,
        state: row.try_get(5)?,
        lease_token: row.try_get(6)?,
        lease_expires_at: row.try_get(7)?,
        attempt: row.try_get(8)?,
        next_attempt_at: row.try_get(9)?,
        last_error: row.try_get(10)?,
        tombstone_pass: row.try_get(11)?,
        created_at: row.try_get(12)?,
    }))
}

pub async fn clear_channel_chat_session_pending_fresh(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE channel_chat_session_binding
SET pending_fresh = FALSE
WHERE chat_session_id = $1"#,
    )
    .bind(chat_session_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn consume_channel_binding_token(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
) -> anyhow::Result<Option<ChannelBindingToken>> {
    let row = sqlx::query(
        r#"UPDATE channel_binding_token
SET consumed_at = now()
WHERE token_hash = $1
  AND consumed_at IS NULL
  AND expires_at > now()
RETURNING token_hash, workspace_id, installation_id, channel_type, channel_user_id, expires_at, consumed_at, created_at"#
    )
        .bind(token_hash)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelBindingToken {
        token_hash: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        channel_user_id: row.try_get(4)?,
        expires_at: row.try_get(5)?,
        consumed_at: row.try_get(6)?,
        created_at: row.try_get(7)?,
    }))
}

/// Atomically consume a legacy Lark binding token. Lark still mints into the
/// dedicated table even though newer channel adapters share
/// `channel_binding_token`.
pub async fn consume_lark_binding_token(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
) -> anyhow::Result<Option<LarkBindingToken>> {
    let row = sqlx::query(
        r#"UPDATE lark_binding_token
SET consumed_at = now()
WHERE token_hash = $1
  AND consumed_at IS NULL
  AND expires_at > now()
RETURNING token_hash, workspace_id, installation_id, lark_open_id,
          expires_at, consumed_at, created_at"#,
    )
    .bind(token_hash)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(LarkBindingToken {
        token_hash: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        lark_open_id: row.try_get(3)?,
        expires_at: row.try_get(4)?,
        consumed_at: row.try_get(5)?,
        created_at: row.try_get(6)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CountChannelMediaPendingObjectsRow {
    pub pending_objects: i64,
    pub tombstoned_objects: i64,
}

pub async fn count_channel_media_pending_objects(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
) -> anyhow::Result<Option<CountChannelMediaPendingObjectsRow>> {
    let row = sqlx::query(
        r#"SELECT
    count(*) FILTER (WHERE state <> 'tombstoned') AS pending_objects,
    count(*) FILTER (WHERE state = 'tombstoned') AS tombstoned_objects
FROM channel_media_pending_object"#,
    )
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(CountChannelMediaPendingObjectsRow {
        pending_objects: row.try_get(0)?,
        tombstoned_objects: row.try_get(1)?,
    }))
}

pub async fn create_channel_binding_token(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    token_hash: &str,
    workspace_id: Uuid,
    installation_id: Uuid,
    channel_type: &str,
    channel_user_id: &str,
    expires_at: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<ChannelBindingToken>> {
    let row = sqlx::query(
        r#"INSERT INTO channel_binding_token (
    token_hash, workspace_id, installation_id, channel_type,
    channel_user_id, expires_at
) VALUES (
    $1, $2, $3, $4, $5,
    LEAST($6::timestamptz, now() + INTERVAL '15 minutes')
)
RETURNING token_hash, workspace_id, installation_id, channel_type, channel_user_id, expires_at, consumed_at, created_at"#
    )
        .bind(token_hash)
        .bind(workspace_id)
        .bind(installation_id)
        .bind(channel_type)
        .bind(channel_user_id)
        .bind(expires_at)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelBindingToken {
        token_hash: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        channel_user_id: row.try_get(4)?,
        expires_at: row.try_get(5)?,
        consumed_at: row.try_get(6)?,
        created_at: row.try_get(7)?,
    }))
}

pub async fn create_channel_chat_session_binding(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
    installation_id: Uuid,
    channel_type: &str,
    channel_chat_id: &str,
    chat_type: &str,
    config: &serde_json::Value,
) -> anyhow::Result<Option<ChannelChatSessionBinding>> {
    let row = sqlx::query(
        r#"INSERT INTO channel_chat_session_binding (
    chat_session_id, installation_id, channel_type, channel_chat_id, chat_type, config
) VALUES (
    $1, $2, $3, $4, $5, $6
)
RETURNING id, chat_session_id, installation_id, channel_type, channel_chat_id, chat_type, last_message_id, last_thread_id, config, created_at, pending_fresh"#
    )
        .bind(chat_session_id)
        .bind(installation_id)
        .bind(channel_type)
        .bind(channel_chat_id)
        .bind(chat_type)
        .bind(config)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelChatSessionBinding {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        channel_chat_id: row.try_get(4)?,
        chat_type: row.try_get(5)?,
        last_message_id: row.try_get(6)?,
        last_thread_id: row.try_get(7)?,
        config: row.try_get(8)?,
        created_at: row.try_get(9)?,
        pending_fresh: row.try_get(10)?,
    }))
}

/// Merges adapter-owned routing metadata into an existing chat binding.
/// Secret-like, short-lived platform context belongs here rather than in the
/// normalized message or installation config. The installation + chat key
/// scope prevents one adapter/session from mutating another binding.
pub async fn merge_channel_chat_session_binding_config(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    channel_chat_id: &str,
    config: &serde_json::Value,
) -> anyhow::Result<Option<ChannelChatSessionBinding>> {
    let row = sqlx::query(
        r#"UPDATE channel_chat_session_binding
SET config = channel_chat_session_binding.config || jsonb_strip_nulls($3::jsonb)
WHERE installation_id = $1
  AND channel_chat_id = $2
RETURNING id, chat_session_id, installation_id, channel_type, channel_chat_id, chat_type, last_message_id, last_thread_id, config, created_at, pending_fresh"#,
    )
    .bind(installation_id)
    .bind(channel_chat_id)
    .bind(config)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelChatSessionBinding {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        channel_chat_id: row.try_get(4)?,
        chat_type: row.try_get(5)?,
        last_message_id: row.try_get(6)?,
        last_thread_id: row.try_get(7)?,
        config: row.try_get(8)?,
        created_at: row.try_get(9)?,
        pending_fresh: row.try_get(10)?,
    }))
}

pub async fn get_channel_receive_cursor(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    channel_type: &str,
) -> anyhow::Result<Option<String>> {
    let row = sqlx::query(
        r#"SELECT cursor
FROM channel_receive_state
WHERE installation_id = $1 AND channel_type = $2
ORDER BY updated_at DESC
LIMIT 1"#,
    )
    .bind(installation_id)
    .bind(channel_type)
    .fetch_optional(executor)
    .await?;
    row.map(|row| row.try_get(0).map_err(anyhow::Error::from))
        .transpose()
}

pub async fn replace_channel_receive_cursor(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    channel_type: &str,
    cursor: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO channel_receive_state (installation_id, channel_type, cursor)
VALUES ($1, $2, $3)
ON CONFLICT (installation_id, channel_type) DO UPDATE SET
    cursor = EXCLUDED.cursor,
    updated_at = now()"#,
    )
    .bind(installation_id)
    .bind(channel_type)
    .bind(cursor)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn delete_channel_receive_state(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    channel_type: &str,
) -> anyhow::Result<u64> {
    Ok(sqlx::query(
        r#"DELETE FROM channel_receive_state
WHERE installation_id = $1 AND channel_type = $2"#,
    )
    .bind(installation_id)
    .bind(channel_type)
    .execute(executor)
    .await?
    .rows_affected())
}

pub async fn create_channel_outbound_card_message(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
    channel_type: &str,
    channel_chat_id: &str,
    channel_card_message_id: &str,
    status: &str,
    task_id: Uuid,
) -> anyhow::Result<Option<ChannelOutboundCardMessage>> {
    let row = sqlx::query(
        r#"INSERT INTO channel_outbound_card_message (
    chat_session_id, task_id, channel_type, channel_chat_id,
    channel_card_message_id, status
) VALUES (
    $1, $6, $2, $3, $4, $5
)
RETURNING id, chat_session_id, task_id, channel_type, channel_chat_id, channel_card_message_id, status, last_patched_at, created_at"#
    )
        .bind(chat_session_id)
        .bind(channel_type)
        .bind(channel_chat_id)
        .bind(channel_card_message_id)
        .bind(status)
        .bind(task_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelOutboundCardMessage {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        task_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        channel_chat_id: row.try_get(4)?,
        channel_card_message_id: row.try_get(5)?,
        status: row.try_get(6)?,
        last_patched_at: row.try_get(7)?,
        created_at: row.try_get(8)?,
    }))
}

pub async fn create_channel_user_binding(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    patchbay_user_id: Uuid,
    installation_id: Uuid,
    channel_type: &str,
    channel_user_id: &str,
    config: &serde_json::Value,
) -> anyhow::Result<Option<ChannelUserBinding>> {
    let row = sqlx::query(
        r#"INSERT INTO channel_user_binding (
    workspace_id, patchbay_user_id, installation_id,
    channel_type, channel_user_id, config
) VALUES (
    $1, $2, $3, $4, $5, $6
)
ON CONFLICT (installation_id, channel_user_id) DO UPDATE SET
    -- jsonb_strip_nulls(EXCLUDED.config) preserves the old lark semantics
    -- ` + "`" + `union_id = COALESCE(EXCLUDED.union_id, lark_user_binding.union_id)` + "`" + `:
    -- a re-bind that carries ` + "`" + `{"union_id": null}` + "`" + ` (or omits the key) must NOT
    -- erase a union_id we already captured. Only non-null incoming keys win.
    config   = channel_user_binding.config || jsonb_strip_nulls(EXCLUDED.config),
    bound_at = now()
WHERE channel_user_binding.patchbay_user_id = EXCLUDED.patchbay_user_id
RETURNING id, workspace_id, patchbay_user_id, installation_id, channel_type, channel_user_id, config, bound_at"#
    )
        .bind(workspace_id)
        .bind(patchbay_user_id)
        .bind(installation_id)
        .bind(channel_type)
        .bind(channel_user_id)
        .bind(config)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelUserBinding {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        patchbay_user_id: row.try_get(2)?,
        installation_id: row.try_get(3)?,
        channel_type: row.try_get(4)?,
        channel_user_id: row.try_get(5)?,
        config: row.try_get(6)?,
        bound_at: row.try_get(7)?,
    }))
}

pub async fn delete_channel_binding_tokens_by_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM channel_binding_token
WHERE installation_id = $1"#,
    )
    .bind(installation_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_channel_chat_session_binding_by_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM channel_chat_session_binding
WHERE chat_session_id = $1"#,
    )
    .bind(chat_session_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_channel_chat_session_bindings_by_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    channel_type: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM channel_chat_session_binding
WHERE installation_id = $1 AND channel_type = $2"#,
    )
    .bind(installation_id)
    .bind(channel_type)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_channel_installations_by_system_runtime_agents(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    runtime_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"WITH doomed AS (
    SELECT id FROM channel_installation
    WHERE agent_id IN (
        SELECT id FROM agent WHERE runtime_id = $1 AND kind = 'system'
    )
),
cleared_chat_sessions AS (
    DELETE FROM channel_chat_session_binding WHERE installation_id IN (SELECT id FROM doomed)
    RETURNING chat_session_id
),
cleared_dingtalk_group_routes AS (
    DELETE FROM dingtalk_group_route WHERE installation_id IN (SELECT id FROM doomed)
),
cleared_outbound_cards AS (
    -- Reach channel_outbound_card_message (keyed by chat_session_id, no FK)
    -- through the just-removed chat-session bindings, same as the reclaim path.
    DELETE FROM channel_outbound_card_message
    WHERE chat_session_id IN (SELECT chat_session_id FROM cleared_chat_sessions)
),
cleared_binding_tokens AS (
    DELETE FROM channel_binding_token WHERE installation_id IN (SELECT id FROM doomed)
),
cleared_user_bindings AS (
    DELETE FROM channel_user_binding WHERE installation_id IN (SELECT id FROM doomed)
),
cleared_inbound_dedup AS (
    DELETE FROM channel_inbound_message_dedup WHERE installation_id IN (SELECT id FROM doomed)
),
cleared_receive_state AS (
    DELETE FROM channel_receive_state WHERE installation_id IN (SELECT id FROM doomed)
),
cleared_audit AS (
    -- Hard delete: purge audit rows rather than detaching them into permanently
    -- unattributable NULL rows (channel_inbound_audit has no workspace_id / reaper).
    DELETE FROM channel_inbound_audit WHERE installation_id IN (SELECT id FROM doomed)
)
DELETE FROM channel_installation WHERE id IN (SELECT id FROM doomed)"#,
    )
    .bind(runtime_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_channel_media_pending_object(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    storage_key: &str,
    workspace_id: Uuid,
    lease_token: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM channel_media_pending_object
WHERE storage_key = $1
  AND workspace_id = $2
  AND lease_token = $3"#,
    )
    .bind(storage_key)
    .bind(workspace_id)
    .bind(lease_token)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_channel_outbound_card_messages_by_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM channel_outbound_card_message
WHERE chat_session_id = $1"#,
    )
    .bind(chat_session_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_channel_user_bindings_by_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM channel_user_binding
WHERE installation_id = $1"#,
    )
    .bind(installation_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn delete_channel_user_bindings_by_workspace_member(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    patchbay_user_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM channel_user_binding
WHERE workspace_id = $1 AND patchbay_user_id = $2"#,
    )
    .bind(workspace_id)
    .bind(patchbay_user_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn find_channel_binding_for_member(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    patchbay_user_id: Uuid,
    channel_type: &str,
) -> anyhow::Result<Option<ChannelUserBinding>> {
    let row = sqlx::query(
        r#"SELECT b.id, b.workspace_id, b.patchbay_user_id, b.installation_id, b.channel_type, b.channel_user_id, b.config, b.bound_at FROM channel_user_binding b
JOIN channel_installation ci ON ci.id = b.installation_id
WHERE b.workspace_id = $1
  AND b.patchbay_user_id = $2
  AND b.channel_type = $3
  AND ci.status = 'active'
ORDER BY b.bound_at DESC
LIMIT 1"#
    )
        .bind(workspace_id)
        .bind(patchbay_user_id)
        .bind(channel_type)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelUserBinding {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        patchbay_user_id: row.try_get(2)?,
        installation_id: row.try_get(3)?,
        channel_type: row.try_get(4)?,
        channel_user_id: row.try_get(5)?,
        config: row.try_get(6)?,
        bound_at: row.try_get(7)?,
    }))
}

pub async fn find_live_channel_binding_token(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    channel_type: &str,
    channel_user_id: &str,
    mint_interval: sqlx::postgres::types::PgInterval,
) -> anyhow::Result<Option<ChannelBindingToken>> {
    let row = sqlx::query(
        r#"SELECT token_hash, workspace_id, installation_id, channel_type, channel_user_id, expires_at, consumed_at, created_at FROM channel_binding_token
WHERE installation_id = $1
  AND channel_type = $2
  AND channel_user_id = $3
  AND consumed_at IS NULL
  AND expires_at > now()
  AND created_at >= now() - $4::interval
ORDER BY created_at DESC
LIMIT 1"#
    )
        .bind(installation_id)
        .bind(channel_type)
        .bind(channel_user_id)
        .bind(mint_interval)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelBindingToken {
        token_hash: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        channel_user_id: row.try_get(4)?,
        expires_at: row.try_get(5)?,
        consumed_at: row.try_get(6)?,
        created_at: row.try_get(7)?,
    }))
}

pub async fn find_reusable_channel_user_binding(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    channel_type: &str,
    channel_user_id: &str,
    team_id: &str,
) -> anyhow::Result<Option<ChannelUserBinding>> {
    let row = sqlx::query(
        r#"SELECT b.id, b.workspace_id, b.patchbay_user_id, b.installation_id, b.channel_type, b.channel_user_id, b.config, b.bound_at FROM channel_user_binding b
JOIN channel_installation ci ON ci.id = b.installation_id
WHERE b.workspace_id = $1
  AND b.channel_type = $2
  AND b.channel_user_id = $3
  AND ci.config ->> 'team_id' = $4::text
ORDER BY b.bound_at DESC
LIMIT 1"#
    )
        .bind(workspace_id)
        .bind(channel_type)
        .bind(channel_user_id)
        .bind(team_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelUserBinding {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        patchbay_user_id: row.try_get(2)?,
        installation_id: row.try_get(3)?,
        channel_type: row.try_get(4)?,
        channel_user_id: row.try_get(5)?,
        config: row.try_get(6)?,
        bound_at: row.try_get(7)?,
    }))
}

pub async fn get_channel_chat_session_binding(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    channel_chat_id: &str,
) -> anyhow::Result<Option<ChannelChatSessionBinding>> {
    let row = sqlx::query(
        r#"SELECT id, chat_session_id, installation_id, channel_type, channel_chat_id, chat_type, last_message_id, last_thread_id, config, created_at, pending_fresh FROM channel_chat_session_binding
WHERE installation_id = $1 AND channel_chat_id = $2"#
    )
        .bind(installation_id)
        .bind(channel_chat_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelChatSessionBinding {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        channel_chat_id: row.try_get(4)?,
        chat_type: row.try_get(5)?,
        last_message_id: row.try_get(6)?,
        last_thread_id: row.try_get(7)?,
        config: row.try_get(8)?,
        created_at: row.try_get(9)?,
        pending_fresh: row.try_get(10)?,
    }))
}

/// Finds a channel hub binding when the caller knows the platform channel but
/// not the thread root. Slack slash commands have this shape: ordinary hub
/// messages use `channel:thread` as their isolation key, while `/issue` only
/// supplies `channel`. Prefer an exact channel binding and otherwise return
/// the newest thread binding in that channel.
pub async fn get_channel_chat_session_binding_for_channel(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    channel_id: &str,
) -> anyhow::Result<Option<ChannelChatSessionBinding>> {
    let row = sqlx::query(
        r#"SELECT id, chat_session_id, installation_id, channel_type, channel_chat_id, chat_type, last_message_id, last_thread_id, config, created_at, pending_fresh FROM channel_chat_session_binding
WHERE installation_id = $1
  AND (
      channel_chat_id = $2
      OR left(channel_chat_id, length($2) + 1) = $2 || ':'
  )
ORDER BY (channel_chat_id = $2) DESC, created_at DESC
LIMIT 1"#,
    )
    .bind(installation_id)
    .bind(channel_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelChatSessionBinding {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        channel_chat_id: row.try_get(4)?,
        chat_type: row.try_get(5)?,
        last_message_id: row.try_get(6)?,
        last_thread_id: row.try_get(7)?,
        config: row.try_get(8)?,
        created_at: row.try_get(9)?,
        pending_fresh: row.try_get(10)?,
    }))
}

pub async fn get_channel_chat_session_binding_by_session(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
    channel_type: &str,
) -> anyhow::Result<Option<ChannelChatSessionBinding>> {
    let row = sqlx::query(
        r#"SELECT id, chat_session_id, installation_id, channel_type, channel_chat_id, chat_type, last_message_id, last_thread_id, config, created_at, pending_fresh FROM channel_chat_session_binding
WHERE chat_session_id = $1
  AND channel_type = $2"#
    )
        .bind(chat_session_id)
        .bind(channel_type)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelChatSessionBinding {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        channel_chat_id: row.try_get(4)?,
        chat_type: row.try_get(5)?,
        last_message_id: row.try_get(6)?,
        last_thread_id: row.try_get(7)?,
        config: row.try_get(8)?,
        created_at: row.try_get(9)?,
        pending_fresh: row.try_get(10)?,
    }))
}

pub async fn get_channel_chat_session_binding_by_session_any(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Option<ChannelChatSessionBinding>> {
    let row = sqlx::query(
        r#"SELECT id, chat_session_id, installation_id, channel_type, channel_chat_id, chat_type, last_message_id, last_thread_id, config, created_at, pending_fresh FROM channel_chat_session_binding
WHERE chat_session_id = $1"#
    )
        .bind(chat_session_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelChatSessionBinding {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        installation_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        channel_chat_id: row.try_get(4)?,
        chat_type: row.try_get(5)?,
        last_message_id: row.try_get(6)?,
        last_thread_id: row.try_get(7)?,
        config: row.try_get(8)?,
        created_at: row.try_get(9)?,
        pending_fresh: row.try_get(10)?,
    }))
}

pub async fn get_channel_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    channel_type: &str,
) -> anyhow::Result<Option<ChannelInstallation>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, agent_id, channel_type, config, status, ws_lease_token, ws_lease_expires_at, installer_user_id, installed_at, created_at, updated_at FROM channel_installation
WHERE id = $1 AND channel_type = $2"#
    )
        .bind(id)
        .bind(channel_type)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        config: row.try_get(4)?,
        status: row.try_get(5)?,
        ws_lease_token: row.try_get(6)?,
        ws_lease_expires_at: row.try_get(7)?,
        installer_user_id: row.try_get(8)?,
        installed_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

pub async fn get_channel_installation_by_app_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    channel_type: &str,
    app_id: &str,
) -> anyhow::Result<Option<ChannelInstallation>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, agent_id, channel_type, config, status, ws_lease_token, ws_lease_expires_at, installer_user_id, installed_at, created_at, updated_at FROM channel_installation
WHERE channel_type = $1
  AND config ->> 'app_id' = $2::text"#
    )
        .bind(channel_type)
        .bind(app_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        config: row.try_get(4)?,
        status: row.try_get(5)?,
        ws_lease_token: row.try_get(6)?,
        ws_lease_expires_at: row.try_get(7)?,
        installer_user_id: row.try_get(8)?,
        installed_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

pub async fn get_channel_installation_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
    channel_type: &str,
) -> anyhow::Result<Option<ChannelInstallation>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, agent_id, channel_type, config, status, ws_lease_token, ws_lease_expires_at, installer_user_id, installed_at, created_at, updated_at FROM channel_installation
WHERE id = $1
  AND workspace_id = $2
  AND channel_type = $3"#
    )
        .bind(id)
        .bind(workspace_id)
        .bind(channel_type)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        config: row.try_get(4)?,
        status: row.try_get(5)?,
        ws_lease_token: row.try_get(6)?,
        ws_lease_expires_at: row.try_get(7)?,
        installer_user_id: row.try_get(8)?,
        installed_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetChannelInstallationOwnerByAppIDRow {
    pub workspace_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub agent_archived_at: Option<DateTime<Utc>>,
}

pub async fn get_channel_installation_owner_by_app_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    channel_type: &str,
    app_id: &str,
) -> anyhow::Result<Option<GetChannelInstallationOwnerByAppIDRow>> {
    let row = sqlx::query(
        r#"SELECT ci.workspace_id, ci.agent_id, a.archived_at AS agent_archived_at
FROM channel_installation ci
LEFT JOIN agent a ON a.id = ci.agent_id
WHERE ci.channel_type = $1
  AND ci.config ->> 'app_id' = $2::text"#,
    )
    .bind(channel_type)
    .bind(app_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetChannelInstallationOwnerByAppIDRow {
        workspace_id: row.try_get(0)?,
        agent_id: row.try_get(1)?,
        agent_archived_at: row.try_get(2)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GetChannelInstallationSlotOwnerByAppIDRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub status: String,
    pub agent_archived_at: Option<DateTime<Utc>>,
    pub agent_exists: bool,
    pub workspace_exists: bool,
}

pub async fn get_channel_installation_slot_owner_by_app_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    channel_type: &str,
    app_id: &str,
) -> anyhow::Result<Option<GetChannelInstallationSlotOwnerByAppIDRow>> {
    let row = sqlx::query(
        r#"SELECT ci.id, ci.workspace_id, ci.agent_id, ci.status,
       a.archived_at AS agent_archived_at,
       (ci.agent_id IS NULL OR a.id IS NOT NULL)::boolean AS agent_exists,
       (w.id IS NOT NULL)::boolean AS workspace_exists
FROM channel_installation ci
LEFT JOIN agent a ON a.id = ci.agent_id
LEFT JOIN workspace w ON w.id = ci.workspace_id
WHERE ci.channel_type = $1
  AND ci.config ->> 'app_id' = $2::text"#,
    )
    .bind(channel_type)
    .bind(app_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(GetChannelInstallationSlotOwnerByAppIDRow {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        status: row.try_get(3)?,
        agent_archived_at: row.try_get(4)?,
        agent_exists: row.try_get(5)?,
        workspace_exists: row.try_get(6)?,
    }))
}

pub async fn get_channel_outbound_card_by_task(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task_id: Uuid,
    channel_type: &str,
) -> anyhow::Result<Option<ChannelOutboundCardMessage>> {
    let row = sqlx::query(
        r#"SELECT id, chat_session_id, task_id, channel_type, channel_chat_id, channel_card_message_id, status, last_patched_at, created_at FROM channel_outbound_card_message
WHERE task_id = $1
  AND channel_type = $2"#
    )
        .bind(task_id)
        .bind(channel_type)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelOutboundCardMessage {
        id: row.try_get(0)?,
        chat_session_id: row.try_get(1)?,
        task_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        channel_chat_id: row.try_get(4)?,
        channel_card_message_id: row.try_get(5)?,
        status: row.try_get(6)?,
        last_patched_at: row.try_get(7)?,
        created_at: row.try_get(8)?,
    }))
}

pub async fn get_channel_user_binding_by_user_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    channel_user_id: &str,
) -> anyhow::Result<Option<ChannelUserBinding>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, patchbay_user_id, installation_id, channel_type, channel_user_id, config, bound_at FROM channel_user_binding
WHERE installation_id = $1 AND channel_user_id = $2"#
    )
        .bind(installation_id)
        .bind(channel_user_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelUserBinding {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        patchbay_user_id: row.try_get(2)?,
        installation_id: row.try_get(3)?,
        channel_type: row.try_get(4)?,
        channel_user_id: row.try_get(5)?,
        config: row.try_get(6)?,
        bound_at: row.try_get(7)?,
    }))
}

pub async fn list_active_channel_installations(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    channel_type: &str,
) -> anyhow::Result<Vec<ChannelInstallation>> {
    let rows = sqlx::query(
        r#"SELECT ci.id, ci.workspace_id, ci.agent_id, ci.channel_type, ci.config, ci.status, ci.ws_lease_token, ci.ws_lease_expires_at, ci.installer_user_id, ci.installed_at, ci.created_at, ci.updated_at FROM channel_installation ci
JOIN workspace w ON w.id = ci.workspace_id
LEFT JOIN agent a ON a.id = ci.agent_id
WHERE ci.status = 'active'
  AND ci.channel_type = $1
  AND (ci.agent_id IS NULL OR a.id IS NOT NULL)
ORDER BY ci.created_at ASC"#
    )
        .bind(channel_type)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ChannelInstallation {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            agent_id: row.try_get(2)?,
            channel_type: row.try_get(3)?,
            config: row.try_get(4)?,
            status: row.try_get(5)?,
            ws_lease_token: row.try_get(6)?,
            ws_lease_expires_at: row.try_get(7)?,
            installer_user_id: row.try_get(8)?,
            installed_at: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
        });
    }
    Ok(out)
}

/// Pages active installations that still need the Lark bot union-id upgrade.
/// The cursor follows the same created_at ordering as the legacy all-rows
/// query, with id as the stable tie-breaker.
pub async fn list_active_channel_installations_missing_bot_union_id_after(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    channel_type: &str,
    after_created_at: Option<DateTime<Utc>>,
    after_id: Option<Uuid>,
    limit: i64,
) -> anyhow::Result<Vec<ChannelInstallation>> {
    let rows = sqlx::query(
        r#"SELECT ci.id, ci.workspace_id, ci.agent_id, ci.channel_type, ci.config, ci.status, ci.ws_lease_token, ci.ws_lease_expires_at, ci.installer_user_id, ci.installed_at, ci.created_at, ci.updated_at
FROM channel_installation ci
JOIN workspace w ON w.id = ci.workspace_id
LEFT JOIN agent a ON a.id = ci.agent_id
WHERE ci.status = 'active'
  AND ci.channel_type = $1
  AND (ci.agent_id IS NULL OR a.id IS NOT NULL)
  AND COALESCE(ci.config ->> 'bot_union_id', '') = ''
  AND ($2::timestamptz IS NULL OR (ci.created_at, ci.id) > ($2, $3))
ORDER BY ci.created_at ASC, ci.id ASC
LIMIT $4"#,
    )
    .bind(channel_type)
    .bind(after_created_at)
    .bind(after_id)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ChannelInstallation {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            agent_id: row.try_get(2)?,
            channel_type: row.try_get(3)?,
            config: row.try_get(4)?,
            status: row.try_get(5)?,
            ws_lease_token: row.try_get(6)?,
            ws_lease_expires_at: row.try_get(7)?,
            installer_user_id: row.try_get(8)?,
            installed_at: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
        });
    }
    Ok(out)
}

pub async fn list_all_active_channel_installations(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
) -> anyhow::Result<Vec<ChannelInstallation>> {
    let rows = sqlx::query(
        r#"SELECT ci.id, ci.workspace_id, ci.agent_id, ci.channel_type, ci.config, ci.status, ci.ws_lease_token, ci.ws_lease_expires_at, ci.installer_user_id, ci.installed_at, ci.created_at, ci.updated_at FROM channel_installation ci
JOIN workspace w ON w.id = ci.workspace_id
LEFT JOIN agent a ON a.id = ci.agent_id
WHERE ci.status = 'active'
  AND (ci.agent_id IS NULL OR a.id IS NOT NULL)
ORDER BY ci.created_at ASC"#
    )
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ChannelInstallation {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            agent_id: row.try_get(2)?,
            channel_type: row.try_get(3)?,
            config: row.try_get(4)?,
            status: row.try_get(5)?,
            ws_lease_token: row.try_get(6)?,
            ws_lease_expires_at: row.try_get(7)?,
            installer_user_id: row.try_get(8)?,
            installed_at: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
        });
    }
    Ok(out)
}

pub async fn list_channel_inbound_audit_by_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    limit: i32,
    offset: i32,
) -> anyhow::Result<Vec<ChannelInboundAudit>> {
    let rows = sqlx::query(
        r#"SELECT id, installation_id, channel_type, channel_chat_id, event_type, channel_event_id, channel_message_id, drop_reason, received_at FROM channel_inbound_audit
WHERE installation_id = $1
ORDER BY received_at DESC
LIMIT $2 OFFSET $3"#
    )
        .bind(installation_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ChannelInboundAudit {
            id: row.try_get(0)?,
            installation_id: row.try_get(1)?,
            channel_type: row.try_get(2)?,
            channel_chat_id: row.try_get(3)?,
            event_type: row.try_get(4)?,
            channel_event_id: row.try_get(5)?,
            channel_message_id: row.try_get(6)?,
            drop_reason: row.try_get(7)?,
            received_at: row.try_get(8)?,
        });
    }
    Ok(out)
}

pub async fn list_channel_installations_by_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    channel_type: &str,
) -> anyhow::Result<Vec<ChannelInstallation>> {
    let rows = sqlx::query(
        r#"SELECT id, workspace_id, agent_id, channel_type, config, status, ws_lease_token, ws_lease_expires_at, installer_user_id, installed_at, created_at, updated_at FROM channel_installation
WHERE workspace_id = $1
  AND channel_type = $2
ORDER BY created_at ASC"#
    )
        .bind(workspace_id)
        .bind(channel_type)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ChannelInstallation {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            agent_id: row.try_get(2)?,
            channel_type: row.try_get(3)?,
            config: row.try_get(4)?,
            status: row.try_get(5)?,
            ws_lease_token: row.try_get(6)?,
            ws_lease_expires_at: row.try_get(7)?,
            installer_user_id: row.try_get(8)?,
            installed_at: row.try_get(9)?,
            created_at: row.try_get(10)?,
            updated_at: row.try_get(11)?,
        });
    }
    Ok(out)
}

pub async fn lock_channel_chat_session_pending_fresh(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"SELECT pending_fresh FROM channel_chat_session_binding
WHERE chat_session_id = $1
FOR UPDATE"#,
    )
    .bind(chat_session_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn lock_channel_installation_app_id_slot(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    channel_type: &str,
    app_id: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"SELECT pg_advisory_xact_lock(
    hashtext($1::text),
    hashtext($2::text)
)"#,
    )
    .bind(channel_type)
    .bind(app_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn lock_channel_installation_agent_slot(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    channel_type: &str,
    workspace_id: Uuid,
    agent_id: Uuid,
) -> anyhow::Result<u64> {
    let key = format!("{workspace_id}:{agent_id}");
    let result =
        sqlx::query(r#"SELECT pg_advisory_xact_lock(hashtext($1::text), hashtext($2::text))"#)
            .bind(channel_type)
            .bind(key)
            .execute(executor)
            .await?;
    Ok(result.rows_affected())
}

/// Serializes the single workspace-scoped installation for a provider.
/// Workspace-scoped installations have no Agent selected until a user sends
/// `/agents` from the connected channel.
pub async fn lock_channel_installation_hub_slot(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    channel_type: &str,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let key = workspace_id.to_string();
    let result =
        sqlx::query(r#"SELECT pg_advisory_xact_lock(hashtext($1::text), hashtext($2::text))"#)
            .bind(channel_type)
            .bind(key)
            .execute(executor)
            .await?;
    Ok(result.rows_affected())
}

pub async fn mark_channel_chat_session_pending_fresh(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
) -> anyhow::Result<Option<bool>> {
    let row = sqlx::query(
        r#"UPDATE channel_chat_session_binding
SET pending_fresh = TRUE
WHERE chat_session_id = $1
RETURNING pending_fresh"#,
    )
    .bind(chat_session_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn mark_channel_inbound_dedup_processed(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    message_id: &str,
    claim_token: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE channel_inbound_message_dedup
SET processed_at = now()
WHERE installation_id = $1
  AND message_id = $2
  AND claim_token = $3
  AND processed_at IS NULL"#,
    )
    .bind(installation_id)
    .bind(message_id)
    .bind(claim_token)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn null_channel_inbound_audit_installation_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE channel_inbound_audit
SET installation_id = NULL
WHERE installation_id = $1"#,
    )
    .bind(installation_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn purge_channel_inbound_dedup(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    received_at: Option<DateTime<Utc>>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM channel_inbound_message_dedup
WHERE received_at < $1"#,
    )
    .bind(received_at)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn purge_expired_channel_binding_tokens(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    expires_at: Option<DateTime<Utc>>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM channel_binding_token
WHERE expires_at < $1"#,
    )
    .bind(expires_at)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn reclaim_dead_channel_installation_by_app_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    channel_type: &str,
    app_id: &str,
    workspace_id: Uuid,
    agent_id: Uuid,
) -> anyhow::Result<Option<Option<Uuid>>> {
    let row = sqlx::query(
        r#"WITH dead AS (
    DELETE FROM channel_installation ci
    WHERE ci.channel_type = $1
      AND ci.config ->> 'app_id' = $2::text
      AND (
            (ci.status = 'revoked'
                AND NOT (ci.workspace_id = $3
                         AND ci.agent_id IS NOT DISTINCT FROM NULLIF($4, '00000000-0000-0000-0000-000000000000'::uuid)))
         OR NOT EXISTS (SELECT 1 FROM workspace w WHERE w.id = ci.workspace_id)
         OR (ci.agent_id IS NOT NULL
             AND NOT EXISTS (SELECT 1 FROM agent a WHERE a.id = ci.agent_id))
      )
    RETURNING ci.id
),
cleared_chat_sessions AS (
    DELETE FROM channel_chat_session_binding
    WHERE installation_id IN (SELECT id FROM dead)
    RETURNING chat_session_id
),
cleared_dingtalk_group_routes AS (
    DELETE FROM dingtalk_group_route
    WHERE installation_id IN (SELECT id FROM dead)
),
cleared_outbound_cards AS (
    -- channel_outbound_card_message is keyed by chat_session_id (no installation_id,
    -- no FK), so it is reached through the just-removed chat-session bindings. On an
    -- orphan reclaim the chat_session row itself is already cascade-gone, but its
    -- binding survived and still carries the id — the only reliable link back.
    DELETE FROM channel_outbound_card_message
    WHERE chat_session_id IN (SELECT chat_session_id FROM cleared_chat_sessions)
),
cleared_binding_tokens AS (
    DELETE FROM channel_binding_token
    WHERE installation_id IN (SELECT id FROM dead)
),
cleared_user_bindings AS (
    DELETE FROM channel_user_binding
    WHERE installation_id IN (SELECT id FROM dead)
),
cleared_inbound_dedup AS (
    DELETE FROM channel_inbound_message_dedup
    WHERE installation_id IN (SELECT id FROM dead)
),
cleared_receive_state AS (
    DELETE FROM channel_receive_state
    WHERE installation_id IN (SELECT id FROM dead)
),
detached_audit AS (
    -- Reclaim keeps the DETACH semantics: the workspace still exists, so a
    -- NULL-installation audit row stays meaningful for operator triage. The hard-
    -- delete paths (DeleteWorkspace / runtime teardown) purge audit outright.
    UPDATE channel_inbound_audit SET installation_id = NULL
    WHERE installation_id IN (SELECT id FROM dead)
)
SELECT id FROM dead"#,
    )
    .bind(channel_type)
    .bind(app_id)
    .bind(workspace_id)
    .bind(agent_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

/// Deletes the provider-owned state for an installation that is being
/// replaced by a different upstream account. Audit rows remain available but
/// are detached from the deleted installation.
pub async fn delete_channel_installation_for_replacement(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
) -> anyhow::Result<bool> {
    let row = sqlx::query_scalar::<_, Uuid>(
        r#"WITH doomed AS (
    DELETE FROM channel_installation WHERE id = $1 RETURNING id
),
cleared_chat_sessions AS (
    DELETE FROM channel_chat_session_binding
    WHERE installation_id IN (SELECT id FROM doomed)
    RETURNING chat_session_id
),
cleared_outbound_cards AS (
    DELETE FROM channel_outbound_card_message
    WHERE chat_session_id IN (SELECT chat_session_id FROM cleared_chat_sessions)
),
cleared_group_routes AS (
    DELETE FROM dingtalk_group_route
    WHERE installation_id IN (SELECT id FROM doomed)
),
cleared_binding_tokens AS (
    DELETE FROM channel_binding_token
    WHERE installation_id IN (SELECT id FROM doomed)
),
cleared_user_bindings AS (
    DELETE FROM channel_user_binding
    WHERE installation_id IN (SELECT id FROM doomed)
),
cleared_inbound_dedup AS (
    DELETE FROM channel_inbound_message_dedup
    WHERE installation_id IN (SELECT id FROM doomed)
),
cleared_receive_state AS (
    DELETE FROM channel_receive_state
    WHERE installation_id IN (SELECT id FROM doomed)
),
detached_audit AS (
    UPDATE channel_inbound_audit SET installation_id = NULL
    WHERE installation_id IN (SELECT id FROM doomed)
)
SELECT id FROM doomed"#,
    )
    .bind(installation_id)
    .fetch_optional(executor)
    .await?;
    Ok(row.is_some())
}

pub async fn record_channel_inbound_drop(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    channel_type: &str,
    event_type: &str,
    drop_reason: &str,
    installation_id: Option<Uuid>,
    channel_chat_id: Option<&str>,
    channel_event_id: Option<&str>,
    channel_message_id: Option<&str>,
    id: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"INSERT INTO channel_inbound_audit (
    installation_id, channel_type, channel_chat_id, event_type,
    channel_event_id, channel_message_id, drop_reason, id
) VALUES (
    $4,
    $1,
    $5,
    $2,
    $6,
    $7,
    $3,
    COALESCE($8::uuid, gen_random_uuid())
)"#,
    )
    .bind(channel_type)
    .bind(event_type)
    .bind(drop_reason)
    .bind(installation_id)
    .bind(channel_chat_id)
    .bind(channel_event_id)
    .bind(channel_message_id)
    .bind(id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn record_channel_media_pending_object(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    storage_key: &str,
    workspace_id: Uuid,
    chat_message_id: Uuid,
    storage_url: &str,
    installation_id: Uuid,
) -> anyhow::Result<Option<String>> {
    let row = sqlx::query(
        r#"INSERT INTO channel_media_pending_object (
    storage_key, workspace_id, chat_message_id, storage_url, installation_id
)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (storage_key) DO UPDATE
SET created_at = now(), next_attempt_at = now(),
    chat_message_id = EXCLUDED.chat_message_id,
    storage_url = EXCLUDED.storage_url
WHERE channel_media_pending_object.state = 'pending'
  AND channel_media_pending_object.workspace_id = EXCLUDED.workspace_id
RETURNING storage_key"#,
    )
    .bind(storage_key)
    .bind(workspace_id)
    .bind(chat_message_id)
    .bind(storage_url)
    .bind(installation_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(row.try_get(0)?))
}

pub async fn release_channel_inbound_dedup(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    installation_id: Uuid,
    message_id: &str,
    claim_token: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"DELETE FROM channel_inbound_message_dedup
WHERE installation_id = $1
  AND message_id = $2
  AND claim_token = $3
  AND processed_at IS NULL"#,
    )
    .bind(installation_id)
    .bind(message_id)
    .bind(claim_token)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn release_channel_media_pending_object(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    backoff: sqlx::postgres::types::PgInterval,
    last_error: Option<&str>,
    storage_key: &str,
    workspace_id: Uuid,
    lease_token: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE channel_media_pending_object
SET lease_token = NULL,
    lease_expires_at = NULL,
    next_attempt_at = now() + $1::interval,
    last_error = $2
WHERE storage_key = $3
  AND workspace_id = $4
  AND lease_token = $5"#,
    )
    .bind(backoff)
    .bind(last_error)
    .bind(storage_key)
    .bind(workspace_id)
    .bind(lease_token)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn release_channel_ws_lease(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    current_token: Option<&str>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE channel_installation
SET ws_lease_token      = NULL,
    ws_lease_expires_at = NULL,
    updated_at          = now()
WHERE id = $1
  AND ws_lease_token = $2"#,
    )
    .bind(id)
    .bind(current_token)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

/// Mirrors an externally-held channel lease into the installation row so the
/// public health projection remains useful when the runtime lease backend is
/// Redis. Redis is still the fencing authority in that mode; this write is a
/// token-bearing observation only and is deliberately not a second CAS.
pub async fn mirror_channel_ws_lease(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    token: &str,
    expires_at: DateTime<Utc>,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"UPDATE channel_installation
SET ws_lease_token = $2,
    ws_lease_expires_at = $3,
    updated_at = now()
WHERE id = $1
  AND status = 'active'"#,
    )
    .bind(id)
    .bind(token)
    .bind(expires_at)
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Clears the Redis lease observation without allowing a stale owner to erase
/// a successor's newer observation.
pub async fn clear_mirrored_channel_ws_lease(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    token: &str,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"UPDATE channel_installation
SET ws_lease_token = NULL,
    ws_lease_expires_at = NULL,
    updated_at = now()
WHERE id = $1
  AND ws_lease_token = $2"#,
    )
    .bind(id)
    .bind(token)
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

pub async fn set_channel_installation_config(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    config: &serde_json::Value,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE channel_installation
SET config = $2, updated_at = now()
WHERE id = $1"#,
    )
    .bind(id)
    .bind(config)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

/// Atomically stamps bot_union_id only while it is still absent. Unlike the
/// legacy read/modify/write helper this cannot overwrite a concurrent secret
/// rotation or reinstall, and a concurrent backfill winner returns false.
pub async fn set_channel_installation_bot_union_id_if_missing(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    channel_type: &str,
    bot_union_id: &str,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        r#"UPDATE channel_installation
SET config = jsonb_set(config, '{bot_union_id}', to_jsonb($3::text)),
    updated_at = now()
WHERE id = $1
  AND channel_type = $2
  AND status = 'active'
  AND COALESCE(config ->> 'bot_union_id', '') = ''
RETURNING id"#,
    )
    .bind(id)
    .bind(channel_type)
    .bind(bot_union_id)
    .fetch_optional(executor)
    .await?;
    Ok(row.is_some())
}

pub async fn set_channel_installation_status(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    status: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE channel_installation
SET status = $2, updated_at = now()
WHERE id = $1"#,
    )
    .bind(id)
    .bind(status)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn tombstone_channel_media_pending_object(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    redelete_delay: sqlx::postgres::types::PgInterval,
    tombstone_pass: i32,
    storage_key: &str,
    workspace_id: Uuid,
    lease_token: Uuid,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE channel_media_pending_object
SET state = 'tombstoned',
    lease_token = NULL,
    lease_expires_at = NULL,
    next_attempt_at = now() + $1::interval,
    -- The pass index lives in its own column: a failed re-delete writes
    -- last_error, so carrying the schedule position there would reset the
    -- walk on every failure and a flaky store could keep the row alive
    -- indefinitely. The delete that got here succeeded, so any previous
    -- failure text is stale.
    tombstone_pass = $2,
    last_error = NULL
WHERE storage_key = $3
  AND workspace_id = $4
  AND lease_token = $5"#,
    )
    .bind(redelete_delay)
    .bind(tombstone_pass)
    .bind(storage_key)
    .bind(workspace_id)
    .bind(lease_token)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn update_channel_chat_session_binding_reply_target(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    chat_session_id: Uuid,
    last_message_id: Option<&str>,
    last_thread_id: Option<&str>,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE channel_chat_session_binding
SET last_message_id = $2,
    last_thread_id  = $3
WHERE chat_session_id = $1"#,
    )
    .bind(chat_session_id)
    .bind(last_message_id)
    .bind(last_thread_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn update_channel_outbound_card_status(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    status: &str,
) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r#"UPDATE channel_outbound_card_message
SET status = $2,
    last_patched_at = now()
WHERE id = $1"#,
    )
    .bind(id)
    .bind(status)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

pub async fn upsert_channel_installation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    agent_id: Uuid,
    channel_type: &str,
    config: &serde_json::Value,
    installer_user_id: Uuid,
) -> anyhow::Result<Option<ChannelInstallation>> {
    let row = sqlx::query(
        r#"INSERT INTO channel_installation (
    workspace_id, agent_id, channel_type, config, installer_user_id
) VALUES (
    $1, $2, $3, $4, $5
)
ON CONFLICT (workspace_id, agent_id, channel_type) DO UPDATE SET
    channel_type      = EXCLUDED.channel_type,
    config            = EXCLUDED.config,
    installer_user_id = EXCLUDED.installer_user_id,
    status            = 'active',
    installed_at      = now(),
    updated_at        = now()
RETURNING id, workspace_id, agent_id, channel_type, config, status, ws_lease_token, ws_lease_expires_at, installer_user_id, installed_at, created_at, updated_at"#
    )
        .bind(workspace_id)
        .bind(agent_id)
        .bind(channel_type)
        .bind(config)
        .bind(installer_user_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        config: row.try_get(4)?,
        status: row.try_get(5)?,
        ws_lease_token: row.try_get(6)?,
        ws_lease_expires_at: row.try_get(7)?,
        installer_user_id: row.try_get(8)?,
        installed_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

/// Inserts or reactivates a workspace-scoped provider installation. The
/// partial unique index guarantees one active hub slot per workspace/type.
pub async fn upsert_channel_installation_hub(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    channel_type: &str,
    config: &serde_json::Value,
    installer_user_id: Uuid,
) -> anyhow::Result<Option<ChannelInstallation>> {
    let row = sqlx::query(
        r#"INSERT INTO channel_installation (
    workspace_id, agent_id, channel_type, config, installer_user_id
) VALUES (
    $1, NULL, $2, $3, $4
)
ON CONFLICT (workspace_id, channel_type) WHERE agent_id IS NULL DO UPDATE SET
    config            = EXCLUDED.config,
    installer_user_id = EXCLUDED.installer_user_id,
    status            = 'active',
    installed_at      = now(),
    updated_at        = now()
RETURNING id, workspace_id, agent_id, channel_type, config, status, ws_lease_token, ws_lease_expires_at, installer_user_id, installed_at, created_at, updated_at"#,
    )
    .bind(workspace_id)
    .bind(channel_type)
    .bind(config)
    .bind(installer_user_id)
    .fetch_optional(executor)
    .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        config: row.try_get(4)?,
        status: row.try_get(5)?,
        ws_lease_token: row.try_get(6)?,
        ws_lease_expires_at: row.try_get(7)?,
        installer_user_id: row.try_get(8)?,
        installed_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}

pub async fn upsert_channel_installation_by_app_id(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    agent_id: Uuid,
    channel_type: &str,
    config: &serde_json::Value,
    installer_user_id: Uuid,
) -> anyhow::Result<Option<ChannelInstallation>> {
    let row = sqlx::query(
        r#"INSERT INTO channel_installation (
    workspace_id, agent_id, channel_type, config, installer_user_id
) VALUES (
    $1, $2, $3, $4, $5
)
ON CONFLICT (channel_type, (config ->> 'app_id')) DO UPDATE SET
    agent_id          = EXCLUDED.agent_id,
    config            = EXCLUDED.config,
    installer_user_id = EXCLUDED.installer_user_id,
    status            = 'active',
    installed_at      = now(),
    updated_at        = now()
WHERE channel_installation.workspace_id = EXCLUDED.workspace_id
RETURNING id, workspace_id, agent_id, channel_type, config, status, ws_lease_token, ws_lease_expires_at, installer_user_id, installed_at, created_at, updated_at"#
    )
        .bind(workspace_id)
        .bind(agent_id)
        .bind(channel_type)
        .bind(config)
        .bind(installer_user_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(ChannelInstallation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        agent_id: row.try_get(2)?,
        channel_type: row.try_get(3)?,
        config: row.try_get(4)?,
        status: row.try_get(5)?,
        ws_lease_token: row.try_get(6)?,
        ws_lease_expires_at: row.try_get(7)?,
        installer_user_id: row.try_get(8)?,
        installed_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        updated_at: row.try_get(11)?,
    }))
}
