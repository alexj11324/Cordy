CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_agent_session_issue
    ON linear_agent_session (workspace_id, patchbay_issue_id, status, updated_at, id);
