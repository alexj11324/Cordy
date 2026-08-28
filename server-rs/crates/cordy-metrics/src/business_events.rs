//! PR3 funnel / community / commercial counters paired with PostHog events —
//!
//! Deferred until the analytics package lands: `RecordEvent` / `IncForEvent`
//! (the pairing bridge dispatching `analytics.Event` names to these counters)
//! plus their property accessors. Every typed `Record*` helper below is
//! analytics-free and ports completely.

use prometheus::{CounterVec, Histogram, HistogramOpts, HistogramVec, Opts};

use cordy_analytics as analytics;
use cordy_analytics::{is_metrics_only, AnalyticsClient, Event};

use crate::business::BusinessMetrics;
use crate::labels::{
    metric_labels, normalize_failure_reason, normalize_runtime_mode, normalize_runtime_provider,
    normalize_task_source,
};
use crate::labels_pr3::{
    normalize_autopilot_cadence, normalize_autopilot_skip_reason, normalize_autopilot_trigger,
    normalize_chat_output_local_path_kind, normalize_cloud_runtime_op,
    normalize_cloud_runtime_status, normalize_contact_sales_source, normalize_daemon_ws_kind,
    normalize_email_rate_limit_action, normalize_email_rate_limit_gate, normalize_feedback_kind,
    normalize_github_action, normalize_github_event_kind, normalize_github_pr_review_result,
    normalize_onboarding_path, normalize_platform, normalize_signup_source,
    normalize_webhook_delivery_status, normalize_webhook_provider,
    normalize_webhook_rate_limit_gate,
};

/// Covers cold-start runtime readiness from <1s to ~5min. Most provider boots
/// land in 5–60s; the long tail catches stuck pulls.
const RUNTIME_READY_BUCKETS: &[f64] = &[1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0];

/// Covers outbound Fleet/Gateway calls from sub-100ms (status pings) to ~30s
/// (provision). Aligns with cloudruntime.defaultTimeout.
const CLOUD_RUNTIME_REQUEST_BUCKETS: &[f64] =
    &[0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 30.0];

/// Covers PR-open → PR-merged latency from minutes to weeks.
const PR_MERGE_SECONDS_BUCKETS: &[f64] = &[
    300.0,
    900.0,
    1800.0,
    3600.0,
    2.0 * 3600.0,
    6.0 * 3600.0,
    12.0 * 3600.0,
    24.0 * 3600.0,
    2.0 * 24.0 * 3600.0,
    7.0 * 24.0 * 3600.0,
    30.0 * 24.0 * 3600.0,
];

pub(crate) struct BusinessEventMetrics {
    signup: CounterVec,
    workspace_created: CounterVec,
    team_invite_sent: CounterVec,
    team_invite_accepted: CounterVec,
    onboarding_started: CounterVec,
    onboarding_questionnaire_submit: CounterVec,
    onboarding_source_submit: CounterVec,
    onboarding_completed: CounterVec,
    cloud_waitlist_joined: CounterVec,
    issue_created: CounterVec,
    chat_message_sent: CounterVec,
    agent_created: CounterVec,
    squad_created: CounterVec,
    autopilot_created: CounterVec,
    issue_executed: CounterVec,
    runtime_registered: CounterVec,
    runtime_ready: CounterVec,
    runtime_ready_seconds: HistogramVec,
    runtime_failed: CounterVec,
    runtime_offline: CounterVec,
    daemon_ws_message_received: CounterVec,
    autopilot_run_started: CounterVec,
    autopilot_run_terminal: CounterVec,
    autopilot_run_skipped: CounterVec,
    webhook_delivery: CounterVec,
    webhook_rate_limited: CounterVec,
    email_rate_limited: CounterVec,
    github_event_received: CounterVec,
    github_pr_review: CounterVec,
    github_pr_merge_seconds: Histogram,
    cloud_runtime_request: CounterVec,
    cloud_runtime_request_duration_secs: HistogramVec,
    feedback_submitted: CounterVec,
    contact_sales_submitted: CounterVec,
    chat_output_local_path: CounterVec,
}

fn cvec(name: &'static str, help: &'static str) -> CounterVec {
    CounterVec::new(Opts::new(name, help), metric_labels(name)).expect("valid counter vec")
}

