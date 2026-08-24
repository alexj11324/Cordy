//! The engine MediaResolver for WeCom — port of `media_ingest.go`.
//!
//! The shape is the one the Router depends on: [`has_media`](Self::has_media)
//! is a pure in-memory look at the payload we already have,
//! [`resolve_media`](Self::resolve_media) runs detached from the connector ACK
//! path, every upload is covered by an intent-ledger row written BEFORE the
//! PUT, and nothing is ever deleted inline — a failure anywhere leaves the row
//! for the reconciler and leaves the message's placeholder text intact.
//!
//! What is wecom-specific is the middle: WeCom hands over a pre-signed COS url
//! and a per-url key instead of a resource id to be fetched with a tenant
//! token, so there is no API client and no credential here — just an HTTP GET
//! and a decrypt (media_download.rs, media_crypt.rs). The callback body says
//! nothing about the file besides those two strings, so the name comes out of
//! the download's Content-Disposition and the type is worked out from the name
//! or sniffed from the decrypted bytes.

use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use cordy_channel::message::{InboundMessage as ChannelInboundMessage, MediaRef, MsgType};
use cordy_channel_engine::resolvers::{
    MediaIntentLedger, MediaResolver, RecordPendingMediaObjectParams, ResolvedIdentity,
    ResolvedInstallation,
};

use crate::media_download::{download_media, open_media_parts};
use crate::media_guard::{new_media_http_client, validate_media_url, MediaAddrBlocked, MediaGuard};
use crate::media_stream::{decrypt_to_file, peek_file};
use crate::senders_registry::SendersRegistry;
use crate::trace::trace_media_headers;
use crate::ws_frame::{wecom_msg_from_raw, InboundMedia, WecomInboundMessage};

/// Failure notice lines. WeCom deployments are China-only, so these follow the
/// Chinese product voice the rest of this adapter already writes in.
pub const MEDIA_UNREADABLE_NOTICE: &str = "抱歉，有附件没能收到，麻烦重新发一次。";
pub const MEDIA_TOO_LARGE_NOTICE: &str = "抱歉，附件太大了，我这边收不下。";

