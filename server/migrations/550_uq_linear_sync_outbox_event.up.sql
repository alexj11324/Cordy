CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_sync_outbox_event
    ON linear_sync_outbox (binding_id, event_key);
