//! GitHub Copilot CLI's headless JSONL adapter.
//!
//! Copilot uses its own one-shot output-format json event stream for task
//! execution. Model discovery is a separate ACP handshake and reuses the
//! already-tested ACP discovery implementation through QoderBackend.

use std::collections::BTreeMap;
use std::io;
use std::process::{ExitStatus, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinError;
use tokio_util::sync::CancellationToken;

use crate::command::{filter_custom_args, filter_launch_prefix, BlockedArgMode, RuntimeCommand};
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
use crate::model::{Catalog, CatalogCache, Model};
use crate::process::OwnedProcessTree;
use crate::qoder::{QoderBackend, QoderConfig};
use crate::stderr::{with_stderr, SharedDiagnosticBuffer, DEFAULT_TAIL_BYTES};
use crate::stream::AgentLineReader;

const MESSAGE_BUFFER: usize = 256;
const TERMINATION_GRACE: Duration = Duration::from_secs(5);
const KILL_GRACE: Duration = Duration::from_secs(10);

pub(crate) static BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> = LazyLock::new(|| {
    BTreeMap::from([
        ("-p", BlockedArgMode::WithValue),
        ("--output-format", BlockedArgMode::WithValue),
        ("--allow-all", BlockedArgMode::Standalone),
        ("--allow-all-tools", BlockedArgMode::Standalone),
        ("--allow-all-paths", BlockedArgMode::Standalone),
        ("--allow-all-urls", BlockedArgMode::Standalone),
        ("--yolo", BlockedArgMode::Standalone),
        ("--no-ask-user", BlockedArgMode::Standalone),
        ("--resume", BlockedArgMode::WithValue),
        ("--acp", BlockedArgMode::Standalone),
    ])
});

#[derive(Debug, Clone, Default)]
pub struct CopilotConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct CopilotBackend {
    config: CopilotConfig,
}

impl CopilotBackend {
    pub fn new(config: CopilotConfig) -> Self {
        Self { config }
    }

    pub async fn discover_models(
        &self,
        cache: &CatalogCache,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        self.discover_models_for_runtime("copilot", cache, cancellation, timeout)
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
            "copilot"
        } else {
            runtime_scope
        };
        let discovery = QoderBackend::new(QoderConfig {
            command: self.config.command.clone(),
            env: self.config.env.clone(),
            default_command: "copilot".to_string(),
            provider: "copilot".to_string(),
            launch_args: vec!["--acp".to_string()],
            discovery_args: vec!["--acp".to_string()],
            ..QoderConfig::default()
        });
        let mut catalog = discovery
            .discover_models_for_runtime(scope, cache, cancellation, timeout)
            .await;
        if catalog.models.is_empty() {
            return static_catalog();
        }
        annotate_copilot_providers(&mut catalog.models);
        catalog.fallback = false;
        catalog
    }
}

