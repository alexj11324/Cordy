//! Installation config decoding: the encrypted bot token blob and the
//! public (non-secret) projection.
//!
//! Port of `server/internal/integrations/telegram/config.go`.

use anyhow::{anyhow, Result};
use base64::Engine as _;
use serde::Deserialize;

/// The platform discriminator persisted on channel_installation.channel_type.
pub const TYPE_TELEGRAM: &str = "telegram";

#[derive(Debug, Clone, Default, Deserialize)]
struct InstallConfig {
    #[serde(default, rename = "app_id")]
    app_id: String,
    #[serde(default, rename = "bot_username")]
    bot_username: String,
    #[serde(default, rename = "bot_token_encrypted")]
    bot_token_encrypted: String,
}

/// Decrypted installation credentials.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Credentials {
    pub bot_id: String,
    pub bot_username: String,
    pub bot_token: String,
}

/// Decrypter seam — Go's `Decrypter func([]byte) ([]byte, error)`.
pub type DecrypterFn = dyn Fn(&[u8]) -> Result<Vec<u8>> + Send + Sync;
pub type Decrypter<'a> = Option<&'a DecrypterFn>;

pub fn decode_credentials(raw: &[u8], decrypt: Decrypter<'_>) -> Result<Credentials> {
    if raw.is_empty() {
        anyhow::bail!("telegram: empty installation config");
    }
    let cfg: InstallConfig = serde_json::from_slice(raw)
        .map_err(|e| anyhow!("decode telegram installation config: {e}"))?;
    let token = decrypt_token(&cfg.bot_token_encrypted, decrypt)
        .map_err(|e| anyhow!("decrypt bot token: {e}"))?;
    Ok(Credentials {
        bot_id: cfg.app_id,
        bot_username: cfg.bot_username,
        bot_token: token,
    })
}

/// The non-secret projection surfaced to clients.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PublicConfig {
    pub bot_id: String,
    pub bot_username: String,
}

pub fn decode_public_config(raw: &[u8]) -> PublicConfig {
    let cfg: InstallConfig = serde_json::from_slice(raw).unwrap_or_default();
    PublicConfig {
        bot_id: cfg.app_id,
        bot_username: cfg.bot_username,
    }
}

fn decrypt_token(enc: &str, decrypt: Decrypter<'_>) -> Result<String> {
    if enc.is_empty() {
        return Ok(String::new());
    }
    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(strip_whitespace(enc))
        .map_err(|e| anyhow!("base64 decode: {e}"))?;
    let plaintext = match decrypt {
        Some(decrypt) => decrypt(&ciphertext)?,
        None => ciphertext,
    };
    Ok(String::from_utf8_lossy(&plaintext).to_string())
}

fn strip_whitespace(s: &str) -> String {
    s.chars()
        .filter(|r| !matches!(r, ' ' | '\t' | '\n' | '\r'))
        .collect()
}

/// Validates the "<numeric bot id>:<secret>" token shape and returns the id.
pub fn parse_bot_id(token: &str) -> Result<String> {
    let token = token.trim();
    let Some((id, secret)) = token.split_once(':') else {
        return Err(invalid_bot_token());
    };
    if id.is_empty() || secret.is_empty() {
        return Err(invalid_bot_token());
    }
    if !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid_bot_token());
    }
    Ok(id.to_string())
}

/// The shared invalid-token error (Go ErrInvalidBotToken).
pub fn invalid_bot_token() -> anyhow::Error {
    anyhow!("telegram: bot token must look like 123456:ABC-DEF…")
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_PLAIN: &str = "123456:ABC-DEF";

    fn passthrough(c: &[u8]) -> Result<Vec<u8>> {
        Ok(c.to_vec())
    }

    fn encoded(plain: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(plain)
    }

    #[test]
    fn decode_credentials_plain_and_encrypted() {
        // Empty config errors.
        assert!(decode_credentials(b"", None).is_err());

        // Unencrypted token (no decrypter) round-trips.
        let raw = format!(
            r#"{{"app_id":"123456","bot_username":"mybot","bot_token_encrypted":"{}"}}"#,
            encoded(KEY_PLAIN)
        );
        let c = decode_credentials(raw.as_bytes(), None).unwrap();
        assert_eq!(c.bot_id, "123456");
        assert_eq!(c.bot_username, "mybot");
        assert_eq!(c.bot_token, KEY_PLAIN);

        // With a decrypter the ciphertext passes through it first.
        let c = decode_credentials(raw.as_bytes(), Some(&passthrough)).unwrap();
        assert_eq!(c.bot_token, KEY_PLAIN);

        // Whitespace inside the base64 is tolerated.
        let spaced = encoded(KEY_PLAIN)
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if i == 4 {
                    format!(" {c}")
                } else {
                    c.to_string()
                }
            })
            .collect::<String>();
        let raw_spaced = format!(r#"{{"app_id":"123456","bot_token_encrypted":"{spaced}"}}"#);
        assert!(decode_credentials(raw_spaced.as_bytes(), None).is_ok());
    }

    #[test]
    fn public_config_ignores_secret_field() {
        let raw = br#"{"app_id":"9","bot_username":"b","bot_token_encrypted":"zzz"}"#;
        let p = decode_public_config(raw);
        assert_eq!(p.bot_id, "9");
        assert_eq!(p.bot_username, "b");
        // Garbage degrades to empty (Go ignores the Unmarshal error).
        assert_eq!(decode_public_config(b"not-json"), PublicConfig::default());
    }

    #[test]
    fn parse_bot_id_validates_shape() {
        assert_eq!(parse_bot_id("123456:ABC").unwrap(), "123456");
        assert!(parse_bot_id("").is_err());
        assert!(parse_bot_id("nocolon").is_err());
        assert!(parse_bot_id(":secret").is_err());
        assert!(parse_bot_id("12a:secret").is_err());
    }
}
