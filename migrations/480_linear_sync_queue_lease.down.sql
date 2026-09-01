ALTER TABLE linear_sync_inbox
    DROP COLUMN IF EXISTS dead_lettered_at,
    DROP COLUMN IF EXISTS max_attempts,
    DROP COLUMN IF EXISTS locked_until,
    DROP COLUMN IF EXISTS locked_by,
    DROP COLUMN IF EXISTS available_at;
