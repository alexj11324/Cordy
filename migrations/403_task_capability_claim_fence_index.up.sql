CREATE UNIQUE INDEX CONCURRENTLY task_token_active_claim_fence_idx ON task_token(task_id, claim_dispatched_at) WHERE revoked_at IS NULL AND claim_dispatched_at IS NOT NULL;
