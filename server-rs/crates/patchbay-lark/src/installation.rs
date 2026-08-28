//! InstallationService.
//!
//! Creates, refreshes and revokes per-agent Lark installations. It owns the
//! at-rest encryption of `app_secret` so that no caller (and no test fixture)
//! can accidentally insert a row with plaintext credentials — the only path
//! to writing the installation config goes through here.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use patchbay_util::secretbox::SecretBox;

use crate::channel_store::{is_no_rows, ChannelStore};
use crate::client::InstallationCredentials;
use crate::params::{
    GetInstallationInWorkspaceParams, SetInstallationStatusParams, UpsertInstallationParams,
};
use crate::store::{text_or_none, Installation};
use crate::types::{region_or_default, InstallationStatus};

/// The input shape RegistrationService assembles after a successful
/// device-flow scan-to-install. The credentials are supplied here as
/// plaintext — encryption happens inside [`InstallationService::upsert`] via
/// the supplied [`SecretBox`], so callers never see (and therefore cannot
/// leak) the ciphertext that lands in the DB.
#[derive(Debug, Clone)]
pub struct InstallationParams {
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub app_id: String,
    /// Plaintext; encrypted at the service boundary.
    pub app_secret: String,
    /// Optional, "" treated as NULL.
    pub tenant_key: String,
    pub bot_open_id: String,
    pub installer_user_id: Uuid,
    /// Which cloud (feishu/lark); empty defaults to feishu.
    pub region: crate::types::Region,
}

/// Surfaces "no row matches in this workspace" — used by the HTTP layer to
/// return 404. Distinct from the store's generic
/// [`crate::channel_store::ErrNoRows`] so handlers do not need to know about
/// the store internals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("lark installation not found")]
pub struct ErrInstallationNotFound;

pub struct InstallationService {
    queries: ChannelStore,
    box_: Arc<SecretBox>,
}

impl InstallationService {
    /// Binds the service to a store handle and a secretbox keyed for at-rest
    /// encryption. The box MUST be present; we refuse to fall back to
    /// plaintext storage even in test or dev configurations because that is
    /// exactly the regression the §4.4 requirement guards against.
    pub fn new(queries: ChannelStore, box_: Arc<SecretBox>) -> Self {
        Self { queries, box_ }
    }

    /// The secretbox, exposed so RegistrationService can seal OUTSIDE its DB
    /// transaction (Go reached the unexported field because both types shared
    /// package lark; Rust uses an explicit accessor instead).
    pub fn seal_app_secret(&self, plaintext: &str) -> anyhow::Result<Vec<u8>> {
        self.box_
            .seal(plaintext.as_bytes())
            .map_err(|e| anyhow::anyhow!("encrypt app_secret: {e}"))
    }

    /// Creates a new installation or refreshes an existing one in place
    /// (matching on the (workspace_id, agent_id) UNIQUE). Re-install resets
    /// status to 'active' but does NOT touch the WS lease — that is the hub's
    /// concern, not ours. The returned row is the post-write state; the
    /// encrypted secret column is included for completeness but callers
    /// SHOULD NOT log or persist it elsewhere.
    pub async fn upsert(&self, p: InstallationParams) -> anyhow::Result<Installation> {
        validate_installation_params(&p)?;
        let sealed = self.seal_app_secret(&p.app_secret)?;
        self.queries
            .upsert_lark_installation(UpsertInstallationParams {
                workspace_id: p.workspace_id,
                agent_id: p.agent_id,
                app_id: p.app_id,
                app_secret_encrypted: sealed,
                bot_open_id: p.bot_open_id,
                installer_user_id: p.installer_user_id,
                tenant_key: text_or_none(&p.tenant_key),
                bot_union_id: None,
                region: region_or_default(p.region.as_str()).as_str().to_string(),
            })
            .await
    }

    /// Flips status to 'revoked' so the WS hub tears the connection down on
    /// its next sweep and the dispatcher drops any in-flight events. The row
    /// is preserved (no DELETE) so audit history remains queryable; a
    /// subsequent re-install via [`Self::upsert`] flips status back to
    /// 'active' atomically.
    pub async fn revoke(&self, id: Uuid) -> anyhow::Result<()> {
        self.queries
            .set_lark_installation_status(SetInstallationStatusParams {
                id,
                status: InstallationStatus::REVOKED.to_string(),
            })
            .await
    }

