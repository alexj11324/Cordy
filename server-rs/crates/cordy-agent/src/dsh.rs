//! DeepSeek Harness (DSH) versioned JSONL stdio adapter.

use std::collections::BTreeMap;
use std::io;
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout, Command};
use tokio::task::JoinError;
use tokio_util::sync::CancellationToken;

use crate::command::RuntimeCommand;
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
use crate::mcp::managed_object;
use crate::model::{Catalog, CatalogCache, Model, ModelDiscoveryCacheKey, ModelThinking};
use crate::process::OwnedProcessTree;
use crate::stderr::{with_stderr, SharedDiagnosticBuffer, DEFAULT_TAIL_BYTES};
use crate::stream::AgentLineReader;

const DSH_PROFILE: &str = "cordy";
const DSH_PROTOCOL_VERSION: i64 = 1;
const DSH_CANCEL_GRACE: Duration = Duration::from_secs(3);
const DSH_TERMINATE_GRACE: Duration = Duration::from_secs(2);
const DSH_KILL_GRACE: Duration = Duration::from_secs(10);
const DSH_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const MESSAGE_BUFFER: usize = 256;

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Default)]
pub struct DshConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct DshBackend {
    config: DshConfig,
}

impl DshBackend {
    pub fn new(config: DshConfig) -> Self {
        Self { config }
    }

    /// Discovers the built-in DSH catalog with the legacy provider cache key.
    pub async fn discover_models(
        &self,
        cache: &CatalogCache,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        self.discover_models_for_runtime("dsh", cache, cancellation, timeout)
            .await
    }

    /// Discovers against a daemon runtime identity so custom DSH profiles do
    /// not share a catalog with another runtime using the same executable.
    pub async fn discover_models_for_runtime(
        &self,
        runtime_scope: &str,
        cache: &CatalogCache,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        let scope = if runtime_scope.trim().is_empty() {
            "dsh"
        } else {
            runtime_scope
        };
        let Some(key) = ModelDiscoveryCacheKey::new(scope, &self.config.command) else {
            return Catalog::default();
        };
        if let Some(catalog) = cache.get(&key) {
            return catalog;
        }
        let timeout = if timeout.is_zero() {
            DSH_DISCOVERY_TIMEOUT
        } else {
            timeout
        };
        let catalog = discover_once(&self.config, cancellation.clone(), timeout)
            .await
            .map_or_else(
                |_| Catalog::default(),
                |models| Catalog {
                    models,
                    fallback: false,
                },
            );
        if cancellation.is_cancelled() {
            return Catalog::default();
        }
        let _ = cache.insert(key, catalog.clone());
        catalog
    }
}

