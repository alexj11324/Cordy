//! OpenClaw's one-shot agent adapter.
//!
//! OpenClaw is not an ACP or ordinary line-stream runtime. Cordy owns the
//! session id, local/gateway routing, JSON output mode, and version gate while
//! accepting both its current pretty JSON result and the older NDJSON event
//! vocabulary.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::process::{ExitStatus, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{ChildStderr, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::command::{filter_custom_args, filter_launch_prefix, BlockedArgMode, RuntimeCommand};
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
};
use crate::model::{Catalog, CatalogCache, Model, ModelDiscoveryCacheKey};
use crate::process::OwnedProcessTree;
use crate::stderr::SharedDiagnosticBuffer;

const MESSAGE_BUFFER: usize = 256;
const MIN_OPENCLAW_VERSION: &str = "2026.5.5";
const VERSION_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);
const DISCOVERY_OUTPUT_MAX: usize = 4 * 1024 * 1024;
const STDOUT_MAX: usize = 64 * 1024 * 1024;
const TERMINATION_GRACE: Duration = Duration::from_secs(5);
const KILL_GRACE: Duration = Duration::from_secs(10);
const RESULT_IDLE_GRACE: Duration = Duration::from_secs(2);
const STDERR_TAIL_BYTES: usize = 16 * 1024;

static VERSION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d+)\.(\d+)\.(\d+)")
        .unwrap_or_else(|error| panic!("invalid OpenClaw version regex: {error}"))
});

pub(crate) static BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> =
    LazyLock::new(|| {
        BTreeMap::from([
            ("--local", BlockedArgMode::Standalone),
            ("--json", BlockedArgMode::Standalone),
            ("--session-id", BlockedArgMode::WithValue),
            ("--message", BlockedArgMode::WithValue),
            ("--model", BlockedArgMode::WithValue),
            ("--system-prompt", BlockedArgMode::WithValue),
        ])
    });

#[derive(Debug, Clone, Default)]
pub struct OpenclawConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct OpenclawBackend {
    config: OpenclawConfig,
}

impl OpenclawBackend {
    pub fn new(config: OpenclawConfig) -> Self {
        Self { config }
    }

    pub async fn discover_models(
        &self,
        cache: &CatalogCache,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        self.discover_models_for_runtime("openclaw", cache, cancellation, timeout)
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
            "openclaw"
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
        let command_path = command_path(&self.config.command);
        let prefix = filter_launch_prefix(&self.config.command.prefix, &BLOCKED_ARGS).args;

        for arguments in [
            vec!["agents", "list", "--json"],
            vec!["agents", "list", "--output", "json"],
            vec!["agents", "list", "-o", "json"],
        ] {
            let budget = remaining();
            if budget.is_zero() {
                break;
            }
            let Some(captured) = capture_command(
                &command_path,
                &prefix,
                &arguments,
                &self.config.env,
                cancellation.clone(),
                budget,
                DISCOVERY_OUTPUT_MAX,
            )
            .await
            .ok()
            .flatten() else {
                continue;
            };
            if let Some(models) = parse_openclaw_agents_json(&captured.stdout) {
                return cache_catalog(cache, key, models);
            }
        }

        let budget = remaining();
        if !budget.is_zero() && !cancellation.is_cancelled() {
            if let Some(captured) = capture_command(
                &command_path,
                &prefix,
                &["agents", "list"],
                &self.config.env,
                cancellation.clone(),
                budget,
                DISCOVERY_OUTPUT_MAX,
            )
            .await
            .ok()
            .flatten()
            {
                let models = parse_openclaw_agents(&String::from_utf8_lossy(&captured.stdout));
                return cache_catalog(cache, key, models);
            }
        }

        Catalog::default()
    }
}

#[async_trait]
impl Backend for OpenclawBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        let path = command_path(&self.config.command);
        check_openclaw_version(
            &path,
            &self.config.command.prefix,
            &self.config.env,
            options.cancellation.clone(),
        )
        .await?;

        let prefix = filter_launch_prefix(&self.config.command.prefix, &BLOCKED_ARGS).args;
        let mut argv = prefix;
        argv.extend(build_openclaw_args(prompt, &options));

        let mut command = Command::new(&path);
        command
            .args(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&self.config.env)
            .kill_on_drop(false);
        if !options.cwd.is_empty() {
            command.current_dir(&options.cwd).env("PWD", &options.cwd);
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
                "OpenClaw stdout pipe unavailable after spawn".to_string(),
            ));
        };
        let Some(stderr) = tree.child_mut().stderr.take() else {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            return Err(AgentError::Protocol(
                "OpenClaw stderr pipe unavailable after spawn".to_string(),
            ));
        };

        let (message_tx, message_rx) = mpsc::channel(MESSAGE_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();
        let cancellation = options.cancellation.clone();
        let timeout = options.timeout;
        let configured_model = options.model.clone();
        let started = Instant::now();
        let deadline = (!timeout.is_zero()).then(|| started + timeout);

        tokio::spawn(async move {
            run_openclaw(
                tree,
                stdout,
                stderr,
                message_tx,
                result_tx,
                cancellation,
                deadline,
                configured_model,
                started,
            )
            .await;
        });

        Ok(Session {
            messages: message_rx,
            result: result_rx,
        })
    }
}

