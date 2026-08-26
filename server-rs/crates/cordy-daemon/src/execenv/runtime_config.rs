//! Port of `execenv/runtime_config.go`.
//!
//! The runtime brief is deliberately written through the provider's native
//! project configuration file. The marker protocol preserves user-authored
//! local-repository instructions, replaces the managed block idempotently,
//! and restores the exact pre-task bytes when a local directory/worktree is
//! finalized.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;

use anyhow::Context;

use super::execenv::TaskContextForEnv;

pub(crate) const RUNTIME_MARKER_BEGIN: &str =
    "<!-- BEGIN CORDY-RUNTIME (auto-managed; do not edit) -->";
pub(crate) const RUNTIME_MARKER_END: &str = "<!-- END CORDY-RUNTIME -->";
pub(crate) const RUNTIME_MANAGED_SEPARATOR: &str = "\n\n";

fn write_new_or_existing(path: &Path, bytes: &[u8], _was_missing: bool) -> anyhow::Result<()> {
    // Match Go's os.WriteFile(path, bytes, 0o644): the requested mode is
    // applied only when creating a file and the kernel still applies umask.
    // Calling set_permissions after fs::write would force 0644 even under a
    // restrictive umask, making task/user context readable by other users on
    // a shared host.
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("write runtime config {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("write runtime config {}", path.display()))?;
    Ok(())
}

/// Return the provider-native config path, or `None` when the provider has no
/// file target and must receive the brief through its inline prompt path.
pub(crate) fn runtime_config_path(work_dir: &str, provider: &str) -> Option<String> {
    let family = crate::agents_probe::builtin_runtime_by_id(provider)
        .map(|runtime| runtime.protocol_family)
        .unwrap_or(provider);
    let filename = match family {
        "claude" => "CLAUDE.md",
        "codebuddy" => "CODEBUDDY.md",
        "qwen" => "QWEN.md",
        "codex" | "copilot" | "opencode" | "deveco" | "openclaw" | "hermes" | "pi" | "cursor"
        | "kimi" | "reasonix" | "dsh" | "kiro" | "antigravity" | "qoder" | "qoderclicn"
        | "traecli" | "grok" | "qwenpaw" | "mcode" | "dim" => "AGENTS.md",
        _ => return None,
    };
    Some(
        Path::new(work_dir)
            .join(filename)
            .to_string_lossy()
            .into_owned(),
    )
}

/// Locate a managed marker block. The end marker is searched strictly after
/// the begin marker. A begin marker without an end consumes the rest of the
/// file so a partially-written block is replaced instead of stacked.
pub(crate) fn locate_marker_block(content: &str) -> Option<(usize, usize)> {
    locate_marker_block_bytes(content.as_bytes())
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn locate_marker_block_bytes(content: &[u8]) -> Option<(usize, usize)> {
    let start = find_bytes(content, RUNTIME_MARKER_BEGIN.as_bytes())?;
    let after_begin = start + RUNTIME_MARKER_BEGIN.len();
    let end = match find_bytes(&content[after_begin..], RUNTIME_MARKER_END.as_bytes()) {
        Some(relative) => {
            let mut end = after_begin + relative + RUNTIME_MARKER_END.len();
            if content.get(end) == Some(&b'\n') {
                end += 1;
            }
            end
        }
        None => content.len(),
    };
    Some((start, end))
}

fn runtime_marker_block(brief: &str) -> String {
    format!(
        "{RUNTIME_MARKER_BEGIN}\n{}\n{RUNTIME_MARKER_END}\n",
        brief.trim_end_matches('\n')
    )
}

/// Write one managed runtime block without touching user content.
pub(crate) fn write_runtime_config_file(path: &Path, brief: &str) -> anyhow::Result<()> {
    let block = runtime_marker_block(brief);
    let existing = match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("read existing runtime config {}", path.display()));
        }
    };

    let Some(existing) = existing else {
        return write_new_or_existing(path, block.as_bytes(), true);
    };
    let content = if let Some((start, end)) = locate_marker_block_bytes(&existing) {
        let mut content = Vec::with_capacity(existing.len() + block.len());
        content.extend_from_slice(&existing[..start]);
        content.extend_from_slice(block.as_bytes());
        content.extend_from_slice(&existing[end..]);
        content
    } else {
        let mut content =
            Vec::with_capacity(existing.len() + RUNTIME_MANAGED_SEPARATOR.len() + block.len());
        content.extend_from_slice(&existing);
        content.extend_from_slice(RUNTIME_MANAGED_SEPARATOR.as_bytes());
        content.extend_from_slice(block.as_bytes());
        content
    };
    write_new_or_existing(path, &content, false)
}

/// Inject the stable runtime brief into the provider-native configuration
/// file. Unknown providers return the assembled brief but do not write a file,
/// matching Go's prompt-only fallback.
pub(crate) fn inject_runtime_config(
    work_dir: &str,
    provider: &str,
    ctx: &TaskContextForEnv,
) -> anyhow::Result<String> {
    let brief = crate::runtime_config_sections::build_meta_skill_content(provider, ctx);
    let Some(path) = runtime_config_path(work_dir, provider) else {
        return Ok(brief);
    };
    write_runtime_config_file(Path::new(&path), &brief)?;
    Ok(brief)
}

