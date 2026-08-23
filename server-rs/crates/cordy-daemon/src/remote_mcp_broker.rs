//! Port of `server/internal/daemon/remote_mcp_broker.go` (lines 1–475).
//!
//! Per-task local HTTP brokers that proxy an agent's MCP traffic to the
//! workspace's approved Remote MCP endpoints. Each connection gets its own
//! 127.0.0.1 listener with a random token path; the proxy enforces the call
//! limit, concurrency limit, approved-method allowlist and pinned tool
//! schemas before anything reaches the upstream server.
//!
//! Deviations from Go:
//! - `net/http` server → the shared hand-rolled HTTP/1.1 plumbing from
//!   [`crate::plugin_hook_mcp`] over a tokio `TcpListener` (one request per
//!   connection); upstream calls use `reqwest`.
//! - `remotemcp` package (server/pkg/remotemcp) is another lane: `Discover`
//!   and `ValidatePublicHTTPSEndpoint` are fail-closed S9-integration seams;
//!   `DigestBytes`/`MaxResponseBytes` are ported faithfully here because they
//!   are pure.
//! - `log/slog` → `tracing` with identical message text; `http.Header` →
//!   ordered `(name, value)` pairs.

// S9-integration: dead_code until Daemon core wires this.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use crate::plugin_hook_mcp::{read_request, ConnTracker};
use crate::repocache::Ctx;
use crate::types::{RemoteMcpConnection, RemoteMcpTool};

pub(crate) const REMOTE_MCP_MAX_REQUEST_BYTES: usize = 1 << 20;
pub(crate) const REMOTE_MCP_MAX_CALLS: i64 = 256;
pub(crate) const REMOTE_MCP_MAX_CONCURRENCY: usize = 8;

/// `remotemcp.MaxResponseBytes` (server/pkg/remotemcp/client.go:23).
pub(crate) const REMOTEMCP_MAX_RESPONSE_BYTES: usize = 4 << 20;

/// Ordered `(name, value)` header list standing in for Go's `http.Header`.
pub(crate) type HeaderList = Vec<(String, String)>;

/// `remoteMCPCredentialResolver` (remote_mcp_broker.go:37): resolves fresh
/// credential headers for a contribution id just before each upstream call.
pub(crate) type RemoteMCPCredentialResolver = Arc<
    dyn Fn(Ctx, String) -> futures_util::future::BoxFuture<'static, anyhow::Result<HeaderList>>
        + Send
        + Sync,
>;

// ---------------------------------------------------------------------------
// S9-integration seam stand-ins (remote_mcp_broker.go imports remotemcp).
// ---------------------------------------------------------------------------

/// S9-integration seam for `remotemcp.Discover` (client.go:188–…). Fail-closed
/// until pkg/remotemcp lands: startup validation rejects every connection so a
/// mis-wired task surfaces loudly instead of proxying unvalidated tools.
async fn discover(
    _ctx: &Ctx,
    _endpoint: &str,
    _allowed_hosts: &[String],
    _protocol_versions: &[String],
    _headers: &HeaderList,
) -> anyhow::Result<Vec<RemoteMcpTool>> {
    Err(anyhow::anyhow!(
        "S9-integration: remotemcp.Discover is not wired yet"
    ))
}

/// S9-integration seam for `remotemcp.ValidatePublicHTTPSEndpoint`
/// (client.go:32–…). Fail-closed until pkg/remotemcp lands.
async fn validate_public_https_endpoint(
    _ctx: &Ctx,
    _endpoint: &str,
    _allowed_hosts: &[String],
) -> anyhow::Result<String> {
    Err(anyhow::anyhow!(
        "S9-integration: remotemcp.ValidatePublicHTTPSEndpoint is not wired yet"
    ))
}

/// S9-integration seam for `remotemcp.NewSecureHTTPClient` (client.go:114–…):
/// builds the pinned-endpoint client. The SSRF dial-guard lands with
/// pkg/remotemcp; until then this is a plain client used only against the
/// endpoint string already validated by [`validate_public_https_endpoint`].
fn new_secure_http_client(_endpoint: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .build()
        .unwrap_or_default()
}

/// `DigestBytes` (server/pkg/remotemcp/types.go:52–56): content digest used to
/// pin approved tool schemas.
pub(crate) fn digest_bytes(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    format!("sha256:{}", hex::encode(digest))
}

/// One synthesised response from [`RemoteMCPProxy::serve_http`].
#[derive(Debug)]
pub(crate) struct RemoteMCPHttpResponse {
    pub status: u16,
    pub headers: HeaderList,
    pub body: Vec<u8>,
}

impl RemoteMCPHttpResponse {
    fn json(status: u16, headers: HeaderList, value: &serde_json::Value) -> Self {
        Self {
            status,
            headers,
            body: serde_json::to_vec(value).unwrap_or_default(),
        }
    }
}

/// `remoteMCPBrokerSet` (remote_mcp_broker.go:31–35): one shutdown token per
/// broker plus a shared drain tracker; `once sync.Once` becomes an atomic flag.
pub(crate) struct RemoteMCPBrokerSet {
    shutdowns: Vec<CancellationToken>,
    conns: Arc<ConnTracker>,
    closed: AtomicBool,
}

