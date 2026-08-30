//! Cursor Agent's stdin/stream-json adapter.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
#[cfg(windows)]
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinError;
use tokio_util::sync::CancellationToken;

use crate::command::{filter_custom_args, filter_launch_prefix, BlockedArgMode, RuntimeCommand};
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
use crate::env::configure_child_env;
use crate::model::{Catalog, CatalogCache, Model, ModelDiscoveryCacheKey};
use crate::process::OwnedProcessTree;
use crate::stderr::{sanitize_diagnostic, with_stderr, SharedDiagnosticBuffer, DEFAULT_TAIL_BYTES};
use crate::stream::AgentLineReader;

const MESSAGE_BUFFER: usize = 256;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVERY_OUTPUT_MAX: usize = 4 * 1024 * 1024;
const TERMINATION_GRACE: Duration = Duration::from_millis(500);
const KILL_GRACE: Duration = Duration::from_secs(10);

type SharedStdin = Arc<Mutex<Option<ChildStdin>>>;

pub(crate) static BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> =
    LazyLock::new(|| {
        BTreeMap::from([
            ("-p", BlockedArgMode::Standalone),
            ("--output-format", BlockedArgMode::WithValue),
            ("--yolo", BlockedArgMode::Standalone),
        ])
    });

#[derive(Debug, Clone, Default)]
pub struct CursorConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct CursorBackend {
    config: CursorConfig,
}

impl CursorBackend {
    pub fn new(config: CursorConfig) -> Self {
        Self { config }
    }

    pub async fn discover_models(
        &self,
        cache: &CatalogCache,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        self.discover_models_for_runtime("cursor", cache, cancellation, timeout)
            .await
    }

    pub async fn discover_models_for_runtime(
        &self,
        runtime_scope: &str,
        cache: &CatalogCache,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        let scope = if runtime_scope.trim().is_empty() {
            "cursor"
        } else {
            runtime_scope
        };
        let Some(key) = ModelDiscoveryCacheKey::new(scope, &self.config.command) else {
            return static_catalog();
        };
        if let Some(catalog) = cache.get(&key) {
            return catalog;
        }
        let timeout = if timeout.is_zero() {
            DISCOVERY_TIMEOUT
        } else {
            timeout
        };
        let output = capture_model_list(
            &self.config.command,
            &self.config.env,
            cancellation,
            timeout,
        )
        .await;
        if let Some(output) = output {
            let models = parse_cursor_models(&output);
            if !models.is_empty() {
                let catalog = Catalog {
                    models,
                    fallback: false,
                };
                let _ = cache.insert(key, catalog.clone());
                return catalog;
            }
        }
        static_catalog()
    }
}

#[async_trait]
impl Backend for CursorBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        let command_path = command_path(&self.config.command);
        let prefix = filter_launch_prefix(&self.config.command.prefix, &BLOCKED_ARGS);
        let mut argv = prefix.args;
        argv.extend(build_cursor_args(&options));
        let (executable, argv) = platform_invocation(command_path, argv);

        let mut command = Command::new(&executable);
        command
            .args(&argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);
        configure_child_environment(&mut command, &self.config.env);
        if !options.cwd.is_empty() {
            command.current_dir(&options.cwd);
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
            AgentError::Protocol("Cursor stdin pipe unavailable after spawn".to_string())
        })?;
        let stdout = tree.child_mut().stdout.take().ok_or_else(|| {
            AgentError::Protocol("Cursor stdout pipe unavailable after spawn".to_string())
        })?;
        let stderr = tree.child_mut().stderr.take().ok_or_else(|| {
            AgentError::Protocol("Cursor stderr pipe unavailable after spawn".to_string())
        })?;

        let (message_tx, message_rx) = mpsc::channel(MESSAGE_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();
        let stdin = Arc::new(Mutex::new(Some(stdin)));
        let started = Instant::now();
        let timeout = options.timeout;
        let cancellation = options.cancellation.clone();
        let configured_model = options.model.clone();
        let stderr_tail = SharedDiagnosticBuffer::new(DEFAULT_TAIL_BYTES);
        let prompt = prompt.as_bytes().to_vec();

        tokio::spawn(async move {
            run_cursor(
                tree,
                stdin,
                stdout,
                stderr,
                prompt,
                message_tx,
                result_tx,
                cancellation,
                timeout,
                configured_model,
                started,
                stderr_tail,
            )
            .await;
        });

        Ok(Session {
            messages: message_rx,
            result: result_rx,
        })
    }
}

