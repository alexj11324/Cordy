//! Feature-flag evaluation and key vocabulary — port of
//! server/pkg/featureflag and server/internal/featureflags/keys.go.
//!
//! The production configuration is layered as FF_<KEY> environment
//! overrides over the YAML file at CORDY_FEATURE_FLAGS_FILE, matching the
//! Go service. The boolean FlagSource trait remains the small seam used
//! by existing Rust handlers; the richer provider/service types are available
//! to new call sites that need decisions, variants, or request targeting.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

use serde::Deserialize;

/// Why a flag decision has its value. The string representation is stable so
/// callers can put it in diagnostics and structured logs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    Static,
    Percent,
    Override,
    Default,
    Error,
}

impl Reason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Percent => "percent",
            Self::Override => "override",
            Self::Default => "default",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Structured result of evaluating one feature flag.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decision {
    pub key: String,
    pub enabled: bool,
    pub variant: String,
    pub reason: Reason,
    pub source: String,
}

/// A per-request evaluation context used by allow/deny lists and percentage
/// rollouts. Missing values are intentionally different from empty values:
/// both are treated as not found, matching the Go implementation.
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

/// Configuration backend for the feature-flag service.
///
/// None means the provider does not know the key. A provider that attempted
/// evaluation but found malformed configuration returns Reason::Error in a
/// decision instead, so a higher-priority bad override cannot silently fall
/// through to a lower-priority value.
pub trait FlagProvider: Send + Sync {
    fn lookup(&self, key: &str, context: &EvalContext) -> Option<Decision>;
    fn name(&self) -> &str;
}

/// Minimal boolean seam retained for existing handler/service call sites.
/// Implementations that need the complete contract can override
/// decision_with_context; old boolean-only implementations remain source
/// compatible through the default method.
pub trait FlagSource: Send + Sync {
    fn is_enabled(&self, key: &str, default: bool) -> bool;

    fn decision_with_context(&self, key: &str, default: bool, _context: &EvalContext) -> Decision {
        let enabled = self.is_enabled(key, default);
        Decision {
            key: key.to_string(),
            enabled,
            variant: bool_to_variant(enabled).to_string(),
            reason: Reason::Static,
            source: "compat".to_string(),
        }
    }
}

impl<T: FlagSource + ?Sized> FlagSource for &T {
    fn is_enabled(&self, key: &str, default: bool) -> bool {
        (**self).is_enabled(key, default)
    }

    fn decision_with_context(&self, key: &str, default: bool, context: &EvalContext) -> Decision {
        (**self).decision_with_context(key, default, context)
    }
}

/// Rule for one statically configured flag.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Rule {
    pub default: bool,
    pub variant: String,
    pub allow: Vec<String>,
    pub allow_by: String,
    pub deny: Vec<String>,
    pub deny_by: String,
    pub percent: Option<PercentRollout>,
}

/// Deterministic percentage rollout configuration.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PercentRollout {
    pub percent: i32,
    pub by: String,
}

/// Thread-safe in-memory provider used by YAML configuration and tests.
#[derive(Default)]
pub struct StaticProvider {
    rules: RwLock<HashMap<String, Rule>>,
}

impl StaticProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, key: impl Into<String>, rule: Rule) {
        let mut rules = self
            .rules
            .write()
            .unwrap_or_else(|error| error.into_inner());
        rules.insert(key.into(), rule);
    }

    /// Atomically replaces all rules, avoiding a mixed old/new configuration
    /// during a reload.
    pub fn load_rules(&self, rules: HashMap<String, Rule>) {
        let mut replacement = HashMap::with_capacity(rules.len());
        replacement.extend(rules);
        let mut current = self
            .rules
            .write()
            .unwrap_or_else(|error| error.into_inner());
        *current = replacement;
    }

    pub fn keys(&self) -> Vec<String> {
        let rules = self.rules.read().unwrap_or_else(|error| error.into_inner());
        let mut keys: Vec<_> = rules.keys().cloned().collect();
        keys.sort();
        keys
    }
}

impl FlagProvider for StaticProvider {
    fn lookup(&self, key: &str, context: &EvalContext) -> Option<Decision> {
        let rule = {
            let rules = self.rules.read().unwrap_or_else(|error| error.into_inner());
            rules.get(key).cloned()
        }?;
        Some(evaluate_rule(key, &rule, context))
    }

