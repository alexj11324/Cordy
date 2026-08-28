//! Shared `/issue` command parsing.
//!

/// The literal command token. We match exactly — `/Issue` or `/ISSUE` do
/// NOT trigger creation. The case sensitivity is product-intentional: it
/// avoids accidentally promoting messages that mention "/issue" inline in
/// a sentence. This is cross-platform product behavior, so it lives in
/// the shared engine rather than in any one adapter.
pub const ISSUE_COMMAND_PREFIX: &str = "/issue";

/// An extracted `/issue` command.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IssueCommand {
    /// Title text after the directive (empty for a bare command).
    pub title: String,
    /// Everything after the first line (empty when none).
    pub description: String,
}

/// Extracts an `/issue` command from a chat-message body. Returns
/// `Some(cmd)` when the message qualifies and the caller should dispatch
/// to IssueService create; `None` otherwise. Recognized shapes:
///
/// - `/issue <title>` → title = "<title>", description = ""
/// - `/issue <title>\n<rest...>` → title = "<title>", description = "<rest>"
/// - `/issue` (alone, no title) → title = "", description = "" (the Router
///   returns a usage result; it never infers a title from history)
///
/// Only the first non-empty line is considered: a body that begins with
/// blank lines and then `/issue ...` still qualifies. A body whose first
/// non-empty line is anything other than the literal prefix is not an
/// issue command, even if `/issue` appears later. `/issue` must be a
/// whole token, not a prefix of one ("/issuetracker" does not match).
pub fn parse_issue_command(body: &str) -> Option<IssueCommand> {
    let lines: Vec<&str> = body.split('\n').collect();

    let first_idx = lines.iter().position(|line| !line.trim().is_empty())?;

    let trimmed = lines[first_idx].trim_start_matches([' ', '\t']);
    let rest = trimmed.strip_prefix(ISSUE_COMMAND_PREFIX)?;
    if !rest.is_empty() {
        let r0 = rest.as_bytes()[0];
        if r0 != b' ' && r0 != b'\t' {
            return None;
        }
    }

    let title = rest.trim();
    let description = if first_idx + 1 < lines.len() {
        lines[first_idx + 1..]
            .join("\n")
            .trim_end_matches([' ', '\t', '\n'])
            .to_string()
    } else {
        String::new()
    };
    Some(IssueCommand {
        title: title.to_string(),
        description,
    })
}

/// Removes the /issue directive line from the full normalized message
/// while preserving the layout that follows it. Unlike CommandText, the
/// full body may still contain adapter-generated inline media
/// placeholders. Keeping those placeholders in the initial issue
/// description gives the detached media binder stable positions to
/// materialize later.
///
/// Content before the directive is deliberately excluded. Some adapters
/// enrich Body with quoted context while keeping CommandText as the
/// user's own command; copying that prefix into the issue would change
/// the existing command contract.
pub fn issue_description_from_command_body(
    body: &str,
    command_text: &str,
    fallback: &str,
) -> String {
    match issue_command_line_bounds(body, command_text) {
        Some((_, end)) => body[end..].trim().to_string(),
        None => fallback.to_string(),
    }
}

struct TextLineBounds {
    start: usize,
    end: usize,
}

/// Locates the actual user-authored /issue directive in the normalized
/// body. Adapters may prepend enriched history containing older /issue
/// lines, so matching the first token-bounded directive is insufficient.
/// CommandText is the un-enriched command source: count identical
/// directive lines there, then select the corresponding first occurrence
/// in the body's user-authored suffix. This also remains stable when the
/// description repeats the exact directive line.
pub(crate) fn issue_command_line_bounds(body: &str, command_text: &str) -> Option<(usize, usize)> {
    let expected = first_issue_command_line(command_text)?;
    let command_occurrences = count_matching_lines(command_text, &expected);
    let candidates = matching_line_bounds(body, &expected);
    // Saturating subtraction mirrors Go's target<0 guard.
    let target = candidates.len().checked_sub(command_occurrences)?;
    candidates.get(target).map(|b| (b.start, b.end))
}

