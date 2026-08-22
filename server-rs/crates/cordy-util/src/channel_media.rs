//! Channel-media provenance markers in durable Markdown.
//!
//! Port of `server/internal/channelmedia/markdown.go`. Channel ingestion
//! materializes attachments asynchronously; these helpers let issue
//! updates distinguish a late channel-media write from an intentional
//! edit.

use regex::Regex;
use uuid::Uuid;

const MARKER_PREFIX: &str = "<!-- cordy:channel-media:";

fn marker_pattern() -> &'static Regex {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| {
        // Go: `<!-- cordy:channel-media:([0-9a-fA-F-]{36}) -->`
        Regex::new(r"<!-- cordy:channel-media:([0-9a-fA-F-]{36}) -->").unwrap()
    })
}

/// The durable, authorization-aware attachment URL persisted in issue and
/// chat Markdown.
pub fn download_path(id: &str) -> String {
    format!("/api/attachments/{id}/download")
}

/// Records that a Markdown attachment was materialized asynchronously by
/// channel ingestion. Markdown renderers ignore the comment, while issue
/// updates use it to distinguish a late channel-media write from an
/// intentional edit.
pub fn marker(id: &str) -> String {
    format!("{MARKER_PREFIX}{id} -->")
}

/// Renders one channel attachment plus its durable provenance marker.
pub fn block(id: &str, filename: &str, image: bool) -> String {
    let download_path = download_path(id);
    let markdown = if image {
        format!("![]({download_path})")
    } else {
        // Escape order matches the Go strings.NewReplacer: backslashes
        // first, then brackets, then CR/LF flattened to spaces. A single
        // pass over non-overlapping pairs equals sequential Replace calls
        // here because every replacement's output is consumed before the
        // next pattern is applied (no replacement emits another trigger).
        let mut label = filename
            .replace('\\', "\\\\")
            .replace('[', "\\[")
            .replace(']', "\\]")
            .replace('\r', " ")
            // Same replacement target, so the char-class form is the
            // identical single pass (clippy collapsible_str_replace).
            .replace(['\r', '\n'], " ");
        if label.is_empty() {
            label = "attachment".to_string();
        }
        format!("[{label}]({download_path})")
    };
    format!("{markdown}\n\n{}", marker(id))
}

/// Returns valid channel-media attachment ids in document order,
/// de-duplicated. Invalid marker-shaped comments are ignored.
pub fn marked_ids(markdown: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for caps in marker_pattern().captures_iter(markdown) {
        let raw = &caps[1];
        let Ok(parsed) = Uuid::parse_str(raw) else {
            continue;
        };
        let id = parsed.to_string();
        if seen.insert(id.clone()) {
            ids.push(id);
        }
    }
    ids
}

/// Reports whether markdown already carries the provenance marker for
/// `id`. It deliberately does not treat a bare attachment URL as
/// provenance.
pub fn has_marker(markdown: &str, id: &str) -> bool {
    markdown.contains(&marker(id))
}

/// Appends a Markdown block using the same spacing contract as issue
/// media materialization.
pub fn append(markdown: &str, block: &str) -> String {
    if markdown.is_empty() {
        return block.to_string();
    }
    format!("{markdown}\n\n{block}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0198c0de-0000-7000-8000-000000000001";

    #[test]
    fn download_path_shape() {
        assert_eq!(download_path(ID), format!("/api/attachments/{ID}/download"));
    }

    #[test]
    fn marker_wraps_id_in_html_comment() {
        assert_eq!(marker(ID), format!("<!-- cordy:channel-media:{ID} -->"));
    }

    #[test]
    fn block_image_uses_embed_syntax() {
        let b = block(ID, "photo.png", true);
        assert_eq!(
            b,
            format!("![](/api/attachments/{ID}/download)\n\n<!-- cordy:channel-media:{ID} -->")
        );
    }

    #[test]
    fn block_file_escapes_and_labels() {
        // \r and \n each map to their own space (two spaces total),
        // matching the Go strings.NewReplacer pairwise semantics.
        let b = block(ID, "weird [name]\\v1\r\nx", false);
        assert!(b.starts_with("[weird \\[name\\]\\\\v1  x](/api/attachments/"));
        // Empty filename falls back to the fixed label.
        let empty = block(ID, "", false);
        assert!(empty.starts_with("[attachment](/api/attachments/"));
    }

    #[test]
    fn marked_ids_dedupe_and_ignore_invalid() {
        let md = format!(
            "a\n{m1}\n\nb\n{m1}\n\nnot-a-uuid <!-- cordy:channel-media:zz -->\n{m2}",
            m1 = marker(ID),
            m2 = marker("0198C0DE-0000-7000-8000-000000000002") // uppercase parses
        );
        let ids = marked_ids(&md);
        assert_eq!(ids, vec![ID, "0198c0de-0000-7000-8000-000000000002"]);
    }

    #[test]
    fn has_marker_requires_exact_marker_not_bare_url() {
        let md = format!("see /api/attachments/{ID}/download and {}", marker(ID));
        assert!(has_marker(&md, ID));
        assert!(!has_marker("bare /api/attachments/x/download", ID));
    }

    #[test]
    fn append_spacing_contract() {
        assert_eq!(append("", "B"), "B");
        assert_eq!(append("A", "B"), "A\n\nB");
    }
}
