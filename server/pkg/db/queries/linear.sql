-- Linear is deliberately kept in one query source so OAuth, installation,
-- queue ownership, links, conflicts, and cleanup share the same workspace
-- predicates. Relationships are application-owned; this file adds no FKs.

-- name: GetLinearConnectionForWorkspace :one
SELECT id, workspace_id, organization_id, organization_name, actor_id,
       access_token_encrypted, refresh_token_encrypted, token_expires_at,
       scopes, webhook_id, status, last_success_at, last_error, created_by_id,
       created_at, updated_at
FROM linear_connection
WHERE workspace_id = $1 AND status <> 'revoked'
ORDER BY created_at DESC
LIMIT 1;

-- name: GetLinearConnectionForWorkspaceForUpdate :one
SELECT id, workspace_id, organization_id, organization_name, actor_id,
       access_token_encrypted, refresh_token_encrypted, token_expires_at,
       scopes, webhook_id, status, last_success_at, last_error, created_by_id,
       created_at, updated_at
FROM linear_connection
WHERE workspace_id = $1
ORDER BY created_at DESC
LIMIT 1
FOR UPDATE;

-- name: GetLinearConnectionByID :one
SELECT id, workspace_id, organization_id, organization_name, actor_id,
       access_token_encrypted, refresh_token_encrypted, token_expires_at,
       scopes, webhook_id, status, last_success_at, last_error, created_by_id,
       created_at, updated_at
FROM linear_connection
WHERE id = $1 AND workspace_id = $2;

-- name: GetLinearConnectionByIDUnscoped :one
SELECT id, workspace_id, organization_id, organization_name, actor_id,
       access_token_encrypted, refresh_token_encrypted, token_expires_at,
       scopes, webhook_id, status, last_success_at, last_error, created_by_id,
       created_at, updated_at
FROM linear_connection
WHERE id = $1;

-- name: FindLinearConnectionsForWebhook :many
SELECT id, workspace_id, organization_id, organization_name, actor_id,
       access_token_encrypted, refresh_token_encrypted, token_expires_at,
       scopes, webhook_id, status, last_success_at, last_error, created_by_id,
       created_at, updated_at
FROM linear_connection
WHERE organization_id = $1
  AND status = 'active'
  AND (webhook_id = $2 OR webhook_id IS NULL)
ORDER BY (webhook_id = $2) DESC, created_at DESC;

-- name: InsertLinearOauthState :exec
INSERT INTO linear_oauth_state
    (id, state_hash, workspace_id, user_id, code_verifier_encrypted,
     redirect_uri, expires_at)
VALUES ($1, $2, $3, $4, $5, $6, $7);

-- name: ConsumeLinearOauthState :one
UPDATE linear_oauth_state
SET consumed_at = now()
WHERE state_hash = $1 AND consumed_at IS NULL AND expires_at > now()
RETURNING workspace_id, user_id, code_verifier_encrypted, redirect_uri;

-- name: CleanupLinearOauthStates :exec
DELETE FROM linear_oauth_state
WHERE expires_at < now() OR consumed_at < now() - interval '1 day';

-- name: UpsertLinearConnection :one
INSERT INTO linear_connection
    (id, workspace_id, organization_id, organization_name, actor_id,
     access_token_encrypted, refresh_token_encrypted, token_expires_at, scopes,
     status, created_by_id)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'active', $10)
ON CONFLICT (workspace_id) DO UPDATE
SET organization_id = EXCLUDED.organization_id,
    organization_name = EXCLUDED.organization_name,
    actor_id = EXCLUDED.actor_id,
    access_token_encrypted = EXCLUDED.access_token_encrypted,
    refresh_token_encrypted = EXCLUDED.refresh_token_encrypted,
    token_expires_at = EXCLUDED.token_expires_at,
    scopes = EXCLUDED.scopes,
    status = 'active',
    last_error = NULL,
    updated_at = now()
