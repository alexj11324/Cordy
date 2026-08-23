//! Sustained-failure Autopilot monitor — port of
//! `server/cmd/server/autopilot_failure_monitor.go`.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use cordy_db::models::{Autopilot, InboxItem};
use cordy_db::queries::autopilot::SelectAutopilotsExceedingFailureThresholdRow as Candidate;
use cordy_db::queries::{agent, autopilot, inbox, member};
use cordy_events::{Bus, Event};
use serde_json::{json, Value};
use sqlx::PgPool;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::autopilot::record_autopilot_rule_version;

const DEFAULT_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const DEFAULT_LOOKBACK: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const DEFAULT_STARTUP_DELAY: Duration = Duration::from_secs(60);
const DEFAULT_MIN_RUNS: i64 = 50;
const DEFAULT_FAIL_RATIO: f64 = 0.9;
const MAX_DB_ATTEMPTS: usize = 3;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(100);
const GO_MAX_DURATION_SECONDS: f64 = i64::MAX as f64 / 1_000_000_000.0;
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq)]
pub struct FailureMonitorConfig {
    /// `None` is the explicit Go `interval <= 0` disabled state.
    pub interval: Option<Duration>,
    pub lookback: Duration,
    pub min_runs: i64,
    pub fail_ratio: f64,
    pub startup_delay: Duration,
}

impl Default for FailureMonitorConfig {
    fn default() -> Self {
        Self {
            interval: Some(DEFAULT_INTERVAL),
            lookback: DEFAULT_LOOKBACK,
            min_runs: DEFAULT_MIN_RUNS,
            fail_ratio: DEFAULT_FAIL_RATIO,
            startup_delay: DEFAULT_STARTUP_DELAY,
        }
    }
}

impl FailureMonitorConfig {
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Some(raw) = env_value("AUTOPILOT_FAIL_MONITOR_INTERVAL") {
            config.interval = match parse_go_duration(&raw) {
                Some(ParsedDuration::Positive(value)) => Some(value),
                Some(ParsedDuration::NonPositive) => None,
                None => {
                    warn_invalid("AUTOPILOT_FAIL_MONITOR_INTERVAL", &raw);
                    config.interval
                }
            };
        }
        if let Some(raw) = env_value("AUTOPILOT_FAIL_MONITOR_LOOKBACK") {
            match parse_go_duration(&raw) {
                Some(ParsedDuration::Positive(value)) => config.lookback = value,
                _ => warn_invalid("AUTOPILOT_FAIL_MONITOR_LOOKBACK", &raw),
            }
        }
        if let Some(raw) = env_value("AUTOPILOT_FAIL_MONITOR_STARTUP_DELAY") {
            match parse_go_duration(&raw) {
                Some(ParsedDuration::Positive(value)) => config.startup_delay = value,
                Some(ParsedDuration::NonPositive) if raw.trim() == "0" => {
                    config.startup_delay = Duration::ZERO;
                }
                _ => warn_invalid("AUTOPILOT_FAIL_MONITOR_STARTUP_DELAY", &raw),
            }
        }
        if let Some(raw) = env_value("AUTOPILOT_FAIL_MONITOR_MIN_RUNS") {
            match raw.trim().parse::<i64>().ok().filter(|value| *value > 0) {
                Some(value) => config.min_runs = value,
                None => warn_invalid("AUTOPILOT_FAIL_MONITOR_MIN_RUNS", &raw),
            }
        }
        if let Some(raw) = env_value("AUTOPILOT_FAIL_MONITOR_FAIL_RATIO") {
            match raw
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| *value > 0.0 && *value <= 1.0)
            {
                Some(value) => config.fail_ratio = value,
                None => warn_invalid("AUTOPILOT_FAIL_MONITOR_FAIL_RATIO", &raw),
            }
        }
        config
    }
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn warn_invalid(name: &str, value: &str) {
    tracing::warn!(
        name,
        value,
        "invalid failure monitor environment value; using default"
    );
}

enum ParsedDuration {
    Positive(Duration),
    NonPositive,
}

