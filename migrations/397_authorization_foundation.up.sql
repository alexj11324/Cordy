-- Phase 1 authorization foundation. Policy rows are deliberately separate
-- from protected resources: this is a grant ledger, not a universal resource
-- registry. Relationships are application-enforced; no foreign keys/cascades.
CREATE TABLE authorization_grant (
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

CREATE TABLE authorization_audit_event (
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
    ADD COLUMN scope JSONB NOT NULL DEFAULT
        '[{"action":"agent.invoke","resource_type":"agent_definition","resource_id":"*"}]'::jsonb
        CHECK (jsonb_typeof(scope) = 'array'),
    ADD COLUMN parent_token_id UUID,
    ADD COLUMN parent_fence BIGINT,
    ADD COLUMN delegation_depth INT NOT NULL DEFAULT 0
        CHECK (delegation_depth BETWEEN 0 AND 8),
    ADD COLUMN delegation_fence BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN claim_dispatched_at TIMESTAMPTZ,
    ADD COLUMN on_behalf_of_user_id UUID,
    ADD COLUMN device_id UUID,
    ADD COLUMN revoked_at TIMESTAMPTZ,
    ADD COLUMN revoked_reason TEXT;

COMMENT ON TABLE task_token IS
    'Short-lived, revocable task capability leases. Raw mat_ bearer values are never stored. scope is server-computed; parent/depth/fences enforce monotonic delegation.';

COMMENT ON COLUMN task_token.user_id IS
    'Compatibility workspace-guard projection of the initiating human. Active leases never project the Agent or Runtime owner through this field.';

WITH ranked AS (
    SELECT id,
           row_number() OVER (PARTITION BY task_id ORDER BY created_at, id) AS fence
    FROM task_token
)
UPDATE task_token token
SET delegation_fence = ranked.fence
FROM ranked
WHERE token.id = ranked.id;

UPDATE task_token token
SET claim_dispatched_at = task.dispatched_at,
    on_behalf_of_user_id = task.originator_user_id,
    user_id = COALESCE(task.originator_user_id, token.user_id),
    revoked_at = CASE
        WHEN task.originator_user_id IS NULL THEN now()
        ELSE token.revoked_at
    END,
    revoked_reason = CASE
        WHEN task.originator_user_id IS NULL THEN 'migration_missing_on_behalf_identity'
        ELSE token.revoked_reason
    END
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

CREATE TRIGGER trg_revoke_task_capability_leases
    AFTER UPDATE OF status, dispatched_at, agent_id, runtime_id, originator_user_id
    ON agent_task_queue
    FOR EACH ROW
    EXECUTE FUNCTION revoke_task_capability_leases_on_task_change();
