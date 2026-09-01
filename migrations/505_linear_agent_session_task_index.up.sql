CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_agent_session_task_id ON linear_agent_session (task_id) WHERE task_id IS NOT NULL;