/// The slice of storage.Storage this resolver drives. ObjectURL is a pure
/// function of configuration, which is what lets the intent ledger persist
/// the object's URL before the object exists.
#[async_trait]
pub trait MediaStorage: Send + Sync {
    async fn upload(
        &self,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> anyhow::Result<String>;
    fn object_url(&self, key: &str) -> String;

    /// The streaming half, which both production backends implement. A
    /// backend without it falls back to the buffered path — correct, just not
    /// memory-flat.
    ///
    /// Port note: Go uses a second interface plus a type assertion; Rust
    /// models the same capability probe as an overridable accessor.
    fn as_stream_storage(&self) -> Option<&dyn MediaStreamStorage> {
        None
    }
}

/// The memory-flat upload seam: ciphertext streams into a decrypt, the
/// plaintext lands in an unlinked temp file, and the upload reads from there.
#[async_trait]
pub trait MediaStreamStorage: Send + Sync {
    async fn upload_stream(
        &self,
        ctx: CancellationToken,
        key: &str,
        data: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
        size_bytes: i64,
        content_type: &str,
        filename: &str,
    ) -> anyhow::Result<String>;
}

/// The WeCom media resolver.
pub struct WecomMediaResolver {
    storage: Arc<dyn MediaStorage>,
    ledger: Arc<dyn MediaIntentLedger>,
    /// Never a bare client: the URL being fetched came off the wire, and this
    /// client refuses to connect to anything that is not public internet
    /// (media_guard.rs).
    http: reqwest::Client,
    /// Reaches the live aibot socket for the installation, to tell the sender
    /// an attachment did not make it. None disables the notice and leaves only
    /// the log.
    notify: Option<Arc<SendersRegistry>>,
}

impl WecomMediaResolver {
    /// Builds the wecom MediaResolver. storage and ledger are required —
    /// without either there is nothing durable to point an attachment at, and
    /// the resolver degrades to leaving the placeholder in place. senders is
    /// optional: without it a failed attachment is only logged.
    pub fn new(
        storage: Arc<dyn MediaStorage>,
        ledger: Arc<dyn MediaIntentLedger>,
        senders: Option<Arc<SendersRegistry>>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            storage,
            ledger,
            http: new_media_http_client(MediaGuard::new())?,
            notify: senders,
        })
    }

    /// Carries a single attachment from url to MediaRef. The ledger row goes
    /// first: from that point on every failure — download, decrypt, upload, a
    /// crash — leaves an intent the reconciler settles, and nothing here
    /// deletes anything.
    async fn ingest_one(
        &self,
        ctx: &CancellationToken,
        inst: &ResolvedInstallation,
        chat_message_id: Uuid,
        wm: &WecomInboundMessage,
        index: usize,
        m: &InboundMedia,
    ) -> anyhow::Result<MediaRef> {
        let kind_str = m.kind.clone();
        let key = media_object_key(inst, chat_message_id, &wm.msg_id, index, &kind_str);
        let link = self.storage.object_url(&key);

        let ok = self
            .ledger
            .record_pending_media_object(RecordPendingMediaObjectParams {
                storage_key: key.clone(),
                workspace_id: inst.workspace_id,
                chat_message_id,
                storage_url: link.clone(),
                installation_id: inst.id,
            })
            .await
            .map_err(|e| anyhow::anyhow!("record media intent: {e}"))?;
        if !ok {
            // The reconciler owns this key; never resurrect it.
            anyhow::bail!("media key {key} is owned by the reconciler");
        }

        if let Some(streamer) = self.storage.as_stream_storage() {
            // The memory-flat path: ciphertext streams from the socket into
            // the decrypt, the plaintext lands in an unlinked temp file, and
            // the upload reads from there. Nothing holds a whole attachment.
            match self
                .ingest_streaming(ctx, streamer, wm, index, m, &key, &link)
                .await
            {
                Ok(ref_) => return Ok(ref_),
                Err(e) if !is_streaming_unavailable(&e) => return Err(e),
                // Fall through: the backend said it could stream and then
                // could not.
                Err(_) => {}
            }
        }

        validate_media_url(&m.url)?;
        let got = download_media(&self.http, &m.url).await?;
        trace_media_headers(
            &wm.msg_id,
            index,
            &got.headers.disposition,
            &got.headers.filename,
        );
        let plain = crate::media_crypt::decrypt_media(&m.aes_key, &got.body)?;

        let (filename, content_type) = describe_media(wm, index, m, &got.headers.filename, &plain);
        self.storage
            .upload(&key, plain.clone(), &content_type, &filename)
            .await
            .map_err(|e| anyhow::anyhow!("upload media: {e}"))?;
        Ok(MediaRef {
            r#type: cordy_channel::MsgType(m.kind.clone()),
            storage_key: key,
            storage_url: link,
            filename,
            mime_type: content_type,
            size_bytes: plain.len() as i64,
            ..Default::default()
        })
    }

    /// Carries one attachment without ever holding it whole: ciphertext
    /// streams from the response body into the decrypt, the plaintext lands in
    /// an unlinked temp file, and the upload reads from that file with the
    /// exact length it now knows.
    #[allow(clippy::too_many_arguments)]
    async fn ingest_streaming(
        &self,
        ctx: &CancellationToken,
        streamer: &dyn MediaStreamStorage,
        wm: &WecomInboundMessage,
        index: usize,
        m: &InboundMedia,
        key: &str,
        link: &str,
    ) -> anyhow::Result<MediaRef> {
        validate_media_url(&m.url)?;
        let (mut body, headers) = open_media_parts(&self.http, &m.url).await?;
        trace_media_headers(&wm.msg_id, index, &headers.disposition, &headers.filename);

        let (file, size) = decrypt_to_file(&m.aes_key, &mut body).await.map_err(|e| {
            // A decrypt failure is the attachment's problem and must be
            // reported as one; only a failure to make the temp file is grounds
            // to fall back, and decrypt_to_file says so in its message.
            if e.to_string().contains("media temp file") {
                anyhow::Error::new(StreamingUnavailable)
                    .context(format!("wecom: streaming media ingest unavailable: {e:#}"))
            } else {
                e
            }
        })?;

        // The type is sniffed from the head of the file rather than from the
        // whole thing — the sniffer only ever reads 512 bytes.
        let mut file = file;
        let head = peek_file(&mut file, 512)?;
        let (filename, content_type) = describe_media(wm, index, m, &headers.filename, &head);

        let file = tokio::fs::File::from_std(file);
        streamer
            .upload_stream(
                ctx.clone(),
                key,
                Box::new(file),
                size,
                &content_type,
                &filename,
            )
            .await
            .map_err(|e| anyhow::anyhow!("upload media: {e}"))?;
        Ok(MediaRef {
            r#type: cordy_channel::MsgType(m.kind.clone()),
            storage_key: key.to_string(),
            storage_url: link.to_string(),
            filename,
            mime_type: content_type,
            size_bytes: size,
            ..Default::default()
        })
    }

    /// Writes one short notice into the chat the attachments came from, over
    /// the same live aibot socket every other wecom message uses.
    ///
    /// Without it the failure is invisible in the worst way: the stored body
    /// still says [Image], so the agent answers as if it had been shown a
    /// picture it never received. The agent run itself is deliberately left to
    /// proceed — the person can see for themselves that the picture did not
    /// get through, and the answer to whatever they typed beside it is still
    /// worth having.
    async fn tell_the_sender(
        &self,
        inst: &ResolvedInstallation,
        wm: &WecomInboundMessage,
        failures: &[MediaFailure],
    ) {
        if failures.is_empty() {
            return;
        }
        let Some(notify) = &self.notify else {
            return;
        };
        let mut chat_id = wm.chat_id.clone();
        if chat_id.is_empty() {
            chat_id = wm.sender_user_id.clone();
        }
        if chat_id.is_empty() {
            return;
        }
        let Some(sender) = notify.get(inst.id) else {
            // Mid-reconnect: the socket this installation writes over does not
            // exist right now. The log below is the whole record.
            tracing::warn!(
                installation_id = %inst.id,
                msg_id = %wm.msg_id,
                "wecom media failure notice not delivered: no live connection"
            );
            return;
        };
        let chat_type = if wm.chat_type.eq_ignore_ascii_case("group") {
            crate::ws_frame::CHAT_TYPE_GROUP_INT
        } else {
            crate::ws_frame::CHAT_TYPE_SINGLE_INT
        };
        let mut lines: Vec<&str> = Vec::with_capacity(failures.len());
        for f in failures {
            let notice = if *f == MediaFailure::TooLarge {
                MEDIA_TOO_LARGE_NOTICE
            } else {
                MEDIA_UNREADABLE_NOTICE
            };
            // Blocked lands on the unreadable wording deliberately. From the
            // sender's side a refused address and a download that fell over
            // are the same event — the attachment did not arrive — and the
            // thing that separates them is what the operator must change,
            // which belongs in the log. Neither the resolved address nor the
            // signed url goes into a chat message.
            //
            // Two kinds sharing one wording is why the dedupe lives here: a
            // message with one blocked and one unreadable attachment is still
            // one piece of news.
            if !lines.contains(&notice) {
                lines.push(notice);
            }
        }
        if let Err(e) = sender
            .send_text(&chat_id, chat_type, &lines.join("\n"))
            .await
        {
            tracing::warn!(
                installation_id = %inst.id,
                msg_id = %wm.msg_id,
                err = %e,
                "wecom media failure notice not delivered"
            );
        }
    }
}

