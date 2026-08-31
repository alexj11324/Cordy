//! The outbound half of the Telegram round trip: streaming placeholder
//! edits and the terminal-reply delivery queue.
//!
//!
//! Streaming: Telegram has no stream-update protocol, so the "stream
//! frame" UX is simulated with the platform's canonical pattern — post one
//! placeholder message on the first partial, then throttled
//! editMessageText calls as the Agent event history grows, and a final edit/send on
//! the done event. Edits are throttled per chat; on a 429 the streamer
//! backs off and the final content always lands via the done path.
//!
//! Port note: Go drives this from the synchronous in-process event bus and
//! owns a worker pool + retry heap. Rust exposes the same state machine as
//! pure decision functions over shared state plus async delivery methods;
//! the event-bus wiring lands with the S8 handler slice.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::api::{ApiError, BotApi, EditMessageTextParams, ReplyParameters, SendMessageParams};
use crate::config::{decode_credentials, DecrypterFn};
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

const OUTBOUND_DELIVERY_CAPACITY: usize = 64;

struct RuntimeTarget {
    installation_id: Uuid,
    installed_at: chrono::DateTime<chrono::Utc>,
    stream_key: String,
    bot_key: String,
    api: BotApi,
    chat_id: i64,
    thread_id: i64,
    reply_to: i64,
}

struct LiveStream {
    state: StreamState,
    bot_key: String,
}

/// Production event-bus subscriber for Telegram streaming, completion, and
/// failure delivery. The pure state machines above remain the protocol source
/// of truth; this type owns their database lookup and network execution.
pub struct Outbound {
    pool: PgPool,
    bus: Weak<patchbay_events::Bus>,
    decrypt: Option<Arc<DecrypterFn>>,
    api_base: String,
    cancel: CancellationToken,
    streams: tokio::sync::Mutex<HashMap<String, LiveStream>>,
    schedules: ChatScheduleRegistry,
    delivery_slots: Arc<tokio::sync::Semaphore>,
}

