//! User-authenticated comment endpoints.

use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use cordy_db::models::{Comment, CommentReaction};
use cordy_db::queries::{attachment, comment, reaction};
use cordy_middleware::workspace::WorkspaceContext;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new().route(
        "/api/comments/{comment_id}/resolve",
        post(resolve).delete(unresolve),
    )
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "workspace not found"))
}

async fn load_comment(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_id: &str,
) -> Result<Comment, Response> {
    let comment_id = Uuid::parse_str(raw_id.trim())
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid comment id"))?;
    let workspace_id = workspace_id(context)?;
    match comment::get_comment_in_workspace(&state.pool, comment_id, workspace_id).await {
        Ok(Some(comment)) => Ok(comment),
        Ok(None) => Err(error_response(StatusCode::NOT_FOUND, "comment not found")),
        Err(error) => {
            tracing::warn!(%error, %comment_id, "failed to load comment");
            Err(error_response(StatusCode::NOT_FOUND, "comment not found"))
        }
    }
}

fn reaction_json(reaction: &CommentReaction) -> Value {
    json!({
        "id": reaction.id,
        "comment_id": reaction.comment_id,
        "actor_type": reaction.actor_type,
        "actor_id": reaction.actor_id,
        "emoji": reaction.emoji,
        "created_at": crate::timefmt::rfc3339(reaction.created_at),
    })
}

async fn comment_json(state: &HandlerState, comment: &Comment) -> Value {
    let reactions = reaction::list_reactions_by_comment_i_ds(&state.pool, vec![comment.id])
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, comment_id = %comment.id, "failed to load comment reactions");
            Vec::new()
        });
    let attachments = attachment::list_attachments_by_comment_i_ds(
        &state.pool,
        vec![comment.id],
        comment.workspace_id,
    )
    .await
    .unwrap_or_else(|error| {
        tracing::warn!(%error, comment_id = %comment.id, "failed to load comment attachments");
        Vec::new()
    });

    comment_json_with_related(
        comment,
        Value::Array(reactions.iter().map(reaction_json).collect()),
        serde_json::to_value(
            attachments
                .iter()
                .map(crate::issue::AttachmentResponse::from)
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| json!([])),
    )
}

fn comment_json_with_related(comment: &Comment, reactions: Value, attachments: Value) -> Value {
    let mut response = serde_json::Map::new();
    response.insert("id".into(), json!(comment.id));
    response.insert("issue_id".into(), json!(comment.issue_id));
    response.insert("author_type".into(), json!(comment.author_type));
    response.insert("author_id".into(), json!(comment.author_id));
    response.insert("content".into(), json!(comment.content));
    response.insert("type".into(), json!(comment.type_));
    response.insert("parent_id".into(), json!(comment.parent_id));
    response.insert(
        "created_at".into(),
        json!(crate::timefmt::rfc3339(comment.created_at)),
    );
    response.insert(
        "updated_at".into(),
        json!(crate::timefmt::rfc3339(comment.updated_at)),
    );
    response.insert("revision".into(), json!(comment.revision));
    response.insert(
        "resolved_at".into(),
        json!(comment.resolved_at.map(crate::timefmt::rfc3339)),
    );
    response.insert("resolved_by_type".into(), json!(comment.resolved_by_type));
    response.insert("resolved_by_id".into(), json!(comment.resolved_by_id));
    if let Some(source_task_id) = comment.source_task_id {
        response.insert("source_task_id".into(), json!(source_task_id));
    }
    if let Some(quick_action_id) = comment.quick_action_id {
        response.insert("quick_action_id".into(), json!(quick_action_id));
    }
    response.insert("reactions".into(), reactions);
    response.insert("attachments".into(), attachments);
    Value::Object(response)
}

fn publish(
    state: &HandlerState,
    context: &WorkspaceContext,
    event_type: &str,
    actor_type: &str,
    actor_id: Uuid,
    comment: Value,
) {
    state.bus.publish(&cordy_events::Event {
        event_type: event_type.into(),
        workspace_id: context.workspace_id.clone(),
        actor_type: actor_type.into(),
        actor_id: actor_id.to_string(),
        payload: json!({ "comment": comment }),
        ..Default::default()
    });
}

