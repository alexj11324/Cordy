//! Cordy HTTP server — Rust replacement for `server/cmd/server`.
//!
//! This is the S1 vertical slice from the migration plan: config loading,
//! pg pool, and health endpoints. Routes are ported domain-by-domain in
//! later steps (475 routes total, see tasks/go-to-rust-migration.md).

use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;

fn build_router(db: Option<sqlx::PgPool>, hub: Option<Arc<cordy_realtime::hub::Hub>>) -> Router {
    cordy_handler::build_router(db, hub)
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
    let app = build_router(Some(db), Some(Arc::new(cordy_realtime::hub::Hub::new())));

    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.server.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, app).await?;
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
