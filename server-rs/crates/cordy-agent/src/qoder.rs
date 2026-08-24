//! Qoder CLI's headless ACP runner for both global binary variants.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::acp::{
    default_permission_decision, AcpClient, AcpError, AcpNotification, AcpPermissionDecision,
};
use crate::acp_mcp::{
    build_acp_mcp_servers, filter_acp_mcp_servers, parse_acp_mcp_capabilities, AcpMcpServer,
};
use crate::command::{filter_custom_args, filter_launch_prefix, BlockedArgMode, RuntimeCommand};
use crate::contract::{
    AgentError, Backend, ExecOptions, ExecutionResult, Message, MessageType, Session, TokenUsage,
    COST_USD_TICKS_PER_USD,
};
use crate::kimi_usage::{scan_kimi_session_usage, KimiUsageScan};
use crate::model::{parse_acp_session_models, Catalog, CatalogCache, ModelDiscoveryCacheKey};
use crate::process::OwnedProcessTree;
use crate::stderr::{with_stderr, SharedDiagnosticBuffer, DEFAULT_TAIL_BYTES};
use crate::version::check_minimum;

const MESSAGE_BUFFER: usize = 256;
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const KILL_GRACE: Duration = Duration::from_secs(10);
const NOTIFICATION_QUIET: Duration = Duration::from_millis(250);
const NOTIFICATION_DRAIN_MAX: Duration = Duration::from_secs(2);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVERY_OUTPUT_MAX: u64 = 4 * 1024 * 1024;
const KIMI_THINKING_MIN_VERSION: &str = "0.29.0";

static BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> = LazyLock::new(|| {
    BTreeMap::from([
        ("--acp", BlockedArgMode::Standalone),
        ("acp", BlockedArgMode::Standalone),
        ("--yolo", BlockedArgMode::Standalone),
    ])
});
static TRAECLI_BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> =
    LazyLock::new(|| {
        BTreeMap::from([
            ("acp", BlockedArgMode::Standalone),
            ("serve", BlockedArgMode::Standalone),
            ("-y", BlockedArgMode::Standalone),
            ("--yolo", BlockedArgMode::Standalone),
            ("-p", BlockedArgMode::Standalone),
            ("--print", BlockedArgMode::Standalone),
            ("--output-format", BlockedArgMode::WithValue),
            ("--permission-mode", BlockedArgMode::WithValue),
        ])
    });
static KIRO_BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> = LazyLock::new(|| {
    BTreeMap::from([
        ("acp", BlockedArgMode::Standalone),
        ("-a", BlockedArgMode::Standalone),
        ("--trust-all-tools", BlockedArgMode::Standalone),
        ("--trust-tools", BlockedArgMode::WithValue),
    ])
});
static QWENPAW_BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> =
    LazyLock::new(|| {
        BTreeMap::from([
            ("acp", BlockedArgMode::Standalone),
            ("--workspace", BlockedArgMode::WithValue),
        ])
    });
static KIMI_BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> =
    LazyLock::new(|| BTreeMap::from([("acp", BlockedArgMode::Standalone)]));
static REASONIX_BLOCKED_ARGS: LazyLock<BTreeMap<&'static str, BlockedArgMode>> =
    LazyLock::new(|| {
        BTreeMap::from([
            ("acp", BlockedArgMode::Standalone),
            ("--model", BlockedArgMode::WithValue),
            ("--profile", BlockedArgMode::WithValue),
            ("--planner", BlockedArgMode::WithValue),
            ("--sandbox-network", BlockedArgMode::WithValue),
            ("--sandbox-bash", BlockedArgMode::WithValue),
            ("--workspace-only", BlockedArgMode::Standalone),
        ])
    });
static TERMINAL_PROVIDER_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:(?:⚠️|❌|\[ERROR\]).*(?:BadRequestError|AuthenticationError|RateLimitError|HTTP \d{3}|Non-retryable|API call failed)|API call failed after \d+ retr(?:y|ies))")
        .unwrap_or_else(|error| panic!("invalid Qoder provider-error regex: {error}"))
});
static OUTPUT_PROVIDER_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)API call failed after \d+ retr(?:y|ies)")
        .unwrap_or_else(|error| panic!("invalid Qoder output-error regex: {error}"))
});

#[derive(Debug, Clone)]
pub struct QoderConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
    pub default_command: String,
    pub provider: String,
    pub launch_args: Vec<String>,
    pub discovery_args: Vec<String>,
    pub resume_method: String,
    pub prompt_content_alias: bool,
    pub model_selection: bool,
    pub reject_failed_load: bool,
    pub coding_project_meta: Option<String>,
    pub full_text_output: bool,
    pub usage_model_unknown: bool,
    pub use_system_prompt: bool,
    pub strict_stop_reason: bool,
}

impl Default for QoderConfig {
    fn default() -> Self {
        Self {
            command: RuntimeCommand::default(),
            env: BTreeMap::new(),
            default_command: "qodercli".to_string(),
            provider: "qoder".to_string(),
            launch_args: vec!["--yolo".to_string(), "--acp".to_string()],
            discovery_args: vec!["--yolo".to_string(), "--acp".to_string()],
            resume_method: "session/resume".to_string(),
            prompt_content_alias: false,
            model_selection: true,
            reject_failed_load: false,
            coding_project_meta: None,
            full_text_output: false,
            usage_model_unknown: false,
            use_system_prompt: true,
            strict_stop_reason: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct QoderBackend {
    config: QoderConfig,
}

impl QoderBackend {
    pub fn new(config: QoderConfig) -> Self {
        Self { config }
    }

    pub async fn discover_models(
        &self,
        cache: &CatalogCache,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        discover_models(&self.config, cache, cancellation, timeout).await
    }
}

#[derive(Debug, Clone, Default)]
pub struct TraecliConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct TraecliBackend {
    inner: QoderBackend,
}

impl TraecliBackend {
    pub fn new(config: TraecliConfig) -> Self {
        Self {
            inner: QoderBackend::new(QoderConfig {
                command: config.command,
                env: config.env,
                default_command: "traecli".to_string(),
                provider: "traecli".to_string(),
                launch_args: ["acp", "serve", "--yolo"].map(str::to_string).to_vec(),
                discovery_args: ["acp", "serve", "--yolo"].map(str::to_string).to_vec(),
                resume_method: "session/load".to_string(),
                prompt_content_alias: false,
                model_selection: true,
                reject_failed_load: false,
                coding_project_meta: None,
                full_text_output: false,
                usage_model_unknown: false,
                use_system_prompt: true,
                strict_stop_reason: false,
            }),
        }
    }

    pub async fn discover_models(
        &self,
        cache: &CatalogCache,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        self.inner
            .discover_models(cache, cancellation, timeout)
            .await
    }
}

#[async_trait]
impl Backend for TraecliBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        self.inner.execute(prompt, options).await
    }
}

#[derive(Debug, Clone, Default)]
pub struct KiroConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct KiroBackend {
    inner: QoderBackend,
}

impl KiroBackend {
    pub fn new(config: KiroConfig) -> Self {
        Self {
            inner: QoderBackend::new(QoderConfig {
                command: config.command,
                env: config.env,
                default_command: "kiro-cli".to_string(),
                provider: "kiro".to_string(),
                launch_args: ["acp", "--trust-all-tools"].map(str::to_string).to_vec(),
                discovery_args: vec!["acp".to_string()],
                resume_method: "session/load".to_string(),
                prompt_content_alias: true,
                model_selection: true,
                reject_failed_load: false,
                coding_project_meta: None,
                full_text_output: false,
                usage_model_unknown: false,
                use_system_prompt: true,
                strict_stop_reason: false,
            }),
        }
    }

    pub async fn discover_models(
        &self,
        cache: &CatalogCache,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        self.inner
            .discover_models(cache, cancellation, timeout)
            .await
    }
}

#[async_trait]
impl Backend for KiroBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        self.inner.execute(prompt, options).await
    }
}

#[derive(Debug, Clone, Default)]
pub struct QwenpawConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct QwenpawBackend {
    inner: QoderBackend,
}

impl QwenpawBackend {
    pub fn new(config: QwenpawConfig) -> Self {
        Self {
            inner: QoderBackend::new(QoderConfig {
                command: config.command,
                env: config.env,
                default_command: "qwenpaw".to_string(),
                provider: "qwenpaw".to_string(),
                launch_args: vec!["acp".to_string()],
                discovery_args: Vec::new(),
                resume_method: "session/load".to_string(),
                prompt_content_alias: false,
                model_selection: false,
                reject_failed_load: true,
                coding_project_meta: Some("qwenpaw.coding_project_dir".to_string()),
                full_text_output: true,
                usage_model_unknown: true,
                use_system_prompt: true,
                strict_stop_reason: false,
            }),
        }
    }
}

#[async_trait]
impl Backend for QwenpawBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        self.inner.execute(prompt, options).await
    }
}

#[derive(Debug, Clone, Default)]
pub struct KimiConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct KimiBackend {
    inner: QoderBackend,
}

impl KimiBackend {
    pub fn new(config: KimiConfig) -> Self {
        Self {
            inner: QoderBackend::new(QoderConfig {
                command: config.command,
                env: config.env,
                default_command: "kimi".to_string(),
                provider: "kimi".to_string(),
                launch_args: vec!["acp".to_string()],
                discovery_args: vec!["acp".to_string()],
                ..QoderConfig::default()
            }),
        }
    }

    pub async fn discover_models(
        &self,
        cache: &CatalogCache,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        discover_kimi_models(&self.inner.config, cache, cancellation, timeout).await
    }
}

#[async_trait]
impl Backend for KimiBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        self.inner.execute(prompt, options).await
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReasonixConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ReasonixBackend {
    inner: QoderBackend,
}

impl ReasonixBackend {
    pub fn new(config: ReasonixConfig) -> Self {
        let launch_args = [
            "acp",
            "--profile",
            "balanced",
            "--planner",
            "auto",
            "--sandbox-network",
            "auto",
            "--sandbox-bash",
            "auto",
            "--workspace-only",
        ]
        .map(str::to_string)
        .to_vec();
        Self {
            inner: QoderBackend::new(QoderConfig {
                command: config.command,
                env: config.env,
                default_command: "reasonix".to_string(),
                provider: "reasonix".to_string(),
                discovery_args: launch_args.clone(),
                launch_args,
                reject_failed_load: true,
                use_system_prompt: false,
                strict_stop_reason: true,
                ..QoderConfig::default()
            }),
        }
    }
}

#[async_trait]
impl Backend for ReasonixBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        self.inner.execute(prompt, options).await
    }
}

pub fn build_qoder_args(options: &ExecOptions) -> Vec<String> {
    build_session_args(&QoderConfig::default(), options)
}

pub fn build_traecli_args(options: &ExecOptions) -> Vec<String> {
    build_session_args(
        &QoderConfig {
            provider: "traecli".to_string(),
            launch_args: ["acp", "serve", "--yolo"].map(str::to_string).to_vec(),
            resume_method: "session/load".to_string(),
            default_command: "traecli".to_string(),
            ..QoderConfig::default()
        },
        options,
    )
}

pub fn build_kiro_args(options: &ExecOptions) -> Vec<String> {
    build_session_args(
        &QoderConfig {
            provider: "kiro".to_string(),
            launch_args: ["acp", "--trust-all-tools"].map(str::to_string).to_vec(),
            discovery_args: vec!["acp".to_string()],
            resume_method: "session/load".to_string(),
            default_command: "kiro-cli".to_string(),
            prompt_content_alias: true,
            ..QoderConfig::default()
        },
        options,
    )
}

