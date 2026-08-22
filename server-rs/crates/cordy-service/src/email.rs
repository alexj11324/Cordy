//! Email delivery — port of `server/internal/service/email.go`.
//!
//! Three delivery paths, in priority order: SMTP relay (SMTP_HOST set) →
//! Resend API (RESEND_API_KEY set) → DEV stdout. The SMTP client is a small
//! hand-rolled async implementation of the net/smtp subset this service uses,
//! so the PLAIN→LOGIN fallback and 8BITMIME negotiation keep their exact Go
//! semantics.

use anyhow::{anyhow, bail};
use base64::Engine as _;
use chrono::Utc;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// Bounds how much user-controlled text (workspace name, inviter name) can
/// land in an email Subject. Prevents attackers from stuffing a full phishing
/// pitch into a workspace name that gets sent from our domain.
const MAX_SUBJECT_FIELD_RUNES: usize = 60;

const RESEND_API_URL: &str = "https://api.resend.com/emails";

const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
/// Stands in for Go's `conn.SetDeadline(now+30s)`: applied per I/O operation
/// rather than as one session-wide deadline, so a slow-but-alive relay cannot
/// get cut off mid-transfer.
const IO_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct EmailService {
    /// Resend API key; `None` mirrors Go's nil client (DEV mode when SMTP is
    /// also unset).
    resend_api_key: Option<String>,
    resend_http: Option<reqwest::Client>,
    from_email: String,
    smtp_host: String,
    smtp_port: String,
    smtp_username: String,
    smtp_password: String,
    smtp_tls_insecure: bool,
    smtp_tls_implicit: bool,
    smtp_ehlo_name: String,
}

// ---------------------------------------------------------------------------
// Pure helpers — separated from I/O so the sanitization behavior is
// unit-testable without mocking an SMTP server or the Resend API.
// ---------------------------------------------------------------------------

fn is_localhost(name: &str) -> bool {
    name == "localhost" || name == "127.0.0.1" || name == "::1"
}

/// One step of the SMTP LOGIN challenge/response exchange (`loginAuth.Next`).
///
/// Servers send challenges either raw ("Username:") or base64-wrapped
/// ("VXNlcm5hbWU6"); both are lowercased and matched by substring so odd
/// relays still work.
fn login_challenge_response(
    challenge: &[u8],
    username: &str,
    password: &str,
) -> Result<Vec<u8>, String> {
    let raw = String::from_utf8_lossy(challenge);
    let raw = raw.trim();
    let mut lowered = raw.to_lowercase();
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(raw) {
        lowered = String::from_utf8_lossy(&decoded).trim().to_lowercase();
    }

    if lowered.contains("username") || lowered.contains("user name") {
        Ok(username.as_bytes().to_vec())
    } else if lowered.contains("password") {
        Ok(password.as_bytes().to_vec())
    } else {
        Err(format!("unexpected LOGIN challenge {raw:?}"))
    }
}

/// Decides whether a failed PLAIN auth is worth retrying with LOGIN on a
/// fresh connection (`smtpAuthWithFallback` decision half).
///
/// Only the two "server does not know PLAIN" shapes qualify — any other
/// failure (bad credentials, TLS required, …) must surface unchanged instead
/// of triggering a reconnect loop. The server's EHLO must also advertise AUTH
/// with LOGIN among its mechanisms.
fn should_fallback_to_login(plain_err: &str, auth_ext: Option<&str>) -> bool {
    let msg = plain_err.to_lowercase();
    if !msg.contains("unrecognized authentication type") && !msg.contains("504 5.7.4") {
        return false;
    }
    match auth_ext {
        Some(line) => line.to_uppercase().contains("LOGIN"),
        None => false,
    }
}

/// From-address resolution chain. With no SMTP host configured we are on the
/// hosted path: RESEND_FROM_EMAIL or the product default. On the self-hosted
/// SMTP path SMTP_FROM_EMAIL wins because operators control that domain.
fn resolve_from_email(smtp_host: &str) -> String {
    let resend_from = std::env::var("RESEND_FROM_EMAIL")
        .unwrap_or_default()
        .trim()
        .to_string();
    if smtp_host.is_empty() {
        if !resend_from.is_empty() {
            return resend_from;
        }
        return "noreply@cordy.ai".to_string();
    }
    let smtp_from = std::env::var("SMTP_FROM_EMAIL")
        .unwrap_or_default()
        .trim()
        .to_string();
    if !smtp_from.is_empty() {
        return smtp_from;
    }
    resend_from
}

/// Byte-for-byte equivalent of Go's `html.EscapeString` (escapes exactly
/// &, ', <, >, "). Used for user-controlled names interpolated into HTML
/// bodies.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&#39;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            _ => out.push(c),
        }
    }
    out
}

