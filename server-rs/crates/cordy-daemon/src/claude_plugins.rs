//! Resolves the
//! user-scope Claude Code plugin installs that Claude Code itself enabled,
//! and projects a manifest's skill/MCP component paths.
//!
//! Symbol map:
//! - `claudePluginInstall` → [`ClaudePluginInstall`]
//! - `listEnabledClaudePlugins` → [`list_enabled_claude_plugins`]
//! - `readClaudePluginManifest` → [`read_claude_plugin_manifest`]
//! - `claudePluginComponentPaths` → [`claude_plugin_component_paths`]

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

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
pub(crate) fn list_enabled_claude_plugins(home: impl AsRef<Path>) -> Vec<ClaudePluginInstall> {
    let home = home.as_ref();
    let settings_raw = match std::fs::read(home.join(".claude").join("settings.json")) {
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

    let installed_raw = match std::fs::read(
        home.join(".claude")
            .join("plugins")
            .join("installed_plugins.json"),
    ) {
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

pub(crate) fn read_claude_plugin_manifest(
    install_path: impl AsRef<Path>,
) -> Option<ClaudePluginManifest> {
    let raw = std::fs::read(
        install_path
            .as_ref()
            .join(".claude-plugin")
            .join("plugin.json"),
    )
    .ok()?;
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
    component_paths(
        Path::new(install_path),
        raw,
        defaults
            .iter()
            .map(|path| PathBuf::from(path.as_str()))
            .collect(),
    )
    .into_iter()
    .map(|path| path.to_string_lossy().into_owned())
    .collect()
}

/// Resolves a plugin's default and manifest-declared MCP configuration paths
/// through the same parser used for its other components.
pub(crate) fn claude_plugin_mcp_paths(plugin: &ClaudePluginInstall) -> Vec<PathBuf> {
    let install_path = Path::new(&plugin.install_path);
    let raw = read_claude_plugin_manifest(install_path)
        .map(|manifest| manifest.mcp_servers_value().clone())
        .unwrap_or(serde_json::Value::Null);
    let install_root = clean_path(install_path);
    component_paths(install_path, &raw, vec![install_path.join(".mcp.json")])
        .into_iter()
        // A component must name a file below the plugin root. This preserves the
        // runtime MCP loader's existing rejection of a manifest value of `.`.
        .filter(|path| path != &install_root)
        .collect()
}

fn component_paths(
    install_path: &Path,
    raw: &serde_json::Value,
    mut paths: Vec<PathBuf>,
) -> Vec<PathBuf> {
    let install_root = clean_path(install_path);
    match raw {
        serde_json::Value::String(one) => {
            if !one.trim().is_empty() {
                paths.push(PathBuf::from(one.as_str()));
            }
        }
        serde_json::Value::Array(many) => {
            for v in many {
                if let Some(s) = v.as_str() {
                    paths.push(PathBuf::from(s));
                }
            }
        }
        _ => {}
    }

    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(paths.len());
    for candidate in paths {
        let candidate = trim_path(&candidate);
        if candidate.as_os_str().is_empty() {
            continue;
        }
        let candidate = if candidate.is_absolute() {
            clean_path(&candidate)
        } else {
            clean_path(&install_root.join(candidate))
        };
        if candidate.strip_prefix(&install_root).is_err() {
            continue;
        }
        if seen.insert(candidate.clone()) {
            out.push(candidate);
        }
    }
    out
}

fn trim_path(path: &Path) -> PathBuf {
    path.to_str()
        .map(|value| PathBuf::from(value.trim()))
        .unwrap_or_else(|| path.to_path_buf())
}

/// Lexically normalizes native paths without touching the filesystem.
fn clean_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    let rooted = path.has_root();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !rooted {
                    out.push(component.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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

    #[test]
    fn mcp_paths_keep_default_and_reject_root_or_escape() {
        let base = tempfile::tempdir().unwrap();
        let install = base.path().join("plugin");
        fs::create_dir_all(install.join(".claude-plugin")).unwrap();
        fs::write(
            install.join(".claude-plugin").join("plugin.json"),
            r#"{"mcpServers":[".","../escape.json","nested/mcp.json"]}"#,
        )
        .unwrap();
        let plugin = ClaudePluginInstall {
            install_path: install.to_string_lossy().into_owned(),
            ..Default::default()
        };

        assert_eq!(
            claude_plugin_mcp_paths(&plugin),
            vec![install.join(".mcp.json"), install.join("nested/mcp.json")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn plugin_discovery_keeps_non_utf8_home_paths() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let base = tempfile::tempdir().unwrap();
        let home = base.path().join(OsString::from_vec(b"home-\xff".to_vec()));
        let plugins_dir = home.join(".claude").join("plugins");
        fs::create_dir_all(&plugins_dir).unwrap();
        fs::write(
            home.join(".claude").join("settings.json"),
            br#"{"enabledPlugins":{"demo@market":true}}"#,
        )
        .unwrap();
        fs::write(
            plugins_dir.join("installed_plugins.json"),
            br#"{"plugins":{"demo@market":[{"scope":"user","installPath":"/plugin"}]}}"#,
        )
        .unwrap();

        assert_eq!(list_enabled_claude_plugins(&home).len(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn component_paths_use_windows_absolute_paths_and_confinement() {
        let raw = serde_json::json!([r"C:\outside\mcp.json", r"C:\plugins\foo\mcp.json"]);
        assert_eq!(
            claude_plugin_component_paths(r"C:\plugins\foo", &raw, &[]),
            vec![r"C:\plugins\foo\mcp.json".to_string()]
        );
    }
}
