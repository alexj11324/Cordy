//! Core issue routes — S8 authenticated issue-domain slice.
//!
//! Ports the stable list/query, detail, create/update/batch-update, children,
//! and issue-label contracts from `server/internal/handler/issue.go` and `label.go`. The
//! workspace middleware resolves slugs/ids, verifies membership, and stamps a
//! `WorkspaceContext` before these handlers run.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Extension, Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{NaiveDate, SecondsFormat};
use cordy_db::models::{
    AgentTaskQueue, Attachment, Issue, IssueLabel, IssueReaction, IssueSubscriber,
};
use cordy_db::queries::issue_reaction::AddIssueReactionRow;
use cordy_db::queries::{
    agent, agent_invocation_target, attachment, issue as issue_q, issue_label, issue_property,
    issue_reaction, member, squad, subscriber, task_usage, user, workspace,
};
use cordy_middleware::workspace::{WorkspaceContext, WorkspaceGuardState};
use cordy_service::issue_service::{
    IssueCreateError, IssueCreateOpts, IssueCreateParams, IssueTriggerInput, IssueTriggerProbe,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const PRIORITIES: &[&str] = &["urgent", "high", "medium", "low", "none"];

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/assignee-frequency", get(get_assignee_frequency))
        .route("/api/issues", get(list_issues).post(create_issue))
        .route("/api/issues/", get(list_issues).post(create_issue))
        .route("/api/issues/query", post(query_issues))
        .route("/api/issues/child-progress", get(child_issue_progress))
        .route("/api/issues/children", get(list_children_by_parents))
        .route("/api/issues/batch-update", post(batch_update_issues))
        .route("/api/issues/{id}", get(get_issue).put(update_issue))
        .route("/api/issues/{id}/", get(get_issue).put(update_issue))
        .route("/api/issues/{id}/move", post(move_issue))
        .route("/api/issues/{id}/children", get(list_child_issues))
        .route("/api/issues/{id}/usage", get(get_issue_usage))
        .route("/api/issues/{id}/attachments", get(list_attachments))
        .route("/api/issues/{id}/active-task", get(get_active_tasks))
        .route("/api/issues/{id}/task-runs", get(list_task_runs))
        .route(
            "/api/issues/{id}/pull-requests",
            get(crate::issue_pull_request::list)
                .post(crate::issue_pull_request::attach)
                .layer(DefaultBodyLimit::max(4 << 20)),
        )
        .route("/api/issues/{id}/tasks/{task_id}/cancel", post(cancel_task))
        .route("/api/issues/{id}/metadata", get(list_issue_metadata))
        .route(
            "/api/issues/{id}/metadata/{key}",
            axum::routing::put(set_issue_metadata_key).delete(delete_issue_metadata_key),
        )
        .route(
            "/api/issues/{id}/properties/{property_id}",
            axum::routing::put(set_issue_property).delete(unset_issue_property),
        )
        .route(
            "/api/issues/{id}/reactions",
            post(add_issue_reaction).delete(remove_issue_reaction),
        )
        .route("/api/issues/{id}/subscribers", get(list_issue_subscribers))
        .route("/api/issues/{id}/subscribe", post(subscribe_to_issue))
        .route("/api/issues/{id}/unsubscribe", post(unsubscribe_from_issue))
        .route(
            "/api/issues/{id}/unsubscribe/subtree",
            post(unsubscribe_from_issue_subtree),
        )
        .route(
            "/api/issues/{id}/labels",
            get(list_labels_for_issue).post(attach_label),
        )
        .route(
            "/api/issues/{id}/labels/{label_id}",
            axum::routing::delete(detach_label),
        )
}

async fn move_anchor_position(
    state: &HandlerState,
    workspace_id: Uuid,
    id: Option<Uuid>,
) -> Result<Option<f64>, Response> {
    let Some(id) = id else { return Ok(None) };
    sqlx::query_scalar::<_, f64>("SELECT position FROM issue WHERE workspace_id=$1 AND id=$2")
        .bind(workspace_id)
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to resolve move anchor");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to resolve move anchor",
            )
        })?
        .map(Some)
        .ok_or_else(|| {
            error_response(
                StatusCode::BAD_REQUEST,
                "move anchor not found in this workspace",
            )
        })
}

fn move_position(
    current: f64,
    before: Option<f64>,
    after: Option<f64>,
) -> Result<f64, &'static str> {
    let position = match (before, after) {
        (Some(before), Some(after)) if before < after => before + (after - before) / 2.0,
        (Some(_), Some(_)) => return Err("move anchors are stale or out of order"),
        (Some(before), None) => before + 1.0,
        (None, Some(after)) => after - 1.0,
        (None, None) => current,
    };
    if !position.is_finite()
        || before.is_some_and(|value| position <= value)
        || after.is_some_and(|value| position >= value)
    {
        return Err("move anchors are too close; refresh and retry");
    }
    Ok(position)
}

async fn move_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let current = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let mut fields = match update_object(&body) {
        Ok(fields) => fields,
        Err(response) => return response,
    };
    const ALLOWED: &[&str] = &[
        "status",
        "assignee_type",
        "assignee_id",
        "parent_issue_id",
        "project_id",
        "before_id",
        "after_id",
        "expected_revision",
    ];
    if let Some(field) = fields
        .keys()
        .find(|field| !ALLOWED.contains(&field.as_str()))
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!("unsupported move field: {field}"),
        );
    }
    if !fields.contains_key("before_id") {
        return error_response(StatusCode::BAD_REQUEST, "before_id is required");
    }
    if !fields.contains_key("after_id") {
        return error_response(StatusCode::BAD_REQUEST, "after_id is required");
    }
    let decode = |name: &str| -> Result<Option<Uuid>, Response> {
        match fields.get(name) {
            Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => Uuid::parse_str(value)
                .map(Some)
                .map_err(|_| error_response(StatusCode::BAD_REQUEST, &format!("invalid {name}"))),
            _ => Err(error_response(
                StatusCode::BAD_REQUEST,
                &format!("{name} must be a UUID or null"),
            )),
        }
    };
    let before_id = match decode("before_id") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let after_id = match decode("after_id") {
        Ok(value) => value,
        Err(response) => return response,
    };
    if before_id == Some(current.id) || after_id == Some(current.id) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "move anchor cannot be the moved issue",
        );
    }
    if before_id.is_some() && before_id == after_id {
        return error_response(StatusCode::BAD_REQUEST, "move anchors must be distinct");
    }
    let before = match move_anchor_position(&state, current.workspace_id, before_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let after = match move_anchor_position(&state, current.workspace_id, after_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let position = match move_position(current.position, before, after) {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::CONFLICT, message),
    };
    fields.remove("before_id");
    fields.remove("after_id");
    fields.insert("position".into(), json!(position));
    match apply_issue_update(&state, &context, &headers, current, &fields, true).await {
        Ok(issue) => issue_response(&state, issue).await,
        Err(response) => response,
    }
}

async fn list_issue_subscribers(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    match subscriber::list_issue_subscribers(&state.pool, issue.id).await {
        Ok(subscribers) => Json(
            subscribers
                .iter()
                .map(SubscriberResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to list subscribers");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list subscribers",
            )
        }
    }
}

#[derive(Default, Deserialize)]
struct SubscriberRequest {
    user_id: Option<Uuid>,
    user_type: Option<String>,
}

async fn subscriber_target(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    request: SubscriberRequest,
) -> Result<(String, Uuid), Response> {
    let (caller_type, caller_id) = request_actor(headers, context);
    let user_type = request.user_type.unwrap_or_else(|| caller_type.into());
    let user_id = request.user_id.unwrap_or(caller_id);
    if !matches!(user_type.as_str(), "member" | "agent") {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "target user is not a member of this workspace",
        ));
    }
    let table = if user_type == "member" {
        "member"
    } else {
        "agent"
    };
    let key = if user_type == "member" {
        "user_id"
    } else {
        "id"
    };
    let statement = format!("SELECT 1 FROM {table} WHERE {key}=$1 AND workspace_id=$2");
    let exists = sqlx::query(&statement)
        .bind(user_id)
        .bind(context.member.workspace_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to verify subscriber target");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to verify subscriber",
            )
        })?;
    if exists.is_none() {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "target user is not a member of this workspace",
        ));
    }
    Ok((user_type, user_id))
}

async fn subscribe_to_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    request: Option<Json<SubscriberRequest>>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let target = subscriber_target(
        &state,
        &context,
        &headers,
        request.map(|Json(value)| value).unwrap_or_default(),
    )
    .await;
    let (user_type, user_id) = match target {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) = subscriber::subscribe_to_issue_explicitly(
        &state.pool,
        issue.id,
        &user_type,
        user_id,
        "manual",
    )
    .await
    {
        tracing::warn!(%error, "failed to subscribe");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to subscribe");
    }
    let (actor_type, actor_id) = request_actor(&headers, &context);
    state.bus.publish(&cordy_events::Event {
        event_type: "subscriber:added".into(), workspace_id: context.workspace_id.clone(),
        actor_type: actor_type.into(), actor_id: actor_id.to_string(),
        payload: json!({ "issue_id": issue.id, "user_type": user_type, "user_id": user_id, "reason": "manual" }),
        ..Default::default()
    });
    Json(json!({ "subscribed": true })).into_response()
}

async fn unsubscribe_from_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    request: Option<Json<SubscriberRequest>>,
) -> Response {
    unsubscribe(
        &state,
        &context,
        &id,
        &headers,
        request.map(|Json(v)| v).unwrap_or_default(),
        false,
    )
    .await
}

async fn unsubscribe_from_issue_subtree(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    request: Option<Json<SubscriberRequest>>,
) -> Response {
    unsubscribe(
        &state,
        &context,
        &id,
        &headers,
        request.map(|Json(v)| v).unwrap_or_default(),
        true,
    )
    .await
}

async fn unsubscribe(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_id: &str,
    headers: &HeaderMap,
    request: SubscriberRequest,
    subtree: bool,
) -> Response {
    let issue = match resolve_issue(state, context, raw_id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let (user_type, user_id) = match subscriber_target(state, context, headers, request).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let removed = if subtree {
        let mut transaction = match state.pool.begin().await {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "failed to unsubscribe");
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to unsubscribe");
            }
        };
        if let Err(error) =
            subscriber::lock_subscriber_writes(&mut *transaction, issue.workspace_id, user_id).await
        {
            tracing::warn!(%error, "failed to lock subscriber writes");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to unsubscribe");
        }
        if user_type == "member" {
            match subscriber::lock_active_member(&mut *transaction, user_id, issue.workspace_id)
                .await
            {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return error_response(
                        StatusCode::FORBIDDEN,
                        "target user is not a member of this workspace",
                    )
                }
                Err(error) => {
                    tracing::warn!(%error, "failed to recheck subscriber membership");
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to unsubscribe",
                    );
                }
            }
        }
        match subscriber::unsubscribe_from_issue_subtree(
            &mut *transaction,
            issue.id,
            &user_type,
            user_id,
        )
        .await
        {
            Ok(ids) => {
                if let Err(error) = transaction.commit().await {
                    tracing::warn!(%error, "failed to commit unsubscribe");
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to unsubscribe",
                    );
                }
                ids.into_iter().flatten().collect::<Vec<_>>()
            }
            Err(error) => {
                tracing::warn!(%error, "failed to unsubscribe subtree");
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to unsubscribe");
            }
        }
    } else {
        if let Err(error) =
            subscriber::remove_issue_subscriber(&state.pool, issue.id, &user_type, user_id).await
        {
            tracing::warn!(%error, "failed to unsubscribe");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to unsubscribe");
        }
        vec![issue.id]
    };
    let (actor_type, actor_id) = request_actor(headers, context);
    for issue_id in removed {
        state.bus.publish(&cordy_events::Event {
            event_type: "subscriber:removed".into(),
            workspace_id: context.workspace_id.clone(),
            actor_type: actor_type.into(),
            actor_id: actor_id.to_string(),
            payload: json!({ "issue_id": issue_id, "user_type": user_type, "user_id": user_id }),
            ..Default::default()
        });
    }
    Json(json!({ "subscribed": false })).into_response()
}

fn request_actor(headers: &HeaderMap, context: &WorkspaceContext) -> (&'static str, Uuid) {
    if headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        == Some("task_token")
    {
        if let Some(id) = headers
            .get("x-agent-id")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| Uuid::parse_str(value).ok())
        {
            return ("agent", id);
        }
    }
    ("member", context.member.user_id)
}

#[derive(Deserialize)]
struct ReactionRequest {
    emoji: String,
}

async fn add_issue_reaction(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ReactionRequest>,
) -> Response {
    if request.emoji.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "emoji is required");
    }
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let (actor_type, actor_id) = request_actor(&headers, &context);
    match issue_reaction::add_issue_reaction(
        &state.pool,
        issue.id,
        issue.workspace_id,
        actor_type,
        actor_id,
        &request.emoji,
    )
    .await
    {
        Ok(Some(reaction)) => {
            let Some(response) = IssueReactionResponse::from_added(&reaction) else {
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to add reaction");
            };
            if reaction.issue_revision > 0 {
                state.bus.publish(&cordy_events::Event {
                    event_type: "issue:reaction_added".into(),
                    workspace_id: context.workspace_id.clone(),
                    actor_type: actor_type.into(),
                    actor_id: actor_id.to_string(),
                    payload: json!({
                        "reaction": response,
                        "issue_id": issue.id,
                        "issue_title": issue.title,
                        "issue_status": issue.status,
                        "creator_type": issue.creator_type,
                        "creator_id": issue.creator_id,
                        "issue_revision": reaction.issue_revision,
                    }),
                    ..Default::default()
                });
            }
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Ok(None) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to add reaction"),
        Err(error) => {
            tracing::warn!(%error, "failed to add issue reaction");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to add reaction")
        }
    }
}

async fn remove_issue_reaction(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ReactionRequest>,
) -> Response {
    if request.emoji.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "emoji is required");
    }
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let (actor_type, actor_id) = request_actor(&headers, &context);
    match issue_reaction::remove_issue_reaction(
        &state.pool,
        issue.id,
        actor_type,
        actor_id,
        &request.emoji,
    )
    .await
    {
        Ok(Some(removed)) => {
            if removed.changed {
                state.bus.publish(&cordy_events::Event {
                    event_type: "issue:reaction_removed".into(),
                    workspace_id: context.workspace_id.clone(),
                    actor_type: actor_type.into(),
                    actor_id: actor_id.to_string(),
                    payload: json!({
                        "issue_id": issue.id,
                        "emoji": request.emoji,
                        "actor_type": actor_type,
                        "actor_id": actor_id,
                        "issue_revision": removed.issue_revision,
                    }),
                    ..Default::default()
                });
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(None) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to remove reaction",
        ),
        Err(error) => {
            tracing::warn!(%error, "failed to remove issue reaction");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to remove reaction",
            )
        }
    }
}

fn valid_metadata_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(first) if first == '_' || first.is_ascii_alphabetic())
        && key.len() <= 64
        && chars.all(|character| {
            character == '_'
                || character == '.'
                || character == '-'
                || character.is_ascii_alphanumeric()
        })
}

async fn list_issue_metadata(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    match resolve_issue(&state, &context, &id).await {
        Ok(issue) => Json(json!({ "metadata": issue.metadata })).into_response(),
        Err(response) => response,
    }
}

fn decode_property_value(body: &[u8]) -> Result<Option<Value>, ()> {
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let request = Value::deserialize(&mut deserializer).map_err(|_| ())?;
    let fields = request.as_object().ok_or(())?;
    Ok(fields.get("value").cloned())
}