RETURNING id, workspace_id, organization_id, organization_name, actor_id,
          access_token_encrypted, refresh_token_encrypted, token_expires_at,
          scopes, webhook_id, status, last_success_at, last_error, created_by_id,
          created_at, updated_at;

-- name: UpdateLinearTokens :exec
UPDATE linear_connection
SET access_token_encrypted = $2, refresh_token_encrypted = $3,
    token_expires_at = $4,
    scopes = CASE WHEN $5 = '' THEN scopes ELSE to_jsonb(regexp_split_to_array($5, '[, ]+')) END,
    last_error = NULL, updated_at = now()
WHERE id = $1 AND status = 'active';

-- name: MarkLinearReauthorizationRequired :exec
UPDATE linear_connection
SET status = 'reauthorization_required', last_error = $2, updated_at = now()
WHERE id = $1;

-- name: MarkLinearRevoked :exec
UPDATE linear_connection
SET status = 'revoked', last_error = $2, updated_at = now()
WHERE id = $1 AND workspace_id = $3;

-- name: BindLinearWebhook :exec
UPDATE linear_connection
SET webhook_id = $2, updated_at = now()
WHERE id = $1 AND status = 'active' AND (webhook_id IS NULL OR webhook_id = $2);

-- name: MarkLinearWebhookAccepted :exec
UPDATE linear_connection
SET last_success_at = now(), last_error = NULL, updated_at = now()
WHERE id = $1 AND status = 'active';

-- name: ListLinearProjectBindings :many
SELECT id, workspace_id, connection_id, patchbay_project_id, linear_project_id,
       linear_team_id, status, sync_mode, initial_source_of_truth,
       status_mapping, agent_label_mapping, activated_at, paused_at,
       created_by_id, created_at, updated_at
FROM linear_project_binding
WHERE workspace_id = $1 AND status <> 'tombstone'
ORDER BY created_at;

-- name: GetLinearProjectBinding :one
SELECT id, workspace_id, connection_id, patchbay_project_id, linear_project_id,
       linear_team_id, status, sync_mode, initial_source_of_truth,
       status_mapping, agent_label_mapping, activated_at, paused_at,
       created_by_id, created_at, updated_at
FROM linear_project_binding
WHERE id = $1 AND workspace_id = $2;

-- name: GetLinearProjectBindingForUpdate :one
SELECT id, workspace_id, connection_id, patchbay_project_id, linear_project_id,
       linear_team_id, status, sync_mode, initial_source_of_truth,
       status_mapping, agent_label_mapping, activated_at, paused_at,
       created_by_id, created_at, updated_at
FROM linear_project_binding
WHERE id = $1 AND workspace_id = $2
FOR UPDATE;

-- name: CreateLinearProjectBinding :one
INSERT INTO linear_project_binding
    (id, workspace_id, connection_id, patchbay_project_id, linear_project_id,
     linear_team_id, status, sync_mode, initial_source_of_truth, status_mapping,
     agent_label_mapping, activated_at, paused_at, created_by_id)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
        CASE WHEN $7 = 'active' THEN now() END,
        CASE WHEN $7 = 'paused' THEN now() END, $12)
RETURNING id, workspace_id, connection_id, patchbay_project_id, linear_project_id,
          linear_team_id, status, sync_mode, initial_source_of_truth,
          status_mapping, agent_label_mapping, activated_at, paused_at,
          created_by_id, created_at, updated_at;

-- name: UpdateLinearProjectBinding :one
UPDATE linear_project_binding
SET status = $3, sync_mode = $4, initial_source_of_truth = $5,
    linear_team_id = $6, status_mapping = $7, agent_label_mapping = $8,
    activated_at = CASE WHEN $3 = 'active' THEN COALESCE(activated_at, now()) ELSE activated_at END,
    paused_at = CASE WHEN $3 = 'paused' THEN now() ELSE paused_at END,
    updated_at = now()
