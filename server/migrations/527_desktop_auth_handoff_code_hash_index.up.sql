CREATE UNIQUE INDEX CONCURRENTLY desktop_auth_handoff_code_hash_uidx
    ON desktop_auth_handoff (code_hash)
    WHERE code_hash IS NOT NULL;
