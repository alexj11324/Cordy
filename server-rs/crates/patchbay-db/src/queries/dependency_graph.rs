//! Typed SQL for the dependency-graph execution domain.
//!
//! The graph tables intentionally have no foreign keys. Every query that
//! crosses a workspace boundary carries `workspace_id`, and the service owns
//! the transaction that validates and writes a complete plan.

use crate::models::{DependencyGraphEdge, DependencyGraphNode, DependencyGraphPlan};
use chrono::{DateTime, Utc};
use sqlx::{Executor, Row};
use uuid::Uuid;

const PLAN_COLUMNS: &str = "id, workspace_id, parent_issue_id, idempotency_key, request_hash, goal, status, created_by_type, created_by_id, attention_required, attention_reason, created_at, updated_at";
const NODE_COLUMNS: &str = "id, plan_id, workspace_id, temp_id, issue_id, title, description, acceptance_criteria, context, outputs, executor_type, executor_id, candidate_executors, owner_type, owner_id, reviewer_type, reviewer_id, runtime_id, model_id, wave, created_at, updated_at";
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

pub async fn lock_plan_by_id<'e, E>(
    executor: E,
    plan_id: Uuid,
    workspace_id: Uuid,
) -> anyhow::Result<Option<DependencyGraphPlan>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphPlan>(&format!(
        "SELECT {PLAN_COLUMNS} FROM dependency_graph_plan WHERE id = $1 AND workspace_id = $2 FOR UPDATE"
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
    after: Option<(DateTime<Utc>, Uuid)>,
) -> anyhow::Result<Vec<DependencyGraphPlan>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphPlan>(&format!(
        "SELECT {PLAN_COLUMNS} FROM dependency_graph_plan plan WHERE plan.workspace_id = $1 AND plan.status = 'active' AND ($2::uuid IS NULL OR EXISTS (SELECT 1 FROM issue parent WHERE parent.id = plan.parent_issue_id AND parent.workspace_id = plan.workspace_id AND parent.project_id = $2)) AND ($3::timestamptz IS NULL OR plan.updated_at < $3 OR (plan.updated_at = $3 AND plan.id > $4)) ORDER BY plan.updated_at DESC, plan.id ASC LIMIT $5"
    ))
    .bind(workspace_id)
    .bind(project_id)
    .bind(after.map(|(updated_at, _)| updated_at))
    .bind(after.map(|(_, id)| id))
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

/// Retires one active plan so its parent can be replanned. A cancelled plan
/// remains queryable for audit history but is no longer considered by the
/// execution gate or the active-plan uniqueness invariant.
pub async fn retire_active_plan<'e, E>(
    executor: E,
    workspace_id: Uuid,
    plan_id: Uuid,
) -> anyhow::Result<Option<DependencyGraphPlan>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphPlan>(&format!(
        "UPDATE dependency_graph_plan SET status = 'cancelled', updated_at = now() WHERE id = $1 AND workspace_id = $2 AND status = 'active' RETURNING {PLAN_COLUMNS}"
    ))
    .bind(plan_id)
    .bind(workspace_id)
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
        "INSERT INTO dependency_graph_node (id, plan_id, workspace_id, temp_id, issue_id, title, description, acceptance_criteria, context, outputs, executor_type, executor_id, candidate_executors, owner_type, owner_id, reviewer_type, reviewer_id, runtime_id, model_id, wave) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20) RETURNING {NODE_COLUMNS}"
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
    .bind(node.executor_type.as_deref())
    .bind(node.executor_id)
    .bind(&node.candidate_executors)
    .bind(node.owner_type.as_deref())
    .bind(node.owner_id)
    .bind(node.reviewer_type.as_deref())
    .bind(node.reviewer_id)
    .bind(node.runtime_id)
    .bind(node.model_id.as_deref())
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
    pub executor_type: Option<String>,
    pub executor_id: Option<Uuid>,
    pub candidate_executors: serde_json::Value,
    pub owner_type: Option<String>,
    pub owner_id: Option<Uuid>,
    pub reviewer_type: Option<String>,
    pub reviewer_id: Option<Uuid>,
    pub runtime_id: Option<Uuid>,
    pub model_id: Option<String>,
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

