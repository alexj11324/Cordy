CREATE UNIQUE INDEX CONCURRENTLY guest_session_transfer_token_hash_uidx
    ON guest_session_transfer (token_hash);
