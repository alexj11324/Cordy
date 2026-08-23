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
}

/// Per-IP fixed-window rate limiter backed by Redis.
pub async fn rate_limit(State(state): State<RateLimitState>, req: Request, next: Next) -> Response {
    let Some(client) = state.client.clone() else {
        return next.run(req).await;
    };

    let cached = state.conn.lock().await.clone();
    let mut conn = match cached {
        Some(conn) => conn,
        None => {
            match tokio::time::timeout(REDIS_OPERATION_TIMEOUT, client.get_connection_manager())
                .await
            {
                Ok(Ok(conn)) => {
                    *state.conn.lock().await = Some(conn.clone());
                    conn
                }
                Ok(Err(error)) => {
                    tracing::warn!(%error, "ratelimit: redis unavailable; allowing request");
                    return next.run(req).await;
                }
                Err(_) => {
                    tracing::warn!("ratelimit: redis connection timed out; allowing request");
                    return next.run(req).await;
                }
            }
        }
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
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            *state.conn.lock().await = None;
            tracing::warn!(error = %e, ip = %ip, "ratelimit: redis error; allowing request");
            return next.run(req).await;
        }
        Err(_) => {
            *state.conn.lock().await = None;
            tracing::warn!(ip = %ip, "ratelimit: redis command timed out; allowing request");
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

    if !trusted_proxies.is_empty() {
        if let Ok(remote_ip) = remote_host.parse::<IpAddr>() {
            if is_trusted_proxy(&remote_ip, trusted_proxies) {
                if let Some(xff) = req
                    .headers()
                    .get("x-forwarded-for")
                    .and_then(|v| v.to_str().ok())
                {
                    if !xff.is_empty() {
                        for part in xff.rsplit(',') {
                            let candidate = part.trim();
                            if let Ok(ip) = candidate.parse::<IpAddr>() {
                                if !is_trusted_proxy(&ip, trusted_proxies) {
                                    return ip.to_string();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Default: RemoteAddr in canonical form.
    if let Ok(ip) = remote_host.parse::<IpAddr>() {
        return ip.to_string();
    }
    remote_host
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
}
