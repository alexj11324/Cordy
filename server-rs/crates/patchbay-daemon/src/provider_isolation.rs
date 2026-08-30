//! OS-enforced provider process isolation.
//!
//! Provider CLIs are untrusted task processes: they can spawn shells and read
//! arbitrary paths unless the first process is sandboxed. The wrapper built
//! here is inherited by every descendant. Unsupported hosts fail closed.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use patchbay_agent::RuntimeCommand;

pub(crate) struct ProviderIsolation<'a> {
    pub provider: &'a str,
    pub task_root: &'a str,
    pub work_dir: &'a str,
    pub temp_dir: &'a str,
    pub provider_source_home: &'a str,
}

pub(crate) fn failure_reason(error: &anyhow::Error) -> &'static str {
    let message = format!("{error:#}").to_ascii_lowercase();
    if message.contains("bubblewrap")
        || message.contains("sandbox-exec")
        || message.contains("unsupported on this host")
    {
        "sandbox_unavailable"
    } else if message.contains("too large") || message.contains("scan overflow") {
        "credential_scan_limit"
    } else if message.contains("resolve")
        || message.contains("inspect")
        || message.contains("metadata")
    {
        "isolation_filesystem_error"
    } else if message.contains("credential")
        || message.contains("provider profile")
        || message.contains("managed runtime")
    {
        "credential_boundary_rejected"
    } else if message.contains("hermes")
        || message.contains("runtime closure")
        || message.contains("interpreter")
    {
        "runtime_closure_rejected"
    } else {
        "isolation_policy_rejected"
    }
}

const MAX_VISIBLE_FILE_SCAN: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum SensitivePathKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SensitivePath {
    path: PathBuf,
    kind: SensitivePathKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialScanScope {
    Task,
    ProviderRuntime,
    HermesRuntime,
}

pub(crate) fn isolate_provider_command(
    command: &mut RuntimeCommand,
    isolation: ProviderIsolation<'_>,
) -> anyhow::Result<()> {
    let executable = canonical_existing(&command.path, "provider executable")?;
    let task_root = canonical_existing(isolation.task_root, "task root")?;
    let work_dir = canonical_existing(isolation.work_dir, "task work directory")?;
    let temp_dir = canonical_existing(isolation.temp_dir, "task temp directory")?;
    let mut sensitive_paths =
        host_sensitive_paths(isolation.provider, isolation.provider_source_home)?;
    let host_credential_files = protected_file_identities(&sensitive_paths)?;
    reject_sensitive_path_overlap(&sensitive_paths, &[&task_root, &work_dir, &temp_dir])?;
    sensitive_paths.extend(sensitive_task_files(
        &[&task_root, &work_dir, &temp_dir],
        &host_credential_files,
    )?);
    sensitive_paths.sort();
    sensitive_paths.dedup();

    #[cfg(target_os = "macos")]
    return isolate_macos(
        command,
        &executable,
        [&task_root, &work_dir, &temp_dir],
        isolation.provider,
        isolation.provider_source_home,
        &sensitive_paths,
    );

    #[cfg(target_os = "linux")]
    return isolate_linux(
        command,
        &executable,
        [&task_root, &work_dir, &temp_dir],
        isolation.provider,
        isolation.provider_source_home,
        &sensitive_paths,
    );

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (command, executable, task_root, work_dir, temp_dir);
        anyhow::bail!("provider task filesystem isolation is unsupported on this host")
    }
}

fn sensitive_task_files(
    visible_roots: &[&Path],
    protected_files: &BTreeSet<FileIdentity>,
) -> anyhow::Result<Vec<SensitivePath>> {
    scan_sensitive_files(visible_roots, CredentialScanScope::Task, protected_files)
}

fn sensitive_provider_runtime_files(
    root: &Path,
    protected_files: &BTreeSet<FileIdentity>,
) -> anyhow::Result<Vec<SensitivePath>> {
    let scope = if root.file_name().is_some_and(|name| name == "hermes-agent")
        && root.join("venv").is_dir()
        && root.join("hermes").is_file()
    {
        CredentialScanScope::HermesRuntime
    } else {
        CredentialScanScope::ProviderRuntime
    };
    scan_sensitive_files(&[root], scope, protected_files)
}

