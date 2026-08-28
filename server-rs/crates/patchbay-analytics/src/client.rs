//! Client surface.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Free-form event attribute bag (Go `map[string]any`).
pub type Props = serde_json::Map<String, serde_json::Value>;

/// A single analytics capture. Fields mirror PostHog's /capture/ shape but are
/// framework-agnostic so alternate backends can plug in later.
#[derive(Debug, Clone, Default)]
pub struct Event {
    /// Name of the event (e.g. "signup", "workspace_created").
    pub name: String,

    /// Identifies the person this event belongs to. For logged-in users this
    /// is user.id; for anonymous events it should be the anon_id previously
    /// used on the frontend so identity merging works.
    pub distinct_id: String,

    /// Scopes the event to a workspace. Required for workspace-level actions;
    /// empty allowed for pre-workspace events (signup).
    pub workspace_id: String,

    /// Free-form bag of event attributes. Never put raw PII like full emails
    /// here — use email_domain.
    pub properties: Option<Props>,

    /// Person properties written only the first time they appear — acquisition
    /// attribution (initial_utm_source, etc.) so later events don't overwrite
    /// the origin.
    pub set_once: Option<Props>,

    /// Person properties that overwrite on every write — mutable cohort
    /// signals (role, use_case, platform_preference).
    pub set: Option<Props>,

    /// Optional; when None the client fills in now().
    pub timestamp: Option<DateTime<Utc>>,
}

/// The narrow surface the rest of the codebase depends on. Handlers call
/// [`AnalyticsClient::capture`] and move on; the implementation is responsible
/// for buffering, batching, and shipping.
#[async_trait]
pub trait AnalyticsClient: Send + Sync {
    /// Enqueues an event. Returns immediately; on a full queue the event is
    /// dropped and counted. Must never block a request handler.
    fn capture(&self, event: Event);

    /// Drains pending events. Call once during graceful shutdown.
    async fn close(&self);
}

pub const DEFAULT_POSTHOG_HOST: &str = "https://us.i.posthog.com";

/// Returns a client configured from environment variables:
///
/// - `POSTHOG_API_KEY`: project API key. Empty → no-op client.
/// - `POSTHOG_HOST`:    API host (default <https://us.i.posthog.com>).
/// - `ANALYTICS_ENVIRONMENT`: production/staging/dev. Defaults from `APP_ENV`.
/// - `ANALYTICS_DISABLED`: "true"/"1" forces a no-op client even when an API
///   key is set (CI and self-hosted opt-out).
pub fn new_from_env() -> Box<dyn AnalyticsClient> {
    if is_disabled() {
        tracing::info!("analytics disabled via ANALYTICS_DISABLED");
        return Box::new(NoopClient);
    }
    let key = std::env::var("POSTHOG_API_KEY").unwrap_or_default();
    if key.is_empty() {
        tracing::info!("analytics: POSTHOG_API_KEY not set, using noop client");
        return Box::new(NoopClient);
    }
    let host = std::env::var("POSTHOG_HOST")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| DEFAULT_POSTHOG_HOST.to_string());
    tracing::info!(host = %host, "analytics: posthog client enabled");
    Box::new(crate::posthog::PostHogClient::new(
        crate::posthog::PostHogConfig {
            api_key: key,
            host,
            environment: environment_from_env(),
            ..crate::posthog::PostHogConfig::default()
        },
    ))
}

fn is_disabled() -> bool {
    matches!(
        std::env::var("ANALYTICS_DISABLED").as_deref(),
        Ok("true") | Ok("1")
    )
}

/// ANALYTICS_ENVIRONMENT, falling back to APP_ENV, falling back to "dev".
pub fn environment_from_env() -> String {
    for key in ["ANALYTICS_ENVIRONMENT", "APP_ENV"] {
        if let Ok(v) = std::env::var(key) {
            let normalized = normalize_environment(&v);
            if !normalized.is_empty() {
                return normalized.to_string();
            }
        }
    }
    "dev".to_string()
}

/// production/prod → production; staging/stage → staging;
/// development/dev/test/local → dev; anything else → "" (unset).
pub fn normalize_environment(v: &str) -> &'static str {
    match v.trim().to_lowercase().as_str() {
        "production" | "prod" => "production",
        "staging" | "stage" => "staging",
        "development" | "dev" | "test" | "local" => "dev",
        _ => "",
    }
}

/// Silently drops all events. Used in tests, in local dev when POSTHOG_API_KEY
/// is unset, and in self-hosted instances that opt out.
pub struct NoopClient;

#[async_trait]
impl AnalyticsClient for NoopClient {
    fn capture(&self, _event: Event) {}
    async fn close(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_normalization_table() {
        assert_eq!(normalize_environment("production"), "production");
        assert_eq!(normalize_environment(" Prod "), "production");
        assert_eq!(normalize_environment("stage"), "staging");
        assert_eq!(normalize_environment("TEST"), "dev");
        assert_eq!(normalize_environment("local"), "dev");
        assert_eq!(normalize_environment(""), "");
        assert_eq!(normalize_environment("bogus"), "");
    }
}
