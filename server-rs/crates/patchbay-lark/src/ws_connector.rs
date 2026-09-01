//! WS long-conn connector.
//!
//! WSLongConnConnector is the production EventConnector that holds the Lark
//! long-conn WebSocket open, decodes the binary Frame envelope the
//! open-platform server pushes, and forwards normalized inbound events to the
//! Hub's Dispatcher.
//!
//! Protocol layer (aligned with `larksuite/oapi-sdk-go/v3/ws`):
//!
//! 1. [`EndpointFetcher`] does the POST /callback/ws/endpoint bootstrap. Lark
//!    returns a single-use wss URL with `device_id` + `service_id` query
//!    parameters acting as the credential. The `service_id` is extracted and
//!    used as Frame.service on every outbound frame.
//! 2. Every WebSocket frame is a binary protobuf Frame. JSON envelopes are
//!    wrapped inside Frame.payload for data events; control, ping, pong, ack
//!    frames are pure-binary.
//! 3. The client emits an app-layer ping frame ([`new_ping_frame`]) on the
//!    ping_interval the server returned in the bootstrap ClientConfig.
//!    WebSocket protocol-level PING is NOT used — Lark's server ignores it.
//!    The server can also push pings at any time; we reply with
//!    [`new_pong_frame`].
//! 4. Every data frame requires an ACK back. The ACK reuses the inbound
//!    frame's Headers verbatim (Lark correlates by message_id) and writes a
//!    JSON Response{code:200, ...} as the Payload. We send ACK 200 on
//!    successful Dispatcher emit, 500 when the Dispatcher reported an infra
//!    failure so Lark retries.
//!
//! Ownership of the §4.4 invariant (ctx cancel breaks blocking read):
//!
//! gorilla/websocket.ReadMessage blocks on the TCP socket and does NOT
//! observe a context; the Go port bridges ctx → read interrupt with a
//! watchdog goroutine that calls conn.Close once ctx fires. The Rust port
//! reproduces the same guarantee structurally: the production connection is
//! an actor task that selects the socket against a per-connection
//! cancellation token, and [`WsConn::close`] cancels that token — so a
//! blocked read future resolves immediately (its channel sender dropped) and
//! the socket is torn down. close is idempotent, exactly like gorilla's.
//!
//! PersonalAgent compatibility risk: the official Feishu docs describe
//! long-conn as "supports 企业自建应用 only". PersonalAgent device-flow apps
//! are not listed as supported. If the bootstrap call returns a structured
//! error from Lark, this connector exits run with the error wrapped and the
//! Hub's backoff loop logs it on every retry — making the misconfiguration
//! visible. See PB-2671 review thread for the smoke-test path.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::client::InstallationCredentials;
use crate::connector::{EventConnector, EventEmitter};
use crate::feishu_types::InboundMessage;
use crate::inbound_enricher::Enricher;
use crate::store::Installation;
use crate::types::OpenId;
use crate::ws_chunk_assembler::{parse_chunk_headers, ChunkAssembler};
use crate::ws_frame::{
    new_ack_frame, new_ping_frame, new_pong_frame, unmarshal_frame, FRAME_HEADER_TYPE_KEY,
    FRAME_HEADER_TYPE_PING, FRAME_METHOD_CONTROL,
};

/// WebSocket binary opcode (RFC 6455).
const OPCODE_BINARY: u8 = 2;

/// WsEndpoint is the resolved transport target plus the server-pushed runtime
/// configuration the connector needs to honor (ping cadence, reconnect
/// hints). service_id is parsed out of the wss URL's `service_id` query
/// parameter — it identifies which Lark backend service ID our outbound
/// frames belong to.
#[derive(Debug, Clone, Default)]
pub struct WsEndpoint {
    pub url: String,
    /// Extra HTTP headers for the upgrade request (Go: http.Header).
    pub headers: Vec<(String, String)>,
    pub service_id: i32,
    pub ping_interval: Duration,
    pub reconnect_interval: Duration,
    pub reconnect_nonce: Duration,
    pub reconnect_count: i32,
}

/// Surfaces a clean server-side close (1000/1001 equivalent) so the read loop
/// can exit Ok instead of failing into the Hub's backoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("lark ws connector: server closed connection")]
pub struct ServerClosedConnection;

/// WSConn is the subset of a websocket connection this connector uses.
///
/// Port note: Go's SetReadDeadline / SetWriteDeadline become the connector's
/// own tokio timeouts around each call; concurrent writes are serialized by
/// the implementation (the production actor funnels them through one mpsc
/// channel), replacing Go's explicit writeMu.
#[async_trait]
pub trait WsConn: Send + Sync {
    /// Reads one message; returns (message_type, payload). message_type uses
    /// RFC 6455 opcodes (1=text, 2=binary). An Err whose chain contains
    /// [`ServerClosedConnection`] means the peer closed cleanly.
    async fn read_message(&self) -> anyhow::Result<(u8, Vec<u8>)>;

    /// Writes one message of the given opcode.
    async fn write_message(&self, message_type: u8, data: Vec<u8>) -> anyhow::Result<()>;

    /// Closes the socket; idempotent. MUST cause any blocked
    /// [`read_message`](Self::read_message) to return immediately (the §4.4
    /// invariant).
    async fn close(&self);
}

/// WSDialer is the dialer surface this connector consumes.
#[async_trait]
pub trait WsDialer: Send + Sync {
    async fn dial(
        &self,
        url: &str,
        request_headers: Vec<(String, String)>,
        write_timeout: Duration,
    ) -> anyhow::Result<Arc<dyn WsConn>>;
}

