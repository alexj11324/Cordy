//! Workspace-scoped channel chat for people and agents.
//!
//! Channels deliberately reuse the workspace membership boundary instead of
//! introducing a second membership system in v1. Human authors come from the
//! authenticated workspace context; agent authors are resolved by the shared
//! task-token actor boundary before a message is written.

use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use patchbay_db::models::{WorkspaceChannel, WorkspaceChannelMessage};
use patchbay_db::queries::workspace_channel as channel_q;
use patchbay_middleware::workspace::WorkspaceContext;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const CHANNEL_NAME_MAX_CHARS: usize = 80;
const CHANNEL_DESCRIPTION_MAX_CHARS: usize = 240;
const MESSAGE_MAX_CHARS: usize = 20_000;

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
    Json(request): Json<CreateChannelRequest>,
) -> Response {
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
) -> Response {
    let channel = match load_channel(&state, &context, &channel_id).await {
        Ok(channel) => channel,
        Err(response) => return response,
    };
    match channel_q::list_messages(&state.pool, channel.id, context.member.workspace_id).await {
        Ok(messages) => Json(messages.iter().map(message_json).collect::<Vec<_>>()).into_response(),
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
}
