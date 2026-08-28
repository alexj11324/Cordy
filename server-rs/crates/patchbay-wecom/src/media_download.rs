//! Fetching the bytes a callback points at — port of `media_download.go`.
//!
//! The URL is a pre-signed Tencent COS address: no access_token, no header,
//! good for five minutes. That makes the fetch itself trivial and puts all
//! the care somewhere else — the body arrives encrypted, its size is not
//! declared anywhere in the callback, and the only description of what the
//! file IS comes back in the response headers.

use std::collections::HashMap;
use std::error::Error;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::AsyncRead;

/// The ceiling on one downloaded body. WeCom caps smart-bot files and video
/// at 100 MB and does not document a cap for images, so this is the ceiling
/// for everything: whatever arrives above it is not something the callback
/// was supposed to hand us.
///
/// On the buffered path the body is held whole — CBC decryption needs the
/// tail before the head can be trusted — so the ceiling is also the
/// per-download memory bound, multiplied by the router's media concurrency.
/// The streaming path bounds disk instead.
pub const MAX_MEDIA_BYTES: usize = 100 << 20;

/// Caps a single fetch. The router already runs media resolution under a 45s
/// budget shared by every attachment on the message and by whatever is queued
/// ahead of it, so one slow object must not eat all of it.
pub const MEDIA_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Returned for a body past [`MAX_MEDIA_BYTES`], either declared in
/// Content-Length or discovered while reading. Callers match on it to tell
/// the user the file was too big rather than that something went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("wecom: media exceeds the {limit} byte limit")]
pub struct MediaTooLarge {
    pub limit: usize,
}

impl MediaTooLarge {
    fn err() -> anyhow::Error {
        anyhow::Error::new(MediaTooLarge {
            limit: MAX_MEDIA_BYTES,
        })
    }
}

/// What the response said about the file. Both fetch paths return it, so the
/// buffered and the streaming ingest learn the same things.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaHeaders {
    /// The display name parsed out of Content-Disposition, or empty. The
    /// callback body carries no name, size or MIME type of its own, so this
    /// header is the only place the original name exists.
    pub filename: String,
    /// The Content-Disposition value exactly as it arrived, kept for the
    /// trace and for nothing else. What COS puts in that header is the one
    /// thing this package cannot check locally — the URL it came from lapses
    /// after five minutes, so a name that looks wrong cannot be re-fetched
    /// and must have been recorded at the time.
    pub disposition: String,
}

/// One fetched body plus what the response said about it.
#[derive(Debug, Clone)]
pub struct DownloadedMedia {
    /// The raw response — still encrypted. decrypt_media turns it into the
    /// file.
    pub body: Vec<u8>,
    pub headers: MediaHeaders,
}

/// GETs one media URL. Errors carry the reason (expired link, oversize body,
/// stalled server) because the caller turns them into something a person
/// reads.
pub async fn download_media(
    hc: &reqwest::Client,
    raw_url: &str,
) -> anyhow::Result<DownloadedMedia> {
    check_media_url(raw_url)?;
    let fetch = async {
        let resp = hc
            .get(raw_url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("wecom: media download: {}", strip_url(&e)))?;
        if let Some(len) = resp.content_length() {
            if len > MAX_MEDIA_BYTES as u64 {
                return Err(MediaTooLarge::err());
            }
        }
        let status = resp.status();
        if !status.is_success() {
            // A five-minute URL that has already lapsed is the ordinary
            // failure here, and COS explains itself in a short XML body.
            // Carry a snippet so the log says which kind of refusal it was.
            let snippet = read_snippet(resp).await;
            return Err(anyhow::anyhow!(
                "wecom: media download: http {} {}: {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or(""),
                snippet
            ));
        }
        let disposition = header_value(&resp, "content-disposition");
        let body = read_capped_body(resp).await?;
        Ok(DownloadedMedia {
            body,
            headers: MediaHeaders {
                filename: media_filename_from_disposition(&disposition),
                disposition,
            },
        })
    };
    tokio::time::timeout(MEDIA_DOWNLOAD_TIMEOUT, fetch)
        .await
        .map_err(|_| anyhow::anyhow!("wecom: media download timed out"))?
}

