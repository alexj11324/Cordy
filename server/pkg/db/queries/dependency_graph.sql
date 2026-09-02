-- Dependency graph execution plans.
--
-- These queries intentionally carry workspace_id on every graph lookup. The
-- graph tables do not have foreign keys, so the workspace predicate is part
-- of the application-level tenant boundary rather than an optional filter.

-- name: LockDependencyGraphParentIssue :one
SELECT id, workspace_id, project_id
FROM issue
WHERE id = $1 AND workspace_id = $2
FOR UPDATE;

-- name: GetDependencyGraphPlanByID :one
SELECT id, workspace_id, parent_issue_id, idempotency_key, request_hash,
       goal, status, created_by_type, created_by_id, created_at, updated_at,
       attention_required, attention_reason
FROM dependency_graph_plan
WHERE id = $1 AND workspace_id = $2;

-- name: GetDependencyGraphPlanForUpdate :one
SELECT id, workspace_id, parent_issue_id, idempotency_key, request_hash,
       goal, status, created_by_type, created_by_id, created_at, updated_at,
       attention_required, attention_reason
FROM dependency_graph_plan
WHERE id = $1 AND workspace_id = $2
FOR UPDATE;

-- name: GetDependencyGraphPlanByIdempotency :one
SELECT id, workspace_id, parent_issue_id, idempotency_key, request_hash,
       goal, status, created_by_type, created_by_id, created_at, updated_at,
       attention_required, attention_reason
FROM dependency_graph_plan
WHERE workspace_id = $1 AND idempotency_key = $2
LIMIT 1;

-- name: GetActiveDependencyGraphPlanForParent :one
SELECT id, workspace_id, parent_issue_id, idempotency_key, request_hash,
       goal, status, created_by_type, created_by_id, created_at, updated_at,
       attention_required, attention_reason
FROM dependency_graph_plan
WHERE workspace_id = $1 AND parent_issue_id = $2 AND status = 'active'
LIMIT 1;

-- name: ListDependencyGraphPlans :many
SELECT p.id, p.workspace_id, p.parent_issue_id, p.idempotency_key,
       p.request_hash, p.goal, p.status, p.created_by_type, p.created_by_id,
       p.created_at, p.updated_at, p.attention_required, p.attention_reason
FROM dependency_graph_plan p
JOIN issue parent
  ON parent.id = p.parent_issue_id
 AND parent.workspace_id = p.workspace_id
WHERE p.workspace_id = $1
  AND p.status = 'active'
  AND ($2::uuid IS NULL OR parent.project_id = $2::uuid)
ORDER BY p.updated_at DESC, p.id ASC
LIMIT $3 OFFSET $4;

-- name: CreateDependencyGraphPlan :one
INSERT INTO dependency_graph_plan (workspace_id, parent_issue_id,
    idempotency_key, request_hash, goal, status, created_by_type, created_by_id, id)
VALUES ($1, $2, $3, $4, $5, COALESCE(sqlc.narg('status')::text, 'active'),
        $6, $7, COALESCE(sqlc.narg('id')::uuid, gen_random_uuid()))
RETURNING id, workspace_id, parent_issue_id, idempotency_key, request_hash,
          goal, status, created_by_type, created_by_id, created_at, updated_at,
          attention_required, attention_reason;

-- name: UpdateDependencyGraphPlanStatus :one
UPDATE dependency_graph_plan
SET status = $3, updated_at = now()
WHERE id = $1 AND workspace_id = $2
RETURNING id, workspace_id, parent_issue_id, idempotency_key, request_hash,
          goal, status, created_by_type, created_by_id, created_at, updated_at,
          attention_required, attention_reason;

-- name: GetDependencyGraphNodeByID :one
SELECT id, plan_id, workspace_id, temp_id, issue_id, title, description,
       acceptance_criteria, context, outputs, executor_type, executor_id,
       candidate_executors, wave, created_at, updated_at, owner_type, owner_id,
       reviewer_type, reviewer_id, runtime_id, model_id
