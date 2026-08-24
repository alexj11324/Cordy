//! Agent assignments for the workspace MCP server library.
//!
//! The library entries remain workspace-owned and write-only. These routes
//! expose only their non-secret summaries and the per-agent enabled toggle.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use cordy_db::models::Agent;
use cordy_db::queries::workspace_mcp::ListAgentMcpServersRow;
use cordy_db::queries::{agent, workspace, workspace_mcp};
use cordy_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/agents/{id}/mcp-servers", get(list).post(add))
        .route(
            "/api/agents/{id}/mcp-servers/{server_id}/enabled",
            axum::routing::put(set_enabled),
        )
        .route(
            "/api/agents/{id}/mcp-servers/{server_id}",
            axum::routing::delete(remove),
        )
}

#[derive(Debug, Serialize)]
struct ServerResponse {
    id: String,
    workspace_id: String,
    name: String,
    transport: String,
    enabled: bool,
    created_at: String,
    updated_at: String,
}

fn transport_of(config: Option<&Value>) -> String {
    let Some(entry) = config.and_then(Value::as_object) else {
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

impl From<ListAgentMcpServersRow> for ServerResponse {
    fn from(row: ListAgentMcpServersRow) -> Self {
        Self {
            id: row.id.map(|id| id.to_string()).unwrap_or_default(),
            workspace_id: row
                .workspace_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            name: row.name,
            transport: transport_of(row.config.as_ref()),
            enabled: row.enabled,
            created_at: row
                .created_at
                .map(crate::timefmt::rfc3339)
                .unwrap_or_default(),
            updated_at: row
                .updated_at
                .map(crate::timefmt::rfc3339)
                .unwrap_or_default(),
        }
    }
}

fn parse_id(raw: &str, field: &str) -> Result<Uuid, Response> {
    Uuid::parse_str(raw.trim())
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, &format!("invalid {field}")))
}

async fn load_agent(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_agent_id: &str,
) -> Result<Agent, Response> {
    let agent_id = parse_id(raw_agent_id, "agent id")?;
    let workspace_id = Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace id"))?;
    match agent::get_agent_in_workspace(&state.pool, agent_id, workspace_id).await {
        Ok(Some(agent)) => Ok(agent),
        Ok(None) | Err(_) => Err(error_response(StatusCode::NOT_FOUND, "agent not found")),
    }
}

async fn is_agent_actor(state: &HandlerState, headers: &HeaderMap, workspace_id: Uuid) -> bool {
    if headers
        .get("x-actor-source")
        .and_then(|value| value.to_str().ok())
        == Some("task_token")
    {
        return true;
    }
    let Some(agent_id) = headers
        .get("x-agent-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
    else {
        return false;
    };
    let Some(task_id) = headers
        .get("x-task-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
    else {
        return false;
    };
    let Ok(Some(actor)) = agent::get_agent(&state.pool, agent_id).await else {
        return false;
    };
    if actor.workspace_id != workspace_id {
        return false;
    }
    matches!(
        agent::get_agent_task(&state.pool, task_id).await,
        Ok(Some(task)) if task.agent_id == agent_id
    )
}

async fn require_writer(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    raw_agent_id: &str,
) -> Result<Agent, Response> {
    let found = load_agent(state, context, raw_agent_id).await?;
    if is_agent_actor(state, headers, found.workspace_id).await {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "agents cannot modify MCP server assignments",
        ));
    }
    if !matches!(context.member.role.as_str(), "owner" | "admin")
        && found.owner_id != Some(context.member.user_id)
    {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "insufficient permissions",
        ));
    }
    Ok(found)
}

async fn list_for_agent(state: &HandlerState, agent_id: Uuid) -> Response {
    match workspace_mcp::list_agent_mcp_servers(&state.pool, agent_id).await {
        Ok(rows) => Json(
            rows.into_iter()
                .map(ServerResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, %agent_id, "failed to list agent MCP servers");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list the agent's MCP servers",
            )
        }
    }
}

async fn list(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_agent_id): Path<String>,
) -> Response {
    match load_agent(&state, &context, &raw_agent_id).await {
        Ok(found) => list_for_agent(&state, found.id).await,
        Err(response) => response,
    }
}

#[derive(Debug, Default, Deserialize)]
struct AddRequest {
    #[serde(default)]
    server_id: String,
}

fn decode_first<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, Response> {
    let mut decoder = serde_json::Deserializer::from_slice(body);
    T::deserialize(&mut decoder)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid request body"))
}

