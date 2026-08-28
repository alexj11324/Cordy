//! Telegram inbound attachment resolution.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use patchbay_channel::{InboundMessage, MediaRef, MsgType};
use patchbay_channel_engine::resolvers::{
    MediaIntentLedger, MediaResolver, RecordPendingMediaObjectParams, ResolvedIdentity,
    ResolvedInstallation,
};

use crate::inbound::{TelegramMediaRef, TelegramRawEvent};
use crate::replier::installation_api;
use crate::DecrypterFn;

pub const MAX_MEDIA_BYTES: usize = 20 << 20;
pub const MAX_MEDIA_PER_MESSAGE: usize = 4;
pub const MEDIA_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

pub trait MediaStorage: Send + Sync {
    fn upload(
        &self,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>>;
    fn object_url(&self, key: &str) -> String;
}

pub struct TelegramMediaResolver {
    decrypt: Option<Arc<DecrypterFn>>,
    api_base: String,
    storage: Arc<dyn MediaStorage>,
    ledger: Arc<dyn MediaIntentLedger>,
    http: reqwest::Client,
}

impl TelegramMediaResolver {
    pub fn new(
        decrypt: Option<Arc<DecrypterFn>>,
        api_base: String,
        storage: Arc<dyn MediaStorage>,
        ledger: Arc<dyn MediaIntentLedger>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            decrypt,
            api_base,
            storage,
            ledger,
            http: guarded_client()?,
        })
    }

    async fn ingest(
        &self,
        ctx: &CancellationToken,
        inst: &ResolvedInstallation,
        chat_message_id: Uuid,
        source_message_id: &str,
        index: usize,
        media: &TelegramMediaRef,
    ) -> anyhow::Result<MediaRef> {
        if media.file_id.is_empty() {
            anyhow::bail!("telegram: attachment has no file_id");
        }
        if media.file_size < 0 || media.file_size as usize > MAX_MEDIA_BYTES {
            anyhow::bail!("telegram: attachment exceeds size limit");
        }
        let key = object_key(inst, chat_message_id, source_message_id, index, media);
        let storage_url = self.storage.object_url(&key);
        let owned = self
            .ledger
            .record_pending_media_object(RecordPendingMediaObjectParams {
                storage_key: key.clone(),
                workspace_id: inst.workspace_id,
                chat_message_id,
                storage_url: storage_url.clone(),
                installation_id: inst.id,
            })
            .await?;
        if !owned {
            anyhow::bail!("telegram: media key is owned by reconciler");
        }

        let api = installation_api(inst, self.decrypt.as_deref(), &self.api_base)?;
        let file = tokio::select! {
            _ = ctx.cancelled() => anyhow::bail!("telegram: media resolution cancelled"),
            result = api.get_file(&media.file_id) => result?,
        };
        let size_hint = [media.file_size, file.file_size]
            .into_iter()
            .filter(|size| *size > 0)
            .max()
            .unwrap_or(0);
        if size_hint as usize > MAX_MEDIA_BYTES {
            anyhow::bail!("telegram: attachment exceeds size limit");
        }
        let url = api.file_url(&file)?;
        validate_url(&url)?;
        let (path, size, response_type) = self.download_to_spool(ctx, url, size_hint).await?;
        let _cleanup = TempCleanup(path.clone());
        let data = tokio::select! {
            _ = ctx.cancelled() => anyhow::bail!("telegram: media resolution cancelled"),
            result = tokio::fs::read(&path) => result?,
        };
        if data.len() != size as usize || data.len() > MAX_MEDIA_BYTES {
            anyhow::bail!("telegram: spooled attachment size mismatch");
        }
        let content_type = clean_content_type(&media.mime_type)
            .or_else(|| clean_content_type(&response_type))
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let filename = clean_filename(&media.filename)
            .unwrap_or_else(|| generated_filename(media, index, &content_type));
        tokio::select! {
            _ = ctx.cancelled() => anyhow::bail!("telegram: media resolution cancelled"),
            result = self.storage.upload(&key, data, &content_type, &filename) => result?,
        }
        Ok(MediaRef {
            r#type: MsgType(media.kind.clone()),
            storage_key: key,
            storage_url,
            filename,
            mime_type: content_type,
            size_bytes: size,
            ..Default::default()
        })
    }

    async fn download_to_spool(
        &self,
        ctx: &CancellationToken,
        url: url::Url,
        size_hint: i64,
    ) -> anyhow::Result<(PathBuf, i64, String)> {
        let operation = async {
            let mut response = self
                .http
                .get(url)
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("telegram: media download request failed"))?;
            if !response.status().is_success() {
                anyhow::bail!("telegram: media download returned non-success status");
            }
            if response
                .content_length()
                .is_some_and(|size| size > MAX_MEDIA_BYTES as u64)
                || size_hint > MAX_MEDIA_BYTES as i64
            {
                anyhow::bail!("telegram: attachment exceeds size limit");
            }
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let path =
                std::env::temp_dir().join(format!("patchbay-telegram-media-{}", Uuid::now_v7()));
            let mut output = tokio::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .await?;
            let cleanup = TempCleanup(path.clone());
            let mut total = 0usize;
            while let Some(chunk) = response.chunk().await? {
                total = total
                    .checked_add(chunk.len())
                    .ok_or_else(|| anyhow::anyhow!("telegram: attachment size overflow"))?;
                if total > MAX_MEDIA_BYTES {
                    anyhow::bail!("telegram: attachment exceeds size limit");
                }
                output.write_all(&chunk).await?;
            }
            output.flush().await?;
            drop(output);
            std::mem::forget(cleanup);
            Ok((path, total as i64, content_type))
        };
        tokio::select! {
            _ = ctx.cancelled() => anyhow::bail!("telegram: media resolution cancelled"),
            result = tokio::time::timeout(MEDIA_FETCH_TIMEOUT, operation) => {
                result.map_err(|_| anyhow::anyhow!("telegram: media download timed out"))?
            }
        }
    }
}

