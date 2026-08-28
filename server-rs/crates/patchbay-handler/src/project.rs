//! Workspace project read handlers.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use patchbay_db::models::{Project, ProjectResource};
use patchbay_db::queries::{project, project_resource};
use patchbay_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use url::Url;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const SEARCH_STATEMENT_TIMEOUT_MS: i64 = 3_000;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/projects/search", get(search))
        .route("/api/projects", get(list).post(create))
        .route("/api/projects/", get(list).post(create))
        .route(
            "/api/projects/{id}",
            get(get_one).put(update).delete(remove),
        )
        .route(
            "/api/projects/{id}/",
            get(get_one).put(update).delete(remove),
        )
        .route(
            "/api/projects/{id}/resources",
            get(list_resources).post(create_resource),
        )
        .route(
            "/api/projects/{id}/resources/",
            get(list_resources).post(create_resource),
        )
        .route(
            "/api/projects/{id}/resources/{resource_id}",
            axum::routing::put(update_resource).delete(remove_resource),
        )
        .route(
            "/api/projects/{id}/resources/{resource_id}/",
            axum::routing::put(update_resource).delete(remove_resource),
        )
}

#[derive(Debug, Serialize)]
struct ProjectResponse {
    id: String,
    workspace_id: String,
    title: String,
    description: Option<String>,
    icon: Option<String>,
    status: String,
    priority: String,
    lead_type: Option<String>,
    lead_id: Option<String>,
    start_date: Option<String>,
    due_date: Option<String>,
    created_at: String,
    updated_at: String,
    issue_count: i64,
    done_count: i64,
    resource_count: i64,
}

impl From<Project> for ProjectResponse {
    fn from(project: Project) -> Self {
        Self {
            id: project.id.to_string(),
            workspace_id: project.workspace_id.to_string(),
            title: project.title,
            description: project.description,
            icon: project.icon,
            status: project.status,
            priority: project.priority,
            lead_type: project.lead_type,
            lead_id: project.lead_id.map(|id| id.to_string()),
            start_date: project
                .start_date
                .map(|date| date.format("%Y-%m-%d").to_string()),
            due_date: project
                .due_date
                .map(|date| date.format("%Y-%m-%d").to_string()),
            created_at: crate::timefmt::rfc3339(project.created_at),
            updated_at: crate::timefmt::rfc3339(project.updated_at),
            issue_count: 0,
            done_count: 0,
            resource_count: 0,
        }
    }
}

#[derive(Debug, Serialize)]
struct ProjectResourceResponse {
    id: String,
    project_id: String,
    workspace_id: String,
    resource_type: String,
    resource_ref: Value,
    label: Option<String>,
    position: i32,
    created_at: String,
    created_by: Option<String>,
}