pub fn build_cursor_args(options: &ExecOptions) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--yolo".to_string(),
    ];
    if !options.cwd.is_empty() {
        args.extend(["--workspace".to_string(), options.cwd.clone()]);
    }
    if !options.model.is_empty() {
        args.extend(["--model".to_string(), options.model.clone()]);
    }
    if !options.resume_session_id.is_empty() {
        args.extend(["--resume".to_string(), options.resume_session_id.clone()]);
    }
    args.extend(filter_custom_args(&options.custom_args, &BLOCKED_ARGS).args);
    args
}

fn command_path(command: &RuntimeCommand) -> String {
    if command.path.trim().is_empty() {
        "cursor-agent".to_string()
    } else {
        command.path.clone()
    }
}

fn configure_child_environment(command: &mut Command, extra: &BTreeMap<String, String>) {
    configure_child_env(command, extra);
}

#[allow(clippy::too_many_arguments)]
async fn run_cursor(
    mut tree: OwnedProcessTree,
    stdin: SharedStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
    prompt: Vec<u8>,
    messages: mpsc::Sender<Message>,
    result_tx: oneshot::Sender<ExecutionResult>,
    cancellation: CancellationToken,
    timeout: Duration,
    configured_model: String,
    started: Instant,
    stderr_tail: SharedDiagnosticBuffer,
) {
    let mut stderr_task = tokio::spawn(pump_stderr(stderr, stderr_tail.clone()));
    let prompt_task = tokio::spawn(write_cursor_prompt(Arc::clone(&stdin), prompt));
    let mut stdout_task = tokio::spawn(read_cursor_stream(stdout, messages, configured_model));

    let outcome = {
        let completion = async {
            tokio::select! {
                state = (&mut stdout_task) => CursorCompletion::Stream(state),
                exit = tree.wait() => {
                    let state = (&mut stdout_task).await;
                    CursorCompletion::Process(exit, state)
                }
            }
        };
        tokio::pin!(completion);
        if timeout.is_zero() {
            tokio::select! {
                completed = &mut completion => RunOutcome::Completed(Box::new(completed)),
                () = cancellation.cancelled() => RunOutcome::Cancelled,
            }
        } else {
            tokio::select! {
                completed = &mut completion => RunOutcome::Completed(Box::new(completed)),
                () = cancellation.cancelled() => RunOutcome::Cancelled,
                () = tokio::time::sleep(timeout) => RunOutcome::TimedOut,
            }
        }
    };

    let (run_end, exit, stream, write) = match outcome {
        RunOutcome::Completed(completed) => match *completed {
            CursorCompletion::Process(exit, stream) => {
                close_stdin(&stdin).await;
                (RunEnd::Completed, Some(exit), stream, prompt_task.await)
            }
            CursorCompletion::Stream(stream) => {
                prompt_task.abort();
                close_stdin(&stdin).await;
                let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
                (RunEnd::Completed, None, stream, Ok(Ok(())))
            }
        },
        RunOutcome::Cancelled => {
            prompt_task.abort();
            close_stdin(&stdin).await;
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            (
                RunEnd::Cancelled,
                None,
                (&mut stdout_task).await,
                Ok(Ok(())),
            )
        }
        RunOutcome::TimedOut => {
            prompt_task.abort();
            close_stdin(&stdin).await;
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            (RunEnd::TimedOut, None, (&mut stdout_task).await, Ok(Ok(())))
        }
    };

    if tokio::time::timeout(KILL_GRACE, &mut stderr_task)
        .await
        .is_err()
    {
        stderr_task.abort();
    }
    let stderr = stderr_tail.tail();
    let mut state = stream.unwrap_or_else(join_failure_state);
    let exit_error = exit
        .as_ref()
        .and_then(|status| process_exit_error(status.as_ref()));
    let exit_code = exit
        .as_ref()
        .map(|status| process_exit_code(status.as_ref()))
        .unwrap_or(0);
    let write_error = write_error(write);
    let (status, mut error) = finalize_cursor(
        run_end,
        timeout,
        &mut state,
        write_error.as_deref(),
        exit_error.as_deref(),
        exit_code,
    );
    if state.unhandled_subtype_count > 0 {
        tracing::warn!(
            provider = "cursor-agent",
            count = state.unhandled_subtype_count,
            "cursor-agent ignored unhandled event subtypes"
        );
    }
    if state.unhandled_types.total > 0 {
        let types = state.unhandled_types.summary();
        tracing::warn!(
            provider = "cursor-agent",
            count = state.unhandled_types.total,
            %types,
            "cursor-agent ignored unhandled event types"
        );
    }
    if !error.is_empty() {
        error = sanitize_diagnostic(&error);
        if state.saw_result {
            // Cursor result errors already contain the provider's terminal
            // message; Go deliberately does not append stderr in this case.
        } else {
            error = with_stderr(&error, "cursor", &stderr);
        }
    }
    let _ = result_tx.send(ExecutionResult {
        status,
        output: cursor_result_output(&state),
        error,
        duration_ms: started.elapsed().as_millis().try_into().unwrap_or(i64::MAX),
        session_id: state.session_id,
        usage: if state.has_result_usage {
            state.result_usage
        } else {
            state.step_usage
        },
        resume_rejected: false,
    });
}