/// Parses the Go duration units used by deployment configuration. A negative
/// value is only meaningful for the interval disable switch.
fn parse_go_duration(raw: &str) -> Option<ParsedDuration> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw == "0" {
        return Some(ParsedDuration::NonPositive);
    }
    let (negative, raw) = raw
        .strip_prefix('-')
        .map_or((false, raw), |value| (true, value));
    if raw.is_empty() {
        return None;
    }
    let bytes = raw.as_bytes();
    let mut cursor = 0;
    let mut seconds = 0.0_f64;
    while cursor < bytes.len() {
        let start = cursor;
        while cursor < bytes.len() && (bytes[cursor].is_ascii_digit() || bytes[cursor] == b'.') {
            cursor += 1;
        }
        if cursor == start {
            return None;
        }
        let value = raw[start..cursor].parse::<f64>().ok()?;
        let (unit, multiplier) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ]
        .into_iter()
        .find(|(unit, _)| raw[cursor..].starts_with(unit))?;
        cursor += unit.len();
        seconds += value * multiplier;
    }
    if !seconds.is_finite() || seconds <= 0.0 || seconds > GO_MAX_DURATION_SECONDS {
        return None;
    }
    if negative {
        Some(ParsedDuration::NonPositive)
    } else {
        Some(ParsedDuration::Positive(Duration::from_secs_f64(seconds)))
    }
}

pub trait FailureMonitorMetrics: Send + Sync {
    fn record(&self, stage: &'static str, outcome: &'static str);
}

