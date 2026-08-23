//! The outbound sender and the per-installation Socket Mode connection. Port
//! of `server/internal/integrations/slack/channel.go` (sender) and
//! `slack_channel.go` (connection).
//!
//! Under the bring-your-own-app (BYO) model every Slack installation carries
//! its own Slack app — its own app-level token (`xapp-`, stored encrypted in
//! the installation config) — so it gets its own connection, exactly like the
//! stage-3 per-installation model and like Feishu today. The engine Supervisor
//! builds one SlackChannel per active Slack installation (via the registered
//! Factory) and owns the lease / reconnect lifecycle; connect blocks on the
//! receive loop.
//!
//! Inbound events are translated by the shared inbound helpers, parameterized
//! by THIS installation's bot user id, and handed to the engine router, which
//! resolves the installation by the event's api_app_id — equal to this app's
//! id, the per-app routing key. Outbound replies primarily flow through the
//! chat:done subscriber (outbound.rs); send satisfies the Channel contract and
//! posts with this installation's bot token.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use cordy_channel::{
    Capability, Config as ChannelConfig, InboundHandler, OutboundMessage, SendResult, Type,
};

use crate::client::SlackClient;
use crate::config::{decrypt_token, Decrypter, InstallConfig};
use crate::inbound::{
    compile_mention_re, inbound_from_app_mention, inbound_from_message, AppMentionEvent,
    EventsApiEnvelope, MessageEvent,
};
use crate::mrkdwn::format_mrkdwn;
use crate::slash_command::{SlashCommand, SlashCommandProcessor};
use crate::TYPE_SLACK;

/// Caps a single outbound chat.postMessage body. Slack hard-caps a message
/// around 40k characters; we chunk below that with headroom.
const MAX_MESSAGE_RUNES: usize = 38000;

/// Bounds the detached processing of one `/issue` slash command (installation +
/// identity resolution, issue creation, response_url reply). It runs off the
/// socket receive loop on its own context, so a slow DB or Slack HTTP call
/// cannot wedge event delivery.
const SLASH_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The responder signature: posts an ephemeral reply to a command's
/// response_url. A boxed future keeps the injection point object-safe.
pub type RespondFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>;
pub type Responder = Arc<dyn Fn(CancellationToken, String, String) -> RespondFuture + Send + Sync>;

/// Posts agent replies back to Slack via chat.postMessage. It is the OUTBOUND
/// half: it holds the per-installation bot token (`xoxb-`) the reply must be
/// sent with (inbound runs on the per-installation Socket Mode connection).
/// The installation identity (workspace / agent / installer) is resolved per
/// message by the Router, so it is absent here.
pub struct SlackSender {
    api: SlackClient,
}

impl SlackSender {
    /// Builds a Send-only client from a decoded bot token. Kept separate from
    /// the outbound subscriber so tests can point it at a local stub server.
    pub fn new(bot_token: &str) -> Self {
        Self {
            api: SlackClient::new(bot_token),
        }
    }

    #[cfg(test)]
    pub fn with_api_url(bot_token: &str, api_url: &str) -> Self {
        Self {
            api: SlackClient::with_api_url(bot_token, api_url),
        }
    }

    /// Delivers a minimal text reply via chat.postMessage, threading into
    /// out.thread_id when set so a decoupled reply lands back in the
    /// originating thread. Long bodies are chunked under Slack's per-message
    /// cap; the returned SendResult carries the timestamp of the LAST posted
    /// chunk.
    pub async fn send(
        &self,
        ctx: CancellationToken,
        out: OutboundMessage,
    ) -> anyhow::Result<SendResult> {
        let thread_ts = outbound_thread_ts(&out);
        let mut last_ts = String::new();
        // Convert the agent's standard Markdown to Slack mrkdwn before posting
        // so bold/headers/links render instead of showing literal markup.
        for chunk in chunk_message(&format_mrkdwn(&out.text), MAX_MESSAGE_RUNES) {
            if ctx.is_cancelled() {
                anyhow::bail!("slack: send cancelled");
            }
            last_ts = self
                .api
                .chat_post_message(&out.chat_id, &chunk, &thread_ts, "")
                .await
                .map_err(|e| anyhow::anyhow!("slack: chat.postMessage: {e}"))?;
        }
        Ok(SendResult {
            message_id: last_ts,
        })
    }
}

