//! A serialized writer for one WebSocket connection — port of `ws_sender.go`.
//!
//! WeCom forbids concurrent writes so every outbound frame goes through the
//! same mutex; the ping loop, subscribe handshake, and media uploads all
//! share this writer.
//!
//! Port note: Go's `wsConn`/`Dialer` interfaces become async traits so tests
//! can inject a fake without a real socket; the production implementation
//! rides tokio-tungstenite with the workspace's single ring TLS provider.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tungstenite::client::IntoClientRequest;

use crate::trace::{
    trace_out_attempt, trace_out_fields, trace_out_result, OutTrace, TRACE_STAGE_WRITE,
};
use crate::wecom_channel::{HANDSHAKE_TIMEOUT, WRITE_DEADLINE};
use crate::ws_frame::{
    aibot_chat_type_from_channel, send_msg_text_body, subscribe_body, FrameEnvelope, FrameHeaders,
    CMD_PING, CMD_SEND_MSG, CMD_SUBSCRIBE,
};
use cordy_channel::message::ChatType;
use cordy_channel::LeaseGeneration;

/// Caps the wait for a verdict. WeCom answers in a few hundred milliseconds;
/// past this we assume the ack was lost rather than the frame refused, which
/// matters because the two call for opposite responses.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// The frame went out and no verdict came back. Distinct from a refusal: the
/// message may well have been delivered, so a caller retries at its own risk
/// rather than reporting failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("wecom: timed out waiting for the server verdict")]
pub struct AckTimeoutError;

/// Marks a failure raised by the socket write itself, as opposed to one
/// raised before any byte could leave: a marshal error. Once the write has
/// been entered, the frame may have reached the peer and been acknowledged at
/// the TCP layer before the local side surfaced a failure — a half-closed
/// connection reports "broken pipe" to the writer for bytes the reader
/// already has. So a failure past that point is not proof of non-delivery,
/// and a caller that treats it as one will either deny a delivery that
/// happened or resend a frame WeCom already acted on.
#[derive(Debug, thiserror::Error)]
#[error("wecom: frame write attempted")]
pub struct WriteAttemptedError(#[source] pub anyhow::Error);

/// The run token ended before the verdict came back. From outside the request
/// the two endings — cancelled before the frame was written, cancelled while
/// waiting for its verdict — cannot be told apart, so callers read all of
/// these as "unknown" rather than as failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("wecom: context cancelled")]
pub struct ContextCancelled;

/// A refusal the server stated. Carrying the errcode rather than a string is
/// what lets a caller tell a permanent refusal (bad frame, bot removed from
/// the chat) from a transient one (rate limited) instead of pattern-matching
/// prose.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("wecom: {cmd} rejected errcode={code} errmsg={msg}")]
pub struct WecomApiError {
    pub cmd: String,
    pub code: i64,
    pub msg: String,
}

/// `errors.Is(err, errAckTimeout)` equivalent: walks the anyhow chain.
pub fn is_ack_timeout(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|c| c.downcast_ref::<AckTimeoutError>().is_some())
}

/// `errors.Is(err, errWriteAttempted)` equivalent.
pub fn is_write_attempted(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|c| c.downcast_ref::<WriteAttemptedError>().is_some())
}

/// Reports whether the request ended because the run token was cancelled.
pub fn is_context_cancelled(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|c| c.downcast_ref::<ContextCancelled>().is_some())
}

/// Downcasts the chain for a stated server refusal.
pub fn as_api_error(err: &anyhow::Error) -> Option<&WecomApiError> {
    err.chain().find_map(|c| c.downcast_ref::<WecomApiError>())
}

// ---- transport ----