impl RemoteMCPBrokerSet {
    /// `Close` (remote_mcp_broker.go:39–53): idempotent shutdown draining
    /// in-flight requests for up to 2 seconds.
    pub(crate) async fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        for token in &self.shutdowns {
            token.cancel();
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), self.conns.wait_idle()).await;
    }
}

/// Outcome of [`start_task_remote_mcp_brokers`], mirroring the Go tuple
/// `(raw, diagnostics, set, error)` minus the error.
#[derive(Default)]
pub(crate) struct RemoteMCPBrokerStart {
    pub config: Option<serde_json::Value>,
    pub diagnostics: Vec<String>,
    pub set: Option<Arc<RemoteMCPBrokerSet>>,
}

/// `startTaskRemoteMCPBrokers` (remote_mcp_broker.go:55–157).
///
/// Deviation vs Go: async (listener binds); the config fragment is built with
/// `serde_json::json!` so its marshal step cannot fail.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn start_task_remote_mcp_brokers(
    setup_ctx: &Ctx,
    lifetime_ctx: &Ctx,
    task_id: &str,
    provider: &str,
    connections: Vec<RemoteMcpConnection>,
    resolve_credential: Option<RemoteMCPCredentialResolver>,
) -> anyhow::Result<RemoteMCPBrokerStart> {
    if connections.is_empty() {
        return Ok(RemoteMCPBrokerStart::default());
    }
    let mut shutdowns: Vec<CancellationToken> = Vec::new();
    let conns = Arc::new(ConnTracker::default());
    let mut servers: HashMap<String, serde_json::Value> = HashMap::new();
    let mut diagnostics: Vec<String> = Vec::new();

    macro_rules! close_set {
        () => {{
            for token in &shutdowns {
                token.cancel();
            }
        }};
    }

    for connection in connections {
        if !provider_supports_remote_mcp_broker(provider) {
            let message = format!(
                "Remote MCP {} is incompatible with provider {}",
                connection.contribution_key, provider
            );
            if connection.failure_policy == "optional" {
                diagnostics.push(message);
                continue;
            }
            close_set!();
            return Err(anyhow::anyhow!(message));
        }
        let mut headers: HeaderList = Vec::new();
        if !connection.credential_header.is_empty() {
            match &resolve_credential {
                Some(resolver) => {
                    let fut = resolver(
                        setup_ctx.clone(),
                        connection.contribution_id.clone(),
                    );
                    match fut.await {
                        Ok(resolved) => headers = resolved,
                        Err(resolve_err) => {
                            let message = format!(
                                "Remote MCP {} credential is unavailable",
                                connection.contribution_key
                            );
                            if connection.failure_policy == "optional" {
                                diagnostics.push(message);
                                continue;
                            }
                            close_set!();
                            return Err(resolve_err.context(message));
                        }
                    }
                }
                None => {
                    let message = format!(
                        "Remote MCP {} credential resolver is unavailable",
                        connection.contribution_key
                    );
                    if connection.failure_policy == "optional" {
                        diagnostics.push(message);
                        continue;
                    }
                    close_set!();
                    return Err(anyhow::anyhow!(message));
                }
            }
        }
        let discovered = discover(
            setup_ctx,
            &connection.endpoint,
            &connection.endpoint_allowed_hosts,
            &connection.protocol_versions,
            &headers,
        )
        .await;
        let validated = match discovered {
            Ok(discovered) => validate_pinned_remote_mcp_tools(&connection.approved_tools, &discovered)
                .map(|_| discovered),
            Err(err) => Err(err),
        };
        if let Err(err) = validated {
            let message = format!(
                "Remote MCP {} failed startup validation",
                connection.contribution_key
            );
            if connection.failure_policy == "optional" {
                diagnostics.push(message);
                continue;
            }
            close_set!();
            return Err(err.context(message));
        }
        let endpoint =
            validate_public_https_endpoint(setup_ctx, &connection.endpoint, &connection.endpoint_allowed_hosts)
                .await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("listen for Remote MCP broker")?;
        let path_token = random_broker_token()?;
        let addr = listener
            .local_addr()
            .context("listen for Remote MCP broker")?;

        let path = format!("/{path_token}");
        let proxy = Arc::new(RemoteMCPProxy {
            task_id: task_id.to_string(),
            endpoint,
            client: new_secure_http_client(&connection.endpoint),
            credential_headers: headers,
            resolve_credential: resolve_credential.clone(),
            path: path.clone(),
            semaphore: tokio::sync::Semaphore::new(REMOTE_MCP_MAX_CONCURRENCY),
            calls: AtomicI64::new(0),
            connection,
        });
        let shutdown = CancellationToken::new();
        shutdowns.push(shutdown.clone());
        let accept_proxy = Arc::clone(&proxy);
        let accept_task_id = task_id.to_string();
        let contribution = proxy.connection.contribution_key.clone();
        let accept_conns = Arc::clone(&conns);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    accepted = listener.accept() => match accepted {
                        Ok((stream, _peer)) => {
                            let conn_proxy = Arc::clone(&accept_proxy);
                            let conn_shutdown = shutdown.clone();
                            let guard = accept_conns.enter();
                            tokio::spawn(async move {
                                handle_connection(conn_proxy, stream, conn_shutdown).await;
                                drop(guard);
                            });
                        }
                        Err(err) => {
                            // Serve returning a non-ErrServerClosed error lands here.
                            tracing::warn!(
                                task_id = %accept_task_id,
                                contribution = %contribution,
                                error = %err,
                                "Remote MCP broker stopped unexpectedly"
                            );
                            break;
                        }
                    }
                }
            }
        });

        let name = remote_mcp_server_name(&proxy.connection);
        servers.insert(
            name,
            json!({
                "type": "http",
                "url": format!("http://{addr}{path}"),
            }),
        );
    }

    if servers.is_empty() {
        close_set!();
        return Ok(RemoteMCPBrokerStart {
            config: None,
            diagnostics,
            set: None,
        });
    }

    let set = Arc::new(RemoteMCPBrokerSet {
        shutdowns,
        conns,
        closed: AtomicBool::new(false),
    });

    // Lifetime watcher (remote_mcp_broker.go:147–150).
    {
        let set_watcher = Arc::clone(&set);
        let lifetime = lifetime_ctx.clone();
        tokio::spawn(async move {
            lifetime.cancelled().await;
            set_watcher.close().await;
        });
    }

    Ok(RemoteMCPBrokerStart {
        config: Some(json!({"mcpServers": servers})),
        diagnostics,
        set: Some(set),
    })
}

