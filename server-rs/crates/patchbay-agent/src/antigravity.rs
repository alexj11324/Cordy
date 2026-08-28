//! Google's Antigravity CLI plain-text print-mode adapter.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader as StdBufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use regex::Regex;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::command::{filter_custom_args, filter_launch_prefix, BlockedArgMode, RuntimeCommand};
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session,
};
use crate::env::configure_child_env;
use crate::model::{Catalog, CatalogCache, Model, ModelDiscoveryCacheKey};
use crate::process::OwnedProcessTree;
use crate::stderr::{with_stderr, SharedDiagnosticBuffer, DEFAULT_TAIL_BYTES};
use crate::stream::AgentLineReader;

const MESSAGE_BUFFER: usize = 256;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);
const NO_CAP_PRINT_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const KILL_GRACE: Duration = Duration::from_secs(10);
const MAX_DISCOVERY_BYTES: u64 = 4 * 1024 * 1024;
const MAX_LOG_LINE_BYTES: usize = 64 * 1024;

pub(crate) static BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> =
    LazyLock::new(|| {
        BTreeMap::from([
            ("-p", BlockedArgMode::WithValue),
            ("--print", BlockedArgMode::WithValue),
            ("--prompt", BlockedArgMode::WithValue),
            ("-i", BlockedArgMode::Standalone),
            ("--prompt-interactive", BlockedArgMode::Standalone),
            ("-c", BlockedArgMode::Standalone),
            ("--continue", BlockedArgMode::Standalone),
            ("--conversation", BlockedArgMode::WithValue),
            ("--model", BlockedArgMode::WithValue),
            ("--print-timeout", BlockedArgMode::WithValue),
            ("--dangerously-skip-permissions", BlockedArgMode::Standalone),
            ("--log-file", BlockedArgMode::WithValue),
            ("--settings", BlockedArgMode::WithValue),
        ])
    });

static CONVERSATION_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"conversation=([0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12})")
        .unwrap_or_else(|error| panic!("invalid Antigravity conversation regex: {error}"))
});
static PRINT_TIMEOUT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Print mode: timed out after \d+ polls")
        .unwrap_or_else(|error| panic!("invalid Antigravity timeout regex: {error}"))
});
static PROVIDER_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"agent executor error:\s*([^\r\n]+)")
        .unwrap_or_else(|error| panic!("invalid Antigravity provider-error regex: {error}"))
});
static APP_DATA_DIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"CLI app data directory:\s*([^\r\n]+)")
        .unwrap_or_else(|error| panic!("invalid Antigravity app-data regex: {error}"))
});

#[derive(Debug, Clone)]
pub struct AntigravityConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
    pub catalog_cache: Arc<CatalogCache>,
}

