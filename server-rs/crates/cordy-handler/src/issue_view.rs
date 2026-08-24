//! Workspace-scoped saved issue views.

use axum::body::{to_bytes, Body, Bytes};
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::models::IssueView;
use cordy_db::queries::{issue_view, member, project};
use cordy_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const BODY_LIMIT: usize = 128 * 1024;
const NAME_LIMIT: usize = 80;
const OWNER_VIEW_LIMIT: i64 = 100;
const SCOPE_TYPES: &[&str] = &["workspace", "my", "project"];
const MY_VARIANTS: &[&str] = &["assigned", "created", "involved", "any"];
const WORKSPACE_VARIANTS: &[&str] = &["members", "agents"];
const VISIBILITIES: &[&str] = &["private", "workspace"];

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/issue-views", get(list).post(create))
        .route(
            "/api/issue-views/{id}",
            get(get_one).patch(update).delete(delete),
        )
}

fn deserialize_null_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Option::unwrap_or_default)
}

fn deserialize_null_i32<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<i32>::deserialize(deserializer).map(Option::unwrap_or_default)
}

#[derive(Debug, Default)]
enum JsonInput {
    #[default]
    Missing,
    Present(Value),
}

fn deserialize_json_input<'de, D>(deserializer: D) -> Result<JsonInput, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(JsonInput::Present)
}

#[derive(Debug, Default, Deserialize)]
struct CreateRequest {
    #[serde(default, deserialize_with = "deserialize_null_string")]
    name: String,
    #[serde(default, deserialize_with = "deserialize_null_string")]
    scope_type: String,
    scope_id: Option<String>,
    scope_variant: Option<String>,
    #[serde(default, deserialize_with = "deserialize_null_string")]
    visibility: String,
    #[serde(default, deserialize_with = "deserialize_null_i32")]
    definition_version: i32,
    #[serde(default, deserialize_with = "deserialize_json_input")]
    query: JsonInput,
    #[serde(default, deserialize_with = "deserialize_json_input")]
    display: JsonInput,
}