async fn resolve(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(comment_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let current = match load_comment(&state, &context, &comment_id).await {
        Ok(comment) => comment,
        Err(response) => return response,
    };
    let was_resolved = current.resolved_at.is_some();
    let (actor_type, actor_id, _) = crate::issue::mutation_actor(&state, &context, &headers).await;
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, comment_id = %current.id, "failed to begin comment resolution");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to resolve comment",
            );
        }
    };
    let cleared = match comment::clear_other_thread_resolutions(
        &mut *transaction,
        current.id,
        current.issue_id,
        current.workspace_id,
    )
    .await
    {
        Ok(cleared) => cleared,
        Err(error) => {
            tracing::warn!(%error, comment_id = %current.id, "failed to clear thread resolutions");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to resolve comment",
            );
        }
    };
    let updated =
        match comment::resolve_comment(&mut *transaction, current.id, Some(&actor_type), actor_id)
            .await
        {
            Ok(Some(updated)) => updated,
            Ok(None) | Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to resolve comment",
                )
            }
        };
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, comment_id = %current.id, "failed to commit comment resolution");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to resolve comment",
        );
    }

    for sibling in cleared {
        let response = comment_json(&state, &sibling).await;
        publish(
            &state,
            &context,
            cordy_protocol::EVENT_COMMENT_UNRESOLVED,
            &actor_type,
            actor_id,
            response,
        );
    }
    let response = comment_json(&state, &updated).await;
    if !was_resolved {
        publish(
            &state,
            &context,
            cordy_protocol::EVENT_COMMENT_RESOLVED,
            &actor_type,
            actor_id,
            response.clone(),
        );
    }
    Json(response).into_response()
}

async fn unresolve(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(comment_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let current = match load_comment(&state, &context, &comment_id).await {
        Ok(comment) => comment,
        Err(response) => return response,
    };
    let was_resolved = current.resolved_at.is_some();
    let (actor_type, actor_id, _) = crate::issue::mutation_actor(&state, &context, &headers).await;
    let updated = match comment::unresolve_comment(&state.pool, current.id).await {
        Ok(Some(updated)) => updated,
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to unresolve comment",
            )
        }
    };
    let response = comment_json(&state, &updated).await;
    if was_resolved {
        publish(
            &state,
            &context,
            cordy_protocol::EVENT_COMMENT_UNRESOLVED,
            &actor_type,
            actor_id,
            response.clone(),
        );
    }
    Json(response).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_response_preserves_nullable_and_omitempty_fields() {
        let raw = Comment {
            author_id: Uuid::parse_str("018f946a-1234-7890-abcd-1234567890ab").unwrap(),
            author_type: "member".into(),
            content: "done".into(),
            created_at: "2026-08-23T12:34:56Z".parse().unwrap(),
            id: Uuid::parse_str("018f946a-2234-7890-abcd-1234567890ab").unwrap(),
            issue_id: Uuid::parse_str("018f946a-3234-7890-abcd-1234567890ab").unwrap(),
            parent_id: None,
            quick_action_id: None,
            resolved_at: None,
            resolved_by_id: None,
            resolved_by_type: None,
            revision: 2,
            source_task_id: None,
            type_: "comment".into(),
            updated_at: "2026-08-23T12:35:00Z".parse().unwrap(),
            via_plugin_id: None,
            workspace_id: Uuid::parse_str("018f946a-4234-7890-abcd-1234567890ab").unwrap(),
        };
        let response = comment_json_with_related(&raw, json!([]), json!([]));
        assert_eq!(response["parent_id"], Value::Null);
        assert_eq!(response["resolved_at"], Value::Null);
        assert_eq!(response["resolved_by_type"], Value::Null);
        assert_eq!(response["resolved_by_id"], Value::Null);
        assert_eq!(response["reactions"], json!([]));
        assert_eq!(response["attachments"], json!([]));
        assert_eq!(response["created_at"], json!("2026-08-23T12:34:56Z"));
        assert!(response.get("source_task_id").is_none());
        assert!(response.get("quick_action_id").is_none());
        assert!(response.get("issue_revision").is_none());
    }
}
