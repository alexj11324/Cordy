//! An opt-in record of every frame this adapter reads and writes — port of
//! `trace.go`.
//!
//! Why it exists: nothing else in the crate logs a frame. Failures log, and
//! only failures — a bad envelope, a non-zero server ack. A run that goes
//! wrong QUIETLY leaves no trace on the server at all: the reply addressed to
//! the room instead of the person, the command that was silently dropped, the
//! receipt that went out twice.
//!
//! With PATCHBAY_WECOM_TRACE=1 the server records enough to check a real-device
//! session afterwards: which way the frame went, what chat it was addressed
//! to, whether that chat is a room or a person, and what the server said
//! back. The switch also covers the one thing a frame does not carry — what
//! an attachment's own response said its name was ([`trace_media_headers`]),
//! which no later inspection can recover once the five-minute media URL has
//! lapsed.
//!
//! Why not `tracing::debug`, which the siblings use for their per-frame
//! lines: LOG_LEVEL defaults to debug, so a debug call is on in every
//! deployment that has not set LOG_LEVEL. Message text must not be logged by
//! default, so the switch has to be its own, and default to off.
//!
//! Off is the right state outside a test session. What this records includes
//! a bounded prefix of message text, because "which of the copy strings was
//! that" cannot be answered from lengths alone — so it is message content, in
//! a log, and it should be turned on deliberately and turned off after. The
//! prefix is capped rather than full, the cap counts runes so a Chinese
//! message is not cut mid-character, bearer tokens are redacted out of every
//! message string, and nothing here reads a credential field: the bot secret
//! rides in the aibot_subscribe body, which [`trace_out_fields`] never
//! descends into.
//!
//! Two strings skip the redactor, both from an attachment's
//! Content-Disposition: the header itself and the filename read out of it.
//! [`trace_media_headers`] writes them verbatim under a cap of its own, and
//! says why the redactor would destroy the only thing the line is for, and
//! why nothing in that header is a credential.

use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine as _;
use serde_json::Value;

use crate::ws_frame::{AibotMsgCallback, FrameEnvelope};

/// Set once at boot and read on every frame.
static TRACING: AtomicBool = AtomicBool::new(false);

/// Turns frame tracing on or off. Called from the server wiring with
/// PATCHBAY_WECOM_TRACE; returns what it set so the caller can log it.
pub fn set_trace(on: bool) -> bool {
    TRACING.store(on, Ordering::SeqCst);
    on
}

/// Reports whether frames are being recorded.
pub fn tracing_on() -> bool {
    TRACING.load(Ordering::SeqCst)
}

/// Bounds what of a message body reaches the log. Long enough to tell one
/// copy string from another and to see which language it is in; short enough
/// that a transcript is not reconstructable from the log.
const TRACE_PREVIEW_RUNES: usize = 120;

/// Bounds an attachment's Content-Disposition, and the name parsed out of it,
/// on their way to the trace. It is a runaway guard on a remote string, not a
/// preview cap, which is why it is not [`TRACE_PREVIEW_RUNES`].
///
/// 120 would defeat the line. `attachment; filename=""` is 23 runes on its
/// own and a percent-escaped CJK character is 9, so a name of eleven Chinese
/// characters is already over the preview cap — and non-ASCII names are the
/// whole reason this line exists, since a name that is nothing but escapes is
/// the one most likely to come out wrong. Worse than losing the tail: the cut
/// lands mid-escape ("…%E7%89%8…"), and a half-written escape cannot be
/// decoded back into the character it stood for, so a truncated line does not
/// even answer the question it was written to answer.
///
/// 2048 is sized against the largest thing this can legitimately be rather
/// than picked round. POSIX filesystems stop a name at 255 BYTES;
/// percent-escaping every one of those triples it to 765, and a header
/// carrying BOTH parameter forms of such a name — filename= and filename*= —
/// comes to about 1570 runes with its scaffolding.
const TRACE_HEADER_RUNES: usize = 2048;

