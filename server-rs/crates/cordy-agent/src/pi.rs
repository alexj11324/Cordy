//! Pi JSON event-stream adapter and the compatible OMP runtime family.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinError;
use tokio_util::sync::CancellationToken;

use crate::command::{filter_custom_args, filter_launch_prefix, BlockedArgMode, RuntimeCommand};
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
use crate::model::{
    Catalog, CatalogCache, Model, ModelDiscoveryCacheKey, ModelThinking, ThinkingLevel,
};
use crate::process::OwnedProcessTree;
use crate::stderr::{with_stderr, SharedDiagnosticBuffer, DEFAULT_TAIL_BYTES};
use crate::stream::AgentLineReader;

const MESSAGE_BUFFER: usize = 256;
const PI_RPC_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(7);
const PI_TABLE_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(8);
const PI_PROMPT_WRITE_GRACE: Duration = Duration::from_secs(2);
const PI_TERMINATE_GRACE: Duration = Duration::from_secs(2);
const PI_KILL_GRACE: Duration = Duration::from_secs(10);
const PI_DISCOVERY_RPC_PREFIX: [&str; 6] = [
    "--mode",
    "rpc",
    "--no-session",
    "--no-skills",
    "--no-prompt-templates",
    "--no-context-files",
];
const PI_THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

#[derive(Debug, Clone, Default)]
pub struct PiConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
    pub default_executable: String,
    pub provider_label: String,
}

#[derive(Debug, Clone)]
pub struct PiBackend {
    config: PiConfig,
}

impl PiBackend {
    pub fn new(config: PiConfig) -> Self {
        Self { config }
    }

    pub async fn discover_models(
        &self,
        cache: &CatalogCache,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        self.discover_models_for_runtime("pi", cache, cancellation, timeout)
            .await
    }

    pub async fn discover_models_for_runtime(
        &self,
        runtime_id: &str,
        cache: &CatalogCache,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        let scope = if runtime_id.trim().is_empty() {
            "pi"
        } else {
            runtime_id
        };
        let Some(key) = ModelDiscoveryCacheKey::new(scope, &self.config.command) else {
            return Catalog::default();
        };
        if let Some(catalog) = cache.get(&key) {
            return catalog;
        }
        let timeout = if timeout.is_zero() {
            PI_RPC_DISCOVERY_TIMEOUT + PI_TABLE_DISCOVERY_TIMEOUT
        } else {
            timeout
        };
        let models = if scope == "omp" {
            self.discover_omp_models(cancellation.clone(), timeout)
                .await
                .unwrap_or_default()
        } else {
            self.discover_pi_models(cancellation.clone(), timeout)
                .await
                .unwrap_or_default()
        };
        let catalog = Catalog {
            models,
            fallback: false,
        };
        if cancellation.is_cancelled() {
            return Catalog::default();
        }
        let _ = cache.insert(key, catalog.clone());
        catalog
    }

    fn command_path(&self) -> &str {
        if !self.config.command.path.is_empty() {
            return &self.config.command.path;
        }
        if !self.config.default_executable.is_empty() {
            return &self.config.default_executable;
        }
        "pi"
    }

    fn label(&self) -> &str {
        if self.config.provider_label.is_empty() {
            "pi"
        } else {
            &self.config.provider_label
        }
    }

