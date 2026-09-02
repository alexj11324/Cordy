-- Work products and provenance.

-- name: GetWorkProductByID :one
SELECT id, workspace_id, kind, provider, external_identity, external_url, provider_record_type, provider_record_id, created_at, updated_at
FROM work_product WHERE id = $1 AND workspace_id = $2;

-- name: ListWorkProductsByWorkspace :many
SELECT id, workspace_id, kind, provider, external_identity, external_url, provider_record_type, provider_record_id, created_at, updated_at
FROM work_product WHERE workspace_id = $1 ORDER BY updated_at DESC, id DESC LIMIT $2 OFFSET $3;

-- name: GetWorkProductByExternalIdentity :one
SELECT id, workspace_id, kind, provider, external_identity, external_url, provider_record_type, provider_record_id, created_at, updated_at
FROM work_product WHERE workspace_id = $1 AND provider = $2 AND external_identity = $3 LIMIT 1;

-- name: CreateWorkProduct :one
INSERT INTO work_product (workspace_id, kind, provider, external_identity, external_url, provider_record_type, provider_record_id, id)
VALUES ($1, $2, $3, $4, sqlc.narg('external_url'), sqlc.narg('provider_record_type'), sqlc.narg('provider_record_id'), COALESCE(sqlc.narg('id')::uuid, gen_random_uuid()))
ON CONFLICT (workspace_id, provider, external_identity) DO UPDATE SET
  external_url = COALESCE(EXCLUDED.external_url, work_product.external_url),
  provider_record_type = COALESCE(EXCLUDED.provider_record_type, work_product.provider_record_type),
  provider_record_id = COALESCE(EXCLUDED.provider_record_id, work_product.provider_record_id),
  updated_at = now()
RETURNING id, workspace_id, kind, provider, external_identity, external_url, provider_record_type, provider_record_id, created_at, updated_at;

-- name: GetWorkProductRelationByID :one
SELECT id, workspace_id, work_product_id, issue_id, task_id, run_id, relation_key, relation_source, attached_by_type, attached_by_id, attached_at, close_intent, detached_at, detached_by_type, detached_by_id, detached_task_id, detached_run_id
FROM work_product_relation WHERE id = $1 AND workspace_id = $2;

-- name: ListWorkProductRelationsByIssue :many
SELECT id, workspace_id, work_product_id, issue_id, task_id, run_id, relation_key, relation_source, attached_by_type, attached_by_id, attached_at, close_intent, detached_at, detached_by_type, detached_by_id, detached_task_id, detached_run_id
FROM work_product_relation WHERE workspace_id = $1 AND issue_id = $2 AND detached_at IS NULL ORDER BY attached_at DESC, id DESC LIMIT $3 OFFSET $4;

-- name: ListWorkProductRelationsByProduct :many
SELECT id, workspace_id, work_product_id, issue_id, task_id, run_id, relation_key, relation_source, attached_by_type, attached_by_id, attached_at, close_intent, detached_at, detached_by_type, detached_by_id, detached_task_id, detached_run_id
FROM work_product_relation WHERE workspace_id = $1 AND work_product_id = $2 AND detached_at IS NULL ORDER BY attached_at DESC, id DESC;

-- name: ListWorkProductRelationsByTask :many
SELECT id, workspace_id, work_product_id, issue_id, task_id, run_id, relation_key, relation_source, attached_by_type, attached_by_id, attached_at, close_intent, detached_at, detached_by_type, detached_by_id, detached_task_id, detached_run_id
FROM work_product_relation WHERE workspace_id = $1 AND task_id = $2 AND detached_at IS NULL ORDER BY attached_at DESC, id DESC;

