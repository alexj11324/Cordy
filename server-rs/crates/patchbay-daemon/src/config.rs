//! Daemon configuration and executable-path resolution.

use std::path::{Path, PathBuf};

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
// Defaults (config.go:21–88)
// ---------------------------------------------------------------------------

use std::collections::BTreeMap;
use std::time::Duration;

/// `DefaultServerURL`.
pub const DEFAULT_SERVER_URL: &str = "ws://localhost:8080/ws";
/// `DefaultPollInterval`.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(30);
/// `DefaultHeartbeatInterval`.
pub const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// `DefaultAgentTimeout`: optional absolute wall-clock cap on a single agent
/// run. 0 = no cap (PB-3064); set PATCHBAY_AGENT_TIMEOUT for a hard ceiling.
pub const DEFAULT_AGENT_TIMEOUT: Duration = Duration::ZERO;
/// `DefaultCodexSemanticInactivityTimeout`.
pub const DEFAULT_CODEX_SEMANTIC_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(600);
/// `DefaultCodexHandshakeTimeout`.
pub const DEFAULT_CODEX_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// `DefaultOpenCodeIdleWatchdog`: OpenCode streams incrementally, so its
/// no-message budget is shorter than the generic idle watchdog.
pub const DEFAULT_OPENCODE_IDLE_WATCHDOG: Duration = Duration::from_secs(600);
/// `DefaultAgentIdleWatchdog` (PB-2300): per-task safety net for runs whose
/// backend went fully silent with an empty queue; 30 min keeps the net for
/// truly stuck runs while leaving headroom for long writes.
pub const DEFAULT_AGENT_IDLE_WATCHDOG: Duration = Duration::from_secs(1800);
/// `DefaultAgentToolWatchdog` (PB-3064): bounds a single in-flight tool call
/// now that there is no wall-clock cap. 0 disables.
pub const DEFAULT_AGENT_TOOL_WATCHDOG: Duration = Duration::from_secs(7200);
/// `DefaultRuntimeName`.
pub const DEFAULT_RUNTIME_NAME: &str = "Local Agent";
/// `DefaultWorkspaceBootstrapSyncInterval`.
pub const DEFAULT_WORKSPACE_BOOTSTRAP_SYNC_INTERVAL: Duration = Duration::from_secs(30);
/// `DefaultWorkspaceLegacySyncInterval`.
pub const DEFAULT_WORKSPACE_LEGACY_SYNC_INTERVAL: Duration = Duration::from_secs(300);
/// `DefaultWorkspaceSyncInterval`.
pub const DEFAULT_WORKSPACE_SYNC_INTERVAL: Duration = Duration::from_secs(1800);
/// `DefaultWorkspaceSyncMaxBackoff`.
pub const DEFAULT_WORKSPACE_SYNC_MAX_BACKOFF: Duration = Duration::from_secs(1800);
/// `DefaultHealthPort`.
pub const DEFAULT_HEALTH_PORT: i32 = 19514;
/// `DefaultMaxConcurrentTasks`.
pub const DEFAULT_MAX_CONCURRENT_TASKS: i64 = 20;
/// `DefaultGCInterval`.
pub const DEFAULT_GC_INTERVAL: Duration = Duration::from_secs(2 * 3600);
/// `DefaultGCTTL`: 1 day — AI-coding issues rarely stay open long.
pub const DEFAULT_GC_TTL: Duration = Duration::from_secs(24 * 3600);
/// `DefaultGCCompletedTaskTTLCloud`: 14 days — Cloud bounds completed
/// issue-task env retention by default.
pub const DEFAULT_GC_COMPLETED_TASK_TTL_CLOUD: Duration = Duration::from_secs(14 * 24 * 3600);
/// `DefaultGCCompletedTaskTTLSelfHost`: disabled — self-host keeps every
/// completed env until its issue goes terminal, unless an operator opts in.
pub const DEFAULT_GC_COMPLETED_TASK_TTL_SELF_HOST: Duration = Duration::ZERO;
/// `DefaultGCOrphanTTL`: 3 days — orphans with no meta.
pub const DEFAULT_GC_ORPHAN_TTL: Duration = Duration::from_secs(72 * 3600);
/// `DefaultGCArtifactTTL`: 12h — drop regenerable artifacts once completed
/// this long.
pub const DEFAULT_GC_ARTIFACT_TTL: Duration = Duration::from_secs(12 * 3600);
/// `DefaultGCCodexSessionTTL`: 14 days — reclaim untouched Codex session
/// stores.
pub const DEFAULT_GC_CODEX_SESSION_TTL: Duration = Duration::from_secs(14 * 24 * 3600);
/// `DefaultGCHermesMemoryTTL`: 90 days — reclaiming these is visible amnesia
/// and they are a few markdown files, so the TTL is long.
pub const DEFAULT_GC_HERMES_MEMORY_TTL: Duration = Duration::from_secs(90 * 24 * 3600);
/// `DefaultGCHermesSessionTTL`: 14 days — these hold Agent event histories; losing an
/// idle one restarts the thread rather than the agent's notes.
pub const DEFAULT_GC_HERMES_SESSION_TTL: Duration = Duration::from_secs(14 * 24 * 3600);
/// `DefaultGCRepoTTL`: 30 days — evict a bare repo cache no task has checked
/// out this long.
pub const DEFAULT_GC_REPO_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
/// `DefaultAutoUpdateCheckInterval`: how often the daemon polls GitHub for a
/// newer CLI release.
pub const DEFAULT_AUTO_UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 3600);

/// `DefaultGCArtifactPatterns`: conservative regenerable build-artifact
/// basenames only; extend via PATCHBAY_GC_ARTIFACT_PATTERNS.
pub fn default_gc_artifact_patterns() -> Vec<String> {
    vec![
        "node_modules".to_string(),
        ".next".to_string(),
        ".turbo".to_string(),
    ]
}

