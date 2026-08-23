//! Port of `server/internal/daemon/plugin_hook_mcp.go` (lines 1–246).
//!
//! A local MCP server that presents this workspace's plugin hooks as tools.
//!
//! Unlike the Remote MCP broker beside it, there is no upstream MCP server to
//! proxy to: a plugin author writes an HTTP endpoint and never learns what MCP
//! is. This synthesises the protocol from the manifest — the hook description
//! becomes the tool description, the hook's input_schema becomes the tool's.
//!
//! A tool call does NOT go to the plugin from here. It goes back to Cordy,
//! which makes the signed request. The daemon runs on someone's laptop; putting
//! the signing secret there would mean every machine running an agent holds a
//! credential that can impersonate the server to every plugin backend. Routing
//! through the server also means the rate limit, circuit breaker, `net:` check
//! and invocation record are the same code for all four triggers.
//!
//! Deviations from Go:
//! - `net/http` → a minimal hand-rolled HTTP/1.1 responder over a tokio
//!   `TcpListener` (no server framework in the crate's dependency set). One
//!   request per connection (`Connection: close`) instead of keep-alive;
//!   header block capped at 64 KiB (Go's http.Server default is 1 MB).
//! - `http.Server.Shutdown(2s)` drain → [`PluginHookMCPSet::close`] cancels
//!   the accept loop and waits up to 2 s for in-flight connection tasks via
//!   `tokio_util::task::TaskTracker`.
//! - `log/slog` → `tracing` with identical message text; the nil-logger
//!   guards disappear because tracing macros are always safe to call.
//! - `pluginHookInvoker` keeps its `context.Context` parameter as a
//!   [`Ctx`] cancellation token (first argument), mirroring daemon.go:6527.

// S9-integration: dead_code until Daemon core wires this.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use crate::repocache::Ctx;
use crate::types::PluginHookTool;

pub(crate) const PLUGIN_HOOK_MCP_PROTOCOL_VERSION: &str = "2024-11-05";
pub(crate) const PLUGIN_HOOK_MCP_MAX_REQUEST_BYTES: usize = 1 << 20;
pub(crate) const PLUGIN_HOOK_MCP_CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Cap on the HTTP header block of an incoming request (deviation: Go relies
/// on net/http's internal default).
const PLUGIN_HOOK_MCP_MAX_HEADER_BYTES: usize = 64 * 1024;

/// In-flight connection counter standing in for `tokio_util::task::TaskTracker`
/// (unavailable: workspace tokio-util lacks the `rt` feature). Lets
/// `close()` drain active requests before returning.
#[derive(Default)]
pub(crate) struct ConnTracker {
    count: AtomicUsize,
    notify: tokio::sync::Notify,
}

impl ConnTracker {
    pub(crate) fn enter(self: &Arc<Self>) -> ConnGuard {
        self.count.fetch_add(1, Ordering::SeqCst);
        ConnGuard(Arc::clone(self))
    }

    pub(crate) async fn wait_idle(&self) {
        loop {
            if self.count.load(Ordering::SeqCst) == 0 {
                return;
            }
            self.notify.notified().await;
        }
    }
}

pub(crate) struct ConnGuard(Arc<ConnTracker>);

impl Drop for ConnGuard {
    fn drop(&mut self) {
        if self.0.count.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.notify.notify_waiters();
        }
    }
}

/// `pluginHookInvoker` (plugin_hook_mcp.go:36–37): performs one hook call
/// against the Cordy server. Arguments mirror the Go closure signature:
/// `(ctx, taskID, installationID, hookKey, input)`.
pub(crate) type PluginHookInvoker = Arc<
    dyn Fn(Ctx, String, String, String, Option<serde_json::Value>)
        -> BoxFuture<'static, anyhow::Result<serde_json::Value>>
        + Send
        + Sync,
>;

/// `pluginHookMCPServer` (plugin_hook_mcp.go:39–46).
pub(crate) struct PluginHookMCPServer {
    task_id: String,
    tools: Vec<PluginHookTool>,
    by_name: HashMap<String, PluginHookTool>,
    invoke: PluginHookInvoker,
    path: String,
}

