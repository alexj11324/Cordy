//! Port of `server/internal/daemon/runtime_mcp.go` (lines 1–547).
//!
//! Task-local MCP configuration: merges the runtime's own MCP servers with
//! the agent's managed entries inside the local daemon so runtime URLs,
//! headers, commands, and env values never need to leave the machine, plus
//! the intentionally non-secret inventory shown in Agent capabilities.
//!
//! Deviations from Go:
//! - `listEnabledClaudePlugins` / `readClaudePluginManifest` /
//!   `claudePluginComponentPaths` (claude_plugins.go) belong to lane C2:
//!   fail-closed S9-integration seams here return no plugins until that file
//!   lands.
//! - `go-toml/v2` → the workspace `toml` crate decoding straight into
//!   `serde_json::Value`; `map[string]any` → `serde_json::Value` objects.
//! - Env-dependent tests serialize on a mutex instead of `t.Setenv`
//!   (process-global env is not test-scoped in Rust).

// S9-integration: dead_code until Daemon core wires this.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context as _;
use serde::Serialize;
use serde_json::{json, Map, Value};

/// `runtimeLocalMcpServerSummary` (runtime_mcp.go:19–24): the intentionally
/// non-secret inventory shown in Agent capabilities. Never add command
/// arguments, URLs, headers, or environment values here: this payload leaves
/// the user's machine.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct RuntimeLocalMcpServerSummary {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "transport", skip_serializing_if = "String::is_empty")]
    pub transport: String,
    #[serde(rename = "source", skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(rename = "enabled")]
    pub enabled: bool,
}

/// `mergeRuntimeAndAgentMcpConfig` (runtime_mcp.go:26–79): builds the
/// task-local MCP configuration used when an agent has MCP servers managed by
/// Cordy. Runtime servers are the base layer and the agent's entries win on a
/// same-name collision.
///
/// A nil/null agent config keeps the provider's native inheritance path
/// intact. A present config (including an empty mcpServers map) opts into the
/// merged, task-local config so adding one managed server no longer disables
/// unrelated runtime servers.
pub(crate) fn merge_runtime_and_agent_mcp_config(
    provider: &str,
    agent_config: &[u8],
) -> anyhow::Result<Vec<u8>> {
    let trimmed = trim_ascii(agent_config);
    if trimmed.is_empty() || trimmed == b"null" {
        return Ok(agent_config.to_vec());
    }

    let (runtime_servers, supported) = match load_runtime_mcp_server_configs(provider) {
        Ok(result) => result,
        Err(err) => return Err(err),
    };
    if !supported {
        return Ok(agent_config.to_vec());
    }

    let agent_document: Value = serde_json::from_slice(trimmed)
        .context("parse agent MCP config")?;
    let mut agent_servers: HashMap<String, Value> = HashMap::new();
    if let Some(servers) = nested_runtime_mcp_map(&agent_document, "mcpServers") {
        for (name, entry) in servers {
            agent_servers.insert(name.clone(), entry.clone());
        }
    } else if provider == "opencode" {
        // Older OpenCode agents may store the provider-native top-level `mcp`
        // map. Its individual entries can still flow through the existing
        // OpenCode adapter when placed under the canonical mcpServers envelope.
        if let Some(servers) = nested_runtime_mcp_map(&agent_document, "mcp") {
            for (name, entry) in servers {
                agent_servers.insert(name.clone(), entry.clone());
            }
        }
    }

    let mut merged = Map::new();
    for (name, entry) in &runtime_servers {
        merged.insert(name.clone(), entry.clone());
    }
    for (name, entry) in &agent_servers {
        merged.insert(name.clone(), entry.clone());
    }

    let raw = json!({"mcpServers": merged});
    serde_json::to_vec(&raw).context("marshal merged MCP config")
}

/// `codebuddyUserMcpConfigPath` (runtime_mcp.go:81–113): returns the
/// user-scope MCP config file CodeBuddy actually reads. The candidate list is
/// a fallback chain, not a merge; when none exist the first candidate is
/// returned so the caller's read fails with NotFound and is treated as "no
/// runtime servers", matching the other providers. Verified against CodeBuddy
/// 2.x by writing each file in turn and reading back `codebuddy mcp list`.
pub(crate) fn codebuddy_user_mcp_config_path(home: &str) -> String {
    codebuddy_user_mcp_config_path_in(
        home,
        std::env::var("CODEBUDDY_CONFIG_DIR").ok().as_deref().map(str::trim),
    )
}

