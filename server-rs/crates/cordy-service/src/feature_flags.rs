//! Feature flags — port of `server/pkg/featureflag` and
//! `server/internal/featureflags`.
//!
//! The public Rust server previously exposed only a boolean `FlagSource` and
//! a reduced YAML reader. This module keeps that small seam for existing
//! callers while providing the complete Go contract: structured decisions,
//! request attributes, static/chain/environment providers, deterministic
//! percentage rollouts, and the full YAML rule shape.

use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

/// Minimal evaluation seam used by current Rust handlers.
///
/// The context-aware default method lets existing boolean-only providers keep
/// compiling while a full [`Service`] can evaluate targeting attributes.
pub trait FlagSource: Send + Sync {
    fn is_enabled(&self, key: &str, default: bool) -> bool;

    fn is_enabled_with_context(&self, key: &str, _context: &EvalContext, default: bool) -> bool {
        self.is_enabled(key, default)
    }
}

/// Why a provider returned a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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

impl std::fmt::Display for Reason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Structured result of a flag evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub key: String,
    pub enabled: bool,
    pub variant: String,
    pub reason: Reason,
    pub source: String,
}

/// Per-request values used by allow/deny targeting and percentage rollouts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvalContext {
    pub user_id: String,
    pub workspace_id: String,
    pub attributes: HashMap<String, String>,
}

impl EvalContext {
    /// Looks up a non-empty targeting value. `user_id` and `workspace_id`
    /// use their dedicated fields, matching the Go context contract.
    pub fn lookup(&self, name: &str) -> Option<&str> {
        match name {
            "user_id" => non_empty(&self.user_id),
            "workspace_id" => non_empty(&self.workspace_id),
            _ => self.attributes.get(name).and_then(|value| non_empty(value)),
        }
    }
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

/// Configuration backend for [`Service`].
pub trait Provider: Send + Sync {
    fn lookup(&self, key: &str, context: &EvalContext) -> Option<Decision>;
    fn name(&self) -> &str;
}

/// Toggle router. Business code should depend on this type (or
/// [`FlagSource`]), not on a concrete provider.
pub struct Service {
    provider: Option<Arc<dyn Provider>>,
}

impl Default for Service {
    fn default() -> Self {
        Self { provider: None }
    }
}

impl Service {
    pub fn new(provider: Option<Arc<dyn Provider>>) -> Self {
        Self { provider }
    }

    pub fn from_provider<P>(provider: P) -> Self
    where
        P: Provider + 'static,
    {
        Self::new(Some(Arc::new(provider)))
    }

    pub fn is_enabled(&self, key: &str, default: bool) -> bool {
        self.is_enabled_with_context(key, &EvalContext::default(), default)
    }

    pub fn is_enabled_with_context(&self, key: &str, context: &EvalContext, default: bool) -> bool {
        self.decision_with_context(key, default, context).enabled
    }

    pub fn variant(&self, key: &str, default: &str) -> String {
        self.variant_with_context(key, default, &EvalContext::default())
    }

    pub fn variant_with_context(&self, key: &str, default: &str, context: &EvalContext) -> String {
        self.decision_with_variant_default(key, default, context)
            .variant
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
        let default_variant = bool_to_variant(default);
        self.decision_with_variant_default(key, &default_variant, context)
    }

    fn decision_with_variant_default(
        &self,
        key: &str,
        default_variant: &str,
        context: &EvalContext,
    ) -> Decision {
        let default_decision =
            default_decision(key, default_variant, variant_enabled(default_variant));
        let Some(provider) = &self.provider else {
            return default_decision;
        };
        let Some(mut decision) = provider.lookup(key, context) else {
            return default_decision;
        };
        decision.key = key.to_owned();
        if decision.reason == Reason::Error {
            tracing::warn!(
                key,
                source = %decision.source,
                "feature flag provider returned an error decision"
            );
        }
        decision
    }

    pub fn provider(&self) -> Option<&Arc<dyn Provider>> {
        self.provider.as_ref()
    }
}

impl FlagSource for Service {
    fn is_enabled(&self, key: &str, default: bool) -> bool {
        Service::is_enabled(self, key, default)
    }

