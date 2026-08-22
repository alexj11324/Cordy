//! Port of `server/internal/daemon/wsrpc.go` (lines 1–380) — the daemon-side
//! half of the generic WS request/response transport (MUL-4257).
//!
//! Symbol map (Go → Rust):
//! - `errWSRPCUnavailable` / `errWSRPCUncertain` / `errWSRPCWriteBufferFull` →
//!   [`WsRpcSentinel`] variants; `errors.Is` checks go through
//!   [`is_uncertain`] / [`sentinel_of`] on the anyhow chain
//! - `wsRPCResponseGrace` → [`WS_RPC_RESPONSE_GRACE`]
//! - `wsClaimUncertainFallbackDelay` → [`ws_claim_uncertain_fallback_delay`]
//! - `wsOutbound` (+ `beginWrite` / `cancel`) → [`WsOutbound`]
//!   ([`WsOutbound::begin_write`] / [`WsOutbound::cancel`])
//! - `wsRPCClient` → [`WsRpcClient`]: `attach` / `markRPCV1Supported` /
//!   `currentGeneration` / `supportsRPCV1` / `Call` / `CallIfRPCV1Supported` /
//!   `call` / `deliver`
//! - `(int, error)` call result → `Result<(i32, Option<T>), WsRpcCallError>`
//!   (status preserved on every outcome, matching Go's tuple)
//! - `ClaimTasksWSFirst` → [`claim_tasks_ws_first`] over the [`WsClaimHost`]
//!   seam
//!
//! Port notes:
//! - Go's nil-receiver guards (`if c == nil`) become Go-side `Option<&…>`
//!   handling at the daemon call sites; the client itself is always present.
//! - Go's `chan protocol.RPCResponsePayload` pending map becomes
//!   `HashMap<String, mpsc::Sender<_>>`; `attach` drops the senders, which
//!   closes the receivers exactly like Go's `close(ch)`.
//! - S9-integration: `ClaimTasksWSFirst` is a *Daemon method in Go. The
//!   daemon struct belongs to another lane, so the touched surface is
//!   captured by the [`WsClaimHost`] trait here and wired to the real Daemon
//!   at integration time.

// S9-integration: dead_code is expected until the Daemon core (daemon.go
// port) wires these symbols; remove this allow when that lane lands.
#![allow(dead_code)]

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::anyhow;
use cordy_protocol::messages::{Message, RpcRequestPayload, RpcResponsePayload};
use cordy_protocol::EVENT_DAEMON_RPC_REQUEST;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::client::BATCH_CLAIM_REQUEST_TIMEOUT;
use crate::repocache::Ctx;
use crate::types::Task;

/// `errWSRPCUnavailable` (wsrpc.go:18): returned by Call when there is no live
/// WS connection to carry the request. Callers treat it as the signal to fall
/// back to HTTP.
/// `errWSRPCUncertain` (wsrpc.go:24): a request's frame WAS sent but the
/// connection dropped before a definitive response — the outcome is unknown,
/// so the caller must NOT fall back to another transport for the same work.
/// `errWSRPCWriteBufferFull` (wsrpc.go:36): the connection's write buffer is
/// saturated; the caller falls back to HTTP rather than blocking the socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum WsRpcSentinel {
    #[error("ws rpc: no active connection")]
    Unavailable,
    #[error("ws rpc: sent but outcome unknown (connection lost)")]
    Uncertain,
    #[error("ws rpc: write buffer full")]
    WriteBufferFull,
}

/// `errors.Is(err, errWSRPCUncertain)` equivalent over an anyhow chain.
pub(crate) fn is_uncertain(err: &anyhow::Error) -> bool {
    sentinel_of(err) == Some(WsRpcSentinel::Uncertain)
}

/// Locates a transport sentinel anywhere in the error chain.
pub(crate) fn sentinel_of(err: &anyhow::Error) -> Option<WsRpcSentinel> {
    err.chain()
        .find_map(|c| c.downcast_ref::<WsRpcSentinel>().copied())
}

/// `wsRPCResponseGrace` (wsrpc.go:30): how much longer the daemon waits for an
/// RPC response beyond the server-side execution budget it requested, so a
/// claim that committed just before the server deadline still reports back
/// before the daemon gives up (MUL-4257).
pub(crate) const WS_RPC_RESPONSE_GRACE: Duration = Duration::from_secs(2);

