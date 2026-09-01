//! Port of `ws_connector.go`: hand-rolls the DingTalk Stream WebSocket
//! connection, replacing the vendor stream SDK. It mirrors the Lark ws
//! connector: a single blocking run owns exactly one socket session.
//! Reconnect/backoff/lease live in the shared engine Supervisor, so run just
//! returns — Ok on a graceful close, Err on a broken connection — and the
//! supervisor decides when to redial.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::inbound::BotCallbackData;
use crate::ws_endpoint::open_connection;
use crate::ws_frame::{
    new_ack_response, new_pong_response, DataFrame, DataFrameResponse, BOT_MESSAGE_TOPIC,
    FRAME_TYPE_CALLBACK, FRAME_TYPE_SYSTEM, SYSTEM_TOPIC_DISCONNECT, SYSTEM_TOPIC_PING,
};

pub const STREAM_PING_INTERVAL: Duration = Duration::from_secs(30);
pub const STREAM_READ_DEADLINE: Duration = Duration::from_secs(90);
pub const STREAM_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Bounds the open + dial handshake so a wedged gateway cannot stall a
/// supervisor sweep.
pub const STREAM_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// The bot-message callback seam: invoked for every decoded CALLBACK frame.
pub type OnMessage =
    Arc<dyn Fn(CancellationToken, BotCallbackData) -> ConnectorHandlerFuture + Send + Sync>;
pub type ConnectorHandlerFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>;

type WsStream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;
type WsSink = futures_util::stream::SplitSink<WsStream, Message>;

/// The production TLS connector: tokio-rustls with the workspace's single
/// ring provider and the OS trust store (same wiring as patchbay-wecom).
/// Built once per process and shared by every Stream connection.
pub struct TlsConnector {
    inner: tokio_tungstenite::Connector,
}

impl TlsConnector {
    pub fn new() -> anyhow::Result<Self> {
        let mut roots = rustls::RootCertStore::empty();
        // Individual unparsable system certs are skipped, matching Go's
        // x509.SystemCertPool tolerance.
        for cert in rustls_native_certs::load_native_certs().certs {
            let _ = roots.add(cert);
        }
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("default protocol versions are always supported")
        .with_root_certificates(roots)
        .with_no_client_auth();
        Ok(Self {
            inner: tokio_tungstenite::Connector::Rustls(Arc::new(config)),
        })
    }
}

impl Default for TlsConnector {
    fn default() -> Self {
        Self::new().expect("system trust store loads")
    }
}

/// Runs one installation's Stream session.
pub struct WsConnector {
    pub http: reqwest::Client,
    pub api_base: String,
    pub app_key: String,
    pub app_secret: String,

    pub on_message: OnMessage,
    pub runtime_health: Option<patchbay_channel::RuntimeHealthReporter>,

    /// TLS wiring for the wss dial. Tests dialing a plain-ws local server can
    /// leave it None.
    pub tls: Option<TlsConnector>,

    pub ping_interval: Duration,
    pub read_deadline: Duration,
    pub write_timeout: Duration,
}

impl WsConnector {
    pub fn new(
        http: reqwest::Client,
        api_base: String,
        app_key: String,
        app_secret: String,
        on_message: OnMessage,
    ) -> Self {
        Self {
            http,
            api_base,
            app_key,
            app_secret,
            on_message,
            runtime_health: None,
            tls: Some(TlsConnector::default()),
            ping_interval: STREAM_PING_INTERVAL,
            read_deadline: STREAM_READ_DEADLINE,
            write_timeout: STREAM_WRITE_TIMEOUT,
        }
    }

    /// Overrides the TLS connector (test convenience).
    pub fn with_tls(mut self, tls: Option<TlsConnector>) -> Self {
        self.tls = tls;
        self
    }

    pub fn with_runtime_health(
        mut self,
        runtime_health: Option<patchbay_channel::RuntimeHealthReporter>,
    ) -> Self {
        self.runtime_health = runtime_health;
        self
    }

    /// Opens the connection and services frames until ctx is cancelled
    /// (returns Ok), the gateway sends a SYSTEM disconnect (returns Ok), or the
    /// socket breaks (returns the error, so the supervisor reconnects under
    /// backoff).
    pub async fn run(&self, ctx: CancellationToken) -> anyhow::Result<()> {
        let open = open_connection(&self.http, &self.api_base, &self.app_key, &self.app_secret);
        // dial_url carries the single-use Stream ticket. Dialer errors may echo
        // their input URL, so do not wrap them into the supervisor/log path.
        let open = tokio::time::timeout(STREAM_HANDSHAKE_TIMEOUT, open);
        let dial_url = match open.await {
            Err(_) => {
                anyhow::bail!("dingtalk stream: open connection: handshake timed out after {STREAM_HANDSHAKE_TIMEOUT:?}")
            }
            Ok(Err(err)) => anyhow::bail!("dingtalk stream: open connection: {err:#}"),
            Ok(Ok(url)) => url,
        };
        let request = dial_url
            .clone()
            .into_client_request()
            .map_err(|_| anyhow::anyhow!("dingtalk stream: dial failed"))?;
        let connect = tokio_tungstenite::connect_async_tls_with_config(
            request,
            None,
            false,
            self.tls.as_ref().map(|t| t.inner.clone()),
        );
        let stream = match tokio::time::timeout(STREAM_HANDSHAKE_TIMEOUT, connect).await {
            Err(_) => anyhow::bail!("dingtalk stream: dial timed out"),
            Ok(Ok((stream, _resp))) => stream,
            Ok(Err(_)) => anyhow::bail!("dingtalk stream: dial failed"),
        };
        if let Some(reporter) = &self.runtime_health {
            reporter.healthy().await;
        }
        let (sink, mut stream_rx) = stream.split();
        let sink = Arc::new(tokio::sync::Mutex::new(sink));

        // Transport-level keepalive; a failed control write leaves teardown to
        // the read loop.
        let ping_ctx = ctx.child_token();
        let ping_task = tokio::spawn(ping_loop(
            sink.clone(),
            ping_ctx.clone(),
            self.ping_interval,
            self.write_timeout,
        ));

        let result = self.read_loop(ctx.clone(), &sink, &mut stream_rx).await;

        ping_ctx.cancel();
        let _ = ping_task.await;
        result
    }