impl From<ProjectResource> for ProjectResourceResponse {
    fn from(resource: ProjectResource) -> Self {
        Self {
            id: resource.id.to_string(),
            project_id: resource.project_id.to_string(),
            workspace_id: resource.workspace_id.to_string(),
            resource_type: resource.resource_type,
            resource_ref: resource.resource_ref,
            label: resource.label,
            position: resource.position,
            created_at: crate::timefmt::rfc3339(resource.created_at),
            created_by: resource.created_by.map(|id| id.to_string()),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CreateProjectRequest {
    #[serde(deserialize_with = "null_default")]
    title: String,
    description: Option<String>,
    icon: Option<String>,
    #[serde(deserialize_with = "null_default")]
    status: String,
    #[serde(deserialize_with = "null_default")]
    priority: String,
    lead_type: Option<String>,
    lead_id: Option<String>,
    start_date: Option<String>,
    due_date: Option<String>,
    #[serde(deserialize_with = "null_default")]
    resources: Vec<CreateResourceRequest>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CreateResourceRequest {
    #[serde(deserialize_with = "null_default")]
    resource_type: String,
    resource_ref: ResourceRefInput,
    label: Option<String>,
    position: Option<i32>,
}

#[derive(Debug, Default)]
struct ResourceRefInput {
    present: bool,
    value: Value,
}

impl<'de> Deserialize<'de> for ResourceRefInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self {
            present: true,
            value: Value::deserialize(deserializer)?,
        })
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct GithubRepoRef {
    #[serde(deserialize_with = "null_default")]
    url: String,
    #[serde(
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    default_branch_hint: String,
    #[serde(
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    r#ref: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct LocalDirectoryRef {
    #[serde(deserialize_with = "null_default")]
    local_path: String,
    #[serde(deserialize_with = "null_default")]
    daemon_id: String,
    #[serde(
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    label: String,
    #[serde(
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    execution_mode: String,
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

fn decode_first<T: serde::de::DeserializeOwned + Default>(body: &[u8]) -> Result<T, ()> {
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let value = Value::deserialize(&mut deserializer).map_err(|_| ())?;
    if value.is_null() {
        Ok(T::default())
    } else {
        serde_json::from_value(value).map_err(|_| ())
    }
}

fn is_valid_git_repo_url(value: &str) -> bool {
    if let Ok(url) = Url::parse(value) {
        if url.host_str().is_some() && matches!(url.scheme(), "http" | "https" | "ssh" | "git") {
            return true;
        }
    }
    if value.contains(' ') || value.contains("://") {
        return false;
    }
    let Some(colon) = value.find(':') else {
        return false;
    };
    if colon == 0 || colon + 1 == value.len() {
        return false;
    }
    let at = value.find('@');
    if at.is_some_and(|at| at >= colon) {
        return false;
    }
    let host_start = at.map_or(0, |at| at + 1);
    !value[host_start..colon].is_empty() && !value[colon + 1..].is_empty()
}

fn is_absolute_local_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.starts_with('/')
        || value.starts_with(r"\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'))
}

fn normalize_resource_ref(resource_type: &str, raw: &ResourceRefInput) -> Result<Value, String> {
    if !raw.present {
        return Err("resource_ref is required".into());
    }
    normalize_resource_value(resource_type, raw.value.clone())
}

fn normalize_resource_value(resource_type: &str, raw: Value) -> Result<Value, String> {
    match resource_type {
        "github_repo" => {
            let mut value: GithubRepoRef = if raw.is_null() {
                GithubRepoRef::default()
            } else {
                serde_json::from_value(raw)
                    .map_err(|error| format!("invalid github_repo payload: {error}"))?
            };
            value.url = value.url.trim().to_string();
            if value.url.is_empty() {
                return Err("github_repo: url is required".into());
            }
            if !is_valid_git_repo_url(&value.url) {
                return Err("github_repo: url must be a valid http(s) or ssh git URL".into());
            }
            value.default_branch_hint = value.default_branch_hint.trim().to_string();
            value.r#ref = value.r#ref.trim().to_string();
            serde_json::to_value(value).map_err(|error| error.to_string())
        }
        "local_directory" => {
            let mut value: LocalDirectoryRef = if raw.is_null() {
                LocalDirectoryRef::default()
            } else {
                serde_json::from_value(raw)
                    .map_err(|error| format!("invalid local_directory payload: {error}"))?
            };
            value.local_path = value.local_path.trim().to_string();
            if value.local_path.is_empty() {
                return Err("local_directory: local_path is required".into());
            }
            if !is_absolute_local_path(&value.local_path) {
                return Err("local_directory: local_path must be an absolute path".into());
            }
            value.daemon_id = value.daemon_id.trim().to_string();
            if value.daemon_id.is_empty() {
                return Err("local_directory: daemon_id is required".into());
            }
            value.label = value.label.trim().to_string();
            value.execution_mode = value.execution_mode.trim().to_string();
            if !matches!(value.execution_mode.as_str(), "" | "in_place" | "worktree") {
                return Err(format!(
                    "local_directory: execution_mode must be {:?} or {:?}, got {:?}",
                    "in_place", "worktree", value.execution_mode
                ));
            }
            serde_json::to_value(value).map_err(|error| error.to_string())
        }
        _ => Err(format!("unknown resource_type {resource_type:?}")),
    }
}

fn local_directory_ref(value: &Value) -> Option<LocalDirectoryRef> {
    serde_json::from_value(value.clone()).ok()
}

fn local_directory_label(value: &Value) -> String {
    local_directory_ref(value)
        .map(|value| value.label.trim().to_string())
        .unwrap_or_default()
}

fn differs_only_by_local_directory_label(left: &Value, right: &Value) -> bool {
    let (Some(mut left), Some(mut right)) = (left.as_object().cloned(), right.as_object().cloned())
    else {
        return false;
    };
    left.remove("label");
    right.remove("label");
    left == right
}

fn with_local_directory_label(value: &Value, label: Option<&str>) -> Result<Value, ()> {
    let mut fields = value.as_object().cloned().ok_or(())?;
    if let Some(label) = label {
        fields.insert("label".into(), Value::String(label.into()));
    } else {
        fields.remove("label");
    }
    Ok(Value::Object(fields))
}

fn runtime_has_worktree(runtime: &patchbay_db::models::AgentRuntime) -> bool {
    runtime
        .metadata
        .get("capabilities")
        .and_then(Value::as_array)
        .is_some_and(|values| {
            values.iter().any(|value| {
                value.as_str() == Some(patchbay_protocol::DAEMON_CAPABILITY_LOCAL_WORKTREE_V1)
            })
        })
}

fn newest_daemon_runtime<'a>(
    runtimes: &'a [patchbay_db::models::AgentRuntime],
    daemon_id: &str,
) -> Option<&'a patchbay_db::models::AgentRuntime> {
    runtimes
        .iter()
        .filter(|runtime| runtime.daemon_id.as_deref() == Some(daemon_id))
        .reduce(
            |current, candidate| match (current.last_seen_at, candidate.last_seen_at) {
                (None, Some(_)) => candidate,
                (Some(current), Some(candidate_seen)) if candidate_seen > current => candidate,
                _ => current,
            },
        )
}

fn latest_daemon_cli_version(
    runtimes: &[patchbay_db::models::AgentRuntime],
    daemon_id: &str,
) -> String {
    runtimes
        .iter()
        .filter(|runtime| runtime.daemon_id.as_deref() == Some(daemon_id))
        .filter_map(|runtime| {
            runtime
                .metadata
                .get("cli_version")
                .and_then(Value::as_str)
                .filter(|version| !version.is_empty())
                .map(|version| (runtime.last_seen_at, version))
        })
        .reduce(|current, candidate| match (current.0, candidate.0) {
            (None, Some(_)) => candidate,
            (Some(current_seen), Some(candidate_seen)) if candidate_seen > current_seen => {
                candidate
            }
            _ => current,
        })
        .map(|(_, version)| version.to_string())
        .unwrap_or_default()
}

async fn require_worktree_capability(
    state: &HandlerState,
    workspace_id: Uuid,
    resource_type: &str,
    resource_ref: &Value,
) -> Result<(), Response> {
    if resource_type != "local_directory" {
        return Ok(());
    }
    let Some(reference) = local_directory_ref(resource_ref) else {
        return Ok(());
    };
    if reference.execution_mode != "worktree" {
        return Ok(());
    }
    let runtimes = patchbay_db::queries::runtime::list_agent_runtimes(&state.pool, workspace_id)
        .await
        .map_err(|error| {
            tracing::warn!(%error, "failed to check runtime capabilities");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to check runtime capabilities",
            )
        })?;
    if newest_daemon_runtime(&runtimes, &reference.daemon_id).is_some_and(runtime_has_worktree) {
        return Ok(());
    }
    Err((
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({
            "error": format!(
                "local_directory: {:?} is set to parallel (worktree) mode, but the Patchbay runtime on that machine does not support it. Update the Patchbay app on that machine to the latest version, or keep the resource on in_place.",
                reference.local_path
            ),
            "code": "daemon_version_unsupported",
            "current_version": latest_daemon_cli_version(&runtimes, &reference.daemon_id),
            "min_version": "0.4.24",
            "daemon_id": reference.daemon_id,
        })),
    )
        .into_response())
}

async fn local_directory_conflict(
    state: &HandlerState,
    project_id: Uuid,
    resource_type: &str,
    resource_ref: &Value,
    exclude_id: Option<Uuid>,
) -> anyhow::Result<bool> {
    if resource_type != "local_directory" {
        return Ok(false);
    }
    let Some(incoming) = local_directory_ref(resource_ref) else {
        return Ok(false);
    };
    let resources = project_resource::list_project_resources(&state.pool, project_id).await?;
    Ok(resources.into_iter().any(|resource| {
        resource.id != exclude_id.unwrap_or(Uuid::nil())
            && resource.resource_type == "local_directory"
            && local_directory_ref(&resource.resource_ref)
                .is_some_and(|existing| existing.daemon_id == incoming.daemon_id)
    }))
}

