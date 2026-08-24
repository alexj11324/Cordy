//! Feishu media ingestion — port of
//! `server/internal/integrations/lark/media_ingest.go`.
//!
//! HasMedia is a pure in-memory decode of the already-received payload — it
//! runs on the connector ACK path. ResolveMedia downloads each resource and
//! records a durable intent BEFORE any write can happen, so every failure from
//! there on (download error, upload error, resolve deadline, crash) simply
//! leaves the intent row for the reconciler; nothing is ever deleted inline.

use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use cordy_channel::{InboundMessage as ChannelMessage, MediaRef, MsgType};
use cordy_channel_engine::resolvers::{
    MediaIntentLedger, MediaResolver, RecordPendingMediaObjectParams, ResolvedIdentity,
    ResolvedInstallation,
};

use crate::client::{
    ApiClient, DownloadResourceParams, DownloadedResourceStream, InstallationCredentials,
};
use crate::content_flatten::LarkPostContent;
use crate::feishu_types::InboundMessage;
use crate::installation::{installation_credentials_for, CredentialsResolver};
use crate::resolvers::lark_msg_from_raw;
use crate::store::Installation;

/// The object-store operations required for ingestion. [`MediaStorage::object_url`]
/// must derive the final URL without performing I/O so the resolver can persist
/// that URL in the intent ledger before uploading the object.
pub trait MediaStorage: Send + Sync {
    fn upload(
        &self,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>>;
    /// The URL a successful upload of key returns.
    fn object_url(&self, key: &str) -> String;
}

/// Optional streaming upload capability. Unknown-length HTTP bodies cannot be
/// sent through S3 PutObject as a non-seekable stream; storages that CAN take a
/// sized stream implement this and the resolver prefers it when the size is
/// known up front.
#[async_trait]
pub trait MediaStreamStorage: Send + Sync {
    async fn upload_stream(
        &self,
        ctx: CancellationToken,
        key: &str,
        body: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
        size_bytes: i64,
        content_type: &str,
        filename: &str,
    ) -> anyhow::Result<()>;
}

pub struct FeishuMediaResolver {
    api: Arc<dyn ApiClient>,
    creds: Arc<dyn CredentialsResolver>,
    storage: Arc<dyn MediaStorage>,
    ledger: Arc<dyn MediaIntentLedger>,
}

impl FeishuMediaResolver {
    pub fn new(
        api: Arc<dyn ApiClient>,
        creds: Arc<dyn CredentialsResolver>,
        storage: Arc<dyn MediaStorage>,
        ledger: Arc<dyn MediaIntentLedger>,
    ) -> Self {
        Self {
            api,
            creds,
            storage,
            ledger,
        }
    }
}

#[async_trait]
impl MediaResolver for FeishuMediaResolver {
    /// Reports whether the message carries downloadable Feishu resources
    /// (standalone image/video/file/audio or post-embedded img/media spans).
    /// Pure in-memory decode of the already-received payload — it runs on the
    /// connector ACK path.
    fn has_media(&self, msg: &ChannelMessage) -> bool {
        match lark_msg_from_raw(msg) {
            Ok(lm) => !media_resources_from_message(&lm).is_empty(),
            Err(_) => false,
        }
    }

