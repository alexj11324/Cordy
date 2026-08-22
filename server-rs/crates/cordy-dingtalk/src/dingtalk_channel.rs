//! Port of `dingtalk_channel.go`: ONE installation's DingTalk Stream
//! connection. Every installation carries its own robot — its own AppKey plus
//! encrypted AppSecret in the installation config — so it gets its own
//! connection, exactly like the per-installation Slack and Feishu adapters.
//! The engine Supervisor builds one channel per active installation (via the
//! registered Factory) and owns the lease / reconnect lifecycle; connect blocks
//! until the run context is cancelled.
//!
//! Inbound events are translated by [`crate::inbound`], parameterized by THIS
//! installation's AppKey so the engine router can resolve the installation (the
//! DingTalk callback carries no robot code). Outbound replies flow through the
//! EventChatDone subscriber ([`crate::outbound`]) and the OutboundReplier;
//! send satisfies the Channel contract for a group reply.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use cordy_channel::capability::Capability;
use cordy_channel::message::{OutboundMessage, SendResult};
use cordy_channel::registry::Registry;
use cordy_channel::{BuiltChannel, Channel, Config, Factory, Type};

use crate::channel_type;
use crate::client::Client;
use crate::config::{decrypt_token, Credentials, Decrypter, InstallConfig};
use crate::dispatch::{DispatchHandle, Dispatcher};
use crate::inbound::{inbound_from_callback, BotCallbackData};
use crate::outbound_send::{SendTarget, Sender};
use crate::replier::{is_addressed_issue_command, target_from_message};
use crate::ws_connector::{OnMessage, WsConnector};

/// Bounds the detached dispatch-error reply send.
const ISSUE_ERROR_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Shutdown budget for draining accepted jobs when the Channel trait's
/// ctx-less disconnect fires (Go used the supervisor's ctx deadline).
const DISPATCH_DRAIN_BUDGET: Duration = Duration::from_secs(30);

const ISSUE_DISPATCH_FAILED_TEXT: &str =
    "⚠️ I couldn't create that issue because an internal error occurred. Please try again.";

/// The slice of channel state the dispatcher's job body runs against. Kept
/// separate so the per-AppKey slot can point at the NEWEST generation's
/// credentials across Stream reconnects without holding the whole channel.
struct ChannelJobRunner {
    /// Generation identity for the slot CAS (Go compares channel pointers).
    generation: Uuid,
    handler: Option<cordy_channel::InboundHandler>,
    client: Arc<Client>,
    app_id: String,
    robot_code: String,
    app_key: String,
    app_secret: String,
}

impl ChannelJobRunner {
    /// The dispatcher's job body: hand the message to the engine and surface
    /// pipeline errors the way the old inline path did.
    async fn run_inbound(&self, ctx: CancellationToken, msg: cordy_channel::InboundMessage) {
        let Some(handler) = &self.handler else {
            return;
        };
        if let Err(err) = handler.call(ctx, msg.clone()).await {
            tracing::warn!(
                error = %err,
                app_id = %self.app_id,
                "dingtalk: inbound handler error"
            );
            notify_issue_dispatch_error(
                &self.client,
                &self.robot_code,
                &self.app_key,
                &self.app_secret,
                &msg,
            );
        }
    }
}

/// Posts an internal-error notice when an addressed /issue command failed
/// inside the engine pipeline (a transient resolver / DB error, before the
/// shared issue-command path could report a Result). The frame is already ACKed
/// and never redelivered, so without this the command would vanish silently.
/// Detached so the ingest path returns promptly.
fn notify_issue_dispatch_error(
    client: &Arc<Client>,
    robot_code: &str,
    app_key: &str,
    app_secret: &str,
    msg: &cordy_channel::InboundMessage,
) {
    if !is_addressed_issue_command(msg) {
        return;
    }
    let client = client.clone();
    let target = target_from_message(msg);
    let creds = Credentials {
        app_key: app_key.to_string(),
        robot_code: robot_code.to_string(),
        app_secret: app_secret.to_string(),
    };
    tokio::spawn(async move {
        let sender = Sender::new(client, creds);
        let send = sender.send(&target, ISSUE_DISPATCH_FAILED_TEXT);
        match tokio::time::timeout(ISSUE_ERROR_REPLY_TIMEOUT, send).await {
            Ok(Ok(_)) => {}
            Ok(Err(send_err)) => {
                tracing::warn!(error = %send_err, "dingtalk: issue dispatch-error reply failed")
            }
            Err(_) => {
                tracing::warn!("dingtalk: issue dispatch-error reply timed out")
            }
        }
    });
}

