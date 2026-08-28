//! Lightweight realtime subsystem counters.
//!
//! Phase 1 (MUL-1138) extends the phase-0 counter set with subscribe / Redis /
//! per-scope-room counters. We keep using std-library atomics rather than a
//! Prometheus dependency; a future phase can re-export the same numbers.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use serde_json::json;

/// Latest low-frequency retention sample for one relay stream. PTTL millis
/// uses Redis sentinel values: -1 means no expiry and -2 means the key does
/// not exist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct RedisStreamObservation {
    pub entries: i64,
    pub memory_bytes: i64,
    pub pttl_millis: i64,
}

type CounterMap = Mutex<HashMap<String, Arc<AtomicI64>>>;

/// Collects lightweight counters describing the realtime subsystem.
///
/// Field names mirror the Go struct; the JSON keys in [`Metrics::snapshot`]
/// match Go's exactly so dashboards keep working across the cutover.
pub struct Metrics {
    pub connects_total: AtomicI64,
    pub disconnects_total: AtomicI64,
    pub active_connections: AtomicI64,
    pub slow_evictions_total: AtomicI64,
    pub messages_sent_total: AtomicI64,
    pub messages_dropped_total: AtomicI64,

    /// Counts connections closed because a peer sent a message over the
    /// inbound read limit, on either the pre-auth or post-auth read path.
    pub inbound_too_large_total: AtomicI64,

    event_sent: CounterMap,
    subscribe_total: CounterMap,
    unsubscribe_total: CounterMap,
    subscribe_denied_total: CounterMap,
    scope_rooms: CounterMap,

    /// Redis relay counters. Zero unless the Redis broadcaster is enabled.
    pub redis_xadd_total: AtomicI64,
    pub redis_xadd_errors: AtomicI64,
    pub redis_xread_total: AtomicI64,
    pub redis_xread_errors: AtomicI64,
    pub redis_ack_total: AtomicI64,
    pub redis_last_xadd_lag_micros: AtomicI64,
    pub redis_mirror_primary_errors: AtomicI64,
    pub redis_mirror_secondary_errors: AtomicI64,
    pub redis_mirror_divergence_total: AtomicI64,
    pub redis_relay_stream_trimmed_total: AtomicI64,
    pub redis_relay_stream_missing_total: AtomicI64,
    pub redis_relay_retention_errors: AtomicI64,
    pub redis_relay_streams_without_ttl: AtomicI64,
    pub redis_used_memory_bytes: AtomicI64,
    pub redis_max_memory_bytes: AtomicI64,
    pub redis_evicted_keys: AtomicI64,

    redis_streams: Mutex<BTreeMap<String, RedisStreamObservation>>,
    redis_streams_without_ttl_by_relay: Mutex<HashMap<String, i64>>,

    /// Set by the relay on startup / reconnect.
    pub redis_connected: AtomicBool,
    redis_last_err: RwLock<String>,
    /// Set once at boot by the relay (or empty in single-node mode).
    pub node_id: RwLock<String>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            connects_total: AtomicI64::new(0),
            disconnects_total: AtomicI64::new(0),
            active_connections: AtomicI64::new(0),
            slow_evictions_total: AtomicI64::new(0),
            messages_sent_total: AtomicI64::new(0),
            messages_dropped_total: AtomicI64::new(0),
            inbound_too_large_total: AtomicI64::new(0),
            event_sent: Mutex::new(HashMap::new()),
            subscribe_total: Mutex::new(HashMap::new()),
            unsubscribe_total: Mutex::new(HashMap::new()),
            subscribe_denied_total: Mutex::new(HashMap::new()),
            scope_rooms: Mutex::new(HashMap::new()),
            redis_xadd_total: AtomicI64::new(0),
            redis_xadd_errors: AtomicI64::new(0),
            redis_xread_total: AtomicI64::new(0),
            redis_xread_errors: AtomicI64::new(0),
            redis_ack_total: AtomicI64::new(0),
            redis_last_xadd_lag_micros: AtomicI64::new(0),
            redis_mirror_primary_errors: AtomicI64::new(0),
            redis_mirror_secondary_errors: AtomicI64::new(0),
            redis_mirror_divergence_total: AtomicI64::new(0),
            redis_relay_stream_trimmed_total: AtomicI64::new(0),
            redis_relay_stream_missing_total: AtomicI64::new(0),
            redis_relay_retention_errors: AtomicI64::new(0),
            redis_relay_streams_without_ttl: AtomicI64::new(0),
            redis_used_memory_bytes: AtomicI64::new(0),
            redis_max_memory_bytes: AtomicI64::new(0),
            redis_evicted_keys: AtomicI64::new(0),
            redis_streams: Mutex::new(BTreeMap::new()),
            redis_streams_without_ttl_by_relay: Mutex::new(HashMap::new()),
            redis_connected: AtomicBool::new(false),
            redis_last_err: RwLock::new(String::new()),
            node_id: RwLock::new(String::new()),
        }
    }
}

