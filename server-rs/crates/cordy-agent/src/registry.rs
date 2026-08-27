//! Canonical runtime-family metadata and fail-closed backend construction.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::antigravity::{AntigravityBackend, AntigravityConfig};
use crate::codebuddy::{CodebuddyBackend, CodebuddyConfig};
use crate::codex::{CodexBackend, CodexConfig};
use crate::command::RuntimeCommand;
use crate::contract::{AgentError, Backend};
use crate::cursor::{CursorBackend, CursorConfig};
use crate::deveco::{DevecoBackend, DevecoConfig};
use crate::dsh::{DshBackend, DshConfig};
use crate::model::{Catalog, CatalogCache};
use crate::openclaw::{OpenclawBackend, OpenclawConfig};
use crate::opencode::{OpencodeBackend, OpencodeConfig};
use crate::pi::{PiBackend, PiConfig};
use crate::qoder::{
    DimBackend, DimConfig, GrokBackend, GrokConfig, KimiBackend, KimiConfig, KiroBackend,
    KiroConfig, McodeBackend, McodeConfig, QoderBackend, QoderConfig, QwenpawBackend,
    QwenpawConfig, ReasonixBackend, ReasonixConfig, TraecliBackend, TraecliConfig,
};
use crate::qwen::{QwenBackend, QwenConfig};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    pub default_command: &'static str,
    pub env_prefix: &'static str,
    pub display_name: &'static str,
    pub launch_header: &'static str,
    pub minimum_version: Option<&'static str>,
    pub model_selection_supported: bool,
    pub resume_rejection_undetectable: bool,
}

macro_rules! provider {
    ($id:literal, $cmd:literal, $env:literal, $name:literal, $header:literal) => {
        ProviderDescriptor {
            id: $id,
            default_command: $cmd,
            env_prefix: $env,
            display_name: $name,
            launch_header: $header,
            minimum_version: None,
            model_selection_supported: true,
            resume_rejection_undetectable: false,
        }
    };
}

