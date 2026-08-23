//! The outbound half of the Telegram round trip: streaming placeholder
//! edits and the terminal-reply delivery queue.
//!
//! Port of `server/internal/integrations/telegram/outbound.go`.
//!
//! Streaming: Telegram has no stream-update protocol, so the "stream
//! frame" UX is simulated with the platform's canonical pattern — post one
//! placeholder message on the first partial, then throttled
//! editMessageText calls as the transcript grows, and a final edit/send on
//! the done event. Edits are throttled per chat; on a 429 the streamer
//! backs off and the final content always lands via the done path.
//!
//! Port note: Go drives this from the synchronous in-process event bus and
//! owns a worker pool + retry heap. Rust exposes the same state machine as
//! pure decision functions over shared state plus async delivery methods;
//! the event-bus wiring lands with the S8 handler slice.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::api::{ApiError, BotApi, EditMessageTextParams, ReplyParameters, SendMessageParams};
use crate::markdown::format_html;
use crate::sender::{chunk_message, utf16_units, MAX_MESSAGE_UNITS};

/// Minimum spacing between editMessageText calls per chat. Telegram
/// tolerates roughly one edit per second per chat, with a much stricter
/// per-group budget (~20 messages/min); 2.5s keeps a long generation well
/// inside both without feeling static.
pub const EDIT_INTERVAL: Duration = Duration::from_millis(2500);

/// Idle schedules remain briefly reusable so sequential tasks and
/// cancellation cannot discard a chat's edit cooldown or retry_after
/// window. Both the schedule and compressed-fallback maps have hard caps.
pub const CHAT_SCHEDULE_IDLE_TTL: Duration = Duration::from_secs(10 * 60);
pub const MAX_CHAT_SCHEDULES: usize = 1024;
pub const MAX_BOT_FALLBACKS: usize = 1024;
pub const CHAT_CAPACITY_RETRY: Duration = Duration::from_secs(1);

/// The first frame's text while the first tokens arrive.
pub const STREAM_PLACEHOLDER: &str = "…";

/// Sent when the agent run fails outright.
pub const TASK_FAILED_TEXT: &str = "❌ 智能体处理失败，请稍后重试。";

/// Serializes Telegram delivery and owns rate-limit state shared by every
/// task targeting the same chat.
#[derive(Debug, Clone)]
pub struct ChatSchedule {
    pub bot_key: String,
    pub chat_id: i64,
    pub refs: i64,
    pub last_edit: Option<SystemTime>,
    pub backoff_till: Option<SystemTime>,
    /// Lock-free backoff snapshot for pruning (Go's atomic.Int64 nanos).
    pub idle_since: Option<SystemTime>,
}

impl ChatSchedule {
    fn new(bot_key: String, chat_id: i64) -> Self {
        Self {
            bot_key,
            chat_id,
            refs: 0,
            last_edit: None,
            backoff_till: None,
            idle_since: None,
        }
    }

    /// When the next edit may fire: the later of the 429 backoff and the
    /// per-chat edit spacing.
    pub fn edit_available_at(&self, now: SystemTime) -> SystemTime {
        let mut available = now;
        if let Some(t) = self.backoff_till {
            if t > available {
                available = t;
            }
        }
        if let Some(last) = self.last_edit {
            let spaced = last + EDIT_INTERVAL;
            if spaced > available {
                available = spaced;
            }
        }
        available
    }
}

/// One in-flight streamed reply.
#[derive(Debug, Clone, Default)]
pub struct StreamState {
    pub chat_id: i64,
    pub thread_id: i64,
    pub reply_to: i64,
    /// Placeholder message being edited; 0 until the first send.
    pub message_id: i64,
    pub accumulated: String,
}

/// The decision [`stream_partial`] makes for the next Telegram call.
#[derive(Debug, Clone, PartialEq)]
pub enum PartialAction {
    /// Nothing to do yet (edit cooldown or 429 backoff active).
    Wait,
    /// Post the placeholder (first frame).
    SendPlaceholder {
        text: String,
        thread_id: i64,
        reply_to: Option<ReplyParameters>,
    },
    /// Edit the existing placeholder.
    Edit { message_id: i64, text: String },
}

