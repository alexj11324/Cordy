//! Delivering the files an agent produced. Port of
//! `server/internal/integrations/wecom/outbound_media.go`.
//!
//! The agent's side of this already exists and is platform-agnostic: it runs
//! `cordy attachment upload <path>`, the file lands in object storage, and
//! CompleteTask binds the row to the assistant message it just wrote. What was
//! missing was the last hop. Everything downstream of that bind assumed a chat
//! window in a browser, so a WeCom conversation was told it could not take
//! files at all.
//!
//! Three things decide the shape here.
//!
//! The answer goes first, always. An upload is megabytes and round trips and it
//! can fail; the sentence the agent wrote cannot be made to wait behind one,
//! and must not be lost to one. So this runs after the reply is out, on its own
//! task and its own budget, and its worst outcome is one extra line saying a
//! file did not make it.
//!
//! The file is its own message. The long connection has no msg_item, so nothing
//! can be embedded in a reply — "answer with an attachment" is necessarily two
//! messages.
//!
//! And WeCom validates bytes against the msgtype. A .pptx declared as an image
//! is refused rather than converted, and each kind has its own size ceiling, so
//! what to call a file is a decision and not a lookup.

use std::sync::Arc;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use cordy_channel::RuntimeTasks;
use cordy_db::models::Attachment;
use cordy_db::queries::agent::get_agent_task;
use cordy_db::queries::attachment::list_attachments_by_chat_message;
use cordy_db::queries::channel::{
    get_channel_chat_session_binding_by_session, get_channel_installation,
};
use cordy_events::Event;

use crate::media_upload::{
    MediaMsgType, MediaSend, MediaUploadTooLarge, OutboundMedia, MAX_MEDIA_UPLOAD_BYTES,
};
use crate::outbound_relay::{OutboundRelay, RelayEventHandler};
use crate::senders_registry::SendersRegistry;
use crate::ws_sender::{is_ack_timeout, is_context_cancelled, is_write_attempted, WsSender};

/// The slice of storage.Storage this path needs: the attachment row carries the
/// object's URL, and these two turn it back into bytes.
#[async_trait::async_trait]
pub trait MediaObjectStore: Send + Sync {
    fn key_from_url(&self, raw_url: &str) -> String;
    /// Reads the whole object. Bounded by [`MAX_MEDIA_UPLOAD_BYTES`] + 1 by the
    /// caller's read loop contract.
    async fn get_object(&self, key: &str, max_bytes: usize) -> anyhow::Result<Vec<u8>>;
}

// We know it did not arrive. Definite, because claiming a definite failure that
// later turns out to be a delivery is how a user ends up ignoring the notice.
const MEDIA_SEND_FAILED_TEXT: &str = "⚠️ 有文件没能发出来，我这边保留着，需要的话我再试一次。";

// The frame went out and no verdict came back, so the file may be in the chat
// already. The wording has to survive both endings: it must not say "failed" to
// someone looking at the file, and it must not say "sent" to someone who never
// got it. It also explains why nothing is resent automatically, since that is
// the obvious next question and the answer is that a duplicate cannot be taken
// back.
const MEDIA_SEND_UNKNOWN_TEXT: &str = "⚠️ 有文件我没收到企业微信的送达回执，可能已经发到了、也可能没有。我不会自动重发，免得发重了；你那边没看到的话说一声，我再发一次。";

// The failure is on our side and before the question was even answered: we
// could not read what was attached to this reply, so we do not know whether
// there was a file. Saying nothing here is what leaves a user waiting for
// something that was never attempted.
const MEDIA_LOOKUP_FAILED_TEXT: &str =
    "⚠️ 我这边没查到这条回答带没带文件，所以要是有，这次没发出来。需要的话我再试一次。";

/// Bounds one answer's whole attachment delivery — reading every object,
/// uploading it, and sending it. Generous because a 20 MiB file over forty
/// acked chunks, two at a time, is not fast, and nothing is waiting on it.
const ATTACHMENT_BUDGET: std::time::Duration = std::time::Duration::from_secs(300);

/// What we actually know about one file after trying to send it. Three values,
/// because the two-valued version of this was wrong in both directions at once:
/// a send whose ack never came was reported to the user as a definite failure
/// even though the file may well be sitting in the chat, and the local failures
/// that never reached the socket at all were reported to nobody.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryState {
    /// WeCom acknowledged the send. The only state that needs no message to
    /// the user; the file is what they see.
    Delivered,
    /// Nothing arrived and nothing can have. Either the file never became a
    /// media_id (the upload was refused, or the object could not be read), or
    /// the send itself came back refused. Safe to retry in principle, and safe
    /// to describe as a failure.
    DefinitelyFailed,
    /// The send frame went out and no verdict came back. The message may be in
    /// the chat. This state must never be retried: the same media_id sent twice
    /// shows the person the file twice and there is nothing to undo it with.
    Unknown,
}