/// Durable publication work created in the same transaction as graph nodes.
/// The handler claims these rows before sending the normal issue:created event
/// and marks them published only after the in-process bus accepts it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DependencyGraphIssueCreatedOutboxRow {
    pub plan_id: Uuid,
    pub node_id: Uuid,
    pub workspace_id: Uuid,
    pub issue_id: Uuid,
}

pub async fn ensure_issue_created_outbox<'e, E>(
    executor: E,
    workspace_id: Uuid,
    plan_id: Uuid,
) -> anyhow::Result<u64>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query(
        r#"INSERT INTO dependency_graph_issue_created_outbox (
    plan_id, node_id, workspace_id, issue_id
)
SELECT node.plan_id, node.id, node.workspace_id, node.issue_id
FROM dependency_graph_node node
WHERE node.plan_id = $1
  AND node.workspace_id = $2
ON CONFLICT (plan_id, node_id) DO NOTHING"#,
    )
    .bind(plan_id)
    .bind(workspace_id)
    .execute(executor)
    .await?
    .rows_affected())
}

pub async fn claim_issue_created_outbox<'e, E>(
    executor: E,
    workspace_id: Uuid,
    plan_id: Uuid,
    lease_owner: &str,
) -> anyhow::Result<Vec<DependencyGraphIssueCreatedOutboxRow>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphIssueCreatedOutboxRow>(
        r#"WITH candidates AS (
    SELECT plan_id, node_id
    FROM dependency_graph_issue_created_outbox
    WHERE workspace_id = $1
      AND plan_id = $2
      AND (
          status = 'pending'
          OR (
              status = 'processing'
              AND (lease_expires_at IS NULL OR lease_expires_at <= now())
          )
      )
    ORDER BY created_at ASC, node_id ASC
    FOR UPDATE SKIP LOCKED
)
UPDATE dependency_graph_issue_created_outbox AS event
SET status = 'processing',
    attempt = event.attempt + 1,
    lease_owner = $3,
    lease_expires_at = now() + interval '30 seconds',
    updated_at = now()
FROM candidates
WHERE event.plan_id = candidates.plan_id
  AND event.node_id = candidates.node_id
RETURNING event.plan_id, event.node_id, event.workspace_id, event.issue_id"#,
    )
    .bind(workspace_id)
    .bind(plan_id)
    .bind(lease_owner)
    .fetch_all(executor)
    .await?)
}

pub async fn mark_issue_created_outbox_published<'e, E>(
    executor: E,
    workspace_id: Uuid,
    plan_id: Uuid,
    node_id: Uuid,
    lease_owner: &str,
) -> anyhow::Result<bool>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query(
        r#"UPDATE dependency_graph_issue_created_outbox
SET status = 'published',
    published_at = now(),
    lease_owner = NULL,
    lease_expires_at = NULL,
    updated_at = now()
WHERE workspace_id = $1
  AND plan_id = $2
  AND node_id = $3
  AND status = 'processing'
  AND lease_owner = $4"#,
    )
    .bind(workspace_id)
    .bind(plan_id)
    .bind(node_id)
    .bind(lease_owner)
    .execute(executor)
    .await?
    .rows_affected()
        == 1)
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

/// Returns the child issue IDs owned by every graph plan affected by deleting
/// one issue. Plan and node rows are locked so the caller can reconcile those
/// children before the graph metadata is removed in the same transaction.
pub async fn list_child_issue_ids_for_issue_delete<'e, E>(
    executor: E,
    workspace_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<Vec<Uuid>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_scalar(
        r#"SELECT node.issue_id
FROM dependency_graph_plan AS plan
JOIN dependency_graph_node AS node
  ON node.plan_id = plan.id
 AND node.workspace_id = plan.workspace_id
WHERE plan.workspace_id = $1
  AND (
      plan.parent_issue_id = $2
      OR node.issue_id = $2
      OR EXISTS (
          SELECT 1
          FROM dependency_graph_edge AS edge
          WHERE edge.plan_id = plan.id
            AND edge.workspace_id = plan.workspace_id
            AND (edge.from_issue_id = $2 OR edge.to_issue_id = $2)
      )
  )
  AND node.issue_id <> $2
FOR UPDATE OF plan, node"#,
    )
    .bind(workspace_id)
    .bind(issue_id)
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