async fn set_issue_property(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_issue, raw_property)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let property_id = match Uuid::parse_str(raw_property.trim()) {
        Ok(property_id) => property_id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid property id"),
    };
    let value = match decode_property_value(&body) {
        Ok(Some(value)) => value,
        Ok(None) => return error_response(StatusCode::BAD_REQUEST, "value is required"),
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let issue = match resolve_issue(&state, &context, &raw_issue).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, %property_id, "failed to begin property write");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to set property");
        }
    };
    if let Err(error) = sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("prop:{property_id}"))
        .execute(&mut *transaction)
        .await
    {
        tracing::warn!(%error, %property_id, "failed to lock property definition");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to set property");
    }
    let definition = match issue_property::get_issue_property(
        &mut *transaction,
        property_id,
        issue.workspace_id,
    )
    .await
    {
        Ok(Some(definition)) => definition,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "property not found"),
        Err(error) => {
            tracing::warn!(%error, %property_id, "failed to resolve property before set");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to set property");
        }
    };
    if definition.archived_at.is_some() {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!(
                "property {:?} is archived and cannot receive new values",
                definition.name
            ),
        );
    }
    let (stored, actor_refs) = match crate::issue_property_value::validate(&definition, &value) {
        Ok(validated) => validated,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    for actor in actor_refs {
        match member::get_member_by_user_and_workspace(
            &mut *transaction,
            actor.user_id,
            issue.workspace_id,
        )
        .await
        {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!(
                        "{:?} does not refer to a member of this workspace",
                        actor.value
                    ),
                )
            }
        }
    }
    let updated = match issue_property::set_issue_property_value(
        &mut *transaction,
        &property_id.to_string(),
        &stored,
        issue.id,
        issue.workspace_id,
    )
    .await
    {
        Ok(Some(updated)) => updated,
        Ok(None) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to set property")
        }
        Err(error)
            if error
                .downcast_ref::<sqlx::Error>()
                .and_then(sqlx::Error::as_database_error)
                .and_then(|error| error.code())
                .is_some_and(|code| code == "23514") =>
        {
            return error_response(
                StatusCode::BAD_REQUEST,
                "issue properties exceed the 16KB size limit",
            )
        }
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, %property_id, "failed to set property");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to set property");
        }
    };
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, issue_id = %issue.id, %property_id, "failed to commit property write");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to set property");
    }
    let properties = object_or_empty(updated.properties.clone());
    let (actor_type, actor_id, task_id) = mutation_actor(&state, &context, &headers).await;
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_ISSUE_PROPERTIES_CHANGED.into(),
        workspace_id: updated.workspace_id.to_string(),
        actor_type,
        actor_id: actor_id.to_string(),
        payload: json!({
            "issue_id": updated.id,
            "properties": properties.clone(),
            "issue_revision": updated.revision,
        }),
        task_id: task_id.map(|id| id.to_string()).unwrap_or_default(),
        chat_session_id: String::new(),
    });
    Json(json!({
        "properties": properties,
        "issue_revision": updated.revision,
    }))
    .into_response()
}

async fn unset_issue_property(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_issue, raw_property)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let property_id = match Uuid::parse_str(raw_property.trim()) {
        Ok(property_id) => property_id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid property id"),
    };
    let issue = match resolve_issue(&state, &context, &raw_issue).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    match issue_property::get_issue_property(&state.pool, property_id, issue.workspace_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "property not found"),
        Err(error) => {
            tracing::warn!(%error, %property_id, "failed to resolve property before unset");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to unset property",
            );
        }
    }
    let updated = match issue_property::delete_issue_property_value(
        &state.pool,
        &property_id.to_string(),
        issue.id,
        issue.workspace_id,
    )
    .await
    {
        Ok(Some(updated)) => updated,
        Ok(None) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to unset property",
            )
        }
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, %property_id, "failed to unset property");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to unset property",
            );
        }
    };
    let properties = object_or_empty(updated.properties.clone());
    let (actor_type, actor_id, task_id) = mutation_actor(&state, &context, &headers).await;
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_ISSUE_PROPERTIES_CHANGED.into(),
        workspace_id: updated.workspace_id.to_string(),
        actor_type,
        actor_id: actor_id.to_string(),
        payload: json!({
            "issue_id": updated.id,
            "properties": properties.clone(),
            "issue_revision": updated.revision,
        }),
        task_id: task_id.map(|id| id.to_string()).unwrap_or_default(),
        chat_session_id: String::new(),
    });
    Json(json!({
        "properties": properties,
        "issue_revision": updated.revision,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct SetMetadataRequest {
    value: Value,
}

async fn set_issue_metadata_key(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((id, key)): Path<(String, String)>,
    Json(request): Json<SetMetadataRequest>,
) -> Response {
    if !valid_metadata_key(&key) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "key must match ^[a-zA-Z_][a-zA-Z0-9_.-]{0,63}$",
        );
    }
    if !matches!(
        request.value,
        Value::String(_) | Value::Number(_) | Value::Bool(_)
    ) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "value must be a primitive: string, number, or bool",
        );
    }
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let count = issue.metadata.as_object().map_or(0, serde_json::Map::len);
    if issue.metadata.get(&key).is_none() && count >= 50 {
        return error_response(StatusCode::BAD_REQUEST, "metadata cannot exceed 50 keys");
    }
    match issue_q::set_issue_metadata_key(
        &state.pool,
        &key,
        &request.value,
        issue.id,
        issue.workspace_id,
    )
    .await
    {
        Ok(Some(updated)) => metadata_response(&state, &context, updated),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "issue not found"),
        Err(error)
            if error
                .downcast_ref::<sqlx::Error>()
                .and_then(|e| e.as_database_error())
                .and_then(|e| e.code())
                .is_some_and(|code| code == "23514") =>
        {
            error_response(
                StatusCode::BAD_REQUEST,
                "metadata exceeds the 8KB size limit",
            )
        }
        Err(error) => {
            tracing::warn!(%error, "failed to set metadata key");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to set metadata key",
            )
        }
    }
}

async fn delete_issue_metadata_key(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((id, key)): Path<(String, String)>,
) -> Response {
    if !valid_metadata_key(&key) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "key must match ^[a-zA-Z_][a-zA-Z0-9_.-]{0,63}$",
        );
    }
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    match issue_q::delete_issue_metadata_key(&state.pool, &key, issue.id, issue.workspace_id).await
    {
        Ok(Some(updated)) => metadata_response(&state, &context, updated),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "issue not found"),
        Err(error) => {
            tracing::warn!(%error, "failed to delete metadata key");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete metadata key",
            )
        }
    }
}

fn metadata_response(state: &HandlerState, context: &WorkspaceContext, issue: Issue) -> Response {
    state.bus.publish(&cordy_events::Event {
        event_type: "issue:metadata_changed".into(),
        workspace_id: context.workspace_id.clone(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload: json!({
            "issue_id": issue.id,
            "metadata": issue.metadata,
            "issue_revision": issue.revision,
        }),
        ..Default::default()
    });
    Json(json!({ "metadata": issue.metadata, "issue_revision": issue.revision })).into_response()
}

/// Workspace guard for the issue group. Kept here because this slice needs a
/// JSON `Response` on every failure path; it uses the shared resolver and the
/// same `WorkspaceContext` type as `cordy-middleware`.
pub async fn require_issue_workspace(
    State(state): State<WorkspaceGuardState>,
    mut request: Request,
    next: Next,
) -> Response {
    let actor_source = header_owned(&request, "x-actor-source");
    let workspace_header = header_owned(&request, "x-workspace-id");
    let slug = query_owned(&request, "workspace_slug")
        .or_else(|| header_owned(&request, "x-workspace-slug"));
    let workspace_query = query_owned(&request, "workspace_id");
    let user_id =
        header_owned(&request, "x-user-id").and_then(|value| Uuid::parse_str(&value).ok());

    let raw_workspace_id = if actor_source.as_deref() == Some("task_token") {
        workspace_header
    } else if let Some(slug) = slug {
        workspace::get_workspace_by_slug(&state.pool, &slug)
            .await
            .ok()
            .flatten()
            .map(|workspace| workspace.id.to_string())
    } else {
        workspace_header.or(workspace_query)
    };
    let Some(raw_workspace_id) = raw_workspace_id else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "workspace_id or workspace_slug is required",
        );
    };
    let Ok(workspace_id) = Uuid::parse_str(&raw_workspace_id) else {
        return error_response(StatusCode::NOT_FOUND, "workspace not found");
    };
    let Some(user_id) = user_id else {
        return error_response(StatusCode::UNAUTHORIZED, "user not authenticated");
    };
    let member =
        match member::get_member_by_user_and_workspace(&state.pool, user_id, workspace_id).await {
            Ok(Some(member)) => member,
            Ok(None) => return error_response(StatusCode::NOT_FOUND, "workspace not found"),
            Err(error) => {
                tracing::warn!(%error, %workspace_id, "workspace membership lookup failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to verify workspace",
                );
            }
        };
    request.extensions_mut().insert(WorkspaceContext {
        workspace_id: workspace_id.to_string(),
        member,
    });
    next.run(request).await
}

fn header_owned(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn query_owned(request: &Request, name: &str) -> Option<String> {
    request.uri().query()?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name && !value.is_empty()).then(|| value.to_string())
    })
}

#[derive(Debug, Default, Deserialize)]
struct ListParams {
    limit: Option<String>,
    offset: Option<String>,
    status: Option<String>,
    statuses: Option<String>,
    status_category: Option<String>,
    status_categories: Option<String>,
    priority: Option<String>,
    priorities: Option<String>,
    assignee_id: Option<String>,
    assignee_ids: Option<String>,
    assignee_types: Option<String>,
    creator_id: Option<String>,
    assignee_filters: Option<String>,
    creator_filters: Option<String>,
    include_no_assignee: Option<String>,
    include_no_project: Option<String>,
    label_ids: Option<String>,
    involves_user_id: Option<String>,
    metadata: Option<String>,
    properties: Option<String>,
    date_field: Option<String>,
    date_start: Option<String>,
    date_end: Option<String>,
    project_id: Option<String>,
    project_ids: Option<String>,
    ids: Option<String>,
    q: Option<String>,
    top_level_only: Option<String>,
    scheduled: Option<String>,
    open_only: Option<String>,
    sort: Option<String>,
    direction: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChildrenByParentsParams {
    parent_ids: Option<String>,
}

const LIST_CHILDREN_BY_PARENTS_LIMIT: usize = 200;

fn parse_parent_ids(raw: &str) -> Result<Vec<Uuid>, &'static str> {
    let parts = raw.split(',').collect::<Vec<_>>();
    if parts.len() > LIST_CHILDREN_BY_PARENTS_LIMIT {
        return Err("too many parent_ids");
    }
    parts
        .into_iter()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| Uuid::parse_str(value).map_err(|_| "invalid parent_ids"))
        .collect()
}

async fn list_children_by_parents(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<ChildrenByParentsParams>,
) -> Response {
    let raw = params.parent_ids.as_deref().unwrap_or_default();
    if raw.is_empty() {
        return Json(json!({ "issues": Vec::<IssueResponse>::new() })).into_response();
    }

    let parent_ids = match parse_parent_ids(raw) {
        Ok(ids) => ids,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, message),
    };
    if parent_ids.is_empty() {
        return Json(json!({ "issues": Vec::<IssueResponse>::new() })).into_response();
    }

    match issue_q::list_children_by_parents(&state.pool, context.member.workspace_id, parent_ids)
        .await
    {
        Ok(issues) => Json(json!({
            "issues": enrich_issue_list(&state, &context, issues).await,
        }))
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, workspace_id = %context.member.workspace_id, "failed to list child issues");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list child issues",
            )
        }
    }
}

#[derive(Debug, Serialize)]
struct ChildProgressResponse {
    parent_issue_id: String,
    total: i64,
    done: i64,
}

