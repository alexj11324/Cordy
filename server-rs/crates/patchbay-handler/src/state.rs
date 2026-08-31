//! Shared handler state — the Rust analogue of the Go `Handler` struct's
//! DB/redis wiring. Domain services are added per-slice as routes land.

use std::sync::Arc;

use patchbay_auth::daemon_token_cache::DaemonTokenCache;
use patchbay_auth::membership_cache::MembershipCache;
use patchbay_auth::pat_cache::PatCache;
use patchbay_realtime::hub::Hub;
use patchbay_service::automation::{AutomationService, EntitlementProvider};
use patchbay_service::email::EmailService;
use patchbay_service::issue_service::IssueService;
use patchbay_service::plugin::PluginService;
use patchbay_service::plugin_event_dispatch::PluginEventDispatcher;
use patchbay_service::plugin_token::CallbackTokens;
use patchbay_service::task_service::TaskService;

/// One replaceable client shared by handler assists and TaskService quick
/// actions. Builder-time configuration happens after domain services already
/// hold their `Arc`s, so the indirection keeps both consumers on one policy.
pub struct HandlerAssistLlm {
    client: std::sync::RwLock<Arc<patchbay_llm::Client>>,
}

impl HandlerAssistLlm {
    fn new(client: patchbay_llm::Client) -> Self {
        Self {
            client: std::sync::RwLock::new(Arc::new(client)),
        }
    }

    pub fn client(&self) -> Arc<patchbay_llm::Client> {
        self.client
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn replace(&self, client: Arc<patchbay_llm::Client>) {
        *self
            .client
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = client;
    }
}

#[async_trait::async_trait]
impl patchbay_service::task_service::ChatQuickActionsLlm for HandlerAssistLlm {
    fn enabled(&self) -> bool {
        self.client().enabled()
    }

    async fn generate_json(
        &self,
        model: &str,
        system_prompt: &str,
        user_prompt: &str,
        temperature: f64,
        max_completion_tokens: i64,
    ) -> anyhow::Result<String> {
        Ok(self
            .client()
            .generate_json(
                model,
                system_prompt,
                user_prompt,
                temperature,
                max_completion_tokens,
            )
            .await?)
    }
}

struct SharedAnalyticsClient(Arc<dyn patchbay_analytics::AnalyticsClient>);

#[async_trait::async_trait]
impl patchbay_analytics::AnalyticsClient for SharedAnalyticsClient {
    fn capture(&self, event: patchbay_analytics::Event) {
        self.0.capture(event);
    }

    async fn close(&self) {
        self.0.close().await;
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AttachmentDownloadMode {
    #[default]
    Auto,
    CloudFront,
    Presign,
    Proxy,
}

#[derive(Clone)]
pub struct AttachmentDownloadSettings {
    pub mode: AttachmentDownloadMode,
    pub public_url: String,
    pub ttl: std::time::Duration,
    pub cloudfront_signer: Option<Arc<crate::cloudfront::CloudFrontSigner>>,
}

impl Default for AttachmentDownloadSettings {
    fn default() -> Self {
        Self {
            mode: AttachmentDownloadMode::Auto,
            public_url: String::new(),
            ttl: std::time::Duration::from_secs(30 * 60),
            cloudfront_signer: None,
        }
    }
}

impl AttachmentDownloadSettings {
    pub async fn from_config(config: &patchbay_config::Config) -> anyhow::Result<Self> {
        let raw_mode = config
            .storage
            .attachment_download_mode
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let mode = match raw_mode.as_str() {
            "" | "auto" => AttachmentDownloadMode::Auto,
            "cloudfront" => AttachmentDownloadMode::CloudFront,
            "presign" => AttachmentDownloadMode::Presign,
            "proxy" => AttachmentDownloadMode::Proxy,
            _ => {
                tracing::warn!(
                    value = raw_mode,
                    "invalid ATTACHMENT_DOWNLOAD_MODE; using auto"
                );
                AttachmentDownloadMode::Auto
            }
        };
        let ttl = match config
            .storage
            .attachment_download_url_ttl
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(raw) => patchbay_auth::cookie::parse_auth_token_ttl(raw).ok_or_else(|| {
                anyhow::anyhow!("ATTACHMENT_DOWNLOAD_URL_TTL must be a positive Go duration")
            })?,
            None => std::time::Duration::from_secs(30 * 60),
        };
        chrono::Duration::from_std(ttl)
            .map_err(|_| anyhow::anyhow!("ATTACHMENT_DOWNLOAD_URL_TTL is too large"))?;
        let cloudfront_signer = if matches!(
            mode,
            AttachmentDownloadMode::Auto | AttachmentDownloadMode::CloudFront
        ) {
            crate::cloudfront::CloudFrontSigner::from_config(config)
                .await?
                .map(Arc::new)
        } else {
            None
        };
        anyhow::ensure!(
            mode != AttachmentDownloadMode::CloudFront || cloudfront_signer.is_some(),
            "ATTACHMENT_DOWNLOAD_MODE=cloudfront requires a usable CloudFront signing key"
        );
        Ok(Self {
            mode,
            public_url: config
                .urls
                .public_url
                .as_deref()
                .unwrap_or_default()
                .trim()
                .trim_end_matches('/')
                .to_string(),
            ttl,
            cloudfront_signer,
        })
    }

    pub fn validate_for_storage(
        &self,
        storage: &dyn crate::attachment_storage::AttachmentStorage,
    ) -> anyhow::Result<()> {
        self.validate_presign_support(storage.supports_presigned_downloads())
    }

    fn validate_presign_support(&self, supports_presigned_downloads: bool) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.mode != AttachmentDownloadMode::Presign || supports_presigned_downloads,
            "ATTACHMENT_DOWNLOAD_MODE=presign requires S3 attachment storage"
        );
        if supports_presigned_downloads
            && matches!(
                self.mode,
                AttachmentDownloadMode::Auto | AttachmentDownloadMode::Presign
            )
        {
            crate::attachment_storage::validate_s3_presign_ttl(self.ttl).map_err(|_| {
                anyhow::anyhow!(
                    "ATTACHMENT_DOWNLOAD_URL_TTL must be between 1 second and 7 days when S3 presigned downloads are enabled"
                )
            })?;
        }
        Ok(())
    }

    pub fn resolve_mode(
        &self,
        storage: Option<&dyn crate::attachment_storage::AttachmentStorage>,
        raw_url: &str,
    ) -> AttachmentDownloadMode {
        match self.mode {
            AttachmentDownloadMode::CloudFront => return AttachmentDownloadMode::CloudFront,
            AttachmentDownloadMode::Presign => return AttachmentDownloadMode::Presign,
            AttachmentDownloadMode::Proxy => return AttachmentDownloadMode::Proxy,
            AttachmentDownloadMode::Auto => {}
        }
        if self
            .cloudfront_signer
            .as_deref()
            .is_some_and(|signer| signer.can_sign_url(raw_url))
        {
            return AttachmentDownloadMode::CloudFront;
        }
        if should_proxy_attachment_url(raw_url) {
            return AttachmentDownloadMode::Proxy;
        }
        if storage.is_some_and(|storage| storage.supports_presigned_downloads()) {
            return AttachmentDownloadMode::Presign;
        }
        AttachmentDownloadMode::Proxy
    }
}

#[cfg(test)]
mod attachment_download_tests {
    use std::time::Duration;

