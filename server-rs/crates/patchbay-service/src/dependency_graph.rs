//! Dependency-aware planning and graph persistence.
//!
//! A planner only proposes a typed plan. This module is the authoritative
//! boundary that validates the proposal, allocates child issues, persists all
//! nodes and edges in one transaction, and derives readiness from persisted
//! issue state. No caller may create assigned children and add dependency
//! edges in separate operations.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use patchbay_db::dbid::new_v7;
use patchbay_db::models::{DependencyGraphEdge, DependencyGraphNode, DependencyGraphPlan, Issue};
use patchbay_db::queries::workspace::increment_issue_counter;
use patchbay_db::queries::{dependency_graph as graph_q, issue as issue_q};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Executor, PgPool};
use uuid::Uuid;

use crate::issue_position::next_top_position;
use crate::issue_status;

pub const HARD_DEPENDENCY_TYPE: &str = "hard";
const MAX_TASKS: usize = 128;
const MAX_EDGES: usize = 512;
const MAX_GOAL_LENGTH: usize = 8_000;
const MAX_TEMP_ID_LENGTH: usize = 64;
const MAX_TITLE_LENGTH: usize = 500;
const MAX_DESCRIPTION_LENGTH: usize = 30_000;
const MAX_TEXT_ITEM_LENGTH: usize = 2_000;
const MAX_EDGE_REASON_LENGTH: usize = 4_000;

fn empty_object() -> Value {
    json!({})
}

/// A planner-selected assignee or candidate. The discriminator is deliberately
/// the same as the issue API (`member`, `agent`, or `team`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanAssignee {
    #[serde(rename = "type")]
    pub type_: String,
    pub id: Uuid,
}

/// One independently verifiable task proposed by a planner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DependencyGraphTaskInput {
    pub temp_id: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default = "empty_object")]
    pub context: Value,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub assignee: Option<PlanAssignee>,
    #[serde(default)]
    pub candidate_assignees: Vec<PlanAssignee>,
}

/// Typed plan submitted by a planner. `waves` is intentionally absent: the
/// server derives it from edges and never trusts a client-supplied topology.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DependencyGraphPlanInput {
    pub goal: String,
    pub parent_issue_id: Uuid,
    pub tasks: Vec<DependencyGraphTaskInput>,
    #[serde(default)]
    pub edges: Vec<DependencyGraphEdgeInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyGraphEdgeInput {
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub reason: String,
    /// Exact output of `from` that `to` consumes. Keeping this separate from
    /// prose makes the dependency auditable and prevents accidental edges
    /// whose reason only says that tasks are in the same area.
    pub consumed_output: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DependencyGraphError {
    #[error("invalid dependency graph plan: {0}")]
    Validation(String),
    #[error("parent issue was not found in this workspace")]
    ParentNotFound,
    #[error("an active dependency graph already exists for this parent issue")]
    ActivePlanExists,
    #[error("idempotency key was already used for a different dependency graph plan")]
    IdempotencyConflict,
    #[error(
        "assignee {assignee_type}:{assignee_id} was not found or is unavailable in this workspace"
    )]
    AssigneeNotFound {
        assignee_type: String,
        assignee_id: Uuid,
    },
    #[error("dependency graph {0} was not found")]
    NotFound(Uuid),
    #[error("dependency graph data is inconsistent: {0}")]
    Integrity(String),
    #[error("dependency graph database error: {0}")]
    Database(String),
}

fn db_error(error: impl std::fmt::Display) -> DependencyGraphError {
    DependencyGraphError::Database(error.to_string())
}

fn invalid(message: impl Into<String>) -> DependencyGraphError {
    DependencyGraphError::Validation(message.into())
}

fn validate_text(
    value: &str,
    field: &str,
    max_length: usize,
    required: bool,
) -> Result<(), DependencyGraphError> {
    let trimmed = value.trim();
    if required && trimmed.is_empty() {
        return Err(invalid(format!("{field} is required")));
    }
    if value.chars().count() > max_length {
        return Err(invalid(format!("{field} exceeds {max_length} characters")));
    }
    if value != trimmed {
        return Err(invalid(format!(
            "{field} must not have surrounding whitespace"
        )));
    }
    Ok(())
}

fn validate_text_list(values: &[String], field: &str) -> Result<(), DependencyGraphError> {
    for (index, value) in values.iter().enumerate() {
        validate_text(
            value,
            &format!("{field}[{index}]"),
            MAX_TEXT_ITEM_LENGTH,
            true,
        )?;
    }
    Ok(())
}

fn validate_assignee_shape(
    assignee: &PlanAssignee,
    field: &str,
) -> Result<(), DependencyGraphError> {
    if assignee.id.is_nil() {
        return Err(invalid(format!("{field}.id must be a non-nil UUID")));
    }
    if !matches!(assignee.type_.as_str(), "member" | "agent" | "team") {
        return Err(invalid(format!(
            "{field}.type must be member, agent, or team"
        )));
    }
    Ok(())
}

fn has_path(
    adjacency: &[Vec<(usize, usize)>],
    source: usize,
    target: usize,
    ignored_edge: usize,
) -> bool {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::from([source]);
    while let Some(current) = queue.pop_front() {
        if current == target {
            return true;
        }
        if !visited.insert(current) {
            continue;
        }
        for (next, edge_index) in &adjacency[current] {
            if *edge_index != ignored_edge {
                queue.push_back(*next);
            }
        }
    }
    false
}

