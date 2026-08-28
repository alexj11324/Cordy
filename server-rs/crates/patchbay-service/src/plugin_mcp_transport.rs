//! The `mcp` transport: a hook that points at an MCP server the plugin
//! author already runs, whose tools Patchbay adopts.
//!
//! The
//! discovery protocol itself lives in `patchbay_remotemcp::discover`; this
//! module wires the plugin domain around it.
//!
//! The whole difference from an `http` hook is who decides the shape. An
//! http hook declares one endpoint in a manifest an administrator read and
//! approved. An MCP server decides its own tool list at runtime and may
//! change it whenever it likes — so "install this plugin" would otherwise
//! be a standing grant to run whatever that server offers next week.
//!
//! Hence the approval: an administrator sees the discovered tools and pins
//! them by name and schema digest. A tool that appears later is not
//! adopted; a tool whose schema drifts stops being called. The daemon-side
//! enforcement already exists — the broker refuses pinned tools that went
//! missing or drifted — so this wires the plugin's hook into the
//! connection shape that check already reads.

use chrono::SecondsFormat;
use uuid::Uuid;

use patchbay_db::models::PluginInstallation;
use patchbay_plugincontract::{
    net_domains, Hook, Manifest, CONFIG_SECRET, TRANSPORT_MCP, TRIGGER_AGENT,
};
use patchbay_remotemcp::{discover, tool_set_digest, Connection, Tool, PLUGIN_CONTRIBUTION_PREFIX};

use crate::plugin::{
    decode_scopes, find_hook, hook_allows_trigger, json_bytes, parse_installation_manifest,
    plugin_errf, uuid_string, PluginError, PluginErrorKind,
};

/// One hook's approved tool list (Go `PluginMCPApproval`). Field names match
/// the Go json tags byte-for-byte.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PluginMcpApproval {
    #[serde(default)]
    pub tools: Vec<Tool>,
    #[serde(rename = "approved_at", default)]
    pub approved_at: String,
    #[serde(
        rename = "approved_by",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub approved_by: String,
}

/// Maps hook key to its approval (Go `PluginMCPApprovals`).
pub type PluginMcpApprovals = std::collections::HashMap<String, PluginMcpApproval>;

/// Decodes the stored approvals blob; a malformed entry is skipped — Go
/// ignores the Unmarshal error the same way (`_ = json.Unmarshal`).
pub fn decode_mcp_approvals(raw: &serde_json::Value) -> PluginMcpApprovals {
    let mut approvals = PluginMcpApprovals::new();
    let serde_json::Value::Object(map) = raw else {
        return approvals;
    };
    for (key, value) in map {
        if let Ok(approval) = serde_json::from_value::<PluginMcpApproval>(value.clone()) {
            approvals.insert(key.clone(), approval);
        }
    }
    approvals
}

/// Asks the hook's MCP server what it offers.
///
/// Read-only and admin-driven: nothing is adopted by discovering it. The
/// administrator picks from this and approves, which is the step that makes
/// a tool callable.
pub async fn discover_mcp_hook_tools(
    pool: &sqlx::PgPool,
    secrets: Option<&patchbay_util::secretbox::SecretBox>,
    installation: &PluginInstallation,
    hook_key: &str,
) -> Result<Vec<Tool>, PluginError> {
    let hook = find_hook(&json_bytes(&installation.manifest), hook_key)?;
    if hook.transport.transport_type != TRANSPORT_MCP {
        return Err(plugin_errf(
            PluginErrorKind::Invalid,
            format!("hook \"{hook_key}\" is not an mcp transport"),
        ));
    }
    // Second layer. Manifest validation already refuses a hook whose
    // transport URL is not covered by a declared net: scope, so this is
    // unreachable for a manifest that installed cleanly — kept because the
    // destination check below takes its allow-list from here, and an empty
    // one must fail closed rather than read as "no restriction".
    let domains =
        net_domains(&decode_scopes(&json_bytes(&installation.granted_scopes)).unwrap_or_default());
    if domains.is_empty() {
        return Err(plugin_errf(
            PluginErrorKind::Forbidden,
            "this Plugin was granted no net: scope, so it cannot reach an MCP server",
        ));
    }

    let headers = mcp_credential_headers(pool, secrets, installation, &hook).await?;
    // Same endpoint guard as every other outbound call: the destination must
    // be inside the consented `net:` set and resolve publicly. An empty
    // protocol-version list means "whatever this build supports".
    discover(&hook.transport.url, &domains, Vec::new(), &headers)
        .await
        .map(|(tools, _)| tools)
        .map_err(|err| {
            PluginError::with_source(
                PluginErrorKind::Unavailable,
                "could not reach the Plugin's MCP server",
                err,
            )
        })
}

