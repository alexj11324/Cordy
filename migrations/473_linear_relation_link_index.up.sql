CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_relation_link_pair
    ON linear_relation_link (workspace_id, from_issue_id, to_issue_id, relation_type);
