//! Workspace-scoped channel chat for people and agents.
//!
//! Channels deliberately reuse the workspace membership boundary instead of
//! introducing a second membership system in v1. Human authors come from the
//! authenticated workspace context; agent authors are resolved by the shared
//! task-token actor boundary before a message is written.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use patchbay_db::models::{WorkspaceChannel, WorkspaceChannelMessage};
use patchbay_db::queries::{agent, chat};
use patchbay_db::queries::workspace_channel as channel_q;
use patchbay_middleware::workspace::WorkspaceContext;
use patchbay_service::task_service::WorkspaceChannelDispatch;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const CHANNEL_NAME_MAX_CHARS: usize = 80;
const CHANNEL_DESCRIPTION_MAX_CHARS: usize = 240;
const MESSAGE_MAX_CHARS: usize = 20_000;
const AGENT_MENTION_PREFIX: &str = "mention://agent/";

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/channels", get(list_channels).post(create_channel))
        .route("/api/channels/{channel_id}", get(get_channel))
        .route(
            "/api/channels/{channel_id}/messages",
            get(list_messages).post(create_message),
        )
}

#[derive(Default, Deserialize)]
struct CreateChannelRequest {
    name: String,
    #[serde(default)]
    description: String,
}

#[derive(Default, Deserialize)]
struct CreateMessageRequest {
    content: String,
    parent_id: Option<String>,
    quoted_message_id: Option<String>,
}

#[derive(Default, Deserialize)]
struct ChannelMessagesQuery {
    limit: Option<usize>,
    before_created_at: Option<String>,
    before_id: Option<String>,
}

fn channel_json(channel: &WorkspaceChannel) -> Value {
    json!({
        "id": channel.id,
        "workspace_id": channel.workspace_id,
        "name": channel.name,
        "slug": channel.slug,
        "description": channel.description,
        "created_by": channel.created_by,
        "archived_at": channel.archived_at.map(crate::timefmt::rfc3339),
        "created_at": crate::timefmt::rfc3339(channel.created_at),
        "updated_at": crate::timefmt::rfc3339(channel.updated_at),
    })
}

fn quoted_message_json(message: &patchbay_db::models::WorkspaceChannelQuotedMessage) -> Value {
    json!({
        "id": message.id,
        "author_type": message.author_type,
        "author_id": message.author_id,
        "author_name": message.author_name,
        "content": message.content,
    })
}

fn message_json(message: &WorkspaceChannelMessage) -> Value {
    json!({
        "id": message.id,
        "workspace_id": message.workspace_id,
        "channel_id": message.channel_id,
        "author_type": message.author_type,
        "author_id": message.author_id,
        "author_name": message.author_name,
        "author_avatar_url": message.author_avatar_url,
        "author_status": message.author_status,
        "content": message.content,
        "parent_message": message.parent_message.as_ref().map(quoted_message_json),
        "parent_id": message.parent_id,
        "quoted_message_id": message.quoted_message_id,
        "quoted_message": message.quoted_message.as_ref().map(quoted_message_json),
        "created_at": crate::timefmt::rfc3339(message.created_at),
        "updated_at": crate::timefmt::rfc3339(message.updated_at),
    })
}

fn parse_uuid(raw: &str, field: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(raw.trim())
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, &format!("invalid {field}")))
}

fn optional_uuid(raw: Option<&str>, field: &str) -> Result<Option<Uuid>, Response> {
    raw.filter(|value| !value.trim().is_empty())
        .map(|value| parse_uuid(value, field))
        .transpose()
}

fn clean_text(value: &str) -> String {
    value.replace('\0', "").trim().to_string()
}

fn validate_length(value: &str, max_chars: usize, field: &str) -> Result<(), Response> {
    if value.chars().count() > max_chars {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            &format!("{field} is too long"),
        ));
    }
    Ok(())
}

