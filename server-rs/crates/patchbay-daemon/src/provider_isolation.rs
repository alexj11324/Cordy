//! OS-enforced provider process isolation.
//!
//! Provider CLIs are untrusted task processes: they can spawn shells and read
//! arbitrary paths unless the first process is sandboxed. The wrapper built
//! here is inherited by every descendant. Unsupported hosts fail closed.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use patchbay_agent::RuntimeCommand;

pub(crate) struct ProviderIsolation<'a> {
    pub task_root: &'a str,
    pub work_dir: &'a str,
    pub temp_dir: &'a str,
}

pub(crate) fn isolate_provider_command(
    command: &mut RuntimeCommand,
    isolation: ProviderIsolation<'_>,
) -> anyhow::Result<()> {
    let executable = canonical_existing(&command.path, "provider executable")?;
    let task_root = canonical_existing(isolation.task_root, "task root")?;
    let work_dir = canonical_existing(isolation.work_dir, "task work directory")?;
    let temp_dir = canonical_existing(isolation.temp_dir, "task temp directory")?;

    #[cfg(target_os = "macos")]
    return isolate_macos(command, &executable, &task_root, &work_dir, &temp_dir);

    #[cfg(target_os = "linux")]
    return isolate_linux(command, &executable, &task_root, &work_dir, &temp_dir);

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (command, executable, task_root, work_dir, temp_dir);
        anyhow::bail!("provider task filesystem isolation is unsupported on this host")
    }
}

fn canonical_existing(path: &str, label: &str) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(!path.trim().is_empty(), "{label} is missing");
    std::fs::canonicalize(path).with_context(|| format!("resolve {label} {path:?}"))
}

fn executable_root(executable: &Path) -> anyhow::Result<PathBuf> {
    executable.parent().map(Path::to_path_buf).ok_or_else(|| {
        anyhow!(
            "provider executable has no parent: {}",
            executable.display()
        )
    })
}

#[cfg(target_os = "macos")]
fn isolate_macos(
    command: &mut RuntimeCommand,
    executable: &Path,
    task_root: &Path,
    work_dir: &Path,
    temp_dir: &Path,
) -> anyhow::Result<()> {
    const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
    anyhow::ensure!(
        Path::new(SANDBOX_EXEC).is_file(),
        "sandbox-exec is unavailable"
    );
    let provider_root = executable_root(executable)?;
    let readable = [
        Path::new("/System"),
        Path::new("/usr"),
        Path::new("/bin"),
        Path::new("/sbin"),
        Path::new("/Library/Apple"),
        Path::new("/opt/homebrew"),
        Path::new("/usr/local"),
        provider_root.as_path(),
        task_root,
        work_dir,
        temp_dir,
    ];
    let mut profile = String::from(
        "(version 1)\n\
         (deny default)\n\
         (allow process*)\n\
         (allow network*)\n\
         (allow sysctl-read)\n\
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
         (allow file-write* (literal \"/dev/null\"))\n",
    );
    for path in readable {
        profile.push_str(&format!(
            "(allow file-read* (subpath \"{}\"))\n",
            sandbox_quote(path)
        ));
    }
    for path in [task_root, work_dir, temp_dir] {
        profile.push_str(&format!(
            "(allow file-write* (subpath \"{}\"))\n",
            sandbox_quote(path)
        ));
    }
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

#[cfg(target_os = "linux")]
fn isolate_linux(
    command: &mut RuntimeCommand,
    executable: &Path,
    task_root: &Path,
    work_dir: &Path,
    temp_dir: &Path,
) -> anyhow::Result<()> {
    let bwrap = ["/usr/bin/bwrap", "/usr/local/bin/bwrap"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .ok_or_else(|| anyhow!("bubblewrap is required for provider task isolation"))?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| anyhow!("daemon HOME is unavailable for provider isolation"))?;
    let provider_root = executable_root(executable)?;
    let mut prefix = vec![
        "--die-with-parent".to_string(),
        "--new-session".to_string(),
        "--unshare-pid".to_string(),
        "--unshare-ipc".to_string(),
        "--unshare-uts".to_string(),
        "--unshare-cgroup".to_string(),
        "--ro-bind".to_string(),
        "/".to_string(),
        "/".to_string(),
        "--tmpfs".to_string(),
        home.to_string_lossy().into_owned(),
    ];
    add_hidden_home_parents(&mut prefix, &home, &provider_root)?;
    prefix.push("--ro-bind".to_string());
    prefix.push(provider_root.to_string_lossy().into_owned());
    prefix.push(provider_root.to_string_lossy().into_owned());
    for path in [task_root, work_dir, temp_dir] {
        add_hidden_home_parents(&mut prefix, &home, path)?;
        prefix.push("--bind".to_string());
        prefix.push(path.to_string_lossy().into_owned());
        prefix.push(path.to_string_lossy().into_owned());
    }
    prefix.extend([
        "--dev-bind".to_string(),
        "/dev".to_string(),
        "/dev".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
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

#[cfg(target_os = "linux")]
fn add_hidden_home_parents(
    args: &mut Vec<String>,
    home: &Path,
    target: &Path,
) -> anyhow::Result<()> {
    if !target.starts_with(home) {
        return Ok(());
    }
    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("sandbox bind target has no parent: {}", target.display()))?;
    let mut current = home.to_path_buf();
    let relative = parent
        .strip_prefix(home)
        .context("sandbox bind escaped daemon home")?;
    for component in relative.components() {
        current.push(component);
        args.push("--dir".to_string());
        args.push(current.to_string_lossy().into_owned());
    }
    Ok(())
}
