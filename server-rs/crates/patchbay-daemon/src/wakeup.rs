//! Daemon task-wakeup WebSocket lifecycle.
//!
//! The daemon-side task-wakeup WebSocket: connection lifecycle policy
//! (backoff/jitter/runtime-set reset), the per-runtime heartbeat sender, and
//! the pure helpers (`taskWakeupURL`, `jitterDuration`,
//! `signalTaskWakeup`).
//!
//! Symbol map (Go → Rust):
//! - `taskWakeupMaxBackoff` / `taskWakeupReadLimit` / pong/write waits /
//!   backoff-reset → [`TASK_WAKEUP_*`] constants
//! - `jitterDuration` / `signalTaskWakeup` → same-named fns
//! - `runWSHeartbeatSender` / `sendWSHeartbeats` →
//!   [`run_ws_heartbeat_sender`] over a [`FrameSink`] trait
//! - `marshalRaw` → inlined (`serde_json::to_value` + Null on failure)
//! - `handleWSHeartbeatAckForConnection`'s rpc-v1 capability scan →
//!   [`ack_advertises_rpc_v1`] (pure half)
//!
//! The socket owner and dispatch methods live in [`crate::manager`];
//! [`crate::control_lifecycle`] connects their events to daemon-core state.

use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;

use patchbay_protocol::{
    DaemonHeartbeatRequestPayload, Message, DAEMON_CAPABILITY_RPC_V1, EVENT_DAEMON_HEARTBEAT,
};

use crate::client::HeartbeatResponse;
use crate::repocache::Ctx;

/// `taskWakeupMaxBackoff` (wakeup.go:23).
pub(crate) const TASK_WAKEUP_MAX_BACKOFF: Duration = Duration::from_secs(30);

/// `taskWakeupReadLimit` (wakeup.go:33): one claim response can carry up to 32
/// complete Task payloads; the old 64 KiB ceiling was smaller than a valid
/// single-task response. 64 MiB keeps reads bounded with headroom for the
/// batch contract.
pub(crate) const TASK_WAKEUP_READ_LIMIT: u64 = 64 << 20;

/// `taskWakeupPongWait` (wakeup.go:37).
pub(crate) const TASK_WAKEUP_PONG_WAIT: Duration = Duration::from_secs(60);
/// `taskWakeupWriteWait` (wakeup.go:38).
pub(crate) const TASK_WAKEUP_WRITE_WAIT: Duration = Duration::from_secs(10);
/// `taskWakeupBackoffResetAfter` (wakeup.go:39).
pub(crate) const TASK_WAKEUP_BACKOFF_RESET_AFTER: Duration = Duration::from_secs(10);
/// Handshake timeout (wakeup.go:131).
pub(crate) const TASK_WAKEUP_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Default heartbeat interval when cfg leaves it unset (wakeup.go:288).
pub(crate) const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

/// `taskWakeup` event routed to idle claim pollers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskWakeup {
    pub runtime_id: String,
}

/// `jitterDuration` (wakeup.go:87): full-width ±d/5 uniform jitter.
pub(crate) fn jitter_duration(d: Duration) -> Duration {
    if d.is_zero() {
        return d;
    }
    let spread = d / 5;
    if spread.is_zero() {
        return d;
    }
    use rand::Rng;
    let span = spread.as_millis() as i64 * 2 + 1;
    let delta = rand::thread_rng().gen_range(0..span) - spread.as_millis() as i64;
    let out = d.as_millis() as i64 + delta;
    if out <= 0 {
        Duration::ZERO
    } else {
        Duration::from_millis(out as u64)
    }
}

/// `signalTaskWakeup` (wakeup.go:479): non-blocking send; a busy poller means
/// it is already claiming.
pub(crate) fn signal_task_wakeup(tx: &mpsc::Sender<TaskWakeup>, runtime_id: &str) {
    let _ = tx.try_send(TaskWakeup {
        runtime_id: runtime_id.to_string(),
    });
}

/// Sink for outbound WS frames — Go's `writes chan<- *wsOutbound`. The writer
/// pump calls [`WsOutboundHandle::begin_write`] before flushing each frame so
/// an RPC caller that gave up can still cancel delivery (see wsrpc.rs).
pub(crate) type FrameTx = mpsc::Sender<OutboundFrame>;

/// A queued frame plus its cancellation handle (Go's `*wsOutbound` with data).
#[derive(Debug)]
pub(crate) struct OutboundFrame {
    pub payload: OutboundPayload,
    pub outbound: std::sync::Arc<crate::wsrpc::WsOutbound>,
}

#[derive(Debug)]
pub(crate) enum OutboundPayload {
    Text(Vec<u8>),
    Pong(Vec<u8>),
}

impl OutboundFrame {
    pub(crate) fn new(data: Vec<u8>) -> Self {
        Self {
            payload: OutboundPayload::Text(data),
            outbound: std::sync::Arc::new(crate::wsrpc::WsOutbound::default()),
        }
    }

    pub(crate) fn pong(data: Vec<u8>) -> Self {
        Self {
            payload: OutboundPayload::Pong(data),
            outbound: std::sync::Arc::new(crate::wsrpc::WsOutbound::default()),
        }
    }
}

