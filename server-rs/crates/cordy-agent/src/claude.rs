//! Claude Code's headless bidirectional stream-JSON adapter.
//!
//! Claude is deliberately kept separate from CodeBuddy even though the two
//! CLIs share most of the wire vocabulary. Their argument ownership,
//! permission response, resume failure, settings, and model-effort contracts
//! are different production APIs.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::command::{filter_custom_args, filter_launch_prefix, BlockedArgMode, RuntimeCommand};
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
use crate::mcp::managed_object;
use crate::model::{
    Catalog, CatalogCache, Model, ModelDiscoveryCacheKey, ModelThinking, ThinkingLevel,
};
use crate::process::OwnedProcessTree;
use crate::stderr::{with_stderr, SharedDiagnosticBuffer, DEFAULT_TAIL_BYTES};
use crate::stream::{finalize_stream, AgentLineReader, AssistantTurn, RunEnd, TerminalState};

const MESSAGE_BUFFER: usize = 256;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVERY_OUTPUT_MAX: usize = 4 * 1024 * 1024;
const TERMINATION_GRACE: Duration = Duration::from_secs(5);
const KILL_GRACE: Duration = Duration::from_secs(10);
const PROMPT_TOO_LONG: &str = "prompt_too_long";

type SharedStdin = Arc<Mutex<Option<ChildStdin>>>;

static EFFORT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"--effort\s*(?:<[^>]+>)?\s*(?:Effort level[^(]*)?\(([^)]+)\)")
        .unwrap_or_else(|error| panic!("invalid Claude effort regex: {error}"))
});

