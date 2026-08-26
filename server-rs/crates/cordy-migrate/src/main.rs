//! cordy-migrate — Rust replacement for `server/cmd/migrate`.
//!
//! Usage: `cordy-migrate up|down|status` (DATABASE_URL env required).
//! The operator backfill is available as
//! `cordy-migrate backfill task-usage-hourly [flags]`.

mod backfill;
mod files;
mod hooks;
mod index_maps;
mod runner;

use std::{env, time::Duration};

use sqlx::postgres::PgPoolOptions;
use tokio_util::sync::CancellationToken;

use crate::backfill::task_usage::{self, OperatorOptions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cordy_migrate=info".into()),
        )
        .init();

    let args: Vec<String> = env::args().skip(1).collect();
    let command = parse_command(&args)?;

    if let Command::BackfillTaskUsageHourly(options) = command {
        return run_operator_command(options).await;
    }

    let db_url =
        env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL is required"))?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await?;

    match command {
        Command::Status => {
            runner::check_ready(&pool).await?;
            println!("ready: all migrations recorded");
            Ok(())
        }
        Command::Up => runner::run_migrations(&pool, "up").await,
        Command::Down => runner::run_migrations(&pool, "down").await,
        Command::BackfillTaskUsageHourly(_) => unreachable!("handled above"),
    }
}

enum Command {
    Up,
    Down,
    Status,
    BackfillTaskUsageHourly(OperatorOptions),
}

fn parse_command(args: &[String]) -> anyhow::Result<Command> {
    match args {
        [] => Ok(Command::Up),
        [command] if command == "up" => Ok(Command::Up),
        [command] if command == "down" => Ok(Command::Down),
        [command] if command == "status" => Ok(Command::Status),
        [command, backfill, rest @ ..]
            if command == "backfill" && backfill == "task-usage-hourly" =>
        {
            Ok(Command::BackfillTaskUsageHourly(parse_operator_options(rest)?))
        }
        _ => anyhow::bail!(
            "usage: cordy-migrate up|down|status | cordy-migrate backfill task-usage-hourly [--dry-run] [--months-back N] [--force-partial] [--sleep-between-slices DURATION]"
        ),
    }
}

fn parse_operator_options(args: &[String]) -> anyhow::Result<OperatorOptions> {
    let mut options = OperatorOptions::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let option = arg
            .strip_prefix("--")
            .or_else(|| arg.strip_prefix('-'))
            .unwrap_or(arg.as_str());
        if option == "dry-run" {
            options.dry_run = true;
        } else if option == "force-partial" {
            options.force_partial = true;
        } else if let Some(value) = option.strip_prefix("months-back=") {
            options.months_back = parse_i64(value, "--months-back")?;
        } else if option == "months-back" {
            index += 1;
            options.months_back = parse_i64(
                args.get(index)
                    .ok_or_else(|| anyhow::anyhow!("--months-back requires a value"))?,
                "--months-back",
            )?;
        } else if let Some(value) = option.strip_prefix("sleep-between-slices=") {
            options.sleep_between_slices = parse_duration(value)?;
        } else if option == "sleep-between-slices" {
            index += 1;
            options.sleep_between_slices = parse_duration(
                args.get(index)
                    .ok_or_else(|| anyhow::anyhow!("--sleep-between-slices requires a value"))?,
            )?;
        } else {
            anyhow::bail!("unknown backfill option {arg:?}");
        }
        index += 1;
    }
    Ok(options)
}

fn parse_i64(value: &str, flag: &str) -> anyhow::Result<i64> {
    value
        .parse::<i64>()
        .map_err(|error| anyhow::anyhow!("invalid {flag} value {value:?}: {error}"))
}