    fn is_enabled_with_context(&self, key: &str, context: &EvalContext, default: bool) -> bool {
        Service::is_enabled_with_context(self, key, context, default)
    }
}

impl<T: FlagSource + ?Sized> FlagSource for &T {
    fn is_enabled(&self, key: &str, default: bool) -> bool {
        (**self).is_enabled(key, default)
    }

    fn is_enabled_with_context(&self, key: &str, context: &EvalContext, default: bool) -> bool {
        (**self).is_enabled_with_context(key, context, default)
    }
}

fn default_decision(key: &str, variant: &str, enabled: bool) -> Decision {
    Decision {
        key: key.to_owned(),
        enabled,
        variant: variant.to_owned(),
        reason: Reason::Default,
        source: "default".to_owned(),
    }
}

fn bool_to_variant(enabled: bool) -> String {
    if enabled { "on" } else { "off" }.to_owned()
}

fn variant_enabled(variant: &str) -> bool {
    !matches!(variant, "" | "off" | "false" | "0")
}

/// A static rule with optional targeting and rollout behavior.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PercentRollout {
    pub percent: i32,
    #[serde(default)]
    pub by: String,
}

/// Thread-safe in-memory provider populated from code or YAML.
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
        let mut rules = self
            .rules
            .write()
            .unwrap_or_else(|error| error.into_inner());
        rules.insert(key.into(), rule);
    }

    /// Replaces the complete rule set atomically.
    pub fn load_rules(&self, rules: HashMap<String, Rule>) {
        let mut current = self
            .rules
            .write()
            .unwrap_or_else(|error| error.into_inner());
        *current = rules;
    }

    pub fn keys(&self) -> Vec<String> {
        let rules = self.rules.read().unwrap_or_else(|error| error.into_inner());
        let mut keys: Vec<_> = rules.keys().cloned().collect();
        keys.sort();
        keys
    }
}

impl Provider for StaticProvider {
    fn lookup(&self, key: &str, context: &EvalContext) -> Option<Decision> {
        let rule = self
            .rules
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .get(key)
            .cloned()?;
        Some(evaluate_rule(key, &rule, context))
    }

    fn name(&self) -> &str {
        "static"
    }
}

fn evaluate_rule(key: &str, rule: &Rule, context: &EvalContext) -> Decision {
    let deny_by = or_default(&rule.deny_by, "user_id");
    if !rule.deny.is_empty()
        && context
            .lookup(deny_by)
            .is_some_and(|value| rule.deny.iter().any(|candidate| candidate == value))
    {
        return decision_from_rule(key, rule, false, Reason::Static);
    }

    let allow_by = or_default(&rule.allow_by, "user_id");
    if !rule.allow.is_empty()
        && context
            .lookup(allow_by)
            .is_some_and(|value| rule.allow.iter().any(|candidate| candidate == value))
    {
        return decision_from_rule(key, rule, true, Reason::Static);
    }

    if let Some(percent) = &rule.percent {
        let by = or_default(&percent.by, "user_id");
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
        bool_to_variant(enabled)
    };
    Decision {
        key: key.to_owned(),
        enabled,
        variant,
        reason,
        source: "static".to_owned(),
    }
}

fn or_default<'a>(value: &'a str, default: &'a str) -> &'a str {
    if value.is_empty() {
        default
    } else {
        value
    }
}

/// First-match provider chain. `None` means the next provider may decide.
pub struct ChainProvider {
    providers: Vec<Arc<dyn Provider>>,
}

impl ChainProvider {
    pub fn new(providers: Vec<Arc<dyn Provider>>) -> Self {
        Self { providers }
    }

