CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS task_token_claim_fence_idx ON task_token(task_id, claim_dispatched_at) WHERE claim_dispatched_at IS NOT NULL;
