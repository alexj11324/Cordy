-- Weixin/iLink adapter queries. These are intentionally separate from the
-- generated generic channel queries; the main server runs sqlc after the
-- adapter lands, while this adapter uses the same SQL through a narrow raw
-- executor in the interim.

-- name: GetWeixinReceiveCursor :one
SELECT cursor
FROM channel_receive_state
WHERE installation_id = $1 AND channel_type = 'weixin';

-- name: UpsertWeixinReceiveCursor :exec
INSERT INTO channel_receive_state (installation_id, channel_type, cursor, updated_at)
VALUES ($1, 'weixin', $2, now())
ON CONFLICT (installation_id, channel_type) DO UPDATE
SET cursor = EXCLUDED.cursor, updated_at = now();

-- name: DeleteWeixinReceiveCursor :exec
DELETE FROM channel_receive_state
WHERE installation_id = $1 AND channel_type = 'weixin';

-- name: MergeWeixinBindingConfig :exec
UPDATE channel_chat_session_binding
SET config = channel_chat_session_binding.config || jsonb_strip_nulls($3::jsonb)
WHERE installation_id = $1
  AND channel_type = 'weixin'
  AND channel_chat_id = $2
  AND retired_at IS NULL;

-- name: LockWeixinInstallationAgentSlot :exec
SELECT pg_advisory_xact_lock(hashtext('weixin'::text), hashtext($1::text));
