//! Channel-backed store domain types for the Feishu integration — port of
//! `server/internal/integrations/lark/store.go`.
//!
//! MUL-3515 generalized the lark_* tables into channel_* (a channel_type
//! discriminator + a JSONB `config` blob for the platform-specific
//! identifiers/credentials). This module owns the one boundary where that
//! JSONB is (de)serialized: the rest of the crate keeps working with flat
//! domain structs whose fields mirror the retired db.Lark* rows
//! one-for-one, so the call sites are a mechanical rename rather than a
//! reshape.
//!
//! The feishu config blob carries exactly the columns that used to be flat on
//! lark_installation / lark_user_binding:
//!
//! ```text
//! installation: app_id, app_secret_encrypted (base64), tenant_key,
//!               bot_open_id, bot_union_id, region
//! user binding: union_id
//! ```
//!
//! app_secret_encrypted is secretbox ciphertext stored as a base64 string.
//! The decoder is whitespace-tolerant on purpose: the migration backfill writes
//! it via PostgreSQL encode(...,'base64'), which MIME-wraps every 76 chars, and
//! a sealed ~72-byte secret exceeds that. The encoder always emits unwrapped
//! base64, so rows written by this adapter are already clean; stripping on read
//! keeps both sources interchangeable.

use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use cordy_db::models::{
    ChannelBindingToken, ChannelChatSessionBinding, ChannelInboundMessageDedup,
    ChannelInstallation, ChannelOutboundCardMessage, ChannelUserBinding,
};

/// The flat, feishu-shaped view of a channel_installation row. It keeps field
/// parity with the lark_installation row it replaced, so the cutover was a
/// rename at the ~190 call sites. The feishu-specific fields (app_id,
/// app_secret_encrypted, tenant_key, bot_open_id, bot_union_id, region) come
/// from the JSONB config; the rest are flat columns.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Installation {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub app_id: String,
    /// secretbox ciphertext; never plaintext. Never log or persist it.
    pub app_secret_encrypted: Vec<u8>,
    pub tenant_key: Option<String>,
    pub bot_open_id: String,
    pub installer_user_id: Uuid,
    pub status: String,
    pub ws_lease_token: Option<String>,
    pub ws_lease_expires_at: Option<DateTime<Utc>>,
    pub installed_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub bot_union_id: Option<String>,
    pub region: String,
}

/// The flat view of a channel_user_binding row. channel_user_id is the feishu
/// open_id; union_id (secondary identity) lives in the JSONB config.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UserBinding {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub cordy_user_id: Uuid,
    pub installation_id: Uuid,
    pub channel_user_id: String,
    pub union_id: Option<String>,
    pub bound_at: DateTime<Utc>,
}

/// The flat view of a channel_chat_session_binding row.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatSessionBinding {
    pub id: Uuid,
    pub chat_session_id: Uuid,
    pub installation_id: Uuid,
    pub channel_chat_id: String,
    pub chat_type: String,
    /// Carries the real chat id ([`LarkBindingConfig`]) when channel_chat_id
    /// is a composite "chat:thread" topic-isolation key; `{}` otherwise.
    pub config: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub last_message_id: Option<String>,
    pub last_thread_id: Option<String>,
}

/// The flat view of a channel_inbound_message_dedup row. Every field is a
/// flat column (no JSON), so this mirrors the channel row 1:1.
#[derive(Debug, Clone, PartialEq)]
pub struct InboundMessageDedup {
    pub installation_id: Uuid,
    pub message_id: String,
    pub received_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub claim_token: Uuid,
}

