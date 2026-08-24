//! Workspace quick-action catalog management.

use std::collections::{HashMap, HashSet};

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::models::{Agent, QuickAction, Squad};
use cordy_db::queries::{agent, agent_invocation_target, quick_action as quick_action_q, squad};
use cordy_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const MAX_ACTIVE: i64 = 30;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/quick-actions", get(list).post(create))
        .route("/api/quick-actions/", get(list).post(create))
        .route(
            "/api/quick-actions/{id}",
            axum::routing::patch(update).delete(delete),
        )
        .route(
            "/api/quick-actions/{id}/",
            axum::routing::patch(update).delete(delete),
        )
}

#[derive(Debug, Serialize)]
struct QuickActionResponse {
    id: Uuid,
    workspace_id: Uuid,
    name: String,
    description: String,
    assignee_type: String,
    assignee_id: Uuid,
    prompt: String,
    visibility: String,
    status: String,
    last_used_at: Option<String>,
    use_count: i64,
    created_by_id: Uuid,
    created_at: String,
    updated_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    target_name: String,
    target_public: bool,
    target_missing: bool,
}

#[derive(Debug, Default, Deserialize)]
struct ListParams {
    include_archived: Option<bool>,
}

fn ids(context: &WorkspaceContext) -> Result<(Uuid, Uuid), Response> {
    let workspace_id = Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "workspace not found"))?;
    Ok((workspace_id, context.member.user_id))
}

fn management_allowed(headers: &HeaderMap) -> Result<(), Response> {
    if headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        == Some("task_token")
    {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "agents cannot manage quick actions",
        ));
    }
    Ok(())
}

fn require_public_role(context: &WorkspaceContext) -> Result<(), Response> {
    if matches!(context.member.role.as_str(), "owner" | "admin") {
        Ok(())
    } else {
        Err(error_response(
            StatusCode::FORBIDDEN,
            "insufficient workspace role",
        ))
    }
}

fn validate_name(raw: &str) -> Result<String, Response> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(error_response(StatusCode::BAD_REQUEST, "name is required"));
    }
    if value.chars().count() > 32 {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "name must be at most 32 characters",
        ));
    }
    Ok(value.to_string())
}

fn validate_description(raw: &str) -> Result<String, Response> {
    let value = raw.trim();
    if value.chars().count() > 200 {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "description must be at most 200 characters",
        ));
    }
    Ok(value.to_string())
}

fn validate_prompt(raw: &str) -> Result<String, Response> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "prompt is required",
        ));
    }
    if value.chars().count() > 4_000 {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "prompt must be at most 4000 characters",
        ));
    }
    if let Some(start) = value.find("{{") {
        if value[start + 2..].contains("}}") {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "template variables are not supported yet",
            ));
        }
    }
    if [
        "mention://agent/",
        "mention://squad/",
        "mention://member/",
        "mention://all/",
    ]
    .iter()
    .any(|needle| value.contains(needle))
    {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "the prompt cannot @mention an agent, squad, or person; a quick action reaches exactly the one target it is bound to (an issue link is fine)",
        ));
    }
    Ok(value.to_string())
}

fn visibility(raw: Option<&str>) -> Result<String, Response> {
    let value = raw
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("public");
    if matches!(value, "public" | "private") {
        Ok(value.to_string())
    } else {
        Err(error_response(
            StatusCode::BAD_REQUEST,
            "visibility must be \"public\" or \"private\"",
        ))
    }
}

