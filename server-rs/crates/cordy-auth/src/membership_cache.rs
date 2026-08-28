//! Redis-backed workspace-membership cache.
use cordy_redis::RecoveringConnection;

const MEMBERSHIP_CACHE_PREFIX: &str = "patchbay:auth:member:";

/// Bounds how long a workspace membership lookup stays cached before the
/// handler goes back to Postgres. Short enough that a removed member loses
/// access within minutes; long enough that a high-frequency caller (daemon
/// heartbeat every ~15s) collapses from one DB round-trip per request to one
/// per TTL window.
pub const MEMBERSHIP_CACHE_TTL_SECS: i64 = 5 * 60;

/// Caches workspace membership existence checks in Redis.
///
/// Tracks ONLY whether a user is a member of a workspace — it does NOT store
/// role information. Authorization decisions that depend on role
/// (require_workspace_role) MUST always query the database directly.
///
/// Revocation latency: a removed member may retain cached access for up to
/// [`MEMBERSHIP_CACHE_TTL_SECS`] (5 min). Combined with `PatCache` (10 min),
/// worst-case revocation delay is 10 min — consistent with the original
/// PATCache design decision.
///
/// A `disabled()` cache is safe to use — every method becomes a no-op or
/// reports a miss, and callers degrade to direct DB lookups (mirrors Go's nil
/// `*MembershipCache`).
#[derive(Clone)]
pub struct MembershipCache {
    conn: Option<RecoveringConnection>,
}

impl MembershipCache {
    /// Builds an active cache backed by `client`'s connection manager.
    pub async fn new(client: redis::Client) -> redis::RedisResult<Self> {
        Ok(Self::from_connection(RecoveringConnection::new(client)))
    }

    pub fn from_connection(conn: RecoveringConnection) -> Self {
        Self { conn: Some(conn) }
    }

    /// A cache that never hits — used when REDIS_URL is unset.
    pub fn disabled() -> Self {
        Self { conn: None }
    }

    fn key(user_id: &str, workspace_id: &str) -> String {
        format!("{MEMBERSHIP_CACHE_PREFIX}{user_id}:{workspace_id}")
    }

    /// Returns whether the user is a cached member of the workspace.
    /// False on miss or any Redis error.
    pub async fn get(&self, user_id: &str, workspace_id: &str) -> bool {
        let Some(mut conn) = self.conn.clone() else {
            return false;
        };
        let result = crate::bounded_redis(
            redis::cmd("GET")
                .arg(Self::key(user_id, workspace_id))
                .query_async::<Option<String>>(&mut conn),
        )
        .await;
        match result {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(error) => {
                tracing::warn!(?error, "membership_cache: get failed; falling back to DB");
                false
            }
        }
    }

    /// Caches the existence of membership for the given user+workspace pair.
    pub async fn set(&self, user_id: &str, workspace_id: &str) {
        let Some(mut conn) = self.conn.clone() else {
            return;
        };
        let result = crate::bounded_redis(
            redis::cmd("SET")
                .arg(Self::key(user_id, workspace_id))
                .arg("1")
                .arg("EX")
                .arg(MEMBERSHIP_CACHE_TTL_SECS)
                .query_async::<()>(&mut conn),
        )
        .await;
        if let Err(error) = result {
            tracing::warn!(?error, "membership_cache: set failed");
        }
    }

    /// Removes the cached entry for a specific user+workspace.
    pub async fn invalidate(&self, user_id: &str, workspace_id: &str) {
        let Some(mut conn) = self.conn.clone() else {
            return;
        };
        let result = crate::bounded_redis(
            redis::cmd("DEL")
                .arg(Self::key(user_id, workspace_id))
                .query_async::<i64>(&mut conn),
        )
        .await;
        if let Err(error) = result {
            tracing::warn!(?error, "membership_cache: invalidate failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_format_matches_go() {
        assert_eq!(
            MembershipCache::key("u-123", "ws-456"),
            "patchbay:auth:member:u-123:ws-456"
        );
    }

    #[tokio::test]
    async fn disabled_cache_is_always_a_miss() {
        let cache = MembershipCache::disabled();
        assert!(!cache.get("u", "ws").await);
        // set/invalidate are no-ops — must not panic.
        cache.set("u", "ws").await;
        cache.invalidate("u", "ws").await;
    }
}
