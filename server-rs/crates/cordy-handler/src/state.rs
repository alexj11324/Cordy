//! Shared handler state — the Rust analogue of the Go `Handler` struct's
//! DB/redis wiring. Domain services are added per-slice as routes land.

use std::sync::Arc;

use cordy_auth::pat_cache::PatCache;
use cordy_realtime::hub::Hub;
use cordy_service::issue_service::IssueService;
use cordy_service::plugin::PluginService;
use cordy_service::plugin_token::CallbackTokens;
use cordy_service::task_service::TaskService;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachmentDownloadMode {
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
        let mode = match config
            .storage
            .attachment_download_mode
            .as_deref()
            .unwrap_or("auto")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "" | "auto" => AttachmentDownloadMode::Auto,
            "cloudfront" => AttachmentDownloadMode::CloudFront,
            "presign" => AttachmentDownloadMode::Presign,
            "proxy" => AttachmentDownloadMode::Proxy,
            _ => anyhow::bail!(
                "ATTACHMENT_DOWNLOAD_MODE must be auto, cloudfront, presign, or proxy"
            ),
        };
        let ttl = config
            .storage
            .attachment_download_url_ttl
            .as_deref()
            .map(parse_attachment_ttl)
            .transpose()?
            .unwrap_or_else(|| std::time::Duration::from_secs(30 * 60));
        let cloudfront_signer = crate::cloudfront::CloudFrontSigner::from_config(config)
            .await?
            .map(Arc::new);
        anyhow::ensure!(
            mode != AttachmentDownloadMode::CloudFront || cloudfront_signer.is_some(),
            "ATTACHMENT_DOWNLOAD_MODE=cloudfront requires a CloudFront signing key"
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
}

