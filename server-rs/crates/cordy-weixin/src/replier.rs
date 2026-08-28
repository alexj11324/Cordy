use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use rand::RngCore as _;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use cordy_channel::InboundMessage;
use cordy_channel_engine::resolvers::{
    OutboundReplier as ReplierSeam, Outcome, ResolvedInstallation, Result as EngineResult,
};

use crate::api::Client;
use crate::config::{decode_credentials, DecrypterFn};
use crate::inbound::WeixinRawEvent;

const AGENT_OFFLINE: &str = "⚠️ The agent is offline. Your message was saved and will continue when its runtime reconnects.";
const AGENT_ARCHIVED: &str = "⚠️ This agent has been archived. Please contact a workspace admin.";
const FRESH_PENDING: &str =
    "✅ Fresh start ready. Your next message will run without prior context.";
const ISSUE_USAGE: &str =
    "Please include a task title:\n\n/issue <title>\n[description] (optional)";

pub struct OutboundReplier {
    pool: sqlx::PgPool,
    decrypt: Option<Arc<DecrypterFn>>,
    app_url: String,
}

impl OutboundReplier {
    pub fn new(pool: sqlx::PgPool, decrypt: Option<Arc<DecrypterFn>>, app_url: String) -> Self {
        Self {
            pool,
            decrypt,
            app_url: app_url.trim_end_matches('/').to_string(),
        }
    }

    async fn post(
        &self,
        ctx: CancellationToken,
        installation: &ResolvedInstallation,
        message: &InboundMessage,
        text: &str,
    ) -> anyhow::Result<()> {
        let raw: WeixinRawEvent = serde_json::from_value(message.raw.clone())?;
        let row = installation
            .platform
            .downcast_ref::<cordy_db::models::ChannelInstallation>()
            .ok_or_else(|| anyhow::anyhow!("weixin installation row unavailable"))?;
        let credentials = decode_credentials(&row.config, self.decrypt.as_deref())?;
        let client = Client::new(&credentials.base_url, &credentials.bot_token)?;
        tokio::select! {
            _ = ctx.cancelled() => Ok(()),
            result = client.send_text(&message.source.sender_id, &raw.context_token, text) => result,
        }
    }

    async fn binding_prompt(
        &self,
        ctx: CancellationToken,
        installation: &ResolvedInstallation,
        message: &InboundMessage,
    ) -> anyhow::Result<()> {
        if self.app_url.is_empty() {
            anyhow::bail!("weixin binding app URL is not configured");
        }
        let mut bytes = [0_u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let raw_token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let token_hash = format!("{:x}", Sha256::digest(raw_token.as_bytes()));
        let expires_at = chrono::Utc::now() + chrono::Duration::minutes(15);
        cordy_db::queries::channel::create_channel_binding_token(
            &self.pool,
            &token_hash,
            installation.workspace_id,
            installation.id,
            crate::TYPE_WEIXIN,
            &message.source.sender_id,
            Some(expires_at),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("weixin binding token was not persisted"))?;
        let encoded: String = url::form_urlencoded::byte_serialize(raw_token.as_bytes()).collect();
        self.post(
            ctx,
            installation,
            message,
            &format!(
                "👋 Link your Patchbay account to continue:\n{}/weixin/bind?token={}\n(This link expires in 15 minutes.)",
                self.app_url, encoded
            ),
        )
        .await
    }
}

#[async_trait]
impl ReplierSeam for OutboundReplier {
    async fn reply(
        &self,
        ctx: CancellationToken,
        installation: &ResolvedInstallation,
        message: &InboundMessage,
        result: &EngineResult,
    ) {
        let Some(outcome) = result.outcome.as_ref() else {
            return;
        };
        let send = if *outcome == Outcome::needs_binding() {
            self.binding_prompt(ctx, installation, message).await
        } else if *outcome == Outcome::agent_offline() {
            self.post(ctx, installation, message, AGENT_OFFLINE).await
        } else if *outcome == Outcome::agent_archived() {
            self.post(ctx, installation, message, AGENT_ARCHIVED).await
        } else if *outcome == Outcome::fresh_pending() {
            self.post(ctx, installation, message, FRESH_PENDING).await
        } else if *outcome == Outcome::issue_usage() {
            self.post(ctx, installation, message, ISSUE_USAGE).await
        } else if *outcome == Outcome::ingested() && result.issue_id.is_some() {
            let identifier = if !result.issue_identifier.is_empty() {
                result.issue_identifier.clone()
            } else {
                result.issue_id.map(|id| id.to_string()).unwrap_or_default()
            };
            self.post(
                ctx,
                installation,
                message,
                &format!("✅ Created {identifier} — {}", result.issue_title.trim()),
            )
            .await
        } else {
            Ok(())
        };
        if let Err(error) = send {
            tracing::warn!(installation_id = %installation.id, %error, "weixin outcome reply failed");
        }
    }
}
