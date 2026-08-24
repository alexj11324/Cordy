//! Shared sliding-window safety gates for Autopilot webhook ingress/dispatch.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cordy_redis::RecoveringConnection;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const OPERATION_TIMEOUT: Duration = Duration::from_millis(250);
const WINDOW: Duration = Duration::from_secs(60);
const TOKEN_PREFIX: &str = "mul:webhook:rate:";
const BAD_IP_PREFIX: &str = "mul:webhook:ip:";
const ABSOLUTE_IP_PREFIX: &str = "mul:webhook:absolute-ip:";

// Redis TIME supplies a cross-replica clock. Milliseconds remain exactly
// representable by Lua numbers, unlike current-epoch nanoseconds.
const ALLOW_SCRIPT: &str = r#"
local now_parts = redis.call('TIME')
local now = (tonumber(now_parts[1]) * 1000) + math.floor(tonumber(now_parts[2]) / 1000)
local cutoff = now - tonumber(ARGV[1])
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', cutoff)
if redis.call('ZCARD', KEYS[1]) >= tonumber(ARGV[2]) then
    return 0
end
redis.call('ZADD', KEYS[1], now, ARGV[4])
redis.call('EXPIRE', KEYS[1], ARGV[3])
return 1
"#;

const CHECK_SCRIPT: &str = r#"
local now_parts = redis.call('TIME')
local now = (tonumber(now_parts[1]) * 1000) + math.floor(tonumber(now_parts[2]) / 1000)
local cutoff = now - tonumber(ARGV[1])
redis.call('ZREMRANGEBYSCORE', KEYS[1], '-inf', cutoff)
if redis.call('ZCARD', KEYS[1]) >= tonumber(ARGV[2]) then
    return 0
end
return 1
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    Allowed,
    Limited { retry_after: Duration },
    Unavailable,
}

#[derive(Clone)]
enum Backend {
    Memory(Arc<Mutex<MemoryState>>),
    Redis(Arc<RecoveringConnection>),
}

#[derive(Default)]
struct MemoryState {
    entries: HashMap<String, VecDeque<Instant>>,
    last_sweep: Option<Instant>,
}

#[derive(Clone)]
pub struct SlidingWindowGate {
    backend: Backend,
    prefix: &'static str,
    limit: usize,
    window: Duration,
}

impl SlidingWindowGate {
    fn memory(prefix: &'static str, limit: usize, window: Duration) -> Self {
        Self {
            backend: Backend::Memory(Arc::new(Mutex::new(MemoryState::default()))),
            prefix,
            limit,
            window,
        }
    }

    fn redis(
        connection: RecoveringConnection,
        prefix: &'static str,
        limit: usize,
        window: Duration,
    ) -> Self {
        Self {
            backend: Backend::Redis(Arc::new(connection)),
            prefix,
            limit,
            window,
        }
    }

    pub async fn allow(&self, key: &str, cancel: &CancellationToken) -> GateDecision {
        self.evaluate(key, true, cancel).await
    }

    pub async fn check(&self, key: &str, cancel: &CancellationToken) -> GateDecision {
        self.evaluate(key, false, cancel).await
    }

