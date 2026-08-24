//! Authenticated Autopilot management API.
//!
//! Ports the twenty `/api/autopilots` routes from the Go handler. Public
//! webhook ingress remains a separate route/domain.

use std::collections::{HashMap, HashSet};

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use chrono::Utc;
use cordy_db::models::{
    Autopilot, AutopilotCollaborator, AutopilotRun, AutopilotSubscriber, AutopilotTrigger,
    WebhookDelivery,
};
use cordy_db::queries::{
    agent, autopilot as autopilot_q, member, project, squad, webhook_delivery,
};
use cordy_middleware::workspace::WorkspaceContext;
use cordy_protocol::{EVENT_AUTOPILOT_CREATED, EVENT_AUTOPILOT_DELETED, EVENT_AUTOPILOT_UPDATED};
use cordy_service::autopilot::{
    new_request_idempotency_key, AutopilotQuotaExceededError, AutopilotService,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const WRITE_DENIED: &str =
    "only the autopilot creator, a workspace admin, or a granted collaborator can manage this autopilot";
const ACCESS_DENIED: &str = "only the autopilot creator or a workspace admin can manage access";

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route(
            "/api/autopilots",
            get(list_autopilots).post(create_autopilot),
        )
        .route("/api/autopilots/cron-preview", get(cron_preview))
        .route("/api/autopilots/usage", get(quota_usage))
        .route(
            "/api/autopilots/{id}",
            get(get_autopilot)
                .patch(update_autopilot)
                .delete(delete_autopilot),
        )
        .route("/api/autopilots/{id}/trigger", post(trigger_autopilot))
        .route("/api/autopilots/{id}/runs", get(list_runs))
        .route("/api/autopilots/{id}/runs/{run_id}", get(get_run))
        .route("/api/autopilots/{id}/deliveries", get(list_deliveries))
        .route(
            "/api/autopilots/{id}/deliveries/{delivery_id}",
            get(get_delivery),
        )
        .route(
            "/api/autopilots/{id}/deliveries/{delivery_id}/replay",
            post(replay_delivery),
        )
        .route("/api/autopilots/{id}/triggers", post(create_trigger))
        .route(
            "/api/autopilots/{id}/triggers/{trigger_id}",
            axum::routing::patch(update_trigger).delete(delete_trigger),
        )
        .route(
            "/api/autopilots/{id}/triggers/{trigger_id}/rotate-webhook-token",
            post(rotate_webhook_token),
        )
        .route(
            "/api/autopilots/{id}/triggers/{trigger_id}/signing-secret",
            axum::routing::put(set_signing_secret),
        )
        .route("/api/autopilots/{id}/collaborators", post(add_collaborator))
        .route(
            "/api/autopilots/{id}/collaborators/{user_id}",
            axum::routing::delete(remove_collaborator),
        )
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace id"))
}

fn parse_id(raw: &str, field: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(raw.trim())
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, &format!("invalid {field}")))
}

fn decode_first<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, Response> {
    let mut decoder = serde_json::Deserializer::from_slice(body);
    T::deserialize(&mut decoder)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid request body"))
}

fn service(state: &HandlerState) -> AutopilotService {
    state.autopilots.as_ref().clone()
}

fn publish(state: &HandlerState, event_type: &str, context: &WorkspaceContext, payload: Value) {
    state.bus.publish(&cordy_events::Event {
        event_type: event_type.into(),
        workspace_id: context.workspace_id.clone(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload,
        ..Default::default()
    });
}

async fn load_autopilot(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_id: &str,
) -> Result<Autopilot, Response> {
    let id = parse_id(raw_id, "autopilot id")?;
    let workspace_id = workspace_id(context)?;
    match autopilot_q::get_autopilot_in_workspace(&state.pool, id, workspace_id).await {
        Ok(Some(autopilot)) => Ok(autopilot),
        Ok(None) | Err(_) => Err(error_response(StatusCode::NOT_FOUND, "autopilot not found")),
    }
}

fn ownership_write(autopilot: &Autopilot, context: &WorkspaceContext) -> bool {
    matches!(context.member.role.as_str(), "owner" | "admin")
        || (autopilot.created_by_type == "member"
            && autopilot.created_by_id == context.member.user_id)
}

async fn can_write(
    state: &HandlerState,
    context: &WorkspaceContext,
    autopilot: &Autopilot,
) -> bool {
    if ownership_write(autopilot, context) {
        return true;
    }
    matches!(
        autopilot_q::is_autopilot_collaborator(&state.pool, autopilot.id, context.member.user_id,)
            .await,
        Ok(Some(true))
    )
}

async fn require_write(
    state: &HandlerState,
    context: &WorkspaceContext,
    autopilot: &Autopilot,
) -> Result<(), Response> {
    if can_write(state, context, autopilot).await {
        Ok(())
    } else {
        Err(error_response(StatusCode::FORBIDDEN, WRITE_DENIED))
    }
}

fn subscriber_entry(subscriber: &AutopilotSubscriber) -> Value {
    json!({
        "user_type": subscriber.user_type,
        "user_id": subscriber.user_id.to_string(),
        "created_at": crate::timefmt::rfc3339(subscriber.created_at),
    })
}

fn collaborator_entry(collaborator: &AutopilotCollaborator) -> Value {
    json!({
        "user_type": collaborator.user_type,
        "user_id": collaborator.user_id.to_string(),
        "granted_by": collaborator.granted_by.to_string(),
        "created_at": crate::timefmt::rfc3339(collaborator.created_at),
    })
}

async fn collaborators_response(
    state: &HandlerState,
    autopilot_id: Uuid,
    status: StatusCode,
) -> Response {
    match autopilot_q::list_autopilot_collaborators(&state.pool, autopilot_id).await {
        Ok(collaborators) => (
            status,
            Json(json!({
                "collaborators": collaborators.iter().map(collaborator_entry).collect::<Vec<_>>()
            })),
        )
            .into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to load collaborators",
        ),
    }
}

fn autopilot_map(autopilot: &Autopilot, subscribers: &[AutopilotSubscriber]) -> Map<String, Value> {
    let mut value = Map::new();
    value.insert("id".into(), json!(autopilot.id.to_string()));
    value.insert(
        "workspace_id".into(),
        json!(autopilot.workspace_id.to_string()),
    );
    value.insert("title".into(), json!(autopilot.title));
    value.insert("description".into(), json!(autopilot.description));
    value.insert(
        "project_id".into(),
        json!(autopilot.project_id.map(|id| id.to_string())),
    );
    value.insert(
        "assignee_type".into(),
        json!(if autopilot.assignee_type.is_empty() {
            "agent"
        } else {
            &autopilot.assignee_type
        }),
    );
    value.insert(
        "assignee_id".into(),
        json!(autopilot.assignee_id.to_string()),
    );
    value.insert("status".into(), json!(autopilot.status));
    value.insert("pause_reason".into(), json!(autopilot.pause_reason));
    value.insert("execution_mode".into(), json!(autopilot.execution_mode));
    value.insert(
        "issue_title_template".into(),
        json!(autopilot.issue_title_template),
    );
    value.insert("created_by_type".into(), json!(autopilot.created_by_type));
    value.insert(
        "created_by_id".into(),
        json!(autopilot.created_by_id.to_string()),
    );
    value.insert(
        "last_run_at".into(),
        json!(autopilot.last_run_at.map(crate::timefmt::rfc3339)),
    );
    value.insert(
        "created_at".into(),
        json!(crate::timefmt::rfc3339(autopilot.created_at)),
    );
    value.insert(
        "updated_at".into(),
        json!(crate::timefmt::rfc3339(autopilot.updated_at)),
    );
    value.insert(
        "subscribers".into(),
        Value::Array(subscribers.iter().map(subscriber_entry).collect()),
    );
    value
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct WebhookEventFilter {
    event: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    actions: Vec<String>,
}

fn validate_event_filters(filters: &[WebhookEventFilter]) -> Result<(), Response> {
    for (i, filter) in filters.iter().enumerate() {
        if filter.event.trim().is_empty() {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                &format!("event_filters[{i}].event must not be empty"),
            ));
        }
        for (j, action) in filter.actions.iter().enumerate() {
            if action.trim().is_empty() {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    &format!("event_filters[{i}].actions[{j}] must not be empty"),
                ));
            }
        }
    }
    Ok(())
}

