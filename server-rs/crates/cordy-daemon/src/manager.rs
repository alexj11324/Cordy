//! Daemon control plane: authenticated WebSocket lifecycle, heartbeat/RPC
//! multiplexing, reconnect recovery, and WS-first task claiming.
//!
//! This is the transport/control slice of `daemon.go` + `wakeup.go`. Agent
//! execution deliberately stays outside this module: parsed control events are
//! handed to the owner through a bounded channel, while task claims return the
//! real [`Task`] payloads for the execution layer to consume.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{self, Message as WsMessage};

use cordy_protocol::{
    DaemonHeartbeatAckPayload, Message, PendingWorkPayload, RpcResponsePayload,
    RuntimeProfilesChangedPayload, TaskAvailablePayload, EVENT_DAEMON_HEARTBEAT_ACK,
    EVENT_DAEMON_PENDING_WORK, EVENT_DAEMON_RPC_RESPONSE, EVENT_DAEMON_RUNTIME_PROFILES_CHANGED,
    EVENT_DAEMON_TASK_AVAILABLE, EVENT_DAEMON_WORKSPACES_CHANGED,
};

use crate::client::{Client, RequestError, BATCH_CLAIM_REQUEST_TIMEOUT};
use crate::repocache::{CancelCause, Ctx};
use crate::types::Task;
use crate::wakeup::{
    ack_advertises_rpc_v1, jitter_duration, run_ws_heartbeat_sender, task_wakeup_url,
    OutboundFrame, OutboundPayload, TASK_WAKEUP_BACKOFF_RESET_AFTER, TASK_WAKEUP_HANDSHAKE_TIMEOUT,
    TASK_WAKEUP_MAX_BACKOFF, TASK_WAKEUP_PONG_WAIT, TASK_WAKEUP_READ_LIMIT, TASK_WAKEUP_WRITE_WAIT,
};
use crate::wsrpc::{
    is_uncertain, SendFrame, WriteBufferFull, WsRpcClient, WsRpcError, WS_RPC_RESPONSE_GRACE,
};

/// Parsed control-plane events. All variants are hints or lifecycle signals;
/// the server remains authoritative and consumers re-read durable state.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ControlEvent {
    Connected { runtime_ids: Vec<String> },
    TaskAvailable(TaskAvailablePayload),
    HeartbeatAck(DaemonHeartbeatAckPayload),
    RuntimeGone { runtime_id: String },
    RuntimeProfilesChanged(RuntimeProfilesChangedPayload),
    WorkspacesChanged,
}

#[derive(Default)]
struct ClaimState {
    batch_unsupported: bool,
    http_fallback_after: Option<Instant>,
}

#[derive(Default)]
struct PendingWorkState {
    inflight: HashSet<String>,
    last_run: HashMap<String, Instant>,
}

const PENDING_WORK_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(15);
const PENDING_WORK_HINT_MIN_INTERVAL: Duration = Duration::from_secs(1);
const PENDING_WORK_HINT_BOOKKEEPING_TTL: Duration = Duration::from_secs(10 * 60);

/// A complete daemon-side control connection manager.
pub(crate) struct DaemonControl {
    client: Arc<Client>,
    server_base_url: String,
    daemon_id: String,
    heartbeat_interval: Duration,
    runtimes_tx: watch::Sender<Vec<String>>,
    events: mpsc::UnboundedSender<ControlEvent>,
    ws_rpc: Arc<WsRpcClient>,
    claim: Mutex<ClaimState>,
    ws_heartbeat_acks: Mutex<HashMap<String, Instant>>,
    pending_work: Mutex<PendingWorkState>,
}

impl DaemonControl {
    pub(crate) fn new(
        client: Arc<Client>,
        server_base_url: impl Into<String>,
        daemon_id: impl Into<String>,
        heartbeat_interval: Duration,
        events: mpsc::UnboundedSender<ControlEvent>,
    ) -> Arc<Self> {
        let (runtimes_tx, _) = watch::channel(Vec::new());
        Arc::new(Self {
            client,
            server_base_url: server_base_url.into(),
            daemon_id: daemon_id.into(),
            heartbeat_interval,
            runtimes_tx,
            events,
            ws_rpc: Arc::new(WsRpcClient::new(WS_RPC_RESPONSE_GRACE)),
            claim: Mutex::new(ClaimState::default()),
            ws_heartbeat_acks: Mutex::new(HashMap::new()),
            pending_work: Mutex::new(PendingWorkState::default()),
        })
    }