impl BusinessEventMetrics {
    pub(crate) fn new() -> Self {
        Self {
            signup: cvec("cordy_signup_total", "Total user signups (account creations)."),
            workspace_created: cvec("cordy_workspace_created_total", "Total workspaces created."),
            team_invite_sent: cvec("cordy_team_invite_sent_total", "Total workspace invitations sent."),
            team_invite_accepted: cvec(
                "cordy_team_invite_accepted_total",
                "Total workspace invitations accepted.",
            ),
            onboarding_started: cvec("cordy_onboarding_started_total", "Total onboarding flows started."),
            onboarding_questionnaire_submit: cvec(
                "cordy_onboarding_questionnaire_submitted_total",
                "Total onboarding questionnaires submitted.",
            ),
            onboarding_source_submit: cvec(
                "cordy_onboarding_source_submitted_total",
                "Total acquisition-source answers or declines recorded (workspace backfill prompt).",
            ),
            onboarding_completed: cvec("cordy_onboarding_completed_total", "Total onboarding flows completed."),
            cloud_waitlist_joined: cvec(
                "cordy_cloud_waitlist_joined_total",
                "Total users that joined the cloud waitlist.",
            ),
            issue_created: cvec("cordy_issue_created_total", "Total issues created (any source)."),
            chat_message_sent: cvec(
                "cordy_chat_message_sent_total",
                "Total user chat messages sent (excludes agent replies).",
            ),
            agent_created: cvec("cordy_agent_created_total", "Total agents created."),
            squad_created: cvec("cordy_squad_created_total", "Total squads created."),
            autopilot_created: cvec("cordy_autopilot_created_total", "Total autopilots created."),
            issue_executed: cvec(
                "cordy_issue_executed_total",
                "First task completion per issue (per-issue exactly-once activation keystone).",
            ),
            runtime_registered: cvec(
                "cordy_runtime_registered_total",
                "Total first-time runtime registrations.",
            ),
            runtime_ready: cvec("cordy_runtime_ready_total", "Total runtimes that reached ready state."),
            runtime_ready_seconds: HistogramVec::new(
                HistogramOpts::new(
                    "cordy_runtime_ready_seconds",
                    "Time from runtime registration to ready (seconds).",
                )
                .buckets(RUNTIME_READY_BUCKETS.to_vec()),
                metric_labels("cordy_runtime_ready_seconds"),
            )
            .expect("valid histogram vec"),
            runtime_failed: cvec(
                "cordy_runtime_failed_total",
                "Total runtime failures by canonical reason.",
            ),
            runtime_offline: cvec("cordy_runtime_offline_total", "Total runtime offline transitions."),
            daemon_ws_message_received: cvec(
                "cordy_daemon_ws_message_received_total",
                "Total daemon WebSocket inbound messages by handler kind.",
            ),
            autopilot_run_started: cvec("cordy_autopilot_run_started_total", "Total autopilot runs started."),
            autopilot_run_terminal: cvec(
                "cordy_autopilot_run_terminal_total",
                "Total autopilot runs that reached a terminal status.",
            ),
            autopilot_run_skipped: cvec(
                "cordy_autopilot_run_skipped_total",
                "Total autopilot runs that admission-skipped (concurrency / cooldown / other).",
            ),
            webhook_delivery: cvec(
                "cordy_webhook_delivery_total",
                "Total inbound webhook deliveries by provider and outcome.",
            ),
            webhook_rate_limited: cvec(
                "cordy_webhook_rate_limited_total",
                "Total webhook admissions or worker dispatches delayed by a bounded safety gate.",
            ),
            email_rate_limited: cvec(
                "cordy_email_rate_limited_total",
                "Total email-producing actions rejected by a bounded safety gate.",
            ),
            github_event_received: cvec(
                "cordy_github_event_received_total",
                "Total GitHub webhook events received by event kind and action.",
            ),
            github_pr_review: cvec(
                "cordy_github_pr_review_total",
                "Total GitHub pull request reviews observed by result.",
            ),
            github_pr_merge_seconds: Histogram::with_opts(
                HistogramOpts::new(
                    "cordy_github_pr_merge_seconds",
                    "Time from PR opened to merged (seconds).",
                )
                .buckets(PR_MERGE_SECONDS_BUCKETS.to_vec()),
            )
            .expect("valid histogram"),
            cloud_runtime_request: cvec(
                "cordy_cloudruntime_request_total",
                "Total outbound cloud runtime requests by op and status bucket.",
            ),
            cloud_runtime_request_duration_secs: HistogramVec::new(
                HistogramOpts::new(
                    "cordy_cloudruntime_request_duration_seconds",
                    "Outbound cloud runtime request duration (seconds).",
                )
                .buckets(CLOUD_RUNTIME_REQUEST_BUCKETS.to_vec()),
                metric_labels("cordy_cloudruntime_request_duration_seconds"),
            )
            .expect("valid histogram vec"),
            feedback_submitted: cvec("cordy_feedback_submitted_total", "Total in-app feedback submissions."),
            contact_sales_submitted: cvec(
                "cordy_contact_sales_submitted_total",
                "Total contact-sales inquiries submitted.",
            ),
            chat_output_local_path: cvec(
                "cordy_chat_output_local_path_total",
                "Total agent chat replies that referenced a runtime-local path, by evidence kind. Observation only — the reply is still delivered.",
            ),
        }
    }

