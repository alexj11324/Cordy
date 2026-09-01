//! DevEco Code's headless JSON-lines adapter.
//!
//! DevEco is an OpenCode-derived CLI, but it owns a separate runtime contract:
//! it accepts the prompt as the final positional argument, does not accept
//! `--prompt`, and exposes models through `deveco models`. Keep this adapter
//! independent so changes to OpenCode or DevEco cannot silently alter the
//! other runtime.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::process::{ExitStatus, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::{ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::command::{filter_custom_args, filter_launch_prefix, BlockedArgMode, RuntimeCommand};
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
use crate::env::configure_child_env;
use crate::model::{Catalog, CatalogCache, Model, ModelDiscoveryCacheKey};
use crate::process::OwnedProcessTree;
use crate::stderr::{SharedDiagnosticBuffer, DEFAULT_TAIL_BYTES};
use crate::stream::AgentLineReader;

const MESSAGE_BUFFER: usize = 256;
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const KILL_GRACE: Duration = Duration::from_secs(10);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVERY_OUTPUT_MAX: u64 = 4 * 1024 * 1024;

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
pub struct DevecoConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct DevecoBackend {
    config: DevecoConfig,
}

impl DevecoBackend {
    pub fn new(config: DevecoConfig) -> Self {
        Self { config }
    }

    pub async fn discover_models(
        &self,
        cache: &CatalogCache,
        cancellation: tokio_util::sync::CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        self.discover_models_for_runtime("deveco", cache, cancellation, timeout)
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
            "deveco"
        } else {
            runtime_scope
        };
        let Some(key) = ModelDiscoveryCacheKey::new(scope, &self.config.command) else {
            return Catalog::default();
        };
        if let Some(catalog) = cache.get(&key) {
            return catalog;
        }

        let models = self
            .discover_models_command(cancellation.clone(), timeout)
            .await;
        if cancellation.is_cancelled() {
            return Catalog::default();
        }
        let catalog = Catalog {
            models,
            session_modes: Vec::new(),
            fallback: false,
        };
        // Match Go's cachedDiscovery: empty discovery is not cached so a
        // transient CLI/network failure can recover on the next request.
        if !catalog.models.is_empty() {
            let _ = cache.insert(key, catalog.clone());
        }
        catalog
    }

    async fn discover_models_command(
        &self,
        cancellation: tokio_util::sync::CancellationToken,
        timeout: Duration,
    ) -> Vec<Model> {
        let command_path = if self.config.command.path.is_empty() {
            "deveco"
        } else {
            self.config.command.path.as_str()
        };
        let prefix = filter_launch_prefix(&self.config.command.prefix, &BLOCKED_ARGS);
        let mut command = Command::new(command_path);
        command
            .args(prefix.args)
            .arg("models")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(false);
        configure_child_env(&mut command, &self.config.env);
        let mut tree = match OwnedProcessTree::spawn(&mut command).await {
            Ok(tree) => tree,
            Err(error) => {
                tracing::debug!(provider = "deveco", %error, "model discovery process failed to start");
                return Vec::new();
            }
        };
        let Some(stdout) = tree.child_mut().stdout.take() else {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            return Vec::new();
        };
        let mut reader = tokio::spawn(async move {
            let mut output = Vec::new();
            let bytes = stdout
                .take(DISCOVERY_OUTPUT_MAX.saturating_add(1))
                .read_to_end(&mut output)
                .await?;
            Ok::<_, io::Error>((bytes, output))
        });
        let timeout = if timeout.is_zero() {
            DISCOVERY_TIMEOUT
        } else {
            timeout
        };
        let outcome = tokio::select! {
            status = tree.wait() => {
                let _ = status;
                DiscoveryOutcome::Completed
            },
            () = cancellation.cancelled() => DiscoveryOutcome::Cancelled,
            () = tokio::time::sleep(timeout) => DiscoveryOutcome::TimedOut,
        };
        if !matches!(outcome, DiscoveryOutcome::Completed) {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
        }
        let output = tokio::time::timeout(KILL_GRACE, &mut reader)
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(Result::ok)
            .filter(|(bytes, _)| u64::try_from(*bytes).is_ok_and(|n| n <= DISCOVERY_OUTPUT_MAX))
            .map(|(_, output)| output)
            .unwrap_or_default();
        if !reader.is_finished() {
            reader.abort();
        }
        if !matches!(outcome, DiscoveryOutcome::Completed) {
            return Vec::new();
        }
        parse_deveco_models(&String::from_utf8_lossy(&output))
    }
}

