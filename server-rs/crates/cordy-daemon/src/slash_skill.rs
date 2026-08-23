//! Port of `server/internal/daemon/slash_skill.go` (35 lines).
//!
//! Deviations from Go:
//! - `regexp.MustCompile` → [`regex::Regex`] in a [`std::sync::LazyLock`]
//!   (same pattern text, RE2-compatible).
//! - No other deviations; slog is not used in this file.

// S9-integration: consumed by prompt.rs (ported in this lane) and manager
// wiring that lands with integration.
#![allow(dead_code)]

use std::sync::LazyLock;

use regex::Regex;

/// `slashSkillRe` (slash_skill.go:8–10).
static SLASH_SKILL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\[/((?:[^\\\]]|\\.)+)\]\(slash://skill/([^)]+)\)"#).expect("valid static regex")
});

/// `SlashSkillRef` (slash_skill.go:12–15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlashSkillRef {
    pub label: String,
    pub id: String,
}

/// `ExtractSlashSkills` (slash_skill.go:17–35): extracts `[/label](slash://skill/id)`
/// references from markdown, unescaping `\[`/`\]` in labels and deduplicating
/// by skill ID (first occurrence wins).
pub(crate) fn extract_slash_skills(md: &str) -> Vec<SlashSkillRef> {
    let mut seen = std::collections::HashSet::new();
    let mut refs = Vec::new();

    for caps in SLASH_SKILL_RE.captures_iter(md) {
        let id = &caps[2];
        if !seen.insert(id.to_string()) {
            continue;
        }

        let label = caps[1].replace(r"\[", "[").replace(r"\]", "]");
        refs.push(SlashSkillRef { label, id: id.to_string() });
    }

    refs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TestExtractSlashSkills/parses_basic_link (slash_skill_test.go:6–14).
    #[test]
    fn parses_basic_link() {
        let refs = extract_slash_skills("please [/deploy](slash://skill/abc-123) this");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].label, "deploy");
        assert_eq!(refs[0].id, "abc-123");
    }

    /// TestExtractSlashSkills/parses_escaped_brackets (slash_skill_test.go:16–21).
    #[test]
    fn parses_escaped_brackets() {
        let refs = extract_slash_skills(r#"[/deploy\[prod\]](slash://skill/x)"#);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].label, "deploy[prod]");
    }

    /// TestExtractSlashSkills/deduplicates_by_ID (slash_skill_test.go:23–28).
    #[test]
    fn deduplicates_by_id() {
        let refs = extract_slash_skills("[/a](slash://skill/same) and [/b](slash://skill/same)");
        assert_eq!(refs.len(), 1);
    }

    /// TestExtractSlashSkills/ignores_slash_action_links (slash_skill_test.go:30–35).
    #[test]
    fn ignores_slash_action_links() {
        let refs = extract_slash_skills("[/x](slash://action/y)");
        assert!(refs.is_empty());
    }

    /// TestExtractSlashSkills/ignores_normal_markdown_links (slash_skill_test.go:37–42).
    #[test]
    fn ignores_normal_markdown_links() {
        let refs = extract_slash_skills("[docs](https://example.com)");
        assert!(refs.is_empty());
    }

    /// TestExtractSlashSkills/ignores_mention_links (slash_skill_test.go:44–49).
    #[test]
    fn ignores_mention_links() {
        let refs = extract_slash_skills("[@user](mention://member/id)");
        assert!(refs.is_empty());
    }

    /// TestExtractSlashSkills/extracts_multiple_distinct_skills
    /// (slash_skill_test.go:51–56).
    #[test]
    fn extracts_multiple_distinct_skills() {
        let refs = extract_slash_skills("[/a](slash://skill/id-1) and [/b](slash://skill/id-2)");
        assert_eq!(refs.len(), 2);
    }

    /// TestExtractSlashSkillsDoesNotMatchPartialProtocol
    /// (slash_skill_test.go:59–69).
    #[test]
    fn does_not_match_partial_protocol() {
        for md in ["[/x](slash://y)", "[/x](slash://skills/y)", "[/x](slash://skill-extra/y)"] {
            assert!(extract_slash_skills(md).is_empty(), "expected 0 refs for {md:?}");
        }
    }
}
