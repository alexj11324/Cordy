//! PostgreSQL-backed inbound drop audit for Feishu/Lark.

use async_trait::async_trait;

use crate::chat::{AuditDropParams, AuditLogger};

pub struct DbAuditLogger {
    pool: sqlx::PgPool,
}

impl DbAuditLogger {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl AuditLogger for DbAuditLogger {
    async fn record_drop(&self, p: AuditDropParams) {
        if let Err(error) = patchbay_db::queries::channel::record_channel_inbound_drop(
            &self.pool,
            crate::channel_store::CHANNEL_TYPE_FEISHU,
            &p.event_type,
            &p.reason.0,
            (!p.installation_id.is_nil()).then_some(p.installation_id),
            (!p.chat_id.0.is_empty()).then_some(p.chat_id.0.as_str()),
            (!p.lark_event_id.is_empty()).then_some(p.lark_event_id.as_str()),
            (!p.lark_message_id.is_empty()).then_some(p.lark_message_id.as_str()),
            patchbay_db::dbid::new_v7(),
        )
        .await
        {
            tracing::warn!(%error, "lark audit: record drop failed");
        }
    }
}
