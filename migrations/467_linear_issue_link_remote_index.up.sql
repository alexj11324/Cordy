CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_issue_link_remote
    ON linear_issue_link (workspace_id, linear_issue_id);
