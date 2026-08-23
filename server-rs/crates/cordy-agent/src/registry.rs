//! Canonical runtime-family metadata.
//!
//! This is metadata, not a backend factory. The crate does not claim a family
//! is executable until a concrete adapter is registered by a later slice.

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
}
