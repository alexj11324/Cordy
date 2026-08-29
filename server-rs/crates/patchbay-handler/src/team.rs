//! Workspace team read handlers.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use patchbay_db::models::{Autopilot, Team, TeamMember};
use patchbay_db::queries::{agent, autopilot, member, team, workspace};
use patchbay_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/teams", get(list).post(create))
        .route("/api/teams/", get(list).post(create))
        .route("/api/teams/{id}", get(get_one).put(update).delete(remove))
        .route("/api/teams/{id}/", get(get_one).put(update).delete(remove))
        .route(
            "/api/teams/{id}/members",
            get(list_members).post(add_member).delete(remove_member),
        )
        .route(
            "/api/teams/{id}/members/",
            get(list_members).post(add_member).delete(remove_member),
        )
        .route(
            "/api/teams/{id}/members/role",
            axum::routing::patch(update_member_role),
        )
        .route(
            "/api/teams/{id}/members/role/",
            axum::routing::patch(update_member_role),
        )
        .route("/api/teams/{id}/members/status", get(list_member_status))
        .route("/api/teams/{id}/members/status/", get(list_member_status))
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct TeamMemberPreviewResponse {
    member_type: String,
    member_id: String,
    role: String,
}

#[derive(Debug, Default)]
struct TeamMemberSummary {
    count: usize,
    preview: Vec<TeamMemberPreviewResponse>,
}

#[derive(Debug, Serialize)]
struct TeamResponse {
    id: String,
    workspace_id: String,
    name: String,
    description: String,
    instructions: String,
    avatar_url: Option<String>,
    leader_id: String,
    creator_id: String,
    created_at: String,
    updated_at: String,
    archived_at: Option<String>,
    archived_by: Option<String>,
    member_count: usize,
    member_preview: Vec<TeamMemberPreviewResponse>,
}

impl TeamResponse {
    fn from_state(state: &HandlerState, value: Team) -> Self {
        Self {
            id: value.id.to_string(),
            workspace_id: value.workspace_id.to_string(),
            name: value.name,
            description: value.description,
            instructions: value.instructions,
            avatar_url: value
                .avatar_url
                .map(|raw| crate::avatar::resolve_url(state, &raw)),
            leader_id: value.leader_id.to_string(),
            creator_id: value.creator_id.to_string(),
            created_at: crate::timefmt::rfc3339(value.created_at),
            updated_at: crate::timefmt::rfc3339(value.updated_at),
            archived_at: value.archived_at.map(crate::timefmt::rfc3339),
            archived_by: value.archived_by.map(|id| id.to_string()),
            member_count: 0,
            member_preview: Vec::new(),
        }
    }
}

fn add_preview(
    summary: &mut TeamMemberSummary,
    member_type: String,
    member_id: Option<Uuid>,
    role: String,
) {
    summary.count += 1;
    if summary.preview.len() < 3 {
        summary.preview.push(TeamMemberPreviewResponse {
            member_type,
            member_id: member_id.map(|id| id.to_string()).unwrap_or_default(),
            role,
        });
    }
}

fn apply_summary(response: &mut TeamResponse, summary: Option<TeamMemberSummary>) {
    if let Some(summary) = summary {
        response.member_count = summary.count;
        response.member_preview = summary.preview;
    }
}

async fn response_with_preview(
    state: &HandlerState,
    value: Team,
) -> Result<TeamResponse, anyhow::Error> {
    let rows = team::list_team_member_preview_rows_by_team(&state.pool, value.id).await?;
    let mut summary = TeamMemberSummary::default();
    for row in rows {
        add_preview(&mut summary, row.member_type, row.member_id, row.role);
    }
    let mut response = TeamResponse::from_state(state, value);
    apply_summary(&mut response, Some(summary));
    Ok(response)
}

#[derive(Debug, Serialize)]
struct TeamMemberResponse {
    id: String,
    team_id: String,
    member_type: String,
    member_id: String,
    role: String,
    created_at: String,
}