    fn name(&self) -> &str {
        "static"
    }
}

/// Environment provider for emergency overrides and local development.
pub struct EnvProvider {
    prefix: String,
    lookup: Arc<EnvLookup>,
}

type EnvLookup = dyn Fn(&str) -> Option<String> + Send + Sync;

impl EnvProvider {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self::with_lookup(prefix, |name| std::env::var(name).ok())
    }

    /// Constructor seam for deterministic tests and embedders that do not
    /// want to read the process environment directly.
    pub fn with_lookup<F>(prefix: impl Into<String>, lookup: F) -> Self
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        Self {
            prefix: prefix.into(),
            lookup: Arc::new(lookup),
        }
    }

    pub fn evaluate(&self, key: &str, context: &EvalContext) -> Option<Decision> {
        let name = format!("{}{}", self.prefix, flag_key_to_env(key));
        let raw = (self.lookup)(&name)?;
        Some(decision_from_env(key, &raw, context))
    }
}

impl FlagProvider for EnvProvider {
    fn lookup(&self, key: &str, context: &EvalContext) -> Option<Decision> {
        self.evaluate(key, context)
    }

    fn name(&self) -> &str {
        "env"
    }
}

/// First-match provider composition. Providers should be ordered from most
/// specific to most generic, normally env overrides before static YAML.
pub struct ChainProvider {
    providers: Vec<Arc<dyn FlagProvider>>,
}

impl ChainProvider {
    pub fn new(providers: Vec<Arc<dyn FlagProvider>>) -> Self {
        Self { providers }
    }

    pub fn providers(&self) -> Vec<Arc<dyn FlagProvider>> {
        self.providers.clone()
    }
}

impl FlagProvider for ChainProvider {
    fn lookup(&self, key: &str, context: &EvalContext) -> Option<Decision> {
        self.providers
            .iter()
            .find_map(|provider| provider.lookup(key, context))
    }

    fn name(&self) -> &str {
        "chain"
    }
}

/// Toggle Router equivalent to Go's featureflag.Service.
#[derive(Default)]
pub struct FlagService {
    provider: Option<Arc<dyn FlagProvider>>,
}

impl FlagService {
    pub fn new(provider: Option<Arc<dyn FlagProvider>>) -> Self {
        Self { provider }
    }

    pub fn from_provider<P: FlagProvider + 'static>(provider: P) -> Self {
        Self::new(Some(Arc::new(provider)))
    }

    pub fn provider(&self) -> Option<Arc<dyn FlagProvider>> {
        self.provider.clone()
    }

    pub fn decision(&self, key: &str, default: bool, context: &EvalContext) -> Decision {
        let Some(provider) = &self.provider else {
            return default_decision(key, bool_to_variant(default), default);
        };
        let Some(mut decision) = provider.lookup(key, context) else {
            return default_decision(key, bool_to_variant(default), default);
        };
        if decision.reason == Reason::Error {
            tracing::warn!(key, source = %decision.source, "feature flag provider returned an error decision");
        }
        decision.key = key.to_string();
        decision
    }

    pub fn variant(&self, key: &str, default: &str, context: &EvalContext) -> String {
        let Some(provider) = &self.provider else {
            return default.to_string();
        };
        provider
            .lookup(key, context)
            .map(|decision| decision.variant)
            .unwrap_or_else(|| default.to_string())
    }
}

impl FlagSource for FlagService {
    fn is_enabled(&self, key: &str, default: bool) -> bool {
        self.decision(key, default, &EvalContext::default()).enabled
    }

    fn decision_with_context(&self, key: &str, default: bool, context: &EvalContext) -> Decision {
        self.decision(key, default, context)
    }
}

/// Production configuration provider: FF_<KEY> overrides the YAML rules
/// loaded from CORDY_FEATURE_FLAGS_FILE.
pub struct ConfiguredFlags {
    rules: RwLock<HashMap<String, Rule>>,
    env: EnvProvider,
}