#[async_trait]
impl MediaResolver for TelegramMediaResolver {
    fn has_media(&self, msg: &InboundMessage) -> bool {
        decode_raw(msg).is_ok_and(|raw| raw.media.iter().any(|media| !media.file_id.is_empty()))
    }

    async fn resolve_media(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        _sender: &ResolvedIdentity,
        _session_id: Uuid,
        chat_message_id: Uuid,
        mut msg: InboundMessage,
    ) -> InboundMessage {
        let raw = match decode_raw(&msg) {
            Ok(raw) => raw,
            Err(error) => {
                tracing::warn!(%error, message_id = %msg.message_id, "telegram media raw decode failed");
                return msg;
            }
        };
        for (index, media) in raw.media.iter().take(MAX_MEDIA_PER_MESSAGE).enumerate() {
            if ctx.is_cancelled() {
                break;
            }
            match self
                .ingest(&ctx, inst, chat_message_id, &msg.message_id, index, media)
                .await
            {
                Ok(reference) => msg.media_refs.push(reference),
                Err(error) => tracing::warn!(
                    %error,
                    message_id = %msg.message_id,
                    media_kind = %media.kind,
                    "telegram media resolve skipped"
                ),
            }
        }
        msg
    }
}

fn decode_raw(msg: &InboundMessage) -> anyhow::Result<TelegramRawEvent> {
    serde_json::from_value(msg.raw.clone()).map_err(anyhow::Error::from)
}

fn object_key(
    inst: &ResolvedInstallation,
    chat_message_id: Uuid,
    message_id: &str,
    index: usize,
    media: &TelegramMediaRef,
) -> String {
    let digest = Sha256::digest(format!(
        "{chat_message_id}\0{message_id}\0{index}\0{}",
        media.file_id
    ));
    format!(
        "workspaces/{}/telegram/{}/{}",
        inst.workspace_id,
        inst.id,
        hex::encode(digest)
    )
}