fn scan_sensitive_files(
    visible_roots: &[&Path],
    scope: CredentialScanScope,
    protected_files: &BTreeSet<FileIdentity>,
) -> anyhow::Result<Vec<SensitivePath>> {
    let mut pending = visible_roots
        .iter()
        .map(|root| root.to_path_buf())
        .collect::<Vec<_>>();
    pending.sort();
    pending.dedup();
    let mut sensitive_paths = BTreeSet::new();
    let mut scanned_directories = BTreeSet::new();
    let mut visited = 0_usize;
    while let Some(directory) = pending.pop() {
        if !scanned_directories.insert(directory.clone()) {
            continue;
        }
        let entries = fs::read_dir(&directory).with_context(|| {
            format!(
                "inspect task directory {} for credentials",
                directory.display()
            )
        })?;
        for entry in entries {
            let entry = entry.with_context(|| {
                format!("inspect task directory entry under {}", directory.display())
            })?;
            visited = visited
                .checked_add(1)
                .ok_or_else(|| anyhow!("task credential scan overflow"))?;
            anyhow::ensure!(
                visited <= MAX_VISIBLE_FILE_SCAN,
                "task directory is too large to prove credential isolation"
            );
            let path = entry.path();
            let file_type = entry
                .file_type()
                .with_context(|| format!("inspect task path {}", path.display()))?;
            let metadata =
                if file_type.is_file() {
                    Some(entry.metadata().with_context(|| {
                        format!("inspect task file metadata {}", path.display())
                    })?)
                } else {
                    None
                };
            if metadata
                .as_ref()
                .and_then(file_identity)
                .is_some_and(|identity| protected_files.contains(&identity))
            {
                anyhow::bail!("provider task path aliases a host credential");
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let normalized = name.to_ascii_lowercase();
            let hidden_runtime_entry = match scope {
                CredentialScanScope::Task => false,
                CredentialScanScope::ProviderRuntime => normalized == ".git",
                CredentialScanScope::HermesRuntime => {
                    matches!(normalized.as_str(), ".git" | "node_modules")
                }
            };
            if hidden_runtime_entry || is_sensitive_credential_name(&name) {
                anyhow::ensure!(
                    !file_type.is_symlink(),
                    "provider task cannot safely mask credential symlink {}",
                    path.display()
                );
                if let Some(metadata) = &metadata {
                    anyhow::ensure!(
                        hard_link_count(metadata) == 1,
                        "provider task cannot safely mask hard-linked credential {}",
                        path.display()
                    );
                }
                add_sensitive_path(
                    &mut sensitive_paths,
                    path,
                    if file_type.is_dir() {
                        SensitivePathKind::Directory
                    } else {
                        SensitivePathKind::File
                    },
                );
                continue;
            }
            if file_type.is_dir() {
                pending.push(path);
            }
        }
    }
    Ok(sensitive_paths.into_iter().collect())
}

fn is_sensitive_env_name(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    if normalized == ".env" || normalized == ".envrc" {
        return true;
    }
    let Some(suffix) = normalized.strip_prefix(".env.") else {
        return false;
    };
    !matches!(
        suffix,
        "example" | "sample" | "template" | "dist" | "defaults"
    )
}

fn is_sensitive_credential_name(name: &str) -> bool {
    if is_sensitive_env_name(name) {
        return true;
    }
    matches!(
        name.to_ascii_lowercase().as_str(),
        ".npmrc" | ".pypirc" | ".netrc" | "auth.json" | ".credentials.json" | "credentials.json"
    )
}

#[cfg(unix)]
fn hard_link_count(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    metadata.nlink()
}

#[cfg(not(unix))]
fn hard_link_count(_metadata: &fs::Metadata) -> u64 {
    1
}

#[cfg(unix)]
fn file_identity(metadata: &fs::Metadata) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt;

    Some(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn file_identity(_metadata: &fs::Metadata) -> Option<FileIdentity> {
    None
}

fn protected_file_identities(
    sensitive_paths: &[SensitivePath],
) -> anyhow::Result<BTreeSet<FileIdentity>> {
    let mut protected = BTreeSet::new();
    let mut pending = Vec::new();
    for sensitive in sensitive_paths {
        let metadata = match fs::metadata(&sensitive.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).context("inspect host credential metadata");
            }
        };
        if metadata.is_file() {
            protect_file_identity(&mut protected, &metadata)?;
        } else if metadata.is_dir() && sensitive.kind == SensitivePathKind::Directory {
            pending.push(sensitive.path.clone());
        }
    }

    let mut scanned_directories = BTreeSet::new();
    let mut visited = 0_usize;
    while let Some(directory) = pending.pop() {
        let canonical = fs::canonicalize(&directory).context("resolve host credential scope")?;
        if !scanned_directories.insert(canonical.clone()) {
            continue;
        }
        for entry in fs::read_dir(&canonical).context("inspect host credential scope")? {
            let entry = entry.context("inspect host credential entry")?;
            visited = visited
                .checked_add(1)
                .ok_or_else(|| anyhow!("host credential scan overflow"))?;
            anyhow::ensure!(
                visited <= MAX_VISIBLE_FILE_SCAN,
                "host credential scope is too large to prove isolation"
            );
            let path = entry.path();
            let metadata = fs::metadata(&path).context("inspect host credential entry metadata")?;
            if metadata.is_file() {
                protect_file_identity(&mut protected, &metadata)?;
            } else if metadata.is_dir() {
                if is_hermes_runtime_directory(&path) {
                    for sensitive in sensitive_provider_runtime_files(&path, &BTreeSet::new())? {
                        if sensitive.kind != SensitivePathKind::File {
                            continue;
                        }
                        match fs::metadata(&sensitive.path) {
                            Ok(metadata) if metadata.is_file() => {
                                protect_file_identity(&mut protected, &metadata)?;
                            }
                            Ok(_) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => {
                                return Err(error)
                                    .context("inspect host runtime credential metadata");
                            }
                        }
                    }
                } else if !is_pruned_host_directory(&path) {
                    pending.push(path);
                }
            }
        }
    }
    Ok(protected)
}

fn protect_file_identity(
    protected: &mut BTreeSet<FileIdentity>,
    metadata: &fs::Metadata,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        hard_link_count(metadata) == 1,
        "host credential has multiple hard links"
    );
    if let Some(identity) = file_identity(metadata) {
        protected.insert(identity);
    }
    Ok(())
}

fn is_hermes_runtime_directory(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "hermes-agent")
        && path.join("venv").is_dir()
        && path.join("hermes").is_file()
}

fn is_pruned_host_directory(path: &Path) -> bool {
    path.file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .as_deref()
        .is_some_and(|name| matches!(name, ".git" | "node_modules"))
}

fn host_sensitive_paths(
    provider: &str,
    provider_source_home: &str,
) -> anyhow::Result<Vec<SensitivePath>> {
    let mut paths = BTreeSet::new();
    let shared_codex_home = absolute_path(
        crate::execenv::codex_home::resolve_shared_codex_home(),
        "shared Codex home",
    )?;
    add_sensitive_file(&mut paths, shared_codex_home.join("auth.json"));

    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        let home = absolute_path(PathBuf::from(home), "daemon HOME")?;
        add_sensitive_file(&mut paths, home.join(".claude").join(".credentials.json"));
        add_sensitive_file(&mut paths, home.join(".claude.json"));
        if provider == "hermes" {
            add_hermes_sensitive_paths(&mut paths, &home.join(".hermes"));
        } else {
            add_sensitive_directory(&mut paths, home.join(".hermes"));
        }
    }
    if let Some(claude_home) =
        std::env::var_os("CLAUDE_CONFIG_DIR").filter(|value| !value.is_empty())
    {
        let claude_home = absolute_path(PathBuf::from(claude_home), "Claude config home")?;
        add_sensitive_file(&mut paths, claude_home.join(".credentials.json"));
    }
    if let Some(hermes_home) = std::env::var_os("HERMES_HOME").filter(|value| !value.is_empty()) {
        let hermes_home = absolute_path(PathBuf::from(hermes_home), "Hermes source home")?;
        if provider == "hermes" {
            add_hermes_sensitive_paths(&mut paths, &hermes_home);
        } else {
            add_sensitive_directory(&mut paths, hermes_home);
        }
    }
    if !provider_source_home.trim().is_empty() {
        let provider_source_home = absolute_path(provider_source_home, "provider source home")?;
        if provider == "hermes" {
            add_hermes_sensitive_paths(&mut paths, &provider_source_home);
        } else {
            add_sensitive_directory(&mut paths, provider_source_home);
        }
    }
    if let Ok(entries) = std::env::current_dir().and_then(fs::read_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if is_sensitive_credential_name(&name.to_string_lossy()) {
                add_sensitive_file(&mut paths, entry.path());
            }
        }
    }
    Ok(paths.into_iter().collect())
}

fn add_hermes_sensitive_paths(paths: &mut BTreeSet<SensitivePath>, source_home: &Path) {
    let root = hermes_root_for_source(source_home);
    if source_home != root.as_path() {
        add_sensitive_directory(paths, source_home.to_path_buf());
    }
    for name in [
        ".env",
        "auth.json",
        "auth.lock",
        "config.yaml",
        "config.yaml.bak",
        "profile.yaml",
        "settings.json",
        "projects.db",
        "gateway_state.json",
        "channel_directory.json",
    ] {
        add_sensitive_file(paths, root.join(name));
    }
    add_sensitive_file(paths, root.join("shared").join("nous_auth.json"));
    for name in [
        "profiles",
        "sessions",
        "memories",
        "logs",
        ".curator_backups",
    ] {
        add_sensitive_directory(paths, root.join(name));
    }
}

fn sensitive_paths_for_runtime_roots(
    existing: &[SensitivePath],
    runtime_roots: &[PathBuf],
) -> anyhow::Result<Vec<SensitivePath>> {
    let runtime_root_refs = runtime_roots
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    reject_sensitive_path_overlap(existing, &runtime_root_refs)?;
    let protected_files = protected_file_identities(existing)?;
    let mut sensitive = existing.iter().cloned().collect::<BTreeSet<_>>();
    for root in runtime_roots {
        anyhow::ensure!(
            root.is_dir(),
            "provider runtime root {} is not a directory",
            root.display()
        );
        sensitive.extend(sensitive_provider_runtime_files(root, &protected_files)?);
    }
    Ok(sensitive.into_iter().collect())
}