/// [`download_media`] without the buffer: it returns the body as a stream so
/// the caller can decrypt it as it arrives, plus the same [`MediaHeaders`]
/// the buffered path reads. Same guard, same status handling, same ceiling —
/// the ceiling is enforced by the [`CappedBody`] the caller reads through, so
/// a body that lies about its length still cannot run away with the process.
///
/// The caller owns the reader; dropping it releases the connection.
pub async fn open_media_parts(
    hc: &reqwest::Client,
    raw_url: &str,
) -> anyhow::Result<(CappedBody<Box<dyn AsyncRead + Send + Unpin>>, MediaHeaders)> {
    check_media_url(raw_url)?;

    // No overall timeout here the way download_media has one: the caller
    // reads this body over the length of a decrypt, and a deadline that
    // covers the fetch would cut the read off mid-file. The router's own
    // media budget bounds the whole operation.
    let resp = hc
        .get(raw_url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("wecom: media download: {}", strip_url(&e)))?;
    if let Some(len) = resp.content_length() {
        if len > MAX_MEDIA_BYTES as u64 {
            return Err(MediaTooLarge::err());
        }
    }
    let status = resp.status();
    if !status.is_success() {
        let snippet = read_snippet(resp).await;
        return Err(anyhow::anyhow!(
            "wecom: media download: http {} {}: {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or(""),
            snippet
        ));
    }
    let disposition = header_value(&resp, "content-disposition");
    let headers = MediaHeaders {
        filename: media_filename_from_disposition(&disposition),
        disposition,
    };
    let stream = resp
        .bytes_stream()
        .map(|r| r.map_err(std::io::Error::other));
    let inner =
        Box::new(tokio_util::io::StreamReader::new(stream)) as Box<dyn AsyncRead + Send + Unpin>;
    // One byte of headroom, as download_media has.
    Ok((CappedBody::new(inner, MAX_MEDIA_BYTES as i64 + 1), headers))
}

async fn read_snippet(resp: reqwest::Response) -> String {
    let mut out = Vec::new();
    let mut stream = resp.bytes_stream();
    while out.len() < 512 {
        match stream.next().await {
            Some(Ok(chunk)) => {
                let take = 512 - out.len();
                out.extend_from_slice(&chunk[..chunk.len().min(take)]);
            }
            _ => break,
        }
    }
    String::from_utf8_lossy(&out).trim().to_string()
}

async fn read_capped_body(resp: reqwest::Response) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| anyhow::anyhow!("wecom: media download: read body: {e}"))?;
        out.extend_from_slice(&chunk);
        if out.len() > MAX_MEDIA_BYTES {
            return Err(MediaTooLarge::err());
        }
    }
    Ok(out)
}