/// EndpointFetcher resolves the per-installation bootstrap response. The
/// implementation is responsible for the POST /callback/ws/endpoint call and
/// surfacing the server-pushed ClientConfig.
#[async_trait]
pub trait EndpointFetcher: Send + Sync {
    async fn endpoint(&self, creds: InstallationCredentials) -> anyhow::Result<WsEndpoint>;
}

/// FrameDecoder turns the JSON payload of a data Frame into either an
/// InboundMessage (`Ok(Some)`) or a no-op (`Ok(None)`). The connector treats
/// a decoder error as per-frame: log + drop, do not tear down the connection.
/// The decoder receives the JSON payload bytes — the outer binary Frame
/// envelope is stripped by the connector.
pub trait FrameDecoder: Send + Sync {
    fn decode(&self, payload: &[u8], inst: &Installation)
        -> anyhow::Result<Option<InboundMessage>>;
}

/// CredentialsProvider supplies the plaintext InstallationCredentials a
/// connector needs for its EndpointFetcher call.
#[async_trait]
pub trait CredentialsProvider: Send + Sync {
    async fn credentials(&self, inst: &Installation) -> anyhow::Result<InstallationCredentials>;
}

/// Wires the connector's dependencies. All injected interfaces are required;
/// nil dependencies cause [`WsLongConnConnector::new`] to return an error
/// rather than producing a connector that would panic at first use. Time
/// fields default at construction.
#[derive(Default)]
pub struct WsConnectorConfig {
    /// Opens the WebSocket transport. Defaults to nothing — required.
    /// Tests inject a fake that points at a local mock server.
    pub dialer: Option<Arc<dyn WsDialer>>,

    /// Resolves the per-installation WS URL + server config (ping interval,
    /// service id) via the bootstrap POST. The connector calls it once per
    /// run, so a transient failure here causes a Hub-level backoff retry
    /// rather than an in-run reconnect storm.
    pub endpoint_fetcher: Option<Arc<dyn EndpointFetcher>>,

    /// Turns a single decoded Frame into either a normalized InboundMessage
    /// (to be emitted upstream) or a "control / heartbeat / unknown" signal
    /// that the connector drops silently. Errors from the decoder do NOT exit
    /// the loop — they log + drop — because one malformed Lark event payload
    /// should not tear down the entire connection.
    pub frame_decoder: Option<Arc<dyn FrameDecoder>>,

    /// Optionally expands a decoded message's body with the context the user
    /// explicitly attached (quoted reply / forwarded bundle) before it is
    /// emitted to the dispatcher. It runs on the inbound read loop, so it is
    /// bounded by enrich_timeout to protect the Lark long-conn ACK budget; on
    /// timeout / fetch failure the enricher degrades to a placeholder rather
    /// than blocking. None disables enrichment (the decoded body is emitted
    /// as-is).
    pub enricher: Option<Arc<dyn Enricher>>,

    /// Caps a single message's enrichment (at most two GetMessage calls). It
    /// MUST stay well under Lark's ~3s long-conn ACK window, since enrichment
    /// runs before the frame is ACKed. Zero defaults to 2 seconds.
    pub enrich_timeout: Duration,

    /// Returns the InstallationCredentials the EndpointFetcher needs.
    /// Typically wraps InstallationService.decrypt_app_secret so the
    /// plaintext secret never sits on the installation row in memory.
    pub credentials_provider: Option<Arc<dyn CredentialsProvider>>,

    /// Fallback cadence for the app-layer ping. In production it is
    /// overridden per-installation by the PingInterval Lark returns in the
    /// bootstrap ClientConfig. Zero defaults to 2 minutes (matches the SDK
    /// default in `larksuite/oapi-sdk-go/v3/ws/client.go`).
    pub ping_interval: Duration,

    /// Bounds a single read. Re-armed before each read; expiry yields a
    /// transient read error which the connector logs and uses to exit,
    /// deferring to the Hub's reconnect backoff. Zero defaults to 6 minutes so
    /// a healthy connection with the 2-minute default ping never trips it.
    pub read_deadline: Duration,

    /// Bounds a single write. Zero defaults to 10s.
    pub write_timeout: Duration,

    /// Bounds how long the chunk assembler holds a partial multi-frame event
    /// before discarding the buffered chunks. Mirrors the SDK's 5-second
    /// default — long enough to absorb pacing across several chunks, short
    /// enough that an abandoned multi-frame event does not leak memory. Zero
    /// defaults to 5 seconds.
    pub chunk_ttl: Duration,
}

impl WsConnectorConfig {
    fn with_defaults(mut self) -> Self {
        if self.ping_interval.is_zero() {
            self.ping_interval = Duration::from_secs(2 * 60);
        }
        if self.read_deadline.is_zero() {
            self.read_deadline = Duration::from_secs(6 * 60);
        }
        if self.write_timeout.is_zero() {
            self.write_timeout = Duration::from_secs(10);
        }
        if self.chunk_ttl.is_zero() {
            self.chunk_ttl = Duration::from_secs(5);
        }
        if self.enrich_timeout.is_zero() {
            self.enrich_timeout = Duration::from_secs(2);
        }
        self
    }
}

/// Validates the supplied config and returns a reusable connector.
pub struct WsLongConnConnector {
    cfg: WsConnectorConfig,
}

impl std::fmt::Debug for WsLongConnConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The config's dyn seams are not Debug; never render credentials.
        f.debug_struct("WsLongConnConnector")
            .finish_non_exhaustive()
    }
}

