//! OpenCode's headless JSON-lines adapter.
//!
//! OpenCode and DevEco share an event vocabulary, but OpenCode has its own
//! production contract: the prompt is written to stdin, model variants are
//! selected with `--variant`, model discovery includes verbose variant JSON,
//! and managed MCP is projected through `OPENCODE_CONFIG_CONTENT`.

use std::collections::BTreeMap;
use std::io;
#[cfg(windows)]
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::command::{filter_custom_args, filter_launch_prefix, BlockedArgMode, RuntimeCommand};
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
use crate::model::{
    Catalog, CatalogCache, Model, ModelDiscoveryCacheKey, ModelThinking, ThinkingLevel,
};
use crate::opencode_mcp::build_opencode_mcp_config_content;
use crate::process::OwnedProcessTree;
use crate::stderr::SharedDiagnosticBuffer;
use crate::stream::AgentLineReader;

const MESSAGE_BUFFER: usize = 256;
const TERMINATION_GRACE: Duration = Duration::from_secs(5);
const KILL_GRACE: Duration = Duration::from_secs(10);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVERY_OUTPUT_MAX: u64 = 4 * 1024 * 1024;
const DEFAULT_TAIL_BYTES: usize = 16 * 1024;

pub(crate) static BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> =
    LazyLock::new(|| {
        BTreeMap::from([
            ("--format", BlockedArgMode::WithValue),
            ("--dir", BlockedArgMode::WithValue),
            ("--variant", BlockedArgMode::WithValue),
            ("--dangerously-skip-permissions", BlockedArgMode::Standalone),
        ])
    });

#[derive(Debug, Clone, Default)]
pub struct OpencodeConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct OpencodeBackend {
    config: OpencodeConfig,
}

impl OpencodeBackend {
    pub fn new(config: OpencodeConfig) -> Self {
        Self { config }
    }

    pub async fn discover_models(
        &self,
        cache: &CatalogCache,
        cancellation: tokio_util::sync::CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        self.discover_models_for_runtime("opencode", cache, cancellation, timeout)
            .await
    }

    pub async fn discover_models_for_runtime(
        &self,
        runtime_scope: &str,
        cache: &CatalogCache,
        cancellation: tokio_util::sync::CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        let scope = if runtime_scope.trim().is_empty() {
            "opencode"
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
            DISCOVERY_TIMEOUT
        } else {
            timeout
        };
        let deadline = Instant::now() + timeout;
        let remaining = || deadline.saturating_duration_since(Instant::now());
        let mut models = if remaining().is_zero() {
            Vec::new()
        } else {
            self.discover_models_command(
                &["models", "--verbose"],
                cancellation.clone(),
                remaining(),
            )
            .await
            .map(|output| parse_opencode_models(&output))
            .unwrap_or_default()
        };
        if models.is_empty() && !cancellation.is_cancelled() {
            let timeout = remaining();
            if !timeout.is_zero() {
                models = self
                    .discover_models_command(&["models"], cancellation.clone(), timeout)
                    .await
                    .map(|output| parse_opencode_models(&output))
                    .unwrap_or_default();
            }
        }
        if cancellation.is_cancelled() {
            return Catalog::default();
        }
        let catalog = Catalog {
            models,
            fallback: false,
        };
        if !catalog.models.is_empty() {
            let _ = cache.insert(key, catalog.clone());
        }
        catalog
    }