#[async_trait]
impl Backend for CopilotBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        let path = command_path(&self.config.command);
        let prefix = filter_launch_prefix(&self.config.command.prefix, &BLOCKED_ARGS);
        let (path, mut launch_args) = platform_invocation(path, prefix.args);
        launch_args.extend(build_copilot_args(prompt, &options));

        let mut command = Command::new(&path);
        command
            .args(launch_args)
            .stdin(Stdio::null())
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
                    AgentError::ExecutableNotFound(path.clone())
                } else {
                    AgentError::Process(error)
                }
            })?;
        let Some(stdout) = tree.child_mut().stdout.take() else {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            return Err(AgentError::Protocol(
                "Copilot stdout pipe unavailable after spawn".to_string(),
            ));
        };
        let Some(stderr) = tree.child_mut().stderr.take() else {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            return Err(AgentError::Protocol(
                "Copilot stderr pipe unavailable after spawn".to_string(),
            ));
        };

        let (message_tx, message_rx) = mpsc::channel(MESSAGE_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();
        let timeout = options.timeout;
        let cancellation = options.cancellation.clone();
        let seed_model = if options.model.is_empty() {
            "copilot".to_string()
        } else {
            options.model.clone()
        };
        let resumed = !options.resume_session_id.is_empty();
        let started = Instant::now();
        let stderr_tail = SharedDiagnosticBuffer::new(DEFAULT_TAIL_BYTES);

        tokio::spawn(async move {
            run_copilot(
                tree,
                stdout,
                stderr,
                message_tx,
                result_tx,
                cancellation,
                timeout,
                seed_model,
                resumed,
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

pub fn build_copilot_args(prompt: &str, options: &ExecOptions) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        prompt.to_string(),
        "--output-format".to_string(),
        "json".to_string(),
        "--allow-all".to_string(),
        "--no-ask-user".to_string(),
    ];
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
        "copilot".to_string()
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
        if should_filter_inherited_env(key_text) {
            continue;
        }
        command.env(key, value);
    }
    command.envs(extra);
}

fn should_filter_inherited_env(key: &str) -> bool {
    if key.to_ascii_uppercase().starts_with("CORDY_") {
        return true;
    }
    matches!(
        key,
        "CLAUDECODE"
            | "CLAUDE_CODE_ENTRYPOINT"
            | "CLAUDE_CODE_EXECPATH"
            | "CLAUDE_CODE_SESSION_ID"
            | "CLAUDE_CODE_SSE_PORT"
    ) || key.starts_with("CLAUDECODE_")
}

#[cfg(windows)]
fn platform_invocation(path: String, prefix: Vec<String>) -> (String, Vec<String>) {
    let path = locate_copilot_command(Path::new(&path)).unwrap_or_else(|| PathBuf::from(path));
    if let Some(native) = resolve_copilot_native_from_shim(&path) {
        return (native.to_string_lossy().into_owned(), prefix);
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !extension.eq_ignore_ascii_case("cmd") && !extension.eq_ignore_ascii_case("bat") {
        return (path.to_string_lossy().into_owned(), prefix);
    }
    let Some(ps1) = path
        .parent()
        .map(|parent| parent.join("copilot.ps1"))
        .filter(|candidate| candidate.is_file())
    else {
        return (path.to_string_lossy().into_owned(), prefix);
    };
    let Some(powershell) = find_powershell() else {
        return (path.to_string_lossy().into_owned(), prefix);
    };
    // The executable is switched by platform_path below; these are the
    // PowerShell launcher arguments before the original fixed prefix.
    let args = vec![
        "-NoProfile".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        ps1.to_string_lossy().into_owned(),
    ]
    .into_iter()
    .chain(prefix)
    .collect();
    (powershell, args)
}

#[cfg(windows)]
fn locate_copilot_command(command: &Path) -> Option<PathBuf> {
    if command.is_absolute() || command.components().count() > 1 {
        return command.is_file().then(|| command.to_path_buf());
    }

    let mut candidates = Vec::new();
    if command.extension().is_some() {
        candidates.push(command.to_path_buf());
    } else {
        candidates.extend([
            command.to_path_buf(),
            command.with_extension("com"),
            command.with_extension("exe"),
            command.with_extension("bat"),
            command.with_extension("cmd"),
        ]);
    }
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        for candidate in &candidates {
            let candidate = directory.join(candidate);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn platform_invocation(path: String, prefix: Vec<String>) -> (String, Vec<String>) {
    (path, prefix)
}

#[cfg(windows)]
fn resolve_copilot_native_from_shim(shim: &Path) -> Option<PathBuf> {
    let extension = shim.extension().and_then(|value| value.to_str())?;
    if !extension.eq_ignore_ascii_case("cmd") {
        return None;
    }
    let parent = shim.parent()?;
    let packages: &[&str] = if cfg!(target_arch = "aarch64") {
        &["copilot-win32-arm64", "copilot-win32-x64"]
    } else {
        &["copilot-win32-x64", "copilot-win32-arm64"]
    };
    for package in packages {
        for candidate in [
            parent
                .join("node_modules")
                .join("@github")
                .join("copilot")
                .join("node_modules")
                .join("@github")
                .join(package)
                .join("copilot.exe"),
            parent
                .join("node_modules")
                .join("@github")
                .join(package)
                .join("copilot.exe"),
        ] {
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

#[derive(Debug, Default)]
struct CopilotState {
    output: String,
    pending_delta: String,
    session_id: String,
    active_model: String,
    final_status: String,
    final_error: String,
    call_usage: BTreeMap<String, TokenUsage>,
    message_usage: BTreeMap<String, TokenUsage>,
    shutdown_usage: BTreeMap<String, TokenUsage>,
    resumed: bool,
    scan_error: String,
}

impl CopilotState {
    fn new(seed_model: String, resumed: bool) -> Self {
        Self {
            active_model: seed_model,
            final_status: "completed".to_string(),
            resumed,
            ..Self::default()
        }
    }

    fn final_output(&self) -> String {
        if self.pending_delta.is_empty() {
            self.output.clone()
        } else {
            self.pending_delta.clone()
        }
    }

    fn resolve_usage(&self) -> BTreeMap<String, TokenUsage> {
        if !self.resumed && has_tokens(&self.shutdown_usage) {
            return self.shutdown_usage.clone();
        }
        if has_tokens(&self.call_usage) {
            return self.call_usage.clone();
        }
        if has_tokens(&self.message_usage) {
            return self.message_usage.clone();
        }
        BTreeMap::new()
    }
}

fn has_tokens(usage: &BTreeMap<String, TokenUsage>) -> bool {
    usage.values().any(|value| {
        value.input_tokens > 0
            || value.output_tokens > 0
            || value.cache_read_tokens > 0
            || value.cache_write_tokens > 0
    })
}

fn add_usage(
    usage: &mut BTreeMap<String, TokenUsage>,
    model: &str,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
) {
    if model.is_empty() {
        return;
    }
    let cache_read = cache_read.max(0);
    let cache_write = cache_write.max(0);
    let output = output.max(0);
    let input = input
        .saturating_sub(cache_read)
        .saturating_sub(cache_write)
        .max(0);
    if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
        return;
    }
    let entry = usage.entry(model.to_string()).or_default();
    entry.input_tokens = entry.input_tokens.saturating_add(input);
    entry.output_tokens = entry.output_tokens.saturating_add(output);
    entry.cache_read_tokens = entry.cache_read_tokens.saturating_add(cache_read);
    entry.cache_write_tokens = entry.cache_write_tokens.saturating_add(cache_write);
}

fn handle_event(event: CopilotEvent, state: &mut CopilotState, messages: &mpsc::Sender<Message>) {
    match event.event_type.as_str() {
        "session.start" => {
            if let Ok(start) = serde_json::from_value::<CopilotSessionStart>(event.data) {
                if !start.selected_model.is_empty() {
                    state.active_model = start.selected_model;
                }
                if !start.session_id.is_empty() {
                    state.session_id = start.session_id;
                }
            }
        }
        "assistant.message_delta" => {
            if let Ok(delta) = serde_json::from_value::<CopilotMessageDelta>(event.data) {
                if !delta.delta_content.is_empty() {
                    state.pending_delta.push_str(&delta.delta_content);
                    send_message(
                        messages,
                        Message {
                            content: delta.delta_content,
                            ..empty_message(MessageType::Text)
                        },
                    );
                }
            }
        }
        "assistant.message" => {
            let Ok(message) = serde_json::from_value::<CopilotAssistantMessage>(event.data) else {
                return;
            };
            if !message.content.is_empty() {
                state.output.clear();
                state.output.push_str(&message.content);
            }
            state.pending_delta.clear();
            if !message.model.is_empty() {
                state.active_model = message.model;
            }
            if !message.reasoning_text.is_empty() {
                send_message(
                    messages,
                    Message {
                        content: message.reasoning_text,
                        ..empty_message(MessageType::Thinking)
                    },
                );
            }
            if message.output_tokens > 0 {
                add_usage(
                    &mut state.message_usage,
                    &state.active_model,
                    0,
                    message.output_tokens,
                    0,
                    0,
                );
            }
            for request in message.tool_requests {
                let input = request
                    .arguments
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                send_message(
                    messages,
                    Message {
                        tool: request.name,
                        call_id: request.tool_call_id,
                        input,
                        ..empty_message(MessageType::ToolUse)
                    },
                );
            }
        }
        "assistant.usage" => {
            let Ok(usage) = serde_json::from_value::<CopilotUsageData>(event.data) else {
                return;
            };
            let model = if usage.model.is_empty() || usage.model == "unknown" {
                state.active_model.clone()
            } else {
                state.active_model = usage.model.clone();
                usage.model
            };
            add_usage(
                &mut state.call_usage,
                &model,
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_tokens,
                usage.cache_write_tokens,
            );
        }
        "session.shutdown" => {
            let Ok(shutdown) = serde_json::from_value::<CopilotShutdownData>(event.data) else {
                return;
            };
            for (model, metric) in shutdown.model_metrics {
                let model = if model.is_empty() {
                    state.active_model.clone()
                } else {
                    model
                };
                add_usage(
                    &mut state.shutdown_usage,
                    &model,
                    metric.usage.input_tokens,
                    metric.usage.output_tokens,
                    metric.usage.cache_read_tokens,
                    metric.usage.cache_write_tokens,
                );
            }
        }
        "assistant.reasoning" | "assistant.reasoning_delta" => {
            if let Ok(reasoning) = serde_json::from_value::<CopilotReasoning>(event.data) {
                let content = if reasoning.content.is_empty() {
                    reasoning.delta_content
                } else {
                    reasoning.content
                };
                if !content.is_empty() {
                    send_message(
                        messages,
                        Message {
                            content,
                            ..empty_message(MessageType::Thinking)
                        },
                    );
                }
            }
        }
        "tool.execution_complete" => {
            let Ok(tool) = serde_json::from_value::<CopilotToolExecutionComplete>(event.data)
            else {
                return;
            };
            if !tool.model.is_empty() {
                state.active_model = tool.model;
            }
            let output = if tool.success {
                tool.result.map(|result| result.content).unwrap_or_default()
            } else if let Some(error) = tool.error {
                format!("Error: {}", error.message)
            } else {
                tool.result.map(|result| result.content).unwrap_or_default()
            };
            send_message(
                messages,
                Message {
                    call_id: tool.tool_call_id,
                    output,
                    ..empty_message(MessageType::ToolResult)
                },
            );
        }
        "assistant.turn_start" => send_message(
            messages,
            Message {
                status: "running".to_string(),
                ..empty_message(MessageType::Status)
            },
        ),
        "session.error" => {
            if let Ok(error) = serde_json::from_value::<CopilotSessionError>(event.data) {
                state.final_status = "failed".to_string();
                state.final_error = error.message.clone();
                send_message(
                    messages,
                    Message {
                        content: error.message,
                        level: "error".to_string(),
                        ..empty_message(MessageType::Log)
                    },
                );
            }
        }
        "session.warning" => {
            if let Ok(warning) = serde_json::from_value::<CopilotSessionWarning>(event.data) {
                send_message(
                    messages,
                    Message {
                        content: warning.message,
                        level: "warn".to_string(),
                        ..empty_message(MessageType::Log)
                    },
                );
            }
        }
        "result" => {
            if !event.session_id.is_empty() {
                state.session_id = event.session_id;
            }
            if event.exit_code != 0 {
                state.final_status = "failed".to_string();
                state.final_error = with_exit_code(&state.final_error, event.exit_code);
            }
        }
        _ => {}
    }
}

async fn read_stream(
    stdout: ChildStdout,
    messages: mpsc::Sender<Message>,
    mut state: CopilotState,
) -> CopilotState {
    let mut reader = AgentLineReader::new(BufReader::new(stdout));
    loop {
        match reader.next_line().await {
            Ok(Some(line)) if !line.trim().is_empty() => {
                match serde_json::from_str::<CopilotEvent>(line.trim()) {
                    Ok(event) => handle_event(event, &mut state, &messages),
                    Err(error) => {
                        tracing::warn!(provider = "copilot", %error, "event parse failed")
                    }
                }
            }
            Ok(Some(_)) => {}
            Ok(None) => return state,
            Err(error) => {
                state.scan_error = error.to_string();
                return state;
            }
        }
    }
}

async fn run_copilot(
    mut tree: OwnedProcessTree,
    stdout: ChildStdout,
    stderr: ChildStderr,
    messages: mpsc::Sender<Message>,
    result_tx: oneshot::Sender<ExecutionResult>,
    cancellation: CancellationToken,
    timeout: Duration,
    seed_model: String,
    resumed: bool,
    started: Instant,
    stderr_tail: SharedDiagnosticBuffer,
) {
    let mut stderr_task = tokio::spawn(pump_stderr(stderr, stderr_tail.clone()));
    let mut stdout_task = tokio::spawn(read_stream(
        stdout,
        messages,
        CopilotState::new(seed_model, resumed),
    ));
    let outcome = {
        let completion = async {
            let exit = tree.wait().await;
            let state = (&mut stdout_task).await;
            (exit, state)
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

    let (run_end, exit, stream) = match outcome {
        RunOutcome::Completed((exit, state)) => (RunEnd::Completed, Some(exit), state),
        RunOutcome::Cancelled => {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            (RunEnd::Cancelled, None, (&mut stdout_task).await)
        }
        RunOutcome::TimedOut => {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            (RunEnd::TimedOut, None, (&mut stdout_task).await)
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
    let exit_error = exit.as_ref().and_then(|status| match status {
        Ok(status) if status.success() => None,
        Ok(status) => Some(status.to_string()),
        Err(error) => Some(format!("wait failed: {error}")),
    });
    let (status, raw_error) = finalize_copilot(run_end, timeout, &mut state, exit_error.as_deref());
    let error = if raw_error.is_empty() {
        String::new()
    } else {
        with_stderr(&raw_error, "copilot", &stderr)
    };
    let usage = state.resolve_usage();
    let session_id = state.session_id.clone();
    let _ = result_tx.send(ExecutionResult {
        status,
        output: state.final_output(),
        error,
        duration_ms: started.elapsed().as_millis().try_into().unwrap_or(i64::MAX),
        session_id,
        usage,
        resume_rejected: false,
    });
}

fn finalize_copilot(
    run_end: RunEnd,
    timeout: Duration,
    state: &mut CopilotState,
    exit_error: Option<&str>,
) -> (String, String) {
    if state.final_status.is_empty() {
        state.final_status = "completed".to_string();
    }
    match run_end {
        RunEnd::TimedOut => {
            state.final_status = "timeout".to_string();
            state.final_error = format!("copilot timed out after {timeout:?}");
        }
        RunEnd::Cancelled => {
            state.final_status = "aborted".to_string();
            state.final_error = "execution cancelled".to_string();
        }
        RunEnd::Completed => {
            if state.final_status == "completed" && !state.scan_error.is_empty() {
                // Go records the scanner failure in logs but leaves a clean
                // process's status unchanged.
            } else if state.final_status == "completed" {
                if let Some(error) = exit_error {
                    state.final_status = "failed".to_string();
                    state.final_error = format!("copilot exited with error: {error}");
                }
            }
        }
    }
    (state.final_status.clone(), state.final_error.clone())
}

fn with_exit_code(message: &str, exit_code: i32) -> String {
    let suffix = format!("copilot exited with code {exit_code}");
    let message = message.trim();
    if message.is_empty() {
        suffix
    } else if message.contains(&suffix) {
        message.to_string()
    } else {
        format!("{message}; {suffix}")
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

fn join_failure_state(error: JoinError) -> CopilotState {
    CopilotState {
        final_status: "failed".to_string(),
        final_error: format!("Copilot stream task failed: {error}"),
        ..CopilotState::default()
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
    Completed((io::Result<ExitStatus>, Result<CopilotState, JoinError>)),
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunEnd {
    Completed,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Deserialize)]
struct CopilotEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    data: Value,
    #[serde(default, alias = "sessionId")]
    session_id: String,
    #[serde(default, alias = "exitCode")]
    exit_code: i32,
}

#[derive(Debug, Deserialize)]
struct CopilotSessionStart {
    #[serde(default, alias = "sessionId")]
    session_id: String,
    #[serde(default, alias = "selectedModel")]
    selected_model: String,
}

#[derive(Debug, Deserialize)]
struct CopilotAssistantMessage {
    #[serde(default)]
    model: String,
    #[serde(default)]
    content: String,
    #[serde(default, alias = "toolRequests")]
    tool_requests: Vec<CopilotToolRequest>,
    #[serde(default, alias = "outputTokens")]
    output_tokens: i64,
    #[serde(default, alias = "reasoningText")]
    reasoning_text: String,
}

#[derive(Debug, Deserialize)]
struct CopilotToolRequest {
    #[serde(default, alias = "toolCallId")]
    tool_call_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct CopilotMessageDelta {
    #[serde(default, alias = "deltaContent")]
    delta_content: String,
}

#[derive(Debug, Deserialize)]
struct CopilotUsageData {
    #[serde(default)]
    model: String,
    #[serde(default, alias = "inputTokens")]
    input_tokens: i64,
    #[serde(default, alias = "outputTokens")]
    output_tokens: i64,
    #[serde(default, alias = "cacheReadTokens")]
    cache_read_tokens: i64,
    #[serde(default, alias = "cacheWriteTokens")]
    cache_write_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct CopilotShutdownData {
    #[serde(default, alias = "modelMetrics")]
    model_metrics: BTreeMap<String, CopilotShutdownMetric>,
}

#[derive(Debug, Deserialize)]
struct CopilotShutdownMetric {
    usage: CopilotUsageData,
}

#[derive(Debug, Deserialize)]
struct CopilotReasoning {
    #[serde(default)]
    content: String,
    #[serde(default, alias = "deltaContent")]
    delta_content: String,
}

#[derive(Debug, Deserialize)]
struct CopilotToolExecutionComplete {
    #[serde(default, alias = "toolCallId")]
    tool_call_id: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    success: bool,
    #[serde(default)]
    result: Option<CopilotToolResult>,
    #[serde(default)]
    error: Option<CopilotToolError>,
}

#[derive(Debug, Deserialize)]
struct CopilotToolResult {
    #[serde(default)]
    content: String,
}

#[derive(Debug, Deserialize)]
struct CopilotToolError {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct CopilotSessionError {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct CopilotSessionWarning {
    #[serde(default)]
    message: String,
}

fn annotate_copilot_providers(models: &mut [Model]) {
    for model in models {
        if model.id.contains('/') {
            continue;
        }
        model.provider = infer_copilot_provider(&model.id).to_string();
    }
}

fn infer_copilot_provider(model_id: &str) -> &'static str {
    if model_id.starts_with("gpt-") || is_openai_reasoning_series_id(model_id) {
        "openai"
    } else if model_id.starts_with("claude-") {
        "anthropic"
    } else if model_id.starts_with("gemini-") {
        "google"
    } else if model_id.starts_with("grok-") {
        "xai"
    } else {
        ""
    }
}

fn is_openai_reasoning_series_id(model_id: &str) -> bool {
    let mut chars = model_id.chars();
    if chars.next() != Some('o') || !chars.next().is_some_and(|value| value.is_ascii_digit()) {
        return false;
    }
    chars.next().is_none_or(|value| value == '-')
}

fn static_catalog() -> Catalog {
    Catalog {
        models: vec![
            model("gpt-5.5", "GPT-5.5", "openai"),
            model("gpt-5.4", "GPT-5.4", "openai"),
            model("gpt-5.4-mini", "GPT-5.4 mini", "openai"),
            model("gpt-5.3-codex", "GPT-5.3-Codex", "openai"),
            model("gpt-5.2-codex", "GPT-5.2-Codex", "openai"),
            model("gpt-5.2", "GPT-5.2", "openai"),
            model("gpt-5-mini", "GPT-5 mini", "openai"),
            model("gpt-4.1", "GPT-4.1", "openai"),
            model("claude-opus-4.7", "Claude Opus 4.7", "anthropic"),
            model("claude-sonnet-4.6", "Claude Sonnet 4.6", "anthropic"),
            model("claude-sonnet-4.5", "Claude Sonnet 4.5", "anthropic"),
            model("claude-haiku-4.5", "Claude Haiku 4.5", "anthropic"),
        ],
        fallback: true,
    }
}

fn model(id: &str, label: &str, provider: &str) -> Model {
    Model {
        id: id.to_string(),
        label: label.to_string(),
        provider: provider.to_string(),
        ..Model::default()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn arguments_own_headless_protocol_and_permissions() {
        let options = ExecOptions {
            model: "gpt-5.4".to_string(),
            resume_session_id: "session-1".to_string(),
            custom_args: strings(&["--output-format", "text", "--allow-all-tools", "--verbose"]),
            ..ExecOptions::default()
        };
        let args = build_copilot_args("hello", &options);
        assert_eq!(
            &args[..10],
            strings(&[
                "-p",
                "hello",
                "--output-format",
                "json",
                "--allow-all",
                "--no-ask-user",
                "--model",
                "gpt-5.4",
                "--resume",
                "session-1",
            ])
        );
        assert!(!args
            .iter()
            .any(|arg| arg == "text" || arg == "--allow-all-tools"));
        assert!(args.iter().any(|arg| arg == "--verbose"));
    }

    #[test]
    fn copilot_provider_inference_matches_vendor_prefixes() {
        assert_eq!(infer_copilot_provider("gpt-5.4"), "openai");
        assert_eq!(infer_copilot_provider("o3-mini"), "openai");
        assert_eq!(infer_copilot_provider("claude-sonnet-4.6"), "anthropic");
        assert_eq!(infer_copilot_provider("gemini-2.5-pro"), "google");
        assert_eq!(infer_copilot_provider("grok-4"), "xai");
        assert_eq!(infer_copilot_provider("custom-model"), "");
    }

    #[test]
    fn static_catalog_is_a_fallback_with_expected_models() {
        let catalog = static_catalog();
        assert!(catalog.fallback);
        assert_eq!(catalog.models.len(), 12);
        assert_eq!(catalog.models[0].id, "gpt-5.5");
        assert_eq!(catalog.models[8].provider, "anthropic");
    }

    #[test]
    fn usage_sources_prefer_fresh_session_totals_and_resume_call_totals() {
        let mut fresh = CopilotState::new("gpt-5.4".to_string(), false);
        add_usage(&mut fresh.shutdown_usage, "gpt-5.4", 100, 10, 30, 0);
        add_usage(&mut fresh.call_usage, "gpt-5.4", 40, 4, 0, 0);
        assert_eq!(fresh.resolve_usage()["gpt-5.4"].input_tokens, 70);

        let mut resumed = CopilotState::new("gpt-5.4".to_string(), true);
        add_usage(&mut resumed.shutdown_usage, "gpt-5.4", 100, 10, 30, 0);
        add_usage(&mut resumed.call_usage, "gpt-5.4", 40, 4, 0, 0);
        assert_eq!(resumed.resolve_usage()["gpt-5.4"].input_tokens, 40);
    }

    #[test]
    fn event_handler_keeps_final_turn_and_maps_tools() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut state = CopilotState::new("copilot".to_string(), false);
        handle_event(
            serde_json::from_value(serde_json::json!({
                "type":"assistant.message_delta",
                "data":{"deltaContent":"partial"}
            }))
            .unwrap_or_else(|error| panic!("delta: {error}")),
            &mut state,
            &tx,
        );
        handle_event(
            serde_json::from_value(serde_json::json!({
                "type":"assistant.message",
                "data":{"content":"final","toolRequests":[],"outputTokens":2}
            }))
            .unwrap_or_else(|error| panic!("message: {error}")),
            &mut state,
            &tx,
        );
        assert_eq!(state.final_output(), "final");
        assert_eq!(
            rx.try_recv().map(|message| message.message_type),
            Ok(MessageType::Text)
        );
    }

    #[test]
    fn tool_only_message_does_not_discard_previous_complete_output() {
        let (tx, _rx) = mpsc::channel(16);
        let mut state = CopilotState::new("copilot".to_string(), false);
        state.output = "previous answer".to_string();
        state.pending_delta = "tool narration".to_string();
        handle_event(
            serde_json::from_value(serde_json::json!({
                "type":"assistant.message",
                "data":{"content":"","toolRequests":[{"toolCallId":"call-1","name":"shell","arguments":{}}]}
            }))
            .unwrap_or_else(|error| panic!("tool-only message: {error}")),
            &mut state,
            &tx,
        );
        assert_eq!(state.final_output(), "previous answer");
    }

    #[test]
    fn inherited_environment_filters_daemon_internal_state() {
        assert!(should_filter_inherited_env("CORDY_TASK_ID"));
        assert!(should_filter_inherited_env("CLAUDECODE"));
        assert!(should_filter_inherited_env("CLAUDECODE_PARENT"));
        assert!(!should_filter_inherited_env("CLAUDE_CODE_GIT_BASH_PATH"));
        assert!(!should_filter_inherited_env("PATH"));
    }

    #[cfg(unix)]
    fn fake_backend(script: &str) -> (tempfile::TempDir, CopilotBackend) {
        let directory = tempfile::tempdir_in(".")
            .unwrap_or_else(|error| panic!("tempdir in workspace: {error}"));
        let executable = directory.path().join("copilot");
        std::fs::write(&executable, script).unwrap_or_else(|error| panic!("write fake: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake: {error}"));
        (
            directory,
            CopilotBackend::new(CopilotConfig {
                command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
                env: BTreeMap::new(),
            }),
        )
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_parses_jsonl_result_and_usage() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
printf '%s\n' '{"type":"session.start","data":{"sessionId":"ses-1","selectedModel":"gpt-5.4"}}'
printf '%s\n' '{"type":"assistant.message_delta","data":{"deltaContent":"hello"}}'
printf '%s\n' '{"type":"assistant.message","data":{"model":"gpt-5.4","content":"hello","toolRequests":[],"outputTokens":2}}'
printf '%s\n' '{"type":"assistant.usage","data":{"model":"gpt-5.4","inputTokens":10,"outputTokens":2,"cacheReadTokens":3,"cacheWriteTokens":0}}'
printf '%s\n' '{"type":"result","sessionId":"ses-1","exitCode":0}'
"#,
        );
        let session = backend
            .execute("prompt", ExecOptions::default())
            .await
            .unwrap_or_else(|error| panic!("execute Copilot: {error}"));
        let Session {
            mut messages,
            result,
        } = session;
        let mut saw_text = false;
        while let Some(message) = messages.recv().await {
            saw_text |= message.message_type == MessageType::Text && message.content == "hello";
        }
        let result = result
            .await
            .unwrap_or_else(|error| panic!("receive result: {error}"));
        assert!(saw_text);
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "hello");
        assert_eq!(result.session_id, "ses-1");
        assert_eq!(result.usage["gpt-5.4"].input_tokens, 7);
        assert_eq!(result.usage["gpt-5.4"].output_tokens, 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_maps_nonzero_result_and_stderr() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
printf '%s\n' '{"type":"session.error","data":{"message":"rate limited"}}'
printf '%s\n' '{"type":"result","sessionId":"ses-2","exitCode":1}'
echo 'provider diagnostic' >&2
exit 1
"#,
        );
        let session = backend
            .execute("prompt", ExecOptions::default())
            .await
            .unwrap_or_else(|error| panic!("execute Copilot: {error}"));
        let Session {
            mut messages,
            result,
        } = session;
        while messages.recv().await.is_some() {}
        let result = result
            .await
            .unwrap_or_else(|error| panic!("receive result: {error}"));
        assert_eq!(result.status, "failed");
        assert!(result.error.contains("rate limited"));
        assert!(result.error.contains("provider diagnostic"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn discovery_reads_acp_models_and_infers_providers() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
test "$1" = --acp || exit 20
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"discovery","models":{"currentModelId":"gpt-5.4","availableModels":[{"modelId":"gpt-5.4","name":"GPT 5.4"},{"modelId":"claude-sonnet-4.6","name":"Claude Sonnet"}]}}}\n' "$id" ;;
  esac
done
"#,
        );
        let catalog = backend
            .discover_models(
                &CatalogCache::default(),
                CancellationToken::new(),
                Duration::from_secs(5),
            )
            .await;
        assert!(!catalog.fallback);
        assert_eq!(catalog.models.len(), 2);
        assert_eq!(catalog.models[0].provider, "openai");
        assert_eq!(catalog.models[1].provider, "anthropic");
    }
}
