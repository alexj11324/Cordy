-- Single-statement concurrent build by repository rule: PostgreSQL rejects
-- CREATE INDEX CONCURRENTLY inside a transaction or multi-command string.
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS slack_oauth_state_hash_uidx
    ON slack_oauth_state(state_hash);