    use super::{should_proxy_attachment_url, AttachmentDownloadMode, AttachmentDownloadSettings};

    #[test]
    fn auto_mode_keeps_internal_object_urls_on_the_proxy() {
        for url in [
            "http://localhost:9000/bucket/object",
            "https://objects.internal/object",
            "https://10.0.0.8/object",
            "https://[::1]/object",
            "/uploads/object",
        ] {
            assert!(should_proxy_attachment_url(url), "{url}");
        }
        assert!(!should_proxy_attachment_url(
            "https://bucket.s3.us-west-2.amazonaws.com/object"
        ));
    }

    #[test]
    fn explicit_presign_requires_a_presigning_storage_backend() {
        let settings = AttachmentDownloadSettings {
            mode: AttachmentDownloadMode::Presign,
            ..AttachmentDownloadSettings::default()
        };

        let error = settings.validate_presign_support(false).unwrap_err();
        assert_eq!(
            error.to_string(),
            "ATTACHMENT_DOWNLOAD_MODE=presign requires S3 attachment storage"
        );
    }

    #[test]
    fn presign_ttl_is_validated_at_startup_for_auto_and_explicit_modes() {
        for mode in [
            AttachmentDownloadMode::Auto,
            AttachmentDownloadMode::Presign,
        ] {
            for ttl in [
                Duration::from_millis(500),
                Duration::from_secs(7 * 24 * 60 * 60 + 1),
            ] {
                let settings = AttachmentDownloadSettings {
                    mode,
                    ttl,
                    ..AttachmentDownloadSettings::default()
                };
                assert!(settings.validate_presign_support(true).is_err());
            }
        }
    }

    #[test]
    fn proxy_mode_does_not_apply_the_s3_presign_ttl_limit() {
        let settings = AttachmentDownloadSettings {
            mode: AttachmentDownloadMode::Proxy,
            ttl: Duration::from_millis(500),
            ..AttachmentDownloadSettings::default()
        };

        settings.validate_presign_support(false).unwrap();
    }

    #[tokio::test]
    async fn explicit_non_cloudfront_modes_do_not_initialize_cloudfront() {
        for mode in ["proxy", "presign"] {
            let mut config = patchbay_config::Config::default();
            config.storage.attachment_download_mode = Some(mode.to_string());
            config.storage.cloudfront_key_pair_id = Some("configured-but-unused".to_string());

            let settings = AttachmentDownloadSettings::from_config(&config)
                .await
                .expect("unused CloudFront configuration must not block startup");

            assert_eq!(
                settings.mode,
                if mode == "proxy" {
                    AttachmentDownloadMode::Proxy
                } else {
                    AttachmentDownloadMode::Presign
                }
            );
            assert!(settings.cloudfront_signer.is_none());
        }
    }
}