/// Pins a tool list for one hook and returns the updated installation row.
///
/// Pinned by digest, not just by name: a server that keeps a tool's name
/// and changes its arguments has changed what the agent is calling, and the
/// agent would have no way to notice.
pub async fn approve_mcp_hook_tools(
    pool: &sqlx::PgPool,
    secrets: Option<&patchbay_util::secretbox::SecretBox>,
    installation: &PluginInstallation,
    hook_key: &str,
    names: &[String],
    user_id: Option<Uuid>,
) -> Result<PluginInstallation, PluginError> {
    let discovered = discover_mcp_hook_tools(pool, secrets, installation, hook_key).await?;
    let mut by_name: std::collections::HashMap<&str, &Tool> =
        std::collections::HashMap::with_capacity(discovered.len());
    for tool in &discovered {
        by_name.insert(tool.name.as_str(), tool);
    }

    let mut approved: Vec<Tool> = Vec::with_capacity(names.len());
    for name in names {
        let Some(tool) = by_name.get(name.as_str()) else {
            // Approving something the server does not currently offer would
            // pin a name with no schema behind it, and the broker would
            // refuse the whole connection at startup.
            return Err(plugin_errf(
                PluginErrorKind::Invalid,
                format!("tool \"{name}\" is not offered by this MCP server"),
            ));
        };
        approved.push((*tool).clone());
    }

    let mut approvals = decode_mcp_approvals(&installation.mcp_approvals);
    if approved.is_empty() {
        // Approving nothing is how an administrator withdraws a hook, so it
        // removes the entry rather than storing an empty allow-list that
        // reads like "approved, with nothing in it".
        approvals.remove(hook_key);
    } else {
        approvals.insert(
            hook_key.to_string(),
            PluginMcpApproval {
                tools: approved,
                approved_at: chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
                approved_by: user_id
                    .filter(|id| !id.is_nil())
                    .map(uuid_string)
                    .unwrap_or_default(),
            },
        );
    }

    let encoded = serde_json::to_value(&approvals).map_err(|e| {
        PluginError::new(PluginErrorKind::Invalid, format!("encode approvals: {e}"))
    })?;
    let updated =
        patchbay_db::queries::plugin::set_plugin_mcp_approvals(pool, installation.id, &encoded)
            .await
            .map_err(|e| {
                PluginError::new(
                    PluginErrorKind::Unavailable,
                    format!("store approvals: {e:#}"),
                )
            })?;
    let Some(updated) = updated else {
        return Err(plugin_errf(
            PluginErrorKind::Unavailable,
            "store approvals: installation no longer exists",
        ));
    };
    Ok(updated)
}

/// Turns approved mcp hooks into broker connections for one workspace.
///
/// Returned in the claim payload beside the http-transport tools, so the
/// daemon handles both through the machinery it already has. A hook with no
/// approval yields no connection. That is the point — installing the plugin
/// is not the grant, approving the tools is.
pub async fn agent_mcp_connections(
    pool: &sqlx::PgPool,
    workspace_id: Uuid,
) -> Result<Vec<Connection>, PluginError> {
    let installations =
        patchbay_db::queries::plugin::list_workspace_plugin_installations(pool, workspace_id)
            .await
            .map_err(|e| {
                PluginError::new(
                    PluginErrorKind::Unavailable,
                    format!("list plugin installations: {e:#}"),
                )
            })?;

    let mut connections: Vec<Connection> = Vec::new();
    for installation in &installations {
        if !installation.enabled {
            continue;
        }
        connections.extend(mcp_connections_for(installation));
    }
    Ok(connections)
}