#[derive(Default, Deserialize)]
struct ListQuery {
    status: Option<String>,
    priority: Option<String>,
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(query): Query<ListQuery>,
) -> Response {
    let projects = match project::list_projects(
        &state.pool,
        context.member.workspace_id,
        query.status.as_deref().filter(|value| !value.is_empty()),
        query.priority.as_deref().filter(|value| !value.is_empty()),
    )
    .await
    {
        Ok(projects) => projects,
        Err(error) => {
            tracing::warn!(%error, workspace_id = %context.workspace_id, "failed to list projects");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list projects");
        }
    };
    let ids = projects
        .iter()
        .map(|project| project.id)
        .collect::<Vec<_>>();
    let (stats, counts) = project_enrichment(&state, &ids).await;
    let response = projects
        .into_iter()
        .map(|project| enrich(ProjectResponse::from(project), &stats, &counts))
        .collect::<Vec<_>>();
    Json(serde_json::json!({ "projects": response, "total": response.len() })).into_response()
}

async fn create(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    body: Bytes,
) -> Response {
    let mut request = match decode_first::<CreateProjectRequest>(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.title.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "title is required");
    }
    if request.status.is_empty() {
        request.status = "planned".into();
    }
    if let Err(message) = validate_enum("status", &request.status, PROJECT_STATUSES) {
        return error_response(StatusCode::BAD_REQUEST, &message);
    }
    if request.priority.is_empty() {
        request.priority = "none".into();
    }
    if let Err(message) = validate_enum("priority", &request.priority, PROJECT_PRIORITIES) {
        return error_response(StatusCode::BAD_REQUEST, &message);
    }
    let lead_id = match request.lead_id.as_deref() {
        Some(raw) => match Uuid::parse_str(raw) {
            Ok(id) => Some(id),
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid lead_id"),
        },
        None => None,
    };
    let start_date = match request.start_date.as_deref() {
        Some(value) => match calendar_date(value, "start_date") {
            Ok(date) => date,
            Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
        },
        None => None,
    };
    let due_date = match request.due_date.as_deref() {
        Some(value) => match calendar_date(value, "due_date") {
            Ok(date) => date,
            Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
        },
        None => None,
    };

    let mut normalized = Vec::with_capacity(request.resources.len());
    let mut local_daemons = HashMap::new();
    for (index, resource) in request.resources.iter_mut().enumerate() {
        resource.resource_type = resource.resource_type.trim().to_string();
        if resource.resource_type.is_empty() {
            return error_response(
                StatusCode::BAD_REQUEST,
                "resources[].resource_type is required",
            );
        }
        let resource_ref =
            match normalize_resource_ref(&resource.resource_type, &resource.resource_ref) {
                Ok(resource_ref) => resource_ref,
                Err(message) => {
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &format!("resources[{index}]: {message}"),
                    )
                }
            };
        if resource.resource_type == "local_directory" {
            let reference = local_directory_ref(&resource_ref).expect("normalized local directory");
            if let Some(previous) = local_daemons.insert(reference.daemon_id.clone(), index) {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &format!(
                        "resources[{index}]: duplicate local_directory for daemon (already at index {previous}); each daemon may attach at most one local_directory per project"
                    ),
                );
            }
            if let Err(response) = require_worktree_capability(
                &state,
                context.member.workspace_id,
                &resource.resource_type,
                &resource_ref,
            )
            .await
            {
                return response;
            }
        }
        normalized.push(resource_ref);
    }

    if request.resources.is_empty() {
        let project = match project::create_project(
            &state.pool,
            context.member.workspace_id,
            &request.title,
            request.description.as_deref(),
            request.icon.as_deref(),
            &request.status,
            request.lead_type.as_deref(),
            lead_id,
            &request.priority,
            start_date,
            due_date,
        )
        .await
        {
            Ok(Some(project)) => project,
            Ok(None) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create project",
                )
            }
            Err(error) if check_violation(&error) => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "project create rejected: a field value failed a database constraint",
                )
            }
            Err(error) => {
                tracing::warn!(%error, "project create failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create project",
                );
            }
        };
        let response = ProjectResponse::from(project);
        publish_project(
            &state,
            &context,
            patchbay_protocol::EVENT_PROJECT_CREATED,
            json!({ "project": &response }),
        );
        return (StatusCode::CREATED, Json(response)).into_response();
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "failed to start project create transaction");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to start transaction",
            );
        }
    };
    let project = match project::create_project(
        &mut *transaction,
        context.member.workspace_id,
        &request.title,
        request.description.as_deref(),
        request.icon.as_deref(),
        &request.status,
        request.lead_type.as_deref(),
        lead_id,
        &request.priority,
        start_date,
        due_date,
    )
    .await
    {
        Ok(Some(project)) => project,
        Ok(None) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create project",
            )
        }
        Err(error) if check_violation(&error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "project create rejected: a field value failed a database constraint",
            )
        }
        Err(error) => {
            tracing::warn!(%error, "project create failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create project",
            );
        }
    };
    let mut resources = Vec::with_capacity(request.resources.len());
    for (index, (resource, resource_ref)) in
        request.resources.iter().zip(normalized.iter()).enumerate()
    {
        let label = resource
            .label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty());
        let position = resource.position.unwrap_or(index as i32);
        let created = match project_resource::create_project_resource(
            &mut *transaction,
            project.id,
            project.workspace_id,
            &resource.resource_type,
            resource_ref,
            label,
            position,
            context.member.user_id,
        )
        .await
        {
            Ok(Some(resource)) => resource,
            Err(error) if unique_violation(&error) => {
                return error_response(
                    StatusCode::CONFLICT,
                    &format!("resources[{index}]: this resource is already attached"),
                )
            }
            Ok(None) | Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("failed to attach resource at index {index}"),
                )
            }
        };
        resources.push(ProjectResourceResponse::from(created));
    }
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, "failed to commit project create");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to commit project create",
        );
    }
    let mut response = ProjectResponse::from(project);
    response.resource_count = resources.len() as i64;
    publish_project(
        &state,
        &context,
        patchbay_protocol::EVENT_PROJECT_CREATED,
        json!({ "project": &response }),
    );
    for resource in &resources {
        publish_project(
            &state,
            &context,
            patchbay_protocol::EVENT_PROJECT_RESOURCE_CREATED,
            json!({ "resource": resource, "project_id": response.id }),
        );
    }
    let mut value = serde_json::to_value(response).expect("project response serializes");
    value
        .as_object_mut()
        .expect("project response is an object")
        .insert(
            "resources".into(),
            serde_json::to_value(resources).expect("resource responses serialize"),
        );
    (StatusCode::CREATED, Json(value)).into_response()
}

