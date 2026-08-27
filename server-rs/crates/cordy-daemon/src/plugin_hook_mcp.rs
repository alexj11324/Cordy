//! Port of `server/internal/daemon/plugin_hook_mcp.go` (246 lines).
//!
//! Symbol map (Go → Rust):
//! - `pluginHookMCPProtocolVersion` / `pluginHookMCPMaxRequestBytes` /
//!   `pluginHookMCPCallTimeout` → [`PLUGIN_HOOK_MCP_PROTOCOL_VERSION`] /
//!   [`PLUGIN_HOOK_MCP_MAX_REQUEST_BYTES`] / [`PLUGIN_HOOK_MCP_CALL_TIMEOUT`]
//! - `pluginHookInvoker` → [`PluginHookInvoker`]
//! - `pluginHookMCPServer` (+ ServeHTTP/handleCall/toolDescriptors) →
//!   [`PluginHookMCPState`] + [`serve_plugin_hook_request`]
//! - `pluginHookMCPSet` → [`PluginHookMCPSet`]
//! - `startTaskPluginHookMCP` → [`start_task_plugin_hook_mcp`]
//!
//! Port notes: unlike the Remote MCP broker there is no upstream MCP server
//! to proxy to — the protocol is synthesised from the manifest. A tool call
//! does NOT go to the plugin from here; it goes back to Cordy via `invoke`
//! (the signing secret never reaches the daemon). The HTTP layer is axum;
//! the production provider adapter owns one server for the task lifetime.

use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use axum::extract::{Request, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};

use crate::types::PluginHookTool;

pub(crate) const PLUGIN_HOOK_MCP_PROTOCOL_VERSION: &str = "2024-11-05";
pub(crate) const PLUGIN_HOOK_MCP_MAX_REQUEST_BYTES: usize = 1 << 20;
pub(crate) const PLUGIN_HOOK_MCP_CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Performs one hook call against the Cordy server (go:37): returns the raw
/// JSON output or an error that becomes a TOOL-level failure for the agent.
pub(crate) type PluginHookInvoker = Arc<
    dyn for<'a> Fn(
            &'a crate::repocache::Ctx,
            &str,
            &str,
            &str,
            &Value,
        ) -> BoxFuture<'a, anyhow::Result<Value>>
        + Send
        + Sync,
>;

#[derive(Clone)]
pub(crate) struct PluginHookMCPState {
    ctx: crate::repocache::Ctx,
    task_id: String,
    tools: Vec<PluginHookTool>,
    by_name: std::collections::BTreeMap<String, PluginHookTool>,
    invoke: PluginHookInvoker,
    path: String,
}

impl PluginHookMCPState {
    fn new(
        ctx: crate::repocache::Ctx,
        task_id: String,
        tools: Vec<PluginHookTool>,
        invoke: PluginHookInvoker,
        path: String,
    ) -> Self {
        let mut by_name = std::collections::BTreeMap::new();
        for tool in &tools {
            // The server namespaces these, but a duplicate arriving anyway
            // must resolve to exactly one hook rather than whichever came
            // last (go:80–88).
            match by_name.entry(tool.name.clone()) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(tool.clone());
                }
                std::collections::btree_map::Entry::Occupied(_) => {
                    tracing::warn!(
                        task_id = %task_id,
                        tool = %tool.name,
                        "plugin hook tool name collided; ignoring the duplicate"
                    );
                }
            }
        }
        Self {
            ctx,
            task_id,
            tools,
            by_name,
            invoke,
            path,
        }
    }
}

async fn plugin_hook_fallback() -> Response {
    HookResponse::empty(StatusCode::NOT_FOUND).into_response()
}

/// Idempotent shutdown handle for one task's server (go:48–68).
#[derive(Default)]
pub(crate) struct PluginHookMCPSet {
    shutdown: Option<tokio_util::sync::CancellationToken>,
    once: Option<std::sync::Once>,
}

impl PluginHookMCPSet {
    pub(crate) fn close(&mut self) {
        let once = self.once.get_or_insert_with(std::sync::Once::new);
        once.call_once(|| {
            if let Some(token) = &self.shutdown {
                token.cancel();
            }
        });
    }
}

impl Drop for PluginHookMCPSet {
    fn drop(&mut self) {
        self.close();
    }
}

