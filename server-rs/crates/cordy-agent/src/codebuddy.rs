//! CodeBuddy's headless bidirectional stream-JSON adapter.

use std::collections::BTreeMap;
use std::io;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinError;

use crate::command::{filter_custom_args, filter_launch_prefix, BlockedArgMode, RuntimeCommand};
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
use crate::mcp::{managed_object, write_managed_temp};
use crate::process::OwnedProcessTree;
use crate::stderr::{with_stderr, SharedDiagnosticBuffer, DEFAULT_TAIL_BYTES};
use crate::stream::{
    finalize_stream, resume_was_rejected, AgentLineReader, AssistantTurn, RunEnd, TerminalState,
};

const MESSAGE_BUFFER: usize = 256;
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const KILL_GRACE: Duration = Duration::from_secs(10);

type SharedStdin = Arc<Mutex<Option<ChildStdin>>>;

static BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> = LazyLock::new(|| {
    BTreeMap::from([
        ("-p", BlockedArgMode::Standalone),
        ("--output-format", BlockedArgMode::WithValue),
        ("--input-format", BlockedArgMode::WithValue),
        ("--permission-mode", BlockedArgMode::WithValue),
        ("--mcp-config", BlockedArgMode::WithValue),
        ("--effort", BlockedArgMode::WithValue),
    ])
});

#[derive(Debug, Clone, Default)]
pub struct CodebuddyConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct CodebuddyBackend {
    config: CodebuddyConfig,
}

impl CodebuddyBackend {
    pub fn new(config: CodebuddyConfig) -> Self {
        Self { config }
    }

    pub(crate) fn config(&self) -> &CodebuddyConfig {
        &self.config
    }
}

pub fn build_codebuddy_args(options: &ExecOptions) -> Vec<String> {
    let mut args = [
        "-p",
        "--output-format",
        "stream-json",
        "--input-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
        "--disallowedTools",
        "AskUserQuestion",
        "EnterPlanMode",
        "ExitPlanMode",
    ]
    .map(str::to_string)
    .to_vec();
    if !options.model.is_empty() {
        args.extend(["--model".to_string(), options.model.clone()]);
    }
    if !options.thinking_level.is_empty() {
        args.extend(["--effort".to_string(), options.thinking_level.clone()]);
    }
    if options.max_turns > 0 {
        args.extend(["--max-turns".to_string(), options.max_turns.to_string()]);
    }
    if !options.system_prompt.is_empty() {
        args.extend([
            "--append-system-prompt".to_string(),
            options.system_prompt.clone(),
        ]);
    }
    if !options.resume_session_id.is_empty() {
        args.extend(["--resume".to_string(), options.resume_session_id.clone()]);
    }
    args.extend(filter_custom_args(&options.extra_args, &BLOCKED_ARGS).args);
    args.extend(filter_custom_args(&options.custom_args, &BLOCKED_ARGS).args);
    args
}

