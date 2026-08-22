#![allow(dead_code)] // S9-integration: consumed by daemon.go core wiring (S8)
//! Port of `server/internal/daemon/local_skills.go` — discovery and import of
//! runtime-local skill directories (per-provider roots, the universal
//! `~/.agents/skills` root, and Claude plugin-contributed skills).
//!
//! Symbol map:
//! - `localSkillRootsForProvider` → [`local_skill_roots_for_provider`]
//! - `listRuntimeLocalSkills` → [`list_runtime_local_skills`]
//! - `enumerateLocalSkills` → inlined into [`walk_root_skills`]
//! - `loadRuntimeLocalSkillBundle` → [`load_runtime_local_skill_bundle`]
//! - `collectLocalSkillFiles` → [`collect_local_skill_files`]
//! - `readLocalSkillMainFile` → [`read_local_skill_main_file`]
//! - `normalizeLocalSkillKey` / `relativizeHomePath` /
//!   `isIgnoredLocalSkillEntry` → same-named functions
//! - `runtimeLocalSkillSummary` / `runtimeLocalSkillBundle` →
//!   [`RuntimeLocalSkillSummary`] / [`RuntimeLocalSkillBundle`]
//!
//! S9-integration: SKILL.md frontmatter parsing mirrors
//! `internal/skill.ParseSkillFrontmatter` (YAML name/description) via the
//! same fence + YAML decode shape used by cordy-service's plugin_skill port;
//! the binary-extension heuristic is the conservative blacklist from
//! `internal/skill/binary.go`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::claude_plugins::{
    claude_plugin_component_paths, list_enabled_claude_plugins, read_claude_plugin_manifest,
};
use crate::execenv::execenv::{join_path, user_home_dir};
use crate::types::SkillFileData;

pub(crate) const MAX_LOCAL_SKILL_FILE_SIZE: i64 = 1 << 20;
pub(crate) const MAX_LOCAL_SKILL_BUNDLE_SIZE: i64 = 8 << 20;
// Kept in lockstep with the server-side importer's maxImportFileCount so a
// skill that imports from a URL/archive also imports from a runtime-local
// directory. The 8 MiB bundle cap is the real guard on skill size.
pub(crate) const MAX_LOCAL_SKILL_FILE_COUNT: usize = 256;
// Cap how deep skill discovery descends below a runtime root.
pub(crate) const MAX_LOCAL_SKILL_DIR_DEPTH: usize = 4;

pub(crate) const LOCAL_SKILL_ROOT_PROVIDER: &str = "provider";
pub(crate) const LOCAL_SKILL_ROOT_UNIVERSAL: &str = "universal";
pub(crate) const LOCAL_SKILL_ROOT_PLUGIN: &str = "plugin";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeLocalSkillSummary {
    #[serde(rename = "key")]
    pub key: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(
        rename = "description",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub description: String,
    #[serde(rename = "source_path")]
    pub source_path: String,
    #[serde(rename = "provider")]
    pub provider: String,
    /// Root classifies which discovery root surfaced this skill ("provider"
    /// or "universal"); older daemons omit it and the server treats an empty
    /// value as "unknown".
    #[serde(rename = "root", skip_serializing_if = "String::is_empty", default)]
    pub root: String,
    #[serde(rename = "plugin", skip_serializing_if = "String::is_empty", default)]
    pub plugin: String,
    #[serde(
        rename = "can_disable",
        skip_serializing_if = "std::ops::Not::not",
        default
    )]
    pub can_disable: bool,
    #[serde(rename = "file_count")]
    pub file_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeLocalSkillBundle {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(
        rename = "description",
        skip_serializing_if = "String::is_empty",
        default
    )]
    pub description: String,
    #[serde(rename = "content")]
    pub content: String,
    #[serde(rename = "source_path")]
    pub source_path: String,
    #[serde(rename = "provider")]
    pub provider: String,
    #[serde(rename = "files", skip_serializing_if = "Vec::is_empty", default)]
    pub files: Vec<SkillFileData>,
}

/// `localSkillRoot`: one discovery location plus a classifier for where it
/// came from. Roots are returned in priority order.
#[derive(Debug, Clone)]
pub struct LocalSkillRoot {
    pub path: String,
    pub kind: &'static str,
    pub key_prefix: String,
    pub plugin: String,
}

