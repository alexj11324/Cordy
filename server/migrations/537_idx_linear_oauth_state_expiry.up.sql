CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_linear_oauth_state_expiry
    ON linear_oauth_state (expires_at) WHERE consumed_at IS NULL;
