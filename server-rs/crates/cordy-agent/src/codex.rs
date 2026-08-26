//! Codex app-server JSON-RPC adapter.
//!
//! Codex is not an ACP process: the app-server multiplexes requests,
//! notifications, and client approvals over one newline-delimited JSON-RPC
//! stream. This module owns that protocol boundary and exposes the same
//! provider contract as the other local runtimes.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Map, Value};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::command::{filter_custom_args, filter_launch_prefix, BlockedArgMode, RuntimeCommand};
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
use crate::mcp::has_managed_config;
use crate::process::OwnedProcessTree;
use crate::stderr::{sanitize_diagnostic, with_stderr, SharedDiagnosticBuffer, DEFAULT_TAIL_BYTES};
use crate::stream::AgentLineReader;

const MESSAGE_BUFFER: usize = 256;
const ACTIVITY_BUFFER: usize = 256;
const DEFAULT_SEMANTIC_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DEFAULT_FIRST_TURN_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const TERMINATION_GRACE: Duration = Duration::from_millis(500);
const KILL_GRACE: Duration = Duration::from_secs(10);
const MAX_PATCH_BYTES: usize = 64 * 1024;

static BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> =
    LazyLock::new(|| BTreeMap::from([("--listen", BlockedArgMode::WithValue)]));

#[derive(Debug, Clone, Default)]
pub struct CodexConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct CodexBackend {
    config: CodexConfig,
}

impl CodexBackend {
    pub fn new(config: CodexConfig) -> Self {
        Self { config }
    }
}

/// Returns the provider-owned launch contract. User arguments are appended
/// after the fixed app-server stdio transport, while protocol flags remain
/// daemon-owned.
pub fn build_codex_args(options: &ExecOptions) -> Vec<String> {
    let managed_mcp = has_managed_config(options.mcp_config.as_ref());
    let mut args = Vec::with_capacity(8 + options.extra_args.len() + options.custom_args.len());
    args.extend([
        "app-server".to_string(),
        "--listen".to_string(),
        "stdio://".to_string(),
    ]);

    let mut user = filter_custom_args(&options.extra_args, &BLOCKED_ARGS).args;
    user.extend(filter_custom_args(&options.custom_args, &BLOCKED_ARGS).args);
    if managed_mcp {
        user = filter_managed_mcp_overrides(user);
    }
    if options.service_tier == "priority" {
        user = strip_fast_mode_conflicts(user);
        user.extend(["--enable".to_string(), "fast_mode".to_string()]);
    }
    args.extend(user);
    args
}

fn filter_managed_mcp_overrides(args: Vec<String>) -> Vec<String> {
    filter_config_namespace(args, |value| {
        let key = value
            .trim()
            .split_once('=')
            .map_or(value.trim(), |(key, _)| key);
        let key = key.trim();
        key == "mcp_servers" || key.starts_with("mcp_servers.")
    })
}

fn strip_fast_mode_conflicts(args: Vec<String>) -> Vec<String> {
    let args = filter_config_namespace(args, |value| {
        let key = value
            .trim()
            .split_once('=')
            .map_or(value.trim(), |(key, _)| key);
        key.trim() == "features.fast_mode"
    });
    let mut filtered = Vec::with_capacity(args.len() + 2);
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(a, b)| (a, Some(b)));
        if flag == "--disable" {
            let value = inline.or_else(|| args.get(index + 1).map(String::as_str));
            if value.is_some_and(|value| value.trim() == "fast_mode") {
                if inline.is_none() {
                    index += 1;
                }
                index += 1;
                continue;
            }
        }
        filtered.push(arg.clone());
        index += 1;
    }
    filtered
}

fn filter_config_namespace<F>(args: Vec<String>, owns: F) -> Vec<String>
where
    F: Fn(&str) -> bool,
{
    let mut filtered = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(a, b)| (a, Some(b)));
        if flag == "-c" || flag == "--config" {
            let value = inline.or_else(|| args.get(index + 1).map(String::as_str));
            if value.is_some_and(&owns) {
                if inline.is_none() {
                    index += 1;
                }
                index += 1;
                continue;
            }
        }
        filtered.push(arg.clone());
        index += 1;
    }
    filtered
}

fn command_path(command: &RuntimeCommand) -> String {
    if command.path.trim().is_empty() {
        "codex".to_string()
    } else {
        command.path.clone()
    }
}

fn configure_child_environment(command: &mut Command, extra: &BTreeMap<String, String>) {
    command.env_clear();
    for (key, value) in std::env::vars_os() {
        let Some(key_text) = key.to_str() else {
            command.env(key, value);
            continue;
        };
        if key_text.to_ascii_uppercase().starts_with("CORDY_")
            || key_text == "CLAUDECODE"
            || key_text.starts_with("CLAUDECODE_")
        {
            continue;
        }
        command.env(key, value);
    }
    command.envs(extra);
}

async fn write_managed_codex_mcp(
    codex_home: Option<&str>,
    config: Option<&Value>,
) -> Result<(), AgentError> {
    let Some(home) = codex_home.filter(|home| !home.trim().is_empty()) else {
        if has_managed_config(config) {
            return Err(AgentError::InvalidConfig(
                "codex: mcp_config is set but CODEX_HOME env var is not configured".to_string(),
            ));
        }
        return Ok(());
    };

    let path = Path::new(home).join("config.toml");
    let existing = match tokio::fs::read_to_string(&path).await {
        Ok(existing) => existing,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(AgentError::Process(error)),
    };
    let begin = "# BEGIN cordy-managed mcp_servers (do not edit; regenerated by daemon)";
    let end = "# END cordy-managed mcp_servers";
    let mut base = remove_managed_block(&existing, begin, end);
    if let Some(config) = config.filter(|value| !value.is_null()) {
        let object = config.as_object().ok_or_else(|| {
            AgentError::InvalidConfig(
                "codex managed MCP configuration must be an object".to_string(),
            )
        })?;
        let servers = match object
            .get("mcpServers")
            .or_else(|| object.get("mcp_servers"))
        {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_object().ok_or_else(|| {
                AgentError::InvalidConfig(
                    "codex managed MCP configuration mcpServers must be an object".to_string(),
                )
            })?),
        };
        base = strip_user_mcp_tables(&base);
        if !base.is_empty() && !base.ends_with('\n') {
            base.push('\n');
        }
        base.push_str(begin);
        base.push('\n');
        if let Some(servers) = servers {
            for (name, value) in servers {
                render_codex_mcp_server(&mut base, name, value)?;
            }
        }
        base.push_str(end);
        base.push('\n');
    }
    tokio::fs::create_dir_all(home)
        .await
        .map_err(AgentError::Process)?;
    let tmp = Path::new(home).join(".config.toml.cordy.tmp");
    tokio::fs::write(&tmp, base.as_bytes())
        .await
        .map_err(AgentError::Process)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(AgentError::Process)?;
    }
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(AgentError::Process)
}

fn remove_managed_block(input: &str, begin: &str, end: &str) -> String {
    let Some(start) = input.find(begin) else {
        return input.to_string();
    };
    let Some(end_offset) = input[start..].find(end) else {
        return input.to_string();
    };
    let mut result = String::with_capacity(input.len());
    result.push_str(&input[..start]);
    let after = start + end_offset + end.len();
    result.push_str(input[after..].trim_start_matches('\n'));
    result
}

