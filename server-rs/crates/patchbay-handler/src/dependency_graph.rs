//! HTTP surface for persisted dependency graphs.
//!
//! Graph responses intentionally contain the normal Issue projection for each
//! node. The web client can therefore open the same real issue surface and
//! subscribe to the same issue events instead of rendering fixture-only graph
//! cards.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use patchbay_authorization::Action;
use patchbay_db::queries::{agent, dependency_graph as graph_q};
use patchbay_middleware::workspace::WorkspaceContext;
use patchbay_service::dependency_graph::{
    apply_dependency_plan, load_active_dependency_graph_for_issue,
    load_active_dependency_graphs_after, load_dependency_graph, retire_dependency_plan,
    DependencyGraphError, DependencyGraphPage, DependencyGraphPlanInput, DependencyGraphSnapshot,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{error_code_response, error_response};
use crate::issue::{
    issue_created_response_with_status_category, issue_prefix, issue_response_with_status_category,
};
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route(
            "/api/issues/{id}/dependency-graph",
            get(get_issue_dependency_graph),
        )
        .route(
            "/api/issues/{id}/dependency-graph/apply",
            post(apply_issue_dependency_graph),
        )
        .route(
            "/api/dependency-graphs/{id}",
            get(get_dependency_graph_by_id),
        )
        .route(
            "/api/dependency-graphs/{id}/retire",
            post(retire_dependency_graph),
        )
        .route("/api/dependency-graphs", get(list_dependency_graphs))
}

#[derive(Debug, Deserialize)]
struct DependencyGraphListQuery {
    project_id: Option<Uuid>,
    limit: Option<u32>,
    cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DependencyGraphCursor {
    v: u8,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    updated_at: DateTime<Utc>,
    id: Uuid,
}

fn decode_graph_cursor(
    raw: Option<&str>,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
) -> Result<Option<(DateTime<Utc>, Uuid)>, Response> {
    let Some(raw) = raw.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let cursor = URL_SAFE_NO_PAD
        .decode(raw)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<DependencyGraphCursor>(&bytes).ok())
        .filter(|cursor| cursor.v == 1)
        .ok_or_else(|| {
            error_response(StatusCode::BAD_REQUEST, "invalid dependency graph cursor")
        })?;
    if cursor.workspace_id != workspace_id {
        return Err(error_response(
            StatusCode::CONFLICT,
            "dependency graph cursor does not belong to this workspace",
        ));
    }
    if cursor.project_id != project_id {
        return Err(error_response(
            StatusCode::CONFLICT,
            "dependency graph cursor does not belong to this project query",
        ));
    }
    Ok(Some((cursor.updated_at, cursor.id)))
}

fn encode_graph_cursor(
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    cursor: (DateTime<Utc>, Uuid),
) -> String {
    URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&DependencyGraphCursor {
            v: 1,
            workspace_id,
            project_id,
            updated_at: cursor.0,
            id: cursor.1,
        })
        .expect("dependency graph cursor is serializable"),
    )
}

async fn publish_pending_issue_created_events(
    state: &HandlerState,
    snapshot: &DependencyGraphSnapshot,
) {
    let lease_owner = format!("dependency-graph-issue-created:{}", Uuid::now_v7());
    let pending = match graph_q::claim_issue_created_outbox(
        &state.pool,
        snapshot.plan.workspace_id,
        snapshot.plan.id,
        &lease_owner,
    )
    .await
    {
        Ok(pending) => pending,
        Err(error) => {
            tracing::warn!(
                %error,
                plan_id = %snapshot.plan.id,
                "dependency graph issue-created publication claim deferred"
            );
            return;
        }
    };
    if pending.is_empty() {
        return;
    }
    let prefix = issue_prefix(state, snapshot.plan.workspace_id).await;
    for event in pending {
        let Some(node) = snapshot
            .nodes
            .iter()
            .find(|node| node.node.id == event.node_id && node.issue.id == event.issue_id)
        else {
            tracing::warn!(
                plan_id = %event.plan_id,
                node_id = %event.node_id,
                issue_id = %event.issue_id,
                "dependency graph issue-created outbox row has no matching snapshot node"
            );
            continue;
        };
        let issue = issue_created_response_with_status_category(
            &node.issue,
            &prefix,
            &node.effective_status,
        );
        state.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_ISSUE_CREATED.to_string(),
            workspace_id: event.workspace_id.to_string(),
            actor_type: snapshot.plan.created_by_type.clone(),
            actor_id: snapshot.plan.created_by_id.to_string(),
            payload: json!({ "issue": issue }),
            ..Default::default()
        });
        match graph_q::mark_issue_created_outbox_published(
            &state.pool,
            event.workspace_id,
            event.plan_id,
            event.node_id,
            &lease_owner,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => tracing::warn!(
                plan_id = %event.plan_id,
                node_id = %event.node_id,
                "dependency graph issue-created publication was not acknowledged"
            ),
            Err(error) => tracing::warn!(
                %error,
                plan_id = %event.plan_id,
                node_id = %event.node_id,
                "dependency graph issue-created publication mark deferred"
            ),
        }
    }
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "workspace not found"))
}