/// Built-in runtime identities (e.g. "omp") declare their user skills dir in
/// the descriptor (pkg/agent/builtin_runtimes.go). Extend when more built-ins
/// are ported.
fn builtin_runtime_user_skills_dir(provider: &str) -> Option<&'static str> {
    match provider {
        "omp" => Some(".omp/agent/skills"),
        _ => None,
    }
}

/// `localSkillRootsForProvider` returns the ordered user-level skill roots
/// scanned for each runtime/provider: the provider-specific root first, then
/// the cross-tool universal root `~/.agents/skills`, then (claude only)
/// enabled-plugin skill directories. Returns `(roots, supported)`; supported
/// is false for providers with no local-skills surface.
pub(crate) fn local_skill_roots_for_provider(
    provider: &str,
) -> anyhow::Result<(Vec<LocalSkillRoot>, bool)> {
    let home = user_home_dir().map_err(|e| anyhow::anyhow!("resolve user home: {e:#}"))?;
    let provider_root: Option<String>;
    if let Some(dir) = builtin_runtime_user_skills_dir(provider) {
        provider_root = Some(join_path(&[&home, dir]));
    } else {
        match provider {
            "claude" => provider_root = Some(join_path(&[&home, ".claude", "skills"])),
            // CodeBuddy Code is a Claude Code fork but ships its own native
            // config directory; it does NOT read ~/.claude/skills unless the
            // user manually symlinks it in.
            "codebuddy" => provider_root = Some(join_path(&[&home, ".codebuddy", "skills"])),
            "codex" => {
                let codex_home = std::env::var("CODEX_HOME")
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let base = if codex_home.is_empty() {
                    join_path(&[&home, ".codex"])
                } else {
                    codex_home
                };
                provider_root = Some(join_path(&[&base, "skills"]));
            }
            "copilot" => provider_root = Some(join_path(&[&home, ".copilot", "skills"])),
            "opencode" => {
                provider_root = Some(join_path(&[&home, ".config", "opencode", "skills"]))
            }
            "deveco" => provider_root = Some(join_path(&[&home, ".config", "deveco", "skills"])),
            "openclaw" => provider_root = Some(join_path(&[&home, ".openclaw", "skills"])),
            "pi" => provider_root = Some(join_path(&[&home, ".pi", "agent", "skills"])),
            "cursor" => provider_root = Some(join_path(&[&home, ".cursor", "skills"])),
            "hermes" => provider_root = Some(join_path(&[&home, ".hermes", "skills"])),
            "kimi" => provider_root = Some(join_path(&[&home, ".kimi", "skills"])),
            "reasonix" => {
                let base = env_or("REASONIX_HOME", &home, ".reasonix");
                provider_root = Some(join_path(&[&base, "skills"]));
            }
            "dsh" => {
                let base = env_or("DSH_HOME", &home, ".dsh");
                provider_root = Some(join_path(&[&base, "skills"]));
            }
            "kiro" => provider_root = Some(join_path(&[&home, ".kiro", "skills"])),
            "qoder" => provider_root = Some(join_path(&[&home, ".qoder", "skills"])),
            "qoderclicn" => provider_root = Some(join_path(&[&home, ".qoder-cn", "skills"])),
            // Official TRAE CLI global skills live in ~/.traecli/skills.
            "traecli" => provider_root = Some(join_path(&[&home, ".traecli", "skills"])),
            // agy inherits Gemini CLI's global skill root.
            "antigravity" => {
                provider_root = Some(join_path(&[&home, ".gemini", "antigravity-cli", "skills"]))
            }
            "grok" => {
                let base = env_or("GROK_HOME", &home, ".grok");
                provider_root = Some(join_path(&[&base, "skills"]));
            }
            "qwen" => {
                let base = env_or("QWEN_HOME", &home, ".qwen");
                provider_root = Some(join_path(&[&base, "skills"]));
            }
            "qwenpaw" => {
                // QWENPAW_WORKING_DIR → COPAW_WORKING_DIR → ~/.copaw (legacy,
                // when present) → ~/.qwenpaw; the root is <home>/skill_pool.
                let mut qwenpaw_home = env_opt("QWENPAW_WORKING_DIR");
                if qwenpaw_home.is_none() {
                    qwenpaw_home = env_opt("COPAW_WORKING_DIR");
                }
                if qwenpaw_home.is_none() {
                    let legacy_copaw = join_path(&[&home, ".copaw"]);
                    if Path::new(&legacy_copaw).exists() {
                        qwenpaw_home = Some(legacy_copaw);
                    }
                }
                let base = qwenpaw_home.unwrap_or_else(|| join_path(&[&home, ".qwenpaw"]));
                provider_root = Some(join_path(&[&base, "skill_pool"]));
            }
            // MCode's default data directory is ~/.minimax; global skills live
            // directly below it.
            "mcode" => provider_root = Some(join_path(&[&home, ".minimax", "skills"])),
            _ => return Ok((Vec::new(), false)),
        }
    }

    let mut roots = vec![
        LocalSkillRoot {
            path: provider_root.expect("provider root set above"),
            kind: LOCAL_SKILL_ROOT_PROVIDER,
            key_prefix: String::new(),
            plugin: String::new(),
        },
        LocalSkillRoot {
            path: join_path(&[&home, ".agents", "skills"]),
            kind: LOCAL_SKILL_ROOT_UNIVERSAL,
            key_prefix: String::new(),
            plugin: String::new(),
        },
    ];
    if provider == "claude" {
        for plugin in list_enabled_claude_plugins(&home) {
            let manifest = read_claude_plugin_manifest(&plugin.install_path);
            let defaults = vec![join_path(&[&plugin.install_path, "skills"])];
            let raw = manifest
                .as_ref()
                .map(|m| m.skills_value().clone())
                .unwrap_or(serde_json::Value::Null);
            for path in claude_plugin_component_paths(&plugin.install_path, &raw, &defaults) {
                roots.push(LocalSkillRoot {
                    path,
                    kind: LOCAL_SKILL_ROOT_PLUGIN,
                    key_prefix: format!("{}:", plugin.name),
                    plugin: plugin.id.clone(),
                });
            }
        }
    }
    Ok((roots, true))
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn env_or(key: &str, home: &str, default_dir: &str) -> String {
    env_opt(key).unwrap_or_else(|| join_path(&[home, default_dir]))
}

/// `isIgnoredLocalSkillEntry`.
pub(crate) fn is_ignored_local_skill_entry(name: &str) -> bool {
    if name.is_empty() || name.starts_with('.') {
        return true;
    }
    matches!(
        name.to_ascii_lowercase().as_str(),
        "license" | "license.md" | "license.txt"
    )
}

/// `normalizeLocalSkillKey`.
pub(crate) fn normalize_local_skill_key(key: &str) -> anyhow::Result<String> {
    if key.trim().is_empty() {
        anyhow::bail!("skill key is required");
    }
    let cleaned = crate::execenv::execenv::clean_path(&key.trim().replace('\\', "/"));
    if cleaned == "." || cleaned.starts_with('/') || cleaned.starts_with("..") {
        anyhow::bail!("invalid skill key");
    }
    Ok(cleaned)
}

/// `relativizeHomePath`.
pub(crate) fn relativize_home_path(path: &str) -> String {
    let home = match user_home_dir() {
        Ok(h) => h,
        Err(_) => return path.replace('\\', "/"),
    };
    if path == home {
        return "~".to_string();
    }
    let prefix = format!("{home}/");
    if let Some(rest) = path.strip_prefix(&prefix) {
        return format!("~/{rest}");
    }
    path.replace('\\', "/")
}

/// `readLocalSkillMainFile`.
pub(crate) fn read_local_skill_main_file(skill_dir: &str) -> anyhow::Result<String> {
    let main_path = join_path(&[skill_dir, "SKILL.md"]);
    let meta = std::fs::metadata(&main_path).map_err(|e| anyhow::anyhow!("{e}"))?;
    if meta.len() as i64 > MAX_LOCAL_SKILL_FILE_SIZE {
        anyhow::bail!("SKILL.md exceeds {} bytes", MAX_LOCAL_SKILL_FILE_SIZE);
    }
    let content = std::fs::read_to_string(&main_path).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(content)
}

/// `collectLocalSkillFiles`: walks `skill_dir` (resolving a symlinked root so
/// shared installers enumerate), skipping ignored entries and SKILL.md itself,
/// rejecting binary/non-UTF-8/NUL-bearing files, and enforcing the per-file,
/// file-count, and total-size caps. `include_content=false` is the discovery
/// pass; both passes must agree on which files make up the bundle.
pub(crate) fn collect_local_skill_files(
    skill_dir: &str,
    include_content: bool,
) -> anyhow::Result<Vec<SkillFileData>> {
    let mut files: Vec<SkillFileData> = Vec::new();
    let mut total_size: i64 = 0;

    // WalkDir does not follow a symlinked root; resolve the real path first
    // so the walk descends into the actual directory.
    let walk_root = std::fs::canonicalize(skill_dir)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| skill_dir.to_string());

    let mut stack = vec![(walk_root.clone(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            // Symlinked directories are skipped entirely (Go: filepath.SkipDir);
            // symlinked files are skipped too. `entry.metadata()` follows links,
            // so check the file type without following.
            if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
                continue;
            }
            if meta.is_dir() {
                if depth >= MAX_LOCAL_SKILL_DIR_DEPTH * 8 || is_ignored_local_skill_entry(&name) {
                    continue;
                }
                stack.push((path.to_string_lossy().into_owned(), depth + 1));
                continue;
            }
            if is_ignored_local_skill_entry(&name) || name.eq_ignore_ascii_case("SKILL.md") {
                continue;
            }

            let rel_raw = match path.strip_prefix(&walk_root) {
                Ok(r) => r.to_string_lossy().into_owned(),
                Err(_) => continue,
            };
            let rel = crate::execenv::execenv::clean_path(&rel_raw);
            if rel == "." || rel.starts_with('/') || rel.starts_with("..") {
                continue;
            }
            if meta.len() as i64 > MAX_LOCAL_SKILL_FILE_SIZE {
                continue;
            }
            if is_likely_binary_file_path(&rel) {
                tracing::info!(
                    skill_dir = %skill_dir,
                    path = %rel,
                    size = meta.len(),
                    reason = "binary_extension",
                    "local skill: skipping binary file"
                );
                continue;
            }
            let content = match std::fs::read(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            // Valid UTF-8 alone is not enough: a NUL byte is legal UTF-8 but
            // the server-side import strips every 0x00, so require both.
            let text = match String::from_utf8(content) {
                Ok(t) => t,
                Err(_) => {
                    tracing::info!(
                        skill_dir = %skill_dir, path = %rel, size = meta.len(),
                        reason = "invalid_utf8",
                        "local skill: skipping binary file"
                    );
                    continue;
                }
            };
            if text.contains('\0') {
                tracing::info!(
                    skill_dir = %skill_dir, path = %rel, size = meta.len(),
                    reason = "embedded_nul",
                    "local skill: skipping binary file"
                );
                continue;
            }
            if files.len() >= MAX_LOCAL_SKILL_FILE_COUNT {
                anyhow::bail!("local skill exceeds {} files", MAX_LOCAL_SKILL_FILE_COUNT);
            }
            total_size += meta.len() as i64;
            if total_size > MAX_LOCAL_SKILL_BUNDLE_SIZE {
                anyhow::bail!(
                    "local skill exceeds {} bytes in total",
                    MAX_LOCAL_SKILL_BUNDLE_SIZE
                );
            }
            files.push(SkillFileData {
                path: rel,
                content: if include_content { text } else { String::new() },
                ..Default::default()
            });
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// `skill.IsLikelyBinaryFilePath` (internal/skill/binary.go): conservative
/// extension blacklist; extensions not listed are assumed text.
pub(crate) fn is_likely_binary_file_path(path: &str) -> bool {
    static BINARY_EXTS: OnceLock<HashSet<&'static str>> = OnceLock::new();
    let exts = BINARY_EXTS.get_or_init(|| {
        [
            // images
            ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".tiff", ".ico", ".heic",
            // fonts
            ".ttf", ".otf", ".woff", ".woff2", ".eot", // archives
            ".zip", ".gz", ".tar", ".bz2", ".7z", ".rar", // documents (binary office)
            ".pdf", ".docx", ".xlsx", ".pptx", ".doc", ".xls", ".ppt", // media
            ".mp3", ".mp4", ".wav", ".avi", ".mov", ".webm", ".m4a", ".flac",
            // compiled / executable
            ".exe", ".dll", ".so", ".dylib", ".class", ".jar", ".wasm", // db / cache
            ".db", ".sqlite", ".sqlite3", ".pyc",
        ]
        .into_iter()
        .collect()
    });
    let lower = path.to_ascii_lowercase();
    lower
        .rfind('.')
        .and_then(|i| exts.get(&lower[i..]))
        .is_some()
}

/// `ParseSkillFrontmatter` (internal/skill/frontmatter.go): returns
/// `(name, description)` from the leading YAML frontmatter block.
pub(crate) fn parse_skill_frontmatter(content: &str) -> (String, String) {
    static NAME_RE: OnceLock<Regex> = OnceLock::new();
    static DESC_RE: OnceLock<Regex> = OnceLock::new();
    let name_re = NAME_RE.get_or_init(|| Regex::new(r"(?m)^name:\s*(.+?)\s*$").expect("regex"));
    let desc_re =
        DESC_RE.get_or_init(|| Regex::new(r"(?m)^description:\s*(.+?)\s*$").expect("regex"));

    let rest = match content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
    {
        Some(r) => r,
        None => return (String::new(), String::new()),
    };
    let end = match rest.find("\n---") {
        Some(i) => i,
        None => return (String::new(), String::new()),
    };
    let block = &rest[..end];
    let name = name_re
        .captures(block)
        .map(|c| unquote_yaml_scalar(&c[1]))
        .unwrap_or_default();
    let description = desc_re
        .captures(block)
        .map(|c| unquote_yaml_scalar(&c[1]))
        .unwrap_or_default();
    (name, description)
}

fn unquote_yaml_scalar(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')))
    {
        return v[1..v.len() - 1].to_string();
    }
    v.to_string()
}

/// `listRuntimeLocalSkills`: walk each root in priority order with symlink
/// following at every level and nested layouts allowed; dedupe strictly by
/// Key (first occurrence wins); sort once by Key after merging.
pub(crate) fn list_runtime_local_skills(
    provider: &str,
) -> anyhow::Result<(Vec<RuntimeLocalSkillSummary>, bool)> {
    let (roots, supported) = local_skill_roots_for_provider(provider)?;
    if !supported {
        return Ok((Vec::new(), false));
    }

    let mut skills: Vec<RuntimeLocalSkillSummary> = Vec::new();
    let mut seen_keys: HashSet<String> = HashSet::new();
    for root in &roots {
        if let Err(err) = std::fs::metadata(&root.path) {
            if err.kind() == std::io::ErrorKind::NotFound {
                continue;
            }
            return Err(anyhow::anyhow!("{err}"));
        }

        // Each root gets its OWN visited set: a user can deliberately expose
        // the same on-disk skill under two names by symlinking across roots.
        let mut root_skills: Vec<RuntimeLocalSkillSummary> = Vec::new();
        let mut visited: HashMap<String, bool> = HashMap::new();
        walk_root_skills(
            provider,
            root,
            &root.path,
            &root.path,
            0,
            &mut visited,
            &mut root_skills,
        );

        for s in root_skills {
            if seen_keys.contains(&s.key) {
                continue;
            }
            seen_keys.insert(s.key.clone());
            skills.push(s);
        }
    }

    skills.sort_by(|a, b| a.key.cmp(&b.key));
    Ok((skills, true))
}

/// `enumerateLocalSkills`: register any directory carrying a SKILL.md at a key
/// relative to the walk root, and never descend past a registered skill even
/// when it contains nested candidates of its own. `visited` keys on the
/// resolved absolute path so a cyclic symlink cannot loop forever.
#[allow(clippy::too_many_arguments)]
fn walk_root_skills(
    provider: &str,
    root: &LocalSkillRoot,
    walk_root: &str,
    current_dir: &str,
    depth: usize,
    visited: &mut HashMap<String, bool>,
    skills: &mut Vec<RuntimeLocalSkillSummary>,
) {
    if depth > MAX_LOCAL_SKILL_DIR_DEPTH {
        return;
    }
    let resolved = match std::fs::canonicalize(current_dir) {
        Ok(p) => p.to_string_lossy().into_owned(),
        Err(_) => return,
    };
    if visited.contains_key(&resolved) {
        return;
    }
    visited.insert(resolved, true);

    let entries = match std::fs::read_dir(current_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_ignored_local_skill_entry(&name) {
            continue;
        }
        let path = join_path(&[current_dir, &name]);
        // stat follows symlinks (Go os.Stat).
        let Ok(info) = std::fs::metadata(&path) else {
            continue;
        };
        if !info.is_dir() {
            continue;
        }

        let main_path = join_path(&[&path, "SKILL.md"]);
        if std::fs::metadata(&main_path).is_ok() {
            let rel = match relative_rel(walk_root, &path) {
                Some(r) => r,
                None => continue,
            };
            let mut key = match normalize_local_skill_key(&rel) {
                Ok(k) => k,
                Err(_) => continue,
            };
            key = format!("{}{}", root.key_prefix, key);

            let content = match read_local_skill_main_file(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let (mut skill_name, description) = parse_skill_frontmatter(&content);
            if !root.plugin.is_empty() {
                skill_name = key.clone();
            } else if skill_name.is_empty() {
                skill_name = path.rsplit('/').next().unwrap_or(&path).to_string();
            }

            let files = match collect_local_skill_files(&path, false) {
                Ok(f) => f,
                Err(_) => continue,
            };

            skills.push(RuntimeLocalSkillSummary {
                key,
                name: skill_name,
                description,
                source_path: relativize_home_path(&path),
                provider: provider.to_string(),
                root: root.kind.to_string(),
                plugin: root.plugin.clone(),
                can_disable: provider == "codex" || provider == "claude",
                // `files` excludes SKILL.md; the summary reports the total.
                file_count: files.len() + 1,
            });
            continue;
        }

        // No SKILL.md here — descend looking for nested skills.
        walk_root_skills(provider, root, walk_root, &path, depth + 1, visited, skills);
    }
}

/// Go's filepath.Rel restricted to the case this module needs: `target`
/// beneath `base`. Returns the slash-normalised relative path.
fn relative_rel(base: &str, target: &str) -> Option<String> {
    let b = base.trim_end_matches('/');
    target
        .strip_prefix(b)
        .map(|rest| rest.trim_start_matches('/').to_string())
}

/// `loadRuntimeLocalSkillBundle`: walk the roots in the same priority order as
/// the list endpoint so import resolves to exactly the skill the list showed.
/// Only a genuine IO/permission fault is returned as an error; anything that
/// is not a valid skill at this key means "this root doesn't have it".
pub(crate) fn load_runtime_local_skill_bundle(
    provider: &str,
    skill_key: &str,
) -> anyhow::Result<(Option<RuntimeLocalSkillBundle>, bool)> {
    let (roots, supported) = local_skill_roots_for_provider(provider)?;
    if !supported {
        return Ok((None, false));
    }

    let key = normalize_local_skill_key(skill_key)?;

    for root in &roots {
        let root_key = if root.key_prefix.is_empty() {
            key.clone()
        } else {
            match key.strip_prefix(root.key_prefix.as_str()) {
                Some(k) => k.to_string(),
                None => continue,
            }
        };
        let skill_dir = join_path(&[&root.path, &root_key]);
        let info = match std::fs::metadata(&skill_dir) {
            // IsNotExist => try the next root; other stat faults are returned.
            Ok(i) => i,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(anyhow::anyhow!("{err}")),
        };
        if !info.is_dir() {
            continue;
        }

        // A directory counts as a skill only when it carries a SKILL.md.
        let main_path = join_path(&[&skill_dir, "SKILL.md"]);
        match std::fs::metadata(&main_path) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(anyhow::anyhow!("{err}")),
        }

        let content = read_local_skill_main_file(&skill_dir)?;
        let (mut name, description) = parse_skill_frontmatter(&content);
        if !root.plugin.is_empty() {
            name = key.clone();
        } else if name.is_empty() {
            name = skill_dir
                .rsplit('/')
                .next()
                .unwrap_or(&skill_dir)
                .to_string();
        }

        let files = collect_local_skill_files(&skill_dir, true)?;
        return Ok((
            Some(RuntimeLocalSkillBundle {
                name,
                description,
                content,
                source_path: relativize_home_path(&skill_dir),
                provider: provider.to_string(),
                files,
            }),
            true,
        ));
    }

    Err(anyhow::anyhow!("local skill not found"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> String {
        let d = std::env::temp_dir().join(format!(
            "localskills-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().into_owned()
    }

    #[test]
    fn ignores_entries_rules() {
        assert!(is_ignored_local_skill_entry(""));
        assert!(is_ignored_local_skill_entry(".git"));
        assert!(is_ignored_local_skill_entry("LICENSE"));
        assert!(is_ignored_local_skill_entry("license.txt"));
        assert!(!is_ignored_local_skill_entry("deploy"));
    }

    #[test]
    fn normalizes_keys() {
        assert_eq!(normalize_local_skill_key("a/b").unwrap(), "a/b");
        assert_eq!(normalize_local_skill_key("./a//b/../c").unwrap(), "a/c");
        assert!(normalize_local_skill_key("  ").is_err());
        assert!(normalize_local_skill_key("/abs").is_err());
        assert!(normalize_local_skill_key("../up").is_err());
    }

    #[test]
    fn binary_extension_detection() {
        assert!(is_likely_binary_file_path("x/a.png"));
        assert!(is_likely_binary_file_path("x/A.PNG"));
        assert!(!is_likely_binary_file_path("x/a.md"));
        assert!(!is_likely_binary_file_path("noext"));
        assert!(is_likely_binary_file_path("x/.png")); // Go Ext(".png") == ".png"
    }

    #[test]
    fn frontmatter_parsing() {
        let (name, desc) =
            parse_skill_frontmatter("---\nname: deploy\ndescription: Deploy things\n---\n\nbody\n");
        assert_eq!(name, "deploy");
        assert_eq!(desc, "Deploy things");

        let (name2, desc2) = parse_skill_frontmatter(
            "---\r\nname: \"quoted\"\r\ndescription: 'single'\r\n---\r\nx\r\n",
        );
        assert_eq!(name2, "quoted");
        assert_eq!(desc2, "single");

        let (n3, d3) = parse_skill_frontmatter("no frontmatter");
        assert_eq!((n3, d3), (String::new(), String::new()));
    }

    #[test]
    fn collect_files_skips_ignored_and_counts_caps() {
        let root = tmp("collect");
        let skill = join_path(&[&root, "my-skill"]);
        fs::create_dir_all(join_path(&[&skill, "sub"])).unwrap();
        fs::write(join_path(&[&skill, "SKILL.md"]), "# hi").unwrap();
        fs::write(join_path(&[&skill, "sub", "helper.py"]), "print(1)").unwrap();
        fs::write(join_path(&[&skill, "logo.png"]), "binary").unwrap();
        fs::write(join_path(&[&skill, "LICENSE"]), "MIT").unwrap();

        let files = collect_local_skill_files(&skill, false).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "sub/helper.py");
        assert_eq!(files[0].content, "");
        assert_eq!(files[0].size_bytes, 0);

        let files = collect_local_skill_files(&skill, true).unwrap();
        assert_eq!(files[0].content, "print(1)");
    }

    #[test]
    fn rejects_nul_and_invalid_utf8() {
        let root = tmp("nul");
        let skill = join_path(&[&root, "s"]);
        fs::create_dir_all(&skill).unwrap();
        fs::write(join_path(&[&skill, "bad.bin"]), b"\xff\xfe\x00").unwrap();

        let files = collect_local_skill_files(&skill, true).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn unsupported_provider_returns_supported_false() {
        let (_, supported) = list_runtime_local_skills("nonexistent-provider-xyz").expect("ok");
        assert!(!supported);
    }

    #[test]
    fn load_missing_bundle_errors() {
        let (bundle, _) =
            load_runtime_local_skill_bundle("nonexistent-provider-xyz", "whatever").unwrap();
        assert!(bundle.is_none());
    }
}