fn mentioned_agent_ids(content: &str) -> Vec<Uuid> {
    let mut ids = Vec::new();
    let mut remaining = content;
    while let Some(offset) = remaining.find(AGENT_MENTION_PREFIX) {
        let candidate = &remaining[offset + AGENT_MENTION_PREFIX.len()..];
        let end = candidate
            .find(|character: char| !character.is_ascii_hexdigit() && character != '-')
            .unwrap_or(candidate.len());
        if let Ok(id) = Uuid::parse_str(&candidate[..end]) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        remaining = &candidate[end..];
        if remaining.is_empty() {
            break;
        }
    }
    ids
}

fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut pending_separator = false;
    for character in value.chars() {
        if character.is_alphanumeric() {
            if pending_separator && !slug.is_empty() {
                slug.push('-');
            }
            pending_separator = false;
            slug.extend(character.to_lowercase());
        } else if !slug.is_empty() {
            pending_separator = true;
        }
        if slug.chars().count() >= 64 {
            break;
        }
    }
    slug.trim_end_matches('-').to_string()
}

fn unique_violation(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23505")
}

fn internal(message: &'static str, error: anyhow::Error) -> Response {
    tracing::error!(%error, "{message}");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, message)
}

async fn load_channel(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_id: &str,
) -> Result<WorkspaceChannel, Response> {
    let channel_id = parse_uuid(raw_id, "channel id")?;
    channel_q::get_channel(&state.pool, channel_id, context.member.workspace_id)
        .await
        .map_err(|error| internal("failed to load channel", error))?
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "channel not found"))
}

fn publish_channel_event(
    state: &HandlerState,
    context: &WorkspaceContext,
    event_type: &str,
    actor_type: &str,
    actor_id: Uuid,
    task_id: Option<Uuid>,
    payload: Value,
) {
    state.bus.publish(&patchbay_events::Event {
        event_type: event_type.to_string(),
        workspace_id: context.workspace_id.clone(),
        actor_type: actor_type.to_string(),
        actor_id: actor_id.to_string(),
        payload,
        task_id: task_id.map(|id| id.to_string()).unwrap_or_default(),
        ..Default::default()
    });
}

async fn dispatch_agent_mentions(
    state: &HandlerState,
    context: &WorkspaceContext,
    channel: &WorkspaceChannel,
    source_message_id: Uuid,
    content: &str,
) {
    for agent_id in mentioned_agent_ids(content) {
        let target = match agent::get_agent_in_workspace(
            &state.pool,
            agent_id,
            context.member.workspace_id,
        )
        .await
        {
            Ok(Some(target)) => target,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(
                    %error,
                    agent_id = %agent_id,
                    channel_id = %channel.id,
                    "failed to load mentioned channel agent"
                );
                continue;
            }
        };
        if target.archived_at.is_some() || target.runtime_id.is_none() {
            continue;
        }
        if !crate::chat_api::can_invoke_agent(
            state,
            &target,
            "member",
            Some(context.member.user_id),
            context.member.workspace_id,
        )
        .await
        {
            tracing::info!(
                agent_id = %agent_id,
                channel_id = %channel.id,
                "skipping channel agent mention without invocation permission"
            );
            continue;
        }
        let session = match chat::create_chat_session(
            &state.pool,
            context.member.workspace_id,
            target.id,
            context.member.user_id,
            &format!("Channel #{}", channel.name),
            false,
            Uuid::nil(),
            Uuid::now_v7(),
        )
        .await
        {
            Ok(Some(session)) => session,
            Ok(None) => {
                tracing::warn!(
                    agent_id = %agent_id,
                    channel_id = %channel.id,
                    "mentioned channel agent session was not created"
                );
                continue;
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    agent_id = %agent_id,
                    channel_id = %channel.id,
                    "failed to create mentioned channel agent session"
                );
                continue;
            }
        };
        let prompt = format!(
            "You are participating in the shared workspace channel #{} with people and other agents. You were mentioned in the channel message below. Reply with a concise, useful message for that channel.\n\n{}",
            channel.name, content
        );
        let dispatch = WorkspaceChannelDispatch {
            workspace_id: context.member.workspace_id,
            channel_id: channel.id,
            source_message_id,
        };
        match state
            .tasks
            .send_direct_chat_message(
                &session,
                &target,
                Some(context.member.user_id),
                &prompt,
                vec![],
                "member",
                Some(context.member.user_id),
                Some(dispatch),
            )
            .await
        {
            Ok(result) => {
                tracing::info!(
                    agent_id = %agent_id,
                    channel_id = %channel.id,
                    task_id = ?result.task.as_ref().map(|task| task.id),
                    "dispatched workspace channel agent mention"
                );
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    agent_id = %agent_id,
                    channel_id = %channel.id,
                    "workspace channel agent mention was not dispatched"
                );
            }
        }
    }
}

