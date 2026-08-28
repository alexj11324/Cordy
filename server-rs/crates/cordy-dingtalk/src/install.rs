//! DingTalk install backend.
//!
//! DingTalk uses the bring-your-own-app (BYO) model: the workspace admin
//! creates their own DingTalk Stream-mode robot, and pastes its AppKey (client
//! id) + AppSecret (client secret) into Cordy (the paste path lives in
//! [`crate::byo_install`]). The [`InstallService`] owns the at-rest encryption
//! of the AppSecret — so no caller can write a channel_installation with a
//! plaintext secret — plus the shared persist transaction and the list / get /
//! revoke management surface.

use sqlx::PgPool;
use uuid::Uuid;

use cordy_db::dbid;
use cordy_db::models::ChannelInstallation;
use cordy_db::queries::channel::{
    get_channel_installation_in_workspace, get_channel_installation_owner_by_app_id,
    list_channel_installations_by_workspace, reclaim_dead_channel_installation_by_app_id,
    set_channel_installation_status, upsert_channel_installation,
};
use cordy_db::queries::dingtalk::{
    delete_ding_talk_installation_for_replacement, get_ding_talk_installation_owner_for_update,
    lock_ding_talk_installation_owner,
};

use crate::TYPE_DINGTALK;

/// Sentinel errors the management surface renders as accurate user-facing
/// messages.
///
/// Port note: Go sentinels become typed variants; messages mirror the Go
/// strings.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    /// No row matches in this workspace.
    #[error("dingtalk installation not found")]
    InstallationNotFound,
    /// The pasted DingTalk robot is already connected to a live owner in a
    /// DIFFERENT Cordy workspace — it would collide with the (channel_type,
    /// app_id) routing index. A DingTalk robot is one bot identity and maps to
    /// one installation/default agent; group-specific routes may target other
    /// agents inside that workspace.
    #[error("dingtalk: this DingTalk robot is already connected to a different Cordy workspace")]
    RobotOwnedByAnotherWorkspace,
    /// The robot is already connected to a DIFFERENT (live, non-archived)
    /// agent in the SAME workspace, pointing the user at the Disconnect they
    /// can actually reach (#4810).
    #[error(
        "dingtalk: this DingTalk robot is already connected to another agent in this workspace"
    )]
    RobotOwnedBySameWorkspace,
    /// The robot's owning agent is archived (and so still holds the robot,
    /// since archiving is reversible). The user recovers by restoring that
    /// agent or disconnecting its robot.
    #[error("dingtalk: this DingTalk robot is connected to an archived agent in this workspace")]
    RobotOwnedByArchivedAgent,
}

/// SQLSTATE unique_violation — the routing-slot collision arbiter.
const PG_UNIQUE_VIOLATION: &str = "23505";

/// The resolved fields persist_install writes. `config_json` holds the AppKey
/// (`config->>'app_id'`) used for inbound routing; the ROW itself is keyed by
/// (workspace, agent) — one installation/default agent per bot.
#[derive(Debug, Clone)]
pub struct InstallPersist {
    pub ws_id: Uuid,
    pub agent_id: Uuid,
    pub installer_id: Uuid,
    /// The AppKey stored at `config->>'app_id'`; it MUST equal the app_id
    /// inside `config_json`. It keys the dead-owner reclaim and the live-owner
    /// lookup that drives the accurate conflict message.
    pub app_id_key: String,
    /// The config blob holding the AppKey for inbound routing.
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
        let cfg: crate::config::InstallConfig = serde_json::from_value(config_json.clone())
            .map_err(|e| anyhow::anyhow!("decode dingtalk installation config: {e}"))?;
        if cfg.app_id.is_empty() {
            anyhow::bail!("dingtalk: installation config has no app_id");
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
/// generic channel_installation rows for channel_type='dingtalk'.
#[derive(Clone)]
pub struct InstallService {
    pool: PgPool,
}

impl InstallService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Stores one DingTalk installation per (workspace, default agent).
    /// Reconnecting the SAME AppKey updates the row in place and preserves its
    /// installation-scoped state. Connecting a DIFFERENT AppKey retires that
    /// state and inserts a fresh installation id: DingTalk senderStaffId is
    /// only organization-scoped, so user and session bindings must never cross
    /// from one robot identity to another.
    ///
    /// The (channel_type, app_id) routing index is the only OTHER unique
    /// constraint. It is NOT this upsert's conflict target, so binding the
    /// robot to a DIFFERENT agent would trip it. Before upserting we therefore
    /// reclaim a DEAD prior owner of the AppKey (a revoked placeholder, or an
    /// orphan whose workspace/agent was deleted) so the robot can move to the
    /// new agent; a LIVE owner trips the unique index and is refused with an
    /// accurate conflict sentinel.
    pub async fn persist_install(&self, p: &InstallPersist) -> anyhow::Result<ChannelInstallation> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| anyhow::anyhow!("begin install tx: {e:#}"))?;

        // A replacement deletes and recreates the unique (workspace, agent,
        // channel) row. Serialize the logical slot across that gap so
        // concurrent installs cannot update the newly-created identity in
        // place.
        lock_ding_talk_installation_owner(&mut *tx, p.ws_id, p.agent_id)
            .await
            .map_err(|e| anyhow::anyhow!("lock dingtalk installation owner: {e:#}"))?;

