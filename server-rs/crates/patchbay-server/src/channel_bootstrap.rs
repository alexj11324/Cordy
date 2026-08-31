//! Server-side bootstrap for self-hosted channel installations.
//!
//! Multica's self-hosted deployment keeps provider credentials in the server
//! deployment rather than asking the browser to collect them.  Cordy follows
//! the same boundary: when an operator explicitly enables bootstrap, this
//! module reads credentials from the server secret environment, encrypts them
//! with the provider secretbox, and materializes the installation rows before
//! the channel supervisor starts.  The app remains read-only in
//! `server_configured` mode.

use std::env;

use anyhow::{Context as _, Result};
use base64::Engine as _;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const BOOTSTRAP_FLAG: &str = "PATCHBAY_MESSAGING_BOOTSTRAP";
const WORKSPACE_ID_ENV: &str = "PATCHBAY_MESSAGING_WORKSPACE_ID";
const INSTALLER_USER_ID_ENV: &str = "PATCHBAY_MESSAGING_INSTALLER_USER_ID";
const AGENT_ID_ENV: &str = "PATCHBAY_MESSAGING_AGENT_ID";

#[derive(Debug, Clone)]
struct BootstrapScope {
    workspace_id: Uuid,
    agent_id: Uuid,
    installer_user_id: Uuid,
}

#[derive(Debug, Clone)]
struct InstallationSpec {
    provider: &'static str,
    channel_type: &'static str,
    app_id: String,
    config: Value,
}

/// Materialize credentials supplied by a self-hosted server operator.
///
/// This is deliberately opt-in. A deployment that only sets
/// `PATCHBAY_MESSAGING_MODE=server_configured` stays read-only and does not
/// mutate its database. The explicit flag makes a restart safe for operators
/// who want to stage the environment before applying it.
pub(crate) async fn provision_from_environment(
    pool: &PgPool,
    config: &patchbay_config::Config,
) -> Result<()> {
    if config.integrations.messaging_mode.as_deref() != Some("server_configured") {
        return Ok(());
    }
    if !env_flag(BOOTSTRAP_FLAG)? {
        return Ok(());
    }

    let specs = installation_specs()?;
    if specs.is_empty() {
        tracing::info!(
            "self-hosted messaging bootstrap enabled but no provider credentials are configured"
        );
        return Ok(());
    }
    let scope = BootstrapScope::from_environment()?;
    for spec in specs {
        persist_installation(pool, &scope, &spec)
            .await
            .with_context(|| format!("bootstrap {} installation", spec.provider))?;
        tracing::info!(
            provider = spec.provider,
            "self-hosted messaging installation bootstrapped"
        );
    }
    Ok(())
}

impl BootstrapScope {
    fn from_environment() -> Result<Self> {
        Ok(Self {
            workspace_id: required_uuid(WORKSPACE_ID_ENV)?,
            installer_user_id: required_uuid(INSTALLER_USER_ID_ENV)?,
            agent_id: optional_uuid(AGENT_ID_ENV)?.unwrap_or_else(Uuid::nil),
        })
    }
}

fn installation_specs() -> Result<Vec<InstallationSpec>> {
    let mut specs = Vec::new();
    if let Some(spec) = slack_spec()? {
        specs.push(spec);
    }
    if let Some(spec) = telegram_spec()? {
        specs.push(spec);
    }
    if let Some(spec) = lark_spec()? {
        specs.push(spec);
    }
    if let Some(spec) = dingtalk_spec()? {
        specs.push(spec);
    }
    if let Some(spec) = wecom_spec()? {
        specs.push(spec);
    }
    // WeChat iLink credentials are minted by the QR authorization flow. A
    // static token is intentionally not accepted here; managed deployments
    // must use the QR flow, and self-hosted deployments remain read-only.
    Ok(specs)
}

