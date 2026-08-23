//! Port of `server/internal/daemon/agents_probe.go` (296 lines).
//!
//! Deviations from Go:
//! - `var probeAgentCLIs = func() ...` (test stub seam) → plain fn plus a
//!   `#[cfg(test)]` resolver override hook, mirroring the shape config.rs
//!   already established for its login-shell tests.
//! - `resolveAgentsViaLoginShell` is private in config.rs and this crate's
//!   Cargo.toml is out of scope for visibility changes, so a local thin
//!   stand-in [`resolve_agents_via_login_shell`] re-implements it on top of
//!   the shared `build_login_shell_resolve_script` / `is_safe_agent_name`;
//!   S9-integration: swap to the config.rs implementation when the daemon
//!   core lane unifies them.
//! - `agent.BuiltinRuntimes` (server/pkg/agent) is not ported; only the
//!   probe-relevant descriptor fields are mirrored in [`builtin_runtimes`].
//! - `exec.CommandContext` + `WaitDelay` → spawn + reader thread +
//!   `recv_timeout`, then kill (same bounded-wait shape as config.rs).
//! - No slog output in this file.

// S9-integration: consumed by config.rs's probe_agent_clis seam stub and the
// manager/health wiring that lands with integration; silence dead-code until
// then.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
#[cfg(test)]
use std::sync::RwLock;
use std::time::Duration;

use crate::config::{
    build_login_shell_resolve_script, codex_desktop_app_bundle_paths, default_agent_command_names,
    is_safe_agent_name, resolve_agent_executable_path,
};
use crate::helpers::env_or_default;
use crate::types::AgentEntry;

/// `shellResolveTTL` (agents_probe.go:29): bounds how long one login-shell
/// PATH resolution is reused across probeAgentCLIs calls. Deliberately much
/// longer than agentDiscoveryInterval so the frequent discovery round stays a
/// pure exec.LookPath sweep — see the Go source comment block for the full
/// rationale.
const SHELL_RESOLVE_TTL: Duration = Duration::from_secs(30 * 60);

/// `loginShellResolveTimeout` (config.go:896), reused by the local shell
/// resolver stand-in.
const LOGIN_SHELL_RESOLVE_TIMEOUT: Duration = Duration::from_secs(3);

struct ShellResolveState {
    cache: HashMap<String, String>,
    key: String,
    resolved_at: std::time::Instant,
}

static SHELL_RESOLVE: Mutex<Option<ShellResolveState>> = Mutex::new(None);

/// Test-only override mirroring Go's `resolveAgentsViaLoginShell` package var
/// (agents_probe_omp_test.go:44–47).
#[cfg(test)]
static LOGIN_SHELL_RESOLVER_OVERRIDE:
    RwLock<Option<fn(&[String]) -> HashMap<String, String>>> = RwLock::new(None);

#[cfg(test)]
fn set_login_shell_resolver_for_tests(f: Option<fn(&[String]) -> HashMap<String, String>>) {
    *LOGIN_SHELL_RESOLVER_OVERRIDE.write().unwrap() = f;
}

/// `shellResolveEnvKey` (agents_probe.go:41–47): fingerprints the environment
/// that determines what a login shell resolves. A change to any of these
/// invalidates the cache immediately, independent of the TTL — the cached
/// answer was for a different environment.
fn shell_resolve_env_key() -> String {
    [
        std::env::var("PATH").unwrap_or_default(),
        std::env::var("SHELL").unwrap_or_default(),
        std::env::var("HOME").unwrap_or_default(),
    ]
    .join("\x00")
}