pub fn build_qwenpaw_args(options: &ExecOptions) -> Vec<String> {
    build_session_args(
        &QoderConfig {
            provider: "qwenpaw".to_string(),
            launch_args: vec!["acp".to_string()],
            default_command: "qwenpaw".to_string(),
            model_selection: false,
            reject_failed_load: true,
            coding_project_meta: Some("qwenpaw.coding_project_dir".to_string()),
            full_text_output: true,
            usage_model_unknown: true,
            ..QoderConfig::default()
        },
        options,
    )
}

pub fn build_kimi_args(options: &ExecOptions) -> Vec<String> {
    build_session_args(
        &QoderConfig {
            provider: "kimi".to_string(),
            launch_args: vec!["acp".to_string()],
            discovery_args: vec!["acp".to_string()],
            default_command: "kimi".to_string(),
            ..QoderConfig::default()
        },
        options,
    )
}

pub fn build_reasonix_args(options: &ExecOptions) -> Vec<String> {
    let backend = ReasonixBackend::new(ReasonixConfig::default());
    build_session_args(&backend.inner.config, options)
}

fn build_session_args(config: &QoderConfig, options: &ExecOptions) -> Vec<String> {
    let blocked = blocked_args(&config.provider);
    let mut args = config.launch_args.clone();
    args.extend(filter_custom_args(&options.extra_args, blocked).args);
    args.extend(filter_custom_args(&options.custom_args, blocked).args);
    if config.provider == "qwenpaw" && !options.qwenpaw_workspace.is_empty() {
        args.extend(["--workspace".to_string(), options.qwenpaw_workspace.clone()]);
    }
    args
}

fn blocked_args(provider: &str) -> &'static BTreeMap<&'static str, BlockedArgMode> {
    match provider {
        "traecli" => &TRAECLI_BLOCKED_ARGS,
        "kiro" => &KIRO_BLOCKED_ARGS,
        "qwenpaw" => &QWENPAW_BLOCKED_ARGS,
        "kimi" => &KIMI_BLOCKED_ARGS,
        "reasonix" => &REASONIX_BLOCKED_ARGS,
        _ => &BLOCKED_ARGS,
    }
}

async fn discover_models(
    config: &QoderConfig,
    cache: &CatalogCache,
    cancellation: CancellationToken,
    timeout: Duration,
) -> Catalog {
    let Some(key) = ModelDiscoveryCacheKey::new(&config.provider, &config.command) else {
        return Catalog::default();
    };
    if let Some(catalog) = cache.get(&key) {
        return catalog;
    }
    let catalog = discover_acp_session(config, cancellation, timeout)
        .await
        .map_or_else(Catalog::default, |(_, session)| Catalog {
            models: parse_acp_session_models(&session, &config.provider),
            fallback: false,
        });
    let _ = cache.insert(key, catalog.clone());
    catalog
}

async fn discover_acp_session(
    config: &QoderConfig,
    cancellation: CancellationToken,
    timeout: Duration,
) -> Option<(Value, Value)> {
    let command_path = if config.command.path.is_empty() {
        config.default_command.as_str()
    } else {
        config.command.path.as_str()
    };
    let blocked = blocked_args(&config.provider);
    let prefix = filter_launch_prefix(&config.command.prefix, blocked);
    let mut argv = prefix.args;
    argv.extend(config.discovery_args.clone());
    let mut command = Command::new(command_path);
    command
        .args(argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .envs(&config.env)
        .kill_on_drop(false);
    let mut tree = match OwnedProcessTree::spawn(&mut command).await {
        Ok(tree) => tree,
        Err(error) => {
            tracing::debug!(provider = %config.provider, error = %error, "ACP model discovery process failed to start");
            return None;
        }
    };
    let Some(stdin) = tree.child_mut().stdin.take() else {
        let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
        return None;
    };
    let Some(stdout) = tree.child_mut().stdout.take() else {
        let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
        return None;
    };
    let provider = config.provider.clone();
    let mut handshake = tokio::spawn(async move {
        let mut client = AcpClient::new(BufReader::new(stdout), stdin);
        let initialize = client
            .request(
                "initialize",
                serde_json::json!({"protocolVersion":1,"clientInfo":{"name":"cordy-model-discovery","version":"0.1.0"},"clientCapabilities":{}}),
                |_| {},
            )
            .await?;
        let directory = tempfile::Builder::new()
            .prefix(&format!("cordy-{provider}-discovery-"))
            .tempdir()
            .map_err(AcpError::Transport)?;
        let session = client
            .request(
                "session/new",
                serde_json::json!({"cwd":directory.path().to_string_lossy(),"mcpServers":[]}),
                |_| {},
            )
            .await?;
        Ok::<_, AcpError>((initialize, session))
    });
    let timeout = if timeout.is_zero() {
        DISCOVERY_TIMEOUT
    } else {
        timeout
    };
    let result = tokio::select! {
        result = &mut handshake => result.ok().and_then(Result::ok),
        () = cancellation.cancelled() => None,
        () = tokio::time::sleep(timeout) => None,
    };
    let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
    if !handshake.is_finished() {
        handshake.abort();
    }
    result
}

async fn discover_kimi_models(
    config: &QoderConfig,
    cache: &CatalogCache,
    cancellation: CancellationToken,
    timeout: Duration,
) -> Catalog {
    let Some(key) = ModelDiscoveryCacheKey::new("kimi", &config.command) else {
        return Catalog::default();
    };
    if let Some(catalog) = cache.get(&key) {
        return catalog;
    }
    let Some((initialize, session)) =
        discover_acp_session(config, cancellation.clone(), timeout).await
    else {
        return Catalog::default();
    };
    let mut catalog = Catalog {
        models: parse_acp_session_models(&session, "kimi"),
        fallback: false,
    };
    let version = initialize
        .get("agentInfo")
        .or_else(|| initialize.get("agent_info"))
        .and_then(|info| info.get("version"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if check_minimum(version, KIMI_THINKING_MIN_VERSION, false).is_ok() {
        if let Some(thinking) = discover_kimi_thinking(config, cancellation, timeout).await {
            for model in &mut catalog.models {
                if let Some(model_thinking) = thinking.get(&model.id) {
                    model.thinking = Some(model_thinking.clone());
                }
            }
        } else {
            tracing::debug!(
                provider = "kimi",
                reason = "provider_list_unavailable",
                "per-model thinking discovery unavailable"
            );
        }
    }
    let _ = cache.insert(key, catalog.clone());
    catalog
}

async fn discover_kimi_thinking(
    config: &QoderConfig,
    cancellation: CancellationToken,
    timeout: Duration,
) -> Option<BTreeMap<String, crate::model::ModelThinking>> {
    let command_path = if config.command.path.is_empty() {
        config.default_command.as_str()
    } else {
        config.command.path.as_str()
    };
    let prefix = filter_launch_prefix(&config.command.prefix, &KIMI_BLOCKED_ARGS);
    let mut argv = prefix.args;
    argv.extend(["provider", "list", "--json"].map(str::to_string));
    let mut command = Command::new(command_path);
    command
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .envs(&config.env)
        .kill_on_drop(false);
    let mut tree = OwnedProcessTree::spawn(&mut command).await.ok()?;
    let stdout = tree.child_mut().stdout.take()?;
    let mut reader = tokio::spawn(async move {
        let mut output = Vec::new();
        stdout
            .take(DISCOVERY_OUTPUT_MAX.saturating_add(1))
            .read_to_end(&mut output)
            .await
            .map(|_| output)
    });
    let timeout = if timeout.is_zero() {
        DISCOVERY_TIMEOUT
    } else {
        timeout
    };
    let succeeded = tokio::select! {
        status = tree.wait() => status.is_ok_and(|status| status.success()),
        () = cancellation.cancelled() => false,
        () = tokio::time::sleep(timeout) => false,
    };
    let output = if succeeded {
        tokio::time::timeout(TERMINATION_GRACE, &mut reader)
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(Result::ok)
            .filter(|output| {
                u64::try_from(output.len()).is_ok_and(|len| len <= DISCOVERY_OUTPUT_MAX)
            })
    } else {
        None
    };
    let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
    if !reader.is_finished() {
        reader.abort();
    }
    output.and_then(|output| parse_kimi_thinking(&output))
}

fn parse_kimi_thinking(output: &[u8]) -> Option<BTreeMap<String, crate::model::ModelThinking>> {
    let response: Value = serde_json::from_slice(output).ok()?;
    let models = response.get("models")?.as_object()?;
    if models.is_empty() {
        return None;
    }
    let mut parsed = BTreeMap::new();
    for (model_id, model) in models {
        let model_id = model_id.trim();
        if model_id.is_empty() {
            continue;
        }
        let efforts = model
            .get("supportEfforts")
            .or_else(|| model.get("support_efforts"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut seen = std::collections::BTreeSet::new();
        let supported_levels: Vec<_> = efforts
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|effort| valid_dynamic_value(effort) && seen.insert((*effort).to_string()))
            .map(|effort| crate::model::ThinkingLevel {
                value: effort.to_string(),
                label: thinking_label(effort),
                description: String::new(),
            })
            .collect();
        if supported_levels.is_empty() {
            continue;
        }
        let default_level = model
            .get("defaultEffort")
            .or_else(|| model.get("default_effort"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|default| seen.contains(*default))
            .unwrap_or_default()
            .to_string();
        parsed.insert(
            model_id.to_string(),
            crate::model::ModelThinking {
                supported_levels,
                default_level,
            },
        );
    }
    Some(parsed)
}

fn valid_dynamic_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.chars().enumerate().all(|(index, character)| {
            character.is_ascii_alphanumeric()
                || ((character == '-' || character == '_' || character == '.') && index > 0)
        })
}

fn thinking_label(value: &str) -> String {
    match value {
        "low" => "Low".to_string(),
        "medium" => "Medium".to_string(),
        "high" => "High".to_string(),
        "max" => "Max".to_string(),
        _ => {
            let mut characters = value.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + characters.as_str()
            })
        }
    }
}

#[async_trait]
impl Backend for QoderBackend {
    async fn execute(&self, prompt: &str, options: ExecOptions) -> Result<Session, AgentError> {
        let mcp_servers = build_acp_mcp_servers(options.mcp_config.as_ref())?;
        let command_path = if self.config.command.path.is_empty() {
            self.config.default_command.as_str()
        } else {
            self.config.command.path.as_str()
        };
        let blocked = blocked_args(&self.config.provider);
        let prefix = filter_launch_prefix(&self.config.command.prefix, blocked);
        log_blocked(
            &self.config.provider,
            "launch prefix",
            &prefix.blocked_flags,
        );
        log_blocked_args(&self.config.provider, blocked, &options);
        let mut argv = prefix.args;
        argv.extend(build_session_args(&self.config, &options));
        let mut command = Command::new(command_path);
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
                    AgentError::ExecutableNotFound(command_path.to_string())
                } else {
                    AgentError::Process(error)
                }
            })?;
        let stdin = tree.child_mut().stdin.take().ok_or_else(|| {
            AgentError::Protocol("Qoder stdin pipe unavailable after spawn".to_string())
        })?;
        let stdout = tree.child_mut().stdout.take().ok_or_else(|| {
            AgentError::Protocol("Qoder stdout pipe unavailable after spawn".to_string())
        })?;
        let stderr = tree.child_mut().stderr.take().ok_or_else(|| {
            AgentError::Protocol("Qoder stderr pipe unavailable after spawn".to_string())
        })?;

        let (message_tx, message_rx) = mpsc::channel(MESSAGE_BUFFER);
        let (result_tx, result_rx) = oneshot::channel();
        let timeout = options.timeout;
        let cancellation = options.cancellation.clone();
        let started = Instant::now();
        let started_at = SystemTime::now();
        let stderr_tail = SharedDiagnosticBuffer::new(DEFAULT_TAIL_BYTES);
        let prompt = prompt.to_string();
        let provider = self.config.provider.clone();
        let resume_method = self.config.resume_method.clone();
        let prompt_content_alias = self.config.prompt_content_alias;
        let model_selection = self.config.model_selection;
        let reject_failed_load = self.config.reject_failed_load;
        let coding_project_meta = self.config.coding_project_meta.clone();
        let full_text_output = self.config.full_text_output;
        let usage_model_unknown = self.config.usage_model_unknown;
        let use_system_prompt = self.config.use_system_prompt;
        let strict_stop_reason = self.config.strict_stop_reason;
        let kimi_home = self.config.env.get("KIMI_CODE_HOME").cloned();
        let resumed = !options.resume_session_id.is_empty();
        let fallback_model = if options.model.is_empty() {
            "unknown".to_string()
        } else {
            options.model.clone()
        };

        tokio::spawn(async move {
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
            let mut protocol_task = tokio::spawn(run_protocol(
                stdin,
                stdout,
                prompt,
                options,
                mcp_servers,
                message_tx,
                provider.clone(),
                resume_method,
                prompt_content_alias,
                model_selection,
                reject_failed_load,
                coding_project_meta,
                full_text_output,
                usage_model_unknown,
                use_system_prompt,
                strict_stop_reason,
            ));
            let end = if timeout.is_zero() {
                tokio::select! {
                    result = &mut protocol_task => RunEnd::Protocol(result),
                    () = cancellation.cancelled() => RunEnd::Cancelled,
                }
            } else {
                tokio::select! {
                    result = &mut protocol_task => RunEnd::Protocol(result),
                    () = cancellation.cancelled() => RunEnd::Cancelled,
                    () = tokio::time::sleep(timeout) => RunEnd::TimedOut,
                }
            };
            let mut outcome = match end {
                RunEnd::Protocol(result) => result.unwrap_or_else(|error| {
                    ProtocolOutcome::failed(format!("{provider} protocol task failed: {error}"))
                }),
                RunEnd::Cancelled => ProtocolOutcome::terminal("aborted", "execution cancelled"),
                RunEnd::TimedOut => ProtocolOutcome::terminal(
                    "timeout",
                    format!("{provider} timed out after {}s", timeout.as_secs_f64()),
                ),
            };
            let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
            if !protocol_task.is_finished() {
                protocol_task.abort();
            }
            if tokio::time::timeout(KILL_GRACE, &mut stderr_task)
                .await
                .is_err()
            {
                stderr_task.abort();
            }
            let stderr = stderr_tail.tail();
            if outcome.status == "completed" {
                if let Some(provider_error) =
                    provider_error(&provider, &stderr, &outcome.full_output)
                {
                    outcome.status = "failed".to_string();
                    outcome.error = provider_error;
                }
            }
            if provider == "kimi" && !has_token_usage(&outcome.usage) {
                let provider_cost = outcome
                    .usage
                    .values()
                    .map(|usage| usage.cost_usd_ticks)
                    .max()
                    .unwrap_or(0);
                outcome.usage = scan_kimi_session_usage(KimiUsageScan {
                    started_at,
                    configured_home: kimi_home.as_deref(),
                    session_id: &outcome.session_id,
                    resumed,
                    fallback_model: &fallback_model,
                });
                if provider_cost > 0 {
                    outcome
                        .usage
                        .entry(fallback_model.clone())
                        .or_default()
                        .cost_usd_ticks = provider_cost;
                }
            }
            if !outcome.error.is_empty() {
                outcome.error = with_stderr(&outcome.error, &provider, &stderr);
            }
            let _ = result_tx.send(ExecutionResult {
                status: outcome.status,
                output: outcome.output,
                error: outcome.error,
                duration_ms: started.elapsed().as_millis().try_into().unwrap_or(i64::MAX),
                session_id: outcome.session_id,
                usage: outcome.usage,
                resume_rejected: outcome.resume_rejected,
            });
        });

        Ok(Session {
            messages: message_rx,
            result: result_rx,
        })
    }
}

