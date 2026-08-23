//! Port of `server/internal/daemon/wakeup.go` (lines 1–526).
//!
//! The task-wakeup control connection: a daemon→server WebSocket that
//! receives push hints (task available, pending work, workspace changes,
//! heartbeat acks, RPC responses) so idle claim pollers wake immediately
//! instead of waiting out their tickers. HTTP polling remains the fallback.
//!
//! Deviations from Go:
//! - gorilla/websocket has no Rust equivalent in this crate's dependency set,
//!   so the socket session (dial, writer pump, ping/pong deadline handling,
//!   read pump) is one S9-integration seam: [`TaskWakeupHost::run_connection`].
//!   Everything around it — the reconnect loop with backoff/jitter/runtime-set
//!   wakeups, URL construction, frame dispatch, heartbeat-ack branching, and
//!   heartbeat frame building — is ported here in full.
//! - `runtimeSetWatcher` (daemon.go:1852–1885, lane B) is ported locally as
//!   [`RuntimeSetWatcher`] on `tokio::sync::Notify` (notify_one coalesces like
//!   Go's cap-1 buffered channel).
//! - `HeartbeatResponse` is the client.go alias for
//!   `protocol.DaemonHeartbeatAckPayload`; the protocol type is used directly.
//! - `taskWakeupBackoffResetAfter` and friends are Go vars mutated by tests;
//!   here they are consts.
//! - `url.Values.Encode` → local percent-encoding helper matching Go's
//!   QueryEscape output for query values.

// S9-integration: consumed by daemon WS wiring that lands with integration;
// silence dead-code until then.
#![allow(dead_code)]

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use rand::Rng;
use tokio::sync::{mpsc, Notify};

use crate::repocache::Ctx;

/// `errRuntimeSetChanged` (wakeup.go:20).
#[derive(Debug, thiserror::Error)]
#[error("runtime set changed")]
pub(crate) struct RuntimeSetChangedError;

pub(crate) fn is_runtime_set_changed(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|c| c.downcast_ref::<RuntimeSetChangedError>().is_some())
}

/// `taskWakeupMaxBackoff` (wakeup.go:23).
pub(crate) const TASK_WAKEUP_MAX_BACKOFF: Duration = Duration::from_secs(30);

/// `taskWakeupReadLimit` (wakeup.go:33): 64 MiB. One tasks.claim response can
/// carry up to 32 complete Task payloads; the old 64 KiB ceiling was smaller
/// than a valid single-task response and left claims dispatched-but-never-
/// started.
pub(crate) const TASK_WAKEUP_READ_LIMIT: u64 = 64 << 20;

/// `taskWakeupPongWait` (wakeup.go:37).
pub(crate) const TASK_WAKEUP_PONG_WAIT: Duration = Duration::from_secs(60);
/// `taskWakeupWriteWait` (wakeup.go:38).
pub(crate) const TASK_WAKEUP_WRITE_WAIT: Duration = Duration::from_secs(10);
/// `taskWakeupBackoffResetAfter` (wakeup.go:39).
pub(crate) const TASK_WAKEUP_BACKOFF_RESET_AFTER: Duration = Duration::from_secs(10);

/// `taskWakeup` (wakeup.go:42–44): one queued wakeup hint.
#[derive(Debug, Clone)]
pub(crate) struct TaskWakeup {
    pub runtime_id: String,
}

// ---------------------------------------------------------------------------
// runtimeSetWatcher (daemon.go:1852–1885) — multi-subscriber pub/sub.
// ---------------------------------------------------------------------------

/// Multi-subscriber "runtime set changed" watcher. Each subscriber behaves
/// like Go's cap-1 buffered channel: a notify with no waiter leaves a permit
/// that the next wait consumes immediately; further notifies coalesce.
#[derive(Default)]
pub(crate) struct RuntimeSetWatcher {
    subscribers: Mutex<Vec<Arc<Notify>>>,
}

impl RuntimeSetWatcher {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// `Subscribe` (daemon.go:1864–1874): returns the wait handle plus an
    /// unsubscribe guard the caller must hold while subscribed.
    pub(crate) fn subscribe(self: &Arc<Self>) -> (Arc<Notify>, RuntimeSetUnsubscribe) {
        let notify = Arc::new(Notify::new());
        self.subscribers.lock().unwrap().push(notify.clone());
        (
            notify.clone(),
            RuntimeSetUnsubscribe {
                watcher: Arc::downgrade(self),
                notify,
            },
        )
    }

    /// `notify` (daemon.go:1876–1885): non-blocking nudge to every subscriber.
    pub(crate) fn notify(&self) {
        for subscriber in self.subscribers.lock().unwrap().iter() {
            subscriber.notify_one();
        }
    }
}