fn graph_creator(
    headers: &HeaderMap,
    context: &WorkspaceContext,
) -> Result<(&'static str, Uuid), Response> {
    if headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        == Some("task_token")
    {
        let Some(agent_id) = headers
            .get("x-agent-id")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| Uuid::parse_str(value).ok())
        else {
            return Err(error_response(
                StatusCode::FORBIDDEN,
                "invalid planner Agent identity",
            ));
        };
        return Ok(("agent", agent_id));
    }
    Ok(("member", context.member.user_id))
}

async fn graph_read_allowed(
    state: &HandlerState,
    headers: &HeaderMap,
    workspace_id: Uuid,
    issue_id: Uuid,
) -> bool {
    let is_task_token = headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        == Some("task_token");
    if is_task_token && crate::issue::TaskAuthorizationContext::from_headers(headers).is_none() {
        return false;
    }
    crate::issue::task_project_resource_allows(
        state,
        headers,
        workspace_id,
        Some(issue_id),
        true,
        Action::RESOURCE_READ,
    )
    .await
}

async fn graph_apply_allowed(
    state: &HandlerState,
    headers: &HeaderMap,
    workspace_id: Uuid,
    parent_issue_id: Uuid,
) -> bool {
    let is_task_token = headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        == Some("task_token");
    let authorization = crate::issue::TaskAuthorizationContext::from_headers(headers);
    if is_task_token && authorization.is_none() {
        return false;
    }
    if !crate::issue::task_project_resource_allows(
        state,
        headers,
        workspace_id,
        Some(parent_issue_id),
        true,
        Action::RESOURCE_USE,
    )
    .await
    {
        return false;
    }
    let Some(authorization) = authorization else {
        return true;
    };
    let Some(agent_id) = authorization.via_agent_id else {
        return false;
    };
    let is_leader_task = agent::get_agent_task_in_workspace(
        &state.pool,
        authorization.task_id,
        workspace_id,
    )
    .await
    .ok()
    .flatten()
    .is_some_and(|task| task.agent_id == agent_id && task.is_leader_task);
    if !is_leader_task {
        return false;
    }
    // Planner task credentials are intentionally narrower than ordinary
    // leader credentials: only the workspace's configured Patrick may apply
    // or retire the authoritative dependency graph. Human members retain the
    // explicit planning path above, while delegated agents cannot impersonate
    // the orchestrator by forging an x-agent-id header.
    agent::get_agent_by_system_key(&state.pool, workspace_id, Some("patrick"))
        .await
        .ok()
        .flatten()
        .is_some_and(|patrick| patrick.id == agent_id)
}

fn graph_error(error: DependencyGraphError) -> Response {
    let (status, code) = match &error {
        DependencyGraphError::Validation(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid_plan"),
        DependencyGraphError::ParentNotFound | DependencyGraphError::NotFound(_) => {
            (StatusCode::NOT_FOUND, "not_found")
        }
        DependencyGraphError::ActivePlanExists => (StatusCode::CONFLICT, "active_plan_exists"),
        DependencyGraphError::IdempotencyConflict => (StatusCode::CONFLICT, "idempotency_conflict"),
        DependencyGraphError::ExecutorNotFound { .. } => {
            (StatusCode::UNPROCESSABLE_ENTITY, "invalid_executor")
        }
        DependencyGraphError::RuntimeNotFound(_) => {
            (StatusCode::UNPROCESSABLE_ENTITY, "invalid_runtime")
        }
        DependencyGraphError::Integrity(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "graph_integrity")
        }
        DependencyGraphError::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, "database_error"),
    };
    if matches!(
        &error,
        DependencyGraphError::Integrity(_) | DependencyGraphError::Database(_)
    ) {
        tracing::error!(error = %error, "dependency graph request failed internally");
        return error_code_response(status, code, "dependency graph operation failed");
    }
    error_code_response(status, code, &error.to_string())
}

