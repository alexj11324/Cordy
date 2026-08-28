//! DingTalk media resolution.
//!
//! DingTalk publishes no inbound image limits. Keep the adapter's memory and
//! remote-I/O budget deliberately below the shared Router's 45-second media
//! deadline: at most two 10 MiB downloads are buffered concurrently.
//!
//! Security model (mirrors Go): DingTalk's authenticated messageFiles/download
//! API can return a public HTTP URL as well as HTTPS. The adapter honors that
//! provider-issued URL without rewriting its scheme, but the URL remains
//! untrusted egress input: every hop is validated for shape/scheme, HTTPS may
//! not downgrade, plain HTTP may not leave its origin, the Referer header
//! never crosses a redirect, downloads are capped in bytes, and DNS resolves
//! only through [`is_public_download_address`] so loopback/RFC1918/link-local
//! and IPv6-transition targets are refused before any connection is dialed.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use cordy_channel::{InboundMessage, MediaRef};
use cordy_channel_engine::resolvers::{
    MediaIntentLedger, MediaResolver, RecordPendingMediaObjectParams, ResolvedIdentity,
    ResolvedInstallation,
};

use crate::client::{is_unauthorized, Client};
use crate::config::{decode_credentials, Credentials, Decrypter};
use crate::resolvers::decode_dingtalk_raw;

pub const MAX_IMAGES_PER_MESSAGE: usize = 4;
pub const MAX_INBOUND_IMAGE_BYTES: usize = 10 << 20;
pub const IMAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Bounds concurrent remote fetches per message (Go errgroup.SetLimit(2)).
pub const MEDIA_FETCH_CONCURRENCY: usize = 2;
pub const MAX_DOWNLOAD_REDIRECTS: usize = 3;

/// The content types an inbound image may carry, mapped to their stored file
/// extension. Anything else is refused after sniffing.
fn allowed_image_ext(content_type: &str) -> Option<&'static str> {
    match content_type {
        "image/png" => Some(".png"),
        "image/jpeg" => Some(".jpg"),
        "image/gif" => Some(".gif"),
        "image/webp" => Some(".webp"),
        "image/bmp" => Some(".bmp"),
        _ => None,
    }
}

/// Defines the object-store operations required for ingestion. `object_url`
/// must derive the final URL without performing I/O so the resolver can
/// persist that URL in the intent ledger before uploading the object.
pub trait MediaStorage: Send + Sync {
    fn upload(
        &self,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>>;
    /// Derives the final URL of an object without I/O.
    fn object_url(&self, key: &str) -> String;
}

/// The DingTalk media resolver. Implements the engine [`MediaResolver`] seam.
pub struct MediaResolverImpl {
    client: Arc<Client>,
    decrypt: Option<Arc<Decrypter>>,
    storage: Arc<dyn MediaStorage>,
    ledger: Arc<dyn MediaIntentLedger>,
}

impl MediaResolverImpl {
    pub fn new(
        client: Arc<Client>,
        decrypt: Option<Arc<Decrypter>>,
        storage: Arc<dyn MediaStorage>,
        ledger: Arc<dyn MediaIntentLedger>,
    ) -> Self {
        Self {
            client,
            decrypt,
            storage,
            ledger,
        }
    }
}

/// Validates a download URL's untrusted shape: a host must exist, credentials
/// may not ride in userinfo, no fragment, and only http/https schemes.
fn validate_download_url(parsed: &url::Url) -> anyhow::Result<()> {
    if parsed.host_str().is_none_or(str::is_empty)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || !parsed.fragment().unwrap_or("").is_empty()
    {
        anyhow::bail!("invalid image download URL shape");
    }
    let scheme = parsed.scheme();
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        anyhow::bail!("invalid image download URL scheme {scheme:?}");
    }
    Ok(())
}

/// Reports whether two URLs share scheme + host (case-insensitive), the origin
/// check a plain-http redirect must pass.
fn same_download_origin(a: &url::Url, b: &url::Url) -> bool {
    a.scheme().eq_ignore_ascii_case(b.scheme())
        && match (a.host_str(), b.host_str()) {
            (Some(ha), Some(hb)) => ha.eq_ignore_ascii_case(hb),
            _ => false,
        }
}

