CREATE UNIQUE INDEX CONCURRENTLY device_authorization_device_code_hash_uidx
    ON device_authorization (device_code_hash);
