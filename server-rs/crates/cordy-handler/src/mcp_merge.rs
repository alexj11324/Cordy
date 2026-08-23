//! MCP config merge helpers — port of `server/internal/handler/workspace_mcp.go`
//! (`ResolveAgentMcpConfig`) and `mcp_overlay.go` (`mergeMCPOverlay`).
//!
//! Precedence: bound workspace servers < agent's own servers < per-task overlay.
//! The contract (GH #6062, MUL-5421):
//! - Only servers explicitly bound to the agent and left enabled are folded in.
//! - The agent's own entry WINS on a name collision with a bound server.
//! - An agent with no bindings and no config of its own resolves to None.
//! - Shape is normalized onto the canonical `mcpServers` container (legacy `mcp`
//!   entries folded in) so the daemon's runtime merge reads one container.

use serde_json::{Map, Value};

/// The two top-level keys an mcp_config document may declare servers under
/// (Go `mcpServerContainers`).
const MCP_SERVER_CONTAINERS: [&str; 2] = ["mcp", "mcpServers"];

/// Go `hasManagedJSON`: reports whether a raw JSON column carries an actual
/// managed payload (non-empty and not the literal `null`).
fn has_managed_json(raw: Option<&Value>) -> bool {
    !matches!(raw, None | Some(Value::Null))
}

/// Go `passthroughAgentMcpConfig`.
fn passthrough_agent_mcp_config(agent_mcp_config: Option<&Value>) -> Option<Value> {
    if !has_managed_json(agent_mcp_config) {
        return None;
    }
    agent_mcp_config.cloned()
}

/// Go `unmarshalServerMap`: decodes an `mcpServers` sub-object into a map keyed
/// by server name. Absent/null yields an empty map. Rejects empty names and
/// non-object entries (every runtime expects objects and would 500 downstream).
fn unmarshal_server_map(raw: Option<&Value>) -> Result<Map<String, Value>, String> {
    let Some(Value::Object(m)) = raw else {
        return Ok(Map::new());
    };
    for (name, server) in m {
        if name.is_empty() {
            return Err("mcp server name must not be empty".to_string());
        }
        // Reject non-object server entries early — every runtime expects the
        // inner value to be an object and would 500 in the sidecar generator
        // otherwise. Mirrors parseCursorManagedMcpServers' guard.
        if !server.is_object() {
            return Err(format!("mcpServers.{name} must be a JSON object"));
        }
    }
    Ok(m.clone())
}

/// Go `parseMcpDocument`: decodes an mcp_config document into its top-level
/// object plus the set of server names it declares across BOTH containers.
fn parse_mcp_document(raw: &Value) -> Result<(Map<String, Value>, Vec<String>), String> {
    let doc = raw.as_object().cloned().unwrap_or_default();
    let mut names = Vec::new();
    for container in MCP_SERVER_CONTAINERS {
        let servers =
            unmarshal_server_map(doc.get(container)).map_err(|e| format!("{container}: {e}"))?;
        for name in servers.keys() {
            if !names.iter().any(|n| n == name) {
                names.push(name.clone());
            }
        }
    }
    Ok((doc, names))
}

pub struct WorkspaceMcpBinding {
    pub name: String,
    pub config: Value,
}