impl WsLongConnConnector {
    pub fn new(cfg: WsConnectorConfig) -> anyhow::Result<Self> {
        if cfg.dialer.is_none() {
            anyhow::bail!("lark ws connector: Dialer is required");
        }
        if cfg.endpoint_fetcher.is_none() {
            anyhow::bail!("lark ws connector: EndpointFetcher is required");
        }
        if cfg.frame_decoder.is_none() {
            anyhow::bail!("lark ws connector: FrameDecoder is required");
        }
        if cfg.credentials_provider.is_none() {
            anyhow::bail!("lark ws connector: CredentialsProvider is required");
        }
        Ok(Self {
            cfg: cfg.with_defaults(),
        })
    }

    fn write_frame(
        &self,
        conn: &Arc<dyn WsConn>,
        f: &crate::ws_frame::Frame,
    ) -> impl Future<Output = anyhow::Result<()>> + Send + '_ {
        let payload = f.marshal();
        let conn = Arc::clone(conn);
        let write_timeout = self.cfg.write_timeout;
        async move {
            tokio::time::timeout(write_timeout, conn.write_message(OPCODE_BINARY, payload))
                .await
                .map_err(|_| anyhow::anyhow!("write timeout"))??;
            Ok(())
        }
    }

    /// Read loop body shared by run. Returns Ok(()) on clean exits (outer ctx
    /// cancelled, server closed) and Err on connection failures (Hub steps up
    /// backoff).
    #[allow(clippy::too_many_lines)]
    async fn read_loop(
        &self,
        ctx: CancellationToken,
        conn: Arc<dyn WsConn>,
        inst: &Installation,
        creds: InstallationCredentials,
        service_id: i32,
        emit: EventEmitter,
    ) -> anyhow::Result<()> {
        // Per-run chunk assembler. State does not need to outlive a single
        // long-conn session — Lark re-sends multi-frame events from chunk 0
        // after a reconnect — so the assembler is built here and dropped when
        // run returns, which also releases any partial buffers held by an
        // abandoned event.
        let assembler = ChunkAssembler::new(self.cfg.chunk_ttl);

        loop {
            // Re-arm the read deadline before every read so a stalled
            // connection eventually unblocks.
            enum ReadOutcome {
                Cancelled,
                Message(u8, Vec<u8>),
            }
            let outcome = tokio::select! {
                biased;
                _ = ctx.cancelled() => ReadOutcome::Cancelled,
                read = tokio::time::timeout(self.cfg.read_deadline, conn.read_message()) => {
                    match read {
                        Ok(Ok((mt, raw))) => ReadOutcome::Message(mt, raw),
                        Ok(Err(err)) => {
                            if err.downcast_ref::<ServerClosedConnection>().is_some() {
                                tracing::info!(error = %err, "lark ws connector: server closed connection");
                                return Ok(());
                            }
                            if ctx.is_cancelled() {
                                tracing::info!(
                                    close_err = %err,
                                    "lark ws connector: ctx cancelled, read returned"
                                );
                                return Ok(());
                            }
                            return Err(anyhow::anyhow!("read message: {err:#}"));
                        }
                        Err(_elapsed) => {
                            if ctx.is_cancelled() {
                                return Ok(());
                            }
                            return Err(anyhow::anyhow!(
                                "read message: read deadline exceeded after {:?}",
                                self.cfg.read_deadline
                            ));
                        }
                    }
                }
            };
            let (msg_type, raw) = match outcome {
                ReadOutcome::Cancelled => return Ok(()),
                ReadOutcome::Message(mt, raw) => (mt, raw),
            };

            // Lark only sends binary frames. A text frame is a Lark-side
            // schema regression — log + drop to be safe.
            if msg_type != OPCODE_BINARY {
                tracing::warn!(
                    msg_type,
                    len = raw.len(),
                    "lark ws connector: dropped non-binary frame"
                );
                continue;
            }

            let frame = match unmarshal_frame(&raw) {
                Ok(f) => f,
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        raw_len = raw.len(),
                        "lark ws connector: frame protobuf decode failed"
                    );
                    continue;
                }
            };

            // Control frames carry ping / pong / config updates. We only have
            // to act on pings (reply with a pong); pongs and config payloads
            // are accepted silently.
            if frame.method == FRAME_METHOD_CONTROL {
                if frame.header_value(FRAME_HEADER_TYPE_KEY) == FRAME_HEADER_TYPE_PING {
                    if let Err(werr) = self.write_frame(&conn, &new_pong_frame(service_id)).await {
                        tracing::warn!(error = %werr, "lark ws connector: pong write failed");
                    }
                }
                continue;
            }

            // Multi-frame events: stash the chunk and skip ACK until the full
            // payload has arrived. This mirrors the SDK's combine() behaviour
            // — the SDK does NOT ACK partial chunks; Lark reconciles delivery
            // on its side using sum/seq, so ACKing partials would tell Lark
            // "we got it" before we actually have the assembled payload.
            let (sum, seq, msg_id) = parse_chunk_headers(&frame);
            let payload: Vec<u8> = if sum > 1 {
                let frame_payload = frame.payload.clone().unwrap_or_default();
                let Some(assembled) = assembler.admit(&msg_id, sum, seq, &frame_payload) else {
                    tracing::debug!(
                        message_id = %msg_id,
                        seq,
                        sum,
                        pending = assembler.pending_count(),
                        "lark ws connector: partial chunk buffered"
                    );
                    continue;
                };
                tracing::debug!(
                    message_id = %msg_id,
                    chunks = sum,
                    bytes = assembled.len(),
                    "lark ws connector: chunk reassembly complete"
                );
                assembled
            } else {
                frame.payload.clone().unwrap_or_default()
            };

            // Data frames: hand the (possibly reassembled) JSON payload to
            // the decoder, emit if it resolved to a message, and ACK back.
            let decoder = self.cfg.frame_decoder.as_ref().expect("validated at new");
            let msg = match decoder.decode(&payload, inst) {
                Err(derr) => {
                    tracing::warn!(
                        error = %derr,
                        payload_len = payload.len(),
                        "lark ws connector: frame decode failed"
                    );
                    // A decode failure still gets a 200 ACK: the message is
                    // valid wire-wise, we just can't act on it. NACKing would
                    // trigger a Lark-side retry storm of a payload we've
                    // already proven we can't parse.
                    if let Err(werr) = self.write_frame(&conn, &new_ack_frame(&frame, true)).await {
                        tracing::warn!(error = %werr, "lark ws connector: ack-after-decode-error write failed");
                        return Err(anyhow::anyhow!("write ack: {werr:#}"));
                    }
                    continue;
                }
                Ok(None) => {
                    // Heartbeat / unhandled event type. ACK 200 so the server
                    // stops sending it; the decoder owns the "what we handle"
                    // policy.
                    if let Err(werr) = self.write_frame(&conn, &new_ack_frame(&frame, true)).await {
                        tracing::warn!(error = %werr, "lark ws connector: ack-after-drop write failed");
                        return Err(anyhow::anyhow!("write ack: {werr:#}"));
                    }
                    continue;
                }
                Ok(Some(msg)) => msg,
            };

            // Enrich the decoded body with explicitly-attached context
            // (quoted reply / forwarded bundle) before emitting. This runs
            // before the frame ACK, so it is bounded by enrich_timeout and
            // degrades to the un-enriched body on timeout rather than
            // blocking the pipeline. Most messages need no enrichment and
            // return immediately without any network call.
            let msg = match self.cfg.enricher.as_ref() {
                Some(enricher) => {
                    match tokio::time::timeout(
                        self.cfg.enrich_timeout,
                        enricher.enrich(msg.clone(), creds.clone()),
                    )
                    .await
                    {
                        Ok(enriched) => enriched,
                        Err(_elapsed) => {
                            tracing::warn!(
                                message_id = %msg.message_id,
                                timeout_ms = self.cfg.enrich_timeout.as_millis() as u64,
                                "lark ws connector: enrichment timed out; emitting decoded body"
                            );
                            msg
                        }
                    }
                }
                None => msg,
            };

            let event_id = msg.event_id.clone();
            if let Err(emit_err) = emit(ctx.clone(), msg).await {
                // Infra failure from Dispatcher (DB down, etc.). NACK so Lark
                // retries this event on a healthy replica; then return so the
                // Hub backs off and reconnects.
                if let Err(werr) = self.write_frame(&conn, &new_ack_frame(&frame, false)).await {
                    tracing::warn!(error = %werr, "lark ws connector: nack write failed");
                }
                tracing::error!(
                    event_id = %event_id,
                    error = %emit_err,
                    "lark ws connector: emit infra error"
                );
                return Err(anyhow::anyhow!("dispatch: {emit_err:#}"));
            }
            if let Err(werr) = self.write_frame(&conn, &new_ack_frame(&frame, true)).await {
                tracing::warn!(error = %werr, "lark ws connector: ack write failed");
                return Err(anyhow::anyhow!("write ack: {werr:#}"));
            }
        }
    }
}

