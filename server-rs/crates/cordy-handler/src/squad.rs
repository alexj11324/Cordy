//! Workspace squad read handlers.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Duration, Utc};
use cordy_db::models::{Squad, SquadMember};
use cordy_db::queries::{squad, workspace};
use cordy_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/squads", get(list))
        .route("/api/squads/", get(list))
        .route("/api/squads/{id}", get(get_one))
        .route("/api/squads/{id}/", get(get_one))
        .route(
            "/api/squads/{id}/members",
            get(list_members).delete(remove_member),
        )
        .route(
            "/api/squads/{id}/members/",
            get(list_members).delete(remove_member),
        )
        .route(
            "/api/squads/{id}/members/role",
            axum::routing::patch(update_member_role),
        )
        .route(
            "/api/squads/{id}/members/role/",
            axum::routing::patch(update_member_role),
        )
        .route("/api/squads/{id}/members/status", get(list_member_status))
        .route("/api/squads/{id}/members/status/", get(list_member_status))
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SquadMemberPreviewResponse {
    member_type: String,
    member_id: String,
    role: String,
}

#[derive(Debug, Default)]
struct SquadMemberSummary {
    count: usize,
    preview: Vec<SquadMemberPreviewResponse>,
}

#[derive(Debug, Serialize)]
struct SquadResponse {
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
    member_preview: Vec<SquadMemberPreviewResponse>,
}

impl From<Squad> for SquadResponse {
    fn from(value: Squad) -> Self {
        Self {
            id: value.id.to_string(),
            workspace_id: value.workspace_id.to_string(),
            name: value.name,
            description: value.description,
            instructions: value.instructions,
            // HandlerState does not yet carry the Go object-store signer. This
            // is the same raw-URL branch Go uses when storage is disabled.
            avatar_url: value.avatar_url,
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
    summary: &mut SquadMemberSummary,
    member_type: String,
    member_id: Option<Uuid>,
    role: String,
) {
    summary.count += 1;
    if summary.preview.len() < 3 {
        summary.preview.push(SquadMemberPreviewResponse {
            member_type,
            member_id: member_id.map(|id| id.to_string()).unwrap_or_default(),
            role,
        });
    }
}

fn apply_summary(response: &mut SquadResponse, summary: Option<SquadMemberSummary>) {
    if let Some(summary) = summary {
        response.member_count = summary.count;
        response.member_preview = summary.preview;
    }
}

#[derive(Debug, Serialize)]
struct SquadMemberResponse {
    id: String,
    squad_id: String,
    member_type: String,
    member_id: String,
    role: String,
    created_at: String,
}

impl From<SquadMember> for SquadMemberResponse {
    fn from(value: SquadMember) -> Self {
        Self {
            id: value.id.to_string(),
            squad_id: value.squad_id.to_string(),
            member_type: value.member_type,
            member_id: value.member_id.to_string(),
            role: value.role,
            created_at: crate::timefmt::rfc3339(value.created_at),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct SquadActiveIssueBrief {
    issue_id: String,
    identifier: String,
    title: String,
    issue_status: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct SquadMemberStatusResponse {
    member_type: String,
    member_id: String,
    status: Option<String>,
    active_issues: Vec<SquadActiveIssueBrief>,
    last_active_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct SquadMemberStatusListResponse {
    members: Vec<SquadMemberStatusResponse>,
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

fn squad_id(raw: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(raw.trim())
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid squad id"))
}

async fn load_squad(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_id: &str,
) -> Result<Squad, Response> {
    let id = squad_id(raw_id)?;
    let workspace_id = workspace_id(context)?;
    squad::get_squad_in_workspace(&state.pool, id, workspace_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "squad not found"))
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let squads = match squad::list_squads(&state.pool, workspace_id).await {
        Ok(squads) => squads,
        Err(error) => {
            tracing::warn!(%error, %workspace_id, "failed to list squads");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list squads");
        }
    };
    let rows = match squad::list_squad_member_preview_rows(&state.pool, workspace_id).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, %workspace_id, "failed to list squad member preview");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list squad member preview",
            );
        }
    };
    let mut summaries = HashMap::<Uuid, SquadMemberSummary>::new();
    for row in rows {
        if let Some(id) = row.squad_id {
            add_preview(
                summaries.entry(id).or_default(),
                row.member_type,
                row.member_id,
                row.role,
            );
        }
    }
    let response = squads
        .into_iter()
        .map(|value| {
            let id = value.id;
            let mut response = SquadResponse::from(value);
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
    let found = match load_squad(&state, &context, &raw_id).await {
        Ok(squad) => squad,
        Err(response) => return response,
    };
    let rows = match squad::list_squad_member_preview_rows_by_squad(&state.pool, found.id).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, squad_id = %found.id, "failed to load squad member preview");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load squad member preview",
            );
        }
    };
    let mut summary = SquadMemberSummary::default();
    for row in rows {
        add_preview(&mut summary, row.member_type, row.member_id, row.role);
    }
    let mut response = SquadResponse::from(found);
    apply_summary(&mut response, Some(summary));
    Json(response).into_response()
}

async fn list_members(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let found = match load_squad(&state, &context, &raw_id).await {
        Ok(squad) => squad,
        Err(response) => return response,
    };
    match squad::list_squad_members(&state.pool, found.id).await {
        Ok(members) => Json(
            members
                .into_iter()
                .map(SquadMemberResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, squad_id = %found.id, "failed to list squad members");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list squad members",
            )
        }
    }
}

