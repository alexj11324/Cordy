//! Markdown mention parsing — port of `server/internal/util/mention.go`.

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;

/// A parsed `mention://` link from markdown content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    /// `member`, `agent`, `squad`, `issue`, or `all`.
    pub user_type: String,
    /// The referenced UUID, or `all` for an all-members mention.
    pub user_id: String,
}

impl Mention {
    pub fn is_all(&self) -> bool {
        self.user_type == "all"
    }
}

fn mention_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\[@?(.+?)\]\(mention://(member|agent|squad|issue|all)/([0-9a-fA-F-]+|all)\)")
            .expect("mention regex is valid")
    })
}

/// Extracts mentions in first-seen order, deduplicated by type and id.
pub fn parse_mentions(content: &str) -> Vec<Mention> {
    let mut seen = HashSet::new();
    mention_regex()
        .captures_iter(content)
        .filter_map(|capture| {
            let user_type = capture.get(2)?.as_str().to_string();
            let user_id = capture.get(3)?.as_str().to_string();
            seen.insert((user_type.clone(), user_id.clone()))
                .then_some(Mention { user_type, user_id })
        })
        .collect()
}

/// Returns whether any parsed mention targets all members.
pub fn has_mention_all(mentions: &[Mention]) -> bool {
    mentions.iter().any(Mention::is_all)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mentions_and_preserves_first_seen_order() {
        assert_eq!(
            parse_mentions(
                "[@A[1]](mention://agent/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa) \
                 [MUL-1](mention://issue/bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb) \
                 [@A again](mention://agent/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa)"
            ),
            vec![
                Mention {
                    user_type: "agent".into(),
                    user_id: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".into(),
                },
                Mention {
                    user_type: "issue".into(),
                    user_id: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".into(),
                },
            ]
        );
    }

    #[test]
    fn supports_all_and_optional_at_prefix() {
        let mentions = parse_mentions(
            "[Bob](mention://member/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa) \
             [@All](mention://all/all)",
        );
        assert_eq!(mentions.len(), 2);
        assert!(has_mention_all(&mentions));
        assert!(mentions[1].is_all());
    }

    #[test]
    fn invalid_links_are_ignored() {
        assert!(
            parse_mentions("plain mention://agent/not-a-uuid [x](mention://unknown/id)").is_empty()
        );
        assert!(!has_mention_all(&[]));
    }
}
