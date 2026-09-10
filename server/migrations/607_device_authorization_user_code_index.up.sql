CREATE UNIQUE INDEX CONCURRENTLY device_authorization_user_code_hash_uidx
    ON device_authorization (user_code_hash);
