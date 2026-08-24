//! The Channel + Factory the engine Supervisor drives, plus the WebSocket run
//! loop for one aibot smart-bot connection — port of `wecom_channel.go`.
//!
//! WeCom allows only one active connection per bot; the Supervisor's WS lease
//! enforces that same "at most one per replica" invariant at the process
//! layer, so the combination gives us a single global connection per
//! installation without wecom-specific coordination.
//!
//! The read loop lives here rather than in a shared connector because the
//! aibot protocol is small enough that a per-installation loop is clearer
//! than an EventConnector abstraction.
//!
//! Port note: Go bridges ctx cancellation to the blocking ReadMessage call
//! with a watchdog that closes the socket. Rust reaches the same unblocking by
//! selecting the read future against the cancellation token — dropping the
//! parked future releases the socket without a forced close.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use cordy_channel::capability::Capability;
use cordy_channel::channel::{BuiltChannel, Channel, Config, Factory, FactoryFuture, Type};
use cordy_channel::handler::InboundHandler;
use cordy_channel::LeaseGeneration;

use crate::credential_probe::{classify_subscribe_ack, is_credentials_rejected};
use crate::credentials::CredentialsResolver;
use crate::metrics::{or_nop_metrics, Metrics};
use crate::senders_registry::SendersRegistry;
use crate::trace::{trace_in, trace_inbound};
use crate::types::{InstallConfig, Installation};
use crate::ws_frame::{
    aibot_chat_type_from_channel, channel_message_from_callback, AibotEventCallback,
    AibotMsgCallback, FrameEnvelope, CMD_EVENT_CALLBACK, CMD_MSG_CALLBACK, CMD_PONG,
    CMD_SERVER_PING, EVENT_DISCONNECTED,
};
use crate::ws_sender::{DefaultDialer, Dialer, WsConn, WsSender};

/// The aibot long-connection endpoint. WeCom publishes a single global
/// endpoint for every bot; the (bot_id, secret) pair carried in the
/// aibot_subscribe frame after the WS handshake identifies which bot the
/// connection belongs to.
pub const DEFAULT_WS_URL: &str = "wss://openws.work.weixin.qq.com";

/// The client-driven heartbeat cadence. WeCom's docs prescribe 30s; below
/// that they may kill the socket, above that we spam.
pub const PING_INTERVAL: Duration = Duration::from_secs(30);

/// Caps the wait between "sent aibot_subscribe" and "received the errcode 0
/// ack". The server responds within a few hundred milliseconds in practice;
/// this bound protects against a silent socket.
pub const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Refreshed on every successful read. If no bytes arrive within this window
/// we assume the socket is dead and force-close it — the Supervisor then
/// handles reconnect. It MUST exceed [`PING_INTERVAL`] by a comfortable margin
/// so a pong is not late enough to trigger a false trip.
pub const READ_DEADLINE: Duration = Duration::from_secs(90);

/// Caps a single frame's write budget. Below this a genuinely slow socket is
/// preferable to an infinitely stuck writer.
pub const WRITE_DEADLINE: Duration = Duration::from_secs(10);

/// Bounds the initial TCP + WS handshake dial.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECTOR_TASK_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

/// How far the callback worker may fall behind the read loop before the read
/// loop blocks. Past this the socket stops being drained, WeCom notices, and
/// the connection is replaced — which is the correct outcome: a replica that
/// cannot keep up should hand the bot to one that can, not quietly discard
/// the messages it could not reach.
pub const CALLBACK_QUEUE_DEPTH: usize = 64;

/// The one line sent back for a message this adapter cannot read at all. It
/// used to say "我目前只能处理文字消息" — text only — which stopped being true
/// the moment photos, files, videos and 图文混排 started routing: a person who
/// has just watched the bot answer a screenshot, then gets told it only
/// handles text, reads that as the bot being broken rather than as this one
/// kind not being supported.
pub const UNSUPPORTED_MSG_TYPE_RECEIPT: &str = "抱歉，我暂时无法处理这类消息。";

/// Returned by [`WecomChannel::send`]. WeCom's generic Channel.Send seam has
/// no honest implementation — the outbound envelope carries no chat_type, and
/// outbound already flows through the replier / outbound subscriber, which
/// read the chat type off the inbound frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("wecom: Channel.Send is not supported; outbound goes through OutboundReplier/Outbound")]
pub struct SendNotSupported;

/// Downcasts the chain for the Send seam refusal.
pub fn is_send_not_supported(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|c| c.downcast_ref::<SendNotSupported>().is_some())
}

/// One installation's aibot smart-bot WebSocket connection. The engine
/// Supervisor builds one per active installation via the registered Factory
/// and drives lease / reconnect lifecycle; connect blocks on the receive loop
/// until the token is cancelled or the link drops.
pub struct WecomChannel {
    /// Nil UUID stands in for Go's invalid pgtype.UUID.
    installation_id: Uuid,
    bot_id: String,
    secret: String,
    /// What this bot is called in a chat, from the installation config. Empty
    /// on every installation that has not filled it in; see
    /// `strip_leading_mentions` for what an empty name falls back to.
    bot_display_name: String,
    handler: Option<InboundHandler>,
    dialer: Arc<dyn Dialer>,
    ws_url: String,
    /// The process-wide installation→sender registry. We hold a reference so
    /// connect can register itself on entry and clear on exit.
    senders: Option<Arc<SendersRegistry>>,
    generation: Arc<LeaseGeneration>,
    /// The health sink. Never called directly — go through `metrics`, which is
    /// always a safe-to-call sink.
    metrics: Arc<dyn Metrics>,
}