/// The flat view of a channel_binding_token row. channel_user_id is the
/// feishu open_id the token will bind once redeemed. (Named *Row to avoid
/// colliding with [`crate::binding_token::BindingToken`], which is the
/// freshly-minted raw-token shape returned to the caller.)
#[derive(Debug, Clone, PartialEq)]
pub struct BindingTokenRow {
    pub token_hash: String,
    pub workspace_id: Uuid,
    pub installation_id: Uuid,
    pub channel_user_id: String,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// The flat view of a channel_outbound_card_message row.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboundCardMessage {
    pub id: Uuid,
    pub chat_session_id: Uuid,
    pub task_id: Option<Uuid>,
    pub channel_chat_id: String,
    pub channel_card_message_id: String,
    pub status: String,
    pub last_patched_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// The JSON shape of channel_installation.config for the feishu channel.
/// app_secret_encrypted is decoded by hand (see [`decode_secret`]) rather than
/// as a json Vec<u8> field, so MIME-wrapped base64 from the SQL backfill
/// round-trips too. skip_serializing_if mirrors the migration's
/// jsonb_strip_nulls.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FeishuInstallConfig {
    #[serde(rename = "app_id", default)]
    app_id: String,
    #[serde(
        rename = "app_secret_encrypted",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    app_secret_encrypted: String,
    #[serde(
        rename = "tenant_key",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    tenant_key: String,
    #[serde(
        rename = "bot_open_id",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    bot_open_id: String,
    #[serde(
        rename = "bot_union_id",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    bot_union_id: String,
    #[serde(rename = "region", default, skip_serializing_if = "String::is_empty")]
    region: String,
}

/// The JSON shape of channel_user_binding.config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeishuBindingConfig {
    #[serde(rename = "union_id", default, skip_serializing_if = "String::is_empty")]
    pub union_id: String,
}

/// Decodes a channel_installation row (flat columns + JSONB config) into the
/// flat [`Installation`] domain struct.
pub fn installation_from_row(row: ChannelInstallation) -> anyhow::Result<Installation> {
    let cfg: FeishuInstallConfig = if row.config.is_null() {
        FeishuInstallConfig::default()
    } else {
        serde_json::from_value(row.config)
            .map_err(|e| anyhow::anyhow!("decode installation config: {e}"))?
    };
    let secret = decode_secret(&cfg.app_secret_encrypted)
        .map_err(|e| anyhow::anyhow!("decode app_secret_encrypted: {e}"))?;
    Ok(Installation {
        id: row.id,
        workspace_id: row.workspace_id,
        agent_id: row.agent_id,
        app_id: cfg.app_id,
        app_secret_encrypted: secret,
        tenant_key: text_or_none(&cfg.tenant_key),
        bot_open_id: cfg.bot_open_id,
        installer_user_id: row.installer_user_id,
        status: row.status,
        ws_lease_token: row.ws_lease_token,
        ws_lease_expires_at: row.ws_lease_expires_at,
        installed_at: row.installed_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        bot_union_id: text_or_none(&cfg.bot_union_id),
        region: cfg.region,
    })
}

/// Decodes the opaque JSONB config supplied to a channel Factory. Identity
/// columns are intentionally absent: the Router resolves those from the
/// durable installation row for every inbound event.
pub fn installation_from_config(config: serde_json::Value) -> anyhow::Result<Installation> {
    let cfg: FeishuInstallConfig = if config.is_null() {
        FeishuInstallConfig::default()
    } else {
        serde_json::from_value(config)
            .map_err(|e| anyhow::anyhow!("decode installation config: {e}"))?
    };
    Ok(Installation {
        app_id: cfg.app_id,
        app_secret_encrypted: decode_secret(&cfg.app_secret_encrypted)
            .map_err(|e| anyhow::anyhow!("decode app_secret_encrypted: {e}"))?,
        tenant_key: text_or_none(&cfg.tenant_key),
        bot_open_id: cfg.bot_open_id,
        bot_union_id: text_or_none(&cfg.bot_union_id),
        region: cfg.region,
        ..Default::default()
    })
}

