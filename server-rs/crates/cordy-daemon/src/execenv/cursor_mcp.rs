//! TEMPORARY S9-integration stand-in for lane E1b's cursor_mcp port.
//! E1b replaces this file with the full cursor_mcp.go port.

use serde_json::Value;

use super::context::SidecarManifest;

/// `prepareCursorMcpConfig` — fail-open stand-in: no-op returning an empty
/// CURSOR_DATA_DIR. The real port materializes managed MCP config.
pub(crate) fn prepare_cursor_mcp_config(
    env_root: &str,
    work_dir: &str,
    _mcp_config: Option<&Value>,
    _mcp_auth_source: &str,
    manifest: Option<&mut SidecarManifest>,
) -> anyhow::Result<String> {
    let _ = (env_root, work_dir, manifest);
    Ok(String::new())
}