async fn write_cursor_prompt(stdin: SharedStdin, prompt: Vec<u8>) -> io::Result<()> {
    let mut guard = stdin.lock().await;
    let writer = guard
        .as_mut()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "Cursor stdin closed"))?;
    writer.write_all(&prompt).await?;
    writer.flush().await
}

async fn close_stdin(stdin: &SharedStdin) {
    stdin.lock().await.take();
}

async fn read_cursor_stream(
    stdout: ChildStdout,
    messages: mpsc::Sender<Message>,
    configured_model: String,
) -> CursorStreamState {
    let mut reader = AgentLineReader::new(BufReader::new(stdout));
    let mut state = CursorStreamState::new(configured_model);
    loop {
        match reader.next_line().await {
            Ok(Some(raw)) => {
                let line = normalize_cursor_stream_line(&raw);
                if line.is_empty() {
                    continue;
                }
                let event = match serde_json::from_str::<CursorEvent>(&line) {
                    Ok(event) => event,
                    Err(_) => {
                        state.invalid_event_count = state.invalid_event_count.saturating_add(1);
                        continue;
                    }
                };
                state.event_count = state.event_count.saturating_add(1);
                state.last_event_type = observed_event_type(&event.event_type);
                if handle_event(event, &messages, &mut state) {
                    return state;
                }
            }
            Ok(None) => return state,
            Err(error) => {
                state.scan_error = error.to_string();
                return state;
            }
        }
    }
}

fn handle_event(
    event: CursorEvent,
    messages: &mpsc::Sender<Message>,
    state: &mut CursorStreamState,
) -> bool {
    if !event.session_id.trim().is_empty() {
        state.session_id = event.session_id.clone();
    }
    match event.event_type.as_str() {
        "system" => match event.subtype.as_str() {
            "init" => send_message(
                messages,
                Message {
                    status: "running".to_string(),
                    ..empty_message(MessageType::Status)
                },
            ),
            "error" => {
                if let Some(error) = cursor_error_text(&event) {
                    state.protocol_error = error.clone();
                    send_message(
                        messages,
                        Message {
                            content: error,
                            ..empty_message(MessageType::Error)
                        },
                    );
                }
            }
            _ => {}
        },
        "assistant" => {
            state.assistant_event_count = state.assistant_event_count.saturating_add(1);
            handle_assistant(&event.message, messages, state);
        }
        "thinking" => match event.subtype.as_str() {
            "delta" => {
                if let Some(text) = event.text.as_deref().filter(|text| !text.is_empty()) {
                    let text = state.thinking.delta(text);
                    if !text.is_empty() {
                        send_message(
                            messages,
                            Message {
                                content: text,
                                ..empty_message(MessageType::Thinking)
                            },
                        );
                    }
                }
            }
            "completed" => state.thinking.complete(),
            _ => state.unhandled_subtype_count = state.unhandled_subtype_count.saturating_add(1),
        },
        "tool_call" => match event.subtype.as_str() {
            "started" => {
                let call = parse_tool_call(&event);
                state.tool_use_count = state.tool_use_count.saturating_add(1);
                send_message(
                    messages,
                    Message {
                        tool: call.name,
                        call_id: call.call_id,
                        input: call.input,
                        ..empty_message(MessageType::ToolUse)
                    },
                );
            }
            "completed" => {
                let call = parse_tool_call(&event);
                send_message(
                    messages,
                    Message {
                        tool: call.name,
                        call_id: call.call_id,
                        output: call.result,
                        ..empty_message(MessageType::ToolResult)
                    },
                );
            }
            _ => state.unhandled_subtype_count = state.unhandled_subtype_count.saturating_add(1),
        },
        "tool_use" => {
            state.tool_use_count = state.tool_use_count.saturating_add(1);
            send_message(
                messages,
                Message {
                    tool: event.tool_name,
                    call_id: event.tool_id,
                    input: value_object(event.parameters),
                    ..empty_message(MessageType::ToolUse)
                },
            );
        }
        "tool_result" => send_message(
            messages,
            Message {
                call_id: event.tool_id,
                output: event.output,
                ..empty_message(MessageType::ToolResult)
            },
        ),
        "result" => {
            state.saw_result = true;
            state.result_is_error = event.is_error || event.subtype == "error";
            state.result_text = value_text(&event.result);
            if state.result_is_error {
                state.result_error = cursor_error_text(&event).unwrap_or_default();
                if state.result_error.is_empty() {
                    state.result_error =
                        "cursor-agent returned an error result without details".to_string();
                }
            }
            if !state.result_text.is_empty() && state.output.is_empty() {
                state.output = state.result_text.clone();
                send_message(
                    messages,
                    Message {
                        content: state.result_text.clone(),
                        ..empty_message(MessageType::Text)
                    },
                );
            }
            accumulate_result_usage(state, &event);
            return true;
        }
        "error" => {
            if let Some(error) = cursor_error_text(&event) {
                state.protocol_error = error.clone();
                send_message(
                    messages,
                    Message {
                        content: error,
                        ..empty_message(MessageType::Error)
                    },
                );
            }
        }
        "text" => {
            if let Ok(part) = serde_json::from_value::<CursorTextPart>(event.part) {
                if !part.text.is_empty() {
                    state.output.push_str(&part.text);
                    state.assistant_bytes = state.assistant_bytes.saturating_add(part.text.len());
                    send_message(
                        messages,
                        Message {
                            content: part.text,
                            ..empty_message(MessageType::Text)
                        },
                    );
                }
            }
        }
        "step_finish" => {
            if let Ok(part) = serde_json::from_value::<CursorStepFinishPart>(event.part) {
                let model = cursor_usage_model(&event.model, &state.configured_model);
                let entry = state.step_usage.entry(model).or_default();
                entry.input_tokens = entry.input_tokens.saturating_add(part.tokens.input);
                entry.output_tokens = entry.output_tokens.saturating_add(part.tokens.output);
                entry.cache_read_tokens = entry
                    .cache_read_tokens
                    .saturating_add(part.tokens.cache.read);
            }
        }
        other => {
            if !is_non_agent_event(other) {
                state.unhandled_types.observe(other);
            }
        }
    }
    false
}