/// Port of Go `ResolveAgentMcpConfig`: folds workspace MCP servers BOUND to an
/// agent into the agent's own mcp_config, normalized onto `mcpServers`.
/// Failure returns the original config unchanged alongside the error so a
/// malformed shared entry never takes away servers the agent runs with today.
pub fn resolve_agent_mcp_config(
    bound: &[WorkspaceMcpBinding],
    agent_mcp_config: Option<&Value>,
) -> Result<Option<Value>, String> {
    if bound.is_empty() {
        return Ok(passthrough_agent_mcp_config(agent_mcp_config));
    }
    let mut shared: Map<String, Value> = Map::new();
    for server in bound {
        if server.name.is_empty() || !has_managed_json(Some(&server.config)) {
            continue;
        }
        shared.insert(server.name.clone(), server.config.clone());
    }
    if shared.is_empty() {
        return Ok(passthrough_agent_mcp_config(agent_mcp_config));
    }

    let Some(agent_cfg) =
        has_managed_json(agent_mcp_config).then(|| agent_mcp_config.expect("checked").clone())
    else {
        // The agent declares nothing of its own: it runs with exactly the
        // servers it was given.
        let mut root = Map::new();
        root.insert("mcpServers".to_string(), Value::Object(shared));
        return Ok(Some(Value::Object(root)));
    };

    let (agent_doc, agent_servers) =
        parse_mcp_document(&agent_cfg).map_err(|e| format!("resolve agent mcp_config: {e}"))?;

    let mut merged: Map<String, Value> = Map::new();
    for (name, server) in &shared {
        // The agent may declare this name in either container; agentServers
        // spans both, so this check covers each spelling.
        if agent_servers.contains(name) {
            continue;
        }
        merged.insert(name.clone(), server.clone());
    }
    // Fold the agent's own entries in, legacy container first so a name present
    // in both resolves to the canonical one.
    for container in MCP_SERVER_CONTAINERS {
        let own = unmarshal_server_map(agent_doc.get(container))
            .map_err(|e| format!("resolve agent mcp_config: agent {container}: {e}"))?;
        for (name, server) in own {
            merged.insert(name, server);
        }
    }

    let mut out = Map::new();
    for (k, v) in &agent_doc {
        // Both containers are consumed into the canonical map above; leaving
        // the legacy key behind would hand the daemon a second, stale copy.
        if k == "mcpServers" || k == "mcp" {
            continue;
        }
        out.insert(k.clone(), v.clone());
    }
    out.insert("mcpServers".to_string(), Value::Object(merged));
    Ok(Some(Value::Object(out)))
}

