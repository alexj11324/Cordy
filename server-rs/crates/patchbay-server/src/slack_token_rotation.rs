//! Managed Slack OAuth token rotation.

use std::time::Duration;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use futures_util::{stream, StreamExt};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

const REFRESH_AHEAD: chrono::Duration = chrono::Duration::minutes(30);
const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const HEALTH_PROBE_CONCURRENCY: usize = 8;

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    ok: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: i64,
}

#[derive(Debug, Deserialize)]
struct AuthTestResponse {
    ok: bool,
    #[serde(default)]
    error: String,
}

const MANAGED_SLACK_OBSERVER_TOKEN: &str = "managed:slack:webhook:v1";

pub fn start(
    pool: sqlx::PgPool,
    cfg: &patchbay_config::Config,
    cancel: CancellationToken,
) -> Option<tokio::task::JoinHandle<()>> {
    if patchbay_handler::config::resolved_messaging_mode(cfg) != "managed" {
        return None;
    }
    let client_id = required(cfg.integrations.slack_client_id.as_deref())?;
    let client_secret = required(cfg.integrations.slack_client_secret.as_deref())?;
    let key = patchbay_util::secretbox::load_key("PATCHBAY_SLACK_SECRET_KEY").ok()?;
    let secret_box = patchbay_util::secretbox::SecretBox::new(&key).ok()?;
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .ok()?;
    Some(tokio::spawn(async move {
        let mut interval = tokio::time::interval(REFRESH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(error) = refresh_due_installations(
                        &pool,
                        &client,
                        &secret_box,
                        &client_id,
                        &client_secret,
                    ).await {
                        tracing::error!(%error, "managed Slack token rotation sweep failed");
                    }
                }
            }
        }
    }))
}

fn required(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

async fn refresh_due_installations(
    pool: &sqlx::PgPool,
    client: &reqwest::Client,
    secret_box: &patchbay_util::secretbox::SecretBox,
    client_id: &str,
    client_secret: &str,
) -> anyhow::Result<()> {
    let rows = patchbay_db::queries::channel::list_active_channel_installations(
        pool,
        patchbay_slack::TYPE_SLACK,
    )
    .await?;
    stream::iter(rows)
        .for_each_concurrent(HEALTH_PROBE_CONCURRENCY, |row| async move {
            refresh_installation(
                pool,
                client,
                secret_box,
                client_id,
                client_secret,
                row,
            )
            .await;
        })
        .await;
    Ok(())
}

async fn refresh_installation(
    pool: &sqlx::PgPool,
    client: &reqwest::Client,
    secret_box: &patchbay_util::secretbox::SecretBox,
    client_id: &str,
    client_secret: &str,
    row: patchbay_db::models::ChannelInstallation,
) {
    let cfg: patchbay_slack::config::InstallConfig =
        match serde_json::from_value(row.config.clone()) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, installation_id = %row.id, "invalid Slack installation config during token rotation");
                return;
            }
        };
    if cfg.transport != "webhook" {
        return;
    }
    let rotated_access_token = if !cfg.refresh_token_encrypted.is_empty()
        && needs_rotation(cfg.token_expires_at, Utc::now())
    {
        match rotate_access_token(
            pool,
            client,
            secret_box,
            client_id,
            client_secret,
            row.id,
            &cfg,
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, installation_id = %row.id, "Slack token rotation failed");
                None
            }
        }
    } else {
        None
    };
    let access_token = match rotated_access_token {
        Some(value) => value,
        None => match decrypt_stored_token(secret_box, &cfg.bot_token_encrypted) {
            Ok(value) if !value.is_empty() => value,
            Ok(_) => {
                record_health(
                    pool,
                    row.id,
                    "error",
                    Some("credential_missing"),
                    Some("The managed Slack access token is missing."),
                )
                .await;
                return;
            }
            Err(error) => {
                tracing::error!(%error, installation_id = %row.id, "failed to decrypt Slack access token");
                record_health(
                    pool,
                    row.id,
                    "error",
                    Some("credential_decryption_failed"),
                    Some("The managed Slack credential could not be read."),
                )
                .await;
                return;
            }
        },
    };
    probe_health(pool, client, row.id, &access_token).await;
}

