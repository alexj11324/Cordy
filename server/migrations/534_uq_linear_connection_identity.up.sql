CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_connection_identity
    ON linear_connection (organization_id, webhook_id) WHERE status <> 'revoked' AND webhook_id IS NOT NULL;