fn header_value(resp: &reqwest::Response, name: &str) -> String {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// Removes the request URL from a transport error before it can be logged.
///
/// reqwest prints the URL in its Display — and the URL here is a pre-signed
/// COS link: a five-minute bearer credential for a colleague's private
/// attachment, good to anyone who presents it. A DNS hiccup, a TCP reset, a
/// TLS error or the download timeout would each have written one into the
/// application log, and from there into whatever ships those logs onward.
/// The wrapped cause carries the same diagnosis with none of the credential,
/// so that is what we keep.
fn strip_url(err: &reqwest::Error) -> String {
    match err.source() {
        Some(src) => src.to_string(),
        None => "request failed".to_string(),
    }
}

/// Refuses anything the transport should not be pointed at. The URL arrives
/// over the authenticated socket, so this is a guard rail rather than a
/// defence, but it is a string from outside naming a host we then GET.
pub fn check_media_url(raw_url: &str) -> anyhow::Result<()> {
    let s = raw_url.trim();
    if s.is_empty() {
        anyhow::bail!("wecom: media url is empty");
    }
    let u = url::Url::parse(s).map_err(|_| anyhow::anyhow!("wecom: media url is unparseable"))?;
    if u.scheme() != "http" && u.scheme() != "https" {
        anyhow::bail!("wecom: media url scheme {:?} is not fetchable", u.scheme());
    }
    if u.host_str().is_none() {
        anyhow::bail!("wecom: media url has no host");
    }
    Ok(())
}

/// Stops a response that keeps going past the ceiling, so an undeclared
/// length cannot be used to fill the disk the way it used to be able to fill
/// the heap.
///
/// Port note: Go's Read returns the final n together with the error; Rust's
/// poll_read cannot do both, so the refusal surfaces on the read AFTER the
/// budget is exhausted. Every byte inside the budget was still delivered, and
/// the caller refuses the whole attachment either way.
pub struct CappedBody<R> {
    inner: R,
    remaining: i64,
}

impl<R> CappedBody<R> {
    pub fn new(inner: R, cap: i64) -> Self {
        Self {
            inner,
            remaining: cap,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for CappedBody<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.remaining <= 0 {
            return Poll::Ready(Err(std::io::Error::other(MediaTooLarge {
                limit: MAX_MEDIA_BYTES,
            })));
        }
        let filled = buf.filled().len();
        let cap = self.remaining.min((buf.capacity() - filled) as i64) as usize;
        let mut sub = tokio::io::ReadBuf::new(buf.initialize_unfilled_to(cap));
        match std::pin::Pin::new(&mut self.inner).poll_read(cx, &mut sub) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Ready(Ok(())) => {
                let n = sub.filled().len();
                self.remaining -= n as i64;
                buf.advance(n);
                Poll::Ready(Ok(()))
            }
        }
    }
}

/// Reads the display name out of Content-Disposition, preferring RFC 5987's
/// extended `filename*` form over the plain `filename=` beside it. Servers
/// that send both put the real (non-ASCII) name in the extended form and a
/// mangled ASCII approximation in the plain one, so taking the plain one
/// would rename every Chinese attachment to underscores.
///
/// A plain `filename=` from COS arrives form-urlencoded, and the parser does
/// not undo that (it percent-decodes only the extended form), so the value
/// needs [`decode_form_encoded_filename`] before it is fit to show anyone.
/// The extended form is skipped there, because a server that sent
/// `filename*` declared its own encoding and the parser has already applied
/// it — decoding a second time would be guessing on top of a declaration.
///
/// The decode runs BEFORE the base-name reduction, not after: an escaped
/// separator (`..%2F..%2Fetc%2Fpasswd`) is a path only once it is decoded,
/// and a cleaner that ran first would hand a traversal straight through.
///
/// The result goes through [`clean_media_filename`], which reduces it to a
/// base name and strips control characters: the header is remote input, and
/// once the escapes above are undone it can carry a separator or a NUL that
/// the raw header could not.
pub fn media_filename_from_disposition(raw: &str) -> String {
    if raw.trim().is_empty() {
        return String::new();
    }
    let Some(params) = parse_content_disposition_params(raw) else {
        return String::new();
    };
    let name = params.get("filename").cloned().unwrap_or_default();
    let name = if !has_extended_filename(raw) {
        decode_form_encoded_filename(&name)
    } else {
        name
    };
    clean_media_filename(&name)
}

/// Reports whether the header offered an RFC 5987 `filename*` at all. The
/// parser folds the extended parameter into params["filename"] and does not
/// say which form it came from, so the raw header is the only place left to
/// ask.
///
/// A plain filename whose VALUE contains the text "filename*" reads as a
/// false positive here. That costs nothing: the only consequence is that the
/// value is left exactly as it arrived.
fn has_extended_filename(raw: &str) -> bool {
    raw.to_lowercase().contains("filename*")
}

/// Undoes application/x-www-form-urlencoding on a filename that provably
/// carries it, and leaves every other name byte for byte.
///
/// Why this is needed: a live tenant sent "PC D&T Strategy 2026.docx" and it
/// was stored as "PC+D%26T+Strategy+2026.docx" — space as '+', '&' as %26,
/// which is exactly query_escape of the original. COS form-encodes the plain
/// filename parameter because that parameter is ASCII-only by specification,
/// and Chinese names take the same route: a non-ASCII name cannot legally sit
/// there, so it arrives as a run of percent escapes, and without this the
/// user is shown the escapes.
///
/// Why it is conditional: a naive unescape reads '+' as a space, so it would
/// rewrite "C++ notes.docx" to "C   notes.docx". Both readings of a bare '+'
/// are legitimate and no header field distinguishes them. What does
/// distinguish them is self-consistency — a genuine form-encoding survives a
/// round trip through the encoder, and an accidental one usually does not:
/// "C++ notes.docx" re-encodes to "C+++notes.docx" (the literal space would
/// have been a '+' too) and is therefore left alone. So the rule is: decode
/// only when re-encoding the result reproduces the header value.
///
/// Known gaps, deliberate (one stored filename is all the evidence there is):
/// any name whose only encoder-touched character is a '+' IS a canonical
/// encoding of the same name with spaces, so the round trip cannot rule it
/// out and this decodes it, losing the pluses ("React+Redux.md" → "React
/// Redux.md"). A name percent-encoded RFC-3986 style (%20 for space) fails
/// the round trip and keeps its escapes. A sender whose unreserved set is not
/// query_escape's fails wholesale rather than partially.
pub fn decode_form_encoded_filename(name: &str) -> String {
    // Nothing an encoder produces looks like this, so there is nothing to
    // undo and no round trip worth running.
    if !name.contains('+') && !name.contains('%') {
        return name.to_string();
    }
    let Some(decoded) = query_unescape(name) else {
        // A stray '%' that begins no valid escape ("100%.docx") means this is
        // not an encoding at all, so there is nothing to undo.
        return name.to_string();
    };
    if decoded == name || !same_form_encoding(&query_escape(&decoded), name) {
        return name.to_string();
    }
    decoded
}

/// Reports whether two form-encodings of the same value agree, allowing only
/// for the case of the hex digits inside percent escapes.
///
/// The tolerance is not cosmetic. query_escape writes upper-case hex and
/// plenty of servers write lower-case, so a byte comparison would reject
/// "%e5%ad%a3%e6%8a%a5.png" — a Chinese filename, the case this decode
/// matters most for, since a non-ASCII name is nothing but escapes.
fn same_form_encoding(a: &str, b: &str) -> bool {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    if ab.len() != bb.len() {
        return false;
    }
    let mut i = 0usize;
    while i < ab.len() {
        if ab[i] == b'%' && bb[i] == b'%' && i + 2 < ab.len() {
            if !ab[i + 1..i + 3].eq_ignore_ascii_case(&bb[i + 1..i + 3]) {
                return false;
            }
            i += 3;
            continue;
        }
        if ab[i] != bb[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// Reduces a name from anywhere to a single path segment with no control
/// characters in it, or empty when nothing usable is left.
///
/// The control-character strip exists because decoding escapes widens what
/// can reach this function: %00 becomes a real NUL and %0D%0A a real CRLF.
/// NUL is the one that costs something — Postgres TEXT cannot store a NUL,
/// so the insert would fail and the whole attachment would be lost, for a
/// file whose bytes downloaded and decrypted perfectly. CR and LF are the
/// header-injection shape.
///
/// Reaching any of this needs a malicious or buggy CDN — a user cannot type a
/// NUL into a filename. That is the right bar anyway: this header is remote
/// input, and this function is the single choke point both fetch paths reach
/// it through.
pub fn clean_media_filename(name: &str) -> String {
    let name = strip_control_runes(name).trim().to_string();
    if name.is_empty() {
        return String::new();
    }
    let replaced = name.replace('\\', "/");
    let base = go_path_base(&replaced);
    if base == "." || base == "/" || base == ".." {
        return String::new();
    }
    base.to_string()
}

/// Drops every control character, dropping rather than substituting: a
/// placeholder would put a character in the name that the sender did not
/// type, and an attachment called "a_b.docx" that was really called "ab.docx"
/// is its own small lie.
///
/// Bytes that decoded to invalid UTF-8 are a separate matter and are NOT
/// dropped: they arrive here already replaced with U+FFFD by the lossy
/// conversion, which is the useful answer — Postgres TEXT will not take an
/// invalid UTF-8 sequence either, and U+FFFD is the standard way to say "a
/// character was here and it did not survive".
fn strip_control_runes(s: &str) -> String {
    s.chars().filter(|r| !r.is_control()).collect()
}

/// Go's path.Base: the last element after collapsing trailing slashes.
fn go_path_base(p: &str) -> &str {
    if p.is_empty() {
        return ".";
    }
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/";
    }
    match trimmed.rfind('/') {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    }
}

/// A minimal `mime.ParseMediaType` for the one header this package reads:
/// splits parameters off the type, honours quoted-string values, lowercases
/// keys, and folds RFC 5987 extended parameters (`filename*=UTF-8''…`) into
/// their plain names after charset-aware percent-decoding — the behaviour the
/// filename preference above depends on.
fn parse_content_disposition_params(raw: &str) -> Option<HashMap<String, String>> {
    let mut parts = raw.split(';');
    let _mediatype = parts.next()?.trim();
    let mut out = HashMap::new();
    for part in parts {
        let part = part.trim_start();
        if part.is_empty() {
            continue;
        }
        let Some(eq) = find_param_equals(part) else {
            continue;
        };
        let key = part[..eq].trim().to_lowercase();
        let mut value = part[eq + 1..].trim();
        let mut quoted = false;
        if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
            value = &value[1..value.len() - 1];
            quoted = true;
        }
        let value = if quoted {
            unquote_header_value(value)
        } else {
            value.to_string()
        };
        if let Some(stripped) = key.strip_suffix('*') {
            // Extended form: charset'lang'%percent-encoded. The parser folds
            // it under the plain name, which is what the caller reads. The
            // extended form wins regardless of the order the two appear in
            // (servers that send both put the real non-ASCII name here), so
            // it overwrites any plain value already stored.
            if let Some(decoded) = decode_rfc5987_value(&value) {
                out.insert(stripped.to_string(), decoded);
            }
        } else {
            // A plain value never displaces an already-folded extended one.
            out.entry(key).or_insert(value);
        }
    }
    Some(out)
}

/// Finds the '=' separating a parameter name from its value, ignoring '='
/// inside a quoted value's name position (names are tokens, so the first '='
/// wins).
fn find_param_equals(part: &str) -> Option<usize> {
    part.find('=')
}

fn unquote_header_value(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Decodes `charset'lang'value` per RFC 5987. Only UTF-8 and the ISO-8859-1
/// fallback are handled; anything else decodes lossily rather than failing,
/// because a mangled name is recoverable by a human while a missing one is
/// not.
fn decode_rfc5987_value(value: &str) -> Option<String> {
    let (charset, rest) = value.split_once('\'')?;
    let (_lang, data) = rest.split_once('\'')?;
    let bytes = percent_decode(data)?;
    match charset.to_lowercase().as_str() {
        "utf-8" | "utf8" => Some(String::from_utf8_lossy(&bytes).into_owned()),
        "iso-8859-1" | "latin1" => Some(bytes.iter().map(|&b| b as char).collect()),
        // Unknown charset: try UTF-8, fall back to lossy — same trade.
        _ => Some(String::from_utf8_lossy(&bytes).into_owned()),
    }
}

fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hi = (*bytes.get(i + 1)?) as char;
                let lo = (*bytes.get(i + 2)?) as char;
                let h = hi.to_digit(16)?;
                let l = lo.to_digit(16)?;
                out.push(((h << 4) | l) as u8);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Some(out)
}

/// Go's url.QueryUnescape: '+' becomes a space, %XX decodes; an invalid
/// escape makes the whole thing fail.
fn query_unescape(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hi = (*bytes.get(i + 1)?) as char;
                let lo = (*bytes.get(i + 2)?) as char;
                let h = hi.to_digit(16)?;
                let l = lo.to_digit(16)?;
                out.push(((h << 4) | l) as u8);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// Go's url.QueryEscape: space becomes '+', A-Za-z0-9-_.~ pass through,
/// everything else becomes upper-case %XX.
fn query_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_media_url_refuses_the_usual_junk() {
        assert!(check_media_url("").is_err());
        assert!(check_media_url("   ").is_err());
        assert!(check_media_url("not a url \u{0}").is_err());
        assert!(check_media_url("ftp://cos.example/x").is_err());
        assert!(check_media_url("file:///etc/passwd").is_err());
        assert!(check_media_url("javascript:alert(1)").is_err());
        assert!(check_media_url("https://cos.example/a?sig=1").is_ok());
        assert!(check_media_url("http://cos.example/a").is_ok());
    }

    #[test]
    fn disposition_prefers_the_extended_form() {
        // Both forms present: the real (non-ASCII) name wins.
        let raw = "attachment; filename=\"_.docx\"; filename*=UTF-8''%E5%AD%A3%E6%8A%A5.png";
        assert_eq!(media_filename_from_disposition(raw), "季报.png");

        // Order does not matter.
        let raw = "attachment; filename*=UTF-8''%E5%AD%A3%E6%8A%A5.png; filename=\"_.docx\"";
        assert_eq!(media_filename_from_disposition(raw), "季报.png");
    }

    #[test]
    fn plain_form_gets_the_conditional_round_trip_decode() {
        // Genuine form-encoding survives the round trip → decoded.
        assert_eq!(
            media_filename_from_disposition("attachment; filename=\"PC+D%26T+Strategy+2026.docx\""),
            "PC D&T Strategy 2026.docx"
        );
        // Lower-case hex escapes (a Chinese name) decode too.
        assert_eq!(
            media_filename_from_disposition("attachment; filename=%e5%ad%a3%e6%8a%a5.png"),
            "季报.png"
        );
        // "C+++notes.docx" is byte-identical to a form-encoding of
        // "C   notes.docx", so no rule reading only this header can tell them
        // apart — we decode, and this name loses its pluses.
        assert_eq!(
            media_filename_from_disposition("attachment; filename=\"C+++notes.docx\""),
            "C   notes.docx"
        );
        // A plus beside a real space fails the round trip → untouched.
        assert_eq!(
            media_filename_from_disposition("attachment; filename=\"C++ notes.docx\""),
            "C++ notes.docx"
        );
        // An escaped plus survives the decode as a literal '+'.
        assert_eq!(
            media_filename_from_disposition("attachment; filename=\"C%2B%2B+notes.docx\""),
            "C++ notes.docx"
        );
        // A name whose only encoder-touched character is a '+' loses it too.
        assert_eq!(
            media_filename_from_disposition("attachment; filename=\"React+Redux.md\""),
            "React Redux.md"
        );
        // And so does a version number.
        assert_eq!(
            media_filename_from_disposition("attachment; filename=\"C++11.pdf\""),
            "C  11.pdf"
        );
        // RFC-3986-style %20 fails the round trip → untouched.
        assert_eq!(
            media_filename_from_disposition("attachment; filename=\"PC%20D%26T.docx\""),
            "PC%20D%26T.docx"
        );
        // No encoder-touched characters at all → untouched.
        assert_eq!(
            media_filename_from_disposition("attachment; filename=\"plain.docx\""),
            "plain.docx"
        );
        // Invalid escape → untouched.
        assert_eq!(
            media_filename_from_disposition("attachment; filename=\"100%.docx\""),
            "100%.docx"
        );
    }

    #[test]
    fn clean_reduces_to_a_safe_base_name() {
        assert_eq!(clean_media_filename("../etc/passwd"), "passwd");
        // The percent-escaped traversal arrives at the cleaner only after
        // decode_form_encoded_filename has undone the escapes (Go: decode
        // runs BEFORE the base-name reduction) — an escaped separator is a
        // path only once decoded.
        assert_eq!(
            clean_media_filename(&decode_form_encoded_filename("..%2F..%2Fetc%2Fpasswd")),
            "passwd"
        );
        // Raw (undecoded) escapes carry no real separator and pass through.
        assert_eq!(
            clean_media_filename("..%2F..%2Fetc%2Fpasswd"),
            "..%2F..%2Fetc%2Fpasswd"
        );
        assert_eq!(clean_media_filename("a\\b\\c.docx"), "c.docx");
        assert_eq!(clean_media_filename("  spaced .png  "), "spaced .png");
        assert_eq!(clean_media_filename("a\u{0}b.docx"), "ab.docx");
        assert_eq!(clean_media_filename("a\r\nb.docx"), "ab.docx");
        assert_eq!(clean_media_filename(""), "");
        assert_eq!(clean_media_filename(".."), "");
        assert_eq!(clean_media_filename("/"), "");
        assert_eq!(clean_media_filename("."), "");
    }

    #[test]
    fn malformed_dispositions_yield_empty_names() {
        assert_eq!(media_filename_from_disposition(""), "");
        assert_eq!(media_filename_from_disposition("   "), "");
        assert_eq!(
            media_filename_from_disposition("garbage-without-params"),
            ""
        );
    }

    #[test]
    fn form_encoding_helpers_match_go() {
        assert_eq!(query_escape("PC D&T.docx"), "PC+D%26T.docx");
        assert_eq!(
            query_unescape("PC+D%26T.docx").as_deref(),
            Some("PC D&T.docx")
        );
        assert_eq!(query_unescape("100%.docx"), None);
        assert!(same_form_encoding("%E5%AD%90", "%e5%ad%90"));
        // Same length, case-insensitive escapes agree — Go returns true here
        // too; the length guard only rejects genuinely different lengths.
        assert!(same_form_encoding("%E5%AD", "%e5%ad"));
        assert!(!same_form_encoding("%E5%AD", "%e5%ad%90")); // lengths differ
        assert!(same_form_encoding("abc", "abc"));
        assert!(!same_form_encoding("abc", "abd"));
    }

    #[test]
    fn capped_body_refuses_past_the_budget_with_a_classifiable_error() {
        // Covered end-to-end in media_stream tests; here just the boundary.
        let data = vec![7u8; 16];
        let mut capped = CappedBody::new(data.as_slice(), 8);
        let mut sink = Vec::new();
        let res = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(tokio::io::copy(&mut capped, &mut sink));
        assert_eq!(sink.len(), 8);
        let err = res.unwrap_err();
        // io::Error::other keeps the inner error reachable via get_ref; walk
        // that (chain()'s traversal through io::Error's source is opaque here).
        let inner = err.get_ref().expect("io::Error::other carries the cause");
        assert!(inner.downcast_ref::<MediaTooLarge>().is_some());
    }
}
