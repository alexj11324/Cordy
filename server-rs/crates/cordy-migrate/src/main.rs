//! cordy-migrate — Rust replacement for `server/cmd/migrate`.
//!
//! Usage: `cordy-migrate up|down|status` (DATABASE_URL env required).

mod files;
mod hooks;
mod index_maps;
mod runner;

use std::time::Duration;

use anyhow::Context as _;
use clap::{Parser, ValueEnum};
use sqlx::postgres::PgPoolOptions;

const DEFAULT_LOCK_TIMEOUT_SECONDS: u64 = 300;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Command {
    Up,
    Down,
    Status,
}

impl Command {
    fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Status => "status",
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "cordy-migrate",
    about = "Apply, roll back, or check Cordy database migrations"
)]
struct Args {
    #[arg(value_enum, default_value = "up")]
    command: Command,

    /// Maximum time to wait for another migration runner to release its lock.
    #[arg(long)]
    lock_timeout_seconds: Option<u64>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cordy_util::install_rustls_crypto_provider()?;
    cordy_migrate::init_logging();

    let args = Args::parse();
    let lock_timeout = configured_lock_timeout(args.lock_timeout_seconds)?;

    let db_url =
        std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL is required"))?;
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let connect = PgPoolOptions::new().max_connections(2).connect(&db_url);
    tokio::pin!(connect);
    let pool = tokio::select! {
        result = &mut connect => result.context("connect to database")?,
        _ = &mut shutdown => anyhow::bail!("migration interrupted by signal while connecting"),
    };

    let operation = async {
        match args.command {
            Command::Status => {
                runner::check_ready(&pool, lock_timeout).await?;
                println!("ready: all migrations recorded");
                Ok(())
            }
            command => runner::run_migrations(&pool, command.as_str(), lock_timeout).await,
        }
    };
    tokio::pin!(operation);
    tokio::select! {
        result = &mut operation => result,
        _ = &mut shutdown => anyhow::bail!("migration interrupted by signal"),
    }
}

fn configured_lock_timeout(cli_seconds: Option<u64>) -> anyhow::Result<Duration> {
    let seconds = match cli_seconds {
        Some(seconds) => seconds,
        None => match std::env::var("CORDY_MIGRATION_LOCK_TIMEOUT_SECONDS") {
            Ok(raw) => raw.parse::<u64>().with_context(|| {
                format!("parse CORDY_MIGRATION_LOCK_TIMEOUT_SECONDS={raw:?} as seconds")
            })?,
            Err(std::env::VarError::NotPresent) => DEFAULT_LOCK_TIMEOUT_SECONDS,
            Err(std::env::VarError::NotUnicode(_)) => {
                anyhow::bail!("CORDY_MIGRATION_LOCK_TIMEOUT_SECONDS must be valid UTF-8")
            }
        },
    };
    if seconds == 0 {
        anyhow::bail!("migration lock timeout must be greater than zero seconds");
    }
    Ok(Duration::from_secs(seconds))
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                    return;
                }
            };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