impl DeliveryState {
    /// Names the states for the log, in the vocabulary the code reasons in, so
    /// an operator reading a line can tell an unconfirmed send from a refused
    /// one without knowing which errcode meant which.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::DefinitelyFailed => "definitely_failed",
            Self::Unknown => "unknown",
        }
    }
}

/// The per-kind ceilings WeCom applies to uploaded material: 10MB for a photo,
/// 10MB for a video, 2MB for a voice note. Bytes past a kind's ceiling still
/// travel — as a file, which has the widest limit of the four — because a file
/// card the user can open beats a photo the server refused.
const MAX_OUTBOUND_IMAGE_BYTES: usize = 10 << 20;
const MAX_OUTBOUND_VOICE_BYTES: usize = 2 << 20;
const MAX_OUTBOUND_VIDEO_BYTES: usize = 10 << 20;

/// Where one answer's files are going: the installation whose socket carries
/// them, and the conversation at the other end.
#[derive(Debug, Clone)]
pub struct AttachmentTarget {
    pub installation_id: Uuid,
    pub chat_id: String,
    pub chat_type: i64,
}

/// Caps how many attachment deliveries read an object at once.
///
/// Process-wide, not per installation: the heap is process-wide, and a
/// per-installation cap on a deployment running several bots just multiplies.
/// Each delivery holds one object while it chunks it up the socket, so this is
/// the number that decides peak resident attachment bytes.
static ATTACHMENT_SLOTS: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(MAX_CONCURRENT_ATTACHMENT_DELIVERIES);

/// How many objects may be in flight. Small on purpose: each one is up to the
/// platform's file ceiling, and the socket they share is a single long
/// connection per bot, so more concurrency buys queueing rather than
/// throughput.
const MAX_CONCURRENT_ATTACHMENT_DELIVERIES: usize = 2;

/// Bounds the deliveries that have found a file and are waiting for a slot.
/// Past it a delivery is shed and the user is told: the answer's text has
/// already reached them, the attachment is still in object storage, and silence
/// would leave them waiting for a file that is not coming.
const MAX_PENDING_ATTACHMENT_DELIVERIES: usize = 32;

/// Bounds the delivery tasks themselves, and with them the attachment lookups
/// they run before anything about the turn is known. Twice the pending cap as
/// headroom, not as a derived quantity: a turn holds admission for its whole
/// life but claims a pending slot only once its lookup has found a file, so a
/// backlog of file-carrying turns meets the pending cap first — the ordering
/// that keeps the user-facing shed on the path that can name a real file.
///
/// So reaching THIS cap does not imply the pending one is full: turns still
/// inside their lookup hold admission and no pending slot, and turns that find
/// no file hold admission for their whole life and never claim one.
const MAX_ADMITTED_ATTACHMENT_DELIVERIES: usize = 2 * MAX_PENDING_ATTACHMENT_DELIVERIES;

/// The chat-done subscriber half that owns file delivery. Construction and text
/// reply wiring live in `replier.rs` (Go splits them across outbound.go /
/// replier.go for the same subscriber); this module holds the attachment
/// machinery.
/// Shared delivery-ratio counters. Kept in an Arc cell so the detached
/// delivery task and the subscriber observe the same caps.
#[derive(Default)]
struct DeliveryCounters {
    /// Pending-slot counter (Go pendingMu + pendingAttachments).
    pending: std::sync::Mutex<usize>,
    /// Admission counter (Go admittedAttachments).
    admitted: std::sync::Mutex<usize>,
}

pub struct Outbound {
    pool: PgPool,
    app_url: String,
    /// None — a deployment with no object storage — delivers an answer exactly
    /// as before, and the agent is told as much in its brief.
    objects: Option<Arc<dyn MediaObjectStore>>,
    senders: Option<Arc<SendersRegistry>>,
    relay: Option<Arc<OutboundRelay>>,
    counters: Arc<DeliveryCounters>,
    runtime_tasks: Arc<RuntimeTasks>,
}

