//! CodeBuddy model and effort discovery through its ACP handshake.

use std::collections::BTreeSet;
use std::io;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio_util::sync::CancellationToken;

use crate::codebuddy::CodebuddyBackend;
use crate::command::filter_launch_prefix;
use crate::env::configure_child_env;
use crate::model::{
    parse_acp_session_modes, Catalog, CatalogCache, Model, ModelDiscoveryCacheKey, ModelThinking,
    ThinkingLevel,
};
use crate::process::OwnedProcessTree;
use crate::stderr::sanitize_diagnostic;
use crate::stream::AgentLineReader;

const DEFAULT_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
const TERMINATION_GRACE: Duration = Duration::from_secs(2);
const KILL_GRACE: Duration = Duration::from_secs(10);
const CLIENT_NAME: &str = "patchbay-model-discovery";

impl CodebuddyBackend {
    /// Returns one authoritative ACP catalog or a marked static stand-in. The
    /// cache rejects stand-ins, so authentication and transient CLI failures
    /// are retried on the next refresh instead of poisoning registration.
    pub async fn discover_models(
        &self,
        cache: &CatalogCache,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Catalog {
        let Some(key) = ModelDiscoveryCacheKey::new("codebuddy", &self.config().command) else {
            return fallback_catalog();
        };
        if let Some(catalog) = cache.get(&key) {
            return catalog;
        }
        let timeout = if timeout.is_zero() {
            DEFAULT_DISCOVERY_TIMEOUT
        } else {
            timeout
        };
        let catalog = match discover_once(self, cancellation, timeout).await {
            Ok(catalog) if !catalog.models.is_empty() => catalog,
            Ok(_) => fallback_catalog(),
            Err(failure) => {
                tracing::debug!(
                    provider = "codebuddy",
                    stage = failure.stage,
                    error = %sanitize_diagnostic(&failure.detail),
                    "model discovery fell back to static catalog"
                );
                fallback_catalog()
            }
        };
        let _ = cache.insert(key, catalog.clone());
        catalog
    }
}

#[derive(Debug)]
struct DiscoveryFailure {
    stage: &'static str,
    detail: String,
}

impl DiscoveryFailure {
    fn new(stage: &'static str, detail: impl Into<String>) -> Self {
        Self {
            stage,
            detail: detail.into(),
        }
    }
}

async fn discover_once(
    backend: &CodebuddyBackend,
    cancellation: CancellationToken,
    timeout: Duration,
) -> Result<Catalog, DiscoveryFailure> {
    let config = backend.config();
    let command_path = if config.command.path.is_empty() {
        "codebuddy"
    } else {
        config.command.path.as_str()
    };
    let prefix = filter_launch_prefix(
        &config.command.prefix,
        CodebuddyBackend::blocked_launch_args(),
    );
    let mut argv = prefix.args;
    argv.push("--acp".to_string());
    let mut command = Command::new(command_path);
    command
        .args(argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(false);
    configure_child_env(&mut command, &config.env);
    let mut tree = OwnedProcessTree::spawn(&mut command)
        .await
        .map_err(|error| DiscoveryFailure::new("process start", error.to_string()))?;
    let mut stdin = tree
        .child_mut()
        .stdin
        .take()
        .ok_or_else(|| DiscoveryFailure::new("stdin setup", "pipe unavailable"))?;
    let stdout = tree
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| DiscoveryFailure::new("stdout setup", "pipe unavailable"))?;
    let mut reader = AgentLineReader::new(BufReader::new(stdout));

    let result = {
        let handshake = run_handshake(&mut stdin, &mut reader);
        tokio::pin!(handshake);
        tokio::select! {
            result = &mut handshake => result,
            () = cancellation.cancelled() => Err(DiscoveryFailure::new("cancellation", "cancelled")),
            () = tokio::time::sleep(timeout) => Err(DiscoveryFailure::new("timeout", format!("exceeded {}s", timeout.as_secs_f64()))),
        }
    };
    drop(stdin);
    let _ = tree.shutdown(TERMINATION_GRACE, KILL_GRACE).await;
    result
}

async fn run_handshake(
    stdin: &mut ChildStdin,
    reader: &mut AgentLineReader<BufReader<ChildStdout>>,
) -> Result<Catalog, DiscoveryFailure> {
    let mut next_id = 1_u64;
    request(
        stdin,
        reader,
        &mut next_id,
        "initialize",
        serde_json::json!({
            "protocolVersion": 1,
            "clientInfo": {"name": CLIENT_NAME, "version": "0.1.0"},
            "clientCapabilities": {},
        }),
    )
    .await
    .map_err(|error| DiscoveryFailure::new("initialize", error))?;

    let directory = tempfile::Builder::new()
        .prefix("patchbay-codebuddy-discovery-")
        .tempdir()
        .map_err(|error| DiscoveryFailure::new("temporary cwd", error.to_string()))?;
    let result = request(
        stdin,
        reader,
        &mut next_id,
        "session/new",
        serde_json::json!({
            "cwd": directory.path().to_string_lossy(),
            "mcpServers": [],
        }),
    )
    .await
    .map_err(|error| DiscoveryFailure::new("session/new", error))?;
    let mut models = parse_models(&result);
    if models.is_empty() {
        return Err(DiscoveryFailure::new(
            "session/new model parsing",
            format!("no catalog in keys: {}", top_level_keys(&result).join(",")),
        ));
    }
    annotate_thinking(&mut models, &result);
    Ok(Catalog {
        models,
        session_modes: parse_acp_session_modes(&result),
        fallback: false,
    })
}

async fn request(
    stdin: &mut ChildStdin,
    reader: &mut AgentLineReader<BufReader<ChildStdout>>,
    next_id: &mut u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let id = *next_id;
    *next_id = (*next_id).saturating_add(1);
    let mut payload = serde_json::to_vec(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .map_err(|error| format!("serialize request: {error}"))?;
    payload.push(b'\n');
    stdin
        .write_all(&payload)
        .await
        .map_err(|error| format!("write request: {error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("flush request: {error}"))?;

    loop {
        let line = reader
            .next_line()
            .await
            .map_err(|error| format!("read response: {error}"))?
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof).to_string())?;
        let Ok(envelope) = serde_json::from_str::<RpcEnvelope>(line.trim()) else {
            continue;
        };
        if envelope.id.as_u64() != Some(id) {
            continue;
        }
        if let Some(error) = envelope.error {
            return Err(format!("RPC {}: {}", error.code, error.message));
        }
        return envelope
            .result
            .ok_or_else(|| "response contained neither result nor error".to_string());
    }
}

#[derive(Debug, Deserialize)]
struct RpcEnvelope {
    #[serde(default)]
    id: Value,
    result: Option<Value>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct SessionModels {
    #[serde(default, rename = "availableModels", alias = "available_models")]
    available: Vec<AvailableModel>,
    #[serde(default, rename = "currentModelId", alias = "current_model_id")]
    current: String,
}

#[derive(Debug, Deserialize)]
struct AvailableModel {
    #[serde(default, rename = "modelId", alias = "model_id")]
    id: String,
    #[serde(default)]
    name: String,
}

fn parse_models(result: &Value) -> Vec<Model> {
    let Some(raw) = result.get("models") else {
        return parse_config_option_models(result);
    };
    let Ok(catalog) = serde_json::from_value::<SessionModels>(raw.clone()) else {
        return parse_config_option_models(result);
    };
    let mut seen = BTreeSet::new();
    let mut models = Vec::new();
    for entry in catalog.available {
        let id = entry.id.trim();
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        models.push(Model {
            id: id.to_string(),
            label: if entry.name.trim().is_empty() {
                id.to_string()
            } else {
                entry.name.trim().to_string()
            },
            provider: model_provider(id).to_string(),
            default: id == catalog.current.trim(),
            ..Model::default()
        });
    }
    if models.is_empty() {
        parse_config_option_models(result)
    } else {
        models
    }
}

fn parse_config_option_models(result: &Value) -> Vec<Model> {
    let options = config_options(result);
    let Some(option) = options.iter().find(|option| {
        option.id.eq_ignore_ascii_case("model") || option.category.eq_ignore_ascii_case("model")
    }) else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    option
        .options
        .iter()
        .filter_map(|choice| {
            let id = choice.value.trim();
            if id.is_empty() || !seen.insert(id.to_string()) {
                return None;
            }
            Some(Model {
                id: id.to_string(),
                label: if choice.name.trim().is_empty() {
                    id.to_string()
                } else {
                    choice.name.trim().to_string()
                },
                provider: model_provider(id).to_string(),
                default: id == option.current.trim(),
                ..Model::default()
            })
        })
        .collect()
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ConfigOption {
    #[serde(default)]
    id: String,
    #[serde(default)]
    category: String,
    #[serde(default, rename = "currentValue", alias = "current_value")]
    current: String,
    #[serde(default)]
    options: Vec<ConfigChoice>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ConfigChoice {
    #[serde(default)]
    value: String,
    #[serde(default)]
    name: String,
}

fn config_options(result: &Value) -> Vec<ConfigOption> {
    result
        .get("configOptions")
        .or_else(|| result.get("config_options"))
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(|option| serde_json::from_value(option.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn annotate_thinking(models: &mut [Model], result: &Value) {
    let option = config_options(result).into_iter().find(|option| {
        matches!(
            option.id.to_ascii_lowercase().as_str(),
            "effort" | "thought_level"
        ) || matches!(
            option.category.to_ascii_lowercase().as_str(),
            "effort" | "thought_level"
        )
    });
    let (levels, default) = option.map_or_else(
        || (static_thinking_levels(), "medium".to_string()),
        |option| {
            let mut seen = BTreeSet::new();
            let levels: Vec<ThinkingLevel> = option
                .options
                .into_iter()
                .filter_map(|choice| {
                    let value = choice.value.trim();
                    if !valid_effort(value) || !seen.insert(value.to_string()) {
                        return None;
                    }
                    Some(ThinkingLevel {
                        value: value.to_string(),
                        label: effort_label(value).to_string(),
                        description: String::new(),
                    })
                })
                .collect();
            if levels.is_empty() {
                return (static_thinking_levels(), "medium".to_string());
            }
            let default = if valid_effort(option.current.trim()) {
                option.current.trim().to_string()
            } else {
                String::new()
            };
            (levels, default)
        },
    );
    let thinking = ModelThinking {
        supported_levels: levels,
        default_level: default,
    };
    for model in models {
        model.thinking = Some(thinking.clone());
    }
}

fn model_provider(id: &str) -> &'static str {
    if id.starts_with("claude-") {
        "anthropic"
    } else if id.starts_with("gemini-") {
        "google"
    } else if id.starts_with("gpt-") {
        "openai"
    } else if id.starts_with("glm-") {
        "zhipu"
    } else if id.starts_with("minimax-") {
        "minimax"
    } else if id.starts_with("kimi-") {
        "kimi"
    } else if id
        .as_bytes()
        .get(0..3)
        .is_some_and(|prefix| prefix[0] == b'h' && prefix[1] == b'y' && prefix[2].is_ascii_digit())
    {
        "hunyuan"
    } else if id.starts_with("deepseek-") {
        "deepseek"
    } else {
        ""
    }
}

fn valid_effort(value: &str) -> bool {
    matches!(
        value,
        "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
    )
}

fn effort_label(value: &str) -> &'static str {
    match value {
        "minimal" => "Minimal",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra high",
        "max" => "Max",
        _ => "",
    }
}

fn static_thinking_levels() -> Vec<ThinkingLevel> {
    ["minimal", "low", "medium", "high", "xhigh", "max"]
        .into_iter()
        .map(|value| ThinkingLevel {
            value: value.to_string(),
            label: effort_label(value).to_string(),
            description: String::new(),
        })
        .collect()
}

fn fallback_catalog() -> Catalog {
    let thinking = ModelThinking {
        supported_levels: static_thinking_levels(),
        default_level: "medium".to_string(),
    };
    let models = [
        ("claude-sonnet-4.6", "Claude Sonnet 4.6", "anthropic", true),
        ("claude-opus-4.7", "Claude Opus 4.7", "anthropic", false),
        ("gemini-3.1-pro", "Gemini 3.1 Pro", "google", false),
        ("gpt-5.5", "GPT 5.5", "openai", false),
        (
            "deepseek-v3-2-volc-ioa",
            "Deepseek V3 2 Volc IOA",
            "deepseek",
            false,
        ),
    ]
    .into_iter()
    .map(|(id, label, provider, default)| Model {
        id: id.to_string(),
        label: label.to_string(),
        provider: provider.to_string(),
        default,
        thinking: Some(thinking.clone()),
        ..Model::default()
    })
    .collect();
    Catalog {
        models,
        session_modes: Vec::new(),
        fallback: true,
    }
}

fn top_level_keys(result: &Value) -> Vec<String> {
    result
        .as_object()
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::codebuddy::CodebuddyConfig;
    use crate::command::RuntimeCommand;

    fn captured_session_result() -> Value {
        serde_json::json!({
            "sessionId": "session-codebuddy",
            "models": {
                "currentModelId": "hy3",
                "availableModels": [
                    {"modelId": "hy3", "name": "Hy3"},
                    {"modelId": "glm-5.2", "name": "GLM-5.2"},
                    {"modelId": "kimi-k3-1", "name": "Kimi-K3"},
                    {"modelId": "deepseek-v3-2-volc", "name": "DeepSeek-V3.2"}
                ]
            },
            "configOptions": [{
                "id": "thought_level", "category": "thought_level",
                "currentValue": "enabled",
                "options": [
                    {"value": "minimal"}, {"value": "low"}, {"value": "medium"},
                    {"value": "high"}, {"value": "xhigh"}, {"value": "max"},
                    {"value": "enabled"}
                ]
            }]
        })
    }

    #[test]
    fn captured_catalog_preserves_labels_default_vendors_and_effort() {
        let result = captured_session_result();
        let mut models = parse_models(&result);
        annotate_thinking(&mut models, &result);
        assert_eq!(models.len(), 4);
        assert_eq!(models[0].label, "Hy3");
        assert!(models[0].default);
        assert_eq!(models[0].provider, "hunyuan");
        assert_eq!(models[1].provider, "zhipu");
        assert_eq!(models[2].provider, "kimi");
        assert_eq!(models[3].provider, "deepseek");
        let thinking = models[0]
            .thinking
            .as_ref()
            .unwrap_or_else(|| panic!("effort catalog"));
        assert_eq!(thinking.supported_levels.len(), 6);
        assert!(thinking.default_level.is_empty());
        assert!(!thinking
            .supported_levels
            .iter()
            .any(|level| level.value == "enabled"));
        assert!(parse_acp_session_modes(&result).is_empty());
    }

    #[test]
    fn fallback_is_marked_and_cache_rejects_it() {
        let fallback = fallback_catalog();
        assert!(fallback.fallback);
        assert!(fallback.models.iter().all(|model| model.thinking.is_some()));
        let cache = CatalogCache::default();
        let key = ModelDiscoveryCacheKey::new("codebuddy", &RuntimeCommand::default())
            .unwrap_or_else(|| panic!("cache key"));
        assert!(!cache.insert(key, fallback));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn discovery_drives_initialize_and_session_new_once() {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| panic!("create discovery fixture: {error}"));
        let executable = directory.path().join("codebuddy");
        let session = captured_session_result().to_string();
        let script = format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":1}}}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":%s}}\n' "$id" '{session}'; exit 0 ;;
  esac
done
"#
        );
        std::fs::write(&executable, script)
            .unwrap_or_else(|error| panic!("write discovery fixture: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod discovery fixture: {error}"));
        let backend = CodebuddyBackend::new(CodebuddyConfig {
            command: RuntimeCommand::new(executable.to_string_lossy(), Vec::new()),
            ..CodebuddyConfig::default()
        });
        let catalog = backend
            .discover_models(
                &CatalogCache::default(),
                CancellationToken::new(),
                Duration::from_secs(5),
            )
            .await;
        assert!(!catalog.fallback);
        assert_eq!(catalog.models.len(), 4);
    }

    #[test]
    fn malformed_models_falls_back_to_config_options() {
        let result = serde_json::json!({
            "models": null,
            "configOptions": [{
                "id": "model",
                "currentValue": "hy3",
                "options": [
                    {"value": "hy3", "name": "Hy3"},
                    {"value": "glm-5.2", "name": "GLM"}
                ]
            }]
        });
        let models = parse_models(&result);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "hy3");
        assert!(models[0].default);
        assert_eq!(models[1].id, "glm-5.2");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn discovery_filters_protocol_critical_launch_prefix() {
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| panic!("create discovery fixture: {error}"));
        let executable = directory.path().join("codebuddy");
        let session = captured_session_result().to_string();
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > "$0.args"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":1}}}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":%s}}\n' "$id" '{session}'; exit 0 ;;
  esac
done
"#
        );
        std::fs::write(&executable, script)
            .unwrap_or_else(|error| panic!("write discovery fixture: {error}"));
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .unwrap_or_else(|error| panic!("chmod discovery fixture: {error}"));
        let backend = CodebuddyBackend::new(CodebuddyConfig {
            command: RuntimeCommand::new(
                executable.to_string_lossy(),
                ["--output-format", "text", "--verbose"]
                    .map(str::to_string)
                    .to_vec(),
            ),
            ..CodebuddyConfig::default()
        });
        let catalog = backend
            .discover_models(
                &CatalogCache::default(),
                CancellationToken::new(),
                Duration::from_secs(5),
            )
            .await;
        assert!(!catalog.fallback);
        let args = std::fs::read_to_string(format!("{}.args", executable.display()))
            .unwrap_or_else(|error| panic!("read discovery args: {error}"));
        assert!(!args.contains("--output-format"), "{args}");
        assert!(args.contains("--acp"), "{args}");
        assert!(args.contains("--verbose"), "{args}");
    }
}