/// Builds the channel_installation.config JSONB from the feishu fields of an
/// [`Installation`]. The secret is emitted as unwrapped base64.
pub fn encode_install_config(inst: &Installation) -> anyhow::Result<serde_json::Value> {
    let cfg = FeishuInstallConfig {
        app_id: inst.app_id.clone(),
        app_secret_encrypted: if inst.app_secret_encrypted.is_empty() {
            String::new()
        } else {
            base64::engine::general_purpose::STANDARD.encode(&inst.app_secret_encrypted)
        },
        tenant_key: inst.tenant_key.clone().unwrap_or_default(),
        bot_open_id: inst.bot_open_id.clone(),
        bot_union_id: inst.bot_union_id.clone().unwrap_or_default(),
        region: inst.region.clone(),
    };
    serde_json::to_value(cfg).map_err(|e| anyhow::anyhow!("encode install config: {e}"))
}

/// Decodes a channel_user_binding row into [`UserBinding`].
pub fn user_binding_from_row(row: ChannelUserBinding) -> anyhow::Result<UserBinding> {
    let cfg: FeishuBindingConfig = if row.config.is_null() {
        FeishuBindingConfig::default()
    } else {
        serde_json::from_value(row.config)
            .map_err(|e| anyhow::anyhow!("decode user binding config: {e}"))?
    };
    Ok(UserBinding {
        id: row.id,
        workspace_id: row.workspace_id,
        cordy_user_id: row.cordy_user_id,
        installation_id: row.installation_id,
        channel_user_id: row.channel_user_id,
        union_id: text_or_none(&cfg.union_id),
        bound_at: row.bound_at,
    })
}

/// Builds channel_user_binding.config from a [`UserBinding`]. Returns the
/// null-stripped JSON (an absent union_id is `{}`), so the upsert's
/// `config || jsonb_strip_nulls(EXCLUDED.config)` merge never clobbers a
/// previously-captured union_id with this write.
pub fn encode_binding_config(b: &UserBinding) -> anyhow::Result<serde_json::Value> {
    serde_json::to_value(FeishuBindingConfig {
        union_id: b.union_id.clone().unwrap_or_default(),
    })
    .map_err(|e| anyhow::anyhow!("encode binding config: {e}"))
}

/// Copies a channel_chat_session_binding row into the flat domain struct.
/// Config stays the raw JSON value; [`crate::outbound::outbound_chat_id`]
/// decodes it.
pub fn chat_session_binding_from_row(row: ChannelChatSessionBinding) -> ChatSessionBinding {
    ChatSessionBinding {
        id: row.id,
        chat_session_id: row.chat_session_id,
        installation_id: row.installation_id,
        channel_chat_id: row.channel_chat_id,
        chat_type: row.chat_type,
        config: row.config,
        created_at: row.created_at,
        last_message_id: row.last_message_id,
        last_thread_id: row.last_thread_id,
    }
}

/// Copies a channel_inbound_message_dedup row into the flat domain struct.
/// No JSON: every field is a flat column.
pub fn dedup_from_row(row: ChannelInboundMessageDedup) -> InboundMessageDedup {
    InboundMessageDedup {
        installation_id: row.installation_id,
        message_id: row.message_id,
        received_at: row.received_at,
        processed_at: row.processed_at,
        claim_token: row.claim_token,
    }
}

/// Copies a channel_binding_token row into the flat domain struct.
pub fn binding_token_from_row(row: ChannelBindingToken) -> BindingTokenRow {
    BindingTokenRow {
        token_hash: row.token_hash,
        workspace_id: row.workspace_id,
        installation_id: row.installation_id,
        channel_user_id: row.channel_user_id,
        expires_at: row.expires_at,
        consumed_at: row.consumed_at,
        created_at: row.created_at,
    }
}

/// Copies a channel_outbound_card_message row into the flat domain struct.
pub fn outbound_card_from_row(row: ChannelOutboundCardMessage) -> OutboundCardMessage {
    OutboundCardMessage {
        id: row.id,
        chat_session_id: row.chat_session_id,
        task_id: row.task_id,
        channel_chat_id: row.channel_chat_id,
        channel_card_message_id: row.channel_card_message_id,
        status: row.status,
        last_patched_at: row.last_patched_at,
        created_at: row.created_at,
    }
}