async fn get_one(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let id = match Uuid::parse_str(raw_id.trim()) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid project id"),
    };
    let found =
        match project::get_project_in_workspace(&state.pool, id, context.member.workspace_id).await
        {
            Ok(Some(project)) => project,
            Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "project not found"),
        };
    let (stats, counts) = project_enrichment(&state, &[found.id]).await;
    Json(enrich(ProjectResponse::from(found), &stats, &counts)).into_response()
}

const PROJECT_STATUSES: &[&str] = &["planned", "in_progress", "paused", "completed", "cancelled"];
const PROJECT_PRIORITIES: &[&str] = &["urgent", "high", "medium", "low", "none"];

#[derive(Default, Deserialize)]
struct UpdateRequest {
    title: Option<String>,
    description: Option<String>,
    icon: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    lead_type: Option<String>,
    lead_id: Option<String>,
    start_date: Option<String>,
    due_date: Option<String>,
}

fn decode_update(body: &[u8]) -> Result<(UpdateRequest, Map<String, Value>), ()> {
    let value = serde_json::from_slice::<Value>(body).map_err(|_| ())?;
    match value {
        Value::Object(fields) => {
            let request = serde_json::from_value(Value::Object(fields.clone())).map_err(|_| ())?;
            Ok((request, fields))
        }
        Value::Null => Ok((UpdateRequest::default(), Map::new())),
        _ => Err(()),
    }
}

fn validate_enum(field: &str, value: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "invalid {field} {value:?}; valid values: {}",
            allowed.join(", ")
        ))
    }
}

fn calendar_date(value: &str, field: &str) -> Result<Option<chrono::NaiveDate>, String> {
    if value.is_empty() {
        return Ok(None);
    }
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(Some)
        .map_err(|_| format!("invalid {field} format, expected YYYY-MM-DD"))
}

fn check_violation(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23514")
}

fn unique_violation(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23505")
}

fn publish_project(
    state: &HandlerState,
    context: &WorkspaceContext,
    event_type: &str,
    payload: Value,
) {
    state.bus.publish(&patchbay_events::Event {
        event_type: event_type.into(),
        workspace_id: context.member.workspace_id.to_string(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload,
        ..Default::default()
    });
}

async fn update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    body: Bytes,
) -> Response {
    let id = match Uuid::parse_str(raw_id.trim()) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid project id"),
    };
    let existing =
        match project::get_project_in_workspace(&state.pool, id, context.member.workspace_id).await
        {
            Ok(Some(project)) => project,
            Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "project not found"),
        };
    let (request, fields) = match decode_update(&body) {
        Ok(decoded) => decoded,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if let Some(status) = request.status.as_deref() {
        if let Err(message) = validate_enum("status", status, PROJECT_STATUSES) {
            return error_response(StatusCode::BAD_REQUEST, &message);
        }
    }
    if let Some(priority) = request.priority.as_deref() {
        if let Err(message) = validate_enum("priority", priority, PROJECT_PRIORITIES) {
            return error_response(StatusCode::BAD_REQUEST, &message);
        }
    }
    let description = if fields.contains_key("description") {
        request.description.as_deref()
    } else {
        existing.description.as_deref()
    };
    let icon = if fields.contains_key("icon") {
        request.icon.as_deref()
    } else {
        existing.icon.as_deref()
    };
    let lead_type = if fields.contains_key("lead_type") {
        request.lead_type.as_deref()
    } else {
        existing.lead_type.as_deref()
    };
    let lead_id = if fields.contains_key("lead_id") {
        match request.lead_id.as_deref() {
            Some(raw) => match Uuid::parse_str(raw) {
                Ok(id) => Some(id),
                Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid lead_id"),
            },
            None => None,
        }
    } else {
        existing.lead_id
    };
    let start_date = if fields.contains_key("start_date") {
        match request.start_date.as_deref() {
            Some(value) => match calendar_date(value, "start_date") {
                Ok(date) => date,
                Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
            },
            None => None,
        }
    } else {
        existing.start_date
    };
    let due_date = if fields.contains_key("due_date") {
        match request.due_date.as_deref() {
            Some(value) => match calendar_date(value, "due_date") {
                Ok(date) => date,
                Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
            },
            None => None,
        }
    } else {
        existing.due_date
    };
    let updated = match project::update_project(
        &state.pool,
        existing.id,
        context.member.workspace_id,
        request.title.as_deref(),
        description,
        icon,
        request.status.as_deref(),
        request.priority.as_deref(),
        lead_type,
        lead_id,
        start_date,
        due_date,
    )
    .await
    {
        Ok(Some(project)) => project,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "project not found"),
        Err(error) if check_violation(&error) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "project update rejected: a field value failed a database constraint",
            )
        }
        Err(error) => {
            tracing::warn!(%error, %id, "project update failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update project",
            );
        }
    };
    let (stats, counts) = project_enrichment(&state, &[updated.id]).await;
    let response = enrich(ProjectResponse::from(updated), &stats, &counts);
    publish_project(
        &state,
        &context,
        patchbay_protocol::EVENT_PROJECT_UPDATED,
        json!({ "project": &response }),
    );
    Json(response).into_response()
}

