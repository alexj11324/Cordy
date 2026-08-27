//! OpenClaw task configuration capability.
//!
//! Ports the production contract of Go's `openclaw_config.go` and
//! `openclaw_config_cache.go`: discovery is delegated to the installed CLI,
//! per-task config pins every workspace to the task workdir, managed MCP is a
//! strict replacement, Gateway pins are emitted without leaking tokens in
//! diagnostics, and successful discovery is cached only with non-secret
//! fingerprints.  All malformed or unavailable provider configuration fails
//! closed; only a genuine fresh install (no config file) receives a minimal
//! wrapper.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::execenv::{OpenclawConfigPrep, OpenclawConfigResult, OpenclawGatewayPin};
use super::isolation::ErrOpenclawCliTimeout;

const CONFIG_FILE: &str = "openclaw-config.json";
const SNAPSHOT_FILE: &str = "openclaw-user-snapshot.json";
const CACHE_FILE: &str = "openclaw-discovery-cache.json";
const CACHE_TTL: Duration = Duration::from_secs(60);
const MIN_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiscoveryCache {
    version: u8,
    cached_at_ns: u128,
    bin_path: String,
    bin_size: u64,
    bin_mtime_ns: u128,
    config_path: String,
    config_size: u64,
    config_mtime_ns: u128,
    env: String,
    agents: Value,
    agents_from_registry: bool,
}

#[derive(Debug, Clone)]
struct Discovery {
    path: String,
    exists: bool,
    agents: Vec<Value>,
    agents_from_registry: bool,
    cached: bool,
}

/// A failed CLI invocation keeps stdout available for narrowly-scoped JSON
/// error classification, but never formats it as part of the error. OpenClaw
/// can print a resolved config (including credentials) to stdout before
/// exiting non-zero.
struct OpenclawCliError {
    message: String,
    stdout: Vec<u8>,
}

impl fmt::Debug for OpenclawCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenclawCliError")
            .field("message", &self.message)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for OpenclawCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OpenclawCliError {}

/// Prepare the task wrapper and return the include root for the child env.
pub(crate) fn prepare_openclaw_config(
    env_root: &str,
    work_dir: &str,
    opts: &OpenclawConfigPrep,
) -> anyhow::Result<OpenclawConfigResult> {
    let bin = if opts.openclaw_bin.trim().is_empty() {
        "openclaw".to_string()
    } else {
        opts.openclaw_bin.clone()
    };
    let timeout = resolve_timeout(opts.timeout);
    let discovery = discover(&bin, timeout, &opts.profile)?;
    let (managed_mcp, has_managed_mcp) = managed_mcp(opts.mcp_config.as_ref())?;

    let mut include_user_config = discovery.exists;
    let mut active_path = discovery.path.clone();
    let mut snapshot_path = None;
    if has_managed_mcp && discovery.exists {
        if let Some(mut resolved) = run_json(&bin, timeout, &["config", "get", "--json"])? {
            let resolved = resolved
                .as_object_mut()
                .ok_or_else(|| anyhow!("openclaw resolved config must be a JSON object"))?;
            strip_user_mcp(resolved);
            let path = Path::new(env_root).join(SNAPSHOT_FILE);
            atomic_json(&path, &Value::Object(resolved.clone()), 0o600)?;
            snapshot_path = Some(path.display().to_string());
        } else {
            // An existing config whose resolved representation is empty/null
            // has no user data to include. Keep the managed-only wrapper
            // usable instead of treating an empty global config as fatal.
            include_user_config = false;
            active_path.clear();
        }
    }

    let config = build_wrapper(
        &active_path,
        include_user_config,
        snapshot_path.as_deref(),
        &discovery.agents,
        discovery.agents_from_registry,
        work_dir,
        managed_mcp,
        has_managed_mcp,
        &opts.gateway,
    );
    let out_path = Path::new(env_root).join(CONFIG_FILE);
    atomic_json(&out_path, &config, 0o600)?;
    let include_root = if snapshot_path.is_some() || !include_user_config {
        String::new()
    } else {
        Path::new(&discovery.path)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .display()
            .to_string()
    };
    tracing::info!(
        active_config = %active_path,
        active_config_exists = include_user_config,
        discovery_cached = discovery.cached,
        managed_mcp = has_managed_mcp,
        "execenv: prepared openclaw config"
    );
    Ok(OpenclawConfigResult {
        config_path: out_path.display().to_string(),
        include_root,
    })
}

