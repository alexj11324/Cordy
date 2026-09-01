//! Managed Slack OAuth token rotation.

use std::time::Duration;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

const REFRESH_AHEAD: chrono::Duration = chrono::Duration::minutes(30);
const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

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
    for row in rows {
        let cfg: patchbay_slack::config::InstallConfig = match serde_json::from_value(
            row.config.clone(),
        ) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, installation_id = %row.id, "invalid Slack installation config during token rotation");
                continue;
            }
        };
        if cfg.transport != "webhook"
            || cfg.refresh_token_encrypted.is_empty()
            || !needs_rotation(cfg.token_expires_at, Utc::now())
        {
            continue;
        }
        let token_box = secret_box.clone();
        let decrypt = move |sealed: &[u8]| token_box.open(sealed).map_err(anyhow::Error::from);
        let refresh_token = match patchbay_slack::config::decrypt_token(
            &cfg.refresh_token_encrypted,
            Some(&decrypt),
        ) {
            Ok(value) if !value.is_empty() => value,
            Ok(_) => continue,
            Err(error) => {
                tracing::error!(%error, installation_id = %row.id, "failed to decrypt Slack refresh token");
                continue;
            }
        };
        let response = client
            .post("https://slack.com/api/oauth.v2.access")
            .basic_auth(client_id, Some(client_secret))
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.as_str()),
            ])
            .send()
            .await
            .and_then(reqwest::Response::error_for_status);
        let refreshed = match response {
            Ok(response) => match response.json::<RefreshResponse>().await {
                Ok(value)
                    if value.ok
                        && !value.access_token.is_empty()
                        && !value.refresh_token.is_empty()
                        && value.expires_in > 0 =>
                {
                    value
                }
                Ok(value) => {
                    tracing::warn!(installation_id = %row.id, error = value.error, "Slack token rotation rejected");
                    continue;
                }
                Err(error) => {
                    tracing::warn!(%error, installation_id = %row.id, "Slack token rotation response decode failed");
                    continue;
                }
            },
            Err(error) => {
                tracing::warn!(%error, installation_id = %row.id, "Slack token rotation request failed");
                continue;
            }
        };
        let sealed_access = base64::engine::general_purpose::STANDARD
            .encode(secret_box.seal(refreshed.access_token.as_bytes())?);
        let sealed_refresh = base64::engine::general_purpose::STANDARD
            .encode(secret_box.seal(refreshed.refresh_token.as_bytes())?);
        let expires_at = Utc::now() + chrono::Duration::seconds(refreshed.expires_in);
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
        .bind(row.id)
        .bind(sealed_access)
        .bind(sealed_refresh)
        .bind(expires_at)
        .bind(&cfg.refresh_token_encrypted)
        .execute(pool)
        .await?;
        if updated.rows_affected() == 1 {
            tracing::info!(installation_id = %row.id, %expires_at, "managed Slack token rotated");
        }
    }
    Ok(())
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
