//! Putting a file INTO a WeCom chat — port of `media_upload.go`.
//!
//! The bot has no REST endpoint for this: media goes up the same WebSocket
//! everything else uses, in three cmds and no access_token
//! (<https://developer.work.weixin.qq.com/document/path/101463>).
//!
//! - aibot_upload_media_init   → declares the file, hands back an upload_id
//! - aibot_upload_media_chunk  → one slice of the bytes, base64'd, × N
//! - aibot_upload_media_finish → hands back the media_id a message can carry
//!
//! Every one of the three says something beyond "accepted", so all three go
//! through the sender's request and read the verdict that comes back.
//!
//! Two properties of the middle step are the whole design here: chunks may go
//! out in any order and concurrently, and re-sending one is idempotent. So
//! the chunks are uploaded a few at a time rather than in lockstep, and a
//! chunk whose ack never comes back is simply sent again — losing one slice
//! of a hundred must not cost the file.
//!
//! What it does NOT do is put a picture inside a reply. The long connection
//! has no msg_item, so a file is always its own message; an answer with an
//! attachment is the answer, then the file.

use base64::Engine as _;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::ws_frame::{CHAT_TYPE_GROUP_INT, CHAT_TYPE_SINGLE_INT, CMD_SEND_MSG};
use crate::ws_sender::{is_ack_timeout, WsSender};

// The three upload cmds.
pub const CMD_UPLOAD_MEDIA_INIT: &str = "aibot_upload_media_init";
pub const CMD_UPLOAD_MEDIA_CHUNK: &str = "aibot_upload_media_chunk";
pub const CMD_UPLOAD_MEDIA_FINISH: &str = "aibot_upload_media_finish";

/// What WeCom will call this file once it is a message. The four values are
/// the protocol's own; nothing else is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaMsgType {
    File,
    Image,
    Voice,
    Video,
}

impl MediaMsgType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Image => "image",
            Self::Voice => "voice",
            Self::Video => "video",
        }
    }

    #[allow(dead_code)] // wire-symmetry inverse of as_str; used by tests of inbound kinds
    fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "file" => Self::File,
            "image" => Self::Image,
            "voice" => Self::Voice,
            "video" => Self::Video,
            _ => return None,
        })
    }
}

impl std::fmt::Display for MediaMsgType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The cap on one chunk BEFORE base64, so a frame on the wire is about a third
/// larger again. [`MAX_MEDIA_CHUNKS`] is the protocol's own ceiling on how
/// many there may be, and the two together are the largest file this TRANSPORT
/// could express — 50 MB. Both are WeCom's own SDK numbers.
pub const MEDIA_CHUNK_BYTES: usize = 512 * 1024;
pub const MAX_MEDIA_CHUNKS: usize = 100;
pub const MAX_MEDIA_TRANSPORT_BYTES: usize = MEDIA_CHUNK_BYTES * MAX_MEDIA_CHUNKS;

/// The largest file we will actually send, and it is a third of what the
/// transport could express.
///
/// The 50 MB the chunk arithmetic allows is the size the FRAMING can describe,
/// not a size the platform accepts. WeCom documents the accepted sizes on the
/// init cmd itself: image at most 10MB, voice 2MB, video 10MB, plain file
/// 20MB. 20 MB for a plain file is the widest any kind may be.
///
/// The per-kind ceilings are applied a layer up in
/// [`crate::outbound_media::wecom_media_kind`], which demotes an oversize
/// image to a file rather than offering the server something it will refuse.
///
/// Chunking a 40 MB file to be refused at the end costs eighty round trips,
/// the whole thing resident in memory, and a user who waits minutes to be told
/// no.
pub const MAX_MEDIA_UPLOAD_BYTES: usize = 20 << 20;

/// The byte caps on a video message's two required fields.
pub const VIDEO_TITLE_BYTES: usize = 64;
pub const VIDEO_DESCRIPTION_BYTES: usize = 512;

