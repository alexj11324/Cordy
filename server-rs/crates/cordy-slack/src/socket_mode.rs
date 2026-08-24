//! Raw Slack Socket Mode transport. Rust has no slack-go equivalent, so this
//! module owns the wire protocol slack-go's socketmode.Client hides: mint a
//! websocket URL via apps.connections.open, consume JSON envelopes, and ACK
//! each one before processing (Slack expires un-ACKed envelopes in ~3s).

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;

use crate::client::SlackClient;

const SOCKET_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);
const SOCKET_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The subset of Socket Mode envelope types this adapter reacts to. Disconnect
/// requests end the stream so its owner can reconnect; hello and incoming
/// error frames remain lifecycle noise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeKind {
    EventsApi,
    SlashCommand,
    Disconnect,
    Other,
}

/// One decoded Socket Mode envelope.
#[derive(Debug, Clone)]
pub struct Envelope {
    pub kind: EnvelopeKind,
    /// The id Slack expects echoed back in the ack frame.
    pub envelope_id: String,
    /// The inner payload (the Events API envelope or the slash-command form
    /// object).
    pub payload: serde_json::Value,
    /// Slack's lifecycle reason for a disconnect control frame.
    pub disconnect_reason: Option<String>,
    /// Whether the envelope carries an id that must be ACKed.
    pub needs_ack: bool,
}

impl Envelope {
    fn parse(raw: &str) -> Option<Envelope> {
        let v: serde_json::Value = serde_json::from_str(raw).ok()?;
        let kind = match v.get("type").and_then(|t| t.as_str())? {
            "events_api" => EnvelopeKind::EventsApi,
            "slash_commands" => EnvelopeKind::SlashCommand,
            "disconnect" => EnvelopeKind::Disconnect,
            _ => EnvelopeKind::Other,
        };
        let envelope_id = v
            .get("envelope_id")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        Some(Envelope {
            kind,
            envelope_id: envelope_id.clone(),
            payload: v.get("payload").cloned().unwrap_or(serde_json::Value::Null),
            disconnect_reason: v
                .get("reason")
                .and_then(|reason| reason.as_str())
                .map(str::to_owned),
            needs_ack: !envelope_id.is_empty(),
        })
    }

    fn ack_frame(&self) -> Option<String> {
        if !self.needs_ack {
            return None;
        }
        Some(
            serde_json::json!({
                "type": "ack",
                "envelope_id": self.envelope_id,
            })
            .to_string(),
        )
    }

    fn disconnect_error(&self) -> Option<anyhow::Error> {
        (self.kind == EnvelopeKind::Disconnect).then(|| {
            let reason = self.disconnect_reason.as_deref().unwrap_or("unknown");
            anyhow::anyhow!("slack: socket mode disconnect requested: {reason}")
        })
    }
}

