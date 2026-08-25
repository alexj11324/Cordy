//! Reasonix task configuration and permission isolation.
//!
//! This is the Rust port of `server/internal/daemon/execenv/reasonix_*`. A
//! Reasonix project config replaces the owner's permission table rather than
//! merging it, so resolving the exact user config and restating every key is a
//! correctness and safety contract. The only task-specific change is denying
//! `ask`, which cannot be answered by an unattended daemon task.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use regex::Regex;

use super::context::{is_pre_exists, record_write_file, SidecarManifest};
use super::execenv::clean_path;

const PROJECT_CONFIG_FILE: &str = "reasonix.toml";
const USER_CONFIG_FILE: &str = "config.toml";
const ASK_TOOL: &str = "ask";

const PROJECT_CONFIG_HEADER: &str = "# Managed by Cordy. Written per task, removed when the task env is cleaned up.\n# Edits are not preserved.\n#\n# [permissions] restates the runtime owner's own table from the Reasonix user\n# config, with the ask tool added to deny: no human can answer a question in an\n# unattended task, and an unanswered question cancels the Reasonix turn.\n\n";

fn lookup(env: &HashMap<String, String>, name: &str) -> String {
    if let Some(value) = env.get(name) {
        return value.clone();
    }
    if cfg!(windows) {
        if let Some((_, value)) = env.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)) {
            return value.clone();
        }
    }
    std::env::var(name).unwrap_or_default()
}

fn expand_vars(input: &str, env: &HashMap<String, String>) -> String {
    if !input.contains("${") {
        return input.to_string();
    }
    let re = Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(:-([^}]*))?\}")
        .expect("reasonix variable expression is valid");
    re.replace_all(input, |caps: &regex::Captures<'_>| {
        let value = lookup(env, &caps[1]);
        if !value.is_empty() {
            value
        } else {
            caps.get(3)
                .map(|match_| match_.as_str().to_string())
                .unwrap_or_default()
        }
    })
    .into_owned()
}

fn user_home(env: &HashMap<String, String>) -> String {
    if cfg!(windows) {
        lookup(env, "USERPROFILE")
    } else {
        lookup(env, "HOME")
    }
}

fn user_config_dir(env: &HashMap<String, String>) -> String {
    if cfg!(windows) {
        return lookup(env, "AppData");
    }
    if cfg!(target_os = "macos") {
        let home = user_home(env);
        return if home.is_empty() {
            String::new()
        } else {
            Path::new(&home)
                .join("Library")
                .join("Application Support")
                .to_string_lossy()
                .into_owned()
        };
    }
    if let Some(dir) = nonempty(&lookup(env, "XDG_CONFIG_HOME")) {
        return if Path::new(dir).is_absolute() {
            dir.to_string()
        } else {
            String::new()
        };
    }
    let home = user_home(env);
    if home.is_empty() {
        String::new()
    } else {
        Path::new(&home)
            .join(".config")
            .to_string_lossy()
            .into_owned()
    }
}