/// The subset of a WebSocket connection the wecom package uses. Kept minimal
/// so tests can inject a fake without embedding a whole client's surface.
///
/// Port note: gorilla's SetReadDeadline/SetWriteDeadline fold into per-call
/// deadlines; ctx cancellation is bridged at call sites by selecting on the
/// token, which drops the parked read future — the same unblocking Go got
/// from its watchdog closing the socket.
#[async_trait]
pub trait WsConn: Send + Sync {
    /// Reads one text or binary frame payload, bounded by `deadline`.
    /// Transport-level control frames (ping/pong) are answered or skipped
    /// internally and never surface here.
    async fn read_message(&self, deadline: Option<Instant>) -> anyhow::Result<Vec<u8>>;

    /// Writes one text frame payload, bounded by `deadline`.
    async fn write_message(&self, data: String, deadline: Option<Instant>) -> anyhow::Result<()>;

    /// Closes the connection. Idempotent; every later read/write fails.
    async fn close(&self);
}

/// Opens a WebSocket connection to the aibot endpoint. Production uses
/// [`DefaultDialer`]; tests wire a fake pointing at a local server.
#[async_trait]
pub trait Dialer: Send + Sync {
    async fn dial(&self, ctx: &CancellationToken, url: &str) -> anyhow::Result<Box<dyn WsConn>>;
}

type TungsteniteStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Production [`WsConn`] over tokio-tungstenite. The stream sits behind one
/// mutex: reads own it for the duration of a read, writes for the duration of
/// a write — the same single-reader/single-writer contract gorilla enforces,
/// serialized rather than split.
pub struct TungsteniteConn {
    read: Arc<tokio::sync::Mutex<Option<SplitStream<TungsteniteStream>>>>,
    write: Arc<
        tokio::sync::Mutex<
            Option<SplitSink<TungsteniteStream, tokio_tungstenite::tungstenite::Message>>,
        >,
    >,
}

impl TungsteniteConn {
    fn new(stream: TungsteniteStream) -> Self {
        let (write, read) = stream.split();
        Self {
            read: Arc::new(tokio::sync::Mutex::new(Some(read))),
            write: Arc::new(tokio::sync::Mutex::new(Some(write))),
        }
    }

    async fn write_frame(
        &self,
        message: tokio_tungstenite::tungstenite::Message,
        deadline: Option<Instant>,
    ) -> anyhow::Result<()> {
        let mut guard = self.write.lock().await;
        let write_one = async {
            let sink = guard
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("wecom: use of closed connection"))?;
            sink.send(message).await.map_err(anyhow::Error::new)
        };
        match deadline {
            Some(d) => tokio::time::timeout_at(d.into(), write_one)
                .await
                .map_err(|_| anyhow::anyhow!("wecom: write deadline exceeded"))?,
            None => write_one.await,
        }
    }
}

#[async_trait]
impl WsConn for TungsteniteConn {
    async fn read_message(&self, deadline: Option<Instant>) -> anyhow::Result<Vec<u8>> {
        let mut guard = self.read.lock().await;
        let read_one = async {
            let stream = guard
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("wecom: use of closed connection"))?;
            loop {
                let msg = stream
                    .next()
                    .await
                    .transpose()?
                    .ok_or_else(|| anyhow::anyhow!("wecom: websocket stream ended"))?;
                match msg {
                    tokio_tungstenite::tungstenite::Message::Ping(data) => {
                        self.write_frame(
                            tokio_tungstenite::tungstenite::Message::Pong(data),
                            deadline,
                        )
                        .await?;
                    }
                    tokio_tungstenite::tungstenite::Message::Pong(_) => continue,
                    tokio_tungstenite::tungstenite::Message::Close(frame) => {
                        anyhow::bail!(
                            "wecom: websocket closed by peer ({})",
                            frame.map(|f| f.code.to_string()).unwrap_or_default()
                        )
                    }
                    msg if msg.is_text() || msg.is_binary() => return Ok(msg.into_data().to_vec()),
                    _ => continue,
                }
            }
        };
        match deadline {
            Some(d) => tokio::time::timeout_at(d.into(), read_one)
                .await
                .map_err(|_| anyhow::anyhow!("wecom: read deadline exceeded"))?,
            None => read_one.await,
        }
    }

    async fn write_message(&self, data: String, deadline: Option<Instant>) -> anyhow::Result<()> {
        self.write_frame(
            tokio_tungstenite::tungstenite::Message::text(data),
            deadline,
        )
        .await
    }

    async fn close(&self) {
        let mut write = self.write.lock().await;
        if let Some(mut sink) = write.take() {
            // Best effort: a peer already gone refuses the close handshake.
            let _ = SinkExt::close(&mut sink).await;
        }
        drop(write);
        *self.read.lock().await = None;
    }
}