fn codebuddy_user_mcp_config_path_in(home: &str, config_dir_env: Option<&str>) -> String {
    let config_dir = match config_dir_env {
        Some(dir) if !dir.is_empty() => dir.to_string(),
        _ => Path::new(home).join(".codebuddy").to_string_lossy().to_string(),
    };
    let candidates = [
        Path::new(&config_dir).join(".mcp.json"),
        Path::new(&config_dir).join("mcp.json"),
        Path::new(home).join(".codebuddy.json"),
    ];
    for candidate in &candidates {
        if let Ok(info) = std::fs::metadata(candidate) {
            if !info.is_dir() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }
    candidates[0].to_string_lossy().to_string()
}

/// `unmarshalRuntimeMcpConfig` (runtime_mcp.go:115–139): decodes one runtime's
/// config file. "jsonc" is JSON with comments and trailing commas, which
/// CodeBuddy accepts in its MCP files — parsing those as strict JSON would
/// drop every server behind a single `//`.
fn unmarshal_runtime_mcp_config(raw: &[u8], format: &str) -> anyhow::Result<Value> {
    match format {
        "toml" => toml::from_str::<Value>(&String::from_utf8_lossy(raw))
            .context("parse runtime MCP config"),
        "jsonc" => {
            let stripped = strip_jsonc(raw).context("parse runtime MCP config")?;
            serde_json::from_slice(&stripped).context("parse runtime MCP config")
        }
        _ => serde_json::from_slice(raw).context("parse runtime MCP config"),
    }
}

/// `stripJSONC` (runtime_mcp.go:141–240): rewrites JSONC into strict JSON —
/// `//` and `/* */` comments become spaces and a single trailing comma before
/// `}` / `]` is dropped. String literals are copied verbatim, so a `//` or a
/// comma inside a command argument survives.
///
/// Output is always the same length as the input — comments and the dropped
/// comma are blanked, never deleted — so a parse-error offset still points at
/// the byte the user actually wrote.
///
/// Only the LAST comma before a closer is blanked, so genuinely malformed
/// input stays malformed: `[1,,,]` does not silently become `[1]`. An
/// unterminated `/*` is an error rather than a blank-to-EOF, so the Agent >
/// MCP tab cannot list servers out of a file CodeBuddy itself rejects.
fn strip_jsonc(raw: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(raw.len());
    // Offset into out of the one comma still eligible for removal, or None.
    // Reset by any value token, so only a genuinely trailing comma is dropped.
    let mut last_comma: Option<usize> = None;
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

        match c {
            b'"' => {
                in_string = true;
                last_comma = None;
                out.push(c);
            }
            b'/' if i + 1 < raw.len() && raw[i + 1] == b'/' => {
                // Blank through end of line, preserving the newline itself.
                while i < raw.len() && raw[i] != b'\n' {
                    out.push(b' ');
                    i += 1;
                }
                if i < raw.len() {
                    out.push(raw[i]);
                }
            }
            b'/' if i + 1 < raw.len() && raw[i + 1] == b'*' => {
                // Consume the opener first so `/*/` cannot reuse its own `*`
                // as the closer's, then blank the body, newlines included, so
                // line numbers and the total byte count both survive.
                out.push(b' ');
                out.push(b' ');
                i += 2;
                let mut closed = false;
                while i < raw.len() {
                    if raw[i] == b'*' && i + 1 < raw.len() && raw[i + 1] == b'/' {
                        out.push(b' ');
                        out.push(b' ');
                        i += 1; // consume '*'; the loop's increment consumes '/'
                        closed = true;
                        break;
                    }
                    if raw[i] == b'\n' {
                        out.push(b'\n');
                    } else {
                        out.push(b' ');
                    }
                    i += 1;
                }
                if !closed {
                    return Err(anyhow::anyhow!("unterminated block comment"));
                }
            }
            b',' => {
                last_comma = Some(out.len());
                out.push(c);
            }
            b'}' | b']' => {
                if let Some(offset) = last_comma.take() {
                    out[offset] = b' ';
                }
                out.push(c);
            }
            b' ' | b'\t' | b'\n' | b'\r' => out.push(c),
            _ => {
                last_comma = None;
                out.push(c);
            }
        }
        i += 1;
    }
    Ok(out)
}