/// One synthesised HTTP response from [`PluginHookMCPServer::serve_http`].
/// Status mirrors Go's `WriteHeader`; `body` is the JSON-RPC payload when one
/// is written (notifications/initialized and 404s carry none).
#[derive(Debug, PartialEq)]
pub(crate) struct PluginHookHttpResponse {
    pub status: u16,
    pub body: Option<serde_json::Value>,
}

impl PluginHookHttpResponse {
    fn ok(body: serde_json::Value) -> Self {
        Self {
            status: 200,
            body: Some(body),
        }
    }

    fn status_only(status: u16) -> Self {
        Self {
            status,
            body: None,
        }
    }
}

/// `pluginHookMCPRequest` (plugin_hook_mcp.go:132–137).
#[derive(Debug, Deserialize)]
struct PluginHookMCPRequest {
    #[serde(default)]
    jsonrpc: String,
    #[serde(default)]
    id: Option<serde_json::Value>,
    #[serde(default)]
    method: String,
    #[serde(default)]
    params: Option<serde_json::Value>,
}

impl PluginHookMCPServer {
    /// `ServeHTTP` (plugin_hook_mcp.go:139–174). The path is a per-task random
    /// token, so a process that merely knows the port cannot reach the tools.
    pub(crate) async fn serve_http(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> PluginHookHttpResponse {
        if path != self.path || method != "POST" {
            return PluginHookHttpResponse::status_only(404);
        }
        let request: PluginHookMCPRequest = match serde_json::from_slice(body) {
            Ok(req) => req,
            Err(_) => {
                return write_plugin_hook_mcp_error(
                    None,
                    -32700,
                    "request is not valid JSON-RPC",
                )
            }
        };

        match request.method.as_str() {
            "initialize" => PluginHookHttpResponse::ok(write_plugin_hook_mcp_result_value(
                request.id,
                json!({
                    "protocolVersion": PLUGIN_HOOK_MCP_PROTOCOL_VERSION,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "cordy-plugins", "version": "1"},
                }),
            )),
            // A notification has no id and takes no reply.
            "notifications/initialized" => PluginHookHttpResponse::status_only(202),
            "tools/list" => PluginHookHttpResponse::ok(write_plugin_hook_mcp_result_value(
                request.id,
                json!({"tools": self.tool_descriptors()}),
            )),
            "tools/call" => self.handle_call(&request).await,
            other => write_plugin_hook_mcp_error(
                request.id,
                -32601,
                &format!("unsupported method {other}"),
            ),
        }
    }

