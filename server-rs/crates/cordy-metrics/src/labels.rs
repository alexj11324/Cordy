//! Metric label vocabulary and input normalization.
//!
//! All inputs go through fixed allow-lists so a misbehaving caller cannot
//! inflate metric cardinality; every "unknown"/"other" bucket keeps the series
//! count bounded even under enum drift.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

pub(crate) const SOURCE: &str = "source";
pub(crate) const RUNTIME_MODE: &str = "runtime_mode";
pub(crate) const PROVIDER: &str = "provider";
pub(crate) const TERMINAL_STATUS: &str = "terminal_status";
pub(crate) const FAILURE_REASON: &str = "failure_reason";
pub(crate) const TOKEN_TYPE: &str = "token_type";
pub(crate) const MODEL: &str = "model";
pub(crate) const MODEL_ALIAS: &str = "model_alias";

// PR3 labels (funnel / community / commercial).
pub(crate) const SIGNUP_SOURCE: &str = "signup_source";
pub(crate) const PLATFORM: &str = "platform";
pub(crate) const PATH: &str = "path";
pub(crate) const CADENCE: &str = "cadence";
pub(crate) const TRIGGER_KIND: &str = "trigger_kind";
pub(crate) const REASON: &str = "reason";
pub(crate) const RECOVERABLE: &str = "recoverable";
pub(crate) const KIND: &str = "kind";
pub(crate) const STATUS: &str = "status";
pub(crate) const EVENT_KIND: &str = "event_kind";
pub(crate) const ACTION: &str = "action";
pub(crate) const RESULT: &str = "result";
pub(crate) const QUERY: &str = "query";
pub(crate) const OP: &str = "op";
pub(crate) const GATE: &str = "gate";
pub(crate) const OUTCOME: &str = "outcome";

/// Label set per business metric, keyed by full metric name. Data-driven so
/// [`validate_business_metric_labels`] can guard future edits against
/// high-cardinality accidents.
pub(crate) const BUSINESS_METRIC_LABELS: &[(&str, &[&str])] = &[
    ("cordy_agent_task_enqueued_total", &[SOURCE, RUNTIME_MODE]),
    ("cordy_agent_task_dispatched_total", &[SOURCE, RUNTIME_MODE]),
    (
        "cordy_agent_task_started_total",
        &[SOURCE, RUNTIME_MODE, PROVIDER],
    ),
    (
        "cordy_agent_task_terminal_total",
        &[SOURCE, RUNTIME_MODE, TERMINAL_STATUS],
    ),
    (
        "cordy_agent_task_failed_total",
        &[SOURCE, RUNTIME_MODE, FAILURE_REASON],
    ),
    (
        "cordy_agent_task_queue_wait_seconds",
        &[SOURCE, RUNTIME_MODE],
    ),
    (
        "cordy_agent_task_run_seconds",
        &[SOURCE, RUNTIME_MODE, TERMINAL_STATUS],
    ),
    (
        "cordy_agent_task_total_seconds",
        &[SOURCE, RUNTIME_MODE, TERMINAL_STATUS],
    ),
    ("cordy_agent_task_in_progress", &[SOURCE, RUNTIME_MODE]),
    (
        "cordy_agent_task_iteration_count",
        &[SOURCE, TERMINAL_STATUS],
    ),
    (
        "cordy_llm_tokens_total",
        &[PROVIDER, MODEL, TOKEN_TYPE, RUNTIME_MODE, SOURCE],
    ),
    (
        "cordy_llm_cost_usd_total",
        &[PROVIDER, MODEL, TOKEN_TYPE, RUNTIME_MODE, SOURCE],
    ),
    (
        "cordy_llm_unpriced_tokens_total",
        &[PROVIDER, MODEL_ALIAS, TOKEN_TYPE],
    ),
    ("cordy_llm_request_total", &[PROVIDER, MODEL, RUNTIME_MODE]),
    ("cordy_task_queued_expired_total", &[SOURCE, RUNTIME_MODE]),
    ("cordy_task_lease_expired_total", &[SOURCE]),
    ("cordy_chat_claim_session_fallback_needed_total", &[]),
    ("cordy_chat_claim_session_fallback_result_total", &[RESULT]),
    ("cordy_chat_claim_resume_query_duration_seconds", &[QUERY]),
    // PR3 funnel / community / commercial.
    ("cordy_signup_total", &[SIGNUP_SOURCE]),
    ("cordy_workspace_created_total", &[SOURCE]),
    ("cordy_team_invite_sent_total", &[]),
    ("cordy_team_invite_accepted_total", &[]),
    ("cordy_onboarding_started_total", &[PLATFORM]),
    ("cordy_onboarding_questionnaire_submitted_total", &[]),
    ("cordy_onboarding_source_submitted_total", &[]),
    ("cordy_onboarding_completed_total", &[PATH]),
    ("cordy_cloud_waitlist_joined_total", &[]),
    ("cordy_issue_created_total", &[SOURCE, PLATFORM]),
    ("cordy_chat_message_sent_total", &[PLATFORM]),
    ("cordy_agent_created_total", &[RUNTIME_MODE, SOURCE]),
    ("cordy_squad_created_total", &[]),
    ("cordy_autopilot_created_total", &[CADENCE]),
    ("cordy_issue_executed_total", &[SOURCE]),
    ("cordy_runtime_registered_total", &[RUNTIME_MODE, PROVIDER]),
    ("cordy_runtime_ready_total", &[RUNTIME_MODE, PROVIDER]),
    ("cordy_runtime_ready_seconds", &[RUNTIME_MODE, PROVIDER]),
    (
        "cordy_runtime_failed_total",
        &[RUNTIME_MODE, PROVIDER, FAILURE_REASON, RECOVERABLE],
    ),
    ("cordy_runtime_offline_total", &[RUNTIME_MODE, PROVIDER]),
    ("cordy_daemon_ws_message_received_total", &[KIND]),
    (
        "cordy_autopilot_run_started_total",
        &[CADENCE, TRIGGER_KIND],
    ),
    (
        "cordy_autopilot_run_terminal_total",
        &[CADENCE, TRIGGER_KIND, TERMINAL_STATUS],
    ),
    ("cordy_autopilot_run_skipped_total", &[CADENCE, REASON]),
    ("cordy_webhook_delivery_total", &[PROVIDER, STATUS]),
    ("cordy_webhook_rate_limited_total", &[GATE]),
    ("cordy_email_rate_limited_total", &[ACTION, GATE]),
    ("cordy_github_event_received_total", &[EVENT_KIND, ACTION]),
    ("cordy_github_pr_review_total", &[RESULT]),
    ("cordy_cloudruntime_request_total", &[OP, STATUS]),
    ("cordy_cloudruntime_request_duration_seconds", &[OP]),
    ("cordy_feedback_submitted_total", &[KIND, PLATFORM]),
    ("cordy_contact_sales_submitted_total", &[SOURCE]),
    ("cordy_chat_output_local_path_total", &[KIND]),
    ("cordy_entitlement_cache_total", &[OUTCOME]),
    ("cordy_entitlement_refresh_total", &[OUTCOME]),
    ("cordy_entitlement_refresh_duration_seconds", &[OUTCOME]),
    ("cordy_entitlement_decision_total", &[GATE, ACTION, REASON]),
    ("cordy_entitlement_version_regression_total", &[SOURCE]),
    (
        "cordy_autopilot_quota_decision_total",
        &[ACTION, SOURCE, RESULT],
    ),
    ("cordy_autopilot_failure_monitor_total", &[ACTION, OUTCOME]),
    ("cordy_autopilot_quota_reconciler_total", &[ACTION, OUTCOME]),
];

