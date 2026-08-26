//! Port of `server/internal/daemon/remote_mcp_broker.go` (475 lines).
//!
//! Symbol map (Go → Rust):
//! - `remoteMCPMaxRequestBytes` / `remoteMCPMaxCalls` / `remoteMCPMaxConcurrency`
//!   → [`REMOTE_MCP_MAX_REQUEST_BYTES`] / [`REMOTE_MCP_MAX_CALLS`] / [`REMOTE_MCP_MAX_CONCURRENCY`]
//! - `remoteMCPBrokerSet` → [`RemoteMCPBrokerSet`]
//! - `startTaskRemoteMCPBrokers` → [`start_task_remote_mcp_brokers`]
//! - `providerSupportsRemoteMCPBroker` → [`provider_supports_remote_mcp_broker`]
//! - `validatePinnedRemoteMCPTools` → [`validate_pinned_remote_mcp_tools`]
//! - `randomBrokerToken` → [`random_broker_token`]
//! - `remoteMCPServerName` → [`remote_mcp_server_name`]
//! - `remoteMCPProxy.ServeHTTP` → [`serve_proxy_request`]
//! - `decodeRemoteMCPSSEData` → [`decode_remote_mcp_sse_data`]
//! - `allowedRemoteMCPMethod` → [`allowed_remote_mcp_method`]
//! - `filterToolsListResponse` → [`filter_tools_list_response`]
//! - `canonicalRemoteMCPJSON` → [`canonical_remote_mcp_json`]
//! - `mergeTaskRemoteMCPConfig` → [`merge_task_remote_mcp_config`]
//!
//! Port notes: `http.Server` becomes axum served on 127.0.0.1:0 with graceful
//! shutdown driven by a CancellationToken (2s grace like Go). Go injects a
//! plain `*http.Client` in tests; the equivalent seam is [`McpUpstream`],
//! whose production impl wraps cordy-remotemcp's pinned [`SecureHttpClient`].
//! The handler explicitly selects every credential, body, and upstream future
//! against the task context so cancellation does not leave a detached call.
//!
//! S9-integration: entry points are wired by the daemon-runner lane.

#![allow(dead_code)]

use std::str::FromStr as _;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Context as _};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::future::BoxFuture;
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

pub(crate) const REMOTE_MCP_MAX_REQUEST_BYTES: usize = 1 << 20;
pub(crate) const REMOTE_MCP_MAX_CALLS: i64 = 256;
pub(crate) const REMOTE_MCP_MAX_CONCURRENCY: usize = 8;

/// `remoteMCPCredentialResolver`: resolves fresh credentials just-in-time;
/// returns ordered header pairs.
pub(crate) type RemoteMCPCredentialResolver = Arc<
    dyn for<'a> Fn(
            &'a crate::repocache::Ctx,
            &'a str,
        ) -> BoxFuture<'a, anyhow::Result<Vec<(String, String)>>>
        + Send
        + Sync,
>;

/// Transport seam standing in for the pinned HTTPS client (tests inject a
/// plain-HTTP transport against local fixtures).
pub(crate) trait McpUpstream: Send + Sync {
    fn send<'a>(
        &'a self,
        ctx: &'a crate::repocache::Ctx,
        request: http::Request<cordy_remotemcp::RequestBody>,
    ) -> BoxFuture<'a, anyhow::Result<http::Response<Vec<u8>>>>;
}

pub(crate) struct SecureUpstream(pub cordy_remotemcp::SecureHttpClient);

impl McpUpstream for SecureUpstream {
    fn send<'a>(
        &'a self,
        ctx: &'a crate::repocache::Ctx,
        request: http::Request<cordy_remotemcp::RequestBody>,
    ) -> BoxFuture<'a, anyhow::Result<http::Response<Vec<u8>>>> {
        Box::pin(async move {
            let response = tokio::select! {
                _ = ctx.cancelled() => Err(anyhow!(ctx.cause().to_string())),
                response = self.0.send(request) => response.map_err(|e| anyhow!("{e}")),
            }?;
            Ok(response)
        })
    }
}

/// One task's broker set: every server this task owns plus their shutdown
/// tokens; Close is idempotent (go:31–53).
pub(crate) struct RemoteMCPBrokerSet {
    shutdown_tokens: Vec<CancellationToken>,
    once: std::sync::Once,
}

impl Default for RemoteMCPBrokerSet {
    fn default() -> Self {
        Self {
            shutdown_tokens: Vec::new(),
            once: std::sync::Once::new(),
        }
    }
}

impl RemoteMCPBrokerSet {
    pub(crate) fn close(&mut self) {
        self.once.call_once(|| {
            for token in &self.shutdown_tokens {
                token.cancel();
            }
        });
    }

    fn push(&mut self, token: CancellationToken) {
        self.shutdown_tokens.push(token);
    }
}

impl Drop for RemoteMCPBrokerSet {
    fn drop(&mut self) {
        self.close();
    }
}

/// Startup outcome of [`start_task_remote_mcp_brokers`] (go:55 signature):
/// config fragment, diagnostics for degraded optional connections, the live
/// set, and any fatal startup error — mirroring Go's 4-value return.
pub(crate) struct BrokerStartup {
    pub config: Option<Value>,
    pub diagnostics: Vec<String>,
    pub set: Option<RemoteMCPBrokerSet>,
    pub error: Option<anyhow::Error>,
}