    /// Replaces the authenticated runtime set. Sorting and deduplication make
    /// identity stable and prevent reconnects for equivalent updates.
    pub(crate) fn set_runtime_ids(&self, runtime_ids: impl IntoIterator<Item = String>) {
        let mut ids: Vec<String> = runtime_ids
            .into_iter()
            .filter(|id| !id.is_empty())
            .collect();
        ids.sort();
        ids.dedup();
        if *self.runtimes_tx.borrow() != ids {
            self.runtimes_tx.send_replace(ids);
        }
    }

    /// Snapshot/subscribe access for the machine-level claim poller. The same
    /// canonical runtime set drives both WebSocket authentication and task
    /// routing, preventing a reconnect/claim split-brain.
    pub(crate) fn runtime_ids(&self) -> Vec<String> {
        self.runtimes_tx.borrow().clone()
    }

    pub(crate) fn subscribe_runtime_ids(&self) -> watch::Receiver<Vec<String>> {
        self.runtimes_tx.subscribe()
    }

    pub(crate) async fn run(self: Arc<Self>, ctx: Ctx) {
        let ws = tokio::spawn(Arc::clone(&self).task_wakeup_loop(ctx.child()));
        let http = tokio::spawn(Arc::clone(&self).heartbeat_supervisor(ctx.child()));
        ctx.cancelled().await;
        let _ = tokio::join!(ws, http);
    }

    /// Go `ClaimTasksWSFirst`: use negotiated WS RPC, suppress unsafe fallback
    /// after an uncertain sent-frame outcome, then use HTTP batch/legacy.
    pub(crate) async fn claim_tasks(
        &self,
        ctx: &Ctx,
        runtime_ids: &[String],
        max_tasks: usize,
    ) -> anyhow::Result<Vec<Task>> {
        if max_tasks == 0 {
            return Ok(Vec::new());
        }
        let (legacy, bypass_ws) = {
            let mut state = self.claim.lock().unwrap();
            if state.batch_unsupported {
                (true, false)
            } else if let Some(after) = state.http_fallback_after {
                if Instant::now() < after {
                    return Ok(Vec::new());
                }
                state.http_fallback_after = None;
                (false, true)
            } else {
                (false, false)
            }
        };
        if legacy {
            return self
                .client
                .claim_tasks_legacy(ctx, runtime_ids, max_tasks)
                .await;
        }

        if !bypass_ws && self.ws_rpc.supports_rpc_v1() {
            #[derive(Deserialize)]
            struct Response {
                #[serde(default)]
                tasks: Vec<Task>,
            }
            let body = json!({
                "daemon_id": self.daemon_id,
                "runtime_ids": runtime_ids,
                "max_tasks": max_tasks,
            });
            match self
                .ws_rpc
                .call_if_rpc_v1_supported::<_, Response>(
                    ctx,
                    "tasks.claim",
                    BATCH_CLAIM_REQUEST_TIMEOUT,
                    Some(&body),
                )
                .await
            {
                Ok((_status, response)) => {
                    return Ok(response.map_or_else(Vec::new, |response| response.tasks));
                }
                Err(err) if is_uncertain(&err) => {
                    self.claim.lock().unwrap().http_fallback_after =
                        Some(Instant::now() + BATCH_CLAIM_REQUEST_TIMEOUT + WS_RPC_RESPONSE_GRACE);
                    return Ok(Vec::new());
                }
                Err(err) => tracing::debug!(error = %err, "ws claim failed; falling back to http"),
            }
        }

        match self
            .client
            .claim_tasks(ctx, &self.daemon_id, runtime_ids, max_tasks as i32)
            .await
        {
            Ok(tasks) => Ok(tasks),
            Err(err) if request_status(&err) == Some(404) => {
                self.claim.lock().unwrap().batch_unsupported = true;
                self.client
                    .claim_tasks_legacy(ctx, runtime_ids, max_tasks)
                    .await
            }
            Err(err) => Err(err),
        }
    }