/// `wsClaimUncertainFallbackDelay` (wsrpc.go:32). A function because Duration
/// addition is not const-stable on this toolchain.
pub(crate) fn ws_claim_uncertain_fallback_delay() -> Duration {
    BATCH_CLAIM_REQUEST_TIMEOUT + WS_RPC_RESPONSE_GRACE
}

// ---------------------------------------------------------------------------
// wsOutbound
// ---------------------------------------------------------------------------

#[derive(Default)]
struct OutboundState {
    sent: bool,
    canceled: bool,
}

/// `wsOutbound` (wsrpc.go:51–56): a frame queued for the WS writer. It is
/// cancelable so an RPC caller that gives up (timeout/detach) before the frame
/// has hit the socket can prevent it from being delivered later — otherwise a
/// backpressured writer could deliver a stale tasks.claim after the daemon
/// already HTTP-fell-back, double-claiming (MUL-4257, Sol-Boy review). The
/// sent/cancel race is arbitrated under one mutex so the decision is atomic:
/// whoever wins determines whether the frame is delivered.
pub(crate) struct WsOutbound {
    /// Frame bytes consumed by the connection's write pump.
    pub(crate) data: Vec<u8>,
    state: Mutex<OutboundState>,
}

impl WsOutbound {
    pub(crate) fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            state: Mutex::new(OutboundState::default()),
        }
    }

    fn lock(state: &Mutex<OutboundState>) -> std::sync::MutexGuard<'_, OutboundState> {
        state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `beginWrite` (wsrpc.go:61–69): called by the writer immediately before
    /// writing to the socket. Returns false when the frame was already
    /// cancelled (skip it); otherwise marks the frame sent so a concurrent
    /// cancel can no longer un-send it.
    pub(crate) fn begin_write(&self) -> bool {
        let mut state = Self::lock(&self.state);
        if state.canceled {
            return false;
        }
        state.sent = true;
        true
    }

    /// `cancel` (wsrpc.go:74–82): called by an RPC caller giving up. Returns
    /// true if the frame was still pending (now cancelled — the writer will
    /// skip it, so it is guaranteed NOT delivered); false if the writer
    /// already began sending it.
    pub(crate) fn cancel(&self) -> bool {
        let mut state = Self::lock(&self.state);
        if state.sent {
            return false;
        }
        state.canceled = true;
        true
    }
}

/// Go's injected `sendFrame func([]byte) (*wsOutbound, error)` — pushes onto
/// the active connection's write channel.
pub(crate) type SendFrameFn =
    Arc<dyn Fn(Vec<u8>) -> Result<Arc<WsOutbound>, anyhow::Error> + Send + Sync>;

// ---------------------------------------------------------------------------
// wsRPCClient
// ---------------------------------------------------------------------------

struct WsRpcInner {
    pending: HashMap<String, mpsc::Sender<RpcResponsePayload>>,
    send_frame: Option<SendFrameFn>,
    /// Belongs to the currently attached connection. attach clears it before
    /// exposing a replacement sender so a claim that races a reconnect can
    /// never carry negotiation state across connections (wsrpc.go:88–91).
    rpc_v1_supported: bool,
    generation: u64,
}

/// `wsRPCClient` (wsrpc.go:84–97): correlates responses to requests by
/// request_id over the shared, multiplexed WS control connection so multiple
/// RPCs can be in flight concurrently.
pub(crate) struct WsRpcClient {
    inner: Mutex<WsRpcInner>,
    /// Added to a call's server-side timeout budget to get how long the daemon
    /// waits for the response, so a claim that committed just before the
    /// server deadline still reports back before the daemon gives up
    /// (MUL-4257).
    grace: Duration,
}

/// Error side of a call, preserving Go's `(status int, err error)` pair:
/// `status` is 0 when the call never reached the server so callers can
/// distinguish transport failure (→ HTTP fallback) from a server-side error.
#[derive(Debug)]
pub(crate) struct WsRpcCallError {
    pub(crate) status: i32,
    pub(crate) source: anyhow::Error,
}

impl WsRpcCallError {
    fn transport(source: anyhow::Error) -> Self {
        Self { status: 0, source }
    }

    fn at(status: i32, source: anyhow::Error) -> Self {
        Self { status, source }
    }
}

impl std::fmt::Display for WsRpcCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.source)
    }
}

impl std::error::Error for WsRpcCallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