pub fn build_dsh_args() -> Vec<String> {
    ["--profile", DSH_PROFILE, "--stdio"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn build_dsh_args_for_command(command: &RuntimeCommand, tail: &str) -> Vec<String> {
    let mut args = if has_profile_selector(&command.prefix) {
        Vec::new()
    } else {
        ["--profile", DSH_PROFILE]
            .into_iter()
            .map(str::to_string)
            .collect()
    };
    args.push(tail.to_string());
    args
}

fn has_profile_selector(prefix: &[String]) -> bool {
    prefix.iter().any(|arg| arg.starts_with("--profile="))
        || prefix.windows(2).any(|window| window[0] == "--profile")
}

#[async_trait]
impl Backend for DshBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        let model = parse_dsh_model_id(&options.model)?;
        let mcp_servers = build_dsh_mcp_servers(options.mcp_config.as_ref())?;
        let command_path = if self.config.command.path.is_empty() {
            "dsh"
        } else {
            self.config.command.path.as_str()
        };
        let argv = self
            .config
            .command
            .argv(&build_dsh_args_for_command(&self.config.command, "--stdio"));
        let mut command = Command::new(command_path);
        command
            .args(&argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&self.config.env)
            .kill_on_drop(false);
        if !options.cwd.is_empty() {
            command.current_dir(&options.cwd);
        }
        let mut tree = OwnedProcessTree::spawn(&mut command)
            .await
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    AgentError::ExecutableNotFound(command_path.to_string())
                } else {
                    AgentError::Process(error)
                }
            })?;
        let mut stdin = tree.child_mut().stdin.take().ok_or_else(|| {
            AgentError::Protocol("DSH stdin pipe unavailable after spawn".to_string())
        })?;
        let stdout = tree.child_mut().stdout.take().ok_or_else(|| {
            AgentError::Protocol("DSH stdout pipe unavailable after spawn".to_string())
        })?;
        let stderr = tree.child_mut().stderr.take().ok_or_else(|| {
            AgentError::Protocol("DSH stderr pipe unavailable after spawn".to_string())
        })?;

        let request_id = next_request_id();
        let execute = DshExecuteCommand {
            v: DSH_PROTOCOL_VERSION,
            command_type: "execute".to_string(),
            request_id: request_id.clone(),
            cwd: options.cwd.clone(),
            prompt: prompt.to_string(),
            resume_session_id: options.resume_session_id.clone(),
            model,
            reasoning_effort: options.thinking_level.clone(),
            mcp_servers,
        };
        write_json_line(&mut stdin, &execute)
            .await
            .map_err(AgentError::Process)?;

        let (message_tx, message_rx) = tokio::sync::mpsc::channel(MESSAGE_BUFFER);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let cancellation = options.cancellation.clone();
        let timeout = options.timeout;
        let started = Instant::now();
        let stderr_tail = SharedDiagnosticBuffer::new(DEFAULT_TAIL_BYTES);
        let stderr_reader_tail = stderr_tail.clone();

        tokio::spawn(async move {
            let mut stderr_task = tokio::spawn(pump_stderr(stderr, stderr_reader_tail));
            let mut reader_task = tokio::spawn(read_frames(stdout, message_tx, request_id.clone()));
            let outcome = {
                let completion = async {
                    let exit = tree.wait().await;
                    let stream = (&mut reader_task).await;
                    (exit, stream)
                };
                tokio::pin!(completion);
                if timeout.is_zero() {
                    tokio::select! {
                        completed = &mut completion => DshCompletionOutcome::Completed(Box::new(completed)),
                        () = cancellation.cancelled() => DshCompletionOutcome::Cancelled,
                    }
                } else {
                    tokio::select! {
                        completed = &mut completion => DshCompletionOutcome::Completed(Box::new(completed)),
                        () = cancellation.cancelled() => DshCompletionOutcome::Cancelled,
                        () = tokio::time::sleep(timeout) => DshCompletionOutcome::DeadlineExceeded,
                    }
                }
            };

            let (run_end, exit, stream) = match outcome {
                DshCompletionOutcome::Completed(completed) => {
                    (RunEnd::Completed, completed.0, completed.1)
                }
                DshCompletionOutcome::Cancelled => {
                    let _ = write_json_line(
                        &mut stdin,
                        &DshCancelCommand {
                            v: DSH_PROTOCOL_VERSION,
                            command_type: "cancel".to_string(),
                            request_id: request_id.clone(),
                        },
                    )
                    .await;
                    let graceful = tokio::time::timeout(DSH_CANCEL_GRACE, async {
                        let exit = tree.wait().await;
                        let stream = (&mut reader_task).await;
                        (exit, stream)
                    })
                    .await;
                    match graceful {
                        Ok(completed) => (RunEnd::Cancelled, completed.0, completed.1),
                        Err(_) => {
                            let _ = tree.shutdown(DSH_TERMINATE_GRACE, DSH_KILL_GRACE).await;
                            let stream = (&mut reader_task).await;
                            (RunEnd::Cancelled, Ok(success_exit_status()), stream)
                        }
                    }
                }
                DshCompletionOutcome::DeadlineExceeded => {
                    let _ = tree.shutdown(DSH_TERMINATE_GRACE, DSH_KILL_GRACE).await;
                    let stream = (&mut reader_task).await;
                    (RunEnd::DeadlineExceeded, Ok(success_exit_status()), stream)
                }
            };

            if tokio::time::timeout(DSH_KILL_GRACE, &mut stderr_task)
                .await
                .is_err()
            {
                stderr_task.abort();
            }
            let stderr = stderr_tail.tail();
            let state = stream.unwrap_or_else(join_failure_state);
            let result = finalize_result(
                run_end,
                timeout,
                exit.as_ref().ok(),
                exit.as_ref().err(),
                state,
                &stderr,
                started.elapsed(),
            );
            let _ = result_tx.send(result);
        });

        Ok(Session {
            messages: message_rx,
            result: result_rx,
        })
    }
}