async fn remove(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let id = match Uuid::parse_str(raw_id.trim()) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid project id"),
    };
    match project::get_project_in_workspace(&state.pool, id, context.member.workspace_id).await {
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => return error_response(StatusCode::NOT_FOUND, "project not found"),
    }
    if !matches!(context.member.role.as_str(), "owner" | "admin") {
        return error_response(StatusCode::FORBIDDEN, "insufficient workspace role");
    }
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, %id, "failed to begin project delete");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to start transaction",
            );
        }
    };
    match project::lock_project_for_delete(&mut *transaction, id, context.member.workspace_id).await
    {
        Ok(Some(_)) => {}
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "project not found"),
        Err(error) => {
            tracing::warn!(%error, %id, "failed to lock project");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to lock project");
        }
    }
    if let Err(error) = patchbay_db::queries::chat::clear_chat_session_project_by_project(
        &mut *transaction,
        id,
        context.member.workspace_id,
    )
    .await
    {
        tracing::warn!(%error, %id, "failed to clear project chat context");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to clear project chat context",
        );
    }
    if let Err(error) = patchbay_db::queries::issue_view::delete_issue_views_by_project_scope(
        &mut *transaction,
        context.member.workspace_id,
        id,
    )
    .await
    {
        tracing::warn!(%error, %id, "failed to delete project views");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to delete project views",
        );
    }
    match project::delete_project(&mut *transaction, id, context.member.workspace_id).await {
        Ok(1) => {}
        Ok(_) => return error_response(StatusCode::NOT_FOUND, "project not found"),
        Err(error) => {
            tracing::warn!(%error, %id, "failed to delete project");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete project",
            );
        }
    }
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, %id, "failed to commit project delete");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to commit project delete",
        );
    }
    publish_project(
        &state,
        &context,
        patchbay_protocol::EVENT_PROJECT_DELETED,
        json!({ "project_id": id.to_string() }),
    );
    StatusCode::NO_CONTENT.into_response()
}

async fn load_project_for_resource(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_id: &str,
) -> Result<Project, Response> {
    let id = Uuid::parse_str(raw_id.trim())
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid project id"))?;
    match project::get_project_in_workspace(&state.pool, id, context.member.workspace_id).await {
        Ok(Some(project)) => Ok(project),
        Ok(None) | Err(_) => Err(error_response(StatusCode::NOT_FOUND, "project not found")),
    }
}

async fn list_resources(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
) -> Response {
    let project = match load_project_for_resource(&state, &context, &raw_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let resources = match project_resource::list_project_resources(&state.pool, project.id).await {
        Ok(resources) => resources,
        Err(error) => {
            tracing::warn!(%error, project_id = %project.id, "failed to list project resources");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list project resources",
            );
        }
    };
    let response = resources
        .into_iter()
        .map(ProjectResourceResponse::from)
        .collect::<Vec<_>>();
    Json(json!({ "resources": response, "total": response.len() })).into_response()
}

async fn create_resource(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_id): Path<String>,
    body: Bytes,
) -> Response {
    let project = match load_project_for_resource(&state, &context, &raw_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let mut request = match decode_first::<CreateResourceRequest>(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    request.resource_type = request.resource_type.trim().to_string();
    if request.resource_type.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "resource_type is required");
    }
    let resource_ref = match normalize_resource_ref(&request.resource_type, &request.resource_ref) {
        Ok(resource_ref) => resource_ref,
        Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
    };
    match local_directory_conflict(
        &state,
        project.id,
        &request.resource_type,
        &resource_ref,
        None,
    )
    .await
    {
        Ok(false) => {}
        Ok(true) => {
            return error_response(
                StatusCode::CONFLICT,
                "this daemon already has a local_directory attached to the project; remove it before adding another",
            )
        }
        Err(error) => {
            tracing::warn!(%error, project_id = %project.id, "failed to check existing resources");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to check existing resources",
            );
        }
    }
    if let Err(response) = require_worktree_capability(
        &state,
        project.workspace_id,
        &request.resource_type,
        &resource_ref,
    )
    .await
    {
        return response;
    }
    let label = request
        .label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty());
    let position = match request.position {
        Some(position) => position,
        None => project_resource::count_project_resources(&state.pool, project.id)
            .await
            .ok()
            .flatten()
            .unwrap_or_default() as i32,
    };
    let created = match project_resource::create_project_resource(
        &state.pool,
        project.id,
        project.workspace_id,
        &request.resource_type,
        &resource_ref,
        label,
        position,
        context.member.user_id,
    )
    .await
    {
        Ok(Some(resource)) => resource,
        Err(error) if unique_violation(&error) => {
            return error_response(
                StatusCode::CONFLICT,
                "this resource is already attached to the project",
            )
        }
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create project resource",
            )
        }
    };
    let response = ProjectResourceResponse::from(created);
    publish_project(
        &state,
        &context,
        patchbay_protocol::EVENT_PROJECT_RESOURCE_CREATED,
        json!({ "resource": &response, "project_id": project.id.to_string() }),
    );
    (StatusCode::CREATED, Json(response)).into_response()
}

fn decode_first_map(body: &[u8]) -> Result<Map<String, Value>, ()> {
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    Option::<Map<String, Value>>::deserialize(&mut deserializer)
        .map(|fields| fields.unwrap_or_default())
        .map_err(|_| ())
}