async fn get_issue_dependency_graph(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(issue_id): Path<Uuid>,
    headers: HeaderMap,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(workspace_id) => workspace_id,
        Err(response) => return response,
    };
    if !graph_read_allowed(&state, &headers, workspace_id, issue_id).await {
        return error_response(
            StatusCode::FORBIDDEN,
            "task capability does not allow reading this dependency graph",
        );
    }
    match load_active_dependency_graph_for_issue(&state.pool, workspace_id, issue_id).await {
        Ok(snapshot) => snapshot_response(&state, snapshot).await,
        Err(error) => graph_error(error),
    }
}

async fn get_dependency_graph_by_id(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(plan_id): Path<Uuid>,
    headers: HeaderMap,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(workspace_id) => workspace_id,
        Err(response) => return response,
    };
    match load_dependency_graph(&state.pool, workspace_id, plan_id).await {
        Ok(snapshot) => {
            if !graph_read_allowed(
                &state,
                &headers,
                workspace_id,
                snapshot.plan.parent_issue_id,
            )
            .await
            {
                return error_response(
                    StatusCode::FORBIDDEN,
                    "task capability does not allow reading this dependency graph",
                );
            }
            snapshot_response(&state, snapshot).await
        }
        Err(error) => graph_error(error),
    }
}

async fn list_dependency_graphs(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(query): Query<DependencyGraphListQuery>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(workspace_id) => workspace_id,
        Err(response) => return response,
    };
    let limit = i64::from(query.limit.unwrap_or(64).min(64));
    let after = match decode_graph_cursor(query.cursor.as_deref(), workspace_id, query.project_id) {
        Ok(after) => after,
        Err(response) => return response,
    };
    match load_active_dependency_graphs_after(
        &state.pool,
        workspace_id,
        query.project_id,
        limit,
        after,
    )
    .await
    {
        Ok(DependencyGraphPage {
            snapshots,
            next_cursor,
        }) => {
            let prefix = issue_prefix(&state, workspace_id).await;
            let mut graphs = Vec::with_capacity(snapshots.len());
            for snapshot in snapshots {
                graphs.push(snapshot_value(&snapshot, &prefix));
            }
            Json(json!({
                "graphs": graphs,
                "next_cursor": next_cursor
                    .map(|cursor| encode_graph_cursor(workspace_id, query.project_id, cursor)),
            }))
            .into_response()
        }
        Err(error) => graph_error(error),
    }
}