async fn child_issue_progress(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    match issue_q::child_issue_progress(&state.pool, context.member.workspace_id).await {
        Ok(rows) => {
            let progress = rows
                .into_iter()
                .filter_map(|row| {
                    row.parent_issue_id
                        .map(|parent_issue_id| ChildProgressResponse {
                            parent_issue_id: parent_issue_id.to_string(),
                            total: row.total,
                            done: row.done,
                        })
                })
                .collect::<Vec<_>>();
            Json(json!({ "progress": progress })).into_response()
        }
        Err(error) => {
            tracing::warn!(%error, workspace_id = %context.member.workspace_id, "failed to get child issue progress");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get child issue progress",
            )
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct AssigneeFrequencyResponse {
    assignee_type: String,
    assignee_id: String,
    frequency: i64,
}

#[derive(Debug, FromRow)]
struct AssigneeActivityFrequencyRow {
    assignee_type: String,
    assignee_id: String,
    frequency: i64,
}

async fn get_assignee_frequency(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let workspace_id = context.member.workspace_id;
    let user_id = context.member.user_id;
    let (activity_counts, issue_counts) = tokio::join!(
        sqlx::query_as::<_, AssigneeActivityFrequencyRow>(
            r#"SELECT
                  details->>'to_type' AS assignee_type,
                  details->>'to_id' AS assignee_id,
                  COUNT(*)::bigint AS frequency
               FROM activity_log
              WHERE workspace_id = $1
                AND actor_id = $2
                AND actor_type = 'member'
                AND action = 'assignee_changed'
                AND details->>'to_type' IS NOT NULL
                AND details->>'to_id' IS NOT NULL
              GROUP BY details->>'to_type', details->>'to_id'"#,
        )
        .bind(workspace_id)
        .bind(user_id)
        .fetch_all(&state.pool),
        issue_q::count_created_issue_assignees(&state.pool, workspace_id, user_id),
    );
    let (activity_counts, issue_counts) = match (activity_counts, issue_counts) {
        (Ok(activity_counts), Ok(issue_counts)) => (activity_counts, issue_counts),
        (activity_result, issue_result) => {
            tracing::warn!(
                workspace_id = %workspace_id,
                activity_error = ?activity_result.err(),
                issue_error = ?issue_result.err(),
                "failed to get assignee frequency"
            );
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get assignee frequency",
            );
        }
    };

    let mut frequencies = HashMap::<(String, String), i64>::new();
    for row in activity_counts {
        *frequencies
            .entry((row.assignee_type, row.assignee_id))
            .or_default() += row.frequency;
    }
    for row in issue_counts {
        if let (Some(assignee_type), Some(assignee_id)) = (row.assignee_type, row.assignee_id) {
            *frequencies
                .entry((assignee_type, assignee_id.to_string()))
                .or_default() += row.frequency;
        }
    }

    let mut response = frequencies
        .into_iter()
        .map(
            |((assignee_type, assignee_id), frequency)| AssigneeFrequencyResponse {
                assignee_type,
                assignee_id,
                frequency,
            },
        )
        .collect::<Vec<_>>();
    response.sort_by_key(|entry| std::cmp::Reverse(entry.frequency));
    Json(response).into_response()
}

#[derive(Debug, FromRow)]
struct ListRow {
    acceptance_criteria: Value,
    assignee_id: Option<Uuid>,
    assignee_type: Option<String>,
    context_refs: Value,
    created_at: chrono::DateTime<chrono::Utc>,
    creator_id: Uuid,
    creator_type: String,
    description: Option<String>,
    due_date: Option<NaiveDate>,
    first_executed_at: Option<chrono::DateTime<chrono::Utc>>,
    id: Uuid,
    last_activity_at: Option<chrono::DateTime<chrono::Utc>>,
    metadata: Value,
    number: i32,
    origin_id: Option<Uuid>,
    origin_type: Option<String>,
    parent_issue_id: Option<Uuid>,
    position: f64,
    priority: String,
    project_id: Option<Uuid>,
    properties: Value,
    revision: i64,
    stage: Option<i32>,
    start_date: Option<NaiveDate>,
    status: String,
    title: String,
    updated_at: chrono::DateTime<chrono::Utc>,
    workspace_id: Uuid,
}

impl ListRow {
    fn into_issue(self) -> Issue {
        Issue {
            acceptance_criteria: self.acceptance_criteria,
            assignee_id: self.assignee_id,
            assignee_type: self.assignee_type,
            context_refs: self.context_refs,
            created_at: self.created_at,
            creator_id: self.creator_id,
            creator_type: self.creator_type,
            description: self.description,
            due_date: self.due_date,
            first_executed_at: self.first_executed_at,
            id: self.id,
            last_activity_at: self.last_activity_at,
            metadata: self.metadata,
            number: self.number,
            origin_id: self.origin_id,
            origin_type: self.origin_type,
            parent_issue_id: self.parent_issue_id,
            position: self.position,
            priority: self.priority,
            project_id: self.project_id,
            properties: self.properties,
            revision: self.revision,
            stage: self.stage,
            start_date: self.start_date,
            status: self.status,
            title: self.title,
            updated_at: self.updated_at,
            workspace_id: self.workspace_id,
        }
    }
}

#[derive(Debug, Clone)]
struct ActorFilter {
    actor_type: String,
    actor_id: Uuid,
}

#[derive(Debug, Clone)]
struct DateFilter {
    column: &'static str,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
enum PropertyAlternative {
    Missing(String),
    Contains(Value),
}

#[derive(Debug, Clone)]
struct IssueFilters {
    workspace_id: Uuid,
    statuses: Vec<String>,
    category_statuses: Option<Vec<String>>,
    closed_statuses: Vec<String>,
    priorities: Vec<String>,
    assignee_id: Option<Uuid>,
    assignee_ids: Vec<Uuid>,
    assignee_types: Vec<String>,
    creator_id: Option<Uuid>,
    project_id: Option<Uuid>,
    project_ids: Vec<Uuid>,
    ids: Option<Vec<Uuid>>,
    assignee_filters: Vec<ActorFilter>,
    creator_filters: Vec<ActorFilter>,
    include_no_assignee: bool,
    include_no_project: bool,
    label_ids: Vec<Uuid>,
    involves_user_id: Option<Uuid>,
    metadata: Option<Value>,
    properties: Vec<Vec<PropertyAlternative>>,
    date_filter: Option<DateFilter>,
    search_terms: Vec<String>,
    search_number: Option<i32>,
    top_level_only: bool,
    scheduled: bool,
}

fn push_issue_filters(query: &mut QueryBuilder<'_, Postgres>, filters: &IssueFilters) {
    query
        .push("i.workspace_id = ")
        .push_bind(filters.workspace_id);
    if !filters.statuses.is_empty() {
        query
            .push(" AND i.status = ANY(")
            .push_bind(filters.statuses.clone())
            .push(")");
    }
    if let Some(category_statuses) = &filters.category_statuses {
        query
            .push(" AND i.status = ANY(")
            .push_bind(category_statuses.clone())
            .push(")");
    }
    if !filters.closed_statuses.is_empty() {
        query
            .push(" AND NOT (i.status = ANY(")
            .push_bind(filters.closed_statuses.clone())
            .push("))");
    }
    if !filters.priorities.is_empty() {
        query
            .push(" AND i.priority = ANY(")
            .push_bind(filters.priorities.clone())
            .push(")");
    }
    if let Some(id) = filters.assignee_id {
        query.push(" AND i.assignee_id = ").push_bind(id);
    }
    if !filters.assignee_ids.is_empty() {
        query
            .push(" AND i.assignee_id = ANY(")
            .push_bind(filters.assignee_ids.clone())
            .push(")");
    }
    if !filters.assignee_types.is_empty() {
        query
            .push(" AND i.assignee_type = ANY(")
            .push_bind(filters.assignee_types.clone())
            .push(")");
    }
    if let Some(id) = filters.creator_id {
        query.push(" AND i.creator_id = ").push_bind(id);
    }
    if let Some(id) = filters.project_id {
        query.push(" AND i.project_id = ").push_bind(id);
    }
    if !filters.project_ids.is_empty() || filters.include_no_project {
        query.push(" AND (");
        if !filters.project_ids.is_empty() {
            query
                .push("i.project_id = ANY(")
                .push_bind(filters.project_ids.clone())
                .push(")");
            if filters.include_no_project {
                query.push(" OR ");
            }
        }
        if filters.include_no_project {
            query.push("i.project_id IS NULL");
        }
        query.push(")");
    }
    if let Some(ids) = &filters.ids {
        query
            .push(" AND i.id = ANY(")
            .push_bind(ids.clone())
            .push(")");
    }
    if !filters.assignee_filters.is_empty() || filters.include_no_assignee {
        query.push(" AND (");
        let mut separated = query.separated(" OR ");
        for actor in &filters.assignee_filters {
            separated
                .push("(i.assignee_type = ")
                .push_bind(actor.actor_type.clone())
                .push(" AND i.assignee_id = ")
                .push_bind(actor.actor_id)
                .push(")");
        }
        if filters.include_no_assignee {
            separated.push("(i.assignee_type IS NULL AND i.assignee_id IS NULL)");
        }
        separated.push_unseparated(")");
    }
    if !filters.creator_filters.is_empty() {
        query.push(" AND (");
        let mut separated = query.separated(" OR ");
        for actor in &filters.creator_filters {
            separated
                .push("(i.creator_type = ")
                .push_bind(actor.actor_type.clone())
                .push(" AND i.creator_id = ")
                .push_bind(actor.actor_id)
                .push(")");
        }
        separated.push_unseparated(")");
    }
    if !filters.label_ids.is_empty() {
        query.push(" AND EXISTS (SELECT 1 FROM issue_to_label itl WHERE itl.issue_id = i.id AND itl.label_id = ANY(")
            .push_bind(filters.label_ids.clone()).push("))");
    }
    if let Some(user_id) = filters.involves_user_id {
        query.push(" AND ((i.assignee_type = 'agent' AND i.assignee_id IN (SELECT a.id FROM agent a WHERE a.workspace_id = ")
            .push_bind(filters.workspace_id).push(" AND a.owner_id = ").push_bind(user_id)
            .push(")) OR (i.assignee_type = 'squad' AND i.assignee_id IN (SELECT sm.squad_id FROM squad_member sm JOIN squad s ON s.id = sm.squad_id WHERE s.workspace_id = ")
            .push_bind(filters.workspace_id).push(" AND sm.member_type = 'member' AND sm.member_id = ").push_bind(user_id)
            .push(" UNION SELECT s.id FROM squad s JOIN agent a ON a.id = s.leader_id WHERE s.workspace_id = ")
            .push_bind(filters.workspace_id).push(" AND a.workspace_id = ").push_bind(filters.workspace_id).push(" AND a.owner_id = ").push_bind(user_id)
            .push(" UNION SELECT sm.squad_id FROM squad_member sm JOIN squad s ON s.id = sm.squad_id JOIN agent a ON a.id = sm.member_id WHERE s.workspace_id = ")
            .push_bind(filters.workspace_id).push(" AND sm.member_type = 'agent' AND a.workspace_id = ").push_bind(filters.workspace_id).push(" AND a.owner_id = ").push_bind(user_id)
            .push(")))");
    }
    if let Some(metadata) = &filters.metadata {
        query
            .push(" AND i.metadata @> ")
            .push_bind(metadata.clone());
    }
    for alternatives in &filters.properties {
        query.push(" AND (");
        let mut separated = query.separated(" OR ");
        for alternative in alternatives {
            match alternative {
                PropertyAlternative::Missing(definition_id) => {
                    separated
                        .push("NOT (i.properties ? ")
                        .push_bind(definition_id.clone())
                        .push(")");
                }
                PropertyAlternative::Contains(value) => {
                    separated.push("i.properties @> ").push_bind(value.clone());
                }
            }
        }
        separated.push_unseparated(")");
    }
    if let Some(date) = &filters.date_filter {
        query
            .push(" AND i.")
            .push(date.column)
            .push(" >= ")
            .push_bind(date.start)
            .push(" AND i.")
            .push(date.column)
            .push(" < ")
            .push_bind(date.end);
    }
    if !filters.search_terms.is_empty() || filters.search_number.is_some() {
        query.push(" AND (");
        if !filters.search_terms.is_empty() {
            query.push("(");
            let mut separated = query.separated(" AND ");
            for pattern in &filters.search_terms {
                separated
                    .push("LOWER(i.title) LIKE ")
                    .push_bind(pattern.clone())
                    .push(" ESCAPE '\\\\'");
            }
            separated.push_unseparated(")");
            if filters.search_number.is_some() {
                query.push(" OR ");
            }
        }
        if let Some(number) = filters.search_number {
            query.push("i.number = ").push_bind(number);
        }
        query.push(")");
    }
    if filters.top_level_only {
        query.push(" AND i.parent_issue_id IS NULL");
    }
    if filters.scheduled {
        query.push(" AND (i.start_date IS NOT NULL OR i.due_date IS NOT NULL)");
    }
}

async fn list_issues(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<ListParams>,
) -> Response {
    list_issues_with_params(&state, &context, params).await
}

/// POST twin of GET /api/issues. Values intentionally stay strings so the two
/// transports share parsing and validation exactly as in Go.
async fn query_issues(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(values): Json<HashMap<String, String>>,
) -> Response {
    let value = match serde_json::to_value(values) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let params = match serde_json::from_value(value) {
        Ok(params) => params,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    list_issues_with_params(&state, &context, params).await
}

async fn list_issues_with_params(
    state: &HandlerState,
    context: &WorkspaceContext,
    params: ListParams,
) -> Response {
    let workspace_id = context.member.workspace_id;
    let open_only = params.open_only.as_deref() == Some("true");
    let limit = params
        .limit
        .as_deref()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(100)
        .min(100);
    let offset = params
        .offset
        .as_deref()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v >= 0)
        .unwrap_or(0);

    let assignee_id = match optional_uuid(params.assignee_id.as_deref(), "assignee_id") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let creator_id = match optional_uuid(params.creator_id.as_deref(), "creator_id") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let project_id = match optional_uuid(params.project_id.as_deref(), "project_id") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let assignee_ids = match uuid_list(params.assignee_ids.as_deref(), "assignee_ids") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let project_ids = match uuid_list(params.project_ids.as_deref(), "project_ids") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let ids = match uuid_list(params.ids.as_deref(), "ids") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let assignee_types = comma_list(params.assignee_types.as_deref());
    if assignee_types
        .iter()
        .any(|kind| !matches!(kind.as_str(), "member" | "agent" | "squad"))
    {
        return error_response(StatusCode::BAD_REQUEST, "invalid assignee_types");
    }

    let statuses = comma_list(params.statuses.as_deref().or(params.status.as_deref()));
    let categories = comma_list(
        params
            .status_categories
            .as_deref()
            .or(params.status_category.as_deref()),
    );
    let category_statuses = match expand_status_categories(state, workspace_id, &categories).await {
        Ok(values) => (!categories.is_empty()).then_some(values),
        Err(response) => return response,
    };
    let closed_statuses = if open_only {
        match expand_status_categories(
            state,
            workspace_id,
            &["done".to_string(), "cancelled".to_string()],
        )
        .await
        {
            Ok(values) => values,
            Err(response) => return response,
        }
    } else {
        Vec::new()
    };
    let priorities = comma_list(params.priorities.as_deref().or(params.priority.as_deref()));
    let assignee_filters =
        match actor_filters(params.assignee_filters.as_deref(), "assignee_filters") {
            Ok(value) => value,
            Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
        };
    let creator_filters = match actor_filters(params.creator_filters.as_deref(), "creator_filters")
    {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let label_ids = match uuid_list(params.label_ids.as_deref(), "label_ids") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let involves_user_id =
        match optional_uuid(params.involves_user_id.as_deref(), "involves_user_id") {
            Ok(value) => value,
            Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
        };
    let metadata = match json_object_filter(params.metadata.as_deref(), "metadata") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let properties = match properties_filter(params.properties.as_deref()) {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let date_filter = match parse_date_filter(
        params.date_field.as_deref(),
        params.date_start.as_deref(),
        params.date_end.as_deref(),
    ) {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let (search_terms, search_number) = search_filter(params.q.as_deref());
    let filters = IssueFilters {
        workspace_id,
        statuses,
        category_statuses,
        closed_statuses,
        priorities,
        assignee_id,
        assignee_ids,
        assignee_types,
        creator_id,
        project_id,
        project_ids,
        ids: params.ids.is_some().then_some(ids),
        assignee_filters,
        creator_filters,
        include_no_assignee: params.include_no_assignee.as_deref() == Some("true"),
        include_no_project: params.include_no_project.as_deref() == Some("true"),
        label_ids,
        involves_user_id,
        metadata,
        properties,
        date_filter,
        search_terms,
        search_number,
        top_level_only: params.top_level_only.as_deref() == Some("true"),
        scheduled: params.scheduled.as_deref() == Some("true"),
    };

    let mut count_query = QueryBuilder::<Postgres>::new("SELECT COUNT(*) FROM issue i WHERE ");
    push_issue_filters(&mut count_query, &filters);
    let total = match count_query
        .build_query_scalar::<i64>()
        .fetch_one(&state.pool)
        .await
    {
        Ok(total) => total,
        Err(error) => {
            tracing::warn!(%error, "failed to count issues");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list issues");
        }
    };

    let mut query = QueryBuilder::<Postgres>::new("SELECT i.* FROM issue i WHERE ");
    push_issue_filters(&mut query, &filters);

    let sort = params.sort.as_deref().unwrap_or("position");
    let direction = params
        .direction
        .as_deref()
        .unwrap_or(if sort == "last_activity" {
            "desc"
        } else {
            "asc"
        });
    if !matches!(direction.to_ascii_lowercase().as_str(), "asc" | "desc") {
        return error_response(StatusCode::BAD_REQUEST, "invalid direction value");
    }
    let direction = direction.to_ascii_uppercase();
    match sort {
        "position" => query.push(" ORDER BY i.position ASC, i.created_at DESC, i.id DESC"),
        "title" | "created_at" | "updated_at" | "start_date" | "due_date" => query
            .push(" ORDER BY i.")
            .push(sort)
            .push(" ")
            .push(direction)
            .push(" NULLS LAST, i.created_at DESC, i.id DESC"),
        "last_activity" => query
            .push(" ORDER BY i.last_activity_at ")
            .push(direction)
            .push(" NULLS LAST, i.id DESC"),
        "priority" => query
            .push(" ORDER BY CASE i.priority WHEN 'urgent' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 WHEN 'low' THEN 3 ELSE 4 END ")
            .push(direction)
            .push(", i.created_at DESC, i.id DESC"),
        "status" => query
            .push(" ORDER BY CASE i.status WHEN 'backlog' THEN 0 WHEN 'todo' THEN 1 WHEN 'in_progress' THEN 2 WHEN 'in_review' THEN 3 WHEN 'done' THEN 4 WHEN 'blocked' THEN 5 WHEN 'cancelled' THEN 6 ELSE 7 END ")
            .push(direction)
            .push(", i.created_at DESC, i.id DESC"),
        _ => return error_response(StatusCode::BAD_REQUEST, "invalid sort value"),
    };
    if !open_only {
        query
            .push(" LIMIT ")
            .push_bind(limit)
            .push(" OFFSET ")
            .push_bind(offset);
    }

    let rows = match query
        .build_query_as::<ListRow>()
        .fetch_all(&state.pool)
        .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, "failed to list issues");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list issues");
        }
    };
    let issues = rows
        .into_iter()
        .map(ListRow::into_issue)
        .collect::<Vec<_>>();
    let responses = enrich_issue_list(state, context, issues).await;
    Json(json!({ "issues": responses, "total": total })).into_response()
}

async fn get_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let issue_id = issue.id;
    let workspace_id = issue.workspace_id;
    let mut responses = enrich_issue_list(&state, &context, vec![issue]).await;
    let mut response = responses.remove(0);
    response.reactions = issue_reaction::list_issue_reactions(&state.pool, issue_id)
        .await
        .unwrap_or_default()
        .iter()
        .map(IssueReactionResponse::from)
        .collect();
    response.attachments =
        attachment::list_attachments_by_issue(&state.pool, issue_id, workspace_id)
            .await
            .unwrap_or_default()
            .iter()
            .map(AttachmentResponse::from)
            .collect();
    Json(response).into_response()
}

#[derive(Debug, Serialize)]
struct IssueUsageResponse {
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_cache_read_tokens: i64,
    total_cache_write_tokens: i64,
    cost_usd_ticks: i64,
    uncosted_input_tokens: i64,
    uncosted_output_tokens: i64,
    uncosted_cache_read_tokens: i64,
    uncosted_cache_write_tokens: i64,
    task_count: i32,
}

impl From<task_usage::GetIssueUsageSummaryRow> for IssueUsageResponse {
    fn from(row: task_usage::GetIssueUsageSummaryRow) -> Self {
        Self {
            total_input_tokens: row.total_input_tokens,
            total_output_tokens: row.total_output_tokens,
            total_cache_read_tokens: row.total_cache_read_tokens,
            total_cache_write_tokens: row.total_cache_write_tokens,
            cost_usd_ticks: row.total_cost_usd_ticks,
            uncosted_input_tokens: row.uncosted_input_tokens,
            uncosted_output_tokens: row.uncosted_output_tokens,
            uncosted_cache_read_tokens: row.uncosted_cache_read_tokens,
            uncosted_cache_write_tokens: row.uncosted_cache_write_tokens,
            task_count: row.task_count,
        }
    }
}

async fn get_issue_usage(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };

    match task_usage::get_issue_usage_summary(&state.pool, issue.id).await {
        Ok(Some(row)) => Json(IssueUsageResponse::from(row)).into_response(),
        Ok(None) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to get issue usage",
        ),
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to get issue usage");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get issue usage",
            )
        }
    }
}

