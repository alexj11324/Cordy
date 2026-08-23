//! Port of `markdown.go`: sampleMarkdown chunking for DingTalk's robot send
//! APIs.
//!
//! DingTalk's robot send APIs (oToMessages.batchSend / groupMessages.send)
//! hard-reject a sampleMarkdown body over ~20000 bytes and drop the whole
//! message, so we chunk well under that. The budget is measured in UTF-8 BYTES,
//! not chars, because the limit is on the encoded payload. A code fence open at
//! a chunk boundary is closed at the end of the chunk and reopened at the start
//! of the next, so neither half renders as broken markdown.

/// Bounds one chunk's body in UTF-8 bytes.
pub const MARKDOWN_BYTE_BUDGET: usize = 16000;

/// A split inside a code block appends "\n```" to make the emitted chunk
/// self-contained. Kept out of the content budget so the final wire body, not
/// just the pre-rendered slice, stays under the hard limit.
const MARKDOWN_SYNTHETIC_FENCE_CLOSE_BYTES: usize = "```".len() + 1; // "\n```"

/// Content budget per chunk before synthetic fence bytes.
const MARKDOWN_CONTENT_BYTE_BUDGET: usize =
    MARKDOWN_BYTE_BUDGET - MARKDOWN_SYNTHETIC_FENCE_CLOSE_BYTES;

/// A continuation repeats the opening fence line. Bound that synthetic prefix
/// so an adversarially long info string cannot consume the entire piece budget
/// (or make it negative) when the next code line is split.
const MAX_MARKDOWN_FENCE_INFO_BYTES: usize = 256;

/// The chat-list notification preview used when the body carries no leading
/// heading. DingTalk shows the title only in the push preview, not in the
/// message body.
pub const DEFAULT_MARKDOWN_TITLE: &str = "Cordy has replied.";

/// Derives the sampleMarkdown title (the notification preview) from the body's
/// first ATX heading, falling back to a default. The heading is left in the
/// body; only its leading hashes are stripped for the preview.
pub fn markdown_title(body: &str) -> String {
    for line in body.split('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let heading = trimmed.trim_start_matches('#').trim();
            if !heading.is_empty() {
                return heading.to_string();
            }
        }
        if !trimmed.is_empty() {
            break;
        }
    }
    DEFAULT_MARKDOWN_TITLE.to_string()
}

