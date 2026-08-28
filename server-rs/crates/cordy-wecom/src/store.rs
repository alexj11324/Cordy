//! The data-layer adapter behind wecom's resolvers — port of `store.go`.
//!
//! It rides on the generalized channel_* tables using the shared generated
//! queries; nothing here writes wecom-specific SQL. All helpers scope by
//! `channel_type = 'wecom'` internally.
//!
//! Port note: Go's Store embeds *db.Queries and a WithTx handle; Rust's
//! executor-generic query functions make the pool itself the transaction
//! seam, so the Store collapses to a typed handle over the pool.

use sqlx::PgPool;
use uuid::Uuid;

use crate::types::{installation_from_row, Installation};

/// The read/write surface the wecom resolvers and outbound path use.
#[derive(Clone)]
pub struct Store {
    pool: PgPool,
}

impl Store {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Looks up an installation by its smart-bot id. The routing key column
    /// is config->>'app_id' stored as the BotID directly (see
    /// encode_install_config) so the shared idx_channel_installation_type_appid
    /// index does the work.
    pub async fn get_installation_by_bot_id(&self, bot_id: &str) -> anyhow::Result<Installation> {
        let row = cordy_db::queries::channel::get_channel_installation_by_app_id(
            &self.pool,
            crate::CHANNEL_TYPE_WECOM,
            bot_id,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("wecom: installation not found for bot {bot_id}"))?;
        // Preserve the channel-facing missing-installation error contract.
        installation_from_row(&row)
    }

    /// Loads an installation by primary key, scoped to channel_type = 'wecom'
    /// so a Feishu id passed here is not silently reused.
    pub async fn get_installation(&self, id: Uuid) -> anyhow::Result<Installation> {
        let row = cordy_db::queries::channel::get_channel_installation(
            &self.pool,
            id,
            crate::CHANNEL_TYPE_WECOM,
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("wecom: installation {id} not found"))?;
        installation_from_row(&row)
    }

    /// Re-checks membership at inbound time. With channel_* FKs removed
    /// (MUL-3515 §4) a stale binding could otherwise route a message to a
    /// user who has since left the workspace.
    pub async fn is_workspace_member(
        &self,
        workspace_id: Uuid,
        user_id: Uuid,
    ) -> anyhow::Result<bool> {
        Ok(cordy_db::queries::member::get_member_by_user_and_workspace(
            &self.pool,
            user_id,
            workspace_id,
        )
        .await?
        .is_some())
    }
}
