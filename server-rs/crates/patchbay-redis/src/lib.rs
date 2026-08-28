//! Shared Redis connection that preserves go-redis's lazy startup behavior.
//!
//! `redis::aio::ConnectionManager` reconnects after an established connection
//! is lost, but its constructor requires Redis to be reachable. Production
//! stores use this wrapper so a configured Redis outage during process startup
//! does not permanently disable them. Their existing per-operation timeouts
//! remain the authority for bounded failure and fail-open/fail-safe behavior.

use std::sync::Arc;

use redis::{
    aio::{ConnectionLike, ConnectionManager},
    Cmd, Pipeline, RedisFuture, RedisResult, Value,
};
use tokio::sync::Mutex;

/// A cloneable Redis connection that establishes its shared manager on the
/// first command and retries initialization after a failed or cancelled
/// attempt.
#[derive(Clone)]
pub struct RecoveringConnection {
    client: redis::Client,
    manager: Arc<Mutex<Option<ConnectionManager>>>,
}

impl RecoveringConnection {
    /// Creates a connection without performing network I/O.
    pub fn new(client: redis::Client) -> Self {
        Self {
            client,
            manager: Arc::new(Mutex::new(None)),
        }
    }

    async fn manager(&self) -> RedisResult<ConnectionManager> {
        let mut manager = self.manager.lock().await;
        if let Some(manager) = manager.as_ref() {
            return Ok(manager.clone());
        }

        // Hold the initialization lock across the attempt so concurrent cache
        // misses do not stampede Redis. Cancellation drops the guard and leaves
        // the slot empty, allowing the next bounded operation to retry.
        let connected = self.client.get_connection_manager().await?;
        *manager = Some(connected.clone());
        Ok(connected)
    }
}

impl ConnectionLike for RecoveringConnection {
    fn req_packed_command<'a>(&'a mut self, cmd: &'a Cmd) -> RedisFuture<'a, Value> {
        Box::pin(async move {
            let mut manager = self.manager().await?;
            manager.req_packed_command(cmd).await
        })
    }

    fn req_packed_commands<'a>(
        &'a mut self,
        cmd: &'a Pipeline,
        offset: usize,
        count: usize,
    ) -> RedisFuture<'a, Vec<Value>> {
        Box::pin(async move {
            let mut manager = self.manager().await?;
            manager.req_packed_commands(cmd, offset, count).await
        })
    }

    fn get_db(&self) -> i64 {
        self.client.get_connection_info().redis.db
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construction_is_lazy_and_preserves_database() {
        // TEST-NET-1 is deliberately unreachable. Construction succeeds
        // synchronously because no network operation is attempted.
        let Ok(client) = redis::Client::open("redis://192.0.2.1:6379/7") else {
            panic!("valid test Redis URL was rejected");
        };
        let connection = RecoveringConnection::new(client);
        assert_eq!(connection.get_db(), 7);
    }

    #[test]
    fn clones_share_the_initialization_slot() {
        let Ok(client) = redis::Client::open("redis://127.0.0.1:6379/") else {
            panic!("valid test Redis URL was rejected");
        };
        let connection = RecoveringConnection::new(client);
        let clone = connection.clone();
        assert!(Arc::ptr_eq(&connection.manager, &clone.manager));
    }

    #[test]
    fn secure_redis_urls_are_supported_without_connecting() {
        let Ok(client) = redis::Client::open("rediss://cache.example.test:6380/2") else {
            panic!("secure Redis URLs must be enabled for production caches");
        };
        let connection = RecoveringConnection::new(client);
        assert_eq!(connection.get_db(), 2);
    }
}