    async fn discover_pi_models(
        &self,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> io::Result<Vec<Model>> {
        let rpc_timeout = timeout.min(PI_RPC_DISCOVERY_TIMEOUT);
        let rpc = tokio::select! {
            () = cancellation.cancelled() => return Err(io::Error::new(io::ErrorKind::Interrupted, "Pi model discovery cancelled")),
            result = tokio::time::timeout(rpc_timeout, self.discover_pi_models_rpc()) => result.ok().and_then(Result::ok),
        };
        if let Some(models) = rpc.filter(|models| !models.is_empty()) {
            return Ok(models);
        }
        if cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Pi model discovery cancelled",
            ));
        }
        let table_timeout = timeout.min(PI_TABLE_DISCOVERY_TIMEOUT);
        tokio::select! {
            () = cancellation.cancelled() => Err(io::Error::new(io::ErrorKind::Interrupted, "Pi model discovery cancelled")),
            result = tokio::time::timeout(table_timeout, self.discover_pi_models_table()) => {
                result.unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::TimedOut, "Pi model table discovery timed out")))
            }
        }
    }

    async fn discover_pi_models_rpc(&self) -> io::Result<Vec<Model>> {
        let mut command = self.discovery_command(&PI_DISCOVERY_RPC_PREFIX);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut tree = OwnedProcessTree::spawn(&mut command).await?;
        let mut stdin = tree
            .child_mut()
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("Pi RPC stdin pipe unavailable"))?;
        let stdout = tree
            .child_mut()
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Pi RPC stdout pipe unavailable"))?;
        for request in [
            serde_json::json!({"id":"cordy-state","type":"get_state"}),
            serde_json::json!({"id":"cordy-models","type":"get_available_models"}),
        ] {
            write_json_line(&mut stdin, &request).await?;
        }

        let mut reader = AgentLineReader::new(BufReader::new(stdout));
        let mut raw_models = Vec::new();
        let mut state = PiRpcState::default();
        let mut models_done = false;
        let mut state_done = false;
        while let Some(line) = reader.next_line().await? {
            let Ok(response) = serde_json::from_str::<PiRpcResponse>(line.trim()) else {
                continue;
            };
            if response.response_type != "response" {
                continue;
            }
            if response.id == "cordy-state" || response.command == "get_state" {
                state_done = true;
                if response.success {
                    let _ = serde_json::from_value::<PiRpcState>(response.data)
                        .map(|value| state = value);
                }
            } else if response.id == "cordy-models" || response.command == "get_available_models" {
                models_done = true;
                if response.success {
                    if let Ok(payload) = serde_json::from_value::<PiRpcModelsPayload>(response.data)
                    {
                        raw_models = payload.models;
                    }
                }
            }
            if models_done && state_done {
                break;
            }
        }
        // `ChildStdin::shutdown` is a no-op on Unix. Drop the pipe explicitly
        // so the RPC process observes EOF before we drain its stdout and wait.
        drop(stdin);
        while reader.next_line().await?.is_some() {}
        let status = tree.wait().await?;
        if !status.success() || !models_done || raw_models.is_empty() {
            return Err(io::Error::other(
                "Pi RPC model discovery returned no catalog",
            ));
        }
        Ok(pi_models_from_rpc(raw_models, state))
    }

    async fn discover_pi_models_table(&self) -> io::Result<Vec<Model>> {
        let mut command = self.discovery_command(&["--list-models"]);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut tree = OwnedProcessTree::spawn(&mut command).await?;
        let stdout = tree
            .child_mut()
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Pi model stdout pipe unavailable"))?;
        let stderr = tree
            .child_mut()
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("Pi model stderr pipe unavailable"))?;
        let stdout_task = tokio::spawn(read_all(stdout));
        let stderr_task = tokio::spawn(read_all(stderr));
        let status = tree.wait().await?;
        let stdout = stdout_task
            .await
            .map_err(|error| io::Error::other(format!("Pi model stdout task: {error}")))??;
        let stderr = stderr_task
            .await
            .map_err(|error| io::Error::other(format!("Pi model stderr task: {error}")))??;
        let text = if stdout.iter().any(|byte| !byte.is_ascii_whitespace()) {
            String::from_utf8_lossy(&stdout).into_owned()
        } else {
            String::from_utf8_lossy(&stderr).into_owned()
        };
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let _ = status;
        Ok(parse_pi_models_table(&text))
    }

    async fn discover_omp_models(
        &self,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> io::Result<Vec<Model>> {
        let mut command = self.discovery_command(&["models", "--json"]);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut tree = OwnedProcessTree::spawn(&mut command).await?;
        let stdout = tree
            .child_mut()
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("OMP model stdout pipe unavailable"))?;
        let output = tokio::select! {
            () = cancellation.cancelled() => return Err(io::Error::new(io::ErrorKind::Interrupted, "OMP model discovery cancelled")),
            result = tokio::time::timeout(timeout, read_all(stdout)) => result
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "OMP model discovery timed out"))??,
        };
        let status = tree.wait().await?;
        if !status.success() || output.is_empty() {
            return Ok(Vec::new());
        }
        Ok(parse_omp_models(&output))
    }

    fn discovery_command(&self, invocation: &[&str]) -> Command {
        let mut command = Command::new(self.command_path());
        let invocation = invocation
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>();
        let prefix = filter_launch_prefix(&self.config.command.prefix, &pi_blocked_args());
        let mut args = prefix.args;
        args.extend(invocation);
        command
            .args(args)
            .envs(&self.config.env)
            .kill_on_drop(false);
        command
    }
}