#[async_trait]
impl Channel for WecomChannel {
    fn r#type(&self) -> Type {
        crate::type_wecom()
    }

    /// Declares what the aibot adapter supports today. Inbound attachments are
    /// downloaded, decrypted and bound (media_ingest.rs), so ATTACHMENT holds
    /// in the same direction it holds for DingTalk. Sending media back out is
    /// a separate matter — it needs WeCom's aibot_upload_media_* handshake —
    /// and is not claimed here.
    fn capabilities(&self) -> Capability {
        Capability::TEXT | Capability::ATTACHMENT
    }

    /// A no-op: the WS connection's whole lifetime is scoped to connect (it
    /// returns when the run token is cancelled), so there is no long-lived
    /// resource to release here.
    async fn disconnect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Dials the aibot long-connection endpoint, sends the subscribe frame,
    /// and runs the read loop until the token is cancelled or the link drops.
    /// Every exit path cancels the derived run state and waits for the spawned
    /// workers to observe it, so a transient failure tears the live connection
    /// down before the Supervisor reconnects — no leaked socket task consuming
    /// events into an unread channel.
    async fn connect(&self, ctx: CancellationToken) -> anyhow::Result<()> {
        let handler = self
            .handler
            .clone()
            .ok_or_else(|| anyhow::anyhow!("wecom: inbound handler not configured"))?;
        if self.bot_id.is_empty() || self.secret.is_empty() {
            anyhow::bail!("wecom: bot_id / secret not configured");
        }
        let ws_url = if self.ws_url.is_empty() {
            DEFAULT_WS_URL.to_string()
        } else {
            self.ws_url.clone()
        };

        let conn: Arc<dyn WsConn> = match self.dialer.dial(&ctx, &ws_url).await {
            Ok(c) => c.into(),
            Err(e) => {
                self.metrics.record_connect_failure();
                return Err(anyhow::anyhow!("wecom: dial {ws_url}: {e}"));
            }
        };

        let sender = Arc::new(WsSender::with_generation(
            conn.clone(),
            self.generation.clone(),
        ));

        // Subscribe — auth the connection. Any error here yields the loop back
        // to the Supervisor for backoff + retry.
        subscribe(self, &ctx, &conn, &sender).await?;
        tracing::info!(
            installation_id = %self.installation_id,
            bot_id = %self.bot_id,
            "wecom: subscribe ok"
        );

        // Install the sender on the registry so the boot-time outbound paths
        // can locate this connection by installation id and push
        // aibot_send_msg over the same socket. Cleared on exit so a stale
        // sender for a dead connection is never dispatched to.
        let registered = match (&self.senders, self.installation_id != Uuid::nil()) {
            (Some(reg), true) => {
                reg.set(
                    self.installation_id,
                    sender.clone(),
                    self.generation.clone(),
                );
                true
            }
            _ => false,
        };

        // Heartbeat — WeCom kills silent sockets past ~90s. We ping every 30s
        // via the shared writer mutex so it interleaves cleanly with other
        // outbound frames.
        let ping_ctx = ctx.child_token();
        let ping_handle = tokio::spawn(ping_loop(ping_ctx.clone(), sender.clone()));

        // Inbound callbacks run on their own worker, not on the read loop.
        // The read loop is the sole deliverer of server verdicts, so anything
        // that handles a callback inline cannot also wait for the ack of a
        // frame it writes — it would be waiting on itself.
        //
        // ONE worker, not a pool: WeCom delivers a chat's messages in order,
        // and the engine's dedup and turn batching assume that order survives.
        //
        // A full queue BLOCKS the read loop rather than dropping.
        // Backpressure costs a reconnect; dropping costs a user's message with
        // nothing to say so.
        let (cb_tx, mut cb_rx) = mpsc::channel::<FrameEnvelope>(CALLBACK_QUEUE_DEPTH);
        let worker_err: Arc<std::sync::Mutex<Option<anyhow::Error>>> =
            Arc::new(std::sync::Mutex::new(None));
        let worker = {
            let w_err = worker_err.clone();
            let w_handler = handler.clone();
            let w_sender = sender.clone();
            let w_conn = conn.clone();
            let w_ctx = ctx.clone();
            let w_bot_id = self.bot_id.clone();
            let w_display_name = self.bot_display_name.clone();
            tokio::spawn(async move {
                while let Some(env) = cb_rx.recv().await {
                    if let Err(e) = dispatch_frame(
                        &w_ctx,
                        &env,
                        &w_handler,
                        &w_sender,
                        &w_bot_id,
                        &w_display_name,
                    )
                    .await
                    {
                        // Wake the read loop if it is parked in a read; a
                        // cancelled token alone will not move it. A read loop
                        // parked on the queue send is woken by the dropped
                        // receiver instead.
                        *w_err.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
                        let _ = tokio::time::timeout(WRITE_DEADLINE, w_conn.close()).await;
                        break;
                    }
                }
            })
        };

        // Read loop. Every frame comes back through the same decode → dispatch
        // → (maybe) reply path. A single bad frame does NOT tear the socket
        // down — only transport / handler errors escalate.
        let result = loop {
            if ctx.is_cancelled() {
                break Ok(());
            }
            // Armed immediately before the read, and nowhere else: the idle
            // window should measure idleness.
            match conn
                .read_message(Some(Instant::now() + READ_DEADLINE))
                .await
            {
                Err(e) => {
                    // The shutdown path closes the socket under us; that is an
                    // ordinary stop, not a failure to report.
                    if ctx.is_cancelled() {
                        break Ok(());
                    }
                    break Err(anyhow::anyhow!("wecom: read: {e}"));
                }
                Ok(payload) => {
                    let env: FrameEnvelope = match serde_json::from_slice(&payload) {
                        Ok(env) => env,
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                size = payload.len(),
                                "wecom: bad frame envelope"
                            );
                            continue;
                        }
                    };
                    trace_in(&env);
                    match env.cmd.as_str() {
                        CMD_MSG_CALLBACK | CMD_EVENT_CALLBACK => {
                            match cb_tx.try_send(env.clone()) {
                                Ok(()) => {
                                    self.metrics.record_callback_queued();
                                    continue;
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => break Ok(()),
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    // The worker is behind. Blocking is the
                                    // deliberate choice, and it is also the
                                    // thing an operator wants to know about —
                                    // from here on the socket stops being
                                    // drained, and if it lasts, WeCom replaces
                                    // the connection.
                                    self.metrics.record_callback_queue_blocked();
                                    tokio::select! {
                                        res = cb_tx.send(env) => {
                                            match res {
                                                Ok(()) => self.metrics.record_callback_queued(),
                                                // The worker has stopped, so this
                                                // send has no receiver. Return and
                                                // let the post-loop handler
                                                // substitute the worker's error,
                                                // which is the real cause.
                                                Err(_) => break Ok(()),
                                            }
                                        }
                                        _ = ctx.cancelled() => break Ok(()),
                                    }
                                }
                            }
                        }
                        // Acks, pings and pongs stay on the read loop: they
                        // are the frames the worker's own writes are waiting
                        // for.
                        _ => {
                            match dispatch_frame(
                                &ctx,
                                &env,
                                &handler,
                                &sender,
                                &self.bot_id,
                                &self.bot_display_name,
                            )
                            .await
                            {
                                Ok(()) => {}
                                Err(e) => break Err(e),
                            }
                        }
                    }
                }
            }
        };

        // Close the queue so the worker drains what is left and exits, then
        // wait for it — its error is the real cause; the read error that
        // followed it is just the socket we closed to get here. Only promoted
        // on a live token: a shutdown or lease-loss cancel that catches a
        // callback mid-flight is an ordinary stop, and promoting that
        // callback's error would report a spurious "connection exited with
        // error".
        drop(cb_tx);
        ping_ctx.cancel();
        if !cordy_channel::shutdown_join_handles(
            vec![worker, ping_handle],
            CONNECTOR_TASK_SHUTDOWN_TIMEOUT,
        )
        .await
        {
            tracing::warn!(
                installation_id = %self.installation_id,
                "wecom: connector tasks exceeded shutdown deadline; aborting"
            );
        }
        if registered {
            if let Some(reg) = &self.senders {
                reg.clear(self.installation_id, &self.generation);
            }
        }
        if tokio::time::timeout(WRITE_DEADLINE, conn.close())
            .await
            .is_err()
        {
            tracing::warn!(
                installation_id = %self.installation_id,
                "wecom: websocket close exceeded shutdown deadline"
            );
        }

        let worker_error = worker_err.lock().unwrap_or_else(|e| e.into_inner()).take();
        match (worker_error, result) {
            (Some(we), _) if !ctx.is_cancelled() => Err(we),
            (_, r) => r,
        }
    }

    /// The outbound Channel entry the engine calls with a normalized
    /// OutboundMessage. Not used: outbound for wecom goes through the
    /// replier / outbound subscriber, which know the message's real chat type
    /// and address the correct chat. The generic seam is never invoked by the
    /// engine for this channel; return not-supported rather than keep a
    /// second, heuristic outbound path alive.
    async fn send(
        &self,
        _out: cordy_channel::message::OutboundMessage,
    ) -> anyhow::Result<cordy_channel::message::SendResult> {
        Err(anyhow::Error::new(SendNotSupported))
    }
}

