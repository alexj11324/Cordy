//! Shared migration backfills used by the migration runner and operator tools.

pub mod backfill;

/// Install the same process logger for the migration runner and every
/// backfill binary. Keeping this at the package boundary avoids four subtly
/// different startup filters for one operator-facing command family.
pub fn init_logging() {
    let log_filter = tracing_subscriber::EnvFilter::try_new(cordy_util::logging::env_filter())
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug"));
    tracing_subscriber::fmt()
        .with_env_filter(log_filter)
        .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
            cordy_util::logging::LOCAL_TIME_FORMAT.to_string(),
        ))
        .with_ansi(cordy_util::logging::stderr_is_terminal())
        .init();
}
