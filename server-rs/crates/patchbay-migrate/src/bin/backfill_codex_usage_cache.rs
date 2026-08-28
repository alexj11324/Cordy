//! Repairs Codex `task_usage.input_tokens` rows written before cached input
//! was normalized at ingestion time.
//!
//! The command is dry-run by default. Pass the hosted deployment timestamp of
//! the ingestion fix with `--cutoff`, review the grouped summary, and use
//! `--execute` only when the candidate rows are confirmed.

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use clap::Parser;
use patchbay_migrate::backfill::codex_usage::{self, Options};
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const DEFAULT_DATABASE_URL: &str = "postgres://patchbay:patchbay@localhost:5432/patchbay?sslmode=disable";

#[derive(Debug, Parser)]
#[command(
    name = "backfill_codex_usage_cache",
    about = "Repair historical Codex cached-input usage rows"
)]
struct Args {
    /// RFC3339 hosted deployment time of the Codex usage normalization fix.
    #[arg(long)]
    cutoff: Option<String>,

    /// Optional workspace UUID to limit the backfill.
    #[arg(long, default_value = "")]
    workspace_id: String,

    /// Number of task_usage rows to update per batch.
    #[arg(long, default_value_t = 1_000)]
    batch_size: i64,

    /// Pause between update batches to throttle write pressure.
    #[arg(long, default_value = "0s", value_parser = parse_go_duration)]
    sleep_between_batches: Duration,

    /// Mutate task_usage rows; without this flag only a dry-run summary is printed.
    #[arg(long)]
    execute: bool,

    /// Rebuild the hourly rollup for the update window after execution.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    rebuild_rollup: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    patchbay_util::install_rustls_crypto_provider()?;
    patchbay_migrate::init_logging();

    let args = Args::parse();
    let options = options_from_args(&args, Utc::now())?;
    let configured_db_url = std::env::var_os("DATABASE_URL")
        .map(|value| {
            value
                .into_string()
                .map_err(|_| anyhow::anyhow!("DATABASE_URL must be valid UTF-8"))
        })
        .transpose()?;
    let db_url = configured_database_url(configured_db_url.as_deref());
    let cancellation = CancellationToken::new();
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let pool = PgPoolOptions::new().max_connections(2).connect(db_url);
    let pool = tokio::select! {
        result = pool => result.context("connect to database")?,
        _ = &mut shutdown => anyhow::bail!("backfill interrupted by signal"),
    };
    tokio::select! {
        result = sqlx::query("SELECT 1").execute(&pool) => {
            result.context("ping database")?;
        }
        _ = &mut shutdown => anyhow::bail!("backfill interrupted by signal"),
    }

    let run_future = codex_usage::run(&pool, options, &cancellation);
    tokio::pin!(run_future);
    tokio::select! {
        result = &mut run_future => result,
        _ = &mut shutdown => {
            cancellation.cancel();
            run_future.await
        },
    }
}

fn options_from_args(args: &Args, now: DateTime<Utc>) -> anyhow::Result<Options> {
    let cutoff_raw = args.cutoff.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "--cutoff is required; use the hosted deployment time of the Codex usage normalization fix"
        )
    })?;
    let cutoff = DateTime::parse_from_rfc3339(cutoff_raw)
        .map_err(|error| anyhow::anyhow!("parse --cutoff as RFC3339: {error}"))?
        .with_timezone(&Utc);
    if cutoff >= now {
        anyhow::bail!(
            "--cutoff must be before now; refusing a future cutoff because it could double-subtract already-normalized rows"
        );
    }

    let options = Options {
        cutoff,
        workspace_id: args.workspace_id.clone(),
        batch_size: args.batch_size,
        sleep_between_batches: args.sleep_between_batches,
        execute: args.execute,
        rebuild_rollup: args.rebuild_rollup,
    };
    codex_usage::validate_options(&options)?;
    Ok(options)
}

fn configured_database_url(value: Option<&str>) -> &str {
    value
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_DATABASE_URL)
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

    if segments == 0 || total_nanos > i64::MAX as u128 {
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
    use super::{configured_database_url, options_from_args, parse_go_duration, Args};
    use chrono::{TimeZone, Utc};
    use std::time::Duration;

    #[test]
    fn corrected_options_require_a_past_cutoff_and_normalize_to_utc() {
        let now = Utc.with_ymd_and_hms(2026, 6, 18, 3, 30, 0).unwrap();
        let args = Args {
            cutoff: Some("2026-06-18T10:00:00+08:00".to_string()),
            workspace_id: String::new(),
            batch_size: 100,
            sleep_between_batches: Duration::ZERO,
            execute: false,
            rebuild_rollup: true,
        };
        let options = options_from_args(&args, now).unwrap();
        assert_eq!(
            options.cutoff,
            Utc.with_ymd_and_hms(2026, 6, 18, 2, 0, 0).unwrap()
        );
    }

    #[test]
    fn options_reject_missing_future_and_invalid_cutoffs() {
        let now = Utc.with_ymd_and_hms(2026, 6, 18, 3, 30, 0).unwrap();
        let base = || Args {
            cutoff: None,
            workspace_id: String::new(),
            batch_size: 100,
            sleep_between_batches: Duration::ZERO,
            execute: false,
            rebuild_rollup: true,
        };
        assert!(options_from_args(&base(), now).is_err());
        let mut future = base();
        future.cutoff = Some("2026-06-18T04:00:00Z".to_string());
        assert!(options_from_args(&future, now).is_err());
        let mut invalid_batch = base();
        invalid_batch.cutoff = Some("2026-06-18T03:00:00Z".to_string());
        invalid_batch.batch_size = 0;
        assert!(options_from_args(&invalid_batch, now).is_err());
    }

    #[test]
    fn duration_parser_accepts_go_compounds() {
        assert_eq!(parse_go_duration("1m30s").unwrap(), Duration::from_secs(90));
        assert_eq!(
            parse_go_duration("500ms").unwrap(),
            Duration::from_millis(500)
        );
        assert!(parse_go_duration("-1s").is_err());
    }

    #[test]
    fn duration_parser_uses_go_int64_limit() {
        assert_eq!(
            parse_go_duration("2562047h47m16.854775807s").unwrap(),
            Duration::from_nanos(i64::MAX as u64)
        );
        assert!(parse_go_duration("9223372037s").is_err());
    }

    #[test]
    fn empty_database_url_uses_the_local_default() {
        assert_eq!(configured_database_url(None), super::DEFAULT_DATABASE_URL);
        assert_eq!(
            configured_database_url(Some("")),
            super::DEFAULT_DATABASE_URL
        );
        assert_eq!(
            configured_database_url(Some("postgres://example")),
            "postgres://example"
        );
    }
}
