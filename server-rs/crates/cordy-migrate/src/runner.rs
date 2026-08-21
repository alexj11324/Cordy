//! Core migration loop — port of `runMigrations` in `server/cmd/migrate/main.go`.

use std::path::PathBuf;

use sqlx::{PgConnection, PgPool};

use crate::files;
use crate::hooks::{self, MIGRATION_ADVISORY_LOCK_KEY};

const DEFAULT_SCHEMA_MIGRATIONS_TABLE: &str = "schema_migrations";

/// Run all pending migrations in `direction` ("up" or "down").
///
/// The loop is deliberately NOT wrapped in a single transaction: the repo
/// ships migrations using CREATE INDEX CONCURRENTLY, which Postgres rejects
/// inside a transaction block. The session-pinned advisory lock serialises
/// concurrent runners; a late-arriving runner queues behind the current one
/// and turns finished migrations into no-op skips.
pub async fn run_migrations(pool: &PgPool, direction: &str) -> anyhow::Result<()> {
    if direction != "up" && direction != "down" {
        anyhow::bail!("invalid direction {direction:?} (want \"up\" or \"down\")");
    }

    let files_list: Vec<PathBuf> = files::files(direction)?;
    let hooks_map = hooks::hooks_for_direction(direction);
    let conditions_map = hooks::conditions_for_direction(direction);

    // pg_advisory_lock is per-session, so pin one connection for the whole run.
    let mut conn = pool.acquire().await?;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(MIGRATION_ADVISORY_LOCK_KEY)
        .execute(&mut *conn)
        .await?;

    let result = run_locked(
        &mut conn,
        pool,
        direction,
        &files_list,
        &hooks_map,
        &conditions_map,
    )
    .await;

    // Best-effort unlock on the success path; on error paths the connection
    // closing at process exit releases session-level locks anyway.
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_ADVISORY_LOCK_KEY)
        .execute(&mut *conn)
        .await;

    result
}

async fn run_locked(
    conn: &mut PgConnection,
    pool: &PgPool,
    direction: &str,
    files_list: &[PathBuf],
    hooks_map: &[(&'static str, hooks::PreMigrationHook)],
    conditions_map: &[(&'static str, hooks::MigrationCondition)],
) -> anyhow::Result<()> {
    sqlx::query(&format!(
        r#"
        CREATE TABLE IF NOT EXISTS {DEFAULT_SCHEMA_MIGRATIONS_TABLE} (
            version TEXT PRIMARY KEY,
            applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
        )
        "#
    ))
    .execute(&mut *conn)
    .await?;

    for file in files_list {
        let version = files::extract_version(file);

        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = $1)")
                .bind(&version)
                .fetch_one(&mut *conn)
                .await?;

        if direction == "up" && exists {
            println!("  skip  {version} (already applied)");
            continue;
        }
        if direction == "down" && !exists {
            println!("  skip  {version} (not applied)");
            continue;
        }

        let sql = std::fs::read_to_string(file)?;

        if let Some((_, hook)) = hooks_map.iter().find(|(v, _)| *v == version) {
            tracing::info!(%version, direction, "running pre-migration hook");
            hook(pool).await?;
        }

        let mut apply_sql = true;
        let mut condition_reason = String::new();
        if let Some((_, condition)) = conditions_map.iter().find(|(v, _)| *v == version) {
            (apply_sql, condition_reason) = condition(&mut *conn).await?;
        }

        if apply_sql {
            sqlx::raw_sql(&sql).execute(&mut *conn).await?;
        }

        if direction == "up" {
            sqlx::query("INSERT INTO schema_migrations (version) VALUES ($1)")
                .bind(&version)
                .execute(&mut *conn)
                .await?;
        } else {
            sqlx::query("DELETE FROM schema_migrations WHERE version = $1")
                .bind(&version)
                .execute(&mut *conn)
                .await?;
        }

        if apply_sql {
            println!("  {direction}  {version}");
        } else {
            println!("  {direction}  {version} (SQL skipped: {condition_reason})");
        }
    }
    Ok(())
}

/// Readiness check: every up migration must be recorded. Checking only the
/// lexically-last version would miss an out-of-order migration.
pub async fn check_ready(pool: &PgPool) -> anyhow::Result<()> {
    let versions = files::all_versions()?;
    for v in versions {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = $1)")
                .bind(&v)
                .fetch_one(pool)
                .await?;
        if !exists {
            anyhow::bail!("migration {v} not recorded; run `cordy-migrate up` first");
        }
    }
    Ok(())
}
