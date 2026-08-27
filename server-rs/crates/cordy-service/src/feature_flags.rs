//! Feature-flag evaluation and key vocabulary — port of
//! `server/pkg/featureflag` and `server/internal/featureflags/keys.go`.
//!
//! The service keeps the provider contract separate from the boolean
//! [`FlagSource`] seam used by existing business services. That preserves the
//! current call sites while retaining Go's targeting, variants, diagnostics,
//! and provider-chain semantics for new callers.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use serde::Deserialize;

/// Minimal evaluation seam used by current business services.
pub trait FlagSource: Send + Sync {
    fn is_enabled(&self, key: &str, default: bool) -> bool;
}

/// Why a provider returned a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Reason {
    Static,
    Percent,
    Override,
    Default,
    Error,
}

/// Structured result of a feature-flag evaluation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Decision {
    pub key: String,
    pub enabled: bool,
    pub variant: String,
    pub reason: Reason,
    pub source: String,
}

/// Request attributes used by allow/deny rules and deterministic rollouts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvalContext {
    pub user_id: String,
    pub workspace_id: String,
    pub attributes: HashMap<String, String>,
}

impl EvalContext {
    /// Looks up a targeting attribute using Go's dedicated-field precedence.
    pub fn lookup(&self, name: &str) -> Option<&str> {
        match name {
            "user_id" => non_empty(&self.user_id),
            "workspace_id" => non_empty(&self.workspace_id),
            _ => self.attributes.get(name).and_then(|value| non_empty(value)),
        }
    }
}

/// A static rule loaded from source-controlled configuration or constructed
/// by an embedding application.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rule {
    pub default: bool,
    pub variant: String,
    pub allow: Vec<String>,
    pub allow_by: String,
    pub deny: Vec<String>,
    pub deny_by: String,
    pub percent: Option<PercentRollout>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PercentRollout {
    pub percent: i32,
    pub by: String,
}

/// A provider knows whether it can evaluate a key. `None` means not found and
/// allows a [`ChainProvider`] to fall through to the next source.
pub trait Provider: Send + Sync {
    fn lookup(&self, key: &str, context: &EvalContext) -> Option<Decision>;
    fn name(&self) -> &str;
}

/// Thread-safe in-memory rule provider.
pub struct StaticProvider {
    rules: RwLock<HashMap<String, Rule>>,
}

impl Default for StaticProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl StaticProvider {
    pub fn new() -> Self {
        Self {
            rules: RwLock::new(HashMap::new()),
        }
    }

    pub fn set(&self, key: impl Into<String>, rule: Rule) {
        let mut rules = match self.rules.write() {
            Ok(rules) => rules,
            Err(poisoned) => poisoned.into_inner(),
        };
        rules.insert(key.into(), rule);
    }

    /// Atomically replaces the complete rule set.
    pub fn load_rules(&self, rules: HashMap<String, Rule>) {
        let mut current = match self.rules.write() {
            Ok(rules) => rules,
            Err(poisoned) => poisoned.into_inner(),
        };
        *current = rules;
    }

    pub fn keys(&self) -> Vec<String> {
        let rules = match self.rules.read() {
            Ok(rules) => rules,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut keys = rules.keys().cloned().collect::<Vec<_>>();
        keys.sort_unstable();
        keys
    }

    fn rule(&self, key: &str) -> Option<Rule> {
        let rules = match self.rules.read() {
            Ok(rules) => rules,
            Err(poisoned) => poisoned.into_inner(),
        };
        rules.get(key).cloned()
    }
}

impl Provider for StaticProvider {
    fn lookup(&self, key: &str, context: &EvalContext) -> Option<Decision> {
        self.rule(key)
            .map(|rule| evaluate_rule(key, &rule, context))
    }

