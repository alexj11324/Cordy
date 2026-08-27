//! cordy-migrate — Rust replacement for `server/cmd/migrate`.
//!
//! Usage: `cordy-migrate up|down|status` (DATABASE_URL defaults to the local
//! Cordy database when unset).
//! The operator backfill is available as
//! `cordy-migrate backfill task-usage-hourly [flags]`.

mod backfill;
mod files;
mod hooks;
mod index_maps;
mod runner;

use std::{env, ffi::OsStr, path::Path, time::Duration};

use chrono::{DateTime, Utc};
use sqlx::postgres::PgPoolOptions;
use tokio_util::sync::CancellationToken;

use crate::backfill::task_usage::OperatorOptions;
use crate::backfill::{codex_usage, issue_activity, task_usage};

const DEFAULT_DATABASE_URL: &str = "postgres://cordy:cordy@localhost:5432/cordy?sslmode=disable";

const MIGRATION_HELP: &str = "usage: cordy-migrate up|down|status | cordy-migrate backfill task-usage-hourly [flags] | cordy-migrate backfill issue-last-activity [flags] | cordy-migrate backfill codex-usage-cache [flags]";
const TASK_USAGE_HELP: &str = "Usage: backfill_task_usage_hourly [flags]\n\nFlags:\n  -h, -help, --help  Show this help\n  -dry-run, --dry-run  Do not write changes\n  -force-partial, --force-partial  Allow partial backfill\n  -months-back N, --months-back=N  Limit the lookback window (0 = all history)\n  -sleep-between-slices DURATION, --sleep-between-slices=DURATION  Pause between monthly slices";
const ISSUE_ACTIVITY_HELP: &str = "Usage: backfill_issue_last_activity [flags]\n\nFlags:\n  -h, -help, --help  Show this help\n  -batch-size N, --batch-size=N  Maximum issue rows per transaction\n  -sleep-between-batches DURATION, --sleep-between-batches=DURATION  Pause between batches\n  -max-batches N, --max-batches=N  Maximum batches (0 = finish all)\n  -max-stalled-passes N, --max-stalled-passes=N  Fail after N stalled passes (0 = disable)";
const CODEX_USAGE_HELP: &str = "Usage: backfill_codex_usage_cache [flags]\n\nFlags:\n  -h, -help, --help  Show this help\n  -cutoff TIMESTAMP, --cutoff=TIMESTAMP  Required RFC3339 deployment time\n  -workspace-id UUID, --workspace-id=UUID  Limit the backfill to one workspace\n  -batch-size N, --batch-size=N  Rows per update batch\n  -sleep-between-batches DURATION, --sleep-between-batches=DURATION  Pause between update batches\n  -execute, --execute  Mutate task_usage rows (default: dry-run)\n  -rebuild-rollup[=BOOL], --rebuild-rollup[=BOOL]  Rebuild the hourly rollup after updates (default: true)";
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cordy_migrate=info".into()),
        )
        .init();

    let mut raw_args = env::args_os();
    let program = raw_args.next();
    let args: Vec<String> = raw_args
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let command = parse_invocation(program.as_deref(), &args)?;

    if let Command::Help(text) = &command {
        println!("{text}");
        return Ok(());
    }

    if let Command::BackfillTaskUsageHourly(options) = &command {
        return run_operator_command(BackfillCommand::TaskUsage(options.clone())).await;
    }
    if let Command::BackfillIssueLastActivity(options) = &command {
        return run_operator_command(BackfillCommand::IssueActivity(options.clone())).await;
    }
    if let Command::BackfillCodexUsageCache(options) = &command {
        return run_operator_command(BackfillCommand::CodexUsage(options.clone())).await;
    }

    // Keep the Go runner's local-development fallback. The Make/script paths
    // normally provide DATABASE_URL, but direct `cordy-migrate up` and the
    // legacy `go run ./cmd/migrate up` both target the local Cordy database
    // when it is unset or explicitly empty.
    let db_url = env::var("DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_DATABASE_URL.to_string());
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
        Command::BackfillIssueLastActivity(_) => unreachable!("handled above"),
        Command::BackfillCodexUsageCache(_) => unreachable!("handled above"),
        Command::Help(_) => unreachable!("handled above"),
    }
}