pub(crate) async fn target(
    state: &HandlerState,
    workspace_id: Uuid,
    assignee_type: &str,
    assignee_id: Uuid,
) -> Result<(String, Agent, bool), Response> {
    let (name, agent) = match assignee_type {
        "agent" => {
            let agent = agent::get_agent_in_workspace(&state.pool, assignee_id, workspace_id)
                .await
                .map_err(|error| db_error(error, "failed to resolve quick action target"))?
                .filter(|agent| agent.archived_at.is_none())
                .ok_or_else(|| {
                    error_response(
                        StatusCode::BAD_REQUEST,
                        "assignee not found in this workspace",
                    )
                })?;
            (agent.name.clone(), agent)
        }
        "squad" => {
            let squad = squad::get_squad_in_workspace(&state.pool, assignee_id, workspace_id)
                .await
                .map_err(|error| db_error(error, "failed to resolve quick action target"))?
                .filter(|squad| squad.archived_at.is_none())
                .ok_or_else(|| {
                    error_response(
                        StatusCode::BAD_REQUEST,
                        "assignee not found in this workspace",
                    )
                })?;
            let leader = agent::get_agent_in_workspace(&state.pool, squad.leader_id, workspace_id)
                .await
                .map_err(|error| db_error(error, "failed to resolve quick action target"))?
                .filter(|agent| agent.archived_at.is_none())
                .ok_or_else(|| {
                    error_response(
                        StatusCode::BAD_REQUEST,
                        "assignee not found in this workspace",
                    )
                })?;
            (squad.name, leader)
        }
        _ => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "assignee_type must be \"agent\" or \"squad\"",
            ));
        }
    };
    let is_public = if agent.permission_mode == "public_to" {
        agent_invocation_target::list_agent_invocation_targets(&state.pool, agent.id)
            .await
            .map(|targets| {
                targets
                    .iter()
                    .any(|target| target.target_type == "workspace")
            })
            .unwrap_or(false)
    } else {
        false
    };
    Ok((name, agent, is_public))
}

async fn response_for(state: &HandlerState, action: QuickAction) -> QuickActionResponse {
    let target = target(
        state,
        action.workspace_id,
        &action.assignee_type,
        action.assignee_id,
    )
    .await;
    let (target_name, target_public, target_missing) = match target {
        Ok((name, _, is_public)) => (name, is_public, false),
        Err(_) => (String::new(), false, true),
    };
    response_with_target(action, target_name, target_public, target_missing)
}

fn response_with_target(
    action: QuickAction,
    target_name: String,
    target_public: bool,
    target_missing: bool,
) -> QuickActionResponse {
    QuickActionResponse {
        id: action.id,
        workspace_id: action.workspace_id,
        name: action.name,
        description: action.description,
        assignee_type: action.assignee_type,
        assignee_id: action.assignee_id,
        prompt: action.prompt,
        visibility: action.visibility,
        status: action.status,
        last_used_at: action.last_used_at.map(crate::timefmt::rfc3339),
        use_count: action.use_count,
        created_by_id: action.created_by_id,
        created_at: crate::timefmt::rfc3339(action.created_at),
        updated_at: crate::timefmt::rfc3339(action.updated_at),
        target_name,
        target_public,
        target_missing,
    }
}

struct QuickActionCatalog {
    agents: HashMap<Uuid, Agent>,
    squads: HashMap<Uuid, Squad>,
    public_agents: HashSet<Uuid>,
}

impl QuickActionCatalog {
    async fn load(state: &HandlerState, workspace_id: Uuid) -> Self {
        let agents = agent::list_agents(&state.pool, workspace_id)
            .await
            .unwrap_or_default();
        let agent_ids = agents.iter().map(|agent| agent.id).collect::<Vec<_>>();
        let public_agents = if agent_ids.is_empty() {
            HashSet::new()
        } else {
            agent_invocation_target::list_agent_invocation_targets_by_agent_i_ds(
                &state.pool,
                agent_ids,
            )
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|target| target.target_type == "workspace")
            .map(|target| target.agent_id)
            .collect()
        };
        Self {
            agents: agents.into_iter().map(|agent| (agent.id, agent)).collect(),
            squads: squad::list_squads(&state.pool, workspace_id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|squad| (squad.id, squad))
                .collect(),
            public_agents,
        }
    }

    fn response(&self, action: QuickAction) -> QuickActionResponse {
        let target = if action.assignee_type == "squad" {
            self.squads.get(&action.assignee_id).and_then(|squad| {
                self.agents
                    .get(&squad.leader_id)
                    .map(|agent| (squad.name.clone(), agent))
            })
        } else {
            self.agents
                .get(&action.assignee_id)
                .map(|agent| (agent.name.clone(), agent))
        };
        match target {
            Some((name, agent)) => response_with_target(
                action,
                name,
                agent.permission_mode == "public_to" && self.public_agents.contains(&agent.id),
                false,
            ),
            None => response_with_target(action, String::new(), false, true),
        }
    }
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<ListParams>,
) -> Response {
    let (workspace_id, user_id) = match ids(&context) {
        Ok(ids) => ids,
        Err(response) => return response,
    };
    match quick_action_q::list_quick_actions(
        &state.pool,
        workspace_id,
        params.include_archived.unwrap_or(false),
        user_id,
    )
    .await
    {
        Ok(actions) => {
            let catalog = QuickActionCatalog::load(&state, workspace_id).await;
            let responses = actions
                .into_iter()
                .map(|action| catalog.response(action))
                .collect::<Vec<_>>();
            Json(serde_json::json!({ "quick_actions": responses })).into_response()
        }
        Err(error) => db_error(error, "failed to list quick actions"),
    }
}

