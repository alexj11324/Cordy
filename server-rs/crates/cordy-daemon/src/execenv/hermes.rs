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
use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HermesProfileSelection {
    pub name: String,
    pub found: bool,
    pub inline: bool,
    arg_from: usize,
    arg_len: usize,
}

pub fn parse_hermes_profile_args(args: &[String]) -> HermesProfileSelection {
    let none = HermesProfileSelection::default();
    let mut index = 0;
    while index < args.len() {
        let arg = unquote_arg(&args[index]);
        if arg == "--" {
            break;
        }
        if arg == "--args" && inside_mcp_add(args, index) {
            break;
        }
        if arg == "-p" || arg == "--profile" {
            let Some(value) = args.get(index + 1).map(|value| unquote_arg(value)) else {
                return none;
            };
            if !valid_profile_arg(&value) {
                return none;
            }
            return HermesProfileSelection {
                name: value,
                found: true,
                inline: false,
                arg_from: index,
                arg_len: 2,
            };
        }
        if let Some(value) = arg.strip_prefix("--profile=") {
            let value = unquote_arg(value);
            return HermesProfileSelection {
                name: value,
                found: true,
                inline: true,
                arg_from: index,
                arg_len: 1,
            };
        }
        if hermes_value_flag(&arg) {
            index += 2;
            continue;
        }
        if hermes_optional_value_flag(&arg)
            && args
                .get(index + 1)
                .is_some_and(|value| !unquote_arg(value).starts_with("-"))
        {
            index += 2;
            continue;
        }
        index += 1;
    }
    none
}

pub fn hermes_launch_argv(launch_prefix: &[String], custom_args: &[String]) -> Vec<String> {
    let mut argv = launch_prefix.to_vec();
    argv.push("acp".to_string());
    argv.extend(
        custom_args
            .iter()
            .filter(|arg| unquote_arg(arg) != "acp")
            .cloned(),
    );
    argv
}

pub fn strip_hermes_profile_selectors(
    launch_prefix: &[String],
    custom_args: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut prefix = launch_prefix.to_vec();
    let mut custom = custom_args
        .iter()
        .filter(|arg| unquote_arg(arg) != "acp")
        .cloned()
        .collect::<Vec<_>>();

    loop {
        let argv = hermes_launch_argv(&prefix, &custom);
        let selection = parse_hermes_profile_args(&argv);
        if !selection.found || selection.arg_len == 0 {
            return (prefix, custom);
        }
        for index in (selection.arg_from..selection.arg_from + selection.arg_len).rev() {
            if index < prefix.len() {
                prefix.remove(index);
            } else if index > prefix.len() {
                custom.remove(index - prefix.len() - 1);
            }
        }
    }
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

fn valid_profile_arg(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == 95 || *byte == 45
        })
}

fn hermes_value_flag(value: &str) -> bool {
    matches!(
        value,
        "-z" | "--oneshot"
            | "-m"
            | "--model"
            | "--provider"
            | "-t"
            | "--toolsets"
            | "-r"
            | "--resume"
            | "-s"
            | "--skills"
            | "--usage-file"
    )
}

fn hermes_optional_value_flag(value: &str) -> bool {
    matches!(value, "-c" | "--continue")
}

fn inside_mcp_add(args: &[String], index: usize) -> bool {
    let Some(mcp) = args
        .iter()
        .take(index)
        .position(|arg| unquote_arg(arg) == "mcp")
    else {
        return false;
    };
    args.iter()
        .skip(mcp + 1)
        .take(index.saturating_sub(mcp + 1))
        .any(|arg| unquote_arg(arg) == "add")
}

fn unquote_arg(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        let last = value.len() - 1;
        if (bytes[0] == 39 && bytes[last] == 39) || (bytes[0] == 34 && bytes[last] == 34) {
            return value[1..last].to_string();
        }
    }
    value.to_string()
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
    if marker.exists() {
        if !marker.is_file() {
            bail!("Hermes state marker is not a regular file");
        }
        return Ok(());
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
    if fs::metadata(source).is_err() {
        return Ok(()); // dangling source links are ignored, like Go.
    }
    if fs::metadata(source)?.is_dir() {
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
    let dirs = existing_external_dirs(&doc)
        .into_iter()
        .map(|value| normalize_external_dir(shared, &value, env))
        .filter(|value| !value.is_empty())
        .chain(std::iter::once(
            Path::new(shared).join("skills").display().to_string(),
        ))
        .collect::<Vec<_>>();
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
    if value == "~" || value.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            value = format!("{}{}", home, value.trim_start_matches('~'));
        }
    }
    let path = PathBuf::from(&value);
    if path.is_absolute() {
        path.display().to_string()
    } else {
        Path::new(shared).join(path).display().to_string()
    }
}

fn expand_vars(value: &str, env: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = value[i + 2..].find('}') {
                let end = i + 2 + end;
                let key = &value[i + 2..end];
                if let Some(replacement) = env.get(key) {
                    out.push_str(replacement);
                } else if let Ok(replacement) = std::env::var(key) {
                    out.push_str(&replacement);
                } else {
                    out.push_str(&value[i..=end]);
                }
                i = end + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
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
    let mut body = fs::read_to_string(&source).unwrap_or_default();
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
    if create_file_link(&store_db, &staged).is_err() {
        return Ok(HermesSessions::default());
    }
    remove_state_files(overlay)?;
    fs::rename(&staged, &target).context("publish Hermes session link")?;
    touch_store(store);
    Ok(HermesSessions {
        mounted: true,
        history_present: has_session_db(store),
    })
}

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
    for entry in fs::read_dir(staging.path())? {
        let entry = entry?;
        fs::rename(entry.path(), store.join(entry.file_name()))?;
    }
    Ok(())
}

fn remove_state_files(dir: &str) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if is_state_entry(&entry.file_name().to_string_lossy()) {
            remove_path(&entry.path())?;
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

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn profile_parser_matches_hermes_argv_and_skips_value_flags() {
        let argv = hermes_launch_argv(
            &args(&["--model", "coder"]),
            &args(&["-p", "research", "acp"]),
        );
        let selection = parse_hermes_profile_args(&argv);
        assert_eq!(selection.name, "research");
        assert!(selection.found);
        assert!(!selection.inline);

        let inline = parse_hermes_profile_args(&args(&["-m", "coder", "--profile=research"]));
        assert_eq!(inline.name, "research");
        assert!(inline.inline);
        for quoted in ["--profile='research'", "--profile=\"research\""] {
            let quoted = parse_hermes_profile_args(&args(&[quoted]));
            assert_eq!(quoted.name, "research");
            assert!(quoted.inline);
        }
        assert!(!parse_hermes_profile_args(&args(&["-p", "no:xdist"])).found);
        assert!(
            !parse_hermes_profile_args(&args(&[
                "mcp",
                "add",
                "server",
                "--args",
                "--profile",
                "child",
            ]))
            .found
        );
    }

    #[test]
    fn overlay_strips_all_profile_selectors_from_both_launch_regions() {
        let (prefix, custom) = strip_hermes_profile_selectors(
            &args(&["--model", "coder"]),
            &args(&["-p", "research", "--profile=other", "flag", "acp"]),
        );
        assert_eq!(prefix, args(&["--model", "coder"]));
        assert_eq!(custom, args(&["flag"]));
    }

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
