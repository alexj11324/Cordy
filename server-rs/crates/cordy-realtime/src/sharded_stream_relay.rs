//! Fixed-reader sharded Redis Stream relay — port of
//! `server/internal/realtime/sharded_stream_relay.go`.
//!
//! Every API node runs one XREAD BLOCK loop per shard and locally filters
//! events by hub subscriptions. This keeps blocked Redis connections bounded
//! by pod_count × shard_count instead of active_scope_count.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::{fmt::Display, future::Future};

use async_trait::async_trait;
use redis::aio::ConnectionManager;
use tokio_util::sync::CancellationToken;

use crate::broadcaster::{Broadcaster, DaemonRuntimeDeliverer, RelayPublisher, SCOPE_USER};
use crate::envelope::{
    deliver_envelope, heartbeat_key, xadd_envelope_command, Envelope, HubFanout,
};
use crate::metrics::M;
use crate::stream_retention::{
    redis_info_int64, redis_ttl_millis, retention_subinterval, stream_min_id,
    StreamRetentionConfig, StreamTtlRefresher, DEFAULT_SHARDED_RELAY_REPLAY_GRACE,
};

const DEFAULT_SHARDED_RELAY_SHARDS: usize = 8;
/// At the measured baseline of roughly 1 KiB per relay entry, 2000 entries
/// across each of eight shards estimates about 16 MiB instead of the former
/// ~800 MiB default. Operators can tune this from observed entry sizes.
// (defaultShardedRelayStreamMaxLen lives in stream_retention.rs)
const DEFAULT_SHARDED_RELAY_READ_COUNT: i64 = 128;
const DEFAULT_SHARDED_RELAY_READ_BLOCK: Duration = Duration::from_secs(5);

/// Redis Stream key used by a fixed relay shard.
pub fn sharded_stream_key(shard: usize) -> String {
    format!("ws:relay:shard:{shard}")
}

/// Controls the fixed-reader Redis Stream relay.
#[derive(Debug, Clone, PartialEq)]
pub struct ShardedStreamRelayConfig {
    pub shards: usize,
    pub stream_max_len: i64,
    pub read_count: i64,
    pub read_block: Duration,
    /// Lookback window on startup: the shard reader starts consuming from
    /// (now - ReplayGrace) rather than "$" so events published while this pod
    /// was down are replayed. Bounded by MAXLEN; consumers must be idempotent.
    pub replay_grace: Duration,
    pub trim_horizon: Duration,
    pub stream_ttl: Duration,
    pub ttl_refresh_interval: Duration,
    pub maintenance_interval: Duration,
    pub stream_ttl_enabled: bool,
}

impl Default for ShardedStreamRelayConfig {
    fn default() -> Self {
        let retention = StreamRetentionConfig::default();
        Self {
            shards: DEFAULT_SHARDED_RELAY_SHARDS,
            stream_max_len: retention.stream_max_len,
            read_count: DEFAULT_SHARDED_RELAY_READ_COUNT,
            read_block: DEFAULT_SHARDED_RELAY_READ_BLOCK,
            replay_grace: DEFAULT_SHARDED_RELAY_REPLAY_GRACE,
            trim_horizon: retention.trim_horizon,
            stream_ttl: retention.stream_ttl,
            ttl_refresh_interval: retention.ttl_refresh_interval,
            maintenance_interval: retention.maintenance_interval,
            stream_ttl_enabled: retention.stream_ttl_enabled,
        }
    }
}

impl ShardedStreamRelayConfig {
    /// Mode-independent stream retention settings.
    pub fn retention_config(&self) -> StreamRetentionConfig {
        let c = self.clone().with_defaults();
        StreamRetentionConfig {
            stream_max_len: c.stream_max_len,
            trim_horizon: c.trim_horizon,
            stream_ttl: c.stream_ttl,
            ttl_refresh_interval: c.ttl_refresh_interval,
            maintenance_interval: c.maintenance_interval,
            stream_ttl_enabled: c.stream_ttl_enabled,
        }
    }

