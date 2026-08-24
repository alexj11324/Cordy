//! Port of server/pkg/db/queries/webhook_delivery.sql (generated webhook_delivery.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn acknowledge_webhook_delivery(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    response_status: Option<i32>,
    response_body: Option<&str>,
) -> anyhow::Result<Option<WebhookDelivery>> {
    let row = sqlx::query(
        r#"UPDATE webhook_delivery
SET response_status = $2,
    response_body = $3,
    last_attempt_at = now()
WHERE id = $1
RETURNING id, workspace_id, autopilot_id, trigger_id, provider, event, dedupe_key, dedupe_source, signature_status, status, attempt_count, selected_headers, content_type, raw_body, response_status, response_body, autopilot_run_id, replayed_from_delivery_id, error, received_at, last_attempt_at, created_at, available_at, lease_token, lease_expires_at, dispatch_attempts, reason_code, replay_idempotency_key"#
    )
        .bind(id)
        .bind(response_status)
        .bind(response_body)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WebhookDelivery {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        autopilot_id: row.try_get(2)?,
        trigger_id: row.try_get(3)?,
        provider: row.try_get(4)?,
        event: row.try_get(5)?,
        dedupe_key: row.try_get(6)?,
        dedupe_source: row.try_get(7)?,
        signature_status: row.try_get(8)?,
        status: row.try_get(9)?,
        attempt_count: row.try_get(10)?,
        selected_headers: row.try_get(11)?,
        content_type: row.try_get(12)?,
        raw_body: row.try_get(13)?,
        response_status: row.try_get(14)?,
        response_body: row.try_get(15)?,
        autopilot_run_id: row.try_get(16)?,
        replayed_from_delivery_id: row.try_get(17)?,
        error: row.try_get(18)?,
        received_at: row.try_get(19)?,
        last_attempt_at: row.try_get(20)?,
        created_at: row.try_get(21)?,
        available_at: row.try_get(22)?,
        lease_token: row.try_get(23)?,
        lease_expires_at: row.try_get(24)?,
        dispatch_attempts: row.try_get(25)?,
        reason_code: row.try_get(26)?,
        replay_idempotency_key: row.try_get(27)?,
    }))
}

pub async fn bump_webhook_delivery_attempt(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<WebhookDelivery>> {
    let row = sqlx::query(
        r#"UPDATE webhook_delivery
SET attempt_count = attempt_count + 1,
    last_attempt_at = now()
WHERE id = $1
RETURNING id, workspace_id, autopilot_id, trigger_id, provider, event, dedupe_key, dedupe_source, signature_status, status, attempt_count, selected_headers, content_type, raw_body, response_status, response_body, autopilot_run_id, replayed_from_delivery_id, error, received_at, last_attempt_at, created_at, available_at, lease_token, lease_expires_at, dispatch_attempts, reason_code, replay_idempotency_key"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WebhookDelivery {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        autopilot_id: row.try_get(2)?,
        trigger_id: row.try_get(3)?,
        provider: row.try_get(4)?,
        event: row.try_get(5)?,
        dedupe_key: row.try_get(6)?,
        dedupe_source: row.try_get(7)?,
        signature_status: row.try_get(8)?,
        status: row.try_get(9)?,
        attempt_count: row.try_get(10)?,
        selected_headers: row.try_get(11)?,
        content_type: row.try_get(12)?,
        raw_body: row.try_get(13)?,
        response_status: row.try_get(14)?,
        response_body: row.try_get(15)?,
        autopilot_run_id: row.try_get(16)?,
        replayed_from_delivery_id: row.try_get(17)?,
        error: row.try_get(18)?,
        received_at: row.try_get(19)?,
        last_attempt_at: row.try_get(20)?,
        created_at: row.try_get(21)?,
        available_at: row.try_get(22)?,
        lease_token: row.try_get(23)?,
        lease_expires_at: row.try_get(24)?,
        dispatch_attempts: row.try_get(25)?,
        reason_code: row.try_get(26)?,
        replay_idempotency_key: row.try_get(27)?,
    }))
}