enum Command {
    Help(&'static str),
    Up,
    Down,
    Status,
    BackfillTaskUsageHourly(OperatorOptions),
    BackfillIssueLastActivity(issue_activity::OperatorOptions),
    BackfillCodexUsageCache(codex_usage::OperatorOptions),
}

fn parse_command(args: &[String]) -> anyhow::Result<Command> {
    match args {
        [command, backfill, rest @ ..]
            if command == "backfill"
                && backfill == "task-usage-hourly"
                && rest.iter().any(|arg| is_help_arg(arg)) =>
        {
            Ok(Command::Help(TASK_USAGE_HELP))
        }
        [command, backfill, rest @ ..]
            if command == "backfill"
                && backfill == "issue-last-activity"
                && rest.iter().any(|arg| is_help_arg(arg)) =>
        {
            Ok(Command::Help(ISSUE_ACTIVITY_HELP))
        }
        [command, backfill, rest @ ..]
            if command == "backfill"
                && backfill == "codex-usage-cache"
                && rest.iter().any(|arg| is_help_arg(arg)) =>
        {
            Ok(Command::Help(CODEX_USAGE_HELP))
        }
        _ if args.iter().any(|arg| is_help_arg(arg)) => Ok(Command::Help(MIGRATION_HELP)),
        [] => Ok(Command::Up),
        [command] if command == "up" => Ok(Command::Up),
        [command] if command == "down" => Ok(Command::Down),
        [command] if command == "status" => Ok(Command::Status),
        [command, backfill, rest @ ..]
            if command == "backfill" && backfill == "task-usage-hourly" =>
        {
            Ok(Command::BackfillTaskUsageHourly(parse_operator_options(
                rest,
            )?))
        }
        [command, backfill, rest @ ..]
            if command == "backfill" && backfill == "issue-last-activity" =>
        {
            Ok(Command::BackfillIssueLastActivity(
                parse_issue_activity_options(rest)?,
            ))
        }
        [command, backfill, rest @ ..]
            if command == "backfill" && backfill == "codex-usage-cache" =>
        {
            Ok(Command::BackfillCodexUsageCache(parse_codex_usage_options(
                rest,
            )?))
        }
        _ => anyhow::bail!("{MIGRATION_HELP}"),
    }
}

/// Preserve the standalone Go backfill command names while routing them to
/// the Rust migration runner. The self-host image installs these names as
/// symlinks to `migrate`, and operators may also invoke the same aliases from
/// a locally built binary.
fn parse_invocation(program: Option<&OsStr>, args: &[String]) -> anyhow::Result<Command> {
    let program_name = program
        .and_then(|program| Path::new(program).file_stem())
        .and_then(OsStr::to_str)
        .unwrap_or("cordy-migrate");
    let help = match program_name {
        "backfill_task_usage_hourly" => Some(TASK_USAGE_HELP),
        "backfill_issue_last_activity" => Some(ISSUE_ACTIVITY_HELP),
        "backfill_codex_usage_cache" => Some(CODEX_USAGE_HELP),
        _ => None,
    };
    if let Some(help) = help {
        if args.iter().any(|arg| is_help_arg(arg)) {
            return Ok(Command::Help(help));
        }
    }

    let Some([command, subcommand]) = (match program_name {
        "backfill_task_usage_hourly" => Some(["backfill", "task-usage-hourly"]),
        "backfill_issue_last_activity" => Some(["backfill", "issue-last-activity"]),
        "backfill_codex_usage_cache" => Some(["backfill", "codex-usage-cache"]),
        _ => None,
    }) else {
        return parse_command(args);
    };

    let mut translated = Vec::with_capacity(args.len() + 2);
    translated.push(command.to_string());
    translated.push(subcommand.to_string());
    translated.extend(args.iter().cloned());
    parse_command(&translated)
}

fn is_help_arg(arg: &str) -> bool {
    matches!(arg, "-h" | "-help" | "--help")
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
        } else if let Some(value) = option.strip_prefix("dry-run=") {
            options.dry_run = parse_bool(value, "--dry-run")?;
        } else if option == "force-partial" {
            options.force_partial = true;
        } else if let Some(value) = option.strip_prefix("force-partial=") {
            options.force_partial = parse_bool(value, "--force-partial")?;
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

fn parse_issue_activity_options(
    args: &[String],
) -> anyhow::Result<issue_activity::OperatorOptions> {
    let mut options = issue_activity::OperatorOptions::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let option = arg
            .strip_prefix("--")
            .or_else(|| arg.strip_prefix('-'))
            .unwrap_or(arg.as_str());
        if let Some(value) = option.strip_prefix("batch-size=") {
            options.batch_size = parse_i64(value, "--batch-size")?;
        } else if option == "batch-size" {
            index += 1;
            options.batch_size = parse_i64(
                args.get(index)
                    .ok_or_else(|| anyhow::anyhow!("--batch-size requires a value"))?,
                "--batch-size",
            )?;
        } else if let Some(value) = option.strip_prefix("sleep-between-batches=") {
            options.sleep_between_batches =
                parse_non_negative_duration(value, "--sleep-between-batches")?;
        } else if option == "sleep-between-batches" {
            index += 1;
            options.sleep_between_batches = parse_non_negative_duration(
                args.get(index)
                    .ok_or_else(|| anyhow::anyhow!("--sleep-between-batches requires a value"))?,
                "--sleep-between-batches",
            )?;
        } else if let Some(value) = option.strip_prefix("max-batches=") {
            options.max_batches = parse_i64(value, "--max-batches")?;
        } else if option == "max-batches" {
            index += 1;
            options.max_batches = parse_i64(
                args.get(index)
                    .ok_or_else(|| anyhow::anyhow!("--max-batches requires a value"))?,
                "--max-batches",
            )?;
        } else if let Some(value) = option.strip_prefix("max-stalled-passes=") {
            options.max_stalled_passes = parse_i64(value, "--max-stalled-passes")?;
        } else if option == "max-stalled-passes" {
            index += 1;
            options.max_stalled_passes = parse_i64(
                args.get(index)
                    .ok_or_else(|| anyhow::anyhow!("--max-stalled-passes requires a value"))?,
                "--max-stalled-passes",
            )?;
        } else {
            anyhow::bail!("unknown issue last-activity option {arg:?}");
        }
        index += 1;
    }
    if options.batch_size < 1 {
        anyhow::bail!("--batch-size must be at least 1");
    }
    if options.max_batches < 0 {
        anyhow::bail!("--max-batches must not be negative");
    }
    if options.max_stalled_passes < 0 {
        anyhow::bail!("--max-stalled-passes must not be negative");
    }
    Ok(options)
}

fn parse_codex_usage_options(args: &[String]) -> anyhow::Result<codex_usage::OperatorOptions> {
    let mut cutoff_raw: Option<String> = None;
    let mut workspace_id = String::new();
    let mut batch_size = 1000_i64;
    let mut sleep_between_batches = Duration::ZERO;
    let mut execute = false;
    let mut rebuild_rollup = true;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        let option = arg
            .strip_prefix("--")
            .or_else(|| arg.strip_prefix('-'))
            .unwrap_or(arg.as_str());
        if let Some(value) = option.strip_prefix("cutoff=") {
            cutoff_raw = Some(value.to_string());
        } else if option == "cutoff" {
            index += 1;
            cutoff_raw = Some(
                args.get(index)
                    .ok_or_else(|| anyhow::anyhow!("--cutoff requires a value"))?
                    .clone(),
            );
        } else if let Some(value) = option.strip_prefix("workspace-id=") {
            workspace_id = value.to_string();
        } else if option == "workspace-id" {
            index += 1;
            workspace_id = args
                .get(index)
                .ok_or_else(|| anyhow::anyhow!("--workspace-id requires a value"))?
                .clone();
        } else if let Some(value) = option.strip_prefix("batch-size=") {
            batch_size = parse_i64(value, "--batch-size")?;
        } else if option == "batch-size" {
            index += 1;
            batch_size = parse_i64(
                args.get(index)
                    .ok_or_else(|| anyhow::anyhow!("--batch-size requires a value"))?,
                "--batch-size",
            )?;
        } else if let Some(value) = option.strip_prefix("sleep-between-batches=") {
            sleep_between_batches = parse_duration(value)?;
        } else if option == "sleep-between-batches" {
            index += 1;
            sleep_between_batches = parse_duration(
                args.get(index)
                    .ok_or_else(|| anyhow::anyhow!("--sleep-between-batches requires a value"))?,
            )?;
        } else if let Some(value) = option.strip_prefix("execute=") {
            execute = parse_bool(value, "--execute")?;
        } else if option == "execute" {
            execute = true;
        } else if let Some(value) = option.strip_prefix("rebuild-rollup=") {
            rebuild_rollup = parse_bool(value, "--rebuild-rollup")?;
        } else if option == "rebuild-rollup" {
            rebuild_rollup = true;
        } else {
            anyhow::bail!("unknown Codex usage backfill option {arg:?}");
        }
        index += 1;
    }

