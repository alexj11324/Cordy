//! Workspace MCP server library routes.
//!
//! Config is intentionally write-only: every response exposes only identity,
//! display transport, and timestamps so credentials cannot leak through the
//! settings API or logs.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use patchbay_db::models::WorkspaceMcpServer;
use patchbay_db::queries::{workspace, workspace_mcp};
use patchbay_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn member_router() -> Router<HandlerState> {
    Router::new().route("/api/workspaces/{id}/mcp-servers", get(list_servers))
}

pub fn admin_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/workspaces/{id}/mcp-servers", post(create_server))
        .route(
            "/api/workspaces/{id}/mcp-servers/{server_id}",
            axum::routing::put(update_server).delete(delete_server),
        )
}

#[derive(Debug, Serialize)]
struct ServerResponse {
    id: String,
    workspace_id: String,
    name: String,
    transport: String,
    created_at: String,
    updated_at: String,
}

impl From<WorkspaceMcpServer> for ServerResponse {
    fn from(server: WorkspaceMcpServer) -> Self {
        Self {
            id: server.id.to_string(),
            workspace_id: server.workspace_id.to_string(),
            name: server.name,
            transport: transport_of(&server.config),
            created_at: crate::timefmt::rfc3339(server.created_at),
            updated_at: crate::timefmt::rfc3339(server.updated_at),
        }
    }
}

fn transport_of(config: &Value) -> String {
    let Some(entry) = config.as_object() else {
        return "unknown".into();
    };
    if let Some(declared) = entry
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return match declared.to_ascii_lowercase().as_str() {
            "local" | "stdio" => "stdio".into(),
            "remote" | "http" | "streamable-http" => "http".into(),
            other => other.to_string(),
        };
    }
    if entry.contains_key("command") {
        "stdio".into()
    } else if entry.contains_key("url") {
        "http".into()
    } else {
        "unknown".into()
    }
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace id"))
}

fn reject_agent(headers: &HeaderMap) -> Result<(), Response> {
    if headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        == Some("task_token")
    {
        Err(error_response(
            StatusCode::FORBIDDEN,
            "agents cannot modify the workspace MCP servers",
        ))
    } else {
        Ok(())
    }
}

fn server_id(raw: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(raw).map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid server id"))
}

fn validate_name(raw: &str) -> Result<String, Response> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(error_response(StatusCode::BAD_REQUEST, "name is required"));
    }
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "name may only contain letters, digits, hyphens, and underscores",
        ));
    }
    Ok(name.to_string())
}

fn validate_config(config: &Value) -> Result<(), Response> {
    match config.as_object() {
        Some(entry) if !entry.is_empty() => Ok(()),
        Some(_) => Err(error_response(
            StatusCode::BAD_REQUEST,
            "config must not be empty",
        )),
        None => Err(error_response(
            StatusCode::BAD_REQUEST,
            "config must be a JSON object",
        )),
    }
}

fn unique_violation(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|error| error.code())
        .is_some_and(|code| code == "23505")
}

async fn list_servers(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    match workspace_mcp::list_workspace_mcp_servers(&state.pool, id).await {
        Ok(servers) => Json(
            servers
                .into_iter()
                .map(ServerResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, workspace_id = %id, "failed to list workspace MCP servers");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list workspace MCP servers",
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct ServerRequest {
    name: String,
    config: Value,
}

async fn create_server(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = reject_agent(&headers) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let request: ServerRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let name = match validate_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response,
    };
    if let Err(response) = validate_config(&request.config) {
        return response;
    }
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create the MCP server",
            )
        }
    };
    if workspace::lock_workspace_for_chat_session_create(&mut *transaction, workspace_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return error_response(StatusCode::NOT_FOUND, "workspace not found");
    }
    let created = workspace_mcp::create_workspace_mcp_server(
        &mut *transaction,
        workspace_id,
        &name,
        &request.config,
        context.member.user_id,
    )
    .await;
    let created = match created {
        Ok(Some(server)) => server,
        Err(error) if unique_violation(&error) => {
            return error_response(
                StatusCode::CONFLICT,
                "an MCP server with this name already exists in the workspace",
            )
        }
        _ => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create the MCP server",
            )
        }
    };
    if transaction.commit().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create the MCP server",
        );
    }
    (StatusCode::CREATED, Json(ServerResponse::from(created))).into_response()
}

#[derive(Debug, Deserialize)]
struct UpdateRequest {
    #[serde(default)]
    name: String,
    config: Option<Value>,
}

async fn update_server(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, raw_server_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = reject_agent(&headers) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let server_id = match server_id(&raw_server_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let request: UpdateRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let name = if request.name.trim().is_empty() {
        None
    } else {
        match validate_name(&request.name) {
            Ok(name) => Some(name),
            Err(response) => return response,
        }
    };
    if let Some(config) = request.config.as_ref() {
        if let Err(response) = validate_config(config) {
            return response;
        }
    }
    match workspace_mcp::update_workspace_mcp_server(
        &state.pool,
        server_id,
        workspace_id,
        name.as_deref(),
        request.config.as_ref(),
    )
    .await
    {
        Ok(Some(server)) => Json(ServerResponse::from(server)).into_response(),
        Err(error) if unique_violation(&error) => error_response(
            StatusCode::CONFLICT,
            "an MCP server with this name already exists in the workspace",
        ),
        Ok(None) | Err(_) => error_response(StatusCode::NOT_FOUND, "MCP server not found"),
    }
}

async fn delete_server(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, raw_server_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = reject_agent(&headers) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let server_id = match server_id(&raw_server_id) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete the MCP server",
            )
        }
    };
    if workspace_mcp::lock_workspace_mcp_server_for_update(
        &mut *transaction,
        server_id,
        workspace_id,
    )
    .await
    .ok()
    .flatten()
    .is_none()
    {
        return error_response(StatusCode::NOT_FOUND, "MCP server not found");
    }
    match workspace_mcp::delete_workspace_mcp_server(&mut *transaction, server_id, workspace_id)
        .await
    {
        Ok(1..) => {}
        Ok(0) => return error_response(StatusCode::NOT_FOUND, "MCP server not found"),
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to delete the MCP server",
            )
        }
    }
    if workspace_mcp::delete_agent_mcp_servers_by_server(&mut *transaction, server_id)
        .await
        .is_err()
        || transaction.commit().await.is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to delete the MCP server",
        );
    }
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_classification_preserves_unknown_explicit_types() {
        assert_eq!(transport_of(&serde_json::json!({"type":"local"})), "stdio");
        assert_eq!(
            transport_of(&serde_json::json!({"type":"sse","url":"x"})),
            "sse"
        );
        assert_eq!(transport_of(&serde_json::json!({"command":"npx"})), "stdio");
        assert_eq!(
            transport_of(&serde_json::json!({"url":"https://x"})),
            "http"
        );
    }

    #[test]
    fn validation_never_echoes_config_contents() {
        let response = validate_config(&Value::String("secret-token".into())).unwrap_err();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