/// `startTaskPluginHookMCP` (go:74–130): starts the tool server for one task
/// if it has tools; returns the MCP config fragment to merge into the agent's.
pub(crate) async fn start_task_plugin_hook_mcp(
    lifetime_ctx: &crate::repocache::Ctx,
    task_id: &str,
    tools: &[PluginHookTool],
    invoke: PluginHookInvoker,
) -> anyhow::Result<(Option<Value>, Option<PluginHookMCPSet>)> {
    if tools.is_empty() {
        return Ok((None, None));
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|err| anyhow!("listen for plugin hook MCP server: {err}"))?;
    let addr = listener
        .local_addr()
        .map_err(|err| anyhow!("local addr: {err}"))?;
    let path_token = crate::remote_mcp_broker::random_broker_token()?;
    let path = format!("/{path_token}");

    let state = Arc::new(PluginHookMCPState::new(
        lifetime_ctx.child(),
        task_id.to_string(),
        tools.to_vec(),
        invoke,
        path.clone(),
    ));
    let app = axum::Router::new()
        .route(
            &path,
            axum::routing::post(plugin_hook_handler).fallback(plugin_hook_fallback),
        )
        .with_state(Arc::clone(&state));
    let shutdown = tokio_util::sync::CancellationToken::new();
    let serve_shutdown = shutdown.clone();
    let serve_task_id = task_id.to_string();
    tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            serve_shutdown.cancelled().await;
        });
        if let Err(serve_err) = server.await {
            tracing::warn!(
                task_id = %serve_task_id,
                error = %serve_err,
                "plugin hook MCP server stopped unexpectedly"
            );
        }
    });
    // Close when the task's lifetime context ends (go:114–117).
    {
        let lifetime_token = lifetime_ctx.token().clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = lifetime_token.cancelled() => shutdown.cancel(),
                _ = shutdown.cancelled() => {}
            }
        });
    }

    Ok((
        Some(json!({
            "mcpServers": {
                "cordy-plugins": {
                    "type": "http",
                    "url": format!("http://{addr}{path}"),
                }
            }
        })),
        Some(PluginHookMCPSet {
            shutdown: Some(shutdown),
            once: None,
        }),
    ))
}

/// Rendered HTTP response of [`serve_plugin_hook_request`].
pub(crate) struct HookResponse {
    pub status: StatusCode,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl HookResponse {
    fn json(status: StatusCode, value: &Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: value.to_string().into_bytes(),
        }
    }

    fn empty(status: StatusCode) -> Self {
        Self {
            status,
            content_type: "",
            body: Vec::new(),
        }
    }

    fn into_response(self) -> Response {
        let mut response = (self.status, self.body).into_response();
        if !self.content_type.is_empty() {
            if let Ok(value) = self.content_type.parse() {
                response.headers_mut().insert("Content-Type", value);
            }
        }
        response
    }
}

fn write_result(id: Option<&Value>, result: &Value) -> HookResponse {
    HookResponse::json(
        StatusCode::OK,
        &json!({"jsonrpc": "2.0", "id": id.unwrap_or(&Value::Null), "result": result}),
    )
}

fn write_error(id: Option<&Value>, code: i64, message: &str) -> HookResponse {
    HookResponse::json(
        StatusCode::OK,
        &json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(&Value::Null),
            "error": {"code": code, "message": message},
        }),
    )
}

async fn plugin_hook_handler(
    State(state): State<Arc<PluginHookMCPState>>,
    request: Request,
) -> Response {
    let method = request.method().clone();
    let uri: Uri = request.uri().clone();
    let max = PLUGIN_HOOK_MCP_MAX_REQUEST_BYTES;
    let body = axum::body::to_bytes(request.into_body(), max)
        .await
        .map(|bytes| bytes.to_vec())
        .unwrap_or_else(|_| Vec::new());
    serve_plugin_hook_request(&state, method.as_str(), uri.path(), &body)
        .await
        .into_response()
}

/// `pluginHookMCPServer.ServeHTTP` (go:139–174) + `handleCall`
/// (go:194–233). The path is a per-task random token, so a process that
/// merely knows the port cannot reach the tools. A tool failure is a TOOL
/// error (isError content the agent reads and carries on from), never a
/// protocol error.
pub(crate) async fn serve_plugin_hook_request(
    state: &PluginHookMCPState,
    http_method: &str,
    path: &str,
    body: &[u8],
) -> HookResponse {
    if path != state.path || http_method != "POST" {
        return HookResponse::empty(StatusCode::NOT_FOUND);
    }
    if body.len() > PLUGIN_HOOK_MCP_MAX_REQUEST_BYTES {
        return write_error(None, -32700, "could not read the request");
    }
    let request: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return write_error(None, -32700, "request is not valid JSON-RPC"),
    };
    let id = request.get("id").cloned();
    let id_ref = id.as_ref();
    let rpc_method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(Value::Null);

    match rpc_method {
        "initialize" => write_result(
            id_ref,
            &json!({
                "protocolVersion": PLUGIN_HOOK_MCP_PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "cordy-plugins", "version": "1"},
            }),
        ),
        // A notification has no id and takes no reply (go:164–166).
        "notifications/initialized" => HookResponse::empty(StatusCode::ACCEPTED),
        "tools/list" => {
            let descriptors: Vec<Value> = state
                .tools
                .iter()
                .map(|tool| {
                    let schema = match &tool.input_schema {
                        Some(schema) if !schema.is_null() => schema.clone(),
                        _ => json!({"type": "object", "properties": {}}),
                    };
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": schema,
                    })
                })
                .collect();
            write_result(id_ref, &json!({"tools": descriptors}))
        }
        "tools/call" => handle_call(state, id_ref, &params).await,
        other => write_error(id_ref, -32601, &format!("unsupported method {other}")),
    }
}