/// Sends the aibot_subscribe frame and waits (up to [`SUBSCRIBE_TIMEOUT`]) for
/// the server's ack. The ack shape is a frame with echoed headers.req_id +
/// errcode; errcode == 0 means good.
///
/// A non-zero errcode goes through classify_subscribe_ack — the same function
/// the install-time credential probe uses on the same ack, so the two cannot
/// answer the same code differently. 40001 / 40013 come back as a credential
/// rejection: the refusal that repeats identically on every backoff until
/// somebody fixes the installation. Every other non-zero code is unverifiable,
/// because a throttle (45009, 45033) or a platform-side failure clears on its
/// own, and counting one as a credential failure would page an operator about
/// a tenant whose bot is fine.
async fn subscribe(
    channel: &WecomChannel,
    ctx: &CancellationToken,
    conn: &Arc<dyn WsConn>,
    sender: &WsSender,
) -> anyhow::Result<()> {
    let req_id = match sender
        .send_subscribe(&channel.bot_id, &channel.secret)
        .await
    {
        Ok(id) => id,
        Err(e) => {
            channel.metrics.record_connect_failure();
            return Err(anyhow::anyhow!("wecom: send subscribe: {e}"));
        }
    };

    // Wait for the matching ack — the server writes it as a frame with cmd
    // empty (or absent) and headers.req_id equal to ours. Any other frame that
    // arrives first is dropped (subscribe is the very first exchange, so this
    // is rare in practice).
    let deadline = Instant::now() + SUBSCRIBE_TIMEOUT;
    loop {
        if ctx.is_cancelled() {
            anyhow::bail!("wecom: context cancelled during subscribe");
        }
        let payload = match conn.read_message(Some(deadline)).await {
            Ok(p) => p,
            // The socket died, the ack never arrived inside the timeout, or
            // our own token was cancelled mid-read. Infrastructure or a
            // shutdown — nobody has to be told either way, and the next
            // backoff may well succeed.
            Err(e) => {
                channel.metrics.record_connect_failure();
                return Err(anyhow::anyhow!("wecom: subscribe read: {e}"));
            }
        };
        let env: FrameEnvelope = match serde_json::from_slice(&payload) {
            Ok(env) => env,
            Err(_) => continue,
        };
        // Traced before the req_id filter: a subscribe that is rejected, or
        // answered on a req_id we never sent, is exactly the failure an
        // operator turns tracing on to see.
        trace_in(&env);
        if env.headers.req_id != req_id {
            continue;
        }
        if env.err_code != 0 {
            // Which counter this lands on depends on what the ack means, and
            // classify_subscribe_ack already decides that — the same verdict
            // the install-time credential probe gets. Branch on its answer
            // rather than testing the errcode again here.
            let err = classify_subscribe_ack(env.err_code, &env.err_msg);
            if is_credentials_rejected(&err) {
                // Refused on its merits: a wrong secret, a deleted bot.
                // Counted apart from every other connection failure because it
                // is the only one that repeats identically on every backoff
                // until a person changes something.
                channel.metrics.record_auth_failure();
            } else {
                // Unverifiable — a throttle, or a platform-side failure. It
                // clears on its own, exactly like a dial that did not land.
                channel.metrics.record_connect_failure();
            }
            return Err(err);
        }
        return Ok(());
    }
}

