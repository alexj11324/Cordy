-- The index is built concurrently in 608. Existing development databases
-- already have this primary key from the pre-release table definition.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conrelid = 'device_authorization'::regclass AND contype = 'p'
    ) THEN
        ALTER TABLE device_authorization
            ADD CONSTRAINT device_authorization_pkey
            PRIMARY KEY USING INDEX device_authorization_pkey;
    END IF;
END;
$$;