/// Base64-decodes the stored app secret ciphertext. It tolerates the newline
/// wrapping PostgreSQL's encode(...,'base64') inserts, so secrets written by
/// the SQL backfill and by [`encode_install_config`] both decode. An empty
/// string yields an empty vec (an installation mid-registration before the
/// secret is sealed).
pub fn decode_secret(s: &str) -> anyhow::Result<Vec<u8>> {
    if s.is_empty() {
        return Ok(Vec::new());
    }
    base64::engine::general_purpose::STANDARD
        .decode(strip_whitespace(s).as_bytes())
        .map_err(|e| anyhow::anyhow!("base64 decode: {e}"))
}

/// Removes the ASCII whitespace MIME base64 wrapping introduces.
fn strip_whitespace(s: &str) -> String {
    s.chars()
        .filter(|r| !matches!(r, ' ' | '\t' | '\n' | '\r'))
        .collect()
}

/// Go's `textOrNull`: empty string → None (NULL), otherwise Some.
pub(crate) fn text_or_none(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_installation() -> Installation {
        Installation {
            id: Uuid::nil(),
            workspace_id: Uuid::nil(),
            agent_id: Uuid::nil(),
            app_id: "cli_a1".into(),
            app_secret_encrypted: b"cipher".to_vec(),
            tenant_key: Some("tk".into()),
            bot_open_id: "ou_bot".into(),
            installer_user_id: Uuid::nil(),
            status: "active".into(),
            ws_lease_token: None,
            ws_lease_expires_at: None,
            installed_at: Utc::now(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            bot_union_id: Some("un_1".into()),
            region: "feishu".into(),
        }
    }

    #[test]
    fn install_config_round_trips_and_strips_nulls() {
        let inst = sample_installation();
        let cfg = encode_install_config(&inst).unwrap();
        assert_eq!(cfg["app_id"], json!("cli_a1"));
        assert!(cfg.get("ws_lease_token").is_none());
        // omitempty: no empty-string keys survive.
        let bare = encode_install_config(&Installation {
            tenant_key: None,
            bot_union_id: None,
            ..sample_installation()
        })
        .unwrap();
        assert!(bare.get("tenant_key").is_none());
        assert!(bare.get("bot_union_id").is_none());

        let decoded: FeishuInstallConfig = serde_json::from_value(cfg).unwrap();
        assert_eq!(decoded.app_id, "cli_a1");
        assert_eq!(decoded.tenant_key, "tk");
    }

    #[test]
    fn secret_round_trip_and_mime_wrapping() {
        use base64::Engine as _;
        let enc = base64::engine::general_purpose::STANDARD.encode(b"sealed-secret");
        assert_eq!(decode_secret(&enc).unwrap(), b"sealed-secret");
        // MIME-wrapped form written by the SQL backfill decodes too.
        let wrapped = format!("{}\n{}", &enc[..4], &enc[4..]);
        assert_eq!(decode_secret(&wrapped).unwrap(), b"sealed-secret");
        assert!(decode_secret("").unwrap().is_empty());
    }

    #[test]
    fn binding_config_null_stripping() {
        fn binding(union_id: Option<&str>) -> UserBinding {
            UserBinding {
                id: Uuid::nil(),
                workspace_id: Uuid::nil(),
                cordy_user_id: Uuid::nil(),
                installation_id: Uuid::nil(),
                channel_user_id: "ou_x".into(),
                union_id: union_id.map(str::to_string),
                bound_at: Utc::now(),
            }
        }
        assert_eq!(
            encode_binding_config(&binding(Some("un"))).unwrap(),
            json!({"union_id": "un"})
        );
        // Pins the jsonb_strip_nulls merge: an absent union_id must serialize
        // as "{}" so the upsert never clobbers a captured union_id.
        assert_eq!(encode_binding_config(&binding(None)).unwrap(), json!({}));
    }

    #[test]
    fn text_or_none_mirrors_go_text_or_null() {
        assert_eq!(text_or_none(""), None);
        assert_eq!(text_or_none("x"), Some("x".to_string()));
    }
}
