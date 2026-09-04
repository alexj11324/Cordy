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

-- The graph coordinator owns the blocked -> todo transition. Keep the update
-- conditional and idempotent so repeated task completion events cannot admit a
-- node before every active hard prerequisite is done.
-- name: PromoteReadyDependencyGraphIssuesForPlan :many
UPDATE issue AS target
SET status = 'todo',
    revision = revision + 1,
    last_activity_at = GREATEST(COALESCE(target.last_activity_at, target.updated_at), now()),
    updated_at = now()
WHERE target.workspace_id = $1
  AND issue_effective_status(target.workspace_id, target.status) = 'blocked'
  AND EXISTS (
      SELECT 1
      FROM dependency_graph_node node
      JOIN dependency_graph_plan plan
        ON plan.id = node.plan_id
       AND plan.workspace_id = node.workspace_id
       AND plan.status = 'active'
      WHERE node.plan_id = $2
        AND node.workspace_id = $1
        AND node.issue_id = target.id
  )
  AND dependency_graph_issue_gate_open($1, target.id)
RETURNING target.id;

-- name: PromoteReadyDependencyGraphDependents :many
UPDATE issue AS target
SET status = 'todo',
    revision = revision + 1,
    last_activity_at = GREATEST(COALESCE(last_activity_at, updated_at), now()),
    updated_at = now()
WHERE target.workspace_id = $1
  AND issue_effective_status(target.workspace_id, target.status) = 'blocked'
  AND EXISTS (
      SELECT 1
      FROM dependency_graph_edge edge
      JOIN dependency_graph_plan plan
        ON plan.id = edge.plan_id
       AND plan.workspace_id = edge.workspace_id
       AND plan.status = 'active'
      WHERE edge.workspace_id = $1
        AND edge.from_issue_id = $2
        AND edge.to_issue_id = target.id
  )
  AND dependency_graph_issue_gate_open($1, target.id)
RETURNING target.id;

-- Runtime recovery closes the commit-to-enqueue window for one agent runtime.
-- Team-owned graph work is recovered through the team's leader agent.
-- name: PromoteReadyDependencyGraphIssuesForRuntime :many
UPDATE issue AS target
SET status = 'todo',
    revision = revision + 1,
    last_activity_at = GREATEST(COALESCE(target.last_activity_at, target.updated_at), now()),
    updated_at = now()
FROM agent agent_owner
WHERE target.workspace_id = agent_owner.workspace_id
  AND agent_owner.id = CASE
      WHEN target.executor_type = 'team' THEN (
          SELECT team.leader_id
          FROM team
          WHERE team.id = target.executor_id
            AND team.workspace_id = target.workspace_id
            AND team.archived_at IS NULL
      )
      ELSE target.executor_id
  END
  AND agent_owner.runtime_id = $1
  AND agent_owner.archived_at IS NULL
  AND issue_effective_status(target.workspace_id, target.status) = 'blocked'
  AND target.executor_type IN ('agent', 'team')
  AND EXISTS (
      SELECT 1
      FROM dependency_graph_node node
      JOIN dependency_graph_plan plan
        ON plan.id = node.plan_id
       AND plan.workspace_id = node.workspace_id
       AND plan.status = 'active'
      WHERE node.workspace_id = target.workspace_id
        AND node.issue_id = target.id
  )
  AND dependency_graph_issue_gate_open(target.workspace_id, target.id)
RETURNING target.id;

-- A pending queue row is treated as already admitted. This makes recovery and
-- completion wakeups safe to replay without creating duplicate task slots.
-- name: ListReadyDependencyGraphIssueIDsForPlan :many
SELECT issue.id
FROM dependency_graph_node node
JOIN dependency_graph_plan plan
  ON plan.id = node.plan_id
 AND plan.workspace_id = node.workspace_id
 AND plan.status = 'active'
JOIN issue
  ON issue.id = node.issue_id
 AND issue.workspace_id = node.workspace_id
LEFT JOIN team team_owner
  ON team_owner.id = issue.executor_id
 AND team_owner.workspace_id = issue.workspace_id
 AND team_owner.archived_at IS NULL
WHERE node.workspace_id = $1
  AND node.plan_id = $2
  AND (
      (issue.executor_type = 'agent' AND issue.executor_id IS NOT NULL)
      OR (issue.executor_type = 'team' AND team_owner.id IS NOT NULL)
  )
  AND issue_effective_status(issue.workspace_id, issue.status) = 'todo'
  AND dependency_graph_issue_gate_open(issue.workspace_id, issue.id)
  AND NOT EXISTS (
      SELECT 1
      FROM agent_task_queue pending
      WHERE pending.issue_id = issue.id
        AND pending.agent_id = CASE
            WHEN issue.executor_type = 'team' THEN team_owner.leader_id
            ELSE issue.executor_id
        END
        AND pending.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'waiting_capacity', 'deferred')
  )
