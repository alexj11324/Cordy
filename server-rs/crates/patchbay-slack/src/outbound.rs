//! Outbound delivery of an agent's chat reply back to Slack — the outbound
//! half of the round trip. Port of
//!
//! Mirrors the Feishu Patcher: on chat:done it finds the Slack chat binding for
//! the finished task's session and posts the reply into the originating
//! channel/thread. Sessions with no Slack binding are ignored, so it coexists
//! with the Feishu patcher on the shared event bus. It is only registered when
//! Slack is configured.

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use patchbay_channel::{OutboundMessage, RuntimeTasks};
use patchbay_channel_engine::provenance::task_input_is_channel_ingested;
use patchbay_db::queries::agent::get_agent_task;
use patchbay_db::queries::channel::{
    get_channel_chat_session_binding_by_session, get_channel_installation_for_runtime,
};
use patchbay_events::{Bus, Event};
use uuid::Uuid;

use crate::channel::SlackSender;
use crate::config::{decode_credentials, Decrypter};
use crate::resolvers::SlackBindingConfig;
use crate::TYPE_SLACK;

/// Builds the Slack outbound subscriber over the pool and the bot/app-token
/// decrypter.
pub struct Outbound {
    pool: PgPool,
    decrypt: Option<Arc<Decrypter>>,
}

impl Outbound {
    pub fn new(pool: PgPool, decrypt: Option<Arc<Decrypter>>) -> Self {
        Self { pool, decrypt }
    }

    /// Subscribes to the chat-done event on the bus.
    pub fn register(self: &Arc<Self>, bus: &Bus, tasks: Arc<RuntimeTasks>) {
        let me = Arc::clone(self);
        bus.subscribe(patchbay_protocol::EVENT_CHAT_DONE, move |e: &Event| {
            // Bus delivery is synchronous, so a stuck Slack HTTP call must not
            // wedge the publish call site: run on a fresh context with a tight
            // timeout instead of the publisher's.
            let me = Arc::clone(&me);
            let e = e.clone();
            tasks.spawn(async move {
                let ctx = tokio_util::sync::CancellationToken::new();
                let result = tokio::time::timeout(REPLY_BUDGET, me.process_event(&ctx, &e))
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("reply deadline exceeded")));
                if let Err(err) = result {
                    tracing::warn!(
                        error = %err,
                        chat_session_id = %e.chat_session_id,
                        "slack outbound: reply delivery failed"
                    );
                }
            });
        });
    }

    async fn process_event(
        &self,
        ctx: &tokio_util::sync::CancellationToken,
        e: &Event,
    ) -> anyhow::Result<()> {
        // Issue / automation tasks carry no chat_session.
        let Ok(session_id) = Uuid::parse_str(&e.chat_session_id) else {
            return Ok(());
        };
        if session_id.is_nil() {
            return Ok(());
        }
        let binding =
            match get_channel_chat_session_binding_by_session(&self.pool, session_id, TYPE_SLACK)
                .await?
            {
                Some(b) => b,
                None => return Ok(()), // not a Slack session (Feishu / web-only)
            };
        let content = chat_done_content(e);
        if content.is_empty() {
            return Ok(()); // nothing to say (empty completion)
        }
        // Only bound, non-empty completions reach here, so classify the task
        // origin before loading credentials or sending. Web/mobile direct-chat
        // tasks can reuse a session that originated in Slack, but their replies
        // belong only in Patchbay. Outbound delivery fails closed when the origin
        // cannot be established. Sealed channel tasks own an input batch just
        // like direct tasks, so the discriminator is the immutable
        // channel_ingested provenance of that batch, not chat_input_task_id
        // presence (which #5645 originally used).
        let Some(task_id) = chat_done_task_id(e) else {
            return Ok(());
        };
        let task = get_agent_task(&self.pool, task_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("load agent task: row missing"))?;
        let deliver = task_input_is_channel_ingested(&self.pool, task.chat_input_task_id).await?;
        if !deliver {
            return Ok(());
        }
        let inst = get_channel_installation_for_runtime(
            &self.pool,
            binding.installation_id,
            TYPE_SLACK,
        )
            .await?
            .ok_or_else(|| anyhow::anyhow!("load slack installation: row missing"))?;
        if inst.status != "active" {
            return Ok(()); // revoked between trigger and reply
        }
        let creds = decode_credentials(&inst.config, self.decrypt.as_deref())
            .map_err(|e| anyhow::anyhow!("decode slack credentials: {e}"))?;
        let (channel_id, thread_ts) = outbound_target(&binding);
        SlackSender::new(&creds.bot_token)
            .send(
                ctx.clone(),
                OutboundMessage {
                    chat_id: channel_id,
                    text: content,
                    thread_id: thread_ts,
                    reply_to: String::new(),
                },
            )
            .await
            .map_err(|e| anyhow::anyhow!("post slack reply: {e}"))?;
        Ok(())
    }
}