    async fn resolve_media(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        _sender: &ResolvedIdentity,
        _session_id: Uuid,
        chat_message_id: Uuid,
        mut msg: ChannelMessage,
    ) -> ChannelMessage {
        if ctx.is_cancelled() {
            return msg;
        }
        let lm = match lark_msg_from_raw(&msg) {
            Ok(lm) => lm,
            Err(err) => {
                log_media_warn(
                    "lark media ingest skipped: raw decode failed",
                    &msg.message_id,
                    &msg.r#type.0,
                    Some(&err),
                );
                return msg;
            }
        };
        let resources = media_resources_from_message(&lm);
        if resources.is_empty() {
            return msg;
        }
        let Some(lark_inst) = inst.platform.downcast_ref::<Installation>() else {
            log_media_warn(
                "lark media ingest skipped: installation payload unavailable",
                &lm.message_id,
                &lm.message_type,
                None,
            );
            return msg;
        };
        let creds = match installation_credentials_for(self.creds.as_ref(), lark_inst) {
            Ok(c) => c,
            Err(err) => {
                log_media_warn(
                    "lark media ingest skipped: credentials unavailable",
                    &lm.message_id,
                    &lm.message_type,
                    Some(&err),
                );
                return msg;
            }
        };

        for (res_index, res) in resources.iter().enumerate() {
            if ctx.is_cancelled() {
                return msg;
            }
            let key = media_object_key(inst, chat_message_id, res);
            let link = self.storage.object_url(&key);
            // Persist the upload intent BEFORE any write can happen. Every
            // failure from here on — download error, upload error (even one
            // the store may still be processing), resolve deadline, crash —
            // simply leaves this row for the reconciler; nothing is ever
            // deleted inline.
            let pending = self
                .ledger
                .record_pending_media_object(RecordPendingMediaObjectParams {
                    storage_key: key.clone(),
                    workspace_id: inst.workspace_id,
                    chat_message_id,
                    storage_url: link.clone(),
                    installation_id: inst.id,
                });
            let owned = match tokio::select! {
                _ = ctx.cancelled() => return msg,
                result = pending => result,
            } {
                Ok(owned) => owned,
                Err(err) => {
                    // No durable intent, no upload — fail-safe direction.
                    log_media_warn(
                        "lark media ingest skipped: intent record failed",
                        &lm.message_id,
                        &lm.message_type,
                        Some(&err),
                    );
                    continue;
                }
            };
            if !owned {
                // The reconciler owns this key ('deleting'); never resurrect it.
                log_media_warn(
                    "lark media ingest skipped: key owned by reconciler",
                    &lm.message_id,
                    &lm.message_type,
                    None,
                );
                continue;
            }
            let download = self.download_resource(
                &creds,
                DownloadResourceParams {
                    message_id: res.message_id.clone(),
                    file_key: res.key.clone(),
                    r#type: res.fetch_type.clone(),
                },
            );
            let got = match tokio::select! {
                _ = ctx.cancelled() => return msg,
                result = download => result,
            } {
                Ok(g) => g,
                Err(err) => {
                    log_media_warn(
                        "lark media download failed",
                        &lm.message_id,
                        &lm.message_type,
                        Some(&err),
                    );
                    continue;
                }
            };
            let content_type = media_content_type(res, &got);
            let filename = media_filename(&lm, res, &got, &content_type, res_index);
            let upload = self.upload_resource(ctx.clone(), &key, got, &content_type, &filename);
            match tokio::select! {
                _ = ctx.cancelled() => return msg,
                result = upload => result,
            } {
                Ok(size_bytes) => {
                    msg.media_refs.push(MediaRef {
                        r#type: res.kind.clone(),
                        storage_key: key,
                        storage_url: link,
                        filename,
                        mime_type: content_type,
                        size_bytes,
                        inline_placeholder: String::new(),
                        inline_index: 0,
                    });
                }
                Err(err) => {
                    // The store may still be processing the PUT — deleting here
                    // could reorder with it. The intent row (written above)
                    // covers the object either way; the reconciler settles it.
                    log_media_warn(
                        "lark media upload failed",
                        &lm.message_id,
                        &lm.message_type,
                        Some(&err),
                    );
                }
            }
        }
        msg
    }
}

impl FeishuMediaResolver {
    async fn download_resource(
        &self,
        creds: &InstallationCredentials,
        p: DownloadResourceParams,
    ) -> anyhow::Result<DownloadedResourceStream> {
        self.api
            .download_message_resource_stream(creds.clone(), p)
            .await
    }

