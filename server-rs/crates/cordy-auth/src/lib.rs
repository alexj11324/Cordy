//! Auth primitives ported from `server/internal/auth`.
//!
//! Modules mirror the Go files one-to-one so review diffs stay aligned:
//! `jwt` (secrets + token minting), `cookie` (session/CSRF), `disabled_users`
//! (emergency denylist), plus the Redis-backed `pat_cache`,
//! `daemon_token_cache`, and `membership_cache` modules.

use std::future::Future;
use std::time::Duration;

pub(crate) const REDIS_CACHE_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedisCacheFailure {
    Redis,
    Timeout,
}

pub(crate) async fn bounded_redis<T>(
    operation: impl Future<Output = redis::RedisResult<T>>,
) -> Result<T, RedisCacheFailure> {
    match tokio::time::timeout(REDIS_CACHE_TIMEOUT, operation).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) => Err(RedisCacheFailure::Redis),
        Err(_) => Err(RedisCacheFailure::Timeout),
    }
}

pub mod cloud_pat;
pub mod cookie;
pub mod daemon_token_cache;
pub mod disabled_users;
pub mod jwt;
pub mod membership_cache;
pub mod pat_cache;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn redis_cache_operations_are_bounded() {
        let pending = std::future::pending::<redis::RedisResult<()>>();
        let result = bounded_redis(pending).await;
        assert_eq!(result, Err(RedisCacheFailure::Timeout));
        assert_eq!(REDIS_CACHE_TIMEOUT, Duration::from_millis(250));
    }
}
