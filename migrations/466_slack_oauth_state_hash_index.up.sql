CREATE UNIQUE INDEX CONCURRENTLY slack_oauth_state_hash_uidx
    ON slack_oauth_state(state_hash);