fn trigger_map(trigger: &AutopilotTrigger, public_url: &str, expose_token: bool) -> Value {
    let is_webhook = trigger.kind == "webhook";
    let token = expose_token
        .then(|| trigger.webhook_token.clone())
        .flatten();
    let path = token
        .as_ref()
        .map(|token| format!("/api/webhooks/autopilots/{token}"));
    let url = path.as_ref().and_then(|path| {
        let base = public_url.trim().trim_end_matches('/');
        (!base.is_empty()).then(|| format!("{base}{path}"))
    });
    let filters = trigger
        .event_filters
        .clone()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let hint = trigger.signing_secret.as_deref().and_then(|secret| {
        let suffix = secret.chars().rev().take(4).collect::<Vec<_>>();
        (suffix.len() == 4).then(|| suffix.into_iter().rev().collect::<String>())
    });
    let mut value = json!({
        "id": trigger.id.to_string(),
        "autopilot_id": trigger.autopilot_id.to_string(),
        "kind": trigger.kind,
        "enabled": trigger.enabled,
        "cron_expression": trigger.cron_expression,
        "timezone": trigger.timezone,
        "next_run_at": trigger.next_run_at.map(crate::timefmt::rfc3339),
        "webhook_token": token,
        "webhook_path": path,
        "webhook_url": url,
        "provider": is_webhook.then(|| if trigger.provider.is_empty() { "generic" } else { &trigger.provider }),
        "has_signing_secret": is_webhook && trigger.signing_secret.as_ref().is_some_and(|secret| !secret.is_empty()),
        "signing_secret_hint": is_webhook.then_some(hint).flatten(),
        "label": trigger.label,
        "last_fired_at": trigger.last_fired_at.map(crate::timefmt::rfc3339),
        "created_at": crate::timefmt::rfc3339(trigger.created_at),
        "updated_at": crate::timefmt::rfc3339(trigger.updated_at),
    });
    if is_webhook && !filters.is_empty() {
        value["event_filters"] = Value::Array(filters);
    }
    value
}

fn run_map(run: &AutopilotRun, include_payload: bool) -> Value {
    json!({
        "id": run.id.to_string(),
        "autopilot_id": run.autopilot_id.to_string(),
        "trigger_id": run.trigger_id.map(|id| id.to_string()),
        "source": run.source,
        "status": run.status,
        "issue_id": run.issue_id.map(|id| id.to_string()),
        "task_id": run.task_id.map(|id| id.to_string()),
        "triggered_at": crate::timefmt::rfc3339(run.triggered_at),
        "completed_at": run.completed_at.map(crate::timefmt::rfc3339),
        "failure_reason": run.failure_reason,
        "reason_code": run.reason_code,
        "trigger_payload": include_payload.then(|| run.trigger_payload.clone()).flatten(),
        "result": run.result,
        "created_at": crate::timefmt::rfc3339(run.created_at),
    })
}

fn pagination(query: &Pagination) -> (i32, i32) {
    let limit = query
        .limit
        .filter(|limit| *limit > 0)
        .unwrap_or(20)
        .min(100);
    let offset = query.offset.filter(|offset| *offset >= 0).unwrap_or(0);
    (limit, offset)
}

#[derive(Debug, Default, Deserialize)]
struct Pagination {
    limit: Option<i32>,
    offset: Option<i32>,
}

fn rule_summary(autopilot: &Autopilot) -> Value {
    json!({
        "assignee_type": autopilot.assignee_type,
        "assignee_id": autopilot.assignee_id.to_string(),
        "status": autopilot.status,
        "execution_mode": autopilot.execution_mode,
    })
}

async fn insert_rule_version(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    autopilot: &Autopilot,
    publisher: Uuid,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"INSERT INTO autopilot_rule_version
           (autopilot_id, workspace_id, published_by_type, published_by_id, config_summary)
           VALUES ($1,$2,'member',$3,$4)"#,
    )
    .bind(autopilot.id)
    .bind(autopilot.workspace_id)
    .bind(publisher)
    .bind(rule_summary(autopilot))
    .execute(executor)
    .await?;
    Ok(())
}

async fn list_autopilots(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(filters): Query<HashMap<String, String>>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let rows = match autopilot_q::list_autopilots(
        &state.pool,
        workspace_id,
        filters
            .get("status")
            .map(String::as_str)
            .filter(|v| !v.is_empty()),
    )
    .await
    {
        Ok(rows) => rows,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list autopilots",
            )
        }
    };
    let collaborator_ids =
        autopilot_q::list_autopilot_i_ds_for_collaborator(&state.pool, context.member.user_id)
            .await
            .unwrap_or_default()
            .into_iter()
            .flatten()
            .collect::<HashSet<_>>();
    let autopilots = rows
        .into_iter()
        .map(|row| {
            let mut value = autopilot_map(&row.autopilot, &[]);
            value.insert(
                "trigger_kinds".into(),
                json!(row.trigger_kinds.unwrap_or_default()),
            );
            if let Some(next) = row.next_run_at {
                value.insert("next_run_at".into(), json!(crate::timefmt::rfc3339(next)));
            }
            if !row.last_run_status.is_empty() {
                value.insert("last_run_status".into(), json!(row.last_run_status));
            }
            value.insert(
                "can_write".into(),
                json!(
                    ownership_write(&row.autopilot, &context)
                        || collaborator_ids.contains(&row.autopilot.id)
                ),
            );
            Value::Object(value)
        })
        .collect::<Vec<_>>();
    Json(json!({ "autopilots": autopilots, "total": autopilots.len() })).into_response()
}