#[async_trait]
impl Backend for CodebuddyBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        managed_object(options.mcp_config.as_ref()).map_err(AgentError::InvalidConfig)?;
        let command_path = if self.config.command.path.is_empty() {
            "codebuddy"
        } else {
            self.config.command.path.as_str()
        };
        let prefix = filter_launch_prefix(&self.config.command.prefix, &BLOCKED_ARGS);
        log_blocked("launch prefix", &prefix.blocked_flags);
        log_blocked_args(&options);
        let mut argv = prefix.args;
        argv.extend(build_codebuddy_args(&options));

        let mut mcp_file = write_managed_temp(options.mcp_config.as_ref(), "cordy-codebuddy-mcp-")?;
        if let Some(file) = mcp_file.as_ref() {
            let path = file.path().to_str().ok_or_else(|| {
                AgentError::InvalidConfig("CodeBuddy MCP path is not valid UTF-8".to_string())
            })?;
            // Never add --strict-mcp-config: managed entries augment the CLI's
            // own user/project/local scopes and win same-name collisions.
            argv.extend(["--mcp-config".to_string(), path.to_string()]);
        }

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
        let stdin = tree.child_mut().stdin.take().ok_or_else(|| {
            AgentError::Protocol("CodeBuddy stdin pipe unavailable after spawn".to_string())
        })?;
        let stdout = tree.child_mut().stdout.take().ok_or_else(|| {
            AgentError::Protocol("CodeBuddy stdout pipe unavailable after spawn".to_string())
        })?;
        let stderr = tree.child_mut().stderr.take().ok_or_else(|| {
            AgentError::Protocol("CodeBuddy stderr pipe unavailable after spawn".to_string())
        })?;

        let (message_tx, message_rx) = mpsc::channel(MESSAGE_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();
        let started = Instant::now();
        let timeout = options.timeout;
        let cancellation = options.cancellation.clone();
        let requested_resume = options.resume_session_id.clone();
        let fallback_model = options.model.clone();
        let stdin = Arc::new(Mutex::new(Some(stdin)));
        let prompt_bytes = prompt_input(prompt)?;
        let stderr_tail = SharedDiagnosticBuffer::new(DEFAULT_TAIL_BYTES);

        tokio::spawn(async move {
            let _mcp_file = mcp_file.take();
            let mut stderr_task = tokio::spawn(pump_stderr(stderr, stderr_tail.clone()));
            let prompt_stdin = Arc::clone(&stdin);
            let mut prompt_task =
                tokio::spawn(async move { write_stdin(prompt_stdin, &prompt_bytes).await });
            let mut stdout_task = tokio::spawn(read_stream(
                stdout,
                Arc::clone(&stdin),
                message_tx,
                fallback_model.clone(),
            ));

            let end = {
                let completion = async {
                    let exit = tree.wait().await;
                    let stream = (&mut stdout_task).await;
                    let write = (&mut prompt_task).await;
                    (exit, stream, write)
                };
                tokio::pin!(completion);
                if timeout.is_zero() {
                    tokio::select! {
                        completed = &mut completion => RunOutcome::Completed(completed),
                        () = cancellation.cancelled() => RunOutcome::Cancelled,
                    }
                } else {
                    tokio::select! {
                        completed = &mut completion => RunOutcome::Completed(completed),
                        () = cancellation.cancelled() => RunOutcome::Cancelled,
                        () = tokio::time::sleep(timeout) => RunOutcome::TimedOut,
                    }
                }
            };

            let (run_end, exit, stream, write) = match end {
                RunOutcome::Completed((exit, stream, write)) => {
                    (RunEnd::Completed, Some(exit), stream, write)
                }
                RunOutcome::Cancelled => {
                    let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
                    close_stdin(&stdin).await;
                    (
                        RunEnd::Cancelled,
                        None,
                        (&mut stdout_task).await,
                        (&mut prompt_task).await,
                    )
                }
                RunOutcome::TimedOut => {
                    let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
                    close_stdin(&stdin).await;
                    (
                        RunEnd::DeadlineExceeded,
                        None,
                        (&mut stdout_task).await,
                        (&mut prompt_task).await,
                    )
                }
            };
            close_stdin(&stdin).await;

            if tokio::time::timeout(KILL_GRACE, &mut stderr_task)
                .await
                .is_err()
            {
                stderr_task.abort();
            }
            let stderr = stderr_tail.tail();
            if !stderr.is_empty() {
                tracing::debug!(provider = "codebuddy", stderr = %stderr, "agent stderr captured");
            }

            let mut state = stream.unwrap_or_else(join_failure_state);
            let write_error = write_error(write);
            let exit_error = exit
                .as_ref()
                .and_then(|exit| process_exit_error(exit.as_ref()));
            let finalized = finalize_stream(
                "codebuddy",
                timeout,
                run_end,
                write_error.as_deref(),
                exit_error.as_deref(),
                &state.session_id,
                &state.terminal,
                None,
            );
            let failed = finalized.status == "failed";
            let resume_rejected = resume_was_rejected(
                &requested_resume,
                &state.session_id,
                failed,
                [finalized.error.as_str(), stderr.as_str()],
            );
            if resume_rejected {
                state.session_id.clear();
            }
            let error = if finalized.error.is_empty() {
                String::new()
            } else {
                with_stderr(&finalized.error, "codebuddy", &stderr)
            };
            let _ = result_tx.send(ExecutionResult {
                status: finalized.status,
                output: finalized.output,
                error,
                duration_ms: started.elapsed().as_millis().try_into().unwrap_or(i64::MAX),
                session_id: state.session_id,
                usage: state.usage,
                resume_rejected,
            });
        });

        Ok(Session {
            messages: message_rx,
            result: result_rx,
        })
    }
}

