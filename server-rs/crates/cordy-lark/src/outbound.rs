//! The task-lifecycle outbound patcher.
//!
//! Reacts to task-lifecycle events on the event bus and forwards chat replies
//! to Lark. The original "thinking → streaming → final card" lifecycle was
//! reduced to a single plain-text reply on chat:done after the card chrome made
//! replies feel like system notifications. The error path is the one survivor
//! of card rendering: failed runs surface as a short error card on task:failed
//! because the visual distinction from a normal reply is genuinely useful.
//!
//! Scope:
//!
//!   - Only tasks whose chat_session has a lark_chat_session_binding produce
//!     outbound. Tasks born from the web UI or autopilot pass through
//!     unchanged.
//!   - Each chat:done yields one Lark text message; there is no streaming, no
//!     throttling, no DB row to track card-state.
//!   - Multi-replica safety is inherited from the inbound WS lease: at most one
//!     replica holds the installation lease at a time, the event bus is
//!     per-process, so exactly one Patcher reacts per run.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use cordy_channel::RuntimeTasks;
use cordy_channel_engine::provenance::task_input_is_channel_ingested;
use cordy_db::queries::agent::{get_agent, get_agent_task};
use cordy_events::{Bus, Event};
use tokio_util::sync::CancellationToken;

use crate::client::{
    ApiClient, InstallationCredentials, ReplyTarget, SendCardParams, SendMarkdownCardParams,
    SendTextParams,
};
use crate::installation::CredentialsResolver;
use crate::markdown_detect::contains_markdown;
use crate::store::{ChatSessionBinding, Installation};
use crate::types::ChatId;

/// Bounds one event's delivery (Go used a 10s context): bus delivery would
/// otherwise let a stuck Lark HTTP call wedge the whole publish call site.
pub const EVENT_BUDGET: Duration = Duration::from_secs(10);

// ---- card rendering ----

/// The small set of card variants the patcher renders. The Renderer is
/// plug-replaceable so the on-wire card template can evolve without touching
/// the patcher's transport / DB logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    Thinking,
    Running,
    Final,
    Error,
}

/// The typed snapshot the Renderer sees when building or patching a card.
/// Fields are populated as they become available during a task lifecycle —
/// issue_number for /issue flows, content for completed chat tasks,
/// error_message for failed ones.
#[derive(Debug, Clone, Default)]
pub struct RenderInput {
    pub kind: Option<CardKind>,
    pub agent_name: String,
    pub issue_number: i32,
    pub issue_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub content: String,
    pub error_message: String,
}

/// Turns a typed RenderInput into the actual Lark card JSON. Centralizing this
/// lets us swap card templates (or A/B them) without touching event
/// subscription or persistence code.
pub trait CardRenderer: Send + Sync {
    fn render(&self, input: &RenderInput) -> anyhow::Result<String>;
}

/// Produces minimal text-only cards that work against Lark's generic
/// interactive-card schema. The exact JSON layout will be refined when the real
/// product card design lands; this default keeps the wiring real without
/// committing the product to a particular template.
pub struct DefaultRenderer;

