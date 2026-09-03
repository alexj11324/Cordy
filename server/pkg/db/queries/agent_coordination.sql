-- Durable coordination outbox primitives.  The event key is the producer's
-- idempotency key; leases are fenced by the owner and expiry so a stale
-- worker cannot complete or retry work claimed by a successor.

-- name: EnqueueAgentCoordinationEvent :one
INSERT INTO agent_coordination_outbox (
    event_key, workspace_id, issue_id, source_task_id, event_type, payload, available_at
)
SELECT
    sqlc.arg('event_key'),
    sqlc.arg('workspace_id'),
    sqlc.arg('issue_id'),
    sqlc.narg('source_task_id')::uuid,
    sqlc.arg('event_type'),
    sqlc.arg('payload'),
    COALESCE(sqlc.narg('available_at')::timestamptz, now())
WHERE EXISTS (
    SELECT 1
    FROM issue AS issue_scope
    WHERE issue_scope.id = sqlc.arg('issue_id')
      AND issue_scope.workspace_id = sqlc.arg('workspace_id')
)
  AND (
      sqlc.narg('source_task_id')::uuid IS NULL
      OR EXISTS (
          SELECT 1
          FROM agent_task_queue AS source_task
          JOIN agent AS source_agent ON source_agent.id = source_task.agent_id
          WHERE source_task.id = sqlc.narg('source_task_id')::uuid
            AND source_task.issue_id = sqlc.arg('issue_id')
            AND source_agent.workspace_id = sqlc.arg('workspace_id')
      )
  )
ON CONFLICT (event_key) DO UPDATE
SET updated_at = agent_coordination_outbox.updated_at
WHERE agent_coordination_outbox.workspace_id = EXCLUDED.workspace_id
  AND agent_coordination_outbox.issue_id = EXCLUDED.issue_id
  AND agent_coordination_outbox.source_task_id IS NOT DISTINCT FROM EXCLUDED.source_task_id
  AND agent_coordination_outbox.event_type = EXCLUDED.event_type
RETURNING agent_coordination_outbox.*;

-- name: ClaimAgentCoordinationOutbox :many
WITH due AS MATERIALIZED (
    SELECT id
    FROM agent_coordination_outbox
    WHERE (
        status = 'pending'
        AND available_at <= now()
    ) OR (
        status = 'processing'
        AND (lease_expires_at IS NULL OR lease_expires_at <= now())
    )
    ORDER BY available_at, created_at, id
    FOR UPDATE SKIP LOCKED
    LIMIT sqlc.arg('batch_size')::int
)
UPDATE agent_coordination_outbox AS outbox
SET status = 'processing',
    lease_owner = sqlc.arg('lease_owner'),
    lease_expires_at = now() + make_interval(
        secs => GREATEST(sqlc.arg('lease_seconds')::double precision, 1.0)
    ),
    attempt = outbox.attempt + 1,
    updated_at = now()
FROM due
WHERE outbox.id = due.id
RETURNING outbox.*;

-- name: CompleteAgentCoordinationOutbox :one
WITH completed AS (
    UPDATE agent_coordination_outbox AS outbox
    SET status = 'completed',
        processed_at = now(),
        lease_owner = NULL,
        lease_expires_at = NULL,
        last_error = NULL,
        updated_at = now()
    WHERE outbox.id = sqlc.arg('id')
      AND outbox.status = 'processing'
      AND outbox.lease_owner = sqlc.arg('lease_owner')
      AND outbox.lease_expires_at > now()
    RETURNING outbox.*
)
SELECT * FROM completed
UNION ALL
SELECT *
FROM agent_coordination_outbox AS outbox
WHERE outbox.id = sqlc.arg('id')
  AND outbox.status = 'completed'
LIMIT 1;

-- name: RetryAgentCoordinationOutbox :execrows
UPDATE agent_coordination_outbox
SET status = 'pending',
    available_at = now() + make_interval(
        secs => GREATEST(sqlc.arg('delay_seconds')::double precision, 1.0)
    ),
    lease_owner = NULL,
    lease_expires_at = NULL,
    last_error = LEFT(sqlc.arg('last_error')::text, 4096),
    updated_at = now()
WHERE id = sqlc.arg('id')
  AND status = 'processing'
  AND lease_owner = sqlc.arg('lease_owner')
  AND lease_expires_at > now();

