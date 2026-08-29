CREATE UNIQUE INDEX CONCURRENTLY guest_session_token_hash_uidx
    ON guest_session (token_hash);
