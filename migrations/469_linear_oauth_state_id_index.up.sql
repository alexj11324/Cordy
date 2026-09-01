CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS uq_linear_oauth_state_id
    ON linear_oauth_state (id);