pub async fn claim_queued_webhook_delivery(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
) -> anyhow::Result<Option<WebhookDelivery>> {
    let row = sqlx::query(
        r#"WITH candidate AS (
    SELECT id
    FROM webhook_delivery
    WHERE status = 'queued'
      AND available_at <= now()
      AND (lease_expires_at IS NULL OR lease_expires_at <= now())
    ORDER BY available_at, created_at
    FOR UPDATE SKIP LOCKED
    LIMIT 1
)
UPDATE webhook_delivery AS d
SET lease_token = gen_random_uuid(),
    lease_expires_at = now() + interval '2 minutes'
FROM candidate
WHERE d.id = candidate.id
RETURNING d.id, d.workspace_id, d.autopilot_id, d.trigger_id, d.provider, d.event, d.dedupe_key, d.dedupe_source, d.signature_status, d.status, d.attempt_count, d.selected_headers, d.content_type, d.raw_body, d.response_status, d.response_body, d.autopilot_run_id, d.replayed_from_delivery_id, d.error, d.received_at, d.last_attempt_at, d.created_at, d.available_at, d.lease_token, d.lease_expires_at, d.dispatch_attempts, d.reason_code, d.replay_idempotency_key"#
    )
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WebhookDelivery {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        autopilot_id: row.try_get(2)?,
        trigger_id: row.try_get(3)?,
        provider: row.try_get(4)?,
        event: row.try_get(5)?,
        dedupe_key: row.try_get(6)?,
        dedupe_source: row.try_get(7)?,
        signature_status: row.try_get(8)?,
        status: row.try_get(9)?,
        attempt_count: row.try_get(10)?,
        selected_headers: row.try_get(11)?,
        content_type: row.try_get(12)?,
        raw_body: row.try_get(13)?,
        response_status: row.try_get(14)?,
        response_body: row.try_get(15)?,
        autopilot_run_id: row.try_get(16)?,
        replayed_from_delivery_id: row.try_get(17)?,
        error: row.try_get(18)?,
        received_at: row.try_get(19)?,
        last_attempt_at: row.try_get(20)?,
        created_at: row.try_get(21)?,
        available_at: row.try_get(22)?,
        lease_token: row.try_get(23)?,
        lease_expires_at: row.try_get(24)?,
        dispatch_attempts: row.try_get(25)?,
        reason_code: row.try_get(26)?,
        replay_idempotency_key: row.try_get(27)?,
    }))
}

pub async fn complete_claimed_webhook_delivery(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    lease_token: Uuid,
    status: &str,
    autopilot_run_id: Option<Uuid>,
    error: Option<&str>,
    reason_code: Option<&str>,
) -> anyhow::Result<Option<WebhookDelivery>> {
    let row = sqlx::query(
        r#"UPDATE webhook_delivery
SET status = $3,
    autopilot_run_id = $4,
    dispatch_attempts = dispatch_attempts + 1,
    error = $5,
    reason_code = $6,
    lease_token = NULL,
    lease_expires_at = NULL,
    last_attempt_at = now()
WHERE id = $1
  AND lease_token = $2
  AND status = 'queued'
RETURNING id, workspace_id, autopilot_id, trigger_id, provider, event, dedupe_key, dedupe_source, signature_status, status, attempt_count, selected_headers, content_type, raw_body, response_status, response_body, autopilot_run_id, replayed_from_delivery_id, error, received_at, last_attempt_at, created_at, available_at, lease_token, lease_expires_at, dispatch_attempts, reason_code, replay_idempotency_key"#
    )
        .bind(id)
        .bind(lease_token)
        .bind(status)
        .bind(autopilot_run_id)
        .bind(error)
        .bind(reason_code)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WebhookDelivery {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        autopilot_id: row.try_get(2)?,
        trigger_id: row.try_get(3)?,
        provider: row.try_get(4)?,
        event: row.try_get(5)?,
        dedupe_key: row.try_get(6)?,
        dedupe_source: row.try_get(7)?,
        signature_status: row.try_get(8)?,
        status: row.try_get(9)?,
        attempt_count: row.try_get(10)?,
        selected_headers: row.try_get(11)?,
        content_type: row.try_get(12)?,
        raw_body: row.try_get(13)?,
        response_status: row.try_get(14)?,
        response_body: row.try_get(15)?,
        autopilot_run_id: row.try_get(16)?,
        replayed_from_delivery_id: row.try_get(17)?,
        error: row.try_get(18)?,
        received_at: row.try_get(19)?,
        last_attempt_at: row.try_get(20)?,
        created_at: row.try_get(21)?,
        available_at: row.try_get(22)?,
        lease_token: row.try_get(23)?,
        lease_expires_at: row.try_get(24)?,
        dispatch_attempts: row.try_get(25)?,
        reason_code: row.try_get(26)?,
        replay_idempotency_key: row.try_get(27)?,
    }))
}

