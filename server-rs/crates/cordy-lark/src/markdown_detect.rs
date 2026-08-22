//! Markdown detection — port of
//! `server/internal/integrations/lark/markdown_detect.go`.

use regex::Regex;
use std::sync::OnceLock;

/// markdownPatterns enumerate the syntax shapes we treat as evidence that the
/// agent's reply is markdown rather than prose. Each pattern is intentionally
/// conservative — better to false-positive (route plain text through the
/// markdown card, which still renders fine) than to false-negative (leave
/// `**bold**` characters visible in the user's transcript).
///
/// Patterns are compiled once; contains_markdown is on the chat-reply hot
/// path.
fn markdown_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            Regex::new(r"(?m)^#{1,6}[ \t]").expect("static regex"),
            Regex::new(r"(?m)^[ \t]*[-*+][ \t]").expect("static regex"),
            Regex::new(r"(?m)^[ \t]*\d+\.[ \t]").expect("static regex"),
            Regex::new(r"(?m)^>[ \t]").expect("static regex"),
            Regex::new(r"(?m)^[ \t]*(?:---|\*\*\*|___)[ \t]*$").expect("static regex"),
            Regex::new(r"\*\*[^*\n]+\*\*").expect("static regex"),
            Regex::new(r"__[^_\n]+__").expect("static regex"),
            Regex::new(r"(?m)^[ \t]*\|.+\|[ \t]*$").expect("static regex"),
            Regex::new(r"\[[^\]\n]+\]\([^)\n]+\)").expect("static regex"),
        ]
    })
}

/// Returns true when the body almost certainly contains markdown syntax that
/// Lark's plain-text `msg_type=text` renderer would leave un-rendered (showing
/// raw asterisks, hashes, pipes, etc.). On true, the chat-reply router
/// upgrades to the schema-2.0 interactive card path so the user sees formatted
/// text.
///
/// Fast-path tokens (backtick, asterisk, pipe, hash, leading dash on any line)
/// are checked first; only on a hit do we run the slower regex pass. Empty
/// strings short-circuit to false so an empty agent reply does not get wrapped
/// in a card.
pub fn contains_markdown(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Fenced code block — strong markdown signal, cheap substring check.
    if s.contains("```") {
        return true;
    }
    // Inline code: only count `…` runs that look paired and contain a
    // non-space char. Bare backticks (e.g. quoting a single keystroke in
    // prose) shouldn't trigger.
    if let Some(i) = s.find('`') {
        if let Some(j) = s[i + 1..].find('`') {
            if j > 0 {
                return true;
            }
        }
    }
    markdown_patterns().iter().any(|re| re.is_match(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_plain_prose_are_not_markdown() {
        assert!(!contains_markdown(""));
        assert!(!contains_markdown("Hello! 一切正常。"));
        assert!(!contains_markdown("a - b")); // dash not at line start + no space-list shape
    }

    #[test]
    fn fenced_code_blocks_trigger() {
        assert!(contains_markdown("look:\n```\ncode\n```"));
    }

    #[test]
    fn paired_inline_code_triggers_but_bare_backtick_does_not() {
        assert!(contains_markdown("run `cargo check` now"));
        assert!(!contains_markdown("press ` to toggle"));
    }

    #[test]
    fn structural_patterns_trigger() {
        assert!(contains_markdown("# Heading"));
        assert!(contains_markdown("###### H6"));
        assert!(contains_markdown("- item"));
        assert!(contains_markdown("* item"));
        assert!(contains_markdown("1. first"));
        assert!(contains_markdown("> quoted"));
        assert!(contains_markdown("---"));
        assert!(contains_markdown("**bold**"));
        assert!(contains_markdown("__bold__"));
        assert!(contains_markdown("| a | b |\n|---|---|"));
        assert!(contains_markdown("[text](https://x.y)"));
    }

    #[test]
    fn single_asterisk_or_underscore_does_not_trigger() {
        assert!(!contains_markdown("well *maybe* not"));
        assert!(!contains_markdown("snake_case_name"));
    }
}
