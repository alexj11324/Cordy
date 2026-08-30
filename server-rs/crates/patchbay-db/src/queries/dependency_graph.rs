//! Typed SQL for the dependency-graph execution domain.
//!
//! The graph tables intentionally have no foreign keys. Every query that
//! crosses a workspace boundary carries `workspace_id`, and the service owns
//! the transaction that validates and writes a complete plan.

use crate::models::{DependencyGraphEdge, DependencyGraphNode, DependencyGraphPlan};
use sqlx::{Executor, Row};
use uuid::Uuid;

const PLAN_COLUMNS: &str = "id, workspace_id, parent_issue_id, idempotency_key, request_hash, goal, status, created_by_type, created_by_id, attention_required, attention_reason, created_at, updated_at";
const NODE_COLUMNS: &str = "id, plan_id, workspace_id, temp_id, issue_id, title, description, acceptance_criteria, context, outputs, assignee_type, assignee_id, candidate_assignees, wave, created_at, updated_at";
const EDGE_COLUMNS: &str = "id, plan_id, workspace_id, from_issue_id, to_issue_id, type AS type_, reason, consumed_output, created_at";

pub async fn get_plan_by_id<'e, E>(
    executor: E,
    plan_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<DependencyGraphPlan>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphPlan>(&format!(
        "SELECT {PLAN_COLUMNS} FROM dependency_graph_plan WHERE id = $1 AND workspace_id = $2"
    ))
    .bind(plan_id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?)
}

pub async fn get_plan_by_idempotency<'e, E>(
    executor: E,
    workspace_id: Uuid,
    idempotency_key: &str,
    for_update: bool,
) -> anyhow::Result<Option<DependencyGraphPlan>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let suffix = if for_update { " FOR UPDATE" } else { "" };
    Ok(sqlx::query_as::<_, DependencyGraphPlan>(&format!(
        "SELECT {PLAN_COLUMNS} FROM dependency_graph_plan WHERE workspace_id = $1 AND idempotency_key = $2{suffix}"
    ))
    .bind(workspace_id)
    .bind(idempotency_key)
    .fetch_optional(executor)
    .await?)
}

pub async fn get_active_plan_for_parent<'e, E>(
    executor: E,
    workspace_id: Uuid,
    parent_issue_id: Uuid,
    for_update: bool,
) -> anyhow::Result<Option<DependencyGraphPlan>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let suffix = if for_update { " FOR UPDATE" } else { "" };
    Ok(sqlx::query_as::<_, DependencyGraphPlan>(&format!(
        "SELECT {PLAN_COLUMNS} FROM dependency_graph_plan WHERE workspace_id = $1 AND parent_issue_id = $2 AND status = 'active'{suffix}"
    ))
    .bind(workspace_id)
    .bind(parent_issue_id)
    .fetch_optional(executor)
    .await?)
}

/// Finds the active graph that owns an issue either as its planner parent or
/// as one of its persisted task nodes. Node ownership wins when an issue is
/// both a graph task and a parent for another graph: the issue detail must
/// explain the prerequisites that gate the task itself.
pub async fn get_active_plan_for_issue<'e, E>(
    executor: E,
    workspace_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<Option<DependencyGraphPlan>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphPlan>(&format!(
        "SELECT {PLAN_COLUMNS} FROM dependency_graph_plan plan WHERE plan.workspace_id = $1 AND plan.status = 'active' AND (plan.parent_issue_id = $2 OR EXISTS (SELECT 1 FROM dependency_graph_node node WHERE node.plan_id = plan.id AND node.workspace_id = plan.workspace_id AND node.issue_id = $2)) ORDER BY CASE WHEN EXISTS (SELECT 1 FROM dependency_graph_node node WHERE node.plan_id = plan.id AND node.workspace_id = plan.workspace_id AND node.issue_id = $2) THEN 0 ELSE 1 END, plan.updated_at DESC, plan.id ASC LIMIT 1"
    ))
    .bind(workspace_id)
    .bind(issue_id)
    .fetch_optional(executor)
    .await?)
}

