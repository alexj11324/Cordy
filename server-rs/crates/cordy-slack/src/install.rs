//! Slack install backend — port of
//! `server/internal/integrations/slack/install.go` (MUL-3666).
//!
//! Slack uses the bring-your-own-app (BYO) model: the workspace admin creates
//! their own Slack app, installs it to their Slack workspace, and pastes its
//! bot token (`xoxb-`) + app-level token (`xapp-`) into Cordy. The InstallService
//! owns the shared persist transaction and the list / get / revoke management
//! surface; at-rest token encryption lives with the wiring that composes the
//! config blob before calling [`InstallService::persist_install`].

use sqlx::PgPool;
use uuid::Uuid;

use cordy_db::dbid;
use cordy_db::models::ChannelInstallation;
use cordy_db::queries::channel::{
    get_channel_installation_in_workspace, get_channel_installation_owner_by_app_id,
    list_channel_installations_by_workspace, reclaim_dead_channel_installation_by_app_id,
    set_channel_installation_status, upsert_channel_installation,
};

use crate::config::InstallConfig;
use crate::TYPE_SLACK;

/// Sentinel errors the management surface renders as accurate user-facing
/// messages.
///
/// Port note: Go sentinels become typed variants; messages mirror the Go
/// strings.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    /// No row matches in this workspace.
    #[error("slack installation not found")]
    InstallationNotFound,
    /// The pasted Slack app is already connected to a live owner in a
    /// DIFFERENT Cordy workspace — it would collide with the
    /// (channel_type, app_id) routing index. A Slack app is one bot identity
    /// and maps to one agent; reusing it here requires disconnecting it in the
    /// other workspace first.
    #[error("slack: this Slack app is already connected to a different Cordy workspace")]
    TeamOwnedByAnotherWorkspace,
    /// The app is already connected to a DIFFERENT (live, non-archived) agent
    /// in the SAME workspace. The old catch-all wrongly blamed "another
    /// workspace"; naming the same-workspace case points the user at the
    /// Disconnect they can actually reach (#4810).
    #[error("slack: this Slack app is already connected to another agent in this workspace")]
    TeamOwnedBySameWorkspace,
    /// The app's owning agent is archived (and so still holds the bot, since
    /// archiving is reversible). The user recovers by restoring that agent or
    /// disconnecting its bot.
    #[error("slack: this Slack app is connected to an archived agent in this workspace")]
    TeamOwnedByArchivedAgent,
}

/// SQLSTATE unique_violation — the routing-slot collision arbiter.
const PG_UNIQUE_VIOLATION: &str = "23505";

/// The resolved fields persist_install writes.
#[derive(Debug, Clone)]
pub struct InstallPersist {
    pub ws_id: Uuid,
    pub agent_id: Uuid,
    pub installer_id: Uuid,
    /// The value stored at `config->>'app_id'` — the real Slack app id — and
    /// MUST equal the app_id inside `config_json`; it keys the dead-owner
    /// reclaim and the live-owner lookup that drives the accurate conflict
    /// message.
    pub app_id_key: String,
    /// The config blob holding the Slack app id used for inbound routing; the
    /// ROW itself is keyed by (workspace, agent) — one bot per agent.
    pub config_json: serde_json::Value,
}

impl InstallPersist {
    /// Composes an InstallPersist from a decoded config, deriving the routing
    /// key from the config's own app_id so the two can never disagree.
    pub fn from_config(
        ws_id: Uuid,
        agent_id: Uuid,
        installer_id: Uuid,
        config_json: serde_json::Value,
    ) -> anyhow::Result<Self> {
        let cfg: InstallConfig = serde_json::from_value(config_json.clone())
            .map_err(|e| anyhow::anyhow!("decode slack installation config: {e}"))?;
        if cfg.app_id.is_empty() {
            anyhow::bail!("slack: installation config has no app_id");
        }
        Ok(Self {
            ws_id,
            agent_id,
            installer_id,
            app_id_key: cfg.app_id,
            config_json,
        })
    }
}

