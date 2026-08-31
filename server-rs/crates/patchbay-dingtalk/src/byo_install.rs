//! Bring-your-own-app install.
//!
//! The user creates their own DingTalk Stream-mode robot and pastes its AppKey
//! (client id) + AppSecret (client secret). There is NO OAuth code exchange:
//! the credentials are validated live by minting an access_token (which proves
//! the AppKey/AppSecret pair is valid), the AppSecret is encrypted at rest,
//! and the installation is persisted.

use base64::Engine as _;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use patchbay_db::models::ChannelInstallation;

use crate::client::{fetch_access_token, DEFAULT_API_BASE};
use crate::config::InstallConfig;
use crate::install::{InstallError, InstallPersist, InstallService};

/// Returned when a pasted credential is empty. The handler maps it to 400 so
/// the dialog can show a precise hint instead of a generic failure.
///
/// Port note: Go's sentinels become typed variants. Go wraps the mint error
/// into the sentinel with `%w: %v`; Rust carries the cause as the variant's
/// `#[source]`, so the chain (and the handler's downcast) keeps both.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ByoError {
    #[error("dingtalk: AppKey (client id) is required")]
    InvalidAppKey,
    #[error("dingtalk: AppSecret (client secret) is required")]
    InvalidAppSecret,
    /// Wraps a live access-token mint that rejected the pasted AppKey /
    /// AppSecret. It is a user error (bad credentials), so the handler maps it
    /// to 400 — unlike an internal encrypt/persist failure, which must surface
    /// as 500.
    #[error("dingtalk: could not validate credentials: {0}")]
    CredentialValidation(String),
    /// Forwards the install-transaction conflict sentinels verbatim so the
    /// handler keeps rendering the accurate owner message.
    #[error(transparent)]
    Install(#[from] InstallError),
}

/// Inputs for a bring-your-own-app install: the agent this bot represents,
/// who is installing, and the two credentials the user pasted from their own
/// DingTalk Stream-mode robot.
#[derive(Debug, Clone)]
pub struct RegisterByoParams {
    pub workspace_id: Uuid,
    pub agent_id: Uuid,
    pub initiator_id: Uuid,
    /// client id — robotCode + access-token mint.
    pub app_key: String,
    /// client secret — access-token mint (encrypted at rest).
    pub app_secret: String,
}

/// Bounds the live access-token validation call.
const VALIDATION_TIMEOUT: Duration = Duration::from_secs(10);

/// Installs user-supplied ("bring your own") DingTalk robots: validates the
/// credentials live, seals the AppSecret at rest, and persists one
/// installation per default agent. The dedicated Stream connection that
/// consumes the stored credentials lives in [`crate::dingtalk_channel`]; this
/// service only persists the installation.
///
/// Because each BYO robot is a distinct DingTalk app — a distinct bot identity
/// — the SAME DingTalk organization can host several of them, one per agent.
/// The stored config carries the AppKey as the routing key (`config->>'app_id'`,
/// equal to the inbound event's robotCode for a Stream-mode robot);
/// [`InstallService::persist_install`] keys the row by (workspace, agent),
/// reclaims a DEAD prior owner of that AppKey (a revoked placeholder, or an
/// orphan whose workspace/agent was deleted) so the robot can move to this
/// agent, and refuses a LIVE owner with an accurate conflict sentinel.
#[derive(Clone)]
pub struct ByoInstallService {
    install: InstallService,
    box_: Arc<patchbay_util::secretbox::SecretBox>,
    http: reqwest::Client,
    api_base: String,
}

impl ByoInstallService {
    /// Binds the service to the pool and the at-rest encryption box. The box
    /// MUST be valid — plaintext secrets are never stored, even in dev.
    pub fn new(
        pool: PgPool,
        box_: Arc<patchbay_util::secretbox::SecretBox>,
        http: Option<reqwest::Client>,
        api_base: &str,
    ) -> Self {
        Self {
            install: InstallService::new(pool),
            box_,
            http: http.unwrap_or_default(),
            api_base: if api_base.is_empty() {
                DEFAULT_API_BASE.to_string()
            } else {
                api_base.to_string()
            },
        }
    }