use std::future::Future;

#[async_trait]
impl EventConnector for WsLongConnConnector {
    /// Satisfies EventConnector. Opens one WebSocket session, reads binary
    /// Frame envelopes until either the ctx is cancelled or the connection
    /// errors, and returns. Ok = clean exit; Err = connection failed (Hub
    /// steps up backoff).
    #[allow(clippy::too_many_lines)]
    async fn run(
        &self,
        ctx: CancellationToken,
        inst: Installation,
        emit: EventEmitter,
        runtime_health: Option<patchbay_channel::RuntimeHealthReporter>,
    ) -> anyhow::Result<()> {
        let provider = self
            .cfg
            .credentials_provider
            .as_ref()
            .expect("validated at new");
        let fetcher = self
            .cfg
            .endpoint_fetcher
            .as_ref()
            .expect("validated at new");
        let dialer = self.cfg.dialer.as_ref().expect("validated at new");

        let creds = provider
            .credentials(&inst)
            .await
            .map_err(|e| anyhow::anyhow!("resolve credentials: {e:#}"))?;

        let endpoint = fetcher
            .endpoint(creds.clone())
            .await
            .map_err(|e| anyhow::anyhow!("resolve ws endpoint: {e:#}"))?;

        // Server-pushed PingInterval beats the static default; this is the
        // SDK behaviour. A zero (server omitted the field) falls back to our
        // static default so we never degenerate to "ping every 0s".
        let ping_interval = if endpoint.ping_interval.is_zero() {
            self.cfg.ping_interval
        } else {
            endpoint.ping_interval
        };

        let conn = dialer
            .dial(
                &endpoint.url,
                endpoint.headers.clone(),
                self.cfg.write_timeout,
            )
            .await
            .map_err(|e| anyhow::anyhow!("dial ws: {e:#}"))?;
        if let Some(reporter) = &runtime_health {
            reporter.healthy().await;
        }

        // runCtx fans out cancellation to the ping task on EVERY run exit,
        // not just on outer-ctx cancel. A read error or emit-infra failure
        // would otherwise leave the ping task ticking on the outer ctx — and
        // the join below would deadlock.
        let run_ctx = ctx.child_token();

        // Ping loop: app-layer binary ping frames at the server's
        // PingInterval.
        let ping_handle = tokio::spawn(ping_loop(
            run_ctx.clone(),
            Arc::clone(&conn),
            endpoint.service_id,
            ping_interval,
            self.cfg.write_timeout,
        ));

        tracing::info!(
            installation_id = %inst.id,
            app_id = %inst.app_id,
            service_id = endpoint.service_id,
            ping_interval_ms = ping_interval.as_millis() as u64,
            "lark ws connector: connected"
        );

        let result = self
            .read_loop(
                ctx,
                Arc::clone(&conn),
                &inst,
                creds,
                endpoint.service_id,
                emit,
            )
            .await;

        run_ctx.cancel();
        conn.close().await;
        let _ = ping_handle.await;

        result
    }
}