#[derive(Debug, Deserialize)]
struct CreateRequest {
    name: String,
    #[serde(default)]
    description: String,
    assignee_type: String,
    assignee_id: String,
    prompt: String,
    #[serde(default)]
    visibility: String,
}

async fn create(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Json(request): Json<CreateRequest>,
) -> Response {
    if let Err(response) = management_allowed(&headers) {
        return response;
    }
    let (workspace_id, user_id) = match ids(&context) {
        Ok(ids) => ids,
        Err(response) => return response,
    };
    let visibility = match visibility(Some(&request.visibility)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if visibility == "public" {
        if let Err(response) = require_public_role(&context) {
            return response;
        }
    }
    let name = match validate_name(&request.name) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let description = match validate_description(&request.description) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let prompt = match validate_prompt(&request.prompt) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Ok(assignee_id) = Uuid::parse_str(&request.assignee_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid assignee_id");
    };
    let (_, _, target_public) =
        match target(&state, workspace_id, &request.assignee_type, assignee_id).await {
            Ok(target) => target,
            Err(response) => return response,
        };
    if visibility == "public" && !target_public {
        return error_response(
            StatusCode::BAD_REQUEST,
            "a public quick action must use an agent every workspace member can trigger; make the agent public or set this action to private",
        );
    }
    if matches!(
        quick_action_q::count_active_quick_actions(&state.pool, workspace_id).await,
        Ok(Some(count)) if count >= MAX_ACTIVE
    ) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "a workspace can have at most 30 active quick actions; archive one first",
        );
    }
    match quick_action_q::create_quick_action(
        &state.pool,
        workspace_id,
        &name,
        &description,
        &request.assignee_type,
        assignee_id,
        &prompt,
        &visibility,
        "member",
        user_id,
    )
    .await
    {
        Ok(Some(action)) => (
            StatusCode::CREATED,
            Json(response_for(&state, action).await),
        )
            .into_response(),
        Ok(None) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create quick action",
        ),
        Err(error) => db_error(error, "failed to create quick action"),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateRequest {
    name: Option<String>,
    description: Option<String>,
    assignee_type: Option<String>,
    assignee_id: Option<String>,
    prompt: Option<String>,
    visibility: Option<String>,
    status: Option<String>,
}