/// `cachedShellResolvedAgents` (agents_probe.go:57–74): resolves every
/// standard agent command name through the user's login shell, reusing the
/// previous result for SHELL_RESOLVE_TTL as long as the resolution-relevant
/// environment is unchanged. A failing shell resolves to an empty map that is
/// still cached, so it isn't re-forked on every probe inside the TTL window.
fn cached_shell_resolved_agents() -> HashMap<String, String> {
    let mut state = SHELL_RESOLVE.lock().unwrap();
    let key = shell_resolve_env_key();
    if let Some(cached) = state.as_ref() {
        if cached.key == key && cached.resolved_at.elapsed() < SHELL_RESOLVE_TTL {
            return cached.cache.clone();
        }
    }
    let names = default_agent_command_names();
    let resolved = {
        #[cfg(test)]
        {
            match *LOGIN_SHELL_RESOLVER_OVERRIDE.read().unwrap() {
                Some(f) => f(&names),
                None => resolve_agents_via_login_shell(&names),
            }
        }
        #[cfg(not(test))]
        {
            resolve_agents_via_login_shell(&names)
        }
    };
    let resolved = if resolved.is_empty() {
        // Distinguish "resolved nothing" from "never resolved" so a failing
        // shell doesn't get re-forked on every probe inside the TTL window.
        HashMap::new()
    } else {
        resolved
    };
    *state = Some(ShellResolveState {
        cache: resolved.clone(),
        key,
        resolved_at: std::time::Instant::now(),
    });
    resolved
}

/// S9-integration stand-in for config.rs's private
/// `resolve_agents_via_login_shell` (config.go:956–1010): ask `$SHELL -ilc`
/// for an absolute, invocation-safe path per name. Empty map when the shell
/// is unavailable / unsupported / times out / yields nothing usable.
fn resolve_agents_via_login_shell(names: &[String]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if names.is_empty() {
        return out;
    }
    let shell = std::env::var("SHELL").unwrap_or_default();
    let shell = shell.trim().to_string();
    if shell.is_empty() {
        return out;
    }
    let shell_base = Path::new(&shell)
        .file_name()
        .map(|b| b.to_string_lossy().into_owned())
        .unwrap_or_default();
    // supportedLoginShells (config.go:914–920): POSIX-compatible shells only.
    if !matches!(shell_base.as_str(), "bash" | "zsh" | "sh" | "dash" | "ksh") {
        return out;
    }
    let safe: Vec<String> = names.iter().filter(|n| is_safe_agent_name(n)).cloned().collect();
    if safe.is_empty() {
        return out;
    }

    let script = build_login_shell_resolve_script(&safe);
    let mut child = match std::process::Command::new(&shell)
        .arg("-ilc")
        .arg(&script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return out,
    };

    let stdout = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        if let Some(mut s) = stdout {
            let _ = s.read_to_end(&mut buf);
        }
        let _ = tx.send(buf);
    });
    let raw = match rx.recv_timeout(LOGIN_SHELL_RESOLVE_TIMEOUT) {
        Ok(buf) => String::from_utf8_lossy(&buf).into_owned(),
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return out;
        }
    };

    for line in raw.trim().split('\n') {
        let Some((name, path)) = line.split_once('\t') else {
            continue;
        };
        let path = path.trim();
        if !Path::new(path).is_absolute() || !is_executable_file(path) {
            continue;
        }
        out.insert(name.to_string(), path.to_string());
    }
    out
}

/// exec.LookPath equivalent for an absolute candidate.
fn is_executable_file(path: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(windows)]
    {
        std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
    }
}

/// S9-integration mirror of `agent.BuiltinRuntime`
/// (server/pkg/agent/builtin_runtimes.go:18–70), restricted to the fields the
/// probe loop reads. The full descriptor registry lands with the pkg/agent
/// port; adding a new fork there extends this table.
pub(crate) struct BuiltinRuntimeProbe {
    pub id: &'static str,
    pub default_command: &'static str,
    pub env_prefix: &'static str,
}