/// Port of Go `mergeMCPOverlay`: layers the per-task overlay (Composio) on top
/// of the agent's saved mcp_config. Overlay wins on server-name collisions
/// because it carries the live user-scoped session URL. On malformed input the
/// agent config is returned unchanged alongside the error.
pub fn merge_mcp_overlay(
    agent_mcp_config: Option<&Value>,
    overlay: Option<&Value>,
) -> Result<Option<Value>, String> {
    if !has_managed_json(overlay) {
        return Ok(passthrough_agent_mcp_config(agent_mcp_config));
    }
    let overlay = overlay.expect("checked");
    if !has_managed_json(agent_mcp_config) {
        // Re-marshal the overlay alone so the daemon receives the exact
        // canonical shape (the input may have arbitrary JSONB whitespace).
        let o_cfg = overlay
            .as_object()
            .ok_or_else(|| "merge mcp overlay: parse overlay".to_string())?;
        return Ok(Some(Value::Object(o_cfg.clone())));
    }
    let agent_cfg = agent_mcp_config.expect("checked");

    let a_cfg = agent_cfg
        .as_object()
        .ok_or_else(|| "merge mcp overlay: parse agent mcp_config".to_string())?;
    let o_cfg = overlay
        .as_object()
        .ok_or_else(|| "merge mcp overlay: parse overlay".to_string())?;

    // Pull each side's `mcpServers` sub-map, default to empty so a
    // well-formed top level with no servers is treated like absent.
    let a_servers = unmarshal_server_map(a_cfg.get("mcpServers"))
        .map_err(|e| format!("merge mcp overlay: agent mcpServers: {e}"))?;
    let o_servers = unmarshal_server_map(o_cfg.get("mcpServers"))
        .map_err(|e| format!("merge mcp overlay: overlay mcpServers: {e}"))?;

    let mut merged = a_servers;
    // Overlay wins on collisions.
    for (k, v) in o_servers {
        merged.insert(k, v);
    }

    // Rebuild: keep any non-mcpServers top-level keys from the agent config,
    // then write the merged mcpServers map back.
    let mut out = Map::new();
    for (k, v) in a_cfg {
        if k == "mcpServers" {
            continue;
        }
        out.insert(k.clone(), v.clone());
    }
    if !merged.is_empty() {
        out.insert("mcpServers".to_string(), Value::Object(merged));
    }
    if out.is_empty() {
        return Ok(None);
    }
    Ok(Some(Value::Object(out)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn binding(name: &str, config: Value) -> WorkspaceMcpBinding {
        WorkspaceMcpBinding {
            name: name.to_string(),
            config,
        }
    }

    #[test]
    fn resolve_with_no_bound_servers_passes_through() {
        let cfg = json!({"mcp": {"a": {"command": "x"}}});
        let out = resolve_agent_mcp_config(&[], Some(&cfg)).unwrap();
        assert_eq!(out.unwrap(), cfg);
    }

    #[test]
    fn resolve_agent_without_config_gets_only_bound() {
        let bound = vec![binding("shared", json!({"command": "shared-cmd"}))];
        let out = resolve_agent_mcp_config(&bound, None).unwrap();
        assert_eq!(
            out.unwrap(),
            json!({"mcpServers": {"shared": {"command": "shared-cmd"}}})
        );
    }

    #[test]
    fn resolve_agent_own_entry_wins_and_normalizes_containers() {
        let bound = vec![
            binding("shared", json!({"command": "shared"})),
            binding("only-bound", json!({"command": "b"})),
        ];
        let agent = json!({"mcp": {"shared": {"command": "own"}}});
        let out = resolve_agent_mcp_config(&bound, Some(&agent))
            .unwrap()
            .unwrap();
        let servers = out.get("mcpServers").unwrap().as_object().unwrap();
        assert_eq!(servers["shared"], json!({"command": "own"}));
        assert_eq!(servers["only-bound"], json!({"command": "b"}));
        // Legacy key consumed into the canonical container.
        assert!(out.get("mcp").is_none());
        // Other top-level keys preserved.
        let agent_doc = json!({"top": true});
        let out2 = resolve_agent_mcp_config(&bound, Some(&agent_doc))
            .unwrap()
            .unwrap();
        assert_eq!(out2["top"], json!(true));
    }

    #[test]
    fn canonical_container_wins_when_both_spellings_define_the_same_server() {
        let bound = vec![binding("bound", json!({"command": "bound"}))];
        let agent = json!({
            "mcp": {"same": {"command": "legacy"}},
            "mcpServers": {"same": {"command": "canonical"}}
        });
        let out = resolve_agent_mcp_config(&bound, Some(&agent))
            .unwrap()
            .unwrap();
        assert_eq!(out["mcpServers"]["same"], json!({"command": "canonical"}));
    }

    #[test]
    fn merge_overlay_wins_on_collision_and_keeps_other_keys() {
        let agent = json!({
            "mode": "x",
            "mcpServers": {"composio": {"url": "stale"}, "own": {"cmd": "c"}}
        });
        let overlay = json!({"mcpServers": {"composio": {"url": "live"}}});
        let out = merge_mcp_overlay(Some(&agent), Some(&overlay))
            .unwrap()
            .unwrap();
        assert_eq!(out["mode"], json!("x"));
        let servers = out.get("mcpServers").unwrap().as_object().unwrap();
        assert_eq!(servers["composio"], json!({"url": "live"}));
        assert_eq!(servers["own"], json!({"cmd": "c"}));
    }

    #[test]
    fn merge_overlay_alone_normalizes_to_canonical_shape() {
        let overlay = json!({"mcpServers": {"a": {"b": 1}}});
        let out = merge_mcp_overlay(None, Some(&overlay)).unwrap();
        assert_eq!(out.unwrap(), overlay);
    }

    #[test]
    fn merge_null_overlay_is_passthrough() {
        let agent = json!({"mcpServers": {}});
        let out = merge_mcp_overlay(Some(&agent), None).unwrap();
        assert_eq!(out.unwrap(), agent);
    }

    #[test]
    fn malformed_overlay_returns_agent_config_unchanged() {
        let agent = json!({"mcpServers": {"a": {}}});
        // Non-object mcpServers entry → error path keeps agent config.
        let overlay = json!({"mcpServers": {"a": "scalar"}});
        let out = merge_mcp_overlay(Some(&agent), Some(&overlay));
        assert!(out.is_err());
    }
}