/// `providerSupportsRemoteMCPBroker` (remote_mcp_broker.go:159–166).
pub(crate) fn provider_supports_remote_mcp_broker(provider: &str) -> bool {
    matches!(provider, "codex" | "claude" | "hermes" | "qoder" | "mcode")
}

/// `validatePinnedRemoteMCPTools` (remote_mcp_broker.go:168–183).
pub(crate) fn validate_pinned_remote_mcp_tools(
    approved: &[RemoteMcpTool],
    discovered: &[RemoteMcpTool],
) -> anyhow::Result<()> {
    let available: HashMap<&str, &RemoteMcpTool> =
        discovered.iter().map(|tool| (tool.name.as_str(), tool)).collect();
    for pinned in approved {
        let current = available
            .get(pinned.name.as_str())
            .ok_or_else(|| anyhow::anyhow!("approved tool {:?} is missing", pinned.name))?;
        if current.schema_digest != pinned.schema_digest {
            return Err(anyhow::anyhow!("approved tool {:?} schema drifted", pinned.name));
        }
    }
    Ok(())
}

/// `randomBrokerToken` (remote_mcp_broker.go:185–191).
pub(crate) fn random_broker_token() -> anyhow::Result<String> {
    use rand::RngCore;
    let mut value = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut value);
    Ok(hex::encode(value))
}

/// `remoteMCPServerName` (remote_mcp_broker.go:193–205).
pub(crate) fn remote_mcp_server_name(connection: &RemoteMcpConnection) -> String {
    let name: String = connection
        .contribution_key
        .to_lowercase()
        .chars()
        .map(|r| match r {
            'a'..='z' | '0'..='9' | '-' | '_' => r,
            _ => '-',
        })
        .collect();
    let suffix: String = connection.contribution_id.replace('-', "");
    let suffix: String = suffix.chars().take(8).collect();
    format!("plugin-{name}-{suffix}")
}

/// `remoteMCPProxy` (remote_mcp_broker.go:207–218).
pub(crate) struct RemoteMCPProxy {
    task_id: String,
    connection: RemoteMcpConnection,
    endpoint: String,
    client: reqwest::Client,
    credential_headers: HeaderList,
    resolve_credential: Option<RemoteMCPCredentialResolver>,
    path: String,
    semaphore: tokio::sync::Semaphore,
    calls: AtomicI64,
}

/// `remoteMCPRequest` (remote_mcp_broker.go:220–225).
#[derive(Debug, Deserialize)]
struct RemoteMCPRequest {
    #[serde(default)]
    jsonrpc: String,
    #[serde(default)]
    id: Option<serde_json::Value>,
    #[serde(default)]
    method: String,
    #[serde(default)]
    params: Option<serde_json::Value>,
}

impl RemoteMCPProxy {
    /// `ServeHTTP` (remote_mcp_broker.go:227–351). The inbound request is
    /// pre-parsed by the shared HTTP plumbing: `(method, path, headers, body)`.
    pub(crate) async fn serve_http(
        &self,
        method: &str,
        path: &str,
        request_headers: &[(String, String)],
        body: &[u8],
    ) -> RemoteMCPHttpResponse {
    let started = Instant::now();
    let mut tool_name = String::new();
    let mut result_class = "rejected";
    let response = self
        .serve_inner(method, path, request_headers, body, &mut tool_name, &mut result_class)
        .await;
        tracing::info!(
            task_id = %self.task_id,
            installation_id = %self.connection.installation_id,
            contribution = %self.connection.contribution_key,
            tool = %tool_name,
            duration_ms = started.elapsed().as_millis() as i64,
            result_class = %result_class,
            "Remote MCP broker call"
        );
        response
    }

