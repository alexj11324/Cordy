//! Qoder CLI's headless ACP runner for both global binary variants.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

use crate::acp::{
    ACP_NOTIFICATION_DRAIN_GRACE, ACP_NOTIFICATION_QUIET_TIME, AcpClient, AcpError, AcpNotification,
};
use crate::acp_mcp::{
    AcpMcpServer, build_acp_mcp_servers, filter_acp_mcp_servers, parse_acp_mcp_capabilities,
};
use crate::command::{BlockedArgMode, RuntimeCommand, filter_custom_args, filter_launch_prefix};
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
use crate::process::OwnedProcessTree;
use crate::stderr::{DEFAULT_TAIL_BYTES, SharedDiagnosticBuffer, with_stderr};

const MESSAGE_BUFFER: usize = 256;
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const KILL_GRACE: Duration = Duration::from_secs(10);

static BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> = LazyLock::new(|| {
    BTreeMap::from([
        ("--acp", BlockedArgMode::Standalone),
        ("acp", BlockedArgMode::Standalone),
        ("--yolo", BlockedArgMode::Standalone),
    ])
});
static TERMINAL_PROVIDER_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:(?:⚠️|❌|\[ERROR\]).*(?:BadRequestError|AuthenticationError|RateLimitError|HTTP \d{3}|Non-retryable|API call failed)|API call failed after \d+ retr(?:y|ies))")
        .unwrap_or_else(|error| panic!("invalid Qoder provider-error regex: {error}"))
});
static OUTPUT_PROVIDER_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)API call failed after \d+ retr(?:y|ies)")
        .unwrap_or_else(|error| panic!("invalid Qoder output-error regex: {error}"))
});

#[derive(Debug, Clone)]
pub struct QoderConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
    pub default_command: String,
}