async fn rotate_access_token(
    pool: &sqlx::PgPool,
    client: &reqwest::Client,
    secret_box: &patchbay_util::secretbox::SecretBox,
    client_id: &str,
    client_secret: &str,
    installation_id: uuid::Uuid,
    cfg: &patchbay_slack::config::InstallConfig,
) -> anyhow::Result<Option<String>> {
    let refresh_token = decrypt_stored_token(secret_box, &cfg.refresh_token_encrypted)?;
    if refresh_token.is_empty() {
        return Ok(None);
    }
    let response = client
        .post("https://slack.com/api/oauth.v2.access")
        .basic_auth(client_id, Some(client_secret))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<RefreshResponse>()
        .await?;
    if !response.ok
        || response.access_token.is_empty()
        || response.refresh_token.is_empty()
        || response.expires_in <= 0
    {
        anyhow::bail!("Slack rejected token rotation: {}", response.error);
    }
    let sealed_access = base64::engine::general_purpose::STANDARD
        .encode(secret_box.seal(response.access_token.as_bytes())?);
    let sealed_refresh = base64::engine::general_purpose::STANDARD
        .encode(secret_box.seal(response.refresh_token.as_bytes())?);
    let expires_at = Utc::now() + chrono::Duration::seconds(response.expires_in);
    let updated = sqlx::query(
        r#"UPDATE channel_installation
SET config = config || jsonb_build_object(
        'bot_token_encrypted', $2::text,
        'refresh_token_encrypted', $3::text,
        'token_expires_at', to_jsonb($4::timestamptz)
    ),
    updated_at = now()
WHERE id = $1
  AND status = 'active'
  AND config ->> 'refresh_token_encrypted' = $5::text"#,
    )
    .bind(installation_id)
    .bind(sealed_access)
    .bind(sealed_refresh)
    .bind(expires_at)
    .bind(&cfg.refresh_token_encrypted)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Ok(None);
    }
    tracing::info!(%installation_id, %expires_at, "managed Slack token rotated");
    Ok(Some(response.access_token))
}

fn decrypt_stored_token(
    secret_box: &patchbay_util::secretbox::SecretBox,
    encrypted: &str,
) -> anyhow::Result<String> {
    // `Decrypter` is a process-safe trait object and therefore `'static`.
    // SecretBox is intentionally cheap to clone, so give the closure owned
    // key material instead of letting a borrowed function argument escape.
    let secret_box = secret_box.clone();
    let decrypt = move |sealed: &[u8]| secret_box.open(sealed).map_err(anyhow::Error::from);
    patchbay_slack::config::decrypt_token(encrypted, Some(&decrypt))
}

async fn probe_health(
    pool: &sqlx::PgPool,
    client: &reqwest::Client,
    installation_id: uuid::Uuid,
    access_token: &str,
) {
    let response = client
        .post("https://slack.com/api/auth.test")
        .bearer_auth(access_token)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status);
    match response {
        Ok(response) => match response.json::<AuthTestResponse>().await {
            Ok(value) if value.ok => {
                record_health(pool, installation_id, "healthy", None, None).await;
            }
            Ok(value) => {
                tracing::warn!(%installation_id, error = value.error, "managed Slack health probe rejected");
                record_health(
                    pool,
                    installation_id,
                    "error",
                    Some("authentication_failed"),
                    Some("Slack rejected the managed app credential."),
                )
                .await;
            }
            Err(error) => {
                tracing::warn!(%error, %installation_id, "managed Slack health response decode failed");
                record_health(
                    pool,
                    installation_id,
                    "degraded",
                    Some("health_probe_invalid_response"),
                    Some("Slack returned an unreadable health response."),
                )
                .await;
            }
        },
        Err(error) => {
            tracing::warn!(%error, %installation_id, "managed Slack health probe failed");
            record_health(
                pool,
                installation_id,
                "degraded",
                Some("health_probe_failed"),
                Some("The managed Slack health probe could not reach Slack."),
            )
            .await;
        }
    }
}

async fn record_health(
    pool: &sqlx::PgPool,
    installation_id: uuid::Uuid,
    state: &str,
    error_code: Option<&str>,
    error_summary: Option<&str>,
) {
    if let Err(error) = patchbay_db::queries::channel::upsert_channel_runtime_observation(
        pool,
        installation_id,
        MANAGED_SLACK_OBSERVER_TOKEN,
        state,
        error_code,
        error_summary,
    )
    .await
    {
        tracing::warn!(%error, %installation_id, "failed to record managed Slack health");
    }
}

fn needs_rotation(expires_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    expires_at.is_some_and(|expires_at| expires_at <= now + REFRESH_AHEAD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_starts_before_access_token_expiry() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        assert!(needs_rotation(
            Some(now + chrono::Duration::minutes(29)),
            now,
        ));
        assert!(!needs_rotation(
            Some(now + chrono::Duration::minutes(31)),
            now,
        ));
        assert!(!needs_rotation(None, now));
    }
}
