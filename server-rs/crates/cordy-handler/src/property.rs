//! Workspace-scoped custom issue-property definition reads.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
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
        .route("/api/properties", get(list).post(create))
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

#[derive(Debug, Default, Deserialize)]
struct CreateRequest {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    type_: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    icon: String,
    config: Option<PropertyConfig>,
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

const PROPERTY_TYPES: &[&str] = &[
    "text",
    "number",
    "select",
    "multi_select",
    "date",
    "checkbox",
    "url",
    "actor",
    "multi_actor",
];

const PROPERTY_ICONS: &[&str] = &[
    "circle-dot",
    "signal-high",
    "user-round",
    "folder-kanban",
    "calendar-days",
    "tag",
    "milestone",
    "flag",
    "bookmark",
    "star",
    "target",
    "shield",
    "bug",
    "zap",
    "rocket",
    "sparkles",
    "lightbulb",
    "globe-2",
    "link",
    "hash",
    "list-checks",
    "circle-check",
    "clock-3",
    "briefcase-business",
    "layers-3",
    "gauge",
    "database",
    "code-2",
    "palette",
    "megaphone",
    "map-pin",
    "package",
    "wrench",
    "heart",
    "circle-alert",
    "lock-keyhole",
];

const RESERVED_NAMES: &[&str] = &[
    "status",
    "priority",
    "assignee",
    "project",
    "parent",
    "stage",
    "label",
    "labels",
    "start_date",
    "due_date",
    "title",
    "description",
    "creator",
    "created_at",
    "updated_at",
    "metadata",
    "properties",
];

fn validate_name(raw: &str) -> Result<String, String> {
    if raw.chars().any(char::is_control) {
        return Err("name cannot contain tabs, newlines, or control characters".into());
    }
    let name = raw.trim();
    if name.is_empty() {
        return Err("name is required".into());
    }
    if name.chars().count() > 32 {
        return Err("name must be 32 characters or fewer".into());
    }
    let normalized = name.to_lowercase().replace(' ', "_");
    if RESERVED_NAMES.contains(&normalized.as_str()) {
        return Err(format!("{name:?} is reserved for a built-in issue field"));
    }
    Ok(name.to_string())
}

fn validate_icon(raw: &str) -> Result<String, String> {
    if raw.chars().any(char::is_control) {
        return Err("icon cannot contain tabs, newlines, or control characters".into());
    }
    let icon = raw.trim();
    if icon.chars().count() > 32 {
        return Err("icon must be 32 characters or fewer".into());
    }
    if !icon.is_empty() && !PROPERTY_ICONS.contains(&icon) {
        return Err("icon must be a supported icon key".into());
    }
    Ok(icon.to_string())
}

fn normalize_color(raw: &str) -> Result<String, String> {
    let value = raw.trim().strip_prefix('#').unwrap_or(raw.trim());
    if value.len() != 6 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("color must be a 6-digit hex value like #3b82f6".into());
    }
    Ok(format!("#{}", value.to_ascii_lowercase()))
}

fn validate_option_name(raw: &str) -> Result<String, String> {
    if raw.chars().any(char::is_control) {
        return Err("name cannot contain tabs, newlines, or control characters".into());
    }
    let name = raw.trim();
    if name.is_empty() {
        return Err("name is required".into());
    }
    if name.chars().count() > 32 {
        return Err("name must be 32 characters or fewer".into());
    }
    Ok(name.to_string())
}

fn validate_config(
    property_type: &str,
    config: Option<PropertyConfig>,
) -> Result<serde_json::Value, String> {
    let has_options = matches!(property_type, "select" | "multi_select");
    let Some(config) = config else {
        return if has_options {
            Err("select properties require at least one option".into())
        } else {
            Ok(json!({}))
        };
    };
    if !has_options {
        return if config.options.is_empty() {
            Ok(json!({}))
        } else {
            Err(format!("type {property_type:?} does not accept options"))
        };
    }
    if config.options.is_empty() {
        return Err("select properties require at least one option".into());
    }
    if config.options.len() > 50 {
        return Err("a property cannot have more than 50 options".into());
    }
    let mut seen_ids = std::collections::HashSet::new();
    let mut seen_names = std::collections::HashSet::new();
    let mut options = Vec::with_capacity(config.options.len());
    for option in config.options {
        let name = validate_option_name(&option.name).map_err(|error| format!("option {error}"))?;
        if !seen_names.insert(name.to_lowercase()) {
            return Err(format!("duplicate option name {name:?}"));
        }
        let color =
            normalize_color(&option.color).map_err(|error| format!("option {name:?}: {error}"))?;
        let id = if option.id.trim().is_empty() {
            Uuid::new_v4().to_string()
        } else {
            let id = option.id.trim().to_string();
            Uuid::parse_str(&id).map_err(|_| format!("option {name:?}: id must be a UUID"))?;
            id
        };
        if !seen_ids.insert(id.clone()) {
            return Err(format!("duplicate option id {id:?}"));
        }
        options.push(PropertyOption { id, name, color });
    }
    serde_json::to_value(PropertyConfig { options })
        .map_err(|_| "invalid property config".to_string())
}

fn decode_create(body: &[u8]) -> Result<CreateRequest, ()> {
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    CreateRequest::deserialize(&mut deserializer).map_err(|_| ())
}

