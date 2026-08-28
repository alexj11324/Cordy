//! Chat quick-actions suggestion pass.
//!
//! One bounded LLM pass per completed chat turn renders three follow-up
//! suggestion pills. The whole call is a nicety attached to a reply the user
//! already has, so budgets are tight: a slow pass is worse than no pass
//! (the client holds a skeleton placeholder until it resolves).
//!
//! This module ports the pure core (context selection, previous-label
//! collection, prompt rendering, rune-budget truncation) plus the
//! [`ChatQuickActionsLlm`] seam. The `TaskService` methods
//! (`GenerateChatQuickActionsForTask` / `...Async`) land with task.go.

use std::collections::HashSet;
use std::time::Duration;

use async_trait::async_trait;
use uuid::Uuid;

use patchbay_db::models::ChatMessage;

pub const CHAT_QUICK_ACTIONS_TIMEOUT: Duration = Duration::from_secs(8);
pub const CHAT_QUICK_ACTIONS_TEMPERATURE: f64 = 0.3;

/// Output cap. GenerateJSON disables reasoning for this latency-sensitive
/// utility call, so the whole budget is available for the JSON response; the
/// headroom keeps verbose or multibyte actions from being cut off mid-object.
pub const CHAT_QUICK_ACTIONS_MAX_COMPLETION_TOKENS: i64 = 2048;

/// Suggestions are about where the conversation goes next, so only the tail of
/// the window matters — older turns cost tokens and latency without changing
/// the answer.
pub const CHAT_QUICK_ACTIONS_CONTEXT_MESSAGES: usize = 6;
/// The latest assistant reply gets the largest share: it is what the
/// suggestions must be anchored in.
pub const CHAT_QUICK_ACTIONS_LATEST_BUDGET: usize = 3000;
pub const CHAT_QUICK_ACTIONS_OLDER_BUDGET: usize = 800;
/// Head/tail split applied to an over-long latest reply. The tail carries the
/// conclusion and proposed next steps — exactly the material suggestions are
/// built from — so keeping only the head would strip the most useful part.
pub const CHAT_QUICK_ACTIONS_HEAD_BUDGET: usize = 2000;
pub const CHAT_QUICK_ACTIONS_TAIL_BUDGET: usize = 1000;
/// Cap on how many previously-suggested labels are replayed to the model.
pub const CHAT_QUICK_ACTIONS_PREVIOUS_MAX: usize = 6;

/// Bounds how many suggestion passes may be in flight process-wide. Passes
/// over the ceiling are dropped, not queued: a suggestion that arrives after
/// the client's pending window has expired is worth nothing, so shedding beats
/// backlogging.
pub const CHAT_QUICK_ACTIONS_MAX_CONCURRENT: i64 = 16;

/// Ordinary chat turn; other kinds (no_response placeholder, failure rows)
/// carry no usable conversation text.
pub use patchbay_protocol::CHAT_MESSAGE_KIND_MESSAGE;

/// The entire instruction set for the pass. Stable across calls so upstream
/// prompt caching applies; the per-call conversation goes in the user message.
///
/// The word "JSON" must stay in this text: response_format=json_object is
/// rejected upstream without it.
pub const CHAT_QUICK_ACTIONS_SYSTEM_PROMPT: &str = r#"You generate follow-up suggestions for a chat between a user and an AI agent.
Your output is rendered as three clickable buttons under the agent's latest
reply. Clicking one sends that suggestion's "prompt" to the same agent as the
user's next message.

You write FOR THE USER, not for the agent. Every suggestion must be something
the user would plausibly want to ask or do next — never a task the agent should
perform on its own, never a status report, never a question addressed to the user.

Quality bar:
- Return exactly 3 suggestions.
- Every suggestion must be anchored in something concrete the latest agent reply
  actually mentioned — a file, a name, an option it listed, a caveat it raised,
  a next step it proposed. Never invent a topic the conversation did not touch.
- Never suggest something the agent already did in this turn.
- Never repeat or paraphrase anything under ALREADY SUGGESTED.
- Make the three distinct from each other: different intents, not three
  rewordings of the same request.