pub(crate) static BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> =
    LazyLock::new(|| {
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
pub struct ClaudeConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ClaudeBackend {
    config: ClaudeConfig,
}

impl ClaudeBackend {
    pub fn new(config: ClaudeConfig) -> Self {
        Self { config }
    }

    /// Returns Claude's static accepted model catalog, augmented with the
    /// effort levels advertised by this exact CLI when help is available.
    pub async fn discover_models(
        &self,
        cache: &CatalogCache,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        self.discover_models_for_runtime("claude", cache, cancellation, timeout)
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
            "claude"
        } else {
            runtime_scope
        };
        let Some(key) = ModelDiscoveryCacheKey::new(scope, &self.config.command) else {
            return static_catalog(None);
        };
        if let Some(catalog) = cache.get(&key) {
            return catalog;
        }
        let timeout = if timeout.is_zero() {
            DISCOVERY_TIMEOUT
        } else {
            timeout
        };
        let help = capture_help(
            &command_path(&self.config.command),
            &self.config.command.prefix,
            &self.config.env,
            cancellation.clone(),
            timeout,
        )
        .await;
        if cancellation.is_cancelled() {
            return Catalog::default();
        }
        let help_text = help.ok().flatten();
        let catalog = static_catalog(help_text.as_deref());
        let _ = cache.insert(key, catalog.clone());
        catalog
    }
}

#[async_trait]
impl Backend for ClaudeBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        managed_object(options.mcp_config.as_ref()).map_err(AgentError::InvalidConfig)?;
        let command_path = command_path(&self.config.command);
        let mut blocked = BLOCKED_ARGS.clone();
        if !options.claude_settings_path.is_empty() {
            blocked.insert("--settings", BlockedArgMode::WithValue);
        }
        let prefix = filter_launch_prefix(&self.config.command.prefix, &blocked);
        log_blocked("launch prefix", &prefix.blocked_flags);
        let extra = filter_custom_args(&options.extra_args, &blocked);
        let custom = filter_custom_args(&options.custom_args, &blocked);
        log_blocked("extra arguments", &extra.blocked_flags);
        log_blocked("custom arguments", &custom.blocked_flags);

        let mut argv = prefix.args;
        argv.extend(build_claude_args_with_blocked(&options, &blocked));
        root_sudo_preflight(&self.config.env)?;

        let mut mcp_file = write_claude_mcp_temp(options.mcp_config.as_ref())?;
        if let Some(file) = mcp_file.as_ref() {
            let path = file.path.to_str().ok_or_else(|| {
                AgentError::InvalidConfig("Claude MCP path is not valid UTF-8".to_string())
            })?;
            argv.extend(["--mcp-config".to_string(), path.to_string()]);
        }

        let mut command = Command::new(&command_path);
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
                    AgentError::ExecutableNotFound(command_path.clone())
                } else {
                    AgentError::Process(error)
                }
            })?;
        let stdin = tree.child_mut().stdin.take().ok_or_else(|| {
            AgentError::Protocol("Claude stdin pipe unavailable after spawn".to_string())
        })?;
        let stdout = tree.child_mut().stdout.take().ok_or_else(|| {
            AgentError::Protocol("Claude stdout pipe unavailable after spawn".to_string())
        })?;
        let stderr = tree.child_mut().stderr.take().ok_or_else(|| {
            AgentError::Protocol("Claude stderr pipe unavailable after spawn".to_string())
        })?;

        let (message_tx, message_rx) = mpsc::channel(MESSAGE_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();
        let timeout = options.timeout;
        let cancellation = options.cancellation.clone();
        let requested_resume = options.resume_session_id.clone();
        let fallback_model = options.model.clone();
        let started = Instant::now();
        let prompt_bytes = prompt_input(prompt)?;
        let stdin = Arc::new(Mutex::new(Some(stdin)));
        let stderr_tail = SharedDiagnosticBuffer::new(DEFAULT_TAIL_BYTES);

        tokio::spawn(async move {
            let _mcp_file = mcp_file.take();
            run_claude(
                tree,
                stdin,
                stdout,
                stderr,
                prompt_bytes,
                message_tx,
                result_tx,
                cancellation,
                timeout,
                requested_resume,
                fallback_model,
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

pub fn build_claude_args(options: &ExecOptions) -> Vec<String> {
    let mut blocked = BLOCKED_ARGS.clone();
    if !options.claude_settings_path.is_empty() {
        blocked.insert("--settings", BlockedArgMode::WithValue);
    }
    build_claude_args_with_blocked(options, &blocked)
}

fn build_claude_args_with_blocked(
    options: &ExecOptions,
    blocked: &BTreeMap<&'static str, BlockedArgMode>,
) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--input-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--permission-mode".to_string(),
        "bypassPermissions".to_string(),
        "--disallowedTools".to_string(),
        "AskUserQuestion".to_string(),
    ];
    if managed_config_present(options.mcp_config.as_ref()) {
        args.push("--strict-mcp-config".to_string());
    }
    if !options.model.is_empty() {
        args.extend(["--model".to_string(), options.model.clone()]);
    }
    if !options.thinking_level.is_empty() {
        args.extend(["--effort".to_string(), options.thinking_level.clone()]);
    }
    if options.max_turns > 0 {
        args.extend(["--max-turns".to_string(), options.max_turns.to_string()]);
    }
    // Claude reads the task-local CLAUDE.md written by the daemon. The Go
    // adapter intentionally does not duplicate system_prompt inline.
    if !options.resume_session_id.is_empty() {
        args.extend(["--resume".to_string(), options.resume_session_id.clone()]);
    }
    args.extend(filter_custom_args(&options.extra_args, blocked).args);
    args.extend(filter_custom_args(&options.custom_args, blocked).args);
    if !options.claude_settings_path.is_empty() {
        args.extend([
            "--settings".to_string(),
            options.claude_settings_path.clone(),
        ]);
    }
    args
}

fn managed_config_present(config: Option<&Value>) -> bool {
    config.is_some_and(|value| !value.is_null())
}

struct ClaudeMcpTemp {
    _directory: tempfile::TempDir,
    path: PathBuf,
}

fn write_claude_mcp_temp(config: Option<&Value>) -> Result<Option<ClaudeMcpTemp>, AgentError> {
    let Some(object) = managed_object(config).map_err(AgentError::InvalidConfig)? else {
        return Ok(None);
    };
    let directory = tempfile::Builder::new()
        .prefix("cordy-claude-mcp-")
        .tempdir()
        .map_err(AgentError::Process)?;
    let value = harden_browser_mcp_config(Value::Object(object.clone()), directory.path())?;
    let path = directory.path().join("mcp-config.json");
    let data = serde_json::to_vec(&value).map_err(|error| {
        AgentError::InvalidConfig(format!("serialize Claude MCP config: {error}"))
    })?;
    write_private_file(&path, &data)?;
    Ok(Some(ClaudeMcpTemp {
        _directory: directory,
        path,
    }))
}

fn write_private_file(path: &Path, data: &[u8]) -> Result<(), AgentError> {
    std::fs::write(path, data).map_err(AgentError::Process)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(AgentError::Process)?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn harden_browser_mcp_config(value: Value, _directory: &Path) -> Result<Value, AgentError> {
    Ok(value)
}

#[cfg(windows)]
fn harden_browser_mcp_config(mut value: Value, directory: &Path) -> Result<Value, AgentError> {
    let Some(servers) = value.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return Ok(value);
    };
    for (name, raw_server) in servers.iter_mut() {
        let Some(server) = raw_server.as_object_mut() else {
            continue;
        };
        let Some(args) = server.get("args").and_then(string_args) else {
            continue;
        };
        let lower_name = name.to_ascii_lowercase();
        if lower_name == "playwright"
            || args.iter().any(|arg| {
                let arg = arg.to_ascii_lowercase();
                arg.contains("@playwright/mcp") || arg.contains("@playwright\\mcp")
            })
        {
            if !args.iter().any(|arg| {
                ["--config", "--cdp-endpoint", "--extension"]
                    .iter()
                    .any(|flag| has_flag(arg, flag))
            }) {
                let config_path = directory.join("playwright-windows-browser.json");
                let browser = serde_json::json!({
                    "browser": {"launchOptions": {"args": ["--disable-gpu"]}}
                });
                let browser_data = serde_json::to_vec(&browser).map_err(|error| {
                    AgentError::InvalidConfig(format!("serialize Playwright MCP config: {error}"))
                })?;
                write_private_file(&config_path, &browser_data)?;
                let mut next = args;
                next.extend([
                    "--config".to_string(),
                    config_path.to_string_lossy().into_owned(),
                ]);
                server.insert("args".to_string(), serde_json::json!(next));
            }
        } else if (lower_name == "chrome-devtools"
            || args
                .iter()
                .any(|arg| arg.to_ascii_lowercase().contains("chrome-devtools-mcp")))
            && !args.iter().any(|arg| chrome_devtools_override(arg))
        {
            if let Some(path) = windows_chromium_fallback_executable() {
                let mut next = args;
                next.push(format!("--executablePath={}", path.to_string_lossy()));
                server.insert("args".to_string(), serde_json::json!(next));
            }
        }
    }
    Ok(value)
}

#[cfg(windows)]
fn string_args(value: &Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|value| value.as_str().map(ToString::to_string))
        .collect()
}

#[cfg(windows)]
fn has_flag(argument: &str, flag: &str) -> bool {
    argument == flag || argument.starts_with(&format!("{flag}="))
}

#[cfg(windows)]
fn chrome_devtools_override(argument: &str) -> bool {
    [
        "--executablePath",
        "--executable-path",
        "-e",
        "--channel",
        "--browserUrl",
        "--browser-url",
        "-u",
        "--wsEndpoint",
        "--ws-endpoint",
        "-w",
        "--autoConnect",
        "--auto-connect",
    ]
    .iter()
    .any(|flag| has_flag(argument, flag))
}

#[cfg(windows)]
fn windows_chromium_fallback_executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CORDY_CHROME_DEVTOOLS_EXECUTABLE_PATH") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    ["ProgramFiles(x86)", "ProgramFiles", "LocalAppData"]
        .iter()
        .filter_map(|key| std::env::var_os(key))
        .map(|root| {
            PathBuf::from(root)
                .join("Microsoft")
                .join("Edge")
                .join("Application")
                .join("msedge.exe")
        })
        .find(|path| path.is_file())
}

