DROP TRIGGER IF EXISTS trg_revoke_task_capability_leases ON agent_task_queue;
DROP FUNCTION IF EXISTS revoke_task_capability_leases_on_terminal_state();
DROP TRIGGER IF EXISTS trg_task_capability_lease_immutable ON task_token;
DROP FUNCTION IF EXISTS enforce_task_capability_lease_immutability();

ALTER TABLE task_token
    DROP COLUMN IF EXISTS revoked_reason,
    DROP COLUMN IF EXISTS revoked_at,
    DROP COLUMN IF EXISTS device_id,
    DROP COLUMN IF EXISTS on_behalf_of_user_id,
    DROP COLUMN IF EXISTS claim_dispatched_at,
    DROP COLUMN IF EXISTS delegation_fence,
    DROP COLUMN IF EXISTS delegation_depth,
    DROP COLUMN IF EXISTS parent_fence,
    DROP COLUMN IF EXISTS parent_token_id,
    DROP COLUMN IF EXISTS scope;

COMMENT ON COLUMN task_token.user_id IS NULL;
COMMENT ON TABLE task_token IS NULL;

DROP TABLE IF EXISTS authorization_audit_event;
DROP TABLE IF EXISTS authorization_grant;