    pub fn providers(&self) -> &[Arc<dyn Provider>] {
        &self.providers
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

/// Environment-backed provider for operational overrides.
pub struct EnvProvider {
    prefix: String,
    lookup: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
}

impl EnvProvider {
    pub fn new(prefix: impl Into<String>) -> Self {
        Self::with_lookup(prefix, |name| env::var(name).ok())
    }

    fn with_lookup<F>(prefix: impl Into<String>, lookup: F) -> Self
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        Self {
            prefix: prefix.into(),
            lookup: Arc::new(lookup),
        }
    }

    #[cfg(test)]
    fn from_values(prefix: impl Into<String>, values: HashMap<String, String>) -> Self {
        Self::with_lookup(prefix, move |name| values.get(name).cloned())
    }
}

impl Provider for EnvProvider {
    fn lookup(&self, key: &str, context: &EvalContext) -> Option<Decision> {
        let name = format!("{}{}", self.prefix, flag_key_to_env(key));
        let raw = (self.lookup)(&name)?;
        let value = raw.trim();

        if value.is_empty() {
            return Some(env_decision(key, false, "off", Reason::Static));
        }

        if let Some(percent) = value.strip_suffix('%') {
            let Ok(percent) = percent.trim().parse::<i32>() else {
                return Some(env_decision(key, false, "off", Reason::Error));
            };
            if !(0..=100).contains(&percent) {
                return Some(env_decision(key, false, "off", Reason::Error));
            }
            let identifier = context.lookup("user_id").unwrap_or_default();
            let enabled = in_percent(key, identifier, percent);
            return Some(env_decision(
                key,
                enabled,
                if enabled { "on" } else { "off" },
                Reason::Percent,
            ));
        }

        match value.to_ascii_lowercase().as_str() {
            "true" | "on" | "1" | "yes" => Some(env_decision(key, true, "on", Reason::Static)),
            "false" | "off" | "0" | "no" => Some(env_decision(key, false, "off", Reason::Static)),
            _ => Some(env_decision(key, true, value, Reason::Static)),
        }
    }

    fn name(&self) -> &str {
        "env"
    }
}

fn env_decision(key: &str, enabled: bool, variant: &str, reason: Reason) -> Decision {
    Decision {
        key: key.to_owned(),
        enabled,
        variant: variant.to_owned(),
        reason,
        source: "env".to_owned(),
    }
}

/// Converts a flag key to the `FF_` environment-key shape used by Go.
pub fn flag_key_to_env(key: &str) -> String {
    let mut result = String::with_capacity(key.len());
    let mut underscore = false;
    for character in key.chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character.to_ascii_uppercase());
            underscore = false;
        } else if !underscore {
            result.push('_');
            underscore = true;
        }
    }
    result.trim_matches('_').to_owned()
}