/// `startTaskRemoteMCPBrokers` (go:55–157): validates and starts one loopback
/// proxy per connection. An optional failure_policy degrades to a diagnostic
/// instead of failing the task.
pub(crate) async fn start_task_remote_mcp_brokers(
    setup_ctx: &crate::repocache::Ctx,
    lifetime_ctx: &crate::repocache::Ctx,
    task_id: &str,
    provider: &str,
    connections: &[cordy_remotemcp::Connection],
    resolve_credential: Option<RemoteMCPCredentialResolver>,
) -> anyhow::Result<BrokerStartup> {
    if connections.is_empty() {
        return Ok(BrokerStartup {
            config: None,
            diagnostics: Vec::new(),
            set: None,
            error: None,
        });
    }
    let mut set = RemoteMCPBrokerSet::default();
    let mut servers = Map::new();
    let mut diagnostics: Vec<String> = Vec::new();

    for connection in connections {
        if let Some(cause) = setup_ctx.err() {
            return finish_err(set, diagnostics, anyhow!(cause.to_string()));
        }
        let degrade = |message: String, diagnostics: &mut Vec<String>| -> Option<anyhow::Error> {
            if connection.failure_policy == "optional" {
                diagnostics.push(message);
                None
            } else {
                Some(anyhow!(message))
            }
        };
        if !provider_supports_remote_mcp_broker(provider) {
            let message = format!(
                "Remote MCP {} is incompatible with provider {}",
                connection.contribution_key, provider
            );
            match degrade(message, &mut diagnostics) {
                None => continue,
                Some(err) => return finish_err(set, diagnostics, err),
            }
        }
        let empty_headers: Vec<(String, String)> = Vec::new();
        let mut headers = empty_headers;
        if !connection.credential_header.is_empty() {
            match &resolve_credential {
                Some(resolver) => {
                    let resolved = tokio::select! {
                        _ = setup_ctx.cancelled() => {
                            return finish_err(
                                set,
                                diagnostics,
                                anyhow!(setup_ctx.cause().to_string()),
                            );
                        }
                        resolved = resolver(setup_ctx, &connection.contribution_id) => resolved,
                    };
                    match resolved {
                        Ok(resolved) => headers = resolved,
                        Err(resolve_err) => {
                            let message = format!(
                                "Remote MCP {} credential is unavailable",
                                connection.contribution_key
                            );
                            match degrade(message.clone(), &mut diagnostics) {
                                None => continue,
                                Some(_) => {
                                    return finish_err(
                                        set,
                                        diagnostics,
                                        anyhow!("{message}: {resolve_err}"),
                                    )
                                }
                            }
                        }
                    }
                }
                None => {
                    let message = format!(
                        "Remote MCP {} credential resolver is unavailable",
                        connection.contribution_key
                    );
                    match degrade(message, &mut diagnostics) {
                        None => continue,
                        Some(err) => return finish_err(set, diagnostics, err),
                    }
                }
            }
        }
        let protocol_versions: Vec<&str> = connection
            .protocol_versions
            .iter()
            .map(String::as_str)
            .collect();
        let discovered = tokio::select! {
            _ = setup_ctx.cancelled() => {
                return finish_err(
                    set,
                    diagnostics,
                    anyhow!(setup_ctx.cause().to_string()),
                );
            }
            discovered = cordy_remotemcp::discover(
                &connection.endpoint,
                &connection.endpoint_allowed_hosts,
                protocol_versions,
                &headers,
            ) => discovered.map_err(|error| anyhow!("{error}")),
        };
        let validated: anyhow::Result<()> = match discovered {
            Ok((tools, _)) => validate_pinned_remote_mcp_tools(&connection.approved_tools, &tools)
                .map_err(|s| anyhow!(s)),
            Err(err) => Err(err),
        };
        if let Err(err) = validated {
            let message = format!(
                "Remote MCP {} failed startup validation",
                connection.contribution_key
            );
            match degrade(message.clone(), &mut diagnostics) {
                None => continue,
                Some(_) => return finish_err(set, diagnostics, anyhow!("{message}: {err}")),
            }
        }
        let endpoint: anyhow::Result<url::Url> = tokio::select! {
            _ = setup_ctx.cancelled() => {
                return finish_err(
                    set,
                    diagnostics,
                    anyhow!(setup_ctx.cause().to_string()),
                );
            }
            endpoint = cordy_remotemcp::validate_public_https_endpoint(
                &connection.endpoint,
                &connection.endpoint_allowed_hosts,
                None,
            ) => endpoint.map_err(|error| anyhow!("{error}")),
        };
        let endpoint = endpoint?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("listen for Remote MCP broker")?;
        let addr = listener.local_addr().context("local addr")?;
        let path_token = random_broker_token()?;
        let path = format!("/{path_token}");
        let proxy = Arc::new(RemoteMCPProxyState {
            ctx: lifetime_ctx.child(),
            task_id: task_id.to_string(),
            connection: connection.clone(),
            endpoint_url: endpoint.clone(),
            upstream: Arc::new(SecureUpstream(cordy_remotemcp::new_secure_http_client(
                &endpoint,
            ))),
            credential_headers: headers.clone(),
            resolve_credential: resolve_credential.clone(),
            path: path.clone(),
            semaphore: Arc::new(tokio::sync::Semaphore::new(REMOTE_MCP_MAX_CONCURRENCY)),
            calls: AtomicI64::new(0),
        });
        let app = axum::Router::new()
            .route(
                &path,
                axum::routing::post(proxy_handler).fallback(proxy_fallback),
            )
            .with_state(Arc::clone(&proxy));
        let shutdown = CancellationToken::new();
        set.push(shutdown.clone());
        let serve_shutdown = shutdown.clone();
        let serve_task_id = task_id.to_string();
        let serve_connection = connection.clone();
        tokio::spawn(async move {
            let server = axum::serve(listener, app).with_graceful_shutdown(async move {
                serve_shutdown.cancelled().await;
            });
            if let Err(serve_err) = server.await {
                tracing::warn!(
                    task_id = %serve_task_id,
                    contribution = %serve_connection.contribution_key,
                    error = %serve_err,
                    "Remote MCP broker stopped unexpectedly"
                );
            }
        });
        let name = remote_mcp_server_name(connection);
        servers.insert(
            name,
            json!({"type": "http", "url": format!("http://{addr}{path}")}),
        );
    }

    if servers.is_empty() {
        set.close();
        return Ok(BrokerStartup {
            config: None,
            diagnostics,
            set: None,
            error: None,
        });
    }
    let lifetime_token = lifetime_ctx.token().clone();
    // Close the whole set when the task's lifetime context ends (go:147–150).
    // The tokens are owned by the returned set as well; this watcher only
    // mirrors cancellation onto them.
    for token in &set.shutdown_tokens {
        let source = token.clone();
        let lifetime_token = lifetime_token.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = lifetime_token.cancelled() => source.cancel(),
                _ = source.cancelled() => {}
            }
        });
    }
    Ok(BrokerStartup {
        config: Some(json!({"mcpServers": servers})),
        diagnostics,
        set: Some(set),
        error: None,
    })
}