/// Sends a periodic app-layer ping frame on the cadence Lark asked for. The
/// previous implementation used WebSocket protocol PING (WriteControl), which
/// the SDK source confirms Lark ignores.
async fn ping_loop(
    ctx: CancellationToken,
    conn: Arc<dyn WsConn>,
    service_id: i32,
    interval: Duration,
    write_timeout: Duration,
) {
    if interval.is_zero() {
        // A zero / negative interval would tick infinitely; bail out quietly.
        // We logged the chosen interval at connect time.
        ctx.cancelled().await;
        return;
    }
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // tokio's interval fires immediately on the first tick; consume it so the
    // cadence matches Go's time.NewTicker (first tick after one interval).
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = ctx.cancelled() => return,
            _ = ticker.tick() => {
                let f = new_ping_frame(service_id);
                let payload = f.marshal();
                match tokio::time::timeout(write_timeout, conn.write_message(OPCODE_BINARY, payload)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        tracing::warn!(error = %err, "lark ws connector: ping write failed");
                        // Don't tear down here — the read loop will exit on
                        // its own when the conn dies. Closing here would race
                        // with the read loop's own cleanup.
                        return;
                    }
                    Err(_elapsed) => {
                        tracing::warn!("lark ws connector: ping write timed out");
                        return;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Production dialer over tokio-tungstenite (Go: GorillaDialer).
// ---------------------------------------------------------------------------

enum Outbound {
    Message {
        message: tokio_tungstenite::tungstenite::Message,
        result: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
    },
    Close,
}

enum InboundItem {
    Message(u8, Vec<u8>),
    ServerClosed,
}

/// The production connection: an actor task owns the split socket halves and
/// multiplexes reads/writes through two mpsc channels. Writes serialize on
/// the channel (replacing Go's writeMu); close cancels the actor's token,
/// which tears the socket down and makes any blocked reader return
/// immediately — the §4.4 watchdog invariant, structural rather than
/// goroutine-based.
struct ActorWsConn {
    outbound_tx: mpsc::Sender<Outbound>,
    inbound_rx: tokio::sync::Mutex<mpsc::Receiver<anyhow::Result<InboundItem>>>,
    closed: CancellationToken,
    actor: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[async_trait]
impl WsConn for ActorWsConn {
    async fn read_message(&self) -> anyhow::Result<(u8, Vec<u8>)> {
        let mut rx = self.inbound_rx.lock().await;
        match rx.recv().await {
            Some(Ok(InboundItem::Message(mt, raw))) => Ok((mt, raw)),
            Some(Ok(InboundItem::ServerClosed)) | None => Err(ServerClosedConnection.into()),
            Some(Err(err)) => Err(err),
        }
    }

    async fn write_message(&self, message_type: u8, data: Vec<u8>) -> anyhow::Result<()> {
        let msg = match message_type {
            1 => tokio_tungstenite::tungstenite::Message::Text(
                String::from_utf8(data)
                    .map_err(|e| anyhow::anyhow!("text frame is not utf-8: {e}"))?,
            ),
            OPCODE_BINARY => tokio_tungstenite::tungstenite::Message::Binary(data),
            other => anyhow::bail!("unsupported websocket opcode {other}"),
        };
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        self.outbound_tx
            .send(Outbound::Message {
                message: msg,
                result: result_tx,
            })
            .await
            .map_err(|_| anyhow::anyhow!("use of closed connection"))?;
        result_rx
            .await
            .map_err(|_| anyhow::anyhow!("connection closed before write completed"))?
    }

    async fn close(&self) {
        // Idempotent by contract. Cancelling wakes the actor, which closes
        // the sink and drops the socket; the inbound sender drops with it, so
        // any blocked read_message resolves immediately.
        self.closed.cancel();
        let _ = self.outbound_tx.try_send(Outbound::Close);
        let Some(mut actor) = self.actor.lock().await.take() else {
            return;
        };
        if tokio::time::timeout(Duration::from_secs(2), &mut actor)
            .await
            .is_err()
        {
            actor.abort();
            let _ = actor.await;
        }
    }
}

/// The production WsDialer over tokio-tungstenite (Go: GorillaDialer).
/// Supports the same explicit HTTP CONNECT proxy used by
/// `PATCHBAY_LARK_WS_PROXY_URL`; the target TLS and WebSocket handshakes happen
/// only after the proxy acknowledges the tunnel.
pub struct TungsteniteDialer {
    handshake_timeout: Duration,
    proxy_url: Option<String>,
}

impl TungsteniteDialer {
    pub fn new() -> Self {
        Self {
            handshake_timeout: Duration::from_secs(15),
            proxy_url: None,
        }
    }

    pub fn with_proxy_url(proxy_url: impl Into<String>) -> Self {
        let proxy_url = proxy_url.into();
        let proxy_url = (!proxy_url.trim().is_empty()).then(|| proxy_url.trim().to_owned());
        Self {
            handshake_timeout: Duration::from_secs(15),
            proxy_url,
        }
    }
}

impl Default for TungsteniteDialer {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WsDialer for TungsteniteDialer {
    async fn dial(
        &self,
        url: &str,
        request_headers: Vec<(String, String)>,
        write_timeout: Duration,
    ) -> anyhow::Result<Arc<dyn WsConn>> {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        let mut request = url
            .to_string()
            .into_client_request()
            .map_err(|e| anyhow::anyhow!("invalid ws url {url:?}: {e}"))?;
        for (k, v) in request_headers {
            let name = tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| anyhow::anyhow!("invalid header name {k:?}: {e}"))?;
            let value = tokio_tungstenite::tungstenite::http::HeaderValue::from_str(&v)
                .map_err(|e| anyhow::anyhow!("invalid header value {v:?}: {e}"))?;
            request.headers_mut().insert(name, value);
        }

        let handshake = async {
            if let Some(proxy_url) = &self.proxy_url {
                let tunnel = open_http_connect_tunnel(proxy_url, url).await?;
                tokio_tungstenite::client_async_tls_with_config(request, tunnel, None, None)
                    .await
                    .map_err(|error| anyhow::anyhow!("ws handshake failed: {error}"))
            } else {
                connect_async(request)
                    .await
                    .map_err(|error| anyhow::anyhow!("ws handshake failed: {error}"))
            }
        };
        let (ws, _resp) = tokio::time::timeout(self.handshake_timeout, handshake)
            .await
            .map_err(|_| {
                anyhow::anyhow!("ws handshake timed out after {:?}", self.handshake_timeout)
            })??;

        let (mut sink, mut stream) = ws.split();
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<Outbound>(64);
        let (inbound_tx, inbound_rx) = mpsc::channel::<anyhow::Result<InboundItem>>(64);
        let closed = CancellationToken::new();

        let closed_for_actor = closed.clone();
        let actor = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = closed_for_actor.cancelled() => {
                        // Best-effort close frame, bounded so a dead socket
                        // cannot hang teardown; dropping the halves shuts the
                        // TCP connection regardless.
                        let _ = tokio::time::timeout(
                            Duration::from_secs(1),
                            sink.close(),
                        ).await;
                        break;
                    }
                    cmd = outbound_rx.recv() => {
                        match cmd {
                            Some(Outbound::Message { message, result }) => {
                                let write = tokio::time::timeout(write_timeout, sink.send(message)).await;
                                let (reply, failed) = match write {
                                    Ok(Ok(())) => (Ok(()), false),
                                    Ok(Err(err)) => (Err(anyhow::anyhow!("ws write: {err}")), true),
                                    Err(_) => (Err(anyhow::anyhow!("ws write timed out after {write_timeout:?}")), true),
                                };
                                let _ = result.send(reply);
                                if failed {
                                    break;
                                }
                            }
                            Some(Outbound::Close) | None => {
                                let _ = tokio::time::timeout(
                                    Duration::from_secs(1),
                                    sink.close(),
                                ).await;
                                break;
                            }
                        }
                    }
                    item = stream.next() => {
                        match item {
                            None => {
                                let _ = inbound_tx.send(Ok(InboundItem::ServerClosed)).await;
                                break;
                            }
                            Some(Err(err)) => {
                                use tokio_tungstenite::tungstenite::Error as WsErrorKind;
                                let mapped: anyhow::Error = match &err {
                                    WsErrorKind::ConnectionClosed | WsErrorKind::AlreadyClosed => {
                                        ServerClosedConnection.into()
                                    }
                                    _ => anyhow::anyhow!("ws read: {err}"),
                                };
                                let _ = inbound_tx.send(Err(mapped)).await;
                                break;
                            }
                            Some(Ok(msg)) => match msg {
                                tokio_tungstenite::tungstenite::Message::Binary(b) => {
                                    if inbound_tx.send(Ok(InboundItem::Message(OPCODE_BINARY, b))).await.is_err() {
                                        break;
                                    }
                                }
                                tokio_tungstenite::tungstenite::Message::Text(t) => {
                                    if inbound_tx.send(Ok(InboundItem::Message(1, t.into_bytes()))).await.is_err() {
                                        break;
                                    }
                                }
                                // Protocol-level ping/pong/close frames. Lark
                                // ignores protocol pings; tungstenite answers
                                // pings automatically on the next flush. A
                                // Close frame precedes ConnectionClosed, so
                                // surface it as the clean-close signal.
                                tokio_tungstenite::tungstenite::Message::Close(_) => {
                                    let _ = inbound_tx.send(Ok(InboundItem::ServerClosed)).await;
                                    break;
                                }
                                tokio_tungstenite::tungstenite::Message::Ping(_)
                                | tokio_tungstenite::tungstenite::Message::Pong(_)
                                | tokio_tungstenite::tungstenite::Message::Frame(_) => {}
                            },
                        }
                    }
                }
            }
        });

        Ok(Arc::new(ActorWsConn {
            outbound_tx,
            inbound_rx: tokio::sync::Mutex::new(inbound_rx),
            closed,
            actor: tokio::sync::Mutex::new(Some(actor)),
        }))
    }
}