/// `loadRuntimeMcpServerConfigs` (runtime_mcp.go:242–316): returns full,
/// secret-bearing runtime MCP entries for task-local merging. Callers must
/// never send the result to the server or logs; the public capabilities
/// endpoint continues to use the redacted summary type above.
///
/// Returns `(servers, supported)`; an unsupported provider yields an empty map
/// and `false`.
pub(crate) fn load_runtime_mcp_server_configs(
    provider: &str,
) -> anyhow::Result<(HashMap<String, Value>, bool)> {
    let home = user_home_dir().context("resolve user home")?;

    enum Source {
        File { path: String, key: &'static str, format: &'static str },
        Unsupported,
    }
    let source = match provider {
        "claude" => Source::File {
            path: Path::new(&home).join(".claude.json").to_string_lossy().to_string(),
            key: "mcpServers",
            format: "json",
        },
        // codebuddy is deliberately absent. CodeBuddy loads its own user,
        // project and local scopes on every launch (codebuddy.go never passes
        // --strict-mcp-config), and a managed entry already wins a same-name
        // collision, so the daemon pre-merging them would only duplicate what
        // the CLI does natively — while losing the scope precedence and the
        // approval gate that protects project-scope servers.
        "codex" => {
            let codex_home = env_trimmed_or("CODEX_HOME", || {
                Path::new(&home).join(".codex").to_string_lossy().to_string()
            });
            Source::File {
                path: Path::new(&codex_home).join("config.toml").to_string_lossy().to_string(),
                key: "mcp_servers",
                format: "toml",
            }
        }
        "cursor" => Source::File {
            path: Path::new(&home).join(".cursor").join("mcp.json").to_string_lossy().to_string(),
            key: "mcpServers",
            format: "json",
        },
        "opencode" => {
            let config_home = env_trimmed_or("XDG_CONFIG_HOME", || {
                Path::new(&home).join(".config").to_string_lossy().to_string()
            });
            Source::File {
                path: Path::new(&config_home)
                    .join("opencode")
                    .join("opencode.json")
                    .to_string_lossy()
                    .to_string(),
                key: "mcp",
                format: "json",
            }
        }
        "openclaw" => {
            let path = openclaw_config_path(&home);
            Source::File { path, key: "mcp.servers", format: "json" }
        }
        _ => Source::Unsupported,
    };

    let Source::File { path, key, format } = source else {
        return Ok((HashMap::new(), false));
    };

    let mut servers: HashMap<String, Value> = HashMap::new();
    match std::fs::read(&path) {
        Ok(raw) => {
            let cfg = unmarshal_runtime_mcp_config(&raw, format)?;
            if let Some(configured) = nested_runtime_mcp_map(&cfg, key) {
                for (name, entry) in configured {
                    servers.insert(
                        name.clone(),
                        normalize_runtime_mcp_entry(provider, entry.clone()),
                    );
                }
            }
        }
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            return Err(anyhow::Error::from(err).context("read runtime MCP config"));
        }
        Err(_) => {}
    }

    if provider == "claude" {
        // User configuration has the same precedence Claude uses: plugin
        // servers only fill names not already defined by the user.
        for (name, entry) in load_claude_plugin_mcp_server_configs(&home) {
            servers.entry(name).or_insert(entry);
        }
    }
    Ok((servers, true))
}

/// `normalizeRuntimeMcpEntry` (runtime_mcp.go:318–337).
fn normalize_runtime_mcp_entry(provider: &str, value: Value) -> Value {
    let Some(entry) = value.as_object() else {
        return value;
    };
    if provider != "codex" {
        return Value::Object(entry.clone());
    }
    let mut entry = entry.clone();
    // Cordy's canonical remote shape calls these `headers`; Codex stores them
    // as `http_headers`. Keep the original key as well so less common
    // Codex-specific settings round-trip through renderCodexMcpServersBlock.
    if let Some(headers) = entry.get("http_headers").cloned() {
        entry.entry("headers".to_string()).or_insert(headers);
    }
    if entry.contains_key("url") && !entry.contains_key("type") {
        entry.insert("type".into(), Value::String("http".into()));
    }
    Value::Object(entry)
}