impl Default for WsRpcClient {
    fn default() -> Self {
        Self::new(WS_RPC_RESPONSE_GRACE)
    }
}

fn lock_inner(inner: &Mutex<WsRpcInner>) -> std::sync::MutexGuard<'_, WsRpcInner> {
    inner.lock().unwrap_or_else(|e| e.into_inner())
}

impl WsRpcClient {
    /// `newWSRPCClient` (wsrpc.go:99–104).
    pub(crate) fn new(grace: Duration) -> Self {
        Self {
            inner: Mutex::new(WsRpcInner {
                pending: HashMap::new(),
                send_frame: None,
                rpc_v1_supported: false,
                generation: 0,
            }),
            grace,
        }
    }

    /// `attach` (wsrpc.go:111–123): binds a live connection's frame writer and
    /// clears the previous connection's negotiated capability. Passing None
    /// detaches (on disconnect), after which Call fails fast until the next
    /// attach and rpc-v1 heartbeat ack. Any pending requests are failed so
    /// their callers fall back to HTTP immediately (dropping the senders
    /// closes the waiting receivers).
    pub(crate) fn attach(&self, send_frame: Option<SendFrameFn>) -> u64 {
        let mut inner = lock_inner(&self.inner);
        inner.generation += 1;
        inner.rpc_v1_supported = false;
        inner.send_frame = send_frame;
        // close(ch) + delete: dropping every sender closes each receiver.
        inner.pending.clear();
        inner.generation
    }

    /// `markRPCV1Supported` (wsrpc.go:128–137): records explicit server
    /// support for the currently attached connection. Heartbeat acks received
    /// without a live sender cannot enable a future connection.
    pub(crate) fn mark_rpc_v1_supported(&self, generation: u64) {
        let mut inner = lock_inner(&self.inner);
        if inner.send_frame.is_some() && inner.generation == generation {
            inner.rpc_v1_supported = true;
        }
    }

    /// `currentGeneration` (wsrpc.go:139–146).
    pub(crate) fn current_generation(&self) -> u64 {
        lock_inner(&self.inner).generation
    }

    /// `supportsRPCV1` (wsrpc.go:152–159): reports whether the live connection
    /// explicitly negotiated rpc-v1. Call repeats this check while capturing
    /// the sender under the same mutex, so this method is only a fast-path
    /// hint and cannot authorize a send by itself.
    pub(crate) fn supports_rpc_v1(&self) -> bool {
        let inner = lock_inner(&self.inner);
        inner.send_frame.is_some() && inner.rpc_v1_supported
    }

