//! Hermes provider workspace contract.
//!
//! This is the Rust capability port of Go's `hermes_home.go`,
//! `hermes_memory.go`, and `hermes_sessions.go`.  Hermes only discovers
//! skills from `HERMES_HOME`, so a task with bound skills receives a private
//! overlay which mirrors the user's home, derives a config/.env, and mounts
//! only the task's skills.  Memory and ACP session stores are keyed outside
//! the task root and linked into the overlay when the daemon resolved a
//! persistent store for the agent/conversation.
//!
//! The shared home is never modified.  All overlay writes are atomic where a
//! file can contain credentials, and a failed mount never destroys the
//! task-local source before a replacement link has been proven possible.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use anyhow::{anyhow, bail, Context};
use serde_yaml::{Mapping, Value};

use super::context::write_skill_files;
use super::execenv::{SkillContextForEnv, TaskContextForEnv};

const TASK_LOCAL_STATE_MARKER: &str = ".cordy-task-local-state-v1";
const SESSION_DB: &str = "state.db";
const SESSION_LINK_STAGING: &str = ".cordy-session-link";

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct HermesSessions {
    pub mounted: bool,
    pub history_present: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HermesProfileResolution {
    pub source_home: String,
    pub must_exist: bool,
    pub error: Option<String>,
}

/// Resolve the profile home using the same precedence as Hermes: an explicit
/// custom `HERMES_HOME`, an already profile-scoped home, then active_profile,
/// otherwise the platform default.  Invalid explicit names fail closed.
pub fn resolve_hermes_profile(custom_home: &str, profile: Option<&str>) -> HermesProfileResolution {
    let mut base = nonempty(custom_home)
        .or_else(|| std::env::var("HERMES_HOME").ok().and_then(|v| nonempty(&v)))
        .unwrap_or_else(platform_default_home);
    base = absolute_clean(&base);
    let root = hermes_root(&base);
    let explicit = profile.is_some();
    let name = match profile {
        Some(value) => value.to_string(),
        None if Path::new(&base)
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|p| p == "profiles") =>
        {
            return HermesProfileResolution {
                source_home: base,
                must_exist: true,
                error: None,
            }
        }
        None => read_active_profile(&root).unwrap_or_default(),
    };
    if !explicit && name.is_empty() {
        return HermesProfileResolution {
            source_home: base,
            must_exist: false,
            error: None,
        };
    }
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return invalid_profile("empty profile name");
    }
    if trimmed.eq_ignore_ascii_case("default") {
        return HermesProfileResolution {
            source_home: root,
            must_exist: false,
            error: None,
        };
    }
    let canonical = trimmed.to_ascii_lowercase();
    let valid = canonical.len() <= 64
        && canonical
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && canonical
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if !valid {
        return invalid_profile(format!("invalid profile name {canonical:?}"));
    }
    if matches!(
        canonical.as_str(),
        "hermes" | "test" | "tmp" | "root" | "sudo"
    ) {
        return invalid_profile(format!("reserved profile name {canonical:?}"));
    }
    HermesProfileResolution {
        source_home: Path::new(&root)
            .join("profiles")
            .join(canonical)
            .display()
            .to_string(),
        must_exist: true,
        error: None,
    }
}

fn invalid_profile(message: impl Into<String>) -> HermesProfileResolution {
    HermesProfileResolution {
        error: Some(message.into()),
        ..Default::default()
    }
}

fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

fn platform_default_home() -> String {
    if cfg!(windows) {
        if let Some(local) = std::env::var("LOCALAPPDATA")
            .ok()
            .and_then(|v| nonempty(&v))
        {
            return Path::new(&local).join("hermes").display().to_string();
        }
        if let Some(home) = std::env::var("USERPROFILE").ok().and_then(|v| nonempty(&v)) {
            return Path::new(&home)
                .join("AppData")
                .join("Local")
                .join("hermes")
                .display()
                .to_string();
        }
    }
    std::env::var("HOME")
        .ok()
        .and_then(|v| nonempty(&v))
        .map(|v| Path::new(&v).join(".hermes").display().to_string())
        .unwrap_or_else(|| std::env::temp_dir().join(".hermes").display().to_string())
}