-- name: CreateWorkProductRelation :one
WITH product_scope AS (
    SELECT id FROM work_product WHERE id = $2 AND workspace_id = $1
), issue_scope AS (
    SELECT id FROM issue WHERE id = sqlc.narg('issue_id')::uuid AND workspace_id = $1
    UNION ALL
    SELECT NULL::uuid WHERE sqlc.narg('issue_id')::uuid IS NULL
)
INSERT INTO work_product_relation (workspace_id, work_product_id, issue_id, task_id, run_id, relation_key, relation_source, attached_by_type, attached_by_id, id)
SELECT $1, $2, sqlc.narg('issue_id'), sqlc.narg('task_id'), sqlc.narg('run_id'), $3, $4, $5, $6, COALESCE(sqlc.narg('id')::uuid, gen_random_uuid())
FROM product_scope, issue_scope
WHERE (sqlc.narg('task_id')::uuid IS NULL OR EXISTS (
    SELECT 1 FROM agent_task_queue task
    JOIN agent ON agent.id = task.agent_id
    WHERE task.id = sqlc.narg('task_id')::uuid
      AND (sqlc.narg('issue_id')::uuid IS NULL OR task.issue_id = sqlc.narg('issue_id')::uuid)
      AND agent.workspace_id = $1
      AND ($5 <> 'agent' OR task.agent_id = $6)
))
  AND (sqlc.narg('run_id')::uuid IS NULL OR EXISTS (
    SELECT 1 FROM agent_task_queue task
    JOIN agent ON agent.id = task.agent_id
    WHERE task.id = sqlc.narg('task_id')::uuid
      AND task.automation_run_id = sqlc.narg('run_id')::uuid
      AND agent.workspace_id = $1
))
ON CONFLICT (work_product_id, relation_key) WHERE detached_at IS NULL DO UPDATE SET
  relation_source = CASE
    WHEN work_product_relation.relation_source = 'execution_branch_discovery'
     AND EXCLUDED.relation_source IN ('task_explicit', 'manual_explicit')
    THEN EXCLUDED.relation_source
    ELSE work_product_relation.relation_source
  END,
  attached_by_type = CASE
    WHEN work_product_relation.relation_source = 'execution_branch_discovery'
     AND EXCLUDED.relation_source IN ('task_explicit', 'manual_explicit')
    THEN EXCLUDED.attached_by_type
    ELSE work_product_relation.attached_by_type
  END,
  attached_by_id = CASE
    WHEN work_product_relation.relation_source = 'execution_branch_discovery'
     AND EXCLUDED.relation_source IN ('task_explicit', 'manual_explicit')
    THEN EXCLUDED.attached_by_id
    ELSE work_product_relation.attached_by_id
  END,
  close_intent = work_product_relation.close_intent OR EXCLUDED.close_intent
RETURNING id, workspace_id, work_product_id, issue_id, task_id, run_id, relation_key, relation_source, attached_by_type, attached_by_id, attached_at, close_intent, detached_at, detached_by_type, detached_by_id, detached_task_id, detached_run_id;

-- name: DetachWorkProductRelation :one
UPDATE work_product_relation SET detached_at = now(), detached_by_type = $3, detached_by_id = $4, detached_task_id = sqlc.narg('detached_task_id'), detached_run_id = sqlc.narg('detached_run_id')
WHERE id = $1 AND workspace_id = $2 AND detached_at IS NULL
RETURNING id, workspace_id, work_product_id, issue_id, task_id, run_id, relation_key, relation_source, attached_by_type, attached_by_id, attached_at, close_intent, detached_at, detached_by_type, detached_by_id, detached_task_id, detached_run_id;

-- name: GetProvenanceByTask :one
SELECT task_id, workspace_id, run_id, repo_identity, execution_workspace, head_branch, head_sha, head_state, started_at, finished_at, discovery_status, discovery_lease_id, discovery_match_count, discovery_reason, discovery_work_product_id, discovery_at, updated_at
FROM agent_task_execution_provenance WHERE workspace_id = $1 AND task_id = $2 ORDER BY updated_at DESC, repo_identity ASC, execution_workspace ASC LIMIT 1;

