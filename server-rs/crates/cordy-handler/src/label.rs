//! Workspace label catalog handlers.

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::models::IssueLabel;
use cordy_db::queries::issue_label;
use cordy_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/labels", get(list_labels).post(create_label))
        .route("/api/labels/", get(list_labels).post(create_label))
        .route(
            "/api/labels/{id}",
            get(get_label).put(update_label).delete(delete_label),
        )
        .route(
            "/api/labels/{id}/",
            get(get_label).put(update_label).delete(delete_label),
        )
}

#[derive(Debug, Serialize)]
struct LabelResponse {
    id: Uuid,
    workspace_id: Uuid,
    resource_type: String,
    name: String,
    description: String,
    color: String,
    usage_count: i64,
    created_at: String,
    updated_at: String,
}

impl From<IssueLabel> for LabelResponse {
    fn from(value: IssueLabel) -> Self {
        Self {
            id: value.id,
            workspace_id: value.workspace_id,
            resource_type: value.resource_type,
            name: value.name,
            description: value.description,
            color: value.color,
            usage_count: 0,
            created_at: crate::timefmt::rfc3339(value.created_at),
            updated_at: crate::timefmt::rfc3339(value.updated_at),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ListParams {
    resource_type: Option<String>,
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "workspace not found"))
}

fn resource_type(raw: Option<&str>) -> Result<&str, Response> {
    let value = raw
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("issue");
    match value {
        "issue" | "agent" | "skill" => Ok(value),
        _ => Err(error_response(
            StatusCode::BAD_REQUEST,
            "resource_type must be issue, agent, or skill",
        )),
    }
}

fn valid_name(raw: &str) -> Result<String, Response> {
    if raw.chars().any(char::is_control) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "name cannot contain tabs, newlines, or control characters",
        ));
    }
    let name = raw.trim();
    if name.is_empty() {
        return Err(error_response(StatusCode::BAD_REQUEST, "name is required"));
    }
    if name.chars().count() > 32 {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "name must be 32 characters or fewer",
        ));
    }
    Ok(name.to_string())
}

fn valid_color(raw: &str) -> Result<String, Response> {
    let color = raw.trim().trim_start_matches('#');
    if color.len() != 6 || !color.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "color must be a 6-digit hex value like #3b82f6",
        ));
    }
    Ok(format!("#{color}").to_ascii_lowercase())
}

fn db_error(error: anyhow::Error, message: &'static str) -> Response {
    tracing::warn!(%error, "{message}");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, message)
}

fn unique_violation(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(|error| error.as_database_error())
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23505")
}

async fn list_labels(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<ListParams>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let resource_type = match resource_type(params.resource_type.as_deref()) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match issue_label::list_labels(&state.pool, workspace_id, resource_type).await {
        Ok(labels) => {
            let labels = labels
                .into_iter()
                .filter_map(|label| {
                    Some(LabelResponse {
                        id: label.id?,
                        workspace_id: label.workspace_id?,
                        resource_type: label.resource_type,
                        name: label.name,
                        description: label.description,
                        color: label.color,
                        usage_count: label.usage_count,
                        created_at: crate::timefmt::rfc3339(label.created_at?),
                        updated_at: crate::timefmt::rfc3339(label.updated_at?),
                    })
                })
                .collect::<Vec<_>>();
            Json(json!({ "labels": labels, "total": labels.len() })).into_response()
        }
        Err(error) => db_error(error, "failed to list labels"),
    }
}

async fn get_label(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid label id");
    };
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match issue_label::get_label(&state.pool, id, workspace_id).await {
        Ok(Some(label)) => Json(LabelResponse::from(label)).into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "label not found"),
        Err(error) => db_error(error, "failed to get label"),
    }
}

#[derive(Debug, Deserialize)]
struct CreateRequest {
    #[serde(default)]
    resource_type: String,
    name: String,
    #[serde(default)]
    description: String,
    color: String,
}