fn absolute_clean(path: &str) -> String {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path.to_string_lossy().into_owned()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
            .to_string_lossy()
            .into_owned()
    }
}

fn hermes_root(home: &str) -> String {
    let path = Path::new(home);
    if path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|p| p == "profiles")
    {
        path.parent()
            .and_then(Path::parent)
            .unwrap_or(path)
            .display()
            .to_string()
    } else {
        home.to_string()
    }
}

fn read_active_profile(root: &str) -> Option<String> {
    let raw = fs::read_to_string(Path::new(root).join("active_profile")).ok()?;
    let value = raw.trim().to_ascii_lowercase();
    (!value.is_empty()).then_some(value)
}

/// Persistent store path used by the daemon's profile-scoped execution plan.
/// Empty output means the inputs cannot safely identify an agent store.
pub fn hermes_memory_store_path(profile: &str, agent_id: &str, source_home: &str) -> String {
    let agent = safe_segment(agent_id);
    if agent.is_empty() {
        return String::new();
    }
    profile_dir(profile)
        .map(|dir| {
            dir.join("hermes-state")
                .join(agent)
                .join(hermes_profile_segment(source_home))
                .display()
                .to_string()
        })
        .unwrap_or_default()
}

pub fn hermes_session_store_path(
    profile: &str,
    agent_id: &str,
    source_home: &str,
    task: &TaskContextForEnv,
) -> String {
    let agent = safe_segment(agent_id);
    let conversation = {
        let issue = safe_segment(&task.issue_id);
        if !issue.is_empty() {
            Some(issue)
        } else {
            let chat = safe_segment(&task.chat_session_id);
            (!chat.is_empty()).then(|| format!("chat_{chat}"))
        }
    };
    match (agent.is_empty(), conversation, profile_dir(profile)) {
        (false, Some(conversation), Some(dir)) => dir
            .join("hermes-sessions")
            .join(agent)
            .join(hermes_profile_segment(source_home))
            .join(conversation)
            .display()
            .to_string(),
        _ => String::new(),
    }
}

fn profile_dir(profile: &str) -> Option<PathBuf> {
    if profile.contains(['/', '\\']) || matches!(profile, "." | "..") {
        return None;
    }
    if let Some(root) = std::env::var_os("CORDY_TASK_CONFIG_ROOT") {
        let root = PathBuf::from(root);
        if !root.is_absolute() {
            return None;
        }
        return Some(if profile.is_empty() {
            root
        } else {
            root.join("profiles").join(profile)
        });
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(if profile.is_empty() {
        home.join(".cordy")
    } else {
        home.join(".cordy").join("profiles").join(profile)
    })
}

fn safe_segment(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() || value == "." || value == ".." || value.contains(['/', '\\']) {
        return String::new();
    }
    value.to_string()
}

fn hermes_profile_segment(source_home: &str) -> String {
    let home = absolute_clean(source_home);
    let native = absolute_clean(&platform_default_home());
    if home == native {
        return "default".to_string();
    }
    let path = Path::new(&home);
    if path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|p| p == "profiles")
    {
        let root = path
            .parent()
            .and_then(Path::parent)
            .map(|root| root.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = path
            .file_name()
            .map(|name| safe_segment(&name.to_string_lossy()))
            .unwrap_or_default();
        if !name.is_empty() {
            return if absolute_clean(&root) == native {
                name
            } else {
                format!("{}_{}", name, sha256_short(&root))
            };
        }
    }
    let digest = sha256_short(&home);
    format!("h_{digest}")
}

fn sha256_short(value: &str) -> String {
    // A stable filesystem-safe digest without adding a new dependency: the
    // daemon already carries sha2 for its other state stores.
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(value.as_bytes());
    hex::encode(&digest[..8])
}

/// Build the Hermes overlay and mount the optional persistent stores.
pub(crate) fn prepare_hermes_home(
    hermes_home: &str,
    source_home: &str,
    source_must_exist: bool,
    skills: &[SkillContextForEnv],
    env: &HashMap<String, String>,
    memory_store: &str,
    session_store: &str,
) -> anyhow::Result<HermesSessions> {
    let shared_home = nonempty(source_home).unwrap_or_else(platform_default_home);
    if source_must_exist {
        let metadata = fs::metadata(&shared_home)
            .with_context(|| format!("hermes profile home {shared_home:?} not found"))?;
        if !metadata.is_dir() {
            bail!("hermes profile home {shared_home:?} is not a directory");
        }
    }
    fs::create_dir_all(hermes_home).context("create Hermes overlay")?;
    restrict_permissions(hermes_home).context("restrict Hermes overlay")?;
    prepare_task_local_state(hermes_home)?;

    let sessions = if !session_store.trim().is_empty() {
        mount_session_db(hermes_home, session_store)?
    } else {
        HermesSessions::default()
    };
    if memory_store.trim().is_empty() {
        detach_memories(hermes_home)?;
    } else {
        mount_memories(hermes_home, memory_store)?;
    }
    mirror_shared_home(&shared_home, hermes_home)?;
    write_derived_config(&shared_home, hermes_home, env)?;
    write_derived_env(&shared_home, hermes_home)?;
    write_bound_skills(hermes_home, skills)?;
    Ok(sessions)
}

fn restrict_permissions(path: &str) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn prepare_task_local_state(home: &str) -> anyhow::Result<()> {
    let marker = Path::new(home).join(TASK_LOCAL_STATE_MARKER);
    match fs::symlink_metadata(&marker) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                bail!("Hermes state marker is not a regular file");
            }
            return Ok(());
        }
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(error.into());
        }
        Err(_) => {}
    }
    for entry in fs::read_dir(home)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_state_entry(&name) {
            remove_path(&entry.path())?;
        }
    }
    atomic_write(&marker, b"task-local Hermes state\n", 0o600)
}