    async fn task_wakeup_loop(self: Arc<Self>, ctx: Ctx) {
        let mut runtime_rx = self.runtimes_tx.subscribe();
        let mut backoff = Duration::from_secs(1);
        loop {
            let runtime_ids = runtime_rx.borrow().clone();
            let started = Instant::now();
            let result = self
                .run_task_wakeup_connection(&ctx, &runtime_ids, &mut runtime_rx)
                .await;
            if ctx.err().is_some() {
                return;
            }
            if matches!(result, Err(ConnectionEnd::RuntimeSetChanged)) {
                backoff = Duration::from_secs(1);
                continue;
            }
            if started.elapsed() >= TASK_WAKEUP_BACKOFF_RESET_AFTER {
                backoff = Duration::from_secs(1);
            }
            if let Err(err) = result {
                tracing::debug!(error = %err, retry_in = ?backoff,
                    "task wakeup websocket unavailable; polling fallback remains active");
            }
            let delay = jitter_duration(backoff);
            tokio::select! {
                () = ctx.cancelled() => return,
                changed = runtime_rx.changed() => {
                    if changed.is_err() { return; }
                    backoff = Duration::from_secs(1);
                    continue;
                }
                _ = tokio::time::sleep(delay) => {}
            }
            backoff = (backoff * 2).min(TASK_WAKEUP_MAX_BACKOFF);
        }
    }

    async fn run_task_wakeup_connection(
        self: &Arc<Self>,
        ctx: &Ctx,
        runtime_ids: &[String],
        runtime_rx: &mut watch::Receiver<Vec<String>>,
    ) -> Result<(), ConnectionEnd> {
        let url = task_wakeup_url(&self.server_base_url, runtime_ids)
            .map_err(ConnectionEnd::Transport)?;
        let request = self
            .client
            .websocket_request(&url)
            .map_err(ConnectionEnd::Transport)?;
        let mut config = tungstenite::protocol::WebSocketConfig::default();
        config.max_message_size = Some(TASK_WAKEUP_READ_LIMIT as usize);
        config.max_frame_size = Some(TASK_WAKEUP_READ_LIMIT as usize);
        let connect = tokio_tungstenite::connect_async_with_config(request, Some(config), false);
        let (socket, _) = tokio::time::timeout(TASK_WAKEUP_HANDSHAKE_TIMEOUT, connect)
            .await
            .map_err(|_| ConnectionEnd::TimedOut("websocket handshake"))?
            .map_err(|err| ConnectionEnd::Transport(err.into()))?;
        let (sink, mut stream) = socket.split();
        let connection_ctx = ctx.child();
        let capacity = 16usize.max(runtime_ids.len().saturating_mul(2));
        let (writes_tx, writes_rx) = mpsc::channel::<OutboundFrame>(capacity);

        let writer_ctx = connection_ctx.child();
        let mut writer = tokio::spawn(run_ws_writer(writer_ctx, sink, writes_rx));
        let send_tx = writes_tx.clone();
        let send_frame: SendFrame = Arc::new(move |frame| {
            let item = OutboundFrame::new(frame);
            let outbound = Arc::clone(&item.outbound);
            send_tx.try_send(item).map_err(|err| match err {
                mpsc::error::TrySendError::Full(_) => WsRpcError::WriteBufferFull(WriteBufferFull),
                mpsc::error::TrySendError::Closed(_) => {
                    WsRpcError::Unavailable(crate::wsrpc::Unavailable)
                }
            })?;
            Ok(outbound)
        });
        let generation = self.ws_rpc.attach(Some(send_frame));
        self.claim.lock().unwrap().batch_unsupported = false;
        self.emit(ControlEvent::Connected {
            runtime_ids: runtime_ids.to_vec(),
        });

        let heartbeat_ctx = connection_ctx.child();
        let heartbeat_ids = runtime_ids.to_vec();
        let heartbeat_writes = writes_tx.clone();
        let heartbeat_interval = self.effective_heartbeat_interval();
        let heartbeat = tokio::spawn(async move {
            run_ws_heartbeat_sender(
                &heartbeat_ctx,
                &heartbeat_ids,
                &heartbeat_writes,
                heartbeat_interval,
            )
            .await;
        });

        let end = tokio::select! {
            () = ctx.cancelled() => ConnectionEnd::Cancelled,
            changed = runtime_rx.changed() => {
                if changed.is_err() { ConnectionEnd::Cancelled } else { ConnectionEnd::RuntimeSetChanged }
            }
            result = self.read_messages(ctx, &mut stream, &writes_tx, generation) => result,
            result = &mut writer => {
                match result {
                    Ok(Ok(())) => ConnectionEnd::Closed,
                    Ok(Err(err)) => ConnectionEnd::Transport(err),
                    Err(err) => ConnectionEnd::Transport(err.into()),
                }
            }
        };

        // Close the socket before detaching RPC. That makes an unsent queued
        // claim definitively unavailable and prevents an HTTP fallback racing a
        // late WS write of the same claim.
        connection_ctx.cancel_with(CancelCause::Cancelled);
        drop(writes_tx);
        let _ = heartbeat.await;
        if !writer.is_finished() {
            let _ = writer.await;
        }
        self.ws_rpc.attach(None);
        self.ws_heartbeat_acks.lock().unwrap().clear();
        match end {
            ConnectionEnd::Cancelled if ctx.err().is_some() => Ok(()),
            other => Err(other),
        }
    }

