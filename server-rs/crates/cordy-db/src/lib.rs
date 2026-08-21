//! Database access layer: connection pool and migrations.
//!
//! The Rust server shares the exact same schema as the Go server
//! (`server/migrations/`, 413 up/down pairs). See migration plan §二
//! hard constraint #3.

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub mod dbid;
pub mod models;
pub mod queries;
pub mod user;

/// Build a connection pool from config.
///
/// Lazy on purpose: the process must boot even while Postgres is down so
/// `/healthz` stays truthful and `/readyz` can report the outage. Config
/// errors still fail fast; connectivity is `/readyz`'s job.
pub async fn connect(cfg: &cordy_config::DatabaseConfig) -> anyhow::Result<PgPool> {
    let url = cfg
        .url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("database.url is not set"))?;

    let pool = PgPoolOptions::new()
        .max_connections(cfg.max_connections)
        .connect_lazy(url)?;
    tracing::info!(max_connections = cfg.max_connections, "pg pool created");
    Ok(pool)
}

/// Cheap liveness probe used by `/readyz`.
///
/// Runtime-checked query on purpose: compile-time `query!` macros require a
/// live DB or `.sqlx` offline cache, which lands in S2 (migration plan §四).
pub async fn ping(pool: &PgPool) -> anyhow::Result<()> {
    let _: i32 = sqlx::query_scalar("select 1").fetch_one(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_fails_fast_without_url() {
        let cfg = cordy_config::DatabaseConfig::default();
        assert!(connect(&cfg).await.is_err());
    }
}