fn nonempty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn clean_dir(env: &HashMap<String, String>, name: &str) -> String {
    let mut dir = lookup(env, name).trim().to_string();
    if dir.is_empty() {
        return String::new();
    }
    dir = expand_vars(&dir, env);
    let home = user_home(env);
    if dir == "~" {
        dir = home;
    } else if let Some(rest) = dir.strip_prefix("~/").or_else(|| dir.strip_prefix(r"~\")) {
        if !home.is_empty() {
            dir = Path::new(&home).join(rest).to_string_lossy().into_owned();
        }
    }
    if !Path::new(&dir).is_absolute() {
        if let Ok(cwd) = std::env::current_dir() {
            dir = cwd.join(dir).to_string_lossy().into_owned();
        }
    }
    clean_path(&dir)
}

fn isolated_home(env: &HashMap<String, String>) -> String {
    clean_dir(env, "REASONIX_HOME")
}

fn home_dir(env: &HashMap<String, String>) -> String {
    if let Some(dir) = nonempty(&isolated_home(env)) {
        return dir.to_string();
    }
    if cfg!(windows) {
        if let Some(dir) = nonempty(&user_config_dir(env)) {
            return Path::new(dir)
                .join("reasonix")
                .to_string_lossy()
                .into_owned();
        }
        let home = lookup(env, "USERPROFILE");
        return if home.is_empty() {
            String::new()
        } else {
            Path::new(&home)
                .join("AppData")
                .join("Roaming")
                .join("reasonix")
                .to_string_lossy()
                .into_owned()
        };
    }
    if let Some(home) = nonempty(&user_home(env)) {
        return Path::new(home)
            .join(".reasonix")
            .to_string_lossy()
            .into_owned();
    }
    let config_dir = user_config_dir(env);
    if config_dir.is_empty() {
        String::new()
    } else {
        Path::new(&config_dir)
            .join("reasonix")
            .to_string_lossy()
            .into_owned()
    }
}

fn user_config_path(env: &HashMap<String, String>) -> String {
    let home = home_dir(env);
    if home.is_empty() {
        String::new()
    } else {
        Path::new(&home)
            .join(USER_CONFIG_FILE)
            .to_string_lossy()
            .into_owned()
    }
}

fn same_path(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let absolute = |value: &str| {
        let path = Path::new(value);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    };
    clean_path(&absolute(a).to_string_lossy()) == clean_path(&absolute(b).to_string_lossy())
}

fn legacy_os_support_dir(env: &HashMap<String, String>) -> String {
    if !isolated_home(env).is_empty() {
        return String::new();
    }
    let config_dir = user_config_dir(env);
    if config_dir.is_empty() {
        return String::new();
    }
    let path = Path::new(&config_dir).join("reasonix");
    let current = home_dir(env);
    if same_path(&path.to_string_lossy(), &current) {
        String::new()
    } else {
        path.to_string_lossy().into_owned()
    }
}

fn legacy_user_config_path(env: &HashMap<String, String>) -> String {
    let dir = legacy_os_support_dir(env);
    if dir.is_empty() {
        return String::new();
    }
    let path = Path::new(&dir).join(USER_CONFIG_FILE);
    let primary = user_config_path(env);
    if same_path(&path.to_string_lossy(), &primary) {
        String::new()
    } else {
        path.to_string_lossy().into_owned()
    }
}

fn legacy_xdg_paths(env: &HashMap<String, String>) -> Vec<String> {
    if !isolated_home(env).is_empty() || cfg!(windows) {
        return Vec::new();
    }
    let mut paths = Vec::new();
    let mut add = |path: PathBuf| {
        let value = clean_path(&path.to_string_lossy());
        if !value.is_empty() && !paths.iter().any(|existing| existing == &value) {
            paths.push(value);
        }
    };
    let xdg = clean_dir(env, "XDG_CONFIG_HOME");
    if !xdg.is_empty() {
        add(Path::new(&xdg).join("reasonix").join(USER_CONFIG_FILE));
    }
    let home = user_home(env);
    if !home.is_empty() {
        add(Path::new(&home)
            .join(".config")
            .join("reasonix")
            .join(USER_CONFIG_FILE));
    }
    paths
}

fn user_config_load_path(env: &HashMap<String, String>) -> String {
    let primary = user_config_path(env);
    if primary.is_empty() {
        return legacy_user_config_path(env);
    }
    if Path::new(&primary).is_file() {
        return primary;
    }
    let legacy = legacy_user_config_path(env);
    if !legacy.is_empty() && Path::new(&legacy).is_file() {
        return legacy;
    }
    for legacy in legacy_xdg_paths(env) {
        if !same_path(&legacy, &primary) && Path::new(&legacy).is_file() {
            return legacy;
        }
    }
    primary
}

fn owner_permissions(path: &str) -> Result<Option<toml::value::Table>> {
    if path.is_empty() {
        return Ok(None);
    }
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("read reasonix user config {path}"))
        }
    };
    let text = std::str::from_utf8(&data).context("reasonix user config is not UTF-8")?;
    let value: toml::Value =
        toml::from_str(text).with_context(|| format!("parse reasonix user config {path}"))?;
    match value.get("permissions") {
        None => Ok(None),
        Some(toml::Value::Table(table)) => Ok(Some(table.clone())),
        Some(other) => bail!(
            "reasonix user config {path}: [permissions] is {}, want a table",
            other.type_str()
        ),
    }
}

fn with_ask_denied(mut permissions: toml::value::Table) -> Result<toml::value::Table> {
    let deny = match permissions.remove("deny") {
        None => Vec::new(),
        Some(toml::Value::Array(values)) => values,
        Some(other) => bail!(
            "reasonix user config: [permissions] deny is {}, want an array of strings",
            other.type_str()
        ),
    };
    let mut rules = Vec::with_capacity(deny.len() + 1);
    for value in deny {
        match value {
            toml::Value::String(rule) => rules.push(rule),
            other => bail!(
                "reasonix user config: [permissions] deny entry is {}, want a string",
                other.type_str()
            ),
        }
    }
    if !rules.iter().any(|rule| rule == ASK_TOOL) {
        rules.push(ASK_TOOL.to_string());
    }
    permissions.insert(
        "deny".to_string(),
        toml::Value::Array(rules.into_iter().map(toml::Value::String).collect()),
    );
    Ok(permissions)
}