fn resolve_timeout(explicit: Duration) -> Duration {
    if !explicit.is_zero() {
        // Go uses an explicit timeout verbatim; only the environment override
        // is bounded. The explicit value is test/embedding input and must not
        // silently change the caller's contract.
        return explicit;
    }
    let value = std::env::var("CORDY_OPENCLAW_CLI_TIMEOUT").unwrap_or_default();
    let value = value.trim();
    let parsed = parse_timeout_override(value);
    parsed
        .unwrap_or(Duration::from_secs(30))
        .clamp(MIN_TIMEOUT, MAX_TIMEOUT)
}

fn parse_timeout_override(value: &str) -> Option<Duration> {
    if value.is_empty() || value.starts_with('-') {
        return None;
    }

    let value = value.strip_prefix('+').unwrap_or(value);
    if value.is_empty() {
        return None;
    }

    // Go accepts a bare number as seconds in the caller (which first tries
    // ParseDuration and then retries with an appended `s`). Preserve that
    // compatibility, including a fractional bare number.
    if !value.chars().any(|ch| ch.is_ascii_alphabetic() || ch == 'µ') {
        let seconds = value.parse::<f64>().ok()?;
        return Duration::try_from_secs_f64(seconds)
            .ok()
            .filter(|duration| !duration.is_zero());
    }

    // Match time.ParseDuration's sequence grammar: number+unit, repeated.
    // Parsing components instead of only the final suffix is what accepts
    // values such as `1m30s` while retaining the same supported units.
    let units = [
        ("ns", 1f64 / 1_000_000_000f64),
        ("us", 1f64 / 1_000_000f64),
        ("µs", 1f64 / 1_000_000f64),
        ("ms", 1f64 / 1_000f64),
        ("s", 1f64),
        ("m", 60f64),
        ("h", 3_600f64),
    ];
    let mut cursor = 0;
    let mut seconds = 0f64;
    let mut parsed_component = false;
    while cursor < value.len() {
        let number_start = cursor;
        let mut has_digit = false;
        while cursor < value.len() && value.as_bytes()[cursor].is_ascii_digit() {
            has_digit = true;
            cursor += 1;
        }
        if cursor < value.len() && value.as_bytes()[cursor] == b'.' {
            cursor += 1;
            while cursor < value.len() && value.as_bytes()[cursor].is_ascii_digit() {
                has_digit = true;
                cursor += 1;
            }
        }
        if !has_digit {
            return None;
        }
        let number = value.get(number_start..cursor)?.parse::<f64>().ok()?;
        let (unit, multiplier) = units.iter().find_map(|(unit, multiplier)| {
            value
                .get(cursor..)?
                .starts_with(unit)
                .then_some((*unit, *multiplier))
        })?;
        cursor += unit.len();
        seconds += number * multiplier;
        if !seconds.is_finite() {
            return None;
        }
        parsed_component = true;
    }
    parsed_component
        .then_some(())
        .and_then(|_| Duration::try_from_secs_f64(seconds).ok())
        .filter(|duration| !duration.is_zero())
}