WHERE id = $1 AND workspace_id = $2
RETURNING id, workspace_id, connection_id, patchbay_project_id, linear_project_id,
          linear_team_id, status, sync_mode, initial_source_of_truth,
          status_mapping, agent_label_mapping, activated_at, paused_at,
          created_by_id, created_at, updated_at;

-- name: TombstoneLinearProjectBinding :exec
WITH tombstoned AS (
    UPDATE linear_project_binding AS binding
    SET status = 'tombstone', paused_at = COALESCE(binding.paused_at, now()), updated_at = now()
    WHERE binding.id = $1 AND binding.workspace_id = $2
    RETURNING binding.id
), deleted_conflicts AS (
    DELETE FROM linear_sync_conflict
    WHERE binding_id IN (SELECT id FROM tombstoned) AND workspace_id = $2
), deleted_outbox AS (
    DELETE FROM linear_sync_outbox
    WHERE binding_id IN (SELECT id FROM tombstoned) AND workspace_id = $2
)
UPDATE linear_issue_link
SET sync_status = 'deleted', updated_at = now()
WHERE binding_id IN (SELECT id FROM tombstoned) AND workspace_id = $2;

-- name: ListLinearMemberBindings :many
SELECT id, workspace_id, connection_id, patchbay_user_id, linear_user_id,
       created_at, updated_at
FROM linear_member_binding
WHERE workspace_id = $1
ORDER BY created_at;

-- name: GetLinearMemberBinding :one
SELECT id, workspace_id, connection_id, patchbay_user_id, linear_user_id,
       created_at, updated_at
FROM linear_member_binding
WHERE workspace_id = $1 AND patchbay_user_id = $2;

-- name: GetLinearMemberBindingByLinearUser :one
SELECT id, workspace_id, connection_id, patchbay_user_id, linear_user_id,
       created_at, updated_at
FROM linear_member_binding
WHERE workspace_id = $1 AND linear_user_id = $2;

-- name: UpsertLinearMemberBinding :one
INSERT INTO linear_member_binding
    (id, workspace_id, connection_id, patchbay_user_id, linear_user_id)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (workspace_id, patchbay_user_id) DO UPDATE
SET connection_id = EXCLUDED.connection_id,
    linear_user_id = EXCLUDED.linear_user_id, updated_at = now()
RETURNING id, workspace_id, connection_id, patchbay_user_id, linear_user_id,
          created_at, updated_at;

-- name: DeleteLinearMemberBinding :exec
DELETE FROM linear_member_binding
WHERE workspace_id = $1 AND patchbay_user_id = $2;

-- name: InsertLinearSyncInbox :execrows
INSERT INTO linear_sync_inbox (id, connection_id, delivery_id, event_type, payload)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (connection_id, delivery_id) DO NOTHING;

-- name: ClaimLinearSyncInbox :one
WITH candidate AS (
    SELECT id FROM linear_sync_inbox
    WHERE processed_at IS NULL AND dead_lettered_at IS NULL
      AND available_at <= now() AND (locked_until IS NULL OR locked_until < now())
    ORDER BY received_at, id
    FOR UPDATE SKIP LOCKED LIMIT 1
)
UPDATE linear_sync_inbox i
SET locked_by = $1, locked_until = now() + make_interval(secs => $2), attempts = attempts + 1
FROM candidate
WHERE i.id = candidate.id
RETURNING i.id, i.connection_id, i.delivery_id, i.event_type, i.payload,
          i.attempts, i.max_attempts;

-- name: RenewLinearSyncInbox :execrows
UPDATE linear_sync_inbox
SET locked_until = now() + make_interval(secs => $2)
WHERE id = $1 AND locked_by = $3 AND processed_at IS NULL AND dead_lettered_at IS NULL;

-- name: CompleteLinearSyncInbox :execrows
UPDATE linear_sync_inbox
SET processed_at = now(), locked_by = NULL, locked_until = NULL, last_error = NULL
WHERE id = $1 AND locked_by = $2;

