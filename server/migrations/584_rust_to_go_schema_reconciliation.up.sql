-- Reconcile the intentional schema differences between the final Rust
-- production series and the Go mainline after equivalent migration names have
-- been recorded. This is a no-op on a fresh Go database apart from replacing
-- the reviewer check with the same canonical definition.

ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_reviewer_pair_check;
ALTER TABLE issue ADD CONSTRAINT issue_reviewer_pair_check CHECK (
    (reviewer_type IS NULL AND reviewer_id IS NULL)
    OR (reviewer_type IN ('member', 'agent', 'team') AND reviewer_id IS NOT NULL)
);

-- Rust stored Linear secretbox ciphertext as standard-base64 TEXT; Go stores
-- the raw sealed bytes. Convert only the historical representation so every
-- credential remains byte-for-byte compatible with Go's secretbox reader.
DO $linear_secret_storage$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema = current_schema()
          AND table_name = 'linear_connection'
          AND column_name = 'access_token_encrypted'
          AND data_type = 'text'
    ) THEN
        ALTER TABLE linear_connection
            ALTER COLUMN access_token_encrypted TYPE BYTEA
                USING decode(access_token_encrypted, 'base64'),
            ALTER COLUMN refresh_token_encrypted TYPE BYTEA
                USING decode(refresh_token_encrypted, 'base64');
    END IF;

    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema = current_schema()
          AND table_name = 'linear_oauth_state'
          AND column_name = 'code_verifier_encrypted'
          AND data_type = 'text'
    ) THEN
        ALTER TABLE linear_oauth_state
            ALTER COLUMN code_verifier_encrypted TYPE BYTEA
                USING decode(code_verifier_encrypted, 'base64');
    END IF;
END
$linear_secret_storage$;

ALTER TABLE linear_connection ALTER COLUMN id SET DEFAULT gen_random_uuid();
ALTER TABLE linear_oauth_state ALTER COLUMN id SET DEFAULT gen_random_uuid();
ALTER TABLE linear_project_binding ALTER COLUMN id SET DEFAULT gen_random_uuid();
ALTER TABLE linear_issue_link ALTER COLUMN id SET DEFAULT gen_random_uuid();
ALTER TABLE linear_sync_inbox ALTER COLUMN id SET DEFAULT gen_random_uuid();
ALTER TABLE linear_sync_outbox ALTER COLUMN id SET DEFAULT gen_random_uuid();
ALTER TABLE linear_member_binding ALTER COLUMN id SET DEFAULT gen_random_uuid();
ALTER TABLE linear_sync_conflict ALTER COLUMN id SET DEFAULT gen_random_uuid();
