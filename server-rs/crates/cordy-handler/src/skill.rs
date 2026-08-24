//! Workspace skill library handlers.
//!
//! Network-backed search/import/refresh lives in the sibling `skill_import`
//! module; both modules share the same wire shapes and transactional helpers.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::models::{IssueLabel, Skill, SkillFile};
use cordy_db::queries::{issue_label, skill};
use cordy_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .merge(crate::skill_import::router())
        .route("/api/skills", get(list).post(create))
        .route("/api/skills/", get(list).post(create))
        .route("/api/skills/{id}", get(get_one).put(update).delete(delete))
        .route("/api/skills/{id}/", get(get_one).put(update).delete(delete))
        .route(
            "/api/skills/{id}/labels",
            get(list_labels).post(attach_label),
        )
        .route(
            "/api/skills/{id}/labels/{label_id}",
            axum::routing::delete(detach_label),
        )
        .route("/api/skills/{id}/files", get(list_files).put(upsert_file))
        .route(
            "/api/skills/{id}/files/{file_id}",
            axum::routing::delete(delete_file),
        )
}

#[derive(Debug, Serialize)]
pub(super) struct SkillResponse {
    pub(super) id: Uuid,
    pub(super) workspace_id: Uuid,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) content: String,
    pub(super) config: Value,
    pub(super) created_by: Option<Uuid>,
    pub(super) created_at: String,
    pub(super) updated_at: String,
}

#[derive(Debug, Serialize)]
struct SkillSummaryResponse {
    id: Uuid,
    workspace_id: Uuid,
    name: String,
    description: String,
    config: Value,
    created_by: Option<Uuid>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
pub(super) struct SkillFileResponse {
    pub(super) id: Uuid,
    pub(super) skill_id: Uuid,
    pub(super) path: String,
    pub(super) content: String,
    pub(super) created_at: String,
    pub(super) updated_at: String,
}

#[derive(Debug, Serialize)]
pub(super) struct SkillWithFilesResponse {
    #[serde(flatten)]
    pub(super) skill: SkillResponse,
    pub(super) files: Vec<SkillFileResponse>,
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

impl From<Skill> for SkillResponse {
    fn from(value: Skill) -> Self {
        Self {
            id: value.id,
            workspace_id: value.workspace_id,
            name: value.name,
            description: value.description,
            content: value.content,
            config: object_config(value.config),
            created_by: value.created_by,
            created_at: crate::timefmt::rfc3339(value.created_at),
            updated_at: crate::timefmt::rfc3339(value.updated_at),
        }
    }
}

impl From<SkillFile> for SkillFileResponse {
    fn from(value: SkillFile) -> Self {
        Self {
            id: value.id,
            skill_id: value.skill_id,
            path: value.path,
            content: value.content,
            created_at: crate::timefmt::rfc3339(value.created_at),
            updated_at: crate::timefmt::rfc3339(value.updated_at),
        }
    }
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

pub(super) fn object_config(value: Value) -> Value {
    if value.is_null() {
        json!({})
    } else {
        value
    }
}

pub(super) fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::NOT_FOUND, "workspace not found"))
}

fn parse_id(raw: &str, name: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(raw)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, &format!("invalid {name}")))
}

pub(super) fn db_error(error: anyhow::Error, message: &str) -> Response {
    tracing::warn!(%error, "{message}");
    error_response(StatusCode::INTERNAL_SERVER_ERROR, message)
}

pub(super) fn unique_violation(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23505")
}

pub(super) fn skill_event(
    event_type: &str,
    workspace_id: Uuid,
    actor_type: &str,
    actor_id: Uuid,
    payload: Value,
) -> cordy_events::Event {
    cordy_events::Event {
        event_type: event_type.into(),
        workspace_id: workspace_id.to_string(),
        actor_type: actor_type.into(),
        actor_id: actor_id.to_string(),
        payload,
        ..Default::default()
    }
}

async fn load_skill(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_id: &str,
) -> Result<Skill, Response> {
    let id = parse_id(raw_id, "skill id")?;
    let workspace_id = workspace_id(context)?;
    match skill::get_skill_in_workspace(&state.pool, id, workspace_id).await {
        Ok(Some(value)) => Ok(value),
        Ok(None) => Err(error_response(StatusCode::NOT_FOUND, "skill not found")),
        // Go deliberately hides lookup failures behind the same 404 boundary.
        Err(error) => {
            tracing::warn!(%error, %id, %workspace_id, "failed to load skill");
            Err(error_response(StatusCode::NOT_FOUND, "skill not found"))
        }
    }
}