fn discover(bin: &str, timeout: Duration, profile: &str) -> anyhow::Result<Discovery> {
    let cache_path = profile_cache_path(profile);
    if let Some(entry) = cache_path.as_deref().and_then(|path| load_cache(path, bin)) {
        if let Some(agents) = cached_agents(&entry.agents) {
            return Ok(Discovery {
                path: entry.config_path,
                exists: true,
                agents,
                agents_from_registry: entry.agents_from_registry,
                cached: true,
            });
        }
    }

    let (path, exists) = active_config_path(bin, timeout)?;
    if !exists {
        return Ok(Discovery {
            path,
            exists: false,
            agents: Vec::new(),
            agents_from_registry: false,
            cached: false,
        });
    }
    let (agents, from_registry) =
        match run_json(bin, timeout, &["config", "get", "agents.list", "--json"]) {
            Ok(Some(value)) => (agents_array(value, "config get agents.list --json")?, false),
            Ok(None) => (Vec::new(), false),
            Err(error) if missing_key(&error) => {
                match run_json(bin, timeout, &["agents", "list", "--json"]) {
                    Ok(Some(value)) => (agents_array(value, "agents list --json")?, true),
                    Ok(None) => (Vec::new(), true),
                    Err(error) if unknown_subcommand(&error) => (Vec::new(), false),
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
    if let Some(cache_path) = cache_path {
        let _ = store_cache(&cache_path, bin, &path, &agents, from_registry);
    }
    Ok(Discovery {
        path,
        exists: true,
        agents,
        agents_from_registry: from_registry,
        cached: false,
    })
}

fn agents_array(value: Value, command: &str) -> anyhow::Result<Vec<Value>> {
    value
        .as_array()
        .cloned()
        .ok_or_else(|| anyhow!("openclaw {command} must return a JSON array"))
}

fn cached_agents(value: &Value) -> Option<Vec<Value>> {
    if value.is_null() {
        Some(Vec::new())
    } else {
        value.as_array().cloned()
    }
}

fn active_config_path(bin: &str, timeout: Duration) -> anyhow::Result<(String, bool)> {
    match run_text(bin, timeout, &["config", "file"]) {
        Ok(output) => {
            let reported = output
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .ok_or_else(|| anyhow!("`openclaw config file` returned empty output"))?;
            let mut path = expand_home(reported)?;
            if !Path::new(&path).is_absolute() {
                path = std::env::current_dir()?.join(path).display().to_string();
            }
            return config_path_state(&path);
        }
        Err(error) if unsupported_config_file(&error) => fallback_config_path(),
        Err(error) => Err(error),
    }
}

fn fallback_config_path() -> anyhow::Result<(String, bool)> {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("OPENCLAW_CONFIG_PATH") {
        if !path.trim().is_empty() {
            let path = expand_home(path.trim())?;
            return config_path_state(&path);
        }
    }
    for key in ["CLAWDBOT_CONFIG_PATH"] {
        if let Ok(path) = std::env::var(key) {
            if !path.trim().is_empty() {
                candidates.push(expand_home(path.trim())?);
            }
        }
    }
    for key in ["OPENCLAW_STATE_DIR", "CLAWDBOT_STATE_DIR"] {
        if let Ok(dir) = std::env::var(key) {
            if !dir.trim().is_empty() {
                let dir = PathBuf::from(expand_home(dir.trim())?);
                for name in [
                    "openclaw.json",
                    "clawdbot.json",
                    "moltbot.json",
                    "moldbot.json",
                ] {
                    candidates.push(dir.join(name).display().to_string());
                }
            }
        }
    }
    let home = std::env::var("OPENCLAW_HOME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| expand_home(value.trim()))
        .transpose()?
        .or_else(|| process_user_home().map(|path| path.display().to_string()))
        .context("resolve openclaw home")?;
    for dir_name in [".openclaw", ".clawdbot", ".moltbot", ".moldbot"] {
        let dir = Path::new(&home).join(dir_name);
        for file_name in [
            "openclaw.json",
            "clawdbot.json",
            "moltbot.json",
            "moldbot.json",
        ] {
            candidates.push(dir.join(file_name).display().to_string());
        }
    }
    for path in candidates {
        let (path, exists) = config_path_state(&path)?;
        if exists {
            return Ok((path, true));
        }
    }
    Ok((
        Path::new(&home)
            .join(".openclaw")
            .join("openclaw.json")
            .display()
            .to_string(),
        false,
    ))
}

fn expand_home(path: &str) -> anyhow::Result<String> {
    if let Some(rest) = path
        .strip_prefix("$OPENCLAW_HOME")
        .or_else(|| path.strip_prefix("${OPENCLAW_HOME}"))
        .filter(|rest| rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\'))
    {
        let home = std::env::var("OPENCLAW_HOME")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .context("OPENCLAW_HOME is not set")?;
        let home = if home == "~" || home.starts_with("~/") || home.starts_with("~\\") {
            expand_tilde(&home)?
        } else {
            home
        };
        return Ok(if rest.is_empty() {
            home
        } else {
            Path::new(&home).join(&rest[1..]).display().to_string()
        });
    }
    if path == "~" || path.starts_with("~/") || path.starts_with("~\\") {
        return expand_tilde(path);
    }
    Ok(path.to_string())
}

fn expand_tilde(path: &str) -> anyhow::Result<String> {
    let home = process_user_home().context("user home is not set")?;
    if path == "~" {
        return Ok(home.display().to_string());
    }
    Ok(home.join(&path[2..]).display().to_string())
}

fn process_user_home() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

fn config_path_state(path: &str) -> anyhow::Result<(String, bool)> {
    let path = Path::new(path);
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            bail!(
                "openclaw config path {} is a directory, not a file",
                path.display()
            )
        }
        Ok(_) => Ok((path.display().to_string(), true)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok((path.display().to_string(), false))
        }
        Err(error) => Err(error.into()),
    }
}

fn managed_mcp(raw: Option<&Value>) -> anyhow::Result<(BTreeMap<String, Value>, bool)> {
    let Some(value) = raw else {
        return Ok((BTreeMap::new(), false));
    };
    if value.is_null() {
        return Ok((BTreeMap::new(), false));
    }
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("mcp_config must be a JSON object"))?;
    let servers = object
        .get("mcpServers")
        .map(|servers| {
            if servers.is_null() {
                Ok(BTreeMap::new())
            } else {
                servers
                    .as_object()
                    .cloned()
                    .map(|object| object.into_iter().collect::<BTreeMap<_, _>>())
                    .ok_or_else(|| anyhow!("mcpServers must be a JSON object"))
            }
        })
        .transpose()?
        .unwrap_or_default();
    let mut output = BTreeMap::new();
    for (name, entry) in servers {
        let object = entry
            .as_object()
            .ok_or_else(|| anyhow!("mcp_servers.{name} must be a JSON object"))?;
        let command = object.get("command").and_then(Value::as_str).unwrap_or("");
        let url = object.get("url").and_then(Value::as_str).unwrap_or("");
        if command.trim().is_empty() && url.trim().is_empty() {
            bail!("mcp_servers.{name} must declare either `command` or `url`");
        }
        output.insert(name, entry);
    }
    Ok((output, true))
}

fn strip_user_mcp(config: &mut Map<String, Value>) {
    let remove_parent = match config.get_mut("mcp") {
        Some(Value::Object(mcp)) => {
            mcp.remove("servers");
            mcp.is_empty()
        }
        _ => false,
    };
    if remove_parent {
        config.remove("mcp");
    }
}

// The arguments mirror the Go contract's independently sourced discovery and
// policy inputs; keeping them explicit prevents a secret-bearing config or
// Gateway pin from being hidden in an untyped aggregate.
#[allow(clippy::too_many_arguments)]
fn build_wrapper(
    active_path: &str,
    exists: bool,
    snapshot_path: Option<&str>,
    agents: &[Value],
    agents_from_registry: bool,
    work_dir: &str,
    managed_mcp: BTreeMap<String, Value>,
    has_managed_mcp: bool,
    gateway: &OpenclawGatewayPin,
) -> Value {
    let mut defaults = Map::new();
    defaults.insert("workspace".into(), Value::String(work_dir.to_string()));
    let mut agent_cfg = Map::new();
    agent_cfg.insert("defaults".into(), Value::Object(defaults));
    if !agents_from_registry && !agents.is_empty() {
        let list = agents
            .iter()
            .filter_map(|entry| {
                let mut object = entry.as_object()?.clone();
                object.insert("workspace".into(), Value::String(work_dir.to_string()));
                Some(Value::Object(object))
            })
            .collect::<Vec<_>>();
        if !list.is_empty() {
            agent_cfg.insert("list".into(), Value::Array(list));
        }
    }
    let mut root = Map::new();
    root.insert("agents".into(), Value::Object(agent_cfg));
    if has_managed_mcp {
        let servers = managed_mcp
            .into_iter()
            .map(|(name, value)| (name, value))
            .collect::<Map<_, _>>();
        root.insert(
            "mcp".into(),
            Value::Object(Map::from_iter([("servers".into(), Value::Object(servers))])),
        );
    }
    if let Some(gateway) = gateway_override(gateway) {
        root.insert("gateway".into(), gateway);
    }
    if let Some(path) = snapshot_path {
        root.insert(
            "$include".into(),
            Value::Array(vec![Value::String(path.into())]),
        );
    } else if exists {
        root.insert(
            "$include".into(),
            Value::Array(vec![Value::String(active_path.into())]),
        );
    }
    Value::Object(root)
}

fn gateway_override(pin: &OpenclawGatewayPin) -> Option<Value> {
    if pin.is_zero() {
        return None;
    }
    let mut value = Map::new();
    if !pin.host.is_empty() {
        value.insert("host".into(), Value::String(pin.host.clone()));
    }
    if pin.port != 0 {
        value.insert("port".into(), Value::Number(pin.port.into()));
    }
    if pin.tls {
        value.insert("tls".into(), Value::Bool(true));
    }
    if !pin.token.is_empty() {
        value.insert(
            "auth".into(),
            Value::Object(Map::from_iter([
                ("mode".into(), Value::String("token".into())),
                ("token".into(), Value::String(pin.token.clone())),
            ])),
        );
    }
    (!value.is_empty()).then_some(Value::Object(value))
}

fn run_json(bin: &str, timeout: Duration, args: &[&str]) -> anyhow::Result<Option<Value>> {
    let output = run_text(bin, timeout, args)?;
    let trimmed = output.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(None);
    }
    serde_json::from_str(trimmed)
        .map(Some)
        .with_context(|| format!("parse `{bin} {}` JSON output", args.join(" ")))
}

fn run_text(bin: &str, timeout: Duration, args: &[&str]) -> anyhow::Result<String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn openclaw {bin}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("openclaw stdout pipe unavailable"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("openclaw stderr pipe unavailable"))?;
    let (stdout_tx, stdout_rx) = mpsc::channel();
    let (stderr_tx, stderr_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        let _ = stdout_tx.send(bytes);
    });
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        let _ = stderr_tx.send(bytes);
    });
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            let stdout = stdout_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap_or_default();
            let stderr = stderr_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap_or_default();
            if status.success() {
                return String::from_utf8(stdout).context("openclaw stdout was not UTF-8");
            }
            let stderr = String::from_utf8_lossy(&stderr);
            let diagnostic = bounded_error(&stderr)
                .map(|value| format!(": {value}"))
                .unwrap_or_default();
            return Err(anyhow::Error::new(OpenclawCliError {
                message: format!("openclaw {} failed{diagnostic}", args.join(" ")),
                stdout,
            }));
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_rx.recv_timeout(Duration::from_millis(100));
            let _ = stderr_rx.recv_timeout(Duration::from_millis(100));
            return Err(anyhow::Error::new(ErrOpenclawCliTimeout).context(format!(
                "openclaw CLI timed out after {:?} while running `{}`",
                timeout,
                args.join(" ")
            )));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn bounded_error(value: &str) -> Option<String> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!value.is_empty()).then(|| value.chars().take(1024).collect())
}

