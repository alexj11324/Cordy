//! Outbound sender: chunking, HTML fallback, reply threading.
//!
//! Port of `server/internal/integrations/telegram/sender.go`.

use anyhow::{anyhow, Result};

use crate::api::{ApiError, BotApi, ReplyParameters, SendMessageParams};
use crate::markdown::format_html;
use crate::message_key;

/// Telegram's message-size budget in UTF-16 code units.
pub const MAX_MESSAGE_UNITS: usize = 4096;

/// Delivers an outbound message: HTML-parsed when possible, plain-text
/// fallback on a parse failure, chunked to the platform limit with only
/// the first chunk quoting the reply target.
pub async fn send(
    api: &BotApi,
    out: &cordy_channel::OutboundMessage,
) -> Result<cordy_channel::SendResult> {
    let chat_id: i64 = out
        .chat_id
        .parse()
        .map_err(|e| anyhow!("telegram: bad chat id {:?}: {e}", out.chat_id))?;
    let thread_id: i64 = if out.thread_id.is_empty() {
        0
    } else {
        out.thread_id.parse().unwrap_or(0)
    };
    let mut reply_to: i64 = if out.reply_to.is_empty() {
        0
    } else {
        parse_message_ref(&out.reply_to)
    };

    let mut last_id = String::new();
    for chunk in chunk_message(&out.text, MAX_MESSAGE_UNITS) {
        let reply = if reply_to != 0 {
            Some(ReplyParameters {
                message_id: reply_to,
                allow_sending_without_reply: true,
            })
        } else {
            None
        };
        let html_params = SendMessageParams {
            chat_id,
            text: format_html(&chunk),
            parse_mode: "HTML".into(),
            message_thread_id: thread_id,
            reply_parameters: reply,
        };
        let delivered = match api.send_message_with_retry_after(&html_params).await {
            Ok(m) => m,
            Err(err) => {
                if !is_html_parse_error(&err) {
                    return Err(anyhow!("telegram: sendMessage: {err:#}"));
                }
                // HTML parse failure → resend the raw chunk unparsed.
                let mut plain = SendMessageParams {
                    chat_id,
                    text: chunk.clone(),
                    parse_mode: String::new(),
                    message_thread_id: thread_id,
                    reply_parameters: reply,
                };
                plain.parse_mode = String::new();
                api.send_message_with_retry_after(&plain)
                    .await
                    .map_err(|e| anyhow!("telegram: sendMessage: {e:#}"))?
            }
        };
        last_id = message_key(chat_id, delivered.message_id);
        reply_to = 0; // only the first chunk quotes
    }
    Ok(cordy_channel::SendResult {
        message_id: last_id,
    })
}

/// "chat:message" → message (the numeric half after the colon).
pub fn parse_message_ref(r#ref: &str) -> i64 {
    let after = match r#ref.split_once(':') {
        Some((_, after)) => after,
        None => r#ref,
    };
    after.parse().unwrap_or(0)
}

/// Splits text into chunks each within `max_units` UTF-16 code units,
/// preferring a newline break past the midpoint (Go chunkMessage).
pub fn chunk_message(text: &str, max_units: usize) -> Vec<String> {
    if max_units == 0 || utf16_units(text) <= max_units {
        return vec![text.to_string()];
    }
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < units.len() {
        // Greedy fill up to max_units.
        let mut n = 0usize;
        let mut end = start;
        while end < units.len() {
            let unit = units[end];
            // A high surrogate plus its low surrogate form one rune = 2 units.
            let cost = if (0xD800..0xDC00).contains(&unit) {
                2
            } else {
                1
            };
            if n + cost > max_units {
                break;
            }
            n += cost;
            end += 1;
            // Skip the paired low surrogate so a pair is never split.
            if cost == 2 && end < units.len() {
                end += 1;
            }
        }
        if end == start {
            end = start + 1;
        }
        // Prefer breaking on the last newline when the pre-break head is
        // still more than half the budget (Go: lastIndexRune('\n')).
        let slice = &units[start..end.min(units.len())];
        if let Some(nl_rel) = slice.iter().rposition(|u| *u == '\n' as u16) {
            let head_units = nl_rel;
            if head_units > max_units / 2 {
                end = start + nl_rel + 1;
            }
        }
        let chunk_units = &units[start..end.min(units.len())];
        let chunk = String::from_utf16_lossy(chunk_units);
        chunks.push(chunk.trim_end_matches('\n').to_string());
        start = end;
    }
    chunks
}

