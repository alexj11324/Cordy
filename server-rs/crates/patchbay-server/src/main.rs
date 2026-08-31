//! Patchbay HTTP server entry point.
//!
//! This is the S1 vertical slice from the migration plan: config loading,
//! pg pool, and health endpoints. Routes are ported domain-by-domain in
//! later steps (475 routes total, see tasks/go-to-rust-migration.md).

use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::prelude::*;

#[cfg(target_os = "linux")]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(target_os = "linux")]
#[allow(non_upper_case_globals)]
#[export_name = "malloc_conf"]
pub static malloc_conf: &[u8] = b"prof:true,prof_active:true,lg_prof_sample:19\0";

mod channel_bootstrap;
mod channel_runtime;
mod http_serve;
mod profiling;
mod realtime_runtime;

const HTTP_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
const LEGACY_CONFIG_FILENAME: &str = "cordy.toml"; // legacy-brand-compat

fn build_version() -> &'static str {
    env!("PATCHBAY_EFFECTIVE_BUILD_VERSION")
}

fn build_commit() -> &'static str {
    env!("PATCHBAY_EFFECTIVE_BUILD_COMMIT")
}

struct ProductionApp {
    router: Router,
    root_cancel: CancellationToken,
    realtime: realtime_runtime::RealtimeRuntime,
    channel_runtime: channel_runtime::ChannelRuntime,
    failure_monitor: patchbay_service::automation_failure_monitor::FailureMonitorRuntime,
    quota_reconciler: patchbay_service::automation_quota_reconciler::QuotaReconcilerRuntime,
    webhook_delivery: patchbay_handler::webhook_delivery_worker::WebhookDeliveryRuntime,
    coordinator: patchbay_service::coordination::CoordinatorRuntime,
    scheduler: patchbay_scheduler::ManagerRuntime,
    heartbeat_scheduler: patchbay_handler::heartbeat_scheduler::HeartbeatSchedulerRuntime,
    runtime_sweeper: patchbay_handler::runtime_sweeper::RuntimeSweeperRuntime,
    work_product_discovery: patchbay_handler::work_product::WorkProductDiscoveryRuntime,
    plugin_events: Option<patchbay_service::plugin_event_dispatch::PluginEventDispatcherRuntime>,
    github_snapshots: Option<patchbay_ghsnapshot::ManagerRuntime>,
    ordered_event_side_effects:
        Option<patchbay_handler::ordered_event_side_effects::OrderedEventSideEffectsRuntime>,
    automation_event_listeners:
        Option<patchbay_handler::automation_listeners::AutomationEventListenersRuntime>,
    task_side_effects: Option<patchbay_service::task_service::TaskSideEffectRuntime>,
    analytics: Arc<dyn patchbay_analytics::AnalyticsClient>,
}

struct VcsWebhookConfig {
    enabled: bool,
    secret_box: Option<patchbay_util::secretbox::SecretBox>,
}

struct MetricsRuntime {
    shutdown: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl MetricsRuntime {
    async fn shutdown(self) {
        if !self.shutdown_with_timeout(Duration::from_secs(3)).await {
            tracing::warn!("metrics server did not exit within shutdown timeout");
        }
    }

    async fn shutdown_with_timeout(self, timeout: Duration) -> bool {
        self.shutdown.cancel();
        let mut task = self.task;
        if tokio::time::timeout(timeout, &mut task).await.is_ok() {
            return true;
        }
        task.abort();
        let _ = task.await;
        false
    }
}

struct ProfilingRuntime {
    shutdown: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl ProfilingRuntime {
    async fn shutdown(self) {
        if !self.shutdown_with_timeout(Duration::from_secs(3)).await {
            tracing::warn!("pprof server did not exit within shutdown timeout");
        }
    }

    async fn shutdown_with_timeout(self, timeout: Duration) -> bool {
        self.shutdown.cancel();
        let mut task = self.task;
        if tokio::time::timeout(timeout, &mut task).await.is_ok() {
            return true;
        }
        task.abort();
        let _ = task.await;
        false
    }
}

impl VcsWebhookConfig {
    fn from_config(cfg: &patchbay_config::Config) -> Self {
        let enabled = cfg.integrations.vcs_integration_enabled.as_deref() == Some("true");
        let secret_box = patchbay_util::secretbox::load_key("PATCHBAY_VCS_SECRET_KEY")
            .ok()
            .and_then(|key| patchbay_util::secretbox::SecretBox::new(&key).ok());
        Self {
            enabled,
            secret_box,
        }
    }

    #[cfg(test)]
    fn disabled() -> Self {
        Self {
            enabled: false,
            secret_box: None,
        }
    }
}

fn github_snapshot_client(
    result: anyhow::Result<Option<patchbay_ghsnapshot::Client>>,
) -> Option<patchbay_ghsnapshot::Client> {
    match result {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(%error, "GitHub PR snapshot pipeline disabled by invalid configuration");
            None
        }
    }
}

fn parse_go_bool(raw: Option<&str>, default: bool) -> bool {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        None => default,
        Some("1" | "t" | "T" | "true" | "TRUE" | "True") => true,
        Some("0" | "f" | "F" | "false" | "FALSE" | "False") => false,
        Some(value) => {
            tracing::warn!(
                value,
                default,
                "invalid boolean environment value; using default"
            );
            default
        }
    }
}