    /// `Call` (wsrpc.go:163–165): issues an RPC on any attached connection.
    /// Transport-level tests and callers that have their own negotiation
    /// contract use this directly.
    pub(crate) async fn call<T, B>(
        &self,
        ctx: &Ctx,
        method: &str,
        server_timeout: Duration,
        req_body: Option<&B>,
    ) -> Result<(i32, Option<T>), WsRpcCallError>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        self.call_inner(ctx, method, server_timeout, req_body, false)
            .await
    }

    /// `CallIfRPCV1Supported` (wsrpc.go:171–173): issues an RPC only when the
    /// currently attached connection explicitly negotiated rpc-v1. The
    /// capability check and sender capture happen under the same mutex, so a
    /// reconnect cannot redirect a call authorized by the previous connection
    /// onto its replacement.
    pub(crate) async fn call_if_rpc_v1_supported<T, B>(
        &self,
        ctx: &Ctx,
        method: &str,
        server_timeout: Duration,
        req_body: Option<&B>,
    ) -> Result<(i32, Option<T>), WsRpcCallError>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        self.call_inner(ctx, method, server_timeout, req_body, true)
            .await
    }

    /// `call` (wsrpc.go:181–284): waits until the response, the per-request
    /// timeout, or ctx cancellation. On a 2xx response the body Value is
    /// returned for typed decoding by the wrappers.
    async fn call_inner<T, B>(
        &self,
        ctx: &Ctx,
        method: &str,
        server_timeout: Duration,
        req_body: Option<&B>,
        require_rpc_v1: bool,
    ) -> Result<(i32, Option<T>), WsRpcCallError>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        let raw_req = match req_body {
            Some(body) => Some(serde_json::to_value(body).map_err(|e| {
                WsRpcCallError::transport(anyhow!("ws rpc: marshal request: {}", e))
            })?),
            None => None,
        };
        let id = uuid::Uuid::now_v7().to_string();
        let payload = serde_json::to_value(RpcRequestPayload {
            request_id: id.clone(),
            method: method.to_string(),
            body: raw_req,
            timeout_ms: server_timeout.as_millis() as i64,
        })
        .map_err(|e| WsRpcCallError::transport(anyhow!("ws rpc: marshal frame: {}", e)))?;
        let frame = serde_json::to_vec(&Message {
            r#type: EVENT_DAEMON_RPC_REQUEST.to_string(),
            payload,
        })
        .map_err(|e| WsRpcCallError::transport(anyhow!("ws rpc: marshal frame: {}", e)))?;

        let (tx, mut rx) = mpsc::channel::<RpcResponsePayload>(1);
        let send = {
            let mut inner = lock_inner(&self.inner);
            if inner.send_frame.is_none() || (require_rpc_v1 && !inner.rpc_v1_supported) {
                return Err(WsRpcCallError::transport(anyhow!(
                    WsRpcSentinel::Unavailable
                )));
            }
            let send = inner.send_frame.clone().expect("checked non-none above");
            inner.pending.insert(id.clone(), tx);
            send
        };

        // Deferred delete of the pending entry (Go's defer func).
        let _pending_guard = PendingGuard {
            client: self,
            id: id.clone(),
        };

        let item = match send(frame) {
            Ok(item) => item,
            Err(err) => {
                return Err(WsRpcCallError::transport(anyhow!(
                    "ws rpc: send: {:#}",
                    err
                )));
            }
        };

        // giveUp resolves an abandoned request. If the frame is still queued we
        // cancel it so the writer never delivers it — a definitively-not-sent
        // outcome that is safe to HTTP-fall-back. If the writer already began
        // sending it, it may reach the server, so the outcome is uncertain and
        // the caller must NOT fall back (that would double-claim, MUL-4257).
        let give_up = || {
            if item.cancel() {
                WsRpcCallError::transport(anyhow!(WsRpcSentinel::Unavailable))
            } else {
                WsRpcCallError::transport(anyhow!(WsRpcSentinel::Uncertain))
            }
        };

        // Wait the server-side budget PLUS a grace margin: a claim that
        // committed just before the server deadline must still report back
        // before the daemon gives up and falls back to HTTP, or we would
        // double-claim (MUL-4257).
        let mut timeout = server_timeout + self.grace;
        if timeout.is_zero() {
            timeout = Duration::from_secs(5);
        }

        let resp = tokio::select! {
            resp = rx.recv() => resp,
            _ = tokio::time::sleep(timeout) => {
                // The budget elapsed. If the frame is still queued behind a
                // backpressured writer, cancel it so it is never delivered
                // after we fall back (giveUp → not-sent). If it already left
                // the writer, the outcome is uncertain and we must not fall
                // back.
                let err = give_up();
                if is_uncertain(&err.source) {
                    return Err(err);
                }
                return Err(WsRpcCallError::transport(anyhow!(
                    "ws rpc: timeout after {}: {}",
                    go_duration_string(timeout),
                    WsRpcSentinel::Unavailable
                )));
            }
            _ = ctx.cancelled() => {
                item.cancel();
                return Err(WsRpcCallError::transport(anyhow!("{}", ctx.cause())));
            }
        };

        let Some(resp) = resp else {
            // The connection detached. Whether the server saw this request
            // depends on whether the frame had already left the writer, so let
            // giveUp() decide (not-sent → safe fallback; sent → uncertain).
            return Err(give_up());
        };
        if (200..300).contains(&resp.status) {
            let body = match resp.body {
                Some(body) if !body.is_null() => {
                    Some(serde_json::from_value::<T>(body).map_err(|e| {
                        WsRpcCallError::at(resp.status, anyhow!("ws rpc: decode response: {}", e))
                    })?)
                }
                _ => None,
            };
            return Ok((resp.status, body));
        }
        let msg = if resp.error.is_empty() {
            format!("ws rpc status {}", resp.status)
        } else {
            resp.error
        };
        Err(WsRpcCallError::at(resp.status, anyhow!(msg)))
    }

    /// `deliver` (wsrpc.go:291–305): routes an inbound rpc_response frame to
    /// the waiting Call. The lookup happens under the mutex so it is
    /// serialized with attach(None)'s clear: a sender present in pending is
    /// guaranteed not yet dropped, so try_send never hits a closed receiver.
    /// Unknown request ids (already timed out / detached) are dropped.
    pub(crate) fn deliver(&self, resp: RpcResponsePayload) {
        let inner = lock_inner(&self.inner);
        if let Some(tx) = inner.pending.get(&resp.request_id) {
            let _ = tx.try_send(resp);
        }
    }
}