    async fn serve_inner(
        &self,
        method: &str,
        path: &str,
        request_headers: &[(String, String)],
        body: &[u8],
        tool_name: &mut String,
        result_class: &mut &str,
    ) -> RemoteMCPHttpResponse {
        if path != self.path || method != "POST" {
            return RemoteMCPHttpResponse {
                status: 404,
                headers: vec![("Content-Type".into(), "text/plain; charset=utf-8".into())],
                body: b"not found\n".to_vec(),
            };
        }
        if self.calls.fetch_add(1, Ordering::SeqCst) + 1 > REMOTE_MCP_MAX_CALLS {
            return write_remote_mcp_error(None, -32002, "Remote MCP task call limit exceeded");
        }
        // Go's select/default around the channel send is a non-blocking acquire.
        let _permit = match self.semaphore.try_acquire() {
            Ok(permit) => permit,
            Err(_) => {
                return write_remote_mcp_error(
                    None,
                    -32003,
                    "Remote MCP concurrency limit exceeded",
                )
            }
        };
        if body.len() > REMOTE_MCP_MAX_REQUEST_BYTES {
            return write_remote_mcp_error(None, -32600, "Remote MCP request is invalid");
        }
        let rpc_request: RemoteMCPRequest = match serde_json::from_slice(body) {
            Ok(req) => req,
            Err(_) => {
                return write_remote_mcp_error(None, -32600, "Remote MCP request is invalid")
            }
        };
        if rpc_request.jsonrpc != "2.0" {
            return write_remote_mcp_error(
                rpc_request.id.clone(),
                -32600,
                "Remote MCP request is invalid",
            );
        }
        if !allowed_remote_mcp_method(&rpc_request.method) {
            return write_remote_mcp_error(
                rpc_request.id.clone(),
                -32601,
                "Only approved Remote MCP tools are available",
            );
        }
        if rpc_request.method == "tools/call" {
            #[derive(Deserialize)]
            struct CallParams {
                #[serde(default)]
                name: String,
            }
            let params: CallParams = match rpc_request
                .params
                .as_ref()
                .and_then(|p| serde_json::from_value(p.clone()).ok())
            {
                Some(params) => params,
                None => {
                    return write_remote_mcp_error(
                        rpc_request.id.clone(),
                        -32602,
                        "Remote MCP tool is not approved",
                    )
                }
            };
            if !self.tool_approved(&params.name) {
                return write_remote_mcp_error(
                    rpc_request.id.clone(),
                    -32602,
                    "Remote MCP tool is not approved",
                );
            }
            *tool_name = params.name;
        }

        let mut upstream = self.client.post(&self.endpoint).body(body.to_vec());
        upstream = upstream
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        for header in ["Mcp-Session-Id", "Mcp-Protocol-Version", "Last-Event-ID"] {
            if let Some((_, value)) = request_headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(header))
            {
                upstream = upstream.header(header, value);
            }
        }
        let mut credential_headers = self.credential_headers.clone();
        if !self.connection.credential_header.is_empty() {
            match &self.resolve_credential {
                Some(resolver) => {
                    let fut = resolver(
                        Ctx::new(),
                        self.connection.contribution_id.clone(),
                    );
                    match fut.await {
                        Ok(resolved) => credential_headers = resolved,
                        Err(_) => {
                            *result_class = "credential_revoked";
                            return write_remote_mcp_error(
                                rpc_request.id.clone(),
                                -32005,
                                "Remote MCP credential is revoked or unavailable",
                            );
                        }
                    }
                }
                None => {}
            }
        }
        for (key, value) in &credential_headers {
            upstream = upstream.header(key, value);
        }
        let response = match upstream.send().await {
            Ok(response) => response,
            Err(_) => {
                *result_class = "remote_error";
                return write_remote_mcp_error(
                    rpc_request.id.clone(),
                    -32000,
                    "Remote MCP service is unavailable",
                );
            }
        };
        let upstream_status = response.status().as_u16();
        let upstream_headers: HeaderList = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value.to_str().ok().map(|v| (name.as_str().to_string(), v.to_string()))
            })
            .collect();
        let response_body = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(_) => {
                *result_class = "remote_error";
                return write_remote_mcp_error(
                    rpc_request.id.clone(),
                    -32000,
                    "Remote MCP service is unavailable",
                );
            }
        };
        if response_body.len() > REMOTEMCP_MAX_RESPONSE_BYTES {
            *result_class = "remote_error";
            return write_remote_mcp_error(
                rpc_request.id.clone(),
                -32001,
                "Remote MCP response exceeded the allowed limit",
            );
        }
        if !(200..300).contains(&upstream_status) {
            *result_class = "remote_error";
            return write_remote_mcp_error(
                rpc_request.id.clone(),
                -32000,
                "Remote MCP service returned an error",
            );
        }

        let mut out_headers: HeaderList = Vec::new();
        let mut out_body: Vec<u8> = Vec::new();
        if rpc_request.method == "tools/list" {
            let content_type = upstream_headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("Content-Type"))
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            let decoded = match decode_remote_mcp_sse_data(&content_type, &response_body) {
                Ok(decoded) => decoded,
                Err(_) => {
                    *result_class = "remote_error";
                    return write_remote_mcp_error(
                        rpc_request.id.clone(),
                        -32000,
                        "Remote MCP service returned an invalid response",
                    );
                }
            };
            out_body = match self.filter_tools_list_response(&decoded) {
                Ok(filtered) => filtered,
                Err(_) => {
                    *result_class = "schema_drift";
                    return write_remote_mcp_error(
                        rpc_request.id.clone(),
                        -32004,
                        "Remote MCP tool schema changed and requires review",
                    );
                }
            };
            out_headers.push(("Content-Type".into(), "application/json".into()));
        } else if let Some((_, value)) = upstream_headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("Content-Type"))
        {
            out_headers.push(("Content-Type".into(), value.clone()));
        }
        for header in ["Mcp-Session-Id", "Mcp-Protocol-Version"] {
            if let Some((_, value)) = upstream_headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(header))
            {
                out_headers.push((header.to_string(), value.clone()));
            }
        }
        *result_class = "success";
        RemoteMCPHttpResponse {
            status: upstream_status,
            headers: out_headers,
            body: out_body,
        }
    }

    /// `toolApproved` (remote_mcp_broker.go:377–384).
    fn tool_approved(&self, name: &str) -> bool {
        self.connection
            .approved_tools
            .iter()
            .any(|tool| tool.name == name)
    }

    /// `filterToolsListResponse` (remote_mcp_broker.go:386–426): keeps only the
    /// pinned tools, re-checking each schema digest against the live listing.
    fn filter_tools_list_response(&self, raw: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut response: HashMap<String, serde_json::Value> = serde_json::from_slice(raw)?;
        let result = response
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing result"))?;
        let result: HashMap<String, serde_json::Value> = serde_json::from_value(result)?;
        let tools_value = result
            .get("tools")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing tools"))?;
        #[derive(Deserialize)]
        struct ListedTool {
            #[serde(default)]
            name: String,
            #[serde(default)]
            description: String,
            #[serde(rename = "inputSchema", default)]
            input_schema: Option<serde_json::Value>,
        }
        let listed: Vec<ListedTool> = serde_json::from_value(tools_value)?;
        let mut by_name: HashMap<String, Vec<u8>> = HashMap::with_capacity(listed.len());
        for tool in listed {
            let schema_raw = serde_json::to_vec(&tool.input_schema.unwrap_or(serde_json::Value::Null))?;
            let canonical = canonical_remote_mcp_json(&schema_raw)?;
            by_name.insert(tool.name, canonical);
        }
        let mut pinned = self.connection.approved_tools.clone();
        pinned.sort_by(|a, b| a.name.cmp(&b.name));
        let mut filtered: Vec<serde_json::Value> = Vec::with_capacity(pinned.len());
        for tool in &pinned {
            let current = by_name.get(&tool.name).ok_or_else(|| anyhow::anyhow!("tool schema drift"))?;
            if digest_bytes(current) != tool.schema_digest {
                return Err(anyhow::anyhow!("tool schema drift"));
            }
            filtered.push(json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": serde_json::from_slice::<serde_json::Value>(current)?,
            }));
        }
        let mut result_out = result;
        result_out.insert("tools".into(), serde_json::to_value(&filtered)?);
        response.insert("result".into(), serde_json::to_value(&result_out)?);
        Ok(serde_json::to_vec(&response)?)
    }
}

