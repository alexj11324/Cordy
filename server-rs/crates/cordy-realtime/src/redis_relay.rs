//! Legacy per-scope Redis Stream relay — port of `server/internal/realtime/redis_relay.go`.
//!
//! One consumer group per node per scope; hub subscription changes drive
//! per-scope XREADGROUP loops. Retention (XTRIM/TTL) is shared with the
//! sharded mode via [`StreamRetentionConfig`].

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use tokio_util::sync::CancellationToken;

use crate::broadcaster::{Broadcaster, DaemonRuntimeDeliverer, RelayPublisher, SCOPE_USER};
use crate::envelope::{
    deliver_envelope, heartbeat_key, inject_event_id, nodes_key, stream_key, xadd_envelope_command,
    Envelope, HubFanout,
};
use crate::metrics::M;
use crate::stream_retention::{stream_min_id, StreamRetentionConfig, StreamTtlRefresher};

/// A scope currently active on this node (`scopeKey` in Go).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScopeKey {
    pub scope_type: String,
    pub scope_id: String,
}

impl ScopeKey {
    pub fn new(scope_type: &str, scope_id: &str) -> Self {
        Self {
            scope_type: scope_type.to_string(),
            scope_id: scope_id.to_string(),
        }
    }
}

/// Callback invoked on scope subscribe/unsubscribe transitions.
pub type ScopeEventCallback = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// The hub-side surface the relay needs: local scope snapshot plus
/// subscription-change callbacks (`LocalScopes` / `SetSubscriptionCallbacks`).
pub trait ScopeSubscriptionSource: Send + Sync {
    /// Snapshot of scopes currently active on this node.
    fn local_scopes(&self) -> Vec<ScopeKey>;
    /// Registers subscribe/unsubscribe callbacks; returns the current snapshot.
    fn set_subscription_callbacks(&self, on_first: ScopeEventCallback, on_last: ScopeEventCallback);
}

struct ConsumerHandle {
    token: CancellationToken,
}

/// Legacy relay: writes every message to a per-scope Redis Stream and consumes
/// streams for which there are local subscribers. Local fanout is delegated to
/// the wrapped hub via [`HubFanout`].
pub struct RedisRelay {
    hub: Arc<dyn HubFanout>,
    registry: Arc<dyn ScopeSubscriptionSource>,
    write_conn: ConnectionManager,
    read_client: redis::Client,
    node_id: String,
    retention: StreamRetentionConfig,
    ttl: StreamTtlRefresher,

    consumers: Mutex<HashMap<ScopeKey, ConsumerHandle>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    shutdown: CancellationToken,
    stopping: AtomicBool,

    legacy_scan_cursor: Mutex<u64>,
    streams_without_ttl: Mutex<HashSet<String>>,
    ttl_scan_seen: Mutex<HashSet<String>>,

    daemon_runtime: Mutex<Option<Arc<dyn DaemonRuntimeDeliverer>>>,
}

impl RedisRelay {
    /// Constructs a relay with separate clients for writes and blocking reads —
    /// the read client is reserved for XREADGROUP BLOCK so long-polling cannot
    /// exhaust the pool used by request-path operations.
    pub async fn new_with_clients(
        hub: Arc<dyn HubFanout>,
        registry: Arc<dyn ScopeSubscriptionSource>,
        write_client: redis::Client,
        read_client: Option<redis::Client>,
        retention: StreamRetentionConfig,
    ) -> anyhow::Result<Self> {
        let read_client = read_client.unwrap_or_else(|| write_client.clone());
        let retention = retention.with_defaults();
        let write_conn = write_client.get_connection_manager().await?;
        Ok(Self {
            hub,
            registry,
            write_conn,
            read_client,
            node_id: cordy_util::new_ulid(),
            ttl: StreamTtlRefresher::new(retention.stream_ttl, retention.ttl_refresh_interval),
            retention,
            consumers: Mutex::new(HashMap::new()),
            tasks: Mutex::new(Vec::new()),
            shutdown: CancellationToken::new(),
            stopping: AtomicBool::new(false),
            legacy_scan_cursor: Mutex::new(0),
            streams_without_ttl: Mutex::new(HashSet::new()),
            ttl_scan_seen: Mutex::new(HashSet::new()),
            daemon_runtime: Mutex::new(None),
        })
    }

    fn write_conn_handle(&self) -> ConnectionManager {
        self.write_conn.clone()
    }