pub async fn list_active_plans<'e, E>(
    executor: E,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    limit: i64,
) -> anyhow::Result<Vec<DependencyGraphPlan>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphPlan>(&format!(
        "SELECT {PLAN_COLUMNS} FROM dependency_graph_plan plan WHERE plan.workspace_id = $1 AND plan.status = 'active' AND ($2::uuid IS NULL OR EXISTS (SELECT 1 FROM issue parent WHERE parent.id = plan.parent_issue_id AND parent.workspace_id = plan.workspace_id AND parent.project_id = $2)) ORDER BY plan.updated_at DESC, plan.id ASC LIMIT $3"
    ))
    .bind(workspace_id)
    .bind(project_id)
    .bind(limit)
    .fetch_all(executor)
    .await?)
}

pub async fn insert_plan<'e, E>(
    executor: E,
    plan: &DependencyGraphPlanInsert,
) -> anyhow::Result<Option<DependencyGraphPlan>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphPlan>(&format!(
        "INSERT INTO dependency_graph_plan (id, workspace_id, parent_issue_id, idempotency_key, request_hash, goal, status, created_by_type, created_by_id) VALUES ($1, $2, $3, $4, $5, $6, 'active', $7, $8) ON CONFLICT (workspace_id, idempotency_key) DO NOTHING RETURNING {PLAN_COLUMNS}"
    ))
    .bind(plan.id)
    .bind(plan.workspace_id)
    .bind(plan.parent_issue_id)
    .bind(&plan.idempotency_key)
    .bind(&plan.request_hash)
    .bind(&plan.goal)
    .bind(&plan.created_by_type)
    .bind(plan.created_by_id)
    .fetch_optional(executor)
    .await?)
}

#[derive(Debug, Clone)]
pub struct DependencyGraphPlanInsert {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub parent_issue_id: Uuid,
    pub idempotency_key: String,
    pub request_hash: String,
    pub goal: String,
    pub created_by_type: String,
    pub created_by_id: Uuid,
}

pub async fn insert_node<'e, E>(
    executor: E,
    node: &DependencyGraphNodeInsert,
) -> anyhow::Result<DependencyGraphNode>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphNode>(&format!(
        "INSERT INTO dependency_graph_node (id, plan_id, workspace_id, temp_id, issue_id, title, description, acceptance_criteria, context, outputs, assignee_type, assignee_id, candidate_assignees, wave) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) RETURNING {NODE_COLUMNS}"
    ))
    .bind(node.id)
    .bind(node.plan_id)
    .bind(node.workspace_id)
    .bind(&node.temp_id)
    .bind(node.issue_id)
    .bind(&node.title)
    .bind(&node.description)
    .bind(&node.acceptance_criteria)
    .bind(&node.context)
    .bind(&node.outputs)
    .bind(node.assignee_type.as_deref())
    .bind(node.assignee_id)
    .bind(&node.candidate_assignees)
    .bind(node.wave)
    .fetch_one(executor)
    .await?)
}

#[derive(Debug, Clone)]
pub struct DependencyGraphNodeInsert {
    pub id: Uuid,
    pub plan_id: Uuid,
    pub workspace_id: Uuid,
    pub temp_id: String,
    pub issue_id: Uuid,
    pub title: String,
    pub description: String,
    pub acceptance_criteria: serde_json::Value,
    pub context: serde_json::Value,
    pub outputs: serde_json::Value,
    pub assignee_type: Option<String>,
    pub assignee_id: Option<Uuid>,
    pub candidate_assignees: serde_json::Value,
    pub wave: i32,
}

