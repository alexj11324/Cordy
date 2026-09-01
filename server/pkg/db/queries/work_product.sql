-- Work products and provenance.

-- name: GetWorkProductByID :one
SELECT * FROM work_product WHERE id = $1 AND workspace_id = $2;

-- name: ListWorkProductsByWorkspace :many
SELECT * FROM work_product WHERE workspace_id = $1 ORDER BY updated_at DESC, id DESC LIMIT $2 OFFSET $3;

-- name: GetWorkProductByExternalIdentity :one
SELECT * FROM work_product WHERE workspace_id = $1 AND provider = $2 AND external_identity = $3 LIMIT 1;

-- name: CreateWorkProduct :one
INSERT INTO work_product (workspace_id, kind, provider, external_identity, external_url, provider_record_type, provider_record_id, id)
VALUES ($1, $2, $3, $4, sqlc.narg('external_url'), sqlc.narg('provider_record_type'), sqlc.narg('provider_record_id'), COALESCE(sqlc.narg('id')::uuid, gen_random_uuid()))
RETURNING *;

-- name: GetWorkProductRelationByID :one
SELECT * FROM work_product_relation WHERE id = $1 AND workspace_id = $2;

-- name: ListWorkProductRelationsByIssue :many
SELECT * FROM work_product_relation WHERE workspace_id = $1 AND issue_id = $2 AND detached_at IS NULL ORDER BY attached_at DESC;

-- name: ListWorkProductRelationsByProduct :many
SELECT * FROM work_product_relation WHERE workspace_id = $1 AND work_product_id = $2 AND detached_at IS NULL ORDER BY attached_at DESC;

-- name: ListWorkProductRelationsByTask :many
SELECT * FROM work_product_relation WHERE workspace_id = $1 AND task_id = $2 AND detached_at IS NULL ORDER BY attached_at DESC;

-- name: CreateWorkProductRelation :one
INSERT INTO work_product_relation (workspace_id, work_product_id, issue_id, task_id, run_id, relation_key, relation_source, attached_by_type, attached_by_id, id)
VALUES ($1, $2, sqlc.narg('issue_id'), sqlc.narg('task_id'), sqlc.narg('run_id'), $3, $4, $5, $6, COALESCE(sqlc.narg('id')::uuid, gen_random_uuid()))
RETURNING *;

-- name: DetachWorkProductRelation :one
UPDATE work_product_relation SET detached_at = now(), detached_by_type = $3, detached_by_id = $4, detached_task_id = sqlc.narg('detached_task_id'), detached_run_id = sqlc.narg('detached_run_id')
WHERE id = $1 AND workspace_id = $2 AND detached_at IS NULL
RETURNING *;

-- name: GetProvenanceByTask :one
SELECT * FROM agent_task_execution_provenance WHERE workspace_id = $1 AND task_id = $2 LIMIT 1;

-- name: ListProvenanceByWorkspace :many
SELECT * FROM agent_task_execution_provenance WHERE workspace_id = $1 ORDER BY updated_at DESC, task_id DESC LIMIT $2 OFFSET $3;

-- name: UpsertProvenance :one
INSERT INTO agent_task_execution_provenance (workspace_id, task_id, run_id, repo_identity, execution_workspace, head_branch, head_sha, head_state, discovery_status, discovery_reason)
VALUES ($1, $2, sqlc.narg('run_id'), COALESCE(sqlc.narg('repo_identity'), ''), COALESCE(sqlc.narg('execution_workspace'), ''), sqlc.narg('head_branch'), sqlc.narg('head_sha'), COALESCE(sqlc.narg('head_state'), 'unknown'), COALESCE(sqlc.narg('discovery_status'), 'not_attempted'), sqlc.narg('discovery_reason'))
ON CONFLICT (workspace_id, task_id, repo_identity, execution_workspace) DO UPDATE SET
  run_id = COALESCE(EXCLUDED.run_id, agent_task_execution_provenance.run_id),
  head_branch = COALESCE(EXCLUDED.head_branch, agent_task_execution_provenance.head_branch),
  head_sha = COALESCE(EXCLUDED.head_sha, agent_task_execution_provenance.head_sha),
  head_state = EXCLUDED.head_state,
  discovery_status = EXCLUDED.discovery_status,
  updated_at = now()
RETURNING *;

-- name: GetIssueProvenanceForDiscovery :many
SELECT * FROM agent_task_execution_provenance WHERE workspace_id = $1 AND discovery_status IN ('pending','in_progress') ORDER BY updated_at LIMIT $2;
