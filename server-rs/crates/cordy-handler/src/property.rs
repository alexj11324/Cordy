//! Workspace-scoped custom issue-property definition reads.

use std::collections::HashMap;

use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::models::IssueProperty;
use cordy_db::queries::issue_property::{self, ListIssuePropertiesRow};
use cordy_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/properties", get(list))
        .route("/api/properties/{id}", get(get_one))
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct PropertyConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    options: Vec<PropertyOption>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct PropertyOption {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    color: String,
}

#[derive(Debug, Serialize)]
struct PropertyResponse {
    id: String,
    workspace_id: String,
    name: String,
    #[serde(rename = "type")]
    type_: String,
    description: String,
    icon: String,
    config: PropertyConfig,
    position: f64,
    archived: bool,
    archived_at: Option<String>,
    usage_count: i64,
    created_at: String,
    updated_at: String,
}

fn config(value: serde_json::Value) -> PropertyConfig {
    serde_json::from_value(value).unwrap_or_default()
}

fn from_model(property: IssueProperty, usage_count: i64) -> PropertyResponse {
    PropertyResponse {
        id: property.id.to_string(),
        workspace_id: property.workspace_id.to_string(),
        name: property.name,
        type_: property.type_,
        description: property.description,
        icon: property.icon,
        config: config(property.config),
        position: property.position,
        archived: property.archived_at.is_some(),
        archived_at: property.archived_at.map(crate::timefmt::rfc3339),
        usage_count,
        created_at: crate::timefmt::rfc3339(property.created_at),
        updated_at: crate::timefmt::rfc3339(property.updated_at),
    }
}

fn from_list(row: ListIssuePropertiesRow) -> Option<PropertyResponse> {
    Some(PropertyResponse {
        id: row.id?.to_string(),
        workspace_id: row.workspace_id?.to_string(),
        name: row.name,
        type_: row.type_,
        description: row.description,
        icon: row.icon,
        config: config(row.config.unwrap_or_else(|| json!({}))),
        position: row.position,
        archived: row.archived_at.is_some(),
        archived_at: row.archived_at.map(crate::timefmt::rfc3339),
        usage_count: row.usage_count,
        created_at: crate::timefmt::rfc3339(row.created_at?),
        updated_at: crate::timefmt::rfc3339(row.updated_at?),
    })
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let include_archived = query
        .get("include_archived")
        .is_some_and(|value| value == "true");
    let rows = match issue_property::list_issue_properties(
        &state.pool,
        context.member.workspace_id,
        include_archived,
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, "failed to list properties");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list properties",
            );
        }
    };
    let total = rows.len();
    let properties = rows.into_iter().filter_map(from_list).collect::<Vec<_>>();
    if properties.len() != total {
        tracing::warn!("property list contained an unexpected null identity or timestamp");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list properties",
        );
    }
    Json(json!({ "properties": properties, "total": total })).into_response()
}

async fn get_one(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let id = match Uuid::parse_str(raw_id.trim()) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid property id"),
    };
    match issue_property::get_issue_property(&state.pool, id, context.member.workspace_id).await {
        Ok(Some(property)) => Json(from_model(property, 0)).into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "property not found"),
        Err(error) => {
            tracing::warn!(%error, %id, "failed to get property");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to get property")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn property_response_matches_config_and_archive_wire_contract() {
        let archived_at = Utc::now();
        let response = from_model(
            IssueProperty {
                archived_at: Some(archived_at),
                config: json!({"options": [{"id":"a","name":"Alpha","color":"red"}]}),
                created_at: archived_at,
                description: "Severity".into(),
                icon: "flag".into(),
                id: Uuid::nil(),
                name: "Severity".into(),
                position: 1.0,
                type_: "select".into(),
                updated_at: archived_at,
                workspace_id: Uuid::nil(),
            },
            3,
        );
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["type"], "select");
        assert_eq!(value["config"]["options"][0]["id"], "a");
        assert_eq!(value["archived"], true);
        assert_eq!(value["usage_count"], 3);
    }

    #[test]
    fn invalid_or_empty_config_degrades_to_object() {
        assert_eq!(
            serde_json::to_value(config(json!(null))).unwrap(),
            json!({})
        );
        assert_eq!(serde_json::to_value(config(json!({}))).unwrap(), json!({}));
    }
}
