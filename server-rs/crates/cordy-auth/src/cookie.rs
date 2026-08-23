//! Session cookie and CSRF logic — port of `server/internal/auth/cookie.go`
//! (pure-logic parts; HTTP response wiring lands with the axum middleware).

use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use std::sync::OnceLock;

pub const AUTH_COOKIE_NAME: &str = "cordy_auth";
pub const CSRF_COOKIE_NAME: &str = "cordy_csrf";

/// Go default: 30 days.
pub const DEFAULT_AUTH_TOKEN_TTL_SECS: i64 = 30 * 24 * 3600;

const TEN_YEARS_SECS: i64 = 10 * 365 * 24 * 3600;
static AUTH_TOKEN_TTL_SECS: OnceLock<i64> = OnceLock::new();

type HmacSha256 = Hmac<Sha256>;

/// Parses a raw `AUTH_TOKEN_TTL` value: a Go duration string ("8760h",
/// "720h30m") first, then plain integer seconds. Returns None when empty,
/// invalid, or non-positive.
pub fn parse_auth_token_ttl(raw: &str) -> Option<std::time::Duration> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    if let Some(d) = parse_go_duration(raw) {
        return warn_if_huge(d);
    }

    let secs: i64 = raw.parse().ok()?;
    if secs <= 0 {
        return None;
    }
    warn_if_huge(std::time::Duration::from_secs(secs as u64))
}

fn warn_if_huge(d: std::time::Duration) -> Option<std::time::Duration> {
    if d.as_secs() as i64 > TEN_YEARS_SECS {
        tracing::warn!(
            hours = d.as_secs() / 3600,
            "AUTH_TOKEN_TTL exceeds 10 years; accepting but verify this is intentional"
        );
    }
    Some(d)
}

/// Minimal Go `time.ParseDuration`: sequence of <decimal><unit> with units
/// h/m/s/ms/us/µs/ns. Rejects empty numbers, unknown units, and <= 0 totals.
fn parse_go_duration(s: &str) -> Option<std::time::Duration> {
    if s.is_empty() {
        return None;
    }
    let mut rest = s;
    let mut total_ns: f64 = 0.0;
    while !rest.is_empty() {
        let num_end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if num_end == 0 {
            return None;
        }
        let num: f64 = rest[..num_end].parse().ok()?;
        rest = &rest[num_end..];

        // Longest unit match first; "µs" is multi-byte.
        let (unit_len, mult_ns) = if rest.starts_with("ms") {
            (2, 1_000_000.0)
        } else if rest.starts_with("µs") || rest.starts_with("us") {
            // Both spellings are 2 bytes ("µ" is 2-byte UTF-8).
            (2, 1_000.0)
        } else if rest.starts_with("ns") {
            (2, 1.0)
        } else if rest.starts_with('h') {
            (1, 3_600_000_000_000.0)
        } else if rest.starts_with('m') {
            (1, 60_000_000_000.0)
        } else if rest.starts_with('s') {
            (1, 1_000_000_000.0)
        } else {
            return None;
        };
        rest = &rest[unit_len..];
        total_ns += num * mult_ns;
    }
    if total_ns <= 0.0 || total_ns > (i64::MAX as f64) {
        return None;
    }
    Some(std::time::Duration::from_nanos(total_ns as u64))
}

/// Effective auth token lifetime in seconds: parsed TTL or the 30-day default.
pub fn auth_token_ttl_secs(raw: Option<&str>) -> i64 {
    if let Some(raw) = raw {
        if let Some(d) = parse_auth_token_ttl(raw) {
            return d.as_secs() as i64;
        }
        if !raw.trim().is_empty() {
            tracing::warn!(
                value = %raw,
                default_seconds = DEFAULT_AUTH_TOKEN_TTL_SECS,
                "AUTH_TOKEN_TTL is not a valid duration or positive integer; using default"
            );
        }
    }
    DEFAULT_AUTH_TOKEN_TTL_SECS
}

/// Process-wide effective auth token lifetime. The environment is read once,
/// matching Go's `sync.Once` configuration contract.
pub fn auth_token_ttl() -> i64 {
    *AUTH_TOKEN_TTL_SECS.get_or_init(|| {
        let raw = std::env::var("AUTH_TOKEN_TTL").ok();
        auth_token_ttl_secs(raw.as_deref())
    })
}

/// Resolves the cookie Domain attribute. An IP literal (optionally
/// dot-prefixed) is rejected with a warning — RFC 6265 §4.1.2.3 forbids IP
/// literals there and browsers silently drop such Set-Cookie headers.
pub fn cookie_domain(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let bare = raw.strip_prefix('.').unwrap_or(raw);
    if bare.parse::<std::net::IpAddr>().is_ok() {
        tracing::warn!(
            value = %raw,
            "COOKIE_DOMAIN looks like an IP address; ignoring. RFC 6265 forbids IP literals in the cookie Domain attribute, so browsers would drop the Set-Cookie. Leave COOKIE_DOMAIN empty for single-host deployments, or use a real domain."
        );
        return None;
    }
    Some(raw.to_string())
}