/// Prepares user-controlled text for the email Subject line.
///
/// Subject is not HTML-rendered, so HTML-escaping would leak literal entities
/// (e.g. &lt;script&gt;) into the recipient's inbox. Instead strip control
/// characters (defense in depth against header-injection-adjacent abuse even
/// though Resend also filters CR/LF) and cap length so attackers can't stuff
/// a full phishing subject into a workspace name.
fn sanitize_subject_field(s: &str) -> String {
    let cleaned: String = s.chars().filter(|c| !c.is_control()).collect();
    if cleaned.chars().count() <= MAX_SUBJECT_FIELD_RUNES {
        return cleaned;
    }
    let mut truncated: String = cleaned.chars().take(MAX_SUBJECT_FIELD_RUNES - 1).collect();
    truncated.push('…');
    truncated
}

/// RFC 2047 Q encoding for header words (`mime.QEncoding.Encode("utf-8", …)`).
/// ASCII-safe input passes through untouched; otherwise emits one or more
/// `=?utf-8?q?…?=` words, folding before any word exceeds the 75-byte
/// encoded-word limit so long CJK subjects stay deliverable.
fn q_encode_utf8(s: &str) -> String {
    let needs = s
        .bytes()
        .any(|b| (b < b' ' && b != b'\t') || b >= 0x7f || b == b'=');
    if !needs {
        return s.to_string();
    }

    const MAX_WORD_BYTES: usize = 75;
    // Fixed overhead of "=?utf-8?q?" + "?" = 11 bytes.
    let budget = MAX_WORD_BYTES - 11;

    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;

    for c in s.chars() {
        // Space and tab fold to '_'; '?' and '_' must be escaped to appear
        // literally inside an encoded word; everything else outside the
        // printable Q-safe set becomes =XX per UTF-8 byte.
        let piece: String = match c {
            ' ' | '\t' => "_".to_string(),
            '?' | '_' | '=' => format!("={:02X}", c as u32),
            c if (c as u32) < 0x20 || (c as u32) >= 0x7f => c
                .to_string()
                .as_bytes()
                .iter()
                .map(|b| format!("={b:02X}"))
                .collect(),
            c => c.to_string(),
        };
        if cur_len + piece.len() > budget {
            words.push(format!("=?utf-8?q?{cur}?="));
            cur.clear();
            cur_len = 0;
        }
        cur.push_str(&piece);
        cur_len += piece.len();
    }
    if cur_len > 0 {
        words.push(format!("=?utf-8?q?{cur}?="));
    }
    // Encoded words on continuation lines are separated by CRLF+WSP per
    // RFC 2047 §2.
    words.join("\r\n ")
}

/// Request body for the Resend send endpoint. Field names match the API's
/// JSON contract (`from`/`to`/`subject`/`html`).
#[derive(Debug, serde::Serialize)]
struct SendEmailRequest {
    from: String,
    to: Vec<String>,
    subject: String,
    html: String,
}

/// Assembles the email request for an invitation. Separated from
/// [`EmailService::send_invitation_email`] so the sanitization behavior is
/// unit-testable without needing to mock the Resend SDK or an SMTP server.
fn build_invitation_params(
    from: &str,
    to: &str,
    inviter_name: &str,
    workspace_name: &str,
    invite_url: &str,
) -> SendEmailRequest {
    let safe_workspace = escape_html(workspace_name);
    let safe_inviter = escape_html(inviter_name);
    let subject_inviter = sanitize_subject_field(inviter_name);
    let subject_workspace = sanitize_subject_field(workspace_name);

    SendEmailRequest {
        from: from.to_string(),
        to: vec![to.to_string()],
        subject: format!("{subject_inviter} invited you to {subject_workspace} on Cordy"),
        html: format!(
            r#"<div style="font-family: sans-serif; max-width: 480px; margin: 0 auto;">
				<h2>You're invited to join {safe_workspace}</h2>
				<p><strong>{safe_inviter}</strong> invited you to collaborate in the <strong>{safe_workspace}</strong> workspace on Cordy.</p>
				<p style="margin: 24px 0;">
					<a href="{invite_url}" style="display: inline-block; padding: 12px 24px; background: #000; color: #fff; text-decoration: none; border-radius: 6px; font-weight: 500;">Accept invitation</a>
				</p>
				<p style="color: #666; font-size: 14px;">You'll need to log in to accept or decline the invitation.</p>
			</div>"#
        ),
    }
}

// ---------------------------------------------------------------------------
// Minimal async SMTP client (the net/smtp subset this service needs)
// ---------------------------------------------------------------------------

/// One parsed SMTP reply: status code plus per-line text (multi-line replies
/// keep their lines for EHLO extension parsing).
#[derive(Debug)]
struct SmtpReply {
    code: u16,
    lines: Vec<String>,
}

impl SmtpReply {
    fn text_joined(&self) -> String {
        self.lines.join("\n")
    }
}

/// Case-insensitive extension lookup over the EHLO lines, mirroring
/// `smtp.Client.Extension` (strings.EqualFold on the first token). Returns
/// the parameters after the keyword — e.g. the mechanism list for AUTH.
fn extension_params(ehlo_lines: &[String], name: &str) -> Option<String> {
    ehlo_lines
        .iter()
        .skip(1) // first 250 line is the server greeting, not an extension
        .find(|l| {
            l.split_whitespace()
                .next()
                .is_some_and(|kw| kw.eq_ignore_ascii_case(name))
        })
        .map(|l| {
            let kw_end = l.find(char::is_whitespace).unwrap_or(l.len());
            l[kw_end..].trim().to_string()
        })
}