impl FailureMonitorMetrics for cordy_metrics::BusinessMetrics {
    fn record(&self, stage: &'static str, outcome: &'static str) {
        self.record_autopilot_failure_monitor(stage, outcome);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Retryable,
    Permanent,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorFailure {
    pub stage: &'static str,
    pub class: FailureClass,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessOutcome {
    Paused { side_effect_failures: usize },
    AlreadyInactive,
    Failed(MonitorFailure),
    Cancelled,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub candidates: usize,
    pub paused: usize,
    pub already_inactive: usize,
    pub retryable_failures: usize,
    pub permanent_failures: usize,
    pub side_effect_failures: usize,
    pub cancelled: bool,
}

#[derive(Clone)]
pub struct AutopilotFailureMonitor {
    pool: PgPool,
    bus: Arc<Bus>,
    metrics: Option<Arc<dyn FailureMonitorMetrics>>,
    pub config: FailureMonitorConfig,
}

impl AutopilotFailureMonitor {
    pub fn new(
        pool: PgPool,
        bus: Arc<Bus>,
        metrics: Option<Arc<dyn FailureMonitorMetrics>>,
        config: FailureMonitorConfig,
    ) -> Self {
        Self {
            pool,
            bus,
            metrics,
            config,
        }
    }

    /// Starts an owned worker. Disabled configuration creates no Tokio task.
    pub fn start(self, cancel: CancellationToken) -> FailureMonitorRuntime {
        let Some(interval) = self.config.interval else {
            tracing::info!("autopilot failure monitor: disabled (interval <= 0)");
            return FailureMonitorRuntime::disabled(self.metrics);
        };
        let runtime_metrics = self.metrics.clone();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { self.run(task_cancel, interval).await });
        FailureMonitorRuntime {
            cancel,
            task: Some(task),
            metrics: runtime_metrics,
        }
    }

    async fn run(self, cancel: CancellationToken, interval: Duration) {
        tracing::info!(
            ?interval,
            lookback = ?self.config.lookback,
            min_runs = self.config.min_runs,
            fail_ratio = self.config.fail_ratio,
            "autopilot failure monitor: starting"
        );
        if !sleep_or_cancel(&cancel, self.config.startup_delay).await {
            return;
        }
        let _ = self.run_once(&cancel).await;
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    return;
                }
                _ = ticker.tick() => {
                    let _ = self.run_once(&cancel).await;
                }
            }
        }
    }

    /// Performs one complete sweep and returns bounded outcome counts for
    /// deterministic integration tests and operational probes.
    pub async fn run_once(&self, cancel: &CancellationToken) -> SweepReport {
        let mut report = SweepReport::default();
        let since = Utc::now()
            - chrono::Duration::from_std(self.config.lookback)
                .expect("Go-compatible duration always fits chrono");
        let candidates = match self
            .retry_db(cancel, "sweep", || {
                autopilot::select_autopilots_exceeding_failure_threshold(
                    &self.pool,
                    self.config.min_runs,
                    self.config.fail_ratio,
                    Some(since),
                )
            })
            .await
        {
            Ok(candidates) => candidates,
            Err(failure) => {
                report_failure(&mut report, &failure);
                report.cancelled = failure.class == FailureClass::Cancelled;
                return report;
            }
        };
        report.candidates = candidates.len();
        if !candidates.is_empty() {
            tracing::info!(
                count = candidates.len(),
                "autopilot failure monitor: candidates"
            );
        }
        for candidate in &candidates {
            match self.process_next(cancel, candidate).await {
                ProcessOutcome::Paused {
                    side_effect_failures,
                } => {
                    report.paused += 1;
                    report.side_effect_failures += side_effect_failures;
                }
                ProcessOutcome::AlreadyInactive => report.already_inactive += 1,
                ProcessOutcome::Failed(failure) => report_failure(&mut report, &failure),
                ProcessOutcome::Cancelled => {
                    report.cancelled = true;
                    break;
                }
            }
        }
        self.record(
            "sweep",
            if report.cancelled {
                "cancelled"
            } else {
                "success"
            },
        );
        report
    }

    /// Processes one ordered candidate. The workspace/status predicate makes
    /// the state transition the idempotency key: losers emit no side effects.
    pub async fn process_next(
        &self,
        cancel: &CancellationToken,
        candidate: &Candidate,
    ) -> ProcessOutcome {
        let (Some(id), Some(workspace_id)) = (candidate.id, candidate.workspace_id) else {
            let failure = MonitorFailure {
                stage: "candidate",
                class: FailureClass::Permanent,
                message: "candidate is missing id or workspace_id".into(),
            };
            self.record_failure(&failure);
            return ProcessOutcome::Failed(failure);
        };
        let paused = match self
            .retry_db(cancel, "pause", || {
                autopilot::system_pause_autopilot_in_workspace(&self.pool, id, workspace_id)
            })
            .await
        {
            Ok(Some(paused)) => paused,
            Ok(None) => {
                self.record("pause", "already_inactive");
                return ProcessOutcome::AlreadyInactive;
            }
            Err(failure) if failure.class == FailureClass::Cancelled => {
                return ProcessOutcome::Cancelled;
            }
            Err(failure) => return ProcessOutcome::Failed(failure),
        };

        let mut side_effect_failures = 0;
        if let Err(error) = record_autopilot_rule_version(&self.pool, &paused, "system", None).await
        {
            side_effect_failures += 1;
            self.record_classified_error("rule_version", &error);
            tracing::warn!(%error, autopilot_id = %paused.id, "autopilot failure monitor: record rule version failed");
        } else {
            self.record("rule_version", "success");
        }

        let fail_pct = failure_percent(candidate.failed_runs, candidate.total_runs);
        tracing::info!(
            autopilot_id = %paused.id,
            workspace_id = %paused.workspace_id,
            title = paused.title,
            failed_runs = candidate.failed_runs,
            total_runs = candidate.total_runs,
            fail_pct,
            "autopilot failure monitor: paused autopilot"
        );

        match self.resolve_recipient(cancel, &paused).await {
            Ok(Some(recipient)) => {
                if let Err(error) = self
                    .create_and_publish_inbox(&paused, candidate, fail_pct, recipient)
                    .await
                {
                    side_effect_failures += 1;
                    self.record_classified_error("inbox", &error);
                    tracing::warn!(%error, autopilot_id = %paused.id, recipient_id = %recipient.id,
                        "autopilot failure monitor: inbox write failed");
                } else {
                    self.record("inbox", "success");
                }
            }
            Ok(None) => self.record("recipient", "no_recipient"),
            Err(failure) => {
                side_effect_failures += 1;
                tracing::debug!(error = %failure.message, autopilot_id = %paused.id,
                    "autopilot failure monitor: recipient resolution failed");
            }
        }

        self.bus.publish(&Event {
            event_type: cordy_protocol::EVENT_AUTOPILOT_UPDATED.into(),
            workspace_id: paused.workspace_id.to_string(),
            actor_type: "system".into(),
            payload: json!({
                "autopilot": autopilot_event_payload(&paused),
                "reason": "auto_paused_high_failure_rate",
            }),
            ..Default::default()
        });
        self.record("pause", "success");
        ProcessOutcome::Paused {
            side_effect_failures,
        }
    }

    async fn resolve_recipient(
        &self,
        cancel: &CancellationToken,
        paused: &Autopilot,
    ) -> Result<Option<Recipient>, MonitorFailure> {
        if paused.created_by_type == "member" {
            return Ok(Some(Recipient {
                recipient_type: "member",
                id: paused.created_by_id,
            }));
        }
        if paused.created_by_type != "agent" {
            return Ok(None);
        }
        let creator = self
            .retry_db(cancel, "recipient", || {
                agent::get_agent_in_workspace(&self.pool, paused.created_by_id, paused.workspace_id)
            })
            .await?;
        let Some(owner_id) = creator.and_then(|value| value.owner_id) else {
            return Ok(None);
        };
        let workspace_member = self
            .retry_db(cancel, "recipient", || {
                member::get_member_by_user_and_workspace(&self.pool, owner_id, paused.workspace_id)
            })
            .await?;
        Ok(workspace_member.map(|value| Recipient {
            recipient_type: "member",
            id: value.user_id,
        }))
    }

    async fn create_and_publish_inbox(
        &self,
        paused: &Autopilot,
        candidate: &Candidate,
        fail_pct: f64,
        recipient: Recipient,
    ) -> anyhow::Result<()> {
        let title = format!("Autopilot paused: {}", paused.title);
        let body = format!(
            "Auto-paused after {} of {} runs failed ({fail_pct:.1}%) in the last {}. Investigate the failures, fix the root cause, then re-enable from the autopilot page.",
            candidate.failed_runs,
            candidate.total_runs,
            format_lookback(self.config.lookback),
        );
        let details = json!({
            "autopilot_id": paused.id.to_string(),
            "autopilot_title": paused.title,
            "failed_runs": candidate.failed_runs,
            "total_runs": candidate.total_runs,
            "fail_pct": fail_pct,
            "lookback_seconds": self.config.lookback.as_secs() as i64,
            "threshold_min_runs": self.config.min_runs,
            "threshold_fail_ratio": self.config.fail_ratio,
            "reason": "auto_paused_high_failure_rate",
        });
        let item = inbox::create_inbox_item(
            &self.pool,
            paused.workspace_id,
            recipient.recipient_type,
            recipient.id,
            "autopilot_paused",
            "attention",
            None,
            &title,
            Some(&body),
            Some("system"),
            None,
            &details,
            cordy_db::dbid::new_v7(),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("create autopilot paused inbox: no row"))?;
        self.bus.publish(&Event {
            event_type: cordy_protocol::EVENT_INBOX_NEW.into(),
            workspace_id: paused.workspace_id.to_string(),
            actor_type: "system".into(),
            payload: json!({ "item": inbox_event_payload(item) }),
            ..Default::default()
        });
        Ok(())
    }

    async fn retry_db<T, F, Fut>(
        &self,
        cancel: &CancellationToken,
        stage: &'static str,
        mut operation: F,
    ) -> Result<T, MonitorFailure>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        for attempt in 1..=MAX_DB_ATTEMPTS {
            if cancel.is_cancelled() {
                let failure = MonitorFailure {
                    stage,
                    class: FailureClass::Cancelled,
                    message: "monitor cancelled".into(),
                };
                self.record_failure(&failure);
                return Err(failure);
            }
            match operation().await {
                Ok(value) => return Ok(value),
                Err(error) => {
                    let class = classify_error(&error);
                    let failure = MonitorFailure {
                        stage,
                        class,
                        message: error.to_string(),
                    };
                    if class != FailureClass::Retryable || attempt == MAX_DB_ATTEMPTS {
                        self.record_failure(&failure);
                        return Err(failure);
                    }
                    self.record(stage, "retryable_error");
                    if !sleep_or_cancel(cancel, RETRY_BASE_DELAY * attempt as u32).await {
                        let failure = MonitorFailure {
                            stage,
                            class: FailureClass::Cancelled,
                            message: "monitor cancelled during retry backoff".into(),
                        };
                        self.record_failure(&failure);
                        return Err(failure);
                    }
                }
            }
        }
        unreachable!("bounded retry loop always returns")
    }

    fn record(&self, stage: &'static str, outcome: &'static str) {
        if let Some(metrics) = &self.metrics {
            metrics.record(stage, outcome);
        }
    }

    fn record_failure(&self, failure: &MonitorFailure) {
        let outcome = match failure.class {
            FailureClass::Retryable => "retryable_error",
            FailureClass::Permanent => "permanent_error",
            FailureClass::Cancelled => "cancelled",
        };
        self.record(failure.stage, outcome);
    }

    fn record_classified_error(&self, stage: &'static str, error: &anyhow::Error) {
        self.record(
            stage,
            match classify_error(error) {
                FailureClass::Retryable => "retryable_error",
                FailureClass::Permanent => "permanent_error",
                FailureClass::Cancelled => "cancelled",
            },
        );
    }
}