-- name: UpsertAgentCoordinationAssignment :one
INSERT INTO agent_coordination_assignment (
    event_id, workspace_id, issue_id, source_task_id, role
)
SELECT
    sqlc.arg('event_id'),
    sqlc.arg('workspace_id'),
    sqlc.arg('issue_id'),
    sqlc.narg('source_task_id')::uuid,
    sqlc.arg('role')
FROM agent_coordination_outbox AS event
JOIN issue AS issue_scope ON issue_scope.id = sqlc.arg('issue_id')
WHERE event.id = sqlc.arg('event_id')
  AND event.workspace_id = sqlc.arg('workspace_id')
  AND event.issue_id = sqlc.arg('issue_id')
  AND issue_scope.workspace_id = sqlc.arg('workspace_id')
  AND event.source_task_id IS NOT DISTINCT FROM sqlc.narg('source_task_id')::uuid
ON CONFLICT (event_id, role) DO UPDATE
SET updated_at = agent_coordination_assignment.updated_at
WHERE agent_coordination_assignment.workspace_id = EXCLUDED.workspace_id
  AND agent_coordination_assignment.issue_id = EXCLUDED.issue_id
  AND agent_coordination_assignment.source_task_id IS NOT DISTINCT FROM EXCLUDED.source_task_id
RETURNING agent_coordination_assignment.*;

-- name: LockAgentCoordinationIssue :one
-- Issue deletion locks the issue row before deleting its coordination rows.
-- Acquire the compatible key-share lock before the worker locks an assignment
-- so a delete and a claimed dispatch use the same issue -> coordination lock
-- order instead of deadlocking while each transaction waits on the other.
SELECT issue.*
FROM issue
WHERE issue.id = sqlc.arg('issue_id')
  AND issue.workspace_id = sqlc.arg('workspace_id')
FOR KEY SHARE;

-- name: LockActiveReviewerTasksForReviewReturn :many
-- Lock order is reviewer task -> issue, matching coordinator promotion. Only
-- tasks correlated to a reviewer coordination assignment are eligible; plain
-- issue tasks must never be retired by a review return.
SELECT task.id
FROM agent_task_queue AS task
WHERE task.issue_id = sqlc.arg('issue_id')
  AND task.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'waiting_capacity', 'deferred')
  AND EXISTS (
      SELECT 1
      FROM agent_coordination_assignment AS assignment
      WHERE assignment.issue_id = task.issue_id
        AND assignment.role = 'reviewer'
        AND (
            assignment.dispatched_task_id = task.id
            OR task.context->>'coordination_assignment_id' = assignment.id::text
        )
  )
ORDER BY task.created_at DESC, task.id DESC
FOR UPDATE OF task;

-- name: GetAgentCoordinationAssignmentForLease :one
-- Loads the assignment only while the caller still owns the event lease. The
-- workspace/issue predicates are deliberate tenant fences in addition to the
-- event id: a stale or cross-tenant caller must not be able to mutate a row
-- merely by guessing an id.
SELECT assignment.*
FROM agent_coordination_assignment AS assignment
JOIN agent_coordination_outbox AS event ON event.id = assignment.event_id
WHERE assignment.event_id = sqlc.arg('event_id')
  AND assignment.workspace_id = sqlc.arg('workspace_id')
  AND assignment.issue_id = sqlc.arg('issue_id')
  AND event.workspace_id = assignment.workspace_id
  AND event.issue_id = assignment.issue_id
  AND event.status = 'processing'
  AND event.lease_owner = sqlc.arg('lease_owner')
  AND event.lease_expires_at > now()
ORDER BY assignment.created_at ASC, assignment.id ASC
LIMIT 1
FOR UPDATE OF assignment;

-- name: GetAgentCoordinationAssignmentForTask :one
-- The cancellation producer resolves an assignment by the dispatched task,
-- or by the immutable assignment id stamped into task.context. The task join
-- below makes both paths tenant- and owner-scoped; the event payload itself is
-- already available on the claimed outbox row and is not duplicated here.
SELECT assignment.*
FROM agent_coordination_assignment AS assignment
JOIN agent_coordination_outbox AS event ON event.id = assignment.event_id
JOIN agent_task_queue AS task ON task.id = sqlc.arg('task_id')
JOIN agent AS task_agent ON task_agent.id = task.agent_id
WHERE assignment.workspace_id = sqlc.arg('workspace_id')
  AND assignment.issue_id = sqlc.arg('issue_id')
  AND event.workspace_id = assignment.workspace_id
  AND event.issue_id = assignment.issue_id
  AND task.issue_id = assignment.issue_id
  AND task.agent_id = assignment.owner_id
  AND task_agent.workspace_id = assignment.workspace_id
  AND (
      assignment.dispatched_task_id = sqlc.arg('task_id')
      OR (
          sqlc.narg('assignment_id')::uuid IS NOT NULL
          AND assignment.id = sqlc.narg('assignment_id')::uuid
      )
  )
