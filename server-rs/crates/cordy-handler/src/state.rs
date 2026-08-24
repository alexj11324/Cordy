//! Shared handler state — the Rust analogue of the Go `Handler` struct's
//! DB/redis wiring. Domain services are added per-slice as routes land.

use std::sync::Arc;

use cordy_auth::daemon_token_cache::DaemonTokenCache;
use cordy_auth::membership_cache::MembershipCache;
use cordy_auth::pat_cache::PatCache;
use cordy_realtime::hub::Hub;
use cordy_service::autopilot::{AutopilotService, EntitlementProvider};
use cordy_service::email::EmailService;
use cordy_service::issue_service::IssueService;
use cordy_service::plugin::PluginService;
use cordy_service::plugin_token::CallbackTokens;
use cordy_service::task_service::TaskService;

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
    pub async fn from_config(config: &cordy_config::Config) -> anyhow::Result<Self> {
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
            Some(raw) => cordy_auth::cookie::parse_auth_token_ttl(raw).ok_or_else(|| {
                anyhow::anyhow!("ATTACHMENT_DOWNLOAD_URL_TTL must be a positive Go duration")
            })?,
            None => std::time::Duration::from_secs(30 * 60),
        };
        chrono::Duration::from_std(ttl)
            .map_err(|_| anyhow::anyhow!("ATTACHMENT_DOWNLOAD_URL_TTL is too large"))?;
        let cloudfront_signer = crate::cloudfront::CloudFrontSigner::from_config(config)
            .await?
            .map(Arc::new);
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
    hub: Arc<cordy_daemon::hub::DaemonHub>,
}

struct DaemonMessageMetrics {
    metrics: Arc<cordy_metrics::BusinessMetrics>,
}

impl cordy_daemon::hub::MessageKindRecorder for DaemonMessageMetrics {
    fn record_daemon_ws_message_received(&self, kind: &str) {
        self.metrics.record_daemon_ws_message_received(kind);
    }
}

impl cordy_service::task_service::TaskWakeupNotifier for DaemonTaskWakeup {
    fn notify_task_available(&self, runtime_id: &str, task_id: &str) {
        self.hub.notify_task_available(runtime_id, task_id);
    }
}

