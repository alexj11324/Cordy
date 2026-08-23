//! Port of `server/internal/daemonws/hub.go` — the daemon-facing WebSocket hub.
//!
//! This is the second WebSocket face of the server (the user-realtime twin
//! lives in `cordy-realtime::hub`). The daemon hub indexes connections by
//! runtime ID, workspace ID, and user ID, and delivers best-effort wakeup
//! hints; daemons still use HTTP claim for correctness.
//!
//! Symbol map (Go → Rust):
//! - `writeWait` / `pongWait` / `pingPeriod` → [`WRITE_WAIT`] / [`PONG_WAIT`] / [`PING_PERIOD`]
//! - `ClientIdentity` (+ `AuthorizedWorkspaceIDs` / `PrimaryWorkspaceID` /
//!   `AllowsWorkspace`) → [`ClientIdentity`]
//! - `client` → [`DaemonClient`] (Go `*client` pointer keys → [`DaemonClientId`])
//! - `client.trySend` → [`DaemonClient::try_send`]
//! - `eventDedupCapacity` + `client.markSeen` → [`EVENT_DEDUP_CAPACITY`] +
//!   [`DedupCache::mark_seen`]
//! - `HeartbeatHandler` func type → [`HeartbeatHandler`] trait (async seam)
//! - `RPCHandler` func type → [`RpcHandler`] trait (async seam)
//! - `maxInFlightRPCPerClient` → [`MAX_IN_FLIGHT_RPC_PER_CLIENT`]
//! - `MessageKindRecorder` interface → [`MessageKindRecorder`] trait
//! - `Hub` / `NewHub` → [`DaemonHub`] / [`DaemonHub::new`]
//! - `SetHeartbeatHandler` / `heartbeatHandler` → [`DaemonHub::set_heartbeat_handler`]
//! - `SetRPCHandler` / `rpcHandler` → [`DaemonHub::set_rpc_handler`]
//! - `SetMessageKindRecorder` / `messageKindRecorder` → [`DaemonHub::set_message_kind_recorder`]
//! - `HandleWebSocket` → identity guard in [`DaemonHub::validate_identity`]; the
//!   HTTP upgrade itself is an axum-lane concern (S9-integration below)
//! - `NotifyTaskAvailable` / `NotifyRuntimeProfilesChanged` /
//!   `NotifyWorkspacesChanged` / `NotifyPendingWork` → same-named methods
//! - `DeliverDaemonRuntime` → [`DaemonHub::deliver_daemon_runtime`]
//! - `notifyFrame` / `notifyWorkspaceFrame` / `notifyUserFrame` → private
//!   `notify_*_frame` methods
//! - `taskAvailableFrame` / `runtimeProfilesChangedFrame` /
//!   `workspacesChangedFrame` / `pendingWorkFrame` / `mustMarshalRaw` →
//!   pub(crate) frame builders shared with `notifier.rs`
//! - `RuntimeConnectionCount` / `WorkspaceConnectionCount` /
//!   `UserConnectionCount` → same-named methods
//! - `register` / `unregister` → [`DaemonHub::register`] / [`DaemonHub::unregister`]
//! - `readPump` / `writePump` / `handleFrame` / `handleRPCFrame` /
//!   `handleHeartbeatFrame` / `sendRPCResponse` → socket pumps land with the
//!   axum handler lane; the pure bookkeeping halves are ported as
//!   [`DaemonHub::handle_frame`] / [`DaemonHub::handle_rpc_frame`] /
//!   [`DaemonHub::handle_heartbeat_frame`] / [`DaemonHub::send_rpc_response`]
//!
//! S9-integration: this module also carries a faithful stand-in for
//! `internal/daemonws/metrics.go` ([`Metrics`] + [`M`]) because both hub.go and
//! notifier.go reference the package-level `M`; reconcile if a dedicated
//! metrics lane lands.
//!
//! Port notes vs Go (mirroring `cordy-realtime/src/hub.rs`):
//! - Go serialises mutations through `sync.RWMutex` around plain maps; here a
//!   single `RwLock<HubInner>` gives the same happens-before guarantees.
//! - Go uses `*client` pointers as map keys; we use monotonically increasing
//!   [`DaemonClientId`]s.
//! - Go's `send chan []byte` becomes a bounded tokio mpsc queue consumed by the
//!   connection's write task; unregister removes the client from all indexes
//!   and cancels its [`CancellationToken`] (the Go code closes the channel —
//!   closing is deferred to the pump owner dropping its receiver half).
//! - slog → tracing with identical field names.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use cordy_protocol::messages::{
    DaemonHeartbeatAckPayload, DaemonHeartbeatRequestPayload, Message, PendingWorkPayload,
    RpcRequestPayload, RpcResponsePayload, RuntimeProfilesChangedPayload, TaskAvailablePayload,
    WorkspacesChangedPayload,
};
use cordy_protocol::{
    EVENT_DAEMON_HEARTBEAT, EVENT_DAEMON_HEARTBEAT_ACK, EVENT_DAEMON_PENDING_WORK,
    EVENT_DAEMON_RPC_REQUEST, EVENT_DAEMON_RPC_RESPONSE, EVENT_DAEMON_RUNTIME_PROFILES_CHANGED,
    EVENT_DAEMON_TASK_AVAILABLE, EVENT_DAEMON_WORKSPACES_CHANGED,
};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

// ---- timing constants (hub.go) -------------------------------------------
// Consumed by the axum WS pump lane; kept here so the wire contract stays next
// to the hub that owns the connection lifecycle.

/// Go `writeWait`: per-write deadline on the outbound socket.
pub const WRITE_WAIT: Duration = Duration::from_secs(10);
/// Go `pongWait`: read deadline refreshed by pong frames.
pub const PONG_WAIT: Duration = Duration::from_secs(60);
/// Go `pingPeriod = (pongWait * 9) / 10`.
pub const PING_PERIOD: Duration = Duration::from_millis((60 * 9 * 1000) / 10);
/// Go `c.conn.SetReadLimit(64 * 1024)` — sized for daemon:rpc_request frames
/// carrying a machine's full runtime_id set (MUL-4257).
pub const MAX_FRAME_READ_BYTES: usize = 64 * 1024;
/// Go `make(chan []byte, 16)` — per-connection outbound buffer.
const SEND_BUFFER_CAPACITY: usize = 16;
/// Go `maxInFlightRPCPerClient` — bounds concurrent RPC handlers per connection
/// so a single daemon cannot fan out unbounded goroutines / DB work over one
/// socket.
pub const MAX_IN_FLIGHT_RPC_PER_CLIENT: usize = 8;
/// Go `eventDedupCapacity` — bounded LRU window for relay event IDs.
pub const EVENT_DEDUP_CAPACITY: usize = 128;

// http.Status* values used by the RPC fallback path.
const HTTP_STATUS_TOO_MANY_REQUESTS: i32 = 429;
const HTTP_STATUS_INTERNAL_SERVER_ERROR: i32 = 500;
const HTTP_STATUS_SERVICE_UNAVAILABLE: i32 = 503;

/// Unique per-connection identifier (Go `*client` pointer equivalent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DaemonClientId(pub u64);

/// Captures the already-authenticated daemon connection scope.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClientIdentity {
    pub daemon_id: String,
    pub user_id: String,
    /// Legacy single-workspace scope used by older callers and daemon-token
    /// auth. New code should populate `workspace_ids` from the runtime rows
    /// authorized for this connection.
    pub workspace_id: String,
    pub workspace_ids: Vec<String>,
    pub runtime_ids: Vec<String>,
    pub client_version: String,
    /// Raw X-Client-Capabilities header captured at connect, so RPC handlers
    /// can honor the same capability gating as the HTTP path.
    pub capabilities: String,
}

impl ClientIdentity {
    /// Returns the connection's workspace scope in stable order, preferring the
    /// multi-workspace field and falling back to `workspace_id` for older
    /// tests/callers.
    pub fn authorized_workspace_ids(&self) -> Vec<String> {
        fn add_unique(seen: &mut HashSet<String>, out: &mut Vec<String>, id: &str) {
            let id = id.trim();
            if id.is_empty() || !seen.insert(id.to_string()) {
                return;
            }
            out.push(id.to_string());
        }
        let mut seen = HashSet::with_capacity(self.workspace_ids.len() + 1);
        let mut out = Vec::with_capacity(self.workspace_ids.len() + 1);
        for id in &self.workspace_ids {
            add_unique(&mut seen, &mut out, id);
        }
        if out.is_empty() {
            add_unique(&mut seen, &mut out, &self.workspace_id);
        }
        out
    }

    pub fn primary_workspace_id(&self) -> String {
        self.authorized_workspace_ids()
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    /// Reports whether `workspace_id` is within the connection scope. An empty
    /// scope remains permissive for legacy unit tests that construct
    /// `ClientIdentity` directly without workspace data.
    pub fn allows_workspace(&self, workspace_id: &str) -> bool {
        let ids = self.authorized_workspace_ids();
        if ids.is_empty() {
            return true;
        }
        ids.iter().any(|id| id == workspace_id)
    }
}

/// Dedup cache with bounded LRU semantics (capacity 128, matching Go).
/// Event IDs are ULIDs so only the last few need tracking.
#[derive(Default)]
pub(crate) struct DedupCache {
    seen: HashSet<String>,
    order: VecDeque<String>,
}

impl DedupCache {
    /// Records eventID as already delivered. Returns true when first seen
    /// (caller should deliver), false when duplicate (caller should drop).
    /// Empty event IDs disable dedup and are always delivered.
    pub(crate) fn mark_seen(&mut self, event_id: &str) -> bool {
        if event_id.is_empty() {
            return true;
        }
        if !self.seen.insert(event_id.to_string()) {
            return false;
        }
        self.order.push_back(event_id.to_string());
        if self.order.len() > EVENT_DEDUP_CAPACITY {
            if let Some(drop) = self.order.pop_front() {
                self.seen.remove(&drop);
            }
        }
        true
    }
}

/// Per-connection state owned by the hub. The WS pump half lives in the axum
/// handler lane and consumes from `sender`.
pub struct DaemonClient {
    pub id: DaemonClientId,
    pub identity: ClientIdentity,
    /// Runtime IDs this connection authenticated for; heartbeats outside the
    /// set are rejected.
    runtimes: HashSet<String>,
    /// Outbound frame queue consumed by the connection's write task
    /// (Go `send chan []byte`, capacity 16).
    pub sender: mpsc::Sender<Vec<u8>>,
    dedup: Mutex<DedupCache>,
    /// Cancelled when the connection tears down so async RPC handlers stop
    /// instead of running against a dead socket (Go `ctx`/`cancel`).
    pub conn_cancel: CancellationToken,
    /// Bounds concurrent RPC handlers for this connection (Go `rpcSem`).
    rpc_permits: Arc<Semaphore>,
}

impl DaemonClient {
    /// Records eventID as delivered; false means duplicate (drop it).
    fn mark_seen(&self, event_id: &str) -> bool {
        self.dedup
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mark_seen(event_id)
    }