const NON_PUBLIC_DOWNLOAD_PREFIXES_V4: [(Ipv4Addr, u8); 8] = [
    (Ipv4Addr::UNSPECIFIED, 8),
    (Ipv4Addr::new(100, 64, 0, 0), 10),
    (Ipv4Addr::new(192, 0, 0, 0), 24),
    (Ipv4Addr::new(192, 0, 2, 0), 24),
    (Ipv4Addr::new(198, 18, 0, 0), 15),
    (Ipv4Addr::new(198, 51, 100, 0), 24),
    (Ipv4Addr::new(203, 0, 113, 0), 24),
    (Ipv4Addr::new(240, 0, 0, 0), 4),
];

// IPv6 transition mechanisms can encapsulate an otherwise-blocked IPv4
// destination in an address that std classifies as global unicast. The
// download client does not need these legacy/local transition ranges, so fail
// closed instead of attempting to decode every deployment-specific mapping
// and risking a route into loopback or RFC1918 space.
const NON_PUBLIC_DOWNLOAD_PREFIXES_V6: [(Ipv6Addr, u8); 10] = [
    (Ipv6Addr::UNSPECIFIED, 96), // deprecated IPv4-compatible ::/96
    (Ipv6Addr::new(0x0064, 0xff9b, 1, 0, 0, 0, 0, 0), 48), // local-use NAT64
    (Ipv6Addr::new(0x0100, 0, 0, 0, 0, 0, 0, 0), 64), // discard-only
    (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 32), // Teredo
    (Ipv6Addr::new(0x2001, 0x0002, 0, 0, 0, 0, 0, 0), 48), // benchmarking
    (Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0), 32), // documentation
    (Ipv6Addr::new(0x2001, 0x0010, 0, 0, 0, 0, 0, 0), 28), // deprecated ORCHID
    (Ipv6Addr::new(0x2001, 0x0020, 0, 0, 0, 0, 0, 0), 28), // ORCHIDv2
    (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16), // 6to4
    (Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20), // documentation
];

/// The standard NAT64 prefix (64:ff9b::/96) carries an embedded IPv4 address
/// in its low 32 bits; [`is_public_download_address`] recurses into it.
const WELL_KNOWN_NAT64_HEAD: [u16; 6] = [0x0064, 0xff9b, 0, 0, 0, 0];

fn v4_in_prefix(v4: Ipv4Addr, base: Ipv4Addr, bits: u8) -> bool {
    if bits == 0 {
        return true;
    }
    let shift = 32 - u32::from(bits);
    (u32::from(v4) >> shift) == (u32::from(base) >> shift)
}

fn v6_in_prefix(v6: Ipv6Addr, base: Ipv6Addr, bits: u8) -> bool {
    let (addr, base) = (u128::from(v6), u128::from(base));
    match bits {
        0 => true,
        128 => addr == base,
        bits => (addr >> (128 - u32::from(bits))) == (base >> (128 - u32::from(bits))),
    }
}