pub fn build_openclaw_args(prompt: &str, options: &ExecOptions) -> Vec<String> {
    let mut args = vec!["agent".to_string()];
    if options.openclaw_mode != "gateway" {
        args.push("--local".to_string());
    }
    args.extend([
        "--json".to_string(),
        "--session-id".to_string(),
        openclaw_session_id(options),
    ]);
    if !options.timeout.is_zero() {
        args.extend([
            "--timeout".to_string(),
            options.timeout.as_secs().to_string(),
        ]);
    }

    // Go's OpenClaw adapter intentionally consumes only per-agent custom_args.
    // ExtraArgs belongs to other provider families' daemon defaults.
    let custom = filter_custom_args(&options.custom_args, &BLOCKED_ARGS).args;
    if !options.model.is_empty() && !custom_args_contains(&custom, "--agent") {
        args.extend(["--agent".to_string(), options.model.clone()]);
    }
    args.extend(custom);

    let message = if options.system_prompt.is_empty() {
        prompt.to_string()
    } else {
        format!("{}\n\n{prompt}", options.system_prompt)
    };
    args.extend(["--message".to_string(), message]);
    args
}

fn openclaw_session_id(options: &ExecOptions) -> String {
    if !options.resume_session_id.is_empty() {
        return options.resume_session_id.clone();
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("cordy-{nanos}")
}

fn custom_args_contains(args: &[String], flag: &str) -> bool {
    let prefix = format!("{flag}=");
    args.iter()
        .any(|argument| argument == flag || argument.starts_with(&prefix))
}

async fn check_openclaw_version(
    path: &str,
    prefix: &[String],
    env: &BTreeMap<String, String>,
    cancellation: CancellationToken,
) -> Result<(), AgentError> {
    let filtered_prefix = filter_launch_prefix(prefix, &BLOCKED_ARGS).args;
    let captured = match capture_command(
        path,
        &filtered_prefix,
        &["--version"],
        env,
        cancellation,
        VERSION_TIMEOUT,
        DISCOVERY_OUTPUT_MAX,
    )
    .await
    {
        Ok(Some(captured)) => captured,
        Ok(None) => {
            return Err(AgentError::Protocol(
                "openclaw --version did not complete".to_string(),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(AgentError::ExecutableNotFound(path.to_string()));
        }
        Err(error) => return Err(AgentError::Process(error)),
    };
    if !captured.status.success() {
        return Err(AgentError::InvalidConfig(format!(
            "openclaw --version failed: {}",
            captured.status
        )));
    }
    let mut output = captured.stdout;
    output.extend(captured.stderr);
    let Some(version) = parse_openclaw_version(&String::from_utf8_lossy(&output)) else {
        return Err(AgentError::InvalidConfig(
            "could not parse openclaw version from output".to_string(),
        ));
    };
    if compare_openclaw_version(&version, MIN_OPENCLAW_VERSION).is_lt() {
        return Err(AgentError::InvalidConfig(format!(
            "openclaw {version} is below the minimum supported version {MIN_OPENCLAW_VERSION}. Run `openclaw update` to upgrade and try again."
        )));
    }
    Ok(())
}

fn parse_openclaw_version(raw: &str) -> Option<String> {
    VERSION_PATTERN
        .captures(raw)
        .map(|captures| captures[0].to_string())
}

fn compare_openclaw_version(left: &str, right: &str) -> std::cmp::Ordering {
    let parse = |value: &str| {
        value
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or_default())
            .collect::<Vec<_>>()
    };
    let left = parse(left);
    let right = parse(right);
    (0..3)
        .map(|index| {
            left.get(index)
                .copied()
                .unwrap_or_default()
                .cmp(&right.get(index).copied().unwrap_or_default())
        })
        .find(|ordering| *ordering != std::cmp::Ordering::Equal)
        .unwrap_or(std::cmp::Ordering::Equal)
}

#[derive(Debug)]
struct CapturedOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

async fn capture_command(
    path: &str,
    prefix: &[String],
    arguments: &[&str],
    env: &BTreeMap<String, String>,
    cancellation: CancellationToken,
    timeout: Duration,
    max_output: usize,
) -> io::Result<Option<CapturedOutput>> {
    let mut command = Command::new(path);
    command
        .args(prefix)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .envs(env)
        .kill_on_drop(false);
    let mut tree = OwnedProcessTree::spawn(&mut command).await?;
    let stdout = tree
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("capture stdout pipe unavailable"))?;
    let stderr = tree
        .child_mut()
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("capture stderr pipe unavailable"))?;
    let mut stdout_task = tokio::spawn(read_limited(stdout, max_output));
    let mut stderr_task = tokio::spawn(read_limited(stderr, max_output));

    let wait = if timeout.is_zero() {
        tokio::select! {
            status = tree.wait() => Some(status?),
            () = cancellation.cancelled() => None,
        }
    } else {
        tokio::select! {
            status = tree.wait() => Some(status?),
            () = cancellation.cancelled() => None,
            () = tokio::time::sleep(timeout) => None,
        }
    };
    let Some(status) = wait else {
        let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
        stdout_task.abort();
        stderr_task.abort();
        return Ok(None);
    };

    let stdout = join_limited_reader(&mut stdout_task).await?;
    let stderr = join_limited_reader(&mut stderr_task).await?;
    Ok(Some(CapturedOutput {
        status,
        stdout,
        stderr,
    }))
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
                format!("OpenClaw output exceeds {max} byte limit"),
            ));
        }
        output.extend_from_slice(&chunk[..bytes]);
    }
}