/// Routes one server frame. Only aibot_msg_callback ever escalates back to the
/// loop's caller (as a handler infra failure); events are logged + acked and
/// everything else is silently dropped.
async fn dispatch_frame(
    ctx: &CancellationToken,
    env: &FrameEnvelope,
    handler: &InboundHandler,
    sender: &WsSender,
    bot_id: &str,
    bot_display_name: &str,
) -> anyhow::Result<()> {
    if ctx.is_cancelled() {
        return Ok(());
    }
    match env.cmd.as_str() {
        CMD_MSG_CALLBACK => {
            let mc: AibotMsgCallback = match serde_json::from_value(env.body.clone()) {
                Ok(mc) => mc,
                Err(e) => {
                    tracing::warn!(error = %e, "wecom: bad aibot_msg_callback body");
                    return Ok(());
                }
            };
            let text_opt = mc.own_text();
            // Traced with the RESOLVED body, not the raw text field: that field
            // is empty for every voice, media and 图文混排 callback, so tracing
            // it would print len=0 for exactly the messages an operator turned
            // tracing on to look at.
            trace_inbound(&mc, text_opt.as_deref().unwrap_or(""));
            let msg = channel_message_from_callback(
                bot_id,
                bot_display_name,
                &mc,
                text_opt.as_deref().unwrap_or(""),
                &env.headers.req_id,
            );
            if text_opt.is_none() {
                // Nothing in this message can be read: a kind the adapter does
                // not know (a location card), or a known kind that arrived
                // without the one field that makes it usable. Silence reads as
                // a broken bot, so answer the same chat with a one-line receipt
                // and stop. Best-effort: a send failure degrades to the prior
                // silent drop.
                tracing::debug!(
                    msg_type = %mc.msg_type,
                    msg_id = %mc.msg_id,
                    "wecom: unsupported message kind, replying with a receipt"
                );
                let chat_type_int = aibot_chat_type_from_channel(&msg.source.chat_type);
                if let Err(e) = sender
                    .send_text(
                        &msg.source.chat_id,
                        chat_type_int,
                        UNSUPPORTED_MSG_TYPE_RECEIPT,
                    )
                    .await
                {
                    tracing::debug!(
                        error = %e,
                        msg_id = %mc.msg_id,
                        "wecom: unsupported-kind receipt send failed"
                    );
                }
                return Ok(());
            }
            handler.call(ctx.clone(), msg).await
        }
        CMD_EVENT_CALLBACK => {
            let ec: AibotEventCallback = match serde_json::from_value(env.body.clone()) {
                Ok(ec) => ec,
                Err(e) => {
                    tracing::warn!(error = %e, "wecom: bad aibot_event_callback body");
                    return Ok(());
                }
            };
            if ec.event.event_type == EVENT_DISCONNECTED {
                // Another connection displaced ours. Return so the Supervisor
                // can backoff and reconnect (which will in turn displace THAT
                // one — the last writer wins).
                anyhow::bail!("wecom: received disconnected_event (superseded)");
            }
            tracing::debug!(event_type = %ec.event.event_type, "wecom: event");
            Ok(())
        }
        CMD_SERVER_PING => {
            // Server-initiated ping (rare per the docs, but handle defensively).
            sender
                .write(json!({
                    "cmd": CMD_PONG,
                    "headers": { "req_id": env.headers.req_id },
                }))
                .await
                .map_err(|e| anyhow::anyhow!("wecom: pong: {e}"))
        }
        CMD_PONG => Ok(()),
        _ => {
            // Anonymous ack frames (empty cmd) for our writes. Most are
            // errcode=0 no-ops, but aibot_send_msg / aibot_upload_media_* can
            // reject with a non-zero errcode. Hand it to whoever wrote the
            // frame, if anybody is waiting. An unclaimed ack is not an error —
            // the pushes that do not wait for a verdict share this connection.
            if sender.route_response(env) {
                return Ok(());
            }
            if env.err_code != 0 {
                tracing::warn!(
                    errcode = env.err_code,
                    errmsg = %env.err_msg,
                    req_id = %env.headers.req_id,
                    "wecom: server ack error"
                );
            }
            Ok(())
        }
    }
}

