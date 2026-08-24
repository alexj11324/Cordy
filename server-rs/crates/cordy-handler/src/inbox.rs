//! Workspace inbox handlers.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cordy_db::models::InboxItem;
use cordy_db::queries::{inbox, issue};
use cordy_middleware::workspace::WorkspaceContext;
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/inbox", get(list))
        .route("/api/inbox/", get(list))
        .route("/api/inbox/archived", get(list_archived))
        .route("/api/inbox/archived/", get(list_archived))
        .route("/api/inbox/unread-count", get(unread_count))
        .route("/api/inbox/unread-count/", get(unread_count))
        .route("/api/inbox/unread-summary", get(unread_summary))
        .route("/api/inbox/unread-summary/", get(unread_summary))
        .route("/api/inbox/mark-all-read", post(mark_all_read))
        .route("/api/inbox/mark-all-read/", post(mark_all_read))
        .route("/api/inbox/archive-all", post(archive_all))
        .route("/api/inbox/archive-all/", post(archive_all))
        .route("/api/inbox/archive-all-read", post(archive_all_read))
        .route("/api/inbox/archive-all-read/", post(archive_all_read))
        .route("/api/inbox/archive-completed", post(archive_completed))
        .route("/api/inbox/archive-completed/", post(archive_completed))
        .route("/api/inbox/{id}/read", post(mark_read))
        .route("/api/inbox/{id}/read/", post(mark_read))
        .route("/api/inbox/{id}/unread", post(mark_unread))
        .route("/api/inbox/{id}/unread/", post(mark_unread))
        .route("/api/inbox/{id}/archive", post(archive_item))
        .route("/api/inbox/{id}/archive/", post(archive_item))
        .route("/api/inbox/{id}/unarchive", post(unarchive_item))
        .route("/api/inbox/{id}/unarchive/", post(unarchive_item))
}

#[derive(Debug, Serialize)]
struct InboxItemResponse {
    id: String,
    workspace_id: String,
    recipient_type: String,
    recipient_id: String,
    r#type: String,
    severity: String,
    issue_id: Option<String>,
    title: String,
    body: Option<String>,
    read: bool,
    archived: bool,
    created_at: String,
    issue_status: Option<String>,
    actor_type: Option<String>,
    actor_id: Option<String>,
    details: Value,
}

impl From<InboxItem> for InboxItemResponse {
    fn from(value: InboxItem) -> Self {
        Self {
            id: value.id.to_string(),
            workspace_id: value.workspace_id.to_string(),
            recipient_type: value.recipient_type,
            recipient_id: value.recipient_id.to_string(),
            r#type: value.type_,
            severity: value.severity,
            issue_id: value.issue_id.map(|id| id.to_string()),
            title: value.title,
            body: value.body,
            read: value.read,
            archived: value.archived,
            created_at: crate::timefmt::rfc3339(value.created_at),
            issue_status: None,
            actor_type: value.actor_type,
            actor_id: value.actor_id.map(|id| id.to_string()),
            details: value.details.unwrap_or(Value::Null),
        }
    }
}

// Mirrors the generated 16-column inbox list rows without changing their scan contract.
#[allow(clippy::too_many_arguments)]
fn row_response(
    id: Option<Uuid>,
    workspace_id: Option<Uuid>,
    recipient_type: String,
    recipient_id: Option<Uuid>,
    type_: String,
    severity: String,
    issue_id: Option<Uuid>,
    title: String,
    body: Option<String>,
    read: bool,
    archived: bool,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
    actor_type: Option<String>,
    actor_id: Option<Uuid>,
    details: Option<Value>,
    issue_status: Option<String>,
) -> InboxItemResponse {
    InboxItemResponse {
        id: id.map(|id| id.to_string()).unwrap_or_default(),
        workspace_id: workspace_id.map(|id| id.to_string()).unwrap_or_default(),
        recipient_type,
        recipient_id: recipient_id.map(|id| id.to_string()).unwrap_or_default(),
        r#type: type_,
        severity,
        issue_id: issue_id.map(|id| id.to_string()),
        title,
        body,
        read,
        archived,
        created_at: created_at.map(crate::timefmt::rfc3339).unwrap_or_default(),
        issue_status,
        actor_type,
        actor_id: actor_id.map(|id| id.to_string()),
        details: details.unwrap_or(Value::Null),
    }
}