fn strip_user_mcp_tables(input: &str) -> String {
    let mut output = String::new();
    let mut skipping = false;
    for line in input.lines() {
        let trimmed = line.trim();
        let is_mcp = trimmed.starts_with("[mcp_servers.") || trimmed.starts_with("[[mcp_servers.");
        if is_mcp {
            skipping = true;
            continue;
        }
        if skipping && trimmed.starts_with('[') {
            skipping = false;
        }
        if !skipping {
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

fn render_codex_mcp_server(
    output: &mut String,
    name: &str,
    value: &Value,
) -> Result<(), AgentError> {
    let object = value.as_object().ok_or_else(|| {
        AgentError::InvalidConfig(format!("codex mcp_servers.{name} must be an object"))
    })?;
    output.push_str("[mcp_servers.");
    output.push_str(&toml_key(name));
    output.push_str("]\n");
    let command = object.get("command").and_then(Value::as_str);
    let url = object.get("url").and_then(Value::as_str);
    if command.is_none() && url.is_none() {
        return Err(AgentError::InvalidConfig(format!(
            "codex mcp_servers.{name} must declare command or url"
        )));
    }
    if let Some(command) = command {
        output.push_str("command = ");
        output.push_str(&toml_string(command));
        output.push('\n');
    }
    if let Some(url) = url {
        output.push_str("url = ");
        output.push_str(&toml_string(url));
        output.push('\n');
    }
    let remote = object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.eq_ignore_ascii_case("http"))
        || (url.is_some() && command.is_none());
    let has_http_headers =
        object.contains_key("http_headers") || object.contains_key("httpHeaders");
    for (key, field) in object {
        match key.as_str() {
            "command" | "url" | "type" | "tools" | "prompts" | "resources" => {}
            "args" => {
                let Some(values) = field.as_array() else {
                    return Err(AgentError::InvalidConfig(format!(
                        "codex mcp_servers.{name}.args must be an array"
                    )));
                };
                let rendered = values
                    .iter()
                    .map(|value| {
                        value.as_str().map(toml_string).ok_or_else(|| {
                            AgentError::InvalidConfig(format!(
                                "codex mcp_servers.{name}.args must contain strings"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                output.push_str("args = [");
                output.push_str(&rendered.join(", "));
                output.push_str("]\n");
            }
            "env" | "headers" | "httpHeaders" | "http_headers" => {
                if key == "headers" && has_http_headers {
                    continue;
                }
                let Some(values) = field.as_object() else {
                    return Err(AgentError::InvalidConfig(format!(
                        "codex mcp_servers.{name}.{key} must be an object"
                    )));
                };
                output.push_str(if key == "env" { "env" } else { "http_headers" });
                output.push_str(" = { ");
                let mut fields = Vec::new();
                for (field_name, field_value) in values {
                    let Some(field_value) = field_value.as_str() else {
                        return Err(AgentError::InvalidConfig(format!(
                            "codex mcp_servers.{name}.{key} values must be strings"
                        )));
                    };
                    fields.push(format!(
                        "{} = {}",
                        toml_key(field_name),
                        toml_string(field_value)
                    ));
                }
                output.push_str(&fields.join(", "));
                output.push_str(" }\n");
            }
            "experimental_use_rmcp_client" => {
                if let Some(value) = field.as_bool() {
                    output.push_str("experimental_use_rmcp_client = ");
                    output.push_str(if value { "true\n" } else { "false\n" });
                }
            }
            _ => {}
        }
    }
    if remote {
        output.push_str("experimental_use_rmcp_client = true\n");
    }
    output.push('\n');
    Ok(())
}

fn toml_key(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        value.to_string()
    } else {
        toml_string(value)
    }
}

fn toml_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                output.push_str(&format!("\\u{:04x}", character as u32))
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

#[async_trait]
impl Backend for CodexBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        let executable = command_path(&self.config.command);
        let mut prefix = filter_launch_prefix(&self.config.command.prefix, &BLOCKED_ARGS).args;
        if has_managed_config(options.mcp_config.as_ref()) {
            prefix = filter_managed_mcp_overrides(prefix);
        }
        if options.service_tier == "priority" {
            prefix = strip_fast_mode_conflicts(prefix);
        }
        let args = build_codex_args(&options);
        let mut command = Command::new(&executable);
        command
            .args(prefix.iter().chain(args.iter()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);
        configure_child_environment(&mut command, &self.config.env);
        if !options.cwd.is_empty() {
            command.current_dir(&options.cwd);
        }

        if self.config.env.contains_key("CODEX_HOME") {
            write_managed_codex_mcp(
                self.config.env.get("CODEX_HOME").map(String::as_str),
                options.mcp_config.as_ref(),
            )
            .await?;
        }

        let mut tree = OwnedProcessTree::spawn(&mut command)
            .await
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    AgentError::ExecutableNotFound(executable.clone())
                } else {
                    AgentError::Process(error)
                }
            })?;
        let stdin = tree.child_mut().stdin.take().ok_or_else(|| {
            AgentError::Protocol("Codex stdin pipe unavailable after spawn".to_string())
        })?;
        let stdout = tree.child_mut().stdout.take().ok_or_else(|| {
            AgentError::Protocol("Codex stdout pipe unavailable after spawn".to_string())
        })?;
        let stderr = tree.child_mut().stderr.take().ok_or_else(|| {
            AgentError::Protocol("Codex stderr pipe unavailable after spawn".to_string())
        })?;

        let (messages_tx, messages_rx) = mpsc::channel(MESSAGE_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();
        let (activity_tx, activity_rx) = mpsc::channel(ACTIVITY_BUFFER);
        let (turn_done_tx, turn_done_rx) = mpsc::channel(8);
        let (event_tx, event_rx) = mpsc::channel(256);
        let client = CodexClient::new(stdin, event_tx);
        let observer = CodexObserver::new(messages_tx, activity_tx, turn_done_tx);
        let started = Instant::now();
        let prompt = prompt.to_string();
        let cancellation = options.cancellation.clone();
        let config = self.config.clone();

        tokio::spawn(async move {
            run_codex(
                tree,
                client,
                observer,
                stdout,
                stderr,
                event_rx,
                activity_rx,
                turn_done_rx,
                result_tx,
                prompt,
                options,
                config,
                started,
            )
            .await;
            drop(cancellation);
        });

        Ok(Session {
            messages: messages_rx,
            result: result_rx,
        })
    }
}

#[derive(Clone)]
struct CodexClient {
    inner: Arc<CodexClientInner>,
}

struct CodexClientInner {
    stdin: Mutex<Option<ChildStdin>>,
    pending: Mutex<BTreeMap<u64, oneshot::Sender<Result<Value, RpcError>>>>,
    next_id: std::sync::atomic::AtomicU64,
    event_tx: mpsc::Sender<WireEvent>,
    process_error: Mutex<Option<String>>,
}

#[derive(Debug)]
struct WireEvent {
    method: String,
    params: Value,
}

#[derive(Debug, Clone)]
struct RpcError {
    code: i64,
    message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} (code={})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

impl CodexClient {
    fn new(stdin: ChildStdin, event_tx: mpsc::Sender<WireEvent>) -> Self {
        Self {
            inner: Arc::new(CodexClientInner {
                stdin: Mutex::new(Some(stdin)),
                pending: Mutex::new(BTreeMap::new()),
                next_id: std::sync::atomic::AtomicU64::new(1),
                event_tx,
                process_error: Mutex::new(None),
            }),
        }
    }

    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout_duration: Duration,
        cancellation: &CancellationToken,
    ) -> Result<Value, AgentError> {
        let id = self
            .inner
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.inner.pending.lock().await.insert(id, sender);
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(error) = self.write_frame(&frame).await {
            self.inner.pending.lock().await.remove(&id);
            return Err(AgentError::Protocol(format!("write {method}: {error}")));
        }
        let response = tokio::select! {
            result = receiver => result
                .map_err(|_| AgentError::Protocol("codex app-server process exited".to_string()))?
                .map_err(|error| AgentError::Protocol(format!("{method}: {error}")))?,
            _ = tokio::time::sleep(timeout_duration) => {
                self.inner.pending.lock().await.remove(&id);
                return Err(AgentError::Protocol(format!(
                    "codex app-server handshake timeout: {method} did not respond after {}s",
                    timeout_duration.as_secs_f64()
                )));
            }
            _ = cancellation.cancelled() => {
                self.inner.pending.lock().await.remove(&id);
                return Err(AgentError::Protocol("execution cancelled".to_string()));
            }
        };
        Ok(response)
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> io::Result<()> {
        let mut frame = Map::new();
        frame.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
        frame.insert("method".to_string(), Value::String(method.to_string()));
        if let Some(params) = params {
            frame.insert("params".to_string(), params);
        }
        self.write_frame(&Value::Object(frame)).await
    }

    async fn respond(&self, id: Value, result: Value) -> io::Result<()> {
        self.write_frame(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
        .await
    }

    async fn respond_error(&self, id: Value, code: i64, message: &str) -> io::Result<()> {
        self.write_frame(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message},
        }))
        .await
    }

    async fn write_frame(&self, frame: &Value) -> io::Result<()> {
        let mut encoded = serde_json::to_vec(frame)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        encoded.push(b'\n');
        let mut stdin = self.inner.stdin.lock().await;
        let Some(stdin) = stdin.as_mut() else {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Codex stdin closed",
            ));
        };
        stdin.write_all(&encoded).await?;
        stdin.flush().await
    }

    async fn close_stdin(&self) {
        self.inner.stdin.lock().await.take();
    }

    async fn mark_process_error(&self, error: impl Into<String>) {
        let error = error.into();
        let mut process_error = self.inner.process_error.lock().await;
        if process_error.is_none() {
            *process_error = Some(error.clone());
        }
        let mut pending = self.inner.pending.lock().await;
        for (_, sender) in std::mem::take(&mut *pending) {
            let _ = sender.send(Err(RpcError {
                code: -32000,
                message: error.clone(),
            }));
        }
    }

    async fn process_error(&self) -> Option<String> {
        self.inner.process_error.lock().await.clone()
    }
}

