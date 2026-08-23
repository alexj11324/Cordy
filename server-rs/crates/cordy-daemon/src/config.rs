//! Port of `server/internal/daemon/config.go` (lines 1–1143).
//!
//! Daemon configuration: defaults, the full [`Config`] struct, the
//! env/flag/CLI-config resolution pipeline in [`load_config`], server-URL
//! normalization, workspaces-root resolution, agent executable discovery
//! (PATH + login-shell fallback + hooks-dir unshadowing), and the OpenClaw
//! config-file → env-var bridge.
//!
//! Deviations from Go:
//! - `probeAgentCLIs` (agents_probe.go, lane B) is a seam returning an empty
//!   map; LoadConfig keeps the no-agents error and callers gate on
//!   `Overrides::allow_no_agents` until lane B lands.
//!   S9-integration: swap for the real probe.
//! - `cli.LoadCLIConfigForProfile` / `cli.CLIConfig` (internal/cli) are
//!   mirrored as minimal local serde types; the file load itself is a seam
//!   returning None. S9-integration: wire to the CLI crate.
//! - `go-shellwords.Parse` → local POSIX-ish tokenizer [`shell_split`]
//!   (quotes + backslash escapes; no backticks/env expansion, matching
//!   go-shellwords defaults).
//! - `agent.BuiltinRuntimeCommands()` (pkg/agent, not ported) is not appended
//!   to [`DEFAULT_AGENT_COMMAND_NAMES`] yet. S9-integration: append when the
//!   descriptor registry lands.
//! - The login-shell resolver's Go test-injection var becomes a
//!   test-only hook ([`set_login_shell_resolver_for_tests`]).
//! - Field names are snake_case mirrors of the Go fields so gc.rs's
//!   `GcConfig` (config.go:99–118 subset) can later be replaced by a
//!   projection — see [`Config::to_gc_config`], which already builds the
//!   real `crate::gc::GcConfig`.

// S9-integration: consumed by daemon bootstrap wiring that lands with
// integration; silence dead-code until then.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;

use crate::helpers::{bool_from_env, duration_from_env, env_or_default, int_from_env};
use crate::types::AgentEntry;

/// `canonicalExecutablePath` (config.go:835–847): absolutize, then resolve
/// symlinks; on failure keep the previous-best path and log at debug.
pub(crate) fn canonical_executable_path(path: &str) -> String {
    let abs = match absolute(path) {
        Ok(abs) => abs,
        Err(err) => {
            tracing::debug!(
                path = %path,
                error = %err,
                "make agent executable path absolute failed; keeping configured path"
            );
            return path.to_string();
        }
    };
    let abs_str = abs.to_string_lossy().into_owned();
    match crate::canonical_path::canonical_path(&abs_str) {
        Ok(real) => real.to_string_lossy().into_owned(),
        Err(err) => {
            tracing::debug!(
                path = %abs_str,
                error = %err,
                "canonicalize agent executable path failed; keeping absolute path"
            );
            abs_str
        }
    }
}

/// `isExecutableFile` (config.go:849–855): exists, not a directory, and has
/// any execute bit set. Windows has no POSIX mode bits — Go's os.Stat on
/// Windows synthesizes them from the file extension; here we only check
/// existence + non-directory there.
pub(crate) fn is_executable_file(path: &str) -> bool {
    let Ok(info) = std::fs::metadata(path) else {
        return false;
    };
    if info.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        info.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// `samePathDir` (config.go:815–833): lexical-absolutize both sides, then
/// best-effort symlink resolution before comparing.
pub(crate) fn same_path_dir(a: &str, b: &str) -> bool {
    use std::path::Component;

    fn clean_abs(p: &str) -> Option<PathBuf> {
        let raw = if Path::new(p).is_absolute() {
            PathBuf::from(p)
        } else {
            std::env::current_dir().ok()?.join(p)
        };
        let mut out = PathBuf::new();
        for comp in raw.components() {
            match comp {
                Component::CurDir => {}
                Component::ParentDir => {
                    if !matches!(out.components().next_back(), Some(Component::Normal(_))) {
                        out.push("..");
                    } else {
                        out.pop();
                    }
                }
                other => out.push(other.as_os_str()),
            }
        }
        Some(out)
    }

    let mut abs_a = match clean_abs(a) {
        Some(p) => p,
        None => return false,
    };
    let mut abs_b = match clean_abs(b) {
        Some(p) => p,
        None => return false,
    };
    if let Ok(real) = std::fs::canonicalize(&abs_a) {
        abs_a = real;
    }
    if let Ok(real) = std::fs::canonicalize(&abs_b) {
        abs_b = real;
    }
    abs_a == abs_b
}

/// `filepath.Abs` equivalent shared by config-side callers (kept local to this
/// module; canonical_path.rs has its own copy for its seam).
fn absolute(path: &str) -> std::io::Result<PathBuf> {
    let p = Path::new(path);
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    Ok(std::env::current_dir()?.join(p))
}

// ---------------------------------------------------------------------------
// Defaults (config.go:21–88).
// ---------------------------------------------------------------------------

pub(crate) const DEFAULT_SERVER_URL: &str = "ws://localhost:8080/ws";
pub(crate) const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(30);
pub(crate) const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// `DefaultAgentTimeout` (config.go:25–30): 0 = no wall-clock cap; a run is
/// bounded only by the inactivity watchdogs (MUL-3064).
pub(crate) const DEFAULT_AGENT_TIMEOUT: Duration = Duration::ZERO;
pub(crate) const DEFAULT_CODEX_SEMANTIC_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub(crate) const DEFAULT_CODEX_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// `DefaultOpenCodeIdleWatchdog` (config.go:33–38).
pub(crate) const DEFAULT_OPEN_CODE_IDLE_WATCHDOG: Duration = Duration::from_secs(10 * 60);
/// `DefaultAgentIdleWatchdog` (config.go:39–51): per-task force-stop when the
/// backend emits nothing for this long with an empty queue (MUL-2300).
pub(crate) const DEFAULT_AGENT_IDLE_WATCHDOG: Duration = Duration::from_secs(30 * 60);
/// `DefaultAgentToolWatchdog` (config.go:52–61): backstop for a tool_use that
/// never gets a tool_result now that there is no wall-clock cap (MUL-3064).
pub(crate) const DEFAULT_AGENT_TOOL_WATCHDOG: Duration = Duration::from_secs(2 * 3600);
pub(crate) const DEFAULT_RUNTIME_NAME: &str = "Local Agent";
pub(crate) const DEFAULT_WORKSPACE_BOOTSTRAP_SYNC_INTERVAL: Duration = Duration::from_secs(30);
pub(crate) const DEFAULT_WORKSPACE_LEGACY_SYNC_INTERVAL: Duration = Duration::from_secs(5 * 60);
pub(crate) const DEFAULT_WORKSPACE_SYNC_INTERVAL: Duration = Duration::from_secs(30 * 60);
pub(crate) const DEFAULT_WORKSPACE_SYNC_MAX_BACKOFF: Duration = Duration::from_secs(30 * 60);
pub(crate) const DEFAULT_HEALTH_PORT: i64 = 19514;
pub(crate) const DEFAULT_MAX_CONCURRENT_TASKS: i64 = 20;
pub(crate) const DEFAULT_GC_INTERVAL: Duration = Duration::from_secs(2 * 3600);
pub(crate) const DEFAULT_GC_TTL: Duration = Duration::from_secs(24 * 3600);
pub(crate) const DEFAULT_GC_COMPLETED_TASK_TTL_CLOUD: Duration =
    Duration::from_secs(14 * 24 * 3600);
pub(crate) const DEFAULT_GC_COMPLETED_TASK_TTL_SELF_HOST: Duration = Duration::ZERO;
pub(crate) const DEFAULT_GC_ORPHAN_TTL: Duration = Duration::from_secs(72 * 3600);
pub(crate) const DEFAULT_GC_ARTIFACT_TTL: Duration = Duration::from_secs(12 * 3600);
pub(crate) const DEFAULT_GC_CODEX_SESSION_TTL: Duration = Duration::from_secs(14 * 24 * 3600);
pub(crate) const DEFAULT_GC_HERMES_MEMORY_TTL: Duration = Duration::from_secs(90 * 24 * 3600);
pub(crate) const DEFAULT_GC_HERMES_SESSION_TTL: Duration = Duration::from_secs(14 * 24 * 3600);
pub(crate) const DEFAULT_GC_REPO_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
pub(crate) const DEFAULT_AUTO_UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 3600);

/// `DefaultGCArtifactPatterns` (config.go:82–88): conservative basename
/// matches that are always cheap to recreate.
pub(crate) fn default_gc_artifact_patterns() -> Vec<String> {
    vec![
        "node_modules".to_string(),
        ".next".to_string(),
        ".turbo".to_string(),
    ]
}