Field rules:
- "label": the button text. A short verb phrase — at most 6 words, or at most 12
  characters in a script that does not put spaces between words. No trailing
  punctuation, no quotes, no emoji. It is a button, not a sentence.
- "prompt": the full message sent on the user's behalf. First person, the user's
  own voice, and SELF-CONTAINED — the agent never sees the label, so the prompt
  must carry every detail itself. One or two sentences.
- "primary": true on exactly one suggestion, the single most likely next step.
  false on all others.

Language: the user message contains a LANGUAGE RULE line near the end. It is
authoritative; follow it exactly.

Output JSON only, exactly this shape:
{"actions":[{"label":"...","prompt":"...","primary":true}]}
No prose, no markdown, no code fences."#;

/// Closes the pass's user message and is the whole language policy. It is last
/// on purpose: everything above it may be in a language the pills must NOT be
/// written in, and the rule has to be the final thing read before the task
/// line. Anchors on the MOST RECENT user turn, not the window as a whole.
pub const CHAT_QUICK_ACTIONS_LANGUAGE_RULE: &str = r#"LANGUAGE RULE: Write every "label" and "prompt" in the same language as the most recent [user] message above. Ignore the agent's reply, older messages, the system instructions, and ALREADY SUGGESTED when choosing the language. If there is no [user] message, use the latest [agent] message."#;

/// Server-validated follow-up attached to one assistant reply — the wire type
/// from `patchbay_protocol` (Go `protocol.ChatQuickAction`).
pub use patchbay_protocol::ChatQuickAction;

/// Which caller asked for a pass. Decides how a failure is reported: an
/// explicit refresh surfaces its own failure, while the automatic pass is
/// best-effort background work that must fail silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatQuickActionsOrigin {
    Automatic,
    Refresh,
}

/// Seam the task service uses to generate follow-up suggestions. A missing
/// implementation, or one whose [`ChatQuickActionsLlm::enabled`] is false,
/// disables the feature entirely: no pending marker is raised and no pills are
/// generated (the expected state for a self-hosted deployment with no
/// PATCHBAY_LLM_API_KEY / PATCHBAY_LLM_BASE_URL).
#[async_trait]
pub trait ChatQuickActionsLlm: Send + Sync {
    fn enabled(&self) -> bool;
    async fn generate_json(
        &self,
        model: &str,
        system_prompt: &str,
        user_prompt: &str,
        temperature: f64,
        max_completion_tokens: i64,
    ) -> anyhow::Result<String>;
}

/// Selects the context window ENDING AT `target`, which is the assistant turn
/// the suggestions attach to. Anchoring on the target (rather than re-reading
/// the session's newest messages) keeps an async pass correct: a turn landing
/// between the completion callback and this read would otherwise supply
/// context while the result is still written to the older turn.
///
/// `rows` must be strictly older than target, newest-first (as produced by
/// ListChatMessagesPage). Returns oldest-first with target appended last.
pub fn select_chat_quick_actions_context(
    rows: &[ChatMessage],
    target: &ChatMessage,
    input_owner_id: Option<Uuid>,
) -> Vec<ChatMessage> {
    // A queued successor's input can be stored before target even though it is
    // logically the next turn. Keep user rows only when their task has already
    // answered inside this anchored window (or owns the target task's input;
    // auto-retry replies use the child task ID while their input keeps the root).
    let mut answered_task_ids: HashSet<Option<Uuid>> = HashSet::new();
    answered_task_ids.insert(target.task_id);
    answered_task_ids.insert(input_owner_id);
    for msg in rows {
        if msg.role == "assistant" {
            answered_task_ids.insert(msg.task_id);
        }
    }

    let mut msgs: Vec<ChatMessage> = Vec::with_capacity(rows.len() + 1);
    for msg in rows.iter().rev() {
        if msg.role == "user" {
            match msg.task_id {
                Some(task_id) if !answered_task_ids.contains(&Some(task_id)) => continue,
                _ => {}
            }
        }
        // Only ordinary turns carry usable text: a no_response row holds an
        // English placeholder body and a failure row holds an error.
        if msg.message_kind != CHAT_MESSAGE_KIND_MESSAGE || msg.content.trim().is_empty() {
            continue;
        }
        msgs.push(msg.clone());
    }
    if msgs.len() > CHAT_QUICK_ACTIONS_CONTEXT_MESSAGES - 1 {
        msgs.drain(..msgs.len() - (CHAT_QUICK_ACTIONS_CONTEXT_MESSAGES - 1));
    }
    msgs.push(target.clone());
    msgs
}