#[derive(Debug, Serialize)]
struct DshExecuteCommand {
    v: i64,
    #[serde(rename = "type")]
    command_type: String,
    request_id: String,
    cwd: String,
    prompt: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    resume_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<DshModelSelection>,
    #[serde(skip_serializing_if = "String::is_empty")]
    reasoning_effort: String,
    mcp_servers: Vec<DshMcpServer>,
}

#[derive(Debug, Serialize)]
struct DshCancelCommand {
    v: i64,
    #[serde(rename = "type")]
    command_type: String,
    request_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DshModelSelection {
    provider: String,
    id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    reasoning_effort: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DshMcpServer {
    name: String,
    transport: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    command: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    args: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    url: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    headers: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct ManagedDshMcpEntry {
    #[serde(default, rename = "type")]
    transport: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

fn build_dsh_mcp_servers(config: Option<&Value>) -> Result<Vec<DshMcpServer>, AgentError> {
    let Some(config) = managed_object(config).map_err(AgentError::InvalidConfig)? else {
        return Ok(Vec::new());
    };
    let Some(raw_servers) = config.get("mcpServers") else {
        return Ok(Vec::new());
    };
    let servers = raw_servers.as_object().ok_or_else(|| {
        AgentError::InvalidConfig("managed MCP `mcpServers` must be a JSON object".to_string())
    })?;
    let mut names: Vec<&String> = servers.keys().collect();
    names.sort();
    let mut output = Vec::with_capacity(names.len());
    for name in names {
        let entry = match serde_json::from_value::<ManagedDshMcpEntry>(servers[name].clone()) {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(server = %name, error = %error, "skipping invalid DSH MCP entry");
                continue;
            }
        };
        let command = entry.command.trim();
        if !command.is_empty() {
            output.push(DshMcpServer {
                name: name.clone(),
                transport: "stdio".to_string(),
                command: command.to_string(),
                args: entry.args,
                env: entry.env,
                url: String::new(),
                headers: BTreeMap::new(),
            });
            continue;
        }
        let url = entry.url.trim();
        if !url.is_empty() {
            if entry.transport.trim().eq_ignore_ascii_case("sse") {
                return Err(AgentError::InvalidConfig(format!(
                    "DSH MCP server {name:?} uses SSE, but the DSH runtime supports stdio and streamable HTTP only"
                )));
            }
            output.push(DshMcpServer {
                name: name.clone(),
                transport: "streamable-http".to_string(),
                command: String::new(),
                args: Vec::new(),
                env: BTreeMap::new(),
                url: url.to_string(),
                headers: entry.headers,
            });
            continue;
        }
        tracing::warn!(server = %name, "skipping DSH MCP entry without command or URL");
    }
    Ok(output)
}

fn parse_dsh_model_id(value: &str) -> Result<Option<DshModelSelection>, AgentError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let Some((provider, model)) = value.split_once('/') else {
        return Err(AgentError::InvalidConfig(
            "DSH model must use the provider/model ID advertised by DSH".to_string(),
        ));
    };
    let provider = decode_model_part(provider, "provider")?;
    let model = decode_model_part(model, "model ID")?;
    if provider.trim().is_empty() {
        return Err(AgentError::InvalidConfig(
            "DSH model provider is empty".to_string(),
        ));
    }
    if model.trim().is_empty() {
        return Err(AgentError::InvalidConfig(
            "DSH model ID is empty".to_string(),
        ));
    }
    Ok(Some(DshModelSelection {
        provider,
        id: model,
        reasoning_effort: String::new(),
    }))
}

fn decode_model_part(value: &str, label: &str) -> Result<String, AgentError> {
    let bytes = value.as_bytes();
    if bytes.iter().enumerate().any(|(index, byte)| {
        *byte == b'%'
            && (index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit())
    }) {
        return Err(AgentError::InvalidConfig(format!(
            "decode DSH model {label}: invalid percent escape"
        )));
    }
    percent_decode_str(value)
        .decode_utf8()
        .map(|value| value.into_owned())
        .map_err(|error| AgentError::InvalidConfig(format!("decode DSH model {label}: {error}")))
}

#[derive(Debug, Deserialize)]
struct DshFrame {
    #[serde(default)]
    v: i64,
    #[serde(rename = "type", default)]
    frame_type: String,
    #[serde(default)]
    runtime: String,
    #[serde(default)]
    request_id: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    call_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: String,
    #[serde(default)]
    output: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    cache_read_tokens: i64,
    #[serde(default)]
    cache_write_tokens: i64,
    #[serde(default)]
    status: String,
    #[serde(default)]
    resume_rejected: bool,
    #[serde(default)]
    error: Option<DshWireError>,
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    models: Vec<DshModelFrame>,
}

#[derive(Debug, Deserialize)]
struct DshWireError {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct DshModelFrame {
    #[serde(default)]
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    default: bool,
    #[serde(default)]
    thinking: Option<DshThinkingFrame>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct DshThinkingFrame {
    #[serde(default)]
    supported_levels: Vec<crate::model::ThinkingLevel>,
    #[serde(default)]
    default_level: String,
}

#[derive(Debug, Default)]
struct DshStreamState {
    ready: bool,
    session_id: String,
    protocol_error: String,
    stream_error: String,
    invalid_frames: usize,
    frame_count: usize,
    usage: BTreeMap<String, TokenUsage>,
    result: Option<DshFrameResult>,
}

#[derive(Debug, Default)]
struct DshFrameResult {
    status: String,
    output: String,
    error: String,
    session_id: String,
    resume_rejected: bool,
}

async fn read_frames(
    stdout: ChildStdout,
    messages: tokio::sync::mpsc::Sender<Message>,
    request_id: String,
) -> DshStreamState {
    let mut reader = AgentLineReader::new(BufReader::new(stdout));
    let mut state = DshStreamState::default();
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let frame = match serde_json::from_str::<DshFrame>(line) {
                    Ok(frame) => frame,
                    Err(_) => {
                        state.invalid_frames += 1;
                        continue;
                    }
                };
                state.frame_count += 1;
                handle_frame(frame, &request_id, &messages, &mut state);
            }
            Ok(None) => return state,
            Err(error) => {
                state.stream_error = format!("read dsh event stream: {error}");
                return state;
            }
        }
    }
}

fn handle_frame(
    frame: DshFrame,
    request_id: &str,
    messages: &tokio::sync::mpsc::Sender<Message>,
    state: &mut DshStreamState,
) {
    if frame.v != DSH_PROTOCOL_VERSION {
        state.protocol_error = format!("dsh returned unsupported protocol version {}", frame.v);
        return;
    }
    if !frame.request_id.is_empty() && frame.request_id != request_id {
        return;
    }
    match frame.frame_type.as_str() {
        "ready" => state.ready = frame.runtime == "dsh",
        "session" => {
            state.session_id = frame.session_id.clone();
            send_message(
                messages,
                Message {
                    message_type: MessageType::Status,
                    status: "running".to_string(),
                    session_id: frame.session_id,
                    ..empty_message(MessageType::Status)
                },
            );
        }
        "text" => {
            if !frame.content.is_empty() {
                send_message(
                    messages,
                    Message {
                        message_type: MessageType::Text,
                        content: frame.content,
                        ..empty_message(MessageType::Text)
                    },
                );
            }
        }
        "thinking" => {
            if !frame.content.is_empty() {
                send_message(
                    messages,
                    Message {
                        message_type: MessageType::Thinking,
                        content: frame.content,
                        ..empty_message(MessageType::Thinking)
                    },
                );
            }
        }
        "tool_call" => {
            send_message(
                messages,
                Message {
                    message_type: MessageType::ToolUse,
                    tool: frame.name,
                    call_id: frame.call_id,
                    input: parse_tool_input(&frame.arguments),
                    ..empty_message(MessageType::ToolUse)
                },
            );
        }
        "tool_result" => {
            send_message(
                messages,
                Message {
                    message_type: MessageType::ToolResult,
                    tool: frame.name,
                    call_id: frame.call_id,
                    output: frame.output,
                    ..empty_message(MessageType::ToolResult)
                },
            );
        }
        "usage" => {
            let key = if frame.provider.is_empty() {
                frame.model
            } else {
                format!("{}/{}", frame.provider, frame.model)
            };
            if !key.is_empty() {
                let usage = state.usage.entry(key).or_default();
                usage.input_tokens += frame.input_tokens;
                usage.output_tokens += frame.output_tokens;
                usage.cache_read_tokens += frame.cache_read_tokens;
                usage.cache_write_tokens += frame.cache_write_tokens;
            }
        }
        "protocol_error" => {
            state.protocol_error = format!("{}: {}", frame.code, frame.message)
                .trim()
                .to_string();
        }
        "result" => {
            let error = frame.error.map_or_else(String::new, |error| {
                format!("{}: {}", error.code, error.message)
                    .trim()
                    .to_string()
            });
            state.result = Some(DshFrameResult {
                status: frame.status,
                output: frame.output,
                error,
                session_id: if frame.session_id.is_empty() {
                    state.session_id.clone()
                } else {
                    frame.session_id
                },
                resume_rejected: frame.resume_rejected,
            });
        }
        _ => {}
    }
}

fn parse_tool_input(arguments: &str) -> BTreeMap<String, Value> {
    let Ok(Value::Object(object)) = serde_json::from_str(arguments) else {
        return BTreeMap::from([("raw".to_string(), Value::String(arguments.to_string()))]);
    };
    object.into_iter().collect()
}

fn send_message(sender: &tokio::sync::mpsc::Sender<Message>, message: Message) {
    let _ = sender.try_send(message);
}

fn empty_message(message_type: MessageType) -> Message {
    Message {
        message_type,
        content: String::new(),
        tool: String::new(),
        call_id: String::new(),
        input: BTreeMap::new(),
        output: String::new(),
        status: String::new(),
        level: String::new(),
        session_id: String::new(),
    }
}

async fn write_json_line<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await
}