#[async_trait]
impl Backend for PiBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        if prompt.trim().is_empty() {
            return Err(AgentError::InvalidConfig(format!(
                "{} prompt must not be empty",
                self.label()
            )));
        }
        let session_path = if options.resume_session_id.is_empty() {
            new_pi_session_path().map_err(|error| {
                AgentError::Process(io::Error::other(format!(
                    "{} session path: {error}",
                    self.label()
                )))
            })?
        } else {
            PathBuf::from(&options.resume_session_id)
        };
        ensure_pi_session_file(&session_path).map_err(AgentError::Process)?;

        let command_path = self.command_path().to_string();
        let prefix = filter_launch_prefix(&self.config.command.prefix, &pi_blocked_args());
        let mut argv = prefix.args;
        argv.extend(build_pi_args(&session_path.to_string_lossy(), &options));
        let mut command = Command::new(&command_path);
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
                    AgentError::ExecutableNotFound(command_path.clone())
                } else {
                    AgentError::Process(error)
                }
            })?;
        let stdin = tree.child_mut().stdin.take().ok_or_else(|| {
            AgentError::Protocol(format!(
                "{} stdin pipe unavailable after spawn",
                self.label()
            ))
        })?;
        let stdout = tree.child_mut().stdout.take().ok_or_else(|| {
            AgentError::Protocol(format!(
                "{} stdout pipe unavailable after spawn",
                self.label()
            ))
        })?;
        let stderr = tree.child_mut().stderr.take().ok_or_else(|| {
            AgentError::Protocol(format!(
                "{} stderr pipe unavailable after spawn",
                self.label()
            ))
        })?;

        let (message_tx, message_rx) = mpsc::channel(MESSAGE_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();
        let prompt = prompt.as_bytes().to_vec();
        let label = self.label().to_string();
        let configured_model = options.model.clone();
        let timeout = options.timeout;
        let cancellation = options.cancellation.clone();
        let session_id = session_path.to_string_lossy().into_owned();
        let started = Instant::now();
        let stderr_tail = SharedDiagnosticBuffer::new(DEFAULT_TAIL_BYTES);
        let stderr_reader_tail = stderr_tail.clone();

        tokio::spawn(async move {
            let mut stderr_task = tokio::spawn(pump_stderr(stderr, stderr_reader_tail));
            let mut reader_task = tokio::spawn(read_pi_events(
                stdout,
                message_tx,
                configured_model,
                label.clone(),
            ));
            let mut writer_task = tokio::spawn(async move {
                let mut stdin = stdin;
                let result = stdin.write_all(&prompt).await;
                let shutdown = stdin.shutdown().await;
                result.and(shutdown)
            });

            let outcome = {
                let completion = async {
                    let exit = tree.wait().await;
                    let stream = (&mut reader_task).await;
                    (exit, stream)
                };
                tokio::pin!(completion);
                if timeout.is_zero() {
                    tokio::select! {
                        completed = &mut completion => PiCompletionOutcome::Completed(completed),
                        () = cancellation.cancelled() => PiCompletionOutcome::Cancelled,
                    }
                } else {
                    tokio::select! {
                        completed = &mut completion => PiCompletionOutcome::Completed(completed),
                        () = cancellation.cancelled() => PiCompletionOutcome::Cancelled,
                        () = tokio::time::sleep(timeout) => PiCompletionOutcome::DeadlineExceeded,
                    }
                }
            };

            let (run_end, exit, stream) = match outcome {
                PiCompletionOutcome::Completed(completed) => {
                    (PiRunEnd::Completed, completed.0, completed.1)
                }
                PiCompletionOutcome::Cancelled => {
                    let _ = tree.shutdown(PI_TERMINATE_GRACE, PI_KILL_GRACE).await;
                    let stream = (&mut reader_task).await;
                    (PiRunEnd::Cancelled, Ok(success_exit_status()), stream)
                }
                PiCompletionOutcome::DeadlineExceeded => {
                    let _ = tree.shutdown(PI_TERMINATE_GRACE, PI_KILL_GRACE).await;
                    let stream = (&mut reader_task).await;
                    (
                        PiRunEnd::DeadlineExceeded,
                        Ok(success_exit_status()),
                        stream,
                    )
                }
            };

            let write_error =
                match tokio::time::timeout(PI_PROMPT_WRITE_GRACE, &mut writer_task).await {
                    Ok(Ok(result)) => result.err().map(|error| error.to_string()),
                    Ok(Err(error)) => Some(format!("Pi prompt writer task failed: {error}")),
                    Err(_) => {
                        writer_task.abort();
                        Some("Pi prompt writer did not finish after process exit".to_string())
                    }
                };
            if tokio::time::timeout(PI_KILL_GRACE, &mut stderr_task)
                .await
                .is_err()
            {
                stderr_task.abort();
            }
            let stderr = stderr_tail.tail();
            let state = stream.unwrap_or_else(join_failure_state);
            let result = finalize_pi_result(
                run_end,
                timeout,
                exit.as_ref().ok(),
                exit.as_ref().err(),
                write_error.as_deref(),
                state,
                &session_id,
                &label,
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

pub fn build_pi_args(session_path: &str, options: &ExecOptions) -> Vec<String> {
    let mut args = vec!["-p".to_string(), "--mode".to_string(), "json".to_string()];
    if !session_path.is_empty() {
        args.extend(["--session".to_string(), session_path.to_string()]);
    }
    if !options.model.trim().is_empty() {
        args.extend(["--model".to_string(), options.model.trim().to_string()]);
    }
    if !options.thinking_level.is_empty() {
        args.extend(["--thinking".to_string(), options.thinking_level.clone()]);
    }
    args.extend(filter_pi_custom_args(&options.custom_args));
    args
}

pub(crate) fn pi_blocked_args() -> BTreeMap<&'static str, BlockedArgMode> {
    BTreeMap::from([
        ("-p", BlockedArgMode::Standalone),
        ("--print", BlockedArgMode::Standalone),
        ("--mode", BlockedArgMode::WithValue),
        ("--session", BlockedArgMode::WithValue),
        ("--thinking", BlockedArgMode::WithValue),
    ])
}

fn pi_custom_arg_modes() -> BTreeMap<&'static str, BlockedArgMode> {
    BTreeMap::from([
        ("--help", BlockedArgMode::Standalone),
        ("-h", BlockedArgMode::Standalone),
        ("--version", BlockedArgMode::Standalone),
        ("-v", BlockedArgMode::Standalone),
        ("--continue", BlockedArgMode::Standalone),
        ("-c", BlockedArgMode::Standalone),
        ("--resume", BlockedArgMode::WithValue),
        ("-r", BlockedArgMode::WithValue),
        ("--provider", BlockedArgMode::WithValue),
        ("--model", BlockedArgMode::WithValue),
        ("--api-key", BlockedArgMode::WithValue),
        ("--system-prompt", BlockedArgMode::WithValue),
        ("--append-system-prompt", BlockedArgMode::WithValue),
        ("--name", BlockedArgMode::WithValue),
        ("-n", BlockedArgMode::WithValue),
        ("--no-session", BlockedArgMode::Standalone),
        ("--session-id", BlockedArgMode::WithValue),
        ("--fork", BlockedArgMode::WithValue),
        ("--session-dir", BlockedArgMode::WithValue),
        ("--models", BlockedArgMode::WithValue),
        ("--no-tools", BlockedArgMode::Standalone),
        ("-nt", BlockedArgMode::Standalone),
        ("--no-builtin-tools", BlockedArgMode::Standalone),
        ("-nbt", BlockedArgMode::Standalone),
        ("--tools", BlockedArgMode::WithValue),
        ("-t", BlockedArgMode::WithValue),
        ("--exclude-tools", BlockedArgMode::WithValue),
        ("-xt", BlockedArgMode::WithValue),
        ("--export", BlockedArgMode::WithValue),
        ("--extension", BlockedArgMode::WithValue),
        ("-e", BlockedArgMode::WithValue),
        ("--no-extensions", BlockedArgMode::Standalone),
        ("-ne", BlockedArgMode::Standalone),
        ("--skill", BlockedArgMode::WithValue),
        ("--prompt-template", BlockedArgMode::WithValue),
        ("--theme", BlockedArgMode::WithValue),
        ("--no-skills", BlockedArgMode::Standalone),
        ("-ns", BlockedArgMode::Standalone),
        ("--no-prompt-templates", BlockedArgMode::Standalone),
        ("-np", BlockedArgMode::Standalone),
        ("--no-themes", BlockedArgMode::Standalone),
        ("--no-context-files", BlockedArgMode::Standalone),
        ("-nc", BlockedArgMode::Standalone),
        ("--list-models", BlockedArgMode::OptionalValue),
        ("--verbose", BlockedArgMode::Standalone),
        ("--approve", BlockedArgMode::Standalone),
        ("-a", BlockedArgMode::Standalone),
        ("--no-approve", BlockedArgMode::Standalone),
        ("-na", BlockedArgMode::Standalone),
        ("--offline", BlockedArgMode::Standalone),
    ])
}

fn filter_pi_custom_args(args: &[String]) -> Vec<String> {
    let modes = pi_custom_arg_modes();
    let filtered = filter_custom_args(args, &pi_blocked_args()).args;
    let mut output = Vec::with_capacity(filtered.len());
    let mut index = 0;
    while index < filtered.len() {
        let arg = &filtered[index];
        if arg.starts_with('@') || !arg.starts_with('-') {
            index += 1;
            continue;
        }
        output.push(arg.clone());
        if arg.contains('=') {
            index += 1;
            continue;
        }
        let flag = arg.as_str();
        let mode = modes.get(flag).copied().or_else(|| {
            flag.starts_with("--")
                .then_some(BlockedArgMode::OptionalValue)
        });
        match mode {
            Some(BlockedArgMode::WithValue) if index + 1 < filtered.len() => {
                // Match Go's filterPiCustomArgs: a known value-taking option
                // owns the next token even when the value starts with '-' or
                // '@'.
                output.push(filtered[index + 1].clone());
                index += 1;
            }
            Some(BlockedArgMode::OptionalValue) if index + 1 < filtered.len() => {
                let value = &filtered[index + 1];
                if !value.starts_with('-') && !value.starts_with('@') {
                    output.push(value.clone());
                    index += 1;
                }
            }
            _ => {}
        }
        index += 1;
    }
    output
}

fn new_pi_session_path() -> io::Result<PathBuf> {
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "Pi home directory is unavailable")
        })?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    Ok(PathBuf::from(home)
        .join(".cordy")
        .join("pi-sessions")
        .join(format!("{nanos}.jsonl")))
}

