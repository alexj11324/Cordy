//! Redis-backed daemon-token (mdt_) lookup cache.
use cordy_redis::RecoveringConnection;
use serde::{Deserialize, Serialize};

/// Namespaces daemon-token cache keys separately from PAT (`patchbay:auth:pat:*`)
/// so the two key spaces can't collide and an invalidation on one kind of
/// token doesn't accidentally hit the other.
const DAEMON_TOKEN_CACHE_PREFIX: &str = "patchbay:auth:daemon:";

/// What DaemonAuth needs from the cached lookup — the workspace_id and
/// daemon_id injected into the request context. Deliberately omits token_hash,
/// expires_at, and the row id; cache entries should leak the minimum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonTokenIdentity {
    #[serde(rename = "w")]
    pub workspace_id: String,
    #[serde(rename = "d")]
    pub daemon_id: String,
}

/// Caches resolved daemon-token lookups in Redis. A `disabled()` cache is
/// safe to use — every method becomes a no-op or reports a miss, so
/// single-node dev / tests with no REDIS_URL degrade cleanly to direct DB
/// lookups (mirrors Go's nil `*DaemonTokenCache`).
#[derive(Clone)]
pub struct DaemonTokenCache {
    conn: Option<RecoveringConnection>,
}

impl DaemonTokenCache {
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

    fn key(hash: &str) -> String {
        format!("{DAEMON_TOKEN_CACHE_PREFIX}{hash}")
    }

    /// Returns the cached identity for a token hash. None on miss or any
    /// Redis / decode error — a dead Redis must not take down auth.
    pub async fn get(&self, hash: &str) -> Option<DaemonTokenIdentity> {
        let mut conn = self.conn.clone()?;
        let raw: Option<String> = match crate::bounded_redis(
            redis::cmd("GET")
                .arg(Self::key(hash))
                .query_async(&mut conn),
        )
        .await
        {
            Ok(raw) => raw,
            Err(error) => {
                tracing::warn!(?error, "daemon_token_cache: get failed; falling back to DB");
                return None;
            }
        };
        let raw = raw?;
        match serde_json::from_str(&raw) {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "daemon_token_cache: malformed entry; falling back to DB"
                );
                None
            }
        }
    }

    /// Populates the cache with the given TTL. Use [`super::pat_cache::ttl_for_expiry`]
    /// to clamp the TTL to the token's remaining lifetime so a daemon token
    /// expiring in <AuthCacheTTL can't outlive its expires_at on a cache hit.
    ///
    /// Errors are logged and swallowed — a cache write failure is not a
    /// request failure.
    pub async fn set(&self, hash: &str, id: &DaemonTokenIdentity, ttl_secs: i64) {
        let Some(mut conn) = self.conn.clone() else {
            return;
        };
        if ttl_secs <= 0 {
            return;
        }
        let Ok(raw) = serde_json::to_string(id) else {
            tracing::warn!("daemon_token_cache: marshal failed");
            return;
        };
        let result = crate::bounded_redis(
            redis::cmd("SET")
                .arg(Self::key(hash))
                .arg(raw)
                .arg("EX")
                .arg(ttl_secs)
                .query_async::<()>(&mut conn),
        )
        .await;
        if let Err(error) = result {
            tracing::warn!(?error, "daemon_token_cache: set failed");
        }
    }

    /// Removes the entry for hash. Called when a daemon token is deleted so
    /// the deletion takes effect immediately rather than waiting for the TTL.
    pub async fn invalidate(&self, hash: &str) {
        let Some(mut conn) = self.conn.clone() else {
            return;
        };
        let result = crate::bounded_redis(
            redis::cmd("DEL")
                .arg(Self::key(hash))
                .query_async::<i64>(&mut conn),
        )
        .await;
        if let Err(error) = result {
            tracing::warn!(
                ?error,
                "daemon_token_cache: invalidate failed; entry will expire on TTL"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_format_matches_go() {
        assert_eq!(
            DaemonTokenCache::key("abc123"),
            "patchbay:auth:daemon:abc123"
        );
    }

    #[test]
    fn identity_json_field_names_match_go() {
        // Go marshals {"w": workspaceID, "d": daemonID}.
        let id = DaemonTokenIdentity {
            workspace_id: "ws-1".into(),
            daemon_id: "dm-2".into(),
        };
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, r#"{"w":"ws-1","d":"dm-2"}"#);

        let back: DaemonTokenIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[tokio::test]
    async fn disabled_cache_is_always_a_miss() {
        let cache = DaemonTokenCache::disabled();
        assert!(cache.get("hash").await.is_none());
        // set/invalidate are no-ops — must not panic.
        cache
            .set(
                "hash",
                &DaemonTokenIdentity {
                    workspace_id: "w".into(),
                    daemon_id: "d".into(),
                },
                60,
            )
            .await;
        cache.invalidate("hash").await;
    }
}
