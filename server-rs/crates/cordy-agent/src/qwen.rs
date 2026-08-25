//! Qwen Code's native non-interactive stream-JSON adapter.

use std::collections::BTreeMap;
use std::io;
use std::process::{ExitStatus, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinError;

use crate::command::{filter_custom_args, filter_launch_prefix, BlockedArgMode, RuntimeCommand};
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
use crate::env::configure_child_env;
use crate::mcp::{managed_object, write_managed_temp};
use crate::process::OwnedProcessTree;
use crate::stderr::{with_stderr, SharedDiagnosticBuffer, DEFAULT_TAIL_BYTES};
use crate::stream::{
    finalize_stream, resume_was_rejected, AgentLineReader, AssistantTurn, RunEnd, TerminalState,
};

const MESSAGE_BUFFER: usize = 256;
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const KILL_GRACE: Duration = Duration::from_secs(10);

static BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> = LazyLock::new(|| {
    BTreeMap::from([
        ("-p", BlockedArgMode::WithValue),
        ("--prompt", BlockedArgMode::WithValue),
        ("-i", BlockedArgMode::WithValue),
        ("--prompt-interactive", BlockedArgMode::WithValue),
        ("-o", BlockedArgMode::WithValue),
        ("--output-format", BlockedArgMode::WithValue),
        ("-m", BlockedArgMode::WithValue),
        ("--model", BlockedArgMode::WithValue),
        ("-r", BlockedArgMode::WithValue),
        ("--resume", BlockedArgMode::WithValue),
        ("-c", BlockedArgMode::Standalone),
        ("--continue", BlockedArgMode::Standalone),
        ("--chat-recording", BlockedArgMode::WithValue),
        ("--mcp-config", BlockedArgMode::WithValue),
        ("--safe-mode", BlockedArgMode::Standalone),
        ("--yolo", BlockedArgMode::Standalone),
        ("-y", BlockedArgMode::Standalone),
        ("--approval-mode", BlockedArgMode::WithValue),
        ("--core-tools", BlockedArgMode::WithValue),
    ])
});

#[derive(Debug, Clone, Default)]
pub struct QwenConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct QwenBackend {
    config: QwenConfig,
}

impl QwenBackend {
    pub fn new(config: QwenConfig) -> Self {
        Self { config }
    }
}

pub fn build_qwen_args(prompt: &str, options: &ExecOptions) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        prompt.to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
    ];
    if !options.model.is_empty() {
        args.extend(["--model".to_string(), options.model.clone()]);
    }
    if !options.resume_session_id.is_empty() {
        args.extend(["--resume".to_string(), options.resume_session_id.clone()]);
    }
    // Non-interactive Qwen filters approval-requiring tools unless bypass is
    // active. Permission mode is daemon-owned for parity with every backend.
    args.push("--yolo".to_string());
    args.extend(filter_custom_args(&options.extra_args, &BLOCKED_ARGS).args);
    args.extend(filter_custom_args(&options.custom_args, &BLOCKED_ARGS).args);
    args
}