impl From<TeamMember> for TeamMemberResponse {
    fn from(value: TeamMember) -> Self {
        Self {
            id: value.id.to_string(),
            team_id: value.team_id.to_string(),
            member_type: value.member_type,
            member_id: value.member_id.to_string(),
            role: value.role,
            created_at: crate::timefmt::rfc3339(value.created_at),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct TeamActiveIssueBrief {
    issue_id: String,
    identifier: String,
    title: String,
    issue_status: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct TeamMemberStatusResponse {
    member_type: String,
    member_id: String,
    status: Option<String>,
    active_issues: Vec<TeamActiveIssueBrief>,
    last_active_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct TeamMemberStatusListResponse {
    members: Vec<TeamMemberStatusResponse>,
}

#[derive(Debug, Serialize)]
struct AutopilotEventResponse {
    id: String,
    workspace_id: String,
    title: String,
    description: Option<String>,
    project_id: Option<String>,
    assignee_type: String,
    assignee_id: String,
    status: String,
    pause_reason: Option<String>,
    execution_mode: String,
    issue_title_template: Option<String>,
    created_by_type: String,
    created_by_id: String,
    last_run_at: Option<String>,
    created_at: String,
    updated_at: String,
    subscribers: Vec<serde_json::Value>,
}

impl From<Autopilot> for AutopilotEventResponse {
    fn from(value: Autopilot) -> Self {
        Self {
            id: value.id.to_string(),
            workspace_id: value.workspace_id.to_string(),
            title: value.title,
            description: value.description,
            project_id: value.project_id.map(|id| id.to_string()),
            assignee_type: if value.assignee_type.is_empty() {
                "agent".into()
            } else {
                value.assignee_type
            },
            assignee_id: value.assignee_id.to_string(),
            status: value.status,
            pause_reason: value.pause_reason,
            execution_mode: value.execution_mode,
            issue_title_template: value.issue_title_template,
            created_by_type: value.created_by_type,
            created_by_id: value.created_by_id.to_string(),
            last_run_at: value.last_run_at.map(crate::timefmt::rfc3339),
            created_at: crate::timefmt::rfc3339(value.created_at),
            updated_at: crate::timefmt::rfc3339(value.updated_at),
            subscribers: Vec::new(),
        }
    }
}

fn derive_member_status(
    archived: bool,
    runtime_status: Option<&str>,
    last_seen: Option<DateTime<Utc>>,
    has_working_task: bool,
    now: DateTime<Utc>,
) -> &'static str {
    if archived {
        "archived"
    } else if has_working_task {
        "working"
    } else if runtime_status.is_none() {
        "offline"
    } else if runtime_status == Some("online") {
        "idle"
    } else if last_seen.is_some_and(|seen| now - seen < Duration::minutes(5)) {
        "unstable"
    } else {
        "offline"
    }
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace_id"))
}

fn team_id(raw: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(raw.trim())
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid team id"))
}

async fn load_team(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_id: &str,
) -> Result<Team, Response> {
    let id = team_id(raw_id)?;
    let workspace_id = workspace_id(context)?;
    team::get_team_in_workspace(&state.pool, id, workspace_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "team not found"))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CreateTeamRequest {
    #[serde(deserialize_with = "null_default")]
    name: String,
    #[serde(deserialize_with = "null_default")]
    description: String,
    #[serde(deserialize_with = "null_default")]
    leader_id: String,
    avatar_url: Option<String>,
}

async fn accepted_avatar_url(
    state: &HandlerState,
    value: Option<String>,
    current: Option<&str>,
) -> Result<Option<String>, Response> {
    match value {
        None => Ok(None),
        Some(value) => crate::avatar::accept_url(state, &value, current)
            .await
            .map(Some)
            .map_err(|message| error_response(StatusCode::FORBIDDEN, message)),
    }
}
async fn create(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    body: Bytes,
) -> Response {
    let request = match decode_first::<CreateTeamRequest>(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "name is required");
    }
    if request.leader_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "leader_id is required");
    }
    let leader_id = match Uuid::parse_str(&request.leader_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid leader_id"),
    };
    let leader =
        match agent::get_agent_in_workspace(&state.pool, leader_id, context.member.workspace_id)
            .await
        {
            Ok(Some(agent)) => agent,
            Ok(None) | Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "leader must be a valid agent in this workspace",
                )
            }
        };
    if !crate::task::can_access_agent(&state, &context, &leader, "member", context.member.user_id)
        .await
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "you can only use an agent you have access to as leader",
        );
    }
    let avatar_url = match accepted_avatar_url(&state, request.avatar_url, None).await {
        Ok(avatar_url) => avatar_url,
        Err(response) => return response,
    };
    let created = match team::create_team(
        &state.pool,
        context.member.workspace_id,
        &request.name,
        &request.description,
        leader_id,
        context.member.user_id,
        avatar_url.as_deref(),
    )
    .await
    {
        Ok(Some(team)) => team,
        Ok(None) | Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to create team")
        }
    };
    // Go intentionally treats leader seeding as best-effort after team
    // creation; preserve that contract until the schema/service slice makes
    // creation atomic on both implementations.
    let _ = team::add_team_member(&state.pool, created.id, "agent", leader_id, "leader").await;
    let response = match response_with_preview(&state, created).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "failed to load team member preview");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load team member preview",
            );
        }
    };
    publish_team_event(
        &state,
        &context,
        patchbay_protocol::EVENT_TEAM_CREATED,
        json!({ "team": &response }),
    );
    let analytics = patchbay_analytics::events::team_created(
        &context.member.user_id.to_string(),
        &context.member.workspace_id.to_string(),
        &response.id,
        1,
    );
    patchbay_metrics::business_events::record_event(
        Some(state.analytics.as_ref()),
        state.business_metrics.as_deref(),
        &analytics,
    );
    (StatusCode::CREATED, Json(response)).into_response()
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let teams = match team::list_teams(&state.pool, workspace_id).await {
        Ok(teams) => teams,
        Err(error) => {
            tracing::warn!(%error, %workspace_id, "failed to list teams");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list teams");
        }
    };
    let rows = match team::list_team_member_preview_rows(&state.pool, workspace_id).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, %workspace_id, "failed to list team member preview");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list team member preview",
            );
        }
    };
    let mut summaries = HashMap::<Uuid, TeamMemberSummary>::new();
    for row in rows {
        if let Some(id) = row.team_id {
            add_preview(
                summaries.entry(id).or_default(),
                row.member_type,
                row.member_id,
                row.role,
            );
        }
    }
    let response = teams
        .into_iter()
        .map(|value| {
            let id = value.id;
            let mut response = TeamResponse::from_state(&state, value);
            apply_summary(&mut response, summaries.remove(&id));
            response
        })
        .collect::<Vec<_>>();
    Json(response).into_response()
}