async fn pump_stderr(mut stderr: ChildStderr, tail: SharedDiagnosticBuffer) {
    let mut buffer = [0_u8; 8192];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(bytes) => tail.push(&buffer[..bytes]),
        }
    }
}

async fn discover_once(
    config: &DshConfig,
    cancellation: CancellationToken,
    timeout: Duration,
) -> io::Result<Vec<Model>> {
    let command_path = if config.command.path.is_empty() {
        "dsh"
    } else {
        config.command.path.as_str()
    };
    let argv = config.command.argv(&build_dsh_args_for_command(
        &config.command,
        "--list-models",
    ));
    let mut command = Command::new(command_path);
    command
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .envs(&config.env)
        .kill_on_drop(false);
    let mut tree = OwnedProcessTree::spawn(&mut command).await?;
    let stdout = tree
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("DSH model stdout pipe unavailable"))?;
    let outcome = {
        let read = async {
            let mut reader = AgentLineReader::new(BufReader::new(stdout));
            let mut models = Vec::new();
            while let Some(line) = reader.next_line().await? {
                let Ok(frame) = serde_json::from_str::<DshFrame>(line.trim()) else {
                    continue;
                };
                if frame.v != DSH_PROTOCOL_VERSION || frame.frame_type != "models" {
                    continue;
                }
                models.extend(frame.models.into_iter().map(model_from_frame));
            }
            let status = tree.wait().await?;
            if !status.success() {
                return Err(io::Error::other(format!(
                    "DSH model discovery exited with {status}"
                )));
            }
            if models.is_empty() {
                return Err(io::Error::other("DSH returned an empty model catalog"));
            }
            Ok(models)
        };
        tokio::pin!(read);
        tokio::select! {
            result = &mut read => DiscoveryOutcome::Completed(result),
            () = cancellation.cancelled() => DiscoveryOutcome::Cancelled,
            () = tokio::time::sleep(timeout) => DiscoveryOutcome::TimedOut,
        }
    };
    match outcome {
        DiscoveryOutcome::Completed(result) => result,
        DiscoveryOutcome::Cancelled => {
            let _ = tree.shutdown(DSH_TERMINATE_GRACE, DSH_KILL_GRACE).await;
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "DSH model discovery cancelled",
            ))
        }
        DiscoveryOutcome::TimedOut => {
            let _ = tree.shutdown(DSH_TERMINATE_GRACE, DSH_KILL_GRACE).await;
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "DSH model discovery timed out",
            ))
        }
    }
}