#[async_trait]
impl Backend for QwenBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        managed_object(options.mcp_config.as_ref()).map_err(AgentError::InvalidConfig)?;

        let command_path = if self.config.command.path.is_empty() {
            "qwen"
        } else {
            self.config.command.path.as_str()
        };
        let prefix = filter_launch_prefix(&self.config.command.prefix, &BLOCKED_ARGS);
        log_blocked("launch prefix", &prefix.blocked_flags);
        let mut argv = prefix.args;
        let provider_args = build_qwen_args(prompt, &options);
        log_blocked_args(&options);
        argv.extend(provider_args);

        let mut mcp_file = write_managed_temp(options.mcp_config.as_ref(), "cordy-qwen-mcp-")?;
        if let Some(file) = mcp_file.as_ref() {
            let path = file.path().to_str().ok_or_else(|| {
                AgentError::InvalidConfig("Qwen MCP path is not valid UTF-8".to_string())
            })?;
            argv.extend(["--mcp-config".to_string(), path.to_string()]);
        }

        let mut command = Command::new(command_path);
        command
            .args(&argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);
        configure_child_env(&mut command, &self.config.env);
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
        let stdout = tree.child_mut().stdout.take().ok_or_else(|| {
            AgentError::Protocol("Qwen stdout pipe unavailable after spawn".to_string())
        })?;
        let stderr = tree.child_mut().stderr.take().ok_or_else(|| {
            AgentError::Protocol("Qwen stderr pipe unavailable after spawn".to_string())
        })?;

        let (message_tx, message_rx) = mpsc::channel(MESSAGE_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();
        let started = Instant::now();
        let timeout = options.timeout;
        let cancellation = options.cancellation.clone();
        let requested_resume = options.resume_session_id.clone();
        let configured_model = options.model.clone();
        let stderr_tail = SharedDiagnosticBuffer::new(DEFAULT_TAIL_BYTES);
        let stderr_reader_tail = stderr_tail.clone();

        tokio::spawn(async move {
            // Keeping this guard in the supervisor task guarantees removal on
            // every terminal path, including timeout and cancellation.
            let _mcp_file = mcp_file.take();
            let mut stderr_task = tokio::spawn(pump_stderr(stderr, stderr_reader_tail));
            let mut stdout_task = tokio::spawn(read_stream(stdout, message_tx, configured_model));

            let end = {
                let completion = async {
                    let exit = tree.wait().await;
                    let stream = (&mut stdout_task).await;
                    (exit, stream)
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

            let (run_end, exit, stream) = match end {
                RunOutcome::Completed((exit, stream)) => (RunEnd::Completed, exit, stream),
                RunOutcome::Cancelled => {
                    let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
                    (
                        RunEnd::Cancelled,
                        Ok(success_exit_status()),
                        (&mut stdout_task).await,
                    )
                }
                RunOutcome::TimedOut => {
                    let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
                    (
                        RunEnd::DeadlineExceeded,
                        Ok(success_exit_status()),
                        (&mut stdout_task).await,
                    )
                }
            };

            if tokio::time::timeout(KILL_GRACE, &mut stderr_task)
                .await
                .is_err()
            {
                stderr_task.abort();
            }
            let stderr = stderr_tail.tail();
            if !stderr.is_empty() {
                tracing::debug!(provider = "qwen", stderr = %stderr, "agent stderr captured");
            }

            let mut state = stream.unwrap_or_else(join_failure_state);
            let exit_error = exit_error(exit.as_ref());
            let finalized = finalize_stream(
                "qwen",
                timeout,
                run_end,
                None,
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
                with_stderr(&finalized.error, "qwen", &stderr)
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
        tracing::warn!(provider = "qwen", source, flags = ?flags, "ignored daemon-owned arguments");
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

async fn read_stream(
    stdout: tokio::process::ChildStdout,
    messages: mpsc::Sender<Message>,
    configured_model: String,
) -> QwenStreamState {
    let mut reader = AgentLineReader::new(BufReader::new(stdout));
    let mut state = QwenStreamState {
        model: configured_model,
        ..QwenStreamState::default()
    };
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<QwenStreamEvent>(line) {
                    Ok(event) => handle_event(event, &messages, &mut state),
                    Err(_) => state.invalid_event_count += 1,
                }
            }
            Ok(None) => return state,
            Err(error) => {
                state.terminal.scan_error = error.to_string();
                return state;
            }
        }
    }
}

enum RunOutcome {
    Completed((io::Result<ExitStatus>, Result<QwenStreamState, JoinError>)),
    Cancelled,
    TimedOut,
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

fn exit_error(exit: Result<&ExitStatus, &io::Error>) -> Option<String> {
    match exit {
        Ok(status) if status.success() => None,
        Ok(status) => Some(status.to_string()),
        Err(error) => Some(format!("wait failed: {error}")),
    }
}

fn join_failure_state(error: JoinError) -> QwenStreamState {
    QwenStreamState {
        terminal: TerminalState {
            scan_error: format!("stream task failed: {error}"),
            ..TerminalState::default()
        },
        ..QwenStreamState::default()
    }
}

#[derive(Debug, Deserialize)]
struct QwenStreamEvent {
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
    result: String,
    #[serde(default)]
    is_error: bool,
    usage: Option<QwenUsage>,
    #[serde(default)]
    error: Value,
}

#[derive(Debug, Deserialize)]
struct QwenPayload {
    #[serde(default)]
    model: String,
    #[serde(default)]
    content: Vec<QwenContentBlock>,
    usage: Option<QwenUsage>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct QwenUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    cache_read_input_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct QwenContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    thinking: String,
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

#[derive(Debug, Default)]
struct QwenStreamState {
    session_id: String,
    model: String,
    terminal: TerminalState,
    usage: BTreeMap<String, TokenUsage>,
    invalid_event_count: usize,
}

fn handle_event(
    event: QwenStreamEvent,
    messages: &mpsc::Sender<Message>,
    state: &mut QwenStreamState,
) {
    if !event.session_id.is_empty() {
        state.session_id = event.session_id.clone();
    }
    if !event.model.is_empty() {
        state.model = event.model.clone();
    }
    match event.event_type.as_str() {
        "system" => send_message(
            messages,
            Message {
                message_type: MessageType::Status,
                status: "running".to_string(),
                session_id: state.session_id.clone(),
                ..empty_message(MessageType::Status)
            },
        ),
        "assistant" => handle_assistant(event.message, messages, state),
        "user" => handle_user(event.message, messages),
        "result" => {
            state.terminal.saw_result = true;
            state.terminal.result_is_error =
                event.is_error || matches!(event.subtype.as_str(), "error" | "failed");
            state.terminal.final_result_text = if state.terminal.result_is_error {
                error_text(&event)
            } else {
                event.result
            };
            if let Some(usage) = event.usage {
                set_usage(&mut state.usage, &state.model, usage);
            }
        }
        "error" => {
            state.terminal.saw_result = true;
            state.terminal.result_is_error = true;
            state.terminal.final_result_text = error_text(&event);
        }
        _ => {}
    }
}

fn handle_assistant(raw: Value, messages: &mpsc::Sender<Message>, state: &mut QwenStreamState) {
    let Ok(payload) = serde_json::from_value::<QwenPayload>(raw) else {
        state.terminal.last_assistant_text.clear();
        return;
    };
    if !payload.model.is_empty() {
        state.model = payload.model.clone();
    }
    if let Some(usage) = payload.usage {
        set_usage(&mut state.usage, &payload.model, usage);
    }
    let mut turn = AssistantTurn {
        understood: true,
        ..AssistantTurn::default()
    };
    for block in payload.content {
        match block.block_type.as_str() {
            "thinking" if !block.thinking.is_empty() => send_message(
                messages,
                Message {
                    content: block.thinking,
                    ..empty_message(MessageType::Thinking)
                },
            ),
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
            "thinking" | "text" => {}
            _ => turn.understood = false,
        }
    }
    state.terminal.last_assistant_text = turn.resolve_fallback(&state.terminal.last_assistant_text);
}

fn handle_user(raw: Value, messages: &mpsc::Sender<Message>) {
    let Ok(payload) = serde_json::from_value::<QwenPayload>(raw) else {
        return;
    };
    for block in payload.content {
        if block.block_type == "tool_result" {
            send_message(
                messages,
                Message {
                    call_id: block.tool_use_id,
                    output: match block.content {
                        Value::String(text) => text,
                        value => value.to_string(),
                    },
                    ..empty_message(MessageType::ToolResult)
                },
            );
        }
    }
}

fn set_usage(usage: &mut BTreeMap<String, TokenUsage>, model: &str, qwen: QwenUsage) {
    if model.is_empty()
        || (qwen.input_tokens == 0 && qwen.output_tokens == 0 && qwen.cache_read_input_tokens == 0)
    {
        return;
    }
    usage.insert(
        model.to_string(),
        TokenUsage {
            input_tokens: qwen.input_tokens,
            output_tokens: qwen.output_tokens,
            cache_read_tokens: qwen.cache_read_input_tokens,
            ..TokenUsage::default()
        },
    );
}

fn error_text(event: &QwenStreamEvent) -> String {
    if !event.result.is_empty() {
        return event.result.clone();
    }
    if let Some(message) = event.error.get("message").and_then(Value::as_str) {
        return message.to_string();
    }
    if !event.error.is_null() {
        return event.error.to_string();
    }
    "qwen returned an error event without details".to_string()
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
    use std::collections::BTreeSet;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn managed_arguments_cannot_replace_protocol_or_permissions() {
        let options = ExecOptions {
            model: "qwen-test".to_string(),
            resume_session_id: "session-1".to_string(),
            extra_args: vec!["--output-format".to_string(), "text".to_string()],
            custom_args: vec![
                "--model=other".to_string(),
                "--yolo".to_string(),
                "--debug".to_string(),
            ],
            ..ExecOptions::default()
        };
        let args = build_qwen_args("secret prompt", &options);
        assert_eq!(
            &args[..8],
            [
                "-p",
                "secret prompt",
                "--output-format",
                "stream-json",
                "--model",
                "qwen-test",
                "--resume",
                "session-1"
            ]
        );
        assert_eq!(args.iter().filter(|arg| *arg == "--yolo").count(), 1);
        assert!(args.iter().any(|arg| arg == "--debug"));
        assert!(!args
            .iter()
            .any(|arg| arg == "text" || arg == "--model=other"));
    }

    #[test]
    fn native_events_map_to_terminal_result_and_messages() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut state = QwenStreamState::default();
        for line in [
            r#"{"type":"system","session_id":"sess-1","model":"qwen-test"}"#,
            r#"{"type":"assistant","message":{"model":"qwen-test","content":[{"type":"thinking","thinking":"considering"},{"type":"tool_use","id":"call-1","name":"read_file","input":{"path":"AGENTS.md"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"call-1","content":"done"}]}}"#,
            r#"{"type":"assistant","message":{"model":"qwen-test","content":[{"type":"text","text":"PONG"}]}}"#,
            r#"{"type":"result","subtype":"success","result":"PONG","usage":{"input_tokens":20,"output_tokens":4,"cache_read_input_tokens":6}}"#,
        ] {
            let event = serde_json::from_str(line);
            assert!(event.is_ok());
            handle_event(event.unwrap_or_else(|_| unreachable!()), &tx, &mut state);
        }
        assert!(state.terminal.saw_result);
        assert!(!state.terminal.result_is_error);
        assert_eq!(state.terminal.final_result_text, "PONG");
        assert_eq!(state.usage["qwen-test"].input_tokens, 20);
        let mut kinds = BTreeSet::new();
        while let Ok(message) = rx.try_recv() {
            kinds.insert(message.message_type as u8);
        }
        assert_eq!(kinds.len(), 5);
    }

    #[test]
    fn terminal_error_and_resume_rejection_fail_closed() {
        let event: QwenStreamEvent = serde_json::from_str(
            r#"{"type":"result","subtype":"error_during_execution","session_id":"echoed","is_error":true,"error":{"message":"No saved session found with ID redacted"}}"#,
        )
        .unwrap_or_else(|_| unreachable!());
        let (tx, _) = mpsc::channel(1);
        let mut state = QwenStreamState::default();
        handle_event(event, &tx, &mut state);
        assert!(state.terminal.result_is_error);
        assert!(resume_was_rejected(
            "requested",
            &state.session_id,
            true,
            [state.terminal.final_result_text.as_str()]
        ));
    }

    #[test]
    fn managed_mcp_file_is_private_and_exact() {
        let config = serde_json::json!({"mcpServers":{"demo":{"command":"echo"}}});
        let file = write_managed_temp(Some(&config), "cordy-qwen-test-");
        assert!(file.is_ok());
        let file = file.ok().flatten().unwrap_or_else(|| unreachable!());
        let data = std::fs::read(file.path());
        assert!(data.is_ok());
        let decoded: Value =
            serde_json::from_slice(&data.unwrap_or_default()).unwrap_or_else(|_| unreachable!());
        assert_eq!(decoded, config);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = file.as_file().metadata();
            assert!(metadata.is_ok());
            assert_eq!(
                metadata
                    .map(|value| value.permissions().mode() & 0o777)
                    .ok(),
                Some(0o600)
            );
        }
    }

    #[cfg(unix)]
    fn fake_backend(script: &str) -> (tempfile::TempDir, QwenBackend) {
        let directory = tempfile::tempdir()
            .unwrap_or_else(|error| panic!("create fake Qwen directory: {error}"));
        let executable = directory.path().join("qwen");
        std::fs::write(&executable, script)
            .unwrap_or_else(|error| panic!("write fake Qwen executable: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("make fake Qwen executable runnable: {error}"));
        let backend = QwenBackend::new(QwenConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            ..QwenConfig::default()
        });
        (directory, backend)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_owns_real_process_and_delivers_one_terminal_result() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
printf '%s\n' '{"type":"system","session_id":"session-real","model":"qwen-test"}'
printf '%s\n' '{"type":"assistant","message":{"model":"qwen-test","content":[{"type":"text","text":"PONG"}]}}'
printf '%s\n' '{"type":"result","subtype":"success","result":"PONG"}'
"#,
        );
        let session = backend
            .execute("reply PONG", ExecOptions::default())
            .await
            .unwrap_or_else(|error| panic!("execute fake Qwen: {error}"));
        let Session {
            mut messages,
            result,
        } = session;
        let mut saw_text = false;
        while let Some(message) = messages.recv().await {
            saw_text |= message.message_type == MessageType::Text && message.content == "PONG";
        }
        let result = result
            .await
            .unwrap_or_else(|error| panic!("receive fake Qwen result: {error}"));
        assert!(saw_text);
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "PONG");
        assert_eq!(result.session_id, "session-real");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_terminates_owned_process_tree() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
sleep 60 &
wait
"#,
        );
        let cancellation = tokio_util::sync::CancellationToken::new();
        let session = backend
            .execute(
                "cancel me",
                ExecOptions {
                    cancellation: cancellation.clone(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute cancellable Qwen: {error}"));
        cancellation.cancel();
        let result = tokio::time::timeout(Duration::from_secs(15), session.result)
            .await
            .unwrap_or_else(|error| panic!("cancellation exceeded bound: {error}"))
            .unwrap_or_else(|error| panic!("receive cancelled Qwen result: {error}"));
        assert_eq!(result.status, "aborted");
        assert!(result.error.contains("cancelled"));
    }
}