async fn list_attachments(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };

    match attachment::list_attachments_by_issue(&state.pool, issue.id, issue.workspace_id).await {
        Ok(attachments) => Json(
            attachments
                .iter()
                .map(AttachmentResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to list attachments");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list attachments",
            )
        }
    }
}

fn hydrate_task_user_ref(
    attribution: &mut serde_json::Map<String, Value>,
    key: &str,
    users: &HashMap<Uuid, user::GetUsersByIDsRow>,
) {
    let Some(reference) = attribution.get_mut(key).and_then(Value::as_object_mut) else {
        return;
    };
    let Some(id) = reference
        .get("id")
        .and_then(Value::as_str)
        .and_then(|id| Uuid::parse_str(id).ok())
    else {
        return;
    };
    let Some(user) = users.get(&id) else { return };
    if !user.name.is_empty() {
        reference.insert("name".into(), Value::String(user.name.clone()));
    }
    if !user.email.is_empty() {
        reference.insert("email".into(), Value::String(user.email.clone()));
    }
    if let Some(avatar_url) = user.avatar_url.as_deref().filter(|url| !url.is_empty()) {
        reference.insert("avatar_url".into(), Value::String(avatar_url.into()));
    }
}

async fn issue_task_maps(
    state: &HandlerState,
    issue: &Issue,
    tasks: &[AgentTaskQueue],
    include_usage: bool,
) -> Vec<Value> {
    let workspace_id = issue.workspace_id.to_string();
    let mut maps = task_maps(state, tasks, &workspace_id).await;

    if include_usage {
        if let Ok(rows) = task_usage::list_issue_task_usage(&state.pool, issue.id).await {
            let mut by_task = HashMap::<Uuid, Vec<Value>>::new();
            for row in rows {
                let Some(task_id) = row.task_id else { continue };
                let mut usage = serde_json::Map::new();
                if !row.provider.is_empty() {
                    usage.insert("provider".into(), Value::String(row.provider));
                }
                usage.insert("model".into(), Value::String(row.model));
                usage.insert("input_tokens".into(), json!(row.input_tokens));
                usage.insert("output_tokens".into(), json!(row.output_tokens));
                usage.insert("cache_read_tokens".into(), json!(row.cache_read_tokens));
                usage.insert("cache_write_tokens".into(), json!(row.cache_write_tokens));
                if let Some(cost) = row.cost_usd_ticks {
                    usage.insert("cost_usd_ticks".into(), json!(cost));
                }
                by_task
                    .entry(task_id)
                    .or_default()
                    .push(Value::Object(usage));
            }
            for (task, map) in tasks.iter().zip(&mut maps) {
                if let Some(usage) = by_task.remove(&task.id) {
                    if let Some(map) = map.as_object_mut() {
                        map.insert("usage".into(), Value::Array(usage));
                    }
                }
            }
        }
    }

    maps
}

pub(crate) async fn task_maps(
    state: &HandlerState,
    tasks: &[AgentTaskQueue],
    workspace_id: &str,
) -> Vec<Value> {
    let mut maps = tasks
        .iter()
        .map(|task| crate::task_json::task_to_map(task, workspace_id))
        .collect::<Vec<_>>();

    let mut user_ids = tasks
        .iter()
        .flat_map(|task| [task.accountable_user_id, task.originator_user_id])
        .flatten()
        .collect::<Vec<_>>();
    user_ids.sort_unstable();
    user_ids.dedup();
    if !user_ids.is_empty() {
        if let Ok(rows) = user::get_users_by_i_ds(&state.pool, user_ids).await {
            let users = rows
                .into_iter()
                .filter_map(|row| row.id.map(|id| (id, row)))
                .collect::<HashMap<_, _>>();
            for map in &mut maps {
                let Some(attribution) = map.get_mut("attribution").and_then(Value::as_object_mut)
                else {
                    continue;
                };
                hydrate_task_user_ref(attribution, "initiator", &users);
                hydrate_task_user_ref(attribution, "originator", &users);
            }
        }
    }

    maps
}

async fn get_active_tasks(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let tasks = agent::list_active_tasks_by_issue(&state.pool, issue.id)
        .await
        .unwrap_or_default();
    let tasks = issue_task_maps(&state, &issue, &tasks, false).await;
    Json(json!({ "tasks": tasks })).into_response()
}

async fn list_task_runs(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let tasks = match agent::list_tasks_by_issue(&state.pool, issue.id).await {
        Ok(tasks) => tasks,
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to list tasks");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list tasks");
        }
    };
    Json(issue_task_maps(&state, &issue, &tasks, true).await).into_response()
}

async fn cancel_task(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((id, task_id)): Path<(String, String)>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let task_id = match Uuid::parse_str(&task_id) {
        Ok(task_id) => task_id,
        Err(_) => return error_response(StatusCode::NOT_FOUND, "task not found"),
    };
    let existing = match agent::get_agent_task(&state.pool, task_id).await {
        Ok(Some(task)) if task.issue_id == Some(issue.id) => task,
        Ok(_) => return error_response(StatusCode::NOT_FOUND, "task not found"),
        Err(error) => {
            tracing::warn!(%error, %task_id, "failed to load task for cancellation");
            return error_response(StatusCode::NOT_FOUND, "task not found");
        }
    };
    let task = match state.tasks.cancel_task_by_user(existing.id).await {
        Ok(task) => task,
        Err(error) => return error_response(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let mut tasks = issue_task_maps(&state, &issue, &[task], false).await;
    Json(tasks.remove(0)).into_response()
}

async fn list_child_issues(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    match issue_q::list_child_issues(&state.pool, issue.id).await {
        Ok(children) => {
            let issues = enrich_issue_list(&state, &context, children).await;
            Json(json!({ "issues": issues })).into_response()
        }
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to list child issues");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list child issues",
            )
        }
    }
}

async fn list_labels_for_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    match labels_for_issues(&state, issue.workspace_id, &[issue.id]).await {
        Ok(mut labels) => Json(json!({
            "labels": labels.remove(&issue.id).unwrap_or_default(),
            "issue_revision": issue.revision,
        }))
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to list issue labels");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list labels")
        }
    }
}

#[derive(Debug, Clone)]
enum UpdateField<T> {
    Missing,
    Null,
    Value(T),
}

impl<T> UpdateField<T> {
    fn is_present(&self) -> bool {
        !matches!(self, Self::Missing)
    }
}

#[allow(clippy::result_large_err)]
fn update_field<T: serde::de::DeserializeOwned>(
    fields: &serde_json::Map<String, Value>,
    name: &str,
) -> Result<UpdateField<T>, Response> {
    let Some(value) = fields.get(name) else {
        return Ok(UpdateField::Missing);
    };
    if value.is_null() {
        return Ok(UpdateField::Null);
    }
    serde_json::from_value(value.clone())
        .map(UpdateField::Value)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid request body"))
}

#[allow(clippy::result_large_err)]
fn update_object(body: &[u8]) -> Result<serde_json::Map<String, Value>, Response> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(fields)) => Ok(fields),
        _ => Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid request body",
        )),
    }
}

async fn update_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let previous = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let fields = match update_object(&body) {
        Ok(fields) => fields,
        Err(response) => return response,
    };
    match apply_issue_update(&state, &context, &headers, previous, &fields, true).await {
        Ok(issue) => issue_response(&state, issue).await,
        Err(response) => response,
    }
}

async fn apply_issue_update(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    previous: Issue,
    fields: &serde_json::Map<String, Value>,
    notify_parent: bool,
) -> Result<Issue, Response> {
    let expected_revision = match update_field::<i64>(fields, "expected_revision")? {
        UpdateField::Value(value) if value > 0 => Some(value),
        UpdateField::Missing | UpdateField::Null => None,
        UpdateField::Value(_) => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "expected_revision must be a positive integer",
            ))
        }
    };
    if let Some(expected) = expected_revision {
        if expected != previous.revision {
            return Err(revision_conflict(&previous, expected, previous.revision));
        }
    }
    let suppress_run = match update_field::<bool>(fields, "suppress_run")? {
        UpdateField::Value(value) => value,
        UpdateField::Missing | UpdateField::Null => false,
    };
    let handoff_note = match update_field::<String>(fields, "handoff_note")? {
        UpdateField::Value(value) => value,
        UpdateField::Missing | UpdateField::Null => String::new(),
    };
    let attachment_ids = match update_field::<Vec<String>>(fields, "attachment_ids")? {
        UpdateField::Value(values) => uuid_strings(&values, "attachment_ids")
            .map_err(|message| error_response(StatusCode::BAD_REQUEST, &message))?,
        UpdateField::Missing | UpdateField::Null => Vec::new(),
    };

    let mut next = previous.clone();
    if let UpdateField::Value(value) = update_field::<String>(fields, "title")? {
        next.title = value;
    }
    if let UpdateField::Value(value) = update_field::<String>(fields, "description")? {
        next.description = Some(value);
    }
    if let UpdateField::Value(value) = update_field::<String>(fields, "status")? {
        next.status =
            match cordy_service::issue_status::resolve(&state.pool, previous.workspace_id, &value)
                .await
            {
                Ok(entry) => entry.key,
                Err(_) => return Err(invalid_status(state, previous.workspace_id, &value).await),
            };
    }
    if let UpdateField::Value(value) = update_field::<String>(fields, "priority")? {
        if !PRIORITIES.contains(&value.as_str()) {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                &format!(
                    "invalid priority: {value} (must be one of: {})",
                    PRIORITIES.join(", ")
                ),
            ));
        }
        next.priority = value;
    }
    if let UpdateField::Value(value) = update_field::<f64>(fields, "position")? {
        if !value.is_finite() {
            return Err(error_response(StatusCode::BAD_REQUEST, "invalid position"));
        }
        next.position = value;
    }

    let assignee_type = update_field::<String>(fields, "assignee_type")?;
    let assignee_id = update_field::<String>(fields, "assignee_id")?;
    let assignee_touched = assignee_type.is_present() || assignee_id.is_present();
    match assignee_type {
        UpdateField::Missing => {}
        UpdateField::Null => next.assignee_type = None,
        UpdateField::Value(value) => next.assignee_type = Some(value),
    }
    match assignee_id {
        UpdateField::Missing => {}
        UpdateField::Null => next.assignee_id = None,
        UpdateField::Value(value) => {
            next.assignee_id = Some(
                Uuid::parse_str(&value)
                    .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid assignee_id"))?,
            )
        }
    }
    if assignee_touched {
        match (next.assignee_type.as_deref(), next.assignee_id) {
            (None, None) => {}
            (Some(kind), Some(id)) => validate_assignee(state, context, kind, id)
                .await
                .map_err(|message| error_response(StatusCode::BAD_REQUEST, &message))?,
            _ => {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "assignee_type and assignee_id must be set together",
                ));
            }
        }
    }

    let start_date = update_field::<String>(fields, "start_date")?;
    if start_date.is_present() {
        next.start_date = match start_date {
            UpdateField::Missing | UpdateField::Null => None,
            UpdateField::Value(value) if value.is_empty() => None,
            UpdateField::Value(value) => {
                Some(NaiveDate::parse_from_str(&value, "%Y-%m-%d").map_err(|_| {
                    error_response(
                        StatusCode::BAD_REQUEST,
                        "invalid start_date format, expected YYYY-MM-DD",
                    )
                })?)
            }
        };
    }
    let due_date = update_field::<String>(fields, "due_date")?;
    if due_date.is_present() {
        next.due_date = match due_date {
            UpdateField::Missing | UpdateField::Null => None,
            UpdateField::Value(value) if value.is_empty() => None,
            UpdateField::Value(value) => {
                Some(NaiveDate::parse_from_str(&value, "%Y-%m-%d").map_err(|_| {
                    error_response(
                        StatusCode::BAD_REQUEST,
                        "invalid due_date format, expected YYYY-MM-DD",
                    )
                })?)
            }
        };
    }

    let parent = update_field::<String>(fields, "parent_issue_id")?;
    if parent.is_present() {
        next.parent_issue_id = match parent {
            UpdateField::Missing | UpdateField::Null => None,
            UpdateField::Value(value) => {
                let parent_id = Uuid::parse_str(&value).map_err(|_| {
                    error_response(StatusCode::BAD_REQUEST, "invalid parent_issue_id")
                })?;
                validate_parent(state, &previous, parent_id).await?;
                Some(parent_id)
            }
        };
    }

    let project = update_field::<String>(fields, "project_id")?;
    if project.is_present() {
        next.project_id = match project {
            UpdateField::Missing | UpdateField::Null => None,
            UpdateField::Value(value) => {
                let project_id = Uuid::parse_str(&value)
                    .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid project_id"))?;
                match cordy_db::queries::project::get_project_in_workspace(
                    &state.pool,
                    project_id,
                    previous.workspace_id,
                )
                .await
                {
                    Ok(Some(_)) => Some(project_id),
                    Ok(None) => {
                        return Err(error_response(
                            StatusCode::BAD_REQUEST,
                            "project not found in this workspace",
                        ))
                    }
                    Err(error) => {
                        tracing::warn!(%error, %project_id, "failed to validate issue project");
                        return Err(error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "failed to validate project",
                        ));
                    }
                }
            }
        };
    }

    let stage = update_field::<i32>(fields, "stage")?;
    if stage.is_present() {
        next.stage = match stage {
            UpdateField::Missing | UpdateField::Null => None,
            UpdateField::Value(value) if value >= 1 => Some(value),
            UpdateField::Value(_) => {
                return Err(error_response(
                    StatusCode::BAD_REQUEST,
                    "stage must be >= 1",
                ))
            }
        };
    }

    let mut tx = state.pool.begin().await.map_err(|error| {
        tracing::warn!(%error, issue_id = %previous.id, "failed to begin issue update");
        error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
    })?;
    if fields.contains_key("status") {
        cordy_db::queries::issue_status::lock_issue_status_catalog_shared(
            &mut *tx,
            previous.workspace_id,
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to lock issue status catalog");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
        })?;
        next.status = cordy_service::issue_status::resolve(
            &mut *tx,
            previous.workspace_id,
            &next.status,
        )
        .await
        .map_err(|_| {
            error_response(
                StatusCode::CONFLICT,
                "the target status was archived while this request was in flight; reload the status list and retry",
            )
        })?
        .key;
    }
    if !attachment_ids.is_empty() {
        attachment::lock_attachments_for_issue_link(
            &mut *tx,
            previous.workspace_id,
            attachment_ids.clone(),
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, issue_id = %previous.id, "failed to lock issue attachments");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
        })?;
    }
    let locked = sqlx::query_as::<_, Issue>(
        "SELECT * FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
    )
    .bind(previous.id)
    .bind(previous.workspace_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| {
        tracing::warn!(%error, issue_id = %previous.id, "failed to lock issue");
        error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
    })?
    .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "issue not found"))?;
    if let Some(expected) = expected_revision {
        if locked.revision != expected {
            return Err(revision_conflict(&locked, expected, locked.revision));
        }
    }

    refresh_untouched_fields(&mut next, &locked, fields);
    if let (UpdateField::Value(incoming), UpdateField::Value(base)) = (
        update_field::<String>(fields, "title")?,
        update_field::<String>(fields, "title_base")?,
    ) {
        if locked.title != base && locked.title != incoming {
            return Err(edit_conflict(&locked));
        }
    }
    if let UpdateField::Value(incoming) = update_field::<String>(fields, "description")? {
        let base = match update_field::<String>(fields, "description_base")? {
            UpdateField::Value(value) => Some(value),
            UpdateField::Missing | UpdateField::Null => None,
        };
        let attachments = attachment::list_attachments_by_issue(
            &mut *tx,
            locked.id,
            locked.workspace_id,
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, issue_id = %locked.id, "failed to load description attachments");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
        })?;
        let current = locked.description.clone().unwrap_or_default();
        if let Some(base) = base.as_deref() {
            if current != base && current != incoming {
                let base_with_late_media =
                    merge_channel_media_description(&current, base, Some(base), &attachments);
                if current != base_with_late_media {
                    return Err(edit_conflict(&locked));
                }
            }
        }
        next.description = Some(merge_channel_media_description(
            &current,
            &incoming,
            base.as_deref(),
            &attachments,
        ));
    }

    let previous = locked;
    let did_change = issue_mutable_fields_differ(&previous, &next);
    if !did_change && attachment_ids.is_empty() {
        return Ok(previous);
    }
    let did_activity = issue_activity_fields_differ(&previous, &next);
    let mut updated = if did_change {
        let updated = sqlx::query_as::<_, Issue>(
        r#"UPDATE issue SET
title = $3, description = $4, status = $5, priority = $6,
assignee_type = $7, assignee_id = $8, position = $9, start_date = $10,
due_date = $11, parent_issue_id = $12, project_id = $13, stage = $14,
revision = revision + 1, updated_at = now(),
last_activity_at = CASE WHEN $15 THEN GREATEST(COALESCE(last_activity_at, updated_at), now()) ELSE last_activity_at END
WHERE id = $1 AND workspace_id = $2
  AND ($16::bigint IS NULL OR revision = $16)
RETURNING *"#,
    )
        .bind(previous.id)
        .bind(previous.workspace_id)
        .bind(&next.title)
        .bind(&next.description)
        .bind(&next.status)
        .bind(&next.priority)
        .bind(&next.assignee_type)
        .bind(next.assignee_id)
        .bind(next.position)
        .bind(next.start_date)
        .bind(next.due_date)
        .bind(next.parent_issue_id)
        .bind(next.project_id)
        .bind(next.stage)
        .bind(did_activity)
        .bind(expected_revision)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| {
            tracing::warn!(%error, issue_id = %previous.id, "failed to update issue");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
        })?;
        let Some(updated) = updated else {
            let actual =
                issue_q::get_issue_in_workspace(&mut *tx, previous.id, previous.workspace_id)
                    .await
                    .ok()
                    .flatten()
                    .map(|issue| issue.revision)
                    .unwrap_or(previous.revision);
            return Err(revision_conflict(
                &previous,
                expected_revision.unwrap_or(previous.revision),
                actual,
            ));
        };
        updated
    } else {
        previous.clone()
    };
    let mut attachments_changed = false;
    if !attachment_ids.is_empty() {
        let linked = cordy_db::queries::attachment::link_attachments_to_issue(
            &mut *tx,
            previous.id,
            previous.workspace_id,
            attachment_ids,
            !did_change,
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, issue_id = %previous.id, "failed to link issue attachments");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to link issue attachments",
            )
        })?;
        if linked.is_some_and(|result| result.linked_count > 0) {
            attachments_changed = true;
            if let Ok(Some(current)) =
                issue_q::get_issue_in_workspace(&mut *tx, previous.id, previous.workspace_id).await
            {
                updated = current;
            }
        }
    }
    tx.commit().await.map_err(|error| {
        tracing::warn!(%error, issue_id = %previous.id, "failed to commit issue update");
        error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update issue")
    })?;
    let (actor_type, actor_id, task_id) = mutation_actor(state, context, headers).await;
    publish_issue_updated(state, &previous, &updated, &actor_type, actor_id, task_id).await;
    if attachments_changed {
        publish_issue_attachments_changed(state, &updated, &actor_type, actor_id, task_id);
    }
    if previous.assignee_type != updated.assignee_type
        || previous.assignee_id != updated.assignee_id
    {
        record_assignee_activity(state, &previous, &updated, &actor_type, actor_id).await;
    }
    let assignee_changed = previous.assignee_type != updated.assignee_type
        || previous.assignee_id != updated.assignee_id;
    let status_changed = previous.status != updated.status;
    if !suppress_run {
        let is_self_loop = if let Some(task_id) = task_id {
            agent::get_agent_task(&state.pool, task_id)
                .await
                .ok()
                .flatten()
                .is_some_and(|task| task.issue_id == Some(updated.id))
        } else {
            false
        };
        let suppress_active_self_assignment = if actor_type == "agent"
            && updated.assignee_type.as_deref() == Some("agent")
            && updated.assignee_id == Some(actor_id)
        {
            agent::has_active_task_for_issue_and_agent(&state.pool, updated.id, actor_id)
                .await
                .map(|active| active.unwrap_or(true))
                .unwrap_or(true)
        } else {
            false
        };
        let trigger = state
            .issues
            .will_enqueue_run(
                IssueTriggerInput {
                    issue: updated.clone(),
                    prev_status: previous.status.clone(),
                    is_create: false,
                    assignee_changed,
                    status_changed,
                },
                IssueTriggerProbe {
                    can_access_agent: None,
                    is_self_loop: Some(Box::new(move |_| is_self_loop)),
                    suppress_active_self_assignment: Some(Box::new(move |_| {
                        suppress_active_self_assignment
                    })),
                },
            )
            .await;
        if let Some(trigger) = trigger {
            let actor_user_id = (actor_type == "member").then_some(actor_id);
            let result = if trigger.assignee_type == "squad" {
                state
                    .tasks
                    .enqueue_task_for_squad_leader_with_handoff(
                        &updated,
                        trigger.agent_id,
                        updated.assignee_id.unwrap_or_default(),
                        &handoff_note,
                        actor_user_id,
                    )
                    .await
            } else {
                state
                    .tasks
                    .enqueue_task_for_issue_with_handoff(&updated, &handoff_note, actor_user_id)
                    .await
            };
            if let Err(error) = result {
                tracing::warn!(%error, issue_id = %updated.id, "failed to enqueue updated issue");
            }
        }
    }
    if notify_parent && status_changed {
        notify_parent_of_child_done(state, &previous, &updated).await;
    }
    Ok(updated)
}