fn parse_attachment_ttl(raw: &str) -> anyhow::Result<std::time::Duration> {
    let raw = raw.trim();
    anyhow::ensure!(
        !raw.is_empty(),
        "ATTACHMENT_DOWNLOAD_URL_TTL cannot be empty"
    );
    let raw = raw.strip_prefix('+').unwrap_or(raw);
    anyhow::ensure!(
        !raw.starts_with('-'),
        "ATTACHMENT_DOWNLOAD_URL_TTL must be positive"
    );
    anyhow::ensure!(raw != "0", "ATTACHMENT_DOWNLOAD_URL_TTL must be positive");

    let mut rest = raw;
    let mut total_nanos = 0_u128;
    while !rest.is_empty() {
        let integer_len = rest.bytes().take_while(u8::is_ascii_digit).count();
        let mut number_len = integer_len;
        let mut fraction = None;
        if rest[integer_len..].starts_with('.') {
            let fraction_start = integer_len + 1;
            let fraction_len = rest[fraction_start..]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
            anyhow::ensure!(
                integer_len > 0 || fraction_len > 0,
                "invalid ATTACHMENT_DOWNLOAD_URL_TTL {raw:?}"
            );
            number_len = fraction_start + fraction_len;
            fraction = Some(&rest[fraction_start..number_len]);
        } else {
            anyhow::ensure!(
                integer_len > 0,
                "invalid ATTACHMENT_DOWNLOAD_URL_TTL {raw:?}"
            );
        }

        let unit_text = &rest[number_len..];
        let (unit, unit_nanos) = [
            ("ns", 1_u128),
            ("us", 1_000),
            ("µs", 1_000),
            ("μs", 1_000),
            ("ms", 1_000_000),
            ("s", 1_000_000_000),
            ("m", 60 * 1_000_000_000),
            ("h", 60 * 60 * 1_000_000_000),
        ]
        .into_iter()
        .find(|(unit, _)| unit_text.starts_with(unit))
        .ok_or_else(|| anyhow::anyhow!("invalid ATTACHMENT_DOWNLOAD_URL_TTL {raw:?}"))?;

        let whole = if integer_len == 0 {
            0
        } else {
            rest[..integer_len].parse::<u128>()?
        };
        let mut segment = whole
            .checked_mul(unit_nanos)
            .ok_or_else(|| anyhow::anyhow!("ATTACHMENT_DOWNLOAD_URL_TTL is too large"))?;
        if let Some(fraction) = fraction.filter(|fraction| !fraction.is_empty()) {
            let scale = 10_u128
                .checked_pow(u32::try_from(fraction.len())?)
                .ok_or_else(|| anyhow::anyhow!("ATTACHMENT_DOWNLOAD_URL_TTL is too precise"))?;
            let fractional_nanos = fraction
                .parse::<u128>()?
                .checked_mul(unit_nanos)
                .ok_or_else(|| anyhow::anyhow!("ATTACHMENT_DOWNLOAD_URL_TTL is too large"))?
                / scale;
            segment = segment
                .checked_add(fractional_nanos)
                .ok_or_else(|| anyhow::anyhow!("ATTACHMENT_DOWNLOAD_URL_TTL is too large"))?;
        }
        total_nanos = total_nanos
            .checked_add(segment)
            .ok_or_else(|| anyhow::anyhow!("ATTACHMENT_DOWNLOAD_URL_TTL is too large"))?;
        rest = &unit_text[unit.len()..];
    }

    anyhow::ensure!(
        total_nanos > 0,
        "ATTACHMENT_DOWNLOAD_URL_TTL must be positive"
    );
    anyhow::ensure!(
        total_nanos <= i64::MAX as u128,
        "ATTACHMENT_DOWNLOAD_URL_TTL is too large"
    );
    Ok(std::time::Duration::from_nanos(u64::try_from(total_nanos)?))
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

fn fanout_workspace_event(hub: &Hub, event: &cordy_events::Event) {
    if event.workspace_id.is_empty() {
        return;
    }
    let Ok(frame) = serde_json::to_vec(&serde_json::json!({
        "type": event.event_type,
        "payload": event.payload,
        "actor_id": event.actor_id,
        "actor_type": event.actor_type,
    })) else {
        return;
    };
    let event_id = uuid::Uuid::now_v7().to_string();
    cordy_realtime::M.record_event(&event.event_type);
    hub.broadcast_to_scope_dedup(
        cordy_realtime::SCOPE_WORKSPACE,
        &event.workspace_id,
        &frame,
        &event_id,
    );
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
    /// Product analytics sink. A no-op client keeps tests and self-hosting inert.
    pub analytics: Arc<dyn cordy_analytics::AnalyticsClient>,
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
    pub model_catalog_cache: Option<Arc<crate::pending_store::ModelCatalogCache>>,
    pub local_skill_list_store: Option<Arc<crate::pending_store::LocalSkillListStore>>,
    pub local_skill_import_store: Option<Arc<crate::pending_store::LocalSkillImportStore>>,
    /// Shared Redis connection for per-IP public-route rate limiting. None is
    /// the Go nil-client path and deliberately fails open.
    pub rate_limit_client: Option<redis::Client>,
    /// Daemon WebSocket hub (cordy-daemon). `None` only in tests — the WS
    /// endpoint reports 503 and daemons fall back to HTTP polling.
    pub daemon_hub: Option<Arc<cordy_daemon::hub::DaemonHub>>,
    /// Shared attachment/object storage and its one download URL policy.
    /// #70 extends this seam; it must not create a second policy or signer.
    pub attachment_storage: Option<Arc<dyn crate::attachment_storage::AttachmentStorage>>,
    pub attachment_download: AttachmentDownloadSettings,
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
            analytics: Arc::new(cordy_analytics::NoopClient),
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
            model_catalog_cache: None,
            local_skill_list_store: None,
            local_skill_import_store: None,
            rate_limit_client: None,
            daemon_hub: Some(daemon_hub),
            attachment_storage: None,
            attachment_download: AttachmentDownloadSettings::default(),
            _task_wakeup: task_wakeup,
        }
    }

    pub fn with_attachment_storage(
        mut self,
        storage: Arc<dyn crate::attachment_storage::AttachmentStorage>,
        download: AttachmentDownloadSettings,
    ) -> Self {
        self.attachment_storage = Some(storage);
        self.attachment_download = download;
        self
    }

    /// Applies the merged TOML+environment realtime metrics credential. The
    /// constructor's environment fallback remains available to lightweight
    /// tests, while production uses the already-loaded config snapshot.
    pub fn with_realtime_metrics_token(mut self, raw: Option<&str>) -> Self {
        self.realtime_metrics_token = raw.unwrap_or_default().trim().to_string();
        self
    }

    pub fn with_observability(
        mut self,
        business_metrics: Option<Arc<cordy_metrics::BusinessMetrics>>,
        http_metrics: Option<Arc<cordy_metrics::HttpMetrics>>,
    ) -> Self {
        if let Some(metrics) = business_metrics.as_ref() {
            self.tasks.configure_metrics(metrics.clone());
        }
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

    /// Installs and starts the S7 GitHub snapshot manager. Applied snapshots
    /// are broadcast with the same weakest-role PR payload as the Go handler.
    pub fn with_github_snapshots(mut self, client: Option<cordy_ghsnapshot::Client>) -> Self {
        let pool = self.pool.clone();
        let event_pool = pool.clone();
        let bus = self.bus.clone();
        let hub = self.hub.clone();
        let on_applied: cordy_ghsnapshot::OnApplied = Arc::new(move |pull_request_id| {
            let pool = event_pool.clone();
            let bus = bus.clone();
            let hub = hub.clone();
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
                let event = cordy_events::Event {
                    event_type: cordy_protocol::EVENT_PULL_REQUEST_UPDATED.into(),
                    workspace_id: pull_request.workspace_id.to_string(),
                    actor_type: "system".into(),
                    payload: serde_json::json!({
                        "pull_request": payload,
                        "linked_issue_ids": issue_ids.into_iter().flatten().map(|id| id.to_string()).collect::<Vec<_>>(),
                    }),
                    ..Default::default()
                };
                bus.publish(&event);
                if let Some(hub) = hub.as_deref() {
                    fanout_workspace_event(hub, &event);
                }
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn attachment_ttl_preserves_go_duration_grammar() {
        assert_eq!(
            parse_attachment_ttl("1h30m").unwrap(),
            Duration::from_secs(90 * 60)
        );
        assert_eq!(
            parse_attachment_ttl("1.5h250ms").unwrap(),
            Duration::from_secs(90 * 60) + Duration::from_millis(250)
        );
        assert_eq!(
            parse_attachment_ttl("500us").unwrap(),
            Duration::from_micros(500)
        );
        for invalid in ["", "0", "-1s", "30", "1d", "1hour"] {
            assert!(parse_attachment_ttl(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn observability_wires_the_already_shared_task_service() {
        let pool =
            sqlx::PgPool::connect_lazy("postgres://invalid.invalid/nope").expect("lazy test pool");
        let state = HandlerState::new(pool, PatCache::disabled(), None);
        let original_tasks = state.tasks.clone();
        let metrics = Arc::new(cordy_metrics::BusinessMetrics::new());

        let state = state.with_observability(Some(metrics.clone()), None);

        assert!(Arc::ptr_eq(&state.tasks, &original_tasks));
        assert!(Arc::ptr_eq(
            state.tasks.metrics.get().expect("task metrics configured"),
            &metrics
        ));
    }

    #[test]
    fn snapshot_event_fanout_reaches_only_its_workspace() {
        let hub = Hub::new();
        let (_target_id, mut target_rx) = hub.register("user-1", "workspace-1");
        let (_other_id, mut other_rx) = hub.register("user-2", "workspace-2");
        let event = cordy_events::Event {
            event_type: cordy_protocol::EVENT_PULL_REQUEST_UPDATED.into(),
            workspace_id: "workspace-1".into(),
            actor_type: "system".into(),
            payload: serde_json::json!({"pull_request": {"id": "pr-1"}}),
            ..Default::default()
        };

        fanout_workspace_event(&hub, &event);

        let frame = target_rx.try_recv().expect("workspace event frame");
        let frame: serde_json::Value = serde_json::from_slice(&frame).expect("valid event json");
        assert_eq!(
            frame,
            serde_json::json!({
                "type": cordy_protocol::EVENT_PULL_REQUEST_UPDATED,
                "payload": {"pull_request": {"id": "pr-1"}},
                "actor_id": "",
                "actor_type": "system",
            })
        );
        assert!(matches!(
            other_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }
}