fn parse_go_duration(raw: &str) -> Option<Duration> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with('-') {
        return None;
    }
    if raw == "0" {
        return Some(Duration::ZERO);
    }
    let bytes = raw.as_bytes();
    let mut cursor = 0;
    let mut seconds = 0.0_f64;
    while cursor < bytes.len() {
        let number_start = cursor;
        while cursor < bytes.len() && (bytes[cursor].is_ascii_digit() || bytes[cursor] == b'.') {
            cursor += 1;
        }
        if cursor == number_start {
            return None;
        }
        let value = raw[number_start..cursor].parse::<f64>().ok()?;
        let units = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ];
        let (unit, multiplier) = units
            .into_iter()
            .find(|(unit, _)| raw[cursor..].starts_with(unit))?;
        cursor += unit.len();
        seconds += value * multiplier;
    }
    (seconds.is_finite() && seconds >= 0.0 && seconds < Duration::MAX.as_secs_f64())
        .then(|| Duration::from_secs_f64(seconds))
}

fn duration_env(name: &str, default: Duration, allow_zero: bool) -> Duration {
    let Ok(raw) = std::env::var(name) else {
        return default;
    };
    match parse_go_duration(&raw).filter(|duration| allow_zero || !duration.is_zero()) {
        Some(duration) => duration,
        None => {
            tracing::warn!(
                name,
                value = raw,
                ?default,
                "invalid duration environment value; using default"
            );
            default
        }
    }
}

fn dedicated_sampler_pool(
    cfg: &patchbay_config::DatabaseConfig,
) -> Option<patchbay_metrics::sampler::BusinessSamplerOptions> {
    let url = cfg.url.as_deref()?;
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_lazy(url)
        .ok()
        .map(|pool| patchbay_metrics::sampler::BusinessSamplerOptions {
            pool: Arc::new(pool),
            cache_ttl: None,
            query_timeout: None,
        })
}

fn automation_entitlements(
    cfg: &patchbay_config::Config,
) -> Option<Arc<dyn patchbay_service::automation::EntitlementProvider>> {
    let enabled = parse_go_bool(
        std::env::var("PATCHBAY_ENTITLEMENT_POLICY_ENABLED")
            .ok()
            .as_deref(),
        false,
    );
    let emergency_disabled = parse_go_bool(
        std::env::var("PATCHBAY_ENTITLEMENT_EMERGENCY_DISABLED")
            .ok()
            .as_deref(),
        false,
    );
    let service_token = std::env::var("PATCHBAY_ENTITLEMENT_SERVICE_TOKEN")
        .ok()
        .or_else(|| cfg.entitlement.service_token.clone())
        .unwrap_or_default();
    let config = patchbay_service::entitlement::EntitlementClientConfig {
        enabled,
        base_url: cfg.entitlement.policy_url.clone().unwrap_or_default(),
        service_token,
        timeout: duration_env(
            "PATCHBAY_ENTITLEMENT_POLICY_TIMEOUT",
            Duration::from_secs(3),
            false,
        ),
        stale_grace: duration_env(
            "PATCHBAY_ENTITLEMENT_STALE_GRACE",
            Duration::from_secs(15 * 60),
            true,
        ),
        emergency_disabled,
    };
    match patchbay_service::entitlement::HttpEntitlementProvider::new(config) {
        Ok(provider) => provider.map(|provider| provider as Arc<_>),
        Err(error) => {
            tracing::error!(%error, "entitlement policy client disabled by invalid configuration");
            None
        }
    }
}

#[cfg(test)]
fn build_router(db: Option<sqlx::PgPool>, hub: Option<Arc<patchbay_realtime::hub::Hub>>) -> Router {
    patchbay_handler::build_router(db, hub)
}

fn install_pending_stores(
    state: patchbay_handler::HandlerState,
    redis_url: Option<&str>,
) -> patchbay_handler::HandlerState {
    let Some(redis_url) = redis_url.map(str::trim).filter(|url| !url.is_empty()) else {
        return state.with_in_memory_pending_stores();
    };
    let client = match redis::Client::open(redis_url) {
        Ok(client) => client,
        Err(_) => {
            tracing::warn!("REDIS_URL is invalid; Redis caches and stores are disabled");
            return state;
        }
    };
    state.with_redis(client)
}

