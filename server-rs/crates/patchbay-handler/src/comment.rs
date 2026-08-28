//! User-authenticated comment endpoints.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use patchbay_db::models::{Comment, CommentReaction};
use patchbay_db::queries::{activity, agent, attachment, comment, issue as issue_q, reaction};
use patchbay_middleware::workspace::WorkspaceContext;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::comment_trigger::{
    compute_comment_agent_triggers, enqueue_comment_triggers, load_parent_comment, preview_agents,
    preview_blocked, retrigger_cancelled_task_survivors, trigger_actor, CommentTriggerInput,
};
use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route(
            "/api/issues/{id}/comments",
            get(crate::comment_list::list_comments).post(create),
        )
        .route(
            "/api/issues/{id}/comments/trigger-preview",
            post(preview_triggers),
        )
        .route(
            "/api/comments/{comment_id}",
            axum::routing::put(update).delete(delete),
        )
        .route(
            "/api/comments/{comment_id}/resolve",
            post(resolve).delete(unresolve),
        )
        .route(
            "/api/comments/{comment_id}/reactions",
            post(add_reaction).delete(remove_reaction),
        )
}

#[derive(Debug, Deserialize)]
struct CommentWriteRequest {
    content: String,
    #[serde(default, rename = "type")]
    type_: String,
    parent_id: Option<String>,
    expected_revision: Option<i64>,
    content_base: Option<String>,
    attachment_ids: Option<Vec<String>>,
    editing_comment_id: Option<String>,
    #[serde(default)]
    suppress_agent_ids: Vec<String>,
}

fn clean_content(value: &str) -> String {
    value
        .chars()
        .filter(|character| *character != '\0')
        .collect()
}

fn uuid_list(values: &[String], field: &str) -> Result<Vec<Uuid>, Response> {
    values
        .iter()
        .map(|raw| {
            Uuid::parse_str(raw)
                .map_err(|_| error_response(StatusCode::BAD_REQUEST, &format!("invalid {field}")))
        })
        .collect()
}