async fn update_resource(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_project_id, raw_resource_id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let project = match load_project_for_resource(&state, &context, &raw_project_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let resource_id = match Uuid::parse_str(raw_resource_id.trim()) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid resource id"),
    };
    let existing = match project_resource::get_project_resource_in_workspace(
        &state.pool,
        resource_id,
        context.member.workspace_id,
    )
    .await
    {
        Ok(Some(resource)) if resource.project_id == project.id => resource,
        Ok(Some(_)) | Ok(None) | Err(_) => {
            return error_response(StatusCode::NOT_FOUND, "project resource not found")
        }
    };
    let fields = match decode_first_map(&body) {
        Ok(fields) => fields,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let ref_provided = fields.contains_key("resource_ref");
    let mut next_ref = existing.resource_ref.clone();
    if let Some(raw_ref) = fields.get("resource_ref") {
        next_ref = match normalize_resource_value(&existing.resource_type, raw_ref.clone()) {
            Ok(resource_ref) => resource_ref,
            Err(message) => return error_response(StatusCode::BAD_REQUEST, &message),
        };
    }
    match local_directory_conflict(
        &state,
        project.id,
        &existing.resource_type,
        &next_ref,
        Some(existing.id),
    )
    .await
    {
        Ok(false) => {}
        Ok(true) => {
            return error_response(
                StatusCode::CONFLICT,
                "another local_directory on this daemon is already attached to the project",
            )
        }
        Err(error) => {
            tracing::warn!(%error, %resource_id, "failed to check existing resources");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to check existing resources",
            );
        }
    }
    let rename_only = ref_provided
        && existing.resource_type == "local_directory"
        && differs_only_by_local_directory_label(&next_ref, &existing.resource_ref);
    if ref_provided && !rename_only {
        if let Err(response) = require_worktree_capability(
            &state,
            project.workspace_id,
            &existing.resource_type,
            &next_ref,
        )
        .await
        {
            return response;
        }
    }
    let mut next_label = existing.label.clone();
    let mut label_cleared = false;
    if let Some(raw_label) = fields.get("label") {
        match raw_label {
            Value::Null => {
                next_label = None;
                label_cleared = true;
            }
            Value::String(label) => {
                let label = label.trim();
                if label.is_empty() {
                    next_label = None;
                    label_cleared = true;
                } else {
                    next_label = Some(label.to_string());
                }
            }
            _ => return error_response(StatusCode::BAD_REQUEST, "label must be a string or null"),
        }
    } else if rename_only {
        let next = local_directory_label(&next_ref);
        if next != local_directory_label(&existing.resource_ref) {
            if next.is_empty() {
                next_label = None;
                label_cleared = true;
            } else {
                next_label = Some(next);
            }
        }
    }
    let mut next_position = existing.position;
    if let Some(raw_position) = fields.get("position") {
        match serde_json::from_value::<Option<i32>>(raw_position.clone()) {
            Ok(Some(position)) => next_position = position,
            Ok(None) => {}
            Err(_) => {
                return error_response(StatusCode::BAD_REQUEST, "position must be an integer")
            }
        }
    }
    if existing.resource_type == "local_directory" {
        let mut name = next_label.clone();
        if name.is_none() && !label_cleared {
            let stored = local_directory_label(&existing.resource_ref);
            if !stored.is_empty() {
                name = Some(stored);
            }
        }
        if ref_provided || next_label.is_some() || label_cleared {
            next_ref = match with_local_directory_label(&next_ref, name.as_deref()) {
                Ok(resource_ref) => resource_ref,
                Err(()) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to update project resource",
                    )
                }
            };
        }
    }
    let updated = match project_resource::update_project_resource(
        &state.pool,
        existing.id,
        &next_ref,
        next_label.as_deref(),
        next_position,
    )
    .await
    {
        Ok(Some(resource)) => resource,
        Err(error) if unique_violation(&error) => {
            return error_response(
                StatusCode::CONFLICT,
                "this resource is already attached to the project",
            )
        }
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "project resource not found"),
        Err(error) => {
            tracing::warn!(%error, %resource_id, "failed to update project resource");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update project resource",
            );
        }
    };
    let response = ProjectResourceResponse::from(updated);
    publish_project(
        &state,
        &context,
        patchbay_protocol::EVENT_PROJECT_RESOURCE_UPDATED,
        json!({ "resource": &response, "project_id": project.id.to_string() }),
    );
    Json(response).into_response()
}

async fn remove_resource(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_project_id, raw_resource_id)): Path<(String, String)>,
) -> Response {
    let project = match load_project_for_resource(&state, &context, &raw_project_id).await {
        Ok(project) => project,
        Err(response) => return response,
    };
    let resource_id = match Uuid::parse_str(raw_resource_id.trim()) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid resource id"),
    };
    let resource = match project_resource::get_project_resource_in_workspace(
        &state.pool,
        resource_id,
        context.member.workspace_id,
    )
    .await
    {
        Ok(Some(resource)) if resource.project_id == project.id => resource,
        Ok(Some(_)) | Ok(None) | Err(_) => {
            return error_response(StatusCode::NOT_FOUND, "project resource not found")
        }
    };
    match project_resource::delete_project_resource(&state.pool, resource.id).await {
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, %resource_id, "failed to delete project resource");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete project resource",
            );
        }
    }
    publish_project(
        &state,
        &context,
        patchbay_protocol::EVENT_PROJECT_RESOURCE_DELETED,
        json!({
            "project_id": project.id.to_string(),
            "resource_id": resource.id.to_string(),
        }),
    );
    StatusCode::NO_CONTENT.into_response()
}

async fn project_enrichment(
    state: &HandlerState,
    ids: &[Uuid],
) -> (HashMap<Uuid, (i64, i64)>, HashMap<Uuid, i64>) {
    if ids.is_empty() {
        return (HashMap::new(), HashMap::new());
    }
    let stats = project::get_project_issue_stats(&state.pool, ids.to_vec())
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            row.project_id
                .map(|id| (id, (row.total_count, row.done_count)))
        })
        .collect();
    let counts = project_resource::get_project_resource_counts(&state.pool, ids.to_vec())
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| row.project_id.map(|id| (id, row.resource_count)))
        .collect();
    (stats, counts)
}

fn enrich(
    mut response: ProjectResponse,
    stats: &HashMap<Uuid, (i64, i64)>,
    counts: &HashMap<Uuid, i64>,
) -> ProjectResponse {
    let id = Uuid::parse_str(&response.id).ok();
    if let Some((total, done)) = id.and_then(|id| stats.get(&id)) {
        response.issue_count = *total;
        response.done_count = *done;
    }
    response.resource_count = id.and_then(|id| counts.get(&id).copied()).unwrap_or(0);
    response
}

#[derive(Default, Deserialize)]
struct SearchQuery {
    q: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
    include_closed: Option<String>,
}

#[derive(Serialize)]
struct SearchProjectResponse {
    #[serde(flatten)]
    project: ProjectResponse,
    match_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    matched_snippet: Option<String>,
}