#[async_trait]
impl MediaResolver for WecomMediaResolver {
    /// Reports whether this callback carried anything to download. It runs
    /// synchronously on the connector ACK path and decides whether the message
    /// pays for a media deadline, a deferred run and a semaphore slot at all,
    /// so it stays a decode of bytes already in hand.
    fn has_media(&self, msg: &ChannelInboundMessage) -> bool {
        wecom_msg_from_raw(msg)
            .map(|wm| !wm.media.is_empty())
            .unwrap_or(false)
    }

    /// Downloads, decrypts and stores every attachment on the message,
    /// returning it with a MediaRef per object that landed. Attachments are
    /// independent: one that fails does not stop the rest, and the sender is
    /// told once at the end about whatever did not arrive.
    async fn resolve_media(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        _sender: &ResolvedIdentity,
        _session_id: Uuid,
        chat_message_id: Uuid,
        mut msg: ChannelInboundMessage,
    ) -> ChannelInboundMessage {
        let wm = match wecom_msg_from_raw(&msg) {
            Ok(wm) => wm,
            Err(e) => {
                tracing::warn!(
                    message_id = %msg.message_id,
                    err = %e,
                    "wecom media ingest skipped: raw decode failed"
                );
                return msg;
            }
        };
        if wm.media.is_empty() {
            return msg;
        }

        let mut failures: Vec<MediaFailure> = Vec::new();
        for (i, m) in wm.media.iter().enumerate() {
            let ingest = self.ingest_one(&ctx, inst, chat_message_id, &wm, i, m);
            let result = tokio::select! {
                _ = ctx.cancelled() => break,
                result = ingest => result,
            };
            match result {
                Ok(r#ref) => msg.media_refs.push(r#ref),
                Err(e) => {
                    let failure = classify_media_failure(&e);
                    if !failures.contains(&failure) {
                        failures.push(failure);
                    }
                    // A refused address gets its own line because the
                    // operator's next move is different from every other
                    // failure here: the attachment is fine and the deployment
                    // declined to dial where its host resolved, so the fix is
                    // configuration, not a retry. Saying only "ingest failed"
                    // for this sends them looking at WeCom.
                    //
                    // The url and the key never reach the log: one is a signed
                    // address anyone could then fetch, the other unlocks it.
                    // strip_url has already taken the url out of the transport
                    // errors, and the guard's refusal carries no address of
                    // its own, so the error is safe to log whole.
                    if failure == MediaFailure::Blocked {
                        tracing::warn!(
                            installation_id = %inst.id,
                            msg_id = %wm.msg_id,
                            attachment = i,
                            kind = %m.kind,
                            err = %e,
                            "wecom media ingest refused by the media address guard: the host resolved to a non-public address and was not dialed. If this deployment sits behind a fake-IP proxy, declare its range in CORDY_WECOM_MEDIA_ALLOW_CIDRS; otherwise this is a URL that should not have been sent."
                        );
                    } else {
                        tracing::warn!(
                            installation_id = %inst.id,
                            msg_id = %wm.msg_id,
                            attachment = i,
                            kind = %m.kind,
                            err = %e,
                            "wecom media ingest failed"
                        );
                    }
                }
            }
        }

        self.tell_the_sender(inst, &wm, &failures).await;
        msg
    }
}

/// Names the object. It is derived from the CHAT message rather than the WeCom
/// message for the reason lark documents: one platform message can be ingested
/// twice (the inbound dedup claim is reclaimable once stale), and a shared key
/// would run the second ingest into the first one's ledger row — possibly a
/// tombstone the intent upsert refuses, silently dropping the media. The
/// attachment's position in the message is part of the key too, since a 图文混排
/// can carry several and the url is not stable enough to key on.
pub fn media_object_key(
    inst: &ResolvedInstallation,
    chat_message_id: Uuid,
    msg_id: &str,
    index: usize,
    kind: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(chat_message_id.to_string());
    hasher.update([0u8]);
    hasher.update(msg_id);
    hasher.update([0u8]);
    hasher.update(kind);
    hasher.update([0u8]);
    hasher.update(index.to_string());
    let sum = hasher.finalize();
    format!(
        "workspaces/{}/wecom/{}/{}",
        inst.workspace_id,
        inst.id,
        hex::encode(sum)
    )
}

/// Works out what to call the file and what to say it is. The callback body
/// carries neither, so the name comes from the download's Content-Disposition
/// and the type from the name's extension, falling back to sniffing the
/// decrypted bytes — the extension is the better signal for the formats that
/// are really zip containers (.docx, .xlsx), sniffing is the better one when
/// there is no name at all.
fn describe_media(
    wm: &WecomInboundMessage,
    index: usize,
    m: &InboundMedia,
    header_name: &str,
    plain: &[u8],
) -> (String, String) {
    let filename = crate::media_download::clean_media_filename(header_name);
    let mut content_type = filename
        .rsplit_once('.')
        .and_then(|(_, ext)| mime_type_by_extension(&ext.to_lowercase()))
        .unwrap_or_default();
    if content_type.is_empty() {
        content_type = detect_content_type(plain);
    }
    if content_type.is_empty() {
        content_type = "application/octet-stream".to_string();
    }
    let filename = if filename.is_empty() {
        fallback_media_name(
            &wm.msg_id,
            index,
            &cordy_channel::MsgType(m.kind.clone()),
            &content_type,
        )
    } else {
        filename
    };
    (filename, content_type)
}

/// Builds a name for an attachment the server did not name. It has to be
/// unique within the message, so the attachment's position is in it — two
/// photos in one 图文混排 otherwise land as one name twice.
fn fallback_media_name(msg_id: &str, index: usize, kind: &MsgType, content_type: &str) -> String {
    let prefix = if *kind == MsgType::image() {
        "wecom-image"
    } else if *kind == MsgType::video() {
        "wecom-video"
    } else {
        "wecom-file"
    };
    format!(
        "{prefix}-{}-{index}{}",
        safe_media_segment(msg_id),
        media_extension(content_type)
    )
}

/// Picks a file extension for a content type, preferring the familiar spelling
/// over whatever the mime database happens to list first (image/jpeg resolves
/// to ".jfif" on some systems).
pub(crate) fn media_extension(content_type: &str) -> String {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    match ct.as_str() {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "video/mp4" => ".mp4",
        "application/pdf" => ".pdf",
        _ => mime_extension_by_type(&ct).unwrap_or(""),
    }
    .to_string()
}

/// Reduces an id to characters that are safe in a filename.
fn safe_media_segment(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return "unknown".to_string();
    }
    let mut b = String::with_capacity(s.len());
    for r in s.chars() {
        match r {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => b.push(r),
            _ => b.push('_'),
        }
    }
    let out = b.trim_matches('_');
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out.to_string()
    }
}