async fn handle_call(
    state: &PluginHookMCPState,
    id: Option<&Value>,
    params: &Value,
) -> HookResponse {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    let Some(tool) = state.by_name.get(name) else {
        return write_error(id, -32602, &format!("unknown tool {name}"));
    };

    let call = (state.invoke)(
        &state.ctx,
        &state.task_id.clone(),
        &tool.installation_id.clone(),
        &tool.hook_key.clone(),
        &arguments,
    );
    let output = match tokio::time::timeout(PLUGIN_HOOK_MCP_CALL_TIMEOUT, call).await {
        Ok(result) => result,
        Err(_) => Err(anyhow!("context deadline exceeded")),
    };
    match output {
        Ok(output) => {
            let text = if output.is_null() {
                "The hook completed and returned nothing.".to_string()
            } else {
                output.to_string()
            };
            write_result(id, &json!({"content": [{"type": "text", "text": text}]}))
        }
        // A TOOL error, not a protocol error: an unreachable plugin endpoint
        // must not fail somebody's issue (go:212–224).
        Err(err) => {
            tracing::info!(
                task_id = %state.task_id,
                tool = %tool.name,
                error = %err,
                "plugin hook tool call failed"
            );
            write_result(
                id,
                &json!({"isError": true, "content": [{"type": "text", "text": err.to_string()}]}),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_tools() -> Vec<PluginHookTool> {
        vec![PluginHookTool {
            installation_id: "install-1".into(),
            hook_key: "issue_assigned".into(),
            name: "fixture.read".into(),
            description: "Read a deterministic fixture value".into(),
            input_schema: Some(json!({"type": "object", "properties": {}})),
        }]
    }

    #[test]
    fn dropping_server_set_closes_its_listener_token() {
        let shutdown = tokio_util::sync::CancellationToken::new();
        let observed = shutdown.clone();
        drop(PluginHookMCPSet {
            shutdown: Some(shutdown),
            once: None,
        });
        assert!(observed.is_cancelled());
    }

    #[tokio::test]
    async fn started_server_exposes_claim_tool_through_its_overlay_url() {
        let ctx = crate::repocache::Ctx::new();
        let (config, server) =
            start_task_plugin_hook_mcp(&ctx, "task-1", &fixture_tools(), ok_invoke())
                .await
                .unwrap();
        let url = config.unwrap()["mcpServers"]["cordy-plugins"]["url"]
            .as_str()
            .unwrap()
            .to_string();
        let response: Value = reqwest::Client::new()
            .post(url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {"name": "fixture.read", "arguments": {}}
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        assert_eq!(response["result"]["content"][0]["text"], "{\"value\":42}");
        drop(server);
    }

    fn ok_invoke() -> PluginHookInvoker {
        Arc::new(
            |_ctx: &crate::repocache::Ctx,
             _task: &str,
             _inst: &str,
             _hook: &str,
             _input: &Value| { Box::pin(async { Ok(json!({"value": 42})) }) },
        )
    }

    fn failing_invoke(message: &'static str) -> PluginHookInvoker {
        Arc::new(
            move |_ctx: &crate::repocache::Ctx,
                  _task: &str,
                  _inst: &str,
                  _hook: &str,
                  _input: &Value| {
                Box::pin(
                    async move { Err::<Value, _>(anyhow!("{message}")) as anyhow::Result<Value> },
                )
            },
        )
    }

    fn state_with(invoke: PluginHookInvoker) -> (PluginHookMCPState, String) {
        let path = format!(
            "/{}",
            crate::remote_mcp_broker::random_broker_token().unwrap()
        );
        let token = path.trim_start_matches('/').to_string();
        (
            PluginHookMCPState::new(
                crate::repocache::Ctx::new(),
                "task-1".into(),
                fixture_tools(),
                invoke,
                path,
            ),
            token,
        )
    }

    async fn post(state: &PluginHookMCPState, body: Value) -> HookResponse {
        serve_plugin_hook_request(
            state,
            "POST",
            &state.path.clone(),
            body.to_string().as_bytes(),
        )
        .await
    }

    #[tokio::test]
    async fn tool_failure_is_a_tool_error_not_a_transport_error() {
        let (state, _) = state_with(failing_invoke("connection refused"));
        let response = post(
            &state,
            json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"fixture.read","arguments":{}}}),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK);
        let decoded: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(decoded["id"], 7);
        assert_eq!(decoded["result"]["isError"], true);
        assert!(decoded["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("connection refused"));
    }

    #[tokio::test]
    async fn duplicate_tool_name_routes_to_the_first_installation_and_hook() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let invoke: PluginHookInvoker = {
            let calls = Arc::clone(&calls);
            Arc::new(move |_ctx, _task, installation, hook, _input| {
                calls
                    .lock()
                    .unwrap()
                    .push((installation.to_string(), hook.to_string()));
                Box::pin(async { Ok(json!({"selected": true})) })
            })
        };
        let tools = vec![
            PluginHookTool {
                installation_id: "first-installation".into(),
                hook_key: "first-hook".into(),
                name: "duplicate".into(),
                ..PluginHookTool::default()
            },
            PluginHookTool {
                installation_id: "second-installation".into(),
                hook_key: "second-hook".into(),
                name: "duplicate".into(),
                ..PluginHookTool::default()
            },
        ];
        let state = PluginHookMCPState::new(
            crate::repocache::Ctx::new(),
            "task-1".into(),
            tools,
            invoke,
            "/token".into(),
        );

        let response = post(
            &state,
            json!({
                "jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":"duplicate","arguments":{}}
            }),
        )
        .await;
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![("first-installation".into(), "first-hook".into())]
        );
    }

    #[tokio::test]
    async fn empty_hook_output_becomes_the_nothing_note() {
        let invoke: PluginHookInvoker = Arc::new(
            |_a: &crate::repocache::Ctx, _b: &str, _c: &str, _d: &str, _e: &Value| {
                Box::pin(async { Ok(Value::Null) })
            },
        );
        let (state, _) = state_with(invoke);
        let response = post(
            &state,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"fixture.read"}}),
        )
        .await;
        let decoded: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(
            decoded["result"]["content"][0]["text"],
            "The hook completed and returned nothing."
        );
    }

    #[tokio::test]
    async fn tools_list_is_what_the_server_sent() {
        let (state, _) = state_with(ok_invoke());
        let response = post(
            &state,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        )
        .await;
        let decoded: Value = serde_json::from_slice(&response.body).unwrap();
        let tools = decoded["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "fixture.read");
        assert_eq!(
            tools[0]["description"],
            "Read a deterministic fixture value"
        );
        assert_eq!(tools[0]["inputSchema"]["type"], "object");

        // A tool with no declared input still needs a schema.
        let bare = vec![PluginHookTool {
            installation_id: "i".into(),
            hook_key: "k".into(),
            name: "bare".into(),
            description: String::new(),
            input_schema: None,
        }];
        let path = "/tok".to_string();
        let bare_state = PluginHookMCPState::new(
            crate::repocache::Ctx::new(),
            "t".into(),
            bare,
            ok_invoke(),
            path.clone(),
        );
        let response = serve_plugin_hook_request(
            &bare_state,
            "POST",
            &path,
            br#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
        )
        .await;
        let decoded: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(
            decoded["result"]["tools"][0]["inputSchema"]["type"],
            "object"
        );
    }

    #[tokio::test]
    async fn refuses_an_unknown_tool() {
        let (state, _) = state_with(ok_invoke());
        let response = post(
            &state,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope"}}),
        )
        .await;
        let decoded: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(decoded["error"]["code"], -32602);
        assert!(decoded["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown tool nope"));
    }

    #[tokio::test]
    async fn refuses_the_wrong_path_and_method() {
        let (state, _) = state_with(ok_invoke());
        let response = serve_plugin_hook_request(&state, "POST", "/wrong", b"{}").await;
        assert_eq!(response.status, StatusCode::NOT_FOUND);
        let response = serve_plugin_hook_request(&state, "GET", &state.path.clone(), b"{}").await;
        assert_eq!(response.status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn initialize_handshake_and_notification() {
        let (state, _) = state_with(ok_invoke());
        let response = post(
            &state,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
        )
        .await;
        let decoded: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(
            decoded["result"]["protocolVersion"],
            PLUGIN_HOOK_MCP_PROTOCOL_VERSION
        );
        assert_eq!(decoded["result"]["serverInfo"]["name"], "cordy-plugins");

        // A notification has no id and takes no reply.
        let response = serve_plugin_hook_request(
            &state,
            "POST",
            &state.path.clone(),
            br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .await;
        assert_eq!(response.status, StatusCode::ACCEPTED);
        assert!(response.body.is_empty());

        let response = post(&state, json!({"jsonrpc":"2.0","id":5,"method":"bogus"})).await;
        let decoded: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(decoded["error"]["code"], -32601);
    }
}