pub async fn insert_edge<'e, E>(
    executor: E,
    edge: &DependencyGraphEdgeInsert,
) -> anyhow::Result<DependencyGraphEdge>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphEdge>(&format!(
        "INSERT INTO dependency_graph_edge (id, plan_id, workspace_id, from_issue_id, to_issue_id, type, reason, consumed_output) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {EDGE_COLUMNS}"
    ))
    .bind(edge.id)
    .bind(edge.plan_id)
    .bind(edge.workspace_id)
    .bind(edge.from_issue_id)
    .bind(edge.to_issue_id)
    .bind(&edge.type_)
    .bind(&edge.reason)
    .bind(&edge.consumed_output)
    .fetch_one(executor)
    .await?)
}

#[derive(Debug, Clone)]
pub struct DependencyGraphEdgeInsert {
    pub id: Uuid,
    pub plan_id: Uuid,
    pub workspace_id: Uuid,
    pub from_issue_id: Uuid,
    pub to_issue_id: Uuid,
    pub type_: String,
    pub reason: String,
    pub consumed_output: String,
}

pub async fn list_nodes<'e, E>(
    executor: E,
    plan_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<DependencyGraphNode>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphNode>(&format!(
        "SELECT {NODE_COLUMNS} FROM dependency_graph_node WHERE plan_id = $1 AND workspace_id = $2 ORDER BY wave ASC, temp_id ASC"
    ))
    .bind(plan_id)
    .bind(workspace_id)
    .fetch_all(executor)
    .await?)
}

pub async fn list_edges<'e, E>(
    executor: E,
    plan_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<DependencyGraphEdge>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphEdge>(&format!(
        "SELECT {EDGE_COLUMNS} FROM dependency_graph_edge WHERE plan_id = $1 AND workspace_id = $2 ORDER BY from_issue_id ASC, to_issue_id ASC, id ASC"
    ))
    .bind(plan_id)
    .bind(workspace_id)
    .fetch_all(executor)
    .await?)
}

pub async fn lock_parent_issue<'e, E>(
    executor: E,
    parent_issue_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<LockedParentIssue>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, LockedParentIssue>(
        "SELECT id, project_id FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
    )
    .bind(parent_issue_id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LockedParentIssue {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DependencyGateState {
    pub gate_open: bool,
    pub satisfied_prerequisites: i64,
    pub total_prerequisites: i64,
}

/// Reads the same predicate used by queue INSERT/claim admission. Missing
/// prerequisite rows count as unsatisfied through the database function.
pub async fn get_gate_state<'e, E>(
    executor: E,
    workspace_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<DependencyGateState>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGateState>(
        r#"SELECT
    dependency_graph_issue_gate_open($1, $2) AS gate_open,
    COALESCE((
        SELECT COUNT(*)::bigint
        FROM dependency_graph_edge edge
        JOIN dependency_graph_plan plan
          ON plan.id = edge.plan_id
         AND plan.workspace_id = edge.workspace_id
         AND plan.status = 'active'
        JOIN issue prerequisite
          ON prerequisite.id = edge.from_issue_id
         AND prerequisite.workspace_id = edge.workspace_id
        WHERE edge.workspace_id = $1
          AND edge.to_issue_id = $2
          AND issue_effective_status(prerequisite.workspace_id, prerequisite.status) = 'done'
    ), 0)::bigint AS satisfied_prerequisites,
    COALESCE((
        SELECT COUNT(*)::bigint
        FROM dependency_graph_edge edge
        JOIN dependency_graph_plan plan
          ON plan.id = edge.plan_id
         AND plan.workspace_id = edge.workspace_id
         AND plan.status = 'active'
        WHERE edge.workspace_id = $1
          AND edge.to_issue_id = $2
    ), 0)::bigint AS total_prerequisites"#,
    )
    .bind(workspace_id)
    .bind(issue_id)
    .fetch_one(executor)
    .await?)
}

/// Promotes blocked nodes whose active-plan prerequisites are all successful.
/// The UPDATE is deliberately idempotent: a concurrent wakeup can only match
/// a row while it is still `blocked`.
pub async fn promote_ready_issues_for_plan<'e, E>(
    executor: E,
    workspace_id: Uuid,
    plan_id: Uuid,
) -> anyhow::Result<Vec<Uuid>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"UPDATE issue AS target
SET status = 'todo', updated_at = now()
WHERE target.workspace_id = $1
  AND target.status = 'blocked'
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
RETURNING target.id"#,
    )
    .bind(workspace_id)
    .bind(plan_id)
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect())
}