/// Gathers the labels already offered in this window so the prompt can forbid
/// repeating them. The newest assistant row is included: on an explicit
/// refresh it holds the pills the user is asking to replace, and offering the
/// same three back is the one outcome a refresh must never produce.
///
/// Newest first, so the most recent suggestions survive the cap. Labels are a
/// de-duplication list ONLY — replayed verbatim even when they drifted to the
/// wrong language (the language rule names them as something to ignore when
/// choosing output language).
pub fn collect_previous_chat_quick_actions(msgs: &[ChatMessage]) -> Vec<String> {
    let mut labels: Vec<String> = Vec::with_capacity(CHAT_QUICK_ACTIONS_PREVIOUS_MAX);
    let mut seen: HashSet<String> = HashSet::with_capacity(CHAT_QUICK_ACTIONS_PREVIOUS_MAX);
    for msg in msgs.iter().rev() {
        let Ok(actions) = serde_json::from_value::<Vec<ChatQuickAction>>(msg.quick_actions.clone())
        else {
            continue;
        };
        for action in actions {
            let label = action.label.trim().to_string();
            if label.is_empty() {
                continue;
            }
            let key = label.to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            labels.push(label);
            if labels.len() == CHAT_QUICK_ACTIONS_PREVIOUS_MAX {
                return labels;
            }
        }
    }
    labels
}

/// Formats the conversation window and the already-suggested labels into the
/// pass's user message. Pure, so the truncation rules are unit-testable
/// without a database. `msgs` is oldest-first and its last entry must be the
/// assistant reply the suggestions are for.
pub fn render_chat_quick_actions_context(msgs: &[ChatMessage], previous: &[String]) -> String {
    let mut b = String::new();
    b.push_str("CONVERSATION (oldest first):\n");
    for (i, msg) in msgs.iter().enumerate() {
        let speaker = if msg.role == "user" { "user" } else { "agent" };
        let content = msg.content.trim();
        let content = if i == msgs.len() - 1 {
            truncate_chat_quick_actions_latest(content)
        } else {
            truncate_chat_quick_actions_runes(content, CHAT_QUICK_ACTIONS_OLDER_BUDGET)
        };
        b.push_str(&format!("[{speaker}]: {content}\n"));
    }

    b.push_str("\nALREADY SUGGESTED (do not repeat or paraphrase):\n");
    if previous.is_empty() {
        b.push_str("(none)\n");
    } else {
        for label in previous {
            b.push_str(&format!("- {label}\n"));
        }
    }

    b.push('\n');
    b.push_str(CHAT_QUICK_ACTIONS_LANGUAGE_RULE);
    b.push_str("\n\nProduce the follow-up suggestions for the latest agent reply.");
    b
}

/// Shortens the anchor reply while keeping both ends. The head establishes
/// what the reply is about and the tail holds its conclusion and proposed next
/// steps; a plain head-only cut on a long reply discards exactly the material
/// the suggestions should be built from.
fn truncate_chat_quick_actions_latest(content: &str) -> String {
    let runes: Vec<char> = content.chars().collect();
    if runes.len() <= CHAT_QUICK_ACTIONS_LATEST_BUDGET {
        return content.to_string();
    }
    let head: String = runes[..CHAT_QUICK_ACTIONS_HEAD_BUDGET].iter().collect();
    let tail: String = runes[runes.len() - CHAT_QUICK_ACTIONS_TAIL_BUDGET..]
        .iter()
        .collect();
    format!("{head}\n…[truncated]…\n{tail}")
}