impl CardRenderer for DefaultRenderer {
    fn render(&self, input: &RenderInput) -> anyhow::Result<String> {
        let header = if input.agent_name.is_empty() {
            "Cordy"
        } else {
            &input.agent_name
        };
        let body = match input.kind {
            Some(CardKind::Thinking) => "Thinking…".to_string(),
            Some(CardKind::Running) => "Working on it…".to_string(),
            Some(CardKind::Final) => {
                let body = input.content.clone();
                if body.is_empty() {
                    "Done.".to_string()
                } else {
                    body
                }
            }
            Some(CardKind::Error) => {
                let mut body = "Run failed.".to_string();
                if !input.error_message.is_empty() {
                    body = format!("Run failed: {}", input.error_message);
                }
                body
            }
            None => anyhow::bail!("unknown card kind"),
        };
        // update_multi MUST be true on every render: Lark refuses to apply
        // PatchInteractiveCard to a card whose config does not declare it a
        // "shared, updatable" card. Since this renderer drives the thinking →
        // streaming → final/error lifecycle (the card is sent once and patched
        // multiple times), an absent update_multi causes every patch after the
        // first send to silently no-op on the Lark side while the local
        // outbound status row still flips to streaming/final. Keep this on
        // every kind — including thinking and error — because that initial
        // JSON IS the body Lark stores and consults for subsequent patches.
        let doc = serde_json::json!({
            "config": {
                "wide_screen_mode": true,
                "update_multi": true,
            },
            "header": {
                "template": "blue",
                "title": {"tag": "plain_text", "content": header},
            },
            "elements": [
                {"tag": "div", "text": {"tag": "plain_text", "content": body}},
            ],
        });
        Ok(serde_json::to_string(&doc)?)
    }
}

pub fn new_default_renderer() -> Arc<dyn CardRenderer> {
    Arc::new(DefaultRenderer)
}

// ---- credentials ----

fn installation_credentials(
    creds: Option<&Arc<dyn CredentialsResolver>>,
    inst: &Installation,
) -> anyhow::Result<InstallationCredentials> {
    let Some(resolver) = creds else {
        anyhow::bail!("lark patcher: credentials resolver missing");
    };
    crate::installation::installation_credentials_for(resolver.as_ref(), inst)
        .map_err(|e| anyhow::anyhow!("decrypt app_secret: {e:#}"))
}

// ---- shared send helpers ----

/// Derives the outbound reply target from the chat binding's most-recent
/// inbound trigger. We thread the reply ONLY when that trigger was itself
/// inside a Lark topic (last_lark_thread_id present): normal group / p2p chats
/// keep the unchanged chat-level send path, and only an @-mention that happened
/// inside a thread gets a threaded reply (replying to last_lark_message_id with
/// reply_in_thread). The zero ReplyTarget means "send at the chat level".
pub fn thread_reply_target(binding: &ChatSessionBinding) -> ReplyTarget {
    match (&binding.last_thread_id, &binding.last_message_id) {
        (Some(t), Some(m)) if !t.is_empty() && !m.is_empty() => ReplyTarget {
            message_id: m.clone(),
            in_thread: true,
        },
        _ => ReplyTarget::default(),
    }
}

/// Threads an outbound reply off the inbound trigger message when that message
/// lived inside a Lark topic (话题). It mirrors threadReplyTarget (used by the
/// event-driven Patcher) but reads the live InboundMessage the replier already
/// holds, so it needs no DB round-trip. An empty thread_id yields the zero
/// ReplyTarget — a chat-level send, i.e. the unchanged behavior for non-thread
/// messages.
pub fn inbound_reply_target(msg: &crate::feishu_types::InboundMessage) -> ReplyTarget {
    if !msg.thread_id.is_empty() && !msg.message_id.is_empty() {
        return ReplyTarget {
            message_id: msg.message_id.clone(),
            in_thread: true,
        };
    }
    ReplyTarget::default()
}