    /// Returns the plaintext app_secret for the supplied installation row.
    /// Used by the WebSocket hub when it needs to authenticate against the
    /// Lark API on behalf of an installation; do NOT use this for read-only
    /// display surfaces. The plaintext value must never round-trip through an
    /// HTTP response.
    pub fn decrypt_app_secret(&self, inst: &Installation) -> anyhow::Result<String> {
        let plain = self
            .box_
            .open(&inst.app_secret_encrypted)
            .map_err(|e| anyhow::anyhow!("decrypt app_secret: {e}"))?;
        String::from_utf8(plain).map_err(|e| anyhow::anyhow!("decrypt app_secret: not utf-8: {e}"))
    }

    /// The workspace-scoped lookup helper. Internal callers (Dispatcher) use
    /// get-by-app-id directly because the event payload only carries app_id;
    /// HTTP-side callers always know the workspace and should use this so a
    /// forged installation_id from a different workspace returns NotFound
    /// instead of leaking existence.
    pub async fn get_in_workspace(
        &self,
        id: Uuid,
        workspace_id: Uuid,
    ) -> anyhow::Result<Installation> {
        match self
            .queries
            .get_lark_installation_in_workspace(GetInstallationInWorkspaceParams {
                id,
                workspace_id,
            })
            .await
        {
            Ok(inst) => Ok(inst),
            Err(err) if is_no_rows(&err) => Err(ErrInstallationNotFound.into()),
            Err(err) => Err(err),
        }
    }

    /// Returns every installation rooted at the workspace, active and
    /// revoked, oldest first. The status column lets the UI distinguish
    /// "wired up" from "torn down but kept for audit".
    pub async fn list_by_workspace(&self, workspace_id: Uuid) -> anyhow::Result<Vec<Installation>> {
        self.queries
            .list_lark_installations_by_workspace(workspace_id)
            .await
    }
}

fn validate_installation_params(p: &InstallationParams) -> anyhow::Result<()> {
    if p.workspace_id.is_nil() {
        anyhow::bail!("workspace_id is required");
    }
    if p.agent_id.is_nil() {
        anyhow::bail!("agent_id is required");
    }
    if p.installer_user_id.is_nil() {
        anyhow::bail!("installer_user_id is required");
    }
    if p.app_id.is_empty() {
        anyhow::bail!("app_id is required");
    }
    if p.app_secret.is_empty() {
        anyhow::bail!("app_secret is required");
    }
    if p.bot_open_id.is_empty() {
        anyhow::bail!("bot_open_id is required");
    }
    Ok(())
}

/// Decrypts an installation's app_secret for the transport layer.
/// `InstallationService` satisfies it directly; tests substitute a fake.
pub trait CredentialsResolver: Send + Sync {
    fn decrypt_app_secret(&self, inst: &Installation) -> anyhow::Result<String>;
}

impl CredentialsResolver for InstallationService {
    fn decrypt_app_secret(&self, inst: &Installation) -> anyhow::Result<String> {
        InstallationService::decrypt_app_secret(self, inst)
    }
}

/// Builds the per-installation transport credentials from an installation row
/// + decrypted secret. Shared by the Patcher and the OutcomeReplier.
pub fn installation_credentials_for(
    creds_resolver: &dyn CredentialsResolver,
    inst: &Installation,
) -> anyhow::Result<InstallationCredentials> {
    let secret = creds_resolver
        .decrypt_app_secret(inst)
        .map_err(|e| anyhow::anyhow!("decrypt app_secret: {e:#}"))?;
    let mut creds = InstallationCredentials {
        app_id: inst.app_id.clone(),
        app_secret: secret,
        tenant_key: String::new(),
        region: region_or_default(&inst.region),
    };
    if let Some(tenant_key) = &inst.tenant_key {
        creds.tenant_key = tenant_key.clone();
    }
    Ok(creds)
}

#[async_trait]
impl crate::ws_connector::CredentialsProvider for InstallationService {
    /// Supplies the plaintext credentials a connector needs for its endpoint
    /// bootstrap; mirrors Go hub.go's credentials provider wiring.
    async fn credentials(&self, inst: &Installation) -> anyhow::Result<InstallationCredentials> {
        installation_credentials_for(self, inst)
    }
}
