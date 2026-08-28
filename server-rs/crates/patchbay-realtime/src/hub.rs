//! WebSocket hub — port of the core state management and fanout logic in
//!
//! Port notes vs Go:
//! - Go serialises mutations through a single Run() goroutine fed by
//!   register/unregister/broadcast channels; here plain locks give the same
//!   happens-before guarantees without a dedicated loop.
//! - Go uses `*Client` pointers as map keys; we use monotonically increasing
//!   [`ClientId`]s.
//! - The WS read/write pumps and the upgrade/auth handshake are HTTP-layer
//!   concerns — they land with the axum handler port (S8).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use serde_json::json;

use crate::broadcaster::{Broadcaster, SCOPE_USER, SCOPE_WORKSPACE};
use crate::metrics::M;

/// A scope currently active on this node.
pub use crate::redis_relay::ScopeKey;

/// Unique per-connection identifier (`*Client` pointer equivalent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(pub u64);

/// Dedup cache with bounded LRU semantics (capacity 128, matching Go).
/// Event IDs are ULIDs so only the last few need tracking.
#[derive(Default)]
struct DedupCache {
    seen: HashSet<String>,
    order: VecDeque<String>,
}

const DEDUP_CAPACITY: usize = 128;

impl DedupCache {
    /// Records eventID as already delivered. Returns true when first seen
    /// (caller should deliver), false when duplicate (caller should drop).
    fn mark_seen(&mut self, event_id: &str) -> bool {
        if event_id.is_empty() {
            return true;
        }
        if !self.seen.insert(event_id.to_string()) {
            return false;
        }
        self.order.push_back(event_id.to_string());
        if self.order.len() > DEDUP_CAPACITY {
            if let Some(drop) = self.order.pop_front() {
                self.seen.remove(&drop);
            }
        }
        true
    }
}

/// Per-connection state owned by the hub. The WS pump half (sender consumer)
/// lives in the handler layer and consumes from `sender`.
pub struct ClientHandle {
    pub id: ClientId,
    pub user_id: String,
    pub workspace_id: String,
    /// Outbound frame queue consumed by the connection's write task.
    pub sender: tokio::sync::mpsc::Sender<Vec<u8>>,
    dedup: Mutex<DedupCache>,
}

impl ClientHandle {
    /// Records eventID as delivered; false means duplicate (drop it).
    fn mark_seen(&self, event_id: &str) -> bool {
        self.dedup
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mark_seen(event_id)
    }
}

/// Decides whether a connection may subscribe to a scope. Implementations
/// typically perform a DB lookup on the underlying resource (task / chat
/// session) and verify it belongs to the workspace. Positive results should
/// be cached to avoid hot-path DB load.
pub trait ScopeAuthorizer: Send + Sync {
    fn authorize_scope(
        &self,
        user_id: &str,
        workspace_id: &str,
        scope_type: &str,
        scope_id: &str,
    ) -> anyhow::Result<bool>;
}

/// Resolves a Personal Access Token to a user ID.
pub trait PatResolver: Send + Sync {
    fn resolve_token(&self, token: &str) -> Option<String>;
}

/// Verifies a user belongs to a workspace.
pub trait MembershipChecker: Send + Sync {
    fn is_member(&self, user_id: &str, workspace_id: &str) -> bool;
}

#[derive(Default)]
struct HubInner {
    rooms: HashMap<ScopeKey, HashSet<ClientId>>,
    clients: HashMap<ClientId, Arc<ClientHandle>>,
    /// Per-client scope subscriptions — guarded by the hub write lock,
    /// mirroring Go's "subscriptions is guarded by hub.mu" contract.
    subscriptions: HashMap<ClientId, HashSet<ScopeKey>>,
    on_first_subscriber: Option<crate::redis_relay::ScopeEventCallback>,
    on_last_subscriber: Option<crate::redis_relay::ScopeEventCallback>,
}

