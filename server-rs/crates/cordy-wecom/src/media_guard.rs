//! The media fetcher is pointed at an address by somebody else — port of
//! `media_guard.go`.
//!
//! A callback carries a pre-signed COS URL and we GET it. The URL is a string
//! that arrived over the socket: WeCom put it there, but the adapter cannot
//! prove that, and the fetch runs from inside the deployment's network with
//! whatever reach that network has. On this machine alone that reach includes
//! a Tailscale tailnet (100.64.0.0/10), a proxy's fake-IP range
//! (198.18.0.0/15), Docker's bridges, and the loopback the backend's own
//! admin endpoints listen on.
//!
//! So the guard is not on the URL, it is on the CONNECTION. Checking the
//! hostname is worth nothing on its own: the URL can redirect (a 302 to
//! http://169.254.169.254/ is one line of attacker-controlled response), and
//! a hostname that passed a check can resolve to something else a moment
//! later. A DNS resolver that refuses a non-public answer runs on every hop
//! of every redirect, against the name actually being connected to, which is
//! the only place both of those are covered at once.
//!
//! Port note: Go guards in DialContext after resolving; reqwest exposes the
//! same seam one step earlier — a custom [`reqwest::dns::Resolve`] whose
//! answers are filtered through the same address policy before any connection
//! is attempted. Redirects are re-resolved by the client, so every hop is
//! checked too.

use std::net::IpAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use ipnetwork::IpNetwork;

/// Returned instead of an address when every address a media host resolves to
/// is one the deployment must not be pointed at. Deliberately distinct from a
/// dial failure: the caller logs the two differently, and only this one means
/// somebody sent us a URL they should not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("wecom: media host resolves to a non-public address")]
pub struct MediaAddrBlocked;

/// Bounds one TCP connect. It sits inside the media download timeout, which
/// bounds the whole fetch.
pub const MEDIA_DIAL_TIMEOUT: Duration = Duration::from_secs(10);

const RESERVED_MEDIA_PREFIXES: &[&str] = &[
    // ---- IPv4 ----
    "0.0.0.0/8",       // "this network"
    "100.64.0.0/10",   // RFC 6598 CGNAT — Tailscale lives here
    "192.0.0.0/24",    // IETF protocol assignments
    "192.0.2.0/24",    // TEST-NET-1
    "198.18.0.0/15",   // benchmarking — and a proxy's fake-IP range
    "198.51.100.0/24", // TEST-NET-2
    "203.0.113.0/24",  // TEST-NET-3
    "240.0.0.0/4",     // reserved, includes 255.255.255.255
    "192.88.99.0/24",  // deprecated 6to4 relay anycast — the v4 end of 2002::/16
    // ---- IPv6 ----
    "100::/64",       // discard-only
    "100:0:0:1::/64", // dummy prefix, also discard-only
    // The whole IETF protocol-assignments block, not its dozen sub-entries.
    // It holds Teredo (2001::/32), benchmarking (2001:2::/48 — the twin of
    // 198.18.0.0/15 above), AMT, AS112-v6, ORCHID/ORCHIDv2, DET, and the PCP /
    // TURN / DNS-SD anycast addresses. Documentation space is 2001:db8::/32,
    // outside this /23 and listed separately.
    "2001::/23",
    "2001:db8::/32", // documentation
    "3fff::/20",     // documentation, RFC 9637
    "5f00::/16",     // SRv6 SIDs, RFC 9602 — routing labels, not hosts
    // Site-local. RFC 3879 deprecated it and IANA delisted it, which is why
    // no predicate and no registry row covers it — but the networks that were
    // numbered out of it before 2004 still route it internally.
    "fec0::/10",
];

