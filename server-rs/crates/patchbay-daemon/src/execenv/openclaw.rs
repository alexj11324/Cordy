//! OpenClaw task configuration capability.
//!
//! Discovery is delegated to the installed CLI, but a host configuration or
//! Gateway bearer is never copied or included in a task. Per-task config pins
//! every workspace to the task workdir and managed MCP is a strict
//! replacement. All credential-bearing host configurations fail closed until
//! OpenClaw is integrated with the provider credential broker; only a genuine
//! fresh install (no config file) receives a minimal wrapper.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::execenv::{OpenclawConfigPrep, OpenclawConfigResult, OpenclawGatewayPin};

const CONFIG_FILE: &str = "openclaw-config.json";
const LEGACY_SNAPSHOT_FILE: &str = "openclaw-user-snapshot.json";
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

/// Prepare the task wrapper and return the include root for the child env.
pub(crate) fn prepare_openclaw_config(
    env_root: &str,
    work_dir: &str,
    opts: &OpenclawConfigPrep,
) -> anyhow::Result<OpenclawConfigResult> {
    remove_legacy_task_credentials(env_root)?;
    let bin = if opts.openclaw_bin.trim().is_empty() {
        "openclaw".to_string()
    } else {
        opts.openclaw_bin.clone()
    };
    let timeout = resolve_timeout(opts.timeout);
    let discovery = discover(&bin, timeout, &opts.profile)?;
    anyhow::ensure!(
        !discovery.exists,
        "host OpenClaw configuration cannot be mounted into a task; a credential broker is required"
    );
    anyhow::ensure!(
        opts.gateway.token.is_empty(),
        "OpenClaw gateway bearer cannot be materialized in a task; a credential broker is required"
    );
    let (managed_mcp, has_managed_mcp) = managed_mcp(opts.mcp_config.as_ref())?;

    let config = build_wrapper(
        &discovery.agents,
        discovery.agents_from_registry,
        work_dir,
        managed_mcp,
        has_managed_mcp,
        &opts.gateway,
    );
    let out_path = Path::new(env_root).join(CONFIG_FILE);
    atomic_json(&out_path, &config, 0o600)?;
    tracing::info!(
        active_config = %discovery.path,
        active_config_exists = discovery.exists,
        discovery_cached = discovery.cached,
        managed_mcp = has_managed_mcp,
        "execenv: prepared openclaw config"
    );
    Ok(OpenclawConfigResult {
        config_path: out_path.display().to_string(),
        include_root: String::new(),
    })
}

fn remove_legacy_task_credentials(env_root: &str) -> anyhow::Result<()> {
    for name in [CONFIG_FILE, LEGACY_SNAPSHOT_FILE] {
        let path = Path::new(env_root).join(name);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "remove legacy OpenClaw task credential file {}",
                        path.display()
                    )
                });
            }
        }
    }
    Ok(())
}

fn resolve_timeout(explicit: Duration) -> Duration {
    if !explicit.is_zero() {
        return explicit.clamp(MIN_TIMEOUT, MAX_TIMEOUT);
    }
    let value = std::env::var("PATCHBAY_OPENCLAW_CLI_TIMEOUT").unwrap_or_default();
    let value = value.trim();
    let parsed = if value.is_empty() {
        None
    } else if let Ok(seconds) = value.parse::<u64>() {
        Some(Duration::from_secs(seconds))
    } else if let Some(raw) = value.strip_suffix('s') {
        raw.parse::<u64>().ok().map(Duration::from_secs)
    } else if let Some(raw) = value.strip_suffix('m') {
        raw.parse::<u64>()
            .ok()
            .and_then(|minutes| minutes.checked_mul(60))
            .map(Duration::from_secs)
    } else {
        None
    };
    parsed
        .unwrap_or(Duration::from_secs(30))
        .clamp(MIN_TIMEOUT, MAX_TIMEOUT)
}

