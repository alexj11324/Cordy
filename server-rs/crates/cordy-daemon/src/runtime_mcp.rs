//! Port of `server/internal/daemon/runtime_mcp.go` (547 lines).
//!
//! Symbol map (Go → Rust):
//! - `runtimeLocalMcpServerSummary` → [`RuntimeLocalMcpServerSummary`]
//! - `mergeRuntimeAndAgentMcpConfig` → [`merge_runtime_and_agent_mcp_config`]
//! - `codebuddyUserMcpConfigPath` → [`codebuddy_user_mcp_config_path`]
//! - `unmarshalRuntimeMcpConfig` / `stripJSONC` → [`unmarshal_runtime_mcp_config`] / [`strip_jsonc`]
//! - `loadRuntimeMcpServerConfigs` → [`load_runtime_mcp_server_configs`]
//! - `normalizeRuntimeMcpEntry` → [`normalize_runtime_mcp_entry`]
//! - `loadClaudePluginMcpServerConfigs` → [`load_claude_plugin_mcp_server_configs`]
//! - `listRuntimeLocalMcpServers` / `runtimeMcpSummaries` /
//!   `listClaudePluginMcpServers` / `nestedRuntimeMcpMap` / `runtimeMcpTransport`
//!   → same-named snake_case fns
//!
//! Port notes: Go's `json.Marshal(map[string]any)` emits keys sorted;
//! serde_json's default BTreeMap ordering matches. TOML configs convert
//! value-by-value. Claude plugin discovery is shared with the local-skills
//! production path through the canonical `claude_plugins` port.
//!
//! S9-integration: entry points are wired by the daemon-runner lane.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context as _};
use serde_json::{json, Map, Value};

use crate::claude_plugins::{
    claude_plugin_component_paths, list_enabled_claude_plugins, read_claude_plugin_manifest,
    ClaudePluginInstall,
};

/// The intentionally non-secret inventory shown in Agent capabilities
/// (go:19–24). Never add command arguments, URLs, headers, or environment
/// values here: this payload leaves the user's machine.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RuntimeLocalMcpServerSummary {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub transport: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    pub enabled: bool,
}

/// Builds the task-local MCP configuration used when an agent has MCP servers
/// managed by Cordy (go:36–79). Runtime servers form the base layer and the
/// agent's entries win on a same-name collision. A null agent config is
/// returned verbatim so the provider's native inheritance path stays intact;
/// a present config (even an empty mcpServers map) opts into the merged
/// document so adding one managed server no longer disables unrelated runtime
/// servers.
pub(crate) fn merge_runtime_and_agent_mcp_config(
    provider: &str,
    agent_config: &Value,
) -> anyhow::Result<Option<Value>> {
    merge_runtime_and_agent_mcp_config_in(&RuntimeMcpEnv::from_process()?, provider, agent_config)
}

pub(crate) fn merge_runtime_and_agent_mcp_config_in(
    env: &RuntimeMcpEnv,
    provider: &str,
    agent_config: &Value,
) -> anyhow::Result<Option<Value>> {
    if matches!(agent_config, Value::Null) {
        return Ok(Some(agent_config.clone()));
    }

    let (runtime_servers, supported) = load_runtime_mcp_server_configs_in(env, provider)?;
    if !supported {
        return Ok(Some(agent_config.clone()));
    }

    let agent_document = agent_config
        .as_object()
        .ok_or_else(|| anyhow!("parse agent MCP config: expected a JSON object"))?;
    let empty = Map::new();
    let agent_servers: &Map<String, Value> =
        match nested_runtime_mcp_map(agent_document, "mcpServers") {
            Some(Value::Object(servers)) => servers,
            _ if provider == "opencode" => {
                // Older OpenCode agents may store the provider-native
                // top-level `mcp` map (go:57–63); its entries flow through
                // the existing adapter under the canonical mcpServers
                // envelope.
                match nested_runtime_mcp_map(agent_document, "mcp") {
                    Some(Value::Object(servers)) => servers,
                    _ => &empty,
                }
            }
            _ => &empty,
        };

    let mut merged = Map::new();
    merged.extend(runtime_servers);
    for (name, entry) in agent_servers {
        merged.insert(name.clone(), entry.clone());
    }
    Ok(Some(json!({ "mcpServers": merged })))
}