/// The production [`Dialer`]. TLS is wired through tokio-rustls with the
/// workspace's single ring provider and the OS trust store; proxy settings
/// are deliberately ignored, matching Go's dialer which sets no Proxy either
/// on this internal path.
pub struct DefaultDialer {
    connector: tokio_tungstenite::Connector,
}

impl DefaultDialer {
    pub fn new() -> anyhow::Result<Self> {
        let mut roots = rustls::RootCertStore::empty();
        // Individual unparsable system certs are skipped, matching Go's
        // x509.SystemCertPool tolerance.
        for cert in rustls_native_certs::load_native_certs().certs {
            let _ = roots.add(cert);
        }
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("default protocol versions are always supported")
        .with_root_certificates(roots)
        .with_no_client_auth();
        Ok(Self {
            connector: tokio_tungstenite::Connector::Rustls(Arc::new(config)),
        })
    }
}

impl Default for DefaultDialer {
    fn default() -> Self {
        Self::new().expect("system trust store loads")
    }
}

#[async_trait]
impl Dialer for DefaultDialer {
    async fn dial(&self, ctx: &CancellationToken, url: &str) -> anyhow::Result<Box<dyn WsConn>> {
        let request = url
            .to_string()
            .into_client_request()
            .map_err(|e| anyhow::anyhow!("wecom: parse ws url: {e}"))?;
        let connect = tokio_tungstenite::connect_async_tls_with_config(
            request,
            None,
            false,
            Some(self.connector.clone()),
        );
        let dial = tokio::time::timeout(HANDSHAKE_TIMEOUT, connect);
        let attempt = async {
            match dial.await {
                Err(_) => Err(anyhow::anyhow!(
                    "wecom: dial {url}: handshake timed out after {HANDSHAKE_TIMEOUT:?}"
                )),
                Ok(Err(e)) => Err(anyhow::anyhow!("wecom: dial {url}: {e}")),
                Ok(Ok((stream, _))) => {
                    Ok(Box::new(TungsteniteConn::new(stream)) as Box<dyn WsConn>)
                }
            }
        };
        tokio::select! {
            _ = ctx.cancelled() => Err(anyhow::anyhow!("wecom: dial cancelled")),
            res = attempt => res,
        }
    }
}

// ---- sender ----

/// One caller parked on one req_id.
struct ReplyWaiter {
    tx: mpsc::Sender<ReplyResult>,
}

/// A server answer. body is Null for the acks that carry nothing but a
/// verdict.
#[derive(Debug)]
struct ReplyResult {
    code: i64,
    msg: String,
    body: Value,
}

/// Owns one entry in [`WsSender::replies`]. Async callers can be dropped by
/// an outer deadline or task abort at any await point, so cleanup cannot live
/// only after the request future returns. Dropping this guard retires the
/// entry on both ordinary completion and cancellation.
struct ReplyRegistration<'a> {
    sender: &'a WsSender,
    req_id: String,
    rx: mpsc::Receiver<ReplyResult>,
}

impl Drop for ReplyRegistration<'_> {
    fn drop(&mut self) {
        self.sender.cancel_reply(&self.req_id);
    }
}

/// Serializes writes to one WebSocket connection. Instantiated per Connect()
/// call and dropped when the connection ends.
pub struct WsSender {
    conn: Arc<dyn WsConn>,
    generation: Arc<LeaseGeneration>,
    write_mu: tokio::sync::Mutex<()>,