/// Runs `send` with the thread reply target and, ONLY when the threaded attempt
/// fails with a Lark error that means the topic reply legitimately cannot land
/// (trigger message recalled, topic gone, topics disabled, aggregated message —
/// see [`crate::http_client::is_thread_reply_unsupported`]), retries once at
/// the chat level so the reply is not silently lost. Any other failure —
/// transport error, 5xx, timeout, rate limit, or an ambiguous "the server may
/// have received it" error — is logged and returned as a failure rather than
/// retried: a blind chat-level retry could duplicate the reply or leak a
/// thread-only reply into the main group chat. When target is already
/// chat-level there is nothing to fall back to and the error is returned.
///
/// It is a free function (rather than a method on one consumer) so the
/// event-driven Patcher and the immediate OutcomeReplier share one classified
/// fallback path.
pub async fn send_with_thread_fallback<F, Fut>(
    op: &str,
    target: ReplyTarget,
    send: F,
) -> anyhow::Result<()>
where
    F: Fn(ReplyTarget) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    match send(target.clone()).await {
        Ok(()) => Ok(()),
        Err(err) => {
            if target.is_set() && crate::http_client::is_thread_reply_unsupported(&err) {
                tracing::warn!(
                    op = %op,
                    reply_message_id = %target.message_id,
                    error = %err,
                    "lark: thread reply unsupported for target, retrying at chat level"
                );
                return match send(ReplyTarget::default()).await {
                    Ok(()) => Ok(()),
                    Err(fallback) => Err(anyhow::anyhow!(
                        "{op} (chat-level fallback after thread-unsupported reply: {err:#}): {fallback:#}"
                    )),
                };
            }
            if target.is_set() {
                tracing::warn!(
                    op = %op,
                    reply_message_id = %target.message_id,
                    error = %err,
                    "lark: thread reply failed; not falling back (non-classified error)"
                );
            }
            Err(anyhow::anyhow!("{op}: {err:#}"))
        }
    }
}

// ---- payload extraction ----

/// Parses the typed-ish payload the task publishers emit — a JSON object with
/// `task_id` (always) and `chat_session_id` (chat tasks only). chat:done
/// carries a structured ChatDonePayload that serializes to the same shape.
/// Prefers the envelope hints, then falls back into the payload map.
fn task_and_session_from_event(e: &Event) -> (Option<Uuid>, Option<Uuid>) {
    let parse = |s: &str| Uuid::parse_str(s).ok().filter(|u| !u.is_nil());
    let mut task_id = parse(&e.task_id);
    let mut chat_session_id = parse(&e.chat_session_id);
    if task_id.is_none() {
        task_id = e
            .payload
            .get("task_id")
            .and_then(Value::as_str)
            .and_then(parse);
    }
    if chat_session_id.is_none() {
        chat_session_id = e
            .payload
            .get("chat_session_id")
            .and_then(Value::as_str)
            .and_then(parse);
    }
    (task_id, chat_session_id)
}

