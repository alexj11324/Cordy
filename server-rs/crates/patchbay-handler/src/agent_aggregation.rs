//! Workspace-wide agent activity and run-count projections.

use std::collections::{HashMap, HashSet};

use axum::extract::{Extension, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use patchbay_db::models::{Agent, AgentInvocationTarget};
use patchbay_db::queries::{agent, agent_invocation_target};
use patchbay_middleware::workspace::WorkspaceContext;
use serde::Serialize;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/agent-activity-30d", get(activity_30d))
        .route("/api/agent-run-counts", get(run_counts))
        .route("/api/agent-task-snapshot", get(task_snapshot))
        .route("/api/working-agents", get(working_agents))
}

#[derive(Debug, Serialize)]
struct ActivityBucket {
    agent_id: String,
    bucket_at: String,
    task_count: i32,
    failed_count: i32,
}

#[derive(Debug, Serialize)]
struct RunCount {
    agent_id: String,
    run_count: i32,
}

#[derive(Debug, Default, serde::Deserialize)]
struct WorkingParams {
    #[serde(default, rename = "type")]
    work_type: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    relation: String,
    #[serde(default)]
    parent: String,
}

#[derive(Debug, Serialize)]
struct WorkingAgent {
    id: String,
    name: String,
    avatar_url: Option<String>,
    running_task_count: i32,
    issue_ids: Vec<String>,
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace id"))
}

fn member_can_view(
    agent: &Agent,
    targets: &[AgentInvocationTarget],
    user_id: Uuid,
    role: &str,
) -> bool {
    if matches!(role, "owner" | "admin") || agent.owner_id == Some(user_id) {
        return true;
    }
    agent.permission_mode == "public_to"
        && targets.iter().any(|target| {
            target.target_type == "workspace"
                || (target.target_type == "member" && target.target_id == user_id)
        })
}

async fn accessible_agent_ids(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    workspace_id: Uuid,
) -> Result<HashSet<Uuid>, Response> {
    let agents = agent::list_all_agents(&state.pool, workspace_id)
        .await
        .map_err(|error| {
            tracing::warn!(%error, %workspace_id, "failed to list agents for aggregate access");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to resolve agent access",
            )
        })?;
    let (actor_type, _, _) = crate::issue::mutation_actor(state, context, headers).await;
    if actor_type == "agent" || matches!(context.member.role.as_str(), "owner" | "admin") {
        return Ok(agents.into_iter().map(|agent| agent.id).collect());
    }

    let targets = agent_invocation_target::list_agent_invocation_targets_by_agent_i_ds(
        &state.pool,
        agents.iter().map(|agent| agent.id).collect(),
    )
    .await
    .map_err(|error| {
        tracing::warn!(%error, %workspace_id, "failed to list agent invocation targets");
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to resolve agent access",
        )
    })?;
    let mut targets_by_agent: HashMap<Uuid, Vec<AgentInvocationTarget>> = HashMap::new();
    for target in targets {
        targets_by_agent
            .entry(target.agent_id)
            .or_default()
            .push(target);
    }
    Ok(agents
        .into_iter()
        .filter(|agent| {
            member_can_view(
                agent,
                targets_by_agent
                    .get(&agent.id)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                context.member.user_id,
                &context.member.role,
            )
        })
        .map(|agent| agent.id)
        .collect())
}

async fn run_counts(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(workspace_id) => workspace_id,
        Err(response) => return response,
    };
    let rows = match agent::get_workspace_agent_run_counts(&state.pool, workspace_id).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, %workspace_id, "failed to get agent run counts");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get agent run counts",
            );
        }
    };
    let allowed = match accessible_agent_ids(&state, &context, &headers, workspace_id).await {
        Ok(allowed) => allowed,
        Err(response) => return response,
    };
    Json(
        rows.into_iter()
            .filter_map(|row| {
                let agent_id = row.agent_id?;
                allowed.contains(&agent_id).then_some(RunCount {
                    agent_id: agent_id.to_string(),
                    run_count: row.run_count,
                })
            })
            .collect::<Vec<_>>(),
    )
    .into_response()
}