pub async fn create_webhook_delivery(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    autopilot_id: Uuid,
    trigger_id: Uuid,
    provider: &str,
    event: &str,
    signature_status: &str,
    status: &str,
    selected_headers: &serde_json::Value,
    dedupe_key: Option<&str>,
    dedupe_source: Option<&str>,
    content_type: Option<&str>,
    raw_body: &serde_json::Value,
    replayed_from_delivery_id: Uuid,
    replay_idempotency_key: Option<&str>,
    reason_code: Option<&str>,
    id: Uuid,
) -> anyhow::Result<Option<WebhookDelivery>> {
    let row = sqlx::query(
        r#"INSERT INTO webhook_delivery (
    workspace_id, autopilot_id, trigger_id, provider, event,
    dedupe_key, dedupe_source, signature_status, status,
    selected_headers, content_type, raw_body,
    replayed_from_delivery_id, replay_idempotency_key, reason_code, id
) VALUES (
    $1, $2, $3, $4, $5,
    $9, $10, $6, $7,
    $8, $11, $12,
    $13, $14,
    $15, COALESCE($16::uuid, gen_random_uuid())
) RETURNING id, workspace_id, autopilot_id, trigger_id, provider, event, dedupe_key, dedupe_source, signature_status, status, attempt_count, selected_headers, content_type, raw_body, response_status, response_body, autopilot_run_id, replayed_from_delivery_id, error, received_at, last_attempt_at, created_at, available_at, lease_token, lease_expires_at, dispatch_attempts, reason_code, replay_idempotency_key"#
    )
        .bind(workspace_id)
        .bind(autopilot_id)
        .bind(trigger_id)
        .bind(provider)
        .bind(event)
        .bind(signature_status)
        .bind(status)
        .bind(selected_headers)
        .bind(dedupe_key)
        .bind(dedupe_source)
        .bind(content_type)
        .bind(raw_body)
        .bind(replayed_from_delivery_id)
        .bind(replay_idempotency_key)
        .bind(reason_code)
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WebhookDelivery {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        autopilot_id: row.try_get(2)?,
        trigger_id: row.try_get(3)?,
        provider: row.try_get(4)?,
        event: row.try_get(5)?,
        dedupe_key: row.try_get(6)?,
        dedupe_source: row.try_get(7)?,
        signature_status: row.try_get(8)?,
        status: row.try_get(9)?,
        attempt_count: row.try_get(10)?,
        selected_headers: row.try_get(11)?,
        content_type: row.try_get(12)?,
        raw_body: row.try_get(13)?,
        response_status: row.try_get(14)?,
        response_body: row.try_get(15)?,
        autopilot_run_id: row.try_get(16)?,
        replayed_from_delivery_id: row.try_get(17)?,
        error: row.try_get(18)?,
        received_at: row.try_get(19)?,
        last_attempt_at: row.try_get(20)?,
        created_at: row.try_get(21)?,
        available_at: row.try_get(22)?,
        lease_token: row.try_get(23)?,
        lease_expires_at: row.try_get(24)?,
        dispatch_attempts: row.try_get(25)?,
        reason_code: row.try_get(26)?,
        replay_idempotency_key: row.try_get(27)?,
    }))
}

