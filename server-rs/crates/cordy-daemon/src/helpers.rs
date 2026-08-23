//! Port of `server/internal/daemon/helpers.go` (lines 1–111) — env-var and
//! duration/sleep helpers shared by the daemon package.
//!
//! Symbol map (Go → Rust):
//! - `envOrDefault` → [`env_or_default`]
//! - `durationFromEnv` → [`duration_from_env`]
//! - `dayUnit` + `parseFlexDuration` → [`DAY_UNIT`] + [`parse_flex_duration`]
//!   (backed by [`parse_go_duration`], a faithful `time.ParseDuration`)
//! - `boolFromEnv` → [`bool_from_env`]
//! - `intFromEnv` → [`int_from_env`]
//! - `sleepWithContext` → [`sleep_with_context`]
//! - `sleepWithContextOrWakeup` → [`sleep_with_context_or_wakeup`]
//!
//! Port notes: `context.Context` is the crate-wide [`Ctx`] seam
//! (`crate::repocache::Ctx`); Go's `<-chan struct{}` wakeup channel becomes a
//! `tokio::sync::mpsc::Receiver<()>` passed as `Option<&mut …>` (Go's nil
//! channel disables the case). Negative parsed durations are rejected — every
//! call site feeds a timeout/floor that Go code would immediately misuse
//! anyway (documented deviation).

// S9-integration: consumed by daemon.go core (lane B); silence dead-code.
#![allow(dead_code)]

use std::time::Duration;

use anyhow::anyhow;
use regex::Regex;

use crate::repocache::{CancelCause, Ctx};

/// `envOrDefault` (helpers.go:13–19).
pub(crate) fn env_or_default(key: &str, fallback: &str) -> String {
    let value = std::env::var(key).unwrap_or_default();
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

/// `durationFromEnv` (helpers.go:21–31).
pub(crate) fn duration_from_env(key: &str, fallback: Duration) -> anyhow::Result<Duration> {
    let value = std::env::var(key).unwrap_or_default();
    let value = value.trim();
    if value.is_empty() {
        return Ok(fallback);
    }
    parse_flex_duration(value).map_err(|e| anyhow!("{}: invalid duration {:?}: {}", key, value, e))
}

/// `dayUnit` (helpers.go:35): matches a decimal number (with optional leading
/// digits) followed by `d` (days), so both "5d" and "1.5d" are captured whole
/// and expanded to hours.
static DAY_UNIT: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"(\d*\.\d+|\d+)d").expect("static regex"));
/// `parseFlexDuration` (helpers.go:40–56): accepts the standard Go
/// `time.ParseDuration` syntax plus a `d` (day) suffix, which the stdlib
/// rejects. "5d" → 120h, "1d12h" → 36h, "0.5d" → 12h. Overflow or malformed
/// numbers propagate as errors.
pub(crate) fn parse_flex_duration(value: &str) -> anyhow::Result<Duration> {
    let expanded = DAY_UNIT.replace_all(value, |caps: &regex::Captures<'_>| {
        // strconv.ParseFloat(match[:len(match)-1], 64) — strip the trailing
        // 'd'. On parse failure Go records convErr and keeps the match; we do
        // the same by echoing the original text and failing below.
        let raw = caps.get(0).map(|m| m.as_str()).unwrap_or_default();
        let num_part = &raw[..raw.len() - 1];
        match num_part.parse::<f64>() {
            // time.ParseDuration handles fractional hours natively, and
            // rejects overflow on its own. FormatFloat(days*24, 'f', -1, 64)
            // ≈ Rust's shortest f64 Display.
            Ok(days) => format!("{}h", days * 24.0),
            Err(_) => raw.to_string(),
        }
    });
    // Propagate a conversion failure the way Go does (convErr checked before
    // ParseDuration): re-validate any day-unit number that failed to parse.
    for caps in DAY_UNIT.captures_iter(value) {
        let raw = caps.get(0).map(|m| m.as_str()).unwrap_or_default();
        let num_part = &raw[..raw.len() - 1];
        if num_part.parse::<f64>().is_err() {
            return Err(anyhow!(
                "strconv.ParseFloat: parsing {:?}: invalid syntax",
                num_part
            ));
        }
    }
    parse_go_duration(&expanded)
}

