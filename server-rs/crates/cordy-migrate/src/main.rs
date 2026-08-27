//! cordy-migrate — Rust replacement for `server/cmd/migrate`.
//!
//! Usage: `cordy-migrate up|down` (DATABASE_URL env required).

mod files;
mod hooks;
mod index_maps;
mod runner;

use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cordy_migrate::init_logging();

    let command = std::env::args().nth(1).unwrap_or_else(|| "up".to_string());
    if !matches!(command.as_str(), "up" | "down" | "status") {
        anyhow::bail!("usage: cordy-migrate up|down|status");
    }

    let db_url =
        std::env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL is required"))?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await?;

    match command.as_str() {
        "status" => {
            runner::check_ready(&pool).await?;
            println!("ready: all migrations recorded");
            Ok(())
        }
        dir => runner::run_migrations(&pool, dir).await,
    }
}