    async fn read_messages<S>(
        self: &Arc<Self>,
        ctx: &Ctx,
        stream: &mut S,
        writes: &mpsc::Sender<OutboundFrame>,
        generation: u64,
    ) -> ConnectionEnd
    where
        S: Stream<Item = Result<WsMessage, tungstenite::Error>> + Unpin,
    {
        loop {
            let next = tokio::select! {
                () = ctx.cancelled() => return ConnectionEnd::Cancelled,
                result = tokio::time::timeout(TASK_WAKEUP_PONG_WAIT, stream.next()) => result,
            };
            let message = match next {
                Err(_) => return ConnectionEnd::TimedOut("websocket read"),
                Ok(Some(Ok(message))) => message,
                Ok(Some(Err(err))) => return ConnectionEnd::Transport(err.into()),
                Ok(None) => return ConnectionEnd::Closed,
            };
            match message {
                WsMessage::Text(text) => {
                    self.dispatch_message(ctx, text.as_bytes(), generation);
                }
                WsMessage::Binary(data) => self.dispatch_message(ctx, &data, generation),
                WsMessage::Ping(data) => {
                    if writes.try_send(OutboundFrame::pong(data.to_vec())).is_err() {
                        return ConnectionEnd::Closed;
                    }
                }
                WsMessage::Pong(_) | WsMessage::Frame(_) => {}
                WsMessage::Close(_) => return ConnectionEnd::Closed,
            }
        }
    }

