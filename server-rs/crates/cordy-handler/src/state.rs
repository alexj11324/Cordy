//! Shared handler state — the Rust analogue of the Go `Handler` struct's
//! DB/redis wiring. Domain services are added per-slice as routes land.

use std::sync::Arc;

use cordy_auth::pat_cache::PatCache;
use cordy_realtime::hub::Hub;
use cordy_service::email::EmailService;
use cordy_service::issue_service::IssueService;
use cordy_service::plugin::PluginService;
use cordy_service::plugin_token::CallbackTokens;
use cordy_service::task_service::TaskService;

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

struct SharedFlagSource(Arc<dyn cordy_service::feature_flags::FlagSource>);

impl cordy_service::feature_flags::FlagSource for SharedFlagSource {
    fn is_enabled(&self, key: &str, default: bool) -> bool {
        self.0.is_enabled(key, default)
    }
}

/// Handler-layer state shared by all axum extractors.
#[derive(Clone)]
pub struct HandlerState {
    pub pool: sqlx::PgPool,
    pub pat_cache: PatCache,
    /// Realtime WS hub (cordy-realtime). `None` only in tests.
    pub hub: Option<Arc<Hub>>,
    /// Event bus (Go h.Bus) for workspace-scoped WS fanout.
    pub bus: Arc<cordy_events::Bus>,
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
    /// Anonymous frontend capability/configuration response.
    pub public_config: crate::config::PublicConfigSettings,
    /// GitHub GraphQL snapshot refresh pipeline. Disabled in lightweight tests.
    pub github_snapshots: Arc<cordy_ghsnapshot::Manager>,
    /// Feature flag source. `None` fails closed for rollout-gated writes.
    pub feature_flags: Option<Arc<dyn cordy_service::feature_flags::FlagSource>>,
    /// Shared Composio service used by both HTTP routes and task overlays.
    pub composio_service: Option<Arc<cordy_composio::Service>>,
    /// Task domain service (Go h.TaskService).
    pub tasks: Arc<TaskService>,
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
    pub attachment_frame_ancestors: Vec<String>,
    /// Keeps the weak notifier installed in `TaskService` alive.
    _task_wakeup: Arc<dyn cordy_service::task_service::TaskWakeupNotifier>,
}

impl HandlerState {
    pub fn new(pool: sqlx::PgPool, pat_cache: PatCache, hub: Option<Arc<Hub>>) -> Self {
        Self::new_with_runtime_integrations(pool, pat_cache, hub, None, None)
    }

    pub fn new_with_runtime_integrations(
        pool: sqlx::PgPool,
        pat_cache: PatCache,
        hub: Option<Arc<Hub>>,
        feature_flags: Option<Arc<dyn cordy_service::feature_flags::FlagSource>>,
        composio_service: Option<Arc<cordy_composio::Service>>,
    ) -> Self {
        let bus = Arc::new(cordy_events::Bus::new());
        let daemon_hub = Arc::new(cordy_daemon::hub::DaemonHub::new());
        let task_wakeup: Arc<dyn cordy_service::task_service::TaskWakeupNotifier> =
            Arc::new(DaemonTaskWakeup {
                hub: daemon_hub.clone(),
            });
        let mut task_service = TaskService::new(pool.clone(), bus.clone());
        task_service.wakeup = Some(Arc::downgrade(&task_wakeup));
        task_service.feature_flags = feature_flags.as_ref().map(|flags| {
            Box::new(SharedFlagSource(flags.clone()))
                as Box<dyn cordy_service::feature_flags::FlagSource>
        });
        task_service.composio = composio_service.as_ref().map(|service| {
            Arc::new(crate::composio::TaskOverlayBuilder::new(service.clone()))
                as Arc<dyn cordy_service::task_service::ComposioOverlayBuilder>
        });
        let tasks = Arc::new(task_service);
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
        Self {
            pool,
            pat_cache,
            hub,
            bus,
            business_metrics: None,
            http_metrics: None,
            auth_settings: crate::auth::AuthSettings::from_env(),
            email_service: Arc::new(EmailService::new()),
            analytics: Arc::new(cordy_analytics::NoopClient),
            auth_rate_limit,
            auth_verify_rate_limit,
            public_config: crate::config::PublicConfigSettings::default(),
            github_snapshots: Arc::new(cordy_ghsnapshot::Manager::new(None, None, None)),
            feature_flags,
            composio_service,
            tasks,
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
            local_skill_list_store: None,
            local_skill_import_store: None,
            rate_limit_client: None,
            vcs_integration_enabled: false,
            vcs_secret_box: None,
            daemon_hub: Some(daemon_hub),
            attachment_storage: None,
            attachment_frame_ancestors: Vec::new(),
            _task_wakeup: task_wakeup,
        }
    }

    pub fn with_attachment_storage(
        mut self,
        storage: Arc<dyn crate::attachment_storage::AttachmentStorage>,
        frame_ancestors: Vec<String>,
    ) -> Self {
        self.attachment_storage = Some(storage);
        self.attachment_frame_ancestors = frame_ancestors;
        self
    }

    pub fn with_public_config(mut self, settings: crate::config::PublicConfigSettings) -> Self {
        self.public_config = settings;
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

    pub fn with_analytics(mut self, analytics: Arc<dyn cordy_analytics::AnalyticsClient>) -> Self {
        self.analytics = analytics;
        self
    }

    pub fn with_auth_settings(mut self, settings: crate::auth::AuthSettings) -> Self {
        self.auth_settings = settings;
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

    /// Builds the pending-request stores from a Redis client (Go
    /// NewRedis{Update,ModelList,LocalSkill*}Store wiring). Callers without
    /// Redis keep `None` fields — the disabled path degrades exactly like Go's
    /// nil-store behavior.
    pub async fn with_redis(mut self, client: redis::Client) -> Result<Self, redis::RedisError> {
        self.auth_rate_limit = self.auth_rate_limit.with_client(client.clone());
        self.auth_verify_rate_limit = self.auth_verify_rate_limit.with_client(client.clone());
        let conn = client.get_connection_manager().await?;
        self.update_store = Some(Arc::new(crate::pending_store::UpdateStore::new(
            conn.clone(),
        )));
        self.model_list_store = Some(Arc::new(crate::pending_store::ModelListStore::new(
            conn.clone(),
        )));
        self.model_catalog_cache = Some(Arc::new(crate::pending_store::ModelCatalogCache::new(
            conn.clone(),
        )));
        self.local_skill_list_store = Some(Arc::new(
            crate::pending_store::LocalSkillListStore::new(conn.clone()),
        ));
        self.local_skill_import_store = Some(Arc::new(
            crate::pending_store::LocalSkillImportStore::new(conn.clone()),
        ));
        Ok(self)
    }

    /// Wires only public-route rate limiting. Kept separate from `with_redis`
    /// so a handler-domain migration cannot implicitly activate pending-store
    /// behavior owned by other S8 domains.
    pub fn with_rate_limit_redis(mut self, client: redis::Client) -> Self {
        // Keep Redis lazy like Go's redis.Client. The middleware establishes
        // and caches a bounded connection on demand, so an unavailable Redis
        // never delays or aborts HTTP server startup and can recover later.
        self.rate_limit_client = Some(client);
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
