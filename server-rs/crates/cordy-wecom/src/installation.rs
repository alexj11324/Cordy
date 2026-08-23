//! The write surface for wecom channel_installation rows — port of
//! `installation.go`.
//!
//! It centralises secretbox encryption of the smart-bot secret so no caller
//! ever handles plaintext beyond this file's boundary, and it is the ONLY
//! path to a wecom row in channel_installation — an admin CLI or an HTTP
//! install endpoint both go through [`InstallationService::upsert`].

use std::sync::Arc;

use sqlx::PgPool;
use uuid::Uuid;

use cordy_db::queries::channel::{
    get_channel_installation_in_workspace, get_channel_installation_owner_by_app_id,
    get_channel_installation_slot_owner_by_app_id, list_channel_installations_by_workspace,
    lock_channel_installation_app_id_slot, reclaim_dead_channel_installation_by_app_id,
    set_channel_installation_status, upsert_channel_installation,
};
use cordy_util::secretbox::SecretBox;
use tokio_util::sync::CancellationToken;

use crate::credential_probe::{CredentialProbe, HandshakeProbe};
use crate::types::{encode_install_config, installation_from_row, INSTALLATION_REVOKED};

/// The plaintext-bearing input to [`InstallationService::upsert`]. The caller
/// supplies the raw (BotID, Secret) pair from the WeCom admin console; the
/// service seals Secret before it touches the DB.
#[derive(Debug, Clone, Default)]
pub struct InstallationParams {
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub installer_user_id: Uuid,

    /// The smart-bot identifier shown on the WeCom admin console. Stable
    /// per-bot; used as both auth identity in the subscribe frame and the
    /// routing key persisted at config->>'app_id'.
    pub bot_id: String,

    /// The plaintext long-connection secret shown once at bot creation on the
    /// admin console. Sealed at the service boundary.
    pub secret: String,

    /// The bot's name as it appears in a chat. Optional; see
    /// [`Installation::bot_display_name`] for why it exists and what an empty
    /// value falls back to. Empty on a re-install of the SAME bot keeps
    /// whatever is already on the row.
    pub bot_display_name: String,
}

/// Creates, refreshes and revokes wecom smart-bot installations through the
/// shared channel_installation table.
///
/// Port note: Go injects `*db.Queries` + an engine.TxStarter so the whole
/// sequence runs in one transaction; Rust threads the transaction through the
/// executor-generic query functions directly, with the pool as the source.
/// The credential probe is required structurally (`Arc<dyn …>`), mirroring
/// Go's refusal to build the service without one — proof of control gates a
/// statement that hard-deletes another workspace's installation and every
/// binding under it, so there is no fail-open setting.
pub struct InstallationService {
    pool: PgPool,
    box_: SecretBox,
    probe: Arc<dyn CredentialProbe>,
}

impl InstallationService {
    /// Binds the service to a pool and a secretbox keyed for at-rest
    /// encryption. The probe defaults to the real handshake against WeCom;
    /// pass [`with_probe`](Self::with_probe) to substitute a fake (tests that
    /// must not open a socket). There is no option that leaves the check off.
    pub fn new(pool: PgPool, box_: SecretBox) -> Self {
        Self {
            pool,
            box_,
            probe: Arc::new(HandshakeProbe::new(None, "")),
        }
    }

    /// Overrides the control check. Production omits it.
    pub fn with_probe(mut self, probe: Arc<dyn CredentialProbe>) -> Self {
        self.probe = probe;
        self
    }

