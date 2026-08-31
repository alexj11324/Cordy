UPDATE agent_task_queue
SET status = 'queued',
    wait_reason = NULL
WHERE status = 'waiting_capacity';

ALTER TABLE agent_task_queue DROP CONSTRAINT IF EXISTS agent_task_queue_status_check;
ALTER TABLE agent_task_queue ADD CONSTRAINT agent_task_queue_status_check
    CHECK (status IN (
        'queued', 'dispatched', 'running', 'waiting_local_directory',
        'completed', 'failed', 'cancelled', 'deferred'
    ));