fn should_proxy_attachment_url(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else {
        return true;
    };
    let Some(host) = url.host_str() else {
        return true;
    };
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty()
        || host == "localhost"
        || host.ends_with(".localhost")
        || !host.contains('.')
        || [
            ".local",
            ".localdomain",
            ".internal",
            ".lan",
            ".home",
            ".docker",
        ]
        .iter()
        .any(|suffix| host.ends_with(suffix))
    {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .is_ok_and(|address| match address {
            std::net::IpAddr::V4(address) => {
                address.is_loopback()
                    || address.is_private()
                    || address.is_link_local()
                    || address.is_multicast()
                    || address.is_unspecified()
            }
            std::net::IpAddr::V6(address) => {
                address.is_loopback()
                    || address.is_unique_local()
                    || address.is_unicast_link_local()
                    || address.is_multicast()
                    || address.is_unspecified()
            }
        })
}

struct DaemonTaskWakeup {
    notifier: Arc<patchbay_daemon::notifier::RelayNotifier>,
}

struct DaemonMessageMetrics {
    metrics: Arc<patchbay_metrics::BusinessMetrics>,
}

impl patchbay_daemon::hub::MessageKindRecorder for DaemonMessageMetrics {
    fn record_daemon_ws_message_received(&self, kind: &str) {
        self.metrics.record_daemon_ws_message_received(kind);
    }
}

#[async_trait::async_trait]
impl patchbay_service::task_service::TaskWakeupNotifier for DaemonTaskWakeup {
    async fn notify_task_available(&self, runtime_id: &str, task_id: &str) {
        self.notifier
            .notify_task_available(runtime_id, task_id)
            .await;
    }
}

/// Handler-layer state shared by all axum extractors.
#[derive(Clone)]
pub struct HandlerState {
    pub pool: sqlx::PgPool,
    /// Shared enforcement interface. Consumers must fail closed on store or
    /// audit errors; UI checks are never an authorization boundary.
    pub authorization: Arc<dyn patchbay_authorization::Authorizer>,
    pub pat_cache: PatCache,
    pub daemon_token_cache: DaemonTokenCache,
    pub membership_cache: MembershipCache,
    pub cloud_pat_verifier: Option<patchbay_auth::cloud_pat::CloudPatVerifier>,
    /// Loaded Fleet base URL used by the cloud-runtime HTTP proxy. Empty means
    /// fall back to `PATCHBAY_CLOUD_FLEET_URL` / `PATCHBAY_FLEET_URL` at router build.
    pub cloud_runtime_base_url: String,
    /// Realtime WS hub (patchbay-realtime). `None` only in tests.
    pub hub: Option<Arc<Hub>>,
    /// Event bus (Go h.Bus) for workspace-scoped WS fanout.
    pub bus: Arc<patchbay_events::Bus>,
    /// Owned background work started by channel HTTP/event surfaces. The
    /// production ChannelRuntime closes admission and joins/aborts this group
    /// during shutdown.
    pub channel_tasks: Arc<patchbay_channel::RuntimeTasks>,
    pub channel_cancel: tokio_util::sync::CancellationToken,
    /// Prometheus business counters. None when METRICS_ADDR is disabled.
    pub business_metrics: Option<Arc<patchbay_metrics::BusinessMetrics>>,
    /// HTTP request metrics. None when METRICS_ADDR is disabled.
    pub http_metrics: Option<Arc<patchbay_metrics::HttpMetrics>>,
    pub heartbeat_scheduler: Arc<dyn crate::heartbeat_scheduler::HeartbeatScheduler>,
    pub liveness_store: Arc<dyn crate::runtime_liveness::LivenessStore>,
    /// Public authentication dependencies and boot-time policy.
    pub auth_settings: crate::auth::AuthSettings,
    /// Clerk session verifier used only by the public session-exchange route.
    /// None keeps self-hosted deployments that do not use Clerk unchanged.
    pub clerk_auth: Option<Arc<dyn crate::clerk_auth::ClerkSessionVerifier>>,
    pub email_service: Arc<EmailService>,
    pub analytics: Arc<dyn patchbay_analytics::AnalyticsClient>,
    pub auth_rate_limit: patchbay_middleware::ratelimit::RateLimitState,
    pub auth_verify_rate_limit: patchbay_middleware::ratelimit::RateLimitState,
    pub invitation_admission: crate::invitation::InvitationAdmission,
    /// Anonymous frontend capability/configuration response.
    pub public_config: crate::config::PublicConfigSettings,
    /// Immutable integration endpoint configuration loaded once at boot.
    /// Channel install flows use this same snapshot as the runtime connectors
    /// instead of re-reading process environment mid-session.
    pub integrations: patchbay_config::IntegrationsConfig,
    /// GitHub GraphQL snapshot refresh pipeline. Disabled in lightweight tests.
    pub github_snapshots: Arc<patchbay_ghsnapshot::Manager>,
    /// Feature flag source. `None` fails closed for rollout-gated writes.
    pub feature_flags: Option<Arc<dyn patchbay_service::feature_flags::FlagSource>>,
    /// Shared Composio service used by both HTTP connection management and
    /// task overlay generation. None when its flag/configuration is disabled.
    pub composio: Option<Arc<patchbay_composio::Service>>,
    /// Task domain service (Go h.TaskService).
    pub tasks: Arc<TaskService>,
    /// Durable task-completion/review-return coordinator. Its PostgreSQL
    /// outbox is authoritative; the worker is started only by production.
    pub coordinator: Arc<patchbay_service::coordination::CoordinatorService>,
    /// Shared Automation service. It must be reused by HTTP paths and durable
    /// workers so entitlement/quota configuration cannot disappear per request.
    pub automations: Arc<AutomationService>,
    /// Issue domain service (Go h.IssueService).
    pub issues: Arc<IssueService>,
    /// Plugin service (Go h.PluginService).
    pub plugins: Arc<PluginService>,
    /// Hook callback token store; None disables callback tokens (fail-closed).
    pub callbacks: Option<Arc<CallbackTokens>>,
    /// One-time PKCE-bound desktop login handoff codes. Redis is used when
    /// configured; single-node deployments intentionally use process memory.
    pub desktop_handoff_tokens: crate::desktop_handoff::DesktopHandoffTokens,
    /// Absolute base URL used in hook callback_url; empty omits the field.
    pub callback_base_url: String,
    /// Production event-hook workers and their bus subscriptions. `None` in
    /// lightweight tests and before production side effects are started.
    plugin_events: Option<Arc<PluginEventDispatcher>>,
    /// Owned Automation issue/task terminal listener set.
    automation_event_listeners: Option<Arc<crate::automation_listeners::AutomationEventListeners>>,
    /// Ordered subscriber → activity → notification pipeline. The bus retains
    /// its callback; this field guards registration and exposes lifecycle.
    ordered_event_side_effects:
        Option<Arc<crate::ordered_event_side_effects::OrderedEventSideEffects>>,
    /// Boot-time bearer token for `/health/realtime`. Empty enables the
    /// direct-loopback-only development policy.
    pub realtime_metrics_token: String,
    /// Pending request stores (update / model list / local skills). Production
    /// uses Redis when configured and process-local stores for single-node
    /// deployments that intentionally omit Redis. `None` is reserved for
    /// invalid Redis configuration, which fails closed like Go.
    pub update_store: Option<Arc<dyn crate::pending_store::UpdateStoreBackend>>,
    pub model_list_store: Option<Arc<dyn crate::pending_store::ModelListStoreBackend>>,
    pub model_catalog_cache: Option<Arc<dyn crate::pending_store::ModelCatalogCacheBackend>>,
    pub webhook_rate_limits: crate::webhook_rate_limit::WebhookRateLimits,
    pub local_skill_list_store: Option<Arc<dyn crate::pending_store::LocalSkillListStoreBackend>>,
    pub local_skill_import_store:
        Option<Arc<dyn crate::pending_store::LocalSkillImportStoreBackend>>,
    /// Shared Redis connection for per-IP public-route rate limiting. None is
    /// the Go nil-client path and deliberately fails open.
    pub rate_limit_client: Option<redis::Client>,
    /// Public VCS webhook gate and at-rest secret decryptor. The feature is
    /// deliberately invisible when disabled and 503s when enabled without a
    /// usable key, matching Go's deployment boundary.
    pub vcs_integration_enabled: bool,
    pub vcs_secret_box: Option<patchbay_util::secretbox::SecretBox>,
    /// Linear installation foundation gate and the master key used to
    /// encrypt OAuth access/refresh tokens at rest.
    pub linear_integration_enabled: bool,
    pub linear_secret_box: Option<patchbay_util::secretbox::SecretBox>,
    /// Daemon WebSocket hub (patchbay-daemon). `None` only in tests — the WS
    /// endpoint reports 503 and daemons fall back to HTTP polling.
    pub daemon_hub: Option<Arc<patchbay_daemon::hub::DaemonHub>>,
    /// Local-first daemon wakeup publisher. Production runtime installs the
    /// shared Redis relay for sharded/dual modes before the router is served.
    pub daemon_notifier: Arc<patchbay_daemon::notifier::RelayNotifier>,
    /// Attachment object store. None is the explicit unconfigured test path.
    pub attachment_storage: Option<Arc<dyn crate::attachment_storage::AttachmentStorage>>,
    /// Production Lark/WeCom media adapter over the same object store.
    pub channel_media_storage: Option<Arc<crate::attachment_storage::ChannelMediaStorage>>,
    pub attachment_frame_ancestors: Vec<String>,
    pub attachment_download: AttachmentDownloadSettings,
    /// On-demand Slack channel history reader. `None` means Slack history is
    /// not configured; chat history then falls back to the persisted transcript.
    pub slack_history: Option<Arc<patchbay_slack::history::History>>,
    /// Server-internal assist LLM. An unconfigured client is deliberately
    /// inert and guarantees that private chat content produces no egress.
    pub llm: Arc<HandlerAssistLlm>,
    /// Low-latency hint for the durable webhook worker. PostgreSQL polling is
    /// authoritative and recovers missed notifications or process restarts.
    webhook_delivery_notify: Option<Arc<tokio::sync::Notify>>,
    /// Low-latency hint for the Linear Inbox worker. PostgreSQL leasing is
    /// authoritative; the worker remains disabled unless both the feature
    /// flag and the workspace canary allowlist are enabled.
    linear_sync_notify: Option<Arc<tokio::sync::Notify>>,
    /// Keeps the weak notifier installed in `TaskService` alive.
    _task_wakeup: Arc<dyn patchbay_service::task_service::TaskWakeupNotifier>,
}

impl HandlerState {
    pub fn new(pool: sqlx::PgPool, pat_cache: PatCache, hub: Option<Arc<Hub>>) -> Self {
        Self::new_with_analytics(
            pool,
            pat_cache,
            hub,
            Arc::new(patchbay_analytics::NoopClient),
        )
    }

    pub fn new_with_analytics(
        pool: sqlx::PgPool,
        pat_cache: PatCache,
        hub: Option<Arc<Hub>>,
        analytics: Arc<dyn patchbay_analytics::AnalyticsClient>,
    ) -> Self {
        Self::new_with_dependencies(pool, pat_cache, hub, analytics, None, None, None)
    }

    pub fn new_with_production_dependencies(
        pool: sqlx::PgPool,
        pat_cache: PatCache,
        hub: Option<Arc<Hub>>,
        analytics: Arc<dyn patchbay_analytics::AnalyticsClient>,
        feature_flags: Arc<dyn patchbay_service::feature_flags::FlagSource>,
        business_metrics: Option<Arc<patchbay_metrics::BusinessMetrics>>,
    ) -> Self {
        let composio =
            if patchbay_service::feature_flags::composio_mcp_apps_enabled(feature_flags.as_ref()) {
                crate::composio::build_service(pool.clone())
                    .ok()
                    .map(Arc::new)
            } else {
                None
            };
        Self::new_with_dependencies(
            pool,
            pat_cache,
            hub,
            analytics,
            Some(feature_flags),
            composio,
            business_metrics,
        )
    }

    fn new_with_dependencies(
        pool: sqlx::PgPool,
        pat_cache: PatCache,
        hub: Option<Arc<Hub>>,
        analytics: Arc<dyn patchbay_analytics::AnalyticsClient>,
        feature_flags: Option<Arc<dyn patchbay_service::feature_flags::FlagSource>>,
        composio: Option<Arc<patchbay_composio::Service>>,
        business_metrics: Option<Arc<patchbay_metrics::BusinessMetrics>>,
    ) -> Self {
        let authorization: Arc<dyn patchbay_authorization::Authorizer> = Arc::new(
            patchbay_authorization::PostgresAuthorizer::new(pool.clone()),
        );
        let bus = Arc::new(patchbay_events::Bus::new());
        let daemon_hub = Arc::new(patchbay_daemon::hub::DaemonHub::new());
        let daemon_notifier = Arc::new(patchbay_daemon::notifier::RelayNotifier::new(
            Some(daemon_hub.clone()),
            None,
        ));
        let task_wakeup: Arc<dyn patchbay_service::task_service::TaskWakeupNotifier> =
            Arc::new(DaemonTaskWakeup {
                notifier: daemon_notifier.clone(),
            });
        let llm = Arc::new(HandlerAssistLlm::new(patchbay_llm::Client::new(
            patchbay_llm::Config::default(),
        )));
        let mut task_service = TaskService::new(pool.clone(), bus.clone());
        task_service.analytics = Some(Box::new(SharedAnalyticsClient(analytics.clone())));
        task_service.metrics = business_metrics.clone();
        task_service.wakeup = Some(Arc::downgrade(&task_wakeup));
        task_service.quick_actions = Some(llm.clone());
        task_service.feature_flags = feature_flags.clone();
        // Phase 1 fails closed for Agent credential delegation. The current
        // Composio overlay embeds the platform's long-lived API key in runtime
        // configuration and is built before a task lease exists, so it cannot
        // satisfy credential.use intersection. Human connection management
        // remains available; task overlays stay off until a short-lived
        // broker consumes the capability lease at dispatch.
        task_service.set_composio_overlay(None);
        let tasks = Arc::new(task_service);
        let coordinator = patchbay_service::coordination::CoordinatorService::new(
            pool.clone(),
            tasks.clone(),
            bus.clone(),
        );
        let automations = Arc::new(AutomationService::new(
            pool.clone(),
            bus.clone(),
            tasks.clone(),
        ));
        let mut issue_service = IssueService::new(pool.clone(), bus.clone(), tasks.clone());
        issue_service.analytics = Some(Box::new(SharedAnalyticsClient(analytics.clone())));
        issue_service.metrics = business_metrics.clone();
        let issues = Arc::new(issue_service);
        let plugins = Arc::new(PluginService::with_pool(pool.clone()));
        let heartbeat_scheduler =
            Arc::new(crate::heartbeat_scheduler::PassthroughHeartbeatScheduler::new(pool.clone()));
        let trusted_proxies = patchbay_middleware::ratelimit::parse_trusted_proxies(
            &std::env::var("RATE_LIMIT_TRUSTED_PROXIES").unwrap_or_default(),
        );
        let mut auth_rate_limit = patchbay_middleware::ratelimit::RateLimitState::disabled(
            positive_env_i64("RATE_LIMIT_AUTH", 5),
            60,
        );
        auth_rate_limit.trusted_proxies = trusted_proxies.clone();
        let mut auth_verify_rate_limit = patchbay_middleware::ratelimit::RateLimitState::disabled(
            positive_env_i64("RATE_LIMIT_AUTH_VERIFY", 20),
            60,
        );
        auth_verify_rate_limit.trusted_proxies = trusted_proxies;
        Self {
            pool,
            authorization,
            pat_cache,
            daemon_token_cache: DaemonTokenCache::disabled(),
            membership_cache: MembershipCache::disabled(),
            cloud_pat_verifier: None,
            cloud_runtime_base_url: String::new(),
            hub,
            bus,
            channel_tasks: Arc::new(patchbay_channel::RuntimeTasks::new()),
            channel_cancel: tokio_util::sync::CancellationToken::new(),
            business_metrics,
            http_metrics: None,
            heartbeat_scheduler,
            liveness_store: Arc::new(crate::runtime_liveness::NoopLivenessStore),
            auth_settings: crate::auth::AuthSettings::from_env(),
            clerk_auth: None,
            email_service: Arc::new(EmailService::new()),
            analytics,
            auth_rate_limit,
            auth_verify_rate_limit,
            invitation_admission: crate::invitation::InvitationAdmission::default(),
            public_config: crate::config::PublicConfigSettings::default(),
            integrations: patchbay_config::IntegrationsConfig::default(),
            github_snapshots: Arc::new(patchbay_ghsnapshot::Manager::new(None, None, None)),
            feature_flags,
            composio,
            tasks,
            coordinator,
            automations,
            issues,
            plugins,
            callbacks: Some(Arc::new(CallbackTokens::new())),
            desktop_handoff_tokens: crate::desktop_handoff::DesktopHandoffTokens::new(),
            callback_base_url: String::new(),
            plugin_events: None,
            automation_event_listeners: None,
            ordered_event_side_effects: None,
            realtime_metrics_token: std::env::var("REALTIME_METRICS_TOKEN")
                .unwrap_or_default()
                .trim()
                .to_string(),
            update_store: None,
            model_list_store: None,
            model_catalog_cache: None,
            webhook_rate_limits: crate::webhook_rate_limit::WebhookRateLimits::default(),
            local_skill_list_store: None,
            local_skill_import_store: None,
            rate_limit_client: None,
            vcs_integration_enabled: false,
            vcs_secret_box: None,
            linear_integration_enabled: false,
            linear_secret_box: None,
            daemon_hub: Some(daemon_hub),
            daemon_notifier,
            attachment_storage: None,
            channel_media_storage: None,
            attachment_frame_ancestors: Vec::new(),
            attachment_download: AttachmentDownloadSettings::default(),
            slack_history: None,
            llm,
            webhook_delivery_notify: None,
            linear_sync_notify: None,
            _task_wakeup: task_wakeup,
        }
    }

    /// Wires the internal OpenAI-compatible assist layer. Invalid retry
    /// budgets fail startup rather than silently selecting another policy.
    pub fn with_llm_from_config(self, llm: &patchbay_config::LlmConfig) -> anyhow::Result<Self> {
        const MAX_RETRIES: u32 = 5;
        let max_retries = match llm.max_retries {
            None => None,
            Some(parsed) => {
                anyhow::ensure!(
                    parsed <= MAX_RETRIES,
                    "PATCHBAY_LLM_MAX_RETRIES must be at most {MAX_RETRIES}, got {parsed}"
                );
                Some(parsed)
            }
        };
        let client = Arc::new(patchbay_llm::Client::new(patchbay_llm::Config {
            api_key: llm.api_key.clone().unwrap_or_default(),
            base_url: llm.base_url.clone().unwrap_or_default(),
            default_model: llm.default_model.clone().unwrap_or_default(),
            max_retries,
        }));
        self.llm.replace(client.clone());
        tracing::info!(
            enabled = client.enabled(),
            max_retries = client.max_retries(),
            default_model = client.default_model(),
            "llm assist policy"
        );
        Ok(self)
    }

    /// Wires the internal OpenAI-compatible assist layer from process env.
    /// Production startup prefers [`Self::with_llm_from_config`].
    pub fn with_llm_from_env(self) -> anyhow::Result<Self> {
        const MAX_RETRIES: u32 = 5;
        let raw_retries = std::env::var("PATCHBAY_LLM_MAX_RETRIES").unwrap_or_default();
        let max_retries = if raw_retries.trim().is_empty() {
            None
        } else {
            let parsed = raw_retries.trim().parse::<u32>().map_err(|_| {
                anyhow::anyhow!(
                    "PATCHBAY_LLM_MAX_RETRIES must be an integer from 0 to {MAX_RETRIES}, got {:?}",
                    raw_retries.trim()
                )
            })?;
            anyhow::ensure!(
                parsed <= MAX_RETRIES,
                "PATCHBAY_LLM_MAX_RETRIES must be at most {MAX_RETRIES}, got {parsed}"
            );
            Some(parsed)
        };
        self.with_llm_from_config(&patchbay_config::LlmConfig {
            api_key: Some(std::env::var("PATCHBAY_LLM_API_KEY").unwrap_or_default())
                .filter(|value| !value.is_empty()),
            base_url: Some(std::env::var("PATCHBAY_LLM_BASE_URL").unwrap_or_default())
                .filter(|value| !value.is_empty()),
            default_model: Some(std::env::var("PATCHBAY_LLM_DEFAULT_MODEL").unwrap_or_default())
                .filter(|value| !value.is_empty()),
            max_retries,
        })
    }

    /// Wires the S7 Slack history service with the same secretbox key used by
    /// channel installation credentials. Missing or invalid keys leave the
    /// reader disabled instead of interpreting ciphertext as plaintext.
    pub fn with_slack_history_from_env(mut self) -> Self {
        let Ok(key) = patchbay_util::secretbox::load_key("PATCHBAY_SLACK_SECRET_KEY") else {
            return self;
        };
        let Ok(secret_box) = patchbay_util::secretbox::SecretBox::new(&key) else {
            return self;
        };
        let decrypt: Arc<patchbay_slack::config::Decrypter> =
            Arc::new(move |sealed| secret_box.open(sealed).map_err(anyhow::Error::from));
        self.slack_history = Some(Arc::new(patchbay_slack::history::History::new(
            self.pool.clone(),
            Some(decrypt),
        )));
        self
    }

    pub fn with_attachment_storage(
        mut self,
        storage: Arc<dyn crate::attachment_storage::AttachmentStorage>,
        frame_ancestors: Vec<String>,
        download: AttachmentDownloadSettings,
    ) -> Self {
        self.channel_media_storage = Some(Arc::new(
            crate::attachment_storage::ChannelMediaStorage::new(storage.clone()),
        ));
        self.attachment_storage = Some(storage);
        self.attachment_frame_ancestors = frame_ancestors;
        self.attachment_download = download;
        self
    }

    pub fn with_public_config(mut self, settings: crate::config::PublicConfigSettings) -> Self {
        self.public_config = settings;
        self
    }

    pub fn with_integrations(mut self, integrations: patchbay_config::IntegrationsConfig) -> Self {
        self.integrations = integrations;
        self
    }

    /// Rebuilds the human Composio HTTP service from loaded config. Task
    /// overlays remain fail-closed until lease-bound credential brokering is
    /// available.
    /// TOML-only deployments no longer depend on process environment after
    /// `Config::load` has already merged env overrides into `config`.
    pub fn with_composio_from_config(mut self, config: &patchbay_config::Config) -> Self {
        self.integrations = config.integrations.clone();
        let enabled = self
            .feature_flags
            .as_deref()
            .is_some_and(patchbay_service::feature_flags::composio_mcp_apps_enabled);
        if !enabled {
            self.composio = None;
            self.tasks.set_composio_overlay(None);
            return self;
        }
        match crate::composio::build_service_from_config(self.pool.clone(), config) {
            Ok(service) => {
                let service = Arc::new(service);
                self.composio = Some(service);
                self.tasks.set_composio_overlay(None);
            }
            Err(error) => {
                tracing::warn!(%error, "composio disabled by incomplete configuration");
                self.composio = None;
                self.tasks.set_composio_overlay(None);
            }
        }
        self
    }

    /// Binds channel-originated tasks to the process cancellation tree. The
    /// owned ChannelRuntime still performs its ordered drain first; this child
    /// token is the fail-safe for startup errors and abnormal root shutdown.
    pub fn with_channel_cancel(mut self, cancel: tokio_util::sync::CancellationToken) -> Self {
        self.channel_cancel = cancel;
        self
    }

    pub fn with_feature_flags(
        mut self,
        flags: Arc<dyn patchbay_service::feature_flags::FlagSource>,
    ) -> Self {
        self.feature_flags = Some(flags);
        self
    }

    /// Replaces the lightweight test plugin service with production env
    /// wiring, including the encryption/signing key and callback URL.
    pub fn with_plugins_from_env(mut self) -> Self {
        let mut plugins = PluginService::new_from_env(self.pool.clone());
        if let Ok(key) = patchbay_util::secretbox::load_key("PATCHBAY_PLUGIN_SECRET_KEY") {
            plugins.secrets = patchbay_util::secretbox::SecretBox::new(&key).ok();
        }
        self.plugins = Arc::new(plugins);
        self.callback_base_url = std::env::var("PATCHBAY_PUBLIC_URL")
            .unwrap_or_default()
            .trim()
            .trim_end_matches('/')
            .to_string();
        if !self.callback_base_url.is_empty() {
            self.callback_base_url.push_str("/api/v1/plugin");
        }
        self
    }

    pub fn with_observability(
        mut self,
        business_metrics: Option<Arc<patchbay_metrics::BusinessMetrics>>,
        http_metrics: Option<Arc<patchbay_metrics::HttpMetrics>>,
    ) -> Self {
        if let (Some(hub), Some(metrics)) = (self.daemon_hub.as_ref(), business_metrics.as_ref()) {
            hub.set_message_kind_recorder(Some(Arc::new(DaemonMessageMetrics {
                metrics: metrics.clone(),
            })));
        }
        self.business_metrics = business_metrics;
        self.http_metrics = http_metrics;
        self
    }

    pub fn with_invitation_admission(
        mut self,
        invitation_admission: crate::invitation::InvitationAdmission,
    ) -> Self {
        self.invitation_admission = invitation_admission;
        self
    }

    pub fn with_heartbeat_scheduler(
        mut self,
        scheduler: Arc<dyn crate::heartbeat_scheduler::HeartbeatScheduler>,
    ) -> Self {
        self.heartbeat_scheduler = scheduler;
        self
    }

    pub fn with_liveness_redis(mut self, client: redis::Client) -> Self {
        self.liveness_store = crate::runtime_liveness::RedisLivenessStore::new(
            patchbay_redis::RecoveringConnection::new(client),
        );
        self
    }

    /// Subscribes and starts event-triggered plugin hooks after plugin config
    /// and feature flags have both been installed. This is deliberately a
    /// production-startup step: constructing lightweight handler state must
    /// not spawn database or network workers.
    pub fn start_plugin_event_dispatcher(
        mut self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> (
        Self,
        Option<patchbay_service::plugin_event_dispatch::PluginEventDispatcherRuntime>,
    ) {
        if self.plugin_events.is_some() {
            return (self, None);
        }
        let Some(callbacks) = self.callbacks.clone() else {
            tracing::warn!("plugins: event hooks disabled because callback tokens are unavailable");
            return (self, None);
        };
        let dispatcher = Arc::new(PluginEventDispatcher::new(
            self.plugins.clone(),
            callbacks,
            self.callback_base_url.clone(),
            self.feature_flags.clone(),
        ));
        patchbay_service::plugin_event_dispatch::subscribe_plugin_events(
            self.bus.as_ref(),
            dispatcher.clone(),
        );
        let runtime = dispatcher.start(cancel);
        self.plugin_events = Some(dispatcher);
        (self, runtime)
    }

    /// Wires the issue/task terminal events that settle linked Automation runs.
    /// Lightweight state construction stays side-effect free; production calls
    /// this only after the shared Automation service has its final dependencies.
    pub fn start_automation_event_listeners(
        mut self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> (
        Self,
        Option<crate::automation_listeners::AutomationEventListenersRuntime>,
    ) {
        if self.automation_event_listeners.is_some() {
            return (self, None);
        }
        let listeners = crate::automation_listeners::AutomationEventListeners::new(
            self.bus.clone(),
            self.automations.clone(),
        );
        let runtime = listeners.start(cancel);
        self.automation_event_listeners = Some(listeners);
        (self, runtime)
    }

    /// Starts the owned subscriber → activity → notification → Automation
    /// pipeline. One FIFO preserves Go's synchronous registration order for
    /// consecutive publications without blocking the synchronous Rust bus.
    pub fn start_ordered_event_side_effects(
        mut self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> (
        Self,
        Option<crate::ordered_event_side_effects::OrderedEventSideEffectsRuntime>,
    ) {
        if self.ordered_event_side_effects.is_some() {
            return (self, None);
        }
        let side_effects = crate::ordered_event_side_effects::OrderedEventSideEffects::new(
            self.pool.clone(),
            self.bus.clone(),
            self.automations.clone(),
        );
        let runtime = side_effects.start(cancel);
        self.ordered_event_side_effects = Some(side_effects);
        (self, runtime)
    }

    /// Installs the production entitlement provider on the one shared
    /// Automation service. `None` is the self-hosted/off policy and deliberately
    /// avoids all quota-table reads.
    pub fn with_automation_entitlements(
        mut self,
        entitlements: Option<Arc<dyn EntitlementProvider>>,
    ) -> Self {
        let mut service =
            AutomationService::new(self.pool.clone(), self.bus.clone(), self.tasks.clone());
        service.entitlements = entitlements;
        service.quota_metrics = self.business_metrics.clone().map(|metrics| {
            metrics as Arc<dyn patchbay_service::automation::AutomationQuotaMetrics>
        });
        self.automations = Arc::new(service);
        self
    }

    /// Prepares the PostgreSQL-leased webhook worker without spawning it.
    /// Production installs root cancellation and owns the returned runtime.
    pub fn prepare_webhook_delivery_worker(
        mut self,
    ) -> (
        Self,
        Arc<crate::webhook_delivery_worker::WebhookDeliveryWorker>,
    ) {
        let notify = Arc::new(tokio::sync::Notify::new());
        let worker = crate::webhook_delivery_worker::WebhookDeliveryWorker::new(
            self.pool.clone(),
            self.automations.clone(),
            notify.clone(),
            self.webhook_rate_limits.token.clone(),
            self.business_metrics.clone(),
        );
        self.webhook_delivery_notify = Some(notify);
        (self, worker)
    }

    /// Starts the durable coordinator after production dependencies have been
    /// finalized. The database outbox remains the source of truth across
    /// missed notifications and process restarts.
    pub fn start_coordinator(
        self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> (Self, patchbay_service::coordination::CoordinatorRuntime) {
        let runtime = self.coordinator.start(cancel);
        (self, runtime)
    }

    pub fn notify_webhook_delivery(&self) {
        if let Some(notify) = &self.webhook_delivery_notify {
            notify.notify_one();
        }
    }

    /// Returns whether Linear pull/import may mutate this workspace. The
    /// allowlist is intentionally required even when the global flag is on,
    /// so the first rollout cannot fan out to every connected workspace.
    pub fn linear_pull_import_enabled(&self, workspace_id: uuid::Uuid) -> bool {
        if !self.linear_pull_import_enabled_for_any_workspace() {
            return false;
        }
        let allowlist = std::env::var("PATCHBAY_LINEAR_PULL_IMPORT_WORKSPACES").unwrap_or_default();
        allowlist
            .split(',')
            .map(str::trim)
            .any(|entry| entry == "*" || entry.eq_ignore_ascii_case(&workspace_id.to_string()))
    }

    pub fn linear_pull_import_enabled_for_any_workspace(&self) -> bool {
        self.linear_integration_enabled
            && self.feature_flags.as_deref().is_some_and(|flags| {
                flags.is_enabled(patchbay_service::feature_flags::LINEAR_PULL_IMPORT, false)
            })
            && std::env::var("PATCHBAY_LINEAR_PULL_IMPORT_WORKSPACES")
                .map(|value| value.split(',').any(|entry| !entry.trim().is_empty()))
                .unwrap_or(false)
    }

    /// Returns the rollout scope used by the Linear pull worker when it
    /// claims durable Inbox rows. `None` means the explicit `*` allowlist is
    /// active; `Some(empty)` deliberately claims nothing when the deployment
    /// has an invalid or empty workspace allowlist. Filtering at claim time
    /// prevents a worker from acknowledging another workspace's receipt just
    /// because the global feature flag is enabled.
    pub fn linear_pull_import_workspace_filter(&self) -> Option<Vec<uuid::Uuid>> {
        let allowlist = std::env::var("PATCHBAY_LINEAR_PULL_IMPORT_WORKSPACES")
            .unwrap_or_default();
        if allowlist
            .split(',')
            .map(str::trim)
            .any(|entry| entry == "*")
        {
            return None;
        }
        Some(
            allowlist
                .split(',')
                .filter_map(|entry| uuid::Uuid::parse_str(entry.trim()).ok())
                .collect(),
        )
    }

    /// Returns whether Linear outbound Issue publication may mutate this
    /// workspace. Push has an independent allowlist and flag so enabling
    /// inbound import never implicitly enables provider writes.
    pub fn linear_push_enabled(&self, workspace_id: uuid::Uuid) -> bool {
        if !self.linear_push_enabled_for_any_workspace() {
            return false;
        }
        let allowlist = std::env::var("PATCHBAY_LINEAR_PUSH_WORKSPACES").unwrap_or_default();
        allowlist
            .split(',')
            .map(str::trim)
            .any(|entry| entry == "*" || entry.eq_ignore_ascii_case(&workspace_id.to_string()))
    }

    pub fn linear_push_enabled_for_any_workspace(&self) -> bool {
        self.linear_integration_enabled
            && self.feature_flags.as_deref().is_some_and(|flags| {
                flags.is_enabled(patchbay_service::feature_flags::LINEAR_PUSH, false)
            })
            && std::env::var("PATCHBAY_LINEAR_PUSH_WORKSPACES")
                .map(|value| value.split(',').any(|entry| !entry.trim().is_empty()))
                .unwrap_or(false)
    }

    /// Prepares the Linear pull/import worker without spawning it. Production
    /// owns the returned runtime and calls this only after final wiring.
    pub fn prepare_linear_sync_worker(
        mut self,
    ) -> (Self, Arc<crate::linear_sync_worker::LinearSyncWorker>) {
        let notify = Arc::new(tokio::sync::Notify::new());
        let worker = crate::linear_sync_worker::LinearSyncWorker::new(self.clone(), notify.clone());
        self.linear_sync_notify = Some(notify);
        (self, worker)
    }

    pub fn notify_linear_sync(&self) {
        if let Some(notify) = &self.linear_sync_notify {
            notify.notify_one();
        }
    }

    pub fn with_auth_settings(mut self, settings: crate::auth::AuthSettings) -> Self {
        self.auth_settings = settings;
        self
    }

    pub fn with_clerk_auth_from_config(
        mut self,
        config: &patchbay_config::AuthConfig,
    ) -> anyhow::Result<Self> {
        self.clerk_auth = crate::clerk_auth::ClerkAuthClient::from_config(config)?
            .map(|client| Arc::new(client) as Arc<dyn crate::clerk_auth::ClerkSessionVerifier>);
        Ok(self)
    }

    pub fn with_cloud_pat_fleet_url(mut self, fleet_url: Option<&str>) -> Self {
        let url = fleet_url
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_default()
            .to_string();
        self.cloud_pat_verifier = (!url.is_empty())
            .then(|| patchbay_auth::cloud_pat::CloudPatVerifier::new(&url))
            .flatten();
        self.cloud_runtime_base_url = url;
        self
    }

    pub fn with_email_service(mut self, email_service: Arc<EmailService>) -> Self {
        self.email_service = email_service;
        self
    }

    pub fn with_rate_limit_trusted_proxies(mut self, raw: Option<&str>) -> Self {
        let trusted =
            patchbay_middleware::ratelimit::parse_trusted_proxies(raw.unwrap_or_default());
        self.auth_rate_limit.trusted_proxies = trusted.clone();
        self.auth_verify_rate_limit.trusted_proxies = trusted;
        self
    }

    /// Installs the S7 GitHub snapshot manager. Applied snapshots
    /// are broadcast with the same weakest-role PR payload as the Go handler.
    pub fn with_github_snapshots(mut self, client: Option<patchbay_ghsnapshot::Client>) -> Self {
        let pool = self.pool.clone();
        let event_pool = pool.clone();
        let bus = self.bus.clone();
        let on_applied: patchbay_ghsnapshot::OnApplied = Arc::new(move |pull_request_id| {
            let pool = event_pool.clone();
            let bus = bus.clone();
            Box::pin(async move {
                let Ok(Some(pull_request)) =
                    patchbay_db::queries::github_snapshot::get_git_hub_pull_request_by_id(
                        &pool,
                        pull_request_id,
                    )
                    .await
                else {
                    return;
                };
                let Ok(issue_ids) = patchbay_db::queries::github::list_issue_i_ds_for_pull_request(
                    &pool,
                    pull_request.workspace_id,
                    pull_request_id,
                )
                .await
                else {
                    return;
                };
                let payload =
                    crate::issue_pull_request::github_model_response(pull_request.clone(), true);
                bus.publish(&patchbay_events::Event {
                    event_type: patchbay_protocol::EVENT_PULL_REQUEST_UPDATED.into(),
                    workspace_id: pull_request.workspace_id.to_string(),
                    actor_type: "system".into(),
                    payload: serde_json::json!({
                        "pull_request": payload,
                        "linked_issue_ids": issue_ids.into_iter().map(|id| id.to_string()).collect::<Vec<_>>(),
                    }),
                    ..Default::default()
                });
            })
        });
        let manager = Arc::new(patchbay_ghsnapshot::Manager::new(
            client,
            Some(pool),
            Some(on_applied),
        ));
        self.github_snapshots = manager;
        self
    }

    /// Builds all handler/service Redis dependencies from the production
    /// client: auth/member caches, empty-claim cache, runtime liveness,
    /// invitation/webhook gates, and pending request stores.
    pub fn with_redis(mut self, client: redis::Client) -> Self {
        let desktop_handoff_connection = patchbay_redis::RecoveringConnection::new(client.clone());
        self.auth_rate_limit = self.auth_rate_limit.with_client(client.clone());
        self.auth_verify_rate_limit = self.auth_verify_rate_limit.with_client(client.clone());
        self.rate_limit_client = Some(client.clone());
        let conn = patchbay_redis::RecoveringConnection::new(client);
        self.invitation_admission = self.invitation_admission.with_redis(conn.clone());
        self.pat_cache = PatCache::from_connection(conn.clone());
        self.daemon_token_cache = DaemonTokenCache::from_connection(conn.clone());
        self.membership_cache = MembershipCache::from_connection(conn.clone());
        if let Some(verifier) = self.cloud_pat_verifier.as_mut() {
            verifier.set_cache(conn.clone());
        }
        self.tasks.install_empty_claim_cache(
            patchbay_service::empty_claim_cache::EmptyClaimCache::new(conn.clone()),
        );
        self.update_store = Some(Arc::new(crate::pending_store::UpdateStore::new(
            conn.clone(),
        )));
        self.model_list_store = Some(Arc::new(crate::pending_store::ModelListStore::new(
            conn.clone(),
        )));
        self.model_catalog_cache = Some(Arc::new(crate::pending_store::ModelCatalogCache::new(
            conn.clone(),
        )));
        self.liveness_store = crate::runtime_liveness::RedisLivenessStore::new(conn.clone());
        self.webhook_rate_limits =
            crate::webhook_rate_limit::WebhookRateLimits::redis(conn.clone());
        self.desktop_handoff_tokens = self
            .desktop_handoff_tokens
            .with_connection(desktop_handoff_connection);
        self.local_skill_list_store = Some(Arc::new(
            crate::pending_store::LocalSkillListStore::new(conn.clone()),
        ));
        self.local_skill_import_store = Some(Arc::new(
            crate::pending_store::LocalSkillImportStore::new(conn.clone()),
        ));
        self
    }

    /// Installs the Go-compatible single-node pending-request lifecycle when
    /// Redis is intentionally absent. These stores are process-local by
    /// design; configured Redis failures still fail closed at startup.
    pub fn with_in_memory_pending_stores(mut self) -> Self {
        self.update_store = Some(Arc::new(crate::pending_store::InMemoryUpdateStore::new()));
        self.model_list_store = Some(Arc::new(crate::pending_store::InMemoryModelListStore::new()));
        self.model_catalog_cache = Some(Arc::new(
            crate::pending_store::InMemoryModelCatalogCache::new(),
        ));
        self.local_skill_list_store = Some(Arc::new(
            crate::pending_store::InMemoryLocalSkillListStore::new(),
        ));
        self.local_skill_import_store = Some(Arc::new(
            crate::pending_store::InMemoryLocalSkillImportStore::new(),
        ));
        self
    }

    /// Connects the daemon WebSocket heartbeat consumer after all production
    /// pending stores have been finalized. The handler snapshots only its
    /// required database/store dependencies and therefore does not retain the
    /// hub or form a lifecycle cycle.
    pub fn with_daemon_heartbeat_handler(self) -> Self {
        if let Some(hub) = self.daemon_hub.as_ref() {
            hub.set_heartbeat_handler(Some(Arc::new(
                crate::daemon::DaemonHeartbeatProcessor::from_state(&self),
            )));
        }
        self
    }

    /// Installs the production daemon WS RPC dispatcher after the shared task
    /// and plugin services are finalized. Its dependency snapshot excludes the
    /// hub, so the callback remains owned without retaining the server state.
    pub fn with_daemon_rpc_handler(self) -> Self {
        if let Some(hub) = self.daemon_hub.as_ref() {
            hub.set_rpc_handler(Some(Arc::new(
                crate::daemon::DaemonRpcProcessor::from_state(&self),
            )));
        }
        self
    }

    /// Wires only public-route rate limiting. Kept separate from `with_redis`
    /// so a handler-domain migration cannot implicitly activate pending-store
    /// behavior owned by other S8 domains.
    pub fn with_rate_limit_redis(mut self, client: redis::Client) -> Self {
        // Keep Redis lazy like Go's redis.Client. Shared recovering
        // connections prevent cold-start stampedes while the existing
        // operation timeouts keep requests bounded.
        let invitation_redis = patchbay_redis::RecoveringConnection::new(client.clone());
        self.rate_limit_client = Some(client.clone());
        self.desktop_handoff_tokens = self
            .desktop_handoff_tokens
            .with_connection(patchbay_redis::RecoveringConnection::new(client.clone()));
        self.invitation_admission = self.invitation_admission.with_redis(invitation_redis);
        self
    }

    /// Installs lazy, fail-open Redis auth limiting without connecting during boot.
    pub fn with_auth_redis(mut self, client: redis::Client) -> Self {
        self.desktop_handoff_tokens = self
            .desktop_handoff_tokens
            .with_connection(patchbay_redis::RecoveringConnection::new(client.clone()));
        self.auth_rate_limit = self.auth_rate_limit.with_client(client.clone());
        self.auth_verify_rate_limit = self.auth_verify_rate_limit.with_client(client);
        self
    }

    pub fn with_vcs_webhooks(
        mut self,
        enabled: bool,
        secret_box: Option<patchbay_util::secretbox::SecretBox>,
    ) -> Self {
        self.vcs_integration_enabled = enabled;
        self.vcs_secret_box = secret_box;
        self
    }

    pub fn with_linear_integration(
        mut self,
        enabled: bool,
        secret_box: Option<patchbay_util::secretbox::SecretBox>,
    ) -> Self {
        self.linear_integration_enabled = enabled;
        self.linear_secret_box = secret_box;
        self
    }
}

fn positive_env_i64(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPOSIO_ENV: [&str; 8] = [
        "COMPOSIO_API_KEY",
        "COMPOSIO_STATE_SECRET",
        "COMPOSIO_CALLBACK_BASE_URL",
        "PATCHBAY_PUBLIC_URL",
        "PATCHBAY_APP_URL",
        "FRONTEND_ORIGIN",
        "FF_COMPOSIO_MCP_APPS",
        "JWT_SECRET",
    ];

    struct RestoreComposioEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);

    impl RestoreComposioEnv {
        fn clear() -> Self {
            let saved = COMPOSIO_ENV
                .into_iter()
                .map(|name| (name, std::env::var_os(name)))
                .collect();
            for name in COMPOSIO_ENV {
                std::env::remove_var(name);
            }
            Self(saved)
        }
    }

    impl Drop for RestoreComposioEnv {
        fn drop(&mut self) {
            for (name, value) in self.0.drain(..) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    fn test_state() -> HandlerState {
        HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            PatCache::disabled(),
            None,
        )
    }

    #[tokio::test]
    async fn production_dependencies_gate_composio_and_keep_task_overlay_disabled() {
        let _env = RestoreComposioEnv::clear();
        let build = || {
            HandlerState::new_with_production_dependencies(
                sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
                PatCache::disabled(),
                None,
                Arc::new(patchbay_analytics::NoopClient),
                Arc::new(patchbay_service::feature_flags::ConfiguredFlags::default()),
                None,
            )
        };

        std::env::set_var("FF_COMPOSIO_MCP_APPS", "1");
        let missing_api_key = build();
        assert!(missing_api_key.composio.is_none());
        assert!(missing_api_key.tasks.composio.read().unwrap().is_none());

        std::env::set_var("COMPOSIO_API_KEY", "test-api-key");
        let missing_state_secret = build();
        assert!(missing_state_secret.composio.is_none());
        assert!(missing_state_secret
            .tasks
            .composio
            .read()
            .unwrap()
            .is_none());

        std::env::set_var("COMPOSIO_STATE_SECRET", "test-state-secret");
        let missing_callback = build();
        assert!(missing_callback.composio.is_none());
        assert!(missing_callback.tasks.composio.read().unwrap().is_none());

        std::env::set_var("COMPOSIO_CALLBACK_BASE_URL", "https://api.example.com/");
        let configured = build();
        assert!(configured.composio.is_some());
        assert!(configured.tasks.composio.read().unwrap().is_none());

        std::env::set_var("FF_COMPOSIO_MCP_APPS", "0");
        let flag_disabled = build();
        assert!(flag_disabled.composio.is_none());
        assert!(flag_disabled.tasks.composio.read().unwrap().is_none());
    }

    #[tokio::test]
    async fn plugin_event_dispatcher_start_is_idempotent() {
        let (state, runtime) =
            test_state().start_plugin_event_dispatcher(tokio_util::sync::CancellationToken::new());
        let first = state
            .plugin_events
            .as_ref()
            .expect("dispatcher started")
            .clone();

        let (state, duplicate_runtime) =
            state.start_plugin_event_dispatcher(tokio_util::sync::CancellationToken::new());
        let second = state.plugin_events.as_ref().expect("dispatcher retained");
        assert!(Arc::ptr_eq(&first, second));
        assert!(duplicate_runtime.is_none());

        runtime
            .expect("first start owns runtime")
            .shutdown(patchbay_service::plugin_event_dispatch::DEFAULT_SHUTDOWN_TIMEOUT)
            .await;
    }

    #[tokio::test]
    async fn plugin_event_dispatcher_fails_closed_without_callback_tokens() {
        let mut state = test_state();
        state.callbacks = None;

        let (state, runtime) =
            state.start_plugin_event_dispatcher(tokio_util::sync::CancellationToken::new());

        assert!(state.plugin_events.is_none());
        assert!(runtime.is_none());
    }

    struct ComposioFlags(bool);

    impl patchbay_service::feature_flags::FlagSource for ComposioFlags {
        fn is_enabled(&self, key: &str, default: bool) -> bool {
            if key == patchbay_service::feature_flags::COMPOSIO_MCP_APPS {
                self.0
            } else {
                default
            }
        }
    }

    #[tokio::test]
    async fn loaded_config_installs_composio_and_llm_without_process_env() {
        let mut config = patchbay_config::Config::default();
        config.integrations.composio_api_key = Some("toml-api-key".into());
        config.integrations.composio_callback_base_url = Some("https://api.example".into());
        config.integrations.composio_state_secret = Some("toml-state-secret".into());
        config.llm.api_key = Some("toml-llm-key".into());
        config.llm.base_url = Some("https://llm.example/v1".into());
        config.llm.default_model = Some("toml-model".into());
        config.llm.max_retries = Some(3);

        let state = test_state()
            .with_feature_flags(Arc::new(ComposioFlags(true)))
            .with_composio_from_config(&config)
            .with_llm_from_config(&config.llm)
            .unwrap();
        assert!(state.composio.is_some());
        assert!(state.llm.client().enabled());
        assert_eq!(state.llm.client().default_model(), "toml-model");
        assert_eq!(state.llm.client().max_retries(), 3);
    }
}