    /// Callers waiting on a server verdict, keyed by the req_id they wrote.
    /// Only the read loop delivers into these, which is why inbound callbacks
    /// must not run on it.
    replies: Mutex<HashMap<String, ReplyWaiter>>,

    /// Numbers outbound frames in the order they reach the socket. Guarded by
    /// write_mu, so it is the wire order by construction, and it is what
    /// pairs a traced send attempt with its outcome — req_id cannot do that
    /// job, because a pong echoes the server's req_id and that may be empty
    /// or repeated. It never goes on the wire.
    seq: AtomicU64,
}

impl WsSender {
    pub fn new(conn: Arc<dyn WsConn>) -> Self {
        Self::with_generation(conn, LeaseGeneration::standalone())
    }

    pub fn with_generation(conn: Arc<dyn WsConn>, generation: Arc<LeaseGeneration>) -> Self {
        Self {
            conn,
            generation,
            write_mu: tokio::sync::Mutex::new(()),
            replies: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(0),
        }
    }

    /// Hands a server response to whoever is waiting for it and reports
    /// whether anybody was. The read loop calls it for every frame that
    /// answers one of our writes; an unclaimed ack is not an error, since the
    /// pushes that do not wait share this connection.
    pub fn route_response(&self, env: &FrameEnvelope) -> bool {
        self.deliver_reply(env)
    }

