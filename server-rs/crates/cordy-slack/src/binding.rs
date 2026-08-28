//! The Slack user-binding token flow: an unbound Slack user who messages the
//! bot gets a "link your account" prompt (minted here, delivered by the
//! OutboundReplier), clicks through to the in-product redeem page, and their
//! Slack user id is bound to their Cordy account. Port of
//!
//! It mirrors lark.BindingTokenService but runs on the generic channel_*
//! queries with channel_type='slack' (lark's ChannelStore hardcodes 'feishu').

use base64::Engine as _;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use cordy_db::queries::channel::{
    consume_channel_binding_token, create_channel_binding_token, create_channel_user_binding,
};
use cordy_db::queries::member::get_member_by_user_and_workspace;

use crate::TYPE_SLACK;

/// Bounds a token's life. The channel_binding_token CHECK enforces the same
/// 15-minute cap so a misconfigured caller cannot mint longer.
pub const BINDING_TOKEN_TTL: chrono::Duration = chrono::Duration::minutes(15);

/// Sentinel errors the redeem path returns.
///
/// Port note: Go sentinels become typed variants; messages mirror the Go
/// strings.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindingError {
    /// Token unknown / already consumed / expired. One opaque error for all
    /// three avoids a replay timing oracle.
    #[error("slack: binding token invalid or expired")]
    TokenInvalid,
    /// This Slack user id is already bound to a different Cordy user (account
    /// transfer must go through explicit unbind).
    #[error("slack: user id is already bound to a different user")]
    AlreadyAssigned,
    /// The redeemer is not a member of the token's workspace. Translated to
    /// 403 at the HTTP boundary.
    #[error("slack: redeemer is not a workspace member")]
    NotWorkspaceMember,
}

/// A freshly minted token. The raw value is returned exactly once (embedded
/// in the binding URL); only its hash is persisted.
#[derive(Debug, Clone)]
pub struct BindingToken {
    pub raw: String,
    pub expires_at: DateTime<Utc>,
}

/// Returned after a successful redemption.
#[derive(Debug, Clone)]
pub struct RedeemedBindingToken {
    pub workspace_id: Uuid,
    pub installation_id: Uuid,
    pub slack_user_id: String,
}

/// Mints and redeems Slack binding tokens. Redemption is transactional:
/// consuming the token and inserting the channel_user_binding row commit
/// together, so a failed bind never burns a token.
pub struct BindingTokenService {
    pool: PgPool,
}

impl BindingTokenService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Creates a single-use binding token for (installation, slack_user_id)
    /// and returns the raw secret + expiry. The raw value must be delivered
    /// over Slack (encrypted in transit by the platform) and never logged.
    pub async fn mint(
        &self,
        workspace_id: Uuid,
        installation_id: Uuid,
        slack_user_id: &str,
    ) -> anyhow::Result<BindingToken> {
        let raw = random_binding_token()?;
        let expires_at = Utc::now() + BINDING_TOKEN_TTL;
        create_channel_binding_token(
            &self.pool,
            &hash_binding_token(&raw),
            workspace_id,
            installation_id,
            TYPE_SLACK,
            slack_user_id,
            Some(expires_at),
        )
        .await
        .map_err(|e| anyhow::anyhow!("persist token: {e:#}"))?;
        Ok(BindingToken { raw, expires_at })
    }

    /// Atomically consumes a raw token and binds the Slack user id to
    /// cordy_user_id (taken from the session, never from the token). Returns
    /// TokenInvalid / AlreadyAssigned / NotWorkspaceMember for the product
    /// cases.
    pub async fn redeem_and_bind(
        &self,
        raw: &str,
        cordy_user_id: Uuid,
    ) -> Result<RedeemedBindingToken, anyhow::Error> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| anyhow::anyhow!("begin tx: {e:#}"))?;

        // ErrNoRows on the consume means unknown / consumed / expired — all
        // one opaque invalid error.
        let row = match consume_channel_binding_token(&mut *tx, &hash_binding_token(raw)).await {
            Ok(Some(row)) => row,
            Ok(None) => return Err(BindingError::TokenInvalid.into()),
            Err(e) => return Err(anyhow::anyhow!("consume token: {e:#}")),
        };

        // Explicit membership gate (no member FK): returning before Commit
        // rolls the consume back, so a non-member's attempt does not burn the
        // token.
        match get_member_by_user_and_workspace(&mut *tx, cordy_user_id, row.workspace_id).await {
            Ok(Some(_)) => {}
            Ok(None) => return Err(BindingError::NotWorkspaceMember.into()),
            Err(e) => return Err(anyhow::anyhow!("check membership: {e:#}")),
        }

        // The ON CONFLICT DO UPDATE WHERE cordy_user_id = … gating rejects a
        // user id already bound elsewhere with no row; that maps to
        // AlreadyAssigned.
        let created = create_channel_user_binding(
            &mut *tx,
            row.workspace_id,
            cordy_user_id,
            row.installation_id,
            TYPE_SLACK,
            &row.channel_user_id,
            &serde_json::json!({}),
        )
        .await;
        if created.is_err() {
            return Err(BindingError::AlreadyAssigned.into());
        }

        tx.commit()
            .await
            .map_err(|e| anyhow::anyhow!("commit: {e:#}"))?;
        Ok(RedeemedBindingToken {
            workspace_id: row.workspace_id,
            installation_id: row.installation_id,
            slack_user_id: row.channel_user_id,
        })
    }
}

fn random_binding_token() -> anyhow::Result<String> {
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

fn hash_binding_token(raw: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_hash_is_sha256_hex_of_raw() {
        // Matches Go hex.EncodeToString(sha256.Sum256(raw)).
        let h = hash_binding_token("");
        assert_eq!(
            h,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(hash_binding_token("abc").len(), 64);
    }

    #[test]
    fn random_tokens_are_url_safe_base64_without_padding() {
        let a = random_binding_token().unwrap();
        let b = random_binding_token().unwrap();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43); // 32 bytes -> 43 unpadded base64url chars
        assert!(!a.contains('+') && !a.contains('/') && !a.contains('='));
    }

    #[test]
    fn ttl_is_fifteen_minutes_like_the_check_constraint() {
        assert_eq!(BINDING_TOKEN_TTL.num_minutes(), 15);
    }
}