pub async fn defer_claimed_webhook_delivery(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    lease_token: Uuid,
    available_at: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<WebhookDelivery>> {
    let row = sqlx::query(
        r#"UPDATE webhook_delivery
SET available_at = $3,
    lease_token = NULL,
    lease_expires_at = NULL
WHERE id = $1
  AND lease_token = $2
  AND status = 'queued'
RETURNING id, workspace_id, autopilot_id, trigger_id, provider, event, dedupe_key, dedupe_source, signature_status, status, attempt_count, selected_headers, content_type, raw_body, response_status, response_body, autopilot_run_id, replayed_from_delivery_id, error, received_at, last_attempt_at, created_at, available_at, lease_token, lease_expires_at, dispatch_attempts, reason_code, replay_idempotency_key"#
    )
        .bind(id)
        .bind(lease_token)
        .bind(available_at)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WebhookDelivery {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        autopilot_id: row.try_get(2)?,
        trigger_id: row.try_get(3)?,
        provider: row.try_get(4)?,
        event: row.try_get(5)?,
        dedupe_key: row.try_get(6)?,
        dedupe_source: row.try_get(7)?,
        signature_status: row.try_get(8)?,
        status: row.try_get(9)?,
        attempt_count: row.try_get(10)?,
        selected_headers: row.try_get(11)?,
        content_type: row.try_get(12)?,
        raw_body: row.try_get(13)?,
        response_status: row.try_get(14)?,
        response_body: row.try_get(15)?,
        autopilot_run_id: row.try_get(16)?,
        replayed_from_delivery_id: row.try_get(17)?,
        error: row.try_get(18)?,
        received_at: row.try_get(19)?,
        last_attempt_at: row.try_get(20)?,
        created_at: row.try_get(21)?,
        available_at: row.try_get(22)?,
        lease_token: row.try_get(23)?,
        lease_expires_at: row.try_get(24)?,
        dispatch_attempts: row.try_get(25)?,
        reason_code: row.try_get(26)?,
        replay_idempotency_key: row.try_get(27)?,
    }))
}

pub async fn get_webhook_delivery(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
) -> anyhow::Result<Option<WebhookDelivery>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, autopilot_id, trigger_id, provider, event, dedupe_key, dedupe_source, signature_status, status, attempt_count, selected_headers, content_type, raw_body, response_status, response_body, autopilot_run_id, replayed_from_delivery_id, error, received_at, last_attempt_at, created_at, available_at, lease_token, lease_expires_at, dispatch_attempts, reason_code, replay_idempotency_key FROM webhook_delivery
WHERE id = $1"#
    )
        .bind(id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WebhookDelivery {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        autopilot_id: row.try_get(2)?,
        trigger_id: row.try_get(3)?,
        provider: row.try_get(4)?,
        event: row.try_get(5)?,
        dedupe_key: row.try_get(6)?,
        dedupe_source: row.try_get(7)?,
        signature_status: row.try_get(8)?,
        status: row.try_get(9)?,
        attempt_count: row.try_get(10)?,
        selected_headers: row.try_get(11)?,
        content_type: row.try_get(12)?,
        raw_body: row.try_get(13)?,
        response_status: row.try_get(14)?,
        response_body: row.try_get(15)?,
        autopilot_run_id: row.try_get(16)?,
        replayed_from_delivery_id: row.try_get(17)?,
        error: row.try_get(18)?,
        received_at: row.try_get(19)?,
        last_attempt_at: row.try_get(20)?,
        created_at: row.try_get(21)?,
        available_at: row.try_get(22)?,
        lease_token: row.try_get(23)?,
        lease_expires_at: row.try_get(24)?,
        dispatch_attempts: row.try_get(25)?,
        reason_code: row.try_get(26)?,
        replay_idempotency_key: row.try_get(27)?,
    }))
}