/// Keeps one conversation dispatcher alive across the channel objects the
/// supervisor rebuilds for Stream reconnects. `current` is replaced before each
/// reconnect so queued jobs use the newest credentials and client.
pub struct DispatchSlot {
    state: Mutex<Option<Arc<ChannelJobRunner>>>,
    queue: OnceLock<Arc<Dispatcher>>,
}

impl DispatchSlot {
    /// Creates the slot together with its dispatcher, whose job body resolves
    /// the CURRENT generation at run time.
    pub(crate) fn new_shared() -> Arc<Self> {
        let slot = Arc::new(Self {
            state: Mutex::new(None),
            queue: OnceLock::new(),
        });
        let weak = Arc::downgrade(&slot);
        let handle: DispatchHandle = Arc::new(move |ctx, msg| {
            let weak = weak.clone();
            Box::pin(async move {
                let Some(slot) = weak.upgrade() else {
                    return;
                };
                let runner = slot.current();
                if let Some(runner) = runner {
                    runner.run_inbound(ctx, msg).await;
                }
            })
        });
        let _ = slot.queue.set(Arc::new(Dispatcher::new(handle)));
        slot
    }

    fn current(&self) -> Option<Arc<ChannelJobRunner>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_current(&self, runner: Arc<ChannelJobRunner>) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Some(runner);
    }

    fn queue(&self) -> &Arc<Dispatcher> {
        self.queue.get().expect("queue set at construction")
    }
}

/// One installation's DingTalk Stream connection (Go `dingtalkChannel`).
pub struct DingTalkChannel {
    /// Generation identity replacing Go's pointer comparison in the slot CAS.
    generation: Uuid,
    /// AppKey — routing key stamped into each inbound envelope.
    app_id: String,
    robot_code: String,
    app_key: String,
    /// Decrypted — opens the Stream connection + mints tokens.
    app_secret: String,
    client: Arc<Client>,
    handler: Option<cordy_channel::InboundHandler>,
    /// Runs inbound jobs off the socket read loop on per-conversation serial
    /// queues. Built once per slot in the factory; it survives redials, and
    /// in-flight jobs deliberately outlive the socket.
    dispatch: Arc<Dispatcher>,
    slot: Arc<DispatchSlot>,
    /// Set by connect only when its run context was already cancelled when the
    /// transport returned (revoke, lease loss, rotation, or process shutdown).
    /// A normal gateway redial leaves it false so the queue survives and
    /// preserves per-conversation ordering across the reconnect.
    stop_dispatch: AtomicBool,
}