async fn get_autopilot(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let autopilot = match load_autopilot(&state, &context, &raw_id).await {
        Ok(autopilot) => autopilot,
        Err(response) => return response,
    };
    let subscribers = autopilot_q::list_autopilot_subscribers(&state.pool, autopilot.id)
        .await
        .unwrap_or_default();
    let can_write = can_write(&state, &context, &autopilot).await;
    let mut autopilot_value = autopilot_map(&autopilot, &subscribers);
    autopilot_value.insert("can_write".into(), json!(can_write));
    autopilot_value.insert(
        "can_manage_access".into(),
        json!(ownership_write(&autopilot, &context)),
    );
    let public_url = std::env::var("CORDY_PUBLIC_URL").unwrap_or_default();
    let triggers = autopilot_q::list_autopilot_triggers(&state.pool, autopilot.id)
        .await
        .unwrap_or_default()
        .iter()
        .map(|trigger| trigger_map(trigger, &public_url, can_write))
        .collect::<Vec<_>>();
    let collaborators = autopilot_q::list_autopilot_collaborators(&state.pool, autopilot.id)
        .await
        .unwrap_or_default()
        .iter()
        .map(collaborator_entry)
        .collect::<Vec<_>>();
    Json(json!({
        "autopilot": Value::Object(autopilot_value),
        "triggers": triggers,
        "collaborators": collaborators,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct SubscriberInput {
    user_type: String,
    user_id: String,
}

#[derive(Debug, Deserialize)]
struct CreateAutopilotRequest {
    title: String,
    description: Option<String>,
    project_id: Option<String>,
    assignee_type: Option<String>,
    assignee_id: String,
    execution_mode: String,
    issue_title_template: Option<String>,
    #[serde(default)]
    subscribers: Vec<SubscriberInput>,
}

async fn validate_subscribers(
    state: &HandlerState,
    workspace_id: Uuid,
    entries: &[SubscriberInput],
) -> Result<Vec<Uuid>, Response> {
    let mut seen = HashSet::new();
    let mut users = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.user_type != "member" {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                &format!("subscribers[{index}].user_type must be 'member'"),
            ));
        }
        let user_id = parse_id(&entry.user_id, &format!("subscribers[{index}].user_id"))?;
        if !seen.insert(user_id) {
            continue;
        }
        if !matches!(
            member::get_member_by_user_and_workspace(&state.pool, user_id, workspace_id).await,
            Ok(Some(_))
        ) {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                &format!("subscribers[{index}] is not a member of this workspace"),
            ));
        }
        users.push(user_id);
    }
    Ok(users)
}

async fn validate_project(
    state: &HandlerState,
    workspace_id: Uuid,
    raw: Option<&str>,
) -> Result<Option<Uuid>, Response> {
    let Some(raw) = raw.filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    let id = parse_id(raw, "project_id")?;
    if !matches!(
        project::get_project_in_workspace(&state.pool, id, workspace_id).await,
        Ok(Some(_))
    ) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "project_id is not a project in this workspace",
        ));
    }
    Ok(Some(id))
}

async fn validate_assignee(
    state: &HandlerState,
    context: &WorkspaceContext,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    kind: &str,
    id: Uuid,
    workspace_id: Uuid,
    active: bool,
) -> Result<(), Response> {
    match kind {
        "agent" => {
            let value = agent::lock_agent_for_autopilot_assignment(&mut **tx, id, workspace_id)
                .await
                .map_err(|_| {
                    error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to validate assignee",
                    )
                })?
                .ok_or_else(|| {
                    error_response(
                        StatusCode::BAD_REQUEST,
                        "assignee_id is not an agent in this workspace",
                    )
                })?;
            if value.archived_at.is_some() {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "assignee agent is archived",
                ));
            }
            if active && value.runtime_id.is_none() {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "assignee agent has no runtime configured",
                ));
            }
        }
        "squad" => {
            let value = squad::lock_squad_for_autopilot_assignment(&mut **tx, id, workspace_id)
                .await
                .map_err(|_| {
                    error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to validate assignee",
                    )
                })?
                .ok_or_else(|| {
                    error_response(
                        StatusCode::BAD_REQUEST,
                        "assignee_id is not a squad in this workspace",
                    )
                })?;
            if value.archived_at.is_some() {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "assignee squad is archived",
                ));
            }
            let leader = agent::lock_agent_for_autopilot_assignment(
                &mut **tx,
                value.leader_id,
                workspace_id,
            )
            .await
            .map_err(|_| {
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to validate squad leader",
                )
            })?
            .ok_or_else(|| {
                error_response(StatusCode::BAD_REQUEST, "squad leader is not available")
            })?;
            if leader.archived_at.is_some() || (active && leader.runtime_id.is_none()) {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "squad leader is not ready for autopilot execution",
                ));
            }
            if !crate::task::can_access_agent(
                state,
                context,
                &leader,
                "member",
                context.member.user_id,
            )
            .await
            {
                return Err(error_response(
                    StatusCode::FORBIDDEN,
                    "cannot assign autopilot to squad with private leader",
                ));
            }
        }
        _ => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "assignee_type must be agent or squad",
            ))
        }
    }
    Ok(())
}

async fn create_autopilot(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    body: Bytes,
) -> Response {
    let req: CreateAutopilotRequest = match decode_first(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if req.title.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "title is required");
    }
    if req.assignee_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "assignee_id is required");
    }
    if !matches!(req.execution_mode.as_str(), "create_issue" | "run_only") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "execution_mode must be create_issue or run_only",
        );
    }
    if let Some(template) = &req.issue_title_template {
        if let Err(message) = cordy_service::autopilot::validate_issue_title_template(template) {
            return error_response(StatusCode::BAD_REQUEST, &message);
        }
    }
    let workspace_id = match workspace_id(&context) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let assignee_id = match parse_id(&req.assignee_id, "assignee_id") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let assignee_type = req
        .assignee_type
        .as_deref()
        .filter(|v| !v.is_empty())
        .unwrap_or("agent");
    let project_id = match validate_project(&state, workspace_id, req.project_id.as_deref()).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let subscribers = match validate_subscribers(&state, workspace_id, &req.subscribers).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut tx = match state.pool.begin().await {
        Ok(v) => v,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create autopilot",
            )
        }
    };
    if let Err(r) = validate_assignee(
        &state,
        &context,
        &mut tx,
        assignee_type,
        assignee_id,
        workspace_id,
        true,
    )
    .await
    {
        return r;
    }
    let created = sqlx::query_as::<_, Autopilot>(r#"INSERT INTO autopilot
        (workspace_id,title,description,project_id,assignee_type,assignee_id,status,execution_mode,issue_title_template,created_by_type,created_by_id)
        VALUES ($1,$2,$3,$4,$5,$6,'active',$7,$8,'member',$9)
        RETURNING *"#)
        .bind(workspace_id).bind(&req.title).bind(&req.description).bind(project_id).bind(assignee_type)
        .bind(assignee_id).bind(&req.execution_mode).bind(&req.issue_title_template).bind(context.member.user_id)
        .fetch_one(&mut *tx).await;
    let autopilot = match created {
        Ok(v) => v,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create autopilot",
            )
        }
    };
    if insert_rule_version(&mut *tx, &autopilot, context.member.user_id)
        .await
        .is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create autopilot",
        );
    }
    for user in subscribers {
        if autopilot_q::add_autopilot_subscriber(&mut *tx, autopilot.id, "member", user)
            .await
            .is_err()
        {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to add autopilot subscriber",
            );
        }
    }
    if tx.commit().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create autopilot",
        );
    }
    let subs = autopilot_q::list_autopilot_subscribers(&state.pool, autopilot.id)
        .await
        .unwrap_or_default();
    let value = Value::Object(autopilot_map(&autopilot, &subs));
    publish(
        &state,
        EVENT_AUTOPILOT_CREATED,
        &context,
        json!({"autopilot": value}),
    );
    (StatusCode::CREATED, Json(value)).into_response()
}

