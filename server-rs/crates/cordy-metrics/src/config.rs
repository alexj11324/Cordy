//! Config surface.

/// METRICS_ADDR-driven configuration for the standalone /metrics server.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub addr: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            addr: std::env::var("METRICS_ADDR")
                .unwrap_or_default()
                .trim()
                .to_string(),
        }
    }

    pub fn enabled(&self) -> bool {
        !self.addr.trim().is_empty()
    }
}

/// Reports whether the address binds a loopback host. Mirrors Go's
/// SplitHostPort-then-ParseIP fallback chain, including the IPv6 bracket
/// strip and the "localhost" name check.
pub fn is_loopback_addr(addr: &str) -> bool {
    let trimmed = addr.trim();
    let host = match split_host_port(trimmed) {
        Some(h) => h,
        None => trimmed.to_string(),
    };
    let host = host.trim_matches(|c| c == '[' || c == ']');
    if host.is_empty() {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Minimal net.SplitHostPort: splits on the last colon. A bare IPv6 literal
/// without brackets has multiple colons and no port — Go errors there too,
/// and we fall back to treating the whole string as the host.
fn split_host_port(addr: &str) -> Option<String> {
    let colon = addr.rfind(':')?;
    // Bracketed IPv6 like [::1]:8080 or bare "::1" — only treat as
    // host:port when exactly one colon exists after the bracket/host part.
    let (host, port) = addr.split_at(colon);
    if port[1..].contains(':') || port.len() == 1 {
        return None;
    }
    if !port[1..].chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(host.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_addresses_match_go_semantics() {
        assert!(is_loopback_addr("127.0.0.1:9091"));
        assert!(is_loopback_addr("localhost:9091"));
        assert!(is_loopback_addr("LOCALHOST:9091"));
        assert!(is_loopback_addr("[::1]:9091"));
        assert!(is_loopback_addr("127.0.0.1"));
        assert!(!is_loopback_addr("0.0.0.0:9091"));
        assert!(!is_loopback_addr(":9091"));
        assert!(!is_loopback_addr(""));
        assert!(!is_loopback_addr("example.com:9091"));
    }

    #[test]
    fn config_enabled_requires_nonblank_addr() {
        let mut cfg = Config { addr: "  ".into() };
        assert!(!cfg.enabled());
        cfg.addr = "127.0.0.1:9091".into();
        assert!(cfg.enabled());
    }
}