fn handle_assistant(raw: &Value, messages: &mpsc::Sender<Message>, state: &mut CursorStreamState) {
    let Ok(payload) = serde_json::from_value::<CursorAssistantMessage>(raw.clone()) else {
        return;
    };
    for block in payload.content {
        match block.block_type.as_str() {
            "output_text" | "text" if !block.text.is_empty() => {
                state.output.push_str(&block.text);
                state.assistant_bytes = state.assistant_bytes.saturating_add(block.text.len());
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
                state.tool_use_count = state.tool_use_count.saturating_add(1);
                send_message(
                    messages,
                    Message {
                        tool: block.name,
                        call_id: block.id,
                        input: value_object(block.input),
                        ..empty_message(MessageType::ToolUse)
                    },
                );
            }
            "output_text" | "text" | "thinking" => {}
            _ => {}
        }
    }
}

fn accumulate_result_usage(state: &mut CursorStreamState, event: &CursorEvent) {
    let model = cursor_usage_model(&event.model, &state.configured_model);
    let entry = state.result_usage.entry(model).or_default();
    let has_top_level = event.input_tokens != 0
        || event.output_tokens != 0
        || event.cache_read_tokens != 0
        || event.cache_write_tokens != 0;
    if has_top_level {
        entry.input_tokens = entry.input_tokens.saturating_add(event.input_tokens);
        entry.output_tokens = entry.output_tokens.saturating_add(event.output_tokens);
        entry.cache_read_tokens = entry
            .cache_read_tokens
            .saturating_add(event.cache_read_tokens);
        entry.cache_write_tokens = entry
            .cache_write_tokens
            .saturating_add(event.cache_write_tokens);
        state.has_result_usage = true;
    } else if let Some(usage) = event.usage {
        entry.input_tokens = entry.input_tokens.saturating_add(usage.input_tokens);
        entry.output_tokens = entry.output_tokens.saturating_add(usage.output_tokens);
        entry.cache_read_tokens = entry
            .cache_read_tokens
            .saturating_add(usage.cache_read_input_tokens);
        entry.cache_write_tokens = entry
            .cache_write_tokens
            .saturating_add(usage.cache_write_input_tokens);
        state.has_result_usage = true;
    }
}