/// S9-integration seam for `loadClaudePluginMcpServerConfigs`
/// (runtime_mcp.go:339–369): walks enabled Claude plugins' `.mcp.json` /
/// manifest MCP files. Empty until lane C2's claude_plugins.rs lands.
fn load_claude_plugin_mcp_server_configs(_home: &str) -> HashMap<String, Value> {
    HashMap::new()
}

/// S9-integration seam for `listClaudePluginMcpServers`
/// (runtime_mcp.go:482–508). Empty until lane C2's claude_plugins.rs lands.
fn list_claude_plugin_mcp_servers(_home: &str) -> Vec<RuntimeLocalMcpServerSummary> {
    Vec::new()
}

/// `listRuntimeLocalMcpServers` (runtime_mcp.go:371–456): the redacted
/// inventory for one provider. Returns `(summaries, supported)`.
pub(crate) fn list_runtime_local_mcp_servers(
    provider: &str,
) -> anyhow::Result<(Vec<RuntimeLocalMcpServerSummary>, bool)> {
    let home = user_home_dir().context("resolve user home")?;

    struct Spec {
        path: String,
        key: &'static str,
        source: &'static str,
        format: &'static str,
    }
    let spec = match provider {
        "claude" => Spec {
            path: Path::new(&home).join(".claude.json").to_string_lossy().to_string(),
            key: "mcpServers",
            source: "User config",
            format: "json",
        },
        "codebuddy" => Spec {
            path: codebuddy_user_mcp_config_path(&home),
            key: "mcpServers",
            source: "User config",
            format: "jsonc",
        },
        "kimi" => {
            // Inventory only — kimi is deliberately absent from
            // load_runtime_mcp_server_configs. `kimi acp` merges this file
            // with the ephemeral `mcpServers` we send in session/new, so
            // merging it in again would spawn every user server twice.
            let kimi_home = env_trimmed_or("KIMI_CODE_HOME", || {
                Path::new(&home).join(".kimi-code").to_string_lossy().to_string()
            });
            Spec {
                path: Path::new(&kimi_home).join("mcp.json").to_string_lossy().to_string(),
                key: "mcpServers",
                source: "User config",
                format: "json",
            }
        }
        "codex" => {
            let codex_home = env_trimmed_or("CODEX_HOME", || {
                Path::new(&home).join(".codex").to_string_lossy().to_string()
            });
            Spec {
                path: Path::new(&codex_home).join("config.toml").to_string_lossy().to_string(),
                key: "mcp_servers",
                source: "User config",
                format: "toml",
            }
        }
        "cursor" => Spec {
            path: Path::new(&home).join(".cursor").join("mcp.json").to_string_lossy().to_string(),
            key: "mcpServers",
            source: "User config",
            format: "json",
        },
        "opencode" => {
            let config_home = env_trimmed_or("XDG_CONFIG_HOME", || {
                Path::new(&home).join(".config").to_string_lossy().to_string()
            });
            Spec {
                path: Path::new(&config_home)
                    .join("opencode")
                    .join("opencode.json")
                    .to_string_lossy()
                    .to_string(),
                key: "mcp",
                source: "User config",
                format: "json",
            }
        }
        "openclaw" => Spec {
            path: openclaw_config_path(&home),
            key: "mcp.servers",
            source: "User config",
            format: "json",
        },
        _ => return Ok((Vec::new(), false)),
    };

    let mut out: Vec<RuntimeLocalMcpServerSummary> = Vec::new();
    match std::fs::read(&spec.path) {
        Ok(raw) => {
            let cfg = unmarshal_runtime_mcp_config(&raw, spec.format)?;
            if let Some(servers) = nested_runtime_mcp_map(&cfg, spec.key) {
                out.extend(runtime_mcp_summaries(servers, spec.source));
            }
        }
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            return Err(anyhow::Error::from(err).context("read runtime MCP config"));
        }
        Err(_) => {}
    }

    if provider == "claude" {
        out.extend(list_claude_plugin_mcp_servers(&home));
    }

    // User configuration wins on a same-name collision. Plugin entries are
    // appended afterwards and only fill names the user config did not define.
    let mut seen = std::collections::HashSet::new();
    out.retain(|server| seen.insert(server.name.clone()));
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok((out, true))
}