const PROXY_RESPONSE_MAX_BYTES: usize = 16 * 1024;

async fn open_http_connect_tunnel(
    proxy_url: &str,
    target_url: &str,
) -> anyhow::Result<tokio::net::TcpStream> {
    use base64::Engine as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let proxy = url::Url::parse(proxy_url)
        .map_err(|error| anyhow::anyhow!("lark ws proxy URL is invalid: {error}"))?;
    if proxy.scheme() != "http" {
        anyhow::bail!("lark ws proxy URL must use http");
    }
    let proxy_host = proxy
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("lark ws proxy URL has no host"))?;
    let proxy_port = proxy.port_or_known_default().unwrap_or(80);
    let proxy_addr = format_host_port(proxy_host, proxy_port);

    let target = url::Url::parse(target_url)
        .map_err(|error| anyhow::anyhow!("lark ws target URL is invalid: {error}"))?;
    if !matches!(target.scheme(), "ws" | "wss") {
        anyhow::bail!("lark ws target URL must use ws or wss");
    }
    let target_host = target
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("lark ws target URL has no host"))?;
    let target_port = target
        .port_or_known_default()
        .ok_or_else(|| anyhow::anyhow!("lark ws target URL has no port"))?;
    let authority = format_host_port(target_host, target_port);

    let mut stream = tokio::net::TcpStream::connect(&proxy_addr)
        .await
        .map_err(|error| anyhow::anyhow!("connect to Lark WS proxy {proxy_addr}: {error}"))?;
    let mut request = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n"
    );
    if !proxy.username().is_empty() || proxy.password().is_some() {
        let username = percent_decode_userinfo(proxy.username())?;
        let password = percent_decode_userinfo(proxy.password().unwrap_or_default())?;
        let credentials = format!("{username}:{password}");
        let encoded = base64::engine::general_purpose::STANDARD.encode(credentials.as_bytes());
        request.push_str("Proxy-Authorization: Basic ");
        request.push_str(&encoded);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| anyhow::anyhow!("write Lark WS proxy CONNECT request: {error}"))?;

    let mut response = Vec::with_capacity(1024);
    while !response.windows(4).any(|window| window == b"\r\n\r\n") {
        if response.len() >= PROXY_RESPONSE_MAX_BYTES {
            anyhow::bail!("Lark WS proxy CONNECT response exceeded byte limit");
        }
        let remaining = PROXY_RESPONSE_MAX_BYTES - response.len();
        let mut chunk = [0_u8; 1024];
        let chunk_len = remaining.min(chunk.len());
        let read = stream
            .read(&mut chunk[..chunk_len])
            .await
            .map_err(|error| anyhow::anyhow!("read Lark WS proxy CONNECT response: {error}"))?;
        if read == 0 {
            anyhow::bail!("Lark WS proxy closed before CONNECT completed");
        }
        response.extend_from_slice(&chunk[..read]);
    }
    let status_line = response
        .split(|byte| *byte == b'\n')
        .next()
        .and_then(|line| std::str::from_utf8(line).ok())
        .map(str::trim)
        .unwrap_or_default();
    let mut status_parts = status_line.split_ascii_whitespace();
    let version = status_parts.next();
    let status = status_parts
        .next()
        .and_then(|status| status.parse::<u16>().ok());
    if !matches!(version, Some("HTTP/1.0" | "HTTP/1.1")) || status != Some(200) {
        anyhow::bail!("Lark WS proxy CONNECT refused with status {status:?}");
    }
    Ok(stream)
}