fn validate_shared_desktop_handoff_redis(
    required: bool,
    redis_url: Option<&str>,
) -> anyhow::Result<()> {
    if !required {
        return Ok(());
    }
    let redis_url = redis_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "REDIS_URL must be configured when shared desktop handoff storage is required"
            )
        })?;
    redis::Client::open(redis_url).map_err(|_| {
        anyhow::anyhow!(
            "REDIS_URL must be a valid Redis URL when shared desktop handoff storage is required"
        )
    })?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn build_production_router(
    db: sqlx::PgPool,
    hub: Arc<patchbay_realtime::hub::Hub>,
    business_metrics: Option<Arc<patchbay_metrics::BusinessMetrics>>,
    http_metrics: Option<Arc<patchbay_metrics::HttpMetrics>>,
    channel_lease_metrics: Option<Arc<patchbay_metrics::ChannelLeaseMetrics>>,
    channel_media_metrics: Option<Arc<patchbay_metrics::ChannelMediaReconcilerMetrics>>,
    wecom_metrics: Option<Arc<patchbay_metrics::WecomMetrics>>,
    lark_backfill_metrics: Option<Arc<patchbay_metrics::LarkBackfillMetrics>>,
    github_client: Option<patchbay_ghsnapshot::Client>,
    cfg: &patchbay_config::Config,
    attachment_storage: Arc<dyn patchbay_handler::attachment_storage::AttachmentStorage>,
    attachment_frame_ancestors: Vec<String>,
    vcs: VcsWebhookConfig,
) -> anyhow::Result<ProductionApp> {
    let feature_flags = Arc::new(patchbay_service::feature_flags::ConfiguredFlags::from_env()?);
    let entitlements = automation_entitlements(cfg);
    let attachment_download =
        patchbay_handler::state::AttachmentDownloadSettings::from_config(cfg).await?;
    attachment_download.validate_for_storage(attachment_storage.as_ref())?;
    let cdn_signed = attachment_download.cloudfront_signer.is_some();
    let analytics: Arc<dyn patchbay_analytics::AnalyticsClient> =
        Arc::from(patchbay_analytics::new_from_env());
    let state = patchbay_handler::HandlerState::new_with_production_dependencies(
        db,
        patchbay_auth::pat_cache::PatCache::disabled(),
        Some(hub.clone()),
        analytics.clone(),
        feature_flags,
        business_metrics.clone(),
    )
    .with_observability(business_metrics, http_metrics)
    .with_invitation_admission(patchbay_handler::invitation::InvitationAdmission::from_env())
    .with_automation_entitlements(entitlements)
    .with_github_snapshots(github_client)
    .with_auth_settings(patchbay_handler::auth::AuthSettings::from_config(cfg))
    .with_clerk_auth_from_config(&cfg.auth)?
    .with_cloud_pat_fleet_url(
        cfg.fleet
            .cloud_fleet_url
            .as_deref()
            .or(cfg.fleet.fleet_url.as_deref()),
    )
    .with_email_service(Arc::new(
        patchbay_service::email::EmailService::from_config_values(
            cfg.email.resend_api_key.as_deref(),
            cfg.email.smtp_host.as_deref(),
        ),
    ))
    .with_rate_limit_trusted_proxies(cfg.urls.rate_limit_trusted_proxies.as_deref())
    .with_attachment_storage(
        attachment_storage,
        attachment_frame_ancestors,
        attachment_download,
    )
    .with_plugins_from_env()
    .with_slack_history_from_env()
    .with_llm_from_config(&cfg.llm)?
    .with_composio_from_config(cfg)
    .with_public_config(patchbay_handler::config::PublicConfigSettings::from_config(
        cfg,
        cfg.storage.cloudfront_domain.clone().unwrap_or_default(),
        cdn_signed,
        build_version().to_string(),
    ))
    .with_vcs_webhooks(vcs.enabled, vcs.secret_box);
    let redis_url = cfg
        .redis
        .url
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let require_shared_desktop_handoff =
        std::env::var("PATCHBAY_REQUIRE_SHARED_DESKTOP_HANDOFF").as_deref() == Ok("true");
    validate_shared_desktop_handoff_redis(require_shared_desktop_handoff, redis_url)?;
    let root_cancel = CancellationToken::new();
    let state =
        install_pending_stores(state, redis_url).with_channel_cancel(root_cancel.child_token());
    // Go registers realtime fanout before subscriber/activity/notification
    // side effects. Preserve that callback order while retaining async relay
    // ownership outside the handler state.
    let mut realtime = realtime_runtime::RealtimeRuntime::from_config(hub, &cfg.redis).await;
    realtime.attach(
        &state.bus,
        state.daemon_hub.clone(),
        state.daemon_notifier.clone(),
    );
    // These consumers drain after every root-owned producer has joined. Give
    // them dedicated cancellation roots so the process root cannot close a
    // FIFO while a producer is publishing a final event during shutdown.
    let (state, ordered_event_side_effects) =
        state.start_ordered_event_side_effects(CancellationToken::new());
    // Automation reconciliation is the final stage of the ordered consumer;
    // do not register a second production subscriber for the same events.
    let automation_event_listeners = None;
    // Event-hook delivery is a consumer lifecycle. It is stopped explicitly
    // after every event producer/listener has drained, rather than sharing the
    // producer root and racing their final publications.
    let (state, plugin_events) = state.start_plugin_event_dispatcher(CancellationToken::new());
    let github_snapshots = state.github_snapshots.start(root_cancel.child_token());
    let heartbeat_scheduler = Arc::new(
        patchbay_handler::heartbeat_scheduler::BatchedHeartbeatScheduler::new(
            state.pool.clone(),
            patchbay_handler::heartbeat_scheduler::DEFAULT_BATCH_INTERVAL,
        ),
    );
    let state = state
        .with_heartbeat_scheduler(heartbeat_scheduler.clone())
        .with_daemon_heartbeat_handler()
        .with_daemon_rpc_handler();
    let (state, webhook_worker) = state.prepare_webhook_delivery_worker();
    let (state, coordinator) = state.start_coordinator(root_cancel.child_token());
    let task_side_effects = state
        .tasks
        .start_side_effect_runtime(root_cancel.child_token());
    let heartbeat_scheduler = heartbeat_scheduler.start(root_cancel.child_token());
    let work_product_discovery = patchbay_handler::work_product::WorkProductDiscoveryRuntime::start(
        state.clone(),
        root_cancel.child_token(),
    );
    let configured_reconnect_grace = duration_env(
        "PATCHBAY_RUNTIME_RECONNECT_GRACE",
        patchbay_handler::runtime_sweeper::DEFAULT_RECONNECT_GRACE,
        false,
    );
    let runtime_reconnect_grace =
        configured_reconnect_grace.max(patchbay_handler::runtime_sweeper::MINIMUM_RECONNECT_GRACE);
    if runtime_reconnect_grace != configured_reconnect_grace {
        tracing::warn!(
            configured = ?configured_reconnect_grace,
            minimum = ?patchbay_handler::runtime_sweeper::MINIMUM_RECONNECT_GRACE,
            "runtime reconnect grace is shorter than heartbeat freshness; clamping"
        );
    }
    let runtime_sweeper = Arc::new(patchbay_handler::runtime_sweeper::RuntimeTaskSweeper::new(
        state.pool.clone(),
        state.liveness_store.clone(),
        state.tasks.clone(),
        state.bus.clone(),
        state.business_metrics.clone(),
        runtime_reconnect_grace,
    ))
    .start(root_cancel.child_token());
    let failure_metrics = state.business_metrics.clone().map(|metrics| {
        metrics as Arc<dyn patchbay_service::automation_failure_monitor::FailureMonitorMetrics>
    });
    let failure_monitor =
        patchbay_service::automation_failure_monitor::AutomationFailureMonitor::new(
            state.pool.clone(),
            state.bus.clone(),
            failure_metrics,
            patchbay_service::automation_failure_monitor::FailureMonitorConfig::from_env(),
        )
        .start(root_cancel.child_token());
    let quota_metrics = state.business_metrics.clone().map(|metrics| {
        metrics as Arc<dyn patchbay_service::automation_quota_reconciler::QuotaReconcilerMetrics>
    });
    let quota_reconciler =
        patchbay_service::automation_quota_reconciler::AutomationQuotaReconciler::new(
            state.automations.clone(),
            quota_metrics,
        )
        .start(root_cancel.child_token());
    let webhook_delivery = webhook_worker.start(root_cancel.child_token());
    let channel_runtime = channel_runtime::ChannelRuntime::start(
        &state,
        cfg,
        channel_lease_metrics,
        channel_media_metrics,
        wecom_metrics,
        lark_backfill_metrics,
    )
    .await?;
    let scheduler =
        patchbay_scheduler::production_manager(state.pool.clone(), state.automations.clone())?;
    let scheduler = scheduler.start(root_cancel.child_token())?;
    Ok(ProductionApp {
        router: patchbay_handler::build_router_from_state(state),
        root_cancel,
        realtime,
        channel_runtime,
        failure_monitor,
        quota_reconciler,
        webhook_delivery,
        coordinator,
        scheduler,
        heartbeat_scheduler,
        runtime_sweeper,
        work_product_discovery,
        plugin_events,
        github_snapshots,
        ordered_event_side_effects,
        automation_event_listeners,
        task_side_effects,
        analytics,
    })
}