impl Outbound {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            app_url: String::new(),
            objects: None,
            senders: None,
            relay: None,
            counters: Arc::new(DeliveryCounters::default()),
            runtime_tasks: Arc::new(RuntimeTasks::new()),
        }
    }

    /// Turns on file delivery. Without it — a deployment with no object
    /// storage — an answer is delivered exactly as it was before.
    pub fn with_attachments(mut self, objects: Arc<dyn MediaObjectStore>) -> Self {
        self.objects = Some(objects);
        self
    }

    pub fn with_senders(mut self, senders: Arc<SendersRegistry>) -> Self {
        self.senders = Some(senders);
        self
    }

    pub fn with_relay(mut self, relay: Arc<OutboundRelay>) -> Self {
        self.relay = Some(relay);
        self
    }

    pub fn with_runtime_tasks(mut self, runtime_tasks: Arc<RuntimeTasks>) -> Self {
        self.runtime_tasks = runtime_tasks;
        self
    }

    pub fn with_app_url(mut self, app_url: impl Into<String>) -> Self {
        self.app_url = app_url.into().trim_end_matches('/').to_string();
        self
    }

    /// Subscribes the production text + attachment delivery path. Text is
    /// accepted by WeCom before attachment work is detached, preserving the
    /// answer-first contract.
    pub fn register(self: &Arc<Self>, bus: &cordy_events::Bus) {
        let me = Arc::clone(self);
        let tasks = self.runtime_tasks.clone();
        bus.subscribe(cordy_protocol::EVENT_CHAT_DONE, move |event| {
            let me = Arc::clone(&me);
            let event = event.clone();
            tasks.spawn(async move {
                match tokio::time::timeout(REPLY_BUDGET, me.process_event(&event, true)).await {
                    Err(_) => tracing::warn!(
                        chat_session_id = %event.chat_session_id,
                        "wecom outbound: reply delivery timed out"
                    ),
                    Ok(Err(error)) => tracing::warn!(
                        %error,
                        chat_session_id = %event.chat_session_id,
                        "wecom outbound: reply delivery failed"
                    ),
                    Ok(Ok(())) => {}
                }
            });
        });
        let me = Arc::clone(self);
        let tasks = self.runtime_tasks.clone();
        bus.subscribe(cordy_protocol::EVENT_INBOX_NEW, move |event| {
            let me = Arc::clone(&me);
            let event = event.clone();
            tasks.spawn(async move {
                if let Err(error) = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    me.process_inbox_event(&event, true),
                )
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("inbox delivery timed out")))
                {
                    tracing::warn!(%error, "wecom outbound: inbox delivery failed");
                }
            });
        });
    }

    async fn process_event(&self, event: &Event, allow_relay: bool) -> anyhow::Result<()> {
        let Some(session_id) = event_uuid(event, &event.chat_session_id, "chat_session_id") else {
            return Ok(());
        };
        let Some(task_id) = event_uuid(event, &event.task_id, "task_id") else {
            return Ok(());
        };
        let Some(binding) =
            get_channel_chat_session_binding_by_session(&self.pool, session_id, crate::TYPE_WECOM)
                .await?
        else {
            return Ok(());
        };
        let task = get_agent_task(&self.pool, task_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("load agent task: no row"))?;
        if !cordy_channel_engine::task_input_is_channel_ingested(
            &self.pool,
            task.chat_input_task_id,
        )
        .await?
        {
            return Ok(());
        }
        let Some(inst) =
            get_channel_installation(&self.pool, binding.installation_id, crate::TYPE_WECOM)
                .await?
        else {
            anyhow::bail!("load wecom installation: no row");
        };
        if inst.status != crate::types::INSTALLATION_ACTIVE {
            return Ok(());
        }
        let target = attachment_target(&binding);
        if self
            .senders
            .as_ref()
            .and_then(|senders| senders.get(inst.id))
            .is_none()
        {
            if allow_relay {
                if let Some(relay) = &self.relay {
                    return relay
                        .forward_chat_done(&CancellationToken::new(), inst.id, event)
                        .await;
                }
            }
            anyhow::bail!("wecom: connection not ready on this replica");
        }
        let content = event
            .payload
            .get("content")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if !content.is_empty() {
            let sender = self
                .senders
                .as_ref()
                .and_then(|senders| senders.get(inst.id))
                .ok_or_else(|| anyhow::anyhow!("wecom: connection not ready"))?;
            sender
                .send_text_ctx(
                    &CancellationToken::new(),
                    &target.chat_id,
                    target.chat_type,
                    content,
                )
                .await?;
        }
        if self.may_carry_attachments(event) {
            self.deliver_attachments(event, target);
        }
        Ok(())
    }

    async fn process_inbox_event(&self, event: &Event, allow_relay: bool) -> anyhow::Result<()> {
        let Some(item) = event.payload.get("item") else {
            return Ok(());
        };
        if item.get("recipient_type").and_then(|value| value.as_str()) != Some("member") {
            return Ok(());
        }
        let Some(recipient_id) = item
            .get("recipient_id")
            .and_then(|value| value.as_str())
            .and_then(|value| value.parse::<Uuid>().ok())
        else {
            return Ok(());
        };
        let Some(workspace_id) = item
            .get("workspace_id")
            .and_then(|value| value.as_str())
            .and_then(|value| value.parse::<Uuid>().ok())
        else {
            return Ok(());
        };
        let Some(binding) = cordy_db::queries::channel::find_channel_binding_for_member(
            &self.pool,
            workspace_id,
            recipient_id,
            crate::TYPE_WECOM,
        )
        .await?
        else {
            return Ok(());
        };
        let sender = self
            .senders
            .as_ref()
            .and_then(|senders| senders.get(binding.installation_id));
        let Some(sender) = sender else {
            if allow_relay {
                if let Some(relay) = &self.relay {
                    return relay
                        .forward_inbox_new(
                            &CancellationToken::new(),
                            binding.installation_id,
                            event,
                        )
                        .await;
                }
            }
            anyhow::bail!("wecom: connection not ready on this replica");
        };
        let slug = cordy_db::queries::workspace::get_workspace(&self.pool, workspace_id)
            .await?
            .map(|workspace| workspace.slug)
            .unwrap_or_default();
        let text = crate::inbox_message::build_inbox_markdown(
            item,
            &workspace_id.to_string(),
            &slug,
            &self.app_url,
        );
        if text.is_empty() {
            return Ok(());
        }
        sender
            .send_text_ctx(
                &CancellationToken::new(),
                &binding.channel_user_id,
                1,
                &text,
            )
            .await
    }

    /// Reports whether this turn is worth the lookups even though the agent
    /// said nothing. Everything it checks is already in hand, so a deployment
    /// with no storage — or an event naming no message — costs no query.
    pub fn may_carry_attachments(&self, e: &Event) -> bool {
        self.objects.is_some() && !e.workspace_id.is_empty() && !chat_done_message_id(e).is_nil()
    }

    /// Hands the answer's files to a task of their own, if there are any to
    /// hand over. Called after the words are out; returns immediately.
    pub fn deliver_attachments(&self, e: &Event, to: AttachmentTarget) {
        let (Some(_objects), Some(_senders)) = (&self.objects, &self.senders) else {
            return;
        };
        let message_id = chat_done_message_id(e);
        if message_id.is_nil() {
            return; // a turn with no assistant message has nothing bound to it
        }
        let Ok(workspace_id) = Uuid::parse_str(&e.workspace_id) else {
            return;
        };
        if workspace_id.is_nil() || to.installation_id.is_nil() || to.chat_id.is_empty() {
            return;
        }
        self.deliver_attachment_target(message_id, workspace_id, to);
    }

    fn deliver_attachment_target(
        &self,
        message_id: Uuid,
        workspace_id: Uuid,
        to: AttachmentTarget,
    ) {
        // Admission is claimed here rather than inside the spawned task,
        // because a task that has already started is a task this cap did not
        // bound. The lookup it runs is on the far side of this gate too: under
        // a slow database, unbounded lookups are the same failure as
        // unbounded tasks wearing a different hat.
        //
        // Nothing is known about this turn yet — whether a file is bound to it
        // is exactly what the lookup would tell us — so a refusal here is
        // logged and not spoken. Telling the user their file was dropped when
        // the turn may have carried none is the false alarm the post-lookup
        // gate exists to avoid.
        if !self.admit_attachment_delivery() {
            tracing::warn!(
                installation_id = %to.installation_id,
                admitted = MAX_ADMITTED_ATTACHMENT_DELIVERIES,
                "wecom outbound: attachment delivery not admitted, too many already running"
            );
            return;
        }
        let me = Arc::new(Self {
            pool: self.pool.clone(),
            app_url: self.app_url.clone(),
            objects: self.objects.clone(),
            senders: self.senders.clone(),
            relay: self.relay.clone(),
            counters: Arc::clone(&self.counters),
            runtime_tasks: self.runtime_tasks.clone(),
        });
        let tasks = self.runtime_tasks.clone();
        let rejected = me.clone();
        if !tasks.spawn(async move {
            let ctx = CancellationToken::new();
            if tokio::time::timeout(
                ATTACHMENT_BUDGET,
                send_attachments(me, ctx, message_id, workspace_id, to),
            )
            .await
            .is_err()
            {
                tracing::warn!("wecom outbound: attachment delivery timed out");
            }
        }) {
            rejected.release_attachment_admission();
        }
    }
}