    /// Buffers the bounded transport stream once, then uploads it through the
    /// shared storage API. This avoids the previous buffered-download plus
    /// upload-buffer copy while preserving the storage lane's current API.
    async fn upload_resource(
        &self,
        _ctx: CancellationToken,
        key: &str,
        got: DownloadedResourceStream,
        content_type: &str,
        filename: &str,
    ) -> anyhow::Result<i64> {
        let DownloadedResourceStream {
            mut body,
            content_type: _,
            filename: _,
            size_bytes: _size_hint,
        } = got;
        let mut data = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut body, &mut data).await?;
        let n = data.len() as i64;
        self.storage
            .upload(key, data, content_type, filename)
            .await?;
        Ok(n)
    }
}

fn log_media_warn(msg: &str, message_id: &str, message_type: &str, err: Option<&anyhow::Error>) {
    match err {
        Some(e) => tracing::warn!(
            message_id = %message_id,
            message_type = %message_type,
            error = %e,
            "{msg}"
        ),
        None => tracing::warn!(
            message_id = %message_id,
            message_type = %message_type,
            "{msg}"
        ),
    }
}

#[derive(Default)]
pub struct LarkMediaResource {
    pub key: String,
    pub kind: MsgType,
    pub fetch_type: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub message_id: String,
}

#[derive(Default, serde::Deserialize)]
struct MessagePayload {
    #[serde(default, rename = "image_key")]
    image_key: String,
    #[serde(default, rename = "file_key")]
    file_key: String,
    #[serde(default, rename = "file_name")]
    file_name: String,
    #[serde(default, rename = "name")]
    name: String,
    #[serde(default, rename = "mime_type")]
    mime_type: String,
    #[serde(default, rename = "content_type")]
    content_type: String,
    #[serde(default, rename = "size")]
    size: i64,
    #[serde(default, rename = "size_bytes")]
    size_bytes: i64,
}

pub fn media_resources_from_message(lm: &InboundMessage) -> Vec<LarkMediaResource> {
    if lm.content.is_empty() {
        return Vec::new();
    }
    let Ok(payload) = serde_json::from_str::<MessagePayload>(&lm.content) else {
        return Vec::new();
    };
    let filename = first_non_empty(&[&payload.file_name, &payload.name]);
    let mime_type = first_non_empty(&[&payload.mime_type, &payload.content_type]);
    let mut size_bytes = payload.size_bytes;
    if size_bytes == 0 {
        size_bytes = payload.size;
    }
    let message_type = lm.message_type.as_str();
    match message_type {
        "image" => {
            if payload.image_key.is_empty() {
                return Vec::new();
            }
            vec![LarkMediaResource {
                key: payload.image_key,
                kind: MsgType::image(),
                fetch_type: "image".to_string(),
                filename,
                mime_type,
                size_bytes,
                message_id: lm.message_id.clone(),
            }]
        }
        "post" => media_resources_from_post(lm),
        "media" | "video" => {
            if payload.file_key.is_empty() {
                return Vec::new();
            }
            vec![LarkMediaResource {
                key: payload.file_key,
                kind: MsgType::video(),
                fetch_type: "file".to_string(),
                filename,
                mime_type,
                size_bytes,
                message_id: lm.message_id.clone(),
            }]
        }
        "file" | "audio" => {
            if payload.file_key.is_empty() {
                return Vec::new();
            }
            let kind = if message_type == "audio" {
                MsgType::audio()
            } else {
                MsgType::file()
            };
            vec![LarkMediaResource {
                key: payload.file_key,
                kind,
                fetch_type: "file".to_string(),
                filename,
                mime_type,
                size_bytes,
                message_id: lm.message_id.clone(),
            }]
        }
        _ => Vec::new(),
    }
}