/// Handler-layer state shared by all axum extractors.
#[derive(Clone)]
pub struct HandlerState {
    pub pool: sqlx::PgPool,
    pub pat_cache: PatCache,
    pub daemon_token_cache: DaemonTokenCache,
    pub membership_cache: MembershipCache,
    pub cloud_pat_verifier: Option<cordy_auth::cloud_pat::CloudPatVerifier>,
    /// Realtime WS hub (cordy-realtime). `None` only in tests.
    pub hub: Option<Arc<Hub>>,
    /// Event bus (Go h.Bus) for workspace-scoped WS fanout.
    pub bus: Arc<cordy_events::Bus>,
    /// Owned background work started by channel HTTP/event surfaces. The
    /// production ChannelRuntime closes admission and joins/aborts this group
    /// during shutdown.
    pub channel_tasks: Arc<cordy_channel::RuntimeTasks>,
    pub channel_cancel: tokio_util::sync::CancellationToken,
    /// Prometheus business counters. None when METRICS_ADDR is disabled.
    pub business_metrics: Option<Arc<cordy_metrics::BusinessMetrics>>,
    /// HTTP request metrics. None when METRICS_ADDR is disabled.
    pub http_metrics: Option<Arc<cordy_metrics::HttpMetrics>>,
    /// Public authentication dependencies and boot-time policy.
    pub auth_settings: crate::auth::AuthSettings,
    pub email_service: Arc<EmailService>,
    pub analytics: Arc<dyn cordy_analytics::AnalyticsClient>,
    pub auth_rate_limit: cordy_middleware::ratelimit::RateLimitState,
    pub auth_verify_rate_limit: cordy_middleware::ratelimit::RateLimitState,
    pub invitation_admission: crate::invitation::InvitationAdmission,
    /// Anonymous frontend capability/configuration response.
    pub public_config: crate::config::PublicConfigSettings,
    /// Immutable integration endpoint configuration loaded once at boot.
    /// Channel install flows use this same snapshot as the runtime connectors
    /// instead of re-reading process environment mid-session.
    pub integrations: cordy_config::IntegrationsConfig,
    /// GitHub GraphQL snapshot refresh pipeline. Disabled in lightweight tests.
    pub github_snapshots: Arc<cordy_ghsnapshot::Manager>,
    /// Feature flag source. `None` fails closed for rollout-gated writes.
    pub feature_flags: Option<Arc<dyn cordy_service::feature_flags::FlagSource>>,
    /// Task domain service (Go h.TaskService).
    pub tasks: Arc<TaskService>,
    /// Shared Autopilot service. It must be reused by HTTP paths and durable
    /// workers so entitlement/quota configuration cannot disappear per request.
    pub autopilots: Arc<AutopilotService>,
    /// Issue domain service (Go h.IssueService).
    pub issues: Arc<IssueService>,
    /// Plugin service (Go h.PluginService).
    pub plugins: Arc<PluginService>,
    /// Hook callback token store; None disables callback tokens (fail-closed).
    pub callbacks: Option<Arc<CallbackTokens>>,
    /// Absolute base URL used in hook callback_url; empty omits the field.
    pub callback_base_url: String,
    /// Boot-time bearer token for `/health/realtime`. Empty enables the
    /// direct-loopback-only development policy.
    pub realtime_metrics_token: String,
    /// Redis-backed pending request stores (update / model list / local
    /// skills). `None` matches Go's nil-store path: every probe reports an
    /// empty queue and report endpoints answer 404, which daemons treat as a
    /// dropped one-shot report.
    pub update_store: Option<Arc<crate::pending_store::UpdateStore>>,
    pub model_list_store: Option<Arc<crate::pending_store::ModelListStore>>,
    pub model_catalog_cache: Option<Arc<crate::pending_store::ModelCatalogCache>>,
    pub runtime_liveness: Option<Arc<dyn crate::runtime_liveness::RuntimeLivenessStore>>,
    pub webhook_rate_limits: crate::webhook_rate_limit::WebhookRateLimits,
    pub local_skill_list_store: Option<Arc<crate::pending_store::LocalSkillListStore>>,
    pub local_skill_import_store: Option<Arc<crate::pending_store::LocalSkillImportStore>>,
    /// Shared Redis connection for per-IP public-route rate limiting. None is
    /// the Go nil-client path and deliberately fails open.
    pub rate_limit_client: Option<redis::Client>,
    /// Public VCS webhook gate and at-rest secret decryptor. The feature is
    /// deliberately invisible when disabled and 503s when enabled without a
    /// usable key, matching Go's deployment boundary.
    pub vcs_integration_enabled: bool,
    pub vcs_secret_box: Option<cordy_util::secretbox::SecretBox>,
    /// Daemon WebSocket hub (cordy-daemon). `None` only in tests — the WS
    /// endpoint reports 503 and daemons fall back to HTTP polling.
    pub daemon_hub: Option<Arc<cordy_daemon::hub::DaemonHub>>,
    /// Attachment object store. None is the explicit unconfigured test path.
    pub attachment_storage: Option<Arc<dyn crate::attachment_storage::AttachmentStorage>>,
    /// Production Lark/WeCom media adapter over the same object store.
    pub channel_media_storage: Option<Arc<crate::attachment_storage::ChannelMediaStorage>>,
    pub attachment_frame_ancestors: Vec<String>,
    pub attachment_download: AttachmentDownloadSettings,
    /// On-demand Slack channel history reader. `None` means Slack history is
    /// not configured; chat history then falls back to the persisted transcript.
    pub slack_history: Option<Arc<cordy_slack::history::History>>,
    /// Server-internal assist LLM. An unconfigured client is deliberately
    /// inert and guarantees that private chat content produces no egress.
    pub llm: Arc<cordy_llm::Client>,
    /// Low-latency hint for the durable webhook worker. PostgreSQL polling is
    /// authoritative and recovers missed notifications or process restarts.
    webhook_delivery_notify: Option<Arc<tokio::sync::Notify>>,
    /// Keeps the weak notifier installed in `TaskService` alive.
    _task_wakeup: Arc<dyn cordy_service::task_service::TaskWakeupNotifier>,
}

