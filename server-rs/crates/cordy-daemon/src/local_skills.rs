//! Port of `server/internal/daemon/local_skills.go` (702 lines).
//!
//! Deviations from Go:
//! - `skill.ParseSkillFrontmatter` / `skill.IsLikelyBinaryFilePath`
//!   (server/internal/skill) → local seam stand-ins in [`skillutil`]; an
//!   identical frontmatter port already exists in
//!   `cordy-service::plugin_skill`, but this crate does not depend on
//!   cordy-service and Cargo.toml is out of scope.
//! - `agent.BuiltinRuntimeByID` → [`builtin_runtime_user_skills_dir`] mirror
//!   of the descriptor registry's probe-relevant field (see agents_probe.rs).
//! - `filepath.WalkDir` → `walkdir` without symlink following (Go skips all
//!   symlinks after resolving the walk root via EvalSymlinks).
//! - `filepath.Clean`/`EvalSymlinks` → the shared lexical cleaner from
//!   skill_cache.rs and `std::fs::canonicalize`.
//! - slog → tracing with identical messages.

// S9-integration: consumed by manager/handlers wiring that lands with
// integration; silence dead-code until then.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::Serialize;
use walkdir::WalkDir;

use crate::skill_cache::go_path_clean;
use crate::types::SkillFileData;

/// `maxLocalSkillFileSize` (local_skills.go:19).
const MAX_LOCAL_SKILL_FILE_SIZE: i64 = 1 << 20;
/// `maxLocalSkillBundleSize` (local_skills.go:20).
const MAX_LOCAL_SKILL_BUNDLE_SIZE: i64 = 8 << 20;
/// `maxLocalSkillFileCount` (local_skills.go:24): kept in lockstep with the
/// server-side importer's maxImportFileCount; the 8 MiB bundle cap is the
/// real guard on skill size.
const MAX_LOCAL_SKILL_FILE_COUNT: usize = 256;
/// `maxLocalSkillDirDepth` (local_skills.go:29): caps how deep discovery
/// descends below a runtime root (opencode stores skills two levels deep).
const MAX_LOCAL_SKILL_DIR_DEPTH: usize = 4;

/// `runtimeLocalSkillSummary` (local_skills.go:32–51).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RuntimeLocalSkillSummary {
    #[serde(rename = "key")]
    pub key: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "description", skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(rename = "source_path")]
    pub source_path: String,
    #[serde(rename = "provider")]
    pub provider: String,
    /// Root classifies which discovery root surfaced this skill; older
    /// daemons omit the field and the server treats empty as "unknown"
    /// (local_skills.go:38–47).
    #[serde(rename = "root", skip_serializing_if = "String::is_empty")]
    pub root: String,
    #[serde(rename = "plugin", skip_serializing_if = "String::is_empty")]
    pub plugin: String,
    #[serde(rename = "can_disable", skip_serializing_if = "std::ops::Not::not")]
    pub can_disable: bool,
    #[serde(rename = "file_count")]
    pub file_count: usize,
}

/// `runtimeLocalSkillBundle` (local_skills.go:53–60).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RuntimeLocalSkillBundle {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "description", skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(rename = "content")]
    pub content: String,
    #[serde(rename = "source_path")]
    pub source_path: String,
    #[serde(rename = "provider")]
    pub provider: String,
    #[serde(rename = "files", skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<SkillFileData>,
}

/// `localSkillRoot` (local_skills.go:66–71): one discovery location plus a
/// classifier for where it came from.
#[derive(Debug, Clone)]
pub(crate) struct LocalSkillRoot {
    path: PathBuf,
    kind: &'static str,
    key_prefix: String,
    plugin: String,
}

/// `localSkillRootProvider` (local_skills.go:76): a runtime's own skill
/// directory; takes priority over the universal root.
pub(crate) const LOCAL_SKILL_ROOT_PROVIDER: &str = "provider";
/// `localSkillRootUniversal` (local_skills.go:81): the cross-tool
/// ~/.agents/skills root, always searched last so a same-key skill in the
/// provider directory keeps winning.
pub(crate) const LOCAL_SKILL_ROOT_UNIVERSAL: &str = "universal";
/// `localSkillRootPlugin` (local_skills.go:86): skills contributed by an
/// enabled runtime plugin, namespaced so keys match Claude Code.
pub(crate) const LOCAL_SKILL_ROOT_PLUGIN: &str = "plugin";

/// S9-integration mirror of `agent.BuiltinRuntime.UserSkillsDir`
/// (server/pkg/agent/builtin_runtimes.go:87–101); only omp exists today.
fn builtin_runtime_user_skills_dir(id: &str) -> Option<&'static str> {
    match id {
        "omp" => Some(".omp/agent/skills"),
        _ => None,
    }
}

/// `os.UserHomeDir`.
fn user_home_dir() -> anyhow::Result<String> {
    #[cfg(unix)]
    {
        std::env::var("HOME").context("resolve user home")
    }
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE").context("resolve user home")
    }
}

/// `localSkillRootsForProvider` (local_skills.go:125–269): ordered user-level
/// skill roots for a provider — provider-specific root first, then the
/// universal ~/.agents/skills root, then enabled Claude plugin roots. Returns
/// `Ok(None)` for providers with no local-skills surface.
pub(crate) fn local_skill_roots_for_provider(
    provider: &str,
) -> anyhow::Result<Option<Vec<LocalSkillRoot>>> {
    let home = user_home_dir().context("resolve user home")?;
    let home = Path::new(&home);

    let provider_root: PathBuf = if let Some(user_skills_dir) =
        builtin_runtime_user_skills_dir(provider)
    {
        // Built-in runtime identities declare their user skills dir in the
        // descriptor and fall through to the common construction below so
        // universal roots, merging, and fallback import all still apply.
        home.join(from_slash(user_skills_dir))
    } else {
        match provider {
            "claude" => home.join(".claude").join("skills"),
            // CodeBuddy Code is a Claude Code fork but ships its own native
            // config directory; it does NOT read ~/.claude/skills unless the
            // user manually symlinks it in (local_skills.go:142–151).
            "codebuddy" => home.join(".codebuddy").join("skills"),
            "codex" => {
                let codex_home = env_trim("CODEX_HOME");
                if codex_home.is_empty() {
                    home.join(".codex").join("skills")
                } else {
                    Path::new(&codex_home).join("skills")
                }
            }
            "copilot" => home.join(".copilot").join("skills"),
            "opencode" => home.join(".config").join("opencode").join("skills"),
            "deveco" => home.join(".config").join("deveco").join("skills"),
            "openclaw" => home.join(".openclaw").join("skills"),
            "pi" => home.join(".pi").join("agent").join("skills"),
            "cursor" => home.join(".cursor").join("skills"),
            "hermes" => home.join(".hermes").join("skills"),
            "kimi" => home.join(".kimi").join("skills"),
            "reasonix" => home_from_env_or(home, "REASONIX_HOME", ".reasonix").join("skills"),
            "dsh" => home_from_env_or(home, "DSH_HOME", ".dsh").join("skills"),
            "kiro" => home.join(".kiro").join("skills"),
            "qoder" => home.join(".qoder").join("skills"),
            "qoderclicn" => home.join(".qoder-cn").join("skills"),
            // Official TRAE CLI global skills live in ~/.traecli/skills
            // (https://docs.trae.cn/cli_skills).
            "traecli" => home.join(".traecli").join("skills"),
            // agy inherits Gemini CLI's global skill root.
            "antigravity" => home.join(".gemini").join("antigravity-cli").join("skills"),
            // GROK_HOME replaces the default ~/.grok home.
            "grok" => home_from_env_or(home, "GROK_HOME", ".grok").join("skills"),
            // QWEN_HOME replaces Qwen Code's global ~/.qwen directory.
            "qwen" => home_from_env_or(home, "QWEN_HOME", ".qwen").join("skills"),
            // QWENPAW_WORKING_DIR (or legacy COPAW_WORKING_DIR) overrides
            // QwenPaw's global ~/.qwenpaw directory; resolution order is
            // QWENPAW_WORKING_DIR → COPAW_WORKING_DIR → ~/.copaw (legacy,
            // when present) → ~/.qwenpaw (default).
            "qwenpaw" => {
                let mut qwenpaw_home = env_trim("QWENPAW_WORKING_DIR");
                if qwenpaw_home.is_empty() {
                    qwenpaw_home = env_trim("COPAW_WORKING_DIR");
                }
                if qwenpaw_home.is_empty() {
                    let legacy_copaw = home.join(".copaw");
                    if legacy_copaw.exists() {
                        legacy_copaw.join("skill_pool")
                    } else {
                        home.join(".qwenpaw").join("skill_pool")
                    }
                } else {
                    Path::new(&qwenpaw_home).join("skill_pool")
                }
            }
            // MCode's default data directory is ~/.minimax.
            "mcode" => home.join(".minimax").join("skills"),
            _ => return Ok(None),
        }
    };

    let mut roots = vec![
        LocalSkillRoot {
            path: provider_root,
            kind: LOCAL_SKILL_ROOT_PROVIDER,
            key_prefix: String::new(),
            plugin: String::new(),
        },
        LocalSkillRoot {
            path: home.join(".agents").join("skills"),
            kind: LOCAL_SKILL_ROOT_UNIVERSAL,
            key_prefix: String::new(),
            plugin: String::new(),
        },
    ];
    if provider == "claude" {
        for plugin in crate::claude_plugins::list_enabled_claude_plugins(&home.to_string_lossy()) {
            let manifest =
                crate::claude_plugins::read_claude_plugin_manifest(&plugin.install_path)
                    .unwrap_or_default();
            let default_skills = Path::new(&plugin.install_path).join("skills");
            for path in crate::claude_plugins::claude_plugin_component_paths(
                &plugin.install_path,
                manifest.skills.as_ref(),
                &[&default_skills.to_string_lossy()],
            ) {
                roots.push(LocalSkillRoot {
                    path: PathBuf::from(path),
                    kind: LOCAL_SKILL_ROOT_PLUGIN,
                    key_prefix: format!("{}:", plugin.name),
                    plugin: plugin.id.clone(),
                });
            }
        }
    }
    Ok(Some(roots))
}