/// Package-level metrics singleton (Go's `var M = &Metrics{}`).
pub static M: LazyLock<Metrics> = LazyLock::new(Metrics::default);

fn counter_slot(map: &CounterMap, key: &str) -> Arc<AtomicI64> {
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .entry(key.to_string())
        .or_insert_with(|| Arc::new(AtomicI64::new(0)))
        .clone()
}

fn snapshot_counters(map: &CounterMap) -> BTreeMap<String, i64> {
    map.lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(k, v)| (k.clone(), v.load(Ordering::Relaxed)))
        .collect()
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Increments the per-event-type send counter. Empty type is ignored.
    pub fn record_event(&self, event_type: &str) {
        if event_type.is_empty() {
            return;
        }
        counter_slot(&self.event_sent, event_type).fetch_add(1, Ordering::Relaxed);
    }

    /// Per-scope-type counter for successful subscribes.
    pub fn subscribes_total(&self, scope_type: &str) -> Arc<AtomicI64> {
        counter_slot(&self.subscribe_total, scope_type)
    }

    /// Per-scope-type counter for unsubscribes.
    pub fn unsubscribes_total(&self, scope_type: &str) -> Arc<AtomicI64> {
        counter_slot(&self.unsubscribe_total, scope_type)
    }

    /// Per-scope-type counter for denied subscribes.
    pub fn subscribe_denied_total(&self, scope_type: &str) -> Arc<AtomicI64> {
        counter_slot(&self.subscribe_denied_total, scope_type)
    }

    /// Adjusts the active-rooms gauge for `scope_type`.
    pub fn inc_room(&self, scope_type: &str) {
        counter_slot(&self.scope_rooms, scope_type).fetch_add(1, Ordering::Relaxed);
    }

    /// Adjusts the active-rooms gauge for `scope_type`.
    pub fn dec_room(&self, scope_type: &str) {
        counter_slot(&self.scope_rooms, scope_type).fetch_add(-1, Ordering::Relaxed);
    }

    /// Stores msg as the most recent Redis consumer error. An empty msg clears it.
    pub fn set_redis_last_error(&self, msg: &str) {
        *self
            .redis_last_err
            .write()
            .unwrap_or_else(|e| e.into_inner()) = msg.to_string();
    }

    /// Most recent Redis consumer error message.
    pub fn last_redis_err(&self) -> String {
        self.redis_last_err
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Replaces the latest sampled statistics for `stream`.
    pub fn observe_redis_stream(
        &self,
        stream: &str,
        entries: i64,
        memory_bytes: i64,
        pttl_millis: i64,
    ) {
        self.redis_streams
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                stream.to_string(),
                RedisStreamObservation {
                    entries,
                    memory_bytes,
                    pttl_millis,
                },
            );
    }

    /// Returns a copy safe for metrics collection (sorted by stream name).
    pub fn redis_stream_observations(&self) -> BTreeMap<String, RedisStreamObservation> {
        self.redis_streams
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Updates one relay mode's latest count and exposes the process-wide sum.
    /// Keeping per-mode state prevents dual-mode collectors from overwriting
    /// each other with whichever maintenance loop ran last.
    pub fn set_redis_streams_without_ttl(&self, relay: &str, count: i64) {
        let total;
        {
            let mut guard = self
                .redis_streams_without_ttl_by_relay
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            guard.insert(relay.to_string(), count);
            total = guard.values().sum();
        }
        self.redis_relay_streams_without_ttl
            .store(total, Ordering::Relaxed);
    }

    /// JSON-friendly copy of the current counter values. Key names match the
    /// Go implementation byte-for-byte.
    pub fn snapshot(&self) -> serde_json::Value {
        let node_id = self
            .node_id
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        json!({
            "connects_total": self.connects_total.load(Ordering::Relaxed),
            "disconnects_total": self.disconnects_total.load(Ordering::Relaxed),
            "active_connections": self.active_connections.load(Ordering::Relaxed),
            "slow_evictions_total": self.slow_evictions_total.load(Ordering::Relaxed),
            "messages_sent_total": self.messages_sent_total.load(Ordering::Relaxed),
            "messages_dropped_total": self.messages_dropped_total.load(Ordering::Relaxed),
            "inbound_too_large_total": self.inbound_too_large_total.load(Ordering::Relaxed),
            "events_sent_by_type": snapshot_counters(&self.event_sent),
            "subscribes_total": snapshot_counters(&self.subscribe_total),
            "unsubscribes_total": snapshot_counters(&self.unsubscribe_total),
            "subscribe_denied_total": snapshot_counters(&self.subscribe_denied_total),
            "active_scope_rooms": snapshot_counters(&self.scope_rooms),
            "redis": {
                "connected": self.redis_connected.load(Ordering::Relaxed),
                "node_id": node_id,
                "xadd_total": self.redis_xadd_total.load(Ordering::Relaxed),
                "xadd_errors": self.redis_xadd_errors.load(Ordering::Relaxed),
                "xread_total": self.redis_xread_total.load(Ordering::Relaxed),
                "xread_errors": self.redis_xread_errors.load(Ordering::Relaxed),
                "ack_total": self.redis_ack_total.load(Ordering::Relaxed),
                "last_xadd_lag_micros": self.redis_last_xadd_lag_micros.load(Ordering::Relaxed),
                "mirror_primary_errors": self.redis_mirror_primary_errors.load(Ordering::Relaxed),
                "mirror_secondary_errors": self.redis_mirror_secondary_errors.load(Ordering::Relaxed),
                "mirror_divergence_total": self.redis_mirror_divergence_total.load(Ordering::Relaxed),
                "stream_trimmed_total": self.redis_relay_stream_trimmed_total.load(Ordering::Relaxed),
                "stream_missing_total": self.redis_relay_stream_missing_total.load(Ordering::Relaxed),
                "retention_errors": self.redis_relay_retention_errors.load(Ordering::Relaxed),
                "streams_without_ttl": self.redis_relay_streams_without_ttl.load(Ordering::Relaxed),
                "used_memory_bytes": self.redis_used_memory_bytes.load(Ordering::Relaxed),
                "max_memory_bytes": self.redis_max_memory_bytes.load(Ordering::Relaxed),
                "evicted_keys": self.redis_evicted_keys.load(Ordering::Relaxed),
                "streams": self.redis_stream_observations(),
                "last_error": self.last_redis_err(),
            },
        })
    }

    /// Zeroes all counters. Tests only.
    pub fn reset(&self) {
        for counter in [
            &self.connects_total,
            &self.disconnects_total,
            &self.active_connections,
            &self.slow_evictions_total,
            &self.messages_sent_total,
            &self.messages_dropped_total,
            &self.inbound_too_large_total,
            &self.redis_xadd_total,
            &self.redis_xadd_errors,
            &self.redis_xread_total,
            &self.redis_xread_errors,
            &self.redis_ack_total,
            &self.redis_last_xadd_lag_micros,
            &self.redis_mirror_primary_errors,
            &self.redis_mirror_secondary_errors,
            &self.redis_mirror_divergence_total,
            &self.redis_relay_stream_trimmed_total,
            &self.redis_relay_stream_missing_total,
            &self.redis_relay_retention_errors,
            &self.redis_relay_streams_without_ttl,
            &self.redis_used_memory_bytes,
            &self.redis_max_memory_bytes,
            &self.redis_evicted_keys,
        ] {
            counter.store(0, Ordering::Relaxed);
        }
        for map in [
            &self.event_sent,
            &self.subscribe_total,
            &self.unsubscribe_total,
            &self.subscribe_denied_total,
            &self.scope_rooms,
        ] {
            map.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
        self.redis_streams
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.redis_streams_without_ttl_by_relay
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.redis_connected.store(false, Ordering::Relaxed);
        self.set_redis_last_error("");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_event_increments_per_type() {
        let m = Metrics::new();
        m.record_event("issue:created");
        m.record_event("issue:created");
        m.record_event("issue:updated");
        m.record_event(""); // empty is ignored

        let snap = m.snapshot();
        assert_eq!(snap["events_sent_by_type"]["issue:created"], 2);
        assert_eq!(snap["events_sent_by_type"]["issue:updated"], 1);
    }

    #[test]
    fn room_gauges_adjust() {
        let m = Metrics::new();
        m.inc_room("workspace");
        m.inc_room("workspace");
        m.dec_room("workspace");

        let snap = m.snapshot();
        assert_eq!(snap["active_scope_rooms"]["workspace"], 1);
    }

    #[test]
    fn streams_without_ttl_sums_across_relays() {
        let m = Metrics::new();
        m.set_redis_streams_without_ttl("sharded", 3);
        m.set_redis_streams_without_ttl("legacy", 2);
        assert_eq!(m.redis_relay_streams_without_ttl.load(Ordering::Relaxed), 5);
        // Same relay overwrites instead of accumulating.
        m.set_redis_streams_without_ttl("sharded", 1);
        assert_eq!(m.redis_relay_streams_without_ttl.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn snapshot_exposes_redis_section_and_reset_clears_it() {
        let m = Metrics::new();
        m.redis_xadd_total.store(7, Ordering::Relaxed);
        m.observe_redis_stream("ws:1", 10, 2048, -1);
        m.set_redis_last_error("boom");

        let snap = m.snapshot();
        assert_eq!(snap["redis"]["xadd_total"], 7);
        assert_eq!(snap["redis"]["last_error"], "boom");
        assert_eq!(snap["redis"]["streams"]["ws:1"]["entries"], 10);

        m.reset();
        let snap = m.snapshot();
        assert_eq!(snap["redis"]["xadd_total"], 0);
        assert_eq!(snap["redis"]["last_error"], "");
        assert_eq!(
            snap["redis"]["streams"].as_object().map(|o| o.len()),
            Some(0)
        );
    }
}
