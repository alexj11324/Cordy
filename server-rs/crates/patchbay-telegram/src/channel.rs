//! Per-installation Telegram long-poll channel and registry factory.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use patchbay_channel::{BuiltChannel, Capability, Channel, Config, Factory, InboundHandler};

use crate::api::{retry_after, BotApi, ConflictError, ReplyParameters, SendMessageParams, Update};
use crate::config::{decode_credentials, DecrypterFn};
use crate::inbound::inbound_from_update;

const POLL_RETRY_DELAY: Duration = Duration::from_secs(2);
const ISSUE_ERROR_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
const ISSUE_DISPATCH_FAILED_TEXT: &str = "⚠️ 创建任务时发生内部错误，请稍后重试。";
const UNSUPPORTED_TYPE_TEXT: &str = "暂不支持此类消息，请发送文字内容。";

pub struct TelegramChannel {
    bot_id: i64,
    bot_username: String,
    api: BotApi,
    handler: Option<InboundHandler>,
    media_enabled: bool,
}

#[async_trait]
impl Channel for TelegramChannel {
    fn r#type(&self) -> patchbay_channel::Type {
        patchbay_channel::Type(crate::TYPE_TELEGRAM.to_string())
    }

    async fn connect(&self, ctx: CancellationToken) -> anyhow::Result<()> {
        let handler = self
            .handler
            .clone()
            .ok_or_else(|| anyhow::anyhow!("telegram: inbound handler not configured"))?;
        let tasks = patchbay_channel::RuntimeTasks::new();
        let result = self.poll(ctx.clone(), handler, &tasks).await;
        if !tasks.shutdown(ISSUE_ERROR_REPLY_TIMEOUT).await {
            tracing::warn!(
                bot_id = self.bot_id,
                "telegram dispatch-error tasks exceeded connection shutdown deadline; aborted"
            );
        }
        result
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn send(
        &self,
        out: patchbay_channel::OutboundMessage,
    ) -> anyhow::Result<patchbay_channel::SendResult> {
        crate::sender::send(&self.api, &out).await
    }

    fn capabilities(&self) -> Capability {
        Capability::TEXT
            | Capability::THREAD_REPLY
            | Capability::QUOTE_REPLY
            | Capability::TYPING_INDICATOR
            | Capability::MESSAGE_EDIT
    }
}

impl TelegramChannel {
    async fn poll(
        &self,
        ctx: CancellationToken,
        handler: InboundHandler,
        tasks: &patchbay_channel::RuntimeTasks,
    ) -> anyhow::Result<()> {
        let mut offset = 0_i64;
        loop {
            let updates = tokio::select! {
                _ = ctx.cancelled() => return Ok(()),
                result = self.api.get_updates(offset) => result,
            };
            let updates = match updates {
                Ok(updates) => updates,
                Err(_) if ctx.is_cancelled() => return Ok(()),
                Err(error)
                    if error
                        .chain()
                        .any(|cause| cause.downcast_ref::<ConflictError>().is_some()) =>
                {
                    tracing::warn!(
                        bot_id = self.bot_id,
                        "telegram getUpdates conflict: bot is polled by another consumer"
                    );
                    return Err(error);
                }
                Err(error) => {
                    if let Some(wait) = retry_after(&error) {
                        tracing::warn!(
                            bot_id = self.bot_id,
                            ?wait,
                            "telegram getUpdates rate limited"
                        );
                        if !sleep_or_cancel(&ctx, wait).await {
                            return Ok(());
                        }
                        continue;
                    }
                    tracing::warn!(bot_id = self.bot_id, %error, "telegram getUpdates failed");
                    if !sleep_or_cancel(&ctx, POLL_RETRY_DELAY).await {
                        return Ok(());
                    }
                    return Err(anyhow::anyhow!("telegram: getUpdates: {error:#}"));
                }
            };

            for update in updates {
                if update.update_id >= offset {
                    offset = update.update_id + 1;
                }
                self.dispatch(&ctx, &handler, tasks, update).await?;
            }
        }
    }

    async fn dispatch(
        &self,
        ctx: &CancellationToken,
        handler: &InboundHandler,
        tasks: &patchbay_channel::RuntimeTasks,
        update: Update,
    ) -> anyhow::Result<()> {
        let Some(message) = inbound_from_update(&update, self.bot_id, &self.bot_username) else {
            return Ok(());
        };
        let is_text = message.r#type == patchbay_channel::MsgType::text();
        if !accepts_inbound_type(&message.r#type, self.media_enabled) {
            if message.source.chat_type == patchbay_channel::ChatType::p2p()
                || message.addressed_to_bot
            {
                self.notify_unsupported(ctx, &update).await;
            }
            return Ok(());
        }
        if is_text && message.text.is_empty() {
            return Ok(());
        }
        if let Err(error) = handler.call(ctx.clone(), message.clone()).await {
            self.notify_issue_dispatch_error(ctx, tasks, message);
            return Err(error);
        }
        Ok(())
    }