fn can_manage(context: &WorkspaceContext, value: &Skill) -> Result<(), Response> {
    if matches!(context.member.role.as_str(), "owner" | "admin")
        || value.created_by == Some(context.member.user_id)
    {
        Ok(())
    } else {
        Err(error_response(
            StatusCode::FORBIDDEN,
            "only the skill creator can manage this skill",
        ))
    }
}

pub(super) fn sanitize(value: &str) -> String {
    value.replace('\0', "")
}

fn clean_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|last| *last != "..") => {
                parts.pop();
            }
            ".." => parts.push(part),
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

pub(super) fn valid_file_path(path: &str) -> bool {
    !path.is_empty()
        && !std::path::Path::new(path).is_absolute()
        && !clean_path(path).starts_with("..")
}

pub(super) fn reserved_content_path(path: &str) -> bool {
    clean_path(path).eq_ignore_ascii_case("SKILL.md")
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

fn decode<T>(body: &[u8]) -> Result<T, Response>
where
    T: for<'de> Deserialize<'de> + Default,
{
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    Option::<T>::deserialize(&mut deserializer)
        .map(Option::unwrap_or_default)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid request body"))
}

#[derive(Debug, Default, Deserialize)]
struct SkillFileRequest {
    #[serde(default, deserialize_with = "null_default")]
    path: String,
    #[serde(default, deserialize_with = "null_default")]
    content: String,
}

#[derive(Debug, Default, Deserialize)]
struct CreateRequest {
    #[serde(default, deserialize_with = "null_default")]
    name: String,
    #[serde(default, deserialize_with = "null_default")]
    description: String,
    #[serde(default, deserialize_with = "null_default")]
    content: String,
    config: Option<Value>,
    #[serde(default, deserialize_with = "null_default")]
    files: Vec<SkillFileRequest>,
}

#[derive(Debug, Default, Deserialize)]
struct UpdateRequest {
    name: Option<String>,
    description: Option<String>,
    content: Option<String>,
    config: Option<Value>,
    files: Option<Vec<SkillFileRequest>>,
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match skill::list_skill_summaries_by_workspace(&state.pool, workspace_id).await {
        Ok(skills) => Json(
            skills
                .into_iter()
                .filter_map(|value| {
                    Some(SkillSummaryResponse {
                        id: value.id?,
                        workspace_id: value.workspace_id?,
                        name: value.name,
                        description: value.description,
                        config: object_config(value.config.unwrap_or_else(|| json!({}))),
                        created_by: value.created_by,
                        created_at: crate::timefmt::rfc3339(value.created_at?),
                        updated_at: crate::timefmt::rfc3339(value.updated_at?),
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => db_error(error, "failed to list skills"),
    }
}

async fn get_one(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let value = match load_skill(&state, &context, &id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let files = match skill::list_skill_files(&state.pool, value.id).await {
        Ok(files) => files.into_iter().map(Into::into).collect(),
        Err(error) => return db_error(error, "failed to list skill files"),
    };
    Json(SkillWithFilesResponse {
        skill: value.into(),
        files,
    })
    .into_response()
}

async fn create(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request: CreateRequest = match decode(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if request.name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "name is required");
    }
    if let Some(file) = request
        .files
        .iter()
        .find(|file| !valid_file_path(&file.path))
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!("invalid file path: {}", file.path),
        );
    }
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(error) => return db_error(error.into(), "failed to create skill"),
    };
    let value = match skill::create_skill(
        &mut *transaction,
        workspace_id,
        &sanitize(&request.name),
        &sanitize(&request.description),
        &sanitize(&request.content),
        &object_config(request.config.unwrap_or_else(|| json!({}))),
        context.member.user_id,
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to create skill")
        }
        Err(error) if unique_violation(&error) => {
            return error_response(
                StatusCode::CONFLICT,
                "a skill with this name already exists",
            )
        }
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to create skill: {error}"),
            )
        }
    };
    let mut files = Vec::with_capacity(request.files.len());
    for file in request
        .files
        .into_iter()
        .filter(|file| !reserved_content_path(&file.path))
    {
        match skill::upsert_skill_file(
            &mut *transaction,
            value.id,
            &sanitize(&file.path),
            &sanitize(&file.content),
        )
        .await
        {
            Ok(Some(file)) => files.push(file.into()),
            Ok(None) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create skill file",
                )
            }
            Err(error) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("failed to create skill: {error}"),
                )
            }
        }
    }
    if let Err(error) = transaction.commit().await {
        return db_error(error.into(), "failed to create skill");
    }
    let response = SkillWithFilesResponse {
        skill: value.into(),
        files,
    };
    let (actor_type, actor_id, _) = crate::issue::mutation_actor(&state, &context, &headers).await;
    state.bus.publish(&skill_event(
        cordy_protocol::EVENT_SKILL_CREATED,
        workspace_id,
        &actor_type,
        actor_id,
        json!({ "skill": &response }),
    ));
    (StatusCode::CREATED, Json(response)).into_response()
}

