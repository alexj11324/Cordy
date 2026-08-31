-- A task whose selected ACP has no available capacity remains durable and
-- visible, but must not be claimed until the coordinator resumes it.
ALTER TABLE agent_task_queue DROP CONSTRAINT IF EXISTS agent_task_queue_status_check;
ALTER TABLE agent_task_queue ADD CONSTRAINT agent_task_queue_status_check
    CHECK (status IN (
        'queued', 'dispatched', 'running', 'waiting_local_directory',
        'waiting_capacity', 'completed', 'failed', 'cancelled', 'deferred'
    ));

-- Capacity-waiting tasks are still pending work.  Keep the normal active
-- execution-lane uniqueness invariant scoped to claimable/running states; a
-- waiting row cannot race a newly admitted duplicate because the existing
-- enqueue transaction owns that decision.