fn can_manage(context: &WorkspaceContext, squad: &Squad) -> bool {
    matches!(context.member.role.as_str(), "owner" | "admin")
        || context.member.user_id == squad.creator_id
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

fn publish_squad_updated(state: &HandlerState, context: &WorkspaceContext, squad_id: Uuid) {
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_SQUAD_UPDATED.into(),
        workspace_id: context.member.workspace_id.to_string(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload: json!({ "squad_id": squad_id }),
        ..Default::default()
    });
}

async fn remove_member(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    body: Bytes,
) -> Response {
    let found = match load_squad(&state, &context, &raw_id).await {
        Ok(squad) => squad,
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
            "cannot remove the squad leader; change leader first",
        );
    }
    match squad::remove_squad_member(&state.pool, found.id, &request.member_type, member_id).await {
        Ok(0) => error_response(StatusCode::NOT_FOUND, "squad member not found"),
        Ok(_) => {
            publish_squad_updated(&state, &context, found.id);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => {
            tracing::warn!(%error, squad_id = %found.id, "failed to remove squad member");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to remove squad member",
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
    let found = match load_squad(&state, &context, &raw_id).await {
        Ok(squad) => squad,
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
    match squad::update_squad_member_role(
        &state.pool,
        found.id,
        &request.member_type,
        member_id,
        &request.role,
    )
    .await
    {
        Ok(Some(member)) => {
            publish_squad_updated(&state, &context, found.id);
            Json(SquadMemberResponse::from(member)).into_response()
        }
        Ok(None) | Err(_) => error_response(StatusCode::NOT_FOUND, "squad member not found"),
    }
}

struct MemberStatusAccumulator {
    response: SquadMemberStatusResponse,
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
    let found = match load_squad(&state, &context, &raw_id).await {
        Ok(squad) => squad,
        Err(response) => return response,
    };
    let rows = match squad::list_squad_member_status_rows(&state.pool, found.id).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, squad_id = %found.id, "failed to list squad member status");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list squad member status",
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
                response: SquadMemberStatusResponse {
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
                entry.response.active_issues.push(SquadActiveIssueBrief {
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
    Json(SquadMemberStatusListResponse { members }).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_counts_every_member_and_keeps_only_three() {
        let mut summary = SquadMemberSummary::default();
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
}
