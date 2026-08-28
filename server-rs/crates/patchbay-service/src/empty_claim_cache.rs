//! Empty-claim cache.
//!
//! Caches "this runtime currently has no queued task" so the daemon's
//! poll-based claim path can short-circuit before hitting Postgres. Only the
//! negative result is cached; positive results always re-check the DB so
//! concurrent claimers race fairly in ClaimAgentTask's FOR UPDATE SKIP LOCKED.
//!
//! The verdict is tagged with a per-runtime invalidation version that every
//! enqueue bumps before waking the daemon — closing the slow-claim race where
//! an empty verdict written AFTER an enqueue would otherwise stall the queued
//! task until TTL.

use std::time::Duration;

use patchbay_redis::RecoveringConnection;

/// Bounds how long a cached "no queued task" verdict stays believable.
/// Enqueue invalidates by bumping the per-runtime version before waking the
/// daemon; the TTL remains the safety net for a missed invalidation (e.g. a
/// transient Redis failure during bump).
pub const EMPTY_CLAIM_CACHE_TTL: Duration = Duration::from_secs(3 * 60);

/// Keeps the version counter alive long enough that a rarely-polled runtime
/// doesn't reset to 0 between an enqueue's INCR and the next claim's GET
/// (which would let a stale tagged empty key suddenly look valid again).
const EMPTY_CLAIM_VERSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Caps every Redis call from this cache. Enqueue paths use a detached
/// context so the cache outlives the request, but a wedged Redis must not
/// stall enqueue indefinitely — bound the blast radius and degrade to "no
/// cache" instead.
const EMPTY_CLAIM_REDIS_TIMEOUT: Duration = Duration::from_millis(250);

const EMPTY_CLAIM_CACHE_PREFIX: &str = "patchbay:claim:runtime:empty:";
const EMPTY_CLAIM_VERSION_PREFIX: &str = "patchbay:claim:runtime:version:";

fn empty_claim_key(runtime_id: &str) -> String {
    format!("{EMPTY_CLAIM_CACHE_PREFIX}{runtime_id}")
}

fn empty_claim_version(runtime_id: &str) -> String {
    format!("{EMPTY_CLAIM_VERSION_PREFIX}{runtime_id}")
}

/// Negative-result cache over Redis. Callers hold
/// `Option<EmptyClaimCache>`: single-node dev / tests with no REDIS_URL
/// leave it unset and degrade cleanly to direct DB lookups, mirroring Go's
/// nil-receiver safety.
#[derive(Clone)]
pub struct EmptyClaimCache {
    rdb: Option<RecoveringConnection>,
}

impl EmptyClaimCache {
    pub fn new(rdb: RecoveringConnection) -> Self {
        Self { rdb: Some(rdb) }
    }

    /// Disabled cache used when Redis is not configured or unavailable.
    pub fn disabled() -> Self {
        Self { rdb: None }
    }

    /// Returns the runtime's current invalidation version. Callers MUST read
    /// this BEFORE the DB SELECT they are about to cache, then pass it back
    /// to [`mark_empty`](Self::mark_empty) so a concurrent bump invalidates
    /// the would-be cache write. Returns 0 ("unknown") on miss or any Redis
    /// error — the caller falls through to the DB path.
    ///
    /// The version key read refreshes its sliding TTL so a long-idle runtime
    /// doesn't let the counter expire and reset between an enqueue's bump and
    /// the next claim.
    pub async fn current_version(&self, runtime_id: &str) -> i64 {
        let Some(rdb) = self.rdb.as_ref() else {
            return 0;
        };
        if runtime_id.is_empty() {
            return 0;
        }
        let key = empty_claim_version(runtime_id);
        let outcome = tokio::time::timeout(EMPTY_CLAIM_REDIS_TIMEOUT, async {
            let mut con = rdb.clone();
            let mut get = redis::cmd("GET");
            get.arg(&key);
            let v: Option<i64> = get.query_async(&mut con).await?;
            // Refresh TTL so the counter doesn't expire and reset on a
            // low-traffic runtime. Best-effort: the result is ignored.
            if v.is_some() {
                let mut expire = redis::cmd("EXPIRE");
                expire.arg(&key).arg(EMPTY_CLAIM_VERSION_TTL.as_secs());
                let _: Result<(), redis::RedisError> = expire.query_async(&mut con).await;
            }
            Ok::<_, redis::RedisError>(v)
        })
        .await;
        match outcome {
            Ok(Ok(Some(v))) => v,
            // Nil reply = never bumped; silence matches Go's redis.Nil branch.
            Ok(Ok(None)) => 0,
            Ok(Err(err)) => {
                tracing::warn!(
                    error = %err,
                    "empty_claim_cache: version get failed; falling back to DB"
                );
                0
            }
            Err(_) => {
                tracing::warn!("empty_claim_cache: version get timed out; falling back to DB");
                0
            }
        }
    }

