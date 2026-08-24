//! Per-IP fixed-window rate limiter — port of `server/internal/middleware/ratelimit.go`.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use ipnetwork::IpNetwork;
use redis::aio::ConnectionManager;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Atomically increments the counter and sets the TTL on first access. The
/// Lua script ensures INCR and EXPIRE cannot be split by a network failure —
/// if INCR succeeds the TTL is guaranteed to be set, preventing a stuck key
/// that acts as a permanent ban.
const RATE_LIMIT_SCRIPT: &str = r#"
local count = redis.call('INCR', KEYS[1])
if count == 1 then
    redis.call('EXPIRE', KEYS[1], ARGV[1])
end
return count
"#;
const REDIS_OPERATION_TIMEOUT: Duration = Duration::from_millis(250);

/// Parses a comma-separated list of CIDRs. Invalid entries are warned and
/// skipped. Empty when raw is blank (default: never trust X-Forwarded-For).
pub fn parse_trusted_proxies(raw: &str) -> Vec<IpNetwork> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    raw.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .filter_map(|p| match p.parse::<IpNetwork>() {
            Ok(net) => Some(net),
            Err(e) => {
                tracing::warn!(
                    cidr = %p,
                    error = %e,
                    "ratelimit: invalid trusted proxy CIDR, skipping"
                );
                None
            }
        })
        .collect()
}

/// Configuration for the rate-limit middleware.
///
/// `client: None` makes the middleware a no-op (fail-open), mirroring Go's nil
/// `*redis.Client`.
#[derive(Clone)]
pub struct RateLimitState {
    pub client: Option<redis::Client>,
    pub conn: Arc<Mutex<Option<ConnectionManager>>>,
    pub limit: i64,
    pub window_secs: i64,
    pub trusted_proxies: Vec<IpNetwork>,
}

impl RateLimitState {
    /// Fail-open limiter used when REDIS_URL is unset.
    pub fn disabled(limit: i64, window_secs: i64) -> Self {
        Self {
            client: None,
            conn: Arc::new(Mutex::new(None)),
            limit,
            window_secs,
            trusted_proxies: Vec::new(),
        }
    }

    pub fn with_client(mut self, client: redis::Client) -> Self {
        self.client = Some(client);
        self
    }

    async fn connection(&self) -> Option<ConnectionManager> {
        if let Some(conn) = self.conn.lock().await.clone() {
            return Some(conn);
        }
        let client = self.client.as_ref()?.clone();
        let conn =
            match tokio::time::timeout(REDIS_OPERATION_TIMEOUT, client.get_connection_manager())
                .await
            {
                Ok(Ok(conn)) => conn,
                Ok(Err(error)) => {
                    tracing::warn!(%error, "ratelimit: redis connect failed; allowing request");
                    return None;
                }
                Err(_) => {
                    tracing::warn!("ratelimit: redis connect timed out; allowing request");
                    return None;
                }
            };
        *self.conn.lock().await = Some(conn.clone());
        Some(conn)
    }

    async fn clear_connection(&self) {
        *self.conn.lock().await = None;
    }
}