async fn get_one(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let found = match load_team(&state, &context, &raw_id).await {
        Ok(team) => team,
        Err(response) => return response,
    };
    match response_with_preview(&state, found).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to load team member preview");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load team member preview",
            )
        }
    }
}

async fn list_members(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let found = match load_team(&state, &context, &raw_id).await {
        Ok(team) => team,
        Err(response) => return response,
    };
    match team::list_team_members(&state.pool, found.id).await {
        Ok(members) => Json(
            members
                .into_iter()
                .map(TeamMemberResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, team_id = %found.id, "failed to list team members");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list team members",
            )
        }
    }
}

fn can_manage(context: &WorkspaceContext, team: &Team) -> bool {
    matches!(context.member.role.as_str(), "owner" | "admin")
        || context.member.user_id == team.creator_id
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MemberMutationRequest {
    #[serde(deserialize_with = "null_default")]
    member_type: String,
    #[serde(deserialize_with = "null_default")]
    member_id: String,
    #[serde(deserialize_with = "null_default")]
    role: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct UpdateTeamRequest {
    name: Option<String>,
    description: Option<String>,
    instructions: Option<String>,
    leader_id: Option<String>,
    avatar_url: Option<String>,
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

fn decode_first<T>(body: &[u8]) -> Result<T, ()>
where
    T: for<'de> Deserialize<'de> + Default,
{
    let mut values = serde_json::Deserializer::from_slice(body).into_iter::<Option<T>>();
    values
        .next()
        .ok_or(())?
        .map(|value| value.unwrap_or_default())
        .map_err(|_| ())
}

fn publish_team_event(
    state: &HandlerState,
    context: &WorkspaceContext,
    event_type: &str,
    payload: serde_json::Value,
) {
    state.bus.publish(&patchbay_events::Event {
        event_type: event_type.into(),
        workspace_id: context.member.workspace_id.to_string(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload,
        ..Default::default()
    });
}

fn publish_team_updated(state: &HandlerState, context: &WorkspaceContext, team_id: Uuid) {
    publish_team_event(
        state,
        context,
        patchbay_protocol::EVENT_TEAM_UPDATED,
        json!({ "team_id": team_id }),
    );
}

fn unique_violation(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23505")
}

async fn update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    body: Bytes,
) -> Response {
    let existing = match load_team(&state, &context, &raw_id).await {
        Ok(team) => team,
        Err(response) => return response,
    };
    if !can_manage(&context, &existing) {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    let request = match decode_first::<UpdateTeamRequest>(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let avatar_url =
        match accepted_avatar_url(&state, request.avatar_url, existing.avatar_url.as_deref()).await
        {
            Ok(avatar_url) => avatar_url,
            Err(response) => return response,
        };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "failed to start team update transaction");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update team");
        }
    };
    match team::lock_team_for_update(&mut *transaction, existing.id, context.member.workspace_id)
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update team")
        }
    }
    let mut leader_id = None;
    let mut new_leader_runtime_bound = true;
    if let Some(raw_leader_id) = request.leader_id.as_deref() {
        let parsed_leader_id = match Uuid::parse_str(raw_leader_id) {
            Ok(id) => id,
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid leader_id"),
        };
        let new_leader = match agent::lock_agent_for_autopilot_assignment(
            &mut *transaction,
            parsed_leader_id,
            context.member.workspace_id,
        )
        .await
        {
            Ok(Some(agent)) => agent,
            Ok(None) | Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "leader must be a valid agent in this workspace",
                )
            }
        };
        if !crate::task::can_access_agent(
            &state,
            &context,
            &new_leader,
            "member",
            context.member.user_id,
        )
        .await
        {
            return error_response(
                StatusCode::FORBIDDEN,
                "you can only use an agent you have access to as leader",
            );
        }
        let is_member =
            match team::is_team_member(&mut *transaction, existing.id, "agent", parsed_leader_id)
                .await
            {
                Ok(Some(is_member)) => is_member,
                Ok(None) | Err(_) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to update team",
                    )
                }
            };
        if !is_member
            && !matches!(
                team::add_team_member(
                    &mut *transaction,
                    existing.id,
                    "agent",
                    parsed_leader_id,
                    "leader",
                )
                .await,
                Ok(Some(_))
            )
        {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update team");
        }
        new_leader_runtime_bound = new_leader.runtime_id.is_some();
        leader_id = Some(parsed_leader_id);
    }
    let updated = match team::update_team(
        &mut *transaction,
        existing.id,
        request.name.as_deref(),
        request.description.as_deref(),
        leader_id,
        avatar_url.as_deref(),
        request.instructions.as_deref(),
    )
    .await
    {
        Ok(Some(team)) => team,
        Ok(None) | Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update team")
        }
    };
    let paused = if request.leader_id.is_some() && !new_leader_runtime_bound {
        match autopilot::pause_autopilots_by_unrunnable_team(&mut *transaction, existing.id).await {
            Ok(autopilots) => autopilots,
            Err(_) => {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update team")
            }
        }
    } else {
        Vec::new()
    };
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, team_id = %existing.id, "failed to commit team update");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update team");
    }
    let response = match response_with_preview(&state, updated).await {
        Ok(response) => response,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load team member preview",
            )
        }
    };
    publish_team_event(
        &state,
        &context,
        patchbay_protocol::EVENT_TEAM_UPDATED,
        json!({ "team": &response }),
    );
    for autopilot in paused {
        publish_team_event(
            &state,
            &context,
            patchbay_protocol::EVENT_AUTOPILOT_UPDATED,
            json!({ "autopilot": AutopilotEventResponse::from(autopilot) }),
        );
    }
    Json(response).into_response()
}

