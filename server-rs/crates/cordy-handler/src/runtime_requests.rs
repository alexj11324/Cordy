//! Frontend-facing runtime pending-request handlers.
//!
//! These routes enqueue work that a daemon later claims from the shared
//! pending stores. They intentionally fail closed when Redis-backed stores are
//! not configured: returning a request that cannot be observed by the daemon
//! would strand the UI in a polling loop.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use cordy_db::models::{AgentRuntime, Member};
use cordy_db::queries::{member, runtime};
use cordy_middleware::workspace::WorkspaceContext;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::error_response;
use crate::pending_store::{
    ModelListRequest, ModelListStatus, MODEL_CATALOG_REVALIDATE_AFTER_SECS,
};
use crate::state::HandlerState;

const STORE_UNAVAILABLE: &str = "runtime pending requests are unavailable";
const UPDATE_IN_PROGRESS: &str = "an update is already in progress for this runtime";

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/runtimes/{runtime_id}/update", post(initiate_update))
        .route("/api/runtimes/{runtime_id}/update/", post(initiate_update))
        .route(
            "/api/runtimes/{runtime_id}/update/{request_id}",
            get(get_update),
        )
        .route(
            "/api/runtimes/{runtime_id}/update/{request_id}/",
            get(get_update),
        )
        .route(
            "/api/runtimes/{runtime_id}/models",
            post(initiate_model_list),
        )
        .route(
            "/api/runtimes/{runtime_id}/models/",
            post(initiate_model_list),
        )
        .route(
            "/api/runtimes/{runtime_id}/models/{request_id}",
            get(get_model_list),
        )
        .route(
            "/api/runtimes/{runtime_id}/models/{request_id}/",
            get(get_model_list),
        )
        .route(
            "/api/runtimes/{runtime_id}/local-skills",
            post(initiate_local_skill_list),
        )
        .route(
            "/api/runtimes/{runtime_id}/local-skills/",
            post(initiate_local_skill_list),
        )
        .route(
            "/api/runtimes/{runtime_id}/local-skills/{request_id}",
            get(get_local_skill_list),
        )
        .route(
            "/api/runtimes/{runtime_id}/local-skills/{request_id}/",
            get(get_local_skill_list),
        )
        .route(
            "/api/runtimes/{runtime_id}/local-skills/import",
            post(initiate_local_skill_import),
        )
        .route(
            "/api/runtimes/{runtime_id}/local-skills/import/",
            post(initiate_local_skill_import),
        )
        .route(
            "/api/runtimes/{runtime_id}/local-skills/import/{request_id}",
            get(get_local_skill_import),
        )
        .route(
            "/api/runtimes/{runtime_id}/local-skills/import/{request_id}/",
            get(get_local_skill_import),
        )
}

fn parse_runtime_id(raw_id: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(raw_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"))
}

async fn load_runtime_member(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_id: &str,
) -> Result<(AgentRuntime, Member), Response> {
    let runtime_id = parse_runtime_id(raw_id)?;
    let found = runtime::get_agent_runtime(&state.pool, runtime_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "runtime not found"))?;
    let member = member::get_member_by_user_and_workspace(
        &state.pool,
        context.member.user_id,
        found.workspace_id,
    )
    .await
    .ok()
    .flatten()
    .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "runtime not found"))?;
    Ok((found, member))
}

fn can_edit_runtime(member: &Member, runtime: &AgentRuntime) -> bool {
    matches!(member.role.as_str(), "owner" | "admin") || runtime.owner_id == Some(member.user_id)
}

fn local_skill_import_allowed(member: &Member, runtime: &AgentRuntime) -> bool {
    runtime.owner_id == Some(member.user_id)
}

