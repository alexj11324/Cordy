//! Port of `ack.go`: the ack notifier stands in for a typing indicator. The
//! classic robot API we send through (oToMessages/batchSend) exposes no
//! per-message reaction, so a long-running agent turn leaves a mobile user
//! staring at silence until the reply lands. On ingest it posts a lightweight
//! "working on it" message so the user sees their message was received.
//!
//! It implements [`TypingNotifier`]. The engine calls on_ingested after an
//! accepted turn has been persisted and scheduled for an agent run. Terminal
//! commands such as /issue return their synchronous result without posting this
//! non-retractable processing acknowledgement.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use patchbay_channel::InboundMessage;
use patchbay_channel_engine::resolvers::{ResolvedInstallation, TypingNotifier};

use crate::client::Client;
use crate::config::Decrypter;
use crate::replier::{send_installation_text, target_from_message};

/// The stand-in "typing" message. Kept short: it is a real, non-retractable
/// chat message, not an ephemeral indicator.
pub const ACK_PROCESSING_TEXT: &str = "👀 On it — I'll reply here when it's ready.";

/// Suppresses duplicate acks for the same session. It sits just above the run
/// debounce window so a burst of messages that flush into one run yields a
/// single ack, while a genuinely later turn re-acks.
pub const ACK_COALESCE_WINDOW: Duration = Duration::from_secs(5);

struct Inner {
    last_ack: HashMap<String, Instant>,
}

/// Delivers text into the installation's conversation. None uses the real
/// Open-API send; tests inject a recorder.
pub type AckSendText = Arc<
    dyn Fn(
            CancellationToken,
            ResolvedInstallation,
            InboundMessage,
            String,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>
        + Send
        + Sync,
>;

/// Posts the processing ack and coalesces bursts per session.
pub struct AckNotifier {
    client: Arc<Client>,
    decrypt: Option<Arc<Decrypter>>,
    window: Duration,

    inner: Mutex<Inner>,

    /// Delivers text into the installation's conversation. None uses the real
    /// Open-API send; tests inject a recorder.
    send_text: Option<AckSendText>,
}

impl AckNotifier {
    /// Builds the ack notifier over the shared outbound client and the
    /// credential decrypter.
    pub fn new(client: Arc<Client>, decrypt: Option<Arc<Decrypter>>) -> Self {
        Self {
            client,
            decrypt,
            window: ACK_COALESCE_WINDOW,
            inner: Mutex::new(Inner {
                last_ack: HashMap::new(),
            }),
            send_text: None,
        }
    }

    /// Overrides the delivery seam (test convenience).
    pub fn with_send_text(mut self, f: AckSendText) -> Self {
        self.send_text = Some(f);
        self
    }

    /// Reports whether an ack for session_id should be skipped, and otherwise
    /// records this ack. The check-and-set is atomic so concurrent ingests of
    /// one burst yield a single ack.
    fn suppress(&self, session_id: Uuid) -> bool {
        if session_id.is_nil() {
            return false;
        }
        let key = session_id.to_string();
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(last) = inner.last_ack.get(&key) {
            if now.duration_since(*last) < self.window {
                return true;
            }
        }
        // Prune entries past the window before inserting. on_settled only fires
        // for runs that enqueue no task, so task-spawning sessions would
        // otherwise leak their entry forever. Stale entries are dead (any later
        // turn re-acks), and this runs only on a cache miss, keeping the map
        // bounded by the sessions seen within one window.
        inner
            .last_ack
            .retain(|_, last| now.duration_since(*last) < self.window);
        inner.last_ack.insert(key, now);
        false
    }
}

#[async_trait]
impl TypingNotifier for AckNotifier {
    /// Posts the processing ack unless a recent ack for the same session is
    /// still within the coalesce window.
    async fn on_ingested(
        &self,
        ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        session_id: Uuid,
    ) {
        if self.suppress(session_id) {
            return;
        }
        let result = match &self.send_text {
            Some(send) => {
                send(
                    ctx,
                    inst.clone(),
                    msg.clone(),
                    ACK_PROCESSING_TEXT.to_string(),
                )
                .await
            }
            None => self.real_send(ctx, inst, msg, ACK_PROCESSING_TEXT).await,
        };
        if let Err(err) = result {
            tracing::warn!(
                installation_id = %inst.id,
                error = %err,
                "dingtalk ack: send failed"
            );
        }
    }

    /// Clears the session's dedup entry so its next turn acks immediately.
    async fn on_settled(&self, _ctx: CancellationToken, session_id: Uuid) {
        if session_id.is_nil() {
            return;
        }
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .last_ack
            .remove(&session_id.to_string());
    }
}

impl AckNotifier {
    async fn real_send(
        &self,
        _ctx: CancellationToken,
        inst: &ResolvedInstallation,
        msg: &InboundMessage,
        text: &str,
    ) -> anyhow::Result<()> {
        send_installation_text(
            &self.client,
            self.decrypt.as_deref(),
            inst,
            &target_from_message(msg),
            text,
        )
        .await
        .map(|_| ())
    }
}