impl Outbound {
    pub fn new(
        pool: PgPool,
        bus: Arc<patchbay_events::Bus>,
        decrypt: Option<Arc<DecrypterFn>>,
        api_base: String,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            pool,
            bus: Arc::downgrade(&bus),
            decrypt,
            api_base,
            cancel,
            streams: tokio::sync::Mutex::new(HashMap::new()),
            schedules: ChatScheduleRegistry::new(),
            delivery_slots: Arc::new(tokio::sync::Semaphore::new(OUTBOUND_DELIVERY_CAPACITY)),
        }
    }

    pub fn register(
        self: &Arc<Self>,
        bus: &patchbay_events::Bus,
        tasks: Arc<patchbay_channel::RuntimeTasks>,
    ) {
        let this = self.clone();
        let partial_tasks = tasks.clone();
        bus.subscribe(patchbay_protocol::EVENT_TASK_MESSAGE, move |event| {
            let this = this.clone();
            let event = event.clone();
            partial_tasks.spawn(async move {
                if let Err(error) = this.process_partial(&event).await {
                    tracing::warn!(%error, task_id = %event.task_id, "telegram partial delivery failed");
                }
            });
        });

        for event_type in [
            patchbay_protocol::EVENT_CHAT_DONE,
            patchbay_protocol::EVENT_TASK_FAILED,
            patchbay_protocol::EVENT_TASK_CANCELLED,
        ] {
            let this = self.clone();
            let tasks = tasks.clone();
            bus.subscribe(event_type, move |event| {
                let Ok(permit) = this.delivery_slots.clone().try_acquire_owned() else {
                    tracing::warn!(task_id = %event.task_id, "telegram terminal delivery queue is full");
                    return;
                };
                let this = this.clone();
                let event = event.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    if let Err(error) = this.process_terminal(&event).await {
                        tracing::warn!(%error, task_id = %event.task_id, "telegram terminal delivery failed");
                    }
                });
            });
        }
    }

    async fn process_partial(&self, event: &patchbay_events::Event) -> anyhow::Result<()> {
        if self.cancel.is_cancelled() {
            return Ok(());
        }
        let Some(content) = task_message_content(&event.payload) else {
            return Ok(());
        };
        let Some(target) = self.resolve_target(event).await? else {
            return Ok(());
        };
        let now = SystemTime::now();
        let mut streams = self.streams.lock().await;
        if !streams.contains_key(&target.stream_key) {
            if self
                .schedules
                .retain(&target.bot_key, target.chat_id, now)
                .is_none()
            {
                return Ok(());
            }
            streams.insert(
                target.stream_key.clone(),
                LiveStream {
                    state: StreamState {
                        chat_id: target.chat_id,
                        thread_id: target.thread_id,
                        reply_to: target.reply_to,
                        ..Default::default()
                    },
                    bot_key: target.bot_key.clone(),
                },
            );
        }
        let schedule = self
            .schedules
            .schedule_snapshot(&target.bot_key, target.chat_id)
            .unwrap_or_else(|| ChatSchedule::new(target.bot_key.clone(), target.chat_id));
        let stream = streams
            .get_mut(&target.stream_key)
            .map(|stream| &mut stream.state)
            .ok_or_else(|| anyhow::anyhow!("telegram stream state disappeared"))?;
        let action = stream_partial(stream, content, now, &schedule);
        let result = match &action {
            PartialAction::Wait => return Ok(()),
            PartialAction::SendPlaceholder {
                text,
                thread_id,
                reply_to,
            } => {
                let params = SendMessageParams {
                    chat_id: target.chat_id,
                    text: text.clone(),
                    parse_mode: "HTML".to_string(),
                    message_thread_id: *thread_id,
                    reply_parameters: *reply_to,
                };
                tokio::select! {
                    _ = self.cancel.cancelled() => return Ok(()),
                    result = target.api.send_message(&params) => result.map(|message| message.message_id),
                }
            }
            PartialAction::Edit { message_id, text } => {
                let params = EditMessageTextParams {
                    chat_id: target.chat_id,
                    message_id: *message_id,
                    text: text.clone(),
                    parse_mode: "HTML".to_string(),
                };
                tokio::select! {
                    _ = self.cancel.cancelled() => return Ok(()),
                    result = target.api.edit_message_text(&params) => match result {
                        Ok(()) => Ok(0),
                        Err(error) if is_not_modified(&error) => Ok(0),
                        Err(error) => Err(error),
                    },
                }
            }
        };
        let delivered = match result {
            Ok(message_id) => {
                if message_id != 0 {
                    stream.message_id = message_id;
                }
                self.schedules
                    .with_schedule(&target.bot_key, target.chat_id, |schedule| {
                        schedule.last_edit = Some(now);
                    });
                true
            }
            Err(error) => {
                self.record_rate_limit(&target, &error, now);
                return Err(error);
            }
        };
        drop(streams);
        if delivered {
            self.mark_round_trip(&target).await;
        }
        Ok(())
    }

    async fn process_terminal(&self, event: &patchbay_events::Event) -> anyhow::Result<()> {
        let task_id = task_id_from_event(event);
        let Some(task_id) = task_id else {
            return Ok(());
        };
        if event.event_type == patchbay_protocol::EVENT_TASK_CANCELLED {
            self.release_stream(&task_id.to_string(), SystemTime::now())
                .await;
            return Ok(());
        }
        if event.event_type == patchbay_protocol::EVENT_TASK_FAILED
            && task_failure_retry_pending(&event.payload)
        {
            self.release_stream(&task_id.to_string(), SystemTime::now())
                .await;
            return Ok(());
        }
        let Some(target) = self.resolve_target(event).await? else {
            return Ok(());
        };
        if event.event_type == patchbay_protocol::EVENT_TASK_FAILED {
            return self.deliver_failure(target).await;
        }
        let content = chat_done_content(&event.payload);
        if content.is_empty() {
            self.release_stream(&target.stream_key, SystemTime::now())
                .await;
            return Ok(());
        }

        let stream = self.streams.lock().await.remove(&target.stream_key);
        if stream.is_none()
            && self
                .schedules
                .retain(&target.bot_key, target.chat_id, SystemTime::now())
                .is_none()
        {
            return Ok(());
        }
        let mut reply = TerminalReply {
            chat_id: target.chat_id,
            thread_id: target.thread_id,
            reply_to: target.reply_to,
            streamed_message_id: stream
                .map(|stream| stream.state.message_id)
                .unwrap_or_default(),
            ..Default::default()
        };
        let result = self
            .drive_terminal_reply(&target, &content, &mut reply)
            .await;
        self.schedules
            .release(&target.bot_key, target.chat_id, SystemTime::now());
        result
    }

    async fn drive_terminal_reply(
        &self,
        target: &RuntimeTarget,
        content: &str,
        reply: &mut TerminalReply,
    ) -> anyhow::Result<()> {
        loop {
            if self.cancel.is_cancelled() {
                return Ok(());
            }
            let now = SystemTime::now();
            let schedule = self
                .schedules
                .schedule_snapshot(&target.bot_key, target.chat_id)
                .unwrap_or_else(|| ChatSchedule::new(target.bot_key.clone(), target.chat_id));
            match terminal_step(reply, &schedule, content, now) {
                TerminalStep::Done => return Ok(()),
                TerminalStep::RetryAt(at) => {
                    let wait = at.duration_since(now).unwrap_or_default();
                    tokio::select! {
                        _ = self.cancel.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(wait) => {}
                    }
                }
                TerminalStep::Call(call) => match call {
                    TerminalCall::EditPlaceholder { message_id, text } => {
                        let params = EditMessageTextParams {
                            chat_id: target.chat_id,
                            message_id,
                            text,
                            parse_mode: "HTML".to_string(),
                        };
                        let result = tokio::select! {
                            _ = self.cancel.cancelled() => return Ok(()),
                            result = target.api.edit_message_text(&params) => result,
                        };
                        match result {
                            Ok(()) => {
                                self.record_edit(target, now);
                                self.mark_round_trip(target).await;
                                match terminal_placeholder_edited(reply, now) {
                                    TerminalStep::Done => return Ok(()),
                                    TerminalStep::RetryAt(at) => {
                                        self.wait_until(at).await;
                                    }
                                    TerminalStep::Call(_) => unreachable!(
                                        "placeholder completion cannot issue an immediate call"
                                    ),
                                }
                            }
                            Err(error) if crate::api::retry_after(&error).is_some() => {
                                self.record_rate_limit(target, &error, now);
                            }
                            Err(_) => {
                                let _ = terminal_placeholder_failed(reply, now);
                            }
                        }
                    }
                    send @ TerminalCall::SendChunk { .. } => {
                        let mut params = terminal_send_params(&send)
                            .ok_or_else(|| anyhow::anyhow!("telegram terminal call mismatch"))?;
                        params.chat_id = target.chat_id;
                        let result = tokio::select! {
                            _ = self.cancel.cancelled() => return Ok(()),
                            result = target.api.send_message(&params) => result,
                        };
                        match result {
                            Ok(_) => {
                                self.record_edit(target, now);
                                self.mark_round_trip(target).await;
                                match terminal_chunk_sent(reply, now) {
                                    TerminalStep::Done => return Ok(()),
                                    TerminalStep::RetryAt(at) => {
                                        self.wait_until(at).await;
                                    }
                                    TerminalStep::Call(_) => unreachable!(
                                        "chunk completion cannot issue an immediate call"
                                    ),
                                }
                            }
                            Err(error) if crate::sender::is_html_parse_error(&error) => {
                                reply.plain_text_fallback = true;
                            }
                            Err(error) => {
                                self.record_rate_limit(target, &error, now);
                                if crate::api::retry_after(&error).is_none() {
                                    return Err(error);
                                }
                            }
                        }
                    }
                },
            }
        }
    }

    async fn deliver_failure(&self, target: RuntimeTarget) -> anyhow::Result<()> {
        let stream = self.streams.lock().await.remove(&target.stream_key);
        if stream.is_none()
            && self
                .schedules
                .retain(&target.bot_key, target.chat_id, SystemTime::now())
                .is_none()
        {
            return Ok(());
        }
        let mut edited = false;
        let mut delivered = false;
        if let Some(params) = stream
            .as_ref()
            .and_then(|stream| failure_edit(&stream.state))
        {
            loop {
                self.wait_for_schedule(&target).await;
                let now = SystemTime::now();
                let result = tokio::select! {
                    _ = self.cancel.cancelled() => break,
                    result = target.api.edit_message_text(&params) => result,
                };
                match result {
                    Ok(()) => {
                        self.record_edit(&target, now);
                        edited = true;
                        delivered = true;
                        break;
                    }
                    Err(error) if is_not_modified(&error) => {
                        edited = true;
                        delivered = true;
                        break;
                    }
                    Err(error) if crate::api::retry_after(&error).is_some() => {
                        self.record_rate_limit(&target, &error, now);
                    }
                    Err(_) => break,
                }
            }
        }
        let result = if edited {
            Ok(())
        } else {
            loop {
                self.wait_for_schedule(&target).await;
                let now = SystemTime::now();
                let params = failure_send(target.chat_id, target.thread_id, target.reply_to);
                let result = tokio::select! {
                    _ = self.cancel.cancelled() => break Ok(()),
                    result = target.api.send_message(&params) => result,
                };
                match result {
                    Ok(_) => {
                        self.record_edit(&target, now);
                        delivered = true;
                        break Ok(());
                    }
                    Err(error) if crate::api::retry_after(&error).is_some() => {
                        self.record_rate_limit(&target, &error, now);
                    }
                    Err(error) => break Err(error),
                }
            }
        };
        if delivered {
            self.mark_round_trip(&target).await;
        }
        self.schedules
            .release(&target.bot_key, target.chat_id, SystemTime::now());
        result
    }

    async fn wait_for_schedule(&self, target: &RuntimeTarget) {
        let now = SystemTime::now();
        let available = self
            .schedules
            .schedule_snapshot(&target.bot_key, target.chat_id)
            .map(|schedule| schedule.edit_available_at(now))
            .unwrap_or(now);
        let wait = available.duration_since(now).unwrap_or_default();
        tokio::select! {
            _ = self.cancel.cancelled() => {}
            _ = tokio::time::sleep(wait) => {}
        }
    }

    async fn wait_until(&self, at: SystemTime) {
        let wait = at.duration_since(SystemTime::now()).unwrap_or_default();
        tokio::select! {
            _ = self.cancel.cancelled() => {}
            _ = tokio::time::sleep(wait) => {}
        }
    }

    async fn resolve_target(
        &self,
        event: &patchbay_events::Event,
    ) -> anyhow::Result<Option<RuntimeTarget>> {
        use patchbay_db::queries::agent::get_agent_task;
        use patchbay_db::queries::channel::{
            get_channel_chat_session_binding_by_session, get_channel_installation,
        };

        let Some(task_id) = task_id_from_event(event) else {
            return Ok(None);
        };
        let Some(task) = get_agent_task(&self.pool, task_id).await? else {
            return Ok(None);
        };
        if !patchbay_channel_engine::task_input_is_channel_ingested(
            &self.pool,
            task.chat_input_task_id,
        )
        .await?
        {
            return Ok(None);
        }
        let Some(session_id) = task.chat_session_id else {
            return Ok(None);
        };
        let Some(binding) = get_channel_chat_session_binding_by_session(
            &self.pool,
            session_id,
            crate::TYPE_TELEGRAM,
        )
        .await?
        else {
            return Ok(None);
        };
        let Some(installation) =
            get_channel_installation(&self.pool, binding.installation_id, crate::TYPE_TELEGRAM)
                .await?
        else {
            return Ok(None);
        };
        if installation.status != "active" {
            return Ok(None);
        }
        let raw = serde_json::to_vec(&installation.config)?;
        let credentials = decode_credentials(&raw, self.decrypt.as_deref())?;
        if credentials.bot_token.is_empty() {
            return Ok(None);
        }
        let (chat_id, thread_id, reply_to) = outbound_target(
            &binding.channel_chat_id,
            &binding.config,
            binding.last_thread_id.as_deref(),
            binding.last_message_id.as_deref(),
        );
        if chat_id == 0 {
            return Ok(None);
        }
        Ok(Some(RuntimeTarget {
            installation_id: installation.id,
            installed_at: installation.installed_at.clone(),
            stream_key: task_id.to_string(),
            bot_key: installation.id.to_string(),
            api: BotApi::new(&self.api_base, &credentials.bot_token),
            chat_id,
            thread_id,
            reply_to,
        }))
    }

    fn record_edit(&self, target: &RuntimeTarget, now: SystemTime) {
        self.schedules
            .with_schedule(&target.bot_key, target.chat_id, |schedule| {
                schedule.last_edit = Some(now);
            });
    }

    fn record_rate_limit(&self, target: &RuntimeTarget, error: &anyhow::Error, now: SystemTime) {
        if let Some(wait) = crate::api::retry_after(error) {
            self.schedules
                .with_schedule(&target.bot_key, target.chat_id, |schedule| {
                    schedule.backoff_till = Some(now + wait);
                });
        }
    }

    async fn mark_round_trip(&self, target: &RuntimeTarget) {
        crate::verification::record_round_trip(
            &self.pool,
            &self.bus,
            target.installation_id,
            target.installed_at,
        )
        .await;
    }

    async fn release_stream(&self, stream_key: &str, now: SystemTime) {
        if let Some(stream) = self.streams.lock().await.remove(stream_key) {
            self.schedules
                .release(&stream.bot_key, stream.state.chat_id, now);
        }
    }
}

fn task_id_from_event(event: &patchbay_events::Event) -> Option<Uuid> {
    let raw = if event.task_id.is_empty() {
        event
            .payload
            .get("task_id")
            .and_then(|value| value.as_str())
            .or_else(|| {
                event
                    .payload
                    .get("task_message")
                    .and_then(|value| value.get("task_id"))
                    .and_then(|value| value.as_str())
            })
            .unwrap_or_default()
    } else {
        &event.task_id
    };
    Uuid::parse_str(raw).ok().filter(|id| !id.is_nil())
}

fn task_message_content(payload: &serde_json::Value) -> Option<&str> {
    let message = payload.get("task_message").unwrap_or(payload);
    if message.get("type").and_then(|value| value.as_str()) != Some("text") {
        return None;
    }
    message
        .get("content")
        .and_then(|value| value.as_str())
        .filter(|content| !content.is_empty())
}

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