/// Reports whether `addr` may carry a media download connection. This preserves the
/// `isPublicDownloadAddress`: std helper checks plus the special-purpose
/// ranges those helpers miss, evaluated on the unmapped address (Go's
/// `Addr.Unmap`).
pub(crate) fn is_public_download_address(addr: IpAddr) -> bool {
    let addr = match addr {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(mapped) => IpAddr::V4(mapped),
            None => IpAddr::V6(v6),
        },
        IpAddr::V4(_) => addr,
    };
    match addr {
        IpAddr::V4(v4) => {
            // Go IsGlobalUnicast(v4): not unspecified/broadcast/loopback/
            // multicast/link-local; RFC1918 stays "global" there but is then
            // refused by IsPrivate.
            !(v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_loopback()
                || v4.is_multicast()
                || v4.is_link_local()
                || v4.is_private())
                && !NON_PUBLIC_DOWNLOAD_PREFIXES_V4
                    .iter()
                    .any(|(base, bits)| v4_in_prefix(v4, *base, *bits))
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            // Go IsGlobalUnicast(v6): not ::, ::1, ff00::/8, fe80::/10.
            if v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || segments[0] & 0xffc0 == 0xfe80
            {
                return false;
            }
            if segments[0] & 0xfe00 == 0xfc00 {
                // fc00::/7 unique-local (Go IsPrivate for v6).
                return false;
            }
            if segments[..6] == WELL_KNOWN_NAT64_HEAD {
                // Permit a synthesized public target only if its embedded IPv4
                // passes the complete deny policy — an attacker-controlled AAAA
                // record must not smuggle loopback/RFC1918 through an
                // apparently global value.
                return is_public_download_address(IpAddr::V4(Ipv4Addr::new(
                    (segments[6] >> 8) as u8,
                    (segments[6] & 0xff) as u8,
                    (segments[7] >> 8) as u8,
                    (segments[7] & 0xff) as u8,
                )));
            }
            !NON_PUBLIC_DOWNLOAD_PREFIXES_V6
                .iter()
                .any(|(base, bits)| v6_in_prefix(v6, *base, *bits))
        }
    }
}

/// The download DNS resolver: every resolved address must pass
/// [`is_public_download_address`] before any of them is returned, so the
/// addresses the connector dials are exactly the ones validated — the same
/// resolve-check-dial sequence Go's publicDownloadDialer performs inside
/// DialContext. Port 0 is returned; reqwest substitutes the scheme default.
#[derive(Clone, Copy, Default)]
struct PublicDownloadResolver;

impl reqwest::dns::Resolve for PublicDownloadResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<std::net::SocketAddr> =
                match tokio::net::lookup_host((host.as_str(), 0u16)).await {
                    Ok(addrs) => addrs.collect(),
                    Err(_) => return Err("resolve download target failed".into()),
                };
            if addrs.is_empty() {
                return Err("resolve download target failed".into());
            }
            if addrs.iter().any(|a| !is_public_download_address(a.ip())) {
                return Err("blocked non-public download target".into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Rejects IP-literal hosts a connection would dial without consulting the
/// custom resolver (hyper-util connects to literals directly). Mirrors Go,
/// where lookup() runs netip.ParseAddr on literal targets through the same
/// public-address gate as hostnames.
fn reject_private_download_literal(parsed: &url::Url) -> anyhow::Result<()> {
    let Some(host) = parsed.host_str().map(str::to_ascii_lowercase) else {
        return Ok(());
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(addr) = host.parse::<IpAddr>() {
        if !is_public_download_address(addr) {
            anyhow::bail!("blocked non-public download target");
        }
    }
    Ok(())
}

fn new_download_http_client() -> anyhow::Result<reqwest::Client> {
    // No proxy: honouring HTTP_PROXY would send the fetch to an address the
    // URL validation never saw (Go disables the proxy too).
    Ok(reqwest::Client::builder()
        .no_proxy()
        .dns_resolver(Arc::new(PublicDownloadResolver))
        .build()?)
}

fn log_warn(msg: &InboundMessage, err: &anyhow::Error) {
    tracing::warn!(
        message_id = %msg.message_id,
        error = %err,
        "dingtalk media resolve skipped"
    );
}

/// Downloads one URL to bytes + sniffed type, following redirects manually so
/// each destination re-runs the full validation — the manual loop replaces
/// Go's CheckRedirect hook while keeping its guarantees: Referer never crosses
/// a hop, HTTPS never downgrades, and a plain-http redirect may not leave its
/// origin.
async fn fetch_bytes(http: &reqwest::Client, raw_url: &str) -> anyhow::Result<(Vec<u8>, String)> {
    let mut current =
        url::Url::parse(raw_url).map_err(|_| anyhow::anyhow!("invalid image download URL"))?;
    validate_download_url(&current)?;
    reject_private_download_literal(&current)?;
    let mut redirects = 0usize;
    loop {
        // The signed query string is a short-lived bearer credential; strip it
        // from any error that escapes so it is never logged or persisted (Go
        // unwraps *url.Error for the same reason).
        let mut resp = http
            .get(current.clone())
            .timeout(IMAGE_FETCH_TIMEOUT)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("download image request failed"))?;
        if resp.status().is_redirection() {
            if redirects >= MAX_DOWNLOAD_REDIRECTS {
                anyhow::bail!("too many redirects");
            }
            redirects += 1;
            let previous = current.clone();
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| anyhow::anyhow!("redirect without location"))?;
            let next = previous
                .join(location)
                .map_err(|_| anyhow::anyhow!("invalid image download URL"))?;
            validate_download_url(&next)?;
            reject_private_download_literal(&next)?;
            if previous.scheme().eq_ignore_ascii_case("https")
                && !next.scheme().eq_ignore_ascii_case("https")
            {
                anyhow::bail!("disallowed HTTPS download redirect downgrade");
            }
            if next.scheme().eq_ignore_ascii_case("http") && !same_download_origin(&previous, &next)
            {
                anyhow::bail!("disallowed cross-origin HTTP download redirect");
            }
            current = next;
            continue;
        }
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("download image: http {}", status.as_u16());
        }
        let capacity = resp
            .content_length()
            .unwrap_or_default()
            .min(MAX_INBOUND_IMAGE_BYTES as u64) as usize;
        let mut data = Vec::with_capacity(capacity);
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("read image response failed"))?
        {
            append_bounded(&mut data, &chunk, MAX_INBOUND_IMAGE_BYTES)?;
        }
        // Sniff the real type off the first 512 bytes rather than trusting the
        // response header, then admit only known image types.
        let sniff_len = data.len().min(512);
        let mut sniffed = detect_content_type(&data[..sniff_len]);
        if let Some(semi) = sniffed.find(';') {
            sniffed = sniffed[..semi].trim().to_string();
        }
        return Ok((data, sniffed));
    }
}