/// Returns a bounded, single-line prefix of s with any bearer token redacted.
/// Newlines become spaces so one frame stays one log line, and the cut is on
/// a rune boundary.
///
/// The redaction is not optional. The binding prompt builds
/// "👋 请先绑定你的 Patchbay 账号，才能与我对话：\n" + appURL +
/// "/wecom/bind?token=" + a 43-character token, and with a normal PATCHBAY_APP_URL
/// the token's last character lands at rune 107-112 — inside the cap. Without
/// this, turning tracing on for a debugging session would log live binding
/// credentials in full. A binding token is a bearer credential, so whoever
/// could read the log could bind that sender's WeCom identity to their own
/// Patchbay account before the user clicked their own link.
///
/// It matches on the query parameter rather than on the bind path, because
/// this is a free function with no access to the configured path. A "token="
/// in a URL is the shape worth hiding wherever it appears.
pub fn trace_preview(s: &str) -> String {
    trace_bound(&redact_bearer_tokens(s), TRACE_PREVIEW_RUNES)
}

/// [`trace_preview`]'s second half on its own: the single-line flattening and
/// a cap, without the redaction. The cap is a parameter because its two
/// callers bound different things for different reasons — a message preview
/// is deliberately short ([`TRACE_PREVIEW_RUNES`]), an attachment header is
/// bounded only so a remote string cannot run away ([`TRACE_HEADER_RUNES`]).
pub fn trace_bound(s: &str, limit: usize) -> String {
    let mut out = String::with_capacity(s.len().min(limit * 4));
    let mut cut = false;
    for (n, r) in s.chars().enumerate() {
        if n == limit {
            cut = true;
            break;
        }
        match r {
            '\n' | '\r' => out.push(' '),
            _ => out.push(r),
        }
    }
    if cut {
        out.push('…');
    }
    out
}

// Stages of one write, named on a failed outcome so a rejected frame is
// distinguishable from one whose deadline could not even be set.
pub const TRACE_STAGE_DEADLINE: &str = "set_write_deadline";
pub const TRACE_STAGE_WRITE: &str = "write_message";

/// What tracing keeps about one outbound frame between extracting its fields
/// and emitting its two lines. `None` from [`trace_out_fields`] means this
/// frame is not being traced; both lines are governed by that single
/// decision, so the log can never hold an attempt whose outcome was
/// suppressed by the switch flipping halfway through a write.
#[derive(Debug, Default, Clone)]
pub struct OutTrace {
    cmd: String,
    req_id: String,
    chatid: Option<String>,
    chat_type: Option<i64>,
    msgtype: Option<String>,
    content_len: Option<usize>,
    text: Option<String>,
}