/// Drops the pending entry when the call scope exits (Go's deferred
/// `delete(c.pending, id)`).
struct PendingGuard<'a> {
    client: &'a WsRpcClient,
    id: String,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        lock_inner(&self.client.inner).pending.remove(&self.id);
    }
}

/// Formats a Duration the way Go's `time.Duration.String` renders whole-second
/// values (e.g. `7s`, `2m5s`) for the timeout error message.
fn go_duration_string(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 && secs.is_multiple_of(60) {
        format!("{}m0s", secs / 60)
    } else if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

// ---------------------------------------------------------------------------
// ClaimTasksWSFirst (wsrpc.go:307–380)
// ---------------------------------------------------------------------------

/// S9-integration seam: Go declares `ClaimTasksWSFirst` on `*Daemon`; the
/// Daemon struct itself belongs to another lane. This trait captures exactly
/// the Daemon surface wsrpc.go touches, following the crate's GcHost pattern.
pub(crate) trait WsClaimHost: Send + Sync {
    /// `d.batchClaimUnsupported.Load()` / `.Store(true)`.
    fn batch_claim_unsupported(&self) -> bool;
    fn set_batch_claim_unsupported(&self);

    /// `d.wsClaimHTTPFallbackAfter` (atomic unix-nanos timestamp).
    fn ws_claim_http_fallback_after_nanos(&self) -> i64;
    fn compare_and_swap_ws_claim_http_fallback_after(&self, from: i64, to: i64) -> bool;
    fn store_ws_claim_http_fallback_after_nanos(&self, nanos: i64);

    fn ws_rpc(&self) -> &WsRpcClient;

    /// `d.client.ClaimTasks`.
    fn claim_tasks_http(
        &self,
        ctx: &Ctx,
        daemon_id: &str,
        runtime_ids: &[String],
        max_tasks: usize,
    ) -> impl Future<Output = anyhow::Result<Vec<Task>>> + Send;

    /// `d.client.claimTasksLegacy`.
    fn claim_tasks_legacy(
        &self,
        ctx: &Ctx,
        runtime_ids: &[String],
        max_tasks: usize,
    ) -> impl Future<Output = anyhow::Result<Vec<Task>>> + Send;
}

fn now_unix_nanos() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or_default()
}

#[derive(Debug, Deserialize)]
struct BatchClaimResp {
    #[serde(default)]
    tasks: Vec<Task>,
}