async fn join_limited_reader(task: &mut JoinHandle<io::Result<Vec<u8>>>) -> io::Result<Vec<u8>> {
    match tokio::time::timeout(KILL_GRACE, &mut *task).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => Err(io::Error::other(format!(
            "OpenClaw output task failed: {error}"
        ))),
        Err(_) => {
            task.abort();
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "OpenClaw output did not terminate",
            ))
        }
    }
}

#[derive(Debug, Default)]
struct StdoutRead {
    bytes: Vec<u8>,
    cut_short: bool,
    error: Option<String>,
}

async fn read_openclaw_stdout(mut stdout: ChildStdout, stop: CancellationToken) -> StdoutRead {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 32 * 1024];
    let mut result_deadline: Option<Instant> = None;

    loop {
        if let Some(deadline) = result_deadline {
            let idle = deadline.saturating_duration_since(Instant::now());
            tokio::select! {
                biased;
                () = stop.cancelled() => return StdoutRead { bytes: output, error: Some("OpenClaw stdout read cancelled".to_string()), ..StdoutRead::default() },
                read = stdout.read(&mut chunk) => match read {
                    Ok(0) => return StdoutRead { bytes: output, ..StdoutRead::default() },
                    Ok(bytes) => {
                        if output.len().saturating_add(bytes) > STDOUT_MAX {
                            return StdoutRead { bytes: output, error: Some(format!("OpenClaw stdout exceeds {STDOUT_MAX} byte limit")), ..StdoutRead::default() };
                        }
                        output.extend_from_slice(&chunk[..bytes]);
                        result_deadline = parse_whole_openclaw_result(&output).map(|_| Instant::now() + RESULT_IDLE_GRACE);
                    }
                    Err(error) => return StdoutRead { bytes: output, error: Some(format!("read stdout: {error}")), ..StdoutRead::default() },
                },
                () = tokio::time::sleep(idle) => {
                    if parse_whole_openclaw_result(&output).is_some() {
                        return StdoutRead { bytes: output, cut_short: true, ..StdoutRead::default() };
                    }
                    result_deadline = None;
                }
            }
        } else {
            tokio::select! {
                biased;
                () = stop.cancelled() => return StdoutRead { bytes: output, error: Some("OpenClaw stdout read cancelled".to_string()), ..StdoutRead::default() },
                read = stdout.read(&mut chunk) => match read {
                    Ok(0) => return StdoutRead { bytes: output, ..StdoutRead::default() },
                    Ok(bytes) => {
                        if output.len().saturating_add(bytes) > STDOUT_MAX {
                            return StdoutRead { bytes: output, error: Some(format!("OpenClaw stdout exceeds {STDOUT_MAX} byte limit")), ..StdoutRead::default() };
                        }
                        output.extend_from_slice(&chunk[..bytes]);
                        result_deadline = parse_whole_openclaw_result(&output).map(|_| Instant::now() + RESULT_IDLE_GRACE);
                    }
                    Err(error) => return StdoutRead { bytes: output, error: Some(format!("read stdout: {error}")), ..StdoutRead::default() },
                }
            }
        }
    }
}

enum FirstOutcome {
    Process(Result<ExitStatus, io::Error>),
    Stdout(Result<StdoutRead, tokio::task::JoinError>),
    Cancelled,
    TimedOut,
}

async fn first_openclaw_outcome(
    tree: &mut OwnedProcessTree,
    stdout_task: &mut JoinHandle<StdoutRead>,
    cancellation: &CancellationToken,
    deadline: Option<Instant>,
) -> FirstOutcome {
    let deadline = match deadline {
        Some(deadline) => deadline,
        None => {
            return tokio::select! {
            status = tree.wait() => FirstOutcome::Process(status),
            stdout = &mut *stdout_task => FirstOutcome::Stdout(stdout),
            () = cancellation.cancelled() => FirstOutcome::Cancelled,
            };
        }
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        FirstOutcome::TimedOut
    } else {
        tokio::select! {
            status = tree.wait() => FirstOutcome::Process(status),
            stdout = &mut *stdout_task => FirstOutcome::Stdout(stdout),
            () = cancellation.cancelled() => FirstOutcome::Cancelled,
            () = tokio::time::sleep(remaining) => FirstOutcome::TimedOut,
        }
    }
}