fn store_unavailable() -> Response {
    error_response(StatusCode::SERVICE_UNAVAILABLE, STORE_UNAVAILABLE)
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

fn decode_first<T>(body: &[u8]) -> Result<T, ()>
where
    T: for<'de> Deserialize<'de> + Default,
{
    let mut values = serde_json::Deserializer::from_slice(body).into_iter::<Option<T>>();
    values
        .next()
        .ok_or(())?
        .map(|value| value.unwrap_or_default())
        .map_err(|_| ())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct UpdateRequestBody {
    #[serde(deserialize_with = "null_default")]
    target_version: String,
}

async fn initiate_update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(runtime_id): Path<String>,
    body: Bytes,
) -> Response {
    let (found, member) = match load_runtime_member(&state, &context, &runtime_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !can_edit_runtime(&member, &found) {
        return error_response(
            StatusCode::FORBIDDEN,
            "only runtime owners and workspace admins can update runtimes",
        );
    }
    let request = match decode_first::<UpdateRequestBody>(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.target_version.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "target_version is required");
    }
    let Some(store) = state.update_store.as_ref() else {
        return store_unavailable();
    };
    match store
        .create(
            &found.id.to_string(),
            &request.target_version,
            &member.user_id.to_string(),
        )
        .await
    {
        Ok(update) => Json(update).into_response(),
        Err(error) if error.to_string() == "update already in progress" => {
            error_response(StatusCode::CONFLICT, UPDATE_IN_PROGRESS)
        }
        Err(error) => {
            tracing::error!(%error, runtime_id = %found.id, "update store create failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to start the update",
            )
        }
    }
}

async fn get_update(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((runtime_id, request_id)): Path<(String, String)>,
) -> Response {
    let (found, member) = match load_runtime_member(&state, &context, &runtime_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(store) = state.update_store.as_ref() else {
        return store_unavailable();
    };
    let update = match store.get(&request_id).await {
        Ok(Some(update)) if update.runtime_id == found.id.to_string() => update,
        Ok(_) => return error_response(StatusCode::NOT_FOUND, "update not found"),
        Err(error) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("failed to load update: {error}"),
            )
        }
    };
    if !can_edit_runtime(&member, &found) && update.initiator_user_id != member.user_id.to_string()
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "only runtime owners, workspace admins, and the update initiator can view this update",
        );
    }
    Json(update).into_response()
}

async fn initiate_model_list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(runtime_id): Path<String>,
) -> Response {
    let (found, _) = match load_runtime_member(&state, &context, &runtime_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if found.status != "online" {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "runtime is offline");
    }
    let resolved_runtime_id = found.id.to_string();
    if let Some(cache) = state.model_catalog_cache.as_ref() {
        match cache.get(&resolved_runtime_id).await {
            Ok(Some(snapshot))
                if crate::pending_store::cacheable_model_catalog(
                    &snapshot.models,
                    snapshot.supported,
                ) =>
            {
                let age = Utc::now()
                    .signed_duration_since(snapshot.stored_at)
                    .num_seconds();
                if age >= MODEL_CATALOG_REVALIDATE_AFTER_SECS {
                    revalidate_model_catalog(&state, &resolved_runtime_id).await;
                }
                let stored_at = snapshot.stored_at;
                return Json(ModelListRequest {
                    id: crate::pending_store::random_id(),
                    runtime_id: resolved_runtime_id,
                    status: ModelListStatus::Completed,
                    models: snapshot.models,
                    supported: snapshot.supported,
                    created_at: stored_at,
                    updated_at: stored_at,
                    cached: true,
                    cached_at: Some(stored_at),
                    ..Default::default()
                })
                .into_response();
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, runtime_id = %resolved_runtime_id, "model catalog cache read failed");
            }
        }
    }
    let Some(store) = state.model_list_store.as_ref() else {
        return store_unavailable();
    };
    match store.create(&resolved_runtime_id).await {
        Ok(request) => {
            state
                .daemon_notifier
                .notify_pending_work(
                    &resolved_runtime_id,
                    cordy_protocol::PENDING_WORK_KIND_MODEL_LIST,
                )
                .await;
            Json(request).into_response()
        }
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("failed to enqueue model list request: {error}"),
        ),
    }
}