async fn preview_triggers(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(issue_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CommentWriteRequest>,
) -> Response {
    let issue = match crate::issue::resolve_issue(&state, &context, &issue_id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let content = clean_content(&request.content);
    if content.is_empty() {
        return Json(json!({ "agents": [], "blocked": [] })).into_response();
    }
    let mut exclude_trigger_comment_id = None;
    let mut parent_id = match request
        .parent_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
    {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid parent_id"),
    };
    if let Some(raw) = request.editing_comment_id.as_deref() {
        let Ok(id) = Uuid::parse_str(raw) else {
            return error_response(StatusCode::BAD_REQUEST, "invalid editing_comment_id");
        };
        let Ok(Some(editing)) =
            comment::get_comment_in_workspace(&state.pool, id, issue.workspace_id).await
        else {
            return error_response(StatusCode::BAD_REQUEST, "invalid editing comment");
        };
        if editing.issue_id != issue.id {
            return error_response(StatusCode::BAD_REQUEST, "invalid editing comment");
        }
        if let Some(requested_parent) = parent_id {
            if editing.parent_id != Some(requested_parent) {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "parent_id does not match editing comment",
                );
            }
        } else {
            parent_id = editing.parent_id;
        }
        exclude_trigger_comment_id = Some(id);
    }
    let parent = load_parent_comment(&state, &issue, parent_id).await;
    if parent_id.is_some() && parent.is_none() {
        return error_response(StatusCode::BAD_REQUEST, "invalid parent comment");
    }
    let (actor_type, actor_id, _, originator_user_id) =
        trigger_actor(&state, &context, &headers, &issue).await;
    let plan = compute_comment_agent_triggers(CommentTriggerInput {
        state: &state,
        issue: &issue,
        content: &content,
        parent: parent.as_ref(),
        actor_type: &actor_type,
        actor_id,
        originator_user_id,
        exclude_trigger_comment_id,
    })
    .await;
    Json(json!({
        "agents": preview_agents(&plan),
        "blocked": preview_blocked(&plan),
    }))
    .into_response()
}

async fn create(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(issue_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CommentWriteRequest>,
) -> Response {
    let issue = match crate::issue::resolve_issue(&state, &context, &issue_id).await {
        Ok(issue) => issue,
        Err(response) => return response,
    };
    let content = clean_content(&request.content);
    if content.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "content is required");
    }
    let type_ = if request.type_.is_empty() {
        "comment"
    } else {
        request.type_.as_str()
    };
    if !matches!(type_, "comment" | "progress_update") {
        return error_response(StatusCode::BAD_REQUEST, "invalid comment type");
    }
    let parent_id = match request
        .parent_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
    {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid parent_id"),
    };
    let parent = load_parent_comment(&state, &issue, parent_id).await;
    if parent_id.is_some() && parent.is_none() {
        return error_response(StatusCode::BAD_REQUEST, "invalid parent comment");
    }
    let (author_type, author_id, task_id, originator_user_id) =
        trigger_actor(&state, &context, &headers, &issue).await;
    if author_type == "agent" {
        if let Some(task_id) = task_id {
            if let Ok(Some(task)) =
                patchbay_db::queries::agent::get_agent_task(&state.pool, task_id).await
            {
                if task.issue_id == Some(issue.id)
                    && task.trigger_comment_id.is_some()
                    && parent_id != task.trigger_comment_id
                    && !task
                        .coalesced_comment_ids
                        .contains(&parent_id.unwrap_or_default())
                {
                    return error_response(
                        StatusCode::CONFLICT,
                        "parent_id is not a comment this task may reply under",
                    );
                }
                match activity::has_squad_leader_no_action_evaluation_for_task(
                    &state.pool,
                    issue.id,
                    author_id,
                    &task_id.to_string(),
                )
                .await
                {
                    Ok(Some(true)) => {
                        return error_response(
                            StatusCode::CONFLICT,
                            "the squad leader has already recorded no_action for this task",
                        )
                    }
                    Ok(Some(false)) | Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(%error, task_id = %task_id, "failed to check squad leader evaluation");
                        return error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "failed to validate squad leader evaluation",
                        );
                    }
                }
            }
        }
    }
    let attachment_ids = match uuid_list(
        request.attachment_ids.as_deref().unwrap_or_default(),
        "attachment_ids",
    ) {
        Ok(ids) => ids,
        Err(response) => return response,
    };
    let suppressed = match uuid_list(&request.suppress_agent_ids, "suppress_agent_ids") {
        Ok(ids) => ids,
        Err(response) => return response,
    };
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::warn!(%error, "failed to begin comment create");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create comment",
            );
        }
    };
    let row = match comment::create_comment(
        &mut *tx,
        issue.id,
        issue.workspace_id,
        &author_type,
        author_id,
        &content,
        type_,
        parent_id,
        task_id,
        None,
        None,
        patchbay_db::dbid::new_v7(),
    )
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "issue not found"),
        Err(error) => {
            tracing::warn!(%error, "failed to create comment");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create comment",
            );
        }
    };
    let id = row.id.unwrap_or_default();
    if attachment::link_attachments_to_comment(&mut *tx, id, issue.id, attachment_ids)
        .await
        .is_err()
        || tx.commit().await.is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to link comment attachments",
        );
    }
    if let Some(parent) = parent.as_ref() {
        let root = comment::get_thread_root(&state.pool, parent.id, issue.workspace_id)
            .await
            .ok()
            .flatten();
        state
            .tasks
            .auto_unresolve_thread_on_reply(
                root.as_ref(),
                &context.workspace_id,
                &author_type,
                &author_id.to_string(),
            )
            .await;
    }
    if author_type == "agent" {
        state
            .tasks
            .cancel_deferred_escalations_for_issue_agent(issue.id, author_id)
            .await;
    }
    let created = comment::get_comment_in_workspace(&state.pool, id, issue.workspace_id)
        .await
        .ok()
        .flatten()
        .expect("created comment");
    let plan = compute_comment_agent_triggers(CommentTriggerInput {
        state: &state,
        issue: &issue,
        content: &content,
        parent: parent.as_ref(),
        actor_type: &author_type,
        actor_id: author_id,
        originator_user_id,
        exclude_trigger_comment_id: Some(id),
    })
    .await;
    let outcomes = enqueue_comment_triggers(&state, &issue, id, &plan, &suppressed).await;
    let mut value = comment_json(&state, &created, &headers).await;
    if let Some(object) = value.as_object_mut() {
        object.insert("issue_revision".into(), json!(row.issue_revision));
        object.insert("trigger_outcomes".into(), json!(outcomes));
    }
    publish(
        &state,
        &context,
        patchbay_protocol::EVENT_COMMENT_CREATED,
        &author_type,
        author_id,
        value.clone(),
    );
    (StatusCode::CREATED, Json(value)).into_response()
}

