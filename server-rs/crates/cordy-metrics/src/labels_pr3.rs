//! PR3 metric-label normalizers — port of
//! `server/internal/metrics/labels_pr3.go`.
//!
//! All inputs go through fixed allow-lists so a misbehaving caller cannot
//! inflate metric cardinality. Every "unknown"/"other" bucket keeps the series
//! count bounded even under enum drift.

use serde_json::Value;

const KNOWN_PLATFORMS: &[&str] = &[
    "server", "web", "desktop", "cli", "mobile", "ios", "unknown",
];

/// Fixed bucket set for the signup_source label. The PostHog event still ships
/// the raw cookie value so analytics keeps the long tail; the Prometheus side
/// gets the bucketed version so cardinality stays bounded even if a misbehaving
/// frontend writes a unique-per-visitor cookie.
const KNOWN_SIGNUP_SOURCES: &[(&str, &str)] = &[
    ("direct", "direct"),
    ("google", "google"),
    ("bing", "bing"),
    ("duckduckgo", "duckduckgo"),
    ("twitter", "twitter"),
    ("x", "twitter"),
    ("linkedin", "linkedin"),
    ("facebook", "facebook"),
    ("instagram", "instagram"),
    ("github", "github"),
    ("gitlab", "gitlab"),
    ("hacker_news", "hacker_news"),
    ("hackernews", "hacker_news"),
    ("reddit", "reddit"),
    ("youtube", "youtube"),
    ("discord", "discord"),
    ("slack", "slack"),
    ("product_hunt", "product_hunt"),
    ("producthunt", "product_hunt"),
    ("medium", "medium"),
    ("dev_to", "dev_to"),
    ("devto", "dev_to"),
    ("email", "email"),
    ("newsletter", "email"),
    ("organic", "organic"),
    ("referral", "referral"),
    ("partner", "partner"),
    ("affiliate", "affiliate"),
    ("ad", "paid"),
    ("ads", "paid"),
    ("paid", "paid"),
    ("cpc", "paid"),
    ("sem", "paid"),
    ("other", "other"),
];

const KNOWN_ONBOARDING_PATHS: &[&str] = &[
    "full",
    "runtime_skipped",
    "cloud_waitlist",
    "skip_existing",
    "invite_accept",
    "unknown",
];
const KNOWN_AUTOPILOT_CADENCES: &[&str] = &[
    "hourly", "daily", "weekly", "monthly", "manual", "webhook", "unknown",
];
const KNOWN_AUTOPILOT_TRIGGERS: &[&str] = &["schedule", "webhook", "manual", "unknown"];
const KNOWN_AUTOPILOT_SKIP_REASONS: &[&str] = &[
    "already_running",
    "recent_run",
    "runtime_offline",
    "throttled",
    "max_concurrency",
    "trigger_disabled",
    "signature_invalid",
    "unknown",
    "other",
];
const KNOWN_WEBHOOK_PROVIDERS: &[&str] = &["github", "generic", "gitlab", "stripe", "other"];
const KNOWN_WEBHOOK_DELIVERY_STATUSES: &[&str] = &[
    "queued",
    "dispatched",
    "failed",
    "rejected",
    "ignored",
    "duplicate",
    "other",
];
const KNOWN_WEBHOOK_RATE_LIMIT_GATES: &[&str] = &[
    "absolute_ip",
    "bad_credential_ip",
    "worker_trigger",
    "other",
];
const KNOWN_EMAIL_RATE_LIMIT_ACTIONS: &[&str] = &["workspace_invitation", "other"];
const KNOWN_EMAIL_RATE_LIMIT_GATES: &[&str] = &["actor", "workspace", "recipient", "other"];
const KNOWN_GITHUB_EVENT_KINDS: &[&str] = &[
    "pull_request",
    "pull_request_review",
    "issues",
    "issue_comment",
    "push",
    "installation",
    "installation_repositories",
    "check_run",
    "check_suite",
    "ping",
    "other",
];
const KNOWN_GITHUB_ACTIONS: &[&str] = &[
    "opened",
    "closed",
    "reopened",
    "merged",
    "synchronize",
    "edited",
    "submitted",
    "created",
    "deleted",
    "labeled",
    "unlabeled",
    "assigned",
    "unassigned",
    "requested",
    "completed",
    "none",
    "other",
];
const KNOWN_GITHUB_PR_REVIEW_RESULTS: &[&str] = &[
    "approved",
    "changes_requested",
    "commented",
    "dismissed",
    "other",
];
const KNOWN_CLOUD_RUNTIME_OPS: &[&str] = &[
    "provision",
    "terminate",
    "status",
    "gateway",
    "billing",
    "fleet",
    "other",
];
const KNOWN_CLOUD_RUNTIME_STATUSES: &[&str] = &["ok", "4xx", "5xx", "timeout", "error"];
const KNOWN_DAEMON_WS_KINDS: &[&str] = &[
    "heartbeat",
    "task_claim",
    "task_complete",
    "task_usage",
    "task_progress",
    "task_messages",
    "log",
    "other",
];
const KNOWN_FEEDBACK_KINDS: &[&str] = &["bug", "feature", "general", "praise", "other"];
/// Evidence kinds for cordy_chat_output_local_path_total (MUL-4899). A closed
/// allowlist is what keeps the offending path out of Prometheus: the caller
/// passes a classification, never a fragment of the reply.
const KNOWN_CHAT_OUTPUT_LOCAL_PATH_KINDS: &[&str] = &["file_url", "workdir_path"];
const KNOWN_CONTACT_SALES_SOURCES: &[&str] =
    &["page", "onboarding", "agents_page", "unknown", "other"];

