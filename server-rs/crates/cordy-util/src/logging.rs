//! Process logging environment shared by the Rust entrypoints.
//!
//! This keeps the Go logger's small process-level contract in one place
//! without coupling the dependency-light utility crate to a subscriber
//! implementation: `LOG_LEVEL` wins, invalid values mean debug, and
//! `RUST_LOG` remains an explicit Rust-only fallback for existing operators.

use std::ffi::OsStr;
use std::io::IsTerminal;

pub const DEFAULT_LEVEL: &str = "debug";

/// Resolve the process filter from the environment.
pub fn env_filter() -> String {
    // `var` treats a present non-UTF-8 value as if it were absent. That would
    // incorrectly let RUST_LOG win over an invalid LOG_LEVEL on Unix. Go's
    // os.Getenv returns the raw string and parseLevel falls back to debug, so
    // preserve the fact that LOG_LEVEL was present while applying the same
    // invalid-value behavior here.
    let log_level = std::env::var_os("LOG_LEVEL");
    let rust_log = std::env::var_os("RUST_LOG");
    filter_from_os_values(log_level.as_deref(), rust_log.as_deref())
}

fn filter_from_os_values(log_level: Option<&OsStr>, rust_log: Option<&OsStr>) -> String {
    let log_level = log_level.map(|value| value.to_string_lossy().into_owned());
    let rust_log = rust_log.map(|value| value.to_string_lossy().into_owned());
    filter_from_values(log_level.as_deref(), rust_log.as_deref())
}

/// Resolve a filter from explicit values so the precedence and Go-compatible
/// level parsing can be tested without mutating process-global environment.
pub fn filter_from_values(log_level: Option<&str>, rust_log: Option<&str>) -> String {
    match log_level {
        Some(value) => normalize_level(value).to_string(),
        None => rust_log
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_LEVEL)
            .to_string(),
    }
}

/// Parse the values accepted by the Go logger. Unknown and empty values
/// intentionally fall back to debug, matching `logger.parseLevel`.
pub fn normalize_level(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "info" => "info",
        "warn" | "warning" => "warn",
        "error" => "error",
        _ => DEFAULT_LEVEL,
    }
}

/// Match Go's `isTerminal(os.Stderr)` decision for ANSI output.
pub fn stderr_is_terminal() -> bool {
    std::io::stderr().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_parser_matches_go_defaults_and_aliases() {
        assert_eq!(normalize_level(" info "), "info");
        assert_eq!(normalize_level("WARNING"), "warn");
        assert_eq!(normalize_level("error"), "error");
        assert_eq!(normalize_level(""), "debug");
        assert_eq!(normalize_level("trace"), "debug");
    }

    #[test]
    fn log_level_takes_precedence_over_rust_log() {
        assert_eq!(filter_from_values(Some("warn"), Some("trace")), "warn");
        assert_eq!(filter_from_values(Some("invalid"), Some("trace")), "debug");
    }

    #[test]
    fn rust_log_is_preserved_when_log_level_is_unset() {
        assert_eq!(
            filter_from_values(None, Some("cordy=trace,tower=info")),
            "cordy=trace,tower=info"
        );
        assert_eq!(filter_from_values(None, Some("  ")), "debug");
        assert_eq!(filter_from_values(None, None), "debug");
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_log_level_is_invalid_instead_of_unset() {
        use std::os::unix::ffi::OsStrExt;

        let invalid = OsStr::from_bytes(b"\xff");
        assert_eq!(
            filter_from_os_values(Some(invalid), Some(OsStr::new("trace"))),
            "debug"
        );
    }
}