async fn update_autopilot(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    body: Bytes,
) -> Response {
    let previous = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = require_write(&state, &context, &previous).await {
        return r;
    }
    let raw: Map<String, Value> = match decode_first::<Value>(&body) {
        Ok(Value::Object(v)) => v,
        _ => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let string = |name: &str| raw.get(name).and_then(Value::as_str).map(str::to_owned);
    let title = string("title");
    let status = string("status");
    let execution = string("execution_mode");
    if execution
        .as_deref()
        .is_some_and(|v| !matches!(v, "create_issue" | "run_only"))
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "execution_mode must be create_issue or run_only",
        );
    }
    if status
        .as_deref()
        .is_some_and(|v| !matches!(v, "active" | "paused" | "archived"))
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "status must be active, paused, or archived",
        );
    }
    let description = raw
        .get("description")
        .map(|v| v.as_str().map(str::to_owned));
    let template = raw
        .get("issue_title_template")
        .map(|v| v.as_str().map(str::to_owned));
    if let Some(Some(value)) = &template {
        if let Err(m) = cordy_service::autopilot::validate_issue_title_template(value) {
            return error_response(StatusCode::BAD_REQUEST, &m);
        }
    }
    let project_id = match raw.get("project_id") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(value)) => {
            match validate_project(&state, previous.workspace_id, Some(value)).await {
                Ok(value) => Some(value),
                Err(response) => return response,
            }
        }
        Some(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "project_id must be a string or null",
            )
        }
    };
    let type_sent = raw.contains_key("assignee_type");
    let id_sent = raw.contains_key("assignee_id");
    let next_type = string("assignee_type")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| previous.assignee_type.clone());
    let next_id = if id_sent {
        match raw.get("assignee_id").and_then(Value::as_str) {
            Some(v) => match parse_id(v, "assignee_id") {
                Ok(v) => v,
                Err(r) => return r,
            },
            None => return error_response(StatusCode::BAD_REQUEST, "assignee_id cannot be null"),
        }
    } else {
        previous.assignee_id
    };
    if type_sent && !id_sent && next_type != previous.assignee_type {
        return error_response(
            StatusCode::BAD_REQUEST,
            "assignee_id is required when changing assignee_type",
        );
    }
    let subscriber_values = if let Some(value) = raw.get("subscribers") {
        match serde_json::from_value::<Vec<SubscriberInput>>(value.clone()) {
            Ok(v) => match validate_subscribers(&state, previous.workspace_id, &v).await {
                Ok(v) => Some(v),
                Err(r) => return r,
            },
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid subscribers"),
        }
    } else {
        None
    };
    let mut tx = match state.pool.begin().await {
        Ok(v) => v,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update autopilot",
            )
        }
    };
    let active = status.as_deref().unwrap_or(&previous.status) == "active";
    if (type_sent || id_sent || (active && previous.status != "active"))
        && validate_assignee(
            &state,
            &context,
            &mut tx,
            &next_type,
            next_id,
            previous.workspace_id,
            active,
        )
        .await
        .is_err()
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "assignee is not ready for autopilot execution",
        );
    }
    let locked_updated_at = sqlx::query_scalar::<_, chrono::DateTime<Utc>>(
        "SELECT updated_at FROM autopilot WHERE id=$1 FOR UPDATE",
    )
    .bind(previous.id)
    .fetch_optional(&mut *tx)
    .await;
    match locked_updated_at {
        Ok(Some(at)) if at == previous.updated_at => {}
        Ok(Some(_)) => return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "the autopilot changed while it was being edited; reload and try again.",
                "code": "autopilot_update_conflict"
            })),
        )
            .into_response(),
        _ => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update autopilot",
            )
        }
    }
    let updated = sqlx::query_as::<_, Autopilot>(r#"UPDATE autopilot SET
        title=COALESCE($2,title), description=CASE WHEN $3 THEN $4 ELSE description END,
        project_id=CASE WHEN $5 THEN $6 ELSE project_id END, assignee_type=$7, assignee_id=$8,
        status=COALESCE($9,status), execution_mode=COALESCE($10,execution_mode),
        issue_title_template=CASE WHEN $11 THEN $12 ELSE issue_title_template END,
        pause_reason=CASE WHEN COALESCE($9,status)='active' THEN NULL ELSE pause_reason END, updated_at=now()
        WHERE id=$1 RETURNING *"#)
        .bind(previous.id).bind(title).bind(description.is_some()).bind(description.flatten())
        .bind(project_id.is_some()).bind(project_id.flatten()).bind(&next_type).bind(next_id).bind(status).bind(execution)
        .bind(template.is_some()).bind(template.flatten()).fetch_one(&mut *tx).await;
    let updated = match updated {
        Ok(v) => v,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update autopilot",
            )
        }
    };
    if let Some(users) = subscriber_values {
        if autopilot_q::delete_autopilot_subscribers_for_autopilot(&mut *tx, updated.id)
            .await
            .is_err()
        {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update subscribers",
            );
        };
        for user in users {
            if autopilot_q::add_autopilot_subscriber(&mut *tx, updated.id, "member", user)
                .await
                .is_err()
            {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to update subscribers",
                );
            }
        }
    }
    let substantive = previous.assignee_type != updated.assignee_type
        || previous.assignee_id != updated.assignee_id
        || previous.status != updated.status
        || previous.execution_mode != updated.execution_mode
        || previous.description != updated.description
        || previous.issue_title_template != updated.issue_title_template;
    if substantive
        && (insert_rule_version(&mut *tx, &updated, context.member.user_id)
            .await
            .is_err()
            || autopilot_q::set_autopilot_trigger_publishers_by_autopilot(
                &mut *tx,
                updated.id,
                Some("member"),
                context.member.user_id,
            )
            .await
            .is_err())
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to publish autopilot update",
        );
    }
    if tx.commit().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update autopilot",
        );
    }
    let subs = autopilot_q::list_autopilot_subscribers(&state.pool, updated.id)
        .await
        .unwrap_or_default();
    let value = Value::Object(autopilot_map(&updated, &subs));
    publish(
        &state,
        EVENT_AUTOPILOT_UPDATED,
        &context,
        json!({"autopilot":value}),
    );
    Json(value).into_response()
}

async fn delete_autopilot(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let autopilot = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = require_write(&state, &context, &autopilot).await {
        return r;
    }
    let mut tx = match state.pool.begin().await {
        Ok(v) => v,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete autopilot",
            )
        }
    };
    let archived = sqlx::query_as::<_, Autopilot>(
        "UPDATE autopilot SET status='archived', pause_reason=NULL, updated_at=now() WHERE id=$1 RETURNING *",
    )
    .bind(autopilot.id)
    .fetch_one(&mut *tx)
    .await;
    let archived = match archived {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete autopilot",
            )
        }
    };
    if insert_rule_version(&mut *tx, &archived, context.member.user_id)
        .await
        .is_err()
        || tx.commit().await.is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to delete autopilot",
        );
    }
    publish(
        &state,
        EVENT_AUTOPILOT_DELETED,
        &context,
        json!({"autopilot_id":autopilot.id.to_string()}),
    );
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct CollaboratorRequest {
    user_id: String,
}
async fn add_collaborator(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    body: Bytes,
) -> Response {
    let ap = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if !ownership_write(&ap, &context) {
        return error_response(StatusCode::FORBIDDEN, ACCESS_DENIED);
    }
    let req: CollaboratorRequest = match decode_first(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if req.user_id.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "user_id is required");
    }
    let uid = match parse_id(&req.user_id, "user_id") {
        Ok(v) => v,
        Err(r) => return r,
    };
    if !matches!(
        member::get_member_by_user_and_workspace(&state.pool, uid, ap.workspace_id).await,
        Ok(Some(_))
    ) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "user_id must be a member of this workspace",
        );
    }
    match autopilot_q::add_autopilot_collaborator(
        &state.pool,
        ap.id,
        "member",
        uid,
        context.member.user_id,
    )
    .await
    {
        Ok(Some(_)) => {
            publish(
                &state,
                EVENT_AUTOPILOT_UPDATED,
                &context,
                json!({"autopilot_id": ap.id.to_string()}),
            );
            collaborators_response(&state, ap.id, StatusCode::CREATED).await
        }
        _ => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to grant access"),
    }
}
async fn remove_collaborator(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_id, raw_user)): Path<(String, String)>,
) -> Response {
    let ap = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if !ownership_write(&ap, &context) {
        return error_response(StatusCode::FORBIDDEN, ACCESS_DENIED);
    }
    let uid = match parse_id(&raw_user, "user id") {
        Ok(v) => v,
        Err(r) => return r,
    };
    match autopilot_q::delete_autopilot_collaborator(&state.pool, ap.id, "member", uid).await {
        Ok(_) => {
            publish(
                &state,
                EVENT_AUTOPILOT_UPDATED,
                &context,
                json!({"autopilot_id": ap.id.to_string()}),
            );
            collaborators_response(&state, ap.id, StatusCode::OK).await
        }
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to revoke access"),
    }
}