// ---------------------------------------------------------------------------
// Config / Overrides (config.go:90–179)
// ---------------------------------------------------------------------------

/// `Config`: all daemon configuration.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub server_base_url: String,
    pub daemon_id: String,
    /// Historical daemon_ids this machine may have registered under; reported
    /// at register time so the server can merge old runtime rows.
    pub legacy_daemon_ids: Vec<String>,
    pub device_name: String,
    pub runtime_name: String,
    /// patchbay CLI version (e.g. "0.1.13").
    pub cli_version: String,
    /// "desktop" when spawned by the Electron app, empty for standalone.
    pub launched_by: String,
    /// Profile name (empty = default).
    pub profile: String,
    /// Discovered agent CLIs keyed by provider.
    pub agents: BTreeMap<String, crate::types::AgentEntry>,
    /// Base path for execution envs (default ~/patchbay_workspaces).
    pub workspaces_root: String,
    /// Preserve env after task for debugging.
    pub keep_env_after_task: bool,
    /// Local HTTP port for health checks (default 19514).
    pub health_port: i32,
    /// Max tasks running in parallel (default 20).
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
    /// Explicit override for the Codex first-turn ceiling; 0 = unset (GH
    /// #3262).
    pub codex_first_turn_no_progress_timeout: Duration,
    pub codex_handshake_timeout: Duration,
    pub opencode_idle_watchdog: Duration,
    pub agent_idle_watchdog: Duration,
    pub agent_tool_watchdog: Duration,
    pub claude_args: Vec<String>,
    pub codex_args: Vec<String>,
    pub codebuddy_args: Vec<String>,
    pub qwen_args: Vec<String>,
    pub qwenpaw_args: Vec<String>,
    /// Custom runtime profile_id → absolute executable path on THIS machine
    /// (PB-3284). Empty means always resolve via PATH.
    pub profile_command_overrides: BTreeMap<String, String>,
}

/// `Overrides`: CLI flags overriding env vars and defaults. Zero values are
/// ignored.
#[derive(Debug, Clone, Default)]
pub(crate) struct Overrides {
    pub server_url: String,
    pub workspaces_root: String,
    pub poll_interval: Duration,
    pub heartbeat_interval: Duration,
    /// Option so an explicit `--agent-timeout 0` (no cap) is distinguishable
    /// from "flag not passed".
    pub agent_timeout: Option<Duration>,
    pub codex_semantic_inactivity_timeout: Duration,
    pub codex_handshake_timeout: Duration,
    pub max_concurrent_tasks: i64,
    pub daemon_id: String,
    pub device_name: String,
    pub runtime_name: String,
    pub profile: String,
    pub health_port: i32,
    /// Reserved for read-only local configuration probes; startup still
    /// refuses to run with no agent CLI.
    pub allow_no_agents: bool,
    /// Forces the auto-update poller off (no symmetric force-on).
    pub disable_auto_update: bool,
    pub auto_update_check_interval: Duration,
    /// Forces the on-disk version watcher off.
    pub disable_auto_reload: bool,
    /// Loaded CLI config for backend overrides (Go: cli.LoadCLIConfigForProfile
    /// result), when a caller has one. Absence = "no override", matching Go's
    /// non-fatal handling of a missing config file.
    pub cli_profile_overrides: Option<CliProfileConfig>,
    /// CLI version stamped by Daemon.Run after load (Go: cfg.CLIVersion).
    pub cli_version: String,
    /// "desktop" when the Electron app spawned this daemon.
    pub launched_by: String,
}

/// CLI profile fields consumed by daemon configuration.
#[derive(Debug, Clone, Default)]
pub(crate) struct CliProfileConfig {
    pub profile_command_overrides: BTreeMap<String, String>,
    pub openclaw_binary_path: String,
    pub openclaw_state_dir: String,
    pub openclaw_cli_timeout: String,
}

// ---------------------------------------------------------------------------
// LoadConfig (config.go:183–563)
// ---------------------------------------------------------------------------