/// `ClaimTasksWSFirst` (wsrpc.go:315–380): the WS-first claim policy
/// (MUL-4257). Issues the tasks.claim RPC over the WS control connection when
/// one is attached, and falls back to the HTTP claim endpoint on transport
/// failures that are known not to have reached the server (no connection,
/// write-buffer full, unsent timeout) or server error. A sent-frame
/// disconnect/timeout is uncertain, so it is retried over HTTP only after a
/// short safety window. An empty Vec means "no tasks this cycle" (Go's
/// `nil, nil` returns).
pub(crate) async fn claim_tasks_ws_first<H: WsClaimHost + ?Sized>(
    host: &H,
    ctx: &Ctx,
    daemon_id: &str,
    runtime_ids: &[String],
    max_tasks: usize,
) -> anyhow::Result<Vec<Task>> {
    // Un-upgraded server without the batch route: a prior poll already learned
    // this (via a 404), so go straight to the legacy per-runtime claim and
    // skip the WS + batch attempts each cycle.
    if host.batch_claim_unsupported() {
        return host.claim_tasks_legacy(ctx, runtime_ids, max_tasks).await;
    }
    let mut bypass_ws_once = false;
    let retry_after_nanos = host.ws_claim_http_fallback_after_nanos();
    if retry_after_nanos > 0 {
        let retry_after = UNIX_EPOCH + Duration::from_nanos(retry_after_nanos as u64);
        let now = SystemTime::now();
        match now.duration_since(retry_after) {
            Err(_) => {
                let remaining = retry_after.duration_since(now).unwrap_or(Duration::ZERO);
                tracing::debug!(
                    retry_after = %go_duration_string_round_ms(remaining),
                    "ws claim outcome uncertain; delaying http fallback until safety window elapses"
                );
                return Ok(Vec::new());
            }
            Ok(_) => {
                if host.compare_and_swap_ws_claim_http_fallback_after(retry_after_nanos, 0) {
                    bypass_ws_once = true;
                    tracing::debug!(
                        "previous ws claim outcome uncertain; using http fallback for this claim cycle"
                    );
                }
            }
        }
    }
    if !bypass_ws_once && host.ws_rpc().supports_rpc_v1() {
        // batchClaimRequestTimeout is the server-side execution budget; the
        // daemon waits that plus the client's grace margin for the response.
        let body = serde_json::json!({
            "daemon_id": daemon_id,
            "runtime_ids": runtime_ids,
            "max_tasks": max_tasks,
        });
        match host
            .ws_rpc()
            .call_if_rpc_v1_supported::<BatchClaimResp, _>(
                ctx,
                "tasks.claim",
                BATCH_CLAIM_REQUEST_TIMEOUT,
                Some(&body),
            )
            .await
        {
            Ok((_, resp)) => return Ok(resp.map(|r| r.tasks).unwrap_or_default()),
            Err(err) => {
                if is_uncertain(&err.source) {
                    // The WS claim may have committed server-side; claiming
                    // the same free slots again over HTTP immediately would
                    // double-claim. Skip this cycle, then force one HTTP batch
                    // claim after the server-side execution budget plus
                    // response grace has elapsed. If the WS claim committed,
                    // the task is already dispatched and stale reclaim owns
                    // recovery; if it did not, HTTP regains liveness for the
                    // queued task.
                    host.store_ws_claim_http_fallback_after_nanos(
                        now_unix_nanos() + ws_claim_uncertain_fallback_delay().as_nanos() as i64,
                    );
                    tracing::debug!(
                        retry_after = ?ws_claim_uncertain_fallback_delay(),
                        "ws claim outcome uncertain after disconnect; delaying http fallback"
                    );
                    return Ok(Vec::new());
                }
                tracing::debug!(error = %err.source, "ws claim failed; falling back to http");
            }
        }
    }
    match host
        .claim_tasks_http(ctx, daemon_id, runtime_ids, max_tasks)
        .await
    {
        Ok(tasks) => Ok(tasks),
        Err(err) => {
            // Server has no batch route (404): freeze the old API contract by
            // falling back to the legacy per-runtime claim loop, and remember
            // it so we don't re-probe every cycle.
            if crate::client::is_batch_claim_unsupported(&err) {
                host.set_batch_claim_unsupported();
                tracing::info!(
                    "batch claim route unsupported by server; using legacy per-runtime claim"
                );
                return host.claim_tasks_legacy(ctx, runtime_ids, max_tasks).await;
            }
            Err(err)
        }
    }
}

/// Renders a remaining delay like Go's `retryAfter.Sub(now).Round(time.Millisecond)`
/// debug field (millisecond precision).
fn go_duration_string_round_ms(d: Duration) -> String {
    let millis = d.as_millis();
    if millis >= 1000 {
        go_duration_string(Duration::from_millis(millis as u64))
    } else {
        format!("{}ms", millis)
    }
}

#[cfg(test)]
mod tests {
    //! Ports of the pure-logic cases from wsrpc_test.go (279 lines): outbound
    //! cancel/sent races, attach/detach semantics, negotiation gating, and the
    //! deliver path. Full socket-loop cases stay with the wakeup lane.

    use super::*;

    fn noop_sender() -> SendFrameFn {
        Arc::new(|frame| Ok(Arc::new(WsOutbound::new(frame))))
    }

    #[test]
    fn outbound_cancel_before_write_skips_delivery() {
        let out = WsOutbound::new(b"{}".to_vec());
        assert!(out.cancel());
        assert!(!out.begin_write(), "cancelled frame must not be written");
    }

    #[test]
    fn outbound_begin_write_wins_race_over_cancel() {
        let out = WsOutbound::new(b"{}".to_vec());
        assert!(out.begin_write());
        assert!(!out.cancel(), "cancel after begin_write must lose");
    }