#[async_trait::async_trait]
impl RelayEventHandler for Outbound {
    async fn handle_chat_done(&self, event: Event) -> anyhow::Result<()> {
        self.process_event(&event, false).await
    }

    async fn handle_inbox_new(&self, event: Event) -> anyhow::Result<()> {
        self.process_inbox_event(&event, false).await
    }

    async fn handle_attachments(
        &self,
        installation_id: Uuid,
        message_id: Uuid,
        workspace_id: Uuid,
        chat_id: String,
        chat_type: i64,
    ) -> anyhow::Result<()> {
        if self.objects.is_none() {
            anyhow::bail!("wecom: attachment storage not configured on relay holder");
        }
        if self
            .senders
            .as_ref()
            .and_then(|senders| senders.get(installation_id))
            .is_none()
        {
            anyhow::bail!("wecom: connection lost before relayed attachment delivery");
        }
        self.deliver_attachment_target(
            message_id,
            workspace_id,
            AttachmentTarget {
                installation_id,
                chat_id,
                chat_type,
            },
        );
        Ok(())
    }
}

const REPLY_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

fn event_uuid(event: &Event, envelope: &str, payload_key: &str) -> Option<Uuid> {
    let raw = if envelope.is_empty() {
        event
            .payload
            .get(payload_key)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
    } else {
        envelope
    };
    raw.parse().ok().filter(|id: &Uuid| !id.is_nil())
}

