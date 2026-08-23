//! Port of execenv/cursor_mcp.go.
//!
//! Symbol map:
//! - CursorMcpAuthSourceEnv        → CURSOR_MCP_AUTH_SOURCE_ENV
//! - cursorWorkspaceTrustedFile /
//!    cursorMcpAuthFile             → CURSOR_WORKSPACE_TRUSTED_FILE / CURSOR_MCP_AUTH_FILE
//! - prepareCursorMcpConfig        → prepare_cursor_mcp_config
//! - seedCursorMcpAuthFile         → seed_cursor_mcp_auth_file
//! - removeCursorMcpAuthFile       → remove_cursor_mcp_auth_file
//! - resolveCursorMcpAuthSource    → resolve_cursor_mcp_auth_source
//! - copyCursorMcpAuthFile         → copy_cursor_mcp_auth_file
//! - hasManagedCursorMcpConfig     → has_managed_cursor_mcp_config
//! - parseCursorManagedMcpServers  → parse_cursor_managed_mcp_servers
//! - marshalCursorMcpConfig        → marshal_cursor_mcp_config
//! - cursorMcpApprovalKeys         → cursor_mcp_approval_keys
//! - marshalCursorMcpApprovalServer → marshal_cursor_mcp_approval_server
//! - marshalJSONStringifyCompatible → (serde_json does not HTML-escape; the
//!    Go workaround is unnecessary — plain
//!    compact serialization matches
//!    JSON.stringify output)
//! - cursorProjectRoot             → cursor_project_root
//! - cursorSlugifyPath             → cursor_slugify_path
//!
//! Deviations:
//! - Go's ordered-struct normalization for approval hashing is reproduced with
//!    serde_json preserve_order + explicit field insertion order.
//! - slog logger parameters dropped.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context};
use serde_json::{json, Map, Value};

use super::context::SidecarManifest;

/// Agent custom_env key the daemon consumes before launching cursor-agent.
pub(crate) const CURSOR_MCP_AUTH_SOURCE_ENV: &str = "CURSOR_MCP_AUTH_SOURCE";

const CURSOR_WORKSPACE_TRUSTED_FILE: &str = ".workspace-trusted";
const CURSOR_MCP_AUTH_FILE: &str = "mcp-auth.json";

/// prepare_cursor_mcp_config writes the Cursor-native MCP sidecars for agents
/// that have an explicit managed mcp_config saved. A null/absent mcp_config
/// means "let Cursor behave normally", so no .cursor/mcp.json or
/// CURSOR_DATA_DIR is created.
pub(crate) fn prepare_cursor_mcp_config(
    env_root: &str,
    work_dir: &str,
    mcp_config: Option<&Value>,
    mcp_auth_source: &str,
    mut manifest: Option<&mut SidecarManifest>,
) -> anyhow::Result<String> {
    if !has_managed_cursor_mcp_config(mcp_config) {
        return Ok(String::new());
    }
    if env_root.is_empty() {
        bail!("env root is required for managed cursor mcp_config");
    }

    let project_root = cursor_project_root(work_dir);
    let servers = match mcp_config {
        Some(v) => parse_cursor_managed_mcp_servers(v)?,
        None => return Ok(String::new()),
    };

    let cursor_dir = join(&project_root, ".cursor");
    super::context::record_mkdir_all(&cursor_dir, manifest.as_deref_mut())
        .context("create .cursor dir")?;
    let config_data = marshal_cursor_mcp_config(&servers)?;
    if let Err(err) = super::context::record_write_file(
        &join(&cursor_dir, "mcp.json"),
        config_data.as_bytes(),
        manifest.as_deref_mut(),
    ) {
        if err
            .downcast_ref::<super::context::ErrPathPreExists>()
            .is_some()
            || format!("{err:#}").contains("refuse to overwrite pre-existing path")
        {
            bail!("managed cursor mcp_config would overwrite existing .cursor/mcp.json");
        }
        return Err(anyhow!("write .cursor/mcp.json: {err:#}"));
    }

    let cursor_data_dir = join(env_root, "cursor-data");
    let project_data_dir = join(
        &join(&cursor_data_dir, "projects"),
        &cursor_slugify_path(&project_root),
    );
    std::fs::create_dir_all(&project_data_dir).context("create cursor project data dir")?;
    remove_cursor_mcp_auth_file(&project_data_dir)?;
    let approvals = cursor_mcp_approval_keys(&project_root, &servers)?;
    let approval_data =
        serde_json::to_string_pretty(&approvals).context("marshal cursor mcp approvals")?;
    std::fs::write(
        join(&project_data_dir, "mcp-approvals.json"),
        approval_data.as_bytes(),
    )
    .context("write cursor mcp approvals")?;
    let trust_data = serde_json::to_string_pretty(&json!({
        "trustedAt": "1970-01-01T00:00:00Z",
        "workspacePath": project_root,
        "trustMethod": "cordy-managed",
    }))
    .context("marshal cursor workspace trust")?;
    std::fs::write(
        join(&project_data_dir, CURSOR_WORKSPACE_TRUSTED_FILE),
        trust_data.as_bytes(),
    )
    .context("write cursor workspace trust")?;
    if !mcp_auth_source.trim().is_empty() {
        seed_cursor_mcp_auth_file(&project_data_dir, mcp_auth_source)?;
    }

    Ok(cursor_data_dir)
}