/// `agent.BuiltinRuntimes` (builtin_runtimes.go:87–101): currently just omp
/// (oh-my-pi), a separate CLI speaking the pi JSON event protocol.
pub(crate) fn builtin_runtimes() -> &'static [BuiltinRuntimeProbe] {
    &[BuiltinRuntimeProbe {
        id: "omp",
        default_command: "omp",
        env_prefix: "CORDY_OMP",
    }]
}

/// One probe leg (agents_probe.go:115–152): env-pinned command first, then
/// the login-shell fallback for bare names only, then the Codex Desktop app
/// bundle for codex specifically.
fn probe(env_var: &str, default_cmd: &str, model_env: &str) -> Option<AgentEntry> {
    let cmd = env_or_default(env_var, default_cmd);
    if let Ok(path) = resolve_agent_executable_path(&cmd) {
        return Some(agent_entry(path, &cmd, model_env));
    }
    // The shell fallback only rescues bare command names. An operator who
    // pinned CORDY_*_PATH to an absolute or relative path that doesn't exist
    // should hard-miss, not silently get a different binary.
    if cmd.contains('/') || cmd.contains('\\') {
        return None;
    }
    if let Some(path) = cached_shell_resolved_agents().get(&cmd) {
        return Some(agent_entry(path.clone(), &cmd, model_env));
    }
    if default_cmd == "codex" && cmd == default_cmd {
        // Codex Desktop bundles its CLI inside the macOS app instead of
        // installing it onto PATH.
        for p in codex_desktop_app_bundle_paths() {
            if Path::new(&p).exists() {
                return Some(agent_entry(p, &cmd, model_env));
            }
        }
    }
    None
}

fn agent_entry(path: String, cmd: &str, model_env: &str) -> AgentEntry {
    AgentEntry {
        path,
        command: cmd.to_string(),
        model: std::env::var(model_env).unwrap_or_default().trim().to_string(),
    }
}

