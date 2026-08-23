//! DingTalk media resolution — port of
//! `server/internal/integrations/dingtalk/media.go`.
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
//! never crosses a redirect, and downloads are capped in bytes.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use cordy_channel::{InboundMessage, MediaRef};
use cordy_channel_engine::resolvers::{
    MediaIntentLedger, MediaResolver, RecordPendingMediaObjectParams, ResolvedIdentity,
    ResolvedInstallation,
};

use crate::client::{is_unauthorized, Client};
use crate::config::{decode_credentials, Decrypter};
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
    let mut redirects = 0usize;
    loop {
        // The signed query string is a short-lived bearer credential; strip it
        // from any error that escapes so it is never logged or persisted (Go
        // unwraps *url.Error for the same reason).
        let resp = http
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
        let data = resp
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("read image: {e}"))?;
        if data.len() > MAX_INBOUND_IMAGE_BYTES {
            anyhow::bail!(
                "image exceeds the {} MB limit",
                MAX_INBOUND_IMAGE_BYTES >> 20
            );
        }
        // Sniff the real type off the first 512 bytes rather than trusting the
        // response header, then admit only known image types.
        let sniff_len = data.len().min(512);
        let mut sniffed = detect_content_type(&data[..sniff_len]);
        if let Some(semi) = sniffed.find(';') {
            sniffed = sniffed[..semi].trim().to_string();
        }
        return Ok((data.to_vec(), sniffed));
    }
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

        // The shared download client ignores proxy settings: honouring
        // HTTP_PROXY would send the fetch to an address the URL validation
        // never saw (Go disables the proxy too).
        let http = reqwest::Client::builder().no_proxy().build().ok();

        // Bounded-concurrency fan-out over the resources. Each slot resolves,
        // records the intent, downloads, uploads, and reports its ref; a
        // failed slot logs and yields None without stopping the others.
        let sem = Arc::new(tokio::sync::Semaphore::new(MEDIA_FETCH_CONCURRENCY));
        let mut handles = Vec::with_capacity(raw.media.len());
        for (index, resource) in raw.media.iter().enumerate() {
            let sem = sem.clone();
            let ctx = ctx.clone();
            let client = self.client.clone();
            let storage = self.storage.clone();
            let ledger = self.ledger.clone();
            let creds = creds.clone();
            let http = http.clone();
            let reference = resource.reference.clone();
            let alt = resource.alt.clone();
            let inline_index = resource.inline_index;
            let inst = inst.clone();
            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.ok()?;
                ingest_one(
                    ctx,
                    client.as_ref(),
                    http.as_ref(),
                    storage.as_ref(),
                    ledger.as_ref(),
                    &inst,
                    chat_message_id,
                    index,
                    inline_index,
                    app_key_secret_robot(&creds.app_key, &creds.app_secret, &creds.robot_code)
                        .as_slice(),
                    &reference,
                    &alt,
                )
                .await
            }));
        }

        let results: Vec<Option<MediaRef>> = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .map(|joined| match joined {
                Ok(inner) => inner,
                Err(e) => {
                    log_warn(&msg, &anyhow::anyhow!("media task failed: {e}"));
                    None
                }
            })
            .collect();
        // Preserve send order regardless of completion order.
        for (i, r) in results.into_iter().enumerate() {
            match r {
                Some(r) => msg.media_refs.push(r),
                None => log_warn(&msg, &anyhow::anyhow!("image {i} did not resolve")),
            }
        }
        msg
    }
}

/// Packs the three credential strings for the spawned task boundary without a
/// dedicated struct (order: app_key, app_secret, robot_code).
fn app_key_secret_robot(app_key: &str, app_secret: &str, robot_code: &str) -> Vec<String> {
    vec![
        app_key.to_string(),
        app_secret.to_string(),
        robot_code.to_string(),
    ]
}

/// Carries one resource from downloadCode to a stored object + MediaRef. The
/// ledger row goes first: from that point on every failure — download,
/// upload, a crash — leaves an intent the reconciler settles, and nothing
/// here deletes anything.
#[allow(clippy::too_many_arguments)]
async fn ingest_one(
    _ctx: CancellationToken,
    client: &Client,
    http: Option<&reqwest::Client>,
    storage: &dyn MediaStorage,
    ledger: &dyn MediaIntentLedger,
    inst: &ResolvedInstallation,
    chat_message_id: Uuid,
    index: usize,
    inline_index: usize,
    cred_triple: &[String],
    reference: &str,
    alt: &str,
) -> Option<MediaRef> {
    let http = http?;
    let [app_key, app_secret, robot_code] = cred_triple else {
        return None;
    };

    // Resolve primary → fallback code like Go's fetchResource.
    let fetched = fetch_resource(
        client, http, app_key, app_secret, robot_code, reference, alt,
    )
    .await
    .ok()?;
    let (data, content_type) = fetched;
    let ext = allowed_image_ext(&content_type)?;
    let filename = format!("dingtalk-image-{}{ext}", index + 1);

    let key = dingtalk_media_object_key(inst, chat_message_id, reference, index);
    let link = storage.object_url(&key);
    // No durable intent, no upload — the fail-safe direction. A false return
    // means the reconciler owns this key; never resurrect it.
    let owned = ledger
        .record_pending_media_object(RecordPendingMediaObjectParams {
            storage_key: key.clone(),
            workspace_id: inst.workspace_id,
            chat_message_id,
            storage_url: link.clone(),
            installation_id: inst.id,
        })
        .await
        .ok()?;
    if !owned {
        tracing::warn!(storage_key = %key, "media key owned by reconciler");
        return None;
    }

    // The store may still be processing the PUT; the intent row covers the
    // object either way.
    storage
        .upload(&key, data.clone(), &content_type, &filename)
        .await
        .ok()?;
    Some(MediaRef {
        r#type: cordy_channel::MsgType::image(),
        storage_key: key,
        storage_url: link,
        filename,
        mime_type: content_type,
        size_bytes: data.len() as i64,
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
