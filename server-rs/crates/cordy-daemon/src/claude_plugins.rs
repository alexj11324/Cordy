#![allow(dead_code)] // S9-integration: consumed by daemon.go core wiring (S8)
//! Port of `server/internal/daemon/claude_plugins.go` — resolves the
//! user-scope Claude Code plugin installs that Claude Code itself enabled,
//! and projects a manifest's skill/MCP component paths.
//!
//! Symbol map:
//! - `claudePluginInstall` → [`ClaudePluginInstall`]
//! - `listEnabledClaudePlugins` → [`list_enabled_claude_plugins`]
//! - `readClaudePluginManifest` → [`read_claude_plugin_manifest`]
//! - `claudePluginComponentPaths` → [`claude_plugin_component_paths`]

use std::collections::HashSet;

use serde::Deserialize;

use crate::execenv::execenv::{clean_path, join_path};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ClaudePluginInstall {
    pub id: String,
    pub name: String,
    pub install_path: String,
}

#[derive(Debug, Deserialize, Default)]
struct ClaudeInstalledPluginsFile {
    #[serde(rename = "plugins", default)]
    plugins: std::collections::HashMap<String, Vec<ClaudePluginInstallEntry>>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct ClaudePluginInstallEntry {
    #[serde(rename = "scope", default)]
    scope: String,
    #[serde(rename = "installPath", default)]
    install_path: String,
}

#[derive(Debug, Deserialize, Default)]
struct ClaudeSettingsFile {
    #[serde(rename = "enabledPlugins", default)]
    enabled_plugins: std::collections::HashMap<String, bool>,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct ClaudePluginManifest {
    #[serde(rename = "name", default)]
    name: String,
    #[serde(rename = "skills", default)]
    skills: serde_json::Value,
    #[serde(rename = "mcpServers", default)]
    mcp_servers: serde_json::Value,
}

impl ClaudePluginManifest {
    pub fn skills_value(&self) -> &serde_json::Value {
        &self.skills
    }