#[derive(Deserialize)]
struct CreateTriggerRequest {
    kind: String,
    cron_expression: Option<String>,
    timezone: Option<String>,
    label: Option<String>,
    provider: Option<String>,
    #[serde(default)]
    event_filters: Vec<WebhookEventFilter>,
}
#[derive(Deserialize)]
struct UpdateTriggerRequest {
    enabled: Option<bool>,
    cron_expression: Option<String>,
    timezone: Option<String>,
    label: Option<String>,
    event_filters: Option<Vec<WebhookEventFilter>>,
}

fn new_webhook_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!(
        "awt_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

fn valid_trigger_kind(kind: &str) -> bool {
    matches!(kind, "schedule" | "webhook")
}
async fn owned_trigger(
    state: &HandlerState,
    ap: &Autopilot,
    raw: &str,
) -> Result<AutopilotTrigger, Response> {
    let id = parse_id(raw, "trigger id")?;
    match autopilot_q::get_autopilot_trigger(&state.pool, id).await {
        Ok(Some(t)) if t.autopilot_id == ap.id => Ok(t),
        _ => Err(error_response(StatusCode::NOT_FOUND, "trigger not found")),
    }
}

async fn create_trigger(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    body: Bytes,
) -> Response {
    let ap = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = require_write(&state, &context, &ap).await {
        return r;
    }
    let req: CreateTriggerRequest = match decode_first(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if !valid_trigger_kind(&req.kind) {
        return error_response(StatusCode::BAD_REQUEST, "kind must be schedule or webhook");
    }
    if req.kind == "webhook"
        && req
            .timezone
            .as_deref()
            .is_some_and(|timezone| !timezone.is_empty())
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "timezone is not valid for webhook triggers",
        );
    }
    if let Err(r) = validate_event_filters(&req.event_filters) {
        return r;
    }
    if req.kind != "webhook" && !req.event_filters.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "event_filters is only valid for webhook triggers",
        );
    }
    if req.kind != "webhook"
        && req
            .provider
            .as_deref()
            .is_some_and(|provider| !provider.is_empty())
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "provider is only valid for webhook triggers",
        );
    }
    let provider = req.provider.as_deref().unwrap_or("generic");
    if req.kind == "webhook" && !matches!(provider, "generic" | "github") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "provider must be generic or github",
        );
    }
    let (cron, tz, next) = if req.kind == "schedule" {
        let c = req
            .cron_expression
            .as_deref()
            .filter(|v| !v.is_empty())
            .ok_or(());
        let t = req
            .timezone
            .as_deref()
            .filter(|v| !v.is_empty())
            .unwrap_or("UTC");
        let c = match c {
            Ok(v) => v,
            Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "cron_expression is required for schedule triggers",
                )
            }
        };
        if cordy_service::cron::validate_timezone(t).is_err() {
            return error_response(StatusCode::BAD_REQUEST, "invalid timezone");
        };
        let next = match cordy_service::cron::compute_next_run(c, t) {
            Ok(v) => v,
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid cron expression"),
        };
        (
            Some(c),
            req.timezone
                .as_deref()
                .filter(|timezone| !timezone.is_empty()),
            Some(next),
        )
    } else {
        (None, None, None)
    };
    let filters = json!(req.event_filters);
    for _ in 0..3 {
        let token = (req.kind == "webhook").then(new_webhook_token);
        let mut tx = match state.pool.begin().await {
            Ok(tx) => tx,
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create trigger",
                )
            }
        };
        let created = autopilot_q::create_autopilot_trigger(
            &mut *tx,
            ap.id,
            &req.kind,
            true,
            cron,
            tz,
            next,
            token.as_deref(),
            req.label.as_deref(),
            (req.kind == "webhook").then_some(provider),
            &filters,
            Some("member"),
            context.member.user_id,
        )
        .await;
        let trigger = match created {
            Ok(Some(trigger)) => trigger,
            Err(_) if req.kind == "webhook" => continue,
            _ => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create trigger",
                )
            }
        };
        if insert_rule_version(&mut *tx, &ap, context.member.user_id)
            .await
            .is_err()
            || tx.commit().await.is_err()
        {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create trigger",
            );
        }
        let value = trigger_map(
            &trigger,
            &std::env::var("CORDY_PUBLIC_URL").unwrap_or_default(),
            true,
        );
        publish(
            &state,
            EVENT_AUTOPILOT_UPDATED,
            &context,
            json!({"autopilot_id": ap.id.to_string(), "trigger": value}),
        );
        return (StatusCode::CREATED, Json(value)).into_response();
    }
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "failed to create trigger",
    )
}