fn finish_err(
    mut set: RemoteMCPBrokerSet,
    diagnostics: Vec<String>,
    err: anyhow::Error,
) -> anyhow::Result<BrokerStartup> {
    set.close();
    Ok(BrokerStartup {
        config: None,
        diagnostics,
        set: None,
        error: Some(err),
    })
}

pub(crate) fn provider_supports_remote_mcp_broker(provider: &str) -> bool {
    matches!(provider, "codex" | "claude" | "hermes" | "qoder" | "mcode")
}

/// Every approved tool must still exist with an identical schema digest
/// (go:168–183).
pub(crate) fn validate_pinned_remote_mcp_tools(
    approved: &[cordy_remotemcp::Tool],
    discovered: &[cordy_remotemcp::Tool],
) -> Result<(), String> {
    let available: std::collections::HashMap<&str, &cordy_remotemcp::Tool> = discovered
        .iter()
        .map(|tool| (tool.name.as_str(), tool))
        .collect();
    for pinned in approved {
        let Some(current) = available.get(pinned.name.as_str()) else {
            return Err(format!("approved tool {:?} is missing", pinned.name));
        };
        if current.schema_digest != pinned.schema_digest {
            return Err(format!("approved tool {:?} schema drifted", pinned.name));
        }
    }
    Ok(())
}

pub(crate) fn random_broker_token() -> anyhow::Result<String> {
    use rand::RngCore;
    let mut value = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut value);
    Ok(hex::encode(value))
}

pub(crate) fn remote_mcp_server_name(connection: &cordy_remotemcp::Connection) -> String {
    let name: String = connection
        .contribution_key
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let mut suffix: String = connection
        .contribution_id
        .chars()
        .filter(|c| *c != '-')
        .collect();
    suffix.truncate(8);
    format!("plugin-{name}-{suffix}")
}

pub(crate) struct RemoteMCPProxyState {
    ctx: crate::repocache::Ctx,
    task_id: String,
    connection: cordy_remotemcp::Connection,
    endpoint_url: url::Url,
    upstream: Arc<dyn McpUpstream>,
    /// Startup-resolved credentials, the default for every call
    /// (`credentialHeaders`, go:212).
    credential_headers: Vec<(String, String)>,
    resolve_credential: Option<RemoteMCPCredentialResolver>,
    path: String,
    semaphore: Arc<tokio::sync::Semaphore>,
    calls: AtomicI64,
}

async fn proxy_fallback() -> Response {
    not_found_response()
}

fn not_found_response() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

async fn proxy_handler(
    State(proxy): State<Arc<RemoteMCPProxyState>>,
    request: Request,
) -> Response {
    serve_proxy_request(&proxy, request).await
}

/// One JSON-RPC-level failure rendered by [`write_remote_mcp_error`] at HTTP
/// 200, carrying its logging class.
struct ProxyFailure {
    result_class: &'static str,
    id: Option<String>,
    code: i64,
    message: String,
}

/// `remoteMCPProxy.ServeHTTP` (go:227–351): one deferred log line covers
/// every exit; protocol errors are JSON-RPC envelopes at HTTP 200.
pub(crate) async fn serve_proxy_request(proxy: &RemoteMCPProxyState, request: Request) -> Response {
    let started = Instant::now();
    let mut tool_name = String::new();
    let mut result_class = "rejected";
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let request_headers = request.headers().clone();

    let response = if path != proxy.path || method != http::Method::POST {
        not_found_response()
    } else {
        match run_proxy_body(proxy, request, &request_headers, &mut tool_name).await {
            Ok((class, response)) => {
                result_class = class;
                response
            }
            Err(failure) => {
                result_class = failure.result_class;
                jsonrpc_error_response(failure.id.as_deref(), failure.code, &failure.message)
            }
        }
    };
    tracing::info!(
        task_id = %proxy.task_id,
        installation_id = %proxy.connection.installation_id,
        contribution = %proxy.connection.contribution_key,
        tool = %tool_name,
        duration_ms = started.elapsed().as_millis() as i64,
        result_class = %result_class,
        "Remote MCP broker call"
    );
    response
}