async fn activity_30d(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(workspace_id) => workspace_id,
        Err(response) => return response,
    };
    let rows = match agent::get_workspace_agent_activity30d(&state.pool, workspace_id).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, %workspace_id, "failed to get agent activity");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get agent activity",
            );
        }
    };
    let allowed = match accessible_agent_ids(&state, &context, &headers, workspace_id).await {
        Ok(allowed) => allowed,
        Err(response) => return response,
    };
    Json(
        rows.into_iter()
            .filter_map(|row| {
                let agent_id = row.agent_id?;
                allowed.contains(&agent_id).then_some(ActivityBucket {
                    agent_id: agent_id.to_string(),
                    bucket_at: row.bucket.map(crate::timefmt::rfc3339).unwrap_or_default(),
                    task_count: row.task_count,
                    failed_count: row.failed_count,
                })
            })
            .collect::<Vec<_>>(),
    )
    .into_response()
}

async fn task_snapshot(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(workspace_id) => workspace_id,
        Err(response) => return response,
    };
    let tasks = match agent::list_workspace_agent_task_snapshot(&state.pool, workspace_id).await {
        Ok(tasks) => tasks,
        Err(error) => {
            tracing::warn!(%error, %workspace_id, "failed to list agent task snapshot");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list agent task snapshot",
            );
        }
    };
    let allowed = match accessible_agent_ids(&state, &context, &headers, workspace_id).await {
        Ok(allowed) => allowed,
        Err(response) => return response,
    };
    let tasks = tasks
        .into_iter()
        .filter(|task| allowed.contains(&task.agent_id))
        .collect::<Vec<_>>();
    Json(crate::issue::task_maps(&state, &tasks, &workspace_id.to_string()).await).into_response()
}

fn validate_working_params(
    params: WorkingParams,
    user_id: Uuid,
) -> Result<(String, String, Option<Uuid>, Option<Uuid>), Response> {
    let work_type = params.work_type.trim().to_string();
    if !matches!(work_type.as_str(), "" | "issue" | "autopilot" | "chat") {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid type: must be issue, autopilot, or chat",
        ));
    }

    let scope = params.scope.trim();
    let mut relation = params.relation.trim().to_string();
    let member_id = match scope {
        "" => {
            if !relation.is_empty() {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "relation requires scope=mine",
                ));
            }
            None
        }
        "mine" => {
            if work_type != "issue" {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "scope=mine requires type=issue",
                ));
            }
            if relation.is_empty() {
                relation = "any".into();
            }
            if !matches!(
                relation.as_str(),
                "assigned" | "created" | "involved" | "any"
            ) {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid relation: must be assigned, created, involved, or any",
                ));
            }
            Some(user_id)
        }
        _ => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "invalid scope: must be mine",
            ))
        }
    };

    let parent = params.parent.trim();
    let parent_id = if parent.is_empty() {
        None
    } else {
        if work_type != "issue" {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "parent requires type=issue",
            ));
        }
        if !scope.is_empty() {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "parent cannot be combined with scope",
            ));
        }
        Some(
            Uuid::parse_str(parent)
                .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid parent"))?,
        )
    };

    Ok((work_type, relation, member_id, parent_id))
}

