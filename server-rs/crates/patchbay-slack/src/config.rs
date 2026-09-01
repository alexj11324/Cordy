//! Installation config + credential decoding.
//!
//! `installConfig` is the JSON shape stored in
//! `channel_installation.config` for a Slack installation. The cross-platform
//! columns stay flat; everything Slack-specific lives in this opaque blob (the
//! documented config boundary).
//!
//! `app_id` is the database routing identity: BYO Socket Mode installations
//! keep using the real Slack app id, while the managed multi-tenant app stores
//! `{api_app_id}:{team_id}` so one official app can be installed into many
//! Slack workspaces without changing the existing routing index. `api_app_id`
//! preserves the real Slack app id for display and provider API calls.
//!
//! bot_token_encrypted (xoxb-, outbound Web API: chat.postMessage) and
//! app_token_encrypted (xapp-, this installation's own Socket Mode connection)
//! are both stored as base64-encoded secretbox ciphertext, never plaintext.

use serde::Deserialize;

/// JSON shape stored in channel_installation.config for a Slack installation.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct InstallConfig {
    #[serde(rename = "app_id", default)]
    pub app_id: String,
    #[serde(rename = "api_app_id", default)]
    pub api_app_id: String,
    #[serde(rename = "team_id", default)]
    pub team_id: String,
    #[serde(rename = "bot_user_id", default)]
    pub bot_user_id: String,
    #[serde(rename = "bot_token_encrypted", default)]
    pub bot_token_encrypted: String,
    #[serde(rename = "app_token_encrypted", default)]
    pub app_token_encrypted: String,
    #[serde(default)]
    pub transport: String,
    #[serde(rename = "refresh_token_encrypted", default)]
    pub refresh_token_encrypted: String,
    #[serde(rename = "token_expires_at", default)]
    pub token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub fn managed_routing_key(app_id: &str, team_id: &str) -> String {
    format!("{app_id}:{team_id}")
}

/// Decoded, decrypted form the outbound sender runs on. The installation
/// IDENTITY (workspace / agent / installer) is deliberately absent: it is
/// resolved per message by the Router's InstallationResolver.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Credentials {
    pub team_id: String,
    pub bot_user_id: String,
    pub bot_token: String,
}

/// Turns stored ciphertext into plaintext. Wiring injects a secretbox-backed
/// implementation; tests inject an identity decrypter (or None, which treats
/// the stored bytes as plaintext).
pub type Decrypter = dyn Fn(&[u8]) -> Result<Vec<u8>, anyhow::Error> + Send + Sync;

/// Thread-safe shared handle to a decrypter, the form resolvers and the typing
/// manager carry between add and clear.
pub type DecrypterArc = std::sync::Arc<Decrypter>;

/// Parses the per-installation config blob and decrypts the stored tokens. It
/// is the single place the Slack config JSON is interpreted.
pub fn decode_credentials(
    raw: &serde_json::Value,
    decrypt: Option<&Decrypter>,
) -> anyhow::Result<Credentials> {
    if raw.is_null() {
        anyhow::bail!("slack: empty installation config");
    }
    let cfg: InstallConfig = serde_json::from_value(raw.clone())
        .map_err(|e| anyhow::anyhow!("decode slack installation config: {e}"))?;
    let bot_token = decrypt_token(&cfg.bot_token_encrypted, decrypt)
        .map_err(|e| anyhow::anyhow!("decrypt bot token: {e}"))?;
    // A missing team falls back to the app id so the credentials always carry a
    // workspace-ish identifier (mirrors the Go fallback).
    let team_id = if cfg.team_id.is_empty() {
        cfg.app_id.clone()
    } else {
        cfg.team_id
    };
    Ok(Credentials {
        team_id,
        bot_user_id: cfg.bot_user_id,
        bot_token,
    })
}

/// Non-secret subset of an installation config, safe to surface on the
/// management API (the encrypted bot token is never included).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct PublicConfig {
    #[serde(rename = "app_id")]
    pub app_id: String,
    #[serde(rename = "team_id")]
    pub team_id: String,
    #[serde(rename = "bot_user_id")]
    pub bot_user_id: String,
}