/// Completion-path wakeup for one prerequisite. The predicate still checks
/// every incoming edge, so a premature or replayed wake cannot release a
/// dependent until all hard prerequisites are done.
pub async fn promote_ready_dependents<'e, E>(
    executor: E,
    workspace_id: Uuid,
    prerequisite_issue_id: Uuid,
) -> anyhow::Result<Vec<Uuid>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"UPDATE issue AS target
SET status = 'todo', updated_at = now()
WHERE target.workspace_id = $1
  AND target.status = 'blocked'
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
RETURNING target.id"#,
    )
    .bind(workspace_id)
    .bind(prerequisite_issue_id)
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect())
}

/// Crash/restart reconciliation for one runtime. It promotes only its own
/// agent-assigned nodes, then the caller can enqueue them through the normal
/// service path. This closes the commit-to-enqueue window without changing
/// the queue's duplicate-slot semantics.
pub async fn promote_ready_issues_for_runtime<'e, E>(
    executor: E,
    runtime_id: Uuid,
) -> anyhow::Result<Vec<Uuid>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"UPDATE issue AS target
SET status = 'todo', updated_at = now()
FROM agent agent_owner
WHERE target.workspace_id = agent_owner.workspace_id
  AND target.assignee_type = 'agent'
  AND target.assignee_id = agent_owner.id
  AND agent_owner.runtime_id = $1
  AND agent_owner.archived_at IS NULL
  AND target.status = 'blocked'
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
RETURNING target.id"#,
    )
    .bind(runtime_id)
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect())
}

/// Lists ready agent-owned graph nodes for a plan. A pending queue row is
/// treated as already admitted so retries/replays never create another task.
pub async fn list_ready_issue_ids_for_plan<'e, E>(
    executor: E,
    workspace_id: Uuid,
    plan_id: Uuid,
) -> anyhow::Result<Vec<Uuid>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"SELECT issue.id
FROM dependency_graph_node node
JOIN dependency_graph_plan plan
  ON plan.id = node.plan_id
 AND plan.workspace_id = node.workspace_id
 AND plan.status = 'active'
JOIN issue ON issue.id = node.issue_id
         AND issue.workspace_id = node.workspace_id
WHERE node.workspace_id = $1
  AND node.plan_id = $2
  AND issue.assignee_type = 'agent'
  AND issue.assignee_id IS NOT NULL
  AND issue_effective_status(issue.workspace_id, issue.status) = 'todo'
  AND dependency_graph_issue_gate_open(issue.workspace_id, issue.id)
  AND NOT EXISTS (
      SELECT 1
      FROM agent_task_queue pending
      WHERE pending.issue_id = issue.id
        AND pending.agent_id = issue.assignee_id
        AND pending.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
  )
ORDER BY node.wave ASC, node.temp_id ASC"#,
    )
    .bind(workspace_id)
    .bind(plan_id)
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect())
}

/// Lists every ready graph task in a workspace for completion-path wakeups.
pub async fn list_ready_issue_ids_for_workspace<'e, E>(
    executor: E,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<Uuid>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"SELECT issue.id
FROM dependency_graph_node node
JOIN dependency_graph_plan plan
  ON plan.id = node.plan_id
 AND plan.workspace_id = node.workspace_id
 AND plan.status = 'active'
JOIN issue ON issue.id = node.issue_id
         AND issue.workspace_id = node.workspace_id
