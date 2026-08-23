//! Host capability gating — port of `capabilities.go`. The manifest schema is
//! defined in full from the first release so publishers never chase a moving
//! contract, but the host lands the machinery one slice at a time. Anything a
//! manifest declares that is not listed here fails installation loudly — a
//! silently ignored contribution would look installed and never fire.

use std::collections::{BTreeSet, HashSet};

use crate::types::{
    Manifest, RESOURCE_SKILL, SCOPE_NET_PREFIX, SURFACE_ISSUE_PANEL, SURFACE_MODAL, TRANSPORT_HTTP,
    TRANSPORT_MCP, TRIGGER_AGENT, TRIGGER_EVENT, TRIGGER_MANUAL, TRIGGER_UI,
};

/// What this host build can actually run. Flip an entry on in the same change
/// that lands its runtime.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub surface_types: HashSet<String>,
    pub hook_triggers: HashSet<String>,
    pub hook_transport: HashSet<String>,
    pub resource_types: HashSet<String>,
}

impl Capabilities {
    fn with<I: IntoIterator<Item = S>, S: Into<String>>(items: I) -> HashSet<String> {
        items.into_iter().map(Into::into).collect()
    }
}

/// The currently shipped set.
///
/// Action API + issue/sidebar surfaces land with the surface runtime; hooks
/// land with the hook engine; the agent trigger, the mcp transport, and skill
/// resources land with the agent integration.
pub fn host_capabilities() -> Capabilities {
    Capabilities {
        // issue_panel mounts in PluginPanelSection; modal opens from a manual
        // hook action. sidebar_panel stays off — it has no host location, and
        // enabling a surface the host cannot render installs a plugin that
        // silently never appears, which is precisely what this gate prevents.
        surface_types: Capabilities::with([SURFACE_ISSUE_PANEL, SURFACE_MODAL]),
        // The agent trigger is not a call site the host drives: the hook is
        // offered to an agent as an MCP tool and the agent decides.
        hook_triggers: Capabilities::with([
            TRIGGER_UI,
            TRIGGER_MANUAL,
            TRIGGER_EVENT,
            TRIGGER_AGENT,
        ]),
        // http calls one declared endpoint; mcp adopts an external MCP server's
        // tools, which is why it ships with an approval step that pins them by
        // schema digest. Installing the plugin is not the grant there;
        // approving the tools is.
        hook_transport: Capabilities::with([TRANSPORT_HTTP, TRANSPORT_MCP]),
        // A skill resource is not a call in either direction — it is a
        // SKILL.md written into the existing skill table at install and
        // removed at uninstall.
        resource_types: Capabilities::with([RESOURCE_SKILL]),
    }
}

/// Describes contributions this build cannot run yet.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("this Cordy version does not support: {}", .missing.join(", "))]
pub struct CapabilityUnavailable {
    pub missing: Vec<String>,
}

impl Manifest {
    /// Reports every declared contribution the host cannot run. It returns all
    /// of them at once so an administrator sees the full gap instead of
    /// discovering it one failed install at a time.
    pub fn check_capabilities(&self, host: &Capabilities) -> Result<(), CapabilityUnavailable> {
        let mut missing = BTreeSet::new();
        for surface in &self.contributes.surfaces {
            if !host.surface_types.contains(&surface.surface_type) {
                missing.insert(format!("surface {}", surface.surface_type));
            }
        }
        for hook in &self.contributes.hooks {
            for trigger in &hook.triggers {
                if !host.hook_triggers.contains(trigger) {
                    missing.insert(format!("hook trigger {trigger}"));
                }
            }
            if !host.hook_transport.contains(&hook.transport.transport_type) {
                missing.insert(format!("hook transport {}", hook.transport.transport_type));
            }
        }
        for resource in &self.contributes.resources {
            if !host.resource_types.contains(&resource.resource_type) {
                missing.insert(format!("resource {}", resource.resource_type));
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        Err(CapabilityUnavailable {
            missing: missing.into_iter().collect(),
        })
    }
}

/// Returns the domains an installation may reach, taken only from the scopes it
/// was granted (every `net:<domain>` scope, prefix stripped).
pub fn net_domains(scopes: &[String]) -> Vec<String> {
    scopes
        .iter()
        .filter_map(|scope| scope.strip_prefix(SCOPE_NET_PREFIX))
        .map(str::to_string)
        .collect()
}