#[async_trait]
impl Channel for DingTalkChannel {
    fn r#type(&self) -> Type {
        channel_type()
    }

    fn capabilities(&self) -> Capability {
        Capability::TEXT | Capability::ATTACHMENT
    }

    /// Drains the dispatch queue only when connect observed lifecycle
    /// cancellation. Transport errors and gateway-requested redials
    /// intentionally keep the queue alive for the next channel generation. The
    /// slot lock prevents an older cancelled generation from closing a queue
    /// already adopted by its replacement.
    async fn disconnect(&self) -> anyhow::Result<()> {
        if !self.stop_dispatch.load(Ordering::SeqCst) {
            return Ok(());
        }
        {
            let current = self.slot.state.lock().unwrap_or_else(|e| e.into_inner());
            match &*current {
                Some(runner) if runner.generation == self.generation => {
                    // Keep current pointing at this generation while accepted
                    // jobs drain. The closed marker is published under the slot
                    // lock before a replacement can inspect the slot, so the
                    // factory will create a fresh queue instead of adopting it.
                    self.dispatch.start_close();
                }
                _ => return Ok(()),
            }
        }
        // The Channel trait carries no caller deadline; bound the drain so a
        // wedged job cannot hang shutdown forever (Go bounded it with the
        // supervisor's ctx).
        let drained = tokio::time::timeout(
            DISPATCH_DRAIN_BUDGET,
            self.dispatch.wait_closed(CancellationToken::new()),
        )
        .await;
        // Clear our claim whether or not the drain finished (Go's deferred CAS).
        {
            let mut current = self.slot.state.lock().unwrap_or_else(|e| e.into_inner());
            let matches = current
                .as_ref()
                .is_some_and(|runner| runner.generation == self.generation);
            if matches {
                *current = None;
            }
        }
        match drained {
            Ok(true) => Ok(()),
            Ok(false) | Err(_) => anyhow::bail!("dingtalk: dispatcher drain timed out"),
        }
    }

    /// Posts a group reply into out.chat_id with this installation's robot. It
    /// satisfies the Channel contract; the primary reply paths (EventChatDone
    /// subscriber and OutboundReplier) build their own sender with a full
    /// target.
    async fn send(&self, out: OutboundMessage) -> anyhow::Result<SendResult> {
        let sender = Sender::new(
            self.client.clone(),
            Credentials {
                app_key: self.app_key.clone(),
                robot_code: self.robot_code.clone(),
                app_secret: self.app_secret.clone(),
            },
        );
        let key = sender
            .send(&SendTarget::group(out.chat_id), &out.text)
            .await?;
        Ok(SendResult { message_id: key })
    }

    /// Opens this installation's Stream connection (authenticated with its own
    /// AppKey/AppSecret) and blocks until ctx is cancelled. The connector owns
    /// a single socket session and returns on ctx cancel, a gateway disconnect,
    /// or a broken socket; the engine.Supervisor owns reconnect/backoff and the
    /// per-installation lease, so an error here just triggers a supervised
    /// redial.
    async fn connect(&self, ctx: CancellationToken) -> anyhow::Result<()> {
        let result = self.connect_inner(&ctx).await;
        // Supervisor cancels its child context after every connect return, so
        // this verdict must be captured here, before control returns to
        // Supervisor.
        self.stop_dispatch
            .store(ctx.is_cancelled(), Ordering::SeqCst);
        result
    }
}

impl DingTalkChannel {
    async fn connect_inner(&self, ctx: &CancellationToken) -> anyhow::Result<()> {
        if self.handler.is_none() {
            anyhow::bail!("dingtalk: inbound handler not configured");
        }
        if self.app_secret.is_empty() {
            anyhow::bail!("dingtalk: app secret not configured");
        }
        let on_message: OnMessage = {
            let app_id = self.app_id.clone();
            let dispatch = self.dispatch.clone();
            Arc::new(move |_ctx, data: BotCallbackData| {
                let app_id = app_id.clone();
                let dispatch = dispatch.clone();
                Box::pin(async move { on_stream_message(&app_id, &dispatch, &data) })
            })
        };
        let conn = WsConnector::new(
            self.client.http().clone(),
            self.client.api_base().to_string(),
            self.app_key.clone(),
            self.app_secret.clone(),
            on_message,
        );
        conn.run(ctx.clone()).await
    }
}

/// The connector's bot-message callback. It translates the event with THIS
/// installation's AppKey and enqueues it on the per-conversation dispatcher, so
/// the socket read loop ACKs immediately and is never blocked by pipeline work
/// (media downloads can take tens of seconds). It always succeeds: DingTalk
/// never redelivers robot messages and the engine's (installation, msgId) dedup
/// guards any duplicate anyway.
fn on_stream_message(
    app_id: &str,
    dispatch: &Arc<Dispatcher>,
    data: &BotCallbackData,
) -> anyhow::Result<()> {
    match inbound_from_callback(data, app_id) {
        None => {
            // The message never reaches the engine (no sender staff id — a
            // system or bot-authored event), so no channel_inbound_audit row is
            // written for it. Log the drop so the report is diagnosable instead
            // of vanishing silently. (Malformed/over-quota media now DOES reach
            // the engine as an unavailable-image placeholder.)
            tracing::info!(
                app_id = %app_id,
                msg_type = %data.msgtype,
                msg_id = %data.msg_id,
                has_sender = !data.sender_staff_id.is_empty(),
                "dingtalk: dropped unsupported inbound message"
            );
        }
        Some(msg) => {
            let conv_id = msg.source.chat_id.clone();
            dispatch.enqueue(&conv_id, msg);
        }
    }
    Ok(())
}