impl HandlerState {
    pub fn new(pool: sqlx::PgPool, pat_cache: PatCache, hub: Option<Arc<Hub>>) -> Self {
        let bus = Arc::new(cordy_events::Bus::new());
        let daemon_hub = Arc::new(cordy_daemon::hub::DaemonHub::new());
        let task_wakeup: Arc<dyn cordy_service::task_service::TaskWakeupNotifier> =
            Arc::new(DaemonTaskWakeup {
                hub: daemon_hub.clone(),
            });
        let mut task_service = TaskService::new(pool.clone(), bus.clone());
        task_service.wakeup = Some(Arc::downgrade(&task_wakeup));
        let tasks = Arc::new(task_service);
        let autopilots = Arc::new(AutopilotService::new(
            pool.clone(),
            bus.clone(),
            tasks.clone(),
        ));
        let issues = Arc::new(IssueService::new(pool.clone(), bus.clone(), tasks.clone()));
        let plugins = Arc::new(PluginService::with_pool(pool.clone()));
        let trusted_proxies = cordy_middleware::ratelimit::parse_trusted_proxies(
            &std::env::var("RATE_LIMIT_TRUSTED_PROXIES").unwrap_or_default(),
        );
        let mut auth_rate_limit = cordy_middleware::ratelimit::RateLimitState::disabled(
            positive_env_i64("RATE_LIMIT_AUTH", 5),
            60,
        );
        auth_rate_limit.trusted_proxies = trusted_proxies.clone();
        let mut auth_verify_rate_limit = cordy_middleware::ratelimit::RateLimitState::disabled(
            positive_env_i64("RATE_LIMIT_AUTH_VERIFY", 20),
            60,
        );
        auth_verify_rate_limit.trusted_proxies = trusted_proxies;
        let llm = cordy_llm::Client::new(cordy_llm::Config::default());
        Self {
            pool,
            pat_cache,
            daemon_token_cache: DaemonTokenCache::disabled(),
            membership_cache: MembershipCache::disabled(),
            cloud_pat_verifier: None,
            hub,
            bus,
            channel_tasks: Arc::new(cordy_channel::RuntimeTasks::new()),
            channel_cancel: tokio_util::sync::CancellationToken::new(),
            business_metrics: None,
            http_metrics: None,
            auth_settings: crate::auth::AuthSettings::from_env(),
            email_service: Arc::new(EmailService::new()),
            analytics: Arc::new(cordy_analytics::NoopClient),
            auth_rate_limit,
            auth_verify_rate_limit,
            invitation_admission: crate::invitation::InvitationAdmission::default(),
            public_config: crate::config::PublicConfigSettings::default(),
            integrations: cordy_config::IntegrationsConfig::default(),
            github_snapshots: Arc::new(cordy_ghsnapshot::Manager::new(None, None, None)),
            feature_flags: None,
            tasks,
            autopilots,
            issues,
            plugins,
            callbacks: Some(Arc::new(CallbackTokens::new())),
            callback_base_url: String::new(),
            realtime_metrics_token: std::env::var("REALTIME_METRICS_TOKEN")
                .unwrap_or_default()
                .trim()
                .to_string(),
            update_store: None,
            model_list_store: None,
            model_catalog_cache: None,
            runtime_liveness: None,
            webhook_rate_limits: crate::webhook_rate_limit::WebhookRateLimits::default(),
            local_skill_list_store: None,
            local_skill_import_store: None,
            rate_limit_client: None,
            vcs_integration_enabled: false,
            vcs_secret_box: None,
            daemon_hub: Some(daemon_hub),
            attachment_storage: None,
            channel_media_storage: None,
            attachment_frame_ancestors: Vec::new(),
            attachment_download: AttachmentDownloadSettings::default(),
            slack_history: None,
            llm: Arc::new(llm),
            webhook_delivery_notify: None,
            _task_wakeup: task_wakeup,
        }
    }