-- name: RetryLinearSyncInbox :execrows
UPDATE linear_sync_inbox
SET available_at = now() + make_interval(secs => $2), locked_by = NULL,
    locked_until = NULL, last_error = $3
WHERE id = $1 AND locked_by = $4;

-- name: DeadLetterLinearSyncInbox :execrows
UPDATE linear_sync_inbox
SET dead_lettered_at = now(), locked_by = NULL, locked_until = NULL, last_error = $2
WHERE id = $1 AND locked_by = $3;

-- name: ClaimLinearSyncOutbox :one
WITH candidate AS (
    SELECT o.id FROM linear_sync_outbox o
    WHERE o.processed_at IS NULL AND o.dead_lettered_at IS NULL
      AND o.available_at <= now() AND (o.locked_until IS NULL OR o.locked_until < now())
      AND NOT EXISTS (
          SELECT 1 FROM linear_sync_outbox older
          WHERE older.binding_id = o.binding_id AND older.issue_id = o.issue_id
            AND older.processed_at IS NULL AND older.dead_lettered_at IS NULL
            AND (older.created_at, older.id) < (o.created_at, o.id)
      )
    ORDER BY o.created_at, o.id
    FOR UPDATE SKIP LOCKED LIMIT 1
)
UPDATE linear_sync_outbox o
SET locked_by = $1, locked_until = now() + make_interval(secs => $2),
    attempts = attempts + 1, updated_at = now()
FROM candidate
WHERE o.id = candidate.id
RETURNING o.id, o.workspace_id, o.binding_id, o.issue_id, o.event_type,
          o.payload, o.attempts, o.max_attempts;

-- name: RenewLinearSyncOutbox :execrows
UPDATE linear_sync_outbox
SET locked_until = now() + make_interval(secs => $2), updated_at = now()
WHERE id = $1 AND locked_by = $3 AND processed_at IS NULL AND dead_lettered_at IS NULL;

-- name: CompleteLinearSyncOutbox :execrows
UPDATE linear_sync_outbox
SET processed_at = now(), locked_by = NULL, locked_until = NULL,
    last_error = NULL, updated_at = now()
WHERE id = $1 AND locked_by = $2;

-- name: RetryLinearSyncOutbox :execrows
UPDATE linear_sync_outbox
SET available_at = now() + make_interval(secs => $2), locked_by = NULL,
    locked_until = NULL, last_error = $3, updated_at = now()
WHERE id = $1 AND locked_by = $4;

-- name: DeadLetterLinearSyncOutbox :execrows
UPDATE linear_sync_outbox
SET dead_lettered_at = now(), locked_by = NULL, locked_until = NULL,
    last_error = $2, updated_at = now()
WHERE id = $1 AND locked_by = $3;

-- name: GetLinearIssueLinkByRemote :one
SELECT id, workspace_id, binding_id, patchbay_issue_id, linear_issue_id,
       linear_identifier, last_common_snapshot, remote_updated_at,
       last_remote_event_at_ms, last_remote_event_id, sync_status, created_at,
       updated_at
FROM linear_issue_link
WHERE binding_id = $1 AND linear_issue_id = $2
FOR UPDATE;

-- name: GetLinearIssueLinkByLocal :one
SELECT id, workspace_id, binding_id, patchbay_issue_id, linear_issue_id,
       linear_identifier, last_common_snapshot, remote_updated_at,
       last_remote_event_at_ms, last_remote_event_id, sync_status, created_at,
       updated_at
FROM linear_issue_link
WHERE workspace_id = $1 AND binding_id = $2 AND patchbay_issue_id = $3
  AND sync_status <> 'deleted'
FOR UPDATE;

-- name: CreateLinearIssueLink :one
INSERT INTO linear_issue_link
    (id, workspace_id, binding_id, patchbay_issue_id, linear_issue_id,
     linear_identifier, last_common_snapshot, remote_updated_at,
     last_remote_event_at_ms, last_remote_event_id, sync_status)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'active')
