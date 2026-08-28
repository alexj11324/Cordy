//! Managed MCP configuration ownership.

use std::io::Write;

use serde_json::Value;
use tempfile::NamedTempFile;

use crate::contract::AgentError;

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

/// Materializes an authoritative managed MCP object into a private temporary
/// file. The returned guard owns deletion and must remain alive for the child
/// process lifetime.
pub fn write_managed_temp(
    config: Option<&Value>,
    prefix: &str,
) -> Result<Option<NamedTempFile>, AgentError> {
    let Some(config) = managed_object(config).map_err(AgentError::InvalidConfig)? else {
        return Ok(None);
    };
    let mut file = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(".json")
        .tempfile()
        .map_err(AgentError::Process)?;
    serde_json::to_writer(file.as_file_mut(), config).map_err(|error| {
        AgentError::InvalidConfig(format!("serialize managed MCP config: {error}"))
    })?;
    file.flush().map_err(AgentError::Process)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(AgentError::Process)?;
    }
    Ok(Some(file))
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

    #[test]
    fn managed_temp_is_exact_private_and_guard_owned() {
        let config = serde_json::json!({"mcpServers":{"demo":{"command":"echo"}}});
        let file = write_managed_temp(Some(&config), "patchbay-mcp-test-")
            .unwrap_or_else(|error| panic!("write managed MCP temp: {error}"))
            .unwrap_or_else(|| panic!("managed object must create a file"));
        let path = file.path().to_path_buf();
        let decoded: Value = serde_json::from_slice(
            &std::fs::read(&path).unwrap_or_else(|error| panic!("read MCP temp: {error}")),
        )
        .unwrap_or_else(|error| panic!("decode MCP temp: {error}"));
        assert_eq!(decoded, config);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                file.as_file()
                    .metadata()
                    .unwrap_or_else(|error| panic!("MCP temp metadata: {error}"))
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        drop(file);
        assert!(!path.exists());
    }
}