/// Extracts the display-safe fields from a stored config blob. A decode miss
/// yields a zero-value PublicConfig rather than an error: the management list
/// should still render the row's identity columns.
pub fn decode_public_config(raw: &serde_json::Value) -> PublicConfig {
    let cfg: InstallConfig = serde_json::from_value(raw.clone()).unwrap_or_default();
    let team_id = if cfg.team_id.is_empty() {
        cfg.app_id.clone()
    } else {
        cfg.team_id
    };
    PublicConfig {
        app_id: if cfg.api_app_id.is_empty() {
            cfg.app_id
        } else {
            cfg.api_app_id
        },
        team_id,
        bot_user_id: cfg.bot_user_id,
    }
}

/// Base64-decodes the stored ciphertext (tolerating the MIME newline wrapping
/// PostgreSQL's encode(...,'base64') emits) and runs it through the injected
/// Decrypter. An empty stored value decodes to an empty token; a None
/// Decrypter treats the decoded bytes as plaintext (test convenience).
pub fn decrypt_token(enc: &str, decrypt: Option<&Decrypter>) -> anyhow::Result<String> {
    if enc.is_empty() {
        return Ok(String::new());
    }
    use base64::Engine as _;
    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(strip_whitespace(enc).as_bytes())
        .map_err(|e| anyhow::anyhow!("base64 decode: {e}"))?;
    match decrypt {
        None => Ok(String::from_utf8_lossy(&ciphertext).into_owned()),
        Some(f) => {
            let plaintext = f(&ciphertext)?;
            Ok(String::from_utf8_lossy(&plaintext).into_owned())
        }
    }
}

/// Removes ASCII whitespace so a MIME-wrapped base64 string (newlines every 64
/// chars) and an unwrapped one decode identically.
fn strip_whitespace(s: &str) -> String {
    s.chars()
        .filter(|r| !matches!(r, ' ' | '\t' | '\n' | '\r'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    #[test]
    fn decode_credentials_falls_back_team_to_app_id() {
        let enc = base64::engine::general_purpose::STANDARD.encode(b"xoxb-token");
        let raw = serde_json::json!({
            "app_id": "A123",
            "bot_user_id": "U1",
            "bot_token_encrypted": enc,
        });
        let creds = decode_credentials(&raw, None).unwrap();
        assert_eq!(creds.bot_token, "xoxb-token");
        assert_eq!(creds.team_id, "A123");
        assert_eq!(creds.bot_user_id, "U1");
    }

    #[test]
    fn decode_credentials_rejects_empty_config() {
        assert!(decode_credentials(&serde_json::Value::Null, None).is_err());
    }

    #[test]
    fn decode_public_config_survives_garbage() {
        let cfg = decode_public_config(&serde_json::json!("not an object"));
        assert_eq!(cfg.app_id, "");
        assert_eq!(cfg.team_id, "");
    }

    #[test]
    fn decode_public_config_exposes_real_managed_app_id() {
        let cfg = decode_public_config(&serde_json::json!({
            "app_id": "A123:T456",
            "api_app_id": "A123",
            "team_id": "T456"
        }));
        assert_eq!(cfg.app_id, "A123");
        assert_eq!(cfg.team_id, "T456");
    }

    #[test]
    fn decrypt_token_tolerates_mime_wrapping_and_passthrough() {
        let wrapped = "aGVsbG8K\naGVsbG8="; // newline-wrapped base64
        assert_eq!(decrypt_token(wrapped, None).unwrap(), "hello\nhello");
        assert_eq!(decrypt_token("", None).unwrap(), "");
    }

    #[test]
    fn decrypt_token_runs_injected_decrypter() {
        let enc = base64::engine::general_purpose::STANDARD.encode(b"cipher");
        let upper = |ct: &[u8]| Ok::<_, anyhow::Error>(ct.to_ascii_uppercase());
        assert_eq!(decrypt_token(&enc, Some(&upper)).unwrap(), "CIPHER");
    }

    #[test]
    fn team_id_read_does_not_fall_back_to_app_id() {
        // install_team_id (resolvers) must NOT apply the app_id fallback the
        // credentials decoder uses: team routing matches the real workspace.
        let raw = serde_json::json!({"app_id": "A123"});
        assert_eq!(super::super::resolvers::install_team_id(&raw), "");
        let raw = serde_json::json!({"team_id": "T1", "app_id": "A123"});
        assert_eq!(super::super::resolvers::install_team_id(&raw), "T1");
        // Undecodable JSON yields "" rather than an error.
        assert_eq!(
            super::super::resolvers::install_team_id(&serde_json::json!("junk")),
            ""
        );
    }
}