#[derive(Debug)]
struct AcceptAnyCert;

impl ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ED25519,
        ]
    }
}

fn tls_connector(insecure: bool) -> anyhow::Result<TlsConnector> {
    // Both crypto providers land in the graph (aws-lc-rs via rustls defaults,
    // ring via reqwest's hyper-rustls); the builder must be told which to use.
    let builder = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("default protocol versions are always supported");
    let config = if insecure {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCert))
            .with_no_client_auth()
    } else {
        let mut roots = RootCertStore::empty();
        // Individual unparsable system certs are skipped, matching Go's
        // x509.SystemCertPool tolerance.
        for cert in rustls_native_certs::load_native_certs().certs {
            let _ = roots.add(cert);
        }
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    Ok(TlsConnector::from(Arc::new(config)))
}

enum SmtpStream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for SmtpStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            SmtpStream::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            SmtpStream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for SmtpStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match &mut *self {
            SmtpStream::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            SmtpStream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            SmtpStream::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            SmtpStream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            SmtpStream::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            SmtpStream::Tls(s) => std::pin::Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// A live SMTP session over plain TCP or TLS, with manual receive buffering
/// so reply lines can be parsed across packet boundaries.
struct SmtpConn {
    stream: SmtpStream,
    rbuf: Vec<u8>,
    tls_active: bool,
    ehlo_lines: Vec<String>,
}

impl SmtpConn {
    async fn dial(
        host: &str,
        port: &str,
        implicit_tls: bool,
        insecure: bool,
    ) -> anyhow::Result<Self> {
        let addr = format!("{host}:{port}");
        let tcp = tokio::time::timeout(DIAL_TIMEOUT, TcpStream::connect(&addr))
            .await
            .map_err(|_| anyhow!("smtp dial {addr}: timeout"))??;

        let (stream, tls_active) = if implicit_tls {
            let connector = tls_connector(insecure)?;
            let name = ServerName::try_from(host.to_string())
                .map_err(|e| anyhow!("smtp tls server name {host:?}: {e}"))?;
            let tls = connector
                .connect(name, tcp)
                .await
                .map_err(|e| anyhow!("smtp dial {addr}: tls: {e}"))?;
            (SmtpStream::Tls(Box::new(tls)), true)
        } else {
            (SmtpStream::Plain(tcp), false)
        };

        let mut c = Self {
            stream,
            rbuf: Vec::new(),
            tls_active,
            ehlo_lines: Vec::new(),
        };
        c.read_reply().await?; // 220 greeting — read by Go's smtp.NewClient too
        Ok(c)
    }

