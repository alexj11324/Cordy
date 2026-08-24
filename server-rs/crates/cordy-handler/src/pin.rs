//! Per-user workspace pins.

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::models::PinnedItem;
use cordy_db::queries::pinned_item;
use cordy_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const VIEW_PIN_TARGET_QUERY: &str = r#"SELECT 1 FROM issue_view
WHERE id = $1
  AND workspace_id = $2
  AND (owner_id = $3 OR visibility = 'workspace')"#;

#[derive(Debug, Serialize)]
struct PinnedItemResponse {
    id: Uuid,
    workspace_id: Uuid,
    user_id: Uuid,
    item_type: String,
    item_id: Uuid,
    position: f64,
    created_at: String,
}

impl From<PinnedItem> for PinnedItemResponse {
    fn from(pin: PinnedItem) -> Self {
        Self {
            id: pin.id,
            workspace_id: pin.workspace_id,
            user_id: pin.user_id,
            item_type: pin.item_type,
            item_id: pin.item_id,
            position: pin.position,
            created_at: crate::timefmt::rfc3339(pin.created_at),
        }
    }
}

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/pins", get(list).post(create))
        .route("/api/pins/", get(list).post(create))
        .route("/api/pins/reorder", axum::routing::put(reorder))
        .route(
            "/api/pins/{item_type}/{item_id}",
            axum::routing::delete(remove),
        )
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "workspace not found"))
}

fn db_error(error: anyhow::Error, message: &'static str) -> Response {
    tracing::warn!(%error, "{message}");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, message)
}

fn publish(
    state: &HandlerState,
    context: &WorkspaceContext,
    kind: &str,
    payload: serde_json::Value,
) {
    state.bus.publish(&cordy_events::Event {
        event_type: kind.into(),
        workspace_id: context.workspace_id.clone(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload,
        ..Default::default()
    });
}

#[derive(Default, Deserialize)]
struct ListQuery {
    include: Option<String>,
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(query): Query<ListQuery>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(r) => return r,
    };
    match pinned_item::list_pinned_items(&state.pool, workspace_id, context.member.user_id).await {
        Ok(mut pins) => {
            if !query.include.as_deref().unwrap_or("").contains("view") {
                pins.retain(|pin| pin.item_type != "view");
            }
            Json(
                pins.into_iter()
                    .map(PinnedItemResponse::from)
                    .collect::<Vec<_>>(),
            )
            .into_response()
        }
        Err(error) => db_error(error, "failed to list pins"),
    }
}

#[derive(Deserialize)]
struct CreateRequest {
    item_type: String,
    item_id: String,
}

async fn create(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<CreateRequest>,
) -> Response {
    if !matches!(request.item_type.as_str(), "issue" | "project" | "view") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "item_type must be 'issue', 'project' or 'view'",
        );
    }
    let Ok(item_id) = Uuid::parse_str(&request.item_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid item_id");
    };
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let exists = match request.item_type.as_str() {
        "issue" => {
            sqlx::query("SELECT 1 FROM issue WHERE id=$1 AND workspace_id=$2")
                .bind(item_id)
                .bind(workspace_id)
                .fetch_optional(&state.pool)
                .await
        }
        "project" => {
            sqlx::query("SELECT 1 FROM project WHERE id=$1 AND workspace_id=$2")
                .bind(item_id)
                .bind(workspace_id)
                .fetch_optional(&state.pool)
                .await
        }
        _ => {
            sqlx::query(VIEW_PIN_TARGET_QUERY)
                .bind(item_id)
                .bind(workspace_id)
                .bind(context.member.user_id)
                .fetch_optional(&state.pool)
                .await
        }
    };
    match exists {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error_response(
                StatusCode::NOT_FOUND,
                &format!("{} not found", request.item_type),
            )
        }
        Err(error) => return db_error(error.into(), "failed to verify pin target"),
    }
    let position = match pinned_item::get_max_pinned_item_position(
        &state.pool,
        workspace_id,
        context.member.user_id,
    )
    .await
    {
        Ok(value) => value.unwrap_or(0.0) + 1.0,
        Err(error) => return db_error(error, "failed to get position"),
    };
    match pinned_item::create_pinned_item(
        &state.pool,
        workspace_id,
        context.member.user_id,
        &request.item_type,
        item_id,
        position,
    )
    .await
    {
        Ok(Some(pin)) => {
            let response = PinnedItemResponse::from(pin);
            publish(&state, &context, "pin:created", json!({ "pin": &response }));
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Ok(None) => db_error(
            anyhow::anyhow!("missing returned row"),
            "failed to create pin",
        ),
        Err(error)
            if error
                .downcast_ref::<sqlx::Error>()
                .and_then(|e| e.as_database_error())
                .and_then(|e| e.code())
                .is_some_and(|c| c == "23505") =>
        {
            error_response(StatusCode::CONFLICT, "item already pinned")
        }
        Err(error) => db_error(error, "failed to create pin"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_pin_validation_uses_issue_view_schema_and_visibility_contract() {
        assert!(VIEW_PIN_TARGET_QUERY.contains("owner_id = $3"));
        assert!(VIEW_PIN_TARGET_QUERY.contains("visibility = 'workspace'"));
        assert!(!VIEW_PIN_TARGET_QUERY.contains("user_id"));
        assert!(!VIEW_PIN_TARGET_QUERY.contains("is_shared"));
    }
}

async fn remove(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((item_type, item_id)): Path<(String, String)>,
) -> Response {
    let Ok(item_id) = Uuid::parse_str(&item_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid item id");
    };
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(r) => return r,
    };
    match pinned_item::delete_pinned_item(
        &state.pool,
        workspace_id,
        context.member.user_id,
        &item_type,
        item_id,
    )
    .await
    {
        Ok(_) => {
            publish(
                &state,
                &context,
                "pin:deleted",
                json!({ "item_type": item_type, "item_id": item_id }),
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => db_error(error, "failed to delete pin"),
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct ReorderItem {
    id: Uuid,
    position: f64,
}
#[derive(Deserialize)]
struct ReorderRequest {
    items: Vec<ReorderItem>,
}

async fn reorder(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<ReorderRequest>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(r) => return r,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(error) => return db_error(error.into(), "failed to reorder pins"),
    };
    for item in &request.items {
        if let Err(error) = pinned_item::update_pinned_item_position(
            &mut *transaction,
            item.position,
            item.id,
            workspace_id,
            context.member.user_id,
        )
        .await
        {
            return db_error(error, "failed to reorder pins");
        }
    }
    if let Err(error) = transaction.commit().await {
        return db_error(error.into(), "failed to reorder pins");
    }
    publish(
        &state,
        &context,
        "pin:reordered",
        json!({ "items": request.items }),
    );
    StatusCode::NO_CONTENT.into_response()
}