    /// Delivers frame to the write pump without blocking and without ever
    /// writing to a torn-down connection (safe against concurrent teardown).
    /// Returns false when the buffer is full or the connection is closing.
    pub fn try_send(&self, frame: Vec<u8>) -> bool {
        matches!(self.sender.try_send(frame), Ok(()))
    }

    /// Reports whether `runtime_id` is inside the connection's authenticated
    /// scope.
    pub fn authorizes_runtime(&self, runtime_id: &str) -> bool {
        self.runtimes.contains(runtime_id)
    }
}

/// Processes a `daemon:heartbeat` frame. Implementations must verify that
/// `runtime_id` is one of `identity.runtime_ids` (the connection's
/// authenticated scope) and return the ack payload to send back. Returning an
/// error skips the ack and is logged at debug level.
///
/// Deliberately NOT time-bounded by the hub: the production handler reaches
/// LocalSkill{List,Import}Store.PopPending, whose Redis Lua claim script has
/// side effects that cannot be safely un-run if cancelled mid-script — the
/// same invariant that keeps the HTTP heartbeat from putting a per-call
/// timeout on PopPending.
#[async_trait]
pub trait HeartbeatHandler: Send + Sync {
    async fn handle_heartbeat(
        &self,
        identity: &ClientIdentity,
        runtime_id: &str,
        supports_batch_import: bool,
    ) -> anyhow::Result<Option<DaemonHeartbeatAckPayload>>;
}

/// Successful RPC outcome: an HTTP-style status plus a response body.
pub struct RpcOutcome {
    pub status: u16,
    pub body: Option<Value>,
}

/// Failed RPC outcome surfaced to the daemon as a non-2xx response so it can
/// fall back to HTTP. A status below 400 is coerced to 500 by the hub
/// (matching Go's `if status < 400 { status = http.StatusInternalServerError }`).
pub struct RpcHandlerError {
    pub status: u16,
    pub error: anyhow::Error,
}

impl RpcHandlerError {
    pub fn new(status: u16, error: anyhow::Error) -> Self {
        Self { status, error }
    }
}

/// Processes a generic `daemon:rpc_request` (MUL-4257). Dispatches on `method`
/// (e.g. "tasks.claim"), scoping work to identity (daemon ID + authenticated
/// runtime IDs). The handler runs in its own spawned task, so it must not
/// assume it owns the read pump. `ctx` is cancelled when the connection tears
/// down or when the request's `timeout_ms` budget elapses server-side, so a
/// slow RPC is cancelled — and its work rolled back — rather than committing
/// after the daemon has already timed out and fallen back to HTTP (MUL-4257).
#[async_trait]
pub trait RpcHandler: Send + Sync {
    async fn handle_rpc(
        &self,
        ctx: &CancellationToken,
        identity: &ClientIdentity,
        method: &str,
        body: Option<&Value>,
    ) -> Result<RpcOutcome, RpcHandlerError>;
}

/// Optional metric hook called once per inbound daemon WebSocket frame. `kind`
/// is the protocol message type with the `"daemon:"` prefix stripped (e.g.
/// "heartbeat") or the literal "unknown" for types we don't model ("invalid"
/// for frames that fail envelope parsing). Absence of a recorder is no-op'd.
pub trait MessageKindRecorder: Send + Sync {
    fn record_daemon_ws_message_received(&self, kind: &str);
}

// ---- metrics (S9-integration: ports internal/daemonws/metrics.go) --------

/// Lightweight daemon-WS counters. Field names mirror the Go struct; the JSON
/// keys in [`Metrics::snapshot`] match Go's exactly.
pub struct Metrics {
    pub connects_total: AtomicI64,
    pub disconnects_total: AtomicI64,
    pub active_connections: AtomicI64,
    pub slow_evictions_total: AtomicI64,

    pub wakeup_published_total: AtomicI64,
    pub wakeup_publish_errors: AtomicI64,
    pub wakeup_received_total: AtomicI64,
    pub wakeup_delivered_hit: AtomicI64,
    pub wakeup_delivered_miss: AtomicI64,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            connects_total: AtomicI64::new(0),
            disconnects_total: AtomicI64::new(0),
            active_connections: AtomicI64::new(0),
            slow_evictions_total: AtomicI64::new(0),
            wakeup_published_total: AtomicI64::new(0),
            wakeup_publish_errors: AtomicI64::new(0),
            wakeup_received_total: AtomicI64::new(0),
            wakeup_delivered_hit: AtomicI64::new(0),
            wakeup_delivered_miss: AtomicI64::new(0),
        }
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            connects_total: AtomicI64::new(0),
            disconnects_total: AtomicI64::new(0),
            active_connections: AtomicI64::new(0),
            slow_evictions_total: AtomicI64::new(0),
            wakeup_published_total: AtomicI64::new(0),
            wakeup_publish_errors: AtomicI64::new(0),
            wakeup_received_total: AtomicI64::new(0),
            wakeup_delivered_hit: AtomicI64::new(0),
            wakeup_delivered_miss: AtomicI64::new(0),
        }
    }

    /// JSON-friendly copy of the current counter values. Key names match the
    /// Go implementation byte-for-byte.
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "connects_total": self.connects_total.load(Ordering::Relaxed),
            "disconnects_total": self.disconnects_total.load(Ordering::Relaxed),
            "active_connections": self.active_connections.load(Ordering::Relaxed),
            "slow_evictions_total": self.slow_evictions_total.load(Ordering::Relaxed),
            "wakeup_published_total": self.wakeup_published_total.load(Ordering::Relaxed),
            "wakeup_publish_errors": self.wakeup_publish_errors.load(Ordering::Relaxed),
            "wakeup_received_total": self.wakeup_received_total.load(Ordering::Relaxed),
            "wakeup_delivered_hit_total": self.wakeup_delivered_hit.load(Ordering::Relaxed),
            "wakeup_delivered_miss_total": self.wakeup_delivered_miss.load(Ordering::Relaxed),
        })
    }

    /// Zeroes all counters. Tests only.
    pub fn reset(&self) {
        for counter in [
            &self.connects_total,
            &self.disconnects_total,
            &self.active_connections,
            &self.slow_evictions_total,
            &self.wakeup_published_total,
            &self.wakeup_publish_errors,
            &self.wakeup_received_total,
            &self.wakeup_delivered_hit,
            &self.wakeup_delivered_miss,
        ] {
            counter.store(0, Ordering::Relaxed);
        }
    }
}

/// Package-level metrics singleton (Go `var M = &Metrics{}`).
pub static M: std::sync::LazyLock<Metrics> = std::sync::LazyLock::new(Metrics::default);

// ---- hub -------------------------------------------------------------------

#[derive(Default)]
struct HubInner {
    clients: HashMap<DaemonClientId, Arc<DaemonClient>>,
    by_runtime: HashMap<String, HashSet<DaemonClientId>>,
    by_workspace: HashMap<String, HashSet<DaemonClientId>>,
    by_user: HashMap<String, HashSet<DaemonClientId>>,
}

/// Keeps daemon WebSocket connections indexed by runtime ID. Messages are
/// best-effort wakeup hints; the daemon still uses HTTP claim for correctness.
pub struct DaemonHub {
    inner: RwLock<HubInner>,

    heartbeat_slot: RwLock<Option<Arc<dyn HeartbeatHandler>>>,
    rpc_slot: RwLock<Option<Arc<dyn RpcHandler>>>,
    kind_recorder_slot: RwLock<Option<Arc<dyn MessageKindRecorder>>>,

    next_client_id: AtomicU64,
}

impl Default for DaemonHub {
    fn default() -> Self {
        Self::new()
    }
}