/// `Config` (config.go:91–147): all daemon configuration. Field names are
/// snake_case mirrors of the Go fields; the GC subset matches gc.rs's
/// `GcConfig` exactly (see [`Config::to_gc_config`]).
#[derive(Debug, Clone, Default)]
pub(crate) struct Config {
    pub server_base_url: String,
    pub daemon_id: String,
    /// Historical daemon_ids this machine may have registered under.
    pub legacy_daemon_ids: Vec<String>,
    pub device_name: String,
    pub runtime_name: String,
    /// cordy CLI version (e.g. "0.1.13").
    pub cli_version: String,
    /// "desktop" when spawned by the Electron app, empty for standalone.
    pub launched_by: String,
    /// profile name (empty = default)
    pub profile: String,
    /// keyed by provider
    pub agents: HashMap<String, AgentEntry>,
    /// base path for execution envs (default: ~/cordy_workspaces)
    pub workspaces_root: String,
    /// preserve env after task for debugging
    pub keep_env_after_task: bool,
    /// local HTTP port for health checks (default: 19514)
    pub health_port: i64,
    /// max tasks running in parallel (default: 20)
    pub max_concurrent_tasks: i64,
    pub gc_enabled: bool,
    pub gc_interval: Duration,
    pub gc_ttl: Duration,
    pub gc_completed_task_ttl: Duration,
    pub gc_orphan_ttl: Duration,
    pub gc_artifact_ttl: Duration,
    pub gc_artifact_patterns: Vec<String>,
    pub gc_repo_ttl: Duration,
    pub gc_repo_maintenance_enabled: bool,
    pub gc_codex_session_ttl: Duration,
    pub gc_hermes_memory_ttl: Duration,
    pub gc_hermes_session_ttl: Duration,
    pub auto_update_enabled: bool,
    pub auto_update_check_interval: Duration,
    pub auto_reload_enabled: bool,
    pub poll_interval: Duration,
    pub heartbeat_interval: Duration,
    pub agent_timeout: Duration,
    pub codex_semantic_inactivity_timeout: Duration,
    /// Explicit Codex first-turn ceiling override; 0 = unset (GH #3262).
    pub codex_first_turn_no_progress_timeout: Duration,
    pub codex_handshake_timeout: Duration,
    /// OpenCode-specific no-message window; 0 falls back to agent_idle_watchdog.
    pub open_code_idle_watchdog: Duration,
    pub agent_idle_watchdog: Duration,
    pub agent_tool_watchdog: Duration,
    pub claude_args: Vec<String>,
    pub codex_args: Vec<String>,
    pub codebuddy_args: Vec<String>,
    pub qwen_args: Vec<String>,
    pub qwenpaw_args: Vec<String>,
    /// custom runtime profile_id → absolute executable path on THIS machine
    /// (MUL-3284). Empty means "always resolve via PATH".
    pub profile_command_overrides: HashMap<String, String>,
}

impl Config {
    /// Projection onto gc.rs's `GcConfig` (the config.go:99–118 subset the GC
    /// loop consumes); keeps the two structs aligned until gc.rs consolidates
    /// onto Config directly.
    pub(crate) fn to_gc_config(&self) -> crate::gc::GcConfig {
        crate::gc::GcConfig {
            profile: self.profile.clone(),
            workspaces_root: PathBuf::from(&self.workspaces_root),
            gc_enabled: self.gc_enabled,
            gc_interval: self.gc_interval,
            gc_ttl: self.gc_ttl,
            gc_completed_task_ttl: self.gc_completed_task_ttl,
            gc_orphan_ttl: self.gc_orphan_ttl,
            gc_artifact_ttl: self.gc_artifact_ttl,
            gc_codex_session_ttl: self.gc_codex_session_ttl,
            gc_hermes_memory_ttl: self.gc_hermes_memory_ttl,
            gc_hermes_session_ttl: self.gc_hermes_session_ttl,
            gc_repo_ttl: self.gc_repo_ttl,
            gc_repo_maintenance_enabled: self.gc_repo_maintenance_enabled,
            gc_artifact_patterns: self.gc_artifact_patterns.clone(),
        }
    }
}

/// `Overrides` (config.go:151–179): CLI flags overriding env vars and
/// defaults. Zero values are ignored.
#[derive(Debug, Clone, Default)]
pub(crate) struct Overrides {
    pub server_url: String,
    pub workspaces_root: String,
    pub poll_interval: Duration,
    pub heartbeat_interval: Duration,
    /// `Option<Duration>` mirrors Go's pointer: an explicit `--agent-timeout 0`
    /// (no cap) is distinguishable from "flag not passed".
    pub agent_timeout: Option<Duration>,
    pub codex_semantic_inactivity_timeout: Duration,
    pub codex_handshake_timeout: Duration,
    pub max_concurrent_tasks: i64,
    pub daemon_id: String,
    pub device_name: String,
    pub runtime_name: String,
    pub profile: String,
    pub health_port: i64,
    /// Reserved for read-only local configuration probes; startup still
    /// refuses to run with no agent CLI unless this is set.
    pub allow_no_agents: bool,
    pub disable_auto_update: bool,
    pub auto_update_check_interval: Duration,
    pub disable_auto_reload: bool,
}

// ---------------------------------------------------------------------------
// URL helpers (config.go:566–633).
// ---------------------------------------------------------------------------

/// Minimal net/url surface: `(scheme, authority, path, query, fragment)`.
fn split_url(raw: &str) -> Option<(String, String, String, String, String)> {
    let (scheme, rest) = raw.split_once("://")?;
    let (no_frag, fragment) = match rest.split_once('#') {
        Some((nf, f)) => (nf, f.to_string()),
        None => (rest, String::new()),
    };
    let (authority_path, query) = match no_frag.split_once('?') {
        Some((ap, q)) => (ap, q.to_string()),
        None => (no_frag, String::new()),
    };
    let (authority, path) = match authority_path.find('/') {
        Some(idx) => (
            authority_path[..idx].to_string(),
            authority_path[idx..].to_string(),
        ),
        None => (authority_path.to_string(), String::new()),
    };
    Some((scheme.to_string(), authority, path, query, fragment))
}

/// `url.Hostname`: userinfo stripped, port stripped, IPv6 brackets stripped.
fn url_hostname(authority: &str) -> &str {
    let host = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    if host.starts_with('[') {
        return host.trim_start_matches('[').trim_end_matches(']');
    }
    host.split(':').next().unwrap_or(host)
}

/// `officialCloudHost` (config.go:570).
const OFFICIAL_CLOUD_HOST: &str = "api.cordy.ai";

/// `isOfficialCloudServer` (config.go:580–586): host-only, case-insensitive;
/// port and path ignored.
pub(crate) fn is_official_cloud_server(base_url: &str) -> bool {
    let Some((_, authority, ..)) = split_url(base_url.trim()) else {
        return false;
    };
    url_hostname(&authority).eq_ignore_ascii_case(OFFICIAL_CLOUD_HOST)
}

/// `defaultGCCompletedTaskTTL` (config.go:604–609): full env removal is
/// irreversible — cloud bounds retention, self-host opts out by default.
pub(crate) fn default_gc_completed_task_ttl(server_base_url: &str) -> Duration {
    if is_official_cloud_server(server_base_url) {
        DEFAULT_GC_COMPLETED_TASK_TTL_CLOUD
    } else {
        DEFAULT_GC_COMPLETED_TASK_TTL_SELF_HOST
    }
}

/// `NormalizeServerBaseURL` (config.go:612–633): ws(s)→http(s), drop a bare
/// "/ws" path, strip query/fragment/trailing slash.
pub(crate) fn normalize_server_base_url(raw: &str) -> anyhow::Result<String> {
    let (scheme, authority, path, _, _) =
        split_url(raw.trim()).context("invalid CORDY_SERVER_URL")?;
    let out_scheme = match scheme.as_str() {
        "ws" => "http",
        "wss" => "https",
        "http" | "https" => scheme.as_str(),
        _ => anyhow::bail!("CORDY_SERVER_URL must use ws, wss, http, or https"),
    };
    let path = if path == "/ws" { String::new() } else { path };
    let mut url = format!("{out_scheme}://{authority}{path}");
    while url.ends_with('/') {
        url.pop();
    }
    Ok(url)
}

// ---------------------------------------------------------------------------
// Workspaces root + env list parsing (config.go:635–713).
// ---------------------------------------------------------------------------

/// `TaskWorkspacesRootEnv` (config.go:640).
pub(crate) const TASK_WORKSPACES_ROOT_ENV: &str = "CORDY_TASK_WORKSPACES_ROOT";

/// `os.UserHomeDir`.
fn home_dir() -> anyhow::Result<PathBuf> {
    #[cfg(unix)]
    let key = "HOME";
    #[cfg(windows)]
    let key = "USERPROFILE";
    std::env::var(key)
        .map(PathBuf::from)
        .map_err(|_| anyhow::anyhow!("${key} is not defined"))
}