#[derive(Debug, Default, Deserialize)]
struct ListParams {
    #[serde(default)]
    scope_type: String,
    scope_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct UpdateRequest {
    name: Option<String>,
    visibility: Option<String>,
    scope_variant: Option<String>,
    #[serde(default, deserialize_with = "deserialize_json_input")]
    query: JsonInput,
    #[serde(default, deserialize_with = "deserialize_json_input")]
    display: JsonInput,
    #[serde(default, deserialize_with = "deserialize_null_i32")]
    expected_revision: i32,
}

#[derive(Debug, Serialize)]
struct IssueViewResponse {
    id: String,
    workspace_id: String,
    owner_id: String,
    name: String,
    scope_type: String,
    scope_id: Option<String>,
    scope_variant: Option<String>,
    visibility: String,
    definition_version: i32,
    query: Value,
    display: Value,
    revision: i32,
    created_at: String,
    updated_at: String,
}

impl From<IssueView> for IssueViewResponse {
    fn from(view: IssueView) -> Self {
        Self {
            id: view.id.to_string(),
            workspace_id: view.workspace_id.to_string(),
            owner_id: view.owner_id.to_string(),
            name: view.name,
            scope_type: view.scope_type,
            scope_id: view.scope_id.map(|id| id.to_string()),
            scope_variant: view.scope_variant,
            visibility: view.visibility,
            definition_version: view.definition_version,
            query: view.query,
            display: view.display,
            revision: view.revision,
            created_at: crate::timefmt::rfc3339(view.created_at),
            updated_at: crate::timefmt::rfc3339(view.updated_at),
        }
    }
}

fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ()> {
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    T::deserialize(&mut deserializer).map_err(|_| ())
}

async fn bounded_body(body: Body) -> Result<Bytes, Response> {
    to_bytes(body, BODY_LIMIT)
        .await
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid request body"))
}

fn validate_name(name: &str) -> Result<(), Response> {
    if !(1..=NAME_LIMIT).contains(&name.chars().count()) {
        Err(error_response(
            StatusCode::BAD_REQUEST,
            "name must be between 1 and 80 characters",
        ))
    } else {
        Ok(())
    }
}

fn validate_variant(scope_type: &str, variant: Option<&str>) -> Result<Option<String>, ()> {
    if scope_type == "my" {
        return variant
            .filter(|value| MY_VARIANTS.contains(value))
            .map(|value| Some(value.to_string()))
            .ok_or(());
    }
    match variant {
        None | Some("") | Some("all") => Ok(None),
        Some(value) if WORKSPACE_VARIANTS.contains(&value) => Ok(Some(value.to_string())),
        Some(_) => Err(()),
    }
}

fn object_input(input: JsonInput, missing_default: Option<Value>) -> Result<Value, ()> {
    match input {
        JsonInput::Missing => missing_default.ok_or(()),
        JsonInput::Present(value) if value.is_object() => Ok(value),
        JsonInput::Present(_) => Err(()),
    }
}

fn can_read(view: &IssueView, user_id: Uuid) -> bool {
    view.owner_id == user_id || view.visibility == "workspace"
}

async fn can_manage(state: &HandlerState, view: &IssueView, user_id: Uuid) -> bool {
    if view.owner_id == user_id {
        return true;
    }
    if view.visibility != "workspace" {
        return false;
    }
    matches!(
        member::get_member_by_user_and_workspace(&state.pool, user_id, view.workspace_id).await,
        Ok(Some(member)) if matches!(member.role.as_str(), "owner" | "admin")
    )
}

async fn load_for_user(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_id: &str,
) -> Result<IssueView, Response> {
    let id = Uuid::parse_str(raw_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid view id"))?;
    match issue_view::get_issue_view(&state.pool, id, context.member.workspace_id).await {
        Ok(Some(view)) if can_read(&view, context.member.user_id) => Ok(view),
        Ok(Some(_)) | Ok(None) => Err(error_response(StatusCode::NOT_FOUND, "view not found")),
        Err(error) => {
            tracing::warn!(%error, %id, "failed to load issue view");
            Err(error_response(StatusCode::NOT_FOUND, "view not found"))
        }
    }
}

async fn create(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    body: Body,
) -> Response {
    let body = match bounded_body(body).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let mut request: CreateRequest = match decode(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if let Err(response) = validate_name(&request.name) {
        return response;
    }
    let count = match issue_view::count_issue_views_by_owner(
        &state.pool,
        context.member.workspace_id,
        context.member.user_id,
    )
    .await
    {
        Ok(Some(count)) => count,
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to check view quota",
            )
        }
    };
    if count >= OWNER_VIEW_LIMIT {
        return error_response(
            StatusCode::BAD_REQUEST,
            "view limit reached for this workspace",
        );
    }
    if !SCOPE_TYPES.contains(&request.scope_type.as_str()) {
        return error_response(StatusCode::BAD_REQUEST, "invalid scope_type");
    }
    if request.visibility.is_empty() {
        request.visibility = "private".into();
    }
    if !VISIBILITIES.contains(&request.visibility.as_str()) {
        return error_response(StatusCode::BAD_REQUEST, "invalid visibility");
    }
    if request.definition_version <= 0 {
        request.definition_version = 1;
    }
    let query = match object_input(request.query, None) {
        Ok(query) => query,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "query must be a JSON object"),
    };
    let display = match object_input(request.display, Some(json!({}))) {
        Ok(display) => display,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "display must be a JSON object"),
    };
    let scope_variant =
        match validate_variant(&request.scope_type, request.scope_variant.as_deref()) {
            Ok(variant) => variant,
            Err(()) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid scope_variant for this scope_type",
                )
            }
        };
    let scope_id = match request.scope_type.as_str() {
        "project" => {
            let Some(raw_id) = request.scope_id.as_deref() else {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "scope_id is required for project views",
                );
            };
            let id = match Uuid::parse_str(raw_id) {
                Ok(id) => id,
                Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid scope_id"),
            };
            match project::get_project_in_workspace(&state.pool, id, context.member.workspace_id)
                .await
            {
                Ok(Some(_)) => Some(id),
                Ok(None) | Err(_) => {
                    return error_response(StatusCode::NOT_FOUND, "project not found")
                }
            }
        }
        "my" => {
            request.visibility = "private".into();
            None
        }
        _ => None,
    };
    match issue_view::create_issue_view(
        &state.pool,
        context.member.workspace_id,
        context.member.user_id,
        &request.name,
        &request.scope_type,
        scope_id,
        scope_variant.as_deref(),
        &request.visibility,
        request.definition_version,
        &query,
        &display,
    )
    .await
    {
        Ok(Some(view)) => {
            (StatusCode::CREATED, Json(IssueViewResponse::from(view))).into_response()
        }
        Ok(None) | Err(_) => {
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to create view")
        }
    }
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<ListParams>,
) -> Response {
    if !SCOPE_TYPES.contains(&params.scope_type.as_str()) {
        return error_response(StatusCode::BAD_REQUEST, "invalid scope_type");
    }
    let scope_id = match params.scope_id.as_deref().filter(|value| !value.is_empty()) {
        Some(raw_id) => match Uuid::parse_str(raw_id) {
            Ok(id) => Some(id),
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid scope_id"),
        },
        None => None,
    };
    match issue_view::list_issue_views_for_user(
        &state.pool,
        context.member.workspace_id,
        &params.scope_type,
        context.member.user_id,
        scope_id,
    )
    .await
    {
        Ok(views) => Json(
            views
                .into_iter()
                .map(IssueViewResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to list issue views");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list views")
        }
    }
}

async fn get_one(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    match load_for_user(&state, &context, &id).await {
        Ok(view) => Json(IssueViewResponse::from(view)).into_response(),
        Err(response) => response,
    }
}