fn normalize_from(list: &[&str], value: &str, fallback: &str) -> String {
    let v = value.trim().to_lowercase();
    if list.contains(&v.as_str()) {
        v
    } else {
        fallback.to_string()
    }
}

fn normalize_from_pairs(pairs: &[(&str, &str)], value: &str, fallback: &str) -> String {
    let v = value.trim().to_lowercase();
    for (key, mapped) in pairs {
        if *key == v {
            return (*mapped).to_string();
        }
    }
    fallback.to_string()
}

pub fn normalize_platform(value: &str) -> String {
    normalize_from(KNOWN_PLATFORMS, value, "unknown")
}

/// Buckets the raw cordy_signup_source cookie payload into the fixed signup
/// channel allow-list. The cookie carries free-form JSON (utm_source /
/// utm_medium / referrer); here we only need a bounded label, so we look at
/// utm_source / source / referrer fields when present, otherwise the bare
/// string. Empty → "direct"; anything not in the allow-list → "other".
pub fn normalize_signup_source(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "direct".to_string();
    }
    let mut current = trimmed.to_string();
    // JSON shape: {"utm_source":"...","utm_medium":"...","referrer":"..."}
    if current.starts_with('{') {
        if let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(&current) {
            for key in ["utm_source", "source", "referrer", "ref"] {
                if let Some(s) = parsed.get(key).and_then(Value::as_str) {
                    if !s.trim().is_empty() {
                        current = s.to_string();
                        break;
                    }
                }
            }
        }
    }
    normalize_from_pairs(
        KNOWN_SIGNUP_SOURCES,
        &canonicalise_signup_channel(&current),
        "other",
    )
}

/// Collapses the raw signup-source string into a shape the allow-list can
/// match: lowercase, trimmed, host-only for URL-ish values, and with a few
/// obvious aliases unified ("twitter.com" → "twitter"). Deliberately
/// defensive — the cookie is set client-side, so any shape is possible.
fn canonicalise_signup_channel(value: &str) -> String {
    let mut v = value.trim().to_lowercase();
    if v.is_empty() {
        return v;
    }
    // Strip an optional URL scheme so "https://twitter.com/foo" → "twitter.com/foo".
    for scheme in ["https://", "http://", "//"] {
        if let Some(stripped) = v.strip_prefix(scheme) {
            v = stripped.to_string();
            break;
        }
    }
    // Take just the host segment.
    if let Some(i) = v.find(['/', '?', '#']) {
        v = v[..i].to_string();
    }
    if let Some(stripped) = v.strip_prefix("www.") {
        v = stripped.to_string();
    }
    // Map well-known hostnames to their channel bucket.
    const HOST_ALIASES: &[(&str, &str)] = &[
        ("google.com", "google"),
        ("google.co.uk", "google"),
        ("bing.com", "bing"),
        ("duckduckgo.com", "duckduckgo"),
        ("twitter.com", "twitter"),
        ("x.com", "twitter"),
        ("t.co", "twitter"),
        ("linkedin.com", "linkedin"),
        ("lnkd.in", "linkedin"),
        ("facebook.com", "facebook"),
        ("fb.com", "facebook"),
        ("instagram.com", "instagram"),
        ("github.com", "github"),
        ("gitlab.com", "gitlab"),
        ("news.ycombinator.com", "hacker_news"),
        ("reddit.com", "reddit"),
        ("old.reddit.com", "reddit"),
        ("youtube.com", "youtube"),
        ("youtu.be", "youtube"),
        ("discord.com", "discord"),
        ("discord.gg", "discord"),
        ("slack.com", "slack"),
        ("producthunt.com", "product_hunt"),
        ("medium.com", "medium"),
        ("dev.to", "dev_to"),
    ];
    for (host, channel) in HOST_ALIASES {
        if v == *host {
            return (*channel).to_string();
        }
    }
    v
}