    async fn discover_models_command(
        &self,
        arguments: &[&str],
        cancellation: tokio_util::sync::CancellationToken,
        timeout: Duration,
    ) -> Option<String> {
        let command_path = resolve_opencode_command(if self.config.command.path.is_empty() {
            "opencode"
        } else {
            self.config.command.path.as_str()
        });
        let prefix = filter_launch_prefix(&self.config.command.prefix, &BLOCKED_ARGS);
        let mut command = Command::new(&command_path);
        command
            .args(prefix.args)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .envs(&self.config.env)
            .kill_on_drop(false);
        let mut tree = OwnedProcessTree::spawn(&mut command).await.ok()?;
        let stdout = tree.child_mut().stdout.take()?;
        let mut reader = tokio::spawn(async move {
            let mut output = Vec::new();
            let bytes = stdout
                .take(DISCOVERY_OUTPUT_MAX.saturating_add(1))
                .read_to_end(&mut output)
                .await?;
            Ok::<_, io::Error>((bytes, output))
        });
        let outcome = tokio::select! {
            status = tree.wait() => {
                let _ = status;
                true
            },
            () = cancellation.cancelled() => false,
            () = tokio::time::sleep(timeout) => false,
        };
        if !outcome {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
        }
        let output = tokio::time::timeout(KILL_GRACE, &mut reader)
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(Result::ok)
            .filter(|(bytes, _)| u64::try_from(*bytes).is_ok_and(|n| n <= DISCOVERY_OUTPUT_MAX))
            .map(|(_, output)| output);
        if !reader.is_finished() {
            reader.abort();
        }
        if !outcome {
            return None;
        }
        output.map(|output| String::from_utf8_lossy(&output).into_owned())
    }
}

pub fn build_opencode_args(options: &ExecOptions) -> Vec<String> {
    let mut args = vec![
        "run".to_string(),
        "--format".to_string(),
        "json".to_string(),
        "--dangerously-skip-permissions".to_string(),
    ];
    if !options.cwd.is_empty() {
        args.extend(["--dir".to_string(), options.cwd.clone()]);
    }
    if !options.model.is_empty() {
        args.extend(["--model".to_string(), options.model.clone()]);
    }
    if !options.thinking_level.is_empty() {
        args.extend(["--variant".to_string(), options.thinking_level.clone()]);
    }
    if !options.resume_session_id.is_empty() {
        args.extend(["--session".to_string(), options.resume_session_id.clone()]);
    }
    args.extend(filter_custom_args(&options.custom_args, &BLOCKED_ARGS).args);
    args
}