async fn apply_issue_dependency_graph(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(parent_issue_id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<DependencyGraphPlanInput>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(workspace_id) => workspace_id,
        Err(response) => return response,
    };
    if input.parent_issue_id != parent_issue_id {
        return error_code_response(
            StatusCode::BAD_REQUEST,
            "parent_mismatch",
            "parent_issue_id must match the issue in the request path",
        );
    }
    let idempotency_key = headers
        .get("idempotency-key")
        .or_else(|| headers.get("x-idempotency-key"))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(idempotency_key) = idempotency_key else {
        return error_code_response(
            StatusCode::BAD_REQUEST,
            "idempotency_key_required",
            "Idempotency-Key header is required",
        );
    };
    if !graph_apply_allowed(&state, &headers, workspace_id, parent_issue_id).await {
        return error_response(
            StatusCode::FORBIDDEN,
            "task capability does not allow applying a dependency graph",
        );
    }
    let task_authorization = crate::issue::TaskAuthorizationContext::from_headers(&headers);
    for task in &input.tasks {
        for executor in task.executor.iter().chain(task.candidate_executors.iter()) {
            if let Err(message) = crate::issue::validate_executor(
                &state,
                &context,
                &executor.type_,
                executor.id,
                task_authorization,
            )
            .await
            {
                return error_code_response(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_executor",
                    &message,
                );
            }
        }
    }
    let (created_by_type, created_by_id) = match graph_creator(&headers, &context) {
        Ok(creator) => creator,
        Err(response) => return response,
    };
    if let Some(authorization) = task_authorization {
        if created_by_type != "agent" || authorization.via_agent_id != Some(created_by_id) {
            return error_response(
                StatusCode::FORBIDDEN,
                "planner identity does not match the task credential",
            );
        }
    }

    match apply_dependency_plan(
        &state.pool,
        workspace_id,
        &input,
        idempotency_key,
        created_by_type,
        created_by_id,
    )
    .await
    {
        Ok(snapshot) => {
            // The outbox is populated in the same transaction as the graph.
            // Drain it for both the first apply and idempotent replays so a
            // post-commit handler failure can recover the standard events.
            publish_pending_issue_created_events(&state, &snapshot).await;
            if let Err(error) = state
                .tasks
                .wake_dependency_graph_ready_tasks(workspace_id, snapshot.plan.id)
                .await
            {
                // The graph commit is already durable. Claim-time recovery
                // will retry admission, so surface the failure to logs while
                // still returning the auditable plan to the caller.
                tracing::warn!(%error, plan_id = %snapshot.plan.id, "dependency graph readiness wakeup deferred");
            }
            state.bus.publish(&patchbay_events::Event {
                event_type: patchbay_protocol::EVENT_DEPENDENCY_GRAPH_UPDATED.to_string(),
                workspace_id: workspace_id.to_string(),
                actor_type: created_by_type.to_string(),
                actor_id: created_by_id.to_string(),
                payload: json!({
                    "plan_id": snapshot.plan.id,
                    "parent_issue_id": snapshot.plan.parent_issue_id,
                }),
                ..Default::default()
            });
            snapshot_response(&state, snapshot).await
        }
        Err(error) => graph_error(error),
    }
}

async fn retire_dependency_graph(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(plan_id): Path<Uuid>,
    headers: HeaderMap,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(workspace_id) => workspace_id,
        Err(response) => return response,
    };
    let plan = match graph_q::get_plan_by_id(&state.pool, plan_id, workspace_id).await {
        Ok(Some(plan)) => plan,
        Ok(None) => return graph_error(DependencyGraphError::NotFound(plan_id)),
        Err(error) => return graph_error(DependencyGraphError::Database(error.to_string())),
    };
    if plan.status != "active" {
        return error_code_response(
            StatusCode::CONFLICT,
            "plan_not_active",
            "dependency graph plan is not active",
        );
    }
    if !graph_apply_allowed(&state, &headers, workspace_id, plan.parent_issue_id).await {
        return error_response(
            StatusCode::FORBIDDEN,
            "task capability does not allow retiring this dependency graph",
        );
    }
    let (actor_type, actor_id) = match graph_creator(&headers, &context) {
        Ok(actor) => actor,
        Err(response) => return response,
    };
    match retire_dependency_plan(&state.pool, workspace_id, plan_id).await {
        Ok(retirement) => {
            state
                .tasks
                .publish_transactional_cancellations(&retirement.cancellation.cancelled_tasks)
                .await;
            for (previous, updated) in retirement.cancellation.cancelled_issues {
                crate::issue::publish_issue_updated(
                    &state, &previous, &updated, actor_type, actor_id, None,
                )
                .await;
            }
            let plan = retirement.plan;
            state.bus.publish(&patchbay_events::Event {
                event_type: patchbay_protocol::EVENT_DEPENDENCY_GRAPH_UPDATED.to_string(),
                workspace_id: workspace_id.to_string(),
                actor_type: actor_type.to_string(),
                actor_id: actor_id.to_string(),
                payload: json!({
                    "plan_id": plan.id,
                    "parent_issue_id": plan.parent_issue_id,
                    "status": plan.status,
                }),
                ..Default::default()
            });
            Json(json!({
                "plan_id": plan.id,
                "parent_issue_id": plan.parent_issue_id,
                "status": plan.status,
            }))
            .into_response()
        }
        Err(error) => graph_error(error),
    }
}

async fn snapshot_response(state: &HandlerState, snapshot: DependencyGraphSnapshot) -> Response {
    let prefix = issue_prefix(state, snapshot.plan.workspace_id).await;
    Json(snapshot_value(&snapshot, &prefix)).into_response()
}