/// The user-scope MCP config file CodeBuddy actually reads — the FIRST of the
/// fallback chain that exists, not a merge (go:97–113): `<configDir>/.mcp.json`
/// → `<configDir>/mcp.json` → `~/.codebuddy.json`, where configDir is
/// `$CODEBUDDY_CONFIG_DIR` (default `~/.codebuddy`). When none exist the first
/// candidate is returned so the caller's read fails as "no runtime servers".
pub(crate) fn codebuddy_user_mcp_config_path(home: &Path) -> PathBuf {
    let dir = std::env::var("CODEBUDDY_CONFIG_DIR").unwrap_or_default();
    codebuddy_user_mcp_config_path_in(
        home,
        Some(Path::new(dir.trim())).filter(|p| !p.as_os_str().is_empty()),
    )
}

pub(crate) fn codebuddy_user_mcp_config_path_in(home: &Path, config_dir: Option<&Path>) -> PathBuf {
    let config_dir = config_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| home.join(".codebuddy"));
    let candidates = [
        config_dir.join(".mcp.json"),
        config_dir.join("mcp.json"),
        home.join(".codebuddy.json"),
    ];
    candidates
        .into_iter()
        .find(|candidate| {
            std::fs::metadata(candidate)
                .map(|info| !info.is_dir())
                .unwrap_or(false)
        })
        .unwrap_or_else(|| home.join(".codebuddy").join(".mcp.json"))
}

/// Decodes one runtime's config file (go:118–139). "jsonc" tolerates comments
/// and trailing commas — parsing those as strict JSON would drop every server
/// behind a single `//`.
fn unmarshal_runtime_mcp_config(raw: &[u8], format: &str) -> anyhow::Result<Map<String, Value>> {
    let parse_err = |err: String| anyhow!("parse runtime MCP config: {err}");
    match format {
        "toml" => {
            let text = std::str::from_utf8(raw).map_err(|err| parse_err(err.to_string()))?;
            let value: toml::Value =
                toml::from_str(text).map_err(|err| parse_err(err.to_string()))?;
            toml_value_to_json(&value)
                .as_object()
                .cloned()
                .ok_or_else(|| parse_err("expected a table at the top level".into()))
        }
        "jsonc" => {
            let stripped = strip_jsonc(raw).map_err(|err| parse_err(err.to_string()))?;
            serde_json::from_slice(&stripped).map_err(|err| parse_err(err.to_string()))
        }
        _ => serde_json::from_slice(raw).map_err(|err| parse_err(err.to_string())),
    }
}

fn toml_value_to_json(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => json!(i),
        toml::Value::Float(f) => json!(f),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
        toml::Value::Array(items) => Value::Array(items.iter().map(toml_value_to_json).collect()),
        toml::Value::Table(table) => {
            let mut out = Map::new();
            for (key, item) in table {
                out.insert(key.clone(), toml_value_to_json(item));
            }
            Value::Object(out)
        }
    }
}