fn finalize_cursor(
    run_end: RunEnd,
    timeout: Duration,
    state: &mut CursorStreamState,
    write_error: Option<&str>,
    exit_error: Option<&str>,
    exit_code: i32,
) -> (String, String) {
    let mut status = "completed".to_string();
    let mut error = String::new();
    if state.result_is_error {
        status = "failed".to_string();
        error = if state.result_error.is_empty() {
            "cursor-agent returned an error result without details".to_string()
        } else {
            state.result_error.clone()
        };
    } else if state.saw_result {
        // A terminal success result is authoritative even if the CLI worker
        // remains alive and has to be killed after the protocol boundary.
    } else {
        match run_end {
            RunEnd::TimedOut => {
                status = "timeout".to_string();
                error = format!("cursor-agent timed out after {timeout:?}");
            }
            RunEnd::Cancelled => {
                status = "aborted".to_string();
                error = "execution cancelled".to_string();
            }
            RunEnd::Completed => {
                if !state.scan_error.is_empty() {
                    status = "failed".to_string();
                    error = format!("cursor-agent stdout read error: {}", state.scan_error);
                } else if !state.protocol_error.is_empty() {
                    status = "failed".to_string();
                    error = state.protocol_error.clone();
                } else if let Some(write_error) = write_error {
                    status = "failed".to_string();
                    error = format!("cursor-agent prompt write failed: {write_error}");
                } else if let Some(exit_error) = exit_error {
                    status = "failed".to_string();
                    error = format!("cursor-agent exited with error: {exit_error}");
                } else {
                    status = "failed".to_string();
                    error = "cursor-agent stream ended without terminal result".to_string();
                }
                error = format!(
                    "{error} (result_seen=false, exit_code={}, scanner_error={}, event_count={}, invalid_event_count={}, last_event_type={}); {}",
                    exit_code,
                    !state.scan_error.is_empty(),
                    state.event_count,
                    state.invalid_event_count,
                    state.last_event_type,
                    "actions completed before finalization may already have taken effect"
                );
            }
        }
    }
    state.final_status = status.clone();
    (status, error)
}

fn cursor_result_output(state: &CursorStreamState) -> String {
    if state.saw_result && state.result_is_error {
        return String::new();
    }
    if state.final_status == "completed" {
        state.output.clone()
    } else {
        String::new()
    }
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

fn process_exit_code(exit: Result<&ExitStatus, &io::Error>) -> i32 {
    match exit {
        Ok(status) => status.code().unwrap_or(-1),
        Err(_) => -1,
    }
}

fn join_failure_state(error: JoinError) -> CursorStreamState {
    CursorStreamState {
        scan_error: format!("stream task failed: {error}"),
        ..CursorStreamState::default()
    }
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

#[derive(Debug)]
enum RunOutcome {
    Completed(Box<CursorCompletion>),
    Cancelled,
    TimedOut,
}

#[derive(Debug)]
enum CursorCompletion {
    Stream(Result<CursorStreamState, JoinError>),
    Process(io::Result<ExitStatus>, Result<CursorStreamState, JoinError>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunEnd {
    Completed,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Deserialize)]
struct CursorEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    subtype: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    message: Value,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    tool_call: Value,
    #[serde(default)]
    call_id: String,
    #[serde(default)]
    tool_name: String,
    #[serde(default)]
    tool_id: String,
    #[serde(default)]
    parameters: Value,
    #[serde(default)]
    output: String,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    is_error: bool,
    #[serde(default, alias = "inputTokens")]
    input_tokens: i64,
    #[serde(default, alias = "outputTokens")]
    output_tokens: i64,
    #[serde(default, alias = "cacheReadTokens")]
    cache_read_tokens: i64,
    #[serde(default, alias = "cacheWriteTokens")]
    cache_write_tokens: i64,
    #[serde(default)]
    usage: Option<CursorUsage>,
    #[serde(default)]
    error: Value,
    #[serde(default)]
    detail: Value,
    #[serde(default)]
    part: Value,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct CursorUsage {
    #[serde(default, alias = "inputTokens")]
    input_tokens: i64,
    #[serde(default, alias = "outputTokens")]
    output_tokens: i64,
    #[serde(
        default,
        alias = "cachedInputTokens",
        alias = "cached_input_tokens",
        alias = "cacheReadTokens",
        alias = "cacheReadInputTokens",
        alias = "cache_read_input_tokens"
    )]
    cache_read_input_tokens: i64,
    #[serde(
        default,
        alias = "cacheWriteTokens",
        alias = "cacheCreationInputTokens",
        alias = "cache_creation_input_tokens"
    )]
    cache_write_input_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct CursorAssistantMessage {
    #[serde(default)]
    content: Vec<CursorContentBlock>,
}

#[derive(Debug, Deserialize)]
struct CursorContentBlock {
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
}