async fn search(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(query): Query<SearchQuery>,
) -> Response {
    let phrase = match query.q {
        Some(phrase) if !phrase.is_empty() => phrase,
        _ => return error_response(StatusCode::BAD_REQUEST, "q parameter is required"),
    };
    let limit = parse_positive(&query.limit, 20).min(50);
    let offset = parse_non_negative(&query.offset, 0);
    let include_closed = query.include_closed.as_deref() == Some("true");
    let escaped_phrase = escape_like(&simple_lowercase(&phrase));
    let terms = phrase
        .split_whitespace()
        .map(|term| escape_like(&simple_lowercase(term)))
        .collect::<Vec<_>>();

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "failed to begin project search");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to search projects",
            );
        }
    };
    if sqlx::query(&format!(
        "SET LOCAL statement_timeout = {SEARCH_STATEMENT_TIMEOUT_MS}"
    ))
    .execute(&mut *transaction)
    .await
    .is_err()
        || sqlx::query("SET LOCAL transaction_read_only = on")
            .execute(&mut *transaction)
            .await
            .is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to search projects",
        );
    }
    let rows = match project::search_projects(
        &mut *transaction,
        context.member.workspace_id,
        &escaped_phrase,
        &terms,
        include_closed,
        limit,
        offset,
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) if statement_timeout(&error) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "search timed out; please refine your query or try again",
            )
        }
        Err(error) => {
            tracing::warn!(%error, workspace_id = %context.workspace_id, query = %phrase, "search projects failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to search projects",
            );
        }
    };
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, "failed to commit project search");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to search projects",
        );
    }

    let total = rows.first().map(|row| row.total_count).unwrap_or_default();
    let ids = rows.iter().map(|row| row.project.id).collect::<Vec<_>>();
    let (stats, counts) = project_enrichment(&state, &ids).await;
    let response = rows
        .into_iter()
        .map(|row| {
            let matched_snippet = (row.match_source == "description")
                .then_some(row.project.description.as_deref())
                .flatten()
                .filter(|description| !description.is_empty())
                .map(|description| extract_snippet(description, &phrase));
            SearchProjectResponse {
                project: enrich(ProjectResponse::from(row.project), &stats, &counts),
                match_source: row.match_source,
                matched_snippet,
            }
        })
        .collect::<Vec<_>>();
    let mut result =
        Json(serde_json::json!({ "projects": response, "total": total })).into_response();
    if let Ok(value) = HeaderValue::from_str(&total.to_string()) {
        result.headers_mut().insert("x-total-count", value);
    }
    result
}