pub fn normalize_onboarding_path(value: &str) -> String {
    normalize_from(KNOWN_ONBOARDING_PATHS, value, "unknown")
}

pub fn normalize_autopilot_cadence(value: &str) -> String {
    normalize_from(KNOWN_AUTOPILOT_CADENCES, value, "unknown")
}

pub fn normalize_autopilot_trigger(value: &str) -> String {
    normalize_from(KNOWN_AUTOPILOT_TRIGGERS, value, "unknown")
}

pub fn normalize_autopilot_skip_reason(value: &str) -> String {
    normalize_from(KNOWN_AUTOPILOT_SKIP_REASONS, value, "other")
}

pub fn normalize_webhook_provider(value: &str) -> String {
    normalize_from(KNOWN_WEBHOOK_PROVIDERS, value, "other")
}

pub fn normalize_webhook_delivery_status(value: &str) -> String {
    normalize_from(KNOWN_WEBHOOK_DELIVERY_STATUSES, value, "other")
}

pub fn normalize_webhook_rate_limit_gate(value: &str) -> String {
    normalize_from(KNOWN_WEBHOOK_RATE_LIMIT_GATES, value, "other")
}

pub fn normalize_email_rate_limit_action(value: &str) -> String {
    normalize_from(KNOWN_EMAIL_RATE_LIMIT_ACTIONS, value, "other")
}

pub fn normalize_email_rate_limit_gate(value: &str) -> String {
    normalize_from(KNOWN_EMAIL_RATE_LIMIT_GATES, value, "other")
}

pub fn normalize_github_event_kind(value: &str) -> String {
    normalize_from(KNOWN_GITHUB_EVENT_KINDS, value, "other")
}

pub fn normalize_github_action(value: &str) -> String {
    if value.trim().is_empty() {
        return "none".to_string();
    }
    normalize_from(KNOWN_GITHUB_ACTIONS, value, "other")
}

pub fn normalize_github_pr_review_result(value: &str) -> String {
    normalize_from(KNOWN_GITHUB_PR_REVIEW_RESULTS, value, "other")
}

pub fn normalize_cloud_runtime_op(value: &str) -> String {
    normalize_from(KNOWN_CLOUD_RUNTIME_OPS, value, "other")
}

/// Collapses an HTTP status code or symbolic outcome string into the fixed
/// bucket set {ok, 4xx, 5xx, timeout, error}. Empty / unknown → "error".
pub fn normalize_cloud_runtime_status(value: &str) -> String {
    let v = value.trim().to_lowercase();
    if KNOWN_CLOUD_RUNTIME_STATUSES.contains(&v.as_str()) {
        return v;
    }
    if v.len() == 3 {
        match v.as_bytes()[0] {
            b'2' => return "ok".to_string(),
            b'4' => return "4xx".to_string(),
            b'5' => return "5xx".to_string(),
            _ => {}
        }
    }
    "error".to_string()
}

/// Maps an HTTP status code to its bucket label. Used by cloudruntime client
/// instrumentation.
pub fn cloud_runtime_status_for_code(code: i32) -> String {
    match code {
        200..=399 => "ok".to_string(),
        400..=499 => "4xx".to_string(),
        500..=599 => "5xx".to_string(),
        _ => "error".to_string(),
    }
}

pub fn normalize_daemon_ws_kind(value: &str) -> String {
    normalize_from(KNOWN_DAEMON_WS_KINDS, value, "other")
}