#[async_trait]
impl Backend for OpencodeBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        let mcp_content = build_opencode_mcp_config_content(options.mcp_config.as_ref())?;
        let command_path = resolve_opencode_command(if self.config.command.path.is_empty() {
            "opencode"
        } else {
            self.config.command.path.as_str()
        });
        let prefix = filter_launch_prefix(&self.config.command.prefix, &BLOCKED_ARGS);
        let mut argv = prefix.args;
        argv.extend(build_opencode_args(&options));

        let mut command = Command::new(&command_path);
        command
            .args(argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&self.config.env)
            .kill_on_drop(false);
        if !options.cwd.is_empty() {
            command.current_dir(&options.cwd).env("PWD", &options.cwd);
        }
        if let Some(content) = mcp_content {
            command.env("OPENCODE_CONFIG_CONTENT", content);
        }

        let mut tree = OwnedProcessTree::spawn(&mut command)
            .await
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    AgentError::ExecutableNotFound(command_path.clone())
                } else {
                    AgentError::Process(error)
                }
            })?;
        let Some(stdout) = tree.child_mut().stdout.take() else {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            return Err(AgentError::Protocol(
                "OpenCode stdout pipe unavailable after spawn".to_string(),
            ));
        };
        let Some(stdin) = tree.child_mut().stdin.take() else {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            return Err(AgentError::Protocol(
                "OpenCode stdin pipe unavailable after spawn".to_string(),
            ));
        };
        let Some(stderr) = tree.child_mut().stderr.take() else {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            return Err(AgentError::Protocol(
                "OpenCode stderr pipe unavailable after spawn".to_string(),
            ));
        };

        let (message_tx, message_rx) = mpsc::channel(MESSAGE_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();
        let events_stop = CancellationToken::new();
        let events_stop_for_task = events_stop.clone();
        let cancellation = options.cancellation.clone();
        let timeout = options.timeout;
        let configured_model = options.model.clone();
        let prompt = prompt.to_string();
        let started = Instant::now();
        let stderr_tail = SharedDiagnosticBuffer::new(DEFAULT_TAIL_BYTES);
        let stderr_reader_tail = stderr_tail.clone();
        let mut events_task = tokio::spawn(read_events(stdout, message_tx, events_stop_for_task));
        let mut stderr_task = tokio::spawn(pump_stderr(stderr, stderr_reader_tail));
        let mut writer_task = tokio::spawn(write_prompt(stdin, prompt));

        tokio::spawn(async move {
            let outcome = if timeout.is_zero() {
                tokio::select! {
                    status = tree.wait() => RunOutcome::Completed(status),
                    () = cancellation.cancelled() => RunOutcome::Cancelled,
                }
            } else {
                tokio::select! {
                    status = tree.wait() => RunOutcome::Completed(status),
                    () = cancellation.cancelled() => RunOutcome::Cancelled,
                    () = tokio::time::sleep(timeout) => RunOutcome::TimedOut,
                }
            };

            let (run_end, exit) = match outcome {
                RunOutcome::Completed(status) => (RunEnd::Completed, status),
                RunOutcome::Cancelled => {
                    writer_task.abort();
                    let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
                    events_stop.cancel();
                    (RunEnd::Cancelled, Ok(success_exit_status()))
                }
                RunOutcome::TimedOut => {
                    writer_task.abort();
                    let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
                    events_stop.cancel();
                    (RunEnd::DeadlineExceeded, Ok(success_exit_status()))
                }
            };

            let write_error = match tokio::time::timeout(KILL_GRACE, &mut writer_task).await {
                Ok(Ok(result)) => result.err(),
                Ok(Err(error)) => Some(io::Error::other(format!(
                    "prompt writer task failed: {error}"
                ))),
                Err(_) => {
                    writer_task.abort();
                    Some(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "prompt writer did not terminate",
                    ))
                }
            };
            let state = join_events(&mut events_task).await;
            if tokio::time::timeout(KILL_GRACE, &mut stderr_task)
                .await
                .is_err()
            {
                stderr_task.abort();
            }
            let stderr = stderr_tail.tail();
            if !stderr.is_empty() {
                tracing::debug!(provider = "opencode", %stderr, "agent stderr captured");
            }

            let mut status = state.status;
            let mut error = state.error;
            if matches!(run_end, RunEnd::DeadlineExceeded) {
                status = "timeout".to_string();
                error = format!("opencode timed out after {}", format_duration(timeout));
            } else if matches!(run_end, RunEnd::Cancelled) {
                status = "aborted".to_string();
                error = "execution cancelled".to_string();
            } else if let Some(exit_error) = exit_error(exit.as_ref()) {
                if status == "completed" {
                    status = "failed".to_string();
                    error = format!("opencode exited with error: {exit_error}");
                } else if state.no_terminal_signal {
                    error = format!("{error}; opencode exited with error: {exit_error}");
                }
            }
            if let Some(write_error) = write_error
                .filter(|_| matches!(run_end, RunEnd::Completed) && !state.saw_terminal_signal)
            {
                let message = format!("opencode prompt write failed: {write_error}");
                error = if error.is_empty() {
                    message
                } else {
                    format!("{error}; {message}")
                };
                status = "failed".to_string();
            }

            let mut usage = BTreeMap::new();
            if state.usage.input_tokens > 0
                || state.usage.output_tokens > 0
                || state.usage.cache_read_tokens > 0
                || state.usage.cache_write_tokens > 0
            {
                let model = if configured_model.is_empty() {
                    "unknown".to_string()
                } else {
                    configured_model
                };
                usage.insert(model, state.usage);
            }
            let _ = result_tx.send(ExecutionResult {
                status,
                output: state.output,
                error,
                duration_ms: started.elapsed().as_millis().try_into().unwrap_or(i64::MAX),
                session_id: state.session_id,
                usage,
                resume_rejected: false,
            });
        });

        Ok(Session {
            messages: message_rx,
            result: result_rx,
        })
    }
}