async fn update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(comment_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<CommentWriteRequest>,
) -> Response {
    let current = match load_comment(&state, &context, &comment_id).await {
        Ok(comment) => comment,
        Err(response) => return response,
    };
    let (actor_type, actor_id, task_id) =
        crate::issue::mutation_actor(&state, &context, &headers).await;
    let is_admin = matches!(context.member.role.as_str(), "owner" | "admin");
    if !(current.author_type == actor_type && current.author_id == actor_id) && !is_admin {
        return error_response(
            StatusCode::FORBIDDEN,
            "only comment author or admin can edit",
        );
    }
    let content = clean_content(&request.content);
    if content.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "content is required");
    }
    let issue =
        match issue_q::get_issue_in_workspace(&state.pool, current.issue_id, current.workspace_id)
            .await
        {
            Ok(Some(issue)) => issue,
            _ => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to update comment",
                )
            }
        };
    let originator_user_id = crate::comment_trigger::effective_invoker(
        &state,
        &issue,
        &headers,
        &actor_type,
        actor_id,
        task_id,
    )
    .await;
    let parent = load_parent_comment(&state, &issue, current.parent_id).await;
    if request
        .expected_revision
        .is_some_and(|revision| revision < 1)
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "expected_revision must be a positive integer",
        );
    }
    if let Some(expected) = request.expected_revision {
        if expected != current.revision {
            return (StatusCode::CONFLICT,Json(json!({"error":"revision conflict","resource":"comment","id":current.id,"expected_revision":expected,"actual_revision":current.revision}))).into_response();
        }
    }
    let source = if actor_type == "agent" && current.author_id == actor_id {
        task_id.unwrap_or_default()
    } else {
        Uuid::nil()
    };
    let replacement_attachments = match request.attachment_ids.as_deref() {
        Some(values) => match uuid_list(values, "attachment_ids") {
            Ok(ids) => Some(ids),
            Err(response) => return response,
        },
        None => None,
    };
    let suppressed = match uuid_list(&request.suppress_agent_ids, "suppress_agent_ids") {
        Ok(ids) => ids,
        Err(response) => return response,
    };
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update comment",
            );
        }
    };
    let updated = match comment::update_comment(
        &mut *tx,
        current.id,
        &content,
        source,
        request.expected_revision,
        request.content_base.as_deref(),
    )
    .await
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            return error_response(StatusCode::CONFLICT, "comment was edited concurrently");
        }
        Err(error) => {
            tracing::warn!(%error,"failed to update comment");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update comment",
            );
        }
    };
    // Cancel only after the strict update has succeeded, and in the same
    // transaction as the comment mutation. This prevents attachment-only,
    // unchanged, and losing concurrent edits from cancelling a still-valid
    // task batch; a later rollback also restores the queue rows.
    let cancelled = if updated.content_changed {
        match agent::cancel_agent_tasks_by_trigger_comment(&mut *tx, current.id).await {
            Ok(cancelled) => cancelled,
            Err(error) => {
                tracing::warn!(%error, comment_id = %current.id, "failed to cancel tasks for edited trigger comment");
                drop(tx);
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to prepare comment edit",
                );
            }
        }
    } else {
        Vec::new()
    };
    let mut attachment_changed = false;
    if let Some(ids) = replacement_attachments {
        match attachment::replace_comment_attachments(&mut *tx, current.id, current.issue_id, ids)
            .await
        {
            Ok(changed) => attachment_changed = changed > 0,
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to replace comment attachments",
                );
            }
        }
    }
    let attachment_revision = if !updated.content_changed && attachment_changed {
        match comment::touch_comment_after_attachment_edit(&mut *tx, current.id, current.issue_id)
            .await
        {
            Ok(Some((_, issue_revision))) => Some(issue_revision),
            Ok(None) | Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to update comment revision",
                );
            }
        }
    } else {
        None
    };
    if tx.commit().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update comment",
        );
    }
    let comment = comment::get_comment_in_workspace(&state.pool, current.id, current.workspace_id)
        .await
        .ok()
        .flatten()
        .expect("updated comment");
    let mut value = comment_json(&state, &comment, &headers).await;
    let mut outcomes = Vec::new();
    state
        .tasks
        .broadcast_cancelled_tasks(&current.workspace_id.to_string(), &cancelled)
        .await;
    if updated.content_changed {
        retrigger_cancelled_task_survivors(&state, &issue, &cancelled, Some(current.id)).await;
        let plan = compute_comment_agent_triggers(CommentTriggerInput {
            state: &state,
            issue: &issue,
            content: &content,
            parent: parent.as_ref(),
            actor_type: &actor_type,
            actor_id,
            originator_user_id,
            exclude_trigger_comment_id: Some(current.id),
        })
        .await;
        outcomes = enqueue_comment_triggers(&state, &issue, current.id, &plan, &suppressed).await;
    }
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "issue_revision".into(),
            json!(attachment_revision.unwrap_or(updated.issue_revision)),
        );
        object.insert("trigger_outcomes".into(), json!(outcomes));
    }
    publish(
        &state,
        &context,
        patchbay_protocol::EVENT_COMMENT_UPDATED,
        &actor_type,
        actor_id,
        value.clone(),
    );
    Json(value).into_response()
}