/// The kind of bad news, not the error itself: the sender gets told what to do
/// differently, and "too big" and "did not arrive" have different answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaFailure {
    Unreadable,
    TooLarge,
    /// The guard refusing the address the media host resolved to. Separated
    /// from Unreadable for the OPERATOR, not the sender: nothing is wrong with
    /// the attachment, and no number of retries will help until the
    /// deployment's own configuration changes.
    Blocked,
}

fn classify_media_failure(err: &anyhow::Error) -> MediaFailure {
    if err.chain().any(|c| {
        c.downcast_ref::<crate::media_download::MediaTooLarge>()
            .is_some()
    }) {
        return MediaFailure::TooLarge;
    }
    if err
        .chain()
        .any(|c| c.downcast_ref::<MediaAddrBlocked>().is_some())
    {
        return MediaFailure::Blocked;
    }
    MediaFailure::Unreadable
}

/// Says the memory-flat path could not be taken for a reason that is not the
/// attachment's fault — no temp space, a backend that advertises UploadStream
/// and rejects it. The caller falls back to the buffered path rather than
/// failing an attachment over an optimisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("wecom: streaming media ingest unavailable")]
struct StreamingUnavailable;

fn is_streaming_unavailable(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|c| c.downcast_ref::<StreamingUnavailable>().is_some())
}