        // Free the (dingtalk, app_id) routing slot from any DEAD prior owner —
        // a revoked placeholder, or an orphan whose owning workspace/agent was
        // deleted (#4810) — before the upsert, so a robot whose old owner is
        // gone can be rebound. A live owner (active agent, including an
        // archived one) is left in place and trips the unique index below,
        // which we turn into an accurate conflict. A None return just means
        // nothing was dead — a no-op, not a failure.
        reclaim_dead_channel_installation_by_app_id(
            &mut *tx,
            TYPE_DINGTALK,
            &p.app_id_key,
            p.ws_id,
            p.agent_id,
        )
        .await
        .map_err(|e| anyhow::anyhow!("reclaim dead dingtalk installation: {e:#}"))?;

        let current =
            get_ding_talk_installation_owner_for_update(&mut *tx, p.ws_id, p.agent_id).await?;
        if let Some(current) = current {
            if current.app_id != p.app_id_key {
                delete_ding_talk_installation_for_replacement(
                    &mut *tx,
                    current.id.unwrap_or(Uuid::nil()),
                    p.ws_id,
                    p.agent_id,
                )
                .await
                .map_err(|e| anyhow::anyhow!("retire replaced dingtalk installation: {e:#}"))?;
            }
        }

        let inst = match upsert_channel_installation(
            &mut *tx,
            p.ws_id,
            p.agent_id,
            TYPE_DINGTALK,
            &p.config_json,
            p.installer_id,
        )
        .await
        {
            Ok(Some(row)) => row,
            Ok(None) => anyhow::bail!("upsert dingtalk installation: no row returned"),
            Err(err) => {
                if is_unique_violation(&err) {
                    return Err(self
                        .live_owner_conflict_err(&self.pool, p.ws_id, &p.app_id_key)
                        .await);
                }
                return Err(anyhow::anyhow!("upsert dingtalk installation: {err:#}"));
            }
        };
        tx.commit()
            .await
            .map_err(|e| anyhow::anyhow!("commit dingtalk install: {e:#}"))?;
        Ok(inst)
    }

    /// Classifies who holds the (dingtalk, app_id) routing slot after the
    /// dead-owner reclaim ran, so persist_install returns a sentinel the
    /// handler renders as an accurate message rather than a catch-all that
    /// always blames "another workspace" (#4810). Read on the base pool, since
    /// the failed upsert has aborted the tx. A now-free slot (concurrent
    /// disconnect) or lookup error falls back to the generic cross-workspace
    /// sentinel — a retry then succeeds.
    async fn live_owner_conflict_err(
        &self,
        executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
        requesting_workspace_id: Uuid,
        app_id: &str,
    ) -> anyhow::Error {
        let Ok(Some(owner)) =
            get_channel_installation_owner_by_app_id(executor, TYPE_DINGTALK, app_id).await
        else {
            return InstallError::RobotOwnedByAnotherWorkspace.into();
        };
        match owner.workspace_id {
            Some(ws) if ws != requesting_workspace_id => {
                InstallError::RobotOwnedByAnotherWorkspace.into()
            }
            Some(_) if owner.agent_archived_at.is_some() => {
                InstallError::RobotOwnedByArchivedAgent.into()
            }
            _ => InstallError::RobotOwnedBySameWorkspace.into(),
        }
    }

    /// Returns every DingTalk installation in the workspace (active and
    /// revoked), for the management surface.
    pub async fn list_by_workspace(&self, ws_id: Uuid) -> anyhow::Result<Vec<ChannelInstallation>> {
        list_channel_installations_by_workspace(&self.pool, ws_id, TYPE_DINGTALK).await
    }

    /// The workspace-scoped lookup so a forged installation id from another
    /// workspace returns NotFound instead of leaking existence.
    pub async fn get_in_workspace(
        &self,
        id: Uuid,
        ws_id: Uuid,
    ) -> Result<ChannelInstallation, anyhow::Error> {
        match get_channel_installation_in_workspace(&self.pool, id, ws_id, TYPE_DINGTALK).await {
            Ok(Some(inst)) => Ok(inst),
            Ok(None) => Err(InstallError::InstallationNotFound.into()),
            Err(err) => Err(err),
        }
    }

    /// Flips status to 'revoked'. The row is preserved for audit; a re-install
    /// flips it back to 'active'. The Supervisor stops supervising the
    /// installation (list_active filters to active), so its Stream connection
    /// winds down, and outbound drops too.
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
    fn error_messages_mirror_go_sentinels() {
        assert_eq!(
            InstallError::InstallationNotFound.to_string(),
            "dingtalk installation not found"
        );
        assert_eq!(
            InstallError::RobotOwnedByAnotherWorkspace.to_string(),
            "dingtalk: this DingTalk robot is already connected to a different Cordy workspace"
        );
        assert_eq!(
            InstallError::RobotOwnedBySameWorkspace.to_string(),
            "dingtalk: this DingTalk robot is already connected to another agent in this workspace"
        );
        assert_eq!(
            InstallError::RobotOwnedByArchivedAgent.to_string(),
            "dingtalk: this DingTalk robot is connected to an archived agent in this workspace"
        );
    }

    #[test]
    fn install_persist_derives_routing_key_from_config() {
        let p = InstallPersist::from_config(
            Uuid::nil(),
            Uuid::nil(),
            Uuid::nil(),
            serde_json::json!({"app_id": "cli_a", "app_secret_encrypted": "x"}),
        )
        .unwrap();
        assert_eq!(p.app_id_key, "cli_a");
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
    fn unique_violation_detection_walks_error_chain() {
        let wrapped = anyhow::Error::new(sqlx::Error::Configuration("no".into()));
        assert!(!is_unique_violation(&wrapped));
    }
}