/// TRANSLATION space, and no configuration reopens it. These addresses are
/// not destinations, they are IPv4 destinations wearing an IPv6 costume:
/// 64:ff9b:1::a9fe:a9fe is 169.254.169.254 the moment a NAT64 translator sees
/// it, and 2002:7f00:1::1 is 127.0.0.1 through a 6to4 relay.
///
/// That costume is why the split exists. The early loopback/private/link-local
/// checks never fire on these — at that point they are IPv6 addresses, and
/// every one of them reaches straight past a guard that only knows how to
/// recognise an IPv4 address when it is written as one. This list is the ONLY
/// thing standing between the guard and the address embedded inside. Letting
/// the operator allow-list override it would mean CORDY_WECOM_MEDIA_ALLOW_CIDRS=::/0
/// reaching the loopback and the metadata endpoint the guard exists to
/// refuse, written in a spelling the operator never thought they were opening.
const TRANSLATION_MEDIA_PREFIXES: &[&str] = &["64:ff9b::/96", "64:ff9b:1::/48", "2002::/16"];

fn parse_prefixes(list: &[&str]) -> Vec<IpNetwork> {
    list.iter()
        .filter_map(|raw| raw.parse::<IpNetwork>().ok())
        .collect()
}

fn reserved() -> &'static [IpNetwork] {
    static LIST: std::sync::OnceLock<Vec<IpNetwork>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| parse_prefixes(RESERVED_MEDIA_PREFIXES))
}

fn translation() -> &'static [IpNetwork] {
    static LIST: std::sync::OnceLock<Vec<IpNetwork>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| parse_prefixes(TRANSLATION_MEDIA_PREFIXES))
}

/// Ranges an operator has declared safe for media fetches despite looking
/// reserved. It exists for one real deployment shape: a machine behind a
/// fake-IP proxy, where the resolver answers every public hostname with an
/// address out of 198.18.0.0/15 and the proxy forwards the traffic onward. On
/// such a machine WeCom's own COS host is indistinguishable from a link-local
/// metadata endpoint by address alone, so the guard refuses every attachment
/// and inbound media stops working entirely.
///
/// Empty by default. Widening this is a decision with a cost: whatever range
/// is listed here can be reached by a URL somebody else controls, which is
/// exactly what the guard exists to prevent.
///
/// What it CANNOT open, at any width: loopback, the private ranges and
/// link-local, which are refused before this list is consulted; and
/// [`TRANSLATION_MEDIA_PREFIXES`], which is refused for the same reason one
/// step later.
static MEDIA_ALLOWED_PREFIXES: RwLock<Vec<IpNetwork>> = RwLock::new(Vec::new());

/// Declares ranges the media guard may dial. Called at boot from
/// CORDY_WECOM_MEDIA_ALLOW_CIDRS. An unparseable entry is reported and
/// skipped rather than silently widening or silently narrowing the guard.
pub fn set_media_allowed_prefixes(cidrs: &[String]) -> Vec<anyhow::Error> {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    for raw in cidrs {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        match raw.parse::<IpNetwork>() {
            Ok(p) => out.push(p),
            Err(e) => errs.push(anyhow::anyhow!("wecom: media allow cidr {raw:?}: {e}")),
        }
    }
    *MEDIA_ALLOWED_PREFIXES
        .write()
        .unwrap_or_else(|e| e.into_inner()) = out;
    errs
}

