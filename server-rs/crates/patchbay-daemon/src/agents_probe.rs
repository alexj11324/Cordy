//! Discovery of installed
//! built-in agent CLIs (PATH lookup + login-shell fallback + Codex Desktop
//! app-bundle probe + DSH profile probe).
//! The login-shell resolver is shared with daemon configuration.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::{resolve_agent_executable_path, resolve_agents_via_login_shell};
use crate::helpers::env_or_default;
use crate::types::AgentEntry;

/// `shellResolveTTL`: deliberately much longer than the discovery interval so
/// the frequent round stays a pure PATH sweep.
pub(crate) const SHELL_RESOLVE_TTL: Duration = Duration::from_secs(30 * 60);

struct ShellResolveState {
    cache: Option<BTreeMap<String, String>>,
    key: String,
    resolved_at: Instant,
}

static SHELL_RESOLVE: Mutex<Option<ShellResolveState>> = Mutex::new(None);

/// `shellResolveEnvKey`: fingerprints the environment that determines what a
/// login shell resolves; any change invalidates the cache immediately.
pub(crate) fn shell_resolve_env_key() -> String {
    [
        std::env::var("PATH").unwrap_or_default(),
        std::env::var("SHELL").unwrap_or_default(),
        std::env::var("HOME").unwrap_or_default(),
    ]
    .join("\x00")
}

/// `defaultAgentCommandNames`: the command names the probe tries before any
/// PATCHBAY_*_PATH override. Built-in runtime identity commands (e.g. "omp") are
/// appended from the descriptor registry.
pub(crate) fn default_agent_command_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = vec![
        "claude",
        "codex",
        "opencode",
        "deveco",
        "openclaw",
        "hermes",
        "pi",
        "cursor-agent",
        "copilot",
        "kimi",
        "reasonix",
        "dsh",
        "kiro-cli",
        "codebuddy",
        "agy",
        "qodercli",
        "qoderclicn",
        "traecli",
        "grok",
        "qwen",
        "qwenpaw",
        "mcode",
        "dim",
    ];
    // agent.BuiltinRuntimeCommands()
    names.extend(builtin_runtime_commands());
    names
}

/// Default commands for built-in runtime identities.
pub(crate) fn builtin_runtime_commands() -> Vec<&'static str> {
    vec!["omp"]
}

/// Built-in runtime identity descriptors the daemon probes independently.
/// Adding a new fork is an entry here.
pub(crate) struct BuiltinRuntimeDesc {
    pub id: &'static str,
    pub env_prefix: &'static str,
    pub default_command: &'static str,
}

pub(crate) const BUILTIN_RUNTIMES: &[BuiltinRuntimeDesc] = &[BuiltinRuntimeDesc {
    id: "omp",
    env_prefix: "PATCHBAY_OMP",
    default_command: "omp",
}];

#[cfg(test)]
pub(crate) fn builtin_runtime_by_id(id: &str) -> Option<&'static BuiltinRuntimeDesc> {
    BUILTIN_RUNTIMES.iter().find(|d| d.id == id)
}

/// `codexDesktopAppBundlePaths`: candidate macOS app-bundle locations for the
/// bundled Codex CLI, ordered system-first, ChatGPT.app before Codex.app.
pub(crate) fn codex_desktop_app_bundle_paths() -> Vec<String> {
    let mut paths = vec![
        "/Applications/ChatGPT.app/Contents/Resources/codex".to_string(),
        "/Applications/Codex.app/Contents/Resources/codex".to_string(),
    ];
    if let Ok(home) = crate::execenv::execenv::user_home_dir() {
        for app in ["ChatGPT.app", "Codex.app"] {
            paths.push(crate::execenv::execenv::join_path(&[
                &home,
                "Applications",
                app,
                "Contents",
                "Resources",
                "codex",
            ]));
        }
    }
    paths
}

/// `cachedShellResolvedAgents`: resolves every standard agent command name
/// through the user's login shell, reusing the previous result for
/// SHELL_RESOLVE_TTL as long as the resolution-relevant env is unchanged.
pub(crate) fn cached_shell_resolved_agents() -> BTreeMap<String, String> {
    let mut state = SHELL_RESOLVE.lock().expect("shell resolve state");
    let key = shell_resolve_env_key();
    if let Some(existing) = state.as_ref() {
        if existing.cache.is_some()
            && existing.key == key
            && existing.resolved_at.elapsed() < SHELL_RESOLVE_TTL
        {
            return existing.cache.clone().unwrap_or_default();
        }
    }
    let names: Vec<String> = default_agent_command_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let resolved = resolve_agents_via_login_shell(&names);
    // Distinguish "resolved nothing" from "never resolved" so a failing shell
    // doesn't get re-forked on every probe inside the TTL window.
    let resolved = if resolved.is_empty() {
        // Still store the empty map with the key so the TTL applies.
        resolved
    } else {
        resolved
    };
    *state = Some(ShellResolveState {
        cache: Some(resolved.clone()),
        key,
        resolved_at: Instant::now(),
    });
    resolved
}

