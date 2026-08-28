//! Public-endpoint validation: the SSRF guard for outbound plugin traffic.
//!
//! Remote MCP endpoint and response validation.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use async_trait::async_trait;
use url::Url;

use crate::devorigin::is_dev_origin;
use crate::error::Error;

const SYSTEM_RESOLVER: SystemResolver = SystemResolver;

/// DNS resolution seam, mirroring Go's `remotemcp.Resolver`
/// (`LookupNetIP(ctx, network, host)`). The network argument is dropped:
/// both call sites ask for `"ip"`, and family filtering happens inside
/// [`is_public_address`].
#[async_trait]
pub trait Resolver: Send + Sync {
    async fn lookup_net_ip(&self, host: &str) -> Result<Vec<IpAddr>, Error>;
}

/// System resolver backed by `getaddrinfo` — Go's `net.DefaultResolver`.
#[derive(Debug, Clone, Copy)]
pub struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn lookup_net_ip(&self, host: &str) -> Result<Vec<IpAddr>, Error> {
        let port = 0u16;
        let addrs = tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| Error::Resolve(e.to_string()))?;
        let mut ips: Vec<IpAddr> = addrs.map(|a| a.ip()).collect();
        ips.sort();
        ips.dedup();
        Ok(ips)
    }
}

/// Validates that `raw` names a public HTTPS endpoint a Plugin may call.
///
/// https-only; userinfo/query/fragment rejected; localhost refused; every
/// resolved address must be public. An operator-named dev origin skips the
/// public-internet requirement and nothing else: the allowed-hosts policy
/// still applies, so a Plugin still cannot reach a destination the
/// administrator did not consent to.
pub async fn validate_public_https_endpoint(
    raw: &str,
    allowed_hosts: &[String],
    resolver: Option<&dyn Resolver>,
) -> Result<Url, Error> {
    let raw = raw.trim();
    let endpoint = Url::parse(raw).map_err(|e| Error::ParseEndpoint(e.to_string()))?;
    if endpoint.scheme() != "https"
        || endpoint.host_str().is_none_or(str::is_empty)
        || has_userinfo(raw)
        || non_empty(endpoint.fragment())
        || non_empty(endpoint.query())
    {
        return Err(Error::NotPublicHttps);
    }
    if is_dev_origin(&endpoint) {
        let dev_host = normalize_host(endpoint.host_str().unwrap_or_default());
        if !allowed_hosts.is_empty() && !host_allowed(&dev_host, allowed_hosts) {
            return Err(Error::HostOutsidePolicy);
        }
        return Ok(endpoint);
    }
    let host = normalize_host(endpoint.host_str().unwrap_or_default());
    if host == "localhost" || host.ends_with(".localhost") {
        return Err(Error::HostNotPublic);
    }
    if !allowed_hosts.is_empty() && !host_allowed(&host, allowed_hosts) {
        return Err(Error::HostOutsidePolicy);
    }
    let addresses = match resolver
        .unwrap_or(&SYSTEM_RESOLVER)
        .lookup_net_ip(&host)
        .await
    {
        Ok(addresses) => addresses,
        // Avoid double-prefixing the Go-parity message.
        Err(Error::Resolve(message)) => return Err(Error::Resolve(message)),
        Err(error) => return Err(Error::Resolve(error.to_string())),
    };
    if addresses.is_empty() {
        return Err(Error::Resolve("no addresses".to_string()));
    }
    for address in addresses {
        if !is_public_address(address) {
            return Err(Error::NonPublicResolved(address));
        }
    }
    Ok(endpoint)
}

/// Exact-host or `*.suffix` wildcard policy match. A wildcard never matches
/// the apex domain itself: `*.example.com` admits `tools.example.com` but not
/// `example.com`.
pub fn host_allowed(host: &str, policies: &[String]) -> bool {
    policies.iter().any(|policy| {
        let trimmed = policy.strip_suffix('.').unwrap_or(policy);
        let policy = trimmed.to_lowercase();
        if host == policy {
            return true;
        }
        if let Some(base) = policy.strip_prefix("*.") {
            let suffix = format!(".{base}");
            if host.ends_with(&suffix) && host != base {
                return true;
            }
        }
        false
    })
}

/// Reports whether `address` is on the public internet.
///
/// Mirrors Go's `isPublicAddress`: netip's helper checks plus the IANA
/// special-purpose ranges those helpers miss. IPv4-mapped IPv6 addresses are
/// evaluated as plain IPv4 (Go's `Addr.Unmap`). Deliberately NOT stricter
/// than Go: e.g. deprecated site-local `fec0::/10` stays admissible because
/// Go admits it too — parity is the security contract here.
pub fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(mapped) => is_public_v4(mapped),
            None => is_public_v6(v6),
        },
    }
}

