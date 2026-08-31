DROP TRIGGER IF EXISTS agent_task_execution_target_snapshot ON agent_task_queue;
DROP FUNCTION IF EXISTS snapshot_agent_task_execution_target();

ALTER TABLE agent_task_queue
    DROP COLUMN IF EXISTS failover_reason,
    DROP COLUMN IF EXISTS policy_revision,
    DROP COLUMN IF EXISTS model_id;