    async fn read_line(&mut self) -> std::io::Result<String> {
        loop {
            if let Some(pos) = self.rbuf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.rbuf.drain(..=pos).collect();
                let s = String::from_utf8_lossy(&line);
                return Ok(s.trim_end_matches(['\r', '\n']).to_string());
            }
            let mut chunk = [0u8; 4096];
            let n = match tokio::time::timeout(IO_TIMEOUT, self.stream.read(&mut chunk)).await {
                Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "smtp read timeout",
                    ))
                }
                Ok(r) => r?,
            };
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "smtp connection closed",
                ));
            }
            self.rbuf.extend_from_slice(&chunk[..n]);
        }
    }

    async fn read_reply(&mut self) -> anyhow::Result<SmtpReply> {
        let mut lines = Vec::new();
        let code;
        loop {
            let line = self.read_line().await?;
            if line.len() < 3 {
                bail!("smtp short reply: {line:?}");
            }
            let c: u16 = line[..3]
                .parse()
                .map_err(|_| anyhow!("smtp bad reply code: {line:?}"))?;
            match line.as_bytes().get(3) {
                None => {
                    lines.push(String::new());
                    code = c;
                    break;
                }
                Some(b'-') => lines.push(line[4..].to_string()),
                Some(_) => {
                    lines.push(line[4..].to_string());
                    code = c;
                    break;
                }
            }
        }
        Ok(SmtpReply { code, lines })
    }

    async fn write_raw(&mut self, data: &[u8]) -> anyhow::Result<()> {
        tokio::time::timeout(IO_TIMEOUT, self.stream.write_all(data))
            .await
            .map_err(|_| anyhow!("smtp write timeout"))??;
        tokio::time::timeout(IO_TIMEOUT, self.stream.flush())
            .await
            .map_err(|_| anyhow!("smtp flush timeout"))??;
        Ok(())
    }

    /// Sends a command and requires a 2xx reply. Errors carry the server's
    /// own text (code included), which the PLAIN→LOGIN fallback matches on.
    async fn cmd(&mut self, cmd: &str) -> anyhow::Result<SmtpReply> {
        self.write_raw(format!("{cmd}\r\n").as_bytes()).await?;
        let r = self.read_reply().await?;
        if (200..300).contains(&r.code) {
            Ok(r)
        } else {
            Err(anyhow!("{} {}", r.code, r.text_joined()))
        }
    }

    /// EHLO with HELO fallback (`smtp.Client.Hello`). Records the extension
    /// lines for later lookups.
    async fn hello(&mut self, local_name: &str) -> anyhow::Result<()> {
        match self.cmd(&format!("EHLO {local_name}")).await {
            Ok(r) => {
                self.ehlo_lines = r.lines;
                Ok(())
            }
            Err(ehlo_err) => {
                let r = self
                    .cmd(&format!("HELO {local_name}"))
                    .await
                    .map_err(|_| ehlo_err)?;
                self.ehlo_lines = r.lines;
                Ok(())
            }
        }
    }

    /// Consumes the plain connection and rebuilds it over TLS
    /// (`smtp.Client.StartTLS`). Receive state is dropped — legal because the
    /// caller has fully consumed the STARTTLS 220 reply.
    async fn upgrade_starttls(self, host: &str, insecure: bool) -> anyhow::Result<Self> {
        let SmtpStream::Plain(tcp) = self.stream else {
            return Ok(self);
        };
        let connector = tls_connector(insecure)?;
        let name = ServerName::try_from(host.to_string())
            .map_err(|e| anyhow!("smtp tls server name {host:?}: {e}"))?;
        let tls = connector
            .connect(name, tcp)
            .await
            .map_err(|e| anyhow!("smtp starttls: {e}"))?;
        Ok(Self {
            stream: SmtpStream::Tls(Box::new(tls)),
            rbuf: Vec::new(),
            tls_active: true,
            ehlo_lines: Vec::new(),
        })
    }

    /// AUTH PLAIN with Go's `smtp.PlainAuth` guard: refuse to send the
    /// credential over an unencrypted connection to anything but localhost.
    async fn auth_plain(
        &mut self,
        host: &str,
        username: &str,
        password: &str,
    ) -> anyhow::Result<()> {
        if !self.tls_active && !is_localhost(host) {
            bail!("unencrypted connection");
        }
        let cred = format!("\u{0}{username}\u{0}{password}");
        let b64 = base64::engine::general_purpose::STANDARD.encode(cred.as_bytes());
        self.cmd(&format!("AUTH PLAIN {b64}")).await?;
        Ok(())
    }

    /// AUTH LOGIN challenge/response exchange (`loginAuth`). Challenge
    /// answers are base64-encoded on the wire, exactly as net/smtp encodes
    /// whatever `Auth.Next` returns.
    async fn auth_login(&mut self, username: &str, password: &str) -> anyhow::Result<()> {
        self.write_raw(b"AUTH LOGIN\r\n").await?;

        let r = self.read_reply().await?;
        if r.code != 334 {
            bail!("smtp auth login: {} {}", r.code, r.text_joined());
        }
        let answer = login_challenge_response(r.text_joined().as_bytes(), username, password)
            .map_err(|e| anyhow!("smtp auth login: {e}"))?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&answer);
        self.write_raw(format!("{b64}\r\n").as_bytes()).await?;

        let r = self.read_reply().await?;
        if r.code != 334 {
            bail!("smtp auth login: {} {}", r.code, r.text_joined());
        }
        let answer = login_challenge_response(r.text_joined().as_bytes(), username, password)
            .map_err(|e| anyhow!("smtp auth login: {e}"))?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&answer);
        self.write_raw(format!("{b64}\r\n").as_bytes()).await?;

        let r = self.read_reply().await?;
        if !(200..300).contains(&r.code) {
            bail!("smtp auth login: {} {}", r.code, r.text_joined());
        }
        Ok(())
    }

    async fn mail_from(&mut self, from: &str) -> anyhow::Result<()> {
        self.cmd(&format!("MAIL FROM:<{from}>"))
            .await
            .map_err(|e| anyhow!("smtp MAIL FROM: {e:#}"))?;
        Ok(())
    }

    async fn rcpt_to(&mut self, to: &str) -> anyhow::Result<()> {
        self.cmd(&format!("RCPT TO:<{to}>"))
            .await
            .map_err(|e| anyhow!("smtp RCPT TO <{to}>: {e:#}"))?;
        Ok(())
    }

    /// DATA phase: 354 intermediate reply, dot-stuffed payload, terminating
    /// ".", then the final 250.
    async fn data(&mut self, payload: &[u8]) -> anyhow::Result<()> {
        self.write_raw(b"DATA\r\n").await?;
        let r = self.read_reply().await?;
        if r.code != 354 {
            bail!("smtp DATA: {} {}", r.code, r.text_joined());
        }
        let mut out = dot_stuff(payload);
        out.extend_from_slice(b".\r\n");
        self.write_raw(&out)
            .await
            .map_err(|e| anyhow!("smtp write body: {e:#}"))?;
        // Final reply after the terminating "." — cmd("") sends nothing and
        // just reads.
        self.write_raw(b"").await?;
        let r = self.read_reply().await?;
        if !(200..300).contains(&r.code) {
            bail!("smtp end data: {} {}", r.code, r.text_joined());
        }
        Ok(())
    }

    async fn quit(&mut self) -> anyhow::Result<()> {
        self.cmd("QUIT").await?;
        Ok(())
    }
}