#[derive(Debug, Clone, Copy)]
struct Recipient {
    recipient_type: &'static str,
    id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    Disabled,
    Stopped,
    Panicked,
    TimedOut,
}

/// Owns both cancellation and the JoinHandle. Drop is a last-resort abort;
/// production calls `shutdown` and waits for the bounded cooperative path.
pub struct FailureMonitorRuntime {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
    metrics: Option<Arc<dyn FailureMonitorMetrics>>,
}

impl FailureMonitorRuntime {
    fn disabled(metrics: Option<Arc<dyn FailureMonitorMetrics>>) -> Self {
        Self {
            cancel: CancellationToken::new(),
            task: None,
            metrics,
        }
    }

    pub async fn shutdown(mut self, timeout: Duration) -> ShutdownOutcome {
        self.cancel.cancel();
        let Some(mut task) = self.task.take() else {
            return ShutdownOutcome::Disabled;
        };
        let outcome = match tokio::time::timeout(timeout, &mut task).await {
            Ok(Ok(())) => ShutdownOutcome::Stopped,
            Ok(Err(_)) => ShutdownOutcome::Panicked,
            Err(_) => {
                task.abort();
                let _ = task.await;
                ShutdownOutcome::TimedOut
            }
        };
        if let Some(metrics) = &self.metrics {
            metrics.record(
                "shutdown",
                match outcome {
                    ShutdownOutcome::Stopped => "success",
                    ShutdownOutcome::TimedOut => "timed_out",
                    ShutdownOutcome::Panicked => "permanent_error",
                    ShutdownOutcome::Disabled => "success",
                },
            );
        }
        outcome
    }
}

