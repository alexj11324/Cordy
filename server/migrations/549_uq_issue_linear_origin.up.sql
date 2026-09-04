CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_issue_linear_origin
    ON issue (workspace_id, origin_type, origin_id) WHERE origin_type = 'linear' AND origin_id IS NOT NULL;