/// High-cardinality label names that must never appear on a business metric:
/// one series per tenant/task/user grows with entities rather than with the
/// deployment.
const FORBIDDEN_METRIC_LABELS: &[&str] = &[
    "workspace_id",
    // installation_id is the same class as the rest: one series per channel
    // installation, growing with tenants rather than with the deployment.
    "installation_id",
    "user_id",
    "agent_id",
    "task_id",
    "issue_id",
    "runtime_id",
    "session_id",
    "ip",
];

/// Panics when a business metric declares a forbidden high-cardinality label.
/// Called from `BusinessMetrics::new`, mirroring the Go constructor guard.
pub fn validate_business_metric_labels() {
    for (metric, labels) in BUSINESS_METRIC_LABELS {
        for label in *labels {
            if FORBIDDEN_METRIC_LABELS.contains(label) {
                panic!("forbidden high-cardinality label {label} on {metric}");
            }
        }
    }
}

/// Label names for a business metric; panics on an undefined metric so a typo
/// fails at construction rather than silently emitting unlabeled series.
pub(crate) fn metric_labels(metric: &str) -> &'static [&'static str] {
    for (name, labels) in BUSINESS_METRIC_LABELS {
        if *name == metric {
            return labels;
        }
    }
    panic!("missing business metric label definition for {metric}");
}

const KNOWN_SOURCES: &[&str] = &[
    "issue",
    "chat",
    "autopilot",
    "autopilot_issue",
    "quick_create",
    "manual",
    "api",
    "other",
];
const KNOWN_RUNTIME_MODES: &[&str] = &["local", "cloud", "unknown"];
const KNOWN_RUNTIME_PROVIDERS: &[&str] = &[
    "antigravity",
    "claude",
    "codebuddy",
    "codex",
    "copilot",
    "cursor",
    "dsh",
    "gemini",
    "grok",
    "hermes",
    "kiro",
    "kimi",
    "reasonix",
    "dim",
    "mcode",
    "cordy_agent",
    "openclaw",
    "opencode",
    "deveco",
    "pi",
    "qoder",
    "qoderclicn",
    "qwen",
    "traecli",
    "other",
];
const KNOWN_TERMINAL_STATUSES: &[&str] = &["completed", "failed", "cancelled", "blocked", "other"];
const KNOWN_TOKEN_TYPES: &[&str] = &["input", "output", "cache_read", "cache_write"];