fn revision_conflict(issue: &Issue, expected: i64, actual: i64) -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "resource changed since it was loaded",
            "code": "revision_conflict",
            "resource_type": "issue",
            "resource_id": issue.id.to_string(),
            "expected_revision": expected,
            "actual_revision": actual,
        })),
    )
        .into_response()
}

fn issue_mutable_fields_differ(left: &Issue, right: &Issue) -> bool {
    left.title != right.title
        || left.description != right.description
        || left.status != right.status
        || left.priority != right.priority
        || left.assignee_type != right.assignee_type
        || left.assignee_id != right.assignee_id
        || left.position != right.position
        || left.start_date != right.start_date
        || left.due_date != right.due_date
        || left.parent_issue_id != right.parent_issue_id
        || left.project_id != right.project_id
        || left.stage != right.stage
}

fn issue_activity_fields_differ(left: &Issue, right: &Issue) -> bool {
    left.title != right.title
        || left.description != right.description
        || left.status != right.status
        || left.priority != right.priority
        || left.assignee_type != right.assignee_type
        || left.assignee_id != right.assignee_id
        || left.start_date != right.start_date
        || left.due_date != right.due_date
        || left.parent_issue_id != right.parent_issue_id
        || left.project_id != right.project_id
        || left.stage != right.stage
}

fn refresh_untouched_fields(
    next: &mut Issue,
    current: &Issue,
    fields: &serde_json::Map<String, Value>,
) {
    if !fields.contains_key("title") {
        next.title = current.title.clone();
    }
    if !fields.contains_key("description") {
        next.description = current.description.clone();
    }
    if !fields.contains_key("status") {
        next.status = current.status.clone();
    }
    if !fields.contains_key("priority") {
        next.priority = current.priority.clone();
    }
    if !fields.contains_key("position") {
        next.position = current.position;
    }
    if !fields.contains_key("assignee_type") && !fields.contains_key("assignee_id") {
        next.assignee_type = current.assignee_type.clone();
        next.assignee_id = current.assignee_id;
    }
    if !fields.contains_key("start_date") {
        next.start_date = current.start_date;
    }
    if !fields.contains_key("due_date") {
        next.due_date = current.due_date;
    }
    if !fields.contains_key("parent_issue_id") {
        next.parent_issue_id = current.parent_issue_id;
    }
    if !fields.contains_key("project_id") {
        next.project_id = current.project_id;
    }
    if !fields.contains_key("stage") {
        next.stage = current.stage;
    }
}

fn edit_conflict(issue: &Issue) -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "resource changed since it was loaded",
            "code": "edit_conflict",
            "resource_type": "issue",
            "resource_id": issue.id.to_string(),
        })),
    )
        .into_response()
}

fn marked_media_ids(markdown: &str) -> Vec<Uuid> {
    const PREFIX: &str = "<!-- cordy:channel-media:";
    let mut ids = Vec::new();
    let mut remaining = markdown;
    while let Some(index) = remaining.find(PREFIX) {
        remaining = &remaining[index + PREFIX.len()..];
        let Some(raw) = remaining.get(..36) else {
            break;
        };
        if remaining.get(36..40) == Some(" -->") {
            if let Ok(id) = Uuid::parse_str(raw) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        remaining = remaining.get(36..).unwrap_or_default();
    }
    ids
}

fn media_marker(id: Uuid) -> String {
    format!("<!-- cordy:channel-media:{id} -->")
}

fn append_markdown(markdown: &str, block: &str) -> String {
    if markdown.is_empty() {
        block.to_string()
    } else {
        format!("{markdown}\n\n{block}")
    }
}

fn merge_channel_media_description(
    current: &str,
    incoming: &str,
    base: Option<&str>,
    attachments: &[Attachment],
) -> String {
    let current_ids = marked_media_ids(current);
    if current_ids.is_empty() {
        return incoming.to_string();
    }
    let base_ids = base.map(marked_media_ids).unwrap_or_default();
    let mut merged = incoming.to_string();
    for id in current_ids {
        let Some(attachment) = attachments.iter().find(|attachment| attachment.id == id) else {
            continue;
        };
        let path = format!("/api/attachments/{id}/download");
        let has_link = merged.contains(&path);
        if base.is_some() && base_ids.contains(&id) && !has_link {
            continue;
        }
        if !has_link {
            let block = if attachment.content_type.starts_with("image/") {
                format!("![]({path})\n\n{}", media_marker(id))
            } else {
                let label = attachment
                    .filename
                    .replace('\\', "\\\\")
                    .replace('[', "\\[")
                    .replace(']', "\\]")
                    .replace(['\r', '\n'], " ");
                format!(
                    "[{}]({path})\n\n{}",
                    if label.is_empty() {
                        "attachment"
                    } else {
                        &label
                    },
                    media_marker(id)
                )
            };
            merged = append_markdown(&merged, &block);
        } else if !merged.contains(&media_marker(id)) {
            merged = append_markdown(&merged, &media_marker(id));
        }
    }
    merged
}

async fn validate_parent(
    state: &HandlerState,
    issue: &Issue,
    parent_id: Uuid,
) -> Result<(), Response> {
    if parent_id == issue.id {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "an issue cannot be its own parent",
        ));
    }
    let mut cursor = parent_id;
    for _ in 0..10 {
        let parent = issue_q::get_issue_in_workspace(&state.pool, cursor, issue.workspace_id)
            .await
            .map_err(|error| {
                tracing::warn!(%error, %parent_id, "failed to validate parent issue");
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to validate parent issue",
                )
            })?;
        let Some(parent) = parent else {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "parent issue not found in this workspace",
            ));
        };
        let Some(ancestor) = parent.parent_issue_id else {
            return Ok(());
        };
        if ancestor == issue.id {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "circular parent relationship detected",
            ));
        }
        cursor = ancestor;
    }
    Ok(())
}

async fn issue_response(state: &HandlerState, issue: Issue) -> Response {
    let mut response =
        IssueResponse::from_issue(&issue, &issue_prefix(state, issue.workspace_id).await);
    response.status_category = Some(
        cordy_service::issue_status::effective(&state.pool, issue.workspace_id, &issue.status)
            .await,
    );
    Json(response).into_response()
}

pub(crate) async fn mutation_actor(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
) -> (String, Uuid, Option<Uuid>) {
    if let Some((task_id, agent_id)) = trusted_agent_task(state, context, headers).await {
        ("agent".to_string(), agent_id, Some(task_id))
    } else {
        ("member".to_string(), context.member.user_id, None)
    }
}

async fn publish_issue_updated(
    state: &HandlerState,
    previous: &Issue,
    issue: &Issue,
    actor_type: &str,
    actor_id: Uuid,
    task_id: Option<Uuid>,
) {
    let prefix = issue_prefix(state, issue.workspace_id).await;
    let category =
        cordy_service::issue_status::effective(&state.pool, issue.workspace_id, &issue.status)
            .await;
    let mut response = IssueResponse::from_issue(issue, &prefix);
    response.status_category = Some(category);
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_ISSUE_UPDATED.to_string(),
        workspace_id: issue.workspace_id.to_string(),
        actor_type: actor_type.to_string(),
        actor_id: actor_id.to_string(),
        payload: json!({
            "issue": response,
            "assignee_changed": previous.assignee_type != issue.assignee_type || previous.assignee_id != issue.assignee_id,
            "status_changed": previous.status != issue.status,
            "priority_changed": previous.priority != issue.priority,
            "project_changed": previous.project_id != issue.project_id,
        }),
        task_id: task_id.map(|id| id.to_string()).unwrap_or_default(),
        chat_session_id: String::new(),
    });
}

/// Applies the PR-merge auto-completion path without bypassing issue-domain
/// side effects. Both GitHub and token-based VCS webhooks use the same
/// combined sibling barrier before calling this helper.
pub(crate) async fn advance_issue_to_done_from_pr(
    state: &HandlerState,
    previous: &Issue,
    source: &str,
) -> Option<Issue> {
    let current_category = cordy_service::issue_status::effective(
        &state.pool,
        previous.workspace_id,
        &previous.status,
    )
    .await;
    if terminal_category(&current_category) {
        return None;
    }
    let updated = match issue_q::update_issue_status(
        &state.pool,
        previous.id,
        "done",
        previous.workspace_id,
    )
    .await
    {
        Ok(Some(issue)) => issue,
        Ok(None) => return None,
        Err(error) => {
            tracing::warn!(%error, issue_id = %previous.id, "failed to complete issue from pull request");
            return None;
        }
    };
    notify_parent_of_child_done(state, previous, &updated).await;
    let prefix = issue_prefix(state, updated.workspace_id).await;
    let mut response = IssueResponse::from_issue(&updated, &prefix);
    response.status_category = Some("done".into());
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_ISSUE_UPDATED.into(),
        workspace_id: updated.workspace_id.to_string(),
        actor_type: "system".into(),
        payload: pr_completion_event_payload(previous, response, source),
        ..Default::default()
    });
    Some(updated)
}

fn pr_completion_event_payload(previous: &Issue, response: IssueResponse, source: &str) -> Value {
    json!({
        "issue": response,
        "assignee_changed": false,
        "status_changed": true,
        "priority_changed": false,
        "project_changed": false,
        "prev_status": previous.status,
        "creator_type": previous.creator_type,
        "creator_id": previous.creator_id.to_string(),
        "source": source,
    })
}

fn publish_issue_attachments_changed(
    state: &HandlerState,
    issue: &Issue,
    actor_type: &str,
    actor_id: Uuid,
    task_id: Option<Uuid>,
) {
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_ISSUE_ATTACHMENTS_CHANGED.to_string(),
        workspace_id: issue.workspace_id.to_string(),
        actor_type: actor_type.to_string(),
        actor_id: actor_id.to_string(),
        payload: json!({
            "issue_id": issue.id.to_string(),
            "issue_revision": issue.revision,
        }),
        task_id: task_id.map(|id| id.to_string()).unwrap_or_default(),
        chat_session_id: String::new(),
    });
}

async fn record_assignee_activity(
    state: &HandlerState,
    previous: &Issue,
    issue: &Issue,
    actor_type: &str,
    actor_id: Uuid,
) {
    let details = json!({
        "from_type": previous.assignee_type,
        "from_id": previous.assignee_id.map(|id| id.to_string()),
        "to_type": issue.assignee_type,
        "to_id": issue.assignee_id.map(|id| id.to_string()),
    });
    if let Err(error) = cordy_db::queries::activity::create_activity(
        &state.pool,
        issue.workspace_id,
        issue.id,
        Some(actor_type),
        actor_id,
        "assignee_changed",
        &details,
        cordy_db::dbid::new_v7(),
    )
    .await
    {
        tracing::warn!(%error, issue_id = %issue.id, "failed to record assignee activity");
    }
}

fn terminal_category(category: &str) -> bool {
    matches!(category, "done" | "cancelled")
}