fn log_blocked_args(options: &ExecOptions) {
    let extra = filter_custom_args(&options.extra_args, &BLOCKED_ARGS);
    let custom = filter_custom_args(&options.custom_args, &BLOCKED_ARGS);
    log_blocked("extra arguments", &extra.blocked_flags);
    log_blocked("custom arguments", &custom.blocked_flags);
}

fn log_blocked(source: &str, flags: &[String]) {
    if !flags.is_empty() {
        tracing::warn!(provider = "codebuddy", source, flags = ?flags, "ignored daemon-owned arguments");
    }
}

fn prompt_input(prompt: &str) -> Result<Vec<u8>, AgentError> {
    let mut payload = serde_json::to_vec(&serde_json::json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": prompt}],
        },
    }))
    .map_err(|error| AgentError::Protocol(format!("serialize CodeBuddy input: {error}")))?;
    payload.push(b'\n');
    Ok(payload)
}

async fn write_stdin(stdin: SharedStdin, bytes: &[u8]) -> io::Result<()> {
    let mut guard = stdin.lock().await;
    let writer = guard
        .as_mut()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "CodeBuddy stdin closed"))?;
    writer.write_all(bytes).await?;
    writer.flush().await
}

async fn close_stdin(stdin: &SharedStdin) {
    stdin.lock().await.take();
}

async fn pump_stderr(mut stderr: tokio::process::ChildStderr, tail: SharedDiagnosticBuffer) {
    let mut buffer = [0_u8; 8192];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(bytes) => tail.push(&buffer[..bytes]),
        }
    }
}

async fn read_stream(
    stdout: tokio::process::ChildStdout,
    stdin: SharedStdin,
    messages: mpsc::Sender<Message>,
    fallback_model: String,
) -> CodebuddyStreamState {
    let mut reader = AgentLineReader::new(BufReader::new(stdout));
    let mut state = CodebuddyStreamState::default();
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<CodebuddyEvent>(line) {
                    Ok(event) => {
                        handle_event(event, &messages, &stdin, &fallback_model, &mut state).await
                    }
                    Err(_) => state.invalid_event_count += 1,
                }
            }
            Ok(None) => {
                close_stdin(&stdin).await;
                return state;
            }
            Err(error) => {
                state.terminal.scan_error = error.to_string();
                close_stdin(&stdin).await;
                return state;
            }
        }
    }
}

type CodebuddyJoinResults = (
    io::Result<ExitStatus>,
    Result<CodebuddyStreamState, JoinError>,
    Result<io::Result<()>, JoinError>,
);

enum RunOutcome {
    Completed(CodebuddyJoinResults),
    Cancelled,
    TimedOut,
}

fn write_error(write: Result<io::Result<()>, JoinError>) -> Option<String> {
    match write {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error.to_string()),
        Err(error) => Some(format!("input task failed: {error}")),
    }
}

fn process_exit_error(exit: Result<&ExitStatus, &io::Error>) -> Option<String> {
    match exit {
        Ok(status) if status.success() => None,
        Ok(status) => Some(status.to_string()),
        Err(error) => Some(format!("wait failed: {error}")),
    }
}

fn join_failure_state(error: JoinError) -> CodebuddyStreamState {
    CodebuddyStreamState {
        terminal: TerminalState {
            scan_error: format!("stream task failed: {error}"),
            ..TerminalState::default()
        },
        ..CodebuddyStreamState::default()
    }
}

#[derive(Debug, Deserialize)]
struct CodebuddyEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    message: Value,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    model: String,
    #[serde(default, rename = "result")]
    result_text: String,
    #[serde(default)]
    is_error: bool,
    usage: Option<CodebuddyUsage>,
    #[serde(default, rename = "modelUsage")]
    model_usage: BTreeMap<String, CodebuddyResultUsage>,
    log: Option<CodebuddyLog>,
    #[serde(default)]
    request_id: String,
    #[serde(default)]
    request: Value,
}

