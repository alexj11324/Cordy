//! Public runtime configuration consumed before authentication.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use cordy_service::feature_flags::{self, FlagSource};
use serde::Serialize;
use std::collections::HashMap;
use url::Url;

use crate::state::HandlerState;

#[derive(Clone, Debug)]
pub struct PublicConfigSettings {
    pub cdn_domain: String,
    pub cdn_signed: bool,
    pub server_version: String,
    pub allow_signup: bool,
    pub google_client_id: String,
    pub daemon_server_url: String,
    pub daemon_app_url: String,
    pub official_cloud: bool,
}

impl Default for PublicConfigSettings {
    fn default() -> Self {
        Self {
            cdn_domain: String::new(),
            cdn_signed: false,
            server_version: String::new(),
            allow_signup: true,
            google_client_id: String::new(),
            daemon_server_url: String::new(),
            daemon_app_url: String::new(),
            official_cloud: false,
        }
    }
}

impl PublicConfigSettings {
    pub fn from_config(
        config: &cordy_config::Config,
        cdn_domain: String,
        cdn_signed: bool,
        server_version: String,
    ) -> Self {
        let app_url = resolve_frontend_app_url_from_config(config);
        let official_cloud = is_official_cloud_daemon_config(&app_url);
        let (daemon_server_url, daemon_app_url) = if app_url.is_empty() || official_cloud {
            (String::new(), String::new())
        } else {
            let public_url = normalize_public_url(
                config.urls.public_url.as_deref().unwrap_or_default(),
            );
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
            allow_signup: config
                .auth
                .allow_signup
                .as_deref()
                .map(str::trim)
                != Some("false"),
            google_client_id: config
                .auth
                .google_client_id
                .as_deref()
                .unwrap_or_default()
                .trim()
                .to_string(),
            daemon_server_url,
            daemon_app_url,
            official_cloud,
        }
    }
}

#[derive(Serialize)]
struct AppConfig {
    cdn_domain: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    cdn_signed: bool,
    allow_signup: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    google_client_id: String,
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
            .unwrap_or_else(|| cordy_analytics::client::DEFAULT_POSTHOG_HOST.to_string())
    };
    let analytics_environment = if analytics_disabled {
        String::new()
    } else {
        cordy_analytics::client::environment_from_env()
    };
    let disabled_flags = DisabledFlags;
    let flags: &dyn FlagSource = state.feature_flags.as_deref().unwrap_or(&disabled_flags);

    Json(AppConfig {
        cdn_domain: state.public_config.cdn_domain.clone(),
        cdn_signed: state.public_config.cdn_signed,
        allow_signup: state.public_config.allow_signup,
        google_client_id: state.public_config.google_client_id.clone(),
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
    })
}

fn resolve_frontend_app_url_from_config(config: &cordy_config::Config) -> String {
    let app_url = normalize_public_url(config.urls.app_url.as_deref().unwrap_or_default());
    if app_url.is_empty() {
        normalize_public_url(config.urls.frontend_origin.as_deref().unwrap_or_default())
    } else {
        app_url
    }
}

fn normalize_public_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

fn is_official_cloud_daemon_config(app_url: &str) -> bool {
    url_host_equals(app_url, "cordy.ai")
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
    fn canonical_hosts_match_go_cloud_detection() {
        for raw in [
            "https://cordy.ai",
            "cordy.ai",
            "cordy.ai:8080",
            "https://cordy.ai.",
        ] {
            assert!(url_host_equals(raw, "cordy.ai"), "{raw}");
        }
        assert!(!url_host_equals("https://evil.example", "cordy.ai"));
        assert!(!url_host_equals("", "cordy.ai"));
    }

    #[test]
    fn workspace_creation_flag_matches_go_exactly() {
        assert!(workspace_creation_disabled_value(Some("true")));
        for value in [None, Some("TRUE"), Some("1"), Some("yes"), Some(" true ")] {
            assert!(!workspace_creation_disabled_value(value));
        }
    }

    #[test]
    fn loaded_config_drives_public_auth_and_daemon_urls() {
        let mut config = cordy_config::Config::default();
        config.auth.allow_signup = Some(" false ".into());
        config.auth.google_client_id = Some(" google-client ".into());
        config.urls.public_url = Some("https://api.example/".into());
        config.urls.app_url = Some("https://app.example/".into());
        let settings = PublicConfigSettings::from_config(
            &config,
            String::new(),
            false,
            "v1".into(),
        );
        assert!(!settings.allow_signup);
        assert_eq!(settings.google_client_id, "google-client");
        assert_eq!(settings.daemon_server_url, "https://api.example");
        assert_eq!(settings.daemon_app_url, "https://app.example");
        assert!(!settings.official_cloud);
    }

    #[test]
    fn official_cloud_suppresses_daemon_urls_and_version() {
        let mut config = cordy_config::Config::default();
        config.urls.app_url = Some("https://cordy.ai".into());
        let settings = PublicConfigSettings::from_config(
            &config,
            String::new(),
            false,
            "v1".into(),
        );
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
            google_client_id: String::new(),
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
    }

    #[tokio::test]
    async fn public_route_is_available_before_authentication() {
        let state = HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            cordy_auth::pat_cache::PatCache::disabled(),
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
}
