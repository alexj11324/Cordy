//! OpenCode-specific MCP config translation and validation.
//!
//! Cordy's managed MCP payload may use the Claude-style `mcpServers` shape,
//! while OpenCode consumes a native `mcp` map. This module validates both
//! shapes before placing the translated map in `OPENCODE_CONFIG_CONTENT`,
//! keeping that boundary explicit and fail-closed.

use serde_json::{Map, Value};

use crate::contract::AgentError;

pub fn build_opencode_mcp_config_content(
    config: Option<&Value>,
) -> Result<Option<String>, AgentError> {
    let Some(config) = config.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let object = config.as_object().ok_or_else(|| {
        AgentError::InvalidConfig("opencode mcp_config must be a JSON object".to_string())
    })?;
    let servers = translate_mcp_config(object).map_err(AgentError::InvalidConfig)?;
    if servers.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(&serde_json::json!({ "mcp": servers }))
        .map(Some)
        .map_err(|error| {
            AgentError::InvalidConfig(format!("opencode mcp_config: marshal: {error}"))
        })
}

fn translate_mcp_config(object: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let mcp_servers = optional_object(object, "mcpServers")?;
    let native = optional_object(object, "mcp")?;
    let mut servers = Map::new();

    if mcp_servers.is_none_or(|servers| servers.is_empty()) {
        let Some(native) = native else {
            return Ok(servers);
        };
        for (name, entry) in native {
            servers.insert(name.clone(), validate_native_entry(name, entry)?);
        }
        return Ok(servers);
    }

    if let Some(native) = native {
        for (name, entry) in native {
            servers.insert(name.clone(), validate_native_entry(name, entry)?);
        }
    }
    if let Some(mcp_servers) = mcp_servers {
        for (name, server) in mcp_servers {
            let server = server.as_object().ok_or_else(|| {
                format!("opencode mcp_config: server {name:?}: entry must be a JSON object")
            })?;
            let translated = translate_claude_server(name, server)?;
            servers.insert(name.clone(), validate_native_entry(name, &translated)?);
        }
    }
    Ok(servers)
}

fn optional_object<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a Map<String, Value>>, String> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    value
        .as_object()
        .map(Some)
        .ok_or_else(|| format!("opencode mcp_config: `{key}` must be an object"))
}

fn translate_claude_server(name: &str, server: &Map<String, Value>) -> Result<Value, String> {
    if let Some(url) = server
        .get("url")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
    {
        let mut out = Map::from_iter([
            ("type".to_string(), Value::String("remote".to_string())),
            ("url".to_string(), Value::String(url.to_string())),
        ]);
        copy_bool_if_present(&mut out, server, "enabled");
        copy_if_present(&mut out, server, "headers");
        copy_if_present(&mut out, server, "oauth");
        copy_if_present(&mut out, server, "timeout");
        return Ok(Value::Object(out));
    }

    let command = opencode_command(server)
        .map_err(|error| format!("opencode mcp_config: server {name:?}: {error}"))?;
    if command.is_empty() {
        return Err(format!(
            "opencode mcp_config: server {name:?} has neither url nor command"
        ));
    }
    let mut out = Map::from_iter([
        ("type".to_string(), Value::String("local".to_string())),
        (
            "command".to_string(),
            Value::Array(command.into_iter().map(Value::String).collect()),
        ),
    ]);
    copy_bool_if_present(&mut out, server, "enabled");
    if let Some(environment) = server.get("env") {
        out.insert("environment".to_string(), environment.clone());
    } else {
        copy_if_present(&mut out, server, "environment");
    }
    copy_if_present(&mut out, server, "timeout");
    Ok(Value::Object(out))
}

fn opencode_command(server: &Map<String, Value>) -> Result<Vec<String>, String> {
    let Some(command) = server.get("command") else {
        return Ok(Vec::new());
    };
    match command {
        Value::String(command) => {
            let mut result = vec![command.clone()];
            result.extend(string_array(server.get("args"), "args")?);
            Ok(result)
        }
        Value::Array(command) => string_array_items(command, "command"),
        _ => Err("command must be a string or string array".to_string()),
    }
}

fn string_array(value: Option<&Value>, field: &str) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(items) = value.as_array() else {
        return Err(format!("{field} must be an array"));
    };
    string_array_items(items, field)
}

fn string_array_items(items: &[Value], field: &str) -> Result<Vec<String>, String> {
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{field} must contain only strings"))
        })
        .collect()
}

fn copy_if_present(dst: &mut Map<String, Value>, src: &Map<String, Value>, key: &str) {
    if let Some(value) = src.get(key) {
        dst.insert(key.to_string(), value.clone());
    }
}

