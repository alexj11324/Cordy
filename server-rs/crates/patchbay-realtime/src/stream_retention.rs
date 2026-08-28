//! Stream retention config and TTL maintenance.
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use redis::aio::ConnectionManager;

const DEFAULT_RELAY_STREAM_TRIM_HORIZON: Duration = Duration::from_secs(10 * 60);
const DEFAULT_RELAY_STREAM_TTL: Duration = Duration::from_secs(15 * 60);
const DEFAULT_RELAY_STREAM_TTL_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_RELAY_STREAM_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60);

/// Defined in sharded_stream_relay.go on the Go side; hoisted here because the
/// retention defaults depend on it.
pub const DEFAULT_SHARDED_RELAY_STREAM_MAX_LEN: i64 = 2000;
/// Defined in sharded_stream_relay.go on the Go side; see note above.
pub const DEFAULT_SHARDED_RELAY_REPLAY_GRACE: Duration = Duration::from_secs(5 * 60);

/// Shared by sharded and legacy relay modes so one set of operator controls
/// has the same meaning during a dual-mode rollout. TTL is deliberately
/// opt-in: deploy the compatible code with TTL disabled, then enable it only
/// after every replica can refresh or remove expirations.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamRetentionConfig {
    pub stream_max_len: i64,
    pub trim_horizon: Duration,
    pub stream_ttl: Duration,
    pub ttl_refresh_interval: Duration,
    pub maintenance_interval: Duration,
    pub stream_ttl_enabled: bool,
}

impl Default for StreamRetentionConfig {
    fn default() -> Self {
        Self {
            stream_max_len: DEFAULT_SHARDED_RELAY_STREAM_MAX_LEN,
            trim_horizon: DEFAULT_RELAY_STREAM_TRIM_HORIZON,
            stream_ttl: DEFAULT_RELAY_STREAM_TTL,
            ttl_refresh_interval: DEFAULT_RELAY_STREAM_TTL_REFRESH_INTERVAL,
            maintenance_interval: DEFAULT_RELAY_STREAM_MAINTENANCE_INTERVAL,
            stream_ttl_enabled: false,
        }
    }
}