    /// Fills missing fields and repairs unsafe retention relationships.
    pub fn with_defaults(mut self) -> Self {
        let def = Self::default();
        if self.shards == 0 {
            self.shards = def.shards;
        }
        if self.stream_max_len <= 0 {
            self.stream_max_len = def.stream_max_len;
        }
        if self.read_count <= 0 {
            self.read_count = def.read_count;
        }
        if self.read_block.is_zero() {
            self.read_block = def.read_block;
        }
        if self.replay_grace.is_zero() {
            self.replay_grace = def.replay_grace;
        }
        if self.trim_horizon <= self.replay_grace {
            self.trim_horizon = self.replay_grace * 2;
        }
        if self.stream_ttl < self.trim_horizon {
            self.stream_ttl = self.trim_horizon + self.replay_grace;
        }
        if self.ttl_refresh_interval.is_zero() || self.ttl_refresh_interval >= self.stream_ttl {
            self.ttl_refresh_interval =
                retention_subinterval(self.stream_ttl, def.ttl_refresh_interval);
        }
        if self.maintenance_interval.is_zero() || self.maintenance_interval >= self.stream_ttl {
            self.maintenance_interval =
                retention_subinterval(self.stream_ttl, def.maintenance_interval);
        }
        self
    }

    /// Alias kept for call-site parity with Go's `Normalized()`.
    pub fn normalized(self) -> Self {
        self.with_defaults()
    }

    /// Checks the retention relationship that keeps trimming outside the
    /// replay window while allowing idle stream keys to expire safely.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(!self.replay_grace.is_zero(), "ReplayGrace must be positive");
        anyhow::ensure!(
            self.trim_horizon > self.replay_grace,
            "TrimHorizon ({:?}) must be greater than ReplayGrace ({:?})",
            self.trim_horizon,
            self.replay_grace
        );
        anyhow::ensure!(
            self.stream_ttl >= self.trim_horizon,
            "StreamTTL ({:?}) must be at least TrimHorizon ({:?})",
            self.stream_ttl,
            self.trim_horizon
        );
        anyhow::ensure!(
            !self.ttl_refresh_interval.is_zero() && self.ttl_refresh_interval < self.stream_ttl,
            "TTLRefreshInterval ({:?}) must be positive and less than StreamTTL ({:?})",
            self.ttl_refresh_interval,
            self.stream_ttl
        );
        anyhow::ensure!(
            !self.maintenance_interval.is_zero() && self.maintenance_interval < self.stream_ttl,
            "MaintenanceInterval ({:?}) must be positive and less than StreamTTL ({:?})",
            self.maintenance_interval,
            self.stream_ttl
        );
        Ok(())
    }
}

/// FNV-1a 32-bit over scopeType + NUL separator + scopeID.
fn fnv32a(parts: &[&[u8]]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for part in parts {
        for &b in *part {
            hash ^= u32::from(b);
            hash = hash.wrapping_mul(0x0100_0193);
        }
    }
    hash
}

async fn connect_with_retry<T, E, Connect, Connecting, OnError>(
    shutdown: &CancellationToken,
    retry_delay: Duration,
    mut connect: Connect,
    mut on_error: OnError,
) -> Option<T>
where
    E: Display,
    Connect: FnMut() -> Connecting,
    Connecting: Future<Output = Result<T, E>>,
    OnError: FnMut(&E),
{
    loop {
        let result = tokio::select! {
            biased;
            () = shutdown.cancelled() => return None,
            result = connect() => result,
        };
        match result {
            Ok(connection) => return Some(connection),
            Err(error) => on_error(&error),
        }
        tokio::select! {
            biased;
            () = shutdown.cancelled() => return None,
            () = tokio::time::sleep(retry_delay) => {}
        }
    }
}

/// Publishes all realtime events into a fixed set of Redis Streams.
pub struct ShardedStreamRelay<H: HubFanout> {
    hub: Arc<H>,
    /// Shared multiplexed handle for fast write-path commands. Clones are
    /// cheap and safe across tasks — no mutex needed.
    write_conn: ConnectionManager,
    /// Used by shard readers to acquire dedicated connections: a blocking
    /// XREAD must not stall other commands multiplexed on one socket.
    read_client: redis::Client,
    node_id: String,
    config: ShardedStreamRelayConfig,
    ttl: StreamTtlRefresher,

    shutdown: CancellationToken,
    stopping: AtomicBool,