    fn dispatch_message(self: &Arc<Self>, ctx: &Ctx, raw: &[u8], generation: u64) {
        let Ok(message) = serde_json::from_slice::<Message>(raw) else {
            tracing::debug!("task wakeup websocket invalid message");
            return;
        };
        match message.r#type.as_str() {
            EVENT_DAEMON_TASK_AVAILABLE => {
                if let Ok(payload) = serde_json::from_value(message.payload) {
                    self.emit(ControlEvent::TaskAvailable(payload));
                }
            }
            EVENT_DAEMON_RUNTIME_PROFILES_CHANGED => {
                if let Ok(payload) =
                    serde_json::from_value::<RuntimeProfilesChangedPayload>(message.payload)
                {
                    if !payload.workspace_id.is_empty() {
                        self.emit(ControlEvent::RuntimeProfilesChanged(payload));
                    }
                }
            }
            EVENT_DAEMON_WORKSPACES_CHANGED => self.emit(ControlEvent::WorkspacesChanged),
            EVENT_DAEMON_PENDING_WORK => {
                if let Ok(payload) = serde_json::from_value::<PendingWorkPayload>(message.payload) {
                    if !payload.runtime_id.is_empty()
                        && self.try_begin_pending_work(&payload.runtime_id)
                    {
                        let control = Arc::clone(self);
                        let root_ctx = ctx.child();
                        tokio::spawn(async move {
                            control.serve_pending_work(root_ctx, payload).await;
                        });
                    }
                }
            }
            EVENT_DAEMON_HEARTBEAT_ACK => {
                if let Ok(ack) = serde_json::from_value(message.payload) {
                    self.handle_heartbeat_ack(ack, generation);
                }
            }
            EVENT_DAEMON_RPC_RESPONSE => {
                if let Ok(response) = serde_json::from_value::<RpcResponsePayload>(message.payload)
                {
                    self.ws_rpc.deliver(response);
                }
            }
            _ => {}
        }
    }

    fn handle_heartbeat_ack(&self, ack: DaemonHeartbeatAckPayload, generation: u64) {
        if ack.runtime_id.is_empty() {
            return;
        }
        if ack.runtime_gone {
            self.emit(ControlEvent::RuntimeGone {
                runtime_id: ack.runtime_id,
            });
            return;
        }
        if ack_advertises_rpc_v1(&ack) {
            self.ws_rpc.mark_rpc_v1_supported(generation);
        }
        self.ws_heartbeat_acks
            .lock()
            .unwrap()
            .insert(ack.runtime_id.clone(), Instant::now());
        self.emit(ControlEvent::HeartbeatAck(ack));
    }

    fn try_begin_pending_work(&self, runtime_id: &str) -> bool {
        if !self.runtimes_tx.borrow().iter().any(|id| id == runtime_id) {
            return false;
        }
        let now = Instant::now();
        let mut state = self.pending_work.lock().unwrap();
        state
            .last_run
            .retain(|_, at| now.duration_since(*at) <= PENDING_WORK_HINT_BOOKKEEPING_TTL);
        if state.inflight.contains(runtime_id)
            || state
                .last_run
                .get(runtime_id)
                .is_some_and(|at| now.duration_since(*at) < PENDING_WORK_HINT_MIN_INTERVAL)
        {
            return false;
        }
        state.inflight.insert(runtime_id.to_string());
        state.last_run.insert(runtime_id.to_string(), now);
        true
    }

    async fn serve_pending_work(self: Arc<Self>, ctx: Ctx, payload: PendingWorkPayload) {
        let runtime_id = payload.runtime_id.clone();
        // Deliberately bypass WS-heartbeat freshness: this caller-triggered
        // hint requests an immediate durable pull, not another periodic tick.
        let result = tokio::time::timeout(
            PENDING_WORK_HEARTBEAT_TIMEOUT,
            self.client.send_heartbeat(&ctx, &runtime_id),
        )
        .await;
        match result {
            Ok(Ok(ack)) if ack.runtime_gone => {
                self.emit(ControlEvent::RuntimeGone {
                    runtime_id: runtime_id.clone(),
                });
            }
            Ok(Ok(ack)) => self.emit(ControlEvent::HeartbeatAck(ack)),
            Ok(Err(err)) if request_status(&err) == Some(404) => {
                self.emit(ControlEvent::RuntimeGone {
                    runtime_id: runtime_id.clone(),
                });
            }
            Ok(Err(err)) => tracing::debug!(
                runtime_id = %runtime_id,
                kind = %payload.kind,
                error = %err,
                "pending work hint heartbeat failed"
            ),
            Err(_) => tracing::debug!(
                runtime_id = %runtime_id,
                kind = %payload.kind,
                "pending work hint heartbeat timed out"
            ),
        }
        self.pending_work
            .lock()
            .unwrap()
            .inflight
            .remove(&runtime_id);
    }

    async fn heartbeat_supervisor(self: Arc<Self>, ctx: Ctx) {
        let mut runtime_rx = self.runtimes_tx.subscribe();
        let mut tasks: HashMap<String, (Ctx, JoinHandle<()>)> = HashMap::new();
        loop {
            let wanted: HashSet<String> = runtime_rx.borrow().iter().cloned().collect();
            let removed: Vec<String> = tasks
                .keys()
                .filter(|id| !wanted.contains(*id))
                .cloned()
                .collect();
            for id in removed {
                if let Some((task_ctx, task)) = tasks.remove(&id) {
                    task_ctx.cancel_with(CancelCause::Cancelled);
                    let _ = task.await;
                }
            }
            for id in wanted {
                if tasks.contains_key(&id) {
                    continue;
                }
                let task_ctx = ctx.child();
                let control = Arc::clone(&self);
                let rid = id.clone();
                let spawned_ctx = task_ctx.clone();
                let handle = tokio::spawn(async move {
                    control.run_runtime_heartbeat(spawned_ctx, rid).await;
                });
                tasks.insert(id, (task_ctx, handle));
            }
            tokio::select! {
                () = ctx.cancelled() => break,
                changed = runtime_rx.changed() => if changed.is_err() { break; },
            }
        }
        for (_, (task_ctx, task)) in tasks {
            task_ctx.cancel_with(CancelCause::Cancelled);
            let _ = task.await;
        }
    }

    async fn run_runtime_heartbeat(&self, ctx: Ctx, runtime_id: String) {
        let interval = self.effective_heartbeat_interval();
        let initial = if interval.is_zero() {
            Duration::ZERO
        } else {
            use rand::Rng;
            Duration::from_nanos(rand::thread_rng().gen_range(0..interval.as_nanos() as u64))
        };
        tokio::select! {
            () = ctx.cancelled() => return,
            _ = tokio::time::sleep(initial) => {}
        }
        let mut failures = 0u8;
        loop {
            if !self.ws_heartbeat_recently_acked(&runtime_id) {
                match self.client.send_heartbeat(&ctx, &runtime_id).await {
                    Ok(ack) if ack.runtime_gone => {
                        self.emit(ControlEvent::RuntimeGone {
                            runtime_id: runtime_id.clone(),
                        });
                        failures = 0;
                    }
                    Ok(ack) => {
                        self.emit(ControlEvent::HeartbeatAck(ack));
                        failures = 0;
                    }
                    Err(err) if request_status(&err) == Some(404) => {
                        self.emit(ControlEvent::RuntimeGone {
                            runtime_id: runtime_id.clone(),
                        });
                        failures = 0;
                    }
                    Err(err) => {
                        failures = failures.saturating_add(1);
                        tracing::warn!(runtime_id = %runtime_id, error = %err, "heartbeat failed");
                        if failures == 2 {
                            self.client.close_idle_connections();
                        }
                    }
                }
            } else {
                failures = 0;
            }
            tokio::select! {
                () = ctx.cancelled() => return,
                _ = tokio::time::sleep(interval) => {}
            }
        }
    }

    fn effective_heartbeat_interval(&self) -> Duration {
        if self.heartbeat_interval.is_zero() {
            Duration::from_secs(15)
        } else {
            self.heartbeat_interval
        }
    }

    fn ws_heartbeat_recently_acked(&self, runtime_id: &str) -> bool {
        self.ws_heartbeat_acks
            .lock()
            .unwrap()
            .get(runtime_id)
            .is_some_and(|at| at.elapsed() < self.effective_heartbeat_interval() * 2)
    }

    fn emit(&self, event: ControlEvent) {
        if self.events.send(event).is_err() {
            tracing::debug!("daemon control event consumer stopped");
        }
    }
}

