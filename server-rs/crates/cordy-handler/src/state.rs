//! Shared handler state — the Rust analogue of the Go `Handler` struct's
//! DB/redis wiring. Domain services are added per-slice as routes land.

use std::sync::Arc;

use cordy_auth::daemon_token_cache::DaemonTokenCache;
use cordy_auth::pat_cache::PatCache;
use cordy_realtime::hub::Hub;
use cordy_service::autopilot::{AutopilotService, EntitlementProvider};
use cordy_service::email::EmailService;
use cordy_service::issue_service::IssueService;
use cordy_service::plugin::PluginService;
use cordy_service::plugin_event_dispatch::PluginEventDispatcher;
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

/// Handler-layer state shared by all axum extractors.
#[derive(Clone)]
pub struct HandlerState {
    pub pool: sqlx::PgPool,
    pub pat_cache: PatCache,
    pub daemon_token_cache: DaemonTokenCache,
    /// Realtime WS hub (cordy-realtime). `None` only in tests.
    pub hub: Option<Arc<Hub>>,
    /// Event bus (Go h.Bus) for workspace-scoped WS fanout.
    pub bus: Arc<cordy_events::Bus>,
    /// Prometheus business counters. None when METRICS_ADDR is disabled.
    pub business_metrics: Option<Arc<cordy_metrics::BusinessMetrics>>,
    /// HTTP request metrics. None when METRICS_ADDR is disabled.
    pub http_metrics: Option<Arc<cordy_metrics::HttpMetrics>>,
    pub heartbeat_scheduler: Arc<dyn crate::heartbeat_scheduler::HeartbeatScheduler>,
    pub liveness_store: Arc<dyn crate::runtime_liveness::LivenessStore>,
    /// Public authentication dependencies and boot-time policy.
    pub auth_settings: crate::auth::AuthSettings,
    pub email_service: Arc<EmailService>,
    pub analytics: Arc<dyn cordy_analytics::AnalyticsClient>,
    pub auth_rate_limit: cordy_middleware::ratelimit::RateLimitState,
    pub auth_verify_rate_limit: cordy_middleware::ratelimit::RateLimitState,
    pub invitation_admission: crate::invitation::InvitationAdmission,
    /// Anonymous frontend capability/configuration response.
    pub public_config: crate::config::PublicConfigSettings,
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
    /// Production event-hook workers and their bus subscriptions. `None` in
    /// lightweight tests and before production side effects are started.
    plugin_events: Option<Arc<PluginEventDispatcher>>,
    /// Owned Autopilot issue/task terminal listener set.
    autopilot_event_listeners: Option<Arc<crate::autopilot_listeners::AutopilotEventListeners>>,
    /// Ordered subscriber → activity → notification pipeline. The bus retains
    /// its callback; this field guards registration and exposes lifecycle.
    ordered_event_side_effects:
        Option<Arc<crate::ordered_event_side_effects::OrderedEventSideEffects>>,
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
        let heartbeat_scheduler =
            Arc::new(crate::heartbeat_scheduler::PassthroughHeartbeatScheduler::new(pool.clone()));
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
            hub,
            bus,
            business_metrics: None,
            http_metrics: None,
            heartbeat_scheduler,
            liveness_store: Arc::new(crate::runtime_liveness::NoopLivenessStore),
            auth_settings: crate::auth::AuthSettings::from_env(),
            email_service: Arc::new(EmailService::new()),
            analytics: Arc::new(cordy_analytics::NoopClient),
            auth_rate_limit,
            auth_verify_rate_limit,
            invitation_admission: crate::invitation::InvitationAdmission::default(),
            public_config: crate::config::PublicConfigSettings::default(),
            github_snapshots: Arc::new(cordy_ghsnapshot::Manager::new(None, None, None)),
            feature_flags: None,
            tasks,
            autopilots,
            issues,
            plugins,
            callbacks: Some(Arc::new(CallbackTokens::new())),
            callback_base_url: String::new(),
            plugin_events: None,
            autopilot_event_listeners: None,
            ordered_event_side_effects: None,
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

    pub fn with_heartbeat_scheduler(
        mut self,
        scheduler: Arc<dyn crate::heartbeat_scheduler::HeartbeatScheduler>,
    ) -> Self {
        self.heartbeat_scheduler = scheduler;
        self
    }

    pub fn with_liveness_redis(mut self, client: redis::Client) -> Self {
        self.liveness_store = crate::runtime_liveness::RedisLivenessStore::new(client);
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
        Option<cordy_service::plugin_event_dispatch::PluginEventDispatcherRuntime>,
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
        cordy_service::plugin_event_dispatch::subscribe_plugin_events(
            self.bus.as_ref(),
            dispatcher.clone(),
        );
        let runtime = dispatcher.start(cancel);
        self.plugin_events = Some(dispatcher);
        (self, runtime)
    }

    /// Wires the issue/task terminal events that settle linked Autopilot runs.
    /// Lightweight state construction stays side-effect free; production calls
    /// this only after the shared Autopilot service has its final dependencies.
    pub fn start_autopilot_event_listeners(
        mut self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> (
        Self,
        Option<crate::autopilot_listeners::AutopilotEventListenersRuntime>,
    ) {
        if self.autopilot_event_listeners.is_some() {
            return (self, None);
        }
        let listeners = crate::autopilot_listeners::AutopilotEventListeners::new(
            self.bus.clone(),
            self.autopilots.clone(),
        );
        let runtime = listeners.start(cancel);
        self.autopilot_event_listeners = Some(listeners);
        (self, runtime)
    }

    /// Starts the owned subscriber → activity → notification pipeline. One
    /// event stays ordered inside one task; independent events remain
    /// concurrent as they are across Go request goroutines.
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
        );
        let runtime = side_effects.start(cancel);
        self.ordered_event_side_effects = Some(side_effects);
        (self, runtime)
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
            self.autopilots.clone(),
            notify.clone(),
        );
        self.webhook_delivery_notify = Some(notify);
        (self, worker)
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

    /// Installs the S7 GitHub snapshot manager. Applied snapshots
    /// are broadcast with the same weakest-role PR payload as the Go handler.
    pub fn with_github_snapshots(mut self, client: Option<cordy_ghsnapshot::Client>) -> Self {
        let pool = self.pool.clone();
        let event_pool = pool.clone();
        let bus = self.bus.clone();
        let on_applied: cordy_ghsnapshot::OnApplied = Arc::new(move |pull_request_id| {
            let pool = event_pool.clone();
            let bus = bus.clone();
            Box::pin(async move {
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
            })
        });
        let manager = Arc::new(cordy_ghsnapshot::Manager::new(
            client,
            Some(pool),
            Some(on_applied),
        ));
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
        self.daemon_token_cache = DaemonTokenCache::new(client.clone()).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> HandlerState {
        HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            PatCache::disabled(),
            None,
        )
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
            .shutdown(cordy_service::plugin_event_dispatch::DEFAULT_SHUTDOWN_TIMEOUT)
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
}