pub async fn get_webhook_delivery_by_trigger_and_dedupe(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    trigger_id: Uuid,
    dedupe_key: Option<&str>,
) -> anyhow::Result<Option<WebhookDelivery>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, autopilot_id, trigger_id, provider, event, dedupe_key, dedupe_source, signature_status, status, attempt_count, selected_headers, content_type, raw_body, response_status, response_body, autopilot_run_id, replayed_from_delivery_id, error, received_at, last_attempt_at, created_at, available_at, lease_token, lease_expires_at, dispatch_attempts, reason_code, replay_idempotency_key FROM webhook_delivery
WHERE trigger_id = $1
  AND dedupe_key = $2
ORDER BY (status IN ('rejected', 'failed')), created_at DESC
LIMIT 1"#
    )
        .bind(trigger_id)
        .bind(dedupe_key)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WebhookDelivery {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        autopilot_id: row.try_get(2)?,
        trigger_id: row.try_get(3)?,
        provider: row.try_get(4)?,
        event: row.try_get(5)?,
        dedupe_key: row.try_get(6)?,
        dedupe_source: row.try_get(7)?,
        signature_status: row.try_get(8)?,
        status: row.try_get(9)?,
        attempt_count: row.try_get(10)?,
        selected_headers: row.try_get(11)?,
        content_type: row.try_get(12)?,
        raw_body: row.try_get(13)?,
        response_status: row.try_get(14)?,
        response_body: row.try_get(15)?,
        autopilot_run_id: row.try_get(16)?,
        replayed_from_delivery_id: row.try_get(17)?,
        error: row.try_get(18)?,
        received_at: row.try_get(19)?,
        last_attempt_at: row.try_get(20)?,
        created_at: row.try_get(21)?,
        available_at: row.try_get(22)?,
        lease_token: row.try_get(23)?,
        lease_expires_at: row.try_get(24)?,
        dispatch_attempts: row.try_get(25)?,
        reason_code: row.try_get(26)?,
        replay_idempotency_key: row.try_get(27)?,
    }))
}

pub async fn get_webhook_delivery_in_workspace(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<WebhookDelivery>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, autopilot_id, trigger_id, provider, event, dedupe_key, dedupe_source, signature_status, status, attempt_count, selected_headers, content_type, raw_body, response_status, response_body, autopilot_run_id, replayed_from_delivery_id, error, received_at, last_attempt_at, created_at, available_at, lease_token, lease_expires_at, dispatch_attempts, reason_code, replay_idempotency_key FROM webhook_delivery
WHERE id = $1 AND workspace_id = $2"#
    )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WebhookDelivery {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        autopilot_id: row.try_get(2)?,
        trigger_id: row.try_get(3)?,
        provider: row.try_get(4)?,
        event: row.try_get(5)?,
        dedupe_key: row.try_get(6)?,
        dedupe_source: row.try_get(7)?,
        signature_status: row.try_get(8)?,
        status: row.try_get(9)?,
        attempt_count: row.try_get(10)?,
        selected_headers: row.try_get(11)?,
        content_type: row.try_get(12)?,
        raw_body: row.try_get(13)?,
        response_status: row.try_get(14)?,
        response_body: row.try_get(15)?,
        autopilot_run_id: row.try_get(16)?,
        replayed_from_delivery_id: row.try_get(17)?,
        error: row.try_get(18)?,
        received_at: row.try_get(19)?,
        last_attempt_at: row.try_get(20)?,
        created_at: row.try_get(21)?,
        available_at: row.try_get(22)?,
        lease_token: row.try_get(23)?,
        lease_expires_at: row.try_get(24)?,
        dispatch_attempts: row.try_get(25)?,
        reason_code: row.try_get(26)?,
        replay_idempotency_key: row.try_get(27)?,
    }))
}