/// Manages WebSocket connections organized into scope-based rooms.
pub struct Hub {
    inner: RwLock<HubInner>,
    authorizer: RwLock<Option<Arc<dyn ScopeAuthorizer>>>,
    next_client_id: AtomicU64,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    /// Creates a new Hub instance.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HubInner::default()),
            authorizer: RwLock::new(None),
            next_client_id: AtomicU64::new(1),
        }
    }

    /// Wires a [`ScopeAuthorizer`] into the hub. Safe to call before run.
    pub fn set_authorizer(&self, a: Arc<dyn ScopeAuthorizer>) {
        *self.authorizer.write().unwrap_or_else(|e| e.into_inner()) = Some(a);
    }

    fn alloc_client_id(&self) -> ClientId {
        ClientId(self.next_client_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Registers a connected client and auto-subscribes it to its workspace
    /// and user scopes. Returns the assigned id plus the outbound queue
    /// consumer for the connection's write task.
    pub fn register(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> (ClientId, tokio::sync::mpsc::Receiver<Vec<u8>>) {
        let id = self.alloc_client_id();
        let (tx, rx) = tokio::sync::mpsc::channel(256);

        let handle = Arc::new(ClientHandle {
            id,
            user_id: user_id.to_string(),
            workspace_id: workspace_id.to_string(),
            sender: tx,
            dedup: Mutex::new(DedupCache::default()),
        });

        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        inner.clients.insert(id, handle.clone());
        inner.subscriptions.insert(id, HashSet::new());
        let total = inner.clients.len();

        // Auto-subscribe to the workspace and user scopes.
        drop(inner);
        if !workspace_id.is_empty() {
            self.subscribe(&handle, SCOPE_WORKSPACE, workspace_id);
        }
        if !user_id.is_empty() {
            self.subscribe(&handle, SCOPE_USER, user_id);
        }

        M.connects_total.fetch_add(1, Ordering::Relaxed);
        M.active_connections.fetch_add(1, Ordering::Relaxed);
        tracing::info!(
            workspace_id = %workspace_id,
            user_id = %user_id,
            total_clients = total,
            "ws client connected"
        );
        (id, rx)
    }

    /// [`Hub::register`] variant that also returns the client handle — the WS
    /// pump layer needs it to dispatch subscribe frames without a hub lookup.
    pub fn register_with_handle(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> (
        ClientId,
        tokio::sync::mpsc::Receiver<Vec<u8>>,
        Option<std::sync::Arc<ClientHandle>>,
    ) {
        let (id, rx) = self.register(user_id, workspace_id);
        let handle = {
            let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
            inner.clients.get(&id).cloned()
        };
        (id, rx, handle)
    }

    /// Drops a client from all rooms and the global set, firing
    /// on-last-subscriber callbacks for any rooms drained as a side effect.
    pub fn unregister(&self, id: ClientId) {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        // Unknown id (already removed by eviction) — nothing to do.
        let Some(handle) = inner.clients.remove(&id) else {
            return;
        };

        let mut drained = Vec::new();
        if let Some(subs) = inner.subscriptions.remove(&id) {
            for key in &subs {
                if let Some(room) = inner.rooms.get_mut(key) {
                    room.remove(&id);
                    if room.is_empty() {
                        inner.rooms.remove(key);
                        drained.push(key.clone());
                    }
                }
            }
        }
        let cb = inner.on_last_subscriber.clone();
        let total = inner.clients.len();
        drop(inner);

        M.disconnects_total.fetch_add(1, Ordering::Relaxed);
        M.active_connections.fetch_add(-1, Ordering::Relaxed);
        if let Some(cb) = cb {
            for key in &drained {
                cb(&key.scope_type, &key.scope_id);
            }
        }
        for key in &drained {
            M.dec_room(&key.scope_type);
        }
        tracing::info!(
            workspace_id = %handle.workspace_id,
            user_id = %handle.user_id,
            total_clients = total,
            "ws client disconnected"
        );
    }

    /// Adds a client to a scope room, firing on-first-subscriber when the
    /// room transitions from empty to non-empty. Returns true if newly added.
    pub fn subscribe(&self, client: &Arc<ClientHandle>, scope_type: &str, scope_id: &str) -> bool {
        if scope_type.is_empty() || scope_id.is_empty() {
            return false;
        }
        let key = ScopeKey::new(scope_type, scope_id);

        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        // Only connected clients may subscribe.
        if !inner.clients.contains_key(&client.id) {
            return false;
        }
        let subs = inner.subscriptions.entry(client.id).or_default();
        if !subs.insert(key.clone()) {
            return false;
        }
        let room = inner.rooms.entry(key.clone()).or_default();
        let first = room.is_empty();
        room.insert(client.id);
        let cb = inner.on_first_subscriber.clone();
        drop(inner);

        M.subscribes_total(scope_type)
            .fetch_add(1, Ordering::Relaxed);
        if first {
            M.inc_room(scope_type);
            if let Some(cb) = cb {
                cb(scope_type, scope_id);
            }
        }
        true
    }

    /// Removes a client from a scope room, firing on-last-subscriber when the
    /// room becomes empty. Returns true if the subscription existed.
    pub fn unsubscribe(
        &self,
        client: &Arc<ClientHandle>,
        scope_type: &str,
        scope_id: &str,
    ) -> bool {
        if scope_type.is_empty() || scope_id.is_empty() {
            return false;
        }
        let key = ScopeKey::new(scope_type, scope_id);

        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if !inner.clients.contains_key(&client.id) {
            return false;
        }
        let removed = match inner.subscriptions.get_mut(&client.id) {
            Some(subs) => subs.remove(&key),
            None => false,
        };
        if !removed {
            return false;
        }
        let mut emptied = false;
        if let Some(room) = inner.rooms.get_mut(&key) {
            room.remove(&client.id);
            if room.is_empty() {
                inner.rooms.remove(&key);
                emptied = true;
            }
        }
        let cb = inner.on_last_subscriber.clone();
        drop(inner);

        M.unsubscribes_total(scope_type)
            .fetch_add(1, Ordering::Relaxed);
        if emptied {
            M.dec_room(scope_type);
            if let Some(cb) = cb {
                cb(scope_type, scope_id);
            }
        }
        true
    }

    /// Reports whether at least one local client subscribes to the scope.
    /// Used by relays to decide whether to keep a per-scope consumer running.
    pub fn has_local_subscribers(&self, scope_type: &str, scope_id: &str) -> bool {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner
            .rooms
            .contains_key(&ScopeKey::new(scope_type, scope_id))
    }

    /// Sends a message to every client subscribed to the scope. Slow clients
    /// (full queues) are evicted under the write lock.
    pub fn broadcast_to_scope_dedup(
        &self,
        scope_type: &str,
        scope_id: &str,
        message: &[u8],
        event_id: &str,
    ) {
        if scope_type.is_empty() || scope_id.is_empty() {
            return;
        }
        let key = ScopeKey::new(scope_type, scope_id);

        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let Some(room) = inner.rooms.get(&key) else {
            return;
        };
        let mut slow = Vec::new();
        let mut sent: u64 = 0;
        for client_id in room {
            let Some(client) = inner.clients.get(client_id) else {
                continue;
            };
            if !client.mark_seen(event_id) {
                continue;
            }
            match client.sender.try_send(message.to_vec()) {
                Ok(()) => sent += 1,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    slow.push(client.clone());
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    // Pump already gone; eviction sweep will reap it.
                    slow.push(client.clone());
                }
            }
        }
        drop(inner);

        if sent > 0 {
            M.messages_sent_total
                .fetch_add(sent as i64, Ordering::Relaxed);
        }
        if !slow.is_empty() {
            self.evict_slow(&slow);
        }
    }

    /// Delivers to every connected client. Non-empty `exclude_workspace`
    /// skips clients whose workspace matches (member:added dedup semantics).
    /// `event_id` is the dedup key (empty disables dedup).
    pub fn fanout_all_dedup(&self, message: &[u8], exclude_workspace: &str, event_id: &str) {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let mut slow = Vec::new();
        let mut sent: u64 = 0;
        for client in inner.clients.values() {
            if !exclude_workspace.is_empty() && client.workspace_id == exclude_workspace {
                continue;
            }
            if !client.mark_seen(event_id) {
                continue;
            }
            match client.sender.try_send(message.to_vec()) {
                Ok(()) => sent += 1,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    slow.push(client.clone());
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    slow.push(client.clone());
                }
            }
        }
        drop(inner);

        if sent > 0 {
            M.messages_sent_total
                .fetch_add(sent as i64, Ordering::Relaxed);
        }
        if !slow.is_empty() {
            self.evict_slow(&slow);
        }
    }

    /// Delivers a message to all connections of `user_id`, optionally skipping
    /// connections whose workspace matches `exclude_workspace`, deduped
    /// against `event_id`.
    pub fn fanout_user(
        &self,
        user_id: &str,
        message: &[u8],
        exclude_workspace: &str,
        event_id: &str,
    ) {
        let key = ScopeKey::new(SCOPE_USER, user_id);
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let Some(room) = inner.rooms.get(&key) else {
            return;
        };
        let mut slow = Vec::new();
        let mut sent: u64 = 0;
        for client_id in room {
            let Some(client) = inner.clients.get(client_id) else {
                continue;
            };
            if !exclude_workspace.is_empty() && client.workspace_id == exclude_workspace {
                continue;
            }
            if !client.mark_seen(event_id) {
                continue;
            }
            match client.sender.try_send(message.to_vec()) {
                Ok(()) => sent += 1,
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    slow.push(client.clone());
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    slow.push(client.clone());
                }
            }
        }
        drop(inner);
        if sent > 0 {
            M.messages_sent_total
                .fetch_add(sent as i64, Ordering::Relaxed);
        }
        if !slow.is_empty() {
            self.evict_slow(&slow);
        }
    }

    /// Removes clients whose send queue was full. Closes their queue, removes
    /// them from every room, fires on-last-subscriber for drained rooms.
    fn evict_slow(&self, slow: &[Arc<ClientHandle>]) {
        M.messages_dropped_total
            .fetch_add(slow.len() as i64, Ordering::Relaxed);
        M.slow_evictions_total
            .fetch_add(slow.len() as i64, Ordering::Relaxed);

        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        let mut evicted = 0usize;
        let mut drained_rooms: Vec<ScopeKey> = Vec::new();
        for c in slow {
            if !inner.clients.contains_key(&c.id) {
                continue;
            }
            inner.clients.remove(&c.id);
            // Clone the subscription set first: iterating a borrow of
            // `inner.subscriptions` while mutating `inner.rooms` would be two
            // overlapping borrows of the same guard.
            let subs: Vec<ScopeKey> = inner
                .subscriptions
                .get(&c.id)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();
            for key in &subs {
                if let Some(room) = inner.rooms.get_mut(key) {
                    room.remove(&c.id);
                    if room.is_empty() {
                        inner.rooms.remove(key);
                        drained_rooms.push(key.clone());
                    }
                }
            }
            evicted += 1;
        }
        let cb = inner.on_last_subscriber.clone();
        drop(inner);

        if evicted > 0 {
            M.active_connections
                .fetch_add(-(evicted as i64), Ordering::Relaxed);
            M.disconnects_total
                .fetch_add(evicted as i64, Ordering::Relaxed);
        }
        for r in &drained_rooms {
            M.dec_room(&r.scope_type);
        }
        if let Some(cb) = cb {
            for r in &drained_rooms {
                cb(&r.scope_type, &r.scope_id);
            }
        }
    }

    /// Authorizes a scope subscription using the wired [`ScopeAuthorizer`].
    /// Returns an error reason string mirroring Go's deny payloads.
    pub fn authorize_subscription(
        &self,
        client: &ClientHandle,
        scope_type: &str,
        scope_id: &str,
    ) -> Result<(), &'static str> {
        let authorizer = self.authorizer.read().unwrap_or_else(|e| e.into_inner());
        match authorizer.as_ref() {
            // Task/chat scopes always require an ownership lookup. Treat
            // missing wiring as a server-side authorization failure instead
            // of granting every authenticated workspace member access.
            None => Err("lookup_failed"),
            Some(a) => {
                match a.authorize_scope(&client.user_id, &client.workspace_id, scope_type, scope_id)
                {
                    Ok(true) => Ok(()),
                    Ok(false) => Err("forbidden"),
                    Err(_) => Err("lookup_failed"),
                }
            }
        }
    }

    /// JSON-friendly summary of the hub state (`{"connections": N, "rooms":
    /// {scope_type: count}}`).
    pub fn snapshot(&self) -> serde_json::Value {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let mut rooms: HashMap<&str, i64> = HashMap::new();
        for key in inner.rooms.keys() {
            *rooms.entry(key.scope_type.as_str()).or_default() += 1;
        }
        json!({
            "connections": inner.clients.len(),
            "rooms": rooms,
        })
    }
}

