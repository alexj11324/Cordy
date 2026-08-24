//! Feature-flag key vocabulary and evaluation helpers — port of
//! `server/internal/featureflags/keys.go`.
//!
//! The concrete flag service (`pkg/featureflag`, env/static/chain providers)
//! is a separate porting unit; until it lands, call sites inject any
//! [`FlagSource`] implementation, mirroring how the Go functions accept a
//! `*featureflag.Service`.

/// Minimal evaluation seam — Go's `flags.IsEnabled(ctx, key, default)`.
pub trait FlagSource: Send + Sync {
    fn is_enabled(&self, key: &str, default: bool) -> bool;
}

/// Startup-loaded YAML defaults with live `FF_<KEY>` environment overrides.
/// This is the boolean projection used by current Rust call sites; it mirrors
/// the Go provider precedence and fail-closed behavior.
#[derive(Debug, Default)]
pub struct ConfiguredFlags {
    defaults: std::collections::HashMap<String, bool>,
}

#[derive(serde::Deserialize)]
struct RuleConfig {
    #[serde(default)]
    default: bool,
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
        let rules: std::collections::HashMap<String, RuleConfig> =
            serde_yaml::from_slice(&bytes)
                .map_err(|error| anyhow::anyhow!("featureflag: parse: {error}"))?;
        Ok(Self {
            defaults: rules
                .into_iter()
                .map(|(key, rule)| (key, rule.default))
                .collect(),
        })
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
        let value = raw.trim();
        match value.to_ascii_lowercase().as_str() {
            "" | "false" | "off" | "0" | "no" => false,
            "true" | "on" | "1" | "yes" => true,
            // Current FlagSource has no evaluation context. Match Go's stable
            // anonymous bucket: the empty identifier is shared by all calls.
            _ if value.ends_with('%') => {
                let Some(percent) = value[..value.len() - 1]
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|percent| *percent <= 100)
                else {
                    return false;
                };
                let mut hash = 2_166_136_261_u32;
                for byte in key.bytes().chain(std::iter::once(0)) {
                    hash ^= u32::from(byte);
                    hash = hash.wrapping_mul(16_777_619);
                }
                hash % 100 < percent
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
        self.defaults.get(key).copied().unwrap_or(default)
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
}