/// `LoadConfig`: builds the daemon configuration from environment variables
/// and optional CLI flag overrides.
///
/// Agent discovery remains an injected [`ProbeAgents`] hook so production and
/// tests share the same validation path. OpenClaw overrides are applied from
/// the loaded CLI profile supplied through [`Overrides::openclaw_override`].
#[allow(clippy::too_many_lines)]
pub(crate) fn load_config(
    overrides: Overrides,
    probe_agents: ProbeAgents<'_>,
) -> anyhow::Result<Config> {
    // Server URL: override > env > default
    let raw_server_url = crate::helpers::env_or_default("PATCHBAY_SERVER_URL", DEFAULT_SERVER_URL);
    let raw_server_url = if !overrides.server_url.is_empty() {
        overrides.server_url.clone()
    } else {
        raw_server_url
    };
    let server_base_url = normalize_server_base_url(&raw_server_url)?;

    // Apply backend overrides from the CLI config file (issue #3875) when a
    // caller supplies one; see module docs. Errors loading it are non-fatal in
    // Go; here absence simply means "no override".
    let mut profile_command_overrides = BTreeMap::new();
    if let Some(cli_cfg) = &overrides.cli_profile_overrides {
        if let Some(oc) = openclaw_override_from(cli_cfg) {
            apply_openclaw_override(Some(&oc));
        }
        for (id, path) in &cli_cfg.profile_command_overrides {
            if id.is_empty() || path.trim().is_empty() {
                continue;
            }
            profile_command_overrides.insert(id.clone(), path.clone());
        }
    }

    // Discover installed agent CLIs (PB-5439 re-runs this live).
    let agents = probe_agents();
    if agents.is_empty() && !overrides.allow_no_agents {
        anyhow::bail!(
            "no agent CLI found: install claude, codebuddy, codex, copilot, opencode, deveco, \
             openclaw, hermes, pi, omp, cursor-agent, kimi, reasonix, dsh, kiro-cli, agy, qodercli, \
             qoderclicn, traecli, grok, qwen, qwenpaw, mcode, or dim and ensure it is on PATH"
        );
    }

    let claude_args = shell_args_from_env("PATCHBAY_CLAUDE_ARGS")?;
    let codex_args = shell_args_from_env("PATCHBAY_CODEX_ARGS")?;
    let codebuddy_args = shell_args_from_env("PATCHBAY_CODEBUDDY_ARGS")?;
    let qwen_args = shell_args_from_env("PATCHBAY_QWEN_ARGS")?;
    let qwenpaw_args = shell_args_from_env("PATCHBAY_QWENPAW_ARGS")?;

    // Host info
    let mut host = hostname();
    if host.trim().is_empty() {
        host = "local-machine".to_string();
    }

    // Durations: override > env > default
    let mut poll_interval =
        crate::helpers::duration_from_env("PATCHBAY_DAEMON_POLL_INTERVAL", DEFAULT_POLL_INTERVAL)?;
    if overrides.poll_interval > Duration::ZERO {
        poll_interval = overrides.poll_interval;
    }

    let mut heartbeat_interval = crate::helpers::duration_from_env(
        "PATCHBAY_DAEMON_HEARTBEAT_INTERVAL",
        DEFAULT_HEARTBEAT_INTERVAL,
    )?;
    if overrides.heartbeat_interval > Duration::ZERO {
        heartbeat_interval = overrides.heartbeat_interval;
    }

    let mut agent_timeout =
        crate::helpers::duration_from_env("PATCHBAY_AGENT_TIMEOUT", DEFAULT_AGENT_TIMEOUT)?;
    if let Some(explicit) = overrides.agent_timeout {
        agent_timeout = explicit;
    }

    let mut codex_semantic_inactivity_timeout = crate::helpers::duration_from_env(
        "PATCHBAY_CODEX_SEMANTIC_INACTIVITY_TIMEOUT",
        DEFAULT_CODEX_SEMANTIC_INACTIVITY_TIMEOUT,
    )?;
    if overrides.codex_semantic_inactivity_timeout > Duration::ZERO {
        codex_semantic_inactivity_timeout = overrides.codex_semantic_inactivity_timeout;
    }

    // 0 = unset: positive value is an explicit operator override (GH #3262 /
    // #5959). Warn when the effective first-turn wait would be truncated by
    // the semantic timer armed first (GH #3291).
    let codex_first_turn_no_progress_timeout =
        crate::helpers::duration_from_env("PATCHBAY_CODEX_FIRST_TURN_TIMEOUT", Duration::ZERO)?;
    if codex_first_turn_no_progress_timeout > Duration::ZERO {
        let effective_semantic_timeout = if codex_semantic_inactivity_timeout <= Duration::ZERO {
            DEFAULT_CODEX_SEMANTIC_INACTIVITY_TIMEOUT
        } else {
            codex_semantic_inactivity_timeout
        };
        if codex_first_turn_no_progress_timeout >= effective_semantic_timeout {
            tracing::warn!(
                first_turn_timeout = %humantime_secs(codex_first_turn_no_progress_timeout),
                semantic_inactivity_timeout = %humantime_secs(effective_semantic_timeout),
                "PATCHBAY_CODEX_FIRST_TURN_TIMEOUT is greater than or equal to the semantic-inactivity timeout; the effective first-turn wait is truncated to the semantic timeout and the model-catalog startup retry is disabled. Because the semantic timer is armed first and equal durations do not deterministically favour the first-turn deadline, set PATCHBAY_CODEX_SEMANTIC_INACTIVITY_TIMEOUT strictly above PATCHBAY_CODEX_FIRST_TURN_TIMEOUT (with some margin) to preserve it."
            );
        }
    }

    let mut codex_handshake_timeout = crate::helpers::duration_from_env(
        "PATCHBAY_CODEX_HANDSHAKE_TIMEOUT",
        DEFAULT_CODEX_HANDSHAKE_TIMEOUT,
    )?;
    if codex_handshake_timeout <= Duration::ZERO {
        codex_handshake_timeout = DEFAULT_CODEX_HANDSHAKE_TIMEOUT;
    }
    if overrides.codex_handshake_timeout > Duration::ZERO {
        codex_handshake_timeout = overrides.codex_handshake_timeout;
    }

    // PATCHBAY_AGENT_IDLE_WATCHDOG=0 disables; positive duration overrides.
    let agent_idle_watchdog = crate::helpers::duration_from_env(
        "PATCHBAY_AGENT_IDLE_WATCHDOG",
        DEFAULT_AGENT_IDLE_WATCHDOG,
    )?;
    // Zero removes the provider-specific override; positive values cannot
    // extend the global bound.
    let opencode_idle_watchdog = crate::helpers::duration_from_env(
        "PATCHBAY_OPENCODE_IDLE_WATCHDOG",
        DEFAULT_OPENCODE_IDLE_WATCHDOG,
    )?;
    // PATCHBAY_AGENT_TOOL_WATCHDOG=0 disables the in-flight-tool backstop.
    let agent_tool_watchdog = crate::helpers::duration_from_env(
        "PATCHBAY_AGENT_TOOL_WATCHDOG",
        DEFAULT_AGENT_TOOL_WATCHDOG,
    )?;

    let mut max_concurrent_tasks = crate::helpers::int_from_env(
        "PATCHBAY_DAEMON_MAX_CONCURRENT_TASKS",
        DEFAULT_MAX_CONCURRENT_TASKS,
    )?;
    if overrides.max_concurrent_tasks > 0 {
        max_concurrent_tasks = overrides.max_concurrent_tasks;
    }

    // Profile
    let profile = overrides.profile.clone();

    // daemon_id resolution: override > env > persistent UUID on disk (#1220).
    let mut daemon_id = std::env::var("PATCHBAY_DAEMON_ID")
        .unwrap_or_default()
        .trim()
        .to_string();
    if !overrides.daemon_id.is_empty() {
        daemon_id = overrides.daemon_id.clone();
    }
    if daemon_id.is_empty() {
        daemon_id = crate::identity::ensure_daemon_id(&profile)
            .map_err(|err| err.context("ensure daemon id"))?;
    }
    // Historical daemon_ids from hostname/profile + pre-#1220 per-profile
    // files, minus anything colliding with the resolved id.
    let mut legacy_daemon_ids = crate::identity::legacy_daemon_ids(&host, &profile);
    if let Ok(uuids) = crate::identity::legacy_daemon_uuids() {
        legacy_daemon_ids.extend(uuids);
    }
    legacy_daemon_ids = crate::identity::filter_legacy_ids(legacy_daemon_ids, &daemon_id);

    let mut device_name = crate::helpers::env_or_default("PATCHBAY_DAEMON_DEVICE_NAME", &host);
    if !overrides.device_name.is_empty() {
        device_name = overrides.device_name.clone();
    }

    let mut runtime_name =
        crate::helpers::env_or_default("PATCHBAY_AGENT_RUNTIME_NAME", DEFAULT_RUNTIME_NAME);
    if !overrides.runtime_name.is_empty() {
        runtime_name = overrides.runtime_name.clone();
    }

    // Workspaces root: override > env > default.
    let workspaces_root = resolve_workspaces_root(&profile, &overrides.workspaces_root)?;

    // Health port: override > default
    let health_port = if overrides.health_port > 0 {
        overrides.health_port
    } else {
        DEFAULT_HEALTH_PORT
    };

    // Keep env after task: env > default (false)
    let keep_env = matches!(
        std::env::var("PATCHBAY_KEEP_ENV_AFTER_TASK").as_deref(),
        Ok("true") | Ok("1")
    );

    // GC config: env > defaults
    let gc_enabled = !matches!(
        std::env::var("PATCHBAY_GC_ENABLED").as_deref(),
        Ok("false") | Ok("0")
    );
    let gc_interval =
        crate::helpers::duration_from_env("PATCHBAY_GC_INTERVAL", DEFAULT_GC_INTERVAL)?;
    let gc_ttl = crate::helpers::duration_from_env("PATCHBAY_GC_TTL", DEFAULT_GC_TTL)?;
    let gc_completed_task_ttl = crate::helpers::duration_from_env(
        "PATCHBAY_GC_COMPLETED_TASK_TTL",
        default_gc_completed_task_ttl(&server_base_url),
    )?;
    let gc_orphan_ttl =
        crate::helpers::duration_from_env("PATCHBAY_GC_ORPHAN_TTL", DEFAULT_GC_ORPHAN_TTL)?;
    let gc_artifact_ttl =
        crate::helpers::duration_from_env("PATCHBAY_GC_ARTIFACT_TTL", DEFAULT_GC_ARTIFACT_TTL)?;
    let gc_codex_session_ttl = crate::helpers::duration_from_env(
        "PATCHBAY_GC_CODEX_SESSION_TTL",
        DEFAULT_GC_CODEX_SESSION_TTL,
    )?;
    let gc_hermes_memory_ttl = crate::helpers::duration_from_env(
        "PATCHBAY_GC_HERMES_MEMORY_TTL",
        DEFAULT_GC_HERMES_MEMORY_TTL,
    )?;
    let gc_hermes_session_ttl = crate::helpers::duration_from_env(
        "PATCHBAY_GC_HERMES_SESSION_TTL",
        DEFAULT_GC_HERMES_SESSION_TTL,
    )?;
    let gc_repo_ttl =
        crate::helpers::duration_from_env("PATCHBAY_GC_REPO_TTL", DEFAULT_GC_REPO_TTL)?;
    let gc_repo_maintenance_enabled =
        crate::helpers::bool_from_env("PATCHBAY_GC_REPO_MAINTENANCE_ENABLED", true);
    let gc_artifact_patterns = patterns_from_env(
        "PATCHBAY_GC_ARTIFACT_PATTERNS",
        &default_gc_artifact_patterns(),
    );

    // Auto-update: opt-in on official cloud, opt-out self-host (PB-2381).
    let mut auto_update_enabled = crate::helpers::bool_from_env(
        "PATCHBAY_DAEMON_AUTO_UPDATE",
        is_official_cloud_server(&server_base_url),
    );
    if overrides.disable_auto_update {
        auto_update_enabled = false;
    }
    let mut auto_update_interval = crate::helpers::duration_from_env(
        "PATCHBAY_DAEMON_AUTO_UPDATE_INTERVAL",
        DEFAULT_AUTO_UPDATE_CHECK_INTERVAL,
    )?;
    if overrides.auto_update_check_interval > Duration::ZERO {
        auto_update_interval = overrides.auto_update_check_interval;
    }

    // Auto-reload is deliberately NOT gated on autoUpdateEnabled ("don't pull
    // new versions" vs "follow the binary I replaced myself" are different
    // concerns).
    let mut auto_reload_enabled =
        crate::helpers::bool_from_env("PATCHBAY_DAEMON_AUTO_RELOAD", true);
    if overrides.disable_auto_reload {
        auto_reload_enabled = false;
    }

    Ok(Config {
        server_base_url,
        // Set by Daemon.Run after config load (Go assigns post-hoc).
        cli_version: overrides.cli_version.clone(),
        launched_by: overrides.launched_by.clone(),
        daemon_id,
        legacy_daemon_ids,
        device_name,
        runtime_name,
        profile,
        agents,
        workspaces_root,
        keep_env_after_task: keep_env,
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
        auto_update_check_interval: auto_update_interval,
        auto_reload_enabled,
        health_port,
        max_concurrent_tasks,
        poll_interval,
        heartbeat_interval,
        agent_timeout,
        codex_semantic_inactivity_timeout,
        codex_first_turn_no_progress_timeout,
        codex_handshake_timeout,
        opencode_idle_watchdog,
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

/// Injected agent-CLI discovery seam. Returns provider → entry.
pub(crate) type ProbeAgents<'a> = &'a dyn Fn() -> BTreeMap<String, crate::types::AgentEntry>;

/// `openclawOverrideFrom` + navigation over the nullable chain (config.go:1100).
fn openclaw_override_from(cfg: &CliProfileConfig) -> Option<OpenClawOverride> {
    Some(OpenClawOverride {
        binary_path: cfg.openclaw_binary_path.clone(),
        state_dir: cfg.openclaw_state_dir.clone(),
        cli_timeout: cfg.openclaw_cli_timeout.clone(),
    })
}

// ---------------------------------------------------------------------------
// URL / path resolution helpers (config.go:566–701)
// ---------------------------------------------------------------------------

/// `officialCloudHost`: the only origin treated as "official" for defaults;
/// staging/preview subdomains deliberately inherit the safer self-host default.
pub const OFFICIAL_CLOUD_HOST: &str = "api.patchbay.ai";

/// `isOfficialCloudServer`: host-only, case-insensitive; port and path ignored.
pub(crate) fn is_official_cloud_server(base_url: &str) -> bool {
    let Ok(u) = url::Url::parse(base_url.trim()) else {
        return false;
    };
    u.host_str()
        .map(|h| h.eq_ignore_ascii_case(OFFICIAL_CLOUD_HOST))
        .unwrap_or(false)
}

/// `defaultGCCompletedTaskTTL`: cloud 14d, self-host disabled — see Go doc
/// comment for the irreversibility rationale.
pub(crate) fn default_gc_completed_task_ttl(server_base_url: &str) -> Duration {
    if is_official_cloud_server(server_base_url) {
        DEFAULT_GC_COMPLETED_TASK_TTL_CLOUD
    } else {
        DEFAULT_GC_COMPLETED_TASK_TTL_SELF_HOST
    }
}

/// `NormalizeServerBaseURL`: converts a WebSocket or HTTP URL to a base HTTP
/// URL. Byte-for-byte semantics with Go including the trailing-slash trim and
/// the `/ws` path strip.
pub fn normalize_server_base_url(raw: &str) -> anyhow::Result<String> {
    let mut u = url::Url::parse(raw.trim())
        .map_err(|err| anyhow::anyhow!("invalid PATCHBAY_SERVER_URL: {err}"))?;
    match u.scheme() {
        "ws" => {
            u.set_scheme("http")
                .map_err(|_| anyhow::anyhow!("set scheme"))?;
        }
        "wss" => {
            u.set_scheme("https")
                .map_err(|_| anyhow::anyhow!("set scheme"))?;
        }
        "http" | "https" => {}
        _ => anyhow::bail!("PATCHBAY_SERVER_URL must use ws, wss, http, or https"),
    }
    if u.path() == "/ws" {
        u.set_path("");
    }
    u.set_query(None);
    u.set_fragment(None);
    let s = u.to_string();
    Ok(s.trim_end_matches('/').to_string())
}

/// `TaskWorkspacesRootEnv`: carries the owning daemon's workspaces root into a
/// managed task's environment so task-mode disk-usage scans the right tree.
pub const TASK_WORKSPACES_ROOT_ENV: &str = "PATCHBAY_TASK_WORKSPACES_ROOT";

/// `ResolveWorkspacesRoot`: explicit override > PATCHBAY_WORKSPACES_ROOT env >
/// default ($HOME/patchbay_workspaces[_<profile>]).
pub fn resolve_workspaces_root(profile: &str, override_root: &str) -> anyhow::Result<String> {
    let mut root = std::env::var("PATCHBAY_WORKSPACES_ROOT")
        .unwrap_or_default()
        .trim()
        .to_string();
    if !override_root.is_empty() {
        root = override_root.to_string();
    }
    if root.is_empty() {
        let home = home_dir()?;
        root = if profile.is_empty() {
            format!("{home}/patchbay_workspaces")
        } else {
            format!("{home}/patchbay_workspaces_{profile}")
        };
    }
    let abs = crate::config::absolute(&root)
        .map_err(|err| anyhow::Error::from(err).context("resolve absolute workspaces root"))?;
    Ok(abs.to_string_lossy().into_owned())
}

fn home_dir() -> anyhow::Result<String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| {
            anyhow::anyhow!("resolve home directory (set PATCHBAY_WORKSPACES_ROOT to override)")
        })
}

/// `ArtifactPatternsFromEnv`.
pub fn artifact_patterns_from_env() -> Vec<String> {
    patterns_from_env(
        "PATCHBAY_GC_ARTIFACT_PATTERNS",
        &default_gc_artifact_patterns(),
    )
}

/// `patternsFromEnv`: comma-separated list; patterns containing path separators
/// are silently dropped (basename-only matcher).
fn patterns_from_env(name: &str, defaults: &[String]) -> Vec<String> {
    let raw = std::env::var(name).unwrap_or_default().trim().to_string();
    if raw.is_empty() {
        return defaults.to_vec();
    }
    raw.split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty() && !p.contains('/') && !p.contains('\\'))
        .map(|p| p.to_string())
        .collect()
}

