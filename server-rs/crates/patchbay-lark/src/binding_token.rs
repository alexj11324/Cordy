//! Binding tokens.
//!
//! BindingTokenService mints and redeems binding tokens for the "you're not
//! bound yet, click here" flow. The TTL is fixed at
//! [`BINDING_TOKEN_TTL`](crate::types::BINDING_TOKEN_TTL) (15 min); the DB
//! CHECK enforces the same cap so a misconfigured caller cannot quietly mint
//! a longer-lived token.
//!
//! Redemption ([`BindingTokenService::redeem_and_bind`]) is transactional:
//! consuming the token and inserting the binding row commit together, so a
//! failed bind never burns a token, and a successful bind never leaves a
//! consumed-but-unused token behind.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::channel_store::{create_lark_user_binding_with, is_no_rows, ChannelStore, ErrNoRows};
use crate::params::CreateUserBindingParams;
use crate::store::{BindingTokenRow, UserBinding};
use crate::types::{OpenId, BINDING_TOKEN_TTL};

/// The public shape of a freshly minted token. The raw token is returned to
/// the caller exactly once — it is the unguessable secret embedded in the
/// binding URL the Bot replies with. After this call returns, only the hash
/// exists server-side; the raw value cannot be recovered from the DB.
#[derive(Debug, Clone)]
pub struct BindingToken {
    pub raw: String,
    pub expires_at: DateTime<Utc>,
}

/// The row returned to the caller after a successful redemption. The
/// redemption path uses these fields to write the binding row.
#[derive(Debug, Clone)]
pub struct RedeemedBindingToken {
    pub workspace_id: Uuid,
    pub installation_id: Uuid,
    pub lark_open_id: OpenId,
}

/// Carries the inputs the installer auto-bind needs. Kept as a struct so
/// adding union_id (Phase 2) does not break callers.
#[derive(Debug, Clone)]
pub struct InstallerBindParams {
    pub workspace_id: Uuid,
    pub installation_id: Uuid,
    /// The installer's Patchbay account.
    pub patchbay_user_id: Uuid,
    /// The installer's per-installation open_id.
    pub lark_open_id: OpenId,
}

/// The narrow surface RegistrationService needs to record the installer's
/// binding row in the same business step as the installation insert. Without
/// this step the first inbound message from the installer would be dropped as
/// `unbound_user` and the Bot would reply "you're not bound, click here…" to
/// the person who just authorized the install seconds ago.
///
/// Implementations MUST be idempotent on (installation_id, lark_open_id): a
/// re-install by the same user should not error.
///
/// `executor` is the transaction-scoped handle (a `&mut PgConnection`) to run
/// the bind against. The caller opens the transaction so the installation
/// insert and the binding write commit together.
#[async_trait]
pub trait InstallerBinder: Send + Sync {
    async fn bind_installer_tx(
        &self,
        executor: &mut sqlx::PgConnection,
        p: InstallerBindParams,
    ) -> anyhow::Result<()>;
}

/// The narrow dependency the outcome replier needs from BindingTokenService.
/// Keeping this as a trait lets tests pin the Lark binding URL without
/// constructing a database-backed token service.
#[async_trait]
pub trait BindingTokenMinter: Send + Sync {
    async fn mint(
        &self,
        workspace_id: Uuid,
        installation_id: Uuid,
        open_id: &OpenId,
    ) -> anyhow::Result<BindingToken>;
}

/// Returned when the token hash does not exist, the token has already been
/// consumed, or it has expired. The caller must NOT distinguish those
/// sub-cases — that distinction enables timing oracles for token replay races
/// and adds no product value (the user sees the same "link invalid or
/// expired, please request a new one" copy either way).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("binding token invalid or expired")]
pub struct ErrBindingTokenInvalid;

/// Returned when a binding row already exists for the (installation, open_id)
/// pair and points at a different Patchbay user. Account transfer must go
/// through an explicit unbind flow; a binding token cannot be used to grab an
/// already-bound open_id from another user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("lark open_id is already bound to a different user")]
pub struct ErrBindingAlreadyAssigned;

/// Returned when the user is not (or no longer) a member of the target
/// workspace, detected by an explicit membership check (PB-3515 §4 removed
/// the member FK that used to enforce this). Translated to 403 at the HTTP
/// boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("redeemer is not a workspace member")]
pub struct ErrBindingNotWorkspaceMember;

pub struct BindingTokenService {
    queries: ChannelStore,
}

impl BindingTokenService {
    /// Constructs the default service over the shared store handle.
    pub fn new(queries: ChannelStore) -> Self {
        Self { queries }
    }

