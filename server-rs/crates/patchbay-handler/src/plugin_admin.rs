//! Workspace-admin plugin installation routes.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::SecondsFormat;
use patchbay_db::models::PluginInstallation;
use patchbay_middleware::workspace::WorkspaceContext;
use patchbay_service::plugin::{
    config_fields_for_manifest, decode_scopes, hook_signing_secret, parse_installation_manifest,
    PluginError, PluginErrorKind,
};
use patchbay_service::plugin_mcp_transport::{
    approve_mcp_hook_tools, approved_mcp_tools, discover_mcp_hook_tools,
};
use patchbay_service::plugin_token::{issue_install_token, revoke_install_token};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::plugin_action::{plugin_error, plugins_enabled};
use crate::state::HandlerState;

/// Member-visible plugin reads. The Go router keeps `GET /plugins` outside the
/// owner/admin group so installed issue panels and hook actions remain usable.
pub fn member_router() -> Router<HandlerState> {
    Router::new().route("/api/workspaces/{id}/plugins", get(list_plugins))
}

/// Install, configure, token, and MCP-approval mutations stay admin-only.
pub fn admin_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/workspaces/{id}/plugins", post(install_plugin))
        .route("/api/workspaces/{id}/plugins/preview", post(preview_plugin))
        .route(
            "/api/workspaces/{id}/plugins/{installation_id}",
            axum::routing::delete(uninstall_plugin),
        )
        .route(
            "/api/workspaces/{id}/plugins/{installation_id}/config",
            axum::routing::put(configure_plugin),
        )
        .route(
            "/api/workspaces/{id}/plugins/{installation_id}/enable",
            post(enable_plugin),
        )
        .route(
            "/api/workspaces/{id}/plugins/{installation_id}/disable",
            post(disable_plugin),
        )
        .route(
            "/api/workspaces/{id}/plugins/{installation_id}/invocations",
            get(list_invocations),
        )
        .route(
            "/api/workspaces/{id}/plugins/{installation_id}/token",
            post(rotate_token).delete(revoke_token),
        )
        .route(
            "/api/workspaces/{id}/plugins/{installation_id}/mcp/{hook_key}/tools",
            get(list_mcp_tools).put(approve_mcp_tools),
        )
}

pub fn router() -> Router<HandlerState> {
    member_router().merge(admin_router())
}

fn require_enabled(state: &HandlerState) -> Result<(), Response> {
    if plugins_enabled(state) {
        Ok(())
    } else {
        Err(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Plugin management is not enabled",
        ))
    }
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace_id"))
}

#[derive(Serialize)]
struct HookResponse {
    key: String,
    name: String,
    description: String,
    triggers: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    events: Vec<String>,
    transport: String,
}

async fn installation_payload(
    state: &HandlerState,
    installation: &PluginInstallation,
) -> Result<Value, PluginError> {
    let manifest_bytes = serde_json::to_vec(&installation.manifest).map_err(|error| {
        PluginError::with_source(PluginErrorKind::Invalid, "encode stored manifest", error)
    })?;
    let manifest = parse_installation_manifest(&manifest_bytes).map_err(|error| {
        PluginError::with_source(
            PluginErrorKind::Invalid,
            "stored plugin manifest is unreadable",
            error,
        )
    })?;
    let configured_secrets = state
        .plugins
        .configured_secret_keys(installation.id)
        .await?;
    let granted_scopes =
        decode_scopes(&serde_json::to_vec(&installation.granted_scopes).unwrap_or_default())
            .unwrap_or_default();
    let config = installation.config.as_object().cloned().unwrap_or_default();
    let hooks = manifest
        .contributes
        .hooks
        .iter()
        .map(|hook| HookResponse {
            key: hook.key.clone(),
            name: hook.name.clone(),
            description: hook.description.clone(),
            triggers: hook.triggers.clone(),
            events: hook.events.clone(),
            transport: hook.transport.transport_type.clone(),
        })
        .collect::<Vec<_>>();
    let description_is_empty = manifest.description.is_empty();
    let mut payload = json!({
        "id": installation.id,
        "plugin_key": installation.plugin_key,
        "name": manifest.name,
        "description": manifest.description,
        "version": installation.version,
        "source_url": installation.source_url,
        "enabled": installation.enabled,
        "granted_scopes": granted_scopes,
        "config_schema": config_fields_for_manifest(&manifest),
        "config": config,
        "configured_secrets": configured_secrets,
        "surfaces": manifest.contributes.surfaces,
        "hooks": hooks,
        "resources": manifest.contributes.resources,
        "created_at": installation.created_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        "updated_at": installation.updated_at.to_rfc3339_opts(SecondsFormat::Secs, true),
    });
    if description_is_empty {
        payload
            .as_object_mut()
            .expect("installation payload is an object")
            .remove("description");
    }
    Ok(payload)
}