/// Bounds one reply delivery off the bus thread (Go used a 10s context).
const REPLY_BUDGET: Duration = Duration::from_secs(10);

/// Extracts the task id from the event envelope or the payload map emitted by
/// TaskService. Outbound delivery fails closed when the task origin cannot be
/// established.
fn chat_done_task_id(e: &Event) -> Option<Uuid> {
    let mut raw = e.task_id.clone();
    if raw.is_empty() {
        raw = e
            .payload
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    }
    Uuid::parse_str(&raw).ok().filter(|id| !id.is_nil())
}

/// Recovers the real send target from the chat binding. The channel_chat_id
/// may be a composite "channel:threadRoot" isolation key, so the real channel
/// id is read from the binding config ([`SlackBindingConfig`]); the reply
/// thread is the recorded last_thread_id.
fn outbound_target(b: &patchbay_db::models::ChannelChatSessionBinding) -> (String, String) {
    let mut channel_id = b.channel_chat_id.clone();
    if !b.config.is_null() {
        if let Ok(cfg) = serde_json::from_value::<SlackBindingConfig>(b.config.clone()) {
            if !cfg.channel_id.is_empty() {
                channel_id = cfg.channel_id;
            }
        }
    }
    let thread_ts = b.last_thread_id.clone().unwrap_or_default();
    (channel_id, thread_ts)
}

/// Extracts the reply text from a chat:done payload (the typed payload or its
/// map form after a serialization round trip — both are JSON Values on this
/// bus).
fn chat_done_content(e: &Event) -> String {
    e.payload
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_id_prefers_envelope_then_payload_and_fails_closed() {
        let mk = |task_id: &str, payload: serde_json::Value| Event {
            event_type: String::new(),
            workspace_id: String::new(),
            actor_type: String::new(),
            actor_id: String::new(),
            payload,
            task_id: task_id.to_string(),
            chat_session_id: String::new(),
        };

        let id = Uuid::now_v7();
        assert_eq!(chat_done_task_id(&mk("", serde_json::json!({}))), None);
        assert_eq!(chat_done_task_id(&mk("junk", serde_json::json!({}))), None);
        // Nil UUID is not a usable origin.
        assert_eq!(
            chat_done_task_id(&mk(&Uuid::nil().to_string(), serde_json::json!({}))),
            None
        );
        assert_eq!(
            chat_done_task_id(&mk(&id.to_string(), serde_json::json!({}))),
            Some(id)
        );
        assert_eq!(
            chat_done_task_id(&mk("", serde_json::json!({"task_id": id.to_string()}))),
            Some(id)
        );
    }

    #[test]
    fn content_extracts_from_payload_map() {
        let e = Event {
            payload: serde_json::json!({"content": "hello world"}),
            ..Default::default()
        };
        assert_eq!(chat_done_content(&e), "hello world");
        let empty = Event::default();
        assert_eq!(chat_done_content(&empty), "");
    }

    #[test]
    fn outbound_target_reads_config_channel_and_last_thread() {
        use chrono::Utc;
        let b = patchbay_db::models::ChannelChatSessionBinding {
            channel_chat_id: "C1:T1".to_string(),
            channel_type: TYPE_SLACK.to_string(),
            chat_session_id: Uuid::nil(),
            chat_type: "group".to_string(),
            config: serde_json::json!({"channel_id": "C_REAL"}),
            created_at: Utc::now(),
            id: Uuid::nil(),
            installation_id: Uuid::nil(),
            last_message_id: None,
            last_thread_id: Some("1700000000.5".to_string()),
            pending_fresh: false,
        };
        let (channel, thread) = outbound_target(&b);
        assert_eq!(channel, "C_REAL");
        assert_eq!(thread, "1700000000.5");

        // No config → composite key passes through as-is; no thread → "".
        let plain = patchbay_db::models::ChannelChatSessionBinding {
            config: serde_json::json!({}),
            last_thread_id: None,
            ..b
        };
        let (channel, thread) = outbound_target(&plain);
        assert_eq!(channel, "C1:T1");
        assert_eq!(thread, "");
    }
}