/// `probeAgentCLIs` (agents_probe.go:91–273): discovers which built-in agent
/// CLIs are installed on this machine and returns one AgentEntry per provider
/// that resolved. Pure discovery: no version detection and no minimum-version
/// gate. Called once from LoadConfig at startup and again from the periodic
/// workspace sync, so a CLI installed while the daemon runs is picked up
/// without a restart (MUL-5439).
///
/// S9-integration: config.rs currently carries an empty seam stub with this
/// name; the daemon-core lane rewires it to call this function.
pub(crate) fn probe_agent_clis() -> HashMap<String, AgentEntry> {
    let mut agents: HashMap<String, AgentEntry> = HashMap::new();

    macro_rules! register {
        ($key:expr, $env:expr, $default:expr, $model:expr) => {
            if let Some(e) = probe($env, $default, $model) {
                agents.insert($key.to_string(), e);
            }
        };
    }

    register!("claude", "CORDY_CLAUDE_PATH", "claude", "CORDY_CLAUDE_MODEL");
    register!("codex", "CORDY_CODEX_PATH", "codex", "CORDY_CODEX_MODEL");
    register!("opencode", "CORDY_OPENCODE_PATH", "opencode", "CORDY_OPENCODE_MODEL");
    register!("deveco", "CORDY_DEVECO_PATH", "deveco", "CORDY_DEVECO_MODEL");
    register!("openclaw", "CORDY_OPENCLAW_PATH", "openclaw", "CORDY_OPENCLAW_MODEL");
    register!("hermes", "CORDY_HERMES_PATH", "hermes", "CORDY_HERMES_MODEL");
    register!("pi", "CORDY_PI_PATH", "pi", "CORDY_PI_MODEL");
    // Built-in runtime identities (e.g. omp) are derived from the descriptor
    // registry; each one probes a separate CLI independently so a host with
    // both pi and omp installed gets two runtimes (agents_probe.go:176–187).
    for desc in builtin_runtimes() {
        let path_env = format!("{}_PATH", desc.env_prefix);
        let model_env = format!("{}_MODEL", desc.env_prefix);
        if let Some(e) = probe(&path_env, desc.default_command, &model_env) {
            agents.insert(desc.id.to_string(), e);
        }
    }
    register!("cursor", "CORDY_CURSOR_PATH", "cursor-agent", "CORDY_CURSOR_MODEL");
    register!("copilot", "CORDY_COPILOT_PATH", "copilot", "CORDY_COPILOT_MODEL");
    register!("kimi", "CORDY_KIMI_PATH", "kimi", "CORDY_KIMI_MODEL");
    register!("reasonix", "CORDY_REASONIX_PATH", "reasonix", "CORDY_REASONIX_MODEL");
    // DSH is registered only when its Cordy runtime profile is installed. A
    // bare dsh binary is not enough: without the bundle it has no --stdio
    // protocol and every task would fail after being advertised as healthy
    // (agents_probe.go:200–205).
    if let Some(e) = probe("CORDY_DSH_PATH", "dsh", "CORDY_DSH_MODEL") {
        if probe_dsh_cordy_profile(&e.path) {
            agents.insert("dsh".to_string(), e);
        }
    }
    register!("kiro", "CORDY_KIRO_PATH", "kiro-cli", "CORDY_KIRO_MODEL");
    register!("codebuddy", "CORDY_CODEBUDDY_PATH", "codebuddy", "CORDY_CODEBUDDY_MODEL");
    // agy 1.0.6 added a --model flag (MUL-3125); CORDY_ANTIGRAVITY_MODEL
    // seeds the daemon-wide default as the exact `agy models` display string
    // (agents_probe.go:212–218).
    register!("antigravity", "CORDY_ANTIGRAVITY_PATH", "agy", "CORDY_ANTIGRAVITY_MODEL");
    // Qoder CLI ships as the `qodercli` binary and must go through probe()
    // like every other provider so the login-shell fallback applies
    // (MUL-5524, agents_probe.go:219–227).
    register!("qoder", "CORDY_QODER_PATH", "qodercli", "CORDY_QODER_MODEL");
    // Qoder CN CLI exposes the same ACP transport under a separate binary and
    // account/config root (agents_probe.go:228–234).
    register!("qoderclicn", "CORDY_QODERCLICN_PATH", "qoderclicn", "CORDY_QODERCLICN_MODEL");
    // ByteDance TRAE CLI over ACP via `traecli acp serve --yolo`
    // (agents_probe.go:235–241).
    register!("traecli", "CORDY_TRAECLI_PATH", "traecli", "CORDY_TRAECLI_MODEL");
    // xAI Grok Build CLI over ACP via `grok agent --always-approve stdio`
    // (agents_probe.go:242–247).
    register!("grok", "CORDY_GROK_PATH", "grok", "CORDY_GROK_MODEL");
    // Qwen Code runs headlessly with -p and stream-json
    // (agents_probe.go:248–252).
    register!("qwen", "CORDY_QWEN_PATH", "qwen", "CORDY_QWEN_MODEL");
    // QwenPaw takes no model env var: the backend never calls session/
    // set_model, so reading one here would advertise a knob that silently
    // does nothing (agents_probe.go:253–260).
    register!("qwenpaw", "CORDY_QWENPAW_PATH", "qwenpaw", "");
    // Dim over ACP via `dim acp` (agents_probe.go:261–266).
    register!("dim", "CORDY_DIM_PATH", "dim", "CORDY_DIM_MODEL");
    // MiniMax Code exposes an ACP v1 server through `mcode acp`; model
    // selection is owned by the MCode runtime (agents_probe.go:267–271).
    register!("mcode", "CORDY_MCODE_PATH", "mcode", "");
    agents
}