/// Extracts the reply text from a chat:done payload.
pub(crate) fn chat_done_content(payload: &Value) -> String {
    payload
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Recovers the failure reason from a task:failed payload (`error` preferred,
/// then `error_message`).
pub(crate) fn error_message_from_payload(payload: &Value) -> String {
    payload
        .get("error")
        .or_else(|| payload.get("error_message"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Recovers the real Lark chat id from the chat binding. The channel_chat_id
/// may be a composite "chat:thread" topic-isolation key, so the real chat id is
/// read from the binding config ([`crate::resolvers::LarkBindingConfig`]);
/// pre-topic rows (config "{}") route by the key itself, which for them IS the
/// real chat id.
pub(crate) fn outbound_chat_id(b: &ChatSessionBinding) -> ChatId {
    if !b.config.is_null() && b.config != serde_json::json!({}) {
        if let Ok(cfg) =
            serde_json::from_value::<crate::resolvers::LarkBindingConfig>(b.config.clone())
        {
            if !cfg.chat_id.is_empty() {
                return ChatId(cfg.chat_id);
            }
        }
    }
    ChatId(b.channel_chat_id.clone())
}

// ---- the Patcher ----

/// Tunes the outbound Patcher. Defaults via `with_defaults`; tests typically
/// override renderer.
#[derive(Clone, Default)]
pub struct PatcherConfig {
    /// Drives the error card template used on the task:failed path. The
    /// success path (chat:done) bypasses the renderer entirely — it sends the
    /// raw assistant reply as a plain text IM message — so this only matters
    /// for the failure branch.
    pub renderer: Option<Arc<dyn CardRenderer>>,
}

impl PatcherConfig {
    fn renderer_or_default(&self) -> Arc<dyn CardRenderer> {
        self.renderer.clone().unwrap_or_else(new_default_renderer)
    }
}

/// Reacts to task-lifecycle events on the event bus and forwards chat replies
/// to Lark as plain text IM messages. Constructed once at boot; register it on
/// the bus exactly once during server startup.
pub struct LarkPatcher {
    pool: PgPool,
    credentials: Option<Arc<dyn CredentialsResolver>>,
    client: Arc<dyn ApiClient>,
    typing_indicator: RwLock<Option<Arc<crate::typing_indicator::TypingIndicatorManager>>>,
    cfg: PatcherConfig,
}

impl LarkPatcher {
    /// Constructs a Patcher bound to its dependencies. The patcher does not
    /// subscribe to the bus until register is called.
    pub fn new(
        pool: PgPool,
        credentials: Option<Arc<dyn CredentialsResolver>>,
        client: Arc<dyn ApiClient>,
        cfg: PatcherConfig,
    ) -> Self {
        Self {
            pool,
            credentials,
            client,
            typing_indicator: RwLock::new(None),
            cfg,
        }
    }

    /// Wires the typing-indicator manager into the patcher so that replies
    /// clear the "processing" reaction before they are sent. Call once at boot
    /// after both the patcher and manager are constructed. None disables the
    /// clear step.
    pub fn set_typing_indicator_manager(
        &self,
        m: Option<Arc<crate::typing_indicator::TypingIndicatorManager>>,
    ) {
        if let Ok(mut slot) = self.typing_indicator.write() {
            *slot = m;
        }
    }

    fn typing(&self) -> Option<Arc<crate::typing_indicator::TypingIndicatorManager>> {
        self.typing_indicator.read().ok().and_then(|s| s.clone())
    }

    /// Subscribes the patcher to the task-lifecycle events it cares about on
    /// the supplied bus. Call sites should invoke it exactly once during server
    /// boot (after the bus + patcher are constructed and before HTTP traffic
    /// starts).
    ///
    /// Subscriptions are deliberately minimal:
    ///
    ///   - chat:done — the agent finished replying. Sent as a plain text IM
    ///     message (Lark `msg_type=text`), not as an interactive card. The
    ///     earlier card-based design made every reply look like a system
    ///     notification nested in card chrome; flipping to plain text makes
    ///     free-form chat feel native.
    ///   - task:failed — the run failed; surface a short error card so the
    ///     failure is visually distinct from a successful reply.
    ///   - task:cancelled — the run ended without an answer. Nothing is sent;
    ///     the subscription exists so the Typing reaction comes off. A
    ///     cancellation publishes no chat-done and no task-failed, so without
    ///     this the badge sits on the user's message for good.
    ///
    /// We deliberately do NOT subscribe to task:queued / task:running (no
    /// thinking-card lifecycle anymore — adds noise without value) or to
    /// task:completed (chat tasks always emit chat:done first, which is what we
    /// care about; non-chat tasks have no Lark binding anyway and would
    /// early-return). Leaving task:completed unsubscribed also avoids the prior
    /// "Done." overwrite regression where the no-content payload would wipe the
    /// real reply.
    pub fn register(self: &Arc<Self>, bus: &Bus, tasks: Arc<RuntimeTasks>) {
        for event_type in [
            cordy_protocol::EVENT_TASK_FAILED,
            cordy_protocol::EVENT_CHAT_DONE,
            cordy_protocol::EVENT_TASK_CANCELLED,
        ] {
            let me = Arc::clone(self);
            let tasks = tasks.clone();
            bus.subscribe(event_type, move |e: &Event| {
                // Use a fresh budgeted context: bus delivery is synchronous so
                // a stuck Lark HTTP call would otherwise wedge the whole
                // publish call site. Run detached like every bus subscriber in
                // this workspace.
                let me = Arc::clone(&me);
                let e = e.clone();
                tasks.spawn(async move {
                    let ctx = CancellationToken::new();
                    match tokio::time::timeout(EVENT_BUDGET, me.process_event(&ctx, &e)).await {
                        Err(_) => tracing::warn!(
                            event_type = %e.event_type,
                            task_id = %e.task_id,
                            chat_session_id = %e.chat_session_id,
                            "lark patcher: event handling timed out"
                        ),
                        Ok(Err(err)) => tracing::warn!(
                            event_type = %e.event_type,
                            task_id = %e.task_id,
                            chat_session_id = %e.chat_session_id,
                            error = %err,
                            "lark patcher: event handling failed"
                        ),
                        Ok(Ok(())) => {}
                    }
                });
            });
        }
    }

    async fn process_event(&self, ctx: &CancellationToken, e: &Event) -> anyhow::Result<()> {
        let (Some(task_id), chat_session_id) = task_and_session_from_event(e) else {
            return Ok(());
        };
        let Some(chat_session_id) = chat_session_id else {
            // Issue / autopilot tasks have no chat_session.
            return Ok(());
        };

        // A cancelled run has no reply to place, so the only thing owed to the
        // user is taking the Typing badge off. That runs before every lookup
        // below, because each of them can answer "no" for a run that still has
        // a badge on screen:
        //
        //   - the binding is gone by the time a session delete's cancels are
        //     broadcast (they fire after the transaction that dropped it
        //     commits);
        //   - the origin classification answers "does this answer belong on
        //     Lark", and a task cancelled for owning an empty input batch — the
        //     failure #6611 fixed the cause of — reports no channel-ingested
        //     messages, so a clear behind it would be skipped on exactly the
        //     run that most needs it. A cancellation has no answer to misroute,
        //     so the question does not arise.
        //
        // Nothing is posted here, so neither gate is protecting anything: the
        // badge is Lark's own, and clear only touches sessions this process put
        // one on. The clear is keyed by session rather than by turn, so
        // cancelling one of two turns in a session takes the badge off both;
        // the worst that costs is a missing badge on a turn still running.
        if e.event_type == cordy_protocol::EVENT_TASK_CANCELLED {
            if let Some(typing) = self.typing() {
                typing.clear(ctx.clone(), chat_session_id).await;
            }
            return Ok(());
        }

        let binding = match cordy_db::queries::channel::get_channel_chat_session_binding_by_session(
            &self.pool,
            chat_session_id,
            crate::channel_store::CHANNEL_TYPE_FEISHU,
        )
        .await?
        {
            Some(row) => chat_session_binding_from_row(row),
            None => return Ok(()), // web-only chat session — not a Lark target
        };

        // Only bound sessions reach here, so classify the task origin before
        // spending any send work. Web/mobile direct-chat tasks can reuse a
        // session that originated in Lark, but their replies belong only in
        // Cordy. Sealed channel tasks own an input batch just like direct
        // tasks, so the discriminator is the immutable channel_ingested
        // provenance of that batch, not chat_input_task_id presence (which
        // #5645 originally used).
        let task = get_agent_task(&self.pool, task_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("load agent task: row missing"))?;
        let deliver = task_input_is_channel_ingested(&self.pool, task.chat_input_task_id).await?;
        if !deliver {
            return Ok(());
        }

        let inst = get_lark_installation(&self.pool, binding.installation_id).await?;
        if !crate::types::InstallationStatus(inst.status.clone()).is_active() {
            // Revoked between trigger and event; nothing to patch.
            return Ok(());
        }
        let creds = installation_credentials(self.credentials.as_ref(), &inst)?;

        let agent_name = match get_agent(&self.pool, inst.agent_id).await {
            Ok(Some(agent)) => agent.name,
            _ => String::new(),
        };

        // Clear the "processing" reaction before the reply is visible so the
        // user sees a clean transition. Best-effort: a failure here is logged
        // but does not block the actual reply.
        if let Some(typing) = self.typing() {
            typing.clear(ctx.clone(), chat_session_id).await;
        }

        match e.event_type.as_str() {
            cordy_protocol::EVENT_CHAT_DONE => {
                self.send_chat_reply(ctx, &creds, &binding, &e.payload)
                    .await
            }
            cordy_protocol::EVENT_TASK_FAILED => {
                self.fail(ctx, &creds, &binding, task_id, &agent_name, &e.payload)
                    .await
            }
            _ => Ok(()),
        }
    }

    /// Turns chat:done payload.content into a Lark message. The wire shape is
    /// chosen per-reply based on whether the body contains any markdown syntax:
    ///
    ///   - Plain prose (no markdown) → `msg_type=text`. A one-line "Hi!" reply
    ///     should feel like a normal IM message, not a notification card with
    ///     chrome around it.
    ///   - Anything with markdown → schema-2.0 interactive card with a
    ///     `tag: "markdown"` body element so Lark's client renders the
    ///     formatting instead of leaving raw `**bold**` characters in the
    ///     transcript.
    ///
    /// Empty content is silently dropped: we'd rather show nothing than "Done."
    async fn send_chat_reply(
        &self,
        _ctx: &CancellationToken,
        creds: &InstallationCredentials,
        binding: &ChatSessionBinding,
        payload: &Value,
    ) -> anyhow::Result<()> {
        let content = chat_done_content(payload);
        if content.is_empty() {
            return Ok(());
        }
        let target = thread_reply_target(binding);
        let chat_id = outbound_chat_id(binding);
        if contains_markdown(&content) {
            return send_with_thread_fallback("send markdown card", target, |t| {
                let client = Arc::clone(&self.client);
                let creds = creds.clone();
                let chat_id = chat_id.clone();
                let content = content.clone();
                async move {
                    client
                        .send_markdown_card(SendMarkdownCardParams {
                            installation_id: creds,
                            chat_id,
                            markdown: content,
                            summary: String::new(),
                            reply_target: t,
                        })
                        .await
                        .map(|_| ())
                        .map_err(|e| anyhow::anyhow!("{e:#}"))
                }
            })
            .await;
        }
        send_with_thread_fallback("send text message", target, |t| {
            let client = Arc::clone(&self.client);
            let creds = creds.clone();
            let chat_id = chat_id.clone();
            let content = content.clone();
            async move {
                client
                    .send_text_message(SendTextParams {
                        installation_id: creds,
                        chat_id,
                        text: content,
                        reply_target: t,
                    })
                    .await
                    .map(|_| ())
                    .map_err(|e| anyhow::anyhow!("{e:#}"))
            }
        })
        .await
    }

    /// Surfaces a short error card on task failure. Unlike the success path
    /// (plain text), failures stay as cards because the user benefits from the
    /// visual distinction — a red / header-styled card is much harder to miss,
    /// and these are rare enough that the card chrome isn't noisy.
    ///
    /// One-shot send (no patching, no DB row): if the task fails a second time
    /// we'd just send a second card, which is fine — failure is usually a
    /// single terminal event.
    async fn fail(
        &self,
        _ctx: &CancellationToken,
        creds: &InstallationCredentials,
        binding: &ChatSessionBinding,
        task_id: Uuid,
        agent_name: &str,
        payload: &Value,
    ) -> anyhow::Result<()> {
        let render = self
            .cfg
            .renderer_or_default()
            .render(&RenderInput {
                kind: Some(CardKind::Error),
                agent_name: agent_name.to_string(),
                task_id: Some(task_id),
                error_message: error_message_from_payload(payload),
                ..Default::default()
            })
            .map_err(|e| anyhow::anyhow!("render error card: {e:#}"))?;
        let target = thread_reply_target(binding);
        let chat_id = outbound_chat_id(binding);
        send_with_thread_fallback("send error card", target, |t| {
            let client = Arc::clone(&self.client);
            let creds = creds.clone();
            let chat_id = chat_id.clone();
            let render = render.clone();
            async move {
                client
                    .send_interactive_card(SendCardParams {
                        installation_id: creds,
                        chat_id,
                        card_json: render,
                        reply_target: t,
                    })
                    .await
                    .map(|_| ())
                    .map_err(|e| anyhow::anyhow!("{e:#}"))
            }
        })
        .await
    }
}

// ---- DB helpers kept local so the patcher owns its narrow queries ----

async fn get_lark_installation(pool: &PgPool, id: Uuid) -> anyhow::Result<Installation> {
    let row = cordy_db::queries::channel::get_channel_installation(
        pool,
        id,
        crate::channel_store::CHANNEL_TYPE_FEISHU,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("load installation: row missing"))?;
    crate::store::installation_from_row(row)
}

fn chat_session_binding_from_row(
    row: cordy_db::models::ChannelChatSessionBinding,
) -> ChatSessionBinding {
    ChatSessionBinding {
        id: row.id,
        chat_session_id: row.chat_session_id,
        installation_id: row.installation_id,
        channel_chat_id: row.channel_chat_id,
        chat_type: row.chat_type,
        config: row.config,
        created_at: row.created_at,
        last_message_id: row.last_message_id,
        last_thread_id: row.last_thread_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_renderer_covers_kinds_and_pins_update_multi() {
        let r = DefaultRenderer;
        let base = RenderInput {
            agent_name: "Atlas".to_string(),
            ..Default::default()
        };

        let thinking = r
            .render(&RenderInput {
                kind: Some(CardKind::Thinking),
                ..base.clone()
            })
            .unwrap();
        assert!(thinking.contains("\"update_multi\":true"), "{thinking}");
        assert!(thinking.contains("Thinking…"));

        let final_empty = r
            .render(&RenderInput {
                kind: Some(CardKind::Final),
                ..base.clone()
            })
            .unwrap();
        assert!(final_empty.contains("Done."));

        let final_content = r
            .render(&RenderInput {
                kind: Some(CardKind::Final),
                content: "hello world".to_string(),
                ..base.clone()
            })
            .unwrap();
        assert!(final_content.contains("hello world"));
        assert!(!final_content.contains("Done."));

        let err_plain = r
            .render(&RenderInput {
                kind: Some(CardKind::Error),
                ..base.clone()
            })
            .unwrap();
        assert!(err_plain.contains("Run failed."));

        let err_detail = r
            .render(&RenderInput {
                kind: Some(CardKind::Error),
                error_message: "boom".to_string(),
                ..base.clone()
            })
            .unwrap();
        assert!(err_detail.contains("Run failed: boom"));

        // Unknown kind errors instead of guessing.
        assert!(r.render(&RenderInput { kind: None, ..base }).is_err());

        // Header falls back to "Cordy" without an agent name.
        let anonymous = r
            .render(&RenderInput {
                kind: Some(CardKind::Running),
                ..Default::default()
            })
            .unwrap();
        assert!(anonymous.contains("Cordy"));
        assert!(anonymous.contains("Working on it…"));
    }

    #[test]
    fn thread_target_requires_both_ids() {
        let mk = |last_msg: Option<String>, last_thread: Option<String>| ChatSessionBinding {
            last_message_id: last_msg,
            last_thread_id: last_thread,
            ..Default::default()
        };
        assert!(!thread_reply_target(&mk(Some("om1".into()), None)).is_set());
        assert!(!thread_reply_target(&mk(None, Some("t1".into()))).is_set());
        assert!(!thread_reply_target(&mk(Some(String::new()), Some("t1".into()))).is_set());
        let threaded = thread_reply_target(&mk(Some("om1".into()), Some("t1".into())));
        assert!(threaded.is_set());
        assert_eq!(threaded.message_id, "om1");
        assert!(threaded.in_thread);
    }

    #[test]
    fn inbound_target_threads_only_full_topics() {
        use crate::feishu_types::InboundMessage;
        let zero = inbound_reply_target(&InboundMessage::default());
        assert!(!zero.is_set());
        let partial = inbound_reply_target(&InboundMessage {
            thread_id: "t".to_string(),
            ..Default::default()
        });
        assert!(!partial.is_set());
        let full = inbound_reply_target(&InboundMessage {
            thread_id: "t".to_string(),
            message_id: "om".to_string(),
            ..Default::default()
        });
        assert!(full.is_set());
        assert_eq!(full.message_id, "om");
    }

    #[test]
    fn chat_id_prefers_config_then_falls_back_to_key() {
        let mk = |channel_chat_id: &str, cfg: Value| ChatSessionBinding {
            channel_chat_id: channel_chat_id.to_string(),
            config: cfg,
            ..Default::default()
        };
        // Topic-composite key routes by the embedded real chat id.
        assert_eq!(
            outbound_chat_id(&mk("oc1:t9", serde_json::json!({"chat_id": "oc1"}))).0,
            "oc1"
        );
        // Pre-topic rows carry {} and route by the key itself.
        assert_eq!(outbound_chat_id(&mk("oc2", serde_json::json!({}))).0, "oc2");
        // Null / absent config behaves like {}.
        assert_eq!(outbound_chat_id(&mk("oc3", Value::Null)).0, "oc3");
        // Malformed config degrades to the key.
        assert_eq!(outbound_chat_id(&mk("oc4", serde_json::json!(7))).0, "oc4");
    }

    #[test]
    fn payload_extractors_cover_map_shapes() {
        assert_eq!(
            chat_done_content(&serde_json::json!({"content": "hi"})),
            "hi"
        );
        assert_eq!(chat_done_content(&serde_json::json!({})), "");
        assert_eq!(chat_done_content(&Value::Null), "");

        assert_eq!(
            error_message_from_payload(&serde_json::json!({"error": "e1"})),
            "e1"
        );
        assert_eq!(
            error_message_from_payload(&serde_json::json!({"error_message": "e2"})),
            "e2"
        );
        // `error` wins over `error_message` when both exist.
        assert_eq!(
            error_message_from_payload(&serde_json::json!({"error": "e1", "error_message": "e2"})),
            "e1"
        );
        assert_eq!(error_message_from_payload(&serde_json::json!({})), "");
    }

    #[test]
    fn event_ids_parse_from_envelope_then_payload_and_fail_closed() {
        let mk = |task_id: &str, session: &str, payload: Value| Event {
            event_type: String::new(),
            workspace_id: String::new(),
            actor_type: String::new(),
            actor_id: String::new(),
            payload,
            task_id: task_id.to_string(),
            chat_session_id: session.to_string(),
        };
        let t = Uuid::now_v7();
        let s = Uuid::now_v7();

        let (tid, sid) = task_and_session_from_event(&mk("", "", serde_json::json!({})));
        assert!(tid.is_none() && sid.is_none());

        // Envelope hints win.
        let (tid, sid) =
            task_and_session_from_event(&mk(&t.to_string(), &s.to_string(), serde_json::json!({})));
        assert_eq!(tid, Some(t));
        assert_eq!(sid, Some(s));

        // Payload fallback fills gaps.
        let (tid, sid) = task_and_session_from_event(&mk(
            "",
            "",
            serde_json::json!({"task_id": t.to_string(), "chat_session_id": s.to_string()}),
        ));
        assert_eq!(tid, Some(t));
        assert_eq!(sid, Some(s));

        // Junk ids read as absent (fail closed).
        let (tid, _) = task_and_session_from_event(&mk("junk", "", serde_json::json!({})));
        assert!(tid.is_none());
        let (tid, _) =
            task_and_session_from_event(&mk(&Uuid::nil().to_string(), "", serde_json::json!({})));
        assert!(tid.is_none(), "nil UUID is not a usable task id");
    }

    #[test]
    fn budget_is_ten_seconds_like_go() {
        assert_eq!(EVENT_BUDGET, Duration::from_secs(10));
    }
}
