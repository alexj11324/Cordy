//! Reconstructs `issue.last_activity_at` from the existing `updated_at`
//! history in bounded, resumable batches.
//!
//! Run this after the last-activity column migration and after every
//! issue-writing backend has been upgraded to maintain the new column.

use anyhow::Context as _;
use clap::Parser;
use cordy_migrate::backfill::issue_activity::{self, Options};
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

const DEFAULT_DATABASE_URL: &str = "postgres://cordy:cordy@localhost:5432/cordy?sslmode=disable";

#[derive(Debug, Parser)]
#[command(
    name = "backfill_issue_last_activity",
    about = "Reconstruct issue.last_activity_at from updated_at"
)]
struct Args {
    /// Maximum issue rows updated per transaction.
    #[arg(long, default_value_t = issue_activity::DEFAULT_BATCH_SIZE)]
    batch_size: i64,

    /// Delay between committed batches.
    #[arg(long, default_value = "100ms", value_parser = parse_go_duration)]
    sleep_between_batches: Duration,

    /// Stop after N batches; zero means finish all remaining rows.
    #[arg(long, default_value_t = 0)]
    max_batches: i64,

    /// Fail after N consecutive no-progress passes; zero disables the guard.
    #[arg(long, default_value_t = 10)]
    max_stalled_passes: i64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cordy_migrate::init_logging();

    let args = Args::parse();
    let options = Options {
        batch_size: args.batch_size,
        sleep_between_batches: args.sleep_between_batches,
        max_batches: args.max_batches,
        max_stalled_passes: args.max_stalled_passes,
    };
    issue_activity::validate_options(options)?;

    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_string());
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .context("connect to database")?;
    sqlx::query("SELECT 1")
        .execute(&pool)
        .await
        .context("ping database")?;

    tokio::select! {
        result = issue_activity::run(&pool, options) => result,
        _ = shutdown_signal() => Err(anyhow::anyhow!("backfill interrupted by signal")),
    }
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

// ponytail: keep the Go duration syntax at this CLI boundary without adding a
// dependency solely for one operator flag.
fn parse_go_duration(value: &str) -> Result<Duration, String> {
    if value == "0" {
        return Ok(Duration::ZERO);
    }
    if value.is_empty() {
        return Err("duration must not be empty".to_string());
    }
    let input = value.strip_prefix('+').unwrap_or(value);
    if input.starts_with('-') {
        return Err("duration must not be negative".to_string());
    }

    let mut offset = 0;
    let mut segments = 0;
    let mut total_nanos = 0u128;
    while offset < input.len() {
        let number_start = offset;
        while input.as_bytes().get(offset).is_some_and(u8::is_ascii_digit) {
            offset += 1;
        }
        if input.as_bytes().get(offset) == Some(&b'.') {
            offset += 1;
            while input.as_bytes().get(offset).is_some_and(u8::is_ascii_digit) {
                offset += 1;
            }
        }
        let number = &input[number_start..offset];
        if number.is_empty() || number == "." {
            return Err(format!("invalid duration {value:?}"));
        }
        let (unit, nanos_per_unit) = duration_unit(&input[offset..])
            .ok_or_else(|| format!("invalid duration unit in {value:?}"))?;
        offset += unit.len();
        let nanos = decimal_nanos(number, nanos_per_unit)
            .map_err(|message| format!("invalid duration {value:?}: {message}"))?;
        total_nanos = total_nanos
            .checked_add(nanos)
            .ok_or_else(|| format!("duration {value:?} overflows"))?;
        segments += 1;
    }

    if segments == 0 || total_nanos > u64::MAX as u128 {
        return Err(format!("duration {value:?} overflows"));
    }
    Ok(Duration::from_nanos(total_nanos as u64))
}

fn duration_unit(value: &str) -> Option<(&'static str, u128)> {
    [
        ("ns", 1u128),
        ("us", 1_000),
        ("µs", 1_000),
        ("μs", 1_000),
        ("ms", 1_000_000),
        ("s", 1_000_000_000),
        ("m", 60_000_000_000),
        ("h", 3_600_000_000_000),
    ]
    .into_iter()
    .find(|(unit, _)| value.starts_with(unit))
}

fn decimal_nanos(value: &str, nanos_per_unit: u128) -> Result<u128, &'static str> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let whole = if whole.is_empty() {
        0
    } else {
        whole.parse::<u128>().map_err(|_| "number is too large")?
    };
    let base = whole
        .checked_mul(nanos_per_unit)
        .ok_or("number is too large")?;
    if fraction.is_empty() {
        return Ok(base);
    }

    let mut fraction_digits = 0u128;
    let mut digits = 0u32;
    for byte in fraction.bytes().take(9) {
        fraction_digits = fraction_digits * 10 + u128::from(byte - b'0');
        digits += 1;
    }
    let fractional_nanos = fraction_digits
        .checked_mul(nanos_per_unit)
        .ok_or("number is too large")?
        / 10u128.pow(digits);
    base.checked_add(fractional_nanos)
        .ok_or("number is too large")
}

#[cfg(test)]
mod tests {
    #[test]
    fn duration_parser_accepts_go_compounds() {
        use super::parse_go_duration;
        use std::time::Duration;

        assert_eq!(
            parse_go_duration("100ms").unwrap(),
            Duration::from_millis(100)
        );
        assert_eq!(parse_go_duration("1m30s").unwrap(), Duration::from_secs(90));
        assert!(parse_go_duration("-1s").is_err());
    }
}