    pub(crate) fn collectors(&self) -> Vec<Box<dyn prometheus::core::Collector>> {
        vec![
            Box::new(self.signup.clone()),
            Box::new(self.workspace_created.clone()),
            Box::new(self.team_invite_sent.clone()),
            Box::new(self.team_invite_accepted.clone()),
            Box::new(self.onboarding_started.clone()),
            Box::new(self.onboarding_questionnaire_submit.clone()),
            Box::new(self.onboarding_source_submit.clone()),
            Box::new(self.onboarding_completed.clone()),
            Box::new(self.cloud_waitlist_joined.clone()),
            Box::new(self.issue_created.clone()),
            Box::new(self.chat_message_sent.clone()),
            Box::new(self.agent_created.clone()),
            Box::new(self.squad_created.clone()),
            Box::new(self.autopilot_created.clone()),
            Box::new(self.issue_executed.clone()),
            Box::new(self.runtime_registered.clone()),
            Box::new(self.runtime_ready.clone()),
            Box::new(self.runtime_ready_seconds.clone()),
            Box::new(self.runtime_failed.clone()),
            Box::new(self.runtime_offline.clone()),
            Box::new(self.daemon_ws_message_received.clone()),
            Box::new(self.autopilot_run_started.clone()),
            Box::new(self.autopilot_run_terminal.clone()),
            Box::new(self.autopilot_run_skipped.clone()),
            Box::new(self.webhook_delivery.clone()),
            Box::new(self.webhook_rate_limited.clone()),
            Box::new(self.email_rate_limited.clone()),
            Box::new(self.github_event_received.clone()),
            Box::new(self.github_pr_review.clone()),
            Box::new(self.github_pr_merge_seconds.clone()),
            Box::new(self.cloud_runtime_request.clone()),
            Box::new(self.cloud_runtime_request_duration_secs.clone()),
            Box::new(self.feedback_submitted.clone()),
            Box::new(self.contact_sales_submitted.clone()),
            Box::new(self.chat_output_local_path.clone()),
        ]
    }
}

// ---- non-PostHog Record* helpers (typed; no analytics.Event source) -------

impl BusinessMetrics {
    /// Counts an autopilot admission-skip with reason.
    pub fn record_autopilot_run_skipped(&self, cadence: &str, reason: &str) {
        self.events
            .autopilot_run_skipped
            .with_label_values(&[
                &normalize_autopilot_cadence(cadence),
                &normalize_autopilot_skip_reason(reason),
            ])
            .inc();
    }

    /// Counts an inbound webhook outcome.
    pub fn record_webhook_delivery(&self, provider: &str, status: &str) {
        self.events
            .webhook_delivery
            .with_label_values(&[
                &normalize_webhook_provider(provider),
                &normalize_webhook_delivery_status(status),
            ])
            .inc();
    }