async fn update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let existing = match load_skill(&state, &context, &id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = can_manage(&context, &existing) {
        return response;
    }
    let request: UpdateRequest = match decode(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Some(file) = request
        .files
        .as_ref()
        .and_then(|files| files.iter().find(|file| !valid_file_path(&file.path)))
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!("invalid file path: {}", file.path),
        );
    }
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(error) => return db_error(error.into(), "failed to start transaction"),
    };
    let config = request.config.map(object_config);
    let name = request.name.as_deref().map(sanitize);
    let description = request.description.as_deref().map(sanitize);
    let content = request.content.as_deref().map(sanitize);
    let value = match skill::update_skill(
        &mut *transaction,
        existing.id,
        name.as_deref(),
        description.as_deref(),
        content.as_deref(),
        config.as_ref(),
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "skill not found"),
        Err(error) if unique_violation(&error) => {
            return error_response(
                StatusCode::CONFLICT,
                "a skill with this name already exists",
            )
        }
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to update skill: {error}"),
            )
        }
    };
    let files = if let Some(files) = request.files {
        if let Err(error) = skill::delete_skill_files_by_skill(&mut *transaction, value.id).await {
            return db_error(error, "failed to delete old skill files");
        }
        let mut responses = Vec::with_capacity(files.len());
        for file in files
            .into_iter()
            .filter(|file| !reserved_content_path(&file.path))
        {
            match skill::upsert_skill_file(
                &mut *transaction,
                value.id,
                &sanitize(&file.path),
                &sanitize(&file.content),
            )
            .await
            {
                Ok(Some(file)) => responses.push(file.into()),
                Ok(None) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to upsert skill file",
                    )
                }
                Err(error) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("failed to upsert skill file: {error}"),
                    )
                }
            }
        }
        responses
    } else {
        match skill::list_skill_files(&mut *transaction, value.id).await {
            Ok(files) => files.into_iter().map(Into::into).collect(),
            // Go ignores this read error and returns an empty file list.
            Err(error) => {
                tracing::warn!(%error, skill_id = %value.id, "failed to list unchanged skill files");
                Vec::new()
            }
        }
    };
    if let Err(error) = transaction.commit().await {
        return db_error(error.into(), "failed to commit");
    }
    let response = SkillWithFilesResponse {
        skill: value.into(),
        files,
    };
    let (actor_type, actor_id, _) = crate::issue::mutation_actor(&state, &context, &headers).await;
    state.bus.publish(&skill_event(
        cordy_protocol::EVENT_SKILL_UPDATED,
        existing.workspace_id,
        &actor_type,
        actor_id,
        json!({ "skill": &response }),
    ));
    Json(response).into_response()
}