/// `decodeRemoteMCPSSEData` (remote_mcp_broker.go:353–366).
pub(crate) fn decode_remote_mcp_sse_data(content_type: &str, raw: &[u8]) -> anyhow::Result<Vec<u8>> {
    if !content_type.to_lowercase().starts_with("text/event-stream") {
        return Ok(raw.to_vec());
    }
    for line in raw.split(|b| *b == b'\n') {
        if line.starts_with(b"data:") {
            let data: &[u8] = &line[b"data:".len()..];
            let trimmed: &[u8] = trim_ascii(data);
            if !trimmed.is_empty() {
                return Ok(trimmed.to_vec());
            }
        }
    }
    Err(anyhow::anyhow!("Remote MCP SSE response contained no data"))
}

fn trim_ascii(mut data: &[u8]) -> &[u8] {
    while let Some(first) = data.first() {
        if first.is_ascii_whitespace() {
            data = &data[1..];
        } else {
            break;
        }
    }
    while let Some(last) = data.last() {
        if last.is_ascii_whitespace() {
            data = &data[..data.len() - 1];
        } else {
            break;
        }
    }
    data
}

/// `allowedRemoteMCPMethod` (remote_mcp_broker.go:368–375).
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

/// `canonicalRemoteMCPJSON` (remote_mcp_broker.go:428–435): compact JSON with
/// deterministically ordered object keys (Go sorts map keys on Marshal;
/// serde_json's default map does the same).
pub(crate) fn canonical_remote_mcp_json(raw: &[u8]) -> anyhow::Result<Vec<u8>> {
    let value: serde_json::Value = serde_json::from_slice(raw)?;
    Ok(serde_json::to_vec(&value)?)
}

