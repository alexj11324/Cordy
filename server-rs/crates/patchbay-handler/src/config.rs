//! Public runtime configuration consumed before authentication.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use patchbay_service::feature_flags::{self, FlagSource};
use serde::Serialize;
use std::collections::HashMap;
use std::net::IpAddr;
use url::Url;

use crate::state::HandlerState;

#[derive(Clone, Debug)]
pub struct PublicConfigSettings {
    pub cdn_domain: String,
    pub cdn_signed: bool,
    pub server_version: String,
    pub allow_signup: bool,
    pub daemon_server_url: String,
    pub daemon_app_url: String,
    pub official_cloud: bool,
    /// Account-facing URL used by hosted channel binding flows.  This is
    /// intentionally separate from the local daemon URL: loopback and LAN
    /// origins are never safe to put in an IM message or QR payload.
    pub messaging_bind_url: String,
    pub messaging: MessagingCapabilities,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MessagingCapabilities {
    pub mode: String,
    #[serde(rename = "setupWritable")]
    pub setup_writable: bool,
    pub platforms: Vec<MessagingPlatformCapability>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MessagingPlatformCapability {
    #[serde(rename = "type")]
    pub channel_type: String,
    pub enabled: bool,
    pub experimental: bool,
}

impl Default for MessagingCapabilities {
    fn default() -> Self {
        Self {
            mode: "disabled".into(),
            setup_writable: false,
            platforms: messaging_platforms(false),
        }
    }
}

impl Default for PublicConfigSettings {
    fn default() -> Self {
        Self {
            cdn_domain: String::new(),
            cdn_signed: false,
            server_version: String::new(),
            allow_signup: true,
            daemon_server_url: String::new(),
            daemon_app_url: String::new(),
            official_cloud: false,
            messaging_bind_url: String::new(),
            messaging: MessagingCapabilities::default(),
        }
    }
}

impl PublicConfigSettings {
    pub fn from_config(
        config: &patchbay_config::Config,
        cdn_domain: String,
        cdn_signed: bool,
        server_version: String,
    ) -> Self {
        let app_url = resolve_frontend_app_url_from_config(config);
        let official_cloud = is_official_cloud_daemon_config(&app_url);
        let messaging_bind_url = public_bind_url_from_config(config, official_cloud);
        let messaging =
            messaging_capabilities(config, official_cloud, !messaging_bind_url.is_empty());
        let (daemon_server_url, daemon_app_url) = if app_url.is_empty() || official_cloud {
            (String::new(), String::new())
        } else {
            let public_url =
                normalize_public_url(config.urls.public_url.as_deref().unwrap_or_default());
            let server_url = if public_url.is_empty() {
                app_url.clone()
            } else {
                public_url
            };
            (server_url, app_url.clone())
        };
        Self {
            cdn_domain,
            cdn_signed,
            server_version,
            allow_signup: config.auth.allow_signup.as_deref().map(str::trim) != Some("false"),
            daemon_server_url,
            daemon_app_url,
            official_cloud,
            messaging_bind_url,
            messaging,
        }
    }
}

#[derive(Serialize)]
struct AppConfig {
    cdn_domain: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    cdn_signed: bool,
    allow_signup: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    workspace_creation_disabled: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    daemon_server_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    daemon_app_url: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    vcs_integration_available: bool,
    posthog_key: String,
    posthog_host: String,
    analytics_environment: String,
    feature_flags: HashMap<String, bool>,
    local_worktree_supported: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    server_version: String,
    messaging: MessagingCapabilities,
}

struct DisabledFlags;

impl FlagSource for DisabledFlags {
    fn is_enabled(&self, _key: &str, default: bool) -> bool {
        default
    }
}

pub fn router() -> Router<HandlerState> {
    Router::new().route("/api/config", get(get_config))
}

pub(crate) fn workspace_creation_disabled() -> bool {
    workspace_creation_disabled_value(std::env::var("DISABLE_WORKSPACE_CREATION").ok().as_deref())
}

fn workspace_creation_disabled_value(value: Option<&str>) -> bool {
    value == Some("true")
}

async fn get_config(State(state): State<HandlerState>) -> Json<AppConfig> {
    let analytics_disabled = matches!(
        std::env::var("ANALYTICS_DISABLED").as_deref(),
        Ok("true") | Ok("1")
    );
    let posthog_key = if analytics_disabled {
        String::new()
    } else {
        std::env::var("POSTHOG_API_KEY").unwrap_or_default()
    };
    let posthog_host = if analytics_disabled || posthog_key.is_empty() {
        std::env::var("POSTHOG_HOST").unwrap_or_default()
    } else {
        std::env::var("POSTHOG_HOST")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| patchbay_analytics::client::DEFAULT_POSTHOG_HOST.to_string())
    };
    let analytics_environment = if analytics_disabled {
        String::new()
    } else {
        patchbay_analytics::client::environment_from_env()
    };
    let disabled_flags = DisabledFlags;
    let flags: &dyn FlagSource = state.feature_flags.as_deref().unwrap_or(&disabled_flags);

    Json(AppConfig {
        cdn_domain: state.public_config.cdn_domain.clone(),
        cdn_signed: state.public_config.cdn_signed,
        allow_signup: state.public_config.allow_signup,
        workspace_creation_disabled: workspace_creation_disabled(),
        daemon_server_url: state.public_config.daemon_server_url.clone(),
        daemon_app_url: state.public_config.daemon_app_url.clone(),
        vcs_integration_available: state.vcs_integration_enabled,
        posthog_key,
        posthog_host,
        analytics_environment,
        feature_flags: feature_flags::evaluate_frontend_public_flags(flags),
        local_worktree_supported: true,
        server_version: if state.public_config.official_cloud {
            String::new()
        } else {
            state.public_config.server_version.clone()
        },
        messaging: state.public_config.messaging.clone(),
    })
}

/// Returns the URL that may safely be placed in a cross-device IM message.
/// A local or private origin is deliberately treated as unavailable; callers
/// must show an operator-actionable state instead of manufacturing a
/// localhost link that another device cannot open.
pub fn public_bind_url_from_config(
    config: &patchbay_config::Config,
    official_cloud: bool,
) -> String {
    let app_url = resolve_frontend_app_url_from_config(config);
    if official_cloud {
        // The official managed surface is fixed to the public product host;
        // do not let a development FRONTEND_ORIGIN leak into a hosted bot.
        return "https://patchbay.aspectlylabs.com".into();
    }
    is_public_https_url(&app_url)
        .then_some(app_url)
        .unwrap_or_default()
}

/// Convenience wrapper for runtime components that only have the loaded
/// deployment config.  It applies the same official-host detection as the
/// anonymous `/api/config` response.
pub fn public_bind_url(config: &patchbay_config::Config) -> String {
    let app_url = resolve_frontend_app_url_from_config(config);
    public_bind_url_from_config(config, is_official_cloud_daemon_config(&app_url))
}

fn messaging_capabilities(
    config: &patchbay_config::Config,
    official_cloud: bool,
    public_bind_available: bool,
) -> MessagingCapabilities {
    let requested = config
        .integrations
        .messaging_mode
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let configured = messaging_platforms(true)
        .iter()
        .any(|platform| platform.enabled);
    let mode = match requested {
        Some("managed") => "managed",
        Some("server_configured") => "server_configured",
        Some("disabled") => "disabled",
        _ if official_cloud => "managed",
        _ if configured => "server_configured",
        _ => "disabled",
    };
    // A local-only deployment cannot complete account-level binding from a
    // phone or third-party platform. Advertise the capability as disabled so
    // both UI and backend agree that no cross-device IM setup is available.
    let mode = if public_bind_available || mode == "disabled" {
        mode
    } else {
        "disabled"
    };
    let enabled = mode != "disabled";
    MessagingCapabilities {
        mode: mode.into(),
        setup_writable: mode == "managed",
        platforms: messaging_platforms(enabled),
    }
}

fn messaging_platforms(enabled: bool) -> Vec<MessagingPlatformCapability> {
    [
        ("lark", "PATCHBAY_LARK_SECRET_KEY"),
        ("slack", "PATCHBAY_SLACK_SECRET_KEY"),
        ("dingtalk", "PATCHBAY_DINGTALK_SECRET_KEY"),
        ("wecom", "PATCHBAY_WECOM_SECRET_KEY"),
        ("telegram", "PATCHBAY_TELEGRAM_SECRET_KEY"),
        ("weixin", "PATCHBAY_WEIXIN_SECRET_KEY"),
    ]
    .into_iter()
    .map(|(channel_type, key)| MessagingPlatformCapability {
        channel_type: channel_type.into(),
        enabled: enabled && patchbay_util::secretbox::load_key(key).is_ok(),
        // No provider is promoted to a verified hosted transport by the
        // capability endpoint alone.  Keep the flag true until the provider
        // has passed the real install -> bind -> message -> reply exercise;
        // a configured secret must never be enough to claim production
        // readiness.
        experimental: true,
    })
    .collect()
}

fn is_public_https_url(raw: &str) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let host = url.host_str().unwrap_or_default().trim_end_matches('.');
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") {
        return false;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(ip) => {
                !(ip.is_unspecified() || ip.is_loopback() || ip.is_private() || ip.is_link_local())
            }
            IpAddr::V6(ip) => !(ip.is_loopback() || ip.is_unspecified() || ip.is_unique_local()),
        };
    }
    true
}