    fn name(&self) -> &str {
        "static"
    }
}

/// Environment provider used for emergency overrides and local development.
#[derive(Clone)]
pub struct EnvProvider {
    prefix: String,
    lookup: Arc<EnvLookup>,
}

type EnvLookup = dyn Fn(&str) -> Option<String> + Send + Sync;

impl EnvProvider {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            lookup: Arc::new(|name| std::env::var(name).ok()),
        }
    }

    #[cfg(test)]
    fn with_lookup<F>(prefix: impl Into<String>, lookup: F) -> Self
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        Self {
            prefix: prefix.into(),
            lookup: Arc::new(lookup),
        }
    }
}

impl Provider for EnvProvider {
    fn lookup(&self, key: &str, context: &EvalContext) -> Option<Decision> {
        let env_name = format!("{}{}", self.prefix, flag_key_to_env(key));
        let raw = (self.lookup)(&env_name)?;
        Some(env_decision(key, &raw, context))
    }

    fn name(&self) -> &str {
        "env"
    }
}

/// Ordered provider composition. The first provider that knows a key wins.
pub struct ChainProvider {
    providers: Vec<Arc<dyn Provider>>,
}

impl ChainProvider {
    pub fn new(providers: impl IntoIterator<Item = Arc<dyn Provider>>) -> Self {
        Self {
            providers: providers.into_iter().collect(),
        }
    }

    pub fn providers(&self) -> Vec<Arc<dyn Provider>> {
        self.providers.clone()
    }
}

impl Provider for ChainProvider {
    fn lookup(&self, key: &str, context: &EvalContext) -> Option<Decision> {
        self.providers
            .iter()
            .find_map(|provider| provider.lookup(key, context))
    }

    fn name(&self) -> &str {
        "chain"
    }
}

/// Framework-level toggle router. `None` is the Rust equivalent of Go's nil
/// `*Service`: every unknown flag returns the supplied default.
pub struct FeatureFlagService {
    provider: Option<Arc<dyn Provider>>,
}

impl Default for FeatureFlagService {
    fn default() -> Self {
        Self::new(None)
    }
}

impl FeatureFlagService {
    pub fn new(provider: Option<Arc<dyn Provider>>) -> Self {
        Self { provider }
    }

    pub fn provider(&self) -> Option<Arc<dyn Provider>> {
        self.provider.clone()
    }

    pub fn is_enabled(&self, key: &str, default: bool) -> bool {
        self.is_enabled_with_context(key, default, &EvalContext::default())
    }

    pub fn is_enabled_with_context(&self, key: &str, default: bool, context: &EvalContext) -> bool {
        self.decision_with_context(key, default, context).enabled
    }

    pub fn variant(&self, key: &str, default: &str) -> String {
        self.variant_with_context(key, default, &EvalContext::default())
    }

    pub fn variant_with_context(&self, key: &str, default: &str, context: &EvalContext) -> String {
        self.provider
            .as_ref()
            .and_then(|provider| provider.lookup(key, context))
            .map(|mut decision| {
                decision.key = key.to_string();
                decision.variant
            })
            .unwrap_or_else(|| default.to_string())
    }

    pub fn decision(&self, key: &str, default: bool) -> Decision {
        self.decision_with_context(key, default, &EvalContext::default())
    }

    pub fn decision_with_context(
        &self,
        key: &str,
        default: bool,
        context: &EvalContext,
    ) -> Decision {
        let Some(provider) = self.provider.as_ref() else {
            return default_decision(key, bool_to_variant(default), default);
        };
        let Some(mut decision) = provider.lookup(key, context) else {
            return default_decision(key, bool_to_variant(default), default);
        };
        decision.key = key.to_string();
        if decision.reason == Reason::Error {
            tracing::warn!(key, source = %decision.source, "feature flag provider returned an error decision");
        }
        decision
    }

