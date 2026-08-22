//! MCP JSON-RPC discovery over the secure HTTP stack.
//!
//! Port of the discovery half of `server/pkg/remotemcp/client.go`
//! (`Discover`, `call`, `notify`, `readResponse`, `canonicalJSON`,
//! `ToolSetDigest`, `SupportedProtocolVersions`). The endpoint validation
//! and client construction halves already live in
//! [`crate::validate`] / [`crate::client`].
//!
//! Wire notes: JSON-RPC 2.0 over HTTP POST; the server may answer with a
//! plain JSON body or an `text/event-stream` whose first `data:` frame
//! carries the same JSON — both accepted, matching Go's `readResponse`.

use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

use crate::client::{new_secure_http_client, RequestBody};
use crate::error::Error;
use crate::types::{digest_bytes, Tool};
use crate::validate::{validate_public_https_endpoint, SystemResolver};

/// MCP protocol revisions this build speaks, most preferred first. Discover
/// offers the first and accepts any of them.
pub fn supported_protocol_versions() -> Vec<&'static str> {
    vec!["2025-03-26", "2024-11-05"]
}

/// Extra headers attached to every discovery request (the plugin's
/// credential header, if any). Header names are matched case-insensitively
/// by the http crate, mirroring Go's `http.Header`.
pub type ExtraHeaders = Vec<(String, String)>;

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct RpcResponse {
    #[allow(dead_code)]
    #[serde(default)]
    jsonrpc: String,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct DiscoveredTool {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "inputSchema")]
    input_schema: Value,
}

/// Asks an MCP server what tools it offers and returns them sorted by name
/// with the set digest.
///
/// Read-only: nothing is adopted by discovering it. `allowed_hosts` is the
/// consented `net:` scope set; an empty set fails closed in
/// [`validate_public_https_endpoint`]'s caller — Go relies on the service
/// layer refusing before it gets here.
pub async fn discover(
    raw_endpoint: &str,
    allowed_hosts: &[String],
    protocol_versions: Vec<&str>,
    headers: &ExtraHeaders,
) -> Result<(Vec<Tool>, String), Error> {
    let endpoint =
        validate_public_https_endpoint(raw_endpoint, allowed_hosts, Some(&SystemResolver)).await?;
    let client = new_secure_http_client(&endpoint);
    let mut session_id = String::new();
    // An empty list means "whatever this build supports", not "nothing is
    // acceptable". The response is checked against this same slice further
    // down, so leaving it empty would reject every server that answers
    // correctly — which is exactly what it did to the first plugin that
    // used this path.
    let protocol_versions: Vec<String> = if protocol_versions.is_empty() {
        supported_protocol_versions()
            .into_iter()
            .map(String::from)
            .collect()
    } else {
        protocol_versions.into_iter().map(String::from).collect()
    };
    let protocol_version = protocol_versions[0].clone();
    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": protocol_version,
            "capabilities": {},
            "clientInfo": {"name": "cordy-plugin-review", "version": "1"},
        },
    });
    let (initialize_response, response_session) =
        call(&client, &endpoint, headers, &session_id, &initialize)
            .await
            .map_err(|e| Error::Request(format!("initialize remote MCP: {e}")))?;
    session_id = response_session;
    let negotiated = initialize_response
        .result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !protocol_versions.iter().any(|v| v == &negotiated) {
        return Err(Error::Request(format!(
            "remote MCP negotiated unsupported protocol version \"{negotiated}\""
        )));
    }
    notify(
        &client,
        &endpoint,
        headers,
        &session_id,
        &json!({
            "jsonrpc": "2.0", "method": "notifications/initialized",
            "params": {},
        }),
    )
    .await
    .map_err(|e| Error::Request(format!("confirm remote MCP initialization: {e}")))?;
    let (tools_response, _) = call(
        &client,
        &endpoint,
        headers,
        &session_id,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    )
    .await
    .map_err(|e| Error::Request(format!("list remote MCP tools: {e}")))?;
    let result: DiscoveredToolsResult = serde_json::from_value(tools_response.result)
        .map_err(|e| Error::Request(format!("decode tools/list result: {e}")))?;
    let mut tools: Vec<Tool> = Vec::with_capacity(result.tools.len());
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for tool in &result.tools {
        if tool.name.trim().is_empty() || seen.contains(tool.name.as_str()) {
            return Err(Error::Request(
                "remote MCP returned an invalid or duplicate tool name".to_string(),
            ));
        }
        seen.insert(tool.name.as_str());
        let canonical = canonical_json(&tool.input_schema)
            .map_err(|e| Error::Request(format!("tool \"{}\" input schema: {e}", tool.name)))?;
        let digest =
            digest_bytes(&serde_json::to_vec(&canonical).map_err(|e| {
                Error::Request(format!("tool \"{}\" input schema: {e}", tool.name))
            })?);
        tools.push(Tool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: canonical,
            schema_digest: digest,
            risk: String::new(),
        });
    }
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let digest = tool_set_digest(&tools)?;
    Ok((tools, digest))
}