/// Env-overridden home subdir helper shared by reasonix/dsh/grok/qwen.
fn home_from_env_or(home: &Path, env_key: &str, default_dir: &str) -> PathBuf {
    let overridden = env_trim(env_key);
    if overridden.is_empty() {
        home.join(default_dir)
    } else {
        PathBuf::from(overridden)
    }
}

fn env_trim(key: &str) -> String {
    std::env::var(key).unwrap_or_default().trim().to_string()
}

/// `filepath.FromSlash`.
fn from_slash(p: &str) -> PathBuf {
    PathBuf::from(p.replace('/', std::path::MAIN_SEPARATOR_STR))
}

/// `isIgnoredLocalSkillEntry` (local_skills.go:271–284).
pub(crate) fn is_ignored_local_skill_entry(name: &str) -> bool {
    if name.is_empty() || name.starts_with('.') {
        return true;
    }
    matches!(
        name.to_ascii_lowercase().as_str(),
        "license" | "license.md" | "license.txt"
    )
}

/// `normalizeLocalSkillKey` (local_skills.go:286–295).
pub(crate) fn normalize_local_skill_key(key: &str) -> anyhow::Result<String> {
    if key.trim().is_empty() {
        anyhow::bail!("skill key is required");
    }
    let cleaned = go_path_clean(key.trim());
    if cleaned == "." || cleaned.starts_with('/') || cleaned.starts_with("..") {
        anyhow::bail!("invalid skill key");
    }
    Ok(cleaned)
}

/// `relativizeHomePath` (local_skills.go:297–310).
pub(crate) fn relativize_home_path(path: &Path) -> String {
    let Ok(home) = user_home_dir() else {
        return to_slash(&path.to_string_lossy());
    };
    let path_str = path.to_string_lossy().into_owned();
    if path_str == home {
        return "~".to_string();
    }
    let prefix = format!("{home}{}", std::path::MAIN_SEPARATOR);
    if let Some(rest) = path_str.strip_prefix(&prefix) {
        return format!("~/{rest}");
    }
    to_slash(&path_str)
}

/// `filepath.ToSlash`.
fn to_slash(p: &str) -> String {
    p.replace(std::path::MAIN_SEPARATOR, "/")
}

/// `readLocalSkillMainFile` (local_skills.go:312–326).
fn read_local_skill_main_file(skill_dir: &Path) -> anyhow::Result<String> {
    let main_path = skill_dir.join("SKILL.md");
    let info = std::fs::metadata(&main_path)?;
    if info.len() as i64 > MAX_LOCAL_SKILL_FILE_SIZE {
        anyhow::bail!("SKILL.md exceeds {MAX_LOCAL_SKILL_FILE_SIZE} bytes");
    }
    let content = std::fs::read(&main_path)?;
    Ok(String::from_utf8_lossy(&content).into_owned())
}