async fn remove(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let existing = match load_team(&state, &context, &raw_id).await {
        Ok(team) => team,
        Err(response) => return response,
    };
    if !can_manage(&context, &existing) {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if existing.archived_at.is_some() {
        return error_response(StatusCode::BAD_REQUEST, "team is already archived");
    }
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, team_id = %existing.id, "failed to start team archive transaction");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to archive team");
        }
    };
    let locked = match team::lock_team_for_update(
        &mut *transaction,
        existing.id,
        context.member.workspace_id,
    )
    .await
    {
        Ok(Some(locked)) if locked.archived_at.is_none() => locked,
        Ok(Some(_)) => return error_response(StatusCode::BAD_REQUEST, "team is already archived"),
        Ok(None) | Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to archive team")
        }
    };
    if let Err(error) =
        team::transfer_team_assignees(&mut *transaction, locked.id, locked.leader_id).await
    {
        tracing::warn!(%error, team_id = %locked.id, "transfer team assignees failed");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to archive team");
    }
    let transferred_autopilots = match team::transfer_team_autopilots_to_leader(
        &mut *transaction,
        locked.id,
        locked.leader_id,
    )
    .await
    {
        Ok(autopilots) => autopilots,
        Err(error) => {
            tracing::warn!(%error, team_id = %locked.id, "transfer team autopilots failed");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to archive team");
        }
    };
    match team::archive_team(&mut *transaction, locked.id, context.member.user_id).await {
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to archive team")
        }
    }
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, team_id = %locked.id, "failed to commit team archive");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to archive team");
    }
    publish_team_event(
        &state,
        &context,
        patchbay_protocol::EVENT_TEAM_DELETED,
        json!({ "team_id": locked.id, "leader_id": locked.leader_id }),
    );
    for autopilot in transferred_autopilots {
        publish_team_event(
            &state,
            &context,
            patchbay_protocol::EVENT_AUTOPILOT_UPDATED,
            json!({ "autopilot": AutopilotEventResponse::from(autopilot) }),
        );
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn add_member(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    body: Bytes,
) -> Response {
    let found = match load_team(&state, &context, &raw_id).await {
        Ok(team) => team,
        Err(response) => return response,
    };
    if !can_manage(&context, &found) {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    let request = match decode_first::<MemberMutationRequest>(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.member_type != "agent" && request.member_type != "member" {
        return error_response(
            StatusCode::BAD_REQUEST,
            "member_type must be 'agent' or 'member'",
        );
    }
    if request.member_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "member_id is required");
    }
    let member_id = match Uuid::parse_str(&request.member_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid member_id"),
    };
    if request.member_type == "agent" {
        let target = match agent::get_agent_in_workspace(
            &state.pool,
            member_id,
            context.member.workspace_id,
        )
        .await
        {
            Ok(Some(agent)) => agent,
            Ok(None) | Err(_) => {
                return error_response(StatusCode::BAD_REQUEST, "agent not found in this workspace")
            }
        };
        if !crate::task::can_access_agent(
            &state,
            &context,
            &target,
            "member",
            context.member.user_id,
        )
        .await
        {
            return error_response(
                StatusCode::FORBIDDEN,
                "you can only add an agent you have access to",
            );
        }
    } else if member::get_member_by_user_and_workspace(
        &state.pool,
        member_id,
        context.member.workspace_id,
    )
    .await
    .ok()
    .flatten()
    .is_none()
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "member not found in this workspace",
        );
    }
    match team::add_team_member(
        &state.pool,
        found.id,
        &request.member_type,
        member_id,
        &request.role,
    )
    .await
    {
        Ok(Some(team_member)) => {
            publish_team_updated(&state, &context, found.id);
            (
                StatusCode::CREATED,
                Json(TeamMemberResponse::from(team_member)),
            )
                .into_response()
        }
        Err(error) if unique_violation(&error) => {
            error_response(StatusCode::CONFLICT, "member already in team")
        }
        Ok(None) | Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to add team member",
        ),
    }
}