async fn join_stdout_task(
    task: &mut JoinHandle<StdoutRead>,
    stop: &CancellationToken,
) -> StdoutRead {
    match tokio::time::timeout(KILL_GRACE, &mut *task).await {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => StdoutRead {
            error: Some(format!("OpenClaw stdout task failed: {error}")),
            ..StdoutRead::default()
        },
        Err(_) => {
            stop.cancel();
            task.abort();
            StdoutRead {
                error: Some("OpenClaw stdout did not terminate".to_string()),
                ..StdoutRead::default()
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_openclaw(
    mut tree: OwnedProcessTree,
    stdout: ChildStdout,
    stderr: ChildStderr,
    messages: mpsc::Sender<Message>,
    result_tx: oneshot::Sender<ExecutionResult>,
    cancellation: CancellationToken,
    deadline: Option<Instant>,
    configured_model: String,
    started: Instant,
) {
    let timeout = deadline
        .map(|deadline| deadline.saturating_duration_since(started))
        .unwrap_or(Duration::ZERO);
    let stop = CancellationToken::new();
    let mut stdout_task = tokio::spawn(read_openclaw_stdout(stdout, stop.clone()));
    let stderr_tail = SharedDiagnosticBuffer::new(STDERR_TAIL_BYTES);
    let stderr_reader_tail = stderr_tail.clone();
    let mut stderr_task = tokio::spawn(pump_stderr(stderr, stderr_reader_tail));

    let first = first_openclaw_outcome(&mut tree, &mut stdout_task, &cancellation, deadline).await;
    let (run_end, mut exit, stdout_state) = match first {
        FirstOutcome::Process(status) => (
            RunEnd::Completed,
            Some(status),
            join_stdout_task(&mut stdout_task, &stop).await,
        ),
        FirstOutcome::Stdout(result) => {
            let state = result.unwrap_or_else(|error| StdoutRead {
                error: Some(format!("OpenClaw stdout task failed: {error}")),
                ..StdoutRead::default()
            });
            if state.cut_short {
                let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
                (RunEnd::CutShort, None, state)
            } else {
                match first_openclaw_process_outcome(&mut tree, &cancellation, deadline).await {
                    (RunEnd::Completed, status) => (RunEnd::Completed, Some(Ok(status)), state),
                    (run_end, status) => {
                        stop.cancel();
                        let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
                        (run_end, Some(Ok(status)), state)
                    }
                }
            }
        }
        FirstOutcome::Cancelled => {
            stop.cancel();
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            (
                RunEnd::Cancelled,
                None,
                join_stdout_task(&mut stdout_task, &stop).await,
            )
        }
        FirstOutcome::TimedOut => {
            stop.cancel();
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            (
                RunEnd::TimedOut,
                None,
                join_stdout_task(&mut stdout_task, &stop).await,
            )
        }
    };

    if !matches!(
        run_end,
        RunEnd::Cancelled | RunEnd::TimedOut | RunEnd::CutShort
    ) {
        stop.cancel();
    }
    if tokio::time::timeout(KILL_GRACE, &mut stderr_task)
        .await
        .is_err()
    {
        stderr_task.abort();
    }
    let stderr = stderr_tail.tail();
    if !stderr.is_empty() {
        tracing::debug!(provider = "openclaw", %stderr, "agent stderr captured");
    }

    let mut state = parse_openclaw_output(&stdout_state.bytes, &messages);
    if let Some(error) = stdout_state.error {
        state.status = "failed".to_string();
        state.error = error;
        state.output.clear();
        state.session_id.clear();
        state.model.clear();
        state.usage = TokenUsage::default();
    }

    if matches!(run_end, RunEnd::TimedOut) {
        state.status = "timeout".to_string();
        state.error = format!("openclaw timed out after {}", format_duration(timeout));
    } else if matches!(run_end, RunEnd::Cancelled) {
        state.status = "aborted".to_string();
        state.error = "execution cancelled".to_string();
    } else if !matches!(run_end, RunEnd::CutShort) {
        if let Some(exit_error) = exit.take().and_then(|status| exit_error_result(&status)) {
            if state.status == "completed" {
                state.status = "failed".to_string();
                state.error = format!("openclaw exited with error: {exit_error}");
            }
        }
    }

    let mut usage = BTreeMap::new();
    if has_usage(state.usage) {
        let model = if state.model.is_empty() {
            if configured_model.is_empty() {
                "unknown".to_string()
            } else {
                configured_model
            }
        } else {
            state.model.clone()
        };
        usage.insert(model, state.usage);
    }
    let _ = result_tx.send(ExecutionResult {
        status: state.status,
        output: state.output,
        error: state.error,
        duration_ms: started.elapsed().as_millis().try_into().unwrap_or(i64::MAX),
        session_id: state.session_id,
        usage,
        resume_rejected: false,
    });
}

async fn first_openclaw_process_outcome(
    tree: &mut OwnedProcessTree,
    cancellation: &CancellationToken,
    deadline: Option<Instant>,
) -> (RunEnd, ExitStatus) {
    let deadline = match deadline {
        Some(deadline) => deadline,
        None => {
            let result = tokio::select! {
            status = tree.wait() => status,
            () = cancellation.cancelled() => return (RunEnd::Cancelled, success_status()),
            };
            return match result {
                Ok(status) => (RunEnd::Completed, status),
                Err(_) => (RunEnd::Completed, success_status()),
            };
        }
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return (RunEnd::TimedOut, success_status());
    }
    let result = {
        tokio::select! {
            status = tree.wait() => status,
            () = cancellation.cancelled() => return (RunEnd::Cancelled, success_status()),
            () = tokio::time::sleep(remaining) => return (RunEnd::TimedOut, success_status()),
        }
    };
    match result {
        Ok(status) => (RunEnd::Completed, status),
        Err(_) => (RunEnd::Completed, success_status()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunEnd {
    Completed,
    TimedOut,
    Cancelled,
    CutShort,
}

fn success_status() -> ExitStatus {
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

fn exit_error(status: &ExitStatus) -> Option<String> {
    (!status.success()).then(|| status.to_string())
}

fn exit_error_result(status: &Result<ExitStatus, io::Error>) -> Option<String> {
    match status {
        Ok(status) => exit_error(status),
        Err(error) => Some(format!("wait failed: {error}")),
    }
}

fn format_duration(timeout: Duration) -> String {
    if timeout.is_zero() {
        return "0s".to_string();
    }
    format!("{}s", timeout.as_secs_f64())
}

#[derive(Debug, Default)]
struct OpenclawEventResult {
    status: String,
    error: String,
    output: String,
    session_id: String,
    model: String,
    usage: TokenUsage,
}

fn parse_openclaw_output(bytes: &[u8], messages: &mpsc::Sender<Message>) -> OpenclawEventResult {
    if let Some(result) = parse_whole_openclaw_result(bytes) {
        let mut output = String::new();
        return build_final_result(&result, messages, &mut output);
    }

    let mut state = OpenclawEventResult {
        status: "completed".to_string(),
        ..OpenclawEventResult::default()
    };
    let mut raw_lines = Vec::new();
    let mut got_events = false;
    for line in String::from_utf8_lossy(bytes).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<OpenclawEvent>(line) {
            if event.kind.is_empty() {
                raw_lines.push(line.to_string());
                continue;
            }
            got_events = true;
            if !event.session_id.is_empty() {
                state.session_id = event.session_id.clone();
            }
            match event.kind.as_str() {
                "text" => {
                    if !event.text.is_empty() {
                        state.output.push_str(&event.text);
                        send_message(
                            messages,
                            Message {
                                content: event.text,
                                ..empty_message(MessageType::Text, "")
                            },
                        );
                    }
                }
                "tool_use" => {
                    let input = event
                        .input
                        .and_then(|value| value.as_object().cloned())
                        .unwrap_or_default()
                        .into_iter()
                        .collect();
                    send_message(
                        messages,
                        Message {
                            tool: event.tool,
                            call_id: event.call_id,
                            input,
                            ..empty_message(MessageType::ToolUse, "")
                        },
                    );
                }
                "tool_result" => send_message(
                    messages,
                    Message {
                        tool: event.tool,
                        call_id: event.call_id,
                        output: event.text,
                        ..empty_message(MessageType::ToolResult, "")
                    },
                ),
                "error" => {
                    let error = event.error_message();
                    send_message(
                        messages,
                        Message {
                            content: error.clone(),
                            ..empty_message(MessageType::Error, "")
                        },
                    );
                    state.status = "failed".to_string();
                    state.error = error;
                }
                "lifecycle" if matches!(event.phase.as_str(), "error" | "failed" | "cancelled") => {
                    let error = event.error_message();
                    send_message(
                        messages,
                        Message {
                            content: error.clone(),
                            ..empty_message(MessageType::Error, "")
                        },
                    );
                    state.status = "failed".to_string();
                    state.error = error;
                }
                "step_start" => {
                    send_message(messages, empty_message(MessageType::Status, "running"))
                }
                "step_finish" => {
                    if let Some(usage) = event.usage.as_ref().and_then(parse_usage_value) {
                        add_usage(&mut state.usage, usage);
                    }
                }
                _ => {}
            }
            continue;
        }
        if let Some(result) = parse_final_result(line) {
            got_events = true;
            let previous_usage = state.usage;
            let mut output = state.output;
            let final_state = build_final_result(&result, messages, &mut output);
            state.output = output;
            if !final_state.session_id.is_empty() {
                state.session_id = final_state.session_id;
            }
            if !final_state.model.is_empty() {
                state.model = final_state.model;
            }
            if has_usage(final_state.usage) {
                state.usage = final_state.usage;
            } else {
                state.usage = previous_usage;
            }
            continue;
        }
        raw_lines.push(line.to_string());
    }

    if !got_events {
        let raw = raw_lines.join("\n");
        if raw.trim().is_empty() {
            state.status = "failed".to_string();
            state.error = "openclaw returned no parseable output".to_string();
        } else {
            state.output = raw.trim().to_string();
        }
    }
    state
}

#[derive(Debug, Deserialize)]
struct OpenclawEvent {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default, rename = "sessionId")]
    session_id: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    tool: String,
    #[serde(default, rename = "callId")]
    call_id: String,
    #[serde(default)]
    input: Option<Value>,
    #[serde(default)]
    usage: Option<Value>,
    #[serde(default)]
    phase: String,
    #[serde(default)]
    error: Option<Value>,
    #[serde(default)]
    message: String,
}

impl OpenclawEvent {
    fn error_message(&self) -> String {
        self.error
            .as_ref()
            .and_then(error_value_message)
            .filter(|message| !message.is_empty())
            .or_else(|| (!self.text.is_empty()).then(|| self.text.clone()))
            .or_else(|| (!self.message.is_empty()).then(|| self.message.clone()))
            .unwrap_or_else(|| "unknown openclaw error".to_string())
    }
}

fn error_value_message(value: &Value) -> Option<String> {
    let object = value.as_object()?;
    object
        .get("data")
        .and_then(Value::as_object)
        .and_then(|data| data.get("message"))
        .and_then(Value::as_str)
        .or_else(|| object.get("message").and_then(Value::as_str))
        .or_else(|| object.get("name").and_then(Value::as_str))
        .map(str::to_string)
}

#[derive(Debug, Deserialize)]
struct OpenclawResult {
    #[serde(default)]
    payloads: Option<Vec<OpenclawPayload>>,
    #[serde(default)]
    meta: Option<OpenclawMeta>,
}

#[derive(Debug, Deserialize)]
struct OpenclawPayload {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct OpenclawMeta {
    #[serde(default, rename = "agentMeta")]
    agent_meta: Option<Value>,
}

fn parse_final_result(raw: &str) -> Option<OpenclawResult> {
    let value = serde_json::from_str::<Value>(raw).ok()?;
    parse_final_value(value)
}

fn parse_final_value(value: Value) -> Option<OpenclawResult> {
    let object = value.as_object()?;
    let has_payloads = object.get("payloads").is_some_and(Value::is_array);
    let has_duration = object
        .get("meta")
        .and_then(Value::as_object)
        .and_then(|meta| meta.get("durationMs"))
        .and_then(Value::as_i64)
        .is_some_and(|duration| duration != 0);
    if !has_payloads && !has_duration {
        return None;
    }
    serde_json::from_value(Value::Object(object.clone())).ok()
}

fn parse_whole_openclaw_result(bytes: &[u8]) -> Option<OpenclawResult> {
    let trimmed = String::from_utf8_lossy(bytes).trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(result) = parse_json_prefix(&trimmed) {
        return Some(result);
    }
    for (index, line) in trimmed.match_indices('\n') {
        let start = index + line.len();
        let candidate = &trimmed[start..];
        if candidate.starts_with('{') {
            if let Some(result) = parse_json_prefix(candidate) {
                return Some(result);
            }
        }
    }
    if trimmed.starts_with('{') {
        parse_json_prefix(&trimmed)
    } else {
        None
    }
}

fn parse_json_prefix(raw: &str) -> Option<OpenclawResult> {
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    let value = Value::deserialize(&mut deserializer).ok()?;
    parse_final_value(value)
}

fn build_final_result(
    result: &OpenclawResult,
    messages: &mpsc::Sender<Message>,
    output: &mut String,
) -> OpenclawEventResult {
    for payload in result.payloads.as_deref().unwrap_or_default() {
        if !payload.text.is_empty() {
            output.push_str(&payload.text);
            send_message(
                messages,
                Message {
                    content: payload.text.clone(),
                    ..empty_message(MessageType::Text, "")
                },
            );
        }
    }
    let mut state = OpenclawEventResult {
        status: "completed".to_string(),
        output: output.clone(),
        ..OpenclawEventResult::default()
    };
    if let Some(meta) = result.meta.as_ref() {
        if let Some(agent_meta) = meta.agent_meta.as_ref().and_then(Value::as_object) {
            state.session_id = agent_meta
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            state.model = agent_meta
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if let Some(usage) = agent_meta.get("usage").and_then(parse_usage_value) {
                state.usage = usage;
            }
        }
    }
    state
}

fn parse_usage_value(value: &Value) -> Option<TokenUsage> {
    let object = value.as_object()?;
    Some(TokenUsage {
        input_tokens: usage_first(object, &["input", "inputTokens", "input_tokens"]),
        output_tokens: usage_first(object, &["output", "outputTokens", "output_tokens"]),
        cache_read_tokens: usage_first(
            object,
            &[
                "cacheRead",
                "cachedInputTokens",
                "cached_input_tokens",
                "cache_read",
                "cache_read_input_tokens",
            ],
        ),
        cache_write_tokens: usage_first(
            object,
            &[
                "cacheWrite",
                "cacheCreationInputTokens",
                "cache_creation_input_tokens",
                "cache_write",
            ],
        ),
        ..TokenUsage::default()
    })
}

fn usage_first(object: &serde_json::Map<String, Value>, keys: &[&str]) -> i64 {
    keys.iter()
        .filter_map(|key| object.get(*key).and_then(value_as_i64))
        .find(|value| *value != 0)
        .unwrap_or_default()
}

fn value_as_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_f64().map(|value| value as i64))
}

fn add_usage(target: &mut TokenUsage, value: TokenUsage) {
    target.input_tokens = target.input_tokens.saturating_add(value.input_tokens);
    target.output_tokens = target.output_tokens.saturating_add(value.output_tokens);
    target.cache_read_tokens = target
        .cache_read_tokens
        .saturating_add(value.cache_read_tokens);
    target.cache_write_tokens = target
        .cache_write_tokens
        .saturating_add(value.cache_write_tokens);
}

fn has_usage(usage: TokenUsage) -> bool {
    usage.input_tokens != 0
        || usage.output_tokens != 0
        || usage.cache_read_tokens != 0
        || usage.cache_write_tokens != 0
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

async fn pump_stderr(mut stderr: ChildStderr, tail: SharedDiagnosticBuffer) {
    let mut buffer = [0_u8; 8192];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(bytes) => tail.push(&buffer[..bytes]),
        }
    }
}

fn command_path(command: &RuntimeCommand) -> String {
    if command.path.trim().is_empty() {
        "openclaw".to_string()
    } else {
        command.path.clone()
    }
}

#[derive(Debug, Deserialize)]
struct OpenclawAgentEntry {
    #[serde(default)]
    name: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    model: String,
}

fn parse_openclaw_agents_json(raw: &[u8]) -> Option<Vec<Model>> {
    let value = serde_json::from_slice::<Value>(raw).ok()?;
    if let Some(entries) = value.as_array() {
        let entries =
            serde_json::from_value::<Vec<OpenclawAgentEntry>>(Value::Array(entries.clone()))
                .ok()?;
        return Some(openclaw_entries_to_models(entries));
    }
    let entries = value.get("agents")?.clone();
    let entries = serde_json::from_value::<Vec<OpenclawAgentEntry>>(entries).ok()?;
    Some(openclaw_entries_to_models(entries))
}

fn openclaw_entries_to_models(entries: Vec<OpenclawAgentEntry>) -> Vec<Model> {
    let mut seen = BTreeSet::new();
    entries
        .into_iter()
        .filter_map(|entry| {
            let id = if entry.id.is_empty() {
                entry.name.clone()
            } else {
                entry.id.clone()
            };
            if id.is_empty() || !seen.insert(id.clone()) {
                return None;
            }
            let display = if entry.name.is_empty() {
                id.clone()
            } else {
                entry.name
            };
            let label = if entry.model.is_empty() {
                display
            } else {
                format!("{display} ({})", entry.model)
            };
            Some(Model {
                id,
                label,
                provider: "openclaw".to_string(),
                ..Model::default()
            })
        })
        .collect()
}

fn parse_openclaw_agents(output: &str) -> Vec<Model> {
    let mut seen = BTreeSet::new();
    output
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 2
                || !is_openclaw_identifier(fields[0])
                || !is_openclaw_identifier(fields[1])
                || !seen.insert(fields[0].to_string())
            {
                return None;
            }
            Some(Model {
                id: fields[0].to_string(),
                label: format!("{} ({})", fields[0], fields[1]),
                provider: "openclaw".to_string(),
                ..Model::default()
            })
        })
        .collect()
}

fn is_openclaw_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() || value.ends_with(':') {
        return false;
    }
    chars.all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '/')
    })
}