fn slack_spec() -> Result<Option<InstallationSpec>> {
    let bot = env_value("SLACK_BOT_TOKEN");
    let app = env_value("SLACK_APP_TOKEN");
    match (bot, app) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => {
            anyhow::bail!("SLACK_BOT_TOKEN and SLACK_APP_TOKEN must be configured together")
        }
        (Some(bot_token), Some(app_token)) => {
            anyhow::ensure!(
                bot_token.starts_with("xoxb-"),
                "SLACK_BOT_TOKEN must start with xoxb-"
            );
            anyhow::ensure!(
                app_token.starts_with("xapp-"),
                "SLACK_APP_TOKEN must start with xapp-"
            );
            let parsed_app_id = parse_slack_app_id(&app_token);
            let app_id = env_value("SLACK_APP_ID")
                .or_else(|| parsed_app_id.map(str::to_owned))
                .ok_or_else(|| anyhow::anyhow!("SLACK_APP_ID is missing or invalid"))?;
            anyhow::ensure!(
                app_id.starts_with('A'),
                "SLACK_APP_ID must be a Slack app id"
            );
            if let Some(parsed_app_id) = parsed_app_id {
                anyhow::ensure!(
                    app_id == parsed_app_id,
                    "SLACK_APP_ID must match the app id in SLACK_APP_TOKEN"
                );
            }
            let team_id = required_value("SLACK_TEAM_ID")?;
            let bot_user_id = required_value("SLACK_BOT_USER_ID")?;
            Ok(Some(InstallationSpec {
                provider: "slack",
                channel_type: patchbay_slack::TYPE_SLACK,
                app_id: app_id.clone(),
                config: json!({
                    "app_id": app_id,
                    "team_id": team_id,
                    "bot_user_id": bot_user_id,
                    "bot_token_encrypted": seal_base64("PATCHBAY_SLACK_SECRET_KEY", &bot_token)?,
                    "app_token_encrypted": seal_base64("PATCHBAY_SLACK_SECRET_KEY", &app_token)?
                }),
            }))
        }
    }
}

fn telegram_spec() -> Result<Option<InstallationSpec>> {
    let Some(bot_token) = env_value("TELEGRAM_BOT_TOKEN") else {
        return Ok(None);
    };
    let bot_id = patchbay_telegram::parse_bot_id(&bot_token)
        .map_err(|_| anyhow::anyhow!("TELEGRAM_BOT_TOKEN has an invalid bot token shape"))?;
    Ok(Some(InstallationSpec {
        provider: "telegram",
        channel_type: patchbay_telegram::TYPE_TELEGRAM,
        app_id: bot_id.clone(),
        config: json!({
            "app_id": bot_id,
            "bot_username": env_value("TELEGRAM_BOT_USERNAME").unwrap_or_default(),
            "bot_token_encrypted": seal_base64("PATCHBAY_TELEGRAM_SECRET_KEY", &bot_token)?
        }),
    }))
}

fn lark_spec() -> Result<Option<InstallationSpec>> {
    let app_id = env_value("LARK_APP_ID");
    let app_secret = env_value("LARK_APP_SECRET");
    match (app_id, app_secret) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => {
            anyhow::bail!("LARK_APP_ID and LARK_APP_SECRET must be configured together")
        }
        (Some(app_id), Some(app_secret)) => Ok(Some(InstallationSpec {
            provider: "lark",
            channel_type: patchbay_lark::channel_store::CHANNEL_TYPE_FEISHU,
            app_id: app_id.clone(),
            config: json!({
                "app_id": app_id,
                "app_secret_encrypted": seal_base64("PATCHBAY_LARK_SECRET_KEY", &app_secret)?,
                "tenant_key": env_value("LARK_TENANT_KEY").unwrap_or_default(),
                "bot_open_id": env_value("LARK_BOT_OPEN_ID").unwrap_or_default(),
                "bot_union_id": env_value("LARK_BOT_UNION_ID").unwrap_or_default(),
                "region": env_value("LARK_REGION").unwrap_or_else(|| "feishu".into())
            }),
        })),
    }
}

fn dingtalk_spec() -> Result<Option<InstallationSpec>> {
    let app_key = env_value("DINGTALK_CLIENT_ID");
    let app_secret = env_value("DINGTALK_CLIENT_SECRET");
    match (app_key, app_secret) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => anyhow::bail!(
            "DINGTALK_CLIENT_ID and DINGTALK_CLIENT_SECRET must be configured together"
        ),
        (Some(app_key), Some(app_secret)) => Ok(Some(InstallationSpec {
            provider: "dingtalk",
            channel_type: patchbay_dingtalk::TYPE_DINGTALK,
            app_id: app_key.clone(),
            config: json!({
                "app_id": app_key.clone(),
                "robot_code": env_value("DINGTALK_ROBOT_CODE").unwrap_or_else(|| app_key.clone()),
                "app_secret_encrypted": seal_base64("PATCHBAY_DINGTALK_SECRET_KEY", &app_secret)?
            }),
        })),
    }
}

fn wecom_spec() -> Result<Option<InstallationSpec>> {
    let bot_id = env_value("WECOM_BOT_ID");
    let secret = env_value("WECOM_SECRET");
    match (bot_id, secret) {
        (None, None) => Ok(None),
        (Some(_), None) | (None, Some(_)) => {
            anyhow::bail!("WECOM_BOT_ID and WECOM_SECRET must be configured together")
        }
        (Some(bot_id), Some(secret)) => {
            let sealed = seal_bytes("PATCHBAY_WECOM_SECRET_KEY", &secret)?;
            Ok(Some(InstallationSpec {
                provider: "wecom",
                channel_type: patchbay_wecom::CHANNEL_TYPE_WECOM,
                app_id: bot_id.clone(),
                config: json!({
                    "app_id": bot_id.clone(),
                    "bot_id": bot_id.clone(),
                    "secret_encrypted": base64::engine::general_purpose::STANDARD.encode(sealed),
                    "bot_display_name": env_value("WECOM_BOT_NAME").unwrap_or_default()
                }),
            }))
        }
    }
}