async fn shutdown_plugin_events(
    runtime: Option<patchbay_service::plugin_event_dispatch::PluginEventDispatcherRuntime>,
) -> Option<patchbay_service::plugin_event_dispatch::PluginEventShutdownOutcome> {
    match runtime {
        Some(runtime) => Some(
            runtime
                .shutdown(patchbay_service::plugin_event_dispatch::DEFAULT_SHUTDOWN_TIMEOUT)
                .await,
        ),
        None => None,
    }
}

async fn shutdown_github_snapshots(
    runtime: Option<patchbay_ghsnapshot::ManagerRuntime>,
) -> Option<patchbay_ghsnapshot::ManagerShutdownOutcome> {
    match runtime {
        Some(runtime) => Some(
            runtime
                .shutdown(patchbay_ghsnapshot::DEFAULT_SHUTDOWN_TIMEOUT)
                .await,
        ),
        None => None,
    }
}

async fn shutdown_ordered_event_side_effects(
    runtime: Option<patchbay_handler::ordered_event_side_effects::OrderedEventSideEffectsRuntime>,
) -> Option<patchbay_handler::ordered_event_side_effects::OrderedEventShutdownOutcome> {
    match runtime {
        Some(runtime) => Some(
            runtime
                .shutdown(patchbay_handler::ordered_event_side_effects::DEFAULT_SHUTDOWN_TIMEOUT)
                .await,
        ),
        None => None,
    }
}

async fn shutdown_automation_event_listeners(
    runtime: Option<patchbay_handler::automation_listeners::AutomationEventListenersRuntime>,
) -> Option<patchbay_handler::automation_listeners::AutomationEventShutdownOutcome> {
    match runtime {
        Some(runtime) => Some(
            runtime
                .shutdown(patchbay_handler::automation_listeners::DEFAULT_SHUTDOWN_TIMEOUT)
                .await,
        ),
        None => None,
    }
}

