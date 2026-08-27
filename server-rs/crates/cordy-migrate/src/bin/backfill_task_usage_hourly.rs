//! Seeds `task_usage_hourly` from historical `task_usage` rows.
//!
//! This is the Rust operator entrypoint for the self-host upgrade path. It
//! shares the idempotent monthly rollup and watermark code with the migration
//! hook in `cordy_migrate::backfill::task_usage`.

use anyhow::Context as _;
use clap::Parser;
use cordy_migrate::backfill::task_usage::{run_standalone, StandaloneOptions};
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

const DEFAULT_DATABASE_URL: &str = "postgres://cordy:cordy@localhost:5432/cordy?sslmode=disable";

#[derive(Debug, Parser)]
#[command(
    name = "backfill_task_usage_hourly",
    about = "Seed task_usage_hourly from historical task_usage rows"
)]
struct Args {
    /// Log monthly slices without changing task_usage_hourly or its watermark.
    #[arg(long)]
    dry_run: bool,

    /// Limit the backfill to the last N months; zero means all available history.
    #[arg(long, default_value_t = 0)]
    months_back: i64,

    /// Acknowledge that a partial backfill leaves older buckets empty.
    #[arg(long)]
    force_partial: bool,

    /// Pause between monthly slices to reduce source-table read pressure.
    #[arg(long, default_value = "0s", value_parser = parse_go_duration)]
    sleep_between_slices: Duration,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cordy_migrate::init_logging();

    let args = Args::parse();
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

    let options = StandaloneOptions {
        dry_run: args.dry_run,
        months_back: args.months_back,
        force_partial: args.force_partial,
        sleep_between_slices: args.sleep_between_slices,
    };

    tokio::select! {
        result = run_standalone(&pool, options) => result,
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

// ponytail: keep this small parser local; adding a duration crate for one
// operator flag would duplicate the existing Go-compatible boundary for no
// production benefit.
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
    if input == "0" {
        return Ok(Duration::ZERO);
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
        if number == "." || number.is_empty() {
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
    let scale = 10u128.pow(digits);
    let fractional_nanos = fraction_digits
        .checked_mul(nanos_per_unit)
        .ok_or("number is too large")?
        / scale;
    base.checked_add(fractional_nanos)
        .ok_or("number is too large")
}

#[cfg(test)]
mod tests {
    use super::parse_go_duration;
    use std::time::Duration;

    #[test]
    fn parses_go_duration_units_and_compounds() {
        assert_eq!(parse_go_duration("0").unwrap(), Duration::ZERO);
        assert_eq!(
            parse_go_duration("1.5ms").unwrap(),
            Duration::from_micros(1_500)
        );
        assert_eq!(parse_go_duration("1m30s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_go_duration("2µs").unwrap(), Duration::from_micros(2));
    }

    #[test]
    fn rejects_negative_or_malformed_durations() {
        assert!(parse_go_duration("-1s").is_err());
        assert!(parse_go_duration("1").is_err());
        assert!(parse_go_duration("1xs").is_err());
    }
}