type ProxyOutcome = Result<(&'static str, Response), ProxyFailure>;

async fn run_proxy_body(
    proxy: &RemoteMCPProxyState,
    request: Request,
    request_headers: &http::HeaderMap,
    tool_name: &mut String,
) -> ProxyOutcome {
    // Call counter includes rejected calls, exactly like Go's Add(1) before
    // the limit check (go:247).
    if proxy.calls.fetch_add(1, Ordering::SeqCst) + 1 > REMOTE_MCP_MAX_CALLS {
        return Err(ProxyFailure {
            result_class: "rejected",
            id: None,
            code: -32002,
            message: "Remote MCP task call limit exceeded".to_string(),
        });
    }
    let Ok(_permit) = proxy.semaphore.clone().try_acquire_owned() else {
        return Err(ProxyFailure {
            result_class: "rejected",
            id: None,
            code: -32003,
            message: "Remote MCP concurrency limit exceeded".to_string(),
        });
    };

    let max = REMOTE_MCP_MAX_REQUEST_BYTES + 1;
    let raw = match tokio::select! {
        _ = proxy.ctx.cancelled() => {
            return Err(ProxyFailure {
                result_class: "cancelled",
                id: None,
                code: -32000,
                message: "Remote MCP task was cancelled".to_string(),
            });
        }
        raw = axum::body::to_bytes(request.into_body(), max) => raw,
    } {
        Ok(raw) if raw.len() <= REMOTE_MCP_MAX_REQUEST_BYTES => raw,
        _ => {
            return Err(ProxyFailure {
                result_class: "rejected",
                id: None,
                code: -32600,
                message: "Remote MCP request is invalid".to_string(),
            })
        }
    };
    let rpc: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    let rpc_id = rpc
        .get("id")
        .filter(|id| !id.is_null())
        .map(|id| id.to_string());
    let jsonrpc_ok = rpc.get("jsonrpc").and_then(Value::as_str) == Some("2.0");
    let method = rpc
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let params = rpc.get("params").cloned().unwrap_or(Value::Null);
    if !jsonrpc_ok {
        return Err(ProxyFailure {
            result_class: "rejected",
            id: rpc_id,
            code: -32600,
            message: "Remote MCP request is invalid".to_string(),
        });
    }
    if !allowed_remote_mcp_method(&method) {
        return Err(ProxyFailure {
            result_class: "rejected",
            id: rpc_id,
            code: -32601,
            message: "Only approved Remote MCP tools are available".to_string(),
        });
    }
    if method == "tools/call" {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        if !tool_approved(&proxy.connection, name) {
            return Err(ProxyFailure {
                result_class: "rejected",
                id: rpc_id,
                code: -32602,
                message: "Remote MCP tool is not approved".to_string(),
            });
        }
        *tool_name = name.to_string();
    }

    // Startup-resolved headers are the baseline; when the connection carries
    // a credential header plus resolver, re-check just before dialing
    // upstream so a revocation takes effect mid-task (go:294–302).
    let mut credential_headers = proxy.credential_headers.clone();
    if !proxy.connection.credential_header.is_empty() {
        if let Some(resolver) = &proxy.resolve_credential {
            let resolved = resolver(&proxy.ctx, &proxy.connection.contribution_id);
            match tokio::select! {
                _ = proxy.ctx.cancelled() => Err(anyhow!(proxy.ctx.cause().to_string())),
                resolved = resolved => resolved,
            } {
                Ok(resolved) => credential_headers = resolved,
                Err(_) => {
                    return Err(ProxyFailure {
                        result_class: "credential_revoked",
                        id: rpc_id,
                        code: -32005,
                        message: "Remote MCP credential is revoked or unavailable".to_string(),
                    })
                }
            }
        }
    }

    let mut builder = http::Request::builder()
        .method(http::Method::POST)
        .uri(
            http::Uri::from_str(proxy.endpoint_url.as_str()).unwrap_or_else(|_| {
                // The endpoint was validated at startup; this fallback keeps the
                // builder total.
                http::Uri::from_static("/")
            }),
        )
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    for header in ["Mcp-Session-Id", "Mcp-Protocol-Version", "Last-Event-ID"] {
        if let Some(value) = request_headers.get(header) {
            builder = builder.header(header, value.clone());
        }
    }
    for (key, value) in &credential_headers {
        builder = builder.header(key.as_str(), value.as_str());
    }
    let upstream_request = builder
        .body(cordy_remotemcp::RequestBody::from(raw.to_vec()))
        .map_err(|err| ProxyFailure {
            result_class: "rejected",
            id: rpc_id.clone(),
            code: -32603,
            message: format!("Remote MCP request failed: {err}"),
        })?;

    let response = tokio::select! {
        _ = proxy.ctx.cancelled() => {
            return Err(ProxyFailure {
                result_class: "cancelled",
                id: rpc_id,
                code: -32000,
                message: "Remote MCP task was cancelled".to_string(),
            });
        }
        response = proxy.upstream.send(&proxy.ctx, upstream_request) => response,
    };
    let response = match response {
        Ok(response) => response,
        Err(_) if proxy.ctx.err().is_some() => {
            return Err(ProxyFailure {
                result_class: "cancelled",
                id: rpc_id,
                code: -32000,
                message: "Remote MCP task was cancelled".to_string(),
            })
        }
        Err(_) => {
            return Err(ProxyFailure {
                result_class: "remote_error",
                id: rpc_id,
                code: -32000,
                message: "Remote MCP service is unavailable".to_string(),
            })
        }
    };

    let max_response = cordy_remotemcp::MAX_RESPONSE_BYTES + 1;
    let (status, upstream_headers, body) = (
        response.status(),
        response.headers().clone(),
        response.into_body(),
    );
    let mut body = body;
    if body.len() > cordy_remotemcp::MAX_RESPONSE_BYTES {
        return Err(ProxyFailure {
            result_class: "remote_error",
            id: rpc_id,
            code: -32001,
            message: "Remote MCP response exceeded the allowed limit".to_string(),
        });
    }
    let _ = max_response;
    if !status.is_success() {
        return Err(ProxyFailure {
            result_class: "remote_error",
            id: rpc_id,
            code: -32000,
            message: "Remote MCP service returned an error".to_string(),
        });
    }

    let content_type = upstream_headers
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let mut response_builder = Response::builder().status(status);
    if method == "tools/list" {
        let decoded =
            decode_remote_mcp_sse_data(&content_type, &body).map_err(|message| ProxyFailure {
                result_class: "remote_error",
                id: rpc_id.clone(),
                code: -32000,
                message,
            })?;
        let filtered = filter_tools_list_response(&decoded, &proxy.connection.approved_tools)
            .map_err(|_| ProxyFailure {
                result_class: "schema_drift",
                id: rpc_id.clone(),
                code: -32004,
                message: "Remote MCP tool schema changed and requires review".to_string(),
            })?;
        body = filtered;
        response_builder = response_builder.header("Content-Type", "application/json");
    } else if !content_type.is_empty() {
        response_builder = response_builder.header("Content-Type", content_type);
    }
    for header in ["Mcp-Session-Id", "Mcp-Protocol-Version"] {
        if let Some(value) = upstream_headers.get(header) {
            response_builder = response_builder.header(header, value.clone());
        }
    }
    Ok((
        "success",
        response_builder
            .body(Body::from(body))
            .unwrap_or_else(|_| not_found_response()),
    ))
}

fn tool_approved(connection: &cordy_remotemcp::Connection, name: &str) -> bool {
    connection
        .approved_tools
        .iter()
        .any(|tool| tool.name == name)
}

/// Extracts the first `data:` payload from a text/event-stream body
/// (go:353–366); non-SSE bodies pass through unchanged.
pub(crate) fn decode_remote_mcp_sse_data(
    content_type: &str,
    raw: &[u8],
) -> Result<Vec<u8>, String> {
    if !content_type.to_lowercase().starts_with("text/event-stream") {
        return Ok(raw.to_vec());
    }
    for line in raw.split(|b| *b == b'\n') {
        if let Some(rest) = line.strip_prefix(b"data:") {
            let data = String::from_utf8_lossy(rest).trim().as_bytes().to_vec();
            if !data.is_empty() {
                return Ok(data);
            }
        }
    }
    Err("Remote MCP SSE response contained no data".to_string())
}

pub(crate) fn allowed_remote_mcp_method(method: &str) -> bool {
    matches!(
        method,
        "initialize"
            | "notifications/initialized"
            | "notifications/cancelled"
            | "ping"
            | "tools/list"
            | "tools/call"
    )
}

/// Go's canonicalRemoteMCPJSON: parse + re-marshal so object keys are sorted
/// (serde_json's BTreeMap does this natively).
pub(crate) fn canonical_remote_mcp_json(raw: &Value) -> anyhow::Result<String> {
    serde_json::to_string(raw).map_err(|err| anyhow!("{err}"))
}

/// Filters a tools/list result down to the approved set and verifies every
/// schema digest still matches (go:386–426). Descriptions come from the
/// pinned approval, not the live server.
pub(crate) fn filter_tools_list_response(
    raw: &[u8],
    approved: &[cordy_remotemcp::Tool],
) -> anyhow::Result<Vec<u8>> {
    let response: Map<String, Value> =
        serde_json::from_slice(raw).map_err(|err| anyhow!("{err}"))?;
    let result = response
        .get("result")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("missing result"))?
        .clone();
    let tools = result.get("tools").cloned().unwrap_or(Value::Null);
    #[derive(serde::Deserialize)]
    struct ListedTool {
        name: String,
        #[serde(default)]
        input_schema: Value,
        #[serde(rename = "inputSchema", default)]
        input_schema_camel: Value,
    }
    let listed: Vec<ListedTool> = serde_json::from_value(tools).map_err(|err| anyhow!("{err}"))?;
    let mut by_name: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for tool in listed {
        let schema = if tool.input_schema.is_null() {
            tool.input_schema_camel
        } else {
            tool.input_schema
        };
        let canonical = canonical_remote_mcp_json(&schema)?;
        by_name.insert(tool.name.clone(), canonical);
    }

    let mut pinned: Vec<&cordy_remotemcp::Tool> = approved.iter().collect();
    pinned.sort_by(|a, b| a.name.cmp(&b.name));
    let mut filtered: Vec<Value> = Vec::with_capacity(pinned.len());
    for tool in pinned {
        match by_name.get(&tool.name) {
            Some(current)
                if cordy_remotemcp::digest_bytes(current.as_bytes()) == tool.schema_digest => {}
            _ => return Err(anyhow!("tool schema drift")),
        }
        let mut entry = json!({"name": tool.name});
        if !tool.description.is_empty() {
            entry["description"] = json!(tool.description);
        }
        entry["inputSchema"] =
            serde_json::from_str::<Value>(by_name.get(&tool.name).expect("checked").as_str())?;
        filtered.push(entry);
    }
    let mut result = result;
    result.insert("tools".to_string(), json!(filtered));
    let mut response = response;
    response.insert("result".to_string(), Value::Object(result));
    serde_json::to_vec(&Value::Object(response)).map_err(|err| anyhow!("{err}"))
}