/// `writeRemoteMCPError` (remote_mcp_broker.go:437–447): JSON-RPC errors ride
/// on HTTP 200 so agents see a protocol-level failure.
fn write_remote_mcp_error(
    id: Option<serde_json::Value>,
    code: i64,
    message: &str,
) -> RemoteMCPHttpResponse {
    RemoteMCPHttpResponse::json(
        200,
        vec![("Content-Type".into(), "application/json".into())],
        &json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(serde_json::Value::Null),
            "error": {"code": code, "message": message},
        }),
    )
}

/// `mergeTaskRemoteMCPConfig` (remote_mcp_broker.go:449–475): overlays broker
/// entries onto the agent's own mcpServers document.
pub(crate) fn merge_task_remote_mcp_config(
    base: &[u8],
    overlay: &[u8],
) -> anyhow::Result<Vec<u8>> {
    #[derive(Serialize, Deserialize)]
    struct MCPServersDocument {
        #[serde(rename = "mcpServers", default)]
        mcp_servers: HashMap<String, serde_json::Value>,
    }
    if overlay.is_empty() {
        return Ok(base.to_vec());
    }
    let overlay_document: MCPServersDocument = serde_json::from_slice(overlay)?;
    let trimmed = trim_ascii(base);
    let mut base_document = MCPServersDocument {
        mcp_servers: HashMap::new(),
    };
    if !trimmed.is_empty() && trimmed != b"null" {
        let parsed: MCPServersDocument = serde_json::from_slice(trimmed)?;
        base_document = parsed;
    }
    for (name, server) in overlay_document.mcp_servers {
        base_document.mcp_servers.insert(name, server);
    }
    Ok(serde_json::to_vec(&base_document)?)
}