    /// Creates a new single-use binding token and returns the raw secret +
    /// expiry. The raw value MUST be sent over a secure channel to the
    /// intended recipient — Lark DMs are encrypted in transit by the platform
    /// — and never logged. Mint is the only function in this module that
    /// produces a raw token; subsequent reads are by hash.
    pub async fn mint(
        &self,
        workspace_id: Uuid,
        installation_id: Uuid,
        open_id: &OpenId,
    ) -> anyhow::Result<BindingToken> {
        let raw = random_token(32)?;
        let hash = hash_token(&raw);
        let expires_at = Utc::now() + BINDING_TOKEN_TTL;

        let row: BindingTokenRow = self
            .queries
            .create_lark_binding_token(crate::params::CreateBindingTokenParams {
                token_hash: hash,
                workspace_id,
                installation_id,
                channel_user_id: open_id.0.clone(),
                expires_at,
            })
            .await
            .map_err(|e| anyhow::anyhow!("persist token: {e:#}"))?;
        Ok(BindingToken {
            raw,
            expires_at: row.expires_at,
        })
    }

    /// Atomically consumes a raw token and writes the binding row in a single
    /// DB transaction. The redeemer's identity is the supplied patchbay_user_id
    /// (taken from the session by the handler, never from the token), so a
    /// stolen token cannot bind a Lark open_id to an attacker's account.
    ///
    /// Failure modes are returned as typed errors:
    ///
    /// - [`ErrBindingTokenInvalid`]: token doesn't exist / already consumed /
    ///   expired. Same opaque error for all three to avoid a timing oracle
    ///   for replay races.
    /// - [`ErrBindingAlreadyAssigned`]: a binding already exists for this
    ///   (installation, open_id), pointing at a DIFFERENT Patchbay user. The
    ///   token is NOT consumed in this case — we roll back so the correct
    ///   holder of the existing binding is not disrupted and ops can still
    ///   revoke the surplus token explicitly. Account transfer must go
    ///   through an explicit unbind, not a redemption.
    /// - [`ErrBindingNotWorkspaceMember`]: the redeemer is not a member of
    ///   the token's workspace. Rolled back identically.
    ///
    /// On the happy path the consume + bind commit together: a successful
    /// return guarantees both the consumed_at write and the binding row
    /// landed; a returned error guarantees neither did.
    pub async fn redeem_and_bind(
        &self,
        raw: &str,
        patchbay_user_id: Uuid,
    ) -> anyhow::Result<RedeemedBindingToken> {
        let mut tx = self
            .queries
            .pool()
            .begin()
            .await
            .map_err(|e| anyhow::anyhow!("begin tx: {e:#}"))?;

        let row =
            match crate::channel_store::consume_lark_binding_token_with(&mut *tx, &hash_token(raw))
                .await
            {
                Ok(row) => row,
                Err(err) if is_no_rows(&err) => return Err(ErrBindingTokenInvalid.into()),
                Err(err) => return Err(anyhow::anyhow!("consume token: {err:#}")),
            };

        // Explicit membership gate. The binding → member FK that used to
        // reject a non-member redeemer is gone (PB-3515 §4), so we check it
        // here. Returning before Commit rolls the consume back, so a
        // non-member's attempt does not burn the token — same outcome the FK
        // violation produced.
        let is_member =
            is_workspace_member_on(&mut *tx, row.workspace_id, patchbay_user_id).await?;
        if !is_member {
            return Err(ErrBindingNotWorkspaceMember.into());
        }

        match create_lark_user_binding_with(
            &mut *tx,
            CreateUserBindingParams {
                workspace_id: row.workspace_id,
                patchbay_user_id,
                installation_id: row.installation_id,
                channel_user_id: row.channel_user_id.clone(),
                union_id: None,
            },
        )
        .await
        {
            Ok(_) => {}
            // No rows here means the conflict row exists but its
            // patchbay_user_id differs from ours, so the WHERE clause on the ON
            // CONFLICT DO UPDATE rejected the rebind. See the comment on
            // CreateChannelUserBinding in queries/channel.sql.
            Err(err) if is_no_rows(&err) => return Err(ErrBindingAlreadyAssigned.into()),
            Err(err) => return Err(anyhow::anyhow!("create binding: {err:#}")),
        }

        tx.commit()
            .await
            .map_err(|e| anyhow::anyhow!("commit: {e:#}"))?;
        Ok(RedeemedBindingToken {
            workspace_id: row.workspace_id,
            installation_id: row.installation_id,
            lark_open_id: OpenId(row.channel_user_id),
        })
    }
}

#[async_trait]
impl BindingTokenMinter for BindingTokenService {
    async fn mint(
        &self,
        workspace_id: Uuid,
        installation_id: Uuid,
        open_id: &OpenId,
    ) -> anyhow::Result<BindingToken> {
        BindingTokenService::mint(self, workspace_id, installation_id, open_id).await
    }
}

