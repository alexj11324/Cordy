CREATE INDEX CONCURRENTLY IF NOT EXISTS task_token_parent_token_id_idx ON task_token(parent_token_id) WHERE parent_token_id IS NOT NULL;