pub const PROVIDERS: &[ProviderDescriptor] = &[
    ProviderDescriptor {
        minimum_version: Some("2.0.0"),
        ..provider!(
            "claude",
            "claude",
            "CORDY_CLAUDE",
            "Claude",
            "claude (stream-json)"
        )
    },
    provider!(
        "codebuddy",
        "codebuddy",
        "CORDY_CODEBUDDY",
        "CodeBuddy",
        "codebuddy (stream-json)"
    ),
    ProviderDescriptor {
        minimum_version: Some("0.100.0"),
        ..provider!("codex", "codex", "CORDY_CODEX", "Codex", "codex app-server")
    },
    ProviderDescriptor {
        minimum_version: Some("1.0.0"),
        resume_rejection_undetectable: true,
        ..provider!(
            "copilot",
            "copilot",
            "CORDY_COPILOT",
            "Copilot",
            "copilot (json)"
        )
    },
    ProviderDescriptor {
        resume_rejection_undetectable: true,
        ..provider!(
            "opencode",
            "opencode",
            "CORDY_OPENCODE",
            "OpenCode",
            "opencode run (json)"
        )
    },
    ProviderDescriptor {
        resume_rejection_undetectable: true,
        ..provider!(
            "deveco",
            "deveco",
            "CORDY_DEVECO",
            "DevEco",
            "deveco run (json)"
        )
    },
    provider!(
        "openclaw",
        "openclaw",
        "CORDY_OPENCLAW",
        "OpenClaw",
        "openclaw agent (json)"
    ),
    provider!("hermes", "hermes", "CORDY_HERMES", "Hermes", "hermes acp"),
    provider!("pi", "pi", "CORDY_PI", "Pi", "pi (json mode)"),
    ProviderDescriptor {
        resume_rejection_undetectable: true,
        ..provider!(
            "cursor",
            "cursor-agent",
            "CORDY_CURSOR",
            "Cursor",
            "cursor-agent (stream-json)"
        )
    },
    provider!("kimi", "kimi", "CORDY_KIMI", "Kimi", "kimi acp"),
    provider!(
        "reasonix",
        "reasonix",
        "CORDY_REASONIX",
        "Reasonix",
        "reasonix acp"
    ),
    provider!(
        "dsh",
        "dsh",
        "CORDY_DSH",
        "DeepSeek Harness",
        "dsh --profile cordy (stdio)"
    ),
    provider!("kiro", "kiro-cli", "CORDY_KIRO", "Kiro", "kiro-cli acp"),
    ProviderDescriptor {
        resume_rejection_undetectable: true,
        ..provider!(
            "antigravity",
            "agy",
            "CORDY_ANTIGRAVITY",
            "Antigravity",
            "agy -p (non-interactive)"
        )
    },
    provider!(
        "qoder",
        "qodercli",
        "CORDY_QODER",
        "Qoder",
        "qodercli --acp"
    ),
    provider!(
        "qoderclicn",
        "qoderclicn",
        "CORDY_QODERCLICN",
        "Qoder CN",
        "qoderclicn --acp"
    ),
    provider!(
        "traecli",
        "traecli",
        "CORDY_TRAECLI",
        "Trae",
        "traecli acp serve"
    ),
    ProviderDescriptor {
        minimum_version: Some("0.2.89"),
        ..provider!("grok", "grok", "CORDY_GROK", "Grok", "grok agent stdio")
    },
    ProviderDescriptor {
        minimum_version: Some("0.20.0"),
        ..provider!(
            "qwen",
            "qwen",
            "CORDY_QWEN",
            "Qwen Code",
            "qwen -p (stream-json)"
        )
    },
    ProviderDescriptor {
        model_selection_supported: false,
        ..provider!(
            "qwenpaw",
            "qwenpaw",
            "CORDY_QWENPAW",
            "QwenPaw",
            "qwenpaw acp"
        )
    },
    ProviderDescriptor {
        minimum_version: Some("0.1.2"),
        model_selection_supported: false,
        ..provider!("mcode", "mcode", "CORDY_MCODE", "MiniMax Code", "mcode acp")
    },
    ProviderDescriptor {
        minimum_version: Some("0.3.10"),
        ..provider!("dim", "dim", "CORDY_DIM", "Dim", "dim acp")
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinRuntimeDescriptor {
    pub id: &'static str,
    pub protocol_family: &'static str,
    pub default_command: &'static str,
    pub env_prefix: &'static str,
    pub display_name: &'static str,
    pub skills_dir: &'static str,
    pub user_skills_dir: &'static str,
    pub launch_header: &'static str,
}

pub const BUILTIN_RUNTIMES: &[BuiltinRuntimeDescriptor] = &[BuiltinRuntimeDescriptor {
    id: "omp",
    protocol_family: "pi",
    default_command: "omp",
    env_prefix: "CORDY_OMP",
    display_name: "Oh-My-Pi",
    skills_dir: ".omp/skills",
    user_skills_dir: ".omp/agent/skills",
    launch_header: "omp (json mode)",
}];

/// Provider-neutral launch inputs resolved by daemon profile/runtime loading.
#[derive(Clone, Default)]
pub struct BackendConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
}

impl std::fmt::Debug for BackendConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BackendConfig")
            .field("command_path", &self.command.path)
            .field("environment_variable_count", &self.env.len())
            .finish_non_exhaustive()
    }
}

pub fn provider(id: &str) -> Option<&'static ProviderDescriptor> {
    PROVIDERS.iter().find(|provider| provider.id == id)
}

pub fn builtin_runtime(id: &str) -> Option<&'static BuiltinRuntimeDescriptor> {
    BUILTIN_RUNTIMES.iter().find(|runtime| runtime.id == id)
}

pub fn protocol_family(id: &str) -> Option<&'static str> {
    if let Some(provider) = provider(id) {
        return Some(provider.id);
    }
    builtin_runtime(id).map(|runtime| runtime.protocol_family)
}

pub fn launch_header(id: &str) -> &'static str {
    provider(id)
        .map(|provider| provider.launch_header)
        .or_else(|| builtin_runtime(id).map(|runtime| runtime.launch_header))
        .unwrap_or("")
}

pub fn resume_rejection_undetectable(id: &str) -> bool {
    protocol_family(id)
        .and_then(provider)
        .is_some_and(|provider| provider.resume_rejection_undetectable)
}

pub fn model_selection_supported(id: &str) -> bool {
    protocol_family(id)
        .and_then(provider)
        .is_none_or(|provider| provider.model_selection_supported)
}