/// Owns the shared install transaction and the management surface over the
/// generic channel_installation rows for channel_type='slack'.
#[derive(Clone)]
pub struct InstallService {
    pool: PgPool,
}

impl InstallService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Upserts the installation keyed by (workspace_id, agent_id,
    /// channel_type): ONE Slack bot per agent. Re-connecting an agent —
    /// including swapping it to a NEW Slack app after a disconnect — UPDATES
    /// that agent's row in place instead of colliding with the
    /// (workspace, agent, channel) unique.
    ///
    /// The (channel_type, app_id) routing index is the only OTHER unique
    /// constraint, and it is NOT this upsert's conflict target, so a unique
    /// violation here means the pasted Slack app is already connected to a
    /// DIFFERENT agent or Cordy workspace — refuse it
    /// ([`InstallError::TeamOwnedByAnotherWorkspace`]) rather than steal it.
    /// No chat-session retire is needed: a row's agent_id never changes (it is
    /// part of the key), so existing sessions stay valid for the same agent.
    pub async fn persist_install(&self, p: &InstallPersist) -> anyhow::Result<ChannelInstallation> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| anyhow::anyhow!("begin install tx: {e:#}"))?;

        // Free the (slack, app_id) routing slot from any DEAD prior owner — a
        // revoked placeholder, or an orphan whose owning workspace/agent was
        // deleted (#4810) — before the upsert, so a bot whose old owner is gone
        // can be rebound. A live owner (active agent, including an archived
        // one) is left in place and trips the unique index below, which we turn
        // into an accurate conflict. A None return just means nothing was dead
        // — a no-op, not a failure.
        reclaim_dead_channel_installation_by_app_id(
            &mut *tx,
            TYPE_SLACK,
            &p.app_id_key,
            p.ws_id,
            p.agent_id,
        )
        .await
        .map_err(|e| anyhow::anyhow!("reclaim dead slack installation: {e:#}"))?;

        let inst = match upsert_channel_installation(
            &mut *tx,
            p.ws_id,
            p.agent_id,
            TYPE_SLACK,
            &p.config_json,
            p.installer_id,
        )
        .await
        {
            Ok(Some(row)) => row,
            Ok(None) => anyhow::bail!("upsert slack installation: no row returned"),
            Err(err) => {
                if is_unique_violation(&err) {
                    // A failed statement leaves this transaction aborted, and
                    // the owner lookup runs on the base pool. End the failed
                    // transaction first so a burst of conflicting installs
                    // cannot occupy every pool connection while each request
                    // waits for a second connection to classify its conflict.
                    if let Err(rollback_error) = tx.rollback().await {
                        tracing::warn!(
                            error = %rollback_error,
                            "rollback failed Slack install conflict transaction"
                        );
                    }
                    return Err(self
                        .live_owner_conflict_err(&self.pool, p.ws_id, &p.app_id_key)
                        .await);
                }
                return Err(anyhow::anyhow!("upsert slack installation: {err:#}"));
            }
        };
        tx.commit()
            .await
            .map_err(|e| anyhow::anyhow!("commit slack install: {e:#}"))?;
        Ok(inst)
    }

    /// Classifies who holds the (slack, app_id) routing slot after the
    /// dead-owner reclaim ran, so persist_install returns a sentinel the
    /// handler renders as an accurate message rather than the old catch-all
    /// that always blamed "another workspace" (#4810). Read on the base pool,
    /// since the failed upsert has aborted the tx. A now-free slot (concurrent
    /// disconnect) or lookup error falls back to the generic cross-workspace
    /// sentinel — a retry then succeeds.
    async fn live_owner_conflict_err(
        &self,
        executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
        requesting_workspace_id: Uuid,
        app_id: &str,
    ) -> anyhow::Error {
        let Ok(Some(owner)) =
            get_channel_installation_owner_by_app_id(executor, TYPE_SLACK, app_id).await
        else {
            return InstallError::TeamOwnedByAnotherWorkspace.into();
        };
        match owner.workspace_id {
            Some(ws) if ws != requesting_workspace_id => {
                InstallError::TeamOwnedByAnotherWorkspace.into()
            }
            Some(_) if owner.agent_archived_at.is_some() => {
                InstallError::TeamOwnedByArchivedAgent.into()
            }
            _ => InstallError::TeamOwnedBySameWorkspace.into(),
        }
    }

    /// Returns every Slack installation in the workspace (active and revoked),
    /// for the management surface.
    pub async fn list_by_workspace(&self, ws_id: Uuid) -> anyhow::Result<Vec<ChannelInstallation>> {
        list_channel_installations_by_workspace(&self.pool, ws_id, TYPE_SLACK).await
    }

    /// The workspace-scoped lookup so a forged installation id from another
    /// workspace returns NotFound instead of leaking existence.
    pub async fn get_in_workspace(
        &self,
        id: Uuid,
        ws_id: Uuid,
    ) -> Result<ChannelInstallation, anyhow::Error> {
        match get_channel_installation_in_workspace(&self.pool, id, ws_id, TYPE_SLACK).await {
            Ok(Some(inst)) => Ok(inst),
            Ok(None) => Err(InstallError::InstallationNotFound.into()),
            Err(err) => Err(err),
        }
    }

    /// Flips status to 'revoked'. The row is preserved for audit; a re-install
    /// flips it back to 'active'. The Supervisor stops supervising the
    /// installation (ListActiveInstallations filters to active), so its Socket
    /// Mode connection winds down, and outbound drops too.
    pub async fn revoke(&self, id: Uuid) -> anyhow::Result<()> {
        set_channel_installation_status(&self.pool, id, "revoked").await?;
        Ok(())
    }

    /// Mints a fresh primary key for a caller composing audit/management rows.
    /// Kept next to the service so adapters do not reach into cordy-db directly.
    pub fn new_row_id() -> Uuid {
        dbid::new_v7()
    }
}