pub async fn list_nodes_for_plans<'e, E>(
    executor: E,
    workspace_id: Uuid,
    plan_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<DependencyGraphNode>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphNode>(&format!(
        "SELECT {NODE_COLUMNS} FROM dependency_graph_node WHERE workspace_id = $1 AND plan_id = ANY($2::uuid[]) ORDER BY plan_id ASC, wave ASC, temp_id ASC"
    ))
    .bind(workspace_id)
    .bind(plan_ids)
    .fetch_all(executor)
    .await?)
}

pub async fn list_edges_for_plans<'e, E>(
    executor: E,
    workspace_id: Uuid,
    plan_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<DependencyGraphEdge>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphEdge>(&format!(
        "SELECT {EDGE_COLUMNS} FROM dependency_graph_edge WHERE workspace_id = $1 AND plan_id = ANY($2::uuid[]) ORDER BY plan_id ASC, from_issue_id ASC, to_issue_id ASC, id ASC"
    ))
    .bind(workspace_id)
    .bind(plan_ids)
    .fetch_all(executor)
    .await?)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DependencyGraphIssueStatus {
    pub issue_id: Uuid,
    pub effective_status: String,
}

pub async fn list_effective_issue_statuses<'e, E>(
    executor: E,
    workspace_id: Uuid,
    issue_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<DependencyGraphIssueStatus>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphIssueStatus>(
        "SELECT id AS issue_id, issue_effective_status(workspace_id, status) AS effective_status FROM issue WHERE workspace_id = $1 AND id = ANY($2::uuid[])",
    )
    .bind(workspace_id)
    .bind(issue_ids)
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

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DependencyGraphGateState {
    pub issue_id: Uuid,
    pub gate_open: bool,
    pub satisfied_prerequisites: i64,
    pub total_prerequisites: i64,
}

/// Reads gate state for all requested issues in one query. The grouped edge
/// counts and the database gate function intentionally share the same active
/// plan and workspace predicates as the single-issue form.
pub async fn get_gate_states<'e, E>(
    executor: E,
    workspace_id: Uuid,
    issue_ids: Vec<Uuid>,
) -> anyhow::Result<Vec<DependencyGraphGateState>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphGateState>(
        r#"WITH requested AS (
    SELECT issue_id
    FROM unnest($2::uuid[]) AS requested(issue_id)
), counts AS (
    SELECT edge.to_issue_id AS issue_id,
           COUNT(*)::bigint AS total_prerequisites,
           COUNT(*) FILTER (
               WHERE prerequisite.id IS NOT NULL
                 AND issue_effective_status(prerequisite.workspace_id, prerequisite.status) = 'done'
           )::bigint AS satisfied_prerequisites
    FROM dependency_graph_edge edge
    JOIN dependency_graph_plan plan
      ON plan.id = edge.plan_id
     AND plan.workspace_id = edge.workspace_id
     AND plan.status = 'active'
    LEFT JOIN issue prerequisite
      ON prerequisite.id = edge.from_issue_id
     AND prerequisite.workspace_id = edge.workspace_id
    WHERE edge.workspace_id = $1
      AND edge.to_issue_id = ANY($2::uuid[])
    GROUP BY edge.to_issue_id
)
SELECT requested.issue_id,
       dependency_graph_issue_gate_open($1, requested.issue_id) AS gate_open,
       COALESCE(counts.satisfied_prerequisites, 0)::bigint AS satisfied_prerequisites,
       COALESCE(counts.total_prerequisites, 0)::bigint AS total_prerequisites