/// `shellArgsFromEnv` (config.go:703): POSIX-ish shell-word splitting of an
/// env var. shellwords.Parse handles single/double quotes and backslashes;
/// this is the same grammar subset the daemon's five ARG vars need.
fn shell_args_from_env(name: &str) -> anyhow::Result<Vec<String>> {
    let raw = std::env::var(name).unwrap_or_default().trim().to_string();
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    parse_shell_words(&raw).ok_or_else(|| anyhow::anyhow!("invalid {name}"))
}

/// Minimal go-shellwords-compatible splitter (quote/backslash aware, no
/// command substitution — Go's Parse without ParseBacktick never expands).
fn parse_shell_words(s: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    let mut started = false;
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            '\'' => {
                started = true;
                let mut closed = false;
                for c2 in chars.by_ref() {
                    if c2 == '\'' {
                        closed = true;
                        break;
                    }
                    cur.push(c2);
                }
                if !closed {
                    return None;
                }
            }
            '"' => {
                started = true;
                let mut closed = false;
                while let Some(c2) = chars.next() {
                    match c2 {
                        '"' => {
                            closed = true;
                            break;
                        }
                        '\\' => {
                            if let Some(&n) = chars.peek() {
                                if n == '"' || n == '\\' {
                                    cur.push(n);
                                    chars.next();
                                } else {
                                    cur.push('\\');
                                }
                            } else {
                                return None;
                            }
                        }
                        _ => cur.push(c2),
                    }
                }
                if !closed {
                    return None;
                }
            }
            '\\' => {
                started = true;
                cur.push(chars.next()?);
            }
            _ => {
                started = true;
                cur.push(c);
            }
        }
    }
    if started {
        out.push(cur);
    }
    Some(out)
}