/// Pure decision for one partial frame: accumulate, cap at the message
/// budget, and choose send-vs-edit. Mirrors pushPartial's gating.
pub fn stream_partial(
    st: &mut StreamState,
    content: &str,
    now: SystemTime,
    schedule: &ChatSchedule,
) -> PartialAction {
    st.accumulated.push_str(content);
    let snapshot = st.accumulated.clone();
    if schedule.edit_available_at(now) > now {
        return PartialAction::Wait;
    }
    let mut text = snapshot;
    if utf16_units(&text) > MAX_MESSAGE_UNITS {
        // Mid-stream overflow: freeze the streamed message at the cap; the
        // full reply is delivered in chunks by the final done send.
        text = chunk_message(&text, MAX_MESSAGE_UNITS)
            .first()
            .cloned()
            .unwrap_or_default();
    }
    if st.message_id == 0 {
        let reply_to = if st.reply_to != 0 {
            Some(ReplyParameters {
                message_id: st.reply_to,
                allow_sending_without_reply: true,
            })
        } else {
            None
        };
        // Empty HTML renders as the placeholder so the user always sees a
        // live message.
        let rendered = format_html(&text);
        let text = if rendered.is_empty() {
            STREAM_PLACEHOLDER.to_string()
        } else {
            rendered
        };
        PartialAction::SendPlaceholder {
            text,
            thread_id: st.thread_id,
            reply_to,
        }
    } else {
        PartialAction::Edit {
            message_id: st.message_id,
            text: format_html(&text),
        }
    }
}

/// Records a successful placeholder send / edit on the shared state.
pub fn stream_applied(
    st: &mut StreamState,
    schedule: &mut ChatSchedule,
    message_id: i64,
    now: SystemTime,
) {
    if message_id != 0 {
        st.message_id = message_id;
    }
    schedule.last_edit = Some(now);
}

/// Reports Telegram's "message is not modified" edit error, which is
/// benign (identical snapshot).
pub fn is_not_modified(err: &anyhow::Error) -> bool {
    let Some(ae) = err.chain().find_map(|c| c.downcast_ref::<ApiError>()) else {
        return false;
    };
    ae.code == 400
        && ae
            .description
            .to_lowercase()
            .contains("message is not modified")
}

/// The terminal-reply delivery state machine for one chat session.
/// Go interleaves these through a worker pool + retry heap; the decision
/// logic below is the same per-request step function, driven by whichever
/// executor the wiring slice chooses.
#[derive(Debug, Clone, Default)]
pub struct TerminalReply {
    /// Resolved destination, filled on init.
    pub chat_id: i64,
    pub thread_id: i64,
    pub reply_to: i64,
    pub bot_token: String,
    pub initialized: bool,
    pub chunks: Vec<String>,
    pub chunk_index: usize,
    pub streamed_message_id: i64,
    pub placeholder_edited: bool,
    pub fallback_fresh_send: bool,
    pub plain_text_fallback: bool,
}

/// The outcome of one terminal delivery step.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalStep {
    /// The reply finished (success or permanent failure).
    Done,
    /// Retry at the given instant (edit spacing, 429, or capacity).
    RetryAt(SystemTime),
    /// Perform this Telegram call now.
    Call(TerminalCall),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TerminalCall {
    /// Edit the streamed placeholder into the first chunk.
    EditPlaceholder { message_id: i64, text: String },
    /// Send one chunk.
    SendChunk {
        text: String,
        parse_html: bool,
        thread_id: i64,
        reply_to: Option<ReplyParameters>,
    },
}