    /// Registers interest in the response for the frame about to be written.
    /// `false` means the req_id is already spoken for — with minted ids that
    /// is a collision we would rather fail on than silently cross wires.
    fn await_reply(&self, req_id: &str) -> Option<ReplyRegistration<'_>> {
        let mut replies = self.replies.lock().unwrap_or_else(|e| e.into_inner());
        if replies.contains_key(req_id) {
            return None;
        }
        let (tx, rx) = mpsc::channel(1);
        replies.insert(req_id.to_string(), ReplyWaiter { tx });
        Some(ReplyRegistration {
            sender: self,
            req_id: req_id.to_string(),
            rx,
        })
    }

    /// Retires a waiter. Called on every exit path including the happy one —
    /// a request is one frame and one answer, so the entry is never useful
    /// twice, and leaving it would leak an entry per send.
    fn cancel_reply(&self, req_id: &str) {
        self.replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(req_id);
    }

    /// Hands a response to the request that asked for it, if there is one,
    /// and reports whether it was taken.
    fn deliver_reply(&self, env: &FrameEnvelope) -> bool {
        if env.headers.req_id.is_empty() {
            return false;
        }
        let waiter = {
            let mut replies = self.replies.lock().unwrap_or_else(|e| e.into_inner());
            replies.remove(&env.headers.req_id)
        };
        let Some(waiter) = waiter else {
            return false;
        };
        // Capacity-1 channel and the entry was removed above, so this never
        // blocks and never delivers twice.
        let _ = waiter.tx.try_send(ReplyResult {
            code: env.err_code,
            msg: env.err_msg.clone(),
            body: env.body.clone(),
        });
        true
    }

    /// Writes one frame under a req_id of our own and waits for the whole
    /// answer. An error is either a [`WecomApiError`] carrying the server's
    /// errcode, an [`AckTimeoutError`], or a transport failure.
    pub async fn request(
        &self,
        ctx: &CancellationToken,
        cmd: &str,
        body: Value,
    ) -> anyhow::Result<Value> {
        if ctx.is_cancelled() {
            return Err(anyhow::Error::new(ContextCancelled)
                .context(format!("wecom: context cancelled before {cmd} was sent")));
        }
        let req_id = new_req_id();
        let Some(mut reply) = self.await_reply(&req_id) else {
            anyhow::bail!("wecom: {cmd} req_id {req_id} is already awaiting a response");
        };
        let frame = serde_json::json!({
            "cmd": cmd,
            "headers": FrameHeaders { req_id: req_id.clone() },
            "body": body,
        });
        let result = async {
            self.write(frame).await?;
            let wait = async {
                reply
                    .rx
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("wecom: reply channel closed"))
            };
            tokio::select! {
                biased;
                // `write` already returned success, so cancellation here is
                // an unknown delivery, not a safe pre-write failure. Preserve
                // GenerationExpired in the source chain for fencing while
                // marking the outer error as attempted for relay idempotency.
                _ = self.generation.cancelled() => Err(anyhow::Error::new(WriteAttemptedError(
                    anyhow::Error::new(cordy_channel::GenerationExpired)
                        .context(format!("wecom: lease generation ended waiting for {cmd} verdict")),
                ))),
                res = wait => match res? {
                    r if r.code != 0 => Err(anyhow::Error::new(WecomApiError {
                        cmd: cmd.to_string(),
                        code: r.code,
                        msg: r.msg,
                    })),
                    r => Ok(r.body),
                },
                _ = tokio::time::sleep(ACK_TIMEOUT) => Err(anyhow::Error::new(AckTimeoutError)),
                _ = ctx.cancelled() => Err(anyhow::Error::new(ContextCancelled)
                    .context(format!("wecom: context cancelled waiting for {cmd} verdict"))),
            }
        }
        .await;
        result
    }

    #[cfg(test)]
    fn waiter_count(&self) -> usize {
        self.replies.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Marshals frame to JSON and pushes it under the writer mutex. The
    /// caller must not hold write_mu — nothing here reaches back into the
    /// Channel.
    ///
    /// Trace fields are extracted BEFORE the mutex (the expensive half: a
    /// regexp redaction pass and a rune-wise cut over the message body), and
    /// emitted inside it, where the ping loop, agent replies and inbox pushes
    /// become ordered — a record taken inside matches the wire by
    /// construction.
    pub async fn write(&self, frame: Value) -> anyhow::Result<()> {
        self.generation.ensure_active()?;
        let t: Option<OutTrace> = trace_out_fields(&frame);

        let _guard = self.write_mu.lock().await;
        self.generation.ensure_active()?;
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(t) = &t {
            trace_out_attempt(seq, t);
        }

        let payload = serde_json::to_string(&frame)
            .map_err(|e| anyhow::anyhow!("wecom: marshal frame: {e}"))?;
        let deadline = Instant::now() + WRITE_DEADLINE;
        let res = self.conn.write_message(payload, Some(deadline)).await;
        if let Some(t) = &t {
            trace_out_result(seq, t, TRACE_STAGE_WRITE, res.as_ref().err());
        }
        res.map_err(|e| anyhow::Error::new(WriteAttemptedError(e)))
    }

    /// Pushes an aibot_send_msg (proactive push) with plain text to a
    /// specific chat. Callers pass the aibot chat_type int (1=single,
    /// 2=group). Fire-and-forget over a fresh background token.
    pub async fn send_text(
        &self,
        chat_id: &str,
        chat_type_int: i64,
        content: &str,
    ) -> anyhow::Result<()> {
        self.send_text_ctx(&CancellationToken::new(), chat_id, chat_type_int, content)
            .await
    }

    /// [`send_text`](Self::send_text) that reads the server's verdict. Before
    /// this, a push was fire-and-forget: a frame WeCom refused — over the
    /// size cap, addressed to a chat the bot is no longer in, rate limited —
    /// returned success, so the caller recorded a delivery that never
    /// happened.
    ///
    /// Safe to block here only because inbound callbacks do not run on the
    /// read loop: the read loop is the sole deliverer of acks, so a send that
    /// waited for one from inside a callback would have waited on itself.
    pub async fn send_text_ctx(
        &self,
        ctx: &CancellationToken,
        chat_id: &str,
        chat_type_int: i64,
        content: &str,
    ) -> anyhow::Result<()> {
        let body = send_msg_text_body(chat_id, chat_type_int, content)?;
        self.request(ctx, CMD_SEND_MSG, body).await?;
        Ok(())
    }

    /// Sends the heartbeat frame. A write failure surfaces on the next read
    /// error path; the ping loop logs it but does not tear the loop down.
    pub async fn send_ping(&self) -> anyhow::Result<()> {
        let frame = serde_json::json!({
            "cmd": CMD_PING,
            "headers": FrameHeaders { req_id: new_req_id() },
        });
        self.write(frame).await
    }

    /// Sends the subscribe frame (auth step one of Connect).
    pub(crate) async fn send_subscribe(
        &self,
        bot_id: &str,
        secret: &str,
    ) -> anyhow::Result<String> {
        let req_id = new_req_id();
        let frame = serde_json::json!({
            "cmd": CMD_SUBSCRIBE,
            "headers": FrameHeaders { req_id: req_id.clone() },
            "body": subscribe_body(bot_id, secret),
        });
        self.write(frame).await?;
        Ok(req_id)
    }

    /// Carries one file through the three upload steps and returns the
    /// media_id a message can be built around. See media_upload.rs.
    pub async fn upload_media(
        &self,
        ctx: &CancellationToken,
        m: crate::media_upload::OutboundMedia,
    ) -> anyhow::Result<String> {
        crate::media_upload::upload_media(self, ctx, &m).await
    }

    /// Delivers an uploaded file as a message, on the same aibot_send_msg
    /// push that carries text. See media_upload.rs.
    pub async fn send_media(
        &self,
        ctx: &CancellationToken,
        chat_id: &str,
        chat_type: i64,
        m: crate::media_upload::MediaSend,
    ) -> anyhow::Result<()> {
        crate::media_upload::send_media(self, ctx, chat_id, chat_type, &m).await
    }

    /// The chat-type helper shared with the channel's unsupported-kind
    /// receipt path.
    pub fn chat_type_int(t: &ChatType) -> i64 {
        aibot_chat_type_from_channel(t)
    }
}