#[derive(Debug, Deserialize)]
struct CodebuddyPayload {
    #[serde(default)]
    model: String,
    #[serde(default)]
    content: Vec<CodebuddyBlock>,
    usage: Option<CodebuddyUsage>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct CodebuddyUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    cache_read_input_tokens: i64,
    #[serde(default)]
    cache_creation_input_tokens: i64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct CodebuddyResultUsage {
    #[serde(default, rename = "inputTokens")]
    input_tokens: i64,
    #[serde(default, rename = "outputTokens")]
    output_tokens: i64,
    #[serde(default, rename = "cacheReadInputTokens")]
    cache_read_input_tokens: i64,
    #[serde(default, rename = "cacheCreationInputTokens")]
    cache_creation_input_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct CodebuddyBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    input: Value,
    #[serde(default)]
    tool_use_id: String,
    #[serde(default)]
    content: Value,
}

#[derive(Debug, Deserialize)]
struct CodebuddyLog {
    #[serde(default)]
    level: String,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Default)]
struct CodebuddyStreamState {
    session_id: String,
    terminal: TerminalState,
    usage: BTreeMap<String, TokenUsage>,
    invalid_event_count: usize,
}

async fn handle_event(
    event: CodebuddyEvent,
    messages: &mpsc::Sender<Message>,
    stdin: &SharedStdin,
    fallback_model: &str,
    state: &mut CodebuddyStreamState,
) {
    match event.event_type.as_str() {
        "assistant" => handle_assistant(event.message, messages, state),
        "user" => handle_user(event.message, messages),
        "system" => {
            if !event.session_id.is_empty() {
                state.session_id = event.session_id;
            }
            send_message(
                messages,
                Message {
                    status: "running".to_string(),
                    session_id: state.session_id.clone(),
                    ..empty_message(MessageType::Status)
                },
            );
        }
        "result" => {
            let usage = result_usage(&event, fallback_model);
            state.terminal.saw_result = true;
            state.terminal.result_is_error = event.is_error;
            state.terminal.final_result_text = event.result_text;
            state.session_id = event.session_id;
            if let Some(usage) = usage {
                state.usage = usage;
            }
            close_stdin(stdin).await;
        }
        "log" => {
            if let Some(log) = event.log {
                send_message(
                    messages,
                    Message {
                        content: log.message,
                        level: log.level,
                        ..empty_message(MessageType::Log)
                    },
                );
            }
        }
        "control_request" => {
            if let Some(response) = control_response(&event) {
                if let Err(error) = write_stdin(Arc::clone(stdin), &response).await {
                    tracing::warn!(provider = "codebuddy", %error, "write control response failed");
                }
            }
        }
        _ => {}
    }
}

fn handle_assistant(
    raw: Value,
    messages: &mpsc::Sender<Message>,
    state: &mut CodebuddyStreamState,
) {
    let Ok(payload) = serde_json::from_value::<CodebuddyPayload>(raw) else {
        state.terminal.last_assistant_text.clear();
        return;
    };
    if let Some(usage) = payload.usage {
        add_usage(&mut state.usage, &payload.model, usage);
    }
    let mut turn = AssistantTurn {
        understood: true,
        ..AssistantTurn::default()
    };
    for block in payload.content {
        match block.block_type.as_str() {
            "text" if !block.text.is_empty() => {
                turn.text.push_str(&block.text);
                send_message(
                    messages,
                    Message {
                        content: block.text,
                        ..empty_message(MessageType::Text)
                    },
                );
            }
            "thinking" if !block.text.is_empty() => send_message(
                messages,
                Message {
                    content: block.text,
                    ..empty_message(MessageType::Thinking)
                },
            ),
            "tool_use" => {
                turn.tool_uses += 1;
                let input = block
                    .input
                    .as_object()
                    .map(|object| {
                        object
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                send_message(
                    messages,
                    Message {
                        tool: block.name,
                        call_id: block.id,
                        input,
                        ..empty_message(MessageType::ToolUse)
                    },
                );
            }
            "text" | "thinking" => {}
            _ => turn.understood = false,
        }
    }
    state.terminal.last_assistant_text = turn.resolve_fallback(&state.terminal.last_assistant_text);
}

fn handle_user(raw: Value, messages: &mpsc::Sender<Message>) {
    let Ok(payload) = serde_json::from_value::<CodebuddyPayload>(raw) else {
        return;
    };
    for block in payload.content {
        if block.block_type == "tool_result" {
            send_message(
                messages,
                Message {
                    call_id: block.tool_use_id,
                    output: if block.content.is_null() {
                        String::new()
                    } else {
                        block.content.to_string()
                    },
                    ..empty_message(MessageType::ToolResult)
                },
            );
        }
    }
}

fn control_response(event: &CodebuddyEvent) -> Option<Vec<u8>> {
    let input = event
        .request
        .get("input")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut response = serde_json::to_vec(&serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": event.request_id,
            "response": {
                "allowed": true,
                "behavior": "allow",
                "updatedInput": input,
            },
        },
    }))
    .ok()?;
    response.push(b'\n');
    Some(response)
}

