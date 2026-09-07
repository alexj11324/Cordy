-- name: DeferOtherQueuedAgentThreadTasks :many
-- Steering one continuation must make that selected row the only claimable
-- continuation in its thread before the current execution is interrupted.
-- The service passes a lineage-validated thread id set while holding the
-- owning Agent's claim lock.
UPDATE agent_task_queue
SET status = 'deferred',
    priority = LEAST(priority, 4),
    fire_at = now()
WHERE id = ANY(@thread_task_ids::uuid[])
  AND id <> @selected_task_id::uuid
  AND status = 'queued'
RETURNING *;

-- name: PrioritizeAgentThreadTask :one
-- Priority 5 is reserved above normal issue urgency (0..4), so the selected
-- continuation is the next task claimed after the current execution stops.
-- The context predicate prevents this task-level operation from accepting an
-- unrelated non-Chat task even if a caller supplied a stale id set.
UPDATE agent_task_queue
SET status = 'queued', priority = 5, fire_at = now()
WHERE id = @id
  AND status IN ('queued', 'deferred')
  AND chat_session_id IS NULL
  AND NULLIF(context->>'agent_thread_parent_task_id', '') IS NOT NULL
RETURNING *;