impl DaemonHub {
    /// Creates a new hub (Go `NewHub`). The gorilla upgrader (and its
    /// always-permissive CheckOrigin, justified there because daemon clients
    /// authenticate with Authorization headers before the upgrade and
    /// DaemonAuth does not accept cookies) belongs to the axum lane.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HubInner::default()),
            heartbeat_slot: RwLock::new(None),
            rpc_slot: RwLock::new(None),
            kind_recorder_slot: RwLock::new(None),
            next_client_id: AtomicU64::new(1),
        }
    }

    fn alloc_client_id(&self) -> DaemonClientId {
        DaemonClientId(self.next_client_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Installs the callback used for `daemon:heartbeat` frames. Wiring is done
    /// after handler construction because the handler depends on DB queries
    /// that aren't available when the hub is built. `None` disables WS
    /// heartbeat processing — daemons fall back to HTTP heartbeat
    /// transparently because their fallback timer fires whenever no ack
    /// arrives.
    pub fn set_heartbeat_handler(&self, handler: Option<Arc<dyn HeartbeatHandler>>) {
        *self
            .heartbeat_slot
            .write()
            .unwrap_or_else(|e| e.into_inner()) = handler;
    }

    fn heartbeat_handler(&self) -> Option<Arc<dyn HeartbeatHandler>> {
        self.heartbeat_slot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Installs the callback used for `daemon:rpc_request` frames (MUL-4257).
    /// Like [`Self::set_heartbeat_handler`] it is wired after handler
    /// construction. `None` disables WS RPC — daemons fall back to the HTTP
    /// claim endpoint.
    pub fn set_rpc_handler(&self, handler: Option<Arc<dyn RpcHandler>>) {
        *self.rpc_slot.write().unwrap_or_else(|e| e.into_inner()) = handler;
    }

    fn rpc_handler(&self) -> Option<Arc<dyn RpcHandler>> {
        self.rpc_slot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Installs an optional callback fired exactly once per inbound daemon
    /// WebSocket frame. Used by the metrics layer to count traffic by handler
    /// kind without hard-coupling the hub to any specific collector.
    pub fn set_message_kind_recorder(&self, recorder: Option<Arc<dyn MessageKindRecorder>>) {
        *self
            .kind_recorder_slot
            .write()
            .unwrap_or_else(|e| e.into_inner()) = recorder;
    }

    fn message_kind_recorder(&self) -> Option<Arc<dyn MessageKindRecorder>> {
        self.kind_recorder_slot
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// HandleWebSocket's pre-upgrade identity guard: rejects connections with
    /// neither runtime IDs nor a user identity. Returns the Go error body text
    /// (`{"error":"runtime_ids or user identity required"}`, HTTP 400). The
    /// upgrade + pump spawn themselves land with the axum lane.
    pub fn validate_identity(identity: &ClientIdentity) -> Result<(), &'static str> {
        if identity.runtime_ids.is_empty() && identity.user_id.is_empty() {
            return Err("runtime_ids or user identity required");
        }
        Ok(())
    }

    /// Registers a connected client under its runtime / workspace / user
    /// scopes. Returns the client handle plus the outbound queue consumer for
    /// the connection's write task. Callers must have passed
    /// [`Self::validate_identity`] first (Go does this inside HandleWebSocket).
    pub fn register(
        &self,
        identity: ClientIdentity,
    ) -> (Arc<DaemonClient>, mpsc::Receiver<Vec<u8>>) {
        let id = self.alloc_client_id();
        let (tx, rx) = mpsc::channel(SEND_BUFFER_CAPACITY);
        let mut runtimes = HashSet::with_capacity(identity.runtime_ids.len());
        for runtime_id in &identity.runtime_ids {
            if !runtime_id.is_empty() {
                runtimes.insert(runtime_id.clone());
            }
        }

        let client = Arc::new(DaemonClient {
            id,
            identity: identity.clone(),
            runtimes,
            sender: tx,
            dedup: Mutex::new(DedupCache::default()),
            conn_cancel: CancellationToken::new(),
            rpc_permits: Arc::new(Semaphore::new(MAX_IN_FLIGHT_RPC_PER_CLIENT)),
        });

        let workspace_ids = identity.authorized_workspace_ids();
        let total;
        {
            let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
            inner.clients.insert(id, client.clone());
            for runtime_id in &client.runtimes {
                inner
                    .by_runtime
                    .entry(runtime_id.clone())
                    .or_default()
                    .insert(id);
            }
            for workspace_id in &workspace_ids {
                inner
                    .by_workspace
                    .entry(workspace_id.clone())
                    .or_default()
                    .insert(id);
            }
            if !identity.user_id.is_empty() {
                inner
                    .by_user
                    .entry(identity.user_id.clone())
                    .or_default()
                    .insert(id);
            }
            total = inner.clients.len();
        }

        M.connects_total.fetch_add(1, Ordering::Relaxed);
        M.active_connections.fetch_add(1, Ordering::Relaxed);
        tracing::info!(
            daemon_id = %identity.daemon_id,
            user_id = %identity.user_id,
            workspace_id = %identity.primary_workspace_id(),
            workspace_ids = ?workspace_ids,
            runtimes = client.runtimes.len(),
            client_version = %identity.client_version,
            total_clients = total,
            "daemon websocket connected"
        );
        (client, rx)
    }

    /// Drops a client from every index and logs the disconnect. Idempotent:
    /// unknown ids (already evicted) are a no-op, mirroring Go's
    /// `if !h.clients[c] { return }` guard. The caller-owned receiver half
    /// dropping is what closes the write pump's channel (Go closes `c.send`
    /// here); the connection token cancellation wakes both pumps.
    pub fn unregister(&self, id: DaemonClientId) {
        let client = {
            let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
            // Unknown id (already removed by eviction) — nothing to do.
            let Some(client) = inner.clients.remove(&id) else {
                return;
            };
            for runtime_id in &client.runtimes {
                if let Some(conns) = inner.by_runtime.get_mut(runtime_id) {
                    conns.remove(&id);
                    if conns.is_empty() {
                        inner.by_runtime.remove(runtime_id);
                    }
                }
            }
            let workspace_ids = client.identity.authorized_workspace_ids();
            for workspace_id in &workspace_ids {
                if let Some(conns) = inner.by_workspace.get_mut(workspace_id) {
                    conns.remove(&id);
                    if conns.is_empty() {
                        inner.by_workspace.remove(workspace_id);
                    }
                }
            }
            if !client.identity.user_id.is_empty() {
                if let Some(conns) = inner.by_user.get_mut(&client.identity.user_id) {
                    conns.remove(&id);
                    if conns.is_empty() {
                        inner.by_user.remove(&client.identity.user_id);
                    }
                }
            }
            let total = inner.clients.len();
            drop(inner);

            M.disconnects_total.fetch_add(1, Ordering::Relaxed);
            M.active_connections.fetch_add(-1, Ordering::Relaxed);
            tracing::info!(
                daemon_id = %client.identity.daemon_id,
                user_id = %client.identity.user_id,
                workspace_id = %client.identity.primary_workspace_id(),
                workspace_ids = ?client.identity.authorized_workspace_ids(),
                runtimes = client.runtimes.len(),
                total_clients = total,
                "daemon websocket disconnected"
            );
            client
        };
        // Wake the pumps after releasing the lock (Go flips sendClosed + closes
        // send inside unregister; the Rust pump owner observes the token).
        client.conn_cancel.cancel();
    }

    /// Sends a best-effort wakeup to daemons watching `runtime_id`.
    pub fn notify_task_available(&self, runtime_id: &str, task_id: &str) {
        self.notify_task_available_with_event(runtime_id, task_id, "");
    }

    /// Asks connected daemons in `workspace_id` to pull runtime profiles after
    /// a create, update, disable, or delete.
    pub fn notify_runtime_profiles_changed(&self, workspace_id: &str, profile_id: &str) {
        self.notify_runtime_profiles_changed_with_event(workspace_id, profile_id, "");
    }

    /// Asks every connected daemon authenticated as `user_id` to reconcile its
    /// workspace membership set.
    pub fn notify_workspaces_changed(&self, user_id: &str) {
        self.notify_workspaces_changed_with_event(user_id, "");
    }

    /// Tells daemons watching `runtime_id` that a heartbeat-carried request is
    /// queued, so they can heartbeat now instead of waiting for the next
    /// scheduled tick (MUL-5444). Best-effort like every other hub
    /// notification: the daemon's own heartbeat schedule remains the
    /// correctness path.
    pub fn notify_pending_work(&self, runtime_id: &str, kind: &str) {
        self.notify_pending_work_with_event(runtime_id, kind, "");
    }

    // Event-ID-carrying variants — called by RelayNotifier with a fresh ULID so
    // local delivery and the Redis loopback dedup against each other.

    pub(crate) fn notify_task_available_with_event(
        &self,
        runtime_id: &str,
        task_id: &str,
        event_id: &str,
    ) {
        if runtime_id.is_empty() {
            return;
        }
        let Some(data) = task_available_frame(runtime_id, task_id) else {
            return;
        };
        let (delivered, deduped) = self.notify_frame(runtime_id, &data, event_id);
        if delivered {
            M.wakeup_delivered_hit.fetch_add(1, Ordering::Relaxed);
        } else if !deduped {
            M.wakeup_delivered_miss.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn notify_runtime_profiles_changed_with_event(
        &self,
        workspace_id: &str,
        profile_id: &str,
        event_id: &str,
    ) {
        if workspace_id.is_empty() {
            return;
        }
        let Some(data) = runtime_profiles_changed_frame(workspace_id, profile_id) else {
            return;
        };
        self.notify_workspace_frame(workspace_id, &data, event_id);
    }

    pub(crate) fn notify_workspaces_changed_with_event(&self, user_id: &str, event_id: &str) {
        if user_id.is_empty() {
            return;
        }
        let Some(data) = workspaces_changed_frame() else {
            return;
        };
        self.notify_user_frame(user_id, &data, event_id);
    }

    pub(crate) fn notify_pending_work_with_event(
        &self,
        runtime_id: &str,
        kind: &str,
        event_id: &str,
    ) {
        if runtime_id.is_empty() {
            return;
        }
        let Some(data) = pending_work_frame(runtime_id, kind) else {
            return;
        };
        let (delivered, deduped) = self.notify_frame(runtime_id, &data, event_id);
        if delivered {
            M.wakeup_delivered_hit.fetch_add(1, Ordering::Relaxed);
        } else if !deduped {
            M.wakeup_delivered_miss.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Routes a relay frame received off the Redis transport to the right local
    /// fanout based on the frame's own type. `scope_id` is the relay shard key
    /// (a runtime, workspace, or user key depending on the frame type).
    pub fn deliver_daemon_runtime(&self, scope_id: &str, frame: &[u8], event_id: &str) {
        M.wakeup_received_total.fetch_add(1, Ordering::Relaxed);
        let Ok(msg) = serde_json::from_slice::<Message>(frame) else {
            tracing::debug!(
                error = "invalid json",
                scope_id = %scope_id,
                event_id = %event_id,
                "daemon websocket relay: invalid frame"
            );
            M.wakeup_delivered_miss.fetch_add(1, Ordering::Relaxed);
            return;
        };
        match msg.r#type.as_str() {
            EVENT_DAEMON_TASK_AVAILABLE => {
                let payload: Result<TaskAvailablePayload, _> = serde_json::from_value(msg.payload);
                let runtime_id = payload.ok().filter(|p| !p.runtime_id.is_empty());
                let Some(payload) = runtime_id else {
                    tracing::debug!(
                        scope_id = %scope_id,
                        event_id = %event_id,
                        "daemon websocket relay: invalid task_available payload"
                    );
                    M.wakeup_delivered_miss.fetch_add(1, Ordering::Relaxed);
                    return;
                };
                let (delivered, deduped) = self.notify_frame(&payload.runtime_id, frame, event_id);
                if delivered {
                    M.wakeup_delivered_hit.fetch_add(1, Ordering::Relaxed);
                } else if !deduped {
                    M.wakeup_delivered_miss.fetch_add(1, Ordering::Relaxed);
                }
            }
            EVENT_DAEMON_RUNTIME_PROFILES_CHANGED => {
                let payload: Result<RuntimeProfilesChangedPayload, _> =
                    serde_json::from_value(msg.payload);
                let workspace_id = payload.ok().filter(|p| !p.workspace_id.is_empty());
                let Some(payload) = workspace_id else {
                    tracing::debug!(
                        scope_id = %scope_id,
                        event_id = %event_id,
                        "daemon websocket relay: invalid runtime_profiles_changed payload"
                    );
                    M.wakeup_delivered_miss.fetch_add(1, Ordering::Relaxed);
                    return;
                };
                let (delivered, deduped) =
                    self.notify_workspace_frame(&payload.workspace_id, frame, event_id);
                if delivered {
                    M.wakeup_delivered_hit.fetch_add(1, Ordering::Relaxed);
                } else if !deduped {
                    M.wakeup_delivered_miss.fetch_add(1, Ordering::Relaxed);
                }
            }
            EVENT_DAEMON_WORKSPACES_CHANGED => {
                let (delivered, deduped) = self.notify_user_frame(scope_id, frame, event_id);
                if delivered {
                    M.wakeup_delivered_hit.fetch_add(1, Ordering::Relaxed);
                } else if !deduped {
                    M.wakeup_delivered_miss.fetch_add(1, Ordering::Relaxed);
                }
            }
            EVENT_DAEMON_PENDING_WORK => {
                let payload: Result<PendingWorkPayload, _> = serde_json::from_value(msg.payload);
                let runtime_id = payload.ok().filter(|p| !p.runtime_id.is_empty());
                let Some(payload) = runtime_id else {
                    tracing::debug!(
                        scope_id = %scope_id,
                        event_id = %event_id,
                        "daemon websocket relay: invalid pending_work payload"
                    );
                    M.wakeup_delivered_miss.fetch_add(1, Ordering::Relaxed);
                    return;
                };
                let (delivered, deduped) = self.notify_frame(&payload.runtime_id, frame, event_id);
                if delivered {
                    M.wakeup_delivered_hit.fetch_add(1, Ordering::Relaxed);
                } else if !deduped {
                    M.wakeup_delivered_miss.fetch_add(1, Ordering::Relaxed);
                }
            }
            _ => {
                M.wakeup_delivered_miss.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Shared fanout core behind `notifyFrame` / `notifyWorkspaceFrame` /
    /// `notifyUserFrame`: dedups against `event_id`, queues the frame without
    /// blocking, and evicts slow clients after releasing the read lock.
    /// Returns `(delivered, deduped)`.
    fn deliver_to_room(
        &self,
        room: &HashSet<DaemonClientId>,
        data: &[u8],
        event_id: &str,
    ) -> (bool, bool) {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let mut delivered = false;
        let mut deduped = false;
        let mut slow: Vec<Arc<DaemonClient>> = Vec::new();
        for client_id in room {
            let Some(client) = inner.clients.get(client_id) else {
                continue;
            };
            if !client.mark_seen(event_id) {
                deduped = true;
                continue;
            }
            match client.sender.try_send(data.to_vec()) {
                Ok(()) => delivered = true,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    slow.push(client.clone());
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    // Pump already gone; eviction sweep reaps it.
                    slow.push(client.clone());
                }
            }
        }
        drop(inner);

        if !slow.is_empty() {
            self.evict_slow(&slow);
        }
        (delivered, deduped)
    }

    fn notify_frame(&self, runtime_id: &str, data: &[u8], event_id: &str) -> (bool, bool) {
        let room = {
            let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
            inner.by_runtime.get(runtime_id).cloned()
        };
        let Some(room) = room else {
            return (false, false);
        };
        self.deliver_to_room(&room, data, event_id)
    }

    fn notify_workspace_frame(
        &self,
        workspace_id: &str,
        data: &[u8],
        event_id: &str,
    ) -> (bool, bool) {
        let room = {
            let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
            inner.by_workspace.get(workspace_id).cloned()
        };
        let Some(room) = room else {
            return (false, false);
        };
        self.deliver_to_room(&room, data, event_id)
    }

    fn notify_user_frame(&self, user_id: &str, data: &[u8], event_id: &str) -> (bool, bool) {
        let room = {
            let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
            inner.by_user.get(user_id).cloned()
        };
        let Some(room) = room else {
            return (false, false);
        };
        self.deliver_to_room(&room, data, event_id)
    }

    /// Removes clients whose send queue was full (Go: `h.unregister(c)` +
    /// `c.conn.Close()` in the notify paths). Cancels each connection token so
    /// the pumps tear down.
    fn evict_slow(&self, slow: &[Arc<DaemonClient>]) {
        M.slow_evictions_total
            .fetch_add(slow.len() as i64, Ordering::Relaxed);
        for client in slow {
            self.unregister(client.id);
        }
    }

    pub fn runtime_connection_count(&self, runtime_id: &str) -> usize {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .by_runtime
            .get(runtime_id)
            .map_or(0, HashSet::len)
    }

    pub fn workspace_connection_count(&self, workspace_id: &str) -> usize {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .by_workspace
            .get(workspace_id)
            .map_or(0, HashSet::len)
    }

    pub fn user_connection_count(&self, user_id: &str) -> usize {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .by_user
            .get(user_id)
            .map_or(0, HashSet::len)
    }

    // ---- inbound frame handling (bookkeeping half of readPump) -------------

    /// Parses one inbound frame, records its kind with the optional recorder,
    /// and dispatches heartbeat / RPC payloads. Unknown app messages are
    /// intentionally ignored for forward compatibility with future daemon →
    /// server message types. Called by the read pump (axum lane).
    pub async fn handle_frame(self: &Arc<Self>, client: &Arc<DaemonClient>, raw: &[u8]) {
        let Ok(msg) = serde_json::from_slice::<Message>(raw) else {
            tracing::debug!(
                daemon_id = %client.identity.daemon_id,
                "daemon websocket invalid frame"
            );
            if let Some(rec) = self.message_kind_recorder() {
                rec.record_daemon_ws_message_received("invalid");
            }
            return;
        };
        let kind = msg.r#type.strip_prefix("daemon:").unwrap_or(&msg.r#type);
        let kind = if kind.is_empty() { "unknown" } else { kind };
        if let Some(rec) = self.message_kind_recorder() {
            rec.record_daemon_ws_message_received(kind);
        }
        match msg.r#type.as_str() {
            EVENT_DAEMON_HEARTBEAT => {
                self.handle_heartbeat_frame(client, &msg.payload).await;
            }
            EVENT_DAEMON_RPC_REQUEST => {
                self.handle_rpc_frame(client, &msg.payload).await;
            }
            _ => {}
        }
    }

    /// Processes a generic `daemon:rpc_request` (MUL-4257): runs the registered
    /// RPC handler in its own task (so a DB-bound claim does not stall the read
    /// pump or the next heartbeat) and writes back a `daemon:rpc_response`
    /// echoing the request id. A missing handler or a full in-flight slot yields
    /// a non-2xx response so the daemon falls back to HTTP. Called by the read
    /// pump (axum lane).
    pub async fn handle_rpc_frame(&self, client: &Arc<DaemonClient>, payload: &Value) {
        let Ok(req) = serde_json::from_value::<RpcRequestPayload>(payload.clone()) else {
            tracing::debug!(
                daemon_id = %client.identity.daemon_id,
                "daemon websocket rpc invalid payload"
            );
            return;
        };
        if req.request_id.is_empty() {
            tracing::debug!(
                daemon_id = %client.identity.daemon_id,
                "daemon websocket rpc missing request_id"
            );
            return;
        }
        let Some(handler) = self.rpc_handler() else {
            Self::send_rpc_response(
                client,
                &req.request_id,
                HTTP_STATUS_SERVICE_UNAVAILABLE,
                None,
                "rpc handler unavailable",
            );
            return;
        };
        // Bound concurrent handlers; if saturated, tell the daemon to fall back
        // rather than queueing unbounded work on one socket.
        let permit = match client.rpc_permits.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                Self::send_rpc_response(
                    client,
                    &req.request_id,
                    HTTP_STATUS_TOO_MANY_REQUESTS,
                    None,
                    "too many in-flight rpc requests",
                );
                return;
            }
        };

        let client = client.clone();
        let ctx = client.conn_cancel.clone();
        let method = req.method.clone();
        let body = req.body.clone();
        let request_id = req.request_id.clone();
        let timeout_ms = req.timeout_ms;
        tokio::spawn(async move {
            let _permit = permit;
            // Bound server-side execution by the caller's requested budget (in
            // addition to the connection token), so a slow RPC is cancelled —
            // and its work rolled back — rather than committing after the
            // daemon has already timed out and fallen back to HTTP (MUL-4257).
            // The daemon waits a grace period beyond this budget, so a claim
            // that DID commit before the deadline still reports back in time.
            let call = handler.handle_rpc(&ctx, &client.identity, &method, body.as_ref());
            let outcome = if timeout_ms > 0 {
                tokio::time::timeout(Duration::from_millis(timeout_ms as u64), call)
                    .await
                    .unwrap_or_else(|_| {
                        Err(RpcHandlerError {
                            status: HTTP_STATUS_INTERNAL_SERVER_ERROR as u16,
                            error: anyhow::anyhow!("context deadline exceeded"),
                        })
                    })
            } else {
                call.await
            };
            match outcome {
                Ok(resp) => {
                    Self::send_rpc_response(
                        &client,
                        &request_id,
                        resp.status as i32,
                        resp.body,
                        "",
                    );
                }
                Err(err) => {
                    let status = if err.status < 400 {
                        HTTP_STATUS_INTERNAL_SERVER_ERROR
                    } else {
                        err.status as i32
                    };
                    Self::send_rpc_response(
                        &client,
                        &request_id,
                        status,
                        None,
                        &format!("{:#}", err.error),
                    );
                }
            }
        });
    }

    /// Builds and queues a `daemon:rpc_response` echoing `request_id`. A full
    /// buffer or closing connection drops the response; the daemon's
    /// per-request timeout fires and it falls back to HTTP. An associated
    /// function because it touches no hub state — the spawned RPC task needs
    /// only the client handle.
    pub(crate) fn send_rpc_response(
        client: &DaemonClient,
        request_id: &str,
        status: i32,
        body: Option<Value>,
        error_msg: &str,
    ) {
        let payload = match serde_json::to_value(RpcResponsePayload {
            request_id: request_id.to_string(),
            status,
            body,
            error: error_msg.to_string(),
        }) {
            Ok(payload) => payload,
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    "daemon websocket rpc response marshal failed"
                );
                return;
            }
        };
        let frame = match serde_json::to_vec(&Message {
            r#type: EVENT_DAEMON_RPC_RESPONSE.to_string(),
            payload,
        }) {
            Ok(frame) => frame,
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    "daemon websocket rpc response marshal failed"
                );
                return;
            }
        };
        if !client.try_send(frame) {
            tracing::debug!(
                daemon_id = %client.identity.daemon_id,
                request_id = %request_id,
                "daemon websocket rpc response dropped"
            );
        }
    }

    /// Processes an inbound `daemon:heartbeat` from the daemon, invokes the
    /// hub's handler, and writes back a `daemon:heartbeat_ack`.
    ///
    /// Intentionally does NOT wrap the handler call in a timeout: the handler
    /// reaches LocalSkill{List,Import}Store.PopPending, whose Redis Lua claim
    /// script has side effects (ZREM + SET-running) that cannot be safely
    /// un-run if cancelled mid-script. The natural bound is the read pump's
    /// lifetime plus Redis's own server-side limits. Called by the read pump
    /// (axum lane).
    pub async fn handle_heartbeat_frame(&self, client: &DaemonClient, payload: &Value) {
        let Some(handler) = self.heartbeat_handler() else {
            // Server doesn't have a heartbeat handler wired — daemon will time
            // out waiting for an ack and fall back to HTTP heartbeat.
            return;
        };

        let Ok(req) = serde_json::from_value::<DaemonHeartbeatRequestPayload>(payload.clone())
        else {
            tracing::debug!(
                daemon_id = %client.identity.daemon_id,
                "daemon websocket heartbeat invalid payload"
            );
            return;
        };
        if req.runtime_id.is_empty() {
            tracing::debug!(
                daemon_id = %client.identity.daemon_id,
                "daemon websocket heartbeat missing runtime_id"
            );
            return;
        }
        if !client.authorizes_runtime(&req.runtime_id) {
            // The connection authenticated for a fixed runtime set; reject any
            // heartbeat for a runtime the client did not register for.
            tracing::warn!(
                daemon_id = %client.identity.daemon_id,
                runtime_id = %req.runtime_id,
                "daemon websocket heartbeat for unauthorized runtime"
            );
            return;
        }

        let ack = handler
            .handle_heartbeat(&client.identity, &req.runtime_id, req.supports_batch_import)
            .await;
        let ack = match ack {
            Ok(Some(ack)) => ack,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    daemon_id = %client.identity.daemon_id,
                    runtime_id = %req.runtime_id,
                    "daemon websocket heartbeat handler failed"
                );
                return;
            }
        };
        let payload = match serde_json::to_value(&ack) {
            Ok(payload) => payload,
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    "daemon websocket heartbeat ack marshal failed"
                );
                return;
            }
        };
        let frame = match serde_json::to_vec(&Message {
            r#type: EVENT_DAEMON_HEARTBEAT_ACK.to_string(),
            payload,
        }) {
            Ok(frame) => frame,
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    "daemon websocket heartbeat ack marshal failed"
                );
                return;
            }
        };
        if !client.try_send(frame) {
            // Send buffer full or connection closing — drop; HTTP heartbeat
            // resumes.
            tracing::debug!(
                daemon_id = %client.identity.daemon_id,
                runtime_id = %req.runtime_id,
                "daemon websocket heartbeat ack dropped"
            );
        }
    }
}

