//! Canonical runtime-family metadata and fail-closed backend construction.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::antigravity::{AntigravityBackend, AntigravityConfig};
use crate::claude::{ClaudeBackend, ClaudeConfig};
use crate::codebuddy::{CodebuddyBackend, CodebuddyConfig};
use crate::codex::{CodexBackend, CodexConfig};
use crate::command::RuntimeCommand;
use crate::contract::{AgentError, Backend};
use crate::copilot::{CopilotBackend, CopilotConfig};
use crate::cursor::{CursorBackend, CursorConfig};
use crate::deveco::{DevecoBackend, DevecoConfig};
use crate::dsh::{DshBackend, DshConfig};
use crate::openclaw::{OpenclawBackend, OpenclawConfig};
use crate::opencode::{OpencodeBackend, OpencodeConfig};
use crate::pi::{PiBackend, PiConfig};
use crate::qoder::{
    DimBackend, DimConfig, GrokBackend, GrokConfig, HermesBackend, HermesConfig, KimiBackend,
    KimiConfig, KiroBackend, KiroConfig, McodeBackend, McodeConfig, QoderBackend, QoderConfig,
    QwenpawBackend, QwenpawConfig, ReasonixBackend, ReasonixConfig, TraecliBackend, TraecliConfig,
};
use crate::qwen::{QwenBackend, QwenConfig};

/// Provider-neutral launch inputs resolved by daemon profile/runtime loading.
#[derive(Clone, Default)]
pub struct BackendConfig {
    pub command: RuntimeCommand,
    pub env: BTreeMap<String, String>,
    /// Built-in runtime identity is required for provider-specific capability
    /// exceptions. A custom profile with the same protocol family must remain
    /// fail-closed until its own behavior is verified.
    pub builtin_runtime: bool,
}

impl std::fmt::Debug for BackendConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BackendConfig")
            .field("command_path", &self.command.path)
            .field("environment_variable_count", &self.env.len())
            .field("builtin_runtime", &self.builtin_runtime)
            .finish_non_exhaustive()
    }
}

