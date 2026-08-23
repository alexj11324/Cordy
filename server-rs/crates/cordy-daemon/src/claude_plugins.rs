//! Port of `server/internal/daemon/claude_plugins.go` (139 lines).
//!
//! Deviations from Go:
//! - `json.RawMessage` → [`serde_json::Value`]; anonymous install entries →
//!   named [`ClaudePluginInstallEntry`].
//! - `filepath.Clean` → [`crate::repocache::normalize_lexically`];
//!   `filepath.Rel` + `..` containment check → `Path::strip_prefix`
//!   (identical accept/reject outcomes: a candidate outside the install
//!   path fails either way).
//! - `filepath.FromSlash` → separator substitution via
//!   [`std::path::MAIN_SEPARATOR_STR`].
//! - No slog output in this file.

// S9-integration: consumed by local_skills.rs (this lane) and runtime_mcp /
// manager wiring that lands with integration; silence dead-code until then.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::repocache::normalize_lexically;

/// `claudePluginInstall` (claude_plugins.go:11–15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudePluginInstall {
    pub id: String,
    pub name: String,
    pub install_path: String,
}

/// Anonymous entry of `claudeInstalledPluginsFile.Plugins`
/// (claude_plugins.go:18–21).
#[derive(Debug, Deserialize)]
struct ClaudePluginInstallEntry {
    #[serde(default)]
    scope: String,
    #[serde(rename = "installPath", default)]
    install_path: String,
}

/// `claudeInstalledPluginsFile` (claude_plugins.go:17–22).
#[derive(Debug, Deserialize)]
struct ClaudeInstalledPluginsFile {
    #[serde(default)]
    plugins: HashMap<String, Vec<ClaudePluginInstallEntry>>,
}

/// `claudeSettingsFile` (claude_plugins.go:24–26).
#[derive(Debug, Deserialize)]
struct ClaudeSettingsFile {
    #[serde(rename = "enabledPlugins", default)]
    enabled_plugins: HashMap<String, bool>,
}

/// `claudePluginManifest` (claude_plugins.go:28–32).
#[derive(Debug, Default, Clone)]
pub(crate) struct ClaudePluginManifest {
    pub name: String,
    pub skills: Option<serde_json::Value>,
    pub mcp_servers: Option<serde_json::Value>,
}

/// `listEnabledClaudePlugins` (claude_plugins.go:38–92): resolves the current
/// user-scope plugin installs that Claude Code itself has enabled. Reading
/// the install registry is deliberate: recursively scanning ~/.claude/plugins
/// would surface both the marketplace checkout and every cached version of
/// the same plugin.
pub(crate) fn list_enabled_claude_plugins(home: &str) -> Vec<ClaudePluginInstall> {
    let settings_raw = match std::fs::read(
        Path::new(home).join(".claude").join("settings.json"),
    ) {
        Ok(raw) => raw,
        Err(_) => return Vec::new(),
    };
    let Ok(settings) = serde_json::from_slice::<ClaudeSettingsFile>(&settings_raw) else {
        return Vec::new();
    };
    if settings.enabled_plugins.is_empty() {
        return Vec::new();
    }

    let installed_raw = match std::fs::read(
        Path::new(home)
            .join(".claude")
            .join("plugins")
            .join("installed_plugins.json"),
    ) {
        Ok(raw) => raw,
        Err(_) => return Vec::new(),
    };
    let Ok(installed) = serde_json::from_slice::<ClaudeInstalledPluginsFile>(&installed_raw)
    else {
        return Vec::new();
    };

    let mut plugin_ids: Vec<String> = settings
        .enabled_plugins
        .iter()
        .filter(|(_, enabled)| **enabled)
        .map(|(id, _)| id.clone())
        .collect();
    plugin_ids.sort();

    let mut plugins = Vec::with_capacity(plugin_ids.len());
    for id in plugin_ids {
        let Some(installs) = installed.plugins.get(&id) else {
            continue;
        };
        if installs.is_empty() {
            continue;
        }
        let mut selected = installs.last().expect("non-empty checked above");
        for install in installs {
            if install.scope == "user" {
                selected = install;
            }
        }
        let install_path = selected.install_path.trim().to_string();
        if install_path.is_empty() {
            continue;
        }

        let mut name = id.splitn(2, '@').next().unwrap_or("").trim().to_string();
        if let Some(manifest) = read_claude_plugin_manifest(&install_path) {
            if !manifest.name.trim().is_empty() {
                name = manifest.name.trim().to_string();
            }
        }
        if name.is_empty() {
            continue;
        }
        plugins.push(ClaudePluginInstall { id, name, install_path });
    }
    plugins
}

/// `readClaudePluginManifest` (claude_plugins.go:94–104).
pub(crate) fn read_claude_plugin_manifest(install_path: &str) -> Option<ClaudePluginManifest> {
    let raw = std::fs::read(
        Path::new(install_path)
            .join(".claude-plugin")
            .join("plugin.json"),
    )
    .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    Some(ClaudePluginManifest {
        name: value
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        skills: value.get("skills").cloned(),
        mcp_servers: value.get("mcpServers").cloned(),
    })
}