fn is_unique_violation(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<sqlx::postgres::PgDatabaseError>()
            .is_some_and(|pg| pg.code() == PG_UNIQUE_VIOLATION)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_persist_derives_routing_key_from_config() {
        let p = InstallPersist::from_config(
            Uuid::nil(),
            Uuid::nil(),
            Uuid::nil(),
            serde_json::json!({"app_id": "A123", "bot_token_encrypted": "x"}),
        )
        .unwrap();
        assert_eq!(p.app_id_key, "A123");
    }

    #[test]
    fn install_persist_rejects_config_without_app_id() {
        assert!(InstallPersist::from_config(
            Uuid::nil(),
            Uuid::nil(),
            Uuid::nil(),
            serde_json::json!({})
        )
        .is_err());
    }

    #[test]
    fn error_messages_mirror_go_sentinels() {
        assert_eq!(
            InstallError::InstallationNotFound.to_string(),
            "slack installation not found"
        );
        assert_eq!(
            InstallError::TeamOwnedByAnotherWorkspace.to_string(),
            "slack: this Slack app is already connected to a different Cordy workspace"
        );
        assert_eq!(
            InstallError::TeamOwnedBySameWorkspace.to_string(),
            "slack: this Slack app is already connected to another agent in this workspace"
        );
        assert_eq!(
            InstallError::TeamOwnedByArchivedAgent.to_string(),
            "slack: this Slack app is connected to an archived agent in this workspace"
        );
    }

    #[test]
    fn unique_violation_detection_walks_error_chain() {
        let wrapped = anyhow::Error::new(sqlx::Error::Configuration("no".into()));
        assert!(!is_unique_violation(&wrapped));
    }
}