#[derive(Debug, Deserialize)]
struct CursorTextPart {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Default, Deserialize)]
struct CursorStepFinishPart {
    #[serde(default)]
    tokens: CursorStepTokens,
}

#[derive(Debug, Default, Deserialize)]
struct CursorStepTokens {
    #[serde(default)]
    input: i64,
    #[serde(default)]
    output: i64,
    #[serde(default)]
    cache: CursorStepCache,
}

#[derive(Debug, Default, Deserialize)]
struct CursorStepCache {
    #[serde(default)]
    read: i64,
}

#[derive(Debug, Default)]
struct CursorStreamState {
    configured_model: String,
    session_id: String,
    output: String,
    result_text: String,
    result_error: String,
    protocol_error: String,
    scan_error: String,
    final_status: String,
    saw_result: bool,
    result_is_error: bool,
    has_result_usage: bool,
    result_usage: BTreeMap<String, TokenUsage>,
    step_usage: BTreeMap<String, TokenUsage>,
    event_count: usize,
    invalid_event_count: usize,
    assistant_event_count: usize,
    assistant_bytes: usize,
    tool_use_count: usize,
    unhandled_subtype_count: usize,
    last_event_type: String,
    thinking: CursorThinkingStream,
    unhandled_types: CursorUnhandledTypeTally,
}

impl CursorStreamState {
    fn new(configured_model: String) -> Self {
        Self {
            configured_model,
            last_event_type: "none".to_string(),
            ..Self::default()
        }
    }
}

#[derive(Debug, Default)]
struct CursorThinkingStream {
    block_open: bool,
    any_sent: bool,
}

impl CursorThinkingStream {
    fn delta(&mut self, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        let text = if !self.block_open && self.any_sent {
            format!("\n\n{text}")
        } else {
            text.to_string()
        };
        self.block_open = true;
        self.any_sent = true;
        text
    }

    fn complete(&mut self) {
        self.block_open = false;
    }
}

#[derive(Debug, Default)]
struct CursorUnhandledTypeTally {
    total: usize,
    counts: BTreeMap<String, usize>,
}

impl CursorUnhandledTypeTally {
    fn observe(&mut self, value: &str) {
        self.total = self.total.saturating_add(1);
        let key = observed_event_type(value);
        let key = if !self.counts.contains_key(&key) && self.counts.len() >= 16 {
            "(overflow)".to_string()
        } else {
            key
        };
        *self.counts.entry(key).or_default() += 1;
    }

    fn summary(&self) -> String {
        self.counts
            .iter()
            .map(|(event_type, count)| format!("{event_type}:{count}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn normalize_cursor_stream_line(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let lower = trimmed.to_ascii_lowercase();
    for prefix in ["stdout:", "stdout=", "stderr:", "stderr="] {
        if lower.starts_with(prefix) {
            return trimmed[prefix.len()..].trim().to_string();
        }
    }
    trimmed.to_string()
}

fn observed_event_type(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return "unknown".to_string();
    }
    if value.len() > 64
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return "invalid".to_string();
    }
    value.to_string()
}

fn is_non_agent_event(event_type: &str) -> bool {
    matches!(event_type.trim(), "user" | "connection" | "retry")
}

fn parse_tool_call(event: &CursorEvent) -> CursorToolCall {
    let mut call = CursorToolCall {
        call_id: cursor_call_id(&event.call_id),
        ..CursorToolCall::default()
    };
    let Some(envelope) = event.tool_call.as_object() else {
        return call;
    };
    if call.call_id.is_empty() {
        if let Some(value) = envelope.get("toolCallId").and_then(Value::as_str) {
            call.call_id = cursor_call_id(value);
        }
    }
    let Some(key) = envelope
        .keys()
        .filter(|key| key.len() > "ToolCall".len() && key.ends_with("ToolCall"))
        .min()
        .cloned()
    else {
        return call;
    };
    call.name = key.trim_end_matches("ToolCall").to_string();
    let Some(payload) = envelope.get(&key).and_then(Value::as_object) else {
        return call;
    };
    if let Some(args) = payload.get("args") {
        call.input = value_object(args.clone());
    }
    if let Some(result) = payload.get("result") {
        call.result = result.to_string();
    }
    call
}

#[derive(Debug, Default)]
struct CursorToolCall {
    name: String,
    call_id: String,
    input: BTreeMap<String, Value>,
    result: String,
}

fn cursor_call_id(raw: &str) -> String {
    raw.trim()
        .split('\n')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn cursor_usage_model(event_model: &str, configured_model: &str) -> String {
    if !event_model.trim().is_empty() {
        event_model.trim().to_string()
    } else if !configured_model.trim().is_empty() {
        configured_model.trim().to_string()
    } else {
        "cursor".to_string()
    }
}

fn value_object(value: Value) -> BTreeMap<String, Value> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        value => value.to_string(),
    }
}