fn validate_auth_config(cfg: &patchbay_config::Config) -> anyhow::Result<()> {
    if cfg.is_production() {
        patchbay_auth::jwt::validate_jwt_secret(cfg.auth.jwt_secret.as_deref().unwrap_or(""))
            .map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    patchbay_util::branding::install_legacy_env_aliases();
    patchbay_util::install_rustls_crypto_provider()?;
    let log_filter = tracing_subscriber::EnvFilter::try_new(patchbay_util::logging::env_filter())
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug"));
    let console_addr: SocketAddr = profiling::TOKIO_CONSOLE_ADDR.parse()?;
    let console_layer = console_subscriber::ConsoleLayer::builder()
        .server_addr(console_addr)
        .spawn();
    let log_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
            patchbay_util::logging::LOCAL_TIME_FORMAT.to_string(),
        ))
        .with_ansi(patchbay_util::logging::stderr_is_terminal())
        .with_filter(log_filter);
    tracing_subscriber::registry()
        .with(console_layer)
        .with(log_layer)
        .init();

    let config_path = if std::path::Path::new("patchbay.toml").exists() {
        std::path::Path::new("patchbay.toml")
    } else if std::path::Path::new(LEGACY_CONFIG_FILENAME).exists() {
        std::path::Path::new(LEGACY_CONFIG_FILENAME)
    } else {
        std::path::Path::new("patchbay.toml")
    };
    let cfg = patchbay_config::Config::load(Some(config_path))?;
    cfg.validate()?;
    validate_auth_config(&cfg)?;
    patchbay_auth::jwt::configure_jwt_secret(cfg.auth.jwt_secret.as_deref())?;
    patchbay_auth::cookie::configure_auth_token_ttl(cfg.auth.auth_token_ttl.as_deref())?;
    tracing::info!(port = cfg.server.port, "starting patchbay-server");

    let db = patchbay_db::connect(&cfg.database).await?;
    let hub = Arc::new(patchbay_realtime::hub::Hub::new());
    let metrics_config = patchbay_metrics::Config::from_env();
    let (
        business_metrics,
        http_metrics,
        channel_lease_metrics,
        channel_media_metrics,
        wecom_metrics,
        lark_backfill_metrics,
        metrics_runtime,
    ) = if metrics_config.enabled() {
        let registry =
            patchbay_metrics::Registry::new(patchbay_metrics::registry::RegistryOptions {
                pool: Some(Arc::new(db.clone())),
                realtime: Some(&patchbay_realtime::M),
                daemonws: Some(&patchbay_daemon::hub::M),
                version: build_version().to_string(),
                commit: build_commit().to_string(),
                sampler: dedicated_sampler_pool(&cfg.database),
            });
        let business = registry.business.clone();
        let http = registry.http.clone();
        let channel_lease = registry.channel_lease.clone();
        let channel_media = registry.channel_media.clone();
        let wecom = registry.wecom.clone();
        let lark_backfill = registry.lark_backfill.clone();
        let gatherer = Arc::new(registry.gatherer.clone());
        let metrics_addr = metrics_config.addr.clone();
        let effective_metrics_addr = patchbay_metrics::server::normalized_bind_addr(&metrics_addr);
        if !patchbay_metrics::is_loopback_addr(&effective_metrics_addr) {
            tracing::warn!(addr = %metrics_addr, "metrics listener is not loopback-only; restrict access with private networking, allowlists, or proxy auth");
        }
        let shutdown = tokio_util::sync::CancellationToken::new();
        let serve_shutdown = shutdown.clone();
        let task = tokio::spawn(async move {
            if let Err(error) =
                patchbay_metrics::server::serve(metrics_addr, gatherer, serve_shutdown).await
            {
                tracing::error!(%error, "metrics server stopped");
            }
        });
        (
            Some(business),
            Some(http),
            Some(channel_lease),
            Some(channel_media),
            Some(wecom),
            Some(lark_backfill),
            Some(MetricsRuntime { shutdown, task }),
        )
    } else {
        (None, None, None, None, None, None, None)
    };
    let github_client = github_snapshot_client(patchbay_ghsnapshot::Client::new_from_env());
    let attachment_storage = patchbay_handler::attachment_storage::from_env(
        cfg.storage.local_upload_dir.as_deref(),
        cfg.storage.local_upload_base_url.as_deref(),
    )
    .await?;
    let attachment_frame_ancestors = cfg
        .urls
        .cors_allowed_origins
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .chain(cfg.urls.frontend_origin.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    let vcs = VcsWebhookConfig::from_config(&cfg);
    let app = build_production_router(
        db,
        hub,
        business_metrics,
        http_metrics,
        channel_lease_metrics,
        channel_media_metrics,
        wecom_metrics,
        lark_backfill_metrics,
        github_client,
        &cfg,
        attachment_storage,
        attachment_frame_ancestors,
        vcs,
    )
    .await?;

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.server.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");
    let profiling_shutdown = tokio_util::sync::CancellationToken::new();
    let profiling_serve_shutdown = profiling_shutdown.clone();
    let profiling_task = tokio::spawn(async move {
        if let Err(error) = profiling::serve(profiling_serve_shutdown).await {
            tracing::error!(%error, "pprof server stopped");
        }
    });
    let profiling_runtime = ProfilingRuntime {
        shutdown: profiling_shutdown,
        task: profiling_task,
    };
    let ProductionApp {
        router,
        root_cancel,
        realtime,
        channel_runtime,
        failure_monitor,
        quota_reconciler,
        webhook_delivery,
        coordinator,
        scheduler,
        heartbeat_scheduler,
        runtime_sweeper,
        work_product_discovery,
        plugin_events,
        github_snapshots,
        ordered_event_side_effects,
        automation_event_listeners,
        task_side_effects,
        analytics,
    } = app;
    let http_shutdown = CancellationToken::new();
    let shutdown_hold = duration_env("PATCHBAY_SHUTDOWN_HOLD_DURATION", Duration::ZERO, true);
    let mut server = std::pin::pin!(http_serve::serve_with_bounded_drain(
        listener,
        router,
        http_shutdown.clone(),
        HTTP_DRAIN_TIMEOUT,
    ));
    let mut http_drain_timed_out = false;
    let serve_result = tokio::select! {
        result = server.as_mut() => result.map(|timed_out| {
            http_drain_timed_out = timed_out;
        }),
        () = shutdown_signal(shutdown_hold) => {
            http_shutdown.cancel();
            server.as_mut().await.map(|timed_out| {
                http_drain_timed_out = timed_out;
            })
        }
    };
    if http_drain_timed_out {
        tracing::warn!(
            timeout_seconds = HTTP_DRAIN_TIMEOUT.as_secs(),
            "HTTP server did not drain within shutdown timeout; aborted remaining connections"
        );
    }
    // Match Go's shutdown ordering: drain every in-flight HTTP handler before
    // stopping maintenance workers. Channel adapters are producers and must
    // drain while realtime fanout is still accepting their final events.
    channel_runtime.shutdown().await;
    // In particular, a heartbeat must not queue an ID after the batched
    // scheduler has performed its final flush.
    root_cancel.cancel();
    let (
        failure_shutdown,
        quota_shutdown,
        webhook_shutdown,
        coordinator_shutdown,
        scheduler_shutdown,
        heartbeat_shutdown,
        runtime_sweeper_shutdown,
        _work_product_discovery_shutdown,
        github_snapshots_shutdown,
    ) = tokio::join!(
        failure_monitor
            .shutdown(patchbay_service::automation_failure_monitor::DEFAULT_SHUTDOWN_TIMEOUT),
        quota_reconciler
            .shutdown(patchbay_service::automation_quota_reconciler::DEFAULT_SHUTDOWN_TIMEOUT),
        webhook_delivery
            .shutdown(patchbay_handler::webhook_delivery_worker::DEFAULT_SHUTDOWN_TIMEOUT),
        coordinator.shutdown(patchbay_service::coordination::DEFAULT_SHUTDOWN_TIMEOUT),
        scheduler.shutdown(),
        heartbeat_scheduler.shutdown(),
        runtime_sweeper.shutdown(),
        work_product_discovery.shutdown(),
        shutdown_github_snapshots(github_snapshots),
    );
    let task_side_effects_shutdown = match task_side_effects {
        Some(runtime) => Some(
            runtime
                .shutdown(patchbay_service::task_service::DEFAULT_SIDE_EFFECT_SHUTDOWN_TIMEOUT)
                .await,
        ),
        None => None,
    };
    let automation_event_listeners_shutdown =
        shutdown_automation_event_listeners(automation_event_listeners).await;
    // Subscriber/activity/notification work consumes events from every
    // producer and listener above. Stop accepting only after those producers
    // have joined, then drain already-admitted events in subscriber → activity
    // → notification order.
    let ordered_event_side_effects_shutdown =
        shutdown_ordered_event_side_effects(ordered_event_side_effects).await;
    let plugin_events_shutdown = shutdown_plugin_events(plugin_events).await;
    // Realtime registered first and is the last event consumer to stop. Its
    // shutdown drains handler background work, forwarder, and Redis relay.
    realtime.shutdown().await;
    // Every analytics producer above has stopped. Close the shared client last
    // so its owned worker can flush the final bounded queue before process exit.
    analytics.close().await;
    match failure_shutdown {
        patchbay_service::automation_failure_monitor::ShutdownOutcome::TimedOut => {
            tracing::warn!("automation failure monitor exceeded shutdown deadline and was aborted");
        }
        patchbay_service::automation_failure_monitor::ShutdownOutcome::Panicked => {
            tracing::error!("automation failure monitor task panicked during shutdown");
        }
        _ => {}
    }
    match quota_shutdown {
        patchbay_service::automation_failure_monitor::ShutdownOutcome::TimedOut => {
            tracing::warn!(
                "automation quota reconciler exceeded shutdown deadline and was aborted"
            );
        }
        patchbay_service::automation_failure_monitor::ShutdownOutcome::Panicked => {
            tracing::error!("automation quota reconciler task panicked during shutdown");
        }
        _ => {}
    }
    match webhook_shutdown {
        patchbay_handler::webhook_delivery_worker::WebhookShutdownOutcome::TimedOut => {
            tracing::warn!("webhook delivery worker exceeded shutdown deadline and was aborted");
        }
        patchbay_handler::webhook_delivery_worker::WebhookShutdownOutcome::Panicked => {
            tracing::error!("webhook delivery worker supervisor panicked during shutdown");
        }
        _ => {}
    }
    match coordinator_shutdown {
        patchbay_service::coordination::CoordinatorShutdownOutcome::TimedOut => {
            tracing::warn!("coordinator exceeded shutdown deadline and was aborted");
        }
        patchbay_service::coordination::CoordinatorShutdownOutcome::Panicked => {
            tracing::error!("coordinator task panicked during shutdown");
        }
        patchbay_service::coordination::CoordinatorShutdownOutcome::Stopped => {}
    }
    match scheduler_shutdown {
        patchbay_scheduler::ShutdownOutcome::TimedOut => {
            tracing::warn!("scheduler exceeded shutdown deadline and was aborted");
        }
        patchbay_scheduler::ShutdownOutcome::Panicked => {
            tracing::error!("scheduler task panicked during shutdown");
        }
        patchbay_scheduler::ShutdownOutcome::Stopped => {}
    }
    match heartbeat_shutdown {
        patchbay_handler::heartbeat_scheduler::HeartbeatShutdownOutcome::TimedOut => {
            tracing::warn!("heartbeat scheduler exceeded shutdown deadline and was aborted");
        }
        patchbay_handler::heartbeat_scheduler::HeartbeatShutdownOutcome::Panicked => {
            tracing::error!("heartbeat scheduler task panicked during shutdown");
        }
        patchbay_handler::heartbeat_scheduler::HeartbeatShutdownOutcome::Stopped => {}
    }
    match runtime_sweeper_shutdown {
        patchbay_handler::runtime_sweeper::RuntimeSweeperShutdownOutcome::TimedOut => {
            tracing::warn!("runtime sweeper exceeded shutdown deadline and was aborted");
        }
        patchbay_handler::runtime_sweeper::RuntimeSweeperShutdownOutcome::Panicked => {
            tracing::error!("runtime sweeper task panicked during shutdown");
        }
        patchbay_handler::runtime_sweeper::RuntimeSweeperShutdownOutcome::Stopped => {}
    }
    match plugin_events_shutdown {
        Some(patchbay_service::plugin_event_dispatch::PluginEventShutdownOutcome::TimedOut) => {
            tracing::warn!("plugin event dispatcher exceeded shutdown deadline and was aborted");
        }
        Some(patchbay_service::plugin_event_dispatch::PluginEventShutdownOutcome::Panicked) => {
            tracing::error!("plugin event dispatcher supervisor panicked during shutdown");
        }
        Some(patchbay_service::plugin_event_dispatch::PluginEventShutdownOutcome::Stopped)
        | None => {}
    }
    match github_snapshots_shutdown {
        Some(patchbay_ghsnapshot::ManagerShutdownOutcome::TimedOut) => {
            tracing::warn!("GitHub snapshot manager exceeded shutdown deadline and was aborted");
        }
        Some(patchbay_ghsnapshot::ManagerShutdownOutcome::Panicked) => {
            tracing::error!("GitHub snapshot manager task panicked during shutdown");
        }
        Some(patchbay_ghsnapshot::ManagerShutdownOutcome::Stopped) | None => {}
    }
    match ordered_event_side_effects_shutdown {
        Some(
            patchbay_handler::ordered_event_side_effects::OrderedEventShutdownOutcome::TimedOut,
        ) => {
            tracing::warn!(
                "ordered event side effects exceeded shutdown deadline and were aborted"
            );
        }
        Some(
            patchbay_handler::ordered_event_side_effects::OrderedEventShutdownOutcome::Panicked,
        ) => {
            tracing::error!("ordered event side-effect task panicked during shutdown");
        }
        Some(
            patchbay_handler::ordered_event_side_effects::OrderedEventShutdownOutcome::Stopped,
        )
        | None => {}
    }
    match automation_event_listeners_shutdown {
        Some(patchbay_handler::automation_listeners::AutomationEventShutdownOutcome::TimedOut) => {
            tracing::warn!(
                "automation event listeners exceeded shutdown deadline and were aborted"
            );
        }
        Some(patchbay_handler::automation_listeners::AutomationEventShutdownOutcome::Panicked) => {
            tracing::error!("automation event listener task panicked during shutdown");
        }
        Some(patchbay_handler::automation_listeners::AutomationEventShutdownOutcome::Stopped)
        | None => {}
    }
    match task_side_effects_shutdown {
        Some(patchbay_service::task_service::TaskSideEffectShutdownOutcome::TimedOut) => {
            tracing::warn!("task side effects exceeded shutdown deadline and were aborted");
        }
        Some(patchbay_service::task_service::TaskSideEffectShutdownOutcome::Panicked) => {
            tracing::error!("task side-effect worker panicked during shutdown");
        }
        Some(patchbay_service::task_service::TaskSideEffectShutdownOutcome::Stopped) | None => {}
    }
    if let Some(metrics_runtime) = metrics_runtime {
        metrics_runtime.shutdown().await;
    }
    profiling_runtime.shutdown().await;
    serve_result?;
    Ok(())
}

