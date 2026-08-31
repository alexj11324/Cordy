CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_issue_workspace_assignee
    ON issue (workspace_id, executor_type, executor_id);