fn cursor_error_text(event: &CursorEvent) -> Option<String> {
    for value in [&event.error, &event.detail, &event.result] {
        let text = value_text(value);
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

fn static_catalog() -> Catalog {
    Catalog {
        models: vec![Model {
            id: "auto".to_string(),
            label: "Auto".to_string(),
            provider: "cursor".to_string(),
            default: true,
            ..Model::default()
        }],
        fallback: true,
    }
}

fn parse_cursor_models(output: &str) -> Vec<Model> {
    let mut seen = BTreeSet::new();
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (id, label) = line.split_once(" - ")?;
            let id = id.trim();
            let mut label = label.trim().to_string();
            if !is_cursor_identifier(id) || !seen.insert(id.to_string()) {
                return None;
            }
            let is_default = label.contains("default");
            if let Some(index) = label.find('(') {
                if index > 0 {
                    label.truncate(index);
                    label = label.trim().to_string();
                }
            }
            if label.is_empty() {
                label = id.to_string();
            }
            Some(Model {
                id: id.to_string(),
                label,
                provider: "cursor".to_string(),
                default: is_default,
                ..Model::default()
            })
        })
        .collect()
}

fn is_cursor_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && !value.ends_with(':')
        && chars.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '/')
        })
}

async fn capture_model_list(
    command: &RuntimeCommand,
    env: &BTreeMap<String, String>,
    cancellation: CancellationToken,
    timeout: Duration,
) -> Option<String> {
    let path = command_path(command);
    let args = command.argv(&["--list-models".to_string()]);
    let (path, args) = platform_invocation(path, args);
    let mut child = Command::new(&path);
    child
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(false);
    configure_child_environment(&mut child, env);
    let mut tree = OwnedProcessTree::spawn(&mut child).await.ok()?;
    let stdout = match tree.child_mut().stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            return None;
        }
    };
    let mut output_task = tokio::spawn(read_bounded(stdout, DISCOVERY_OUTPUT_MAX));
    let outcome = {
        let completion = async {
            let output = (&mut output_task).await;
            let exit = tree.wait().await;
            (output, exit)
        };
        tokio::pin!(completion);
        tokio::select! {
            value = &mut completion => Some(value),
            () = cancellation.cancelled() => None,
            () = tokio::time::sleep(timeout) => None,
        }
    };
    let Some((output, _exit)) = outcome else {
        output_task.abort();
        let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
        return None;
    };
    let output = output.ok()?.ok()?;
    String::from_utf8(output).ok()
}

async fn read_bounded(mut stdout: ChildStdout, maximum: usize) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let bytes = stdout.read(&mut buffer).await?;
        if bytes == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(bytes) > maximum {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Cursor model list exceeded output limit",
            ));
        }
        output.extend_from_slice(&buffer[..bytes]);
    }
}

#[cfg(windows)]
fn platform_invocation(path: String, args: Vec<String>) -> (String, Vec<String>) {
    let path = locate_cursor_command(Path::new(&path)).unwrap_or_else(|| PathBuf::from(path));
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !extension.eq_ignore_ascii_case("cmd") && !extension.eq_ignore_ascii_case("bat") {
        return (path.to_string_lossy().into_owned(), args);
    }
    let Some(ps1) = path
        .parent()
        .map(|parent| parent.join("cursor-agent.ps1"))
        .filter(|candidate| candidate.is_file())
    else {
        return (path.to_string_lossy().into_owned(), args);
    };
    let Some(powershell) = find_powershell() else {
        return (path.to_string_lossy().into_owned(), args);
    };
    let mut rewritten = vec![
        "-NoProfile".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        ps1.to_string_lossy().into_owned(),
    ];
    rewritten.extend(args);
    (powershell, rewritten)
}

#[cfg(not(windows))]
fn platform_invocation(path: String, args: Vec<String>) -> (String, Vec<String>) {
    (path, args)
}