struct ProbeOutcome {
    entry: AgentEntry,
    found: bool,
}

fn probe(env_var: &str, default_cmd: &str, model_env: &str) -> ProbeOutcome {
    let cmd = env_or_default(env_var, default_cmd);
    if let Ok(path) = resolve_agent_executable_path(&cmd) {
        return ProbeOutcome {
            entry: agent_entry(&path, &cmd, model_env),
            found: true,
        };
    }
    // The shell fallback only rescues bare command names. An operator who
    // pinned PATCHBAY_*_PATH to a path that doesn't exist should hard-miss.
    if cmd.contains('/') || cmd.contains('\\') {
        return ProbeOutcome {
            entry: AgentEntry::default(),
            found: false,
        };
    }
    if let Some(path) = cached_shell_resolved_agents().get(&cmd) {
        return ProbeOutcome {
            entry: agent_entry(path, &cmd, model_env),
            found: true,
        };
    }
    if default_cmd == "codex" && cmd == default_cmd {
        // Codex Desktop bundles its CLI inside the macOS app instead of
        // installing it onto PATH.
        for p in codex_desktop_app_bundle_paths() {
            if std::fs::metadata(&p).is_ok() {
                return ProbeOutcome {
                    entry: agent_entry(&p, &cmd, model_env),
                    found: true,
                };
            }
        }
    }
    ProbeOutcome {
        entry: AgentEntry::default(),
        found: false,
    }
}