impl Default for QoderConfig {
    fn default() -> Self {
        Self {
            command: RuntimeCommand::default(),
            env: BTreeMap::new(),
            default_command: "qodercli".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct QoderBackend {
    config: QoderConfig,
}

impl QoderBackend {
    pub fn new(config: QoderConfig) -> Self {
        Self { config }
    }
}

pub fn build_qoder_args(options: &ExecOptions) -> Vec<String> {
    let mut args = vec!["--yolo".to_string(), "--acp".to_string()];
    args.extend(filter_custom_args(&options.extra_args, &BLOCKED_ARGS).args);
    args.extend(filter_custom_args(&options.custom_args, &BLOCKED_ARGS).args);
    args
}

#[async_trait]
impl Backend for QoderBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        let mcp_servers = build_acp_mcp_servers(options.mcp_config.as_ref())?;
        let command_path = if self.config.command.path.is_empty() {
            self.config.default_command.as_str()
        } else {
            self.config.command.path.as_str()
        };
        let prefix = filter_launch_prefix(&self.config.command.prefix, &BLOCKED_ARGS);
        log_blocked("launch prefix", &prefix.blocked_flags);
        log_blocked_args(&options);
        let mut argv = prefix.args;
        argv.extend(build_qoder_args(&options));
        let mut command = Command::new(command_path);
        command
            .args(argv)
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
        let stdin = tree.child_mut().stdin.take().ok_or_else(|| {
            AgentError::Protocol("Qoder stdin pipe unavailable after spawn".to_string())
        })?;
        let stdout = tree.child_mut().stdout.take().ok_or_else(|| {
            AgentError::Protocol("Qoder stdout pipe unavailable after spawn".to_string())
        })?;
        let stderr = tree.child_mut().stderr.take().ok_or_else(|| {
            AgentError::Protocol("Qoder stderr pipe unavailable after spawn".to_string())
        })?;

        let (message_tx, message_rx) = mpsc::channel(MESSAGE_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();
        let timeout = options.timeout;
        let cancellation = options.cancellation.clone();
        let started = Instant::now();
        let stderr_tail = SharedDiagnosticBuffer::new(DEFAULT_TAIL_BYTES);
        let prompt = prompt.to_string();

        tokio::spawn(async move {
            let stderr_reader = stderr_tail.clone();
            let mut stderr_task = tokio::spawn(async move {
                let mut stderr = stderr;
                let mut buffer = [0_u8; 8192];
                while let Ok(bytes) = stderr.read(&mut buffer).await {
                    if bytes == 0 {
                        break;
                    }
                    stderr_reader.push(&buffer[..bytes]);
                }
            });
            let mut protocol_task = tokio::spawn(run_protocol(
                stdin,
                stdout,
                prompt,
                options,
                mcp_servers,
                message_tx,
            ));
            let end = if timeout.is_zero() {
                tokio::select! {
                    result = &mut protocol_task => RunEnd::Protocol(result),
                    () = cancellation.cancelled() => RunEnd::Cancelled,
                }
            } else {
                tokio::select! {
                    result = &mut protocol_task => RunEnd::Protocol(result),
                    () = cancellation.cancelled() => RunEnd::Cancelled,
                    () = tokio::time::sleep(timeout) => RunEnd::TimedOut,
                }
            };
            let mut outcome = match end {
                RunEnd::Protocol(result) => result.unwrap_or_else(|error| {
                    ProtocolOutcome::failed(format!("Qoder protocol task failed: {error}"))
                }),
                RunEnd::Cancelled => ProtocolOutcome::terminal("aborted", "execution cancelled"),
                RunEnd::TimedOut => ProtocolOutcome::terminal(
                    "timeout",
                    format!("qoder timed out after {}s", timeout.as_secs_f64()),
                ),
            };
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            if !protocol_task.is_finished() {
                protocol_task.abort();
            }
            if tokio::time::timeout(KILL_GRACE, &mut stderr_task)
                .await
                .is_err()
            {
                stderr_task.abort();
            }
            let stderr = stderr_tail.tail();
            if outcome.status == "completed" {
                if let Some(provider_error) = provider_error(&stderr, &outcome.full_output) {
                    outcome.status = "failed".to_string();
                    outcome.error = provider_error;
                }
            }
            if !outcome.error.is_empty() {
                outcome.error = with_stderr(&outcome.error, "qoder", &stderr);
            }
            let _ = result_tx.send(ExecutionResult {
                status: outcome.status,
                output: outcome.output,
                error: outcome.error,
                duration_ms: started.elapsed().as_millis().try_into().unwrap_or(i64::MAX),
                session_id: outcome.session_id,
                usage: outcome.usage,
                resume_rejected: outcome.resume_rejected,
            });
        });

        Ok(Session {
            messages: message_rx,
            result: result_rx,
        })
    }
}

enum RunEnd {
    Protocol(Result<ProtocolOutcome, tokio::task::JoinError>),
    Cancelled,
    TimedOut,
}

#[derive(Default)]
struct ProtocolOutcome {
    status: String,
    output: String,
    full_output: String,
    error: String,
    session_id: String,
    usage: BTreeMap<String, TokenUsage>,
    resume_rejected: bool,
}

impl ProtocolOutcome {
    fn failed(error: impl Into<String>) -> Self {
        Self::terminal("failed", error)
    }

    fn terminal(status: &str, error: impl Into<String>) -> Self {
        Self {
            status: status.to_string(),
            error: error.into(),
            ..Self::default()
        }
    }
}

async fn run_protocol(
    stdin: ChildStdin,
    stdout: ChildStdout,
    prompt: String,
    options: ExecOptions,
    mcp_servers: Vec<AcpMcpServer>,
    messages: mpsc::Sender<Message>,
) -> ProtocolOutcome {
    let mut client = AcpClient::new(BufReader::new(stdout), stdin);
    let initialize = match client
        .request(
            "initialize",
            serde_json::json!({"protocolVersion":1,"clientInfo":{"name":"cordy-agent-sdk","version":"0.2.0"},"clientCapabilities":{}}),
            |_| {},
        )
        .await
    {
        Ok(result) => result,
        Err(error) => return ProtocolOutcome::failed(format!("qoder initialize failed: {error}")),
    };
    let mcp_servers =
        filter_acp_mcp_servers(mcp_servers, parse_acp_mcp_capabilities(&initialize), false);
    let cwd = if options.cwd.is_empty() {
        "."
    } else {
        &options.cwd
    };
    let (session_result, mut session_id) = if options.resume_session_id.is_empty() {
        match client
            .request(
                "session/new",
                serde_json::json!({"cwd":cwd,"mcpServers":mcp_servers}),
                |_| {},
            )
            .await
        {
            Ok(result) => {
                let session_id = extract_session_id(&result);
                if session_id.is_empty() {
                    return ProtocolOutcome::failed("qoder session/new returned no session ID");
                }
                (result, session_id)
            }
            Err(error) => {
                return ProtocolOutcome::failed(format!("qoder session/new failed: {error}"));
            }
        }
    } else {
        match client
            .request(
                "session/resume",
                serde_json::json!({"cwd":cwd,"sessionId":options.resume_session_id,"mcpServers":mcp_servers}),
                |_| {},
            )
            .await
        {
            Ok(result) => {
                let returned = extract_session_id(&result);
                let session_id = if returned.is_empty() { options.resume_session_id.clone() } else { returned };
                (result, session_id)
            }
            Err(error) => return protocol_failure("session/resume", error, String::new(), false),
        }
    };
    let mut effective_model = if options.model.is_empty() {
        extract_current_model(&session_result)
    } else {
        options.model.clone()
    };
    if !options.model.is_empty() {
        if let Err(error) = client
            .request(
                "session/set_model",
                serde_json::json!({"sessionId":session_id,"modelId":options.model}),
                |_| {},
            )
            .await
        {
            let rejected = !options.resume_session_id.is_empty() && error.is_session_not_found();
            if rejected {
                session_id.clear();
            }
            return protocol_failure("set_model", error, session_id, rejected);
        }
    }
    if effective_model.is_empty() {
        effective_model = "unknown".to_string();
    }
    let user_text = if options.system_prompt.is_empty() {
        prompt
    } else {
        format!("{}\n\n---\n\n{}", options.system_prompt, prompt)
    };
    let mut state = NotificationState::default();
    let prompt_result = client
        .request(
            "session/prompt",
            serde_json::json!({"sessionId":session_id,"prompt":[{"type":"text","text":user_text}]}),
            |notification| handle_notification(notification, &messages, &mut state),
        )
        .await;
    let prompt_result = match prompt_result {
        Ok(result) => result,
        Err(error) => {
            let rejected = !options.resume_session_id.is_empty() && error.is_session_not_found();
            if rejected {
                session_id.clear();
            }
            return protocol_failure("session/prompt", error, session_id, rejected);
        }
    };
    // A few ACP runtimes flush their final session/update notifications after
    // resolving session/prompt. Keep reading briefly so trailing answer text,
    // tool completions, and usage updates are not lost when the process is
    // shut down immediately after the response.
    let _ = client
        .drain_notifications(
            ACP_NOTIFICATION_QUIET_TIME,
            ACP_NOTIFICATION_DRAIN_GRACE,
            |notification| handle_notification(notification, &messages, &mut state),
        )
        .await;
    let stop_reason = prompt_result
        .get("stopReason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut usage = BTreeMap::new();
    let turn_usage = merge_usage(state.usage, parse_usage(prompt_result.get("usage")));
    if turn_usage != TokenUsage::default() {
        usage.insert(effective_model, turn_usage);
    }
    let (output, full_output) = state.deliverable.finish();
    ProtocolOutcome {
        status: if stop_reason == "cancelled" {
            "aborted"
        } else {
            "completed"
        }
        .to_string(),
        error: if stop_reason == "cancelled" {
            "qoder cancelled the prompt".to_string()
        } else {
            String::new()
        },
        output,
        full_output,
        session_id,
        usage,
        resume_rejected: false,
    }
}

fn protocol_failure(
    stage: &str,
    error: AcpError,
    session_id: String,
    rejected: bool,
) -> ProtocolOutcome {
    ProtocolOutcome {
        status: "failed".to_string(),
        error: format!("qoder {stage} failed: {error}"),
        session_id,
        resume_rejected: rejected,
        ..ProtocolOutcome::default()
    }
}

#[derive(Default)]
struct NotificationState {
    deliverable: Deliverable,
    tools: HashMap<String, PendingTool>,
    usage: TokenUsage,
}

#[derive(Default)]
struct PendingTool {
    name: String,
    input: BTreeMap<String, Value>,
    emitted: bool,
}

#[derive(Default)]
struct Deliverable {
    full: String,
    current: String,
    previous: String,
}

impl Deliverable {
    fn text(&mut self, text: &str) {
        self.full.push_str(text);
        self.current.push_str(text);
    }
    fn tool_boundary(&mut self) {
        if !self.current.trim().is_empty() {
            self.previous.clone_from(&self.current);
        }
        self.current.clear();
    }
    fn finish(self) -> (String, String) {
        let output = if self.current.trim().is_empty() {
            self.previous
        } else {
            self.current
        };
        (output, self.full)
    }
}

fn handle_notification(
    notification: AcpNotification,
    messages: &mpsc::Sender<Message>,
    state: &mut NotificationState,
) {
    if !matches!(
        notification.method.as_str(),
        "session/update" | "session/notification"
    ) {
        return;
    }
    let Some(update) = notification.params.get("update") else {
        return;
    };
    let (kind, data) = normalize_update(update);
    match kind.as_str() {
        "agentmessagechunk" => {
            if let Some(text) = content_text(data) {
                state.deliverable.text(&text);
                send(messages, message(MessageType::Text, &text));
            }
        }
        "agentthoughtchunk" => {
            if let Some(text) = content_text(data) {
                send(messages, message(MessageType::Thinking, &text));
            }
        }
        "toolcall" => handle_tool_start(data, messages, state),
        "toolcallupdate" => handle_tool_update(data, messages, state),
        "usageupdate" | "turnend" => {
            let update = parse_usage(data.get("usage").or(Some(data)));
            state.usage = merge_usage(state.usage, update);
        }
        _ => {}
    }
}

fn normalize_update(update: &Value) -> (String, &Value) {
    if let Some(kind) = update
        .get("sessionUpdate")
        .or_else(|| update.get("type"))
        .and_then(Value::as_str)
    {
        return (normalize_kind(kind), update);
    }
    if let Some(object) = update.as_object().filter(|object| object.len() == 1) {
        if let Some((kind, data)) = object.iter().next() {
            return (normalize_kind(kind), data);
        }
    }
    (String::new(), update)
}

fn normalize_kind(kind: &str) -> String {
    kind.chars()
        .filter(|c| !matches!(c, '_' | '-'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn content_text(data: &Value) -> Option<String> {
    let content = data.get("content")?;
    let rendered = render_content(content)?;
    (!rendered.is_empty()).then_some(rendered)
}

fn render_content(content: &Value) -> Option<String> {
    if let Some(text) = content.get("text").and_then(Value::as_str) {
        return (!text.is_empty()).then(|| text.to_string());
    }
    if let Some(blocks) = content.as_array() {
        let pieces: Vec<String> = blocks.iter().filter_map(render_content_block).collect();
        return (!pieces.is_empty()).then(|| pieces.join("\n"));
    }
    render_content_block(content)
}

fn render_content_block(block: &Value) -> Option<String> {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => block
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        Some("content") => block.get("content").and_then(render_content),
        Some("diff") => render_diff(block),
        _ => None,
    }
}

fn render_diff(block: &Value) -> Option<String> {
    let path = block
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())?;
    let old_text = block
        .get("oldText")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let new_text = block
        .get("newText")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if old_text.is_empty() {
        Some(format!(
            "--- {path}\n+++ {path}\n(new file, {} bytes)",
            new_text.len()
        ))
    } else {
        Some(format!(
            "--- {path}\n+++ {path}\n(edited: {} → {} bytes)",
            old_text.len(),
            new_text.len()
        ))
    }
}

fn handle_tool_start(
    data: &Value,
    messages: &mpsc::Sender<Message>,
    state: &mut NotificationState,
) {
    let id = data
        .get("toolCallId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if id.is_empty() {
        return;
    }
    let name = tool_name(data);
    let input = tool_input(data);
    state.deliverable.tool_boundary();
    let emitted = !input.is_empty();
    if emitted {
        send(messages, tool_use(&id, &name, input.clone()));
    }
    state.tools.insert(
        id,
        PendingTool {
            name,
            input,
            emitted,
        },
    );
}

fn handle_tool_update(
    data: &Value,
    messages: &mpsc::Sender<Message>,
    state: &mut NotificationState,
) {
    let status = data
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(status, "completed" | "failed") {
        return;
    }
    let id = data
        .get("toolCallId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if id.is_empty() {
        return;
    }
    let completion_input = tool_input_option(data);
    let pending = match state.tools.remove(id) {
        Some(pending) => pending,
        None => {
            state.deliverable.tool_boundary();
            PendingTool {
                name: tool_name(data),
                input: completion_input.clone().unwrap_or_default(),
                emitted: false,
            }
        }
    };
    if !pending.emitted {
        let input = completion_input.unwrap_or(pending.input);
        send(messages, tool_use(id, &pending.name, input));
    }
    let output = ["rawOutput", "output"]
        .iter()
        .find_map(|key| data.get(*key))
        .map(render_value)
        .or_else(|| content_text(data))
        .unwrap_or_default();
    let mut result = message(MessageType::ToolResult, "");
    result.call_id = id.to_string();
    result.output = output;
    result.status = status.to_string();
    send(messages, result);
}

fn tool_name(data: &Value) -> String {
    normalize_tool_name(
        data.get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty())
            .or_else(|| data.get("name").and_then(Value::as_str))
            .unwrap_or("tool"),
    )
}

fn tool_input(data: &Value) -> BTreeMap<String, Value> {
    tool_input_option(data).unwrap_or_default()
}

fn tool_input_option(data: &Value) -> Option<BTreeMap<String, Value>> {
    ["rawInput", "input", "parameters"]
        .iter()
        .find_map(|key| data.get(*key).and_then(Value::as_object))
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
}

fn normalize_tool_name(raw: &str) -> String {
    let title = raw.trim();
    if title.is_empty() {
        return "tool".to_string();
    }
    let title = title.split_once(':').map_or(title, |(name, _)| name.trim());
    if title.is_empty() {
        return "tool".to_string();
    }
    let lower = title.to_ascii_lowercase();
    match lower.as_str() {
        "read" | "read file" => "read_file".to_string(),
        "write" | "write file" => "write_file".to_string(),
        "edit" | "patch" => "edit_file".to_string(),
        "shell" | "bash" | "terminal" | "run command" | "run shell command" => {
            "terminal".to_string()
        }
        "search" | "grep" | "find" => "search_files".to_string(),
        "glob" => "glob".to_string(),
        "web search" => "web_search".to_string(),
        "fetch" | "web fetch" => "web_fetch".to_string(),
        "todo" | "todo write" => "todo_write".to_string(),
        _ => lower.replace([' ', '-'], "_"),
    }
}

fn render_value(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

fn parse_usage(value: Option<&Value>) -> TokenUsage {
    let value = value.unwrap_or(&Value::Null);
    let input_tokens = integer(value, &["inputTokens", "input_tokens"]);
    let output_tokens = integer(value, &["outputTokens", "output_tokens"]);
    let cache_read_tokens = integer(
        value,
        &[
            "cachedReadTokens",
            "cacheReadTokens",
            "cachedInputTokens",
            "cached_input_tokens",
            "cache_read_tokens",
            "cache_read_input_tokens",
        ],
    );
    let total_tokens = integer(value, &["totalTokens", "total_tokens"]);
    // Some ACP agents include cached input in inputTokens while also exposing
    // it as cachedReadTokens. Only subtract it when totalTokens proves that
    // inclusive shape; exclusive-bucket agents must remain unchanged.
    let input_tokens = if total_tokens > 0
        && cache_read_tokens > 0
        && cache_read_tokens <= input_tokens
        && total_tokens == input_tokens.saturating_add(output_tokens)
    {
        input_tokens - cache_read_tokens
    } else {
        input_tokens
    };
    TokenUsage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens: integer(
            value,
            &[
                "cachedWriteTokens",
                "cacheWriteTokens",
                "cache_write_tokens",
            ],
        ),
        cost_usd_ticks: integer(value, &["costUsdTicks", "cost_usd_ticks"]),
    }
}

fn merge_usage(current: TokenUsage, next: TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: current.input_tokens.max(next.input_tokens),
        output_tokens: current.output_tokens.max(next.output_tokens),
        cache_read_tokens: current.cache_read_tokens.max(next.cache_read_tokens),
        cache_write_tokens: current.cache_write_tokens.max(next.cache_write_tokens),
        cost_usd_ticks: current.cost_usd_ticks.max(next.cost_usd_ticks),
    }
}

fn integer(value: &Value, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_i64))
        .unwrap_or(0)
        .max(0)
}