/// A compact stand-in for Go's mime.TypeByExtension over the types this
/// adapter actually meets. Unknown extensions yield None and the caller falls
/// through to sniffing.
fn mime_type_by_extension(ext: &str) -> Option<String> {
    let ct = match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "amr" => "audio/amr",
        "wav" => "audio/wav",
        "ogg" => "application/ogg",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "txt" | "log" | "md" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "json" => "application/json",
        _ => return None,
    };
    Some(ct.to_string())
}

/// The reverse lookup used when naming unnamed files.
fn mime_extension_by_type(ct: &str) -> Option<&'static str> {
    Some(match ct {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "image/svg+xml" => ".svg",
        "video/mp4" => ".mp4",
        "video/quicktime" => ".mov",
        "video/webm" => ".webm",
        "audio/mpeg" => ".mp3",
        "audio/amr" => ".amr",
        "audio/wav" => ".wav",
        "application/ogg" => ".ogg",
        "application/pdf" => ".pdf",
        "application/zip" => ".zip",
        "application/msword" => ".doc",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => ".docx",
        "application/vnd.ms-excel" => ".xls",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => ".xlsx",
        "application/vnd.ms-powerpoint" => ".ppt",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => ".pptx",
        "text/plain" | "text/plain; charset=utf-8" => ".txt",
        "text/csv" | "text/csv; charset=utf-8" => ".csv",
        "text/html" | "text/html; charset=utf-8" => ".html",
        "application/json" => ".json",
        _ => return None,
    })
}

