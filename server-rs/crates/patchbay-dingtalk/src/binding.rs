//! Port of `binding.go`: the DingTalk user-binding token flow. An unbound
//! DingTalk user who messages the bot gets a "link your account" prompt (minted
//! here, delivered by the OutboundReplier), clicks through to the in-product
//! redeem page, and their DingTalk staff id is bound to their Patchbay account. It
//! mirrors slack.BindingTokenService but runs on the generic channel_* queries
//! with channel_type='dingtalk'.

use base64::Engine as _;
use rand::RngCore;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use patchbay_db::queries::channel::{
    consume_channel_binding_token, create_channel_binding_token, create_channel_user_binding,
};
use patchbay_db::queries::member::get_member_by_user_and_workspace;

use crate::TYPE_DINGTALK;

/// Bounds a token's life. The channel_binding_token CHECK enforces the same
/// 15-minute cap so a misconfigured caller cannot mint longer.
pub const BINDING_TOKEN_TTL_SECS: i64 = 15 * 60;

/// Sentinel errors of the binding flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BindingError {
    /// Token unknown / already consumed / expired. One opaque error for all
    /// three avoids a replay timing oracle.
    #[error("dingtalk: binding token invalid or expired")]
    TokenInvalid,
    /// This DingTalk user id is already bound to a different Patchbay user
    /// (account transfer must go through explicit unbind).
    #[error("dingtalk: user id is already bound to a different user")]
    AlreadyAssigned,
    /// The redeemer is not a member of the token's workspace. Translated to
    /// 403 at the HTTP boundary.
    #[error("dingtalk: redeemer is not a workspace member")]
    NotWorkspaceMember,
}

/// A freshly minted token. The raw value is returned exactly once (embedded in
/// the binding URL); only its hash is persisted.
#[derive(Debug, Clone)]
pub struct BindingToken {
    pub raw: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// Returned after a successful redemption.
#[derive(Debug, Clone)]
pub struct RedeemedBindingToken {
    pub workspace_id: Uuid,
    pub installation_id: Uuid,
    pub dingtalk_user_id: String,
}

/// Mints and redeems DingTalk binding tokens. Redemption is transactional:
/// consuming the token and inserting the channel_user_binding row commit
/// together, so a failed bind never burns a token.
#[derive(Clone)]
pub struct BindingTokenService {
    pool: sqlx::PgPool,
}

impl BindingTokenService {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    /// Creates a single-use binding token for (installation, dingtalk_user_id)
    /// and returns the raw secret + expiry. The raw value must be delivered
    /// over DingTalk (encrypted in transit by the platform) and never logged.
    pub async fn mint(
        &self,
        workspace_id: Uuid,
        installation_id: Uuid,
        dingtalk_user_id: &str,
    ) -> anyhow::Result<BindingToken> {
        let raw = random_binding_token(32).map_err(|e| anyhow::anyhow!("generate token: {e}"))?;
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(BINDING_TOKEN_TTL_SECS);
        create_channel_binding_token(
            &self.pool,
            &hash_binding_token(&raw),
            workspace_id,
            installation_id,
            TYPE_DINGTALK,
            dingtalk_user_id,
            Some(expires_at),
        )
        .await
        .map_err(|e| anyhow::anyhow!("persist token: {e:#}"))?;
        Ok(BindingToken { raw, expires_at })
    }

    /// Atomically consumes a raw token and binds the DingTalk user id to
    /// patchbay_user_id (taken from the session, never from the token). Returns
    /// [`BindingError::TokenInvalid`] /
    /// [`BindingError::AlreadyAssigned`] /
    /// [`BindingError::NotWorkspaceMember`].
    pub async fn redeem_and_bind(
        &self,
        raw: &str,
        patchbay_user_id: Uuid,
    ) -> anyhow::Result<RedeemedBindingToken> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| anyhow::anyhow!("begin tx: {e:#}"))?;

        let row = consume_channel_binding_token(&mut *tx, &hash_binding_token(raw))
            .await
            .map_err(|e| anyhow::anyhow!("consume token: {e:#}"))?;
        let Some(row) = row else {
            return Err(BindingError::TokenInvalid.into());
        };
        if row.channel_type != TYPE_DINGTALK {
            // Consume and bind share this transaction, so returning here rolls
            // the consume back. A token from another adapter must never create
            // a DingTalk binding against that adapter's installation.
            return Err(BindingError::TokenInvalid.into());
        }

        // Explicit membership gate (no member FK): returning before Commit
        // rolls the consume back, so a non-member's attempt does not burn the
        // token.
        let member = get_member_by_user_and_workspace(&mut *tx, patchbay_user_id, row.workspace_id)
            .await
            .map_err(|e| anyhow::anyhow!("check membership: {e:#}"))?;
        if member.is_none() {
            return Err(BindingError::NotWorkspaceMember.into());
        }

        let created = create_channel_user_binding(
            &mut *tx,
            row.workspace_id,
            patchbay_user_id,
            row.installation_id,
            TYPE_DINGTALK,
            &row.channel_user_id,
            &serde_json::json!({}),
        )
        .await
        .map_err(|e| anyhow::anyhow!("create binding: {e:#}"))?;
        // None means the existing binding points at a different user — the ON
        // CONFLICT DO UPDATE WHERE patchbay_user_id=… gating rejected it.
        if created.is_none() {
            return Err(BindingError::AlreadyAssigned.into());
        }

        tx.commit()
            .await
            .map_err(|e| anyhow::anyhow!("commit: {e:#}"))?;
        Ok(RedeemedBindingToken {
            workspace_id: row.workspace_id,
            installation_id: row.installation_id,
            dingtalk_user_id: row.channel_user_id,
        })
    }
}

#[async_trait::async_trait]
impl crate::replier::BindingMinter for BindingTokenService {
    async fn mint(
        &self,
        workspace_id: Uuid,
        installation_id: Uuid,
        dingtalk_user_id: &str,
    ) -> anyhow::Result<BindingToken> {
        BindingTokenService::mint(self, workspace_id, installation_id, dingtalk_user_id).await
    }
}

fn random_binding_token(n: usize) -> Result<String, rand::Error> {
    let mut buf = vec![0u8; n];
    rand::thread_rng().try_fill_bytes(&mut buf)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

fn hash_binding_token(raw: &str) -> String {
    hex::encode(Sha256::digest(raw.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_and_url_safe() {
        let a = random_binding_token(32).unwrap();
        let b = random_binding_token(32).unwrap();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43); // 32 bytes → 43 unpadded base64url chars
        assert!(a
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn hash_is_sha256_hex() {
        assert_eq!(
            hash_binding_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
