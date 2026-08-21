//! Port of server/pkg/db/queries/autopilot_quota.sql (generated autopilot_quota.sql.go).
//! Positional extraction mirrors Go's Scan order exactly.

#![allow(clippy::too_many_arguments)]
#![allow(unused_imports)]

use crate::models::*;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

pub async fn consume_autopilot_quota_reservation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    reservation_id: Uuid,
) -> anyhow::Result<Option<AutopilotQuotaPeriod>> {
    let row = sqlx::query(
        r#"WITH locked AS (
    SELECT qr.id, qr.workspace_id, qr.period_start, qr.period_end, qr.policy_revision, qr.subscription_version, qr.source, qr.idempotency_key, qr.state, qr.created_at, qr.finalized_at FROM autopilot_quota_reservation qr
    WHERE qr.id = $1 AND qr.state = 'reserved'
    FOR UPDATE
), changed AS (
    UPDATE autopilot_quota_reservation AS r
    SET state = 'consumed', finalized_at = now()
    FROM locked
    WHERE r.id = locked.id
      AND EXISTS (
          SELECT 1 FROM autopilot_quota_period p
          WHERE p.workspace_id = locked.workspace_id
            AND p.period_start = locked.period_start
            AND p.period_end = locked.period_end
      )
    RETURNING locked.workspace_id, locked.period_start, locked.period_end
)
UPDATE autopilot_quota_period AS p
SET reserved_count = reserved_count - 1,
    used_count = used_count + 1,
    updated_at = now()
FROM changed
WHERE p.workspace_id = changed.workspace_id
  AND p.period_start = changed.period_start
  AND p.period_end = changed.period_end
RETURNING p.workspace_id, p.period_start, p.period_end, p.used_count, p.reserved_count, p.blocked_counts, p.created_at, p.updated_at"#
    )
        .bind(reservation_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutopilotQuotaPeriod {
        workspace_id: row.try_get(0)?,
        period_start: row.try_get(1)?,
        period_end: row.try_get(2)?,
        used_count: row.try_get(3)?,
        reserved_count: row.try_get(4)?,
        blocked_counts: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
    }))
}

pub async fn create_autopilot_quota_reservation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    period_start: Option<DateTime<Utc>>,
    period_end: Option<DateTime<Utc>>,
    policy_revision: i64,
    subscription_version: i64,
    source: &str,
    idempotency_key: &str,
) -> anyhow::Result<Option<AutopilotQuotaReservation>> {
    let row = sqlx::query(
        r#"INSERT INTO autopilot_quota_reservation (
    workspace_id, period_start, period_end, policy_revision,
    subscription_version, source, idempotency_key
) VALUES ($1, $2, $3, $4, $5, $6, $7)
RETURNING id, workspace_id, period_start, period_end, policy_revision, subscription_version, source, idempotency_key, state, created_at, finalized_at"#
    )
        .bind(workspace_id)
        .bind(period_start)
        .bind(period_end)
        .bind(policy_revision)
        .bind(subscription_version)
        .bind(source)
        .bind(idempotency_key)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutopilotQuotaReservation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        period_start: row.try_get(2)?,
        period_end: row.try_get(3)?,
        policy_revision: row.try_get(4)?,
        subscription_version: row.try_get(5)?,
        source: row.try_get(6)?,
        idempotency_key: row.try_get(7)?,
        state: row.try_get(8)?,
        created_at: row.try_get(9)?,
        finalized_at: row.try_get(10)?,
    }))
}

pub async fn ensure_autopilot_quota_period(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    period_start: Option<DateTime<Utc>>,
    period_end: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<AutopilotQuotaPeriod>> {
    let row = sqlx::query(
        r#"INSERT INTO autopilot_quota_period (workspace_id, period_start, period_end)
VALUES ($1, $2, $3)
ON CONFLICT (workspace_id, period_start, period_end) DO UPDATE
SET updated_at = autopilot_quota_period.updated_at
RETURNING workspace_id, period_start, period_end, used_count, reserved_count, blocked_counts, created_at, updated_at"#
    )
        .bind(workspace_id)
        .bind(period_start)
        .bind(period_end)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutopilotQuotaPeriod {
        workspace_id: row.try_get(0)?,
        period_start: row.try_get(1)?,
        period_end: row.try_get(2)?,
        used_count: row.try_get(3)?,
        reserved_count: row.try_get(4)?,
        blocked_counts: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
    }))
}