/// A compact stand-in for Go's http.DetectContentType over the signatures that
/// matter here: it reads the head of the decrypted bytes and names the common
/// image/video/document formats, falling back to octet-stream. Text-shaped
/// bodies report as UTF-8 plain text, matching Go's sniffing class.
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
    if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        return "image/webp".to_string();
    }
    if data.starts_with(b"%PDF-") {
        return "application/pdf".to_string();
    }
    if data.len() >= 12 && &data[4..8] == b"ftyp" {
        return "video/mp4".to_string();
    }
    if data.starts_with(b"ID3")
        || data.starts_with(&[0xff, 0xfb])
        || data.starts_with(&[0xff, 0xf3])
    {
        return "audio/mpeg".to_string();
    }
    if data.starts_with(b"OggS") {
        return "application/ogg".to_string();
    }
    if data.starts_with(b"\x1f\x8b") {
        return "application/x-gzip".to_string();
    }
    if data.starts_with(b"PK\x03\x04") {
        return "application/zip".to_string();
    }
    if looks_like_text(data) {
        return "text/plain; charset=utf-8".to_string();
    }
    "application/octet-stream".to_string()
}

fn looks_like_text(data: &[u8]) -> bool {
    let head = &data[..data.len().min(512)];
    if head.is_empty() {
        return false;
    }
    head.iter()
        .all(|&b| b == b'\t' || b == b'\n' || b == b'\r' || (0x20..=0x7e).contains(&b) || b >= 0x80)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media_download::MAX_MEDIA_BYTES;

    #[test]
    fn object_key_is_deterministic_and_scoped() {
        let inst = ResolvedInstallation {
            id: Uuid::now_v7(),
            workspace_id: Uuid::now_v7(),
            ..Default::default()
        };
        let msg = Uuid::now_v7();
        let a = media_object_key(&inst, msg, "m1", 0, "image");
        let b = media_object_key(&inst, msg, "m1", 0, "image");
        assert_eq!(a, b);
        assert!(a.starts_with(&format!(
            "workspaces/{}/wecom/{}/",
            inst.workspace_id, inst.id
        )));

        // Position and kind are part of the key: two photos in one mixed
        // message must not collide.
        let c = media_object_key(&inst, msg, "m1", 1, "image");
        assert_ne!(a, c);
        let d = media_object_key(&inst, msg, "m1", 0, "file");
        assert_ne!(a, d);
        // A different chat message never shares a key.
        let e = media_object_key(&inst, Uuid::now_v7(), "m1", 0, "image");
        assert_ne!(a, e);
    }

    #[test]
    fn describe_prefers_header_name_then_extension_then_sniff() {
        let wm = WecomInboundMessage::default();
        let m = InboundMedia {
            kind: "file".to_string(),
            url: "https://cos/x".to_string(),
            aes_key: "k".to_string(),
        };

        let (name, ct) = describe_media(&wm, 0, &m, "report.pdf", b"\x00\x01\x02");
        assert_eq!(name, "report.pdf");
        assert_eq!(ct, "application/pdf");

        // No name: sniffed.
        let png: Vec<u8> = b"\x89PNG\r\n\x1a\nrest".to_vec();
        let (name, ct) = describe_media(&wm, 2, &m, "", &png);
        assert_eq!(name, "wecom-file-unknown-2.png");
        assert_eq!(ct, "image/png");

        // Name without a useful extension: sniff wins.
        let (name, ct) = describe_media(&wm, 0, &m, "noext", b"%PDF-1.7 rest");
        assert_eq!(name, "noext");
        assert_eq!(ct, "application/pdf");

        // Zip-container extensions beat sniffing (docx IS a zip).
        let zipish = b"PK\x03\x04whatever";
        let (name, ct) = describe_media(&wm, 0, &m, "notes.docx", zipish);
        assert_eq!(
            ct,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
        assert_eq!(name, "notes.docx");
    }

    #[test]
    fn fallback_names_are_unique_per_position_and_kind() {
        let img = fallback_media_name("msg==1", 0, &MsgType::image(), "image/jpeg");
        let vid = fallback_media_name("msg==1", 0, &MsgType::video(), "video/mp4");
        let img2 = fallback_media_name("msg==1", 1, &MsgType::image(), "image/jpeg");
        assert_eq!(img, "wecom-image-msg__1-0.jpg");
        assert_eq!(vid, "wecom-video-msg__1-0.mp4");
        assert_ne!(img, img2);

        // An id of nothing usable still produces a stable name.
        assert_eq!(
            fallback_media_name("   ", 0, &MsgType::file(), "application/octet-stream"),
            "wecom-file-unknown-0"
        );
    }

    #[test]
    fn safe_segment_reduces_hostile_ids() {
        assert_eq!(safe_media_segment("abc-DEF_123"), "abc-DEF_123");
        // Dots map to underscores like any other stripped rune, then edge
        // underscores are trimmed — so "../etc/passwd" collapses to "etc_passwd".
        assert_eq!(safe_media_segment("../etc/passwd"), "etc_passwd");
        assert_eq!(safe_media_segment(""), "unknown");
        assert_eq!(safe_media_segment("  "), "unknown");
        assert_eq!(safe_media_segment("___"), "unknown");
    }

    #[test]
    fn media_extension_prefers_familiar_spellings() {
        assert_eq!(media_extension("image/jpeg"), ".jpg");
        assert_eq!(media_extension("IMAGE/JPEG"), ".jpg");
        assert_eq!(media_extension("image/png"), ".png");
        assert_eq!(media_extension("video/mp4"), ".mp4");
        assert_eq!(media_extension("application/pdf"), ".pdf");
        assert_eq!(media_extension("text/csv; charset=utf-8"), ".csv");
        assert_eq!(media_extension("weird/type"), "");
    }

    #[test]
    fn sniffing_covers_the_common_formats() {
        assert_eq!(detect_content_type(b"\x89PNG\r\n\x1a\n"), "image/png");
        assert_eq!(detect_content_type(b"\xff\xd8\xff\xe0"), "image/jpeg");
        assert_eq!(detect_content_type(b"GIF89a...."), "image/gif");
        assert_eq!(detect_content_type(b"%PDF-1.7"), "application/pdf");
        let mp4 = [
            0u8, 0, 0, 32, b'f', b't', b'y', b'p', b'm', b'p', b'4', b'2',
        ];
        assert_eq!(detect_content_type(&mp4), "video/mp4");
        assert_eq!(detect_content_type(b"PK\x03\x04"), "application/zip");
        assert_eq!(
            detect_content_type(b"hello world"),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            detect_content_type(&[0x00, 0x01, 0x02]),
            "application/octet-stream"
        );
    }

    #[test]
    fn failure_classification_walks_wrapped_chains() {
        let too_large = crate::media_download::MediaTooLarge {
            limit: MAX_MEDIA_BYTES,
        };
        let wrapped = anyhow::Error::new(too_large).context("wecom: media decrypt: read");
        assert_eq!(classify_media_failure(&wrapped), MediaFailure::TooLarge);

        let wrapped = anyhow::Error::new(MediaAddrBlocked).context("outer");
        assert_eq!(classify_media_failure(&wrapped), MediaFailure::Blocked);

        let plain: anyhow::Error = anyhow::anyhow!("socket died");
        assert_eq!(classify_media_failure(&plain), MediaFailure::Unreadable);
    }

    #[test]
    fn streaming_unavailable_is_detected_through_context() {
        let e = anyhow::Error::new(StreamingUnavailable)
            .context("wecom: streaming media ingest unavailable: wecom: media temp file: no space");
        assert!(is_streaming_unavailable(&e));
        let plain = anyhow::anyhow!("bad key");
        assert!(!is_streaming_unavailable(&plain));
    }

    #[test]
    fn notices_are_the_chinese_product_voice() {
        assert_eq!(
            MEDIA_UNREADABLE_NOTICE,
            "抱歉，有附件没能收到，麻烦重新发一次。"
        );
        assert_eq!(MEDIA_TOO_LARGE_NOTICE, "抱歉，附件太大了，我这边收不下。");
    }
}