fn media_resources_from_post(lm: &InboundMessage) -> Vec<LarkMediaResource> {
    if lm.content.is_empty() {
        return Vec::new();
    }
    let Ok(doc) = serde_json::from_str::<LarkPostContent>(&lm.content) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // A post may reference the same image_key/file_key in more than one span.
    // The object key is derived from (message, type, key), so duplicates would
    // upload to the SAME key twice: a later failed attempt could destroy the
    // object an earlier success already produced (dangling attachment), and a
    // later success would yield two attachment rows for one object. Collapse
    // them here, before any upload.
    let mut seen = std::collections::HashSet::new();
    for para in &doc.content {
        for span in para.iter() {
            match span.tag.as_str() {
                "img" => {
                    if span.image_key.is_empty()
                        || seen.contains(&format!("image\u{0}{}", span.image_key))
                    {
                        continue;
                    }
                    seen.insert(format!("image\u{0}{}", span.image_key));
                    out.push(LarkMediaResource {
                        key: span.image_key.clone(),
                        kind: MsgType::image(),
                        fetch_type: "image".to_string(),
                        filename: first_non_empty(&[&span.file_name, &span.name]),
                        mime_type: span.mime_type.clone(),
                        size_bytes: 0,
                        message_id: lm.message_id.clone(),
                    });
                }
                "media" => {
                    if span.file_key.is_empty()
                        || seen.contains(&format!("file\u{0}{}", span.file_key))
                    {
                        continue;
                    }
                    seen.insert(format!("file\u{0}{}", span.file_key));
                    out.push(LarkMediaResource {
                        key: span.file_key.clone(),
                        kind: MsgType::video(),
                        fetch_type: "file".to_string(),
                        filename: first_non_empty(&[&span.file_name, &span.name]),
                        mime_type: span.mime_type.clone(),
                        size_bytes: 0,
                        message_id: lm.message_id.clone(),
                    });
                }
                _ => {}
            }
        }
    }
    out
}

/// Derives the object key from the CHAT message the object will be attached to,
/// not from the platform message alone: a platform message can be ingested
/// twice (the inbound dedup row is reclaimable once its claim is 60s stale, and
/// vacuumed after 24h), and a shared key would run the second ingest into the
/// first one's ledger row. That row may be a tombstone — the intent upsert
/// refuses anything that has left 'pending', so the second ingest would
/// silently drop its media for as long as the re-delete schedule runs. A key
/// per chat message keeps the ingests independent; nothing leaks, because each
/// one's objects are covered by its own ledger row.
fn media_object_key(
    inst: &ResolvedInstallation,
    chat_message_id: Uuid,
    res: &LarkMediaResource,
) -> String {
    let sum = Sha256::digest(format!(
        "{chat_message_id}\u{0}{}\u{0}{}\u{0}{}",
        res.message_id, res.fetch_type, res.key
    ));
    [
        "workspaces",
        &inst.workspace_id.to_string(),
        "lark",
        &inst.id.to_string(),
        &hex::encode(sum),
    ]
    .join("/")
}

/// Picks a name for the stored object. index disambiguates the generated form:
/// every resource on one message shares a MessageID, so three photos in one
/// send used to produce three objects all named "feishu-image-<msg>.jpg".
fn media_filename(
    lm: &InboundMessage,
    res: &LarkMediaResource,
    got: &DownloadedResourceStream,
    content_type: &str,
    index: usize,
) -> String {
    for candidate in [&got.filename, &res.filename] {
        if let Some(name) = clean_filename(candidate) {
            return ensure_audio_filename_extension(&name, &res.kind, content_type);
        }
    }
    let prefix = match res.kind.0.as_str() {
        "image" => "feishu-image",
        "video" => "feishu-video",
        "audio" => "feishu-audio",
        _ => "feishu-file",
    };
    let mut name = format!("{prefix}-{}", safe_path_segment(&lm.message_id));
    if index > 0 {
        name.push_str(&format!("-{}", index + 1));
    }
    format!("{name}{}", media_extension(content_type))
}