async fn installation_from_path(
    state: &HandlerState,
    context: &WorkspaceContext,
    installation_id: &str,
) -> Result<PluginInstallation, Response> {
    require_enabled(state)?;
    state
        .plugins
        .installation_for_workspace(workspace_id(context)?, installation_id)
        .await
        .map_err(|error| plugin_error(&error, "failed to load the Plugin"))
}

async fn list_plugins(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    if let Err(response) = require_enabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let installations = match patchbay_db::queries::plugin::list_workspace_plugin_installations(
        &state.pool,
        workspace_id,
    )
    .await
    {
        Ok(installations) => installations,
        Err(error) => {
            tracing::warn!(%error, "failed to list plugins");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list Plugins");
        }
    };
    let mut plugins = Vec::with_capacity(installations.len());
    for installation in &installations {
        match installation_payload(&state, installation).await {
            Ok(payload) => plugins.push(payload),
            Err(error) => {
                tracing::warn!(%error, "failed to render plugin installation");
                return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list Plugins");
            }
        }
    }
    Json(json!({ "plugins": plugins })).into_response()
}

#[derive(Deserialize)]
struct PreviewRequest {
    source_url: String,
}

async fn preview_plugin(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    body: Bytes,
) -> Response {
    if let Err(response) = require_enabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let request: PreviewRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    match state
        .plugins
        .preview_plugin(workspace_id, &request.source_url)
        .await
    {
        Ok(preview) => Json(preview).into_response(),
        Err(error) => plugin_error(&error, "failed to read the Plugin manifest"),
    }
}

#[derive(Deserialize)]
struct InstallRequest {
    source_url: String,
    #[serde(default)]
    granted_scopes: Vec<String>,
}

async fn install_plugin(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    body: Bytes,
) -> Response {
    if let Err(response) = require_enabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let request: InstallRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let installation = match state
        .plugins
        .install_plugin(
            workspace_id,
            context.member.user_id,
            &request.source_url,
            &request.granted_scopes,
        )
        .await
    {
        Ok(installation) => installation,
        Err(error) => return plugin_error(&error, "failed to install the Plugin"),
    };
    match installation_payload(&state, &installation).await {
        Ok(payload) => (StatusCode::CREATED, Json(payload)).into_response(),
        Err(error) => plugin_error(&error, "failed to install the Plugin"),
    }
}

#[derive(Deserialize)]
struct ConfigureRequest {
    #[serde(default)]
    values: serde_json::Map<String, Value>,
}

async fn configure_plugin(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, installation_id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let installation = match installation_from_path(&state, &context, &installation_id).await {
        Ok(installation) => installation,
        Err(response) => return response,
    };
    let request: ConfigureRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let updated = match state
        .plugins
        .set_config(&installation, &request.values)
        .await
    {
        Ok(updated) => updated,
        Err(error) => return plugin_error(&error, "failed to configure the Plugin"),
    };
    match installation_payload(&state, &updated).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => plugin_error(&error, "failed to configure the Plugin"),
    }
}

async fn set_enabled(
    state: HandlerState,
    context: WorkspaceContext,
    installation_id: String,
    enabled: bool,
) -> Response {
    let installation = match installation_from_path(&state, &context, &installation_id).await {
        Ok(installation) => installation,
        Err(response) => return response,
    };
    let updated = match state.plugins.set_enabled(&installation, enabled).await {
        Ok(updated) => updated,
        Err(error) => return plugin_error(&error, "failed to update the Plugin"),
    };
    match installation_payload(&state, &updated).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => plugin_error(&error, "failed to update the Plugin"),
    }
}

async fn enable_plugin(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, installation_id)): Path<(String, String)>,
) -> Response {
    set_enabled(state, context, installation_id, true).await
}

async fn disable_plugin(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, installation_id)): Path<(String, String)>,
) -> Response {
    set_enabled(state, context, installation_id, false).await
}

async fn uninstall_plugin(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, installation_id)): Path<(String, String)>,
) -> Response {
    let installation = match installation_from_path(&state, &context, &installation_id).await {
        Ok(installation) => installation,
        Err(response) => return response,
    };
    match state.plugins.uninstall(&installation).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => plugin_error(&error, "failed to uninstall the Plugin"),
    }
}

async fn list_invocations(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, installation_id)): Path<(String, String)>,
) -> Response {
    let installation = match installation_from_path(&state, &context, &installation_id).await {
        Ok(installation) => installation,
        Err(response) => return response,
    };
    match patchbay_db::queries::plugin::list_plugin_invocations(&state.pool, installation.id, 100)
        .await
    {
        Ok(rows) => Json(json!({
            "invocations": rows.into_iter().map(InvocationResponse::from).collect::<Vec<_>>()
        }))
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to load plugin activity");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load the Plugin activity",
            )
        }
    }
}

