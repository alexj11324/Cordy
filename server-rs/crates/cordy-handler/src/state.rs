//! Shared handler state — the Rust analogue of the Go `Handler` struct's
//! DB/redis wiring. Domain services are added per-slice as routes land.

use std::sync::Arc;

use cordy_auth::pat_cache::PatCache;
use cordy_realtime::hub::Hub;
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
    /// GitHub GraphQL snapshot refresh pipeline. Disabled in lightweight tests.
    pub github_snapshots: Arc<cordy_ghsnapshot::Manager>,
    /// Feature flag source. `None` fails closed for rollout-gated writes.
    pub feature_flags: Option<Arc<dyn cordy_service::feature_flags::FlagSource>>,
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
    pub local_skill_list_store: Option<Arc<crate::pending_store::LocalSkillListStore>>,
    pub local_skill_import_store: Option<Arc<crate::pending_store::LocalSkillImportStore>>,
    /// Daemon WebSocket hub (cordy-daemon). `None` only in tests — the WS
    /// endpoint reports 503 and daemons fall back to HTTP polling.
    pub daemon_hub: Option<Arc<cordy_daemon::hub::DaemonHub>>,
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
        let issues = Arc::new(IssueService::new(pool.clone(), bus.clone(), tasks.clone()));
        let plugins = Arc::new(PluginService::with_pool(pool.clone()));
        Self {
            pool,
            pat_cache,
            hub,
            bus,
            business_metrics: None,
            http_metrics: None,
            github_snapshots: Arc::new(cordy_ghsnapshot::Manager::new(None, None, None)),
            feature_flags: None,
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
            local_skill_list_store: None,
            local_skill_import_store: None,
            daemon_hub: Some(daemon_hub),
            _task_wakeup: task_wakeup,
        }
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
        let conn = client.get_connection_manager().await?;
        self.update_store = Some(Arc::new(crate::pending_store::UpdateStore::new(
            conn.clone(),
        )));
        self.model_list_store = Some(Arc::new(crate::pending_store::ModelListStore::new(
            conn.clone(),
        )));
        self.local_skill_list_store = Some(Arc::new(
            crate::pending_store::LocalSkillListStore::new(conn.clone()),
        ));
        self.local_skill_import_store = Some(Arc::new(
            crate::pending_store::LocalSkillImportStore::new(conn),
        ));
        Ok(self)
    }
}