async fn run_claude(
    mut tree: OwnedProcessTree,
    stdin: SharedStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
    prompt: Vec<u8>,
    messages: mpsc::Sender<Message>,
    result_tx: oneshot::Sender<ExecutionResult>,
    cancellation: CancellationToken,
    timeout: Duration,
    requested_resume: String,
    fallback_model: String,
    started: Instant,
    stderr_tail: SharedDiagnosticBuffer,
) {
    let mut stderr_task = tokio::spawn(pump_stderr(stderr, stderr_tail.clone()));
    let prompt_stdin = Arc::clone(&stdin);
    let mut prompt_task = tokio::spawn(async move { write_stdin(prompt_stdin, &prompt).await });
    let mut stdout_task = tokio::spawn(read_stream(
        stdout,
        Arc::clone(&stdin),
        messages,
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
    let mut state = stream.unwrap_or_else(join_failure_state);
    let write_error = write_error(write);
    let exit_error = exit
        .as_ref()
        .and_then(|status| process_exit_error(status.as_ref()));
    let guard = state.saw_async_launch.then_some(
        "claude launched an async background task; Cordy-managed runs require foreground execution",
    );
    let finalized = finalize_stream(
        "claude",
        timeout,
        run_end,
        write_error.as_deref(),
        exit_error.as_deref(),
        &state.session_id,
        &state.terminal,
        guard,
    );
    let failed = finalized.status == "failed";
    let resume_rejected = claude_resume_was_rejected(
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
        with_stderr(&finalized.error, "claude", &stderr)
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
}

async fn read_stream(
    stdout: ChildStdout,
    stdin: SharedStdin,
    messages: mpsc::Sender<Message>,
    fallback_model: String,
) -> ClaudeStreamState {
    let mut reader = AgentLineReader::new(BufReader::new(stdout));
    let mut state = ClaudeStreamState::default();
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<ClaudeEvent>(line) {
                    Ok(event) => {
                        handle_event(event, &stdin, &messages, &fallback_model, &mut state).await;
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

async fn handle_event(
    event: ClaudeEvent,
    stdin: &SharedStdin,
    messages: &mpsc::Sender<Message>,
    fallback_model: &str,
    state: &mut ClaudeStreamState,
) {
    state.event_count = state.event_count.saturating_add(1);
    match event.event_type.as_str() {
        "assistant" => {
            state.assistant_event_count = state.assistant_event_count.saturating_add(1);
            let turn = handle_assistant(event.message, messages, &mut state.usage);
            state.tool_use_count = state.tool_use_count.saturating_add(turn.tool_uses);
            if !turn.understood {
                state.unreadable_assistant_count =
                    state.unreadable_assistant_count.saturating_add(1);
            }
            state.terminal.last_assistant_text =
                turn.resolve_fallback(&state.terminal.last_assistant_text);
        }
        "user" => {
            state.saw_async_launch |= handle_user(event.message, messages);
        }
        "system" => {
            if !event.session_id.is_empty() {
                state.session_id = event.session_id.clone();
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
            state.terminal.saw_result = true;
            state.terminal.result_is_error = event.is_error;
            state.terminal.final_result_text = event.result_text.clone();
            state.terminal.terminal_reason_error =
                terminal_reason_failure(&event.terminal_reason, &state.terminal.final_result_text);
            if !event.session_id.is_empty() {
                state.session_id = event.session_id.clone();
            }
            if let Some(usage) = result_usage(&event, fallback_model) {
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
                    tracing::warn!(provider = "claude", %error, "write control response failed");
                }
            }
        }
        _ => {}
    }
}

fn handle_assistant(
    raw: Value,
    messages: &mpsc::Sender<Message>,
    usage: &mut BTreeMap<String, TokenUsage>,
) -> AssistantTurn {
    let Ok(payload) = serde_json::from_value::<ClaudePayload>(raw) else {
        return AssistantTurn::default();
    };
    if let Some(value) = payload.usage {
        add_usage(usage, &payload.model, value);
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
                turn.tool_uses = turn.tool_uses.saturating_add(1);
                send_message(
                    messages,
                    Message {
                        tool: block.name,
                        call_id: block.id,
                        input: block
                            .input
                            .as_object()
                            .cloned()
                            .unwrap_or_default()
                            .into_iter()
                            .collect(),
                        ..empty_message(MessageType::ToolUse)
                    },
                );
            }
            "text" | "thinking" => {}
            _ => turn.understood = false,
        }
    }
    turn
}

fn handle_user(raw: Value, messages: &mpsc::Sender<Message>) -> bool {
    let Ok(payload) = serde_json::from_value::<ClaudePayload>(raw) else {
        return false;
    };
    let mut saw_async_launch = false;
    for block in payload.content {
        if block.block_type != "tool_result" {
            continue;
        }
        saw_async_launch |= has_async_launch(&block.content);
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
    saw_async_launch
}

fn control_response(event: &ClaudeEvent) -> Option<Vec<u8>> {
    let mut input = event
        .request
        .get("input")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if input
        .get("run_in_background")
        .and_then(Value::as_bool)
        .is_some_and(|value| value)
    {
        input.insert("run_in_background".to_string(), Value::Bool(false));
    }
    let mut response = serde_json::to_vec(&serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": event.request_id,
            "response": {
                "behavior": "allow",
                "updatedInput": input,
            },
        },
    }))
    .ok()?;
    response.push(b'\n');
    Some(response)
}

fn has_async_launch(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status == "async_launched")
                || object.get("content").is_some_and(has_async_launch)
        }
        Value::Array(values) => values.iter().any(has_async_launch),
        _ => false,
    }
}