impl Drop for FailureMonitorRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn classify_error(error: &anyhow::Error) -> FailureClass {
    let Some(sqlx_error) = error.downcast_ref::<sqlx::Error>() else {
        return FailureClass::Permanent;
    };
    match sqlx_error {
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::Protocol(_)
        | sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed => FailureClass::Retryable,
        sqlx::Error::Database(database) => {
            let code = database.code();
            if code.as_deref().is_some_and(|code| {
                code.starts_with("08")
                    || matches!(
                        code,
                        "40001" | "40P01" | "53300" | "57P01" | "57P02" | "57P03"
                    )
            }) {
                FailureClass::Retryable
            } else {
                FailureClass::Permanent
            }
        }
        _ => FailureClass::Permanent,
    }
}

fn report_failure(report: &mut SweepReport, failure: &MonitorFailure) {
    match failure.class {
        FailureClass::Retryable => report.retryable_failures += 1,
        FailureClass::Permanent => report.permanent_failures += 1,
        FailureClass::Cancelled => report.cancelled = true,
    }
}

async fn sleep_or_cancel(cancel: &CancellationToken, duration: Duration) -> bool {
    if duration.is_zero() {
        return !cancel.is_cancelled();
    }
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(duration) => true,
    }
}

fn failure_percent(failed: i64, total: i64) -> f64 {
    if total <= 0 {
        100.0
    } else {
        ((failed as f64 / total as f64) * 1000.0).round() / 10.0
    }
}