ORDER BY assignment.created_at DESC, assignment.id DESC
LIMIT 1;

-- name: GetCoordinationAgentForDispatch :one
-- Resolve only a user agent that is still bound to a healthy runtime in the
-- same workspace. The coordinator uses the database clock and the same
-- private-runtime ownership rule as the normal daemon claim query; an
-- offline/archived/rebound agent is deferred instead of receiving a stranded
-- task.
SELECT agent.*
FROM agent
JOIN agent_runtime AS runtime ON runtime.id = agent.runtime_id
WHERE agent.id = sqlc.arg('agent_id')
  AND agent.workspace_id = sqlc.arg('workspace_id')
  AND agent.kind = 'user'
  AND agent.archived_at IS NULL
  AND agent.runtime_id IS NOT NULL
  AND runtime.workspace_id = agent.workspace_id
  AND runtime.status = 'online'
  AND COALESCE(runtime.last_seen_at, runtime.updated_at) >= now() - make_interval(
      secs => GREATEST(sqlc.arg('runtime_stale_seconds')::double precision, 1.0)
  )
  AND (
      runtime.visibility = 'public'
      OR (
          runtime.visibility = 'private'
          AND (
              runtime.owner_id IS NULL
              OR agent.owner_id IS NULL
              OR runtime.owner_id = agent.owner_id
          )
      )
  );

-- name: AssignAgentCoordinationAssignmentForLease :execrows
-- Records a deterministic owner decision while the event lease is held. The
-- same-owner update is a no-op for attempt/decision purposes, which makes a
-- worker retry idempotent even if it reaches this statement twice.
UPDATE agent_coordination_assignment AS assignment
SET status = 'assigned',
    owner_type = 'agent',
    owner_id = sqlc.arg('owner_id'),
    decision = CASE
        WHEN assignment.status = 'assigned'
         AND assignment.owner_type = 'agent'
         AND assignment.owner_id = sqlc.arg('owner_id')
            THEN assignment.decision
        ELSE assignment.decision || sqlc.arg('decision')::jsonb
    END,
    attempt = CASE
        WHEN assignment.status = 'assigned'
         AND assignment.owner_type = 'agent'
         AND assignment.owner_id = sqlc.arg('owner_id')
            THEN assignment.attempt
        ELSE assignment.attempt + 1
    END,
    assigned_at = COALESCE(assignment.assigned_at, now()),
    last_error = NULL,
    updated_at = now()
FROM agent_coordination_outbox AS event
JOIN agent AS owner_agent ON owner_agent.id = sqlc.arg('owner_id')
WHERE assignment.id = sqlc.arg('assignment_id')
  AND assignment.event_id = sqlc.arg('event_id')
  AND assignment.workspace_id = sqlc.arg('workspace_id')
  AND assignment.issue_id = sqlc.arg('issue_id')
  AND assignment.status IN ('pending', 'assigned')
  AND event.id = assignment.event_id
  AND event.workspace_id = assignment.workspace_id
  AND event.issue_id = assignment.issue_id
  AND event.status = 'processing'
  AND event.lease_owner = sqlc.arg('lease_owner')
  AND event.lease_expires_at > now()
  AND owner_agent.workspace_id = assignment.workspace_id
  AND owner_agent.kind = 'user'
  AND owner_agent.archived_at IS NULL;

-- name: CompleteAgentCoordinationAssignmentForTask :execrows
-- A terminal task is the producer-side fence. The task status, task owner,
-- workspace, issue, and dispatched-task identity must all agree before the
-- assignment is closed. This write intentionally does not use an outbox
-- lease: it runs in the same transaction as the task's terminal status
-- transition, and the terminal task row is its idempotency fence.
UPDATE agent_coordination_assignment AS assignment
SET status = 'completed',
    updated_at = now()
FROM agent_coordination_outbox AS event
JOIN agent AS task_agent ON task_agent.id = sqlc.arg('agent_id')
JOIN agent_task_queue AS task ON task.id = sqlc.arg('task_id')
WHERE assignment.event_id = event.id
  AND assignment.id = sqlc.arg('assignment_id')
  AND assignment.dispatched_task_id = task.id
  AND assignment.workspace_id = sqlc.arg('workspace_id')
  AND assignment.issue_id = sqlc.arg('issue_id')
  AND task.agent_id = sqlc.arg('agent_id')
  AND task_agent.workspace_id = assignment.workspace_id
  AND task.issue_id = assignment.issue_id
  AND task.status IN ('completed', 'cancelled')
  AND assignment.status IN ('assigned', 'dispatched', 'completed')
  AND event.workspace_id = assignment.workspace_id
  AND event.issue_id = assignment.issue_id;