    #[tokio::test]
    async fn call_without_connection_is_unavailable() {
        let client = WsRpcClient::new(WS_RPC_RESPONSE_GRACE);
        let ctx = Ctx::new();
        let err = client
            .call::<serde_json::Value, _>(
                &ctx,
                "tasks.claim",
                Duration::from_secs(1),
                Some(&serde_json::json!({})),
            )
            .await
            .unwrap_err();
        assert_eq!(err.status, 0);
        assert_eq!(sentinel_of(&err.source), Some(WsRpcSentinel::Unavailable));
    }

    #[tokio::test]
    async fn call_requires_rpc_v1_when_gated() {
        let client = WsRpcClient::new(WS_RPC_RESPONSE_GRACE);
        client.attach(Some(noop_sender()));
        let ctx = Ctx::new();
        let err = client
            .call_if_rpc_v1_supported::<serde_json::Value, _>(
                &ctx,
                "tasks.claim",
                Duration::from_secs(1),
                None::<&[u8]>,
            )
            .await
            .unwrap_err();
        assert_eq!(sentinel_of(&err.source), Some(WsRpcSentinel::Unavailable));
    }

    #[tokio::test]
    async fn negotiation_does_not_survive_reconnect() {
        let client = WsRpcClient::new(WS_RPC_RESPONSE_GRACE);
        let gen = client.attach(Some(noop_sender()));
        client.mark_rpc_v1_supported(gen);
        assert!(client.supports_rpc_v1());
        // Re-attach clears the capability and bumps the generation.
        let gen2 = client.attach(Some(noop_sender()));
        assert_ne!(gen, gen2);
        assert!(!client.supports_rpc_v1());
        // A stale ack from the old generation cannot re-enable the new one.
        client.mark_rpc_v1_supported(gen);
        assert!(!client.supports_rpc_v1());
        client.mark_rpc_v1_supported(gen2);
        assert!(client.supports_rpc_v1());
    }

    #[tokio::test]
    async fn detach_fails_pending_calls_as_unavailable() {
        let client = Arc::new(WsRpcClient::new(Duration::from_secs(5)));
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Arc<WsOutbound>>(4);
        let gen = client.attach(Some(Arc::new(move |frame| {
            let out = Arc::new(WsOutbound::new(frame));
            let _ = tx.try_send(out.clone());
            Ok(out)
        })));
        client.mark_rpc_v1_supported(gen);

        let call_client = client.clone();
        let handle = tokio::spawn(async move {
            call_client
                .call_if_rpc_v1_supported::<serde_json::Value, _>(
                    &Ctx::new(),
                    "tasks.claim",
                    Duration::from_secs(10),
                    None::<&[u8]>,
                )
                .await
        });

        // Writer consumes the queued frame (begin_write wins the race), then
        // the connection detaches before any response arrives.
        let out = rx.recv().await.expect("frame queued");
        assert!(out.begin_write());
        drop(rx);
        client.attach(None);

        let err = handle.await.unwrap().unwrap_err();
        assert_eq!(err.status, 0);
        assert!(
            is_uncertain(&err.source),
            "sent-frame detach must be uncertain, got {:?}",
            err.source
        );
    }

    #[tokio::test]
    async fn deliver_routes_response_to_waiting_call() {
        let client = Arc::new(WsRpcClient::new(Duration::from_secs(5)));
        let gen = client.attach(Some(noop_sender()));
        client.mark_rpc_v1_supported(gen);

        let call_client = client.clone();
        let handle = tokio::spawn(async move {
            call_client
                .call_if_rpc_v1_supported::<serde_json::Value, _>(
                    &Ctx::new(),
                    "tasks.claim",
                    Duration::from_secs(10),
                    Some(&serde_json::json!({"max_tasks": 3})),
                )
                .await
        });

        // Wait until the request registers, then deliver its response.
        for _ in 0..200 {
            {
                let inner = client.inner.lock().unwrap_or_else(|e| e.into_inner());
                if inner.pending.len() == 1 {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let request_id = {
            let inner = client.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.pending.keys().next().cloned().expect("pending call")
        };
        client.deliver(RpcResponsePayload {
            request_id,
            status: 400,
            body: None,
            error: "nope".to_string(),
        });

        let (status, _) = handle.await.unwrap().unwrap_err().into_parts();
        assert_eq!(status, 400);
    }

    impl WsRpcCallError {
        fn into_parts(self) -> (i32, anyhow::Error) {
            (self.status, self.source)
        }
    }
}