async fn delete(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(comment_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let current = match load_comment(&state, &context, &comment_id).await {
        Ok(comment) => comment,
        Err(response) => return response,
    };
    let (actor_type, actor_id, _) = crate::issue::mutation_actor(&state, &context, &headers).await;
    let is_admin = matches!(context.member.role.as_str(), "owner" | "admin");
    if !(current.author_type == actor_type && current.author_id == actor_id) && !is_admin {
        return error_response(
            StatusCode::FORBIDDEN,
            "only comment author or admin can delete",
        );
    }
    let cancelled = match state
        .tasks
        .cancel_tasks_by_trigger_comment(current.id)
        .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, "failed to cancel tasks for deleted trigger comment");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete comment",
            );
        }
    };
    let issue =
        issue_q::get_issue_in_workspace(&state.pool, current.issue_id, current.workspace_id)
            .await
            .ok()
            .flatten();
    let attachment_urls = match attachment::list_attachments_by_comment(
        &state.pool,
        current.id,
        current.workspace_id,
    )
    .await
    {
        Ok(rows) => rows.into_iter().map(|row| row.url).collect::<Vec<_>>(),
        Err(error) => {
            tracing::warn!(%error, comment_id = %current.id, "failed to collect comment attachment URLs");
            if let Some(issue) = issue.as_ref() {
                retrigger_cancelled_task_survivors(&state, issue, &cancelled, None).await;
            }
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete comment",
            );
        }
    };
    match comment::delete_comment(&state.pool, current.id, current.workspace_id).await {
        Ok(Some(row)) if row.changed => {
            if let Some(issue) = issue.as_ref() {
                retrigger_cancelled_task_survivors(&state, issue, &cancelled, Some(current.id))
                    .await;
            }
            state.bus.publish(&patchbay_events::Event{event_type:patchbay_protocol::EVENT_COMMENT_DELETED.into(),workspace_id:current.workspace_id.to_string(),actor_type,actor_id:actor_id.to_string(),payload:json!({"comment_id":current.id,"issue_id":current.issue_id,"issue_revision":row.issue_revision}),..Default::default()});
            crate::issue::delete_attachment_objects(&state, attachment_urls).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(_) => {
            if let Some(issue) = issue.as_ref() {
                retrigger_cancelled_task_survivors(&state, issue, &cancelled, None).await;
            }
            error_response(StatusCode::NOT_FOUND, "comment not found")
        }
        Err(error) => {
            tracing::warn!(%error,"failed to delete comment");
            if let Some(issue) = issue.as_ref() {
                retrigger_cancelled_task_survivors(&state, issue, &cancelled, None).await;
            }
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete comment",
            )
        }
    }
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

