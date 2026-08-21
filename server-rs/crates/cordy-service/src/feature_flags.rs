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
}
