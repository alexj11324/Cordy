//! Untrusted-text Markdown link guard.
//!
//! Port of `server/internal/integrations/channel/member_text.go`.

/// Separates the standard inline Markdown `](` link adjacency in
/// untrusted text. Platform-native markup needs its own guard. Text
/// without `](` is returned byte-for-byte unchanged.
pub fn break_markdown_link_adjacency(s: &str) -> String {
    s.replace("](", "] (")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaks_inline_link_adjacency() {
        assert_eq!(
            break_markdown_link_adjacency("see](http://x"),
            "see] (http://x"
        );
        assert_eq!(
            break_markdown_link_adjacency("multiple](a](b"),
            "multiple] (a] (b"
        );
    }

    #[test]
    fn text_without_adjacency_is_byte_identical() {
        for case in ["plain text", "already ] ( spaced", "", "reversed(]"] {
            assert_eq!(break_markdown_link_adjacency(case), case);
        }
        assert_eq!(break_markdown_link_adjacency(""), "");
    }
}
