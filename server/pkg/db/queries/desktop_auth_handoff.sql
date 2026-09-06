-- name: CreateDesktopAuthHandoff :exec
INSERT INTO desktop_auth_handoff (
    state,
    code_challenge,
    callback_protocol,
    expires_at
)
VALUES ($1, $2, $3, now() + interval '10 minutes');

-- name: RegisterDesktopGoogleAttempt :one
INSERT INTO desktop_auth_handoff (
    state,
    code_challenge,
    callback_protocol,
    expires_at
)
VALUES ($1, $2, 'patchbay', now() + interval '5 minutes')
ON CONFLICT (state) DO UPDATE
SET state = EXCLUDED.state
WHERE desktop_auth_handoff.code_challenge = EXCLUDED.code_challenge
  AND desktop_auth_handoff.user_id IS NULL
  AND desktop_auth_handoff.code_hash IS NULL
  AND desktop_auth_handoff.completed_at IS NULL
  AND desktop_auth_handoff.expires_at > now()
RETURNING created_at;

-- name: GetDesktopGoogleAttempt :one
SELECT created_at
FROM desktop_auth_handoff
WHERE state = $1
  AND code_challenge = $2
  AND user_id IS NULL
  AND code_hash IS NULL
  AND completed_at IS NULL
  AND expires_at > now();

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

-- name: RedeemDesktopLocalIdentity :one
DELETE FROM desktop_auth_handoff
WHERE code_hash = $1
  AND state = $2
  AND code_challenge = $3
  AND user_id IS NOT NULL
  AND completed_at IS NOT NULL
  AND completed_at > now() - interval '1 minute'
  AND expires_at > now()
RETURNING user_id;

-- name: ConsumeDesktopLocalAuthAttempt :execrows
DELETE FROM desktop_auth_handoff
WHERE state = $1
  AND code_challenge = $2
  AND user_id IS NULL
  AND code_hash IS NULL
  AND completed_at IS NULL
  AND expires_at > now();