ORDER BY node.wave ASC, node.temp_id ASC;

-- name: ListReadyDependencyGraphIssueIDsForWorkspace :many
SELECT issue.id
FROM dependency_graph_node node
JOIN dependency_graph_plan plan
  ON plan.id = node.plan_id
 AND plan.workspace_id = node.workspace_id
 AND plan.status = 'active'
JOIN issue
  ON issue.id = node.issue_id
 AND issue.workspace_id = node.workspace_id
LEFT JOIN team team_owner
  ON team_owner.id = issue.executor_id
 AND team_owner.workspace_id = issue.workspace_id
 AND team_owner.archived_at IS NULL
WHERE node.workspace_id = $1
  AND (
      (issue.executor_type = 'agent' AND issue.executor_id IS NOT NULL)
      OR (issue.executor_type = 'team' AND team_owner.id IS NOT NULL)
  )
  AND issue_effective_status(issue.workspace_id, issue.status) = 'todo'
  AND dependency_graph_issue_gate_open(issue.workspace_id, issue.id)
  AND NOT EXISTS (
      SELECT 1
      FROM agent_task_queue pending
      WHERE pending.issue_id = issue.id
        AND pending.agent_id = CASE
            WHEN issue.executor_type = 'team' THEN team_owner.leader_id
            ELSE issue.executor_id
        END
        AND pending.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'waiting_capacity', 'deferred')
  )
ORDER BY node.wave ASC, node.temp_id ASC;

-- name: ListReadyDependencyGraphIssueIDsForRuntime :many
SELECT issue.id
FROM dependency_graph_node node
JOIN dependency_graph_plan plan
  ON plan.id = node.plan_id
 AND plan.workspace_id = node.workspace_id
 AND plan.status = 'active'
JOIN issue
  ON issue.id = node.issue_id
 AND issue.workspace_id = node.workspace_id
LEFT JOIN team team_owner
  ON team_owner.id = issue.executor_id
 AND team_owner.workspace_id = issue.workspace_id
 AND team_owner.archived_at IS NULL
JOIN agent agent_owner
  ON agent_owner.id = CASE
      WHEN issue.executor_type = 'team' THEN team_owner.leader_id
      ELSE issue.executor_id
  END
 AND agent_owner.workspace_id = issue.workspace_id
WHERE agent_owner.runtime_id = $1
  AND agent_owner.archived_at IS NULL
  AND (
      (issue.executor_type = 'agent' AND issue.executor_id IS NOT NULL)
      OR (issue.executor_type = 'team' AND team_owner.id IS NOT NULL)
  )
  AND issue_effective_status(issue.workspace_id, issue.status) = 'todo'
  AND dependency_graph_issue_gate_open(issue.workspace_id, issue.id)
  AND NOT EXISTS (
      SELECT 1
      FROM agent_task_queue pending
      WHERE pending.issue_id = issue.id
        AND pending.agent_id = CASE
            WHEN issue.executor_type = 'team' THEN team_owner.leader_id
            ELSE issue.executor_id
        END
        AND pending.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'waiting_capacity', 'deferred')
  )
ORDER BY node.wave ASC, node.temp_id ASC;

-- Admission is the only scheduler-owned status transition for graph work.
-- name: AdmitReadyDependencyGraphIssue :many
UPDATE issue
SET status = 'in_progress',
    revision = revision + 1,
    last_activity_at = GREATEST(COALESCE(last_activity_at, updated_at), now()),
    updated_at = now()
WHERE id = $1
  AND workspace_id = $2
  AND issue_effective_status(workspace_id, status) = 'todo'
  AND executor_type IN ('agent', 'team')
  AND executor_id IS NOT NULL
  AND dependency_graph_issue_gate_open(workspace_id, id)
RETURNING id;

-- Failed/cancelled prerequisites never open a gate. They mark the active plan
-- for operator attention so the graph remains auditable and fail-closed.
-- name: MarkDependencyGraphAttentionForPrerequisite :many
UPDATE dependency_graph_plan AS plan
SET attention_required = true,
    attention_reason = $3,
    updated_at = now()
WHERE plan.workspace_id = $1
  AND plan.status = 'active'
  AND EXISTS (
      SELECT 1
      FROM dependency_graph_edge edge
      WHERE edge.plan_id = plan.id
        AND edge.workspace_id = plan.workspace_id
        AND edge.from_issue_id = $2
  )
RETURNING plan.id;