fn cache_catalog(cache: &CatalogCache, key: ModelDiscoveryCacheKey, models: Vec<Model>) -> Catalog {
    let catalog = Catalog {
        models,
        fallback: false,
    };
    if !catalog.models.is_empty() {
        let _ = cache.insert(key, catalog.clone());
    }
    catalog
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
    fn args_map_model_to_agent_and_inject_system_prompt() {
        let options = ExecOptions {
            model: "research-bot".to_string(),
            system_prompt: "Read only".to_string(),
            thread_name: "task".to_string(),
            resume_session_id: "ses-1".to_string(),
            ..ExecOptions::default()
        };
        let args = build_openclaw_args("task", &options);
        assert_eq!(
            args,
            strings(&[
                "agent",
                "--local",
                "--json",
                "--session-id",
                "ses-1",
                "--agent",
                "research-bot",
                "--message",
                "Read only\n\ntask"
            ])
        );
    }

    #[test]
    fn args_keep_gateway_mode_and_custom_agent() {
        let options = ExecOptions {
            model: "from-model".to_string(),
            openclaw_mode: "gateway".to_string(),
            custom_args: strings(&["--agent", "from-custom", "--local", "--model", "bad"]),
            thread_name: "prompt".to_string(),
            ..ExecOptions::default()
        };
        let args = build_openclaw_args("prompt", &options);
        assert!(!args.contains(&"--local".to_string()));
        assert_eq!(args.iter().filter(|value| *value == "--agent").count(), 1);
        assert!(args.contains(&"from-custom".to_string()));
        assert!(!args.contains(&"bad".to_string()));
    }

    #[test]
    fn version_parser_requires_three_segments() {
        assert_eq!(
            parse_openclaw_version("openclaw v2026.5.5 abc"),
            Some("2026.5.5".to_string())
        );
        assert_eq!(parse_openclaw_version("openclaw 2026.5"), None);
        assert!(compare_openclaw_version("2026.4.9", MIN_OPENCLAW_VERSION).is_lt());
        assert!(compare_openclaw_version("2026.5.5", MIN_OPENCLAW_VERSION).is_eq());
    }

    #[test]
    fn parse_pretty_result_and_usage() {
        let input = br#"log before
{
  "payloads": [{"text": "hello"}],
  "meta": {"agentMeta": {"sessionId": "ses-2", "model": "deepseek-chat", "usage": {"input": 4, "outputTokens": 3}}}
}
log after"#;
        let messages = mpsc::channel(8).0;
        let result = parse_openclaw_output(input, &messages);
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "hello");
        assert_eq!(result.session_id, "ses-2");
        assert_eq!(result.model, "deepseek-chat");
        assert_eq!(result.usage.input_tokens, 4);
        assert_eq!(result.usage.output_tokens, 3);
    }

    #[test]
    fn parse_ndjson_events_and_canonical_empty_error() {
        let messages = mpsc::channel(8).0;
        let result = parse_openclaw_output(
            br#"{"type":"text","text":"hi","sessionId":"ses-3"}
{"type":"step_finish","usage":{"inputTokens":2,"output":1}}"#,
            &messages,
        );
        assert_eq!(result.output, "hi");
        assert_eq!(result.session_id, "ses-3");
        assert_eq!(result.usage.input_tokens, 2);
        assert_eq!(result.usage.output_tokens, 1);

        let empty = parse_openclaw_output(b"", &messages);
        assert_eq!(empty.status, "failed");
        assert_eq!(empty.error, "openclaw returned no parseable output");
    }

    #[test]
    fn parse_agent_catalog_prefers_id_and_rejects_decoration() {
        let json = br#"[{"id":"sub2api","name":"Sub2API OPS","model":"gpt-4o"}]"#;
        let models = parse_openclaw_agents_json(json).unwrap_or_default();
        assert_eq!(models[0].id, "sub2api");
        assert_eq!(models[0].label, "Sub2API OPS (gpt-4o)");
        let text = parse_openclaw_agents(
            "Identity:\n◇ Agents:\ndeepseek-v4 deepseek-v4\ndeepseek-v4 deepseek-v4\n",
        );
        assert_eq!(text.len(), 1);
        assert_eq!(text[0].id, "deepseek-v4");
    }

    #[cfg(unix)]
    fn fake_backend(script: &str) -> (tempfile::TempDir, OpenclawBackend) {
        let directory = tempfile::tempdir_in(".")
            .unwrap_or_else(|error| panic!("tempdir in workspace: {error}"));
        let executable = directory.path().join("openclaw");
        std::fs::write(&executable, script).unwrap_or_else(|error| panic!("write fake: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake: {error}"));
        (
            directory,
            OpenclawBackend::new(OpenclawConfig {
                command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
                ..OpenclawConfig::default()
            }),
        )
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_checks_version_and_parses_final_result() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'openclaw 2026.5.5'
  exit 0
fi
printf '%s\n' '{"payloads":[{"text":"hello"}],"meta":{"agentMeta":{"sessionId":"ses-openclaw","model":"deepseek-chat","usage":{"input":4,"output":3}}}}'
"#,
        );
        let session = backend
            .execute(
                "do the thing",
                ExecOptions {
                    model: "research-bot".to_string(),
                    resume_session_id: "ses-input".to_string(),
                    timeout: Duration::from_secs(5),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute fake OpenClaw: {error}"));
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
        assert_eq!(result.session_id, "ses-openclaw");
        assert_eq!(result.usage["deepseek-chat"].input_tokens, 4);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_rejects_old_version_before_spawning_agent() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'openclaw 2026.4.9'
  exit 0
fi
exit 99
"#,
        );
        let error = match backend.execute("ignored", ExecOptions::default()).await {
            Ok(_) => panic!("old OpenClaw version must be rejected"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(message.contains("2026.4.9"));
        assert!(message.contains(MIN_OPENCLAW_VERSION));
        assert!(message.contains("openclaw update"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn execute_cuts_short_after_complete_result_from_lingering_process() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo 'openclaw 2026.5.5'
  exit 0
fi
printf '%s\n' '{"payloads":[{"text":"done"}],"meta":{"durationMs":1}}'
sleep 30
"#,
        );
        let session = backend
            .execute(
                "ignored",
                ExecOptions {
                    timeout: Duration::from_secs(10),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute lingering fake OpenClaw: {error}"));
        let Session {
            mut messages,
            result,
        } = session;
        while messages.recv().await.is_some() {}
        let result = result
            .await
            .unwrap_or_else(|error| panic!("receive result: {error}"));
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "done");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn discovery_accepts_json_agent_catalog() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
if [ "$1" = "agents" ] && [ "$2" = "list" ]; then
  printf '%s\n' '[{"id":"sub2api","name":"Sub2API OPS","model":"gpt-4o"}]'
  exit 0
fi
exit 1
"#,
        );
        let catalog = backend
            .discover_models_for_runtime(
                "openclaw-test",
                &CatalogCache::default(),
                CancellationToken::new(),
                Duration::from_secs(5),
            )
            .await;
        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.models[0].id, "sub2api");
        assert_eq!(catalog.models[0].label, "Sub2API OPS (gpt-4o)");
    }
}