/// How many chunks may be in flight at once, and it falls as the file grows.
///
/// The ladder is WeCom's own, from the SDK's uploadMedia: `totalChunks <= 4 ?
/// totalChunks : totalChunks <= 10 ? 3 : 2`. The reason it steps down is past
/// ten chunks the WeCom backend answers a burst of concurrent chunks with a
/// system error — a server-side limit, so no amount of retrying gets around it
/// and a fixed 4 simply fails on exactly the large uploads that most need to
/// succeed.
///
/// The small end matters too, and in the other direction: at four chunks or
/// fewer every chunk goes at once, so a 2 MB file is not serialized for a
/// caution that only applies to big ones.
///
/// A second reason to stay low: the socket is shared with every other chat
/// the bot is in, and a file at full tilt stalls their replies.
pub fn media_chunk_parallelism(chunks: usize) -> usize {
    match chunks {
        0 => 1,
        n @ 1..=4 => n,
        n if n <= 10 => 3,
        _ => 2,
    }
}

/// How many times one chunk is offered. The second try exists for a lost ack
/// and nothing else: a chunk the server refuses is refused again, so a
/// rejection ends the upload on the spot.
const MEDIA_CHUNK_ATTEMPTS: usize = 2;

/// Past the size WeCom accepts. Refused before the first byte is read rather
/// than after eighty round trips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("wecom: media exceeds the {limit} byte upload limit")]
pub struct MediaUploadTooLarge {
    pub limit: usize,
}

impl MediaUploadTooLarge {
    pub fn err() -> anyhow::Error {
        anyhow::Error::new(MediaUploadTooLarge {
            limit: MAX_MEDIA_UPLOAD_BYTES,
        })
    }
}

/// A zero-byte file. WeCom has nothing to store and the user has nothing to
/// open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("wecom: media has no bytes to upload")]
pub struct MediaUploadEmpty;

/// One file on its way to a chat.
#[derive(Debug, Clone)]
pub struct OutboundMedia {
    /// Kind decides which msgtype the finished message uses, and WeCom
    /// validates the bytes against it — a .pptx declared as an image is
    /// refused, not converted.
    pub kind: MediaMsgType,

    /// What the recipient sees on the file card. It is also the only hint the
    /// server gets about the format, so it keeps its extension.
    pub filename: String,

    pub data: Vec<u8>,
}

impl OutboundMedia {
    pub fn validate(&self) -> anyhow::Result<()> {
        // The enum closes the type set, mirroring Go's switch default.
        if self.filename.trim().is_empty() {
            anyhow::bail!("wecom: media upload requires a filename");
        }
        Ok(())
    }
}

/// A finished upload addressed as a message.
#[derive(Debug, Clone)]
pub struct MediaSend {
    pub kind: MediaMsgType,
    pub media_id: String,

    /// Title and Description are video's alone — the other three kinds have no
    /// field for them and reject a body that carries one.
    pub title: String,
    pub description: String,
}

/// Cuts a file into the slices the protocol will take. The slices alias the
/// input; nothing is copied until a chunk is base64'd for its own frame.
pub fn split_media_chunks(data: &[u8]) -> anyhow::Result<Vec<&[u8]>> {
    if data.is_empty() {
        return Err(anyhow::Error::new(MediaUploadEmpty));
    }
    if data.len() > MAX_MEDIA_UPLOAD_BYTES {
        return Err(MediaUploadTooLarge::err());
    }
    let mut chunks = Vec::with_capacity(data.len().div_ceil(MEDIA_CHUNK_BYTES));
    let mut start = 0usize;
    while start < data.len() {
        let end = (start + MEDIA_CHUNK_BYTES).min(data.len());
        chunks.push(&data[start..end]);
        start = end;
    }
    Ok(chunks)
}

/// Carries one file through the three steps and returns the media_id a
/// message can be built around. `ctx` bounds the whole thing.
pub async fn upload_media(
    sender: &WsSender,
    ctx: &CancellationToken,
    m: &OutboundMedia,
) -> anyhow::Result<String> {
    m.validate()?;
    let chunks = split_media_chunks(&m.data)?;
    let upload_id = upload_media_init(sender, ctx, m, chunks.len()).await?;
    upload_media_chunks(sender, ctx, &upload_id, &chunks).await?;
    upload_media_finish(sender, ctx, &upload_id).await
}