pub async fn get_autopilot_quota_period(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    period_start: Option<DateTime<Utc>>,
    period_end: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<AutopilotQuotaPeriod>> {
    let row = sqlx::query(
        r#"SELECT workspace_id, period_start, period_end, used_count, reserved_count, blocked_counts, created_at, updated_at FROM autopilot_quota_period
WHERE workspace_id = $1 AND period_start = $2 AND period_end = $3"#
    )
        .bind(workspace_id)
        .bind(period_start)
        .bind(period_end)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutopilotQuotaPeriod {
        workspace_id: row.try_get(0)?,
        period_start: row.try_get(1)?,
        period_end: row.try_get(2)?,
        used_count: row.try_get(3)?,
        reserved_count: row.try_get(4)?,
        blocked_counts: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
    }))
}

pub async fn get_autopilot_quota_reservation_by_key(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    period_start: Option<DateTime<Utc>>,
    period_end: Option<DateTime<Utc>>,
    idempotency_key: &str,
) -> anyhow::Result<Option<AutopilotQuotaReservation>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, period_start, period_end, policy_revision, subscription_version, source, idempotency_key, state, created_at, finalized_at FROM autopilot_quota_reservation
WHERE workspace_id = $1
  AND period_start = $2
  AND period_end = $3
  AND idempotency_key = $4
  AND state <> 'released'"#
    )
        .bind(workspace_id)
        .bind(period_start)
        .bind(period_end)
        .bind(idempotency_key)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutopilotQuotaReservation {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        period_start: row.try_get(2)?,
        period_end: row.try_get(3)?,
        policy_revision: row.try_get(4)?,
        subscription_version: row.try_get(5)?,
        source: row.try_get(6)?,
        idempotency_key: row.try_get(7)?,
        state: row.try_get(8)?,
        created_at: row.try_get(9)?,
        finalized_at: row.try_get(10)?,
    }))
}

pub async fn increment_autopilot_quota_blocked(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    source: &str,
    workspace_id: Uuid,
    period_start: Option<DateTime<Utc>>,
    period_end: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<AutopilotQuotaPeriod>> {
    let row = sqlx::query(
        r#"UPDATE autopilot_quota_period
SET blocked_counts = jsonb_set(
        blocked_counts,
        ARRAY[$1::text],
        to_jsonb(COALESCE((blocked_counts ->> $1::text)::bigint, 0) + 1),
        true
    ),
    updated_at = now()
WHERE workspace_id = $2
  AND period_start = $3
  AND period_end = $4
RETURNING workspace_id, period_start, period_end, used_count, reserved_count, blocked_counts, created_at, updated_at"#
    )
        .bind(source)
        .bind(workspace_id)
        .bind(period_start)
        .bind(period_end)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutopilotQuotaPeriod {
        workspace_id: row.try_get(0)?,
        period_start: row.try_get(1)?,
        period_end: row.try_get(2)?,
        used_count: row.try_get(3)?,
        reserved_count: row.try_get(4)?,
        blocked_counts: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
    }))
}

pub async fn increment_autopilot_quota_reserved(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    period_start: Option<DateTime<Utc>>,
    period_end: Option<DateTime<Utc>>,
) -> anyhow::Result<Option<AutopilotQuotaPeriod>> {
    let row = sqlx::query(
        r#"UPDATE autopilot_quota_period
SET reserved_count = reserved_count + 1,
    updated_at = now()
WHERE workspace_id = $1 AND period_start = $2 AND period_end = $3
RETURNING workspace_id, period_start, period_end, used_count, reserved_count, blocked_counts, created_at, updated_at"#
    )
        .bind(workspace_id)
        .bind(period_start)
        .bind(period_end)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutopilotQuotaPeriod {
        workspace_id: row.try_get(0)?,
        period_start: row.try_get(1)?,
        period_end: row.try_get(2)?,
        used_count: row.try_get(3)?,
        reserved_count: row.try_get(4)?,
        blocked_counts: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
    }))
}