/// `probeDshCordyProfile` (agents_probe.go:275–296): run
/// `<exe> --profile cordy --probe` under a 5s timeout (Go WaitDelay 1s) and
/// look for the NDJSON frame identifying a compatible DSH install.
fn probe_dsh_cordy_profile(executable_path: &str) -> bool {
    let mut child = match std::process::Command::new(executable_path)
        .args(["--profile", "cordy", "--probe"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let stdout = child.stdout.take();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        if let Some(mut s) = stdout {
            let _ = s.read_to_end(&mut buf);
        }
        let _ = tx.send(buf);
    });
    let output = match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(buf) => buf,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
    };
    let _ = child.wait();
    for line in String::from_utf8_lossy(&output).split('\n') {
        let Ok(frame) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if frame.get("v").and_then(|v| v.as_i64()) == Some(1)
            && frame.get("type").and_then(|v| v.as_str()) == Some("probe")
            && frame.get("runtime").and_then(|v| v.as_str()) == Some("dsh")
            && frame.get("protocol_version").and_then(|v| v.as_i64()) == Some(1)
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes env-var mutation across tests (Go's t.Setenv semantics).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn write_executable(path: &Path, content: &str) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(path, content).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(windows)]
        {
            let _ = content;
            std::fs::write(path, "").unwrap();
        }
    }

    struct EnvGuard(Vec<(String, Option<String>)>);
    impl EnvGuard {
        fn set(key: &str, value: &str) -> Self {
            let old = std::env::var(key).ok();
            std::env::set_var(key, value);
            EnvGuard(vec![(key.to_string(), old)])
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, old) in &self.0 {
                match old {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    /// resetShellResolveCacheForTest: clear the process-wide TTL cache and
    /// stub the resolver so tests never fork a real login shell.
    fn reset_shell_resolve_cache() {
        SHELL_RESOLVE.lock().unwrap().take();
        set_login_shell_resolver_for_tests(Some(|_| HashMap::new()));
    }

    /// TestProbeDshCordyProfile (agents_probe_dsh_test.go:10–38).
    #[test]
    fn probe_dsh_cordy_profile_frames() {
        let cases: [(&str, &str, bool); 3] = [
            (
                "compatible",
                r#"printf '%s\n' '{"v":1,"type":"probe","runtime":"dsh","plugin_version":"test","protocol_version":1}'"#,
                true,
            ),
            ("missing plugin", r#"printf '%s\n' 'profile not installed'"#, false),
            (
                "future protocol",
                r#"printf '%s\n' '{"v":2,"type":"probe","runtime":"dsh","protocol_version":2}'"#,
                false,
            ),
        ];
        for (name, body, want) in cases {
            let dir = tempfile::tempdir().unwrap().keep();
            let path = dir.join("dsh");
            write_executable(&path, &format!("#!/bin/sh\nset -eu\n{body}\n"));
            assert_eq!(probe_dsh_cordy_profile(&path.to_string_lossy()), want, "case {name}");
        }
    }

    /// TestProbeAgentCLIsRequiresDshCordyProfile
    /// (agents_probe_dsh_test.go:40–73): dsh is only discovered when its
    /// Cordy profile probe passes.
    #[test]
    fn probe_agent_clis_requires_dsh_cordy_profile() {
        let _env = ENV_LOCK.lock().unwrap();
        reset_shell_resolve_cache();
        let cases: [(&str, bool); 2] = [
            (
                "profile installed",
                r#"printf '%s\n' '{"v":1,"type":"probe","runtime":"dsh","protocol_version":1}'"#,
                true,
            ),
            ("profile missing", r#"printf '%s\n' 'missing cordy profile'; exit 1"#, false),
        ];
        for (name, body, want) in cases {
            let fake_dir = tempfile::tempdir().unwrap().keep();
            let path = fake_dir.join("dsh");
            write_executable(&path, &format!("#!/bin/sh\nset -eu\n{body}\n"));
            let _guard = EnvGuard::set("PATH", &fake_dir.to_string_lossy());
            let _guard = EnvGuard::set("CORDY_DSH_PATH", "");
            let found = probe_agent_clis().contains_key("dsh");
            assert_eq!(found, want, "case {name}");
        }
    }

    /// TestProbeAgentCLIs_DiscoversPiAndOmpSeparately
    /// (agents_probe_omp_test.go:21–68): pi and omp are separate entries with
    /// their own commands.
    #[test]
    fn discovers_pi_and_omp_separately() {
        let _env = ENV_LOCK.lock().unwrap();
        reset_shell_resolve_cache();

        let fake_dir = tempfile::tempdir().unwrap().keep();
        for name in ["pi", "omp"] {
            write_executable(&fake_dir.join(name), "#!/bin/sh\nexit 0\n");
        }
        let _guard = EnvGuard::set("PATH", &fake_dir.to_string_lossy());
        let _guard = EnvGuard::set("CORDY_PI_PATH", "");
        let _guard = EnvGuard::set("CORDY_OMP_PATH", "");

        let agents = probe_agent_clis();
        let pi = agents.get("pi").expect("pi not discovered");
        let omp = agents.get("omp").expect("omp not discovered");
        assert_eq!(pi.command, "pi");
        assert_eq!(omp.command, "omp");
    }

    /// TestProbeAgentCLIs_QoderLoginShellFallback
    /// (agents_probe_qoder_test.go): an empty PATH forces the LookPath leg to
    /// miss, so qoder/qoderclicn can only come from the login-shell fallback.
    #[test]
    fn qoder_and_qoderclicn_via_login_shell_fallback() {
        let _env = ENV_LOCK.lock().unwrap();
        reset_shell_resolve_cache();

        let resolved_dir = tempfile::tempdir().unwrap().keep();
        for name in ["qodercli", "qoderclicn"] {
            write_executable(&resolved_dir.join(name), "#!/bin/sh\nexit 0\n");
        }
        let resolved_map: HashMap<String, String> = [
            ("qodercli".to_string(), resolved_dir.join("qodercli").to_string_lossy().into_owned()),
            (
                "qoderclicn".to_string(),
                resolved_dir.join("qoderclicn").to_string_lossy().into_owned(),
            ),
        ]
        .into_iter()
        .collect();
        set_login_shell_resolver_for_tests(Some(move |_| resolved_map.clone()));

        let _guard = EnvGuard::set("PATH", "");
        let _guard = EnvGuard::set("CORDY_QODER_PATH", "");
        let _guard = EnvGuard::set("CORDY_QODERCLICN_PATH", "");

        let agents = probe_agent_clis();
        let qoder = agents.get("qoder").expect("qoder not discovered via login-shell fallback");
        assert_eq!(qoder.command, "qodercli");
        let cn = agents.get("qoderclicn").expect("qoderclicn not discovered via login-shell fallback");
        assert_eq!(cn.command, "qoderclicn");
    }

    /// shellResolveEnvKey sensitivity (agents_probe.go:38–47): any change to
    /// PATH/SHELL/HOME invalidates the cache independent of the TTL.
    #[test]
    fn shell_resolve_cache_invalidated_by_env_change() {
        let _env = ENV_LOCK.lock().unwrap();
        reset_shell_resolve_cache();

        let first = {
            set_login_shell_resolver_for_tests(Some(|_| {
                [("claude".to_string(), "/bin/claude".to_string())].into_iter().collect()
            }));
            cached_shell_resolved_agents()
        };
        assert_eq!(first.get("claude").map(String::as_str), Some("/bin/claude"));

        // Same env within TTL → cached answer even though the resolver now
        // returns something else.
        set_login_shell_resolver_for_tests(Some(|_| HashMap::new()));
        assert_eq!(
            cached_shell_resolved_agents().get("claude").map(String::as_str),
            Some("/bin/claude")
        );

        // Changing PATH busts the cache immediately.
        let _guard = EnvGuard::set("PATH", "/nonexistent-probe-path");
        assert!(cached_shell_resolved_agents().get("claude").is_none());
    }
}