#[derive(Serialize)]
struct InvocationResponse {
    id: String,
    hook_key: String,
    trigger: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_type: Option<String>,
    attempt: i32,
    latency_ms: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    created_at: String,
}

impl From<patchbay_db::models::PluginInvocation> for InvocationResponse {
    fn from(row: patchbay_db::models::PluginInvocation) -> Self {
        Self {
            id: row.id.to_string(),
            hook_key: row.hook_key,
            trigger: row.trigger,
            status: row.status,
            event_type: row.event_type,
            attempt: row.attempt,
            latency_ms: row.latency_ms,
            error: row.error,
            created_at: row.created_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        }
    }
}

async fn rotate_token(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, installation_id)): Path<(String, String)>,
) -> Response {
    let installation = match installation_from_path(&state, &context, &installation_id).await {
        Ok(installation) => installation,
        Err(response) => return response,
    };
    let token = match issue_install_token(&state.pool, installation.id).await {
        Ok(token) => token,
        Err(error) => return plugin_error(&error, "failed to issue the Plugin token"),
    };
    let signing_secret = match hook_signing_secret(&state.plugins.deployment_key, installation.id) {
        Ok(secret) => secret,
        Err(error) => return plugin_error(&error, "failed to derive the signing secret"),
    };
    Json(json!({ "token": token, "signing_secret": signing_secret })).into_response()
}

async fn revoke_token(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, installation_id)): Path<(String, String)>,
) -> Response {
    let installation = match installation_from_path(&state, &context, &installation_id).await {
        Ok(installation) => installation,
        Err(response) => return response,
    };
    match revoke_install_token(&state.pool, installation.id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => plugin_error(&error, "failed to revoke the Plugin token"),
    }
}

async fn list_mcp_tools(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, installation_id, hook_key)): Path<(String, String, String)>,
) -> Response {
    let installation = match installation_from_path(&state, &context, &installation_id).await {
        Ok(installation) => installation,
        Err(response) => return response,
    };
    let discovered = match discover_mcp_hook_tools(
        &state.pool,
        state.plugins.secrets.as_ref(),
        &installation,
        &hook_key,
    )
    .await
    {
        Ok(tools) => tools,
        Err(error) => return plugin_error(&error, "failed to reach the Plugin's MCP server"),
    };
    let approved = approved_mcp_tools(&installation, &hook_key);
    Json(json!({
        "tools": discovered.into_iter().map(|tool| {
            let pinned = approved.get(&tool.name);
            let drifted = pinned.is_some_and(|pinned| pinned.schema_digest != tool.schema_digest);
            McpToolResponse {
                name: tool.name,
                description: tool.description,
                schema_digest: tool.schema_digest,
                approved: pinned.is_some(),
                drifted,
            }
        }).collect::<Vec<_>>()
    }))
    .into_response()
}

#[derive(Serialize)]
struct McpToolResponse {
    name: String,
    description: String,
    schema_digest: String,
    approved: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    drifted: bool,
}

#[derive(Deserialize)]
struct ApproveToolsRequest {
    #[serde(default)]
    tools: Vec<String>,
}

async fn approve_mcp_tools(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, installation_id, hook_key)): Path<(String, String, String)>,
    body: Bytes,
) -> Response {
    let installation = match installation_from_path(&state, &context, &installation_id).await {
        Ok(installation) => installation,
        Err(response) => return response,
    };
    let request: ApproveToolsRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let updated = match approve_mcp_hook_tools(
        &state.pool,
        state.plugins.secrets.as_ref(),
        &installation,
        &hook_key,
        &request.tools,
        Some(context.member.user_id),
    )
    .await
    {
        Ok(updated) => updated,
        Err(error) => return plugin_error(&error, "failed to approve the MCP tools"),
    };
    match installation_payload(&state, &updated).await {
        Ok(payload) => Json(payload).into_response(),
        Err(error) => plugin_error(&error, "failed to load the Plugin"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt as _;

    fn state() -> HandlerState {
        HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            patchbay_auth::pat_cache::PatCache::disabled(),
            None,
        )
    }

    #[tokio::test]
    async fn plugin_list_is_on_the_member_router_not_the_admin_router() {
        let workspace = "018f03a0-c4d2-7a37-ae4d-5aa45de12f11";
        let uri = format!("/api/workspaces/{workspace}/plugins");

        let member_post = member_router()
            .with_state(state())
            .oneshot(
                Request::post(&uri)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(member_post.status(), StatusCode::METHOD_NOT_ALLOWED);

        let admin_get = admin_router()
            .with_state(state())
            .oneshot(Request::get(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(admin_get.status(), StatusCode::METHOD_NOT_ALLOWED);

        let member_get = member_router()
            .with_state(state())
            .oneshot(Request::get(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_ne!(member_get.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_ne!(member_get.status(), StatusCode::NOT_FOUND);
    }
}
