CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_issue_workspace_owner
    ON issue (workspace_id, owner_type, owner_id);