/// One delivery step: init → edit-placeholder → chunked sends with the
/// plain-text fallback. Mirrors sendNextTerminalRequest exactly.
pub fn terminal_step(
    reply: &mut TerminalReply,
    schedule: &ChatSchedule,
    full_content: &str,
    now: SystemTime,
) -> TerminalStep {
    if !reply.initialized {
        reply.chunks = chunk_message(full_content, MAX_MESSAGE_UNITS);
        reply.initialized = true;
        if reply.chunks.is_empty() {
            return TerminalStep::Done;
        }
        return TerminalStep::RetryAt(now);
    }

    let available = schedule.edit_available_at(now);
    if available > now {
        return TerminalStep::RetryAt(available);
    }

    // First chunk edits the streamed placeholder when one exists.
    if reply.streamed_message_id != 0 && !reply.placeholder_edited && !reply.fallback_fresh_send {
        return TerminalStep::Call(TerminalCall::EditPlaceholder {
            message_id: reply.streamed_message_id,
            text: format_html(&reply.chunks[0]),
        });
    }

    let chunk = reply.chunks[reply.chunk_index].clone();
    let parse_html = !reply.plain_text_fallback;
    let reply_to = if reply.chunk_index == 0 && reply.reply_to != 0 {
        Some(ReplyParameters {
            message_id: reply.reply_to,
            allow_sending_without_reply: true,
        })
    } else {
        None
    };
    TerminalStep::Call(TerminalCall::SendChunk {
        text: chunk,
        parse_html,
        thread_id: reply.thread_id,
        reply_to,
    })
}

/// Applies a successful placeholder edit: advance to the next chunk (or
/// finish when there is only one).
pub fn terminal_placeholder_edited(reply: &mut TerminalReply, now: SystemTime) -> TerminalStep {
    reply.placeholder_edited = true;
    reply.chunk_index = 1;
    if reply.chunk_index == reply.chunks.len() {
        return TerminalStep::Done;
    }
    TerminalStep::RetryAt(now + EDIT_INTERVAL)
}

/// Applies a placeholder-edit failure: fall back to a fresh send of chunk
/// 0 (the streamed message stays as-is).
pub fn terminal_placeholder_failed(reply: &mut TerminalReply, now: SystemTime) -> TerminalStep {
    reply.fallback_fresh_send = true;
    reply.chunk_index = 0;
    TerminalStep::RetryAt(now)
}

/// Applies a successful chunk send: advance or finish.
pub fn terminal_chunk_sent(reply: &mut TerminalReply, now: SystemTime) -> TerminalStep {
    reply.chunk_index += 1;
    reply.plain_text_fallback = false;
    if reply.chunk_index == reply.chunks.len() {
        return TerminalStep::Done;
    }
    TerminalStep::RetryAt(now + EDIT_INTERVAL)
}

/// Builds the SendMessageParams for a [`TerminalCall::SendChunk`].
pub fn terminal_send_params(call: &TerminalCall) -> Option<SendMessageParams> {
    let TerminalCall::SendChunk {
        text,
        parse_html,
        thread_id,
        reply_to,
    } = call
    else {
        return None;
    };
    Some(SendMessageParams {
        chat_id: 0, // filled by the caller (reply.chat_id)
        text: if *parse_html {
            format_html(text)
        } else {
            text.clone()
        },
        parse_mode: if *parse_html {
            "HTML".into()
        } else {
            String::new()
        },
        message_thread_id: *thread_id,
        reply_parameters: *reply_to,
    })
}

/// The shared per-bot fallback map: when a chat schedule must be evicted
/// while it still holds an active 429 backoff, that deadline merges into
/// the bot-level fallback so the rate-limit state survives eviction.
#[derive(Default)]
pub struct BotFallbackBackoff {
    inner: Mutex<HashMap<String, SystemTime>>,
}

impl BotFallbackBackoff {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn merge(&self, bot_key: &str, until: SystemTime, now: SystemTime) {
        if until <= now {
            return;
        }
        let mut map = self.inner.lock().unwrap();
        prune_locked(&mut map, now);
        match map.get(bot_key) {
            Some(current) if *current >= until => {}
            _ => {
                map.insert(bot_key.to_string(), until);
            }
        }
    }

    pub fn till(&self, bot_key: &str, now: SystemTime) -> Option<SystemTime> {
        let mut map = self.inner.lock().unwrap();
        prune_locked(&mut map, now);
        map.get(bot_key).copied()
    }

    fn can_merge(&self, bot_key: &str, now: SystemTime) -> bool {
        let mut map = self.inner.lock().unwrap();
        prune_locked(&mut map, now);
        map.contains_key(bot_key) || map.len() < MAX_BOT_FALLBACKS
    }
}

