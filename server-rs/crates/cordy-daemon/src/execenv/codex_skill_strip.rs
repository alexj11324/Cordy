//! Port of execenv/codex_skill_strip.go.
//!
//! Symbol map:
//! - stripSkillsConfigEntries   → strip_skills_config_entries
//! - sanitizeCopiedCodexConfig  → sanitize_copied_codex_config
//!
//! Background: Codex Desktop writes `[[skills.config]]` entries whose
//! plugin-backed members lack the required `path` field, which Codex CLI's
//! stricter TOML parser rejects (`missing field path`). Cordy copies the
//! user's config verbatim into each per-task codex-home, so the whole array is
//! stripped — Cordy writes assigned skills directly to codex-home/skills/.

use anyhow::Result;

/// Removes every `[[skills.config]]` array-of-tables block. Lines outside the
/// blocks are preserved untouched; trailing blank-line clusters are collapsed
/// so repeated copies don't grow the file unboundedly.
pub(crate) fn strip_skills_config_entries(content: &str) -> String {
    if !content.contains("[[skills.config]]") {
        return content.to_string();
    }

    let mut out: Vec<&str> = Vec::with_capacity(content.lines().count());
    let mut in_skills_config = false;
    for line in content.split('\n') {
        let trimmed = line.trim();
        // A new TOML header always closes the current [[skills.config]] block.
        if trimmed.starts_with('[') {
            if trimmed == "[[skills.config]]" {
                in_skills_config = true;
                continue;
            }
            in_skills_config = false;
            out.push(line);
            continue;
        }
        if in_skills_config {
            continue;
        }
        out.push(line);
    }

    let stripped = format!("{}\n", out.join("\n").trim_end_matches('\n'));
    if stripped.trim().is_empty() {
        return String::new();
    }
    stripped
}

/// Rewrites the per-task config.toml in place, dropping inherited
/// `[[skills.config]]` entries. No-op when absent or unchanged.
pub(crate) fn sanitize_copied_codex_config(config_path: &str) -> Result<()> {
    let data = match std::fs::read_to_string(config_path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(anyhow::anyhow!("read config.toml: {e}")),
    };
    let stripped = strip_skills_config_entries(&data);
    if stripped == data {
        return Ok(());
    }
    std::fs::write(config_path, stripped).map_err(|e| anyhow::anyhow!("write config.toml: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of TestStripSkillsConfigEntries: whole array removed, surrounding
    // content byte-preserved, trailing blank cluster collapsed.
    #[test]
    fn test_strip_skills_config_entries() {
        let input = "model = \"gpt\"\n\n[[skills.config]]\nname = \"a\"\npath = \"/x\"\n\n[[skills.config]]\nname = \"superpowers:brainstorming\"\n\n[profiles.fast]\nwins = true\n";
        let got = strip_skills_config_entries(input);
        assert!(got.starts_with("model = \"gpt\""), "{got}");
        assert!(!got.contains("skills.config"), "{got}");
        assert!(!got.contains("brainstorming"));
        assert!(got.contains("[profiles.fast]\nwins = true"));

        // Idempotent.
        assert_eq!(strip_skills_config_entries(&got), got);
    }

    // Port of TestStripSkillsConfigEntriesNoop: no marker → unchanged.
    #[test]
    fn test_noop_without_marker() {
        let input = "[features]\nmulti_agent = false\n";
        assert_eq!(strip_skills_config_entries(input), input);
    }

    // Port of TestStripSkillsConfigEntriesAllContent: stripping everything
    // yields empty string rather than a whitespace husk.
    #[test]
    fn test_all_content_stripped() {
        let input = "[[skills.config]]\nname = \"a\"\n";
        assert_eq!(strip_skills_config_entries(input), "");
    }

    // Port of TestSanitizeCopiedCodexConfigFileBehavior: missing file is a
    // no-op; changed content is written back; unchanged content isn't touched
    // (mtime preserved).
    #[test]
    fn test_sanitize_file_behavior() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("config.toml");

        // Missing file: no error, nothing created.
        sanitize_copied_codex_config(cfg.to_str().unwrap()).unwrap();
        assert!(!cfg.exists());

        // With entries: rewritten.
        std::fs::write(&cfg, "[[skills.config]]\nname = \"a\"\n").unwrap();
        sanitize_copied_codex_config(cfg.to_str().unwrap()).unwrap();
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            "",
            "all-skill-config file strips to empty"
        );

        // Clean content: write skipped (mtime stable).
        std::fs::write(&cfg, "model = \"gpt\"\n").unwrap();
        let before = std::fs::metadata(&cfg).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        sanitize_copied_codex_config(cfg.to_str().unwrap()).unwrap();
        let after = std::fs::metadata(&cfg).unwrap().modified().unwrap();
        assert_eq!(before, after, "unchanged content must not be rewritten");
    }
}
