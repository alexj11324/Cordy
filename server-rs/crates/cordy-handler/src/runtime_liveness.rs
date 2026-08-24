//! Redis-backed runtime heartbeat liveness, matching Go's
//! `runtime_liveness_store.go`.

use async_trait::async_trait;
use cordy_redis::RecoveringConnection;
use std::{collections::HashMap, time::Duration};

pub const RUNTIME_LIVENESS_TTL: Duration = Duration::from_secs(90);
pub const RUNTIME_HEARTBEAT_DB_FLUSH_INTERVAL: Duration = Duration::from_secs(60);
const REDIS_OPERATION_TIMEOUT: Duration = Duration::from_millis(250);
const RUNTIME_LIVENESS_KEY_PREFIX: &str = "mul:runtime:hb:";

fn liveness_key(runtime_id: &str) -> String {
    format!("{RUNTIME_LIVENESS_KEY_PREFIX}{runtime_id}")
}

pub fn heartbeat_needs_db_write(
    store_configured: bool,
    status: &str,
    last_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    !store_configured
        || status != "online"
        || last_seen_at.is_none_or(|last_seen| {
            now.signed_duration_since(last_seen)
                >= chrono::Duration::from_std(RUNTIME_HEARTBEAT_DB_FLUSH_INTERVAL)
                    .expect("runtime heartbeat flush interval fits chrono")
        })
}

#[async_trait]
pub trait RuntimeLivenessStore: Send + Sync {
    async fn touch(&self, runtime_id: &str, ttl: Duration) -> anyhow::Result<()>;
    async fn is_alive_batch(&self, runtime_ids: &[String])
        -> anyhow::Result<HashMap<String, bool>>;
    async fn forget(&self, runtime_id: &str) -> anyhow::Result<()>;
}

pub struct RedisRuntimeLivenessStore {
    connection: RecoveringConnection,
}

impl RedisRuntimeLivenessStore {
    pub fn new(connection: RecoveringConnection) -> Self {
        Self { connection }
    }
}

#[async_trait]
impl RuntimeLivenessStore for RedisRuntimeLivenessStore {
    async fn touch(&self, runtime_id: &str, ttl: Duration) -> anyhow::Result<()> {
        anyhow::ensure!(!runtime_id.is_empty(), "liveness touch: empty runtime id");
        anyhow::ensure!(!ttl.is_zero(), "liveness touch: zero TTL");
        let mut connection = self.connection.clone();
        let mut command = redis::cmd("SET");
        command
            .arg(liveness_key(runtime_id))
            .arg("1")
            .arg("PX")
            .arg(ttl.as_millis().min(u64::MAX as u128) as u64);
        tokio::time::timeout(
            REDIS_OPERATION_TIMEOUT,
            command.query_async::<()>(&mut connection),
        )
        .await
        .map_err(|_| anyhow::anyhow!("liveness touch timed out"))?
        .map_err(|error| anyhow::anyhow!("liveness touch: {error}"))
    }

    async fn is_alive_batch(
        &self,
        runtime_ids: &[String],
    ) -> anyhow::Result<HashMap<String, bool>> {
        if runtime_ids.is_empty() {
            return Ok(HashMap::new());
        }
        anyhow::ensure!(
            runtime_ids.iter().all(|runtime_id| !runtime_id.is_empty()),
            "liveness batch: empty runtime id"
        );
        let keys = runtime_ids
            .iter()
            .map(|runtime_id| liveness_key(runtime_id))
            .collect::<Vec<_>>();
        let mut connection = self.connection.clone();
        let mut command = redis::cmd("MGET");
        command.arg(keys);
        let values = tokio::time::timeout(
            REDIS_OPERATION_TIMEOUT,
            command.query_async::<Vec<Option<Vec<u8>>>>(&mut connection),
        )
        .await
        .map_err(|_| anyhow::anyhow!("liveness batch timed out"))?
        .map_err(|error| anyhow::anyhow!("liveness batch: {error}"))?;
        anyhow::ensure!(
            values.len() == runtime_ids.len(),
            "liveness batch returned an unexpected result count"
        );
        Ok(runtime_ids
            .iter()
            .cloned()
            .zip(values.into_iter().map(|value| value.is_some()))
            .collect())
    }

    async fn forget(&self, runtime_id: &str) -> anyhow::Result<()> {
        if runtime_id.is_empty() {
            return Ok(());
        }
        let mut connection = self.connection.clone();
        let mut command = redis::cmd("DEL");
        command.arg(liveness_key(runtime_id));
        tokio::time::timeout(
            REDIS_OPERATION_TIMEOUT,
            command.query_async::<i64>(&mut connection),
        )
        .await
        .map_err(|_| anyhow::anyhow!("liveness forget timed out"))?
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!("liveness forget: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_namespace_and_timing_invariants_match_go() {
        assert_eq!(liveness_key("runtime-1"), "mul:runtime:hb:runtime-1");
        assert_eq!(RUNTIME_LIVENESS_TTL, Duration::from_secs(90));
        assert_eq!(RUNTIME_HEARTBEAT_DB_FLUSH_INTERVAL, Duration::from_secs(60));
        assert!(RUNTIME_HEARTBEAT_DB_FLUSH_INTERVAL < RUNTIME_LIVENESS_TTL);
        assert!(REDIS_OPERATION_TIMEOUT < RUNTIME_HEARTBEAT_DB_FLUSH_INTERVAL);
    }

    #[test]
    fn db_fallback_and_flush_decision_preserve_authoritative_state() {
        let now = chrono::Utc::now();
        assert!(heartbeat_needs_db_write(false, "online", Some(now), now));
        assert!(heartbeat_needs_db_write(true, "offline", Some(now), now));
        assert!(heartbeat_needs_db_write(true, "online", None, now));
        assert!(!heartbeat_needs_db_write(
            true,
            "online",
            Some(now - chrono::Duration::seconds(59)),
            now
        ));
        assert!(heartbeat_needs_db_write(
            true,
            "online",
            Some(now - chrono::Duration::seconds(60)),
            now
        ));
    }
}