    /// True only when (a) an empty verdict is cached AND (b) it carries the
    /// runtime's current version. A stale verdict written before a concurrent
    /// bump reads as false so the caller falls through to the DB.
    pub async fn is_empty(&self, runtime_id: &str) -> bool {
        let Some(rdb) = self.rdb.as_ref() else {
            return false;
        };
        if runtime_id.is_empty() {
            return false;
        }
        let mut con = rdb.clone();
        let mut mget = redis::cmd("MGET");
        mget.arg(empty_claim_key(runtime_id))
            .arg(empty_claim_version(runtime_id));
        let outcome = tokio::time::timeout(
            EMPTY_CLAIM_REDIS_TIMEOUT,
            mget.query_async::<(Option<String>, Option<String>)>(&mut con),
        )
        .await;
        let vals = match outcome {
            Ok(Ok(vals)) => vals,
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "empty_claim_cache: mget failed; falling back to DB");
                return false;
            }
            Err(_) => {
                tracing::warn!("empty_claim_cache: mget timed out; falling back to DB");
                return false;
            }
        };
        let Some(empty_ver) = vals.0 else {
            return false;
        };
        // A missing version key means "no enqueue has ever bumped this
        // runtime", logically version 0 — the same value current_version
        // returns on miss. A mark_empty written with v=0 must match here,
        // otherwise the fast path would never trigger for fresh runtimes.
        let cur_ver = vals.1.unwrap_or_else(|| "0".to_string());
        empty_ver == cur_ver
    }

    /// Stores the empty verdict tagged with `observed_version` (the value
    /// returned by [`current_version`](Self::current_version) BEFORE the
    /// SELECT that confirmed emptiness). A concurrent bump between the two
    /// makes the next reader reject this entry. Errors log and swallow — a
    /// cache write failure is not a request failure.
    pub async fn mark_empty(&self, runtime_id: &str, observed_version: i64) {
        let Some(rdb) = self.rdb.as_ref() else {
            return;
        };
        if runtime_id.is_empty() {
            return;
        }
        let mut con = rdb.clone();
        let mut set = redis::cmd("SET");
        set.arg(empty_claim_key(runtime_id))
            .arg(observed_version.to_string())
            .arg("EX")
            .arg(EMPTY_CLAIM_CACHE_TTL.as_secs());
        let fut = set.query_async::<()>(&mut con);
        match tokio::time::timeout(EMPTY_CLAIM_REDIS_TIMEOUT, fut).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => tracing::warn!(error = %err, "empty_claim_cache: set failed"),
            Err(_) => tracing::warn!("empty_claim_cache: set timed out"),
        }
    }

    /// Increments the runtime's invalidation version. Called from every
    /// enqueue path BEFORE the daemon WS wakeup so any verdict written under
    /// the previous version is rejected on the next read — no separate DEL on
    /// the empty key needed. Errors log and swallow: a Redis hiccup must not
    /// stop a legitimate enqueue, and the empty key still expires on its own
    /// TTL so the worst-case stall is bounded.
    pub async fn bump(&self, runtime_id: &str) {
        let Some(rdb) = self.rdb.as_ref() else {
            return;
        };
        if runtime_id.is_empty() {
            return;
        }
        let key = empty_claim_version(runtime_id);
        let mut con = rdb.clone();
        // atomic() borrows the pipe mutably; hold the pipe by value and
        // drive it through reborrows so nothing temporary outlives a
        // statement.
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.cmd("INCR").arg(&key);
        pipe.cmd("EXPIRE")
            .arg(&key)
            .arg(EMPTY_CLAIM_VERSION_TTL.as_secs());
        let fut = pipe.query_async::<()>(&mut con);
        match tokio::time::timeout(EMPTY_CLAIM_REDIS_TIMEOUT, fut).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "empty_claim_cache: bump failed; entry will expire on TTL");
            }
            Err(_) => {
                tracing::warn!("empty_claim_cache: bump timed out; entry will expire on TTL");
            }
        }
    }
}
