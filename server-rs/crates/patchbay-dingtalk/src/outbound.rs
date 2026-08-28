//! Port of `outbound.go`: delivers an agent's chat reply back to DingTalk —
//! the outbound half of the round trip. On chat:done / task:failed it finds the
//! DingTalk chat binding for the task's session and posts the reply (or failure
//! notice) into the originating conversation. Sessions with no DingTalk binding
//! are ignored, so it coexists with the Feishu and Slack subscribers on the
//! shared event bus. Registered only when DingTalk is configured.

use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use patchbay_channel::RuntimeTasks;
use patchbay_channel_engine::task_input_is_channel_ingested;
use patchbay_db::queries::agent::get_agent_task;
use patchbay_db::queries::channel::{
    get_channel_chat_session_binding_by_session, get_channel_installation,
};

use crate::client::Client;
use crate::config::{decode_credentials, Decrypter};
use crate::outbound_send::Sender;
use crate::resolvers::outbound_target;
use crate::TYPE_DINGTALK;

/// Bus delivery is synchronous, so a stuck DingTalk HTTP call must not wedge
/// the publish call site: process on a detached task under this budget.
const OUTBOUND_EVENT_TIMEOUT: Duration = Duration::from_secs(10);

/// The DingTalk outbound event subscriber.
pub struct Outbound {
    pool: sqlx::PgPool,
    decrypt: Option<Arc<Decrypter>>,
    client: Arc<Client>,
}

impl Outbound {
    /// Builds the subscriber over the pool, the AppSecret decrypter, and the
    /// shared token-caching Client.
    pub fn new(pool: sqlx::PgPool, decrypt: Option<Arc<Decrypter>>, client: Arc<Client>) -> Self {
        Self {
            pool,
            decrypt,
            client,
        }
    }

    /// Subscribes to chat-done and task-failed. Task-failed keeps the DingTalk
    /// conversation consistent with the web transcript — without it a failed
    /// run leaves the user staring at the "👀 On it" ack forever.
    pub fn register(self: &Arc<Self>, bus: &patchbay_events::Bus, tasks: Arc<RuntimeTasks>) {
        let this = self.clone();
        let chat_done_tasks = tasks.clone();
        bus.subscribe(patchbay_protocol::events::EVENT_CHAT_DONE, move |e| {
            let this = this.clone();
            let e = e.clone();
            chat_done_tasks.spawn(async move { this.process_detached(&e).await });
        });
        let this = self.clone();
        bus.subscribe(patchbay_protocol::events::EVENT_TASK_FAILED, move |e| {
            let this = this.clone();
            let e = e.clone();
            tasks.spawn(async move { this.process_detached(&e).await });
        });
    }

    /// Bus delivery is synchronous, so a stuck DingTalk HTTP call must not
    /// wedge the publish call site: process on a detached task under this
    /// budget.
    async fn process_detached(&self, e: &patchbay_events::Event) {
        match tokio::time::timeout(OUTBOUND_EVENT_TIMEOUT, self.process_event(e)).await {
            Err(_) => tracing::warn!(
                chat_session_id = %e.chat_session_id,
                "dingtalk outbound: reply delivery timed out"
            ),
            Ok(Err(err)) => tracing::warn!(
                error = %err,
                chat_session_id = %e.chat_session_id,
                "dingtalk outbound: reply delivery failed"
            ),
            Ok(Ok(())) => {}
        }
    }

    async fn process_event(&self, e: &patchbay_events::Event) -> anyhow::Result<()> {
        let (task_id, session_id) = task_and_session_from_event(e);
        let Some(session_id) = session_id else {
            // Issue / autopilot tasks carry no chat_session.
            return Ok(());
        };
        let Some(task_id) = task_id else {
            return Ok(());
        };
        let content = event_content(e);
        if content.is_empty() {
            // Nothing to say (empty completion, or a retry-pending failure).
            return Ok(());
        }
        let binding =
            get_channel_chat_session_binding_by_session(&self.pool, session_id, TYPE_DINGTALK)
                .await?;
        let Some(binding) = binding else {
            // Not a DingTalk session (Feishu / Slack / web-only).
            return Ok(());
        };
        let task = get_agent_task(&self.pool, task_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("load agent task: no row"))?;
        let deliver = task_input_is_channel_ingested(&self.pool, task.chat_input_task_id).await?;
        if !deliver {
            return Ok(());
        }
        let inst = get_channel_installation(&self.pool, binding.installation_id, TYPE_DINGTALK)
            .await?
            .ok_or_else(|| anyhow::anyhow!("load dingtalk installation: no row"))?;
        if inst.status != "active" {
            // Revoked between trigger and reply.
            return Ok(());
        }
        let creds = decode_credentials(&inst.config, self.decrypt.as_deref())
            .map_err(|e| anyhow::anyhow!("decode dingtalk credentials: {e:#}"))?;
        Sender::new(self.client.clone(), creds)
            .send(&outbound_target(&binding), &content)
            .await
            .map_err(|e| anyhow::anyhow!("post dingtalk reply: {e:#}"))?;
        Ok(())
    }
}

/// Extracts the deliverable text from an EventChatDone payload or an
/// EventTaskFailed payload. Empty means stay silent.
///
/// For task-failed the text mirrors the web transcript's failure chat_message:
/// the broadcast's `error` field carries the same redacted failure text and is
/// omitted while an auto-retry is pending (the retry attempt reports its own
/// outcome), so error-present means deliverable.
fn event_content(e: &patchbay_events::Event) -> String {
    let payload = &e.payload;
    if e.event_type == patchbay_protocol::events::EVENT_TASK_FAILED {
        if payload
            .get("retry_pending")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return String::new();
        }
        if let Some(s) = payload.get("error").and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return format!("⚠️ {s}");
            }
        }
        return String::new();
    }
    payload
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn parse_uuid(raw: &str) -> Option<Uuid> {
    raw.parse().ok()
}

fn payload_str<'a>(payload: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

fn task_and_session_from_event(e: &patchbay_events::Event) -> (Option<Uuid>, Option<Uuid>) {
    let mut task_id = parse_uuid(&e.task_id);
    let mut session_id = parse_uuid(&e.chat_session_id);
    let payload = &e.payload;
    if task_id.is_none() {
        task_id = payload_str(payload, "task_id").and_then(parse_uuid);
    }
    if session_id.is_none() {
        session_id = payload_str(payload, "chat_session_id").and_then(parse_uuid);
    }
    (task_id, session_id)
}
