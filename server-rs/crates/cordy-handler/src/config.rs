//! Public runtime configuration consumed before authentication.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use cordy_service::feature_flags::{self, FlagSource};
use serde::Serialize;
use std::collections::HashMap;
use url::Url;

use crate::state::HandlerState;

#[derive(Clone, Debug, Default)]
pub struct PublicConfigSettings {
    pub cdn_domain: String,
    pub cdn_signed: bool,
    pub server_version: String,
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

async fn get_config(State(state): State<HandlerState>) -> Json<AppConfig> {
    let (daemon_server_url, daemon_app_url) = daemon_setup_urls_from_env();
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
        allow_signup: state.auth_settings.public_signup_allowed(),
        google_client_id: state.auth_settings.google_client_id().to_string(),
        workspace_creation_disabled: std::env::var("DISABLE_WORKSPACE_CREATION").as_deref()
            == Ok("true"),
        daemon_server_url,
        daemon_app_url,
        vcs_integration_available: state.vcs_integration_enabled,
        posthog_key,
        posthog_host,
        analytics_environment,
        feature_flags: feature_flags::evaluate_frontend_public_flags(flags),
        local_worktree_supported: true,
        server_version: if is_official_cloud_deployment() {
            String::new()
        } else {
            state.public_config.server_version.clone()
        },
    })
}

fn daemon_setup_urls_from_env() -> (String, String) {
    let mut server_url =
        normalize_public_url(&std::env::var("CORDY_PUBLIC_URL").unwrap_or_default());
    let app_url = resolve_frontend_app_url();
    if app_url.is_empty() || is_official_cloud_daemon_config(&app_url) {
        return (String::new(), String::new());
    }
    if server_url.is_empty() {
        server_url.clone_from(&app_url);
    }
    (server_url, app_url)
}

fn resolve_frontend_app_url() -> String {
    let app_url = normalize_public_url(&std::env::var("CORDY_APP_URL").unwrap_or_default());
    if app_url.is_empty() {
        normalize_public_url(&std::env::var("FRONTEND_ORIGIN").unwrap_or_default())
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

fn is_official_cloud_deployment() -> bool {
    is_official_cloud_daemon_config(&resolve_frontend_app_url())
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
        let mut loaded = cordy_config::Config::default();
        loaded.auth.allow_signup = Some("false".into());
        loaded.auth.google_client_id = Some("toml-client-id".into());
        let state = HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        )
        .with_auth_settings(crate::auth::AuthSettings::from_config(&loaded));
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
        assert_eq!(value["allow_signup"], false);
        assert_eq!(value["google_client_id"], "toml-client-id");
    }
}