/// Picks the Slack thread_ts for an outbound reply: an explicit quote target
/// wins, else the thread the inbound message belonged to.
fn outbound_thread_ts(out: &OutboundMessage) -> String {
    if !out.reply_to.is_empty() {
        return out.reply_to.clone();
    }
    out.thread_id.clone()
}

/// Splits text into <=max_runes-rune pieces on rune boundaries so a long agent
/// reply does not exceed Slack's per-message cap. An empty body yields a
/// single empty chunk (Slack rejects truly empty text, but the caller guards
/// against that upstream).
fn chunk_message(text: &str, max_runes: usize) -> Vec<String> {
    if max_runes == 0 || text.chars().count() <= max_runes {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut n = 0usize;
    for c in text.chars() {
        if n == max_runes {
            chunks.push(std::mem::take(&mut current));
            n = 0;
        }
        current.push(c);
        n += 1;
    }
    chunks.push(current);
    chunks
}

/// The slash command payload Socket Mode delivers (slack-go SlashCommand). The
/// fields mirror the form-encoded POST body Slack documents; unknown fields are
/// ignored.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct SocketSlashCommand {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(rename = "team_id", default)]
    pub team_id: String,
    #[serde(rename = "api_app_id", default)]
    pub api_app_id: String,
    #[serde(rename = "response_url", default)]
    pub response_url: String,
}

impl From<&SocketSlashCommand> for SlashCommand {
    fn from(c: &SocketSlashCommand) -> Self {
        Self {
            command: c.command.clone(),
            text: c.text.clone(),
            user_id: c.user_id.clone(),
            team_id: c.team_id.clone(),
            api_app_id: c.api_app_id.clone(),
            response_url: c.response_url.clone(),
        }
    }
}

/// ONE installation's Socket Mode connection. The engine Supervisor builds one
/// per active Slack installation and owns reconnects.
pub struct SlackChannel {
    app_id: String,
    bot_user_id: String,
    /// Decrypted xapp- — authorizes the Socket Mode connection.
    app_token: String,
    /// Bot-token client for outbound send.
    bot_api: SlackClient,
    handler: Option<InboundHandler>,
    /// None disables /issue slash-command handling.
    slash: Option<Arc<SlashCommandProcessor>>,
}

#[async_trait]
impl cordy_channel::Channel for SlackChannel {
    fn r#type(&self) -> Type {
        Type(TYPE_SLACK.to_string())
    }

    fn capabilities(&self) -> Capability {
        Capability::TEXT | Capability::THREAD_REPLY
    }

    /// Opens this installation's Socket Mode connection (authenticated with
    /// its OWN app-level token) and runs the receive loop until ctx is
    /// cancelled or the link drops.
    async fn connect(&self, ctx: CancellationToken) -> anyhow::Result<()> {
        // The Socket Mode connection authenticates with the app-level token
        // alone; the bot token is only for outbound Web API calls.
        let app_client = SlackClient::new(&self.app_token);
        let ws = crate::socket_mode::SocketModeStream::dial(&app_client, ctx.clone()).await?;
        self.connect_impl(ctx, ws).await
    }

    /// A no-op: the Socket Mode connection's whole lifetime is scoped to
    /// connect (it returns when the token is cancelled), so there is no
    /// long-lived resource to release here. Mirrors feishuChannel.Disconnect.
    async fn disconnect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Posts an outbound reply with this installation's bot token, reusing the
    /// shared sender (Markdown→mrkdwn, chunking, threading).
    async fn send(&self, out: OutboundMessage) -> anyhow::Result<SendResult> {
        self.bot_send(CancellationToken::new(), out).await
    }
}