pub async fn get_webhook_replay_by_idempotency_key(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    replayed_from_delivery_id: Uuid,
    replay_idempotency_key: Option<&str>,
) -> anyhow::Result<Option<WebhookDelivery>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, autopilot_id, trigger_id, provider, event, dedupe_key, dedupe_source, signature_status, status, attempt_count, selected_headers, content_type, raw_body, response_status, response_body, autopilot_run_id, replayed_from_delivery_id, error, received_at, last_attempt_at, created_at, available_at, lease_token, lease_expires_at, dispatch_attempts, reason_code, replay_idempotency_key FROM webhook_delivery
WHERE replayed_from_delivery_id = $1 AND replay_idempotency_key = $2
LIMIT 1"#
    )
        .bind(replayed_from_delivery_id)
        .bind(replay_idempotency_key)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WebhookDelivery {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        autopilot_id: row.try_get(2)?,
        trigger_id: row.try_get(3)?,
        provider: row.try_get(4)?,
        event: row.try_get(5)?,
        dedupe_key: row.try_get(6)?,
        dedupe_source: row.try_get(7)?,
        signature_status: row.try_get(8)?,
        status: row.try_get(9)?,
        attempt_count: row.try_get(10)?,
        selected_headers: row.try_get(11)?,
        content_type: row.try_get(12)?,
        raw_body: row.try_get(13)?,
        response_status: row.try_get(14)?,
        response_body: row.try_get(15)?,
        autopilot_run_id: row.try_get(16)?,
        replayed_from_delivery_id: row.try_get(17)?,
        error: row.try_get(18)?,
        received_at: row.try_get(19)?,
        last_attempt_at: row.try_get(20)?,
        created_at: row.try_get(21)?,
        available_at: row.try_get(22)?,
        lease_token: row.try_get(23)?,
        lease_expires_at: row.try_get(24)?,
        dispatch_attempts: row.try_get(25)?,
        reason_code: row.try_get(26)?,
        replay_idempotency_key: row.try_get(27)?,
    }))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ListWebhookDeliveriesByAutopilotRow {
    pub id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub autopilot_id: Option<Uuid>,
    pub trigger_id: Option<Uuid>,
    pub provider: String,
    pub event: String,
    pub dedupe_key: Option<String>,
    pub dedupe_source: Option<String>,
    pub signature_status: String,
    pub status: String,
    pub attempt_count: i32,
    pub content_type: Option<String>,
    pub response_status: Option<i32>,
    pub autopilot_run_id: Option<Uuid>,
    pub replayed_from_delivery_id: Option<Uuid>,
    pub error: Option<String>,
    pub received_at: Option<DateTime<Utc>>,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
    pub available_at: Option<DateTime<Utc>>,
    pub dispatch_attempts: i32,
    pub reason_code: Option<String>,
    pub replay_idempotency_key: Option<String>,
}

