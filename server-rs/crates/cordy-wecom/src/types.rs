//! The decoded, in-memory view of a WeCom smart-bot `channel_installation`
//! row — port of `types.go`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `channel_installation.status` value for a live installation.
pub const INSTALLATION_ACTIVE: &str = "active";
/// `channel_installation.status` value after an explicit disconnect. The row
/// is preserved so audit trails remain queryable; a subsequent upsert flips
/// it back to [`INSTALLATION_ACTIVE`] atomically.
pub const INSTALLATION_REVOKED: &str = "revoked";

/// The decoded, in-memory view of a WeCom smart-bot channel_installation
/// row. `secret_encrypted` is a secretbox-sealed blob and never plaintext;
/// callers who need the plaintext go through the
/// [`crate::credentials::CredentialsResolver`].
///
/// Port note: Go holds `pgtype.UUID` (nullable); Rust uses `Uuid`, with the
/// nil UUID standing in for Go's invalid/zero UUID.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Installation {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub installer_user_id: Uuid,
    /// One of [`INSTALLATION_ACTIVE`] / [`INSTALLATION_REVOKED`].
    pub status: String,

    /// The smart-bot identifier the WeCom admin console assigns at bot
    /// creation. It is BOTH the auth identity presented in the
    /// aibot_subscribe frame AND the routing key we persist as
    /// `config->>'app_id'` so GetChannelInstallationByAppID resolves an
    /// inbound event to its installation.
    pub bot_id: String,

    /// The sealed long-connection secret. This is distinct from the
    /// token/EncodingAESKey used by callback-mode bots (which we do not
    /// use). Rotated via re-install.
    pub secret_encrypted: Vec<u8>,

    /// What the bot is called in a chat. A WeCom group mention arrives as
    /// literal text — "@Cordy Bot /new 重新分析" — with no structured
    /// mention list anywhere in the payload, so recognising where the
    /// mention ends is the only way a name containing a space does not
    /// swallow the command after it.
    ///
    /// Optional. Empty falls back to the whitespace heuristic in
    /// `strip_leading_mentions`, which is correct for a single-word name and
    /// is what every existing installation gets.
    pub bot_display_name: String,
}

impl Installation {
    /// Reports whether this installation is live (`status == "active"`).
    pub fn is_active(&self) -> bool {
        self.status == INSTALLATION_ACTIVE
    }
}

/// The plaintext-bearing view the WebSocket subscribe frame needs. It is
/// minted per-connect by the credentials resolver so a plaintext secret
/// never lives on the durable [`Installation`] itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InstallationCredentials {
    pub bot_id: String,
    pub secret: String,
}

/// The on-disk (JSONB) shape of `channel_installation.config` for wecom
/// smart-bot rows. `app_id == bot_id` so the shared
/// idx_channel_installation_type_appid index and GetChannelInstallationByAppID
/// query stay generic.
///
/// Port note: Go's `[]byte` field JSON-marshals as a base64 string (and as
/// `null` when empty); the custom serde below keeps that wire shape so rows
/// written by either implementation read back identically.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InstallConfig {
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub bot_id: String,
    #[serde(default, with = "base64_bytes_opt")]
    pub secret_encrypted: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bot_display_name: String,
}

mod base64_bytes_opt {
    //! `Option<Vec<u8>>` ↔ base64 string / null, matching Go's `[]byte`
    //! JSON encoding (nil slice marshals as `null`).

    use base64::Engine as _;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            None => s.serialize_none(),
            Some(bytes) => {
                s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
            }
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        use serde::Deserialize as _;
        let raw: Option<String> = Option::<String>::deserialize(d)?;
        match raw {
            None => Ok(None),
            Some(text) if text.is_empty() => Ok(Some(Vec::new())),
            Some(text) => base64::engine::general_purpose::STANDARD
                .decode(text.as_bytes())
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }
}

