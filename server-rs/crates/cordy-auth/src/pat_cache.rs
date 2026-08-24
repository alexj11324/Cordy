//! Redis-backed PAT lookup cache — port of `server/internal/auth/pat_cache.go`.

use chrono::{DateTime, Utc};
use redis::aio::ConnectionManager;

/// Bounds how long a token-hash lookup stays cached before auth goes back to
/// Postgres. Short enough that revocation lag from a missed invalidation is
/// bounded; long enough that a high-frequency client collapses from one DB
/// round-trip per request to one per TTL window.
pub const AUTH_CACHE_TTL_SECS: i64 = 10 * 60;

/// Namespaces auth-cache keys away from the realtime relay (`ws:*`) and
/// local-skill (`mul:local_skill:*`) keys.
const PAT_CACHE_PREFIX: &str = "mul:auth:pat:";

/// Caches resolved PAT lookups in Redis. A `disabled()` cache is safe to use —
/// every method becomes a no-op or reports a miss, and auth degrades to direct
/// DB lookups (mirrors Go's nil `*PATCache`).
#[derive(Clone)]
pub struct PatCache {
    conn: Option<ConnectionManager>,
}

impl PatCache {
    /// Builds an active cache backed by `client`'s connection manager
    /// (auto-reconnecting, shared like go-redis's internal pool).
    pub async fn new(client: redis::Client) -> redis::RedisResult<Self> {
        let conn = client.get_connection_manager().await?;
        Ok(Self::from_connection_manager(conn))
    }

    pub fn from_connection_manager(conn: ConnectionManager) -> Self {
        Self { conn: Some(conn) }
    }

    /// A cache that never hits — used when REDIS_URL is unset.
    pub fn disabled() -> Self {
        Self { conn: None }
    }

    fn key(hash: &str) -> String {
        format!("{PAT_CACHE_PREFIX}{hash}")
    }

    /// Returns the cached user_id for a token hash. None on miss or ANY Redis
    /// error — a dead Redis must not take down auth.
    pub async fn get(&self, hash: &str) -> Option<String> {
        let mut conn = self.conn.clone()?;
        match crate::bounded_redis(
            redis::cmd("GET")
                .arg(Self::key(hash))
                .query_async::<Option<String>>(&mut conn),
        )
        .await
        {
            Ok(v) => v,
            Err(error) => {
                tracing::warn!(?error, "pat_cache: get failed; falling back to DB");
                None
            }
        }
    }

    /// Populates the cache. Callers MUST pass a TTL no longer than the token's
    /// remaining lifetime — use [`ttl_for_expiry`]. Errors are logged and
    /// swallowed: a cache write failure is not a request failure.
    pub async fn set(&self, hash: &str, user_id: &str, ttl_secs: i64) {
        let Some(conn) = self.conn.clone() else {
            return;
        };
        if ttl_secs <= 0 {
            return;
        }
        let mut conn = conn;
        let result = crate::bounded_redis(
            redis::cmd("SET")
                .arg(Self::key(hash))
                .arg(user_id)
                .arg("EX")
                .arg(ttl_secs)
                .query_async::<()>(&mut conn),
        )
        .await;
        if let Err(error) = result {
            tracing::warn!(?error, "pat_cache: set failed");
        }
    }

    /// Removes the entry for `hash`. Called on PAT revocation so the revoke
    /// takes effect immediately rather than waiting out the TTL.
    pub async fn invalidate(&self, hash: &str) {
        let Some(conn) = self.conn.clone() else {
            return;
        };
        let mut conn = conn;
        let result = crate::bounded_redis(
            redis::cmd("DEL")
                .arg(Self::key(hash))
                .query_async::<i64>(&mut conn),
        )
        .await;
        if let Err(error) = result {
            tracing::warn!(
                ?error,
                "pat_cache: invalidate failed; entry will expire on TTL"
            );
        }
    }
}

/// Cache TTL for a token given its expires_at:
/// - None (token never expires) → full [`AUTH_CACHE_TTL_SECS`].
/// - In the future → min(AUTH_CACHE_TTL_SECS, time until expiry).
/// - At or before now → 0 (caller skips caching; TOCTOU between SELECT and
///   Set is possible).
pub fn ttl_for_expiry(now: DateTime<Utc>, expires_at: Option<DateTime<Utc>>) -> i64 {
    let Some(expires_at) = expires_at else {
        return AUTH_CACHE_TTL_SECS;
    };
    let remaining = (expires_at - now).num_seconds();
    if remaining <= 0 {
        return 0;
    }
    remaining.min(AUTH_CACHE_TTL_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn ttl_for_expiry_branches() {
        let now = Utc::now();
        // No expiry → full TTL.
        assert_eq!(ttl_for_expiry(now, None), AUTH_CACHE_TTL_SECS);
        // Far-future expiry → clamped to TTL.
        assert_eq!(
            ttl_for_expiry(now, Some(now + Duration::hours(24))),
            AUTH_CACHE_TTL_SECS
        );
        // Near expiry → remaining time.
        assert_eq!(ttl_for_expiry(now, Some(now + Duration::seconds(60))), 60);
        // Already expired → 0.
        assert_eq!(ttl_for_expiry(now, Some(now - Duration::seconds(1))), 0);
        // Exactly now → 0.
        assert_eq!(ttl_for_expiry(now, Some(now)), 0);
    }
}
