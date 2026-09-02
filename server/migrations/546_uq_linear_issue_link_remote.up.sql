CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_issue_link_remote
    ON linear_issue_link (binding_id, linear_issue_id) WHERE sync_status <> 'deleted';