async fn notify_parent_of_child_done(state: &HandlerState, previous: &Issue, issue: &Issue) {
    let Some(parent_id) = issue.parent_issue_id else {
        return;
    };
    let mut resolver = cordy_service::issue_status::Resolver::new(issue.workspace_id);
    let previous_category = resolver.effective(&state.pool, &previous.status).await;
    let current_category = resolver.effective(&state.pool, &issue.status).await;
    if terminal_category(&previous_category) || !terminal_category(&current_category) {
        return;
    }
    let Some(parent) = issue_q::get_issue_in_workspace(&state.pool, parent_id, issue.workspace_id)
        .await
        .ok()
        .flatten()
    else {
        return;
    };
    let parent_category = resolver.effective(&state.pool, &parent.status).await;
    if matches!(parent_category.as_str(), "backlog" | "done" | "cancelled")
        || parent.assignee_type.as_deref() == Some("member")
    {
        return;
    }
    let children = match issue_q::list_child_issues(&state.pool, parent.id).await {
        Ok(children) => children,
        Err(error) => {
            tracing::warn!(%error, parent_id = %parent.id, "failed to inspect child completion barrier");
            return;
        }
    };
    let staged = children.iter().any(|child| child.stage.is_some());
    if staged && issue.stage.is_none() {
        return;
    }
    let closed_stage = issue.stage;
    for child in &children {
        if staged {
            let Some(child_stage) = child.stage else {
                continue;
            };
            if child_stage > closed_stage.unwrap_or_default() {
                continue;
            }
        }
        let category = resolver.effective(&state.pool, &child.status).await;
        if !terminal_category(&category) {
            return;
        }
    }

    let (mention, target_agent, squad_id) =
        match (parent.assignee_type.as_deref(), parent.assignee_id) {
            (Some("agent"), Some(agent_id)) => (
                format!("[@assignee](mention://agent/{agent_id}) "),
                Some(agent_id),
                None,
            ),
            (Some("squad"), Some(squad_id)) => {
                let leader =
                    squad::get_squad_in_workspace(&state.pool, squad_id, parent.workspace_id)
                        .await
                        .ok()
                        .flatten()
                        .map(|squad| squad.leader_id);
                (
                    format!("[@squad](mention://squad/{squad_id}) "),
                    leader,
                    Some(squad_id),
                )
            }
            _ => (String::new(), None, None),
        };
    let identifier = format!(
        "{}-{}",
        issue_prefix(state, issue.workspace_id).await,
        issue.number
    );
    let progress = if let Some(stage) = closed_stage {
        format!("Stage {stage} is complete")
    } else {
        "All sub-issues are complete".to_string()
    };
    let content = format!(
        "{mention}{progress} — the last sub-issue [{identifier}](mention://issue/{}) — \"{}\" — just finished. Continue the parent or move it to review when complete.",
        issue.id,
        issue.title.replace(['\r', '\n'], " ")
    );
    let created = match cordy_db::queries::comment::create_comment(
        &state.pool,
        parent.id,
        parent.workspace_id,
        "system",
        Uuid::nil(),
        &content,
        "system",
        None,
        None,
        None,
        None,
        cordy_db::dbid::new_v7(),
    )
    .await
    {
        Ok(Some(created)) => created,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%error, parent_id = %parent.id, "failed to create child completion comment");
            return;
        }
    };
    let comment_id = created.id;
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_COMMENT_CREATED.to_string(),
        workspace_id: parent.workspace_id.to_string(),
        actor_type: "system".to_string(),
        actor_id: String::new(),
        payload: json!({
            "comment": {
                "id": created.id.map(|id| id.to_string()),
                "issue_id": created.issue_id.map(|id| id.to_string()),
                "author_type": created.author_type,
                "author_id": created.author_id.map(|id| id.to_string()),
                "content": created.content,
                "type": created.type_,
                "revision": created.revision,
            },
            "issue_title": parent.title,
            "issue_revision": created.issue_revision,
        }),
        task_id: String::new(),
        chat_session_id: String::new(),
    });
    if let (Some(agent_id), Some(comment_id)) = (target_agent, comment_id) {
        let result = if let Some(squad_id) = squad_id {
            state
                .tasks
                .enqueue_task_for_squad_leader(&parent, agent_id, squad_id, Some(comment_id))
                .await
        } else {
            state
                .tasks
                .enqueue_task_for_mention(&parent, agent_id, Some(comment_id))
                .await
        };
        if let Err(error) = result {
            tracing::warn!(%error, parent_id = %parent.id, "failed to wake parent assignee");
        }
    }
}

async fn batch_update_issues(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let root = match update_object(&body) {
        Ok(fields) => fields,
        Err(response) => return response,
    };
    let issue_ids = match root.get("issue_ids").cloned() {
        Some(value) => match serde_json::from_value::<Vec<String>>(value) {
            Ok(ids) if !ids.is_empty() => ids,
            _ => return error_response(StatusCode::BAD_REQUEST, "issue_ids is required"),
        },
        None => return error_response(StatusCode::BAD_REQUEST, "issue_ids is required"),
    };
    let updates = match root.get("updates") {
        Some(Value::Object(fields)) => fields,
        _ => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let mutation_keys = [
        "title",
        "description",
        "status",
        "priority",
        "position",
        "assignee_type",
        "assignee_id",
        "start_date",
        "due_date",
        "parent_issue_id",
        "project_id",
        "stage",
    ];
    if !mutation_keys.iter().any(|key| updates.contains_key(*key)) {
        return Json(json!({ "updated": 0 })).into_response();
    }

    if let Some(Value::String(status)) = updates.get("status") {
        if cordy_service::issue_status::resolve(&state.pool, context.member.workspace_id, status)
            .await
            .is_err()
        {
            return invalid_status(&state, context.member.workspace_id, status).await;
        }
    } else if updates.contains_key("status") {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    }
    if let Some(Value::String(priority)) = updates.get("priority") {
        if !PRIORITIES.contains(&priority.as_str()) {
            return error_response(StatusCode::BAD_REQUEST, "invalid priority");
        }
    } else if updates.contains_key("priority") {
        return error_response(StatusCode::BAD_REQUEST, "invalid request body");
    }
    if let Some(project) = updates.get("project_id") {
        if !project.is_null() {
            let Some(raw) = project.as_str() else {
                return error_response(StatusCode::BAD_REQUEST, "invalid project_id");
            };
            let Ok(project_id) = Uuid::parse_str(raw) else {
                return error_response(StatusCode::BAD_REQUEST, "invalid project_id");
            };
            if !matches!(
                cordy_db::queries::project::get_project_in_workspace(
                    &state.pool,
                    project_id,
                    context.member.workspace_id,
                )
                .await,
                Ok(Some(_))
            ) {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "project not found in this workspace",
                );
            }
        }
    }

    let mut updated = 0usize;
    let mut parent_notifications = HashMap::<Uuid, (Issue, Issue)>::new();
    for raw_id in issue_ids {
        let Ok(id) = Uuid::parse_str(&raw_id) else {
            continue;
        };
        let previous =
            match issue_q::get_issue_in_workspace(&state.pool, id, context.member.workspace_id)
                .await
            {
                Ok(Some(issue)) => issue,
                _ => continue,
            };
        let previous_snapshot = previous.clone();
        if let Ok(issue) =
            apply_issue_update(&state, &context, &headers, previous, updates, false).await
        {
            if previous_snapshot.status != issue.status {
                if let Some(parent_id) = issue.parent_issue_id {
                    let replace = parent_notifications
                        .get(&parent_id)
                        .is_none_or(|(_, current)| issue.stage > current.stage);
                    if replace {
                        parent_notifications.insert(parent_id, (previous_snapshot, issue));
                    }
                }
            }
            updated += 1;
        }
    }
    for (_, (previous, issue)) in parent_notifications {
        notify_parent_of_child_done(&state, &previous, &issue).await;
    }
    Json(json!({ "updated": updated })).into_response()
}

#[derive(Debug, Deserialize)]
struct AttachLabelRequest {
    label_id: String,
}

async fn attach_label(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    Json(request): Json<AttachLabelRequest>,
) -> Response {
    if request.label_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "label_id is required");
    }
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let label_id = match Uuid::parse_str(&request.label_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid label_id"),
    };
    let label = match issue_label::get_label(&state.pool, label_id, issue.workspace_id).await {
        Ok(Some(label)) if label.resource_type == "issue" => label,
        Ok(Some(_)) => return error_response(StatusCode::NOT_FOUND, "issue label not found"),
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "label not found"),
        Err(error) => {
            tracing::warn!(%error, %label_id, "failed to load issue label");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to attach label");
        }
    };
    let result = match issue_label::attach_label_to_issue(
        &state.pool,
        issue.id,
        label.id,
        issue.workspace_id,
    )
    .await
    {
        Ok(Some(result)) => result,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "label not found"),
        Err(error) => {
            tracing::warn!(%error, %label_id, "failed to attach issue label");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to attach label");
        }
    };
    label_mutation_response(
        &state,
        &context,
        &issue,
        result.changed,
        result.issue_revision,
    )
    .await
}

async fn detach_label(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((id, label_id)): Path<(String, String)>,
) -> Response {
    let issue = match resolve_issue(&state, &context, &id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let label_id = match Uuid::parse_str(&label_id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid label id"),
    };
    match issue_label::get_label(&state.pool, label_id, issue.workspace_id).await {
        Ok(Some(label)) if label.resource_type == "issue" => {}
        Ok(Some(_)) => return error_response(StatusCode::NOT_FOUND, "issue label not found"),
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "label not found"),
        Err(error) => {
            tracing::warn!(%error, %label_id, "failed to load issue label");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to detach label");
        }
    }
    let result = match issue_label::detach_label_from_issue(
        &state.pool,
        issue.id,
        label_id,
        issue.workspace_id,
    )
    .await
    {
        Ok(Some(result)) => result,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "label not found"),
        Err(error) => {
            tracing::warn!(%error, %label_id, "failed to detach issue label");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to detach label");
        }
    };
    label_mutation_response(
        &state,
        &context,
        &issue,
        result.changed,
        result.issue_revision,
    )
    .await
}

async fn label_mutation_response(
    state: &HandlerState,
    context: &WorkspaceContext,
    issue: &Issue,
    changed: bool,
    revision: i64,
) -> Response {
    let labels = match labels_for_issues(state, issue.workspace_id, &[issue.id]).await {
        Ok(mut labels) => labels.remove(&issue.id).unwrap_or_default(),
        Err(error) => {
            tracing::warn!(%error, issue_id = %issue.id, "failed to reload issue labels");
            return Json(json!({})).into_response();
        }
    };
    if changed {
        state.bus.publish(&cordy_events::Event {
            event_type: cordy_protocol::EVENT_ISSUE_LABELS_CHANGED.to_string(),
            workspace_id: issue.workspace_id.to_string(),
            actor_type: "member".to_string(),
            actor_id: context.member.user_id.to_string(),
            payload: json!({
                "issue_id": issue.id.to_string(),
                "labels": labels,
                "issue_revision": revision,
            }),
            task_id: String::new(),
            chat_session_id: String::new(),
        });
    }
    if revision > 0 {
        Json(json!({ "labels": labels, "issue_revision": revision })).into_response()
    } else {
        Json(json!({ "labels": labels })).into_response()
    }
}

#[derive(Debug, Deserialize)]
struct CreateIssueRequest {
    title: String,
    description: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    priority: String,
    assignee_type: Option<String>,
    assignee_id: Option<String>,
    parent_issue_id: Option<String>,
    project_id: Option<String>,
    stage: Option<i32>,
    start_date: Option<String>,
    due_date: Option<String>,
    #[serde(default)]
    attachment_ids: Vec<String>,
    #[serde(default)]
    label_ids: Vec<String>,
    origin_type: Option<String>,
    origin_id: Option<String>,
    #[serde(default)]
    allow_duplicate: bool,
}

async fn create_issue(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    body: Result<Json<CreateIssueRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match body {
        Ok(body) => body,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.title.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "title is required");
    }
    let workspace_id = context.member.workspace_id;
    let status = if request.status.is_empty() {
        "todo".to_string()
    } else {
        request.status
    };
    let (status, status_category) =
        match cordy_service::issue_status::resolve(&state.pool, workspace_id, &status).await {
            Ok(entry) => (entry.key, entry.category),
            Err(_) => return invalid_status(&state, workspace_id, &status).await,
        };
    let priority = if request.priority.is_empty() {
        "none".to_string()
    } else {
        request.priority
    };
    if !PRIORITIES.contains(&priority.as_str()) {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!(
                "invalid priority {:?}; valid values: {}",
                priority,
                PRIORITIES.join(", ")
            ),
        );
    }
    if request.stage.is_some_and(|stage| stage < 1) {
        return error_response(StatusCode::BAD_REQUEST, "stage must be >= 1");
    }
    let assignee_id = match optional_uuid(request.assignee_id.as_deref(), "assignee_id") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    if request.assignee_type.is_some() != assignee_id.is_some() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "assignee_type and assignee_id must be provided together",
        );
    }
    if request
        .assignee_type
        .as_deref()
        .is_some_and(|kind| !matches!(kind, "member" | "agent" | "squad"))
    {
        return error_response(StatusCode::BAD_REQUEST, "invalid assignee_type");
    }
    if let (Some(kind), Some(id)) = (request.assignee_type.as_deref(), assignee_id) {
        if let Err(message) = validate_assignee(&state, &context, kind, id).await {
            return error_response(StatusCode::BAD_REQUEST, &message);
        }
    }
    let parent_issue_id = match optional_uuid(request.parent_issue_id.as_deref(), "parent_issue_id")
    {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let project_id = match optional_uuid(request.project_id.as_deref(), "project_id") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let start_date = match optional_date(request.start_date.as_deref(), "start_date") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let due_date = match optional_date(request.due_date.as_deref(), "due_date") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let attachment_ids = match uuid_strings(&request.attachment_ids, "attachment_ids") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let label_ids = match uuid_strings(&request.label_ids, "label_ids") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    if request.origin_type.is_some() != request.origin_id.is_some() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "origin_type and origin_id must be provided together",
        );
    }
    if request
        .origin_type
        .as_deref()
        .is_some_and(|kind| kind != "quick_create")
    {
        return error_response(StatusCode::BAD_REQUEST, "unsupported origin_type");
    }
    let origin_id = match optional_uuid(request.origin_id.as_deref(), "origin_id") {
        Ok(value) => value,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let task_identity = trusted_agent_task(&state, &context, &headers).await;
    let (creator_type, creator_id) = task_identity
        .as_ref()
        .map(|(_, agent_id)| ("agent".to_string(), *agent_id))
        .unwrap_or_else(|| ("member".to_string(), context.member.user_id));
    let (origin_type, origin_id) = if request.origin_type.is_some() {
        (request.origin_type, origin_id)
    } else if let Some((task_id, _)) = task_identity {
        (Some("agent_create".to_string()), Some(task_id))
    } else {
        (None, None)
    };

    let prefix = issue_prefix(&state, workspace_id).await;
    let broadcast_prefix = prefix.clone();
    let broadcast_status_category = status_category.clone();
    let result = state
        .issues
        .create(
            IssueCreateParams {
                workspace_id,
                title: request.title,
                description: request.description,
                status,
                priority,
                assignee_type: request.assignee_type,
                assignee_id,
                creator_type,
                creator_id,
                parent_issue_id,
                project_id,
                start_date,
                due_date,
                origin_type,
                origin_id,
                attachment_ids,
                label_ids,
                allow_duplicate: request.allow_duplicate,
                stage: request.stage,
            },
            IssueCreateOpts {
                actor_id: creator_id.to_string(),
                platform: headers
                    .get("x-client-platform")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string(),
                broadcast_payload: Some(Arc::new(move |issue, _, labels| {
                    let mut response = IssueResponse::from_issue(issue, &broadcast_prefix);
                    response.status_category = Some(broadcast_status_category.clone());
                    response.labels = Some(labels.iter().map(LabelResponse::from).collect());
                    json!({ "issue": response })
                })),
                ..IssueCreateOpts::default()
            },
        )
        .await;

    match result {
        Ok(result) => {
            let Some(issue) = result.issue else {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create issue",
                );
            };
            let mut response = IssueResponse::from_issue(&issue, &prefix);
            response.status_category = Some(status_category);
            response.labels = Some(result.labels.iter().map(LabelResponse::from).collect());
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(IssueCreateError::ActiveDuplicate { duplicate }) => {
            let duplicate = duplicate.map(|issue| IssueResponse::from_issue(&issue, &prefix));
            (StatusCode::CONFLICT, Json(json!({
                "code": "active_duplicate_issue",
                "error": "an active duplicate issue already exists",
                "issue": duplicate,
            })))
                .into_response()
        }
        Err(IssueCreateError::ParentIssueNotFound) => error_response(
            StatusCode::BAD_REQUEST,
            "parent issue not found in this workspace",
        ),
        Err(IssueCreateError::ProjectNotFound) => error_response(
            StatusCode::BAD_REQUEST,
            "project not found in this workspace",
        ),
        Err(IssueCreateError::LabelNotFound) => error_response(
            StatusCode::BAD_REQUEST,
            "one or more labels not found in this workspace",
        ),
        Err(IssueCreateError::StatusUnavailable) => error_response(
            StatusCode::CONFLICT,
            "the target status was archived while this request was in flight; reload the status list and retry",
        ),
        Err(error) => {
            tracing::warn!(%error, %workspace_id, "failed to create issue");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to create issue")
        }
    }
}