async fn delete(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let value = match load_skill(&state, &context, &id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = can_manage(&context, &value) {
        return response;
    }
    let mut transaction = match state.pool.begin().await {
        Ok(value) => value,
        Err(error) => return db_error(error.into(), "failed to start transaction"),
    };
    if let Err(error) =
        issue_label::delete_skill_label_assignments_by_skill(&mut *transaction, value.id).await
    {
        return db_error(error, "failed to remove skill label assignments");
    }
    if let Err(error) = skill::delete_skill(&mut *transaction, value.id, value.workspace_id).await {
        return db_error(error, "failed to delete skill");
    }
    if let Err(error) = transaction.commit().await {
        return db_error(error.into(), "failed to commit skill deletion");
    }
    let (actor_type, actor_id, _) = crate::issue::mutation_actor(&state, &context, &headers).await;
    state.bus.publish(&skill_event(
        cordy_protocol::EVENT_SKILL_DELETED,
        value.workspace_id,
        &actor_type,
        actor_id,
        json!({ "skill_id": value.id }),
    ));
    StatusCode::NO_CONTENT.into_response()
}

async fn list_files(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let value = match load_skill(&state, &context, &id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match skill::list_skill_files(&state.pool, value.id).await {
        Ok(files) => Json(
            files
                .into_iter()
                .map(SkillFileResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => db_error(error, "failed to list skill files"),
    }
}

async fn upsert_file(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let value = match load_skill(&state, &context, &id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = can_manage(&context, &value) {
        return response;
    }
    let request: SkillFileRequest = match decode(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !valid_file_path(&request.path) {
        return error_response(StatusCode::BAD_REQUEST, "invalid file path");
    }
    if reserved_content_path(&request.path) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "SKILL.md is reserved for the primary skill content",
        );
    }
    match skill::upsert_skill_file(
        &state.pool,
        value.id,
        &sanitize(&request.path),
        &sanitize(&request.content),
    )
    .await
    {
        Ok(Some(file)) => Json(SkillFileResponse::from(file)).into_response(),
        Ok(None) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to upsert skill file",
        ),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("failed to upsert skill file: {error}"),
        ),
    }
}

async fn delete_file(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((id, file_id)): Path<(String, String)>,
) -> Response {
    let value = match load_skill(&state, &context, &id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = can_manage(&context, &value) {
        return response;
    }
    let file_id = match parse_id(&file_id, "file id") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let file = match skill::get_skill_file(&state.pool, file_id).await {
        Ok(Some(file)) if file.skill_id == value.id => file,
        Ok(_) => return error_response(StatusCode::NOT_FOUND, "skill file not found"),
        Err(error) => {
            tracing::warn!(%error, %file_id, "failed to load skill file");
            return error_response(StatusCode::NOT_FOUND, "skill file not found");
        }
    };
    match skill::delete_skill_file(&state.pool, file.id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => db_error(error, "failed to delete skill file"),
    }
}

async fn list_labels(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let value = match load_skill(&state, &context, &id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match issue_label::list_labels_by_skill(&state.pool, value.id, value.workspace_id).await {
        Ok(labels) => Json(json!({
            "labels": labels.into_iter().map(LabelResponse::from).collect::<Vec<_>>()
        }))
        .into_response(),
        Err(error) => db_error(error, "failed to list skill labels"),
    }
}

#[derive(Debug, Default, Deserialize)]
struct AttachLabelRequest {
    #[serde(default, deserialize_with = "null_default")]
    label_id: String,
}

async fn attach_label(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let value = match load_skill(&state, &context, &id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = can_manage(&context, &value) {
        return response;
    }
    let request: AttachLabelRequest = match decode::<AttachLabelRequest>(&body) {
        Ok(value) if !value.label_id.is_empty() => value,
        _ => return error_response(StatusCode::BAD_REQUEST, "label_id is required"),
    };
    let label_id = match parse_id(&request.label_id, "label_id") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let label = match issue_label::get_label(&state.pool, label_id, value.workspace_id).await {
        Ok(Some(label)) if label.resource_type == "skill" => label,
        Ok(_) => return error_response(StatusCode::NOT_FOUND, "skill label not found"),
        Err(error) => {
            tracing::warn!(%error, %label_id, "failed to load skill label");
            return error_response(StatusCode::NOT_FOUND, "skill label not found");
        }
    };
    if let Err(error) =
        issue_label::attach_label_to_skill(&state.pool, value.id, label.id, value.workspace_id)
            .await
    {
        return db_error(error, "failed to attach skill label");
    }
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_LABEL_UPDATED.into(),
        workspace_id: value.workspace_id.to_string(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload: json!({ "label": LabelResponse::from(label) }),
        ..Default::default()
    });
    list_labels(State(state), Extension(context), Path(id)).await
}

async fn detach_label(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((id, label_id)): Path<(String, String)>,
) -> Response {
    let value = match load_skill(&state, &context, &id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(response) = can_manage(&context, &value) {
        return response;
    }
    let label_id = match parse_id(&label_id, "label id") {
        Ok(value) => value,
        Err(response) => return response,
    };
    if let Err(error) =
        issue_label::detach_label_from_skill(&state.pool, value.id, label_id, value.workspace_id)
            .await
    {
        return db_error(error, "failed to detach skill label");
    }
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_LABEL_UPDATED.into(),
        workspace_id: value.workspace_id.to_string(),
        actor_type: "member".into(),
        actor_id: context.member.user_id.to_string(),
        payload: json!({ "label_id": label_id, "resource_type": "skill" }),
        ..Default::default()
    });
    list_labels(State(state), Extension(context), Path(id)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use http_body_util::BodyExt as _;

    fn member(role: &str, user_id: Uuid) -> WorkspaceContext {
        WorkspaceContext {
            workspace_id: Uuid::new_v4().to_string(),
            member: cordy_db::models::Member {
                id: Uuid::new_v4(),
                workspace_id: Uuid::new_v4(),
                user_id,
                role: role.into(),
                created_at: Utc::now(),
            },
        }
    }

    fn model(created_by: Option<Uuid>) -> Skill {
        Skill {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            name: "review".into(),
            description: String::new(),
            content: String::new(),
            config: Value::Null,
            created_by,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            plugin_installation_id: None,
        }
    }

    #[test]
    fn file_paths_match_go_cleaning_and_reserved_rules() {
        assert!(valid_file_path("references/checklist.md"));
        assert!(valid_file_path("docs/../examples/a.md"));
        assert!(!valid_file_path(""));
        assert!(!valid_file_path("/tmp/secret"));
        assert!(!valid_file_path("../secret"));
        assert!(!valid_file_path("docs/../../secret"));
        assert!(reserved_content_path("SKILL.md"));
        assert!(reserved_content_path("./skill.MD"));
        assert!(reserved_content_path("docs/../SKILL.md"));
        assert!(!reserved_content_path("docs/SKILL.md"));
    }

    #[test]
    fn creator_and_admin_can_manage_but_other_member_cannot() {
        let creator = Uuid::new_v4();
        let value = model(Some(creator));
        assert!(can_manage(&member("member", creator), &value).is_ok());
        assert!(can_manage(&member("owner", Uuid::new_v4()), &value).is_ok());
        assert!(can_manage(&member("admin", Uuid::new_v4()), &value).is_ok());
        assert_eq!(
            can_manage(&member("member", Uuid::new_v4()), &value)
                .unwrap_err()
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn skill_wire_uses_empty_object_for_null_config() {
        let response = Json(SkillResponse::from(model(None))).into_response();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["config"], json!({}));
        assert!(value["created_by"].is_null());
    }

    #[test]
    fn decoders_match_go_required_field_defaults() {
        let request: CreateRequest = decode(br#"{"description":"hello"}"#).unwrap();
        assert_eq!(request.name, "");
        assert!(request.files.is_empty());
        let file: SkillFileRequest = decode(br#"{"path":"refs/a.md"}"#).unwrap();
        assert_eq!(file.content, "");
        assert!(decode::<CreateRequest>(b"not-json").is_err());
    }

    #[test]
    fn decoder_accepts_go_explicit_null_zero_values() {
        let request: CreateRequest =
            decode(br#"{"name":"review","description":null,"content":null,"files":null}"#).unwrap();
        assert_eq!(request.name, "review");
        assert_eq!(request.description, "");
        assert_eq!(request.content, "");
        assert!(request.files.is_empty());

        let file: SkillFileRequest = decode(br#"{"path":"refs/a.md","content":null}"#).unwrap();
        assert_eq!(file.content, "");

        let update: UpdateRequest = decode(b"null").unwrap();
        assert!(update.name.is_none());
        assert!(update.files.is_none());
    }

    #[test]
    fn skill_event_preserves_resolved_agent_actor() {
        let workspace_id = Uuid::new_v4();
        let agent_id = Uuid::new_v4();
        let event = skill_event(
            cordy_protocol::EVENT_SKILL_UPDATED,
            workspace_id,
            "agent",
            agent_id,
            json!({}),
        );
        assert_eq!(event.workspace_id, workspace_id.to_string());
        assert_eq!(event.actor_type, "agent");
        assert_eq!(event.actor_id, agent_id.to_string());
    }
}