fn is_state_entry(name: &str) -> bool {
    name == SESSION_DB || name.starts_with("state.db-")
}

fn overlay_owned(name: &str) -> bool {
    matches!(
        name,
        "skills"
            | "config.yaml"
            | "memories"
            | "active_profile"
            | "profiles"
            | ".env"
            | TASK_LOCAL_STATE_MARKER
    ) || is_state_entry(name)
}

fn mirror_shared_home(shared: &str, overlay: &str) -> anyhow::Result<()> {
    let mut mirrored = HashSet::new();
    match fs::read_dir(shared) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if overlay_owned(&name) {
                    continue;
                }
                let source = entry.path();
                let target = Path::new(overlay).join(&name);
                link_shared_entry(&source, &target)?;
                mirrored.insert(name);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    for entry in fs::read_dir(overlay)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !overlay_owned(&name) && !mirrored.contains(&name) {
            remove_path(&entry.path())?;
        }
    }
    Ok(())
}

fn link_shared_entry(source: &Path, target: &Path) -> anyhow::Result<()> {
    if let Ok(existing) = fs::symlink_metadata(target) {
        if existing.file_type().is_symlink()
            && fs::read_link(target).ok().as_deref() == Some(source)
        {
            return Ok(());
        }
        remove_path(target)?;
    }
    let source_metadata = match fs::metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(()); // dangling source links are ignored, like Go.
        }
        Err(error) => return Err(error.into()),
    };
    if source_metadata.is_dir() {
        create_dir_link(source, target)?;
    } else {
        create_file_link(source, target)?;
    }
    Ok(())
}

fn create_dir_link(source: &Path, target: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        junction::create(source, target).map_err(anyhow::Error::new)
    }
}

fn create_file_link(source: &Path, target: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        fs::copy(source, target)
            .map(|_| ())
            .map_err(anyhow::Error::new)
    }
}

fn create_session_file_link(source: &Path, target: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(source, target)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        // A copied SQLite file is not a session mount: writes stay in the
        // overlay and disappear with the task.  Match Go's fail-closed
        // behavior when the host cannot create a real symlink.
        std::os::windows::fs::symlink_file(source, target).map_err(anyhow::Error::new)
    }
}