/// SMTP dot-stuffing, mirroring net/textproto's dotWriter: transmission
/// lines starting with '.' get another '.' prepended so the receiver can spot
/// the bare "." terminator, and every bare LF is promoted to CRLF (a CR that
/// is already followed by LF passes through untouched). The result always
/// ends with CRLF so the terminator is well-formed even for input lacking a
/// trailing newline.
fn dot_stuff(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 16);
    let mut at_line_start = true;
    let mut prev_was_cr = false;
    for &b in body {
        if at_line_start && b == b'.' {
            out.push(b'.');
        }
        if b == b'\n' && !prev_was_cr {
            out.push(b'\r');
        }
        out.push(b);
        prev_was_cr = b == b'\r';
        at_line_start = b == b'\n';
    }
    if !out.ends_with(b"\r\n") {
        if out.last() == Some(&b'\n') {
            let n = out.len();
            out[n - 1] = b'\r';
            out.push(b'\n');
        } else {
            out.extend_from_slice(b"\r\n");
        }
    }
    out
}

// ---------------------------------------------------------------------------
// EmailService
// ---------------------------------------------------------------------------

impl EmailService {
    pub fn new() -> Self {
        let api_key = std::env::var("RESEND_API_KEY").unwrap_or_default();
        let smtp_host = std::env::var("SMTP_HOST")
            .unwrap_or_default()
            .trim()
            .to_string();
        let mut smtp_port = std::env::var("SMTP_PORT")
            .unwrap_or_default()
            .trim()
            .to_string();
        if smtp_port.is_empty() {
            smtp_port = "25".to_string();
        }
        let smtp_username = std::env::var("SMTP_USERNAME").unwrap_or_default();
        let smtp_password = std::env::var("SMTP_PASSWORD").unwrap_or_default();
        let smtp_tls_insecure = std::env::var("SMTP_TLS_INSECURE").unwrap_or_default() == "true";
        let from = resolve_from_email(&smtp_host);

        // EHLO/HELO name, only relevant on the SMTP relay send path. net/smtp
        // defaults to "localhost", which strict relays (e.g.
        // smtp-relay.gmail.com) reject from a public source. Fall back to the
        // machine hostname when SMTP_EHLO_NAME is unset. Resolved only in
        // SMTP mode so the Resend/DEV paths never touch hostname resolution
        // or emit its failure log.
        let mut smtp_ehlo_name = String::new();
        if !smtp_host.is_empty() {
            smtp_ehlo_name = std::env::var("SMTP_EHLO_NAME")
                .unwrap_or_default()
                .trim()
                .to_string();
            if smtp_ehlo_name.is_empty() {
                match hostname::get() {
                    Ok(h) => smtp_ehlo_name = h.to_string_lossy().trim().to_string(),
                    Err(err) => {
                        // Empty name makes sendSMTP skip Hello() and fall back
                        // to net/smtp's lazy "localhost" — which strict relays
                        // reject. Surface it so operators know to set
                        // SMTP_EHLO_NAME explicitly.
                        println!(
                            "EmailService: os.Hostname() failed ({err}); SMTP EHLO falls back to \"localhost\" — set SMTP_EHLO_NAME for strict relays"
                        );
                    }
                }
            }
        }

        // SMTP_TLS=implicit forces an immediate TLS handshake on connect
        // (SMTPS). Required by providers like Aliyun enterprise mail that
        // only offer port 465 SSL and do not advertise STARTTLS. Default
        // (empty / "starttls") preserves the prior STARTTLS-upgrade behavior.
        let smtp_tls_mode = std::env::var("SMTP_TLS")
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let mut smtp_tls_implicit =
            smtp_tls_mode == "implicit" || smtp_tls_mode == "smtps" || smtp_tls_mode == "ssl";
        if smtp_tls_mode.is_empty() && smtp_port == "465" {
            smtp_tls_implicit = true;
        }
        if !smtp_tls_mode.is_empty() && !smtp_tls_implicit && smtp_tls_mode != "starttls" {
            println!(
                "EmailService: SMTP_TLS={smtp_tls_mode:?} not recognized, falling back to starttls"
            );
        }

        let (resend_api_key, resend_http) = if api_key.is_empty() {
            (None, None)
        } else {
            (Some(api_key.clone()), Some(reqwest::Client::new()))
        };

        match (!smtp_host.is_empty(), resend_api_key.is_some()) {
            (true, _) => {
                let tls_label = if smtp_tls_implicit {
                    "implicit-tls"
                } else {
                    "starttls"
                };
                println!(
                    "EmailService: SMTP relay {}:{} ({}) from={from}",
                    smtp_host, smtp_port, tls_label
                );
            }
            (false, true) => println!("EmailService: Resend API from={from}"),
            (false, false) => println!(
                "EmailService: DEV mode — codes printed to stdout (set CORDY_DEV_VERIFICATION_CODE in .env for a fixed local code)"
            ),
        }

        Self {
            resend_api_key,
            resend_http,
            from_email: from,
            smtp_host,
            smtp_port,
            smtp_username,
            smtp_password,
            smtp_tls_insecure,
            smtp_tls_implicit,
            smtp_ehlo_name,
        }
    }