fn add_usage(usage: &mut BTreeMap<String, TokenUsage>, model: &str, value: CodebuddyUsage) {
    if model.is_empty() {
        return;
    }
    let entry = usage.entry(model.to_string()).or_default();
    entry.input_tokens = entry.input_tokens.saturating_add(value.input_tokens);
    entry.output_tokens = entry.output_tokens.saturating_add(value.output_tokens);
    entry.cache_read_tokens = entry
        .cache_read_tokens
        .saturating_add(value.cache_read_input_tokens);
    entry.cache_write_tokens = entry
        .cache_write_tokens
        .saturating_add(value.cache_creation_input_tokens);
}

fn result_usage(
    event: &CodebuddyEvent,
    fallback_model: &str,
) -> Option<BTreeMap<String, TokenUsage>> {
    let models: BTreeMap<String, TokenUsage> = event
        .model_usage
        .iter()
        .filter(|(model, usage)| !model.is_empty() && usage.has_tokens())
        .map(|(model, usage)| (model.clone(), usage.normalized()))
        .collect();
    if !models.is_empty() {
        return Some(models);
    }
    let usage = event.usage.filter(CodebuddyUsage::has_tokens)?;
    let model = if event.model.is_empty() {
        fallback_model
    } else {
        &event.model
    };
    (!model.is_empty()).then(|| BTreeMap::from([(model.to_string(), usage.normalized())]))
}

impl CodebuddyUsage {
    fn has_tokens(&self) -> bool {
        self.input_tokens > 0
            || self.output_tokens > 0
            || self.cache_read_input_tokens > 0
            || self.cache_creation_input_tokens > 0
    }

    fn normalized(self) -> TokenUsage {
        TokenUsage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_input_tokens,
            cache_write_tokens: self.cache_creation_input_tokens,
            ..TokenUsage::default()
        }
    }
}

impl CodebuddyResultUsage {
    fn has_tokens(&self) -> bool {
        self.input_tokens > 0
            || self.output_tokens > 0
            || self.cache_read_input_tokens > 0
            || self.cache_creation_input_tokens > 0
    }