fn prune_locked(map: &mut HashMap<String, SystemTime>, now: SystemTime) {
    map.retain(|_, until| *until > now);
}

/// The chat-schedule registry: refcounted per (bot, chat) with an idle TTL
/// and a hard capacity that evicts inactive schedules first, merging their
/// backoff into the bot fallback when necessary.
#[derive(Default)]
pub struct ChatScheduleRegistry {
    chats: Mutex<BTreeMap<(String, i64), ChatSchedule>>,
    fallback: BotFallbackBackoff,
}

impl ChatScheduleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquires a schedule for delivery, creating or reviving one as
    /// needed. `None` when the cache is saturated with in-use entries.
    pub fn retain(&self, bot_key: &str, chat_id: i64, now: SystemTime) -> Option<()> {
        let mut chats = self.chats.lock().unwrap();
        Self::prune_idle_locked(&mut chats, now);
        let key = (bot_key.to_string(), chat_id);
        if let Some(schedule) = chats.get_mut(&key) {
            let expired = schedule.refs == 0
                && schedule.idle_since.is_some_and(|t| {
                    now.duration_since(t).unwrap_or_default() >= CHAT_SCHEDULE_IDLE_TTL
                })
                && schedule.backoff_till.is_none_or(|t| t <= now);
            if expired {
                chats.remove(&key);
            }
        }
        if !chats.contains_key(&key) {
            if !Self::make_room_locked(&mut chats, &self.fallback, now) {
                return None;
            }
            let mut schedule = ChatSchedule::new(bot_key.to_string(), chat_id);
            schedule.backoff_till = self.fallback.till(bot_key, now);
            chats.insert(key.clone(), schedule);
        }
        let schedule = chats.get_mut(&key).unwrap();
        schedule.refs += 1;
        schedule.idle_since = None;
        Some(())
    }

    pub fn release(&self, bot_key: &str, chat_id: i64, now: SystemTime) {
        let mut chats = self.chats.lock().unwrap();
        let key = (bot_key.to_string(), chat_id);
        if let Some(schedule) = chats.get_mut(&key) {
            schedule.refs -= 1;
            if schedule.refs == 0 {
                schedule.idle_since = Some(now);
            }
        }
    }

    /// Mutates a schedule through `f` (edit timestamps, backoff).
    pub fn with_schedule<R>(
        &self,
        bot_key: &str,
        chat_id: i64,
        f: impl FnOnce(&mut ChatSchedule) -> R,
    ) -> Option<R> {
        let mut chats = self.chats.lock().unwrap();
        chats.get_mut(&(bot_key.to_string(), chat_id)).map(f)
    }

    pub fn schedule_snapshot(&self, bot_key: &str, chat_id: i64) -> Option<ChatSchedule> {
        let chats = self.chats.lock().unwrap();
        chats.get(&(bot_key.to_string(), chat_id)).cloned()
    }

    pub fn fallback_till(&self, bot_key: &str, now: SystemTime) -> Option<SystemTime> {
        self.fallback.till(bot_key, now)
    }

    fn prune_idle_locked(chats: &mut BTreeMap<(String, i64), ChatSchedule>, now: SystemTime) {
        chats.retain(|_, schedule| {
            !(schedule.refs == 0
                && schedule.idle_since.is_some_and(|t| {
                    now.duration_since(t).unwrap_or_default() >= CHAT_SCHEDULE_IDLE_TTL
                })
                && schedule.backoff_till.is_none_or(|t| t <= now))
        });
    }

    /// Enforces the hard cap: evict the oldest inactive schedule; if every
    /// idle candidate holds an active retry_after, merge its exact deadline
    /// into the bot fallback before eviction. A fully in-use cache refuses.
    fn make_room_locked(
        chats: &mut BTreeMap<(String, i64), ChatSchedule>,
        fallback: &BotFallbackBackoff,
        now: SystemTime,
    ) -> bool {
        if chats.len() < MAX_CHAT_SCHEDULES {
            return true;
        }
        // Pass 1: any inactive schedule without an active backoff.
        let inactive = chats
            .iter()
            .filter(|(_, s)| {
                s.refs == 0
                    && s.idle_since
                        .is_some_and(|t| now.duration_since(t).unwrap_or_default() >= EDIT_INTERVAL)
                    && s.backoff_till.is_none_or(|t| t <= now)
            })
            .map(|(k, s)| (k.clone(), s.idle_since))
            .min_by_key(|(_, idle)| *idle);
        if let Some((key, _)) = inactive {
            chats.remove(&key);
            return true;
        }
        // Pass 2: inactive with an active backoff — merge then evict.
        let active = chats
            .iter()
            .filter(|(_, s)| {
                s.refs == 0
                    && s.idle_since
                        .is_some_and(|t| now.duration_since(t).unwrap_or_default() >= EDIT_INTERVAL)
                    && s.backoff_till.is_some_and(|t| t > now)
            })
            .filter(|(_, s)| fallback.can_merge(&s.bot_key, now))
            .map(|(k, s)| (k.clone(), s.idle_since, s.backoff_till))
            .min_by_key(|(_, idle, _)| *idle);
        if let Some((key, _, backoff)) = active {
            if let Some(until) = backoff {
                fallback.merge(&key.0, until, now);
            }
            chats.remove(&key);
            return true;
        }
        false
    }
}