/// `runtimeMcpSummaries` (runtime_mcp.go:458–480).
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
        let mut enabled = true;
        if let Some(flag) = entry.get("enabled").and_then(Value::as_bool) {
            enabled = flag;
        }
        if entry.get("disabled").and_then(Value::as_bool) == Some(true) {
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

/// `nestedRuntimeMcpMap` (runtime_mcp.go:510–528).
fn nested_runtime_mcp_map<'a>(cfg: &'a Value, path: &str) -> Option<&'a Map<String, Value>> {
    let mut current = cfg.as_object()?;
    let parts: Vec<&str> = path.split('.').collect();
    for (index, part) in parts.iter().enumerate() {
        let mapped = current.get(*part)?.as_object()?;
        if index == parts.len() - 1 {
            return Some(mapped);
        }
        current = mapped;
    }
    None
}

/// `runtimeMcpTransport` (runtime_mcp.go:530–547).
fn runtime_mcp_transport(entry: &Map<String, Value>) -> String {
    let kind = entry.get("type").and_then(Value::as_str).unwrap_or("");
    match kind.to_lowercase().as_str() {
        "local" | "stdio" => return "stdio".into(),
        "remote" | "http" | "streamable-http" => return "http".into(),
        "sse" => return "sse".into(),
        _ => {}
    }
    if entry.contains_key("command") {
        return "stdio".into();
    }
    if entry.contains_key("url") {
        return "http".into();
    }
    "unknown".into()
}

// ---------------------------------------------------------------------------
// Shared helpers.
// ---------------------------------------------------------------------------

fn trim_ascii(mut data: &[u8]) -> &[u8] {
    while let Some(first) = data.first() {
        if first.is_ascii_whitespace() {
            data = &data[1..];
        } else {
            break;
        }
    }
    while let Some(last) = data.last() {
        if last.is_ascii_whitespace() {
            data = &data[..data.len() - 1];
        } else {
            break;
        }
    }
    data
}

fn user_home_dir() -> anyhow::Result<String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| anyhow::anyhow!("$HOME is not defined"))
}

fn env_trimmed_or(name: &str, default: impl FnOnce() -> String) -> String {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => default(),
    }
}

