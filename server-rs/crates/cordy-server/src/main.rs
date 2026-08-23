//! Cordy HTTP server — Rust replacement for `server/cmd/server`.
//!
//! This is the S1 vertical slice from the migration plan: config loading,
//! pg pool, and health endpoints. Routes are ported domain-by-domain in
//! later steps (475 routes total, see tasks/go-to-rust-migration.md).

#[cfg(test)]
use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;

#[cfg(test)]
fn build_router(db: Option<sqlx::PgPool>, hub: Option<Arc<cordy_realtime::hub::Hub>>) -> Router {
    cordy_handler::build_router(db, hub)
}

fn build_production_router(
    db: sqlx::PgPool,
    hub: Arc<cordy_realtime::hub::Hub>,
    business_metrics: Option<Arc<cordy_metrics::BusinessMetrics>>,
    http_metrics: Option<Arc<cordy_metrics::HttpMetrics>>,
    storage: Arc<dyn cordy_handler::attachment_storage::AttachmentStorage>,
    download: cordy_handler::state::AttachmentDownloadSettings,
    realtime_metrics_token: Option<&str>,
) -> Router {
    let state = cordy_handler::HandlerState::new(
        db,
        cordy_auth::pat_cache::PatCache::disabled(),
        Some(hub),
    )
    .with_attachment_storage(storage, download)
    .with_realtime_metrics_token(realtime_metrics_token)
    .with_observability(business_metrics, http_metrics);
    cordy_handler::build_router_from_state(state)
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
    tracing::info!(port = cfg.server.port, "starting cordy-server");

    let db = cordy_db::connect(&cfg.database).await?;
    let storage = cordy_handler::attachment_storage::from_env(
        cfg.storage.local_upload_dir.as_deref(),
        cfg.storage.local_upload_base_url.as_deref(),
        cfg.storage.cloudfront_domain.as_deref(),
    )
    .await?;
    let download = cordy_handler::state::AttachmentDownloadSettings::from_config(&cfg).await?;
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
    let app = build_production_router(
        db,
        hub,
        business_metrics,
        http_metrics,
        storage,
        download,
        cfg.redis.realtime_metrics_token.as_deref(),
    );

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
}