    let cutoff_raw = cutoff_raw.ok_or_else(|| {
        anyhow::anyhow!(
            "--cutoff is required; use the hosted deployment time of the Codex usage normalization fix"
        )
    })?;
    let cutoff = DateTime::parse_from_rfc3339(&cutoff_raw)
        .map_err(|error| anyhow::anyhow!("parse --cutoff as RFC3339: {error}"))?
        .with_timezone(&Utc);
    if cutoff >= Utc::now() {
        anyhow::bail!(
            "--cutoff must be before now; refusing a future cutoff because it could double-subtract already-normalized rows"
        );
    }
    if batch_size <= 0 {
        anyhow::bail!("--batch-size must be positive");
    }

    Ok(codex_usage::OperatorOptions {
        cutoff,
        workspace_id,
        batch_size,
        sleep_between_batches,
        execute,
        rebuild_rollup,
    })
}

fn parse_i64(value: &str, flag: &str) -> anyhow::Result<i64> {
    value
        .parse::<i64>()
        .map_err(|error| anyhow::anyhow!("invalid {flag} value {value:?}: {error}"))
}

fn parse_bool(value: &str, flag: &str) -> anyhow::Result<bool> {
    match value {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => anyhow::bail!("invalid {flag} value {value:?}; want true or false"),
    }
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

fn parse_non_negative_duration(value: &str, flag: &str) -> anyhow::Result<Duration> {
    if value.starts_with('-') {
        anyhow::bail!("{flag} must not be negative");
    }
    parse_duration(value)
}

enum BackfillCommand {
    TaskUsage(OperatorOptions),
    IssueActivity(issue_activity::OperatorOptions),
    CodexUsage(codex_usage::OperatorOptions),
}

async fn run_operator_command(command: BackfillCommand) -> anyhow::Result<()> {
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
        let result = match command {
            BackfillCommand::TaskUsage(options) => {
                task_usage::run_operator(&pool, options, &cancellation).await
            }
            BackfillCommand::IssueActivity(options) => {
                issue_activity::run_operator(&pool, options, &cancellation).await
            }
            BackfillCommand::CodexUsage(options) => {
                codex_usage::run_operator(&pool, options, &cancellation).await
            }
        };
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
    fn issue_activity_flags_accept_go_short_and_long_forms() {
        let args = [
            "backfill".to_string(),
            "issue-last-activity".to_string(),
            "-batch-size=17".to_string(),
            "--sleep-between-batches".to_string(),
            "250ms".to_string(),
            "-max-batches".to_string(),
            "4".to_string(),
            "--max-stalled-passes=3".to_string(),
        ];
        let Command::BackfillIssueLastActivity(options) = parse_command(&args)
            .unwrap_or_else(|error| panic!("parse issue activity flags: {error}"))
        else {
            panic!("expected issue activity backfill command");
        };
        assert_eq!(options.batch_size, 17);
        assert_eq!(options.sleep_between_batches, Duration::from_millis(250));
        assert_eq!(options.max_batches, 4);
        assert_eq!(options.max_stalled_passes, 3);
    }

    #[test]
    fn issue_activity_defaults_match_go_operator() {
        let options = issue_activity::OperatorOptions::default();
        assert_eq!(options.batch_size, 1000);
        assert_eq!(options.sleep_between_batches, Duration::from_millis(100));
        assert_eq!(options.max_batches, 0);
        assert_eq!(options.max_stalled_passes, 10);
    }

    #[test]
    fn legacy_backfill_program_names_route_to_rust_commands() {
        let task_args = vec!["--dry-run".to_string()];
        let Command::BackfillTaskUsageHourly(options) = parse_invocation(
            Some(OsStr::new("/app/backfill_task_usage_hourly")),
            &task_args,
        )
        .expect("task alias should parse") else {
            panic!("expected task usage backfill command");
        };
        assert!(options.dry_run);

        let issue_args = vec!["--max-batches".to_string(), "1".to_string()];
        let Command::BackfillIssueLastActivity(options) = parse_invocation(
            Some(OsStr::new("backfill_issue_last_activity")),
            &issue_args,
        )
        .expect("issue alias should parse") else {
            panic!("expected issue activity backfill command");
        };
        assert_eq!(options.max_batches, 1);

        let codex_args = vec!["--cutoff".to_string(), "2000-01-01T00:00:00Z".to_string()];
        let Command::BackfillCodexUsageCache(_) = parse_invocation(
            Some(OsStr::new("backfill_codex_usage_cache.exe")),
            &codex_args,
        )
        .expect("Codex alias should parse") else {
            panic!("expected Codex usage backfill command");
        };
    }

    #[test]
    fn legacy_backfill_flags_preserve_go_bool_and_help_forms() {
        let args = vec![
            "--dry-run=false".to_string(),
            "-force-partial=true".to_string(),
        ];
        let Command::BackfillTaskUsageHourly(options) =
            parse_invocation(Some(OsStr::new("backfill_task_usage_hourly")), &args)
                .expect("explicit boolean forms should parse")
        else {
            panic!("expected task usage backfill command");
        };
        assert!(!options.dry_run);
        assert!(options.force_partial);

        let Command::Help(text) = parse_invocation(
            Some(OsStr::new("backfill_issue_last_activity")),
            &["--help".to_string()],
        )
        .expect("issue activity help should parse") else {
            panic!("expected issue activity help");
        };
        assert!(text.contains("-max-stalled-passes"));
    }

    #[test]
    fn codex_alias_help_lists_every_legacy_flag() {
        let Command::Help(text) = parse_invocation(
            Some(OsStr::new("backfill_codex_usage_cache")),
            &["-h".to_string()],
        )
        .expect("Codex help should parse") else {
            panic!("expected Codex help");
        };
        for flag in [
            "--cutoff",
            "--workspace-id",
            "--batch-size",
            "--sleep-between-batches",
            "--execute",
            "--rebuild-rollup",
        ] {
            assert!(text.contains(flag), "help is missing {flag}");
        }
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
