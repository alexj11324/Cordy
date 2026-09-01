CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_agent_session_awaiting_link ON linear_agent_session (workspace_id, connection_id, linear_issue_id) WHERE status = 'awaiting_issue_link';