async fn update_trigger(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_id, raw_trigger)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let ap = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = require_write(&state, &context, &ap).await {
        return r;
    }
    let old = match owned_trigger(&state, &ap, &raw_trigger).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let req: UpdateTriggerRequest = match decode_first(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if old.kind != "schedule" && (req.cron_expression.is_some() || req.timezone.is_some()) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "cron_expression and timezone are only valid for schedule triggers",
        );
    }
    if let Some(filters) = &req.event_filters {
        if old.kind != "webhook" {
            return error_response(
                StatusCode::BAD_REQUEST,
                "event_filters are only valid for webhook triggers",
            );
        }
        if let Err(r) = validate_event_filters(filters) {
            return r;
        }
    }
    let cron = req
        .cron_expression
        .as_deref()
        .or(old.cron_expression.as_deref());
    let tz = req
        .timezone
        .as_deref()
        .or(old.timezone.as_deref())
        .unwrap_or("UTC");
    let next = if old.kind == "schedule" {
        let c = match cron {
            Some(v) => v,
            None => return error_response(StatusCode::BAD_REQUEST, "cron_expression is required"),
        };
        if cordy_service::cron::validate_timezone(tz).is_err() {
            return error_response(StatusCode::BAD_REQUEST, "invalid timezone");
        };
        match cordy_service::cron::compute_next_run(c, tz) {
            Ok(v) => Some(v),
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid cron expression"),
        }
    } else {
        old.next_run_at
    };
    let filters = req.event_filters.as_ref().map(|v| json!(v));
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update trigger",
            )
        }
    };
    let updated = sqlx::query_as::<_, AutopilotTrigger>(
        r#"UPDATE autopilot_trigger SET
           enabled=COALESCE($2,enabled), cron_expression=COALESCE($3,cron_expression),
           timezone=COALESCE($4,timezone), next_run_at=$5, label=COALESCE($6,label),
           event_filters=COALESCE($7,event_filters), updated_at=now()
           WHERE id=$1 RETURNING *"#,
    )
    .bind(old.id)
    .bind(req.enabled)
    .bind(req.cron_expression.as_deref())
    .bind(req.timezone.as_deref())
    .bind(next)
    .bind(req.label.as_deref())
    .bind(filters)
    .fetch_one(&mut *tx)
    .await;
    let updated = match updated {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update trigger",
            )
        }
    };
    let substantive = old.enabled != updated.enabled
        || old.cron_expression != updated.cron_expression
        || old.timezone != updated.timezone
        || old.event_filters != updated.event_filters;
    if substantive
        && (insert_rule_version(&mut *tx, &ap, context.member.user_id)
            .await
            .is_err()
            || autopilot_q::set_autopilot_trigger_publisher(
                &mut *tx,
                updated.id,
                Some("member"),
                context.member.user_id,
            )
            .await
            .is_err())
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update trigger",
        );
    }
    if tx.commit().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update trigger",
        );
    }
    let value = trigger_map(
        &updated,
        &std::env::var("CORDY_PUBLIC_URL").unwrap_or_default(),
        true,
    );
    publish(
        &state,
        EVENT_AUTOPILOT_UPDATED,
        &context,
        json!({"autopilot_id": ap.id.to_string(), "trigger": value}),
    );
    Json(value).into_response()
}
async fn delete_trigger(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_id, raw_trigger)): Path<(String, String)>,
) -> Response {
    let ap = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = require_write(&state, &context, &ap).await {
        return r;
    }
    let t = match owned_trigger(&state, &ap, &raw_trigger).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut tx = match state.pool.begin().await {
        Ok(v) => v,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete trigger",
            )
        }
    };
    if autopilot_q::delete_autopilot_trigger(&mut *tx, t.id)
        .await
        .is_err()
        || insert_rule_version(&mut *tx, &ap, context.member.user_id)
            .await
            .is_err()
        || tx.commit().await.is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to delete trigger",
        );
    }
    publish(
        &state,
        EVENT_AUTOPILOT_UPDATED,
        &context,
        json!({"autopilot_id": ap.id.to_string(), "trigger_id": t.id.to_string()}),
    );
    StatusCode::NO_CONTENT.into_response()
}
async fn rotate_webhook_token(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_id, raw_trigger)): Path<(String, String)>,
) -> Response {
    let ap = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = require_write(&state, &context, &ap).await {
        return r;
    }
    let t = match owned_trigger(&state, &ap, &raw_trigger).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if t.kind != "webhook" {
        return error_response(StatusCode::BAD_REQUEST, "trigger is not a webhook trigger");
    }
    for _ in 0..5 {
        let token = new_webhook_token();
        match autopilot_q::rotate_autopilot_trigger_webhook_token(&state.pool, t.id, Some(&token))
            .await
        {
            Ok(Some(v)) => {
                let value = trigger_map(
                    &v,
                    &std::env::var("CORDY_PUBLIC_URL").unwrap_or_default(),
                    true,
                );
                publish(
                    &state,
                    EVENT_AUTOPILOT_UPDATED,
                    &context,
                    json!({"autopilot_id": ap.id.to_string(), "trigger": value}),
                );
                return Json(value).into_response();
            }
            Ok(None) => return error_response(StatusCode::NOT_FOUND, "trigger not found"),
            Err(_) => continue,
        }
    }
    error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "failed to rotate webhook token",
    )
}
#[derive(Deserialize)]
struct SigningSecretRequest {
    signing_secret: String,
}
async fn set_signing_secret(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_id, raw_trigger)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let ap = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = require_write(&state, &context, &ap).await {
        return r;
    }
    let t = match owned_trigger(&state, &ap, &raw_trigger).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if t.kind != "webhook" {
        return error_response(StatusCode::BAD_REQUEST, "trigger is not a webhook trigger");
    }
    let req: SigningSecretRequest = match decode_first(&body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let secret = req.signing_secret.trim();
    if !secret.is_empty() && secret.len() < 16 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "signing_secret must be at least 16 characters",
        );
    };
    match autopilot_q::set_autopilot_trigger_signing_secret(
        &state.pool,
        t.id,
        (!secret.is_empty()).then_some(secret),
    )
    .await
    {
        Ok(Some(v)) => {
            let value = trigger_map(
                &v,
                &std::env::var("CORDY_PUBLIC_URL").unwrap_or_default(),
                true,
            );
            publish(
                &state,
                EVENT_AUTOPILOT_UPDATED,
                &context,
                json!({"autopilot_id": ap.id.to_string(), "trigger": value}),
            );
            Json(value).into_response()
        }
        _ => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update signing secret",
        ),
    }
}

async fn cron_preview(Query(query): Query<HashMap<String, String>>) -> Response {
    let cron = match query.get("expr").filter(|v| !v.is_empty()) {
        Some(v) => v,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"expr is required","code":"invalid_cron"})),
            )
                .into_response()
        }
    };
    let tz = query
        .get("tz")
        .map(String::as_str)
        .filter(|v| !v.is_empty())
        .unwrap_or("UTC");
    if cordy_service::cron::validate_timezone(tz).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid timezone","code":"invalid_timezone"})),
        )
            .into_response();
    }
    let runs = match cordy_service::cron::next_occurrences_after_utc(cron, tz, Utc::now(), 3) {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid cron expression","code":"invalid_cron"})),
            )
                .into_response()
        }
    };
    Json(json!({"next_runs":runs.into_iter().map(crate::timefmt::rfc3339).collect::<Vec<_>>()}))
        .into_response()
}
async fn quota_usage(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let wid = match workspace_id(&context) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match service(&state).quota_usage(wid).await {
        Ok(v) => {
            let action = if v.enabled { v.action } else { "off".into() };
            let blocked_counts = if v.enabled {
                json!(v.blocked_counts)
            } else {
                Value::Null
            };
            Json(json!({
                "action": action,
                "used": v.used,
                "reserved": v.reserved,
                "limit": v.limit,
                "period_start": v.period_start.map(crate::timefmt::rfc3339),
                "period_end": v.period_end.map(crate::timefmt::rfc3339),
                "reset_at": v.reset_at.map(crate::timefmt::rfc3339),
                "blocked_counts": blocked_counts,
            }))
            .into_response()
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to load autopilot quota usage",
        ),
    }
}