/// `sendWSHeartbeats` (wakeup.go:302): queue one daemon:heartbeat frame per
/// runtime; drop beats when the writer is backed up (HTTP heartbeat resumes on
/// its next tick once the freshness window expires). Ctx cancellation stops
/// mid-batch like Go's ctx.Err() check.
///
/// Returns false when ctx cancelled during the batch.
pub(crate) async fn send_ws_heartbeats(
    ctx: &Ctx,
    runtime_ids: &[String],
    writes: &FrameTx,
) -> bool {
    for rid in runtime_ids {
        if ctx.err().is_some() {
            return false;
        }
        // marshalRaw equivalent: payload Value; serialization of these plain
        // structs cannot fail, mirroring Go's nil-RawMessage-on-error skip.
        let payload = serde_json::to_value(&DaemonHeartbeatRequestPayload {
            runtime_id: rid.clone(),
            supports_batch_import: true,
        })
        .unwrap_or(Value::Null);
        let frame = match serde_json::to_vec(&Message {
            r#type: EVENT_DAEMON_HEARTBEAT.to_string(),
            payload,
        }) {
            Ok(frame) => frame,
            Err(_) => continue,
        };
        let item = OutboundFrame::new(frame);
        match writes.try_send(item) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::debug!(runtime_id = %rid, "ws heartbeat dropped: writer backlog");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => return false,
        }
    }
    true
}

/// `runWSHeartbeatSender` (wakeup.go:284): emits a daemon:heartbeat per
/// runtime every interval; the first batch fires immediately so the server
/// learns the connection identity without waiting a full interval. Returns
/// when ctx cancels or the write channel closes.
pub(crate) async fn run_ws_heartbeat_sender(
    ctx: &Ctx,
    runtime_ids: &[String],
    writes: &FrameTx,
    mut interval: Duration,
) {
    if !send_ws_heartbeats(ctx, runtime_ids, writes).await {
        return;
    }
    if interval.is_zero() {
        interval = DEFAULT_HEARTBEAT_INTERVAL;
    }
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = ctx.cancelled() => return,
            _ = ticker.tick() => {
                if !send_ws_heartbeats(ctx, runtime_ids, writes).await {
                    return;
                }
            }
        }
    }
}

/// Reports whether a heartbeat acknowledgement advertises RPC v1.
pub(crate) fn ack_advertises_rpc_v1(ack: &HeartbeatResponse) -> bool {
    ack.server_capabilities
        .iter()
        .any(|capability| capability == DAEMON_CAPABILITY_RPC_V1)
}

/// `taskWakeupURL` (wakeup.go:486): builds ws(s)://host/api/daemon/ws with the
/// sorted runtime ids as a comma-joined query parameter. Query encoding uses
/// form-urlencoded semantics exactly like Go's u.Query().Encode().
pub(crate) fn task_wakeup_url(base_url: &str, runtime_ids: &[String]) -> anyhow::Result<String> {
    let trimmed = base_url.trim();
    let mut url = url::Url::parse(trimmed)
        .map_err(|err| anyhow::anyhow!("invalid daemon server URL: {err}"))?;
    match url.scheme() {
        "http" => url
            .set_scheme("ws")
            .map_err(|_| anyhow::anyhow!("set scheme"))?,
        "https" => url
            .set_scheme("wss")
            .map_err(|_| anyhow::anyhow!("set scheme"))?,
        "ws" | "wss" => {}
        _ => anyhow::bail!("daemon server URL must use http, https, ws, or wss"),
    }
    let mut path = url.path().trim_end_matches('/').to_string();
    // Go: u.Path = TrimRight(u.Path, "/") + "/api/daemon/ws" — an existing
    // /ws path (the default server URL shape) collapses to the endpoint.
    if path == "/ws" {
        path.clear();
    }
    path.push_str("/api/daemon/ws");
    url.set_path(&path);

    let mut ids: Vec<&String> = runtime_ids.iter().collect();
    ids.sort();
    if ids.is_empty() {
        url.set_query(None);
    } else {
        let mut query = url.query_pairs_mut();
        query.clear();
        query.append_pair(
            "runtime_ids",
            &ids.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(","),
        );
    }
    url.set_fragment(None);
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_stays_within_fifth() {
        for _ in 0..200 {
            let j = jitter_duration(Duration::from_secs(30));
            let base = 30_000i64;
            assert!((j.as_millis() as i64 - base).abs() <= base / 5 + 1);
        }
        assert_eq!(jitter_duration(Duration::ZERO), Duration::ZERO);
        // Tiny durations with zero spread pass through unchanged.
        assert_eq!(
            jitter_duration(Duration::from_nanos(1)),
            Duration::from_nanos(1)
        );
    }

    #[test]
    fn task_wakeup_url_builds_sorted_query() {
        let url = task_wakeup_url(
            "http://server.example/ws/",
            &["rb".to_string(), "ra".to_string()],
        )
        .unwrap();
        assert_eq!(url, "ws://server.example/api/daemon/ws?runtime_ids=ra%2Crb");
    }

    #[test]
    fn task_wakeup_url_https_upgrades() {
        let url = task_wakeup_url("https://api.patchbay.ai", &[]).unwrap();
        assert_eq!(url, "wss://api.patchbay.ai/api/daemon/ws");
    }

    #[test]
    fn task_wakeup_url_rejects_bad_scheme() {
        assert!(task_wakeup_url("ftp://x", &[]).is_err());
    }

    #[test]
    fn ack_capability_scan() {
        let mut ack: HeartbeatResponse =
            serde_json::from_str(r#"{"runtime_id":"r1","status":"ok"}"#).expect("ack wire shape");
        assert!(!ack_advertises_rpc_v1(&ack));
        assert!(!ack_advertises_rpc_v1(&ack));
        ack.server_capabilities = vec![DAEMON_CAPABILITY_RPC_V1.to_string()];
        assert!(ack_advertises_rpc_v1(&ack));
    }
}
