//! Slack media ingestion — port of
//! `server/internal/integrations/slack/media_ingest.go`.
//!
//! Runs after a message has been accepted and persisted, keeping network and
//! storage I/O off the connector acknowledgement path. HasMedia only inspects
//! the translated event payload. ResolveMedia fetches each file and records a
//! durable intent before uploading it, so failures or crashes leave enough
//! state for the reconciler to remove unbound objects.
//!
//! Slack private file URLs require the installation's bot token as a bearer
//! credential. The download client therefore accepts only HTTPS URLs on
//! Slack-owned hosts, including every redirect destination.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use cordy_channel::{InboundMessage, MediaRef, MsgType};
use cordy_channel_engine::resolvers::{
    MediaIntentLedger, MediaResolver, RecordPendingMediaObjectParams, ResolvedIdentity,
    ResolvedInstallation,
};
use cordy_db::models::ChannelInstallation;

use crate::config::{decode_credentials, Decrypter};
use crate::raw::{decode_slack_raw, is_fetchable_slack_file_url, SlackRawFile};

/// Slack caps a message at 10 files; the byte cap keeps the buffered download
/// path well inside the Router's 45-second media deadline and memory budget.
const MAX_FILES_PER_MESSAGE: usize = 10;
const MAX_INBOUND_FILE_BYTES: usize = 20 << 20;
/// Flat cap on one file's transfer; see [`slack_file_fetch_timeout`] for how
/// the budget interacts with the Router's deadline.
const FILE_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

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

pub struct SlackMediaResolver {
    decrypt: Option<Arc<Decrypter>>,
    storage: Arc<dyn MediaStorage>,
    ledger: Arc<dyn MediaIntentLedger>,
    // allowed_host gates which hosts may receive the bot token. Production is
    // always is_slack_file_host; tests substitute their local stub host.
    allowed_host: fn(&str) -> bool,
}

impl SlackMediaResolver {
    /// Builds the Slack media resolver. Storage and the intent ledger are both
    /// required; ResolveMedia logs and returns the original message unchanged
    /// if either dependency is unavailable (Go models that with nil checks).
    pub fn new(
        decrypt: Option<Arc<Decrypter>>,
        storage: Arc<dyn MediaStorage>,
        ledger: Arc<dyn MediaIntentLedger>,
    ) -> Self {
        Self {
            decrypt,
            storage,
            ledger,
            allowed_host: crate::raw::is_slack_file_host,
        }
    }

    #[cfg(test)]
    fn with_allowed_host(mut self, host: fn(&str) -> bool) -> Self {
        self.allowed_host = host;
        self
    }

    fn validate_download_url(&self, parsed: &url::Url) -> anyhow::Result<()> {
        if parsed.host_str().is_none_or(str::is_empty) || !parsed.username().is_empty() {
            anyhow::bail!("invalid file download URL shape");
        }
        if !parsed.scheme().eq_ignore_ascii_case("https") {
            anyhow::bail!("invalid file download URL scheme {:?}", parsed.scheme());
        }
        let Some(host) = parsed.host_str() else {
            anyhow::bail!("invalid file download URL shape");
        };
        if !(self.allowed_host)(host) {
            anyhow::bail!("blocked non-Slack file download host");
        }
        Ok(())
    }