/// Returns the stable FNV-1a bucket in `[0, 100)` for a flag and identifier.
/// The explicit zero separator and UTF-8 byte iteration are part of the
/// cross-language frontend/backend contract.
pub fn bucket_for(key: &str, identifier: &str) -> u32 {
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

pub fn in_percent(key: &str, identifier: &str, percent: i32) -> bool {
    match percent {
        value if value <= 0 => false,
        value if value >= 100 => true,
        value => bucket_for(key, identifier) < value as u32,
    }
}

#[derive(Debug, Deserialize)]
struct RuleConfig {
    #[serde(default)]
    default: bool,
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
    percent: i32,
    #[serde(default)]
    by: String,
}

impl From<RuleConfig> for Rule {
    fn from(config: RuleConfig) -> Self {
        Self {
            default: config.default,
            variant: config.variant,
            allow: config.allow,
            allow_by: config.allow_by,
            deny: config.deny,
            deny_by: config.deny_by,
            percent: config.percent.map(|percent| PercentRollout {
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

pub fn parse_rules_yaml(bytes: &[u8]) -> anyhow::Result<HashMap<String, Rule>> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(HashMap::new());
    }
    let raw: HashMap<String, RuleConfig> = serde_yaml::from_slice(bytes)
        .map_err(|error| anyhow::anyhow!("featureflag: parse: {error}"))?;
    Ok(raw
        .into_iter()
        .map(|(key, rule)| (key, rule.into()))
        .collect())
}

/// Startup configuration used by the Rust server. Environment overrides are
/// checked first, then the complete YAML rule set, matching Go's chain order.
pub struct ConfiguredFlags {
    service: Service,
}

impl Default for ConfiguredFlags {
    fn default() -> Self {
        Self {
            service: Service::default(),
        }
    }
}

impl ConfiguredFlags {
    pub fn from_env() -> anyhow::Result<Self> {
        let path = env::var("CORDY_FEATURE_FLAGS_FILE")
            .unwrap_or_default()
            .trim()
            .to_owned();

        let env_provider: Arc<dyn Provider> = Arc::new(EnvProvider::new("FF_"));
        let mut providers = vec![env_provider];
        if !path.is_empty() {
            let static_provider = Arc::new(StaticProvider::new());
            static_provider.load_rules(load_rules_from_yaml_file(&path)?);
            let provider: Arc<dyn Provider> = static_provider;
            providers.push(provider);
        }

        Ok(Self {
            service: Service::from_provider(ChainProvider::new(providers)),
        })
    }

    pub fn decision(&self, key: &str, default: bool, context: &EvalContext) -> Decision {
        self.service.decision_with_context(key, default, context)
    }

    pub fn service(&self) -> &Service {
        &self.service
    }
}

impl FlagSource for ConfiguredFlags {
    fn is_enabled(&self, key: &str, default: bool) -> bool {
        self.service.is_enabled(key, default)
    }

    fn is_enabled_with_context(&self, key: &str, context: &EvalContext, default: bool) -> bool {
        self.service.is_enabled_with_context(key, context, default)
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
    let mut output = HashMap::with_capacity(FRONTEND_PUBLIC_FLAGS.len() + 3);
    for key in FRONTEND_PUBLIC_FLAGS {
        output.insert((*key).to_owned(), flags.is_enabled(key, false));
    }
    output.insert(AGENT_BUILDER_COMPAT.to_owned(), true);
    output.insert(AGENT_SKILL_TOGGLES_COMPAT.to_owned(), true);
    output.insert(RESOURCE_LABELS_COMPAT.to_owned(), true);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(user_id: &str) -> EvalContext {
        EvalContext {
            user_id: user_id.to_owned(),
            ..EvalContext::default()
        }
    }

    #[test]
    fn cross_language_buckets_match_go_and_typescript() {
        let cases = [
            ("billing_new_invoice", "user-42", 97),
            ("feature_a", "user-1", 50),
            ("checkout_algo", "u-7f8a", 11),
            ("ws_rollout", "workspace-1", 62),
            ("empty_id_flag", "", 83),
            ("flag", "é", 53),
            ("flag", "🦄", 82),
            ("实验", "user-1", 90),
            ("flag", "用户-1", 95),
            ("checkout_算法", "user-100", 79),
        ];
        for (key, identifier, expected) in cases {
            assert_eq!(bucket_for(key, identifier), expected, "{key}/{identifier}");
        }
    }

    #[test]
    fn eval_context_ignores_empty_attributes() {
        let mut context = context("u-1");
        context.workspace_id = "w-2".to_owned();
        context.attributes.insert("plan".into(), "pro".into());
        context.attributes.insert("country".into(), String::new());
        assert_eq!(context.lookup("user_id"), Some("u-1"));
        assert_eq!(context.lookup("workspace_id"), Some("w-2"));
        assert_eq!(context.lookup("plan"), Some("pro"));
        assert_eq!(context.lookup("country"), None);
        assert_eq!(context.lookup("unknown"), None);
    }

    #[test]
    fn static_provider_applies_deny_allow_percent_and_variant() {
        let provider = StaticProvider::new();
        provider.set(
            "exp",
            Rule {
                default: false,
                variant: "experiment-v2".into(),
                allow: vec!["internal".into()],
                deny: vec!["banned".into()],
                ..Rule::default()
            },
        );
        assert!(!provider.lookup("exp", &context("banned")).unwrap().enabled);
        assert_eq!(
            provider.lookup("exp", &context("banned")).unwrap().variant,
            "off"
        );
        assert_eq!(
            provider
                .lookup("exp", &context("internal"))
                .unwrap()
                .variant,
            "experiment-v2"
        );
        assert_eq!(provider.lookup("missing", &EvalContext::default()), None);

        provider.set(
            "rollout",
            Rule {
                percent: Some(PercentRollout {
                    percent: 100,
                    by: "workspace_id".into(),
                }),
                ..Rule::default()
            },
        );
        let mut workspace = EvalContext::default();
        workspace.workspace_id = "w-1".into();
        let decision = provider.lookup("rollout", &workspace).unwrap();
        assert!(decision.enabled);
        assert_eq!(decision.reason, Reason::Percent);
    }

    #[test]
    fn static_provider_load_rules_replaces_and_sorts() {
        let provider = StaticProvider::new();
        provider.set("old", Rule::default());
        provider.load_rules(HashMap::from([
            ("zeta".into(), Rule::default()),
            ("alpha".into(), Rule::default()),
        ]));
        assert_eq!(provider.keys(), ["alpha", "zeta"]);
        assert_eq!(provider.lookup("old", &EvalContext::default()), None);
    }

    #[test]
    fn env_provider_matches_boolean_variant_percent_and_error_contract() {
        let provider = EnvProvider::from_values(
            "FF_",
            HashMap::from([
                ("FF_ENABLED".into(), "TRUE".into()),
                ("FF_ALGO".into(), "experiment-v2".into()),
                ("FF_DEMO".into(), "100%".into()),
                ("FF_BAD".into(), "abc%".into()),
                ("FF_EMPTY".into(), String::new()),
            ]),
        );
        assert_eq!(provider.lookup("missing", &EvalContext::default()), None);
        assert_eq!(
            provider
                .lookup("enabled", &EvalContext::default())
                .unwrap()
                .variant,
            "on"
        );
        assert_eq!(
            provider
                .lookup("algo", &EvalContext::default())
                .unwrap()
                .variant,
            "experiment-v2"
        );
        assert!(
            provider
                .lookup("demo", &EvalContext::default())
                .unwrap()
                .enabled
        );
        assert_eq!(
            provider
                .lookup("bad", &EvalContext::default())
                .unwrap()
                .reason,
            Reason::Error
        );
        assert!(
            !provider
                .lookup("empty", &EvalContext::default())
                .unwrap()
                .enabled
        );
        assert_eq!(
            flag_key_to_env("checkout.newPayment"),
            "CHECKOUT_NEWPAYMENT"
        );
        assert_eq!(flag_key_to_env("  weird  spaces  "), "WEIRD_SPACES");
    }

    #[test]
    fn chain_provider_first_match_wins() {
        let first = StaticProvider::new();
        first.set(
            "flag",
            Rule {
                default: true,
                ..Rule::default()
            },
        );
        let second = StaticProvider::new();
        second.set("flag", Rule::default());
        let first: Arc<dyn Provider> = Arc::new(first);
        let second: Arc<dyn Provider> = Arc::new(second);
        let chain = ChainProvider::new(vec![first, second]);
        let decision = chain.lookup("flag", &EvalContext::default()).unwrap();
        assert!(decision.enabled);
        assert_eq!(decision.source, "static");
    }

    #[test]
    fn service_defaults_and_variant_projection_are_stable() {
        let service = Service::default();
        let default = service.decision("missing", false);
        assert_eq!(default.reason, Reason::Default);
        assert_eq!(default.source, "default");
        assert_eq!(service.variant("missing", "control"), "control");

        let provider = StaticProvider::new();
        provider.set(
            "exp",
            Rule {
                variant: "arm-a".into(),
                ..Rule::default()
            },
        );
        let service = Service::from_provider(provider);
        assert_eq!(service.variant("exp", "control"), "off");
        assert_eq!(
            service.variant_with_context("exp", "control", &context("u")),
            "off"
        );
    }

    #[test]
    fn yaml_parser_keeps_the_full_rule_shape() {
        let rules = parse_rules_yaml(
            br#"
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
"#,
        )
        .unwrap();
        let rule = rules.get("checkout_algo").unwrap();
        assert_eq!(rule.variant, "experiment-v2");
        assert_eq!(rule.allow_by, "user_id");
        assert_eq!(rule.deny_by, "workspace_id");
        assert_eq!(rule.percent.as_ref().unwrap().percent, 25);
        assert!(parse_rules_yaml(b"  \n\n").unwrap().is_empty());
        assert!(parse_rules_yaml(b"flag: {").is_err());
    }

    #[test]
    fn frontend_public_flags_keep_compat_keys() {
        struct Enabled;
        impl FlagSource for Enabled {
            fn is_enabled(&self, key: &str, _default: bool) -> bool {
                key == PLUGINS_V1
            }
        }
        let flags = evaluate_frontend_public_flags(&Enabled);
        assert_eq!(flags.len(), 7);
        assert!(flags[PLUGINS_V1]);
        assert!(flags[AGENT_BUILDER_COMPAT]);
        assert!(flags[AGENT_SKILL_TOGGLES_COMPAT]);
        assert!(flags[RESOURCE_LABELS_COMPAT]);
    }
}