async fn add(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(raw_agent_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let found = match require_writer(&state, &context, &headers, &raw_agent_id).await {
        Ok(found) => found,
        Err(response) => return response,
    };
    let request = match decode_first::<Option<AddRequest>>(&body) {
        Ok(request) => request.unwrap_or_default(),
        Err(response) => return response,
    };
    let server_id = match parse_id(&request.server_id, "server_id") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to add the MCP server",
            )
        }
    };
    if workspace::lock_workspace_for_chat_session_create(&mut *transaction, found.workspace_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return error_response(StatusCode::NOT_FOUND, "workspace not found");
    }
    if workspace_mcp::lock_workspace_mcp_server_for_share(
        &mut *transaction,
        server_id,
        found.workspace_id,
    )
    .await
    .ok()
    .flatten()
    .is_none()
    {
        return error_response(
            StatusCode::NOT_FOUND,
            "MCP server not found in this workspace",
        );
    }
    if workspace_mcp::add_agent_mcp_server(&mut *transaction, found.id, server_id)
        .await
        .is_err()
        || transaction.commit().await.is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to add the MCP server",
        );
    }
    list_for_agent(&state, found.id).await
}

#[derive(Debug, Deserialize)]
struct EnabledRequest {
    enabled: Option<bool>,
}

async fn set_enabled(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_agent_id, raw_server_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let found = match require_writer(&state, &context, &headers, &raw_agent_id).await {
        Ok(found) => found,
        Err(response) => return response,
    };
    let server_id = match parse_id(&raw_server_id, "server id") {
        Ok(id) => id,
        Err(response) => return response,
    };
    let request = match decode_first::<EnabledRequest>(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let Some(enabled) = request.enabled else {
        return error_response(StatusCode::BAD_REQUEST, "enabled is required");
    };
    match workspace_mcp::set_agent_mcp_server_enabled(&state.pool, found.id, server_id, enabled)
        .await
    {
        Ok(0) => error_response(
            StatusCode::NOT_FOUND,
            "this MCP server is not assigned to the agent",
        ),
        Ok(_) => list_for_agent(&state, found.id).await,
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update the MCP server",
        ),
    }
}

async fn remove(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((raw_agent_id, raw_server_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let found = match require_writer(&state, &context, &headers, &raw_agent_id).await {
        Ok(found) => found,
        Err(response) => return response,
    };
    let server_id = match parse_id(&raw_server_id, "server id") {
        Ok(id) => id,
        Err(response) => return response,
    };
    match workspace_mcp::remove_agent_mcp_server(&state.pool, found.id, server_id).await {
        Ok(0) => error_response(
            StatusCode::NOT_FOUND,
            "this MCP server is not assigned to the agent",
        ),
        Ok(_) => list_for_agent(&state, found.id).await,
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to remove the MCP server",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn transport_classification_is_non_lossy() {
        assert_eq!(
            transport_of(Some(&serde_json::json!({"type":"local"}))),
            "stdio"
        );
        assert_eq!(
            transport_of(Some(&serde_json::json!({"type":"sse","url":"secret"}))),
            "sse"
        );
        assert_eq!(
            transport_of(Some(&serde_json::json!({"command":"npx"}))),
            "stdio"
        );
        assert_eq!(transport_of(None), "unknown");
    }

    #[test]
    fn response_never_serializes_secret_config() {
        let response = ServerResponse::from(ListAgentMcpServersRow {
            id: Some(Uuid::nil()),
            workspace_id: Some(Uuid::nil()),
            name: "secret-server".into(),
            config: Some(serde_json::json!({"url":"https://token@example.com"})),
            created_at: Some(Utc::now()),
            updated_at: Some(Utc::now()),
            enabled: true,
        });
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(!encoded.contains("token@example.com"));
        assert!(!encoded.contains("config"));
        assert!(encoded.contains("\"transport\":\"http\""));
    }

    #[test]
    fn writer_gate_matches_owner_admin_and_agent_owner_rule() {
        fn can_write(role: &str, owner_id: Option<Uuid>, user_id: Uuid) -> bool {
            matches!(role, "owner" | "admin") || owner_id == Some(user_id)
        }
        let user = Uuid::new_v4();
        assert!(can_write("owner", None, user));
        assert!(can_write("admin", None, user));
        assert!(can_write("member", Some(user), user));
        assert!(!can_write("member", Some(Uuid::new_v4()), user));
    }

    #[test]
    fn decoder_matches_go_first_value_and_null_toggle_behavior() {
        let add = decode_first::<AddRequest>(br#"{"server_id":"first"} {"server_id":"ignored"}"#)
            .unwrap();
        assert_eq!(add.server_id, "first");
        let toggle = decode_first::<EnabledRequest>(br#"{"enabled":null}"#).unwrap();
        assert!(toggle.enabled.is_none());
    }
}