    /// Creates or refreshes an installation row. The conflict key on
    /// channel_installation is (workspace_id, agent_id, channel_type), so
    /// re-running upsert against an existing triple rotates every field on
    /// the row and flips status back to 'active'. The returned Installation
    /// reflects the post-write DB state.
    ///
    /// The whole sequence — lock the bot's routing slot, read its current
    /// owner, refuse or probe, reclaim, write — runs inside one transaction.
    /// Order is the point:
    ///
    /// - idx_channel_installation_type_appid is UNIQUE on
    ///   (channel_type, config->>'app_id') with NO workspace in it, so a bot
    ///   id's routing slot is global across the deployment, and the reclaim
    ///   hard-deletes whoever holds it along with every user binding,
    ///   chat-session binding and pending token beneath. Bot ids are not
    ///   secret — every member talking to the bot can read one — so the
    ///   reclaim has to be gated on proof that the caller controls the bot.
    ///   That is the probe.
    /// - But the probe is itself a side effect on the live platform: it
    ///   subscribes, and WeCom allows exactly one live subscriber per bot, so
    ///   subscribing displaces whoever is connected. Running it before
    ///   knowing who owns the slot means a request that is about to be
    ///   REFUSED still knocks the rightful owner offline. So the owner read
    ///   comes first, and a request that cannot succeed returns its conflict
    ///   having touched nothing, locally or at WeCom.
    /// - Both steps read state that a concurrent install or reconnect can
    ///   change underneath them, so they are serialized on the slot itself
    ///   (a transaction-level advisory lock).
    pub async fn upsert(
        &self,
        ctx: &CancellationToken,
        p: &InstallationParams,
    ) -> anyhow::Result<crate::types::Installation> {
        validate_installation_params(p)?;

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| anyhow::anyhow!("wecom: begin install tx: {e}"))?;

        // The serialization boundary. Held until commit/rollback, so every
        // step below sees one consistent answer for who owns this bot.
        lock_channel_installation_app_id_slot(&mut *tx, crate::CHANNEL_TYPE_WECOM, &p.bot_id)
            .await
            .map_err(|e| anyhow::anyhow!("wecom: lock bot routing slot: {e}"))?;

        // Who holds the slot right now — read inside the lock, before
        // anything external happens.
        bot_slot_conflict_err(&mut *tx, p).await?;

        // Nothing live is at stake now: the slot is free, revoked, orphaned,
        // or already this caller's own. Prove control before the reclaim acts
        // on it.
        self.probe.probe(ctx, &p.bot_id, &p.secret).await?;

        let sealed = self
            .box_
            .seal(p.secret.as_bytes())
            .map_err(|e| anyhow::anyhow!("wecom: encrypt secret: {e}"))?;

        // Reclaim-then-upsert. UpsertChannelInstallation conflicts on
        // (workspace_id, agent_id, channel_type), but the (channel_type,
        // app_id) slot is guarded by idx_channel_installation_type_appid.
        // Disconnect only flips status to 'revoked' — it does not free the
        // row — so without a reclaim step a bot revoked from agent A can
        // never be connected to agent B. The reclaim deletes any DEAD owner
        // of this bot's slot and clears its dependent rows in the same
        // statement, while leaving a LIVE owner in place to trip the index
        // below. bot_slot_conflict_err has already refused every live owner
        // above, so reaching that unique violation now means the slot changed
        // hands despite the lock; bot_owner_conflict_err still turns it into
        // an accurate conflict rather than a raw Postgres string.
        //
        // A None return just means nothing was dead — a no-op, not a failure.
        reclaim_dead_channel_installation_by_app_id(
            &mut *tx,
            crate::CHANNEL_TYPE_WECOM,
            &p.bot_id,
            p.workspace_id,
            p.agent_id,
        )
        .await
        .map_err(|e| anyhow::anyhow!("wecom: reclaim dead installation: {e}"))?;

        // The row this upsert is about to overwrite, read on the tx handle so
        // it is serialized with the write. Zero when this is a first install.
        let carried = current_installation(&mut *tx, p.workspace_id, p.agent_id).await?;
        // The chat name is optional in the dialog, so an admin rotating a
        // leaked secret leaves it blank — and blanking it would put group
        // slash commands back to the whitespace guess that this field exists
        // to replace. Keep what is on the row. A bot SWAP is different: the
        // old name belongs to the old bot, and carrying it would make the new
        // bot answer to a mention that is not its own.
        let display_name = if p.bot_display_name.is_empty() && carried.bot_id == p.bot_id {
            carried.bot_display_name.clone()
        } else {
            p.bot_display_name.clone()
        };
        let cfg = encode_install_config(&crate::types::Installation {
            bot_id: p.bot_id.clone(),
            secret_encrypted: sealed,
            bot_display_name: display_name,
            ..Default::default()
        })?;