    /// `toolDescriptors` (plugin_hook_mcp.go:176–192).
    fn tool_descriptors(&self) -> Vec<serde_json::Value> {
        let mut descriptors = Vec::with_capacity(self.tools.len());
        for tool in &self.tools {
            // A tool with no declared input still needs a schema, or providers
            // reject the list outright.
            let schema = match &tool.input_schema {
                Some(schema) if !schema.is_null() => schema.clone(),
                _ => json!({"type": "object", "properties": {}}),
            };
            descriptors.push(json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": schema,
            }));
        }
        descriptors
    }

    /// `handleCall` (plugin_hook_mcp.go:194–233).
    async fn handle_call(&self, request: &PluginHookMCPRequest) -> PluginHookHttpResponse {
        #[derive(Deserialize)]
        struct CallParams {
            #[serde(default)]
            name: String,
            #[serde(default)]
            arguments: Option<serde_json::Value>,
        }
        let Some(params_value) = &request.params else {
            return write_plugin_hook_mcp_error(
                request.id.clone(),
                -32602,
                "invalid tool call parameters",
            );
        };
        let params: CallParams = match serde_json::from_value(params_value.clone()) {
            Ok(params) => params,
            Err(_) => {
                return write_plugin_hook_mcp_error(
                    request.id.clone(),
                    -32602,
                    "invalid tool call parameters",
                )
            }
        };
        let tool = match self.by_name.get(&params.name) {
            Some(tool) => tool.clone(),
            None => {
                return write_plugin_hook_mcp_error(
                    request.id.clone(),
                    -32602,
                    &format!("unknown tool {}", params.name),
                )
            }
        };

        let fut = (self.invoke)(
            Ctx::new(),
            self.task_id.clone(),
            tool.installation_id.clone(),
            tool.hook_key.clone(),
            params.arguments,
        );
        let output = match tokio::time::timeout(PLUGIN_HOOK_MCP_CALL_TIMEOUT, fut).await {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => {
                // A TOOL error, not a protocol error. The agent reads this,
                // decides the tool did not work, and carries on with the task —
                // which is the whole point: an unreachable plugin endpoint must
                // not fail somebody's issue.
                tracing::info!(
                    task_id = %self.task_id,
                    tool = %tool.name,
                    error = %err,
                    "plugin hook tool call failed"
                );
                return PluginHookHttpResponse::ok(write_plugin_hook_mcp_result_value(
                    request.id.clone(),
                    json!({
                        "isError": true,
                        "content": [{"type": "text", "text": format!("{err:#}")}],
                    }),
                ));
            }
            // Matches Go's context.DeadlineExceeded text surfacing through the
            // invoker's wrapped context error.
            Err(_) => {
                tracing::info!(
                    task_id = %self.task_id,
                    tool = %tool.name,
                    error = "context deadline exceeded",
                    "plugin hook tool call failed"
                );
                return PluginHookHttpResponse::ok(write_plugin_hook_mcp_result_value(
                    request.id.clone(),
                    json!({
                        "isError": true,
                        "content": [{"type": "text", "text": "context deadline exceeded"}],
                    }),
                ));
            }
        };

        let text = if output.is_null() {
            "The hook completed and returned nothing.".to_string()
        } else {
            output.to_string()
        };
        PluginHookHttpResponse::ok(write_plugin_hook_mcp_result_value(
            request.id.clone(),
            json!({"content": [{"type": "text", "text": text}]}),
        ))
    }
}

/// `writePluginHookMCPResult` (plugin_hook_mcp.go:235–238): builds the JSON-RPC
/// success envelope around `result`.
fn write_plugin_hook_mcp_result_value(
    id: Option<serde_json::Value>,
    result: serde_json::Value,
) -> serde_json::Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// `writePluginHookMCPError` (plugin_hook_mcp.go:240–246).
fn write_plugin_hook_mcp_error(
    id: Option<serde_json::Value>,
    code: i64,
    message: &str,
) -> PluginHookHttpResponse {
    PluginHookHttpResponse::ok(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message},
    }))
}

/// `pluginHookMCPSet` (plugin_hook_mcp.go:48–52): owns the shutdown plumbing;
/// `once sync.Once` becomes an atomic flag.
pub(crate) struct PluginHookMCPSet {
    shutdown: CancellationToken,
    conns: Arc<ConnTracker>,
    closed: AtomicBool,
}

impl PluginHookMCPSet {
    /// `Close` (plugin_hook_mcp.go:54–68): idempotent shutdown that stops the
    /// accept loop and drains in-flight requests for up to 2 seconds.
    pub(crate) async fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(2), self.conns.wait_idle()).await;
    }
}