    async fn notify_unsupported(&self, ctx: &CancellationToken, update: &Update) {
        let Some(message) = update.message.as_ref() else {
            return;
        };
        let params = SendMessageParams {
            chat_id: message.chat.id,
            text: UNSUPPORTED_TYPE_TEXT.to_string(),
            message_thread_id: message.message_thread_id,
            reply_parameters: Some(ReplyParameters {
                message_id: message.message_id,
                allow_sending_without_reply: true,
            }),
            ..Default::default()
        };
        let result = tokio::select! {
            _ = ctx.cancelled() => return,
            result = self.api.send_message(&params) => result,
        };
        if let Err(error) = result {
            tracing::warn!(%error, "telegram unsupported-type notice failed");
        }
    }

    fn notify_issue_dispatch_error(
        &self,
        ctx: &CancellationToken,
        tasks: &patchbay_channel::RuntimeTasks,
        message: patchbay_channel::InboundMessage,
    ) {
        if !is_addressed_issue_command(&message) {
            return;
        }
        let api = self.api.clone();
        let ctx = ctx.child_token();
        tasks.spawn(async move {
            let Ok(chat_id) = message.source.chat_id.parse::<i64>() else {
                tracing::warn!(chat_id = %message.source.chat_id, "telegram issue dispatch-error reply has invalid chat id");
                return;
            };
            let thread_id = message.source.thread_id.parse().unwrap_or(0);
            let reply_to = crate::sender::parse_message_ref(&message.message_id);
            let params = SendMessageParams {
                chat_id,
                text: ISSUE_DISPATCH_FAILED_TEXT.to_string(),
                message_thread_id: thread_id,
                reply_parameters: (reply_to != 0).then_some(ReplyParameters {
                    message_id: reply_to,
                    allow_sending_without_reply: true,
                }),
                ..Default::default()
            };
            tokio::select! {
                biased;
                _ = ctx.cancelled() => {}
                result = tokio::time::timeout(
                    ISSUE_ERROR_REPLY_TIMEOUT,
                    api.send_message(&params),
                ) => match result {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => {
                        tracing::warn!(%error, "telegram issue dispatch-error reply failed")
                    }
                    Err(_) => tracing::warn!("telegram issue dispatch-error reply timed out"),
                }
            }
        });
    }
}

fn is_addressed_issue_command(message: &patchbay_channel::InboundMessage) -> bool {
    if !message.addressed_to_bot {
        return false;
    }
    let source = if message.command_text.is_empty() {
        &message.text
    } else {
        &message.command_text
    };
    patchbay_channel_engine::parse_issue_command(source).is_some()
}

fn is_supported_media_type(message_type: &patchbay_channel::MsgType) -> bool {
    *message_type == patchbay_channel::MsgType::image()
        || *message_type == patchbay_channel::MsgType::audio()
        || *message_type == patchbay_channel::MsgType::video()
        || *message_type == patchbay_channel::MsgType::file()
}

fn accepts_inbound_type(message_type: &patchbay_channel::MsgType, media_enabled: bool) -> bool {
    *message_type == patchbay_channel::MsgType::text()
        || (media_enabled && is_supported_media_type(message_type))
}

async fn sleep_or_cancel(ctx: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        _ = ctx.cancelled() => false,
        _ = tokio::time::sleep(duration) => true,
    }
}

#[derive(Clone, Default)]
pub struct ChannelDeps {
    pub decrypt: Option<Arc<DecrypterFn>>,
    pub api_base: String,
    pub media_enabled: bool,
}

pub fn register_telegram(registry: &patchbay_channel::Registry, deps: ChannelDeps) {
    registry.register(
        patchbay_channel::Type(crate::TYPE_TELEGRAM.to_string()),
        new_telegram_factory(deps),
    );
}

pub fn new_telegram_factory(deps: ChannelDeps) -> Factory {
    Arc::new(move |cfg: Config| {
        let deps = deps.clone();
        Box::pin(async move {
            let raw = serde_json::to_vec(&cfg.raw).map_err(|error| {
                anyhow::anyhow!("telegram: encode installation config: {error}")
            })?;
            let credentials = decode_credentials(&raw, deps.decrypt.as_deref())?;
            if credentials.bot_token.is_empty() {
                anyhow::bail!("telegram: installation has no bot token");
            }
            let bot_id = credentials.bot_id.parse::<i64>().map_err(|error| {
                anyhow::anyhow!("telegram: installation app_id is not a bot id: {error}")
            })?;
            Ok(Arc::new(TelegramChannel {
                bot_id,
                bot_username: credentials.bot_username,
                api: BotApi::new(&deps.api_base, &credentials.bot_token),
                handler: cfg.handler,
                media_enabled: deps.media_enabled,
            }) as BuiltChannel)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_types_require_the_production_resolver() {
        for message_type in [
            patchbay_channel::MsgType::image(),
            patchbay_channel::MsgType::audio(),
            patchbay_channel::MsgType::video(),
            patchbay_channel::MsgType::file(),
        ] {
            assert!(accepts_inbound_type(&message_type, true));
            assert!(!accepts_inbound_type(&message_type, false));
        }
        assert!(accepts_inbound_type(&patchbay_channel::MsgType::text(), false));
        assert!(!accepts_inbound_type(
            &patchbay_channel::MsgType::unknown(),
            true
        ));
    }
}