const V4_BLOCKED_PREFIXES: [(Ipv4Addr, u8); 8] = [
    (Ipv4Addr::new(100, 64, 0, 0), 10), // CGNAT / shared address space
    (Ipv4Addr::new(192, 0, 0, 0), 24),  // IETF protocol assignments
    (Ipv4Addr::new(192, 0, 2, 0), 24),  // TEST-NET-1
    (Ipv4Addr::new(198, 18, 0, 0), 15), // benchmarking
    (Ipv4Addr::new(198, 51, 100, 0), 24), // TEST-NET-2
    (Ipv4Addr::new(203, 0, 113, 0), 24), // TEST-NET-3
    (Ipv4Addr::new(224, 0, 0, 0), 4),   // multicast (redundant with helper)
    (Ipv4Addr::new(240, 0, 0, 0), 4),   // reserved
];

fn is_public_v4(v4: Ipv4Addr) -> bool {
    !(v4.is_private()
        || v4.is_loopback()
        || v4.is_link_local()
        || v4.is_multicast()
        || v4.is_unspecified()
        || v4 == Ipv4Addr::BROADCAST
        || V4_BLOCKED_PREFIXES
            .iter()
            .any(|(base, bits)| v4_in_prefix(v4, *base, *bits)))
}

fn is_public_v6(v6: Ipv6Addr) -> bool {
    let segments = v6.segments();
    let unique_local = segments[0] & 0xfe00 == 0xfc00; // fc00::/7
    let link_local_unicast = segments[0] & 0xffc0 == 0xfe80; // fe80::/10
    let link_local_multicast = segments[0] == 0xff02; // ff02::/16
    let transition_or_reserved = [
        (Ipv6Addr::new(0x0064, 0xff9b, 0, 0, 0, 0, 0, 0), 96),
        (Ipv6Addr::new(0x0064, 0xff9b, 1, 0, 0, 0, 0, 0), 48),
        (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16),
        (Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 0), 10),
        (Ipv6Addr::new(0x0100, 0, 0, 0, 0, 0, 0, 0), 64),
        (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23),
    ]
    .iter()
    .any(|(base, bits)| v6_in_prefix(v6, *base, *bits));
    !(unique_local
        || link_local_unicast
        || link_local_multicast
        || v6.is_loopback()
        || v6.is_multicast()
        || v6.is_unspecified()
        || transition_or_reserved
        || v6_in_prefix(v6, Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0), 32))
}

fn v4_in_prefix(ip: Ipv4Addr, base: Ipv4Addr, bits: u8) -> bool {
    if bits == 0 {
        return true;
    }
    let shift = 32 - u32::from(bits);
    (u32::from(ip) >> shift) == (u32::from(base) >> shift)
}

fn v6_in_prefix(ip: Ipv6Addr, base: Ipv6Addr, bits: u8) -> bool {
    if bits == 0 {
        return true;
    }
    let shift = 128 - u128::from(bits);
    (u128::from(ip) >> shift) == (u128::from(base) >> shift)
}

/// Lowercase, strip IPv6 brackets and one trailing dot — the Rust equivalent
/// of Go's `strings.ToLower(strings.TrimSuffix(u.Hostname(), "."))`.
pub(crate) fn normalize_host(raw: &str) -> String {
    let no_brackets = raw
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(raw);
    let no_dot = no_brackets.strip_suffix('.').unwrap_or(no_brackets);
    no_dot.to_lowercase()
}

