-- name: EnsureAutomationQuotaPeriod :one
-- The no-op update is intentional: ON CONFLICT UPDATE locks the period row for
-- the caller's transaction, serialising admission for one workspace/period.
INSERT INTO automation_quota_period (workspace_id, period_start, period_end)
VALUES ($1, $2, $3)
ON CONFLICT (workspace_id, period_start, period_end) DO UPDATE
SET updated_at = automation_quota_period.updated_at
RETURNING *;

-- name: GetAutomationQuotaPeriod :one
SELECT * FROM automation_quota_period
WHERE workspace_id = $1 AND period_start = $2 AND period_end = $3;

-- name: GetAutomationQuotaReservationByKey :one
SELECT * FROM automation_quota_reservation
WHERE workspace_id = $1
  AND period_start = $2
  AND period_end = $3
  AND idempotency_key = $4
  AND state <> 'released';

-- name: CreateAutomationQuotaReservation :one
INSERT INTO automation_quota_reservation (
    workspace_id, period_start, period_end, policy_revision,
    subscription_version, source, idempotency_key
) VALUES ($1, $2, $3, $4, $5, $6, $7)
RETURNING *;

-- name: IncrementAutomationQuotaReserved :one
UPDATE automation_quota_period
SET reserved_count = reserved_count + 1,
    updated_at = now()
WHERE workspace_id = $1 AND period_start = $2 AND period_end = $3
RETURNING *;

-- name: IncrementAutomationQuotaBlocked :one
UPDATE automation_quota_period
SET blocked_counts = jsonb_set(
        blocked_counts,
        ARRAY[@source::text],
        to_jsonb(COALESCE((blocked_counts ->> @source::text)::bigint, 0) + 1),
        true
    ),
    updated_at = now()
WHERE workspace_id = @workspace_id
  AND period_start = @period_start
  AND period_end = @period_end
RETURNING *;

-- name: ConsumeAutomationQuotaReservation :one
-- used_count is monotonic within a period: consuming a reserved slot is the
-- only write that changes it, and no release path decrements it.
WITH locked AS (
    SELECT qr.* FROM automation_quota_reservation qr
    WHERE qr.id = @reservation_id AND qr.state = 'reserved'
    FOR UPDATE
), changed AS (
    UPDATE automation_quota_reservation AS r
    SET state = 'consumed', finalized_at = now()
    FROM locked
    WHERE r.id = locked.id
      AND EXISTS (
          SELECT 1 FROM automation_quota_period p
          WHERE p.workspace_id = locked.workspace_id
            AND p.period_start = locked.period_start
            AND p.period_end = locked.period_end
      )
    RETURNING locked.workspace_id, locked.period_start, locked.period_end
)
UPDATE automation_quota_period AS p
SET reserved_count = reserved_count - 1,
    used_count = used_count + 1,
    updated_at = now()
FROM changed
WHERE p.workspace_id = changed.workspace_id
  AND p.period_start = changed.period_start
  AND p.period_end = changed.period_end
RETURNING p.*;

-- name: ReleaseAutomationQuotaReservation :one
-- Release is intentionally limited to still-reserved work. A consumed
-- create_issue slot remains counted after cancellation, blocking, or deletion.
WITH locked AS (
    SELECT qr.* FROM automation_quota_reservation qr
    WHERE qr.id = @reservation_id AND qr.state = 'reserved'
    FOR UPDATE
), changed AS (
    UPDATE automation_quota_reservation AS r
    SET state = 'released', finalized_at = now()
    FROM locked
    WHERE r.id = locked.id
      AND EXISTS (
          SELECT 1 FROM automation_quota_period p
          WHERE p.workspace_id = locked.workspace_id
            AND p.period_start = locked.period_start
            AND p.period_end = locked.period_end
      )
    RETURNING locked.workspace_id, locked.period_start, locked.period_end
)
UPDATE automation_quota_period AS p
SET reserved_count = reserved_count - 1,
    updated_at = now()
FROM changed
WHERE p.workspace_id = changed.workspace_id
  AND p.period_start = changed.period_start
  AND p.period_end = changed.period_end
RETURNING p.*;

-- name: ListRecoverableAutomationQuotaReservations :many
SELECT r.*
FROM automation_quota_reservation r
LEFT JOIN automation_run ar ON ar.quota_reservation_id = r.id
WHERE r.state = 'reserved'
  AND (
      (
          r.created_at < @terminal_created_before
          AND (ar.id IS NULL OR ar.status IN ('completed', 'failed', 'skipped'))
      )
      OR (
          -- Manual/API requests have no durable retry owner. Reclaim only an
          -- hours-old partial run with neither a linked side effect nor even
          -- an unlinked task row; schedule and webhook retries repair their
          -- own partial state and are deliberately excluded.
          r.created_at < @partial_created_before
          AND ar.source IN ('manual', 'api')
          AND (
              ar.status = 'pending'
              OR (ar.status = 'issue_created' AND ar.issue_id IS NULL)
              OR (ar.status = 'running' AND ar.task_id IS NULL)
          )
          AND NOT EXISTS (
              SELECT 1
              FROM agent_task_queue task
              WHERE task.automation_run_id = ar.id
          )
      )
  )
ORDER BY r.created_at
LIMIT @row_limit;