fn attachment_target(binding: &cordy_db::models::ChannelChatSessionBinding) -> AttachmentTarget {
    let config =
        serde_json::from_value::<crate::resolvers::WecomBindingConfig>(binding.config.clone()).ok();
    AttachmentTarget {
        installation_id: binding.installation_id,
        chat_id: config
            .as_ref()
            .map(|config| config.chat_id.clone())
            .filter(|chat_id| !chat_id.is_empty())
            .unwrap_or_else(|| binding.channel_chat_id.clone()),
        chat_type: crate::ws_frame::aibot_chat_type_from_channel(&cordy_channel::ChatType(
            binding.chat_type.clone(),
        )),
    }
}

/// Delivers every file bound to one answer. Free function (not an associated
/// fn) so its future carries no receiver reference across awaits — the shape
/// tokio::spawn can prove Send. Files are independent: one that fails does not
/// stop the rest, and what is known about the ones that did not plainly arrive
/// is said once at the end rather than once each.
///
/// The caller has already claimed admission (see deliver_attachments), which is
/// what bounds the number of tasks running this and the number of lookups
/// below. What is rationed here is different and deliberately after the lookup:
/// a turn with no file bound to it must not consume a pending slot, and — the
/// reason this matters to the user rather than to the scheduler — a delivery
/// refused for want of one can only be reported honestly by something that
/// already knows a file was waiting. Rationing this stage ahead of the lookup
/// would either stay silent about a file that was dropped or warn about a file
/// that never existed.
fn send_attachments(
    me: Arc<Outbound>,
    ctx: CancellationToken,
    message_id: Uuid,
    workspace_id: Uuid,
    to: AttachmentTarget,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    // The inner body awaits wsSender upload paths whose futures hold
    // short-lived &refs (chunk closures); boxing with an explicit Send bound
    // forces the compiler to verify the concrete, owned shape instead of a
    // higher-ranked generalization that rustc cannot prove here.
    Box::pin(async move {
        struct AdmissionGuard(Arc<DeliveryCounters>);
        impl Drop for AdmissionGuard {
            fn drop(&mut self) {
                let mut admitted = self
                    .0
                    .admitted
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                *admitted -= 1;
            }
        }
        let _admission_guard = AdmissionGuard(Arc::clone(&me.counters));
        send_attachments_owned(Arc::clone(&me), ctx, message_id, workspace_id, to).await;
    })
}

