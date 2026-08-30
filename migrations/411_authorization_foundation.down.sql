DROP TRIGGER IF EXISTS trg_revoke_task_capability_leases ON agent_task_queue;
DROP FUNCTION IF EXISTS revoke_task_capability_leases_on_task_change();
DROP TRIGGER IF EXISTS trg_task_capability_lease_immutable ON task_token;
DROP FUNCTION IF EXISTS enforce_task_capability_lease_immutability();

-- Restore migration 284's pre-authorization task-owner fence. The Phase 1
-- archived-Agent enqueue restriction belongs to the application version that
-- consumes the capability columns removed below.
CREATE OR REPLACE FUNCTION lock_task_owner_rows(
    p_agent_id uuid,
    p_issue_id uuid,
    p_runtime_id uuid
)
RETURNS boolean
LANGUAGE plpgsql
AS $$
DECLARE
    required int := (CASE WHEN p_agent_id IS NULL THEN 0 ELSE 1 END)
                  + (CASE WHEN p_issue_id IS NULL THEN 0 ELSE 1 END)
                  + (CASE WHEN p_runtime_id IS NULL THEN 0 ELSE 1 END);
    resolved int;
    distinct_workspaces int;
    locked int;
BEGIN
    IF required = 0 THEN
        RETURN TRUE;
    END IF;

    WITH owners AS (
        SELECT a.workspace_id FROM agent a WHERE a.id = p_agent_id
        UNION ALL
        SELECT i.workspace_id FROM issue i WHERE i.id = p_issue_id
        UNION ALL
        SELECT r.workspace_id FROM agent_runtime r WHERE r.id = p_runtime_id
    )
    SELECT count(*), count(DISTINCT workspace_id)
    INTO resolved, distinct_workspaces
    FROM owners;

    IF resolved <> required THEN
        RETURN FALSE;
    END IF;

    WITH locked_workspaces AS (
        SELECT w.id
        FROM workspace w
        WHERE w.id IN (
            SELECT a.workspace_id FROM agent a WHERE a.id = p_agent_id
            UNION
            SELECT i.workspace_id FROM issue i WHERE i.id = p_issue_id
            UNION
            SELECT r.workspace_id FROM agent_runtime r WHERE r.id = p_runtime_id
        )
        ORDER BY w.id
        FOR KEY SHARE
    )
    SELECT count(*) INTO locked FROM locked_workspaces;

    IF locked <> distinct_workspaces THEN
        RETURN FALSE;
    END IF;

    locked := 0;

    IF p_agent_id IS NOT NULL THEN
        PERFORM 1 FROM agent WHERE id = p_agent_id FOR KEY SHARE;
        IF FOUND THEN locked := locked + 1; END IF;
    END IF;

    IF p_issue_id IS NOT NULL THEN
        PERFORM 1 FROM issue WHERE id = p_issue_id FOR KEY SHARE;
        IF FOUND THEN locked := locked + 1; END IF;
    END IF;

    IF p_runtime_id IS NOT NULL THEN
        PERFORM 1 FROM agent_runtime WHERE id = p_runtime_id FOR KEY SHARE;
        IF FOUND THEN locked := locked + 1; END IF;
    END IF;

    RETURN locked = required;
END;
$$;

-- The legacy schema cannot represent scope, delegation, revocation, identity,
-- or claim fencing. There is no safe way to retain even an active Phase 1
-- bearer without widening it when these columns and the new binary boundary
-- disappear, so a security rollback deliberately interrupts every task lease.
DELETE FROM task_token;

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