/// Declares the file and takes the upload_id back.
///
/// The optional md5 field is deliberately not sent. The document lists it
/// without saying what it is taken over — the raw file or the base64 of it —
/// and a server that checks a value we guessed wrong would refuse every
/// upload with an errcode that says nothing about why.
async fn upload_media_init(
    sender: &WsSender,
    ctx: &CancellationToken,
    m: &OutboundMedia,
    chunks: usize,
) -> anyhow::Result<String> {
    let body = sender
        .request(
            ctx,
            CMD_UPLOAD_MEDIA_INIT,
            json!({
                "type": m.kind.as_str(),
                "filename": m.filename,
                "total_size": m.data.len(),
                "total_chunks": chunks,
            }),
        )
        .await?;
    let upload_id = body
        .get("upload_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if upload_id.is_empty() {
        anyhow::bail!("wecom: upload init returned no upload_id");
    }
    Ok(upload_id)
}

/// Sends every slice, a few at a time. The first failure cancels the rest:
/// there is no partial upload worth finishing.
async fn upload_media_chunks(
    sender: &WsSender,
    ctx: &CancellationToken,
    upload_id: &str,
    chunks: &[&[u8]],
) -> anyhow::Result<()> {
    use futures_util::StreamExt;

    let limit = media_chunk_parallelism(chunks.len());
    let cancel = ctx.child_token();
    let cancel_for_stream = cancel.clone();
    // Chunks are owned copies: a borrowed slice inside the buffered stream's
    // closures trips rustc's higher-ranked Send elaboration when this future is
    // spawned, and the copy is bounded by the chunk size anyway.
    let owned: Vec<(usize, Vec<u8>)> = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| (i, c.to_vec()))
        .collect();
    let mut results = futures_util::stream::iter(owned.into_iter().map(move |(i, chunk)| {
        let chunk_cancel = cancel_for_stream.clone();
        async move { upload_media_chunk(sender, &chunk_cancel, upload_id, i, &chunk).await }
    }))
    .buffer_unordered(limit);

    let mut first_err: Option<anyhow::Error> = None;
    while let Some(res) = results.next().await {
        if let Err(e) = res {
            if first_err.is_none() {
                first_err = Some(e);
                cancel.cancel();
            }
        }
    }
    match first_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Sends one slice, once more if its verdict never arrived.
///
/// chunk_index goes on the wire as a number. The field table in the document
/// calls it a string and the worked example beside it passes a number; WeCom's
/// own SDK sends a number, so that is what the server is known to accept.
async fn upload_media_chunk(
    sender: &WsSender,
    ctx: &CancellationToken,
    upload_id: &str,
    index: usize,
    chunk: &[u8],
) -> anyhow::Result<()> {
    let body = json!({
        "upload_id": upload_id,
        "chunk_index": index,
        "base64_data": base64::engine::general_purpose::STANDARD.encode(chunk),
    });
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..MEDIA_CHUNK_ATTEMPTS {
        match sender
            .request(ctx, CMD_UPLOAD_MEDIA_CHUNK, body.clone())
            .await
        {
            Ok(_) => return Ok(()),
            Err(e) => {
                // A refusal is the server's answer and will be its answer
                // again. Only a verdict that never came back is worth a second
                // offer, and the protocol's own idempotence is what makes that
                // safe.
                if !is_ack_timeout(&e) {
                    last_err = Some(e);
                    break;
                }
                tracing::warn!(
                    upload_id = %upload_id,
                    chunk_index = index,
                    attempt = attempt + 1,
                    "wecom: media chunk got no verdict, sending it again"
                );
                last_err = Some(e);
            }
        }
    }
    match last_err {
        Some(e) => {
            // The headlined verdict keeps the errcode greppable; the typed
            // cause stays on the chain for is_ack_timeout classification.
            let verdict = e
                .downcast_ref::<crate::ws_sender::WecomApiError>()
                .map(|api| format!("server refused {}: {}", api.code, api.msg))
                .unwrap_or_else(|| e.to_string());
            Err(e.context(format!("wecom: upload chunk {index}: {verdict}")))
        }
        None => Err(anyhow::anyhow!(
            "wecom: upload chunk {index}: no attempts made"
        )),
    }
}