async fn create_label(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<CreateRequest>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let name = match valid_name(&request.name) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let color = match valid_color(&request.color) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let resource_type = match resource_type(Some(&request.resource_type)) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let description = request.description.trim().replace('\0', "");
    match issue_label::create_label(
        &state.pool,
        workspace_id,
        resource_type,
        &name,
        &description,
        &color,
    )
    .await
    {
        Ok(Some(label)) => {
            let response = LabelResponse::from(label);
            state.bus.publish(&cordy_events::Event {
                event_type: "label:created".into(),
                workspace_id: workspace_id.to_string(),
                actor_type: "member".into(),
                actor_id: context.member.user_id.to_string(),
                payload: json!({ "label": response }),
                ..Default::default()
            });
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Ok(None) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to create label"),
        Err(error) if unique_violation(&error) => error_response(
            StatusCode::CONFLICT,
            "a label with that name already exists",
        ),
        Err(error) => db_error(error, "failed to create label"),
    }
}

#[derive(Debug, Deserialize)]
struct UpdateRequest {
    name: Option<String>,
    description: Option<String>,
    color: Option<String>,
}

async fn update_label(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    Json(request): Json<UpdateRequest>,
) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid label id");
    };
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let name = match request.name.as_deref().map(valid_name).transpose() {
        Ok(value) => value,
        Err(response) => return response,
    };
    let color = match request.color.as_deref().map(valid_color).transpose() {
        Ok(value) => value,
        Err(response) => return response,
    };
    let description = request
        .description
        .map(|value| value.trim().replace('\0', ""));
    match issue_label::update_label(
        &state.pool,
        id,
        workspace_id,
        name.as_deref(),
        description.as_deref(),
        color.as_deref(),
    )
    .await
    {
        Ok(Some(label)) => {
            let response = LabelResponse::from(label);
            state.bus.publish(&cordy_events::Event {
                event_type: "label:updated".into(),
                workspace_id: workspace_id.to_string(),
                actor_type: "member".into(),
                actor_id: context.member.user_id.to_string(),
                payload: json!({ "label": response }),
                ..Default::default()
            });
            Json(response).into_response()
        }
        Ok(None) => error_response(StatusCode::NOT_FOUND, "label not found"),
        Err(error) if unique_violation(&error) => error_response(
            StatusCode::CONFLICT,
            "a label with that name already exists",
        ),
        Err(error) => db_error(error, "failed to update label"),
    }
}

async fn delete_label(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let Ok(id) = Uuid::parse_str(&id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid label id");
    };
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(error) => return db_error(error.into(), "failed to start transaction"),
    };
    let result = async {
        issue_label::delete_issue_label_assignments_by_label(&mut *transaction, id).await?;
        issue_label::delete_agent_label_assignments_by_label(&mut *transaction, id).await?;
        issue_label::delete_skill_label_assignments_by_label(&mut *transaction, id).await?;
        let deleted = issue_label::delete_label(&mut *transaction, id, workspace_id).await?;
        if deleted.is_none() {
            anyhow::bail!("not_found");
        }
        transaction.commit().await?;
        anyhow::Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            state.bus.publish(&cordy_events::Event {
                event_type: "label:deleted".into(),
                workspace_id: workspace_id.to_string(),
                actor_type: "member".into(),
                actor_id: context.member.user_id.to_string(),
                payload: json!({ "label_id": id }),
                ..Default::default()
            });
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) if error.to_string() == "not_found" => {
            error_response(StatusCode::NOT_FOUND, "label not found")
        }
        Err(error) => db_error(error, "failed to delete label"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_validation_matches_go_contract() {
        assert_eq!(resource_type(None).unwrap(), "issue");
        assert!(resource_type(Some("project")).is_err());
        assert_eq!(valid_color("3B82F6").unwrap(), "#3b82f6");
        assert!(valid_color("red").is_err());
        assert_eq!(valid_name("  triage  ").unwrap(), "triage");
        assert!(valid_name("bad\nname").is_err());
    }
}