fn media_content_type(res: &LarkMediaResource, got: &DownloadedResourceStream) -> String {
    let content_type = got.content_type.trim().to_string();
    if res.kind.0 == MsgType::audio().0 && is_generic_binary_content_type(&content_type) {
        let hinted = res.mime_type.trim().to_string();
        if !is_generic_binary_content_type(&hinted) {
            return hinted;
        }
        // Feishu audio messages are Opus. Its resource endpoint can return an
        // extensionless Content-Disposition filename together with the generic
        // audio/octet-stream type, so preserve the protocol-level format here.
        return "audio/opus".to_string();
    }
    if content_type.is_empty() {
        return if res.mime_type.trim().is_empty() {
            "application/octet-stream".to_string()
        } else {
            res.mime_type.trim().to_string()
        };
    }
    content_type
}

fn is_generic_binary_content_type(content_type: &str) -> bool {
    let base = content_type.split(';').next().unwrap_or("");
    matches!(
        base.trim().to_ascii_lowercase().as_str(),
        "" | "application/octet-stream" | "audio/octet-stream"
    )
}

fn ensure_audio_filename_extension(name: &str, kind: &MsgType, content_type: &str) -> String {
    if kind.0 != MsgType::audio().0 || has_path_ext(name) {
        return name.to_string();
    }
    format!("{name}{}", media_extension(content_type))
}

/// Mirrors Go `path.Ext(name) != ""`: a dot-suffix after the final slash.
fn has_path_ext(name: &str) -> bool {
    match name.rfind('/') {
        Some(i) => name[i + 1..].contains('.'),
        None => name.contains('.'),
    }
}

fn clean_filename(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    // Backslashes are treated as separators like Go's ReplaceAll + path.Base.
    let base = name.replace('\\', "/");
    let base = base.rsplit('/').next().unwrap_or("");
    // A name made only of dots is not a filename; letting one through means
    // the object's display name is ".." wherever it is later rendered or
    // re-saved. Fall through to the generated name instead.
    if base.trim_matches('.').is_empty() || base == "/" {
        return None;
    }
    Some(base.to_string())
}

/// Pins the common types rather than consulting the host's mime database
/// (Go used mime.ExtensionsByType, which reads /etc/mime.types — absent in
/// slim containers).
fn media_extension(content_type: &str) -> &'static str {
    let base = content_type.split(';').next().unwrap_or("").trim();
    match base {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "video/mp4" => ".mp4",
        "audio/opus" => ".opus",
        "audio/ogg" => ".ogg",
        "audio/amr" => ".amr",
        "audio/mpeg" => ".mp3",
        "application/pdf" => ".pdf",
        _ => "",
    }
}