fn join(base: &str, seg: &str) -> String {
    super::execenv::join_path(&[base, seg])
}

fn seed_cursor_mcp_auth_file(project_data_dir: &str, source: &str) -> anyhow::Result<()> {
    let source_path = resolve_cursor_mcp_auth_source(source)?;
    let target = join(project_data_dir, CURSOR_MCP_AUTH_FILE);
    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink(&source_path, &target).is_ok() {
            return Ok(());
        }
    }
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_file(&source_path, &target).is_ok() {
            return Ok(());
        }
    }
    copy_cursor_mcp_auth_file(&target, &source_path).context("seed cursor mcp auth file")
}

fn remove_cursor_mcp_auth_file(project_data_dir: &str) -> anyhow::Result<()> {
    let target = join(project_data_dir, CURSOR_MCP_AUTH_FILE);
    match std::fs::remove_file(&target) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow!("remove prior cursor mcp auth file: {e}")),
    }
}

fn resolve_cursor_mcp_auth_source(source: &str) -> anyhow::Result<String> {
    let mut source = source.trim().to_string();
    if source.is_empty() {
        bail!("{CURSOR_MCP_AUTH_SOURCE_ENV} is empty");
    }
    if source == "~" || source.starts_with("~/") {
        let home = super::execenv::user_home_dir()
            .map_err(|e| anyhow!("resolve {CURSOR_MCP_AUTH_SOURCE_ENV} home directory: {e}"))?;
        if source == "~" {
            source = home;
        } else {
            source = join(&home, &source[2..]);
        }
    }
    if !source.starts_with('/')
        && !source.starts_with("\\\\")
        && source.as_bytes().get(1) != Some(&b':')
    {
        bail!(
            "{CURSOR_MCP_AUTH_SOURCE_ENV} must be an absolute path to {CURSOR_MCP_AUTH_FILE} or its containing Cursor project directory"
        );
    }
    let source = clean_lexical(&source);
    let info = std::fs::metadata(&source)
        .map_err(|e| anyhow!("stat {CURSOR_MCP_AUTH_SOURCE_ENV}: {e}"))?;
    let mut source = source;
    if info.is_dir() {
        source = join(&source, CURSOR_MCP_AUTH_FILE);
        let inner = std::fs::metadata(&source).map_err(|e| {
            anyhow!("stat {CURSOR_MCP_AUTH_SOURCE_ENV} {CURSOR_MCP_AUTH_FILE}: {e}")
        })?;
        if inner.is_dir() {
            bail!("{CURSOR_MCP_AUTH_SOURCE_ENV} must resolve to a file, got directory {source}");
        }
    }
    let base = Path::new(&source)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    if base != CURSOR_MCP_AUTH_FILE {
        bail!("{CURSOR_MCP_AUTH_SOURCE_ENV} must point at {CURSOR_MCP_AUTH_FILE}, got {base}");
    }
    Ok(source)
}

