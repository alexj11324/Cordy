//! Best-effort semantic titles for the opening human turn of a chat.

use std::time::Duration;

use cordy_db::models::ChatSession;
use cordy_db::queries::chat;
use futures_util::FutureExt;
use serde_json::json;
use uuid::Uuid;

use crate::state::HandlerState;

const TITLE_GENERATION_TIMEOUT: Duration = Duration::from_secs(20);
const TITLE_MAX_CHARS: usize = 200;

const SYSTEM_PROMPT: &str = r#"You write a very short title that summarizes the topic of a chat conversation, given the user's opening message.

Rules:
- Output ONLY the title text — nothing else, no explanation.
- Keep it short: a few words, ideally under 8, never a full sentence.
- Write the title in the SAME language as the user's message, and in no other.
- Do NOT wrap the title in quotes or brackets.
- Do NOT prefix it with a label such as "Title:", in any language.
- Do NOT end with a period or any trailing punctuation."#;

/// Go treats inspection errors as "not first" so title generation can never
/// turn a successful chat send into an error or duplicate external request.
pub(crate) fn should_generate_for_first_message(
    has_user_message: &anyhow::Result<Option<bool>>,
) -> bool {
    matches!(has_user_message, Ok(Some(false)) | Ok(None))
}

/// Starts the title worker and returns immediately. The supervising task owns
/// the timeout and aborts a hung request; a panic stays inside the inner Tokio
/// task and is logged without affecting the server or the successful send.
pub(crate) fn generate_title_async(
    state: HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    current_title: String,
    source_text: String,
) {
    if !state.llm.enabled() || source_text.trim().is_empty() {
        return;
    }

    let side_effects = state.tasks.clone();
    side_effects.spawn_side_effect(async move {
        let worker = std::panic::AssertUnwindSafe(generate_and_apply(
            state,
            workspace_id,
            user_id,
            session_id,
            current_title,
            source_text,
        ))
        .catch_unwind();
        match tokio::time::timeout(TITLE_GENERATION_TIMEOUT, worker).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => {
                tracing::warn!(%error, %session_id, "chat title generation failed; keeping original title");
            }
            Ok(Err(_)) => {
                tracing::error!(%session_id, "chat title generation task panicked; keeping original title");
            }
            Err(_) => {
                tracing::warn!(%session_id, "chat title generation timed out; keeping original title");
            }
        }
    });
}

async fn generate_and_apply(
    state: HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    current_title: String,
    source_text: String,
) -> anyhow::Result<()> {
    let raw = state
        .llm
        .generate_text("", SYSTEM_PROMPT, &source_text)
        .await?;
    let title = sanitize_chat_title(&raw);
    if title.is_empty() {
        return Ok(());
    }

    // Compare-and-swap is the no-clobber boundary: a manual rename (or a
    // competing automatic writer) changes the expected title and wins.
    let updated =
        chat::update_chat_session_title_if_current(&state.pool, &title, session_id, &current_title)
            .await?;
    let Some(updated) = updated else {
        return Ok(());
    };
    publish_title_update(&state, workspace_id, user_id, &updated);
    Ok(())
}

fn publish_title_update(
    state: &HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    session: &ChatSession,
) {
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::events::EVENT_CHAT_SESSION_UPDATED.to_owned(),
        workspace_id: workspace_id.to_string(),
        actor_type: "member".to_owned(),
        actor_id: user_id.to_string(),
        payload: json!({
            "chat_session_id": session.id,
            "title": session.title,
            "updated_at": crate::timefmt::rfc3339(session.updated_at),
        }),
        chat_session_id: session.id.to_string(),
        ..Default::default()
    });
}

pub(crate) fn sanitize_chat_title(raw: &str) -> String {
    let mut title = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return title;
    }

    loop {
        let before = title.clone();
        title = strip_label_prefix(&title).trim().to_owned();
        title = strip_surrounding_wrapper(&title);
        title = title
            .trim_end_matches([
                '.', '。', '!', '！', '?', '？', ',', '，', ';', '；', ':', '：', '、', ' ',
            ])
            .trim()
            .to_owned();
        if title.is_empty() || title == before {
            break;
        }
    }

    title
        .chars()
        .take(TITLE_MAX_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn strip_label_prefix(value: &str) -> &str {
    const PREFIXES: &[&str] = &[
        "title:",
        "title：",
        "标题:",
        "标题：",
        "题目:",
        "题目：",
        "主题:",
        "主题：",
    ];
    for prefix in PREFIXES {
        let Some(head) = value.get(..prefix.len()) else {
            continue;
        };
        if head.eq_ignore_ascii_case(prefix) {
            return value.get(prefix.len()..).unwrap_or_default();
        }
    }
    value
}

fn strip_surrounding_wrapper(value: &str) -> String {
    let mut output = value.trim().to_owned();
    loop {
        let mut chars = output.chars();
        let Some(first) = chars.next() else {
            return output;
        };
        let Some(last) = output.chars().next_back() else {
            return output;
        };
        let matching = match first {
            '"' => '"',
            '\'' => '\'',
            '`' => '`',
            '“' => '”',
            '‘' => '’',
            '「' => '」',
            '『' => '』',
            '《' => '》',
            '（' => '）',
            '(' => ')',
            '【' => '】',
            '[' => ']',
            _ => return output,
        };
        if last != matching || output.chars().count() < 2 {
            return output;
        }
        let start = first.len_utf8();
        let end = output.len() - last.len_utf8();
        output = output[start..end].trim().to_owned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_message_trigger_fails_closed_on_inspection_error() {
        assert!(should_generate_for_first_message(&Ok(Some(false))));
        assert!(should_generate_for_first_message(&Ok(None)));
        assert!(!should_generate_for_first_message(&Ok(Some(true))));
        assert!(!should_generate_for_first_message(&Err(anyhow::anyhow!(
            "db unavailable"
        ))));
    }

    #[test]
    fn sanitizes_nested_model_formatting_and_unicode_length() {
        let cases = [
            ("Fix login bug", "Fix login bug"),
            ("\"Title: Fix login\".", "Fix login"),
            ("「标题：修复登录问题」。", "修复登录问题"),
            ("Fix\nlogin\tbug!", "Fix login bug"),
            ("\"。\"", ""),
        ];
        for (input, expected) in cases {
            assert_eq!(sanitize_chat_title(input), expected);
        }
        let long = "界".repeat(TITLE_MAX_CHARS + 20);
        assert_eq!(sanitize_chat_title(&long).chars().count(), TITLE_MAX_CHARS);
    }

    #[test]
    fn title_cas_query_requires_the_observed_title() {
        // This pins the generated DB API's contract used above: both the id
        // and observed title are mandatory, so a manual rename yields None.
        let source = include_str!("../../cordy-db/src/queries/chat.rs");
        let start = source
            .find("pub async fn update_chat_session_title_if_current")
            .unwrap_or_default();
        let query = &source[start..source.len().min(start + 900)];
        assert!(query.contains("WHERE id = $2 AND title = $3"));
        assert!(query.contains("expected_title"));
    }
}