/// Resolve the npm `.cmd` shim to OpenCode's bundled native binary on Windows.
///
/// `cmd.exe` argument forwarding truncates literal newlines, which are common
/// in Cordy's stdin prompt. The Go adapter avoids that loss by bypassing the
/// shim; keep the same behavior here when a pinned launch path points at the
/// npm wrapper.
fn resolve_opencode_command(path: &str) -> String {
    #[cfg(windows)]
    {
        if !path.to_ascii_lowercase().ends_with(".cmd") {
            return path.to_string();
        }
        let shim = locate_opencode_shim(path);
        let Some(prefix) = shim.as_deref().and_then(Path::parent) else {
            return path.to_string();
        };
        for package in opencode_windows_package_candidates() {
            let candidate = prefix
                .join("node_modules")
                .join("opencode-ai")
                .join("node_modules")
                .join(package)
                .join("bin")
                .join("opencode.exe");
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    path.to_string()
}

#[cfg(windows)]
fn locate_opencode_shim(path: &str) -> Option<std::path::PathBuf> {
    let candidate = Path::new(path);
    if candidate.is_absolute() || candidate.components().count() > 1 {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(candidate))
        .find(|candidate| candidate.is_file())
}

#[cfg(windows)]
fn opencode_windows_package_candidates() -> [&'static str; 3] {
    if std::env::consts::ARCH == "aarch64" {
        [
            "opencode-windows-arm64",
            "opencode-windows-x64",
            "opencode-windows-x64-baseline",
        ]
    } else {
        [
            "opencode-windows-x64",
            "opencode-windows-x64-baseline",
            "opencode-windows-arm64",
        ]
    }
}

async fn write_prompt(mut stdin: ChildStdin, prompt: String) -> io::Result<()> {
    stdin.write_all(prompt.as_bytes()).await
}

#[derive(Debug, Clone, Copy)]
enum RunEnd {
    Completed,
    DeadlineExceeded,
    Cancelled,
}

enum RunOutcome {
    Completed(io::Result<ExitStatus>),
    Cancelled,
    TimedOut,
}

#[derive(Debug, Default)]
struct OpencodeEventResult {
    status: String,
    error: String,
    output: String,
    session_id: String,
    usage: TokenUsage,
    no_terminal_signal: bool,
    saw_terminal_signal: bool,
}

async fn read_events(
    stdout: ChildStdout,
    messages: mpsc::Sender<Message>,
    stop: CancellationToken,
) -> OpencodeEventResult {
    let mut reader = AgentLineReader::new(BufReader::new(stdout));
    let mut state = OpencodeEventResult {
        status: "completed".to_string(),
        ..OpencodeEventResult::default()
    };
    let mut open_step = false;
    let mut step_has_continuation_tool = false;
    let mut awaiting_continuation = false;
    let mut saw_step_finish = false;
    let mut step_produced_output = false;
    let mut last_step_void = false;

    loop {
        let line = match tokio::select! {
            line = reader.next_line() => line,
            _ = stop.cancelled() => break,
        } {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => {
                if state.status == "completed" {
                    state.status = "failed".to_string();
                    state.error = format!("stdout read error: {error}");
                }
                return state;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<OpencodeEvent>(line) else {
            continue;
        };
        if !event.session_id.is_empty() {
            state.session_id = event.session_id.clone();
        }
        match event.event_type.as_str() {
            "text" => {
                if !event.part.text.is_empty() {
                    step_produced_output = true;
                    state.output.push_str(&event.part.text);
                    send_message(
                        &messages,
                        Message {
                            content: event.part.text,
                            ..empty_message(MessageType::Text, "")
                        },
                    );
                }
            }
            "tool_use" => {
                step_produced_output = true;
                if event
                    .part
                    .metadata
                    .as_ref()
                    .is_none_or(|metadata| !metadata.provider_executed)
                {
                    step_has_continuation_tool = true;
                }
                handle_tool_event(&event.part, &messages);
            }
            "error" => {
                let error = event
                    .error
                    .as_ref()
                    .map(OpencodeError::message)
                    .filter(|message| !message.is_empty())
                    .unwrap_or_else(|| "unknown opencode error".to_string());
                send_message(
                    &messages,
                    Message {
                        content: error.clone(),
                        ..empty_message(MessageType::Error, "")
                    },
                );
                state.status = "failed".to_string();
                state.error = error;
            }
            "step_start" => {
                open_step = true;
                step_has_continuation_tool = false;
                awaiting_continuation = false;
                step_produced_output = false;
                send_message(&messages, empty_message(MessageType::Status, "running"));
            }
            "step_finish" => {
                open_step = false;
                saw_step_finish = true;
                awaiting_continuation = event.part.reason == "tool-calls"
                    || (!event.part.reason.is_empty() && step_has_continuation_tool);
                step_has_continuation_tool = false;
                if let Some(tokens) = event.part.tokens.as_ref() {
                    state.usage.input_tokens += tokens.input;
                    state.usage.output_tokens += tokens.output;
                    if let Some(cache) = tokens.cache.as_ref() {
                        state.usage.cache_read_tokens += cache.read;
                        state.usage.cache_write_tokens += cache.write;
                    }
                }
                last_step_void = !step_produced_output && !step_reported_usage(&event.part);
                if step_reported_usage(&event.part) {
                    step_produced_output = true;
                }
            }
            _ => {}
        }
    }

    if state.status == "completed" {
        if open_step {
            state.status = "failed".to_string();
            state.error =
                "opencode stream ended without a terminal signal (step still open at EOF)"
                    .to_string();
            state.no_terminal_signal = true;
        } else if awaiting_continuation {
            state.status = "failed".to_string();
            state.error = "opencode stream ended without a terminal signal (last step required a continuation that never started)".to_string();
            state.no_terminal_signal = true;
        } else if last_step_void {
            state.status = "failed".to_string();
            state.error = "opencode stream ended on an empty step (no text, no tool call, no reported usage) — the provider produced nothing".to_string();
            state.no_terminal_signal = true;
        }
    }
    state.saw_terminal_signal = saw_step_finish && !state.no_terminal_signal;
    state
}

fn step_reported_usage(part: &OpencodePart) -> bool {
    if part.cost > 0.0 {
        return true;
    }
    let Some(tokens) = part.tokens.as_ref() else {
        return false;
    };
    tokens.input > 0
        || tokens.output > 0
        || tokens.reasoning > 0
        || tokens.total > 0
        || tokens
            .cache
            .as_ref()
            .is_some_and(|cache| cache.read > 0 || cache.write > 0)
}

fn handle_tool_event(part: &OpencodePart, messages: &mpsc::Sender<Message>) {
    let input = part
        .state
        .as_ref()
        .and_then(|state| state.input.as_object())
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
            tool: part.tool.clone(),
            call_id: part.call_id.clone(),
            input,
            ..empty_message(MessageType::ToolUse, "")
        },
    );
    if let Some(state) = part.state.as_ref() {
        if matches!(state.status.as_str(), "completed" | "error") {
            let output = if state.status == "error" && !state.error.is_empty() {
                state.error.clone()
            } else {
                extract_tool_output(&state.output)
            };
            send_message(
                messages,
                Message {
                    tool: part.tool.clone(),
                    call_id: part.call_id.clone(),
                    output,
                    ..empty_message(MessageType::ToolResult, "")
                },
            );
        }
    }
}

fn extract_tool_output(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        value => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn empty_message(message_type: MessageType, status: &str) -> Message {
    Message {
        message_type,
        content: String::new(),
        tool: String::new(),
        call_id: String::new(),
        input: BTreeMap::new(),
        output: String::new(),
        status: status.to_string(),
        level: String::new(),
        session_id: String::new(),
    }
}

fn send_message(messages: &mpsc::Sender<Message>, message: Message) {
    let _ = messages.try_send(message);
}

async fn join_events(task: &mut JoinHandle<OpencodeEventResult>) -> OpencodeEventResult {
    match tokio::time::timeout(KILL_GRACE, &mut *task).await {
        Ok(Ok(state)) => state,
        Ok(Err(error)) => OpencodeEventResult {
            status: "failed".to_string(),
            error: format!("event stream task failed: {error}"),
            ..OpencodeEventResult::default()
        },
        Err(_) => {
            task.abort();
            OpencodeEventResult {
                status: "failed".to_string(),
                error: "opencode event stream did not terminate".to_string(),
                ..OpencodeEventResult::default()
            }
        }
    }
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

fn exit_error(exit: Result<&ExitStatus, &io::Error>) -> Option<String> {
    match exit {
        Ok(status) if status.success() => None,
        Ok(status) => Some(status.to_string()),
        Err(error) => Some(format!("wait failed: {error}")),
    }
}

fn format_duration(timeout: Duration) -> String {
    if timeout.is_zero() {
        return "0s".to_string();
    }
    format!("{}s", timeout.as_secs_f64())
}

fn success_exit_status() -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(0)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(0)
    }
}

#[derive(Debug, Deserialize)]
struct OpencodeEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(rename = "sessionID", default)]
    session_id: String,
    #[serde(default)]
    part: OpencodePart,
    #[serde(default)]
    error: Option<OpencodeError>,
}