    /// Overrides the API base used for the BYO access-token validation call
    /// (tests point it at a local server).
    pub fn with_api_base(mut self, api_base: &str) -> Self {
        self.api_base = api_base.to_string();
        self
    }

    /// Installs a user-supplied ("bring your own") DingTalk robot for a
    /// default agent. See the type docs for the routing/reclaim semantics.
    pub async fn register_byo(
        &self,
        p: RegisterByoParams,
    ) -> Result<ChannelInstallation, anyhow::Error> {
        self.register_byo_with_limit(p, None).await
    }

    /// Registers a BYO robot and, when requested by the hosted handler,
    /// enforces the installation cap inside the persistence transaction.
    pub async fn register_byo_with_limit(
        &self,
        p: RegisterByoParams,
        installation_limit: Option<i64>,
    ) -> Result<ChannelInstallation, anyhow::Error> {
        let app_key = p.app_key.trim();
        let app_secret = p.app_secret.trim();
        if app_key.is_empty() {
            return Err(ByoError::InvalidAppKey.into());
        }
        if app_secret.is_empty() {
            return Err(ByoError::InvalidAppSecret.into());
        }

        // Validate the credentials live: a successful access_token mint proves
        // the AppKey/AppSecret pair is real and installed. The robotCode of a
        // Stream-mode robot equals the AppKey, so no separate identity lookup
        // is needed. A timeout and a mint failure both surface as the same
        // user-facing validation error, with Go's `%w: %v` wrapping kept in
        // the message.
        let mint = fetch_access_token(&self.http, &self.api_base, app_key, app_secret);
        let mint_result = match tokio::time::timeout(VALIDATION_TIMEOUT, mint).await {
            Err(_) => Err(anyhow::anyhow!(
                "credential validation timed out after {VALIDATION_TIMEOUT:?}"
            )),
            Ok(r) => r,
        };
        if let Err(err) = mint_result {
            return Err(ByoError::CredentialValidation(format!("{err:#}")).into());
        }

        let sealed_secret = self
            .box_
            .seal(app_secret.as_bytes())
            .map_err(|e| anyhow::anyhow!("encrypt dingtalk app secret: {e}"))?;
        let cfg = InstallConfig {
            app_id: app_key.to_string(),
            robot_code: app_key.to_string(),
            app_secret_encrypted: base64::engine::general_purpose::STANDARD.encode(&sealed_secret),
        };
        let config_json = serde_json::to_value(&cfg)
            .map_err(|e| anyhow::anyhow!("encode dingtalk installation config: {e}"))?;

        // Persist one installation per default agent (the row is keyed by
        // workspace + agent). Group routes can target other agents without
        // another Stream connection. The stored config carries the AppKey for
        // inbound routing; persist_install reclaims a DEAD prior owner of that
        // AppKey so the robot can move to this agent, and refuses a LIVE owner
        // with an accurate conflict sentinel.
        self.install
            .persist_install_with_limit(&InstallPersist {
                ws_id: p.workspace_id,
                agent_id: p.agent_id,
                installer_id: p.initiator_id,
                app_id_key: app_key.to_string(),
                config_json,
            }, installation_limit)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> ByoInstallService {
        let key = [7u8; 32];
        ByoInstallService::new(
            sqlx::PgPool::connect_lazy("postgres://invalid").unwrap(),
            Arc::new(patchbay_util::secretbox::SecretBox::new(&key).unwrap()),
            None,
            "",
        )
    }

    #[tokio::test]
    async fn empty_app_key_is_rejected_before_any_io() {
        let err = service()
            .register_byo(RegisterByoParams {
                workspace_id: Uuid::nil(),
                agent_id: Uuid::nil(),
                initiator_id: Uuid::nil(),
                app_key: "  ".into(),
                app_secret: "s".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "dingtalk: AppKey (client id) is required");
    }

    #[tokio::test]
    async fn empty_app_secret_is_rejected() {
        let err = service()
            .register_byo(RegisterByoParams {
                workspace_id: Uuid::nil(),
                agent_id: Uuid::nil(),
                initiator_id: Uuid::nil(),
                app_key: "cli_a".into(),
                app_secret: String::new(),
            })
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "dingtalk: AppSecret (client secret) is required"
        );
    }
}