/// Remove only the managed marker block and its fixed separator. A file
/// created from a missing-file state is removed; a pre-existing file,
/// including an empty or whitespace-only file, is retained byte-for-byte.
pub(crate) fn cleanup_runtime_config(work_dir: &str, provider: &str) -> anyhow::Result<()> {
    let Some(path) = runtime_config_path(work_dir, provider) else {
        return Ok(());
    };
    let path = Path::new(&path);
    let existing = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("read runtime config {}", path.display()));
        }
    };
    let Some((start, end)) = locate_marker_block_bytes(&existing) else {
        return Ok(());
    };

    let mut pre = &existing[..start];
    let post = &existing[end..];
    let had_separator = pre.ends_with(RUNTIME_MANAGED_SEPARATOR.as_bytes());
    if had_separator {
        pre = &pre[..pre.len() - RUNTIME_MANAGED_SEPARATOR.len()];
    }
    let mut remainder = Vec::with_capacity(pre.len() + post.len());
    remainder.extend_from_slice(pre);
    remainder.extend_from_slice(post);
    if !had_separator && remainder.is_empty() {
        match fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("remove runtime config {}", path.display()));
            }
        }
    }
    // Existing files are rewritten without changing their existing mode.
    write_new_or_existing(path, &remainder, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_path(dir: &Path, provider: &str) -> std::path::PathBuf {
        Path::new(&runtime_config_path(&dir.to_string_lossy(), provider).unwrap()).to_path_buf()
    }

    #[test]
    fn provider_paths_include_builtin_family() {
        let dir = Path::new("/tmp/cordy-runtime-config-test");
        assert_eq!(config_path(dir, "claude"), dir.join("CLAUDE.md"));
        assert_eq!(config_path(dir, "codebuddy"), dir.join("CODEBUDDY.md"));
        assert_eq!(config_path(dir, "qwen"), dir.join("QWEN.md"));
        assert_eq!(config_path(dir, "codex"), dir.join("AGENTS.md"));
        assert_eq!(config_path(dir, "omp"), dir.join("AGENTS.md"));
        assert!(runtime_config_path(&dir.to_string_lossy(), "unknown").is_none());
    }

    #[test]
    fn marker_writer_preserves_and_replaces_user_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        let user = "# User instructions\nkeep this exact\n";
        fs::write(&path, user).unwrap();
        write_runtime_config_file(&path, "first\n").unwrap();
        let injected = String::from_utf8(fs::read(&path).unwrap()).unwrap();
        assert!(injected.starts_with(user));
        assert_eq!(injected.matches(RUNTIME_MARKER_BEGIN).count(), 1);
        write_runtime_config_file(&path, "second\n\n").unwrap();
        let replaced = String::from_utf8(fs::read(&path).unwrap()).unwrap();
        assert_eq!(replaced.matches(RUNTIME_MARKER_BEGIN).count(), 1);
        assert!(replaced.contains("second"));
        assert!(!replaced.contains("first"));
        cleanup_runtime_config(&dir.path().to_string_lossy(), "codex").unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), user);
    }

    #[test]
    fn marker_writer_round_trips_missing_and_empty_files() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.md");
        write_runtime_config_file(&missing, "brief").unwrap();
        assert!(missing.exists());
        let text = fs::read_to_string(&missing).unwrap();
        assert!(text.starts_with(RUNTIME_MARKER_BEGIN));
        cleanup_runtime_config(&dir.path().to_string_lossy(), "codex").unwrap();
        // The test uses a different filename, so exercise the public cleanup
        // contract with the provider-native target as well.
        let agents = dir.path().join("AGENTS.md");
        write_runtime_config_file(&agents, "brief").unwrap();
        cleanup_runtime_config(&dir.path().to_string_lossy(), "codex").unwrap();
        assert!(!agents.exists());

        fs::write(&agents, b"").unwrap();
        write_runtime_config_file(&agents, "brief").unwrap();
        cleanup_runtime_config(&dir.path().to_string_lossy(), "codex").unwrap();
        assert!(agents.exists());
        assert_eq!(fs::read(&agents).unwrap(), b"");
    }

    #[test]
    fn malformed_begin_is_replaced_and_cleanup_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        let malformed = format!("user\n{RUNTIME_MARKER_BEGIN}\npartial");
        fs::write(&path, malformed).unwrap();
        write_runtime_config_file(&path, "new").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches(RUNTIME_MARKER_BEGIN).count(), 1);
        assert!(text.contains("new"));
        cleanup_runtime_config(&dir.path().to_string_lossy(), "codex").unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "user\n");
    }

    #[test]
    fn cleanup_without_marker_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        fs::write(&path, "user\n").unwrap();
        cleanup_runtime_config(&dir.path().to_string_lossy(), "codex").unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "user\n");
    }
}
