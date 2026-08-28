//! Redis implementation of the token-fenced channel lease store.
//!
//! Every mutation is a single Lua operation, so compare + expiry
//! update/delete cannot be interleaved by another replica.
//!
//! Port note: Go's `redis.Script.Run` sends EVALSHA with an automatic
//! EVAL fallback after NOSCRIPT. Rust issues EVAL directly (full source
//! every call): identical atomicity and watch semantics, one round trip,
//! and no script-cache coupling. The Go `Ready` preload exists to warm
//! that cache; here `ready` proves equivalent capability by executing
//! every script against a throwaway key.

use std::collections::HashSet;

use async_trait::async_trait;
use regex::Regex;
use uuid::Uuid;

use crate::lease::{AcquireLeaseParams, LeaseError, LeaseStore, ReleaseLeaseParams};

pub(crate) const REDIS_LEASE_KEY_PREFIX: &str = "patchbay:channel-lease:v1:";

fn lease_namespace_pattern() -> &'static Regex {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"^[A-Za-z0-9._-]+$").unwrap())
}

// Script sources are byte-identical to the Go constants: the CAS lives in
// one atomic Lua op per mutation.
const REDIS_TRY_ACQUIRE_LEASE_SOURCE: &str = r#"
local current = redis.call('GET', KEYS[1])
if (not current) or current == ARGV[1] then
  redis.call('SET', KEYS[1], ARGV[1], 'PX', ARGV[2])
  return 1
end
return 0
"#;

const REDIS_RENEW_LEASE_SOURCE: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('PEXPIRE', KEYS[1], ARGV[2])
end
return 0
"#;

const REDIS_RELEASE_LEASE_SOURCE: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('DEL', KEYS[1])
end
return 0
"#;

/// Implements token-fenced channel leases over a cloneable Redis
/// connection handle.
///
/// Port note: Go holds `*redis.Client`; Rust holds a ConnectionManager
/// (single multiplexed connection), which preserves the standalone /
/// sentinel restriction documented on [`LeaseStore::list_held`].
#[derive(Clone)]
pub struct RedisLeaseStore {
    conn: redis::aio::ConnectionManager,
    namespace: String,
}

impl RedisLeaseStore {
    /// Validates the namespace and clones a connection handle.
    pub fn new(conn: redis::aio::ConnectionManager, namespace: &str) -> Result<Self, LeaseError> {
        if !lease_namespace_pattern().is_match(namespace) {
            return Err(LeaseError::Backend(anyhow::anyhow!(
                "CHANNEL_WS_LEASE_NAMESPACE must match [A-Za-z0-9._-]+"
            )));
        }
        Ok(Self {
            conn,
            namespace: namespace.to_string(),
        })
    }