/// Turns one installation's approved mcp hooks into broker connections.
/// Separated from the workspace query so the three conditions that decide
/// whether a hook is offered at all — mcp transport, agent trigger, a
/// non-empty approval — are testable without a database.
pub fn mcp_connections_for(installation: &PluginInstallation) -> Vec<Connection> {
    let manifest: Manifest = match parse_installation_manifest(&json_bytes(&installation.manifest))
    {
        Ok(manifest) => manifest,
        Err(_) => {
            // One unreadable manifest must not hide every other plugin's
            // tools.
            return Vec::new();
        }
    };
    let approvals = decode_mcp_approvals(&installation.mcp_approvals);
    let domains =
        net_domains(&decode_scopes(&json_bytes(&installation.granted_scopes)).unwrap_or_default());

    let mut connections: Vec<Connection> = Vec::new();
    for hook in &manifest.contributes.hooks {
        if hook.transport.transport_type != TRANSPORT_MCP {
            continue;
        }
        if !hook_allows_trigger(hook, TRIGGER_AGENT) {
            continue;
        }
        let Some(approval) = approvals.get(&hook.key) else {
            continue;
        };
        if approval.tools.is_empty() {
            continue;
        }
        let credential_header = manifest
            .config
            .field(&format!("{}_credential", hook.key))
            .filter(|field| field.field_type == CONFIG_SECRET)
            .is_some();
        connections.push(Connection {
            installation_id: uuid_string(installation.id),
            contribution_id: format!(
                "{PLUGIN_CONTRIBUTION_PREFIX}{}:{}",
                uuid_string(installation.id),
                hook.key
            ),
            contribution_key: crate::plugin_agent_tools::plugin_tool_name(&manifest.key, &hook.key),
            endpoint: hook.transport.url.clone(),
            transport: "http".to_string(),
            // The same exact-host set the consent screen showed. The broker
            // re-checks it at dial, so a manifest that later repoints its
            // own hook still cannot reach anywhere new.
            endpoint_allowed_hosts: domains.clone(),
            credential_header: if credential_header {
                "Authorization"
            } else {
                ""
            }
            .to_string(),
            approved_tools: approval.tools.clone(),
            tool_schema_digest: tool_set_digest_or_empty(&approval.tools),
            // A plugin's MCP server going down must not fail the task, for
            // the same reason a failing http hook is a tool error: an agent
            // should still be able to work on the issue.
            failure_policy: "optional".to_string(),
            ..Default::default()
        });
    }
    connections
}

/// Pins the whole approved set, not just each tool. A digest error yields
/// the empty string: the broker then treats the set as unpinned at the set
/// level while still checking every tool individually, which is the
/// degradation that fails safe.
fn tool_set_digest_or_empty(tools: &[Tool]) -> String {
    tool_set_digest(tools).unwrap_or_default()
}

/// Builds the auth header from the installation's secret config, if the
/// manifest declared one for this hook.
///
/// Reuses the existing secret storage — write-only, secretbox-encrypted,
/// never echoed by any read endpoint — rather than introducing a second
/// place where a plugin's credentials live.
async fn mcp_credential_headers(
    pool: &sqlx::PgPool,
    secrets: Option<&patchbay_util::secretbox::SecretBox>,
    installation: &PluginInstallation,
    hook: &Hook,
) -> Result<Vec<(String, String)>, PluginError> {
    let manifest: Manifest = parse_installation_manifest(&json_bytes(&installation.manifest))
        .map_err(|e| PluginError::new(PluginErrorKind::Invalid, e.to_string()))?;
    let credential_field = format!("{}_credential", hook.key);
    let declared_secret = manifest
        .config
        .field(&credential_field)
        .filter(|field| field.field_type == CONFIG_SECRET)
        .is_some();
    if !declared_secret {
        return Ok(Vec::new());
    }
    let secret = decrypted_secret(pool, secrets, installation.id, &credential_field).await?;
    if secret.is_empty() {
        // A declared-but-unset credential is a configuration gap, not a
        // reason to fail discovery with a confusing transport error.
        return Ok(Vec::new());
    }
    Ok(vec![("Authorization".to_string(), secret)])
}

