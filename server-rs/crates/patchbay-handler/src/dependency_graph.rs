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
use patchbay_middleware::workspace::WorkspaceContext;
use patchbay_service::dependency_graph::{
    apply_dependency_plan, load_active_dependency_graph_for_issue, load_active_dependency_graphs,
    load_dependency_graph,
    DependencyGraphError, DependencyGraphPlanInput, DependencyGraphSnapshot,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{error_code_response, error_response};
use crate::issue::issue_response_projection;
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
        .route("/api/dependency-graphs", get(list_dependency_graphs))
}

#[derive(Debug, Deserialize)]
struct DependencyGraphListQuery {
    project_id: Option<Uuid>,
    limit: Option<u32>,
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "workspace not found"))
}

fn graph_error(error: DependencyGraphError) -> Response {
    let (status, code) = match &error {
        DependencyGraphError::Validation(_) => (StatusCode::UNPROCESSABLE_ENTITY, "invalid_plan"),
        DependencyGraphError::ParentNotFound | DependencyGraphError::NotFound(_) => {
            (StatusCode::NOT_FOUND, "not_found")
        }
        DependencyGraphError::ActivePlanExists => (StatusCode::CONFLICT, "active_plan_exists"),
        DependencyGraphError::IdempotencyConflict => {
            (StatusCode::CONFLICT, "idempotency_conflict")
        }
        DependencyGraphError::AssigneeNotFound { .. } => {
            (StatusCode::UNPROCESSABLE_ENTITY, "invalid_assignee")
        }
        DependencyGraphError::Integrity(_) => (StatusCode::INTERNAL_SERVER_ERROR, "graph_integrity"),
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
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(workspace_id) => workspace_id,
        Err(response) => return response,
    };
    match load_active_dependency_graph_for_issue(&state.pool, workspace_id, issue_id).await {
        Ok(snapshot) => snapshot_response(&state, snapshot).await,
        Err(error) => graph_error(error),
    }
}

async fn get_dependency_graph_by_id(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(plan_id): Path<Uuid>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(workspace_id) => workspace_id,
        Err(response) => return response,
    };
    match load_dependency_graph(&state.pool, workspace_id, plan_id).await {
        Ok(snapshot) => snapshot_response(&state, snapshot).await,
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
    match load_active_dependency_graphs(&state.pool, workspace_id, query.project_id, limit).await {
        Ok(snapshots) => {
            let mut graphs = Vec::with_capacity(snapshots.len());
            for snapshot in snapshots {
                graphs.push(snapshot_value(&state, snapshot).await);
            }
            Json(json!({ "graphs": graphs })).into_response()
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

    match apply_dependency_plan(
        &state.pool,
        workspace_id,
        &input,
        idempotency_key,
        "member",
        context.member.user_id,
    )
    .await
    {
        Ok(snapshot) => {
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
                actor_type: "member".to_string(),
                actor_id: context.member.user_id.to_string(),
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

async fn snapshot_response(state: &HandlerState, snapshot: DependencyGraphSnapshot) -> Response {
    Json(snapshot_value(state, snapshot).await).into_response()
}

async fn snapshot_value(state: &HandlerState, snapshot: DependencyGraphSnapshot) -> Value {
    let mut node_values = Vec::with_capacity(snapshot.nodes.len());
    for node in &snapshot.nodes {
        let issue = issue_response_projection(state, &node.issue).await;
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
            "assignee_type": node.node.assignee_type,
            "assignee_id": node.node.assignee_id,
            "candidate_assignees": node.node.candidate_assignees,
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
        "parent": issue_response_projection(state, &snapshot.parent).await,
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
