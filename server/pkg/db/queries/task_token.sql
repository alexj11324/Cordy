-- name: CreateTaskToken :one
WITH target_agent_guard AS (
    SELECT agent.id, agent.workspace_id
    FROM agent
    WHERE agent.id = @agent_id
      AND agent.workspace_id = @workspace_id
      AND agent.archived_at IS NULL
    FOR SHARE
), claim AS (
    SELECT task.id, task.delegated_from_task_id, task.dispatched_at
    FROM target_agent_guard agent_guard
    JOIN agent_task_queue task ON task.agent_id = agent_guard.id
    WHERE task.id = @task_id
      AND task.status IN ('dispatched', 'running', 'waiting_local_directory', 'deferred')
      AND task.dispatched_at IS NOT DISTINCT FROM @claim_dispatched_at::timestamptz
      AND task.agent_id = @agent_id
      AND task.originator_user_id IS NOT DISTINCT FROM @on_behalf_of_user_id::uuid
      AND task.runtime_id IS NOT DISTINCT FROM @device_id::uuid
    FOR SHARE OF task
), parent AS (
    SELECT token.id, token.scope, token.delegation_depth, token.delegation_fence,
           token.workspace_id, token.on_behalf_of_user_id, token.device_id
    FROM task_token token
    JOIN agent_task_queue task ON task.id = token.task_id
    JOIN agent current_agent ON current_agent.id = task.agent_id
    WHERE token.task_id = @parent_task_id
      AND token.revoked_at IS NULL
      AND token.expires_at > now()
      AND token.claim_dispatched_at = task.dispatched_at
      AND token.agent_id = task.agent_id
      AND token.device_id IS NOT DISTINCT FROM task.runtime_id
      AND token.on_behalf_of_user_id IS NOT DISTINCT FROM task.originator_user_id
      AND token.workspace_id = current_agent.workspace_id
      AND current_agent.archived_at IS NULL
      AND task.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
    ORDER BY token.created_at DESC, token.id DESC
    LIMIT 1
    FOR SHARE OF token
), lease AS (
    SELECT
        CASE WHEN @parent_task_id::uuid IS NULL THEN @scope::jsonb ELSE COALESCE((
            SELECT jsonb_agg(requested.capability)
            FROM jsonb_array_elements(@scope::jsonb) requested(capability)
            WHERE EXISTS (
                SELECT 1
                FROM parent, jsonb_array_elements(parent.scope) bound(capability)
                WHERE bound.capability->>'action' = requested.capability->>'action'
                  AND bound.capability->>'resource_type' = requested.capability->>'resource_type'
                  AND (
                      bound.capability->>'resource_id' = '*'
                      OR bound.capability->>'resource_id' = requested.capability->>'resource_id'
                  )
            )
        ), '[]'::jsonb) END AS effective_scope,
        parent.id AS parent_id,
        parent.delegation_fence AS parent_fence,
        COALESCE(parent.delegation_depth + 1, 0) AS depth
    FROM claim
    LEFT JOIN parent ON TRUE
    WHERE @parent_task_id::uuid IS NULL
       OR (claim.delegated_from_task_id = @parent_task_id
           AND parent.id IS NOT NULL
           AND parent.workspace_id = @workspace_id
           AND parent.on_behalf_of_user_id IS NOT DISTINCT FROM @on_behalf_of_user_id
           AND parent.device_id IS NOT DISTINCT FROM @device_id)
), inserted AS (
    INSERT INTO task_token (
        token_hash, task_id, agent_id, workspace_id, user_id, expires_at, id,
        scope, parent_token_id, parent_fence, delegation_depth,
        delegation_fence, claim_dispatched_at, on_behalf_of_user_id, device_id
    )
    SELECT @token_hash, @task_id, @agent_id, @workspace_id, @user_id, @expires_at, COALESCE(@id::uuid, gen_random_uuid()),
           lease.effective_scope, lease.parent_id, lease.parent_fence, lease.depth,
           @delegation_fence, @claim_dispatched_at, @on_behalf_of_user_id, @device_id
    FROM lease
    WHERE lease.depth <= 8
    ON CONFLICT (task_id, claim_dispatched_at)
        WHERE claim_dispatched_at IS NOT NULL
        DO NOTHING
    RETURNING *
)
SELECT id, token_hash, task_id, agent_id, workspace_id, user_id, expires_at, created_at, scope, parent_token_id, parent_fence, delegation_depth, delegation_fence, claim_dispatched_at, on_behalf_of_user_id, device_id, revoked_at, revoked_reason FROM inserted;

-- name: GetTaskTokenByHash :one
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
    WHERE token.token_hash = @token_hash
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
    SELECT * FROM lease_chain WHERE token_hash = @token_hash
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

-- name: DeleteTaskTokensByTask :exec
DELETE FROM task_token WHERE task_id = @task_id;

-- name: DeleteExpiredTaskTokens :exec
DELETE FROM task_token WHERE expires_at <= now();

-- name: RevokeTaskToken :execrows
UPDATE task_token
SET revoked_at = now(), revoked_reason = @revoked_reason
WHERE id = @id AND revoked_at IS NULL;

-- name: RevokeTaskTokensByTask :execrows
UPDATE task_token
SET revoked_at = COALESCE(revoked_at, now()),
    revoked_reason = COALESCE(revoked_reason, @revoked_reason)
WHERE task_id = @task_id AND revoked_at IS NULL;

-- name: TaskTokenExistsForClaim :one
SELECT EXISTS (
    SELECT 1
    FROM task_token
    WHERE task_id = @task_id
      AND claim_dispatched_at IS NOT DISTINCT FROM @claim_dispatched_at::timestamptz
);