pub async fn list_recoverable_autopilot_quota_reservations(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    terminal_created_before: Option<DateTime<Utc>>,
    partial_created_before: Option<DateTime<Utc>>,
    row_limit: i32,
) -> anyhow::Result<Vec<AutopilotQuotaReservation>> {
    let rows = sqlx::query(
        r#"SELECT r.id, r.workspace_id, r.period_start, r.period_end, r.policy_revision, r.subscription_version, r.source, r.idempotency_key, r.state, r.created_at, r.finalized_at
FROM autopilot_quota_reservation r
LEFT JOIN autopilot_run ar ON ar.quota_reservation_id = r.id
WHERE r.state = 'reserved'
  AND (
      (
          r.created_at < $1
          AND (ar.id IS NULL OR ar.status IN ('completed', 'failed', 'skipped'))
      )
      OR (
          -- Manual/API requests have no durable retry owner. Reclaim only an
          -- hours-old partial run with neither a linked side effect nor even
          -- an unlinked task row; schedule and webhook retries repair their
          -- own partial state and are deliberately excluded.
          r.created_at < $2
          AND ar.source IN ('manual', 'api')
          AND (
              ar.status = 'pending'
              OR (ar.status = 'issue_created' AND ar.issue_id IS NULL)
              OR (ar.status = 'running' AND ar.task_id IS NULL)
          )
          AND NOT EXISTS (
              SELECT 1
              FROM agent_task_queue task
              WHERE task.autopilot_run_id = ar.id
          )
      )
  )
ORDER BY r.created_at
LIMIT $3"#
    )
        .bind(terminal_created_before)
        .bind(partial_created_before)
        .bind(row_limit)
        .fetch_all(executor)
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(AutopilotQuotaReservation {
            id: row.try_get(0)?,
            workspace_id: row.try_get(1)?,
            period_start: row.try_get(2)?,
            period_end: row.try_get(3)?,
            policy_revision: row.try_get(4)?,
            subscription_version: row.try_get(5)?,
            source: row.try_get(6)?,
            idempotency_key: row.try_get(7)?,
            state: row.try_get(8)?,
            created_at: row.try_get(9)?,
            finalized_at: row.try_get(10)?,
        });
    }
    Ok(out)
}

pub async fn release_autopilot_quota_reservation(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    reservation_id: Uuid,
) -> anyhow::Result<Option<AutopilotQuotaPeriod>> {
    let row = sqlx::query(
        r#"WITH locked AS (
    SELECT qr.id, qr.workspace_id, qr.period_start, qr.period_end, qr.policy_revision, qr.subscription_version, qr.source, qr.idempotency_key, qr.state, qr.created_at, qr.finalized_at FROM autopilot_quota_reservation qr
    WHERE qr.id = $1 AND qr.state = 'reserved'
    FOR UPDATE
), changed AS (
    UPDATE autopilot_quota_reservation AS r
    SET state = 'released', finalized_at = now()
    FROM locked
    WHERE r.id = locked.id
      AND EXISTS (
          SELECT 1 FROM autopilot_quota_period p
          WHERE p.workspace_id = locked.workspace_id
            AND p.period_start = locked.period_start
            AND p.period_end = locked.period_end
      )
    RETURNING locked.workspace_id, locked.period_start, locked.period_end
)
UPDATE autopilot_quota_period AS p
SET reserved_count = reserved_count - 1,
    updated_at = now()
FROM changed
WHERE p.workspace_id = changed.workspace_id
  AND p.period_start = changed.period_start
  AND p.period_end = changed.period_end
RETURNING p.workspace_id, p.period_start, p.period_end, p.used_count, p.reserved_count, p.blocked_counts, p.created_at, p.updated_at"#
    )
        .bind(reservation_id)
        .fetch_optional(executor)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(AutopilotQuotaPeriod {
        workspace_id: row.try_get(0)?,
        period_start: row.try_get(1)?,
        period_end: row.try_get(2)?,
        used_count: row.try_get(3)?,
        reserved_count: row.try_get(4)?,
        blocked_counts: row.try_get(5)?,
        created_at: row.try_get(6)?,
        updated_at: row.try_get(7)?,
    }))
}
