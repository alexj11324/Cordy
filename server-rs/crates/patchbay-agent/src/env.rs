//! Minimal child-process environments for agent backends.
//!
//! A provider process can spawn arbitrary tools, so ambient daemon variables
//! are authority, not harmless convenience. In particular API keys, credential
//! socket paths, provider homes and platform login tokens must never be
//! inherited. Only inert locale/terminal/process-discovery values cross this
//! boundary; the daemon then adds task-scoped overrides explicitly.

use std::collections::BTreeMap;
use std::ffi::OsString;

use tokio::process::Command;

pub(crate) fn configure_child_env(command: &mut Command, overrides: &BTreeMap<String, String>) {
    command.env_clear();
    for (key, value) in filtered_inherited_env() {
        command.env(key, value);
    }
    for (key, value) in overrides {
        command.env(key, value);
    }
}

/// Final fail-closed seal applied immediately before every provider spawn.
/// It preserves values explicitly set on the command by an adapter, but drops
/// every implicit ambient variable except the inert allowlist above. Keeping
/// this in the shared process owner prevents a new backend from accidentally
/// reintroducing daemon credentials merely by omitting an env helper call.
pub(crate) fn seal_child_env(command: &mut Command) {
    let explicit = command
        .as_std()
        .get_envs()
        .filter_map(|(key, value)| value.map(|value| (key.to_os_string(), value.to_os_string())))
        .collect::<Vec<_>>();
    command.env_clear();
    for (key, value) in filtered_inherited_env() {
        command.env(key, value);
    }
    for (key, value) in explicit {
        command.env(key, value);
    }
}

fn filtered_inherited_env() -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(key, _)| is_inert_inherited_env_key(&key.to_string_lossy()))
        .collect()
}

fn is_inert_inherited_env_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "PATH"
            | "LANG"
            | "LANGUAGE"
            | "TZ"
            | "TERM"
            | "COLORTERM"
            | "NO_COLOR"
            | "FORCE_COLOR"
            | "SYSTEMROOT"
            | "WINDIR"
            | "COMSPEC"
            | "PATHEXT"
            | "OS"
            | "NUMBER_OF_PROCESSORS"
            | "PROCESSOR_ARCHITECTURE"
    ) || upper.starts_with("LC_")
}

#[cfg(test)]
pub(crate) fn merge_child_env(
    ambient: impl IntoIterator<Item = (String, String)>,
    overrides: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for (key, value) in ambient {
        if is_inert_inherited_env_key(&key) {
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
        assert_eq!(
            env.get("PATCHBAY_TOKEN").map(String::as_str),
            Some("mat_task")
        );
        assert_eq!(
            env.get("PATCHBAY_SERVER_URL").map(String::as_str),
            Some("https://task.example")
        );
        assert!(!env.contains_key("CLAUDE_CODE_GIT_BASH_PATH"));
        assert!(!env.contains_key("CLAUDECODEX"));
        assert!(!env.contains_key("CLAUDECODE"));
        assert!(!env.contains_key("CLAUDE_CODE_SESSION_ID"));
        assert!(!env.contains_key("CLAUDECODE_INTERNAL"));
    }

    #[test]
    fn ambient_credentials_and_host_paths_are_never_inherited() {
        let ambient = [
            ("HOME", "/Users/owner"),
            ("SSH_AUTH_SOCK", "/tmp/owner-agent.sock"),
            ("OPENAI_API_KEY", "long-lived"),
            ("ANTHROPIC_API_KEY", "long-lived"),
            ("AWS_PROFILE", "production"),
            ("LANG", "en_US.UTF-8"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()));
        let env = merge_child_env(ambient, &BTreeMap::new());
        assert_eq!(env.get("LANG").map(String::as_str), Some("en_US.UTF-8"));
        for key in [
            "HOME",
            "SSH_AUTH_SOCK",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "AWS_PROFILE",
        ] {
            assert!(!env.contains_key(key), "unexpected inherited {key}");
        }
    }
}
