CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_sync_outbox_pending_issue
    ON linear_sync_outbox (binding_id, issue_id, created_at, id) WHERE processed_at IS NULL AND dead_lettered_at IS NULL;
