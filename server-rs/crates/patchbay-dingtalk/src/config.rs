//! Port of `config.go`: the per-installation config blob, credential
//! decryption, and the channel discriminator.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// JSON shape stored in channel_installation.config for a DingTalk
/// installation. The cross-platform columns stay flat; everything
/// DingTalk-specific lives in this opaque blob (the documented config
/// boundary).
///
/// `app_id` holds the AppKey, which for a Stream-mode robot equals the inbound
/// event's robotCode. It is the per-installation routing key: the generic
/// GetChannelInstallationByAppID query (`config->>'app_id'`) and the
/// (channel_type, app_id) unique index map an inbound event's robotCode to its
/// installation, so several robots — several agents — in one DingTalk org stay
/// distinct.
///
/// `robot_code` is kept explicit for the outbound send APIs
/// (oToMessages.batchSend / groupMessages.send both require it); it equals
/// app_id but is stored separately so the outbound path never has to assume the
/// equivalence.
///
/// `app_secret_encrypted` is base64-encoded secretbox ciphertext, never
/// plaintext. The AppKey itself is not a secret and lives in app_id in the
/// clear.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstallConfig {
    #[serde(rename = "app_id")]
    pub app_id: String,
    #[serde(
        rename = "robot_code",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub robot_code: String,
    #[serde(rename = "app_secret_encrypted")]
    pub app_secret_encrypted: String,
}

impl InstallConfig {
    /// Returns the explicit robot_code, falling back to app_id for the
    /// Stream-mode robot where the two are equal (older configs stored only
    /// app_id).
    pub fn robot_code_or_app_id(&self) -> &str {
        if !self.robot_code.is_empty() {
            &self.robot_code
        } else {
            &self.app_id
        }
    }
}

/// The decoded, decrypted form the outbound sender and the access-token cache
/// run on. The installation IDENTITY (workspace / agent / installer) is
/// deliberately absent: it is resolved per message by the Router's
/// InstallationResolver.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Credentials {
    pub app_key: String,
    pub robot_code: String,
    pub app_secret: String,
}

/// Turns stored ciphertext into plaintext. The wiring injects a
/// secretbox-backed implementation; tests inject None (stored bytes treated as
/// plaintext).
pub type Decrypter = dyn Fn(&[u8]) -> Result<Vec<u8>, anyhow::Error> + Send + Sync;

/// Parses the per-installation config blob and decrypts the stored AppSecret.
/// It is the single place the DingTalk config JSON is interpreted for the
/// outbound/token paths.
pub fn decode_credentials(
    raw: &serde_json::Value,
    decrypt: Option<&Decrypter>,
) -> anyhow::Result<Credentials> {
    if raw.is_null() {
        anyhow::bail!("dingtalk: empty installation config");
    }
    let cfg: InstallConfig = serde_json::from_value(raw.clone())
        .map_err(|e| anyhow::anyhow!("decode dingtalk installation config: {e}"))?;
    let app_secret = decrypt_token(&cfg.app_secret_encrypted, decrypt)
        .map_err(|e| anyhow::anyhow!("decrypt app secret: {e}"))?;
    let robot_code = cfg.robot_code_or_app_id().to_string();
    Ok(Credentials {
        app_key: cfg.app_id,
        robot_code,
        app_secret,
    })
}

/// Base64-decodes the stored ciphertext (tolerating the MIME newline wrapping
/// PostgreSQL's encode(...,'base64') emits) and runs it through the injected
/// decrypter. An empty stored value decodes to an empty secret; a None
/// decrypter treats the decoded bytes as plaintext (test convenience).
pub fn decrypt_token(enc: &str, decrypt: Option<&Decrypter>) -> anyhow::Result<String> {
    if enc.is_empty() {
        return Ok(String::new());
    }
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
    use base64::engine::general_purpose::STANDARD;

    #[test]
    fn robot_code_falls_back_to_app_id() {
        let cfg = InstallConfig {
            app_id: "cli_a".into(),
            robot_code: String::new(),
            app_secret_encrypted: String::new(),
        };
        assert_eq!(cfg.robot_code_or_app_id(), "cli_a");
        let cfg2 = InstallConfig {
            robot_code: "rc".into(),
            ..cfg
        };
        assert_eq!(cfg2.robot_code_or_app_id(), "rc");
    }

    #[test]
    fn decode_credentials_roundtrips_plaintext_without_decrypter() {
        let enc = STANDARD.encode(b"sekret");
        let raw = serde_json::json!({
            "app_id": "cli_a",
            "app_secret_encrypted": enc,
        });
        let creds = decode_credentials(&raw, None).unwrap();
        assert_eq!(creds.app_key, "cli_a");
        assert_eq!(creds.robot_code, "cli_a"); // falls back to app_id
        assert_eq!(creds.app_secret, "sekret");
    }

    #[test]
    fn decode_credentials_tolerates_mime_wrapped_base64() {
        let wrapped = "c2Vr\ncmV0\n"; // "sekret" split across lines
        let raw = serde_json::json!({
            "app_id": "cli_a",
            "robot_code": "rc1",
            "app_secret_encrypted": wrapped,
        });
        let creds = decode_credentials(&raw, None).unwrap();
        assert_eq!(creds.app_secret, "sekret");
        assert_eq!(creds.robot_code, "rc1");
    }

    #[test]
    fn empty_config_is_an_error() {
        assert!(decode_credentials(&serde_json::Value::Null, None).is_err());
    }

    #[test]
    fn empty_secret_decodes_to_empty() {
        let raw = serde_json::json!({"app_id": "a", "app_secret_encrypted": ""});
        let creds = decode_credentials(&raw, None).unwrap();
        assert_eq!(creds.app_secret, "");
    }

    #[test]
    fn decrypter_is_applied_to_decoded_bytes() {
        let enc = STANDARD.encode(b"ciphertext");
        let raw = serde_json::json!({
            "app_id": "a",
            "app_secret_encrypted": enc,
        });
        let creds = decode_credentials(
            &raw,
            Some(&|b: &[u8]| Ok(b.iter().rev().copied().collect())),
        )
        .unwrap();
        assert_eq!(creds.app_secret, "txetrehpic");
    }
}