fn format_lookback(duration: Duration) -> String {
    if duration.is_zero() {
        return "0s".into();
    }
    let hours = duration.as_secs() / 3600;
    if hours >= 24 && duration.as_secs() % (24 * 3600) == 0 {
        let days = hours / 24;
        return if days == 1 {
            "1 day".into()
        } else {
            format!("{days} days")
        };
    }
    if duration.as_secs() % 3600 == 0 {
        return if hours == 1 {
            "1 hour".into()
        } else {
            format!("{hours} hours")
        };
    }
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        let remainder = seconds % 3600;
        return format!("{}h{}m{}s", seconds / 3600, remainder / 60, remainder % 60);
    }
    if seconds >= 60 {
        return format!("{}m{}s", seconds / 60, seconds % 60);
    }
    format!("{duration:?}")
}

fn autopilot_event_payload(value: &Autopilot) -> Value {
    json!({
        "id": value.id.to_string(),
        "workspace_id": value.workspace_id.to_string(),
        "title": value.title,
        "description": value.description,
        "assignee_id": value.assignee_id.to_string(),
        "status": value.status,
        "execution_mode": value.execution_mode,
        "issue_title_template": value.issue_title_template,
        "created_by_type": value.created_by_type,
        "created_by_id": value.created_by_id.to_string(),
        "last_run_at": value.last_run_at.map(rfc3339_seconds),
        "created_at": rfc3339_seconds(value.created_at),
        "updated_at": rfc3339_seconds(value.updated_at),
    })
}

fn inbox_event_payload(item: InboxItem) -> Value {
    json!({
        "id": item.id.to_string(),
        "workspace_id": item.workspace_id.to_string(),
        "recipient_type": item.recipient_type,
        "recipient_id": item.recipient_id.map(|id| id.to_string()),
        "type": item.type_,
        "severity": item.severity,
        "issue_id": item.issue_id.map(|id| id.to_string()),
        "title": item.title,
        "body": item.body,
        "read": item.read,
        "archived": item.archived,
        "created_at": rfc3339_seconds(item.created_at),
        "actor_type": item.actor_type,
        "actor_id": item.actor_id.map(|id| id.to_string()),
        "details": item.details.unwrap_or_else(|| json!({})),
    })
}

fn rfc3339_seconds(value: chrono::DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_duration_and_disable_semantics() {
        assert!(matches!(
            parse_go_duration("0"),
            Some(ParsedDuration::NonPositive)
        ));
        assert!(matches!(
            parse_go_duration("-1s"),
            Some(ParsedDuration::NonPositive)
        ));
        assert!(matches!(
            parse_go_duration("1h30m"),
            Some(ParsedDuration::Positive(value)) if value == Duration::from_secs(5400)
        ));
        assert!(parse_go_duration("one day").is_none());
    }

    #[test]
    fn percentage_and_lookback_match_go_display() {
        assert_eq!(failure_percent(9, 10), 90.0);
        assert_eq!(failure_percent(2, 3), 66.7);
        assert_eq!(failure_percent(0, 0), 100.0);
        assert_eq!(format_lookback(Duration::from_secs(7 * 86400)), "7 days");
        assert_eq!(format_lookback(Duration::from_secs(3600)), "1 hour");
    }

    #[test]
    fn non_database_errors_are_permanent() {
        assert_eq!(
            classify_error(&anyhow::anyhow!("invalid candidate")),
            FailureClass::Permanent
        );
    }
}