async fn read_codex_stdout(stdout: ChildStdout, client: CodexClient) {
    let mut reader = AgentLineReader::new(BufReader::new(stdout));
    loop {
        let line = match reader.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => {
                client
                    .mark_process_error("codex app-server process exited")
                    .await;
                return;
            }
            Err(error) => {
                client
                    .mark_process_error(format!("codex stdout read error: {error}"))
                    .await;
                return;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(frame) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(object) = frame.as_object() else {
            continue;
        };
        if let Some(id) = object.get("id") {
            if let Some(method) = object.get("method").and_then(Value::as_str) {
                let params = object.get("params").cloned().unwrap_or(Value::Null);
                handle_server_request(&client, id.clone(), method, params).await;
            } else {
                let request_id = id.as_u64().unwrap_or(0);
                let sender = client.inner.pending.lock().await.remove(&request_id);
                if let Some(sender) = sender {
                    if let Some(error) = object.get("error") {
                        let rpc_error = RpcError {
                            code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                            message: error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("Codex RPC error")
                                .to_string(),
                        };
                        let _ = sender.send(Err(rpc_error));
                    } else {
                        let _ =
                            sender.send(Ok(object.get("result").cloned().unwrap_or(Value::Null)));
                    }
                }
            }
        } else if let Some(method) = object.get("method").and_then(Value::as_str) {
            let _ = client
                .inner
                .event_tx
                .send(WireEvent {
                    method: method.to_string(),
                    params: object.get("params").cloned().unwrap_or(Value::Null),
                })
                .await;
        }
    }
}

async fn handle_server_request(client: &CodexClient, id: Value, method: &str, params: Value) {
    let result = match method {
        "item/commandExecution/requestApproval" | "execCommandApproval" => {
            Some(serde_json::json!({"decision": "accept"}))
        }
        "item/fileChange/requestApproval" | "applyPatchApproval" => {
            Some(serde_json::json!({"decision": "accept"}))
        }
        "item/permissions/requestApproval" => Some(permission_response(params)),
        "mcpServer/elicitation/request" => {
            Some(serde_json::json!({"action": "accept", "content": null, "_meta": null}))
        }
        _ => None,
    };
    if let Some(result) = result {
        let _ = client.respond(id, result).await;
    } else {
        let _ = client
            .respond_error(id, -32601, &format!("method not found: {method}"))
            .await;
        client
            .mark_process_error(format!("unsupported Codex app-server request: {method}"))
            .await;
    }
}

fn permission_response(params: Value) -> Value {
    let mut permissions = Map::new();
    if let Some(object) = params.get("permissions").and_then(Value::as_object) {
        for key in ["network", "fileSystem"] {
            if let Some(value) = object.get(key) {
                permissions.insert(key.to_string(), value.clone());
            }
        }
    }
    serde_json::json!({"permissions": permissions, "scope": "turn"})
}

async fn pump_stderr(mut stderr: ChildStderr, tail: SharedDiagnosticBuffer) {
    let mut buffer = [0_u8; 4096];
    loop {
        match tokio::io::AsyncReadExt::read(&mut stderr, &mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(count) => tail.push(&buffer[..count]),
        }
    }
}

async fn cleanup_codex(
    tree: &mut OwnedProcessTree,
    client: &CodexClient,
    reader_task: JoinHandle<()>,
    stderr_task: JoinHandle<()>,
    event_task: JoinHandle<()>,
    stderr_tail: &SharedDiagnosticBuffer,
) -> String {
    client.close_stdin().await;
    let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
    reader_task.abort();
    event_task.abort();
    let _ = tokio::time::timeout(Duration::from_secs(2), stderr_task).await;
    stderr_tail.tail()
}

#[derive(Clone)]
struct CodexObserver {
    state: Arc<Mutex<ObserverState>>,
    messages: mpsc::Sender<Message>,
    activity: mpsc::Sender<String>,
    turn_done: mpsc::Sender<bool>,
}

#[derive(Debug, Default)]
struct ObserverState {
    thread_id: String,
    turn_id: String,
    protocol: ProtocolKind,
    gate_armed: bool,
    turn_started: bool,
    turn_done: bool,
    completed_turns: BTreeSet<String>,
    final_answer: String,
    last_agent_message: String,
    turn_error: String,
    usage: TokenUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtocolKind {
    Unknown,
    Legacy,
    Raw,
}

impl Default for ProtocolKind {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug, Clone, Default)]
struct ObserverSnapshot {
    final_answer: String,
    last_agent_message: String,
    turn_error: String,
    thread_id: String,
    usage: TokenUsage,
}

impl CodexObserver {
    fn new(
        messages: mpsc::Sender<Message>,
        activity: mpsc::Sender<String>,
        turn_done: mpsc::Sender<bool>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(ObserverState::default())),
            messages,
            activity,
            turn_done,
        }
    }

    async fn set_thread(&self, thread_id: &str) {
        self.state.lock().await.thread_id = thread_id.to_string();
    }

    async fn arm(&self) {
        self.state.lock().await.gate_armed = true;
    }

    async fn snapshot(&self) -> ObserverSnapshot {
        let state = self.state.lock().await;
        ObserverSnapshot {
            final_answer: state.final_answer.clone(),
            last_agent_message: state.last_agent_message.clone(),
            turn_error: state.turn_error.clone(),
            thread_id: state.thread_id.clone(),
            usage: state.usage,
        }
    }

    async fn handle_notification(&self, method: &str, params: Value) {
        {
            let state = self.state.lock().await;
            if !state.gate_armed {
                return;
            }
            if let Some(thread_id) = params.get("threadId").and_then(Value::as_str) {
                if !state.thread_id.is_empty() && thread_id != state.thread_id {
                    return;
                }
            }
            let event_turn_id = params
                .get("turnId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    params
                        .get("turn")
                        .and_then(|turn| turn.get("id"))
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                });
            if let Some(event_turn_id) = event_turn_id {
                if !state.turn_id.is_empty() && event_turn_id != state.turn_id {
                    return;
                }
            }
            if state.protocol != ProtocolKind::Legacy
                && !state.turn_started
                && (method.starts_with("item/")
                    || method == "turn/completed"
                    || method == "thread/status/changed")
            {
                // A resumed thread can replay the previous turn on the same
                // JSON-RPC stream. Do not let replayed items/completion end
                // the new turn before its own turn/started boundary.
                return;
            }
        }

        if method == "codex/event" || method.starts_with("codex/event/") {
            self.state.lock().await.protocol = ProtocolKind::Legacy;
            if let Some(message) = params.get("msg") {
                self.handle_legacy_event(message).await;
            }
            return;
        }

        {
            let mut state = self.state.lock().await;
            if state.protocol == ProtocolKind::Legacy {
                return;
            }
            if state.protocol == ProtocolKind::Unknown
                && (method == "turn/started"
                    || method == "turn/completed"
                    || method == "thread/started"
                    || method.starts_with("item/"))
            {
                state.protocol = ProtocolKind::Raw;
            }
        }
        self.handle_raw_notification(method, params).await;
    }

    async fn handle_legacy_event(&self, value: &Value) {
        let Some(message) = value.as_object() else {
            return;
        };
        match message
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "task_started" => {
                self.state.lock().await.turn_started = true;
                self.emit(
                    Message {
                        message_type: MessageType::Status,
                        status: "running".to_string(),
                        session_id: self.state.lock().await.thread_id.clone(),
                        ..Message::default()
                    },
                    "status:running",
                )
                .await;
            }
            "agent_message" => {
                if let Some(text) = message.get("message").and_then(Value::as_str) {
                    self.emit_text(text, false).await;
                }
            }
            "exec_command_begin" => {
                let call_id = string_field(message, "call_id");
                let command = string_field(message, "command");
                self.emit(
                    Message {
                        message_type: MessageType::ToolUse,
                        tool: "exec_command".to_string(),
                        call_id,
                        input: one_string_input("command", &command),
                        ..Message::default()
                    },
                    "tool_use:exec_command",
                )
                .await;
            }
            "exec_command_end" => {
                self.emit(
                    Message {
                        message_type: MessageType::ToolResult,
                        tool: "exec_command".to_string(),
                        call_id: string_field(message, "call_id"),
                        output: string_field(message, "output"),
                        ..Message::default()
                    },
                    "tool_result:exec_command",
                )
                .await;
            }
            "patch_apply_begin" => {
                let changes = normalize_legacy_changes(message.get("changes"));
                self.emit(
                    Message {
                        message_type: MessageType::ToolUse,
                        tool: "patch_apply".to_string(),
                        call_id: string_field(message, "call_id"),
                        input: patch_input(changes.as_ref()),
                        ..Message::default()
                    },
                    "tool_use:patch_apply",
                )
                .await;
            }
            "patch_apply_end" => {
                let changes = normalize_legacy_changes(message.get("changes"));
                let status = string_field(message, "status");
                let status = if status.is_empty() {
                    if message.get("success").and_then(Value::as_bool) == Some(true) {
                        "completed".to_string()
                    } else {
                        "failed".to_string()
                    }
                } else {
                    status
                };
                self.emit(
                    Message {
                        message_type: MessageType::ToolResult,
                        tool: "patch_apply".to_string(),
                        call_id: string_field(message, "call_id"),
                        output: patch_result_output(
                            &normalize_patch_status(&status),
                            changes.as_ref(),
                            string_field(message, "stdout"),
                            string_field(message, "stderr"),
                        ),
                        ..Message::default()
                    },
                    "tool_result:patch_apply",
                )
                .await;
            }
            "task_complete" => {
                self.add_usage(message).await;
                self.finish_turn(false).await;
            }
            "turn_aborted" => self.finish_turn(true).await,
            _ => {}
        }
    }

    async fn handle_raw_notification(&self, method: &str, params: Value) {
        match method {
            "turn/started" => {
                let turn_id = nested_string(&params, &["turn", "id"]);
                {
                    let mut state = self.state.lock().await;
                    state.turn_started = true;
                    if !turn_id.is_empty() {
                        state.turn_id = turn_id;
                    }
                }
                let session_id = self.state.lock().await.thread_id.clone();
                self.emit(
                    Message {
                        message_type: MessageType::Status,
                        status: "running".to_string(),
                        session_id,
                        ..Message::default()
                    },
                    "status:running",
                )
                .await;
            }
            "turn/completed" => {
                let turn_id = nested_string(&params, &["turn", "id"]);
                let status = nested_string(&params, &["turn", "status"]);
                let duplicate = {
                    let mut state = self.state.lock().await;
                    if turn_id.is_empty() {
                        false
                    } else {
                        !state.completed_turns.insert(turn_id)
                    }
                };
                if duplicate {
                    return;
                }
                if status == "failed" {
                    let error = nested_string(&params, &["turn", "error", "message"]);
                    self.set_turn_error(if error.is_empty() {
                        "codex turn failed"
                    } else {
                        &error
                    })
                    .await;
                }
                if let Some(turn) = params.get("turn") {
                    self.add_usage_value(turn).await;
                }
                self.finish_turn(matches!(
                    status.as_str(),
                    "cancelled" | "canceled" | "aborted" | "interrupted"
                ))
                .await;
            }
            "error" => {
                let will_retry = params
                    .get("willRetry")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let message = nested_string(&params, &["error", "message"]);
                let message = if message.is_empty() {
                    string_field(params.as_object().unwrap_or(&Map::new()), "message")
                } else {
                    message
                };
                if !message.is_empty() {
                    let _ = self.activity.try_send(if will_retry {
                        "error:retry".to_string()
                    } else {
                        "error:terminal".to_string()
                    });
                    if !will_retry {
                        self.set_turn_error(&message).await;
                        self.finish_turn(false).await;
                    }
                }
            }
            "thread/status/changed" => {
                if nested_string(&params, &["status", "type"]) == "idle"
                    && self.state.lock().await.turn_started
                {
                    self.finish_turn(false).await;
                }
            }
            _ if method.starts_with("item/") => self.handle_item(method, &params).await,
            _ => {}
        }
    }

    async fn handle_item(&self, method: &str, params: &Value) {
        let Some(item) = params.get("item").and_then(Value::as_object) else {
            return;
        };
        let item_type = string_field(item, "type");
        let item_id = string_field(item, "id");
        let _ = self.activity.try_send(format!(
            "{}:{}{}",
            method,
            if item_type.is_empty() {
                "unknown"
            } else {
                &item_type
            },
            if item_id.is_empty() {
                String::new()
            } else {
                format!(":{item_id}")
            }
        ));
        match (method, item_type.as_str()) {
            ("item/started", "commandExecution") => {
                self.emit(
                    Message {
                        message_type: MessageType::ToolUse,
                        tool: "exec_command".to_string(),
                        call_id: item_id,
                        input: one_string_input("command", &string_field(item, "command")),
                        ..Message::default()
                    },
                    "tool_use:exec_command",
                )
                .await;
            }
            ("item/completed", "commandExecution") => {
                self.emit(
                    Message {
                        message_type: MessageType::ToolResult,
                        tool: "exec_command".to_string(),
                        call_id: item_id,
                        output: string_field(item, "aggregatedOutput"),
                        ..Message::default()
                    },
                    "tool_result:exec_command",
                )
                .await;
            }
            ("item/started", "fileChange") => {
                let changes = normalize_raw_changes(item.get("changes"));
                self.emit(
                    Message {
                        message_type: MessageType::ToolUse,
                        tool: "patch_apply".to_string(),
                        call_id: item_id,
                        input: patch_input(changes.as_ref()),
                        ..Message::default()
                    },
                    "tool_use:patch_apply",
                )
                .await;
            }
            ("item/completed", "fileChange") => {
                let changes = normalize_raw_changes(item.get("changes"));
                let status = normalize_patch_status(&string_field(item, "status"));
                self.emit(
                    Message {
                        message_type: MessageType::ToolResult,
                        tool: "patch_apply".to_string(),
                        call_id: item_id,
                        output: patch_result_output(
                            &status,
                            changes.as_ref(),
                            String::new(),
                            String::new(),
                        ),
                        ..Message::default()
                    },
                    "tool_result:patch_apply",
                )
                .await;
            }
            ("item/started", "mcpToolCall") => {
                self.emit(
                    Message {
                        message_type: MessageType::ToolUse,
                        tool: item
                            .get("tool")
                            .and_then(Value::as_str)
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or("mcp_tool")
                            .to_string(),
                        call_id: item_id,
                        input: mcp_tool_input(item),
                        ..Message::default()
                    },
                    "tool_use:mcp",
                )
                .await;
            }
            ("item/completed", "mcpToolCall") => {
                let status = normalize_patch_status(&string_field(item, "status"));
                self.emit(
                    Message {
                        message_type: MessageType::ToolResult,
                        tool: item
                            .get("tool")
                            .and_then(Value::as_str)
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or("mcp_tool")
                            .to_string(),
                        call_id: item_id,
                        output: mcp_tool_result(item, &status),
                        status,
                        ..Message::default()
                    },
                    "tool_result:mcp",
                )
                .await;
            }
            ("item/completed", "agentMessage") => {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    let final_answer = string_field(item, "phase") == "final_answer";
                    self.emit_text(text, final_answer).await;
                    if final_answer && self.state.lock().await.turn_started {
                        self.finish_turn(false).await;
                    }
                }
            }
            _ => {}
        }
    }

    async fn set_turn_error(&self, message: &str) {
        let mut state = self.state.lock().await;
        if state.turn_error.is_empty() {
            state.turn_error = message.to_string();
        }
    }

    async fn finish_turn(&self, aborted: bool) {
        let should_send = {
            let mut state = self.state.lock().await;
            if state.turn_done {
                false
            } else {
                state.turn_done = true;
                true
            }
        };
        if should_send {
            let _ = self.turn_done.try_send(aborted);
        }
    }

    async fn emit_text(&self, text: &str, final_answer: bool) {
        if text.is_empty() {
            return;
        }
        {
            let mut state = self.state.lock().await;
            state.last_agent_message = text.to_string();
            if final_answer {
                state.final_answer = text.to_string();
            }
        }
        self.emit(
            Message {
                message_type: MessageType::Text,
                content: text.to_string(),
                ..Message::default()
            },
            "text",
        )
        .await;
    }

    async fn emit(&self, message: Message, activity: &str) {
        let _ = self.messages.try_send(message);
        if !activity.is_empty() {
            let _ = self.activity.try_send(activity.to_string());
        }
    }

    async fn add_usage(&self, object: &Map<String, Value>) {
        self.add_usage_value(&Value::Object(object.clone())).await;
    }

    async fn add_usage_value(&self, value: &Value) {
        let Some(object) = value.as_object() else {
            return;
        };
        let usage = ["usage", "token_usage", "tokens"]
            .iter()
            .find_map(|key| object.get(*key).and_then(Value::as_object));
        let Some(usage) = usage else {
            return;
        };
        let input = first_nonzero_i64(usage, &["input_tokens", "input", "prompt_tokens"]);
        let cached = first_nonzero_i64(
            usage,
            &[
                "cached_input_tokens",
                "cache_read_tokens",
                "cache_read_input_tokens",
            ],
        );
        let output = first_nonzero_i64(usage, &["output_tokens", "output", "completion_tokens"]);
        let write = first_nonzero_i64(
            usage,
            &["cache_write_tokens", "cache_creation_input_tokens"],
        );
        let mut state = self.state.lock().await;
        state.usage.input_tokens += input.saturating_sub(cached).max(0);
        state.usage.output_tokens += output;
        state.usage.cache_read_tokens += cached;
        state.usage.cache_write_tokens += write;
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_codex(
    mut tree: OwnedProcessTree,
    client: CodexClient,
    observer: CodexObserver,
    stdout: ChildStdout,
    stderr: ChildStderr,
    mut event_rx: mpsc::Receiver<WireEvent>,
    mut activity_rx: mpsc::Receiver<String>,
    mut turn_done_rx: mpsc::Receiver<bool>,
    result_tx: oneshot::Sender<ExecutionResult>,
    prompt: String,
    options: ExecOptions,
    _config: CodexConfig,
    started: Instant,
) {
    let stderr_tail = SharedDiagnosticBuffer::new(DEFAULT_TAIL_BYTES);
    let reader_task: JoinHandle<()> = tokio::spawn(read_codex_stdout(stdout, client.clone()));
    let stderr_task = tokio::spawn(pump_stderr(stderr, stderr_tail.clone()));
    let event_observer = observer.clone();
    let event_task = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            event_observer
                .handle_notification(&event.method, event.params)
                .await;
        }
    });

    let handshake_timeout = nonzero_duration(options.handshake_timeout, DEFAULT_HANDSHAKE_TIMEOUT);
    let semantic_timeout = nonzero_duration(
        options.semantic_inactivity_timeout,
        DEFAULT_SEMANTIC_TIMEOUT,
    );
    let execution_deadline =
        (options.timeout > Duration::ZERO).then(|| tokio::time::Instant::now() + options.timeout);

    let initialization = client
        .request(
            "initialize",
            serde_json::json!({
                "clientInfo": {
                    "name": "cordy-agent-sdk",
                    "title": "Cordy Agent SDK",
                    "version": "0.2.0"
                },
                "capabilities": {"experimentalApi": true}
            }),
            effective_request_timeout(execution_deadline, handshake_timeout),
            &options.cancellation,
        )
        .await;
    if let Err(error) = initialization {
        let stderr = cleanup_codex(
            &mut tree,
            &client,
            reader_task,
            stderr_task,
            event_task,
            &stderr_tail,
        )
        .await;
        let mut message = format!("codex initialize failed: {error}");
        if !stderr.is_empty() {
            message = with_stderr(&message, "codex", &sanitize_diagnostic(&stderr));
        }
        let _ = result_tx.send(ExecutionResult {
            status: "failed".to_string(),
            error: message,
            duration_ms: started.elapsed().as_millis() as i64,
            ..ExecutionResult::default()
        });
        return;
    }
    let _ = client.notify("initialized", None).await;

    let mut resumed = false;
    let mut thread_id = String::new();
    if !options.resume_session_id.trim().is_empty() {
        observer.set_thread(&options.resume_session_id).await;
        let mut params = serde_json::json!({
            "threadId": options.resume_session_id,
            "cwd": null_if_empty(&options.cwd),
            "model": null_if_empty(&options.model),
            "developerInstructions": null
        });
        apply_reasoning(&mut params, &options.thinking_level);
        apply_service_tier(&mut params, &options.service_tier);
        match client
            .request(
                "thread/resume",
                params,
                effective_request_timeout(execution_deadline, handshake_timeout),
                &options.cancellation,
            )
            .await
        {
            Ok(value) => {
                thread_id = extract_thread_id(&value);
                resumed = !thread_id.is_empty();
            }
            Err(error) if is_resume_overflow(&error) => {
                let _ = cleanup_codex(
                    &mut tree,
                    &client,
                    reader_task,
                    stderr_task,
                    event_task,
                    &stderr_tail,
                )
                .await;
                let _ = result_tx.send(ExecutionResult {
                    status: "failed".to_string(),
                    error: format!("codex thread/resume failed: {error}"),
                    duration_ms: started.elapsed().as_millis() as i64,
                    resume_rejected: true,
                    ..ExecutionResult::default()
                });
                return;
            }
            Err(error) if is_transport_error(&error) => {
                let stderr = cleanup_codex(
                    &mut tree,
                    &client,
                    reader_task,
                    stderr_task,
                    event_task,
                    &stderr_tail,
                )
                .await;
                let mut message = format!("codex thread/resume failed: {error}");
                if !stderr.is_empty() {
                    message = with_stderr(&message, "codex", &sanitize_diagnostic(&stderr));
                }
                let _ = result_tx.send(ExecutionResult {
                    status: "failed".to_string(),
                    error: message,
                    duration_ms: started.elapsed().as_millis() as i64,
                    ..ExecutionResult::default()
                });
                return;
            }
            Err(_) => {}
        }
    }

    if thread_id.is_empty() {
        let mut params = serde_json::json!({
            "model": null_if_empty(&options.model),
            "modelProvider": null,
            "profile": null,
            "cwd": null_if_empty(&options.cwd),
            "approvalPolicy": null,
            "sandbox": null,
            "config": null,
            "baseInstructions": null,
            "developerInstructions": null,
            "compactPrompt": null,
            "includeApplyPatchTool": null,
            "experimentalRawEvents": false,
            "persistExtendedHistory": true
        });
        apply_reasoning(&mut params, &options.thinking_level);
        apply_service_tier(&mut params, &options.service_tier);
        match client
            .request(
                "thread/start",
                params,
                effective_request_timeout(execution_deadline, handshake_timeout),
                &options.cancellation,
            )
            .await
        {
            Ok(value) => thread_id = extract_thread_id(&value),
            Err(error) => {
                let stderr = cleanup_codex(
                    &mut tree,
                    &client,
                    reader_task,
                    stderr_task,
                    event_task,
                    &stderr_tail,
                )
                .await;
                let mut message = format!("codex thread/start failed: {error}");
                if !stderr.is_empty() {
                    message = with_stderr(&message, "codex", &sanitize_diagnostic(&stderr));
                }
                let _ = result_tx.send(ExecutionResult {
                    status: "failed".to_string(),
                    error: message,
                    duration_ms: started.elapsed().as_millis() as i64,
                    ..ExecutionResult::default()
                });
                return;
            }
        }
    }

    if thread_id.is_empty() {
        let stderr = cleanup_codex(
            &mut tree,
            &client,
            reader_task,
            stderr_task,
            event_task,
            &stderr_tail,
        )
        .await;
        let mut message = "codex thread/start returned no thread ID".to_string();
        if !stderr.is_empty() {
            message = with_stderr(&message, "codex", &sanitize_diagnostic(&stderr));
        }
        let _ = result_tx.send(ExecutionResult {
            status: "failed".to_string(),
            error: message,
            duration_ms: started.elapsed().as_millis() as i64,
            ..ExecutionResult::default()
        });
        return;
    }
    observer.set_thread(&thread_id).await;

    if !options.thread_name.trim().is_empty() {
        let _ = client
            .request(
                "thread/name/set",
                serde_json::json!({"threadId": thread_id, "name": options.thread_name.trim()}),
                effective_request_timeout(execution_deadline, handshake_timeout),
                &options.cancellation,
            )
            .await;
    }

    observer.arm().await;
    let input = if options.resume_expected && !resumed {
        format!("{}{}", options.resume_continuity_notice, prompt)
    } else {
        prompt
    };
    let mut turn_params = serde_json::json!({
        "threadId": thread_id,
        "input": [{"type": "text", "text": input}],
    });
    apply_reasoning(&mut turn_params, &options.thinking_level);
    apply_service_tier(&mut turn_params, &options.service_tier);
    if let Err(error) = client
        .request(
            "turn/start",
            turn_params,
            effective_request_timeout(execution_deadline, handshake_timeout),
            &options.cancellation,
        )
        .await
    {
        let stderr = cleanup_codex(
            &mut tree,
            &client,
            reader_task,
            stderr_task,
            event_task,
            &stderr_tail,
        )
        .await;
        let mut message = format!("codex turn/start failed: {error}");
        if !stderr.is_empty() {
            message = with_stderr(&message, "codex", &sanitize_diagnostic(&stderr));
        }
        let _ = result_tx.send(ExecutionResult {
            status: "failed".to_string(),
            error: message,
            session_id: thread_id,
            duration_ms: started.elapsed().as_millis() as i64,
            ..ExecutionResult::default()
        });
        return;
    }

    let mut semantic_deadline = tokio::time::Instant::now() + semantic_timeout;
    let first_timeout = if options.first_turn_no_progress_timeout > Duration::ZERO {
        options.first_turn_no_progress_timeout
    } else {
        DEFAULT_FIRST_TURN_TIMEOUT.min(semantic_timeout)
    };
    let mut first_deadline = None;
    let mut first_started = false;
    let mut first_progress = false;
    let mut status = "completed".to_string();
    let mut error = String::new();

    loop {
        tokio::select! {
            Some(activity) = activity_rx.recv() => {
                semantic_deadline = tokio::time::Instant::now() + semantic_timeout;
                if activity == "status:running" && !first_started {
                    first_started = true;
                    first_deadline = Some(tokio::time::Instant::now() + first_timeout);
                } else if first_started
                    && !first_progress
                    && activity != "status:running"
                    && activity != "error:retry"
                {
                    first_progress = true;
                    first_deadline = None;
                }
            }
            Some(aborted) = turn_done_rx.recv() => {
                if aborted {
                    status = "aborted".to_string();
                    error = "codex turn aborted".to_string();
                } else {
                    let turn_error = observer.snapshot().await.turn_error;
                    if !turn_error.is_empty() {
                        status = "failed".to_string();
                        error = turn_error;
                    }
                }
                break;
            }
            _ = async {
                if let Some(deadline) = execution_deadline {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                status = "timeout".to_string();
                error = format!(
                    "codex timed out after {}s",
                    options.timeout.as_secs_f64()
                );
                break;
            }
            _ = async {
                if let Some(deadline) = first_deadline {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                status = "timeout".to_string();
                error = format!(
                    "codex app-server no progress timeout after {}s",
                    first_timeout.as_secs_f64()
                );
                break;
            }
            _ = tokio::time::sleep_until(semantic_deadline) => {
                status = "timeout".to_string();
                error = format!(
                    "codex semantic inactivity timeout after {}s",
                    semantic_timeout.as_secs_f64()
                );
                break;
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                if let Some(process_error) = client.process_error().await {
                    status = "failed".to_string();
                    error = process_error;
                    break;
                }
            }
            _ = options.cancellation.cancelled() => {
                status = "aborted".to_string();
                error = "execution cancelled".to_string();
                break;
            }
        }
    }

    let snapshot = observer.snapshot().await;
    if status == "completed" && !snapshot.turn_error.is_empty() {
        status = "failed".to_string();
        error = snapshot.turn_error.clone();
    }
    if status == "failed" && error.is_empty() {
        error = snapshot.turn_error.clone();
    }
    let stderr = cleanup_codex(
        &mut tree,
        &client,
        reader_task,
        stderr_task,
        event_task,
        &stderr_tail,
    )
    .await;
    if status != "completed" && !stderr.is_empty() {
        error = with_stderr(&error, "codex", &sanitize_diagnostic(&stderr));
    }

    let output = if !snapshot.final_answer.is_empty() {
        snapshot.final_answer
    } else {
        snapshot.last_agent_message
    };
    let mut usage = BTreeMap::new();
    if snapshot.usage.input_tokens > 0
        || snapshot.usage.output_tokens > 0
        || snapshot.usage.cache_read_tokens > 0
        || snapshot.usage.cache_write_tokens > 0
    {
        usage.insert(
            if options.model.is_empty() {
                "unknown".to_string()
            } else {
                options.model
            },
            snapshot.usage,
        );
    }
    let _ = result_tx.send(ExecutionResult {
        status,
        output,
        error,
        duration_ms: started.elapsed().as_millis() as i64,
        session_id: if snapshot.thread_id.is_empty() {
            thread_id
        } else {
            snapshot.thread_id
        },
        usage,
        ..ExecutionResult::default()
    });
}

fn nonzero_duration(value: Duration, fallback: Duration) -> Duration {
    if value.is_zero() {
        fallback
    } else {
        value
    }
}

fn effective_request_timeout(
    deadline: Option<tokio::time::Instant>,
    handshake_timeout: Duration,
) -> Duration {
    let Some(deadline) = deadline else {
        return handshake_timeout;
    };
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    remaining.min(handshake_timeout)
}

fn null_if_empty(value: &str) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        Value::String(value.to_string())
    }
}