/// Constructs only protocol families with a real implementation in this
/// crate. Metadata registration is deliberately insufficient: callers get a
/// hard error instead of a backend that fails later or pretends to execute.
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
        "claude" => Ok(Arc::new(ClaudeBackend::new(ClaudeConfig {
            command: config.command,
            env: config.env,
        }))),
        "copilot" => Ok(Arc::new(CopilotBackend::new(CopilotConfig {
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
        "codebuddy" => Ok(Arc::new(CodebuddyBackend::new(CodebuddyConfig {
            command: config.command,
            env: config.env,
        }))),
        "codex" => Ok(Arc::new(CodexBackend::new(CodexConfig {
            command: config.command,
            env: config.env,
        }))),
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
        "hermes" => Ok(Arc::new(HermesBackend::new(HermesConfig {
            command: config.command,
            env: config.env,
            builtin_runtime: config.builtin_runtime,
        }))),
        "mcode" => Ok(Arc::new(McodeBackend::new(McodeConfig {
            command: config.command,
            env: config.env,
        }))),
        "dim" => Ok(Arc::new(DimBackend::new(DimConfig {
            command: config.command,
            env: config.env,
        }))),
        "dsh" => Ok(Arc::new(DshBackend::new(DshConfig {
            command: config.command,
            env: config.env,
        }))),
        "deveco" => Ok(Arc::new(DevecoBackend::new(DevecoConfig {
            command: config.command,
            env: config.env,
        }))),
        "opencode" => Ok(Arc::new(OpencodeBackend::new(OpencodeConfig {
            command: config.command,
            env: config.env,
        }))),
        "openclaw" => Ok(Arc::new(OpenclawBackend::new(OpenclawConfig {
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
pub enum ModelDiscoveryOutput {
    OmpModelsJson,
}

/// A built-in runtime's model-discovery command. Presence replaces protocol-
/// family discovery entirely; absence disables discovery for that runtime
/// rather than falling back to a potentially incompatible family command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelDiscoveryOverride {
    pub arguments: &'static [&'static str],
    pub output: ModelDiscoveryOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelDiscoveryStrategy {
    ProviderFamily(&'static str),
    RuntimeOverride(ModelDiscoveryOverride),
    DisabledBuiltinRuntime,
    Unknown,
}

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
    pub default_executable: &'static str,
    pub provider_label: &'static str,
    pub model_discovery: Option<ModelDiscoveryOverride>,
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
    default_executable: "omp",
    provider_label: "omp",
    model_discovery: Some(ModelDiscoveryOverride {
        arguments: &["models", "--json"],
        output: ModelDiscoveryOutput::OmpModelsJson,
    }),
}];

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

/// Resolves discovery without allowing a built-in runtime identity to
/// silently inherit its protocol family's CLI command.
pub fn model_discovery_strategy(id: &str) -> ModelDiscoveryStrategy {
    if let Some(runtime) = builtin_runtime(id) {
        return runtime.model_discovery.map_or(
            ModelDiscoveryStrategy::DisabledBuiltinRuntime,
            ModelDiscoveryStrategy::RuntimeOverride,
        );
    }
    provider(id).map_or(ModelDiscoveryStrategy::Unknown, |provider| {
        ModelDiscoveryStrategy::ProviderFamily(provider.id)
    })
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

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
    fn omp_descriptor_owns_execution_and_model_discovery_overrides() {
        let Some(omp) = builtin_runtime("omp") else {
            panic!("omp descriptor must exist");
        };
        assert_eq!(omp.default_executable, "omp");
        assert_eq!(omp.provider_label, "omp");
        let discovery = ModelDiscoveryOverride {
            arguments: &["models", "--json"],
            output: ModelDiscoveryOutput::OmpModelsJson,
        };
        assert_eq!(omp.model_discovery, Some(discovery));
        assert_eq!(
            model_discovery_strategy("omp"),
            ModelDiscoveryStrategy::RuntimeOverride(discovery)
        );
        assert_ne!(
            model_discovery_strategy("omp"),
            ModelDiscoveryStrategy::ProviderFamily("pi"),
            "OMP must never fall back to Pi's incompatible --list-models command"
        );
        assert_eq!(
            model_discovery_strategy("pi"),
            ModelDiscoveryStrategy::ProviderFamily("pi")
        );
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
    fn backend_registry_constructs_only_landed_protocols() {
        let claude = build_backend("claude", BackendConfig::default());
        assert!(claude.is_ok());
        let copilot = build_backend("copilot", BackendConfig::default());
        assert!(copilot.is_ok());
        let codex = build_backend("codex", BackendConfig::default());
        assert!(codex.is_ok());

        let cursor = build_backend("cursor", BackendConfig::default());
        assert!(cursor.is_ok());

        let qwen = build_backend("qwen", BackendConfig::default());
        assert!(qwen.is_ok());

        let codebuddy = build_backend("codebuddy", BackendConfig::default());
        assert!(codebuddy.is_ok());

        let codex = build_backend("codex", BackendConfig::default());
        assert!(codex.is_ok());

        let antigravity = build_backend("antigravity", BackendConfig::default());
        assert!(antigravity.is_ok());

        assert!(build_backend("qoder", BackendConfig::default()).is_ok());
        assert!(build_backend("qoderclicn", BackendConfig::default()).is_ok());
        assert!(build_backend("traecli", BackendConfig::default()).is_ok());
        assert!(build_backend("kiro", BackendConfig::default()).is_ok());
        assert!(build_backend("qwenpaw", BackendConfig::default()).is_ok());
        assert!(build_backend("kimi", BackendConfig::default()).is_ok());
        assert!(build_backend("reasonix", BackendConfig::default()).is_ok());
        assert!(build_backend("dsh", BackendConfig::default()).is_ok());
        assert!(build_backend("deveco", BackendConfig::default()).is_ok());
        assert!(build_backend("opencode", BackendConfig::default()).is_ok());
        assert!(build_backend("openclaw", BackendConfig::default()).is_ok());
        let pi = build_backend("pi", BackendConfig::default());
        assert!(pi.is_ok(), "custom Pi runtime must build");
        let omp = build_backend("omp", BackendConfig::default());
        assert!(omp.is_ok(), "OMP runtime must reuse the Pi backend family");
        assert!(build_backend("grok", BackendConfig::default()).is_ok());
        assert!(build_backend("mcode", BackendConfig::default()).is_ok());
        assert!(build_backend("dim", BackendConfig::default()).is_ok());

        let unknown = build_backend("unknown", BackendConfig::default());
        assert!(matches!(
            unknown,
            Err(AgentError::UnsupportedRuntime(runtime)) if runtime == "unknown"
        ));
    }
}