/// Sends heartbeat frames every [`PING_INTERVAL`] until the token is
/// cancelled. A write failure surfaces on the next read error path; we log it
/// here but do not tear the loop down ourselves.
async fn ping_loop(ctx: CancellationToken, sender: Arc<WsSender>) {
    // First tick after a full interval, matching Go's time.NewTicker.
    let mut interval =
        tokio::time::interval_at(tokio::time::Instant::now() + PING_INTERVAL, PING_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ctx.cancelled() => return,
            _ = interval.tick() => {
                if let Err(e) = sender.send_ping().await {
                    tracing::warn!(error = %e, "wecom: ping write failed");
                }
            }
        }
    }
}

/// Bundles the shared dependencies the wecom Factory closes over. The engine
/// inbound handler is supplied per-build via [`Config::handler`]; the
/// credentials resolver decrypts the stored secret.
///
/// Port note: Go's nilable fields become Options; boot wires ONE registry
/// instance shared with the outbound paths, and a None metrics discards every
/// counter.
#[derive(Clone, Default)]
pub struct ChannelDeps {
    pub credentials: Option<Arc<dyn CredentialsResolver>>,
    pub senders: Option<Arc<SendersRegistry>>,
    pub metrics: Option<Arc<dyn Metrics>>,
    /// Overrides the default dialer. Tests point it at a local server;
    /// production leaves this None.
    pub dialer: Option<Arc<dyn Dialer>>,
    /// Overrides [`DEFAULT_WS_URL`]. Same test-only intent as dialer.
    pub ws_url: String,
}

/// Registers the per-installation wecom smart-bot Factory so the engine
/// Supervisor builds + supervises one WecomChannel per active installation.
/// "Adding wecom smart-bot inbound" is this call plus the adapter — no engine
/// edit.
pub fn register_wecom(reg: &cordy_channel::Registry, deps: ChannelDeps) {
    reg.register(crate::type_wecom(), new_wecom_factory(deps));
}

fn new_wecom_factory(deps: ChannelDeps) -> Factory {
    Arc::new(move |cfg: Config| -> FactoryFuture {
        let deps = deps.clone();
        Box::pin(async move {
            let credentials = deps
                .credentials
                .clone()
                .ok_or_else(|| anyhow::anyhow!("wecom: credentials resolver missing"))?;
            let ic: InstallConfig = if cfg.raw.is_null() {
                InstallConfig::default()
            } else {
                serde_json::from_value(cfg.raw.clone())
                    .map_err(|e| anyhow::anyhow!("wecom: decode installation config: {e}"))?
            };
            if ic.bot_id.is_empty() {
                anyhow::bail!("wecom: installation config missing bot_id");
            }
            let inst = Installation {
                bot_id: ic.bot_id.clone(),
                secret_encrypted: ic.secret_encrypted(),
                ..Default::default()
            };
            let creds = credentials
                .credentials(&inst)
                .await
                .map_err(|e| anyhow::anyhow!("wecom: decrypt secret: {e}"))?;
            Ok(Arc::new(WecomChannel {
                installation_id: cfg.id.unwrap_or(Uuid::nil()),
                bot_id: creds.bot_id,
                secret: creds.secret,
                bot_display_name: ic.bot_display_name,
                handler: cfg.handler,
                dialer: deps.dialer.clone().unwrap_or_else(default_dialer),
                ws_url: deps.ws_url.clone(),
                senders: deps.senders.clone(),
                generation: cfg.generation.unwrap_or_else(LeaseGeneration::standalone),
                metrics: or_nop_metrics(deps.metrics.clone()),
            }) as BuiltChannel)
        })
    })
}