fn apply_reasoning(params: &mut Value, level: &str) {
    if level.is_empty() {
        return;
    }
    let Some(object) = params.as_object_mut() else {
        return;
    };
    if object.contains_key("input") {
        object.insert("effort".to_string(), Value::String(level.to_string()));
        return;
    }
    let config = object
        .entry("config".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(config) = config.as_object_mut() {
        config.insert(
            "model_reasoning_effort".to_string(),
            Value::String(level.to_string()),
        );
    }
}

fn apply_service_tier(params: &mut Value, tier: &str) {
    if !tier.is_empty() {
        if let Some(object) = params.as_object_mut() {
            object.insert("serviceTier".to_string(), Value::String(tier.to_string()));
        }
    }
}

fn extract_thread_id(value: &Value) -> String {
    value
        .get("thread")
        .and_then(Value::as_object)
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn is_transport_error(error: &AgentError) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("process exited")
        || text.contains("handshake timeout")
        || text.contains("stdout read error")
        || text.starts_with("codex thread/resume failed: write")
}

fn is_resume_overflow(error: &AgentError) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("thread/resume")
        && (text.contains("line exceeds") || text.contains("token too long"))
}

fn string_field(object: &Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn nested_string(value: &Value, keys: &[&str]) -> String {
    let mut current = value;
    for key in keys {
        let Some(next) = current.get(*key) else {
            return String::new();
        };
        current = next;
    }
    current.as_str().unwrap_or_default().to_string()
}

fn one_string_input(key: &str, value: &str) -> BTreeMap<String, Value> {
    BTreeMap::from([(key.to_string(), Value::String(value.to_string()))])
}

fn first_nonzero_i64(object: &Map<String, Value>, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|key| {
            object
                .get(*key)
                .and_then(Value::as_i64)
                .filter(|value| *value != 0)
        })
        .unwrap_or(0)
}