        let row = match upsert_channel_installation(
            &mut *tx,
            p.workspace_id,
            p.agent_id,
            crate::CHANNEL_TYPE_WECOM,
            &cfg,
            p.installer_user_id,
        )
        .await
        {
            Ok(row) => row,
            Err(e) if is_unique_violation(&e) => {
                // A LIVE owner still holds the slot. Read the owner on the
                // non-tx connection (this tx is now in aborted state) to name
                // it.
                return Err(self.bot_owner_conflict_err(p.workspace_id, &p.bot_id).await);
            }
            Err(e) => return Err(anyhow::anyhow!("wecom: upsert installation: {e}")),
        };
        let row = row.ok_or_else(|| anyhow::anyhow!("wecom: upsert installation: no row"))?;
        tx.commit()
            .await
            .map_err(|e| anyhow::anyhow!("wecom: commit install tx: {e}"))?;
        installation_from_row(&row)
    }

    /// Flips status to 'revoked' — the row is preserved so audit trails
    /// remain queryable, and a subsequent upsert flips it back to 'active'
    /// atomically. A revoked row is skipped by the router's installation
    /// resolver (Active=false → invalid_event drop with audit).
    pub async fn revoke(&self, id: Uuid) -> anyhow::Result<()> {
        set_channel_installation_status(&self.pool, id, INSTALLATION_REVOKED).await?;
        Ok(())
    }

    /// Returns every wecom installation for the given workspace in creation
    /// order. Used by the Settings and Agent-Integrations tabs to render
    /// "connected bots" lists; revoked rows are included so operators can see
    /// history (the UI filters on Status).
    pub async fn list_by_workspace(
        &self,
        workspace_id: Uuid,
    ) -> anyhow::Result<Vec<crate::types::Installation>> {
        let rows = list_channel_installations_by_workspace(
            &self.pool,
            workspace_id,
            crate::CHANNEL_TYPE_WECOM,
        )
        .await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let inst = installation_from_row(row)
                .map_err(|e| anyhow::anyhow!("wecom: decode installation {}: {e}", row.id))?;
            out.push(inst);
        }
        Ok(out)
    }

    /// Loads one installation scoped to (id, workspace_id) so a forged UUID
    /// from another workspace returns not-found instead of leaking existence.
    /// Returns [`InstallationNotFound`] on either a missing row or a row that
    /// exists but belongs to another channel_type.
    pub async fn get_in_workspace(
        &self,
        id: Uuid,
        workspace_id: Uuid,
    ) -> anyhow::Result<crate::types::Installation> {
        let row = get_channel_installation_in_workspace(
            &self.pool,
            id,
            workspace_id,
            crate::CHANNEL_TYPE_WECOM,
        )
        .await?
        .ok_or(InstallationNotFound)?;
        installation_from_row(&row)
    }

    /// Names who holds the (wecom, bot_id) routing slot so the handler can
    /// tell the admin where to go. Read after the upsert failed, so a slot
    /// that has since been freed — or a lookup that fails — falls back to the
    /// cross-workspace message: it is the only one that is never wrong about
    /// where to look, and a retry succeeds anyway.
    async fn bot_owner_conflict_err(
        &self,
        requesting_workspace_id: Uuid,
        bot_id: &str,
    ) -> anyhow::Error {
        let owner = match get_channel_installation_owner_by_app_id(
            &self.pool,
            crate::CHANNEL_TYPE_WECOM,
            bot_id,
        )
        .await
        {
            Ok(Some(owner)) => owner,
            _ => return anyhow::Error::new(BotOwnershipError::AnotherWorkspace),
        };
        match owner.workspace_id {
            Some(ws) if ws != requesting_workspace_id => {
                anyhow::Error::new(BotOwnershipError::AnotherWorkspace)
            }
            _ if owner.agent_archived_at.is_some() => {
                anyhow::Error::new(BotOwnershipError::ArchivedAgent)
            }
            _ => anyhow::Error::new(BotOwnershipError::SameWorkspace),
        }
    }
}

/// Returned by [`InstallationService::get_in_workspace`] when either no row
/// exists at the given (id, workspace) or the row belongs to a different
/// channel_type. Distinct from a plain missing-row error so HTTP handlers can
/// map it to 404 without importing sqlx.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("wecom: installation not found")]
pub struct InstallationNotFound;