fn missing_key(error: &anyhow::Error) -> bool {
    // JSON-mode OpenClaw errors are emitted on stdout. If there is a
    // structured envelope, require it to name the requested path before
    // accepting a missing-key fallback; an unrelated message such as
    // `OPENAI_API_KEY is not set` must remain a real preparation failure.
    for cause in error.chain() {
        if let Some(cli_error) = cause.downcast_ref::<OpenclawCliError>() {
            if let Some(message) = structured_error_message(&cli_error.stdout) {
                return message.to_ascii_lowercase().contains("agents.list")
                    && missing_key_message(&message);
            }
        }
    }
    missing_key_message(&error.to_string())
}

fn structured_error_message(stdout: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(stdout).ok()?;
    let message = value.get("error")?.as_str()?.trim();
    (!message.is_empty()).then(|| message.to_string())
}

fn missing_key_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    ["no value at", "not set", "missing key", "path not found"]
        .iter()
        .any(|needle| message.contains(needle))
}

fn unsupported_config_file(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("unknown command")
        || message.contains("too many arguments")
        || message.contains("unknown option")
}

fn unknown_subcommand(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    message.contains("unknown command")
        || message.contains("unknown option")
        || message.contains("does not recognize")
        || message.contains("unknown argument")
}

fn profile_cache_path(profile: &str) -> Option<String> {
    if profile.contains(['/', '\\']) || matches!(profile, "." | "..") {
        return None;
    }
    let root = std::env::var_os("CORDY_TASK_CONFIG_ROOT")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| process_user_home().map(|home| home.join(".cordy")))?;
    let dir = if profile.is_empty() {
        root
    } else {
        root.join("profiles").join(profile)
    };
    Some(dir.join(CACHE_FILE).display().to_string())
}