enum DiscoveryOutcome<T> {
    Completed(io::Result<T>),
    Cancelled,
    TimedOut,
}

fn model_from_frame(frame: DshModelFrame) -> Model {
    Model {
        id: frame.id,
        label: frame.label,
        provider: frame.provider,
        default: frame.default,
        thinking: frame.thinking.and_then(|thinking| {
            (!thinking.supported_levels.is_empty()).then_some(ModelThinking {
                supported_levels: thinking.supported_levels,
                default_level: thinking.default_level,
            })
        }),
        ..Model::default()
    }
}

#[derive(Debug, Clone, Copy)]
enum RunEnd {
    Completed,
    Cancelled,
    DeadlineExceeded,
}

enum DshCompletionOutcome {
    Completed(Box<(io::Result<ExitStatus>, Result<DshStreamState, JoinError>)>),
    Cancelled,
    DeadlineExceeded,
}

fn finalize_result(
    run_end: RunEnd,
    timeout: Duration,
    exit: Option<&ExitStatus>,
    wait_error: Option<&io::Error>,
    state: DshStreamState,
    stderr: &str,
    elapsed: Duration,
) -> ExecutionResult {
    let mut result = if let Some(result) = state.result {
        ExecutionResult {
            status: result.status,
            output: result.output,
            error: result.error,
            session_id: result.session_id,
            usage: state.usage,
            resume_rejected: result.resume_rejected,
            duration_ms: elapsed.as_millis().try_into().unwrap_or(i64::MAX),
        }
    } else {
        let (status, error) = match run_end {
            RunEnd::DeadlineExceeded => (
                "timeout".to_string(),
                format!("dsh timed out after {}s", timeout.as_secs_f64()),
            ),
            RunEnd::Cancelled => (
                "cancelled".to_string(),
                "dsh execution cancelled".to_string(),
            ),
            RunEnd::Completed => {
                let error = if !state.stream_error.is_empty() {
                    state.stream_error.clone()
                } else if let Some(error) = wait_error {
                    format!("wait dsh process: {error}")
                } else if !state.protocol_error.is_empty() {
                    state.protocol_error.clone()
                } else if !state.ready {
                    "dsh exited before the runtime protocol became ready".to_string()
                } else if exit.is_some_and(|status| !status.success()) {
                    format!(
                        "dsh exited with error: {}",
                        exit.unwrap_or(&success_exit_status())
                    )
                } else {
                    "dsh exited without a terminal result".to_string()
                };
                ("failed".to_string(), error)
            }
        };
        ExecutionResult {
            status,
            error,
            session_id: state.session_id,
            usage: state.usage,
            duration_ms: elapsed.as_millis().try_into().unwrap_or(i64::MAX),
            ..ExecutionResult::default()
        }
    };
    if !result.error.is_empty() {
        result.error = with_stderr(&result.error, "dsh", stderr);
    }
    result
}