async fn trusted_agent_task(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
) -> Option<(Uuid, Uuid)> {
    let agent_id = header_uuid(headers, "x-agent-id")?;
    let task_id = header_uuid(headers, "x-task-id")?;
    let task = agent::get_agent_task(&state.pool, task_id)
        .await
        .ok()
        .flatten()?;
    if task.agent_id != agent_id {
        return None;
    }
    agent::get_agent_in_workspace(&state.pool, agent_id, context.member.workspace_id)
        .await
        .ok()
        .flatten()
        .filter(|agent| agent.archived_at.is_none())?;
    Some((task_id, agent_id))
}

fn header_uuid(headers: &HeaderMap, name: &str) -> Option<Uuid> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
}

async fn validate_assignee(
    state: &HandlerState,
    context: &WorkspaceContext,
    kind: &str,
    id: Uuid,
) -> Result<(), String> {
    let workspace_id = context.member.workspace_id;
    match kind {
        "member" => {
            if member::get_member_by_user_and_workspace(&state.pool, id, workspace_id)
                .await
                .ok()
                .flatten()
                .is_none()
            {
                return Err("assignee member not found in this workspace".to_string());
            }
        }
        "agent" => {
            let target = agent::get_agent_in_workspace(&state.pool, id, workspace_id)
                .await
                .ok()
                .flatten()
                .filter(|agent| agent.archived_at.is_none())
                .ok_or_else(|| "assignee agent not found in this workspace".to_string())?;
            if !can_member_invoke_agent(state, context.member.user_id, workspace_id, &target).await
            {
                return Err("you do not have permission to invoke this agent".to_string());
            }
        }
        "squad" => {
            let target = squad::get_squad_in_workspace(&state.pool, id, workspace_id)
                .await
                .ok()
                .flatten()
                .filter(|squad| squad.archived_at.is_none())
                .ok_or_else(|| "assignee squad not found in this workspace".to_string())?;
            let leader = agent::get_agent_in_workspace(&state.pool, target.leader_id, workspace_id)
                .await
                .ok()
                .flatten()
                .filter(|agent| agent.archived_at.is_none())
                .ok_or_else(|| "squad leader is unavailable".to_string())?;
            if !can_member_invoke_agent(state, context.member.user_id, workspace_id, &leader).await
            {
                return Err("you do not have permission to invoke this squad".to_string());
            }
        }
        _ => return Err("invalid assignee_type".to_string()),
    }
    Ok(())
}

async fn can_member_invoke_agent(
    state: &HandlerState,
    user_id: Uuid,
    workspace_id: Uuid,
    target: &cordy_db::models::Agent,
) -> bool {
    if target.owner_id == Some(user_id) {
        return true;
    }
    if target.permission_mode != "public_to" {
        return false;
    }
    let is_member = member::get_member_by_user_and_workspace(&state.pool, user_id, workspace_id)
        .await
        .ok()
        .flatten()
        .is_some();
    agent_invocation_target::list_agent_invocation_targets(&state.pool, target.id)
        .await
        .unwrap_or_default()
        .iter()
        .any(|entry| {
            (entry.target_type == "workspace" && is_member)
                || (entry.target_type == "member" && entry.target_id == user_id)
        })
}

pub(crate) async fn resolve_issue(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw: &str,
) -> Result<Issue, Response> {
    let workspace_id = context.member.workspace_id;
    let result = if let Ok(id) = Uuid::parse_str(raw) {
        issue_q::get_issue_in_workspace(&state.pool, id, workspace_id).await
    } else {
        let Some((prefix, number)) = raw.rsplit_once('-') else {
            return Err(error_response(StatusCode::NOT_FOUND, "issue not found"));
        };
        let expected_prefix = issue_prefix(state, workspace_id).await;
        let Ok(number) = number.parse::<i32>() else {
            return Err(error_response(StatusCode::NOT_FOUND, "issue not found"));
        };
        if !prefix.eq_ignore_ascii_case(&expected_prefix) {
            return Err(error_response(StatusCode::NOT_FOUND, "issue not found"));
        }
        issue_q::get_issue_by_number(&state.pool, workspace_id, number).await
    };
    match result {
        Ok(Some(issue)) => Ok(issue),
        Ok(None) => Err(error_response(StatusCode::NOT_FOUND, "issue not found")),
        Err(error) => {
            tracing::warn!(%error, issue = raw, "failed to load issue");
            Err(error_response(StatusCode::NOT_FOUND, "issue not found"))
        }
    }
}

async fn enrich_issue_list(
    state: &HandlerState,
    context: &WorkspaceContext,
    issues: Vec<Issue>,
) -> Vec<IssueResponse> {
    let prefix = issue_prefix(state, context.member.workspace_id).await;
    let ids = issues.iter().map(|issue| issue.id).collect::<Vec<_>>();
    let mut labels = labels_for_issues(state, context.member.workspace_id, &ids)
        .await
        .unwrap_or_default();
    let mut status_resolver =
        cordy_service::issue_status::Resolver::new(context.member.workspace_id);
    let mut responses = Vec::with_capacity(issues.len());
    for issue in issues {
        let category = status_resolver.effective(&state.pool, &issue.status).await;
        let mut response = IssueResponse::from_issue(&issue, &prefix);
        response.status_category = Some(category);
        response.labels = Some(labels.remove(&issue.id).unwrap_or_default());
        responses.push(response);
    }
    responses
}

async fn labels_for_issues(
    state: &HandlerState,
    workspace_id: Uuid,
    issue_ids: &[Uuid],
) -> anyhow::Result<HashMap<Uuid, Vec<LabelResponse>>> {
    if issue_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows =
        issue_label::list_labels_for_issues(&state.pool, issue_ids.to_vec(), workspace_id).await?;
    let mut labels = HashMap::<Uuid, Vec<LabelResponse>>::new();
    for row in rows {
        if let (
            Some(issue_id),
            Some(id),
            Some(label_workspace_id),
            Some(created_at),
            Some(updated_at),
        ) = (
            row.issue_id,
            row.id,
            row.workspace_id,
            row.created_at,
            row.updated_at,
        ) {
            labels.entry(issue_id).or_default().push(LabelResponse {
                id: id.to_string(),
                workspace_id: label_workspace_id.to_string(),
                resource_type: row.resource_type,
                name: row.name,
                description: row.description,
                color: row.color,
                usage_count: 0,
                created_at: timestamp(created_at),
                updated_at: timestamp(updated_at),
            });
        }
    }
    Ok(labels)
}

async fn issue_prefix(state: &HandlerState, workspace_id: Uuid) -> String {
    workspace::get_workspace(&state.pool, workspace_id)
        .await
        .ok()
        .flatten()
        .map(|workspace| {
            if workspace.issue_prefix.trim().is_empty() {
                legacy_issue_prefix(&workspace.name)
            } else {
                workspace.issue_prefix
            }
        })
        .unwrap_or_else(|| "ISSUE".to_string())
}

pub(crate) fn legacy_issue_prefix(name: &str) -> String {
    let letters = name
        .chars()
        .filter(|character| character.is_ascii_alphabetic())
        .take(3)
        .collect::<String>()
        .to_ascii_uppercase();
    if letters.is_empty() {
        "WS".to_string()
    } else {
        letters
    }
}

async fn invalid_status(state: &HandlerState, workspace_id: Uuid, status: &str) -> Response {
    let allowed = cordy_service::issue_status::active_keys(&state.pool, workspace_id)
        .await
        .unwrap_or_else(|_| {
            [
                "backlog",
                "todo",
                "in_progress",
                "in_review",
                "done",
                "blocked",
                "cancelled",
            ]
            .into_iter()
            .map(str::to_string)
            .collect()
        });
    error_response(
        StatusCode::BAD_REQUEST,
        &format!(
            "invalid status {:?}; valid values: {}",
            status,
            allowed.join(", ")
        ),
    )
}

async fn expand_status_categories(
    state: &HandlerState,
    workspace_id: Uuid,
    categories: &[String],
) -> Result<Vec<String>, Response> {
    if categories.is_empty() {
        return Ok(Vec::new());
    }
    let entries =
        cordy_db::queries::issue_status::list_issue_status_entries(&state.pool, workspace_id, true)
            .await
            .map_err(|error| {
                tracing::warn!(%error, "failed to expand issue status categories");
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to resolve status categories",
                )
            })?;
    let mut keys = Vec::new();
    for category in categories {
        if cordy_service::issue_status::is_built_in(category) {
            keys.push(category.clone());
        }
        keys.extend(
            entries
                .iter()
                .filter(|entry| entry.category == *category)
                .map(|entry| entry.key.clone()),
        );
    }
    keys.sort();
    keys.dedup();
    Ok(keys)
}

fn actor_filters(raw: Option<&str>, field: &str) -> Result<Vec<ActorFilter>, String> {
    comma_list(raw)
        .into_iter()
        .map(|value| {
            let (actor_type, id) = value
                .split_once(':')
                .ok_or_else(|| format!("invalid {field}"))?;
            if !matches!(actor_type, "member" | "agent" | "squad") || id.trim().is_empty() {
                return Err(format!("invalid {field}"));
            }
            Ok(ActorFilter {
                actor_type: actor_type.to_string(),
                actor_id: Uuid::parse_str(id.trim()).map_err(|_| format!("invalid {field}"))?,
            })
        })
        .collect()
}

fn json_object_filter(raw: Option<&str>, field: &str) -> Result<Option<Value>, String> {
    let value = json_filter(raw, field)?;
    if value.as_ref().is_some_and(|value| !value.is_object()) {
        return Err(format!("invalid {field}"));
    }
    Ok(value)
}

fn json_filter(raw: Option<&str>, field: &str) -> Result<Option<Value>, String> {
    raw.filter(|raw| !raw.trim().is_empty())
        .map(|raw| serde_json::from_str(raw).map_err(|_| format!("invalid {field}")))
        .transpose()
}

fn properties_filter(raw: Option<&str>) -> Result<Vec<Vec<PropertyAlternative>>, String> {
    let Some(raw) = raw.filter(|raw| !raw.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    let parsed = serde_json::from_str::<HashMap<String, Vec<String>>>(raw).map_err(|_| {
        "properties filter must be a JSON object of {definitionId: [values]}".to_string()
    })?;
    let mut groups = Vec::new();
    for (definition_id, values) in parsed {
        Uuid::parse_str(&definition_id).map_err(|_| {
            format!("properties filter key {definition_id:?} is not a definition id")
        })?;
        if values.is_empty() {
            continue;
        }
        let mut alternatives = Vec::new();
        for value in values {
            if value.is_empty() {
                return Err("properties filter values cannot be empty".to_string());
            }
            if value == "__none__" {
                alternatives.push(PropertyAlternative::Missing(definition_id.clone()));
                continue;
            }
            alternatives.push(PropertyAlternative::Contains(
                json!({ definition_id.clone(): value }),
            ));
            alternatives.push(PropertyAlternative::Contains(
                json!({ definition_id.clone(): [value.clone()] }),
            ));
            if matches!(value.as_str(), "true" | "false") {
                alternatives.push(PropertyAlternative::Contains(
                    json!({ definition_id.clone(): value == "true" }),
                ));
            }
        }
        groups.push(alternatives);
    }
    Ok(groups)
}

fn parse_date_filter(
    field: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
) -> Result<Option<DateFilter>, String> {
    if field.is_none() && start.is_none() && end.is_none() {
        return Ok(None);
    }
    let (Some(field), Some(start), Some(end)) = (field, start, end) else {
        return Err("date_field, date_start, and date_end are required together".to_string());
    };
    let column = match field.trim() {
        "created_at" => "created_at",
        "updated_at" => "updated_at",
        _ => return Err("invalid date_field".to_string()),
    };
    let start = chrono::DateTime::parse_from_rfc3339(start)
        .map_err(|_| "invalid date_start".to_string())?
        .with_timezone(&chrono::Utc);
    let end = chrono::DateTime::parse_from_rfc3339(end)
        .map_err(|_| "invalid date_end".to_string())?
        .with_timezone(&chrono::Utc);
    if start >= end {
        return Err("date_start must be before date_end".to_string());
    }
    Ok(Some(DateFilter { column, start, end }))
}

fn search_filter(raw: Option<&str>) -> (Vec<String>, Option<i32>) {
    let Some(query) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return (Vec::new(), None);
    };
    let terms = query
        .to_lowercase()
        .split_whitespace()
        .map(|term| format!("%{}%", escape_like(term)))
        .collect();
    let numeric_text = if let Some((prefix, number)) = query.split_once('-') {
        (prefix
            .chars()
            .all(|character| character.is_ascii_alphabetic())
            && !prefix.is_empty()
            && !number.contains('-'))
        .then_some(number)
    } else {
        Some(query)
    };
    let numeric = numeric_text
        .and_then(|number| number.parse::<i32>().ok())
        .filter(|number| *number > 0);
    (terms, numeric)
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn optional_uuid(raw: Option<&str>, field: &str) -> Result<Option<Uuid>, String> {
    raw.filter(|value| !value.is_empty())
        .map(|value| Uuid::parse_str(value).map_err(|_| format!("invalid {field}")))
        .transpose()
}

fn uuid_list(raw: Option<&str>, field: &str) -> Result<Vec<Uuid>, String> {
    raw.map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| Uuid::parse_str(value).map_err(|_| format!("invalid {field}")))
            .collect()
    })
    .unwrap_or_else(|| Ok(Vec::new()))
}

fn uuid_strings(raw: &[String], field: &str) -> Result<Vec<Uuid>, String> {
    raw.iter()
        .map(|value| Uuid::parse_str(value).map_err(|_| format!("invalid {field}")))
        .collect()
}

fn comma_list(raw: Option<&str>) -> Vec<String> {
    raw.map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

fn optional_date(raw: Option<&str>, field: &str) -> Result<Option<NaiveDate>, String> {
    raw.filter(|value| !value.is_empty())
        .map(|value| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|_| format!("invalid {field} format, expected YYYY-MM-DD"))
        })
        .transpose()
}