/// Extracts what tracing records about a frame on its way to WeCom. The
/// sender calls it BEFORE taking the writer mutex: this is the expensive half
/// — a regexp redaction pass and a rune-by-rune cut over the message body —
/// and none of it needs to be serialized with the socket. Only the emit does,
/// and that is [`trace_out_attempt`]'s job.
///
/// It reads named fields rather than dumping the frame: the aibot_subscribe
/// body carries the smart-bot secret, and a wholesale dump would put it in
/// the log. Add a field here only after checking what every cmd puts under
/// it.
pub fn trace_out_fields(frame: &Value) -> Option<OutTrace> {
    if !tracing_on() {
        return None;
    }
    let mut t = OutTrace {
        cmd: frame
            .get("cmd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        ..Default::default()
    };
    t.req_id = frame
        .get("headers")
        .and_then(|h| h.get("req_id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if let Some(body) = frame.get("body").filter(|b| b.is_object()) {
        t.chatid = body
            .get("chatid")
            .and_then(Value::as_str)
            .map(str::to_string);
        t.chat_type = body.get("chat_type").and_then(Value::as_i64);
        t.msgtype = body
            .get("msgtype")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(content) = body
            .get("markdown")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
        {
            t.content_len = Some(content.len());
            t.text = Some(trace_preview(content));
        }
    }
    Some(t)
}

/// Records a frame at the moment it is about to be written. The sender calls
/// it under the writer mutex, which is the point at which concurrent senders
/// — the ping loop, agent replies, inbox pushes — become ordered. So the
/// order of these lines is the order the frames reach the wire, by
/// construction rather than by correlation.
pub fn trace_out_attempt(seq: u64, t: &OutTrace) {
    tracing::info!(
        target: "wecom_trace",
        dir = "out",
        seq = seq,
        cmd = %t.cmd,
        req_id = %t.req_id,
        chatid = t.chatid.as_deref().unwrap_or(""),
        chat_type = t.chat_type.unwrap_or(0),
        msgtype = t.msgtype.as_deref().unwrap_or(""),
        len = t.content_len.unwrap_or(0),
        text = t.text.as_deref().unwrap_or(""),
        "wecom trace"
    );
}

/// Records what became of the attempt carrying the same seq. Also under the
/// writer mutex, so an attempt and its outcome are never split by another
/// task's frame.
///
/// The error text goes through [`trace_preview`]: it is the socket's words,
/// not ours, so it is bounded and redacted like every other message string
/// here.
pub fn trace_out_result(seq: u64, t: &OutTrace, stage: &str, err: Option<&anyhow::Error>) {
    match err {
        None => tracing::info!(
            target: "wecom_trace",
            dir = "out.done",
            seq = seq,
            cmd = %t.cmd,
            req_id = %t.req_id,
            ok = true,
            "wecom trace"
        ),
        Some(e) => tracing::info!(
            target: "wecom_trace",
            dir = "out.done",
            seq = seq,
            cmd = %t.cmd,
            req_id = %t.req_id,
            ok = false,
            stage = stage,
            error = %trace_preview(&e.to_string()),
            "wecom trace"
        ),
    }
}

/// Records a frame arriving from WeCom, including the server's verdict on
/// something we sent — an errcode here is how a silent failure becomes
/// visible. dispatch_frame only warns on a non-zero errcode for the anonymous
/// ack case, so without this a rejected aibot_send_msg is the only rejection
/// that shows up at all.
pub fn trace_in(env: &FrameEnvelope) {
    if !tracing_on() {
        return;
    }
    tracing::info!(
        target: "wecom_trace",
        dir = "in",
        cmd = %env.cmd,
        req_id = %env.headers.req_id,
        errcode = env.err_code,
        errmsg = %trace_preview(&env.err_msg),
        "wecom trace"
    );
}

/// Records a decoded user message: what the adapter believes about who sent
/// it and where it landed, which is exactly the pair that gets confused (the
/// room's id in one field, the person's in the other). [`trace_in`] alone
/// cannot show it — the callback body is still raw JSON at that point.
pub fn trace_inbound(mc: &AibotMsgCallback, text: &str) {
    if !tracing_on() {
        return;
    }
    tracing::info!(
        target: "wecom_trace",
        dir = "in.msg",
        msg_id = %mc.msg_id,
        chatid = %mc.chat_id,
        chat_type = %mc.chat_type,
        sender = %mc.from.user_id,
        msgtype = %mc.msg_type,
        len = text.len(),
        text = %trace_preview(text),
        "wecom trace"
    );
}

/// What an attachment's response said about itself: the Content-Disposition
/// exactly as it arrived, and the name this package parsed out of it. Those
/// two side by side are the whole diagnosis for a filename that comes out
/// looking wrong, and they cannot be recovered later — the URL they came from
/// is good for five minutes, so an attachment whose name is questioned
/// tomorrow can never be re-fetched to settle it.
///
/// Both values are flattened to one line and capped at
/// [`TRACE_HEADER_RUNES`], and neither is run through
/// [`redact_bearer_tokens`] the way every other traced string is. Two
/// deliberate departures, for one reason: the point of this line is the exact
/// bytes.
///
/// The redactor rewrites anything shaped like "token=…", and a file may
/// legitimately be called that — a redacted filename is a filename nobody can
/// diagnose. Nothing here is a credential anyway: the header carries a name,
/// and the pre-signed URL that does carry one is deliberately absent from
/// this line and from everything else media_download.rs emits.
///
/// It fires even when the header was absent, recording an empty value. A run
/// that produced no line at all would leave the reader unable to tell "the
/// server sent no Content-Disposition" from "the switch was off".
pub fn trace_media_headers(msg_id: &str, index: usize, disposition: &str, filename: &str) {
    if !tracing_on() {
        return;
    }
    tracing::info!(
        target: "wecom_trace",
        dir = "in.media",
        msg_id = %msg_id,
        index = index,
        content_disposition = %trace_bound(disposition, TRACE_HEADER_RUNES),
        filename = %trace_bound(filename, TRACE_HEADER_RUNES),
        "wecom trace"
    );
}

fn token_pattern() -> &'static regex::Regex {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| {
        // Finds a token carried as a query parameter, whatever path it sits
        // on. Deliberately broad: an access_token or a code is no safer in a
        // log than a binding token.
        regex::Regex::new(r#"(?i)\b((?:binding_token|access_token|token|code)=)([^\s&"'<>]+)"#)
            .expect("valid token redaction pattern")
    })
}

