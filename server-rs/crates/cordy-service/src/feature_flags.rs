//! Feature-flag evaluation and key vocabulary — port of
//! `server/pkg/featureflag` and `server/internal/featureflags/keys.go`.
//!
//! The Rust call sites mostly need a boolean seam, but the source also keeps
//! the Go rule-file contract (allow/deny targeting, percent rollouts, custom
//! attributes, and variants) so a self-hosted deployment does not silently
//! lose targeting behavior during the server migration.

use std::collections::HashMap;

/// Per-request attributes used by targeted rules.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EvalContext {
    pub user_id: String,
    pub workspace_id: String,
    pub attributes: HashMap<String, String>,
}

impl EvalContext {
    pub fn lookup(&self, name: &str) -> Option<&str> {
        match name {
            "user_id" => (!self.user_id.is_empty()).then_some(self.user_id.as_str()),
            "workspace_id" => (!self.workspace_id.is_empty()).then_some(self.workspace_id.as_str()),
            _ => self
                .attributes
                .get(name)
                .map(String::as_str)
                .filter(|value| !value.is_empty()),
        }
    }
}

/// The context-aware result of evaluating a flag.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlagDecision {
    pub enabled: bool,
    pub variant: String,
}

fn bool_variant(enabled: bool) -> String {
    if enabled {
        "on".to_string()
    } else {
        "off".to_string()
    }
}

/// Evaluation seam — the boolean method remains source-compatible with the
/// existing Rust call sites, while targeted callers can use the context-aware
/// defaults below.
pub trait FlagSource: Send + Sync {
    fn is_enabled(&self, key: &str, default: bool) -> bool;

    fn decision_with_context(
        &self,
        key: &str,
        default: bool,
        _context: &EvalContext,
    ) -> FlagDecision {
        let enabled = self.is_enabled(key, default);
        FlagDecision {
            enabled,
            variant: bool_variant(enabled),
        }
    }

    fn is_enabled_with_context(&self, key: &str, default: bool, context: &EvalContext) -> bool {
        self.decision_with_context(key, default, context).enabled
    }

    fn variant_with_context(&self, key: &str, default: bool, context: &EvalContext) -> String {
        self.decision_with_context(key, default, context).variant
    }
}

/// A startup-loaded YAML rule set with live `FF_<KEY>` environment overrides.
/// The environment layer wins over the file layer, matching Go's provider
/// chain and preserving emergency kill switches.
#[derive(Debug, Default)]
pub struct ConfiguredFlags {
    rules: HashMap<String, Rule>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct Rule {
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub variant: String,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub allow_by: String,
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default)]
    pub deny_by: String,
    #[serde(default)]
    pub percent: Option<PercentRollout>,
}

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct PercentRollout {
    #[serde(default)]
    pub percent: i32,
    #[serde(default)]
    pub by: String,
}

impl ConfiguredFlags {
    pub fn from_env() -> Result<Self, anyhow::Error> {
        let path = std::env::var("CORDY_FEATURE_FLAGS_FILE")
            .unwrap_or_default()
            .trim()
            .to_string();
        if path.is_empty() {
            return Ok(Self::default());
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| anyhow::anyhow!("featureflag: read {path}: {error}"))?;
        if String::from_utf8_lossy(&bytes).trim().is_empty() {
            return Ok(Self::default());
        }
        let rules: HashMap<String, Rule> = serde_yaml::from_slice(&bytes)
            .map_err(|error| anyhow::anyhow!("featureflag: parse: {error}"))?;
        Ok(Self { rules })
    }

    fn env_name(key: &str) -> String {
        let mut output = String::from("FF_");
        let mut underscore = false;
        for character in key.chars() {
            if character.is_ascii_alphanumeric() {
                output.push(character.to_ascii_uppercase());
                underscore = false;
            } else if !underscore {
                output.push('_');
                underscore = true;
            }
        }
        output.trim_end_matches('_').to_string()
    }

    fn env_decision(key: &str, raw: &str) -> bool {
        Self::env_decision_with_context(key, raw, &EvalContext::default()).enabled
    }

    fn env_decision_with_context(key: &str, raw: &str, context: &EvalContext) -> FlagDecision {
        let value = raw.trim();
        match value.to_ascii_lowercase().as_str() {
            "" | "false" | "off" | "0" | "no" => FlagDecision {
                enabled: false,
                variant: "off".to_string(),
            },
            "true" | "on" | "1" | "yes" => FlagDecision {
                enabled: true,
                variant: "on".to_string(),
            },
            _ if value.ends_with('%') => {
                let Some(percent) = value[..value.len() - 1]
                    .trim()
                    .parse::<i32>()
                    .ok()
                    .filter(|percent| (0..=100).contains(percent))
                else {
                    return FlagDecision {
                        enabled: false,
                        variant: "off".to_string(),
                    };
                };
                let identifier = context.lookup("user_id").unwrap_or_default();
                let enabled = in_percent(key, identifier, percent);
                FlagDecision {
                    enabled,
                    variant: bool_variant(enabled),
                }
            }
            _ => FlagDecision {
                enabled: true,
                variant: value.to_string(),
            },
        }
    }

