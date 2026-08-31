-- Recreate while the role columns still use their 453 names. PostgreSQL keeps
-- the index attached to the renamed columns when 453 is rolled back.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_issue_assignee
    ON issue (executor_type, executor_id);