// ---- trait integrations -------------------------------------------------

#[async_trait]
impl Broadcaster for Hub {
    async fn broadcast_to_scope(&self, scope_type: &str, scope_id: &str, message: &[u8]) {
        self.broadcast_to_scope_dedup(scope_type, scope_id, message, "");
    }

    async fn send_to_user(&self, user_id: &str, message: &[u8], exclude_workspace: Option<&str>) {
        self.fanout_user(user_id, message, exclude_workspace.unwrap_or_default(), "");
    }

    async fn broadcast(&self, message: &[u8]) {
        self.fanout_all_dedup(message, "", "");
    }
}

#[async_trait]
impl crate::envelope::HubFanout for Hub {
    async fn fanout_all_dedup(&self, frame: &[u8], exclude_workspace: &str, event_id: &str) {
        self.fanout_all_dedup(frame, exclude_workspace, event_id);
    }

    async fn fanout_user(
        &self,
        user_id: &str,
        frame: &[u8],
        exclude_workspace: &str,
        event_id: &str,
    ) {
        self.fanout_user(user_id, frame, exclude_workspace, event_id);
    }

    async fn broadcast_to_scope_dedup(
        &self,
        scope_type: &str,
        scope_id: &str,
        frame: &[u8],
        event_id: &str,
    ) {
        self.broadcast_to_scope_dedup(scope_type, scope_id, frame, event_id);
    }
}