#[derive(Debug, Default, Deserialize)]
struct OpencodePart {
    #[serde(default)]
    text: String,
    #[serde(default)]
    tool: String,
    #[serde(rename = "callID", default)]
    call_id: String,
    #[serde(default)]
    state: Option<OpencodeToolState>,
    #[serde(default)]
    metadata: Option<OpencodeMetadata>,
    #[serde(default)]
    tokens: Option<OpencodeTokens>,
    #[serde(default)]
    cost: f64,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Deserialize)]
struct OpencodeMetadata {
    #[serde(rename = "providerExecuted", default)]
    provider_executed: bool,
}

#[derive(Debug, Deserialize)]
struct OpencodeTokens {
    #[serde(default)]
    input: i64,
    #[serde(default)]
    output: i64,
    #[serde(default)]
    reasoning: i64,
    #[serde(default)]
    total: i64,
    #[serde(default)]
    cache: Option<OpencodeCacheTokens>,
}

#[derive(Debug, Deserialize)]
struct OpencodeCacheTokens {
    #[serde(default)]
    read: i64,
    #[serde(default)]
    write: i64,
}

#[derive(Debug, Deserialize)]
struct OpencodeToolState {
    #[serde(default)]
    status: String,
    #[serde(default)]
    input: Value,
    #[serde(default)]
    output: Value,
    #[serde(default)]
    error: String,
}