fn load_cache(path: &str, bin: &str) -> Option<DiscoveryCache> {
    let entry: DiscoveryCache = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    if entry.version != 1 || entry.config_path.is_empty() {
        return None;
    }
    let age = now_ns().checked_sub(entry.cached_at_ns)?;
    if age > CACHE_TTL.as_nanos() {
        return None;
    }
    let fingerprint = fingerprint(bin, &entry.config_path)?;
    (fingerprint == entry_fingerprint(&entry)).then_some(entry)
}

fn store_cache(
    path: &str,
    bin: &str,
    config: &str,
    agents: &[Value],
    from_registry: bool,
) -> anyhow::Result<()> {
    let fingerprint = fingerprint(bin, config)
        .ok_or_else(|| anyhow!("openclaw cache fingerprint unavailable"))?;
    let data = DiscoveryCache {
        version: 1,
        cached_at_ns: now_ns(),
        bin_path: fingerprint.0,
        bin_size: fingerprint.1,
        bin_mtime_ns: fingerprint.2,
        config_path: config.to_string(),
        config_size: fingerprint.3,
        config_mtime_ns: fingerprint.4,
        env: env_fingerprint(),
        agents: Value::Array(agents.to_vec()),
        agents_from_registry: from_registry,
    };
    atomic_json(Path::new(path), &serde_json::to_value(data)?, 0o600)
}

