//! cordy-migrate — Rust replacement for `server/cmd/migrate`.
//!
//! Usage: `cordy-migrate up|down|status` (DATABASE_URL env required).
//! The issue activity backfill is available as
//! `cordy-migrate backfill-issue-last-activity [options]`.

mod backfill;
mod files;
mod hooks;
mod index_maps;
mod runner;

use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cordy_migrate=info".into()),
        )
        .init();

    let mut args = std::env::args();
    let command = args.next().unwrap_or_else(|| "cordy-migrate".to_string());
    let subcommand = args.next().unwrap_or_else(|| "up".to_string());

    if subcommand == "backfill-issue-last-activity" {
        return run_issue_activity_backfill(args).await;
    }
    if !matches!(subcommand.as_str(), "up" | "down" | "status") {
        anyhow::bail!(
            "usage: {command} up|down|status\n       {command} backfill-issue-last-activity [options]"
        );
    }

    let db_url =
        std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL is required"))?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await?;

    match subcommand.as_str() {
        "status" => {
            runner::check_ready(&pool).await?;
            println!("ready: all migrations recorded");
            Ok(())
        }
        dir => runner::run_migrations(&pool, dir).await,
    }
}

async fn run_issue_activity_backfill(args: impl Iterator<Item = String>) -> anyhow::Result<()> {
    let Some(options) = backfill::issue_activity::Options::parse(args)? else {
        eprintln!("{}", backfill::issue_activity::USAGE);
        return Ok(());
    };
    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgres://cordy:cordy@localhost:5432/cordy?sslmode=disable".to_string()
    });
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await?;
    let run = backfill::issue_activity::run(&pool, options);
    tokio::select! {
        result = run => result.map(|summary| {
            tracing::info!(
                rows_backfilled = summary.rows_backfilled,
                remaining = summary.remaining,
                batches = summary.batches,
                passes = summary.passes,
                "issue last-activity backfill complete"
            );
        }),
        result = tokio::signal::ctrl_c() => {
            result?;
            tracing::warn!("issue last-activity backfill interrupted");
            Ok(())
        }
    }
}