    async fn read_loop(
        &self,
        ctx: CancellationToken,
        sink: &Arc<tokio::sync::Mutex<WsSink>>,
        stream_rx: &mut futures_util::stream::SplitStream<WsStream>,
    ) -> anyhow::Result<()> {
        loop {
            let next = tokio::select! {
                _ = ctx.cancelled() => return Ok(()),
                next = tokio::time::timeout(self.read_deadline, stream_rx.next()) => next,
            };
            let msg = match next {
                // Read deadline exceeded without any frame (incl. pongs).
                Ok(None) | Err(_) => anyhow::bail!("dingtalk stream: read deadline exceeded"),
                Ok(Some(m)) => m?,
            };
            let Message::Text(text) = msg else {
                // Protocol-level pings are answered by tungstenite on read;
                // pong/binary/close frames carry no application payload here.
                continue;
            };
            let frame: DataFrame = match serde_json::from_str(&text) {
                Ok(f) => f,
                Err(err) => {
                    tracing::warn!(error = %err, "dingtalk stream: malformed frame");
                    continue;
                }
            };

            if frame.frame_type == FRAME_TYPE_SYSTEM && frame.topic() == SYSTEM_TOPIC_PING {
                let resp = new_pong_response(frame.message_id(), &frame.data);
                if let Err(err) = self.write_response(sink, resp).await {
                    if ctx.is_cancelled() {
                        return Ok(());
                    }
                    anyhow::bail!("dingtalk stream: write pong: {err:#}");
                }
            } else if frame.frame_type == FRAME_TYPE_SYSTEM
                && frame.topic() == SYSTEM_TOPIC_DISCONNECT
            {
                // Gateway asks us to reconnect; return cleanly and let the
                // supervisor redial.
                return Ok(());
            } else if frame.frame_type == FRAME_TYPE_CALLBACK && frame.topic() == BOT_MESSAGE_TOPIC
            {
                self.dispatch_callback(&ctx, &frame, sink).await;
            } else {
                tracing::warn!(
                    frame_type = %frame.frame_type,
                    topic = %frame.topic(),
                    "dingtalk stream: unhandled frame"
                );
            }
        }
    }

    /// Serializes and sends one response frame under the shared write timeout.
    async fn write_response(
        &self,
        sink: &Arc<tokio::sync::Mutex<WsSink>>,
        resp: DataFrameResponse,
    ) -> anyhow::Result<()> {
        let message = response_message(&resp)?;
        let mut guard = sink.lock().await;
        tokio::time::timeout(self.write_timeout, guard.send(message))
            .await
            .map_err(|_| anyhow::anyhow!("dingtalk stream: write timed out"))??;
        Ok(())
    }

    /// Decodes a bot-message callback, hands it to on_message, and always ACKs
    /// (echoing the frame's messageId). A decode or handler error is logged,
    /// never surfaced: DingTalk expires un-ACKed frames fast and the engine's
    /// (installation, msgId) dedup guards any redelivery — matching the prior
    /// SDK callback's always-ACK behavior.
    async fn dispatch_callback(
        &self,
        ctx: &CancellationToken,
        frame: &DataFrame,
        sink: &Arc<tokio::sync::Mutex<WsSink>>,
    ) {
        match serde_json::from_str::<BotCallbackData>(&frame.data) {
            Err(err) => tracing::warn!(error = %err, "dingtalk stream: decode callback"),
            Ok(payload) => {
                if let Err(err) = (self.on_message)(ctx.clone(), payload).await {
                    tracing::warn!(error = %err, "dingtalk stream: handler error");
                }
            }
        }
        let ack = new_ack_response(frame.message_id());
        if let Err(err) = self.write_response(sink, ack).await {
            tracing::warn!(error = %err, "dingtalk stream: write ack");
        }
    }
}

/// DingTalk Stream responses are JSON WebSocket text messages. The gateway's
/// official SDK uses `WriteJSON`, which selects the text opcode; a binary JSON
/// frame is not an equivalent acknowledgement at the protocol boundary.
fn response_message(resp: &DataFrameResponse) -> anyhow::Result<Message> {
    Ok(Message::Text(serde_json::to_string(resp)?.into()))
}

async fn ping_loop(
    sink: Arc<tokio::sync::Mutex<WsSink>>,
    ctx: CancellationToken,
    interval: Duration,
    write_timeout: Duration,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await; // the first tick fires immediately; skip it
    loop {
        tokio::select! {
            _ = ctx.cancelled() => return,
            _ = ticker.tick() => {}
        }
        let mut guard = sink.lock().await;
        if tokio::time::timeout(write_timeout, guard.send(Message::Ping(Bytes::new())))
            .await
            .is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_response_uses_text_frame() {
        let message = response_message(&new_ack_response("mid-1")).unwrap();
        let Message::Text(payload) = message else {
            panic!("DingTalk Stream ACK must use a WebSocket text frame");
        };
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["code"], 200);
        assert_eq!(value["headers"]["messageId"], "mid-1");
    }
}