fn append_bounded(data: &mut Vec<u8>, chunk: &[u8], limit: usize) -> anyhow::Result<()> {
    if chunk.len() > limit.saturating_sub(data.len()) {
        anyhow::bail!("image exceeds the {} MB limit", limit >> 20);
    }
    data.extend_from_slice(chunk);
    Ok(())
}

/// Minimal DetectContentType equivalent covering the signatures DingTalk
/// images actually carry (PNG / JPEG / GIF / WEBP / BMP); everything else
/// falls back like Go's algorithm so the caller's allow-list refuses it.
pub(crate) fn detect_content_type(data: &[u8]) -> String {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return "image/png".to_string();
    }
    if data.starts_with(b"\xff\xd8\xff") {
        return "image/jpeg".to_string();
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return "image/gif".to_string();
    }
    if data.len() >= 12 && &data[8..12] == b"WEBP" {
        return "image/webp".to_string();
    }
    if data.len() >= 2 && data[0] == b'B' && data[1] == b'M' {
        return "image/bmp".to_string();
    }
    const HTML_SIGS: [&[u8]; 4] = [b"<!DOCTYPE HTML", b"<HTML", b"<HEAD", b"<SCRIPT"];
    let upper: Vec<u8> = data
        .iter()
        .take(256)
        .map(|b| b.to_ascii_uppercase())
        .collect();
    for sig in HTML_SIGS {
        let hit = upper
            .iter()
            .position(|&b| b == b'<')
            .map(|start| upper[start..].starts_with(sig))
            .unwrap_or(false);
        if hit {
            return "text/html; charset=utf-8".to_string();
        }
    }
    // Go falls through to a textual scan; treat printable bytes as text/plain.
    let printable = data.is_empty()
        || data.iter().take(512).all(|&b| {
            b == 0x09
                || b == 0x0a
                || b == 0x0c
                || b == 0x0d
                || (0x20..=0x7e).contains(&b)
                || b >= 0x80
        });
    if printable {
        "text/plain; charset=utf-8".to_string()
    } else {
        "application/octet-stream".to_string()
    }
}