fn ensure_pi_session_file(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // This only reserves/touches the per-run session path. If a timestamp
    // collision finds an existing file, preserve it rather than truncating it.
    let _file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    Ok(())
}

#[derive(Debug, Deserialize)]
struct PiEvent {
    #[serde(rename = "type", default)]
    event_type: String,
    #[serde(rename = "assistantMessageEvent", default)]
    assistant_message_event: Option<PiAssistantMessageEvent>,
    #[serde(rename = "toolCallId", default)]
    tool_call_id: String,
    #[serde(rename = "toolName", default)]
    tool_name: String,
    #[serde(default)]
    args: Option<Value>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    message: Option<Value>,
    #[serde(default)]
    success: bool,
    #[serde(rename = "finalError", default)]
    final_error: String,
}

#[derive(Debug, Deserialize)]
struct PiAssistantMessageEvent {
    #[serde(rename = "type", default)]
    event_type: String,
    #[serde(default)]
    delta: String,
}

#[derive(Debug, Deserialize)]
struct PiMessage {
    #[serde(default)]
    model: String,
    #[serde(default)]
    usage: Option<PiUsage>,
    #[serde(rename = "stopReason", default)]
    stop_reason: String,
    #[serde(rename = "errorMessage", default)]
    error_message: String,
}

#[derive(Debug, Deserialize)]
struct PiUsage {
    #[serde(default)]
    input: i64,
    #[serde(default)]
    output: i64,
    #[serde(rename = "cacheRead", default)]
    cache_read: i64,
    #[serde(rename = "cacheWrite", default)]
    cache_write: i64,
}

#[derive(Debug, Default)]
struct PiStreamState {
    output: String,
    text_buffer: String,
    final_status: String,
    final_error: String,
    last_turn_error: String,
    stream_error: String,
    usage: BTreeMap<String, TokenUsage>,
}

async fn read_pi_events(
    stdout: ChildStdout,
    messages: mpsc::Sender<Message>,
    configured_model: String,
    label: String,
) -> PiStreamState {
    let mut reader = AgentLineReader::new(BufReader::new(stdout));
    let mut state = PiStreamState {
        final_status: "completed".to_string(),
        ..PiStreamState::default()
    };
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Ok(event) = serde_json::from_str::<PiEvent>(line) else {
                    continue;
                };
                handle_pi_event(event, &messages, &configured_model, &label, &mut state);
            }
            Ok(None) => break,
            Err(error) => {
                state.stream_error = error.to_string();
                break;
            }
        }
    }
    let text = flush_pi_text_buffer(&mut state.text_buffer);
    if !text.is_empty() {
        state.output.push_str(&text);
        try_send(&messages, Message::text(text));
    }
    state
}