/// Marks a request the caller can fix by filling something in, as opposed to
/// a failure of ours. It exists so the HTTP layer can tell them apart: a
/// missing field is the caller's 400, the credential errors carry the two
/// outcomes of actually asking WeCom (400 and 503), and everything left over
/// is a 500 — because telling an admin their credentials are wrong when
/// Postgres briefly went away sends them to rotate a secret that was fine,
/// and a WeCom secret, once rotated, cannot be recovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("wecom: invalid installation parameters: {0} is required")]
pub struct InvalidInstallationParams(pub &'static str);

/// The one conflict upsert cannot resolve: the bot is already connected
/// somewhere else. UpsertChannelInstallation conflicts on
/// (workspace_id, agent_id, channel_type), but idx_channel_installation_type_appid
/// is UNIQUE on (channel_type, config->>'app_id') — so connecting the SAME
/// bot to a second agent misses the ON CONFLICT clause entirely and trips the
/// index instead. Without these the admin reads the raw Postgres text
/// ("duplicate key value violates unique constraint …") in a toast.
///
/// One bot is one connection: the WeCom long connection allows a single live
/// subscriber per bot, so two agents cannot share one. The way out is always
/// to free the bot first, which is what each message says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BotOwnershipError {
    /// Another agent in the admin's own workspace holds the bot. Reversible
    /// from the same settings screen.
    #[error("wecom: this bot is already connected to another agent in this workspace")]
    SameWorkspace,
    /// The holder is archived, so it does not show up in the agent list and
    /// the bot looks free while it is not.
    #[error("wecom: this bot is connected to an archived agent in this workspace")]
    ArchivedAgent,
    /// The holder is out of sight entirely and only someone with access there
    /// can release it.
    #[error("wecom: this bot is already connected to a different Cordy workspace")]
    AnotherWorkspace,
}

/// Downcasts the chain for the ownership sentinels, mirroring Go's
/// `errors.Is(err, ErrBotOwnedBy…)` checks at the HTTP layer.
pub fn as_bot_ownership_error(err: &anyhow::Error) -> Option<BotOwnershipError> {
    err.chain()
        .find_map(|c| c.downcast_ref::<BotOwnershipError>().copied())
}

/// Postgres' unique_violation SQLSTATE.
const PG_UNIQUE_VIOLATION: &str = "23505";

fn is_unique_violation(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<sqlx::postgres::PgDatabaseError>()
            .is_some_and(|db| db.code() == PG_UNIQUE_VIOLATION)
            || cause.downcast_ref::<sqlx::Error>().is_some_and(|e| {
                e.as_database_error()
                    .is_some_and(|d| d.code().is_some_and(|c| c == PG_UNIQUE_VIOLATION))
            })
    })
}

/// A lightweight pre-write check for required fields. It does NOT verify
/// anything against WeCom.
fn validate_installation_params(p: &InstallationParams) -> anyhow::Result<()> {
    let missing = if p.workspace_id == Uuid::nil() {
        "workspace_id"
    } else if p.agent_id == Uuid::nil() {
        "agent_id"
    } else if p.installer_user_id == Uuid::nil() {
        "installer_user_id"
    } else if p.bot_id.is_empty() {
        "bot_id"
    } else if p.secret.is_empty() {
        "secret"
    } else {
        return Ok(());
    };
    Err(anyhow::Error::new(InvalidInstallationParams(missing)))
}

/// Reads the (workspace, agent, wecom) row an upsert is about to replace, or
/// the zero Installation when there is none.
///
/// There is no query keyed on that triple — the conflict key of the upsert is
/// not something any read path needed until now — so this filters the
/// workspace's wecom rows, of which there are a handful. Running it on the tx
/// handle is what makes it serializable with the write.
async fn current_installation<'e, E>(
    executor: E,
    workspace_id: Uuid,
    agent_id: Uuid,
) -> anyhow::Result<crate::types::Installation>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows =
        list_channel_installations_by_workspace(executor, workspace_id, crate::CHANNEL_TYPE_WECOM)
            .await
            .map_err(|e| anyhow::anyhow!("wecom: read current installation: {e}"))?;
    for row in &rows {
        if row.agent_id == agent_id {
            return installation_from_row(row);
        }
    }
    Ok(crate::types::Installation::default())
}

