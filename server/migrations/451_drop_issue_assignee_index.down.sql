CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_issue_assignee
    ON issue (executor_type, executor_id);
