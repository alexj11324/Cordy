//! Process logging environment shared by the Rust entrypoints.
//!
//! This keeps the Go logger's small process-level contract in one place
//! without coupling the dependency-light utility crate to a subscriber
//! implementation: `LOG_LEVEL` wins, invalid values mean debug, and
//! `RUST_LOG` remains an explicit Rust-only fallback for existing operators.

use std::io::IsTerminal;

pub const DEFAULT_LEVEL: &str = "debug";
pub const LOCAL_TIME_FORMAT: &str = "%H:%M:%S%.3f";

/// Resolve the process filter from the environment.
pub fn env_filter() -> String {
    let log_level = std::env::var("LOG_LEVEL").ok();
    let rust_log = std::env::var("RUST_LOG").ok();
    filter_from_values(log_level.as_deref(), rust_log.as_deref())
}

/// Resolve a filter from explicit values so the precedence and Go-compatible
/// level parsing can be tested without mutating process-global environment.
pub fn filter_from_values(log_level: Option<&str>, rust_log: Option<&str>) -> String {
    match log_level {
        Some(value) => cordy_scoped_filter(normalize_level(value)),
        None => rust_log
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| cordy_scoped_filter(DEFAULT_LEVEL)),
    }
}

fn cordy_scoped_filter(level: &str) -> String {
    format!("warn,cordy={level}")
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
        assert_eq!(
            filter_from_values(Some("warn"), Some("trace")),
            "warn,cordy=warn"
        );
        assert_eq!(
            filter_from_values(Some("invalid"), Some("trace")),
            "warn,cordy=debug"
        );
    }

    #[test]
    fn rust_log_is_preserved_when_log_level_is_unset() {
        assert_eq!(
            filter_from_values(None, Some("cordy=trace,tower=info")),
            "cordy=trace,tower=info"
        );
        assert_eq!(filter_from_values(None, Some("  ")), "warn,cordy=debug");
        assert_eq!(filter_from_values(None, None), "warn,cordy=debug");
    }

    #[test]
    fn local_time_layout_matches_the_operator_contract() {
        let time =
            chrono::NaiveTime::from_hms_milli_opt(14, 3, 2, 7).unwrap_or_else(|| unreachable!());
        assert_eq!(time.format(LOCAL_TIME_FORMAT).to_string(), "14:03:02.007");
    }
}