#[derive(Debug)]
enum DiscoveryOutcome {
    Completed,
    Cancelled,
    TimedOut,
}

pub fn build_deveco_args(prompt: &str, options: &ExecOptions) -> Vec<String> {
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
    // DevEco's Go contract intentionally consumes only per-agent custom_args;
    // daemon-wide ExtraArgs are not supported for this runtime.
    args.extend(filter_custom_args(&options.custom_args, &BLOCKED_ARGS).args);
    args.push(prompt.to_string());
    args
}

#[async_trait]
impl Backend for DevecoBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        let command_path = if self.config.command.path.is_empty() {
            "deveco"
        } else {
            self.config.command.path.as_str()
        };
        let prefix = filter_launch_prefix(&self.config.command.prefix, &BLOCKED_ARGS);
        let mut argv = prefix.args;
        argv.extend(build_deveco_args(prompt, &options));

        let mut command = Command::new(command_path);
        command
            .args(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);
        configure_child_env(&mut command, &self.config.env);
        if !options.cwd.is_empty() {
            command.current_dir(&options.cwd).env("PWD", &options.cwd);
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
        let stdout = tree.child_mut().stdout.take().ok_or_else(|| {
            AgentError::Protocol("DevEco stdout pipe unavailable after spawn".to_string())
        })?;
        let stderr = tree.child_mut().stderr.take().ok_or_else(|| {
            AgentError::Protocol("DevEco stderr pipe unavailable after spawn".to_string())
        })?;

        let (message_tx, message_rx) = mpsc::channel(MESSAGE_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();
        let cancellation = options.cancellation.clone();
        let timeout = options.timeout;
        let configured_model = options.model.clone();
        let started = Instant::now();
        let stderr_tail = SharedDiagnosticBuffer::new(DEFAULT_TAIL_BYTES);
        let stderr_reader_tail = stderr_tail.clone();
        let mut events_task = tokio::spawn(read_events(stdout, message_tx));
        let mut stderr_task = tokio::spawn(pump_stderr(stderr, stderr_reader_tail));

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
                    let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
                    (RunEnd::Cancelled, Ok(success_exit_status()))
                }
                RunOutcome::TimedOut => {
                    let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
                    (RunEnd::DeadlineExceeded, Ok(success_exit_status()))
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
                tracing::debug!(provider = "deveco", %stderr, "agent stderr captured");
            }

            let mut status = state.status;
            let mut error = state.error;
            if matches!(run_end, RunEnd::DeadlineExceeded) {
                status = "timeout".to_string();
                error = format!("deveco timed out after {}", format_duration(timeout));
            } else if matches!(run_end, RunEnd::Cancelled) {
                status = "aborted".to_string();
                error = "execution cancelled".to_string();
            } else if status == "completed" {
                if let Some(exit_error) = exit_error(exit.as_ref()) {
                    status = "failed".to_string();
                    error = format!("deveco exited with error: {exit_error}");
                }
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
struct DevecoEventResult {
    status: String,
    error: String,
    output: String,
    session_id: String,
    usage: TokenUsage,
}

async fn read_events(stdout: ChildStdout, messages: mpsc::Sender<Message>) -> DevecoEventResult {
    let mut reader = AgentLineReader::new(BufReader::new(stdout));
    let mut state = DevecoEventResult {
        status: "completed".to_string(),
        ..DevecoEventResult::default()
    };
    loop {
        let line = match reader.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => return state,
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
        let Ok(event) = serde_json::from_str::<DevecoEvent>(line) else {
            continue;
        };
        if !event.session_id.is_empty() {
            state.session_id = event.session_id.clone();
        }
        match event.event_type.as_str() {
            "step_start" => send_message(&messages, empty_message(MessageType::Status, "running")),
            "text" => {
                if !event.part.text.is_empty() {
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
            "tool_use" => handle_tool_event(&event.part, &messages),
            "error" => {
                let error = event
                    .error
                    .as_ref()
                    .map(DevecoError::message)
                    .filter(|message| !message.is_empty())
                    .unwrap_or_else(|| "unknown deveco error".to_string());
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
            "step_finish" => {
                if let Some(tokens) = event.part.tokens {
                    state.usage.input_tokens += tokens.input;
                    state.usage.output_tokens += tokens.output;
                    if let Some(cache) = tokens.cache {
                        state.usage.cache_read_tokens += cache.read;
                        state.usage.cache_write_tokens += cache.write;
                    }
                }
            }
            _ => {}
        }
    }
}

fn handle_tool_event(part: &DevecoPart, messages: &mpsc::Sender<Message>) {
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

async fn join_events(task: &mut JoinHandle<DevecoEventResult>) -> DevecoEventResult {
    match tokio::time::timeout(KILL_GRACE, &mut *task).await {
        Ok(Ok(state)) => state,
        Ok(Err(error)) => DevecoEventResult {
            status: "failed".to_string(),
            error: format!("event stream task failed: {error}"),
            ..DevecoEventResult::default()
        },
        Err(_) => {
            task.abort();
            DevecoEventResult {
                status: "failed".to_string(),
                error: "deveco event stream did not terminate".to_string(),
                ..DevecoEventResult::default()
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
struct DevecoEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(rename = "sessionID", default)]
    session_id: String,
    #[serde(default)]
    part: DevecoPart,
    #[serde(default)]
    error: Option<DevecoError>,
}

#[derive(Debug, Default, Deserialize)]
struct DevecoPart {
    #[serde(default)]
    text: String,
    #[serde(default)]
    tool: String,
    #[serde(rename = "callID", default)]
    call_id: String,
    #[serde(default)]
    state: Option<DevecoToolState>,
    #[serde(default)]
    tokens: Option<DevecoTokens>,
}

#[derive(Debug, Deserialize)]
struct DevecoToolState {
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
struct DevecoTokens {
    #[serde(default)]
    input: i64,
    #[serde(default)]
    output: i64,
    #[serde(default)]
    cache: Option<DevecoCacheTokens>,
}

#[derive(Debug, Deserialize)]
struct DevecoCacheTokens {
    #[serde(default)]
    read: i64,
    #[serde(default)]
    write: i64,
}

#[derive(Debug, Deserialize)]
struct DevecoError {
    #[serde(default)]
    name: String,
    #[serde(default)]
    data: Option<DevecoErrorData>,
}

#[derive(Debug, Deserialize)]
struct DevecoErrorData {
    #[serde(default)]
    message: String,
}

impl DevecoError {
    fn message(&self) -> String {
        if let Some(data) = self.data.as_ref() {
            if !data.message.is_empty() {
                return data.message.clone();
            }
        }
        self.name.clone()
    }
}

pub fn parse_deveco_models(output: &str) -> Vec<Model> {
    let mut models = Vec::new();
    let mut seen = BTreeSet::new();
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let Some(id) = fields.first().copied() else {
            continue;
        };
        if id.starts_with(['{', '[', '"'])
            || !id.contains('/')
            || id == id.to_uppercase()
            || !seen.insert(id.to_string())
        {
            continue;
        }
        let provider = id
            .split_once('/')
            .map(|(provider, _)| provider)
            .filter(|provider| !provider.is_empty())
            .unwrap_or_default();
        models.push(Model {
            id: id.to_string(),
            label: id.to_string(),
            provider: provider.to_string(),
            ..Model::default()
        });
    }
    models
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use tokio_util::sync::CancellationToken;

    use super::*;

    #[test]
    fn args_keep_deveco_positional_prompt_and_owned_protocol() {
        let options = ExecOptions {
            cwd: "/tmp/task".to_string(),
            model: "deveco/GLM-5.1".to_string(),
            thinking_level: "high".to_string(),
            resume_session_id: "session-1".to_string(),
            custom_args: vec![
                "--format".to_string(),
                "text".to_string(),
                "--dir".to_string(),
                "/evil".to_string(),
                "--keep".to_string(),
            ],
            ..ExecOptions::default()
        };
        let args = build_deveco_args("do the thing", &options);
        let expected: Vec<String> = [
            "run",
            "--format",
            "json",
            "--dangerously-skip-permissions",
            "--dir",
            "/tmp/task",
            "--model",
            "deveco/GLM-5.1",
            "--variant",
            "high",
            "--session",
            "session-1",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        assert_eq!(&args[..12], expected.as_slice());
        assert!(args.contains(&"--keep".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("do the thing"));
        assert!(!args.iter().any(|arg| arg == "--prompt"));
        assert!(!args.iter().any(|arg| arg == "text" || arg == "/evil"));
    }

    #[test]
    fn model_parser_skips_headers_json_noise_and_duplicates() {
        let models = parse_deveco_models(
            "PROVIDER/MODEL\nanthropic/claude-sonnet-4\tClaude\nopenai/gpt-5\nopenai/gpt-5\n{\"models\":[]}\n",
        );
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "anthropic/claude-sonnet-4");
        assert_eq!(models[0].provider, "anthropic");
        assert_eq!(models[1].label, "openai/gpt-5");
    }

    #[cfg(unix)]
    fn fake_backend(script: &str) -> (tempfile::TempDir, DevecoBackend) {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let executable = directory.path().join("deveco");
        std::fs::write(&executable, script).unwrap_or_else(|error| panic!("write fake: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake: {error}"));
        (
            directory,
            DevecoBackend::new(DevecoConfig {
                command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
                ..DevecoConfig::default()
            }),
        )
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_delivers_events_args_usage_and_session() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
printf '%s\n' '{"type":"step_start","sessionID":"ses_fake"}'
printf '%s\n' '{"type":"text","sessionID":"ses_fake","part":{"text":"ok"}}'
printf '%s\n' '{"type":"step_finish","sessionID":"ses_fake","part":{"tokens":{"input":7,"output":3}}}'
"#,
        );
        let session = backend
            .execute(
                "do the thing",
                ExecOptions {
                    cwd: "/tmp".to_string(),
                    model: "deveco/GLM-5.1".to_string(),
                    timeout: Duration::from_secs(5),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute fake DevEco: {error}"));
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
        assert_eq!(result.session_id, "ses_fake");
        assert_eq!(result.usage["deveco/GLM-5.1"].input_tokens, 7);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_pairs_tool_events_and_promotes_provider_errors() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
printf '%s\n' '{"type":"step_start","sessionID":"ses-tool"}'
printf '%s\n' '{"type":"tool_use","sessionID":"ses-tool","part":{"tool":"bash","callID":"call-1","state":{"status":"completed","input":{"command":"pwd"},"output":"/tmp"}}}'
printf '%s\n' '{"type":"error","sessionID":"ses-tool","error":{"name":"UnknownError","data":{"message":"Model not found"}}}'
"#,
        );
        let session = backend
            .execute("run", ExecOptions::default())
            .await
            .unwrap_or_else(|error| panic!("execute fake DevEco: {error}"));
        let Session {
            mut messages,
            result,
        } = session;
        let mut saw_tool = false;
        let mut saw_tool_result = false;
        let mut saw_error = false;
        while let Some(message) = messages.recv().await {
            saw_tool |= message.message_type == MessageType::ToolUse
                && message.call_id == "call-1"
                && message.input.get("command").and_then(Value::as_str) == Some("pwd");
            saw_tool_result |= message.message_type == MessageType::ToolResult
                && message.call_id == "call-1"
                && message.output == "/tmp";
            saw_error |=
                message.message_type == MessageType::Error && message.content == "Model not found";
        }
        let result = result
            .await
            .unwrap_or_else(|error| panic!("receive result: {error}"));
        assert!(saw_tool);
        assert!(saw_tool_result);
        assert!(saw_error);
        assert_eq!(result.status, "failed");
        assert_eq!(result.error, "Model not found");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn discovery_runs_models_and_scopes_cache() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
if [ "$1" = "models" ]; then
  printf '%s\n' 'PROVIDER/MODEL' 'anthropic/claude-sonnet-4 Claude' 'openai/gpt-5'
fi
"#,
        );
        let catalog = backend
            .discover_models_for_runtime(
                "deveco-test",
                &CatalogCache::default(),
                CancellationToken::new(),
                Duration::from_secs(5),
            )
            .await;
        assert_eq!(catalog.models.len(), 2);
        assert!(catalog
            .models
            .iter()
            .any(|model| model.id == "openai/gpt-5"));
    }

    #[test]
    fn parser_output_ids_are_unique_and_ordered() {
        let ids: BTreeSet<_> = parse_deveco_models("a/x\nb/y\na/x\n")
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, BTreeSet::from(["a/x".to_string(), "b/y".to_string()]));
    }
}