impl Default for ConfiguredFlags {
    fn default() -> Self {
        Self {
            rules: RwLock::new(HashMap::new()),
            env: EnvProvider::new("FF_"),
        }
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct RuleConfig {
    #[serde(default)]
    default: Option<bool>,
    #[serde(default, deserialize_with = "null_default")]
    variant: String,
    #[serde(default, deserialize_with = "null_default")]
    allow: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    allow_by: String,
    #[serde(default, deserialize_with = "null_default")]
    deny: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    deny_by: String,
    #[serde(default)]
    percent: Option<PercentConfig>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct PercentConfig {
    #[serde(default, deserialize_with = "null_default")]
    percent: i32,
    #[serde(default, deserialize_with = "null_default")]
    by: String,
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

impl RuleConfig {
    fn into_rule(self) -> Rule {
        Rule {
            default: self.default.unwrap_or(false),
            variant: self.variant,
            allow: self.allow,
            allow_by: self.allow_by,
            deny: self.deny,
            deny_by: self.deny_by,
            percent: self.percent.map(|percent| PercentRollout {
                percent: percent.percent,
                by: percent.by,
            }),
        }
    }
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
        let raw: Option<HashMap<String, Option<RuleConfig>>> = serde_yaml::from_slice(&bytes)
            .map_err(|error| anyhow::anyhow!("featureflag: parse: {error}"))?;
        let rules = raw
            .unwrap_or_default()
            .into_iter()
            .map(|(key, rule)| (key, rule.unwrap_or_default().into_rule()))
            .collect();
        let configured = Self::default();
        configured.load_rules(rules);
        Ok(configured)
    }

    pub fn load_rules(&self, rules: HashMap<String, Rule>) {
        let mut replacement = HashMap::with_capacity(rules.len());
        replacement.extend(rules);
        let mut current = self
            .rules
            .write()
            .unwrap_or_else(|error| error.into_inner());
        *current = replacement;
    }

    pub fn set(&self, key: impl Into<String>, rule: Rule) {
        let mut rules = self
            .rules
            .write()
            .unwrap_or_else(|error| error.into_inner());
        rules.insert(key.into(), rule);
    }

    pub fn keys(&self) -> Vec<String> {
        let rules = self.rules.read().unwrap_or_else(|error| error.into_inner());
        let mut keys: Vec<_> = rules.keys().cloned().collect();
        keys.sort();
        keys
    }

    #[cfg(test)]
    fn env_name(key: &str) -> String {
        format!("FF_{}", flag_key_to_env(key))
    }

    #[cfg(test)]
    fn env_decision(key: &str, raw: &str) -> bool {
        decision_from_env(key, raw, &EvalContext::default()).enabled
    }
}

impl FlagProvider for ConfiguredFlags {
    fn lookup(&self, key: &str, context: &EvalContext) -> Option<Decision> {
        if let Some(decision) = self.env.evaluate(key, context) {
            return Some(decision);
        }
        let rule = {
            let rules = self.rules.read().unwrap_or_else(|error| error.into_inner());
            rules.get(key).cloned()
        }?;
        Some(evaluate_rule(key, &rule, context))
    }

    fn name(&self) -> &str {
        "configured"
    }
}

impl FlagSource for ConfiguredFlags {
    fn is_enabled(&self, key: &str, default: bool) -> bool {
        self.decision_with_context(key, default, &EvalContext::default())
            .enabled
    }

    fn decision_with_context(&self, key: &str, default: bool, context: &EvalContext) -> Decision {
        FlagProvider::lookup(self, key, context)
            .unwrap_or_else(|| default_decision(key, bool_to_variant(default), default))
    }
}

fn evaluate_rule(key: &str, rule: &Rule, context: &EvalContext) -> Decision {
    let deny_by = if rule.deny_by.is_empty() {
        "user_id"
    } else {
        rule.deny_by.as_str()
    };
    if !rule.deny.is_empty()
        && context
            .lookup(deny_by)
            .is_some_and(|value| rule.deny.iter().any(|candidate| candidate == value))
    {
        return decision_from_rule(key, rule, false, Reason::Static);
    }

    let allow_by = if rule.allow_by.is_empty() {
        "user_id"
    } else {
        rule.allow_by.as_str()
    };
    if !rule.allow.is_empty()
        && context
            .lookup(allow_by)
            .is_some_and(|value| rule.allow.iter().any(|candidate| candidate == value))
    {
        return decision_from_rule(key, rule, true, Reason::Static);
    }

    if let Some(percent) = &rule.percent {
        let by = if percent.by.is_empty() {
            "user_id"
        } else {
            percent.by.as_str()
        };
        let identifier = context.lookup(by).unwrap_or_default();
        return decision_from_rule(
            key,
            rule,
            in_percent(key, identifier, percent.percent),
            Reason::Percent,
        );
    }

    decision_from_rule(key, rule, rule.default, Reason::Static)
}

fn decision_from_rule(key: &str, rule: &Rule, enabled: bool, reason: Reason) -> Decision {
    let variant = if enabled && !rule.variant.is_empty() {
        rule.variant.clone()
    } else {
        bool_to_variant(enabled).to_string()
    };
    Decision {
        key: key.to_string(),
        enabled,
        variant,
        reason,
        source: "static".to_string(),
    }
}

fn decision_from_env(key: &str, raw: &str, context: &EvalContext) -> Decision {
    let value = raw.trim();
    if value.is_empty() {
        return env_decision(key, false, "off", Reason::Static);
    }

    if let Some(percent_value) = value.strip_suffix('%') {
        let parsed = percent_value.trim().parse::<i32>();
        let Ok(percent) = parsed else {
            return env_decision(key, false, "off", Reason::Error);
        };
        if !(0..=100).contains(&percent) {
            return env_decision(key, false, "off", Reason::Error);
        }
        let identifier = context.lookup("user_id").unwrap_or_default();
        let enabled = in_percent(key, identifier, percent);
        return env_decision(key, enabled, bool_to_variant(enabled), Reason::Percent);
    }

    let lowered = value.to_ascii_lowercase();
    match lowered.as_str() {
        "true" | "on" | "1" | "yes" => env_decision(key, true, "on", Reason::Static),
        "false" | "off" | "0" | "no" => env_decision(key, false, "off", Reason::Static),
        _ => env_decision(key, true, value, Reason::Static),
    }
}

fn env_decision(key: &str, enabled: bool, variant: &str, reason: Reason) -> Decision {
    Decision {
        key: key.to_string(),
        enabled,
        variant: variant.to_string(),
        reason,
        source: "env".to_string(),
    }
}

fn default_decision(key: &str, variant: &str, enabled: bool) -> Decision {
    Decision {
        key: key.to_string(),
        enabled,
        variant: variant.to_string(),
        reason: Reason::Default,
        source: "default".to_string(),
    }
}

fn bool_to_variant(enabled: bool) -> &'static str {
    if enabled {
        "on"
    } else {
        "off"
    }
}

fn bucket_for(key: &str, identifier: &str) -> u32 {
    let mut hash = 2_166_136_261_u32;
    for byte in key
        .bytes()
        .chain(std::iter::once(0))
        .chain(identifier.bytes())
    {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash % 100
}

fn in_percent(key: &str, identifier: &str, percent: i32) -> bool {
    match percent {
        value if value <= 0 => false,
        value if value >= 100 => true,
        value => bucket_for(key, identifier) < value as u32,
    }
}

fn flag_key_to_env(key: &str) -> String {
    let mut output = String::with_capacity(key.len());
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
    output.trim_matches('_').to_string()
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

pub fn custom_issue_statuses_enabled(flags: &dyn FlagSource) -> bool {
    flags.is_enabled(CUSTOM_ISSUE_STATUSES, false)
}

pub fn evaluate_frontend_public_flags(flags: &dyn FlagSource) -> HashMap<String, bool> {
    evaluate_frontend_public_flags_with_context(flags, &EvalContext::default())
}

pub fn evaluate_frontend_public_flags_with_context(
    flags: &dyn FlagSource,
    context: &EvalContext,
) -> HashMap<String, bool> {
    let mut out = HashMap::with_capacity(FRONTEND_PUBLIC_FLAGS.len() + 3);
    for key in FRONTEND_PUBLIC_FLAGS {
        out.insert(
            (*key).to_string(),
            flags.decision_with_context(key, false, context).enabled,
        );
    }
    out.insert(AGENT_BUILDER_COMPAT.to_string(), true);
    out.insert(AGENT_SKILL_TOGGLES_COMPAT.to_string(), true);
    out.insert(RESOURCE_LABELS_COMPAT.to_string(), true);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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

    fn mock_env(values: &[(&str, &str)]) -> EnvProvider {
        let values = values
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<HashMap<_, _>>();
        EnvProvider::with_lookup("FF_", move |name| values.get(name).cloned())
    }

    #[test]
    fn disabled_by_default() {
        let flags = FakeFlags::new(&[]);
        assert!(!billing_workspace_subscriptions_enabled(&flags));
        assert!(!composio_mcp_apps_enabled(&flags));
        assert!(!plugins_v1_enabled(&flags));
        assert!(!custom_issue_statuses_enabled(&flags));
    }

    #[test]
    fn frontend_map_includes_public_plus_forced_compat() {
        let flags = FakeFlags::new(&[PLUGINS_V1]);
        let map = evaluate_frontend_public_flags(&flags);
        assert_eq!(map.len(), 7);
        assert!(map[PLUGINS_V1]);
        assert!(!map[BILLING_WORKSPACE_SUBSCRIPTIONS]);
        assert!(!map[CUSTOM_ISSUE_STATUSES]);
        assert!(map[AGENT_BUILDER_COMPAT]);
        assert!(map[AGENT_SKILL_TOGGLES_COMPAT]);
        assert!(map[RESOURCE_LABELS_COMPAT]);
    }

    #[test]
    fn static_provider_applies_deny_allow_percent_and_variant_order() {
        let provider = StaticProvider::new();
        provider.set(
            "experiment",
            Rule {
                default: false,
                variant: "experiment-v2".to_string(),
                allow: vec!["internal".to_string()],
                deny: vec!["banned".to_string()],
                percent: Some(PercentRollout {
                    percent: 100,
                    by: "workspace_id".to_string(),
                }),
                ..Rule::default()
            },
        );

        let denied = EvalContext {
            user_id: "banned".to_string(),
            workspace_id: "workspace".to_string(),
            ..EvalContext::default()
        };
        assert_eq!(
            provider.lookup("experiment", &denied).unwrap().variant,
            "off"
        );

        let allowed = EvalContext {
            user_id: "internal".to_string(),
            ..denied.clone()
        };
        let decision = provider.lookup("experiment", &allowed).unwrap();
        assert!(decision.enabled);
        assert_eq!(decision.variant, "experiment-v2");
        assert_eq!(decision.reason, Reason::Static);
    }

    #[test]
    fn static_provider_percent_uses_context_and_fnv_golden_values() {
        let provider = StaticProvider::new();
        provider.set(
            "billing_new_invoice",
            Rule {
                percent: Some(PercentRollout {
                    percent: 98,
                    ..PercentRollout::default()
                }),
                ..Rule::default()
            },
        );
        let context = EvalContext {
            user_id: "user-42".to_string(),
            ..EvalContext::default()
        };
        let decision = provider.lookup("billing_new_invoice", &context).unwrap();
        assert!(decision.enabled);
        assert_eq!(bucket_for("billing_new_invoice", "user-42"), 97);
        assert_eq!(bucket_for("feature_a", "user-1"), 50);
        assert_eq!(bucket_for("checkout_algo", "u-7f8a"), 11);
        assert_eq!(bucket_for("ws_rollout", "workspace-1"), 62);
        assert_eq!(bucket_for("empty_id_flag", ""), 83);
        assert_eq!(bucket_for("flag", "é"), 53);
        assert_eq!(bucket_for("flag", "🦄"), 82);
        assert_eq!(bucket_for("实验", "user-1"), 90);
        assert_eq!(bucket_for("flag", "用户-1"), 95);
        assert_eq!(bucket_for("checkout_算法", "user-100"), 79);
    }

    #[test]
    fn yaml_rule_shape_preserves_targeting_and_variant_fields() {
        let yaml = r#"
checkout_algo:
  default: false
  variant: experiment-v2
  allow: [user-internal]
  allow_by: user_id
  deny: [banned-tenant]
  deny_by: workspace_id
  percent:
    percent: 25
    by: user_id
"#;
        let raw: HashMap<String, RuleConfig> = serde_yaml::from_str(yaml).unwrap();
        let rule = raw.into_iter().next().unwrap().1.into_rule();
        assert!(!rule.default);
        assert_eq!(rule.variant, "experiment-v2");
        assert_eq!(rule.allow, ["user-internal"]);
        assert_eq!(rule.allow_by, "user_id");
        assert_eq!(rule.deny, ["banned-tenant"]);
        assert_eq!(rule.deny_by, "workspace_id");
        assert_eq!(rule.percent.unwrap().percent, 25);
    }

    #[test]
    fn yaml_nulls_preserve_go_zero_value_compatibility() {
        let yaml = r#"
null_rule:
  default: null
  variant: null
  allow: null
  allow_by: null
  deny: null
  deny_by: null
  percent:
    percent: null
    by: null
empty_rule: null
"#;
        let raw: Option<HashMap<String, Option<RuleConfig>>> = serde_yaml::from_str(yaml).unwrap();
        let rules: HashMap<_, _> = raw
            .unwrap_or_default()
            .into_iter()
            .map(|(key, rule)| (key, rule.unwrap_or_default().into_rule()))
            .collect();

        assert_eq!(
            rules["null_rule"],
            Rule {
                percent: Some(PercentRollout::default()),
                ..Rule::default()
            }
        );
        assert_eq!(rules["empty_rule"], Rule::default());
        assert!(
            serde_yaml::from_str::<Option<HashMap<String, Option<RuleConfig>>>>("null")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn env_provider_supports_booleans_variants_percent_and_errors() {
        let context = EvalContext {
            user_id: "anyone".to_string(),
            ..EvalContext::default()
        };
        let provider = mock_env(&[("FF_DEMO", "yes")]);
        assert_eq!(provider.lookup("demo", &context).unwrap().variant, "on");

        let provider = mock_env(&[("FF_ALGO", "experiment-v2")]);
        let variant = provider.lookup("algo", &context).unwrap();
        assert!(variant.enabled);
        assert_eq!(variant.variant, "experiment-v2");

        let provider = mock_env(&[("FF_DEMO", "100%")]);
        assert!(provider.lookup("demo", &context).unwrap().enabled);

        let provider = mock_env(&[("FF_DEMO", "abc%")]);
        let decision = provider.lookup("demo", &context).unwrap();
        assert_eq!(decision.reason, Reason::Error);
        assert!(!decision.enabled);

        let provider = mock_env(&[("FF_DEMO", "")]);
        assert!(!provider.lookup("demo", &context).unwrap().enabled);
    }

    #[test]
    fn configured_flags_layers_env_over_yaml() {
        let configured = ConfiguredFlags::default();
        configured.set(
            "demo_flag",
            Rule {
                default: true,
                ..Rule::default()
            },
        );
        let env = mock_env(&[("FF_DEMO_FLAG", "false")]);
        let chain = ChainProvider::new(vec![Arc::new(env), Arc::new(configured)]);
        let service = FlagService::from_provider(chain);
        let decision = service.decision("demo_flag", true, &EvalContext::default());
        assert!(!decision.enabled);
        assert_eq!(decision.source, "env");
    }

    #[test]
    fn configured_flags_env_name_matches_go_normalization() {
        assert_eq!(
            ConfiguredFlags::env_name("checkout.new-payment"),
            "FF_CHECKOUT_NEW_PAYMENT"
        );
        assert_eq!(flag_key_to_env("  weird  spaces  "), "WEIRD_SPACES");
        assert!(ConfiguredFlags::env_decision("flag", "yes"));
        assert!(!ConfiguredFlags::env_decision("flag", "invalid%"));
    }

    #[test]
    fn chain_first_hit_wins_and_service_defaults_missing_keys() {
        let first = StaticProvider::new();
        first.set(
            "shared",
            Rule {
                default: true,
                ..Rule::default()
            },
        );
        let second = StaticProvider::new();
        second.set("shared", Rule::default());
        let service =
            FlagService::from_provider(ChainProvider::new(vec![Arc::new(first), Arc::new(second)]));
        assert!(service.is_enabled("shared", false));
        let missing = service.decision("missing", true, &EvalContext::default());
        assert_eq!(missing.reason, Reason::Default);
        assert!(missing.enabled);
        assert_eq!(missing.variant, "on");
        assert_eq!(
            service.variant("missing", "control", &EvalContext::default()),
            "control"
        );
    }
}