    /// Builds the production provider chain: `FF_*` overrides YAML rules.
    pub fn from_env() -> anyhow::Result<Self> {
        let env_provider: Arc<dyn Provider> = Arc::new(EnvProvider::new(ENV_OVERRIDE_PREFIX));
        let path = std::env::var(ENV_FLAG_FILE)
            .unwrap_or_default()
            .trim()
            .to_string();
        let mut providers = vec![env_provider];
        let mut loaded_count = 0;
        if !path.is_empty() {
            let rules = load_rules_from_yaml_file(&path)?;
            loaded_count = rules.len();
            let static_provider = Arc::new(StaticProvider::new());
            static_provider.load_rules(rules);
            providers.push(static_provider);
        }
        tracing::info!(
            file = %path,
            rules = loaded_count,
            env_prefix = ENV_OVERRIDE_PREFIX,
            "feature flags initialised"
        );
        Ok(Self::new(Some(Arc::new(ChainProvider::new(providers)))))
    }

    #[cfg(test)]
    fn env_name(key: &str) -> String {
        flag_key_to_env(key)
    }

    #[cfg(test)]
    fn env_decision_for_test(key: &str, raw: &str) -> bool {
        env_decision(key, raw, &EvalContext::default()).enabled
    }
}

impl FlagSource for FeatureFlagService {
    fn is_enabled(&self, key: &str, default: bool) -> bool {
        FeatureFlagService::is_enabled(self, key, default)
    }
}

/// Kept as a source-compatible name for the earlier boolean-only port.
pub type ConfiguredFlags = FeatureFlagService;

const ENV_FLAG_FILE: &str = "CORDY_FEATURE_FLAGS_FILE";
const ENV_OVERRIDE_PREFIX: &str = "FF_";

#[derive(Debug, Deserialize)]
struct RuleConfig {
    #[serde(default)]
    default: Option<bool>,
    #[serde(default)]
    variant: String,
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    allow_by: String,
    #[serde(default)]
    deny: Vec<String>,
    #[serde(default)]
    deny_by: String,
    #[serde(default)]
    percent: Option<PercentConfig>,
}

#[derive(Debug, Deserialize)]
struct PercentConfig {
    #[serde(default)]
    percent: i32,
    #[serde(default)]
    by: String,
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

pub fn load_rules_from_yaml_file(path: impl AsRef<Path>) -> anyhow::Result<HashMap<String, Rule>> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)
        .map_err(|error| anyhow::anyhow!("featureflag: read {}: {error}", path.display()))?;
    parse_rules_yaml(&bytes)
}

fn parse_rules_yaml(bytes: &[u8]) -> anyhow::Result<HashMap<String, Rule>> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(HashMap::new());
    }
    let raw: HashMap<String, RuleConfig> = serde_yaml::from_slice(bytes)
        .map_err(|error| anyhow::anyhow!("featureflag: parse: {error}"))?;
    Ok(raw
        .into_iter()
        .map(|(key, config)| (key, config.into_rule()))
        .collect())
}

fn evaluate_rule(key: &str, rule: &Rule, context: &EvalContext) -> Decision {
    let deny_by = if rule.deny_by.is_empty() {
        "user_id"
    } else {
        &rule.deny_by
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
        &rule.allow_by
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
            &percent.by
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
    Decision {
        key: key.to_string(),
        enabled,
        variant: if enabled && !rule.variant.is_empty() {
            rule.variant.clone()
        } else {
            bool_to_variant(enabled)
        },
        reason,
        source: "static".to_string(),
    }
}

fn env_decision(key: &str, raw: &str, context: &EvalContext) -> Decision {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Decision {
            key: key.to_string(),
            enabled: false,
            variant: "off".to_string(),
            reason: Reason::Static,
            source: "env".to_string(),
        };
    }

    if let Some(percent_text) = trimmed.strip_suffix('%') {
        let Ok(percent) = percent_text.trim().parse::<i32>() else {
            return error_decision(key);
        };
        if !(0..=100).contains(&percent) {
            return error_decision(key);
        }
        let identifier = context.lookup("user_id").unwrap_or_default();
        let enabled = in_percent(key, identifier, percent);
        return Decision {
            key: key.to_string(),
            enabled,
            variant: bool_to_variant(enabled),
            reason: Reason::Percent,
            source: "env".to_string(),
        };
    }

    let lowered = trimmed.to_ascii_lowercase();
    let (enabled, variant) = match lowered.as_str() {
        "true" | "on" | "1" | "yes" => (true, "on".to_string()),
        "false" | "off" | "0" | "no" => (false, "off".to_string()),
        _ => (true, trimmed.to_string()),
    };
    Decision {
        key: key.to_string(),
        enabled,
        variant,
        reason: Reason::Static,
        source: "env".to_string(),
    }
}