/// `collectLocalSkillFiles` (local_skills.go:328–452): enumerate the
/// supporting files under `skill_dir`, skipping symlinks, ignored entries,
/// SKILL.md itself, oversized/binary/non-round-trippable payloads. When
/// `include_content` is false (discovery pass) paths are still fully read so
/// both passes agree on which files make up the bundle.
fn collect_local_skill_files(skill_dir: &Path, include_content: bool) -> anyhow::Result<Vec<SkillFileData>> {
    let mut files: Vec<SkillFileData> = Vec::new();
    let mut total_size: i64 = 0;

    // filepath.WalkDir does not follow a symlinked root, so when the runtime
    // root contains symlinks into a shared skill installer walking from the
    // symlink path enumerates zero children. Resolve the real path first so
    // the walk descends into the actual directory (local_skills.go:332–340).
    let walk_root = std::fs::canonicalize(skill_dir).unwrap_or_else(|_| skill_dir.to_path_buf());

    let mut entries: Vec<(PathBuf, String, i64)> = Vec::new();
    for entry in WalkDir::new(&walk_root).follow_links(false) {
        let Ok(entry) = entry else { continue };
        let path = entry.path().to_path_buf();
        if path == walk_root {
            continue;
        }
        let file_type = entry.file_type();
        if file_type.is_symlink() {
            // Go skips every symlink entry (its IsDir check inside the
            // symlink branch is unreachable for lstat-typed DirEntries).
            continue;
        }
        if file_type.is_dir() {
            if is_ignored_local_skill_entry(entry.file_name().to_string_lossy().as_ref()) {
                continue;
            }
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_ignored_local_skill_entry(&name) || name.eq_ignore_ascii_case("SKILL.md") {
            continue;
        }

        let Ok(rel) = path.strip_prefix(&walk_root) else { continue };
        let rel = rel.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
        if rel == "." || rel.starts_with("..") || Path::new(&rel).is_absolute() {
            continue;
        }

        let Ok(info) = entry.metadata() else { continue };
        if info.len() as i64 > MAX_LOCAL_SKILL_FILE_SIZE {
            continue;
        }
        entries.push((path, rel, info.len() as i64));
    }

    // Deterministic order matching Go's final sort by slash-rel path.
    entries.sort_by(|a, b| a.1.cmp(&b.1));

    for (path, rel, size) in entries {
        // A binary supporting file cannot survive SkillFileData.Content: the
        // bytes go out as a string and JSON rewrites every invalid UTF-8 byte
        // to U+FFFD. The extension blacklist is the cheap first pass; the
        // content read below is the actual guarantee (local_skills.go:378–411).
        if skillutil::is_likely_binary_file_path(&rel) {
            tracing::info!(
                skill_dir = %skill_dir.display(),
                path = %rel,
                size,
                reason = "binary_extension",
                "local skill: skipping binary file"
            );
            continue;
        }
        let Ok(content) = std::fs::read(&path) else { continue };
        // Require valid UTF-8 AND NUL-free: a NUL byte is legal UTF-8 but the
        // server-side import strips every 0x00, so such a file would come
        // back different from what went in (local_skills.go:406–411).
        if std::str::from_utf8(&content).is_err() || content.contains(&0u8) {
            let reason = if std::str::from_utf8(&content).is_ok() {
                "embedded_nul"
            } else {
                "invalid_utf8"
            };
            tracing::info!(
                skill_dir = %skill_dir.display(),
                path = %rel,
                size,
                reason,
                "local skill: skipping binary file"
            );
            continue;
        }
        if files.len() >= MAX_LOCAL_SKILL_FILE_COUNT {
            anyhow::bail!("local skill exceeds {MAX_LOCAL_SKILL_FILE_COUNT} files");
        }
        total_size += size;
        if total_size > MAX_LOCAL_SKILL_BUNDLE_SIZE {
            anyhow::bail!("local skill exceeds {MAX_LOCAL_SKILL_BUNDLE_SIZE} bytes in total");
        }

        let mut file = SkillFileData { path: rel, ..Default::default() };
        if include_content {
            file.content = String::from_utf8(content).unwrap_or_default();
        }
        files.push(file);
    }

    Ok(files)
}

/// `listRuntimeLocalSkills` (local_skills.go:454–511): merge every discovery
/// root in priority order, dedupe strictly by Key (first occurrence wins),
/// then sort once by Key.
pub(crate) fn list_runtime_local_skills(
    provider: &str,
) -> anyhow::Result<(Vec<RuntimeLocalSkillSummary>, bool)> {
    let Some(roots) = local_skill_roots_for_provider(provider)? else {
        return Ok((Vec::new(), false));
    };

    let mut skills: Vec<RuntimeLocalSkillSummary> = Vec::new();
    let mut seen_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
    for root in &roots {
        match std::fs::metadata(&root.path) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
            Ok(_) => {}
        }

        // Each root gets its OWN visited set: a user can deliberately expose
        // the same on-disk skill under two names by symlinking across roots,
        // and both must be listed (local_skills.go:485–491).
        let mut root_skills: Vec<RuntimeLocalSkillSummary> = Vec::new();
        let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        enumerate_local_skills(
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

/// `enumerateLocalSkills` (local_skills.go:513–609): walk `current_dir`
/// looking for skill directories (directories containing a SKILL.md); register
/// each at a key relative to `walk_root` and stop descending that branch.
/// `visited` keys on the resolved absolute path so cyclic symlinks can't loop.
#[allow(clippy::too_many_arguments)]
fn enumerate_local_skills(
    provider: &str,
    root: &LocalSkillRoot,
    walk_root: &Path,
    current_dir: &Path,
    depth: usize,
    visited: &mut std::collections::HashSet<PathBuf>,
    skills: &mut Vec<RuntimeLocalSkillSummary>,
) {
    if depth > MAX_LOCAL_SKILL_DIR_DEPTH {
        return;
    }
    let Ok(resolved) = std::fs::canonicalize(current_dir) else {
        return;
    };
    if !visited.insert(resolved) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(current_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_ignored_local_skill_entry(&name) {
            continue;
        }
        let path = current_dir.join(entry.file_name());
        // fs::metadata follows symlinks, like Go's os.Stat here.
        let Ok(info) = std::fs::metadata(&path) else { continue };
        if !info.is_dir() {
            continue;
        }

        let main_path = path.join("SKILL.md");
        if std::fs::metadata(&main_path).is_ok() {
            let Ok(rel) = path.strip_prefix(walk_root) else { continue };
            let rel = rel.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
            let Ok(mut key) = normalize_local_skill_key(&rel) else { continue };
            key = format!("{}{key}", root.key_prefix);

            let Ok(content) = read_local_skill_main_file(&path) else { continue };
            let (mut skill_name, description) = skillutil::parse_skill_frontmatter(&content);
            if !root.plugin.is_empty() {
                skill_name = key.clone();
            } else if skill_name.is_empty() {
                skill_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
            }

            let Ok(files) = collect_local_skill_files(&path, false) else { continue };

            // FileCount adds one back for SKILL.md itself: the supporting
            // bundle intentionally excludes it (it travels in Content), but
            // the list summary shows the user-facing total
            // (local_skills.go:596–601).
            skills.push(RuntimeLocalSkillSummary {
                key,
                name: skill_name,
                description,
                source_path: relativize_home_path(&path),
                provider: provider.to_string(),
                root: root.kind.to_string(),
                plugin: root.plugin.clone(),
                can_disable: provider == "codex" || provider == "claude",
                file_count: files.len() + 1,
            });
            continue;
        }

        // No SKILL.md here — descend looking for nested skills.
        enumerate_local_skills(provider, root, walk_root, &path, depth + 1, visited, skills);
    }
}

/// `loadRuntimeLocalSkillBundle` (local_skills.go:611–702): resolve a skill
/// key against the ordered roots. A root "has" the skill only when it is a
/// directory carrying a SKILL.md — the exact condition the list endpoint
/// registers on — so list/load always agree. Only a genuine IO/permission
/// fault aborts the search; not-found falls through to lower-priority roots.
pub(crate) fn load_runtime_local_skill_bundle(
    provider: &str,
    skill_key: &str,
) -> anyhow::Result<(Option<RuntimeLocalSkillBundle>, bool)> {
    let Some(roots) = local_skill_roots_for_provider(provider)? else {
        return Ok((None, false));
    };

    let key = normalize_local_skill_key(skill_key)?;

    for root in &roots {
        let mut root_key = key.as_str();
        if !root.key_prefix.is_empty() {
            let Some(stripped) = key.strip_prefix(root.key_prefix.as_str()) else {
                continue;
            };
            root_key = stripped;
        }
        let skill_dir = root.path.join(from_slash(root_key));
        let info = match std::fs::metadata(&skill_dir) {
            // NotFound => this root simply lacks the skill, try the next.
            // Any other stat error is returned as-is rather than silently
            // skipped, since skipping could load a DIFFERENT same-key skill
            // from a lower-priority root (Eve review #1).
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
            Ok(info) => info,
        };
        if !info.is_dir() {
            // Not a directory: the list endpoint never surfaces a non-dir as
            // a skill, so this root has no skill at this key.
            continue;
        }

        // A directory only counts as a skill when it actually contains a
        // SKILL.md; a same-key directory without one must not shadow a valid
        // lower-priority-root skill (local_skills.go:660–673).
        let main_path = skill_dir.join("SKILL.md");
        match std::fs::metadata(&main_path) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
            Ok(_) => {}
        }

        let content = read_local_skill_main_file(&skill_dir)?;
        let (mut name, description) = skillutil::parse_skill_frontmatter(&content);
        if !root.plugin.is_empty() {
            name = key.clone();
        } else if name.is_empty() {
            name = skill_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
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

    anyhow::bail!("local skill not found")
}

/// S9-integration seam stand-ins for `server/internal/skill`
/// (frontmatter.go + binary.go). Keep byte-compatible with
/// cordy-service::plugin_skill's port of the same code.
pub(crate) mod skillutil {
    use std::path::Path;
    use std::sync::LazyLock;

    /// `IsLikelyBinaryFilePath` (binary.go:19–40): conservative extension
    /// blacklist; unlisted extensions are assumed text.
    pub(crate) fn is_likely_binary_file_path(path: &str) -> bool {
        let ext = Path::new(path)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        matches!(
            ext.as_str(),
            // images
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "ico" | "heic"
            // fonts
            | "ttf" | "otf" | "woff" | "woff2" | "eot"
            // archives
            | "zip" | "gz" | "tar" | "bz2" | "7z" | "rar"
            // documents (binary office)
            | "pdf" | "docx" | "xlsx" | "pptx" | "doc" | "xls" | "ppt"
            // media
            | "mp3" | "mp4" | "wav" | "avi" | "mov" | "webm" | "m4a" | "flac"
            // compiled / executable
            | "exe" | "dll" | "so" | "dylib" | "class" | "jar" | "wasm"
            // db / cache
            | "db" | "sqlite" | "sqlite3" | "pyc"
        )
    }

    /// `ParseSkillFrontmatter` (frontmatter.go:27–48): extract name and
    /// description from the YAML frontmatter block; empty strings when absent
    /// or malformed. Values are coerced per key so a structured value in one
    /// field never discards a valid sibling key.
    pub(crate) fn parse_skill_frontmatter(content: &str) -> (String, String) {
        static FRONTMATTER_PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| {
            regex::Regex::new(r"(?s)\A---\r?\n(.*?\r?\n)---").expect("valid static regex")
        });
        if !content.starts_with("---") {
            return (String::new(), String::new());
        }
        let Some(caps) = FRONTMATTER_PATTERN.captures(content) else {
            return (String::new(), String::new());
        };
        let block = &caps[1];
        let Ok(fm) = serde_yaml::from_str::<serde_yaml::Value>(block) else {
            return (String::new(), String::new());
        };
        // Trimmed because both fields are single-line labels wherever they
        // are consumed, while YAML block scalars carry a trailing newline by
        // clip chomping (MUL-5645).
        (
            coerce_frontmatter_value(fm.get("name")).trim().to_string(),
            coerce_frontmatter_value(fm.get("description")).trim().to_string(),
        )
    }

    /// `coerceFrontmatterValue` (frontmatter.go:53–76): nil becomes empty,
    /// strings pass through, other scalars use their literal form, structured
    /// values are JSON-encoded.
    fn coerce_frontmatter_value(value: Option<&serde_yaml::Value>) -> String {
        match value {
            None | Some(serde_yaml::Value::Null) => String::new(),
            Some(serde_yaml::Value::String(s)) => s.clone(),
            Some(serde_yaml::Value::Bool(b)) => b.to_string(),
            Some(serde_yaml::Value::Number(n)) => n.to_string(),
            Some(other) => serde_json::to_string(other).unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes HOME/env mutation across tests (Go's t.Setenv semantics).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard(Vec<(String, Option<String>)>);
    impl EnvGuard {
        fn new(vars: &[(&str, &str)]) -> Self {
            let mut saved = Vec::new();
            for (key, value) in vars {
                saved.push((key.to_string(), std::env::var(key).ok()));
                std::env::set_var(key, value);
            }
            EnvGuard(saved)
        }
        fn home(home: &Path) -> Self {
            Self::new(&[("HOME", &home.to_string_lossy())])
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

    /// writeTestLocalSkill (local_skills_test.go:10–27).
    fn write_test_local_skill(root: &Path, rel: &str, files: &[(&str, &str)]) -> PathBuf {
        let skill_dir = root.join(from_slash(rel));
        std::fs::create_dir_all(&skill_dir).unwrap();
        for (path, content) in files {
            let full_path = skill_dir.join(from_slash(path));
            std::fs::create_dir_all(full_path.parent().unwrap()).unwrap();
            std::fs::write(full_path, content).unwrap();
        }
        skill_dir
    }

    /// writeTestClaudePlugin (local_skills_test.go:29–54).
    fn write_test_claude_plugin(home: &Path, id: &str, name: &str, enabled: bool) -> PathBuf {
        let install_path = home.join(".claude").join("plugins").join("cache").join(name).join("1.0.0");
        std::fs::create_dir_all(install_path.join(".claude-plugin")).unwrap();
        std::fs::write(
            install_path.join(".claude-plugin").join("plugin.json"),
            format!(r#"{{"name":"{name}","skills":"./skills","mcpServers":"./mcp.json"}}"#),
        )
        .unwrap();
        std::fs::create_dir_all(home.join(".claude").join("plugins")).unwrap();
        std::fs::write(
            home.join(".claude").join("plugins").join("installed_plugins.json"),
            format!(
                r#"{{"version":2,"plugins":{{"{id}":[{{"scope":"user","installPath":"{}"}}]}}}}"#,
                install_path.display()
            ),
        )
        .unwrap();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(
            home.join(".claude").join("settings.json"),
            format!(r#"{{"enabledPlugins":{{"{id}":{enabled}}}}}"#),
        )
        .unwrap();
        install_path
    }

    fn symlink_dir(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            let _ = (target, link);
            panic!("symlink test is POSIX-only")
        }
    }

    /// TestListRuntimeLocalSkills_Claude (local_skills_test.go:56–101).
    #[test]
    fn list_runtime_local_skills_claude() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".claude").join("skills"),
            "review-helper",
            &[
                ("SKILL.md", "---\nname: Review Helper\ndescription: Review pull requests\n---\n# Review Helper\n"),
                ("templates/check.md", "checklist"),
                ("LICENSE", "ignored"),
                (".secret", "ignored"),
            ],
        );

        let (skills, supported) = list_runtime_local_skills("claude").unwrap();
        assert!(supported);
        assert_eq!(skills.len(), 1);
        let skill = &skills[0];
        assert_eq!(skill.key, "review-helper");
        assert_eq!(skill.name, "Review Helper");
        assert_eq!(skill.description, "Review pull requests");
        // 2 = supporting file + SKILL.md itself.
        assert_eq!(skill.file_count, 2);
        assert_eq!(skill.source_path, "~/.claude/skills/review-helper");
        assert!(skill.can_disable);
    }

    /// TestListRuntimeLocalSkills_Mcode (local_skills_test.go:103–121).
    #[test]
    fn list_runtime_local_skills_mcode() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".minimax").join("skills"),
            "mcode-review",
            &[("SKILL.md", "---\nname: MCode Review\ndescription: Review code with MiniMax Code\n---\n")],
        );

        let (skills, supported) = list_runtime_local_skills("mcode").unwrap();
        assert!(supported);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].source_path, "~/.minimax/skills/mcode-review");
    }

    /// TestListRuntimeLocalSkills_Codebuddy (local_skills_test.go:130–167):
    /// CodeBuddy must discover from ~/.codebuddy/skills, not ~/.claude/skills.
    #[test]
    fn list_runtime_local_skills_codebuddy() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".codebuddy").join("skills"),
            "review-helper",
            &[("SKILL.md", "---\nname: CodeBuddy Review\ndescription: Review code with CodeBuddy\n---\n")],
        );
        write_test_local_skill(
            &home.join(".claude").join("skills"),
            "review-helper",
            &[("SKILL.md", "---\nname: Claude Review\n---\n")],
        );

        let (skills, supported) = list_runtime_local_skills("codebuddy").unwrap();
        assert!(supported);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].key, "review-helper");
        assert_eq!(skills[0].name, "CodeBuddy Review");
        assert_eq!(skills[0].source_path, "~/.codebuddy/skills/review-helper");
        assert!(!skills[0].can_disable);
    }

    /// TestRuntimeLocalSkills_CodebuddyExcludesClaudePluginSkills
    /// (local_skills_test.go:169–202).
    #[test]
    fn codebuddy_excludes_claude_plugin_skills() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".codebuddy").join("skills"),
            "codebuddy-only",
            &[("SKILL.md", "---\nname: CodeBuddy Only\n---\n")],
        );
        let install_path = write_test_claude_plugin(&home, "paper-desktop@paper", "paper-desktop", true);
        write_test_local_skill(
            &install_path.join("skills"),
            "design-to-code",
            &[("SKILL.md", "---\nname: Claude Plugin Skill\n---\n")],
        );

        let (skills, supported) = list_runtime_local_skills("codebuddy").unwrap();
        assert!(supported);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].key, "codebuddy-only");

        let result = load_runtime_local_skill_bundle("codebuddy", "paper-desktop:design-to-code");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "local skill not found");
    }

    /// TestListRuntimeLocalSkills_ClaudeEnabledPlugin
    /// (local_skills_test.go:204–234).
    #[test]
    fn claude_enabled_plugin() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        let install_path = write_test_claude_plugin(&home, "paper-desktop@paper", "paper-desktop", true);
        write_test_local_skill(
            &install_path.join("skills"),
            "design-to-code",
            &[("SKILL.md", "---\nname: Design to code\ndescription: Turn a design into code\n---\n")],
        );

        let (skills, supported) = list_runtime_local_skills("claude").unwrap();
        assert!(supported);
        assert_eq!(skills.len(), 1);
        let got = &skills[0];
        assert_eq!(got.key, "paper-desktop:design-to-code");
        assert_eq!(got.name, "paper-desktop:design-to-code");
        assert_eq!(got.root, LOCAL_SKILL_ROOT_PLUGIN);
        assert_eq!(got.plugin, "paper-desktop@paper");

        let (bundle, supported) = load_runtime_local_skill_bundle("claude", &got.key).unwrap();
        assert!(supported);
        let bundle = bundle.unwrap();
        assert_eq!(bundle.name, got.key);
        assert_eq!(bundle.description, "Turn a design into code");
    }

    /// TestListRuntimeLocalSkills_ClaudeDisabledPluginIsHidden
    /// (local_skills_test.go:236–251).
    #[test]
    fn claude_disabled_plugin_is_hidden() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        let install_path = write_test_claude_plugin(&home, "paper-desktop@paper", "paper-desktop", false);
        write_test_local_skill(
            &install_path.join("skills"),
            "design-to-code",
            &[("SKILL.md", "---\nname: Design to code\n---\n")],
        );

        let (skills, supported) = list_runtime_local_skills("claude").unwrap();
        assert!(supported);
        assert!(skills.is_empty());
    }

    /// TestListRuntimeLocalSkills_Kiro (local_skills_test.go:253–280).
    #[test]
    fn list_runtime_local_skills_kiro() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".kiro").join("skills"),
            "review-helper",
            &[("SKILL.md", "---\nname: Kiro Review\ndescription: Review code with Kiro\n---\n")],
        );

        let (skills, supported) = list_runtime_local_skills("kiro").unwrap();
        assert!(supported);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].key, "review-helper");
        assert_eq!(skills[0].name, "Kiro Review");
        assert_eq!(skills[0].source_path, "~/.kiro/skills/review-helper");
    }

    /// TestLocalSkills_DiscoversACPProviderRoots
    /// (local_skills_test.go:282–402): table over the provider-root mapping.
    #[test]
    fn discovers_acp_provider_roots() {
        let cases: [(&str, &str, &str, &str); 8] = [
            ("hermes", ".hermes/skills", "~/.hermes/skills/review-helper", "Hermes Review"),
            ("kimi", ".kimi/skills", "~/.kimi/skills/review-helper", "Kimi Review"),
            ("reasonix", ".reasonix/skills", "~/.reasonix/skills/review-helper", "Reasonix Review"),
            ("dsh", ".dsh/skills", "~/.dsh/skills/review-helper", "DSH Review"),
            ("qoder", ".qoder/skills", "~/.qoder/skills/review-helper", "Qoder Review"),
            ("qoderclicn", ".qoder-cn/skills", "~/.qoder-cn/skills/review-helper", "Qoder CN Review"),
            ("qwen", ".qwen/skills", "~/.qwen/skills/review-helper", "Qwen Review"),
            ("grok", ".grok/skills", "~/.grok/skills/review-helper", "Grok Review"),
        ];
        for (provider, root, want_path, want_name) in cases {
            let _env_lock = ENV_LOCK.lock().unwrap();
            let home = tempfile::tempdir().unwrap().keep();
            let mut vars: Vec<(String, Option<String>)> =
                vec![("HOME".to_string(), std::env::var("HOME").ok())];
            std::env::set_var("HOME", &home);
            for key in ["GROK_HOME", "QWEN_HOME", "REASONIX_HOME", "DSH_HOME"] {
                if provider == "grok" && key == "GROK_HOME"
                    || provider == "qwen" && key == "QWEN_HOME"
                    || provider == "reasonix" && key == "REASONIX_HOME"
                    || provider == "dsh" && key == "DSH_HOME"
                {
                    vars.push((key.to_string(), std::env::var(key).ok()));
                    std::env::remove_var(key);
                }
            }
            let guard = EnvGuard(vars);

            write_test_local_skill(
                &home.join(from_slash(root)),
                "review-helper",
                &[
                    ("SKILL.md", &format!("---\nname: {want_name}\ndescription: Review code\n---\n# Review\n")),
                    ("notes.md", "notes"),
                ],
            );

            let (skills, supported) = list_runtime_local_skills(provider).unwrap();
            assert!(supported, "{provider} should be supported");
            assert_eq!(skills.len(), 1, "{provider}");
            assert_eq!(skills[0].key, "review-helper", "{provider}");
            assert_eq!(skills[0].name, want_name, "{provider}");
            assert_eq!(skills[0].root, LOCAL_SKILL_ROOT_PROVIDER, "{provider}");
            assert_eq!(skills[0].source_path, want_path, "{provider}");

            let (bundle, supported) = load_runtime_local_skill_bundle(provider, "review-helper").unwrap();
            assert!(supported, "{provider} should be supported for import");
            let bundle = bundle.unwrap();
            assert_eq!(bundle.name, want_name, "{provider}");
            assert_eq!(bundle.source_path, want_path, "{provider}");
            assert_eq!(bundle.files.len(), 1, "{provider}");

            drop(guard);
        }
    }

    /// TestListRuntimeLocalSkills_GrokUsesGROKHOME
    /// (local_skills_test.go:404–437).
    #[test]
    fn grok_uses_grok_home() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let grok_home = tempfile::tempdir().unwrap().keep().join("custom-grok-home");
        let _guard = EnvGuard::new(&[
            ("HOME", &home.to_string_lossy()),
            ("GROK_HOME", &grok_home.to_string_lossy()),
        ]);

        write_test_local_skill(
            &grok_home.join("skills"),
            "review-helper",
            &[("SKILL.md", "---\nname: Grok Home Review\ndescription: Review code\n---\n")],
        );
        write_test_local_skill(
            &home.join(".grok").join("skills"),
            "wrong-home",
            &[("SKILL.md", "---\nname: Wrong Home\n---\n")],
        );

        let (skills, supported) = list_runtime_local_skills("grok").unwrap();
        assert!(supported);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].key, "review-helper");
        assert_eq!(
            skills[0].source_path,
            grok_home.join("skills").join("review-helper").to_string_lossy()
        );

        let (bundle, _) = load_runtime_local_skill_bundle("grok", "review-helper").unwrap();
        assert_eq!(bundle.unwrap().name, "Grok Home Review");
    }

    /// TestListRuntimeLocalSkills_QwenUsesQWENHOME
    /// (local_skills_test.go:439–472).
    #[test]
    fn qwen_uses_qwen_home() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let qwen_home = tempfile::tempdir().unwrap().keep().join("custom-qwen-home");
        let _guard = EnvGuard::new(&[
            ("HOME", &home.to_string_lossy()),
            ("QWEN_HOME", &qwen_home.to_string_lossy()),
        ]);

        write_test_local_skill(
            &qwen_home.join("skills"),
            "review-helper",
            &[("SKILL.md", "---\nname: Qwen Home Review\ndescription: Review code\n---\n")],
        );
        write_test_local_skill(
            &home.join(".qwen").join("skills"),
            "wrong-home",
            &[("SKILL.md", "---\nname: Wrong Home\n---\n")],
        );

        let (skills, supported) = list_runtime_local_skills("qwen").unwrap();
        assert!(supported);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].key, "review-helper");
        assert_eq!(
            skills[0].source_path,
            qwen_home.join("skills").join("review-helper").to_string_lossy()
        );

        let (bundle, _) = load_runtime_local_skill_bundle("qwen", "review-helper").unwrap();
        assert_eq!(bundle.unwrap().name, "Qwen Home Review");
    }

    /// TestListRuntimeLocalSkills_FollowsSymlinkedSkillDirs
    /// (local_skills_test.go:480–532).
    #[test]
    #[cfg(unix)]
    fn follows_symlinked_skill_dirs() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        let target = write_test_local_skill(
            &home.join(".agents").join("skills"),
            "lark-doc",
            &[
                ("SKILL.md", "---\nname: Lark Doc\ndescription: Drive lark docs\n---\n"),
                ("helper.md", "stub"),
            ],
        );
        let skills_root = home.join(".claude").join("skills");
        std::fs::create_dir_all(&skills_root).unwrap();
        symlink_dir(&target, &skills_root.join("lark-doc"));
        write_test_local_skill(
            &skills_root,
            "review-helper",
            &[("SKILL.md", "---\nname: Review Helper\n---\n")],
        );

        let (skills, supported) = list_runtime_local_skills("claude").unwrap();
        assert!(supported);
        assert_eq!(skills.len(), 2);
        let by_symlink = skills.iter().find(|s| s.key == "lark-doc").expect("symlinked skill missing");
        assert_eq!(by_symlink.name, "Lark Doc");
        // Source path is reported relative to the runtime root, not the
        // resolved target.
        assert_eq!(by_symlink.source_path, "~/.claude/skills/lark-doc");
    }

    /// TestListRuntimeLocalSkills_CodexUsesSharedCODEXHOME
    /// (local_skills_test.go:534–563).
    #[test]
    fn codex_uses_shared_codex_home() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let codex_home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::new(&[
            ("HOME", &home.to_string_lossy()),
            ("CODEX_HOME", &codex_home.to_string_lossy()),
        ]);

        write_test_local_skill(&codex_home.join("skills"), "debugger", &[("SKILL.md", "# Debugger\n")]);
        write_test_local_skill(
            &home.join(".codex").join("skills"),
            "wrong-home",
            &[("SKILL.md", "# Wrong Home\n")],
        );

        let (skills, supported) = list_runtime_local_skills("codex").unwrap();
        assert!(supported);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].key, "debugger");
        assert_eq!(
            skills[0].source_path,
            codex_home.join("skills").join("debugger").to_string_lossy()
        );
    }

    /// TestListRuntimeLocalSkills_DescendsIntoNestedSkillDirs
    /// (local_skills_test.go:575–612): nested SKILL.md inside an already-
    /// registered skill must NOT register as a separate skill.
    #[test]
    fn descends_into_nested_skill_dirs() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        let root = home.join(".config").join("opencode").join("skills");
        write_test_local_skill(
            &root,
            "top",
            &[
                ("SKILL.md", "---\nname: Top\n---\n"),
                ("templates/SKILL.md", "not a real skill"),
            ],
        );
        write_test_local_skill(
            &root,
            "release/reporter",
            &[("SKILL.md", "---\nname: Release Reporter\n---\n")],
        );

        let (skills, supported) = list_runtime_local_skills("opencode").unwrap();
        assert!(supported);
        let keys: Vec<String> = skills.iter().map(|s| s.key.clone()).collect();
        assert_eq!(keys, vec!["release/reporter", "top"]);
    }

    /// TestLoadRuntimeLocalSkillBundle_OpenCode
    /// (local_skills_test.go:614–649).
    #[test]
    fn load_bundle_opencode() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".config").join("opencode").join("skills"),
            "release/reporter",
            &[
                ("SKILL.md", "---\nname: Release Reporter\ndescription: Summarize release notes\n---\n"),
                ("docs/template.md", "template body"),
                ("examples/sample.md", "sample body"),
            ],
        );

        let (bundle, supported) =
            load_runtime_local_skill_bundle("opencode", "release/reporter").unwrap();
        assert!(supported);
        let bundle = bundle.unwrap();
        assert_eq!(bundle.name, "Release Reporter");
        assert_eq!(bundle.description, "Summarize release notes");
        assert_eq!(bundle.files.len(), 2);
        assert_eq!(bundle.files[0].path, "docs/template.md");
        assert_eq!(bundle.files[0].content, "template body");
        assert_eq!(bundle.files[1].path, "examples/sample.md");
        assert_eq!(bundle.files[1].content, "sample body");
        assert_eq!(bundle.source_path, "~/.config/opencode/skills/release/reporter");
    }

    /// TestListRuntimeLocalSkills_OpenClaw (local_skills_test.go:651–672).
    #[test]
    fn list_runtime_local_skills_openclaw() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".openclaw").join("skills"),
            "planner",
            &[("SKILL.md", "# Planner\n")],
        );

        let (skills, supported) = list_runtime_local_skills("openclaw").unwrap();
        assert!(supported);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].source_path, "~/.openclaw/skills/planner");
    }

    /// TestLoadRuntimeLocalSkillBundle_Cursor (local_skills_test.go:674–701).
    #[test]
    fn load_bundle_cursor() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".cursor").join("skills"),
            "docs-helper",
            &[
                ("SKILL.md", "---\nname: Docs Helper\n---\n"),
                ("notes/tips.md", "tips"),
                ("examples/a.txt", "example"),
                (".hidden/skip.txt", "ignore"),
            ],
        );

        let (bundle, supported) = load_runtime_local_skill_bundle("cursor", "docs-helper").unwrap();
        assert!(supported);
        let bundle = bundle.unwrap();
        assert_eq!(bundle.name, "Docs Helper");
        assert_eq!(bundle.files.len(), 2);
        assert_eq!(bundle.source_path, "~/.cursor/skills/docs-helper");
    }

    /// TestListRuntimeLocalSkills_DiscoversUniversalAgentsRoot
    /// (local_skills_test.go:709–744).
    #[test]
    fn discovers_universal_agents_root() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".agents").join("skills"),
            "universal-helper",
            &[
                ("SKILL.md", "---\nname: Universal Helper\ndescription: Cross-tool skill\n---\n"),
                ("docs/info.md", "info"),
            ],
        );

        let (skills, supported) = list_runtime_local_skills("claude").unwrap();
        assert!(supported);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].key, "universal-helper");
        assert_eq!(skills[0].name, "Universal Helper");
        assert_eq!(skills[0].root, LOCAL_SKILL_ROOT_UNIVERSAL);
        assert_eq!(skills[0].source_path, "~/.agents/skills/universal-helper");
        assert_eq!(skills[0].file_count, 2);
    }

    /// TestLoadRuntimeLocalSkillBundle_ImportsFromUniversalRoot
    /// (local_skills_test.go:749–775).
    #[test]
    fn imports_from_universal_root() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".agents").join("skills"),
            "shared-skill",
            &[
                ("SKILL.md", "---\nname: Shared Skill\ndescription: Imported from agents root\n---\n"),
                ("examples/use.md", "usage"),
                ("scripts/run.sh", "echo hi"),
            ],
        );

        let (bundle, supported) = load_runtime_local_skill_bundle("claude", "shared-skill").unwrap();
        assert!(supported);
        let bundle = bundle.unwrap();
        assert_eq!(bundle.name, "Shared Skill");
        assert_eq!(bundle.files.len(), 2);
        assert_eq!(bundle.source_path, "~/.agents/skills/shared-skill");
    }

    /// TestLocalSkills_ProviderRootWinsOnKeyConflict
    /// (local_skills_test.go:782–821).
    #[test]
    fn provider_root_wins_on_key_conflict() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".claude").join("skills"),
            "dup",
            &[("SKILL.md", "---\nname: Provider Copy\n---\n")],
        );
        write_test_local_skill(
            &home.join(".agents").join("skills"),
            "dup",
            &[("SKILL.md", "---\nname: Universal Copy\n---\n")],
        );

        let (skills, _) = list_runtime_local_skills("claude").unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "Provider Copy");
        assert_eq!(skills[0].root, LOCAL_SKILL_ROOT_PROVIDER);
        assert_eq!(skills[0].source_path, "~/.claude/skills/dup");

        let (bundle, _) = load_runtime_local_skill_bundle("claude", "dup").unwrap();
        let bundle = bundle.unwrap();
        assert_eq!(bundle.name, "Provider Copy");
        assert_eq!(bundle.source_path, "~/.claude/skills/dup");
    }

    /// TestListRuntimeLocalSkills_MergesBothRoots
    /// (local_skills_test.go:824–856).
    #[test]
    fn merges_both_roots() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".claude").join("skills"),
            "provider-only",
            &[("SKILL.md", "---\nname: Provider Only\n---\n")],
        );
        write_test_local_skill(
            &home.join(".agents").join("skills"),
            "universal-only",
            &[("SKILL.md", "---\nname: Universal Only\n---\n")],
        );

        let (skills, _) = list_runtime_local_skills("claude").unwrap();
        let keys: Vec<String> = skills.iter().map(|s| s.key.clone()).collect();
        assert_eq!(keys, vec!["provider-only", "universal-only"]);
        let roots: std::collections::HashMap<&str, &str> =
            skills.iter().map(|s| (s.key.as_str(), s.root.as_str())).collect();
        assert_eq!(roots["provider-only"], LOCAL_SKILL_ROOT_PROVIDER);
        assert_eq!(roots["universal-only"], LOCAL_SKILL_ROOT_UNIVERSAL);
    }

    /// TestListRuntimeLocalSkills_MissingUniversalRootIsNotAnError
    /// (local_skills_test.go:861–880).
    #[test]
    fn missing_universal_root_is_not_an_error() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".claude").join("skills"),
            "only-provider",
            &[("SKILL.md", "---\nname: Only Provider\n---\n")],
        );

        let (skills, supported) = list_runtime_local_skills("claude").unwrap();
        assert!(supported);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].key, "only-provider");
    }

    /// TestListRuntimeLocalSkills_BothRootsMissing
    /// (local_skills_test.go:883–897).
    #[test]
    fn both_roots_missing() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        let (skills, supported) = list_runtime_local_skills("claude").unwrap();
        assert!(supported);
        assert!(skills.is_empty());
    }

    /// TestListRuntimeLocalSkills_NestedSkillInUniversalRoot
    /// (local_skills_test.go:900–926).
    #[test]
    fn nested_skill_in_universal_root() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".agents").join("skills"),
            "release/reporter",
            &[("SKILL.md", "---\nname: Release Reporter\n---\n")],
        );

        let (skills, _) = list_runtime_local_skills("opencode").unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].key, "release/reporter");
        assert_eq!(skills[0].root, LOCAL_SKILL_ROOT_UNIVERSAL);

        let (bundle, _) = load_runtime_local_skill_bundle("opencode", "release/reporter").unwrap();
        assert_eq!(bundle.unwrap().name, "Release Reporter");
    }

    /// TestLoadRuntimeLocalSkillBundle_FallsThroughToUniversalOnNotExist
    /// (local_skills_test.go:930–952).
    #[test]
    fn falls_through_to_universal_on_not_exist() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".claude").join("skills"),
            "something-else",
            &[("SKILL.md", "---\nname: Something Else\n---\n")],
        );
        write_test_local_skill(
            &home.join(".agents").join("skills"),
            "only-universal",
            &[("SKILL.md", "---\nname: Only Universal\n---\n")],
        );

        let (bundle, _) = load_runtime_local_skill_bundle("claude", "only-universal").unwrap();
        let bundle = bundle.unwrap();
        assert_eq!(bundle.name, "Only Universal");
        assert_eq!(bundle.source_path, "~/.agents/skills/only-universal");
    }

    /// TestLoadRuntimeLocalSkillBundle_DoesNotMaskReadErrorWithUniversalFallback
    /// (local_skills_test.go:959–979): SKILL.md-as-directory makes the read
    /// fail; the error must surface instead of falling through.
    #[test]
    fn does_not_mask_read_error_with_universal_fallback() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        let clash_dir = home.join(".claude").join("skills").join("clash");
        std::fs::create_dir_all(clash_dir.join("SKILL.md")).unwrap();
        write_test_local_skill(
            &home.join(".agents").join("skills"),
            "clash",
            &[("SKILL.md", "---\nname: Universal Clash\n---\n")],
        );

        let result = load_runtime_local_skill_bundle("claude", "clash");
        assert!(result.is_err());
    }

    /// TestListRuntimeLocalSkills_PerRootVisitedAllowsCrossRootSymlinkAlias
    /// (local_skills_test.go:985–1021).
    #[test]
    #[cfg(unix)]
    fn per_root_visited_allows_cross_root_symlink_alias() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        let target = write_test_local_skill(
            &home.join(".agents").join("skills"),
            "foo",
            &[("SKILL.md", "---\nname: Foo\n---\n")],
        );
        let claude_root = home.join(".claude").join("skills");
        std::fs::create_dir_all(&claude_root).unwrap();
        symlink_dir(&target, &claude_root.join("bar"));

        let (skills, _) = list_runtime_local_skills("claude").unwrap();
        let keys: Vec<String> = skills.iter().map(|s| s.key.clone()).collect();
        assert_eq!(keys, vec!["bar", "foo"]);
        let roots: std::collections::HashMap<&str, &str> =
            skills.iter().map(|s| (s.key.as_str(), s.root.as_str())).collect();
        assert_eq!(roots["bar"], LOCAL_SKILL_ROOT_PROVIDER);
        assert_eq!(roots["foo"], LOCAL_SKILL_ROOT_UNIVERSAL);
    }

    /// TestLoadRuntimeLocalSkillBundle_ProviderDirWithoutSkillMdFallsThrough
    /// (local_skills_test.go:1031–1073): an existing-but-invalid provider dir
    /// must not shadow the universal skill; list and load must agree.
    #[test]
    fn provider_dir_without_skill_md_falls_through() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        write_test_local_skill(
            &home.join(".claude").join("skills"),
            "shadowed",
            &[("notes.md", "not a skill — no SKILL.md here")],
        );
        write_test_local_skill(
            &home.join(".agents").join("skills"),
            "shadowed",
            &[
                ("SKILL.md", "---\nname: Real Shadowed\ndescription: The valid one\n---\n"),
                ("docs/info.md", "info"),
            ],
        );

        let (skills, _) = list_runtime_local_skills("claude").unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].key, "shadowed");
        assert_eq!(skills[0].root, LOCAL_SKILL_ROOT_UNIVERSAL);
        assert_eq!(skills[0].source_path, "~/.agents/skills/shadowed");

        let (bundle, _) = load_runtime_local_skill_bundle("claude", "shadowed").unwrap();
        let bundle = bundle.unwrap();
        assert_eq!(bundle.name, "Real Shadowed");
        assert_eq!(bundle.source_path, "~/.agents/skills/shadowed");
    }

    /// TestLoadRuntimeLocalSkillBundle_ProviderNonDirFallsThrough
    /// (local_skills_test.go:1078–1102).
    #[test]
    fn provider_non_dir_falls_through() {
        let _env_lock = ENV_LOCK.lock().unwrap();
        let home = tempfile::tempdir().unwrap().keep();
        let _guard = EnvGuard::home(&home);

        let claude_root = home.join(".claude").join("skills");
        std::fs::create_dir_all(&claude_root).unwrap();
        std::fs::write(claude_root.join("filish"), b"x").unwrap();
        write_test_local_skill(
            &home.join(".agents").join("skills"),
            "filish",
            &[("SKILL.md", "---\nname: Filish\n---\n")],
        );

        let (bundle, _) = load_runtime_local_skill_bundle("claude", "filish").unwrap();
        let bundle = bundle.unwrap();
        assert_eq!(bundle.name, "Filish");
        assert_eq!(bundle.source_path, "~/.agents/skills/filish");
    }

    /// invalidUTF8Docx (local_skills_test.go:1104–1108).
    const INVALID_UTF8_DOCX: &[u8] = b"PK\x03\x04\x14\x00\x06\x00\xff\xfe\x80\x81payload";
    /// invalidUTF8Bytes (local_skills_test.go:1182).
    const INVALID_UTF8_BYTES: &[u8] = b"\xff\xfe\x00\x01payload\x80\x81";
    /// validUTF8WithNUL (local_skills_test.go:1242): UTF-16LE "Hello".
    const VALID_UTF8_WITH_NUL: &[u8] = b"H\x00e\x00l\x00l\x00o\x00";

    /// TestCollectLocalSkillFiles_SkipsBinarySupportingFiles
    /// (local_skills_test.go:1110–1137).
    #[test]
    fn collect_skips_binary_supporting_files() {
        let root = tempfile::tempdir().unwrap().keep();
        let skill_dir = write_test_local_skill(
            &root,
            "docs",
            &[
                ("SKILL.md", "---\nname: docs\n---\nbody\n"),
                ("references/notes.md", "# notes\n"),
            ],
        );
        // Binary payloads written as raw bytes (Go embeds them in string
        // literals; Rust test helpers take &str so write them directly).
        std::fs::create_dir_all(skill_dir.join("references")).unwrap();
        std::fs::write(skill_dir.join("references").join("report.docx"), INVALID_UTF8_DOCX).unwrap();
        std::fs::create_dir_all(skill_dir.join("assets")).unwrap();
        std::fs::write(skill_dir.join("assets").join("logo.png"), b"\x89PNG\r\n\x1a\n\x00\x00\x00").unwrap();

        let files = collect_local_skill_files(&skill_dir, true).unwrap();
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, vec!["references/notes.md"]);
        assert_eq!(files[0].content, "# notes\n");
    }

    /// TestCollectLocalSkillFiles_BinarySkipIsConsistentAcrossPasses
    /// (local_skills_test.go:1139–1178).
    #[test]
    fn collect_binary_skip_consistent_across_passes() {
        let root = tempfile::tempdir().unwrap().keep();
        let skill_dir = write_test_local_skill(
            &root,
            "docs",
            &[("SKILL.md", "---\nname: docs\n---\nbody\n"), ("references/notes.md", "# notes\n")],
        );
        std::fs::create_dir_all(skill_dir.join("references")).unwrap();
        std::fs::write(skill_dir.join("references").join("report.docx"), INVALID_UTF8_DOCX).unwrap();
        std::fs::create_dir_all(skill_dir.join("weights")).unwrap();
        std::fs::write(skill_dir.join("weights").join("model.safetensors"), INVALID_UTF8_BYTES).unwrap();

        let listed = collect_local_skill_files(&skill_dir, false).unwrap();
        let synced = collect_local_skill_files(&skill_dir, true).unwrap();
        let listed_paths: Vec<String> = listed.iter().map(|f| f.path.clone()).collect();
        let synced_paths: Vec<String> = synced.iter().map(|f| f.path.clone()).collect();
        assert_eq!(listed_paths, synced_paths, "passes must agree");
        assert_eq!(listed_paths, vec!["references/notes.md"]);
    }

    /// TestCollectLocalSkillFiles_SkipsUnlistedBinaryExtension
    /// (local_skills_test.go:1187–1210).
    #[test]
    fn collect_skips_unlisted_binary_extension() {
        let root = tempfile::tempdir().unwrap().keep();
        let skill_dir = write_test_local_skill(
            &root,
            "docs",
            &[("SKILL.md", "---\nname: docs\n---\nbody\n"), ("references/notes.md", "# notes\n")],
        );
        std::fs::create_dir_all(skill_dir.join("weights")).unwrap();
        std::fs::write(skill_dir.join("weights").join("model.safetensors"), INVALID_UTF8_BYTES).unwrap();
        std::fs::create_dir_all(skill_dir.join("data")).unwrap();
        std::fs::write(skill_dir.join("data").join("table.parquet"), INVALID_UTF8_BYTES).unwrap();
        std::fs::write(skill_dir.join("blob.bin"), INVALID_UTF8_BYTES).unwrap();

        let files = collect_local_skill_files(&skill_dir, true).unwrap();
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, vec!["references/notes.md"]);
    }

    /// TestCollectLocalSkillFiles_SkipsInvalidUTF8WithNoExtension
    /// (local_skills_test.go:1213–1234).
    #[test]
    fn collect_skips_invalid_utf8_with_no_extension() {
        let root = tempfile::tempdir().unwrap().keep();
        let skill_dir = write_test_local_skill(
            &root,
            "docs",
            &[("SKILL.md", "---\nname: docs\n---\nbody\n"), ("references/notes.md", "# notes\n")],
        );
        std::fs::create_dir_all(skill_dir.join("references")).unwrap();
        std::fs::write(skill_dir.join("references").join("README"), INVALID_UTF8_BYTES).unwrap();

        let files = collect_local_skill_files(&skill_dir, true).unwrap();
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, vec!["references/notes.md"]);
    }

    /// TestCollectLocalSkillFiles_SkipsUnlistedExtensionWithEmbeddedNUL
    /// (local_skills_test.go:1246–1267).
    #[test]
    fn collect_skips_unlisted_extension_with_embedded_nul() {
        let root = tempfile::tempdir().unwrap().keep();
        let skill_dir = write_test_local_skill(
            &root,
            "docs",
            &[("SKILL.md", "---\nname: docs\n---\nbody\n"), ("references/notes.md", "# notes\n")],
        );
        std::fs::create_dir_all(skill_dir.join("weights")).unwrap();
        std::fs::write(skill_dir.join("weights").join("model.dat"), VALID_UTF8_WITH_NUL).unwrap();

        let files = collect_local_skill_files(&skill_dir, true).unwrap();
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, vec!["references/notes.md"]);
    }

    /// TestCollectLocalSkillFiles_SkipsNoExtensionWithEmbeddedNUL
    /// (local_skills_test.go:1271–1292).
    #[test]
    fn collect_skips_no_extension_with_embedded_nul() {
        let root = tempfile::tempdir().unwrap().keep();
        let skill_dir = write_test_local_skill(
            &root,
            "docs",
            &[("SKILL.md", "---\nname: docs\n---\nbody\n"), ("references/notes.md", "# notes\n")],
        );
        std::fs::create_dir_all(skill_dir.join("references")).unwrap();
        std::fs::write(skill_dir.join("references").join("UTF16NAME"), VALID_UTF8_WITH_NUL).unwrap();

        let files = collect_local_skill_files(&skill_dir, true).unwrap();
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, vec!["references/notes.md"]);
    }

    /// TestCollectLocalSkillFiles_ValidUTF8TextPassesThroughUntouched
    /// (local_skills_test.go:1298–1320).
    #[test]
    fn collect_valid_utf8_text_passes_through_untouched() {
        let root = tempfile::tempdir().unwrap().keep();
        let body = "# Notes\n\nContém acentos e emoji 🎉 — não é binário.\n";
        let skill_dir = write_test_local_skill(
            &root,
            "docs",
            &[
                ("SKILL.md", "---\nname: docs\n---\nbody\n"),
                ("notes.txt", body),
                ("data.csv", "a,b,c\n1,2,3\n"),
                ("config", "key=value\n"),
            ],
        );

        let files = collect_local_skill_files(&skill_dir, true).unwrap();
        assert_eq!(files.len(), 3);
        let notes = files.iter().find(|f| f.path == "notes.txt").unwrap();
        assert_eq!(notes.content, body);
    }
}
