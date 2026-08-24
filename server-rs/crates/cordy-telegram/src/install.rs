//! Transactional Telegram installation persistence and ownership routing.

use sqlx::PgPool;
use uuid::Uuid;

use cordy_db::models::ChannelInstallation;
use cordy_db::queries::channel::{
    get_channel_installation_owner_by_app_id, reclaim_dead_channel_installation_by_app_id,
    upsert_channel_installation,
};

use crate::TYPE_TELEGRAM;

const PG_UNIQUE_VIOLATION: &str = "23505";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("telegram: this bot is already connected to another agent in this workspace")]
    BotOwnedBySameWorkspace,
    #[error("telegram: this bot is connected to an archived agent in this workspace")]
    BotOwnedByArchivedAgent,
    #[error("telegram: this bot is already connected to a different Cordy workspace")]
    BotOwnedByAnotherWorkspace,
}

#[derive(Debug, Clone)]
pub struct InstallPersist {
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub installer_id: Uuid,
    pub bot_id: String,
    pub config: serde_json::Value,
}

impl InstallPersist {
    pub fn new(
        workspace_id: Uuid,
        agent_id: Uuid,
        installer_id: Uuid,
        bot_id: impl Into<String>,
        config: serde_json::Value,
    ) -> anyhow::Result<Self> {
        let bot_id = bot_id.into();
        if bot_id.is_empty()
            || config.get("app_id").and_then(serde_json::Value::as_str) != Some(bot_id.as_str())
        {
            anyhow::bail!("telegram: installation config bot id does not match routing key");
        }
        Ok(Self {
            workspace_id,
            agent_id,
            installer_id,
            bot_id,
            config,
        })
    }
}

#[derive(Clone)]
pub struct InstallService {
    pool: PgPool,
}

impl InstallService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Reclaims dead owners and atomically upserts the bot for one agent.
    /// A live routing-slot collision is classified after Postgres arbitrates
    /// the unique index, so concurrent installers cannot both succeed.
    pub async fn persist_install(
        &self,
        params: &InstallPersist,
    ) -> anyhow::Result<ChannelInstallation> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|error| anyhow::anyhow!("begin Telegram install: {error:#}"))?;
        reclaim_dead_channel_installation_by_app_id(
            &mut *tx,
            TYPE_TELEGRAM,
            &params.bot_id,
            params.workspace_id,
            params.agent_id,
        )
        .await
        .map_err(|error| anyhow::anyhow!("reclaim dead Telegram installation: {error:#}"))?;

        let installation = match upsert_channel_installation(
            &mut *tx,
            params.workspace_id,
            params.agent_id,
            TYPE_TELEGRAM,
            &params.config,
            params.installer_id,
        )
        .await
        {
            Ok(Some(installation)) => installation,
            Ok(None) => anyhow::bail!("upsert Telegram installation: no row returned"),
            Err(error) if is_unique_violation(&error) => {
                return Err(self
                    .live_owner_conflict(params.workspace_id, &params.bot_id)
                    .await);
            }
            Err(error) => return Err(anyhow::anyhow!("upsert Telegram installation: {error:#}")),
        };
        tx.commit()
            .await
            .map_err(|error| anyhow::anyhow!("commit Telegram install: {error:#}"))?;
        Ok(installation)
    }

    async fn live_owner_conflict(
        &self,
        requesting_workspace_id: Uuid,
        bot_id: &str,
    ) -> anyhow::Error {
        let Ok(Some(owner)) =
            get_channel_installation_owner_by_app_id(&self.pool, TYPE_TELEGRAM, bot_id).await
        else {
            return InstallError::BotOwnedByAnotherWorkspace.into();
        };
        match owner.workspace_id {
            Some(workspace_id) if workspace_id != requesting_workspace_id => {
                InstallError::BotOwnedByAnotherWorkspace.into()
            }
            Some(_) if owner.agent_archived_at.is_some() => {
                InstallError::BotOwnedByArchivedAgent.into()
            }
            _ => InstallError::BotOwnedBySameWorkspace.into(),
        }
    }
}

fn is_unique_violation(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<sqlx::postgres::PgDatabaseError>()
            .is_some_and(|database| database.code() == PG_UNIQUE_VIOLATION)
            || cause.downcast_ref::<sqlx::Error>().is_some_and(|sqlx| {
                sqlx.as_database_error()
                    .is_some_and(|database| database.code().as_deref() == Some(PG_UNIQUE_VIOLATION))
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persist_requires_config_and_routing_bot_ids_to_match() {
        assert!(InstallPersist::new(
            Uuid::nil(),
            Uuid::nil(),
            Uuid::nil(),
            "123",
            serde_json::json!({"app_id":"123"}),
        )
        .is_ok());
        assert!(InstallPersist::new(
            Uuid::nil(),
            Uuid::nil(),
            Uuid::nil(),
            "123",
            serde_json::json!({"app_id":"456"}),
        )
        .is_err());
    }

    #[test]
    fn ownership_errors_preserve_go_recovery_scopes() {
        assert!(InstallError::BotOwnedBySameWorkspace
            .to_string()
            .contains("another agent in this workspace"));
        assert!(InstallError::BotOwnedByArchivedAgent
            .to_string()
            .contains("archived agent"));
        assert!(InstallError::BotOwnedByAnotherWorkspace
            .to_string()
            .contains("different Cordy workspace"));
    }
}