/// `ResolveWorkspacesRoot` (config.go:649–670): override > CORDY_WORKSPACES_ROOT
/// > $HOME/cordy_workspaces[_<profile>], then absolutized.
pub(crate) fn resolve_workspaces_root(profile: &str, r#override: &str) -> anyhow::Result<String> {
    let mut root = std::env::var("CORDY_WORKSPACES_ROOT")
        .unwrap_or_default()
        .trim()
        .to_string();
    if !r#override.is_empty() {
        root = r#override.to_string();
    }
    if root.is_empty() {
        let home = home_dir().map_err(|e| {
            anyhow::anyhow!("resolve home directory: {e:#} (set CORDY_WORKSPACES_ROOT to override)")
        })?;
        root = if profile.is_empty() {
            home.join("cordy_workspaces")
        } else {
            home.join(format!("cordy_workspaces_{profile}"))
        }
        .to_string_lossy()
        .into_owned();
    }
    absolute(&root)
        .map(|p| p.to_string_lossy().into_owned())
        .context("resolve absolute workspaces root")
}

/// `ArtifactPatternsFromEnv` (config.go:676–678).
pub(crate) fn artifact_patterns_from_env() -> Vec<String> {
    patterns_from_env(
        "CORDY_GC_ARTIFACT_PATTERNS",
        &default_gc_artifact_patterns(),
    )
}

/// `patternsFromEnv` (config.go:684–701): comma-separated; separator-bearing
/// entries silently dropped — only directory basenames are ever matched.
pub(crate) fn patterns_from_env(name: &str, defaults: &[String]) -> Vec<String> {
    let raw = std::env::var(name).unwrap_or_default();
    let raw = raw.trim();
    if raw.is_empty() {
        return defaults.to_vec();
    }
    raw.split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty() && !p.contains('/') && !p.contains('\\'))
        .map(|p| p.to_string())
        .collect()
}

/// Minimal go-shellwords tokenizer: whitespace-separated words with single /
/// double quoting and backslash escapes. No backticks or env expansion,
/// matching go-shellwords' defaults.
fn shell_split(raw: &str) -> anyhow::Result<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has_word = false;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                if has_word {
                    out.push(std::mem::take(&mut cur));
                    has_word = false;
                }
            }
            '\'' => {
                has_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    cur.push(c);
                }
            }
            '"' => {
                has_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => {
                            if let Some(&n @ ('"' | '\\')) = chars.peek() {
                                cur.push(n);
                                chars.next();
                            } else {
                                cur.push('\\');
                            }
                        }
                        other => cur.push(other),
                    }
                }
            }
            '\\' => {
                has_word = true;
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            other => {
                has_word = true;
                cur.push(other);
            }
        }
    }
    if has_word {
        out.push(cur);
    }
    Ok(out)
}

/// `shellArgsFromEnv` (config.go:703–713).
fn shell_args_from_env(name: &str) -> anyhow::Result<Vec<String>> {
    let raw = std::env::var(name).unwrap_or_default();
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    shell_split(raw).map_err(|e| anyhow::anyhow!("invalid {name}: {e:#}"))
}

// ---------------------------------------------------------------------------
// Agent executable discovery (config.go:715–855).
// ---------------------------------------------------------------------------

/// `exec.LookPath` (unix semantics): bare names search PATH for an executable
/// regular file; path-bearing arguments are checked directly.
fn look_path(cmd: &str) -> anyhow::Result<String> {
    if cmd.contains('/') || cmd.contains('\\') {
        if is_executable_file(cmd) {
            return Ok(cmd.to_string());
        }
        anyhow::bail!("exec: {}: no such file or executable", cmd);
    }
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let candidate = Path::new(dir).join(cmd);
        if is_executable_file(&candidate.to_string_lossy()) {
            return Ok(candidate.to_string_lossy().into_owned());
        }
    }
    anyhow::bail!("exec: {:?}: executable file not found in $PATH", cmd)
}

/// `resolveAgentExecutablePath` (config.go:727–741): pin bare command names to
/// the startup-resolved path; keep name-dispatching shim entrypoints; skip a
/// ~/.cordy/hooks directory shadowing a real binary.
pub(crate) fn resolve_agent_executable_path(cmd: &str) -> anyhow::Result<String> {
    let resolved = look_path(cmd)?;
    if cmd.contains('/') || cmd.contains('\\') {
        return Ok(crate::canonical_path::canonical_configured_executable_path(
            &resolved,
        ));
    }
    if is_in_cordy_hooks_dir(&resolved) {
        if let Ok(unshadowed) = look_path_excluding_cordy_hooks(cmd) {
            return Ok(unshadowed);
        }
    }
    Ok(crate::canonical_path::discovered_executable_path(&resolved))
}

/// `agentExecutablePresent` (config.go:748–754): whether the pinned path still
/// resolves to a runnable executable (MUL-4486).
pub(crate) fn agent_executable_present(path: &str) -> bool {
    !path.is_empty() && look_path(path).is_ok()
}

/// Test-only injection point mirroring Go's `resolveAgentsViaLoginShell` var.
#[cfg(test)]
type ShellEnvResolver = fn(&[String]) -> HashMap<String, String>;

#[cfg(test)]
static LOGIN_SHELL_RESOLVER: std::sync::RwLock<Option<ShellEnvResolver>> =
    std::sync::RwLock::new(None);

#[cfg(test)]
pub(crate) fn set_login_shell_resolver_for_tests(f: Option<ShellEnvResolver>) {
    *LOGIN_SHELL_RESOLVER.write().unwrap() = f;
}

/// `reresolveAgentCommand` (config.go:764–782): re-run startup resolution on
/// the miss path only, so the login-shell cost is paid rarely.
pub(crate) fn reresolve_agent_command(cmd: &str) -> Option<String> {
    if cmd.is_empty() {
        return None;
    }
    if let Ok(path) = resolve_agent_executable_path(cmd) {
        return Some(path);
    }
    // Absolute/relative overrides skip the shell fallback: an operator-pinned
    // CORDY_*_PATH that vanished stays a hard miss.
    if !cmd.contains('/') && !cmd.contains('\\') {
        let resolver = || {
            #[cfg(test)]
            {
                if let Some(f) = *LOGIN_SHELL_RESOLVER.read().unwrap() {
                    return f(&[cmd.to_string()]);
                }
            }
            resolve_agents_via_login_shell(&[cmd.to_string()])
        };
        if let Some(path) = resolver().get(cmd) {
            return Some(path.clone());
        }
    }
    None
}

/// `lookPathExcludingCordyHooks` (config.go:784–798).
pub(crate) fn look_path_excluding_cordy_hooks(cmd: &str) -> anyhow::Result<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        if is_cordy_hooks_dir(dir) {
            continue;
        }
        let candidate = Path::new(dir).join(cmd);
        let candidate = candidate.to_string_lossy().into_owned();
        if is_executable_file(&candidate) {
            return Ok(crate::canonical_path::discovered_executable_path(
                &candidate,
            ));
        }
    }
    anyhow::bail!("exec: {:?}: executable file not found in $PATH", cmd)
}

/// `isInCordyHooksDir` (config.go:800–805).
pub(crate) fn is_in_cordy_hooks_dir(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let dir = Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    is_cordy_hooks_dir(&dir)
}

/// `isCordyHooksDir` (config.go:807–813).
pub(crate) fn is_cordy_hooks_dir(dir: &str) -> bool {
    let Ok(home) = home_dir() else { return false };
    if home.as_os_str().is_empty() {
        return false;
    }
    same_path_dir(dir, &home.join(".cordy").join("hooks").to_string_lossy())
}

// ---------------------------------------------------------------------------
// Login-shell fallback (config.go:857–1094).
// ---------------------------------------------------------------------------