impl StreamRetentionConfig {
    /// Fills zero/invalid fields with safe cross-mode defaults.
    pub fn with_defaults(mut self) -> Self {
        let def = Self::default();
        if self.stream_max_len <= 0 {
            self.stream_max_len = def.stream_max_len;
        }
        if self.trim_horizon.is_zero() {
            self.trim_horizon = def.trim_horizon;
        }
        if self.stream_ttl < self.trim_horizon {
            self.stream_ttl = self.trim_horizon + DEFAULT_SHARDED_RELAY_REPLAY_GRACE;
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
}

/// Picks a TTL-refresh cadence: keep the operator's preference when it fits
/// strictly inside the TTL, otherwise fall back to a third of it.
pub fn retention_subinterval(ttl: Duration, preferred: Duration) -> Duration {
    if !preferred.is_zero() && preferred < ttl {
        return preferred;
    }
    let interval = ttl / 3;
    if interval.is_zero() {
        return Duration::from_nanos(1);
    }
    interval
}

type NowFn = Box<dyn Fn() -> Instant + Send + Sync>;

/// Limits PEXPIRE calls on the publish path while ensuring active stream keys
/// remain eligible for volatile-* eviction policies. A maintenance pass
/// repairs any TTL that was missed after a partial failure.
pub struct StreamTtlRefresher {
    ttl: Duration,
    refresh_every: Duration,
    now: NowFn,

    last_refresh: Mutex<HashMap<String, Instant>>,
}

impl StreamTtlRefresher {
    pub fn new(ttl: Duration, refresh_every: Duration) -> Self {
        Self {
            ttl,
            refresh_every,
            now: Box::new(Instant::now),
            last_refresh: Mutex::new(HashMap::new()),
        }
    }

    #[cfg(test)]
    fn with_now(mut self, now: NowFn) -> Self {
        self.now = now;
        self
    }

    fn tick(&self) -> Instant {
        (self.now)()
    }

    /// Refreshes the key's TTL at most once per `refresh_every` window.
    pub async fn refresh_if_due(
        &self,
        conn: &mut ConnectionManager,
        key: &str,
    ) -> anyhow::Result<()> {
        let now = self.tick();
        if !self.claim_refresh(key, now) {
            return Ok(());
        }

        let ok: bool = match redis::cmd("PEXPIRE")
            .arg(key)
            .arg(self.ttl.as_millis() as i64)
            .query_async(conn)
            .await
        {
            Ok(ok) => ok,
            Err(e) => {
                self.release_refresh(key, now);
                return Err(e.into());
            }
        };
        if !ok {
            self.release_refresh(key, now);
            anyhow::bail!("stream {key:?} disappeared before TTL refresh");
        }
        Ok(())
    }

    /// Assigns a TTL only when a stream exists without one. It intentionally
    /// does not refresh a healthy TTL, so an idle stream can expire.
    pub async fn repair_missing_ttl(
        &self,
        conn: &mut ConnectionManager,
        key: &str,
    ) -> anyhow::Result<i64> {
        self.reconcile_ttl(conn, key, true).await
    }

    /// Repairs a missing TTL when enabled and removes any persisted TTL when
    /// disabled. The disabled path is the compatibility and rollback phase:
    /// once all new replicas have observed it, old binaries can keep writing
    /// without an inherited expiry deleting an active stream.
    ///
    /// Returns the PTTL value in milliseconds; Redis sentinel values pass
    /// through (-2 = key missing, -1 = no expiry).
    pub async fn reconcile_ttl(
        &self,
        conn: &mut ConnectionManager,
        key: &str,
        enabled: bool,
    ) -> anyhow::Result<i64> {
        let ttl_millis: i64 = redis::cmd("PTTL").arg(key).query_async(conn).await?;
        if !enabled {
            self.forget(key);
            if ttl_millis == -2 || ttl_millis == -1 {
                return Ok(ttl_millis);
            }
            let ok: bool = redis::cmd("PERSIST").arg(key).query_async(conn).await?;
            if !ok {
                return Ok(-2);
            }
            return Ok(-1);
        }
        match ttl_millis {
            // Key does not exist.
            -2 => Ok(-2),
            // Key exists without expiry — adopt it into the managed window.
            -1 => {
                let ok: bool = redis::cmd("PEXPIRE")
                    .arg(key)
                    .arg(self.ttl.as_millis() as i64)
                    .query_async(conn)
                    .await?;
                if !ok {
                    return Ok(-2);
                }
                self.last_refresh
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key.to_string(), self.tick());
                Ok(self.ttl.as_millis() as i64)
            }
            other => Ok(other),
        }
    }

    /// Drops bookkeeping entries last touched before `before`.
    pub fn forget_stale(&self, before: Instant) {
        let mut guard = self.last_refresh.lock().unwrap_or_else(|e| e.into_inner());
        guard.retain(|_, refreshed_at| *refreshed_at >= before);
    }

    pub fn forget(&self, key: &str) {
        self.last_refresh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }

    /// Claims the right to refresh `key` now; false when a refresh happened
    /// within the current window.
    fn claim_refresh(&self, key: &str, now: Instant) -> bool {
        let mut guard = self.last_refresh.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(last) = guard.get(key) {
            if now.duration_since(*last) < self.refresh_every {
                return false;
            }
        }
        guard.insert(key.to_string(), now);
        true
    }

    /// Rolls back a claim whose Redis write failed — but only if nothing else
    /// claimed it in the meantime.
    fn release_refresh(&self, key: &str, claimed_at: Instant) {
        let mut guard = self.last_refresh.lock().unwrap_or_else(|e| e.into_inner());
        if guard.get(key) == Some(&claimed_at) {
            guard.remove(key);
        }
    }
}

/// Redis stream ID marking the trim horizon: `{millis}-0`, clamped to 0.
pub fn stream_min_id(now: SystemTime, horizon: Duration) -> String {
    let base = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let millis = (base - horizon.as_millis() as i64).max(0);
    format!("{millis}-0")
}

/// Converts a PTTL result to milliseconds. Sentinel values (-2 missing,
/// -1 no expiry) pass through unchanged; with redis-rs returning raw
/// milliseconds already, this is the identity kept for call-site parity.
pub fn redis_ttl_millis(ttl_millis: i64) -> i64 {
    ttl_millis
}

/// Extracts an integer field from Redis INFO text output ("key:value" lines).
pub fn redis_info_int64(info: &str, key: &str) -> Option<i64> {
    let prefix = format!("{key}:");
    for line in info.split('\n') {
        let line = line.trim();
        if !line.starts_with(&prefix) {
            continue;
        }
        return line[prefix.len()..].trim().parse().ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fill_invalid_fields() {
        let cfg = StreamRetentionConfig::default().with_defaults();
        assert_eq!(cfg.stream_max_len, DEFAULT_SHARDED_RELAY_STREAM_MAX_LEN);
        assert_eq!(cfg.trim_horizon, DEFAULT_RELAY_STREAM_TRIM_HORIZON);
        // TTL (15m) > trim horizon (10m), so it survives unchanged.
        assert_eq!(cfg.stream_ttl, DEFAULT_RELAY_STREAM_TTL);
        assert!(!cfg.stream_ttl_enabled);
    }

    #[test]
    fn ttl_below_trim_horizon_gets_grace_added() {
        let cfg = StreamRetentionConfig {
            stream_ttl: Duration::from_secs(60),
            ..Default::default()
        }
        .with_defaults();
        assert_eq!(
            cfg.stream_ttl,
            DEFAULT_RELAY_STREAM_TRIM_HORIZON + DEFAULT_SHARDED_RELAY_REPLAY_GRACE
        );
    }

    #[test]
    fn invalid_intervals_fall_back_to_third_of_ttl() {
        let cfg = StreamRetentionConfig {
            ttl_refresh_interval: Duration::ZERO,
            maintenance_interval: Duration::from_secs(3600), // >= TTL (15m)
            ..Default::default()
        }
        .with_defaults();

        // Default preferences (30s refresh / 60s maintenance) are valid
        // against the 15m TTL, so they are kept rather than ttl/3.
        assert_eq!(cfg.ttl_refresh_interval, Duration::from_secs(30));
        assert_eq!(cfg.maintenance_interval, Duration::from_secs(60));

        // Order matters: a tiny TTL is FIRST lifted to trim_horizon+grace,
        // so by the time intervals are validated the default preferences are
        // valid again — matching the Go implementation's sequencing.
        let tiny = StreamRetentionConfig {
            stream_ttl: Duration::from_secs(20),
            ..Default::default()
        }
        .with_defaults();
        assert_eq!(
            tiny.stream_ttl,
            DEFAULT_RELAY_STREAM_TRIM_HORIZON + DEFAULT_SHARDED_RELAY_REPLAY_GRACE
        );
        assert_eq!(tiny.ttl_refresh_interval, Duration::from_secs(30));

        // Genuine ttl/3 fallback needs a small TTL *and* a small trim horizon.
        let tiny_trim = StreamRetentionConfig {
            stream_ttl: Duration::from_secs(20),
            trim_horizon: Duration::from_secs(10),
            ..Default::default()
        }
        .with_defaults();
        assert_eq!(tiny_trim.stream_ttl, Duration::from_secs(20));
        assert_eq!(tiny_trim.ttl_refresh_interval, Duration::from_secs(20) / 3);
    }

    #[test]
    fn valid_operator_intervals_are_kept() {
        let cfg = StreamRetentionConfig {
            ttl_refresh_interval: Duration::from_secs(45),
            maintenance_interval: Duration::from_secs(120),
            ..Default::default()
        }
        .with_defaults();
        assert_eq!(cfg.ttl_refresh_interval, Duration::from_secs(45));
        assert_eq!(cfg.maintenance_interval, Duration::from_secs(120));
    }

    #[test]
    fn subinterval_prefers_valid_preference() {
        let ttl = Duration::from_secs(900);
        assert_eq!(
            retention_subinterval(ttl, Duration::from_secs(30)),
            Duration::from_secs(30)
        );
        // Preference >= ttl falls back to a third.
        assert_eq!(retention_subinterval(ttl, ttl), ttl / 3);
        // Zero preference falls back too.
        assert_eq!(retention_subinterval(ttl, Duration::ZERO), ttl / 3);
    }

    #[test]
    fn claim_refresh_windows_repeat_calls() {
        let start = Instant::now();
        let clock_start = start;
        let refresher = StreamTtlRefresher::new(Duration::from_secs(900), Duration::from_secs(30))
            .with_now(Box::new(move || clock_start));

        // With a frozen clock the first claim wins and repeats are denied.
        assert!(refresher.claim_refresh("k", refresher.tick()));
        assert!(!refresher.claim_refresh("k", refresher.tick()));
        refresher.forget("k");
        assert!(refresher.claim_refresh("k", refresher.tick()));
    }

    #[test]
    fn release_refresh_only_rolls_back_own_claim() {
        let refresher = StreamTtlRefresher::new(Duration::from_secs(900), Duration::from_secs(30));
        let t0 = refresher.tick();
        assert!(refresher.claim_refresh("k", t0));
        // Releasing with a different timestamp must not remove the entry.
        refresher.release_refresh("k", t0 + Duration::from_secs(1));
        assert!(!refresher.claim_refresh("k", refresher.tick()));
        // Releasing with the original timestamp removes it.
        refresher.release_refresh("k", t0);
        assert!(refresher.claim_refresh("k", refresher.tick()));
    }

    #[test]
    fn forget_stale_prunes_old_entries() {
        let refresher = StreamTtlRefresher::new(Duration::from_secs(900), Duration::from_secs(30));
        let t0 = refresher.tick();
        refresher.claim_refresh("old", t0);
        refresher.claim_refresh("new", t0 + Duration::from_secs(600));

        refresher.forget_stale(t0 + Duration::from_secs(300));

        let guard = refresher.last_refresh.lock().unwrap();
        assert!(!guard.contains_key("old"));
        assert!(guard.contains_key("new"));
    }

    #[test]
    fn min_id_formats_and_clamps_negative() {
        let epoch = SystemTime::UNIX_EPOCH;
        let now = epoch + Duration::from_secs(1_700_000_000);
        // UnixMilli semantics: milliseconds since epoch minus horizon.
        let expected_ms = 1_700_000_000i64 * 1000 - 60_000;
        assert_eq!(
            stream_min_id(now, Duration::from_secs(60)),
            format!("{expected_ms}-0")
        );
        // Horizon larger than the timestamp clamps to 0.
        assert_eq!(stream_min_id(epoch, Duration::from_secs(60)), "0-0");
    }

    #[test]
    fn info_parser_reads_first_matching_line() {
        let info = "# Server\r\nredis_version:7.2\nused_memory:1048576\nother_used_memory:9\n";
        assert_eq!(redis_info_int64(info, "used_memory"), Some(1_048_576));
        assert_eq!(redis_info_int64(info, "missing_key"), None);
        assert_eq!(redis_info_int64(info, "redis_version"), None);
    }

    #[test]
    fn ttl_millis_passthrough_keeps_sentinels() {
        assert_eq!(redis_ttl_millis(-2), -2);
        assert_eq!(redis_ttl_millis(-1), -1);
        assert_eq!(redis_ttl_millis(90_000), 90_000);
    }
}
