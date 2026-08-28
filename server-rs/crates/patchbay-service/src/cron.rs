//! Cron schedule evaluation.
//!
//! Go uses robfig/cron v3 (5-field crontab); the Rust port uses `croner`,
//! which parses the same standard 5-field syntax. robfig's five-year search
//! horizon (Next() returning the zero time for never-matching expressions)
//! is mirrored with an explicit year-based cutoff so impossible expressions
//! like "0 0 30 2 *" terminate instead of spinning.

use chrono::{DateTime, Datelike, Utc};
use chrono_tz::Tz;
use croner::Cron;

/// Parses cronExpr in the named IANA timezone and returns the parser plus the
/// loaded location.
///
/// robfig v3.0.1 reads an optional "TZ="/"CRON_TZ=" prefix up to the first
/// space and panics (parser.go:99, slice[:-1]) when that space is missing.
/// The preview endpoint feeds raw user text here, so reject the shape the
/// parser cannot survive instead of turning a typo into a 500.
fn parse_cron_schedule(cron_expr: &str, timezone: &str) -> anyhow::Result<(Cron, Tz)> {
    // Guard the shape robfig v3.0.1 cannot survive: a TZ="/"CRON_TZ=" prefix
    // with no separating space panics its own parser (parser.go:99).
    if (cron_expr.starts_with("TZ=") || cron_expr.starts_with("CRON_TZ="))
        && !cron_expr.contains(' ')
    {
        anyhow::bail!(
            "parse cron: missing schedule after timezone prefix {:?}",
            cron_expr
        );
    }

    // croner has no robfig-style TZ= prefix — strip it manually and let the
    // embedded zone override the caller's timezone argument.
    let mut effective_tz = timezone;
    let mut expr = cron_expr;
    for prefix in ["TZ=", "CRON_TZ="] {
        if let Some(rest) = expr.strip_prefix(prefix) {
            if let Some((tz_name, schedule)) = rest.split_once(' ') {
                effective_tz = tz_name;
                expr = schedule;
            }
            break;
        }
    }

    let sched: Cron = expr
        .parse()
        .map_err(|e| anyhow::anyhow!("parse cron: {e}"))?;
    let loc: Tz = effective_tz
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid timezone {:?}: {}", effective_tz, e))?;
    Ok((sched, loc))
}

/// Parses cronExpr in the named IANA timezone and returns the next activation
/// strictly after `after`. The result is always in UTC and represents the
/// canonical fire time of the next occurrence.
///
/// `after` is interpreted as an absolute instant; callers should pass DB time
/// (e.g. `SELECT now()`) rather than wall-clock now so that two app instances
/// with skewed clocks still produce the same answer.
pub fn next_occurrence_after_utc(
    cron_expr: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> anyhow::Result<DateTime<Utc>> {
    let (sched, loc) = parse_cron_schedule(cron_expr, timezone)?;
    let local_after = after.with_timezone(&loc);
    sched
        .find_next_occurrence(&local_after, false)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| anyhow::anyhow!("no next occurrence within search horizon: {e}"))
}

/// Parses cronExpr in `timezone` once and returns the next `count` activations
/// strictly after `after`, ascending, in UTC. Stops early — returning a
/// shorter slice — when the expression has no further occurrence within the
/// five-year search horizon (e.g. "0 0 30 2 *").
///
/// Unlike calling [`next_occurrence_after_utc`] in a loop, the expression is
/// parsed and the location loaded exactly once. The schedule editor's preview
/// endpoint asks for this on every debounced keystroke, so the difference is
/// not academic.
pub fn next_occurrences_after_utc(
    cron_expr: &str,
    timezone: &str,
    after: DateTime<Utc>,
    count: usize,
) -> anyhow::Result<Vec<DateTime<Utc>>> {
    let (sched, loc) = parse_cron_schedule(cron_expr, timezone)?;
    let mut out = Vec::with_capacity(count);
    let mut cursor = after.with_timezone(&loc);

    // Parity with robfig's five-year search horizon.
    let horizon_year = cursor.year() + 5;

    for _ in 0..count {
        match sched.find_next_occurrence(&cursor, false) {
            Ok(next) if next.year() <= horizon_year => {
                cursor = next;
                out.push(next.with_timezone(&Utc));
            }
            _ => break,
        }
    }
    Ok(out)
}

