-- Phase 1 authorization foundation. Policy rows are deliberately separate
-- from protected resources: this is a grant ledger, not a universal resource
-- registry. Relationships are application-enforced; no foreign keys/cascades.
CREATE TABLE IF NOT EXISTS authorization_grant (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL,
    principal_type TEXT NOT NULL CHECK (principal_type IN (
        'user', 'team', 'agent_definition', 'task_run',
        'device_runtime', 'service', 'system'
    )),
    principal_id UUID,
    action TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id UUID,
    effect TEXT NOT NULL CHECK (effect IN ('allow', 'deny', 'require_approval')),
    conditions JSONB NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(conditions) = 'object'),
    expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    revoked_by UUID,
    created_by UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE authorization_grant IS
    'Permanent RBAC/ReBAC/ABAC grants. Explicit deny wins. A grant never widens a task_run beyond its task_token capability lease.';

CREATE TABLE IF NOT EXISTS authorization_audit_event (
    id UUID NOT NULL DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL,
    principal_type TEXT NOT NULL,
    principal_id UUID,
    on_behalf_of_user_id UUID,
    via_agent_id UUID,
    device_id UUID,
    action TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id UUID,
    decision TEXT NOT NULL CHECK (decision IN ('allow', 'deny', 'require_approval')),
    reason TEXT NOT NULL,
    matched_grant_ids UUID[] NOT NULL DEFAULT '{}',
    policy_version TEXT NOT NULL,
    obligations JSONB NOT NULL DEFAULT '[]'::jsonb
        CHECK (jsonb_typeof(obligations) = 'array'),
    delegation_chain JSONB NOT NULL DEFAULT '[]'::jsonb
        CHECK (jsonb_typeof(delegation_chain) = 'array'),
    context JSONB NOT NULL DEFAULT '{}'::jsonb
        CHECK (jsonb_typeof(context) = 'object'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE authorization_audit_event IS
    'Append-only explain ledger: who/on_behalf_of/via/device/action/resource/decision/why plus matched grants, policy and obligations. Never stores bearer tokens or secrets.';

-- Evolve mat_ task tokens in place into short-lived capability leases. Existing
-- rows receive only the legacy agent.invoke capability; all new claims replace
-- this with a server-computed scope.
ALTER TABLE task_token
    ADD COLUMN IF NOT EXISTS scope JSONB NOT NULL DEFAULT
        '[{"action":"agent.invoke","resource_type":"agent_definition","resource_id":"*"}]'::jsonb
        CHECK (jsonb_typeof(scope) = 'array'),
    ADD COLUMN IF NOT EXISTS parent_token_id UUID,
    ADD COLUMN IF NOT EXISTS parent_fence BIGINT,
    ADD COLUMN IF NOT EXISTS delegation_depth INT NOT NULL DEFAULT 0
        CHECK (delegation_depth BETWEEN 0 AND 8),
    ADD COLUMN IF NOT EXISTS delegation_fence BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS claim_dispatched_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS on_behalf_of_user_id UUID,
    ADD COLUMN IF NOT EXISTS device_id UUID,
    ADD COLUMN IF NOT EXISTS revoked_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS revoked_reason TEXT;

COMMENT ON TABLE task_token IS
    'Short-lived, revocable task capability leases. Raw mat_ bearer values are never stored. scope is server-computed; parent/depth/fences enforce monotonic delegation.';

COMMENT ON COLUMN task_token.user_id IS
    'Compatibility workspace-guard projection of the initiating human. Active leases never project the Agent or Runtime owner through this field.';

-- Extend the task-owner write fence introduced by migration 284: an archived
-- Agent is not a valid task owner. Archive takes FOR UPDATE on the same Agent
-- row, so a concurrent enqueue's FOR KEY SHARE blocks and then re-checks this
-- predicate against the committed archived state.
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
        SELECT a.workspace_id FROM agent a
        WHERE a.id = p_agent_id AND a.archived_at IS NULL
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
            SELECT a.workspace_id FROM agent a
            WHERE a.id = p_agent_id AND a.archived_at IS NULL
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
        PERFORM 1 FROM agent
        WHERE id = p_agent_id AND archived_at IS NULL
        FOR KEY SHARE;
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

-- A previous unrecorded execution may already have installed the immutability
-- trigger. Remove it before replaying the deterministic legacy-token backfill;
-- it is recreated after the backfill and deduplication complete.
DROP TRIGGER IF EXISTS trg_task_capability_lease_immutable ON task_token;

UPDATE task_token token
SET claim_dispatched_at = task.dispatched_at,
    on_behalf_of_user_id = task.originator_user_id,
    user_id = COALESCE(task.originator_user_id, token.user_id),
    device_id = task.runtime_id,
    revoked_at = CASE
        WHEN task.originator_user_id IS NULL
          OR task.runtime_id IS NULL
          OR task.delegated_from_task_id IS NOT NULL
            THEN COALESCE(token.revoked_at, now())
        ELSE token.revoked_at
    END,
    revoked_reason = COALESCE(
        token.revoked_reason,
        CASE
            WHEN task.originator_user_id IS NULL THEN 'migration_missing_on_behalf_identity'
            WHEN task.runtime_id IS NULL THEN 'migration_missing_device_binding'
            -- Historical child tokens did not record the exact parent token/fence.
            -- Treating them as roots would silently discard ancestor revocation and
            -- scope, so deployment settles them explicitly instead of guessing a
            -- delegation chain that cannot be proven from current-main data.
            WHEN task.delegated_from_task_id IS NOT NULL THEN 'migration_unfenced_delegation'
        END
    )
FROM agent_task_queue task
WHERE task.id = token.task_id;

-- If historical claim retries left multiple tokens for the same dispatch,
-- preserve only the newest. Claim consumption is immutable even after
-- revocation, so duplicate historical rows cannot remain before the
-- unconditional claim-fence index is built.
WITH duplicates AS (
    SELECT id,
           row_number() OVER (
               PARTITION BY task_id, claim_dispatched_at
               ORDER BY created_at DESC, id DESC
           ) AS ordinal
    FROM task_token
    WHERE claim_dispatched_at IS NOT NULL
)
DELETE FROM task_token token
USING duplicates
WHERE token.id = duplicates.id AND duplicates.ordinal > 1;

-- Rank only the surviving claim rows so a replay cannot renumber a token that
-- was retained from a duplicate historical dispatch.
WITH ranked AS (
    SELECT id,
           row_number() OVER (PARTITION BY task_id ORDER BY created_at, id) AS fence
    FROM task_token
)
UPDATE task_token token
SET delegation_fence = ranked.fence
FROM ranked
WHERE token.id = ranked.id;

CREATE OR REPLACE FUNCTION enforce_task_capability_lease_immutability()
RETURNS TRIGGER AS $$
BEGIN
    IF OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS NULL THEN
        RAISE EXCEPTION 'revoked task capability leases cannot be revived';
    END IF;
    IF NEW.scope IS DISTINCT FROM OLD.scope
       OR NEW.token_hash IS DISTINCT FROM OLD.token_hash
       OR NEW.parent_token_id IS DISTINCT FROM OLD.parent_token_id
       OR NEW.parent_fence IS DISTINCT FROM OLD.parent_fence
       OR NEW.delegation_depth IS DISTINCT FROM OLD.delegation_depth
       OR NEW.delegation_fence IS DISTINCT FROM OLD.delegation_fence
       OR NEW.claim_dispatched_at IS DISTINCT FROM OLD.claim_dispatched_at
       OR NEW.task_id IS DISTINCT FROM OLD.task_id
       OR NEW.agent_id IS DISTINCT FROM OLD.agent_id
       OR NEW.workspace_id IS DISTINCT FROM OLD.workspace_id
       OR NEW.user_id IS DISTINCT FROM OLD.user_id
       OR NEW.expires_at IS DISTINCT FROM OLD.expires_at
       OR NEW.on_behalf_of_user_id IS DISTINCT FROM OLD.on_behalf_of_user_id
       OR NEW.device_id IS DISTINCT FROM OLD.device_id THEN
        RAISE EXCEPTION 'task capability lease identity and scope are immutable';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_task_capability_lease_immutable
    BEFORE UPDATE ON task_token
    FOR EACH ROW
    EXECUTE FUNCTION enforce_task_capability_lease_immutability();

CREATE OR REPLACE FUNCTION revoke_task_capability_leases_on_task_change()
RETURNS TRIGGER AS $$
BEGIN
    IF NEW.agent_id IS DISTINCT FROM OLD.agent_id
       OR NEW.runtime_id IS DISTINCT FROM OLD.runtime_id
       OR NEW.originator_user_id IS DISTINCT FROM OLD.originator_user_id
       OR NEW.dispatched_at IS DISTINCT FROM OLD.dispatched_at THEN
        UPDATE task_token
        SET revoked_at = COALESCE(revoked_at, now()),
            revoked_reason = COALESCE(revoked_reason, 'task_identity_changed')
        WHERE task_id = NEW.id AND revoked_at IS NULL;
    ELSIF NEW.status IN ('completed', 'failed', 'cancelled')
       AND OLD.status IS DISTINCT FROM NEW.status THEN
        UPDATE task_token
        SET revoked_at = COALESCE(revoked_at, now()),
            revoked_reason = COALESCE(revoked_reason, 'task_' || NEW.status)
        WHERE task_id = NEW.id AND revoked_at IS NULL;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_revoke_task_capability_leases ON agent_task_queue;
CREATE TRIGGER trg_revoke_task_capability_leases
    AFTER UPDATE OF status, dispatched_at, agent_id, runtime_id, originator_user_id
    ON agent_task_queue
    FOR EACH ROW
    EXECUTE FUNCTION revoke_task_capability_leases_on_task_change();