    /// Opens the SMTP session: dial (implicit TLS optional) → greeting →
    /// EHLO → optional STARTTLS upgrade → re-EHLO. Mirrors
    /// `EmailService.openSMTPClient`.
    async fn open_smtp_client(&self) -> anyhow::Result<SmtpConn> {
        let mut c = SmtpConn::dial(
            &self.smtp_host,
            &self.smtp_port,
            self.smtp_tls_implicit,
            self.smtp_tls_insecure,
        )
        .await?;
        let hello_name: &str = if self.smtp_ehlo_name.is_empty() {
            "localhost"
        } else {
            &self.smtp_ehlo_name
        };
        c.hello(hello_name).await?;

        if !self.smtp_tls_implicit && extension_params(&c.ehlo_lines, "STARTTLS").is_some() {
            c = c
                .upgrade_starttls(&self.smtp_host, self.smtp_tls_insecure)
                .await?;
            c.hello(hello_name).await?;
        }
        Ok(c)
    }

    /// Delivers an HTML email via the configured SMTP server. Supports
    /// unauthenticated relay (SMTP_USERNAME empty) and authenticated SMTP,
    /// upgrading to STARTTLS when advertised. Set SMTP_TLS_INSECURE=true for
    /// self-signed or private CA certificates.
    async fn send_smtp(&self, to: &str, subject: &str, html_body: &str) -> anyhow::Result<()> {
        if self.from_email.trim().is_empty() {
            bail!("SMTP_FROM_EMAIL or RESEND_FROM_EMAIL is required when SMTP_HOST is set");
        }

        let mut c = self.open_smtp_client().await?;

        if !self.smtp_username.is_empty() {
            let plain_res = c
                .auth_plain(&self.smtp_host, &self.smtp_username, &self.smtp_password)
                .await;
            if let Err(auth_err) = plain_res {
                let auth_ext = extension_params(&c.ehlo_lines, "AUTH");
                if !should_fallback_to_login(&auth_err.to_string(), auth_ext.as_deref()) {
                    return Err(anyhow!("smtp auth: {auth_err:#}"));
                }

                // The PLAIN attempt may have left the session in a weird
                // state; reconnect and retry with LOGIN from a clean slate.
                drop(c);
                let mut c2 = self.open_smtp_client().await.map_err(|e| {
                    anyhow!(
                        "smtp auth: plain auth failed ({auth_err}); login reconnect failed: {e:#}"
                    )
                })?;
                c2.auth_login(&self.smtp_username, &self.smtp_password)
                    .await
                    .map_err(|e| {
                        anyhow!(
                            "smtp auth: plain auth failed ({auth_err}); login auth fallback failed: {e:#}"
                        )
                    })?;
                c = c2;
            }
        }

        // Probe 8BITMIME after (possible) STARTTLS so the extension list is
        // current. Use quoted-printable for relays that don't advertise
        // 8BITMIME — safer for non-ASCII workspace/inviter names crossing
        // strict or older SMTP hops.
        let has_8bit = extension_params(&c.ehlo_lines, "8BITMIME").is_some();
        let encoded_subject = q_encode_utf8(subject);
        let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let msg_id = format!("<{nanos}@{}>", self.smtp_host);

        let (body_bytes, cte): (Vec<u8>, &str) = if has_8bit {
            (html_body.as_bytes().to_vec(), "8bit")
        } else {
            (
                quoted_printable::encode_to_str(html_body).into_bytes(),
                "quoted-printable",
            )
        };

        c.mail_from(&self.from_email).await?;
        c.rcpt_to(to).await?;

        let date = Utc::now().format("%a, %d %b %Y %H:%M:%S %z");
        let headers = format!(
            "From: {}\r\nTo: {}\r\nSubject: {}\r\nDate: {date}\r\nMessage-ID: {msg_id}\r\nMIME-Version: 1.0\r\nContent-Type: text/html; charset=UTF-8\r\nContent-Transfer-Encoding: {cte}\r\n\r\n",
            self.from_email, to, encoded_subject
        );
        let mut payload = headers.into_bytes();
        payload.extend_from_slice(&body_bytes);

        c.data(&payload).await?;
        c.quit().await
    }