pub fn normalize_feedback_kind(value: &str) -> String {
    normalize_from(KNOWN_FEEDBACK_KINDS, value, "other")
}

pub fn normalize_contact_sales_source(value: &str) -> String {
    normalize_from(KNOWN_CONTACT_SALES_SOURCES, value, "other")
}

pub fn normalize_chat_output_local_path_kind(value: &str) -> String {
    normalize_from(KNOWN_CHAT_OUTPUT_LOCAL_PATH_KINDS, value, "other")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_and_cadence_buckets() {
        assert_eq!(normalize_platform(" Desktop "), "desktop");
        assert_eq!(normalize_platform("watchos"), "unknown");
        assert_eq!(normalize_autopilot_cadence("Weekly"), "weekly");
        assert_eq!(normalize_autopilot_cadence("cron"), "unknown");
        assert_eq!(normalize_autopilot_skip_reason("THROTTLED"), "throttled");
        assert_eq!(normalize_autopilot_skip_reason("vibe"), "other");
    }

    #[test]
    fn signup_source_bare_aliases() {
        assert_eq!(normalize_signup_source(""), "direct");
        assert_eq!(normalize_signup_source("   "), "direct");
        assert_eq!(normalize_signup_source("x"), "twitter");
        assert_eq!(normalize_signup_source("hackernews"), "hacker_news");
        assert_eq!(normalize_signup_source("cpc"), "paid");
        assert_eq!(normalize_signup_source("baidu"), "other");
    }

    #[test]
    fn signup_source_json_cookie_extracts_field() {
        let cookie = r#"{"utm_source":"x","utm_medium":"social","referrer":""}"#;
        assert_eq!(normalize_signup_source(cookie), "twitter");
        // Falls through keys until a non-empty string is found.
        let cookie = r#"{"utm_source":"","source":"github"}"#;
        assert_eq!(normalize_signup_source(cookie), "github");
        // Malformed JSON → treated as a bare string → other.
        assert_eq!(normalize_signup_source("{not-json"), "other");
    }

    #[test]
    fn signup_source_url_host_aliasing() {
        assert_eq!(
            normalize_signup_source("https://news.ycombinator.com/item?id=1"),
            "hacker_news"
        );
        assert_eq!(
            normalize_signup_source("https://www.youtube.com/watch"),
            "youtube"
        );
        assert_eq!(normalize_signup_source("t.co/abc"), "twitter");
        // Unknown hosts collapse to "other" — no free-form passthrough.
        assert_eq!(normalize_signup_source("example.com/page"), "other");
    }

    #[test]
    fn github_action_empty_is_none() {
        assert_eq!(normalize_github_action(""), "none");
        assert_eq!(normalize_github_action("  "), "none");
        assert_eq!(normalize_github_action("Opened"), "opened");
        assert_eq!(normalize_github_action("yeeted"), "other");
    }

    #[test]
    fn cloud_runtime_status_bucketing() {
        assert_eq!(normalize_cloud_runtime_status("OK"), "ok");
        assert_eq!(normalize_cloud_runtime_status("timeout"), "timeout");
        assert_eq!(normalize_cloud_runtime_status("200"), "ok");
        assert_eq!(normalize_cloud_runtime_status("404"), "4xx");
        assert_eq!(normalize_cloud_runtime_status("503"), "5xx");
        assert_eq!(normalize_cloud_runtime_status("301"), "error");
        assert_eq!(normalize_cloud_runtime_status(""), "error");

        assert_eq!(cloud_runtime_status_for_code(199), "error");
        assert_eq!(cloud_runtime_status_for_code(200), "ok");
        assert_eq!(cloud_runtime_status_for_code(302), "ok");
        assert_eq!(cloud_runtime_status_for_code(400), "4xx");
        assert_eq!(cloud_runtime_status_for_code(500), "5xx");
        assert_eq!(cloud_runtime_status_for_code(600), "error");
    }

    #[test]
    fn chat_output_local_path_closed_enum() {
        assert_eq!(
            normalize_chat_output_local_path_kind("file_url"),
            "file_url"
        );
        assert_eq!(
            normalize_chat_output_local_path_kind("workdir_path"),
            "workdir_path"
        );
        // Never a fragment of the reply — closed enum only.
        assert_eq!(
            normalize_chat_output_local_path_kind("/Users/x/secrets"),
            "other"
        );
    }
}
