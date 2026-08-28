//! Filtered child-process environments for agent backends.
//!
//! Daemon-level `PATCHBAY_*` values and Claude runtime markers must not leak into
//! provider-controlled tools. Task-scoped overrides are applied after the
//! ambient filter so an explicit `PATCHBAY_TOKEN` still reaches the child.

use std::collections::BTreeMap;
use std::ffi::OsString;

use tokio::process::Command;

pub fn configure_child_env(command: &mut Command, overrides: &BTreeMap<String, String>) {
    command.env_clear();
    for (key, value) in filtered_inherited_env() {
        command.env(key, value);
    }
    for (key, value) in overrides {
        command.env(key, value);
    }
}

fn filtered_inherited_env() -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(key, _)| !is_filtered_child_env_key(&key.to_string_lossy()))
        .collect()
}

#[cfg(test)]
pub(crate) fn merge_child_env(
    ambient: impl IntoIterator<Item = (String, String)>,
    overrides: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for (key, value) in ambient {
        if !is_filtered_child_env_key(&key) {
            env.insert(key, value);
        }
    }
    env.extend(
        overrides
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    env
}

fn is_filtered_child_env_key(key: &str) -> bool {
    if key.to_ascii_uppercase().starts_with("PATCHBAY_") {
        return true;
    }
    matches!(
        key,
        "CLAUDECODE"
            | "CLAUDE_CODE_ENTRYPOINT"
            | "CLAUDE_CODE_EXECPATH"
            | "CLAUDE_CODE_SESSION_ID"
            | "CLAUDE_CODE_SSE_PORT"
    ) || key.starts_with("CLAUDECODE_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_inherited_patchbay_and_claude_runtime_markers() {
        let ambient = [
            ("PATH", "/usr/bin"),
            ("PATCHBAY_TOKEN", "daemon-secret"),
            ("PATCHBAY_SERVER_URL", "https://daemon.example"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_SESSION_ID", "nested"),
            ("CLAUDECODE_INTERNAL", "1"),
            ("CLAUDE_CODE_GIT_BASH_PATH", "C:\\Git\\bash.exe"),
            ("CLAUDECODEX", "keep-me"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()));
        let overrides = BTreeMap::from([
            ("PATCHBAY_TOKEN".to_string(), "mat_task".to_string()),
            (
                "PATCHBAY_SERVER_URL".to_string(),
                "https://task.example".to_string(),
            ),
        ]);
        let env = merge_child_env(ambient, &overrides);
        assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert_eq!(env.get("PATCHBAY_TOKEN").map(String::as_str), Some("mat_task"));
        assert_eq!(
            env.get("PATCHBAY_SERVER_URL").map(String::as_str),
            Some("https://task.example")
        );
        assert_eq!(
            env.get("CLAUDE_CODE_GIT_BASH_PATH").map(String::as_str),
            Some("C:\\Git\\bash.exe")
        );
        assert_eq!(env.get("CLAUDECODEX").map(String::as_str), Some("keep-me"));
        assert!(!env.contains_key("CLAUDECODE"));
        assert!(!env.contains_key("CLAUDE_CODE_SESSION_ID"));
        assert!(!env.contains_key("CLAUDECODE_INTERNAL"));
        assert!(!is_filtered_child_env_key("CLAUDE_CODE_MAX_OUTPUT_TOKENS"));
        assert!(is_filtered_child_env_key("patchbay_token"));
    }
}
