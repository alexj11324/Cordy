//! Content flattening.
//!
//! Renders a Lark message's body.content — the raw, JSON-encoded string Lark
//! double-encodes — into plain text, dispatching on msg_type. It is the
//! shared structural step used by BOTH ingress paths:
//!
//! - the inbound decoder, for the user's own text / post message, and
//! - the enricher, for the quoted-reply parent and merge_forward child
//!   messages it pulls back over the IM REST API.
//!
//! Mention placeholders (@_user_N) are preserved verbatim; the caller is
//! responsible for resolving them against the message's mentions[] array via
//! [`crate::frame_decoder::resolve_mentions`]. The two ingress shapes (WS
//! receive event vs IM REST item) carry the mentions array differently — only
//! the caller knows which one applies — so flattening stays mention-agnostic.
//!
//! Non-text media types render as a stable bracketed placeholder so the agent
//! sees that *something* was attached without this fast path downloading the
//! binary; the detached media resolver separately fetches the resource and
//! binds it as a chat attachment, with the placeholder as the durable
//! fallback. merge_forward is intercepted by the enricher before it reaches
//! here (expanding it needs an HTTP round-trip); the inline placeholder is
//! only a fallback for a forward nested inside another forward.

use serde::Deserialize;

/// The msg_type of a "merged & forwarded" message — a bundle of other
/// messages a user forwarded as one unit. Its own body.content is a fixed
/// sentinel string; the actual forwarded messages come back as the extra
/// items[] of a GetMessage call.
pub const LARK_MSG_TYPE_MERGE_FORWARD: &str = "merge_forward";

pub fn flatten_content(msg_type: &str, raw_content: &str) -> String {
    match msg_type {
        "text" => extract_text_body(raw_content),
        "post" => flatten_post_content(raw_content),
        "image" => "[Image]".to_string(),
        "file" => "[File]".to_string(),
        "audio" => "[Audio]".to_string(),
        "media" | "video" => "[Video]".to_string(),
        "sticker" => "[Sticker]".to_string(),
        "interactive" => "[interactive card]".to_string(),
        "share_chat" => "[Shared Chat]".to_string(),
        "share_user" => "[Shared User Card]".to_string(),
        "system" => "[System Message]".to_string(),
        "merge_forward" => "[forwarded messages]".to_string(),
        _ => String::new(),
    }
}

/// Mirrors the RECEIVE-side shape of a `post` rich-text body.content.
/// Crucially this is NOT the locale-wrapped form the SEND API takes
/// ({"zh_cn": {...}}): an inbound post body.content unmarshals directly into
/// {title, content}. content is a 2-D array — the outer array is the ordered
/// list of paragraphs, each inner array the ordered spans of that paragraph;
/// the newline between paragraphs is implicit in the array boundary, not a
/// span.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LarkPostContent {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content: Vec<Vec<LarkPostSpan>>,
}

/// One node inside a post paragraph. Only the fields that carry renderable
/// text are modelled; the tag set is extensible, so the flattener emits
/// `text` for any unrecognized tag and skips it otherwise rather than failing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LarkPostSpan {
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub href: String,
    #[serde(rename = "user_id", default)]
    pub user_id: String,
    #[serde(rename = "user_name", default)]
    pub user_name: String,
    #[serde(rename = "image_key", default)]
    pub image_key: String,
    #[serde(rename = "file_key", default)]
    pub file_key: String,
    #[serde(rename = "file_name", default)]
    pub file_name: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "mime_type", default)]
    pub mime_type: String,
}

