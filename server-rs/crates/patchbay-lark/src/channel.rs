//! Feishu/Lark adapter for the shared channel Supervisor.

use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use patchbay_channel::{
    Capability, Channel, Config, Factory, FactoryFuture, InboundHandler, InboundMessage,
    OutboundMessage, ReplyCtx, SendResult, Source,
};

use crate::client::{ApiClient, ReplyTarget, SendTextParams};
use crate::connector::{EventConnector, EventEmitter};
use crate::feishu_types::{DispatchResult, InboundMessage as LarkInboundMessage};
use crate::installation::{installation_credentials_for, CredentialsResolver};
use crate::store::{installation_from_config, Installation};
use crate::types::ChatId;

pub struct FeishuChannel {
    installation: Installation,
    connector: Arc<dyn EventConnector>,
    handler: Option<InboundHandler>,
    api: Arc<dyn ApiClient>,
    credentials: Arc<dyn CredentialsResolver>,
}

#[async_trait]
impl Channel for FeishuChannel {
    fn r#type(&self) -> patchbay_channel::Type {
        patchbay_channel::Type::feishu()
    }

    async fn connect(&self, ctx: CancellationToken) -> anyhow::Result<()> {
        let handler = self
            .handler
            .clone()
            .ok_or_else(|| anyhow::anyhow!("lark: inbound handler not configured"))?;
        let emit: EventEmitter = Arc::new(move |emit_ctx, message| {
            let handler = handler.clone();
            Box::pin(async move {
                handler
                    .call(emit_ctx, channel_message_from_lark(message))
                    .await?;
                Ok(DispatchResult::default())
            })
        });
        self.connector
            .run(ctx, self.installation.clone(), emit)
            .await
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn send(&self, out: OutboundMessage) -> anyhow::Result<SendResult> {
        let credentials =
            installation_credentials_for(self.credentials.as_ref(), &self.installation)?;
        let reply_target = outbound_reply_target(&out.reply_to, &out.thread_id);
        let message_id = self
            .api
            .send_text_message(SendTextParams {
                installation_id: credentials,
                chat_id: ChatId(out.chat_id),
                text: out.text,
                reply_target,
            })
            .await?;
        Ok(SendResult { message_id })
    }

    fn capabilities(&self) -> Capability {
        Capability::TEXT
            | Capability::RICH_CARD
            | Capability::THREAD_REPLY
            | Capability::QUOTE_REPLY
            | Capability::ATTACHMENT
            | Capability::TYPING_INDICATOR
            | Capability::MESSAGE_EDIT
    }
}

fn outbound_reply_target(reply_to: &str, thread_id: &str) -> ReplyTarget {
    if reply_to.is_empty() {
        ReplyTarget::default()
    } else {
        ReplyTarget {
            message_id: reply_to.to_string(),
            in_thread: !thread_id.is_empty(),
        }
    }
}

pub fn channel_message_from_lark(message: LarkInboundMessage) -> InboundMessage {
    let reply_to =
        (!message.parent_id.is_empty() || !message.root_id.is_empty()).then(|| ReplyCtx {
            message_id: message.parent_id.clone(),
            root_id: message.root_id.clone(),
        });
    let raw = serde_json::to_value(&message).unwrap_or(serde_json::Value::Null);
    InboundMessage {
        event_id: message.event_id,
        message_id: message.message_id,
        source: Source {
            channel_type: patchbay_channel::Type::feishu(),
            chat_id: message.chat_id.0,
            chat_type: message.chat_type,
            sender_id: message.sender_open_id.0,
            thread_id: message.thread_id,
            ..Default::default()
        },
        r#type: channel_msg_type(&message.message_type),
        text: message.body,
        command_text: message.command_body,
        reply_to,
        addressed_to_bot: message.addressed_to_bot,
        force_fresh: message.force_fresh_session,
        raw,
        ..Default::default()
    }
}

fn channel_msg_type(message_type: &str) -> patchbay_channel::MsgType {
    match message_type {
        "image" => patchbay_channel::MsgType::image(),
        "file" => patchbay_channel::MsgType::file(),
        "audio" => patchbay_channel::MsgType::audio(),
        "media" | "video" => patchbay_channel::MsgType::video(),
        "" | "text" | "post" | "merge_forward" | "interactive" => patchbay_channel::MsgType::text(),
        _ => patchbay_channel::MsgType::unknown(),
    }
}

#[derive(Clone)]
pub struct FeishuChannelDeps {
    pub connector: Arc<dyn EventConnector>,
    pub api: Arc<dyn ApiClient>,
    pub credentials: Arc<dyn CredentialsResolver>,
}

pub fn register_feishu(registry: &patchbay_channel::Registry, deps: FeishuChannelDeps) {
    registry.register(patchbay_channel::Type::feishu(), feishu_factory(deps));
}

fn feishu_factory(deps: FeishuChannelDeps) -> Factory {
    Arc::new(move |cfg: Config| -> FactoryFuture {
        let deps = deps.clone();
        Box::pin(async move {
            let installation = installation_from_config(cfg.raw)
                .map_err(|error| anyhow::anyhow!("decode feishu installation config: {error}"))?;
            Ok(Arc::new(FeishuChannel {
                installation,
                connector: deps.connector,
                handler: cfg.handler,
                api: deps.api,
                credentials: deps.credentials,
            }) as Arc<dyn Channel>)
        })
    })
}