impl Default for AntigravityConfig {
    fn default() -> Self {
        Self {
            command: RuntimeCommand::default(),
            env: BTreeMap::new(),
            catalog_cache: Arc::new(CatalogCache::default()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AntigravityBackend {
    config: AntigravityConfig,
}

impl AntigravityBackend {
    pub fn new(config: AntigravityConfig) -> Self {
        Self { config }
    }

    pub async fn discover_models(
        &self,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        let Some(key) = ModelDiscoveryCacheKey::new("antigravity", &self.config.command) else {
            return Catalog::default();
        };
        if let Some(catalog) = self.config.catalog_cache.get(&key) {
            return catalog;
        }
        let timeout = if timeout.is_zero() {
            DISCOVERY_TIMEOUT
        } else {
            timeout
        };
        let models = discover_once(&self.config, cancellation, timeout)
            .await
            .unwrap_or_default();
        let catalog = Catalog {
            models,
            fallback: false,
        };
        let _ = self.config.catalog_cache.insert(key, catalog.clone());
        catalog
    }
}

pub fn build_antigravity_args(prompt: &str, log_path: &Path, options: &ExecOptions) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        prompt.to_string(),
        "--dangerously-skip-permissions".to_string(),
    ];
    if !options.model.is_empty() {
        args.extend(["--model".to_string(), options.model.clone()]);
    }
    args.extend([
        "--print-timeout".to_string(),
        format_cli_duration(if options.timeout.is_zero() {
            NO_CAP_PRINT_TIMEOUT
        } else {
            options.timeout
        }),
        "--log-file".to_string(),
        log_path.to_string_lossy().into_owned(),
    ]);
    if !options.resume_session_id.is_empty() {
        args.extend([
            "--conversation".to_string(),
            options.resume_session_id.clone(),
        ]);
    }
    if !options.cwd.is_empty() {
        args.extend([
            "--add-dir".to_string(),
            Path::new(&options.cwd).to_string_lossy().into_owned(),
        ]);
    }
    args.extend(filter_custom_args(&options.extra_args, &BLOCKED_ARGS).args);
    args.extend(filter_custom_args(&options.custom_args, &BLOCKED_ARGS).args);
    args
}

#[async_trait]
impl Backend for AntigravityBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        if !options.model.is_empty() {
            let catalog = self
                .discover_models(options.cancellation.clone(), DISCOVERY_TIMEOUT)
                .await;
            validate_model(&options.model, &catalog.models)?;
        }

        let command_path = if self.config.command.path.is_empty() {
            "agy"
        } else {
            self.config.command.path.as_str()
        };
        let log_file = tempfile::Builder::new()
            .prefix("patchbay-agy-log-")
            .suffix(".log")
            .tempfile()
            .map_err(AgentError::Process)?;
        let log_path = log_file.into_temp_path();
        let prefix = filter_launch_prefix(&self.config.command.prefix, &BLOCKED_ARGS);
        log_blocked("launch prefix", &prefix.blocked_flags);
        log_blocked_args(&options);
        let mut argv = prefix.args;
        argv.extend(build_antigravity_args(prompt, &log_path, &options));

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
            AgentError::Protocol("Antigravity stdout pipe unavailable after spawn".to_string())
        })?;
        let stderr = tree.child_mut().stderr.take().ok_or_else(|| {
            AgentError::Protocol("Antigravity stderr pipe unavailable after spawn".to_string())
        })?;

        let (message_tx, message_rx) = mpsc::channel(MESSAGE_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();
        let timeout = options.timeout;
        let cancellation = options.cancellation.clone();
        let stderr_tail = SharedDiagnosticBuffer::new(DEFAULT_TAIL_BYTES);
        let started = Instant::now();
        let recovery_message_tx = message_tx.clone();

        tokio::spawn(async move {
            let _log_path = log_path;
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
            let mut stdout_task = tokio::spawn(read_plain_text(stdout, message_tx));

            let outcome = {
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

            let (end, exit, stream) = match outcome {
                RunOutcome::Completed((exit, stream)) => (RunEnd::Completed, Some(exit), stream),
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
            let stream = stream.unwrap_or_else(|error| PlainOutput {
                read_error: format!("stdout task failed: {error}"),
                ..PlainOutput::default()
            });
            let markers = scan_provider_log(&_log_path);
            let session_id = markers.conversation_id.clone();
            let mut status = "completed".to_string();
            let mut error = String::new();
            match end {
                RunEnd::Cancelled => {
                    status = "aborted".to_string();
                    error = "execution cancelled".to_string();
                }
                RunEnd::TimedOut => {
                    status = "timeout".to_string();
                    error = format!("agy timed out after {}", format_cli_duration(timeout));
                }
                RunEnd::Completed => {
                    if !stream.read_error.is_empty() {
                        status = "failed".to_string();
                        error = format!("agy stdout read error: {}", stream.read_error);
                    } else if let Some(Err(wait_error)) = exit.as_ref() {
                        status = "failed".to_string();
                        error = format!("agy wait failed: {wait_error}");
                    } else if exit
                        .as_ref()
                        .is_some_and(|exit| exit.as_ref().is_ok_and(|s| !s.success()))
                    {
                        status = "failed".to_string();
                        error = format!(
                            "agy exited with error: {}",
                            exit.as_ref()
                                .and_then(|e| e.as_ref().ok())
                                .map_or_else(|| "unknown".to_string(), ToString::to_string)
                        );
                    } else if markers.print_timeout {
                        status = "timeout".to_string();
                        error = format!("agy --print-timeout elapsed after {} waiting for the agent response; a long-running command likely outlived the print timeout", format_cli_duration(if timeout.is_zero() { NO_CAP_PRINT_TIMEOUT } else { timeout }));
                    } else if let Some(provider_error) = markers.provider_error.as_deref() {
                        status = "failed".to_string();
                        error = format!("agy provider error: {provider_error}");
                    }
                }
            }
            let mut output = stream.output;
            if status == "completed" && output.trim().is_empty() {
                output = recover_transcript(&markers.app_data_dir, &session_id);
                if !output.is_empty() {
                    let _ = recovery_message_tx.try_send(message(MessageType::Text, &output, ""));
                }
            }
            if !error.is_empty() {
                error = with_stderr(&error, "agy", &stderr);
            }
            let _ = result_tx.send(ExecutionResult {
                status,
                output,
                error,
                duration_ms: started.elapsed().as_millis().try_into().unwrap_or(i64::MAX),
                session_id,
                ..ExecutionResult::default()
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
    Cancelled,
    TimedOut,
}

enum RunOutcome<T> {
    Completed(T),
    Cancelled,
    TimedOut,
}

#[derive(Debug, Default)]
struct PlainOutput {
    output: String,
    read_error: String,
}

async fn read_plain_text(
    stdout: tokio::process::ChildStdout,
    messages: mpsc::Sender<Message>,
) -> PlainOutput {
    let mut reader = AgentLineReader::new(BufReader::new(stdout));
    let mut output = PlainOutput::default();
    let _ = messages.try_send(message(MessageType::Status, "", "running"));
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                if !output.output.is_empty() {
                    output.output.push('\n');
                }
                output.output.push_str(&line);
                if !line.trim().is_empty() {
                    let _ = messages.try_send(message(MessageType::Text, &line, ""));
                }
            }
            Ok(None) => return output,
            Err(error) => {
                output.read_error = error.to_string();
                return output;
            }
        }
    }
}

fn message(message_type: MessageType, content: &str, status: &str) -> Message {
    Message {
        message_type,
        content: content.to_string(),
        status: status.to_string(),
        tool: String::new(),
        call_id: String::new(),
        input: BTreeMap::new(),
        output: String::new(),
        level: String::new(),
        session_id: String::new(),
    }
}

async fn discover_once(
    config: &AntigravityConfig,
    cancellation: CancellationToken,
    timeout: Duration,
) -> io::Result<Vec<Model>> {
    let command_path = if config.command.path.is_empty() {
        "agy"
    } else {
        &config.command.path
    };
    let argv = config.command.argv(&["models".to_string()]);
    let mut command = Command::new(command_path);
    command
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(false);
    configure_child_env(&mut command, &config.env);
    let mut tree = OwnedProcessTree::spawn(&mut command).await?;
    let stdout = tree
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("Antigravity model stdout pipe unavailable"))?;
    let outcome = {
        let read = async {
            let mut output = Vec::new();
            let bytes = stdout
                .take(MAX_DISCOVERY_BYTES + 1)
                .read_to_end(&mut output)
                .await?;
            let status = tree.wait().await?;
            if !status.success() || bytes as u64 > MAX_DISCOVERY_BYTES {
                return Ok(Vec::new());
            }
            Ok(parse_models(&String::from_utf8_lossy(&output)))
        };
        tokio::pin!(read);
        tokio::select! {
            result = &mut read => RunOutcome::Completed(result),
            () = cancellation.cancelled() => RunOutcome::Cancelled,
            () = tokio::time::sleep(timeout) => RunOutcome::TimedOut,
        }
    };
    match outcome {
        RunOutcome::Completed(result) => result,
        RunOutcome::Cancelled | RunOutcome::TimedOut => {
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            Ok(Vec::new())
        }
    }
}

fn parse_models(output: &str) -> Vec<Model> {
    let mut seen = BTreeSet::new();
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let id = fields.next().unwrap_or_default().trim();
            if id.is_empty() || !seen.insert(id.to_string()) {
                return None;
            }
            let label = fields
                .next()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .unwrap_or(id);
            Some(Model {
                id: id.to_string(),
                label: label.to_string(),
                provider: "antigravity".to_string(),
                ..Model::default()
            })
        })
        .collect()
}

