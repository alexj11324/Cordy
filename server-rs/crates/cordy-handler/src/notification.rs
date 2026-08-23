//! Per-user workspace notification preferences.

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::queries::notification_preference;
use cordy_middleware::workspace::WorkspaceContext;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route(
            "/api/notification-preferences",
            get(get_preferences)
                .patch(patch_preferences)
                .put(update_preferences),
        )
        .route(
            "/api/notification-preferences/",
            get(get_preferences)
                .patch(patch_preferences)
                .put(update_preferences),
        )
}

fn context_ids(context: &WorkspaceContext) -> Result<(Uuid, Uuid), Response> {
    let workspace_id = Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "workspace not found"))?;
    Ok((workspace_id, context.member.user_id))
}

fn response(workspace_id: Uuid, preferences: Value) -> Response {
    let preferences = preferences.as_object().cloned().unwrap_or_default();
    Json(json!({
        "workspace_id": workspace_id,
        "preferences": preferences,
    }))
    .into_response()
}

async fn get_preferences(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let (workspace_id, user_id) = match context_ids(&context) {
        Ok(ids) => ids,
        Err(response) => return response,
    };
    match notification_preference::get_notification_preference(&state.pool, workspace_id, user_id)
        .await
    {
        Ok(Some(preference)) => response(workspace_id, preference.preferences),
        Ok(None) => response(workspace_id, Value::Object(Map::new())),
        Err(error) => {
            tracing::warn!(%error, "notification preference lookup failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to get notification preferences",
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct PreferenceRequest {
    preferences: Option<Map<String, Value>>,
}

fn validate(request: PreferenceRequest) -> Result<Value, Response> {
    let Some(preferences) = request.preferences else {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "preferences field is required",
        ));
    };
    for (group, value) in &preferences {
        if !matches!(
            group.as_str(),
            "assignments"
                | "status_changes"
                | "comments"
                | "mentions"
                | "updates"
                | "agent_activity"
                | "system_notifications"
        ) {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid preference group: {group}"),
            ));
        }
        if !matches!(value.as_str(), Some("all" | "muted")) {
            let value = value.as_str().unwrap_or_default();
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                &format!("invalid preference value: {value}"),
            ));
        }
    }
    Ok(Value::Object(preferences))
}

async fn update_preferences(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<PreferenceRequest>,
) -> Response {
    write_preferences(state, context, request, false).await
}

async fn patch_preferences(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<PreferenceRequest>,
) -> Response {
    write_preferences(state, context, request, true).await
}

async fn write_preferences(
    state: HandlerState,
    context: WorkspaceContext,
    request: PreferenceRequest,
    patch: bool,
) -> Response {
    let (workspace_id, user_id) = match context_ids(&context) {
        Ok(ids) => ids,
        Err(response) => return response,
    };
    let preferences = match validate(request) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let result = if patch {
        notification_preference::patch_notification_preference(
            &state.pool,
            workspace_id,
            user_id,
            &preferences,
        )
        .await
    } else {
        notification_preference::upsert_notification_preference(
            &state.pool,
            workspace_id,
            user_id,
            &preferences,
        )
        .await
    };
    match result {
        Ok(Some(preference)) => response(workspace_id, preference.preferences),
        Ok(None) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update notification preferences",
        ),
        Err(error) => {
            tracing::warn!(%error, "notification preference update failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update notification preferences",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_groups_and_values() {
        assert!(validate(PreferenceRequest {
            preferences: Some(Map::from_iter([("unknown".into(), json!("all"))])),
        })
        .is_err());
        assert!(validate(PreferenceRequest {
            preferences: Some(Map::from_iter([("comments".into(), json!("some"))])),
        })
        .is_err());
        assert!(validate(PreferenceRequest {
            preferences: Some(Map::from_iter([("comments".into(), json!("muted"))])),
        })
        .is_ok());
    }
}
