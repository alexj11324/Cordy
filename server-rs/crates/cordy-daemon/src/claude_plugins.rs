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
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ClaudePluginInstall {
    pub id: String,
    pub name: String,
    pub install_path: PathBuf,
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
pub(crate) fn list_enabled_claude_plugins(home: &Path) -> Vec<ClaudePluginInstall> {
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
        let install_path_text = selected.install_path.trim();
        if install_path_text.is_empty() {
            continue;
        }
        let install_path = PathBuf::from(install_path_text);

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

pub(crate) fn read_claude_plugin_manifest(install_path: &Path) -> Option<ClaudePluginManifest> {
    let raw = std::fs::read(install_path.join(".claude-plugin").join("plugin.json")).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// `claudePluginComponentPaths`: defaults plus whatever the manifest's
/// `raw` field carries (a single string or an array of strings). Relative
/// candidates are joined to `install_path`; every result is cleaned and
/// confined to the install path, deduped in order.
pub(crate) fn claude_plugin_component_paths(
    install_path: &Path,
    raw: &serde_json::Value,
    defaults: &[PathBuf],
) -> Vec<PathBuf> {
    let install_root = clean_path(install_path);
    let mut paths: Vec<PathBuf> = defaults.to_vec();
    match raw {
        serde_json::Value::String(one) => {
            if !one.trim().is_empty() {
                paths.push(PathBuf::from(one));
            }
        }
        serde_json::Value::Array(many) => {
            // Go unmarshals the whole []string or rejects the entire value.
            // Do not partially accept a malformed mixed-type array.
            if many.iter().all(serde_json::Value::is_string) {
                paths.extend(many.iter().filter_map(|v| v.as_str()).map(PathBuf::from));
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
        // Path::is_absolute is the platform-native equivalent of Go's
        // filepath.IsAbs: it handles drive-letter and UNC paths on Windows.
        let candidate = if candidate.is_absolute() {
            clean_path(&candidate)
        } else {
            clean_path(&install_root.join(&candidate))
        };
        // Confine to the install path: rel must not escape (".." prefix).
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

/// Lexically normalizes a path without touching the filesystem. This is the
/// platform-native counterpart of Go's filepath.Clean used by the component
/// boundary check; unlike the daemon's slash-only helper it preserves Windows
/// prefixes and non-UTF-8 path components.
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
        let install = Path::new("/plugins/foo");
        let defaults = vec![PathBuf::from("/plugins/foo/skills")];
        let one = serde_json::json!("skills-extra");
        assert_eq!(
            claude_plugin_component_paths(install, &one, &defaults),
            vec![
                PathBuf::from("/plugins/foo/skills"),
                PathBuf::from("/plugins/foo/skills-extra")
            ]
        );
        let many = serde_json::json!(["a", "b"]);
        assert_eq!(
            claude_plugin_component_paths(install, &many, &defaults),
            vec![
                PathBuf::from("/plugins/foo/skills"),
                PathBuf::from("/plugins/foo/a"),
                PathBuf::from("/plugins/foo/b")
            ]
        );
    }

    #[test]
    fn component_paths_reject_escapes_and_dupes() {
        let install = Path::new("/plugins/foo");
        let raw = serde_json::json!(["../escape", "/plugins/foo/a", "/plugins/foo/a"]);
        assert_eq!(
            claude_plugin_component_paths(install, &raw, &[]),
            vec![PathBuf::from("/plugins/foo/a")]
        );
    }

    #[test]
    fn component_paths_reject_sibling_prefixes() {
        let raw = serde_json::json!(["/plugins/foobar/skills", "/plugins/foo/skills"]);
        assert_eq!(
            claude_plugin_component_paths(Path::new("/plugins/foo"), &raw, &[]),
            vec![PathBuf::from("/plugins/foo/skills")]
        );
    }

    #[test]
    fn component_paths_skip_blank_entries() {
        let raw = serde_json::json!(["", "   "]);
        assert!(claude_plugin_component_paths(Path::new("/p"), &raw, &[]).is_empty());
    }

    #[test]
    fn component_paths_reject_mixed_type_arrays_like_go() {
        let raw = serde_json::json!(["kept-only-by-partial-decoder", 42]);
        assert!(claude_plugin_component_paths(Path::new("/plugins/foo"), &raw, &[]).is_empty());
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
        let install = base.path().join("plugin");
        fs::write(
            plugins_dir.join("installed_plugins.json"),
            serde_json::json!({
                "plugins": {
                    "demo@market": [{"scope": "user", "installPath": install}]
                }
            })
            .to_string(),
        )
        .unwrap();

        let discovered = list_enabled_claude_plugins(&home);
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].install_path, install);
    }

    #[cfg(windows)]
    #[test]
    fn component_paths_use_windows_absolute_paths_and_confinement() {
        let install = Path::new(r"C:\plugins\foo");
        let raw = serde_json::json!([r"C:\outside\mcp.json", r"C:\plugins\foo\mcp.json"]);
        assert_eq!(
            claude_plugin_component_paths(install, &raw, &[]),
            vec![PathBuf::from(r"C:\plugins\foo\mcp.json")]
        );
    }
}
