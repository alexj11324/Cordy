//! Cordy HTTP server — Rust replacement for `server/cmd/server`.
//!
//! This is the S1 vertical slice from the migration plan: config loading,
//! pg pool, and health endpoints. Routes are ported domain-by-domain in
//! later steps (475 routes total, see tasks/go-to-rust-migration.md).

use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

mod channel_runtime;
mod realtime_runtime;

struct VcsWebhookConfig {
    enabled: bool,
    secret_box: Option<cordy_util::secretbox::SecretBox>,
}

struct ProductionApp {
    router: Router,
    runtime_sweeper: cordy_handler::runtime_sweeper::RuntimeSweeperHandle,
    realtime: realtime_runtime::RealtimeRuntime,
    channel_runtime: Option<channel_runtime::ChannelRuntime>,
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

impl VcsWebhookConfig {
    #[cfg(test)]
    fn disabled() -> Self {
        Self {
            enabled: false,
            secret_box: None,
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

fn autopilot_entitlements(
    cfg: &cordy_config::Config,
    metrics: Option<Arc<cordy_metrics::BusinessMetrics>>,
) -> Option<Arc<dyn cordy_service::autopilot::EntitlementProvider>> {
    let enabled = parse_go_bool(
        std::env::var("CORDY_ENTITLEMENT_POLICY_ENABLED")
            .ok()
            .as_deref(),
        false,
    );
    let emergency_disabled = parse_go_bool(
        std::env::var("CORDY_ENTITLEMENT_EMERGENCY_DISABLED")
            .ok()
            .as_deref(),
        false,
    );
    let service_token = std::env::var("CORDY_ENTITLEMENT_SERVICE_TOKEN")
        .ok()
        .or_else(|| cfg.entitlement.service_token.clone())
        .unwrap_or_default();
    let config = cordy_service::entitlement::EntitlementClientConfig {
        enabled,
        base_url: cfg.entitlement.policy_url.clone().unwrap_or_default(),
        service_token,
        timeout: duration_env(
            "CORDY_ENTITLEMENT_POLICY_TIMEOUT",
            Duration::from_secs(3),
            false,
        ),
        stale_grace: duration_env(
            "CORDY_ENTITLEMENT_STALE_GRACE",
            Duration::from_secs(15 * 60),
            true,
        ),
        emergency_disabled,
    };
    match cordy_service::entitlement::HttpEntitlementProvider::new_with_metrics(
        config,
        metrics.clone(),
    ) {
        Ok(provider) => provider.map(|provider| provider as Arc<_>),
        Err(error) => {
            if let Some(metrics) = metrics.as_deref() {
                metrics.record_entitlement_config_error();
            }
            tracing::error!(%error, "entitlement policy client disabled by invalid configuration");
            None
        }
    }
}

#[cfg(test)]
fn build_router(db: Option<sqlx::PgPool>, hub: Option<Arc<cordy_realtime::hub::Hub>>) -> Router {
    cordy_handler::build_router(db, hub)
}

fn install_pending_stores(
    state: cordy_handler::HandlerState,
    redis_url: Option<&str>,
) -> cordy_handler::HandlerState {
    let Some(redis_url) = redis_url.map(str::trim).filter(|url| !url.is_empty()) else {
        return state;
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

#[allow(clippy::too_many_arguments)]
async fn build_production_router(
    db: sqlx::PgPool,
    hub: Arc<cordy_realtime::hub::Hub>,
    business_metrics: Option<Arc<cordy_metrics::BusinessMetrics>>,
    http_metrics: Option<Arc<cordy_metrics::HttpMetrics>>,
    channel_lease_metrics: Option<Arc<cordy_metrics::ChannelLeaseMetrics>>,
    channel_media_metrics: Option<Arc<cordy_metrics::ChannelMediaReconcilerMetrics>>,
    wecom_metrics: Option<Arc<cordy_metrics::WecomMetrics>>,
    lark_backfill_metrics: Option<Arc<cordy_metrics::LarkBackfillMetrics>>,
    github_client: Option<cordy_ghsnapshot::Client>,
    cfg: &cordy_config::Config,
    attachment_storage: Arc<dyn cordy_handler::attachment_storage::AttachmentStorage>,
    attachment_frame_ancestors: Vec<String>,
    vcs: VcsWebhookConfig,
) -> anyhow::Result<ProductionApp> {
    let feature_flags = Arc::new(cordy_service::feature_flags::ConfiguredFlags::from_env()?);
    let entitlements = autopilot_entitlements(cfg, business_metrics.clone());
    let attachment_download =
        cordy_handler::state::AttachmentDownloadSettings::from_config(cfg).await?;
    attachment_download.validate_for_storage(attachment_storage.as_ref())?;
    let cdn_signed = attachment_download.cloudfront_signer.is_some();
    let mut state = cordy_handler::HandlerState::new(
        db,
        cordy_auth::pat_cache::PatCache::disabled(),
        Some(hub.clone()),
    )
    .with_observability(business_metrics, http_metrics)
    .with_autopilot_entitlements(entitlements)
    .with_github_snapshots(github_client)
    .with_analytics(Arc::from(cordy_analytics::new_from_env()))
    .with_auth_settings(cordy_handler::auth::AuthSettings::from_config(cfg))
    .with_cloud_pat_fleet_url(cfg.fleet.cloud_fleet_url.as_deref())
    .with_email_service(Arc::new(
        cordy_service::email::EmailService::from_config_values(
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
    .with_llm_from_env()?
    .with_feature_flags(feature_flags)
    .with_integrations(cfg.integrations.clone())
    .with_public_config(cordy_handler::config::PublicConfigSettings {
        cdn_domain: cfg.storage.cloudfront_domain.clone().unwrap_or_default(),
        cdn_signed,
        server_version: env!("CARGO_PKG_VERSION").to_string(),
    })
    .with_vcs_webhooks(vcs.enabled, vcs.secret_box);
    let redis_url = cfg
        .redis
        .url
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    if let Some(redis_url) = redis_url {
        match redis::Client::open(redis_url) {
            Ok(client) => {
                state = state
                    .with_rate_limit_redis(client.clone())
                    .with_auth_redis(client);
            }
            Err(error) => {
                tracing::warn!(%error, "invalid REDIS_URL; public-route rate limiting disabled");
            }
        }
    } else {
        tracing::warn!("public-route rate limiting disabled: REDIS_URL not configured");
    }
    let state = install_pending_stores(state, redis_url);
    let mut realtime = realtime_runtime::RealtimeRuntime::from_config(hub, &cfg.redis).await;
    realtime.attach(
        &state.bus,
        state.daemon_hub.clone(),
        state.daemon_notifier.clone(),
        state.background_runtime.clone(),
    );
    let state = state
        .start_autopilot_quota_reconciler()
        .start_webhook_delivery_worker();
    let channel_runtime = channel_runtime::ChannelRuntime::start(
        &state,
        cfg,
        channel_lease_metrics,
        channel_media_metrics,
        wecom_metrics,
        lark_backfill_metrics,
    )
    .await?;
    let configured_reconnect_grace = duration_env(
        "CORDY_RUNTIME_RECONNECT_GRACE",
        cordy_handler::runtime_sweeper::DEFAULT_RECONNECT_GRACE,
        false,
    );
    let minimum_reconnect_grace = cordy_handler::runtime_sweeper::MINIMUM_RECONNECT_GRACE;
    if configured_reconnect_grace < minimum_reconnect_grace {
        tracing::warn!(
            configured = ?configured_reconnect_grace,
            minimum = ?minimum_reconnect_grace,
            "runtime reconnect grace is shorter than heartbeat freshness; clamping"
        );
    }
    let runtime_sweeper = cordy_handler::runtime_sweeper::RuntimeSweeper::from_state(
        state.clone(),
        configured_reconnect_grace.max(minimum_reconnect_grace),
    )
    .start();
    Ok(ProductionApp {
        router: cordy_handler::build_router_from_state(state),
        runtime_sweeper,
        realtime,
        channel_runtime,
    })
}

fn validate_auth_config(cfg: &cordy_config::Config) -> anyhow::Result<()> {
    if cfg.is_production() {
        cordy_auth::jwt::validate_jwt_secret(cfg.auth.jwt_secret.as_deref().unwrap_or(""))
            .map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cordy=info,tower=info".into()),
        )
        .init();

    let cfg = cordy_config::Config::load(Some(std::path::Path::new("cordy.toml")))?;
    cfg.validate()?;
    validate_auth_config(&cfg)?;
    cordy_auth::jwt::configure_jwt_secret(cfg.auth.jwt_secret.as_deref())?;
    cordy_auth::cookie::configure_auth_token_ttl(cfg.auth.auth_token_ttl.as_deref())?;
    tracing::info!(port = cfg.server.port, "starting cordy-server");

    let db = cordy_db::connect(&cfg.database).await?;
    let hub = Arc::new(cordy_realtime::hub::Hub::new());
    let metrics_config = cordy_metrics::Config::from_env();
    let (
        business_metrics,
        http_metrics,
        channel_lease_metrics,
        channel_media_metrics,
        wecom_metrics,
        lark_backfill_metrics,
        metrics_runtime,
    ) = if metrics_config.enabled() {
        let registry = cordy_metrics::Registry::new(cordy_metrics::registry::RegistryOptions {
            pool: Some(Arc::new(db.clone())),
            realtime: Some(&cordy_realtime::M),
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: option_env!("CORDY_GIT_COMMIT")
                .unwrap_or("unknown")
                .to_string(),
            sampler: None,
        });
        let business = registry.business.clone();
        let http = registry.http.clone();
        let channel_lease = registry.channel_lease.clone();
        let channel_media = registry.channel_media.clone();
        let wecom = registry.wecom.clone();
        let lark_backfill = registry.lark_backfill.clone();
        let gatherer = Arc::new(registry.gatherer.clone());
        let metrics_addr = metrics_config.addr.clone();
        let effective_metrics_addr = cordy_metrics::server::normalized_bind_addr(&metrics_addr);
        if !cordy_metrics::is_loopback_addr(&effective_metrics_addr) {
            tracing::warn!(addr = %metrics_addr, "metrics listener is not loopback-only; restrict access with private networking, allowlists, or proxy auth");
        }
        let shutdown = tokio_util::sync::CancellationToken::new();
        let serve_shutdown = shutdown.clone();
        let task = tokio::spawn(async move {
            if let Err(error) =
                cordy_metrics::server::serve(metrics_addr, gatherer, serve_shutdown).await
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
    let github_client = cordy_ghsnapshot::Client::new_from_env()?;
    let attachment_storage = cordy_handler::attachment_storage::from_env(
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
    let vcs_enabled = cfg.integrations.vcs_integration_enabled.as_deref() == Some("true");
    let vcs_secret_box = cordy_util::secretbox::load_key("CORDY_VCS_SECRET_KEY")
        .ok()
        .and_then(|key| cordy_util::secretbox::SecretBox::new(&key).ok());
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
        VcsWebhookConfig {
            enabled: vcs_enabled,
            secret_box: vcs_secret_box,
        },
    )
    .await?;

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.server.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");
    let ProductionApp {
        router,
        runtime_sweeper,
        realtime,
        channel_runtime,
    } = app;
    let sweeper_cancel = runtime_sweeper.cancellation();
    let serve_result = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        sweeper_cancel.cancel();
    })
    .await;
    if let Some(channel_runtime) = channel_runtime {
        channel_runtime.shutdown().await;
    }
    runtime_sweeper.shutdown(Duration::from_secs(5)).await;
    realtime.shutdown().await;
    if let Some(metrics_runtime) = metrics_runtime {
        metrics_runtime.shutdown().await;
    }
    serve_result?;
    Ok(())
}

async fn shutdown_signal() {
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
    tracing::info!("shutdown signal received; draining HTTP server");
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

    fn test_attachment_storage() -> Arc<dyn cordy_handler::attachment_storage::AttachmentStorage> {
        Arc::new(
            cordy_handler::attachment_storage::LocalStorage::new(
                std::env::temp_dir().join("cordy-server-route-tests"),
                String::new(),
            )
            .expect("test local storage"),
        )
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
        let mut cfg = cordy_config::Config::default();
        cfg.redis.url = Some("redis://127.0.0.1:1/".into());
        let app = build_production_router(
            db,
            Arc::new(cordy_realtime::hub::Hub::new()),
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
        let ProductionApp {
            router,
            runtime_sweeper,
            realtime,
            channel_runtime,
        } = app;
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
        if let Some(channel_runtime) = channel_runtime {
            channel_runtime.shutdown().await;
        }
        realtime.shutdown().await;
        runtime_sweeper.shutdown(Duration::from_secs(1)).await;

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
        let mut cfg = cordy_config::Config::default();
        cfg.redis.url = Some(redis_url);
        let app = build_production_router(
            db,
            Arc::new(cordy_realtime::hub::Hub::new()),
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
        let ProductionApp {
            router,
            runtime_sweeper,
            realtime,
            channel_runtime,
        } = app;
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
        if let Some(channel_runtime) = channel_runtime {
            channel_runtime.shutdown().await;
        }
        realtime.shutdown().await;
        runtime_sweeper.shutdown(Duration::from_secs(1)).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn production_rejects_missing_or_insecure_jwt_secrets() {
        let mut cfg = cordy_config::Config::default();
        cfg.server.app_env = Some("production".into());
        assert!(validate_auth_config(&cfg).is_err());

        cfg.auth.jwt_secret = Some("cordy-dev-secret-change-in-production".into());
        assert!(validate_auth_config(&cfg).is_err());

        cfg.auth.jwt_secret = Some("a-long-random-production-secret".into());
        assert!(validate_auth_config(&cfg).is_ok());
    }

    #[test]
    fn invalid_redis_config_keeps_pending_stores_fail_closed() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let state = cordy_handler::HandlerState::new(
            pool,
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        );
        let state = install_pending_stores(state, Some("not-a-redis-url"));
        assert!(state.update_store.is_none());
        assert!(state.model_list_store.is_none());
        assert!(state.local_skill_list_store.is_none());
        assert!(state.local_skill_import_store.is_none());
    }

    #[test]
    fn unreachable_redis_is_installed_lazily_for_later_recovery() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let state = cordy_handler::HandlerState::new(
            pool,
            cordy_auth::pat_cache::PatCache::disabled(),
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

    #[tokio::test]
    async fn malformed_redis_url_disables_auth_limiting_without_blocking_router_build() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let mut cfg = cordy_config::Config::default();
        cfg.redis.url = Some("not a redis URL".into());
        cfg.urls.rate_limit_trusted_proxies = Some("10.0.0.0/8".into());
        let app = build_production_router(
            pool,
            Arc::new(cordy_realtime::hub::Hub::new()),
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
        let ProductionApp {
            router: _,
            runtime_sweeper,
            realtime,
            channel_runtime,
        } = app;
        if let Some(channel_runtime) = channel_runtime {
            channel_runtime.shutdown().await;
        }
        realtime.shutdown().await;
        runtime_sweeper.shutdown(Duration::from_secs(1)).await;
    }
}