pub(crate) fn reaction_json(reaction: &CommentReaction) -> Value {
    json!({
        "id": reaction.id,
        "comment_id": reaction.comment_id,
        "actor_type": reaction.actor_type,
        "actor_id": reaction.actor_id,
        "emoji": reaction.emoji,
        "created_at": crate::timefmt::rfc3339(reaction.created_at),
    })
}

fn added_reaction_json(reaction: &reaction::AddReactionRow) -> Value {
    let mut response = serde_json::Map::new();
    response.insert(
        "id".into(),
        json!(reaction.id.map(|id| id.to_string()).unwrap_or_default()),
    );
    response.insert(
        "comment_id".into(),
        json!(reaction
            .comment_id
            .map(|id| id.to_string())
            .unwrap_or_default()),
    );
    response.insert("actor_type".into(), json!(reaction.actor_type));
    response.insert(
        "actor_id".into(),
        json!(reaction
            .actor_id
            .map(|id| id.to_string())
            .unwrap_or_default()),
    );
    response.insert("emoji".into(), json!(reaction.emoji));
    response.insert(
        "created_at".into(),
        json!(reaction
            .created_at
            .map(crate::timefmt::rfc3339)
            .unwrap_or_default()),
    );
    if reaction.comment_revision > 0 {
        response.insert("comment_revision".into(), json!(reaction.comment_revision));
    }
    Value::Object(response)
}

pub(crate) async fn comment_json(
    state: &HandlerState,
    comment: &Comment,
    headers: &HeaderMap,
) -> Value {
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
                .map(|item| crate::issue::AttachmentResponse::for_request(state, item, headers))
                .collect::<Vec<_>>(),
        )
        .unwrap_or_else(|_| json!([])),
    )
}

pub(crate) fn comment_json_with_related(
    comment: &Comment,
    reactions: Value,
    attachments: Value,
) -> Value {
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

pub(crate) fn publish(
    state: &HandlerState,
    context: &WorkspaceContext,
    event_type: &str,
    actor_type: &str,
    actor_id: Uuid,
    comment: Value,
) {
    state.bus.publish(&patchbay_events::Event {
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
                );
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
        let response = comment_json(&state, &sibling, &headers).await;
        publish(
            &state,
            &context,
            patchbay_protocol::EVENT_COMMENT_UNRESOLVED,
            &actor_type,
            actor_id,
            response,
        );
    }
    let response = comment_json(&state, &updated, &headers).await;
    if !was_resolved {
        publish(
            &state,
            &context,
            patchbay_protocol::EVENT_COMMENT_RESOLVED,
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
            );
        }
    };
    let response = comment_json(&state, &updated, &headers).await;
    if was_resolved {
        publish(
            &state,
            &context,
            patchbay_protocol::EVENT_COMMENT_UNRESOLVED,
            &actor_type,
            actor_id,
            response.clone(),
        );
    }
    Json(response).into_response()
}

#[derive(Deserialize)]
struct ReactionRequest {
    emoji: String,
}