impl crate::redis_relay::ScopeSubscriptionSource for Hub {
    fn local_scopes(&self) -> Vec<ScopeKey> {
        let inner = self.inner.read().unwrap_or_else(|e| e.into_inner());
        inner.rooms.keys().cloned().collect()
    }

    fn set_subscription_callbacks(
        &self,
        on_first: crate::redis_relay::ScopeEventCallback,
        on_last: crate::redis_relay::ScopeEventCallback,
    ) {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        inner.on_first_subscriber = Some(on_first);
        inner.on_last_subscriber = Some(on_last);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broadcaster::SCOPE_TASK;
    use std::sync::Mutex;

    fn shared_log() -> Arc<Mutex<Vec<&'static str>>> {
        Arc::new(Mutex::new(Vec::new()))
    }

    /// The receiver MUST stay bound for the sender to accept frames —
    /// dropping it closes the queue and every broadcast would evict the
    /// client as "slow".
    fn make_client(
        hub: &Hub,
        user: &str,
        ws: &str,
    ) -> (
        Arc<ClientHandle>,
        ClientId,
        tokio::sync::mpsc::Receiver<Vec<u8>>,
    ) {
        let (id, rx) = hub.register(user, ws);
        let inner = hub.inner.read().unwrap();
        (inner.clients[&id].clone(), id, rx)
    }