/// Lexical clean for rooted paths (filepath.Clean subset used here).
fn clean_lexical(path: &str) -> String {
    Path::new(path)
        .components()
        .fold(std::path::PathBuf::new(), |mut acc, c| {
            use std::path::Component::*;
            match c {
                CurDir => {}
                RootDir => acc.push("/"),
                ParentDir => {
                    acc.pop();
                }
                other => acc.push(other.as_os_str()),
            }
            acc
        })
        .to_string_lossy()
        .into_owned()
}

fn copy_cursor_mcp_auth_file(target: &str, source: &str) -> anyhow::Result<()> {
    let data = std::fs::read(source)?;
    let result = std::fs::write(target, &data);
    if let Err(e) = result {
        let _ = std::fs::remove_file(target);
        return Err(e.into());
    }
    Ok(())
}

fn has_managed_cursor_mcp_config(raw: Option<&Value>) -> bool {
    match raw {
        None => false,
        Some(Value::Null) => false,
        Some(_) => true,
    }
}

fn parse_cursor_managed_mcp_servers(raw: &Value) -> anyhow::Result<BTreeMap<String, Value>> {
    let obj = raw
        .as_object()
        .ok_or_else(|| anyhow!("parse mcp_config json: not a JSON object"))?;
    let servers = match obj.get("mcpServers") {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(Value::Object(m)) => {
            let mut out = BTreeMap::new();
            for (name, server) in m {
                if name.trim().is_empty() {
                    bail!("mcp server name must not be empty");
                }
                if !server.is_object() {
                    bail!("mcp_servers.{name} must be a JSON object");
                }
                out.insert(name.clone(), server.clone());
            }
            out
        }
        Some(other) => bail!("parse mcp_config json: mcpServers must be an object, got {other}"),
    };
    Ok(servers)
}

/// Serializes the .cursor/mcp.json body. Sorted keys (BTreeMap upstream of the
/// Value map) keep output deterministic; a trailing newline mirrors Go.
fn marshal_cursor_mcp_config(servers: &BTreeMap<String, Value>) -> anyhow::Result<String> {
    let mut map = Map::new();
    // Insert in sorted key order so the bytes are deterministic like Go's
    // sorted marshaling path.
    let mut names: Vec<&String> = servers.keys().collect();
    names.sort();
    for name in names {
        map.insert(name.clone(), servers[name].clone());
    }
    let mut root = Map::new();
    root.insert("mcpServers".to_string(), Value::Object(map));
    let mut data =
        serde_json::to_string_pretty(&Value::Object(root)).context("marshal cursor mcp config")?;
    data.push('\n');
    Ok(data)
}

/// cursor_mcp_approval_keys computes the per-server approval file entries:
/// "<name>-<first16 hex chars of sha256(compact payload)>", where the payload
/// is {"path":<projectRoot>,"server":<normalized server>}.
fn cursor_mcp_approval_keys(
    project_root: &str,
    servers: &BTreeMap<String, Value>,
) -> anyhow::Result<Vec<String>> {
    use sha2::{Digest, Sha256};
    let mut approvals = Vec::with_capacity(servers.len());
    for (name, server) in servers {
        let server_json = marshal_cursor_mcp_approval_server(server)
            .map_err(|e| anyhow!("marshal mcp_servers.{name} for cursor approval: {e}"))?;
        let path_json =
            serde_json::to_string(project_root).context("marshal cursor project root")?;
        let mut payload = String::from("{\"path\":");
        payload.push_str(&path_json);
        payload.push_str(",\"server\":");
        payload.push_str(&server_json);
        payload.push('}');

        let sum = Sha256::digest(payload.as_bytes());
        let hex_sum = hex::encode(sum);
        approvals.push(format!("{name}-{}", &hex_sum[..16]));
    }
    Ok(approvals)
}