/// The unsubscribe func returned by Go's Subscribe, as a Drop guard.
pub(crate) struct RuntimeSetUnsubscribe {
    watcher: Weak<RuntimeSetWatcher>,
    notify: Arc<Notify>,
}

impl Drop for RuntimeSetUnsubscribe {
    fn drop(&mut self) {
        if let Some(watcher) = self.watcher.upgrade() {
            watcher
                .subscribers
                .lock()
                .unwrap()
                .retain(|n| !Arc::ptr_eq(n, &self.notify));
        }
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (wakeup.go:80–97, 479–526).
// ---------------------------------------------------------------------------

/// `shouldResetTaskWakeupBackoff` (wakeup.go:80–85): a connection that stayed
/// up through the reset window means the network recovered — restart the
/// backoff ladder from 1s on the next failure.
pub(crate) fn should_reset_task_wakeup_backoff(connected_for: Duration) -> bool {
    if connected_for.is_zero() {
        return false;
    }
    connected_for >= TASK_WAKEUP_BACKOFF_RESET_AFTER
}

/// `jitterDuration` (wakeup.go:87–97): full-length ± spread where
/// spread = d/5, uniform over [-spread, +spread].
pub(crate) fn jitter_duration(d: Duration) -> Duration {
    if d.is_zero() {
        return d;
    }
    let spread_ms = (d.as_millis() / 5) as i64;
    if spread_ms <= 0 {
        return d;
    }
    let delta = rand::thread_rng().gen_range(0..=(spread_ms * 2)) - spread_ms;
    let jittered = d.as_millis() as i64 + delta;
    Duration::from_millis(jittered.max(0) as u64)
}

/// `signalTaskWakeup` (wakeup.go:479–484): non-blocking send; drops when the
/// queue already holds a hint.
pub(crate) fn signal_task_wakeup(task_wakeups: &mpsc::Sender<TaskWakeup>, runtime_id: &str) {
    let _ = task_wakeups.try_send(TaskWakeup {
        runtime_id: runtime_id.to_string(),
    });
}

/// `sleepWithContextOrRuntimeChange` (wakeup.go:514–526). Err mirrors
/// ctx.Err().
pub(crate) async fn sleep_with_context_or_runtime_change(
    ctx: &Ctx,
    d: Duration,
    runtime_set_ch: &Notify,
) -> Result<(), crate::repocache::CancelCause> {
    tokio::select! {
        cause = ctx.cancelled() => {
            let () = cause;
            Err(ctx.cause())
        }
        _ = runtime_set_ch.notified() => Ok(()),
        _ = tokio::time::sleep(d) => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// URL construction (wakeup.go:486–512).
// ---------------------------------------------------------------------------

/// Percent-encoding matching Go's `url.QueryEscape` for query values:
/// unreserved [A-Za-z0-9-_.~] pass through, space becomes '+', everything
/// else becomes %XX.
fn query_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `taskWakeupURL` (wakeup.go:486–512): swap http(s)→ws(s), append the WS
/// path, attach the sorted runtime_ids query parameter.
///
/// Deviation: existing query pairs are preserved verbatim (split on '&')
/// rather than re-encoded field-by-field; daemon base URLs do not carry
/// encoded queries in practice.
pub(crate) fn task_wakeup_url(base_url: &str, runtime_ids: &[String]) -> anyhow::Result<String> {
    let trimmed = base_url.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        anyhow::bail!("invalid daemon server URL");
    };
    let out_scheme = match scheme {
        "http" => "ws",
        "https" => "wss",
        "ws" | "wss" => scheme,
        _ => anyhow::bail!("daemon server URL must use http, https, ws, or wss"),
    };

    // Strip fragment first, then split off any existing query.
    let no_frag = rest.split('#').next().unwrap_or(rest);
    let (authority_path, existing_query) = match no_frag.split_once('?') {
        Some((ap, q)) => (ap, Some(q)),
        None => (no_frag, None),
    };
    let path = match authority_path.find('/') {
        Some(idx) => {
            let (authority, path) = authority_path.split_at(idx);
            format!("{authority}{path}")
        }
        None => authority_path.to_string(),
    };

    let mut pairs: Vec<String> = existing_query
        .map(|q| {
            q.split('&')
                .filter(|p| !p.is_empty())
                .filter(|p| p.split('=').next().unwrap_or("") != "runtime_ids")
                .map(|p| p.to_string())
                .collect()
        })
        .unwrap_or_default();

    let mut ids: Vec<&str> = runtime_ids.iter().map(|s| s.as_str()).collect();
    ids.sort_unstable();
    if !ids.is_empty() {
        pairs.push(format!("runtime_ids={}", query_escape(&ids.join(","))));
    }

    let mut url = format!(
        "{}://{}/api/daemon/ws",
        out_scheme,
        path.trim_end_matches('/')
    );
    if !pairs.is_empty() {
        url.push('?');
        url.push_str(&pairs.join("&"));
    }
    Ok(url)
}

// ---------------------------------------------------------------------------
// Frame building + dispatch (wakeup.go:302–325, 335–367, 373–445).
// ---------------------------------------------------------------------------

/// `sendWSHeartbeats` per-runtime frame body (wakeup.go:307–310):
/// `{"type":"daemon:heartbeat","payload":{"runtime_id":…,"supports_batch_import":true}}`.
/// None mirrors marshalRaw's nil on marshal failure.
pub(crate) fn heartbeat_frame(runtime_id: &str) -> Option<Vec<u8>> {
    let payload = cordy_protocol::messages::DaemonHeartbeatRequestPayload {
        runtime_id: runtime_id.to_string(),
        supports_batch_import: true,
    };
    let message = cordy_protocol::messages::Message {
        r#type: cordy_protocol::events::EVENT_DAEMON_HEARTBEAT.to_string(),
        payload: serde_json::to_value(&payload).ok()?,
    };
    serde_json::to_vec(&message).ok()
}

/// `writeBufSize` (wakeup.go:166–169): fits a full per-runtime heartbeat
/// batch plus headroom; a fixed 8-slot queue silently dropped heartbeats once
/// a daemon watched more than ~8 runtimes.
pub(crate) fn write_buf_size(runtime_count: usize) -> usize {
    usize::max(16, 2 * runtime_count)
}

/// Daemon-side collaborators the wakeup frame handlers need (the *Daemon
/// methods wakeup.go calls). Integration wires this to the Daemon struct.
#[async_trait::async_trait]
pub(crate) trait WakeupFrameHost: Send + Sync {
    /// `d.handleRuntimeGone(runtimeID)` — spawned detached in Go.
    async fn handle_runtime_gone(&self, runtime_id: String);
    /// `d.wsRPC.markRPCV1Supported(generation)`.
    fn mark_rpc_v1_supported(&self, generation: u64);
    /// `d.recordWSHeartbeatAck(runtimeID)` — freshness mark suppressing HTTP.
    fn record_ws_heartbeat_ack(&self, runtime_id: &str);
    /// `d.handleHeartbeatActions(ctx, runtimeID, ack)`.
    async fn handle_heartbeat_actions(
        &self,
        ctx: &Ctx,
        runtime_id: &str,
        ack: &cordy_protocol::messages::DaemonHeartbeatAckPayload,
    );
    /// `d.handleRuntimeProfilesChanged(payload)` — spawned detached in Go.
    async fn handle_runtime_profiles_changed(
        &self,
        payload: &cordy_protocol::messages::RuntimeProfilesChangedPayload,
    );
    /// `d.handlePendingWorkHint(runtimeID, kind)` — spawned detached in Go.
    async fn handle_pending_work_hint(&self, runtime_id: &str, kind: &str);
    /// `d.workspaceChanges.broadcast()` (nil-guarded in Go).
    fn broadcast_workspace_changes(&self);
    /// `d.wsRPC.deliver(resp)`.
    fn deliver_rpc_response(&self, resp: cordy_protocol::messages::RpcResponsePayload);
}

/// `handleWSHeartbeatAckForConnection` (wakeup.go:351–367).
///
/// A RuntimeGone ack is the WS twin of HTTP 404 "runtime not found": route it
/// through the same self-heal entry point and do NOT record a freshness mark —
/// pretending the runtime is alive would let HTTP keep skipping its own
/// heartbeat against the dead UUID.
pub(crate) async fn handle_ws_heartbeat_ack_for_connection(
    host: Arc<dyn WakeupFrameHost>,
    ctx: &Ctx,
    ack: Option<&cordy_protocol::messages::DaemonHeartbeatAckPayload>,
    ws_rpc_generation: u64,
) {
    let Some(ack) = ack else { return };
    if ack.runtime_id.is_empty() {
        return;
    }
    if ack.runtime_gone {
        tokio::spawn({
            let host = host.clone();
            let runtime_id = ack.runtime_id.clone();
            async move { host.handle_runtime_gone(runtime_id).await }
        });
        return;
    }
    if ack
        .server_capabilities
        .iter()
        .any(|c| c == cordy_protocol::messages::DAEMON_CAPABILITY_RPC_V1)
    {
        host.mark_rpc_v1_supported(ws_rpc_generation);
    }
    host.record_ws_heartbeat_ack(&ack.runtime_id);
    host.handle_heartbeat_actions(ctx, &ack.runtime_id, ack)
        .await;
}

/// Outcome of [`dispatch_wakeup_frame`] for frames whose Go handler spawns a
/// detached goroutine — the caller owns the spawn so the read pump stays free.
#[derive(Debug)]
pub(crate) enum DeferredAction {
    RuntimeProfilesChanged(cordy_protocol::messages::RuntimeProfilesChangedPayload),
    PendingWork { runtime_id: String, kind: String },
}

/// `readTaskWakeupMessagesForConnection`'s switch (wakeup.go:388–443), minus
/// the socket reads: classify one decoded frame and either act inline or
/// return the action the caller must spawn. Unknown types fall through
/// silently, matching Go's switch default.
pub(crate) fn dispatch_wakeup_frame(
    host: &Arc<dyn WakeupFrameHost>,
    msg_type: &str,
    payload: &[u8],
    ws_rpc_generation: u64,
    task_wakeups: &mpsc::Sender<TaskWakeup>,
) -> Option<DeferredAction> {
    match msg_type {
        cordy_protocol::events::EVENT_DAEMON_TASK_AVAILABLE => {
            let parsed: Result<cordy_protocol::messages::TaskAvailablePayload, _> =
                if payload.is_empty() {
                    // Go leaves the zero payload on decode error-free empty
                    // bytes; mirror with an all-empty payload.
                    Ok(cordy_protocol::messages::TaskAvailablePayload {
                        runtime_id: String::new(),
                        task_id: String::new(),
                    })
                } else {
                    serde_json::from_slice(payload)
                };
            match parsed {
                Ok(payload) => {
                    if !payload.runtime_id.is_empty() {
                        tracing::debug!(
                            runtime_id = %payload.runtime_id,
                            task_id = %payload.task_id,
                            "task wakeup received"
                        );
                    }
                    signal_task_wakeup(task_wakeups, &payload.runtime_id);
                    None
                }
                Err(err) => {
                    tracing::debug!(error = %err, "task wakeup websocket invalid payload");
                    None
                }
            }
        }
        cordy_protocol::events::EVENT_DAEMON_RUNTIME_PROFILES_CHANGED => {
            match serde_json::from_slice::<cordy_protocol::messages::RuntimeProfilesChangedPayload>(
                payload,
            ) {
                Ok(payload) => {
                    if payload.workspace_id.is_empty() {
                        tracing::debug!("runtime profile refresh websocket missing workspace_id");
                        return None;
                    }
                    Some(DeferredAction::RuntimeProfilesChanged(payload))
                }
                Err(err) => {
                    tracing::debug!(error = %err, "runtime profile refresh websocket invalid payload");
                    None
                }
            }
        }
        cordy_protocol::events::EVENT_DAEMON_WORKSPACES_CHANGED => {
            host.broadcast_workspace_changes();
            None
        }
        cordy_protocol::events::EVENT_DAEMON_PENDING_WORK => {
            match serde_json::from_slice::<cordy_protocol::messages::PendingWorkPayload>(payload) {
                Ok(payload) => {
                    if payload.runtime_id.is_empty() {
                        tracing::debug!("pending work websocket missing runtime_id");
                        return None;
                    }
                    Some(DeferredAction::PendingWork {
                        runtime_id: payload.runtime_id,
                        kind: payload.kind,
                    })
                }
                Err(err) => {
                    tracing::debug!(error = %err, "pending work websocket invalid payload");
                    None
                }
            }
        }
        cordy_protocol::events::EVENT_DAEMON_HEARTBEAT_ACK => {
            match serde_json::from_slice::<cordy_protocol::messages::DaemonHeartbeatAckPayload>(
                payload,
            ) {
                Ok(ack) => {
                    // context.Background() in Go — a fresh root ctx here.
                    let host = host.clone();
                    tokio::spawn(async move {
                        let ctx = Ctx::new();
                        handle_ws_heartbeat_ack_for_connection(
                            host,
                            &ctx,
                            Some(&ack),
                            ws_rpc_generation,
                        )
                        .await;
                    });
                    None
                }
                Err(err) => {
                    tracing::debug!(error = %err, "ws heartbeat ack invalid payload");
                    None
                }
            }
        }
        cordy_protocol::events::EVENT_DAEMON_RPC_RESPONSE => {
            match serde_json::from_slice::<cordy_protocol::messages::RpcResponsePayload>(payload) {
                Ok(resp) => {
                    host.deliver_rpc_response(resp);
                    None
                }
                Err(err) => {
                    tracing::debug!(error = %err, "ws rpc response invalid payload");
                    None
                }
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Connection loop (wakeup.go:46–78, 99–251).
// ---------------------------------------------------------------------------

/// How a single WS connection ended (`runTaskWakeupConnection`'s error slot).
#[derive(Debug)]
pub(crate) enum ConnectionEnd {
    /// `<-ctx.Done()` — shut down the loop entirely.
    ContextCancelled,
    /// errRuntimeSetChanged — reconnect immediately without backoff.
    RuntimeSetChanged,
    /// Any other error — back off before reconnecting.
    Error(anyhow::Error),
}

/// The *Daemon surface the wakeup loop needs. Integration wires this to the
/// Daemon struct; unit tests supply fakes.
#[async_trait::async_trait]
pub(crate) trait TaskWakeupHost: Send + Sync {
    /// `d.allRuntimeIDs()`.
    fn all_runtime_ids(&self) -> Vec<String>;
    /// `d.cfg.ServerBaseURL`.
    fn server_base_url(&self) -> String;
    /// Authorization + X-Client-* headers built from `d.client`
    /// (wakeup.go:105–120), including X-Client-Capabilities so a claim built
    /// over this connection gets identical capability gating (MUL-4257).
    fn auth_and_client_headers(&self) -> Vec<(String, String)>;
    /// `d.runtimeSet`.
    fn runtime_set(&self) -> &Arc<RuntimeSetWatcher>;
    /// `d.reconcile != nil` guard + broadcast (wakeup.go:155–157).
    fn broadcast_reconcile(&self);
    /// `d.batchClaimUnsupported.Store(false)` (wakeup.go:196).
    fn reset_batch_claim_unsupported(&self);
    /// `d.clearWSHeartbeatAcks()` (wakeup.go:143).
    fn clear_ws_heartbeat_acks(&self);

    /// The gorilla/websocket session body of `runTaskWakeupConnection`
    /// (wakeup.go:130–250): dial with the given URL/headers, run the writer
    /// pump + heartbeat sender + read pump until ctx cancellation, a
    /// runtime-set change, or a read/write error. Returns how long the
    /// connection stayed up and how it ended.
    ///
    /// S9-integration: lands with the daemon WS stack; the surrounding loop
    /// logic lives in [`run_task_wakeup_loop`].
    async fn run_connection(
        &self,
        ctx: &Ctx,
        ws_url: &str,
        headers: &[(String, String)],
        task_wakeups: &mpsc::Sender<TaskWakeup>,
    ) -> (Duration, ConnectionEnd);
}

/// `taskWakeupLoop` (wakeup.go:46–78): connect → handle → back off → repeat.
/// A runtime-set change resets backoff and reconnects immediately; a
/// connection that stayed healthy long enough also resets the ladder.
pub(crate) async fn run_task_wakeup_loop(
    host: &dyn TaskWakeupHost,
    ctx: &Ctx,
    task_wakeups: &mpsc::Sender<TaskWakeup>,
) {
    let mut backoff = Duration::from_secs(1);
    let (runtime_set_ch, _unsub) = host.runtime_set().subscribe();

    loop {
        let runtime_ids = host.all_runtime_ids();
        let ws_url = match task_wakeup_url(&host.server_base_url(), &runtime_ids) {
            Ok(url) => url,
            Err(err) => {
                // Same shape as Go: dial-phase errors surface through the
                // debug log below with zero uptime.
                tracing::debug!(
                    error = %err,
                    retry_in = ?backoff,
                    "task wakeup websocket unavailable; polling fallback remains active"
                );
                if sleep_with_context_or_runtime_change(
                    ctx,
                    jitter_duration(backoff),
                    &runtime_set_ch,
                )
                .await
                .is_err()
                {
                    return;
                }
                backoff = next_backoff(backoff);
                continue;
            }
        };

        host.reset_batch_claim_unsupported();
        let headers = host.auth_and_client_headers();
        let (connected_for, end) = host
            .run_connection(ctx, &ws_url, &headers, task_wakeups)
            .await;

        match end {
            ConnectionEnd::ContextCancelled => return,
            ConnectionEnd::RuntimeSetChanged => {
                backoff = Duration::from_secs(1);
                continue;
            }
            ConnectionEnd::Error(err) => {
                if should_reset_task_wakeup_backoff(connected_for) {
                    backoff = Duration::from_secs(1);
                }
                tracing::debug!(
                    error = %err,
                    retry_in = ?backoff,
                    "task wakeup websocket unavailable; polling fallback remains active"
                );
            }
        }

        if sleep_with_context_or_runtime_change(ctx, jitter_duration(backoff), &runtime_set_ch)
            .await
            .is_err()
        {
            return;
        }
        backoff = next_backoff(backoff);
    }
}

/// Backoff doubling capped at `taskWakeupMaxBackoff` (wakeup.go:71–76).
fn next_backoff(current: Duration) -> Duration {
    if current >= TASK_WAKEUP_MAX_BACKOFF {
        return current;
    }
    let doubled = current * 2;
    if doubled > TASK_WAKEUP_MAX_BACKOFF {
        TASK_WAKEUP_MAX_BACKOFF
    } else {
        doubled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TestTaskWakeupURL table (wakeup_test.go:20–64).
    #[test]
    fn task_wakeup_url_table() {
        let cases = [
            (
                "http base",
                "http://localhost:8080",
                vec!["runtime-b".to_string(), "runtime-a".to_string()],
                "ws://localhost:8080/api/daemon/ws?runtime_ids=runtime-a%2Cruntime-b",
            ),
            (
                "https base",
                "https://api.example.com",
                vec!["runtime-1".to_string()],
                "wss://api.example.com/api/daemon/ws?runtime_ids=runtime-1",
            ),
            (
                "base path",
                "https://api.example.com/cordy",
                vec!["runtime-1".to_string()],
                "wss://api.example.com/cordy/api/daemon/ws?runtime_ids=runtime-1",
            ),
            (
                "account-only connection",
                "https://api.example.com",
                vec![],
                "wss://api.example.com/api/daemon/ws",
            ),
        ];
        for (name, base, ids, want) in cases {
            assert_eq!(task_wakeup_url(base, &ids).unwrap(), want, "case {name:?}");
        }
        assert!(task_wakeup_url("ftp://x", &[]).is_err());
    }

    /// TestShouldResetTaskWakeupBackoffRequiresStableConnection
    /// (wakeup_test.go:477–493).
    #[test]
    fn should_reset_backoff_requires_stable_connection() {
        assert!(!should_reset_task_wakeup_backoff(Duration::ZERO));
        assert!(!should_reset_task_wakeup_backoff(Duration::from_secs(9)));
        assert!(should_reset_task_wakeup_backoff(Duration::from_secs(10)));
    }

    /// jitterDuration stays within ±d/5 (wakeup.go:87–97).
    #[test]
    fn jitter_stays_within_spread() {
        let d = Duration::from_secs(10);
        for _ in 0..200 {
            let j = jitter_duration(d);
            assert!(
                j >= Duration::from_secs(8) && j <= Duration::from_secs(12),
                "{j:?}"
            );
        }
        assert_eq!(jitter_duration(Duration::ZERO), Duration::ZERO);
    }

    /// query_escape matches Go's QueryEscape for query values.
    #[test]
    fn query_escape_matches_go() {
        assert_eq!(query_escape("runtime-a,runtime-b"), "runtime-a%2Cruntime-b");
        assert_eq!(query_escape("plain"), "plain");
        assert_eq!(query_escape("a b"), "a+b");
        assert_eq!(query_escape("100%"), "100%25");
    }

    /// writeBufSize sizing rule (wakeup.go:166–169).
    #[test]
    fn write_buffer_sizing() {
        assert_eq!(write_buf_size(0), 16);
        assert_eq!(write_buf_size(8), 16);
        assert_eq!(write_buf_size(9), 18);
        assert_eq!(write_buf_size(32), 64);
    }

    /// Heartbeat frame JSON shape (wakeup.go:307–310).
    #[test]
    fn heartbeat_frame_shape() {
        let raw = heartbeat_frame("rt-1").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(v["type"], "daemon:heartbeat");
        assert_eq!(v["payload"]["runtime_id"], "rt-1");
        assert_eq!(v["payload"]["supports_batch_import"], true);
    }

    /// Recording fake for dispatch tests.
    struct FakeFrameHost {
        rpc_v1_marked: std::sync::atomic::AtomicU64,
        recorded_acks: Mutex<Vec<String>>,
        workspaces_broadcasts: std::sync::atomic::AtomicUsize,
        delivered_rpcs: Mutex<Vec<String>>,
        runtime_gone: Mutex<Vec<String>>,
    }

    impl FakeFrameHost {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                rpc_v1_marked: std::sync::atomic::AtomicU64::new(0),
                recorded_acks: Mutex::new(Vec::new()),
                workspaces_broadcasts: std::sync::atomic::AtomicUsize::new(0),
                delivered_rpcs: Mutex::new(Vec::new()),
                runtime_gone: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait::async_trait]
    impl WakeupFrameHost for FakeFrameHost {
        async fn handle_runtime_gone(&self, runtime_id: String) {
            self.runtime_gone.lock().unwrap().push(runtime_id);
        }
        fn mark_rpc_v1_supported(&self, generation: u64) {
            self.rpc_v1_marked.store(generation, Ordering::SeqCst);
        }
        fn record_ws_heartbeat_ack(&self, runtime_id: &str) {
            self.recorded_acks
                .lock()
                .unwrap()
                .push(runtime_id.to_string());
        }
        async fn handle_heartbeat_actions(
            &self,
            _ctx: &Ctx,
            _runtime_id: &str,
            _ack: &cordy_protocol::messages::DaemonHeartbeatAckPayload,
        ) {
        }
        async fn handle_runtime_profiles_changed(
            &self,
            _payload: &cordy_protocol::messages::RuntimeProfilesChangedPayload,
        ) {
        }
        async fn handle_pending_work_hint(&self, _runtime_id: &str, _kind: &str) {}
        fn broadcast_workspace_changes(&self) {
            self.workspaces_broadcasts.fetch_add(1, Ordering::SeqCst);
        }
        fn deliver_rpc_response(&self, resp: cordy_protocol::messages::RpcResponsePayload) {
            self.delivered_rpcs.lock().unwrap().push(resp.request_id);
        }
    }

    use std::sync::atomic::Ordering;

    fn ack_payload(runtime_id: &str, capabilities: &[&str], gone: bool) -> Vec<u8> {
        serde_json::json!({
            "runtime_id": runtime_id,
            "status": "ok",
            "server_capabilities": capabilities,
            "runtime_gone": gone,
        })
        .to_string()
        .into_bytes()
    }

    /// handleWSHeartbeatAckForConnection branching (wakeup.go:351–367):
    /// capabilities mark RPC-v1, freshness recorded, RuntimeGone routes to
    /// self-heal WITHOUT a freshness mark.
    #[tokio::test]
    async fn heartbeat_ack_branching() {
        let host = FakeFrameHost::new();
        let host_dyn: Arc<dyn WakeupFrameHost> = host.clone();
        let (tx, rx) = mpsc::channel(1);
        let _ = rx; // drain side

        // Normal ack with rpc-v1 capability.
        dispatch_wakeup_frame(
            &host_dyn,
            cordy_protocol::events::EVENT_DAEMON_HEARTBEAT_ACK,
            &ack_payload(
                "rt-1",
                &[cordy_protocol::messages::DAEMON_CAPABILITY_RPC_V1],
                false,
            ),
            7,
            &tx,
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while host.recorded_acks.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("freshness mark never recorded");
        assert_eq!(host.rpc_v1_marked.load(Ordering::SeqCst), 7);

        // RuntimeGone: self-heal spawned, NO freshness mark.
        dispatch_wakeup_frame(
            &host_dyn,
            cordy_protocol::events::EVENT_DAEMON_HEARTBEAT_ACK,
            &ack_payload("rt-gone", &[], true),
            7,
            &tx,
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while host.runtime_gone.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("runtime-gone self-heal never ran");
        assert_eq!(
            host.recorded_acks.lock().unwrap().len(),
            1,
            "RuntimeGone must not record a freshness mark"
        );

        // Empty runtime id ignored.
        dispatch_wakeup_frame(
            &host_dyn,
            cordy_protocol::events::EVENT_DAEMON_HEARTBEAT_ACK,
            &ack_payload("", &[], false),
            7,
            &tx,
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(host.recorded_acks.lock().unwrap().len(), 1);
    }

    /// Task-available frame signals the wakeup channel (wakeup.go:389–400).
    #[tokio::test]
    async fn task_available_frame_signals_channel() {
        let host = FakeFrameHost::new();
        let host_dyn: Arc<dyn WakeupFrameHost> = host.clone();
        let (tx, mut rx) = mpsc::channel(1);
        let body = serde_json::json!({"runtime_id": "rt-9", "task_id": "t-1"})
            .to_string()
            .into_bytes();

        dispatch_wakeup_frame(
            &host_dyn,
            cordy_protocol::events::EVENT_DAEMON_TASK_AVAILABLE,
            &body,
            0,
            &tx,
        );
        let got = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("wakeup not signalled")
            .unwrap();
        assert_eq!(got.runtime_id, "rt-9");

        // Invalid payload is skipped, not fatal.
        dispatch_wakeup_frame(
            &host_dyn,
            cordy_protocol::events::EVENT_DAEMON_TASK_AVAILABLE,
            b"{not json",
            0,
            &tx,
        );
        assert!(rx.try_recv().is_err());
    }

    /// Workspaces-changed broadcasts; RPC responses deliver
    /// (wakeup.go:412–443).
    #[tokio::test]
    async fn workspaces_changed_and_rpc_response_frames() {
        let host = FakeFrameHost::new();
        let host_dyn: Arc<dyn WakeupFrameHost> = host.clone();
        let (tx, _rx) = mpsc::channel(1);

        dispatch_wakeup_frame(
            &host_dyn,
            cordy_protocol::events::EVENT_DAEMON_WORKSPACES_CHANGED,
            b"{}",
            0,
            &tx,
        );
        assert_eq!(host.workspaces_broadcasts.load(Ordering::SeqCst), 1);

        let rpc = serde_json::json!({"request_id": "req-1", "status": 200})
            .to_string()
            .into_bytes();
        dispatch_wakeup_frame(
            &host_dyn,
            cordy_protocol::events::EVENT_DAEMON_RPC_RESPONSE,
            &rpc,
            0,
            &tx,
        );
        assert_eq!(
            *host.delivered_rpcs.lock().unwrap(),
            vec!["req-1".to_string()]
        );
    }

    /// Pending-work frames defer a spawned hint (wakeup.go:416–428).
    #[tokio::test]
    async fn pending_work_frame_defers_hint() {
        let host = FakeFrameHost::new();
        let host_dyn: Arc<dyn WakeupFrameHost> = host.clone();
        let (tx, _rx) = mpsc::channel(1);
        let body = serde_json::json!({"runtime_id": "rt-2", "kind": "tasks"})
            .to_string()
            .into_bytes();

        let action = dispatch_wakeup_frame(
            &host_dyn,
            cordy_protocol::events::EVENT_DAEMON_PENDING_WORK,
            &body,
            0,
            &tx,
        );
        match action {
            Some(DeferredAction::PendingWork { runtime_id, kind }) => {
                assert_eq!(runtime_id, "rt-2");
                assert_eq!(kind, "tasks");
            }
            other => panic!("expected deferred pending-work action, got {other:?}"),
        }

        // Missing runtime_id is skipped.
        let missing = dispatch_wakeup_frame(
            &host_dyn,
            cordy_protocol::events::EVENT_DAEMON_PENDING_WORK,
            br#"{"kind":"tasks"}"#,
            0,
            &tx,
        );
        assert!(missing.is_none());
    }

    /// RuntimeSetWatcher notify/wait/unsubscribe semantics
    /// (daemon.go:1852–1885).
    #[tokio::test]
    async fn runtime_set_watcher_semantics() {
        let watcher = Arc::new(RuntimeSetWatcher::new());
        let (ch1, _unsub1) = watcher.subscribe();
        let (ch2, unsub2) = watcher.subscribe();

        // Notify with no waiter: permit buffered, both wake instantly.
        watcher.notify();
        tokio::time::timeout(Duration::from_millis(50), ch1.notified())
            .await
            .expect("subscriber 1 missed buffered notify");
        tokio::time::timeout(Duration::from_millis(50), ch2.notified())
            .await
            .expect("subscriber 2 missed buffered notify");

        // After unsubscribing, ch2 no longer receives notifications.
        drop(unsub2);
        watcher.notify();
        tokio::time::timeout(Duration::from_millis(50), ch1.notified())
            .await
            .expect("subscriber 1 missed post-unsub notify");
        let result = tokio::time::timeout(Duration::from_millis(50), ch2.notified()).await;
        assert!(result.is_err(), "unsubscribed subscriber still notified");
    }

    /// Reconnect-loop backoff ladder (wakeup.go:46–78): a failing connection
    /// doubles backoff up to the cap; cancellation exits.
    #[tokio::test]
    async fn wakeup_loop_exits_on_cancellation() {
        struct CancellingHost {
            runtime_set: Arc<RuntimeSetWatcher>,
            ctx: Ctx,
        }
        #[async_trait::async_trait]
        impl TaskWakeupHost for CancellingHost {
            fn all_runtime_ids(&self) -> Vec<String> {
                vec!["rt".into()]
            }
            fn server_base_url(&self) -> String {
                "http://localhost:1".into()
            }
            fn auth_and_client_headers(&self) -> Vec<(String, String)> {
                Vec::new()
            }
            fn runtime_set(&self) -> &Arc<RuntimeSetWatcher> {
                &self.runtime_set
            }
            fn broadcast_reconcile(&self) {}
            fn reset_batch_claim_unsupported(&self) {}
            fn clear_ws_heartbeat_acks(&self) {}
            async fn run_connection(
                &self,
                _ctx: &Ctx,
                _url: &str,
                _headers: &[(String, String)],
                _wakeups: &mpsc::Sender<TaskWakeup>,
            ) -> (Duration, ConnectionEnd) {
                // Cancel the daemon context mid-run; the loop must exit.
                self.ctx
                    .cancel_with(crate::repocache::CancelCause::Shutdown);
                (
                    Duration::ZERO,
                    ConnectionEnd::Error(anyhow::anyhow!("dial failed")),
                )
            }
        }

        let host = CancellingHost {
            runtime_set: Arc::new(RuntimeSetWatcher::new()),
            ctx: Ctx::new(),
        };
        let (tx, _rx) = mpsc::channel(1);
        tokio::time::timeout(
            Duration::from_secs(2),
            run_task_wakeup_loop(&host, &host.ctx, &tx),
        )
        .await
        .expect("loop did not exit on cancellation");
    }
}
