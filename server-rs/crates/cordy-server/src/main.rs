//! Cordy HTTP server — Rust replacement for `server/cmd/server`.
//!
//! This is the S1 vertical slice from the migration plan: config loading,
//! pg pool, and health endpoints. Routes are ported domain-by-domain in
//! later steps (475 routes total, see tasks/go-to-rust-migration.md).

use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const PENDING_STORE_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

struct VcsWebhookConfig {
    enabled: bool,
    secret_box: Option<cordy_util::secretbox::SecretBox>,
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

#[cfg(test)]
fn build_router(db: Option<sqlx::PgPool>, hub: Option<Arc<cordy_realtime::hub::Hub>>) -> Router {
    cordy_handler::build_router(db, hub)
}

async fn install_pending_stores(
    state: cordy_handler::HandlerState,
    redis_url: Option<&str>,
) -> cordy_handler::HandlerState {
    let Some(redis_url) = redis_url.map(str::trim).filter(|url| !url.is_empty()) else {
        return state;
    };
    let client = match redis::Client::open(redis_url) {
        Ok(client) => client,
        Err(_) => {
            tracing::warn!("REDIS_URL is invalid; runtime pending requests are disabled");
            return state;
        }
    };
    match tokio::time::timeout(
        PENDING_STORE_CONNECT_TIMEOUT,
        state.clone().with_redis(client),
    )
    .await
    {
        Ok(Ok(wired)) => wired,
        Ok(Err(_)) | Err(_) => {
            tracing::warn!("Redis is unavailable; runtime pending requests are disabled");
            state
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn build_production_router(
    db: sqlx::PgPool,
    hub: Arc<cordy_realtime::hub::Hub>,
    business_metrics: Option<Arc<cordy_metrics::BusinessMetrics>>,
    http_metrics: Option<Arc<cordy_metrics::HttpMetrics>>,
    github_client: Option<cordy_ghsnapshot::Client>,
    cfg: &cordy_config::Config,
    attachment_storage: Arc<dyn cordy_handler::attachment_storage::AttachmentStorage>,
    attachment_frame_ancestors: Vec<String>,
    vcs: VcsWebhookConfig,
) -> anyhow::Result<Router> {
    let feature_flags = Arc::new(cordy_service::feature_flags::ConfiguredFlags::from_env()?);
    let mut state = cordy_handler::HandlerState::new(
        db,
        cordy_auth::pat_cache::PatCache::disabled(),
        Some(hub),
    )
    .with_observability(business_metrics, http_metrics)
    .with_github_snapshots(github_client)
    .with_analytics(Arc::from(cordy_analytics::new_from_env()))
    .with_auth_settings(cordy_handler::auth::AuthSettings::from_config(cfg))
    .with_email_service(Arc::new(
        cordy_service::email::EmailService::from_config_values(
            cfg.email.resend_api_key.as_deref(),
            cfg.email.smtp_host.as_deref(),
        ),
    ))
    .with_rate_limit_trusted_proxies(cfg.urls.rate_limit_trusted_proxies.as_deref())
    .with_attachment_storage(attachment_storage, attachment_frame_ancestors)
    .with_plugins_from_env()
    .with_feature_flags(feature_flags)
    .with_public_config(cordy_handler::config::PublicConfigSettings {
        cdn_domain: cfg.storage.cloudfront_domain.clone().unwrap_or_default(),
        cdn_signed: cfg.storage.cloudfront_key_pair_id.is_some()
            && (cfg.storage.cloudfront_private_key.is_some()
                || cfg.storage.cloudfront_private_key_secret.is_some()),
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
    let state = install_pending_stores(state, redis_url).await;
    Ok(cordy_handler::build_router_from_state(state))
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
    let (business_metrics, http_metrics) = if metrics_config.enabled() {
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
        let gatherer = Arc::new(registry.gatherer.clone());
        let metrics_addr = metrics_config.addr.clone();
        let effective_metrics_addr = cordy_metrics::server::normalized_bind_addr(&metrics_addr);
        if !cordy_metrics::is_loopback_addr(&effective_metrics_addr) {
            tracing::warn!(addr = %metrics_addr, "metrics listener is not loopback-only; restrict access with private networking, allowlists, or proxy auth");
        }
        tokio::spawn(async move {
            if let Err(error) = cordy_metrics::server::serve(metrics_addr, gatherer).await {
                tracing::error!(%error, "metrics server stopped");
            }
        });
        (Some(business), Some(http))
    } else {
        (None, None)
    };
    let github_client = cordy_ghsnapshot::Client::new_from_env()?;
    let attachment_storage = cordy_handler::attachment_storage::from_env(
        cfg.storage.local_upload_dir.as_deref(),
        cfg.storage.local_upload_base_url.as_deref(),
    )?;
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
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

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
    async fn unavailable_rate_limit_redis_fails_open() {
        let db = sqlx::PgPool::connect_lazy("postgres://invalid/invalid")
            .unwrap_or_else(|_| unreachable!("static test URL is valid"));
        let mut cfg = cordy_config::Config::default();
        cfg.redis.url = Some("redis://127.0.0.1:1/".into());
        let router = build_production_router(
            db,
            Arc::new(cordy_realtime::hub::Hub::new()),
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
        let router = build_production_router(
            db,
            Arc::new(cordy_realtime::hub::Hub::new()),
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

    #[tokio::test]
    async fn invalid_redis_config_keeps_pending_stores_fail_closed() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let state = cordy_handler::HandlerState::new(
            pool,
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        );
        let state = install_pending_stores(state, Some("not-a-redis-url")).await;
        assert!(state.update_store.is_none());
        assert!(state.model_list_store.is_none());
        assert!(state.local_skill_list_store.is_none());
        assert!(state.local_skill_import_store.is_none());
    }

    #[tokio::test]
    async fn unreachable_redis_does_not_block_server_startup() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let state = cordy_handler::HandlerState::new(
            pool,
            cordy_auth::pat_cache::PatCache::disabled(),
            None,
        );
        let started = tokio::time::Instant::now();
        let state = install_pending_stores(state, Some("redis://192.0.2.1:6379/")).await;
        assert!(started.elapsed() <= PENDING_STORE_CONNECT_TIMEOUT + Duration::from_secs(1));
        assert!(state.update_store.is_none());
        assert!(state.model_list_store.is_none());
        assert!(state.local_skill_list_store.is_none());
        assert!(state.local_skill_import_store.is_none());
    }

    #[tokio::test]
    async fn malformed_redis_url_disables_auth_limiting_without_blocking_router_build() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let mut cfg = cordy_config::Config::default();
        cfg.redis.url = Some("not a redis URL".into());
        cfg.urls.rate_limit_trusted_proxies = Some("10.0.0.0/8".into());
        let _router = build_production_router(
            pool,
            Arc::new(cordy_realtime::hub::Hub::new()),
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
    }
}