async fn list_channels(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    match channel_q::list_channels(&state.pool, context.member.workspace_id).await {
        Ok(channels) => Json(channels.iter().map(channel_json).collect::<Vec<_>>()).into_response(),
        Err(error) => internal("failed to list channels", error),
    }
}

async fn create_channel(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Json(request): Json<CreateChannelRequest>,
) -> Response {
    let (_, _, task_id) = crate::issue::mutation_actor(&state, &context, &headers).await;
    if task_id.is_some() {
        return error_response(
            StatusCode::FORBIDDEN,
            "agents cannot create workspace channels",
        );
    }
    let name = clean_text(&request.name);
    if name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "name is required");
    }
    if let Err(response) = validate_length(&name, CHANNEL_NAME_MAX_CHARS, "name") {
        return response;
    }
    let description = clean_text(&request.description);
    if let Err(response) =
        validate_length(&description, CHANNEL_DESCRIPTION_MAX_CHARS, "description")
    {
        return response;
    }
    let slug = slugify(&name);
    if slug.is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "name must contain at least one letter or number",
        );
    }
    let channel = match channel_q::create_channel(
        &state.pool,
        Uuid::now_v7(),
        context.member.workspace_id,
        &name,
        &slug,
        &description,
        context.member.user_id,
    )
    .await
    {
        Ok(Some(channel)) => channel,
        Ok(None) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create channel",
            )
        }
        Err(error) if unique_violation(&error) => {
            return error_response(
                StatusCode::CONFLICT,
                "a channel with that name already exists",
            )
        }
        Err(error) => return internal("failed to create channel", error),
    };
    let response = channel_json(&channel);
    publish_channel_event(
        &state,
        &context,
        patchbay_protocol::events::EVENT_CHANNEL_CREATED,
        "member",
        context.member.user_id,
        None,
        json!({ "channel": &response }),
    );
    (StatusCode::CREATED, Json(response)).into_response()
}

async fn get_channel(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(channel_id): Path<String>,
) -> Response {
    match load_channel(&state, &context, &channel_id).await {
        Ok(channel) => Json(channel_json(&channel)).into_response(),
        Err(response) => response,
    }
}