    async fn resend_send(&self, req: SendEmailRequest) -> anyhow::Result<()> {
        let key = self.resend_api_key.as_deref().expect("checked by caller");
        let http = self.resend_http.as_ref().expect("checked by caller");
        let resp = http
            .post(RESEND_API_URL)
            .bearer_auth(key)
            .json(&req)
            .send()
            .await
            .map_err(|e| anyhow!("resend send: {e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("resend send: {status} {body}");
        }
        Ok(())
    }

    /// Sends a one-time login code. The code is server-generated (6-digit
    /// numeric) so no user-controlled text reaches the email body here.
    /// Delivery priority: SMTP relay → Resend API → DEV stdout.
    pub async fn send_verification_code(&self, to: &str, code: &str) -> anyhow::Result<()> {
        let body = format!(
            r#"<div style="font-family: sans-serif; max-width: 400px; margin: 0 auto;">
			<h2>Your verification code</h2>
			<p style="font-size: 32px; font-weight: bold; letter-spacing: 8px; margin: 24px 0;">{code}</p>
			<p>This code expires in 10 minutes.</p>
			<p style="color: #666; font-size: 14px;">If you didn't request this code, you can safely ignore this email.</p>
		</div>"#
        );

        if !self.smtp_host.is_empty() {
            return self
                .send_smtp(to, "Your Cordy verification code", &body)
                .await;
        }
        if self.resend_api_key.is_none() {
            println!("[DEV] Verification code for {to}: {code}");
            return Ok(());
        }
        let req = SendEmailRequest {
            from: self.from_email.clone(),
            to: vec![to.to_string()],
            subject: "Your Cordy verification code".to_string(),
            html: body,
        };
        self.resend_send(req).await
    }

    /// Notifies the invitee that they have been invited to a workspace.
    /// invitationID is included in the URL so the email deep-links to
    /// /invite/{id}.
    pub async fn send_invitation_email(
        &self,
        to: &str,
        inviter_name: &str,
        workspace_name: &str,
        invitation_id: &str,
    ) -> anyhow::Result<()> {
        let mut app_url = std::env::var("FRONTEND_ORIGIN")
            .unwrap_or_default()
            .trim()
            .to_string();
        if app_url.is_empty() {
            app_url = "https://cordy.ai".to_string();
        }
        let invite_url = format!("{app_url}/invite/{invitation_id}");

        if !self.smtp_host.is_empty() {
            let params = build_invitation_params(
                &self.from_email,
                to,
                inviter_name,
                workspace_name,
                &invite_url,
            );
            return self.send_smtp(to, &params.subject, &params.html).await;
        }
        if self.resend_api_key.is_none() {
            println!(
                "[DEV] Invitation email to {to}: {inviter_name} invited you to {workspace_name} — {invite_url}"
            );
            return Ok(());
        }
        let params = build_invitation_params(
            &self.from_email,
            to,
            inviter_name,
            workspace_name,
            &invite_url,
        );
        self.resend_send(params).await
    }
}

impl Default for EmailService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_localhost_matches_the_three_shapes() {
        assert!(is_localhost("localhost"));
        assert!(is_localhost("127.0.0.1"));
        assert!(is_localhost("::1"));
        assert!(!is_localhost("smtp.example.com"));
        assert!(!is_localhost("localhost.evil.com"));
    }

    #[test]
    fn login_challenge_recognizes_username_and_password() {
        let user = login_challenge_response(b"Username:", "u", "p").unwrap();
        assert_eq!(user, b"u");
        let pass = login_challenge_response(b"Password:", "u", "p").unwrap();
        assert_eq!(pass, b"p");
        // "user name" two-word variant.
        let alt = login_challenge_response(b"User Name:", "u", "p").unwrap();
        assert_eq!(alt, b"u");
    }

    #[test]
    fn login_challenge_decodes_base64_wrapped_challenges() {
        use base64::Engine as _;
        let wrapped = base64::engine::general_purpose::STANDARD.encode(b"Password:");
        let pass = login_challenge_response(wrapped.as_bytes(), "u", "p").unwrap();
        assert_eq!(pass, b"p");
    }

    #[test]
    fn login_challenge_rejects_unknown_prompts() {
        let err = login_challenge_response(b"CAPTCHA:", "u", "p").unwrap_err();
        assert!(err.contains("unexpected LOGIN challenge"));
    }

    #[test]
    fn fallback_only_for_unrecognized_auth_type_with_login_advertised() {
        assert!(should_fallback_to_login(
            "504 5.7.4 Unrecognized authentication type",
            Some("LOGIN PLAIN XOAUTH2")
        ));
        assert!(should_fallback_to_login(
            "server says: 504 5.7.4 something",
            Some("LOGIN")
        ));
        // Other failures must surface unchanged.
        assert!(!should_fallback_to_login(
            "535 5.7.8 Bad credentials",
            Some("LOGIN PLAIN")
        ));
        assert!(!should_fallback_to_login(
            "unencrypted connection",
            Some("LOGIN")
        ));
        // Right error, but server never advertised AUTH or lacks LOGIN.
        assert!(!should_fallback_to_login(
            "504 5.7.4 Unrecognized authentication type",
            None
        ));
        assert!(!should_fallback_to_login(
            "504 5.7.4 Unrecognized authentication type",
            Some("PLAIN CRAM-MD5")
        ));
    }

