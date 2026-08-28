//! The `agent` trigger.
//!
//! A plugin hook offered to an agent as an MCP tool.
//!
//! This is the fourth call site, and the one that is NOT the host deciding to
//! call something. An agent sees a tool, reads its description, and chooses.
//! That choice is the whole reason hooks may be reached from an agent at all:
//! the alternative — a hook that must run before or after every turn — is a
//! third party holding the product's main loop open, which is why no such
//! position exists anywhere in this design.
//!
//! The daemon renders these as tools but does NOT call the plugin. A tool call
//! goes back to the server, which performs the signed request. The signing
//! secret is derived from the deployment key and never leaves this process;
//! equally important, routing through the server means the rate limit, the
//! circuit breaker, the `net:` destination check and the invocation record all
//! apply exactly as they do for every other trigger, rather than being
//! reimplemented daemon-side where they would drift.

use std::sync::Arc;

use regex::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::plugin::{
    hook_allows_trigger, parse_installation_manifest, uuid_string, PluginErrorKind, PluginService,
};
use crate::plugin_action::authorize_plugin_action;
use crate::plugin_hook::{invoke_hook, HookInvocation};
use crate::plugin_token::HookActor;

/// One hook, described the way an agent will see it.
#[derive(Debug, Clone, Serialize)]
pub struct PluginHookTool {
    pub installation_id: String,
    pub hook_key: String,
    /// What the agent sees, namespaced so two plugins contributing the same
    /// hook key do not collide.
    pub name: String,
    /// The manifest's hook description verbatim: it is what the agent reads to
    /// decide whether to call this, so the plugin author writes it and the host
    /// does not paraphrase.
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<String>,
}

/// Matches everything MCP tool names should not contain. Agents and providers
/// vary in what they accept; letters, digits and underscores are the
/// intersection that works everywhere.
fn tool_name_unsafe() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[^a-zA-Z0-9_]+").expect("static regex"))
}

fn clean_name(value: &str) -> String {
    tool_name_unsafe()
        .replace_all(value, "_")
        .trim_matches('_')
        .to_string()
}

/// Namespaces a hook so two plugins can both contribute a hook called
/// "summarize" without one shadowing the other.
///
/// The naive version of this — sanitize both halves and join with an underscore
/// — is not injective, and a test caught it: a plugin key uses `.` and `-`, both
/// of which have to become `_`, so `a.b` and `a-b` collapse together, and
/// `a.b_` + `c` collides with `a.b` + `c`. Two different plugins would then be
/// offering the agent one tool name, and whichever was registered last would
/// answer for both.
///
/// So the prefix carries a short digest of the FULL plugin key rather than a
/// lossy transliteration of it. The readable part is the last segment, which is
/// what a person recognises; the digest is what makes it unique.
///
/// The `__` separator is safe because a hook key cannot contain one: its pattern
/// requires an alphanumeric after every underscore, so a doubled underscore is
/// unrepresentable.
pub fn plugin_tool_name(plugin_key: &str, hook_key: &str) -> String {
    let readable_segment = plugin_key.rsplit('.').next().unwrap_or(plugin_key);
    let mut readable = clean_name(readable_segment);
    if readable.is_empty() {
        readable = "plugin".to_string();
    }
    let digest = Sha256::digest(plugin_key.as_bytes());
    format!(
        "{readable}_{}__{}",
        &hex::encode(digest)[..6],
        clean_name(hook_key)
    )
}