// ---- frame builders (shared with notifier.rs) ------------------------------

fn marshal_message(r#type: &str, payload: Value) -> Option<Vec<u8>> {
    serde_json::to_vec(&Message {
        r#type: r#type.to_string(),
        payload,
    })
    .ok()
}

pub(crate) fn task_available_frame(runtime_id: &str, task_id: &str) -> Option<Vec<u8>> {
    let payload = serde_json::to_value(TaskAvailablePayload {
        runtime_id: runtime_id.to_string(),
        task_id: task_id.to_string(),
    })
    .ok()?;
    marshal_message(EVENT_DAEMON_TASK_AVAILABLE, payload)
}

pub(crate) fn runtime_profiles_changed_frame(
    workspace_id: &str,
    profile_id: &str,
) -> Option<Vec<u8>> {
    let payload = serde_json::to_value(RuntimeProfilesChangedPayload {
        workspace_id: workspace_id.to_string(),
        runtime_profile_id: profile_id.to_string(),
    })
    .ok()?;
    marshal_message(EVENT_DAEMON_RUNTIME_PROFILES_CHANGED, payload)
}

pub(crate) fn workspaces_changed_frame() -> Option<Vec<u8>> {
    let payload = serde_json::to_value(WorkspacesChangedPayload {}).ok()?;
    marshal_message(EVENT_DAEMON_WORKSPACES_CHANGED, payload)
}

