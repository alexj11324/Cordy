CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_agent_task_queue_waiting_capacity_issue_agent
    ON agent_task_queue (issue_id, agent_id)
    WHERE status = 'waiting_capacity';