fn copy_bool_if_present(dst: &mut Map<String, Value>, src: &Map<String, Value>, key: &str) {
    if src.get(key).is_some_and(Value::is_boolean) {
        copy_if_present(dst, src, key);
    }
}

fn validate_native_entry(name: &str, raw: &Value) -> Result<Value, String> {
    let object = raw.as_object().ok_or_else(|| {
        format!("opencode mcp_config: server {name:?}: entry must be a JSON object")
    })?;
    let type_value = object.get("type");
    let type_name = match type_value {
        Some(value) => value.as_str().ok_or_else(|| {
            format!("opencode mcp_config: server {name:?}: `type` must be a string, got {value}")
        })?,
        None => "",
    };
    match type_name {
        "local" => validate_local(name, object)?,
        "remote" => validate_remote(name, object)?,
        "" if type_value.is_none() => validate_enabled_only(name, object)?,
        "" => {
            return Err(format!(
                "opencode mcp_config: server {name:?}: missing required field `type`"
            ));
        }
        other => {
            return Err(format!(
                "opencode mcp_config: server {name:?}: invalid type {other:?} (must be \"local\" or \"remote\")"
            ));
        }
    }
    Ok(raw.clone())
}

fn validate_local(name: &str, object: &Map<String, Value>) -> Result<(), String> {
    reject_unknown(
        name,
        object,
        ["type", "command", "environment", "enabled", "timeout"],
    )?;
    let command = object.get("command").ok_or_else(|| {
        format!(
            "opencode mcp_config: server {name:?}: local server missing required field `command`"
        )
    })?;
    if command.as_array().is_none_or(Vec::is_empty) {
        return Err(format!(
            "opencode mcp_config: server {name:?}: local `command` must be a non-empty string array"
        ));
    }
    require_string_array(name, command, "command")?;
    if let Some(environment) = object.get("environment") {
        require_string_map(name, environment, "environment")?;
    }
    require_bool(name, object.get("enabled"), "enabled")?;
    require_positive_integer(name, object.get("timeout"), "timeout")
}

fn validate_remote(name: &str, object: &Map<String, Value>) -> Result<(), String> {
    reject_unknown(
        name,
        object,
        ["type", "url", "headers", "oauth", "enabled", "timeout"],
    )?;
    if object
        .get("url")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(format!(
            "opencode mcp_config: server {name:?}: remote server missing required field `url`"
        ));
    }
    if let Some(headers) = object.get("headers") {
        require_string_map(name, headers, "headers")?;
    }
    if let Some(oauth) = object.get("oauth") {
        validate_oauth(name, oauth)?;
    }
    require_bool(name, object.get("enabled"), "enabled")?;
    require_positive_integer(name, object.get("timeout"), "timeout")
}

fn validate_enabled_only(name: &str, object: &Map<String, Value>) -> Result<(), String> {
    reject_unknown(name, object, ["enabled"])?;
    if !object.get("enabled").is_some_and(Value::is_boolean) {
        return Err(format!(
            "opencode mcp_config: server {name:?}: missing required field `type` (must be \"local\" or \"remote\", or use bare {{\"enabled\": bool}} to override an inherited server)"
        ));
    }
    Ok(())
}

fn validate_oauth(name: &str, oauth: &Value) -> Result<(), String> {
    if oauth == &Value::Bool(false) {
        return Ok(());
    }
    let object = oauth.as_object().ok_or_else(|| {
        format!(
            "opencode mcp_config: server {name:?}: `oauth` must be an object or `false`, got {oauth}"
        )
    })?;
    reject_unknown(
        name,
        object,
        [
            "clientId",
            "clientSecret",
            "scope",
            "callbackPort",
            "redirectUri",
        ],
    )?;
    for field in ["clientId", "clientSecret", "scope", "redirectUri"] {
        if let Some(value) = object.get(field) {
            if !value.is_string() && !value.is_null() {
                return Err(format!(
                    "opencode mcp_config: server {name:?}: oauth `{field}` must be a string"
                ));
            }
        }
    }
    if let Some(port) = object.get("callbackPort") {
        let valid = port.is_null()
            || port
                .as_i64()
                .is_some_and(|port| (1..=65_535).contains(&port));
        if !valid {
            return Err(format!(
                "opencode mcp_config: server {name:?}: oauth `callbackPort` must be in 1..65535"
            ));
        }
    }
    Ok(())
}

fn reject_unknown<const N: usize>(
    name: &str,
    object: &Map<String, Value>,
    allowed: [&str; N],
) -> Result<(), String> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "opencode mcp_config: server {name:?}: unknown field `{key}`"
            ));
        }
    }
    Ok(())
}