    #[test]
    fn register_auto_subscribes_workspace_and_user() {
        let hub = Hub::new();
        let (_client, _, _rx) = make_client(&hub, "u-1", "ws-1");

        assert!(hub.has_local_subscribers(SCOPE_WORKSPACE, "ws-1"));
        assert!(hub.has_local_subscribers(SCOPE_USER, "u-1"));
        assert!(hub.has_local_subscribers(SCOPE_WORKSPACE, "ws-1"));
    }

    #[test]
    fn unsubscribe_removes_last_subscriber_room_index() {
        let hub = Hub::new();
        let (client, _, _rx) = make_client(&hub, "u-1", "ws-1");

        assert!(hub.subscribe(&client, SCOPE_TASK, "task-1"));
        assert!(hub.has_local_subscribers(SCOPE_TASK, "task-1"));
        assert!(hub.unsubscribe(&client, SCOPE_TASK, "task-1"));
        assert!(!hub.has_local_subscribers(SCOPE_TASK, "task-1"));
        assert!(!hub.unsubscribe(&client, SCOPE_TASK, "task-1"));
    }

    #[test]
    fn first_and_last_callbacks_fire_on_room_boundaries() {
        let hub = Hub::new();
        let log = shared_log();

        {
            let mut inner = hub.inner.write().unwrap();
            let l = log.clone();
            inner.on_first_subscriber = Some(Arc::new(move |_t: &str, _i: &str| {
                l.lock().unwrap().push("first")
            }));
            let l = log.clone();
            inner.on_last_subscriber = Some(Arc::new(move |_t: &str, _i: &str| {
                l.lock().unwrap().push("last")
            }));
        }

        let (client_a, id_a, _rx_a) = make_client(&hub, "u-1", "ws-1");
        // Room already auto-subscribed at register; second join adds no event.
        let (client_b, id_b, _rx_b) = make_client(&hub, "u-2", "ws-1");

        hub.unregister(id_a);
        hub.unregister(id_b);

        let events = log.lock().unwrap();
        assert_eq!(events.first(), Some(&"first"));
        assert_eq!(events.last(), Some(&"last"));
        let _ = (client_a, client_b);
    }