fn handle_pi_event(
    event: PiEvent,
    messages: &mpsc::Sender<Message>,
    configured_model: &str,
    label: &str,
    state: &mut PiStreamState,
) {
    match event.event_type.as_str() {
        "agent_start" => try_send(
            messages,
            Message {
                message_type: MessageType::Status,
                status: "running".to_string(),
                ..empty_message(MessageType::Status)
            },
        ),
        "turn_start" => {
            state.output.clear();
            state.text_buffer.clear();
            state.last_turn_error.clear();
        }
        "message_update" => {
            let Some(update) = event.assistant_message_event else {
                return;
            };
            if update.event_type == "text_delta" {
                let text = drain_pi_text_buffer_string(&mut state.text_buffer, &update.delta);
                if !text.is_empty() {
                    state.output.push_str(&text);
                    try_send(messages, Message::text(text));
                }
            } else if update.event_type == "thinking_delta" && !update.delta.is_empty() {
                try_send(
                    messages,
                    Message {
                        message_type: MessageType::Thinking,
                        content: update.delta,
                        ..empty_message(MessageType::Thinking)
                    },
                );
            }
        }
        "tool_execution_start" => try_send(
            messages,
            Message {
                message_type: MessageType::ToolUse,
                tool: event.tool_name,
                call_id: event.tool_call_id,
                input: event
                    .args
                    .and_then(|value| value.as_object().cloned())
                    .map(|object| object.into_iter().collect())
                    .unwrap_or_default(),
                ..empty_message(MessageType::ToolUse)
            },
        ),
        "tool_execution_end" => try_send(
            messages,
            Message {
                message_type: MessageType::ToolResult,
                tool: event.tool_name,
                call_id: event.tool_call_id,
                output: decode_pi_value(event.result),
                ..empty_message(MessageType::ToolResult)
            },
        ),
        "turn_end" => {
            let Some(message) = event
                .message
                .and_then(|value| serde_json::from_value::<PiMessage>(value).ok())
            else {
                return;
            };
            if let Some(usage) = message.usage {
                let model = if message.model.is_empty() {
                    if configured_model.is_empty() {
                        "unknown"
                    } else {
                        configured_model
                    }
                } else {
                    message.model.as_str()
                };
                let entry = state.usage.entry(model.to_string()).or_default();
                entry.input_tokens += usage.input;
                entry.output_tokens += usage.output;
                entry.cache_read_tokens += usage.cache_read;
                entry.cache_write_tokens += usage.cache_write;
            }
            if message.stop_reason == "error" {
                state.last_turn_error = if message.error_message.is_empty() {
                    format!("{label} ended the turn with an error")
                } else {
                    message.error_message
                };
            }
        }
        "error" => {
            let error = decode_pi_value(event.message);
            try_send(
                messages,
                Message {
                    message_type: MessageType::Error,
                    content: error.clone(),
                    ..empty_message(MessageType::Error)
                },
            );
            if state.final_status == "completed" {
                state.final_status = "failed".to_string();
                state.final_error = error;
            }
        }
        "auto_retry_end" if !event.success && state.final_status == "completed" => {
            state.final_status = "failed".to_string();
            state.final_error = if event.final_error.is_empty() {
                format!("{label} exhausted automatic retries")
            } else {
                event.final_error
            };
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn finalize_pi_result(
    run_end: PiRunEnd,
    timeout: Duration,
    exit: Option<&ExitStatus>,
    wait_error: Option<&io::Error>,
    write_error: Option<&str>,
    state: PiStreamState,
    session_id: &str,
    label: &str,
    stderr: &str,
    elapsed: Duration,
) -> ExecutionResult {
    let mut status = state.final_status;
    let mut error = state.final_error;
    if matches!(run_end, PiRunEnd::DeadlineExceeded) {
        status = "timeout".to_string();
        error = format!("{label} timed out after {}s", timeout.as_secs_f64());
    } else if matches!(run_end, PiRunEnd::Cancelled) {
        status = "aborted".to_string();
        error = "execution cancelled".to_string();
    } else if status == "completed" {
        if !state.stream_error.is_empty() {
            status = "failed".to_string();
            error = format!("read {label} event stream: {}", state.stream_error);
        } else if let Some(wait_error) = wait_error {
            status = "failed".to_string();
            error = format!("{label} exited with error: {wait_error}");
        } else if let Some(write_error) = write_error {
            status = "failed".to_string();
            error = format!("{label} prompt write failed: {write_error}");
        } else if !state.last_turn_error.is_empty() {
            status = "failed".to_string();
            error = state.last_turn_error;
        } else if exit.is_some_and(|status| !status.success()) {
            status = "failed".to_string();
            error = format!(
                "{label} exited with error: {}",
                exit.unwrap_or(&success_exit_status())
            );
        }
    }
    if !error.is_empty() {
        error = with_stderr(&error, label, stderr);
    }
    ExecutionResult {
        status,
        output: state.output,
        error,
        duration_ms: elapsed.as_millis().try_into().unwrap_or(i64::MAX),
        session_id: session_id.to_string(),
        usage: state.usage,
        ..ExecutionResult::default()
    }
}

enum PiRunEnd {
    Completed,
    Cancelled,
    DeadlineExceeded,
}

enum PiCompletionOutcome {
    Completed((io::Result<ExitStatus>, Result<PiStreamState, JoinError>)),
    Cancelled,
    DeadlineExceeded,
}

fn join_failure_state(error: JoinError) -> PiStreamState {
    PiStreamState {
        final_status: "failed".to_string(),
        stream_error: format!("Pi stream task failed: {error}"),
        ..PiStreamState::default()
    }
}

async fn read_all<R: tokio::io::AsyncRead + Unpin>(mut reader: R) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    reader.read_to_end(&mut output).await?;
    Ok(output)
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

async fn write_json_line<W: AsyncWrite + Unpin>(writer: &mut W, value: &Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await
}

fn decode_pi_value(value: Option<Value>) -> String {
    match value {
        Some(Value::String(value)) => value,
        Some(value) => value.to_string(),
        None => String::new(),
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

fn try_send(sender: &mpsc::Sender<Message>, message: Message) {
    let _ = sender.try_send(message);
}

trait PiMessageExt {
    fn text(content: String) -> Self;
}

impl PiMessageExt for Message {
    fn text(content: String) -> Self {
        Self {
            message_type: MessageType::Text,
            content,
            ..empty_message(MessageType::Text)
        }
    }
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

#[derive(Debug, Default, Deserialize)]
struct PiRpcState {
    #[serde(default)]
    model: Option<PiRpcModel>,
    #[serde(rename = "thinkingLevel", default)]
    thinking_level: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct PiRpcModel {
    #[serde(default)]
    id: String,
    #[serde(default)]
    provider: String,
    #[serde(default)]
    reasoning: bool,
    #[serde(rename = "thinkingLevelMap", default)]
    thinking_level_map: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Deserialize)]
struct PiRpcResponse {
    #[serde(default)]
    id: String,
    #[serde(rename = "type", default)]
    response_type: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    success: bool,
    #[serde(default)]
    data: Value,
}

#[derive(Debug, Default, Deserialize)]
struct PiRpcModelsPayload {
    #[serde(default)]
    models: Vec<PiRpcModel>,
}

fn pi_models_from_rpc(raw_models: Vec<PiRpcModel>, state: PiRpcState) -> Vec<Model> {
    let default_id = state
        .model
        .as_ref()
        .filter(|model| !model.provider.is_empty() && !model.id.is_empty())
        .map(|model| format!("{}/{}", model.provider, model.id));
    let mut seen = BTreeMap::new();
    let mut models = Vec::new();
    for raw in raw_models {
        let provider = raw.provider.trim();
        let model_id = raw.id.trim();
        if provider.is_empty() || model_id.is_empty() {
            continue;
        }
        let id = format!("{provider}/{model_id}");
        if seen.insert(id.clone(), ()).is_some() {
            continue;
        }
        let default = default_id.as_deref() == Some(id.as_str());
        let mut thinking = pi_thinking_from_rpc_model(&raw);
        if default {
            if let Some(thinking) = thinking.as_mut() {
                if thinking
                    .supported_levels
                    .iter()
                    .any(|level| level.value == state.thinking_level)
                {
                    thinking.default_level = state.thinking_level.clone();
                }
            }
        }
        models.push(Model {
            id: id.clone(),
            label: id.clone(),
            provider: provider.to_string(),
            default,
            thinking,
            ..Model::default()
        });
    }
    models
}

fn pi_thinking_from_rpc_model(model: &PiRpcModel) -> Option<ModelThinking> {
    if !model.reasoning {
        return None;
    }
    let mut supported_levels = Vec::new();
    for value in PI_THINKING_LEVELS {
        match model.thinking_level_map.get(value) {
            Some(None) => continue,
            None if matches!(value, "xhigh" | "max") => continue,
            _ => supported_levels.push(ThinkingLevel {
                value: value.to_string(),
                label: pi_thinking_label(value).to_string(),
                description: String::new(),
            }),
        }
    }
    (!supported_levels.is_empty()).then_some(ModelThinking {
        supported_levels,
        default_level: String::new(),
    })
}

fn pi_thinking_label(value: &str) -> &str {
    match value {
        "off" => "Off",
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra high",
        "max" => "Max",
        _ => value,
    }
}

fn parse_pi_models_table(output: &str) -> Vec<Model> {
    let mut seen = BTreeMap::new();
    let mut models = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || is_pi_discovery_noise(line) {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let Some(first) = fields.first() else {
            continue;
        };
        if first.eq_ignore_ascii_case("provider") {
            continue;
        }
        let id = if first.contains([':', '/']) {
            first.replacen(':', "/", 1)
        } else if fields.len() >= 2 {
            format!("{}/{}", first, fields[1])
        } else {
            continue;
        };
        let Some(slash) = id.find('/') else {
            continue;
        };
        if slash == 0 || slash + 1 == id.len() || seen.insert(id.clone(), ()).is_some() {
            continue;
        }
        models.push(Model {
            provider: id[..slash].to_string(),
            label: id.clone(),
            id,
            ..Model::default()
        });
    }
    models
}

fn is_pi_discovery_noise(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("no models match pattern")
        || lower.starts_with("warning:")
        || lower.starts_with("error:")
        || lower.starts_with("info:")
        || line.bytes().any(|byte| byte == 96)
        || lower.contains("--help")
        || lower.contains("usage:")
        || lower.contains("unknown flag")
        || lower.contains("unknown command")
}

fn parse_omp_models(data: &[u8]) -> Vec<Model> {
    #[derive(Deserialize)]
    struct OmpModel {
        #[serde(default)]
        id: String,
        #[serde(default)]
        provider: String,
        #[serde(default)]
        selector: String,
        #[serde(default)]
        name: String,
    }
    #[derive(Deserialize)]
    struct OmpPayload {
        #[serde(default)]
        models: Vec<OmpModel>,
    }
    let Ok(payload) = serde_json::from_slice::<OmpPayload>(data) else {
        return Vec::new();
    };
    let mut seen = BTreeMap::new();
    let mut models = Vec::new();
    for entry in payload.models {
        let bare_id = entry.id.trim();
        if bare_id.is_empty() {
            continue;
        }
        let provider = entry.provider.trim();
        let selector = if !entry.selector.trim().is_empty() {
            entry.selector.trim().to_string()
        } else if !provider.is_empty() {
            format!("{provider}/{bare_id}")
        } else {
            bare_id.to_string()
        };
        if seen.insert(selector.clone(), ()).is_some() {
            continue;
        }
        models.push(Model {
            id: selector.clone(),
            label: if entry.name.trim().is_empty() {
                selector.clone()
            } else {
                entry.name.trim().to_string()
            },
            provider: provider.to_string(),
            ..Model::default()
        });
    }
    models
}

fn drain_pi_text_buffer_string(buffer: &mut String, delta: &str) -> String {
    buffer.push_str(delta);
    let (emit, pending) = drain_pi_sanitized_text(buffer);
    *buffer = pending;
    emit
}

fn flush_pi_text_buffer(buffer: &mut String) -> String {
    let pending = std::mem::take(buffer);
    let (mut emit, pending) = drain_pi_sanitized_text(&pending);
    emit.push_str(&strip_pi_control_tokens(&pending));
    emit
}

fn drain_pi_sanitized_text(input: &str) -> (String, String) {
    let mut output = String::new();
    let mut index = 0;
    while index < input.len() {
        let Some((start, prefix_len)) = next_pi_tool_markup_prefix(input, index) else {
            let safe_len = safe_pi_text_emit_len(&input[index..]);
            output.push_str(&strip_pi_control_tokens(&input[index..index + safe_len]));
            return (output, input[index + safe_len..].to_string());
        };
        output.push_str(&input[index..start]);
        let Some(end) = scan_pi_tool_markup_end(input, start + prefix_len) else {
            output = strip_pi_control_tokens(&output);
            return (output, input[start..].to_string());
        };
        index = end;
    }
    (strip_pi_control_tokens(&output), String::new())
}

fn next_pi_tool_markup_prefix(input: &str, from: usize) -> Option<(usize, usize)> {
    let call = input[from..].find("call:").map(|offset| (from + offset, 5));
    let response = input[from..]
        .find("response:")
        .map(|offset| (from + offset, 9));
    match (call, response) {
        (Some(left), Some(right)) => Some(if left.0 <= right.0 { left } else { right }),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn scan_pi_tool_markup_end(input: &str, mut index: usize) -> Option<usize> {
    let name_start = index;
    while index < input.len() && is_pi_tool_name_byte(input.as_bytes()[index]) {
        index += 1;
    }
    if index == name_start || index >= input.len() || input.as_bytes()[index] != b'{' {
        return None;
    }
    let mut depth = 0;
    let mut in_quote = false;
    while index < input.len() {
        if input[index..].starts_with("<|\"|>") {
            in_quote = !in_quote;
            index += 5;
            continue;
        }
        if !in_quote {
            match input.as_bytes()[index] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    index += 1;
                    if depth == 0 {
                        if input[index..].starts_with("<tool_call|>") {
                            index += "<tool_call|>".len();
                        }
                        return Some(index);
                    }
                    continue;
                }
                _ => {}
            }
        }
        let character = input[index..].chars().next()?;
        index += character.len_utf8();
    }
    None
}

fn is_pi_tool_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn safe_pi_text_emit_len(input: &str) -> usize {
    let mut hold = 0;
    for prefix in ["call:", "response:"] {
        for length in 1..prefix.len().min(input.len()) {
            if input.ends_with(&prefix[..length]) {
                hold = hold.max(length);
            }
        }
    }
    if let Some(index) = input.rfind('<') {
        let suffix = &input[index..];
        if suffix.len() <= 64 && looks_like_pi_control_prefix(suffix) {
            hold = hold.max(suffix.len());
        }
    }
    input.len() - hold
}

fn looks_like_pi_control_prefix(input: &str) -> bool {
    !input.is_empty()
        && input.len() <= 64
        && input
            .as_bytes()
            .iter()
            .skip(1)
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'|' | b'>'))
}

fn strip_pi_control_tokens(input: &str) -> String {
    let mut output = String::new();
    let mut index = 0;
    while index < input.len() {
        if let Some(end) = scan_pi_control_token(input, index) {
            index = end;
            continue;
        }
        let character = input[index..].chars().next().unwrap_or_default();
        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn scan_pi_control_token(input: &str, start: usize) -> Option<usize> {
    if input[start..].starts_with("<|") {
        let mut index = start + 2;
        let name_start = index;
        while index < input.len() && is_pi_tool_name_byte(input.as_bytes()[index]) {
            index += 1;
        }
        if index == name_start || index >= input.len() || input.as_bytes()[index] != b'>' {
            return None;
        }
        index += 1;
        while index < input.len() && is_pi_tool_name_byte(input.as_bytes()[index]) {
            index += 1;
        }
        return Some(index);
    }
    if input.as_bytes().get(start) == Some(&b'<') {
        let mut index = start + 1;
        let name_start = index;
        while index < input.len() && is_pi_tool_name_byte(input.as_bytes()[index]) {
            index += 1;
        }
        if index > name_start && input[index..].starts_with("|>") {
            return Some(index + 2);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake_backend(script: &str, runtime: &str) -> (tempfile::TempDir, PiBackend) {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let executable = directory.path().join(runtime);
        std::fs::write(&executable, script)
            .unwrap_or_else(|error| panic!("write Pi fixture: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod Pi fixture: {error}"));
        let backend = PiBackend::new(PiConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            env: BTreeMap::new(),
            default_executable: runtime.to_string(),
            provider_label: runtime.to_string(),
        });
        (directory, backend)
    }

    #[test]
    fn pi_args_keep_model_and_remove_prompt_inputs() {
        let options = ExecOptions {
            model: "claude/claude-opus-5".to_string(),
            thinking_level: "high".to_string(),
            custom_args: vec![
                "--tools".to_string(),
                "read,bash".to_string(),
                "@prompt.md".to_string(),
                "positional".to_string(),
                "--mode".to_string(),
                "rpc".to_string(),
            ],
            ..ExecOptions::default()
        };
        let args = build_pi_args("/tmp/session.jsonl", &options);
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--model", "claude/claude-opus-5"]));
        assert!(args.windows(2).any(|pair| pair == ["--tools", "read,bash"]));
        assert!(!args
            .iter()
            .any(|value| value == "@prompt.md" || value == "positional"));
        assert!(!args.iter().any(|value| value == "rpc"));
    }

    #[test]
    fn pi_known_value_args_keep_dash_prefixed_values() {
        let options = ExecOptions {
            custom_args: vec![
                "--provider".to_string(),
                "-anthropic".to_string(),
                "--api-key".to_string(),
                "@credential".to_string(),
            ],
            ..ExecOptions::default()
        };
        let args = build_pi_args("", &options);
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--provider", "-anthropic"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--api-key", "@credential"]));
    }

    #[test]
    fn pi_text_sanitizer_handles_split_markup_and_control_tokens() {
        let chunks = ["before ca", "ll:bash{command:<|\"|>ls", "<|\"|>} after"];
        let mut buffer = String::new();
        let mut output = String::new();
        for chunk in chunks {
            output.push_str(&drain_pi_text_buffer_string(&mut buffer, chunk));
        }
        output.push_str(&flush_pi_text_buffer(&mut buffer));
        assert_eq!(output, "before  after");

        let mut buffer = String::new();
        let output = drain_pi_text_buffer_string(
            &mut buffer,
            "before call:bash{\"command\":\"你好\"} after",
        );
        assert_eq!(output + &flush_pi_text_buffer(&mut buffer), "before  after");

        let mut buffer = String::new();
        let mut output = drain_pi_text_buffer_string(&mut buffer, "before <|tu");
        output.push_str(&drain_pi_text_buffer_string(&mut buffer, "rn>model after"));
        output.push_str(&flush_pi_text_buffer(&mut buffer));
        assert_eq!(output, "before  after");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_translates_pi_events_and_last_turn_output() {
        let (_directory, backend) = fake_backend(
            r##"#!/bin/sh
cat > /dev/null
printf '%s\n' '{"type":"agent_start"}'
printf '%s\n' '{"type":"turn_start"}'
printf '%s\n' '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"old"}}'
printf '%s\n' '{"type":"turn_end","message":{"model":"test","usage":{"input":1,"output":2}}}'
printf '%s\n' '{"type":"turn_start"}'
printf '%s\n' '{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","delta":"reason"}}'
printf '%s\n' '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"final"}}'
printf '%s\n' '{"type":"tool_execution_start","toolCallId":"call-1","toolName":"bash","args":{"command":"pwd"}}'
printf '%s\n' '{"type":"tool_execution_end","toolCallId":"call-1","toolName":"bash","result":"/work"}'
printf '%s\n' '{"type":"turn_end","message":{"model":"test","usage":{"input":3,"output":4}}}'
"##,
            "pi",
        );
        let session = backend
            .execute(
                "prompt",
                ExecOptions {
                    timeout: Duration::from_secs(5),
                    resume_session_id: std::env::temp_dir()
                        .join("cordy-pi-test-session.jsonl")
                        .to_string_lossy()
                        .into_owned(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute Pi: {error}"));
        let mut messages = Vec::new();
        let mut receiver = session.messages;
        while let Some(message) = receiver.recv().await {
            messages.push(message);
        }
        let result = session
            .result
            .await
            .unwrap_or_else(|error| panic!("Pi result: {error}"));
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "final");
        assert_eq!(result.usage["test"].input_tokens, 4);
        assert!(messages
            .iter()
            .any(|message| message.message_type == MessageType::Thinking));
        assert!(messages
            .iter()
            .any(|message| message.message_type == MessageType::ToolUse));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pi_rpc_discovery_builds_authoritative_thinking_catalog() {
        // The fixture waits for EOF after both responses. This keeps the
        // regression test focused on closing stdin before draining and wait.
        let (_directory, backend) = fake_backend(
            r##"#!/bin/sh
test "$1" = --mode && test "$2" = rpc || exit 9
while IFS= read -r request; do
  case "$request" in
    *cordy-state*) printf '%s\n' '{"id":"cordy-state","type":"response","command":"get_state","success":true,"data":{"model":{"provider":"anthropic","id":"claude-sonnet"},"thinkingLevel":"high"}}' ;;
    *cordy-models*) printf '%s\n' '{"id":"cordy-models","type":"response","command":"get_available_models","success":true,"data":{"models":[{"provider":"anthropic","id":"claude-sonnet","name":"Sonnet","reasoning":true,"thinkingLevelMap":{"max":null}}]}}' ; break ;;
  esac
done
cat > /dev/null
"##,
            "pi",
        );
        let cache = CatalogCache::default();
        let catalog = backend
            .discover_models_for_runtime(
                "pi",
                &cache,
                CancellationToken::new(),
                Duration::from_secs(5),
            )
            .await;
        assert_eq!(catalog.models.len(), 1);
        assert!(catalog.models[0].default);
        assert_eq!(
            catalog.models[0].thinking.as_ref().unwrap().default_level,
            "high"
        );
        assert!(!catalog.models[0]
            .thinking
            .as_ref()
            .unwrap()
            .supported_levels
            .iter()
            .any(|level| level.value == "max"));
        assert_eq!(catalog.models[0].label, "anthropic/claude-sonnet");
    }

    #[test]
    fn omp_models_json_uses_selector_and_display_name() {
        let models = parse_omp_models(
            br#"{"models":[{"provider":"anthropic","id":"claude-sonnet","selector":"anthropic/claude-sonnet","name":"Sonnet"}]}"#,
        );
        assert_eq!(models[0].id, "anthropic/claude-sonnet");
        assert_eq!(models[0].label, "Sonnet");
    }
}