    fn rule_decision(&self, key: &str, default: bool, context: &EvalContext) -> FlagDecision {
        let Some(rule) = self.rules.get(key) else {
            return FlagDecision {
                enabled: default,
                variant: bool_variant(default),
            };
        };

        let deny_by = if rule.deny_by.is_empty() {
            "user_id"
        } else {
            rule.deny_by.as_str()
        };
        if rule
            .deny
            .iter()
            .any(|value| context.lookup(deny_by) == Some(value.as_str()))
        {
            return decision_for_rule(rule, false);
        }

        let allow_by = if rule.allow_by.is_empty() {
            "user_id"
        } else {
            rule.allow_by.as_str()
        };
        if rule
            .allow
            .iter()
            .any(|value| context.lookup(allow_by) == Some(value.as_str()))
        {
            return decision_for_rule(rule, true);
        }

        if let Some(percent) = rule.percent.as_ref() {
            let by = if percent.by.is_empty() {
                "user_id"
            } else {
                percent.by.as_str()
            };
            let enabled = in_percent(key, context.lookup(by).unwrap_or_default(), percent.percent);
            return decision_for_rule(rule, enabled);
        }
        decision_for_rule(rule, rule.default)
    }
}

fn decision_for_rule(rule: &Rule, enabled: bool) -> FlagDecision {
    FlagDecision {
        enabled,
        variant: if enabled && !rule.variant.is_empty() {
            rule.variant.clone()
        } else {
            bool_variant(enabled)
        },
    }
}