/// Replaces the value of any token-shaped query parameter with a fixed
/// marker, keeping enough of the line to be worth logging.
pub fn redact_bearer_tokens(s: &str) -> String {
    if !s.contains('=') {
        return s.to_string();
    }
    token_pattern()
        .replace_all(s, "${1}[redacted]")
        .into_owned()
}

/// Base64 helper shared with tests below.
#[allow(dead_code)]
pub(crate) fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Mutex, MutexGuard};

    fn trace_test_lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn set_trace_roundtrips() {
        let _guard = trace_test_lock();
        assert!(set_trace(true));
        assert!(tracing_on());
        assert!(!set_trace(false));
        assert!(!tracing_on());
    }

    #[test]
    fn trace_bound_flattens_and_caps_on_runes() {
        assert_eq!(trace_bound("a\nb\rc", 10), "a b c");
        assert_eq!(trace_bound("hello", 5), "hello");
        assert_eq!(trace_bound("hello!", 5), "hello…");
        // Rune-counted, not byte-counted: three CJK chars fit under 3.
        assert_eq!(trace_bound("登录坏", 3), "登录坏");
        assert_eq!(trace_bound("登录坏了", 3), "登录坏…");
        assert_eq!(trace_bound("", 5), "");
    }

    #[test]
    fn redact_covers_every_token_param_shape() {
        assert_eq!(
            redact_bearer_tokens("https://app/wecom/bind?token=SECRET43CHARS&x=1"),
            "https://app/wecom/bind?token=[redacted]&x=1"
        );
        assert_eq!(
            redact_bearer_tokens("BINDING_TOKEN=abc ACCESS_TOKEN=def"),
            "BINDING_TOKEN=[redacted] ACCESS_TOKEN=[redacted]"
        );
        assert_eq!(redact_bearer_tokens("no equals here"), "no equals here");
        // Non-token params stay intact.
        assert_eq!(
            redact_bearer_tokens("chatid=c123&code=zzz"),
            "chatid=c123&code=[redacted]"
        );
    }

    #[test]
    fn trace_out_fields_reads_named_fields_only() {
        let _guard = trace_test_lock();
        set_trace(true);
        let frame = json!({
            "cmd": "aibot_send_msg",
            "headers": {"req_id": "r9"},
            "body": {
                "chatid": "c1", "chat_type": 2, "msgtype": "markdown",
                "markdown": {"content": "hi there"},
            },
        });
        let t = trace_out_fields(&frame).unwrap();
        assert_eq!(t.cmd, "aibot_send_msg");
        assert_eq!(t.req_id, "r9");
        assert_eq!(t.chatid.as_deref(), Some("c1"));
        assert_eq!(t.chat_type, Some(2));
        assert_eq!(t.msgtype.as_deref(), Some("markdown"));
        assert_eq!(t.content_len, Some(8));
        assert_eq!(t.text.as_deref(), Some("hi there"));

        // The subscribe secret never surfaces: body fields outside the named
        // list are ignored.
        let sub = json!({
            "cmd": "aibot_subscribe",
            "headers": {"req_id": "r1"},
            "body": {"bot_id": "b", "secret": "TOPSECRET"},
        });
        let t = trace_out_fields(&sub).unwrap();
        assert_eq!(t.text, None);

        set_trace(false);
        assert!(trace_out_fields(&frame).is_none());
    }

    #[test]
    fn media_header_line_fires_even_when_absent() {
        let _guard = trace_test_lock();
        set_trace(true);
        // No panic and no filtering: absence is recorded as empty strings.
        trace_media_headers("m1", 0, "", "");
        set_trace(false);
    }

    #[test]
    fn b64_helper_matches_go_std_encoding() {
        assert_eq!(b64(b"sealed"), "c2VhbGVk");
    }
}