#[async_trait]
impl InstallerBinder for BindingTokenService {
    /// The auto-binding path for the device-flow install: the user who just
    /// authorized the install is recorded as bound to their own open_id, so
    /// the first inbound message in the bot's DM arrives at a `bound` identity
    /// check and the user is NOT prompted with a redundant "click here to
    /// bind" card.
    ///
    /// `executor` is the RegistrationService's transaction-scoped connection.
    /// The service opens a transaction that wraps the installation insert and
    /// this binding write so a half-applied install (installation row without
    /// the installer binding) cannot land.
    ///
    /// Token redemption deliberately does NOT share this code path:
    /// - redeem_and_bind consumes a server-minted token in the same tx as the
    ///   binding insert; that's how anti-replay works.
    /// - bind_installer_tx is invoked from the device-flow success hook where
    ///   the authoritative proof of identity is the Lark-validated polling
    ///   response (open_id returned alongside the freshly minted client_id /
    ///   client_secret). There is no token to consume, and inventing one
    ///   would only widen the attack surface.
    ///
    /// The underlying CreateLarkUserBinding query is idempotent on
    /// (installation_id, open_id) when patchbay_user_id matches (the ON CONFLICT
    /// DO UPDATE gating spelled out on the SQL), so a re-install by the same
    /// user is a no-op metadata refresh. A re-install by a DIFFERENT user
    /// surfaces as [`ErrBindingAlreadyAssigned`] — the registration caller
    /// treats that as a hard error and the frontend surfaces it as "this Lark
    /// account is bound elsewhere", preventing one workspace admin from
    /// silently rebinding another's PersonalAgent install.
    async fn bind_installer_tx(
        &self,
        executor: &mut sqlx::PgConnection,
        p: InstallerBindParams,
    ) -> anyhow::Result<()> {
        // Explicit membership gate, replacing the removed member FK
        // (PB-3515 §4): the installer must be a member of the workspace they
        // are binding into.
        let is_member = is_workspace_member_on(&mut *executor, p.workspace_id, p.patchbay_user_id)
            .await
            .map_err(|e| anyhow::anyhow!("check membership: {e:#}"))?;
        if !is_member {
            return Err(ErrBindingNotWorkspaceMember.into());
        }
        match create_lark_user_binding_with(
            executor,
            CreateUserBindingParams {
                workspace_id: p.workspace_id,
                patchbay_user_id: p.patchbay_user_id,
                installation_id: p.installation_id,
                channel_user_id: p.lark_open_id.0.clone(),
                union_id: None,
            },
        )
        .await
        {
            Ok(_) => Ok(()),
            Err(err) if is_no_rows(&err) => Err(ErrBindingAlreadyAssigned.into()),
            Err(err) => Err(anyhow::anyhow!("bind installer: {err:#}")),
        }
    }
}

/// Executor-generic membership check mirroring
/// ChannelStore::is_workspace_member for use inside transactions.
async fn is_workspace_member_on(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> anyhow::Result<bool> {
    Ok(
        patchbay_db::queries::member::get_member_by_user_and_workspace(
            executor,
            user_id,
            workspace_id,
        )
        .await?
        .is_some(),
    )
}

/// URL-safe so the token embeds cleanly in the binding URL without escaping.
/// RawURLEncoding drops `=` padding which is optional for decoders and would
/// otherwise look ugly in user-visible URLs.
fn random_token(n: usize) -> anyhow::Result<String> {
    use rand::RngCore;
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

fn hash_token(raw: &str) -> String {
    let sum = Sha256::digest(raw.as_bytes());
    hex::encode(sum)
}

/// Re-exported for downstream wiring parity with the Go package's exported
/// constructor surface.
pub fn new_binding_token_service(queries: ChannelStore) -> Arc<BindingTokenService> {
    Arc::new(BindingTokenService::new(queries))
}

#[allow(unused)]
fn _marker(_: ErrNoRows, _: UserBinding) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_token_is_sha256_hex() {
        // Pinned against sha256("abc") = ba7816bf8f01cfea414140de5dae2223
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn random_tokens_are_url_safe_and_sized() {
        let t = random_token(32).unwrap();
        assert_eq!(t.len(), 43); // 32 bytes → 43 base64url chars, no padding
        assert!(!t.contains('+'));
        assert!(!t.contains('/'));
        assert!(!t.contains('='));
        assert_ne!(random_token(32).unwrap(), t);
    }

    #[test]
    fn typed_errors_render_go_messages() {
        assert_eq!(
            ErrBindingTokenInvalid.to_string(),
            "binding token invalid or expired"
        );
        assert_eq!(
            ErrBindingAlreadyAssigned.to_string(),
            "lark open_id is already bound to a different user"
        );
        assert_eq!(
            ErrBindingNotWorkspaceMember.to_string(),
            "redeemer is not a workspace member"
        );
    }
}