fn entry_fingerprint(entry: &DiscoveryCache) -> (String, u64, u128, u64, u128, String) {
    (
        entry.bin_path.clone(),
        entry.bin_size,
        entry.bin_mtime_ns,
        entry.config_size,
        entry.config_mtime_ns,
        entry.env.clone(),
    )
}

fn fingerprint(bin: &str, config: &str) -> Option<(String, u64, u128, u64, u128, String)> {
    let bin_path = resolve_bin(bin);
    let bin_meta = fs::metadata(&bin_path).ok()?;
    let cfg_meta = fs::metadata(config).ok()?;
    Some((
        bin_path,
        bin_meta.len(),
        modified_ns(&bin_meta),
        cfg_meta.len(),
        modified_ns(&cfg_meta),
        env_fingerprint(),
    ))
}

fn resolve_bin(bin: &str) -> String {
    if bin.contains('/') || bin.contains('\\') {
        return bin.to_string();
    }
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(bin))
                .find(|candidate| candidate.is_file())
        })
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| bin.to_string())
}

fn modified_ns(metadata: &fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or_default()
}

fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default()
}

fn env_fingerprint() -> String {
    [
        "OPENCLAW_CONFIG_PATH",
        "OPENCLAW_HOME",
        "OPENCLAW_STATE_DIR",
        "CLAWDBOT_CONFIG_PATH",
        "CLAWDBOT_STATE_DIR",
        "HOME",
        "USERPROFILE",
    ]
    .iter()
    .map(|key| format!("{key}={}", std::env::var(key).unwrap_or_default()))
    .collect::<Vec<_>>()
    .join("\0")
}

fn atomic_json(path: &Path, value: &Value, mode: u32) -> anyhow::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let data = serde_json::to_vec_pretty(value)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(mode))?;
    }
    temp.as_file_mut().write_all(&data)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| anyhow!(error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_mcp_distinguishes_absent_and_empty_and_validates_entries() {
        assert!(!managed_mcp(None).unwrap().1);
        assert!(!managed_mcp(Some(&Value::Null)).unwrap().1);
        assert!(managed_mcp(Some(&serde_json::json!([]))).is_err());
        assert!(managed_mcp(Some(&serde_json::json!({
            "mcpServers": []
        })))
        .is_err());
        assert!(
            managed_mcp(Some(&serde_json::json!({"mcpServers": {}})))
                .unwrap()
                .1
        );
        assert!(managed_mcp(Some(&serde_json::json!({
            "mcpServers": {"bad": {"name": "missing transport"}}
        })))
        .is_err());
    }

    #[test]
    fn wrapper_pins_workspace_all_agents_and_masks_no_secret_in_debug() {
        let pin = OpenclawGatewayPin {
            host: "gw".into(),
            port: 18789,
            token: "secret".into(),
            tls: true,
        };
        let cfg = build_wrapper(
            "/home/user/openclaw.json",
            true,
            None,
            &[serde_json::json!({"id": "main", "model": "x"})],
            false,
            "/task/workdir",
            BTreeMap::new(),
            false,
            &pin,
        );
        assert_eq!(cfg["agents"]["defaults"]["workspace"], "/task/workdir");
        assert_eq!(cfg["agents"]["list"][0]["workspace"], "/task/workdir");
        assert_eq!(cfg["gateway"]["auth"]["token"], "secret");
        assert!(pin.to_string().contains("***"));
        assert!(!pin.to_string().contains("secret"));
        let public: Value = serde_json::to_value(&pin).unwrap();
        assert_eq!(public["token"], "***");
    }

    #[test]
    fn timeout_override_is_bounded() {
        assert_eq!(
            resolve_timeout(Duration::from_secs(0)),
            Duration::from_secs(30)
        );
        assert_eq!(
            resolve_timeout(Duration::from_secs(3600)),
            Duration::from_secs(3600)
        );
        assert_eq!(
            resolve_timeout(Duration::from_millis(1)),
            Duration::from_millis(1)
        );
        assert_eq!(
            parse_timeout_override("500ms"),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            parse_timeout_override("1.5s"),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(parse_timeout_override("2m"), Some(Duration::from_secs(120)));
    }
}