/// Broker-side twin of plugin_hook_mcp's connection handler: reads one request,
/// dispatches into [`RemoteMCPProxy::serve_http`], writes one response.
async fn handle_connection(
    proxy: Arc<RemoteMCPProxy>,
    mut stream: TcpStream,
    shutdown: CancellationToken,
) {
    let request = read_request(&mut stream, &shutdown).await;
    let Some((method, path, headers, body)) = request else {
        return;
    };
    let response = proxy.serve_http(&method, &path, &headers, &body).await;
    let reason = match response.status {
        200 => "OK",
        202 => "Accepted",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let mut head = format!("HTTP/1.1 {} {}\r\n", response.status, reason);
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n", response.body.len()));
    head.push_str("Connection: close\r\n\r\n");
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(&response.body).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    const CREDENTIAL: &str = "fixture-token";

    /// Minimal stand-in for pkg/remotemcp/remotemcptest (fixture.go): a local
    /// MCP upstream that rejects unauthenticated calls, lists two tools and
    /// records fixture.write values.
    struct TestUpstream {
        addr: std::net::SocketAddr,
        writes: Arc<Mutex<Vec<String>>>,
        calls: Arc<AtomicUsize>,
        status: Arc<AtomicU16>,
        shutdown: CancellationToken,
        task: tokio::task::JoinHandle<()>,
    }

    type AtomicU16 = std::sync::atomic::AtomicU16;

    impl TestUpstream {
        async fn spawn() -> Self {
            Self::spawn_with(200).await
        }

        async fn spawn_with(status: u16) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let writes = Arc::new(Mutex::new(Vec::new()));
            let calls = Arc::new(AtomicUsize::new(0));
            let status = Arc::new(AtomicU16::new(status));
            let shutdown = CancellationToken::new();
            let token = shutdown.clone();
            let writes_task = Arc::clone(&writes);
            let calls_task = Arc::clone(&calls);
            let status_inner = Arc::clone(&status);
            let task = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = token.cancelled() => break,
                        accepted = listener.accept() => match accepted {
                            Ok((mut stream, _)) => {
                                let w = Arc::clone(&writes_task);
                                let c = Arc::clone(&calls_task);
                                let s = Arc::clone(&status_inner);
                                tokio::spawn(async move {
                                    let _ = serve_fixture(&mut stream, &w, &c, s.load(Ordering::SeqCst)).await;
                                });
                            }
                            Err(_) => break,
                        }
                    }
                }
            });
            Self {
                addr,
                writes,
                calls,
                status,
                shutdown,
                task,
            }
        }

        fn url(&self) -> String {
            format!("http://{}", self.addr)
        }
    }

    async fn serve_fixture(
        stream: &mut TcpStream,
        writes: &Mutex<Vec<String>>,
        calls: &AtomicUsize,
        status: u16,
    ) -> std::io::Result<()> {
        let no_cancel = CancellationToken::new();
        let Some((_method, _path, headers, body)) = read_request(stream, &no_cancel).await else {
            return Ok(());
        };
        calls.fetch_add(1, Ordering::SeqCst);
        let auth = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("Authorization"))
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        if auth != format!("Bearer {CREDENTIAL}") {
            let payload = json!({"error": "credential rejected"}).to_string();
            return write_raw(stream, 401, &[("Content-Type", "application/json")], payload.as_bytes()).await;
        }
        let request: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!({}));
        let method = request["method"].as_str().unwrap_or("");
        if method == "notifications/initialized" {
            return write_raw(stream, status, &[], b"").await;
        }
        let result = match method {
            "tools/list" => json!({"tools": [
                {"name": "fixture.read", "description": "Read a deterministic fixture value",
                 "inputSchema": {"type": "object", "properties": {}}},
                {"name": "fixture.write", "description": "Append a value to the fixture log",
                 "inputSchema": {"type": "object", "required": ["value"],
                                 "properties": {"value": {"type": "string"}}}},
            ]}),
            "tools/call" => {
                let name = request["params"]["name"].as_str().unwrap_or("");
                let value = request["params"]["arguments"]["value"].as_str().unwrap_or("").to_string();
                match name {
                    "fixture.read" => json!({"content": [{"type": "text", "text": "fixture-value"}]}),
                    "fixture.write" => {
                        writes.lock().unwrap().push(value);
                        json!({"content": [{"type": "text", "text": "written"}]})
                    }
                    _ => {
                        let payload = json!({
                            "jsonrpc": "2.0", "id": request["id"],
                            "error": {"code": -32602, "message": "unknown tool"},
                        })
                        .to_string();
                        return write_raw(stream, 200, &[("Content-Type", "application/json")], payload.as_bytes()).await;
                    }
                }
            }
            _ => {
                let payload = json!({
                    "jsonrpc": "2.0", "id": request["id"],
                    "error": {"code": -32601, "message": "method not found"},
                })
                .to_string();
                return write_raw(stream, 200, &[("Content-Type", "application/json")], payload.as_bytes()).await;
            }
        };
        let payload = json!({"jsonrpc": "2.0", "id": request["id"], "result": result}).to_string();
        write_raw(stream, 200, &[("Content-Type", "application/json")], payload.as_bytes()).await
    }

    async fn write_raw(
        stream: &mut TcpStream,
        status: u16,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> std::io::Result<()> {
        let reason = if status == 200 { "OK" } else { "Unauthorized" };
        let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(body).await?;
        stream.shutdown().await
    }

    fn test_proxy(upstream: &TestUpstream, connection: RemoteMcpConnection) -> RemoteMCPProxy {
        RemoteMCPProxy {
            task_id: "task".into(),
            connection,
            endpoint: upstream.url(),
            client: reqwest::Client::new(),
            credential_headers: Vec::new(),
            resolve_credential: None,
            path: "/capability".into(),
            semaphore: tokio::sync::Semaphore::new(REMOTE_MCP_MAX_CONCURRENCY),
            calls: AtomicI64::new(0),
        }
    }

    /// TestRemoteMCPProxyFiltersToolsAndInjectsCredential.
    #[tokio::test]
    async fn remote_mcp_proxy_filters_tools_and_injects_credential() {
        let upstream = TestUpstream::spawn().await;
        let schema = json!({"type": "object", "properties": {}});
        let canonical = canonical_remote_mcp_json(&serde_json::to_vec(&schema).unwrap()).unwrap();
        let mut connection = RemoteMcpConnection {
            installation_id: "installation".into(),
            contribution_key: "fixture".into(),
            ..Default::default()
        };
        connection.approved_tools.push(RemoteMcpTool {
            name: "fixture.read".into(),
            description: "pinned".into(),
            input_schema: serde_json::from_slice(&canonical).unwrap(),
            schema_digest: digest_bytes(&canonical),
            risk: "read".into(),
        });
        let proxy = test_proxy(&upstream, connection);

        let response = proxy
            .serve_http(
                "POST",
                "/capability",
                &[("Authorization".into(), format!("Bearer {CREDENTIAL}"))],
                br#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#,
            )
            .await;
        assert_eq!(response.status, 200, "body: {}", String::from_utf8_lossy(&response.body));
        let body = String::from_utf8_lossy(&response.body).to_string();
        assert!(
            !body.contains("fixture.write") && body.contains("fixture.read"),
            "filtered tools/list = {body}"
        );
        assert!(body.contains("pinned"), "tools/list did not preserve reviewed description: {body}");

        let write_schema = json!({
            "type": "object",
            "required": ["value"],
            "properties": {"value": {"type": "string"}},
        });
        let write_canonical =
            canonical_remote_mcp_json(&serde_json::to_vec(&write_schema).unwrap()).unwrap();
        proxy.connection.approved_tools.push(RemoteMcpTool {
            name: "fixture.write".into(),
            description: String::new(),
            input_schema: serde_json::from_slice(&write_canonical).unwrap(),
            schema_digest: digest_bytes(&write_canonical),
            risk: "write".into(),
        });
        let write_response = proxy
            .serve_http(
                "POST",
                "/capability",
                &[("Authorization".into(), format!("Bearer {CREDENTIAL}"))],
                br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"fixture.write","arguments":{"value":"broker-write"}}}"#,
            )
            .await;
        assert_eq!(write_response.status, 200, "{}", String::from_utf8_lossy(&write_response.body));
        let writes = upstream.writes.lock().unwrap().clone();
        assert_eq!(writes.len(), 1, "writes = {writes:?}");
        assert_eq!(writes[0], "broker-write");

        upstream.shutdown.cancel();
        upstream.task.abort();
    }

    /// TestRemoteMCPProxyRejectsUnapprovedToolWithoutCallingUpstream.
    #[tokio::test]
    async fn remote_mcp_proxy_rejects_unapproved_tool_without_calling_upstream() {
        let upstream = TestUpstream::spawn().await;
        let mut connection = RemoteMcpConnection::default();
        connection.approved_tools.push(RemoteMcpTool {
            name: "allowed".into(),
            ..Default::default()
        });
        let proxy = test_proxy(&upstream, connection);
        let response = proxy
            .serve_http(
                "POST",
                "/capability",
                &[],
                br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"denied","arguments":{"secret":"not logged"}}}"#,
            )
            .await;
        let body = String::from_utf8_lossy(&response.body).to_string();
        assert_eq!(upstream.calls.load(Ordering::SeqCst), 0, "upstream was called");
        assert!(body.contains("not approved"), "response={body}");

        upstream.shutdown.cancel();
        upstream.task.abort();
    }

    /// TestRemoteMCPProxyPreservesAcceptedNotificationStatus.
    #[tokio::test]
    async fn remote_mcp_proxy_preserves_accepted_notification_status() {
        let upstream = TestUpstream::spawn_with(202).await;
        let proxy = test_proxy(&upstream, RemoteMcpConnection::default());
        let response = proxy
            .serve_http(
                "POST",
                "/capability",
                &[],
                br#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#,
            )
            .await;
        assert_eq!(response.status, 202, "status must pass through");
        assert!(response.body.is_empty(), "notification response body must be empty");

        upstream.shutdown.cancel();
        upstream.task.abort();
    }

    /// TestRemoteMCPProxyRechecksCredentialBeforeUpstreamCall.
    #[tokio::test]
    async fn remote_mcp_proxy_rechecks_credential_before_upstream_call() {
        let upstream = TestUpstream::spawn().await;
        let mut connection = RemoteMcpConnection {
            contribution_id: "contribution".into(),
            credential_header: "Authorization".into(),
            ..Default::default()
        };
        connection.approved_tools.push(RemoteMcpTool {
            name: "allowed".into(),
            ..Default::default()
        });
        let resolver: RemoteMCPCredentialResolver = Arc::new(|_ctx: Ctx, _id: String| {
            Box::pin(async { Err(anyhow::anyhow!("revoked")) }) as futures_util::future::BoxFuture<'static, _>
        });
        let proxy = RemoteMCPProxy {
            task_id: "task".into(),
            connection,
            endpoint: upstream.url(),
            client: reqwest::Client::new(),
            credential_headers: Vec::new(),
            resolve_credential: Some(resolver),
            path: "/capability".into(),
            semaphore: tokio::sync::Semaphore::new(REMOTE_MCP_MAX_CONCURRENCY),
            calls: AtomicI64::new(0),
        };
        let response = proxy
            .serve_http(
                "POST",
                "/capability",
                &[],
                br#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"allowed","arguments":{}}}"#,
            )
            .await;
        let body = String::from_utf8_lossy(&response.body).to_string();
        assert_eq!(upstream.calls.load(Ordering::SeqCst), 0, "upstream was called");
        assert!(body.contains("revoked or unavailable"), "response={body}");

        upstream.shutdown.cancel();
        upstream.task.abort();
    }

    /// TestRemoteMCPProviderMatrixAndConfigMerge.
    #[test]
    fn remote_mcp_provider_matrix_and_config_merge() {
        for provider in ["codex", "claude", "hermes", "qoder", "mcode"] {
            assert!(
                provider_supports_remote_mcp_broker(provider),
                "provider {provider} must support Remote MCP"
            );
        }
        assert!(!provider_supports_remote_mcp_broker("deveco"));
        let merged = merge_task_remote_mcp_config(
            br#"{"mcpServers":{"agent":{"command":"agent"}}}"#,
            br#"{"mcpServers":{"plugin":{"type":"http","url":"http://127.0.0.1/mcp"}}}"#,
        )
        .unwrap();
        let merged = String::from_utf8(merged).unwrap();
        assert!(merged.contains("agent") && merged.contains("plugin"), "merge = {merged}");
    }

    /// TestDecodeRemoteMCPSSEData.
    #[test]
    fn decode_remote_mcp_sse_data() {
        let raw = decode_remote_mcp_sse_data(
            "text/event-stream; charset=utf-8",
            b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1}\n\n",
        )
        .unwrap();
        assert_eq!(String::from_utf8(raw).unwrap(), r#"{"jsonrpc":"2.0","id":1}"#);
    }
}