async fn send_attachments_owned(
    me: Arc<Outbound>,
    ctx: CancellationToken,
    message_id: Uuid,
    workspace_id: Uuid,
    to: AttachmentTarget,
) {
    let Some(senders) = me.senders.clone() else {
        return;
    };
    let rows = match list_attachments_by_chat_message(&me.pool, message_id, workspace_id).await {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(
                error = %err,
                chat_message_id = %message_id,
                "wecom outbound: attachment lookup failed"
            );
            tell_user(
                ctx.clone(),
                me.clone(),
                to.clone(),
                MEDIA_LOOKUP_FAILED_TEXT.to_string(),
            )
            .await;
            return;
        }
    };
    if rows.is_empty() {
        return;
    }
    // Past here a file is known to be waiting, so every way out of this
    // function has to end in either a delivery or a sentence to the user.

    // Shed when too many deliveries that found a file are already
    // outstanding. The semaphore below bounds how many RUN at once; this
    // bounds how many wait for it, and unlike admission it can name what
    // was dropped, so the user hears about it.
    if !me.claim_attachment_slot() {
        tracing::warn!(
            installation_id = %to.installation_id,
            attachments = rows.len(),
            pending = MAX_PENDING_ATTACHMENT_DELIVERIES,
            "wecom outbound: attachment delivery shed, too many already pending"
        );
        tell_user(
            ctx.clone(),
            me.clone(),
            to.clone(),
            MEDIA_SEND_FAILED_TEXT.to_string(),
        )
        .await;
        return;
    }
    // Released on every path from here. The guard owns an Arc of the
    // counters so no &Outbound borrow crosses the awaits below.
    struct SlotGuard(Arc<DeliveryCounters>);
    impl Drop for SlotGuard {
        fn drop(&mut self) {
            let mut pending = self.0.pending.lock().unwrap_or_else(|e| e.into_inner());
            *pending -= 1;
        }
    }
    let _slot_guard = SlotGuard(Arc::clone(&me.counters));

    // Acquired here and never before the spawn. Bus publish is synchronous
    // on the task-completion path, so blocking out there would wedge the
    // completion path for up to the attachment budget — which is the very
    // thing the detached task exists to prevent.
    let slot = tokio::select! {
        _ = ctx.cancelled() => return,
        slot = ATTACHMENT_SLOTS.acquire() => slot,
    };
    if slot.is_err() {
        return; // semaphore closed: process shutting down
    }

    // Resolved here rather than carried in from the caller: the send that
    // delivered the words may have been minutes ago on a socket since
    // replaced, and the registry always holds the live one.
    let Some(sender) = senders.get(to.installation_id) else {
        // No upload or send has started, so moving the complete attachment
        // job to the successor is safe. The relay claim prevents two holders
        // from accepting it during rollover.
        let Some(relay) = &me.relay else {
            tracing::warn!(
                installation_id = %to.installation_id,
                attachments = rows.len(),
                "wecom outbound: no live connection for attachment delivery"
            );
            return;
        };
        if let Err(error) = relay
            .forward_attachments(
                &ctx,
                to.installation_id,
                message_id,
                workspace_id,
                &to.chat_id,
                to.chat_type,
            )
            .await
        {
            tracing::warn!(
                %error,
                installation_id = %to.installation_id,
                attachments = rows.len(),
                "wecom outbound: attachment rollover relay failed"
            );
        }
        return;
    };
    let mut failed = 0usize;
    let mut unknown = 0usize;
    for row in rows {
        // Log fields captured before the row moves into the delivery.
        let (attachment_id, content_type, size_bytes) =
            (row.id, row.content_type.clone(), row.size_bytes);
        let (state, err) = send_attachment(
            ctx.clone(),
            sender.clone(),
            me.objects.clone(),
            row,
            to.clone(),
        )
        .await;
        match state {
            DeliveryState::DefinitelyFailed => failed += 1,
            DeliveryState::Unknown => unknown += 1,
            DeliveryState::Delivered => {}
        }
        if let Some(err) = err {
            // The object's URL stays out of the log: it is an address that
            // serves the file to whoever holds it.
            tracing::warn!(
                error = %err,
                delivery = state.as_str(),
                installation_id = %to.installation_id,
                attachment_id = %attachment_id,
                content_type = %content_type,
                size_bytes,
                "wecom outbound: attachment not confirmed delivered"
            );
        }
    }
    // The answer is already on the user's screen and it may well refer to a
    // file. Saying nothing would leave them looking for one that never
    // comes — but saying "it failed" about a file that did arrive is its
    // own harm, so each group speaks for itself and an unconfirmed send
    // never borrows the definite wording.
    let mut lines: Vec<&str> = Vec::new();
    if failed > 0 {
        lines.push(MEDIA_SEND_FAILED_TEXT);
    }
    if unknown > 0 {
        lines.push(MEDIA_SEND_UNKNOWN_TEXT);
    }
    if !lines.is_empty() {
        tell_user(ctx.clone(), me.clone(), to.clone(), lines.join("\n")).await;
    }
}

/// Puts one sentence into the conversation, best effort. Every caller is
/// already on a path where something went wrong, so a failure here is logged
/// and dropped rather than propagated — there is nothing further to try.
async fn tell_user(
    ctx: CancellationToken,
    outbound: Arc<Outbound>,
    to: AttachmentTarget,
    text: String,
) {
    let result = if let Some(sender) = outbound
        .senders
        .as_ref()
        .and_then(|senders| senders.get(to.installation_id))
    {
        sender
            .send_text_ctx(&ctx, &to.chat_id, to.chat_type, &text)
            .await
    } else if let Some(relay) = &outbound.relay {
        relay
            .send_text(&ctx, to.installation_id, &to.chat_id, to.chat_type, &text)
            .await
    } else {
        Err(anyhow::anyhow!("wecom: connection not ready"))
    };
    if let Err(err) = result {
        tracing::warn!(
            error = %err,
            installation_id = %to.installation_id,
            "wecom outbound: could not tell the user about the file"
        );
    }
}