#[cfg(windows)]
fn locate_cursor_command(command: &Path) -> Option<PathBuf> {
    if command.is_absolute() || command.components().count() > 1 {
        return command.is_file().then(|| command.to_path_buf());
    }
    let mut names = vec![command.to_path_buf()];
    if command.extension().is_none() {
        names.extend([
            command.with_extension("com"),
            command.with_extension("exe"),
            command.with_extension("bat"),
            command.with_extension("cmd"),
        ]);
    }
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        for name in &names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn find_powershell() -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        for name in ["pwsh.exe", "powershell.exe"] {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let candidate = root
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    candidate
        .is_file()
        .then(|| candidate.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn build_cursor_args_owns_protocol_and_user_options() {
        let options = ExecOptions {
            cwd: "/workspace".to_string(),
            model: "gpt-5".to_string(),
            resume_session_id: "session-42".to_string(),
            custom_args: vec![
                "--output-format=text".to_string(),
                "--yolo".to_string(),
                "--verbose".to_string(),
            ],
            ..ExecOptions::default()
        };

        assert_eq!(
            build_cursor_args(&options),
            vec![
                "-p",
                "--output-format",
                "stream-json",
                "--yolo",
                "--workspace",
                "/workspace",
                "--model",
                "gpt-5",
                "--resume",
                "session-42",
                "--verbose",
            ]
        );
    }

    #[test]
    fn normalize_and_parse_cursor_models_preserve_contract() {
        assert_eq!(
            normalize_cursor_stream_line("  stdout:  {\"type\":\"text\"}  "),
            r#"{"type":"text"}"#
        );
        assert_eq!(
            normalize_cursor_stream_line(" stderr=  warning "),
            "warning"
        );
        assert_eq!(normalize_cursor_stream_line("  \t"), "");

        let models = parse_cursor_models(
            "gpt-5 - GPT-5 (default)\n\
             gpt-5 - duplicate\n\
             claude-sonnet - Claude Sonnet\n\
             invalid id - Invalid\n\
             bad: - Invalid\n",
        );
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-5");
        assert_eq!(models[0].label, "GPT-5");
        assert!(models[0].default);
        assert_eq!(models[1].id, "claude-sonnet");
        assert!(!models[1].default);

        let fallback = static_catalog();
        assert!(fallback.fallback);
        assert_eq!(fallback.models.len(), 1);
        assert_eq!(fallback.models[0].id, "auto");
        assert!(fallback.models[0].default);
    }

    #[test]
    fn parse_tool_call_normalizes_call_id_and_payload() {
        let event: CursorEvent = serde_json::from_value(json!({
            "type": "tool_call",
            "call_id": "  call-42\nignored",
            "tool_call": {
                "shellToolCall": {
                    "args": {"command": "pwd"},
                    "result": {"exitCode": 0}
                }
            }
        }))
        .unwrap_or_else(|error| panic!("valid tool call event: {error}"));

        let call = parse_tool_call(&event);
        assert_eq!(call.call_id, "call-42");
        assert_eq!(call.name, "shell");
        assert_eq!(call.input.get("command"), Some(&json!("pwd")));
        assert_eq!(call.result, r#"{"exitCode":0}"#);
    }

    #[test]
    fn finalize_hides_partial_output_without_result_and_prefers_success_result() {
        let mut partial = CursorStreamState {
            output: "partial answer".to_string(),
            protocol_error: "protocol drift".to_string(),
            ..CursorStreamState::default()
        };
        let (status, error) = finalize_cursor(
            RunEnd::Completed,
            Duration::from_secs(1),
            &mut partial,
            Some("broken pipe"),
            Some("exit status: 9"),
            9,
        );
        assert_eq!(status, "failed");
        assert!(error.starts_with("protocol drift"));
        assert!(!error.contains("prompt write failed"));
        assert!(!error.contains("exited with error"));
        assert!(error.contains("exit_code=9"));
        assert_eq!(cursor_result_output(&partial), "");

        let mut success = CursorStreamState {
            output: "assistant answer".to_string(),
            result_text: "terminal result".to_string(),
            saw_result: true,
            ..CursorStreamState::default()
        };
        let (status, error) = finalize_cursor(
            RunEnd::TimedOut,
            Duration::from_secs(1),
            &mut success,
            Some("broken pipe"),
            Some("exit status: 9"),
            9,
        );
        assert_eq!(status, "completed");
        assert!(error.is_empty());
        assert_eq!(cursor_result_output(&success), "assistant answer");

        let mut structured_error = CursorStreamState {
            output: "partial provider output".to_string(),
            result_error: "provider rejected request".to_string(),
            saw_result: true,
            result_is_error: true,
            ..CursorStreamState::default()
        };
        let (status, error) = finalize_cursor(
            RunEnd::TimedOut,
            Duration::from_secs(1),
            &mut structured_error,
            Some("broken pipe"),
            Some("exit status: 17"),
            17,
        );
        assert_eq!(status, "failed");
        assert_eq!(error, "provider rejected request");
        assert_eq!(cursor_result_output(&structured_error), "");
    }
}