#[derive(Debug, Deserialize)]
struct OpencodeError {
    #[serde(default)]
    name: String,
    #[serde(default)]
    data: Option<OpencodeErrorData>,
}

#[derive(Debug, Deserialize)]
struct OpencodeErrorData {
    #[serde(default)]
    message: String,
}

impl OpencodeError {
    fn message(&self) -> String {
        self.data
            .as_ref()
            .map(|data| data.message.clone())
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| self.name.clone())
    }
}

pub fn parse_opencode_models(output: &str) -> Vec<Model> {
    let lines: Vec<_> = output.lines().collect();
    let mut models = Vec::new();
    let mut index_by_id = BTreeMap::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        let Some(id) = parse_opencode_model_id_line(line) else {
            index += 1;
            continue;
        };
        let model_index = if let Some(index) = index_by_id.get(&id).copied() {
            index
        } else {
            let provider = id
                .split_once('/')
                .map(|(provider, _)| provider)
                .filter(|provider| !provider.is_empty())
                .unwrap_or_default()
                .to_string();
            let index = models.len();
            index_by_id.insert(id.clone(), index);
            models.push(Model {
                id: id.clone(),
                label: id,
                provider,
                ..Model::default()
            });
            index
        };
        let mut next = index + 1;
        while next < lines.len() && lines[next].trim().is_empty() {
            next += 1;
        }
        if next < lines.len() && lines[next].trim_start().starts_with('{') {
            let (raw, resume_at) = collect_model_json(&lines, next);
            if let Ok(metadata) = serde_json::from_str::<OpenCodeModelMetadata>(&raw) {
                annotate_model(&mut models[model_index], metadata);
            }
            index = resume_at;
        } else {
            index += 1;
        }
    }
    models
}

fn parse_opencode_model_id_line(line: &str) -> Option<String> {
    let id = line.split_whitespace().next()?;
    if id.starts_with(['"', '{', '[']) || !id.contains('/') || id == id.to_uppercase() {
        return None;
    }
    Some(id.to_string())
}