fn safe_path_segment(s: &str) -> String {
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

fn first_non_empty(values: &[&String]) -> String {
    for v in values {
        let t = v.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PanicCredentials;

    impl CredentialsResolver for PanicCredentials {
        fn decrypt_app_secret(&self, _inst: &Installation) -> anyhow::Result<String> {
            panic!("cancelled resolver must not decrypt credentials")
        }
    }

    struct PanicStorage;

    impl MediaStorage for PanicStorage {
        fn upload(
            &self,
            _key: &str,
            _data: Vec<u8>,
            _content_type: &str,
            _filename: &str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>>
        {
            panic!("cancelled resolver must not upload")
        }

        fn object_url(&self, _key: &str) -> String {
            panic!("cancelled resolver must not derive an object URL")
        }
    }

    struct PanicLedger;

    #[async_trait]
    impl MediaIntentLedger for PanicLedger {
        async fn record_pending_media_object(
            &self,
            _p: RecordPendingMediaObjectParams,
        ) -> anyhow::Result<bool> {
            panic!("cancelled resolver must not write an intent")
        }
    }

    fn lm(message_type: &str, content: &str) -> InboundMessage {
        InboundMessage {
            message_type: message_type.to_string(),
            message_id: "om_1".to_string(),
            content: content.to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn cancelled_resolve_returns_before_any_io() {
        let resolver = FeishuMediaResolver::new(
            Arc::new(crate::client::StubApiClient::new()),
            Arc::new(PanicCredentials),
            Arc::new(PanicStorage),
            Arc::new(PanicLedger),
        );
        let ctx = CancellationToken::new();
        ctx.cancel();
        let msg = ChannelMessage {
            message_id: "om_cancelled".to_string(),
            ..Default::default()
        };
        let got = resolver
            .resolve_media(
                ctx,
                &ResolvedInstallation::default(),
                &ResolvedIdentity {
                    user_id: Uuid::nil(),
                },
                Uuid::nil(),
                Uuid::nil(),
                msg,
            )
            .await;
        assert_eq!(got.message_id, "om_cancelled");
    }

    #[test]
    fn standalone_media_types_map_onto_resources() {
        let m = lm("image", r#"{"image_key":"img_v2"}"#);
        let rs = media_resources_from_message(&m);
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].key, "img_v2");
        assert_eq!(rs[0].fetch_type, "image");
        assert_eq!(rs[0].kind, MsgType::image());

        let m = lm("video", r#"{"file_key":"f1","size":99}"#);
        let rs = media_resources_from_message(&m);
        assert_eq!(rs[0].fetch_type, "file");
        assert_eq!(rs[0].kind, MsgType::video());
        assert_eq!(rs[0].size_bytes, 99);

        let m = lm("audio", r#"{"file_key":"a1","mime_type":"audio/ogg"}"#);
        let rs = media_resources_from_message(&m);
        assert_eq!(rs[0].kind, MsgType::audio());

        // Missing keys → no media promised.
        assert!(media_resources_from_message(&lm("image", "{}")).is_empty());
        assert!(media_resources_from_message(&lm("file", "{}")).is_empty());
        // Unknown / garbage → none.
        assert!(media_resources_from_message(&lm("text", "{}")).is_empty());
        assert!(media_resources_from_message(&lm("image", "not json")).is_empty());
        assert!(media_resources_from_message(&lm("image", "")).is_empty());
    }

    #[test]
    fn post_spans_are_deduped_by_kind_and_key() {
        let content = serde_json::json!({
            "title": "",
            "content": [[{"tag": "img", "image_key": "k1"}, {"tag": "text", "text": "hi"}],
                        [{"tag": "img", "image_key": "k1"}, {"tag": "media", "file_key": "v1"},
                         {"tag": "media", "file_key": "v1"}]]
        })
        .to_string();
        let rs = media_resources_from_post(&lm("post", &content));
        assert_eq!(rs.len(), 2, "duplicate spans collapse");
        assert_eq!(rs[0].key, "k1");
        assert_eq!(rs[1].key, "v1");
    }

    #[test]
    fn filenames_prefer_real_names_then_generated_with_index() {
        let mk = |msg_id: &str| InboundMessage {
            message_id: msg_id.to_string(),
            ..Default::default()
        };
        let res = LarkMediaResource {
            key: "k".into(),
            kind: MsgType::image(),
            fetch_type: "image".into(),
            filename: "".into(),
            mime_type: "".into(),
            size_bytes: 0,
            message_id: "om_1".into(),
        };
        let got = DownloadedResourceStream {
            body: Box::new(std::io::Cursor::new(Vec::new())),
            content_type: String::new(),
            filename: "".into(),
            size_bytes: 0,
        };
        let lm = mk("om_AB-c");
        // Generated: prefix + safe segment (+ index>0) + extension.
        assert_eq!(
            media_filename(&lm, &res, &got, "image/jpeg", 0),
            "feishu-image-om_AB-c.jpg"
        );
        assert_eq!(
            media_filename(&lm, &res, &got, "image/jpeg", 2),
            "feishu-image-om_AB-c-3.jpg"
        );
        // Unsafe characters collapse to underscores then trim.
        let lm = mk("!!!");
        assert_eq!(
            media_filename(&lm, &res, &got, "", 0),
            "feishu-image-unknown"
        );

        // Resource-supplied name wins over the generated form.
        let named = LarkMediaResource {
            filename: " report final.pdf ".into(),
            ..res
        };
        assert_eq!(
            media_filename(&lm, &named, &got, "application/pdf", 4),
            "report final.pdf"
        );
    }

    #[test]
    fn audio_gets_extension_when_missing_and_content_type_fallbacks() {
        assert_eq!(
            ensure_audio_filename_extension("voice", &MsgType::audio(), "audio/opus"),
            "voice.opus"
        );
        assert_eq!(
            ensure_audio_filename_extension("voice.opus", &MsgType::audio(), "audio/opus"),
            "voice.opus"
        );
        assert_eq!(
            ensure_audio_filename_extension("photo", &MsgType::image(), "image/png"),
            "photo"
        );

        let got = |ct: &str| DownloadedResourceStream {
            body: Box::new(std::io::Cursor::new(Vec::new())),
            content_type: ct.to_string(),
            filename: String::new(),
            size_bytes: 0,
        };
        let audio_hinted = LarkMediaResource {
            kind: MsgType::audio(),
            mime_type: "audio/ogg".into(),
            ..Default::default()
        };
        // Generic response type + usable hint → hint.
        assert_eq!(
            media_content_type(&audio_hinted, &got("application/octet-stream")),
            "audio/ogg"
        );
        // Generic everywhere → Opus protocol default.
        let audio_plain = LarkMediaResource {
            kind: MsgType::audio(),
            ..Default::default()
        };
        assert_eq!(
            media_content_type(&audio_plain, &got("; charset=binary")),
            "audio/opus"
        );
        // Non-audio generic → octet-stream.
        let file_res = LarkMediaResource {
            kind: MsgType::file(),
            ..Default::default()
        };
        assert_eq!(
            media_content_type(&file_res, &got("")),
            "application/octet-stream"
        );
        assert!(is_generic_binary_content_type("Audio/octet-stream"));
        assert!(!is_generic_binary_content_type("image/png"));
    }

    #[test]
    fn clean_filename_rejects_paths_dots_and_blank() {
        assert_eq!(clean_filename(" a b.png ").as_deref(), Some("a b.png"));
        assert_eq!(clean_filename(r"C:\tmp\x.png").as_deref(), Some("x.png"));
        assert_eq!(clean_filename(".."), None);
        assert_eq!(clean_filename("..."), None);
        assert_eq!(clean_filename("/"), None);
        assert_eq!(clean_filename("   "), None);
    }

    #[test]
    fn extensions_pin_common_types() {
        assert_eq!(media_extension("image/jpeg"), ".jpg");
        assert_eq!(media_extension("image/jpeg; charset=binary"), ".jpg");
        assert_eq!(media_extension("audio/opus"), ".opus");
        assert_eq!(media_extension("application/pdf"), ".pdf");
        assert_eq!(media_extension("weird/type"), "");
    }

    #[test]
    fn object_keys_hash_chat_message_and_resource() {
        let inst = ResolvedInstallation {
            id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            ..Default::default()
        };
        let res = LarkMediaResource {
            key: "K".into(),
            kind: MsgType::image(),
            fetch_type: "image".into(),
            filename: "".into(),
            mime_type: "".into(),
            size_bytes: 0,
            message_id: "om_9".into(),
        };
        let a = media_object_key(&inst, Uuid::now_v7(), &res);
        let b = media_object_key(&inst, Uuid::now_v7(), &res);
        assert_ne!(a, b);
        assert!(
            a.starts_with(&format!("workspaces/{}/lark/", inst.workspace_id)),
            "{a}"
        );
        assert_eq!(a.split('/').count(), 5);
    }

    #[test]
    fn safe_segment_reduces_and_trims() {
        assert_eq!(safe_path_segment("om_1-ab"), "om_1-ab");
        assert_eq!(safe_path_segment("  "), "unknown");
        assert_eq!(safe_path_segment("***"), "unknown");
        assert_eq!(safe_path_segment("a b/c"), "a_b_c");
    }

    #[test]
    fn first_non_empty_trims_and_skips_blanks() {
        assert_eq!(first_non_empty(&[&String::new(), &" x ".to_string()]), "x");
        assert_eq!(first_non_empty(&[&String::new()]), "");
    }
}