-- name: CompleteAgentCoordinationAssignmentForLease :execrows
-- Assignment and outbox completion are called in the same transaction. The
-- lease owner/expiry fence is repeated here rather than relying on the prior
-- SELECT, because another worker may have taken the lease after any lock was
-- released or after a transaction was retried.
UPDATE agent_coordination_assignment AS assignment
SET status = sqlc.arg('status'),
    dispatched_task_id = COALESCE(sqlc.narg('dispatched_task_id')::uuid, assignment.dispatched_task_id),
    dispatched_at = CASE
        WHEN sqlc.narg('dispatched_task_id')::uuid IS NULL THEN assignment.dispatched_at
        ELSE COALESCE(assignment.dispatched_at, now())
    END,
    decision = assignment.decision || COALESCE(sqlc.narg('decision')::jsonb, '{}'::jsonb),
    last_error = sqlc.narg('last_error'),
    updated_at = now()
FROM agent_coordination_outbox AS event
WHERE assignment.id = sqlc.arg('assignment_id')
  AND assignment.event_id = sqlc.arg('event_id')
  AND assignment.workspace_id = sqlc.arg('workspace_id')
  AND assignment.issue_id = sqlc.arg('issue_id')
  AND assignment.status IN ('pending', 'assigned', 'dispatched')
  AND event.id = assignment.event_id
  AND event.workspace_id = assignment.workspace_id
  AND event.issue_id = assignment.issue_id
  AND event.status = 'processing'
  AND event.lease_owner = sqlc.arg('lease_owner')
  AND event.lease_expires_at > now();

-- name: DeferAgentCoordinationAssignmentForLease :execrows
-- A selected owner is retained across a retry; an unowned assignment remains
-- pending. This prevents a transient runtime/capacity failure from losing an
-- explicit reviewer decision while still allowing a future worker to select
-- an owner when none was chosen.
UPDATE agent_coordination_assignment AS assignment
SET status = CASE WHEN assignment.owner_id IS NULL THEN 'pending' ELSE 'assigned' END,
    last_error = LEFT(sqlc.arg('last_error')::text, 4096),
    updated_at = now()
FROM agent_coordination_outbox AS event
WHERE assignment.id = sqlc.arg('assignment_id')
  AND assignment.event_id = sqlc.arg('event_id')
  AND assignment.workspace_id = sqlc.arg('workspace_id')
  AND assignment.issue_id = sqlc.arg('issue_id')
  AND event.id = assignment.event_id
  AND event.workspace_id = assignment.workspace_id
  AND event.issue_id = assignment.issue_id
  AND event.status = 'processing'
  AND event.lease_owner = sqlc.arg('lease_owner')
  AND event.lease_expires_at > now();

-- name: SetAgentCoordinationAssignmentOwner :execrows
-- Used by an explicitly selected reviewer and by cancellation recovery. It is
-- idempotent and never rewinds an assignment already dispatched/completed.
UPDATE agent_coordination_assignment AS assignment
SET owner_type = 'agent',
    owner_id = sqlc.arg('owner_id'),
    status = 'assigned',
    assigned_at = COALESCE(assignment.assigned_at, now()),
    updated_at = now()
FROM agent_coordination_outbox AS event
JOIN agent AS owner_agent ON owner_agent.id = sqlc.arg('owner_id')
WHERE event.id = assignment.event_id
  AND event.event_key = sqlc.arg('event_key')
  AND event.workspace_id = sqlc.arg('workspace_id')
  AND event.issue_id = sqlc.arg('issue_id')
  AND event.status = 'pending'
  AND assignment.workspace_id = event.workspace_id
  AND assignment.issue_id = event.issue_id
  AND assignment.role = sqlc.arg('role')
  AND assignment.status IN ('pending', 'assigned')
  AND (
      assignment.owner_id IS NULL
      OR (
          assignment.owner_type = 'agent'
          AND assignment.owner_id = sqlc.arg('owner_id')
      )
  )
  AND owner_agent.workspace_id = event.workspace_id
  AND owner_agent.kind = 'user'
  AND owner_agent.archived_at IS NULL;