fn collect_model_json(lines: &[&str], start: usize) -> (String, usize) {
    let mut raw = String::new();
    for (index, source_line) in lines.iter().enumerate().skip(start) {
        let line = source_line.trim();
        if index > start && parse_opencode_model_id_line(line).is_some() {
            return (raw, index);
        }
        if !raw.is_empty() {
            raw.push('\n');
        }
        raw.push_str(source_line);
        if serde_json::from_str::<Value>(&raw).is_ok() {
            return (raw, index + 1);
        }
    }
    (raw, lines.len())
}

#[derive(Debug, Deserialize)]
struct OpenCodeModelMetadata {
    #[serde(default)]
    reasoning: bool,
    #[serde(default)]
    variants: BTreeMap<String, OpenCodeVariant>,
}

#[derive(Debug, Deserialize)]
struct OpenCodeVariant {
    #[serde(default)]
    disabled: bool,
    #[serde(rename = "reasoningEffort", default)]
    reasoning_effort: String,
    #[serde(default)]
    thinking: Value,
}

fn annotate_model(model: &mut Model, metadata: OpenCodeModelMetadata) {
    let looks_reasoning = metadata.reasoning
        || metadata.variants.iter().any(|(name, variant)| {
            variant_order(name).is_some()
                || !variant.reasoning_effort.is_empty()
                || !variant.thinking.is_null()
        });
    if !looks_reasoning {
        return;
    }
    let mut values: Vec<_> = metadata
        .variants
        .into_iter()
        .filter(|(_, variant)| !variant.disabled)
        .map(|(value, _)| value)
        .filter(|value| !value.is_empty())
        .collect();
    values.sort_by(
        |left, right| match (variant_order(left), variant_order(right)) {
            (Some(left), Some(right)) => left.cmp(&right),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => left.cmp(right),
        },
    );
    if values.is_empty() {
        return;
    }
    let supported_levels = values
        .into_iter()
        .map(|value| ThinkingLevel {
            label: variant_label(&value),
            value,
            ..ThinkingLevel::default()
        })
        .collect();
    model.thinking = Some(ModelThinking {
        supported_levels,
        ..ModelThinking::default()
    });
}

fn variant_order(value: &str) -> Option<usize> {
    ["none", "minimal", "low", "medium", "high", "xhigh", "max"]
        .iter()
        .position(|known| *known == value)
}