async fn add_reaction(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(comment_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let current = match load_comment(&state, &context, &comment_id).await {
        Ok(comment) => comment,
        Err(response) => return response,
    };
    let request: ReactionRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.emoji.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "emoji is required");
    }
    let (actor_type, actor_id, _) = crate::issue::mutation_actor(&state, &context, &headers).await;
    let added = match reaction::add_reaction(
        &state.pool,
        current.id,
        current.workspace_id,
        &actor_type,
        actor_id,
        &request.emoji,
    )
    .await
    {
        Ok(Some(added)) => added,
        Ok(None) | Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to add reaction");
        }
    };
    let response = added_reaction_json(&added);
    if added.comment_revision > 0 {
        let (issue_title, issue_status) =
            match issue_q::get_issue(&state.pool, current.issue_id).await {
                Ok(Some(issue)) => (issue.title, issue.status),
                _ => (String::new(), String::new()),
            };
        state.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_REACTION_ADDED.into(),
            workspace_id: context.workspace_id.clone(),
            actor_type: actor_type.clone(),
            actor_id: actor_id.to_string(),
            payload: json!({
                "reaction": response,
                "issue_id": current.issue_id,
                "issue_title": issue_title,
                "issue_status": issue_status,
                "comment_id": current.id,
                "comment_author_type": current.author_type,
                "comment_author_id": current.author_id,
                "comment_revision": added.comment_revision,
            }),
            ..Default::default()
        });
    }
    (StatusCode::CREATED, Json(response)).into_response()
}

async fn remove_reaction(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(comment_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let current = match load_comment(&state, &context, &comment_id).await {
        Ok(comment) => comment,
        Err(response) => return response,
    };
    let request: ReactionRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.emoji.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "emoji is required");
    }
    let (actor_type, actor_id, _) = crate::issue::mutation_actor(&state, &context, &headers).await;
    let removed = match reaction::remove_reaction(
        &state.pool,
        current.id,
        &actor_type,
        actor_id,
        &request.emoji,
    )
    .await
    {
        Ok(Some(removed)) => removed,
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to remove reaction",
            );
        }
    };
    if removed.changed {
        state.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_REACTION_REMOVED.into(),
            workspace_id: context.workspace_id.clone(),
            actor_type: actor_type.clone(),
            actor_id: actor_id.to_string(),
            payload: json!({
                "comment_id": current.id,
                "issue_id": current.issue_id,
                "emoji": request.emoji,
                "actor_type": actor_type,
                "actor_id": actor_id,
                "comment_revision": removed.comment_revision,
            }),
            ..Default::default()
        });
    }
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_write_normalization_and_mentions_match_boundary_contract() {
        use crate::comment_trigger::mention_ids;
        let first = Uuid::parse_str("018f946a-1234-7890-abcd-1234567890ab").unwrap();
        let second = Uuid::parse_str("018f946a-2234-7890-abcd-1234567890ab").unwrap();
        let content = format!(
            "a\0 [@one](mention://agent/{first}) duplicate mention://agent/{first} and mention://agent/{second})"
        );
        assert!(!clean_content(&content).contains('\0'));
        assert_eq!(mention_ids(&content, "agent"), vec![first, second]);
        assert!(mention_ids(&content, "squad").is_empty());
    }

    #[test]
    fn attachment_and_suppression_ids_fail_closed() {
        let valid = Uuid::new_v4();
        assert_eq!(
            uuid_list(&[valid.to_string()], "attachment_ids").unwrap(),
            vec![valid]
        );
        assert!(uuid_list(&["not-a-uuid".into()], "suppress_agent_ids").is_err());
    }

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

    #[test]
    fn added_reaction_omits_zero_revision_for_idempotent_add() {
        let response = added_reaction_json(&reaction::AddReactionRow {
            id: Some(Uuid::parse_str("018f946a-5234-7890-abcd-1234567890ab").unwrap()),
            comment_id: Some(Uuid::parse_str("018f946a-2234-7890-abcd-1234567890ab").unwrap()),
            workspace_id: Some(Uuid::parse_str("018f946a-4234-7890-abcd-1234567890ab").unwrap()),
            actor_type: "member".into(),
            actor_id: Some(Uuid::parse_str("018f946a-1234-7890-abcd-1234567890ab").unwrap()),
            emoji: "👍".into(),
            created_at: Some("2026-08-23T12:34:56Z".parse().unwrap()),
            comment_revision: 0,
        });
        assert_eq!(response["emoji"], json!("👍"));
        assert_eq!(response["created_at"], json!("2026-08-23T12:34:56Z"));
        assert!(response.get("comment_revision").is_none());
    }
}