async fn wait_for_shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to install Ctrl-C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(%error, "failed to install SIGTERM handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

async fn shutdown_signal(hold: Duration) {
    wait_for_shutdown_signal().await;
    if hold.is_zero() {
        tracing::info!("shutdown signal received; draining HTTP server");
        return;
    }

    tracing::info!(
        hold_seconds = hold.as_secs_f64(),
        "shutdown signal received; holding admission before drain"
    );
    tokio::select! {
        () = tokio::time::sleep(hold) => {
            tracing::info!("shutdown hold complete; draining HTTP server");
        }
        () = wait_for_shutdown_signal() => {
            tracing::warn!("second shutdown signal received; skipping shutdown hold");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Notify;
    use tower::ServiceExt;

    struct DropSignal(Arc<AtomicBool>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    fn test_attachment_storage() -> Arc<dyn patchbay_handler::attachment_storage::AttachmentStorage>
    {
        Arc::new(
            patchbay_handler::attachment_storage::LocalStorage::new(
                std::env::temp_dir().join("patchbay-server-route-tests"),
                String::new(),
            )
            .expect("test local storage"),
        )
    }

    #[test]
    fn vcs_production_configuration_matches_go_exact_true_and_fails_closed() {
        const KEY_ENV: &str = "PATCHBAY_VCS_SECRET_KEY";
        let original = std::env::var_os(KEY_ENV);
        let mut cfg = patchbay_config::Config::default();

        std::env::remove_var(KEY_ENV);
        cfg.integrations.vcs_integration_enabled = Some("1".into());
        let noncanonical_flag = VcsWebhookConfig::from_config(&cfg);
        assert!(!noncanonical_flag.enabled);
        assert!(noncanonical_flag.secret_box.is_none());

        cfg.integrations.vcs_integration_enabled = Some("true".into());
        let missing_key = VcsWebhookConfig::from_config(&cfg);
        assert!(missing_key.enabled);
        assert!(missing_key.secret_box.is_none());

        std::env::set_var(KEY_ENV, "not-base64");
        let invalid_key = VcsWebhookConfig::from_config(&cfg);
        assert!(invalid_key.enabled);
        assert!(invalid_key.secret_box.is_none());

        std::env::set_var(KEY_ENV, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
        let configured = VcsWebhookConfig::from_config(&cfg);
        assert!(configured.enabled);
        assert!(configured.secret_box.is_some());

        cfg.integrations.vcs_integration_enabled = Some("false".into());
        let disabled = VcsWebhookConfig::from_config(&cfg);
        assert!(!disabled.enabled);
        assert!(disabled.secret_box.is_some());

        match original {
            Some(value) => std::env::set_var(KEY_ENV, value),
            None => std::env::remove_var(KEY_ENV),
        }
    }

    #[test]
    fn invalid_github_snapshot_credentials_disable_only_the_pipeline() {
        let client = github_snapshot_client(Err(anyhow::anyhow!("invalid private key")));
        assert!(client.is_none());
    }

    #[tokio::test]
    async fn health_reports_ok_without_db() {
        let app = build_router(None, None);
        let res = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn readyz_is_503_without_db() {
        let app = build_router(None, None);
        let res = app
            .oneshot(Request::get("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn metrics_shutdown_aborts_a_stalled_server_task() {
        let shutdown = tokio_util::sync::CancellationToken::new();
        let entered = Arc::new(Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn({
            let entered = entered.clone();
            let dropped = dropped.clone();
            async move {
                let _drop_signal = DropSignal(dropped);
                entered.notify_one();
                std::future::pending::<()>().await;
            }
        });
        entered.notified().await;

        let runtime = MetricsRuntime { shutdown, task };
        assert!(!runtime.shutdown_with_timeout(Duration::ZERO).await);
        assert!(dropped.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn unavailable_rate_limit_redis_fails_open() {
        let db = sqlx::PgPool::connect_lazy("postgres://invalid/invalid")
            .unwrap_or_else(|_| unreachable!("static test URL is valid"));
        let mut cfg = patchbay_config::Config::default();
        cfg.redis.url = Some("redis://127.0.0.1:1/".into());
        let ProductionApp {
            router,
            root_cancel,
            realtime,
            channel_runtime,
            ..
        } = build_production_router(
            db,
            Arc::new(patchbay_realtime::hub::Hub::new()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            &cfg,
            test_attachment_storage(),
            Vec::new(),
            VcsWebhookConfig::disabled(),
        )
        .await
        .unwrap();
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            router.oneshot(
                Request::post("/api/contact-sales")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            ),
        )
        .await
        .expect("unavailable Redis must not block the request")
        .expect("response");
        channel_runtime.shutdown().await;
        root_cancel.cancel();
        realtime.shutdown().await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unresponsive_rate_limit_redis_fails_open() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind black-hole Redis");
        let address = listener.local_addr().expect("listener address");
        let black_hole = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("accept Redis client");
            std::future::pending::<()>().await;
        });
        let db = sqlx::PgPool::connect_lazy("postgres://invalid/invalid")
            .unwrap_or_else(|_| unreachable!("static test URL is valid"));
        let redis_url = format!("redis://{address}/");
        let mut cfg = patchbay_config::Config::default();
        cfg.redis.url = Some(redis_url);
        let ProductionApp {
            router,
            root_cancel,
            realtime,
            channel_runtime,
            ..
        } = build_production_router(
            db,
            Arc::new(patchbay_realtime::hub::Hub::new()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            &cfg,
            test_attachment_storage(),
            Vec::new(),
            VcsWebhookConfig::disabled(),
        )
        .await
        .unwrap();
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            router.oneshot(
                Request::post("/api/contact-sales")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            ),
        )
        .await
        .expect("unresponsive Redis must not block the request")
        .expect("response");
        black_hole.abort();
        channel_runtime.shutdown().await;
        root_cancel.cancel();
        realtime.shutdown().await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn production_rejects_missing_or_insecure_jwt_secrets() {
        let mut cfg = patchbay_config::Config::default();
        cfg.server.app_env = Some("production".into());
        assert!(validate_auth_config(&cfg).is_err());

        cfg.auth.jwt_secret = Some("patchbay-dev-secret-change-in-production".into());
        assert!(validate_auth_config(&cfg).is_err());

        cfg.auth.jwt_secret = Some("a-long-random-production-secret".into());
        assert!(validate_auth_config(&cfg).is_ok());
    }

    #[tokio::test]
    async fn invalid_redis_config_keeps_pending_stores_fail_closed() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let state = patchbay_handler::HandlerState::new(
            pool,
            patchbay_auth::pat_cache::PatCache::disabled(),
            None,
        );
        let state = install_pending_stores(state, Some("not-a-redis-url"));
        assert!(state.update_store.is_none());
        assert!(state.model_list_store.is_none());
        assert!(state.model_catalog_cache.is_none());
        assert!(state.local_skill_list_store.is_none());
        assert!(state.local_skill_import_store.is_none());
    }

    #[tokio::test]
    async fn redis_free_configuration_installs_in_memory_pending_stores() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let state = patchbay_handler::HandlerState::new(
            pool,
            patchbay_auth::pat_cache::PatCache::disabled(),
            None,
        );
        let state = install_pending_stores(state, None);
        assert!(state.update_store.is_some());
        assert!(state.model_list_store.is_some());
        assert!(state.model_catalog_cache.is_some());
        assert!(state.local_skill_list_store.is_some());
        assert!(state.local_skill_import_store.is_some());
        let created = state
            .update_store
            .as_ref()
            .unwrap()
            .create("runtime-1", "v2", "user-1")
            .await
            .unwrap();
        assert!(state
            .update_store
            .as_ref()
            .unwrap()
            .has_pending("runtime-1")
            .await
            .unwrap());
        assert_eq!(created.target_version, "v2");
    }

    #[tokio::test]
    async fn unreachable_redis_is_installed_lazily_for_later_recovery() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let state = patchbay_handler::HandlerState::new(
            pool,
            patchbay_auth::pat_cache::PatCache::disabled(),
            None,
        );
        let started = tokio::time::Instant::now();
        let state = install_pending_stores(state, Some("redis://192.0.2.1:6379/"));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(state.update_store.is_some());
        assert!(state.model_list_store.is_some());
        assert!(state.local_skill_list_store.is_some());
        assert!(state.local_skill_import_store.is_some());
    }

    #[test]
    fn shared_desktop_handoff_requires_a_nonempty_valid_redis_url() {
        assert!(validate_shared_desktop_handoff_redis(false, None).is_ok());
        assert!(validate_shared_desktop_handoff_redis(true, None).is_err());
        assert!(validate_shared_desktop_handoff_redis(true, Some("   ")).is_err());
        assert!(validate_shared_desktop_handoff_redis(true, Some("not a redis URL")).is_err());
        assert!(
            validate_shared_desktop_handoff_redis(true, Some("redis://redis.internal:6379/0"))
                .is_ok()
        );
    }

    #[tokio::test]
    async fn malformed_redis_url_disables_auth_limiting_without_blocking_router_build() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let mut cfg = patchbay_config::Config::default();
        cfg.redis.url = Some("not a redis URL".into());
        cfg.urls.rate_limit_trusted_proxies = Some("10.0.0.0/8".into());
        let ProductionApp {
            root_cancel,
            realtime,
            channel_runtime,
            ..
        } = build_production_router(
            pool,
            Arc::new(patchbay_realtime::hub::Hub::new()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            &cfg,
            test_attachment_storage(),
            Vec::new(),
            VcsWebhookConfig::disabled(),
        )
        .await
        .unwrap();
        channel_runtime.shutdown().await;
        root_cancel.cancel();
        realtime.shutdown().await;
    }
}