-- name: ListProvenanceByWorkspace :many
SELECT task_id, workspace_id, run_id, repo_identity, execution_workspace, head_branch, head_sha, head_state, started_at, finished_at, discovery_status, discovery_lease_id, discovery_match_count, discovery_reason, discovery_work_product_id, discovery_at, updated_at
FROM agent_task_execution_provenance WHERE workspace_id = $1 ORDER BY updated_at DESC, task_id DESC, repo_identity ASC, execution_workspace ASC LIMIT $2 OFFSET $3;

-- name: UpsertProvenance :one
INSERT INTO agent_task_execution_provenance (workspace_id, task_id, run_id, repo_identity, execution_workspace, head_branch, head_sha, head_state, discovery_status, discovery_reason)
VALUES ($1, $2, sqlc.narg('run_id'), COALESCE(sqlc.narg('repo_identity'), ''), COALESCE(sqlc.narg('execution_workspace'), ''), sqlc.narg('head_branch'), sqlc.narg('head_sha'), COALESCE(sqlc.narg('head_state'), 'unknown'), COALESCE(sqlc.narg('discovery_status'), 'not_attempted'), sqlc.narg('discovery_reason'))
ON CONFLICT (workspace_id, task_id, repo_identity, execution_workspace) DO UPDATE SET
  run_id = COALESCE(EXCLUDED.run_id, agent_task_execution_provenance.run_id),
  head_branch = CASE WHEN EXCLUDED.head_state <> 'unknown' THEN EXCLUDED.head_branch ELSE COALESCE(agent_task_execution_provenance.head_branch, EXCLUDED.head_branch) END,
  head_sha = CASE WHEN EXCLUDED.head_state <> 'unknown' THEN EXCLUDED.head_sha ELSE COALESCE(agent_task_execution_provenance.head_sha, EXCLUDED.head_sha) END,
  head_state = CASE WHEN EXCLUDED.head_state <> 'unknown' THEN EXCLUDED.head_state ELSE agent_task_execution_provenance.head_state END,
  started_at = COALESCE(agent_task_execution_provenance.started_at, now()),
  updated_at = now()
RETURNING task_id, workspace_id, run_id, repo_identity, execution_workspace, head_branch, head_sha, head_state, started_at, finished_at, discovery_status, discovery_lease_id, discovery_match_count, discovery_reason, discovery_work_product_id, discovery_at, updated_at;

-- name: GetIssueProvenanceForDiscovery :many
SELECT task_id, workspace_id, run_id, repo_identity, execution_workspace, head_branch, head_sha, head_state, started_at, finished_at, discovery_status, discovery_lease_id, discovery_match_count, discovery_reason, discovery_work_product_id, discovery_at, updated_at
FROM agent_task_execution_provenance WHERE workspace_id = $1 AND discovery_status IN ('pending','in_progress') ORDER BY updated_at LIMIT $2;

-- name: ListExecutionProvenanceByTask :many
SELECT task_id, workspace_id, run_id, repo_identity, execution_workspace, head_branch, head_sha, head_state, started_at, finished_at, discovery_status, discovery_lease_id, discovery_match_count, discovery_reason, discovery_work_product_id, discovery_at, updated_at
FROM agent_task_execution_provenance
WHERE workspace_id = $1 AND task_id = $2
ORDER BY updated_at DESC, repo_identity ASC, execution_workspace ASC;

-- name: MarkTaskDiscoveryPending :exec
UPDATE agent_task_execution_provenance
SET discovery_status = 'pending',
    discovery_match_count = 0,
    discovery_reason = NULL,
    discovery_work_product_id = NULL,
    discovery_at = NULL,
    finished_at = COALESCE(finished_at, now()),
    updated_at = now()
WHERE workspace_id = $1 AND task_id = $2 AND discovery_status = 'not_attempted';

-- name: ListPendingExecutionDiscoveryTasks :many
SELECT DISTINCT workspace_id, task_id
FROM agent_task_execution_provenance
WHERE discovery_status = 'pending'
   OR (discovery_status = 'in_progress' AND updated_at < now() - interval '5 minutes')
ORDER BY workspace_id, task_id
LIMIT $1;
