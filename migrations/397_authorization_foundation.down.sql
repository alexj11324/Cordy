DROP TRIGGER IF EXISTS trg_revoke_task_capability_leases ON agent_task_queue;
DROP FUNCTION IF EXISTS revoke_task_capability_leases_on_terminal_state();
DROP TRIGGER IF EXISTS trg_task_capability_lease_immutable ON task_token;
DROP FUNCTION IF EXISTS enforce_task_capability_lease_immutability();

-- The legacy schema cannot represent revocation or terminal claim fencing.
-- Remove those bearer rows before dropping the columns so rollback cannot
-- silently revive authority that Phase 1 already invalidated.
DELETE FROM task_token token
USING agent_task_queue task
WHERE task.id = token.task_id
  AND (
      token.revoked_at IS NOT NULL
      OR token.expires_at <= now()
      OR task.status NOT IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
      OR token.claim_dispatched_at IS DISTINCT FROM task.dispatched_at
  );

-- An orphan bearer cannot be validated once the Phase 1 task/claim columns
-- are gone. Delete it explicitly before restoring the legacy lookup shape.
DELETE FROM task_token token
WHERE NOT EXISTS (
    SELECT 1 FROM agent_task_queue task WHERE task.id = token.task_id
);

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