/// os.Hostname equivalent.
fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .unwrap_or_default()
}

fn humantime_secs(d: Duration) -> String {
    format!("{}s", d.as_secs_f64())
}

// ---------------------------------------------------------------------------
// Agent executable resolution (config.go:715–1094)
// ---------------------------------------------------------------------------

/// `resolveAgentExecutablePath` (config.go:727): the executable entry point to
/// keep for an agent command. Bare names are pinned so later PATH changes
/// cannot redirect launches; ~/.patchbay/hooks shadowing is skipped to avoid
/// wrapper recursion.
pub(crate) fn resolve_agent_executable_path(cmd: &str) -> anyhow::Result<String> {
    let resolved = look_path(cmd)?;
    if cmd.contains('/') || cmd.contains('\\') {
        return Ok(crate::canonical_path::canonical_configured_executable_path(
            &resolved,
        ));
    }
    if is_in_patchbay_hooks_dir(&resolved) {
        if let Ok(unshadowed) = look_path_excluding_patchbay_hooks(cmd) {
            return Ok(discovered_executable_path(&unshadowed));
        }
    }
    Ok(discovered_executable_path(&resolved))
}

/// `agentExecutablePresent` (config.go:748): a pinned path that no longer
/// LookPaths has vanished from disk (PB-4486).
pub(crate) fn agent_executable_present(path: &str) -> bool {
    !path.is_empty() && look_path(path).is_ok()
}