fn variant_label(value: &str) -> String {
    match value {
        "none" => "None".to_string(),
        "minimal" => "Minimal".to_string(),
        "low" => "Low".to_string(),
        "medium" => "Medium".to_string(),
        "high" => "High".to_string(),
        "xhigh" => "Extra high".to_string(),
        "max" => "Max".to_string(),
        other => other
            .split('-')
            .map(|word| {
                let mut chars = word.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use tokio_util::sync::CancellationToken;

    use super::*;

    #[test]
    fn args_keep_prompt_off_argv_and_filter_owned_flags() {
        let options = ExecOptions {
            cwd: "/tmp/task".to_string(),
            model: "opencode/deepseek-v4".to_string(),
            thinking_level: "max".to_string(),
            resume_session_id: "session-1".to_string(),
            custom_args: vec![
                "--variant".to_string(),
                "low".to_string(),
                "--dir".to_string(),
                "/evil".to_string(),
                "--keep".to_string(),
            ],
            ..ExecOptions::default()
        };
        let args = build_opencode_args(&options);
        assert!(args.contains(&"--keep".to_string()));
        assert!(!args.iter().any(|arg| arg == "low" || arg == "/evil"));
        assert!(!args.iter().any(|arg| arg == "--prompt"));
        assert_eq!(args[0], "run");
    }

    #[test]
    fn verbose_model_parser_extracts_variants_and_deduplicates() {
        let models = parse_opencode_models(
            "anthropic/claude-sonnet-4\n{\"reasoning\":true,\"variants\":{\"high\":{},\"low\":{},\"max\":{\"disabled\":true}}}\nopenai/gpt-5\nanthropic/claude-sonnet-4\n",
        );
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].provider, "anthropic");
        let levels = &models[0]
            .thinking
            .as_ref()
            .unwrap_or_else(|| panic!("thinking catalog"))
            .supported_levels;
        assert_eq!(
            levels
                .iter()
                .map(|level| level.value.as_str())
                .collect::<Vec<_>>(),
            ["low", "high"]
        );
    }

    #[cfg(unix)]
    fn fake_backend(script: &str) -> (tempfile::TempDir, OpencodeBackend) {
        let directory = tempfile::tempdir_in(".")
            .unwrap_or_else(|error| panic!("tempdir in workspace: {error}"));
        let executable = directory.path().join("opencode");
        std::fs::write(&executable, script).unwrap_or_else(|error| panic!("write fake: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake: {error}"));
        (
            directory,
            OpencodeBackend::new(OpencodeConfig {
                command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
                ..OpencodeConfig::default()
            }),
        )
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_writes_prompt_and_translates_terminal_events() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
cat >/dev/null
case "$OPENCODE_CONFIG_CONTENT" in
  *mcp*) ;;
  *) exit 17 ;;
esac
printf '%s\n' '{"type":"step_start","sessionID":"ses-open"}'
printf '%s\n' '{"type":"text","sessionID":"ses-open","part":{"text":"ok"}}'
printf '%s\n' '{"type":"step_finish","sessionID":"ses-open","part":{"reason":"stop","tokens":{"input":7,"output":3}}}'
"#,
        );
        let session = backend
            .execute(
                "do the thing",
                ExecOptions {
                    mcp_config: Some(serde_json::json!({
                        "mcpServers": {"demo": {"command": "echo"}}
                    })),
                    timeout: Duration::from_secs(5),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute fake OpenCode: {error}"));
        let Session {
            mut messages,
            result,
        } = session;
        let mut saw_text = false;
        while let Some(message) = messages.recv().await {
            saw_text |= message.message_type == MessageType::Text && message.content == "ok";
        }
        let result = result
            .await
            .unwrap_or_else(|error| panic!("receive result: {error}"));
        assert!(saw_text);
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "ok");
        assert_eq!(result.session_id, "ses-open");
        assert_eq!(result.usage["unknown"].input_tokens, 7);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn empty_step_and_mcp_config_are_fail_closed() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
cat >/dev/null
printf '%s\n' '{"type":"step_start","sessionID":"ses-empty"}'
printf '%s\n' '{"type":"step_finish","sessionID":"ses-empty","part":{"reason":"stop"}}'
"#,
        );
        let managed = serde_json::json!({"mcpServers":{"demo":{"command":"echo"}}});
        let session = backend
            .execute(
                "prompt",
                ExecOptions {
                    mcp_config: Some(managed),
                    timeout: Duration::from_secs(5),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute fake OpenCode: {error}"));
        let Session {
            mut messages,
            result,
        } = session;
        while messages.recv().await.is_some() {}
        let result = result
            .await
            .unwrap_or_else(|error| panic!("receive result: {error}"));
        assert_eq!(result.status, "failed");
        assert!(result.error.contains("empty step"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_keeps_timeout_status_when_prompt_writer_is_aborted() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
cat >/dev/null
sleep 10
"#,
        );
        let session = backend
            .execute(
                "prompt",
                ExecOptions {
                    timeout: Duration::from_millis(25),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute fake OpenCode: {error}"));
        let Session {
            mut messages,
            result,
        } = session;
        while messages.recv().await.is_some() {}
        let result = result
            .await
            .unwrap_or_else(|error| panic!("receive result: {error}"));
        assert_eq!(result.status, "timeout");
        assert!(result.error.contains("timed out"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn discovery_retries_plain_models_after_empty_verbose_output() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
if [ "$2" = "--verbose" ]; then
  exit 1
fi
printf '%s\n' 'anthropic/claude-sonnet-4'
"#,
        );
        let catalog = backend
            .discover_models_for_runtime(
                "opencode-test",
                &CatalogCache::default(),
                CancellationToken::new(),
                Duration::from_secs(5),
            )
            .await;
        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.models[0].id, "anthropic/claude-sonnet-4");
    }
}