/// Derives the storage key from the CHAT message the object will be attached
/// to, plus the resource ref and position — hashed like Go's
/// dingtalkMediaObjectKey so two resources never collide.
pub fn dingtalk_media_object_key(
    inst: &ResolvedInstallation,
    chat_message_id: Uuid,
    reference: &str,
    index: usize,
) -> String {
    let sum = Sha256::digest(format!("{chat_message_id}\u{0}{reference}\u{0}{index}").as_bytes());
    [
        "workspaces",
        &inst.workspace_id.to_string(),
        "dingtalk",
        &inst.id.to_string(),
        &hex::encode(sum),
    ]
    .join("/")
}

#[async_trait]
impl MediaResolver for MediaResolverImpl {
    /// A pure decode of the already-received callback metadata. Remote work
    /// stays behind resolve_media, after dedup, identity and membership checks.
    fn has_media(&self, msg: &InboundMessage) -> bool {
        decode_dingtalk_raw(msg)
            .map(|raw| !raw.media.is_empty())
            .unwrap_or(false)
    }

    /// Downloads and stores every image on the message, returning it with a
    /// MediaRef per object that landed. Images are independent: one that fails
    /// does not stop the rest; concurrency is bounded by
    /// [`MEDIA_FETCH_CONCURRENCY`] and refs keep send order regardless of
    /// completion order.
    async fn resolve_media(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        _sender: &ResolvedIdentity,
        _session_id: Uuid,
        chat_message_id: Uuid,
        mut msg: InboundMessage,
    ) -> InboundMessage {
        let raw = match decode_dingtalk_raw(&msg) {
            Ok(raw) => raw,
            Err(_) => return msg,
        };
        if raw.media.is_empty() {
            return msg;
        }
        if raw.media.len() > MAX_IMAGES_PER_MESSAGE {
            log_warn(
                &msg,
                &anyhow::anyhow!(
                    "{} images exceed the limit of {}",
                    raw.media.len(),
                    MAX_IMAGES_PER_MESSAGE
                ),
            );
            return msg;
        }
        let row = match crate::db_row_from_platform(inst) {
            Ok(row) => row,
            Err(err) => {
                log_warn(&msg, &anyhow::anyhow!("{err:#}"));
                return msg;
            }
        };
        let creds = match decode_credentials(&row.config, self.decrypt.as_deref()) {
            Ok(c) => c,
            Err(e) => {
                log_warn(&msg, &anyhow::anyhow!("decode credentials: {e:#}"));
                return msg;
            }
        };

        // The shared download client ignores proxy settings and resolves DNS
        // only through the public-address gate (Go: transport.Proxy = nil +
        // publicDownloadDialer).
        let http = new_download_http_client().ok();

        // Scope every image future to this resolver call. `buffer_unordered`
        // provides the same bounded concurrency as Go's errgroup, but does
        // not detach work: dropping this resolver at the Router deadline
        // drops every in-flight download/upload future as well.
        let mut results: Vec<(usize, Option<MediaRef>)> =
            futures_util::stream::iter(raw.media.into_iter().enumerate())
                .map(|(index, resource)| {
                    let ctx = ctx.clone();
                    let client = self.client.clone();
                    let storage = self.storage.clone();
                    let ledger = self.ledger.clone();
                    let creds = creds.clone();
                    let http = http.clone();
                    let reference = resource.reference;
                    let alt = resource.alt;
                    let inline_index = resource.inline_index;
                    let inst = inst.clone();
                    async move {
                        let resolved = ingest_one(
                            ctx,
                            client.as_ref(),
                            http.as_ref(),
                            storage.as_ref(),
                            ledger.as_ref(),
                            &inst,
                            chat_message_id,
                            index,
                            inline_index,
                            &creds,
                            &reference,
                            &alt,
                        )
                        .await;
                        (index, resolved)
                    }
                })
                .buffer_unordered(MEDIA_FETCH_CONCURRENCY)
                .collect()
                .await;
        // Preserve send order regardless of completion order.
        results.sort_unstable_by_key(|(index, _)| *index);
        for (i, r) in results {
            match r {
                Some(r) => msg.media_refs.push(r),
                None => log_warn(&msg, &anyhow::anyhow!("image {i} did not resolve")),
            }
        }
        msg
    }
}