async fn run_ws_writer<S>(
    ctx: Ctx,
    mut sink: S,
    mut writes: mpsc::Receiver<OutboundFrame>,
) -> anyhow::Result<()>
where
    S: Sink<WsMessage, Error = tungstenite::Error> + Unpin,
{
    loop {
        let item = tokio::select! {
            biased;
            () = ctx.cancelled() => {
                let _ = tokio::time::timeout(TASK_WAKEUP_WRITE_WAIT, sink.close()).await;
                return Ok(());
            }
            item = writes.recv() => match item {
                Some(item) => item,
                None => {
                    let _ = tokio::time::timeout(TASK_WAKEUP_WRITE_WAIT, sink.close()).await;
                    return Ok(());
                }
            }
        };
        if !item.outbound.begin_write() {
            continue;
        }
        let message = match item.payload {
            OutboundPayload::Text(data) => WsMessage::Text(String::from_utf8(data)?.into()),
            OutboundPayload::Pong(data) => WsMessage::Pong(data.into()),
        };
        tokio::time::timeout(TASK_WAKEUP_WRITE_WAIT, sink.send(message))
            .await
            .map_err(|_| anyhow::anyhow!("websocket write timed out"))??;
    }
}

fn request_status(err: &anyhow::Error) -> Option<u16> {
    err.downcast_ref::<RequestError>()
        .map(|err| err.status_code)
}