fn format_host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn percent_decode_userinfo(value: &str) -> anyhow::Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            anyhow::bail!("lark ws proxy credentials contain invalid percent encoding");
        }
        let high = hex_digit(bytes[index + 1])?;
        let low = hex_digit(bytes[index + 2])?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    String::from_utf8(decoded)
        .map_err(|_| anyhow::anyhow!("lark ws proxy credentials are not valid UTF-8"))
}

fn hex_digit(value: u8) -> anyhow::Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => anyhow::bail!("lark ws proxy credentials contain invalid percent encoding"),
    }
}

use tokio_tungstenite::connect_async;

/// Convenience constructor mirroring Go's CredentialsProviderFunc: adapts a
/// plain async closure into the trait object.
pub fn credentials_provider_fn<F, Fut>(f: F) -> Arc<dyn CredentialsProvider>
where
    F: Fn(Installation) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = anyhow::Result<InstallationCredentials>> + Send,
{
    struct Wrapper<F>(F);
    #[async_trait]
    impl<F, Fut> CredentialsProvider for Wrapper<F>
    where
        F: Fn(Installation) -> Fut + Send + Sync,
        Fut: std::future::Future<Output = anyhow::Result<InstallationCredentials>> + Send,
    {
        async fn credentials(
            &self,
            inst: &Installation,
        ) -> anyhow::Result<InstallationCredentials> {
            (self.0)(inst.clone()).await
        }
    }
    Arc::new(Wrapper(f))
}

/// Convenience constructor mirroring Go's EndpointFetcherFunc.
pub fn endpoint_fetcher_fn<F, Fut>(f: F) -> Arc<dyn EndpointFetcher>
where
    F: Fn(InstallationCredentials) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = anyhow::Result<WsEndpoint>> + Send,
{
    struct Wrapper<F>(F);
    #[async_trait]
    impl<F, Fut> EndpointFetcher for Wrapper<F>
    where
        F: Fn(InstallationCredentials) -> Fut + Send + Sync,
        Fut: std::future::Future<Output = anyhow::Result<WsEndpoint>> + Send,
    {
        async fn endpoint(&self, creds: InstallationCredentials) -> anyhow::Result<WsEndpoint> {
            (self.0)(creds).await
        }
    }
    Arc::new(Wrapper(f))
}