fn terminal_reason_failure(reason: &str, result: &str) -> String {
    if reason.trim() != PROMPT_TOO_LONG {
        return String::new();
    }
    let mut message = format!(
        "claude ended the turn with terminal_reason={PROMPT_TOO_LONG}: the session's context window is exhausted and compaction could not recover it"
    );
    if !result.trim().is_empty() {
        message.push_str(" (");
        message.push_str(result.trim());
        message.push(')');
    }
    message
}

fn result_usage(event: &ClaudeEvent, fallback_model: &str) -> Option<BTreeMap<String, TokenUsage>> {
    let models: BTreeMap<String, TokenUsage> = event
        .model_usage
        .iter()
        .filter(|(model, usage)| !model.is_empty() && usage.has_tokens())
        .map(|(model, usage)| (model.clone(), usage.normalized()))
        .collect();
    if !models.is_empty() {
        return Some(models);
    }
    let usage = event.usage.filter(ClaudeUsage::has_tokens)?;
    let model = if event.model.is_empty() {
        fallback_model
    } else {
        event.model.as_str()
    };
    (!model.is_empty()).then(|| BTreeMap::from([(model.to_string(), usage.normalized())]))
}

fn claude_resume_was_rejected<'a>(
    requested: &str,
    emitted: &str,
    failed: bool,
    texts: impl IntoIterator<Item = &'a str>,
) -> bool {
    if !failed || requested.is_empty() {
        return false;
    }
    const PHRASES: &[&str] = &[
        "no conversation found",
        "no saved session found",
        "已绑定另外",
        "bound to another account",
        "bound to a different account",
    ];
    if texts.into_iter().any(|text| {
        let text = text.to_ascii_lowercase();
        PHRASES.iter().any(|phrase| text.contains(phrase))
    }) {
        return true;
    }
    !emitted.is_empty() && emitted != requested
}