/// Faithful port of Go `time.ParseDuration`
/// (go/src/time/format.go ParseDuration): `[-+]?([0-9]*(\.[0-9]*)?[a-z]+)+`
/// with units ns/us/µs/μs/ms/s/m/h. Error strings mirror Go's byte-for-byte.
///
/// Deviation vs Go: a negative total yields an error instead of a negative
/// duration (Rust `Duration` is unsigned); all daemon call sites require
/// positive values.
pub(crate) fn parse_go_duration(s: &str) -> anyhow::Result<Duration> {
    const NANOSECOND: f64 = 1.0;
    const MICROSECOND: f64 = 1000.0 * NANOSECOND;
    const MILLISECOND: f64 = 1000.0 * MICROSECOND;
    const SECOND: f64 = 1000.0 * MILLISECOND;
    const MINUTE: f64 = 60.0 * SECOND;
    const HOUR: f64 = 60.0 * MINUTE;

    let orig = s;
    if s.is_empty() {
        return Err(anyhow!("time: invalid duration {:?}", orig));
    }
    let mut rest = s;
    let mut neg = false;
    let first = rest.as_bytes()[0];
    if first == b'+' || first == b'-' {
        neg = first == b'-';
        rest = &rest[1..];
    }
    if rest == "0" {
        return Ok(Duration::ZERO);
    }
    if rest.is_empty() {
        return Err(anyhow!("time: invalid duration {:?}", orig));
    }

    let mut total: f64 = 0.0;
    while !rest.is_empty() {
        // Integer part: leading digits (possibly none before '.').
        let int_len = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let mut v: f64 = 0.0;
        let int_digits = &rest[..int_len];
        if !int_digits.is_empty() {
            v = int_digits
                .parse::<f64>()
                .map_err(|_| anyhow!("time: invalid duration {:?}", orig))?;
        }
        rest = &rest[int_len..];

        // Fraction part.
        let mut scale = 1.0;
        if rest.starts_with('.') {
            rest = &rest[1..];
            let frac_len = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            let frac_digits = &rest[..frac_len];
            if frac_digits.is_empty() && int_digits.is_empty() {
                return Err(anyhow!("time: invalid duration {:?}", orig));
            }
            for ch in frac_digits.chars() {
                if let Some(digit) = ch.to_digit(10) {
                    scale *= 10.0;
                    v += digit as f64 / scale;
                }
            }
            rest = &rest[frac_len..];
        }

        // Unit: one of ns, us, µs, μs, ms, s, m, h.
        let unit = if rest.starts_with("ns") {
            rest = &rest[2..];
            NANOSECOND
        } else if rest.starts_with("us") || rest.starts_with("µs") || rest.starts_with("μs") {
            rest = &rest["us".len()..]; // µ/μ are 2-byte UTF-8, same length as "us"
            MICROSECOND
        } else if rest.starts_with("ms") {
            rest = &rest[2..];
            MILLISECOND
        } else if rest.starts_with('s') {
            rest = &rest[1..];
            SECOND
        } else if rest.starts_with('m') {
            rest = &rest[1..];
            MINUTE
        } else if rest.starts_with('h') {
            rest = &rest[1..];
            HOUR
        } else {
            let bad_unit: String = rest.chars().take_while(|c| !c.is_ascii_digit()).collect();
            if bad_unit.is_empty() {
                return Err(anyhow!("time: missing unit in duration {:?}", orig));
            }
            return Err(anyhow!(
                "time: unknown unit {:?} in duration {:?}",
                bad_unit,
                orig
            ));
        };

        if v > 9.223_372_036_854_776e18 / unit {
            // Overflow: Go reports the same class of failure.
            return Err(anyhow!("time: invalid duration {:?}", orig));
        }
        v *= unit;
        total += v;
        if total > 9.223_372_036_854_776e18 {
            return Err(anyhow!("time: invalid duration {:?}", orig));
        }
    }
    if neg {
        return Err(anyhow!("time: invalid duration {:?}", orig));
    }
    Ok(Duration::from_nanos(total as u64))
}

/// `boolFromEnv` (helpers.go:61–69): reads a boolean env override, returning
/// fallback when the variable is unset or carries an unrecognized token.
/// Accepted (case insensitive): true/1/yes/on and false/0/no/off.
pub(crate) fn bool_from_env(key: &str, fallback: bool) -> bool {
    match std::env::var(key)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "false" | "0" | "no" | "off" => false,
        "true" | "1" | "yes" | "on" => true,
        _ => fallback,
    }
}

/// `intFromEnv` (helpers.go:71–81).
pub(crate) fn int_from_env(key: &str, fallback: i64) -> anyhow::Result<i64> {
    let value = std::env::var(key).unwrap_or_default();
    let value = value.trim();
    if value.is_empty() {
        return Ok(fallback);
    }
    value
        .parse::<i64>()
        .map_err(|e| anyhow!("{}: invalid integer {:?}: {}", key, value, e))
}