fn hermes_root_for_source(source_home: &Path) -> PathBuf {
    let profiles = source_home
        .parent()
        .filter(|parent| parent.file_name().is_some_and(|name| name == "profiles"));
    profiles
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| source_home.to_path_buf())
}

fn absolute_path(path: impl AsRef<Path>, label: &str) -> anyhow::Result<PathBuf> {
    let path = path.as_ref();
    anyhow::ensure!(!path.as_os_str().is_empty(), "{label} is missing");
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .with_context(|| format!("resolve {label}"))?
            .join(path))
    }
}

fn add_sensitive_file(paths: &mut BTreeSet<SensitivePath>, path: PathBuf) {
    add_sensitive_path(paths, path, SensitivePathKind::File);
}

fn add_sensitive_directory(paths: &mut BTreeSet<SensitivePath>, path: PathBuf) {
    add_sensitive_path(paths, path, SensitivePathKind::Directory);
}

fn add_sensitive_path(paths: &mut BTreeSet<SensitivePath>, path: PathBuf, kind: SensitivePathKind) {
    let mut candidates = BTreeSet::from([path.clone()]);
    if let Ok(canonical) = fs::canonicalize(path) {
        candidates.insert(canonical);
    }
    #[cfg(target_os = "macos")]
    {
        for candidate in candidates.clone() {
            candidates.extend(macos_path_aliases(&candidate));
        }
    }
    for path in candidates {
        paths.insert(SensitivePath {
            path,
            kind: kind.clone(),
        });
    }
}

fn reject_sensitive_path_overlap(
    sensitive_paths: &[SensitivePath],
    visible_roots: &[&Path],
) -> anyhow::Result<()> {
    for sensitive in sensitive_paths {
        anyhow::ensure!(
            sensitive.path != Path::new("/"),
            "provider credential path cannot be the filesystem root"
        );
        for visible in visible_roots {
            anyhow::ensure!(
                !visible.starts_with(&sensitive.path) && !sensitive.path.starts_with(visible),
                "provider task path {} overlaps host credential path {}",
                visible.display(),
                sensitive.path.display()
            );
        }
    }
    Ok(())
}

fn canonical_existing(path: &str, label: &str) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(!path.trim().is_empty(), "{label} is missing");
    std::fs::canonicalize(path).with_context(|| format!("resolve {label} {path:?}"))
}

fn executable_root(executable: &Path) -> anyhow::Result<PathBuf> {
    let root = executable.parent().map(Path::to_path_buf).ok_or_else(|| {
        anyhow!(
            "provider executable has no parent: {}",
            executable.display()
        )
    })?;
    anyhow::ensure!(
        root != Path::new("/"),
        "provider executable at filesystem root cannot be isolated"
    );
    Ok(root)
}