/// Cuts an older message to a rune budget. Head only: for context turns, what
/// the message opened with is enough to follow the thread.
fn truncate_chat_quick_actions_runes(content: &str, max_runes: usize) -> String {
    if content.chars().count() <= max_runes {
        return content.to_string();
    }
    let head: String = content.chars().take(max_runes).collect();
    format!("{head}…")
}

/// The reserved in-band fence syntax agents used to append suggestions with.
pub const CHAT_QUICK_ACTIONS_FENCE: &str = "```quick-actions\n";

/// Max actions accepted from an agent-supplied candidate list.
const CHAT_QUICK_ACTION_MAX_COUNT: usize = 3;

/// Splits a retired in-band quick-actions footer off a reply. Returns the
/// visible body and (discarded) parsed candidates. A mid-response fence is
/// ordinary user-visible markdown and is left intact; a footer that closes
/// before the end of the reply is also left intact so an unrelated closing
/// fence cannot truncate the message.
///
/// Suggestions are generated server-side now, so this is only a defensive
/// stripper: pre-retirement provider sessions still carry the syntax in their
/// history and agents keep emitting it for a while.
pub fn split_chat_quick_actions(
    output: &str,
) -> (String, Vec<patchbay_protocol::messages::ChatQuickAction>) {
    let normalized = output.replace("\r\n", "\n");
    let trimmed = normalized.trim_end_matches([' ', '\t', '\n']);
    if !trimmed.ends_with("\n```") {
        return (output.to_string(), vec![]);
    }

    let without_close = trimmed.strip_suffix("\n```").expect("checked suffix");
    let marker = format!("\n{CHAT_QUICK_ACTIONS_FENCE}");
    let (visible, raw) = match without_close.rfind(&marker) {
        Some(start) => (
            &without_close[..start],
            &without_close[start + marker.len()..],
        ),
        None => {
            if let Some(rest) = without_close.strip_prefix(CHAT_QUICK_ACTIONS_FENCE) {
                ("", rest)
            } else {
                return (output.to_string(), vec![]);
            }
        }
    };
    // Guard against pairing the final closing fence with a mid-response opener.
    if raw.contains("\n```") {
        return (output.to_string(), vec![]);
    }

    let visible = visible.trim_end_matches([' ', '\t', '\n']);
    let Ok(candidates) =
        serde_json::from_str::<Vec<patchbay_protocol::messages::ChatQuickAction>>(raw)
    else {
        return (visible.to_string(), vec![]);
    };
    // Parsed candidates are deliberately discarded: server-side generation has
    // replaced in-band suggestions.
    (visible.to_string(), sanitize_chat_quick_actions(candidates))
}

/// Enforces the server-side contract on agent-supplied candidates: at most
/// three actions, normalized non-empty labels, case-insensitive label dedup,
/// prompts defaulting to the label, rune-safe truncation, exactly one primary.
pub fn sanitize_chat_quick_actions(
    candidates: Vec<patchbay_protocol::messages::ChatQuickAction>,
) -> Vec<patchbay_protocol::messages::ChatQuickAction> {
    use patchbay_protocol::messages::ChatQuickAction;
    const LABEL_MAX: usize = 80;
    const PROMPT_MAX: usize = 500;
    let mut actions: Vec<ChatQuickAction> = Vec::with_capacity(CHAT_QUICK_ACTION_MAX_COUNT);
    let mut seen: HashSet<String> = HashSet::new();
    let mut primary_seen = false;
    for candidate in candidates {
        if actions.len() == CHAT_QUICK_ACTION_MAX_COUNT {
            break;
        }
        let label = candidate
            .label
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if label.is_empty() {
            continue;
        }
        let label = truncate_chat_quick_actions_runes(&label, LABEL_MAX);
        let key = label.to_lowercase();
        if !seen.insert(key) {
            continue;
        }

        let prompt = candidate.prompt.trim();
        let prompt = if prompt.is_empty() {
            label.clone()
        } else {
            truncate_chat_quick_actions_runes(prompt, PROMPT_MAX)
        };
        let primary = candidate.primary && !primary_seen;
        primary_seen |= primary;
        actions.push(ChatQuickAction {
            label,
            prompt,
            primary,
        });
    }
    if !actions.is_empty() && !actions.iter().any(|a| a.primary) {
        actions[0].primary = true;
    }
    actions
}

