use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

const KEY_PREFIX: &str = "mul:runtime:hb:";

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
    client: redis::Client,
}

impl RedisLivenessStore {
    pub fn new(client: redis::Client) -> Arc<Self> {
        Arc::new(Self { client })
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
        let mut connection = self.client.get_multiplexed_async_connection().await?;
        redis::cmd("SET")
            .arg(Self::key(runtime_id))
            .arg("1")
            .arg("PX")
            .arg(u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX))
            .query_async::<()>(&mut connection)
            .await?;
        Ok(())
    }

    async fn is_alive_batch(&self, runtime_ids: &[String]) -> (HashMap<String, bool>, bool) {
        if runtime_ids.is_empty() {
            return (HashMap::new(), true);
        }
        let mut connection = match self.client.get_multiplexed_async_connection().await {
            Ok(connection) => connection,
            Err(error) => {
                tracing::warn!(%error, "liveness connection failed; falling back to DB");
                return (HashMap::new(), false);
            }
        };
        let keys = runtime_ids
            .iter()
            .map(|id| Self::key(id))
            .collect::<Vec<_>>();
        let values = match redis::cmd("MGET")
            .arg(keys)
            .query_async::<Vec<Option<String>>>(&mut connection)
            .await
        {
            Ok(values) => values,
            Err(error) => {
                tracing::warn!(%error, count = runtime_ids.len(), "liveness mget failed; falling back to DB");
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
        let result = async {
            let mut connection = self.client.get_multiplexed_async_connection().await?;
            redis::cmd("DEL")
                .arg(Self::key(runtime_id))
                .query_async::<()>(&mut connection)
                .await?;
            Ok::<_, redis::RedisError>(())
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, %runtime_id, "liveness forget failed");
        }
    }
}