    pub fn record_webhook_rate_limited(&self, gate: &str) {
        self.events
            .webhook_rate_limited
            .with_label_values(&[&normalize_webhook_rate_limit_gate(gate)])
            .inc();
    }

    pub fn record_email_rate_limited(&self, action: &str, gate: &str) {
        self.events
            .email_rate_limited
            .with_label_values(&[
                &normalize_email_rate_limit_action(action),
                &normalize_email_rate_limit_gate(gate),
            ])
            .inc();
    }

    /// Counts a GitHub webhook event by event kind / action.
    pub fn record_github_event_received(&self, event_kind: &str, action: &str) {
        self.events
            .github_event_received
            .with_label_values(&[
                &normalize_github_event_kind(event_kind),
                &normalize_github_action(action),
            ])
            .inc();
    }

    /// Counts a PR review observation by result.
    pub fn record_github_pr_review(&self, result: &str) {
        self.events
            .github_pr_review
            .with_label_values(&[&normalize_github_pr_review_result(result)])
            .inc();
    }

    /// Records open→merge latency in seconds. Negative or zero values ignored.
    pub fn observe_github_pr_merge_seconds(&self, seconds: f64) {
        if seconds <= 0.0 {
            return;
        }
        self.events.github_pr_merge_seconds.observe(seconds);
    }

    /// Counts an outbound Fleet/Gateway call by op + status bucket and
    /// observes its duration.
    pub fn record_cloud_runtime_request(&self, op: &str, status: &str, duration_seconds: f64) {
        let op = normalize_cloud_runtime_op(op);
        let status = normalize_cloud_runtime_status(status);
        self.events
            .cloud_runtime_request
            .with_label_values(&[&op, &status])
            .inc();
        if duration_seconds >= 0.0 {
            self.events
                .cloud_runtime_request_duration_secs
                .with_label_values(&[&op])
                .observe(duration_seconds);
        }
    }

    /// Counts a chat reply that referenced a runtime-local path, by evidence
    /// kind ("file_url" / "workdir_path").
    ///
    /// Observation only: the reply is delivered either way. The server cannot
    /// judge these paths the way the CLI lint can — it has no access to the
    /// daemon's filesystem to stat them — so this measures whether the MUL-4899
    /// prompt contract is landing, and must never gate delivery on a lexical
    /// guess. The label is a closed enum precisely so no fragment of the path
    /// or reply body can reach Prometheus.
    pub fn record_chat_output_local_path(&self, kind: &str) {
        self.events
            .chat_output_local_path
            .with_label_values(&[&normalize_chat_output_local_path_kind(kind)])
            .inc();
    }

    /// Counts an inbound daemon WS message by handler kind.
    pub fn record_daemon_ws_message_received(&self, kind: &str) {
        self.events
            .daemon_ws_message_received
            .with_label_values(&[&normalize_daemon_ws_kind(kind)])
            .inc();
    }
}

// ---- PostHog↔Prometheus pairing bridge -------------------------------------

type Props = serde_json::Map<String, serde_json::Value>;