/// Carries one file from object storage into the chat, and reports what is
/// known about where it ended up. The error is for the log; the state is what
/// the user is told.
async fn send_attachment(
    ctx: CancellationToken,
    sender: Arc<WsSender>,
    objects: Option<Arc<dyn MediaObjectStore>>,
    row: Attachment,
    to: AttachmentTarget,
) -> (DeliveryState, Option<anyhow::Error>) {
    let Some(objects) = objects else {
        return (
            DeliveryState::DefinitelyFailed,
            Some(anyhow::anyhow!("object store unavailable")),
        );
    };
    // The recorded size is checked before a single byte is fetched. An
    // oversize attachment is refused either way — readObject re-checks what
    // it actually read, because the column is metadata and the object is
    // the truth — but reading 40 MB out of storage to then refuse it is
    // work nobody benefits from.
    if row.size_bytes > MAX_MEDIA_UPLOAD_BYTES as i64 {
        return (
            DeliveryState::DefinitelyFailed,
            Some(
                MediaUploadTooLarge::err()
                    .context(format!("attachment is {} bytes", row.size_bytes)),
            ),
        );
    }
    let read = read_object(&objects, &row.url);
    let data = match tokio::select! {
        _ = ctx.cancelled() => Err(anyhow::anyhow!("attachment delivery deadline exceeded")),
        data = read => data,
    } {
        Ok(data) => data,
        Err(e) => return (DeliveryState::DefinitelyFailed, Some(e)),
    };
    let kind = wecom_media_kind(&row.content_type, &row.filename, data.len());
    let name = outbound_media_name(&row.filename, &row.content_type);

    let media_id = match sender
        .upload_media(
            &ctx,
            OutboundMedia {
                kind,
                filename: name.clone(),
                data,
            },
        )
        .await
    {
        Ok(id) => id,
        Err(e) => {
            // A failed upload never produced a media_id, so no message was
            // ever addressed to the chat — including when the failure was
            // the finish step's own lost ack. The file is definitely not
            // there.
            return (
                DeliveryState::DefinitelyFailed,
                Some(anyhow::anyhow!("upload {}: {e}", kind.as_str())),
            );
        }
    };
    // Video is the only kind with fields beyond the media_id, and both are
    // required. The file's own name is what there is to say about it — the
    // attachment row carries no caption and the agent's words are already
    // in the message above.
    let ext_len = media_path_ext(&name).len();
    let title = name[..name.len() - ext_len]
        .trim_end_matches('.')
        .to_string();
    let result = sender
        .send_media(
            &ctx,
            &to.chat_id,
            to.chat_type,
            MediaSend {
                kind,
                media_id,
                title,
                description: name,
            },
        )
        .await;
    let outcome = send_outcome(result.as_ref().err());
    (outcome, result.err())
}

/// Pulls the whole file into memory. It has to be whole: the upload declares
/// total_size and total_chunks before the first chunk goes out, so there is no
/// streaming this one.
async fn read_object(
    objects: &Arc<dyn MediaObjectStore>,
    raw_url: &str,
) -> anyhow::Result<Vec<u8>> {
    let key = objects.key_from_url(raw_url);
    if key.is_empty() {
        anyhow::bail!("wecom: attachment is not an object this deployment stores");
    }
    // One byte of headroom, so reading exactly the cap can be told from a file
    // that has more to come. The cap is the platform's, not the framing's, so
    // the read stops there rather than at the 50 MB the chunk protocol could
    // have expressed — the extra 30 MB would only ever be resident long enough
    // to be refused.
    let data = objects
        .get_object(&key, MAX_MEDIA_UPLOAD_BYTES + 1)
        .await
        .map_err(|e| anyhow::anyhow!("read attachment: {e:#}"))?;
    if data.len() > MAX_MEDIA_UPLOAD_BYTES {
        return Err(MediaUploadTooLarge::err());
    }
    Ok(data)
}

impl Outbound {
    /// Reserves one of the pending slots, or reports that the backlog is full.
    fn claim_attachment_slot(&self) -> bool {
        let mut pending = self
            .counters
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *pending >= MAX_PENDING_ATTACHMENT_DELIVERIES {
            return false;
        }
        *pending += 1;
        true
    }

    /// Reserves the right to start one delivery task, or reports that too many
    /// are already running. Claimed before the spawn: after it, the task and
    /// its lookup are already past anything this could bound.
    fn admit_attachment_delivery(&self) -> bool {
        let mut admitted = self
            .counters
            .admitted
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *admitted >= MAX_ADMITTED_ATTACHMENT_DELIVERIES {
            return false;
        }
        *admitted += 1;
        true
    }

    fn release_attachment_admission(&self) {
        let mut admitted = self
            .counters
            .admitted
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *admitted -= 1;
    }
}

/// Reads a media push's error for what it says about the message.
///
/// The one distinction that matters: a refusal is WeCom answering, and a
/// missing answer is not an answer. An ack timeout means the frame reached the
/// socket and the verdict never came, which leaves the message possibly
/// delivered.
///
/// A transport failure lands there too, and for the same reason rather than a
/// weaker one. WriteAttemptedError marks an error raised once the write had
/// been entered; past that point the frame may already be at the peer, since a
/// half-closed connection reports "broken pipe" to the writer for bytes the
/// reader has. Only what fails before the write — a marshal error, a deadline
/// the connection refused — is provably undelivered.
///
/// A cancellation lands on the same side, and less precisely than one would
/// like. The request returns the same ContextCancelled whether the token ended
/// before the frame was written or while waiting for its verdict, so from out
/// here the two cannot be told apart. Reading all of these as unknown is the
/// direction that costs least: an unknown is never resent and is described in
/// words that hold either way, so a send that never happened is under-claimed
/// rather than a send that did happen being denied.
fn send_outcome(err: Option<&anyhow::Error>) -> DeliveryState {
    match err {
        None => DeliveryState::Delivered,
        Some(e) if is_ack_timeout(e) || is_write_attempted(e) || is_context_cancelled(e) => {
            DeliveryState::Unknown
        }
        Some(_) => DeliveryState::DefinitelyFailed,
    }
}