async fn list_runs(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    Query(query): Query<Pagination>,
) -> Response {
    let ap = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (limit, offset) = pagination(&query);
    match autopilot_q::list_autopilot_runs(&state.pool, ap.id, limit, offset).await {
        Ok(rows) => {
            let values = rows
                .iter()
                .map(|run| run_map(run, false))
                .collect::<Vec<_>>();
            Json(json!({"runs": values, "total": values.len()})).into_response()
        }
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list runs"),
    }
}
async fn get_run(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_id, raw_run)): Path<(String, String)>,
) -> Response {
    let ap = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let id = match parse_id(&raw_run, "run id") {
        Ok(v) => v,
        Err(r) => return r,
    };
    match autopilot_q::get_autopilot_run(&state.pool, id).await {
        Ok(Some(run)) if run.autopilot_id == ap.id => Json(run_map(&run, true)).into_response(),
        _ => error_response(StatusCode::NOT_FOUND, "run not found"),
    }
}
async fn trigger_autopilot(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let ap = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = require_write(&state, &context, &ap).await {
        return r;
    }
    if ap.status != "active" {
        return error_response(StatusCode::BAD_REQUEST, "autopilot is not active");
    }
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(new_request_idempotency_key);
    if key.len() > 255 {
        return error_response(StatusCode::BAD_REQUEST, "Idempotency-Key is too long");
    };
    let actor_user_id = manual_actor_user_id(&headers, context.member.user_id);
    match service(&state)
        .dispatch_autopilot_manual_with_key(&ap, Uuid::nil(), &Value::Null, actor_user_id, &key)
        .await
    {
        Ok(outcome) => {
            let mut value = run_map(&outcome.run, true);
            if let Some(reason) = outcome.reason_code {
                value["reason_code"] = json!(reason.as_str());
            }
            Json(value).into_response()
        }
        Err(error)
            if error
                .downcast_ref::<AutopilotQuotaExceededError>()
                .is_some() =>
        {
            let quota = error.downcast_ref::<AutopilotQuotaExceededError>().unwrap();
            let retry_after = (quota.reset_at - Utc::now()).num_seconds().max(1);
            let mut response_headers = HeaderMap::new();
            if let Ok(value) = retry_after.to_string().parse() {
                response_headers.insert("retry-after", value);
            }
            (
                StatusCode::TOO_MANY_REQUESTS,
                response_headers,
                Json(json!({
                    "reason_code": "quota_exceeded",
                    "used": quota.used,
                    "reserved": quota.reserved,
                    "limit": quota.limit,
                    "reset_at": crate::timefmt::rfc3339(quota.reset_at),
                })),
            )
                .into_response()
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to trigger autopilot",
        ),
    }
}

fn manual_actor_user_id(headers: &HeaderMap, member_id: Uuid) -> Option<Uuid> {
    (headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        != Some("task_token"))
    .then_some(member_id)
}

fn delivery_map(d: &WebhookDelivery, detail: bool) -> Value {
    let mut value = json!({"id":d.id.to_string(),"workspace_id":d.workspace_id.to_string(),"autopilot_id":d.autopilot_id.to_string(),"trigger_id":d.trigger_id.to_string(),"provider":d.provider,"event":d.event,"dedupe_key":d.dedupe_key,"dedupe_source":d.dedupe_source,"signature_status":d.signature_status,"status":d.status,"attempt_count":d.attempt_count,"dispatch_attempts":d.dispatch_attempts,"available_at":crate::timefmt::rfc3339(d.available_at),"content_type":d.content_type,"response_status":d.response_status,"autopilot_run_id":d.autopilot_run_id.map(|v|v.to_string()),"replayed_from_delivery_id":d.replayed_from_delivery_id.map(|v|v.to_string()),"error":d.error,"reason_code":d.reason_code,"replay_idempotency_key":d.replay_idempotency_key,"received_at":crate::timefmt::rfc3339(d.received_at),"last_attempt_at":crate::timefmt::rfc3339(d.last_attempt_at),"created_at":crate::timefmt::rfc3339(d.created_at)});
    if detail {
        value["selected_headers"] = d.selected_headers.clone();
        if let Some(raw) = &d.raw_body {
            value["raw_body"] = json!(String::from_utf8_lossy(raw))
        }
        if let Some(body) = &d.response_body {
            value["response_body"] = json!(body)
        }
    }
    value
}

fn replay_event(raw: &[u8], headers: &Value) -> Result<String, ()> {
    let raw = raw.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(raw);
    let body: Value = serde_json::from_slice(raw).map_err(|_| ())?;
    if !matches!(body, Value::Object(_) | Value::Array(_)) {
        return Err(());
    }
    let header = |name: &str| {
        headers
            .as_object()
            .and_then(|values| {
                values
                    .get(name)
                    .or_else(|| values.get(&name.to_ascii_lowercase()))
            })
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    if let Some(github) = header("X-GitHub-Event") {
        let action = body
            .get("action")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty());
        return Ok(match action {
            Some(action) => format!("github.{github}.{action}"),
            None => format!("github.{github}"),
        });
    }
    if let Some(gitlab) = header("X-Gitlab-Event") {
        return Ok(format!("gitlab.{gitlab}"));
    }
    if let Some(event) = header("X-Event-Type") {
        return Ok(event.to_owned());
    }
    for field in ["event", "type", "action"] {
        if let Some(event) = body
            .get(field)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            return Ok(event.to_owned());
        }
    }
    Ok("webhook.received".into())
}