/// A live Socket Mode connection. `run` consumes envelopes until the socket
/// closes, the token is cancelled, or the handler errors.
pub struct SocketModeStream {
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl SocketModeStream {
    /// Dials a Socket Mode session with the app-level token: mints a
    /// single-use wss URL via apps.connections.open, then performs the
    /// websocket handshake.
    pub async fn dial(
        app_token_client: &SlackClient,
        ctx: CancellationToken,
    ) -> anyhow::Result<Self> {
        let url = tokio::select! {
            _ = ctx.cancelled() => anyhow::bail!("slack: dial cancelled"),
            u = app_token_client.apps_connections_open() => u?,
        };
        if url.is_empty() {
            anyhow::bail!("slack: empty socket mode url");
        }
        let (ws, _resp) = tokio::select! {
            _ = ctx.cancelled() => anyhow::bail!("slack: dial cancelled"),
            result = tokio::time::timeout(
                SOCKET_HANDSHAKE_TIMEOUT,
                tokio_tungstenite::connect_async(&url),
            ) => result
                .map_err(|_| anyhow::anyhow!(
                    "slack: socket mode handshake timed out after {SOCKET_HANDSHAKE_TIMEOUT:?}"
                ))?
                .map_err(|error| anyhow::anyhow!("slack: socket mode handshake: {error}"))?,
        };
        Ok(Self { ws })
    }

    async fn send_frame(
        &mut self,
        frame: WsMessage,
        operation: &'static str,
    ) -> anyhow::Result<()> {
        tokio::time::timeout(SOCKET_WRITE_TIMEOUT, self.ws.send(frame))
            .await
            .map_err(|_| {
                anyhow::anyhow!("slack: {operation} write timed out after {SOCKET_WRITE_TIMEOUT:?}")
            })?
            .map_err(|error| anyhow::anyhow!("slack: {operation} write failed: {error}"))
    }

    /// Runs the receive loop: decode each envelope, ACK it FIRST (Slack
    /// expires un-ACKed envelopes in ~3s, far below any handler's work; the
    /// ack is independent of the handler outcome), then invoke the handler.
    /// Returns Ok on graceful close or token cancellation.
    pub async fn run<F, Fut>(mut self, handler: F) -> anyhow::Result<()>
    where
        F: Fn(Envelope) -> Fut + Send + Sync,
        Fut: std::future::Future<Output = anyhow::Result<()>> + Send,
    {
        let handler = Arc::new(handler);
        loop {
            let msg = tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                    // Slack sends periodic pings; a silent 30s window means the
                    // link is wedged — surface it so the supervisor reconnects.
                    anyhow::bail!("slack: socket mode read timeout");
                }
                m = self.ws.next() => match m {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => anyhow::bail!("slack: socket mode error: {e}"),
                    None => anyhow::bail!("slack: socket mode event stream closed"),
                },
            };
            let text = match msg {
                WsMessage::Text(t) => t,
                WsMessage::Ping(p) => {
                    self.send_frame(WsMessage::Pong(p), "pong").await?;
                    continue;
                }
                WsMessage::Pong(_) | WsMessage::Binary(_) | WsMessage::Frame(_) => continue,
                WsMessage::Close(c) => {
                    anyhow::bail!("slack: socket mode closed: {c:?}");
                }
            };
            let Some(envelope) = Envelope::parse(&text) else {
                continue;
            };
            // ACK first, independent of handler outcome.
            if let Some(ack) = envelope.ack_frame() {
                self.send_frame(WsMessage::Text(ack.into()), "ack").await?;
            }
            if let Some(error) = envelope.disconnect_error() {
                return Err(error);
            }
            handler(envelope).await?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_envelope_kinds_and_payloads() {
        let e = Envelope::parse(
            r#"{"type":"events_api","envelope_id":"1d3c","payload":{"team_id":"T1","event":{"type":"message"}}}"#,
        )
        .unwrap();
        assert_eq!(e.kind, EnvelopeKind::EventsApi);
        assert_eq!(e.envelope_id, "1d3c");
        assert!(e.needs_ack);
        assert_eq!(e.payload["team_id"], "T1");

        let e = Envelope::parse(
            r#"{"type":"slash_commands","envelope_id":"e2","payload":{"command":"/issue"}}"#,
        )
        .unwrap();
        assert_eq!(e.kind, EnvelopeKind::SlashCommand);
        assert_eq!(e.payload["command"], "/issue");

        let e = Envelope::parse(
            r#"{"type":"disconnect","reason":"refresh_requested","debug_info":{"host":"wss-111.slack.com"}}"#,
        )
        .unwrap();
        assert_eq!(e.kind, EnvelopeKind::Disconnect);
        assert_eq!(e.disconnect_reason.as_deref(), Some("refresh_requested"));
        assert!(!e.needs_ack);
        assert_eq!(
            e.disconnect_error().unwrap().to_string(),
            "slack: socket mode disconnect requested: refresh_requested"
        );

        let e = Envelope::parse(r#"{"type":"hello"}"#).unwrap();
        assert_eq!(e.kind, EnvelopeKind::Other);
        assert!(!e.needs_ack);
        assert!(e.ack_frame().is_none());

        // Non-JSON frames are skipped by run().
        assert!(Envelope::parse("not json").is_none());
    }

    #[test]
    fn ack_frame_echoes_envelope_id() {
        let e =
            Envelope::parse(r#"{"type":"events_api","envelope_id":"abc","payload":{}}"#).unwrap();
        assert_eq!(
            e.ack_frame().unwrap(),
            r#"{"envelope_id":"abc","type":"ack"}"#
        );
    }
}
