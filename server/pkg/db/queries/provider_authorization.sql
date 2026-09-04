-- name: CreateProviderAuthorizationGrant :one
INSERT INTO authorization_grant (
    id, workspace_id, principal_type, principal_id, action, resource_type,
    resource_id, effect, conditions, expires_at, created_by
) VALUES (
    @id, @workspace_id, @principal_type, @principal_id, 'credential.use',
    'provider_identity', @resource_id, @effect, @conditions, @expires_at,
    @created_by
)
RETURNING *;

-- name: ListProviderAuthorizationGrants :many
SELECT *
FROM authorization_grant
WHERE workspace_id = @workspace_id
  AND resource_type = 'provider_identity'
  AND (
      created_by = @actor_id
      OR (principal_type = 'user' AND principal_id = @actor_id)
      OR (principal_type = 'team' AND principal_id IN (
          SELECT team_id FROM team_member
          WHERE member_type = 'member' AND member_id = @actor_id
      ))
  )
ORDER BY created_at DESC, id DESC;

-- name: ListActiveProviderAuthorizationGrants :many
SELECT *
FROM authorization_grant
WHERE workspace_id = @workspace_id
  AND action = 'credential.use'
  AND resource_type = 'provider_identity'
  AND (resource_id IS NULL OR resource_id = @runtime_id)
  AND revoked_at IS NULL
  AND (expires_at IS NULL OR expires_at > now())
ORDER BY created_at, id;

-- name: GetProviderAuthorizationGrant :one
SELECT *
FROM authorization_grant
WHERE id = @id AND workspace_id = @workspace_id
  AND resource_type = 'provider_identity';

-- name: RevokeProviderAuthorizationGrant :execrows
UPDATE authorization_grant
SET revoked_at = now(), revoked_by = @actor_id, updated_at = now()
WHERE id = @id AND workspace_id = @workspace_id
  AND resource_type = 'provider_identity'
  AND created_by = @actor_id
  AND revoked_at IS NULL;

-- name: CreateAuthorizationAuditEvent :one
INSERT INTO authorization_audit_event (
    id, workspace_id, principal_type, principal_id, on_behalf_of_user_id,
    via_agent_id, device_id, action, resource_type, resource_id, decision,
    reason, matched_grant_ids, policy_version, obligations, delegation_chain,
    context
) VALUES (
    @id, @workspace_id, @principal_type, @principal_id,
    @on_behalf_of_user_id, @via_agent_id, @device_id, @action,
    @resource_type, @resource_id, @decision, @reason,
    @matched_grant_ids, @policy_version, @obligations, @delegation_chain,
    @context
)
RETURNING *;