fn timestamp(value: chrono::DateTime<chrono::Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[derive(Debug, Serialize)]
struct IssueResponse {
    id: String,
    workspace_id: String,
    number: i32,
    identifier: String,
    title: String,
    description: Option<String>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_category: Option<String>,
    priority: String,
    assignee_type: Option<String>,
    assignee_id: Option<String>,
    creator_type: String,
    creator_id: String,
    parent_issue_id: Option<String>,
    project_id: Option<String>,
    position: f64,
    stage: Option<i32>,
    start_date: Option<String>,
    due_date: Option<String>,
    created_at: String,
    updated_at: String,
    revision: i64,
    last_activity_at: Option<String>,
    metadata: Value,
    properties: Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reactions: Vec<IssueReactionResponse>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<AttachmentResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<Vec<LabelResponse>>,
}

impl IssueResponse {
    fn from_issue(issue: &Issue, prefix: &str) -> Self {
        Self {
            id: issue.id.to_string(),
            workspace_id: issue.workspace_id.to_string(),
            number: issue.number,
            identifier: format!("{prefix}-{}", issue.number),
            title: issue.title.clone(),
            description: issue.description.clone(),
            status: issue.status.clone(),
            status_category: cordy_service::issue_status::is_built_in(&issue.status)
                .then(|| issue.status.clone()),
            priority: issue.priority.clone(),
            assignee_type: issue.assignee_type.clone(),
            assignee_id: issue.assignee_id.map(|id| id.to_string()),
            creator_type: issue.creator_type.clone(),
            creator_id: issue.creator_id.to_string(),
            parent_issue_id: issue.parent_issue_id.map(|id| id.to_string()),
            project_id: issue.project_id.map(|id| id.to_string()),
            position: issue.position,
            stage: issue.stage,
            start_date: issue
                .start_date
                .map(|date| date.format("%Y-%m-%d").to_string()),
            due_date: issue
                .due_date
                .map(|date| date.format("%Y-%m-%d").to_string()),
            created_at: timestamp(issue.created_at),
            updated_at: timestamp(issue.updated_at),
            revision: issue.revision,
            last_activity_at: issue.last_activity_at.map(timestamp),
            metadata: object_or_empty(issue.metadata.clone()),
            properties: object_or_empty(issue.properties.clone()),
            reactions: Vec::new(),
            attachments: Vec::new(),
            labels: None,
        }
    }
}

#[derive(Debug, Serialize)]
struct IssueReactionResponse {
    id: String,
    issue_id: String,
    actor_type: String,
    actor_id: String,
    emoji: String,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    issue_revision: Option<i64>,
}

impl From<&IssueReaction> for IssueReactionResponse {
    fn from(reaction: &IssueReaction) -> Self {
        Self {
            id: reaction.id.to_string(),
            issue_id: reaction.issue_id.to_string(),
            actor_type: reaction.actor_type.clone(),
            actor_id: reaction.actor_id.to_string(),
            emoji: reaction.emoji.clone(),
            created_at: timestamp(reaction.created_at),
            issue_revision: None,
        }
    }
}

impl IssueReactionResponse {
    fn from_added(reaction: &AddIssueReactionRow) -> Option<Self> {
        Some(Self {
            id: reaction.id?.to_string(),
            issue_id: reaction.issue_id?.to_string(),
            actor_type: reaction.actor_type.clone(),
            actor_id: reaction.actor_id?.to_string(),
            emoji: reaction.emoji.clone(),
            created_at: timestamp(reaction.created_at?),
            issue_revision: (reaction.issue_revision > 0).then_some(reaction.issue_revision),
        })
    }
}

#[derive(Debug, Serialize)]
struct SubscriberResponse {
    issue_id: String,
    user_type: String,
    user_id: String,
    reason: String,
    created_at: String,
}

impl From<&IssueSubscriber> for SubscriberResponse {
    fn from(subscriber: &IssueSubscriber) -> Self {
        Self {
            issue_id: subscriber.issue_id.to_string(),
            user_type: subscriber.user_type.clone(),
            user_id: subscriber.user_id.to_string(),
            reason: subscriber.reason.clone(),
            created_at: timestamp(subscriber.created_at),
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct AttachmentResponse {
    id: String,
    workspace_id: String,
    issue_id: Option<String>,
    comment_id: Option<String>,
    chat_session_id: Option<String>,
    chat_message_id: Option<String>,
    uploader_type: String,
    uploader_id: String,
    filename: String,
    url: String,
    download_url: String,
    markdown_url: String,
    content_type: String,
    size_bytes: i64,
    created_at: String,
}

impl From<&Attachment> for AttachmentResponse {
    fn from(attachment: &Attachment) -> Self {
        let stable_url = format!("/api/attachments/{}/download", attachment.id);
        Self {
            id: attachment.id.to_string(),
            workspace_id: attachment.workspace_id.to_string(),
            issue_id: attachment.issue_id.map(|id| id.to_string()),
            comment_id: attachment.comment_id.map(|id| id.to_string()),
            chat_session_id: attachment.chat_session_id.map(|id| id.to_string()),
            chat_message_id: attachment.chat_message_id.map(|id| id.to_string()),
            uploader_type: attachment.uploader_type.clone(),
            uploader_id: attachment.uploader_id.to_string(),
            filename: attachment.filename.clone(),
            url: attachment.url.clone(),
            download_url: stable_url.clone(),
            markdown_url: stable_url,
            content_type: attachment.content_type.clone(),
            size_bytes: attachment.size_bytes,
            created_at: timestamp(attachment.created_at),
        }
    }
}

fn object_or_empty(value: Value) -> Value {
    if value.is_object() {
        value
    } else {
        json!({})
    }
}

#[derive(Debug, Serialize)]
struct LabelResponse {
    id: String,
    workspace_id: String,
    resource_type: String,
    name: String,
    description: String,
    color: String,
    usage_count: i64,
    created_at: String,
    updated_at: String,
}

impl From<&IssueLabel> for LabelResponse {
    fn from(label: &IssueLabel) -> Self {
        Self {
            id: label.id.to_string(),
            workspace_id: label.workspace_id.to_string(),
            resource_type: label.resource_type.clone(),
            name: label.name.clone(),
            description: label.description.clone(),
            color: label.color.clone(),
            usage_count: 0,
            created_at: timestamp(label.created_at),
            updated_at: timestamp(label.updated_at),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn fixture_issue() -> Issue {
        let timestamp = Utc.with_ymd_and_hms(2026, 8, 23, 3, 30, 0).unwrap();
        Issue {
            acceptance_criteria: json!([]),
            assignee_id: None,
            assignee_type: None,
            context_refs: json!([]),
            created_at: timestamp,
            creator_id: Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f12").unwrap(),
            creator_type: "member".into(),
            description: None,
            due_date: None,
            first_executed_at: None,
            id: Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f11").unwrap(),
            last_activity_at: Some(timestamp),
            metadata: Value::Null,
            number: 14,
            origin_id: None,
            origin_type: None,
            parent_issue_id: None,
            position: -7.0,
            priority: "none".into(),
            project_id: None,
            properties: Value::Null,
            revision: 3,
            stage: Some(4),
            start_date: None,
            status: "in_progress".into(),
            title: "Port handlers".into(),
            updated_at: timestamp,
            workspace_id: Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f10").unwrap(),
        }
    }

    fn fixture_attachment(id: Uuid) -> Attachment {
        Attachment {
            chat_message_id: None,
            chat_session_id: None,
            comment_id: None,
            content_type: "image/png".into(),
            created_at: Utc.with_ymd_and_hms(2026, 8, 23, 3, 30, 0).unwrap(),
            filename: "diagram.png".into(),
            id,
            issue_id: Some(fixture_issue().id),
            size_bytes: 42,
            task_id: None,
            uploader_id: fixture_issue().creator_id,
            uploader_type: "member".into(),
            url: "/uploads/diagram.png".into(),
            workspace_id: fixture_issue().workspace_id,
        }
    }

    #[test]
    fn issue_response_matches_go_wire_shape() {
        let value =
            serde_json::to_value(IssueResponse::from_issue(&fixture_issue(), "CORD")).unwrap();
        assert_eq!(value["identifier"], "CORD-14");
        assert_eq!(value["status_category"], "in_progress");
        assert_eq!(value["created_at"], "2026-08-23T03:30:00Z");
        assert_eq!(value["metadata"], json!({}));
        assert_eq!(value["properties"], json!({}));
        assert!(value.get("labels").is_none());
    }

    #[test]
    fn pr_completion_event_matches_go_wire_shape() {
        let previous = fixture_issue();
        let mut updated = previous.clone();
        updated.status = "done".into();
        updated.revision += 1;
        let mut response = IssueResponse::from_issue(&updated, "CORD");
        response.status_category = Some("done".into());
        let issue = serde_json::to_value(&response).expect("issue response");

        assert_eq!(
            pr_completion_event_payload(&previous, response, "github_pr_merged"),
            json!({
                "issue": issue,
                "assignee_changed": false,
                "status_changed": true,
                "priority_changed": false,
                "project_changed": false,
                "prev_status": "in_progress",
                "creator_type": "member",
                "creator_id": "018f03a0-c4d2-7a37-ae4d-5aa45de12f12",
                "source": "github_pr_merged",
            })
        );
    }

    #[test]
    fn list_parameter_validation_rejects_malformed_ids() {
        assert!(optional_uuid(Some("not-a-uuid"), "assignee_id").is_err());
        assert!(uuid_list(Some("not-a-uuid"), "ids").is_err());
        assert!(uuid_list(Some(""), "ids").unwrap().is_empty());
    }

    #[test]
    fn children_by_parents_enforces_uuid_and_fanout_limits() {
        assert!(parse_parent_ids("").unwrap().is_empty());
        assert_eq!(
            parse_parent_ids("018f03a0-c4d2-7a37-ae4d-5aa45de12f11")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            parse_parent_ids("not-a-uuid").unwrap_err(),
            "invalid parent_ids"
        );
        assert_eq!(
            parse_parent_ids(&vec![Uuid::nil().to_string(); 201].join(",")).unwrap_err(),
            "too many parent_ids"
        );
    }

    #[test]
    fn date_parser_preserves_calendar_wire_format() {
        assert_eq!(
            optional_date(Some("2026-08-23"), "due_date").unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 23)
        );
        assert!(optional_date(Some("08/23/2026"), "due_date").is_err());
    }

    #[test]
    fn search_filter_matches_every_escaped_title_term_and_identifiers() {
        let (terms, number) = search_filter(Some("Fix 100%_safe"));
        assert_eq!(terms, vec!["%fix%", "%100\\%\\_safe%"]);
        assert_eq!(number, None);
        assert_eq!(search_filter(Some("CORD-42")).1, Some(42));
        assert_eq!(search_filter(Some("42")).1, Some(42));
        assert_eq!(search_filter(Some("CORD-extra-42")).1, None);
    }

    #[test]
    fn actor_and_property_filters_preserve_table_facet_semantics() {
        let id = "018f03a0-c4d2-7a37-ae4d-5aa45de12f11";
        let actors = actor_filters(Some(&format!("member:{id}")), "assignee_filters").unwrap();
        assert_eq!(actors.len(), 1);
        assert_eq!(actors[0].actor_type, "member");
        assert!(actor_filters(Some("unknown:value"), "assignee_filters").is_err());

        let groups =
            properties_filter(Some(&format!(r#"{{"{id}":["choice","__none__"]}}"#))).unwrap();
        assert_eq!(groups.len(), 1);
        assert!(groups[0].iter().any(
            |alternative| matches!(alternative, PropertyAlternative::Missing(value) if value == id)
        ));
    }

    #[test]
    fn property_value_decoder_preserves_missing_null_and_go_first_value_behavior() {
        assert_eq!(decode_property_value(br#"{"other":1}"#), Ok(None));
        assert_eq!(
            decode_property_value(br#"{"value":null}"#),
            Ok(Some(Value::Null))
        );
        assert_eq!(
            decode_property_value(br#"{"value":"high","unknown":true} trailing"#),
            Ok(Some(json!("high")))
        );
        assert!(decode_property_value(b"[]").is_err());
    }

    #[test]
    fn update_parser_distinguishes_missing_null_and_value() {
        let fields = update_object(br#"{"assignee_id":null,"stage":4}"#).unwrap();
        assert!(matches!(
            update_field::<String>(&fields, "assignee_id").unwrap(),
            UpdateField::Null
        ));
        assert!(matches!(
            update_field::<i32>(&fields, "stage").unwrap(),
            UpdateField::Value(4)
        ));
        assert!(matches!(
            update_field::<String>(&fields, "project_id").unwrap(),
            UpdateField::Missing
        ));
    }

    #[test]
    fn legacy_prefix_fallback_matches_frozen_go_rule() {
        assert_eq!(legacy_issue_prefix("Frontend Team"), "FRO");
        assert_eq!(legacy_issue_prefix("前端团队"), "WS");
    }

    #[test]
    fn position_only_update_does_not_count_as_activity() {
        let issue = fixture_issue();
        let mut moved = issue.clone();
        moved.position = issue.position + 1.0;
        assert!(issue_mutable_fields_differ(&issue, &moved));
        assert!(!issue_activity_fields_differ(&issue, &moved));
    }

    #[test]
    fn status_update_counts_as_activity() {
        let issue = fixture_issue();
        let mut updated = issue.clone();
        updated.status = "in_review".into();
        assert!(issue_mutable_fields_differ(&issue, &updated));
        assert!(issue_activity_fields_differ(&issue, &updated));
    }

    #[test]
    fn description_merge_preserves_only_late_channel_media() {
        let id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f13").unwrap();
        let path = format!("/api/attachments/{id}/download");
        let block = format!("![]({path})\n\n{}", media_marker(id));
        let current = append_markdown("Original", &block);
        let attachments = vec![fixture_attachment(id)];

        let merged = merge_channel_media_description(
            &current,
            "Original with local edit",
            Some("Original"),
            &attachments,
        );
        assert!(merged.contains("Original with local edit"));
        assert!(merged.contains(&path));
        assert!(merged.contains(&media_marker(id)));

        let deleted = merge_channel_media_description(
            &current,
            "Original with local edit",
            Some(&current),
            &attachments,
        );
        assert!(!deleted.contains(&path));
    }

    #[test]
    fn locked_snapshot_refreshes_fields_the_request_did_not_touch() {
        let mut next = fixture_issue();
        let mut current = next.clone();
        current.priority = "urgent".into();
        current.title = "concurrent title".into();
        let fields = update_object(br#"{"title":"local title"}"#).unwrap();
        next.title = "local title".into();
        refresh_untouched_fields(&mut next, &current, &fields);
        assert_eq!(next.title, "local title");
        assert_eq!(next.priority, "urgent");
    }

    #[test]
    fn reaction_and_subscriber_responses_match_go_wire_contracts() {
        let id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f11").unwrap();
        let actor_id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f12").unwrap();
        let created_at = chrono::DateTime::parse_from_rfc3339("2026-08-23T12:34:56.789Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let reaction = IssueReactionResponse::from_added(&AddIssueReactionRow {
            id: Some(id),
            issue_id: Some(id),
            workspace_id: Some(id),
            actor_type: "member".into(),
            actor_id: Some(actor_id),
            emoji: "👍".into(),
            created_at: Some(created_at),
            issue_revision: 7,
        })
        .unwrap();
        let reaction = serde_json::to_value(reaction).unwrap();
        assert_eq!(reaction["created_at"], "2026-08-23T12:34:56Z");
        assert_eq!(reaction["issue_revision"], 7);
        assert!(reaction.get("workspace_id").is_none());

        let subscriber = serde_json::to_value(SubscriberResponse::from(&IssueSubscriber {
            created_at,
            issue_id: id,
            opt_out_scope: Some("subtree".into()),
            reason: "manual".into(),
            unsubscribed_at: Some(created_at),
            user_id: actor_id,
            user_type: "member".into(),
        }))
        .unwrap();
        assert_eq!(subscriber["created_at"], "2026-08-23T12:34:56Z");
        assert!(subscriber.get("opt_out_scope").is_none());
        assert!(subscriber.get("unsubscribed_at").is_none());
    }

    #[test]
    fn actor_headers_require_server_stamped_task_token_source() {
        let user_id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f11").unwrap();
        let agent_id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f12").unwrap();
        let workspace_id = Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f13").unwrap();
        let context = WorkspaceContext {
            workspace_id: workspace_id.to_string(),
            member: cordy_db::models::Member {
                created_at: chrono::Utc::now(),
                id: Uuid::nil(),
                role: "member".into(),
                user_id,
                workspace_id,
            },
        };
        let mut headers = HeaderMap::new();
        headers.insert("x-agent-id", agent_id.to_string().parse().unwrap());
        assert_eq!(request_actor(&headers, &context), ("member", user_id));

        headers.insert("x-actor-source", "task_token".parse().unwrap());
        assert_eq!(request_actor(&headers, &context), ("agent", agent_id));
    }

    #[test]
    fn issue_usage_response_matches_go_wire_contract() {
        let response = IssueUsageResponse::from(task_usage::GetIssueUsageSummaryRow {
            total_input_tokens: 1,
            total_output_tokens: 2,
            total_cache_read_tokens: 3,
            total_cache_write_tokens: 4,
            total_cost_usd_ticks: 5,
            uncosted_input_tokens: 6,
            uncosted_output_tokens: 7,
            uncosted_cache_read_tokens: 8,
            uncosted_cache_write_tokens: 9,
            task_count: 10,
        });
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "total_input_tokens": 1,
                "total_output_tokens": 2,
                "total_cache_read_tokens": 3,
                "total_cache_write_tokens": 4,
                "cost_usd_ticks": 5,
                "uncosted_input_tokens": 6,
                "uncosted_output_tokens": 7,
                "uncosted_cache_read_tokens": 8,
                "uncosted_cache_write_tokens": 9,
                "task_count": 10,
            })
        );
    }

    #[test]
    fn attachment_list_response_matches_go_stable_url_contract() {
        let attachment =
            fixture_attachment(Uuid::parse_str("018f03a0-c4d2-7a37-ae4d-5aa45de12f13").unwrap());
        let response = serde_json::to_value(AttachmentResponse::from(&attachment)).unwrap();
        let stable_url = format!("/api/attachments/{}/download", attachment.id);
        assert_eq!(response["download_url"], stable_url);
        assert_eq!(response["markdown_url"], stable_url);
        assert_eq!(response["created_at"], "2026-08-23T03:30:00Z");
        assert_eq!(response["issue_id"], fixture_issue().id.to_string());
        assert!(response.get("task_id").is_none());
    }
}