/// `startTaskPluginHookMCP` (plugin_hook_mcp.go:70–130): starts the tool
/// server for one task, if it has tools.
///
/// Returns the MCP config fragment to merge into the agent's, exactly like the
/// Remote MCP broker, so the two arrive at the agent through one path.
///
/// Deviation vs Go: the nil-invoke guard collapses into the empty-tools check
/// (the Rust invoker type is non-optional); the config fragment is built with
/// `serde_json::json!`, so its marshal step cannot fail; the function is
/// `async` because binding the listener is.
pub(crate) async fn start_task_plugin_hook_mcp(
    lifetime_ctx: &Ctx,
    task_id: &str,
    tools: Vec<PluginHookTool>,
    invoke: PluginHookInvoker,
) -> anyhow::Result<(Option<serde_json::Value>, Option<Arc<PluginHookMCPSet>>)> {
    if tools.is_empty() {
        return Ok((None, None));
    }

    let mut by_name: HashMap<String, PluginHookTool> = HashMap::with_capacity(tools.len());
    for tool in &tools {
        // The server namespaces these, but a duplicate arriving anyway must
        // resolve to exactly one hook rather than whichever came last.
        if by_name.contains_key(&tool.name) {
            tracing::warn!(
                task_id = %task_id,
                tool = %tool.name,
                "plugin hook tool name collided; ignoring the duplicate"
            );
            continue;
        }
        by_name.insert(tool.name.clone(), tool.clone());
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("listen for plugin hook MCP server")?;
    let path_token = crate::remote_mcp_broker::random_broker_token()?;

    let server = Arc::new(PluginHookMCPServer {
        task_id: task_id.to_string(),
        by_name,
        invoke,
        path: format!("/{path_token}"),
        tools,
    });
    let shutdown = CancellationToken::new();
    let conns = Arc::new(ConnTracker::default());
    let set = Arc::new(PluginHookMCPSet {
        shutdown: shutdown.clone(),
        conns: Arc::clone(&conns),
        closed: AtomicBool::new(false),
    });

    let addr = listener
        .local_addr()
        .context("listen for plugin hook MCP server")?;
    let accept_server = Arc::clone(&server);
    let accept_task_id = task_id.to_string();
    let accept_conns = Arc::clone(&conns);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                accepted = listener.accept() => match accepted {
                    Ok((stream, _peer)) => {
                        let conn_server = Arc::clone(&accept_server);
                        let conn_shutdown = shutdown.clone();
                        let guard = accept_conns.enter();
                        tokio::spawn(async move {
                            handle_connection(conn_server, stream, conn_shutdown).await;
                            drop(guard);
                        });
                    }
                    Err(err) => {
                        // Serve returning a non-ErrServerClosed error lands here.
                        tracing::warn!(
                            task_id = %accept_task_id,
                            error = %err,
                            "plugin hook MCP server stopped unexpectedly"
                        );
                        break;
                    }
                }
            }
        }
    });

    // Lifetime watcher (plugin_hook_mcp.go:114–117).
    {
        let set_watcher = Arc::clone(&set);
        let lifetime = lifetime_ctx.clone();
        tokio::spawn(async move {
            lifetime.cancelled().await;
            set_watcher.close().await;
        });
    }

    let raw = json!({
        "mcpServers": {
            "cordy-plugins": {
                "type": "http",
                "url": format!("http://{addr}{}", server.path),
            },
        }
    });
    Ok((Some(raw), Some(set)))
}

// ---------------------------------------------------------------------------
// Minimal HTTP/1.1 plumbing over tokio TCP (deviation stand-in for net/http).
// Shared by both local MCP servers in this lane: plugin_hook_mcp and
// remote_mcp_broker.
// ---------------------------------------------------------------------------

/// Reads one request off `stream`, dispatches into [`PluginHookMCPServer`],
/// writes one response, then closes (`Connection: close` semantics).
async fn handle_connection(
    server: Arc<PluginHookMCPServer>,
    mut stream: TcpStream,
    shutdown: CancellationToken,
) {
    let request = read_request(&mut stream, &shutdown).await;
    let Some((method, path, _headers, body)) = request else {
        return;
    };
    let response = server.serve_http(&method, &path, &body).await;
    let reason = match response.status {
        200 => "OK",
        202 => "Accepted",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let payload = response
        .body
        .and_then(|v| serde_json::to_vec(&v).ok())
        .unwrap_or_default();
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        reason,
        payload.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(&payload).await;
    let _ = stream.shutdown().await;
}

/// Parses the request line + headers and reads up to
/// [`PLUGIN_HOOK_MCP_MAX_REQUEST_BYTES`] of body (Go's
/// `io.ReadAll(io.LimitReader(r.Body, …))`: an oversized body is truncated so
/// JSON parsing fails downstream). Returns `(method, path, headers, body)`;
/// `None` means "no response" — malformed transport input or shutdown
/// mid-read.
pub(crate) async fn read_request(
    stream: &mut TcpStream,
    shutdown: &CancellationToken,
) -> Option<(String, String, Vec<(String, String)>, Vec<u8>)> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if buf.len() > PLUGIN_HOOK_MCP_MAX_HEADER_BYTES {
            return None;
        }
        tokio::select! {
            _ = shutdown.cancelled() => return None,
            read = stream.read(&mut chunk) => match read {
                Ok(0) => return None,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                        break pos;
                    }
                }
                Err(_) => return None,
            }
        }
    };

    let headers = std::str::from_utf8(&buf[..header_end]).ok()?;
    let mut lines = headers.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split(' ');
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    // Strip any query string; the token path never carries one.
    let path = target.split('?').next().unwrap_or(&target).to_string();

    let mut content_length: usize = 0;
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_string();
        let value = value.trim().to_string();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.parse().unwrap_or(0);
        }
        headers.push((name, value));
    }

    let want = content_length.min(PLUGIN_HOOK_MCP_MAX_REQUEST_BYTES);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < want {
        tokio::select! {
            _ = shutdown.cancelled() => return None,
            read = stream.read(&mut chunk) => match read {
                Ok(0) => break,
                Ok(n) => body.extend_from_slice(&chunk[..n]),
                Err(_) => return None,
            }
        }
    }
    body.truncate(want);
    Some((method, path, headers, body))
}