/// Strict launch-boundary check for an already-resolved executable path.
/// Unlike [`agent_executable_present`], this preserves the underlying I/O
/// error so callers can self-heal a genuinely vanished path without treating
/// permission, filesystem, or executable-policy failures as a PATH miss.
pub(crate) fn check_agent_executable_for_launch(path: &str) -> std::io::Result<()> {
    if path.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "agent executable path is empty",
        ));
    }
    let metadata = std::fs::metadata(path)?;
    if metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "agent executable path is a directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "agent executable path is not executable",
            ));
        }
    }
    Ok(())
}

/// `reresolveAgentCommand` (config.go:764): re-runs startup resolution on the
/// miss path only, with the login-shell fallback for bare names.
pub(crate) fn reresolve_agent_command(cmd: &str) -> Option<String> {
    if cmd.is_empty() {
        return None;
    }
    if let Ok(path) = resolve_agent_executable_path(cmd) {
        return Some(path);
    }
    // Bare name invisible on our PATH: retry via the user's login shell.
    // Absolute/relative overrides stay hard misses.
    if !cmd.contains('/') && !cmd.contains('\\') {
        if let Some(path) =
            resolve_agents_via_login_shell(std::slice::from_ref(&cmd.to_string())).get(cmd)
        {
            return Some(path.clone());
        }
    }
    None
}

/// exec.LookPath equivalent: searches PATH for an executable file when `cmd`
/// is bare; validates executability for explicit paths (Go's LookPath also
/// requires an executable match for paths containing separators).
fn look_path(cmd: &str) -> anyhow::Result<String> {
    if cmd.contains('/') || cmd.contains('\\') {
        for candidate in executable_candidates(PathBuf::from(cmd)) {
            if is_executable_file_cmd(&candidate.to_string_lossy()) {
                return Ok(candidate.to_string_lossy().into_owned());
            }
        }
        anyhow::bail!("exec: {}: not found", cmd);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let dir = if dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            dir
        };
        for candidate in executable_candidates(dir.join(cmd)) {
            if is_executable_file_cmd(&candidate.to_string_lossy()) {
                return Ok(candidate.to_string_lossy().into_owned());
            }
        }
    }
    anyhow::bail!("exec: {}: not found", cmd)
}

#[cfg(not(windows))]
fn executable_candidates(path: PathBuf) -> Vec<PathBuf> {
    vec![path]
}

#[cfg(windows)]
fn executable_candidates(path: PathBuf) -> Vec<PathBuf> {
    if path.extension().is_some() {
        return vec![path];
    }
    let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    extensions
        .split(';')
        .filter_map(|ext| {
            let ext = ext.trim().trim_start_matches('.');
            (!ext.is_empty()).then(|| path.with_extension(ext))
        })
        .collect()
}

fn is_executable_file_cmd(path: &str) -> bool {
    crate::config::is_executable_file(path)
}