-- name: CreateProviderAuthorizationDecision :one
-- Provider budget admission and the corresponding explain event must share one
-- transaction-level lock. A read-then-insert pair lets two daemons both see
-- the same remaining budget; this query serializes the read and records the
-- final decision atomically. It never stores bearer tokens or secrets.
WITH budget_lock AS (
    SELECT pg_advisory_xact_lock(hashtextextended(@budget_lock_key::text, 0))
), reservation_totals AS (
    SELECT COALESCE(sum(
        CASE
            WHEN event.context->>'provider_request_tokens' ~ '^[0-9]+$'
            THEN (event.context->>'provider_request_tokens')::bigint
            ELSE 0::bigint
        END
    ), 0)::bigint AS total_reserved
    FROM authorization_audit_event event
    CROSS JOIN budget_lock
    WHERE event.workspace_id = @workspace_id
      AND event.action = 'credential.use'
      AND event.resource_type = 'provider_identity'
      AND event.resource_id = @resource_id
      AND event.decision = 'allow'
      AND event.matched_grant_ids && @matched_grant_ids::uuid[]
      AND event.context->>'provider_budget_reservation' = 'true'
), final_decision AS (
    SELECT
        CASE
            WHEN @enforce_budget::boolean
             AND @decision::text = 'allow'
             AND reservation_totals.total_reserved > @budget_limit::bigint - @reservation::bigint
            THEN 'deny'
            ELSE @decision::text
        END AS decision,
        CASE
            WHEN @enforce_budget::boolean
             AND @decision::text = 'allow'
             AND reservation_totals.total_reserved > @budget_limit::bigint - @reservation::bigint
            THEN @budget_exhausted_reason::text
            ELSE @reason::text
        END AS reason,
        CASE
            WHEN @enforce_budget::boolean
             AND @decision::text = 'allow'
             AND reservation_totals.total_reserved > @budget_limit::bigint - @reservation::bigint
            THEN 0::bigint
            ELSE @reservation::bigint
        END AS reservation
    FROM reservation_totals
)
INSERT INTO authorization_audit_event (
    id, workspace_id, principal_type, principal_id, on_behalf_of_user_id,
    via_agent_id, device_id, action, resource_type, resource_id, decision,
    reason, matched_grant_ids, policy_version, obligations, delegation_chain,
    context
)
SELECT
    @id, @workspace_id, @principal_type, @principal_id, @on_behalf_of_user_id,
    @via_agent_id, @device_id, @action, @resource_type, @resource_id,
    final_decision.decision, final_decision.reason, @matched_grant_ids,
    @policy_version, @obligations, @delegation_chain,
    jsonb_set(
        jsonb_set(
            @context::jsonb,
            '{provider_request_tokens}',
            to_jsonb(final_decision.reservation)
        ),
        '{provider_budget_reservation}',
        to_jsonb(final_decision.decision = 'allow' AND final_decision.reservation > 0)
    )
FROM final_decision
RETURNING id, workspace_id, principal_type, principal_id, on_behalf_of_user_id,
          via_agent_id, device_id, action, resource_type, resource_id, decision,
          reason, matched_grant_ids, policy_version, obligations,
          delegation_chain, context, created_at;

-- name: GetAuthorizationDecision :one
SELECT *
FROM authorization_audit_event
WHERE id = @id AND workspace_id = @workspace_id;

-- name: GetTaskCapabilityLease :one
SELECT * FROM task_token WHERE id = @id AND workspace_id = @workspace_id;

-- name: SumProviderAuthorizationReservations :one
SELECT COALESCE(sum(
    CASE
        WHEN context->>'provider_request_tokens' ~ '^[0-9]+$'
        THEN (context->>'provider_request_tokens')::bigint
        ELSE 0::bigint
    END
), 0)::bigint
FROM authorization_audit_event
WHERE workspace_id = @workspace_id
  AND action = 'credential.use'
  AND resource_type = 'provider_identity'
  AND resource_id = @runtime_id
  AND decision = 'allow'
  AND matched_grant_ids && @grant_ids::uuid[]
  AND context->>'provider_budget_reservation' = 'true'
  AND context->>'provider_request_tokens' ~ '^[0-9]+$';