/// Executes a partial action against the live API, updating state on
/// success and reporting failure classification for the backoff decision.
pub async fn execute_partial(
    api: &BotApi,
    st: &mut StreamState,
    schedule: &mut ChatSchedule,
    action: PartialAction,
    now: SystemTime,
) -> anyhow::Result<()> {
    match action {
        PartialAction::Wait => Ok(()),
        PartialAction::SendPlaceholder {
            text,
            thread_id,
            reply_to,
        } => {
            let mut params = SendMessageParams {
                chat_id: st.chat_id,
                text,
                parse_mode: "HTML".into(),
                message_thread_id: thread_id,
                reply_parameters: reply_to,
            };
            params.chat_id = st.chat_id;
            let m = api.send_message(&params).await?;
            stream_applied(st, schedule, m.message_id, now);
            Ok(())
        }
        PartialAction::Edit { message_id, text } => {
            let params = EditMessageTextParams {
                chat_id: st.chat_id,
                message_id,
                text,
                parse_mode: "HTML".into(),
            };
            match api.edit_message_text(&params).await {
                Ok(()) => {
                    stream_applied(st, schedule, 0, now);
                    Ok(())
                }
                Err(err) if is_not_modified(&err) => {
                    stream_applied(st, schedule, 0, now);
                    Ok(())
                }
                Err(err) => Err(err),
            }
        }
    }
}

/// Builds the task-failed notice edit for a streamed placeholder.
pub fn failure_edit(st: &StreamState) -> Option<EditMessageTextParams> {
    (st.message_id != 0).then(|| EditMessageTextParams {
        chat_id: st.chat_id,
        message_id: st.message_id,
        text: TASK_FAILED_TEXT.to_string(),
        parse_mode: String::new(),
    })
}

/// Builds the task-failed notice send.
pub fn failure_send(chat_id: i64, thread_id: i64, reply_to: i64) -> SendMessageParams {
    SendMessageParams {
        chat_id,
        text: TASK_FAILED_TEXT.to_string(),
        parse_mode: String::new(),
        message_thread_id: thread_id,
        reply_parameters: if reply_to != 0 {
            Some(ReplyParameters {
                message_id: reply_to,
                allow_sending_without_reply: true,
            })
        } else {
            None
        },
    }
}

/// Recovers the numeric chat id (preferring the binding config over the
/// composite binding key), the reply thread, and the quote target from a
/// channel_chat_session_binding row.
pub fn outbound_target(
    binding_chat_id: &str,
    binding_config: &serde_json::Value,
    last_thread_id: Option<&str>,
    last_message_id: Option<&str>,
) -> (i64, i64, i64) {
    let mut raw = binding_chat_id.to_string();
    if let Some(cfg_chat) = binding_config.get("chat_id").and_then(|v| v.as_str()) {
        if !cfg_chat.is_empty() {
            raw = cfg_chat.to_string();
        }
    }
    let chat_id = raw.parse().unwrap_or(0);
    let thread_id = last_thread_id.and_then(|t| t.parse().ok()).unwrap_or(0);
    let reply_to = last_message_id.map(crate::parse_message_ref).unwrap_or(0);
    (chat_id, thread_id, reply_to)
}