/// Reports whether a runtime rejects a model selector without its provider
/// prefix. This is an execution contract: only these runtimes need a catalog
/// read before launching a task with a pinned model.
pub fn model_selector_must_be_provider_qualified(id: &str) -> bool {
    matches!(protocol_family(id), Some("opencode" | "deveco"))
}

/// Discovers the model catalog for an accepted runtime command.
///
/// The daemon must use the command selected by registration, including a
/// custom profile's fixed prefix, so discovery cannot be implemented by
/// rebuilding a provider's default command at the call site. Providers that
/// deliberately have no account-independent catalog return an empty catalog;
/// unsupported provider families fail closed instead of pretending discovery
/// succeeded.
pub async fn discover_models(
    runtime_id: &str,
    config: BackendConfig,
    cache: &CatalogCache,
    cancellation: CancellationToken,
    timeout: Duration,
) -> Result<Catalog, AgentError> {
    let family = protocol_family(runtime_id)
        .ok_or_else(|| AgentError::UnsupportedRuntime(runtime_id.to_string()))?;
    let BackendConfig { command, env } = config;
    match family {
        "antigravity" => Ok(AntigravityBackend::new(AntigravityConfig {
            command,
            env,
            ..AntigravityConfig::default()
        })
        .discover_models(cancellation, timeout)
        .await),
        "codebuddy" => Ok(CodebuddyBackend::new(CodebuddyConfig { command, env })
            .discover_models(cache, cancellation, timeout)
            .await),
        "cursor" => Ok(CursorBackend::new(CursorConfig { command, env })
            .discover_models_for_runtime(runtime_id, cache, cancellation, timeout)
            .await),
        "deveco" => Ok(DevecoBackend::new(DevecoConfig { command, env })
            .discover_models_for_runtime(runtime_id, cache, cancellation, timeout)
            .await),
        "dsh" => Ok(DshBackend::new(DshConfig { command, env })
            .discover_models_for_runtime(runtime_id, cache, cancellation, timeout)
            .await),
        "openclaw" => Ok(OpenclawBackend::new(OpenclawConfig { command, env })
            .discover_models_for_runtime(runtime_id, cache, cancellation, timeout)
            .await),
        "opencode" => Ok(OpencodeBackend::new(OpencodeConfig { command, env })
            .discover_models_for_runtime(runtime_id, cache, cancellation, timeout)
            .await),
        "pi" => Ok(PiBackend::new(PiConfig {
            command,
            env,
            default_executable: if runtime_id == "omp" {
                "omp".to_string()
            } else {
                "pi".to_string()
            },
            provider_label: if runtime_id == "omp" {
                "omp".to_string()
            } else {
                "pi".to_string()
            },
        })
        .discover_models_for_runtime(runtime_id, cache, cancellation, timeout)
        .await),
        "qoder" | "qoderclicn" => {
            let backend = QoderBackend::new(QoderConfig {
                command,
                env,
                default_command: if runtime_id == "qoderclicn" {
                    "qoderclicn".to_string()
                } else {
                    "qodercli".to_string()
                },
                provider: runtime_id.to_string(),
                ..QoderConfig::default()
            });
            Ok(backend.discover_models(cache, cancellation, timeout).await)
        }
        "traecli" => Ok(TraecliBackend::new(TraecliConfig { command, env })
            .discover_models(cache, cancellation, timeout)
            .await),
        "kiro" => Ok(KiroBackend::new(KiroConfig { command, env })
            .discover_models(cache, cancellation, timeout)
            .await),
        "kimi" => Ok(KimiBackend::new(KimiConfig { command, env })
            .discover_models(cache, cancellation, timeout)
            .await),
        "reasonix" => Ok(ReasonixBackend::new(ReasonixConfig { command, env })
            .discover_models(cache, cancellation, timeout)
            .await),
        "grok" => Ok(GrokBackend::new(GrokConfig { command, env })
            .discover_models(cache, cancellation, timeout)
            .await),
        "dim" => Ok(DimBackend::new(DimConfig { command, env })
            .discover_models(cache, cancellation, timeout)
            .await),
        "qwen" | "qwenpaw" | "mcode" => Ok(Catalog::default()),
        "codex" => Ok(CodexBackend::new(CodexConfig { command, env })
            .discover_models(cache, cancellation, timeout)
            .await),
        _ => Err(AgentError::UnsupportedRuntime(runtime_id.to_string())),
    }
}

