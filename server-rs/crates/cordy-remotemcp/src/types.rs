//! Wire types shared by Remote MCP discovery and the daemon broker.
//!
//! JSON field names are part of the stable wire contract.

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Marks a connection contributed by an installed Plugin rather than by a
/// workspace's own Remote MCP configuration.
///
/// The two kinds share one [`Connection`] shape and one broker, but their
/// credentials live in different places and are served by different routes.
/// The daemon holds only the contribution id at dial time, so the id is what
/// has to say which kind it is — inferring it from the string's shape would
/// make a cloud-issued id that happened to contain a colon resolve against
/// the plugin route.
pub const PLUGIN_CONTRIBUTION_PREFIX: &str = "plugin:";

/// One remote MCP tool pinned by an administrator. `schema_digest` freezes
/// the exact input schema that was approved so the daemon broker can reject a
/// server that later swaps a tool's contract underneath a running task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(rename = "input_schema", default)]
    pub input_schema: serde_json::Value,
    #[serde(rename = "schema_digest")]
    pub schema_digest: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub risk: String,
}

/// Claim-time, task-scoped connection metadata. Credentials are intentionally
/// absent from this wire type and are resolved just-in-time by the daemon
/// broker.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Connection {
    #[serde(rename = "installation_id")]
    pub installation_id: String,
    #[serde(rename = "contribution_id")]
    pub contribution_id: String,
    #[serde(rename = "contribution_key")]
    pub contribution_key: String,
    #[serde(rename = "config_id")]
    pub config_id: String,
    #[serde(rename = "config_revision")]
    pub config_revision: i64,
    pub endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_config: Option<serde_json::Value>,
    pub transport: String,
    #[serde(rename = "protocol_versions")]
    pub protocol_versions: Vec<String>,
    #[serde(
        rename = "endpoint_allowed_hosts",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub endpoint_allowed_hosts: Vec<String>,
    #[serde(rename = "credential_header", skip_serializing_if = "String::is_empty")]
    pub credential_header: String,
    #[serde(rename = "approved_tools")]
    pub approved_tools: Vec<Tool>,
    #[serde(rename = "tool_schema_digest")]
    pub tool_schema_digest: String,
    #[serde(rename = "failure_policy")]
    pub failure_policy: String,
}

/// Content digest used to pin approved tool schemas.
pub fn digest_bytes(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    format!("sha256:{}", hex::encode(digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_json_field_names_match_go_tags() {
        let tool = Tool {
            name: "search".into(),
            description: String::new(),
            input_schema: serde_json::json!({ "type": "object" }),
            schema_digest: "sha256:abc".into(),
            risk: "read".into(),
        };
        let json = serde_json::to_value(&tool).unwrap();
        assert_eq!(json["name"], "search");
        assert_eq!(json["input_schema"]["type"], "object");
        assert_eq!(json["schema_digest"], "sha256:abc");
        assert_eq!(json["risk"], "read");
        assert!(json.get("description").is_none(), "omitempty parity");
    }

    #[test]
    fn connection_roundtrips_go_wire_shape() {
        let raw = r#"{"installation_id":"i","contribution_id":"c","contribution_key":"k","config_id":"cfg","config_revision":3,"endpoint":"https://mcp.example.com","transport":"streamable-http","protocol_versions":["2025-03-26"],"approved_tools":[],"tool_schema_digest":"sha256:x","failure_policy":"fail"}"#;
        let connection: Connection = serde_json::from_str(raw).unwrap();
        assert_eq!(connection.config_revision, 3);
        assert!(connection.endpoint_allowed_hosts.is_empty());
        let encoded = serde_json::to_value(&connection).unwrap();
        assert!(encoded.get("public_config").is_none(), "omitempty parity");
        assert!(
            encoded.get("credential_header").is_none(),
            "omitempty parity"
        );
    }

    #[test]
    fn digest_bytes_uses_sha256_hex_prefix_form() {
        let digest = digest_bytes(b"hello");
        assert!(digest.starts_with("sha256:"));
        assert_eq!(digest.len(), "sha256:".len() + 64);
        assert_eq!(digest_bytes(b"hello"), digest);
    }
}