pub(crate) fn pending_work_frame(runtime_id: &str, kind: &str) -> Option<Vec<u8>> {
    let payload = serde_json::to_value(PendingWorkPayload {
        runtime_id: runtime_id.to_string(),
        kind: kind.to_string(),
    })
    .ok()?;
    marshal_message(EVENT_DAEMON_PENDING_WORK, payload)
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared by hub.rs AND notifier.rs tests: both assert on the global [`M`]
    //! counters, so all such tests must serialise on this one mutex.
    use std::sync::Mutex as StdMutex;

    static METRICS_GUARD: StdMutex<()> = StdMutex::new(());

    pub(crate) fn lock_metrics() -> std::sync::MutexGuard<'static, ()> {
        METRICS_GUARD.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn reset_metrics() {
        super::M.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{lock_metrics, reset_metrics};
    use super::*;
    use cordy_protocol::PENDING_WORK_KIND_MODEL_LIST;
    use std::sync::Mutex as StdMutex;

    /// Attaches a bare test client directly into the hub indexes (port of
    /// attachDaemonTestClient / attachDaemonWorkspaceTestClient /
    /// attachDaemonUserTestClient, which poke hub.mu internals in Go).
    fn attach_client(
        hub: &DaemonHub,
        identity: ClientIdentity,
        runtimes: &[&str],
        scope: Scope,
    ) -> (Arc<DaemonClient>, mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = mpsc::channel(2);
        let client = Arc::new(DaemonClient {
            id: hub.alloc_client_id(),
            identity,
            runtimes: runtimes.iter().map(|s| s.to_string()).collect(),
            sender: tx,
            dedup: Mutex::new(DedupCache::default()),
            conn_cancel: CancellationToken::new(),
            rpc_permits: Arc::new(Semaphore::new(MAX_IN_FLIGHT_RPC_PER_CLIENT)),
        });
        let mut inner = hub.inner.write().unwrap_or_else(|e| e.into_inner());
        inner.clients.insert(client.id, client.clone());
        match scope {
            Scope::Runtime(key) => {
                inner
                    .by_runtime
                    .entry(key.to_string())
                    .or_default()
                    .insert(client.id);
            }
            Scope::Workspace(key) => {
                inner
                    .by_workspace
                    .entry(key.to_string())
                    .or_default()
                    .insert(client.id);
            }
            Scope::User(key) => {
                inner
                    .by_user
                    .entry(key.to_string())
                    .or_default()
                    .insert(client.id);
            }
        }
        (client, rx)
    }

    enum Scope {
        Runtime(&'static str),
        Workspace(&'static str),
        User(&'static str),
    }

    fn decode_frame(frame: &[u8]) -> (String, Value) {
        let msg: Message = serde_json::from_slice(frame).expect("test frame decodes");
        (msg.r#type, msg.payload)
    }

    // ---- ClientIdentity ----------------------------------------------------

    #[test]
    fn authorized_workspace_ids_dedups_trims_and_falls_back() {
        let multi = ClientIdentity {
            workspace_id: "legacy".into(),
            workspace_ids: vec!["ws-1".into(), " ws-1 ".into(), "".into(), "ws-2".into()],
            ..Default::default()
        };
        assert_eq!(
            multi.authorized_workspace_ids(),
            vec!["ws-1".to_string(), "ws-2".to_string()]
        );
        assert_eq!(multi.primary_workspace_id(), "ws-1");

        let legacy = ClientIdentity {
            workspace_id: "ws-legacy".into(),
            ..Default::default()
        };
        assert_eq!(legacy.authorized_workspace_ids(), vec!["ws-legacy"]);
        assert_eq!(legacy.primary_workspace_id(), "ws-legacy");

        let empty = ClientIdentity::default();
        assert!(empty.authorized_workspace_ids().is_empty());
        assert_eq!(empty.primary_workspace_id(), "");
    }

    #[test]
    fn allows_workspace_permissive_when_scope_empty() {
        let scoped = ClientIdentity {
            workspace_ids: vec!["ws-1".into()],
            ..Default::default()
        };
        assert!(scoped.allows_workspace("ws-1"));
        assert!(!scoped.allows_workspace("ws-2"));

        let unscoped = ClientIdentity::default();
        // Empty scope stays permissive for legacy direct constructions.
        assert!(unscoped.allows_workspace("anything"));
    }

    #[test]
    fn validate_identity_requires_runtime_or_user() {
        assert_eq!(
            DaemonHub::validate_identity(&ClientIdentity::default()),
            Err("runtime_ids or user identity required")
        );
        assert!(DaemonHub::validate_identity(&ClientIdentity {
            runtime_ids: vec!["rt-1".into()],
            ..Default::default()
        })
        .is_ok());
        assert!(DaemonHub::validate_identity(&ClientIdentity {
            user_id: "u-1".into(),
            ..Default::default()
        })
        .is_ok());
    }

    // ---- dedup LRU -----------------------------------------------------------

    #[test]
    fn mark_seen_lru_capacity_and_duplicates() {
        let mut cache = DedupCache::default();
        assert!(cache.mark_seen(""), "empty id always delivers");
        for i in 0..EVENT_DEDUP_CAPACITY {
            assert!(cache.mark_seen(&format!("e{i}")));
        }
        // Duplicate within window.
        assert!(!cache.mark_seen("e0"));
        // One more pushes e0 out of the LRU window.
        assert!(cache.mark_seen("overflow"));
        assert!(cache.mark_seen("e0"), "e0 was evicted from the LRU window");
    }

    // ---- routing tables (WS-dial tests from hub_test.go, minus the socket) --

    #[test]
    fn notify_task_available_reaches_runtime_room() {
        let _guard = lock_metrics();
        reset_metrics();

        let hub = DaemonHub::new();
        let (client, mut rx) = attach_client(
            &hub,
            ClientIdentity {
                runtime_ids: vec!["runtime-1".into()],
                ..Default::default()
            },
            &["runtime-1"],
            Scope::Runtime("runtime-1"),
        );

        hub.notify_task_available("runtime-1", "task-1");

        let frame = rx.try_recv().expect("wakeup queued");
        let (r#type, payload) = decode_frame(&frame);
        assert_eq!(r#type, EVENT_DAEMON_TASK_AVAILABLE);
        assert_eq!(payload["runtime_id"], "runtime-1");
        assert_eq!(payload["task_id"], "task-1");
        assert_eq!(M.wakeup_delivered_hit.load(Ordering::Relaxed), 1);
        assert_eq!(M.wakeup_delivered_miss.load(Ordering::Relaxed), 0);
        drop(client);
    }

    #[test]
    fn notify_runtime_profiles_changed_reaches_workspace_room() {
        let _guard = lock_metrics();
        reset_metrics();

        let hub = DaemonHub::new();
        let (_client, mut rx) = attach_client(
            &hub,
            ClientIdentity {
                workspace_id: "ws-1".into(),
                runtime_ids: vec!["runtime-1".into()],
                ..Default::default()
            },
            &["runtime-1"],
            Scope::Workspace("ws-1"),
        );

        hub.notify_runtime_profiles_changed("ws-1", "profile-1");

        let frame = rx.try_recv().expect("profile refresh queued");
        let (r#type, payload) = decode_frame(&frame);
        assert_eq!(r#type, EVENT_DAEMON_RUNTIME_PROFILES_CHANGED);
        assert_eq!(payload["workspace_id"], "ws-1");
        assert_eq!(payload["runtime_profile_id"], "profile-1");
    }

    #[test]
    fn notify_workspaces_changed_supports_account_only_connection() {
        let _guard = lock_metrics();
        reset_metrics();

        let hub = DaemonHub::new();
        let (_client, mut rx) = attach_client(
            &hub,
            ClientIdentity {
                user_id: "user-1".into(),
                ..Default::default()
            },
            &[],
            Scope::User("user-1"),
        );

        hub.notify_workspaces_changed("user-1");

        let frame = rx.try_recv().expect("workspaces changed queued");
        let (r#type, _payload) = decode_frame(&frame);
        assert_eq!(r#type, EVENT_DAEMON_WORKSPACES_CHANGED);
    }

    #[test]
    fn notify_pending_work_carries_kind_and_dedups_duplicate_event() {
        let _guard = lock_metrics();
        reset_metrics();

        let hub = DaemonHub::new();
        let (_client, mut rx) = attach_client(
            &hub,
            ClientIdentity {
                runtime_ids: vec!["runtime-1".into()],
                ..Default::default()
            },
            &["runtime-1"],
            Scope::Runtime("runtime-1"),
        );

        let frame =
            pending_work_frame("runtime-1", PENDING_WORK_KIND_MODEL_LIST).expect("frame builds");
        hub.deliver_daemon_runtime("runtime-1", &frame, "event-1");

        let got = rx.try_recv().expect("pending work queued");
        let (r#type, payload) = decode_frame(&got);
        assert_eq!(r#type, EVENT_DAEMON_PENDING_WORK);
        assert_eq!(payload["runtime_id"], "runtime-1");
        assert_eq!(payload["kind"], PENDING_WORK_KIND_MODEL_LIST);

        // Same event id must not be delivered twice — a mirrored relay can
        // publish the same hint through two paths.
        hub.deliver_daemon_runtime("runtime-1", &frame, "event-1");
        assert!(
            rx.try_recv().is_err(),
            "expected no second delivery for a duplicate event id"
        );
        assert_eq!(M.wakeup_received_total.load(Ordering::Relaxed), 2);
        assert_eq!(M.wakeup_delivered_hit.load(Ordering::Relaxed), 1);
        assert_eq!(M.wakeup_delivered_miss.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn notify_pending_work_ignores_empty_runtime() {
        let _guard = lock_metrics();
        reset_metrics();

        let hub = DaemonHub::new();
        let (_client, mut rx) = attach_client(
            &hub,
            ClientIdentity {
                runtime_ids: vec!["runtime-1".into()],
                ..Default::default()
            },
            &["runtime-1"],
            Scope::Runtime("runtime-1"),
        );

        hub.notify_pending_work("", PENDING_WORK_KIND_MODEL_LIST);
        assert!(
            rx.try_recv().is_err(),
            "expected no frame for an empty runtime id"
        );
    }

    #[test]
    fn deliver_daemon_runtime_routes_by_payload_not_shard_key() {
        let _guard = lock_metrics();
        reset_metrics();

        let hub = DaemonHub::new();
        let (_client, mut rx) = attach_client(
            &hub,
            ClientIdentity {
                runtime_ids: vec!["runtime-1".into()],
                ..Default::default()
            },
            &["runtime-1"],
            Scope::Runtime("runtime-1"),
        );

        // Shard key deliberately wrong: pending_work routes by payload
        // runtime_id, not by the relay shard key.
        let frame =
            pending_work_frame("runtime-1", PENDING_WORK_KIND_MODEL_LIST).expect("frame builds");
        hub.deliver_daemon_runtime("some-other-shard", &frame, "event-2");
        assert!(
            rx.try_recv().is_ok(),
            "payload runtime_id wins over shard key"
        );

        // Malformed payloads count as misses, never panic.
        hub.deliver_daemon_runtime("x", b"{not json", "event-3");
        hub.deliver_daemon_runtime(
            "x",
            br#"{"type":"daemon:task_available","payload":{"runtime_id":""}}"#,
            "event-4",
        );
        hub.deliver_daemon_runtime(
            "x",
            br#"{"type":"daemon:unknown_kind","payload":{}}"#,
            "event-5",
        );
        assert_eq!(M.wakeup_delivered_miss.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn register_indexes_all_scopes_and_unregister_cleans_up() {
        let hub = DaemonHub::new();
        let (client, _rx) = hub.register(ClientIdentity {
            workspace_ids: vec!["ws-1".into(), "ws-2".into()],
            runtime_ids: vec!["rt-1".into(), "rt-2".into()],
            user_id: "u-1".into(),
            ..Default::default()
        });

        assert_eq!(hub.runtime_connection_count("rt-1"), 1);
        assert_eq!(hub.runtime_connection_count("rt-2"), 1);
        assert_eq!(hub.workspace_connection_count("ws-1"), 1);
        assert_eq!(hub.workspace_connection_count("ws-2"), 1);
        assert_eq!(hub.workspace_connection_count("ws-3"), 0);
        assert_eq!(hub.user_connection_count("u-1"), 1);

        hub.unregister(client.id);
        assert_eq!(hub.runtime_connection_count("rt-1"), 0);
        assert_eq!(hub.workspace_connection_count("ws-1"), 0);
        assert_eq!(hub.workspace_connection_count("ws-2"), 0);
        assert_eq!(hub.user_connection_count("u-1"), 0);
        // Idempotent like Go's presence-guarded unregister.
        hub.unregister(client.id);
    }

    #[test]
    fn slow_clients_are_evicted_and_counted() {
        let _guard = lock_metrics();
        reset_metrics();

        let hub = DaemonHub::new();
        let (client, mut rx) = attach_client(
            &hub,
            ClientIdentity {
                runtime_ids: vec!["runtime-1".into()],
                ..Default::default()
            },
            &["runtime-1"],
            Scope::Runtime("runtime-1"),
        );

        // Fill the 2-slot test buffer, then overflow: the client is evicted as
        // slow and its connection token fires.
        for i in 0..3 {
            hub.notify_task_available_with_event(
                "runtime-1",
                &format!("task-{i}"),
                &format!("event-{i}"),
            );
        }
        assert!(rx.try_recv().is_ok(), "first buffered frame survives");
        assert_eq!(hub.runtime_connection_count("runtime-1"), 0);
        assert!(
            client.conn_cancel.is_cancelled(),
            "eviction cancels the conn"
        );
        assert_eq!(M.slow_evictions_total.load(Ordering::Relaxed), 1);
    }

    // ---- heartbeat handling --------------------------------------------------

    struct RecordingHeartbeat {
        calls: StdMutex<Vec<(String, bool)>>,
        ack: Option<DaemonHeartbeatAckPayload>,
        err: Option<anyhow::Error>,
    }

    impl RecordingHeartbeat {
        fn ok(runtime_id: &str) -> Self {
            Self {
                calls: StdMutex::new(Vec::new()),
                ack: Some(DaemonHeartbeatAckPayload {
                    runtime_id: runtime_id.to_string(),
                    status: "ok".into(),
                    server_capabilities: Vec::new(),
                    runtime_gone: false,
                    pending_update: None,
                    pending_model_list: None,
                    pending_local_skills: None,
                    pending_local_skill_import: None,
                    pending_local_skill_imports: Vec::new(),
                }),
                err: None,
            }
        }
    }

    #[async_trait]
    impl HeartbeatHandler for RecordingHeartbeat {
        async fn handle_heartbeat(
            &self,
            _identity: &ClientIdentity,
            runtime_id: &str,
            supports_batch_import: bool,
        ) -> anyhow::Result<Option<DaemonHeartbeatAckPayload>> {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((runtime_id.to_string(), supports_batch_import));
            if let Some(err) = &self.err {
                return Err(anyhow::anyhow!("{err}"));
            }
            Ok(self.ack.clone())
        }
    }

    fn heartbeat_payload(runtime_id: &str) -> Value {
        serde_json::to_value(DaemonHeartbeatRequestPayload {
            runtime_id: runtime_id.to_string(),
            supports_batch_import: false,
        })
        .expect("payload serializes")
    }

    #[tokio::test]
    async fn heartbeat_roundtrip_writes_ack_frame() {
        let hub = Arc::new(DaemonHub::new());
        let handler = Arc::new(RecordingHeartbeat::ok("runtime-1"));
        hub.set_heartbeat_handler(Some(handler.clone()));

        let (client, _rx) = hub.register(ClientIdentity {
            workspace_id: "ws-1".into(),
            runtime_ids: vec!["runtime-1".into()],
            ..Default::default()
        });

        hub.handle_heartbeat_frame(&client, &heartbeat_payload("runtime-1"))
            .await;

        let calls = handler.calls.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(calls.as_slice(), [("runtime-1".to_string(), false)]);
        drop(calls);
    }

    #[tokio::test]
    async fn heartbeat_rejects_unauthorized_runtime() {
        let hub = Arc::new(DaemonHub::new());
        let handler = Arc::new(RecordingHeartbeat::ok("runtime-1"));
        hub.set_heartbeat_handler(Some(handler.clone()));

        let (client, mut rx) = hub.register(ClientIdentity {
            runtime_ids: vec!["runtime-1".into()],
            ..Default::default()
        });

        hub.handle_heartbeat_frame(&client, &heartbeat_payload("runtime-other"))
            .await;

        assert!(
            rx.try_recv().is_err(),
            "expected no ack for unauthorized runtime"
        );
        assert!(
            handler
                .calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "handler invoked for unauthorized runtime"
        );
    }

    #[tokio::test]
    async fn heartbeat_without_handler_is_silent() {
        let hub = DaemonHub::new();
        let (client, mut rx) = attach_client(
            &hub,
            ClientIdentity {
                runtime_ids: vec!["runtime-1".into()],
                ..Default::default()
            },
            &["runtime-1"],
            Scope::Runtime("runtime-1"),
        );

        hub.handle_heartbeat_frame(&client, &heartbeat_payload("runtime-1"))
            .await;
        assert!(rx.try_recv().is_err(), "no handler wired → no ack");
    }

    // ---- RPC handling ----------------------------------------------------------

    struct EchoRpc;

    #[async_trait]
    impl RpcHandler for EchoRpc {
        async fn handle_rpc(
            &self,
            _ctx: &CancellationToken,
            identity: &ClientIdentity,
            method: &str,
            _body: Option<&Value>,
        ) -> Result<RpcOutcome, RpcHandlerError> {
            Ok(RpcOutcome {
                status: 200,
                body: Some(
                    serde_json::json!({"ok": true, "method": method, "daemon": identity.daemon_id}),
                ),
            })
        }
    }

    async fn recv_rpc_response(
        rx: &mut mpsc::Receiver<Vec<u8>>,
        request_id: &str,
    ) -> RpcResponsePayload {
        loop {
            let frame = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
                .await
                .expect("response arrives within 2s")
                .expect("queue open");
            let (r#type, payload) = decode_frame(&frame);
            assert_eq!(r#type, EVENT_DAEMON_RPC_RESPONSE);
            let resp: RpcResponsePayload = serde_json::from_value(payload).expect("resp decodes");
            if resp.request_id == request_id {
                return resp;
            }
        }
    }

    #[tokio::test]
    async fn rpc_dispatch_roundtrip_echoes_request_id() {
        let hub = Arc::new(DaemonHub::new());
        hub.set_rpc_handler(Some(Arc::new(EchoRpc)));

        let (client, mut rx) = hub.register(ClientIdentity {
            daemon_id: "daemon-1".into(),
            runtime_ids: vec!["rt-1".into()],
            ..Default::default()
        });

        hub.handle_rpc_frame(
            &client,
            &serde_json::json!({
                "request_id": "req-1",
                "method": "tasks.claim",
                "body": {"max_tasks": 3}
            }),
        )
        .await;

        let resp = recv_rpc_response(&mut rx, "req-1").await;
        assert_eq!(resp.status, 200);
        assert_eq!(
            resp.body.as_ref().and_then(|b| b.get("ok")).cloned(),
            Some(serde_json::json!(true))
        );
        assert_eq!(
            resp.body.and_then(|b| b.get("daemon").cloned()),
            Some(serde_json::json!("daemon-1"))
        );
        assert_eq!(resp.error, "");
    }

    #[tokio::test]
    async fn rpc_dispatch_no_handler_returns_503() {
        let hub = Arc::new(DaemonHub::new());
        let (client, mut rx) = attach_client(
            &hub,
            ClientIdentity {
                daemon_id: "daemon-1".into(),
                runtime_ids: vec!["rt-1".into()],
                ..Default::default()
            },
            &["rt-1"],
            Scope::Runtime("rt-1"),
        );

        hub.handle_rpc_frame(
            &client,
            &serde_json::json!({"request_id": "req-3", "method": "tasks.claim"}),
        )
        .await;

        let resp = recv_rpc_response(&mut rx, "req-3").await;
        assert_eq!(resp.status, HTTP_STATUS_SERVICE_UNAVAILABLE);
        assert_eq!(resp.error, "rpc handler unavailable");
    }

    #[tokio::test]
    async fn rpc_dispatch_missing_request_id_is_dropped() {
        let hub = Arc::new(DaemonHub::new());
        let (client, mut rx) = attach_client(
            &hub,
            ClientIdentity {
                daemon_id: "daemon-1".into(),
                runtime_ids: vec!["rt-1".into()],
                ..Default::default()
            },
            &["rt-1"],
            Scope::Runtime("rt-1"),
        );

        hub.handle_rpc_frame(
            &client,
            &serde_json::json!({"request_id": "", "method": "tasks.claim"}),
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(rx.try_recv().is_err(), "missing request_id → no response");
    }

    #[tokio::test]
    async fn rpc_dispatch_handler_error_maps_to_5xx() {
        struct FailingRpc;
        #[async_trait]
        impl RpcHandler for FailingRpc {
            async fn handle_rpc(
                &self,
                _ctx: &CancellationToken,
                _identity: &ClientIdentity,
                _method: &str,
                _body: Option<&Value>,
            ) -> Result<RpcOutcome, RpcHandlerError> {
                Err(RpcHandlerError::new(
                    0,
                    anyhow::anyhow!("context deadline exceeded"),
                ))
            }
        }

        let hub = Arc::new(DaemonHub::new());
        hub.set_rpc_handler(Some(Arc::new(FailingRpc)));
        let (client, mut rx) = attach_client(
            &hub,
            ClientIdentity {
                daemon_id: "daemon-1".into(),
                runtime_ids: vec!["rt-1".into()],
                ..Default::default()
            },
            &["rt-1"],
            Scope::Runtime("rt-1"),
        );

        hub.handle_rpc_frame(
            &client,
            &serde_json::json!({"request_id": "req-2", "method": "tasks.claim"}),
        )
        .await;

        let resp = recv_rpc_response(&mut rx, "req-2").await;
        assert!(resp.status >= 400, "status = {}, want 5xx", resp.status);
        assert_eq!(resp.error, "context deadline exceeded");
    }

    #[tokio::test]
    async fn rpc_dispatch_saturated_semaphore_returns_429() {
        let hub = Arc::new(DaemonHub::new());
        hub.set_rpc_handler(Some(Arc::new(EchoRpc)));

        let (client, mut rx) = attach_client(
            &hub,
            ClientIdentity {
                daemon_id: "daemon-1".into(),
                runtime_ids: vec!["rt-1".into()],
                ..Default::default()
            },
            &["rt-1"],
            Scope::Runtime("rt-1"),
        );

        // Hold every permit so the connection is saturated and the hub must
        // take the 429 fallback path.
        let _held: Vec<tokio::sync::OwnedSemaphorePermit> = (0..MAX_IN_FLIGHT_RPC_PER_CLIENT)
            .map(|_| {
                client
                    .rpc_permits
                    .clone()
                    .try_acquire_owned()
                    .expect("permit")
            })
            .collect();
        assert_eq!(client.rpc_permits.available_permits(), 0);

        hub.handle_rpc_frame(
            &client,
            &serde_json::json!({"request_id": "req-full", "method": "tasks.claim"}),
        )
        .await;

        let resp = recv_rpc_response(&mut rx, "req-full").await;
        assert_eq!(resp.status, HTTP_STATUS_TOO_MANY_REQUESTS);
        assert_eq!(resp.error, "too many in-flight rpc requests");
    }

    #[tokio::test]
    async fn rpc_dispatch_server_timeout_cancels_handler() {
        struct SlowRpc;
        #[async_trait]
        impl RpcHandler for SlowRpc {
            async fn handle_rpc(
                &self,
                ctx: &CancellationToken,
                _identity: &ClientIdentity,
                _method: &str,
                _body: Option<&Value>,
            ) -> Result<RpcOutcome, RpcHandlerError> {
                // Park until cancelled (connection teardown or TimeoutMs).
                ctx.cancelled().await;
                Err(RpcHandlerError::new(0, anyhow::anyhow!("context canceled")))
            }
        }

        let hub = Arc::new(DaemonHub::new());
        hub.set_rpc_handler(Some(Arc::new(SlowRpc)));
        let (client, mut rx) = attach_client(
            &hub,
            ClientIdentity {
                daemon_id: "daemon-1".into(),
                runtime_ids: vec!["rt-1".into()],
                ..Default::default()
            },
            &["rt-1"],
            Scope::Runtime("rt-1"),
        );

        hub.handle_rpc_frame(
            &client,
            &serde_json::json!({
                "request_id": "req-timeout",
                "method": "tasks.claim",
                "timeout_ms": 50
            }),
        )
        .await;

        let resp = recv_rpc_response(&mut rx, "req-timeout").await;
        assert!(resp.status >= 400);
        assert_eq!(resp.error, "context deadline exceeded");
    }

    #[tokio::test]
    async fn rpc_disconnect_during_handler_does_not_panic() {
        struct GatedRpc {
            release: Arc<tokio::sync::Notify>,
        }
        #[async_trait]
        impl RpcHandler for GatedRpc {
            async fn handle_rpc(
                &self,
                _ctx: &CancellationToken,
                _identity: &ClientIdentity,
                _method: &str,
                _body: Option<&Value>,
            ) -> Result<RpcOutcome, RpcHandlerError> {
                self.release.notified().await;
                Ok(RpcOutcome {
                    status: 200,
                    body: Some(serde_json::json!({"ok": true})),
                })
            }
        }

        let hub = Arc::new(DaemonHub::new());
        let release = Arc::new(tokio::sync::Notify::new());
        hub.set_rpc_handler(Some(Arc::new(GatedRpc {
            release: release.clone(),
        })));

        let (client, rx) = hub.register(ClientIdentity {
            daemon_id: "daemon-1".into(),
            runtime_ids: vec!["rt-1".into()],
            ..Default::default()
        });

        hub.handle_rpc_frame(
            &client,
            &serde_json::json!({"request_id": "req-1", "method": "tasks.claim"}),
        )
        .await;

        // Simulate the client disconnecting before the handler returns: the
        // hub unregisters (token cancelled) and the pump drops its receiver.
        hub.unregister(client.id);
        drop(rx);
        release.notify_waiters();

        // Give the handler task time to attempt its late response; passing ==
        // no panic on a closed queue (try_send returns false instead).
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // ---- message-kind recording ---------------------------------------------

    struct KindLog(StdMutex<Vec<String>>);

    impl MessageKindRecorder for KindLog {
        fn record_daemon_ws_message_received(&self, kind: &str) {
            self.0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(kind.to_string());
        }
    }

    #[tokio::test]
    async fn handle_frame_records_kinds_and_dispatches() {
        let hub = Arc::new(DaemonHub::new());
        let log = Arc::new(KindLog(StdMutex::new(Vec::new())));
        hub.set_message_kind_recorder(Some(log.clone()));

        let (client, _rx) = hub.register(ClientIdentity {
            runtime_ids: vec!["rt-1".into()],
            ..Default::default()
        });

        let hb = serde_json::to_vec(&Message {
            r#type: EVENT_DAEMON_HEARTBEAT.to_string(),
            payload: heartbeat_payload("rt-1"),
        })
        .expect("frame builds");
        hub.handle_frame(&client, &hb).await;

        let unknown = serde_json::to_vec(&Message {
            r#type: "daemon:".to_string(),
            payload: serde_json::json!({}),
        })
        .expect("frame builds");
        hub.handle_frame(&client, &unknown).await;

        let unprefixed = serde_json::to_vec(&Message {
            r#type: "issue:created".to_string(),
            payload: serde_json::json!({}),
        })
        .expect("frame builds");
        hub.handle_frame(&client, &unprefixed).await;

        hub.handle_frame(&client, b"{broken").await;

        let kinds = log.0.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            kinds.as_slice(),
            ["heartbeat", "unknown", "issue:created", "invalid"]
        );
    }

    // ---- metrics snapshot shape ----------------------------------------------

    #[test]
    fn metrics_snapshot_keys_match_go() {
        let m = Metrics::new();
        m.connects_total.store(1, Ordering::Relaxed);
        m.wakeup_delivered_hit.store(2, Ordering::Relaxed);
        let snap = m.snapshot();
        assert_eq!(snap["connects_total"], 1);
        assert_eq!(snap["disconnects_total"], 0);
        assert_eq!(snap["active_connections"], 0);
        assert_eq!(snap["slow_evictions_total"], 0);
        assert_eq!(snap["wakeup_published_total"], 0);
        assert_eq!(snap["wakeup_publish_errors"], 0);
        assert_eq!(snap["wakeup_received_total"], 0);
        assert_eq!(snap["wakeup_delivered_hit_total"], 2);
        assert_eq!(snap["wakeup_delivered_miss_total"], 0);
        m.reset();
        assert_eq!(m.snapshot()["connects_total"], 0);
    }

    // ---- local/Redis loopback dedup (RelayNotifier round-trips) ---------------
    // Ports TestRelayNotifierDedups{LocalRedisLoopback,RuntimeProfilesChangedLoopback,
    // WorkspacesChangedLoopback} from hub_test.go: the notifier delivers locally
    // first, publishes to the relay with the same event id, and the loopback
    // delivery through deliver_daemon_runtime must be deduped.

    /// Port of localFirstDaemonRelayPublisher: records the publish and asserts
    /// the local fanout already queued a frame before the relay publish ran.
    struct LocalFirstRelayPublisher {
        rx: Arc<StdMutex<mpsc::Receiver<Vec<u8>>>>,
        called: StdMutex<bool>,
        record: StdMutex<Option<(String, String, Vec<u8>, String)>>,
    }

    #[async_trait]
    impl cordy_realtime::RelayPublisher for LocalFirstRelayPublisher {
        async fn publish_with_id(
            &self,
            scope_type: &str,
            scope_id: &str,
            _exclude: &str,
            frame: &[u8],
            event_id: &str,
        ) -> anyhow::Result<()> {
            *self.called.lock().unwrap_or_else(|e| e.into_inner()) = true;
            *self.record.lock().unwrap_or_else(|e| e.into_inner()) = Some((
                scope_type.to_string(),
                scope_id.to_string(),
                frame.to_vec(),
                event_id.to_string(),
            ));

            // Local fanout must happen before relay publish.
            let mut rx = self.rx.lock().unwrap_or_else(|e| e.into_inner());
            let local = rx
                .try_recv()
                .expect("expected local fanout to happen before relay publish");
            let _ = local;
            Ok(())
        }
    }

    fn attach_loopback_relay(
        client: &Arc<DaemonClient>,
        rx: mpsc::Receiver<Vec<u8>>,
    ) -> (
        Arc<StdMutex<mpsc::Receiver<Vec<u8>>>>,
        Arc<LocalFirstRelayPublisher>,
    ) {
        let rx = Arc::new(StdMutex::new(rx));
        let relay = Arc::new(LocalFirstRelayPublisher {
            rx: rx.clone(),
            called: StdMutex::new(false),
            record: StdMutex::new(None),
        });
        let _ = client;
        (rx, relay)
    }

    #[tokio::test]
    async fn relay_notifier_dedups_local_redis_loopback() {
        let _guard = lock_metrics();
        reset_metrics();

        let hub = Arc::new(DaemonHub::new());
        let (_client, rx) = attach_client(
            &hub,
            ClientIdentity::default(),
            &["runtime-1"],
            Scope::Runtime("runtime-1"),
        );
        let (rx, relay) = attach_loopback_relay(&_client, rx);
        let notifier = crate::notifier::RelayNotifier::new(Some(hub.clone()), Some(relay.clone()));

        notifier.notify_task_available("runtime-1", "task-1").await;

        assert!(
            *relay.called.lock().unwrap_or_else(|e| e.into_inner()),
            "expected relay publish to be invoked"
        );
        let (_, scope_id, frame, event_id) = relay
            .record
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("publish recorded");
        assert!(!event_id.is_empty(), "expected event id");
        assert_eq!(
            M.wakeup_delivered_hit.load(Ordering::Relaxed),
            1,
            "local delivery counts one hit"
        );

        hub.deliver_daemon_runtime(&scope_id, &frame, &event_id);

        assert!(
            rx.lock()
                .unwrap_or_else(|e| e.into_inner())
                .try_recv()
                .is_err(),
            "expected redis loopback to be deduped"
        );
        assert_eq!(M.wakeup_delivered_hit.load(Ordering::Relaxed), 1);
        assert_eq!(M.wakeup_delivered_miss.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn relay_notifier_dedups_runtime_profiles_changed_loopback() {
        let _guard = lock_metrics();
        reset_metrics();

        let hub = Arc::new(DaemonHub::new());
        let (_client, rx) = attach_client(
            &hub,
            ClientIdentity {
                workspace_ids: vec!["ws-1".into()],
                ..Default::default()
            },
            &[],
            Scope::Workspace("ws-1"),
        );
        let (rx, relay) = attach_loopback_relay(&_client, rx);
        let notifier = crate::notifier::RelayNotifier::new(Some(hub.clone()), Some(relay.clone()));

        notifier
            .notify_runtime_profiles_changed("ws-1", "profile-1")
            .await;

        assert!(
            *relay.called.lock().unwrap_or_else(|e| e.into_inner()),
            "expected relay publish to be invoked"
        );
        let (_, scope_id, frame, event_id) = relay
            .record
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("publish recorded");
        assert!(!event_id.is_empty());
        // Workspace-scope fanout does not count hits (matches Go).
        assert_eq!(
            M.wakeup_delivered_hit.load(Ordering::Relaxed),
            0,
            "delivered hit metric = 0 before redis relay delivery"
        );

        hub.deliver_daemon_runtime(&scope_id, &frame, &event_id);

        assert!(
            rx.lock()
                .unwrap_or_else(|e| e.into_inner())
                .try_recv()
                .is_err(),
            "expected redis loopback to be deduped"
        );
        assert_eq!(M.wakeup_delivered_hit.load(Ordering::Relaxed), 0);
        assert_eq!(M.wakeup_delivered_miss.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn relay_notifier_dedups_workspaces_changed_loopback() {
        let _guard = lock_metrics();
        reset_metrics();

        let hub = Arc::new(DaemonHub::new());
        let (_client, rx) = attach_client(
            &hub,
            ClientIdentity {
                user_id: "user-1".into(),
                ..Default::default()
            },
            &[],
            Scope::User("user-1"),
        );
        let (rx, relay) = attach_loopback_relay(&_client, rx);
        let notifier = crate::notifier::RelayNotifier::new(Some(hub.clone()), Some(relay.clone()));

        notifier.notify_workspaces_changed("user-1").await;
        assert!(
            *relay.called.lock().unwrap_or_else(|e| e.into_inner()),
            "expected local delivery followed by relay publish"
        );
        let (_, scope_id, frame, event_id) = relay
            .record
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("publish recorded");

        hub.deliver_daemon_runtime(&scope_id, &frame, &event_id);
        assert!(
            rx.lock()
                .unwrap_or_else(|e| e.into_inner())
                .try_recv()
                .is_err(),
            "expected redis loopback to be deduped"
        );
    }
}
