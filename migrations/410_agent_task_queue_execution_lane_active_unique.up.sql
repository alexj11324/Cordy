CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_agent_task_queue_execution_lane_active_unique
    ON agent_task_queue (execution_lane_key)
    WHERE status IN ('dispatched', 'running', 'waiting_local_directory');