pub(crate) fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// testHookTools (plugin_hook_mcp_test.go:18–25).
    fn test_hook_tools() -> Vec<PluginHookTool> {
        vec![
            PluginHookTool {
                installation_id: "inst-1".into(),
                hook_key: "summarize".into(),
                name: "triage_a1b2__summarize".into(),
                description: "Summarize the thread.".into(),
                input_schema: Some(json!({"type": "object"})),
            },
            PluginHookTool {
                installation_id: "inst-2".into(),
                hook_key: "summarize".into(),
                name: "release_c3d4__summarize".into(),
                description: "Summarize the release.".into(),
                input_schema: None,
            },
        ]
    }

    fn never_invoker() -> PluginHookInvoker {
        Arc::new(|_ctx: Ctx, _task: String, _inst: String, _hook: String, _input| {
            Box::pin(async { Ok(serde_json::Value::Null) }) as BoxFuture<'static, _>
        })
    }

    /// startTestHookMCP (plugin_hook_mcp_test.go:27–37).
    fn start_test_hook_mcp(invoke: PluginHookInvoker) -> PluginHookMCPServer {
        let tools = test_hook_tools();
        let by_name: HashMap<String, PluginHookTool> = tools
            .iter()
            .map(|tool| (tool.name.clone(), tool.clone()))
            .collect();
        PluginHookMCPServer {
            task_id: "task-1".into(),
            tools,
            by_name,
            invoke,
            path: "/token".into(),
        }
    }

    /// callMCP (plugin_hook_mcp_test.go:39–52).
    async fn call_mcp(
        server: &PluginHookMCPServer,
        path: &str,
        body: &str,
    ) -> PluginHookHttpResponse {
        server.serve_http("POST", path, body.as_bytes()).await
    }

    /// An unreachable plugin endpoint must not fail the agent's task. It comes
    /// back as a tool result flagged isError, which the agent reads and works
    /// around — not as a protocol error, which would look to the agent like its
    /// tooling is broken.
    #[tokio::test]
    async fn plugin_hook_tool_failure_is_a_tool_error_not_a_transport_error() {
        let invoker: PluginHookInvoker = Arc::new(
            |_ctx: Ctx,
             _task: String,
             _inst: String,
             _hook: String,
             _input: Option<serde_json::Value>| {
                Box::pin(async {
                    Err(anyhow::anyhow!("hook endpoint did not answer"))
                        as anyhow::Result<serde_json::Value>
                }) as BoxFuture<'static, _>
            },
        );
        let server = start_test_hook_mcp(invoker);

        let response = call_mcp(
            &server,
            "/token",
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"triage_a1b2__summarize","arguments":{}}}"#,
        )
        .await;

        let decoded = response.body.expect("no result");
        let result = decoded.get("result").cloned();
        let result = result.expect("a failing hook produced no result");
        assert_eq!(
            result.get("isError"),
            Some(&json!(true)),
            "the tool result must be flagged as an error, got {result}"
        );
        assert!(
            decoded.get("error").is_none(),
            "a failing hook produced a JSON-RPC error, which reads as broken tooling"
        );
    }

    /// Both plugins' hooks are offered, under their distinct names.
    #[tokio::test]
    async fn plugin_hook_tools_list_is_what_the_server_sent() {
        let server = start_test_hook_mcp(never_invoker());
        let response =
            call_mcp(&server, "/token", r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
                .await;

        let result = response.body.expect("no result").get("result").cloned().expect("result");
        let tools = result
            .get("tools")
            .and_then(|t| t.as_array())
            .expect("tools array");
        assert_eq!(tools.len(), 2, "tools = {result}, want the two the server sent");
        let mut names = std::collections::HashSet::new();
        for tool in tools {
            let name = tool
                .get("name")
                .and_then(|n| n.as_str())
                .expect("tool name")
                .to_string();
            // A tool with no declared input still needs a schema or providers
            // reject the whole list.
            assert!(
                tool.get("inputSchema").is_some_and(|s| !s.is_null()),
                "tool {name} has no inputSchema"
            );
            names.insert(name);
        }
        assert!(
            names.contains("triage_a1b2__summarize") && names.contains("release_c3d4__summarize"),
            "both plugins' hooks must appear under distinct names, got {names:?}"
        );
    }

    /// A name the server did not send is not callable, however plausible it looks.
    #[tokio::test]
    async fn plugin_hook_tool_refuses_an_unknown_tool() {
        let called = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&called);
        let invoker: PluginHookInvoker = Arc::new(
            move |_ctx: Ctx,
                  _task: String,
                  _inst: String,
                  _hook: String,
                  _input: Option<serde_json::Value>| {
                flag.store(true, Ordering::SeqCst);
                Box::pin(async { Ok(serde_json::Value::Null) }) as BoxFuture<'static, _>
            },
        );
        let server = start_test_hook_mcp(invoker);
        let response = call_mcp(
            &server,
            "/token",
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"something__else","arguments":{}}}"#,
        )
        .await;

        assert!(
            response.body.and_then(|b| b.get("error").cloned()).is_some(),
            "an unknown tool must be refused"
        );
        assert!(!called.load(Ordering::SeqCst), "an unknown tool reached the invoker");
    }

    /// The path is a per-task random token, so knowing the port is not enough.
    #[tokio::test]
    async fn plugin_hook_mcp_refuses_the_wrong_path() {
        let server = start_test_hook_mcp(never_invoker());
        let response = server
            .serve_http(
                "POST",
                "/guessed",
                br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#.as_slice(),
            )
            .await;
        assert_eq!(response.status, 404, "want 404 for an unknown path");
    }

    /// End-to-end smoke over the hand-rolled HTTP layer: a real TCP round trip
    /// against start_task_plugin_hook_mcp returns a usable config fragment and
    /// answers tools/list on the tokened URL.
    #[tokio::test]
    async fn start_serves_config_fragment_over_tcp() {
        let invoker: PluginHookInvoker = Arc::new(
            |_ctx: Ctx,
             _task: String,
             _inst: String,
             _hook: String,
             _input: Option<serde_json::Value>| {
                Box::pin(async { Ok(json!({"echo": true})) }) as BoxFuture<'static, _>
            },
        );
        let lifetime = Ctx::new();
        let (config, set) =
            start_task_plugin_hook_mcp(&lifetime, "task-9", test_hook_tools(), invoker)
                .await
                .expect("start");
        let config = config.expect("config fragment");
        let set = set.expect("server set");
        let url = config["mcpServers"]["cordy-plugins"]["url"]
            .as_str()
            .expect("url")
            .to_string();

        let body = br#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#;
        let resp = reqwest::Client::new()
            .post(&url)
            .header("Content-Type", "application/json")
            .body(body.to_vec())
            .send()
            .await
            .expect("post");
        assert_eq!(resp.status(), 200);
        let decoded: serde_json::Value = resp.json().await.expect("json");
        assert_eq!(decoded["id"], json!(7));
        assert_eq!(decoded["result"]["tools"].as_array().map(Vec::len), Some(2));

        set.close().await;
    }
}