    #[test]
    fn broadcast_to_scope_reaches_only_room_members() {
        let hub = Hub::new();
        let (_a, id_a, _rx_a) = make_client(&hub, "u-1", "ws-1");
        let (_b, id_b, _rx_b) = make_client(&hub, "u-2", "ws-2");

        hub.broadcast_to_scope_dedup(SCOPE_WORKSPACE, "ws-1", b"hello", "");

        // A (ws-1 member) got the frame; B (ws-2 non-member) did not —
        // verified via send-queue capacity consumption.
        let inner = hub.inner.read().unwrap();
        assert_eq!(inner.clients[&id_a].sender.capacity(), 255);
        assert_eq!(inner.clients[&id_b].sender.capacity(), 256);
    }

    #[test]
    fn mark_seen_lru_capacity_and_duplicates() {
        let mut cache = DedupCache::default();
        assert!(cache.mark_seen("")); // empty id always delivers
        for i in 0..DEDUP_CAPACITY {
            assert!(cache.mark_seen(&format!("e{i}")));
        }
        // Duplicate within window.
        assert!(!cache.mark_seen("e0"));
        // One more pushes e0 out of the LRU window.
        assert!(cache.mark_seen("overflow"));
        assert!(cache.mark_seen("e0"), "e0 was evicted from the LRU window");
    }

    #[tokio::test]
    async fn fanout_user_excludes_workspace_and_delivers() {
        let hub = Arc::new(Hub::new());
        let (_h1, id1, _rx1) = make_client(&hub, "u-1", "ws-keep");
        let (_h2, id2, _rx2) = make_client(&hub, "u-1", "ws-skip");

        hub.fanout_user("u-1", b"msg", "ws-skip", "");

        let inner = hub.inner.read().unwrap();
        // id1's queue received the message; id2's was skipped. Verify via
        // sender capacity: both channels start at 256 capacity.
        let h1 = inner.clients[&id1].clone();
        let h2 = inner.clients[&id2].clone();
        drop(inner);
        assert_eq!(h1.sender.capacity(), 255);
        assert_eq!(h2.sender.capacity(), 256);
    }

    #[test]
    fn snapshot_shape_matches_go() {
        let hub = Hub::new();
        let (_, _, _rx) = make_client(&hub, "u-1", "ws-1");

        let snap = hub.snapshot();
        assert_eq!(snap["connections"], 1);
        assert_eq!(snap["rooms"]["workspace"], 1);
        assert_eq!(snap["rooms"]["user"], 1);
    }

    #[test]
    fn absent_scope_authorizer_fails_closed() {
        let hub = Hub::new();
        let (client, _, _rx) = make_client(&hub, "u-1", "ws-1");

        assert_eq!(
            hub.authorize_subscription(&client, "task", "task-1"),
            Err("lookup_failed")
        );
    }
}