fn normalize_legacy_changes(value: Option<&Value>) -> Option<Vec<Value>> {
    let object = value?.as_object()?;
    let mut changes = Vec::with_capacity(object.len());
    for (path, value) in object {
        let Some(entry) = value.as_object() else {
            continue;
        };
        let mut normalized = Map::new();
        normalized.insert("path".to_string(), Value::String(path.clone()));
        if let Some(kind) = entry.get("type").and_then(Value::as_str) {
            normalized.insert("kind".to_string(), Value::String(kind.to_string()));
        }
        if let Some(diff) = entry.get("unified_diff").and_then(Value::as_str) {
            normalized.insert("diff".to_string(), Value::String(diff.to_string()));
        }
        if let Some(content) = entry.get("content").and_then(Value::as_str) {
            normalized.insert("content".to_string(), Value::String(content.to_string()));
        }
        if let Some(move_path) = entry.get("move_path").and_then(Value::as_str) {
            normalized.insert(
                "move_path".to_string(),
                Value::String(move_path.to_string()),
            );
        }
        if normalized.len() > 1 {
            changes.push(Value::Object(normalized));
        }
    }
    (!changes.is_empty()).then_some(changes)
}

fn normalize_raw_changes(value: Option<&Value>) -> Option<Vec<Value>> {
    let changes = value?.as_array()?;
    let mut normalized = Vec::with_capacity(changes.len());
    for change in changes {
        let Some(entry) = change.as_object() else {
            continue;
        };
        let mut item = Map::new();
        if let Some(path) = entry.get("path").and_then(Value::as_str) {
            item.insert("path".to_string(), Value::String(path.to_string()));
        }
        let (kind, move_path) = match entry.get("kind") {
            Some(Value::Object(kind)) => (
                kind.get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                kind.get("move_path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
            Some(Value::String(kind)) => (kind.clone(), String::new()),
            _ => (String::new(), String::new()),
        };
        if !kind.is_empty() {
            item.insert("kind".to_string(), Value::String(kind.clone()));
        }
        if !move_path.is_empty() {
            item.insert("move_path".to_string(), Value::String(move_path.clone()));
        }
        if let Some(body) = entry.get("diff").and_then(Value::as_str) {
            if kind == "add" || kind == "delete" {
                item.insert("content".to_string(), Value::String(body.to_string()));
            } else {
                item.insert(
                    "diff".to_string(),
                    Value::String(strip_moved_to_suffix(body, &move_path)),
                );
            }
        } else if let Some(content) = entry.get("content").and_then(Value::as_str) {
            item.insert("content".to_string(), Value::String(content.to_string()));
        }
        if !item.is_empty() {
            normalized.push(Value::Object(item));
        }
    }
    (!normalized.is_empty()).then_some(normalized)
}

fn strip_moved_to_suffix(value: &str, move_path: &str) -> String {
    if move_path.is_empty() {
        return value.to_string();
    }
    value
        .strip_suffix(&format!("\n\nMoved to: {move_path}"))
        .unwrap_or(value)
        .to_string()
}

fn patch_input(changes: Option<&Vec<Value>>) -> BTreeMap<String, Value> {
    let Some(changes) = changes else {
        return BTreeMap::new();
    };
    let original_bytes = patch_body_bytes(changes);
    let mut safe = Value::Array(changes.clone());
    redact_value(&mut safe, 0);
    let mut truncated = false;
    if let Some(values) = safe.as_array_mut() {
        if patch_body_bytes(values) > MAX_PATCH_BYTES {
            apply_patch_budget(values);
            truncated = true;
        }
    }
    let mut result = BTreeMap::from([("changes".to_string(), safe)]);
    if truncated {
        result.insert("truncated".to_string(), Value::Bool(true));
        result.insert(
            "original_bytes".to_string(),
            Value::Number(original_bytes.into()),
        );
    }
    result
}

fn patch_body_bytes(changes: &[Value]) -> usize {
    changes
        .iter()
        .filter_map(Value::as_object)
        .flat_map(|object| {
            ["diff", "content"]
                .into_iter()
                .filter_map(move |key| object.get(key))
        })
        .filter_map(Value::as_str)
        .map(str::len)
        .sum()
}

fn apply_patch_budget(changes: &mut [Value]) {
    let mut remaining = MAX_PATCH_BYTES;
    for change in changes {
        let Some(object) = change.as_object_mut() else {
            continue;
        };
        for key in ["diff", "content"] {
            let Some(body) = object.get(key).and_then(Value::as_str) else {
                continue;
            };
            if body.len() <= remaining {
                remaining -= body.len();
                continue;
            }
            let kept = truncate_utf8(body, remaining);
            if kept.is_empty() {
                object.remove(key);
            } else {
                object.insert(key.to_string(), Value::String(kept));
            }
            object.insert("truncated".to_string(), Value::Bool(true));
            remaining = 0;
        }
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    String::from_utf8_lossy(&value.as_bytes()[..max_bytes]).to_string()
}

fn redact_value(value: &mut Value, depth: usize) {
    if depth > 16 {
        return;
    }
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                let lower = key.to_ascii_lowercase();
                if lower.contains("token")
                    || lower.contains("secret")
                    || lower.contains("password")
                    || lower.contains("api_key")
                    || lower == "authorization"
                {
                    *value = Value::String("[REDACTED]".to_string());
                } else {
                    redact_value(value, depth + 1);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_value(value, depth + 1);
            }
        }
        Value::String(text) => {
            *text = sanitize_diagnostic(text);
        }
        _ => {}
    }
}

fn normalize_patch_status(status: &str) -> String {
    if status == "inProgress" {
        "in_progress".to_string()
    } else {
        status.to_string()
    }
}

fn patch_result_output(
    status: &str,
    changes: Option<&Vec<Value>>,
    stdout: String,
    stderr: String,
) -> String {
    let count = changes.map_or(0, Vec::len);
    let mut pieces = Vec::new();
    if !status.is_empty() || count > 0 {
        let files = if count == 1 {
            "1 file".to_string()
        } else {
            format!("{count} files")
        };
        pieces.push(if status.is_empty() {
            format!("{files} changed")
        } else if count == 0 {
            status.to_string()
        } else {
            format!("{status} ({files})")
        });
    }
    if !stdout.trim().is_empty() {
        pieces.push(stdout.trim().to_string());
    }
    if !stderr.trim().is_empty() {
        pieces.push(stderr.trim().to_string());
    }
    pieces.join("\n")
}

fn mcp_tool_input(item: &Map<String, Value>) -> BTreeMap<String, Value> {
    let mut input = BTreeMap::new();
    if let Some(server) = item.get("server").and_then(Value::as_str) {
        if !server.trim().is_empty() {
            input.insert("server".to_string(), Value::String(server.to_string()));
        }
    }
    if let Some(arguments) = item.get("arguments") {
        let mut arguments = arguments.clone();
        redact_value(&mut arguments, 0);
        input.insert("arguments".to_string(), arguments);
    }
    input
}

fn mcp_tool_result(item: &Map<String, Value>, status: &str) -> String {
    let status = if status.is_empty() {
        "completed"
    } else {
        status
    };
    let mut output = status.to_string();
    if let Some(duration) = item
        .get("durationMs")
        .or_else(|| item.get("duration_ms"))
        .and_then(Value::as_i64)
        .filter(|duration| *duration > 0)
    {
        output.push_str(&format!("\nduration: {duration} ms"));
    }
    let error = nested_string(&Value::Object(item.clone()), &["error", "message"]);
    if !error.trim().is_empty() {
        output.push_str("\nerror: ");
        output.push_str(&sanitize_diagnostic(&error));
    }
    output
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn launch_args_keep_owned_transport_and_filter_user_listen() {
        let options = ExecOptions {
            extra_args: vec![
                "--listen".to_string(),
                "tcp://unsafe".to_string(),
                "-c".to_string(),
                "model=o3".to_string(),
            ],
            custom_args: vec!["--config=mcp_servers.old.command=bad".to_string()],
            ..ExecOptions::default()
        };
        assert_eq!(
            build_codex_args(&options),
            vec![
                "app-server",
                "--listen",
                "stdio://",
                "-c",
                "model=o3",
                "--config=mcp_servers.old.command=bad"
            ]
        );

        let managed = ExecOptions {
            mcp_config: Some(serde_json::json!({"mcpServers": {}})),
            ..options
        };
        assert_eq!(
            build_codex_args(&managed),
            vec!["app-server", "--listen", "stdio://", "-c", "model=o3"]
        );
    }

    #[test]
    fn priority_tier_owns_fast_mode() {
        let options = ExecOptions {
            service_tier: "priority".to_string(),
            custom_args: vec![
                "--disable".to_string(),
                "fast_mode".to_string(),
                "-c".to_string(),
                "features.fast_mode=false".to_string(),
                "--disable=other_feature".to_string(),
            ],
            ..ExecOptions::default()
        };
        assert_eq!(
            build_codex_args(&options),
            vec![
                "app-server",
                "--listen",
                "stdio://",
                "--disable=other_feature",
                "--enable",
                "fast_mode"
            ]
        );
    }

    #[test]
    fn request_shapes_preserve_resume_continuity_and_overrides() {
        let mut start = serde_json::json!({
            "input": [{"type": "text", "text": "prompt"}],
            "config": null
        });
        apply_reasoning(&mut start, "high");
        apply_service_tier(&mut start, "priority");
        assert_eq!(start["effort"], "high");
        assert_eq!(start["serviceTier"], "priority");

        let input = format!("{}{}", "CONTEXT LOST\n", "prompt");
        assert_eq!(input, "CONTEXT LOST\nprompt");
    }

    #[test]
    fn raw_patch_changes_are_normalized_and_bounded() {
        let changes = normalize_raw_changes(Some(&serde_json::json!([
            {
                "path": "new.txt",
                "kind": {"type": "add"},
                "diff": "hello"
            },
            {
                "path": "old.txt",
                "kind": {"type": "update", "move_path": "moved.txt"},
                "diff": "diff\n\nMoved to: moved.txt"
            }
        ])))
        .unwrap_or_default();
        assert_eq!(changes[0]["content"], "hello");
        assert_eq!(changes[1]["diff"], "diff");
        assert_eq!(changes[1]["move_path"], "moved.txt");
    }

    #[tokio::test]
    async fn observer_isolates_threads_and_selects_final_answer() {
        let (messages_tx, mut messages_rx) = mpsc::channel(16);
        let (activity_tx, mut activity_rx) = mpsc::channel(16);
        let (done_tx, mut done_rx) = mpsc::channel(4);
        let observer = CodexObserver::new(messages_tx, activity_tx, done_tx);
        observer.set_thread("thread-main").await;
        observer.arm().await;

        observer
            .handle_notification(
                "item/completed",
                serde_json::json!({
                    "threadId": "thread-main",
                    "turnId": "turn-old",
                    "item": {
                        "type": "agentMessage",
                        "id": "old-item",
                        "phase": "final_answer",
                        "text": "replayed history"
                    }
                }),
            )
            .await;
        assert!(messages_rx.try_recv().is_err());

        observer
            .handle_notification(
                "turn/started",
                serde_json::json!({
                    "threadId": "thread-other",
                    "turn": {"id": "turn-other"}
                }),
            )
            .await;
        assert!(messages_rx.try_recv().is_err());

        observer
            .handle_notification(
                "turn/started",
                serde_json::json!({
                    "threadId": "thread-main",
                    "turn": {"id": "turn-main"}
                }),
            )
            .await;
        observer
            .handle_notification(
                "item/completed",
                serde_json::json!({
                    "threadId": "thread-main",
                    "item": {
                        "type": "agentMessage",
                        "id": "item-1",
                        "phase": "final_answer",
                        "text": "done"
                    }
                }),
            )
            .await;
        observer
            .handle_notification(
                "turn/completed",
                serde_json::json!({
                    "threadId": "thread-main",
                    "turn": {
                        "id": "turn-main",
                        "status": "completed",
                        "usage": {
                            "input_tokens": 10,
                            "cached_input_tokens": 4,
                            "output_tokens": 6
                        }
                    }
                }),
            )
            .await;

        assert_eq!(activity_rx.recv().await.as_deref(), Some("status:running"));
        let message = messages_rx.recv().await.unwrap_or_default();
        assert_eq!(message.message_type, MessageType::Status);
        let message = messages_rx.recv().await.unwrap_or_default();
        assert_eq!(message.content, "done");
        assert_eq!(done_rx.recv().await, Some(false));
        let snapshot = observer.snapshot().await;
        assert_eq!(snapshot.final_answer, "done");
        assert_eq!(snapshot.usage.input_tokens, 6);
        assert_eq!(snapshot.usage.cache_read_tokens, 4);
        assert_eq!(snapshot.usage.output_tokens, 6);
    }

    #[tokio::test]
    async fn managed_mcp_is_private_and_replaces_owned_tables() {
        let directory = tempdir().unwrap_or_else(|error| panic!("temp dir: {error}"));
        let home = directory.path().to_string_lossy().to_string();
        let path = Path::new(&home).join("config.toml");
        tokio::fs::write(
            &path,
            "[other]\nvalue = true\n\n[mcp_servers.old]\ncommand = \"bad\"\n",
        )
        .await
        .unwrap_or_else(|error| panic!("write config: {error}"));
        write_managed_codex_mcp(
            Some(&home),
            Some(&serde_json::json!({
                "mcpServers": {
                    "fetch": {
                        "command": "uvx",
                        "args": ["mcp-server-fetch"],
                        "env": {"TOKEN": "secret"}
                    }
                }
            })),
        )
        .await
        .unwrap_or_else(|error| panic!("write managed config: {error}"));
        let text = tokio::fs::read_to_string(path)
            .await
            .unwrap_or_else(|error| panic!("read config: {error}"));
        assert!(text.contains("[other]"));
        assert!(text.contains("[mcp_servers.fetch]"));
        assert!(!text.contains("[mcp_servers.old]"));
        assert!(text.contains("TOKEN = \"secret\""));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(Path::new(&home).join("config.toml"))
                .unwrap_or_else(|error| panic!("metadata: {error}"))
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[tokio::test]
    async fn empty_managed_mcp_set_is_valid_and_remote_headers_are_normalized() {
        let directory = tempdir().unwrap_or_else(|error| panic!("temp dir: {error}"));
        let home = directory.path().to_string_lossy().to_string();
        let path = Path::new(&home).join("config.toml");
        tokio::fs::write(
            &path,
            "# BEGIN cordy-managed mcp_servers (do not edit; regenerated by daemon)\n\
             [mcp_servers.old]\ncommand = \"bad\"\n\
             # END cordy-managed mcp_servers\n",
        )
        .await
        .unwrap_or_else(|error| panic!("write config: {error}"));
        write_managed_codex_mcp(Some(&home), Some(&serde_json::json!({})))
            .await
            .unwrap_or_else(|error| panic!("write empty managed config: {error}"));
        let text = tokio::fs::read_to_string(&path)
            .await
            .unwrap_or_else(|error| panic!("read config: {error}"));
        assert!(text.contains("# BEGIN cordy-managed mcp_servers"));
        assert!(text.contains("# END cordy-managed mcp_servers"));
        assert!(!text.contains("[mcp_servers.old]"));

        let mut rendered = String::new();
        render_codex_mcp_server(
            &mut rendered,
            "remote",
            &serde_json::json!({
                "type": "http",
                "url": "https://example.test/mcp",
                "headers": {"Authorization": "Bearer value"}
            }),
        )
        .unwrap_or_else(|error| panic!("render remote MCP: {error}"));
        assert!(rendered.contains("http_headers"));
        assert!(rendered.contains("experimental_use_rmcp_client = true"));
        assert!(!rendered.contains("type ="));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn backend_drives_fake_app_server_and_returns_final_answer() {
        let script = r#"
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{}}'
      ;;
    *'"method":"thread/start"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"thread-main"}}}'
      ;;
    *'"method":"turn/start"'*)
      printf '%s\n' '{"jsonrpc":"2.0","method":"turn/started","params":{"threadId":"thread-main","turn":{"id":"turn-main"}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"thread-main","item":{"type":"agentMessage","id":"item-main","phase":"final_answer","text":"fake answer"}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"thread-main","turn":{"id":"turn-main","status":"completed"}}}'
      printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{}}'
      ;;
  esac
done
"#;
        let backend = CodexBackend::new(CodexConfig {
            command: RuntimeCommand::new("sh", vec!["-c".to_string(), script.to_string()]),
            env: BTreeMap::new(),
        });
        let options = ExecOptions {
            handshake_timeout: Duration::from_secs(2),
            semantic_inactivity_timeout: Duration::from_secs(2),
            first_turn_no_progress_timeout: Duration::from_secs(1),
            ..ExecOptions::default()
        };
        let mut session = backend
            .execute("hello", options)
            .await
            .unwrap_or_else(|error| panic!("start fake Codex: {error}"));
        let mut messages = Vec::new();
        while let Some(message) = session.messages.recv().await {
            messages.push(message);
        }
        let result = session
            .result
            .await
            .unwrap_or_else(|error| panic!("fake Codex result: {error}"));
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "fake answer");
        assert_eq!(result.session_id, "thread-main");
        assert!(messages
            .iter()
            .any(|message| message.message_type == MessageType::Status));
        assert!(messages
            .iter()
            .any(|message| message.content == "fake answer"));
    }

    #[test]
    fn transport_error_classification_is_fail_closed() {
        assert!(is_transport_error(&AgentError::Protocol(
            "codex app-server handshake timeout: thread/resume".to_string()
        )));
        assert!(!is_transport_error(&AgentError::Protocol(
            "thread/resume: unknown thread (code=-32000)".to_string()
        )));
        assert_eq!(
            nonzero_duration(Duration::ZERO, Duration::from_secs(2)),
            Duration::from_secs(2)
        );
    }
}