/// openclaw's config path resolution (runtime_mcp.go:277–284, 409–416):
/// CLAWDBOT_CONFIG_PATH wins, then OPENCLAW_STATE_DIR/openclaw.json, then
/// ~/.openclaw/openclaw.json.
fn openclaw_config_path(home: &str) -> String {
    if let Ok(path) = std::env::var("CLAWDBOT_CONFIG_PATH") {
        if !path.trim().is_empty() {
            return path.trim().to_string();
        }
    }
    let state_dir = env_trimmed_or("OPENCLAW_STATE_DIR", || {
        Path::new(home).join(".openclaw").to_string_lossy().to_string()
    });
    Path::new(&state_dir).join("openclaw.json").to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Process-global env is not test-scoped in Rust; serialize the tests that
    /// mirror Go's t.Setenv.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn set_env(name: &str, value: Option<&str>) {
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }

    // ------------------------------------------------------------------
    // stripJSONC (runtime_mcp_test.go:298–401) — pure logic.
    // ------------------------------------------------------------------

    #[test]
    fn strip_jsonc_repairs_comments_and_trailing_commas() {
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
            let stripped = strip_jsonc(input.as_bytes())
                .unwrap_or_else(|err| panic!("stripJSONC {name}: {err}"));
            let got: Value = serde_json::from_slice(&stripped)
                .unwrap_or_else(|err| panic!("unmarshal {name}: {err}"));
            assert_eq!(got, want, "case {name}");
        }
    }

    /// A trailing comma that is NOT trailing must survive: blanking it would
    /// merge two array elements into one.
    #[test]
    fn strip_jsonc_keeps_separator_commas() {
        let stripped = strip_jsonc(br#"["a","b",]"#).unwrap();
        let got: Value = serde_json::from_slice(&stripped).unwrap();
        assert_eq!(got, json!(["a", "b"]));
    }

    /// Repairing only a genuine trailing comma keeps malformed input
    /// malformed — a config the CLI would reject must not be silently
    /// accepted here.
    #[test]
    fn strip_jsonc_leaves_malformed_input_invalid() {
        for input in [r"[1,,,]", r#"{"a":1,,}"#, "[1,,2]"] {
            if let Ok(stripped) = strip_jsonc(input.as_bytes()) {
                assert!(
                    serde_json::from_slice::<Value>(&stripped).is_err(),
                    "{input} must stay invalid"
                );
            }
        }
    }

    /// Comments and the dropped comma are blanked, never deleted, so a
    /// parse-error offset still points at the byte the user wrote.
    #[test]
    fn strip_jsonc_preserves_byte_length() {
        for input in [
            "{\n  // c\n  \"a\": 1,\n}\n",
            "{/* block */\"a\":1,}",
            r#"{"a":"// not a comment"}"#,
            "{/**/\"a\":1}",
            "{}",
        ] {
            let stripped = strip_jsonc(input.as_bytes())
                .unwrap_or_else(|err| panic!("stripJSONC {input:?}: {err}"));
            assert_eq!(stripped.len(), input.len(), "{input:?}");
        }
    }

    /// A file CodeBuddy itself rejects must not produce an inventory listing;
    /// blanking an unterminated comment to EOF would have turned the first
    /// into valid JSON, and letting the opener's `*` double as the closer's
    /// would have made `/*/` a complete comment.
    #[test]
    fn strip_jsonc_rejects_unterminated_block_comment() {
        for (name, input) in [
            ("unterminated", r#"{"mcpServers":{}} /* oops"#),
            ("opener not closer", "/*/ {\"mcpServers\":{}}"),
            ("unterminated after opener", "/*"),
        ] {
            assert!(
                strip_jsonc(input.as_bytes()).is_err(),
                "{name}: {input} must fail"
            );
        }
    }

    // ------------------------------------------------------------------
    // codebuddyUserMcpConfigPath (runtime_mcp_test.go:190–239).
    // ------------------------------------------------------------------

    #[test]
    fn codebuddy_user_mcp_config_path_precedence() {
        let _env = ENV_LOCK.lock().unwrap();
        set_env("CODEBUDDY_CONFIG_DIR", None);
        let home = tempfile::tempdir().unwrap();
        let home_str = home.path().to_string_lossy().to_string();
        let config_dir = home.path().join(".codebuddy");
        std::fs::create_dir_all(&config_dir).unwrap();

        // Nothing on disk: the caller must still get a stable path so the
        // read fails with NotFound rather than the provider looking
        // unsupported.
        assert_eq!(
            codebuddy_user_mcp_config_path_in(&home_str, None),
            config_dir.join(".mcp.json").to_string_lossy()
        );

        let legacy = home.path().join(".codebuddy.json");
        std::fs::write(&legacy, br#"{"mcpServers":{}}"#).unwrap();
        assert_eq!(
            codebuddy_user_mcp_config_path_in(&home_str, None),
            legacy.to_string_lossy()
        );

        let plain = config_dir.join("mcp.json");
        std::fs::write(&plain, br#"{"mcpServers":{}}"#).unwrap();
        assert_eq!(
            codebuddy_user_mcp_config_path_in(&home_str, None),
            plain.to_string_lossy(),
            "mcp.json must win over ~/.codebuddy.json"
        );

        let dotted = config_dir.join(".mcp.json");
        std::fs::write(&dotted, br#"{"mcpServers":{}}"#).unwrap();
        assert_eq!(
            codebuddy_user_mcp_config_path_in(&home_str, None),
            dotted.to_string_lossy(),
            ".mcp.json must win over mcp.json"
        );
    }

    #[test]
    fn codebuddy_user_mcp_config_path_honors_config_dir_env() {
        let _env = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let want = config_dir.path().join(".mcp.json");
        std::fs::write(&want, br#"{"mcpServers":{}}"#).unwrap();
        assert_eq!(
            codebuddy_user_mcp_config_path_in(
                &home.path().to_string_lossy(),
                Some(config_dir.path().to_string_lossy().as_ref()),
            ),
            want.to_string_lossy()
        );
    }

    // ------------------------------------------------------------------
    // Inventory + merge (runtime_mcp_test.go:11–91, 131–184, 241–296,
    // 405–468).
    // ------------------------------------------------------------------

    #[test]
    fn list_runtime_local_mcp_servers_codex_redacts_details() {
        let _env = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        set_env("HOME", Some(home.path().to_string_lossy().as_ref()));
        set_env("CODEX_HOME", None);
        let config_dir = home.path().join(".codex");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            concat!(
                "[mcp_servers.fetch]\n",
                "command = \"uvx\"\n",
                "args = [\"mcp-server-fetch\", \"--token\", \"secret\"]\n\n",
                "[mcp_servers.docs]\n",
                "url = \"https://secret.example/mcp\"\n",
                "enabled = false\n",
            ),
        )
        .unwrap();

        let (servers, supported) = list_runtime_local_mcp_servers("codex").unwrap();
        assert!(supported && servers.len() == 2, "supported={supported} servers={servers:?}");
        assert_eq!(servers[0].name, "docs");
        assert_eq!(servers[0].transport, "http");
        assert!(!servers[0].enabled);
        assert_eq!(servers[1].name, "fetch");
        assert_eq!(servers[1].transport, "stdio");
        assert!(servers[1].enabled);
    }

    #[test]
    fn list_runtime_local_mcp_servers_claude_missing_config() {
        let _env = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        set_env("HOME", Some(home.path().to_string_lossy().as_ref()));
        let (servers, supported) = list_runtime_local_mcp_servers("claude").unwrap();
        assert!(supported && servers.is_empty(), "supported={supported} servers={servers:?}");
    }

    #[test]
    fn list_runtime_local_mcp_servers_unknown_provider() {
        let _env = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        set_env("HOME", Some(home.path().to_string_lossy().as_ref()));
        let (servers, supported) = list_runtime_local_mcp_servers("future-runtime").unwrap();
        assert!(!supported && servers.is_empty());
    }

    #[test]
    fn merge_runtime_and_agent_mcp_config_codex_normalizes_headers() {
        let _env = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        set_env("HOME", Some(home.path().to_string_lossy().as_ref()));
        set_env("CODEX_HOME", None);
        let config_dir = home.path().join(".codex");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            concat!(
                "[mcp_servers.docs]\n",
                "url = \"https://runtime.example/mcp\"\n",
                "http_headers = { Authorization = \"Bearer local-secret\" }\n\n",
                "[mcp_servers.fetch]\n",
                "command = \"uvx\"\n",
                "args = [\"mcp-server-fetch\"]\n",
            ),
        )
        .unwrap();

        let merged = merge_runtime_and_agent_mcp_config(
            "codex",
            br#"{"mcpServers":{"agent":{"command":"node","args":["agent.js"]}}}"#,
        )
        .unwrap();
        let document: Value = serde_json::from_slice(&merged).unwrap();
        let mcp_servers = document.get("mcpServers").unwrap().as_object().unwrap();
        assert_eq!(mcp_servers.len(), 3, "merged servers = {mcp_servers:?}");
        let docs = mcp_servers.get("docs").unwrap();
        assert_eq!(docs.get("type"), Some(&json!("http")));
        assert_eq!(
            docs.get("headers").unwrap().get("Authorization"),
            Some(&json!("Bearer local-secret"))
        );
    }

    #[test]
    fn merge_runtime_and_agent_mcp_config_null_keeps_native_inheritance() {
        let _env = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        set_env("HOME", Some(home.path().to_string_lossy().as_ref()));
        for raw in [&b""[..], b"null".as_slice(), b" null ".as_slice()] {
            let merged = merge_runtime_and_agent_mcp_config("claude", raw).unwrap();
            assert_eq!(merged, raw);
        }
    }

    /// CodeBuddy resolves its own config and loads its own scopes on every
    /// launch, so `~/.claude.json` is never consulted and plugin servers are
    /// never attributed to CodeBuddy (the plugin walk is an S9-integration
    /// seam until lane C2 lands, matching the empty result either way).
    #[test]
    fn list_runtime_local_mcp_servers_codebuddy_reads_its_own_config() {
        let _env = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        set_env("HOME", Some(home.path().to_string_lossy().as_ref()));
        set_env("CODEBUDDY_CONFIG_DIR", None);
        let config_dir = home.path().join(".codebuddy");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            home.path().join(".claude.json"),
            br#"{"mcpServers":{"claude-only":{"command":"claude-cmd"}}}"#,
        )
        .unwrap();
        // JSONC: a comment and a trailing comma must not lose the server.
        let jsonc = "{\n  // user scope\n  \"mcpServers\": {\n    \"buddy\": { \"command\": \"buddy-cmd\", },\n  },\n}\n";
        std::fs::write(config_dir.join(".mcp.json"), jsonc).unwrap();

        let (servers, supported) = list_runtime_local_mcp_servers("codebuddy").unwrap();
        assert!(supported && servers.len() == 1 && servers[0].name == "buddy",
            "supported={supported} servers={servers:?}");
    }

    /// CodeBuddy loads its own user/project/local scopes on every launch, so
    /// the daemon must NOT pre-merge them.
    #[test]
    fn merge_runtime_and_agent_mcp_config_codebuddy_is_passthrough() {
        let _env = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        set_env("HOME", Some(home.path().to_string_lossy().as_ref()));
        set_env("CODEBUDDY_CONFIG_DIR", None);
        let config_dir = home.path().join(".codebuddy");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join(".mcp.json"),
            br#"{"mcpServers":{"local-only":{"command":"local-cmd"}}}"#,
        )
        .unwrap();

        let agent_config =
            br#"{"mcpServers":{"agent-only":{"command":"agent-cmd"}}}"#.as_slice();
        let merged = merge_runtime_and_agent_mcp_config("codebuddy", agent_config).unwrap();
        assert_eq!(merged, agent_config, "codebuddy merge must be a passthrough");
    }

    /// The same unterminated-comment input must reach the caller as a parse
    /// error, so the inventory surfaces the problem rather than silently
    /// reporting zero servers.
    #[test]
    fn list_runtime_local_mcp_servers_codebuddy_rejects_unterminated_comment() {
        let _env = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        set_env("HOME", Some(home.path().to_string_lossy().as_ref()));
        set_env("CODEBUDDY_CONFIG_DIR", None);
        let config_dir = home.path().join(".codebuddy");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join(".mcp.json"),
            br#"{"mcpServers":{"buddy":{"command":"buddy-cmd"}}} /* oops"#,
        )
        .unwrap();

        assert!(
            list_runtime_local_mcp_servers("codebuddy").is_err(),
            "expected a parse error for a config CodeBuddy itself rejects"
        );
    }

    #[test]
    fn list_runtime_local_mcp_servers_kimi() {
        let _env = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        set_env("HOME", Some(home.path().to_string_lossy().as_ref()));
        set_env("KIMI_CODE_HOME", None);
        let kimi_home = home.path().join(".kimi-code");
        std::fs::create_dir_all(&kimi_home).unwrap();
        std::fs::write(
            kimi_home.join("mcp.json"),
            br#"{"mcpServers":{"paper":{"url":"https://paper.example/mcp"}}}"#,
        )
        .unwrap();

        let (servers, supported) = list_runtime_local_mcp_servers("kimi").unwrap();
        assert!(supported && servers.len() == 1 && servers[0].name == "paper",
            "supported={supported} servers={servers:?}");
    }

    /// kimi merges <KIMI_CODE_HOME>/mcp.json with the ephemeral `mcpServers`
    /// array the ACP backend sends in session/new, and a duplicate name spawns
    /// the server twice. So kimi is inventory-only: the task-local merge must
    /// leave the agent's config untouched.
    #[test]
    fn merge_runtime_and_agent_mcp_config_kimi_is_passthrough() {
        let _env = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap();
        set_env("HOME", Some(home.path().to_string_lossy().as_ref()));
        set_env("KIMI_CODE_HOME", None);
        let kimi_home = home.path().join(".kimi-code");
        std::fs::create_dir_all(&kimi_home).unwrap();
        std::fs::write(
            kimi_home.join("mcp.json"),
            br#"{"mcpServers":{"paper":{"url":"https://paper.example/mcp"}}}"#,
        )
        .unwrap();

        let agent_config =
            br#"{"mcpServers":{"agent-only":{"command":"agent-cmd"}}}"#.as_slice();
        let merged = merge_runtime_and_agent_mcp_config("kimi", agent_config).unwrap();
        assert_eq!(merged, agent_config, "kimi merge must be a passthrough");
    }
}