WHERE node.workspace_id = $1
  AND issue.assignee_type = 'agent'
  AND issue.assignee_id IS NOT NULL
  AND issue_effective_status(issue.workspace_id, issue.status) = 'todo'
  AND dependency_graph_issue_gate_open(issue.workspace_id, issue.id)
  AND NOT EXISTS (
      SELECT 1
      FROM agent_task_queue pending
      WHERE pending.issue_id = issue.id
        AND pending.agent_id = issue.assignee_id
        AND pending.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
  )
ORDER BY node.wave ASC, node.temp_id ASC"#,
    )
    .bind(workspace_id)
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect())
}

/// Runtime-scoped form used by claim recovery before the candidate SELECT.
pub async fn list_ready_issue_ids_for_runtime<'e, E>(
    executor: E,
    runtime_id: Uuid,
) -> anyhow::Result<Vec<Uuid>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"SELECT issue.id
FROM dependency_graph_node node
JOIN dependency_graph_plan plan
  ON plan.id = node.plan_id
 AND plan.workspace_id = node.workspace_id
 AND plan.status = 'active'
JOIN issue ON issue.id = node.issue_id
         AND issue.workspace_id = node.workspace_id
JOIN agent agent_owner
  ON agent_owner.id = issue.assignee_id
 AND agent_owner.workspace_id = issue.workspace_id
WHERE agent_owner.runtime_id = $1
  AND agent_owner.archived_at IS NULL
  AND issue.assignee_type = 'agent'
  AND issue.assignee_id IS NOT NULL
  AND issue_effective_status(issue.workspace_id, issue.status) = 'todo'
  AND dependency_graph_issue_gate_open(issue.workspace_id, issue.id)
  AND NOT EXISTS (
      SELECT 1
      FROM agent_task_queue pending
      WHERE pending.issue_id = issue.id
        AND pending.agent_id = issue.assignee_id
        AND pending.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
  )
ORDER BY node.wave ASC, node.temp_id ASC"#,
    )
    .bind(runtime_id)
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect())
}

/// Persists a fail-closed attention marker when a prerequisite fails or is
/// cancelled. It is intentionally separate from plan cancellation: operators
/// can inspect/replan the active graph without losing its audit history.
pub async fn mark_attention_for_prerequisite<'e, E>(
    executor: E,
    workspace_id: Uuid,
    prerequisite_issue_id: Uuid,
    reason: &str,
) -> anyhow::Result<Vec<Uuid>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"UPDATE dependency_graph_plan AS plan
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
RETURNING plan.id"#,
    )
    .bind(workspace_id)
    .bind(prerequisite_issue_id)
    .bind(reason)
    .fetch_all(executor)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| row.get::<Uuid, _>("id"))
        .collect())
}

pub async fn set_issue_acceptance_criteria<'e, E>(
    executor: E,
    issue_id: Uuid,
    workspace_id: Uuid,
    acceptance_criteria: &serde_json::Value,
) -> anyhow::Result<()>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(
        "UPDATE issue SET acceptance_criteria = $1, updated_at = now() WHERE id = $2 AND workspace_id = $3",
    )
    .bind(acceptance_criteria)
    .bind(issue_id)
    .bind(workspace_id)
    .execute(executor)
    .await?;
    Ok(())
}

pub async fn validate_assignee<'e, E>(
    executor: E,
    workspace_id: Uuid,
    assignee_type: &str,
    assignee_id: Uuid,
) -> anyhow::Result<bool>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let exists = match assignee_type {
        "member" => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM member WHERE id = $1 AND workspace_id = $2)",
        )
        .bind(assignee_id)
        .bind(workspace_id)
        .fetch_one(executor)
        .await?,
        "agent" => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM agent WHERE id = $1 AND workspace_id = $2 AND archived_at IS NULL)",
        )
        .bind(assignee_id)
        .bind(workspace_id)
        .fetch_one(executor)
        .await?,
        "team" => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM team WHERE id = $1 AND workspace_id = $2 AND archived_at IS NULL)",
        )
        .bind(assignee_id)
        .bind(workspace_id)
        .fetch_one(executor)
        .await?,
        _ => false,
    };
    Ok(exists)
}
