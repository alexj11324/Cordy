//! Shared `/new` fresh-session command parsing.
//!
//! Port of `server/internal/integrations/channel/engine/fresh_command.go`.

const FRESH_SESSION_COMMAND_PREFIX: &str = "/new";

/// Extracts a first-line /new command from a channel message. Returns the
/// user prompt with the directive removed. The command is shared product
/// behavior: every channel that reaches the Router gets the same
/// fresh-session affordance without reimplementing parsing in its
/// adapter.
///
/// Matching follows the /issue command rules: case-sensitive,
/// token-bounded, and only the first non-empty line can be a command.
/// That means /new and /issue are mutually exclusive on the same first
/// line.
pub fn parse_fresh_session_command(body: &str) -> Option<String> {
    let lines: Vec<&str> = body.split('\n').collect();

    let first_idx = lines.iter().position(|line| !line.trim().is_empty())?;

    let trimmed = lines[first_idx].trim_start_matches([' ', '\t']);
    let rest = trimmed.strip_prefix(FRESH_SESSION_COMMAND_PREFIX)?;
    if !rest.is_empty() {
        let r0 = rest.as_bytes()[0];
        if r0 != b' ' && r0 != b'\t' {
            return None;
        }
    }

    let mut body_parts: Vec<String> = Vec::with_capacity(2);
    let first_line_body = rest.trim();
    if !first_line_body.is_empty() {
        body_parts.push(first_line_body.to_string());
    }
    if first_idx + 1 < lines.len() {
        body_parts.push(lines[first_idx + 1..].join("\n"));
    }
    Some(
        body_parts
            .join("\n")
            .trim_end_matches([' ', '\t', '\n'])
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fresh_session_command_table() {
        let cases: &[(&str, &str, bool, &str)] = &[
            (
                "new with same-line body",
                "/new start from scratch",
                true,
                "start from scratch",
            ),
            (
                "leading blank lines tolerated",
                "\n\n/new re-check the deploy",
                true,
                "re-check the deploy",
            ),
            (
                "multi-line body preserved",
                "/new title\nline one\nline two",
                true,
                "title\nline one\nline two",
            ),
            ("command alone produces empty body", "/new", true, ""),
            (
                "prefix of token rejected",
                "/newness is not a command",
                false,
                "",
            ),
            (
                "mid-sentence command rejected",
                "please /new this run",
                false,
                "",
            ),
            ("wrong case rejected", "/New help", false, ""),
            ("normal body rejected", "help me normally", false, ""),
        ];
        for &(name, body, want_match, want_body) in cases {
            match parse_fresh_session_command(body) {
                Some(got) => {
                    assert!(want_match, "{name}: unexpected match");
                    assert_eq!(got, want_body, "{name}: body mismatch");
                }
                None => assert!(!want_match, "{name}: expected match"),
            }
        }
    }

    #[test]
    fn fresh_and_issue_are_mutually_exclusive_on_first_line() {
        // A /issue first line is NOT a /new command (and vice versa).
        assert!(parse_fresh_session_command("/issue Fix it").is_none());
        assert!(super::super::issue_command::parse_issue_command("/new start").is_none());
    }
}
