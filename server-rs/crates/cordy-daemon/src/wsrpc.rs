//! Port of `server/internal/daemon/wsrpc.go` (lines 1–305).
//!
//! The daemon-side half of the generic WS request/response transport
//! (MUL-4257). It correlates responses to requests by request_id over the
//! shared, multiplexed WS control connection so multiple RPCs can be in flight
//! concurrently. Sending is delegated to an injected frame-sender closure
//! (which pushes onto the active connection's write channel); when no
//! connection is attached, [`WsRpcClient::call`] fails fast with
//! [`WsRpcError::Unavailable`] and the caller uses HTTP.
//!
//! Symbol map (Go → Rust):
//! - `errWSRPCUnavailable` → [`WsRpcError::Unavailable`]
//! - `errWSRPCUncertain` → [`WsRpcError::Uncertain`]
//! - `errWSRPCWriteBufferFull` → [`WsRpcError::WriteBufferFull`]
//! - `wsRPCResponseGrace` → [`WS_RPC_RESPONSE_GRACE`]
//! - `wsClaimUncertainFallbackDelay` → [`ws_claim_uncertain_fallback_delay`]
//! - `wsOutbound` → [`WsOutbound`]
//! - `wsRPCClient` → [`WsRpcClient`]
//! - `attach` / `markRPCV1Supported` / `currentGeneration` /
//!   `supportsRPCV1` / `call` / `deliver` → same-named methods
//!
//! Deviations from Go:
//! - `ClaimTasksWSFirst` (wsrpc.go:307–380) is Daemon wiring — it reads
//!   `d.batchClaimUnsupported`, `d.wsClaimHTTPFallbackAfter`, `d.logger` and
//!   `d.client`. It lands with the daemon.go core (lane B), which owns those
//!   fields.
//! - `batchClaimRequestTimeout` (client.go:255) is defined in `client.rs`
//!   alongside the rest of client.go; `ws_claim_uncertain_fallback_delay`
//!   references it through that module to keep one source of truth.
//! - Go's `<-chan protocol.RPCResponsePayload` pending map becomes
//!   `HashMap<String, mpsc::Sender<..>>`; `deliver` uses `try_send` on a
//!   capacity-1 channel to replicate the non-blocking send. Detach closes
//!   delivery by removing the entry (the caller observes the removal as
//!   [`WsRpcError::Unavailable`]-vs-[`WsRpcError::Uncertain`] via
//!   [`WsOutbound::cancel`], exactly like Go's closed channel).

// S9-integration: consumed by daemon.go core (lane B) and the hub WS pump;
// silence dead-code until wired.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use cordy_protocol::{Message, RpcRequestPayload, RpcResponsePayload, EVENT_DAEMON_RPC_REQUEST};

use crate::client::BATCH_CLAIM_REQUEST_TIMEOUT;

/// `errWSRPCUnavailable`: no live WS connection to carry the request (or the
/// frame was cancelled before it left the writer). Callers treat this as the
/// signal to fall back to HTTP.
#[derive(Debug, thiserror::Error)]
#[error("ws rpc: no active connection")]
pub struct Unavailable;

/// `errWSRPCUncertain`: the request's frame WAS sent but the connection dropped
/// before a definitive response. The outcome is unknown (the server may have
/// committed), so the caller must NOT fall back to another transport for the
/// same work — that risks a double claim (MUL-4257).
#[derive(Debug, thiserror::Error)]
#[error("ws rpc: sent but outcome unknown (connection lost)")]
pub struct Uncertain;

/// `errWSRPCWriteBufferFull`: the connection's write buffer is saturated; the
/// caller falls back to HTTP rather than blocking the socket.
#[derive(Debug, thiserror::Error)]
#[error("ws rpc: write buffer full")]
pub struct WriteBufferFull;