ON CONFLICT (workspace_id, patchbay_issue_id) WHERE sync_status <> 'deleted'
DO UPDATE SET linear_issue_id = EXCLUDED.linear_issue_id,
              linear_identifier = EXCLUDED.linear_identifier,
              last_common_snapshot = EXCLUDED.last_common_snapshot,
              remote_updated_at = EXCLUDED.remote_updated_at,
              last_remote_event_at_ms = EXCLUDED.last_remote_event_at_ms,
              last_remote_event_id = EXCLUDED.last_remote_event_id,
              sync_status = 'active', updated_at = now()
RETURNING id, workspace_id, binding_id, patchbay_issue_id, linear_issue_id,
          linear_identifier, last_common_snapshot, remote_updated_at,
          last_remote_event_at_ms, last_remote_event_id, sync_status, created_at,
          updated_at;

-- name: UpdateLinearIssueLink :exec
UPDATE linear_issue_link
SET linear_identifier = $2, last_common_snapshot = $3,
    remote_updated_at = $4, last_remote_event_at_ms = $5,
    last_remote_event_id = $6, sync_status = $7, updated_at = now()
WHERE id = $1 AND workspace_id = $8;

-- name: SetLinearIssueLinkDeleted :exec
UPDATE linear_issue_link
SET sync_status = 'deleted', updated_at = now()
WHERE id = $1 AND workspace_id = $2;

-- name: CreateLinearSyncConflict :exec
INSERT INTO linear_sync_conflict
    (id, workspace_id, binding_id, link_id, patchbay_issue_id, linear_issue_id,
     field, base_value, local_value, remote_value, source_event_id,
     source_event_at_ms)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
ON CONFLICT (link_id, field) WHERE status = 'open' DO UPDATE
SET local_value = EXCLUDED.local_value, remote_value = EXCLUDED.remote_value,
    source_event_id = EXCLUDED.source_event_id,
    source_event_at_ms = EXCLUDED.source_event_at_ms, updated_at = now();

-- name: ListLinearSyncConflicts :many
SELECT c.id, c.workspace_id, c.binding_id, c.link_id, c.patchbay_issue_id,
       c.linear_issue_id, l.linear_identifier, c.field, c.base_value,
       c.local_value, c.remote_value, c.source_event_id, c.source_event_at_ms,
       c.status, c.resolution, c.resolved_value, c.resolved_by_id,
       c.created_at, c.updated_at
FROM linear_sync_conflict c
LEFT JOIN linear_issue_link l ON l.id = c.link_id
WHERE c.workspace_id = $1 AND c.status = $2
ORDER BY c.created_at DESC;

-- name: GetLinearSyncConflictForUpdate :one
SELECT id, workspace_id, binding_id, link_id, patchbay_issue_id, linear_issue_id,
       field, base_value, local_value, remote_value, source_event_id,
       source_event_at_ms, status, resolution, resolved_value, resolved_by_id,
       created_at, updated_at
FROM linear_sync_conflict
WHERE id = $1 AND workspace_id = $2
FOR UPDATE;

-- name: ResolveLinearSyncConflict :execrows
UPDATE linear_sync_conflict
SET status = 'resolved', resolution = $3, resolved_value = $4,
    resolved_by_id = $5, updated_at = now()
WHERE id = $1 AND workspace_id = $2 AND status = 'open';

-- name: CountLinearSyncConflicts :one
SELECT count(*)::bigint
FROM linear_sync_conflict
WHERE workspace_id = $1 AND status = $2;

-- name: LinearProjectBelongsToWorkspace :one
SELECT EXISTS (
    SELECT 1 FROM project
    WHERE id = $1 AND workspace_id = $2
);

-- name: CountLinearProjectIssues :one
SELECT count(*)::bigint
FROM issue
WHERE workspace_id = $1 AND project_id = $2;