/// Returns a random correlation id for a WebSocket frame's headers.req_id.
/// The server echoes it back on each ack so the client can pair replies with
/// requests.
pub fn new_req_id() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Base64-encodes bytes with the standard alphabet (Go encoding/base64
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    struct FakeConn {
        writes: Mutex<Vec<String>>,
        fail_writes: AtomicBool,
    }

    impl FakeConn {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                writes: Mutex::new(Vec::new()),
                fail_writes: AtomicBool::new(false),
            })
        }
    }

    #[async_trait]
    impl WsConn for FakeConn {
        async fn read_message(&self, _deadline: Option<Instant>) -> anyhow::Result<Vec<u8>> {
            Err(anyhow::anyhow!("fake: no reads"))
        }
        async fn write_message(&self, data: String, _d: Option<Instant>) -> anyhow::Result<()> {
            if self.fail_writes.load(Ordering::SeqCst) {
                anyhow::bail!("broken pipe");
            }
            self.writes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(data);
            Ok(())
        }
        async fn close(&self) {}
    }

    #[tokio::test]
    async fn write_serializes_and_marks_attempts() {
        let conn = FakeConn::new();
        let s = WsSender::new(conn.clone());
        s.write(serde_json::json!({"cmd": "ping"})).await.unwrap();
        s.write(serde_json::json!({"cmd": "ping"})).await.unwrap();
        assert_eq!(conn.writes.lock().unwrap().len(), 2);

        conn.fail_writes.store(true, Ordering::SeqCst);
        let err = s.write(serde_json::json!({})).await.unwrap_err();
        assert!(is_write_attempted(&err), "{err}");
    }

    #[tokio::test]
    async fn request_times_out_without_a_verdict() {
        let s = WsSender::new(FakeConn::new());
        let err = s
            .request(
                &CancellationToken::new(),
                "aibot_send_msg",
                serde_json::json!({}),
            )
            .await
            .unwrap_err();
        assert!(is_ack_timeout(&err), "{err}");
    }

    #[tokio::test]
    async fn generation_loss_after_write_is_marked_attempted() {
        struct RevokingConn(Arc<LeaseGeneration>);

        #[async_trait]
        impl WsConn for RevokingConn {
            async fn read_message(&self, _: Option<Instant>) -> anyhow::Result<Vec<u8>> {
                std::future::pending().await
            }

            async fn write_message(&self, _: String, _: Option<Instant>) -> anyhow::Result<()> {
                self.0.revoke();
                Ok(())
            }

            async fn close(&self) {}
        }

        let generation = LeaseGeneration::standalone();
        let sender =
            WsSender::with_generation(Arc::new(RevokingConn(generation.clone())), generation);
        let error = sender
            .request(
                &CancellationToken::new(),
                "aibot_send_msg",
                serde_json::json!({}),
            )
            .await
            .unwrap_err();

        assert!(is_write_attempted(&error), "{error:#}");
        assert!(error.chain().any(|cause| cause
            .downcast_ref::<cordy_channel::GenerationExpired>()
            .is_some()));
    }

    #[tokio::test]
    async fn route_response_delivers_to_the_waiting_request() {
        let s = Arc::new(WsSender::new(FakeConn::new()));
        // Park a waiter manually, then deliver an ack for its req_id.
        let mut reply = s.await_reply("r1").expect("fresh req_id");
        assert!(s.await_reply("r1").is_none(), "duplicate req_id refused");

        let env = FrameEnvelope {
            headers: FrameHeaders {
                req_id: "r1".to_string(),
            },
            err_code: 45009,
            err_msg: "rate limited".to_string(),
            ..Default::default()
        };
        assert!(s.route_response(&env));
        assert!(!s.route_response(&env), "second delivery finds nobody");

        let res = reply.rx.recv().await.unwrap();
        assert_eq!(res.code, 45009);
        assert_eq!(res.msg, "rate limited");
    }

    #[tokio::test]
    async fn dropping_request_future_retires_its_waiter() {
        let conn = FakeConn::new();
        let sender = Arc::new(WsSender::new(conn.clone()));
        let task_sender = sender.clone();
        let request = tokio::spawn(async move {
            task_sender
                .request(
                    &CancellationToken::new(),
                    "aibot_send_msg",
                    serde_json::json!({}),
                )
                .await
        });

        let req_id = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let frame = conn
                    .writes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .first()
                    .cloned();
                if let Some(frame) = frame {
                    let frame: Value = serde_json::from_str(&frame).unwrap();
                    break frame["headers"]["req_id"].as_str().unwrap().to_string();
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("request writes before the test deadline");
        assert_eq!(sender.waiter_count(), 1);

        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(sender.waiter_count(), 0);

        let late_ack = FrameEnvelope {
            headers: FrameHeaders { req_id },
            ..Default::default()
        };
        assert!(!sender.route_response(&late_ack));
    }

    #[tokio::test]
    async fn unclaimed_ack_is_not_an_error() {
        let s = WsSender::new(FakeConn::new());
        let env = FrameEnvelope {
            headers: FrameHeaders {
                req_id: "nobody".to_string(),
            },
            ..Default::default()
        };
        assert!(!s.route_response(&env));
        // Empty req_id never routes.
        assert!(!s.route_response(&FrameEnvelope::default()));
    }

    #[test]
    fn api_error_carries_the_verdict() {
        let e: anyhow::Error = WecomApiError {
            cmd: "aibot_send_msg".to_string(),
            code: 45009,
            msg: "api freq limited".to_string(),
        }
        .into();
        let api = as_api_error(&e).unwrap();
        assert_eq!(api.code, 45009);
        assert_eq!(api.cmd, "aibot_send_msg");
        assert!(e.to_string().contains("errcode=45009"));
    }

    #[test]
    fn req_ids_are_hex_and_unique() {
        let a = new_req_id();
        let b = new_req_id();
        assert_eq!(a.len(), 16);
        assert_ne!(a, b);
        assert!(hex::decode(&a).is_ok());
    }
}