/// UTF-16 code-unit length (surrogate pairs count as 2).
pub fn utf16_units(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Reports whether the API error is an HTML parse rejection (400 with a
/// parse-flavored description), the trigger for the plain-text fallback.
pub fn is_html_parse_error(err: &anyhow::Error) -> bool {
    let Some(ae) = err.chain().find_map(|c| c.downcast_ref::<ApiError>()) else {
        return false;
    };
    if ae.code != 400 {
        return false;
    }
    let description = ae.description.to_lowercase();
    description.contains("parse entities")
        || description.contains("unsupported start tag")
        || description.contains("can't find end tag")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_ref_parses_both_shapes() {
        assert_eq!(parse_message_ref("-100:42"), 42);
        assert_eq!(parse_message_ref("42"), 42);
        assert_eq!(parse_message_ref("chat:xx"), 0);
        assert_eq!(parse_message_ref(""), 0);
    }

    #[test]
    fn utf16_units_counts_surrogate_pairs_double() {
        assert_eq!(utf16_units("abc"), 3);
        assert_eq!(utf16_units("🎉"), 2);
        assert_eq!(utf16_units("a🎉b"), 4);
    }

    #[test]
    fn chunk_message_respects_budget_and_prefers_newline() {
        assert_eq!(chunk_message("short", 100), vec!["short".to_string()]);
        assert_eq!(chunk_message("short", 0), vec!["short".to_string()]);

        // Budget split.
        let chunks = chunk_message("abcdefgh", 3);
        assert_eq!(chunks.join(""), "abcdefgh");
        assert!(chunks.iter().all(|c| utf16_units(c) <= 3));

        // Newline preferred when the head half exceeds half the budget.
        // Go: greedy fill reaches "aaaa\nb" (6 units), the last newline
        // leaves a 4-unit head (> 6/2), so it breaks after "aaaa\n"; the
        // remainder then fills greedily in budget-sized chunks (6, 3).
        let text = "aaaa\nbbbbbbbbbb";
        let chunks = chunk_message(text, 6);
        assert_eq!(chunks.concat(), "aaaabbbbbbbbbb");
        assert_eq!(chunks[0], "aaaa");
        assert_eq!(utf16_units(chunks[1].as_str()), 6);
        assert_eq!(utf16_units(chunks.last().unwrap().as_str()), 4);

        // Surrogate pairs never split mid-pair.
        let emoji = "🎉🎉🎉🎉";
        let chunks = chunk_message(emoji, 3);
        assert!(chunks.iter().all(|c| utf16_units(c) <= 3));
        assert_eq!(chunks.concat(), emoji);
    }

    #[test]
    fn html_parse_error_detection() {
        let mk = |code: u16, desc: &str| {
            anyhow::Error::new(ApiError {
                code,
                description: desc.into(),
                retry_after: 0,
            })
        };
        assert!(is_html_parse_error(&mk(
            400,
            "Bad Request: can't parse entities"
        )));
        assert!(is_html_parse_error(&mk(
            400,
            "Bad Request: Unsupported start tag"
        )));
        assert!(!is_html_parse_error(&mk(400, "chat not found")));
        assert!(!is_html_parse_error(&mk(429, "can't parse entities")));
        assert!(!is_html_parse_error(&anyhow!("transport")));
    }
}