/// `writeRemoteMCPError` (go:437–447): JSON-RPC envelope at HTTP 200; an
/// absent id becomes JSON null.
pub(crate) fn jsonrpc_error_response(id: Option<&str>, code: i64, message: &str) -> Response {
    let id_json: Value = match id {
        Some(id) => serde_json::from_str(id).unwrap_or(Value::Null),
        None => Value::Null,
    };
    let body = json!({
        "jsonrpc": "2.0",
        "id": id_json,
        "error": {"code": code, "message": message},
    });
    (
        StatusCode::OK,
        [("Content-Type", HeaderValue::from_static("application/json"))],
        body.to_string(),
    )
        .into_response()
}

/// `mergeTaskRemoteMCPConfig` (go:449–475): overlay's mcpServers win over
/// base's on a same-name collision; an empty overlay returns base untouched.
pub(crate) fn merge_task_remote_mcp_config(base: &str, overlay: &str) -> anyhow::Result<String> {
    if overlay.trim().is_empty() {
        return Ok(base.to_string());
    }
    let overlay_document: Value = serde_json::from_str(overlay).map_err(|err| anyhow!("{err}"))?;
    let empty_map = Map::new();
    let overlay_servers = overlay_document
        .get("mcpServers")
        .and_then(Value::as_object)
        .unwrap_or(&empty_map);

    let trimmed = base.trim();
    let mut base_document = json!({"mcpServers": {}});
    if !trimmed.is_empty() && trimmed != "null" {
        base_document = serde_json::from_str(trimmed).map_err(|err| anyhow!("{err}"))?;
    }
    let base_document = base_document
        .as_object_mut()
        .ok_or_else(|| anyhow!("base MCP config must be a JSON object"))?;
    let servers = base_document
        .entry("mcpServers".to_string())
        .or_insert_with(|| json!({}));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| anyhow!("base MCP config mcpServers must be a JSON object"))?;
    for (name, server) in overlay_servers {
        servers.insert(name.clone(), server.clone());
    }
    Ok(serde_json::to_string(&base_document)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn provider_matrix() {
        for provider in ["codex", "claude", "hermes", "qoder", "mcode"] {
            assert!(provider_supports_remote_mcp_broker(provider), "{provider}");
        }
        assert!(!provider_supports_remote_mcp_broker("deveco"));
    }

    #[test]
    fn dropping_broker_set_cancels_started_brokers() {
        let token = CancellationToken::new();
        let observed = token.clone();
        let mut set = RemoteMCPBrokerSet::default();
        set.push(token);

        drop(set);

        assert!(observed.is_cancelled());
    }

    #[test]
    fn config_merge_overlay_wins_and_base_survives() {
        let merged = merge_task_remote_mcp_config(
            r#"{"mcpServers":{"agent":{"command":"agent"}}}"#,
            r#"{"mcpServers":{"plugin":{"type":"http","url":"http://127.0.0.1/mcp"}}}"#,
        )
        .unwrap();
        assert!(merged.contains(r#""agent""#) && merged.contains(r#""plugin""#));
        let doc: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(doc["mcpServers"]["agent"]["command"], "agent");
        assert_eq!(doc["mcpServers"]["plugin"]["url"], "http://127.0.0.1/mcp");

        // Empty overlay returns base untouched.
        assert_eq!(
            merge_task_remote_mcp_config(r#"{"mcpServers":{}}"#, "").unwrap(),
            r#"{"mcpServers":{}}"#
        );

        // Overlaying a malformed base must be a normal preparation error,
        // not an indexing panic that takes down the daemon.
        assert!(merge_task_remote_mcp_config("[]", r#"{"mcpServers":{}}"#).is_err());
        assert!(merge_task_remote_mcp_config(
            r#"{"mcpServers":[]}"#,
            r#"{"mcpServers":{}}"#
        )
        .is_err());
    }

    #[test]
    fn decode_sse_data_extracts_first_payload_and_passes_json_through() {
        let raw = decode_remote_mcp_sse_data(
            "text/event-stream; charset=utf-8",
            b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1}\n\n",
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(raw).unwrap(),
            r#"{"jsonrpc":"2.0","id":1}"#
        );

        let passthrough = decode_remote_mcp_sse_data("application/json", b"{}").unwrap();
        assert_eq!(passthrough, b"{}");

        assert!(decode_remote_mcp_sse_data("text/event-stream", b"event: x\n").is_err());
    }

    #[test]
    fn pinned_tool_validation_reports_missing_then_drift() {
        let discovered = vec![cordy_remotemcp::Tool {
            name: "fixture.read".into(),
            description: String::new(),
            input_schema: json!({}),
            schema_digest: "sha256:aaa".into(),
            risk: String::new(),
        }];
        validate_pinned_remote_mcp_tools(
            &[cordy_remotemcp::Tool {
                name: "fixture.read".into(),
                description: String::new(),
                input_schema: json!({}),
                schema_digest: "sha256:aaa".into(),
                risk: String::new(),
            }],
            &discovered,
        )
        .unwrap();

        let err = validate_pinned_remote_mcp_tools(
            &[cordy_remotemcp::Tool {
                name: "fixture.deleted".into(),
                description: String::new(),
                input_schema: json!({}),
                schema_digest: "sha256:whatever".into(),
                risk: String::new(),
            }],
            &discovered,
        )
        .unwrap_err();
        assert!(err.contains("missing"), "{err}");

        let err = validate_pinned_remote_mcp_tools(
            &[cordy_remotemcp::Tool {
                name: "fixture.read".into(),
                description: String::new(),
                input_schema: json!({}),
                schema_digest: "sha256:the-shape-that-was-approved".into(),
                risk: String::new(),
            }],
            &discovered,
        )
        .unwrap_err();
        assert!(err.contains("drifted"), "{err}");
    }

    #[test]
    fn server_name_sanitizes_key_and_truncates_suffix() {
        let connection = cordy_remotemcp::Connection {
            contribution_key: "Toolbox Pro!".to_string(),
            contribution_id: "01234567-89ab-cdef".to_string(),
            ..Default::default()
        };
        assert_eq!(
            remote_mcp_server_name(&connection),
            "plugin-toolbox-pro--01234567"
        );
    }

    /// Plain-HTTP transport against a local axum fixture, standing in for
    /// Go's `httptest.Client()` injection.
    struct PlainUpstream {
        client: reqwest::Client,
    }

    impl McpUpstream for PlainUpstream {
        fn send<'a>(
            &'a self,
            _ctx: &'a crate::repocache::Ctx,
            request: http::Request<cordy_remotemcp::RequestBody>,
        ) -> BoxFuture<'a, anyhow::Result<http::Response<Vec<u8>>>> {
            Box::pin(async move {
                let (parts, body) = request.into_parts();
                use http_body_util::BodyExt as _;
                let bytes = body
                    .collect()
                    .await
                    .map_err(|e| anyhow!("{e}"))?
                    .to_bytes()
                    .to_vec();
                let response = self
                    .client
                    .post(parts.uri.to_string())
                    .headers(parts.headers)
                    .body(bytes)
                    .send()
                    .await
                    .map_err(|e| anyhow!("{e}"))?;
                let status = response.status();
                let headers = response.headers().clone();
                let body = response.bytes().await.map_err(|e| anyhow!("{e}"))?.to_vec();
                let mut builder = http::Response::builder().status(
                    http::StatusCode::from_u16(status.as_u16()).map_err(|e| anyhow!("{e}"))?,
                );
                for (name, value) in &headers {
                    builder = builder.header(name, value);
                }
                builder.body(body).map_err(|e| anyhow!("{e}"))
            })
        }
    }

    struct BlockingUpstream {
        started: Arc<tokio::sync::Notify>,
    }

    impl McpUpstream for BlockingUpstream {
        fn send<'a>(
            &'a self,
            _ctx: &'a crate::repocache::Ctx,
            _request: http::Request<cordy_remotemcp::RequestBody>,
        ) -> BoxFuture<'a, anyhow::Result<http::Response<Vec<u8>>>> {
            let started = Arc::clone(&self.started);
            Box::pin(async move {
                started.notify_one();
                std::future::pending::<anyhow::Result<http::Response<Vec<u8>>>>().await
            })
        }
    }

    async fn spawn_fixture() -> (
        url::Url,
        tokio_util::sync::CancellationToken,
        Arc<std::sync::Mutex<Vec<String>>>,
    ) {
        let writes: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let app = {
            let writes = Arc::clone(&writes);
            axum::Router::new().route(
                "/",
                axum::routing::post(move |headers: axum::http::HeaderMap, body: axum::body::Bytes| async move {
                    if headers.get("Authorization").and_then(|v| v.to_str().ok())
                        != Some("Bearer fixture-token")
                    {
                        return (
                            axum::http::StatusCode::UNAUTHORIZED,
                            [("Content-Type", "application/json")],
                            json!({"error": "credential rejected"}).to_string(),
                        );
                    }
                    let input: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    let id = input.get("id").cloned().unwrap_or(Value::Null);
                    match input.get("method").and_then(Value::as_str) {
                        Some("initialize") => (
                            axum::http::StatusCode::OK,
                            [("Content-Type", "application/json")],
                            json!({
                                "jsonrpc": "2.0", "id": id,
                                "result": {
                                    "protocolVersion": "2025-03-26",
                                    "capabilities": {"tools": {}},
                                    "serverInfo": {"name": "cordy-remote-mcp-fixture", "version": "1.0.0"},
                                }
                            })
                            .to_string(),
                        ),
                        Some("notifications/initialized") => (
                            axum::http::StatusCode::ACCEPTED,
                            [("Content-Type", "application/json")],
                            String::new(),
                        ),
                        Some("tools/list") => (
                            axum::http::StatusCode::OK,
                            [("Content-Type", "application/json")],
                            json!({
                                "jsonrpc": "2.0", "id": id,
                                "result": {"tools": [
                                    {"name": "fixture.read", "description": "Read a deterministic fixture value",
                                     "inputSchema": {"type": "object", "properties": {}}},
                                    {"name": "fixture.write", "description": "Append a value to the fixture log",
                                     "inputSchema": {"type": "object", "required": ["value"],
                                                     "properties": {"value": {"type": "string"}}}},
                                ]}
                            })
                            .to_string(),
                        ),
                        Some("tools/call") => {
                            let name = input["params"]["name"].as_str().unwrap_or("");
                            match name {
                                "fixture.read" => (
                                    axum::http::StatusCode::OK,
                                    [("Content-Type", "application/json")],
                                    json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"fixture-value"}]}}).to_string(),
                                ),
                                "fixture.write" => {
                                    writes.lock().unwrap().push(
                                        input["params"]["arguments"]["value"].as_str().unwrap_or("").to_string(),
                                    );
                                    (
                                        axum::http::StatusCode::OK,
                                        [("Content-Type", "application/json")],
                                        json!({"jsonrpc":"2.0","id":id,"result":{"content":[{"type":"text","text":"written"}]}}).to_string(),
                                    )
                                }
                                _ => (
                                    axum::http::StatusCode::OK,
                                    [("Content-Type", "application/json")],
                                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"unknown tool"}}).to_string(),
                                ),
                            }
                        }
                        _ => (
                            axum::http::StatusCode::OK,
                            [("Content-Type", "application/json")],
                            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"}}).to_string(),
                        ),
                    }
                }),
            )
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shutdown = tokio_util::sync::CancellationToken::new();
        let serve_shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move { serve_shutdown.cancelled().await })
                .await;
        });
        (
            url::Url::parse(&format!("http://{addr}/")).unwrap(),
            shutdown,
            writes,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn proxy_state(
        endpoint: url::Url,
        approved: Vec<cordy_remotemcp::Tool>,
        credential_headers: Vec<(String, String)>,
    ) -> RemoteMCPProxyState {
        RemoteMCPProxyState {
            ctx: crate::repocache::Ctx::new(),
            task_id: "task".into(),
            connection: cordy_remotemcp::Connection {
                installation_id: "installation".into(),
                contribution_key: "fixture".into(),
                approved_tools: approved,
                ..Default::default()
            },
            endpoint_url: endpoint,
            upstream: Arc::new(PlainUpstream {
                client: reqwest::Client::new(),
            }),
            credential_headers,
            resolve_credential: None,
            path: "/capability".into(),
            semaphore: Arc::new(tokio::sync::Semaphore::new(REMOTE_MCP_MAX_CONCURRENCY)),
            calls: AtomicI64::new(0),
        }
    }

    fn canned_read_tool() -> cordy_remotemcp::Tool {
        let schema = canonical_remote_mcp_json(&json!({"type":"object","properties":{}})).unwrap();
        cordy_remotemcp::Tool {
            name: "fixture.read".into(),
            description: "pinned".into(),
            input_schema: serde_json::from_str(&schema).unwrap(),
            schema_digest: cordy_remotemcp::digest_bytes(schema.as_bytes()),
            risk: "read".into(),
        }
    }

    fn post_request(state_path: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method(http::Method::POST)
            .uri(state_path)
            .header("Host", "localhost")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn proxy_filters_tools_list_and_injects_credential() {
        let (endpoint, shutdown, writes) = spawn_fixture().await;
        let state = proxy_state(
            endpoint.clone(),
            vec![canned_read_tool()],
            vec![("Authorization".into(), "Bearer fixture-token".into())],
        );

        let response = serve_proxy_request(
            &state,
            post_request(
                "/capability",
                json!({
                    "jsonrpc":"2.0","id":1,"method":"tools/list","params":{}
                }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body_bytes);
        assert!(
            !text.contains("fixture.write"),
            "filtered tools/list = {text}"
        );
        assert!(text.contains("fixture.read"), "{text}");
        assert!(
            text.contains("pinned"),
            "reviewed description preserved: {text}"
        );

        // Approving fixture.write and calling it must reach the upstream.
        let write_schema = canonical_remote_mcp_json(&json!({
            "type":"object",
            "required":["value"],
            "properties":{"value":{"type":"string"}}
        }))
        .unwrap();
        let mut approved = vec![canned_read_tool()];
        approved.push(cordy_remotemcp::Tool {
            name: "fixture.write".into(),
            description: String::new(),
            input_schema: serde_json::from_str(&write_schema).unwrap(),
            schema_digest: cordy_remotemcp::digest_bytes(write_schema.as_bytes()),
            risk: "write".into(),
        });
        let state = proxy_state(
            endpoint,
            approved,
            vec![("Authorization".into(), "Bearer fixture-token".into())],
        );
        let response = serve_proxy_request(
            &state,
            post_request(
                "/capability",
                json!({
                    "jsonrpc":"2.0","id":2,"method":"tools/call",
                    "params":{"name":"fixture.write","arguments":{"value":"broker-write"}}
                }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(*writes.lock().unwrap(), vec!["broker-write".to_string()]);
        shutdown.cancel();
    }

    #[tokio::test]
    async fn proxy_rejects_unapproved_tool_without_calling_upstream() {
        let (endpoint, shutdown, _writes) = spawn_fixture().await;
        let approved = vec![cordy_remotemcp::Tool {
            name: "allowed".into(),
            description: String::new(),
            input_schema: Value::Null,
            schema_digest: String::new(),
            risk: String::new(),
        }];
        let state = proxy_state(endpoint, approved, Vec::new());
        let response = serve_proxy_request(
            &state,
            post_request(
                "/capability",
                json!({
                    "jsonrpc":"2.0","id":1,"method":"tools/call",
                    "params":{"name":"denied","arguments":{"secret":"not logged"}}
                }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("not approved"), "{text}");
        shutdown.cancel();
    }

    #[tokio::test]
    async fn proxy_preserves_accepted_notification_status() {
        // A dedicated permissive upstream (go:92–99): any POST answers 202
        // with an empty body.
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(|| async { (StatusCode::ACCEPTED, Vec::<u8>::new()) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let state = proxy_state(
            url::Url::parse(&format!("http://{addr}/")).unwrap(),
            Vec::new(),
            Vec::new(),
        );
        let response = serve_proxy_request(
            &state,
            post_request(
                "/capability",
                json!({
                    "jsonrpc":"2.0","method":"notifications/initialized","params":{}
                }),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn proxy_rechecks_credential_before_upstream_call() {
        let (endpoint, shutdown, _) = spawn_fixture().await;
        let approved = vec![cordy_remotemcp::Tool {
            name: "allowed".into(),
            description: String::new(),
            input_schema: Value::Null,
            schema_digest: String::new(),
            risk: String::new(),
        }];
        let mut state = proxy_state(endpoint, approved, Vec::new());
        state.connection.credential_header = "Authorization".into();
        state.resolve_credential = Some(Arc::new(|_ctx: &crate::repocache::Ctx, _id: &str| {
            Box::pin(async { Err(anyhow!("revoked")) })
        }));
        let response = serve_proxy_request(&state, post_request("/capability", json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"allowed","arguments":{}}
        }))).await;
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("revoked or unavailable"), "{text}");
        shutdown.cancel();
    }

    #[tokio::test]
    async fn proxy_cancellation_stops_an_inflight_upstream_call() {
        let started = Arc::new(tokio::sync::Notify::new());
        let approved = vec![cordy_remotemcp::Tool {
            name: "allowed".into(),
            description: String::new(),
            input_schema: Value::Null,
            schema_digest: String::new(),
            risk: String::new(),
        }];
        let mut state = proxy_state(
            url::Url::parse("https://example.com/").unwrap(),
            approved,
            Vec::new(),
        );
        state.upstream = Arc::new(BlockingUpstream {
            started: Arc::clone(&started),
        });
        let ctx = state.ctx.clone();
        let response = serve_proxy_request(
            &state,
            post_request(
                "/capability",
                json!({
                    "jsonrpc":"2.0","id":1,"method":"tools/call",
                    "params":{"name":"allowed","arguments":{}}
                }),
            ),
        );
        tokio::pin!(response);
        tokio::select! {
            _ = started.notified() => ctx.cancel_with(crate::repocache::CancelCause::Cancelled),
            _ = &mut response => panic!("proxy call completed before cancellation"),
        }
        let response = tokio::time::timeout(Duration::from_secs(1), &mut response)
            .await
            .expect("cancellation should finish the proxy call");
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("Remote MCP task was cancelled"), "{text}");
    }
}
