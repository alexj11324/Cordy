//! Shared migration backfills used by the migration runner and operator tools.

use std::sync::OnceLock;

pub mod backfill;
mod files;

/// Every migration version required by the current source tree, in apply
/// order. Runtime readiness and the migration CLI share this discovery logic
/// so neither can accidentally treat a database with an interior gap as ready.
pub fn required_versions() -> anyhow::Result<&'static [String]> {
    static REQUIRED_VERSIONS: OnceLock<Vec<String>> = OnceLock::new();
    if let Some(versions) = REQUIRED_VERSIONS.get() {
        return Ok(versions);
    }
    let discovered = files::all_versions()?;
    Ok(REQUIRED_VERSIONS.get_or_init(|| discovered))
}

/// Install the same process logger for the migration runner and every
/// backfill binary. Keeping this at the package boundary avoids four subtly
/// different startup filters for one operator-facing command family.
pub fn init_logging() {
    let (writer, terminal) = logging_output();
    let log_filter = tracing_subscriber::EnvFilter::try_new(patchbay_util::logging::env_filter())
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug"));
    tracing_subscriber::fmt()
        .with_env_filter(log_filter)
        .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
            patchbay_util::logging::LOCAL_TIME_FORMAT.to_string(),
        ))
        .with_writer(writer)
        .with_ansi(terminal)
        .init();
}

/// Keep the writer and terminal decision on the same stream. The formatter's
/// implicit default is stdout, while the Go migration/logger contract and the
/// existing TTY check both use stderr.
fn logging_output() -> (fn() -> std::io::Stderr, bool) {
    (std::io::stderr, patchbay_util::logging::stderr_is_terminal())
}

#[cfg(test)]
mod tests {
    use super::logging_output;

    #[test]
    fn migration_logging_writer_matches_the_stderr_tty_decision() {
        let (make_writer, terminal) = logging_output();
        let _: std::io::Stderr = make_writer();
        assert_eq!(terminal, patchbay_util::logging::stderr_is_terminal());
    }
}