pub async fn list_webhook_deliveries_by_autopilot(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    autopilot_id: Uuid,
    workspace_id: Uuid,
    limit: i32,
    offset: i32,
) -> anyhow::Result<Vec<ListWebhookDeliveriesByAutopilotRow>> {
    let rows = sqlx::query(
        r#"SELECT
    d.id, d.workspace_id, d.autopilot_id, d.trigger_id, d.provider, d.event,
    d.dedupe_key, d.dedupe_source, d.signature_status, d.status,
    d.attempt_count, d.content_type, d.response_status,
    d.autopilot_run_id, d.replayed_from_delivery_id, d.error,
    d.received_at, d.last_attempt_at, d.created_at,
    d.available_at, d.dispatch_attempts, d.reason_code, d.replay_idempotency_key
FROM webhook_delivery d
JOIN autopilot a ON a.id = d.autopilot_id
WHERE d.autopilot_id = $1
  AND a.workspace_id = $2
ORDER BY d.created_at DESC
LIMIT $3 OFFSET $4"#,
    )
    .bind(autopilot_id)
    .bind(workspace_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(executor)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(ListWebhookDeliveriesByAutopilotRow {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            autopilot_id: row.try_get(2)?,
            trigger_id: row.try_get(3)?,
            provider: row.try_get(4)?,
            event: row.try_get(5)?,
            dedupe_key: row.try_get(6)?,
            dedupe_source: row.try_get(7)?,
            signature_status: row.try_get(8)?,
            status: row.try_get(9)?,
            attempt_count: row.try_get(10)?,
            content_type: row.try_get(11)?,
            response_status: row.try_get(12)?,
            autopilot_run_id: row.try_get(13)?,
            replayed_from_delivery_id: row.try_get(14)?,
            error: row.try_get(15)?,
            received_at: row.try_get(16)?,
            last_attempt_at: row.try_get(17)?,
            created_at: row.try_get(18)?,
            available_at: row.try_get(19)?,
            dispatch_attempts: row.try_get(20)?,
            reason_code: row.try_get(21)?,
            replay_idempotency_key: row.try_get(22)?,
        });
    }
    Ok(out)
}

pub async fn retry_claimed_webhook_delivery(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    lease_token: Uuid,
    available_at: Option<DateTime<Utc>>,
    error: Option<&str>,
) -> anyhow::Result<Option<WebhookDelivery>> {
    let row = sqlx::query(
        r#"UPDATE webhook_delivery
SET available_at = $3,
    dispatch_attempts = dispatch_attempts + 1,
    error = $4,
    lease_token = NULL,
    lease_expires_at = NULL,
    last_attempt_at = now()
WHERE id = $1
  AND lease_token = $2
  AND status = 'queued'
RETURNING id, workspace_id, autopilot_id, trigger_id, provider, event, dedupe_key, dedupe_source, signature_status, status, attempt_count, selected_headers, content_type, raw_body, response_status, response_body, autopilot_run_id, replayed_from_delivery_id, error, received_at, last_attempt_at, created_at, available_at, lease_token, lease_expires_at, dispatch_attempts, reason_code, replay_idempotency_key"#
    )
        .bind(id)
        .bind(lease_token)
        .bind(available_at)
        .bind(error)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WebhookDelivery {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        autopilot_id: row.try_get(2)?,
        trigger_id: row.try_get(3)?,
        provider: row.try_get(4)?,
        event: row.try_get(5)?,
        dedupe_key: row.try_get(6)?,
        dedupe_source: row.try_get(7)?,
        signature_status: row.try_get(8)?,
        status: row.try_get(9)?,
        attempt_count: row.try_get(10)?,
        selected_headers: row.try_get(11)?,
        content_type: row.try_get(12)?,
        raw_body: row.try_get(13)?,
        response_status: row.try_get(14)?,
        response_body: row.try_get(15)?,
        autopilot_run_id: row.try_get(16)?,
        replayed_from_delivery_id: row.try_get(17)?,
        error: row.try_get(18)?,
        received_at: row.try_get(19)?,
        last_attempt_at: row.try_get(20)?,
        created_at: row.try_get(21)?,
        available_at: row.try_get(22)?,
        lease_token: row.try_get(23)?,
        lease_expires_at: row.try_get(24)?,
        dispatch_attempts: row.try_get(25)?,
        reason_code: row.try_get(26)?,
        replay_idempotency_key: row.try_get(27)?,
    }))
}