async fn revalidate_model_catalog(state: &HandlerState, runtime_id: &str) {
    let Some(store) = state.model_list_store.as_ref() else {
        return;
    };
    match store.has_pending(runtime_id).await {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            tracing::debug!(%error, %runtime_id, "model catalog revalidate probe failed");
            return;
        }
    }
    if let Err(error) = store.create(runtime_id).await {
        tracing::debug!(%error, %runtime_id, "model catalog revalidate enqueue failed");
        return;
    }
    state
        .daemon_notifier
        .notify_pending_work(runtime_id, cordy_protocol::PENDING_WORK_KIND_MODEL_LIST)
        .await;
}

async fn get_model_list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((runtime_id, request_id)): Path<(String, String)>,
) -> Response {
    let (found, _) = match load_runtime_member(&state, &context, &runtime_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(store) = state.model_list_store.as_ref() else {
        return store_unavailable();
    };
    match store.get(&request_id).await {
        Ok(Some(request)) if request.runtime_id == found.id.to_string() => {
            Json(request).into_response()
        }
        Ok(_) => error_response(StatusCode::NOT_FOUND, "request not found"),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("failed to load request: {error}"),
        ),
    }
}

async fn initiate_local_skill_list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(runtime_id): Path<String>,
) -> Response {
    let (found, _) = match load_runtime_member(&state, &context, &runtime_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if found.status != "online" {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "runtime is offline");
    }
    let Some(store) = state.local_skill_list_store.as_ref() else {
        return store_unavailable();
    };
    match store.create(&found.id.to_string()).await {
        Ok(request) => Json(request).into_response(),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("failed to enqueue local skills request: {error}"),
        ),
    }
}