impl SlackChannel {
    async fn bot_send(
        &self,
        ctx: CancellationToken,
        out: OutboundMessage,
    ) -> anyhow::Result<SendResult> {
        let thread_ts = outbound_thread_ts(&out);
        let mut last_ts = String::new();
        for chunk in chunk_message(&format_mrkdwn(&out.text), MAX_MESSAGE_RUNES) {
            if ctx.is_cancelled() {
                anyhow::bail!("slack: send cancelled");
            }
            last_ts = self
                .bot_api
                .chat_post_message(&out.chat_id, &chunk, &thread_ts, "")
                .await
                .map_err(|e| anyhow::anyhow!("slack: chat.postMessage: {e}"))?;
        }
        Ok(SendResult {
            message_id: last_ts,
        })
    }

    /// Opens this installation's Socket Mode connection (authenticated with
    /// its OWN app-level token) and runs the receive loop until ctx is
    /// cancelled or the link drops.
    ///
    /// Port note: Go drives slack-go's socketmode.Client event pump. Rust has
    /// no equivalent SDK, so this implementation mints websocket URLs through
    /// apps.connections.open and consumes the raw Socket Mode frames itself
    /// (hello / events_api / slash_commands / disconnect envelopes, ACK per
    /// envelope id). The frame protocol is JSON text frames:
    /// {"type":"events_api","envelope_id":..,"payload":{...}} etc.; each
    /// inbound envelope is ACKed with {"type":"ack","envelope_id":..} BEFORE
    /// any processing, mirroring Go's Ack-first ordering.
    async fn connect_impl(
        &self,
        ctx: CancellationToken,
        ws: crate::socket_mode::SocketModeStream,
    ) -> anyhow::Result<()> {
        if self.handler.is_none() {
            anyhow::bail!("slack: inbound handler not configured");
        }
        let mention_re = compile_mention_re(&self.bot_user_id);
        // Every exit path cancels run_ctx and waits for the run task to
        // observe it and exit, so a transient failure tears the live
        // connection down before the supervisor reconnects — no leaked socket
        // consuming events into an unread queue.
        let read_loop = tokio::select! {
            _ = ctx.cancelled() => return Ok(()),
            res = ws.run(|envelope| self.handle_envelope(envelope, &mention_re)) => res,
        };
        match read_loop {
            Ok(()) => Ok(()),
            Err(e) => {
                if ctx.is_cancelled() {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Translates one decoded Socket Mode envelope into engine calls. Returns
    /// whether to keep the connection alive; an infrastructure error
    /// propagates so the supervisor reconnects, while product drops are
    /// swallowed.
    async fn handle_envelope(
        &self,
        envelope: crate::socket_mode::Envelope,
        mention_re: &Option<regex::Regex>,
    ) -> anyhow::Result<()> {
        use crate::socket_mode::EnvelopeKind;
        match envelope.kind {
            EnvelopeKind::EventsApi => {
                let payload: EventsApiEnvelope =
                    serde_json::from_value(envelope.payload.clone())
                        .map_err(|e| anyhow::anyhow!("decode events api payload: {e}"))?;
                self.dispatch_events_api(&payload, mention_re).await?;
                Ok(())
            }
            EnvelopeKind::SlashCommand => {
                // Handling never fails the connection (product outcomes are
                // ephemeral replies, not infra errors).
                if let Ok(cmd) =
                    serde_json::from_value::<SocketSlashCommand>(envelope.payload.clone())
                {
                    self.dispatch_slash_command(cmd).await;
                }
                Ok(())
            }
            // hello/connecting/incoming-errors are lifecycle noise.
            EnvelopeKind::Other => Ok(()),
        }
    }

    /// Translates one Events API envelope to a normalized inbound message and
    /// hands it to the engine. A non-nil handler error is an infrastructure
    /// failure; it propagates so the supervisor reconnects. A legitimate
    /// product drop returns quietly.
    async fn dispatch_events_api(
        &self,
        e: &EventsApiEnvelope,
        mention_re: &Option<regex::Regex>,
    ) -> anyhow::Result<()> {
        let Some(handler) = &self.handler else {
            anyhow::bail!("slack: inbound handler not configured");
        };
        let inner = &e.event;
        let event_type = inner.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let msg = match event_type {
            "app_mention" => {
                let m: AppMentionEvent = serde_json::from_value(inner.clone())
                    .map_err(|err| anyhow::anyhow!("decode app_mention: {err}"))?;
                inbound_from_app_mention(e, &m, &self.bot_user_id, mention_re.as_ref())
            }
            // Every other inner type carrying a channel is treated as the
            // message event (message / message.channels / …); events without
            // one are lifecycle noise.
            _ if !inner.get("channel").is_none() => {
                let m: MessageEvent = serde_json::from_value(inner.clone())
                    .map_err(|err| anyhow::anyhow!("decode message: {err}"))?;
                inbound_from_message(e, &m, &self.bot_user_id, mention_re.as_ref())
            }
            _ => None,
        };
        if let Some(msg) = msg {
            handler.call(CancellationToken::new(), msg).await?;
        }
        Ok(())
    }

    /// Processes an already-ACKed `/issue` slash command on a detached task
    /// with its own bounded context, so the issue creation and response_url
    /// reply never block the socket receive loop (mirrors the router's
    /// detached outbound path). A nil processor (slash handling not wired)
    /// drops it.
    async fn dispatch_slash_command(&self, cmd: SocketSlashCommand) {
        let Some(slash) = &self.slash else {
            tracing::warn!(
                command = %cmd.command,
                app_id = %self.app_id,
                "slack: slash command received but no processor configured"
            );
            return;
        };
        let slash = Arc::clone(slash);
        let cmd = SlashCommand::from(&cmd);
        tokio::spawn(async move {
            let ctx = tokio_util::sync::CancellationToken::new();
            let cancel = ctx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(SLASH_COMMAND_TIMEOUT).await;
                cancel.cancel();
            });
            slash.handle(ctx, cmd).await;
        });
    }
}

/// Shared dependencies the Slack Factory closes over. The engine inbound
/// handler is supplied per-build via `ChannelConfig::handler`; the Decrypter
/// turns the installation's stored ciphertext tokens into plaintext.
pub struct ChannelDeps {
    pub decrypt: Option<Arc<Decrypter>>,
    /// Handles the `/issue` slash command delivered over Socket Mode. None
    /// leaves slash-command handling off (the connection still serves messages
    /// and @-mentions); tests that only exercise inbound messages pass None.
    pub slash: Option<Arc<SlashCommandProcessor>>,
}

/// Registers the per-installation Slack Factory so the engine.Supervisor
/// builds + supervises one SlackChannel per active Slack installation. Adding
/// Slack inbound is this call plus the adapter — no engine edit, the same
/// contract as `lark.RegisterFeishu`.
pub fn register_slack(reg: &cordy_channel::Registry, deps: ChannelDeps) {
    reg.register(Type(TYPE_SLACK.to_string()), new_slack_factory(deps));
}

pub fn new_slack_factory(deps: ChannelDeps) -> cordy_channel::Factory {
    Arc::new(move |cfg: ChannelConfig| {
        let decrypt = deps.decrypt.clone();
        let slash = deps.slash.clone();
        Box::pin(async move {
            let ic: InstallConfig = serde_json::from_value(cfg.raw.clone())
                .map_err(|e| anyhow::anyhow!("slack: decode installation config: {e}"))?;
            let app_token = decrypt_token(&ic.app_token_encrypted, decrypt.as_deref())
                .map_err(|e| anyhow::anyhow!("slack: decrypt app token: {e}"))?;
            if app_token.is_empty() {
                anyhow::bail!("slack: installation has no app-level token");
            }
            let bot_token = decrypt_token(&ic.bot_token_encrypted, decrypt.as_deref())
                .map_err(|e| anyhow::anyhow!("slack: decrypt bot token: {e}"))?;
            Ok(Arc::new(SlackChannel {
                app_id: ic.app_id,
                bot_user_id: ic.bot_user_id,
                app_token,
                bot_api: SlackClient::new(bot_token),
                handler: cfg.handler,
                slash,
            }) as cordy_channel::BuiltChannel)
        })
    })
}

// Referenced by tests of the factory path; keeps the field name documented.
#[allow(dead_code)]
fn _app_id_of(c: &SlackChannel) -> &str {
    &c.app_id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunking_splits_on_rune_boundaries_and_keeps_short_text_whole() {
        assert_eq!(chunk_message("hi", 10), vec!["hi".to_string()]);
        // Rune boundaries respected: é counts once, not two bytes.
        assert_eq!(
            chunk_message("éééé", 2),
            vec!["éé".to_string(), "éé".to_string()]
        );
        let long = "ab".repeat(100);
        let chunks = chunk_message(&long, 64);
        assert_eq!(chunks.len(), 4);
        assert!(chunks.iter().all(|c| c.chars().count() <= 64));
        assert_eq!(chunks.concat(), long);
    }

    #[test]
    fn thread_target_prefers_quote_over_thread() {
        let out = OutboundMessage {
            chat_id: "C1".into(),
            text: "x".into(),
            thread_id: "T1".into(),
            reply_to: "Q9".into(),
        };
        assert_eq!(outbound_thread_ts(&out), "Q9");
        let plain = OutboundMessage {
            chat_id: "C1".into(),
            text: "x".into(),
            thread_id: "T1".into(),
            reply_to: String::new(),
        };
        assert_eq!(outbound_thread_ts(&plain), "T1");
    }

    #[test]
    fn capabilities_declare_text_and_thread_reply_only() {
        use cordy_channel::Channel as _;
        let ch = SlackChannel {
            app_id: "A1".into(),
            bot_user_id: "U1".into(),
            app_token: "xapp-".into(),
            bot_api: SlackClient::new("xoxb-"),
            handler: None,
            slash: None,
        };
        assert_eq!(
            ch.capabilities(),
            Capability::TEXT | Capability::THREAD_REPLY
        );
        assert_eq!(ch.r#type().0, "slack");
    }

    #[tokio::test]
    async fn sender_posts_mrkdwn_through_the_client() {
        // A stub HTTP server capturing the posted form.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = br#"{"ok":true,"ts":"1700000000.000001"}"#;
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(body).await.unwrap();
            req
        });
        let sender = SlackSender::with_api_url("xoxb-", &format!("http://{addr}/"));
        let res = sender
            .send(
                CancellationToken::new(),
                OutboundMessage {
                    chat_id: "C1".into(),
                    text: "**bold**".into(),
                    thread_id: "1700000000.5".into(),
                    reply_to: String::new(),
                },
            )
            .await
            .unwrap();
        let req = handle.await.unwrap();
        assert_eq!(res.message_id, "1700000000.000001");
        // The mrkdwn conversion ran before posting.
        assert!(req.contains("text=*bold*"), "{req}");
        // Threaded into the requested topic.
        assert!(req.contains("thread_ts=1700000000.5"), "{req}");
    }

    #[tokio::test]
    async fn factory_decodes_config_and_refuses_missing_app_token() {
        use base64::Engine as _;
        let enc = |raw: &[u8]| base64::engine::general_purpose::STANDARD.encode(raw);
        let reg = cordy_channel::Registry::new();
        register_slack(
            &reg,
            ChannelDeps {
                decrypt: None,
                slash: None,
            },
        );

        // Missing app token → refused, not half-built.
        let err = match reg
            .build(ChannelConfig {
                r#type: Type(TYPE_SLACK.to_string()),
                raw: serde_json::json!({"app_id": "A1", "bot_token_encrypted": enc(b"xoxb")}),
                id: None,
                handler: None,
            })
            .await
        {
            Ok(_) => panic!("expected missing-app-token error"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("no app-level token"), "{err}");

        // Both tokens present → built with the decrypted identity.
        let built = reg
            .build(ChannelConfig {
                r#type: Type(TYPE_SLACK.to_string()),
                raw: serde_json::json!({
                    "app_id": "A1",
                    "bot_user_id": "U_BOT",
                    "bot_token_encrypted": enc(b"xoxb-bot"),
                    "app_token_encrypted": enc(b"xapp-app"),
                }),
                id: None,
                handler: None,
            })
            .await
            .unwrap();
        assert_eq!(built.r#type().0, TYPE_SLACK);
    }
}
