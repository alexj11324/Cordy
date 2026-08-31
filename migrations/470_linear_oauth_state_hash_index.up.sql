CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_oauth_state_hash
    ON linear_oauth_state (state_hash);