fn publish(state: &HandlerState, context: &WorkspaceContext, event_type: &str, payload: Value) {
    state.bus.publish(&cordy_events::Event {
        event_type: event_type.into(),
        workspace_id: context.member.workspace_id.to_string(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload,
        ..Default::default()
    });
}

async fn enrich(state: &HandlerState, mut response: InboxItemResponse) -> InboxItemResponse {
    let Some(issue_id) = response
        .issue_id
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok())
    else {
        return response;
    };
    if let Ok(Some(found)) = issue::get_issue(&state.pool, issue_id).await {
        response.issue_status = Some(found.status);
    }
    response
}

async fn load_item(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_id: &str,
) -> Result<InboxItem, Response> {
    let id = Uuid::parse_str(raw_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid inbox item id"))?;
    let item = inbox::get_inbox_item_in_workspace(&state.pool, id, context.member.workspace_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "inbox item not found"))?;
    if item.recipient_type != "member" || item.recipient_id != context.member.user_id {
        return Err(error_response(
            StatusCode::NOT_FOUND,
            "inbox item not found",
        ));
    }
    Ok(item)
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    match inbox::list_inbox_items(
        &state.pool,
        context.member.workspace_id,
        "member",
        context.member.user_id,
    )
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| {
                    row_response(
                        row.id,
                        row.workspace_id,
                        row.recipient_type,
                        row.recipient_id,
                        row.type_,
                        row.severity,
                        row.issue_id,
                        row.title,
                        row.body,
                        row.read,
                        row.archived,
                        row.created_at,
                        row.actor_type,
                        row.actor_id,
                        row.details,
                        row.issue_status,
                    )
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to list inbox");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list inbox")
        }
    }
}

async fn list_archived(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    match inbox::list_archived_inbox_items(
        &state.pool,
        context.member.workspace_id,
        "member",
        context.member.user_id,
    )
    .await
    {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| {
                    row_response(
                        row.id,
                        row.workspace_id,
                        row.recipient_type,
                        row.recipient_id,
                        row.type_,
                        row.severity,
                        row.issue_id,
                        row.title,
                        row.body,
                        row.read,
                        row.archived,
                        row.created_at,
                        row.actor_type,
                        row.actor_id,
                        row.details,
                        row.issue_status,
                    )
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to list archived inbox");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list archived inbox",
            )
        }
    }
}

async fn mutate_item(
    state: HandlerState,
    context: WorkspaceContext,
    raw_id: String,
    action: &'static str,
) -> Response {
    let previous = match load_item(&state, &context, &raw_id).await {
        Ok(item) => item,
        Err(response) => return response,
    };
    let result = match action {
        "read" => inbox::mark_inbox_read(&state.pool, previous.id).await,
        "unread" => inbox::mark_inbox_unread(&state.pool, previous.id).await,
        "archive" => inbox::archive_inbox_item(&state.pool, previous.id).await,
        "unarchive" => inbox::unarchive_inbox_item(&state.pool, previous.id).await,
        _ => unreachable!(),
    };
    let item = match result {
        Ok(Some(item)) => item,
        Ok(None) | Err(_) => {
            let message = match action {
                "read" => "failed to mark read",
                "unread" => "failed to mark unread",
                "archive" => "failed to archive",
                _ => "failed to unarchive",
            };
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, message);
        }
    };
    if let Some(issue_id) = item.issue_id {
        if action == "archive" {
            let _ = inbox::archive_inbox_by_issue(
                &state.pool,
                item.workspace_id,
                &item.recipient_type,
                item.recipient_id,
                issue_id,
            )
            .await;
        } else if action == "unarchive" {
            let _ = inbox::unarchive_inbox_by_issue(
                &state.pool,
                item.workspace_id,
                &item.recipient_type,
                item.recipient_id,
                issue_id,
            )
            .await;
        }
    }
    let (event_type, payload) = match action {
        "read" => (
            cordy_protocol::EVENT_INBOX_READ,
            json!({ "item_id": item.id, "recipient_id": item.recipient_id }),
        ),
        "unread" => (
            cordy_protocol::EVENT_INBOX_UNREAD,
            json!({ "item_id": item.id, "recipient_id": item.recipient_id }),
        ),
        "archive" => (
            cordy_protocol::EVENT_INBOX_ARCHIVED,
            json!({ "item_id": item.id, "issue_id": item.issue_id, "recipient_id": item.recipient_id }),
        ),
        _ => (
            cordy_protocol::EVENT_INBOX_UNARCHIVED,
            json!({ "item_id": item.id, "issue_id": item.issue_id, "recipient_id": item.recipient_id }),
        ),
    };
    publish(&state, &context, event_type, payload);
    Json(enrich(&state, InboxItemResponse::from(item)).await).into_response()
}