/// Flattens a received `post` body.content into plain text: the title (when
/// present) on its own first line, then one line per paragraph. Within a
/// paragraph spans are joined with a single space — this matches Lark's own
/// rendering, where logically separate chunks ("Lark 集成", then a link
/// "PR #3277") read as space-separated words.
///
/// A link span renders as "text (href)" so the URL survives into the agent's
/// context; an `at` span renders as its @_user_N placeholder (or the inline
/// user_name when Lark already resolved it) so a downstream resolve_mentions
/// pass can substitute the display name. Media spans degrade to the same
/// bracketed placeholders flatten_content uses.
pub fn flatten_post_content(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let Ok(doc) = serde_json::from_str::<LarkPostContent>(raw) else {
        return String::new();
    };

    let mut lines: Vec<String> = Vec::new();
    if !doc.title.is_empty() {
        lines.push(doc.title);
    }
    for para in &doc.content {
        lines.push(flatten_post_paragraph(para));
    }
    // TrimRight(b.String(), "\n") in Go drops trailing empty paragraph lines.
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

fn flatten_post_paragraph(spans: &[LarkPostSpan]) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(spans.len());
    for s in spans {
        match s.tag.as_str() {
            "text" | "code_block" => {
                if !s.text.is_empty() {
                    parts.push(s.text.clone());
                }
            }
            "a" => match (!s.text.is_empty(), !s.href.is_empty()) {
                (true, true) => parts.push(format!("{} ({})", s.text, s.href)),
                (true, false) => parts.push(s.text.clone()),
                (false, true) => parts.push(s.href.clone()),
                (false, false) => {}
            },
            "at" => {
                // Prefer an already-resolved display name; otherwise emit the
                // user_id, which on the receive side is the @_user_N
                // placeholder a later resolve_mentions pass maps to a name.
                if !s.user_name.is_empty() {
                    parts.push(format!("@{}", s.user_name));
                } else if !s.user_id.is_empty() {
                    parts.push(s.user_id.clone());
                }
            }
            "img" => parts.push("[Image]".to_string()),
            "media" => parts.push("[Video]".to_string()),
            "emotion" => {
                // emoji_type is an enum key (e.g. "SMILE"), not display text
                // — skip it rather than leak the key.
            }
            "hr" => parts.push("---".to_string()),
            _ => {
                if !s.text.is_empty() {
                    parts.push(s.text.clone());
                }
            }
        }
    }
    parts.join(" ")
}

pub(crate) fn extract_text_body(content: &str) -> String {
    if content.is_empty() {
        return String::new();
    }
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(content) else {
        return String::new();
    };
    doc.get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_extraction_handles_escapes() {
        assert_eq!(extract_text_body(r#"{"text":"hello"}"#), "hello");
        assert_eq!(extract_text_body(r#"{"text":"a\"b\nc"}"#), "a\"b\nc");
        assert_eq!(extract_text_body(""), "");
        assert_eq!(extract_text_body("not json"), "");
        assert_eq!(extract_text_body("{}"), "");
    }

    #[test]
    fn media_types_render_placeholders() {
        assert_eq!(flatten_content("image", "{}"), "[Image]");
        assert_eq!(flatten_content("video", "{}"), "[Video]");
        assert_eq!(flatten_content("media", "{}"), "[Video]");
        assert_eq!(flatten_content("audio", "{}"), "[Audio]");
        assert_eq!(flatten_content("file", "{}"), "[File]");
        assert_eq!(flatten_content("sticker", "{}"), "[Sticker]");
        assert_eq!(flatten_content("interactive", "{}"), "[interactive card]");
        assert_eq!(flatten_content("share_chat", "{}"), "[Shared Chat]");
        assert_eq!(flatten_content("share_user", "{}"), "[Shared User Card]");
        assert_eq!(flatten_content("system", "{}"), "[System Message]");
        assert_eq!(
            flatten_content("merge_forward", "{}"),
            "[forwarded messages]"
        );
        assert_eq!(flatten_content("unknown_type", "{}"), "");
    }

    #[test]
    fn post_flattens_title_paragraphs_links_and_mentions() {
        let raw = serde_json::json!({
            "title": "Weekly Report",
            "content": [
                [
                    {"tag": "text", "text": "Lark 集成"},
                    {"tag": "a", "text": "PR #3277", "href": "https://git/pr/3277"}
                ],
                [
                    {"tag": "at", "user_id": "@_user_1"}
                ],
                [
                    {"tag": "at", "user_name": "Alice"}
                ],
                [{"tag": "hr"}],
                [{"tag": "emotion", "emoji_type": "SMILE"}]
            ]
        })
        .to_string();
        assert_eq!(
            flatten_post_content(&raw),
            "Weekly Report\nLark 集成 PR #3277 (https://git/pr/3277)\n@_user_1\n@Alice\n---"
        );
    }

    #[test]
    fn post_flatten_degrades_on_garbage() {
        assert_eq!(flatten_post_content(""), "");
        assert_eq!(flatten_post_content("not json"), "");
    }
}