/// Splits body into pieces each at most [`MARKDOWN_BYTE_BUDGET`] bytes,
/// preferring line boundaries. A code fence (` ``` `) left open at a boundary
/// is closed at the end of the chunk and reopened at the start of the next so
/// each chunk is self-contained markdown. A single line longer than the budget
/// is hard-split on a byte boundary that respects UTF-8 char edges.
pub fn chunk_markdown(body: &str) -> Vec<String> {
    if body.len() <= MARKDOWN_BYTE_BUDGET {
        return vec![body.to_string()];
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut fence_open = false;
    // fenceInfo is the opening fence line (e.g. "```go") of the currently open
    // block, so a continuation chunk can reopen with the SAME info string.
    // DingTalk highlights by that language tag; a bare "```" reopen renders the
    // continuation unhighlighted.
    let mut fence_info = String::new();

    // Drop a chunk that carries only fence/blank lines — e.g. an opening fence
    // stranded right before an oversized line, or a reopened fence with nothing
    // after it — so it never renders as an empty code block.
    fn flush(
        cur: &mut String,
        chunks: &mut Vec<String>,
        fence_open: bool,
        reopen: bool,
        fence_info: &str,
    ) {
        if cur.is_empty() {
            return;
        }
        let mut text = std::mem::take(cur);
        if fence_open {
            text.push_str("\n```");
        }
        if !is_blank_chunk(&text) {
            chunks.push(text);
        }
        if reopen && fence_open {
            cur.push_str(fence_info);
            cur.push('\n');
        }
    }

    for line in split_keep_newline(body) {
        // A single oversized line cannot fit a chunk; hard-split it.
        if line.len() > MARKDOWN_CONTENT_BYTE_BUDGET {
            flush(&mut cur, &mut chunks, fence_open, true, &fence_info);
            let mut piece_budget = MARKDOWN_CONTENT_BYTE_BUDGET;
            if fence_open {
                piece_budget = MARKDOWN_BYTE_BUDGET
                    - fence_info.len()
                    - 1
                    - MARKDOWN_SYNTHETIC_FENCE_CLOSE_BYTES;
            }
            for piece in hard_split(line, piece_budget) {
                // A piece split out of an oversized line inside a code block must
                // carry its own fences, or it would render as plain text.
                if fence_open {
                    chunks.push(format!("{fence_info}\n{piece}\n```"));
                } else {
                    chunks.push(piece);
                }
            }
            continue;
        }
        if cur.len() + line.len() > MARKDOWN_CONTENT_BYTE_BUDGET {
            flush(&mut cur, &mut chunks, fence_open, true, &fence_info);
        }
        if is_fence_line(line) {
            if fence_open {
                fence_open = false;
                fence_info.clear();
            } else {
                fence_open = true;
                fence_info = continuation_fence(line);
            }
        }
        cur.push_str(line);
    }
    flush(&mut cur, &mut chunks, fence_open, false, &fence_info);
    chunks
}

fn continuation_fence(line: &str) -> String {
    let fence = line.trim_end_matches(['\r', '\n']);
    if fence.len() > MAX_MARKDOWN_FENCE_INFO_BYTES {
        return "```".to_string();
    }
    fence.to_string()
}

/// Splits s into lines, keeping the trailing "\n" on each line so reassembly
/// (join) is exact. Go's SplitAfter yields a trailing "" when s ends in "\n";
/// drop it so an exact-newline body does not gain a blank line.
fn split_keep_newline(s: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut rest = s;
    while let Some(pos) = rest.find('\n') {
        let (line, next) = rest.split_at(pos + 1);
        lines.push(line);
        rest = next;
    }
    if !rest.is_empty() {
        lines.push(rest);
    }
    lines
}

/// Reports whether a line opens or closes a fenced code block (its first
/// non-space content is ` ``` `).
fn is_fence_line(line: &str) -> bool {
    line.trim_start_matches([' ', '\t']).starts_with("```")
}

/// Reports whether text carries no renderable content — every line is blank or
/// a fence marker. Such a chunk would render as an empty code block, so the
/// chunker drops it instead of sending it.
fn is_blank_chunk(text: &str) -> bool {
    for line in text.split('\n') {
        let t = line.trim();
        if !t.is_empty() && !t.starts_with("```") {
            return false;
        }
    }
    true
}

/// Breaks s into byte-budget pieces without cutting a UTF-8 char.
///
/// All production callers provide a much larger budget. Keep this helper total
/// anyway: a future synthetic-prefix change must not turn a malformed budget
/// into an infinite loop, negative slice, or split UTF-8 char.
fn hard_split(mut s: &str, mut budget: usize) -> Vec<String> {
    const UTF8_MAX: usize = 4;
    if budget < UTF8_MAX {
        budget = UTF8_MAX;
    }
    let mut pieces: Vec<String> = Vec::new();
    while s.len() > budget {
        let mut cut = budget;
        // Walk back to a char boundary. For valid UTF-8 this always terminates
        // within three steps (no rune exceeds four bytes), so cut > 0 holds and
        // slicing below cannot panic; the cut == 0 guard mirrors Go's defensive
        // fallback for hypothetical non-UTF-8 input.
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        if cut == 0 {
            cut = budget;
        }
        pieces.push(s[..cut].to_string());
        s = &s[cut..];
    }
    if !s.is_empty() {
        pieces.push(s.to_string());
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_from_first_heading() {
        assert_eq!(markdown_title("# Fix login\nbody"), "Fix login");
        assert_eq!(markdown_title("##   Spaced  "), "Spaced");
        assert_eq!(markdown_title("###"), DEFAULT_MARKDOWN_TITLE); // hashes only
                                                                   // Blank leading lines do NOT stop the scan; only a non-empty,
                                                                   // non-heading line does (Go breaks on the first such line).
        assert_eq!(markdown_title("\n\n# Late heading"), "Late heading");
        assert_eq!(
            markdown_title("plain first line\n# later"),
            DEFAULT_MARKDOWN_TITLE
        );
        assert_eq!(markdown_title(""), DEFAULT_MARKDOWN_TITLE);
    }

    #[test]
    fn short_body_is_single_chunk() {
        assert_eq!(chunk_markdown("hello"), vec!["hello"]);
    }

    #[test]
    fn splits_on_line_boundaries_under_budget() {
        let line = "x".repeat(1000);
        let mut body = String::new();
        for _ in 0..20 {
            body.push_str(&line);
            body.push('\n');
        }
        assert!(body.len() > MARKDOWN_BYTE_BUDGET);
        let chunks = chunk_markdown(&body);
        assert!(chunks.len() >= 2);
        // Reassembly is exact: every input byte appears once across chunks.
        let joined: String = chunks.join("");
        assert_eq!(joined, body);
    }

    #[test]
    fn open_fence_is_closed_and_reopened() {
        let content_line = "y".repeat(9000);
        let body = format!("```go\n{}\n{}\n```\n", content_line, content_line);
        assert!(body.len() > MARKDOWN_BYTE_BUDGET);
        let chunks = chunk_markdown(&body);
        assert_eq!(chunks.len(), 2);
        // First chunk opens with the fence and closes it.
        assert!(chunks[0].starts_with("```go"));
        assert!(chunks[0].ends_with("\n```"));
        // Continuation reopens with the SAME info string and carries the
        // source closing fence.
        assert!(chunks[1].starts_with("```go\n"));
        assert!(chunks[1].contains("```"));
    }

    #[test]
    fn oversized_line_inside_code_block_carries_own_fences() {
        let huge = "z".repeat(MARKDOWN_CONTENT_BYTE_BUDGET + 10);
        let body = format!("```\n{huge}\n```\n");
        let chunks = chunk_markdown(&body);
        assert!(chunks.len() >= 2);
        for c in &chunks {
            assert!(
                c.starts_with("```") || c.ends_with("```") || c.contains("```"),
                "piece missing fences: {c:?}"
            );
        }
    }

    #[test]
    fn oversized_plain_line_hard_splits_without_fences() {
        let huge = "a".repeat(MARKDOWN_CONTENT_BYTE_BUDGET + 5);
        let chunks = chunk_markdown(&huge);
        assert!(chunks.len() >= 2);
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        assert_eq!(total, huge.len());
    }

    #[test]
    fn hard_split_respects_char_boundaries() {
        // 'é' is two bytes; budget 4 keeps pairs intact.
        let s = "éééé"; // 8 bytes
        let pieces = hard_split(s, 4);
        assert_eq!(pieces, vec!["éé", "éé"]);
        let tiny = hard_split("aébc", 2);
        assert_eq!(tiny.join(""), "aébc");
        for p in &tiny {
            assert!(p.chars().all(|_| true));
        }
    }

    #[test]
    fn blank_only_chunks_are_dropped() {
        // An opening fence stranded right before an oversized line produces no
        // empty code-block chunk.
        let huge = "b".repeat(MARKDOWN_CONTENT_BYTE_BUDGET * 2);
        let body = format!("```\n{huge}");
        let chunks = chunk_markdown(&body);
        for c in &chunks {
            assert!(!is_blank_chunk(c));
        }
    }

    #[test]
    fn all_chunks_within_byte_budget() {
        let mut body = String::new();
        for i in 0..200 {
            body.push_str(&format!("line {i} with some padding text\n"));
        }
        body.push_str(&"w".repeat(40_000));
        for c in chunk_markdown(&body) {
            assert!(
                c.len() <= MARKDOWN_BYTE_BUDGET,
                "chunk too large: {}",
                c.len()
            );
        }
    }
}