fn in_percent(key: &str, identifier: &str, percent: i32) -> bool {
    if percent <= 0 {
        return false;
    }
    if percent >= 100 {
        return true;
    }
    let mut hash = 2_166_136_261_u32;
    for byte in key
        .bytes()
        .chain(std::iter::once(0))
        .chain(identifier.bytes())
    {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash % 100 < percent as u32
}

impl FlagSource for ConfiguredFlags {
    fn is_enabled(&self, key: &str, default: bool) -> bool {
        if let Ok(value) = std::env::var(Self::env_name(key)) {
            return Self::env_decision(key, &value);
        }
        self.rule_decision(key, default, &EvalContext::default())
            .enabled
    }

    fn decision_with_context(
        &self,
        key: &str,
        default: bool,
        context: &EvalContext,
    ) -> FlagDecision {
        if let Ok(value) = std::env::var(Self::env_name(key)) {
            return Self::env_decision_with_context(key, &value, context);
        }
        self.rule_decision(key, default, context)
    }
}

impl<T: FlagSource + ?Sized> FlagSource for &T {
    fn is_enabled(&self, key: &str, default: bool) -> bool {
        (**self).is_enabled(key, default)
    }

    fn decision_with_context(
        &self,
        key: &str,
        default: bool,
        context: &EvalContext,
    ) -> FlagDecision {
        (**self).decision_with_context(key, default, context)
    }
}

pub const BILLING_WORKSPACE_SUBSCRIPTIONS: &str = "billing_workspace_subscriptions";
pub const COMPOSIO_MCP_APPS: &str = "composio_mcp_apps";
pub const PLUGINS_V1: &str = "plugins_v1";

/// Gates CREATING a custom issue status (MUL-6243) — a rollout gate, not a
/// behavior switch, deliberately one-way. Readers ship unconditionally (the
/// built-in keys behave identically); gating creation means a custom value
/// cannot come into existence until the whole fleet can read it. Once a
/// workspace has custom statuses, turning this off does NOT make existing
/// ones safe for an older binary.
pub const CUSTOM_ISSUE_STATUSES: &str = "custom_issue_statuses";

// No longer release flags — kept publishing as permanently enabled so older
// desktop clients that still gate on these config decisions fail open:
pub const AGENT_BUILDER_COMPAT: &str = "agents_agent_builder";
pub const AGENT_SKILL_TOGGLES_COMPAT: &str = "agents_skill_toggles";
pub const RESOURCE_LABELS_COMPAT: &str = "settings_resource_labels";

const FRONTEND_PUBLIC_FLAGS: &[&str] = &[
    BILLING_WORKSPACE_SUBSCRIPTIONS,
    COMPOSIO_MCP_APPS,
    PLUGINS_V1,
    // The settings UI needs this to decide whether to offer status creation
    // at all; without it the tab would show a "New status" button that 403s.
    CUSTOM_ISSUE_STATUSES,
];

pub fn billing_workspace_subscriptions_enabled(flags: &dyn FlagSource) -> bool {
    flags.is_enabled(BILLING_WORKSPACE_SUBSCRIPTIONS, false)
}

pub fn composio_mcp_apps_enabled(flags: &dyn FlagSource) -> bool {
    flags.is_enabled(COMPOSIO_MCP_APPS, false)
}

pub fn plugins_v1_enabled(flags: &dyn FlagSource) -> bool {
    flags.is_enabled(PLUGINS_V1, false)
}

/// Reports whether creating custom issue statuses is allowed. Default false:
/// a fleet mid-rollout must not be able to mint a status value its older pods
/// cannot interpret.
pub fn custom_issue_statuses_enabled(flags: &dyn FlagSource) -> bool {
    flags.is_enabled(CUSTOM_ISSUE_STATUSES, false)
}

/// Evaluates every flag the frontend may see, plus the three compat keys
/// forced to true.
pub fn evaluate_frontend_public_flags(
    flags: &dyn FlagSource,
) -> std::collections::HashMap<String, bool> {
    let mut out = std::collections::HashMap::with_capacity(FRONTEND_PUBLIC_FLAGS.len() + 3);
    for key in FRONTEND_PUBLIC_FLAGS {
        out.insert((*key).to_string(), flags.is_enabled(key, false));
    }
    out.insert(AGENT_BUILDER_COMPAT.to_string(), true);
    out.insert(AGENT_SKILL_TOGGLES_COMPAT.to_string(), true);
    out.insert(RESOURCE_LABELS_COMPAT.to_string(), true);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeFlags {
        enabled: Vec<&'static str>,
    }

    impl FakeFlags {
        fn new(enabled: &'static [&'static str]) -> Self {
            Self {
                enabled: enabled.to_vec(),
            }
        }
    }

    impl FlagSource for FakeFlags {
        fn is_enabled(&self, key: &str, _default: bool) -> bool {
            self.enabled.contains(&key)
        }
    }

    #[test]
    fn disabled_by_default() {
        let flags = FakeFlags::new(&[]);
        assert!(!billing_workspace_subscriptions_enabled(&flags));
        assert!(!composio_mcp_apps_enabled(&flags));
        assert!(!plugins_v1_enabled(&flags));
        // Rollout gate fails closed mid-fleet.
        assert!(!custom_issue_statuses_enabled(&flags));
    }

    #[test]
    fn enabled_keys_evaluate_true() {
        let flags = FakeFlags::new(&[COMPOSIO_MCP_APPS, CUSTOM_ISSUE_STATUSES]);
        assert!(composio_mcp_apps_enabled(&flags));
        assert!(custom_issue_statuses_enabled(&flags));
        assert!(!plugins_v1_enabled(&flags));
    }

    #[test]
    fn frontend_map_includes_public_plus_forced_compat() {
        let flags = FakeFlags::new(&[PLUGINS_V1]);
        let map = evaluate_frontend_public_flags(&flags);
        assert_eq!(map.len(), 7);
        assert!(map[PLUGINS_V1]);
        assert!(!map[BILLING_WORKSPACE_SUBSCRIPTIONS]);
        assert!(!map[CUSTOM_ISSUE_STATUSES]);
        // Compat keys are permanently true regardless of source state.
        assert!(map[AGENT_BUILDER_COMPAT]);
        assert!(map[AGENT_SKILL_TOGGLES_COMPAT]);
        assert!(map[RESOURCE_LABELS_COMPAT]);
    }

    #[test]
    fn configured_flags_project_boolean_env_values() {
        assert!(ConfiguredFlags::env_decision("flag", "yes"));
        assert!(ConfiguredFlags::env_decision("flag", "experiment-v2"));
        assert!(!ConfiguredFlags::env_decision("flag", "off"));
        assert!(!ConfiguredFlags::env_decision("flag", "invalid%"));
        assert_eq!(
            ConfiguredFlags::env_name("checkout.new-payment"),
            "FF_CHECKOUT_NEW_PAYMENT"
        );
    }

    #[test]
    fn configured_flags_apply_targeting_and_variants() {
        let mut rules = HashMap::new();
        rules.insert(
            "targeted".to_string(),
            Rule {
                default: false,
                variant: "experiment-v2".to_string(),
                allow: vec!["internal".to_string()],
                allow_by: "plan".to_string(),
                deny: vec!["blocked".to_string()],
                deny_by: "workspace_id".to_string(),
                percent: None,
            },
        );
        let flags = ConfiguredFlags { rules };

        let enabled = EvalContext {
            attributes: HashMap::from([(String::from("plan"), String::from("internal"))]),
            ..EvalContext::default()
        };
        let decision = flags.decision_with_context("targeted", false, &enabled);
        assert_eq!(decision.variant, "experiment-v2");
        assert!(decision.enabled);

        let denied = EvalContext {
            workspace_id: "blocked".to_string(),
            attributes: HashMap::from([(String::from("plan"), String::from("internal"))]),
            ..EvalContext::default()
        };
        let decision = flags.decision_with_context("targeted", false, &denied);
        assert_eq!(decision.variant, "off");
        assert!(!decision.enabled);
    }

    #[test]
    fn configured_flags_percent_rollout_uses_key_and_identifier() {
        let mut rules = HashMap::new();
        rules.insert(
            "workspace-rollout".to_string(),
            Rule {
                percent: Some(PercentRollout {
                    percent: 100,
                    by: "workspace_id".to_string(),
                }),
                ..Rule::default()
            },
        );
        let flags = ConfiguredFlags { rules };
        let context = EvalContext {
            workspace_id: "workspace-1".to_string(),
            ..EvalContext::default()
        };
        let decision = flags.decision_with_context("workspace-rollout", false, &context);
        assert!(decision.enabled);
        assert_eq!(decision.variant, "on");
    }
}