/// Seals the upload and takes the media_id back.
async fn upload_media_finish(
    sender: &WsSender,
    ctx: &CancellationToken,
    upload_id: &str,
) -> anyhow::Result<String> {
    let body = sender
        .request(
            ctx,
            CMD_UPLOAD_MEDIA_FINISH,
            json!({ "upload_id": upload_id }),
        )
        .await?;
    let media_id = body
        .get("media_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if media_id.is_empty() {
        anyhow::bail!("wecom: upload finish returned no media_id");
    }
    Ok(media_id)
}

/// Delivers an uploaded file as a message, on the same aibot_send_msg push
/// that carries text.
///
/// The documentation leaves two routes open and contradicts itself about
/// them. aibot_send_msg's field table lists only template_card and markdown,
/// while aibot_respond_msg's msgtype table does name file, image, voice and
/// video. The push wins anyway, for two reasons. WeCom's own Node SDK pushes
/// media on aibot_send_msg, which is the one piece of evidence either way that
/// comes from working code. And aibot_respond_msg is addressed by the req_id
/// of the callback that opened the turn, which this delivery path does not
/// hold: the answer has already gone out as its own push, so there is no live
/// turn left to respond to.
///
/// Which of the two a real bot accepts has not been observed. A refusal is
/// returned as it stands rather than retried on the other route — the caller
/// tells the user the file did not make it, which is honest, where a second
/// attempt on an address we do not have would not be.
pub async fn send_media(
    sender: &WsSender,
    ctx: &CancellationToken,
    chat_id: &str,
    chat_type: i64,
    m: &MediaSend,
) -> anyhow::Result<()> {
    let body = send_msg_media_body(chat_id, chat_type, m)?;
    if let Err(e) = sender.request(ctx, CMD_SEND_MSG, body).await {
        // A verdict that never came is not a refusal. The frame went out, and
        // the read loop can simply have been busy. Resending on that would put
        // the SAME media_id out a second time and the person sees the picture
        // twice with nothing to undo — so it is reported as-is, and said
        // distinctly in the log because "may already have arrived" is a
        // different thing to chase than "was refused".
        if is_ack_timeout(&e) {
            tracing::warn!(
                media_id = %m.media_id,
                msgtype = %m.kind,
                "wecom: media push not acknowledged in time; not resending"
            );
        }
        return Err(e);
    }
    tracing::info!(
        route = CMD_SEND_MSG,
        media_id = %m.media_id,
        msgtype = %m.kind,
        "wecom: media delivered"
    );
    Ok(())
}

/// Builds an aibot_send_msg body carrying media.
fn send_msg_media_body(chat_id: &str, chat_type: i64, m: &MediaSend) -> anyhow::Result<Value> {
    if chat_id.is_empty() {
        anyhow::bail!("wecom: send_msg requires chat_id");
    }
    if chat_type != CHAT_TYPE_SINGLE_INT && chat_type != CHAT_TYPE_GROUP_INT {
        anyhow::bail!("wecom: send_msg chat_type must be 1 (single) or 2 (group)");
    }
    let mut body = media_body_fields(m)?;
    body["chatid"] = json!(chat_id);
    body["chat_type"] = json!(chat_type);
    Ok(body)
}

/// The `{msgtype, <kind>:{...}}` pair a media frame carries.
fn media_body_fields(m: &MediaSend) -> anyhow::Result<Value> {
    if m.media_id.is_empty() {
        anyhow::bail!("wecom: media message requires a media_id");
    }
    let mut nested = json!({ "media_id": m.media_id });
    match m.kind {
        MediaMsgType::File | MediaMsgType::Image | MediaMsgType::Voice => {}
        MediaMsgType::Video => {
            nested["title"] = json!(clip_utf8(&m.title, VIDEO_TITLE_BYTES));
            nested["description"] = json!(clip_utf8(&m.description, VIDEO_DESCRIPTION_BYTES));
        }
    }
    let mut body = serde_json::Map::new();
    body.insert("msgtype".to_string(), json!(m.kind.as_str()));
    body.insert(m.kind.as_str().to_string(), nested);
    Ok(Value::Object(body))
}