    /// Verifies connectivity, then runs a full acquire/renew/release
    /// cycle against a throwaway per-node key. Redis-backed startup is
    /// fail-closed: callers must not start the Supervisor if this fails.
    pub async fn ready(&self) -> Result<(), LeaseError> {
        let mut conn = self.conn.clone();
        redis::cmd("PING")
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| LeaseError::Backend(anyhow::anyhow!("ping: {e}")))?;
        let ready_key = format!(
            "{REDIS_LEASE_KEY_PREFIX}{}:__ready__:{}",
            self.namespace,
            crate::ids::new_node_id()
        );
        const READY_TOKEN: &str = "readiness-check";
        // Each step asserts result == 1 exactly like the Go loop.
        for (name, source, result) in [
            ("try_acquire", REDIS_TRY_ACQUIRE_LEASE_SOURCE, 5000i64),
            ("renew", REDIS_RENEW_LEASE_SOURCE, 5000),
            ("release", REDIS_RELEASE_LEASE_SOURCE, 0),
        ] {
            let outcome: i64 = eval_script(
                &mut conn,
                source,
                &ready_key,
                &[READY_TOKEN.to_string(), result.to_string()],
            )
            .await
            .map_err(|e| LeaseError::Backend(anyhow::anyhow!("execute {name} script: {e}")))?;
            if outcome != 1 {
                return Err(LeaseError::Backend(anyhow::anyhow!(
                    "execute {name} script: unexpected result {outcome}"
                )));
            }
        }
        Ok(())
    }

    fn key(&self, id: Uuid) -> String {
        format!("{REDIS_LEASE_KEY_PREFIX}{}:{}", self.namespace, id)
    }

    /// Shared acquire/renew body: TTL validation, script dispatch, and
    /// the 0-result → NotAcquired mapping.
    async fn run_lease_script(
        &self,
        source: &'static str,
        arg: AcquireLeaseParams,
    ) -> Result<(), LeaseError> {
        let ttl_millis = arg.ttl.num_milliseconds();
        if ttl_millis <= 0 {
            return Err(LeaseError::Backend(anyhow::anyhow!(
                "channel lease TTL must be at least 1ms"
            )));
        }
        let token = arg.token.clone();
        let ttl_str = ttl_millis.to_string();
        let result: i64 = eval_script(
            &mut self.conn.clone(),
            source,
            &self.key(arg.id),
            &[token, ttl_str],
        )
        .await?;
        if result == 0 {
            return Err(LeaseError::NotAcquired);
        }
        Ok(())
    }
}

#[async_trait]
impl LeaseStore for RedisLeaseStore {
    async fn list_held(&self, ids: &[Uuid]) -> Result<HashSet<String>, LeaseError> {
        let mut held = HashSet::with_capacity(ids.len());
        if ids.is_empty() {
            return Ok(held);
        }
        let keys: Vec<String> = ids.iter().map(|id| self.key(*id)).collect();
        // RedisLeaseStore intentionally targets standalone/sentinel, not
        // cluster. A future cluster-mode implementation must group keys by
        // hash slot or pipeline individual GETs; cross-slot MGET is not
        // valid.
        let values: Vec<Option<String>> = {
            let mut cmd = redis::cmd("MGET");
            for k in &keys {
                cmd.arg(k);
            }
            cmd.query_async(&mut self.conn.clone())
                .await
                .map_err(|e| LeaseError::Backend(anyhow::Error::from(e)))?
        };
        for (id, value) in ids.iter().zip(values.iter()) {
            if value.is_some() {
                held.insert(id.to_string());
            }
        }
        Ok(held)
    }

    async fn try_acquire(&self, arg: AcquireLeaseParams) -> Result<(), LeaseError> {
        self.run_lease_script(REDIS_TRY_ACQUIRE_LEASE_SOURCE, arg)
            .await
    }

    async fn renew(&self, arg: AcquireLeaseParams) -> Result<(), LeaseError> {
        self.run_lease_script(REDIS_RENEW_LEASE_SOURCE, arg).await
    }

    async fn release(&self, arg: ReleaseLeaseParams) -> Result<(), LeaseError> {
        let token = arg.token.clone();
        let _: i64 = eval_script(
            &mut self.conn.clone(),
            REDIS_RELEASE_LEASE_SOURCE,
            &self.key(arg.id),
            &[token],
        )
        .await?;
        // A stale token returns 0 and is an intentional fenced no-op.
        Ok(())
    }
}

/// Runs one Lua script with a single key and string args, returning the
/// integer reply (nil reads as 0).
async fn eval_script(
    conn: &mut redis::aio::ConnectionManager,
    source: &str,
    key: &str,
    args: &[String],
) -> Result<i64, anyhow::Error> {
    let mut cmd = redis::cmd("EVAL");
    cmd.arg(source).arg(1).arg(key);
    for a in args {
        cmd.arg(a);
    }
    let v: Option<i64> = cmd.query_async(conn).await.map_err(anyhow::Error::from)?;
    Ok(v.unwrap_or(0))
}