fn extract_session_id(value: &Value) -> String {
    value
        .get("sessionId")
        .or_else(|| value.get("session_id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn extract_current_model(value: &Value) -> String {
    value
        .get("models")
        .and_then(|models| {
            models
                .get("currentModelId")
                .or_else(|| models.get("current_model_id"))
        })
        .or_else(|| value.get("currentModelId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn provider_error(stderr: &str, output: &str) -> Option<String> {
    if let Some(found) = TERMINAL_PROVIDER_ERROR.find(stderr) {
        return Some(format!("qoder provider error: {}", found.as_str()));
    }
    OUTPUT_PROVIDER_ERROR
        .find(output)
        .map(|found| format!("qoder provider error: {}", found.as_str()))
}

fn message(message_type: MessageType, content: &str) -> Message {
    Message {
        message_type,
        content: content.to_string(),
        tool: String::new(),
        call_id: String::new(),
        input: BTreeMap::new(),
        output: String::new(),
        status: String::new(),
        level: String::new(),
        session_id: String::new(),
    }
}

fn tool_use(id: &str, name: &str, input: BTreeMap<String, Value>) -> Message {
    let mut value = message(MessageType::ToolUse, "");
    value.call_id = id.to_string();
    value.tool = name.to_string();
    value.input = input;
    value
}

fn send(messages: &mpsc::Sender<Message>, value: Message) {
    let _ = messages.try_send(value);
}

fn log_blocked_args(options: &ExecOptions) {
    log_blocked(
        "extra arguments",
        &filter_custom_args(&options.extra_args, &BLOCKED_ARGS).blocked_flags,
    );
    log_blocked(
        "custom arguments",
        &filter_custom_args(&options.custom_args, &BLOCKED_ARGS).blocked_flags,
    );
}

fn log_blocked(source: &str, flags: &[String]) {
    if !flags.is_empty() {
        tracing::warn!(provider = "qoder", source, flags = ?flags, "ignored daemon-owned arguments");
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn arguments_keep_protocol_and_permission_mode_owned() {
        let args = build_qoder_args(&ExecOptions {
            extra_args: vec!["--acp".into(), "--safe".into()],
            custom_args: vec!["acp".into(), "--yolo".into(), "--debug".into()],
            ..ExecOptions::default()
        });
        assert_eq!(args.iter().filter(|arg| arg.as_str() == "--acp").count(), 1);
        assert_eq!(
            args.iter().filter(|arg| arg.as_str() == "--yolo").count(),
            1
        );
        assert!(args.iter().any(|arg| arg == "--safe"));
        assert!(args.iter().any(|arg| arg == "--debug"));
        assert!(!args.iter().any(|arg| arg == "acp"));
    }

    #[test]
    fn deliverable_uses_text_after_latest_tool_boundary() {
        let mut deliverable = Deliverable::default();
        deliverable.text("I will inspect.");
        deliverable.tool_boundary();
        deliverable.text("Final answer.");
        let (output, full) = deliverable.finish();
        assert_eq!(output, "Final answer.");
        assert_eq!(full, "I will inspect.Final answer.");
    }

    #[test]
    fn qoder_tool_names_are_stable_for_human_titles() {
        for (title, expected) in [
            ("Read file", "read_file"),
            ("Read file: /tmp/a.txt", "read_file"),
            ("Run command", "terminal"),
            ("Run command: cargo test", "terminal"),
            ("Search", "search_files"),
            ("custom tool", "custom_tool"),
        ] {
            assert_eq!(normalize_tool_name(title), expected, "title {title:?}");
        }
    }

    #[test]
    fn tool_completion_prefers_input_and_renders_content_blocks() {
        let (sender, mut receiver) = mpsc::channel(8);
        let mut state = NotificationState::default();
        handle_notification(
            AcpNotification {
                method: "session/update".to_string(),
                params: serde_json::json!({
                    "update": {
                        "type": "ToolCall",
                        "toolCallId": "tool-1",
                        "title": "Read file",
                    }
                }),
            },
            &sender,
            &mut state,
        );
        handle_notification(
            AcpNotification {
                method: "session/update".to_string(),
                params: serde_json::json!({
                    "update": {
                        "type": "ToolCallUpdate",
                        "toolCallId": "tool-1",
                        "status": "completed",
                        "rawInput": {"path": "README.md"},
                        "content": [
                            {"type": "content", "content": {"type": "text", "text": "read output"}},
                            {"type": "diff", "path": "src/main.rs", "oldText": "old", "newText": "new"}
                        ]
                    }
                }),
            },
            &sender,
            &mut state,
        );

        let tool = receiver.try_recv().expect("deferred tool use");
        assert_eq!(tool.message_type, MessageType::ToolUse);
        assert_eq!(tool.tool, "read_file");
        assert_eq!(
            tool.input.get("path").and_then(Value::as_str),
            Some("README.md")
        );
        let result = receiver.try_recv().expect("tool result");
        assert_eq!(result.message_type, MessageType::ToolResult);
        assert!(result.output.contains("read output"));
        assert!(result.output.contains("--- src/main.rs"));
        assert!(result.output.contains("(edited: 3 → 3 bytes)"));
    }

    #[test]
    fn inclusive_cached_input_is_normalized_only_with_total_tokens_proof() {
        let inclusive = serde_json::json!({
            "inputTokens": 100,
            "outputTokens": 20,
            "cachedReadTokens": 30,
            "totalTokens": 120
        });
        let usage = parse_usage(Some(&inclusive));
        assert_eq!(usage.input_tokens, 70);
        assert_eq!(usage.cache_read_tokens, 30);

        let exclusive = serde_json::json!({
            "inputTokens": 100,
            "outputTokens": 20,
            "cachedReadTokens": 30,
            "totalTokens": 150
        });
        assert_eq!(parse_usage(Some(&exclusive)).input_tokens, 100);
    }

    #[cfg(unix)]
    fn fake_backend(script: &str) -> (tempfile::TempDir, QoderBackend) {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let executable = directory.path().join("qodercli");
        std::fs::write(&executable, script)
            .unwrap_or_else(|error| panic!("write fake Qoder: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake Qoder: {error}"));
        let backend = QoderBackend::new(QoderConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            ..QoderConfig::default()
        });
        (directory, backend)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_acp_process_streams_tools_usage_and_permission() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"mcpCapabilities":{"http":true}}}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"qoder-real","models":{"currentModelId":"qoder-auto"}}}\n' "$id" ;;
    *'"method":"session/prompt"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":99,"method":"session/request_permission","params":{"options":[{"optionId":"forever","kind":"allow_always"},{"optionId":"once","kind":"allow_once"}]}}'
      IFS= read -r permission
      case "$permission" in *'"optionId":"once"'*) ;; *) exit 12 ;; esac
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk","content":{"text":"narration"}}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"ToolCall","toolCallId":"tool-1","name":"read_file","rawInput":{"path":"README.md"}}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"ToolCallUpdate","toolCallId":"tool-1","name":"read_file","status":"completed","output":"contents"}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk","content":{"text":"final answer"}}}}'
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn","usage":{"inputTokens":11,"outputTokens":4}}}\n' "$id"
      sleep 0.01
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk","content":{"text":" after response"}}}}'
      ;;
  esac