/// Parses cronExpr in `timezone` and returns every activation in the
/// half-open interval `(after, until]`, in canonical UTC order (ascending).
/// Used by the Autopilot schedule dispatch job to enumerate every plan_time
/// that became due between the last stored occurrence and DB now().
///
/// The slice is capped at 1024 entries — a safety net against an accidental
/// "every second" cron over a multi-day catch-up window. The scheduler
/// manager additionally caps the returned slice at JobSpec.MaxPlansPerTick.
pub fn next_occurrences_utc(
    cron_expr: &str,
    timezone: &str,
    after: DateTime<Utc>,
    until: DateTime<Utc>,
) -> anyhow::Result<Vec<DateTime<Utc>>> {
    let (sched, loc) = parse_cron_schedule(cron_expr, timezone)?;
    const HARD_CAP: usize = 1024;

    let mut out = Vec::with_capacity(8);
    let mut cursor = after.with_timezone(&loc);
    let until_local = until.with_timezone(&loc);
    let horizon_year = cursor.year() + 5;

    while out.len() < HARD_CAP {
        match sched.find_next_occurrence(&cursor, false) {
            Ok(next) if next.year() <= horizon_year => {
                if next > until_local {
                    break;
                }
                out.push(next.with_timezone(&Utc));
                cursor = next;
            }
            _ => break,
        }
    }
    Ok(out)
}

/// Evaluates the cron at the app's local now() and backs the display-only
/// autopilot_trigger.next_run_at column for the trigger create/update
/// handlers and the failure monitor. Using the local clock for this display
/// value is deliberate: app/DB clock skew under NTP is far below the column's
/// minute-level granularity, so threading DB time through these UI write
/// paths would buy no user-visible accuracy.
///
/// Scheduling decisions are a separate concern and MUST go through
/// [`next_occurrences_utc`] / [`next_occurrence_after_utc`] against DB time
/// instead: dispatch correctness across clock-skewed app instances depends
/// on it.
pub fn compute_next_run(cron_expr: &str, timezone: &str) -> anyhow::Result<DateTime<Utc>> {
    next_occurrence_after_utc(cron_expr, timezone, Utc::now())
}

/// Returns an error if the timezone string is not recognized.
pub fn validate_timezone(timezone: &str) -> anyhow::Result<()> {
    timezone
        .parse::<Tz>()
        .map_err(|e| anyhow::anyhow!("invalid timezone {:?}: {}", timezone, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    #[test]
    fn standard_five_field_expression_parses() {
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let next = next_occurrence_after_utc("30 14 * * *", "UTC", after).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 1, 1, 14, 30, 0).unwrap());
    }

    #[test]
    fn timezone_shifts_the_fire_time() {
        // 14:30 in New York (EST, UTC-5 in January) = 19:30 UTC.
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let next = next_occurrence_after_utc("30 14 * * *", "America/New_York", after).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 1, 1, 19, 30, 0).unwrap());
    }

    #[test]
    fn batch_returns_count_results_ascending() {
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let runs = next_occurrences_after_utc("0 */6 * * *", "UTC", after, 3).unwrap();
        assert_eq!(runs.len(), 3);
        // Strictly after 00:00, the next */6 fires are 06:00 / 12:00 / 18:00.
        assert_eq!(runs[0].hour(), 6);
        assert_eq!(runs[1].hour(), 12);
        assert_eq!(runs[2].hour(), 18);
        for w in runs.windows(2) {
            assert!(w[0] < w[1], "must be ascending");
        }
    }

    #[test]
    fn interval_enumeration_stops_at_until() {
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let until = Utc.with_ymd_and_hms(2026, 1, 1, 13, 0, 0).unwrap();
        let runs = next_occurrences_utc("0 */6 * * *", "UTC", after, until).unwrap();
        // Half-open (after, until]: 06:00 and 12:00 fire; 00:00 excluded,
        // 18:00 beyond.
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].hour(), 6);
        assert_eq!(runs[1].hour(), 12);
    }

    #[test]
    fn impossible_expression_terminates_within_horizon() {
        // Feb 30 never exists — robfig returns the zero time after its
        // five-year search; we must terminate via the year cutoff.
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let runs = next_occurrences_after_utc("0 0 30 2 *", "UTC", after, 10).unwrap();
        assert!(runs.is_empty());
    }

    #[test]
    fn tz_prefix_without_space_is_rejected() {
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        assert!(next_occurrence_after_utc("TZ=UTC", "UTC", after).is_err());
        assert!(next_occurrence_after_utc("CRON_TZ=UTC", "UTC", after).is_err());
        // With the separating space the prefix form is accepted.
        assert!(next_occurrence_after_utc("TZ=UTC 30 14 * * *", "UTC", after).is_ok());
    }

    #[test]
    fn invalid_timezone_is_rejected() {
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        assert!(next_occurrence_after_utc("30 14 * * *", "Mars/Olympus", after).is_err());
        assert!(validate_timezone("Not/AZone").is_err());
        assert!(validate_timezone("America/New_York").is_ok());
    }
}