async fn update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<UpdateRequest>,
) -> Response {
    if let Err(response) = management_allowed(&headers) {
        return response;
    }
    let (workspace_id, user_id) = match ids(&context) {
        Ok(ids) => ids,
        Err(response) => return response,
    };
    let Ok(id) = Uuid::parse_str(&id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid quick action id");
    };
    let existing = match quick_action_q::get_quick_action(&state.pool, id, workspace_id).await {
        Ok(Some(action)) if action.visibility != "private" || action.created_by_id == user_id => {
            action
        }
        Ok(_) => return error_response(StatusCode::NOT_FOUND, "quick action not found"),
        Err(error) => return db_error(error, "failed to get quick action"),
    };
    let resulting_visibility = match request.visibility.as_deref() {
        Some(value) => match visibility(Some(value)) {
            Ok(value) => value,
            Err(response) => return response,
        },
        None => existing.visibility.clone(),
    };
    if resulting_visibility == "public" || existing.visibility == "public" {
        if let Err(response) = require_public_role(&context) {
            return response;
        }
    }
    let name = match request.name.as_deref().map(validate_name).transpose() {
        Ok(value) => value,
        Err(response) => return response,
    };
    let description = match request
        .description
        .as_deref()
        .map(validate_description)
        .transpose()
    {
        Ok(value) => value,
        Err(response) => return response,
    };
    let prompt = match request.prompt.as_deref().map(validate_prompt).transpose() {
        Ok(value) => value,
        Err(response) => return response,
    };
    if request
        .status
        .as_deref()
        .is_some_and(|value| !matches!(value, "active" | "archived"))
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "status must be \"active\" or \"archived\"",
        );
    }
    if request.status.as_deref() == Some("active")
        && existing.status != "active"
        && matches!(
            quick_action_q::count_active_quick_actions(&state.pool, workspace_id).await,
            Ok(Some(count)) if count >= MAX_ACTIVE
        )
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "a workspace can have at most 30 active quick actions",
        );
    }
    let resulting_type = request
        .assignee_type
        .as_deref()
        .unwrap_or(&existing.assignee_type);
    let resulting_id = match request.assignee_id.as_deref() {
        Some(value) => match Uuid::parse_str(value) {
            Ok(value) => value,
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid assignee_id"),
        },
        None => existing.assignee_id,
    };
    let (_, _, target_public) =
        match target(&state, workspace_id, resulting_type, resulting_id).await {
            Ok(target) => target,
            Err(response) => return response,
        };
    if resulting_visibility == "public" && !target_public {
        return error_response(
            StatusCode::BAD_REQUEST,
            "a public quick action must use an agent every workspace member can trigger; make the agent public or set this action to private",
        );
    }
    match quick_action_q::update_quick_action(
        &state.pool,
        id,
        workspace_id,
        name.as_deref(),
        description.as_deref(),
        request.assignee_type.as_deref(),
        resulting_id,
        prompt.as_deref(),
        request
            .visibility
            .as_ref()
            .map(|_| resulting_visibility.as_str()),
        request.status.as_deref(),
    )
    .await
    {
        Ok(Some(action)) => Json(response_for(&state, action).await).into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "quick action not found"),
        Err(error) => db_error(error, "failed to update quick action"),
    }
}

async fn delete(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = management_allowed(&headers) {
        return response;
    }
    let (workspace_id, user_id) = match ids(&context) {
        Ok(ids) => ids,
        Err(response) => return response,
    };
    let Ok(id) = Uuid::parse_str(&id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid quick action id");
    };
    let existing = match quick_action_q::get_quick_action(&state.pool, id, workspace_id).await {
        Ok(Some(action)) if action.visibility != "private" || action.created_by_id == user_id => {
            action
        }
        Ok(_) => return error_response(StatusCode::NOT_FOUND, "quick action not found"),
        Err(error) => return db_error(error, "failed to get quick action"),
    };
    if existing.visibility == "public" {
        if let Err(response) = require_public_role(&context) {
            return response;
        }
    }
    match quick_action_q::delete_quick_action(&state.pool, id, workspace_id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => db_error(error, "failed to delete quick action"),
    }
}

fn db_error(error: anyhow::Error, message: &'static str) -> Response {
    tracing::warn!(%error, "{message}");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_preserves_quick_action_safety_contract() {
        assert_eq!(visibility(None).unwrap(), "public");
        assert!(visibility(Some("team")).is_err());
        assert!(validate_prompt("{{issue.title}}").is_err());
        assert!(validate_prompt("[@bot](mention://agent/id)").is_err());
        assert!(validate_prompt("[related](mention://issue/id)").is_ok());
        assert!(validate_name(&"x".repeat(33)).is_err());
    }
}