fn snapshot_value(snapshot: &DependencyGraphSnapshot, prefix: &str) -> Value {
    let mut node_values = Vec::with_capacity(snapshot.nodes.len());
    for node in &snapshot.nodes {
        let issue =
            issue_response_with_status_category(&node.issue, prefix, &node.effective_status);
        node_values.push(json!({
            "id": node.node.id,
            "temp_id": node.node.temp_id,
            "issue_id": node.node.issue_id,
            "issue": issue,
            "title": node.node.title,
            "description": node.node.description,
            "acceptance_criteria": node.node.acceptance_criteria,
            "context": node.node.context,
            "outputs": node.node.outputs,
            "owner_type": node.node.owner_type,
            "owner_id": node.node.owner_id,
            "executor_type": node.node.executor_type,
            "executor_id": node.node.executor_id,
            "candidate_executors": node.node.candidate_executors,
            "reviewer_type": node.node.reviewer_type,
            "reviewer_id": node.node.reviewer_id,
            "runtime_id": node.node.runtime_id,
            "model_id": node.node.model_id,
            "wave": node.node.wave,
            "status": node.issue.status,
            "readiness": {
                "state": node.readiness.state,
                "gate_open": node.readiness.gate_open,
                "satisfied_prerequisites": node.readiness.satisfied_prerequisites,
                "total_prerequisites": node.readiness.total_prerequisites,
                "unlock_condition": node.readiness.unlock_condition,
            },
        }));
    }
    let children = node_values
        .iter()
        .filter_map(|node| node.get("issue").cloned())
        .collect::<Vec<Value>>();
    let edges = snapshot
        .edges
        .iter()
        .map(|edge| {
            json!({
                "id": edge.edge.id,
                "plan_id": edge.edge.plan_id,
                "from_issue_id": edge.edge.from_issue_id,
                "to_issue_id": edge.edge.to_issue_id,
                "from": edge.from_temp_id,
                "to": edge.to_temp_id,
                "type": edge.edge.type_,
                "reason": edge.edge.reason,
                "consumed_output": edge.edge.consumed_output,
                "prerequisite_status": edge.prerequisite_status,
                "satisfied": edge.satisfied,
                "satisfied_prerequisites": edge.satisfied_prerequisites,
                "total_prerequisites": edge.total_prerequisites,
                "unlock_condition": edge.unlock_condition,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "plan": {
            "id": snapshot.plan.id,
            "workspace_id": snapshot.plan.workspace_id,
            "parent_issue_id": snapshot.plan.parent_issue_id,
            "idempotency_key": snapshot.plan.idempotency_key,
            "goal": snapshot.plan.goal,
            "status": snapshot.plan.status,
            "attention_required": snapshot.plan.attention_required,
            "attention_reason": snapshot.plan.attention_reason,
            "created_by_type": snapshot.plan.created_by_type,
            "created_by_id": snapshot.plan.created_by_id,
            "created_at": snapshot.plan.created_at,
            "updated_at": snapshot.plan.updated_at,
        },
        "parent": issue_response_with_status_category(
            &snapshot.parent,
            prefix,
            &snapshot.parent_effective_status,
        ),
        "children": children,
        "nodes": node_values,
        "edges": edges,
        "waves": snapshot.waves,
        "readiness": {
            "total": snapshot.readiness.total,
            "ready": snapshot.readiness.ready,
            "running": snapshot.readiness.running,
            "blocked": snapshot.readiness.blocked,
            "done": snapshot.readiness.done,
            "cancelled": snapshot.readiness.cancelled,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_cursor_is_bound_to_workspace_and_project() {
        let workspace_id = Uuid::now_v7();
        let other_workspace_id = Uuid::now_v7();
        let project_id = Some(Uuid::now_v7());
        let updated_at = Utc::now();
        let graph_id = Uuid::now_v7();
        let encoded = encode_graph_cursor(workspace_id, project_id, (updated_at, graph_id));

        assert_eq!(
            decode_graph_cursor(Some(&encoded), workspace_id, project_id).expect("cursor decodes"),
            Some((updated_at, graph_id))
        );
        assert!(decode_graph_cursor(Some(&encoded), other_workspace_id, project_id).is_err());
        assert!(decode_graph_cursor(Some(&encoded), workspace_id, None).is_err());
    }
}
