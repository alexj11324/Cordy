//! Feature-flag key vocabulary and evaluation helpers.
//!
//! The concrete flag service (`pkg/featureflag`, env/static/chain providers)
//! is a separate porting unit; until it lands, call sites inject any
//! [`FlagSource`] implementation, mirroring how the Go functions accept a
//! `*featureflag.Service`.

/// Minimal evaluation seam — Go's `flags.IsEnabled(ctx, key, default)`.
pub trait FlagSource: Send + Sync {
    fn is_enabled(&self, key: &str, default: bool) -> bool;
}

/// Startup-loaded YAML rules with live `FF_<KEY>` environment overrides.
/// The current Rust call sites have no user/workspace evaluation context, so
/// percentage rollouts use the same stable anonymous bucket as the Go provider.
#[derive(Debug, Default)]
pub struct ConfiguredFlags {
    rules: std::collections::HashMap<String, ConfiguredRule>,
}

#[derive(Debug, Clone, Copy)]
struct ConfiguredRule {
    default: bool,
    percent: Option<u32>,
}

#[derive(serde::Deserialize)]
struct RuleConfig {
    #[serde(default)]
    default: bool,
    #[serde(default)]
    percent: Option<PercentRuleConfig>,
}

#[derive(serde::Deserialize)]
struct PercentRuleConfig {
    percent: u32,
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
        Self::from_yaml_bytes(&bytes)
    }

    fn from_yaml_bytes(bytes: &[u8]) -> Result<Self, anyhow::Error> {
        if String::from_utf8_lossy(bytes).trim().is_empty() {
            return Ok(Self::default());
        }
        let rules: std::collections::HashMap<String, RuleConfig> = serde_yaml::from_slice(bytes)
            .map_err(|error| anyhow::anyhow!("featureflag: parse: {error}"))?;
        let mut configured = std::collections::HashMap::with_capacity(rules.len());
        for (key, rule) in rules {
            let percent = rule.percent.map(|rule| rule.percent);
            if let Some(percent) = percent {
                anyhow::ensure!(
                    percent <= 100,
                    "featureflag: {key} percent must be between 0 and 100"
                );
            }
            configured.insert(
                key,
                ConfiguredRule {
                    default: rule.default,
                    percent,
                },
            );
        }
        Ok(Self { rules: configured })
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

    fn anonymous_bucket(key: &str) -> u32 {
        let mut hash = 2_166_136_261_u32;
        for byte in key.bytes().chain(std::iter::once(0)) {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(16_777_619);
        }
        hash % 100
    }

    fn percent_decision(key: &str, percent: u32) -> bool {
        Self::anonymous_bucket(key) < percent.min(100)
    }

    fn env_decision(key: &str, raw: &str) -> bool {
        let value = raw.trim();
        match value.to_ascii_lowercase().as_str() {
            "" | "false" | "off" | "0" | "no" => false,
            "true" | "on" | "1" | "yes" => true,
            _ if value.ends_with('%') => {
                let Some(percent) = value[..value.len() - 1]
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|percent| *percent <= 100)
                else {
                    return false;
                };
                Self::percent_decision(key, percent)
            }
            _ => true,
        }
    }
}

impl FlagSource for ConfiguredFlags {
    fn is_enabled(&self, key: &str, default: bool) -> bool {
        if let Ok(value) = std::env::var(Self::env_name(key)) {
            return Self::env_decision(key, &value);
        }
        let Some(rule) = self.rules.get(key) else {
            return default;
        };
        match rule.percent {
            Some(percent) => Self::percent_decision(key, percent),
            None => rule.default,
        }
    }
}

impl<T: FlagSource + ?Sized> FlagSource for &T {
    fn is_enabled(&self, key: &str, default: bool) -> bool {
        (**self).is_enabled(key, default)
    }
}

pub const BILLING_WORKSPACE_SUBSCRIPTIONS: &str = "billing_workspace_subscriptions";
pub const COMPOSIO_MCP_APPS: &str = "composio_mcp_apps";
pub const PLUGINS_V1: &str = "plugins_v1";

/// Gates CREATING a custom issue status (PB-6243) — a rollout gate, not a
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
    fn configured_flags_evaluate_yaml_percentage_rules() {
        let flags = ConfiguredFlags::from_yaml_bytes(
            br#"
custom_issue_statuses:
  default: false
  percent:
    percent: 100
plugins_v1:
  default: true
  percent:
    percent: 0
billing_workspace_subscriptions:
  default: true
"#,
        )
        .unwrap();
        assert!(flags.is_enabled(CUSTOM_ISSUE_STATUSES, false));
        assert!(!flags.is_enabled(PLUGINS_V1, true));
        assert!(flags.is_enabled(BILLING_WORKSPACE_SUBSCRIPTIONS, false));
    }

    #[test]
    fn configured_flags_reject_invalid_yaml_percentages() {
        let error = ConfiguredFlags::from_yaml_bytes(
            br#"
plugins_v1:
  percent:
    percent: 101
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("between 0 and 100"));
    }
}