    stream_seen: Vec<AtomicBool>,
    stream_generation: Vec<AtomicU64>,

    daemon_runtime: Mutex<Option<Arc<dyn DaemonRuntimeDeliverer>>>,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl<H: HubFanout + 'static> ShardedStreamRelay<H> {
    /// `read_client` defaults to the write client when None (Go's nil check).
    /// Async because establishing the shared write connection performs I/O.
    pub async fn new(
        hub: Arc<H>,
        write_client: redis::Client,
        read_client: Option<redis::Client>,
        config: ShardedStreamRelayConfig,
    ) -> anyhow::Result<Self> {
        let read_client = read_client.unwrap_or_else(|| write_client.clone());
        let config = config.with_defaults();
        let shards = config.shards;
        let ttl = StreamTtlRefresher::new(config.stream_ttl, config.ttl_refresh_interval);
        let write_conn = write_client.get_connection_manager().await?;
        Ok(Self {
            hub,
            write_conn,
            read_client,
            node_id: ulid::Ulid::new().to_string(),
            config,
            ttl,
            shutdown: CancellationToken::new(),
            stopping: AtomicBool::new(false),
            stream_seen: (0..shards).map(|_| AtomicBool::new(false)).collect(),
            stream_generation: (0..shards).map(|_| AtomicU64::new(0)).collect(),
            daemon_runtime: Mutex::new(None),
            tasks: Mutex::new(Vec::new()),
        })
    }

    /// Cheap handle for issuing write-path commands from any task.
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

    fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Relaxed)
    }

    /// Pings Redis, stamps the metrics NodeID, then spawns the heartbeat,
    /// retention, and one reader loop per shard. Tasks exit on `stop()` or
    /// when the returned token's guard is dropped elsewhere.
    pub fn start(self: &Arc<Self>) {
        M.node_id
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clone_from(&self.node_id);

        let state = self.clone();
        let handle = tokio::spawn(async move {
            // Initial connectivity probe — failures are logged but not fatal;
            // the loops retry and keep the connected gauge honest.
            let connected = state.ping_write().await.is_ok();
            M.redis_connected.store(connected, Ordering::Relaxed);
            if !connected {
                M.set_redis_last_error("initial ping failed");
            } else {
                tracing::info!("realtime/sharded-redis: connected");
            }

            let mut set = tokio::task::JoinSet::new();
            let hb_state = state.clone();
            set.spawn(async move { hb_state.heartbeat_loop().await });
            let rt_state = state.clone();
            set.spawn(async move { rt_state.retention_loop().await });
            for shard in 0..state.config.shards {
                let shard_state = state.clone();
                set.spawn(async move { shard_state.read_shard(shard).await });
            }
            // Keep the token alive for the lifetime of the task set.
            let token = state.shutdown.clone();
            loop {
                tokio::select! {
                    () = token.cancelled() => break,
                    Some(res) = set.join_next() => {
                        if let Err(e) = res {
                            tracing::warn!(error = %e, "realtime/sharded-redis: task panicked");
                        }
                    }
                }
            }
            // Drain remaining tasks after cancellation.
            while set.join_next().await.is_some() {}
        });
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(handle);
    }

    async fn ping_write(&self) -> anyhow::Result<()> {
        let mut conn = self.write_conn_handle();
        redis::cmd("PING").query_async::<()>(&mut conn).await?;
        Ok(())
    }

    /// Signals all background loops to exit.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Relaxed);
        self.shutdown.cancel();
    }

    pub async fn wait(&self) {
        let handles: Vec<_> = self
            .tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect();
        for handle in handles {
            let _ = handle.await;
        }
    }

    fn shard_for(&self, scope_type: &str, scope_id: &str) -> usize {
        let h = fnv32a(&[scope_type.as_bytes(), &[0], scope_id.as_bytes()]);
        (h % self.config.shards as u32) as usize
    }

    /// Redis stream ID anchored to (now - ReplayGrace) so a freshly started
    /// shard reader replays only the recent grace window rather than the
    /// entire retained stream. The "-0" suffix matches any sequence number at
    /// that millisecond.
    fn replay_start_id(&self) -> String {
        stream_min_id(std::time::SystemTime::now(), self.config.replay_grace)
    }

    async fn publish_with_id_inner(
        &self,
        scope_type: &str,
        scope_id: &str,
        exclude: &str,
        frame: &[u8],
        id: &str,
    ) -> anyhow::Result<()> {
        let ev = Envelope::new(&self.node_id, scope_type, scope_id, exclude, frame, id);
        let shard = self.shard_for(scope_type, scope_id);
        let stream = sharded_stream_key(shard);

        let cmd = xadd_envelope_command(&stream, self.config.stream_max_len, &ev);

        let start = std::time::Instant::now();
        let result: anyhow::Result<String> = {
            let mut conn = self.write_conn_handle();
            cmd.query_async(&mut conn).await.map_err(Into::into)
        };

        match result {
            Err(e) => {
                M.redis_xadd_errors.fetch_add(1, Ordering::Relaxed);
                M.set_redis_last_error(&e.to_string());
                tracing::warn!(
                    error = %e,
                    scope = %scope_type,
                    scope_id = %scope_id,
                    stream = %stream,
                    "realtime/sharded-redis: XADD failed"
                );
                return Err(e);
            }
            Ok(_) => {
                M.redis_xadd_total.fetch_add(1, Ordering::Relaxed);
                M.redis_last_xadd_lag_micros
                    .store(start.elapsed().as_micros() as i64, Ordering::Relaxed);
                self.stream_seen[shard].store(true, Ordering::Relaxed);
                if self.config.stream_ttl_enabled {
                    let mut conn = self.write_conn_handle();
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

    async fn read_shard(self: Arc<Self>, shard: usize) {
        let stream = sharded_stream_key(shard);
        // Dedicated connection per reader: XREAD BLOCK would otherwise stall a
        // multiplexed connection shared with other commands.
        let Some(mut conn) = connect_with_retry(
            &self.shutdown,
            Duration::from_secs(1),
            || self.read_client.get_connection_manager(),
            |error| {
                M.redis_xread_errors.fetch_add(1, Ordering::Relaxed);
                M.set_redis_last_error(&error.to_string());
                tracing::error!(
                    error = %error,
                    shard,
                    "realtime/sharded-redis: reader connection failed; retrying"
                );
            },
        )
        .await
        else {
            return;
        };
        // Start from a bounded lookback window, not "$", so that events
        // published while this pod was down are replayed. Downstream
        // consumers are idempotent.
        let mut last_id = self.replay_start_id();
        let mut generation = self.stream_generation[shard].load(Ordering::Relaxed);
        loop {
            if self.shutdown.is_cancelled() || self.is_stopping() {
                return;
            }
            let current = self.stream_generation[shard].load(Ordering::Relaxed);
            if current != generation {
                last_id = self.replay_start_id();
                generation = current;
            }
            if !self
                .read_shard_once(&mut conn, shard, &stream, &mut last_id)
                .await
            {
                return;
            }
        }
    }

    async fn retention_loop(self: Arc<Self>) {
        self.maintain_streams().await;
        let mut ticker = tokio::time::interval(self.config.maintenance_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => return,
                _ = ticker.tick() => self.maintain_streams().await,
            }
        }
    }

    async fn maintain_streams(&self) {
        let min_id = stream_min_id(std::time::SystemTime::now(), self.config.trim_horizon);
        let mut without_ttl = 0i64;

        for shard in 0..self.config.shards {
            if self.shutdown.is_cancelled() {
                return;
            }
            let stream = sharded_stream_key(shard);
            let mut conn = self.write_conn_handle();

            let exists: Result<i64, _> = redis::cmd("EXISTS")
                .arg(&stream)
                .query_async(&mut conn)
                .await;
            let exists = match exists {
                Ok(n) => n,
                Err(e) => {
                    self.record_retention_error(
                        "EXISTS failed",
                        &e.to_string(),
                        &[("stream", stream.as_str())],
                    );
                    continue;
                }
            };
            if exists == 0 {
                self.update_stream_presence(shard, false);
                M.observe_redis_stream(&stream, 0, 0, -2);
                continue;
            }
            self.update_stream_presence(shard, true);

            match redis::cmd("XTRIM")
                .arg(&stream)
                .arg("MINID")
                .arg(&min_id)
                .query_async::<i64>(&mut conn)
                .await
            {
                Err(e) => {
                    self.record_retention_error(
                        "XTRIM MINID failed",
                        &e.to_string(),
                        &[("stream", stream.as_str()), ("min_id", min_id.as_str())],
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
                .reconcile_ttl(&mut conn, &stream, self.config.stream_ttl_enabled)
                .await;
            match &ttl {
                Ok(t) if self.config.stream_ttl_enabled && *t == -1 => without_ttl += 1,
                Err(e) => {
                    self.record_retention_error(
                        "stream TTL repair failed",
                        &e.to_string(),
                        &[("stream", stream.as_str())],
                    );
                }
                _ => {}
            }
            let ttl = ttl.unwrap_or(-2);

            let length = redis::cmd("XLEN")
                .arg(&stream)
                .query_async::<i64>(&mut conn)
                .await
                .inspect_err(|e| {
                    self.record_retention_error(
                        "XLEN failed",
                        &e.to_string(),
                        &[("stream", stream.as_str())],
                    );
                })
                .unwrap_or(0);

            let memory_bytes = redis::cmd("MEMORY USAGE")
                .arg(&stream)
                .query_async::<Option<i64>>(&mut conn)
                .await
                .inspect_err(|e| {
                    self.record_retention_error(
                        "MEMORY USAGE failed",
                        &e.to_string(),
                        &[("stream", stream.as_str())],
                    );
                })
                .unwrap_or(None)
                .unwrap_or(0);

            drop(conn);
            M.observe_redis_stream(&stream, length, memory_bytes, redis_ttl_millis(ttl));
        }
        M.set_redis_streams_without_ttl("sharded", without_ttl);
        self.observe_redis_server().await;
    }

    fn update_stream_presence(&self, shard: usize, exists: bool) {
        if exists {
            self.stream_seen[shard].store(true, Ordering::Relaxed);
            return;
        }
        if self.stream_seen[shard].swap(false, Ordering::Relaxed) {
            self.stream_generation[shard].fetch_add(1, Ordering::Relaxed);
            self.ttl.forget(&sharded_stream_key(shard));
            M.redis_relay_stream_missing_total
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    async fn observe_redis_server(&self) {
        let mut conn = self.write_conn_handle();

        if let Ok(info) = redis::cmd("INFO")
            .arg("memory")
            .query_async::<String>(&mut conn)
            .await
        {
            if let Some(used) = redis_info_int64(&info, "used_memory") {
                M.redis_used_memory_bytes.store(used, Ordering::Relaxed);
            }
            if let Some(max) = redis_info_int64(&info, "maxmemory") {
                M.redis_max_memory_bytes.store(max, Ordering::Relaxed);
            }
        }
        if let Ok(info) = redis::cmd("INFO")
            .arg("stats")
            .query_async::<String>(&mut conn)
            .await
        {
            if let Some(evicted) = redis_info_int64(&info, "evicted_keys") {
                M.redis_evicted_keys.store(evicted, Ordering::Relaxed);
            }
        }
    }

    fn record_retention_error(&self, message: &str, err: &str, attrs: &[(&str, &str)]) {
        M.redis_relay_retention_errors
            .fetch_add(1, Ordering::Relaxed);
        M.set_redis_last_error(err);
        tracing::warn!(error = %err, attrs = ?attrs, "realtime/sharded-redis: {}", message);
    }

    /// Single XREAD iteration for one shard. Returns true to continue the
    /// loop; false exits because shutdown fired mid-error-backoff. last_id
    /// advances past every message read.
    async fn read_shard_once(
        &self,
        conn: &mut ConnectionManager,
        shard: usize,
        stream: &str,
        last_id: &mut String,
    ) -> bool {
        let block_ms = self.config.read_block.as_millis() as i64;
        let raw: Result<redis::Value, _> = redis::cmd("XREAD")
            .arg("COUNT")
            .arg(self.config.read_count)
            .arg("BLOCK")
            .arg(block_ms)
            .arg("STREAMS")
            .arg(stream)
            .arg(last_id.as_str())
            .query_async(conn)
            .await;

        match raw {
            // Nil reply = block timeout with no new messages.
            Ok(redis::Value::Nil) => true,
            Ok(value) => {
                for (_key, messages) in parse_xread_response(&value) {
                    for (id, fields) in messages {
                        *last_id = id.clone();
                        M.redis_xread_total.fetch_add(1, Ordering::Relaxed);
                        self.deliver_fields(&fields).await;
                    }
                }
                true
            }
            Err(e) => {
                M.redis_xread_errors.fetch_add(1, Ordering::Relaxed);
                M.set_redis_last_error(&e.to_string());
                tracing::warn!(
                    error = %e,
                    shard,
                    stream = %stream,
                    "realtime/sharded-redis: XREAD failed"
                );
                tokio::select! {
                    () = self.shutdown.cancelled() => false,
                    () = tokio::time::sleep(Duration::from_secs(1)) => true,
                }
            }
        }
    }

    async fn deliver_fields(&self, fields: &[(String, String)]) {
        if let Some(ev) = Envelope::from_field_pairs(fields) {
            if ev.scope.is_empty() || ev.scope_id.is_empty() {
                return;
            }
            let daemon_runtime = self
                .daemon_runtime
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            deliver_envelope(self.hub.clone(), daemon_runtime, ev).await;
        }
    }

    async fn heartbeat_loop(self: Arc<Self>) {
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
        let stamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true);
        let mut conn = self.write_conn_handle();
        let result = redis::cmd("SET")
            .arg(heartbeat_key(&self.node_id))
            .arg(stamp)
            .arg("EX")
            .arg(crate::envelope::HEARTBEAT_TTL_SECS)
            .query_async::<()>(&mut conn)
            .await;
        match result {
            Ok(()) => M.redis_connected.store(true, Ordering::Relaxed),
            Err(e) => {
                M.redis_connected.store(false, Ordering::Relaxed);
                M.set_redis_last_error(&e.to_string());
            }
        }
    }
}

/// One relay stream's decoded messages: `(stream_key, [(msg_id, field_pairs)])`.
type XReadStreamBatch = Vec<(String, Vec<(String, Vec<(String, String)>)>)>;

pub fn parse_xread_response(raw: &redis::Value) -> XReadStreamBatch {
    let mut out = Vec::new();
    let redis::Value::Array(streams) = raw else {
        return out;
    };
    for stream in streams {
        let redis::Value::Array(pair) = stream else {
            continue;
        };
        let (Some(key), Some(entries)) = (pair.first(), pair.get(1)) else {
            continue;
        };
        let key = bulk_str(key);
        let mut messages = Vec::new();
        if let redis::Value::Array(list) = entries {
            for msg in list {
                let redis::Value::Array(id_fields) = msg else {
                    continue;
                };
                let (Some(id), Some(fields)) = (id_fields.first(), id_fields.get(1)) else {
                    continue;
                };
                let mut kv = Vec::new();
                if let redis::Value::Array(fv) = fields {
                    let mut it = fv.iter();
                    while let (Some(k), Some(v)) = (it.next(), it.next()) {
                        kv.push((bulk_str(k), bulk_str(v)));
                    }
                }
                messages.push((bulk_str(id), kv));
            }
        }
        out.push((key, messages));
    }
    out
}

fn bulk_str(v: &redis::Value) -> String {
    match v {
        redis::Value::BulkString(b) => String::from_utf8_lossy(b).to_string(),
        _ => String::new(),
    }
}

#[async_trait]
impl<H: HubFanout + 'static> RelayPublisher for ShardedStreamRelay<H> {
    async fn publish_with_id(
        &self,
        scope_type: &str,
        scope_id: &str,
        exclude: &str,
        frame: &[u8],
        event_id: &str,
    ) -> anyhow::Result<()> {
        self.publish_with_id_inner(scope_type, scope_id, exclude, frame, event_id)
            .await
    }
}

#[async_trait]
impl<H: HubFanout + 'static> Broadcaster for ShardedStreamRelay<H> {
    async fn broadcast_to_scope(&self, scope_type: &str, scope_id: &str, message: &[u8]) {
        let id = ulid::Ulid::new().to_string();
        let _ = self
            .publish_with_id_inner(scope_type, scope_id, "", message, &id)
            .await;
    }

    async fn send_to_user(&self, user_id: &str, message: &[u8], exclude_workspace: Option<&str>) {
        let exclude = exclude_workspace.unwrap_or("");
        let id = ulid::Ulid::new().to_string();
        let _ = self
            .publish_with_id_inner(SCOPE_USER, user_id, exclude, message, &id)
            .await;
    }

    async fn broadcast(&self, message: &[u8]) {
        let id = ulid::Ulid::new().to_string();
        let _ = self
            .publish_with_id_inner("global", "all", "", message, &id)
            .await;
    }

    // broadcast_to_workspace inherits the default SCOPE_WORKSPACE delegation.
}

#[async_trait]
impl<H: HubFanout + 'static> crate::relay_lifecycle::ManagedRelay for ShardedStreamRelay<H> {
    fn node_id(&self) -> String {
        self.node_id()
    }

    fn start(self: Arc<Self>, shutdown: CancellationToken) {
        ShardedStreamRelay::start(&self);
        let relay = self.clone();
        let handle = tokio::spawn(async move {
            tokio::select! {
                () = shutdown.cancelled() => relay.stop(),
                () = relay.shutdown.cancelled() => {}
            }
        });
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(handle);
    }

    fn stop(&self) {
        ShardedStreamRelay::stop(self);
    }

    async fn wait(&self) {
        ShardedStreamRelay::wait(self).await;
    }

    fn set_daemon_runtime_deliverer(&self, deliverer: Arc<dyn DaemonRuntimeDeliverer>) {
        ShardedStreamRelay::set_daemon_runtime_deliverer(self, deliverer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broadcaster::SCOPE_WORKSPACE;

    #[test]
    fn shard_key_format_matches_go() {
        assert_eq!(sharded_stream_key(0), "ws:relay:shard:0");
        assert_eq!(sharded_stream_key(7), "ws:relay:shard:7");
    }

    #[test]
    fn shard_for_is_stable_and_in_range() {
        let cfg = ShardedStreamRelayConfig::default().with_defaults();
        assert_eq!(cfg.shards, 8);

        // Deterministic hashing without a live relay instance.
        let hash = fnv32a(&[SCOPE_WORKSPACE.as_bytes(), &[0], b"ws-1"]);
        let expected = (hash % 8) as usize;
        let again = fnv32a(&[SCOPE_WORKSPACE.as_bytes(), &[0], b"ws-1"]);
        assert_eq!((again % 8) as usize, expected);
        assert!(expected < 8);
    }

    #[test]
    fn fnv32a_matches_known_vectors() {
        // FNV-1a 32-bit of empty input is the offset basis.
        assert_eq!(fnv32a(&[&[]]), 0x811c_9dc5);
        // FNV-1a("a") = 0xe40c292c
        assert_eq!(fnv32a(&[b"a"]), 0xe40c_292c);
    }

    #[test]
    fn config_validation_rejects_bad_relationships() {
        let mut cfg = ShardedStreamRelayConfig::default().with_defaults();
        assert!(cfg.validate().is_ok());

        cfg.replay_grace = Duration::ZERO;
        assert!(cfg.validate().is_err());
    }

    #[tokio::test]
    async fn reader_connection_retries_initial_failure() {
        let shutdown = CancellationToken::new();
        let attempts = AtomicU64::new(0);
        let errors = AtomicU64::new(0);

        let connection = connect_with_retry(
            &shutdown,
            Duration::ZERO,
            || {
                let attempt = attempts.fetch_add(1, Ordering::Relaxed);
                async move {
                    if attempt == 0 {
                        Err("offline")
                    } else {
                        Ok("connected")
                    }
                }
            },
            |_| {
                errors.fetch_add(1, Ordering::Relaxed);
            },
        )
        .await;

        assert_eq!(connection, Some("connected"));
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
        assert_eq!(errors.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn cancelled_reader_does_not_attempt_connection() {
        let shutdown = CancellationToken::new();
        shutdown.cancel();
        let attempts = AtomicU64::new(0);

        let connection = connect_with_retry(
            &shutdown,
            Duration::ZERO,
            || {
                attempts.fetch_add(1, Ordering::Relaxed);
                std::future::ready(Result::<(), &str>::Err("offline"))
            },
            |_| {},
        )
        .await;

        assert_eq!(connection, None);
        assert_eq!(attempts.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn with_defaults_repairs_trim_vs_grace() {
        let cfg = ShardedStreamRelayConfig {
            trim_horizon: Duration::from_secs(60),
            replay_grace: Duration::from_secs(300),
            ..Default::default()
        }
        .with_defaults();
        // TrimHorizon <= ReplayGrace lifts TrimHorizon to 2x grace.
        assert_eq!(cfg.trim_horizon, Duration::from_secs(600));
        // TTL then lifts to trim + grace.
        assert_eq!(cfg.stream_ttl, Duration::from_secs(900));
    }

    #[test]
    fn xread_parser_reads_nested_value_tree() {
        use redis::Value;

        let raw = Value::Array(vec![Value::Array(vec![
            Value::BulkString(b"ws:relay:shard:3".to_vec()),
            Value::Array(vec![
                Value::Array(vec![
                    Value::BulkString(b"1700-0".to_vec()),
                    Value::Array(vec![
                        Value::BulkString(b"event_id".to_vec()),
                        Value::BulkString(b"evt-1".to_vec()),
                        Value::BulkString(b"payload_json".to_vec()),
                        Value::BulkString(br#"{"type":"t"}"#.to_vec()),
                    ]),
                ]),
                Value::Array(vec![
                    Value::BulkString(b"1701-0".to_vec()),
                    Value::Array(vec![
                        Value::BulkString(b"event_id".to_vec()),
                        Value::BulkString(b"evt-2".to_vec()),
                        Value::BulkString(b"payload_json".to_vec()),
                        Value::BulkString(br#"{"type":"u"}"#.to_vec()),
                    ]),
                ]),
            ]),
        ])]);

        let parsed = parse_xread_response(&raw);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "ws:relay:shard:3");
        assert_eq!(parsed[0].1.len(), 2);
        assert_eq!(parsed[0].1[0].0, "1700-0");
        assert_eq!(parsed[0].1[0].1[0].0, "event_id");
        assert_eq!(parsed[0].1[0].1[0].1, "evt-1");

        // Nil reply parses to no streams.
        assert!(parse_xread_response(&Value::Nil).is_empty());
    }

    #[tokio::test]
    async fn envelope_delivery_via_hub_fanout() {
        use crate::envelope::{deliver_envelope, Envelope};

        #[derive(Default)]
        struct RecordingHub {
            scopes: Mutex<Vec<(String, String)>>,
            users: Mutex<Vec<String>>,
            globals: Mutex<u32>,
        }

        #[async_trait]
        impl HubFanout for RecordingHub {
            async fn fanout_all_dedup(&self, _: &[u8], _: &str, _: &str) {
                *self.globals.lock().unwrap() += 1;
            }
            async fn fanout_user(&self, user_id: &str, _: &[u8], _: &str, _: &str) {
                self.users.lock().unwrap().push(user_id.to_string());
            }
            async fn broadcast_to_scope_dedup(
                &self,
                scope_type: &str,
                scope_id: &str,
                _: &[u8],
                _: &str,
            ) {
                self.scopes
                    .lock()
                    .unwrap()
                    .push((scope_type.to_string(), scope_id.to_string()));
            }
        }

        let hub = Arc::new(RecordingHub::default());

        // Workspace-scoped envelope routes to broadcast_to_scope_dedup.
        let ev = Envelope::new("n", "workspace", "ws-1", "", br#"{"type":"t"}"#, "e-1");
        deliver_envelope(hub.clone(), None, ev).await;
        assert_eq!(hub.scopes.lock().unwrap().len(), 1);

        // User-scoped routes to fanout_user.
        let ev = Envelope::new("n", SCOPE_USER, "u-1", "", br#"{"type":"t"}"#, "e-2");
        deliver_envelope(hub.clone(), None, ev).await;
        assert_eq!(hub.users.lock().unwrap().last().unwrap(), "u-1");

        // Global routes to fanout_all_dedup.
        let ev = Envelope::new("n", "global", "all", "", br#"{"type":"t"}"#, "e-3");
        deliver_envelope(hub.clone(), None, ev).await;
        assert_eq!(*hub.globals.lock().unwrap(), 1);
    }
}