/// Parses the duration forms used by Go's `time.ParseDuration` for the
/// operator's throttle flag. Negative values are validated and represented as
/// zero because the Go command treats them as "no sleep" when it checks `> 0`.
fn parse_duration(value: &str) -> anyhow::Result<Duration> {
    let (negative, value) = if let Some(value) = value.strip_prefix('-') {
        (true, value)
    } else if let Some(value) = value.strip_prefix('+') {
        (false, value)
    } else {
        (false, value)
    };
    if value.is_empty() {
        anyhow::bail!("invalid duration {value:?}");
    }
    if value == "0" {
        return Ok(Duration::ZERO);
    }

    let mut offset = 0;
    let mut total_nanos = 0_u128;
    while offset < value.len() {
        let integer_start = offset;
        let mut integer = 0_u128;
        while offset < value.len() && value.as_bytes()[offset].is_ascii_digit() {
            let digit = u128::from(value.as_bytes()[offset] - b'0');
            integer = integer
                .checked_mul(10)
                .and_then(|value| value.checked_add(digit))
                .ok_or_else(|| anyhow::anyhow!("invalid duration {value:?}"))?;
            offset += 1;
        }
        let integer_end = offset;
        let mut fraction_start = offset;
        let mut fraction_end = offset;
        if value.as_bytes().get(offset) == Some(&b'.') {
            offset += 1;
            fraction_start = offset;
            while offset < value.len() && value.as_bytes()[offset].is_ascii_digit() {
                offset += 1;
            }
            fraction_end = offset;
        }
        if integer_start == integer_end && fraction_start == fraction_end {
            anyhow::bail!("invalid duration {value:?}");
        }

        let rest = &value[offset..];
        let (unit_len, unit_nanos) = if rest.starts_with("ns") {
            (2, 1_u128)
        } else if rest.starts_with("us") || rest.starts_with("µs") {
            (
                if rest.starts_with("us") {
                    2
                } else {
                    "µs".len()
                },
                1_000,
            )
        } else if rest.starts_with("ms") {
            (2, 1_000_000)
        } else if rest.starts_with('s') {
            (1, 1_000_000_000)
        } else if rest.starts_with('m') {
            (1, 60_000_000_000)
        } else if rest.starts_with('h') {
            (1, 3_600_000_000_000)
        } else {
            anyhow::bail!("invalid duration {value:?}");
        };
        offset += unit_len;

        let whole_nanos = integer
            .checked_mul(unit_nanos)
            .ok_or_else(|| anyhow::anyhow!("invalid duration {value:?}"))?;
        let fraction_nanos = fractional_nanos(&value[fraction_start..fraction_end], unit_nanos)?;
        total_nanos = total_nanos
            .checked_add(whole_nanos)
            .and_then(|total| total.checked_add(fraction_nanos))
            .filter(|total| *total <= i64::MAX as u128)
            .ok_or_else(|| anyhow::anyhow!("invalid duration {value:?}"))?;
    }

    if negative {
        Ok(Duration::ZERO)
    } else {
        Ok(Duration::from_nanos(total_nanos as u64))
    }
}

fn fractional_nanos(fraction: &str, unit_nanos: u128) -> anyhow::Result<u128> {
    if fraction.is_empty() {
        return Ok(0);
    }
    let precision = fraction.len().min(9);
    let mut digits = 0_u128;
    for byte in fraction.as_bytes().iter().take(precision) {
        digits = digits
            .checked_mul(10)
            .and_then(|value| value.checked_add(u128::from(*byte - b'0')))
            .ok_or_else(|| anyhow::anyhow!("invalid fractional duration"))?;
    }
    for _ in precision..9 {
        digits *= 10;
    }
    digits
        .checked_mul(unit_nanos)
        .map(|value| value / 1_000_000_000)
        .ok_or_else(|| anyhow::anyhow!("invalid fractional duration"))
}

async fn run_operator_command(options: OperatorOptions) -> anyhow::Result<()> {
    let cancellation = CancellationToken::new();
    let signal_task = spawn_signal_handler(cancellation.clone());
    let result = async {
        let db_url = env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://cordy:cordy@localhost:5432/cordy?sslmode=disable".to_string()
        });
        let pool = tokio::select! {
            _ = cancellation.cancelled() => anyhow::bail!("execution cancelled"),
            result = PgPoolOptions::new().max_connections(2).connect(&db_url) => result?,
        };
        tokio::select! {
            _ = cancellation.cancelled() => anyhow::bail!("execution cancelled"),
            result = sqlx::query("SELECT 1").execute(&pool) => { result?; },
        }
        let result = task_usage::run_operator(&pool, options, &cancellation).await;
        pool.close().await;
        result
    }
    .await;
    signal_task.abort();
    result
}

fn spawn_signal_handler(cancellation: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            let Ok(mut terminate) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            else {
                let _ = tokio::signal::ctrl_c().await;
                cancellation.cancel();
                return;
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
        cancellation.cancel();
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_flags_accept_go_short_and_long_forms() {
        let args = [
            "backfill".to_string(),
            "task-usage-hourly".to_string(),
            "-dry-run".to_string(),
            "--months-back=3".to_string(),
            "-force-partial".to_string(),
            "--sleep-between-slices".to_string(),
            "1m250ms".to_string(),
        ];
        let Command::BackfillTaskUsageHourly(options) =
            parse_command(&args).unwrap_or_else(|error| panic!("parse flags: {error}"))
        else {
            panic!("expected task usage backfill command");
        };
        assert!(options.dry_run);
        assert_eq!(options.months_back, 3);
        assert!(options.force_partial);
        assert_eq!(options.sleep_between_slices, Duration::from_millis(60_250));
    }

    #[test]
    fn duration_parser_matches_go_units_signs_and_fraction_truncation() {
        assert_eq!(
            parse_duration("+1.0000000009s").unwrap_or_default(),
            Duration::from_secs(1)
        );
        assert_eq!(
            parse_duration("1µs").unwrap_or_default(),
            Duration::from_micros(1)
        );
        assert_eq!(
            parse_duration(".5s").unwrap_or_default(),
            Duration::from_millis(500)
        );
        assert_eq!(parse_duration("-2s").unwrap_or_default(), Duration::ZERO);
    }

    #[test]
    fn duration_parser_rejects_invalid_and_overflowing_values() {
        assert!(parse_duration("-not-a-duration").is_err());
        assert!(parse_duration("1").is_err());
        assert!(parse_duration("9223372036.854775808s").is_err());
    }
}