    async fn evaluate(&self, key: &str, consume: bool, cancel: &CancellationToken) -> GateDecision {
        if self.limit == 0 {
            return GateDecision::Allowed;
        }
        match &self.backend {
            Backend::Memory(state) => {
                let now = Instant::now();
                let mut state = state.lock().await;
                let sweep_interval = self.window.min(Duration::from_secs(60));
                if state
                    .last_sweep
                    .is_none_or(|last| now.duration_since(last) >= sweep_interval)
                {
                    state.entries.retain(|_, hits| {
                        hits.back()
                            .is_some_and(|created| now.duration_since(*created) < self.window)
                    });
                    state.last_sweep = Some(now);
                }
                let hits = state
                    .entries
                    .entry(format!("{}{}", self.prefix, key))
                    .or_default();
                while hits
                    .front()
                    .is_some_and(|created| now.duration_since(*created) >= self.window)
                {
                    hits.pop_front();
                }
                if hits.len() >= self.limit {
                    let retry_after = hits
                        .front()
                        .map(|oldest| self.window.saturating_sub(now.duration_since(*oldest)))
                        .unwrap_or(self.window)
                        .max(Duration::from_secs(1));
                    return GateDecision::Limited { retry_after };
                }
                if consume {
                    hits.push_back(now);
                }
                GateDecision::Allowed
            }
            Backend::Redis(connection) => {
                let mut connection = connection.as_ref().clone();
                let script = redis::Script::new(if consume { ALLOW_SCRIPT } else { CHECK_SCRIPT });
                let mut invocation = script.prepare_invoke();
                invocation
                    .key(format!("{}{}", self.prefix, key))
                    .arg(self.window.as_millis().min(u64::MAX as u128) as u64)
                    .arg(self.limit);
                if consume {
                    invocation
                        .arg(self.window.as_secs().saturating_mul(2).max(1))
                        .arg(Uuid::new_v4().to_string());
                }
                let operation = tokio::time::timeout(
                    OPERATION_TIMEOUT,
                    invocation.invoke_async::<i64>(&mut connection),
                );
                let result = tokio::select! {
                    _ = cancel.cancelled() => return GateDecision::Unavailable,
                    result = operation => result,
                };
                match result {
                    Ok(Ok(1)) => GateDecision::Allowed,
                    Ok(Ok(_)) => GateDecision::Limited {
                        retry_after: self.window.max(Duration::from_secs(1)),
                    },
                    Ok(Err(error)) => {
                        tracing::warn!(%error, "webhook rate limiter Redis error; failing open");
                        GateDecision::Unavailable
                    }
                    Err(_) => {
                        tracing::warn!("webhook rate limiter Redis timeout; failing open");
                        GateDecision::Unavailable
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct WebhookRateLimits {
    pub token: SlidingWindowGate,
    pub bad_credential_ip: SlidingWindowGate,
    pub absolute_ip: SlidingWindowGate,
}

impl Default for WebhookRateLimits {
    fn default() -> Self {
        Self {
            token: SlidingWindowGate::memory(TOKEN_PREFIX, 60, WINDOW),
            bad_credential_ip: SlidingWindowGate::memory(BAD_IP_PREFIX, 30, WINDOW),
            absolute_ip: SlidingWindowGate::memory(ABSOLUTE_IP_PREFIX, 600, WINDOW),
        }
    }
}

impl WebhookRateLimits {
    pub fn redis(connection: RecoveringConnection) -> Self {
        Self {
            token: SlidingWindowGate::redis(connection.clone(), TOKEN_PREFIX, 60, WINDOW),
            bad_credential_ip: SlidingWindowGate::redis(
                connection.clone(),
                BAD_IP_PREFIX,
                30,
                WINDOW,
            ),
            absolute_ip: SlidingWindowGate::redis(connection, ABSOLUTE_IP_PREFIX, 600, WINDOW),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_and_limits_match_go() {
        let limits = WebhookRateLimits::default();
        assert_eq!(
            (limits.token.prefix, limits.token.limit),
            (TOKEN_PREFIX, 60)
        );
        assert_eq!(
            (
                limits.bad_credential_ip.prefix,
                limits.bad_credential_ip.limit
            ),
            (BAD_IP_PREFIX, 30)
        );
        assert_eq!(
            (limits.absolute_ip.prefix, limits.absolute_ip.limit),
            (ABSOLUTE_IP_PREFIX, 600)
        );
        assert_eq!(limits.token.window, Duration::from_secs(60));
    }

    #[test]
    fn scripts_trim_before_count_and_only_allow_consumes() {
        let trim = ALLOW_SCRIPT.find("ZREMRANGEBYSCORE").unwrap();
        let count = ALLOW_SCRIPT.find("ZCARD").unwrap();
        let insert = ALLOW_SCRIPT.find("ZADD").unwrap();
        assert!(trim < count && count < insert);
        assert!(!CHECK_SCRIPT.contains("ZADD"));
        assert!(ALLOW_SCRIPT.contains("redis.call('TIME')"));
    }

    #[tokio::test]
    async fn memory_check_does_not_consume_and_allow_is_bounded() {
        let gate = SlidingWindowGate::memory("test:", 1, WINDOW);
        let cancel = CancellationToken::new();
        assert_eq!(gate.check("key", &cancel).await, GateDecision::Allowed);
        assert_eq!(gate.check("key", &cancel).await, GateDecision::Allowed);
        assert_eq!(gate.allow("key", &cancel).await, GateDecision::Allowed);
        assert!(matches!(
            gate.allow("key", &cancel).await,
            GateDecision::Limited { .. }
        ));
    }
}