/// Convenience constructor mirroring Go's FrameDecoderFunc.
pub fn frame_decoder_fn<F>(f: F) -> Arc<dyn FrameDecoder>
where
    F: Fn(&[u8], &Installation) -> anyhow::Result<Option<InboundMessage>> + Send + Sync + 'static,
{
    struct Wrapper<F>(F);
    impl<F> FrameDecoder for Wrapper<F>
    where
        F: Fn(&[u8], &Installation) -> anyhow::Result<Option<InboundMessage>> + Send + Sync,
    {
        fn decode(
            &self,
            payload: &[u8],
            inst: &Installation,
        ) -> anyhow::Result<Option<InboundMessage>> {
            (self.0)(payload, inst)
        }
    }
    Arc::new(Wrapper(f))
}

/// Unused-import guard: OpenId re-exported for downstream wiring parity with
/// the Go package's hub.go helpers.
#[allow(unused)]
fn _open_id_marker(_: OpenId) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_match_go_values() {
        let cfg = WsConnectorConfig::default().with_defaults();
        assert_eq!(cfg.ping_interval, Duration::from_secs(120));
        assert_eq!(cfg.read_deadline, Duration::from_secs(360));
        assert_eq!(cfg.write_timeout, Duration::from_secs(10));
        assert_eq!(cfg.chunk_ttl, Duration::from_secs(5));
        assert_eq!(cfg.enrich_timeout, Duration::from_secs(2));
    }

    #[test]
    fn new_rejects_missing_dependencies() {
        let err = WsLongConnConnector::new(WsConnectorConfig::default()).unwrap_err();
        assert!(err.to_string().contains("Dialer is required"));

        let cfg = WsConnectorConfig {
            dialer: Some(Arc::new(TungsteniteDialer::new())),
            ..WsConnectorConfig::default()
        };
        let err = WsLongConnConnector::new(cfg).unwrap_err();
        assert!(err.to_string().contains("EndpointFetcher is required"));
    }

    #[test]
    fn zero_server_ping_interval_falls_back_to_config_default() {
        // Mirrors the run-loop branch: endpoint.ping_interval <= 0 → default.
        let endpoint = WsEndpoint::default();
        let cfg = WsConnectorConfig::default().with_defaults();
        let effective = if endpoint.ping_interval.is_zero() {
            cfg.ping_interval
        } else {
            endpoint.ping_interval
        };
        assert_eq!(effective, Duration::from_secs(120));
    }

    #[tokio::test]
    async fn http_connect_proxy_receives_target_and_basic_auth() {
        use base64::Engine as _;
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = listener.local_addr().unwrap();
        let proxy = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .unwrap();
            String::from_utf8(request).unwrap()
        });

        let proxy_url = format!("http://user:p%40ss@{proxy_addr}");
        let tunnel =
            open_http_connect_tunnel(&proxy_url, "wss://open.feishu.cn/callback/ws?service_id=1")
                .await
                .unwrap();
        drop(tunnel);
        let request = proxy.await.unwrap();
        assert!(request
            .starts_with("CONNECT open.feishu.cn:443 HTTP/1.1\r\nHost: open.feishu.cn:443\r\n"));
        let auth = base64::engine::general_purpose::STANDARD.encode(b"user:p@ss");
        assert!(request.contains(&format!("Proxy-Authorization: Basic {auth}\r\n")));
    }

    #[tokio::test]
    async fn proxy_rejects_non_http_scheme_before_dial() {
        let error = open_http_connect_tunnel(
            "socks5://127.0.0.1:1080",
            "wss://open.feishu.cn/callback/ws",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("must use http"));
    }

    #[tokio::test]
    async fn actor_write_waits_for_the_socket_result() {
        let (outbound_tx, mut outbound_rx) = mpsc::channel(1);
        let (_inbound_tx, inbound_rx) = mpsc::channel(1);
        let conn = Arc::new(ActorWsConn {
            outbound_tx,
            inbound_rx: tokio::sync::Mutex::new(inbound_rx),
            closed: CancellationToken::new(),
            actor: tokio::sync::Mutex::new(None),
        });

        let writer = {
            let conn = Arc::clone(&conn);
            tokio::spawn(async move { conn.write_message(OPCODE_BINARY, vec![1, 2, 3]).await })
        };
        let Some(Outbound::Message { result, .. }) = outbound_rx.recv().await else {
            panic!("expected queued websocket message");
        };
        assert!(!writer.is_finished());
        result.send(Ok(())).expect("writer still waiting");
        writer.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn actor_close_cancels_and_joins_the_owner_task() {
        let (outbound_tx, _outbound_rx) = mpsc::channel(1);
        let (_inbound_tx, inbound_rx) = mpsc::channel(1);
        let closed = CancellationToken::new();
        let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_closed = closed.clone();
        let task_observed = observed.clone();
        let actor = tokio::spawn(async move {
            task_closed.cancelled().await;
            task_observed.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let conn = ActorWsConn {
            outbound_tx,
            inbound_rx: tokio::sync::Mutex::new(inbound_rx),
            closed,
            actor: tokio::sync::Mutex::new(Some(actor)),
        };

        conn.close().await;

        assert!(observed.load(std::sync::atomic::Ordering::SeqCst));
        assert!(conn.actor.lock().await.is_none());
    }
}
