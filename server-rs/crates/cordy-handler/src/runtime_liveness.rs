use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

const KEY_PREFIX: &str = "mul:runtime:hb:";
const REDIS_OPERATION_TIMEOUT: Duration = Duration::from_millis(250);

#[async_trait]
pub trait LivenessStore: Send + Sync {
    fn available(&self) -> bool;
    async fn touch(&self, runtime_id: &str, ttl: Duration) -> anyhow::Result<()>;
    async fn is_alive_batch(&self, runtime_ids: &[String]) -> (HashMap<String, bool>, bool);
    async fn forget(&self, runtime_id: &str);
}

pub struct NoopLivenessStore;

#[async_trait]
impl LivenessStore for NoopLivenessStore {
    fn available(&self) -> bool {
        false
    }
    async fn touch(&self, _: &str, _: Duration) -> anyhow::Result<()> {
        Ok(())
    }
    async fn is_alive_batch(&self, _: &[String]) -> (HashMap<String, bool>, bool) {
        (HashMap::new(), false)
    }
    async fn forget(&self, _: &str) {}
}

pub struct RedisLivenessStore {
    connection: cordy_redis::RecoveringConnection,
}

impl RedisLivenessStore {
    pub fn new(connection: cordy_redis::RecoveringConnection) -> Arc<Self> {
        Arc::new(Self { connection })
    }
    fn key(runtime_id: &str) -> String {
        format!("{KEY_PREFIX}{runtime_id}")
    }
}

#[async_trait]
impl LivenessStore for RedisLivenessStore {
    fn available(&self) -> bool {
        true
    }

    async fn touch(&self, runtime_id: &str, ttl: Duration) -> anyhow::Result<()> {
        anyhow::ensure!(
            !runtime_id.is_empty(),
            "redis liveness store: empty runtime id"
        );
        let mut connection = self.connection.clone();
        tokio::time::timeout(
            REDIS_OPERATION_TIMEOUT,
            redis::cmd("SET")
                .arg(Self::key(runtime_id))
                .arg("1")
                .arg("PX")
                .arg(u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX))
                .query_async::<()>(&mut connection),
        )
        .await
        .map_err(|_| anyhow::anyhow!("liveness touch timed out"))??;
        Ok(())
    }

    async fn is_alive_batch(&self, runtime_ids: &[String]) -> (HashMap<String, bool>, bool) {
        if runtime_ids.is_empty() {
            return (HashMap::new(), true);
        }
        let mut connection = self.connection.clone();
        let keys = runtime_ids
            .iter()
            .map(|id| Self::key(id))
            .collect::<Vec<_>>();
        let values = match tokio::time::timeout(
            REDIS_OPERATION_TIMEOUT,
            redis::cmd("MGET")
                .arg(keys)
                .query_async::<Vec<Option<String>>>(&mut connection),
        )
        .await
        {
            Ok(Ok(values)) => values,
            Ok(Err(error)) => {
                tracing::warn!(%error, count = runtime_ids.len(), "liveness mget failed; falling back to DB");
                return (HashMap::new(), false);
            }
            Err(_) => {
                tracing::warn!(
                    count = runtime_ids.len(),
                    "liveness mget timed out; falling back to DB"
                );
                return (HashMap::new(), false);
            }
        };
        let alive = runtime_ids
            .iter()
            .cloned()
            .zip(values.into_iter().map(|value| value.is_some()))
            .collect();
        (alive, true)
    }

    async fn forget(&self, runtime_id: &str) {
        if runtime_id.is_empty() {
            return;
        }
        let mut connection = self.connection.clone();
        match tokio::time::timeout(
            REDIS_OPERATION_TIMEOUT,
            redis::cmd("DEL")
                .arg(Self::key(runtime_id))
                .query_async::<()>(&mut connection),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => tracing::warn!(%error, %runtime_id, "liveness forget failed"),
            Err(_) => tracing::warn!(%runtime_id, "liveness forget timed out"),
        }
    }
}