fn agent_entry(path: &str, cmd: &str, model_env: &str) -> AgentEntry {
    AgentEntry {
        path: path.to_string(),
        command: cmd.to_string(),
        model: std::env::var(model_env)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

/// `probeAgentCLIs`: discovers which built-in agent CLIs are installed on this
/// machine and returns one AgentEntry per provider that resolved. Pure
/// discovery — no version detection and no minimum-version gate.
pub fn probe_agent_clis() -> BTreeMap<String, AgentEntry> {
    let mut agents: BTreeMap<String, AgentEntry> = BTreeMap::new();

    fn add(agents: &mut BTreeMap<String, AgentEntry>, key: &str, outcome: ProbeOutcome) {
        if outcome.found {
            agents.insert(key.to_string(), outcome.entry);
        }
    }
    let mut add = |key: &str, outcome: ProbeOutcome| add(&mut agents, key, outcome);

    add(
        "claude",
        probe("PATCHBAY_CLAUDE_PATH", "claude", "PATCHBAY_CLAUDE_MODEL"),
    );
    add(
        "codex",
        probe("PATCHBAY_CODEX_PATH", "codex", "PATCHBAY_CODEX_MODEL"),
    );
    add(
        "opencode",
        probe(
            "PATCHBAY_OPENCODE_PATH",
            "opencode",
            "PATCHBAY_OPENCODE_MODEL",
        ),
    );
    add(
        "deveco",
        probe("PATCHBAY_DEVECO_PATH", "deveco", "PATCHBAY_DEVECO_MODEL"),
    );
    add(
        "openclaw",
        probe(
            "PATCHBAY_OPENCLAW_PATH",
            "openclaw",
            "PATCHBAY_OPENCLAW_MODEL",
        ),
    );
    add(
        "hermes",
        probe("PATCHBAY_HERMES_PATH", "hermes", "PATCHBAY_HERMES_MODEL"),
    );
    add("pi", probe("PATCHBAY_PI_PATH", "pi", "PATCHBAY_PI_MODEL"));
    // Built-in runtime identities are derived from the descriptor registry so
    // adding a new fork is a descriptor entry, not a probe edit.
    for desc in BUILTIN_RUNTIMES {
        let path_env = format!("{}_PATH", desc.env_prefix);
        let model_env = format!("{}_MODEL", desc.env_prefix);
        add(desc.id, probe(&path_env, desc.default_command, &model_env));
    }
    add(
        "cursor",
        probe(
            "PATCHBAY_CURSOR_PATH",
            "cursor-agent",
            "PATCHBAY_CURSOR_MODEL",
        ),
    );
    add(
        "copilot",
        probe("PATCHBAY_COPILOT_PATH", "copilot", "PATCHBAY_COPILOT_MODEL"),
    );
    add(
        "kimi",
        probe("PATCHBAY_KIMI_PATH", "kimi", "PATCHBAY_KIMI_MODEL"),
    );
    add(
        "reasonix",
        probe(
            "PATCHBAY_REASONIX_PATH",
            "reasonix",
            "PATCHBAY_REASONIX_MODEL",
        ),
    );
    // DSH registers only when its Patchbay runtime profile is installed: a bare
    // dsh binary has no --stdio protocol and every task would fail after
    // being advertised as healthy.
    let dsh = probe("PATCHBAY_DSH_PATH", "dsh", "PATCHBAY_DSH_MODEL");
    if dsh.found && probe_dsh_patchbay_profile(&dsh.entry.path) {
        add("dsh", dsh);
    }
    add(
        "kiro",
        probe("PATCHBAY_KIRO_PATH", "kiro-cli", "PATCHBAY_KIRO_MODEL"),
    );
    add(
        "codebuddy",
        probe(
            "PATCHBAY_CODEBUDDY_PATH",
            "codebuddy",
            "PATCHBAY_CODEBUDDY_MODEL",
        ),
    );
    add(
        "antigravity",
        probe(
            "PATCHBAY_ANTIGRAVITY_PATH",
            "agy",
            "PATCHBAY_ANTIGRAVITY_MODEL",
        ),
    );
    add(
        "qoder",
        probe("PATCHBAY_QODER_PATH", "qodercli", "PATCHBAY_QODER_MODEL"),
    );
    add(
        "qoderclicn",
        probe(
            "PATCHBAY_QODERCLICN_PATH",
            "qoderclicn",
            "PATCHBAY_QODERCLICN_MODEL",
        ),
    );
    add(
        "traecli",
        probe("PATCHBAY_TRAECLI_PATH", "traecli", "PATCHBAY_TRAECLI_MODEL"),
    );
    add(
        "grok",
        probe("PATCHBAY_GROK_PATH", "grok", "PATCHBAY_GROK_MODEL"),
    );
    add(
        "qwen",
        probe("PATCHBAY_QWEN_PATH", "qwen", "PATCHBAY_QWEN_MODEL"),
    );
    // QwenPaw takes no model env var: the backend never calls session/set_model.
    add("qwenpaw", probe("PATCHBAY_QWENPAW_PATH", "qwenpaw", ""));
    add(
        "dim",
        probe("PATCHBAY_DIM_PATH", "dim", "PATCHBAY_DIM_MODEL"),
    );
    // MiniMax Code model selection is owned by the runtime: no model env.
    add("mcode", probe("PATCHBAY_MCODE_PATH", "mcode", ""));
    agents
}

/// `probeDshPatchbayProfile`: runs `<dsh> --profile patchbay --probe` with a 5s cap
/// and accepts the run when one NDJSON frame identifies the Patchbay profile.
pub(crate) fn probe_dsh_patchbay_profile(executable_path: &str) -> bool {
    let Ok(mut child) = std::process::Command::new(executable_path)
        .args(["--profile", "patchbay", "--probe"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let output = match crate::config::wait_with_timeout(&mut child, Duration::from_secs(6)) {
        Ok(raw) => raw,
        Err(_) => {
            let _ = child.kill();
            return false;
        }
    };
    #[derive(serde::Deserialize)]
    struct Frame {
        #[serde(rename = "v")]
        _version: i64,
        #[serde(rename = "type")]
        frame_type: String,
        #[serde(rename = "runtime")]
        runtime: String,
        #[serde(rename = "protocol_version")]
        protocol_version: i64,
    }
    for line in output.split('\n') {
        if let Ok(frame) = serde_json::from_str::<Frame>(line.trim()) {
            if frame.frame_type == "probe"
                && frame.runtime == "dsh"
                && frame.protocol_version == 1
                && frame._version == 1
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_resolve_env_key_changes_with_path() {
        let k1 = shell_resolve_env_key();
        // Same env → same key.
        let k2 = shell_resolve_env_key();
        assert_eq!(k1, k2);
    }

    #[test]
    fn default_command_names_cover_the_probe_set() {
        let names = default_agent_command_names();
        for expected in ["claude", "codex", "omp", "qwenpaw", "mcode", "dim"] {
            assert!(names.contains(&expected), "missing {expected}");
        }
        assert_eq!(builtin_runtime_commands(), vec!["omp"]);
        assert!(builtin_runtime_by_id("omp").is_some());
        assert!(builtin_runtime_by_id("nope").is_none());
    }

    #[test]
    fn codex_bundle_paths_order() {
        let paths = codex_desktop_app_bundle_paths();
        assert_eq!(
            paths[0],
            "/Applications/ChatGPT.app/Contents/Resources/codex"
        );
        assert_eq!(paths[1], "/Applications/Codex.app/Contents/Resources/codex");
        // Home candidates follow.
        assert!(paths.len() >= 3);
        assert!(paths[2].contains("/Applications/ChatGPT.app"));
    }

    #[test]
    fn probe_of_missing_command_is_not_found() {
        // A command that cannot exist anywhere; the shell fallback may fork
        // but must still report not-found for a path-shaped override.
        let outcome = probe(
            "PATCHBAY_TEST_MISSING_PATH",
            "/definitely/not/a/binary-xyz",
            "",
        );
        assert!(!outcome.found);
    }
}