/// Decides what WeCom is told this file is.
///
/// The content type leads, because it is what the uploader declared. When it
/// says nothing useful — empty, or the octet-stream that means "bytes" — the
/// filename's extension is the better guess. A kind whose ceiling the file
/// exceeds is demoted to a file rather than sent and refused.
fn wecom_media_kind(content_type: &str, filename: &str, size: usize) -> MediaMsgType {
    let mut ct = base_content_type(content_type);
    if ct.is_empty() || ct == "application/octet-stream" {
        ct = base_content_type(&mime_type_by_extension(media_path_ext(filename)));
    }
    if ct.starts_with("image/") && size <= MAX_OUTBOUND_IMAGE_BYTES {
        return MediaMsgType::Image;
    }
    if ct.starts_with("video/") && size <= MAX_OUTBOUND_VIDEO_BYTES {
        return MediaMsgType::Video;
    }
    // Voice is AMR only. An mp3 sent as a voice note is refused, and as a file
    // it is at least playable after a tap.
    if ct == "audio/amr" && size <= MAX_OUTBOUND_VOICE_BYTES {
        return MediaMsgType::Voice;
    }
    MediaMsgType::File
}

/// Drops the parameters a content type may carry, so
/// "text/csv; charset=utf-8" compares as "text/csv".
fn base_content_type(s: &str) -> String {
    let s = s.trim().to_ascii_lowercase();
    match s.find(';') {
        Some(i) => s[..i].trim().to_string(),
        None => s,
    }
}

/// What the recipient sees on the file card. It is reduced to a single path
/// segment — the name reaches the wire and a stored filename is not guaranteed
/// to be one — and given an extension when it has none, since that is the only
/// hint WeCom gets about the format.
fn outbound_media_name(filename: &str, content_type: &str) -> String {
    let mut name = crate::media_download::clean_media_filename(filename);
    if name.is_empty() {
        name = "attachment".to_string();
    }
    if media_path_ext(&name).is_empty() {
        if let Some(ext) = media_extension(&base_content_type(content_type)) {
            name.push_str(ext);
        }
    }
    name
}

/// Go path.Ext equivalent: the suffix beginning at the final dot in the final
/// element; empty when there is none. A leading dot on a dotfile is not an
/// extension.
fn media_path_ext(name: &str) -> &str {
    let segment = name.rsplit(['/', '\\']).next().unwrap_or("");
    match segment.rfind('.') {
        Some(i) if i > 0 => &segment[i..],
        _ => "",
    }
}

/// Pins the common types whose familiar spelling differs from what a mime
/// database lists first (image/jpeg resolves to ".jfif" on some systems).
fn media_extension(content_type: &str) -> Option<&'static str> {
    match content_type {
        "image/jpeg" => Some(".jpg"),
        "image/png" => Some(".png"),
        "image/gif" => Some(".gif"),
        "image/webp" => Some(".webp"),
        "video/mp4" => Some(".mp4"),
        "application/pdf" => Some(".pdf"),
        "text/plain" => Some(".txt"),
        _ => mime_guess_ext(content_type),
    }
}

fn mime_guess_ext(content_type: &str) -> Option<&'static str> {
    match content_type {
        "application/json" => Some(".json"),
        "text/csv" => Some(".csv"),
        "application/zip" => Some(".zip"),
        _ => None,
    }
}

/// A minimal mime.TypeByExtension stand-in covering the extensions WeCom
/// attachments actually carry; unknown ones yield "" like Go's lookup miss.
fn mime_type_by_extension(ext: &str) -> String {
    match ext {
        ".jpg" | ".jpeg" => "image/jpeg".to_string(),
        ".png" => "image/png".to_string(),
        ".gif" => "image/gif".to_string(),
        ".webp" => "image/webp".to_string(),
        ".mp4" => "video/mp4".to_string(),
        ".amr" => "audio/amr".to_string(),
        ".pdf" => "application/pdf".to_string(),
        ".txt" => "text/plain".to_string(),
        _ => String::new(),
    }
}

/// Pulls the assistant message id out of a chat:done payload. It is the key
/// every attachment on this turn is bound to.
fn chat_done_message_id(e: &Event) -> Uuid {
    e.payload
        .get("message_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or(Uuid::nil())
}