fn allowed_prefixes() -> Vec<IpNetwork> {
    MEDIA_ALLOWED_PREFIXES
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// The base refusals every address faces before any allow-list is consulted:
/// everything that is not routable public internet.
fn base_addr_refused(a: IpAddr) -> bool {
    match a {
        IpAddr::V4(v) => {
            v.is_loopback()
                || v.is_private()
                || v.is_unspecified()
                || v.is_link_local()
                || v.is_broadcast()
                || v.is_multicast()
        }
        IpAddr::V6(v) => {
            v.is_loopback()
                || v.is_unspecified()
                || v.is_multicast()
                || v.is_unicast_link_local()
                // Interface-local (ff01::/16) and link-local (ff02::/16)
                // multicast scopes.
                || matches!(v.segments()[0], 0xff01 | 0xff02)
                // Unique-local fc00::/7 — the v6 private range.
                || v.segments()[0] & 0xfe00 == 0xfc00
        }
    }
}

/// The production policy: everything that is not routable public internet is
/// refused.
///
/// An IPv4-mapped IPv6 address (::ffff:127.0.0.1) reports none of the IPv4
/// predicates until it is unmapped, which is the whole trick.
pub fn public_addr_only(addr: IpAddr) -> bool {
    let a = addr.to_canonical();
    if base_addr_refused(a) {
        return false;
    }
    // Translation space first, and before the allow-list is consulted at all.
    // The address inside one of these is an IPv4 address the checks above
    // would have refused on sight; the only reason they did not fire is the
    // spelling. Nothing an operator configures reopens it.
    if translation().iter().any(|p| p.contains(a)) {
        return false;
    }
    if reserved().iter().any(|p| p.contains(a)) {
        // An operator may have declared this range theirs — a fake-IP proxy's
        // pool is the case this exists for. Checked only for addresses the
        // guard would otherwise refuse, so an empty allow-list leaves the
        // guard exactly as strict as before.
        return allowed_prefixes().iter().any(|allowed| allowed.contains(a));
    }
    true
}

/// The address policy a guard consults per resolved address. Production uses
/// [`public_addr_only`]; tests substitute a policy that allows the loopback
/// their own server is on, so that what is under test is the guard's decision
/// rather than the test harness's address.
pub type AddrPolicy = Arc<dyn Fn(IpAddr) -> bool + Send + Sync>;

/// The guard every media download goes through. It resolves the destination
/// itself and hands the client only checked addresses — never the hostname,
/// because resolving twice is the rebinding window.
#[derive(Clone, Default)]
pub struct MediaGuard {
    /// None selects [`public_addr_only`].
    pub allow: Option<AddrPolicy>,
}

impl MediaGuard {
    pub fn new() -> Self {
        Self::default()
    }

    fn policy(&self) -> AddrPolicy {
        self.allow
            .clone()
            .unwrap_or_else(|| Arc::new(public_addr_only))
    }
}

struct GuardedResolve {
    allow: AddrPolicy,
}

impl reqwest::dns::Resolve for GuardedResolve {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let allow = self.allow.clone();
        Box::pin(async move {
            let host = name.as_str();
            let addrs: Vec<_> = tokio::net::lookup_host((host, 0u16))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("wecom: media dial: resolve {host}: {e}").into()
                })?
                .collect();
            let mut checked = Vec::with_capacity(addrs.len());
            for sa in addrs {
                if allow(sa.ip()) {
                    checked.push(sa);
                }
            }
            if checked.is_empty() {
                // Every resolved address was refused. The refusal carries no
                // address of its own so it is safe to log whole.
                return Err(Box::new(MediaAddrBlocked) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(checked.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Builds the client every media download goes through.
///
/// Two guards, and they cover different things. The resolver refuses a
/// destination; the redirect policy refuses a SCHEME, because a redirect to
/// file:// or gopher:// never reaches a resolver at all and the transport
/// would happily hand it to a protocol handler.
pub fn new_media_http_client(guard: MediaGuard) -> anyhow::Result<reqwest::Client> {
    let allow = guard.policy();
    reqwest::Client::builder()
        .dns_resolver(Arc::new(GuardedResolve { allow }))
        .redirect(redirect_policy())
        // Nothing about a COS object needs a proxy, and honouring HTTP_PROXY
        // here would send the fetch to an address the guard never sees.
        .no_proxy()
        .connect_timeout(MEDIA_DIAL_TIMEOUT)
        .build()
        .map_err(|e| anyhow::anyhow!("wecom: build media http client: {e}"))
}

fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 {
            attempt.error("wecom: media download: too many redirects")
        } else {
            if !matches!(attempt.url().scheme(), "http" | "https") {
                let scheme = attempt.url().scheme().to_string();
                attempt.error(format!(
                    "wecom: media redirect to scheme {scheme:?} refused"
                ))
            } else {
                attempt.follow()
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    #[test]
    fn loopback_and_private_are_refused_even_when_allow_listed() {
        assert!(!public_addr_only(IpAddr::from([127, 0, 0, 1])));
        assert!(!public_addr_only(IpAddr::from([10, 0, 0, 1])));
        assert!(!public_addr_only(IpAddr::from([192, 168, 1, 1])));
        assert!(!public_addr_only(IpAddr::from([169, 254, 1, 1])));
        assert!(!public_addr_only(IpAddr::from(Ipv6Addr::LOCALHOST)));
        assert!(!public_addr_only(IpAddr::from([
            0xfd00, 0, 0, 0, 0, 0, 0, 1
        ])));

        // The allow-list cannot reopen any of these.
        set_media_allowed_prefixes(&["::/0".to_string(), "0.0.0.0/0".to_string()]);
        assert!(!public_addr_only(IpAddr::from([127, 0, 0, 1])));
        assert!(!public_addr_only(IpAddr::from([10, 0, 0, 1])));
        set_media_allowed_prefixes(&[]);
    }

    #[test]
    fn cgnat_is_reserved_but_reopenable() {
        let tailscale = IpAddr::from([100, 101, 102, 103]);
        assert!(!public_addr_only(tailscale));
        set_media_allowed_prefixes(&["100.64.0.0/10".to_string()]);
        assert!(public_addr_only(tailscale));
        set_media_allowed_prefixes(&[]);
    }

    #[test]
    fn fake_ip_range_is_reopenable_for_proxy_deployments() {
        let fake_ip = IpAddr::from([198, 18, 0, 1]);
        assert!(!public_addr_only(fake_ip));
        set_media_allowed_prefixes(&["198.18.0.0/15".to_string()]);
        assert!(public_addr_only(fake_ip));
        set_media_allowed_prefixes(&[]);
    }

    #[test]
    fn translation_space_is_never_reopenable() {
        set_media_allowed_prefixes(&["::/0".to_string()]);
        // NAT64 well-known prefix wrapping the metadata endpoint.
        let nat64 = IpAddr::from(Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0xa9fe, 0xa9fe));
        assert!(!public_addr_only(nat64));
        // 6to4 wrapping loopback.
        let sixto4 = IpAddr::from(Ipv6Addr::new(0x2002, 0x7f00, 0x0001, 0, 0, 0, 0, 0));
        assert!(!public_addr_only(sixto4));
        set_media_allowed_prefixes(&[]);
    }

    #[test]
    fn documentation_and_test_nets_are_refused() {
        assert!(!public_addr_only(IpAddr::from([192, 0, 2, 1])));
        assert!(!public_addr_only(IpAddr::from([198, 51, 100, 7])));
        assert!(!public_addr_only(IpAddr::from([203, 0, 113, 9])));
        assert!(!public_addr_only(IpAddr::from([240, 1, 2, 3])));
        assert!(!public_addr_only(IpAddr::from([
            0x2001, 0xdb8, 0, 0, 0, 0, 0, 1
        ])));
    }

    #[test]
    fn public_addresses_pass() {
        assert!(public_addr_only(IpAddr::from([8, 8, 8, 8])));
        assert!(public_addr_only(IpAddr::from([1, 1, 1, 1])));
        assert!(public_addr_only(IpAddr::from(Ipv6Addr::new(
            0x2606, 0x4700, 0, 0, 0, 0, 0, 0x1111
        ))));
    }

    #[test]
    fn ipv4_mapped_ipv6_is_unmapped_before_checks() {
        let mapped = IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 1));
        assert!(!public_addr_only(mapped));
        let mapped_public = IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x0808, 0x0808));
        assert!(public_addr_only(mapped_public));
    }

    #[test]
    fn unparseable_cidrs_are_reported_not_applied() {
        let errs = set_media_allowed_prefixes(&[
            "not-a-cidr".to_string(),
            "  ".to_string(),
            "10.0.0.0/8".to_string(),
        ]);
        assert_eq!(errs.len(), 1);
        // The valid entry replaced the list wholesale.
        assert_eq!(allowed_prefixes().len(), 1);
        set_media_allowed_prefixes(&[]);
        assert!(allowed_prefixes().is_empty());
    }
}