/// Lists the hooks an agent running in this workspace may call.
///
/// Disabled installations are skipped, which is what makes disabling a plugin
/// take effect on the next task rather than only in the UI. An uninstalled one
/// has no row at all.
pub async fn agent_hook_tools(
    service: &PluginService,
    workspace_id: Uuid,
) -> Result<Vec<PluginHookTool>, crate::plugin::PluginError> {
    let installations =
        cordy_db::queries::plugin::list_workspace_plugin_installations(&service.pool, workspace_id)
            .await
            .map_err(|e| {
                crate::plugin::PluginError::with_source(
                    PluginErrorKind::Unavailable,
                    "list plugin installations",
                    crate::plugin::box_anyhow(e),
                )
            })?;

    let mut tools = Vec::new();
    for installation in &installations {
        if !installation.enabled {
            continue;
        }
        // One unreadable manifest must not hide every other plugin's tools.
        let Ok(manifest) =
            parse_installation_manifest(&crate::plugin::json_bytes(&installation.manifest))
        else {
            continue;
        };
        for hook in &manifest.contributes.hooks {
            if !hook_allows_trigger(hook, cordy_plugincontract::TRIGGER_AGENT) {
                continue;
            }
            tools.push(PluginHookTool {
                installation_id: uuid_string(installation.id),
                hook_key: hook.key.clone(),
                name: plugin_tool_name(&manifest.key, &hook.key),
                description: hook.description.clone(),
                input_schema: hook.input_schema.as_ref().map(|raw| raw.get().to_string()),
            });
        }
    }
    // Stable order so a task's tool list does not reshuffle between claims for
    // no reason, which shows up as cache churn in provider-side prompt caching.
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tools)
}

/// Runs one agent-triggered hook.
///
/// The actor is the AGENT, not the person who filed the issue and not the
/// plugin: an agent chose to call this, so the writes it produces are the
/// agent's, exactly as they would be if the agent had written them directly.
/// author_type already has a value for that, which is why this trigger needs no
/// new one.
pub async fn invoke_agent_hook(
    service: Arc<PluginService>,
    callbacks: Option<&crate::plugin_token::CallbackTokens>,
    callback_base_url: &str,
    installation_id: &str,
    hook_key: &str,
    agent_id: Uuid,
    input: Option<&serde_json::Value>,
) -> (
    crate::plugin_hook::HookResult,
    Result<(), crate::plugin::PluginError>,
) {
    let caller =
        match authorize_plugin_action(&service.pool, installation_id, Uuid::nil(), "").await {
            Ok(caller) => caller,
            Err(err) => return (Default::default(), Err(err)),
        };
    let hook = match crate::plugin::find_hook(
        &crate::plugin::json_bytes(&caller.installation.manifest),
        hook_key,
    ) {
        Ok(hook) => hook,
        Err(err) => return (Default::default(), Err(err)),
    };
    let invocation = HookInvocation {
        installation: &caller.installation,
        hook: &hook,
        trigger: cordy_plugincontract::TRIGGER_AGENT,
        event_type: "",
        actor: HookActor {
            actor_type: "agent".to_string(),
            id: agent_id,
        },
        issue_id: None,
        input,
    };
    invoke_hook(&service, callbacks, callback_base_url, invocation, 1).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_are_namespaced_and_injective_across_similar_keys() {
        let first = plugin_tool_name("com.example.a.b", "summarize");
        let second = plugin_tool_name("com.example.a-b", "summarize");
        assert_ne!(
            first, second,
            "a.b and a-b must not collapse into one tool name"
        );
        assert_eq!(
            plugin_tool_name("com.example.notes", "summarize"),
            plugin_tool_name("com.example.notes", "summarize"),
            "the same inputs must be deterministic"
        );
    }

    #[test]
    fn readable_part_is_the_last_segment_and_digest_is_the_full_key() {
        let name = plugin_tool_name("com.example.my-plugin", "do_thing");
        assert!(
            name.starts_with("my_plugin_"),
            "readable segment keeps recognisability: {name}"
        );
        assert!(name.contains("__"), "separator present: {name}");
        assert!(name.ends_with("do_thing"), "hook key sanitized: {name}");
        // Only safe characters survive.
        assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    }

    #[test]
    fn empty_readable_segment_falls_back_to_plugin() {
        let name = plugin_tool_name("...", "k");
        assert!(name.starts_with("plugin_"), "{name}");
    }
}