fn parse_positive(value: &Option<String>, default: i64) -> i64 {
    value
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn parse_non_negative(value: &Option<String>, default: i64) -> i64 {
    value
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(default)
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn simple_lower_chars(value: &str) -> Vec<char> {
    value
        .chars()
        .map(|character| character.to_lowercase().next().unwrap_or(character))
        .collect()
}

fn simple_lowercase(value: &str) -> String {
    simple_lower_chars(value).into_iter().collect()
}

fn statement_timeout(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(|error| error.as_database_error())
        .and_then(|error| error.code())
        .is_some_and(|code| code == "57014")
}

fn extract_snippet(content: &str, query: &str) -> String {
    let content_chars = content.chars().collect::<Vec<_>>();
    let lower_chars = simple_lower_chars(content);
    let query_chars = simple_lower_chars(query);
    let mut found = find_chars(&lower_chars, &query_chars).map(|index| (index, query_chars.len()));
    if found.is_none() {
        found = query
            .split_whitespace()
            .filter_map(|term| {
                let chars = simple_lower_chars(term);
                find_chars(&lower_chars, &chars).map(|index| (index, chars.len()))
            })
            .min_by_key(|(index, _)| *index);
    }
    let Some((index, match_len)) = found else {
        return if content_chars.len() > 120 {
            format!("{}...", content_chars[..120].iter().collect::<String>())
        } else {
            content.to_string()
        };
    };
    let start = index.saturating_sub(40);
    let end = (index + match_len + 80).min(content_chars.len());
    let mut snippet = content_chars[start..end].iter().collect::<String>();
    if start > 0 {
        snippet.insert_str(0, "...");
    }
    if end < content_chars.len() {
        snippet.push_str("...");
    }
    snippet
}

fn find_chars(haystack: &[char], needle: &[char]) -> Option<usize> {
    (!needle.is_empty() && needle.len() <= haystack.len())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_wire_uses_calendar_dates_and_nullable_fields() {
        let now = chrono::Utc::now();
        let response = ProjectResponse::from(Project {
            id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            title: "Migration".into(),
            description: None,
            icon: None,
            status: "planned".into(),
            priority: "none".into(),
            lead_type: None,
            lead_id: None,
            start_date: chrono::NaiveDate::from_ymd_opt(2026, 8, 23),
            due_date: None,
            created_at: now,
            updated_at: now,
        });
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["start_date"], "2026-08-23");
        assert_eq!(value["description"], serde_json::Value::Null);
        assert_eq!(value["issue_count"], 0);
    }

    #[test]
    fn project_resource_wire_preserves_json_and_nullable_creator() {
        let now = chrono::Utc::now();
        let response = ProjectResourceResponse::from(ProjectResource {
            id: Uuid::nil(),
            project_id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            resource_type: "github_repo".into(),
            resource_ref: json!({"url": "git@github.com:patchbay-ai/patchbay.git"}),
            label: None,
            position: 2,
            created_at: now,
            created_by: None,
        });
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(
            value["resource_ref"]["url"],
            "git@github.com:patchbay-ai/patchbay.git"
        );
        assert_eq!(value["created_by"], Value::Null);
        assert_eq!(value["position"], 2);
    }

    #[test]
    fn resource_create_decoder_and_validation_match_go_presence_rules() {
        let request = decode_first::<CreateResourceRequest>(
            br#"{"resource_type":" github_repo ","resource_ref":{"url":" git@github.com:patchbay-ai/patchbay.git ","default_branch_hint":" main "},"future":true} trailing"#,
        )
        .unwrap();
        let normalized = normalize_resource_ref("github_repo", &request.resource_ref).unwrap();
        assert_eq!(normalized["url"], "git@github.com:patchbay-ai/patchbay.git");
        assert_eq!(normalized["default_branch_hint"], "main");

        let missing =
            decode_first::<CreateResourceRequest>(br#"{"resource_type":"github_repo"}"#).unwrap();
        assert_eq!(
            normalize_resource_ref("github_repo", &missing.resource_ref).unwrap_err(),
            "resource_ref is required"
        );
        let explicit_null = decode_first::<CreateResourceRequest>(
            br#"{"resource_type":"github_repo","resource_ref":null}"#,
        )
        .unwrap();
        assert_eq!(
            normalize_resource_ref("github_repo", &explicit_null.resource_ref).unwrap_err(),
            "github_repo: url is required"
        );
        let project = decode_first::<CreateProjectRequest>(
            br#"{"title":null,"status":null,"priority":null,"resources":null} trailing"#,
        )
        .unwrap();
        assert!(project.title.is_empty());
        assert!(project.status.is_empty());
        assert!(project.priority.is_empty());
        assert!(project.resources.is_empty());
    }

    #[test]
    fn resource_ref_validation_accepts_supported_git_and_absolute_path_forms() {
        for value in [
            "https://github.com/o/r.git",
            "ssh://git@github.com/o/r.git",
            "git@github.com:o/r.git",
        ] {
            assert!(is_valid_git_repo_url(value), "{value}");
        }
        for value in ["not-a-url", "git@github.com", "host:path@user"] {
            assert!(!is_valid_git_repo_url(value), "{value}");
        }
        for value in [
            "/Users/alex/Patchbay",
            r"C:\\code\\Patchbay",
            r"\\server\\share",
        ] {
            assert!(is_absolute_local_path(value), "{value}");
        }
        assert!(!is_absolute_local_path("relative/path"));
    }

    #[test]
    fn local_directory_label_convergence_preserves_unknown_fields() {
        let stored = json!({
            "local_path": "/repo",
            "daemon_id": "daemon-1",
            "label": "Old",
            "execution_mode": "worktree",
            "future": {"keep": true}
        });
        let renamed = json!({
            "local_path": "/repo",
            "daemon_id": "daemon-1",
            "label": "New",
            "execution_mode": "worktree",
            "future": {"keep": true}
        });
        assert!(differs_only_by_local_directory_label(&renamed, &stored));
        let synchronized = with_local_directory_label(&stored, Some("Current")).unwrap();
        assert_eq!(synchronized["label"], "Current");
        assert_eq!(synchronized["future"]["keep"], true);
        let cleared = with_local_directory_label(&stored, None).unwrap();
        assert!(cleared.get("label").is_none());
    }

    #[test]
    fn newest_runtime_capability_fails_closed_after_downgrade() {
        let now = chrono::Utc::now();
        let runtime = |id: u128, seen, metadata| patchbay_db::models::AgentRuntime {
            created_at: now,
            custom_name: None,
            daemon_id: Some("daemon-1".into()),
            device_info: "macOS".into(),
            id: Uuid::from_u128(id),
            last_seen_at: Some(seen),
            legacy_daemon_id: None,
            metadata,
            name: "runtime".into(),
            owner_id: None,
            profile_id: None,
            provider: "codex".into(),
            runtime_mode: "local".into(),
            status: "online".into(),
            updated_at: now,
            visibility: "private".into(),
            workspace_id: Uuid::nil(),
        };
        let runtimes = vec![
            runtime(
                1,
                now - chrono::Duration::minutes(1),
                json!({"capabilities":[patchbay_protocol::DAEMON_CAPABILITY_LOCAL_WORKTREE_V1],"cli_version":"0.4.24"}),
            ),
            runtime(2, now, json!({"capabilities":[],"cli_version":"0.4.23"})),
        ];
        let newest = newest_daemon_runtime(&runtimes, "daemon-1").unwrap();
        assert!(!runtime_has_worktree(newest));
        assert_eq!(latest_daemon_cli_version(&runtimes, "daemon-1"), "0.4.23");
    }

    #[test]
    fn search_parsing_matches_go_defaults_and_caps() {
        assert_eq!(parse_positive(&None, 20), 20);
        assert_eq!(parse_positive(&Some("0".into()), 20), 20);
        assert_eq!(parse_positive(&Some("75".into()), 20).min(50), 50);
        assert_eq!(parse_non_negative(&Some("-1".into()), 0), 0);
    }

    #[test]
    fn update_decoder_preserves_nullable_field_presence() {
        let (request, fields) = decode_update(
            br#"{"description":null,"icon":"rocket","start_date":"","unknown":true}"#,
        )
        .unwrap();
        assert!(fields.contains_key("description"));
        assert!(request.description.is_none());
        assert_eq!(request.icon.as_deref(), Some("rocket"));
        assert_eq!(request.start_date.as_deref(), Some(""));
        assert!(decode_update(br#"{"status":"planned"} trailing"#).is_err());
        let (request, fields) = decode_update(b"null").unwrap();
        assert!(fields.is_empty());
        assert!(request.title.is_none());
        assert!(decode_update(b"[]").is_err());
    }

    #[test]
    fn project_update_validation_matches_go_contract() {
        assert!(validate_enum("status", "in_progress", PROJECT_STATUSES).is_ok());
        assert!(validate_enum("status", "active", PROJECT_STATUSES)
            .unwrap_err()
            .contains("planned, in_progress, paused, completed, cancelled"));
        assert_eq!(
            calendar_date("2026-08-23", "start_date").unwrap(),
            chrono::NaiveDate::from_ymd_opt(2026, 8, 23)
        );
        assert_eq!(calendar_date("", "due_date").unwrap(), None);
        assert_eq!(
            calendar_date("08/23/2026", "due_date").unwrap_err(),
            "invalid due_date format, expected YYYY-MM-DD"
        );
    }

    #[test]
    fn snippet_preserves_unicode_and_falls_back_to_terms() {
        let content =
            "这是一段很长的中文内容，包含了搜索关键词测试用例，用来验证多字节字符不会被截断";
        assert!(extract_snippet(content, "搜索关键词").contains("搜索关键词"));
        assert!(
            extract_snippet("deploy now, kubernetes later", "deploy kubernetes").contains("deploy")
        );
    }

    #[test]
    fn snippet_indices_remain_aligned_for_expanding_unicode_lowercase() {
        let content = format!("{}x", "İ".repeat(100));
        let snippet = extract_snippet(&content, "x");
        assert!(snippet.contains('x'));
        assert!(snippet.chars().count() <= 124);
    }

    #[test]
    fn like_escaping_matches_go() {
        assert_eq!(escape_like(r"a%b_c\d"), r"a\%b\_c\\d");
    }
}