#[derive(Debug, thiserror::Error)]
enum ConnectionEnd {
    #[error("context cancelled")]
    Cancelled,
    #[error("runtime set changed")]
    RuntimeSetChanged,
    #[error("websocket closed")]
    Closed,
    #[error("{0} timed out")]
    TimedOut(&'static str),
    #[error(transparent)]
    Transport(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_hdr_async;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    async fn next_event(rx: &mut mpsc::UnboundedReceiver<ControlEvent>) -> ControlEvent {
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("event timeout")
            .expect("event channel closed")
    }

    #[tokio::test]
    #[allow(clippy::result_large_err)] // tungstenite's required handshake callback error type
    async fn real_websocket_negotiates_heartbeat_and_claim_rpc() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(stream, |request: &Request, response: Response| {
                assert_eq!(request.headers()["authorization"], "Bearer token");
                assert!(request.headers()["x-client-capabilities"]
                    .to_str()
                    .unwrap()
                    .contains("rpc-v1"));
                Ok(response)
            })
            .await
            .unwrap();
            let heartbeat = socket.next().await.unwrap().unwrap();
            let heartbeat: Message = serde_json::from_slice(&heartbeat.into_data()).unwrap();
            assert_eq!(heartbeat.r#type, cordy_protocol::EVENT_DAEMON_HEARTBEAT);
            socket
                .send(WsMessage::Text(
                    serde_json::to_string(&Message {
                        r#type: EVENT_DAEMON_HEARTBEAT_ACK.to_string(),
                        payload: json!({
                            "runtime_id": "runtime-1",
                            "status": "ok",
                            "server_capabilities": ["rpc-v1"]
                        }),
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
            let rpc = socket.next().await.unwrap().unwrap();
            let rpc: Message = serde_json::from_slice(&rpc.into_data()).unwrap();
            let request_id = rpc.payload["request_id"].as_str().unwrap();
            assert_eq!(rpc.payload["method"], "tasks.claim");
            socket
                .send(WsMessage::Text(
                    serde_json::to_string(&Message {
                        r#type: EVENT_DAEMON_RPC_RESPONSE.to_string(),
                        payload: json!({"request_id": request_id, "status": 200, "body": {"tasks": []}}),
                    })
                    .unwrap()
                    .into(),
                ))
                .await
                .unwrap();
        });

        let client = Arc::new(Client::new(format!("http://{address}")));
        client.set_token("token");
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        let control = DaemonControl::new(
            client,
            format!("http://{address}"),
            "daemon-1",
            Duration::from_secs(30),
            events_tx,
        );
        control.set_runtime_ids(["runtime-1".to_string()]);
        let ctx = Ctx::new();
        let running = tokio::spawn(Arc::clone(&control).task_wakeup_loop(ctx.clone()));
        assert!(matches!(
            next_event(&mut events_rx).await,
            ControlEvent::Connected { .. }
        ));
        assert!(matches!(
            next_event(&mut events_rx).await,
            ControlEvent::HeartbeatAck(_)
        ));

        let tasks = control
            .claim_tasks(&ctx, &["runtime-1".to_string()], 2)
            .await
            .unwrap();
        assert!(tasks.is_empty());
        ctx.cancel_with(CancelCause::Cancelled);
        running.await.unwrap();
        server.await.unwrap();
    }

    #[test]
    fn heartbeat_freshness_is_cleared_on_disconnect_boundary() {
        let client = Arc::new(Client::new("http://127.0.0.1"));
        let (events, _) = mpsc::unbounded_channel();
        let control = DaemonControl::new(
            client,
            "http://127.0.0.1",
            "d",
            Duration::from_secs(1),
            events,
        );
        control
            .ws_heartbeat_acks
            .lock()
            .unwrap()
            .insert("r".into(), Instant::now());
        assert!(control.ws_heartbeat_recently_acked("r"));
        control.ws_heartbeat_acks.lock().unwrap().clear();
        assert!(!control.ws_heartbeat_recently_acked("r"));
    }

    #[test]
    fn runtime_set_is_sorted_deduplicated_and_stable() {
        let client = Arc::new(Client::new("http://127.0.0.1"));
        let (events, _) = mpsc::unbounded_channel();
        let control = DaemonControl::new(
            client,
            "http://127.0.0.1",
            "d",
            Duration::from_secs(1),
            events,
        );
        control.set_runtime_ids(["b".into(), "a".into(), "b".into(), String::new()]);
        assert_eq!(
            &*control.runtimes_tx.borrow(),
            &["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn pending_work_is_owned_coalesced_and_rate_limited() {
        let client = Arc::new(Client::new("http://127.0.0.1"));
        let (events, _) = mpsc::unbounded_channel();
        let control = DaemonControl::new(
            client,
            "http://127.0.0.1",
            "d",
            Duration::from_secs(1),
            events,
        );
        assert!(!control.try_begin_pending_work("not-ours"));
        control.set_runtime_ids(["ours".into()]);
        assert!(control.try_begin_pending_work("ours"));
        assert!(!control.try_begin_pending_work("ours"));
        control.pending_work.lock().unwrap().inflight.remove("ours");
        assert!(!control.try_begin_pending_work("ours"));
    }
}