FROM requested
LEFT JOIN counts ON counts.issue_id = requested.issue_id"#,
    )
    .bind(workspace_id)
    .bind(issue_ids)
    .fetch_all(executor)
    .await?)
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
SET status = 'todo',
    revision = revision + 1,
    last_activity_at = GREATEST(COALESCE(last_activity_at, updated_at), now()),
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
SET status = 'todo',
    revision = revision + 1,
    last_activity_at = GREATEST(COALESCE(last_activity_at, updated_at), now()),
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

/// Atomically admits one dependency-graph issue into the execution lane.
/// Graph creation and dependency promotion intentionally leave work in Todo;
/// this is the coordinator's admission boundary.  The gate and status are
/// checked again while the row is updated so a replay cannot start a blocked
/// issue, and only one concurrent coordinator can move a Todo row to
/// In Progress.
pub async fn admit_ready_issue_for_execution<'e, E>(
    executor: E,
    workspace_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<bool>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let row = sqlx::query(
        r#"UPDATE issue
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
RETURNING id"#,
    )
    .bind(issue_id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    Ok(row.is_some())
}

/// Returns the concrete ACP/model target selected by the active planner for an
/// issue.  A graph node may intentionally omit both fields, in which case the
/// normal execution-agent default applies.  The service treats a half-filled
/// pair as invalid rather than silently mixing a planner model with a runtime
/// default.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DependencyGraphExecutionTarget {
    pub runtime_id: Option<Uuid>,
    pub model_id: Option<String>,
}

pub async fn get_execution_target_for_issue<'e, E>(
    executor: E,
    workspace_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<Option<DependencyGraphExecutionTarget>>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    Ok(sqlx::query_as::<_, DependencyGraphExecutionTarget>(
        r#"SELECT node.runtime_id, node.model_id
FROM dependency_graph_node node
JOIN dependency_graph_plan plan
  ON plan.id = node.plan_id
 AND plan.workspace_id = node.workspace_id
 AND plan.status = 'active'
WHERE node.workspace_id = $1
  AND node.issue_id = $2
ORDER BY plan.updated_at DESC, plan.id ASC, node.id ASC
LIMIT 1"#,
    )
    .bind(workspace_id)
    .bind(issue_id)
    .fetch_optional(executor)
    .await?)
}

/// Reopens an issue when admission succeeded but task creation failed before
/// a queue row was committed. The pending-task guard makes this safe when a
/// create actually won a race and the caller only observed a later error.
pub async fn release_admitted_issue_for_execution<'e, E>(
    executor: E,
    workspace_id: Uuid,
    issue_id: Uuid,
) -> anyhow::Result<bool>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let row = sqlx::query(
        r#"UPDATE issue
SET status = 'todo',
    revision = revision + 1,
    last_activity_at = GREATEST(COALESCE(last_activity_at, updated_at), now()),
    updated_at = now()
WHERE id = $1
  AND workspace_id = $2
  AND issue_effective_status(workspace_id, status) = 'in_progress'
  AND NOT EXISTS (
      SELECT 1 FROM agent_task_queue pending
      WHERE pending.issue_id = issue.id
        AND pending.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'waiting_capacity', 'deferred')
  )
RETURNING id"#,
    )
    .bind(issue_id)
    .bind(workspace_id)
    .fetch_optional(executor)
    .await?;
    Ok(row.is_some())
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

pub async fn validate_executor<'e, E>(
    executor: E,
    workspace_id: Uuid,
    executor_type: &str,
    executor_id: Uuid,
) -> anyhow::Result<bool>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let exists = match executor_type {
        "member" => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM member WHERE user_id = $1 AND workspace_id = $2)",
        )
        .bind(executor_id)
        .bind(workspace_id)
        .fetch_one(executor)
        .await?,
        "agent" => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM agent WHERE id = $1 AND workspace_id = $2 AND archived_at IS NULL)",
        )
        .bind(executor_id)
        .bind(workspace_id)
        .fetch_one(executor)
        .await?,
        "team" => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM team WHERE id = $1 AND workspace_id = $2 AND archived_at IS NULL)",
        )
        .bind(executor_id)
        .bind(workspace_id)
        .fetch_one(executor)
        .await?,
        _ => false,
    };
    Ok(exists)
}