/// `lookPathExcludingPatchbayHooks` (config.go:784).
fn look_path_excluding_patchbay_hooks(cmd: &str) -> anyhow::Result<String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let dir = if dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            dir
        };
        if is_patchbay_hooks_dir(&dir.to_string_lossy()) {
            continue;
        }
        for candidate in executable_candidates(dir.join(cmd)) {
            let candidate = candidate.to_string_lossy().into_owned();
            if crate::config::is_executable_file(&candidate) {
                return Ok(discovered_executable_path(&candidate));
            }
        }
    }
    Err(anyhow::anyhow!("exec: not found"))
}

/// `isInPatchbayHooksDir` (config.go:800).
fn is_in_patchbay_hooks_dir(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let parent = std::path::Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    is_patchbay_hooks_dir(&parent)
}

/// `isPatchbayHooksDir` (config.go:807).
fn is_patchbay_hooks_dir(dir: &str) -> bool {
    let Ok(home) = home_dir() else {
        return false;
    };
    if home.is_empty() {
        return false;
    }
    same_path_dir(dir, &format!("{home}/.patchbay/hooks"))
}

/// `OpenclawCLITimeoutEnv` (openclaw_config.go:76).
pub(crate) const OPENCLAW_CLI_TIMEOUT_ENV: &str = "PATCHBAY_OPENCLAW_CLI_TIMEOUT";

/// `discoveredExecutablePath` (canonical_path.go:25): canonicalize but keep
/// the invoked basename for shim-dispatched entrypoints (see canonical_path.rs).
fn discovered_executable_path(path: &str) -> String {
    crate::canonical_path::discovered_executable_path(path)
}

/// `loginShellResolveTimeout` / `loginShellResolveWaitDelay` (config.go:896,
/// 908): total startup penalty from a pathological rc file is bounded by
/// timeout + wait_delay.
const LOGIN_SHELL_RESOLVE_TIMEOUT: Duration = Duration::from_secs(3);
const LOGIN_SHELL_RESOLVE_WAIT_DELAY: Duration = Duration::from_secs(2);

/// `supportedLoginShells`: POSIX-compatible interpreters only; fish excluded.
fn is_supported_login_shell(shell_base: &str) -> bool {
    matches!(shell_base, "bash" | "zsh" | "sh" | "dash" | "ksh")
}

/// `resolveAgentsViaLoginShell` (config.go:956): asks the user's login shell
/// (`$SHELL -ilc`) to print absolute, invocation-safe paths, since daemon-style
/// processes don't inherit interactive PATH additions (fnm/nvm/volta,
/// native installers). Only outputs that still pass a fresh LookPath are
/// trusted.
pub(crate) fn resolve_agents_via_login_shell(names: &[String]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if names.is_empty() {
        return out;
    }
    let shell = std::env::var("SHELL")
        .unwrap_or_default()
        .trim()
        .to_string();
    if shell.is_empty() {
        return out;
    }
    let base = std::path::Path::new(&shell)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !is_supported_login_shell(&base) {
        return out;
    }
    let safe: Vec<&String> = names.iter().filter(|n| is_safe_agent_name(n)).collect();
    if safe.is_empty() {
        return out;
    }

    let script = build_login_shell_resolve_script(safe.iter().map(|s| s.as_str()).collect());
    let child = std::process::Command::new(&shell)
        .args(["-ilc", &script])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn();
    let output = child.and_then(|mut c| {
        // Go bounds this with CommandContext + WaitDelay; a coarse wall-clock
        // cap here serves the same purpose (kill + reap).
        wait_with_timeout(
            &mut c,
            LOGIN_SHELL_RESOLVE_TIMEOUT + LOGIN_SHELL_RESOLVE_WAIT_DELAY,
        )
    });
    let Ok(raw) = output else {
        return out;
    };
    for line in raw.trim().split('\n') {
        let mut parts = line.splitn(2, '\t');
        let (Some(name), Some(rest)) = (parts.next(), parts.next()) else {
            continue;
        };
        let path = rest.trim();
        if !std::path::Path::new(path).is_absolute() {
            continue;
        }
        // Final reality check from the daemon's vantage point (fnm multishells).
        if look_path(path).is_err() {
            continue;
        }
        out.insert(name.to_string(), path.to_string());
    }
    out
}