/// Cuts a string to a byte budget on a character boundary. WeCom counts
/// bytes, a Chinese title spends three of them per character, and a cut
/// through the middle of one sends the server broken UTF-8.
pub(crate) fn clip_utf8(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s[..cut].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    #[test]
    fn parallelism_ladder_matches_the_sdk() {
        assert_eq!(media_chunk_parallelism(0), 1);
        assert_eq!(media_chunk_parallelism(1), 1);
        assert_eq!(media_chunk_parallelism(3), 3);
        assert_eq!(media_chunk_parallelism(4), 4);
        assert_eq!(media_chunk_parallelism(5), 3);
        assert_eq!(media_chunk_parallelism(10), 3);
        assert_eq!(media_chunk_parallelism(11), 2);
        assert_eq!(media_chunk_parallelism(100), 2);
    }

    #[test]
    fn split_rejects_empty_and_oversize_before_any_round_trip() {
        assert_eq!(
            split_media_chunks(&[]).unwrap_err().to_string(),
            "wecom: media has no bytes to upload"
        );
        let big = vec![0u8; MAX_MEDIA_UPLOAD_BYTES + 1];
        let e = split_media_chunks(&big).unwrap_err();
        assert!(
            e.to_string().contains(&MAX_MEDIA_UPLOAD_BYTES.to_string()),
            "{e}"
        );
    }

    #[test]
    fn split_cuts_on_exact_boundaries_with_a_short_tail() {
        let data = vec![7u8; MEDIA_CHUNK_BYTES * 2 + 11];
        let chunks = split_media_chunks(&data).unwrap();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), MEDIA_CHUNK_BYTES);
        assert_eq!(chunks[1].len(), MEDIA_CHUNK_BYTES);
        assert_eq!(chunks[2].len(), 11);
        // Slices alias the input: reassembling reproduces the bytes.
        let joined: Vec<u8> = chunks.concat();
        assert_eq!(joined, data);
    }

    #[test]
    fn exactly_one_chunk_boundary_file_makes_one_chunk() {
        let data = vec![1u8; MEDIA_CHUNK_BYTES];
        let chunks = split_media_chunks(&data).unwrap();
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn validate_requires_a_filename() {
        let ok = OutboundMedia {
            kind: MediaMsgType::File,
            filename: "a.pdf".to_string(),
            data: vec![1],
        };
        assert!(ok.validate().is_ok());
        let bad = OutboundMedia {
            kind: MediaMsgType::File,
            filename: "   ".to_string(),
            data: vec![1],
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn media_msg_type_wire_values() {
        assert_eq!(MediaMsgType::File.as_str(), "file");
        assert_eq!(MediaMsgType::Image.as_str(), "image");
        assert_eq!(MediaMsgType::Voice.as_str(), "voice");
        assert_eq!(MediaMsgType::Video.as_str(), "video");
        assert_eq!(MediaMsgType::from_wire("video"), Some(MediaMsgType::Video));
        assert_eq!(MediaMsgType::from_wire("sticker"), None);
    }

    #[test]
    fn media_body_fields_shape_per_kind() {
        let file = MediaSend {
            kind: MediaMsgType::File,
            media_id: "M1".to_string(),
            title: "t".to_string(),
            description: "d".to_string(),
        };
        let v = media_body_fields(&file).unwrap();
        assert_eq!(v["msgtype"], json!("file"));
        assert_eq!(v["file"]["media_id"], json!("M1"));
        // Title/description are video-only: absent for the other kinds.
        assert!(v["file"].get("title").is_none());

        let video = MediaSend {
            kind: MediaMsgType::Video,
            media_id: "V1".to_string(),
            title: "标题".to_string(),
            description: "描述".to_string(),
        };
        let v = media_body_fields(&video).unwrap();
        assert_eq!(v["msgtype"], json!("video"));
        assert_eq!(v["video"]["title"], json!("标题"));
        assert_eq!(v["video"]["description"], json!("描述"));

        let no_id = MediaSend {
            kind: MediaMsgType::Image,
            media_id: String::new(),
            title: String::new(),
            description: String::new(),
        };
        assert!(media_body_fields(&no_id).is_err());
    }

    #[test]
    fn send_msg_media_body_validates_addressing() {
        let m = MediaSend {
            kind: MediaMsgType::Image,
            media_id: "M".to_string(),
            title: String::new(),
            description: String::new(),
        };
        assert!(send_msg_media_body("", 1, &m).is_err());
        assert!(send_msg_media_body("c", 5, &m).is_err());
        let v = send_msg_media_body("c", CHAT_TYPE_SINGLE_INT, &m).unwrap();
        assert_eq!(v["chatid"], json!("c"));
        assert_eq!(v["chat_type"], json!(1));
        assert_eq!(v["msgtype"], json!("image"));
    }

    #[test]
    fn clip_cuts_on_character_boundaries_not_mid_rune() {
        // Each CJK char is 3 bytes; budget 7 must cut at 6, not 7.
        assert_eq!(clip_utf8("中文中文", 7), "中文");
        assert_eq!(clip_utf8("abc", 10), "abc");
        assert_eq!(clip_utf8("", 5), "");
        assert_eq!(clip_utf8("abcdef", 3), "abc");
        // Budget smaller than one rune clips to empty rather than panicking.
        assert_eq!(clip_utf8("中", 2), "");
    }

    #[tokio::test]
    async fn chunk_refusal_ends_the_upload_without_a_retry() {
        // A scripted conn that echoes every written frame's req_id back with a
        // refusal verdict proves the non-ack-timeout branch breaks out after
        // ONE attempt.
        use std::sync::Mutex;

        struct RefusingConn {
            last_req_id: Mutex<Option<String>>,
            /// The req_id the router task already answered, so each frame gets
            /// exactly one verdict.
            routed: Mutex<String>,
            writes: std::sync::atomic::AtomicUsize,
        }
        impl RefusingConn {
            fn new() -> Self {
                Self {
                    last_req_id: Mutex::new(None),
                    routed: Mutex::new(String::new()),
                    writes: std::sync::atomic::AtomicUsize::new(0),
                }
            }
        }
        #[async_trait]
        impl crate::ws_sender::WsConn for RefusingConn {
            async fn read_message(
                &self,
                _deadline: Option<std::time::Instant>,
            ) -> anyhow::Result<Vec<u8>> {
                let req_id = self
                    .last_req_id
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone()
                    .unwrap_or_default();
                Ok(serde_json::to_vec(&serde_json::json!({
                    "headers": {"req_id": req_id},
                    "errcode": 45009,
                    "errmsg": "api freq limited",
                }))
                .unwrap())
            }
            async fn write_message(
                &self,
                data: String,
                _: Option<std::time::Instant>,
            ) -> anyhow::Result<()> {
                self.writes
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let v: Value = serde_json::from_str(&data).unwrap_or_default();
                *self.last_req_id.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(v["headers"]["req_id"].as_str().unwrap_or("").to_string());
                Ok(())
            }
            async fn close(&self) {}
        }

        let conn = std::sync::Arc::new(RefusingConn::new());
        let sender = std::sync::Arc::new(WsSender::new(conn.clone()));
        // Stand-in for the channel's read loop: every frame the upload writes
        // gets its (refusing) verdict routed back, exactly as
        // dispatch_frame's anonymous-ack arm does on the live connection.
        let route_conn = conn.clone();
        let route_sender = sender.clone();
        tokio::spawn(async move {
            loop {
                let req_id = route_conn
                    .last_req_id
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                let Some(req_id) = req_id else { continue };
                if req_id == *route_conn.routed.lock().unwrap_or_else(|e| e.into_inner()) {
                    continue;
                }
                *route_conn.routed.lock().unwrap_or_else(|e| e.into_inner()) = req_id.clone();
                let _ = route_sender.route_response(&crate::ws_frame::FrameEnvelope {
                    headers: crate::ws_frame::FrameHeaders { req_id },
                    err_code: 45009,
                    err_msg: "api freq limited".to_string(),
                    ..Default::default()
                });
            }
        });
        let err = upload_media_chunk(
            &sender,
            &CancellationToken::new(),
            "up-1",
            3,
            b"chunk-bytes",
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("upload chunk 3"), "{err}");
        assert!(err.to_string().contains("45009"), "{err}");
        // Exactly one attempt: a refusal is the server's answer and will be
        // its answer again.
        assert_eq!(conn.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