fn string_prop(props: Option<&Props>, key: &str) -> String {
    props
        .and_then(|p| p.get(key))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn int64_prop(props: Option<&Props>, key: &str) -> i64 {
    props
        .and_then(|p| p.get(key))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
}

fn bool_prop(props: Option<&Props>, key: &str) -> bool {
    props
        .and_then(|p| p.get(key))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn bool_label(v: bool) -> &'static str {
    if v {
        "true"
    } else {
        "false"
    }
}

/// Increments the matching Prometheus counter and, for any event that still
/// ships to PostHog, enqueues the PostHog event too — so the two cannot drift.
/// Both sides are best-effort and never block the request path.
///
/// As of MUL-4127 every server-side event is flagged metrics-only, so capture
/// is skipped for all of them; the path is retained so a future
/// non-metrics-only event name would still ship.
pub fn record_event(client: Option<&dyn AnalyticsClient>, m: Option<&BusinessMetrics>, ev: &Event) {
    if let Some(client) = client {
        if !is_metrics_only(&ev.name) {
            client.capture(ev.clone());
        }
    }
    if let Some(m) = m {
        m.inc_for_event(ev);
    }
}

impl BusinessMetrics {
    /// Dispatches an analytics.Event to the matching Prometheus counter.
    /// Unknown event names are silently ignored — missing dispatch entries are
    /// a pairing-test concern, not a runtime error.
    pub fn inc_for_event(&self, ev: &Event) {
        let props = ev.properties.as_ref();
        match ev.name.as_str() {
            analytics::EVENT_SIGNUP => self
                .events
                .signup
                .with_label_values(&[&normalize_signup_source(&string_prop(
                    props,
                    "signup_source",
                ))])
                .inc(),
            analytics::EVENT_WORKSPACE_CREATED => self
                .events
                .workspace_created
                .with_label_values(&[&normalize_task_source(&string_prop(props, "source"))])
                .inc(),
            analytics::EVENT_TEAM_INVITE_SENT => self
                .events
                .team_invite_sent
                .with_label_values(&[] as &[&str])
                .inc(),
            analytics::EVENT_TEAM_INVITE_ACCEPTED => self
                .events
                .team_invite_accepted
                .with_label_values(&[] as &[&str])
                .inc(),
            analytics::EVENT_ONBOARDING_STARTED => self
                .events
                .onboarding_started
                .with_label_values(&[&normalize_platform(&string_prop(props, "platform"))])
                .inc(),
            analytics::EVENT_ONBOARDING_QUESTIONNAIRE_SUBMIT => self
                .events
                .onboarding_questionnaire_submit
                .with_label_values(&[] as &[&str])
                .inc(),
            analytics::EVENT_ONBOARDING_SOURCE_SUBMIT => self
                .events
                .onboarding_source_submit
                .with_label_values(&[] as &[&str])
                .inc(),
            analytics::EVENT_ONBOARDING_COMPLETED => self
                .events
                .onboarding_completed
                .with_label_values(&[&normalize_onboarding_path(&string_prop(
                    props,
                    "completion_path",
                ))])
                .inc(),
            analytics::EVENT_CLOUD_WAITLIST_JOINED => self
                .events
                .cloud_waitlist_joined
                .with_label_values(&[] as &[&str])
                .inc(),
            analytics::EVENT_ISSUE_CREATED => self
                .events
                .issue_created
                .with_label_values(&[
                    &normalize_task_source(&string_prop(props, "source")),
                    &normalize_platform(&string_prop(props, "platform")),
                ])
                .inc(),
            analytics::EVENT_CHAT_MESSAGE_SENT => self
                .events
                .chat_message_sent
                .with_label_values(&[&normalize_platform(&string_prop(props, "platform"))])
                .inc(),
            analytics::EVENT_AGENT_CREATED => self
                .events
                .agent_created
                .with_label_values(&[
                    &normalize_runtime_mode(&string_prop(props, "runtime_mode")),
                    &normalize_task_source(&string_prop(props, "source")),
                ])
                .inc(),
            analytics::EVENT_SQUAD_CREATED => self
                .events
                .squad_created
                .with_label_values(&[] as &[&str])
                .inc(),
            analytics::EVENT_AUTOPILOT_CREATED => self
                .events
                .autopilot_created
                .with_label_values(&[&normalize_autopilot_cadence(&string_prop(props, "cadence"))])
                .inc(),
            analytics::EVENT_ISSUE_EXECUTED => self
                .events
                .issue_executed
                .with_label_values(&[&normalize_task_source(&string_prop(props, "source"))])
                .inc(),
            analytics::EVENT_RUNTIME_REGISTERED => self
                .events
                .runtime_registered
                .with_label_values(&[
                    &normalize_runtime_mode(&string_prop(props, "runtime_mode")),
                    &normalize_runtime_provider(&string_prop(props, "provider")),
                ])
                .inc(),
            analytics::EVENT_RUNTIME_READY => {
                let runtime_mode = normalize_runtime_mode(&string_prop(props, "runtime_mode"));
                let provider = normalize_runtime_provider(&string_prop(props, "provider"));
                self.events
                    .runtime_ready
                    .with_label_values(&[&runtime_mode, &provider])
                    .inc();
                let d = int64_prop(props, "ready_duration_ms");
                if d > 0 {
                    self.events
                        .runtime_ready_seconds
                        .with_label_values(&[&runtime_mode, &provider])
                        .observe(d as f64 / 1000.0);
                }
            }
            analytics::EVENT_RUNTIME_FAILED => self
                .events
                .runtime_failed
                .with_label_values(&[
                    &normalize_runtime_mode(&string_prop(props, "runtime_mode")),
                    &normalize_runtime_provider(&string_prop(props, "provider")),
                    &normalize_failure_reason(&string_prop(props, "failure_reason")),
                    bool_label(bool_prop(props, "recoverable")),
                ])
                .inc(),
            analytics::EVENT_RUNTIME_OFFLINE => self
                .events
                .runtime_offline
                .with_label_values(&[
                    &normalize_runtime_mode(&string_prop(props, "runtime_mode")),
                    &normalize_runtime_provider(&string_prop(props, "provider")),
                ])
                .inc(),
            analytics::EVENT_AUTOPILOT_RUN_STARTED => self
                .events
                .autopilot_run_started
                .with_label_values(&[
                    &normalize_autopilot_cadence(&string_prop(props, "cadence")),
                    &normalize_autopilot_trigger(&string_prop(props, "trigger_kind")),
                ])
                .inc(),
            analytics::EVENT_AUTOPILOT_RUN_COMPLETED => self
                .events
                .autopilot_run_terminal
                .with_label_values(&[
                    &normalize_autopilot_cadence(&string_prop(props, "cadence")),
                    &normalize_autopilot_trigger(&string_prop(props, "trigger_kind")),
                    "completed",
                ])
                .inc(),
            analytics::EVENT_AUTOPILOT_RUN_FAILED => self
                .events
                .autopilot_run_terminal
                .with_label_values(&[
                    &normalize_autopilot_cadence(&string_prop(props, "cadence")),
                    &normalize_autopilot_trigger(&string_prop(props, "trigger_kind")),
                    "failed",
                ])
                .inc(),
            analytics::EVENT_FEEDBACK_SUBMITTED => self
                .events
                .feedback_submitted
                .with_label_values(&[
                    &normalize_feedback_kind(&string_prop(props, "kind")),
                    &normalize_platform(&string_prop(props, "platform")),
                ])
                .inc(),
            analytics::EVENT_CONTACT_SALES_SUBMITTED => self
                .events
                .contact_sales_submitted
                .with_label_values(&[&normalize_contact_sales_source(&string_prop(
                    props,
                    "form_source",
                ))])
                .inc(),
            // agent_task_* lifecycle telemetry is recorded straight to
            // Prometheus via the typed RecordTask* methods; anything else
            // reaching this arm is a missing case.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_helpers_normalize_and_count() {
        let m = BusinessMetrics::new();
        m.record_autopilot_run_skipped("Daily", "ALREADY_RUNNING");
        assert_eq!(
            m.events
                .autopilot_run_skipped
                .with_label_values(&["daily", "already_running"])
                .get(),
            1.0
        );

        m.record_webhook_delivery("GitHub", "dispatched");
        assert_eq!(
            m.events
                .webhook_delivery
                .with_label_values(&["github", "dispatched"])
                .get(),
            1.0
        );

        m.record_webhook_rate_limited("worker_trigger");
        assert_eq!(
            m.events
                .webhook_rate_limited
                .with_label_values(&["worker_trigger"])
                .get(),
            1.0
        );

        m.record_email_rate_limited("workspace_invitation", "recipient");
        assert_eq!(
            m.events
                .email_rate_limited
                .with_label_values(&["workspace_invitation", "recipient"])
                .get(),
            1.0
        );

        m.record_github_event_received("pull_request", "");
        assert_eq!(
            m.events
                .github_event_received
                .with_label_values(&["pull_request", "none"])
                .get(),
            1.0
        );

        m.record_github_pr_review("approved");
        assert_eq!(
            m.events
                .github_pr_review
                .with_label_values(&["approved"])
                .get(),
            1.0
        );

        m.record_daemon_ws_message_received("heartbeat");
        assert_eq!(
            m.events
                .daemon_ws_message_received
                .with_label_values(&["heartbeat"])
                .get(),
            1.0
        );
    }

    #[test]
    fn pr_merge_latency_ignores_non_positive() {
        let m = BusinessMetrics::new();
        m.observe_github_pr_merge_seconds(0.0);
        m.observe_github_pr_merge_seconds(-5.0);
        assert_eq!(m.events.github_pr_merge_seconds.get_sample_count(), 0);
        m.observe_github_pr_merge_seconds(900.0);
        assert_eq!(m.events.github_pr_merge_seconds.get_sample_count(), 1);
    }

    #[test]
    fn cloud_runtime_request_counts_and_observes() {
        let m = BusinessMetrics::new();
        m.record_cloud_runtime_request("Provision", "503", -1.0);
        assert_eq!(
            m.events
                .cloud_runtime_request
                .with_label_values(&["provision", "5xx"])
                .get(),
            1.0
        );
        // Negative duration not observed.
        assert_eq!(
            m.events
                .cloud_runtime_request_duration_secs
                .with_label_values(&["provision"])
                .get_sample_count(),
            0
        );
        m.record_cloud_runtime_request("status", "200", 0.08);
        assert_eq!(
            m.events
                .cloud_runtime_request_duration_secs
                .with_label_values(&["status"])
                .get_sample_count(),
            1
        );
    }

    #[test]
    fn chat_output_local_path_is_closed_enum() {
        let m = BusinessMetrics::new();
        m.record_chat_output_local_path("file_url");
        m.record_chat_output_local_path("/Users/someone/secret");
        assert_eq!(
            m.events
                .chat_output_local_path
                .with_label_values(&["file_url"])
                .get(),
            1.0
        );
        assert_eq!(
            m.events
                .chat_output_local_path
                .with_label_values(&["other"])
                .get(),
            1.0
        );
    }

    struct CountingClient(std::sync::atomic::AtomicU64);

    #[async_trait::async_trait]
    impl AnalyticsClient for CountingClient {
        fn capture(&self, _event: Event) {
            use std::sync::atomic::Ordering;
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        async fn close(&self) {}
    }

    #[test]
    fn record_event_pairs_counter_and_skips_metrics_only_capture() {
        let m = BusinessMetrics::new();
        let client = CountingClient(std::sync::atomic::AtomicU64::new(0));

        // Server-side events are metrics-only: counter increments, PostHog skipped.
        let ev = cordy_analytics::signup("u1", "a@b.co", "x");
        record_event(Some(&client), Some(&m), &ev);
        assert_eq!(
            m.events.signup.with_label_values(&["twitter"]).get(),
            1.0,
            "signup_source 'x' normalizes to the 'twitter' bucket"
        );
        assert_eq!(client.0.load(std::sync::atomic::Ordering::Relaxed), 0);

        // A non-metrics-only name would ship to PostHog AND hit the default arm.
        let mut ev = Event {
            name: "client_crash".into(),
            ..Default::default()
        };
        let mut props = Props::new();
        props.insert("platform".into(), serde_json::json!("web"));
        ev.properties = Some(props);
        record_event(Some(&client), Some(&m), &ev);
        assert_eq!(client.0.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[test]
    fn inc_for_event_dispatch_spot_checks() {
        let m = BusinessMetrics::new();

        let ready = cordy_analytics::runtime_ready("", "ws", "rt", "dm", "claude", 4500);
        m.inc_for_event(&ready);
        assert_eq!(
            m.events
                .runtime_ready
                .with_label_values(&["local", "claude"])
                .get(),
            1.0
        );
        assert_eq!(
            m.events
                .runtime_ready_seconds
                .with_label_values(&["local", "claude"])
                .get_sample_count(),
            1,
            "ready_duration_ms>0 observed as seconds"
        );

        let assignee = cordy_analytics::AutopilotAssignee {
            agent_id: "ag".into(),
            ..Default::default()
        };
        let done = cordy_analytics::autopilot_run_completed(
            "u", "ws", "ap", "run", "daily", &assignee, "schedule", 100,
        );
        m.inc_for_event(&done);
        assert_eq!(
            m.events
                .autopilot_run_terminal
                .with_label_values(&["daily", "schedule", "completed"])
                .get(),
            1.0
        );

        // Unknown names silently ignored.
        m.inc_for_event(&Event {
            name: "not_a_real_event".into(),
            ..Default::default()
        });
    }
}