fn discover(bin: &str, timeout: Duration, profile: &str) -> anyhow::Result<Discovery> {
    let cache_path = profile_cache_path(profile);
    if let Some(entry) = cache_path.as_deref().and_then(|path| load_cache(path, bin)) {
        let agents = entry.agents.as_array().cloned().unwrap_or_default();
        return Ok(Discovery {
            path: entry.config_path,
            exists: true,
            agents,
            agents_from_registry: entry.agents_from_registry,
            cached: true,
        });
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
            Ok(Some(value)) => (value.as_array().cloned().unwrap_or_default(), false),
            Ok(None) => (Vec::new(), false),
            Err(error) if missing_key(&error) => {
                match run_json(bin, timeout, &["agents", "list", "--json"]) {
                    Ok(Some(value)) => (value.as_array().cloned().unwrap_or_default(), true),
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
            Ok((path.clone(), Path::new(&path).is_file()))
        }
        Err(error) if unsupported_config_file(&error) => fallback_config_path(),
        Err(error) => Err(error),
    }
}

fn fallback_config_path() -> anyhow::Result<(String, bool)> {
    let mut candidates = Vec::new();
    for key in ["OPENCLAW_CONFIG_PATH", "CLAWDBOT_CONFIG_PATH"] {
        if let Ok(path) = std::env::var(key) {
            if !path.trim().is_empty() {
                candidates.push(expand_home(path.trim())?);
            }
        }
    }
    for key in ["OPENCLAW_HOME", "OPENCLAW_STATE_DIR", "CLAWDBOT_STATE_DIR"] {
        if let Ok(dir) = std::env::var(key) {
            if !dir.trim().is_empty() {
                candidates.push(
                    Path::new(&expand_home(dir.trim())?)
                        .join("openclaw.json")
                        .display()
                        .to_string(),
                );
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(
            Path::new(&home)
                .join(".openclaw/openclaw.json")
                .display()
                .to_string(),
        );
        candidates.push(
            Path::new(&home)
                .join(".clawdbot/clawdbot.json")
                .display()
                .to_string(),
        );
        candidates.push(
            Path::new(&home)
                .join(".moltbot/moltbot.json")
                .display()
                .to_string(),
        );
    }
    let path = candidates
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .unwrap_or_default();
    Ok((path.clone(), !path.is_empty()))
}

fn expand_home(path: &str) -> anyhow::Result<String> {
    if path == "~" || path.starts_with("~/") {
        let home = std::env::var("HOME").context("HOME is not set")?;
        return Ok(format!("{}{}", home, path.trim_start_matches('~')));
    }
    Ok(path.to_string())
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
                    .map(|servers| servers.clone().into_iter().collect())
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

// Keep the credential-free policy inputs explicit so a secret-bearing host
// config or Gateway pin cannot be hidden in an untyped aggregate.
fn build_wrapper(
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
        let servers = managed_mcp.into_iter().collect::<Map<_, _>>();
        root.insert(
            "mcp".into(),
            Value::Object(Map::from_iter([("servers".into(), Value::Object(servers))])),
        );
    }
    if let Some(gateway) = gateway_override(gateway) {
        root.insert("gateway".into(), gateway);
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
            let stdout = String::from_utf8_lossy(&stdout);
            let stderr = String::from_utf8_lossy(&stderr);
            let diagnostic = bounded_error(&stderr).or_else(|| bounded_error(&stdout));
            bail!(
                "openclaw {} failed{}",
                args.join(" "),
                diagnostic.map(|v| format!(": {v}")).unwrap_or_default()
            );
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_rx.recv_timeout(Duration::from_millis(100));
            let _ = stderr_rx.recv_timeout(Duration::from_millis(100));
            bail!(
                "openclaw {} timed out after {}s",
                args.join(" "),
                timeout.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn bounded_error(value: &str) -> Option<String> {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!value.is_empty()).then(|| value.chars().take(1024).collect())
}

fn missing_key(error: &anyhow::Error) -> bool {
    let message = error.to_string().to_ascii_lowercase();
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
    message.contains("unknown command") || message.contains("unknown option")
}

fn profile_cache_path(profile: &str) -> Option<String> {
    if profile.contains(['/', '\\']) || matches!(profile, "." | "..") {
        return None;
    }
    let root = std::env::var_os("PATCHBAY_TASK_CONFIG_ROOT")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".patchbay")))?;
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
    #[cfg(not(unix))]
    let _ = mode;
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
            &[serde_json::json!({"id": "main", "model": "x"})],
            false,
            "/task/workdir",
            BTreeMap::new(),
            false,
            &pin,
        );
        assert_eq!(cfg["agents"]["defaults"]["workspace"], "/task/workdir");
        assert_eq!(cfg["agents"]["list"][0]["workspace"], "/task/workdir");
        assert!(cfg["gateway"].get("auth").is_none());
        assert!(cfg.get("$include").is_none());
        assert!(pin.to_string().contains("***"));
        assert!(!pin.to_string().contains("secret"));
    }

    #[test]
    fn legacy_task_credentials_are_removed_before_preparation() {
        let root = tempfile::tempdir().unwrap();
        for name in [CONFIG_FILE, LEGACY_SNAPSHOT_FILE] {
            fs::write(root.path().join(name), "long-lived-secret").unwrap();
        }

        remove_legacy_task_credentials(root.path().to_str().unwrap()).unwrap();

        assert!(!root.path().join(CONFIG_FILE).exists());
        assert!(!root.path().join(LEGACY_SNAPSHOT_FILE).exists());
    }

    #[test]
    fn timeout_override_is_bounded() {
        assert_eq!(
            resolve_timeout(Duration::from_secs(0)),
            Duration::from_secs(30)
        );
        assert_eq!(resolve_timeout(Duration::from_secs(3600)), MAX_TIMEOUT);
        assert_eq!(resolve_timeout(Duration::from_millis(1)), MIN_TIMEOUT);
    }
}