async fn remove_member(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    body: Bytes,
) -> Response {
    let found = match load_team(&state, &context, &raw_id).await {
        Ok(team) => team,
        Err(response) => return response,
    };
    if !can_manage(&context, &found) {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    let request = match decode_first::<MemberMutationRequest>(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let member_id = match Uuid::parse_str(&request.member_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid member_id"),
    };
    if request.member_type == "agent" && found.leader_id == member_id {
        return error_response(
            StatusCode::BAD_REQUEST,
            "cannot remove the team leader; change leader first",
        );
    }
    match team::remove_team_member(&state.pool, found.id, &request.member_type, member_id).await {
        Ok(0) => error_response(StatusCode::NOT_FOUND, "team member not found"),
        Ok(_) => {
            publish_team_updated(&state, &context, found.id);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => {
            tracing::warn!(%error, team_id = %found.id, "failed to remove team member");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to remove team member",
            )
        }
    }
}

async fn update_member_role(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    body: Bytes,
) -> Response {
    let found = match load_team(&state, &context, &raw_id).await {
        Ok(team) => team,
        Err(response) => return response,
    };
    if !can_manage(&context, &found) {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    let request = match decode_first::<MemberMutationRequest>(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let member_id = match Uuid::parse_str(&request.member_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid member_id"),
    };
    match team::update_team_member_role(
        &state.pool,
        found.id,
        &request.member_type,
        member_id,
        &request.role,
    )
    .await
    {
        Ok(Some(member)) => {
            publish_team_updated(&state, &context, found.id);
            Json(TeamMemberResponse::from(member)).into_response()
        }
        Ok(None) | Err(_) => error_response(StatusCode::NOT_FOUND, "team member not found"),
    }
}

struct MemberStatusAccumulator {
    response: TeamMemberStatusResponse,
    archived: bool,
    has_working_task: bool,
    runtime_status: Option<String>,
    runtime_seen_at: Option<DateTime<Utc>>,
    latest_active_at: Option<DateTime<Utc>>,
}

async fn list_member_status(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let found = match load_team(&state, &context, &raw_id).await {
        Ok(team) => team,
        Err(response) => return response,
    };
    let rows = match team::list_team_member_status_rows(&state.pool, found.id).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, team_id = %found.id, "failed to list team member status");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list team member status",
            );
        }
    };
    let prefix = workspace::get_workspace(&state.pool, found.workspace_id)
        .await
        .ok()
        .flatten()
        .map(|workspace| {
            if workspace.issue_prefix.is_empty() {
                crate::issue::legacy_issue_prefix(&workspace.name)
            } else {
                workspace.issue_prefix
            }
        })
        .unwrap_or_default();
    let now = Utc::now();
    let mut order = Vec::<String>::new();
    let mut grouped = HashMap::<String, MemberStatusAccumulator>::new();
    for row in rows {
        let member_id = row.member_id.map(|id| id.to_string()).unwrap_or_default();
        let entry = grouped.entry(member_id.clone()).or_insert_with(|| {
            order.push(member_id.clone());
            MemberStatusAccumulator {
                response: TeamMemberStatusResponse {
                    member_type: row.member_type.clone(),
                    member_id: member_id.clone(),
                    status: None,
                    active_issues: Vec::new(),
                    last_active_at: None,
                },
                archived: row.agent_archived_at.is_some(),
                has_working_task: false,
                runtime_status: row.runtime_status.clone(),
                runtime_seen_at: row.runtime_last_seen_at,
                latest_active_at: None,
            }
        });
        if row.member_type != "agent" {
            continue;
        }
        if row.task_id.is_some() {
            if matches!(row.task_status.as_deref(), Some("dispatched" | "running")) {
                entry.has_working_task = true;
            }
            if let Some(issue_id) = row.task_issue_id {
                entry.response.active_issues.push(TeamActiveIssueBrief {
                    issue_id: issue_id.to_string(),
                    identifier: format!("{}-{}", prefix, row.issue_number.unwrap_or_default()),
                    title: row.issue_title.unwrap_or_default(),
                    issue_status: row.issue_status.unwrap_or_default(),
                });
            }
            if row.task_dispatched_at > entry.latest_active_at {
                entry.latest_active_at = row.task_dispatched_at;
            }
        }
    }
    let members = order
        .into_iter()
        .filter_map(|id| grouped.remove(&id))
        .map(|mut entry| {
            if entry.response.member_type == "agent" {
                entry.response.status = Some(
                    derive_member_status(
                        entry.archived,
                        entry.runtime_status.as_deref(),
                        entry.runtime_seen_at,
                        entry.has_working_task,
                        now,
                    )
                    .to_string(),
                );
                entry.response.last_active_at = entry
                    .latest_active_at
                    .or(entry.runtime_seen_at)
                    .map(crate::timefmt::rfc3339);
            }
            entry.response
        })
        .collect();
    Json(TeamMemberStatusListResponse { members }).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_counts_every_member_and_keeps_only_three() {
        let mut summary = TeamMemberSummary::default();
        for index in 0..4 {
            add_preview(
                &mut summary,
                "agent".into(),
                Some(Uuid::from_u128(index + 1)),
                "worker".into(),
            );
        }
        assert_eq!(summary.count, 4);
        assert_eq!(summary.preview.len(), 3);
    }

    #[test]
    fn status_precedence_matches_go_contract() {
        let now = DateTime::parse_from_rfc3339("2026-05-18T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            derive_member_status(true, Some("online"), Some(now), true, now),
            "archived"
        );
        assert_eq!(
            derive_member_status(false, None, None, true, now),
            "working"
        );
        assert_eq!(
            derive_member_status(false, Some("online"), Some(now), false, now),
            "idle"
        );
        assert_eq!(
            derive_member_status(
                false,
                Some("offline"),
                Some(now - Duration::minutes(2)),
                false,
                now
            ),
            "unstable"
        );
        assert_eq!(
            derive_member_status(
                false,
                Some("offline"),
                Some(now - Duration::hours(2)),
                false,
                now
            ),
            "offline"
        );
        assert_eq!(
            derive_member_status(false, None, None, false, now),
            "offline"
        );
    }

    #[test]
    fn member_mutation_decoder_matches_go_first_value_and_null_defaults() {
        let request = decode_first::<MemberMutationRequest>(
            br#"{"member_type":"agent","member_id":"abc","role":"reviewer"} true"#,
        )
        .unwrap();
        assert_eq!(request.member_type, "agent");
        assert_eq!(request.member_id, "abc");
        assert_eq!(request.role, "reviewer");

        let empty = decode_first::<MemberMutationRequest>(b"null").unwrap();
        assert!(empty.member_type.is_empty());
        assert!(empty.member_id.is_empty());
        assert!(decode_first::<MemberMutationRequest>(b"").is_err());

        let null_fields = decode_first::<MemberMutationRequest>(
            br#"{"member_type":null,"member_id":null,"role":null}"#,
        )
        .unwrap();
        assert!(null_fields.member_type.is_empty());
        assert!(null_fields.member_id.is_empty());
        assert!(null_fields.role.is_empty());
    }

    #[test]
    fn typed_uuid_comparison_protects_leader_for_noncanonical_input() {
        let leader = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f12").unwrap();
        let uppercase = Uuid::parse_str("018F03A0C4D27A37AE4D5AA45DE12F12").unwrap();
        assert_eq!(leader, uppercase);
    }

    #[test]
    fn team_crud_decoders_preserve_go_null_and_first_value_contract() {
        let create = decode_first::<CreateTeamRequest>(
            br#"{"name":null,"description":null,"leader_id":null,"avatar_url":"  emoji:robot  "} false"#,
        )
        .unwrap();
        assert!(create.name.is_empty());
        assert!(create.description.is_empty());
        assert!(create.leader_id.is_empty());
        assert_eq!(create.avatar_url.as_deref(), Some("  emoji:robot  "));

        let update = decode_first::<UpdateTeamRequest>(
            br#"{"name":null,"leader_id":null,"avatar_url":null} true"#,
        )
        .unwrap();
        assert!(update.name.is_none());
        assert!(update.leader_id.is_none());
        assert!(update.avatar_url.is_none());
    }
}