fn add_usage(usage: &mut BTreeMap<String, TokenUsage>, model: &str, value: ClaudeUsage) {
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

#[derive(Debug, Deserialize)]
struct ClaudeEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    message: Value,
    #[serde(default, alias = "sessionId")]
    session_id: String,
    #[serde(default)]
    model: String,
    #[serde(default, rename = "result")]
    result_text: String,
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    terminal_reason: String,
    usage: Option<ClaudeUsage>,
    #[serde(default, rename = "modelUsage")]
    model_usage: BTreeMap<String, ClaudeResultUsage>,
    log: Option<ClaudeLog>,
    #[serde(default, rename = "request_id", alias = "requestId")]
    request_id: String,
    #[serde(default)]
    request: Value,
}

#[derive(Debug, Deserialize)]
struct ClaudePayload {
    #[serde(default)]
    model: String,
    #[serde(default)]
    content: Vec<ClaudeBlock>,
    usage: Option<ClaudeUsage>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct ClaudeUsage {
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
struct ClaudeResultUsage {
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
struct ClaudeBlock {
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
    #[serde(default, rename = "tool_use_id")]
    tool_use_id: String,
    #[serde(default)]
    content: Value,
}

#[derive(Debug, Deserialize)]
struct ClaudeLog {
    #[serde(default)]
    level: String,
    #[serde(default)]
    message: String,
}

impl ClaudeUsage {
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

impl ClaudeResultUsage {
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

#[derive(Debug, Default)]
struct ClaudeStreamState {
    session_id: String,
    terminal: TerminalState,
    usage: BTreeMap<String, TokenUsage>,
    saw_async_launch: bool,
    event_count: usize,
    invalid_event_count: usize,
    assistant_event_count: usize,
    tool_use_count: usize,
    unreadable_assistant_count: usize,
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

async fn write_stdin(stdin: SharedStdin, bytes: &[u8]) -> io::Result<()> {
    let mut guard = stdin.lock().await;
    let writer = guard
        .as_mut()
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "Claude stdin closed"))?;
    writer.write_all(bytes).await?;
    writer.flush().await
}

async fn close_stdin(stdin: &SharedStdin) {
    stdin.lock().await.take();
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

enum RunOutcome {
    Completed(
        (
            io::Result<ExitStatus>,
            Result<ClaudeStreamState, JoinError>,
            Result<io::Result<()>, JoinError>,
        ),
    ),
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

fn join_failure_state(error: JoinError) -> ClaudeStreamState {
    ClaudeStreamState {
        terminal: TerminalState {
            scan_error: format!("stream task failed: {error}"),
            ..TerminalState::default()
        },
        ..ClaudeStreamState::default()
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
    .map_err(|error| AgentError::Protocol(format!("serialize Claude input: {error}")))?;
    payload.push(b'\n');
    Ok(payload)
}

fn command_path(command: &RuntimeCommand) -> String {
    if command.path.trim().is_empty() {
        "claude".to_string()
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

fn log_blocked(source: &str, flags: &[String]) {
    if !flags.is_empty() {
        tracing::warn!(provider = "claude", source, flags = ?flags, "ignored daemon-owned arguments");
    }
}

fn root_sudo_preflight(env: &BTreeMap<String, String>) -> Result<(), AgentError> {
    #[cfg(unix)]
    {
        let sandbox = env
            .get("IS_SANDBOX")
            .cloned()
            .or_else(|| std::env::var("IS_SANDBOX").ok())
            .is_some_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            });
        if unsafe { libc::geteuid() } == 0 && !sandbox {
            return Err(AgentError::InvalidConfig(
                "Claude Code refuses bypassPermissions under root/sudo privileges. Run the Cordy daemon as a non-root user, or set IS_SANDBOX=1 if running in a genuine container/sandbox".to_string(),
            ));
        }
    }
    Ok(())
}

fn static_catalog(help: Option<&str>) -> Catalog {
    let superset = help.map_or_else(
        || vec!["low", "medium", "high"],
        claude_effort_levels_from_help,
    );
    let models = static_models()
        .into_iter()
        .map(|mut model| {
            let levels = project_effort_levels(&superset, effort_allow(&model.id));
            if !levels.is_empty() {
                model.thinking = Some(ModelThinking {
                    supported_levels: levels,
                    default_level: "medium".to_string(),
                });
            }
            model
        })
        .collect();
    Catalog {
        models,
        fallback: false,
    }
}

pub fn static_models() -> Vec<Model> {
    vec![
        Model {
            id: "claude-sonnet-5".to_string(),
            label: "Claude Sonnet 5".to_string(),
            provider: "anthropic".to_string(),
            ..Model::default()
        },
        Model {
            id: "claude-sonnet-4-6".to_string(),
            label: "Claude Sonnet 4.6".to_string(),
            provider: "anthropic".to_string(),
            default: true,
            ..Model::default()
        },
        Model {
            id: "claude-fable-5".to_string(),
            label: "Claude Fable 5".to_string(),
            provider: "anthropic".to_string(),
            ..Model::default()
        },
        Model {
            id: "claude-opus-5".to_string(),
            label: "Claude Opus 5".to_string(),
            provider: "anthropic".to_string(),
            ..Model::default()
        },
        Model {
            id: "claude-opus-4-8".to_string(),
            label: "Claude Opus 4.8".to_string(),
            provider: "anthropic".to_string(),
            ..Model::default()
        },
        Model {
            id: "claude-opus-4-7".to_string(),
            label: "Claude Opus 4.7".to_string(),
            provider: "anthropic".to_string(),
            ..Model::default()
        },
        Model {
            id: "claude-haiku-4-5-20251001".to_string(),
            label: "Claude Haiku 4.5".to_string(),
            provider: "anthropic".to_string(),
            ..Model::default()
        },
        Model {
            id: "claude-opus-4-6".to_string(),
            label: "Claude Opus 4.6".to_string(),
            provider: "anthropic".to_string(),
            ..Model::default()
        },
        Model {
            id: "claude-sonnet-4-5".to_string(),
            label: "Claude Sonnet 4.5".to_string(),
            provider: "anthropic".to_string(),
            ..Model::default()
        },
    ]
}

fn effort_allow(model: &str) -> Option<&'static [&'static str]> {
    match model {
        "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6" => {
            Some(&["low", "medium", "high", "xhigh", "max"])
        }
        "claude-sonnet-4-6" | "claude-sonnet-4-5" => Some(&["low", "medium", "high", "max"]),
        "claude-haiku-4-5-20251001" => Some(&["low", "medium", "high"]),
        _ => None,
    }
}

fn claude_effort_levels_from_help(help: &str) -> Vec<&str> {
    if let Some(captures) = EFFORT_RE.captures(help) {
        let values: Vec<_> = captures
            .get(1)
            .map(|capture| {
                capture
                    .as_str()
                    .split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        if !values.is_empty() {
            return values;
        }
    }
    if help.contains("--effort") {
        return vec!["low", "medium", "high", "xhigh", "max"];
    }
    Vec::new()
}

fn project_effort_levels(superset: &[&str], allow: Option<&[&str]>) -> Vec<ThinkingLevel> {
    superset
        .iter()
        .filter(|value| allow.is_none_or(|allowed| allowed.contains(value)))
        .map(|value| ThinkingLevel {
            value: (*value).to_string(),
            label: match *value {
                "low" => "Low",
                "medium" => "Medium",
                "high" => "High",
                "xhigh" => "Extra high",
                "max" => "Max",
                other => other,
            }
            .to_string(),
            ..ThinkingLevel::default()
        })
        .collect()
}

async fn capture_help(
    path: &str,
    prefix: &[String],
    env: &BTreeMap<String, String>,
    cancellation: CancellationToken,
    timeout: Duration,
) -> io::Result<Option<String>> {
    let mut command = Command::new(path);
    command
        .args(prefix)
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
    configure_child_environment(&mut command, env);
    let mut tree = OwnedProcessTree::spawn(&mut command).await?;
    let stdout = tree
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("Claude help stdout pipe unavailable"))?;
    let stderr = tree
        .child_mut()
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("Claude help stderr pipe unavailable"))?;
    let mut stdout_task = tokio::spawn(read_limited(stdout, DISCOVERY_OUTPUT_MAX));
    let mut stderr_task = tokio::spawn(read_limited(stderr, DISCOVERY_OUTPUT_MAX));
    let status = tokio::select! {
        result = tree.wait() => result?,
        () = cancellation.cancelled() => {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            stdout_task.abort();
            stderr_task.abort();
            return Ok(None);
        }
        () = tokio::time::sleep(timeout) => {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            stdout_task.abort();
            stderr_task.abort();
            return Ok(None);
        }
    };
    let stdout = join_reader(&mut stdout_task).await?;
    let stderr = join_reader(&mut stderr_task).await?;
    if !status.success() {
        return Ok(None);
    }
    let mut output = stdout;
    output.extend(stderr);
    Ok(Some(String::from_utf8_lossy(&output).into_owned()))
}

async fn read_limited(mut reader: impl AsyncRead + Unpin, max: usize) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 32 * 1024];
    loop {
        let bytes = reader.read(&mut chunk).await?;
        if bytes == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(bytes) > max {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Claude help output exceeds {max} byte limit"),
            ));
        }
        output.extend_from_slice(&chunk[..bytes]);
    }
}

async fn join_reader(task: &mut JoinHandle<io::Result<Vec<u8>>>) -> io::Result<Vec<u8>> {
    match tokio::time::timeout(KILL_GRACE, &mut *task).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => Err(io::Error::other(format!(
            "Claude help reader failed: {error}"
        ))),
        Err(_) => {
            task.abort();
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Claude help output did not terminate",
            ))
        }
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
    fn arguments_own_protocol_and_claude_permissions() {
        let options = ExecOptions {
            model: "claude-opus-5".to_string(),
            thinking_level: "high".to_string(),
            max_turns: 12,
            resume_session_id: "session-1".to_string(),
            claude_settings_path: "/settings.json".to_string(),
            extra_args: strings(&["--output-format", "text"]),
            custom_args: strings(&[
                "--effort=max",
                "--permission-mode",
                "default",
                "--settings",
                "/user-settings.json",
            ]),
            ..ExecOptions::default()
        };
        let args = build_claude_args(&options);
        assert_eq!(
            &args[..10],
            strings(&[
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
            ])
        );
        assert!(args.windows(2).any(|pair| pair == ["--effort", "high"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--settings", "/settings.json"]));
        assert!(!args
            .iter()
            .any(|arg| arg == "text" || arg == "--effort=max" || arg == "/user-settings.json"));
    }

    #[test]
    fn managed_mcp_uses_strict_mode_and_system_prompt_is_not_duplicated() {
        let options = ExecOptions {
            system_prompt: "do not duplicate".to_string(),
            mcp_config: Some(serde_json::json!({"mcpServers": {}})),
            ..ExecOptions::default()
        };
        let args = build_claude_args(&options);
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert!(!args.contains(&"do not duplicate".to_string()));
    }

    #[test]
    fn control_response_allows_tools_and_forces_background_foreground() {
        let event: ClaudeEvent = serde_json::from_value(serde_json::json!({
            "type":"control_request", "request_id":"r1",
            "request":{"input":{"run_in_background":true,"command":"pwd"}}
        }))
        .unwrap_or_else(|error| panic!("control event: {error}"));
        let raw = control_response(&event).unwrap_or_default();
        let value: Value =
            serde_json::from_slice(&raw).unwrap_or_else(|error| panic!("response: {error}"));
        assert_eq!(value["response"]["response"]["behavior"], "allow");
        assert_eq!(
            value["response"]["response"]["updatedInput"]["run_in_background"],
            false
        );
    }

    #[test]
    fn context_exhaustion_is_a_structured_failure() {
        let message = terminal_reason_failure("prompt_too_long", "compaction failed");
        assert!(message.contains("terminal_reason=prompt_too_long"));
        assert!(terminal_reason_failure("end_turn", "done").is_empty());
    }

    #[test]
    fn static_catalog_projects_per_model_effort_contract() {
        let catalog = static_catalog(Some(
            "--effort <level> Effort level for the current session (low, medium, high, xhigh, max)",
        ));
        let opus = catalog
            .models
            .iter()
            .find(|model| model.id == "claude-opus-5");
        let haiku = catalog
            .models
            .iter()
            .find(|model| model.id == "claude-haiku-4-5-20251001");
        assert_eq!(
            opus.and_then(|model| model.thinking.as_ref())
                .map(|thinking| thinking.supported_levels.len()),
            Some(5)
        );
        assert_eq!(
            haiku
                .and_then(|model| model.thinking.as_ref())
                .map(|thinking| thinking.supported_levels.len()),
            Some(3)
        );
    }

    #[test]
    fn effort_help_parser_accepts_claude_description_text() {
        assert_eq!(
            claude_effort_levels_from_help(
                "--effort <level> Effort level for the current session (low, medium, high)"
            ),
            vec!["low", "medium", "high"]
        );
        assert!(claude_effort_levels_from_help("claude --help without effort").is_empty());
    }

    #[test]
    fn inherited_environment_filters_only_cordy_and_claude_internal_state() {
        assert!(should_filter_inherited_env("CORDY_WORKSPACE"));
        assert!(should_filter_inherited_env("CORDY_"));
        assert!(should_filter_inherited_env("CLAUDECODE"));
        assert!(should_filter_inherited_env("CLAUDECODE_PARENT"));
        assert!(should_filter_inherited_env("CLAUDE_CODE_SESSION_ID"));
        assert!(should_filter_inherited_env("CLAUDE_CODE_ENTRYPOINT"));
        assert!(!should_filter_inherited_env("CLAUDE_CODE_GIT_BASH_PATH"));
        assert!(!should_filter_inherited_env("PATH"));
    }

    #[test]
    fn resume_rejection_uses_claude_specific_signals() {
        assert!(claude_resume_was_rejected(
            "requested",
            "requested",
            true,
            ["No conversation found for this session"]
        ));
        assert!(claude_resume_was_rejected(
            "requested",
            "requested",
            true,
            ["bound to a different account"]
        ));
        assert!(!claude_resume_was_rejected(
            "requested",
            "requested",
            true,
            ["conversation not found"]
        ));
        assert!(!claude_resume_was_rejected(
            "requested",
            "requested",
            true,
            ["network connection failed"]
        ));
        assert!(claude_resume_was_rejected(
            "requested",
            "new-session",
            true,
            [""]
        ));
        assert!(!claude_resume_was_rejected(
            "requested",
            "new-session",
            false,
            ["No conversation found"]
        ));
    }

    #[cfg(unix)]
    #[test]
    fn managed_mcp_temp_is_private_and_guard_owned() {
        let file = write_claude_mcp_temp(Some(&serde_json::json!({
            "mcpServers": {"demo": {"command": "echo"}}
        })))
        .unwrap_or_else(|error| panic!("write Claude MCP temp: {error}"))
        .unwrap_or_else(|| panic!("managed MCP config must create a temp file"));
        let path = file.path.clone();
        let value: Value = serde_json::from_slice(
            &std::fs::read(&path).unwrap_or_else(|error| panic!("read Claude MCP temp: {error}")),
        )
        .unwrap_or_else(|error| panic!("decode Claude MCP temp: {error}"));
        assert_eq!(value["mcpServers"]["demo"]["command"], "echo");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path)
                .unwrap_or_else(|error| panic!("Claude MCP metadata: {error}"))
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(file);
        assert!(!path.exists());
    }

    #[cfg(unix)]
    fn fake_backend(script: &str) -> (tempfile::TempDir, ClaudeBackend) {
        let directory = tempfile::tempdir_in(".")
            .unwrap_or_else(|error| panic!("tempdir in workspace: {error}"));
        let executable = directory.path().join("claude");
        std::fs::write(&executable, script).unwrap_or_else(|error| panic!("write fake: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake: {error}"));
        (
            directory,
            ClaudeBackend::new(ClaudeConfig {
                command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
                env: BTreeMap::from([(String::from("IS_SANDBOX"), String::from("1"))]),
            }),
        )
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_parses_stream_and_result_usage() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
printf '%s\n' '{"type":"system","session_id":"ses-1"}'
printf '%s\n' '{"type":"assistant","message":{"model":"claude-opus-5","content":[{"type":"text","text":"hello"}],"usage":{"input_tokens":4,"output_tokens":2}}}'
printf '%s\n' '{"type":"result","session_id":"ses-1","result":"hello","is_error":false,"model":"claude-opus-5","modelUsage":{"claude-opus-5":{"inputTokens":4,"outputTokens":2}}}'
"#,
        );
        let session = backend
            .execute("prompt", ExecOptions::default())
            .await
            .unwrap_or_else(|error| panic!("execute Claude: {error}"));
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
        assert_eq!(result.usage["claude-opus-5"].output_tokens, 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn discovery_reads_effort_levels_from_help() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
printf '%s\n' '--effort <level> Effort level for the current session (low, medium, high, xhigh, max)'
"#,
        );
        let catalog = backend
            .discover_models(
                &CatalogCache::default(),
                CancellationToken::new(),
                Duration::from_secs(5),
            )
            .await;
        assert_eq!(catalog.models.len(), 9);
        assert!(catalog.models.iter().any(|model| model.default));
        assert_eq!(
            catalog
                .models
                .iter()
                .find(|model| model.id == "claude-opus-5")
                .and_then(|model| model.thinking.as_ref())
                .map(|thinking| thinking.supported_levels.len()),
            Some(5)
        );
    }
}