/// Detects userinfo (`user[:pass]@`) in the authority component.
///
/// The `url` crate cannot distinguish `https://host/` from `https://@host/`
/// through its accessors, so scan the authority text directly — Go rejects
/// any non-nil `URL.User`.
fn has_userinfo(raw: &str) -> bool {
    let Some((_, rest)) = raw.split_once("://") else {
        return false;
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest[..authority_end].contains('@')
}

fn non_empty(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    struct StaticResolver(BTreeMap<String, Vec<IpAddr>>);

    #[async_trait]
    impl Resolver for StaticResolver {
        async fn lookup_net_ip(&self, host: &str) -> Result<Vec<IpAddr>, Error> {
            Ok(self.0.get(host).cloned().unwrap_or_default())
        }
    }

    fn hosts(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    #[tokio::test]
    async fn accepts_public_endpoint_within_policy() {
        let resolver = StaticResolver(BTreeMap::from([(
            "mcp.example.com".to_string(),
            vec![IpAddr::from([8, 8, 8, 8])],
        )]));
        let endpoint = validate_public_https_endpoint(
            "https://mcp.example.com/v1/mcp",
            &hosts(&["mcp.example.com"]),
            Some(&resolver),
        )
        .await
        .unwrap();
        assert_eq!(endpoint.host_str(), Some("mcp.example.com"));
    }

    #[tokio::test]
    async fn rejects_non_public_endpoints() {
        let resolver = StaticResolver(BTreeMap::from([
            (
                "mcp.example.com".to_string(),
                vec![IpAddr::from([8, 8, 8, 8])],
            ),
            (
                "private.example".to_string(),
                vec![IpAddr::from([169, 254, 169, 254])],
            ),
        ]));
        for raw in [
            "http://mcp.example.com/mcp",
            "https://localhost/mcp",
            "https://sub.localhost/mcp",
            "https://localhost.:443/mcp",
            "https://token@mcp.example.com/mcp",
            "https://@mcp.example.com/mcp",
            "https://private.example/mcp",
            "https://mcp.example.com/mcp?x=1",
            "https://mcp.example.com/mcp#frag",
        ] {
            assert!(
                validate_public_https_endpoint(raw, &[], Some(&resolver))
                    .await
                    .is_err(),
                "endpoint {raw} was accepted"
            );
        }
    }

    #[tokio::test]
    async fn reports_policy_violations() {
        let resolver = StaticResolver(BTreeMap::from([(
            "mcp.example.com".to_string(),
            vec![IpAddr::from([8, 8, 8, 8])],
        )]));
        let error = validate_public_https_endpoint(
            "https://mcp.example.com/mcp",
            &hosts(&["other.example.com"]),
            Some(&resolver),
        )
        .await
        .expect_err("policy violation expected");
        assert!(error.to_string().contains("policy"), "{error}");
    }

    #[tokio::test]
    async fn tolerates_trailing_dot_in_host() {
        let resolver = StaticResolver(BTreeMap::from([(
            "mcp.example.com".to_string(),
            vec![IpAddr::from([8, 8, 8, 8])],
        )]));
        let endpoint = validate_public_https_endpoint(
            "https://mcp.example.com./mcp",
            &hosts(&["mcp.example.com"]),
            Some(&resolver),
        )
        .await
        .unwrap();
        assert_eq!(endpoint.host_str(), Some("mcp.example.com."));
    }

    #[test]
    fn wildcard_does_not_match_apex() {
        assert!(host_allowed(
            "tools.example.com",
            &hosts(&["*.example.com"])
        ));
        assert!(!host_allowed("example.com", &hosts(&["*.example.com"])));
        assert!(host_allowed("example.com", &hosts(&["example.com."])));
        assert!(!host_allowed("attacker.com", &hosts(&["*.example.com"])));
    }

    #[test]
    fn ip_range_matrix_matches_go_blocklist() {
        let cases: &[(&str, bool)] = &[
            ("8.8.8.8", true),
            ("1.1.1.1", true),
            ("100.63.255.255", true),
            ("100.128.0.0", true),
            ("192.0.1.1", true),
            ("198.17.255.255", true),
            ("198.20.0.1", true),
            ("203.0.114.1", true),
            ("2606:4700::1111", true),
            ("2001:4860:4860::8888", true),
            ("::ffff:8.8.8.8", true),
            ("10.1.2.3", false),
            ("172.16.0.1", false),
            ("172.31.255.255", false),
            ("192.168.1.1", false),
            ("127.0.0.1", false),
            ("169.254.169.254", false),
            ("0.0.0.0", false),
            ("100.64.0.0", false),
            ("100.127.255.255", false),
            ("192.0.0.1", false),
            ("192.0.2.1", false),
            ("198.18.0.1", false),
            ("198.19.255.255", false),
            ("198.51.100.1", false),
            ("203.0.113.1", false),
            ("224.0.0.1", false),
            ("239.255.255.255", false),
            ("240.0.0.1", false),
            ("255.255.255.255", false),
            ("::", false),
            ("::1", false),
            ("fe80::1", false),
            ("febf:ffff::1", false),
            ("fd00::1", false),
            ("fdff::1", false),
            ("2001:db8::1", false),
            ("2001:db8:ffff:ffff:ffff:ffff:ffff:ffff", false),
            ("64:ff9b::7f00:1", false),
            ("64:ff9b:1::a9fe:a9fe", false),
            ("2002:7f00:1::1", false),
            ("fec0::1", false),
            ("100::1", false),
            ("ff02::1", false),
            ("::ffff:10.0.0.1", false),
        ];
        for (raw, want_public) in cases {
            let address: IpAddr = raw.parse().unwrap();
            assert_eq!(is_public_address(address), *want_public, "address {raw}");
        }
    }
}
