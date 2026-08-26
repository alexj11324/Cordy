//! Shared process logging configuration — port of `server/internal/logger`.
//!
//! The Go server, migration runner, and daemon all read the same `LOG_LEVEL`
//! vocabulary and suppress ANSI when output is redirected. Rust's tracing
//! subscriber remains the sink so existing structured fields and the daemon's
//! rotating writer stay intact; this module owns the cross-process policy.

use std::error::Error;
use std::io::IsTerminal;

use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::EnvFilter;

/// Parses the Go logger's `LOG_LEVEL` values. Unknown and empty values retain
/// the Go default of debug.
pub fn parse_level(raw: &str) -> LevelFilter {
    match raw.trim().to_ascii_lowercase().as_str() {
        "info" => LevelFilter::INFO,
        "warn" | "warning" => LevelFilter::WARN,
        "error" => LevelFilter::ERROR,
        _ => LevelFilter::DEBUG,
    }
}

/// Reports whether stderr is attached to a terminal. ANSI is disabled for
/// redirected daemon/server logs, matching the Go `FileInfo.ModeCharDevice`
/// check.
pub fn stderr_is_terminal() -> bool {
    std::io::stderr().is_terminal()
}

/// Builds the shared local clock format used by the Go tint handler.
pub fn local_timer() -> tracing_subscriber::fmt::time::ChronoLocal {
    tracing_subscriber::fmt::time::ChronoLocal::new("%H:%M:%S%.3f".to_string())
}

/// Selects a filter with the same precedence used by the process entrypoints:
/// an explicit daemon filter, `RUST_LOG`, the Go-compatible `LOG_LEVEL`, then
/// the caller's binary-specific default. Invalid explicit/RUST_LOG filters
/// fail closed to the next source rather than preventing startup.
pub fn env_filter(explicit: Option<&str>, default_filter: &str) -> EnvFilter {
    let explicit = explicit.map(str::trim).filter(|value| !value.is_empty());
    let rust_log = std::env::var("RUST_LOG").ok();
    let rust_log = rust_log
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    for candidate in [explicit, rust_log] {
        if let Some(candidate) = candidate {
            if let Ok(filter) = EnvFilter::try_new(candidate) {
                return filter;
            }
        }
    }

    if let Ok(log_level) = std::env::var("LOG_LEVEL") {
        return EnvFilter::new(scoped_log_level_filter(
            default_filter,
            parse_level(&log_level),
        ));
    }
    EnvFilter::new(default_filter)
}

fn scoped_log_level_filter(default_filter: &str, level: LevelFilter) -> String {
    let directives = default_filter
        .split(',')
        .map(str::trim)
        .filter(|directive| !directive.is_empty())
        .collect::<Vec<_>>();
    let has_scoped_directive = directives.iter().any(|directive| directive.contains('='));
    if !has_scoped_directive {
        return level.to_string();
    }

    directives
        .into_iter()
        .map(|directive| {
            directive.split_once('=').map_or_else(
                || directive.to_string(),
                |(target, _)| format!("{target}={level}"),
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Initializes the process-global tracing subscriber with the shared log
/// policy. This is the Rust equivalent of `logger.Init`.
pub fn init(default_filter: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(env_filter(None, default_filter))
        .with_timer(local_timer())
        .with_target(false)
        .with_ansi(stderr_is_terminal())
        .try_init()
        .map_err(Into::into)
}

/// Creates a component span equivalent to Go's `NewLogger(component)`.
/// Callers can enter the span around a standalone component's work while
/// retaining the same global subscriber and filter policy.
pub fn component_span(component: &str) -> tracing::Span {
    tracing::warn_span!("component", component = component)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_go_log_levels_and_defaults_to_debug() {
        assert_eq!(parse_level("info"), LevelFilter::INFO);
        assert_eq!(parse_level(" WARN "), LevelFilter::WARN);
        assert_eq!(parse_level("warning"), LevelFilter::WARN);
        assert_eq!(parse_level("error"), LevelFilter::ERROR);
        assert_eq!(parse_level("debug"), LevelFilter::DEBUG);
        assert_eq!(parse_level(""), LevelFilter::DEBUG);
        assert_eq!(parse_level("not-a-level"), LevelFilter::DEBUG);
    }

    #[test]
    fn component_span_carries_the_component_field() {
        use tracing::dispatcher::Dispatch;
        use tracing_subscriber::layer::SubscriberExt;

        let subscriber = tracing_subscriber::registry().with(LevelFilter::INFO);
        tracing::dispatcher::with_default(&Dispatch::new(subscriber), || {
            let span = component_span("daemon");
            assert_eq!(
                span.metadata().map(|metadata| metadata.name()),
                Some("component")
            );
        });
    }

    #[test]
    fn log_level_overrides_application_scopes_without_widening_dependencies() {
        assert_eq!(
            scoped_log_level_filter("cordy=info,tower=info", LevelFilter::DEBUG),
            "cordy=debug,tower=debug"
        );
        assert_eq!(
            scoped_log_level_filter("cordy=info,tower=off", LevelFilter::WARN),
            "cordy=warn,tower=warn"
        );
        assert_eq!(scoped_log_level_filter("info", LevelFilter::ERROR), "error");
    }
}