/// Answers one question: may this install act on the (wecom, bot_id) routing
/// slot at all? It returns the sentinel to refuse with, or Ok to proceed.
/// MUST be called inside the transaction holding the slot's advisory lock —
/// outside it the answer is a guess that a concurrent install or reconnect
/// can invalidate before it is used.
///
/// It is the gate in front of BOTH side effects upsert can produce: the
/// probe's subscribe (which displaces whoever is connected to this bot at
/// WeCom) and the reclaim's hard delete (which takes the row and its
/// bindings). A refusal here costs the caller nothing and the current owner
/// nothing.
///
/// The classification mirrors the reclaim's own definition of "dead", so the
/// two cannot drift into disagreeing about which rows are takeable:
///
/// - no row       → the slot is free.
/// - orphan       → the workspace or agent row is gone; the reclaim frees it
///   and nothing is connected to it.
/// - revoked      → the owner's explicit "I'm done with this bot"; the
///   reclaim takes it (or, if it is this caller's own row, the
///   upsert reactivates it in place).
/// - active, ours → a re-install or secret rotation of a bot this
///   (workspace, agent) already holds. Not a conflict: the
///   upsert refreshes it in place and the reclaim spares it.
/// - active, somebody else's → refused. This is the case the whole gate
///   exists for.
async fn bot_slot_conflict_err<'e, E>(executor: E, p: &InstallationParams) -> anyhow::Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let owner = get_channel_installation_slot_owner_by_app_id(
        executor,
        crate::CHANNEL_TYPE_WECOM,
        &p.bot_id,
    )
    .await
    .map_err(|e| anyhow::anyhow!("wecom: read bot routing slot owner: {e}"))?;
    let Some(owner) = owner else {
        return Ok(()); // no row → the slot is free
    };
    let ours = owner.workspace_id == Some(p.workspace_id) && owner.agent_id == Some(p.agent_id);
    if !owner.workspace_exists || !owner.agent_exists {
        return Ok(()); // orphan
    }
    if owner.status == INSTALLATION_REVOKED {
        return Ok(());
    }
    if ours {
        return Ok(()); // re-install / rotation of our own row
    }
    if owner.workspace_id != Some(p.workspace_id) {
        return Err(anyhow::Error::new(BotOwnershipError::AnotherWorkspace));
    }
    if owner.agent_archived_at.is_some() {
        return Err(anyhow::Error::new(BotOwnershipError::ArchivedAgent));
    }
    Err(anyhow::Error::new(BotOwnershipError::SameWorkspace))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_requires_every_field() {
        let base = InstallationParams {
            workspace_id: Uuid::now_v7(),
            agent_id: Uuid::now_v7(),
            installer_user_id: Uuid::now_v7(),
            bot_id: "b".to_string(),
            secret: "s".to_string(),
            bot_display_name: String::new(),
        };
        assert!(validate_installation_params(&base).is_ok());

        let mut p = base.clone();
        p.workspace_id = Uuid::nil();
        assert_eq!(
            validate_installation_params(&p).unwrap_err().to_string(),
            "wecom: invalid installation parameters: workspace_id is required"
        );

        let mut p = base.clone();
        p.agent_id = Uuid::nil();
        assert!(validate_installation_params(&p).is_err());

        let mut p = base.clone();
        p.installer_user_id = Uuid::nil();
        assert!(validate_installation_params(&p).is_err());

        let mut p = base.clone();
        p.bot_id = String::new();
        assert!(validate_installation_params(&p).is_err());

        let mut p = base;
        p.secret = String::new();
        assert!(validate_installation_params(&p).is_err());
    }

    #[test]
    fn ownership_sentinels_roundtrip_through_anyhow() {
        let e = anyhow::Error::new(BotOwnershipError::ArchivedAgent);
        assert_eq!(
            as_bot_ownership_error(&e),
            Some(BotOwnershipError::ArchivedAgent)
        );
        assert_eq!(
            e.to_string(),
            "wecom: this bot is connected to an archived agent in this workspace"
        );
    }

    #[test]
    fn unique_violation_detection() {
        let e = anyhow::Error::new(sqlx::Error::ColumnNotFound("x".to_string()));
        assert!(!is_unique_violation(&e));
        // The positive path needs a genuine 23505, which requires a live DB
        // (the integration tests cover it); here we prove only that a non-DB
        // error walks the whole chain without a false positive.
    }
}
