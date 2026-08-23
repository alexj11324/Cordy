//! The CredentialsResolver default implementation that unseals an
//! Installation's encrypted secret for the WebSocket subscribe frame — port
//! of `credentials.go`.
//!
//! The wecom package owns encryption via a secretbox supplied at boot; the
//! Installation itself never holds plaintext, so every caller that needs a
//! real secret comes through here.

use async_trait::async_trait;

use cordy_util::secretbox::SecretBox;

use crate::types::{Installation, InstallationCredentials};

/// Mints per-call [`InstallationCredentials`] with plaintext secrets by
/// unsealing what the Installation carries. Boot injects a concrete
/// implementation; unit tests supply a fake.
#[async_trait]
pub trait CredentialsResolver: Send + Sync {
    async fn credentials(&self, inst: &Installation) -> anyhow::Result<InstallationCredentials>;
}

/// Decrypts the smart-bot secret using a single secretbox shared across every
/// wecom installation. Rotation is the same story as Feishu / Slack: change
/// CORDY_WECOM_SECRET_KEY, and every existing row needs a re-encrypt
/// migration.
///
/// Port note: Go's constructor rejects a nil box so the wire-up cannot fall
/// back to plaintext; Rust's `SecretBox` is not nullable, so the invariant is
/// structural and the constructor is infallible.
#[derive(Clone)]
pub struct SecretboxCredentialsResolver {
    box_: SecretBox,
}

impl SecretboxCredentialsResolver {
    pub fn new(box_: SecretBox) -> Self {
        Self { box_ }
    }
}

#[async_trait]
impl CredentialsResolver for SecretboxCredentialsResolver {
    /// Returns the plaintext-bearing credentials for an installation. The
    /// returned value is owned, safe to hand around without aliasing the
    /// encrypted blob.
    async fn credentials(&self, inst: &Installation) -> anyhow::Result<InstallationCredentials> {
        let secret = self
            .box_
            .open(&inst.secret_encrypted)
            .map_err(|e| anyhow::anyhow!("wecom: decrypt secret: {e}"))?;
        Ok(InstallationCredentials {
            bot_id: inst.bot_id.clone(),
            secret: String::from_utf8_lossy(&secret).into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver_with_key(key: &[u8]) -> SecretboxCredentialsResolver {
        SecretboxCredentialsResolver::new(SecretBox::new(key).unwrap())
    }

    #[tokio::test]
    async fn unseals_the_stored_secret() {
        let r = resolver_with_key(&[7u8; 32]);
        let sealed = r.box_.seal(b"plaintext-secret").unwrap();
        let inst = Installation {
            bot_id: "bot-1".to_string(),
            secret_encrypted: sealed,
            ..Default::default()
        };
        let creds = r.credentials(&inst).await.unwrap();
        assert_eq!(creds.bot_id, "bot-1");
        assert_eq!(creds.secret, "plaintext-secret");
    }

    #[tokio::test]
    async fn wrong_key_is_an_error_not_garbage() {
        let r = resolver_with_key(&[7u8; 32]);
        let other = resolver_with_key(&[8u8; 32]);
        let sealed = r.box_.seal(b"plaintext-secret").unwrap();
        let inst = Installation {
            bot_id: "bot-1".to_string(),
            secret_encrypted: sealed,
            ..Default::default()
        };
        assert!(other.credentials(&inst).await.is_err());
    }
}