/// Normalizes a server entry the way Cursor does before hashing: stdio
/// servers reduce to type/command/args/env/cwd (in that field order, absent
/// fields omitted); remote servers to type/url/headers. Unknown fields drop.
/// serde_json serializes fields in insertion order, matching Go's struct
/// field order; and unlike Go's encoder it never HTML-escapes, which is what
/// Go's marshalJSONStringifyCompatible had to work around.
fn marshal_cursor_mcp_approval_server(raw: &Value) -> anyhow::Result<String> {
    let obj = raw
        .as_object()
        .ok_or_else(|| anyhow!("approval server is not an object"))?;

    if let Some(command) = obj.get("command") {
        // Field order matters (Cursor normalizes before hashing): emit
        // type/command/args/env/cwd explicitly instead of relying on map
        // iteration order.
        let mut parts: Vec<String> = Vec::with_capacity(5);
        push_field(&mut parts, obj, "type");
        parts.push(format!("\"command\":{}", serde_json::to_string(command)?));
        push_field(&mut parts, obj, "args");
        push_field(&mut parts, obj, "env");
        push_field(&mut parts, obj, "cwd");
        return Ok(format!("{{{}}}", parts.join(",")));
    }
    if let Some(url) = obj.get("url") {
        let mut parts: Vec<String> = Vec::with_capacity(3);
        push_field(&mut parts, obj, "type");
        parts.push(format!("\"url\":{}", serde_json::to_string(url)?));
        push_field(&mut parts, obj, "headers");
        return Ok(format!("{{{}}}", parts.join(",")));
    }
    // Neither shape: emit the compacted original.
    serde_json::to_string(raw).map_err(Into::into)
}

fn push_field(parts: &mut Vec<String>, obj: &Map<String, Value>, key: &str) {
    // Go's omitempty on json.RawMessage omits only absent/empty values; a
    // present null is emitted as `"key":null`. Mirror that: include any key
    // that exists, whatever its value.
    if let Some(v) = obj.get(key) {
        if let Ok(json) = serde_json::to_string(v) {
            parts.push(format!("\"{key}\":{json}"));
        }
    }
}

/// cursor_project_root resolves the nearest ancestor of work_dir holding a
/// .git entry, falling back to the canonicalised workdir itself.
fn cursor_project_root(work_dir: &str) -> String {
    if work_dir.is_empty() {
        return work_dir.to_string();
    }
    let dir = std::fs::canonicalize(work_dir)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| work_dir.to_string());
    let fallback = dir.clone();
    let mut dir = dir;
    loop {
        if std::fs::metadata(join(&dir, ".git")).is_ok() {
            return dir;
        }
        let parent = match Path::new(&dir).parent() {
            Some(p) if !p.as_os_str().is_empty() && p != Path::new(&dir) => {
                p.to_string_lossy().into_owned()
            }
            _ => return fallback,
        };
        dir = parent;
    }
}