fn default_dialer() -> Arc<dyn Dialer> {
    // Production dialer. Built lazily so a broken trust store surfaces as a
    // connect-time error rather than a boot-time panic.
    Arc::new(DefaultDialer::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::InstallationCredentials;
    use cordy_channel::message::InboundMessage as ChannelInboundMessage;
    use serde_json::Value;
    use std::sync::Mutex as StdMutex;

    struct FakeCredentials;
    #[async_trait]
    impl CredentialsResolver for FakeCredentials {
        async fn credentials(
            &self,
            inst: &Installation,
        ) -> anyhow::Result<InstallationCredentials> {
            Ok(InstallationCredentials {
                bot_id: inst.bot_id.clone(),
                secret: format!("secret-for-{}", inst.bot_id),
            })
        }
    }

    /// A scripted connection: written subscribe/send frames get an automatic
    /// zero-ack enqueued, and further reads come from the script.
    struct ScriptedConn {
        reads: StdMutex<Vec<Vec<u8>>>,
        writes: StdMutex<Vec<Value>>,
    }

    impl ScriptedConn {
        fn new(script: Vec<Value>) -> Arc<Self> {
            Arc::new(Self {
                reads: StdMutex::new(
                    script
                        .into_iter()
                        .map(|v| serde_json::to_vec(&v).unwrap())
                        .collect(),
                ),
                writes: StdMutex::new(Vec::new()),
            })
        }

        fn writes(&self) -> Vec<Value> {
            self.writes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    #[async_trait]
    impl WsConn for ScriptedConn {
        async fn read_message(&self, _d: Option<Instant>) -> anyhow::Result<Vec<u8>> {
            let mut q = self.reads.lock().unwrap_or_else(|e| e.into_inner());
            if q.is_empty() {
                anyhow::bail!("script exhausted");
            }
            Ok(q.remove(0))
        }

        async fn write_message(&self, data: String, _d: Option<Instant>) -> anyhow::Result<()> {
            let frame: Value = serde_json::from_str(&data)?;
            self.writes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(frame.clone());
            // Auto-ack any frame carrying a req_id, mirroring the server.
            let req_id = frame["headers"]["req_id"].as_str().unwrap_or("");
            if !req_id.is_empty() {
                self.reads.lock().unwrap_or_else(|e| e.into_inner()).insert(
                    0,
                    serde_json::to_vec(&json!({
                        "headers": {"req_id": req_id},
                        "errcode": 0,
                    }))
                    .unwrap(),
                );
            }
            Ok(())
        }

        async fn close(&self) {}
    }

    struct ScriptedDialer(Arc<ScriptedConn>);

    #[async_trait]
    impl Dialer for ScriptedDialer {
        async fn dial(
            &self,
            _ctx: &CancellationToken,
            _url: &str,
        ) -> anyhow::Result<Box<dyn WsConn>> {
            Ok(Box::new(ScriptedHandle(self.0.clone())))
        }
    }

    struct ScriptedHandle(Arc<ScriptedConn>);

    #[async_trait]
    impl WsConn for ScriptedHandle {
        async fn read_message(&self, d: Option<Instant>) -> anyhow::Result<Vec<u8>> {
            self.0.read_message(d).await
        }
        async fn write_message(&self, data: String, d: Option<Instant>) -> anyhow::Result<()> {
            self.0.write_message(data, d).await
        }
        async fn close(&self) {
            self.0.close().await
        }
    }

    fn test_channel(dialer: Arc<dyn Dialer>, handler: InboundHandler) -> WecomChannel {
        WecomChannel {
            installation_id: Uuid::now_v7(),
            bot_id: "bot-1".to_string(),
            secret: "sec".to_string(),
            bot_display_name: "Cordy Bot".to_string(),
            handler: Some(handler),
            dialer,
            ws_url: String::new(),
            senders: None,
            generation: LeaseGeneration::standalone(),
            metrics: Arc::new(crate::metrics::NopMetrics),
        }
    }

    #[tokio::test]
    async fn connect_delivers_a_text_callback_to_the_handler() {
        let (tx, mut rx) = mpsc::channel::<ChannelInboundMessage>(1);
        let handler = InboundHandler::new(move |_ctx, msg| {
            let tx = tx.clone();
            Box::pin(async move {
                tx.send(msg).await.ok();
                Ok(())
            })
        });

        let script = vec![json!({
            "cmd": "aibot_msg_callback",
            "headers": {"req_id": "srv-1"},
            "body": {
                "msgid": "m1", "chattype": "single",
                "from": {"userid": "u1"},
                "msgtype": "text",
                "text": {"content": "hello bot"},
            },
        })];
        let conn = ScriptedConn::new(script);
        let ch = test_channel(Arc::new(ScriptedDialer(conn.clone())), handler);

        let res = ch.connect(CancellationToken::new()).await;
        // The script ends after one message; the read loop reports the
        // transport end, which is the expected shape for a scripted run.
        assert!(res.is_err(), "script exhaustion should end the loop");

        let msg = rx.recv().await.expect("handler should have been called");
        assert_eq!(msg.text, "hello bot");
        assert_eq!(msg.source.sender_id, "u1");
        assert_eq!(msg.source.chat_id, "u1");

        // The subscribe frame went out first, carrying the bot identity.
        let writes = conn.writes();
        assert_eq!(writes[0]["cmd"], json!("aibot_subscribe"));
        assert_eq!(writes[0]["body"]["bot_id"], json!("bot-1"));
    }

    #[tokio::test]
    async fn cancelled_generation_drops_queued_callback_before_dispatch() {
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handler_called = called.clone();
        let handler = InboundHandler::new(move |_ctx, _msg| {
            let called = handler_called.clone();
            Box::pin(async move {
                called.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
        });
        let conn = ScriptedConn::new(Vec::new());
        let sender = WsSender::new(conn);
        let ctx = CancellationToken::new();
        ctx.cancel();
        let env = FrameEnvelope {
            cmd: CMD_MSG_CALLBACK.to_string(),
            headers: crate::ws_frame::FrameHeaders {
                req_id: "cancelled".to_string(),
            },
            body: json!({
                "msgid": "m-cancelled",
                "chattype": "single",
                "from": {"userid": "u1"},
                "msgtype": "text",
                "text": {"content": "do not dispatch"}
            }),
            ..Default::default()
        };

        dispatch_frame(&ctx, &env, &handler, &sender, "bot-1", "Cordy")
            .await
            .unwrap();
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn unsupported_kind_gets_a_receipt_not_a_dispatch() {
        let (tx, mut rx) = mpsc::channel::<ChannelInboundMessage>(1);
        let handler = InboundHandler::new(move |_ctx, msg| {
            let tx = tx.clone();
            Box::pin(async move {
                tx.send(msg).await.ok();
                Ok(())
            })
        });

        let script = vec![json!({
            "cmd": "aibot_msg_callback",
            "headers": {"req_id": "srv-2"},
            "body": {
                "msgid": "m2", "chattype": "group", "chatid": "c1",
                "from": {"userid": "u1"},
                "msgtype": "location",
            },
        })];
        let conn = ScriptedConn::new(script);
        let ch = test_channel(Arc::new(ScriptedDialer(conn.clone())), handler);
        let _ = ch.connect(CancellationToken::new()).await;

        // Nothing reached the handler…
        assert!(rx.try_recv().is_err());
        // …but the receipt went out as an aibot_send_msg markdown push to the
        // group chat.
        let writes = conn.writes();
        let send = writes
            .iter()
            .find(|w| w["cmd"] == json!("aibot_send_msg"))
            .expect("receipt should be pushed");
        assert_eq!(send["body"]["chatid"], json!("c1"));
        assert_eq!(send["body"]["chat_type"], json!(2));
        assert_eq!(
            send["body"]["markdown"]["content"],
            json!(UNSUPPORTED_MSG_TYPE_RECEIPT)
        );
    }

    #[tokio::test]
    async fn disconnected_event_ends_the_connection_with_an_error() {
        let handler = InboundHandler::new(|_ctx, _msg| Box::pin(async { Ok(()) }));
        let script = vec![json!({
            "cmd": "aibot_event_callback",
            "headers": {"req_id": "srv-3"},
            "body": {"event": {"eventtype": "disconnected_event"}},
        })];
        let conn = ScriptedConn::new(script);
        let ch = test_channel(Arc::new(ScriptedDialer(conn)), handler);
        let res = ch.connect(CancellationToken::new()).await.unwrap_err();
        assert!(res.to_string().contains("superseded"), "{res}");
    }

    #[tokio::test]
    async fn refused_subscribe_surfaces_a_credential_rejection() {
        type SharedReqId = std::sync::Arc<std::sync::Mutex<Option<String>>>;
        type SharedReads = std::sync::Arc<std::sync::atomic::AtomicUsize>;
        struct RefusingConn {
            last_req_id: SharedReqId,
            reads: SharedReads,
        }
        #[async_trait]
        impl WsConn for RefusingConn {
            async fn read_message(&self, _d: Option<Instant>) -> anyhow::Result<Vec<u8>> {
                // Deliver the echoing refusal once (the ack carries the req_id
                // captured from the subscribe write), then end the "socket" so
                // the loop cannot spin on a mismatched frame.
                if self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
                    anyhow::bail!("wecom: socket closed");
                }
                let req_id = self
                    .last_req_id
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                    .unwrap_or_default();
                Ok(serde_json::to_vec(&json!({
                    "headers": {"req_id": req_id},
                    "errcode": 40001,
                    "errmsg": "不合法的secret参数",
                }))
                .unwrap())
            }
            async fn write_message(&self, data: String, _: Option<Instant>) -> anyhow::Result<()> {
                let v: serde_json::Value = serde_json::from_str(&data).unwrap_or_default();
                *self.last_req_id.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(v["headers"]["req_id"].as_str().unwrap_or("").to_string());
                Ok(())
            }
            async fn close(&self) {}
        }
        let last_req_id = std::sync::Arc::new(std::sync::Mutex::new(None));
        let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        struct RefusingDialer {
            last_req_id: std::sync::Arc<std::sync::Mutex<Option<String>>>,
            reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        }
        #[async_trait]
        impl Dialer for RefusingDialer {
            async fn dial(
                &self,
                _: &CancellationToken,
                _: &str,
            ) -> anyhow::Result<Box<dyn WsConn>> {
                Ok(Box::new(RefusingConn {
                    last_req_id: std::sync::Arc::clone(&self.last_req_id),
                    reads: std::sync::Arc::clone(&self.reads),
                }))
            }
        }

        // RefusingConn must see the shared state; restructure it as an
        // owned-fields struct built per dial but backed by the shared cells.
        let handler = InboundHandler::new(|_ctx, _msg| Box::pin(async { Ok(()) }));
        let ch = test_channel(
            Arc::new(RefusingDialer {
                last_req_id: std::sync::Arc::clone(&last_req_id),
                reads: std::sync::Arc::clone(&reads),
            }),
            handler,
        );
        let err = ch.connect(CancellationToken::new()).await.unwrap_err();
        assert!(
            crate::credential_probe::is_credentials_rejected(&err),
            "{err}"
        );
    }

    #[tokio::test]
    async fn connect_validates_configuration_before_dialing() {
        let handler = InboundHandler::new(|_ctx, _msg| Box::pin(async { Ok(()) }));
        let mut ch = test_channel(
            Arc::new(ScriptedDialer(ScriptedConn::new(vec![]))),
            handler.clone(),
        );
        ch.bot_id = String::new();
        assert!(ch.connect(CancellationToken::new()).await.is_err());

        let ch = WecomChannel {
            handler: None,
            ..test_channel(Arc::new(ScriptedDialer(ScriptedConn::new(vec![]))), handler)
        };
        assert!(ch.connect(CancellationToken::new()).await.is_err());
    }

    #[test]
    fn send_seam_is_honestly_unsupported() {
        let handler = InboundHandler::new(|_ctx, _msg| Box::pin(async { Ok(()) }));
        let ch = test_channel(Arc::new(ScriptedDialer(ScriptedConn::new(vec![]))), handler);
        let err = futures_executor_block(ch.send(Default::default()))
            .expect_err("send must be unsupported");
        assert!(is_send_not_supported(&err));
    }

    fn futures_executor_block<F: std::future::Future>(fut: F) -> F::Output {
        // Minimal single-thread executor for a never-pending future.
        let mut fut = Box::pin(fut);
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        loop {
            match fut.as_mut().poll(&mut cx) {
                std::task::Poll::Ready(v) => return v,
                std::task::Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn factory_builds_from_installation_config() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let deps = ChannelDeps {
            credentials: Some(Arc::new(FakeCredentials)),
            ..Default::default()
        };
        let factory = new_wecom_factory(deps);
        let cfg = Config {
            r#type: crate::type_wecom(),
            raw: json!({"bot_id": "bot-9", "secret_encrypted": null}),
            id: Some(Uuid::now_v7()),
            handler: Some(InboundHandler::new(|_ctx, _msg| Box::pin(async { Ok(()) }))),
            generation: None,
        };
        let built = rt.block_on(factory(cfg)).unwrap();
        assert_eq!(built.r#type(), crate::type_wecom());
        assert_eq!(
            built.capabilities(),
            Capability::TEXT | Capability::ATTACHMENT
        );
    }

    #[tokio::test]
    async fn factory_refuses_missing_resolver_or_bot_id() {
        let factory = new_wecom_factory(ChannelDeps::default());
        let cfg = Config {
            r#type: crate::type_wecom(),
            raw: json!({"bot_id": "b"}),
            id: None,
            handler: None,
            generation: None,
        };
        assert!(factory(cfg).await.is_err());

        let deps = ChannelDeps {
            credentials: Some(Arc::new(FakeCredentials)),
            ..Default::default()
        };
        let factory = new_wecom_factory(deps);
        let cfg = Config {
            r#type: crate::type_wecom(),
            raw: json!({}),
            id: None,
            handler: None,
            generation: None,
        };
        let err = match factory(cfg).await {
            Ok(_) => panic!("expected factory error"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("missing bot_id"), "{err}");
    }

    #[test]
    fn constants_match_go_values() {
        assert_eq!(DEFAULT_WS_URL, "wss://openws.work.weixin.qq.com");
        assert_eq!(PING_INTERVAL, Duration::from_secs(30));
        assert_eq!(SUBSCRIBE_TIMEOUT, Duration::from_secs(10));
        assert_eq!(READ_DEADLINE, Duration::from_secs(90));
        assert_eq!(WRITE_DEADLINE, Duration::from_secs(10));
        assert_eq!(HANDSHAKE_TIMEOUT, Duration::from_secs(15));
        assert_eq!(CALLBACK_QUEUE_DEPTH, 64);
        assert_eq!(
            UNSUPPORTED_MSG_TYPE_RECEIPT,
            "抱歉，我暂时无法处理这类消息。"
        );
    }
}