/// Opens one stored secret.
///
/// The only place a plugin secret is ever decrypted for use, and it is used
/// here to authenticate to the plugin author's OWN server — never echoed
/// back through any read endpoint, which is the rule the separate secret
/// table exists to make structural.
async fn decrypted_secret(
    pool: &sqlx::PgPool,
    secrets: Option<&patchbay_util::secretbox::SecretBox>,
    installation_id: Uuid,
    key: &str,
) -> Result<String, PluginError> {
    let Some(secrets) = secrets else {
        return Err(plugin_errf(
            PluginErrorKind::Unavailable,
            "plugin secrets are disabled: PATCHBAY_PLUGIN_SECRET_KEY is not configured",
        ));
    };
    let row = patchbay_db::queries::plugin::get_plugin_secret(pool, installation_id, key)
        .await
        .map_err(|e| PluginError::new(PluginErrorKind::Unavailable, e.to_string()))?;
    let Some(row) = row else {
        return Ok(String::new());
    };
    let plaintext = secrets
        .open(&row.ciphertext)
        .map_err(|e| PluginError::new(PluginErrorKind::Unavailable, e.to_string()))?;
    Ok(String::from_utf8_lossy(&plaintext).to_string())
}

/// The pinned set for one hook, by name (Go `ApprovedMCPTools`).
pub fn approved_mcp_tools(
    installation: &PluginInstallation,
    hook_key: &str,
) -> std::collections::HashMap<String, Tool> {
    let approvals = decode_mcp_approvals(&installation.mcp_approvals);
    let Some(approval) = approvals.get(hook_key) else {
        return std::collections::HashMap::new();
    };
    approval
        .tools
        .iter()
        .map(|tool| (tool.name.clone(), tool.clone()))
        .collect()
}

