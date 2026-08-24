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

use crate::acp::{AcpClient, AcpError, AcpNotification};
use crate::acp_mcp::{
    build_acp_mcp_servers, filter_acp_mcp_servers, parse_acp_mcp_capabilities, AcpMcpServer,
};
use crate::command::{filter_custom_args, filter_launch_prefix, BlockedArgMode, RuntimeCommand};
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
use crate::process::OwnedProcessTree;
use crate::stderr::{with_stderr, SharedDiagnosticBuffer, DEFAULT_TAIL_BYTES};

const MESSAGE_BUFFER: usize = 256;
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const KILL_GRACE: Duration = Duration::from_secs(10);
const NOTIFICATION_QUIET: Duration = Duration::from_millis(250);
const NOTIFICATION_DRAIN_MAX: Duration = Duration::from_secs(2);

static BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> = LazyLock::new(|| {
    BTreeMap::from([
        ("--acp", BlockedArgMode::Standalone),
        ("acp", BlockedArgMode::Standalone),
        ("--yolo", BlockedArgMode::Standalone),
    ])
});
static TRAECLI_BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> =
    LazyLock::new(|| {
        BTreeMap::from([
            ("acp", BlockedArgMode::Standalone),
            ("serve", BlockedArgMode::Standalone),
            ("-y", BlockedArgMode::Standalone),
            ("--yolo", BlockedArgMode::Standalone),
            ("-p", BlockedArgMode::Standalone),
            ("--print", BlockedArgMode::Standalone),
            ("--output-format", BlockedArgMode::WithValue),
            ("--permission-mode", BlockedArgMode::WithValue),
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
    pub provider: String,
    pub launch_args: Vec<String>,
    pub resume_method: String,
}

impl Default for QoderConfig {
    fn default() -> Self {
        Self {
            command: RuntimeCommand::default(),
            env: BTreeMap::new(),
            default_command: "qodercli".to_string(),
            provider: "qoder".to_string(),
            launch_args: vec!["--yolo".to_string(), "--acp".to_string()],
            resume_method: "session/resume".to_string(),
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

#[derive(Debug, Clone, Default)]
pub struct TraecliConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct TraecliBackend {
    inner: QoderBackend,
}

impl TraecliBackend {
    pub fn new(config: TraecliConfig) -> Self {
        Self {
            inner: QoderBackend::new(QoderConfig {
                command: config.command,
                env: config.env,
                default_command: "traecli".to_string(),
                provider: "traecli".to_string(),
                launch_args: ["acp", "serve", "--yolo"].map(str::to_string).to_vec(),
                resume_method: "session/load".to_string(),
            }),
        }
    }
}

#[async_trait]
impl Backend for TraecliBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        self.inner.execute(prompt, options).await
    }
}

pub fn build_qoder_args(options: &ExecOptions) -> Vec<String> {
    build_session_args(&QoderConfig::default(), options)
}

pub fn build_traecli_args(options: &ExecOptions) -> Vec<String> {
    build_session_args(
        &QoderConfig {
            provider: "traecli".to_string(),
            launch_args: ["acp", "serve", "--yolo"].map(str::to_string).to_vec(),
            resume_method: "session/load".to_string(),
            default_command: "traecli".to_string(),
            ..QoderConfig::default()
        },
        options,
    )
}

fn build_session_args(config: &QoderConfig, options: &ExecOptions) -> Vec<String> {
    let blocked = blocked_args(&config.provider);
    let mut args = config.launch_args.clone();
    args.extend(filter_custom_args(&options.extra_args, blocked).args);
    args.extend(filter_custom_args(&options.custom_args, blocked).args);
    args
}

fn blocked_args(provider: &str) -> &'static BTreeMap<&'static str, BlockedArgMode> {
    if provider == "traecli" {
        &TRAECLI_BLOCKED_ARGS
    } else {
        &BLOCKED_ARGS
    }
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
        let blocked = blocked_args(&self.config.provider);
        let prefix = filter_launch_prefix(&self.config.command.prefix, blocked);
        log_blocked(
            &self.config.provider,
            "launch prefix",
            &prefix.blocked_flags,
        );
        log_blocked_args(&self.config.provider, blocked, &options);
        let mut argv = prefix.args;
        argv.extend(build_session_args(&self.config, &options));
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
        let provider = self.config.provider.clone();
        let resume_method = self.config.resume_method.clone();

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
                provider.clone(),
                resume_method,
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
                    ProtocolOutcome::failed(format!("{provider} protocol task failed: {error}"))
                }),
                RunEnd::Cancelled => ProtocolOutcome::terminal("aborted", "execution cancelled"),
                RunEnd::TimedOut => ProtocolOutcome::terminal(
                    "timeout",
                    format!("{provider} timed out after {}s", timeout.as_secs_f64()),
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
                if let Some(provider_error) =
                    provider_error(&provider, &stderr, &outcome.full_output)
                {
                    outcome.status = "failed".to_string();
                    outcome.error = provider_error;
                }
            }
            if !outcome.error.is_empty() {
                outcome.error = with_stderr(&outcome.error, &provider, &stderr);
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
    provider: String,
    resume_method: String,
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
        Err(error) => return ProtocolOutcome::failed(format!("{provider} initialize failed: {error}")),
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
                    return ProtocolOutcome::failed(format!(
                        "{provider} session/new returned no session ID"
                    ));
                }
                (result, session_id)
            }
            Err(error) => {
                return ProtocolOutcome::failed(format!("{provider} session/new failed: {error}"))
            }
        }
    } else {
        match client
            .request(
                &resume_method,
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
            Err(error) => return protocol_failure(&provider, &resume_method, error, String::new(), false),
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
            let stage = format!("could not switch to model {:?}", options.model);
            return protocol_failure(&provider, &stage, error, session_id, rejected);
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
            return protocol_failure(&provider, "session/prompt", error, session_id, rejected);
        }
    };
    if let Err(error) = client
        .drain_notifications(NOTIFICATION_QUIET, NOTIFICATION_DRAIN_MAX, |notification| {
            handle_notification(notification, &messages, &mut state)
        })
        .await
    {
        tracing::debug!(provider = %provider, error = %error, "ACP post-response notification drain ended early");
    }
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
            format!("{provider} cancelled the prompt")
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
    provider: &str,
    stage: &str,
    error: AcpError,
    session_id: String,
    rejected: bool,
) -> ProtocolOutcome {
    ProtocolOutcome {
        status: "failed".to_string(),
        error: format!("{provider} {stage} failed: {error}"),
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
                state.deliverable.text(text);
                send(messages, message(MessageType::Text, text));
            }
        }
        "agentthoughtchunk" => {
            if let Some(text) = content_text(data) {
                send(messages, message(MessageType::Thinking, text));
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

fn content_text(data: &Value) -> Option<&str> {
    data.get("content")?
        .get("text")?
        .as_str()
        .filter(|text| !text.is_empty())
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
    let pending = match state.tools.remove(id) {
        Some(pending) => pending,
        None => {
            state.deliverable.tool_boundary();
            PendingTool {
                name: tool_name(data),
                input: tool_input(data),
                emitted: false,
            }
        }
    };
    if !pending.emitted {
        send(messages, tool_use(id, &pending.name, pending.input));
    }
    let output = ["rawOutput", "output"]
        .iter()
        .find_map(|key| data.get(*key))
        .map(render_value)
        .or_else(|| content_text(data).map(str::to_string))
        .unwrap_or_default();
    let mut result = message(MessageType::ToolResult, "");
    result.call_id = id.to_string();
    result.output = output;
    result.status = status.to_string();
    send(messages, result);
}

fn tool_name(data: &Value) -> String {
    let name = data
        .get("name")
        .or_else(|| data.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .split(':')
        .next()
        .unwrap_or("tool")
        .trim()
        .to_string();
    match name.to_ascii_lowercase().as_str() {
        "shell" | "terminal" => "terminal".to_string(),
        "read" => "read_file".to_string(),
        "write" => "write_file".to_string(),
        _ => name,
    }
}

fn tool_input(data: &Value) -> BTreeMap<String, Value> {
    ["rawInput", "input", "parameters"]
        .iter()
        .find_map(|key| data.get(*key).and_then(Value::as_object))
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn render_value(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

fn parse_usage(value: Option<&Value>) -> TokenUsage {
    let value = value.unwrap_or(&Value::Null);
    TokenUsage {
        input_tokens: integer(value, &["inputTokens", "input_tokens"]),
        output_tokens: integer(value, &["outputTokens", "output_tokens"]),
        cache_read_tokens: integer(
            value,
            &["cachedReadTokens", "cacheReadTokens", "cache_read_tokens"],
        ),
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

fn provider_error(provider: &str, stderr: &str, output: &str) -> Option<String> {
    if let Some(found) = TERMINAL_PROVIDER_ERROR.find(stderr) {
        return Some(format!("{provider} provider error: {}", found.as_str()));
    }
    OUTPUT_PROVIDER_ERROR
        .find(output)
        .map(|found| format!("{provider} provider error: {}", found.as_str()))
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

fn log_blocked_args(
    provider: &str,
    blocked: &BTreeMap<&str, BlockedArgMode>,
    options: &ExecOptions,
) {
    log_blocked(
        "extra arguments",
        &filter_custom_args(&options.extra_args, blocked).blocked_flags,
    );
    log_blocked(
        "custom arguments",
        &filter_custom_args(&options.custom_args, blocked).blocked_flags,
    );
}

fn log_blocked(provider: &str, source: &str, flags: &[String]) {
    if !flags.is_empty() {
        tracing::warn!(provider, source, flags = ?flags, "ignored daemon-owned arguments");
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
    fn traecli_arguments_keep_acp_serve_and_headless_mode_owned() {
        let args = build_traecli_args(&ExecOptions {
            custom_args: [
                "acp",
                "serve",
                "--yolo",
                "--output-format",
                "json",
                "--add-dir",
                "/extra",
            ]
            .map(str::to_string)
            .to_vec(),
            ..ExecOptions::default()
        });
        assert_eq!(&args[..3], ["acp", "serve", "--yolo"]);
        assert_eq!(args.iter().filter(|arg| arg.as_str() == "acp").count(), 1);
        assert_eq!(args.iter().filter(|arg| arg.as_str() == "serve").count(), 1);
        assert!(!args.iter().any(|arg| arg == "json"));
        assert!(args.windows(2).any(|pair| pair == ["--add-dir", "/extra"]));
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
      sleep 0.05
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk","content":{"text":" tail"}}}}'
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
        assert_eq!(result.output, "final answer tail");
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

    #[cfg(unix)]
    #[tokio::test]
    async fn traecli_resume_uses_session_load_and_ignores_history_replay() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let executable = directory.path().join("traecli");
        let requests = directory.path().join("requests.jsonl");
        std::fs::write(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$TRAE_REQUESTS"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"loadSession":true}}}\n' "$id" ;;
    *'"method":"session/load"'*)
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk","content":{"text":"old history"}}}}'
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"method":"session/prompt"'*)
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk","content":{"text":"current answer"}}}}'
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id" ;;
  esac
done
"#,
        )
        .unwrap_or_else(|error| panic!("write fake TRAE: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake TRAE: {error}"));
        let backend = TraecliBackend::new(TraecliConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            env: BTreeMap::from([(
                "TRAE_REQUESTS".to_string(),
                requests.to_string_lossy().into_owned(),
            )]),
        });
        let session = backend
            .execute(
                "prompt",
                ExecOptions {
                    resume_session_id: "existing-session".to_string(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute TRAE: {error}"));
        let result = session
            .result
            .await
            .unwrap_or_else(|error| panic!("TRAE result: {error}"));
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "current answer");
        assert_eq!(result.session_id, "existing-session");
        let requests = std::fs::read_to_string(requests)
            .unwrap_or_else(|error| panic!("read TRAE requests: {error}"));
        assert!(requests.contains("\"method\":\"session/load\""));
        assert!(!requests.contains("\"method\":\"session/resume\""));
    }
}
