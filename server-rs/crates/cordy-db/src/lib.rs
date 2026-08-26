//! Database access layer: connection pool and migrations.
//!
//! The Rust server shares the exact same schema as the Go server
//! (`server/migrations/`, 413 up/down pairs). See migration plan §二
//! hard constraint #3.

use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

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

    let (max_connections, min_connections) = effective_pool_limits(cfg, url);
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .min_connections(min_connections)
        .connect_lazy(url)?;
    tracing::info!(max_connections, min_connections, "pg pool created");
    Ok(pool)
}

/// Applies the Go server's pool precedence:
/// environment overrides win over `pool_*` DATABASE_URL parameters, which
/// win over the Rust config defaults. Invalid positive overrides fall back to
/// the next lower-precedence value and are logged.
pub fn effective_pool_limits(cfg: &cordy_config::DatabaseConfig, url: &str) -> (u32, u32) {
    let max_env = std::env::var("DATABASE_MAX_CONNS").ok();
    let min_env = std::env::var("DATABASE_MIN_CONNS").ok();
    effective_pool_limits_with_env(cfg, url, max_env.as_deref(), min_env.as_deref())
}

fn effective_pool_limits_with_env(
    cfg: &cordy_config::DatabaseConfig,
    url: &str,
    max_env: Option<&str>,
    min_env: Option<&str>,
) -> (u32, u32) {
    let max_fallback = pool_url_param(url, "pool_max_conns").unwrap_or(cfg.max_connections);
    let min_fallback = pool_url_param(url, "pool_min_conns").unwrap_or(cfg.min_connections);
    let max_connections = positive_env_u32("DATABASE_MAX_CONNS", max_env, max_fallback.max(1));
    let min_connections =
        positive_env_u32("DATABASE_MIN_CONNS", min_env, min_fallback).min(max_connections);
    (max_connections, min_connections)
}

fn pool_url_param(url: &str, key: &str) -> Option<u32> {
    let parsed = url::Url::parse(url).ok()?;
    let raw = parsed
        .query_pairs()
        .find_map(|(name, value)| (name == key).then(|| value.into_owned()))?;
    match raw.parse::<u32>().ok().filter(|value| *value > 0) {
        Some(value) => Some(value),
        None => {
            tracing::warn!(key, value = %raw, "invalid database pool URL parameter; using fallback");
            None
        }
    }
}

fn positive_env_u32(name: &str, raw: Option<&str>, fallback: u32) -> u32 {
    let Some(raw) = raw.filter(|value| !value.is_empty()) else {
        return fallback;
    };
    match raw.parse::<u32>().ok().filter(|value| *value > 0) {
        Some(value) => value,
        None => {
            tracing::warn!(name, value = %raw, default = fallback, "invalid database pool env override; using fallback");
            fallback
        }
    }
}

/// Logs pool pressure every 15 seconds. SQLx does not expose pgxpool's
/// acquire/wait counters, so this records the portable connection gauges and
/// keeps the same periodic operational signal without inventing wait values.
pub async fn run_pool_stats_logger(pool: PgPool, cancellation: CancellationToken) {
    let period = Duration::from_secs(15);
    // Tokio's regular interval ticks immediately; Go's time.Ticker waits for
    // the first period. Start at the same cadence so startup does not produce
    // an extra sample that the Go operational contract never emitted.
    let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    loop {
        tokio::select! {
            _ = cancellation.cancelled() => return,
            _ = interval.tick() => {}
        }
        let total = pool.size() as usize;
        let idle = pool.num_idle();
        let acquired = total.saturating_sub(idle);
        if acquired >= total && total > 0 {
            tracing::warn!(
                max_connections = pool.options().get_max_connections(),
                total_conns = total,
                acquired_conns = acquired,
                idle_conns = idle,
                "db pool pressure"
            );
        } else {
            tracing::info!(
                max_connections = pool.options().get_max_connections(),
                total_conns = total,
                acquired_conns = acquired,
                idle_conns = idle,
                "db pool stats"
            );
        }
    }
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

    fn test_config() -> cordy_config::DatabaseConfig {
        cordy_config::DatabaseConfig {
            url: None,
            max_connections: 25,
            min_connections: 5,
        }
    }

    #[tokio::test]
    async fn connect_fails_fast_without_url() {
        let cfg = cordy_config::DatabaseConfig::default();
        assert!(connect(&cfg).await.is_err());
    }

    #[test]
    fn pool_limits_use_config_defaults_and_url_parameters() {
        let cfg = test_config();
        assert_eq!(
            effective_pool_limits_with_env(&cfg, "postgres://localhost/cordy", None, None),
            (25, 5)
        );
        assert_eq!(
            effective_pool_limits_with_env(
                &cfg,
                "postgres://localhost/cordy?pool_max_conns=40&pool_min_conns=7",
                None,
                None,
            ),
            (40, 7)
        );
    }

    #[test]
    fn pool_env_overrides_url_and_clamps_min_to_max() {
        let cfg = test_config();
        assert_eq!(
            effective_pool_limits_with_env(
                &cfg,
                "postgres://localhost/cordy?pool_max_conns=40&pool_min_conns=7",
                Some("12"),
                Some("30"),
            ),
            (12, 12)
        );
    }

    #[test]
    fn invalid_pool_overrides_fall_back_without_using_sqlx_defaults() {
        let cfg = cordy_config::DatabaseConfig {
            url: None,
            max_connections: 0,
            min_connections: 5,
        };
        assert_eq!(
            effective_pool_limits_with_env(
                &cfg,
                "postgres://localhost/cordy?pool_max_conns=bad&pool_min_conns=0",
                Some("not-a-number"),
                Some(" "),
            ),
            (1, 1)
        );
    }
}