    pub fn node_id(&self) -> String {
        self.node_id.clone()
    }

    pub fn set_daemon_runtime_deliverer(&self, d: Arc<dyn DaemonRuntimeDeliverer>) {
        *self
            .daemon_runtime
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(d);
    }

    /// Prevents new scope consumers from starting and cancels active ones.
    /// Cancel the startup token before awaiting [`RedisRelay::wait`].
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Relaxed);
        self.shutdown.cancel();
        let mut guard = self.consumers.lock().unwrap_or_else(|e| e.into_inner());
        for handle in guard.values() {
            handle.token.cancel();
        }
        guard.clear();
    }

    /// Wires hub→relay subscription callbacks, starts heartbeat and sweeper
    /// loops, and spins up consumers for any scopes the hub already knows
    /// about. Requires `Arc<Self>` so spawned loops can hold the relay alive.
    pub fn start(self: &Arc<Self>) {
        M.node_id
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clone_from(&self.node_id);

        let state = self.clone();
        let probe_state = state.clone();
        state.spawn(async move {
            // Initial connectivity probe (non-fatal; gauges stay honest).
            let mut conn = probe_state.write_conn_handle();
            let ping = redis::cmd("PING").query_async::<()>(&mut conn).await;
            match &ping {
                Err(e) => {
                    tracing::error!(error = %e, "realtime/redis: initial ping failed");
                    M.redis_connected.store(false, Ordering::Relaxed);
                    M.set_redis_last_error(&e.to_string());
                }
                Ok(()) => M.redis_connected.store(true, Ordering::Relaxed),
            }
            drop(conn);
        });

        let on_first: ScopeEventCallback = {
            let s = state.clone();
            Arc::new(move |t, id| s.start_consumer(t, id))
        };
        let on_last: ScopeEventCallback = {
            let s = state.clone();
            Arc::new(move |t, id| s.stop_consumer(t, id))
        };
        state.registry.set_subscription_callbacks(on_first, on_last);

        for key in state.registry.local_scopes() {
            state.start_consumer(&key.scope_type, &key.scope_id);
        }

        let hb_state = state.clone();
        state.spawn(async move { hb_state.heartbeat_loop().await });
        let sw_state = state.clone();
        state.spawn(async move { sw_state.consumer_sweeper().await });
    }

    /// Spawns a tracked background task owned by this relay.
    fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static) {
        let handle = tokio::spawn(fut);
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(handle);
    }

    /// Waits until all relay-owned tasks have exited (after stop/cancel).
    pub async fn wait(&self) {
        let handles: Vec<_> = self
            .tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect();
        for h in handles {
            let _ = h.await;
        }
    }

    /// Starts a single per-scope XREADGROUP loop if not already running.
    fn start_consumer(self: &Arc<Self>, scope_type: &str, scope_id: &str) {
        let key = ScopeKey::new(scope_type, scope_id);
        let token = {
            let mut guard = self.consumers.lock().unwrap_or_else(|e| e.into_inner());
            if self.stopping.load(Ordering::Relaxed) || self.shutdown.is_cancelled() {
                return;
            }
            if guard.contains_key(&key) {
                return;
            }
            let token = self.shutdown.child_token();
            guard.insert(
                key.clone(),
                ConsumerHandle {
                    token: token.clone(),
                },
            );
            token
        };

        let this = self.clone();
        let handle = tokio::spawn(async move {
            this.run_consumer(token, &key).await;
        });
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(handle);
    }

    fn stop_consumer(&self, scope_type: &str, scope_id: &str) {
        let key = ScopeKey::new(scope_type, scope_id);
        if let Some(handle) = self
            .consumers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&key)
        {
            handle.token.cancel();
        }
    }

    async fn run_consumer(&self, token: CancellationToken, key: &ScopeKey) {
        let stream = stream_key(&key.scope_type, &key.scope_id);
        let group = format!("node:{}", self.node_id);
        let consumer_name = self.node_id.clone();

        // MKSTREAM ensures the stream exists. BUSYGROUP is ignored.
        {
            let mut conn = self.write_conn_handle();
            if let Err(e) = self
                .ensure_consumer_group(&mut conn, &stream, &group, "$")
                .await
            {
                tracing::warn!(
                    error = %e,
                    scope = %key.scope_type,
                    scope_id = %key.scope_id,
                    "realtime/redis: XGROUP CREATE failed"
                );
            }
            if self.retention.stream_ttl_enabled {
                if let Err(e) = self.ttl.refresh_if_due(&mut conn, &stream).await {
                    self.record_retention_error(
                        "consumer stream PEXPIRE failed",
                        &e.to_string(),
                        &[("stream", stream.as_str())],
                    );
                }
            }
        }

        // Register ourselves as a node interested in this scope.
        {
            let mut conn = self.write_conn_handle();
            let score = (chrono::Utc::now()
                + chrono::Duration::seconds(crate::envelope::HEARTBEAT_TTL_SECS))
            .timestamp() as f64;
            let _: Result<i64, _> = redis::cmd("ZADD")
                .arg(nodes_key(&key.scope_type, &key.scope_id))
                .arg(score)
                .arg(&self.node_id)
                .query_async(&mut conn)
                .await;
        }

        loop {
            if token.is_cancelled() || self.shutdown.is_cancelled() {
                break;
            }

            let mut conn = match self.read_client.get_connection_manager().await {
                Ok(c) => c,
                Err(e) => {
                    M.redis_xread_errors.fetch_add(1, Ordering::Relaxed);
                    M.set_redis_last_error(&e.to_string());
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };

            let raw: Result<redis::Value, _> = redis::cmd("XREADGROUP")
                .arg("GROUP")
                .arg(&group)
                .arg(&consumer_name)
                .arg("COUNT")
                .arg(32)
                .arg("BLOCK")
                .arg(5_000)
                .arg("STREAMS")
                .arg(&stream)
                .arg(">")
                .query_async(&mut conn)
                .await;

            match raw {
                // Nil reply or block timeout — keep polling.
                Ok(redis::Value::Nil) => continue,
                Err(e) if e.to_string().contains("NOGROUP") => {
                    // Stream was deleted out from under us: recreate the group
                    // from 0-0 to drain any stragglers, then keep going.
                    self.ttl.forget(&stream);
                    let mut w = self.write_conn_handle();
                    let repaired = self
                        .ensure_consumer_group(&mut w, &stream, &group, "0-0")
                        .await
                        .is_ok();
                    if repaired
                        && self.retention.stream_ttl_enabled
                        && self.ttl.refresh_if_due(&mut w, &stream).await.is_err()
                    {
                        self.record_retention_error(
                            "consumer group recovery TTL refresh failed",
                            "refresh failed",
                            &[("stream", stream.as_str())],
                        );
                    }
                    if repaired {
                        M.redis_relay_stream_missing_total
                            .fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    self.record_retention_error(
                        "consumer group recovery failed",
                        "repair failed",
                        &[("stream", stream.as_str())],
                    );
                    M.redis_xread_errors.fetch_add(1, Ordering::Relaxed);
                    M.set_redis_last_error("NOGROUP repair failed");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                Err(e) => {
                    M.redis_xread_errors.fetch_add(1, Ordering::Relaxed);
                    M.set_redis_last_error(&e.to_string());
                    tracing::warn!(
                        error = %e,
                        stream = %stream,
                        "realtime/redis: XREADGROUP failed"
                    );
                    tokio::select! {
                        () = token.cancelled() => break,
                        () = tokio::time::sleep(Duration::from_secs(1)) => {}
                    }
                    continue;
                }
                Ok(value) => {
                    for (_k, messages) in crate::sharded_stream_relay::parse_xread_response(&value)
                    {
                        for (id, fields) in messages {
                            M.redis_xread_total.fetch_add(1, Ordering::Relaxed);
                            self.deliver_fields(key, &fields).await;
                            // Ack after delivery so a crash re-delivers.
                            let mut w = self.write_conn_handle();
                            let acked: Result<i64, _> = redis::cmd("XACK")
                                .arg(&stream)
                                .arg(&group)
                                .arg(&id)
                                .query_async(&mut w)
                                .await;
                            match acked {
                                Ok(_) => M.redis_ack_total.fetch_add(1, Ordering::Relaxed),
                                Err(e) => {
                                    tracing::debug!(error = %e, id = %id, "realtime/redis: XACK failed");
                                    0
                                }
                            };
                        }
                    }
                }
            }
        }

        // Best-effort consumer cleanup.
        let mut w = self.write_conn_handle();
        let _: Result<i64, _> = redis::cmd("XGROUP")
            .arg("DELCONSUMER")
            .arg(&stream)
            .arg(&group)
            .arg(&consumer_name)
            .query_async(&mut w)
            .await;
    }

    async fn ensure_consumer_group(
        &self,
        conn: &mut ConnectionManager,
        stream: &str,
        group: &str,
        start_id: &str,
    ) -> anyhow::Result<()> {
        let result: Result<(), _> = redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(stream)
            .arg(group)
            .arg(start_id)
            .arg("MKSTREAM")
            .query_async(conn)
            .await;
        match result {
            Ok(()) => Ok(()),
            // BUSYGROUP means it already exists — that's fine.
            Err(e) if e.to_string().contains("BUSYGROUP") => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    async fn deliver_fields(&self, key: &ScopeKey, fields: &[(String, String)]) {
        if let Some(mut ev) = Envelope::from_field_pairs(fields) {
            if ev.scope.is_empty() {
                ev.scope = key.scope_type.clone();
            }
            if ev.scope_id.is_empty() {
                ev.scope_id = key.scope_id.clone();
            }
            let daemon_runtime = self
                .daemon_runtime
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            deliver_envelope(self.hub.clone(), daemon_runtime, ev).await;
        }
    }

    async fn heartbeat_loop(&self) {
        let mut ticker = tokio::time::interval(Duration::from_secs(
            crate::envelope::HEARTBEAT_PERIOD_SECS as u64,
        ));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            self.heartbeat_once().await;
            tokio::select! {
                () = self.shutdown.cancelled() => return,
                _ = ticker.tick() => {}
            }
        }
    }

    async fn heartbeat_once(&self) {
        let stamp = cordy_util::rfc3339_nano(chrono::Utc::now());
        let mut conn = self.write_conn_handle();
        let result = redis::cmd("SET")
            .arg(heartbeat_key(&self.node_id))
            .arg(stamp)
            .arg("EX")
            .arg(crate::envelope::HEARTBEAT_TTL_SECS)
            .query_async::<()>(&mut conn)
            .await;
        match result {
            Err(e) => {
                M.redis_connected.store(false, Ordering::Relaxed);
                M.set_redis_last_error(&e.to_string());
                return;
            }
            Ok(()) => M.redis_connected.store(true, Ordering::Relaxed),
        }

        // Refresh our membership in every local scope's node ZSET.
        let expiry = (chrono::Utc::now()
            + chrono::Duration::seconds(crate::envelope::HEARTBEAT_TTL_SECS))
        .timestamp() as f64;
        for key in self.registry.local_scopes() {
            let _: Result<i64, _> = redis::cmd("ZADD")
                .arg(nodes_key(&key.scope_type, &key.scope_id))
                .arg(expiry)
                .arg(&self.node_id)
                .query_async(&mut conn)
                .await;
        }
    }

    /// Periodically drops stale ZSET entries and advances a bounded SCAN over
    /// legacy per-scope streams, repairing keys created by older pods before
    /// TTL retention existed — without blocking Redis.
    async fn consumer_sweeper(&self) {
        let mut ticker = tokio::time::interval(self.retention.maintenance_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            self.sweep_legacy_streams().await;
            tokio::select! {
                () = self.shutdown.cancelled() => return,
                _ = ticker.tick() => {}
            }
        }
    }

    async fn sweep_legacy_streams(&self) {
        let now = chrono::Utc::now();
        let min_id = stream_min_id(std::time::SystemTime::now(), self.retention.trim_horizon);
        let mut local_streams: HashSet<String> = HashSet::new();

        for key in self.registry.local_scopes() {
            let stream = stream_key(&key.scope_type, &key.scope_id);
            local_streams.insert(stream.clone());
            self.observe_legacy_scan_key(&stream);
            self.maintain_legacy_stream(&stream, &min_id).await;

            // Drop ZSET entries whose node heartbeats have expired.
            let cutoff = now.timestamp() as f64;
            let mut conn = self.write_conn_handle();
            let _: Result<i64, _> = redis::cmd("ZREMRANGEBYSCORE")
                .arg(nodes_key(&key.scope_type, &key.scope_id))
                .arg("-inf")
                .arg(format!("{cutoff}"))
                .query_async(&mut conn)
                .await;
        }

        // Bounded SCAN over all legacy streams (cursor advances across sweeps).
        let cursor = *self
            .legacy_scan_cursor
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut conn = self.write_conn_handle();
        let scan: Result<(u64, Vec<String>), _> = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg("ws:scope:*:stream")
            .arg("COUNT")
            .arg(crate::envelope::LEGACY_STREAM_SCAN_COUNT)
            .query_async(&mut conn)
            .await;
        drop(conn);

        match scan {
            Err(e) => {
                self.record_retention_error(
                    "legacy stream SCAN failed",
                    &e.to_string(),
                    &[("cursor", &cursor.to_string())],
                );
            }
            Ok((next_cursor, keys)) => {
                for stream in keys {
                    self.observe_legacy_scan_key(&stream);
                    if local_streams.contains(&stream) {
                        continue;
                    }
                    self.maintain_legacy_stream(&stream, &min_id).await;
                }
                if next_cursor == 0 {
                    self.complete_legacy_ttl_scan();
                }
                *self
                    .legacy_scan_cursor
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = next_cursor;
            }
        }

        self.ttl
            .forget_stale(std::time::Instant::now() - self.retention.stream_ttl);
    }

    async fn maintain_legacy_stream(&self, stream: &str, min_id: &str) {
        let mut conn = self.write_conn_handle();

        match redis::cmd("XTRIM")
            .arg(stream)
            .arg("MINID")
            .arg(min_id)
            .query_async::<i64>(&mut conn)
            .await
        {
            Err(e) if !e.to_string().is_empty() => {
                self.record_retention_error(
                    "XTRIM MINID failed",
                    &e.to_string(),
                    &[("stream", stream)],
                );
            }
            Ok(trimmed) if trimmed > 0 => {
                M.redis_relay_stream_trimmed_total
                    .fetch_add(trimmed, Ordering::Relaxed);
            }
            _ => {}
        }

        let ttl = self
            .ttl
            .reconcile_ttl(&mut conn, stream, self.retention.stream_ttl_enabled)
            .await;
        match &ttl {
            Ok(t) => self.observe_legacy_ttl(stream, *t),
            Err(e) => {
                self.record_retention_error(
                    "stream TTL repair failed",
                    &e.to_string(),
                    &[("stream", stream)],
                );
            }
        }
    }

    fn observe_legacy_ttl(&self, stream: &str, ttl_millis: i64) {
        let mut guard = self
            .streams_without_ttl
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.retention.stream_ttl_enabled && ttl_millis == -1 {
            guard.insert(stream.to_string());
        } else {
            guard.remove(stream);
        }
        let count = guard.len() as i64;
        drop(guard);
        M.set_redis_streams_without_ttl("legacy", count);
    }

    fn observe_legacy_scan_key(&self, stream: &str) {
        self.ttl_scan_seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(stream.to_string());
    }

    /// Prunes without-TTL entries that the completed full scan did not see —
    /// they belonged to streams deleted since the last sweep.
    fn complete_legacy_ttl_scan(&self) {
        let mut guard = self
            .streams_without_ttl
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut seen = self.ttl_scan_seen.lock().unwrap_or_else(|e| e.into_inner());
        guard.retain(|stream| seen.contains(stream));
        seen.clear();
        let count = guard.len() as i64;
        drop(guard);
        M.set_redis_streams_without_ttl("legacy", count);
    }

    fn record_retention_error(&self, message: &str, err: &str, attrs: &[(&str, &str)]) {
        M.redis_relay_retention_errors
            .fetch_add(1, Ordering::Relaxed);
        M.set_redis_last_error(err);
        tracing::warn!(error = %err, attrs = ?attrs, "realtime/redis: {}", message);
    }
}

#[async_trait]
impl RelayPublisher for RedisRelay {
    async fn publish_with_id(
        &self,
        scope_type: &str,
        scope_id: &str,
        exclude: &str,
        frame: &[u8],
        event_id: &str,
    ) -> anyhow::Result<()> {
        let ev = Envelope::new(
            &self.node_id,
            scope_type,
            scope_id,
            exclude,
            frame,
            event_id,
        );
        let stream = stream_key(scope_type, scope_id);

        let cmd = xadd_envelope_command(&stream, self.retention.stream_max_len, &ev);

        let start = std::time::Instant::now();
        let mut conn = self.write_conn_handle();
        match cmd.query_async::<String>(&mut conn).await {
            Err(e) => {
                M.redis_xadd_errors.fetch_add(1, Ordering::Relaxed);
                M.set_redis_last_error(&e.to_string());
                tracing::warn!(
                    error = %e,
                    scope = %scope_type,
                    scope_id = %scope_id,
                    "realtime/redis: XADD failed"
                );
                return Err(e.into());
            }
            Ok(_) => {
                M.redis_xadd_total.fetch_add(1, Ordering::Relaxed);
                M.redis_last_xadd_lag_micros
                    .store(start.elapsed().as_micros() as i64, Ordering::Relaxed);
                if self.retention.stream_ttl_enabled {
                    if let Err(e) = self.ttl.refresh_if_due(&mut conn, &stream).await {
                        self.record_retention_error(
                            "PEXPIRE failed",
                            &e.to_string(),
                            &[("stream", stream.as_str())],
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Broadcaster for RedisRelay {
    async fn broadcast_to_scope(&self, scope_type: &str, scope_id: &str, message: &[u8]) {
        let id = cordy_util::new_ulid();
        let _ = self
            .publish_with_id(scope_type, scope_id, "", message, &id)
            .await;
    }

    async fn send_to_user(&self, user_id: &str, message: &[u8], exclude_workspace: Option<&str>) {
        let exclude = exclude_workspace.unwrap_or("");
        let id = cordy_util::new_ulid();
        let _ = self
            .publish_with_id(SCOPE_USER, user_id, exclude, message, &id)
            .await;
    }

    /// Daemon broadcast — writes to a special "global" stream so other nodes
    /// can fan out to all clients regardless of subscriptions.
    async fn broadcast(&self, message: &[u8]) {
        let id = cordy_util::new_ulid();
        let _ = self
            .publish_with_id("global", "all", "", message, &id)
            .await;
    }

    // broadcast_to_workspace inherits the default SCOPE_WORKSPACE delegation.
}

#[async_trait]
impl crate::relay_lifecycle::ManagedRelay for RedisRelay {
    fn node_id(&self) -> String {
        self.node_id()
    }

    fn start(self: Arc<Self>, shutdown: CancellationToken) {
        RedisRelay::start(&self);
        let relay = self.clone();
        self.spawn(async move {
            tokio::select! {
                () = shutdown.cancelled() => relay.stop(),
                () = relay.shutdown.cancelled() => {}
            }
        });
    }

    fn stop(&self) {
        RedisRelay::stop(self);
    }

    async fn wait(&self) {
        RedisRelay::wait(self).await;
    }

    fn set_daemon_runtime_deliverer(&self, deliverer: Arc<dyn DaemonRuntimeDeliverer>) {
        RedisRelay::set_daemon_runtime_deliverer(self, deliverer);
    }
}

/// Delivers every event to local clients before publishing the same frame to
/// Redis. A shared event id is injected into the client frame so the relay's
/// local loopback is discarded by the hub dedup cache.
pub struct DualWriteBroadcaster {
    hub: Arc<dyn HubFanout>,
    relay: Arc<dyn RelayPublisher>,
}

impl DualWriteBroadcaster {
    pub fn new(hub: Arc<dyn HubFanout>, relay: Arc<dyn RelayPublisher>) -> Self {
        Self { hub, relay }
    }

    async fn deliver_and_publish(
        &self,
        scope_type: &str,
        scope_id: &str,
        exclude: &str,
        message: &[u8],
    ) {
        let event_id = cordy_util::new_ulid();
        let frame = inject_event_id(message, &event_id);
        if scope_type == "global" {
            self.hub.fanout_all_dedup(&frame, exclude, &event_id).await;
        } else if scope_type == SCOPE_USER {
            self.hub
                .fanout_user(scope_id, &frame, exclude, &event_id)
                .await;
        } else {
            self.hub
                .broadcast_to_scope_dedup(scope_type, scope_id, &frame, &event_id)
                .await;
        }
        if let Err(error) = self
            .relay
            .publish_with_id(scope_type, scope_id, exclude, &frame, &event_id)
            .await
        {
            tracing::warn!(
                %error,
                scope = scope_type,
                scope_id,
                event_id,
                "realtime relay publish failed after local delivery"
            );
        }
    }
}

#[async_trait]
impl Broadcaster for DualWriteBroadcaster {
    async fn broadcast_to_scope(&self, scope_type: &str, scope_id: &str, message: &[u8]) {
        self.deliver_and_publish(scope_type, scope_id, "", message)
            .await;
    }

    async fn send_to_user(&self, user_id: &str, message: &[u8], exclude_workspace: Option<&str>) {
        self.deliver_and_publish(
            SCOPE_USER,
            user_id,
            exclude_workspace.unwrap_or_default(),
            message,
        )
        .await;
    }

    async fn broadcast(&self, message: &[u8]) {
        self.deliver_and_publish("global", "all", "", message).await;
    }
}