/// Carries one resource from downloadCode to a stored object + MediaRef. No
/// object upload starts until the ledger row is durable; from that point on an
/// upload failure or crash leaves an intent the reconciler settles, and
/// nothing here deletes anything.
#[allow(clippy::too_many_arguments)]
async fn ingest_one(
    ctx: CancellationToken,
    client: &Client,
    http: Option<&reqwest::Client>,
    storage: &dyn MediaStorage,
    ledger: &dyn MediaIntentLedger,
    inst: &ResolvedInstallation,
    chat_message_id: Uuid,
    index: usize,
    inline_index: usize,
    credentials: &Credentials,
    reference: &str,
    alt: &str,
) -> Option<MediaRef> {
    let http = http?;
    if ctx.is_cancelled() {
        return None;
    }

    // Resolve primary → fallback code like Go's fetchResource.
    let fetched = tokio::select! {
        biased;
        _ = ctx.cancelled() => return None,
        fetched = fetch_resource(
            client,
            http,
            &credentials.app_key,
            &credentials.app_secret,
            &credentials.robot_code,
            reference,
            alt,
        ) => fetched.ok()?,
    };
    let (data, content_type) = fetched;
    let ext = allowed_image_ext(&content_type)?;
    let filename = format!("dingtalk-image-{}{ext}", index + 1);

    let key = dingtalk_media_object_key(inst, chat_message_id, reference, index);
    let link = storage.object_url(&key);
    // No durable intent, no upload — the fail-safe direction. A false return
    // means the reconciler owns this key; never resurrect it.
    let owned = tokio::select! {
        biased;
        _ = ctx.cancelled() => return None,
        owned = ledger.record_pending_media_object(RecordPendingMediaObjectParams {
            storage_key: key.clone(),
            workspace_id: inst.workspace_id,
            chat_message_id,
            storage_url: link.clone(),
            installation_id: inst.id,
        }) => owned.ok()?,
    };
    if !owned {
        tracing::warn!(storage_key = %key, "media key owned by reconciler");
        return None;
    }

    // The store may still be processing the PUT; the intent row covers the
    // object either way.
    let size_bytes = data.len() as i64;
    tokio::select! {
        biased;
        _ = ctx.cancelled() => return None,
        uploaded = storage.upload(&key, data, &content_type, &filename) => {
            uploaded.ok()?;
        }
    }
    Some(MediaRef {
        r#type: cordy_channel::MsgType::image(),
        storage_key: key,
        storage_url: link,
        filename,
        mime_type: content_type,
        size_bytes,
        inline_placeholder: crate::inbound::DINGTALK_IMAGE_PLACEHOLDER.to_string(),
        inline_index,
    })
}

/// Fetches one resource by its primary downloadCode, falling back to the
/// secondary code when present and different (Go's fetchResource). A 401
/// refreshes the cached token once, mirroring the sender path.
async fn fetch_resource(
    client: &Client,
    http: &reqwest::Client,
    app_key: &str,
    app_secret: &str,
    robot_code: &str,
    reference: &str,
    alt: &str,
) -> anyhow::Result<(Vec<u8>, String)> {
    let primary = fetch_by_code(client, http, app_key, app_secret, robot_code, reference).await;
    let Err(primary_err) = primary else {
        return primary;
    };
    if alt.is_empty() || alt == reference {
        return Err(primary_err);
    }
    fetch_by_code(client, http, app_key, app_secret, robot_code, alt)
        .await
        .map_err(|fallback_err| {
            anyhow::anyhow!(
                "primary media reference: {primary_err:#}; fallback media reference: {fallback_err:#}"
            )
        })
}

