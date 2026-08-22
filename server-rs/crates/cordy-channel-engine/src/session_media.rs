//! Inline-media body composition helpers from the shared chat-session
//! service.
//!
//! Port of the pure half of
//! `server/internal/integrations/channel/engine/session.go`
//! (composeInlineMediaBody / composeIssueCommandMediaDescription /
//! nthSubstringIndex / inlineAttachmentMarkdown / defaultMediaFilename).
//! The DB transactional half (EnsureSession / AppendUserMessage /
//! BindMediaRefs) lands with the sqlx wiring slice and reuses these.

use cordy_channel::MediaRef;
use uuid::Uuid;

#[cfg(test)]
use cordy_util::channel_media as channelmedia;

use crate::issue_command::issue_command_line_bounds as issue_command_line_bounds_internal;

/// One placeholder → Markdown replacement request.
#[derive(Debug, Clone)]
pub struct InlineMediaReplacement {
    pub placeholder: String,
    pub index: i32,
    pub markdown: String,
}

struct InlineMediaEdit {
    start: usize,
    end: usize,
    text: String,
}

/// Replaces every resolvable placeholder occurrence with its Markdown and
/// reports whether anything changed. Overlapping edits are applied
/// left-to-right; an edit starting before the previous one's end is
/// skipped (Go sort + last-cursor semantics).
pub fn compose_inline_media_body(
    body: &str,
    replacements: &[InlineMediaReplacement],
) -> (String, bool) {
    let mut edits: Vec<InlineMediaEdit> = Vec::new();
    for replacement in replacements {
        if replacement.placeholder.is_empty()
            || replacement.index < 0
            || replacement.markdown.is_empty()
        {
            continue;
        }
        let Some(start) =
            nth_substring_index(body, &replacement.placeholder, replacement.index as usize)
        else {
            continue;
        };
        edits.push(InlineMediaEdit {
            start,
            end: start + replacement.placeholder.len(),
            text: replacement.markdown.clone(),
        });
    }
    if edits.is_empty() {
        return (body.to_string(), false);
    }
    edits.sort_by_key(|e| e.start);
    let mut out = String::with_capacity(body.len());
    let mut last = 0usize;
    for edit in edits {
        if edit.start < last {
            continue;
        }
        out.push_str(&body[last..edit.start]);
        out.push_str(&edit.text);
        last = edit.end;
    }
    out.push_str(&body[last..]);
    (out, true)
}

/// Materializes media in the same positions as the normalized inbound
/// body, then removes the /issue directive line. Only resolved media
/// before the command is retained from the prefix; adapter-added quoted
/// context remains excluded from the issue description contract.
pub fn compose_issue_command_media_description(
    body: &str,
    command_text: &str,
    replacements: &[InlineMediaReplacement],
    fallback: &str,
) -> (String, bool) {
    let Some((command_start, _)) = issue_command_line_bounds_internal(body, command_text) else {
        return (fallback.to_string(), false);
    };

    let mut prefix: Vec<(usize, String)> = Vec::with_capacity(replacements.len());
    for replacement in replacements {
        let start = nth_substring_index(body, &replacement.placeholder, replacement.index as usize);
        if let Some(start) = start {
            if start < command_start {
                prefix.push((start, replacement.markdown.clone()));
            }
        }
    }
    prefix.sort_by_key(|(start, _)| *start);

    let (composed, changed) = compose_inline_media_body(body, replacements);
    if !changed {
        return (fallback.to_string(), false);
    }
    let Some((_, command_end)) = issue_command_line_bounds_internal(&composed, command_text) else {
        return (fallback.to_string(), false);
    };

    let mut parts: Vec<String> = Vec::with_capacity(prefix.len() + 1);
    for (_, markdown) in &prefix {
        let trimmed = markdown.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed.to_string());
        }
    }
    let suffix = composed[command_end.min(composed.len())..].trim();
    if !suffix.is_empty() {
        parts.push(suffix.to_string());
    }
    let description = parts.join("\n\n");
    for replacement in replacements {
        if !replacement.markdown.is_empty() && description.contains(&replacement.markdown) {
            return (description, true);
        }
    }
    // A malformed adapter layout placed every matched marker inside the
    // command line that is removed above. Fall back to append so
    // attachments never become invisible merely to preserve an unusable
    // inline layout.
    (fallback.to_string(), false)
}

/// Finds the zero-based byte offset of occurrence `target` of `marker`,
/// or None. Mirrors Go nthSubstringIndex (target < 0 never reaches here —
/// callers filter).
fn nth_substring_index(body: &str, marker: &str, target: usize) -> Option<usize> {
    if marker.is_empty() {
        // Go strings.Index with an empty needle returns 0 for every
        // occurrence; keep the degenerate case well-defined.
        return if target == 0 { Some(0) } else { None };
    }
    let mut offset = 0usize;
    for index in 0..=target {
        let found = body[offset..].find(marker)? + offset;
        if index == target {
            return Some(found);
        }
        offset = found + marker.len();
    }
    None
}

/// Renders one attachment reference as inline Markdown against its
/// attachment id (chat-message path; issues use channelmedia::block).
pub fn inline_attachment_markdown(r#ref: &MediaRef, id: Uuid) -> String {
    let download_path = format!("/api/attachments/{id}/download");
    if r#ref.r#type.0 == "image" {
        return format!("![]({download_path})");
    }
    let mut label = r#ref
        .filename
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]");
    if label.is_empty() {
        label = "attachment".to_string();
    }
    format!("[{label}]({download_path})")
}