fn require_string_array(name: &str, value: &Value, field: &str) -> Result<(), String> {
    let Some(array) = value.as_array() else {
        return Err(format!(
            "opencode mcp_config: server {name:?}: `{field}` must be an array"
        ));
    };
    if array.iter().any(|item| !item.is_string()) {
        return Err(format!(
            "opencode mcp_config: server {name:?}: `{field}` must contain only strings"
        ));
    }
    Ok(())
}

fn require_string_map(name: &str, value: &Value, field: &str) -> Result<(), String> {
    if value.is_null() {
        return Ok(());
    }
    let Some(object) = value.as_object() else {
        return Err(format!(
            "opencode mcp_config: server {name:?}: `{field}` must be an object of strings"
        ));
    };
    if object
        .values()
        .any(|item| !item.is_string() && !item.is_null())
    {
        return Err(format!(
            "opencode mcp_config: server {name:?}: `{field}` must contain only strings"
        ));
    }
    Ok(())
}

fn require_bool(name: &str, value: Option<&Value>, field: &str) -> Result<(), String> {
    if value.is_some_and(|value| !value.is_boolean() && !value.is_null()) {
        return Err(format!(
            "opencode mcp_config: server {name:?}: `{field}` must be a boolean"
        ));
    }
    Ok(())
}

fn require_positive_integer(name: &str, value: Option<&Value>, field: &str) -> Result<(), String> {
    if value.is_some_and(|value| !value.is_null() && value.as_i64().is_none_or(|value| value <= 0))
    {
        return Err(format!(
            "opencode mcp_config: server {name:?}: `{field}` must be a positive integer"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_claude_servers_to_authoritative_opencode_config() {
        let input = serde_json::json!({
            "mcpServers": {
                "local": {"command": "uvx", "args": ["demo"], "env": {"TOKEN": "x"}},
                "remote": {"url": "https://mcp.example", "headers": {"X-Key": "secret"}}
            }
        });
        let content = build_opencode_mcp_config_content(Some(&input));
        assert!(content.is_ok());
        let content = content.ok().flatten().unwrap_or_default();
        let parsed: Value = serde_json::from_str(&content).unwrap_or(Value::Null);
        assert_eq!(parsed["mcp"]["local"]["type"], "local");
        assert_eq!(
            parsed["mcp"]["local"]["command"],
            serde_json::json!(["uvx", "demo"])
        );
        assert_eq!(parsed["mcp"]["local"]["environment"]["TOKEN"], "x");
        assert_eq!(parsed["mcp"]["remote"]["type"], "remote");
    }

    #[test]
    fn rejects_unknown_native_fields_and_bad_oauth() {
        let unknown =
            serde_json::json!({"mcp": {"demo": {"type": "local", "command": ["x"], "wat": true}}});
        assert!(build_opencode_mcp_config_content(Some(&unknown)).is_err());
        let oauth = serde_json::json!({"mcp": {"demo": {"type": "remote", "url": "https://mcp.example", "oauth": true}}});
        assert!(build_opencode_mcp_config_content(Some(&oauth)).is_err());
    }

    #[test]
    fn claude_optional_fields_match_go_decoder_compatibility() {
        let input = serde_json::json!({
            "mcpServers": {
                "local": {
                    "command": "node",
                    "enabled": "legacy-invalid-value",
                    "env": null,
                    "timeout": null
                }
            }
        });
        let content = build_opencode_mcp_config_content(Some(&input))
            .unwrap_or_else(|error| panic!("translate legacy config: {error}"))
            .unwrap_or_default();
        let parsed: Value =
            serde_json::from_str(&content).unwrap_or_else(|error| panic!("parse config: {error}"));
        let local = &parsed["mcp"]["local"];
        assert!(local.get("enabled").is_none());
        assert!(local["environment"].is_null());
        assert!(local["timeout"].is_null());

        let native_null = serde_json::json!({
            "mcpServers": null,
            "mcp": {"remote": {"type": "remote", "url": "https://mcp.example", "headers": null}}
        });
        assert!(build_opencode_mcp_config_content(Some(&native_null)).is_ok());
    }

    #[test]
    fn null_empty_and_non_object_follow_managed_config_contract() {
        assert!(build_opencode_mcp_config_content(None).is_ok_and(|value| value.is_none()));
        assert!(build_opencode_mcp_config_content(Some(&Value::Null))
            .is_ok_and(|value| value.is_none()));
        let empty = serde_json::json!({});
        assert!(build_opencode_mcp_config_content(Some(&empty)).is_ok_and(|value| value.is_none()));
        assert!(build_opencode_mcp_config_content(Some(&serde_json::json!([]))).is_err());
    }
}