fn render_project_config(user_path: &str) -> Result<Vec<u8>> {
    let permissions = with_ask_denied(owner_permissions(user_path)?.unwrap_or_default())?;
    let mut root = toml::value::Table::new();
    root.insert("permissions".to_string(), toml::Value::Table(permissions));
    let body =
        toml::to_string(&toml::Value::Table(root)).context("encode reasonix project config")?;
    let mut content = PROJECT_CONFIG_HEADER.as_bytes().to_vec();
    content.extend_from_slice(body.as_bytes());
    Ok(content)
}

/// Writes the task-scoped Reasonix config, preserving an existing repository
/// file and refusing to replace an unreadable owner permission table.
pub(crate) fn write_reasonix_project_config(
    work_dir: &str,
    task_env: &HashMap<String, String>,
    manifest: Option<&mut SidecarManifest>,
) -> Result<()> {
    if work_dir.is_empty() {
        return Ok(());
    }
    let user_path = user_config_load_path(task_env);
    let content = match render_project_config(&user_path) {
        Ok(content) => content,
        Err(error) => {
            tracing::warn!(
                user_config = %user_path,
                error = %format!("{error:#}"),
                "execenv: cannot restate Reasonix user permissions; leaving project config absent"
            );
            return Ok(());
        }
    };
    let path = Path::new(work_dir).join(PROJECT_CONFIG_FILE);
    let path_string = path.to_string_lossy().into_owned();
    // Preserve the no-overwrite contract even for callers that do not need
    // sidecar cleanup bookkeeping. `record_write_file` only performs its
    // pre-existing-path check when given a manifest, so use an ephemeral one
    // rather than falling back to an unconditional write.
    let mut ephemeral_manifest = SidecarManifest::default();
    let tracking_manifest = manifest.or(Some(&mut ephemeral_manifest));
    match record_write_file(&path_string, &content, tracking_manifest) {
        Ok(()) => Ok(()),
        Err(error) if is_pre_exists(&error) => {
            tracing::warn!(path = %path_string, "execenv: project reasonix.toml already exists; leaving it untouched");
            Ok(())
        }
        Err(error) => Err(error).with_context(|| format!("write {PROJECT_CONFIG_FILE}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_home_wins_and_project_config_preserves_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("reasonix-home");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join(USER_CONFIG_FILE),
            "[permissions]\nallow = [\"read\"]\ndeny = [\"write\"]\n",
        )
        .unwrap();
        let env = HashMap::from([
            (
                "REASONIX_HOME".to_string(),
                home.to_string_lossy().into_owned(),
            ),
            ("HOME".to_string(), "/wrong".to_string()),
        ]);
        let body = render_project_config(&user_config_load_path(&env)).unwrap();
        let value: toml::Value = toml::from_str(&String::from_utf8(body).unwrap()).unwrap();
        let permissions = value.get("permissions").unwrap().as_table().unwrap();
        assert_eq!(
            permissions["allow"].as_array().unwrap()[0].as_str(),
            Some("read")
        );
        assert!(permissions["deny"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some("write")));
        assert!(permissions["deny"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some(ASK_TOOL)));
    }

    #[test]
    fn existing_project_config_is_not_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let work_dir = temp.path().to_string_lossy().into_owned();
        let path = temp.path().join(PROJECT_CONFIG_FILE);
        fs::write(&path, "[permissions]\ndeny = [\"custom\"]\n").unwrap();
        write_reasonix_project_config(&work_dir, &HashMap::new(), None).unwrap();
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "[permissions]\ndeny = [\"custom\"]\n"
        );
    }

    #[test]
    fn malformed_deny_shape_does_not_write_project_config() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("reasonix-home");
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join(USER_CONFIG_FILE),
            "[permissions]\ndeny = \"ask\"\n",
        )
        .unwrap();
        let work_dir = temp.path().join("work");
        fs::create_dir_all(&work_dir).unwrap();
        let env = HashMap::from([(
            "REASONIX_HOME".to_string(),
            home.to_string_lossy().into_owned(),
        )]);
        write_reasonix_project_config(work_dir.to_str().unwrap(), &env, None).unwrap();
        assert!(!work_dir.join(PROJECT_CONFIG_FILE).exists());
    }
}
