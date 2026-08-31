//! Configuration loading for the Patchbay Rust server.
//!
//! Stable environment-variable names preserve the deployment contract, while
//! a TOML file is also accepted for local development. Environment variables
//! take precedence over file values.
//!
//! The inventory covers startup-critical reads across authentication,
//! middleware, and storage.

use std::path::Path;

use serde::Deserialize;

pub mod agent_concurrency;

/// Top-level configuration.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub auth: AuthConfig,
    pub urls: UrlsConfig,
    pub storage: StorageConfig,
    pub email: EmailConfig,
    pub llm: LlmConfig,
    pub integrations: IntegrationsConfig,
    pub entitlement: EntitlementConfig,
    pub fleet: FleetConfig,
}

/// Core HTTP server settings.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// `PORT` — Go default "8080".
    pub port: u16,
    /// `APP_ENV` — "production" gates safety checks (insecure secrets refused,
    /// dev verification code ignored).
    pub app_env: Option<String>,
}

// Manual impl on purpose: derive would yield port = 0; Go defaults to 8080.
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            app_env: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    /// `DATABASE_URL` (pgx conn string). Required.
    pub url: Option<String>,
    pub min_connections: u32,
    pub max_connections: u32,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: None,
            min_connections: 5,
            max_connections: 25,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RedisConfig {
    /// `REDIS_URL`. Unset = single-node in-memory mode.
    pub url: Option<String>,
    /// `CHANNEL_WS_LEASE_REDIS_URL` / `_BACKEND` / `_NAMESPACE`.
    pub channel_ws_lease_url: Option<String>,
    pub channel_ws_lease_backend: Option<String>,
    pub channel_ws_lease_namespace: Option<String>,
    /// `REALTIME_RELAY_REDIS_URL` / `REALTIME_RELAY_MODE`.
    pub realtime_relay_url: Option<String>,
    pub realtime_relay_mode: Option<String>,
    /// `REALTIME_METRICS_TOKEN`.
    pub realtime_metrics_token: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// `JWT_SECRET` — empty falls back to a dev-only default in Go; production
    /// must refuse insecure values (denylist enforced by the auth layer, S4).
    pub jwt_secret: Option<String>,
    /// `AUTH_TOKEN_TTL` — duration string, parsed by the auth layer.
    pub auth_token_ttl: Option<String>,
    /// `ALLOW_SIGNUP`, `ALLOWED_EMAILS`, `ALLOWED_EMAIL_DOMAINS`.
    pub allow_signup: Option<String>,
    pub allowed_emails: Option<String>,
    pub allowed_email_domains: Option<String>,
    /// `COOKIE_DOMAIN`.
    pub cookie_domain: Option<String>,
    /// `PATCHBAY_DEV_VERIFICATION_CODE` — ignored when APP_ENV=production.
    pub dev_verification_code: Option<String>,
    /// Clerk session exchange. `CLERK_JWT_KEY` is the instance PEM public
    /// key; `CLERK_ISSUER` and `CLERK_AUTHORIZED_PARTIES` bind otherwise
    /// valid tokens to this deployment; `CLERK_SECRET_KEY` reads the verified
    /// primary email from Clerk's Backend API.
    pub clerk_secret_key: Option<String>,
    pub clerk_jwt_key: Option<String>,
    pub clerk_issuer: Option<String>,
    pub clerk_authorized_parties: Option<String>,
    /// `PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN` authorizes the independent broker
    /// to bypass only the shared peer-IP limiter on desktop Google exchange.
    pub desktop_broker_auth_token: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct UrlsConfig {
    /// `PATCHBAY_PUBLIC_URL`, `PATCHBAY_APP_URL`, `FRONTEND_ORIGIN`,
    /// `CORS_ALLOWED_ORIGINS`, `PATCHBAY_TRUSTED_PROXIES`,
    /// `RATE_LIMIT_TRUSTED_PROXIES`.
    pub public_url: Option<String>,
    pub app_url: Option<String>,
    pub frontend_origin: Option<String>,
    pub cors_allowed_origins: Option<String>,
    pub trusted_proxies: Option<String>,
    pub rate_limit_trusted_proxies: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// `ATTACHMENT_DOWNLOAD_MODE`.
    pub attachment_download_mode: Option<String>,
    /// `ATTACHMENT_DOWNLOAD_URL_TTL` — Go duration, default 30 minutes.
    pub attachment_download_url_ttl: Option<String>,
    /// `LOCAL_UPLOAD_DIR` / `LOCAL_UPLOAD_BASE_URL` (local storage driver).
    pub local_upload_dir: Option<String>,
    pub local_upload_base_url: Option<String>,
    /// `CLOUDFRONT_DOMAIN` / `_KEY_PAIR_ID` / `_PRIVATE_KEY` /
    /// `_PRIVATE_KEY_SECRET` (signed URLs). AWS credentials come from the
    /// standard `AWS_*` env vars read by aws-config itself.
    pub cloudfront_domain: Option<String>,
    pub cloudfront_key_pair_id: Option<String>,
    pub cloudfront_private_key: Option<String>,
    pub cloudfront_private_key_secret: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct EmailConfig {
    /// `RESEND_API_KEY`, `SMTP_HOST`.
    pub resend_api_key: Option<String>,
    pub smtp_host: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    /// `PATCHBAY_LLM_API_KEY` / `_BASE_URL` / `_DEFAULT_MODEL` / `_MAX_RETRIES`.
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub max_retries: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct IntegrationsConfig {
    /// Composio: `COMPOSIO_API_KEY` / `_CALLBACK_BASE_URL` / `_STATE_SECRET`.
    pub composio_api_key: Option<String>,
    pub composio_callback_base_url: Option<String>,
    pub composio_state_secret: Option<String>,
    /// Lark: `PATCHBAY_LARK_CALLBACK_BASE_URL` / `_HTTP_BASE_URL` /
    /// `_REGISTRATION_DOMAIN` / `_REGISTRATION_LARK_DOMAIN` / `_WS_PROXY_URL`.
    pub lark_callback_base_url: Option<String>,
    pub lark_http_base_url: Option<String>,
    pub lark_registration_domain: Option<String>,
    pub lark_registration_lark_domain: Option<String>,
    pub lark_ws_proxy_url: Option<String>,
    /// WeCom: `PATCHBAY_WECOM_MEDIA_ALLOW_CIDRS` / `PATCHBAY_WECOM_TRACE`.
    pub wecom_media_allow_cidrs: Option<String>,
    pub wecom_trace: Option<String>,
    /// `PATCHBAY_VCS_INTEGRATION_ENABLED`.
    pub vcs_integration_enabled: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct EntitlementConfig {
    /// `PATCHBAY_ENTITLEMENT_POLICY_URL` / `_SERVICE_TOKEN`.
    pub policy_url: Option<String>,
    pub service_token: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct FleetConfig {
    /// `PATCHBAY_FLEET_URL` / `PATCHBAY_CLOUD_FLEET_URL`.
    pub fleet_url: Option<String>,
    pub cloud_fleet_url: Option<String>,
}

fn env_str(slot: &mut Option<String>, key: &str) {
    if let Ok(v) = std::env::var(key) {
        let v = v.trim().to_string();
        if !v.is_empty() {
            *slot = Some(v);
        }
    }
}

fn env_u32(slot: &mut Option<u32>, key: &str) -> anyhow::Result<()> {
    if let Ok(v) = std::env::var(key) {
        let v = v.trim().to_string();
        if !v.is_empty() {
            *slot = Some(
                v.parse()
                    .map_err(|_| anyhow::anyhow!("invalid {key} {v:?}: expected u32"))?,
            );
        }
    }
    Ok(())
}

impl Config {
    /// Load config from an optional TOML file, then apply env overrides.
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        let mut cfg: Config = match path {
            Some(p) if p.exists() => {
                let raw = std::fs::read_to_string(p)
                    .map_err(|e| anyhow::anyhow!("read config {}: {e}", p.display()))?;
                toml::from_str(&raw)
                    .map_err(|e| anyhow::anyhow!("parse config {}: {e}", p.display()))?
            }
            _ => Config::default(),
        };
        cfg.apply_env_overrides()?;
        Ok(cfg)
    }

    fn apply_env_overrides(&mut self) -> anyhow::Result<()> {
        // server
        if let Ok(port) = std::env::var("PORT") {
            let port = port.trim().to_string();
            if !port.is_empty() {
                self.server.port = port
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid PORT {port:?}: expected u16"))?;
            }
        }
        env_str(&mut self.server.app_env, "APP_ENV");

        // database / redis
        env_str(&mut self.database.url, "DATABASE_URL");
        let mut min_connections = Some(self.database.min_connections);
        let mut max_connections = Some(self.database.max_connections);
        env_u32(&mut min_connections, "DATABASE_MIN_CONNS")?;
        env_u32(&mut max_connections, "DATABASE_MAX_CONNS")?;
        self.database.min_connections = min_connections.unwrap_or_default();
        self.database.max_connections = max_connections.unwrap_or_default();
        if self.database.min_connections > self.database.max_connections {
            self.database.min_connections = self.database.max_connections;
        }
        env_str(&mut self.redis.url, "REDIS_URL");
        env_str(
            &mut self.redis.channel_ws_lease_url,
            "CHANNEL_WS_LEASE_REDIS_URL",
        );
        env_str(
            &mut self.redis.channel_ws_lease_backend,
            "CHANNEL_WS_LEASE_BACKEND",
        );
        env_str(
            &mut self.redis.channel_ws_lease_namespace,
            "CHANNEL_WS_LEASE_NAMESPACE",
        );
        env_str(
            &mut self.redis.realtime_relay_url,
            "REALTIME_RELAY_REDIS_URL",
        );
        env_str(&mut self.redis.realtime_relay_mode, "REALTIME_RELAY_MODE");
        env_str(
            &mut self.redis.realtime_metrics_token,
            "REALTIME_METRICS_TOKEN",
        );

        // auth
        env_str(&mut self.auth.jwt_secret, "JWT_SECRET");
        env_str(&mut self.auth.auth_token_ttl, "AUTH_TOKEN_TTL");
        env_str(&mut self.auth.allow_signup, "ALLOW_SIGNUP");
        env_str(&mut self.auth.allowed_emails, "ALLOWED_EMAILS");
        env_str(
            &mut self.auth.allowed_email_domains,
            "ALLOWED_EMAIL_DOMAINS",
        );
        env_str(&mut self.auth.cookie_domain, "COOKIE_DOMAIN");
        env_str(
            &mut self.auth.dev_verification_code,
            "PATCHBAY_DEV_VERIFICATION_CODE",
        );
        env_str(&mut self.auth.clerk_secret_key, "CLERK_SECRET_KEY");
        env_str(&mut self.auth.clerk_jwt_key, "CLERK_JWT_KEY");
        env_str(&mut self.auth.clerk_issuer, "CLERK_ISSUER");
        env_str(
            &mut self.auth.clerk_authorized_parties,
            "CLERK_AUTHORIZED_PARTIES",
        );
        env_str(
            &mut self.auth.desktop_broker_auth_token,
            "PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN",
        );

        // urls
        env_str(&mut self.urls.public_url, "PATCHBAY_PUBLIC_URL");
        env_str(&mut self.urls.app_url, "PATCHBAY_APP_URL");
        env_str(&mut self.urls.frontend_origin, "FRONTEND_ORIGIN");
        env_str(&mut self.urls.cors_allowed_origins, "CORS_ALLOWED_ORIGINS");
        env_str(&mut self.urls.trusted_proxies, "PATCHBAY_TRUSTED_PROXIES");
        env_str(
            &mut self.urls.rate_limit_trusted_proxies,
            "RATE_LIMIT_TRUSTED_PROXIES",
        );

        // storage
        env_str(
            &mut self.storage.attachment_download_mode,
            "ATTACHMENT_DOWNLOAD_MODE",
        );
        env_str(
            &mut self.storage.attachment_download_url_ttl,
            "ATTACHMENT_DOWNLOAD_URL_TTL",
        );
        env_str(&mut self.storage.local_upload_dir, "LOCAL_UPLOAD_DIR");
        env_str(
            &mut self.storage.local_upload_base_url,
            "LOCAL_UPLOAD_BASE_URL",
        );
        env_str(&mut self.storage.cloudfront_domain, "CLOUDFRONT_DOMAIN");
        env_str(
            &mut self.storage.cloudfront_key_pair_id,
            "CLOUDFRONT_KEY_PAIR_ID",
        );
        env_str(
            &mut self.storage.cloudfront_private_key,
            "CLOUDFRONT_PRIVATE_KEY",
        );
        env_str(
            &mut self.storage.cloudfront_private_key_secret,
            "CLOUDFRONT_PRIVATE_KEY_SECRET",
        );

        // email
        env_str(&mut self.email.resend_api_key, "RESEND_API_KEY");
        env_str(&mut self.email.smtp_host, "SMTP_HOST");

        // llm
        env_str(&mut self.llm.api_key, "PATCHBAY_LLM_API_KEY");
        env_str(&mut self.llm.base_url, "PATCHBAY_LLM_BASE_URL");
        env_str(&mut self.llm.default_model, "PATCHBAY_LLM_DEFAULT_MODEL");
        env_u32(&mut self.llm.max_retries, "PATCHBAY_LLM_MAX_RETRIES")?;

        // integrations
        env_str(&mut self.integrations.composio_api_key, "COMPOSIO_API_KEY");
        env_str(
            &mut self.integrations.composio_callback_base_url,
            "COMPOSIO_CALLBACK_BASE_URL",
        );
        env_str(
            &mut self.integrations.composio_state_secret,
            "COMPOSIO_STATE_SECRET",
        );
        env_str(
            &mut self.integrations.lark_callback_base_url,
            "PATCHBAY_LARK_CALLBACK_BASE_URL",
        );
        env_str(
            &mut self.integrations.lark_http_base_url,
            "PATCHBAY_LARK_HTTP_BASE_URL",
        );
        env_str(
            &mut self.integrations.lark_registration_domain,
            "PATCHBAY_LARK_REGISTRATION_DOMAIN",
        );
        env_str(
            &mut self.integrations.lark_registration_lark_domain,
            "PATCHBAY_LARK_REGISTRATION_LARK_DOMAIN",
        );
        env_str(
            &mut self.integrations.lark_ws_proxy_url,
            "PATCHBAY_LARK_WS_PROXY_URL",
        );
        env_str(
            &mut self.integrations.wecom_media_allow_cidrs,
            "PATCHBAY_WECOM_MEDIA_ALLOW_CIDRS",
        );
        env_str(&mut self.integrations.wecom_trace, "PATCHBAY_WECOM_TRACE");
        env_str(
            &mut self.integrations.vcs_integration_enabled,
            "PATCHBAY_VCS_INTEGRATION_ENABLED",
        );

        // entitlement / fleet
        env_str(
            &mut self.entitlement.policy_url,
            "PATCHBAY_ENTITLEMENT_POLICY_URL",
        );
        env_str(
            &mut self.entitlement.service_token,
            "PATCHBAY_ENTITLEMENT_SERVICE_TOKEN",
        );
        env_str(&mut self.fleet.fleet_url, "PATCHBAY_FLEET_URL");
        env_str(&mut self.fleet.cloud_fleet_url, "PATCHBAY_CLOUD_FLEET_URL");

        Ok(())
    }

    /// Validate required fields. Call after `load`.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.database.url.is_none() {
            anyhow::bail!("DATABASE_URL is required");
        }
        if let Some(secret) = self.auth.desktop_broker_auth_token.as_deref() {
            if secret.len() != 64
                || !secret
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                anyhow::bail!(
                    "PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN must be 64 lowercase hexadecimal characters"
                );
            }
        }
        Ok(())
    }

    /// True when APP_ENV=production — gates safety checks.
    pub fn is_production(&self) -> bool {
        self.server
            .app_env
            .as_deref()
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("production"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that touch process env must not run concurrently.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Tests must not observe ambient env (CI/dev shells often export
    /// DATABASE_URL); clear the vars Config::load reads.
    fn clear_ambient_env() {
        for var in [
            "PORT",
            "DATABASE_URL",
            "DATABASE_MIN_CONNS",
            "DATABASE_MAX_CONNS",
            "REDIS_URL",
            "APP_ENV",
            "JWT_SECRET",
            "CLERK_SECRET_KEY",
            "CLERK_JWT_KEY",
            "CLERK_ISSUER",
            "CLERK_AUTHORIZED_PARTIES",
            "PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN",
            "PATCHBAY_LLM_MAX_RETRIES",
        ] {
            std::env::remove_var(var);
        }
    }

    #[test]
    fn defaults_are_sane() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_ambient_env();
        let cfg = Config::load(None).unwrap();
        assert_eq!(cfg.server.port, 8080);
        assert_eq!(cfg.database.min_connections, 5);
        assert_eq!(cfg.database.max_connections, 25);
        assert!(cfg.database.url.is_none());
        assert!(!cfg.is_production());
    }

    #[test]
    fn parses_toml_file() {
        // Env overrides beat file values, so ambient env must be cleared and
        // this test serialized against the other env-touching tests.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_ambient_env();
        let dir = std::env::temp_dir().join(format!("patchbay-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("patchbay.toml");
        std::fs::write(
            &path,
            "[server]\nport = 9090\n\n[database]\nmin_connections = 2\nmax_connections = 5\n\n[urls]\npublic_url = \"https://x.example\"\n",
        )
        .unwrap();

        let cfg = Config::load(Some(&path)).unwrap();
        assert_eq!(cfg.server.port, 9090);
        assert_eq!(cfg.database.min_connections, 2);
        assert_eq!(cfg.database.max_connections, 5);
        assert_eq!(cfg.urls.public_url.as_deref(), Some("https://x.example"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn env_overrides_file() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_ambient_env();
        std::env::set_var("PORT", "7777");
        let cfg = Config::load(None).unwrap();
        assert_eq!(cfg.server.port, 7777);
        std::env::remove_var("PORT");
    }

    #[test]
    fn database_pool_env_overrides_match_production_defaults_and_clamp_minimum() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_ambient_env();
        std::env::set_var("DATABASE_MIN_CONNS", "12");
        std::env::set_var("DATABASE_MAX_CONNS", "8");
        let cfg = Config::load(None).unwrap();
        assert_eq!(cfg.database.min_connections, 8);
        assert_eq!(cfg.database.max_connections, 8);
        clear_ambient_env();
    }

    #[test]
    fn typed_env_vars_parse_and_reject_garbage() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_ambient_env();
        std::env::set_var("PATCHBAY_LLM_MAX_RETRIES", "4");
        let cfg = Config::load(None).unwrap();
        assert_eq!(cfg.llm.max_retries, Some(4));
        std::env::set_var("PATCHBAY_LLM_MAX_RETRIES", "lots");
        assert!(Config::load(None).is_err());
        std::env::remove_var("PATCHBAY_LLM_MAX_RETRIES");
    }

    #[test]
    fn production_flag_detected() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_ambient_env();
        std::env::set_var("APP_ENV", "Production");
        let cfg = Config::load(None).unwrap();
        assert!(cfg.is_production());
        std::env::remove_var("APP_ENV");
    }

    #[test]
    fn validate_requires_database_url() {
        let cfg = Config::default();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_malformed_desktop_broker_auth_token() {
        let mut cfg = Config::default();
        cfg.database.url = Some("postgres://invalid/invalid".into());
        cfg.auth.desktop_broker_auth_token = Some("A".repeat(64));
        assert_eq!(
            cfg.validate().unwrap_err().to_string(),
            "PATCHBAY_DESKTOP_BROKER_AUTH_TOKEN must be 64 lowercase hexadecimal characters"
        );
        cfg.auth.desktop_broker_auth_token = Some("a".repeat(64));
        assert!(cfg.validate().is_ok());
    }
}