fn property_definition_actor_is_agent(headers: &HeaderMap, resolved_actor_type: &str) -> bool {
    headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|source| source == "task_token")
        || resolved_actor_type == "agent"
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

async fn create(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Auth stamps task-token requests with a server-owned source marker. Keep
    // that identity authoritative even if the task is concurrently cleaned up
    // before the legacy task lookup below can complete.
    if property_definition_actor_is_agent(&headers, "member") {
        return error_response(
            StatusCode::FORBIDDEN,
            "agents cannot manage property definitions",
        );
    }
    let (actor_type, _, _) = crate::issue::mutation_actor(&state, &context, &headers).await;
    if property_definition_actor_is_agent(&headers, &actor_type) {
        return error_response(
            StatusCode::FORBIDDEN,
            "agents cannot manage property definitions",
        );
    }
    if !matches!(context.member.role.as_str(), "owner" | "admin") {
        return error_response(StatusCode::FORBIDDEN, "insufficient workspace role");
    }
    let request = match decode_create(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let name = match validate_name(&request.name) {
        Ok(name) => name,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    if !PROPERTY_TYPES.contains(&request.type_.as_str()) {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!(
                "invalid type {:?}; valid types: {}",
                request.type_,
                PROPERTY_TYPES.join(", ")
            ),
        );
    }
    if request.description.chars().count() > 500 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "description must be 500 characters or fewer",
        );
    }
    let icon = match validate_icon(&request.icon) {
        Ok(icon) => icon,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let config = match validate_config(&request.type_, request.config) {
        Ok(config) => config,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    let description = request.description.trim().replace('\0', "");

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "failed to begin property definition create");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create property",
            );
        }
    };
    if let Err(error) = sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("props:{}", context.member.workspace_id))
        .execute(&mut *transaction)
        .await
    {
        tracing::warn!(%error, "failed to lock property definitions");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create property",
        );
    }
    let active = match issue_property::count_active_issue_properties(
        &mut *transaction,
        context.member.workspace_id,
    )
    .await
    {
        Ok(Some(active)) => active,
        Ok(None) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create property",
            )
        }
        Err(error) => {
            tracing::warn!(%error, "failed to count active properties");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create property",
            );
        }
    };
    if active >= 20 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "a workspace cannot have more than 20 active properties; archive unused ones first",
        );
    }
    let property = match issue_property::create_issue_property(
        &mut *transaction,
        context.member.workspace_id,
        &name,
        &request.type_,
        &description,
        &icon,
        &config,
    )
    .await
    {
        Ok(Some(property)) => property,
        Ok(None) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create property",
            )
        }
        Err(error)
            if error
                .downcast_ref::<sqlx::Error>()
                .and_then(sqlx::Error::as_database_error)
                .and_then(|error| error.code())
                .is_some_and(|code| code == "23505") =>
        {
            return error_response(
                StatusCode::CONFLICT,
                "a property with that name already exists",
            )
        }
        Err(error) => {
            tracing::warn!(%error, "failed to create property");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create property",
            );
        }
    };
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, "failed to commit property definition create");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create property",
        );
    }
    let response = from_model(property, 0);
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_PROPERTY_CREATED.into(),
        workspace_id: context.member.workspace_id.to_string(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload: json!({ "property": &response }),
        ..Default::default()
    });
    (StatusCode::CREATED, Json(response)).into_response()
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

    #[test]
    fn definition_validation_matches_names_icons_and_option_contract() {
        assert!(validate_name("Due Date").is_err());
        assert_eq!(validate_name(" Customer ").as_deref(), Ok("Customer"));
        assert!(validate_name("line\nbreak").is_err());
        assert_eq!(validate_icon(" flag ").as_deref(), Ok("flag"));
        assert!(validate_icon("⚑").is_err());

        let config = validate_config(
            "select",
            Some(PropertyConfig {
                options: vec![PropertyOption {
                    id: String::new(),
                    name: " Critical ".into(),
                    color: "3B82F6".into(),
                }],
            }),
        )
        .unwrap();
        assert_eq!(config["options"][0]["name"], "Critical");
        assert_eq!(config["options"][0]["color"], "#3b82f6");
        assert!(Uuid::parse_str(config["options"][0]["id"].as_str().unwrap()).is_ok());
        assert!(validate_config(
            "text",
            Some(PropertyConfig {
                options: vec![PropertyOption::default()]
            })
        )
        .is_err());
    }

    #[test]
    fn create_decoder_accepts_unknown_fields_and_first_json_value() {
        let request =
            decode_create(br#"{"name":"Severity","type":"text","unknown":true} trailing"#).unwrap();
        assert_eq!(request.name, "Severity");
        assert_eq!(request.type_, "text");
        assert!(decode_create(b"").is_err());
    }

    #[test]
    fn task_token_marker_remains_agent_when_legacy_lookup_falls_back_to_member() {
        let mut headers = HeaderMap::new();
        headers.insert("x-actor-source", "task_token".parse().unwrap());

        assert!(property_definition_actor_is_agent(&headers, "member"));
        assert!(!property_definition_actor_is_agent(
            &HeaderMap::new(),
            "member"
        ));
    }
}