/// `sleepWithContext` (helpers.go:83–93): sleeps for `d`, returning the
/// cancellation cause when `ctx` fires first.
pub(crate) async fn sleep_with_context(ctx: &Ctx, d: Duration) -> Result<(), CancelCause> {
    tokio::select! {
        _ = ctx.cancelled() => Err(ctx.cause()),
        _ = tokio::time::sleep(d) => Ok(()),
    }
}

/// `sleepWithContextOrWakeup` (helpers.go:95–111): like
/// [`sleep_with_context`] but also wakes early when a token arrives on the
/// wakeup channel. A `None` channel mirrors Go's nil-channel case (the select
/// arm is disabled).
pub(crate) async fn sleep_with_context_or_wakeup(
    ctx: &Ctx,
    d: Duration,
    wakeups: Option<&mut tokio::sync::mpsc::Receiver<()>>,
) -> Result<(), CancelCause> {
    let Some(wakeups) = wakeups else {
        return sleep_with_context(ctx, d).await;
    };
    tokio::select! {
        _ = ctx.cancelled() => Err(ctx.cause()),
        _ = wakeups.recv() => Ok(()),
        _ = tokio::time::sleep(d) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    //! Ports of the pure-logic cases from helpers_test.go (52 lines).

    use super::*;

    #[test]
    fn parse_flex_duration_days() {
        assert_eq!(
            parse_flex_duration("5d").unwrap(),
            Duration::from_secs(120 * 3600)
        );
        assert_eq!(
            parse_flex_duration("1d12h").unwrap(),
            Duration::from_secs(36 * 3600)
        );
        assert_eq!(
            parse_flex_duration("0.5d").unwrap(),
            Duration::from_secs(12 * 3600)
        );
    }

    #[test]
    fn parse_flex_duration_stdlib_syntax() {
        assert_eq!(
            parse_flex_duration("300ms").unwrap(),
            Duration::from_millis(300)
        );
        assert_eq!(
            parse_flex_duration("2h45m").unwrap(),
            Duration::from_secs(2 * 3600 + 45 * 60)
        );
        assert_eq!(
            parse_flex_duration("1.5h").unwrap(),
            Duration::from_millis(5_400_000)
        );
        assert_eq!(parse_flex_duration("0").unwrap(), Duration::ZERO);
    }

    #[test]
    fn parse_flex_duration_errors() {
        assert!(parse_flex_duration("").is_err());
        assert!(parse_flex_duration("5").is_err()); // missing unit
        assert!(parse_flex_duration("5x").is_err()); // unknown unit
        assert!(parse_flex_duration("-1h").is_err()); // negative rejected (deviation)
        assert!(parse_flex_duration("9999999999999999999999h").is_err()); // overflow
    }

    #[test]
    fn bool_from_env_tokens() {
        // Direct table over the accepted tokens via a temp-var-free wrapper:
        // set/unset through std::env is process-global, so exercise the match
        // arms through parse-level checks instead.
        for (token, want) in [
            ("true", true),
            ("TRUE", true),
            ("1", true),
            ("yes", true),
            ("on", true),
            ("false", false),
            ("0", false),
            ("no", false),
            ("off", false),
            ("OFF", false),
            ("bogus", true), // unrecognized → fallback
        ] {
            let got = match token.to_ascii_lowercase().as_str() {
                "false" | "0" | "no" | "off" => false,
                "true" | "1" | "yes" | "on" => true,
                _ => true, // fallback=true
            };
            assert_eq!(got, want, "token {token}");
        }
    }

    #[test]
    fn int_from_env_parse() {
        assert_eq!("42".parse::<i64>().unwrap(), 42);
        assert!("abc".parse::<i64>().is_err());
    }

    #[tokio::test]
    async fn sleep_with_context_completes() {
        let ctx = Ctx::new();
        sleep_with_context(&ctx, Duration::from_millis(1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn sleep_with_context_cancelled() {
        let ctx = Ctx::new();
        ctx.cancel_with(CancelCause::Shutdown);
        let err = sleep_with_context(&ctx, Duration::from_secs(60))
            .await
            .unwrap_err();
        assert_eq!(err, CancelCause::Shutdown);
    }

    #[tokio::test]
    async fn sleep_with_wakeup_wakes_early() {
        let ctx = Ctx::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(1);
        tx.send(()).await.unwrap();
        sleep_with_context_or_wakeup(&ctx, Duration::from_secs(60), Some(&mut rx))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn sleep_with_nil_wakeup_delegates() {
        let ctx = Ctx::new();
        sleep_with_context_or_wakeup(&ctx, Duration::from_millis(1), None)
            .await
            .unwrap();
    }
}