fn normalize_from(list: &[&str], value: &str, fallback: &str) -> String {
    let v = value.trim().to_lowercase();
    if list.contains(&v.as_str()) {
        v
    } else {
        fallback.to_string()
    }
}

static KNOWN_FAILURE_REASONS: LazyLock<HashSet<String>> = LazyLock::new(|| {
    cordy_task_failure::all_reasons()
        .iter()
        .map(|r| r.as_str().to_string())
        .collect()
});

static MODEL_ALIAS_UNSAFE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^a-z0-9._:/+-]+").unwrap());

pub fn normalize_task_source(value: &str) -> String {
    normalize_from(KNOWN_SOURCES, value, "other")
}

pub fn normalize_runtime_mode(value: &str) -> String {
    normalize_from(KNOWN_RUNTIME_MODES, value, "unknown")
}

pub fn normalize_runtime_provider(value: &str) -> String {
    normalize_from(KNOWN_RUNTIME_PROVIDERS, value, "other")
}

pub fn normalize_terminal_status(value: &str) -> String {
    normalize_from(KNOWN_TERMINAL_STATUSES, value, "other")
}

/// Canonical failure reasons pass through verbatim (case-sensitive, trim
/// only); anything else falls to [`cordy_task_failure::classify`] so free-form
/// error text still lands in a bounded `agent_error.*` bucket.
pub fn normalize_failure_reason(value: &str) -> String {
    let v = value.trim();
    if KNOWN_FAILURE_REASONS.contains(v) {
        v.to_string()
    } else {
        cordy_task_failure::classify(v).to_string()
    }
}

pub fn normalize_token_type(value: &str) -> String {
    normalize_from(KNOWN_TOKEN_TYPES, value, "input")
}

/// Lowercases, replaces every character outside `[a-z0-9._:/+-]` with `_`,
/// and caps at 128 bytes. Post-replacement output is pure ASCII, so the byte
/// cap can never split a multi-byte rune.
pub fn normalize_model_alias(value: &str) -> String {
    let v = value.trim().to_lowercase();
    if v.is_empty() {
        return "unknown".to_string();
    }
    let sanitized = MODEL_ALIAS_UNSAFE_RE.replace_all(&v, "_");
    if sanitized.len() > 128 {
        sanitized[..128].to_string()
    } else {
        sanitized.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_passes_on_current_table() {
        validate_business_metric_labels();
    }

    #[test]
    #[should_panic(expected = "missing business metric label definition")]
    fn metric_labels_panics_on_unknown_metric() {
        metric_labels("cordy_nonexistent_total");
    }

    #[test]
    fn normalizers_fall_back_to_buckets() {
        assert_eq!(normalize_task_source(" Issue "), "issue");
        assert_eq!(normalize_task_source("Slack"), "other");
        assert_eq!(normalize_runtime_mode("CLOUD"), "cloud");
        assert_eq!(normalize_runtime_mode("self-hosted"), "unknown");
        assert_eq!(normalize_runtime_provider("Claude"), "claude");
        assert_eq!(normalize_runtime_provider("made-up"), "other");
        assert_eq!(normalize_terminal_status("Completed"), "completed");
        assert_eq!(normalize_terminal_status(""), "other");
        assert_eq!(normalize_token_type("CACHE_READ"), "cache_read");
        assert_eq!(normalize_token_type("weird"), "input");
    }

    #[test]
    fn failure_reason_passthrough_then_classify() {
        assert_eq!(normalize_failure_reason(" timeout "), "timeout");
        assert_eq!(
            normalize_failure_reason("agent_error.provider_network"),
            "agent_error.provider_network"
        );
        // Free-form text falls through to the classifier.
        assert_eq!(
            normalize_failure_reason("dial tcp 1.2.3.4:443: connection refused"),
            "agent_error.provider_network"
        );
        // Case-sensitive canonical lookup: not a known value → classified.
        assert_eq!(normalize_failure_reason("TIMEOUT"), "agent_error.unknown");
    }

    #[test]
    fn model_alias_sanitizes_and_caps() {
        assert_eq!(normalize_model_alias("  GPT-5.6 Luna! "), "gpt-5.6_luna_");
        assert_eq!(normalize_model_alias(""), "unknown");
        assert_eq!(normalize_model_alias("   "), "unknown");
        let long = format!("{}{}", "a".repeat(200), " 字");
        let out = normalize_model_alias(&long);
        assert_eq!(out.len(), 128);
        assert!(out.is_ascii());
    }
}