-- Structural validity of one capability lease, keyed by lease id rather than
-- by the bearer hash: the daemon pre-operation check names the lease it holds,
-- and the raw mat_ value never travels on that path. Same chain walk as
-- GetTaskTokenByHash — revocation, expiry, task state, claim fence, and
-- monotonic delegation scope — so a lease cannot be validated here on terms
-- the bearer path would refuse. Returns no row when the lease is unusable;
-- binding to a specific task/agent/runtime/actor and the provider capability
-- itself are checked by the caller against the returned row.
-- name: GetValidTaskCapabilityLease :one
WITH RECURSIVE lease_chain AS (
    SELECT token.id, token.task_id, token.agent_id, token.workspace_id,
           token.scope, token.parent_token_id, token.parent_fence,
           token.delegation_depth, token.delegation_fence,
           token.claim_dispatched_at, token.on_behalf_of_user_id,
           token.device_id, token.revoked_at, token.revoked_reason,
           token.expires_at, token.created_at, token.token_hash, token.user_id,
           task.status AS task_status, task.dispatched_at AS current_dispatched_at,
           task.agent_id AS current_agent_id, task.runtime_id AS current_device_id,
           task.originator_user_id AS current_on_behalf_of_user_id,
           current_agent.workspace_id AS current_workspace_id,
           current_agent.archived_at AS current_agent_archived_at,
           ARRAY[token.id] AS path
    FROM task_token token
    JOIN agent_task_queue task ON task.id = token.task_id
    JOIN agent current_agent ON current_agent.id = task.agent_id
    WHERE token.id = @lease_id
  UNION ALL
    SELECT parent.id, parent.task_id, parent.agent_id, parent.workspace_id,
           parent.scope, parent.parent_token_id, parent.parent_fence,
           parent.delegation_depth, parent.delegation_fence,
           parent.claim_dispatched_at, parent.on_behalf_of_user_id,
           parent.device_id, parent.revoked_at, parent.revoked_reason,
           parent.expires_at, parent.created_at, parent.token_hash, parent.user_id,
           task.status, task.dispatched_at,
           task.agent_id, task.runtime_id, task.originator_user_id,
           current_agent.workspace_id, current_agent.archived_at,
           child.path || parent.id
    FROM task_token parent
    JOIN lease_chain child ON child.parent_token_id = parent.id
    JOIN agent_task_queue task ON task.id = parent.task_id
    JOIN agent current_agent ON current_agent.id = task.agent_id
    WHERE NOT parent.id = ANY(child.path)
      AND cardinality(child.path) <= 9
), leaf AS (
    SELECT * FROM lease_chain WHERE id = @lease_id
), invalid AS (
    SELECT 1
    FROM lease_chain lease
    LEFT JOIN lease_chain parent ON parent.id = lease.parent_token_id
    WHERE lease.revoked_at IS NOT NULL
       OR lease.expires_at <= now()
       OR lease.task_status NOT IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
       OR lease.claim_dispatched_at IS DISTINCT FROM lease.current_dispatched_at
       OR lease.agent_id <> lease.current_agent_id
       OR lease.device_id IS DISTINCT FROM lease.current_device_id
       OR lease.on_behalf_of_user_id IS DISTINCT FROM lease.current_on_behalf_of_user_id
       OR lease.workspace_id <> lease.current_workspace_id
       OR lease.current_agent_archived_at IS NOT NULL
       OR lease.delegation_depth > 8
       OR (lease.parent_token_id IS NULL AND lease.delegation_depth <> 0)
           OR (lease.parent_token_id IS NOT NULL AND (
              parent.id IS NULL
              OR lease.delegation_depth <> parent.delegation_depth + 1
              OR lease.parent_fence IS DISTINCT FROM parent.delegation_fence
              OR lease.agent_id <> parent.agent_id
              OR lease.workspace_id <> parent.workspace_id
              OR lease.on_behalf_of_user_id IS DISTINCT FROM parent.on_behalf_of_user_id
              OR lease.device_id IS DISTINCT FROM parent.device_id
              OR EXISTS (
                  SELECT 1
                  FROM jsonb_array_elements(lease.scope) child_cap(capability)
                  WHERE NOT EXISTS (
                      SELECT 1
                      FROM jsonb_array_elements(parent.scope) parent_cap(capability)
                      WHERE parent_cap.capability->>'action' = child_cap.capability->>'action'
                        AND parent_cap.capability->>'resource_type' = child_cap.capability->>'resource_type'
                        AND (
                            parent_cap.capability->>'resource_id' = '*'
                            OR parent_cap.capability->>'resource_id' = child_cap.capability->>'resource_id'
                        )
                  )
              )
          ))
    LIMIT 1
)
SELECT id, token_hash, task_id, agent_id, workspace_id, user_id,
       expires_at, created_at, scope, parent_token_id, parent_fence,
       delegation_depth, delegation_fence, claim_dispatched_at,
       on_behalf_of_user_id, device_id, revoked_at, revoked_reason
FROM leaf
WHERE NOT EXISTS (SELECT 1 FROM invalid)
  AND (SELECT count(*) FROM lease_chain) = delegation_depth + 1
  AND EXISTS (
      SELECT 1 FROM lease_chain root
      WHERE root.parent_token_id IS NULL AND root.delegation_depth = 0
  );