/// Parses one suggestion pass's raw model output into sanitized actions.
///
/// Attempt order narrows from strict to desperate: the object the pass was
/// asked for (or a bare array), then the inside of a code fence, then the
/// outermost bracket span. The bracket scan runs last because leading prose
/// may itself contain brackets ("here's [my] take: [...]"), which would
/// misalign the slice if it were tried first. Anything unparseable degrades
/// to no suggestions; this output never reaches the transcript.
pub fn parse_chat_quick_actions_output(
    raw: &str,
) -> Vec<patchbay_protocol::messages::ChatQuickAction> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    for candidate in [raw, inside_code_fence(raw).as_str()] {
        if let Some(actions) = unmarshal_chat_quick_actions(candidate) {
            return actions;
        }
    }
    let start = raw.find('[');
    let end = raw.rfind(']');
    if let (Some(start), Some(end)) = (start, end) {
        if end > start {
            if let Some(actions) = unmarshal_chat_quick_actions(&raw[start..=end]) {
                return actions;
            }
            return Vec::new();
        }
    }
    Vec::new()
}

/// Content of the first fenced code block in `raw` (language tag tolerated),
/// or empty when no complete fence exists.
fn inside_code_fence(raw: &str) -> String {
    let Some(open) = raw.find("```") else {
        return String::new();
    };
    let rest = &raw[open + 3..];
    let Some(nl) = rest.find('\n') else {
        return String::new();
    };
    let rest = &rest[nl + 1..];
    let Some(closing) = rest.find("```") else {
        return String::new();
    };
    rest[..closing].trim().to_string()
}