fn write_derived_config(
    shared: &str,
    overlay: &str,
    env: &HashMap<String, String>,
) -> anyhow::Result<()> {
    let source = Path::new(shared).join("config.yaml");
    let target = Path::new(overlay).join("config.yaml");
    let mut doc = match fs::read_to_string(&source) {
        Ok(raw) => match serde_yaml::from_str::<Value>(&raw) {
            Ok(doc) => doc,
            Err(error) => {
                tracing::warn!(error = %error, "execenv: Hermes config parse failed; preserving source config");
                return atomic_write(&target, raw.as_bytes(), 0o600);
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Value::Mapping(Mapping::new()),
        Err(error) => return Err(error.into()),
    };
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();
    for value in existing_external_dirs(&doc) {
        let value = normalize_external_dir(shared, &value, env);
        if !value.is_empty() && seen.insert(value.clone()) {
            dirs.push(value);
        }
    }
    let shared_skills = Path::new(shared).join("skills").display().to_string();
    if seen.insert(shared_skills.clone()) {
        dirs.push(shared_skills);
    }
    set_external_dirs(&mut doc, dirs);
    disable_memory_provider(&mut doc);
    let data = serde_yaml::to_string(&doc)?;
    atomic_write(&target, data.as_bytes(), 0o600)
}

fn existing_external_dirs(doc: &Value) -> Vec<String> {
    let Some(skills) = doc.get("skills") else {
        return Vec::new();
    };
    match skills.get("external_dirs") {
        Some(Value::String(value)) => vec![value.clone()],
        Some(Value::Sequence(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn normalize_external_dir(shared: &str, raw: &str, env: &HashMap<String, String>) -> String {
    let mut value = expand_vars(raw.trim(), env);
    if value.is_empty() {
        return value;
    }
    // Leave unresolved variables for Hermes to expand at runtime. Prefixing
    // such a value with the shared home changes the meaning of the config and
    // diverges from Go's os.Expand-based implementation.
    if value.contains("${") {
        return value;
    }
    if value == "~" || value.starts_with("~/") {
        if let Some(home) = process_user_home() {
            value = format!("{}{}", home.display(), value.trim_start_matches('~'));
        }
    }
    let path = PathBuf::from(&value);
    if path.is_absolute() {
        lexical_clean(path).display().to_string()
    } else {
        lexical_clean(Path::new(shared).join(path))
            .display()
            .to_string()
    }
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

fn lexical_clean(path: PathBuf) -> PathBuf {
    let rooted = path.has_root();
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !cleaned.pop() && !rooted {
                    cleaned.push(component.as_os_str());
                }
            }
            _ => cleaned.push(component.as_os_str()),
        }
    }
    if cleaned.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        cleaned
    }
}

fn expand_vars(value: &str, env: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(start) = value[cursor..].find("${").map(|offset| cursor + offset) {
        out.push_str(&value[cursor..start]);
        let Some(end_offset) = value[start + 2..].find('}') else {
            out.push_str(&value[start..]);
            return out;
        };
        let end = start + 2 + end_offset;
        let key = &value[start + 2..end];
        if let Some(replacement) = env.get(key) {
            out.push_str(replacement);
        } else if let Ok(replacement) = std::env::var(key) {
            out.push_str(&replacement);
        } else {
            out.push_str(&value[start..=end]);
        }
        cursor = end + 1;
    }
    out.push_str(&value[cursor..]);
    out
}

fn set_external_dirs(doc: &mut Value, dirs: Vec<String>) {
    let root = ensure_mapping(doc);
    let skills = root
        .entry(Value::String("skills".into()))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    let skills = ensure_mapping(skills);
    skills.insert(
        Value::String("external_dirs".into()),
        Value::Sequence(dirs.into_iter().map(Value::String).collect()),
    );
}

fn disable_memory_provider(doc: &mut Value) {
    let root = ensure_mapping(doc);
    let memory = root
        .entry(Value::String("memory".into()))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    let memory = ensure_mapping(memory);
    memory.insert(
        Value::String("provider".into()),
        Value::String(String::new()),
    );
}

fn ensure_mapping(value: &mut Value) -> &mut Mapping {
    if !matches!(value, Value::Mapping(_)) {
        *value = Value::Mapping(Mapping::new());
    }
    match value {
        Value::Mapping(mapping) => mapping,
        _ => unreachable!(),
    }
}

fn write_derived_env(shared: &str, overlay: &str) -> anyhow::Result<()> {
    let source = Path::new(shared).join(".env");
    let mut body = match fs::read_to_string(&source) {
        Ok(body) => body,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    body = body
        .lines()
        .filter(|line| dotenv_key(line) != Some("HERMES_HOME"))
        .collect::<Vec<_>>()
        .join("\n");
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(&format!("HERMES_HOME='{}'\n", Path::new(overlay).display()));
    atomic_write(&Path::new(overlay).join(".env"), body.as_bytes(), 0o600)
}

fn dotenv_key(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").unwrap_or(line).trim();
    let (key, _) = line.split_once('=')?;
    let key = key.trim();
    (!key.is_empty()).then_some(key)
}

fn write_bound_skills(overlay: &str, skills: &[SkillContextForEnv]) -> anyhow::Result<()> {
    let skills_dir = Path::new(overlay).join("skills");
    remove_path(&skills_dir)?;
    write_skill_files(&skills_dir.display().to_string(), skills, None)
}

fn mount_memories(overlay: &str, store: &str) -> anyhow::Result<()> {
    let target = Path::new(overlay).join("memories");
    let store = Path::new(store);
    fs::create_dir_all(store)?;
    restrict_permissions(&store.display().to_string())?;
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink()
            && fs::read_link(&target).ok().as_deref() == Some(store)
        {
            touch_store(store);
            return Ok(());
        }
        if metadata.is_dir() && !metadata.file_type().is_symlink() && store_is_empty(store)? {
            copy_tree(&target, store)?;
        }
        remove_path(&target)?;
    }
    create_dir_link(store, &target)?;
    touch_store(store);
    Ok(())
}

fn detach_memories(overlay: &str) -> anyhow::Result<()> {
    let target = Path::new(overlay).join("memories");
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            remove_path(&target)?;
        }
    }
    fs::create_dir_all(target)?;
    Ok(())
}

fn mount_session_db(overlay: &str, store: &str) -> anyhow::Result<HermesSessions> {
    let _mount_guard = SESSION_MOUNT_LOCK
        .lock()
        .map_err(|_| anyhow!("Hermes session mount lock poisoned"))?;
    let store = Path::new(store);
    fs::create_dir_all(store)?;
    restrict_permissions(&store.display().to_string())?;
    let target = Path::new(overlay).join(SESSION_DB);
    let store_db = store.join(SESSION_DB);
    if let Ok(metadata) = fs::symlink_metadata(&target) {
        if metadata.file_type().is_symlink()
            && fs::read_link(&target).ok().as_deref() == Some(store_db.as_path())
        {
            touch_store(store);
            return Ok(HermesSessions {
                mounted: true,
                history_present: has_session_db(store),
            });
        }
    }
    if !has_session_db(store) {
        migrate_session_files(overlay, store)?;
    }
    // Prove the link can be created before deleting a task-local database.
    let staged = Path::new(overlay).join(SESSION_LINK_STAGING);
    remove_path(&staged)?;
    if create_session_file_link(&store_db, &staged).is_err() {
        return Ok(HermesSessions::default());
    }
    remove_session_state_files(Path::new(overlay))?;
    fs::rename(&staged, &target).context("publish Hermes session link")?;
    touch_store(store);
    Ok(HermesSessions {
        mounted: true,
        history_present: has_session_db(store),
    })
}

// Session-store migration and publication are serialized in this process so
// the occupancy check, stale-family cleanup, and directory promotion cannot
// interleave between concurrent task preparations.
static SESSION_MOUNT_LOCK: Mutex<()> = Mutex::new(());

fn touch_store(path: &Path) {
    // Store GC uses directory mtime as the last-activity signal. Updating it
    // is best-effort: active-store reservations still protect a live mount,
    // while a read-only filesystem should not make task preparation fail.
    if let Ok(file) = fs::File::open(path) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
}

fn has_session_db(store: &Path) -> bool {
    fs::metadata(store.join(SESSION_DB))
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

fn migrate_session_files(overlay: &str, store: &Path) -> anyhow::Result<()> {
    let files = fs::read_dir(overlay)?
        .filter_map(Result::ok)
        .filter(|entry| is_state_entry(&entry.file_name().to_string_lossy()))
        .filter(|entry| entry.file_type().map(|f| f.is_file()).unwrap_or(false))
        .collect::<Vec<_>>();
    if files.is_empty() {
        return Ok(());
    }
    let staging = tempfile::tempdir_in(store.parent().unwrap_or(store))?;
    for entry in files {
        fs::copy(entry.path(), staging.path().join(entry.file_name()))?;
    }
    publish_session_staging(staging.path(), store)
}

fn publish_session_staging(staging: &Path, store: &Path) -> anyhow::Result<()> {
    if has_session_db(store) {
        return Ok(());
    }
    remove_session_state_files(store)?;
    if fs::read_dir(store)?.next().is_some() {
        bail!("refusing to replace non-empty Hermes session store");
    }
    fs::remove_dir(store).context("remove empty Hermes session store")?;
    fs::rename(staging, store).context("publish Hermes session staging")
}

fn remove_session_state_files(dir: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if is_state_entry(&entry.file_name().to_string_lossy()) {
            let file_type = entry.file_type()?;
            if !file_type.is_file() && !file_type.is_symlink() {
                bail!(
                    "refusing to remove Hermes session state {}: unexpected entry type",
                    entry.path().display()
                );
            }
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn store_is_empty(dir: &Path) -> anyhow::Result<bool> {
    Ok(fs::read_dir(dir)?.next().is_none())
}

fn copy_tree(source: &Path, target: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let src = entry.path();
        let dst = target.join(entry.file_name());
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_tree(&src, &dst)?;
        } else if ty.is_file() {
            fs::copy(src, dst)?;
        } else {
            bail!("refusing to migrate unsupported Hermes memory entry");
        }
    }
    Ok(())
}

fn remove_path(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
    }
}

fn atomic_write(path: &Path, data: &[u8], mode: u32) -> anyhow::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(mode))?;
    }
    temp.write_all(data)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| anyhow!(error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn profile_resolution_rejects_reserved_and_scopes_named_profiles() {
        let invalid = resolve_hermes_profile("/tmp/hermes", Some("root"));
        assert!(invalid.error.is_some());
        let named = resolve_hermes_profile("/tmp/hermes", Some("Work-1"));
        assert_eq!(named.source_home, "/tmp/hermes/profiles/work-1");
        assert!(named.must_exist);
        let nested = resolve_hermes_profile("/tmp/hermes/profiles/old", Some("Work-2"));
        assert_eq!(nested.source_home, "/tmp/hermes/profiles/work-2");
        assert_eq!(
            hermes_profile_segment("/tmp/custom/profiles/research"),
            format!("research_{}", sha256_short("/tmp/custom"))
        );
    }

    #[test]
    fn overlay_derives_config_env_and_bound_skills_without_mutating_source() {
        let root = tempdir().unwrap();
        let shared = root.path().join("shared");
        let overlay = root.path().join("overlay");
        fs::create_dir_all(shared.join("skills")).unwrap();
        fs::write(shared.join(".env"), "API_KEY=secret\nHERMES_HOME=/wrong\n").unwrap();
        fs::write(
            shared.join("config.yaml"),
            "skills:\n  external_dirs: [\"${SKILLS}\"]\nmemory:\n  provider: hindsight\n",
        )
        .unwrap();
        let skills = vec![SkillContextForEnv {
            name: "review".into(),
            description: "review code".into(),
            content: "# Review\n".into(),
            files: Vec::new(),
        }];
        let mut env = HashMap::new();
        env.insert("SKILLS".into(), "/srv/skills".into());
        prepare_hermes_home(
            &overlay.display().to_string(),
            &shared.display().to_string(),
            true,
            &skills,
            &env,
            "",
            "",
        )
        .unwrap();
        let cfg = fs::read_to_string(overlay.join("config.yaml")).unwrap();
        assert!(cfg.contains("/srv/skills"));
        assert!(cfg.contains("provider:"));
        let dotenv = fs::read_to_string(overlay.join(".env")).unwrap();
        assert!(!dotenv.contains("HERMES_HOME=/wrong"));
        assert!(dotenv.contains("HERMES_HOME='"));
        assert!(overlay.join("skills/review/SKILL.md").exists());
        assert_eq!(
            fs::read_to_string(shared.join(".env")).unwrap(),
            "API_KEY=secret\nHERMES_HOME=/wrong\n"
        );
    }

    #[test]
    fn external_dirs_preserve_unknown_vars_clean_paths_and_ordered_uniqueness() {
        let root = tempdir().unwrap();
        let shared = root.path().join("shared");
        let overlay = root.path().join("overlay");
        fs::create_dir_all(shared.join("skills")).unwrap();
        fs::write(
            shared.join("config.yaml"),
            "skills:\n  external_dirs:\n    - ${CORDY_HERMES_UNKNOWN_EXTERNAL_DIR_9f4d}\n    - ./skills/../skills\n    - ./skills\n    - ${CORDY_HERMES_UNKNOWN_EXTERNAL_DIR_9f4d}\n",
        )
        .unwrap();

        write_derived_config(
            &shared.display().to_string(),
            &overlay.display().to_string(),
            &HashMap::new(),
        )
        .unwrap();
        let doc: Value =
            serde_yaml::from_str(&fs::read_to_string(overlay.join("config.yaml")).unwrap())
                .unwrap();
        let dirs = doc["skills"]["external_dirs"].as_sequence().unwrap();
        assert_eq!(
            dirs[0].as_str(),
            Some("${CORDY_HERMES_UNKNOWN_EXTERNAL_DIR_9f4d}")
        );
        assert_eq!(
            dirs[1].as_str(),
            Some(shared.join("skills").to_str().unwrap())
        );
        assert_eq!(dirs.len(), 2);
    }

    #[test]
    fn unreadable_shared_env_is_not_silently_replaced() {
        let root = tempdir().unwrap();
        let shared = root.path().join("shared");
        let overlay = root.path().join("overlay");
        fs::create_dir_all(shared.join(".env")).unwrap();
        fs::create_dir_all(&overlay).unwrap();

        assert!(write_derived_env(
            &shared.display().to_string(),
            &overlay.display().to_string(),
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_task_state_marker_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = tempdir().unwrap();
        let marker = root.path().join(TASK_LOCAL_STATE_MARKER);
        let target = root.path().join("marker-target");
        fs::write(&target, b"marker").unwrap();
        symlink(&target, &marker).unwrap();

        assert!(prepare_task_local_state(root.path().to_str().unwrap()).is_err());
    }

    #[test]
    fn session_migration_publishes_database_family_before_cleanup() {
        let root = tempdir().unwrap();
        let overlay = root.path().join("overlay");
        let store = root.path().join("store");
        fs::create_dir_all(&overlay).unwrap();
        fs::create_dir_all(&store).unwrap();
        fs::write(overlay.join(SESSION_DB), b"sqlite").unwrap();
        fs::write(overlay.join("state.db-wal"), b"wal").unwrap();

        let result = mount_session_db(overlay.to_str().unwrap(), store.to_str().unwrap()).unwrap();

        assert!(result.mounted);
        assert!(has_session_db(&store));
        assert_eq!(fs::read(store.join("state.db-wal")).unwrap(), b"wal");
        assert!(fs::symlink_metadata(overlay.join(SESSION_DB))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn persistent_memory_and_session_mounts_are_reused() {
        let root = tempdir().unwrap();
        let shared = root.path().join("shared");
        let overlay = root.path().join("overlay");
        let memory = root.path().join("memory");
        let session = root.path().join("session");
        fs::create_dir_all(shared.join("skills")).unwrap();
        fs::create_dir_all(&memory).unwrap();
        fs::create_dir_all(&session).unwrap();
        fs::write(session.join("state.db"), b"sqlite").unwrap();
        prepare_hermes_home(
            &overlay.display().to_string(),
            &shared.display().to_string(),
            true,
            &[SkillContextForEnv::default()],
            &HashMap::new(),
            &memory.display().to_string(),
            &session.display().to_string(),
        )
        .unwrap();
        assert!(fs::symlink_metadata(overlay.join("memories"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(fs::symlink_metadata(overlay.join("state.db"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(has_session_db(&session));
    }
}