    /// Wires the internal OpenAI-compatible assist layer. Invalid retry
    /// budgets fail startup rather than silently selecting another policy.
    pub fn with_llm_from_env(mut self) -> anyhow::Result<Self> {
        const MAX_RETRIES: u32 = 5;
        let raw_retries = std::env::var("CORDY_LLM_MAX_RETRIES").unwrap_or_default();
        let max_retries = if raw_retries.trim().is_empty() {
            None
        } else {
            let parsed = raw_retries.trim().parse::<u32>().map_err(|_| {
                anyhow::anyhow!(
                    "CORDY_LLM_MAX_RETRIES must be an integer from 0 to {MAX_RETRIES}, got {:?}",
                    raw_retries.trim()
                )
            })?;
            anyhow::ensure!(
                parsed <= MAX_RETRIES,
                "CORDY_LLM_MAX_RETRIES must be at most {MAX_RETRIES}, got {parsed}"
            );
            Some(parsed)
        };
        self.llm = Arc::new(cordy_llm::Client::new(cordy_llm::Config {
            api_key: std::env::var("CORDY_LLM_API_KEY").unwrap_or_default(),
            base_url: std::env::var("CORDY_LLM_BASE_URL").unwrap_or_default(),
            default_model: std::env::var("CORDY_LLM_DEFAULT_MODEL").unwrap_or_default(),
            max_retries,
        }));
        tracing::info!(
            enabled = self.llm.enabled(),
            max_retries = self.llm.max_retries(),
            default_model = self.llm.default_model(),
            "llm assist policy"
        );
        Ok(self)
    }