fn clean_filename(raw: &str) -> Option<String> {
    let value = raw.rsplit(['/', '\\']).next()?.trim();
    (!value.is_empty() && value != "." && value != "..").then(|| value.chars().take(255).collect())
}

fn clean_content_type(raw: &str) -> Option<String> {
    let value = raw.split(';').next()?.trim().to_ascii_lowercase();
    (value.contains('/') && value.len() <= 127).then_some(value)
}

fn generated_filename(media: &TelegramMediaRef, index: usize, content_type: &str) -> String {
    let extension = match content_type {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "audio/ogg" => "ogg",
        "video/mp4" => "mp4",
        "application/pdf" => "pdf",
        _ => "bin",
    };
    format!("telegram-{}-{}.{}", media.kind, index + 1, extension)
}

struct TempCleanup(PathBuf);

impl Drop for TempCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Clone, Copy)]
struct PublicResolver;

impl reqwest::dns::Resolve for PublicResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addresses: Vec<_> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|_| "telegram: media DNS resolution failed")?
                .collect();
            if addresses.is_empty() || addresses.iter().any(|addr| !is_public(addr.ip())) {
                return Err("telegram: media destination is not public".into());
            }
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

fn guarded_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .dns_resolver(Arc::new(PublicResolver))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(anyhow::Error::from)
}

fn validate_url(url: &url::Url) -> anyhow::Result<()> {
    if url.scheme() != "https"
        || url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        anyhow::bail!("telegram: invalid media endpoint");
    }
    if let Some(host) = url.host_str() {
        let literal = host.trim_start_matches('[').trim_end_matches(']');
        if literal.parse::<IpAddr>().is_ok_and(|addr| !is_public(addr)) {
            anyhow::bail!("telegram: media destination is not public");
        }
    }
    Ok(())
}

fn is_public(addr: IpAddr) -> bool {
    match addr.to_canonical() {
        IpAddr::V4(addr) => {
            !(addr.is_unspecified()
                || addr.is_broadcast()
                || addr.is_loopback()
                || addr.is_multicast()
                || addr.is_link_local()
                || addr.is_private()
                || in_v4(addr, Ipv4Addr::new(100, 64, 0, 0), 10)
                || in_v4(addr, Ipv4Addr::new(192, 0, 0, 0), 24)
                || in_v4(addr, Ipv4Addr::new(192, 0, 2, 0), 24)
                || in_v4(addr, Ipv4Addr::new(198, 18, 0, 0), 15)
                || in_v4(addr, Ipv4Addr::new(198, 51, 100, 0), 24)
                || in_v4(addr, Ipv4Addr::new(203, 0, 113, 0), 24)
                || in_v4(addr, Ipv4Addr::new(240, 0, 0, 0), 4))
        }
        IpAddr::V6(addr) => {
            !(addr.is_unspecified()
                || addr.is_loopback()
                || addr.is_multicast()
                || addr.is_unicast_link_local()
                || addr.segments()[0] & 0xfe00 == 0xfc00
                || in_v6(addr, Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, 0, 0), 96)
                || in_v6(addr, Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0), 48)
                || in_v6(addr, Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0), 32)
                || in_v6(addr, Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16))
        }
    }
}

fn in_v4(addr: Ipv4Addr, base: Ipv4Addr, bits: u8) -> bool {
    u32::from(addr) >> (32 - bits) == u32::from(base) >> (32 - bits)
}

fn in_v6(addr: Ipv6Addr, base: Ipv6Addr, bits: u8) -> bool {
    u128::from(addr) >> (128 - bits) == u128::from(base) >> (128 - bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_and_transition_destinations_are_refused() {
        assert!(!is_public(IpAddr::from([127, 0, 0, 1])));
        assert!(!is_public(IpAddr::from([10, 0, 0, 1])));
        assert!(!is_public(IpAddr::V6(Ipv6Addr::new(
            0x64, 0xff9b, 0, 0, 0, 0, 0x7f00, 1
        ))));
        assert!(is_public(IpAddr::from([8, 8, 8, 8])));
    }
}