async fn persist_installation(
    pool: &PgPool,
    scope: &BootstrapScope,
    spec: &InstallationSpec,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    if scope.agent_id.is_nil() {
        patchbay_db::queries::channel::lock_channel_installation_hub_slot(
            &mut *tx,
            spec.channel_type,
            scope.workspace_id,
        )
        .await?;
    } else {
        patchbay_db::queries::channel::lock_channel_installation_agent_slot(
            &mut *tx,
            spec.channel_type,
            scope.workspace_id,
            scope.agent_id,
        )
        .await?;
    }
    patchbay_db::queries::channel::lock_channel_installation_app_id_slot(
        &mut *tx,
        spec.channel_type,
        &spec.app_id,
    )
    .await?;
    patchbay_db::queries::channel::reclaim_dead_channel_installation_by_app_id(
        &mut *tx,
        spec.channel_type,
        &spec.app_id,
        scope.workspace_id,
        scope.agent_id,
    )
    .await?;
    let row = if scope.agent_id.is_nil() {
        patchbay_db::queries::channel::upsert_channel_installation_hub(
            &mut *tx,
            scope.workspace_id,
            spec.channel_type,
            &spec.config,
            scope.installer_user_id,
        )
        .await?
    } else {
        patchbay_db::queries::channel::upsert_channel_installation(
            &mut *tx,
            scope.workspace_id,
            scope.agent_id,
            spec.channel_type,
            &spec.config,
            scope.installer_user_id,
        )
        .await?
    };
    anyhow::ensure!(row.is_some(), "installation upsert returned no row");
    tx.commit().await?;
    Ok(())
}

fn seal_base64(key_env: &str, value: &str) -> Result<String> {
    Ok(base64::engine::general_purpose::STANDARD.encode(seal_bytes(key_env, value)?))
}

fn seal_bytes(key_env: &str, value: &str) -> Result<Vec<u8>> {
    let key = patchbay_util::secretbox::load_key(key_env)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("load {key_env}"))?;
    let secret_box = patchbay_util::secretbox::SecretBox::new(&key)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("initialize {key_env}"))?;
    secret_box
        .seal(value.as_bytes())
        .map_err(anyhow::Error::from)
        .with_context(|| format!("encrypt value for {key_env}"))
}

fn env_value(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn required_value(name: &str) -> Result<String> {
    env_value(name).ok_or_else(|| anyhow::anyhow!("{name} must be configured"))
}

fn required_uuid(name: &str) -> Result<Uuid> {
    let raw = required_value(name)?;
    Uuid::parse_str(&raw).with_context(|| format!("{name} must be a UUID"))
}

fn optional_uuid(name: &str) -> Result<Option<Uuid>> {
    let Some(raw) = env_value(name) else {
        return Ok(None);
    };
    Uuid::parse_str(&raw)
        .with_context(|| format!("{name} must be a UUID"))
        .map(Some)
}

fn env_flag(name: &str) -> Result<bool> {
    parse_env_flag(name, env_value(name).as_deref())
}

fn parse_env_flag(name: &str, value: Option<&str>) -> Result<bool> {
    match value {
        None | Some("0") | Some("false") | Some("no") => Ok(false),
        Some("1") | Some("true") | Some("yes") => Ok(true),
        Some(value) => anyhow::bail!("{name} must be true or false, got {value:?}"),
    }
}

fn parse_slack_app_id(token: &str) -> Option<&str> {
    let mut fields = token.split('-');
    (fields.next() == Some("xapp"))
        .then(|| fields.next())
        .flatten()
        .and_then(|_| fields.next())
        .filter(|id| id.starts_with('A') && id.len() > 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_slack_app_id_from_app_token() {
        assert_eq!(parse_slack_app_id("xapp-1-A123-456"), Some("A123"));
        assert_eq!(parse_slack_app_id("xoxb-123"), None);
        assert_eq!(parse_slack_app_id("xapp-1-B123-456"), None);
    }

    #[test]
    fn falsey_bootstrap_values_are_disabled() {
        for value in ["0", "false", "no"] {
            assert!(!parse_env_flag(BOOTSTRAP_FLAG, Some(value)).unwrap());
        }
        assert!(!parse_env_flag(BOOTSTRAP_FLAG, None).unwrap());
    }
}