fn join_failure_state(error: JoinError) -> DshStreamState {
    DshStreamState {
        stream_error: format!("DSH stream task failed: {error}"),
        ..DshStreamState::default()
    }
}

fn next_request_id() -> String {
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("cordy-{}-{}", nanos, sequence)
}

#[cfg(unix)]
fn success_exit_status() -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    ExitStatus::from_raw(0)
}

#[cfg(windows)]
fn success_exit_status() -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    ExitStatus::from_raw(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake_backend(script: &str) -> (tempfile::TempDir, DshBackend) {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let executable = directory.path().join("dsh");
        std::fs::write(&executable, script)
            .unwrap_or_else(|error| panic!("write DSH fixture: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod DSH fixture: {error}"));
        let backend = DshBackend::new(DshConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            env: BTreeMap::new(),
        });
        (directory, backend)
    }

    #[test]
    fn model_ids_decode_provider_and_slash() {
        let model = parse_dsh_model_id("deepseek-official/deepseek-v4%2Fflash")
            .unwrap_or_else(|error| panic!("parse DSH model: {error}"))
            .unwrap_or_else(|| panic!("model expected"));
        assert_eq!(model.provider, "deepseek-official");
        assert_eq!(model.id, "deepseek-v4/flash");
        assert!(parse_dsh_model_id("deepseek-v4-flash").is_err());
        assert!(parse_dsh_model_id("deepseek-official/deepseek-v4%2").is_err());
    }

    #[test]
    fn dsh_profile_is_owned_by_the_fixed_prefix_when_already_present() {
        let command =
            RuntimeCommand::new("dsh", vec!["--profile".to_string(), "cordy".to_string()]);
        assert_eq!(
            build_dsh_args_for_command(&command, "--stdio"),
            vec!["--stdio".to_string()]
        );
        let command = RuntimeCommand::new("dsh", vec!["--wrapper".to_string()]);
        assert_eq!(
            build_dsh_args_for_command(&command, "--list-models"),
            vec![
                "--profile".to_string(),
                "cordy".to_string(),
                "--list-models".to_string()
            ]
        );
    }

    #[test]
    fn model_thinking_is_omitted_without_supported_levels() {
        let model = model_from_frame(DshModelFrame {
            id: "model".to_string(),
            label: "Model".to_string(),
            provider: String::new(),
            default: false,
            thinking: Some(DshThinkingFrame {
                supported_levels: Vec::new(),
                default_level: "medium".to_string(),
            }),
        });
        assert!(model.thinking.is_none());
    }

    #[test]
    fn result_status_is_preserved_when_empty() {
        let result = finalize_result(
            RunEnd::Completed,
            Duration::from_secs(5),
            None,
            None,
            DshStreamState {
                result: Some(DshFrameResult::default()),
                ..DshStreamState::default()
            },
            "",
            Duration::ZERO,
        );
        assert_eq!(result.status, "");
    }

    #[test]
    fn mcp_conversion_rejects_sse_and_preserves_sorted_servers() {
        let config = serde_json::json!({
            "mcpServers": {
                "remote": {"type":"streamable-http", "url":"https://mcp.example", "headers":{"Authorization":"Bearer test"}},
                "local": {"command":"node", "args":["server.js"], "env":{"TOKEN":"value"}}
            }
        });
        let servers = build_dsh_mcp_servers(Some(&config))
            .unwrap_or_else(|error| panic!("convert DSH MCP: {error}"));
        assert_eq!(servers[0].name, "local");
        assert_eq!(servers[0].transport, "stdio");
        assert_eq!(servers[1].transport, "streamable-http");
        assert!(build_dsh_mcp_servers(Some(&serde_json::json!({
            "mcpServers": {"sse": {"type":"sse", "url":"https://mcp.example"}}
        })))
        .is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_translates_frames_usage_and_result() {
        let (_directory, backend) = fake_backend(
            r##"#!/bin/sh
test "$1" = --profile && test "$2" = cordy && test "$3" = --stdio || exit 9
printf '%s\n' '{"v":1,"type":"ready","runtime":"dsh"}'
IFS= read -r command
case "$command" in *'"type":"execute"'*) ;; *) exit 8 ;; esac
printf '%s\n' '{"v":1,"type":"session","session_id":"session-1"}'
printf '%s\n' '{"v":1,"type":"thinking","content":"checking"}'
printf '%s\n' '{"v":1,"type":"tool_call","call_id":"call-1","name":"bash","arguments":"{\"command\":\"pwd\"}"}'
printf '%s\n' '{"v":1,"type":"tool_result","call_id":"call-1","name":"bash","output":"/work"}'
printf '%s\n' '{"v":1,"type":"text","content":"done"}'
printf '%s\n' '{"v":1,"type":"usage","provider":"deepseek-official","model":"deepseek-v4-flash","input_tokens":12,"output_tokens":3,"cache_read_tokens":2}'
printf '%s\n' '{"v":1,"type":"result","status":"completed","session_id":"session-1","output":"done"}'
"##,
        );
        let session = backend
            .execute(
                "say done",
                ExecOptions {
                    cwd: std::env::temp_dir().to_string_lossy().into_owned(),
                    timeout: Duration::from_secs(5),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute DSH: {error}"));
        let mut messages = Vec::new();
        let mut receiver = session.messages;
        while let Some(message) = receiver.recv().await {
            messages.push(message);
        }
        let result = session
            .result
            .await
            .unwrap_or_else(|error| panic!("DSH result: {error}"));
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "done");
        assert_eq!(result.session_id, "session-1");
        assert_eq!(
            result.usage["deepseek-official/deepseek-v4-flash"].input_tokens,
            12
        );
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0].message_type, MessageType::Status);
        assert_eq!(messages[2].tool, "bash");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_sends_protocol_cancel_before_terminal_result() {
        let (_directory, backend) = fake_backend(
            r##"#!/bin/sh
printf '%s\n' '{"v":1,"type":"ready","runtime":"dsh"}'
IFS= read -r execute
printf '%s\n' '{"v":1,"type":"session","session_id":"session-cancel"}'
IFS= read -r cancel
case "$cancel" in *'"type":"cancel"'*) ;; *) exit 8 ;; esac
printf '%s\n' '{"v":1,"type":"result","status":"cancelled","session_id":"session-cancel"}'
"##,
        );
        let cancellation = CancellationToken::new();
        let session = backend
            .execute(
                "wait",
                ExecOptions {
                    timeout: Duration::from_secs(5),
                    cancellation: cancellation.clone(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute DSH: {error}"));
        let Session {
            mut messages,
            result,
        } = session;
        let session_message = messages
            .recv()
            .await
            .unwrap_or_else(|| panic!("DSH session message"));
        assert_eq!(session_message.session_id, "session-cancel");
        cancellation.cancel();
        while messages.recv().await.is_some() {}
        let result = result
            .await
            .unwrap_or_else(|error| panic!("DSH cancellation result: {error}"));
        assert_eq!(result.status, "cancelled");
        assert_eq!(result.session_id, "session-cancel");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn discovery_reads_models_and_scopes_cache() {
        let (_directory, backend) = fake_backend(
            r##"#!/bin/sh
test "$1" = --profile && test "$2" = cordy && test "$3" = --list-models || exit 9
printf '%s\n' '{"v":1,"type":"models","models":[{"id":"deepseek-official/deepseek-v4-flash","label":"DeepSeek V4 Flash","provider":"DeepSeek","default":true}]}'
"##,
        );
        let cache = CatalogCache::default();
        let first = backend
            .discover_models_for_runtime(
                "dsh\\0workspace=w\\0runtime=r\\0profile=",
                &cache,
                CancellationToken::new(),
                Duration::from_secs(5),
            )
            .await;
        assert_eq!(first.models.len(), 1);
        assert!(first.models[0].default);
    }
}