/// The shared dependencies the DingTalk Factory closes over. The engine inbound
/// handler is supplied per-build via [`Config::handler`]; the Decrypter turns
/// the installation's stored ciphertext AppSecret into plaintext; the Client
/// owns the outbound token cache + transport.
#[derive(Default)]
pub struct ChannelDeps {
    pub decrypt: Option<Arc<Decrypter>>,
    pub client: Option<Arc<Client>>,
}

/// Registers the per-installation DingTalk Factory so the engine Supervisor
/// builds + supervises one channel per active installation.
pub fn register_dingtalk(reg: &Registry, deps: ChannelDeps) {
    reg.register(channel_type(), new_dingtalk_factory(deps));
}

pub fn new_dingtalk_factory(deps: ChannelDeps) -> Factory {
    let client = deps
        .client
        .unwrap_or_else(|| Arc::new(Client::new(None, "")));
    let decrypt = deps.decrypt;
    // One dispatcher slot per installation AppKey, alive across reconnects.
    let slots: Arc<Mutex<HashMap<String, Arc<DispatchSlot>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    Arc::new(move |cfg: Config| {
        let client = client.clone();
        let decrypt = decrypt.clone();
        let slots = slots.clone();
        Box::pin(async move { build_channel(cfg, client, decrypt, slots).await })
    })
}

async fn build_channel(
    cfg: Config,
    client: Arc<Client>,
    decrypt: Option<Arc<Decrypter>>,
    slots: Arc<Mutex<HashMap<String, Arc<DispatchSlot>>>>,
) -> anyhow::Result<BuiltChannel> {
    let ic: InstallConfig = serde_json::from_value(cfg.raw.clone())
        .map_err(|e| anyhow::anyhow!("dingtalk: decode installation config: {e}"))?;
    let app_secret = decrypt_token(&ic.app_secret_encrypted, decrypt.as_deref())
        .map_err(|e| anyhow::anyhow!("dingtalk: decrypt app secret: {e:#}"))?;
    if app_secret.is_empty() {
        anyhow::bail!("dingtalk: installation has no app secret");
    }

    // Supervisor.Build runs once per reconnect. Reusing the queue by the
    // installation's unique AppKey prevents an old in-flight turn and the next
    // turn received after reconnect from running concurrently.
    let slot = {
        let mut map = slots.lock().unwrap_or_else(|e| e.into_inner());
        let reusable = map
            .get(&ic.app_id)
            .filter(|s| !s.queue().is_closed())
            .cloned();
        match reusable {
            Some(slot) => slot,
            None => {
                let fresh = DispatchSlot::new_shared();
                map.insert(ic.app_id.clone(), fresh.clone());
                fresh
            }
        }
    };

    let generation = Uuid::now_v7();
    let runner = Arc::new(ChannelJobRunner {
        generation,
        handler: cfg.handler.clone(),
        client: client.clone(),
        app_id: ic.app_id.clone(),
        robot_code: ic.robot_code_or_app_id().to_string(),
        app_key: ic.app_id.clone(),
        app_secret: app_secret.clone(),
    });
    slot.set_current(runner);

    let ch = Arc::new(DingTalkChannel {
        generation,
        app_id: ic.app_id.clone(),
        robot_code: ic.robot_code_or_app_id().to_string(),
        app_key: ic.app_id.clone(),
        app_secret,
        client,
        handler: cfg.handler,
        dispatch: slot.queue().clone(),
        slot,
        stop_dispatch: AtomicBool::new(false),
    });
    Ok(ch as BuiltChannel)
}
