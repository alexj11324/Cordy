//! Managed MCP configuration ownership.

use serde_json::Value;

/// Only an absent value or JSON null means "inherit runtime configuration".
/// An object, including `{}`, is an authoritative managed set.
pub fn has_managed_config(config: Option<&Value>) -> bool {
    config.is_some_and(|value| !value.is_null())
}

/// Returns the canonical object that provider adapters should translate. A
/// managed non-object is rejected rather than silently dropping all servers.
pub fn managed_object(
    config: Option<&Value>,
) -> Result<Option<&serde_json::Map<String, Value>>, String> {
    let Some(value) = config.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    value
        .as_object()
        .map(Some)
        .ok_or_else(|| "managed MCP configuration must be a JSON object".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_keeps_three_state_semantics() {
        assert!(!has_managed_config(None));
        assert!(!has_managed_config(Some(&Value::Null)));
        assert!(has_managed_config(Some(&serde_json::json!({}))));
        assert!(has_managed_config(Some(
            &serde_json::json!({"mcpServers": {}})
        )));
    }

    #[test]
    fn managed_non_object_fails_closed() {
        assert!(managed_object(Some(&serde_json::json!([]))).is_err());
        let empty = serde_json::json!({});
        let result = managed_object(Some(&empty));
        assert!(result.is_ok());
        assert_eq!(result.ok().flatten().map(serde_json::Map::len), Some(0));
    }
}
