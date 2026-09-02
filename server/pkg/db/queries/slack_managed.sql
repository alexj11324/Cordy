-- Managed Slack OAuth state + installation runtime observations (slice 6).
-- slack_oauth_state rows are workspace-scoped with no foreign keys
-- (repository rule): expiry is enforced at claim time, opportunistic purges
-- run on every begin_install, and workspace teardown sweeps them explicitly
-- (see DeleteWorkspaceConnections) because a deleted workspace never starts
-- another install. Only the state HASH is stored, never the raw token.
-- channel_installation_runtime_observation carries one row per installation,
-- rewritten by the supervising host; it follows channel_installation through
-- the application-owned sweeps.

-- name: CreateSlackOAuthState :one
-- Records one in-flight hosted install authorization. The caller purges
-- expired rows first (PurgeExpiredSlackOAuthStates) so the table cannot grow
-- on abandoned installs.
INSERT INTO slack_oauth_state (
    state_hash, workspace_id, installer_user_id, redirect_url, expires_at
) VALUES (
    $1, $2, $3, $4, $5
)
RETURNING *;

-- name: ConsumeSlackOAuthState :one
-- Single-use, expiry-checked claim for the OAuth callback: returns the row
-- only if it has not expired, deleting it atomically so a replayed callback
-- finds nothing. A missing row (pgx.ErrNoRows) means unknown, expired, or
-- already consumed — all three render the same "restart the install" answer.
DELETE FROM slack_oauth_state
WHERE state_hash = $1
  AND expires_at > now()
RETURNING *;

-- name: PurgeExpiredSlackOAuthStates :exec
-- Opportunistic vacuum run at the head of every begin_install. The expires_at
-- bound is caller-supplied (normally now()) so tests pin time deterministically.
DELETE FROM slack_oauth_state
WHERE expires_at <= $1;

-- name: DeleteSlackOAuthStatesByWorkspace :exec
-- Application-layer integrity (schema has no FK/cascade): drops in-flight
-- authorizations with their workspace. Wired into DeleteWorkspaceConnections
-- alongside the Linear OAuth sweep.
DELETE FROM slack_oauth_state
WHERE workspace_id = $1;

-- name: UpsertRuntimeObservation :one
-- Records the latest supervision verdict for one installation. The unique
-- index on installation_id (migration 574) is the conflict arbiter, so
-- concurrent supervisors converge on one row rather than appending history.
INSERT INTO channel_installation_runtime_observation (
    installation_id, state, observed_at, error_code, error_summary, observer_token
) VALUES (
    $1, $2, $3, $4, $5, $6
)
ON CONFLICT (installation_id) DO UPDATE SET
    state         = EXCLUDED.state,
    observed_at   = EXCLUDED.observed_at,
    error_code    = EXCLUDED.error_code,
    error_summary = EXCLUDED.error_summary,
    observer_token = EXCLUDED.observer_token,
    updated_at    = now()
RETURNING *;

-- name: GetRuntimeObservation :one
SELECT * FROM channel_installation_runtime_observation
WHERE installation_id = $1;

-- name: DeleteRuntimeObservationsByInstallation :exec
-- Application-layer integrity: drops the observation with its installation.
-- Used by the runtime-teardown sweep and the reclaim path.
DELETE FROM channel_installation_runtime_observation
WHERE installation_id = $1;
