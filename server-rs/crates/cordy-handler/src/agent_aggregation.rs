//! Workspace-wide agent activity and run-count projections.

use std::collections::{HashMap, HashSet};

use axum::extract::{Extension, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::models::{Agent, AgentInvocationTarget};
use cordy_db::queries::{agent, agent_invocation_target};
use cordy_middleware::workspace::WorkspaceContext;
use serde::Serialize;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/agent-activity-30d", get(activity_30d))
        .route("/api/agent-run-counts", get(run_counts))
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
}