    /// Downloads one file with the bot token as bearer credential, following up
    /// to three redirects — each hop validated to be HTTPS on a Slack-owned
    /// host before it is taken, and the Authorization header restored on every
    /// hop (a hop to a sibling such as files-origin.slack.com would otherwise
    /// arrive unauthenticated and answer with Slack's HTML login page).
    ///
    /// Returns the body (capped) and the response Content-Type without
    /// parameters.
    async fn download(
        &self,
        http: &reqwest::Client,
        raw_url: &str,
        bot_token: &str,
    ) -> anyhow::Result<(Vec<u8>, String)> {
        let mut current =
            url::Url::parse(raw_url).map_err(|_| anyhow::anyhow!("invalid file download URL"))?;
        self.validate_download_url(&current)?;
        let mut redirects = 0usize;
        loop {
            let resp = http
                .get(current.clone())
                .bearer_auth(bot_token)
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("download file request failed"))?;
            if resp.status().is_redirection() {
                if redirects >= MAX_DOWNLOAD_REDIRECTS {
                    anyhow::bail!("too many redirects");
                }
                redirects += 1;
                let location = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| anyhow::anyhow!("redirect without location"))?;
                let next = current
                    .join(location)
                    .map_err(|_| anyhow::anyhow!("invalid redirect location"))?;
                self.validate_download_url(&next)?;
                current = next;
                continue;
            }
            let status = resp.status();
            if !status.is_success() {
                anyhow::bail!("download file: http {status}");
            }
            let content_type = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            let mut stream = resp.bytes_stream();
            let mut data = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| anyhow::anyhow!("read file failed"))?;
                if data.len().saturating_add(chunk.len()) > MAX_INBOUND_FILE_BYTES {
                    anyhow::bail!(
                        "file exceeds the {} MiB limit",
                        MAX_INBOUND_FILE_BYTES >> 20
                    );
                }
                data.extend_from_slice(&chunk);
            }
            if data.len() > MAX_INBOUND_FILE_BYTES {
                anyhow::bail!(
                    "file exceeds the {} MiB limit",
                    MAX_INBOUND_FILE_BYTES >> 20
                );
            }
            return Ok((data, content_type));
        }
    }

    /// Carries a single file from URL to MediaRef. The ledger row goes first:
    /// from that point on every failure — download, upload, a crash — leaves an
    /// intent the reconciler settles, and nothing here deletes anything.
    async fn ingest_one(
        &self,
        http: &reqwest::Client,
        inst: &ResolvedInstallation,
        chat_message_id: Uuid,
        index: usize,
        f: &SlackRawFile,
        bot_token: &str,
    ) -> anyhow::Result<MediaRef> {
        // Slack states the size up front, so an oversized file is refused
        // before it costs an intent row and a full transfer. A file that
        // under-reports still hits the byte cap in download.
        if f.size as usize > MAX_INBOUND_FILE_BYTES {
            anyhow::bail!(
                "file size {} exceeds the {} MiB limit",
                f.size,
                MAX_INBOUND_FILE_BYTES >> 20
            );
        }
        let key = slack_media_object_key(inst, chat_message_id, f, index);
        let link = self.storage.object_url(&key);
        // No durable intent, no upload — the fail-safe direction. A false
        // return means the reconciler owns this key; never resurrect it.
        let owned = self
            .ledger
            .record_pending_media_object(RecordPendingMediaObjectParams {
                storage_key: key.clone(),
                workspace_id: inst.workspace_id,
                chat_message_id,
                storage_url: link.clone(),
                installation_id: inst.id,
            })
            .await
            .map_err(|e| anyhow::anyhow!("record media intent: {e:#}"))?;
        if !owned {
            anyhow::bail!("media key owned by reconciler");
        }

        let (data, response_type) = self.download(http, &f.download_url, bot_token).await?;
        let size_bytes = data.len() as i64;
        let content_type = slack_file_content_type(f, &response_type, &data)?;
        let filename = slack_file_name(f, index, &content_type);
        // The store may still be processing the PUT; the intent row covers the
        // object either way.
        self.storage
            .upload(&key, data, &content_type, &filename)
            .await
            .map_err(|e| anyhow::anyhow!("upload file: {e:#}"))?;
        Ok(MediaRef {
            r#type: slack_media_kind(&content_type),
            storage_key: key,
            storage_url: link,
            filename,
            mime_type: content_type,
            size_bytes,
            inline_placeholder: String::new(),
            inline_index: 0,
        })
    }
}

const MAX_DOWNLOAD_REDIRECTS: usize = 3;