fn resolve_frontend_app_url_from_config(config: &patchbay_config::Config) -> String {
    let app_url = normalize_public_url(config.urls.app_url.as_deref().unwrap_or_default());
    if app_url.is_empty() {
        let public_url =
            normalize_public_url(config.urls.public_url.as_deref().unwrap_or_default());
        if public_url.is_empty() {
            normalize_public_url(config.urls.frontend_origin.as_deref().unwrap_or_default())
        } else {
            public_url
        }
    } else {
        app_url
    }
}

fn normalize_public_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

fn is_official_cloud_daemon_config(app_url: &str) -> bool {
    url_host_equals(app_url, "patchbay.aspectlylabs.com")
}

fn url_host_equals(raw: &str, expected: &str) -> bool {
    canonical_url_host(raw)
        .is_some_and(|host| host == expected.trim().trim_end_matches('.').to_ascii_lowercase())
}

fn canonical_url_host(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let parsed = Url::parse(raw).ok().filter(|url| url.host_str().is_some());
    let parsed = match parsed {
        Some(parsed) => parsed,
        None => Url::parse(&format!("https://{raw}")).ok()?,
    };
    Some(
        parsed
            .host_str()?
            .trim_end_matches('.')
            .to_ascii_lowercase(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[test]
    fn canonical_hosts_match_official_cloud_detection() {
        for raw in [
            "https://patchbay.aspectlylabs.com",
            "patchbay.aspectlylabs.com",
            "patchbay.aspectlylabs.com:8080",
            "https://patchbay.aspectlylabs.com.",
        ] {
            assert!(is_official_cloud_daemon_config(raw), "{raw}");
        }
        for raw in [
            "https://patchbay.ai",
            "https://www.aspectlylabs.com",
            "http://localhost:3000",
            "https://evil.example",
            "",
        ] {
            assert!(!is_official_cloud_daemon_config(raw), "{raw}");
        }
    }

    #[test]
    fn workspace_creation_flag_matches_go_exactly() {
        // The Go server deliberately checks os.Getenv(...) == "true".
        assert!(workspace_creation_disabled_value(Some("true")));
        for value in [None, Some("TRUE"), Some("1"), Some("yes"), Some(" true ")] {
            assert!(!workspace_creation_disabled_value(value));
        }
    }

    #[test]
    fn loaded_config_drives_public_auth_and_daemon_urls() {
        let mut config = patchbay_config::Config::default();
        config.auth.allow_signup = Some(" false ".into());
        config.urls.public_url = Some("https://api.example/".into());
        config.urls.app_url = Some("https://app.example/".into());
        let settings =
            PublicConfigSettings::from_config(&config, String::new(), false, "v1".into());
        assert!(!settings.allow_signup);
        assert_eq!(settings.daemon_server_url, "https://api.example");
        assert_eq!(settings.daemon_app_url, "https://app.example");
        assert!(!settings.official_cloud);
    }

    #[test]
    fn official_cloud_suppresses_daemon_urls_and_version() {
        let mut config = patchbay_config::Config::default();
        config.urls.app_url = Some("https://patchbay.aspectlylabs.com".into());
        let settings =
            PublicConfigSettings::from_config(&config, String::new(), false, "v1".into());
        assert!(settings.official_cloud);
        assert!(settings.daemon_server_url.is_empty());
        assert!(settings.daemon_app_url.is_empty());
    }

    #[test]
    fn serialized_shape_keeps_required_false_and_empty_fields() {
        let value = serde_json::to_value(AppConfig {
            cdn_domain: String::new(),
            cdn_signed: false,
            allow_signup: true,
            workspace_creation_disabled: false,
            daemon_server_url: String::new(),
            daemon_app_url: String::new(),
            vcs_integration_available: false,
            posthog_key: String::new(),
            posthog_host: String::new(),
            analytics_environment: String::new(),
            feature_flags: feature_flags::evaluate_frontend_public_flags(&DisabledFlags),
            local_worktree_supported: true,
            server_version: String::new(),
            messaging: MessagingCapabilities::default(),
        })
        .unwrap();
        assert_eq!(value["cdn_domain"], "");
        assert_eq!(value["allow_signup"], true);
        assert_eq!(value["posthog_key"], "");
        assert_eq!(value["local_worktree_supported"], true);
        assert!(value.get("cdn_signed").is_none());
        assert!(value.get("server_version").is_none());
        assert_eq!(value["feature_flags"]["agents_skill_toggles"], true);
        assert_eq!(value["feature_flags"]["plugins_v1"], false);
        assert_eq!(value["messaging"]["mode"], "disabled");
        assert_eq!(value["messaging"]["setupWritable"], false);
    }

    #[tokio::test]
    async fn public_route_is_available_before_authentication() {
        let state = HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            patchbay_auth::pat_cache::PatCache::disabled(),
            None,
        );
        let response = router()
            .with_state(state)
            .oneshot(Request::get("/api/config").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["local_worktree_supported"], true);
        assert_eq!(value["feature_flags"]["agents_agent_builder"], true);
    }

    #[tokio::test]
    async fn loaded_public_config_is_served_before_authentication() {
        let mut config = patchbay_config::Config::default();
        config.auth.allow_signup = Some("false".into());
        config.urls.public_url = Some("https://api.example".into());
        config.urls.app_url = Some("https://app.example".into());
        let state = HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            patchbay_auth::pat_cache::PatCache::disabled(),
            None,
        )
        .with_public_config(PublicConfigSettings::from_config(
            &config,
            String::new(),
            false,
            "v-test".into(),
        ));
        let response = router()
            .with_state(state)
            .oneshot(Request::get("/api/config").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["allow_signup"], false);
        assert_eq!(value["daemon_server_url"], "https://api.example");
        assert_eq!(value["daemon_app_url"], "https://app.example");
        assert_eq!(value["server_version"], "v-test");
    }

    #[test]
    fn cross_device_bind_url_rejects_loopback_and_private_origins() {
        let mut config = patchbay_config::Config::default();
        for raw in [
            "http://localhost:3000",
            "https://localhost:3000",
            "https://127.0.0.1:3000",
            "https://0.0.0.0:3000",
            "https://192.168.1.10",
            "https://patchbay.local",
            "https://user:password@app.example",
            "https://app.example/path?token=secret",
        ] {
            config.urls.app_url = Some(raw.into());
            assert!(
                public_bind_url_from_config(&config, false).is_empty(),
                "{raw}"
            );
        }
        config.urls.app_url = Some("https://app.example".into());
        assert_eq!(
            public_bind_url_from_config(&config, false),
            "https://app.example"
        );
        assert_eq!(
            public_bind_url_from_config(&config, true),
            "https://patchbay.aspectlylabs.com"
        );
    }

    #[test]
    fn explicit_messaging_mode_controls_setup_ownership() {
        let mut config = patchbay_config::Config::default();
        config.integrations.messaging_mode = Some("server_configured".into());
        config.urls.app_url = Some("https://app.example".into());
        let capabilities = messaging_capabilities(&config, false, true);
        assert_eq!(capabilities.mode, "server_configured");
        assert!(!capabilities.setup_writable);

        config.integrations.messaging_mode = Some("disabled".into());
        let capabilities = messaging_capabilities(&config, false, true);
        assert_eq!(capabilities.mode, "disabled");
        assert!(capabilities
            .platforms
            .iter()
            .all(|platform| !platform.enabled));
    }

    #[test]
    fn local_only_messaging_is_disabled_even_when_keys_are_present() {
        let mut config = patchbay_config::Config::default();
        config.integrations.messaging_mode = Some("server_configured".into());
        config.urls.app_url = Some("http://localhost:3000".into());
        let capabilities = messaging_capabilities(&config, false, false);
        assert_eq!(capabilities.mode, "disabled");
        assert!(capabilities
            .platforms
            .iter()
            .all(|platform| !platform.enabled));
    }
}