/// `defaultAgentCommandNames` (config.go:866–869). S9-integration: append
/// `agent.BuiltinRuntimeCommands()` when the descriptor registry lands.
pub(crate) fn default_agent_command_names() -> Vec<String> {
    [
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
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// `codexDesktopAppBundlePaths` (config.go:877–889): macOS app-bundle
/// candidates for the bundled Codex CLI; ChatGPT.app before legacy Codex.app.
pub(crate) fn codex_desktop_app_bundle_paths() -> Vec<String> {
    let mut paths = vec![
        "/Applications/ChatGPT.app/Contents/Resources/codex".to_string(),
        "/Applications/Codex.app/Contents/Resources/codex".to_string(),
    ];
    if let Ok(home) = home_dir() {
        for app in ["ChatGPT.app", "Codex.app"] {
            paths.push(
                home.join("Applications")
                    .join(app)
                    .join("Contents")
                    .join("Resources")
                    .join("codex")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    paths
}

/// `loginShellResolveTimeout` (config.go:896). Go's companion
/// `loginShellResolveWaitDelay` (config.go:908) has no equivalent here — see
/// the resolver's deviation note.
const LOGIN_SHELL_RESOLVE_TIMEOUT: Duration = Duration::from_secs(3);

/// `supportedLoginShells` (config.go:914–920): POSIX-compatible shells only.
fn is_supported_login_shell(shell_base: &str) -> bool {
    matches!(shell_base, "bash" | "zsh" | "sh" | "dash" | "ksh")
}

/// `isSafeAgentName` (config.go:1079–1094): bare command names safe to inline
/// into the resolver script.
pub(crate) fn is_safe_agent_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_lowercase()
                || c.is_ascii_uppercase()
                || c.is_ascii_digit()
                || c == '-'
                || c == '_'
                || c == '.'
        })
}

/// `buildLoginShellResolveScript` (config.go:1043–1072): unalias/unset -f,
/// `command -v`, absolute-path guard, `pwd -P` canonicalization while the
/// shell is alive, hooks-dir bypass, then `name\tpath` per line.
pub(crate) fn build_login_shell_resolve_script(names: &[String]) -> String {
    let mut b = String::new();
    b.push_str("for n in");
    for n in names {
        b.push(' ');
        b.push_str(n);
    }
    b.push_str("; do\n");
    b.push_str("  unalias \"$n\" 2>/dev/null\n");
    b.push_str("  unset -f \"$n\" 2>/dev/null\n");
    b.push_str("  p=$(command -v \"$n\" 2>/dev/null) || continue\n");
    b.push_str("  [ -n \"$p\" ] || continue\n");
    b.push_str("  case \"$p\" in /*) ;; *) continue ;; esac\n");
    b.push_str("  d=$(dirname \"$p\") && f=$(basename \"$p\") && c=$(cd \"$d\" 2>/dev/null && pwd -P) || continue\n");
    b.push_str("  hc=\"\"\n");
    b.push_str("  if [ -n \"${HOME:-}\" ]; then hd=\"$HOME/.cordy/hooks\"; hc=$(cd \"$hd\" 2>/dev/null && pwd -P) || hc=\"\"; fi\n");
    b.push_str("  if [ -n \"$hc\" ] && [ \"$c\" = \"$hc\" ]; then\n");
    b.push_str("    oldIFS=$IFS; IFS=:\n");
    b.push_str("    for d2 in $PATH; do\n");
    b.push_str("      [ -n \"$d2\" ] || d2=.\n");
    b.push_str("      c2=$(cd \"$d2\" 2>/dev/null && pwd -P) || continue\n");
    b.push_str("      [ \"$c2\" = \"$hc\" ] && continue\n");
    b.push_str(
        "      if [ -f \"$c2/$n\" ] && [ -x \"$c2/$n\" ]; then c=\"$c2\"; f=\"$n\"; break; fi\n",
    );
    b.push_str("    done\n");
    b.push_str("    IFS=$oldIFS\n");
    b.push_str("  fi\n");
    b.push_str("  printf '%s\\t%s\\n' \"$n\" \"$c/$f\"\n");
    b.push_str("done\n");
    b
}

/// `resolveAgentsViaLoginShell` (config.go:956–1010): ask `$SHELL -ilc` for an
/// absolute, invocation-safe path per name. Empty map when the shell is
/// unavailable / unsupported / times out / yields nothing usable.
///
/// Deviation: Go's Cmd.WaitDelay force-closes pipes after the timeout; here a
/// timed-out child is killed and its reader thread abandoned, bounding the
/// caller's wait at timeout + kill rather than timeout + waitDelay.
fn resolve_agents_via_login_shell(names: &[String]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if names.is_empty() {
        return out;
    }
    let shell = std::env::var("SHELL").unwrap_or_default();
    let shell = shell.trim();
    if shell.is_empty() {
        return out;
    }
    let shell_base = Path::new(shell)
        .file_name()
        .map(|b| b.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !is_supported_login_shell(&shell_base) {
        return out;
    }
    let safe: Vec<String> = names
        .iter()
        .filter(|n| is_safe_agent_name(n))
        .cloned()
        .collect();
    if safe.is_empty() {
        return out;
    }

    let script = build_login_shell_resolve_script(&safe);
    let mut child = match std::process::Command::new(shell)
        .arg("-ilc")
        .arg(&script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return out,
    };

    // Bounded wait: a reader thread drains stdout while the caller waits on
    // the channel; on expiry the shell is killed and the pipe abandoned.
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
        if !Path::new(path).is_absolute() || look_path(path).is_err() {
            continue;
        }
        out.insert(name.to_string(), path.to_string());
    }
    out
}

// ---------------------------------------------------------------------------
// OpenClaw config-file bridge (config.go:1096–1143).
// ---------------------------------------------------------------------------

/// Minimal mirror of `cli.OpenClawOverride`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(crate) struct OpenClawFileOverride {
    #[serde(default, rename = "binary_path")]
    pub binary_path: String,
    #[serde(default, rename = "state_dir")]
    pub state_dir: String,
    #[serde(default, rename = "cli_timeout")]
    pub cli_timeout: String,
}

/// Minimal mirror of `cli.CLIConfig` covering the fields LoadConfig reads.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(crate) struct CliFileConfig {
    #[serde(default, rename = "backends")]
    pub backends: Option<CliBackendOverrides>,
    #[serde(default, rename = "profile_command_overrides")]
    pub profile_command_overrides: HashMap<String, String>,
}

/// Minimal mirror of `cli.BackendOverrides`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub(crate) struct CliBackendOverrides {
    #[serde(default, rename = "openclaw")]
    pub open_claw: Option<OpenClawFileOverride>,
}

/// S9-integration seam: `cli.LoadCLIConfigForProfile`
/// (internal/cli/config.go:304–321). Returns None until the CLI crate port
/// lands — config-file overrides are then simply absent, which matches the
/// missing-file behavior in Go.
fn load_cli_config_for_profile(_profile: &str) -> anyhow::Result<Option<CliFileConfig>> {
    Ok(None)
}

/// `openclawOverrideFrom` (config.go:1100–1105): navigate the nullable chain.
fn openclaw_override_from(cfg: &CliFileConfig) -> Option<&OpenClawFileOverride> {
    cfg.backends.as_ref()?.open_claw.as_ref()
}

/// `execenv.OpenclawCLITimeoutEnv` (execenv/openclaw_config.go:76).
const OPENCLAW_CLI_TIMEOUT_ENV: &str = "CORDY_OPENCLAW_CLI_TIMEOUT";

/// `applyOpenclawOverride` (config.go:1124–1143): translate config-file fields
/// into process env vars. Env-set-by-user wins over config-set-by-file: we
/// only set when the var is not already present.
fn apply_openclaw_override(oc: Option<&OpenClawFileOverride>) {
    let Some(oc) = oc else { return };
    let set_if_absent = |key: &str, value: &str| {
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, value);
        }
    };
    if !oc.binary_path.is_empty() {
        set_if_absent("CORDY_OPENCLAW_PATH", &oc.binary_path);
    }
    if !oc.state_dir.is_empty() {
        set_if_absent("OPENCLAW_STATE_DIR", &oc.state_dir);
    }
    if !oc.cli_timeout.is_empty() {
        set_if_absent(OPENCLAW_CLI_TIMEOUT_ENV, &oc.cli_timeout);
    }
}

// ---------------------------------------------------------------------------
// LoadConfig (config.go:181–564).
// ---------------------------------------------------------------------------

/// S9-integration seam: `probeAgentCLIs` (agents_probe.go, lane B). Returns an
/// empty map until that port lands; `Overrides::allow_no_agents` gates the
/// startup error in tests and read-only probes.
fn probe_agent_clis() -> HashMap<String, AgentEntry> {
    HashMap::new()
}

/// `os.Hostname` with Go's "local-machine" fallback (config.go:271–274).
fn hostname_or_fallback() -> String {
    #[cfg(unix)]
    unsafe {
        let mut buf = [0u8; 256];
        if libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            let host = String::from_utf8_lossy(&buf[..end]).into_owned();
            if !host.trim().is_empty() {
                return host;
            }
        }
    }
    #[cfg(windows)]
    {
        if let Ok(host) = std::env::var("COMPUTERNAME") {
            if !host.trim().is_empty() {
                return host;
            }
        }
    }
    "local-machine".to_string()
}