fn error_decision(key: &str) -> Decision {
    Decision {
        key: key.to_string(),
        enabled: false,
        variant: "off".to_string(),
        reason: Reason::Error,
        source: "env".to_string(),
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

fn non_empty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn bool_to_variant(enabled: bool) -> String {
    if enabled {
        "on".to_string()
    } else {
        "off".to_string()
    }
}

fn default_decision(key: &str, variant: String, enabled: bool) -> Decision {
    Decision {
        key: key.to_string(),
        enabled,
        variant,
        reason: Reason::Default,
        source: "default".to_string(),
    }
}

pub const BILLING_WORKSPACE_SUBSCRIPTIONS: &str = "billing_workspace_subscriptions";
pub const COMPOSIO_MCP_APPS: &str = "composio_mcp_apps";
pub const PLUGINS_V1: &str = "plugins_v1";
pub const CUSTOM_ISSUE_STATUSES: &str = "custom_issue_statuses";

pub const AGENT_BUILDER_COMPAT: &str = "agents_agent_builder";
pub const AGENT_SKILL_TOGGLES_COMPAT: &str = "agents_skill_toggles";
pub const RESOURCE_LABELS_COMPAT: &str = "settings_resource_labels";

const FRONTEND_PUBLIC_FLAGS: &[&str] = &[
    BILLING_WORKSPACE_SUBSCRIPTIONS,
    COMPOSIO_MCP_APPS,
    PLUGINS_V1,
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
    let mut out = HashMap::with_capacity(FRONTEND_PUBLIC_FLAGS.len() + 3);
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

    fn context(user_id: &str) -> EvalContext {
        EvalContext {
            user_id: user_id.to_string(),
            ..EvalContext::default()
        }
    }

    fn provider<P: Provider + 'static>(provider: P) -> Arc<dyn Provider> {
        Arc::new(provider)
    }

    #[test]
    fn static_provider_enforces_deny_before_allow_and_percent() {
        let static_provider = StaticProvider::new();
        static_provider.set(
            "experiment",
            Rule {
                default: false,
                variant: "experiment-v2".to_string(),
                allow: vec!["internal".to_string()],
                deny: vec!["banned".to_string()],
                percent: Some(PercentRollout {
                    percent: 100,
                    by: String::new(),
                }),
                ..Rule::default()
            },
        );

        let denied = static_provider
            .lookup("experiment", &context("banned"))
            .expect("known flag");
        assert!(!denied.enabled);
        assert_eq!(denied.variant, "off");

        let allowed = static_provider
            .lookup("experiment", &context("internal"))
            .expect("known flag");
        assert!(allowed.enabled);
        assert_eq!(allowed.variant, "experiment-v2");
    }

    #[test]
    fn percent_rollout_uses_stable_cross_language_fnv_bucket() {
        assert_eq!(bucket_for("billing_new_invoice", "user-42"), 97);
        assert_eq!(bucket_for("feature_a", "user-1"), 50);
        assert_eq!(bucket_for("flag", "é"), 53);
        assert_eq!(bucket_for("flag", "🦄"), 82);
        assert!(in_percent("feature_a", "user-1", 51));
        assert!(!in_percent("feature_a", "user-1", 50));
    }

    #[test]
    fn env_provider_distinguishes_missing_empty_and_variant_values() {
        let values = Arc::new(std::sync::Mutex::new(HashMap::from([
            ("FF_EMPTY".to_string(), "".to_string()),
            ("FF_VARIANT".to_string(), "experiment-v2".to_string()),
        ])));
        let lookup_values = Arc::clone(&values);
        let env = EnvProvider::with_lookup("FF_", move |name| {
            lookup_values
                .lock()
                .ok()
                .and_then(|values| values.get(name).cloned())
        });
        let empty = env.lookup("empty", &EvalContext::default()).expect("set");
        assert!(!empty.enabled);
        assert_eq!(empty.variant, "off");
        let variant = env.lookup("variant", &EvalContext::default()).expect("set");
        assert!(variant.enabled);
        assert_eq!(variant.variant, "experiment-v2");
        assert!(env.lookup("missing", &EvalContext::default()).is_none());
    }

    #[test]
    fn malformed_env_percent_is_an_error_decision() {
        let env = EnvProvider::with_lookup("FF_", |name| {
            (name == "FF_DEMO").then(|| "150%".to_string())
        });
        let decision = env
            .lookup("demo", &EvalContext::default())
            .expect("malformed values still match");
        assert_eq!(decision.reason, Reason::Error);
        assert!(!decision.enabled);
    }

    #[test]
    fn provider_chain_gives_env_override_precedence() {
        let static_provider = StaticProvider::new();
        static_provider.set(
            "kill_switch",
            Rule {
                default: true,
                ..Rule::default()
            },
        );
        let env = EnvProvider::with_lookup("FF_", |name| {
            (name == "FF_KILL_SWITCH").then(|| "false".to_string())
        });
        let service = FeatureFlagService::new(Some(provider(ChainProvider::new([
            provider(env),
            provider(static_provider),
        ]))));
        assert!(!service.is_enabled("kill_switch", true));
        assert_eq!(service.decision("kill_switch", true).source, "env");
    }

    #[test]
    fn yaml_loader_preserves_full_rule_shape_and_empty_file() {
        let yaml = br#"
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
        let rules = parse_rules_yaml(yaml).expect("valid YAML");
        let rule = rules.get("checkout_algo").expect("rule");
        assert_eq!(rule.variant, "experiment-v2");
        assert_eq!(rule.allow, ["user-internal"]);
        assert_eq!(rule.deny_by, "workspace_id");
        assert_eq!(rule.percent.as_ref().map(|value| value.percent), Some(25));
        assert!(parse_rules_yaml(b" \n\t").expect("empty YAML").is_empty());
    }

    #[test]
    fn service_returns_variant_and_default_decisions() {
        let static_provider = StaticProvider::new();
        static_provider.set(
            "checkout_algo",
            Rule {
                default: true,
                variant: "experiment-v2".to_string(),
                ..Rule::default()
            },
        );
        let service = FeatureFlagService::new(Some(provider(static_provider)));
        assert_eq!(service.variant("checkout_algo", "control"), "experiment-v2");
        assert_eq!(service.variant("unknown", "control"), "control");
        let decision = service.decision("unknown", true);
        assert_eq!(decision.reason, Reason::Default);
        assert_eq!(decision.variant, "on");
    }

    #[test]
    fn frontend_public_flags_keep_compatibility_keys_enabled() {
        let service = FeatureFlagService::default();
        let flags = evaluate_frontend_public_flags(&service);
        assert_eq!(flags.len(), 7);
        assert!(flags[AGENT_BUILDER_COMPAT]);
        assert!(flags[AGENT_SKILL_TOGGLES_COMPAT]);
        assert!(flags[RESOURCE_LABELS_COMPAT]);
    }

    #[test]
    fn env_key_normalization_matches_go() {
        assert_eq!(
            FeatureFlagService::env_name("checkout.newPayment"),
            "CHECKOUT_NEWPAYMENT"
        );
        assert!(FeatureFlagService::env_decision_for_test("flag", "yes"));
        assert!(FeatureFlagService::env_decision_for_test(
            "flag",
            "experiment-v2"
        ));
        assert!(!FeatureFlagService::env_decision_for_test(
            "flag", "invalid%"
        ));
    }
}
