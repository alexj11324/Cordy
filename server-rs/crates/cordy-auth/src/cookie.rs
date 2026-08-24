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

/// Installs the final TOML+environment auth TTL before handlers mint tokens.
pub fn configure_auth_token_ttl(raw: Option<&str>) -> anyhow::Result<()> {
    AUTH_TOKEN_TTL_SECS
        .set(auth_token_ttl_secs(raw))
        .map_err(|_| anyhow::anyhow!("auth token TTL was already initialized"))
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

/// Values for the two `Set-Cookie` headers that clear the session and CSRF
/// cookies. `Max-Age=0` is Go net/http's wire representation for MaxAge=-1.
pub fn clear_auth_cookie_values(domain: Option<&str>, secure: bool) -> [String; 2] {
    [
        clear_cookie_value(AUTH_COOKIE_NAME, domain, secure, true),
        clear_cookie_value(CSRF_COOKIE_NAME, domain, secure, false),
    ]
}

/// Values for the auth and CSRF `Set-Cookie` headers issued after login.
/// Attribute order and Max-Age/Expires semantics match Go's `http.SetCookie`.
pub fn set_auth_cookie_values(
    token: &str,
    domain: Option<&str>,
    secure: bool,
) -> anyhow::Result<[String; 2]> {
    set_auth_cookie_values_at(token, domain, secure, chrono::Utc::now(), auth_token_ttl())
}

fn set_auth_cookie_values_at(
    token: &str,
    domain: Option<&str>,
    secure: bool,
    now: chrono::DateTime<chrono::Utc>,
    ttl: i64,
) -> anyhow::Result<[String; 2]> {
    let expires = now + chrono::Duration::seconds(ttl);
    let csrf = generate_csrf_token(token)?;
    Ok([
        session_cookie_value(AUTH_COOKIE_NAME, token, domain, secure, true, ttl, expires),
        session_cookie_value(CSRF_COOKIE_NAME, &csrf, domain, secure, false, ttl, expires),
    ])
}

fn session_cookie_value(
    name: &str,
    value: &str,
    domain: Option<&str>,
    secure: bool,
    http_only: bool,
    max_age: i64,
    expires: chrono::DateTime<chrono::Utc>,
) -> String {
    let mut cookie = format!("{name}={value}; Path=/");
    if let Some(domain) = domain.filter(|value| !value.is_empty()) {
        cookie.push_str("; Domain=");
        cookie.push_str(domain);
    }
    cookie.push_str("; Expires=");
    cookie.push_str(&expires.format("%a, %d %b %Y %H:%M:%S GMT").to_string());
    cookie.push_str("; Max-Age=");
    cookie.push_str(&max_age.to_string());
    if http_only {
        cookie.push_str("; HttpOnly");
    }
    if secure {
        cookie.push_str("; Secure");
    }
    cookie.push_str("; SameSite=Strict");
    cookie
}

fn clear_cookie_value(name: &str, domain: Option<&str>, secure: bool, http_only: bool) -> String {
    let mut value = format!("{name}=; Path=/");
    if let Some(domain) = domain.filter(|value| !value.is_empty()) {
        value.push_str("; Domain=");
        value.push_str(domain);
    }
    value.push_str("; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0");
    if http_only {
        value.push_str("; HttpOnly");
    }
    if secure {
        value.push_str("; Secure");
    }
    value.push_str("; SameSite=Strict");
    value
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

    #[test]
    fn clear_cookie_values_match_go_logout_contract() {
        let values = clear_auth_cookie_values(Some(".example.com"), true);
        assert_eq!(
            values[0],
            "cordy_auth=; Path=/; Domain=.example.com; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; HttpOnly; Secure; SameSite=Strict"
        );
        assert_eq!(
            values[1],
            "cordy_csrf=; Path=/; Domain=.example.com; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Max-Age=0; Secure; SameSite=Strict"
        );

        let local = clear_auth_cookie_values(None, false);
        assert!(!local[0].contains("Domain="));
        assert!(!local[0].contains("; Secure"));
        assert!(local[0].contains("; HttpOnly"));
        assert!(!local[1].contains("; HttpOnly"));
    }

    #[test]
    fn set_cookie_values_match_go_login_contract() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-23T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let values = set_auth_cookie_values_at(
            "header.payload.sig",
            Some(".example.com"),
            true,
            now,
            DEFAULT_AUTH_TOKEN_TTL_SECS,
        )
        .unwrap();

        assert!(values[0]
            .starts_with("cordy_auth=header.payload.sig; Path=/; Domain=.example.com; Expires="));
        assert!(values[0].contains("; Max-Age=2592000; HttpOnly; Secure; SameSite=Strict"));
        assert!(values[1].starts_with("cordy_csrf="));
        assert!(values[1].contains("; Domain=.example.com; Expires="));
        assert!(values[1].contains("; Max-Age=2592000; Secure; SameSite=Strict"));
        assert!(!values[1].contains("; HttpOnly"));
    }
}