/// cursor_slugify_path maps every non-alphanumeric run to a single '-' with
/// leading/trailing dashes trimmed — Cursor's project-data slug.
pub(crate) fn cursor_slugify_path(path: &str) -> String {
    let mut b = String::with_capacity(path.len());
    let mut last_dash = false;
    for r in path.chars() {
        if r.is_ascii_alphanumeric() {
            b.push(r);
            last_dash = false;
            continue;
        }
        if !last_dash {
            b.push('-');
            last_dash = true;
        }
    }
    b.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of TestCursorSlugifyPath.
    #[test]
    fn test_cursor_slugify_path() {
        assert_eq!(
            cursor_slugify_path("/Users/alex/My Project"),
            "Users-alex-My-Project"
        );
        assert_eq!(cursor_slugify_path(""), "");
        assert_eq!(cursor_slugify_path("---"), "");
        assert_eq!(cursor_slugify_path("/a//b"), "a-b");
    }

    // Port of TestHasManagedCursorMcpConfig.
    #[test]
    fn test_has_managed_cursor_mcp_config() {
        assert!(!has_managed_cursor_mcp_config(None));
        assert!(!has_managed_cursor_mcp_config(Some(&Value::Null)));
        assert!(has_managed_cursor_mcp_config(Some(&json!({}))));
        assert!(has_managed_cursor_mcp_config(Some(
            &json!({"mcpServers": {}})
        )));
    }

    // Port of TestParseCursorManagedMcpServers validation arms.
    #[test]
    fn test_parse_cursor_managed_mcp_servers() {
        assert!(parse_cursor_managed_mcp_servers(&json!({}))
            .unwrap()
            .is_empty());
        assert!(
            parse_cursor_managed_mcp_servers(&json!({"mcpServers": null}))
                .unwrap()
                .is_empty()
        );
        assert!(parse_cursor_managed_mcp_servers(&json!({"mcpServers": {" ": {}}})).is_err());
        assert!(parse_cursor_managed_mcp_servers(&json!({"mcpServers": {"x": "notobj"}})).is_err());
        assert!(
            parse_cursor_managed_mcp_servers(&json!({"mcpServers": {"x": null}})).is_err(),
            "null server must be rejected as non-object"
        );
    }

    // Port of TestMarshalCursorMcpApprovalServer: stdio normalization keeps
    // only type/command/args/env/cwd; remote uses type/url/headers; unknown
    // shapes pass through compacted.
    #[test]
    fn test_marshal_cursor_mcp_approval_server() {
        let stdio = json!({
            "type": "stdio", "command": "npx", "args": ["-y", "x"],
            "env": {"A": "1"}, "cwd": "/w", "unknownField": true,
        });
        let got = marshal_cursor_mcp_approval_server(&stdio).unwrap();
        assert!(got.contains("\"command\":\"npx\""), "{got}");
        assert!(!got.contains("unknownField"), "{got}");
        // Field order matters for the hash: type precedes command.
        let t = got.find("\"type\"").unwrap();
        let c = got.find("\"command\"").unwrap();
        assert!(t < c, "{got}");

        let remote = json!({"url": "https://x", "headers": {"h": "v"}, "extra": 1});
        let got = marshal_cursor_mcp_approval_server(&remote).unwrap();
        assert!(got.contains("\"url\":\"https://x\""), "{got}");
        assert!(!got.contains("extra"), "{got}");

        let passthrough = json!({"weird": true});
        assert_eq!(
            marshal_cursor_mcp_approval_server(&passthrough).unwrap(),
            "{\"weird\":true}"
        );
    }

    // Port of TestCursorMcpApprovalKeys: deterministic name+hash entries.
    #[test]
    fn test_cursor_mcp_approval_keys() {
        let mut servers = BTreeMap::new();
        servers.insert("zeta".to_string(), json!({"command": "z"}));
        servers.insert("alpha".to_string(), json!({"command": "a"}));
        let keys = cursor_mcp_approval_keys("/proj", &servers).unwrap();
        assert_eq!(keys.len(), 2);
        for k in &keys {
            let (name, hash) = k.split_once('-').unwrap();
            assert!(matches!(name, "alpha" | "zeta"));
            assert_eq!(hash.len(), 16);
        }
    }

    // Port of TestResolveCursorMcpAuthSource.
    #[test]
    fn test_resolve_cursor_mcp_auth_source() {
        assert!(resolve_cursor_mcp_auth_source("").is_err());
        assert!(resolve_cursor_mcp_auth_source("relative/path").is_err());
        assert!(resolve_cursor_mcp_auth_source("~other/x").is_err());

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_string_lossy().to_string();

        // Directory pointing at the containing project dir resolves inside it.
        let auth = join(&root, CURSOR_MCP_AUTH_FILE);
        std::fs::write(&auth, b"{}").unwrap();
        assert_eq!(
            resolve_cursor_mcp_auth_source(&root).unwrap(),
            clean_lexical(&auth)
        );

        // Wrong basename refused.
        let wrong = join(&root, "nope.json");
        std::fs::write(&wrong, b"{}").unwrap();
        assert!(resolve_cursor_mcp_auth_source(&wrong).is_err());
    }

    // Port of TestCursorProjectRoot: walks up to the nearest .git ancestor.
    #[test]
    fn test_cursor_project_root() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let sub = repo.join("services/api");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        let root = cursor_project_root(sub.to_str().unwrap());
        // macOS /tmp is a symlink into /private/tmp — canonicalize both sides.
        let canon = |p: &str| {
            clean_lexical(
                std::fs::canonicalize(p)
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| p.to_string())
                    .as_str(),
            )
        };
        assert_eq!(canon(&root), canon(repo.to_str().unwrap()));

        // No git anywhere: falls back to the (canonicalised) workdir itself.
        let bare = tmp.path().join("plain");
        std::fs::create_dir_all(&bare).unwrap();
        let root = cursor_project_root(bare.to_str().unwrap());
        assert_eq!(
            clean_lexical(&root),
            clean_lexical(
                std::fs::canonicalize(bare.to_str().unwrap())
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| bare.to_string_lossy().into_owned())
                    .as_str()
            )
        );

        assert_eq!(cursor_project_root(""), "");
    }

    // Port of TestPrepareCursorMcpConfigNilIsNoop plus the happy-path sidecar
    // layout (mcp.json + approvals + workspace trust + data dir).
    #[test]
    fn test_prepare_cursor_mcp_config() {
        // Nil/absent mcp_config: no-op returning "".
        assert_eq!(
            prepare_cursor_mcp_config("/env", "/work", None, "", None).unwrap(),
            ""
        );
        assert_eq!(
            prepare_cursor_mcp_config("/env", "/work", Some(&Value::Null), "", None).unwrap(),
            ""
        );

        let tmp = tempfile::tempdir().unwrap();
        let env_root = tmp.path().join("env");
        let work_dir = tmp.path().join("work");
        std::fs::create_dir_all(&work_dir).unwrap();

        let cfg = json!({"mcpServers": {"fetcher": {"command": "npx", "args": ["-y","f"]}}});
        let data_dir = prepare_cursor_mcp_config(
            env_root.to_str().unwrap(),
            work_dir.to_str().unwrap(),
            Some(&cfg),
            "",
            None,
        )
        .unwrap();
        assert!(
            data_dir.starts_with(env_root.to_str().unwrap()),
            "{data_dir}"
        );

        let mcp_json = std::fs::read_to_string(join(
            &join(work_dir.to_str().unwrap(), ".cursor"),
            "mcp.json",
        ))
        .unwrap();
        assert!(mcp_json.contains("\"fetcher\""), "{mcp_json}");

        let slug = cursor_slugify_path(&cursor_project_root(work_dir.to_str().unwrap()));
        let proj_data = join(&join(&data_dir, "projects"), &slug);
        let approvals = std::fs::read_to_string(join(&proj_data, "mcp-approvals.json")).unwrap();
        assert!(approvals.contains("fetcher-"), "{approvals}");
        let trust =
            std::fs::read_to_string(join(&proj_data, CURSOR_WORKSPACE_TRUSTED_FILE)).unwrap();
        assert!(trust.contains("cordy-managed"), "{trust}");
    }

    // Port of TestPrepareCursorMcpConfigRefusesExistingSidecar.
    #[test]
    fn test_prepare_cursor_mcp_config_refuses_existing_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let env_root = tmp.path().join("env");
        let work_dir = tmp.path().join("work");
        std::fs::create_dir_all(work_dir.join(".cursor")).unwrap();
        std::fs::write(work_dir.join(".cursor").join("mcp.json"), b"user-owned").unwrap();

        let cfg = json!({"mcpServers": {"x": {"command": "y"}}});
        let mut m = SidecarManifest::default();
        let err = prepare_cursor_mcp_config(
            env_root.to_str().unwrap(),
            work_dir.to_str().unwrap(),
            Some(&cfg),
            "",
            Some(&mut m),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("would overwrite existing"),
            "{err:#}"
        );
        // The user's file is untouched.
        assert_eq!(
            std::fs::read_to_string(work_dir.join(".cursor").join("mcp.json")).unwrap(),
            "user-owned"
        );
    }
}