fn replay_rejected(delivery: &WebhookDelivery) -> bool {
    delivery.status == "rejected"
        || matches!(delivery.signature_status.as_str(), "invalid" | "rejected")
}
async fn list_deliveries(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    Query(query): Query<Pagination>,
) -> Response {
    let ap = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (limit, offset) = pagination(&query);
    match webhook_delivery::list_webhook_deliveries_by_autopilot(
        &state.pool,
        ap.id,
        ap.workspace_id,
        limit,
        offset,
    )
    .await
    {
        Ok(rows) => {
            let values=rows.into_iter().map(|d|json!({"id":d.id.map(|v|v.to_string()),"workspace_id":d.workspace_id.map(|v|v.to_string()),"autopilot_id":d.autopilot_id.map(|v|v.to_string()),"trigger_id":d.trigger_id.map(|v|v.to_string()),"provider":d.provider,"event":d.event,"dedupe_key":d.dedupe_key,"dedupe_source":d.dedupe_source,"signature_status":d.signature_status,"status":d.status,"attempt_count":d.attempt_count,"dispatch_attempts":d.dispatch_attempts,"available_at":d.available_at.map(crate::timefmt::rfc3339),"content_type":d.content_type,"response_status":d.response_status,"autopilot_run_id":d.autopilot_run_id.map(|v|v.to_string()),"replayed_from_delivery_id":d.replayed_from_delivery_id.map(|v|v.to_string()),"error":d.error,"reason_code":d.reason_code,"replay_idempotency_key":d.replay_idempotency_key,"received_at":d.received_at.map(crate::timefmt::rfc3339),"last_attempt_at":d.last_attempt_at.map(crate::timefmt::rfc3339),"created_at":d.created_at.map(crate::timefmt::rfc3339)})).collect::<Vec<_>>();
            Json(json!({"deliveries": values, "total": values.len()})).into_response()
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list deliveries",
        ),
    }
}
async fn get_delivery(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_id, raw_delivery)): Path<(String, String)>,
) -> Response {
    let ap = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let id = match parse_id(&raw_delivery, "delivery id") {
        Ok(v) => v,
        Err(r) => return r,
    };
    match webhook_delivery::get_webhook_delivery_in_workspace(&state.pool, id, ap.workspace_id)
        .await
    {
        Ok(Some(d)) if d.autopilot_id == ap.id => Json(delivery_map(&d, true)).into_response(),
        _ => error_response(StatusCode::NOT_FOUND, "webhook delivery not found"),
    }
}
async fn replay_delivery(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_id, raw_delivery)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let ap = match load_autopilot(&state, &context, &raw_id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = require_write(&state, &context, &ap).await {
        return r;
    }
    let id = match parse_id(&raw_delivery, "delivery id") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let original =
        match webhook_delivery::get_webhook_delivery_in_workspace(&state.pool, id, ap.workspace_id)
            .await
        {
            Ok(Some(v)) if v.autopilot_id == ap.id => v,
            _ => return error_response(StatusCode::NOT_FOUND, "webhook delivery not found"),
        };
    if replay_rejected(&original) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "cannot replay a delivery that failed signature verification",
        );
    }
    let raw = match original.raw_body.clone() {
        Some(v) => v,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "original delivery has no raw body to replay",
            )
        }
    };
    let event = match replay_event(&raw, &original.selected_headers) {
        Ok(event) => event,
        Err(()) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "stored body no longer parses as a webhook payload",
            )
        }
    };
    if ap.status != "active" {
        return error_response(StatusCode::BAD_REQUEST, "autopilot is not active");
    }
    let trigger = match autopilot_q::get_autopilot_trigger(&state.pool, original.trigger_id).await {
        Ok(Some(v)) if v.autopilot_id == ap.id && v.enabled => v,
        Ok(Some(_)) => return error_response(StatusCode::BAD_REQUEST, "trigger is disabled"),
        _ => return error_response(StatusCode::NOT_FOUND, "trigger not found"),
    };
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(new_request_idempotency_key);
    if key.len() > 255 {
        return error_response(StatusCode::BAD_REQUEST, "Idempotency-Key is too long");
    }
    match webhook_delivery::get_webhook_replay_by_idempotency_key(
        &state.pool,
        original.id,
        Some(&key),
    )
    .await
    {
        Ok(Some(existing)) => {
            return (StatusCode::ACCEPTED, Json(delivery_map(&existing, true))).into_response()
        }
        Ok(None) => {}
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to inspect replay request",
            )
        }
    }
    let inserted = sqlx::query_as::<_, WebhookDelivery>(
        r#"INSERT INTO webhook_delivery
           (workspace_id,autopilot_id,trigger_id,provider,event,dedupe_key,dedupe_source,
            signature_status,status,selected_headers,content_type,raw_body,
            replayed_from_delivery_id,replay_idempotency_key)
           VALUES ($1,$2,$3,$4,$5,NULL,NULL,'not_required','queued',$6,$7,$8,$9,$10)
           RETURNING *"#,
    )
    .bind(ap.workspace_id)
    .bind(ap.id)
    .bind(trigger.id)
    .bind(&original.provider)
    .bind(event)
    .bind(&original.selected_headers)
    .bind(&original.content_type)
    .bind(raw)
    .bind(original.id)
    .bind(&key)
    .fetch_one(&state.pool)
    .await;
    match inserted {
        Ok(v) => {
            state.notify_webhook_delivery();
            (StatusCode::ACCEPTED, Json(delivery_map(&v, true))).into_response()
        }
        Err(error) if is_unique_violation(&error) => {
            match webhook_delivery::get_webhook_replay_by_idempotency_key(
                &state.pool,
                original.id,
                Some(&key),
            )
            .await
            {
                Ok(Some(existing)) => {
                    state.notify_webhook_delivery();
                    (StatusCode::ACCEPTED, Json(delivery_map(&existing, true))).into_response()
                }
                _ => error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create replay delivery",
                ),
            }
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create replay delivery",
        ),
    }
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23505")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn webhook_trigger() -> AutopilotTrigger {
        let now = Utc::now();
        AutopilotTrigger {
            id: Uuid::new_v4(),
            autopilot_id: Uuid::new_v4(),
            kind: "webhook".into(),
            enabled: true,
            cron_expression: None,
            timezone: None,
            next_run_at: None,
            webhook_token: Some("awt_do_not_expose".into()),
            label: None,
            last_fired_at: None,
            created_at: now,
            updated_at: now,
            provider: "github".into(),
            signing_secret: Some("super-secret-value".into()),
            event_filters: None,
            published_by_type: Some("member".into()),
            published_by_id: Some(Uuid::new_v4()),
        }
    }

    #[test]
    fn generated_webhook_token_is_strong_and_url_safe() {
        let token = new_webhook_token();
        assert!(token.starts_with("awt_"));
        assert_eq!(token.len(), 47);
        assert!(!token.contains(['+', '/', '=']));
    }
    #[test]
    fn pagination_is_bounded() {
        assert_eq!(
            pagination(&Pagination {
                limit: Some(999),
                offset: Some(-1)
            }),
            (100, 0)
        );
    }
    #[test]
    fn rejects_empty_event_names() {
        assert!(validate_event_filters(&[WebhookEventFilter {
            event: "".into(),
            actions: vec![]
        }])
        .is_err());
    }

    #[test]
    fn non_writer_cannot_see_webhook_bearer_token() {
        let value = trigger_map(&webhook_trigger(), "https://cordy.test", false);
        assert_eq!(value["webhook_token"], Value::Null);
        assert_eq!(value["webhook_path"], Value::Null);
        assert_eq!(value["webhook_url"], Value::Null);
    }

    #[test]
    fn signing_secret_is_never_serialized() {
        let value = trigger_map(&webhook_trigger(), "", true);
        let encoded = serde_json::to_string(&value).unwrap();
        assert!(!encoded.contains("super-secret-value"));
        assert_eq!(value["has_signing_secret"], true);
        assert_eq!(value["signing_secret_hint"], "alue");
    }

    #[test]
    fn signing_secret_hint_is_utf8_safe() {
        let mut trigger = webhook_trigger();
        trigger.signing_secret = Some("前缀安全密钥".into());
        let value = trigger_map(&trigger, "", true);
        assert_eq!(value["signing_secret_hint"], "安全密钥");
    }

    #[test]
    fn collaborator_request_only_requires_user_id() {
        let request: CollaboratorRequest = serde_json::from_value(json!({
            "user_id": Uuid::new_v4().to_string()
        }))
        .unwrap();
        assert!(!request.user_id.is_empty());
    }

    #[test]
    fn task_token_manual_trigger_has_no_human_attribution() {
        let member_id = Uuid::new_v4();
        let mut headers = HeaderMap::new();
        assert_eq!(manual_actor_user_id(&headers, member_id), Some(member_id));
        headers.insert("x-actor-source", "task_token".parse().unwrap());
        assert_eq!(manual_actor_user_id(&headers, member_id), None);
    }

    #[test]
    fn api_trigger_kind_is_rejected() {
        assert!(valid_trigger_kind("schedule"));
        assert!(valid_trigger_kind("webhook"));
        assert!(!valid_trigger_kind("api"));
    }

    #[test]
    fn replay_rejects_terminal_signature_rejection_status() {
        let now = Utc::now();
        let delivery = WebhookDelivery {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            autopilot_id: Uuid::new_v4(),
            trigger_id: Uuid::new_v4(),
            provider: "generic".into(),
            event: "webhook.received".into(),
            dedupe_key: None,
            dedupe_source: None,
            signature_status: "not_required".into(),
            status: "rejected".into(),
            attempt_count: 1,
            selected_headers: json!({}),
            content_type: Some("application/json".into()),
            raw_body: Some(b"{}".to_vec()),
            response_status: None,
            response_body: None,
            autopilot_run_id: None,
            replayed_from_delivery_id: None,
            error: None,
            received_at: now,
            last_attempt_at: now,
            created_at: now,
            available_at: now,
            lease_token: None,
            lease_expires_at: None,
            dispatch_attempts: 0,
            reason_code: None,
            replay_idempotency_key: None,
        };
        assert!(replay_rejected(&delivery));
    }
}