    #[test]
    fn escape_html_matches_go_escape_string() {
        assert_eq!(
            escape_html("<script>\"x\" &' '</script>"),
            "&lt;script&gt;&#34;x&#34; &amp;&#39; &#39;&lt;/script&gt;"
        );
        assert_eq!(escape_html("plain"), "plain");
    }

    #[test]
    fn sanitize_strips_control_characters() {
        assert_eq!(sanitize_subject_field("a\nb\rc\td\u{0}e"), "abcde");
    }

    #[test]
    fn sanitize_caps_at_sixty_runes_with_ellipsis() {
        let sixty: String = "é".repeat(60);
        assert_eq!(sanitize_subject_field(&sixty), sixty);
        let sixty_one: String = "é".repeat(61);
        let out = sanitize_subject_field(&sixty_one);
        assert_eq!(out.chars().count(), 60);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().take(59).count(), 59);
    }

    #[test]
    fn q_encoding_passes_ascii_through() {
        assert_eq!(q_encode_utf8("Hello world 123!"), "Hello world 123!");
    }

    #[test]
    fn q_encoding_encodes_non_ascii_and_specials() {
        let out = q_encode_utf8("café");
        assert_eq!(out, "=?utf-8?q?caf=C3=A9?=");
        // Space folds to underscore inside encoded words.
        assert_eq!(
            q_encode_utf8("你好 世界"),
            "=?utf-8?q?=E4=BD=A0=E5=A5=BD_=E4=B8=96=E7=95=8C?="
        );
        // '=' forces encoding even though ASCII.
        assert_eq!(q_encode_utf8("a=b"), "=?utf-8?q?a=3Db?=");
    }

    #[test]
    fn q_encoding_folds_long_subjects_into_multiple_words() {
        let long = "你".repeat(60); // 180 UTF-8 bytes → 540 escape chars
        let out = q_encode_utf8(&long);
        for word in out.split("\r\n ") {
            assert!(word.len() <= 75, "word too long: {word}");
            assert!(word.starts_with("=?utf-8?q?") && word.ends_with("?="));
        }
        // Round-trip: unfold, then decode every word's inline =XX pairs.
        let joined: String = out.replace("\r\n ", "");
        let mut decoded: Vec<u8> = Vec::new();
        for word in joined.split("=?utf-8?q?") {
            let word = word.strip_suffix("?=").unwrap_or(word);
            let bytes = word.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'=' {
                    decoded.push(
                        u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap(), 16)
                            .unwrap(),
                    );
                    i += 3;
                } else {
                    decoded.push(bytes[i]);
                    i += 1;
                }
            }
        }
        assert_eq!(String::from_utf8(decoded).unwrap(), long);
    }

    #[test]
    fn invitation_params_escapes_html_but_not_subject() {
        let req = build_invitation_params(
            "noreply@cordy.ai",
            "bob@example.com",
            "Eve <script>",
            "Acme & Co \"inc\"",
            "https://cordy.ai/invite/x",
        );
        // Body escapes everything dangerous.
        assert!(req.html.contains("Eve &lt;script&gt;"));
        assert!(req.html.contains("Acme &amp; Co &#34;inc&#34;"));
        assert!(req.html.contains(r#"href="https://cordy.ai/invite/x""#));
        // Subject is sanitized (control chars stripped, length capped), not
        // HTML-escaped.
        assert_eq!(
            req.subject,
            "Eve <script> invited you to Acme & Co \"inc\" on Cordy"
        );
        // Resend API JSON field names.
        let v = serde_json::to_value(&req).unwrap();
        assert!(v.get("from").is_some());
        assert!(v.get("to").is_some());
        assert!(v.get("subject").is_some());
        assert!(v.get("html").is_some());
    }

    #[test]
    fn dot_stuffing_doubles_leading_dots_and_normalizes_trailing_crlf() {
        assert_eq!(dot_stuff(b"..hidden"), b"...hidden\r\n");
        assert_eq!(dot_stuff(b"line1\n.line2\n"), b"line1\r\n..line2\r\n");
        assert_eq!(dot_stuff(b"no newline"), b"no newline\r\n");
        assert_eq!(dot_stuff(b"a\nb"), b"a\r\nb\r\n");
        assert_eq!(dot_stuff(b""), b"\r\n");
    }

    #[test]
    fn extension_lookup_is_case_insensitive_and_returns_params() {
        let lines = vec![
            "mail.example.com ESMTP".to_string(),
            "PIPELINING".to_string(),
            "AUTH LOGIN PLAIN XOAUTH2".to_string(),
            "auth login".to_string(),
        ];
        assert_eq!(
            extension_params(&lines, "auth").as_deref(),
            Some("LOGIN PLAIN XOAUTH2")
        );
        assert_eq!(extension_params(&lines, "pipelining"), Some(String::new()));
        assert_eq!(extension_params(&lines, "STARTTLS"), None);
    }
}
