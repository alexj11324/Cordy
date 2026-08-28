use anyhow::{anyhow, Result};
use base64::Engine as _;
use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
struct InstallConfig {
    #[serde(default)]
    app_id: String,
    #[serde(default)]
    ilink_user_id: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    bot_token_encrypted: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Credentials {
    pub bot_id: String,
    pub ilink_user_id: String,
    pub base_url: String,
    pub bot_token: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublicConfig {
    pub bot_id: String,
    pub ilink_user_id: String,
}

pub type DecrypterFn = dyn Fn(&[u8]) -> Result<Vec<u8>> + Send + Sync;

pub fn decode_credentials(
    value: &serde_json::Value,
    decrypt: Option<&DecrypterFn>,
) -> Result<Credentials> {
    let config: InstallConfig = serde_json::from_value(value.clone())
        .map_err(|error| anyhow!("decode weixin installation config: {error}"))?;
    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(
            config
                .bot_token_encrypted
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>(),
        )
        .map_err(|error| anyhow!("decode weixin bot token: {error}"))?;
    let plaintext = match decrypt {
        Some(decrypt) => decrypt(&ciphertext)?,
        None => ciphertext,
    };
    let bot_token = String::from_utf8(plaintext)
        .map_err(|error| anyhow!("decode weixin bot token text: {error}"))?;
    if config.app_id.is_empty() || bot_token.is_empty() {
        anyhow::bail!("weixin installation is missing bot credentials");
    }
    Ok(Credentials {
        bot_id: config.app_id,
        ilink_user_id: config.ilink_user_id,
        base_url: config.base_url,
        bot_token,
    })
}

pub fn decode_public_config(value: &serde_json::Value) -> PublicConfig {
    let config = serde_json::from_value::<InstallConfig>(value.clone()).unwrap_or_default();
    PublicConfig {
        bot_id: config.app_id,
        ilink_user_id: config.ilink_user_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_projection_never_exposes_token() {
        let raw = serde_json::json!({
            "app_id": "bot@im.bot",
            "ilink_user_id": "owner@im.wechat",
            "bot_token_encrypted": "c2VjcmV0"
        });
        let public = decode_public_config(&raw);
        assert_eq!(public.bot_id, "bot@im.bot");
        assert_eq!(public.ilink_user_id, "owner@im.wechat");
    }
}