/// The display filename for a media object the platform did not name.
pub fn default_media_filename(kind: &str, id: &str, content_type: &str) -> String {
    let prefix = match kind {
        "image" => "image",
        "video" => "video",
        "audio" => "audio",
        "file" => "file",
        _ => "attachment",
    };
    let ext = match content_type {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "video/mp4" => ".mp4",
        _ => "",
    };
    format!("{prefix}-{id}{ext}")
}

// Issue-side rendering intentionally routes through
// cordy_util::channel_media (single source of truth with Go's
// channelmedia.Block call inside bindMediaRefs); the DB wiring slice
// consumes it directly.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nth_occurrence_finds_indexed_match() {
        let body = "a [IMG] b [IMG] c";
        assert_eq!(nth_substring_index(body, "[IMG]", 0), Some(2));
        assert_eq!(nth_substring_index(body, "[IMG]", 1), Some(10));
        assert_eq!(nth_substring_index(body, "[IMG]", 2), None);
        assert_eq!(nth_substring_index(body, "zz", 0), None);
    }

    #[test]
    fn inline_body_replaces_all_and_reports_change() {
        let reps = vec![
            InlineMediaReplacement {
                placeholder: "[IMG]".to_string(),
                index: 0,
                markdown: "![](u1)".to_string(),
            },
            InlineMediaReplacement {
                placeholder: "[IMG]".to_string(),
                index: 1,
                markdown: "![](u2)".to_string(),
            },
        ];
        let (out, changed) = compose_inline_media_body("x [IMG] y [IMG]", &reps);
        assert!(changed);
        assert_eq!(out, "x ![](u1) y ![](u2)");
    }

    #[test]
    fn inline_body_unresolvable_is_noop() {
        let reps = vec![InlineMediaReplacement {
            placeholder: "[MISSING]".to_string(),
            index: 0,
            markdown: "![](u1)".to_string(),
        }];
        let (out, changed) = compose_inline_media_body("plain", &reps);
        assert!(!changed);
        assert_eq!(out, "plain");
    }

    #[test]
    fn overlapping_edits_apply_left_to_right() {
        // Second edit starts inside the first replacement's span → skipped.
        let reps = vec![
            InlineMediaReplacement {
                placeholder: "ab".to_string(),
                index: 0,
                markdown: "XY".to_string(),
            },
            InlineMediaReplacement {
                placeholder: "bc".to_string(),
                index: 0,
                markdown: "Z".to_string(),
            },
        ];
        let (out, changed) = compose_inline_media_body("abc", &reps);
        assert!(changed);
        assert_eq!(out, "XYc");
    }

    #[test]
    fn issue_description_keeps_prefix_media_drops_directive() {
        let body = "[IMG]\n/issue Fix it\nsteps here";
        let reps = vec![InlineMediaReplacement {
            placeholder: "[IMG]".to_string(),
            index: 0,
            markdown: "![](u1)".to_string(),
        }];
        let (desc, changed) =
            compose_issue_command_media_description(body, "/issue Fix it\nsteps", &reps, "fb");
        assert!(changed);
        assert_eq!(desc, "![](u1)\n\nsteps here");
    }

    #[test]
    fn issue_description_falls_back_when_no_directive() {
        let reps = vec![InlineMediaReplacement {
            placeholder: "[IMG]".to_string(),
            index: 0,
            markdown: "![](u1)".to_string(),
        }];
        let (desc, changed) =
            compose_issue_command_media_description("[IMG] no command", "", &reps, "fb");
        assert!(!changed);
        assert_eq!(desc, "fb");
    }

    #[test]
    fn default_filename_table() {
        assert_eq!(
            default_media_filename("image", "ID", "image/png"),
            "image-ID.png"
        );
        assert_eq!(
            default_media_filename("video", "ID", "video/mp4"),
            "video-ID.mp4"
        );
        assert_eq!(
            default_media_filename("file", "ID", "application/zip"),
            "file-ID"
        );
        assert_eq!(default_media_filename("poll", "ID", ""), "attachment-ID");
    }

    #[test]
    fn inline_markdown_escapes_label_and_detects_image() {
        use cordy_channel::{MediaRef, MsgType};
        let img = MediaRef {
            r#type: MsgType::image(),
            filename: "p.png".to_string(),
            ..Default::default()
        };
        let id = Uuid::nil();
        assert_eq!(
            inline_attachment_markdown(&img, id),
            format!("![](/api/attachments/{id}/download)")
        );
        let file = MediaRef {
            r#type: MsgType::file(),
            filename: "we[ird]".to_string(),
            ..Default::default()
        };
        assert_eq!(
            inline_attachment_markdown(&file, id),
            format!("[we\\[ird\\]](/api/attachments/{id}/download)")
        );
        let unnamed = MediaRef {
            r#type: MsgType::file(),
            ..Default::default()
        };
        assert_eq!(
            inline_attachment_markdown(&unnamed, id),
            format!("[attachment](/api/attachments/{id}/download)")
        );
    }

    #[test]
    fn block_path_documented() {
        // Issue-side rendering intentionally routes through
        // cordy_util::channel_media::block (same as Go channelmedia.Block).
        let b = channelmedia::block("0198c0de-0000-7000-8000-000000000001", "f.png", true);
        assert!(b.starts_with("![](/api/attachments/0198c0de"));
    }
}