async fn get_local_skill_list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((runtime_id, request_id)): Path<(String, String)>,
) -> Response {
    let (found, _) = match load_runtime_member(&state, &context, &runtime_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(store) = state.local_skill_list_store.as_ref() else {
        return store_unavailable();
    };
    match store.get(&request_id).await {
        Ok(Some(request)) if request.runtime_id == found.id.to_string() => {
            Json(request).into_response()
        }
        Ok(_) => error_response(StatusCode::NOT_FOUND, "request not found"),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("failed to load request: {error}"),
        ),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LocalSkillImportBody {
    #[serde(deserialize_with = "null_default")]
    skill_key: String,
    name: Option<String>,
    description: Option<String>,
    #[serde(deserialize_with = "null_default")]
    action: String,
    #[serde(deserialize_with = "null_default")]
    target_skill_id: String,
    #[serde(deserialize_with = "null_default")]
    supports_conflict: bool,
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

async fn initiate_local_skill_import(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(runtime_id): Path<String>,
    body: Bytes,
) -> Response {
    let (found, member) = match load_runtime_member(&state, &context, &runtime_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !local_skill_import_allowed(&member, &found) {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    if found.status != "online" {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "runtime is offline");
    }
    let request = match decode_first::<LocalSkillImportBody>(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let skill_key = request.skill_key.trim();
    if skill_key.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "skill_key is required");
    }
    let target_skill_id = match request.action.as_str() {
        "" => String::new(),
        "overwrite" => match Uuid::parse_str(request.target_skill_id.trim()) {
            Ok(id) => id.to_string(),
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid target_skill_id"),
        },
        _ => return error_response(StatusCode::BAD_REQUEST, "invalid action"),
    };
    let supports_conflict = request.supports_conflict || request.action == "overwrite";
    let Some(store) = state.local_skill_import_store.as_ref() else {
        return store_unavailable();
    };
    match store
        .create_import(
            &found.id.to_string(),
            &member.user_id.to_string(),
            skill_key,
            clean_optional(request.name),
            clean_optional(request.description),
            &request.action,
            &target_skill_id,
            supports_conflict,
        )
        .await
    {
        Ok(request) => Json(request).into_response(),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("failed to enqueue local skill import: {error}"),
        ),
    }
}

async fn get_local_skill_import(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((runtime_id, request_id)): Path<(String, String)>,
) -> Response {
    let (found, member) = match load_runtime_member(&state, &context, &runtime_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !local_skill_import_allowed(&member, &found) {
        return error_response(StatusCode::FORBIDDEN, "insufficient permissions");
    }
    let Some(store) = state.local_skill_import_store.as_ref() else {
        return store_unavailable();
    };
    match store.get(&request_id).await {
        Ok(Some(request)) if request.runtime_id == found.id.to_string() => {
            Json(request).into_response()
        }
        Ok(_) => error_response(StatusCode::NOT_FOUND, "request not found"),
        Err(error) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("failed to load request: {error}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use http_body_util::BodyExt as _;

    fn member(role: &str, user_id: Uuid) -> Member {
        Member {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            user_id,
            role: role.to_string(),
            created_at: Utc::now(),
        }
    }

    fn runtime(owner_id: Option<Uuid>) -> AgentRuntime {
        let now = Utc::now();
        AgentRuntime {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            daemon_id: Some("daemon".into()),
            name: "Runtime".into(),
            runtime_mode: "local".into(),
            provider: "claude".into(),
            status: "online".into(),
            device_info: String::new(),
            metadata: serde_json::json!({}),
            last_seen_at: Some(now),
            created_at: now,
            updated_at: now,
            owner_id,
            legacy_daemon_id: None,
            visibility: "private".into(),
            profile_id: None,
            custom_name: None,
        }
    }

    #[test]
    fn update_permissions_match_go_contract() {
        let owner_id = Uuid::new_v4();
        let found = runtime(Some(owner_id));
        assert!(can_edit_runtime(&member("member", owner_id), &found));
        assert!(can_edit_runtime(&member("owner", Uuid::new_v4()), &found));
        assert!(can_edit_runtime(&member("admin", Uuid::new_v4()), &found));
        assert!(!can_edit_runtime(&member("member", Uuid::new_v4()), &found));
    }

    #[test]
    fn local_skill_import_is_owner_only_even_for_admins() {
        let owner_id = Uuid::new_v4();
        let found = runtime(Some(owner_id));
        assert!(local_skill_import_allowed(
            &member("member", owner_id),
            &found
        ));
        assert!(!local_skill_import_allowed(
            &member("admin", Uuid::new_v4()),
            &found
        ));
        assert!(!local_skill_import_allowed(
            &member("owner", Uuid::new_v4()),
            &found
        ));
    }

    #[test]
    fn import_decoder_and_normalization_match_go() {
        let request = decode_first::<LocalSkillImportBody>(
            br#"{"skill_key":"  review-helper  ","name":"  Review  ","description":"  ","action":"overwrite","target_skill_id":"018f03a0-c4d2-7a37-ae4d-5aa45de12f11"} trailing"#,
        )
        .unwrap();
        assert_eq!(request.skill_key.trim(), "review-helper");
        assert_eq!(clean_optional(request.name).as_deref(), Some("Review"));
        assert_eq!(clean_optional(request.description), None);
        assert_eq!(request.action, "overwrite");

        let null = decode_first::<LocalSkillImportBody>(b"null").unwrap();
        assert!(null.skill_key.is_empty());
        let null_fields = decode_first::<LocalSkillImportBody>(
            br#"{"skill_key":null,"action":null,"target_skill_id":null,"supports_conflict":null}"#,
        )
        .unwrap();
        assert!(null_fields.skill_key.is_empty());
        assert!(null_fields.action.is_empty());
        assert!(!null_fields.supports_conflict);
    }

    #[test]
    fn malformed_runtime_ids_are_bad_requests() {
        assert_eq!(
            parse_runtime_id("not-a-uuid").unwrap_err().status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn disabled_store_is_explicit_service_unavailable() {
        let response = store_unavailable();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"error": STORE_UNAVAILABLE})
        );
    }
}