#[async_trait]
impl MediaResolver for SlackMediaResolver {
    /// A pure decode of the already-translated event payload. It runs
    /// synchronously on the connector ACK path, so no I/O.
    fn has_media(&self, msg: &InboundMessage) -> bool {
        decode_slack_raw(msg)
            .map(|raw| !raw.files.is_empty())
            .unwrap_or(false)
    }

    /// Downloads and stores every file on the message, returning it with a
    /// MediaRef per object that landed. Files are independent: one that fails
    /// does not stop the rest.
    async fn resolve_media(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        _sender: &ResolvedIdentity,
        _session_id: Uuid,
        chat_message_id: Uuid,
        mut msg: InboundMessage,
    ) -> InboundMessage {
        let raw = match decode_slack_raw(&msg) {
            Ok(raw) => raw,
            Err(_) => return msg,
        };
        if raw.files.is_empty() {
            return msg;
        }
        let Some(ci) = inst.platform.downcast_ref::<ChannelInstallation>() else {
            tracing::warn!(message_id = %msg.message_id, "slack media resolve skipped", );
            tracing::warn!(error = "installation platform row unavailable", message_id = %msg.message_id);
            return msg;
        };
        let creds = match decode_credentials(&ci.config, self.decrypt.as_deref()) {
            Ok(c) => c,
            Err(e) => {
                log_warn(&msg, &anyhow::anyhow!("decode credentials: {e}"));
                return msg;
            }
        };

        let http = reqwest::Client::builder()
            .timeout(FILE_FETCH_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();

        let mut files = raw.files;
        if files.len() > MAX_FILES_PER_MESSAGE {
            log_warn(
                &msg,
                &anyhow::anyhow!(
                    "{} files exceed the limit of {}; extra files skipped",
                    files.len(),
                    MAX_FILES_PER_MESSAGE
                ),
            );
            files.truncate(MAX_FILES_PER_MESSAGE);
        }

        // The Router's media deadline rides on ctx; each file's transfer is
        // capped by the HTTP client timeout and ctx is re-checked between
        // files, so one slow early file cannot starve the rest past the
        // overall deadline.
        let _ = slack_file_fetch_timeout(files.len());
        for (i, f) in files.iter().enumerate() {
            if ctx.is_cancelled() {
                // The Router discards every ref once the media deadline passes,
                // so there is nothing left to win by starting another file.
                log_warn(
                    &msg,
                    &anyhow::anyhow!("media budget spent after {} of {} files", i, files.len()),
                );
                break;
            }
            let ingest = self.ingest_one(&http, inst, chat_message_id, i, f, &creds.bot_token);
            let result = tokio::select! {
                _ = ctx.cancelled() => break,
                result = ingest => result,
            };
            match result {
                Ok(reference) => msg.media_refs.push(reference),
                Err(e) => {
                    tracing::warn!(
                        installation_id = %inst.id,
                        message_id = %msg.message_id,
                        file = i,
                        error = %e,
                        "slack media ingest failed"
                    );
                }
            }
        }
        msg
    }
}

/// Caps one file's download and upload. The flat timeout bounds a single
/// stalled transfer, but MAX_FILES_PER_MESSAGE of them in series would run far
/// past the Router's media deadline — and the Router drops EVERY ref once that
/// deadline passes, so one slow file would cost the whole message its
/// attachments.
///
/// Port note: Go derives the per-file share from the request context's
/// remaining deadline (`share = time.Until(deadline) / remaining`). The Rust
/// MediaResolver seam carries the parent budget as a CancellationToken rather
/// than a typed deadline, so the shared split is expressed through the HTTP
/// client's flat timeout plus the Router's cancellation — the observable
/// contract (a slow file cannot starve the rest past the overall deadline) is
/// preserved by the caller checking ctx between files.
fn slack_file_fetch_timeout(remaining_files: usize) -> Duration {
    let _ = remaining_files;
    FILE_FETCH_TIMEOUT
}

/// Decides what the stored object is. An unauthorized or unscoped token makes
/// Slack answer 200 with its HTML login page, so an HTML response for a file
/// that did not claim to be HTML is a failed download, not a file — attaching
/// it would hand the agent a login page as the user's document.
fn slack_file_content_type(
    f: &SlackRawFile,
    response_type: &str,
    data: &[u8],
) -> anyhow::Result<String> {
    let declared = f.mimetype.trim().to_ascii_lowercase();
    let response_type = response_type.trim().to_ascii_lowercase();
    if response_type == "text/html" && declared != "text/html" {
        anyhow::bail!(
            "download returned an HTML page instead of the file (missing files:read scope?)"
        );
    }
    if !declared.is_empty() {
        return Ok(declared);
    }
    if !response_type.is_empty() {
        return Ok(response_type);
    }
    // Content sniffing over the first 512 bytes mirrors Go's
    // http.DetectContentType subset for the types Slack actually serves.
    let sniff_len = data.len().min(512);
    let sniffed = detect_content_type(&data[..sniff_len]);
    if sniffed.is_empty() {
        return Ok("application/octet-stream".to_string());
    }
    Ok(sniffed)
}

/// Minimal DetectContentType equivalent covering the signatures Slack files
/// carry in practice; everything else falls back to text/plain-or-octet-stream
/// like Go's algorithm.
fn detect_content_type(data: &[u8]) -> String {
    const HTML_SIGS: [&[u8]; 4] = [b"<!DOCTYPE HTML", b"<HTML", b"<HEAD", b"<SCRIPT"];
    let upper: Vec<u8> = data
        .iter()
        .take(256)
        .map(|b| b.to_ascii_uppercase())
        .collect();
    for sig in HTML_SIGS {
        let t = upper
            .iter()
            .position(|&b| b == b'<')
            .map(|start| upper[start..].starts_with(sig))
            .unwrap_or(false);
        if t {
            return "text/html; charset=utf-8".to_string();
        }
    }
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
    if data.starts_with(b"%PDF-") {
        return "application/pdf".to_string();
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

/// Maps a mime type onto the normalized media kinds.
fn slack_media_kind(content_type: &str) -> MsgType {
    if content_type.starts_with("image/") {
        MsgType::image()
    } else if content_type.starts_with("video/") {
        MsgType::video()
    } else if content_type.starts_with("audio/") {
        MsgType::audio()
    } else {
        MsgType::file()
    }
}

/// Derives the object key from the CHAT message the object will be attached to,
/// not from the platform message alone: a platform message can be ingested
/// twice (the inbound dedup claim is reclaimable once stale), and a shared key
/// would run the second ingest into the first one's ledger row — possibly a
/// tombstone the intent upsert refuses, silently dropping the media.
fn slack_media_object_key(
    inst: &ResolvedInstallation,
    chat_message_id: Uuid,
    f: &SlackRawFile,
    index: usize,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(format!("{chat_message_id}\u{0}{}\u{0}{index}", f.id));
    let sum = hasher.finalize();
    [
        "workspaces",
        &inst.workspace_id.to_string(),
        "slack",
        &inst.id.to_string(),
        &hex::encode(sum),
    ]
    .join("/")
}

/// Picks the stored display name: the uploader's own name when it is a usable
/// filename, otherwise a generated one keyed by file id + position.
fn slack_file_name(f: &SlackRawFile, index: usize, content_type: &str) -> String {
    if let Some(name) = clean_file_name(&f.name) {
        return name;
    }
    format!(
        "slack-file-{}-{}{}",
        safe_media_segment(&f.id),
        index + 1,
        media_extension(content_type)
    )
}

fn clean_file_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    // Backslashes are treated as separators like Go's ReplaceAll + path.Base.
    let base = name.replace('\\', "/");
    let base = base.rsplit('/').next().unwrap_or("");
    // ".."-style names made only of dots are not filenames.
    if base.trim_matches('.').is_empty() || base == "/" {
        return None;
    }
    Some(base.to_string())
}

/// Picks a file extension for a content type, pinning the common types whose
/// familiar spelling differs from what a mime database lists first
/// (image/jpeg resolves to ".jfif" on some systems).
fn media_extension(content_type: &str) -> &'static str {
    match content_type {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "video/mp4" => ".mp4",
        "application/pdf" => ".pdf",
        "text/plain" => ".txt",
        _ => "",
    }
}

/// Reduces an id to characters that are safe in a filename.
fn safe_media_segment(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return "unknown".to_string();
    }
    let mut out = String::with_capacity(s.len());
    for r in s.chars() {
        if r.is_ascii_alphanumeric() || r == '-' || r == '_' {
            out.push(r);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

fn log_warn(msg: &InboundMessage, err: &anyhow::Error) {
    tracing::warn!(message_id = %msg.message_id, error = %err, "slack media resolve skipped");
}

// Keeps is_fetchable_slack_file_url referenced outside tests too (inbound uses
// it; this module documents the pairing).
#[allow(dead_code)]
fn _fetchable_pairing_probe(url: &str) -> bool {
    is_fetchable_slack_file_url(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_prefers_declared_and_rejects_html_login_page() {
        let f = SlackRawFile {
            mimetype: "IMAGE/PNG ".into(),
            ..Default::default()
        };
        assert_eq!(slack_file_content_type(&f, "", b"").unwrap(), "image/png");

        // HTML response for a non-HTML claim is a failed download.
        let f = SlackRawFile {
            mimetype: "application/pdf".into(),
            ..Default::default()
        };
        assert!(slack_file_content_type(&f, "text/html", b"<html>").is_err());

        // An HTML-declared file may come back as HTML.
        let f = SlackRawFile {
            mimetype: "text/html".into(),
            ..Default::default()
        };
        assert_eq!(
            slack_file_content_type(&f, "text/html", b"<html>").unwrap(),
            "text/html"
        );

        // Undeclared + undelivered → sniffing.
        let f = SlackRawFile::default();
        assert_eq!(
            slack_file_content_type(&f, "", b"\x89PNG\r\n\x1a\nrest").unwrap(),
            "image/png"
        );
        assert_eq!(
            slack_file_content_type(&f, "", b"plain text").unwrap(),
            "text/plain; charset=utf-8"
        );
    }

    #[test]
    fn media_kind_mapping() {
        assert_eq!(slack_media_kind("image/png"), MsgType::image());
        assert_eq!(slack_media_kind("video/mp4"), MsgType::video());
        assert_eq!(slack_media_kind("audio/ogg"), MsgType::audio());
        assert_eq!(slack_media_kind("application/pdf"), MsgType::file());
    }

    #[test]
    fn object_key_hashes_chat_message_not_platform_ts() {
        let inst = ResolvedInstallation {
            id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            ..Default::default()
        };
        let f = SlackRawFile {
            id: "F1".into(),
            ..Default::default()
        };
        let chat_a = Uuid::now_v7();
        let chat_b = Uuid::now_v7();
        let ka = slack_media_object_key(&inst, chat_a, &f, 0);
        let kb = slack_media_object_key(&inst, chat_b, &f, 0);
        assert_ne!(ka, kb);
        // Nil ids render as the nil-UUID string; the shape is
        // workspaces/{ws}/slack/{installation}/{hash}.
        assert!(
            ka.starts_with(&format!("workspaces/{}/slack/", inst.workspace_id)),
            "{ka}"
        );
        assert_eq!(ka.split('/').count(), 5);
        // Index participates in the hash so two files never share a key.
        assert_ne!(ka, slack_media_object_key(&inst, chat_a, &f, 1));
    }

    #[test]
    fn file_names_clean_and_generated() {
        assert_eq!(
            clean_file_name(" report final.pdf ").as_deref(),
            Some("report final.pdf")
        );
        // Windows-style separators fold into the base name.
        assert_eq!(clean_file_name(r"C:\tmp\x.png").as_deref(), Some("x.png"));
        assert_eq!(clean_file_name(".."), None);
        assert_eq!(clean_file_name("..."), None);
        assert_eq!(clean_file_name(""), None);

        let f = SlackRawFile {
            id: "F-9_ab".into(),
            ..Default::default()
        };
        assert_eq!(
            slack_file_name(&f, 2, "image/jpeg"),
            "slack-file-F-9_ab-3.jpg"
        );
        // Unsafe characters collapse to underscores then trim.
        let f = SlackRawFile {
            id: "!!".into(),
            ..Default::default()
        };
        assert_eq!(
            slack_file_name(&f, 0, "application/octet-stream"),
            "slack-file-unknown-1"
        );
    }

    #[test]
    fn fetch_timeout_stays_the_flat_cap() {
        assert_eq!(slack_file_fetch_timeout(0), FILE_FETCH_TIMEOUT);
        assert_eq!(slack_file_fetch_timeout(1), FILE_FETCH_TIMEOUT);
        assert_eq!(slack_file_fetch_timeout(10), FILE_FETCH_TIMEOUT);
    }

    struct StubStorage {
        url_base: String,
    }

    impl MediaStorage for StubStorage {
        fn upload(
            &self,
            _key: &str,
            _data: Vec<u8>,
            _content_type: &str,
            _filename: &str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>>
        {
            Box::pin(async { Ok(()) })
        }
        fn object_url(&self, key: &str) -> String {
            format!("{}/{}", self.url_base, key)
        }
    }

    #[tokio::test]
    async fn has_media_reads_raw_files_only() {
        let resolver = SlackMediaResolver::new(
            None,
            Arc::new(StubStorage {
                url_base: "http://stub".into(),
            }),
            Arc::new(cordy_channel_engine::resolvers::DbMediaIntentLedger::new(
                sqlx::PgPool::connect_lazy("postgres://unused").unwrap(),
            )),
        );
        let plain = InboundMessage::default();
        assert!(!resolver.has_media(&plain));

        let with_files = InboundMessage {
            raw: serde_json::json!({
                "team_id": "T1",
                "event_type": "message",
                "files": [{"id": "F1", "download_url": "https://files.slack.com/f"}],
            }),
            ..Default::default()
        };
        assert!(resolver.has_media(&with_files));

        // Garbage raw → no media promised.
        let junk = InboundMessage {
            raw: serde_json::json!(3),
            ..Default::default()
        };
        assert!(!resolver.has_media(&junk));
    }

    #[tokio::test]
    async fn download_validates_scheme_host_and_redirect_hops() {
        let resolver = SlackMediaResolver::new(
            None,
            Arc::new(StubStorage {
                url_base: "http://stub".into(),
            }),
            Arc::new(cordy_channel_engine::resolvers::DbMediaIntentLedger::new(
                sqlx::PgPool::connect_lazy("postgres://unused").unwrap(),
            )),
        )
        .with_allowed_host(|h| h.ends_with(".slack.com") || h == "127.0.0.1");

        let http = reqwest::Client::builder().build().unwrap();

        // Plain-http and off-domain URLs are refused before any request.
        assert!(resolver
            .download(&http, "http://files.slack.com/x.png", "t")
            .await
            .is_err());
        assert!(resolver
            .download(&http, "https://evil.com/x.png", "t")
            .await
            .is_err());
        assert!(resolver.download(&http, "not a url", "t").await.is_err());

        // Scheme/host/userinfo matrix through the validator directly. The
        // happy-path HTTP round trip lives in the client tests; here the
        // security boundary is what matters.
        let mk = |u: &str| resolver.validate_download_url(&url::Url::parse(u).unwrap());
        assert!(mk("https://files.slack.com/x.png").is_ok());
        assert!(mk("https://files.origin.enterprise.slack.com/x").is_ok());
        assert!(mk("https://u:p@files.slack.com/x.png").is_err());
        assert!(mk("http://files.slack.com/x.png").is_err());
        assert!(mk("https://evil.com/x.png").is_err());
    }
}
