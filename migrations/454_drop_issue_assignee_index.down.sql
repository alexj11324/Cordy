CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_issue_assignee
    ON issue (assignee_type, assignee_id);