/// Resolves the credential for one mcp hook, for the daemon's broker.
/// Returns `(name, value)`; both empty when the manifest declared none
/// (Go `MCPHookCredential`).
pub async fn mcp_hook_credential(
    pool: &sqlx::PgPool,
    secrets: Option<&patchbay_util::secretbox::SecretBox>,
    installation: &PluginInstallation,
    hook_key: &str,
) -> Result<(String, String), PluginError> {
    let hook = find_hook(&json_bytes(&installation.manifest), hook_key)?;
    let headers = mcp_credential_headers(pool, secrets, installation, &hook).await?;
    if let Some((name, value)) = headers.into_iter().next() {
        return Ok((name, value));
    }
    Ok((String::new(), String::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base_installation(mcp_approvals: serde_json::Value) -> PluginInstallation {
        PluginInstallation {
            config: json!({}),
            created_at: chrono::Utc::now(),
            enabled: true,
            granted_scopes: json!([]),
            id: Uuid::nil(),
            installed_by: None,
            manifest: json!({"key": "test-plugin", "contributes": {"hooks": []}}),
            mcp_approvals,
            plugin_key: "test-plugin".to_string(),
            source_url: String::new(),
            token_hash: None,
            token_rotated_at: None,
            updated_at: chrono::Utc::now(),
            version: "1.0.0".to_string(),
            workspace_id: Uuid::nil(),
        }
    }

    fn tool(name: &str) -> Tool {
        Tool {
            name: name.to_string(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
            schema_digest: format!("sha256:{name}"),
            risk: String::new(),
        }
    }

    #[test]
    fn decode_mcp_approvals_roundtrip_and_tolerates_garbage() {
        assert!(decode_mcp_approvals(&json!({})).is_empty());
        assert!(decode_mcp_approvals(&serde_json::Value::Null).is_empty());
        // Malformed entries are skipped, matching Go's ignore-the-error.
        let garbage = json!({"hook": [1, 2, 3]});
        assert!(decode_mcp_approvals(&garbage).is_empty());

        let good = json!({
            "search": {
                "tools": [{
                    "name": "search",
                    "input_schema": {"type": "object"},
                    "schema_digest": "sha256:x"
                }],
                "approved_at": "2026-08-21T00:00:00Z",
                "approved_by": "user"
            }
        });
        let decoded = decode_mcp_approvals(&good);
        assert_eq!(decoded["search"].tools.len(), 1);
        assert_eq!(decoded["search"].approved_by, "user");
    }

    #[test]
    fn mcp_connections_for_skips_unapproved_hooks() {
        let mut inst = base_installation(json!({}));
        inst.granted_scopes = json!(["net:mcp.example.com"]);
        inst.manifest = json!({
            "key": "test-plugin",
            "config": {"search_credential": {"type": "secret", "label": "Credential"}},
            "contributes": {"hooks": [{
                "key": "search",
                "triggers": ["agent"],
                "transport": {"type": "mcp", "url": "https://mcp.example.com"}
            }]}
        });
        // No approval yet: installing is not the grant.
        assert!(mcp_connections_for(&inst).is_empty());
    }

    #[test]
    fn mcp_connections_for_builds_connection_from_approval() {
        let mut inst = base_installation(json!({
            "search": {
                "tools": [serde_json::to_value(tool("search")).unwrap()],
                "approved_at": "2026-08-21T00:00:00Z"
            }
        }));
        inst.granted_scopes = json!(["net:mcp.example.com"]);
        inst.manifest = json!({
            "key": "test-plugin",
            "config": {"search_credential": {"type": "secret", "label": "Credential"}},
            "contributes": {"hooks": [{
                "key": "search",
                "triggers": ["agent"],
                "transport": {"type": "mcp", "url": "https://mcp.example.com/mcp"}
            }]}
        });
        let connections = mcp_connections_for(&inst);
        assert_eq!(connections.len(), 1);
        let conn = &connections[0];
        assert!(conn.contribution_id.starts_with("plugin:"));
        // plugin_tool_name folds separators and appends a digest suffix
        // (anti-collision), so only assert the shape here.
        assert!(conn.contribution_key.ends_with("__search"));
        assert_eq!(conn.credential_header, "Authorization");
        assert_eq!(conn.endpoint_allowed_hosts, vec!["mcp.example.com"]);
        assert_eq!(conn.failure_policy, "optional");
        assert_eq!(conn.approved_tools[0].name, "search");
        assert!(conn.tool_schema_digest.starts_with("sha256:"));
    }

    #[test]
    fn mcp_connections_for_requires_agent_trigger() {
        let mut inst = base_installation(json!({
            "search": {
                "tools": [serde_json::to_value(tool("search")).unwrap()],
                "approved_at": "2026-08-21T00:00:00Z"
            }
        }));
        inst.manifest = json!({
            "key": "test-plugin",
            "contributes": {"hooks": [{
                "key": "search",
                "triggers": ["issue.created"],
                "transport": {"type": "mcp", "url": "https://mcp.example.com"}
            }]}
        });
        // Approved but not agent-triggered: not offered.
        assert!(mcp_connections_for(&inst).is_empty());
    }

    #[test]
    fn unreadable_manifest_yields_no_connections() {
        let mut inst = base_installation(json!({}));
        inst.manifest = json!("not-an-object");
        assert!(mcp_connections_for(&inst).is_empty());
    }

    #[test]
    fn empty_approval_is_not_offered() {
        let mut inst = base_installation(json!({
            "search": {"tools": [], "approved_at": "2026-08-21T00:00:00Z"}
        }));
        inst.granted_scopes = json!(["net:mcp.example.com"]);
        inst.manifest = json!({
            "key": "test-plugin",
            "contributes": {"hooks": [{
                "key": "search",
                "triggers": ["agent"],
                "transport": {"type": "mcp", "url": "https://mcp.example.com"}
            }]}
        });
        // An empty allow-list reads like "not approved" on the wire.
        assert!(mcp_connections_for(&inst).is_empty());
    }

    #[test]
    fn approved_mcp_tools_indexes_by_name() {
        let mut inst = base_installation(json!({
            "h": {
                "tools": [
                    serde_json::to_value(tool("a")).unwrap(),
                    serde_json::to_value(tool("b")).unwrap()
                ],
                "approved_at": "2026-08-21T00:00:00Z"
            }
        }));
        inst.manifest = json!({"key": "k", "contributes": {"hooks": []}});
        let by_name = approved_mcp_tools(&inst, "h");
        assert!(by_name.contains_key("a") && by_name.contains_key("b"));
        assert!(approved_mcp_tools(&inst, "missing").is_empty());
    }
}