async fn update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    let view = match load_for_user(&state, &context, &id).await {
        Ok(view) => view,
        Err(response) => return response,
    };
    if !can_manage(&state, &view, context.member.user_id).await {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    let body = match bounded_body(body).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let request: UpdateRequest = match decode(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.expected_revision <= 0 {
        return error_response(StatusCode::BAD_REQUEST, "expected_revision is required");
    }
    let name = match request.name {
        Some(name) => {
            if let Err(response) = validate_name(&name) {
                return response;
            }
            name
        }
        None => view.name.clone(),
    };
    let visibility = match request.visibility {
        Some(visibility) => {
            if !VISIBILITIES.contains(&visibility.as_str()) {
                return error_response(StatusCode::BAD_REQUEST, "invalid visibility");
            }
            if view.scope_type == "my" && visibility != "private" {
                return error_response(StatusCode::BAD_REQUEST, "my views are always private");
            }
            visibility
        }
        None => view.visibility.clone(),
    };
    let query = match request.query {
        JsonInput::Missing => view.query.clone(),
        input => match object_input(input, None) {
            Ok(query) => query,
            Err(()) => {
                return error_response(StatusCode::BAD_REQUEST, "query must be a JSON object")
            }
        },
    };
    let display = match request.display {
        JsonInput::Missing => view.display.clone(),
        input => match object_input(input, None) {
            Ok(display) => display,
            Err(()) => {
                return error_response(StatusCode::BAD_REQUEST, "display must be a JSON object")
            }
        },
    };
    let scope_variant = match request.scope_variant {
        Some(variant) => match validate_variant(&view.scope_type, Some(&variant)) {
            Ok(variant) => variant,
            Err(()) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid scope_variant for this scope_type",
                )
            }
        },
        None => view.scope_variant.clone(),
    };
    match issue_view::update_issue_view(
        &state.pool,
        view.id,
        context.member.workspace_id,
        &name,
        &visibility,
        scope_variant.as_deref(),
        &query,
        &display,
        request.expected_revision,
    )
    .await
    {
        Ok(Some(updated)) => Json(IssueViewResponse::from(updated)).into_response(),
        Ok(None) => error_response(StatusCode::CONFLICT, "view was modified by someone else"),
        Err(error) => {
            tracing::warn!(%error, view_id = %view.id, "failed to update issue view");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update view")
        }
    }
}

async fn delete(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let view = match load_for_user(&state, &context, &id).await {
        Ok(view) => view,
        Err(response) => return response,
    };
    if !can_manage(&state, &view, context.member.user_id).await {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    match issue_view::delete_issue_view(&state.pool, view.id, context.member.workspace_id).await {
        Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to delete view"),
        Err(error) => {
            tracing::warn!(%error, view_id = %view.id, "failed to delete issue view");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to delete view")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_matches_go_null_unknown_and_first_value_contract() {
        let request: CreateRequest = decode(
            br#"{"name":null,"scope_type":"workspace","definition_version":null,"query":{},"future":true} trailing"#,
        )
        .unwrap();
        assert!(request.name.is_empty());
        assert_eq!(request.definition_version, 0);
        assert!(matches!(request.query, JsonInput::Present(value) if value.is_object()));
    }

    #[test]
    fn variant_validation_matches_scope_contract() {
        assert_eq!(
            validate_variant("my", Some("assigned")),
            Ok(Some("assigned".into()))
        );
        assert!(validate_variant("my", None).is_err());
        assert_eq!(validate_variant("workspace", Some("all")), Ok(None));
        assert_eq!(
            validate_variant("project", Some("agents")),
            Ok(Some("agents".into()))
        );
        assert!(validate_variant("workspace", Some("assigned")).is_err());
    }

    #[test]
    fn object_inputs_distinguish_missing_from_explicit_null() {
        assert_eq!(
            object_input(JsonInput::Missing, Some(json!({}))),
            Ok(json!({}))
        );
        assert!(object_input(JsonInput::Missing, None).is_err());
        assert!(object_input(JsonInput::Present(Value::Null), Some(json!({}))).is_err());
    }

    #[test]
    fn response_preserves_nullable_scope_and_json_shapes() {
        let value = serde_json::to_value(IssueViewResponse {
            id: Uuid::nil().to_string(),
            workspace_id: Uuid::nil().to_string(),
            owner_id: Uuid::nil().to_string(),
            name: "Assigned".into(),
            scope_type: "my".into(),
            scope_id: None,
            scope_variant: Some("assigned".into()),
            visibility: "private".into(),
            definition_version: 1,
            query: json!({"status": ["open"]}),
            display: json!({}),
            revision: 1,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        })
        .unwrap();
        assert_eq!(value["scope_id"], Value::Null);
        assert!(value["query"].is_object());
    }

    #[tokio::test]
    async fn body_reader_enforces_128_kib_boundary_as_bad_request() {
        let response = bounded_body(Body::from(vec![b'x'; BODY_LIMIT + 1]))
            .await
            .unwrap_err();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            bounded_body(Body::from(vec![b'x'; BODY_LIMIT]))
                .await
                .unwrap()
                .len(),
            BODY_LIMIT
        );
    }
}