    fn normalized(self) -> TokenUsage {
        TokenUsage {
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            cache_read_tokens: self.cache_read_input_tokens,
            cache_write_tokens: self.cache_creation_input_tokens,
            ..TokenUsage::default()
        }
    }
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

fn send_message(messages: &mpsc::Sender<Message>, message: Message) {
    let _ = messages.try_send(message);
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn arguments_own_protocol_permissions_and_headless_tools() {
        let options = ExecOptions {
            model: "sonnet".to_string(),
            thinking_level: "high".to_string(),
            max_turns: 25,
            system_prompt: "runtime brief".to_string(),
            resume_session_id: "session-1".to_string(),
            extra_args: vec!["--output-format".to_string(), "text".to_string()],
            custom_args: vec![
                "--effort=max".to_string(),
                "--max-budget-usd".to_string(),
                "2".to_string(),
            ],
            ..ExecOptions::default()
        };
        let args = build_codebuddy_args(&options);
        assert_eq!(
            &args[..12],
            [
                "-p",
                "--output-format",
                "stream-json",
                "--input-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "bypassPermissions",
                "--disallowedTools",
                "AskUserQuestion",
                "EnterPlanMode",
                "ExitPlanMode",
            ]
        );
        assert!(args.windows(2).any(|pair| pair == ["--effort", "high"]));
        assert!(args.windows(2).any(|pair| pair == ["--max-turns", "25"]));
        assert!(!args
            .iter()
            .any(|arg| arg == "text" || arg == "--effort=max"));
        assert!(!args.iter().any(|arg| arg == "--strict-mcp-config"));
    }

    #[test]
    fn prompt_and_control_payloads_use_native_bidirectional_shapes() {
        let prompt = prompt_input("hello").unwrap_or_else(|error| panic!("prompt: {error}"));
        let prompt: Value = serde_json::from_slice(&prompt)
            .unwrap_or_else(|error| panic!("decode prompt: {error}"));
        assert_eq!(prompt["message"]["content"][0]["text"], "hello");

        let event: CodebuddyEvent = serde_json::from_value(serde_json::json!({
            "type": "control_request",
            "request_id": "permission-1",
            "request": {"input": {"command": "ls"}},
        }))
        .unwrap_or_else(|error| panic!("control event: {error}"));
        let response = control_response(&event).unwrap_or_else(|| panic!("control response"));
        let response: Value = serde_json::from_slice(&response)
            .unwrap_or_else(|error| panic!("decode response: {error}"));
        assert_eq!(response["response"]["request_id"], "permission-1");
        assert_eq!(response["response"]["response"]["allowed"], true);
        assert_eq!(response["response"]["response"]["behavior"], "allow");
        assert_eq!(
            response["response"]["response"]["updatedInput"]["command"],
            "ls"
        );
    }

    #[test]
    fn result_usage_prefers_authoritative_per_model_map() {
        let event: CodebuddyEvent = serde_json::from_value(serde_json::json!({
            "type": "result",
            "model": "fallback",
            "usage": {"input_tokens": 1},
            "modelUsage": {
                "sonnet": {"inputTokens": 100, "outputTokens": 50,
                    "cacheReadInputTokens": 10, "cacheCreationInputTokens": 5}
            }
        }))
        .unwrap_or_else(|error| panic!("result event: {error}"));
        let usage =
            result_usage(&event, "fallback").unwrap_or_else(|| panic!("result usage must exist"));
        assert_eq!(usage.len(), 1);
        assert_eq!(usage["sonnet"].input_tokens, 100);
        assert_eq!(usage["sonnet"].cache_write_tokens, 5);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_streams_real_bidirectional_process() {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| panic!("create CodeBuddy fixture: {error}"));
        let executable = directory.path().join("codebuddy");
        std::fs::write(
            &executable,
            r#"#!/bin/sh
IFS= read -r prompt
printf '%s\n' '{"type":"system","session_id":"session-cb"}'
printf '%s\n' '{"type":"assistant","message":{"model":"sonnet","content":[{"type":"text","text":"PONG"}]}}'
printf '%s\n' '{"type":"result","session_id":"session-cb","result":"PONG","is_error":false}'
"#,
        )
        .unwrap_or_else(|error| panic!("write CodeBuddy fixture: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod CodeBuddy fixture: {error}"));
        let backend = CodebuddyBackend::new(CodebuddyConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            ..CodebuddyConfig::default()
        });
        let Session {
            mut messages,
            result,
        } = backend
            .execute("reply PONG", ExecOptions::default())
            .await
            .unwrap_or_else(|error| panic!("execute CodeBuddy fixture: {error}"));
        let mut saw_text = false;
        while let Some(message) = messages.recv().await {
            saw_text |= message.message_type == MessageType::Text && message.content == "PONG";
        }
        let result = result
            .await
            .unwrap_or_else(|error| panic!("receive CodeBuddy result: {error}"));
        assert!(saw_text);
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "PONG");
        assert_eq!(result.session_id, "session-cb");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_breaks_a_blocked_prompt_write_before_drain() {
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("create blocked CodeBuddy fixture: {error}"));
        let executable = directory.path().join("codebuddy");
        std::fs::write(&executable, "#!/bin/sh\nsleep 60\n")
            .unwrap_or_else(|error| panic!("write blocked CodeBuddy fixture: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod blocked CodeBuddy fixture: {error}"));
        let backend = CodebuddyBackend::new(CodebuddyConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            ..CodebuddyConfig::default()
        });
        let cancellation = tokio_util::sync::CancellationToken::new();
        let session = backend
            .execute(
                &"x".repeat(2 * 1024 * 1024),
                ExecOptions {
                    cancellation: cancellation.clone(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute blocked CodeBuddy fixture: {error}"));
        cancellation.cancel();
        let result = tokio::time::timeout(Duration::from_secs(15), session.result)
            .await
            .unwrap_or_else(|error| panic!("blocked write cancellation exceeded bound: {error}"))
            .unwrap_or_else(|error| panic!("receive cancelled CodeBuddy result: {error}"));
        assert_eq!(result.status, "aborted");
    }
}