    pub fn mcp_servers_value(&self) -> &serde_json::Value {
        &self.mcp_servers
    }
}

/// `listEnabledClaudePlugins` resolves the current user-scope plugin installs
/// that Claude Code itself has enabled. Reading the install registry is
/// deliberate: recursively scanning ~/.claude/plugins would surface both the
/// marketplace checkout and every cached version of the same plugin.
pub(crate) fn list_enabled_claude_plugins(home: &str) -> Vec<ClaudePluginInstall> {
    let settings_raw = match std::fs::read(join_path(&[home, ".claude", "settings.json"])) {
        Ok(raw) => raw,
        Err(_) => return Vec::new(),
    };
    let settings: ClaudeSettingsFile = match serde_json::from_slice(&settings_raw) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    if settings.enabled_plugins.is_empty() {
        return Vec::new();
    }

    let installed_raw = match std::fs::read(join_path(&[
        home,
        ".claude",
        "plugins",
        "installed_plugins.json",
    ])) {
        Ok(raw) => raw,
        Err(_) => return Vec::new(),
    };
    let installed: ClaudeInstalledPluginsFile = match serde_json::from_slice(&installed_raw) {
        Ok(i) => i,
        Err(_) => return Vec::new(),
    };

    let mut plugin_ids: Vec<String> = settings
        .enabled_plugins
        .iter()
        .filter(|(_, enabled)| **enabled)
        .map(|(id, _)| id.clone())
        .collect();
    plugin_ids.sort();

    let mut plugins = Vec::with_capacity(plugin_ids.len());
    for id in &plugin_ids {
        let installs = match installed.plugins.get(id) {
            Some(i) if !i.is_empty() => i,
            _ => continue,
        };
        // Last entry wins, unless a user-scope install exists.
        let mut selected = installs[installs.len() - 1].clone();
        for install in installs {
            if install.scope == "user" {
                selected = install.clone();
            }
        }
        let install_path = selected.install_path.trim().to_string();
        if install_path.is_empty() {
            continue;
        }

        let mut name = id.split('@').next().unwrap_or("").trim().to_string();
        if let Some(manifest) = read_claude_plugin_manifest(&install_path) {
            let manifest_name = manifest.name.trim();
            if !manifest_name.is_empty() {
                name = manifest_name.to_string();
            }
        }
        if name.is_empty() {
            continue;
        }
        plugins.push(ClaudePluginInstall {
            id: id.clone(),
            name,
            install_path,
        });
    }
    plugins
}

pub(crate) fn read_claude_plugin_manifest(install_path: &str) -> Option<ClaudePluginManifest> {
    let raw = std::fs::read(join_path(&[install_path, ".claude-plugin", "plugin.json"])).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// `claudePluginComponentPaths`: defaults plus whatever the manifest's
/// `raw` field carries (a single string or an array of strings). Relative
/// candidates are joined to `install_path`; every result is cleaned and
/// confined to the install path, deduped in order.
pub(crate) fn claude_plugin_component_paths(
    install_path: &str,
    raw: &serde_json::Value,
    defaults: &[String],
) -> Vec<String> {
    let install_root = std::path::PathBuf::from(clean_path(install_path));
    let mut paths: Vec<String> = defaults.to_vec();
    match raw {
        serde_json::Value::String(one) => {
            if !one.trim().is_empty() {
                paths.push(one.clone());
            }
        }
        serde_json::Value::Array(many) => {
            for v in many {
                if let Some(s) = v.as_str() {
                    paths.push(s.to_string());
                }
            }
        }
        _ => {}
    }

    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(paths.len());
    for candidate in paths {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            continue;
        }
        // Mirror Go: absolute candidates are used as-is (cleaned), relative
        // ones are joined onto the install path. All comparisons here use the
        // slash-separated Clean semantics of execenv's clean_path.
        let candidate = if candidate.starts_with('/') {
            clean_path(candidate)
        } else {
            clean_path(&join_path(&[install_path, candidate]))
        };
        // Confine to the install path: rel must not escape (".." prefix).
        if std::path::Path::new(&candidate)
            .strip_prefix(&install_root)
            .is_err()
        {
            continue;
        }
        if seen.insert(candidate.clone()) {
            out.push(candidate);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_paths_accept_string_or_array() {
        let install = "/plugins/foo";
        let defaults = vec!["/plugins/foo/skills".to_string()];
        let one = serde_json::json!("skills-extra");
        assert_eq!(
            claude_plugin_component_paths(install, &one, &defaults),
            vec![
                "/plugins/foo/skills".to_string(),
                "/plugins/foo/skills-extra".to_string()
            ]
        );
        let many = serde_json::json!(["a", "b"]);
        assert_eq!(
            claude_plugin_component_paths(install, &many, &defaults),
            vec![
                "/plugins/foo/skills".to_string(),
                "/plugins/foo/a".to_string(),
                "/plugins/foo/b".to_string()
            ]
        );
    }

    #[test]
    fn component_paths_reject_escapes_and_dupes() {
        let install = "/plugins/foo";
        let raw = serde_json::json!(["../escape", "/plugins/foo/a", "/plugins/foo/a"]);
        assert_eq!(
            claude_plugin_component_paths(install, &raw, &[]),
            vec!["/plugins/foo/a".to_string()]
        );
    }

    #[test]
    fn component_paths_reject_sibling_prefixes() {
        let raw = serde_json::json!(["/plugins/foobar/skills", "/plugins/foo/skills"]);
        assert_eq!(
            claude_plugin_component_paths("/plugins/foo", &raw, &[]),
            vec!["/plugins/foo/skills".to_string()]
        );
    }

    #[test]
    fn component_paths_skip_blank_entries() {
        let raw = serde_json::json!(["", "   "]);
        assert!(claude_plugin_component_paths("/p", &raw, &[]).is_empty());
    }
}