    /// Wires the S7 Slack history service with the same secretbox key used by
    /// channel installation credentials. Missing or invalid keys leave the
    /// reader disabled instead of interpreting ciphertext as plaintext.
    pub fn with_slack_history_from_env(mut self) -> Self {
        let Ok(key) = cordy_util::secretbox::load_key("CORDY_SLACK_SECRET_KEY") else {
            return self;
        };
        let Ok(secret_box) = cordy_util::secretbox::SecretBox::new(&key) else {
            return self;
        };
        let decrypt: Arc<cordy_slack::config::Decrypter> =
            Arc::new(move |sealed| secret_box.open(sealed).map_err(anyhow::Error::from));
        self.slack_history = Some(Arc::new(cordy_slack::history::History::new(
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

    pub fn with_integrations(mut self, integrations: cordy_config::IntegrationsConfig) -> Self {
        self.integrations = integrations;
        self
    }

    pub fn with_feature_flags(
        mut self,
        flags: Arc<dyn cordy_service::feature_flags::FlagSource>,
    ) -> Self {
        self.feature_flags = Some(flags);
        self
    }

    /// Replaces the lightweight test plugin service with production env
    /// wiring, including the encryption/signing key and callback URL.
    pub fn with_plugins_from_env(mut self) -> Self {
        let mut plugins = PluginService::new_from_env(self.pool.clone());
        if let Ok(key) = cordy_util::secretbox::load_key("CORDY_PLUGIN_SECRET_KEY") {
            plugins.secrets = cordy_util::secretbox::SecretBox::new(&key).ok();
        }
        self.plugins = Arc::new(plugins);
        self.callback_base_url = std::env::var("CORDY_PUBLIC_URL")
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
        business_metrics: Option<Arc<cordy_metrics::BusinessMetrics>>,
        http_metrics: Option<Arc<cordy_metrics::HttpMetrics>>,
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

    /// Installs the production entitlement provider on the one shared
    /// Autopilot service. `None` is the self-hosted/off policy and deliberately
    /// avoids all quota-table reads.
    pub fn with_autopilot_entitlements(
        mut self,
        entitlements: Option<Arc<dyn EntitlementProvider>>,
    ) -> Self {
        let mut service =
            AutopilotService::new(self.pool.clone(), self.bus.clone(), self.tasks.clone());
        service.entitlements = entitlements;
        service.quota_metrics = self
            .business_metrics
            .clone()
            .map(|metrics| metrics as Arc<dyn cordy_service::autopilot::AutopilotQuotaMetrics>);
        self.autopilots = Arc::new(service);
        self
    }

    /// Starts the bounded PostgreSQL-leased webhook worker pool. Call only
    /// from production startup after all Autopilot service wiring is complete.
    pub fn start_webhook_delivery_worker(mut self) -> Self {
        let notify = Arc::new(tokio::sync::Notify::new());
        crate::webhook_delivery_worker::WebhookDeliveryWorker::new(
            self.pool.clone(),
            self.autopilots.clone(),
            notify.clone(),
            self.webhook_rate_limits.token.clone(),
            self.business_metrics.clone(),
        )
        .start();
        self.webhook_delivery_notify = Some(notify);
        self
    }

    pub fn start_autopilot_quota_reconciler(self) -> Self {
        if !self.autopilots.quota_enabled() {
            return self;
        }
        let service = self.autopilots.clone();
        tokio::spawn(async move {
            loop {
                let now = chrono::Utc::now();
                match service
                    .reconcile_quota_reservations(
                        now - chrono::Duration::minutes(10),
                        now - chrono::Duration::hours(6),
                        100,
                    )
                    .await
                {
                    Ok(settled) if settled > 0 => {
                        tracing::info!(settled, "autopilot quota reconciler settled reservations");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(%error, "autopilot quota reconciler failed");
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        });
        self
    }

    pub fn notify_webhook_delivery(&self) {
        if let Some(notify) = &self.webhook_delivery_notify {
            notify.notify_one();
        }
    }

    pub fn with_analytics(mut self, analytics: Arc<dyn cordy_analytics::AnalyticsClient>) -> Self {
        self.analytics = analytics;
        self
    }

    pub fn with_auth_settings(mut self, settings: crate::auth::AuthSettings) -> Self {
        self.auth_settings = settings;
        self
    }

    pub fn with_cloud_pat_fleet_url(mut self, fleet_url: Option<&str>) -> Self {
        self.cloud_pat_verifier = fleet_url.and_then(cordy_auth::cloud_pat::CloudPatVerifier::new);
        self
    }

    pub fn with_email_service(mut self, email_service: Arc<EmailService>) -> Self {
        self.email_service = email_service;
        self
    }

    pub fn with_rate_limit_trusted_proxies(mut self, raw: Option<&str>) -> Self {
        let trusted = cordy_middleware::ratelimit::parse_trusted_proxies(raw.unwrap_or_default());
        self.auth_rate_limit.trusted_proxies = trusted.clone();
        self.auth_verify_rate_limit.trusted_proxies = trusted;
        self
    }

    /// Installs and starts the S7 GitHub snapshot manager. Applied snapshots
    /// are broadcast with the same weakest-role PR payload as the Go handler.
    pub fn with_github_snapshots(mut self, client: Option<cordy_ghsnapshot::Client>) -> Self {
        let pool = self.pool.clone();
        let event_pool = pool.clone();
        let bus = self.bus.clone();
        let on_applied: cordy_ghsnapshot::OnApplied = Arc::new(move |pull_request_id| {
            let pool = event_pool.clone();
            let bus = bus.clone();
            tokio::spawn(async move {
                let Ok(Some(pull_request)) =
                    cordy_db::queries::github_snapshot::get_git_hub_pull_request_by_id(
                        &pool,
                        pull_request_id,
                    )
                    .await
                else {
                    return;
                };
                let Ok(issue_ids) = cordy_db::queries::github::list_issue_i_ds_for_pull_request(
                    &pool,
                    pull_request_id,
                )
                .await
                else {
                    return;
                };
                let payload =
                    crate::issue_pull_request::github_model_response(pull_request.clone(), true);
                bus.publish(&cordy_events::Event {
                    event_type: cordy_protocol::EVENT_PULL_REQUEST_UPDATED.into(),
                    workspace_id: pull_request.workspace_id.to_string(),
                    actor_type: "system".into(),
                    payload: serde_json::json!({
                        "pull_request": payload,
                        "linked_issue_ids": issue_ids.into_iter().flatten().map(|id| id.to_string()).collect::<Vec<_>>(),
                    }),
                    ..Default::default()
                });
            });
        });
        let manager = Arc::new(cordy_ghsnapshot::Manager::new(
            client,
            Some(pool),
            Some(on_applied),
        ));
        manager.start();
        self.github_snapshots = manager;
        self
    }

    /// Builds all handler/service Redis dependencies from the production
    /// client: auth/member caches, empty-claim cache, runtime liveness,
    /// invitation/webhook gates, and pending request stores. Callers without
    /// Redis keep the explicit disabled implementations and preserve the Go
    /// nil-store behavior.
    pub fn with_redis(mut self, client: redis::Client) -> Self {
        self.auth_rate_limit = self.auth_rate_limit.with_client(client.clone());
        self.auth_verify_rate_limit = self.auth_verify_rate_limit.with_client(client.clone());
        let conn = cordy_redis::RecoveringConnection::new(client);
        self.invitation_admission = self.invitation_admission.with_redis(conn.clone());
        self.pat_cache = PatCache::from_connection(conn.clone());
        self.daemon_token_cache = DaemonTokenCache::from_connection(conn.clone());
        self.membership_cache = MembershipCache::from_connection(conn.clone());
        if let Some(verifier) = self.cloud_pat_verifier.as_mut() {
            verifier.set_cache(conn.clone());
        }
        self.tasks.install_empty_claim_cache(
            cordy_service::empty_claim_cache::EmptyClaimCache::new(conn.clone()),
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
        self.runtime_liveness = Some(Arc::new(
            crate::runtime_liveness::RedisRuntimeLivenessStore::new(conn.clone()),
        ));
        self.webhook_rate_limits =
            crate::webhook_rate_limit::WebhookRateLimits::redis(conn.clone());
        self.local_skill_list_store = Some(Arc::new(
            crate::pending_store::LocalSkillListStore::new(conn.clone()),
        ));
        self.local_skill_import_store = Some(Arc::new(
            crate::pending_store::LocalSkillImportStore::new(conn.clone()),
        ));
        self
    }

    /// Wires only public-route rate limiting. Kept separate from `with_redis`
    /// so a handler-domain migration cannot implicitly activate pending-store
    /// behavior owned by other S8 domains.
    pub fn with_rate_limit_redis(mut self, client: redis::Client) -> Self {
        // Keep Redis lazy like Go's redis.Client. Shared recovering
        // connections prevent cold-start stampedes while the existing
        // operation timeouts keep requests bounded.
        let invitation_redis = cordy_redis::RecoveringConnection::new(client.clone());
        self.rate_limit_client = Some(client);
        self.invitation_admission = self.invitation_admission.with_redis(invitation_redis);
        self
    }

    /// Installs lazy, fail-open Redis auth limiting without connecting during boot.
    pub fn with_auth_redis(mut self, client: redis::Client) -> Self {
        self.auth_rate_limit = self.auth_rate_limit.with_client(client.clone());
        self.auth_verify_rate_limit = self.auth_verify_rate_limit.with_client(client);
        self
    }

    pub fn with_vcs_webhooks(
        mut self,
        enabled: bool,
        secret_box: Option<cordy_util::secretbox::SecretBox>,
    ) -> Self {
        self.vcs_integration_enabled = enabled;
        self.vcs_secret_box = secret_box;
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