#[derive(Debug, Deserialize)]
struct DiscoveredToolsResult {
    #[serde(default)]
    tools: Vec<DiscoveredTool>,
}

/// Pins the whole approved set, not just each tool. A schema that cannot
/// canonicalize fails the call (Go returns the error; the service layer's
/// `toolSetDigest` degrades to the empty string).
pub fn tool_set_digest(tools: &[Tool]) -> Result<String, Error> {
    let mut copy: Vec<Tool> = tools.to_vec();
    for tool in &mut copy {
        tool.input_schema = canonical_json(&tool.input_schema)
            .map_err(|e| Error::Request(format!("tool \"{}\" input schema: {e}", tool.name)))?;
    }
    copy.sort_by(|a, b| a.name.cmp(&b.name));
    let raw = serde_json::to_vec(&copy).map_err(|e| Error::Request(e.to_string()))?;
    Ok(digest_bytes(&raw))
}

/// Canonicalizes a JSON value the way Go's roundtrip does: decode into the
/// generic value, re-encode. Map key order follows serde_json's BTreeMap
/// ordering, which is a strict superset of Go's sorted-key guarantee for
/// the digest's purpose (both are deterministic).
fn canonical_json(raw: &Value) -> Result<Value, String> {
    let value = if raw.is_null() {
        // Go: an empty RawMessage becomes `{"type":"object"}`.
        json!({"type": "object"})
    } else {
        raw.clone()
    };
    // Re-encode through a string to normalize number formatting the same
    // way Go's json.Marshal normalizes after Unmarshal into `any`.
    let text = serde_json::to_string(&value).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

async fn notify(
    client: &crate::client::SecureHttpClient,
    endpoint: &Url,
    headers: &ExtraHeaders,
    session_id: &str,
    payload: &Value,
) -> Result<(), Error> {
    let request = build_request(
        endpoint,
        headers,
        session_id,
        &serde_json::to_vec(payload).map_err(|e| Error::Request(e.to_string()))?,
    )?;
    let response = client.send(request).await?;
    if !response.status().is_success() {
        return Err(Error::Request(format!(
            "remote MCP returned HTTP {}",
            response.status().as_u16()
        )));
    }
    Ok(())
}

async fn call(
    client: &crate::client::SecureHttpClient,
    endpoint: &Url,
    headers: &ExtraHeaders,
    session_id: &str,
    payload: &Value,
) -> Result<(RpcResponse, String), Error> {
    let request = build_request(
        endpoint,
        headers,
        session_id,
        &serde_json::to_vec(payload).map_err(|e| Error::Request(e.to_string()))?,
    )?;
    let response = client.send(request).await?;
    if !response.status().is_success() {
        return Err(Error::Request(format!(
            "remote MCP returned HTTP {}",
            response.status().as_u16()
        )));
    }
    let session = response
        .headers()
        .get("Mcp-Session-Id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let raw = read_response(&response)?;
    let decoded: RpcResponse = serde_json::from_slice(&raw)
        .map_err(|e| Error::Request(format!("decode JSON-RPC response: {e}")))?;
    if let Some(err) = &decoded.error {
        return Err(Error::Request(format!(
            "remote MCP error {}: {}",
            err.code, err.message
        )));
    }
    Ok((decoded, session))
}

fn build_request(
    endpoint: &Url,
    headers: &ExtraHeaders,
    session_id: &str,
    body: &[u8],
) -> Result<http::Request<RequestBody>, Error> {
    let mut builder = http::Request::builder()
        .method(http::Method::POST)
        .uri(endpoint.as_str())
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    for (name, value) in headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    if !session_id.is_empty() {
        builder = builder.header("Mcp-Session-Id", session_id);
    }
    builder
        .body(RequestBody::from(body.to_vec()))
        .map_err(|e| Error::Request(e.to_string()))
}

/// Reads the body: SSE responses return the first non-empty `data:` frame's
/// JSON; everything else is returned verbatim. The size cap is already
/// enforced by [`crate::client::SecureHttpClient::send`].
fn read_response(response: &http::Response<Vec<u8>>) -> Result<Vec<u8>, Error> {
    let content_type = response
        .headers()
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if content_type.starts_with("text/event-stream") {
        for line in String::from_utf8_lossy(response.body()).lines() {
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if !data.is_empty() {
                    return Ok(data.as_bytes().to_vec());
                }
            }
        }
        return Err(Error::Request(
            "remote MCP SSE response contained no data".to_string(),
        ));
    }
    Ok(response.body().clone())
}

/// True when `wanted` is in `values` (helper mirroring Go's containsString,
/// kept for parity with the Go test table).
pub fn contains_string(values: &[String], wanted: &str) -> bool {
    values.iter().any(|v| v == wanted)
}

// host_allowed is re-exported here because Go's Discover path reaches it via
// the same package; the service layer uses validate::host_allowed directly.
#[allow(unused_imports)]
use crate::validate::host_allowed as _host_allowed_reexport;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_versions_match_go() {
        assert_eq!(
            supported_protocol_versions(),
            vec!["2025-03-26", "2024-11-05"]
        );
    }

    #[test]
    fn canonical_json_defaults_empty_to_object_schema() {
        assert_eq!(
            canonical_json(&Value::Null).unwrap(),
            json!({"type": "object"})
        );
        assert_eq!(
            canonical_json(&json!({"type": "object", "x": 1})).unwrap(),
            json!({"type": "object", "x": 1})
        );
    }

    #[test]
    fn canonical_json_is_deterministic_across_key_order() {
        let a = canonical_json(&json!({"b": 1, "a": 2})).unwrap();
        let b = canonical_json(&json!({"a": 2, "b": 1})).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            digest_bytes(serde_json::to_vec(&a).unwrap().as_slice()),
            digest_bytes(serde_json::to_vec(&b).unwrap().as_slice())
        );
    }

    #[test]
    fn tool_set_digest_sorts_and_canonicalizes() {
        let tools = vec![
            Tool {
                name: "b".into(),
                description: String::new(),
                input_schema: json!({"type": "string"}),
                schema_digest: "sha256:x".into(),
                risk: String::new(),
            },
            Tool {
                name: "a".into(),
                description: String::new(),
                input_schema: json!({"type": "object"}),
                schema_digest: "sha256:y".into(),
                risk: String::new(),
            },
        ];
        let digest = tool_set_digest(&tools).unwrap();
        assert!(digest.starts_with("sha256:"));
        // Order-independent: same set in reverse yields the same digest.
        let reversed: Vec<Tool> = tools.iter().rev().cloned().collect();
        assert_eq!(digest, tool_set_digest(&reversed).unwrap());
    }

    #[test]
    fn read_response_parses_first_sse_data_frame() {
        let response = http::Response::builder()
            .header("Content-Type", "text/event-stream")
            .body(b"data: {\"jsonrpc\":\"2.0\"}\n\n".to_vec())
            .unwrap();
        let raw = read_response(&response).unwrap();
        assert_eq!(raw, b"{\"jsonrpc\":\"2.0\"}");
    }

    #[test]
    fn read_response_passes_plain_json_through() {
        let response = http::Response::builder()
            .header("Content-Type", "application/json")
            .body(b"{\"ok\":true}".to_vec())
            .unwrap();
        assert_eq!(read_response(&response).unwrap(), b"{\"ok\":true}");
    }

    #[test]
    fn read_response_rejects_sse_without_data() {
        let response = http::Response::builder()
            .header("Content-Type", "text/event-stream")
            .body(b"event: ping\n\n".to_vec())
            .unwrap();
        assert!(read_response(&response).is_err());
    }

    #[test]
    fn contains_string_matches_go_helper() {
        let values = vec!["2025-03-26".to_string()];
        assert!(contains_string(&values, "2025-03-26"));
        assert!(!contains_string(&values, "2024-11-05"));
    }
}