#[cfg(target_os = "macos")]
fn isolate_macos(
    command: &mut RuntimeCommand,
    executable: &Path,
    visible_roots: [&Path; 3],
    provider: &str,
    provider_source_home: &str,
    sensitive_paths: &[SensitivePath],
) -> anyhow::Result<()> {
    let [task_root, work_dir, temp_dir] = visible_roots;
    const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
    anyhow::ensure!(
        Path::new(SANDBOX_EXEC).is_file(),
        "sandbox-exec is unavailable"
    );
    let provider_root = executable_root(executable)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    let provider_install = provider_install_root(executable, &provider_root, home.as_deref())?;
    reject_provider_install_source_overlap(&provider_install, provider_source_home)?;
    let mut provider_readable_roots = BTreeSet::from([provider_install]);
    provider_readable_roots.extend(hermes_runtime_roots(
        provider,
        executable,
        home.as_deref(),
        provider_source_home,
    )?);
    if let Some(home) = &home {
        provider_readable_roots.extend(managed_runtime_roots(home)?);
    }
    provider_readable_roots.retain(|path| path.is_dir());
    let provider_readable_roots = provider_readable_roots.into_iter().collect::<Vec<_>>();
    let effective_sensitive =
        sensitive_paths_for_runtime_roots(sensitive_paths, &provider_readable_roots)?;
    let mut readable = vec![
        PathBuf::from("/System"),
        PathBuf::from("/bin"),
        PathBuf::from("/sbin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/usr/lib"),
        PathBuf::from("/usr/libexec"),
        PathBuf::from("/usr/share"),
        PathBuf::from("/Library/Apple"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/opt/homebrew/lib"),
        PathBuf::from("/opt/homebrew/share"),
        PathBuf::from("/opt/homebrew/opt"),
        PathBuf::from("/opt/homebrew/Cellar"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/local/lib"),
        PathBuf::from("/usr/local/share"),
        PathBuf::from("/usr/local/opt"),
        PathBuf::from("/usr/local/Cellar"),
        task_root.to_path_buf(),
        work_dir.to_path_buf(),
        temp_dir.to_path_buf(),
    ];
    readable.extend(provider_readable_roots);
    let mut readable_aliases = BTreeSet::new();
    for path in readable {
        readable_aliases.extend(macos_path_aliases(&path));
    }
    let readable = readable_aliases
        .into_iter()
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    let mut profile = String::from(
        "(version 1)\n\
         (deny default)\n\
         (import \"dyld-support.sb\")\n\
         (allow process-exec)\n\
         (allow process-fork)\n\
         (allow network*)\n\
         (allow mach-lookup\n\
             (global-name \"com.apple.SystemConfiguration.configd\")\n\
             (global-name \"com.apple.cfprefsd.agent\")\n\
             (global-name \"com.apple.networkd\")\n\
             (global-name \"com.apple.system.logger\"))\n\
         (allow file-read*\n\
             (literal \"/dev/null\")\n\
             (literal \"/dev/random\")\n\
             (literal \"/dev/urandom\")\n\
             (subpath \"/dev/fd\"))\n\
         (allow file-read-metadata file-test-existence\n\
             (literal \"/\")\n\
             (literal \"/etc\")\n\
             (literal \"/tmp\")\n\
             (literal \"/var\"))\n\
         (allow file-write-data (subpath \"/dev/fd\"))\n\
         (allow file-write* (literal \"/dev/null\"))\n",
    );
    for path in &readable {
        profile.push_str(&format!(
            "(allow file-read-metadata file-test-existence (path-ancestors \"{}\"))\n",
            sandbox_quote(path.as_path())
        ));
        profile.push_str(&format!(
            "(allow file-read* (subpath \"{}\"))\n",
            sandbox_quote(path.as_path())
        ));
        profile.push_str(&format!(
            "(allow file-map-executable (subpath \"{}\"))\n",
            sandbox_quote(path.as_path())
        ));
    }
    for path in [task_root, work_dir, temp_dir] {
        profile.push_str(&format!(
            "(allow file-write* (subpath \"{}\"))\n",
            sandbox_quote(path)
        ));
    }
    append_macos_sensitive_rules(&mut profile, &effective_sensitive);
    let mut prefix = vec!["-p".to_string(), profile, "--".to_string()];
    prefix.push(executable.to_string_lossy().into_owned());
    prefix.append(&mut command.prefix);
    command.path = SANDBOX_EXEC.to_string();
    command.prefix = prefix;
    Ok(())
}

#[cfg(target_os = "macos")]
fn sandbox_quote(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

#[cfg(target_os = "macos")]
fn macos_path_aliases(path: &Path) -> BTreeSet<PathBuf> {
    let mut aliases = BTreeSet::from([path.to_path_buf()]);
    for (public, private) in [
        (Path::new("/etc"), Path::new("/private/etc")),
        (Path::new("/tmp"), Path::new("/private/tmp")),
        (Path::new("/var"), Path::new("/private/var")),
    ] {
        if let Ok(relative) = path.strip_prefix(public) {
            aliases.insert(private.join(relative));
        }
        if let Ok(relative) = path.strip_prefix(private) {
            aliases.insert(public.join(relative));
        }
    }
    aliases
}

#[cfg(target_os = "macos")]
fn append_macos_sensitive_rules(profile: &mut String, sensitive_paths: &[SensitivePath]) {
    for sensitive in sensitive_paths {
        let filter = match sensitive.kind {
            SensitivePathKind::File => "literal",
            SensitivePathKind::Directory => "subpath",
        };
        profile.push_str(&format!(
            "(deny file-read* ({filter} \"{}\"))\n\
             (deny file-write* ({filter} \"{}\"))\n\
             (deny file-map-executable ({filter} \"{}\"))\n",
            sandbox_quote(&sensitive.path),
            sandbox_quote(&sensitive.path),
            sandbox_quote(&sensitive.path)
        ));
    }
}

#[cfg(target_os = "linux")]
fn isolate_linux(
    command: &mut RuntimeCommand,
    executable: &Path,
    visible_roots: [&Path; 3],
    provider: &str,
    provider_source_home: &str,
    sensitive_paths: &[SensitivePath],
) -> anyhow::Result<()> {
    let [task_root, work_dir, temp_dir] = visible_roots;
    let bwrap = ["/usr/bin/bwrap", "/usr/local/bin/bwrap"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .ok_or_else(|| anyhow!("bubblewrap is required for provider task isolation"))?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| anyhow!("daemon HOME is unavailable for provider isolation"))?;
    let provider_root = executable_root(executable)?;
    let readable_roots = linux_readable_roots(
        executable,
        &provider_root,
        &home,
        provider,
        provider_source_home,
    )?;
    let effective_sensitive =
        sensitive_paths_for_runtime_roots(sensitive_paths, &readable_roots.provider)?;
    let mut prefix = vec![
        "--die-with-parent".to_string(),
        "--new-session".to_string(),
        "--unshare-pid".to_string(),
        "--unshare-ipc".to_string(),
        "--unshare-uts".to_string(),
        "--unshare-cgroup".to_string(),
    ];

    let mut mount_targets = readable_roots.all.clone();
    mount_targets.extend([
        task_root.to_path_buf(),
        work_dir.to_path_buf(),
        temp_dir.to_path_buf(),
    ]);
    mount_targets.extend(
        effective_sensitive
            .iter()
            .map(|sensitive| sensitive.path.clone()),
    );
    mount_targets.extend([
        home.clone(),
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
        PathBuf::from("/dev"),
        PathBuf::from("/proc"),
    ]);
    add_mount_target_parents(&mut prefix, &mount_targets)?;

    for path in &readable_roots.all {
        prefix.push("--ro-bind".to_string());
        prefix.push(path.to_string_lossy().into_owned());
        prefix.push(path.to_string_lossy().into_owned());
    }
    prefix.extend([
        "--tmpfs".to_string(),
        "/tmp".to_string(),
        "--tmpfs".to_string(),
        "/var/tmp".to_string(),
        "--dev".to_string(),
        "/dev".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
    ]);
    for path in [task_root, work_dir, temp_dir] {
        prefix.push("--bind".to_string());
        prefix.push(path.to_string_lossy().into_owned());
        prefix.push(path.to_string_lossy().into_owned());
    }
    let mounted_roots = readable_roots
        .all
        .iter()
        .map(PathBuf::as_path)
        .chain([task_root, work_dir, temp_dir])
        .collect::<Vec<_>>();
    append_linux_sensitive_mounts(&mut prefix, &effective_sensitive, &mounted_roots);
    prefix.extend([
        "--chdir".to_string(),
        work_dir.to_string_lossy().into_owned(),
        "--".to_string(),
        executable.to_string_lossy().into_owned(),
    ]);
    prefix.append(&mut command.prefix);
    command.path = bwrap.to_string();
    command.prefix = prefix;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn task_env_classifier_rejects_runtime_secrets_but_keeps_templates() {
        for name in [".env", ".envrc", ".env.local", ".env.production"] {
            assert!(is_sensitive_env_name(name), "{name}");
        }
        for name in [".ENV", ".ENVRC", ".ENV.LOCAL"] {
            assert!(is_sensitive_env_name(name), "{name}");
        }
        for name in [
            ".npmrc",
            ".pypirc",
            ".netrc",
            "auth.json",
            "credentials.json",
        ] {
            assert!(is_sensitive_credential_name(name), "{name}");
        }
        for name in [
            ".env.example",
            ".env.sample",
            ".env.template",
            ".env.dist",
            ".env.defaults",
            "env.local",
        ] {
            assert!(!is_sensitive_env_name(name), "{name}");
        }
    }

    #[test]
    fn public_isolation_failure_reason_never_contains_host_paths() {
        let error =
            anyhow!("provider task path /Users/owner/.codex/auth.json aliases a host credential");

        let reason = failure_reason(&error);

        assert_eq!(reason, "credential_boundary_rejected");
        assert!(!reason.contains("/Users/owner"));
        assert!(!reason.contains("auth.json"));
        assert_eq!(
            failure_reason(&anyhow!(
                "inspect host credential metadata: permission denied"
            )),
            "isolation_filesystem_error"
        );
    }

    #[test]
    fn task_visible_credential_files_are_selected_for_masking() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("app")).unwrap();
        let credential = root.path().join("app").join(".env.local");
        fs::write(&credential, "TOKEN=secret\n").unwrap();
        let expected = BTreeSet::from([
            SensitivePath {
                path: credential.clone(),
                kind: SensitivePathKind::File,
            },
            SensitivePath {
                path: fs::canonicalize(&credential).unwrap(),
                kind: SensitivePathKind::File,
            },
        ]);
        assert_eq!(
            sensitive_task_files(&[root.path()], &BTreeSet::new())
                .unwrap()
                .into_iter()
                .collect::<BTreeSet<_>>(),
            expected
        );
    }

    #[test]
    fn credential_roots_cannot_overlap_task_visible_roots() {
        let sensitive = vec![SensitivePath {
            path: PathBuf::from("/host/provider/auth.json"),
            kind: SensitivePathKind::File,
        }];
        assert!(reject_sensitive_path_overlap(&sensitive, &[Path::new("/host/provider")]).is_err());
        assert!(reject_sensitive_path_overlap(&sensitive, &[Path::new("/task")]).is_ok());
    }

    #[test]
    fn sensitive_scan_covers_every_visible_root_and_build_output() {
        let task = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let task_secret = task.path().join("secrets").join(".env.local");
        let build_secret = work.path().join("build").join(".env.production");
        let temp_secret = temp.path().join(".env");
        for path in [&task_secret, &build_secret, &temp_secret] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "TOKEN=secret\n").unwrap();
        }
        let selected =
            sensitive_task_files(&[task.path(), work.path(), temp.path()], &BTreeSet::new())
                .unwrap();
        for path in [task_secret, build_secret, temp_secret] {
            assert!(selected.iter().any(|sensitive| sensitive.path == path));
        }
    }

    #[cfg(unix)]
    #[test]
    fn hard_linked_task_credential_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        let credential = root.path().join(".env");
        fs::write(&credential, "TOKEN=secret\n").unwrap();
        fs::hard_link(&credential, root.path().join("alias")).unwrap();

        let error = sensitive_task_files(&[root.path()], &BTreeSet::new()).unwrap_err();
        assert!(error.to_string().contains("hard-linked credential"));
    }

    #[cfg(unix)]
    #[test]
    fn innocuous_hard_link_to_host_credential_fails_closed() {
        let host = tempfile::tempdir().unwrap();
        let task = tempfile::tempdir().unwrap();
        let credential = host.path().join("auth.json");
        fs::write(&credential, "long-lived-secret\n").unwrap();
        let sensitive = vec![SensitivePath {
            path: credential.clone(),
            kind: SensitivePathKind::File,
        }];
        let protected = protected_file_identities(&sensitive).unwrap();
        fs::hard_link(&credential, task.path().join("README.cache")).unwrap();

        let error = sensitive_task_files(&[task.path()], &protected).unwrap_err();

        assert!(error.to_string().contains("aliases a host credential"));
    }

    #[cfg(unix)]
    #[test]
    fn innocuous_hard_link_to_file_in_host_profile_fails_closed() {
        let host = tempfile::tempdir().unwrap();
        let task = tempfile::tempdir().unwrap();
        let credential = host.path().join("profiles").join("account.db");
        fs::create_dir_all(credential.parent().unwrap()).unwrap();
        fs::write(&credential, "private-provider-state\n").unwrap();
        let protected = protected_file_identities(&[SensitivePath {
            path: host.path().to_path_buf(),
            kind: SensitivePathKind::Directory,
        }])
        .unwrap();
        fs::hard_link(&credential, task.path().join("cache.db")).unwrap();

        let error = sensitive_task_files(&[task.path()], &protected).unwrap_err();

        assert!(error.to_string().contains("aliases a host credential"));
    }

    #[cfg(unix)]
    #[test]
    fn existing_host_credential_hard_link_fails_before_any_root_is_exposed() {
        let host = tempfile::tempdir().unwrap();
        let readable = tempfile::tempdir().unwrap();
        let credential = host.path().join("auth.json");
        fs::write(&credential, "long-lived-secret\n").unwrap();
        fs::hard_link(&credential, readable.path().join("cache.dat")).unwrap();

        let error = protected_file_identities(&[SensitivePath {
            path: credential,
            kind: SensitivePathKind::File,
        }])
        .unwrap_err();

        assert!(error.to_string().contains("multiple hard links"));
    }

    #[cfg(unix)]
    #[test]
    fn hermes_runtime_env_inode_is_protected_inside_sensitive_profile() {
        let host = tempfile::tempdir().unwrap();
        let task = tempfile::tempdir().unwrap();
        let runtime = host.path().join("hermes-agent");
        let credential = runtime.join(".env");
        fs::create_dir_all(runtime.join("venv")).unwrap();
        fs::write(runtime.join("hermes"), "").unwrap();
        fs::write(&credential, "HERMES_TOKEN=long-lived-secret\n").unwrap();
        let protected = protected_file_identities(&[SensitivePath {
            path: host.path().to_path_buf(),
            kind: SensitivePathKind::Directory,
        }])
        .unwrap();
        fs::hard_link(&credential, task.path().join("README.cache")).unwrap();

        let error = sensitive_task_files(&[task.path()], &protected).unwrap_err();

        assert!(error.to_string().contains("aliases a host credential"));
    }

    #[cfg(unix)]
    #[test]
    fn sensitive_scan_records_lexical_and_canonical_aliases() {
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join("runtime");
        let alias = root.path().join("runtime-alias");
        fs::create_dir_all(&runtime).unwrap();
        fs::write(runtime.join(".env"), "TOKEN=secret\n").unwrap();
        std::os::unix::fs::symlink(&runtime, &alias).unwrap();

        let sensitive = sensitive_provider_runtime_files(&alias, &BTreeSet::new()).unwrap();
        for path in [
            alias.join(".env"),
            fs::canonicalize(runtime.join(".env")).unwrap(),
        ] {
            assert!(sensitive.iter().any(|item| item.path == path));
        }
    }

    #[test]
    fn readable_runtime_scan_discovers_nested_provider_credentials() {
        let root = tempfile::tempdir().unwrap();
        let credential = root.path().join("lib").join("provider").join(".npmrc");
        fs::create_dir_all(credential.parent().unwrap()).unwrap();
        fs::write(&credential, "//registry.example/:_authToken=secret\n").unwrap();

        let selected =
            sensitive_paths_for_runtime_roots(&[], &[root.path().to_path_buf()]).unwrap();

        assert!(selected.iter().any(|sensitive| {
            sensitive.path == credential && sensitive.kind == SensitivePathKind::File
        }));
    }

    #[cfg(unix)]
    #[test]
    fn hidden_runtime_symlinks_fail_closed() {
        for hidden in [".git", "node_modules"] {
            let root = tempfile::tempdir().unwrap();
            let runtime = root.path().join("hermes-agent");
            let external = root.path().join("external");
            fs::create_dir_all(runtime.join("venv")).unwrap();
            fs::create_dir_all(&external).unwrap();
            fs::write(runtime.join("hermes"), "").unwrap();
            std::os::unix::fs::symlink(&external, runtime.join(hidden)).unwrap();

            let error = sensitive_provider_runtime_files(&runtime, &BTreeSet::new()).unwrap_err();
            assert!(error.to_string().contains("credential symlink"), "{hidden}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn managed_runtime_alias_to_host_profile_is_rejected() {
        let home = tempfile::tempdir().unwrap();
        let profile = home.path().join(".hermes");
        let versions = home.path().join(".nvm").join("versions");
        let alias = versions.join("v24");
        fs::create_dir_all(profile.join("bin")).unwrap();
        fs::create_dir_all(&versions).unwrap();
        std::os::unix::fs::symlink(&profile, &alias).unwrap();

        let error = managed_runtime_root(&alias.join("bin"), home.path()).unwrap_err();
        assert!(error.to_string().contains("must not be symlinked"));
    }

    #[cfg(unix)]
    #[test]
    fn managed_runtime_alias_cannot_escape_home() {
        let home = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let versions = home.path().join(".nvm").join("versions");
        let alias = versions.join("v24");
        fs::create_dir_all(external.path().join("bin")).unwrap();
        fs::create_dir_all(&versions).unwrap();
        std::os::unix::fs::symlink(external.path(), &alias).unwrap();

        let error = managed_runtime_root(&alias.join("bin"), home.path()).unwrap_err();
        assert!(error.to_string().contains("escapes daemon HOME"));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn provider_install_candidate_rejects_root_and_home_ancestors() {
        let home = Path::new("/home/daemon");
        assert!(!provider_install_candidate_allowed(
            Path::new("/"),
            Some(home)
        ));
        assert!(!provider_install_candidate_allowed(
            Path::new("/home"),
            Some(home)
        ));
        assert!(!provider_install_candidate_allowed(home, Some(home)));
        assert!(provider_install_candidate_allowed(
            Path::new("/home/daemon/.nvm/versions/node/v24"),
            Some(home)
        ));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn provider_install_fallback_cannot_expose_home() {
        let home = tempfile::tempdir().unwrap();
        let executable = home.path().join("provider");

        let error = provider_install_root(&executable, home.path(), Some(home.path())).unwrap_err();
        assert!(error.to_string().contains("would expose the daemon home"));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn provider_install_cannot_overlap_host_profile() {
        let root = tempfile::tempdir().unwrap();
        let install = root.path().join("real-profile");
        let profile_alias = root.path().join("profile-alias");
        fs::create_dir_all(&install).unwrap();
        std::os::unix::fs::symlink(&install, &profile_alias).unwrap();

        let error =
            reject_provider_install_source_overlap(&install, profile_alias.to_str().unwrap())
                .unwrap_err();
        assert!(error.to_string().contains("overlaps host provider profile"));
    }

    #[test]
    fn hermes_profile_credentials_are_separate_from_runtime_code() {
        let home = tempfile::tempdir().unwrap();
        let runtime = home.path().join("hermes-agent");
        fs::create_dir_all(runtime.join(".git")).unwrap();
        fs::create_dir_all(runtime.join("website")).unwrap();
        fs::write(runtime.join(".envrc"), "TOKEN=secret\n").unwrap();
        fs::write(runtime.join("website").join(".npmrc"), "TOKEN=secret\n").unwrap();
        let mut sensitive = BTreeSet::new();
        add_hermes_sensitive_paths(&mut sensitive, home.path());
        sensitive.extend(sensitive_provider_runtime_files(&runtime, &BTreeSet::new()).unwrap());

        assert!(sensitive.contains(&SensitivePath {
            path: home.path().join("auth.json"),
            kind: SensitivePathKind::File,
        }));
        assert!(sensitive.contains(&SensitivePath {
            path: runtime.join(".envrc"),
            kind: SensitivePathKind::File,
        }));
        assert!(sensitive.contains(&SensitivePath {
            path: runtime.join(".git"),
            kind: SensitivePathKind::Directory,
        }));
        assert!(sensitive.contains(&SensitivePath {
            path: runtime.join("website").join(".npmrc"),
            kind: SensitivePathKind::File,
        }));
        assert!(!sensitive.contains(&SensitivePath {
            path: runtime,
            kind: SensitivePathKind::Directory,
        }));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn hermes_wrapper_adds_only_its_runtime_closure() {
        let home = tempfile::tempdir().unwrap();
        let source = home.path().join(".hermes");
        let runtime = source.join("hermes-agent");
        let uv_runtime = home
            .path()
            .join(".local")
            .join("share")
            .join("uv")
            .join("python")
            .join("cpython-test");
        let executable = home.path().join(".local").join("bin").join("hermes");
        fs::create_dir_all(runtime.join("venv").join("bin")).unwrap();
        fs::create_dir_all(uv_runtime.join("bin")).unwrap();
        fs::create_dir_all(uv_runtime.join("lib")).unwrap();
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(uv_runtime.join("bin").join("python3"), "").unwrap();
        std::os::unix::fs::symlink(
            uv_runtime.join("bin").join("python3"),
            runtime.join("venv").join("bin").join("python"),
        )
        .unwrap();
        fs::write(runtime.join("hermes"), "").unwrap();
        fs::write(
            &executable,
            format!(
                "exec \"{}/venv/bin/python\" \"{}/hermes\" \"$@\"\n",
                runtime.display(),
                runtime.display()
            ),
        )
        .unwrap();

        let mut expected = vec![
            fs::canonicalize(runtime).unwrap(),
            fs::canonicalize(uv_runtime).unwrap(),
        ];
        expected.sort();
        assert_eq!(
            hermes_runtime_roots(
                "hermes",
                &executable,
                Some(home.path()),
                source.to_str().unwrap(),
            )
            .unwrap(),
            expected
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_system_symlink_aliases_are_bidirectional() {
        for (public, private) in [
            ("/var/folders/task/.env", "/private/var/folders/task/.env"),
            ("/tmp/task/.env", "/private/tmp/task/.env"),
            ("/etc/hosts", "/private/etc/hosts"),
        ] {
            assert!(macos_path_aliases(Path::new(public)).contains(Path::new(private)));
            assert!(macos_path_aliases(Path::new(private)).contains(Path::new(public)));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_profile_denies_sensitive_files_after_readable_roots() {
        let mut profile = "(allow file-read* (subpath \"/task\"))\n".to_string();
        append_macos_sensitive_rules(
            &mut profile,
            &[SensitivePath {
                path: PathBuf::from("/task/.env.local"),
                kind: SensitivePathKind::File,
            }],
        );
        assert!(profile.contains("(deny file-read* (literal \"/task/.env.local\"))"));
        assert!(profile.contains("(deny file-write* (literal \"/task/.env.local\"))"));
        assert!(profile.contains("(deny file-map-executable (literal \"/task/.env.local\"))"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_runtime_masks_task_env_but_keeps_template_readable() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        let temp = root.path().join("tmp");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(&temp).unwrap();
        let secret = work.join(".env.local");
        let template = work.join(".env.example");
        fs::write(&secret, "PROVIDER_TOKEN=long-lived-secret\n").unwrap();
        fs::write(&template, "PROVIDER_TOKEN=replace-me\n").unwrap();

        let mut denied = RuntimeCommand::new("/bin/cat", Vec::new());
        isolate_provider_command(
            &mut denied,
            ProviderIsolation {
                provider: "codex",
                task_root: root.path().to_str().unwrap(),
                work_dir: work.to_str().unwrap(),
                temp_dir: temp.to_str().unwrap(),
                provider_source_home: "",
            },
        )
        .unwrap();
        let denied_output = std::process::Command::new(&denied.path)
            .args(denied.argv(&[secret.to_string_lossy().into_owned()]))
            .output()
            .unwrap();
        assert!(!denied_output.status.success());
        assert!(!String::from_utf8_lossy(&denied_output.stdout).contains("long-lived-secret"));

        let mut allowed = RuntimeCommand::new("/bin/cat", Vec::new());
        isolate_provider_command(
            &mut allowed,
            ProviderIsolation {
                provider: "codex",
                task_root: root.path().to_str().unwrap(),
                work_dir: work.to_str().unwrap(),
                temp_dir: temp.to_str().unwrap(),
                provider_source_home: "",
            },
        )
        .unwrap();
        let allowed_output = std::process::Command::new(&allowed.path)
            .args(allowed.argv(&[template.to_string_lossy().into_owned()]))
            .output()
            .unwrap();
        assert!(
            allowed_output.status.success(),
            "sandboxed template read failed with {:?}: {}",
            allowed_output.status,
            String::from_utf8_lossy(&allowed_output.stderr)
        );
        assert!(String::from_utf8_lossy(&allowed_output.stdout).contains("replace-me"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_runtime_cannot_read_host_process_environment() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        let temp = root.path().join("tmp");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(&temp).unwrap();
        let mut sentinel = std::process::Command::new("/bin/sleep")
            .arg("30")
            .env("PATCHBAY_HOST_SECRET_SENTINEL", "must-not-be-visible")
            .spawn()
            .unwrap();

        let mut command = RuntimeCommand::new("/bin/ps", Vec::new());
        isolate_provider_command(
            &mut command,
            ProviderIsolation {
                provider: "codex",
                task_root: root.path().to_str().unwrap(),
                work_dir: work.to_str().unwrap(),
                temp_dir: temp.to_str().unwrap(),
                provider_source_home: "",
            },
        )
        .unwrap();
        let output = std::process::Command::new(&command.path)
            .args(command.argv(&["e".to_string(), "-p".to_string(), sentinel.id().to_string()]))
            .output()
            .unwrap();
        let _ = sentinel.kill();
        let _ = sentinel.wait();

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.contains("PATCHBAY_HOST_SECRET_SENTINEL"));
        assert!(!stdout.contains("must-not-be-visible"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_masks_task_env_inside_home_without_mounting_host_root() {
        let mut args = Vec::new();
        append_linux_sensitive_mounts(
            &mut args,
            &[
                SensitivePath {
                    path: PathBuf::from("/home/daemon/.codex/auth.json"),
                    kind: SensitivePathKind::File,
                },
                SensitivePath {
                    path: PathBuf::from("/home/daemon/task/.env.local"),
                    kind: SensitivePathKind::File,
                },
            ],
            &[Path::new("/home/daemon/task")],
        );
        assert_eq!(
            args,
            ["--ro-bind", "/dev/null", "/home/daemon/task/.env.local"]
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        );
        assert!(!args.windows(3).any(|window| {
            window
                .iter()
                .map(String::as_str)
                .eq(["--ro-bind", "/", "/"])
        }));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_masks_credentials_reexposed_by_managed_runtime_mounts() {
        let credential = SensitivePath {
            path: PathBuf::from("/home/daemon/.nvm/versions/node/v24/.credentials.json"),
            kind: SensitivePathKind::File,
        };
        let mut args = Vec::new();
        append_linux_sensitive_mounts(
            &mut args,
            &[credential],
            &[Path::new("/home/daemon/.nvm/versions/node/v24")],
        );

        assert_eq!(
            args,
            [
                "--ro-bind",
                "/dev/null",
                "/home/daemon/.nvm/versions/node/v24/.credentials.json",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
        );
        assert!(LINUX_READABLE_DIRECTORIES.contains(&"/home/linuxbrew/.linuxbrew/Cellar"));
    }
}

#[cfg(target_os = "linux")]
fn append_linux_sensitive_mounts(
    args: &mut Vec<String>,
    sensitive_paths: &[SensitivePath],
    visible_roots: &[&Path],
) {
    for sensitive in sensitive_paths {
        let visible = visible_roots
            .iter()
            .any(|root| sensitive.path.starts_with(root));
        if !visible {
            continue;
        }
        match sensitive.kind {
            SensitivePathKind::File => {
                args.push("--ro-bind".to_string());
                args.push("/dev/null".to_string());
                args.push(sensitive.path.to_string_lossy().into_owned());
            }
            SensitivePathKind::Directory => {
                args.push("--tmpfs".to_string());
                args.push(sensitive.path.to_string_lossy().into_owned());
            }
        }
    }
}

#[cfg(target_os = "linux")]
const LINUX_READABLE_DIRECTORIES: &[&str] = &[
    "/bin",
    "/sbin",
    "/lib",
    "/lib64",
    "/usr/bin",
    "/usr/sbin",
    "/usr/lib",
    "/usr/lib64",
    "/usr/share",
    "/usr/local/bin",
    "/usr/local/lib",
    "/usr/local/lib64",
    "/usr/local/share",
    "/opt/homebrew/bin",
    "/opt/homebrew/lib",
    "/opt/homebrew/share",
    "/opt/homebrew/opt",
    "/home/linuxbrew/.linuxbrew/bin",
    "/home/linuxbrew/.linuxbrew/lib",
    "/home/linuxbrew/.linuxbrew/share",
    "/home/linuxbrew/.linuxbrew/opt",
    "/home/linuxbrew/.linuxbrew/Cellar",
    "/nix/store",
    "/run/current-system",
    "/etc/ssl/certs",
    "/etc/pki/ca-trust",
    "/etc/ca-certificates",
];

#[cfg(target_os = "linux")]
struct LinuxReadableRoots {
    all: Vec<PathBuf>,
    provider: Vec<PathBuf>,
}

#[cfg(target_os = "linux")]
fn linux_readable_roots(
    executable: &Path,
    provider_root: &Path,
    home: &Path,
    provider: &str,
    provider_source_home: &str,
) -> anyhow::Result<LinuxReadableRoots> {
    let mut roots = BTreeSet::new();
    let mut provider_roots = BTreeSet::new();
    for path in LINUX_READABLE_DIRECTORIES {
        let path = PathBuf::from(path);
        if path.exists() {
            roots.insert(path);
        }
    }
    for path in [
        "/etc/resolv.conf",
        "/etc/hosts",
        "/etc/nsswitch.conf",
        "/etc/passwd",
        "/etc/group",
        "/etc/localtime",
        "/etc/ld.so.cache",
        "/etc/os-release",
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/pki/tls/certs/ca-bundle.crt",
    ] {
        let path = PathBuf::from(path);
        if path.exists() {
            roots.insert(path);
        }
    }
    let provider_install = provider_install_root(executable, provider_root, Some(home))?;
    reject_provider_install_source_overlap(&provider_install, provider_source_home)?;
    provider_roots.insert(provider_install);
    for runtime in hermes_runtime_roots(provider, executable, Some(home), provider_source_home)? {
        provider_roots.insert(runtime);
    }
    for path in managed_runtime_roots(home)? {
        provider_roots.insert(path);
    }
    provider_roots.retain(|path| path.is_dir());
    roots.extend(provider_roots.iter().cloned());
    anyhow::ensure!(
        roots
            .iter()
            .all(|path| path.is_absolute() && path != Path::new("/")),
        "provider sandbox readable root is not absolute"
    );
    Ok(LinuxReadableRoots {
        all: roots.into_iter().collect(),
        provider: provider_roots.into_iter().collect(),
    })
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn provider_install_root(
    executable: &Path,
    fallback: &Path,
    home: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    for ancestor in executable.ancestors().skip(1).take(8) {
        if provider_install_candidate_allowed(ancestor, home)
            && (ancestor.join("package.json").is_file() || ancestor.join("pyvenv.cfg").is_file())
        {
            return Ok(ancestor.to_path_buf());
        }
    }
    anyhow::ensure!(
        provider_install_candidate_allowed(fallback, home),
        "provider install root {} would expose the daemon home",
        fallback.display()
    );
    Ok(fallback.to_path_buf())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn reject_provider_install_source_overlap(
    provider_install: &Path,
    provider_source_home: &str,
) -> anyhow::Result<()> {
    if provider_source_home.trim().is_empty() {
        return Ok(());
    }
    let provider_install = fs::canonicalize(provider_install)
        .context("resolve provider install root before credential overlap check")?;
    let source = absolute_path(provider_source_home, "provider source home")?;
    let source = match fs::canonicalize(&source) {
        Ok(canonical) => canonical,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => source,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("resolve provider source home {}", source.display()))
        }
    };
    anyhow::ensure!(
        !provider_install.starts_with(&source) && !source.starts_with(&provider_install),
        "provider install root {} overlaps host provider profile {}",
        provider_install.display(),
        source.display()
    );
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn hermes_runtime_roots(
    provider: &str,
    executable: &Path,
    home: Option<&Path>,
    provider_source_home: &str,
) -> anyhow::Result<Vec<PathBuf>> {
    if provider != "hermes" || executable.file_name().is_none_or(|name| name != "hermes") {
        return Ok(Vec::new());
    }
    let wrapper = fs::read_to_string(executable).unwrap_or_default();
    let mut candidates = Vec::new();
    if let Some(home) = home {
        candidates.push(home.join(".hermes").join("hermes-agent"));
    }
    if !provider_source_home.trim().is_empty() {
        let source = absolute_path(provider_source_home, "Hermes source home")?;
        candidates.push(hermes_root_for_source(&source).join("hermes-agent"));
    }
    candidates.sort();
    candidates.dedup();
    let mut runtime_root = None;
    for candidate in candidates {
        let candidate_text = candidate.to_string_lossy();
        if wrapper.contains(candidate_text.as_ref())
            && candidate.join("venv").join("bin").join("python").is_file()
            && candidate.join("hermes").is_file()
        {
            runtime_root = Some(canonical_existing(&candidate_text, "Hermes runtime root")?);
            break;
        }
    }
    let Some(runtime_root) = runtime_root else {
        anyhow::ensure!(
            !wrapper.contains("hermes-agent"),
            "Hermes wrapper runtime closure could not be isolated"
        );
        return Ok(Vec::new());
    };
    let interpreter = fs::canonicalize(runtime_root.join("venv").join("bin").join("python"))
        .context("resolve Hermes Python interpreter")?;
    let mut roots = vec![runtime_root.clone()];
    if !interpreter.starts_with(&runtime_root) && !system_runtime_contains(&interpreter) {
        roots.push(uv_python_runtime_root(&interpreter, home)?);
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn system_runtime_contains(path: &Path) -> bool {
    [
        "/System",
        "/bin",
        "/usr/bin",
        "/usr/lib",
        "/usr/local/bin",
        "/usr/local/lib",
        "/usr/local/Cellar",
        "/opt/homebrew/bin",
        "/opt/homebrew/lib",
        "/opt/homebrew/Cellar",
        "/home/linuxbrew/.linuxbrew",
        "/nix/store",
    ]
    .iter()
    .any(|root| path.starts_with(root))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn uv_python_runtime_root(interpreter: &Path, home: Option<&Path>) -> anyhow::Result<PathBuf> {
    let home = home.ok_or_else(|| anyhow!("daemon HOME is unavailable for Hermes isolation"))?;
    let bases = [
        home.join(".local").join("share").join("uv").join("python"),
        home.join("Library")
            .join("Application Support")
            .join("uv")
            .join("python"),
    ];
    for base in bases {
        let base = fs::canonicalize(&base).unwrap_or(base);
        let Ok(relative) = interpreter.strip_prefix(&base) else {
            continue;
        };
        let Some(version) = relative.components().next() else {
            continue;
        };
        let root = base.join(Path::new(version.as_os_str()));
        anyhow::ensure!(
            interpreter.starts_with(root.join("bin"))
                && root.join("bin").is_dir()
                && root.join("lib").is_dir(),
            "Hermes Python runtime closure is incomplete"
        );
        return fs::canonicalize(root).context("resolve Hermes uv Python runtime root");
    }
    anyhow::bail!(
        "Hermes interpreter {} is outside an allowed runtime",
        interpreter.display()
    )
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn provider_install_candidate_allowed(candidate: &Path, home: Option<&Path>) -> bool {
    candidate.is_absolute()
        && candidate != Path::new("/")
        && home.is_none_or(|home| candidate != home && !home.starts_with(candidate))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn managed_runtime_roots(home: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let Some(path) = std::env::var_os("PATH") else {
        return Ok(Vec::new());
    };
    let mut roots = BTreeSet::new();
    for directory in std::env::split_paths(&path) {
        if let Some(root) = managed_runtime_root(&directory, home)? {
            roots.insert(root);
        }
    }
    Ok(roots.into_iter().collect())
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn managed_runtime_root(directory: &Path, home: &Path) -> anyhow::Result<Option<PathBuf>> {
    if !directory.is_absolute() || !directory.is_dir() || !directory.starts_with(home) {
        return Ok(None);
    }
    let text = directory.to_string_lossy();
    let managed = [
        "/.nvm/versions/",
        "/.pyenv/versions/",
        "/.asdf/installs/",
        "/.local/share/mise/installs/",
    ]
    .iter()
    .any(|marker| text.contains(marker));
    if !managed || directory.file_name().is_none_or(|name| name != "bin") {
        return Ok(None);
    }
    let lexical_root = directory
        .parent()
        .ok_or_else(|| anyhow!("managed runtime bin has no parent"))?;
    let canonical_home = fs::canonicalize(home).context("resolve daemon HOME")?;
    let canonical_root = fs::canonicalize(lexical_root).context("resolve managed runtime root")?;
    let canonical_bin = fs::canonicalize(directory).context("resolve managed runtime bin")?;
    anyhow::ensure!(
        canonical_root != canonical_home && canonical_root.starts_with(&canonical_home),
        "managed runtime root escapes daemon HOME"
    );
    anyhow::ensure!(
        canonical_root == lexical_root && canonical_bin == canonical_root.join("bin"),
        "managed runtime root must not be symlinked"
    );
    Ok(Some(canonical_root))
}

#[cfg(target_os = "linux")]
fn add_mount_target_parents(args: &mut Vec<String>, targets: &[PathBuf]) -> anyhow::Result<()> {
    let mut directories = BTreeSet::new();
    for target in targets {
        anyhow::ensure!(target.is_absolute(), "sandbox mount target is not absolute");
        let mut parents = target.ancestors().skip(1).collect::<Vec<_>>();
        parents.reverse();
        for parent in parents {
            if parent != Path::new("/") {
                directories.insert(parent.to_path_buf());
            }
        }
    }
    for directory in directories {
        args.push("--dir".to_string());
        args.push(directory.to_string_lossy().into_owned());
    }
    Ok(())
}