/// Validates a planner proposal and returns server-derived topological waves.
/// The returned waves are only a display/scheduling projection; persisted
/// edges remain the source of truth.
pub fn validate_dependency_plan(
    input: &DependencyGraphPlanInput,
) -> Result<Vec<Vec<String>>, DependencyGraphError> {
    validate_text(&input.goal, "goal", MAX_GOAL_LENGTH, true)?;
    if input.parent_issue_id.is_nil() {
        return Err(invalid("parent_issue_id must be a non-nil UUID"));
    }
    if input.tasks.is_empty() {
        return Err(invalid("tasks must contain at least one task"));
    }
    if input.tasks.len() > MAX_TASKS {
        return Err(invalid(format!("tasks cannot exceed {MAX_TASKS} entries")));
    }
    if input.edges.len() > MAX_EDGES {
        return Err(invalid(format!("edges cannot exceed {MAX_EDGES} entries")));
    }

    let mut task_indexes = HashMap::with_capacity(input.tasks.len());
    for (index, task) in input.tasks.iter().enumerate() {
        validate_text(
            &task.temp_id,
            &format!("tasks[{index}].temp_id"),
            MAX_TEMP_ID_LENGTH,
            true,
        )?;
        if task_indexes.insert(task.temp_id.as_str(), index).is_some() {
            return Err(invalid(format!(
                "duplicate task temp_id {:?}",
                task.temp_id
            )));
        }
        validate_text(
            &task.title,
            &format!("tasks[{index}].title"),
            MAX_TITLE_LENGTH,
            true,
        )?;
        validate_text(
            &task.description,
            &format!("tasks[{index}].description"),
            MAX_DESCRIPTION_LENGTH,
            false,
        )?;
        if task.acceptance_criteria.is_empty() {
            return Err(invalid(format!(
                "tasks[{index}].acceptance_criteria must contain at least one criterion"
            )));
        }
        validate_text_list(
            &task.acceptance_criteria,
            &format!("tasks[{index}].acceptance_criteria"),
        )?;
        if task.outputs.is_empty() {
            return Err(invalid(format!(
                "tasks[{index}].outputs must contain at least one observable output"
            )));
        }
        validate_text_list(&task.outputs, &format!("tasks[{index}].outputs"))?;
        if !task.context.is_object() {
            return Err(invalid(format!(
                "tasks[{index}].context must be a JSON object"
            )));
        }
        let mut outputs = HashSet::new();
        for output in &task.outputs {
            if !outputs.insert(output.as_str()) {
                return Err(invalid(format!(
                    "tasks[{index}].outputs contains duplicate output {:?}",
                    output
                )));
            }
        }
        if let Some(assignee) = &task.assignee {
            validate_assignee_shape(assignee, &format!("tasks[{index}].assignee"))?;
        }
        for (candidate_index, candidate) in task.candidate_assignees.iter().enumerate() {
            validate_assignee_shape(
                candidate,
                &format!("tasks[{index}].candidate_assignees[{candidate_index}]"),
            )?;
        }
    }

    let mut adjacency = vec![Vec::<(usize, usize)>::new(); input.tasks.len()];
    let mut indegree = vec![0usize; input.tasks.len()];
    let mut edge_pairs = BTreeSet::new();
    for (edge_index, edge) in input.edges.iter().enumerate() {
        validate_text(
            &edge.from,
            &format!("edges[{edge_index}].from"),
            MAX_TEMP_ID_LENGTH,
            true,
        )?;
        validate_text(
            &edge.to,
            &format!("edges[{edge_index}].to"),
            MAX_TEMP_ID_LENGTH,
            true,
        )?;
        if edge.type_ != HARD_DEPENDENCY_TYPE {
            return Err(invalid(format!(
                "edges[{edge_index}].type must be hard in V1"
            )));
        }
        validate_text(
            &edge.reason,
            &format!("edges[{edge_index}].reason"),
            MAX_EDGE_REASON_LENGTH,
            true,
        )?;
        validate_text(
            &edge.consumed_output,
            &format!("edges[{edge_index}].consumed_output"),
            MAX_TEXT_ITEM_LENGTH,
            true,
        )?;
        let Some(&from) = task_indexes.get(edge.from.as_str()) else {
            return Err(invalid(format!(
                "edges[{edge_index}].from references unknown task {:?}",
                edge.from
            )));
        };
        let Some(&to) = task_indexes.get(edge.to.as_str()) else {
            return Err(invalid(format!(
                "edges[{edge_index}].to references unknown task {:?}",
                edge.to
            )));
        };
        if from == to {
            return Err(invalid(format!(
                "edges[{edge_index}] cannot be a self dependency"
            )));
        }
        if !edge_pairs.insert((from, to)) {
            return Err(invalid(format!(
                "duplicate dependency edge {} -> {}",
                edge.from, edge.to
            )));
        }
        if !input.tasks[from]
            .outputs
            .iter()
            .any(|output| output == &edge.consumed_output)
        {
            return Err(invalid(format!(
                "edges[{edge_index}].consumed_output {:?} is not an output of {}",
                edge.consumed_output, edge.from
            )));
        }
        adjacency[from].push((to, edge_index));
        indegree[to] += 1;
    }

    // Reject transitive edges instead of storing a graph whose explanation
    // claims a direct dependency that is already implied by another path.
    for (edge_index, edge) in input.edges.iter().enumerate() {
        let from = task_indexes[edge.from.as_str()];
        let to = task_indexes[edge.to.as_str()];
        if has_path(&adjacency, from, to, edge_index) {
            return Err(invalid(format!(
                "dependency edge {} -> {} is transitively redundant",
                edge.from, edge.to
            )));
        }
    }

    // Kahn's algorithm both detects cycles and derives deterministic waves.
    let mut current = (0..input.tasks.len())
        .filter(|index| indegree[*index] == 0)
        .collect::<Vec<_>>();
    let mut visited = 0usize;
    let mut waves = Vec::new();
    while !current.is_empty() {
        let mut wave = Vec::with_capacity(current.len());
        let mut next = Vec::new();
        for index in current {
            visited += 1;
            wave.push(input.tasks[index].temp_id.clone());
            for (dependent, _) in &adjacency[index] {
                indegree[*dependent] -= 1;
                if indegree[*dependent] == 0 {
                    next.push(*dependent);
                }
            }
        }
        waves.push(wave);
        current = next;
    }
    if visited != input.tasks.len() {
        return Err(invalid("dependency graph contains a cycle"));
    }
    Ok(waves)
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        Value::Object(values) => {
            let ordered = values
                .into_iter()
                .map(|(key, value)| (key, canonicalize(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(ordered.into_iter().collect())
        }
        other => other,
    }
}

/// Returns the idempotency hash stored with a plan. Hashing the typed payload
/// (with object keys canonicalized) prevents a retry from creating a second
/// graph while still rejecting a reused key for a different proposal.
pub fn plan_request_hash(input: &DependencyGraphPlanInput) -> String {
    let value = serde_json::to_value(input).expect("dependency plan is serializable");
    let canonical =
        serde_json::to_vec(&canonicalize(value)).expect("canonical plan is serializable");
    format!("sha256:{}", hex::encode(Sha256::digest(canonical)))
}

#[derive(Debug, Clone)]
pub struct DependencyGraphSnapshot {
    pub plan: DependencyGraphPlan,
    pub parent: Issue,
    pub parent_effective_status: String,
    pub nodes: Vec<DependencyGraphNodeSnapshot>,
    pub edges: Vec<DependencyGraphEdgeSnapshot>,
    pub waves: Vec<Vec<String>>,
    pub readiness: DependencyGraphReadiness,
    /// True only for the first successful application, not an idempotent
    /// replay or a read-only graph load. The handler uses this to avoid
    /// duplicating issue-created events on retries.
    pub newly_created: bool,
}

#[derive(Debug, Clone)]
pub struct DependencyGraphPage {
    pub snapshots: Vec<DependencyGraphSnapshot>,
    pub next_cursor: Option<(DateTime<Utc>, Uuid)>,
}

#[derive(Debug, Clone)]
pub struct DependencyGraphNodeSnapshot {
    pub node: DependencyGraphNode,
    pub issue: Issue,
    pub effective_status: String,
    pub readiness: DependencyNodeReadiness,
}

#[derive(Debug, Clone)]
pub struct DependencyGraphEdgeSnapshot {
    pub edge: DependencyGraphEdge,
    pub from_temp_id: String,
    pub to_temp_id: String,
    pub prerequisite_status: String,
    pub satisfied: bool,
    pub satisfied_prerequisites: i64,
    pub total_prerequisites: i64,
    pub unlock_condition: String,
}

#[derive(Debug, Clone)]
pub struct DependencyNodeReadiness {
    pub state: String,
    pub gate_open: bool,
    pub satisfied_prerequisites: i64,
    pub total_prerequisites: i64,
    pub unlock_condition: String,
}

#[derive(Debug, Clone, Default)]
pub struct DependencyGraphReadiness {
    pub total: i64,
    pub ready: i64,
    pub running: i64,
    pub blocked: i64,
    pub done: i64,
    pub cancelled: i64,
}

fn unlock_condition(satisfied: i64, total: i64) -> String {
    if total == 0 {
        "No hard prerequisites; ready immediately".to_string()
    } else {
        format!(
            "All {total} hard prerequisites must be Done ({satisfied}/{total} currently satisfied)"
        )
    }
}

fn node_state(status: &str, gate_open: bool) -> String {
    if status == issue_status::DONE {
        "done".to_string()
    } else if status == issue_status::CANCELLED {
        "cancelled".to_string()
    } else if !gate_open || status == issue_status::BLOCKED {
        "blocked".to_string()
    } else if matches!(status, issue_status::IN_PROGRESS | issue_status::IN_REVIEW) {
        "running".to_string()
    } else if status == issue_status::TODO {
        "ready".to_string()
    } else {
        // Unknown/backlog states are not admitted by the scheduler. Report
        // them as blocked so the Graph surface cannot present a non-admitted
        // task as a runnable "todo" state when the dependency gate is open.
        "blocked".to_string()
    }
}

async fn load_graph(
    pool: &PgPool,
    plan: DependencyGraphPlan,
) -> Result<DependencyGraphSnapshot, DependencyGraphError> {
    let mut snapshots = load_graphs(pool, vec![plan]).await?;
    snapshots.pop().ok_or_else(|| {
        DependencyGraphError::Integrity("dependency graph loader returned no snapshot".to_string())
    })
}

/// Loads all graph material for a page with bounded batch queries. Keeping the
/// issue, effective-status, edge, and gate reads outside the per-plan loop
/// prevents a 64-plan list from turning into a serial N+1 query storm.
async fn load_graphs(
    pool: &PgPool,
    plans: Vec<DependencyGraphPlan>,
) -> Result<Vec<DependencyGraphSnapshot>, DependencyGraphError> {
    if plans.is_empty() {
        return Ok(Vec::new());
    }
    let plan_ids = plans.iter().map(|plan| plan.id).collect::<Vec<_>>();
    let nodes = graph_q::list_nodes_for_plans(pool, plans[0].workspace_id, plan_ids.clone())
        .await
        .map_err(db_error)?;
    let edges = graph_q::list_edges_for_plans(pool, plans[0].workspace_id, plan_ids)
        .await
        .map_err(db_error)?;

    let mut issue_ids = plans
        .iter()
        .map(|plan| plan.parent_issue_id)
        .chain(nodes.iter().map(|node| node.issue_id))
        .collect::<Vec<_>>();
    issue_ids.sort_unstable();
    issue_ids.dedup();
    let issue_by_id =
        issue_q::list_issues_in_workspace_by_ids(pool, plans[0].workspace_id, issue_ids.clone())
            .await
            .map_err(db_error)?
            .into_iter()
            .map(|issue| (issue.id, issue))
            .collect::<HashMap<_, _>>();
    let status_by_issue =
        graph_q::list_effective_issue_statuses(pool, plans[0].workspace_id, issue_ids.clone())
            .await
            .map_err(db_error)?
            .into_iter()
            .map(|status| (status.issue_id, status.effective_status))
            .collect::<HashMap<_, _>>();
    let mut node_issue_ids = nodes.iter().map(|node| node.issue_id).collect::<Vec<_>>();
    node_issue_ids.sort_unstable();
    node_issue_ids.dedup();
    let gate_by_issue = graph_q::get_gate_states(pool, plans[0].workspace_id, node_issue_ids)
        .await
        .map_err(db_error)?
        .into_iter()
        .map(|gate| (gate.issue_id, gate))
        .collect::<HashMap<_, _>>();

    plans
        .into_iter()
        .map(|plan| {
            let plan_nodes = nodes
                .iter()
                .filter(|node| node.plan_id == plan.id)
                .cloned()
                .collect::<Vec<_>>();
            let plan_edges = edges
                .iter()
                .filter(|edge| edge.plan_id == plan.id)
                .cloned()
                .collect::<Vec<_>>();
            build_graph_snapshot(
                plan,
                plan_nodes,
                plan_edges,
                &issue_by_id,
                &status_by_issue,
                &gate_by_issue,
            )
        })
        .collect()
}

fn build_graph_snapshot(
    plan: DependencyGraphPlan,
    nodes: Vec<DependencyGraphNode>,
    edges: Vec<DependencyGraphEdge>,
    issue_by_id: &HashMap<Uuid, Issue>,
    status_by_issue: &HashMap<Uuid, String>,
    gate_by_issue: &HashMap<Uuid, graph_q::DependencyGraphGateState>,
) -> Result<DependencyGraphSnapshot, DependencyGraphError> {
    let parent = issue_by_id
        .get(&plan.parent_issue_id)
        .cloned()
        .ok_or_else(|| {
            DependencyGraphError::Integrity(format!(
                "parent issue {} is missing",
                plan.parent_issue_id
            ))
        })?;
    let parent_effective_status = status_by_issue
        .get(&plan.parent_issue_id)
        .cloned()
        .ok_or_else(|| {
            DependencyGraphError::Integrity(format!(
                "parent issue {} has no effective status",
                plan.parent_issue_id
            ))
        })?;
    if nodes.is_empty() {
        return Err(DependencyGraphError::Integrity(
            "a dependency graph plan has no nodes".to_string(),
        ));
    }

    let mut node_by_issue = HashMap::with_capacity(nodes.len());
    let mut seen_issue_ids = HashSet::with_capacity(nodes.len());
    let mut seen_temp_ids = HashSet::with_capacity(nodes.len());
    for node in nodes {
        if !seen_issue_ids.insert(node.issue_id) {
            return Err(DependencyGraphError::Integrity(format!(
                "graph contains duplicate issue node {}",
                node.issue_id
            )));
        }
        if !seen_temp_ids.insert(node.temp_id.clone()) {
            return Err(DependencyGraphError::Integrity(format!(
                "graph contains duplicate temp_id {:?}",
                node.temp_id
            )));
        }
        if !issue_by_id.contains_key(&node.issue_id) {
            return Err(DependencyGraphError::Integrity(format!(
                "graph node {} points to missing issue {}",
                node.temp_id, node.issue_id
            )));
        }
        node_by_issue.insert(node.issue_id, node);
    }

    let mut node_snapshots = Vec::with_capacity(node_by_issue.len());
    let mut readiness = DependencyGraphReadiness {
        total: node_by_issue.len() as i64,
        ..Default::default()
    };
    for (issue_id, node) in &node_by_issue {
        let issue = issue_by_id
            .get(issue_id)
            .expect("node issue was checked before snapshot construction");
        let status = status_by_issue.get(issue_id).ok_or_else(|| {
            DependencyGraphError::Integrity(format!(
                "graph node {} has no effective status",
                node.temp_id
            ))
        })?;
        let gate = gate_by_issue.get(issue_id).ok_or_else(|| {
            DependencyGraphError::Integrity(format!(
                "graph node {} has no dependency gate state",
                node.temp_id
            ))
        })?;
        let state = node_state(status, gate.gate_open);
        match state.as_str() {
            "ready" => readiness.ready += 1,
            "running" => readiness.running += 1,
            "blocked" => readiness.blocked += 1,
            "done" => readiness.done += 1,
            "cancelled" => readiness.cancelled += 1,
            _ => {}
        }
        node_snapshots.push(DependencyGraphNodeSnapshot {
            node: node.clone(),
            issue: issue.clone(),
            effective_status: status.clone(),
            readiness: DependencyNodeReadiness {
                state,
                gate_open: gate.gate_open,
                satisfied_prerequisites: gate.satisfied_prerequisites,
                total_prerequisites: gate.total_prerequisites,
                unlock_condition: unlock_condition(
                    gate.satisfied_prerequisites,
                    gate.total_prerequisites,
                ),
            },
        });
    }
    node_snapshots.sort_by(|left, right| left.node.temp_id.cmp(&right.node.temp_id));

    let temp_id_by_issue = node_by_issue
        .iter()
        .map(|(issue_id, node)| (*issue_id, node.temp_id.clone()))
        .collect::<HashMap<_, _>>();
    let readiness_by_issue = node_snapshots
        .iter()
        .map(|node| (node.issue.id, node.readiness.clone()))
        .collect::<HashMap<_, _>>();
    let edge_snapshots = edges
        .into_iter()
        .map(|edge| {
            let prerequisite_status = status_by_issue
                .get(&edge.from_issue_id)
                .cloned()
                .ok_or_else(|| {
                    DependencyGraphError::Integrity(format!(
                        "edge {} points to missing source node",
                        edge.id
                    ))
                })?;
            let target = readiness_by_issue.get(&edge.to_issue_id).ok_or_else(|| {
                DependencyGraphError::Integrity(format!(
                    "edge {} points to missing target node",
                    edge.id
                ))
            })?;
            Ok(DependencyGraphEdgeSnapshot {
                from_temp_id: temp_id_by_issue
                    .get(&edge.from_issue_id)
                    .cloned()
                    .ok_or_else(|| {
                        DependencyGraphError::Integrity(format!(
                            "edge {} source temp_id is missing",
                            edge.id
                        ))
                    })?,
                to_temp_id: temp_id_by_issue
                    .get(&edge.to_issue_id)
                    .cloned()
                    .ok_or_else(|| {
                        DependencyGraphError::Integrity(format!(
                            "edge {} target temp_id is missing",
                            edge.id
                        ))
                    })?,
                satisfied: prerequisite_status == issue_status::DONE,
                prerequisite_status,
                satisfied_prerequisites: target.satisfied_prerequisites,
                total_prerequisites: target.total_prerequisites,
                unlock_condition: target.unlock_condition.clone(),
                edge,
            })
        })
        .collect::<Result<Vec<_>, DependencyGraphError>>()?;

    let waves = derive_persisted_waves(&node_snapshots, &edge_snapshots)?;
    Ok(DependencyGraphSnapshot {
        plan,
        parent,
        parent_effective_status,
        nodes: node_snapshots,
        edges: edge_snapshots,
        waves,
        readiness,
        newly_created: false,
    })
}

fn derive_persisted_waves(
    nodes: &[DependencyGraphNodeSnapshot],
    edges: &[DependencyGraphEdgeSnapshot],
) -> Result<Vec<Vec<String>>, DependencyGraphError> {
    for node in nodes {
        if node.node.wave < 0 || node.node.wave as usize >= nodes.len() {
            return Err(DependencyGraphError::Integrity(format!(
                "node {:?} has invalid persisted wave {}",
                node.node.temp_id, node.node.wave
            )));
        }
    }
    let mut wave_by_temp_id = HashMap::with_capacity(nodes.len());
    for node in nodes {
        wave_by_temp_id.insert(node.node.temp_id.clone(), node.node.wave);
    }
    let max_wave = nodes.iter().map(|node| node.node.wave).max().unwrap_or(0);
    let mut waves = vec![Vec::new(); max_wave as usize + 1];
    for node in nodes {
        waves[node.node.wave as usize].push(node.node.temp_id.clone());
    }
    for wave in &mut waves {
        wave.sort();
    }
    for edge in edges {
        let from_wave = wave_by_temp_id.get(&edge.from_temp_id).copied();
        let to_wave = wave_by_temp_id.get(&edge.to_temp_id).copied();
        if !matches!(from_wave.zip(to_wave), Some((from, to)) if from < to) {
            return Err(DependencyGraphError::Integrity(format!(
                "edge {} does not point from an earlier wave",
                edge.edge.id
            )));
        }
    }
    Ok(waves)
}

pub async fn load_dependency_graph(
    pool: &PgPool,
    workspace_id: Uuid,
    plan_id: Uuid,
) -> Result<DependencyGraphSnapshot, DependencyGraphError> {
    let plan = graph_q::get_plan_by_id(pool, plan_id, workspace_id)
        .await
        .map_err(db_error)?
        .ok_or(DependencyGraphError::NotFound(plan_id))?;
    load_graph(pool, plan).await
}

pub async fn load_active_dependency_graph(
    pool: &PgPool,
    workspace_id: Uuid,
    parent_issue_id: Uuid,
) -> Result<DependencyGraphSnapshot, DependencyGraphError> {
    let plan = graph_q::get_active_plan_for_parent(pool, workspace_id, parent_issue_id, false)
        .await
        .map_err(db_error)?
        .ok_or(DependencyGraphError::NotFound(parent_issue_id))?;
    load_graph(pool, plan).await
}

/// Loads the graph relevant to one issue detail. A graph task can be reached
/// through its node as well as through the planner parent route; the query
/// prefers node ownership so the detail explains the task's own gate.
pub async fn load_active_dependency_graph_for_issue(
    pool: &PgPool,
    workspace_id: Uuid,
    issue_id: Uuid,
) -> Result<DependencyGraphSnapshot, DependencyGraphError> {
    let plan = graph_q::get_active_plan_for_issue(pool, workspace_id, issue_id)
        .await
        .map_err(db_error)?
        .ok_or(DependencyGraphError::NotFound(issue_id))?;
    load_graph(pool, plan).await
}

pub async fn load_active_dependency_graphs(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<DependencyGraphSnapshot>, DependencyGraphError> {
    Ok(
        load_active_dependency_graphs_after(pool, workspace_id, project_id, limit, None)
            .await?
            .snapshots,
    )
}

pub async fn load_active_dependency_graphs_after(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    limit: i64,
    after: Option<(DateTime<Utc>, Uuid)>,
) -> Result<DependencyGraphPage, DependencyGraphError> {
    let limit = limit.clamp(1, 64);
    let mut plans = graph_q::list_active_plans(pool, workspace_id, project_id, limit + 1, after)
        .await
        .map_err(db_error)?;
    let has_next_page = plans.len() > limit as usize;
    if has_next_page {
        plans.truncate(limit as usize);
    }
    let next_cursor = if has_next_page {
        plans.last().map(|plan| (plan.updated_at, plan.id))
    } else {
        None
    };
    let snapshots = load_graphs(pool, plans).await?;
    Ok(DependencyGraphPage {
        snapshots,
        next_cursor,
    })
}

pub async fn retire_dependency_plan(
    pool: &PgPool,
    workspace_id: Uuid,
    plan_id: Uuid,
) -> Result<DependencyGraphPlan, DependencyGraphError> {
    graph_q::retire_active_plan(pool, workspace_id, plan_id)
        .await
        .map_err(db_error)?
        .ok_or(DependencyGraphError::NotFound(plan_id))
}

async fn validate_persisted_assignee<'e, E>(
    executor: E,
    workspace_id: Uuid,
    assignee: &PlanAssignee,
) -> Result<(), DependencyGraphError>
where
    E: Executor<'e, Database = sqlx::Postgres>,
{
    let found = graph_q::validate_assignee(executor, workspace_id, &assignee.type_, assignee.id)
        .await
        .map_err(db_error)?;
    if found {
        Ok(())
    } else {
        Err(DependencyGraphError::AssigneeNotFound {
            assignee_type: assignee.type_.clone(),
            assignee_id: assignee.id,
        })
    }
}

fn unique_constraint(error: &anyhow::Error) -> Option<&str> {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(|error| error.as_database_error())
        .and_then(|error| error.constraint())
}

/// Atomically validates and applies a complete planner proposal.
///
/// The parent row is locked before checking the active-plan invariant. That
/// serializes competing plans for one parent while the idempotency index
/// handles retries that arrive through different parent routes. Child issues
/// are deliberately not enqueued here; PR2's readiness gate is the only
/// admission path and will enqueue roots/newly-unlocked nodes after this
/// transaction commits.
pub async fn apply_dependency_plan(
    pool: &PgPool,
    workspace_id: Uuid,
    input: &DependencyGraphPlanInput,
    idempotency_key: &str,
    created_by_type: &str,
    created_by_id: Uuid,
) -> Result<DependencyGraphSnapshot, DependencyGraphError> {
    let waves = validate_dependency_plan(input)?;
    let idempotency_key = idempotency_key.trim();
    if idempotency_key.is_empty() {
        return Err(invalid("idempotency_key is required"));
    }
    if idempotency_key.chars().count() > 255 {
        return Err(invalid("idempotency_key exceeds 255 characters"));
    }
    if !matches!(created_by_type, "member" | "agent" | "system") || created_by_id.is_nil() {
        return Err(invalid(
            "created_by must be a non-nil member, agent, or system identity",
        ));
    }
    let request_hash = plan_request_hash(input);
    let mut tx = pool.begin().await.map_err(db_error)?;

    if let Some(existing) =
        graph_q::get_plan_by_idempotency(&mut *tx, workspace_id, idempotency_key, true)
            .await
            .map_err(db_error)?
    {
        if existing.request_hash != request_hash {
            return Err(DependencyGraphError::IdempotencyConflict);
        }
        tx.commit().await.map_err(db_error)?;
        return load_graph(pool, existing).await;
    }

    // The authenticated workspace is authoritative. Locking the parent row
    // also makes active-plan checks race-free for competing plans.
    let parent = graph_q::lock_parent_issue(&mut *tx, input.parent_issue_id, workspace_id)
        .await
        .map_err(db_error)?;
    let Some(parent) = parent else {
        return Err(DependencyGraphError::ParentNotFound);
    };

    // The first lookup avoids unnecessary locking for the common retry case.
    // Re-check after locking the parent: a concurrent identical apply may have
    // inserted the idempotency row while this transaction waited for the
    // parent lock. Without this second lookup it would be misreported as an
    // active-plan conflict instead of replaying the original result.
    if let Some(existing) =
        graph_q::get_plan_by_idempotency(&mut *tx, workspace_id, idempotency_key, true)
            .await
            .map_err(db_error)?
    {
        if existing.request_hash != request_hash {
            return Err(DependencyGraphError::IdempotencyConflict);
        }
        tx.commit().await.map_err(db_error)?;
        return load_graph(pool, existing).await;
    }

    if graph_q::get_active_plan_for_parent(&mut *tx, workspace_id, input.parent_issue_id, true)
        .await
        .map_err(db_error)?
        .is_some()
    {
        return Err(DependencyGraphError::ActivePlanExists);
    }

    for task in &input.tasks {
        if let Some(assignee) = &task.assignee {
            validate_persisted_assignee(&mut *tx, workspace_id, assignee).await?;
        }
        for candidate in &task.candidate_assignees {
            validate_persisted_assignee(&mut *tx, workspace_id, candidate).await?;
        }
    }

    if created_by_type == "agent" {
        validate_persisted_assignee(
            &mut *tx,
            workspace_id,
            &PlanAssignee {
                type_: "agent".to_string(),
                id: created_by_id,
            },
        )
        .await?;
    }

    let plan = graph_q::insert_plan(
        &mut *tx,
        &graph_q::DependencyGraphPlanInsert {
            id: new_v7(),
            workspace_id,
            parent_issue_id: input.parent_issue_id,
            idempotency_key: idempotency_key.to_string(),
            request_hash,
            goal: input.goal.clone(),
            created_by_type: created_by_type.to_string(),
            created_by_id,
        },
    )
    .await
    .map_err(|error| match unique_constraint(&error) {
        Some("uq_dependency_graph_plan_active_parent") => DependencyGraphError::ActivePlanExists,
        _ => db_error(error),
    })?;
    let Some(plan) = plan else {
        let existing =
            graph_q::get_plan_by_idempotency(&mut *tx, workspace_id, idempotency_key, true)
                .await
                .map_err(db_error)?
                .ok_or_else(|| {
                    DependencyGraphError::Database(
                        "idempotency conflict did not return the existing plan".to_string(),
                    )
                })?;
        if existing.request_hash != plan_request_hash(input) {
            return Err(DependencyGraphError::IdempotencyConflict);
        }
        tx.commit().await.map_err(db_error)?;
        return load_graph(pool, existing).await;
    };

    let mut issue_by_temp_id = HashMap::with_capacity(input.tasks.len());
    let wave_by_temp_id = waves
        .iter()
        .enumerate()
        .flat_map(|(wave, task_ids)| task_ids.iter().map(move |temp_id| (temp_id.as_str(), wave)))
        .collect::<HashMap<_, _>>();

    for task in &input.tasks {
        let incoming_count = input
            .edges
            .iter()
            .filter(|edge| edge.to == task.temp_id)
            .count();
        let status = if incoming_count == 0 {
            issue_status::TODO
        } else {
            issue_status::BLOCKED
        };
        let issue_number = increment_issue_counter(&mut *tx, workspace_id)
            .await
            .map_err(db_error)?
            .ok_or_else(|| {
                DependencyGraphError::Database("workspace counter row is missing".to_string())
            })?;
        let position = next_top_position(&mut *tx, workspace_id, status)
            .await
            .map_err(db_error)?;
        let assignee_type = task
            .assignee
            .as_ref()
            .map(|assignee| assignee.type_.as_str());
        let assignee_id = task.assignee.as_ref().map(|assignee| assignee.id);
        let issue = issue_q::create_issue(
            &mut *tx,
            workspace_id,
            &task.title,
            Some(&task.description),
            status,
            "none",
            assignee_type,
            assignee_id,
            if created_by_type == "agent" {
                "agent"
            } else {
                "member"
            },
            created_by_id,
            Some(input.parent_issue_id),
            position,
            None,
            None,
            issue_number,
            parent.project_id,
            None,
            new_v7(),
        )
        .await
        .map_err(db_error)?
        .ok_or_else(|| {
            DependencyGraphError::Database("planned issue insert returned no row".to_string())
        })?;
        let acceptance_criteria = json!(task.acceptance_criteria);
        graph_q::set_issue_acceptance_criteria(
            &mut *tx,
            issue.id,
            workspace_id,
            &acceptance_criteria,
        )
        .await
        .map_err(db_error)?;
        let _node = graph_q::insert_node(
            &mut *tx,
            &graph_q::DependencyGraphNodeInsert {
                id: new_v7(),
                plan_id: plan.id,
                workspace_id,
                temp_id: task.temp_id.clone(),
                issue_id: issue.id,
                title: task.title.clone(),
                description: task.description.clone(),
                acceptance_criteria,
                context: task.context.clone(),
                outputs: json!(task.outputs),
                assignee_type: task
                    .assignee
                    .as_ref()
                    .map(|assignee| assignee.type_.clone()),
                assignee_id,
                candidate_assignees: json!(task.candidate_assignees),
                wave: *wave_by_temp_id
                    .get(task.temp_id.as_str())
                    .expect("validated task appears in a derived wave")
                    as i32,
            },
        )
        .await
        .map_err(db_error)?;
        issue_by_temp_id.insert(task.temp_id.clone(), issue.id);
    }

    for edge in &input.edges {
        let from_issue_id = *issue_by_temp_id
            .get(&edge.from)
            .expect("validated edge source has an allocated issue");
        let to_issue_id = *issue_by_temp_id
            .get(&edge.to)
            .expect("validated edge target has an allocated issue");
        graph_q::insert_edge(
            &mut *tx,
            &graph_q::DependencyGraphEdgeInsert {
                id: new_v7(),
                plan_id: plan.id,
                workspace_id,
                from_issue_id,
                to_issue_id,
                type_: HARD_DEPENDENCY_TYPE.to_string(),
                reason: edge.reason.clone(),
                consumed_output: edge.consumed_output.clone(),
            },
        )
        .await
        .map_err(db_error)?;
    }

    tx.commit().await.map_err(db_error)?;
    let mut snapshot = load_dependency_graph(pool, workspace_id, plan.id).await?;
    snapshot.newly_created = true;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    fn task(temp_id: &str, output: &str) -> DependencyGraphTaskInput {
        DependencyGraphTaskInput {
            temp_id: temp_id.to_string(),
            title: format!("{temp_id} title"),
            description: format!("{temp_id} description"),
            acceptance_criteria: vec![format!("{temp_id} is verifiable")],
            context: json!({"source": temp_id}),
            outputs: vec![output.to_string()],
            assignee: None,
            candidate_assignees: Vec::new(),
        }
    }

    fn edge(from: &str, to: &str, output: &str) -> DependencyGraphEdgeInput {
        DependencyGraphEdgeInput {
            from: from.to_string(),
            to: to.to_string(),
            type_: HARD_DEPENDENCY_TYPE.to_string(),
            reason: format!("{to} consumes {output} from {from}"),
            consumed_output: output.to_string(),
        }
    }

    fn plan(
        tasks: Vec<DependencyGraphTaskInput>,
        edges: Vec<DependencyGraphEdgeInput>,
    ) -> DependencyGraphPlanInput {
        DependencyGraphPlanInput {
            goal: "ship a coherent feature".to_string(),
            parent_issue_id: Uuid::now_v7(),
            tasks,
            edges,
        }
    }

    #[test]
    fn roots_and_parallel_waves_are_derived_from_edges() {
        let input = plan(
            vec![task("a", "a-out"), task("b", "b-out"), task("c", "c-out")],
            vec![edge("a", "c", "a-out")],
        );
        assert_eq!(
            validate_dependency_plan(&input).unwrap(),
            vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["c".to_string()]
            ]
        );
    }

    #[test]
    fn rejects_self_duplicate_unknown_and_non_hard_edges() {
        let mut self_edge = plan(vec![task("a", "a-out")], Vec::new());
        self_edge.edges = vec![edge("a", "a", "a-out")];
        assert!(matches!(
            validate_dependency_plan(&self_edge),
            Err(DependencyGraphError::Validation(message)) if message.contains("self")
        ));

        let mut unknown = plan(vec![task("a", "a-out")], Vec::new());
        unknown.edges = vec![edge("missing", "a", "x")];
        assert!(matches!(
            validate_dependency_plan(&unknown),
            Err(DependencyGraphError::Validation(message)) if message.contains("unknown")
        ));

        let mut duplicate = plan(
            vec![task("a", "a-out"), task("b", "b-out")],
            vec![edge("a", "b", "a-out"), edge("a", "b", "a-out")],
        );
        assert!(matches!(
            validate_dependency_plan(&duplicate),
            Err(DependencyGraphError::Validation(message)) if message.contains("duplicate")
        ));

        duplicate.edges = vec![DependencyGraphEdgeInput {
            type_: "soft".to_string(),
            ..edge("a", "b", "a-out")
        }];
        assert!(matches!(
            validate_dependency_plan(&duplicate),
            Err(DependencyGraphError::Validation(message)) if message.contains("hard")
        ));
    }

    #[test]
    fn text_limits_count_unicode_characters() {
        let mut input = plan(vec![task("a", "a-out")], Vec::new());
        input.tasks[0].title = "界".repeat(MAX_TITLE_LENGTH);
        assert!(validate_dependency_plan(&input).is_ok());

        input.tasks[0].title.push('界');
        assert!(matches!(
            validate_dependency_plan(&input),
            Err(DependencyGraphError::Validation(message)) if message.contains("title exceeds")
        ));
    }

    #[test]
    fn rejects_cycles_transitive_edges_and_unconsumed_outputs() {
        let mut cycle = plan(
            vec![task("a", "a-out"), task("b", "b-out")],
            vec![edge("a", "b", "a-out"), edge("b", "a", "b-out")],
        );
        assert!(matches!(
            validate_dependency_plan(&cycle),
            Err(DependencyGraphError::Validation(message)) if message.contains("cycle")
        ));

        cycle = plan(
            vec![task("a", "a-out"), task("b", "b-out"), task("c", "c-out")],
            vec![
                edge("a", "b", "a-out"),
                edge("b", "c", "b-out"),
                edge("a", "c", "a-out"),
            ],
        );
        assert!(matches!(
            validate_dependency_plan(&cycle),
            Err(DependencyGraphError::Validation(message)) if message.contains("transitively")
        ));

        let mut missing_output = plan(
            vec![task("a", "a-out"), task("b", "b-out")],
            vec![edge("a", "b", "not-produced")],
        );
        assert!(matches!(
            validate_dependency_plan(&missing_output),
            Err(DependencyGraphError::Validation(message)) if message.contains("not an output")
        ));
        missing_output.edges[0].consumed_output = "".to_string();
        assert!(matches!(
            validate_dependency_plan(&missing_output),
            Err(DependencyGraphError::Validation(message)) if message.contains("consumed_output")
        ));
    }

    #[test]
    fn hashes_semantically_equivalent_json_context_deterministically() {
        let mut first = plan(vec![task("a", "a-out")], Vec::new());
        let mut second = first.clone();
        first.tasks[0].context = serde_json::from_str(r#"{"z":1,"a":{"y":2,"x":3}}"#).unwrap();
        second.tasks[0].context = serde_json::from_str(r#"{"a":{"x":3,"y":2},"z":1}"#).unwrap();
        assert_eq!(plan_request_hash(&first), plan_request_hash(&second));
    }

    /// Controlled acceptance for the real persistence boundary. This is
    /// intentionally DB-backed rather than a prompt/snapshot test: a typed
    /// plan is atomically applied, replayed through its idempotency key, and
    /// only promoted after the prerequisite reaches successful Done.
    #[tokio::test]
    async fn typed_plan_atomic_apply_and_readiness_gate_are_consistent() {
        let Some(url) = std::env::var("DATABASE_URL").ok() else {
            eprintln!("skipping: DATABASE_URL not set");
            return;
        };
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect dependency graph contract PostgreSQL");
        let workspace_id: Uuid = sqlx::query_scalar(
            "INSERT INTO workspace (name, slug) VALUES ('dependency graph contract', $1) RETURNING id",
        )
        .bind(format!("dependency-graph-{}", Uuid::now_v7().simple()))
        .fetch_one(&pool)
        .await
        .expect("create dependency graph contract workspace");
        let creator_id = Uuid::now_v7();
        let parent_number = increment_issue_counter(&pool, workspace_id)
            .await
            .expect("increment parent issue counter")
            .expect("workspace issue counter row");
        let parent = issue_q::create_issue(
            &pool,
            workspace_id,
            "dependency graph parent",
            Some("contract parent"),
            issue_status::TODO,
            "none",
            None,
            None,
            "member",
            creator_id,
            None,
            0.0,
            None,
            None,
            parent_number,
            None,
            None,
            new_v7(),
        )
        .await
        .expect("insert dependency graph parent")
        .expect("parent issue row");

        let mut input = plan(
            vec![
                task("contract", "contract-output"),
                task("consumer", "consumer-output"),
            ],
            vec![edge("contract", "consumer", "contract-output")],
        );
        input.parent_issue_id = parent.id;

        // A validation/assignee failure must leave no plan or child issue
        // behind. This is the fail-closed side of atomic apply, before the
        // successful controlled flow below.
        let mut invalid_input = input.clone();
        invalid_input.tasks[0].assignee = Some(PlanAssignee {
            type_: "agent".to_string(),
            id: Uuid::now_v7(),
        });
        let invalid_result = apply_dependency_plan(
            &pool,
            workspace_id,
            &invalid_input,
            &format!("dependency-graph-invalid-{}", Uuid::now_v7()),
            "member",
            creator_id,
        )
        .await;
        assert!(matches!(
            invalid_result,
            Err(DependencyGraphError::AssigneeNotFound { .. })
        ));
        let plans_after_rollback: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM dependency_graph_plan WHERE workspace_id = $1 AND parent_issue_id = $2",
        )
        .bind(workspace_id)
        .bind(parent.id)
        .fetch_one(&pool)
        .await
        .expect("count plans after failed atomic apply");
        let children_after_rollback: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM issue WHERE workspace_id = $1 AND parent_issue_id = $2",
        )
        .bind(workspace_id)
        .bind(parent.id)
        .fetch_one(&pool)
        .await
        .expect("count children after failed atomic apply");
        assert_eq!(plans_after_rollback, 0);
        assert_eq!(children_after_rollback, 0);

        let idempotency_key = format!("dependency-graph-contract-{}", Uuid::now_v7());
        let (first_result, concurrent_result) = tokio::join!(
            apply_dependency_plan(
                &pool,
                workspace_id,
                &input,
                &idempotency_key,
                "member",
                creator_id,
            ),
            apply_dependency_plan(
                &pool,
                workspace_id,
                &input,
                &idempotency_key,
                "member",
                creator_id,
            ),
        );
        let first = first_result.expect("apply typed dependency graph plan");
        let concurrent = concurrent_result.expect("concurrent idempotent dependency graph plan");
        assert_eq!(concurrent.plan.id, first.plan.id);
        assert_ne!(first.newly_created, concurrent.newly_created);
        assert!(first.newly_created || concurrent.newly_created);
        assert_eq!(first.nodes.len(), 2);
        assert_eq!(first.edges.len(), 1);
        assert_eq!(first.edges[0].from_temp_id, "contract");
        assert_eq!(first.edges[0].to_temp_id, "consumer");
        assert_eq!(first.edges[0].edge.consumed_output, "contract-output");

        let root = first
            .nodes
            .iter()
            .find(|node| node.node.temp_id == "contract")
            .expect("root node");
        let dependent = first
            .nodes
            .iter()
            .find(|node| node.node.temp_id == "consumer")
            .expect("dependent node");
        assert_eq!(root.issue.status, issue_status::TODO);
        assert_eq!(root.readiness.state, "ready");
        assert!(root.readiness.gate_open);
        assert_eq!(dependent.issue.status, issue_status::BLOCKED);
        assert_eq!(dependent.readiness.state, "blocked");
        assert!(!dependent.readiness.gate_open);
        assert_eq!(
            dependent.readiness.unlock_condition,
            "All 1 hard prerequisites must be Done (0/1 currently satisfied)"
        );

        // A replay returns the original graph and does not allocate another
        // plan or another pair of child issues.
        let replay = apply_dependency_plan(
            &pool,
            workspace_id,
            &input,
            &idempotency_key,
            "member",
            creator_id,
        )
        .await
        .expect("replay idempotent dependency graph plan");
        assert_eq!(replay.plan.id, first.plan.id);
        assert!(!replay.newly_created);
        assert_eq!(replay.nodes.len(), 2);
        let root_issue_id = root.issue.id;
        let dependent_issue_id = dependent.issue.id;

        sqlx::query("UPDATE issue SET status = 'done', updated_at = now() WHERE id = $1 AND workspace_id = $2")
            .bind(root_issue_id)
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("complete graph prerequisite");
        let before_wakeup = load_dependency_graph(&pool, workspace_id, first.plan.id)
            .await
            .expect("load graph after prerequisite completion");
        let dependent_before_wakeup = before_wakeup
            .nodes
            .iter()
            .find(|node| node.issue.id == dependent_issue_id)
            .expect("dependent before wakeup");
        assert!(dependent_before_wakeup.readiness.gate_open);
        assert_eq!(dependent_before_wakeup.readiness.satisfied_prerequisites, 1);
        assert_eq!(dependent_before_wakeup.issue.status, issue_status::BLOCKED);
        let dependent_revision_before_wakeup = dependent_before_wakeup.issue.revision;

        let mut tx = pool.begin().await.expect("begin dependency wakeup");
        let promoted = graph_q::promote_ready_dependents(&mut *tx, workspace_id, root_issue_id)
            .await
            .expect("promote ready dependent");
        tx.commit().await.expect("commit dependency wakeup");
        assert_eq!(promoted, vec![dependent_issue_id]);

        let mut replay_wakeup = pool.begin().await.expect("begin replay wakeup");
        let promoted_again =
            graph_q::promote_ready_dependents(&mut *replay_wakeup, workspace_id, root_issue_id)
                .await
                .expect("replay dependency wakeup");
        replay_wakeup
            .commit()
            .await
            .expect("commit replay dependency wakeup");
        assert!(promoted_again.is_empty());

        let after_wakeup = load_dependency_graph(&pool, workspace_id, first.plan.id)
            .await
            .expect("load graph after wakeup");
        let dependent_after_wakeup = after_wakeup
            .nodes
            .iter()
            .find(|node| node.issue.id == dependent_issue_id)
            .expect("dependent after wakeup");
        assert_eq!(dependent_after_wakeup.issue.status, issue_status::TODO);
        assert_eq!(
            dependent_after_wakeup.issue.revision,
            dependent_revision_before_wakeup + 1
        );
        assert_eq!(dependent_after_wakeup.readiness.state, "ready");
        assert!(dependent_after_wakeup.readiness.gate_open);

        let retired = retire_dependency_plan(&pool, workspace_id, first.plan.id)
            .await
            .expect("retire dependency graph plan");
        assert_eq!(retired.status, "cancelled");

        // Deleting any graph node removes the whole affected plan before the
        // issue row disappears; no future graph read can observe a dangling
        // node or edge.
        assert_eq!(
            issue_q::delete_issue(&pool, root_issue_id, workspace_id)
                .await
                .expect("delete dependency graph node issue"),
            1
        );
        for (table, key_column) in [
            ("dependency_graph_edge", "plan_id"),
            ("dependency_graph_node", "plan_id"),
            ("dependency_graph_plan", "id"),
        ] {
            let remaining: i64 =
                sqlx::query_scalar(&format!(
                    "SELECT COUNT(*) FROM {table} WHERE {key_column} = $1"
                ))
                    .bind(first.plan.id)
                    .fetch_one(&pool)
                    .await
                    .expect("count graph rows after issue deletion");
            assert_eq!(remaining, 0, "dangling rows remain in {table}");
        }

        // This test owns all rows it creates. Graph rows are explicitly
        // removed because the graph domain intentionally has no FKs.
        sqlx::query("DELETE FROM dependency_graph_edge WHERE plan_id = $1")
            .bind(first.plan.id)
            .execute(&pool)
            .await
            .expect("cleanup dependency graph edges");
        sqlx::query("DELETE FROM dependency_graph_node WHERE plan_id = $1")
            .bind(first.plan.id)
            .execute(&pool)
            .await
            .expect("cleanup dependency graph nodes");
        sqlx::query("DELETE FROM dependency_graph_plan WHERE id = $1")
            .bind(first.plan.id)
            .execute(&pool)
            .await
            .expect("cleanup dependency graph plan");
        sqlx::query("DELETE FROM issue WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("cleanup dependency graph issues");
        sqlx::query("DELETE FROM workspace WHERE id = $1")
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("cleanup dependency graph workspace");
    }
}