/// Rewrites JSONC into strict JSON at constant length (go:161–240): `//` and
/// `/* */` comments become spaces and only a genuinely trailing comma before
/// `}` / `]` is blanked. String literals survive verbatim, so a `//` inside a
/// command argument is untouched. Output length always equals input length so
/// a parse-error offset still points at the byte the user wrote. An
/// unterminated `/*` is an error rather than blank-to-EOF, and `/*/` cannot
/// reuse its opener's `*` as the closer's — a file CodeBuddy itself rejects
/// must never yield an inventory.
pub(crate) fn strip_jsonc(raw: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut out: Vec<u8> = Vec::with_capacity(raw.len());
    // Offset into out of the one comma still eligible for removal, or -1;
    // reset by any value token, so only a genuinely trailing comma is dropped.
    let mut last_comma: isize = -1;
    let mut in_string = false;
    let mut escaped = false;

    let mut i = 0usize;
    while i < raw.len() {
        let c = raw[i];

        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        if c == b'"' {
            in_string = true;
            last_comma = -1;
            out.push(c);
        } else if c == b'/' && i + 1 < raw.len() && raw[i + 1] == b'/' {
            // Blank through end of line, preserving the newline itself.
            while i < raw.len() && raw[i] != b'\n' {
                out.push(b' ');
                i += 1;
            }
            if i < raw.len() {
                out.push(raw[i]);
            }
        } else if c == b'/' && i + 1 < raw.len() && raw[i + 1] == b'*' {
            out.push(b' ');
            out.push(b' ');
            i += 2;
            let mut closed = false;
            while i < raw.len() {
                if raw[i] == b'*' && i + 1 < raw.len() && raw[i + 1] == b'/' {
                    out.push(b' ');
                    out.push(b' ');
                    i += 1;
                    closed = true;
                    break;
                }
                out.push(if raw[i] == b'\n' { b'\n' } else { b' ' });
                i += 1;
            }
            if !closed {
                return Err("unterminated block comment");
            }
        } else if c == b',' {
            last_comma = out.len() as isize;
            out.push(c);
        } else if c == b'}' || c == b']' {
            if last_comma >= 0 {
                out[last_comma as usize] = b' ';
            }
            last_comma = -1;
            out.push(c);
        } else if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
            out.push(c);
        } else {
            last_comma = -1;
            out.push(c);
        }
        i += 1;
    }
    Ok(out)
}

fn user_home_dir() -> anyhow::Result<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("resolve user home"))
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("USERPROFILE")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("resolve user home"))
    }
}

/// The environment inputs the runtime MCP loaders read. Production resolves
/// these from the process environment once per call
/// ([`RuntimeMcpEnv::from_process`]); tests pass tempdir values directly so
/// they never race sibling tests that mutate process env.
#[derive(Debug, Clone, Default)]
pub(crate) struct RuntimeMcpEnv {
    pub home: PathBuf,
    pub codex_home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub kimi_code_home: Option<PathBuf>,
    pub codebuddy_config_dir: Option<PathBuf>,
    pub clawdbot_config_path: Option<PathBuf>,
    pub openclaw_state_dir: Option<PathBuf>,
}

