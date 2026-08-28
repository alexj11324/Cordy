use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use cordy_channel::{BuiltChannel, Capability, Channel, Config, Factory, InboundHandler};

use crate::api::Client;
use crate::config::{decode_credentials, DecrypterFn};
use crate::inbound::inbound_from_message;

pub struct WeixinChannel {
    installation_id: uuid::Uuid,
    bot_id: String,
    client: Client,
    handler: InboundHandler,
    pool: sqlx::PgPool,
}

#[async_trait]
impl Channel for WeixinChannel {
    fn r#type(&self) -> cordy_channel::Type {
        cordy_channel::Type(crate::TYPE_WEIXIN.to_string())
    }

    async fn connect(&self, ctx: CancellationToken) -> anyhow::Result<()> {
        let mut cursor = cordy_db::queries::channel::get_channel_receive_cursor(
            &self.pool,
            self.installation_id,
            crate::TYPE_WEIXIN,
        )
        .await?
        .unwrap_or_default();
        loop {
            let response = tokio::select! {
                _ = ctx.cancelled() => return Ok(()),
                response = self.client.get_updates(&cursor) => response,
            };
            match response {
                Ok(response) => {
                    let next_cursor = response.get_updates_buf;
                    for raw in response.msgs {
                        if let Some(message) = inbound_from_message(&raw, &self.bot_id) {
                            self.handler.call(ctx.clone(), message).await?;
                        } else if raw.message_type == 1
                            && !raw.from_user_id.is_empty()
                            && raw.from_user_id != self.bot_id
                            && raw.group_id.is_empty()
                            && !raw.context_token.is_empty()
                            && !raw.item_list.is_empty()
                        {
                            self.client
                                .send_text(
                                    &raw.from_user_id,
                                    &raw.context_token,
                                    "This WeChat connection currently supports text messages only.",
                                )
                                .await?;
                        }
                    }
                    if !next_cursor.is_empty() {
                        let mut tx = self.pool.begin().await?;
                        cordy_db::queries::channel::replace_channel_receive_cursor(
                            &mut *tx,
                            self.installation_id,
                            crate::TYPE_WEIXIN,
                            &next_cursor,
                        )
                        .await?;
                        tx.commit().await?;
                        cursor = next_cursor;
                    }
                }
                Err(error) => {
                    tracing::warn!(bot_id = %self.bot_id, %error, "weixin long poll failed");
                    tokio::select! {
                        _ = ctx.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                    }
                    return Err(error);
                }
            }
        }
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn send(
        &self,
        out: cordy_channel::OutboundMessage,
    ) -> anyhow::Result<cordy_channel::SendResult> {
        self.client
            .send_text(&out.chat_id, &out.reply_to, &out.text)
            .await?;
        Ok(cordy_channel::SendResult::default())
    }

    fn capabilities(&self) -> Capability {
        Capability::TEXT
    }
}

#[derive(Clone, Default)]
pub struct ChannelDeps {
    pub decrypt: Option<Arc<DecrypterFn>>,
    pub pool: Option<sqlx::PgPool>,
}

pub fn register(registry: &cordy_channel::Registry, deps: ChannelDeps) {
    registry.register(
        cordy_channel::Type(crate::TYPE_WEIXIN.to_string()),
        factory(deps),
    );
}

pub fn factory(deps: ChannelDeps) -> Factory {
    Arc::new(move |config: Config| {
        let deps = deps.clone();
        Box::pin(async move {
            let credentials = decode_credentials(&config.raw, deps.decrypt.as_deref())?;
            let installation_id = config
                .id
                .ok_or_else(|| anyhow::anyhow!("weixin: installation id not configured"))?;
            let pool = deps
                .pool
                .ok_or_else(|| anyhow::anyhow!("weixin: database pool not configured"))?;
            let handler = config
                .handler
                .ok_or_else(|| anyhow::anyhow!("weixin: inbound handler not configured"))?;
            Ok(Arc::new(WeixinChannel {
                installation_id,
                bot_id: credentials.bot_id,
                client: Client::new(&credentials.base_url, &credentials.bot_token)?,
                handler,
                pool,
            }) as BuiltChannel)
        })
    })
}
