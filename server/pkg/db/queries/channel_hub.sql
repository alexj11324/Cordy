-- name: GetChannelHubRoute :one
-- Exact conversation wins; Slack channel-only commands can reuse the newest
-- active thread's selection. Never inherit a retired /new generation.
SELECT config FROM channel_chat_session_binding
WHERE installation_id = sqlc.arg('installation_id')
  AND retired_at IS NULL
  AND (
    channel_chat_id = sqlc.arg('binding_key')::text
    OR channel_chat_id = sqlc.arg('channel_id')::text
    OR left(channel_chat_id, length(sqlc.arg('channel_id')::text) + 1) = sqlc.arg('channel_id')::text || ':'
  )
ORDER BY (channel_chat_id = sqlc.arg('binding_key')::text) DESC,
         (channel_chat_id = sqlc.arg('channel_id')::text) DESC, created_at DESC, id DESC
LIMIT 1;

-- name: LockChannelInstallationForHub :one
-- Workspace deletion takes the workspace lock first. Hub writes then lock
-- installation, Chat and binding, and cannot revive a paused/revoked install.
SELECT id FROM channel_installation
WHERE id = sqlc.arg('installation_id')
  AND workspace_id = sqlc.arg('workspace_id')
  AND status = 'active' AND hosted_paused_at IS NULL
  AND (agent_id IS NULL OR agent_id = '00000000-0000-0000-0000-000000000000'::uuid)
FOR SHARE;

-- name: SwitchHubChatSessionAgent :execrows
-- Run under the Chat's runtime-bind lock, in the same transaction as the Hub
-- binding update. Invocation rights, not management visibility, gate routing.
UPDATE chat_session AS cs
SET agent_id = a.id, runtime_id = a.runtime_id,
    session_id = CASE WHEN cs.agent_id IS DISTINCT FROM a.id THEN NULL ELSE cs.session_id END,
    work_dir = CASE WHEN cs.agent_id IS DISTINCT FROM a.id THEN NULL ELSE cs.work_dir END,
    updated_at = now()
FROM agent AS a
WHERE cs.id = sqlc.arg('chat_session_id')
  AND cs.workspace_id = sqlc.arg('workspace_id')
  AND cs.status = 'active'
  AND a.id = sqlc.arg('agent_id') AND a.workspace_id = cs.workspace_id
  AND a.archived_at IS NULL AND a.kind = 'user'
  AND EXISTS (
    SELECT 1 FROM member m WHERE m.workspace_id = cs.workspace_id AND m.user_id = sqlc.arg('user_id')
  )
  AND (
    a.owner_id = sqlc.arg('user_id')
    OR (a.permission_mode = 'public_to' AND EXISTS (
      SELECT 1 FROM agent_invocation_target t WHERE t.agent_id = a.id
        AND (t.target_type = 'workspace' OR (t.target_type = 'member' AND t.target_id = sqlc.arg('user_id')))
    ))
  );

-- name: MergeChannelHubRoute :execrows
-- A concurrently retired or replaced binding must fail the whole transaction,
-- including the Chat Agent change. Preserve provider routing configuration.
UPDATE channel_chat_session_binding
SET config = config || jsonb_build_object('hub_agent_id', sqlc.arg('agent_id')::uuid)
WHERE installation_id = sqlc.arg('installation_id')
  AND channel_chat_id = sqlc.arg('binding_key')::text
  AND chat_session_id = sqlc.arg('chat_session_id')
  AND retired_at IS NULL;
