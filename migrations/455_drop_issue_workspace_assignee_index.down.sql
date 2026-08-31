CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_issue_workspace_assignee
    ON issue (workspace_id, assignee_type, assignee_id);
