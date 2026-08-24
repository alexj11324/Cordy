//! Managed MCP conversion and fail-closed ACP capability negotiation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contract::AgentError;
use crate::mcp::managed_object;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum AcpMcpServer {
    Stdio {
        name: String,
        command: String,
        args: Vec<String>,
        env: Vec<AcpMcpValue>,
    },
    Remote {
        #[serde(rename = "type")]
        transport: String,
        name: String,
        url: String,
        headers: Vec<AcpMcpValue>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcpMcpValue {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Default, Deserialize)]
struct ManagedEntry {
    #[serde(default, rename = "type")]
    transport: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

/// Converts Cordy's canonical `{ "mcpServers": { ... } }` object into the
/// ACP v1 array shape. Malformed top-level ownership fails closed; malformed
/// individual entries are skipped so one bad optional tool does not prevent
/// every valid server from reaching the runtime.
pub fn build_acp_mcp_servers(config: Option<&Value>) -> Result<Vec<AcpMcpServer>, AgentError> {
    let Some(config) = managed_object(config).map_err(AgentError::InvalidConfig)? else {
        return Ok(Vec::new());
    };
    let Some(raw_servers) = config.get("mcpServers") else {
        warn_alternate_key(config);
        return Ok(Vec::new());
    };
    let servers = raw_servers.as_object().ok_or_else(|| {
        AgentError::InvalidConfig("managed MCP `mcpServers` must be a JSON object".to_string())
    })?;
    let mut output = Vec::with_capacity(servers.len());
    let mut names: Vec<&String> = servers.keys().collect();
    names.sort();
    for name in names {
        let entry = match serde_json::from_value::<ManagedEntry>(servers[name].clone()) {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(server = %name, error = %error, "skipping invalid managed MCP entry");
                continue;
            }
        };
        let command = entry.command.trim();
        if !command.is_empty() {
            output.push(AcpMcpServer::Stdio {
                name: name.clone(),
                command: command.to_string(),
                args: entry.args,
                env: pairs(entry.env),
            });
            continue;
        }
        let url = entry.url.trim();
        if !url.is_empty() {
            let transport = match entry.transport.trim().to_ascii_lowercase().as_str() {
                "sse" => "sse",
                "" | "http" | "streamable-http" | "http_streamable" => "http",
                _ => "http",
            };
            output.push(AcpMcpServer::Remote {
                transport: transport.to_string(),
                name: name.clone(),
                url: url.to_string(),
                headers: pairs(entry.headers),
            });
            continue;
        }
        tracing::warn!(server = %name, "skipping managed MCP entry without command or URL");
    }
    Ok(output)
}

fn pairs(values: BTreeMap<String, String>) -> Vec<AcpMcpValue> {
    values
        .into_iter()
        .map(|(name, value)| AcpMcpValue { name, value })
        .collect()
}

fn warn_alternate_key(config: &serde_json::Map<String, Value>) {
    for key in ["servers", "mcp", "mcp_servers"] {
        if config
            .get(key)
            .and_then(Value::as_object)
            .is_some_and(|entries| !entries.is_empty())
        {
            tracing::warn!(
                found_key = key,
                "managed MCP configuration has no `mcpServers`; runtime-native shapes are not forwarded"
            );
            return;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpMcpCapabilityDeclaration {
    Invalid,
    Omitted,
    Declared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcpMcpCapabilities {
    pub declaration: AcpMcpCapabilityDeclaration,
    pub http: bool,
    pub sse: bool,
}

impl AcpMcpCapabilities {
    fn invalid() -> Self {
        Self {
            declaration: AcpMcpCapabilityDeclaration::Invalid,
            http: false,
            sse: false,
        }
    }

    fn omitted() -> Self {
        Self {
            declaration: AcpMcpCapabilityDeclaration::Omitted,
            ..Self::invalid()
        }
    }
}

/// Distinguishes genuine omission from malformed capability declarations.
/// Both fail closed under ACP v1; only genuine omission can qualify for a
/// separately verified built-in-runtime exception.
pub fn parse_acp_mcp_capabilities(initialize_result: &Value) -> AcpMcpCapabilities {
    let Some(top) = initialize_result.as_object() else {
        return AcpMcpCapabilities::invalid();
    };
    let Some(agent_capabilities) = top.get("agentCapabilities") else {
        return AcpMcpCapabilities::omitted();
    };
    let Some(agent_capabilities) = agent_capabilities.as_object() else {
        return AcpMcpCapabilities::invalid();
    };
    let Some(mcp) = agent_capabilities.get("mcpCapabilities") else {
        return AcpMcpCapabilities::omitted();
    };
    let Some(mcp) = mcp.as_object() else {
        return AcpMcpCapabilities::invalid();
    };
    let http = optional_bool(mcp, "http");
    let sse = optional_bool(mcp, "sse");
    let (Ok(http), Ok(sse)) = (http, sse) else {
        return AcpMcpCapabilities::invalid();
    };
    AcpMcpCapabilities {
        declaration: AcpMcpCapabilityDeclaration::Declared,
        http,
        sse,
    }
}

fn optional_bool(object: &serde_json::Map<String, Value>, key: &str) -> Result<bool, ()> {
    object
        .get(key)
        .map_or(Ok(false), |value| value.as_bool().ok_or(()))
}

/// Keeps stdio entries and only those remote transports the runtime declared.
/// `verified_omission_exception` must come from exact built-in runtime identity,
/// never merely a protocol-family label used by a custom wrapper.
pub fn filter_acp_mcp_servers(
    servers: Vec<AcpMcpServer>,
    capabilities: AcpMcpCapabilities,
    verified_omission_exception: bool,
) -> Vec<AcpMcpServer> {
    if capabilities.declaration == AcpMcpCapabilityDeclaration::Omitted
        && verified_omission_exception
    {
        return servers;
    }
    servers
        .into_iter()
        .filter(|server| match server {
            AcpMcpServer::Stdio { .. } => true,
            AcpMcpServer::Remote { transport, .. } if transport == "http" => capabilities.http,
            AcpMcpServer::Remote { transport, .. } if transport == "sse" => capabilities.sse,
            AcpMcpServer::Remote { .. } => false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_is_deterministic_and_preserves_remote_credentials() {
        let config = serde_json::json!({
            "mcpServers": {
                "remote": {
                    "type": "streamable-http",
                    "url": "https://mcp.example.test",
                    "headers": {"X-Z": "last", "Authorization": "Bearer private"}
                },
                "local": {
                    "command": "node",
                    "args": ["server.js"],
                    "env": {"Z": "last", "A": "first"}
                },
                "invalid": {"args": ["missing-command"]}
            }
        });
        let servers = build_acp_mcp_servers(Some(&config))
            .unwrap_or_else(|error| panic!("convert MCP: {error}"));
        assert_eq!(servers.len(), 2);
        assert_eq!(
            serde_json::to_value(&servers).unwrap_or_else(|error| panic!("encode MCP: {error}")),
            serde_json::json!([
                {"name":"local","command":"node","args":["server.js"],"env":[{"name":"A","value":"first"},{"name":"Z","value":"last"}]},
                {"type":"http","name":"remote","url":"https://mcp.example.test","headers":[{"name":"Authorization","value":"Bearer private"},{"name":"X-Z","value":"last"}]}
            ])
        );
    }

    #[test]
    fn malformed_managed_ownership_fails_closed() {
        assert!(build_acp_mcp_servers(Some(&serde_json::json!([]))).is_err());
        assert!(build_acp_mcp_servers(Some(&serde_json::json!({"mcpServers": []}))).is_err());
        let alternate = build_acp_mcp_servers(Some(&serde_json::json!({
            "servers": {"x": {}}
        })))
        .unwrap_or_else(|error| panic!("alternate key should be ignored: {error}"));
        assert!(alternate.is_empty());
    }

    #[test]
    fn capabilities_distinguish_omitted_from_invalid_and_filter_fail_closed() {
        let omitted = parse_acp_mcp_capabilities(&serde_json::json!({"agentCapabilities": {}}));
        let invalid = parse_acp_mcp_capabilities(
            &serde_json::json!({"agentCapabilities":{"mcpCapabilities":{"http":"yes"}}}),
        );
        let declared = parse_acp_mcp_capabilities(
            &serde_json::json!({"agentCapabilities":{"mcpCapabilities":{"http":true,"sse":false}}}),
        );
        assert_eq!(omitted.declaration, AcpMcpCapabilityDeclaration::Omitted);
        assert_eq!(invalid.declaration, AcpMcpCapabilityDeclaration::Invalid);
        assert_eq!(declared.declaration, AcpMcpCapabilityDeclaration::Declared);

        let servers = vec![
            AcpMcpServer::Stdio {
                name: "local".into(),
                command: "node".into(),
                args: Vec::new(),
                env: Vec::new(),
            },
            AcpMcpServer::Remote {
                transport: "http".into(),
                name: "http".into(),
                url: "https://http".into(),
                headers: Vec::new(),
            },
            AcpMcpServer::Remote {
                transport: "sse".into(),
                name: "sse".into(),
                url: "https://sse".into(),
                headers: Vec::new(),
            },
        ];
        assert_eq!(
            filter_acp_mcp_servers(servers.clone(), invalid, false).len(),
            1
        );
        assert_eq!(
            filter_acp_mcp_servers(servers.clone(), declared, false).len(),
            2
        );
        assert_eq!(filter_acp_mcp_servers(servers, omitted, true).len(), 3);
    }
}