fn validate_model(model: &str, available: &[Model]) -> Result<(), AgentError> {
    if model.is_empty() || available.is_empty() || available.iter().any(|entry| entry.id == model) {
        return Ok(());
    }
    Err(AgentError::InvalidConfig(format!(
        "antigravity model {model:?} is not available from `agy models`; pick one of: {}",
        available
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

fn format_cli_duration(duration: Duration) -> String {
    let seconds = duration.as_secs().max(1);
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}h{minutes}m{seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn last_optional_capture(regex: &Regex, input: &str) -> Option<String> {
    regex
        .captures_iter(input)
        .filter_map(|captures| {
            captures
                .get(1)
                .map(|value| value.as_str().trim().to_string())
        })
        .last()
}

#[derive(Default)]
struct LogMarkers {
    conversation_id: String,
    app_data_dir: String,
    print_timeout: bool,
    provider_error: Option<String>,
}

fn scan_provider_log(path: &Path) -> LogMarkers {
    let Ok(file) = File::open(path) else {
        return LogMarkers::default();
    };
    let mut reader = StdBufReader::new(file);
    let mut markers = LogMarkers::default();
    let mut line = Vec::new();
    loop {
        line.clear();
        let Ok(bytes) = read_bounded_line(&mut reader, &mut line) else {
            break;
        };
        if bytes == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        if let Some(conversation_id) = last_optional_capture(&CONVERSATION_ID, &text) {
            markers.conversation_id = conversation_id;
        }
        if let Some(app_data) = last_optional_capture(&APP_DATA_DIR, &text) {
            markers.app_data_dir = app_data;
        }
        if PRINT_TIMEOUT.is_match(&text) {
            markers.print_timeout = true;
        }
        if let Some(provider_error) = last_optional_capture(&PROVIDER_ERROR, &text) {
            markers.provider_error = Some(provider_error);
        }
    }
    markers
}

fn read_bounded_line<R: BufRead>(reader: &mut R, line: &mut Vec<u8>) -> io::Result<usize> {
    let limit = u64::try_from(MAX_LOG_LINE_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let bytes = {
        let mut bounded = std::io::Read::take(&mut *reader, limit);
        bounded.read_until(b'\n', line)?
    };
    if line.len() > MAX_LOG_LINE_BYTES && !line.ends_with(b"\n") {
        loop {
            let buffer = reader.fill_buf()?;
            if buffer.is_empty() {
                break;
            }
            if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
                reader.consume(newline + 1);
                break;
            }
            let consumed = buffer.len();
            reader.consume(consumed);
        }
    }
    Ok(bytes)
}

fn recover_transcript(app_data: &str, conversation_id: &str) -> String {
    if conversation_id.is_empty() || app_data.is_empty() {
        return String::new();
    }
    let path = PathBuf::from(app_data)
        .join("brain")
        .join(conversation_id)
        .join(".system_generated")
        .join("logs")
        .join("transcript.jsonl");
    let Ok(transcript) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let mut parts = Vec::new();
    for line in transcript.lines() {
        let Ok(record) = serde_json::from_str::<TranscriptRecord>(line) else {
            continue;
        };
        if record.record_type == "USER_INPUT" {
            parts.clear();
            continue;
        }
        if record.record_type == "PLANNER_RESPONSE"
            && record.source == "MODEL"
            && record.status == "DONE"
        {
            if let Some(text) = record
                .content
                .as_str()
                .filter(|text| !text.trim().is_empty())
            {
                parts.push(text.to_string());
            }
        }
    }
    parts.join("\n\n")
}

#[derive(Deserialize)]
struct TranscriptRecord {
    #[serde(rename = "type")]
    record_type: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    content: serde_json::Value,
}

fn log_blocked_args(options: &ExecOptions) {
    log_blocked(
        "extra arguments",
        &filter_custom_args(&options.extra_args, &BLOCKED_ARGS).blocked_flags,
    );
    log_blocked(
        "custom arguments",
        &filter_custom_args(&options.custom_args, &BLOCKED_ARGS).blocked_flags,
    );
}

fn log_blocked(source: &str, flags: &[String]) {
    if !flags.is_empty() {
        tracing::warn!(provider = "antigravity", source, flags = ?flags, "ignored daemon-owned arguments");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn arguments_preserve_daemon_ownership() {
        let options = ExecOptions {
            cwd: "/work".into(),
            model: "gemini-high".into(),
            resume_session_id: "cid".into(),
            timeout: Duration::from_secs(1200),
            custom_args: strings(&["--model", "bad", "--settings=x", "--add-dir", "/extra"]),
            ..ExecOptions::default()
        };
        let args = build_antigravity_args("secret", Path::new("/tmp/agy.log"), &options);
        assert_eq!(
            args.iter().filter(|arg| arg.as_str() == "--model").count(),
            1
        );
        assert!(!args
            .iter()
            .any(|arg| arg == "bad" || arg.starts_with("--settings")));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--print-timeout", "20m0s"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--conversation", "cid"]));
        assert!(args.windows(2).any(|pair| pair == ["--add-dir", "/extra"]));
    }

    #[test]
    fn model_parser_handles_modern_legacy_and_duplicates() {
        let models = parse_models(
            "gemini-high\tGemini High\nClaude Opus 4.6 (Thinking)\ngemini-high\tduplicate\n",
        );
        assert_eq!(models.len(), 2);
        assert_eq!(
            (models[0].id.as_str(), models[0].label.as_str()),
            ("gemini-high", "Gemini High")
        );
        assert_eq!(models[1].id, "Claude Opus 4.6 (Thinking)");
        assert!(validate_model("missing", &models).is_err());
        assert!(validate_model("anything", &[]).is_ok());
    }

    #[test]
    fn transcript_recovery_returns_only_latest_turn() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let cid = "b8b263a4-4b2f-4339-acc9-78b248e2b606";
        let transcript = directory
            .path()
            .join("brain")
            .join(cid)
            .join(".system_generated/logs/transcript.jsonl");
        std::fs::create_dir_all(
            transcript
                .parent()
                .unwrap_or_else(|| panic!("transcript parent")),
        )
        .unwrap_or_else(|error| panic!("create transcript: {error}"));
        std::fs::write(&transcript, concat!("{\"type\":\"USER_INPUT\",\"content\":\"old\"}\n", "{\"type\":\"PLANNER_RESPONSE\",\"source\":\"MODEL\",\"status\":\"DONE\",\"content\":\"old answer\"}\n", "{\"type\":\"USER_INPUT\",\"content\":\"new\"}\n", "{\"type\":\"PLANNER_RESPONSE\",\"source\":\"MODEL\",\"status\":\"DONE\",\"content\":\"new narration\"}\n", "{\"type\":\"PLANNER_RESPONSE\",\"source\":\"MODEL\",\"status\":\"DONE\",\"content\":\"new answer\"}\n"))        .unwrap_or_else(|error| panic!("write transcript: {error}"));
        assert_eq!(
            recover_transcript(&directory.path().to_string_lossy(), cid),
            "new narration\n\nnew answer"
        );
    }

    #[test]
    fn provider_log_scan_keeps_markers_without_loading_the_whole_file() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let path = directory.path().join("agy.log");
        let cid = "b8b263a4-4b2f-4339-acc9-78b248e2b606";
        let mut body = format!("CLI app data directory: /tmp/agy\nconversation={cid}\n");
        body.push_str(&"x".repeat(64 * 1024));
        body.push_str("\nagent executor error: upstream 502\n");
        std::fs::write(&path, body).unwrap_or_else(|error| panic!("write log: {error}"));
        let markers = scan_provider_log(&path);
        assert_eq!(markers.conversation_id, cid);
        assert_eq!(markers.app_data_dir, "/tmp/agy");
        assert_eq!(markers.provider_error.as_deref(), Some("upstream 502"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_process_classifies_logged_error_and_discovers_models() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let executable = directory.path().join("agy");
        std::fs::write(
            &executable,
            r#"#!/bin/sh
if [ "$1" = models ]; then printf 'gemini-high\tGemini High\n'; exit 0; fi
log=''
while [ $# -gt 0 ]; do case "$1" in --log-file) log="$2"; shift 2;; *) shift;; esac; done
printf 'conversation=b8b263a4-4b2f-4339-acc9-78b248e2b606\n' >> "$log"
printf 'agent executor error: FAILED_PRECONDITION: unsupported location\n' >> "$log"
exit 0
"#,
        )
        .unwrap_or_else(|error| panic!("write fake agy: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake agy: {error}"));
        let backend = AntigravityBackend::new(AntigravityConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            ..AntigravityConfig::default()
        });
        let catalog = backend
            .discover_models(CancellationToken::new(), Duration::from_secs(5))
            .await;
        assert_eq!(
            catalog.models.first().map(|model| model.id.as_str()),
            Some("gemini-high")
        );
        let session = backend
            .execute(
                "prompt",
                ExecOptions {
                    model: "gemini-high".into(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute: {error}"));
        let result = session
            .result
            .await
            .unwrap_or_else(|error| panic!("result: {error}"));
        assert_eq!(result.status, "failed");
        assert!(result.error.contains("FAILED_PRECONDITION"));
        assert_eq!(result.session_id, "b8b263a4-4b2f-4339-acc9-78b248e2b606");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_process_recovers_empty_stdout_into_message_and_result() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let cid = "5f75443f-b6f7-4d89-bf38-31b0adb01fbf";
        let transcript = directory
            .path()
            .join("brain")
            .join(cid)
            .join(".system_generated/logs/transcript.jsonl");
        std::fs::create_dir_all(
            transcript
                .parent()
                .unwrap_or_else(|| panic!("transcript parent")),
        )
        .unwrap_or_else(|error| panic!("create transcript: {error}"));
        std::fs::write(
            &transcript,
            concat!(
                "{\"type\":\"USER_INPUT\",\"content\":\"prompt\"}\n",
                "{\"type\":\"PLANNER_RESPONSE\",\"source\":\"MODEL\",\"status\":\"DONE\",\"content\":\"recovered answer\"}\n"
            ),
        )
        .unwrap_or_else(|error| panic!("write transcript: {error}"));
        let executable = directory.path().join("agy");
        let script = format!(
            r#"#!/bin/sh
log=''
while [ $# -gt 0 ]; do case "$1" in --log-file) log="$2"; shift 2;; *) shift;; esac; done
printf 'CLI app data directory: {}\n' >> "$log"
printf 'conversation={}\n' >> "$log"
exit 0
"#,
            directory.path().display(),
            cid
        );
        std::fs::write(&executable, script)
            .unwrap_or_else(|error| panic!("write fake agy: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake agy: {error}"));
        let backend = AntigravityBackend::new(AntigravityConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            ..AntigravityConfig::default()
        });
        let session = backend
            .execute("prompt", ExecOptions::default())
            .await
            .unwrap_or_else(|error| panic!("execute: {error}"));
        let Session {
            mut messages,
            result,
        } = session;
        let mut recovered_message = false;
        while let Some(message) = messages.recv().await {
            recovered_message |=
                message.message_type == MessageType::Text && message.content == "recovered answer";
        }
        let result = result
            .await
            .unwrap_or_else(|error| panic!("result: {error}"));
        assert!(recovered_message);
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "recovered answer");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_terminates_owned_process_tree() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let executable = directory.path().join("agy");
        std::fs::write(&executable, "#!/bin/sh\nsleep 60 &\nwait\n")
            .unwrap_or_else(|error| panic!("write fake agy: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake agy: {error}"));
        let backend = AntigravityBackend::new(AntigravityConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            ..AntigravityConfig::default()
        });
        let cancellation = CancellationToken::new();
        let session = backend
            .execute(
                "prompt",
                ExecOptions {
                    cancellation: cancellation.clone(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute: {error}"));
        cancellation.cancel();
        let result = tokio::time::timeout(Duration::from_secs(15), session.result)
            .await
            .unwrap_or_else(|error| panic!("cancellation exceeded bound: {error}"))
            .unwrap_or_else(|error| panic!("result: {error}"));
        assert_eq!(result.status, "aborted");
        assert!(result.error.contains("cancelled"));
    }
}