enum RunEnd {
    Protocol(Result<ProtocolOutcome, tokio::task::JoinError>),
    Cancelled,
    TimedOut,
}

#[derive(Default)]
struct ProtocolOutcome {
    status: String,
    output: String,
    full_output: String,
    error: String,
    session_id: String,
    usage: BTreeMap<String, TokenUsage>,
    resume_rejected: bool,
}

impl ProtocolOutcome {
    fn failed(error: impl Into<String>) -> Self {
        Self::terminal("failed", error)
    }

    fn terminal(status: &str, error: impl Into<String>) -> Self {
        Self {
            status: status.to_string(),
            error: error.into(),
            ..Self::default()
        }
    }
}

async fn run_protocol(
    stdin: ChildStdin,
    stdout: ChildStdout,
    prompt: String,
    options: ExecOptions,
    mcp_servers: Vec<AcpMcpServer>,
    messages: mpsc::Sender<Message>,
    provider: String,
    resume_method: String,
    prompt_content_alias: bool,
    model_selection: bool,
    reject_failed_load: bool,
    coding_project_meta: Option<String>,
    full_text_output: bool,
    usage_model_unknown: bool,
    use_system_prompt: bool,
    strict_stop_reason: bool,
) -> ProtocolOutcome {
    let mut client = AcpClient::new(BufReader::new(stdout), stdin);
    let initialize = match client
        .request(
            "initialize",
            serde_json::json!({"protocolVersion":1,"clientInfo":{"name":"cordy-agent-sdk","version":"0.2.0"},"clientCapabilities":{}}),
            |_| {},
        )
        .await
    {
        Ok(result) => result,
        Err(error) => return ProtocolOutcome::failed(format!("{provider} initialize failed: {error}")),
    };
    if provider == "reasonix" && reasonix_capability_versions(&initialize) != (1, 1) {
        tracing::warn!(
            provider = "reasonix",
            "ACP status capabilities are unavailable or incompatible; usage and cost may be incomplete"
        );
    }
    let mcp_servers =
        filter_acp_mcp_servers(mcp_servers, parse_acp_mcp_capabilities(&initialize), false);
    let cwd = if options.cwd.is_empty() {
        "."
    } else {
        &options.cwd
    };
    let (session_result, mut session_id) = if options.resume_session_id.is_empty() {
        match client
            .request(
                "session/new",
                session_params(cwd, None, mcp_servers, coding_project_meta.as_deref()),
                |_| {},
            )
            .await
        {
            Ok(result) => {
                let session_id = extract_session_id(&result);
                if session_id.is_empty() {
                    return ProtocolOutcome::failed(format!(
                        "{provider} session/new returned no session ID"
                    ));
                }
                (result, session_id)
            }
            Err(error) => {
                return ProtocolOutcome::failed(if provider == "reasonix" {
                    reasonix_session_new_error(&error)
                } else {
                    format!("{provider} session/new failed: {error}")
                })
            }
        }
    } else {
        match client
            .request(
                &resume_method,
                session_params(
                    cwd,
                    Some(&options.resume_session_id),
                    mcp_servers,
                    coding_project_meta.as_deref(),
                ),
                |_| {},
            )
            .await
        {
            Ok(result) => {
                let returned = extract_session_id(&result);
                let session_id = if returned.is_empty() {
                    options.resume_session_id.clone()
                } else {
                    returned
                };
                (result, session_id)
            }
            Err(error) => {
                let rejected = reject_failed_load && error.is_session_not_found();
                if provider == "reasonix" && is_reasonix_lease_conflict(&error) {
                    return ProtocolOutcome::failed(format!(
                        "reasonix session is already in use; close the other Reasonix window or process first: {error}"
                    ));
                }
                return protocol_failure(&provider, &resume_method, error, String::new(), rejected);
            }
        }
    };
    let mut effective_model = if usage_model_unknown || !model_selection {
        "unknown".to_string()
    } else if matches!(provider.as_str(), "kimi" | "reasonix") {
        options.model.clone()
    } else if options.model.is_empty() {
        extract_current_model(&session_result)
    } else {
        options.model.clone()
    };
    if model_selection && !options.model.is_empty() {
        if let Err(error) = client
            .request(
                "session/set_model",
                serde_json::json!({"sessionId":session_id,"modelId":options.model}),
                |_| {},
            )
            .await
        {
            let rejected = !options.resume_session_id.is_empty() && error.is_session_not_found();
            if rejected {
                session_id.clear();
            }
            let stage = format!("could not switch to model {:?}", options.model);
            return protocol_failure(&provider, &stage, error, session_id, rejected);
        }
    }
    if effective_model.is_empty() {
        effective_model = "unknown".to_string();
    }
    if provider == "kimi" && !options.thinking_level.is_empty() {
        let requested_level = options.thinking_level.clone();
        match client
            .request(
                "session/set_config_option",
                serde_json::json!({
                    "sessionId":session_id,
                    "configId":"thinking",
                    "value":requested_level,
                }),
                |_| {},
            )
            .await
        {
            Ok(result) => {
                let confirmed = extract_config_value(&result, "thinking");
                if confirmed.as_deref() != Some(requested_level.as_str()) {
                    tracing::warn!(
                        provider = "kimi",
                        requested_level,
                        effective_level = confirmed.as_deref().unwrap_or("unknown"),
                        "runtime did not confirm requested thinking level; continuing"
                    );
                }
            }
            Err(error) => tracing::warn!(
                provider = "kimi",
                requested_level,
                error = %error,
                "runtime rejected requested thinking level; continuing"
            ),
        }
    }
    if provider == "reasonix" && !options.thinking_level.is_empty() {
        apply_reasonix_effort(
            &mut client,
            &session_result,
            &session_id,
            &options.thinking_level,
            options.model.is_empty(),
        )
        .await;
    }
    let user_text = if !use_system_prompt || options.system_prompt.is_empty() {
        prompt
    } else {
        format!("{}\n\n---\n\n{}", options.system_prompt, prompt)
    };
    let mut state = NotificationState {
        kiro_dialect: provider == "kiro",
        extended_tool_names: matches!(provider.as_str(), "kiro" | "kimi" | "reasonix"),
        reasonix_dialect: provider == "reasonix",
        ..NotificationState::default()
    };
    let prompt_blocks = serde_json::json!([{"type":"text","text":user_text}]);
    let prompt_params = if prompt_content_alias {
        serde_json::json!({
            "sessionId": session_id,
            "content": prompt_blocks.clone(),
            "prompt": prompt_blocks,
        })
    } else {
        serde_json::json!({"sessionId":session_id,"prompt":prompt_blocks})
    };
    let mut blocked_question = String::new();
    let mut missing_permission_metadata = false;
    let prompt_result = if provider == "reasonix" {
        client
            .request_with_permission(
                "session/prompt",
                prompt_params,
                |notification| handle_notification(notification, &messages, &mut state),
                |params| {
                    let decision = reasonix_permission_decision(params);
                    if let Some(question) = decision.question {
                        blocked_question = question;
                    }
                    missing_permission_metadata |= decision.metadata_missing;
                    decision.decision
                },
            )
            .await
    } else {
        client
            .request("session/prompt", prompt_params, |notification| {
                handle_notification(notification, &messages, &mut state)
            })
            .await
    };
    let prompt_result = match prompt_result {
        Ok(result) => result,
        Err(error) => {
            let rejected = !options.resume_session_id.is_empty() && error.is_session_not_found();
            if rejected {
                session_id.clear();
            }
            if provider == "kiro"
                && is_kiro_close_error(&error)
                && state.last_finishing_status == "completed"
            {
                let (mut output, full_output) = state.deliverable.finish();
                if full_text_output {
                    output.clone_from(&full_output);
                }
                return ProtocolOutcome {
                    status: "completed".to_string(),
                    output,
                    full_output,
                    session_id,
                    ..ProtocolOutcome::default()
                };
            }
            if provider == "reasonix" && !blocked_question.is_empty() {
                return ProtocolOutcome {
                    status: "failed".to_string(),
                    error: blocked_question,
                    session_id,
                    resume_rejected: rejected,
                    ..ProtocolOutcome::default()
                };
            }
            let rejected = rejected
                || (provider == "kiro"
                    && !options.resume_session_id.is_empty()
                    && is_kiro_oversized_history_image(&error));
            return protocol_failure(&provider, "session/prompt", error, session_id, rejected);
        }
    };
    let drain = if provider == "reasonix" {
        client
            .drain_notifications_with_permission(
                NOTIFICATION_QUIET,
                NOTIFICATION_DRAIN_MAX,
                |notification| handle_notification(notification, &messages, &mut state),
                |params| {
                    let decision = reasonix_permission_decision(params);
                    if let Some(question) = decision.question {
                        blocked_question = question;
                    }
                    missing_permission_metadata |= decision.metadata_missing;
                    decision.decision
                },
            )
            .await
    } else {
        client
            .drain_notifications(NOTIFICATION_QUIET, NOTIFICATION_DRAIN_MAX, |notification| {
                handle_notification(notification, &messages, &mut state)
            })
            .await
    };
    if let Err(error) = drain {
        tracing::debug!(provider = %provider, error = %error, "ACP post-response notification drain ended early");
    }
    if missing_permission_metadata {
        tracing::warn!(
            provider = "reasonix",
            "permission request omitted trusted metadata; protected-decision detection may be degraded"
        );
    }
    let stop_reason = prompt_result
        .get("stopReason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut usage = BTreeMap::new();
    let mut turn_usage = merge_usage(state.usage, parse_usage(prompt_result.get("usage")));
    if provider == "reasonix" && turn_usage == TokenUsage::default() {
        turn_usage = state.reasonix_usage;
    }
    if provider == "reasonix" && options.model.is_empty() && !state.reasonix_model.is_empty() {
        effective_model.clone_from(&state.reasonix_model);
    }
    if turn_usage != TokenUsage::default() {
        usage.insert(effective_model, turn_usage);
    }
    let (mut output, full_output) = state.deliverable.finish();
    if full_text_output {
        output.clone_from(&full_output);
    }
    let (mut status, mut error) = match stop_reason {
        "cancelled" => ("aborted", format!("{provider} cancelled the prompt")),
        "end_turn" | "" if !strict_stop_reason => ("completed", String::new()),
        "end_turn" => ("completed", String::new()),
        "error" => (
            "failed",
            format!("{provider} ended the prompt with stopReason=error"),
        ),
        unsupported => (
            "failed",
            format!("{provider} returned unsupported stopReason {unsupported:?}"),
        ),
    };
    if provider == "reasonix"
        && !blocked_question.is_empty()
        && matches!(status, "completed" | "failed")
    {
        status = "failed";
        error = blocked_question;
    }
    ProtocolOutcome {
        status: status.to_string(),
        error,
        output,
        full_output,
        session_id,
        usage,
        resume_rejected: false,
    }
}

fn session_params(
    cwd: &str,
    session_id: Option<&str>,
    mcp_servers: Vec<AcpMcpServer>,
    coding_project_meta: Option<&str>,
) -> Value {
    let mut params = serde_json::json!({"cwd":cwd,"mcpServers":mcp_servers});
    if let Some(session_id) = session_id {
        params["sessionId"] = Value::String(session_id.to_string());
    }
    if cwd != "." {
        if let Some(key) = coding_project_meta {
            let mut meta = serde_json::Map::new();
            meta.insert(key.to_string(), Value::String(cwd.to_string()));
            params["_meta"] = Value::Object(meta);
        }
    }
    params
}

fn protocol_failure(
    provider: &str,
    stage: &str,
    error: AcpError,
    session_id: String,
    rejected: bool,
) -> ProtocolOutcome {
    ProtocolOutcome {
        status: "failed".to_string(),
        error: format!("{provider} {stage} failed: {error}"),
        session_id,
        resume_rejected: rejected,
        ..ProtocolOutcome::default()
    }
}

#[derive(Default)]
struct NotificationState {
    deliverable: Deliverable,
    tools: HashMap<String, PendingTool>,
    usage: TokenUsage,
    last_finishing_status: String,
    kiro_dialect: bool,
    extended_tool_names: bool,
    reasonix_dialect: bool,
    reasonix_sequence: u64,
    reasonix_usage: TokenUsage,
    reasonix_model: String,
}

#[derive(Default)]
struct PendingTool {
    name: String,
    input: BTreeMap<String, Value>,
    emitted: bool,
    finishing: bool,
}

#[derive(Default)]
struct Deliverable {
    full: String,
    current: String,
    previous: String,
}

impl Deliverable {
    fn text(&mut self, text: &str) {
        self.full.push_str(text);
        self.current.push_str(text);
    }
    fn tool_boundary(&mut self) {
        if !self.current.trim().is_empty() {
            self.previous.clone_from(&self.current);
        }
        self.current.clear();
    }
    fn finish(self) -> (String, String) {
        let output = if self.current.trim().is_empty() {
            self.previous
        } else {
            self.current
        };
        (output, self.full)
    }
}

fn handle_notification(
    notification: AcpNotification,
    messages: &mpsc::Sender<Message>,
    state: &mut NotificationState,
) {
    if state.reasonix_dialect {
        observe_reasonix_status(&notification, state);
    }
    if !matches!(
        notification.method.as_str(),
        "session/update" | "session/notification"
    ) {
        return;
    }
    let Some(update) = notification.params.get("update") else {
        return;
    };
    let (kind, data) = normalize_update(update);
    match kind.as_str() {
        "agentmessagechunk" => {
            if let Some(text) = content_text(data) {
                state.deliverable.text(text);
                send(messages, message(MessageType::Text, text));
            }
        }
        "agentthoughtchunk" => {
            if let Some(text) = content_text(data) {
                send(messages, message(MessageType::Thinking, text));
            }
        }
        "toolcall" => handle_tool_start(data, messages, state),
        "toolcallupdate" => handle_tool_update(data, messages, state),
        "usageupdate" | "turnend" => {
            let update = parse_usage(data.get("usage").or(Some(data)));
            state.usage = merge_usage(state.usage, update);
        }
        _ => {}
    }
}

fn observe_reasonix_status(notification: &AcpNotification, state: &mut NotificationState) {
    if notification.method != "_reasonix.io/session/status_update"
        || integer(&notification.params, &["schemaVersion"]) != 1
    {
        return;
    }
    let Some(sequence) = notification.params.get("sequence").and_then(Value::as_u64) else {
        return;
    };
    if sequence < state.reasonix_sequence {
        return;
    }
    let Some(status) = notification.params.get("status") else {
        return;
    };
    let turn = status
        .get("usage")
        .and_then(|usage| usage.get("turn"))
        .unwrap_or(&Value::Null);
    let prompt = integer(turn, &["promptTokens"]);
    let cache_read = integer(turn, &["cacheHitTokens"]);
    let cache_miss = integer(turn, &["cacheMissTokens"]);
    let input = if cache_miss > 0 {
        cache_miss
    } else {
        prompt
            .checked_sub(cache_read)
            .filter(|value| *value >= 0)
            .unwrap_or(prompt)
    };
    let cost_usd_ticks = turn
        .get("estimatedCost")
        .and_then(Value::as_f64)
        .filter(|cost| cost.is_finite() && *cost >= 0.0)
        .filter(|cost| {
            *cost <= i64::MAX as f64 / COST_USD_TICKS_PER_USD as f64
                && is_reasonix_usd_currency(
                    turn.get("currency")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
        })
        .map(|cost| (cost * COST_USD_TICKS_PER_USD as f64).round() as i64)
        .unwrap_or(0);
    state.reasonix_sequence = sequence;
    state.reasonix_usage = TokenUsage {
        input_tokens: input,
        output_tokens: integer(turn, &["completionTokens"]),
        cache_read_tokens: cache_read,
        cache_write_tokens: 0,
        cost_usd_ticks,
    };
    state.reasonix_model = status
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
}

fn is_reasonix_usd_currency(currency: &str) -> bool {
    matches!(
        currency.trim().to_ascii_uppercase().as_str(),
        "USD" | "$" | "US$"
    )
}

fn normalize_update(update: &Value) -> (String, &Value) {
    if let Some(kind) = update
        .get("sessionUpdate")
        .or_else(|| update.get("type"))
        .and_then(Value::as_str)
    {
        return (normalize_kind(kind), update);
    }
    if let Some(object) = update.as_object().filter(|object| object.len() == 1) {
        if let Some((kind, data)) = object.iter().next() {
            return (normalize_kind(kind), data);
        }
    }
    (String::new(), update)
}

fn normalize_kind(kind: &str) -> String {
    kind.chars()
        .filter(|c| !matches!(c, '_' | '-'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn content_text(data: &Value) -> Option<&str> {
    data.get("content")?
        .get("text")?
        .as_str()
        .filter(|text| !text.is_empty())
}

fn handle_tool_start(
    data: &Value,
    messages: &mpsc::Sender<Message>,
    state: &mut NotificationState,
) {
    let id = data
        .get("toolCallId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if id.is_empty() {
        return;
    }
    let name = tool_name(data, state.extended_tool_names);
    let input = tool_input(data);
    let finishing = state.kiro_dialect && is_finishing_tool(&name, &input);
    state.deliverable.tool_boundary();
    let emitted = !input.is_empty();
    if emitted {
        send(messages, tool_use(&id, &name, input.clone()));
    }
    state.tools.insert(
        id,
        PendingTool {
            name,
            input,
            emitted,
            finishing,
        },
    );
}

fn handle_tool_update(
    data: &Value,
    messages: &mpsc::Sender<Message>,
    state: &mut NotificationState,
) {
    let status = data
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(status, "completed" | "failed") {
        return;
    }
    let id = data
        .get("toolCallId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if id.is_empty() {
        return;
    }
    let pending = match state.tools.remove(id) {
        Some(pending) => pending,
        None => {
            state.deliverable.tool_boundary();
            PendingTool {
                name: tool_name(data, state.extended_tool_names),
                input: tool_input(data),
                emitted: false,
                finishing: false,
            }
        }
    };
    let finishing = state.kiro_dialect
        && (pending.finishing || is_finishing_tool(&pending.name, &pending.input));
    if !pending.emitted {
        send(messages, tool_use(id, &pending.name, pending.input));
    }
    let output = ["rawOutput", "output"]
        .iter()
        .find_map(|key| data.get(*key))
        .map(render_value)
        .or_else(|| content_text(data).map(str::to_string))
        .unwrap_or_default();
    let mut result = message(MessageType::ToolResult, "");
    result.call_id = id.to_string();
    result.output = output;
    result.status = status.to_string();
    send(messages, result);
    if finishing {
        state.last_finishing_status = status.to_string();
    }
}

fn tool_name(data: &Value, extended_names: bool) -> String {
    let name = data
        .get("name")
        .or_else(|| data.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .split(':')
        .next()
        .unwrap_or("tool")
        .trim()
        .to_string();
    let lower = name.to_ascii_lowercase();
    if !extended_names {
        return match lower.as_str() {
            "shell" | "terminal" => "terminal".to_string(),
            "read" => "read_file".to_string(),
            "write" => "write_file".to_string(),
            _ => name,
        };
    }
    match lower.as_str() {
        "read" | "read file" => "read_file".to_string(),
        "write" | "write file" => "write_file".to_string(),
        "edit" | "patch" => "edit_file".to_string(),
        "shell" | "bash" | "terminal" | "run command" | "run shell command" => {
            "terminal".to_string()
        }
        "grep" | "search" | "find" => "search_files".to_string(),
        "glob" => "glob".to_string(),
        "code" => "code".to_string(),
        "web search" => "web_search".to_string(),
        "fetch" | "web fetch" => "web_fetch".to_string(),
        "todo" | "todo write" | "todo list" | "todo_list" => "todo_write".to_string(),
        _ => name.to_ascii_lowercase().replace(' ', "_"),
    }
}

fn is_finishing_tool(name: &str, input: &BTreeMap<String, Value>) -> bool {
    name == "goal_complete"
        || input
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(is_kiro_issue_comment_add_command)
}

fn is_kiro_issue_comment_add_command(command: &str) -> bool {
    let mut parts =
        trim_leading_env_assignments(command.split_whitespace().map(str::to_string).collect());
    if parts.len() >= 3 && is_posix_shell(&parts[0]) && parts[1] == "-c" {
        let inner = parts[2..].join(" ");
        let inner = inner.trim_matches(['\"', '\'']);
        parts =
            trim_leading_env_assignments(inner.split_whitespace().map(str::to_string).collect());
    }
    if parts.len() < 4 {
        return false;
    }
    let executable = parts[0].strip_prefix("./").unwrap_or(&parts[0]);
    (executable == "cordy" || executable.ends_with("/cordy"))
        && parts[1..4] == ["issue", "comment", "add"]
}

fn trim_leading_env_assignments(mut parts: Vec<String>) -> Vec<String> {
    let first_non_assignment = parts
        .iter()
        .position(|part| !is_env_assignment(part))
        .unwrap_or(parts.len());
    parts.drain(..first_non_assignment);
    parts
}

fn is_env_assignment(part: &str) -> bool {
    let Some((name, _)) = part.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name.chars().enumerate().all(|(index, character)| {
            character == '_'
                || character.is_ascii_alphabetic()
                || (index > 0 && character.is_ascii_digit())
        })
}

fn is_posix_shell(command: &str) -> bool {
    matches!(
        command.rsplit('/').next().unwrap_or(command),
        "sh" | "bash" | "zsh" | "dash"
    )
}

fn is_kiro_close_error(error: &AcpError) -> bool {
    error
        .rpc_details()
        .is_some_and(|(method, code, message, data)| {
            method == "session/prompt"
                && code == -32603
                && message.trim().eq_ignore_ascii_case("Internal error")
                && data
                    .to_ascii_lowercase()
                    .contains("failed to generate a response")
        })
}

fn is_kiro_oversized_history_image(error: &AcpError) -> bool {
    error.rpc_details().is_some_and(|(method, code, _, data)| {
        let data = data.to_ascii_lowercase();
        method == "session/prompt"
            && code == -32603
            && data.contains("image.source.base64.data")
            && data.contains("image dimensions exceed max allowed size")
    })
}

fn tool_input(data: &Value) -> BTreeMap<String, Value> {
    ["rawInput", "input", "parameters"]
        .iter()
        .find_map(|key| data.get(*key).and_then(Value::as_object))
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn render_value(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

fn parse_usage(value: Option<&Value>) -> TokenUsage {
    let value = value.unwrap_or(&Value::Null);
    TokenUsage {
        input_tokens: integer(value, &["inputTokens", "input_tokens"]),
        output_tokens: integer(value, &["outputTokens", "output_tokens"]),
        cache_read_tokens: integer(
            value,
            &["cachedReadTokens", "cacheReadTokens", "cache_read_tokens"],
        ),
        cache_write_tokens: integer(
            value,
            &[
                "cachedWriteTokens",
                "cacheWriteTokens",
                "cache_write_tokens",
            ],
        ),
        cost_usd_ticks: integer(value, &["costUsdTicks", "cost_usd_ticks"]),
    }
}

fn merge_usage(current: TokenUsage, next: TokenUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: current.input_tokens.max(next.input_tokens),
        output_tokens: current.output_tokens.max(next.output_tokens),
        cache_read_tokens: current.cache_read_tokens.max(next.cache_read_tokens),
        cache_write_tokens: current.cache_write_tokens.max(next.cache_write_tokens),
        cost_usd_ticks: current.cost_usd_ticks.max(next.cost_usd_ticks),
    }
}

fn has_token_usage(usage: &BTreeMap<String, TokenUsage>) -> bool {
    usage.values().any(|usage| {
        usage.input_tokens > 0
            || usage.output_tokens > 0
            || usage.cache_read_tokens > 0
            || usage.cache_write_tokens > 0
    })
}

fn integer(value: &Value, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_i64))
        .unwrap_or(0)
        .max(0)
}

fn extract_session_id(value: &Value) -> String {
    value
        .get("sessionId")
        .or_else(|| value.get("session_id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn extract_current_model(value: &Value) -> String {
    value
        .get("models")
        .and_then(|models| {
            models
                .get("currentModelId")
                .or_else(|| models.get("current_model_id"))
        })
        .or_else(|| value.get("currentModelId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn extract_config_value(value: &Value, config_id: &str) -> Option<String> {
    value
        .get("configOptions")
        .or_else(|| value.get("config_options"))
        .and_then(Value::as_array)
        .and_then(|options| {
            options.iter().find(|option| {
                option
                    .get("id")
                    .or_else(|| option.get("configId"))
                    .and_then(Value::as_str)
                    == Some(config_id)
            })
        })
        .and_then(|option| {
            option
                .get("currentValue")
                .or_else(|| option.get("current_value"))
        })
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            value
                .get("currentValue")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcpEffortOption {
    config_id: String,
    current: String,
    choices: Vec<String>,
}

fn parse_acp_effort_option(value: &Value) -> Option<AcpEffortOption> {
    let options = value
        .get("configOptions")
        .or_else(|| value.get("config_options"))?
        .as_array()?;
    for option in options {
        let config_id = option
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let category = option
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if config_id.is_empty()
            || !matches!(
                config_id.to_ascii_lowercase().as_str(),
                "effort" | "thought_level"
            ) && !matches!(
                category.to_ascii_lowercase().as_str(),
                "effort" | "thought_level"
            )
        {
            continue;
        }
        let mut choices = Vec::new();
        for choice in option
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let value = choice
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim();
            if !value.is_empty() && !choices.iter().any(|choice| choice == value) {
                choices.push(value.to_string());
            }
        }
        let current = option
            .get("currentValue")
            .or_else(|| option.get("current_value"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        return Some(AcpEffortOption {
            config_id: config_id.to_string(),
            current: choices
                .iter()
                .any(|choice| choice == current)
                .then(|| current.to_string())
                .unwrap_or_default(),
            choices,
        });
    }
    None
}

async fn apply_reasonix_effort<R, W>(
    client: &mut AcpClient<R, W>,
    session_result: &Value,
    session_id: &str,
    requested: &str,
    state_is_current: bool,
) where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let Some(option) =
        parse_acp_effort_option(session_result).filter(|option| !option.choices.is_empty())
    else {
        tracing::warn!(
            provider = "reasonix",
            requested_level = requested,
            "session advertises no reasoning-effort option; sending the prompt without it"
        );
        return;
    };
    if state_is_current && !option.choices.iter().any(|choice| choice == requested) {
        tracing::warn!(
            provider = "reasonix",
            config_id = option.config_id,
            requested_level = requested,
            advertised_levels = %option.choices.join(","),
            "session does not advertise the requested reasoning effort; sending the prompt without it"
        );
        return;
    }
    let result = client
        .request(
            "session/set_config_option",
            serde_json::json!({
                "sessionId":session_id,
                "configId":&option.config_id,
                "value":requested,
            }),
            |_| {},
        )
        .await;
    match result {
        Ok(result)
            if extract_config_value(&result, &option.config_id).as_deref() == Some(requested) =>
        {
            tracing::debug!(
                provider = "reasonix",
                config_id = option.config_id,
                level = requested,
                "session reasoning effort confirmed"
            );
        }
        Ok(result) => tracing::warn!(
            provider = "reasonix",
            config_id = option.config_id,
            requested_level = requested,
            effective_level = extract_config_value(&result, &option.config_id)
                .as_deref()
                .unwrap_or("unknown"),
            "runtime did not confirm the requested reasoning effort; sending the prompt anyway"
        ),
        Err(error) => tracing::warn!(
            provider = "reasonix",
            config_id = option.config_id,
            requested_level = requested,
            effective_level = if option.current.is_empty() { "unknown" } else { &option.current },
            error = %error,
            "runtime rejected the reasoning effort request; sending the prompt anyway"
        ),
    }
}

fn reasonix_capability_versions(value: &Value) -> (i64, i64) {
    let meta = value
        .get("agentCapabilities")
        .and_then(|capabilities| capabilities.get("_meta"));
    let version = |key: &str| {
        meta.and_then(|meta| meta.get(key))
            .and_then(|capability| capability.get("schemaVersion"))
            .and_then(Value::as_i64)
            .unwrap_or(0)
    };
    (
        version("_reasonix.io/session/status"),
        version("_reasonix.io/session/status_update"),
    )
}

#[derive(Debug, PartialEq, Eq)]
struct ReasonixPermissionDecision {
    decision: AcpPermissionDecision,
    question: Option<String>,
    metadata_missing: bool,
}

fn reasonix_permission_decision(params: Option<&Value>) -> ReasonixPermissionDecision {
    let tool_call = params.and_then(|params| params.get("toolCall"));
    let options = params
        .and_then(|params| params.get("options"))
        .and_then(Value::as_array);
    let reject_once = options.into_iter().flatten().find_map(|option| {
        option
            .get("kind")
            .and_then(Value::as_str)
            .filter(|kind| kind.trim().eq_ignore_ascii_case("reject_once"))
            .and_then(|_| option.get("optionId").and_then(Value::as_str))
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    });
    let is_question = tool_call
        .and_then(|tool| tool.get("toolCallId"))
        .and_then(Value::as_str)
        .is_some_and(|id| id.starts_with("ask-"))
        || options.into_iter().flatten().any(|option| {
            option
                .get("optionId")
                .and_then(Value::as_str)
                .is_some_and(|id| id.ends_with(":cancel"))
        });
    if is_question {
        let mut reason = "Reasonix requested interactive user input, which is unavailable in an unattended Cordy task".to_string();
        if let Some(title) = tool_call
            .and_then(|tool| tool.get("title"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            reason.push_str(": ");
            reason.push_str(&clip_reasonix_permission_title(title));
        }
        return ReasonixPermissionDecision {
            decision: reject_once.map_or_else(
                || AcpPermissionDecision::Reject(reason.clone()),
                AcpPermissionDecision::Select,
            ),
            question: Some(reason),
            metadata_missing: false,
        };
    }
    let metadata = tool_call
        .and_then(|tool| tool.get("_meta"))
        .and_then(|meta| meta.get("reasonix.io"));
    let tool = metadata
        .and_then(|meta| meta.get("tool"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let fresh = metadata
        .and_then(|meta| meta.get("fresh"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let protected = matches!(
        tool,
        "exit_plan_mode"
            | "remember"
            | "forget"
            | "sandbox_escape"
            | "config_write"
            | "plan_mode_read_only_command"
    );
    let decision = if fresh || protected {
        reject_once.map_or_else(
            || {
                AcpPermissionDecision::Reject(
                    "Reasonix requested a protected decision that requires fresh human approval"
                        .to_string(),
                )
            },
            AcpPermissionDecision::Select,
        )
    } else {
        default_permission_decision(params)
    };
    ReasonixPermissionDecision {
        decision,
        question: None,
        metadata_missing: tool.is_empty(),
    }
}

fn clip_reasonix_permission_title(title: &str) -> String {
    let mut chars = title.chars();
    let clipped: String = chars.by_ref().take(240).collect();
    if chars.next().is_some() {
        format!("{clipped}…")
    } else {
        clipped
    }
}

fn is_reasonix_lease_conflict(error: &AcpError) -> bool {
    error.rpc_details().is_some_and(|(_, code, message, data)| {
        let text = format!("{message} {data}").to_ascii_lowercase();
        code == -32600 && text.contains(" in use") && text.contains("close the other reasonix")
    })
}

fn reasonix_session_new_error(error: &AcpError) -> String {
    let text = error.to_string().to_ascii_lowercase();
    let suffix = if text.contains("not configured") || text.contains("no default_model configured")
    {
        "; run `reasonix setup` in the runtime owner's environment and retry"
    } else {
        ""
    };
    format!("reasonix session/new failed: {error}{suffix}")
}

fn provider_error(provider: &str, stderr: &str, output: &str) -> Option<String> {
    if let Some(found) = TERMINAL_PROVIDER_ERROR.find(stderr) {
        return Some(format!("{provider} provider error: {}", found.as_str()));
    }
    OUTPUT_PROVIDER_ERROR
        .find(output)
        .map(|found| format!("{provider} provider error: {}", found.as_str()))
}

fn message(message_type: MessageType, content: &str) -> Message {
    Message {
        message_type,
        content: content.to_string(),
        tool: String::new(),
        call_id: String::new(),
        input: BTreeMap::new(),
        output: String::new(),
        status: String::new(),
        level: String::new(),
        session_id: String::new(),
    }
}

fn tool_use(id: &str, name: &str, input: BTreeMap<String, Value>) -> Message {
    let mut value = message(MessageType::ToolUse, "");
    value.call_id = id.to_string();
    value.tool = name.to_string();
    value.input = input;
    value
}

fn send(messages: &mpsc::Sender<Message>, value: Message) {
    let _ = messages.try_send(value);
}

fn log_blocked_args(
    provider: &str,
    blocked: &BTreeMap<&str, BlockedArgMode>,
    options: &ExecOptions,
) {
    log_blocked(
        provider,
        "extra arguments",
        &filter_custom_args(&options.extra_args, blocked).blocked_flags,
    );
    log_blocked(
        provider,
        "custom arguments",
        &filter_custom_args(&options.custom_args, blocked).blocked_flags,
    );
}

fn log_blocked(provider: &str, source: &str, flags: &[String]) {
    if !flags.is_empty() {
        tracing::warn!(provider, source, flags = ?flags, "ignored daemon-owned arguments");
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn arguments_keep_protocol_and_permission_mode_owned() {
        let args = build_qoder_args(&ExecOptions {
            extra_args: vec!["--acp".into(), "--safe".into()],
            custom_args: vec!["acp".into(), "--yolo".into(), "--debug".into()],
            ..ExecOptions::default()
        });
        assert_eq!(args.iter().filter(|arg| arg.as_str() == "--acp").count(), 1);
        assert_eq!(
            args.iter().filter(|arg| arg.as_str() == "--yolo").count(),
            1
        );
        assert!(args.iter().any(|arg| arg == "--safe"));
        assert!(args.iter().any(|arg| arg == "--debug"));
        assert!(!args.iter().any(|arg| arg == "acp"));
    }

    #[test]
    fn traecli_arguments_keep_acp_serve_and_headless_mode_owned() {
        let args = build_traecli_args(&ExecOptions {
            custom_args: [
                "acp",
                "serve",
                "--yolo",
                "--output-format",
                "json",
                "--add-dir",
                "/extra",
            ]
            .map(str::to_string)
            .to_vec(),
            ..ExecOptions::default()
        });
        assert_eq!(&args[..3], ["acp", "serve", "--yolo"]);
        assert_eq!(args.iter().filter(|arg| arg.as_str() == "acp").count(), 1);
        assert_eq!(args.iter().filter(|arg| arg.as_str() == "serve").count(), 1);
        assert!(!args.iter().any(|arg| arg == "json"));
        assert!(args.windows(2).any(|pair| pair == ["--add-dir", "/extra"]));
    }

    #[test]
    fn kiro_arguments_keep_protocol_and_trust_mode_owned() {
        let args = build_kiro_args(&ExecOptions {
            custom_args: [
                "acp",
                "-a",
                "--trust-all-tools",
                "--trust-tools",
                "terminal",
                "--agent",
                "cordy",
            ]
            .map(str::to_string)
            .to_vec(),
            ..ExecOptions::default()
        });
        assert_eq!(&args[..2], ["acp", "--trust-all-tools"]);
        assert_eq!(args.iter().filter(|arg| arg.as_str() == "acp").count(), 1);
        assert_eq!(
            args.iter()
                .filter(|arg| arg.as_str() == "--trust-all-tools")
                .count(),
            1
        );
        assert!(!args.iter().any(|arg| arg == "terminal"));
        assert!(args.windows(2).any(|pair| pair == ["--agent", "cordy"]));
    }

    #[test]
    fn qwenpaw_arguments_keep_workspace_owned() {
        let args = build_qwenpaw_args(&ExecOptions {
            extra_args: ["acp", "--verbose"].map(str::to_string).to_vec(),
            custom_args: ["--workspace", "/wrong", "--debug"]
                .map(str::to_string)
                .to_vec(),
            qwenpaw_workspace: "/owned".to_string(),
            ..ExecOptions::default()
        });
        assert_eq!(args[0], "acp");
        assert_eq!(args.iter().filter(|arg| arg.as_str() == "acp").count(), 1);
        assert!(!args.iter().any(|arg| arg == "/wrong"));
        assert_eq!(&args[args.len() - 2..], ["--workspace", "/owned"]);
        assert!(args.iter().any(|arg| arg == "--verbose"));
        assert!(args.iter().any(|arg| arg == "--debug"));
    }

    #[test]
    fn kimi_arguments_keep_acp_subcommand_owned() {
        let args = build_kimi_args(&ExecOptions {
            extra_args: ["acp", "--verbose"].map(str::to_string).to_vec(),
            custom_args: ["acp", "--debug"].map(str::to_string).to_vec(),
            ..ExecOptions::default()
        });
        assert_eq!(args.iter().filter(|arg| arg.as_str() == "acp").count(), 1);
        assert_eq!(args[0], "acp");
        assert!(args.iter().any(|arg| arg == "--verbose"));
        assert!(args.iter().any(|arg| arg == "--debug"));
    }

    #[test]
    fn reasonix_arguments_keep_sandbox_and_profile_owned() {
        let args = build_reasonix_args(&ExecOptions {
            custom_args: [
                "acp",
                "--profile",
                "unsafe",
                "--sandbox-network",
                "off",
                "--workspace-only",
                "--debug",
            ]
            .map(str::to_string)
            .to_vec(),
            ..ExecOptions::default()
        });
        assert_eq!(
            &args[..10],
            [
                "acp",
                "--profile",
                "balanced",
                "--planner",
                "auto",
                "--sandbox-network",
                "auto",
                "--sandbox-bash",
                "auto",
                "--workspace-only",
            ]
        );
        assert_eq!(args.iter().filter(|arg| arg.as_str() == "acp").count(), 1);
        assert!(!args.iter().any(|arg| arg == "unsafe" || arg == "off"));
        assert!(args.iter().any(|arg| arg == "--debug"));
    }

    #[test]
    fn reasonix_permission_policy_rejects_questions_and_fresh_decisions() {
        let question = serde_json::json!({
            "toolCall":{"toolCallId":"ask-1","title":"Choose a deployment"},
            "options":[{"optionId":"ask-1:cancel","kind":"reject_once"}]
        });
        let decision = reasonix_permission_decision(Some(&question));
        assert_eq!(
            decision.decision,
            AcpPermissionDecision::Select("ask-1:cancel".to_string())
        );
        assert!(decision
            .question
            .as_deref()
            .is_some_and(|question| question.contains("Choose a deployment")));
        assert!(!decision.metadata_missing);

        let fresh = serde_json::json!({
            "toolCall":{"toolCallId":"tool-1","_meta":{"reasonix.io":{"tool":"terminal","fresh":true}}},
            "options":[
                {"optionId":"allow","kind":"allow_once"},
                {"optionId":"reject","kind":"reject_once"}
            ]
        });
        assert_eq!(
            reasonix_permission_decision(Some(&fresh)).decision,
            AcpPermissionDecision::Select("reject".to_string())
        );
    }

    #[test]
    fn reasonix_permission_policy_allows_ordinary_tools_and_flags_missing_metadata() {
        let ordinary = serde_json::json!({
            "toolCall":{"toolCallId":"tool-1","_meta":{"reasonix.io":{"tool":"read_file"}}},
            "options":[{"optionId":"allow","kind":"allow_once"}]
        });
        let decision = reasonix_permission_decision(Some(&ordinary));
        assert_eq!(
            decision.decision,
            AcpPermissionDecision::Select("allow".to_string())
        );
        assert!(!decision.metadata_missing);

        let missing = serde_json::json!({
            "toolCall":{"toolCallId":"tool-2"},
            "options":[{"optionId":"allow","kind":"allow_once"}]
        });
        assert!(reasonix_permission_decision(Some(&missing)).metadata_missing);
    }

    #[test]
    fn reasonix_effort_parser_preserves_runtime_vocabulary() {
        let option = parse_acp_effort_option(&serde_json::json!({
            "config_options":[{
                "id":"effort",
                "category":"thought_level",
                "current_value":"high",
                "options":[
                    {"value":"auto","name":"Auto"},
                    {"value":"high","name":"High"},
                    {"value":"high","name":"duplicate"},
                    {"value":"","name":"empty"}
                ]
            }]
        }))
        .unwrap_or_else(|| panic!("Reasonix effort option"));
        assert_eq!(option.config_id, "effort");
        assert_eq!(option.current, "high");
        assert_eq!(option.choices, ["auto", "high"]);
    }

    #[test]
    fn reasonix_status_uses_newest_sequence_and_authoritative_cost() {
        let (messages, _receiver) = mpsc::channel(1);
        let mut state = NotificationState {
            reasonix_dialect: true,
            ..NotificationState::default()
        };
        handle_notification(
            AcpNotification {
                method: "_reasonix.io/session/status_update".to_string(),
                params: serde_json::json!({
                    "schemaVersion":1,
                    "sequence":2,
                    "status":{"model":"deepseek-v4","usage":{"turn":{
                        "promptTokens":120,
                        "completionTokens":30,
                        "cacheHitTokens":100,
                        "cacheMissTokens":20,
                        "estimatedCost":0.125,
                        "currency":"USD"
                    }}}
                }),
            },
            &messages,
            &mut state,
        );
        handle_notification(
            AcpNotification {
                method: "_reasonix.io/session/status_update".to_string(),
                params: serde_json::json!({
                    "schemaVersion":1,
                    "sequence":1,
                    "status":{"model":"stale","usage":{"turn":{"promptTokens":1}}}
                }),
            },
            &messages,
            &mut state,
        );
        assert_eq!(state.reasonix_model, "deepseek-v4");
        assert_eq!(state.reasonix_usage.input_tokens, 20);
        assert_eq!(state.reasonix_usage.output_tokens, 30);
        assert_eq!(state.reasonix_usage.cache_read_tokens, 100);
        assert_eq!(state.reasonix_usage.cost_usd_ticks, 1_250_000_000);
    }

    #[test]
    fn reasonix_capability_versions_require_both_schema_one_extensions() {
        assert_eq!(
            reasonix_capability_versions(&serde_json::json!({
                "agentCapabilities":{"_meta":{
                    "_reasonix.io/session/status":{"schemaVersion":1},
                    "_reasonix.io/session/status_update":{"schemaVersion":1}
                }}
            })),
            (1, 1)
        );
        assert_eq!(reasonix_capability_versions(&Value::Null), (0, 0));
    }

    #[test]
    fn kimi_thinking_catalog_rejects_unsafe_and_duplicate_values() {
        let parsed = parse_kimi_thinking(
            br#"{"models":{"kimi-code/k3":{"supportEfforts":["low","low","max","../bad",""],"defaultEffort":"../bad"}}}"#,
        )
        .unwrap_or_else(|| panic!("parse Kimi thinking catalog"));
        let thinking = &parsed["kimi-code/k3"];
        assert_eq!(thinking.supported_levels.len(), 2);
        assert_eq!(thinking.supported_levels[0].value, "low");
        assert_eq!(thinking.supported_levels[1].value, "max");
        assert!(thinking.default_level.is_empty());
    }

    #[test]
    fn kiro_finishing_command_is_payload_driven_and_strict() {
        assert!(is_kiro_issue_comment_add_command(
            "CORDY_TOKEN=x sh -c 'cordy issue comment add TASK-1 --body done'"
        ));
        assert!(is_kiro_issue_comment_add_command(
            "/usr/local/bin/cordy issue comment add TASK-1"
        ));
        assert!(!is_kiro_issue_comment_add_command(
            "cordy issue comment list TASK-1"
        ));
        assert!(!is_kiro_issue_comment_add_command(
            "printf cordy issue comment add"
        ));
    }

    #[test]
    fn kiro_error_classifiers_require_exact_rpc_shape() {
        let close = AcpError::Rpc {
            method: "session/prompt".to_string(),
            code: -32603,
            message: "Internal error".to_string(),
            data: ", data=Kiro failed to generate a response".to_string(),
        };
        assert!(is_kiro_close_error(&close));
        let oversized = AcpError::Rpc {
            method: "session/prompt".to_string(),
            code: -32603,
            message: "Internal error".to_string(),
            data: ", data=messages.2.content.0.image.source.base64.data: image dimensions exceed max allowed size".to_string(),
        };
        assert!(is_kiro_oversized_history_image(&oversized));
        assert!(!is_kiro_close_error(&oversized));
    }

    #[test]
    fn kiro_most_recent_finishing_result_controls_close_guard() {
        let (messages, _receiver) = mpsc::channel(8);
        let mut state = NotificationState {
            kiro_dialect: true,
            extended_tool_names: true,
            ..NotificationState::default()
        };
        for (id, status) in [("first", "completed"), ("final", "failed")] {
            handle_tool_start(
                &serde_json::json!({
                    "toolCallId": id,
                    "name": "anything",
                    "rawInput": {"command":"cordy issue comment add CORD-1"}
                }),
                &messages,
                &mut state,
            );
            handle_tool_update(
                &serde_json::json!({"toolCallId":id,"status":status}),
                &messages,
                &mut state,
            );
        }
        assert_eq!(state.last_finishing_status, "failed");
    }

    #[test]
    fn deliverable_uses_text_after_latest_tool_boundary() {
        let mut deliverable = Deliverable::default();
        deliverable.text("I will inspect.");
        deliverable.tool_boundary();
        deliverable.text("Final answer.");
        let (output, full) = deliverable.finish();
        assert_eq!(output, "Final answer.");
        assert_eq!(full, "I will inspect.Final answer.");
    }

    #[cfg(unix)]
    fn fake_backend(script: &str) -> (tempfile::TempDir, QoderBackend) {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let executable = directory.path().join("qodercli");
        std::fs::write(&executable, script)
            .unwrap_or_else(|error| panic!("write fake Qoder: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake Qoder: {error}"));
        let backend = QoderBackend::new(QoderConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            ..QoderConfig::default()
        });
        (directory, backend)
    }

    #[cfg(unix)]
    fn fake_kiro_backend(script: &str) -> (tempfile::TempDir, std::path::PathBuf, KiroBackend) {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let executable = directory.path().join("kiro-cli");
        let requests = directory.path().join("requests.jsonl");
        std::fs::write(&executable, script)
            .unwrap_or_else(|error| panic!("write fake Kiro: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake Kiro: {error}"));
        let backend = KiroBackend::new(KiroConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            env: BTreeMap::from([(
                "KIRO_REQUESTS".to_string(),
                requests.to_string_lossy().into_owned(),
            )]),
        });
        (directory, requests, backend)
    }

    #[cfg(unix)]
    fn fake_qwenpaw_backend(
        script: &str,
    ) -> (tempfile::TempDir, std::path::PathBuf, QwenpawBackend) {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let executable = directory.path().join("qwenpaw");
        let requests = directory.path().join("requests.jsonl");
        std::fs::write(&executable, script)
            .unwrap_or_else(|error| panic!("write fake QwenPaw: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake QwenPaw: {error}"));
        let backend = QwenpawBackend::new(QwenpawConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            env: BTreeMap::from([(
                "QWENPAW_REQUESTS".to_string(),
                requests.to_string_lossy().into_owned(),
            )]),
        });
        (directory, requests, backend)
    }

    #[cfg(unix)]
    fn fake_kimi_backend(script: &str) -> (tempfile::TempDir, std::path::PathBuf, KimiBackend) {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let executable = directory.path().join("kimi");
        let requests = directory.path().join("requests.jsonl");
        let kimi_home = directory.path().join("kimi-home");
        std::fs::write(&executable, script)
            .unwrap_or_else(|error| panic!("write fake Kimi: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake Kimi: {error}"));
        let backend = KimiBackend::new(KimiConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            env: BTreeMap::from([
                (
                    "KIMI_REQUESTS".to_string(),
                    requests.to_string_lossy().into_owned(),
                ),
                (
                    "KIMI_CODE_HOME".to_string(),
                    kimi_home.to_string_lossy().into_owned(),
                ),
            ]),
        });
        (directory, requests, backend)
    }

    #[cfg(unix)]
    fn fake_reasonix_backend(script: &str) -> (tempfile::TempDir, ReasonixBackend) {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let executable = directory.path().join("reasonix");
        std::fs::write(&executable, script)
            .unwrap_or_else(|error| panic!("write fake Reasonix: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake Reasonix: {error}"));
        let backend = ReasonixBackend::new(ReasonixConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            env: BTreeMap::new(),
        });
        (directory, backend)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kiro_preserves_only_proven_finishing_completion_and_sends_both_payloads() {
        let (_directory, requests, backend) = fake_kiro_backend(
            r#"#!/bin/sh
test "$1" = acp && test "$2" = --trust-all-tools || exit 20
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$KIRO_REQUESTS"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"kiro-1"}}\n' "$id" ;;
    *'"method":"session/prompt"'*)
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call","toolCallId":"finish-1","title":"Running delivery","rawInput":{"command":"cordy issue comment add CORD-1 --body done"}}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call_update","toolCallId":"finish-1","status":"completed","rawOutput":"ok"}}}'
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32603,"message":"Internal error","data":"Kiro failed to generate a response"}}\n' "$id" ;;
  esac
done
"#,
        );
        let session = backend
            .execute("ship it", ExecOptions::default())
            .await
            .unwrap_or_else(|error| panic!("execute Kiro: {error}"));
        let result = session
            .result
            .await
            .unwrap_or_else(|error| panic!("Kiro result: {error}"));
        assert_eq!(result.status, "completed");
        assert!(result.error.is_empty());
        assert_eq!(result.session_id, "kiro-1");
        let requests = std::fs::read_to_string(requests)
            .unwrap_or_else(|error| panic!("read Kiro requests: {error}"));
        let prompt: Value = serde_json::from_str(
            requests
                .lines()
                .find(|line| line.contains("\"method\":\"session/prompt\""))
                .unwrap_or_else(|| panic!("Kiro prompt request missing")),
        )
        .unwrap_or_else(|error| panic!("parse Kiro prompt: {error}"));
        assert_eq!(prompt["params"]["content"], prompt["params"]["prompt"]);
        assert_eq!(prompt["params"]["prompt"][0]["text"], "ship it");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kiro_model_discovery_uses_non_executing_acp_mode() {
        let (_directory, requests, backend) = fake_kiro_backend(
            r#"#!/bin/sh
printf '%s\n' "$@" > "$KIRO_REQUESTS.args"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"discovery","models":{"currentModelId":"claude-sonnet-4.5","availableModels":[{"modelId":"claude-sonnet-4.5","name":"Claude Sonnet 4.5"}]}}}\n' "$id" ;;
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
        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.models[0].id, "claude-sonnet-4.5");
        let args = std::fs::read_to_string(format!("{}.args", requests.display()))
            .unwrap_or_else(|error| panic!("read Kiro discovery args: {error}"));
        assert_eq!(args, "acp\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kiro_oversized_resumed_history_keeps_session_for_audit() {
        let (_directory, _requests, backend) = fake_kiro_backend(
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"method":"session/load"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"method":"session/prompt"'*) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32603,"message":"Internal error","data":"messages.14.content.0.image.source.base64.data: image dimensions exceed max allowed size"}}\n' "$id" ;;
  esac
done
"#,
        );
        let session = backend
            .execute(
                "continue",
                ExecOptions {
                    resume_session_id: "poisoned-session".to_string(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute Kiro: {error}"));
        let result = session
            .result
            .await
            .unwrap_or_else(|error| panic!("Kiro result: {error}"));
        assert_eq!(result.status, "failed");
        assert!(result.resume_rejected);
        assert_eq!(result.session_id, "poisoned-session");
        assert!(result
            .error
            .contains("image dimensions exceed max allowed size"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn qwenpaw_executes_with_project_meta_without_mutating_model() {
        let (directory, requests, backend) = fake_qwenpaw_backend(
            r#"#!/bin/sh
printf '%s\n' "$@" > "$QWENPAW_REQUESTS.args"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$QWENPAW_REQUESTS"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"paw-1","models":{"currentModelId":"shared-model"}}}\n' "$id" ;;
    *'"method":"session/prompt"'*)
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk","content":{"text":"before tool"}}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"ToolCall","toolCallId":"t1","name":"read","rawInput":{"path":"README.md"}}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"ToolCallUpdate","toolCallId":"t1","status":"completed","output":"ok"}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk","content":{"text":" after tool"}}}}'
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn","usage":{"inputTokens":9,"outputTokens":3}}}\n' "$id" ;;
  esac
done
"#,
        );
        let cwd = directory.path().to_string_lossy().into_owned();
        let session = backend
            .execute(
                "prompt",
                ExecOptions {
                    cwd: cwd.clone(),
                    model: "must-not-be-written".to_string(),
                    qwenpaw_workspace: "/managed/workspace".to_string(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute QwenPaw: {error}"));
        let result = session
            .result
            .await
            .unwrap_or_else(|error| panic!("QwenPaw result: {error}"));
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "before tool after tool");
        assert_eq!(result.usage["unknown"].input_tokens, 9);
        let requests = std::fs::read_to_string(&requests)
            .unwrap_or_else(|error| panic!("read QwenPaw requests: {error}"));
        assert!(!requests.contains("session/set_model"));
        let session_new: Value = serde_json::from_str(
            requests
                .lines()
                .find(|line| line.contains("\"method\":\"session/new\""))
                .unwrap_or_else(|| panic!("QwenPaw session/new missing")),
        )
        .unwrap_or_else(|error| panic!("parse QwenPaw session/new: {error}"));
        assert_eq!(
            session_new["params"]["_meta"]["qwenpaw.coding_project_dir"],
            cwd
        );
        let args = std::fs::read_to_string(format!("{}.args", requests_path(&directory)))
            .unwrap_or_else(|error| panic!("read QwenPaw args: {error}"));
        assert_eq!(args, "acp\n--workspace\n/managed/workspace\n");
    }

    #[cfg(unix)]
    fn requests_path(directory: &tempfile::TempDir) -> String {
        directory
            .path()
            .join("requests.jsonl")
            .to_string_lossy()
            .into_owned()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn qwenpaw_missing_loaded_session_requests_fresh_retry() {
        let (_directory, _requests, backend) = fake_qwenpaw_backend(
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"method":"session/load"'*) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32603,"message":"No session found with id stale"}}\n' "$id" ;;
  esac
done
"#,
        );
        let session = backend
            .execute(
                "continue",
                ExecOptions {
                    resume_session_id: "stale".to_string(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute QwenPaw: {error}"));
        let result = session
            .result
            .await
            .unwrap_or_else(|error| panic!("QwenPaw result: {error}"));
        assert_eq!(result.status, "failed");
        assert!(result.resume_rejected);
        assert!(result.session_id.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kimi_applies_model_and_thinking_then_falls_back_to_wire_usage() {
        let (_directory, requests, backend) = fake_kimi_backend(
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$KIMI_REQUESTS"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"kimi-session"}}\n' "$id" ;;
    *'"method":"session/set_model"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"method":"session/set_config_option"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"configOptions":[{"id":"thinking","currentValue":"max"}]}}\n' "$id" ;;
    *'"method":"session/prompt"'*)
      mkdir -p "$KIMI_CODE_HOME/sessions/workspace/kimi-session/agents/main"
      printf '%s\n' '{"type":"usage.record","time":0,"usage":{"inputOther":12,"output":4,"inputCacheRead":20,"inputCacheCreation":3}}' > "$KIMI_CODE_HOME/sessions/workspace/kimi-session/agents/main/wire.jsonl"
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk","content":{"text":"Kimi answer"}}}}'
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id" ;;
  esac
done
"#,
        );
        let session = backend
            .execute(
                "prompt",
                ExecOptions {
                    model: "kimi-k3".to_string(),
                    thinking_level: "max".to_string(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute Kimi: {error}"));
        let result = session
            .result
            .await
            .unwrap_or_else(|error| panic!("Kimi result: {error}"));
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "Kimi answer");
        assert_eq!(result.usage["kimi-k3"].input_tokens, 12);
        assert_eq!(result.usage["kimi-k3"].output_tokens, 4);
        assert_eq!(result.usage["kimi-k3"].cache_read_tokens, 20);
        assert_eq!(result.usage["kimi-k3"].cache_write_tokens, 3);
        let requests = std::fs::read_to_string(requests)
            .unwrap_or_else(|error| panic!("read Kimi requests: {error}"));
        let set_model = requests
            .find("\"method\":\"session/set_model\"")
            .unwrap_or_else(|| panic!("Kimi set_model missing"));
        let set_thinking = requests
            .find("\"method\":\"session/set_config_option\"")
            .unwrap_or_else(|| panic!("Kimi thinking config missing"));
        let prompt = requests
            .find("\"method\":\"session/prompt\"")
            .unwrap_or_else(|| panic!("Kimi prompt missing"));
        assert!(set_model < set_thinking && set_thinking < prompt);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn kimi_discovery_merges_version_gated_per_model_effort() {
        let (_directory, _requests, backend) = fake_kimi_backend(
            r#"#!/bin/sh
if test "$1" = provider && test "$2" = list && test "$3" = --json; then
  printf '%s\n' '{"models":{"kimi-code/k3":{"supportEfforts":["low","high","max"],"defaultEffort":"high"},"kimi-code/plain":{"supportEfforts":[]}}}'
  exit 0
fi
test "$1" = acp || exit 20
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"agentInfo":{"name":"Kimi Code CLI","version":"0.33.0"}}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"discovery","configOptions":[{"id":"model","category":"model","currentValue":"kimi-code/k3","options":[{"value":"kimi-code/k3","name":"K3"},{"value":"kimi-code/plain","name":"Plain"}]}]}}\n' "$id" ;;
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
        assert_eq!(catalog.models.len(), 2);
        assert!(catalog.models[0].default);
        let thinking = catalog.models[0]
            .thinking
            .as_ref()
            .unwrap_or_else(|| panic!("Kimi K3 thinking catalog missing"));
        assert_eq!(thinking.default_level, "high");
        assert_eq!(
            thinking
                .supported_levels
                .iter()
                .map(|level| level.value.as_str())
                .collect::<Vec<_>>(),
            ["low", "high", "max"]
        );
        assert!(catalog.models[1].thinking.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn real_acp_process_streams_tools_usage_and_permission() {
        let (_directory, backend) = fake_backend(
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"mcpCapabilities":{"http":true}}}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"qoder-real","models":{"currentModelId":"qoder-auto","availableModels":[{"modelId":"qoder-auto","name":"Qoder Auto"}]}}}\n' "$id" ;;
    *'"method":"session/prompt"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":99,"method":"session/request_permission","params":{"options":[{"optionId":"forever","kind":"allow_always"},{"optionId":"once","kind":"allow_once"}]}}'
      IFS= read -r permission
      case "$permission" in *'"optionId":"once"'*) ;; *) exit 12 ;; esac
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk","content":{"text":"narration"}}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"ToolCall","toolCallId":"tool-1","name":"read_file","rawInput":{"path":"README.md"}}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"ToolCallUpdate","toolCallId":"tool-1","name":"read_file","status":"completed","output":"contents"}}}'
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk","content":{"text":"final answer"}}}}'
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn","usage":{"inputTokens":11,"outputTokens":4}}}\n' "$id"
      sleep 0.05
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk","content":{"text":" tail"}}}}'
      ;;
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
        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.models[0].id, "qoder-auto");
        assert!(catalog.models[0].default);
        let session = backend
            .execute("prompt", ExecOptions::default())
            .await
            .unwrap_or_else(|error| panic!("execute Qoder: {error}"));
        let Session {
            mut messages,
            result,
        } = session;
        let mut types = Vec::new();
        while let Some(message) = messages.recv().await {
            types.push(message.message_type);
        }
        let result = result
            .await
            .unwrap_or_else(|error| panic!("Qoder result: {error}"));
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "final answer tail");
        assert_eq!(result.session_id, "qoder-real");
        assert_eq!(result.usage["qoder-auto"].input_tokens, 11);
        assert!(types.contains(&MessageType::ToolUse));
        assert!(types.contains(&MessageType::ToolResult));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reasonix_executes_effort_and_blocks_interactive_questions() {
        let (_directory, backend) = fake_reasonix_backend(
            r#"#!/bin/sh
test "$1 $2 $3 $4 $5 $6 $7 $8 $9 ${10}" = "acp --profile balanced --planner auto --sandbox-network auto --sandbox-bash auto --workspace-only" || exit 20
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"_meta":{"_reasonix.io/session/status":{"schemaVersion":1},"_reasonix.io/session/status_update":{"schemaVersion":1}}}}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"reasonix-1","models":{"currentModelId":"deepseek-v4"},"configOptions":[{"id":"effort","category":"thought_level","currentValue":"auto","options":[{"value":"auto"},{"value":"high"}]}]}}\n' "$id" ;;
    *'"method":"session/set_config_option"'*)
      case "$line" in *'"configId":"effort"'*'"value":"high"'*) ;; *) exit 21 ;; esac
      printf '{"jsonrpc":"2.0","id":%s,"result":{"configOptions":[{"id":"effort","currentValue":"high"}]}}\n' "$id"
      ;;
    *'"method":"session/prompt"'*)
      printf '%s\n' '{"jsonrpc":"2.0","id":99,"method":"session/request_permission","params":{"toolCall":{"toolCallId":"ask-1","title":"Choose deployment"},"options":[{"optionId":"ask-1:cancel","kind":"reject_once"}]}}'
      IFS= read -r permission
      case "$permission" in *'"optionId":"ask-1:cancel"'*) ;; *) exit 22 ;; esac
      printf '%s\n' '{"jsonrpc":"2.0","method":"_reasonix.io/session/status_update","params":{"schemaVersion":1,"sequence":1,"status":{"model":"deepseek-v4","usage":{"turn":{"promptTokens":50,"completionTokens":4,"cacheHitTokens":40,"cacheMissTokens":10,"estimatedCost":0.01,"currency":"USD"}}}}}'
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"error"}}\n' "$id"
      ;;
  esac
done
"#,
        );
        let session = backend
            .execute(
                "prompt",
                ExecOptions {
                    thinking_level: "high".to_string(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute Reasonix: {error}"));
        let result = session
            .result
            .await
            .unwrap_or_else(|error| panic!("Reasonix result: {error}"));
        assert_eq!(result.status, "failed");
        assert!(result.error.contains("Choose deployment"));
        assert_eq!(result.session_id, "reasonix-1");
        assert_eq!(result.usage["deepseek-v4"].input_tokens, 10);
        assert_eq!(result.usage["deepseek-v4"].cost_usd_ticks, 100_000_000);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_terminates_owned_process_tree() {
        let (_directory, backend) = fake_backend("#!/bin/sh\nsleep 60 &\nwait\n");
        let cancellation = tokio_util::sync::CancellationToken::new();
        let session = backend
            .execute(
                "prompt",
                ExecOptions {
                    cancellation: cancellation.clone(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute Qoder: {error}"));
        cancellation.cancel();
        let result = tokio::time::timeout(Duration::from_secs(15), session.result)
            .await
            .unwrap_or_else(|error| panic!("cancellation exceeded bound: {error}"))
            .unwrap_or_else(|error| panic!("Qoder result: {error}"));
        assert_eq!(result.status, "aborted");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn traecli_resume_uses_session_load_and_ignores_history_replay() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let executable = directory.path().join("traecli");
        let requests = directory.path().join("requests.jsonl");
        std::fs::write(
            &executable,
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$TRAE_REQUESTS"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"agentCapabilities":{"loadSession":true}}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"discovery","models":{"currentModelId":"Doubao-Seed-2.1-Pro","availableModels":[{"modelId":"Doubao-Seed-2.1-Pro","name":"Doubao Seed Pro"}]}}}\n' "$id" ;;
    *'"method":"session/load"'*)
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk","content":{"text":"old history"}}}}'
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id" ;;
    *'"method":"session/prompt"'*)
      printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"update":{"type":"AgentMessageChunk","content":{"text":"current answer"}}}}'
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id" ;;
  esac
done
"#,
        )
        .unwrap_or_else(|error| panic!("write fake TRAE: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod fake TRAE: {error}"));
        let backend = TraecliBackend::new(TraecliConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            env: BTreeMap::from([(
                "TRAE_REQUESTS".to_string(),
                requests.to_string_lossy().into_owned(),
            )]),
        });
        let catalog = backend
            .discover_models(
                &CatalogCache::default(),
                CancellationToken::new(),
                Duration::from_secs(5),
            )
            .await;
        assert_eq!(catalog.models[0].id, "Doubao-Seed-2.1-Pro");
        let session = backend
            .execute(
                "prompt",
                ExecOptions {
                    resume_session_id: "existing-session".to_string(),
                    ..ExecOptions::default()
                },
            )
            .await
            .unwrap_or_else(|error| panic!("execute TRAE: {error}"));
        let result = session
            .result
            .await
            .unwrap_or_else(|error| panic!("TRAE result: {error}"));
        assert_eq!(result.status, "completed");
        assert_eq!(result.output, "current answer");
        assert_eq!(result.session_id, "existing-session");
        let requests = std::fs::read_to_string(requests)
            .unwrap_or_else(|error| panic!("read TRAE requests: {error}"));
        assert!(requests.contains("\"method\":\"session/load\""));
        assert!(!requests.contains("\"method\":\"session/resume\""));
    }
}