/// Session cookies carry the Secure flag iff FRONTEND_ORIGIN is https —
/// browsers silently drop Secure cookies on plain-HTTP pages, so the flag
/// tracks the user-facing scheme rather than an environment name.
pub fn is_secure_cookie(frontend_origin: Option<&str>) -> bool {
    let raw = frontend_origin.map(str::trim).unwrap_or("");
    if raw.is_empty() {
        return false;
    }
    match raw.split_once("://") {
        Some((scheme, _)) => scheme.eq_ignore_ascii_case("https"),
        None => false,
    }
}

/// CSRF token bound to the auth token via HMAC:
/// hex(nonce16) + "." + hex(HMAC-SHA256(nonce, key=authToken)).
/// An attacker who can write cookies on a subdomain cannot forge a valid
/// CSRF token without knowing the auth token.
pub fn generate_csrf_token(auth_token: &str) -> anyhow::Result<String> {
    let mut nonce = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut nonce);
    let nonce_hex = hex::encode(nonce);

    let mut mac = HmacSha256::new_from_slice(auth_token.as_bytes())?;
    mac.update(&nonce);
    Ok(format!(
        "{nonce_hex}.{}",
        hex::encode(mac.finalize().into_bytes())
    ))
}

/// Verifies X-CSRF-Token against the auth token by recomputing the HMAC
/// (constant-time compare), not by string equality. Safe-method gating
/// (GET/HEAD/OPTIONS skip) lives at the middleware layer.
pub fn verify_csrf_signature(auth_token: &str, csrf_header: &str) -> bool {
    let Some((nonce_hex, sig_hex)) = csrf_header.split_once('.') else {
        return false;
    };
    let (Ok(nonce), Ok(expected_sig)) = (hex::decode(nonce_hex), hex::decode(sig_hex)) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(auth_token.as_bytes()) else {
        return false;
    };
    mac.update(&nonce);
    mac.verify_slice(&expected_sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_parses_go_durations_and_seconds() {
        assert_eq!(
            parse_auth_token_ttl("8760h").unwrap(),
            std::time::Duration::from_secs(8760 * 3600)
        );
        assert_eq!(
            parse_auth_token_ttl("720h30m").unwrap(),
            std::time::Duration::from_secs((720 * 3600) + (30 * 60))
        );
        assert_eq!(
            parse_auth_token_ttl("3600").unwrap(),
            std::time::Duration::from_secs(3600)
        );
        assert_eq!(
            parse_auth_token_ttl("300ms").unwrap(),
            std::time::Duration::from_millis(300)
        );
    }

    #[test]
    fn ttl_rejects_invalid_and_nonpositive() {
        assert!(parse_auth_token_ttl("").is_none());
        assert!(parse_auth_token_ttl("   ").is_none());
        assert!(parse_auth_token_ttl("abc").is_none());
        assert!(parse_auth_token_ttl("-5h").is_none());
        assert!(parse_auth_token_ttl("0").is_none());
        assert!(parse_auth_token_ttl("h").is_none());
    }

    #[test]
    fn ttl_falls_back_to_default() {
        assert_eq!(auth_token_ttl_secs(None), DEFAULT_AUTH_TOKEN_TTL_SECS);
        assert_eq!(auth_token_ttl_secs(Some("")), DEFAULT_AUTH_TOKEN_TTL_SECS);
        assert_eq!(
            auth_token_ttl_secs(Some("garbage")),
            DEFAULT_AUTH_TOKEN_TTL_SECS
        );
        assert_eq!(auth_token_ttl_secs(Some("7200")), 7200);
    }

    #[test]
    fn csrf_roundtrip_and_tamper_rejection() {
        let auth_token = "mul_test_token";
        let csrf = generate_csrf_token(auth_token).unwrap();

        assert!(verify_csrf_signature(auth_token, &csrf));
        // Wrong auth token must fail.
        assert!(!verify_csrf_signature("mul_other", &csrf));
        // Tampered signature must fail.
        let mut tampered = csrf.clone();
        tampered.replace_range(
            csrf.len() - 1..,
            if csrf.ends_with('0') { "1" } else { "0" },
        );
        assert!(!verify_csrf_signature(auth_token, &tampered));
        // Malformed shapes must fail.
        assert!(!verify_csrf_signature(auth_token, "no-dot"));
        assert!(!verify_csrf_signature(auth_token, "zz.zz"));
    }

    #[test]
    fn cookie_domain_rejects_ip_literals() {
        assert_eq!(cookie_domain(None), None);
        assert_eq!(cookie_domain(Some("")), None);
        assert_eq!(cookie_domain(Some("192.168.1.1")), None);
        assert_eq!(cookie_domain(Some(".127.0.0.1")), None);
        assert_eq!(
            cookie_domain(Some("example.com")),
            Some("example.com".to_string())
        );
        assert_eq!(
            cookie_domain(Some(".example.com")),
            Some(".example.com".to_string())
        );
    }

    #[test]
    fn secure_flag_tracks_frontend_scheme() {
        assert!(!is_secure_cookie(None));
        assert!(!is_secure_cookie(Some("")));
        assert!(!is_secure_cookie(Some("http://app.example.com")));
        assert!(is_secure_cookie(Some("https://app.example.com")));
        assert!(is_secure_cookie(Some("HTTPS://app.example.com")));
        assert!(!is_secure_cookie(Some("app.example.com")));
    }
}