FROM dependency_graph_node
WHERE id = $1 AND workspace_id = $2;

-- name: ListDependencyGraphNodesByPlan :many
SELECT id, plan_id, workspace_id, temp_id, issue_id, title, description,
       acceptance_criteria, context, outputs, executor_type, executor_id,
       candidate_executors, wave, created_at, updated_at, owner_type, owner_id,
       reviewer_type, reviewer_id, runtime_id, model_id
FROM dependency_graph_node
WHERE plan_id = $1 AND workspace_id = $2
ORDER BY wave, temp_id, id;

-- name: CreateDependencyGraphNode :one
INSERT INTO dependency_graph_node (
    plan_id, workspace_id, temp_id, issue_id, title, description,
    acceptance_criteria, context, outputs, executor_type, executor_id,
    candidate_executors, owner_type, owner_id, reviewer_type, reviewer_id,
    runtime_id, model_id, wave, id
)
VALUES (
    $1, $2, $3, $4, $5, COALESCE($6, ''),
    COALESCE($7::jsonb, '[]'), COALESCE($8::jsonb, '{}'),
    COALESCE($9::jsonb, '[]'), $12, $13, COALESCE($10::jsonb, '[]'),
    $14, $15, $16, $17, $18, $19, $11,
    COALESCE($20::uuid, gen_random_uuid())
)
RETURNING id, plan_id, workspace_id, temp_id, issue_id, title, description,
          acceptance_criteria, context, outputs, executor_type, executor_id,
          candidate_executors, wave, created_at, updated_at, owner_type, owner_id,
          reviewer_type, reviewer_id, runtime_id, model_id;

-- name: GetDependencyGraphEdgeByID :one
SELECT id, plan_id, workspace_id, from_issue_id, to_issue_id, type, reason,
       consumed_output, created_at
FROM dependency_graph_edge
WHERE id = $1 AND workspace_id = $2;

-- name: ListDependencyGraphEdgesByPlan :many
SELECT id, plan_id, workspace_id, from_issue_id, to_issue_id, type, reason,
       consumed_output, created_at
FROM dependency_graph_edge
WHERE plan_id = $1 AND workspace_id = $2
ORDER BY from_issue_id, to_issue_id, id;

-- name: CreateDependencyGraphEdge :one
INSERT INTO dependency_graph_edge (
    plan_id, workspace_id, from_issue_id, to_issue_id, type, reason,
    consumed_output, id
)
VALUES ($1, $2, $3, $4, $5, $6, $7,
        COALESCE(sqlc.narg('id')::uuid, gen_random_uuid()))
RETURNING id, plan_id, workspace_id, from_issue_id, to_issue_id, type, reason,
          consumed_output, created_at;

-- name: GetActiveDependencyGraphForIssue :one
SELECT p.id, p.workspace_id, p.parent_issue_id, p.idempotency_key,
       p.request_hash, p.goal, p.status, p.created_by_type, p.created_by_id,
       p.created_at, p.updated_at, p.attention_required, p.attention_reason
FROM dependency_graph_plan p
WHERE p.workspace_id = $1
  AND p.status = 'active'
  AND (
      p.parent_issue_id = $2
      OR EXISTS (
          SELECT 1
          FROM dependency_graph_node n
          WHERE n.plan_id = p.id
            AND n.workspace_id = p.workspace_id
            AND n.issue_id = $2
      )
  )
ORDER BY CASE WHEN EXISTS (
    SELECT 1
    FROM dependency_graph_node n
    WHERE n.plan_id = p.id
      AND n.workspace_id = p.workspace_id
      AND n.issue_id = $2
) THEN 0 ELSE 1 END, p.updated_at DESC, p.id ASC
LIMIT 1;

-- name: CreateDependencyGraphIssueCreatedOutbox :exec
INSERT INTO dependency_graph_issue_created_outbox
    (plan_id, node_id, workspace_id, issue_id, status, attempt)
VALUES ($1, $2, $3, $4, 'pending', 0)
ON CONFLICT (plan_id, node_id) DO NOTHING;
