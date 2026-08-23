//! Port of `server/internal/daemon/slash_skill.go` — extraction of
//! `[label](slash://skill/<id>)` references from markdown chat messages.
//!
//! Symbol map:
//! - `SlashSkillRef` → [`SlashSkillRef`]
//! - `ExtractSlashSkills` → [`extract_slash_skills`]

use std::collections::HashSet;

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashSkillRef {
    pub label: String,
    pub id: String,
}

/// `slashSkillRe`: `\[/((?:[^\]\\]|\\.)+)\]\(slash://skill/([^)]+)\)`
///
/// Note the protocol segment is exactly `skill` — `slash://action/…`,
/// `slash://skills/…`, and `slash://skill-extra/…` must NOT match
/// (TestExtractSlashSkillsDoesNotMatchPartialProtocol).
fn slash_skill_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\[/((?:[^\[\]\\]|\\.)+)\]\(slash://skill/([^)]+)\)").expect("static regex")
    })
}

pub fn extract_slash_skills(md: &str) -> Vec<SlashSkillRef> {
    let re = slash_skill_re();
    let mut seen = HashSet::new();
    let mut refs = Vec::new();
    for cap in re.captures_iter(md) {
        let id = cap.get(2).map(|m| m.as_str()).unwrap_or_default();
        if !seen.insert(id.to_string()) {
            continue;
        }
        // Unescape the label the way Go's strings.ReplaceAll pair does:
        // `\[` → `[`, then `\]` → `]`.
        let label = cap
            .get(1)
            .map(|m| m.as_str())
            .unwrap_or_default()
            .replace("\\[", "[")
            .replace("\\]", "]");
        refs.push(SlashSkillRef { label, id: id.to_string() });
    }
    refs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_link() {
        let refs = extract_slash_skills("please [/deploy](slash://skill/abc-123) this");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].label, "deploy");
        assert_eq!(refs[0].id, "abc-123");
    }

    #[test]
    fn parses_escaped_brackets() {
        let refs = extract_slash_skills("[/deploy\\[prod\\]](slash://skill/x)");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].label, "deploy[prod]");
    }

    #[test]
    fn deduplicates_by_id() {
        let refs = extract_slash_skills("[/a](slash://skill/same) and [/b](slash://skill/same)");
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn ignores_slash_action_links() {
        assert!(extract_slash_skills("[/x](slash://action/y)").is_empty());
    }

    #[test]
    fn ignores_normal_markdown_links() {
        assert!(extract_slash_skills("[docs](https://example.com)").is_empty());
    }

    #[test]
    fn ignores_mention_links() {
        assert!(extract_slash_skills("[@user](mention://member/id)").is_empty());
    }

    #[test]
    fn extracts_multiple_distinct_skills() {
        let refs = extract_slash_skills("[/a](slash://skill/id-1) and [/b](slash://skill/id-2)");
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn does_not_match_partial_protocol() {
        for md in ["[/x](slash://y)", "[/x](slash://skills/y)", "[/x](slash://skill-extra/y)"] {
            assert!(
                extract_slash_skills(md).is_empty(),
                "expected 0 refs for {md}"
            );
        }
    }
}