pub async fn update_webhook_delivery_dispatched(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    status: &str,
    autopilot_run_id: Uuid,
    response_status: Option<i32>,
    response_body: Option<&str>,
) -> anyhow::Result<Option<WebhookDelivery>> {
    let row = sqlx::query(
        r#"UPDATE webhook_delivery
SET status = $2,
    autopilot_run_id = $3,
    response_status = $4,
    response_body = $5,
    last_attempt_at = now()
WHERE id = $1
RETURNING id, workspace_id, autopilot_id, trigger_id, provider, event, dedupe_key, dedupe_source, signature_status, status, attempt_count, selected_headers, content_type, raw_body, response_status, response_body, autopilot_run_id, replayed_from_delivery_id, error, received_at, last_attempt_at, created_at, available_at, lease_token, lease_expires_at, dispatch_attempts, reason_code, replay_idempotency_key"#
    )
        .bind(id)
        .bind(status)
        .bind(autopilot_run_id)
        .bind(response_status)
        .bind(response_body)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WebhookDelivery {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        autopilot_id: row.try_get(2)?,
        trigger_id: row.try_get(3)?,
        provider: row.try_get(4)?,
        event: row.try_get(5)?,
        dedupe_key: row.try_get(6)?,
        dedupe_source: row.try_get(7)?,
        signature_status: row.try_get(8)?,
        status: row.try_get(9)?,
        attempt_count: row.try_get(10)?,
        selected_headers: row.try_get(11)?,
        content_type: row.try_get(12)?,
        raw_body: row.try_get(13)?,
        response_status: row.try_get(14)?,
        response_body: row.try_get(15)?,
        autopilot_run_id: row.try_get(16)?,
        replayed_from_delivery_id: row.try_get(17)?,
        error: row.try_get(18)?,
        received_at: row.try_get(19)?,
        last_attempt_at: row.try_get(20)?,
        created_at: row.try_get(21)?,
        available_at: row.try_get(22)?,
        lease_token: row.try_get(23)?,
        lease_expires_at: row.try_get(24)?,
        dispatch_attempts: row.try_get(25)?,
        reason_code: row.try_get(26)?,
        replay_idempotency_key: row.try_get(27)?,
    }))
}

pub async fn update_webhook_delivery_terminal(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    id: Uuid,
    status: &str,
    error: Option<&str>,
    reason_code: Option<&str>,
    response_status: Option<i32>,
    response_body: Option<&str>,
) -> anyhow::Result<Option<WebhookDelivery>> {
    let row = sqlx::query(
        r#"UPDATE webhook_delivery
SET status = $2,
    error = $3,
    reason_code = $4,
    response_status = $5,
    response_body = $6,
    last_attempt_at = now()
WHERE id = $1
RETURNING id, workspace_id, autopilot_id, trigger_id, provider, event, dedupe_key, dedupe_source, signature_status, status, attempt_count, selected_headers, content_type, raw_body, response_status, response_body, autopilot_run_id, replayed_from_delivery_id, error, received_at, last_attempt_at, created_at, available_at, lease_token, lease_expires_at, dispatch_attempts, reason_code, replay_idempotency_key"#
    )
        .bind(id)
        .bind(status)
        .bind(error)
        .bind(reason_code)
        .bind(response_status)
        .bind(response_body)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(WebhookDelivery {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        autopilot_id: row.try_get(2)?,
        trigger_id: row.try_get(3)?,
        provider: row.try_get(4)?,
        event: row.try_get(5)?,
        dedupe_key: row.try_get(6)?,
        dedupe_source: row.try_get(7)?,
        signature_status: row.try_get(8)?,
        status: row.try_get(9)?,
        attempt_count: row.try_get(10)?,
        selected_headers: row.try_get(11)?,
        content_type: row.try_get(12)?,
        raw_body: row.try_get(13)?,
        response_status: row.try_get(14)?,
        response_body: row.try_get(15)?,
        autopilot_run_id: row.try_get(16)?,
        replayed_from_delivery_id: row.try_get(17)?,
        error: row.try_get(18)?,
        received_at: row.try_get(19)?,
        last_attempt_at: row.try_get(20)?,
        created_at: row.try_get(21)?,
        available_at: row.try_get(22)?,
        lease_token: row.try_get(23)?,
        lease_expires_at: row.try_get(24)?,
        dispatch_attempts: row.try_get(25)?,
        reason_code: row.try_get(26)?,
        replay_idempotency_key: row.try_get(27)?,
    }))
}