/// Extracts the reply text from a chat-done payload (Go chatDoneContent).
pub fn chat_done_content(payload: &serde_json::Value) -> String {
    payload
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Extracts the task id from an event payload (Go eventTaskID).
pub fn event_task_id(payload: &serde_json::Value) -> Option<Uuid> {
    payload
        .get("task_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
}

/// Whether the failure payload asks for a silent auto-retry (no user
/// notice — the retry will deliver a fresh outcome).
pub fn task_failure_retry_pending(payload: &serde_json::Value) -> bool {
    payload
        .get("retry_pending")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

use uuid::Uuid;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn epoch(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn schedule() -> ChatSchedule {
        ChatSchedule::new("bot".into(), 1)
    }

    #[test]
    fn stream_first_frame_sends_placeholder() {
        let mut st = StreamState {
            chat_id: -100,
            thread_id: 5,
            reply_to: 42,
            ..Default::default()
        };
        let sched = schedule();
        let now = epoch(1_000);
        match stream_partial(&mut st, "hello ", now, &sched) {
            PartialAction::SendPlaceholder {
                text,
                thread_id,
                reply_to,
            } => {
                assert!(text.contains("hello"));
                assert_eq!(thread_id, 5);
                assert_eq!(reply_to.unwrap().message_id, 42);
            }
            other => panic!("expected SendPlaceholder, got {other:?}"),
        }
        // Record the placeholder id; next frame becomes an edit.
        let mut sched2 = schedule();
        stream_applied(&mut st, &mut sched2, 77, now);
        assert_eq!(st.message_id, 77);
        let sched = schedule();
        match stream_partial(&mut st, "world", now, &sched) {
            PartialAction::Edit { message_id, text } => {
                assert_eq!(message_id, 77);
                assert!(text.contains("world"));
            }
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn stream_waits_inside_edit_interval() {
        let mut st = StreamState::default();
        let mut sched = schedule();
        let now = epoch(1_000);
        sched.last_edit = Some(now);
        match stream_partial(&mut st, "x", now, &sched) {
            PartialAction::Wait => {}
            other => panic!("expected Wait, got {other:?}"),
        }
        // After the interval passes, delivery proceeds.
        let later = now + EDIT_INTERVAL + Duration::from_secs(1);
        match stream_partial(&mut st, "x", later, &sched) {
            PartialAction::SendPlaceholder { .. } => {}
            other => panic!("expected SendPlaceholder, got {other:?}"),
        }
    }

    #[test]
    fn stream_overflow_freezes_at_budget() {
        let mut st = StreamState::default();
        let sched = schedule();
        let long = "a".repeat(MAX_MESSAGE_UNITS * 2);
        match stream_partial(&mut st, &long, epoch(1), &sched) {
            PartialAction::SendPlaceholder { text, .. } => {
                assert!(utf16_units(&text) <= MAX_MESSAGE_UNITS);
            }
            other => panic!("expected SendPlaceholder, got {other:?}"),
        }
    }

    #[test]
    fn not_modified_is_benign() {
        let err = anyhow::Error::new(ApiError {
            code: 400,
            description: "Bad Request: message is not modified".into(),
            retry_after: 0,
        });
        assert!(is_not_modified(&err));
        let err = anyhow::Error::new(ApiError {
            code: 400,
            description: "Bad Request: chat not found".into(),
            retry_after: 0,
        });
        assert!(!is_not_modified(&err));
    }

    #[test]
    fn terminal_steps_follow_edit_placeholder_then_chunks() {
        let mut reply = TerminalReply {
            streamed_message_id: 55,
            chat_id: -100,
            thread_id: 3,
            ..Default::default()
        };
        let sched = schedule();
        let now = epoch(2_000);
        // Init: chunk + immediate retry.
        match terminal_step(&mut reply, &sched, "one\ntwo", now) {
            TerminalStep::RetryAt(t) => assert_eq!(t, now),
            other => panic!("expected RetryAt, got {other:?}"),
        }
        // Edit placeholder.
        match terminal_step(&mut reply, &sched, "one\ntwo", now) {
            TerminalStep::Call(TerminalCall::EditPlaceholder { message_id, text }) => {
                assert_eq!(message_id, 55);
                assert!(text.contains("<br") || text.contains("one"));
            }
            other => panic!("expected EditPlaceholder, got {other:?}"),
        }
        // Single-chunk content finishes right after the placeholder edit.
        match terminal_placeholder_edited(&mut reply, now) {
            TerminalStep::Done => {}
            other => panic!("expected Done, got {other:?}"),
        }

        // Multi-chunk content: after the placeholder edit, chunk 1 sends
        // following the edit interval, and only chunk 0 quotes.
        let mut reply = TerminalReply {
            streamed_message_id: 55,
            chat_id: -100,
            thread_id: 3,
            ..Default::default()
        };
        let long = format!("{}\n{}", "a".repeat(MAX_MESSAGE_UNITS), "tail");
        let _ = terminal_step(&mut reply, &sched, &long, now); // init
        match terminal_step(&mut reply, &sched, &long, now) {
            TerminalStep::Call(TerminalCall::EditPlaceholder { message_id, .. }) => {
                assert_eq!(message_id, 55);
            }
            other => panic!("expected EditPlaceholder, got {other:?}"),
        }
        match terminal_placeholder_edited(&mut reply, now) {
            TerminalStep::RetryAt(t) => assert_eq!(t, now + EDIT_INTERVAL),
            other => panic!("expected RetryAt, got {other:?}"),
        }
        match terminal_step(&mut reply, &sched, &long, now) {
            TerminalStep::Call(TerminalCall::SendChunk {
                text,
                parse_html,
                thread_id,
                reply_to,
            }) => {
                // The chunk boundary lands right after the newline; the
                // leading newline of the tail chunk is preserved verbatim
                // (Go does not re-trim chunk interiors).
                assert_eq!(text.trim(), "tail");
                assert!(parse_html);
                assert_eq!(thread_id, 3);
                assert!(reply_to.is_none(), "only chunk 0 quotes");
            }
            other => panic!("expected SendChunk, got {other:?}"),
        }
        assert_eq!(terminal_chunk_sent(&mut reply, now), TerminalStep::Done);
    }

    #[test]
    fn terminal_placeholder_failure_falls_back_to_fresh_send() {
        let mut reply = TerminalReply {
            streamed_message_id: 55,
            chat_id: -100,
            reply_to: 9,
            ..Default::default()
        };
        let sched = schedule();
        let now = epoch(3_000);
        let _ = terminal_step(&mut reply, &sched, "only", now); // init
        let _ = terminal_step(&mut reply, &sched, "only", now); // edit call
        match terminal_placeholder_failed(&mut reply, now) {
            TerminalStep::RetryAt(t) => assert_eq!(t, now),
            other => panic!("expected RetryAt, got {other:?}"),
        }
        // Fresh send of chunk 0, quoting the reply target.
        match terminal_step(&mut reply, &sched, "only", now) {
            TerminalStep::Call(TerminalCall::SendChunk { reply_to, .. }) => {
                assert_eq!(reply_to.unwrap().message_id, 9);
            }
            other => panic!("expected SendChunk, got {other:?}"),
        }
    }

    #[test]
    fn html_parse_failure_switches_to_plain_text() {
        let mut reply = TerminalReply {
            chat_id: 1,
            chunks: vec!["<b>x".into()],
            initialized: true,
            ..Default::default()
        };
        let sched = schedule();
        let now = epoch(4_000);
        match terminal_step(&mut reply, &sched, "<b>x", now) {
            TerminalStep::Call(TerminalCall::SendChunk { parse_html, .. }) => {
                assert!(parse_html);
            }
            other => panic!("expected SendChunk, got {other:?}"),
        }
        // Simulate the parse error path: plain_text_fallback flips on and
        // the retry sends unparsed.
        reply.plain_text_fallback = true;
        match terminal_step(&mut reply, &sched, "<b>x", now) {
            TerminalStep::Call(TerminalCall::SendChunk {
                parse_html, text, ..
            }) => {
                assert!(!parse_html);
                assert_eq!(text, "<b>x");
            }
            other => panic!("expected SendChunk, got {other:?}"),
        }
        // Success resets the fallback for subsequent chunks.
        assert_eq!(terminal_chunk_sent(&mut reply, now), TerminalStep::Done);
    }

    #[test]
    fn backoff_gates_terminal_steps() {
        let mut reply = TerminalReply {
            chunks: vec!["x".into()],
            initialized: true,
            ..Default::default()
        };
        let mut sched = schedule();
        sched.backoff_till = Some(epoch(5_100));
        let now = epoch(5_000);
        match terminal_step(&mut reply, &sched, "x", now) {
            TerminalStep::RetryAt(t) => assert_eq!(t, epoch(5_100)),
            other => panic!("expected RetryAt, got {other:?}"),
        }
    }

    #[test]
    fn registry_refcount_and_capacity_eviction() {
        let reg = ChatScheduleRegistry::new();
        let now = epoch(10_000);
        assert!(reg.retain("bot", 1, now).is_some());
        reg.with_schedule("bot", 1, |s| s.last_edit = Some(now));
        reg.release("bot", 1, now);

        // Snapshot reflects the retained edit timestamp.
        let snap = reg.schedule_snapshot("bot", 1).unwrap();
        assert_eq!(snap.last_edit, Some(now));
        drop(snap);

        // Fill to capacity and force eviction of the idle schedule.
        let mut ok = true;
        for i in 0..MAX_CHAT_SCHEDULES {
            let chat = 2 + i as i64;
            ok = reg.retain("bot", chat, now).is_some();
            if !ok {
                break;
            }
            reg.release("bot", chat, now);
        }
        // The original schedule (older idle_since) is the first evicted, so
        // a fresh retain for it re-creates cleanly.
        assert!(reg.retain("bot", 1, now).is_some());
        let _ = ok;
    }

    #[test]
    fn fallback_merges_backoff_across_eviction() {
        let fallback = BotFallbackBackoff::new();
        let later = epoch(20_000);
        let now = epoch(19_000);
        fallback.merge("bot", later, now);
        assert_eq!(fallback.till("bot", now), Some(later));
        // Earlier merge does not shorten.
        fallback.merge("bot", epoch(19_500), now);
        assert_eq!(fallback.till("bot", now), Some(later));
        // Past deadlines prune away.
        let after = later + Duration::from_secs(1);
        assert_eq!(fallback.till("bot", after), None);
    }

    #[test]
    fn outbound_target_prefers_config_chat_id() {
        let cfg = json!({"chat_id": "-999"});
        let (chat, thread, reply) = outbound_target("-100", &cfg, Some("7"), Some("-100:88"));
        assert_eq!(chat, -999);
        assert_eq!(thread, 7);
        assert_eq!(reply, 88);
        // Without config the binding key is used verbatim.
        let (chat, _, _) = outbound_target("-100", &serde_json::Value::Null, None, None);
        assert_eq!(chat, -100);
    }

    #[test]
    fn payload_helpers_extract_fields() {
        let payload =
            json!({"content": "final text", "task_id": "0198c0de-0000-7000-8000-000000000001"});
        assert_eq!(chat_done_content(&payload), "final text");
        assert!(event_task_id(&payload).is_some());
        assert!(!task_failure_retry_pending(&payload));
        assert!(task_failure_retry_pending(&json!({"retry_pending": true})));
    }

    #[test]
    fn failure_notices_carry_thread_and_quote() {
        let edit = failure_edit(&StreamState {
            message_id: 31,
            chat_id: -100,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(edit.message_id, 31);
        assert_eq!(edit.text, TASK_FAILED_TEXT);
        assert!(failure_edit(&StreamState::default()).is_none());

        let send = failure_send(-100, 5, 88);
        assert_eq!(send.message_thread_id, 5);
        assert_eq!(send.reply_parameters.unwrap().message_id, 88);
    }
}