/// The RPC error taxonomy ([`Unavailable`] / [`Uncertain`] /
/// [`WriteBufferFull`]) plus wrapped call failures.
#[derive(Debug, thiserror::Error)]
pub enum WsRpcError {
    #[error(transparent)]
    Unavailable(#[from] Unavailable),
    #[error(transparent)]
    Uncertain(#[from] Uncertain),
    #[error(transparent)]
    WriteBufferFull(#[from] WriteBufferFull),
    /// `fmt.Errorf`-wrapped transport failures (marshal/send errors).
    #[error("{0}")]
    Wrapped(String),
}

/// True when `err` is the sentinel `e` (Go `errors.Is(err, errWSRPCX)`).
pub fn is_unavailable(err: &WsRpcError) -> bool {
    matches!(err, WsRpcError::Unavailable(_))
}

/// See [`is_unavailable`].
pub fn is_uncertain(err: &WsRpcError) -> bool {
    matches!(err, WsRpcError::Uncertain(_))
}

/// See [`is_unavailable`].
pub fn is_write_buffer_full(err: &WsRpcError) -> bool {
    matches!(err, WsRpcError::WriteBufferFull(_))
}

/// `wsRPCResponseGrace` (wsrpc.go:30): how much longer the daemon waits for an
/// RPC response beyond the server-side execution budget it requested, so a
/// claim that committed just before the server deadline still reports back
/// before the daemon gives up (MUL-4257).
pub const WS_RPC_RESPONSE_GRACE: Duration = Duration::from_secs(2);

/// `wsClaimUncertainFallbackDelay` (wsrpc.go:32).
pub fn ws_claim_uncertain_fallback_delay() -> Duration {
    BATCH_CLAIM_REQUEST_TIMEOUT + WS_RPC_RESPONSE_GRACE
}

/// Delivery end for a pending request. `None` means detached/closed (Go's
/// closed-channel receive).
type PendingTx = mpsc::Sender<RpcResponsePayload>;

/// Injected frame writer: pushes `frame` onto the active connection's write
/// channel and returns the cancellable outbound handle. Go:
/// `sendFrame func([]byte) (*wsOutbound, error)`.
pub type SendFrame = Arc<dyn Fn(Vec<u8>) -> Result<Arc<WsOutbound>, WsRpcError> + Send + Sync>;

/// Lifecycle of one queued frame (Go's `sent`/`canceled` mutex pair).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum OutboundState {
    #[default]
    Pending,
    Sent,
    Canceled,
}

/// `wsOutbound` (wsrpc.go:51): a frame queued for the WS writer. It is
/// cancelable so an RPC caller that gives up (timeout/detach) before the frame
/// has hit the socket can prevent it from being delivered later — otherwise a
/// backpressured writer could deliver a stale tasks.claim after the daemon
/// already HTTP-fell-back, double-claiming (MUL-4257, Sol-Boy review).
/// sent/cancel race under one lock so the decision is atomic: whoever wins
/// determines whether the frame is delivered.
#[derive(Debug, Default)]
pub struct WsOutbound {
    state: Mutex<OutboundState>,
}

impl WsOutbound {
    /// `beginWrite` (wsrpc.go:61): called by the writer immediately before
    /// WriteMessage. Returns false when the frame was already cancelled (skip
    /// it); otherwise marks the frame sent so a concurrent cancel can no
    /// longer un-send it.
    pub(crate) fn begin_write(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if *state == OutboundState::Canceled {
            return false;
        }
        *state = OutboundState::Sent;
        true
    }

    /// `cancel` (wsrpc.go:74): called by an RPC caller giving up. Returns true
    /// if the frame was still pending (now cancelled — the writer will skip
    /// it, so it is guaranteed NOT delivered); false if the writer already
    /// began sending it.
    pub(crate) fn cancel(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if *state == OutboundState::Sent {
            return false;
        }
        *state = OutboundState::Canceled;
        true
    }
}

/// Per-connection attachment plus negotiation state, guarded by the client
/// mutex exactly like the Go fields it mirrors.
#[derive(Default)]
struct Attach {
    send_frame: Option<SendFrame>,
    /// Belongs to the currently attached connection. attach clears it before
    /// exposing a replacement sender so a claim that races a reconnect can
    /// never carry negotiation state across connections.
    rpc_v1_supported: bool,
    generation: u64,
}

/// `wsRPCClient` (wsrpc.go:84).
pub(crate) struct WsRpcClient {
    inner: Mutex<Inner>,
    /// Added to a call's server-side timeout budget to get how long the daemon
    /// waits for the response, so a claim that committed just before the
    /// server deadline still reports back before the daemon gives up
    /// (MUL-4257).
    grace: Duration,
}

struct Inner {
    attach: Attach,
    pending: HashMap<String, PendingTx>,
}

impl WsRpcClient {
    /// `newWSRPCClient` (wsrpc.go:99).
    pub(crate) fn new(grace: Duration) -> Self {
        Self {
            inner: Mutex::new(Inner {
                attach: Attach::default(),
                pending: HashMap::new(),
            }),
            grace,
        }
    }

    /// `attach` (wsrpc.go:111): binds a live connection's frame writer and
    /// clears the previous connection's negotiated capability. Passing `None`
    /// detaches (on disconnect), after which [`Self::call`] fails fast until
    /// the next attach and rpc-v1 heartbeat ack. Any pending requests are
    /// failed so their callers fall back to HTTP immediately. Returns the new
    /// generation counter.
    pub(crate) fn attach(&self, send_frame: Option<SendFrame>) -> u64 {
        let mut inner = self.inner.lock().unwrap();
        inner.attach.generation += 1;
        inner.attach.rpc_v1_supported = false;
        inner.attach.send_frame = send_frame;
        // Dropping the senders fails every waiter (recv returns None), which
        // is Go closing each pending channel.
        inner.pending.clear();
        inner.attach.generation
    }

    /// `markRPCV1Supported` (wsrpc.go:128): records explicit server support
    /// for the currently attached connection. Heartbeat acks received without
    /// a live sender cannot enable a future connection.
    pub(crate) fn mark_rpc_v1_supported(&self, generation: u64) {
        let mut inner = self.inner.lock().unwrap();
        if inner.attach.send_frame.is_some() && inner.attach.generation == generation {
            inner.attach.rpc_v1_supported = true;
        }
    }

    /// `currentGeneration` (wsrpc.go:139).
    pub(crate) fn current_generation(&self) -> u64 {
        self.inner.lock().unwrap().attach.generation
    }

    /// `supportsRPCV1` (wsrpc.go:152): reports whether the live connection
    /// explicitly negotiated rpc-v1. [`Self::call_if_rpc_v1_supported`] repeats
    /// this check while capturing the sender under the same mutex, so this
    /// method is only a fast-path hint and cannot authorize a send by itself.
    pub(crate) fn supports_rpc_v1(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        inner.attach.send_frame.is_some() && inner.attach.rpc_v1_supported
    }

    /// `Call` (wsrpc.go:163): issues an RPC on any attached connection.
    /// Transport-level tests and callers that have their own negotiation
    /// contract use this directly. Returns the response status (0 when the
    /// call never reached the server) so the caller can distinguish transport
    /// failure (→ HTTP fallback) from a server-side error.
    pub(crate) async fn call<Q, R>(
        &self,
        ctx: &crate::repocache::Ctx,
        method: &str,
        server_timeout: Duration,
        req_body: Option<&Q>,
    ) -> Result<(u16, Option<R>), WsRpcError>
    where
        Q: Serialize + Send + Sync,
        R: DeserializeOwned + Send,
    {
        self.call_inner(ctx, method, server_timeout, req_body, false)
            .await
    }

    /// `CallIfRPCV1Supported` (wsrpc.go:171): issues an RPC only when the
    /// currently attached connection explicitly negotiated rpc-v1. The
    /// capability check and sender capture happen under the same mutex, so a
    /// reconnect cannot redirect a call authorized by the previous connection
    /// onto its replacement.
    pub(crate) async fn call_if_rpc_v1_supported<Q, R>(
        &self,
        ctx: &crate::repocache::Ctx,
        method: &str,
        server_timeout: Duration,
        req_body: Option<&Q>,
    ) -> Result<(u16, Option<R>), WsRpcError>
    where
        Q: Serialize + Send + Sync,
        R: DeserializeOwned + Send,
    {
        self.call_inner(ctx, method, server_timeout, req_body, true)
            .await
    }

    /// `call` (wsrpc.go:181): blocks until the response, the per-request
    /// timeout, or ctx cancellation. `req_body` is marshaled into the request
    /// envelope; on a 2xx response the body (if any) is deserialized into `R`.
    async fn call_inner<Q, R>(
        &self,
        ctx: &crate::repocache::Ctx,
        method: &str,
        server_timeout: Duration,
        req_body: Option<&Q>,
        require_rpc_v1: bool,
    ) -> Result<(u16, Option<R>), WsRpcError>
    where
        Q: Serialize + Send + Sync,
        R: DeserializeOwned + Send,
    {
        let raw_req =
            match req_body {
                Some(body) => Some(serde_json::to_value(body).map_err(|err| {
                    WsRpcError::Wrapped(format!("ws rpc: marshal request: {err}"))
                })?),
                None => None,
            };
        let id = Uuid::now_v7().to_string();
        // Go marshals the raw request into Value without re-quoting; passing
        // the Value straight through keeps byte-for-byte parity.
        let frame_payload = serde_json::to_value(Message {
            r#type: EVENT_DAEMON_RPC_REQUEST.to_string(),
            payload: serde_json::to_value(RpcRequestPayload {
                request_id: id.clone(),
                method: method.to_string(),
                body: raw_req,
                timeout_ms: server_timeout.as_millis() as i64,
            })
            .map_err(|err| WsRpcError::Wrapped(format!("ws rpc: marshal frame: {err}")))?,
        })
        .map_err(|err| WsRpcError::Wrapped(format!("ws rpc: marshal frame: {err}")))?;
        let frame = serde_json::to_vec(&frame_payload).expect("Value serialization cannot fail");

        let (tx, mut rx) = mpsc::channel::<RpcResponsePayload>(1);
        let send = {
            let mut inner = self.inner.lock().unwrap();
            if inner.attach.send_frame.is_none()
                || (require_rpc_v1 && !inner.attach.rpc_v1_supported)
            {
                return Err(Unavailable.into());
            }
            let send = inner
                .attach
                .send_frame
                .clone()
                .expect("checked non-None above");
            inner.pending.insert(id.clone(), tx);
            send
        };

        // On any exit path, drop our pending entry (Go's deferred delete).
        let result = self
            .drive_call(ctx, server_timeout, send, frame, &mut rx)
            .await;
        self.inner.lock().unwrap().pending.remove(&id);
        result
    }

    /// The wait/select half of `call` (wsrpc.go:223–283), split out so the
    /// pending-entry cleanup wraps exactly one await site.
    async fn drive_call<R>(
        &self,
        ctx: &crate::repocache::Ctx,
        server_timeout: Duration,
        send: SendFrame,
        frame: Vec<u8>,
        rx: &mut mpsc::Receiver<RpcResponsePayload>,
    ) -> Result<(u16, Option<R>), WsRpcError>
    where
        R: DeserializeOwned + Send,
    {
        let item = match send(frame) {
            Ok(item) => item,
            Err(err) => return Err(WsRpcError::Wrapped(format!("ws rpc: send: {err}"))),
        };

        // `giveUp` (wsrpc.go:233): resolves an abandoned request. If the
        // frame is still queued we cancel it so the writer never delivers it
        // — a definitively-not-sent outcome that is safe to HTTP-fall-back.
        // If the writer already began sending it, it may reach the server,
        // so the outcome is uncertain and the caller must NOT fall back
        // (that would double-claim, MUL-4257).
        let give_up = || {
            if item.cancel() {
                WsRpcError::Unavailable(Unavailable)
            } else {
                WsRpcError::Uncertain(Uncertain)
            }
        };

        // Wait the server-side budget PLUS a grace margin: a claim that
        // committed just before the server deadline must still report back
        // before the daemon gives up and falls back to HTTP, or we would
        // double-claim (MUL-4257). (wsrpc.go:243–248)
        let mut timeout = server_timeout + self.grace;
        if timeout.is_zero() {
            timeout = Duration::from_secs(5);
        }
        let sleep = tokio::time::sleep(timeout);
        tokio::pin!(sleep);

        tokio::select! {
            resp = rx.recv() => {
                let resp = match resp {
                    Some(resp) => resp,
                    None => {
                        // The connection detached. Whether the server saw this
                        // request depends on whether the frame had already
                        // left the writer, so let give_up() decide
                        // (not-sent → safe fallback; sent → uncertain).
                        return Err(give_up());
                    }
                };
                let status = u16::try_from(resp.status).unwrap_or(0);
                if (200..300).contains(&status) {
                    let body = match resp.body {
                        Some(body) if !body.is_null() => Some(body),
                        _ => None,
                    };
                    let parsed = match body {
                        Some(body) => Some(
                            serde_json::from_value::<R>(body).map_err(|err| {
                                WsRpcError::Wrapped(format!("ws rpc: decode response: {err}"))
                            })?,
                        ),
                        None => None,
                    };
                    return Ok((status, parsed));
                }
                let msg = if resp.error.is_empty() {
                    format!("ws rpc status {}", resp.status)
                } else {
                    resp.error
                };
                Err(WsRpcError::Wrapped(msg))
            }
            _ = &mut sleep => {
                // The budget elapsed. If the frame is still queued behind a
                // backpressured writer, cancel it so it is never delivered
                // after we fall back (giveUp → not-sent). If it already left
                // the writer, the outcome is uncertain and we must not fall
                // back. (wsrpc.go:271–279)
                let err = give_up();
                if is_uncertain(&err) {
                    return Err(err);
                }
                Err(WsRpcError::Wrapped(format!(
                    "ws rpc: timeout after {:?}: {}",
                    timeout,
                    Unavailable
                )))
            }
            _ = ctx.cancelled() => {
                item.cancel();
                Err(WsRpcError::Unavailable(Unavailable))
            }
        }
    }

    /// `deliver` (wsrpc.go:291): routes an inbound rpc_response frame to the
    /// waiting call. The lookup happens under the mutex so it is serialized
    /// with detach's clear: an entry present in pending is guaranteed to have
    /// a live receiver slot, so `try_send` replicates the buffered
    /// non-blocking send. Unknown request ids (already timed out / detached)
    /// are dropped.
    pub(crate) fn deliver(&self, resp: RpcResponsePayload) {
        let inner = self.inner.lock().unwrap();
        if let Some(tx) = inner.pending.get(&resp.request_id) {
            let _ = tx.try_send(resp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::oneshot;

    fn sender_that_reports_outbound(
        tx: oneshot::Sender<Arc<WsOutbound>>,
        mark_sent: bool,
    ) -> SendFrame {
        let tx = Mutex::new(Some(tx));
        Arc::new(move |_frame| {
            let outbound = Arc::new(WsOutbound::default());
            if mark_sent {
                assert!(outbound.begin_write());
            }
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(Arc::clone(&outbound));
            }
            Ok(outbound)
        })
    }

    #[tokio::test]
    async fn detach_after_write_is_uncertain_not_safe_fallback() {
        let client = Arc::new(WsRpcClient::new(Duration::from_secs(1)));
        let (sent_tx, sent_rx) = oneshot::channel();
        client.attach(Some(sender_that_reports_outbound(sent_tx, true)));
        let call_client = Arc::clone(&client);
        let call = tokio::spawn(async move {
            call_client
                .call::<_, serde_json::Value>(
                    &crate::repocache::Ctx::new(),
                    "tasks.claim",
                    Duration::from_secs(30),
                    Some(&json!({})),
                )
                .await
        });
        sent_rx.await.unwrap();
        client.attach(None);

        assert!(is_uncertain(&call.await.unwrap().unwrap_err()));
    }

    #[tokio::test]
    async fn detach_before_write_is_definitively_unavailable() {
        let client = Arc::new(WsRpcClient::new(Duration::from_secs(1)));
        let (queued_tx, queued_rx) = oneshot::channel();
        client.attach(Some(sender_that_reports_outbound(queued_tx, false)));
        let call_client = Arc::clone(&client);
        let call = tokio::spawn(async move {
            call_client
                .call::<_, serde_json::Value>(
                    &crate::repocache::Ctx::new(),
                    "tasks.claim",
                    Duration::from_secs(30),
                    Some(&json!({})),
                )
                .await
        });
        let outbound = queued_rx.await.unwrap();
        client.attach(None);

        let err = call.await.unwrap().unwrap_err();
        assert!(is_unavailable(&err));
        assert!(
            !outbound.begin_write(),
            "detached caller must cancel queued frame"
        );
    }

    #[test]
    fn capability_is_scoped_to_connection_generation() {
        let client = WsRpcClient::new(Duration::ZERO);
        let sender: SendFrame = Arc::new(|_| Ok(Arc::new(WsOutbound::default())));
        let first = client.attach(Some(Arc::clone(&sender)));
        client.mark_rpc_v1_supported(first);
        assert!(client.supports_rpc_v1());

        let second = client.attach(Some(sender));
        assert!(!client.supports_rpc_v1());
        client.mark_rpc_v1_supported(first);
        assert!(!client.supports_rpc_v1());
        client.mark_rpc_v1_supported(second);
        assert!(client.supports_rpc_v1());
    }
}