/// Per-IP fixed-window rate limiter backed by Redis.
pub async fn rate_limit(State(state): State<RateLimitState>, req: Request, next: Next) -> Response {
    let Some(mut conn) = state.connection().await else {
        return next.run(req).await;
    };

    let ip = extract_ip(&req, &state.trusted_proxies);
    let key = rate_limit_key(req.uri().path(), &ip);

    let script = redis::Script::new(RATE_LIMIT_SCRIPT);
    let count = match tokio::time::timeout(
        REDIS_OPERATION_TIMEOUT,
        script
            .key(key)
            .arg(state.window_secs)
            .invoke_async::<i64>(&mut conn),
    )
    .await
    {
        Ok(Ok(count)) => count,
        Ok(Err(error)) => {
            state.clear_connection().await;
            tracing::warn!(%error, ip = %ip, "ratelimit: redis error; allowing request");
            return next.run(req).await;
        }
        Err(_) => {
            state.clear_connection().await;
            tracing::warn!(ip = %ip, "ratelimit: redis timed out; allowing request");
            return next.run(req).await;
        }
    };

    if count > state.limit {
        // Hand-built to match Go exactly: Retry-After header +
        // Content-Type application/json + fixed error envelope.
        let mut res = Response::new(Body::from(r#"{"error":"too many requests"}"#));
        *res.status_mut() = StatusCode::TOO_MANY_REQUESTS;
        if let Ok(v) = HeaderValue::from_str(&state.window_secs.to_string()) {
            res.headers_mut().insert("retry-after", v);
        }
        res.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        return res;
    }

    next.run(req).await
}

/// Determines the client IP for rate limiting purposes. Only honors
/// X-Forwarded-For when the direct connection originates from a trusted
/// proxy; walks the XFF chain right-to-left so the rightmost non-trusted
/// entry wins.
fn extract_ip(req: &Request, trusted_proxies: &[IpNetwork]) -> String {
    // Requires the server to be started with into_make_service_with_connect_info.
    let remote_host = req
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0.ip().to_string())
        .unwrap_or_default();

    client_ip(
        req.headers(),
        remote_host.parse::<IpAddr>().ok(),
        trusted_proxies,
    )
}

/// Resolves a limiter key from the peer and forwarding headers. Forwarded
/// values are considered only for a trusted direct peer, parsed as IP
/// addresses, and walked right-to-left to prevent a client-controlled prefix
/// from bypassing per-IP limits.
pub fn client_ip(
    headers: &axum::http::HeaderMap,
    remote_ip: Option<IpAddr>,
    trusted_proxies: &[IpNetwork],
) -> String {
    if remote_ip.is_some_and(|ip| is_trusted_proxy(&ip, trusted_proxies)) {
        if let Some(xff) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
        {
            for part in xff.rsplit(',') {
                if let Ok(ip) = part.trim().parse::<IpAddr>() {
                    if !is_trusted_proxy(&ip, trusted_proxies) {
                        return ip.to_string();
                    }
                }
            }
        }
        if let Some(real_ip) = headers
            .get("x-real-ip")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<IpAddr>().ok())
        {
            return real_ip.to_string();
        }
    }
    remote_ip.map(|ip| ip.to_string()).unwrap_or_default()
}

fn is_trusted_proxy(ip: &IpAddr, cidrs: &[IpNetwork]) -> bool {
    cidrs.iter().any(|net| net.contains(*ip))
}

fn rate_limit_key(path: &str, ip: &str) -> String {
    let sanitized = path.strip_prefix('/').unwrap_or(path).replace('/', ":");
    format!("mul:ratelimit:{sanitized}:{ip}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cidr_list_and_skips_invalid() {
        let nets = parse_trusted_proxies("10.0.0.0/8, bogus, 172.16.0.0/12");
        assert_eq!(nets.len(), 2);
        assert!(parse_trusted_proxies("").is_empty());
        assert!(parse_trusted_proxies("   ").is_empty());
    }

    #[test]
    fn key_sanitizes_path_segments() {
        assert_eq!(
            rate_limit_key("/api/issues/123", "1.2.3.4"),
            "mul:ratelimit:api:issues:123:1.2.3.4"
        );
        assert_eq!(rate_limit_key("/", "1.2.3.4"), "mul:ratelimit::1.2.3.4");
    }

    #[test]
    fn trusted_proxy_membership() {
        let nets = parse_trusted_proxies("10.0.0.0/8");
        let ip: IpAddr = "10.1.2.3".parse().unwrap();
        assert!(is_trusted_proxy(&ip, &nets));
        let outside: IpAddr = "11.1.2.3".parse().unwrap();
        assert!(!is_trusted_proxy(&outside, &nets));
    }

    #[tokio::test]
    async fn unavailable_redis_connection_is_bounded_and_fail_open() {
        let state = RateLimitState::disabled(5, 60).with_client(
            redis::Client::open("redis://127.0.0.1:1").expect("valid unavailable Redis URL"),
        );
        let started = std::time::Instant::now();
        assert!(state.connection().await.is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
