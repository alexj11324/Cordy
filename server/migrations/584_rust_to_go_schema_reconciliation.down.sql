-- A fresh Go database already had these definitions before migration 584, so
-- only undo changes owned by a retained Rust migration ledger.
DO $rust_to_go_reconciliation_down$
BEGIN
    IF EXISTS (
        SELECT 1 FROM schema_migrations WHERE version = '454_issue_roles'
    ) THEN
        ALTER TABLE issue DROP CONSTRAINT IF EXISTS issue_reviewer_pair_check;
    END IF;

    IF EXISTS (
        SELECT 1 FROM schema_migrations
        WHERE version = '465_linear_installation_foundation'
    ) THEN
        IF EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema = current_schema()
              AND table_name = 'linear_connection'
              AND column_name = 'access_token_encrypted'
              AND data_type = 'bytea'
        ) THEN
            ALTER TABLE linear_connection
                ALTER COLUMN access_token_encrypted TYPE TEXT
                    USING encode(access_token_encrypted, 'base64'),
                ALTER COLUMN refresh_token_encrypted TYPE TEXT
                    USING encode(refresh_token_encrypted, 'base64');
        END IF;
        IF EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema = current_schema()
              AND table_name = 'linear_oauth_state'
              AND column_name = 'code_verifier_encrypted'
              AND data_type = 'bytea'
        ) THEN
            ALTER TABLE linear_oauth_state
                ALTER COLUMN code_verifier_encrypted TYPE TEXT
                    USING encode(code_verifier_encrypted, 'base64');
        END IF;

        ALTER TABLE linear_connection ALTER COLUMN id DROP DEFAULT;
        ALTER TABLE linear_oauth_state ALTER COLUMN id DROP DEFAULT;
        ALTER TABLE linear_project_binding ALTER COLUMN id DROP DEFAULT;
        ALTER TABLE linear_issue_link ALTER COLUMN id DROP DEFAULT;
        ALTER TABLE linear_sync_inbox ALTER COLUMN id DROP DEFAULT;
        ALTER TABLE linear_sync_outbox ALTER COLUMN id DROP DEFAULT;
        ALTER TABLE linear_member_binding ALTER COLUMN id DROP DEFAULT;
        ALTER TABLE linear_sync_conflict ALTER COLUMN id DROP DEFAULT;
    END IF;
END
$rust_to_go_reconciliation_down$;