async fn working_agents(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Query(params): Query<WorkingParams>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(workspace_id) => workspace_id,
        Err(response) => return response,
    };
    let (work_type, relation, member_id, parent_id) =
        match validate_working_params(params, context.member.user_id) {
            Ok(values) => values,
            Err(response) => return response,
        };
    let rows = match agent::list_workspace_working_agents(
        &state.pool,
        workspace_id,
        &work_type,
        &relation,
        member_id,
        parent_id,
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, %workspace_id, "failed to list workspace working agents");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list workspace working agents",
            );
        }
    };
    let allowed = match accessible_agent_ids(&state, &context, &headers, workspace_id).await {
        Ok(allowed) => allowed,
        Err(response) => return response,
    };
    Json(
        rows.into_iter()
            .filter_map(|row| {
                let id = row.id?;
                allowed.contains(&id).then_some(WorkingAgent {
                    id: id.to_string(),
                    name: row.name,
                    // HandlerState has no storage signer yet; this is the same
                    // raw URL branch used by the Go handler when none is wired.
                    avatar_url: row.avatar_url,
                    running_task_count: row.running_task_count,
                    issue_ids: row
                        .issue_ids
                        .unwrap_or_default()
                        .into_iter()
                        .map(|id| id.to_string())
                        .collect(),
                })
            })
            .collect::<Vec<_>>(),
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    fn fixture_agent(owner_id: Uuid, permission_mode: &str) -> Agent {
        Agent {
            id: Uuid::now_v7(),
            workspace_id: Uuid::now_v7(),
            name: "Agent".into(),
            avatar_url: None,
            runtime_mode: "local".into(),
            runtime_config: serde_json::json!({}),
            visibility: "private".into(),
            status: "idle".into(),
            max_concurrent_tasks: 1,
            owner_id: Some(owner_id),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            description: String::new(),
            runtime_id: None,
            instructions: String::new(),
            archived_at: None,
            archived_by: None,
            custom_env: serde_json::json!({}),
            custom_args: serde_json::json!([]),
            mcp_config: Some(serde_json::json!({})),
            model: None,
            thinking_level: None,
            composio_toolkit_allowlist: None,
            permission_mode: permission_mode.into(),
            kind: "user".into(),
            system_key: None,
            disabled_runtime_skills: serde_json::json!([]),
            service_tier: None,
        }
    }

    fn target(agent_id: Uuid, target_type: &str, target_id: Uuid) -> AgentInvocationTarget {
        AgentInvocationTarget {
            agent_id,
            created_at: Utc::now(),
            created_by: None,
            id: Uuid::now_v7(),
            target_id,
            target_type: target_type.into(),
        }
    }

    #[test]
    fn visibility_matches_go_owner_admin_and_public_to_rules() {
        let owner = Uuid::now_v7();
        let member = Uuid::now_v7();
        let private = fixture_agent(owner, "private");
        assert!(member_can_view(&private, &[], owner, "member"));
        assert!(member_can_view(&private, &[], member, "admin"));
        assert!(!member_can_view(&private, &[], member, "member"));

        let public = fixture_agent(owner, "public_to");
        assert!(member_can_view(
            &public,
            &[target(public.id, "workspace", Uuid::now_v7())],
            member,
            "member"
        ));
        assert!(member_can_view(
            &public,
            &[target(public.id, "member", member)],
            member,
            "member"
        ));
        assert!(!member_can_view(
            &public,
            &[target(public.id, "member", Uuid::now_v7())],
            member,
            "member"
        ));
        assert!(!member_can_view(
            &public,
            &[target(public.id, "team", member)],
            member,
            "member"
        ));
    }

    #[test]
    fn working_params_match_go_validation_contract() {
        let user_id = Uuid::now_v7();
        let params = WorkingParams {
            work_type: " issue ".into(),
            scope: " mine ".into(),
            relation: String::new(),
            parent: String::new(),
        };
        let (work_type, relation, member_id, parent_id) =
            validate_working_params(params, user_id).unwrap();
        assert_eq!(work_type, "issue");
        assert_eq!(relation, "any");
        assert_eq!(member_id, Some(user_id));
        assert_eq!(parent_id, None);

        let invalid = WorkingParams {
            work_type: "chat".into(),
            scope: "mine".into(),
            ..Default::default()
        };
        assert_eq!(
            validate_working_params(invalid, user_id)
                .unwrap_err()
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
}