done
"#,
        );
        let session = backend
            .execute("prompt", ExecOptions::default())
            .await
            .unwrap_or_else(|error| panic!("execute Qoder: {error}"));
        let Session {
            mut messages,
            result,
        } = session;
        let mut types = Vec::new();
        while let Some(message) = messages.recv().await {
            types.push(message.message_type);
        }
        let result = result
            .await
            .unwrap_or_else(|error| panic!("Qoder result: {error}"));
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "final answer after response");
        assert_eq!(result.session_id, "qoder-real");
        assert_eq!(result.usage["qoder-auto"].input_tokens, 11);
        assert!(types.contains(&MessageType::ToolUse));
        assert!(types.contains(&MessageType::ToolResult));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_terminates_owned_process_tree() {
        let (_directory, backend) = fake_backend("#!/bin/sh\nsleep 60 &\nwait\n");
        let cancellation = tokio_util::sync::CancellationToken::new();
        let session = backend
            .execute(
                "prompt",
                ExecOptions {
                    cancellation: cancellation.clone(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute Qoder: {error}"));
        cancellation.cancel();
        let result = tokio::time::timeout(Duration::from_secs(15), session.result)
            .await
            .unwrap_or_else(|error| panic!("cancellation exceeded bound: {error}"))
            .unwrap_or_else(|error| panic!("Qoder result: {error}"));
        assert_eq!(result.status, "aborted");
    }
}
