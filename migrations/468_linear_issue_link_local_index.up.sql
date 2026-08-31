CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_issue_link_local
    ON linear_issue_link (workspace_id, issue_id);