macro_rules! item_handler {
    ($name:ident, $action:literal) => {
        async fn $name(
            State(state): State<HandlerState>,
            Extension(context): Extension<WorkspaceContext>,
            Path(raw_id): Path<String>,
        ) -> Response {
            mutate_item(state, context, raw_id, $action).await
        }
    };
}

item_handler!(mark_read, "read");
item_handler!(mark_unread, "unread");
item_handler!(archive_item, "archive");
item_handler!(unarchive_item, "unarchive");

async fn unread_count(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    match inbox::count_unread_inbox(
        &state.pool,
        context.member.workspace_id,
        "member",
        context.member.user_id,
    )
    .await
    {
        Ok(Some(count)) => Json(json!({ "count": count })).into_response(),
        Ok(None) | Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to count unread inbox",
        ),
    }
}

#[derive(Debug, Serialize)]
struct WorkspaceUnreadResponse {
    workspace_id: String,
    count: i64,
}

async fn unread_summary(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    match inbox::count_unread_inbox_by_workspace(&state.pool, context.member.user_id).await {
        Ok(rows) => Json(
            rows.into_iter()
                .map(|row| WorkspaceUnreadResponse {
                    workspace_id: row
                        .workspace_id
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                    count: row.count,
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to summarize unread inbox",
        ),
    }
}

async fn batch(state: HandlerState, context: WorkspaceContext, action: &'static str) -> Response {
    let result = match action {
        "read" => {
            inbox::mark_all_inbox_read(
                &state.pool,
                context.member.workspace_id,
                context.member.user_id,
            )
            .await
        }
        "all" => {
            inbox::archive_all_inbox(
                &state.pool,
                context.member.workspace_id,
                context.member.user_id,
            )
            .await
        }
        "read-archive" => {
            inbox::archive_all_read_inbox(
                &state.pool,
                context.member.workspace_id,
                context.member.user_id,
            )
            .await
        }
        "completed" => {
            inbox::archive_completed_inbox(
                &state.pool,
                context.member.workspace_id,
                context.member.user_id,
            )
            .await
        }
        _ => unreachable!(),
    };
    let count = match result {
        Ok(count) => count,
        Err(_) => {
            let message = match action {
                "read" => "failed to mark all inbox read",
                "all" => "failed to archive all inbox",
                "read-archive" => "failed to archive all read inbox",
                _ => "failed to archive completed inbox",
            };
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, message);
        }
    };
    let event_type = if action == "read" {
        cordy_protocol::EVENT_INBOX_BATCH_READ
    } else {
        cordy_protocol::EVENT_INBOX_BATCH_ARCHIVED
    };
    publish(
        &state,
        &context,
        event_type,
        json!({ "recipient_id": context.member.user_id, "count": count }),
    );
    Json(json!({ "count": count })).into_response()
}

macro_rules! batch_handler {
    ($name:ident, $action:literal) => {
        async fn $name(
            State(state): State<HandlerState>,
            Extension(context): Extension<WorkspaceContext>,
        ) -> Response {
            batch(state, context, $action).await
        }
    };
}

batch_handler!(mark_all_read, "read");
batch_handler!(archive_all, "all");
batch_handler!(archive_all_read, "read-archive");
batch_handler!(archive_completed, "completed");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_keeps_null_details_and_nullable_fields() {
        let value = row_response(
            None,
            None,
            "member".into(),
            None,
            "mention".into(),
            "info".into(),
            None,
            "Title".into(),
            None,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
        );
        let json = serde_json::to_value(value).unwrap();
        assert_eq!(json["details"], Value::Null);
        assert_eq!(json["issue_status"], Value::Null);
        assert_eq!(json["id"], "");
    }
}