pub(crate) fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> std::io::Result<String> {
    use std::io::Read;
    use std::sync::mpsc::{self, TryRecvError};

    let deadline = std::time::Instant::now() + timeout;
    // Read stdout on a helper thread so a quiet shell (or a descendant that
    // inherits stdout) cannot block the timeout poll in `Read::read`.
    let mut stdout = child.stdout.take().expect("piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || loop {
        let mut chunk = vec![0u8; 4096];
        match stdout.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => {
                chunk.truncate(n);
                if tx.send(Ok(chunk)).is_err() {
                    return;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(err) => {
                let _ = tx.send(Err(err));
                return;
            }
        }
    });

    let mut buf = Vec::new();
    let mut child_exited = false;
    let mut reader_done = false;
    loop {
        loop {
            match rx.try_recv() {
                Ok(Ok(chunk)) => buf.extend_from_slice(&chunk),
                Ok(Err(err)) => return Err(err),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    reader_done = true;
                    break;
                }
            }
        }

        if !child_exited {
            child_exited = child.try_wait()?.is_some();
        }
        if child_exited && reader_done {
            break;
        }
        if std::time::Instant::now() >= deadline {
            if !child_exited {
                let _ = child.kill();
                let _ = child.wait();
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !child_exited {
        let _ = child.wait();
    }
    while let Ok(result) = rx.try_recv() {
        buf.extend_from_slice(&result?);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// `buildLoginShellResolveScript` (config.go:1043): verbatim script body; all
/// input names vetted by is_safe_agent_name before inlining unquoted.
fn build_login_shell_resolve_script(names: Vec<&str>) -> String {
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
    b.push_str("  if [ -n \"${HOME:-}\" ]; then hd=\"$HOME/.patchbay/hooks\"; hc=$(cd \"$hd\" 2>/dev/null && pwd -P) || hc=\"\"; fi\n");
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

/// `isSafeAgentName` (config.go:1079): ASCII letters, digits, dot, dash,
/// underscore — guards shell-script inlining.
fn is_safe_agent_name(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    s.bytes().all(|r| {
        r.is_ascii_lowercase()
            || r.is_ascii_uppercase()
            || r.is_ascii_digit()
            || r == b'-'
            || r == b'_'
            || r == b'.'
    })
}

/// `applyOpenclawOverride` (config.go:1124): translates config-file overrides
/// into env vars, user-exported vars winning. Side-effecting Setenv is
/// intentional and scoped to OpenClaw-specific vars during startup.
fn apply_openclaw_override(oc: Option<&OpenClawOverride>) {
    let Some(oc) = oc else {
        return;
    };
    if !oc.binary_path.is_empty() && std::env::var_os("PATCHBAY_OPENCLAW_PATH").is_none() {
        // SAFETY-free: single-threaded startup window before any backend spawn.
        std::env::set_var("PATCHBAY_OPENCLAW_PATH", &oc.binary_path);
    }
    if !oc.state_dir.is_empty() && std::env::var_os("OPENCLAW_STATE_DIR").is_none() {
        std::env::set_var("OPENCLAW_STATE_DIR", &oc.state_dir);
    }
    if !oc.cli_timeout.is_empty() && std::env::var_os(OPENCLAW_CLI_TIMEOUT_ENV).is_none() {
        std::env::set_var(OPENCLAW_CLI_TIMEOUT_ENV, &oc.cli_timeout);
    }
}

/// `cli.OpenClawOverride`.
#[derive(Debug, Clone, Default)]
pub(crate) struct OpenClawOverride {
    pub binary_path: String,
    pub state_dir: String,
    pub cli_timeout: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_server_base_url_variants() {
        assert_eq!(
            normalize_server_base_url("ws://localhost:8080/ws").unwrap(),
            "http://localhost:8080"
        );
        assert_eq!(
            normalize_server_base_url("wss://api.patchbay.ai/ws").unwrap(),
            "https://api.patchbay.ai"
        );
        assert_eq!(
            normalize_server_base_url("http://example.com/base/").unwrap(),
            "http://example.com/base"
        );
        assert!(normalize_server_base_url("ftp://x").is_err());
    }

    #[test]
    fn official_cloud_detection_host_only() {
        assert!(is_official_cloud_server(
            "https://API.PATCHBAY.AI:443/some/path"
        ));
        // Staging subdomains deliberately excluded.
        assert!(!is_official_cloud_server("https://staging.patchbay.ai"));
        assert!(!is_official_cloud_server("not a url"));
    }

    #[test]
    fn completed_task_ttl_by_deployment_kind() {
        assert_eq!(
            default_gc_completed_task_ttl("https://api.patchbay.ai"),
            DEFAULT_GC_COMPLETED_TASK_TTL_CLOUD
        );
        assert_eq!(
            default_gc_completed_task_ttl("http://localhost:8080"),
            DEFAULT_GC_COMPLETED_TASK_TTL_SELF_HOST
        );
    }

    #[test]
    fn resolve_workspaces_root_profile_suffix() {
        let root = resolve_workspaces_root("staging", "").unwrap();
        assert!(root.ends_with("patchbay_workspaces_staging"), "{root}");
        let root = resolve_workspaces_root("", "").unwrap();
        assert!(root.ends_with("patchbay_workspaces"));
        let custom_root = std::env::temp_dir().join("patchbay-custom-root");
        let root = resolve_workspaces_root("", custom_root.to_str().unwrap()).unwrap();
        assert_eq!(Path::new(&root), custom_root.as_path());
    }

    #[test]
    fn safe_agent_names_only() {
        assert!(is_safe_agent_name("cursor-agent"));
        assert!(is_safe_agent_name("qoderclicn"));
        assert!(!is_safe_agent_name(""));
        assert!(!is_safe_agent_name("a;b"));
        assert!(!is_safe_agent_name("$(x)"));
    }

    #[test]
    fn login_shell_script_shape() {
        let script = build_login_shell_resolve_script(vec!["claude", "codex"]);
        assert!(script.starts_with("for n in claude codex; do\n"));
        assert!(script.contains("unalias \"$n\" 2>/dev/null\n"));
        assert!(script.contains("unset -f \"$n\" 2>/dev/null\n"));
        assert!(script.contains(".patchbay/hooks"));
        assert!(script.ends_with("done\n"));
    }

    #[test]
    fn shell_words_rejects_unterminated_syntax() {
        assert!(parse_shell_words("--model 'unterminated").is_none());
        assert!(parse_shell_words("--model \"unterminated").is_none());
        assert!(parse_shell_words("--model dangling\\").is_none());
    }

    #[test]
    fn shell_words_preserves_valid_quoted_arguments() {
        assert_eq!(
            parse_shell_words(r#"--model "gpt 5" 'high effort' escaped\ value"#),
            Some(vec![
                "--model".to_string(),
                "gpt 5".to_string(),
                "high effort".to_string(),
                "escaped value".to_string(),
            ])
        );
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_stdout_wait_is_bounded() {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 2"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();

        let output = wait_with_timeout(&mut child, Duration::from_millis(50)).unwrap();

        assert!(output.is_empty());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn patterns_from_env_shape_via_defaults() {
        // Indirect check: defaults never contain separators.
        for p in default_gc_artifact_patterns() {
            assert!(!p.contains('/') && !p.contains('\\'));
        }
    }
}