impl InstallConfig {
    /// The sealed secret bytes, or empty when absent.
    pub fn secret_encrypted(&self) -> Vec<u8> {
        self.secret_encrypted.clone().unwrap_or_default()
    }
}

/// Marshals an [`Installation`]'s config-bearing fields into the JSONB blob
/// stored in `channel_installation.config`.
pub fn encode_install_config(inst: &Installation) -> anyhow::Result<serde_json::Value> {
    if inst.bot_id.is_empty() {
        anyhow::bail!("wecom: bot_id is required");
    }
    // Go's nil []byte marshals as null; an empty sealed blob is stored the
    // same way so rows written by either implementation read back identically.
    let secret = if inst.secret_encrypted.is_empty() {
        None
    } else {
        Some(inst.secret_encrypted.clone())
    };
    let cfg = InstallConfig {
        app_id: inst.bot_id.clone(),
        bot_id: inst.bot_id.clone(),
        secret_encrypted: secret,
        bot_display_name: inst.bot_display_name.clone(),
    };
    serde_json::to_value(cfg).map_err(|e| anyhow::anyhow!("wecom: encode install config: {e}"))
}

/// Hydrates an [`Installation`] from a `channel_installation` row. The row's
/// channel_type is trusted (the callers already scope queries by
/// `channel_type = 'wecom'`), so it is not re-checked here.
pub fn installation_from_row(
    row: &cordy_db::models::ChannelInstallation,
) -> anyhow::Result<Installation> {
    let cfg: InstallConfig = if row.config.is_null() {
        InstallConfig::default()
    } else {
        serde_json::from_value(row.config.clone())
            .map_err(|e| anyhow::anyhow!("wecom: decode installation config: {e}"))?
    };
    Ok(Installation {
        id: row.id,
        workspace_id: row.workspace_id,
        agent_id: row.agent_id,
        installer_user_id: row.installer_user_id,
        status: row.status.clone(),
        secret_encrypted: cfg.secret_encrypted(),
        bot_display_name: cfg.bot_display_name,
        bot_id: cfg.bot_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use serde_json::json;

    #[test]
    fn encode_requires_bot_id() {
        let inst = Installation::default();
        assert!(encode_install_config(&inst).is_err());
    }

    #[test]
    fn encode_roundtrips_with_go_wire_shape() {
        let inst = Installation {
            bot_id: "bot-1".to_string(),
            secret_encrypted: b"sealed".to_vec(),
            bot_display_name: "Cordy Bot".to_string(),
            ..Default::default()
        };
        let v = encode_install_config(&inst).unwrap();
        // app_id mirrors bot_id; the secret rides as a base64 string like
        // Go's []byte.
        assert_eq!(v["app_id"], json!("bot-1"));
        assert_eq!(v["bot_id"], json!("bot-1"));
        assert_eq!(
            v["secret_encrypted"],
            json!(base64::engine::general_purpose::STANDARD.encode(b"sealed"))
        );
        assert_eq!(v["bot_display_name"], json!("Cordy Bot"));

        let back: InstallConfig = serde_json::from_value(v).unwrap();
        assert_eq!(back.bot_id, "bot-1");
        assert_eq!(back.secret_encrypted(), b"sealed".to_vec());
        assert_eq!(back.bot_display_name, "Cordy Bot");
    }

    #[test]
    fn empty_secret_marshals_as_null_like_go() {
        let inst = Installation {
            bot_id: "bot-2".to_string(),
            ..Default::default()
        };
        let v = encode_install_config(&inst).unwrap();
        assert!(v["secret_encrypted"].is_null());
    }

    #[test]
    fn decode_tolerates_missing_fields() {
        let cfg: InstallConfig =
            serde_json::from_value(json!({"app_id": "a", "bot_id": "b"})).unwrap();
        assert_eq!(cfg.bot_id, "b");
        assert!(cfg.secret_encrypted.is_none());
        assert_eq!(cfg.bot_display_name, "");
    }
}