async fn fetch_by_code(
    client: &Client,
    http: &reqwest::Client,
    app_key: &str,
    app_secret: &str,
    robot_code: &str,
    code: &str,
) -> anyhow::Result<(Vec<u8>, String)> {
    let mut retried = false;
    loop {
        let url = client
            .message_file_download_url(app_key, app_secret, robot_code, code)
            .await
            .map_err(|e| anyhow::anyhow!("resolve download url: {e:#}"))?;
        match fetch_bytes(http, &url).await {
            Ok(out) => return Ok(out),
            Err(err) if !retried && is_unauthorized(&err) => {
                retried = true;
                client.invalidate(app_key);
            }
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_download_buffer_rejects_chunk_before_exceeding_limit() {
        let mut data = vec![1, 2, 3];
        append_bounded(&mut data, &[4, 5], 5).unwrap();
        assert_eq!(data, vec![1, 2, 3, 4, 5]);

        let err = append_bounded(&mut data, &[6], 5).unwrap_err();
        assert!(err.to_string().contains("image exceeds"));
        assert_eq!(data, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn address_matrix_matches_go_blocklist() {
        let cases: &[(&str, bool)] = &[
            // Public.
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
            ("::ffff:8.8.8.8", true),   // mapped public v4
            ("64:ff9b::8.8.8.8", true), // NAT64 embedding a public target
            // Private / loopback / multicast / link-local / broadcast.
            ("10.1.2.3", false),
            ("172.16.0.1", false),
            ("172.31.255.255", false),
            ("192.168.1.1", false),
            ("127.0.0.1", false),
            ("169.254.169.254", false),
            ("0.0.0.0", false),
            ("0.1.2.3", false),
            ("224.0.0.1", false),
            ("239.255.255.250", false),
            ("240.0.0.1", false),
            ("255.255.255.255", false),
            ("::", false),
            ("::1", false),
            ("fe80::1", false),
            ("febf:ffff::1", false),
            ("fd00::1", false),
            ("ff02::1", false),
            ("::ffff:10.0.0.1", false),    // mapped private v4
            ("64:ff9b::127.0.0.1", false), // NAT64 smuggling loopback
            // Special-purpose ranges std's helpers miss (Go prefix list).
            ("100.64.0.0", false),
            ("100.127.255.255", false),
            ("192.0.0.1", false),
            ("192.0.2.1", false),
            ("198.18.0.1", false),
            ("198.19.255.255", false),
            ("198.51.100.1", false),
            ("203.0.113.1", false),
            ("::1.2.3.4", false),    // ::/96 IPv4-compatible
            ("64:ff9b:1::1", false), // local-use NAT64
            ("64:ff9b:1::8.8.8.8", false),
            ("100::1", false),      // discard-only
            ("2001::1", false),     // Teredo
            ("2001:2::1", false),   // benchmarking
            ("2001:db8::1", false), // documentation
            ("2001:10::1", false),  // deprecated ORCHID
            ("2001:20::1", false),  // ORCHIDv2
            ("2002::1", false),     // 6to4
            ("2002:0800::1", false),
            ("3fff::1", false), // documentation v6
        ];
        for (raw, want_public) in cases {
            let addr: IpAddr = raw.parse().unwrap();
            assert_eq!(is_public_download_address(addr), *want_public, "addr {raw}");
        }
    }

    #[test]
    fn literal_private_hosts_are_rejected_at_url_level() {
        for raw in [
            "https://169.254.169.254/latest/meta-data",
            "http://127.0.0.1/x",
            "http://[::1]/x",
            "http://[fd00::1]/x",
            "http://[64:ff9b::127.0.0.1]/x",
            "http://192.168.1.10/x",
        ] {
            let parsed = url::Url::parse(raw).unwrap();
            assert!(
                reject_private_download_literal(&parsed).is_err(),
                "{raw} was accepted"
            );
        }
        for raw in [
            "https://download.dingtalk.com/media.png?sig=abc",
            "http://media.example.com/x",
            "http://[2606:4700::1111]/x",
            "http://[64:ff9b::8.8.8.8]/x",
            "https://8.8.8.8/y",
        ] {
            let parsed = url::Url::parse(raw).unwrap();
            assert!(
                reject_private_download_literal(&parsed).is_ok(),
                "{raw} was rejected"
            );
        }
    }
}