/// `claudePluginComponentPaths` (claude_plugins.go:106–139): resolves the
/// declared component locations (a single string or an array) plus any
/// defaults into cleaned absolute paths contained within `install_path`.
pub(crate) fn claude_plugin_component_paths(
    install_path: &str,
    raw: Option<&serde_json::Value>,
    defaults: &[&str],
) -> Vec<String> {
    let mut paths: Vec<String> = defaults.iter().map(|s| s.to_string()).collect();
    match raw {
        Some(value) if value.is_string() => {
            let one = value.as_str().unwrap_or_default();
            if !one.trim().is_empty() {
                paths.push(one.to_string());
            }
        }
        Some(value) => {
            if let Ok(many) = serde_json::from_value::<Vec<String>>(value.clone()) {
                paths.extend(many);
            }
        }
        None => {}
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(paths.len());
    for candidate in paths {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            continue;
        }
        let joined = if Path::new(candidate).is_absolute() {
            PathBuf::from(candidate)
        } else {
            Path::new(install_path).join(from_slash(candidate))
        };
        let candidate = normalize_lexically(&joined);
        // filepath.Rel(installPath, candidate) failing or escaping (".." or
        // "../…") both mean "outside the install path".
        let rel_ok = candidate
            .strip_prefix(install_path)
            .is_ok();
        if !rel_ok {
            continue;
        }
        let key = candidate.to_string_lossy().into_owned();
        if seen.insert(key.clone()) {
            out.push(key);
        }
    }
    out
}

/// `filepath.FromSlash`: restores OS separators in a slash-separated path.
fn from_slash(p: &str) -> PathBuf {
    PathBuf::from(p.replace('/', std::path::MAIN_SEPARATOR_STR))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// claudePluginComponentPaths semantics (claude_plugins.go:106–139):
    /// string-or-array payloads, defaults first, dedup, containment enforced.
    #[test]
    fn component_paths_resolve_within_install_path() {
        let root = tempfile::tempdir().unwrap().keep();
        let root_s = root.to_string_lossy().into_owned();

        let single = serde_json::json!("commands");
        assert_eq!(
            claude_plugin_component_paths(&root_s, Some(&single), &["skills"]),
            vec![
                Path::new(&root_s).join("skills").to_string_lossy().into_owned(),
                Path::new(&root_s).join("commands").to_string_lossy().into_owned(),
            ]
        );

        let many = serde_json::json!(["commands", "agents"]);
        assert_eq!(
            claude_plugin_component_paths(&root_s, Some(&many), &[]),
            vec![
                Path::new(&root_s).join("commands").to_string_lossy().into_owned(),
                Path::new(&root_s).join("agents").to_string_lossy().into_owned(),
            ]
        );

        // Absolute candidates outside the install path are dropped;
        // duplicates collapse.
        let outside = serde_json::json!([root_s.join("..").to_string_lossy().into_owned(), "commands"]);
        assert_eq!(
            claude_plugin_component_paths(&root_s, Some(&outside), &["commands"]),
            vec![Path::new(&root_s).join("commands").to_string_lossy().into_owned()]
        );
    }

    /// listEnabledClaudePlugins behavior (claude_plugins.go:38–92): missing
    /// files and empty registries yield no plugins.
    #[test]
    fn missing_files_yield_no_plugins() {
        let home = tempfile::tempdir().unwrap().keep();
        assert!(list_enabled_claude_plugins(&home.to_string_lossy()).is_empty());

        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(
            home.join(".claude").join("settings.json"),
            br#"{"enabledPlugins":{}}"#,
        )
        .unwrap();
        assert!(list_enabled_claude_plugins(&home.to_string_lossy()).is_empty());
    }

    /// End-to-end selection (claude_plugins.go:57–91): only enabled IDs,
    /// user-scope entry preferred over later entries, manifest name wins,
    /// disabled IDs skipped.
    #[test]
    fn selects_enabled_user_scope_plugin_with_manifest_name() {
        let home = tempfile::tempdir().unwrap().keep();
        let install_dir = home.join("plugin-checkout");
        std::fs::create_dir_all(install_dir.join(".claude-plugin")).unwrap();
        std::fs::write(
            install_dir.join(".claude-plugin").join("plugin.json"),
            br#"{"name":"Deploy Tools","skills":"skills"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(home.join(".claude").join("plugins")).unwrap();
        std::fs::write(
            home.join(".claude").join("settings.json"),
            br#"{"enabledPlugins":{"deploy@acme":true,"off@acme":false}}"#,
        )
        .unwrap();
        std::fs::write(
            home.join(".claude").join("plugins").join("installed_plugins.json"),
            format!(
                r#"{{"plugins":{{"deploy@acme":[{{"scope":"local","installPath":"{}"}},{{"scope":"user","installPath":"{}"}}]}}}}"#,
                install_dir.with_file_name("other").display(),
                install_dir.display(),
            ),
        )
        .unwrap();

        let plugins = list_enabled_claude_plugins(&home.to_string_lossy());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].id, "deploy@acme");
        assert_eq!(plugins[0].name, "Deploy Tools");
        assert_eq!(plugins[0].install_path, install_dir.to_string_lossy());
    }
}