/// Constructs a real backend for the provider families already implemented in
/// this crate. Registry metadata alone is not enough: unsupported families
/// fail before a task can pretend to execute.
pub fn build_backend(
    runtime_id: &str,
    config: BackendConfig,
) -> Result<Arc<dyn Backend>, AgentError> {
    let family = protocol_family(runtime_id)
        .ok_or_else(|| AgentError::UnsupportedRuntime(runtime_id.to_string()))?;
    match family {
        "antigravity" => Ok(Arc::new(AntigravityBackend::new(AntigravityConfig {
            command: config.command,
            env: config.env,
            ..AntigravityConfig::default()
        }))),
        "codebuddy" => Ok(Arc::new(CodebuddyBackend::new(CodebuddyConfig {
            command: config.command,
            env: config.env,
        }))),
        "codex" => Ok(Arc::new(CodexBackend::new(CodexConfig {
            command: config.command,
            env: config.env,
        }))),
        "cursor" => Ok(Arc::new(CursorBackend::new(CursorConfig {
            command: config.command,
            env: config.env,
        }))),
        "deveco" => Ok(Arc::new(DevecoBackend::new(DevecoConfig {
            command: config.command,
            env: config.env,
        }))),
        "dsh" => Ok(Arc::new(DshBackend::new(DshConfig {
            command: config.command,
            env: config.env,
        }))),
        "openclaw" => Ok(Arc::new(OpenclawBackend::new(OpenclawConfig {
            command: config.command,
            env: config.env,
        }))),
        "opencode" => Ok(Arc::new(OpencodeBackend::new(OpencodeConfig {
            command: config.command,
            env: config.env,
        }))),
        "pi" => {
            let (default_executable, provider_label) = if runtime_id == "omp" {
                ("omp", "omp")
            } else {
                ("pi", "pi")
            };
            Ok(Arc::new(PiBackend::new(PiConfig {
                command: config.command,
                env: config.env,
                default_executable: default_executable.to_string(),
                provider_label: provider_label.to_string(),
            })))
        }
        "qwen" => Ok(Arc::new(QwenBackend::new(QwenConfig {
            command: config.command,
            env: config.env,
        }))),
        "qwenpaw" => Ok(Arc::new(QwenpawBackend::new(QwenpawConfig {
            command: config.command,
            env: config.env,
        }))),
        "kiro" => Ok(Arc::new(KiroBackend::new(KiroConfig {
            command: config.command,
            env: config.env,
        }))),
        "kimi" => Ok(Arc::new(KimiBackend::new(KimiConfig {
            command: config.command,
            env: config.env,
        }))),
        "reasonix" => Ok(Arc::new(ReasonixBackend::new(ReasonixConfig {
            command: config.command,
            env: config.env,
        }))),
        "grok" => Ok(Arc::new(GrokBackend::new(GrokConfig {
            command: config.command,
            env: config.env,
        }))),
        "mcode" => Ok(Arc::new(McodeBackend::new(McodeConfig {
            command: config.command,
            env: config.env,
        }))),
        "dim" => Ok(Arc::new(DimBackend::new(DimConfig {
            command: config.command,
            env: config.env,
        }))),
        "qoder" | "qoderclicn" => Ok(Arc::new(QoderBackend::new(QoderConfig {
            command: config.command,
            env: config.env,
            default_command: if runtime_id == "qoderclicn" {
                "qoderclicn".to_string()
            } else {
                "qodercli".to_string()
            },
            ..QoderConfig::default()
        }))),
        "traecli" => Ok(Arc::new(TraecliBackend::new(TraecliConfig {
            command: config.command,
            env: config.env,
        }))),
        _ => Err(AgentError::UnsupportedRuntime(runtime_id.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn backend_config() -> BackendConfig {
        BackendConfig {
            command: RuntimeCommand::new("/bin/agent", Vec::new()),
            ..BackendConfig::default()
        }
    }

    #[test]
    fn provider_whitelist_matches_latest_migration_contract() {
        let expected: BTreeSet<&str> = [
            "claude",
            "codebuddy",
            "codex",
            "copilot",
            "opencode",
            "deveco",
            "openclaw",
            "hermes",
            "pi",
            "cursor",
            "kimi",
            "reasonix",
            "dsh",
            "kiro",
            "antigravity",
            "qoder",
            "qoderclicn",
            "traecli",
            "grok",
            "qwen",
            "qwenpaw",
            "mcode",
            "dim",
        ]
        .into_iter()
        .collect();
        let actual: BTreeSet<&str> = PROVIDERS.iter().map(|provider| provider.id).collect();
        assert_eq!(actual, expected);
        assert_eq!(actual.len(), PROVIDERS.len(), "provider ids must be unique");
    }

    #[test]
    fn every_runtime_has_launch_and_command_metadata() {
        for provider in PROVIDERS {
            assert!(
                !provider.default_command.is_empty(),
                "{} command",
                provider.id
            );
            assert!(!provider.launch_header.is_empty(), "{} header", provider.id);
            assert_eq!(protocol_family(provider.id), Some(provider.id));
        }
        assert_eq!(protocol_family("omp"), Some("pi"));
        assert_eq!(launch_header("omp"), "omp (json mode)");
        assert_eq!(launch_header("unknown"), "");
    }

    #[test]
    fn capability_exceptions_are_exact() {
        let undetectable: BTreeSet<&str> = PROVIDERS
            .iter()
            .filter(|provider| provider.resume_rejection_undetectable)
            .map(|provider| provider.id)
            .collect();
        assert_eq!(
            undetectable,
            ["antigravity", "copilot", "cursor", "deveco", "opencode"]
                .into_iter()
                .collect()
        );
        assert!(!model_selection_supported("qwenpaw"));
        assert!(!model_selection_supported("mcode"));
        assert!(model_selection_supported("omp"));
        assert!(model_selection_supported("unknown"));
    }

    #[test]
    fn selector_qualification_follows_the_runtime_protocol_family() {
        assert!(model_selector_must_be_provider_qualified("opencode"));
        assert!(model_selector_must_be_provider_qualified("deveco"));
        assert!(!model_selector_must_be_provider_qualified("pi"));
        assert!(!model_selector_must_be_provider_qualified("omp"));
        assert!(!model_selector_must_be_provider_qualified("unknown"));
    }

    #[test]
    fn factory_constructs_every_implemented_runtime() {
        for runtime in [
            "antigravity",
            "codebuddy",
            "codex",
            "cursor",
            "deveco",
            "dsh",
            "openclaw",
            "opencode",
            "pi",
            "omp",
            "qwen",
            "qwenpaw",
            "kiro",
            "kimi",
            "reasonix",
            "grok",
            "mcode",
            "dim",
            "qoder",
            "qoderclicn",
            "traecli",
        ] {
            assert!(
                build_backend(runtime, backend_config()).is_ok(),
                "factory rejected implemented runtime {runtime}"
            );
        }
    }

    #[test]
    fn factory_fails_closed_for_unknown_or_unimplemented_runtime() {
        for runtime in ["unknown", "claude", "copilot", "hermes"] {
            assert!(matches!(
                build_backend(runtime, backend_config()),
                Err(AgentError::UnsupportedRuntime(value)) if value == runtime
            ));
        }
    }

    #[tokio::test]
    async fn discovery_keeps_catalogless_runtimes_empty() {
        let catalog = discover_models(
            "qwen",
            BackendConfig::default(),
            &CatalogCache::default(),
            tokio_util::sync::CancellationToken::new(),
            Duration::ZERO,
        )
        .await
        .expect("catalogless runtime discovery is supported");
        assert_eq!(catalog, Catalog::default());
    }

    #[tokio::test]
    async fn codex_discovery_uses_static_fallback_when_cli_is_unavailable() {
        let catalog = discover_models(
            "codex",
            BackendConfig {
                command: RuntimeCommand::new("/nonexistent/codex", Vec::new()),
                ..BackendConfig::default()
            },
            &CatalogCache::default(),
            tokio_util::sync::CancellationToken::new(),
            Duration::ZERO,
        )
        .await
        .expect("codex discovery should degrade to its static catalog");
        assert_eq!(
            catalog.models.first().map(|model| model.id.as_str()),
            Some("gpt-5.6-sol")
        );
    }
}
