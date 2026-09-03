-- name: ClaimChannelRuntimeObserver :execrows
-- Installation -> observation lock order matches revoke and capacity pause.
-- A missing, revoked or paused installation cannot acquire a new reporter.
WITH current_installation AS MATERIALIZED (
    SELECT ci.id FROM channel_installation ci
    JOIN workspace w ON w.id = ci.workspace_id
    WHERE ci.id = sqlc.arg(installation_id)
      AND ci.status = 'active' AND ci.hosted_paused_at IS NULL
      AND (ci.ws_lease_token IS NULL OR (
          ci.ws_lease_token = sqlc.arg(observer_token)::text
          AND ci.ws_lease_expires_at > now()
      ))
    FOR SHARE OF ci
)
INSERT INTO channel_installation_runtime_observation (
    installation_id, state, observed_at, error_code, error_summary, observer_token
)
SELECT id, 'starting', now(), NULL, NULL, sqlc.arg(observer_token)::text
FROM current_installation
ON CONFLICT (installation_id) DO UPDATE SET
    state = EXCLUDED.state, observed_at = EXCLUDED.observed_at,
    error_code = NULL, error_summary = NULL,
    observer_token = EXCLUDED.observer_token, updated_at = now();

-- name: ObserveChannelRuntime :execrows
-- Token fencing prevents late reports from overwriting a successor. Lock the
-- installation first so deletion and hosted pause cannot leave orphan status.
WITH current_installation AS MATERIALIZED (
    SELECT id FROM channel_installation
    WHERE id = sqlc.arg(installation_id)
      AND status = 'active' AND hosted_paused_at IS NULL
    FOR SHARE
)
UPDATE channel_installation_runtime_observation AS observation
SET state = sqlc.arg(state)::text, observed_at = now(),
    error_code = NULLIF(sqlc.arg(error_code)::text, ''),
    error_summary = NULLIF(sqlc.arg(error_summary)::text, ''), updated_at = now()
FROM current_installation
WHERE observation.installation_id = current_installation.id
  AND observation.observer_token = sqlc.arg(observer_token)::text;

-- name: ListChannelConnectionStates :many
-- Batch the authorized installation IDs. Provider summaries and credentials
-- are deliberately excluded from this public projection query.
SELECT ci.id AS installation_id, ci.status, ci.updated_at,
       ci.ws_lease_token, ci.ws_lease_expires_at, ci.hosted_paused_at,
       observation.state, observation.observed_at, observation.error_code,
       observation.observer_token
FROM channel_installation ci
LEFT JOIN channel_installation_runtime_observation observation ON observation.installation_id = ci.id
WHERE ci.workspace_id = sqlc.arg(workspace_id)
  AND ci.id = ANY(sqlc.arg(installation_ids)::uuid[]);
