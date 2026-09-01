-- Dependency graph execution plans.

-- name: GetDependencyGraphPlanByID :one
SELECT * FROM dependency_graph_plan WHERE id = $1 AND workspace_id = $2;

-- name: GetDependencyGraphPlanForUpdate :one
SELECT * FROM dependency_graph_plan WHERE id = $1 AND workspace_id = $2 FOR UPDATE;

-- name: GetDependencyGraphPlanByIdempotency :one
SELECT * FROM dependency_graph_plan WHERE workspace_id = $1 AND idempotency_key = $2 LIMIT 1;

-- name: GetActiveDependencyGraphPlanForParent :one
SELECT * FROM dependency_graph_plan WHERE workspace_id = $1 AND parent_issue_id = $2 AND status = 'active' LIMIT 1;

-- name: ListDependencyGraphPlans :many
SELECT * FROM dependency_graph_plan
WHERE workspace_id = $1
  AND ($2::uuid IS NULL OR project_id = $2::uuid)
ORDER BY updated_at DESC, id DESC
LIMIT $3 OFFSET $4;

-- name: CreateDependencyGraphPlan :one
INSERT INTO dependency_graph_plan (workspace_id, parent_issue_id, idempotency_key, request_hash, goal, status, created_by_type, created_by_id, id)
VALUES ($1, $2, $3, $4, $5, COALESCE(sqlc.narg('status')::text, 'active'), $6, $7, COALESCE(sqlc.narg('id')::uuid, gen_random_uuid()))
RETURNING *;

-- name: UpdateDependencyGraphPlanStatus :one
UPDATE dependency_graph_plan SET status = $3, updated_at = now()
WHERE id = $1 AND workspace_id = $2
RETURNING *;

-- name: GetDependencyGraphNodeByID :one
SELECT * FROM dependency_graph_node WHERE id = $1 AND workspace_id = $2;

-- name: ListDependencyGraphNodesByPlan :many
SELECT * FROM dependency_graph_node WHERE plan_id = $1 AND workspace_id = $2 ORDER BY wave, created_at;

-- name: CreateDependencyGraphNode :one
INSERT INTO dependency_graph_node (plan_id, workspace_id, temp_id, issue_id, title, description, acceptance_criteria, context, outputs, executor_type, executor_id, candidate_executors, owner_type, owner_id, reviewer_type, reviewer_id, runtime_id, model_id, wave, id)
VALUES ($1, $2, $3, $4, $5, COALESCE($6, ''), COALESCE($7::jsonb, '[]'), COALESCE($8::jsonb, '{}'), COALESCE($9::jsonb, '[]'), sqlc.narg('executor_type'), sqlc.narg('executor_id'), COALESCE($10::jsonb, '[]'), sqlc.narg('owner_type'), sqlc.narg('owner_id'), sqlc.narg('reviewer_type'), sqlc.narg('reviewer_id'), sqlc.narg('runtime_id'), sqlc.narg('model_id'), $11, COALESCE(sqlc.narg('id')::uuid, gen_random_uuid()))
RETURNING *;

-- name: GetDependencyGraphEdgeByID :one
SELECT * FROM dependency_graph_edge WHERE id = $1 AND workspace_id = $2;

-- name: ListDependencyGraphEdgesByPlan :many
SELECT * FROM dependency_graph_edge WHERE plan_id = $1 AND workspace_id = $2 ORDER BY created_at;

-- name: CreateDependencyGraphEdge :one
INSERT INTO dependency_graph_edge (plan_id, workspace_id, from_issue_id, to_issue_id, type, reason, consumed_output, id)
VALUES ($1, $2, $3, $4, $5, $6, $7, COALESCE(sqlc.narg('id')::uuid, gen_random_uuid()))
RETURNING *;

-- name: GetActiveDependencyGraphForIssue :one
SELECT p.* FROM dependency_graph_plan p
JOIN dependency_graph_node n ON n.plan_id = p.id AND n.workspace_id = p.workspace_id
WHERE p.workspace_id = $1 AND n.issue_id = $2 AND p.status = 'active'
LIMIT 1;
