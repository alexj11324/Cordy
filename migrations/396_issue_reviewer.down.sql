ALTER TABLE issue
    DROP COLUMN IF EXISTS reviewer_id,
    DROP COLUMN IF EXISTS reviewer_type;
