use std::sync::{Arc, Weak};
use std::time::Duration;

use base64::Engine as _;
use uuid::Uuid;

use patchbay_channel::RuntimeTasks;
use patchbay_channel_engine::task_input_is_channel_ingested;
use patchbay_db::queries::agent::get_agent_task;
use patchbay_db::queries::channel::{
    get_channel_chat_session_binding_by_session, get_channel_installation_for_runtime,
};

use crate::api::Client;
use crate::config::{decode_credentials, DecrypterFn};
use crate::resolvers::WeixinBindingConfig;

const DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Outbound {
    pool: sqlx::PgPool,
    decrypt: Option<Arc<DecrypterFn>>,
    bus: Weak<patchbay_events::Bus>,
}

impl Outbound {
    pub fn new(
        pool: sqlx::PgPool,
        decrypt: Option<Arc<DecrypterFn>>,
        bus: Arc<patchbay_events::Bus>,
    ) -> Self {
        Self {
            pool,
            decrypt,
            bus: Arc::downgrade(&bus),
        }
    }

    pub fn register(self: &Arc<Self>, bus: &patchbay_events::Bus, tasks: Arc<RuntimeTasks>) {
        let this = Arc::clone(self);
        for event_type in [
            patchbay_protocol::EVENT_CHAT_DONE,
            patchbay_protocol::EVENT_TASK_FAILED,
        ] {
            let this = Arc::clone(&this);
            let tasks = tasks.clone();
            bus.subscribe(event_type, move |event| {
                let this = Arc::clone(&this);
                let event = event.clone();
                tasks.spawn(async move {
                    match tokio::time::timeout(DELIVERY_TIMEOUT, this.process(&event)).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => tracing::warn!(%error, "weixin outbound delivery failed"),
                        Err(_) => tracing::warn!("weixin outbound delivery timed out"),
                    }
                });
            });
        }
    }

    async fn process(&self, event: &patchbay_events::Event) -> anyhow::Result<()> {
        let session_id = parse_uuid(&event.chat_session_id)
            .or_else(|| payload_uuid(&event.payload, "chat_session_id"));
        let Some(session_id) = session_id else {
            return Ok(());
        };
        let Some(binding) =
            get_channel_chat_session_binding_by_session(&self.pool, session_id, crate::TYPE_WEIXIN)
                .await?
        else {
            return Ok(());
        };
        let task_id =
            parse_uuid(&event.task_id).or_else(|| payload_uuid(&event.payload, "task_id"));
        let Some(task_id) = task_id else {
            return Ok(());
        };
        let task = get_agent_task(&self.pool, task_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("weixin outbound task row missing"))?;
        if !task_input_is_channel_ingested(&self.pool, task.chat_input_task_id).await? {
            return Ok(());
        }
        if event.event_type == patchbay_protocol::EVENT_TASK_FAILED
            && task_failure_retry_pending(&event.payload)
        {
            return Ok(());
        }
        let text = if event.event_type == patchbay_protocol::EVENT_TASK_FAILED {
            "❌ The agent run failed. Please try again."
        } else {
            event
                .payload
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
        };
        if text.is_empty() {
            return Ok(());
        }
        let installation =
            get_channel_installation_for_runtime(
                &self.pool,
                binding.installation_id,
                crate::TYPE_WEIXIN,
            )
                .await?
                .filter(|row| row.status == "active")
                .ok_or_else(|| anyhow::anyhow!("weixin installation is inactive or missing"))?;
        let installed_at = installation.installed_at;
        let credentials = decode_credentials(&installation.config, self.decrypt.as_deref())?;
        let target: WeixinBindingConfig = serde_json::from_value(binding.config)?;
        let ciphertext =
            base64::engine::general_purpose::STANDARD.decode(target.context_token_encrypted)?;
        let plaintext = match self.decrypt.as_deref() {
            Some(decrypt) => decrypt(&ciphertext)?,
            None => ciphertext,
        };
        let context_token = String::from_utf8(plaintext)?;
        Client::new(&credentials.base_url, &credentials.bot_token)?
            .send_text(&target.user_id, &context_token, text)
            .await?;
        // This path is reached only after an inbound WeChat message was
        // resolved to a chat session. A successful provider send therefore
        // proves the first real inbound -> outbound round trip for this
        // installation. Keep the marker server-owned and credential-free.
        crate::verification::record_round_trip(
            &self.pool,
            &self.bus,
            installation.id,
            installed_at,
        )
        .await;
        Ok(())
    }
}

fn parse_uuid(value: &str) -> Option<Uuid> {
    value.parse::<Uuid>().ok().filter(|id| !id.is_nil())
}

fn payload_uuid(value: &serde_json::Value, key: &str) -> Option<Uuid> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .and_then(parse_uuid)
}

fn task_failure_retry_pending(payload: &serde_json::Value) -> bool {
    payload
        .get("retry_pending")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}