async fn list_messages(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(channel_id): Path<String>,
    Query(query): Query<ChannelMessagesQuery>,
) -> Response {
    let channel = match load_channel(&state, &context, &channel_id).await {
        Ok(channel) => channel,
        Err(response) => return response,
    };
    let limit = query.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return error_response(StatusCode::BAD_REQUEST, "invalid limit");
    }
    let cursor = match (&query.before_created_at, &query.before_id) {
        (None, None) => (None, Uuid::nil()),
        (Some(created_at), Some(id)) => {
            let created_at = match DateTime::parse_from_rfc3339(created_at) {
                Ok(created_at) => created_at.with_timezone(&Utc),
                Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid cursor"),
            };
            let id = match parse_uuid(id, "cursor") {
                Ok(id) => id,
                Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid cursor"),
            };
            (Some(created_at), id)
        }
        _ => return error_response(StatusCode::BAD_REQUEST, "invalid cursor"),
    };
    match channel_q::list_messages(
        &state.pool,
        channel.id,
        context.member.workspace_id,
        (limit + 1) as i32,
        cursor.0,
        cursor.1,
    )
    .await
    {
        Ok(mut messages) => {
            let has_more = messages.len() > limit;
            messages.truncate(limit);
            let next_cursor = if has_more {
                messages.last().map(|message| {
                    json!({
                        "created_at": patchbay_util::rfc3339_nano(message.created_at),
                        "id": message.id,
                    })
                })
            } else {
                None
            };
            messages.reverse();
            Json(json!({
                "messages": messages.iter().map(message_json).collect::<Vec<_>>(),
                "limit": limit,
                "has_more": has_more,
                "next_cursor": next_cursor,
            }))
            .into_response()
        }
        Err(error) => internal("failed to list channel messages", error),
    }
}

async fn create_message(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(channel_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CreateMessageRequest>,
) -> Response {
    let channel = match load_channel(&state, &context, &channel_id).await {
        Ok(channel) => channel,
        Err(response) => return response,
    };
    let content = clean_text(&request.content);
    if content.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "content is required");
    }
    if let Err(response) = validate_length(&content, MESSAGE_MAX_CHARS, "content") {
        return response;
    }
    let parent_id = match optional_uuid(request.parent_id.as_deref(), "parent_id") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let quoted_message_id =
        match optional_uuid(request.quoted_message_id.as_deref(), "quoted_message_id") {
            Ok(id) => id,
            Err(response) => return response,
        };
    for (message_id, field) in [
        (parent_id, "parent message"),
        (quoted_message_id, "quoted message"),
    ] {
        if let Some(message_id) = message_id {
            match channel_q::get_message(
                &state.pool,
                message_id,
                channel.id,
                context.member.workspace_id,
            )
            .await
            {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("{field} not found in this channel"),
                    )
                }
                Err(error) => return internal("failed to validate message reference", error),
            }
        }
    }
    let (author_type, author_id, task_id) =
        crate::issue::mutation_actor(&state, &context, &headers).await;
    let message_id = Uuid::now_v7();
    match channel_q::create_message(
        &state.pool,
        message_id,
        context.member.workspace_id,
        channel.id,
        &author_type,
        author_id,
        &content,
        parent_id,
        quoted_message_id,
    )
    .await
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create channel message",
            )
        }
        Err(error) => return internal("failed to create channel message", error),
    }
    let message = match channel_q::get_message(
        &state.pool,
        message_id,
        channel.id,
        context.member.workspace_id,
    )
    .await
    {
        Ok(Some(message)) => message,
        Ok(None) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "created channel message could not be loaded",
            )
        }
        Err(error) => return internal("failed to load created channel message", error),
    };
    let response = message_json(&message);
    publish_channel_event(
        &state,
        &context,
        patchbay_protocol::events::EVENT_CHANNEL_MESSAGE_CREATED,
        &author_type,
        author_id,
        task_id,
        json!({ "channel_id": channel.id, "message": &response }),
    );
    dispatch_agent_mentions(&state, &context, &channel, message.id, &content).await;
    (StatusCode::CREATED, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_keeps_unicode_words_and_collapses_separators() {
        assert_eq!(slugify("Team / 讨论 2026"), "team-讨论-2026");
        assert_eq!(slugify("  #Launch!!!"), "launch");
    }

    #[test]
    fn route_set_is_complete() {
        let _ = router();
    }

    #[test]
    fn mentioned_agent_ids_deduplicates_canonical_mentions() {
        let id = Uuid::now_v7();
        let content = format!("[A](mention://agent/{id}) mention://agent/{id} mention://member/{id}");
        assert_eq!(mentioned_agent_ids(&content), vec![id]);
    }
}