/// The pass asks for {"actions":[...]} because response_format=json_object
/// requires a top-level object. A bare array is still accepted so a model
/// that drops the wrapper does not cost the user their suggestions.
fn unmarshal_chat_quick_actions(
    raw: &str,
) -> Option<Vec<patchbay_protocol::messages::ChatQuickAction>> {
    use patchbay_protocol::messages::ChatQuickAction;

    if raw.is_empty() {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct Wrapper {
        #[serde(default)]
        actions: Option<Vec<ChatQuickAction>>,
    }
    if let Ok(wrapper) = serde_json::from_str::<Wrapper>(raw) {
        if let Some(actions) = wrapper.actions {
            return Some(sanitize_chat_quick_actions(actions));
        }
    }
    let candidates = serde_json::from_str::<Vec<ChatQuickAction>>(raw).ok()?;
    Some(sanitize_chat_quick_actions(candidates))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            channel_ingested: false,
            channel_media_pending_until: None,
            chat_session_id: Uuid::nil(),
            content: content.to_string(),
            created_at: chrono::Utc::now(),
            elapsed_ms: None,
            failure_reason: None,
            id: Uuid::now_v7(),
            message_kind: CHAT_MESSAGE_KIND_MESSAGE.to_string(),
            quick_actions: serde_json::Value::Null,
            role: role.to_string(),
            task_id: None,
        }
    }

    fn with_task(mut m: ChatMessage, task_id: Option<Uuid>) -> ChatMessage {
        m.task_id = task_id;
        m
    }

    fn with_kind(mut m: ChatMessage, kind: &str) -> ChatMessage {
        m.message_kind = kind.to_string();
        m
    }

    fn with_quick_actions(mut m: ChatMessage, actions: serde_json::Value) -> ChatMessage {
        m.quick_actions = actions;
        m
    }

    #[test]
    fn selection_drops_unanswered_user_rows_and_keeps_window_tail() {
        let t1 = Uuid::now_v7();
        let t2 = Uuid::now_v7();
        let t3 = Uuid::now_v7();

        // Newest-first rows strictly older than target.
        let rows = vec![
            with_task(msg("user", "queued successor input"), Some(t3)), // t3 never answered → dropped
            with_kind(with_task(msg("assistant", ""), Some(t2)), "no_response"), // empty/kind filtered
            with_task(msg("assistant", "second answer"), Some(t2)),
            with_task(msg("user", "second question"), Some(t2)),
            with_task(msg("user", "first question"), Some(t1)),
        ];
        let target = with_task(msg("assistant", "target reply"), Some(t2));

        let out = select_chat_quick_actions_context(&rows, &target, Some(t1));

        // t2 counts as answered: Go seeds answeredTaskIDs from RAW rows
        // (including the kind-filtered no_response row) BEFORE filtering.
        let texts: Vec<&str> = out.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "first question",
                "second question",
                "second answer",
                "target reply"
            ]
        );
        assert_eq!(out.last().unwrap().id, target.id);
    }

    #[test]
    fn selection_caps_to_context_minus_one_then_appends_target() {
        let rows: Vec<ChatMessage> = (0..10)
            .map(|i| {
                msg(
                    if i % 2 == 0 { "user" } else { "assistant" },
                    &format!("m{i}"),
                )
            })
            .collect();
        let target = msg("assistant", "anchor");
        let out = select_chat_quick_actions_context(&rows, &target, None);
        assert_eq!(out.len(), CHAT_QUICK_ACTIONS_CONTEXT_MESSAGES);
        // rows are newest-first per contract: m0 newest. Keep last 5 turns
        // oldest-first → m4..m0.
        assert_eq!(out[0].content, "m4");
        assert_eq!(out.last().unwrap().content, "anchor");
    }

    #[test]
    fn previous_labels_newest_first_dedup_case_insensitive_capped() {
        let mk = |labels: &[&str]| {
            json!(labels
                .iter()
                .map(|l| json!({"label": l, "prompt": "p"}))
                .collect::<Vec<_>>())
        };
        let mut older = msg("assistant", "a");
        older = with_quick_actions(older, mk(&["Fix bug", "Add test", "fix BUG"]));
        let mut newer = msg("assistant", "b");
        newer = with_quick_actions(newer, mk(&["Write docs", "", "  ", "write DOCS"]));

        let out = collect_previous_chat_quick_actions(&[older, newer]);
        // Case-insensitive dedup: "write DOCS" duplicates "Write docs",
        // "fix BUG" duplicates "Fix bug"; blank labels dropped.
        assert_eq!(out, vec!["Write docs", "Fix bug", "Add test"]);
    }

    #[test]
    fn previous_labels_skip_null_and_malformed_rows() {
        let mut bad = msg("assistant", "x");
        bad = with_quick_actions(bad, json!("not-an-array"));
        let ok = msg("assistant", "y");
        let out = collect_previous_chat_quick_actions(&[bad, ok]);
        assert!(out.is_empty());
    }

    #[test]
    fn render_matches_go_layout_exactly() {
        let target = msg("assistant", "final reply");
        let mut earlier = msg("user", "earlier");
        earlier = with_quick_actions(earlier, json!([{"label": "Old", "prompt": "p"}]));
        let msgs = vec![earlier, target];

        let out = render_chat_quick_actions_context(&msgs, &["Old".to_string()]);
        assert_eq!(
            out,
            "CONVERSATION (oldest first):\n\
             [user]: earlier\n\
             [agent]: final reply\n\
             \nALREADY SUGGESTED (do not repeat or paraphrase):\n\
             - Old\n\
             \nLANGUAGE RULE: Write every \"label\" and \"prompt\" in the same language as the most recent [user] message above. Ignore the agent's reply, older messages, the system instructions, and ALREADY SUGGESTED when choosing the language. If there is no [user] message, use the latest [agent] message.\n\
             \nProduce the follow-up suggestions for the latest agent reply."
        );
    }

    #[test]
    fn split_strips_trailing_in_band_footer() {
        let output =
            "Here is my reply.\n\n```quick-actions\n[{\"label\":\"A\",\"prompt\":\"a\"}]\n```";
        let (visible, actions) = split_chat_quick_actions(output);
        assert_eq!(visible, "Here is my reply.");
        assert_eq!(
            actions.len(),
            1,
            "footer parsed but candidates are discarded"
        );
    }

    #[test]
    fn split_leaves_mid_response_fence_intact() {
        // A quick-actions fence that closes before the end of the reply is
        // ordinary visible markdown — the final ``` pairs with an unrelated
        // fence and must not truncate the message.
        let output = "before\n```quick-actions\n[]\n```\nafter\n```";
        let (visible, actions) = split_chat_quick_actions(output);
        assert_eq!(actions.len(), 0);
        assert!(visible.contains("before"));
    }

    #[test]
    fn split_returns_original_when_no_footer() {
        let (visible, actions) = split_chat_quick_actions("plain reply");
        assert_eq!(visible, "plain reply");
        assert_eq!(actions.len(), 0);
    }

    #[test]
    fn sanitize_dedups_caps_and_forces_single_primary() {
        use patchbay_protocol::messages::ChatQuickAction;
        let candidates = vec![
            ChatQuickAction {
                label: "First".into(),
                prompt: "p1".into(),
                primary: false,
            },
            ChatQuickAction {
                label: "first".into(),
                prompt: "".into(),
                primary: true,
            },
            ChatQuickAction {
                label: "  ".into(),
                prompt: "x".into(),
                primary: false,
            },
            ChatQuickAction {
                label: "Third".into(),
                prompt: "p3".into(),
                primary: false,
            },
            ChatQuickAction {
                label: "Fourth".into(),
                prompt: "p4".into(),
                primary: true,
            },
        ];
        let actions = sanitize_chat_quick_actions(candidates);
        assert_eq!(actions.len(), 3, "empty label dropped, cap at three");
        assert_eq!(actions[0].label, "First");
        assert!(!actions[0].primary);
        // "first" duplicates "First" case-insensitively and is dropped; the
        // kept Third/Fourth fill the slots. Fourth's primary flag is honored.
        assert_eq!(actions[1].label, "Third");
        assert!(!actions[1].primary);
        assert_eq!(actions[2].label, "Fourth");
        assert!(actions[2].primary);
    }

    #[test]
    fn render_empty_previous_writes_none_marker() {
        let msgs = vec![msg("assistant", "r")];
        let out = render_chat_quick_actions_context(&msgs, &[]);
        assert!(out.contains("ALREADY SUGGESTED (do not repeat or paraphrase):\n(none)\n"));
    }

    #[test]
    fn truncation_keeps_head_and_tail_of_long_latest_reply() {
        let long = format!("{}middle{}", "h".repeat(2500), "t".repeat(2500));
        let out = truncate_chat_quick_actions_latest(&long);
        assert_eq!(
            out.chars().count(),
            2000 + "\n…[truncated]…\n".chars().count() + 1000
        );
        assert!(out.starts_with(&"h".repeat(2000)));
        assert!(out.ends_with(&"t".repeat(1000)));
        assert!(out.contains("\n…[truncated]…\n"));
    }

    #[test]
    fn truncation_passthrough_within_budget() {
        assert_eq!(truncate_chat_quick_actions_latest("short"), "short");
        assert_eq!(truncate_chat_quick_actions_runes("short", 10), "short");
    }

    #[test]
    fn truncation_is_rune_safe_not_byte_safe() {
        // 5 CJK runes = 15 bytes; budget 4 runes must keep 4 runes, not bytes.
        let cjk = "你好世界再见";
        assert_eq!(truncate_chat_quick_actions_runes(cjk, 4), "你好世界…");
        let long = format!("{cjk}{cjk}{cjk}"); // 15 runes
        let out = truncate_chat_quick_actions_latest(&long);
        let head: String = long.chars().take(2000).collect();
        assert!(out.starts_with(&head));
    }

    #[test]
    fn origin_and_action_serde_roundtrip_matches_go_tags() {
        let action: ChatQuickAction =
            serde_json::from_str(r#"{"label":"L","prompt":"P","primary":true}"#).unwrap();
        assert!(action.primary);
        // omitempty: false primary is absent on serialize.
        let plain = ChatQuickAction {
            label: "a".into(),
            prompt: "b".into(),
            primary: false,
        };
        assert_eq!(
            serde_json::to_string(&plain).unwrap(),
            r#"{"label":"a","prompt":"b"}"#
        );
    }
}
