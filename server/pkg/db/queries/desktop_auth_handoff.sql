-- name: CreateDesktopAuthHandoff :exec
INSERT INTO desktop_auth_handoff (
    state,
    code_challenge,
    callback_protocol,
    expires_at
)
VALUES ($1, $2, $3, now() + interval '10 minutes');

-- name: CompleteDesktopAuthHandoff :one
UPDATE desktop_auth_handoff
SET user_id = $2,
    code_hash = $3,
    completed_at = now()
WHERE state = $1
  AND code_challenge = $4
  AND user_id IS NULL
  AND code_hash IS NULL
  AND completed_at IS NULL
  AND expires_at > now()
RETURNING callback_protocol;

-- name: RedeemDesktopAuthHandoff :one
DELETE FROM desktop_auth_handoff
WHERE code_hash = $1
  AND code_challenge = $2
  AND user_id IS NOT NULL
  AND completed_at IS NOT NULL
  AND expires_at > now()
RETURNING user_id;