/// `LoadConfig` (config.go:183–563): build daemon configuration from env vars,
/// CLI flag overrides, and (once the CLI port lands) the CLI config file.
pub(crate) fn load_config(overrides: Overrides) -> anyhow::Result<Config> {
    // Server URL: override > env > default.
    let raw_server_url = {
        let from_env = env_or_default("CORDY_SERVER_URL", DEFAULT_SERVER_URL);
        if overrides.server_url.is_empty() {
            from_env
        } else {
            overrides.server_url.clone()
        }
    };
    let server_base_url = normalize_server_base_url(&raw_server_url)?;

    // Backend overrides from the CLI config file (#3875): env wins over
    // config wins over default. Errors are non-fatal — a missing/malformed
    // config must not block startup.
    let mut profile_command_overrides: HashMap<String, String> = HashMap::new();
    match load_cli_config_for_profile(&overrides.profile) {
        Err(err) => {
            tracing::warn!(
                profile = %overrides.profile,
                error = %err,
                "could not load CLI config for backend overrides; proceeding without"
            );
        }
        Ok(None) => {}
        Ok(Some(cli_cfg)) => {
            apply_openclaw_override(openclaw_override_from(&cli_cfg));
            for (id, path) in &cli_cfg.profile_command_overrides {
                if id.is_empty() || path.trim().is_empty() {
                    continue;
                }
                profile_command_overrides.insert(id.clone(), path.clone());
            }
        }
    }

    // Discover installed agent CLIs (MUL-5439 re-runs this live).
    let agents = probe_agent_clis();
    if agents.is_empty() && !overrides.allow_no_agents {
        anyhow::bail!(
            "no agent CLI found: install claude, codebuddy, codex, copilot, opencode, deveco, openclaw, hermes, pi, omp, cursor-agent, kimi, reasonix, dsh, kiro-cli, agy, qodercli, qoderclicn, traecli, grok, qwen, qwenpaw, mcode, or dim and ensure it is on PATH"
        );
    }

    let claude_args = shell_args_from_env("CORDY_CLAUDE_ARGS")?;
    let codex_args = shell_args_from_env("CORDY_CODEX_ARGS")?;
    let codebuddy_args = shell_args_from_env("CORDY_CODEBUDDY_ARGS")?;
    let qwen_args = shell_args_from_env("CORDY_QWEN_ARGS")?;
    let qwenpaw_args = shell_args_from_env("CORDY_QWENPAW_ARGS")?;

    // Host info.
    let host = hostname_or_fallback();

    // Durations: override > env > default.
    let mut poll_interval = duration_from_env("CORDY_DAEMON_POLL_INTERVAL", DEFAULT_POLL_INTERVAL)?;
    if overrides.poll_interval > Duration::ZERO {
        poll_interval = overrides.poll_interval;
    }

    let mut heartbeat_interval = duration_from_env(
        "CORDY_DAEMON_HEARTBEAT_INTERVAL",
        DEFAULT_HEARTBEAT_INTERVAL,
    )?;
    if overrides.heartbeat_interval > Duration::ZERO {
        heartbeat_interval = overrides.heartbeat_interval;
    }

    let mut agent_timeout = duration_from_env("CORDY_AGENT_TIMEOUT", DEFAULT_AGENT_TIMEOUT)?;
    if let Some(explicit) = overrides.agent_timeout {
        agent_timeout = explicit;
    }

    let mut codex_semantic_inactivity_timeout = duration_from_env(
        "CORDY_CODEX_SEMANTIC_INACTIVITY_TIMEOUT",
        DEFAULT_CODEX_SEMANTIC_INACTIVITY_TIMEOUT,
    )?;
    if overrides.codex_semantic_inactivity_timeout > Duration::ZERO {
        codex_semantic_inactivity_timeout = overrides.codex_semantic_inactivity_timeout;
    }

    // 0 = unset: positive values are an explicit operator override
    // (GH #3262 / #5959).
    let codex_first_turn_no_progress_timeout =
        duration_from_env("CORDY_CODEX_FIRST_TURN_TIMEOUT", Duration::ZERO)?;
    // The first-turn override only raises the first-turn watchdog; when it is
    // >= the semantic timeout the effective first-item wait is truncated to
    // the semantic timeout (armed first), disabling the model-catalog startup
    // retry (GH #3291).
    if codex_first_turn_no_progress_timeout > Duration::ZERO {
        let effective_semantic_timeout = if codex_semantic_inactivity_timeout <= Duration::ZERO {
            DEFAULT_CODEX_SEMANTIC_INACTIVITY_TIMEOUT
        } else {
            codex_semantic_inactivity_timeout
        };
        if codex_first_turn_no_progress_timeout >= effective_semantic_timeout {
            tracing::warn!(
                first_turn_timeout = ?codex_first_turn_no_progress_timeout,
                semantic_inactivity_timeout = ?effective_semantic_timeout,
                "CORDY_CODEX_FIRST_TURN_TIMEOUT is greater than or equal to the semantic-inactivity timeout; the effective first-turn wait is truncated to the semantic timeout and the model-catalog startup retry is disabled. Because the semantic timer is armed first and equal durations do not deterministically favour the first-turn deadline, set CORDY_CODEX_SEMANTIC_INACTIVITY_TIMEOUT strictly above CORDY_CODEX_FIRST_TURN_TIMEOUT (with some margin) to preserve it."
            );
        }
    }

    let mut codex_handshake_timeout = duration_from_env(
        "CORDY_CODEX_HANDSHAKE_TIMEOUT",
        DEFAULT_CODEX_HANDSHAKE_TIMEOUT,
    )?;
    if codex_handshake_timeout <= Duration::ZERO {
        codex_handshake_timeout = DEFAULT_CODEX_HANDSHAKE_TIMEOUT;
    }
    if overrides.codex_handshake_timeout > Duration::ZERO {
        codex_handshake_timeout = overrides.codex_handshake_timeout;
    }

    // CORDY_AGENT_IDLE_WATCHDOG=0 disables; positive overrides the default.
    let agent_idle_watchdog =
        duration_from_env("CORDY_AGENT_IDLE_WATCHDOG", DEFAULT_AGENT_IDLE_WATCHDOG)?;
    // Zero removes the OpenCode-specific window (falls back to the global
    // watchdog); positives cannot extend the global bound.
    let open_code_idle_watchdog = duration_from_env(
        "CORDY_OPENCODE_IDLE_WATCHDOG",
        DEFAULT_OPEN_CODE_IDLE_WATCHDOG,
    )?;
    let agent_tool_watchdog =
        duration_from_env("CORDY_AGENT_TOOL_WATCHDOG", DEFAULT_AGENT_TOOL_WATCHDOG)?;

    let mut max_concurrent_tasks = int_from_env(
        "CORDY_DAEMON_MAX_CONCURRENT_TASKS",
        DEFAULT_MAX_CONCURRENT_TASKS,
    )?;
    if overrides.max_concurrent_tasks > 0 {
        max_concurrent_tasks = overrides.max_concurrent_tasks;
    }

    // Profile.
    let profile = overrides.profile.clone();

    // daemon_id resolution: override > env > persistent UUID on disk.
    let mut daemon_id = std::env::var("CORDY_DAEMON_ID")
        .unwrap_or_default()
        .trim()
        .to_string();
    if !overrides.daemon_id.is_empty() {
        daemon_id = overrides.daemon_id.clone();
    }
    if daemon_id.is_empty() {
        daemon_id = crate::identity::ensure_daemon_id(&profile).context("ensure daemon id")?;
    }

    // Historical ids for server-side runtime-row merges.
    let mut legacy_daemon_ids = crate::identity::legacy_daemon_ids(&host, &profile);
    // Pre-#1220 per-profile daemon.id files surface as merge candidates too.
    if let Ok(uuids) = crate::identity::legacy_daemon_uuids() {
        legacy_daemon_ids.extend(uuids);
    }
    let legacy_daemon_ids = crate::identity::filter_legacy_ids(legacy_daemon_ids, &daemon_id);

    let mut device_name = env_or_default("CORDY_DAEMON_DEVICE_NAME", &host);
    if !overrides.device_name.is_empty() {
        device_name = overrides.device_name.clone();
    }

    let mut runtime_name = env_or_default("CORDY_AGENT_RUNTIME_NAME", DEFAULT_RUNTIME_NAME);
    if !overrides.runtime_name.is_empty() {
        runtime_name = overrides.runtime_name.clone();
    }

    let workspaces_root = resolve_workspaces_root(&profile, &overrides.workspaces_root)?;

    // Health port: override > default.
    let health_port = if overrides.health_port > 0 {
        overrides.health_port
    } else {
        DEFAULT_HEALTH_PORT
    };

    // Keep env after task: env > default (false).
    let keep_env_raw = std::env::var("CORDY_KEEP_ENV_AFTER_TASK").unwrap_or_default();
    let keep_env_after_task = keep_env_raw == "true" || keep_env_raw == "1";

    // GC config: env > defaults.
    let gc_enabled = !matches!(
        std::env::var("CORDY_GC_ENABLED").as_deref(),
        Ok("false" | "0")
    );
    let gc_interval = duration_from_env("CORDY_GC_INTERVAL", DEFAULT_GC_INTERVAL)?;
    let gc_ttl = duration_from_env("CORDY_GC_TTL", DEFAULT_GC_TTL)?;
    let gc_completed_task_ttl = duration_from_env(
        "CORDY_GC_COMPLETED_TASK_TTL",
        default_gc_completed_task_ttl(&server_base_url),
    )?;
    let gc_orphan_ttl = duration_from_env("CORDY_GC_ORPHAN_TTL", DEFAULT_GC_ORPHAN_TTL)?;
    let gc_artifact_ttl = duration_from_env("CORDY_GC_ARTIFACT_TTL", DEFAULT_GC_ARTIFACT_TTL)?;
    let gc_codex_session_ttl =
        duration_from_env("CORDY_GC_CODEX_SESSION_TTL", DEFAULT_GC_CODEX_SESSION_TTL)?;
    let gc_hermes_memory_ttl =
        duration_from_env("CORDY_GC_HERMES_MEMORY_TTL", DEFAULT_GC_HERMES_MEMORY_TTL)?;
    let gc_hermes_session_ttl =
        duration_from_env("CORDY_GC_HERMES_SESSION_TTL", DEFAULT_GC_HERMES_SESSION_TTL)?;
    let gc_repo_ttl = duration_from_env("CORDY_GC_REPO_TTL", DEFAULT_GC_REPO_TTL)?;
    let gc_repo_maintenance_enabled = bool_from_env("CORDY_GC_REPO_MAINTENANCE_ENABLED", true);
    let gc_artifact_patterns = patterns_from_env(
        "CORDY_GC_ARTIFACT_PATTERNS",
        &default_gc_artifact_patterns(),
    );

    // Auto-update: opt-in on Cordy Cloud, opt-out on self-host (MUL-2381);
    // CORDY_DAEMON_AUTO_UPDATE flips either default.
    let mut auto_update_enabled = bool_from_env(
        "CORDY_DAEMON_AUTO_UPDATE",
        is_official_cloud_server(&server_base_url),
    );
    if overrides.disable_auto_update {
        auto_update_enabled = false;
    }
    let mut auto_update_check_interval = duration_from_env(
        "CORDY_DAEMON_AUTO_UPDATE_INTERVAL",
        DEFAULT_AUTO_UPDATE_CHECK_INTERVAL,
    )?;
    if overrides.auto_update_check_interval > Duration::ZERO {
        auto_update_check_interval = overrides.auto_update_check_interval;
    }

    // Auto-reload is deliberately NOT gated on autoUpdateEnabled: "don't pull
    // new versions" and "follow the binary I replaced myself" are different
    // concerns. Default on for every CLI-launched daemon.
    let mut auto_reload_enabled = bool_from_env("CORDY_DAEMON_AUTO_RELOAD", true);
    if overrides.disable_auto_reload {
        auto_reload_enabled = false;
    }

    Ok(Config {
        server_base_url,
        daemon_id,
        legacy_daemon_ids,
        device_name,
        runtime_name,
        // cli_version / launched_by are filled by the launch wiring, not by
        // LoadConfig (same as Go).
        cli_version: String::new(),
        launched_by: String::new(),
        profile,
        agents,
        workspaces_root,
        keep_env_after_task,
        health_port,
        max_concurrent_tasks,
        gc_enabled,
        gc_interval,
        gc_ttl,
        gc_completed_task_ttl,
        gc_orphan_ttl,
        gc_artifact_ttl,
        gc_artifact_patterns,
        gc_repo_ttl,
        gc_repo_maintenance_enabled,
        gc_codex_session_ttl,
        gc_hermes_memory_ttl,
        gc_hermes_session_ttl,
        auto_update_enabled,
        auto_update_check_interval,
        auto_reload_enabled,
        poll_interval,
        heartbeat_interval,
        agent_timeout,
        codex_semantic_inactivity_timeout,
        codex_first_turn_no_progress_timeout,
        codex_handshake_timeout,
        open_code_idle_watchdog,
        agent_idle_watchdog,
        agent_tool_watchdog,
        claude_args,
        codex_args,
        codebuddy_args,
        qwen_args,
        qwenpaw_args,
        profile_command_overrides,
    })
}