impl RuntimeMcpEnv {
    pub(crate) fn from_process() -> anyhow::Result<Self> {
        let read = |key: &str| -> Option<PathBuf> {
            std::env::var(key)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Ok(Self {
            home: user_home_dir()?,
            codex_home: read("CODEX_HOME"),
            xdg_config_home: read("XDG_CONFIG_HOME"),
            kimi_code_home: read("KIMI_CODE_HOME"),
            codebuddy_config_dir: read("CODEBUDDY_CONFIG_DIR"),
            clawdbot_config_path: read("CLAWDBOT_CONFIG_PATH"),
            openclaw_state_dir: read("OPENCLAW_STATE_DIR"),
        })
    }

    fn or(key_dir: &Option<PathBuf>, fallback: PathBuf) -> PathBuf {
        key_dir.clone().unwrap_or(fallback)
    }

    fn codex_home(&self) -> PathBuf {
        Self::or(&self.codex_home, self.home.join(".codex"))
    }

    fn opencode_config_home(&self) -> PathBuf {
        Self::or(&self.xdg_config_home, self.home.join(".config"))
    }

    fn kimi_home(&self) -> PathBuf {
        Self::or(&self.kimi_code_home, self.home.join(".kimi-code"))
    }

    fn openclaw_path(&self) -> PathBuf {
        if let Some(configured) = &self.clawdbot_config_path {
            return configured.clone();
        }
        let state_dir = Self::or(&self.openclaw_state_dir, self.home.join(".openclaw"));
        state_dir.join("openclaw.json")
    }
}

/// Full, secret-bearing runtime MCP entries for task-local merging
/// (go:246–316). Callers must never send the result to the server or logs;
/// the capabilities endpoint uses [`list_runtime_local_mcp_servers`]. The
/// bool reports whether the provider is supported. codebuddy is deliberately
/// absent from this table (go:256–261): it merges scopes natively on launch,
/// and pre-merging would lose scope precedence and the approval gate.
pub(crate) fn load_runtime_mcp_server_configs(
    provider: &str,
) -> anyhow::Result<(Map<String, Value>, bool)> {
    load_runtime_mcp_server_configs_in(&RuntimeMcpEnv::from_process()?, provider)
}

pub(crate) fn load_runtime_mcp_server_configs_in(
    env: &RuntimeMcpEnv,
    provider: &str,
) -> anyhow::Result<(Map<String, Value>, bool)> {
    let home = &env.home;
    type Spec = (PathBuf, &'static str, &'static str);
    let spec: Option<Spec> = match provider {
        "claude" => Some((home.join(".claude.json"), "mcpServers", "json")),
        "codex" => Some((env.codex_home().join("config.toml"), "mcp_servers", "toml")),
        "cursor" => Some((home.join(".cursor").join("mcp.json"), "mcpServers", "json")),
        "opencode" => Some((
            env.opencode_config_home()
                .join("opencode")
                .join("opencode.json"),
            "mcp",
            "json",
        )),
        "openclaw" => Some((env.openclaw_path(), "mcp.servers", "json")),
        _ => None,
    };
    let Some((path, key, format)) = spec else {
        return Ok((Map::new(), false));
    };

    let mut servers = Map::new();
    match std::fs::read(&path) {
        Ok(raw) => {
            let cfg = unmarshal_runtime_mcp_config(&raw, format)?;
            if let Some(Value::Object(configured)) = nested_runtime_mcp_map(&cfg, key) {
                for (name, entry) in configured {
                    servers.insert(
                        name.clone(),
                        normalize_runtime_mcp_entry(provider, entry.clone()),
                    );
                }
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(anyhow!("read runtime MCP config: read {path:?}: {err}"))
                .context("read runtime MCP config");
        }
    }

    if provider == "claude" {
        // Same precedence Claude uses: plugin servers only fill names not
        // already defined by the user (go:306–314).
        for (name, entry) in load_claude_plugin_mcp_server_configs(home) {
            servers.entry(name).or_insert(entry);
        }
    }
    Ok((servers, true))
}

/// Cordy's canonical remote shape calls these `headers`; Codex stores them as
/// `http_headers`. Keep the original key too so less common Codex-specific
/// settings round-trip (go:318–337).
fn normalize_runtime_mcp_entry(provider: &str, value: Value) -> Value {
    let Some(entry) = value.as_object() else {
        return value;
    };
    if provider != "codex" {
        return Value::Object(entry.clone());
    }
    let mut entry = entry.clone();
    if let Some(headers) = entry.get("http_headers").cloned() {
        entry.entry("headers".to_string()).or_insert(headers);
    }
    if entry.contains_key("url") && !entry.contains_key("type") {
        entry.insert("type".to_string(), Value::String("http".to_string()));
    }
    Value::Object(entry)
}

/// Redacted inventory for Agent capabilities (go:371–456), deduped with user
/// configuration winning on a same-name collision and sorted
/// case-insensitively by name.
pub(crate) fn list_runtime_local_mcp_servers(
    provider: &str,
) -> anyhow::Result<(Vec<RuntimeLocalMcpServerSummary>, bool)> {
    list_runtime_local_mcp_servers_in(&RuntimeMcpEnv::from_process()?, provider)
}

pub(crate) fn list_runtime_local_mcp_servers_in(
    env: &RuntimeMcpEnv,
    provider: &str,
) -> anyhow::Result<(Vec<RuntimeLocalMcpServerSummary>, bool)> {
    let home = &env.home;
    type Spec = (PathBuf, &'static str, &'static str, &'static str);
    let spec: Option<Spec> = match provider {
        "claude" => Some((
            home.join(".claude.json"),
            "mcpServers",
            "User config",
            "json",
        )),
        "codebuddy" => Some((
            codebuddy_user_mcp_config_path_in(home, env.codebuddy_config_dir.as_deref()),
            "mcpServers",
            "User config",
            "jsonc",
        )),
        // Inventory only — kimi is deliberately absent from
        // load_runtime_mcp_server_configs (go:384–393): `kimi acp` merges this
        // file with the ephemeral mcpServers sent in session/new, so merging
        // it in again would spawn every user server twice.
        "kimi" => Some((
            env.kimi_home().join("mcp.json"),
            "mcpServers",
            "User config",
            "json",
        )),
        "codex" => Some((
            env.codex_home().join("config.toml"),
            "mcp_servers",
            "User config",
            "toml",
        )),
        "cursor" => Some((
            home.join(".cursor").join("mcp.json"),
            "mcpServers",
            "User config",
            "json",
        )),
        "opencode" => Some((
            env.opencode_config_home()
                .join("opencode")
                .join("opencode.json"),
            "mcp",
            "User config",
            "json",
        )),
        "openclaw" => Some((env.openclaw_path(), "mcp.servers", "User config", "json")),
        _ => None,
    };

    let mut out: Vec<RuntimeLocalMcpServerSummary> = Vec::new();
    let mut supported = false;
    if let Some((path, key, source, format)) = spec {
        supported = true;
        match std::fs::read(&path) {
            Ok(raw) => {
                let cfg = unmarshal_runtime_mcp_config(&raw, format)?;
                if let Some(Value::Object(servers)) = nested_runtime_mcp_map(&cfg, key) {
                    out.extend(runtime_mcp_summaries(servers, source));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(anyhow!("read runtime MCP config: read {path:?}: {err}"));
            }
        }
    }

    if provider == "claude" {
        out.extend(list_claude_plugin_mcp_servers(home));
    }

    let mut seen = std::collections::BTreeSet::new();
    out.retain(|server| seen.insert(server.name.clone()));
    out.sort_by_key(|server| server.name.to_lowercase());
    Ok((out, supported))
}

fn runtime_mcp_summaries(
    servers: &Map<String, Value>,
    source: &str,
) -> Vec<RuntimeLocalMcpServerSummary> {
    let mut out = Vec::with_capacity(servers.len());
    for (name, value) in servers {
        let Some(entry) = value.as_object() else {
            continue;
        };
        if name.trim().is_empty() {
            continue;
        }
        let mut enabled = entry
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if entry
            .get("disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            enabled = false;
        }
        out.push(RuntimeLocalMcpServerSummary {
            name: name.clone(),
            transport: runtime_mcp_transport(entry),
            source: source.to_string(),
            enabled,
        });
    }
    out
}

fn list_claude_plugin_mcp_servers(home: &Path) -> Vec<RuntimeLocalMcpServerSummary> {
    let mut out = Vec::new();
    for plugin in list_enabled_claude_plugins(home) {
        let paths = claude_plugin_mcp_paths(&plugin);
        for path in paths {
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(cfg) = serde_json::from_str::<Map<String, Value>>(&raw) else {
                continue;
            };
            if let Some(Value::Object(servers)) = nested_runtime_mcp_map(&cfg, "mcpServers") {
                out.extend(runtime_mcp_summaries(
                    servers,
                    &format!("Claude Plugin · {}", plugin.name),
                ));
            }
        }
    }
    out
}

/// Dotted-path lookup ("mcp.servers") where every segment must be an object
/// (go:510–528).
pub(crate) fn nested_runtime_mcp_map<'a>(
    cfg: &'a Map<String, Value>,
    path: &str,
) -> Option<&'a Value> {
    let mut current: &Value = cfg.get(path.split('.').next()?)?;
    for part in path.split('.').skip(1) {
        current = current.as_object()?.get(part)?;
    }
    Some(current)
}

/// Transport classification for the summary inventory (go:530–547).
fn load_claude_plugin_mcp_server_configs(home: &Path) -> Map<String, Value> {
    let mut out = Map::new();
    for plugin in list_enabled_claude_plugins(home) {
        let paths = claude_plugin_mcp_paths(&plugin);
        for path in paths {
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(cfg) = serde_json::from_str::<Map<String, Value>>(&raw) else {
                continue;
            };
            if let Some(Value::Object(servers)) = nested_runtime_mcp_map(&cfg, "mcpServers") {
                for (name, entry) in servers {
                    out.entry(name.clone()).or_insert_with(|| entry.clone());
                }
            }
        }
    }
    out
}

fn claude_plugin_mcp_paths(plugin: &ClaudePluginInstall) -> Vec<std::path::PathBuf> {
    let raw = read_claude_plugin_manifest(&plugin.install_path)
        .map(|manifest| manifest.mcp_servers_value().clone())
        .unwrap_or(Value::Null);
    let defaults = [plugin.install_path.join(".mcp.json")];
    claude_plugin_component_paths(&plugin.install_path, &raw, &defaults)
}

pub(crate) fn runtime_mcp_transport(entry: &Map<String, Value>) -> String {
    let kind = entry.get("type").and_then(Value::as_str).unwrap_or("");
    match kind.to_lowercase().as_str() {
        "local" | "stdio" => return "stdio".to_string(),
        "remote" | "http" | "streamable-http" => return "http".to_string(),
        "sse" => return "sse".to_string(),
        _ => {}
    }
    if entry.contains_key("command") {
        return "stdio".to_string();
    }
    if entry.contains_key("url") {
        return "http".to_string();
    }
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_env(home: &Path) -> RuntimeMcpEnv {
        RuntimeMcpEnv {
            home: home.to_path_buf(),
            ..Default::default()
        }
    }

    #[test]
    fn strip_jsonc_handles_comments_and_trailing_commas() {
        let cases: Vec<(&str, &str, Value)> = vec![
            ("line comment", "{\n// hi\n\"a\":1\n}", json!({"a": 1})),
            ("block comment", "{/* hi */\"a\":1}", json!({"a": 1})),
            ("trailing comma object", r#"{"a":1,}"#, json!({"a": 1})),
            (
                "trailing comma nested",
                r#"{"a":{"b":1,},}"#,
                json!({"a": {"b": 1}}),
            ),
            (
                "comment-like text inside a string is preserved",
                r#"{"a":"http://x//y, /* not a comment */"}"#,
                json!({"a": "http://x//y, /* not a comment */"}),
            ),
            (
                "escaped quote in string",
                r#"{"a":"say \"//\" ok"}"#,
                json!({"a": "say \"//\" ok"}),
            ),
        ];
        for (name, input, want) in cases {
            let stripped = strip_jsonc(input.as_bytes()).unwrap_or_else(|e| panic!("{name}: {e}"));
            let got: Value = serde_json::from_slice(&stripped)
                .unwrap_or_else(|e| panic!("{name}: unmarshal {stripped:?}: {e}"));
            assert_eq!(got, want, "case {name}");
        }
    }

    #[test]
    fn strip_jsonc_keeps_separator_commas() {
        let stripped = strip_jsonc(br#"["a","b",]"#).unwrap();
        let got: Value = serde_json::from_slice(&stripped).unwrap();
        assert_eq!(got, json!(["a", "b"]));
    }

    #[test]
    fn strip_jsonc_leaves_malformed_input_invalid() {
        for input in ["[1,,,]", r#"{"a":1,,}"#, "[1,,2]"] {
            if let Ok(stripped) = strip_jsonc(input.as_bytes()) {
                let parsed: Result<Value, _> = serde_json::from_slice(&stripped);
                assert!(parsed.is_err(), "{input} must stay invalid");
            }
        }
    }

    #[test]
    fn strip_jsonc_preserves_byte_length() {
        for input in [
            "{\n  // c\n  \"a\": 1,\n}\n",
            "{/* block */\"a\":1,}",
            r#"{"a":"// not a comment"}"#,
            "{/**/\"a\":1}",
            "{}",
        ] {
            let stripped = strip_jsonc(input.as_bytes()).unwrap_or_else(|e| panic!("{input}: {e}"));
            assert_eq!(stripped.len(), input.len(), "{input}");
        }
    }

    #[test]
    fn strip_jsonc_rejects_unterminated_block_comment() {
        for input in [
            r#"{"mcpServers":{}} /* oops"#,
            "/*/ {\"mcpServers\":{}}",
            "/*",
        ] {
            assert!(strip_jsonc(input.as_bytes()).is_err(), "{input}");
        }
    }

    #[test]
    fn merge_claude_combines_and_agent_wins() {
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join(".claude.json"),
            r#"{"mcpServers":{"runtime-only":{"command":"runtime-cmd","env":{"TOKEN":"local-secret"}},"shared":{"command":"runtime-shared"}}}"#,
        )
        .unwrap();

        let merged = merge_runtime_and_agent_mcp_config_in(
            &test_env(home.path()),
            "claude",
            &json!({"mcpServers":{"shared":{"command":"agent-shared"},"agent-only":{"url":"https://agent.example/mcp"}}}),
        )
        .unwrap()
        .unwrap();
        let servers = merged["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 3, "{servers:?}");
        assert_eq!(servers["shared"]["command"], "agent-shared");
        assert_eq!(servers["runtime-only"]["command"], "runtime-cmd");
        assert_eq!(servers["runtime-only"]["env"]["TOKEN"], "local-secret");
    }

    #[test]
    fn merge_codex_normalizes_headers() {
        let home = tempfile::tempdir().unwrap();
        let config_dir = home.path().join(".codex");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("config.toml"),
            "[mcp_servers.docs]\nurl = \"https://runtime.example/mcp\"\nhttp_headers = { Authorization = \"Bearer local-secret\" }\n\n[mcp_servers.fetch]\ncommand = \"uvx\"\nargs = [\"mcp-server-fetch\"]\n",
        )
        .unwrap();

        let merged = merge_runtime_and_agent_mcp_config_in(
            &test_env(home.path()),
            "codex",
            &json!({"mcpServers":{"agent":{"command":"node","args":["agent.js"]}}}),
        )
        .unwrap()
        .unwrap();
        let servers = merged["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 3, "{servers:?}");
        assert_eq!(servers["docs"]["type"], "http");
        assert_eq!(
            servers["docs"]["headers"]["Authorization"],
            "Bearer local-secret"
        );
    }

    #[test]
    fn merge_null_keeps_native_inheritance() {
        // Go's nil / "" / " null" RawMessage forms all arrive as Value::Null
        // in the daemon-side Task model.
        let raw = Value::Null;
        let merged =
            merge_runtime_and_agent_mcp_config_in(&test_env(Path::new("/tmp")), "claude", &raw)
                .unwrap();
        assert_eq!(merged.unwrap(), raw);
    }

    #[test]
    fn codebuddy_is_passthrough_and_kimi_too() {
        let agent_cfg = &json!({"mcpServers":{"agent":{"command":"node"}}});
        for provider in ["codebuddy", "kimi"] {
            let merged = merge_runtime_and_agent_mcp_config_in(
                &test_env(Path::new("/tmp")),
                provider,
                agent_cfg,
            )
            .unwrap()
            .unwrap();
            assert_eq!(&merged, agent_cfg, "{provider}");
        }
    }

    #[test]
    fn unknown_provider_passthrough() {
        let agent_cfg = &json!({"mcpServers":{"agent":{"command":"node"}}});
        let merged =
            merge_runtime_and_agent_mcp_config_in(&test_env(Path::new("/tmp")), "dsh", agent_cfg)
                .unwrap()
                .unwrap();
        assert_eq!(&merged, agent_cfg);
    }

    #[test]
    fn opencode_legacy_top_level_mcp_container_survives() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".config").join("opencode");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("opencode.json"),
            r#"{"mcp":{"runtime-local":{"command":"runtime-server"}}}"#,
        )
        .unwrap();
        let env = test_env(home.path());

        // Agent with a bound server under mcpServers keeps its legacy entries.
        let merged = merge_runtime_and_agent_mcp_config_in(
            &env,
            "opencode",
            &json!({"mcpServers":{"shared":{"url":"https://shared.example"}}}),
        )
        .unwrap()
        .unwrap();
        let servers = merged["mcpServers"].as_object().unwrap();
        for name in ["runtime-local", "shared"] {
            assert!(servers.contains_key(name), "{servers:?}");
        }
        assert_eq!(servers.len(), 2);

        // An agent with only the legacy container keeps it too.
        let merged = merge_runtime_and_agent_mcp_config_in(
            &env,
            "opencode",
            &json!({"mcp":{"private":{"command":"private-server"}}}),
        )
        .unwrap()
        .unwrap();
        let servers = merged["mcpServers"].as_object().unwrap();
        assert!(servers.contains_key("private") && servers.contains_key("runtime-local"));
    }

    #[test]
    fn codebuddy_user_mcp_config_path_precedence() {
        let home = tempfile::tempdir().unwrap();
        let config_dir = home.path().join(".codebuddy");
        fs::create_dir_all(&config_dir).unwrap();

        assert_eq!(
            codebuddy_user_mcp_config_path_in(home.path(), None),
            config_dir.join(".mcp.json"),
            "none exist → first candidate"
        );
        fs::write(config_dir.join("mcp.json"), "{}").unwrap();
        assert_eq!(
            codebuddy_user_mcp_config_path_in(home.path(), None),
            config_dir.join("mcp.json")
        );
        fs::write(config_dir.join(".mcp.json"), "{}").unwrap();
        assert_eq!(
            codebuddy_user_mcp_config_path_in(home.path(), None),
            config_dir.join(".mcp.json"),
            "first of the chain wins, not a merge"
        );
    }

    #[test]
    fn codebuddy_user_mcp_config_path_honors_config_dir_env() {
        let home = tempfile::tempdir().unwrap();
        let custom = tempfile::tempdir().unwrap();
        fs::write(custom.path().join("mcp.json"), "{}").unwrap();
        assert_eq!(
            codebuddy_user_mcp_config_path_in(home.path(), Some(custom.path())),
            custom.path().join("mcp.json")
        );
    }

    #[test]
    fn list_codebuddy_reads_its_own_config_as_jsonc() {
        let home = tempfile::tempdir().unwrap();
        let config_dir = home.path().join(".codebuddy");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join(".mcp.json"),
            "// user comment\n{\"mcpServers\":{\"cb\":{\"type\":\"stdio\",\"command\":\"cb\",\"disabled\":true}}}",
        )
        .unwrap();
        let (servers, supported) =
            list_runtime_local_mcp_servers_in(&test_env(home.path()), "codebuddy").unwrap();
        assert!(supported);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "cb");
        assert_eq!(servers[0].transport, "stdio");
        assert!(!servers[0].enabled);
        assert_eq!(servers[0].source, "User config");
    }

    #[test]
    fn list_codebuddy_rejects_unterminated_comment() {
        let home = tempfile::tempdir().unwrap();
        let config_dir = home.path().join(".codebuddy");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(config_dir.join(".mcp.json"), "{\"mcpServers\":{}} /* oops").unwrap();
        assert!(list_runtime_local_mcp_servers_in(&test_env(home.path()), "codebuddy").is_err());
    }

    #[test]
    fn unknown_provider_inventory_is_empty_and_unsupported() {
        let (servers, supported) =
            list_runtime_local_mcp_servers_in(&test_env(Path::new("/tmp")), "dsh").unwrap();
        assert!(servers.is_empty());
        assert!(!supported);
    }

    #[test]
    fn list_kimi_inventory_only() {
        let home = tempfile::tempdir().unwrap();
        let kimi_home = home.path().join(".kimi-code");
        fs::create_dir_all(&kimi_home).unwrap();
        fs::write(
            kimi_home.join("mcp.json"),
            r#"{"mcpServers":{"k":{"type":"http","url":"https://k.example","enabled":false}}}"#,
        )
        .unwrap();
        let mut env = test_env(home.path());
        env.kimi_code_home = Some(kimi_home);
        let (servers, _) = list_runtime_local_mcp_servers_in(&env, "kimi").unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].transport, "http");
        assert!(!servers[0].enabled);
    }
}