fn first_issue_command_line(command_text: &str) -> Option<String> {
    for line in command_text.split('\n') {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(ISSUE_COMMAND_PREFIX) {
            if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t') {
                return Some(trimmed.to_string());
            }
        }
        if !trimmed.is_empty() {
            return None;
        }
    }
    None
}

fn count_matching_lines(text: &str, expected: &str) -> usize {
    text.split('\n')
        .filter(|line| line.trim() == expected)
        .count()
}

fn matching_line_bounds(body: &str, expected: &str) -> Vec<TextLineBounds> {
    let mut bounds = Vec::new();
    // Byte-offset iteration mirrors the Go scanner: bounds are byte
    // indices into `body`.
    let mut offset = 0;
    loop {
        let line_end = body[offset..].find('\n');
        let (line, next) = match line_end {
            Some(rel) => {
                let abs = offset + rel;
                (&body[offset..abs], abs + 1)
            }
            None => (&body[offset..], body.len()),
        };
        if line.trim() == expected {
            bounds.push(TextLineBounds {
                start: offset,
                end: next,
            });
        }
        if line_end.is_none() || next > body.len() {
            break;
        }
        offset = next;
    }
    bounds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_issue_command_table() {
        let cases: &[(&str, &str, bool, &str, &str)] = &[
            (
                "title only",
                "/issue Fix the login bug",
                true,
                "Fix the login bug",
                "",
            ),
            (
                "title + description",
                "/issue Fix login\nIt 500s on submit\nsince Tuesday",
                true,
                "Fix login",
                "It 500s on submit\nsince Tuesday",
            ),
            ("bare command", "/issue", true, "", ""),
            (
                "bare command with trailing space",
                "/issue   ",
                true,
                "",
                "",
            ),
            ("leading blank lines", "\n\n/issue Title", true, "Title", ""),
            ("tab separator", "/issue\tTabbed", true, "Tabbed", ""),
            ("not a token", "/issuetracker do thing", false, "", ""),
            (
                "prefix mid-sentence",
                "hey /issue not a command",
                false,
                "",
                "",
            ),
            ("case sensitive", "/Issue Title", false, "", ""),
            ("empty", "", false, "", ""),
            ("only whitespace", "   \n  ", false, "", ""),
        ];
        for &(name, body, want_ok, want_title, want_desc) in cases {
            let cmd = parse_issue_command(body);
            assert_eq!(cmd.is_some(), want_ok, "{name}: ok mismatch");
            if let Some(cmd) = cmd {
                assert_eq!(cmd.title, want_title, "{name}: title");
                assert_eq!(cmd.description, want_desc, "{name}: description");
            }
        }
    }

    #[test]
    fn description_from_body_preserves_inline_layout() {
        let got = issue_description_from_command_body(
            "/issue explain below questions\nWhat is this?\n[Image]\nAnd what is this?\n[Image]",
            "/issue explain below questions\nWhat is this?And what is this?",
            "flattened fallback",
        );
        assert_eq!(got, "What is this?\n[Image]\nAnd what is this?\n[Image]");
    }

    #[test]
    fn description_from_body_excludes_enriched_prefix() {
        let got = issue_description_from_command_body(
            "> quoted context\n/issue Real intent\nrepro steps",
            "/issue Real intent\nrepro steps",
            "fallback",
        );
        assert_eq!(got, "repro steps");
    }

    #[test]
    fn description_from_body_ignores_issue_lines_in_prefix() {
        let got = issue_description_from_command_body(
            "<quoted_message>\n/issue Old intent\n</quoted_message>\n/issue Real intent\nrepro steps",
            "/issue Real intent\nrepro steps",
            "fallback",
        );
        assert_eq!(got, "repro steps");
    }

    #[test]
    fn description_from_body_handles_repeated_directive_line() {
        let got = issue_description_from_command_body(
            "<quoted_message>\n/issue Same\n</quoted_message>\n/issue Same\nDetails\n/issue Same",
            "/issue Same\nDetails\n/issue Same",
            "fallback",
        );
        assert_eq!(got, "Details\n/issue Same");
    }

    #[test]
    fn description_from_body_falls_back_without_directive() {
        let got = issue_description_from_command_body("rewritten body", "/issue Missing", "parsed");
        assert_eq!(got, "parsed");
    }
}