// ---------------------------------------------------------------------------
// Tests (config_test.go pure-logic cases).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes env-mutating tests (process-global env).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// t.Setenv-style batch helper: sets/removes vars under a lock with a
    /// fresh HOME, restoring everything afterwards.
    fn with_env<F: FnOnce()>(vars: &[(&str, Option<&str>)], f: F) {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut saved: Vec<(String, Option<String>)> =
            vec![("HOME".into(), std::env::var("HOME").ok())];
        std::env::set_var("HOME", home.path());
        for (k, v) in vars {
            saved.push((k.to_string(), std::env::var(k).ok()));
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        f();
        for (k, prev) in saved {
            match prev {
                Some(p) => std::env::set_var(&k, p),
                None => std::env::remove_var(&k),
            }
        }
    }

    fn self_host_overrides() -> Overrides {
        Overrides {
            server_url: "http://localhost:8080".into(),
            workspaces_root: "/tmp/cordy-test-ws".into(),
            allow_no_agents: true,
            ..Default::default()
        }
    }

    /// TestPatternsFromEnv_DefaultsWhenUnset (config_test.go:95–108).
    #[test]
    fn patterns_from_env_defaults_when_unset() {
        let defaults = vec!["node_modules".into(), ".next".into(), ".turbo".into()];
        let got = patterns_from_env("CORDY_GC_ARTIFACT_PATTERNS", &defaults);
        assert_eq!(got, defaults);
        // Callers get a copy, not an alias.
        let mut got = got;
        got[0] = "mutated".into();
        assert_eq!(defaults[0], "node_modules");
    }

    /// TestDefaultGCIntervalIsTwoHours (config_test.go:109–113).
    #[test]
    fn default_gc_interval_is_two_hours() {
        assert_eq!(DEFAULT_GC_INTERVAL, Duration::from_secs(2 * 3600));
    }

    /// TestDefaultGCCompletedTaskTTLOnlyBoundsOfficialCloudHost
    /// (config_test.go:190–214).
    #[test]
    fn default_gc_completed_task_ttl_only_bounds_official_cloud_host() {
        let cases = [
            (
                "official cloud",
                "https://api.cordy.ai",
                DEFAULT_GC_COMPLETED_TASK_TTL_CLOUD,
            ),
            (
                "official cloud with port and path",
                "https://API.Cordy.AI:443/api",
                DEFAULT_GC_COMPLETED_TASK_TTL_CLOUD,
            ),
            (
                "staging",
                "https://api-staging.cordy.ai",
                DEFAULT_GC_COMPLETED_TASK_TTL_SELF_HOST,
            ),
            (
                "self-host",
                "https://cordy.example.com",
                DEFAULT_GC_COMPLETED_TASK_TTL_SELF_HOST,
            ),
            (
                "localhost",
                "http://localhost:8080",
                DEFAULT_GC_COMPLETED_TASK_TTL_SELF_HOST,
            ),
            (
                "unparseable",
                "://nope",
                DEFAULT_GC_COMPLETED_TASK_TTL_SELF_HOST,
            ),
        ];
        for (name, url, want) in cases {
            assert_eq!(default_gc_completed_task_ttl(url), want, "case {name:?}");
        }
    }

    /// TestIsOfficialCloudServer table (config_test.go:390–425).
    #[test]
    fn is_official_cloud_server_table() {
        let cases = [
            ("canonical cloud https", "https://api.cordy.ai", true),
            (
                "canonical cloud trailing slash",
                "https://api.cordy.ai/",
                true,
            ),
            (
                "canonical cloud case-insensitive",
                "https://API.Cordy.AI",
                true,
            ),
            ("cloud over plain http", "http://api.cordy.ai", true),
            ("localhost is self-host", "http://localhost:8080", false),
            ("loopback ip is self-host", "http://127.0.0.1:8080", false),
            ("lan ip is self-host", "http://192.168.0.28:8080", false),
            (
                "third-party host is self-host",
                "https://cordy.example.com",
                false,
            ),
            (
                "cordy.ai apex is not the api host",
                "https://cordy.ai",
                false,
            ),
            (
                "staging subdomain is self-host",
                "https://staging.cordy.ai",
                false,
            ),
            (
                "preview subdomain is self-host",
                "https://api-preview.cordy.ai",
                false,
            ),
            ("empty string is self-host", "", false),
            ("garbage string is self-host", "::not a url::", false),
        ];
        for (name, url, want) in cases {
            assert_eq!(is_official_cloud_server(url), want, "case {name:?}");
        }
    }

    /// TestPatternsFromEnv_DropsSeparatorBearingEntries
    /// (config_test.go:228–235).
    #[test]
    fn patterns_from_env_drops_separator_bearing_entries() {
        with_env(
            &[(
                "CORDY_GC_ARTIFACT_PATTERNS",
                Some("ok, foo/bar , ../x, also-ok"),
            )],
            || {
                let got = patterns_from_env(
                    "CORDY_GC_ARTIFACT_PATTERNS",
                    &default_gc_artifact_patterns(),
                );
                assert_eq!(got, vec!["ok".to_string(), "also-ok".to_string()]);
            },
        );
    }

    /// TestIsSafeAgentName (config_test.go:237–260).
    #[test]
    fn is_safe_agent_name_table() {
        let cases = [
            ("claude", true),
            ("cursor-agent", true),
            ("kiro_cli", true),
            ("v1.2", true),
            ("Claude2", true),
            ("", false),
            ("a b", false),
            ("a/b", false),
            ("a;b", false),
            ("a$b", false),
            ("a`b", false),
            ("a'b", false),
            ("a\"b", false),
        ];
        for (input, want) in cases {
            assert_eq!(is_safe_agent_name(input), want, "input {input:?}");
        }
    }

    /// TestBuildLoginShellResolveScript_ShapeAndContent
    /// (config_test.go:262–303).
    #[test]
    fn build_login_shell_resolve_script_shape_and_content() {
        let got = build_login_shell_resolve_script(&["claude".into(), "cursor-agent".into()]);
        assert!(got.contains("for n in claude cursor-agent;"));
        let idx_unalias = got
            .find("unalias \"$n\" 2>/dev/null")
            .expect("unalias step");
        let idx_unset_fn = got
            .find("unset -f \"$n\" 2>/dev/null")
            .expect("unset -f step");
        let idx_lookup = got.find("command -v \"$n\"").expect("command -v step");
        assert!(
            idx_unalias < idx_lookup && idx_unset_fn < idx_lookup,
            "unalias/unset -f must precede command -v (#2512)"
        );
        assert!(got.contains("pwd -P"), "missing pwd -P canonicalisation");
        assert!(
            got.contains(r"printf '%s\t%s\n'"),
            "missing tab-separated printf"
        );
    }

    // ---- OpenClaw config-file bridge -------------------------------------

    /// TestApplyOpenclawOverride family (config_test.go:1319–1412).
    #[test]
    fn apply_openclaw_override_env_precedence() {
        // Nil override leaves pre-set values untouched.
        with_env(
            &[
                ("CORDY_OPENCLAW_PATH", Some("/before/openclaw")),
                ("OPENCLAW_STATE_DIR", Some("/before/state")),
            ],
            || {
                apply_openclaw_override(None);
                assert_eq!(
                    std::env::var("CORDY_OPENCLAW_PATH").unwrap(),
                    "/before/openclaw"
                );
                assert_eq!(
                    std::env::var("OPENCLAW_STATE_DIR").unwrap(),
                    "/before/state"
                );

                // Env wins over config (#3875 back-compat contract).
                apply_openclaw_override(Some(&OpenClawFileOverride {
                    binary_path: "/from/config/openclaw".into(),
                    state_dir: "/from/config/state".into(),
                    cli_timeout: String::new(),
                }));
                assert_eq!(
                    std::env::var("CORDY_OPENCLAW_PATH").unwrap(),
                    "/before/openclaw"
                );
                assert_eq!(
                    std::env::var("OPENCLAW_STATE_DIR").unwrap(),
                    "/before/state"
                );
            },
        );

        // Unset env + both fields configured → both set from config.
        with_env(
            &[("CORDY_OPENCLAW_PATH", None), ("OPENCLAW_STATE_DIR", None)],
            || {
                apply_openclaw_override(Some(&OpenClawFileOverride {
                    binary_path: "/from/config/openclaw".into(),
                    state_dir: "/from/config/state".into(),
                    cli_timeout: String::new(),
                }));
                assert_eq!(
                    std::env::var("CORDY_OPENCLAW_PATH").unwrap(),
                    "/from/config/openclaw"
                );
                assert_eq!(
                    std::env::var("OPENCLAW_STATE_DIR").unwrap(),
                    "/from/config/state"
                );
            },
        );

        // Partial fields: only configured fields are set — never Setenv("").
        with_env(
            &[("CORDY_OPENCLAW_PATH", None), ("OPENCLAW_STATE_DIR", None)],
            || {
                apply_openclaw_override(Some(&OpenClawFileOverride {
                    binary_path: String::new(),
                    state_dir: "/from/config/state".into(),
                    cli_timeout: String::new(),
                }));
                assert!(
                    std::env::var_os("CORDY_OPENCLAW_PATH").is_none(),
                    "CORDY_OPENCLAW_PATH must remain unset when binary_path is empty"
                );
                assert_eq!(
                    std::env::var("OPENCLAW_STATE_DIR").unwrap(),
                    "/from/config/state"
                );
            },
        );
    }

    /// TestOpenclawOverrideFrom_NavigationCases (config_test.go:1414–1431).
    #[test]
    fn openclaw_override_from_navigation_cases() {
        assert!(openclaw_override_from(&CliFileConfig::default()).is_none());
        assert!(openclaw_override_from(&CliFileConfig {
            backends: Some(CliBackendOverrides::default()),
            profile_command_overrides: HashMap::new(),
        })
        .is_none());
        let cfg = CliFileConfig {
            backends: Some(CliBackendOverrides {
                open_claw: Some(OpenClawFileOverride {
                    state_dir: "/x".into(),
                    ..Default::default()
                }),
            }),
            profile_command_overrides: HashMap::new(),
        };
        assert_eq!(openclaw_override_from(&cfg).unwrap().state_dir, "/x");
    }

    // ---- LoadConfig env resolution ---------------------------------------

    /// TestLoadConfig_CompletedTaskTTLDefaultsDisabledOnSelfHostAndReadsEnv
    /// (config_test.go:118–149): localhost → self-host branch → retention
    /// unbounded until an operator opts in.
    #[test]
    fn load_config_completed_task_ttl_self_host_default_and_env() {
        with_env(
            &[
                ("CORDY_GC_COMPLETED_TASK_TTL", None),
                (
                    "CORDY_DAEMON_ID",
                    Some("11111111-1111-1111-1111-111111111111"),
                ),
            ],
            || {
                let cfg = load_config(self_host_overrides()).unwrap();
                assert_eq!(
                    cfg.gc_completed_task_ttl,
                    DEFAULT_GC_COMPLETED_TASK_TTL_SELF_HOST
                );

                std::env::set_var("CORDY_GC_COMPLETED_TASK_TTL", "48h");
                let cfg = load_config(self_host_overrides()).unwrap();
                assert_eq!(cfg.gc_completed_task_ttl, Duration::from_secs(48 * 3600));
            },
        );
    }

    /// TestLoadConfig_CompletedTaskTTLDefaultsBoundedOnOfficialCloud
    /// (config_test.go:151–188): official cloud bounds retention by default.
    #[test]
    fn load_config_completed_task_ttl_bounded_on_official_cloud() {
        with_env(
            &[(
                "CORDY_DAEMON_ID",
                Some("11111111-1111-1111-1111-111111111111"),
            )],
            || {
                let overrides = Overrides {
                    server_url: "https://api.cordy.ai".into(),
                    workspaces_root: "/tmp/cordy-test-ws".into(),
                    allow_no_agents: true,
                    ..Default::default()
                };
                let cfg = load_config(overrides).unwrap();
                assert_eq!(
                    cfg.gc_completed_task_ttl,
                    DEFAULT_GC_COMPLETED_TASK_TTL_CLOUD
                );
            },
        );
    }

    /// TestRepoMaintenanceKillSwitchDefaultsOnAndCanDisable
    /// (config_test.go:216–226).
    #[test]
    fn repo_maintenance_kill_switch_defaults_on_and_can_disable() {
        with_env(
            &[(
                "CORDY_DAEMON_ID",
                Some("11111111-1111-1111-1111-111111111111"),
            )],
            || {
                let cfg = load_config(self_host_overrides()).unwrap();
                assert!(cfg.gc_repo_maintenance_enabled);

                std::env::set_var("CORDY_GC_REPO_MAINTENANCE_ENABLED", "false");
                let cfg = load_config(self_host_overrides()).unwrap();
                assert!(!cfg.gc_repo_maintenance_enabled);
            },
        );
    }

    /// TestLoadConfig_CodexHandshakeTimeout (config_test.go:617–673):
    /// default > env > zero-env-resets-to-default > override.
    #[test]
    fn load_config_codex_handshake_timeout() {
        with_env(
            &[
                ("CORDY_CODEX_HANDSHAKE_TIMEOUT", None),
                (
                    "CORDY_DAEMON_ID",
                    Some("11111111-1111-1111-1111-111111111111"),
                ),
            ],
            || {
                let cfg = load_config(self_host_overrides()).unwrap();
                assert_eq!(cfg.codex_handshake_timeout, DEFAULT_CODEX_HANDSHAKE_TIMEOUT);

                std::env::set_var("CORDY_CODEX_HANDSHAKE_TIMEOUT", "47s");
                let cfg = load_config(self_host_overrides()).unwrap();
                assert_eq!(cfg.codex_handshake_timeout, Duration::from_secs(47));

                std::env::set_var("CORDY_CODEX_HANDSHAKE_TIMEOUT", "0");
                let cfg = load_config(self_host_overrides()).unwrap();
                assert_eq!(cfg.codex_handshake_timeout, DEFAULT_CODEX_HANDSHAKE_TIMEOUT);

                let overrides = Overrides {
                    codex_handshake_timeout: Duration::from_secs(12),
                    ..self_host_overrides()
                };
                let cfg = load_config(overrides).unwrap();
                assert_eq!(cfg.codex_handshake_timeout, Duration::from_secs(12));
            },
        );
    }

    /// TestLoadConfig_CodexFirstTurnNoProgressTimeout
    /// (config_test.go:675–721): unset and explicit "0" both mean unset;
    /// positive honored verbatim; no CLI parity.
    #[test]
    fn load_config_codex_first_turn_no_progress_timeout() {
        with_env(
            &[
                ("CORDY_CODEX_FIRST_TURN_TIMEOUT", None),
                (
                    "CORDY_DAEMON_ID",
                    Some("11111111-1111-1111-1111-111111111111"),
                ),
            ],
            || {
                let cfg = load_config(self_host_overrides()).unwrap();
                assert_eq!(cfg.codex_first_turn_no_progress_timeout, Duration::ZERO);

                std::env::set_var("CORDY_CODEX_FIRST_TURN_TIMEOUT", "0");
                let cfg = load_config(self_host_overrides()).unwrap();
                assert_eq!(cfg.codex_first_turn_no_progress_timeout, Duration::ZERO);

                std::env::set_var("CORDY_CODEX_FIRST_TURN_TIMEOUT", "90s");
                let cfg = load_config(self_host_overrides()).unwrap();
                assert_eq!(
                    cfg.codex_first_turn_no_progress_timeout,
                    Duration::from_secs(90)
                );
            },
        );
    }

    /// TestLoadConfig_OpenCodeIdleWatchdog (config_test.go:763–808).
    #[test]
    fn load_config_opencode_idle_watchdog() {
        with_env(
            &[
                ("CORDY_OPENCODE_IDLE_WATCHDOG", None),
                (
                    "CORDY_DAEMON_ID",
                    Some("11111111-1111-1111-1111-111111111111"),
                ),
            ],
            || {
                let cfg = load_config(self_host_overrides()).unwrap();
                assert_eq!(cfg.open_code_idle_watchdog, DEFAULT_OPEN_CODE_IDLE_WATCHDOG);

                std::env::set_var("CORDY_OPENCODE_IDLE_WATCHDOG", "5m");
                let cfg = load_config(self_host_overrides()).unwrap();
                assert_eq!(cfg.open_code_idle_watchdog, Duration::from_secs(300));

                std::env::set_var("CORDY_OPENCODE_IDLE_WATCHDOG", "0");
                let cfg = load_config(self_host_overrides()).unwrap();
                assert_eq!(cfg.open_code_idle_watchdog, Duration::ZERO);
            },
        );
    }

    /// Auto-update defaults/env trio (config_test.go:603–615, 810–859).
    #[test]
    fn load_config_auto_update_defaults_and_env() {
        with_env(
            &[
                ("CORDY_DAEMON_AUTO_UPDATE", None),
                (
                    "CORDY_DAEMON_ID",
                    Some("11111111-1111-1111-1111-111111111111"),
                ),
            ],
            || {
                // Self-host (localhost): off by default.
                let cfg = load_config(self_host_overrides()).unwrap();
                assert!(!cfg.auto_update_enabled, "self-host must default off");

                // Official cloud: on by default.
                let overrides = Overrides {
                    server_url: "https://api.cordy.ai".into(),
                    workspaces_root: "/tmp/cordy-test-ws".into(),
                    allow_no_agents: true,
                    ..Default::default()
                };
                let cfg = load_config(overrides).unwrap();
                assert!(cfg.auto_update_enabled, "cloud must default on");

                // Env forces ON for self-host.
                std::env::set_var("CORDY_DAEMON_AUTO_UPDATE", "true");
                let cfg = load_config(self_host_overrides()).unwrap();
                assert!(cfg.auto_update_enabled);

                // Env forces OFF for cloud.
                std::env::set_var("CORDY_DAEMON_AUTO_UPDATE", "false");
                let overrides = Overrides {
                    server_url: "https://api.cordy.ai".into(),
                    workspaces_root: "/tmp/cordy-test-ws".into(),
                    allow_no_agents: true,
                    ..Default::default()
                };
                let cfg = load_config(overrides).unwrap();
                assert!(!cfg.auto_update_enabled);
            },
        );
    }

    /// Auto-reload trio (config_test.go:883–959): defaults on even for
    /// self-host, not gated on the auto-update env, disable switch works.
    #[test]
    fn load_config_auto_reload_defaults_and_switches() {
        with_env(
            &[
                ("CORDY_DAEMON_AUTO_RELOAD", None),
                ("CORDY_DAEMON_AUTO_UPDATE", None),
                (
                    "CORDY_DAEMON_ID",
                    Some("11111111-1111-1111-1111-111111111111"),
                ),
            ],
            || {
                let cfg = load_config(self_host_overrides()).unwrap();
                assert!(
                    cfg.auto_reload_enabled,
                    "auto-reload defaults on even for self-host"
                );

                std::env::set_var("CORDY_DAEMON_AUTO_UPDATE", "false");
                let cfg = load_config(self_host_overrides()).unwrap();
                assert!(cfg.auto_reload_enabled, "not gated on auto-update env");

                std::env::set_var("CORDY_DAEMON_AUTO_RELOAD", "false");
                let cfg = load_config(self_host_overrides()).unwrap();
                assert!(!cfg.auto_reload_enabled);

                let overrides = Overrides {
                    disable_auto_reload: true,
                    ..self_host_overrides()
                };
                let cfg = load_config(overrides).unwrap();
                assert!(!cfg.auto_reload_enabled);
            },
        );
    }

    /// to_gc_config projection stays field-aligned with gc.rs's GcConfig.
    #[test]
    fn gc_config_projection_round_trips() {
        let cfg = Config {
            profile: "staging".into(),
            workspaces_root: "/tmp/ws".into(),
            gc_enabled: true,
            gc_interval: Duration::from_secs(7200),
            gc_ttl: Duration::from_secs(86400),
            gc_completed_task_ttl: Duration::from_secs(1),
            gc_orphan_ttl: Duration::from_secs(2),
            gc_artifact_ttl: Duration::from_secs(3),
            gc_codex_session_ttl: Duration::from_secs(4),
            gc_hermes_memory_ttl: Duration::from_secs(5),
            gc_hermes_session_ttl: Duration::from_secs(6),
            gc_repo_ttl: Duration::from_secs(7),
            gc_repo_maintenance_enabled: false,
            gc_artifact_patterns: vec!["node_modules".into()],
            ..Default::default()
        };
        let gc = cfg.to_gc_config();
        assert_eq!(gc.profile, "staging");
        assert_eq!(gc.workspaces_root, PathBuf::from("/tmp/ws"));
        assert_eq!(gc.gc_interval, Duration::from_secs(7200));
        assert_eq!(gc.gc_ttl, Duration::from_secs(86400));
        assert_eq!(gc.gc_orphan_ttl, Duration::from_secs(2));
        assert_eq!(gc.gc_artifact_ttl, Duration::from_secs(3));
        assert_eq!(gc.gc_codex_session_ttl, Duration::from_secs(4));
        assert_eq!(gc.gc_hermes_memory_ttl, Duration::from_secs(5));
        assert_eq!(gc.gc_hermes_session_ttl, Duration::from_secs(6));
        assert_eq!(gc.gc_repo_ttl, Duration::from_secs(7));
        assert!(!gc.gc_repo_maintenance_enabled);
        assert_eq!(gc.gc_artifact_patterns, vec!["node_modules".to_string()]);
    }

    /// shell_split covers go-shellwords' default tokenization surface.
    #[test]
    fn shell_split_quotes_and_escapes() {
        assert_eq!(
            shell_split("--model claude-3").unwrap(),
            vec!["--model", "claude-3"]
        );
        assert_eq!(shell_split("'a b' c").unwrap(), vec!["a b", "c"]);
        assert_eq!(shell_split("\"a b\"c").unwrap(), vec!["a bc"]);
        assert_eq!(shell_split("a\\ b").unwrap(), vec!["a b"]);
        assert_eq!(shell_split("").unwrap(), Vec::<String>::new());
    }
}
