//! Self-contained task helpers and retry policy (truncate/summary, trivial
//! done detection, retry ceilings and delays, resume-safety guards).
//!
//! Everything here is free of service state.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use patchbay_db::models::AgentTaskQueue;

use crate::task_failure;

/// Caps the trigger-comment snapshot length so the row stays cheap to
/// transmit (it ends up in every task list response).
pub const TRIGGER_SUMMARY_MAX_LEN: usize = 200;

/// Bounds the completion-fallback comment synthesized from a task's final
/// output when the agent left no comment of its own. Anything larger is a
/// runaway raw-stream dump (observed at 190–264 KB) which must never be
/// posted, even partially, to the issue thread.
pub const MAX_SYNTHESIZED_FALLBACK_COMMENT_RUNES: usize = 8000;

pub const OVERSIZED_FALLBACK_COMMENT_NOTICE: &str = "This task completed, but its output was too large to post safely. The raw output was not posted. Review the task in this issue's Agent thread.";

pub const TASK_ANALYTICS_CONTEXT_CACHE_MAX: usize = 4096;

/// Maximum DB heartbeat age accepted by every task release path (deferred
/// promotion, stale-dispatch reclaim, fresh claim). Must exceed the 60s DB
/// heartbeat flush interval + one ~15s daemon heartbeat + ~30s batch
/// scheduler tick; 150s leaves a 45s buffer above that 105s worst case.
pub const RUNTIME_CLAIM_FRESHNESS_SECONDS: f64 = 150.0;

/// Must exceed daemon client.Timeout for /tasks/claim (30s) plus
/// /tasks/{id}/start (30s) plus scheduling slack. Longer pre-start work is
/// protected by [`PREPARE_LEASE_DURATION`] instead.
pub const CLAIM_RESPONSE_RECOVERY_WINDOW: Duration = Duration::from_secs(90);

pub const PREPARE_LEASE_DURATION: Duration = Duration::from_secs(45);

/// Shortens `s` to `max_runes` with a trailing `…` when truncated. Operates
/// on runes (not bytes) so multibyte characters count as one each; flattens
/// newlines/tabs to spaces and strips surrounding whitespace first so a
/// leading newline doesn't waste budget.
pub fn truncate_for_summary(s: &str, max_runes: usize) -> String {
    let flattened: String = s
        .chars()
        .map(|r| match r {
            '\n' | '\r' | '\t' => ' ',
            other => other,
        })
        .collect();
    let rs: Vec<char> = flattened.trim().chars().collect();
    if rs.len() <= max_runes {
        return rs.into_iter().collect();
    }
    let mut out: String = rs[..max_runes].iter().collect();
    out.push('…');
    out
}

/// Bounds a synthesized completion-fallback comment body. Unlike
/// [`truncate_for_summary`] it preserves genuine final messages below the cap
/// verbatim; above the cap the whole body is replaced with a fixed notice —
/// runaway dumps put process narration at the head, so any excerpt can expose
/// execution details and still discard the final answer. Callers pass the
/// already-redacted body.
pub fn truncate_fallback_comment_body(body: &str, max_runes: usize) -> String {
    if body.chars().count() <= max_runes {
        return body.to_string();
    }
    OVERSIZED_FALLBACK_COMMENT_NOTICE.to_string()
}

const TRIVIAL_DONE_MARKERS: &[&str] = &["done", "готово", "готова", "сделано", "完成", "完了"];

pub fn is_trivial_done_output(output: &str) -> bool {
    let normalized = output.trim().to_lowercase();
    let normalized = normalized.trim_matches(['.', '!', '！', '。', '…', ' ']);
    TRIVIAL_DONE_MARKERS.contains(&normalized)
}

// --- Retry policy --------------------------------------------------------

/// Failure reasons eligible for automatic retry. Only infrastructure-shaped
/// failures re-run; provider auth/quota etc. are terminal.
fn retryable(reason: &str) -> bool {
    matches!(
        reason,
        "runtime_offline"
            | "runtime_recovery"
            | "timeout"
            | "codex_semantic_inactivity"
            | "agent_error.provider_network"
            | "skill_bundle_unavailable"
    )
}

const RUNTIME_OFFLINE_RETRY_DEFERRAL: Duration = Duration::from_secs(1);
const PROVIDER_NETWORK_MAX_ATTEMPTS: i32 = 3;
const PROVIDER_NETWORK_FINAL_RETRY_WAIT: Duration = Duration::from_secs(5);

/// How many attempts the auto-retry path allows for a failure reason. Only
/// ever WIDENS the task's generic max_attempts, and only for reasons with a
/// bespoke schedule. max_attempts <= 1 explicitly disables auto-retry, so it
/// is never overridden — a disabled task must not be revived by a raised
/// ceiling. Persisted into the retry child so the row stays self-consistent.
pub fn retry_attempt_ceiling(reason: &str, task_max_attempts: i32) -> i32 {
    if task_max_attempts <= 1 {
        return task_max_attempts;
    }
    if reason == task_failure::Reason::AGENT_PROVIDER_NETWORK.as_str()
        && task_max_attempts < PROVIDER_NETWORK_MAX_ATTEMPTS
    {
        return PROVIDER_NETWORK_MAX_ATTEMPTS;
    }
    task_max_attempts
}

/// How long to defer the NEXT attempt after a failure at `failed_attempt`.
/// runtime_offline always gets a positive fire_at (health-gated promotion);
/// provider_network's final attempt is deferred ~5s; every other retry is
/// immediate (zero delay → child created claimable at once).
pub fn retry_delay_for_attempt(reason: &str, failed_attempt: i32) -> Duration {
    if reason == task_failure::Reason::RUNTIME_OFFLINE.as_str() {
        return RUNTIME_OFFLINE_RETRY_DEFERRAL;
    }
    if reason == task_failure::Reason::AGENT_PROVIDER_NETWORK.as_str()
        && failed_attempt >= PROVIDER_NETWORK_MAX_ATTEMPTS - 1
    {
        return PROVIDER_NETWORK_FINAL_RETRY_WAIT;
    }
    Duration::ZERO
}

fn resume_unsafe_failure_reason(reason: &str) -> bool {
    // Failures that poison the agent CONVERSATION (not the workdir): resuming
    // the same session would immediately replay the stuck state. Keep in sync
    // with the GetLastTaskSession / GetLastChatTaskSession resume blacklists.
    matches!(
        reason,
        "iteration_limit"
            | "agent_fallback_message"
            | "api_invalid_request"
            | "codex_semantic_inactivity"
            | "agent_error.context_overflow"
            | "codex_resume_oversized"
    )
}

/// Reports whether a failed task's agent session must NOT be resumed on a
/// retry. Combines the failure_reason poison set with the same
/// defense-in-depth on raw error text the resume queries apply: an Anthropic
/// 400 invalid_request_error means the history itself is unprocessable even
/// when failure_reason was mis- or un-classified. Callers holding only a
/// failure_reason may pass an empty error_text.
pub fn resume_unsafe_failure(failure_reason: &str, error_text: &str) -> bool {
    if resume_unsafe_failure_reason(failure_reason) {
        return true;
    }
    let lower = error_text.to_lowercase();
    if lower.contains("400") && lower.contains("invalid_request_error") {
        return true;
    }
    // Provider credential-resolution failures are deterministic on resume:
    // the missing credential is baked into the session's provider state.
    if task_failure::auth_method_unresolved(error_text) {
        return true;
    }
    // Same defense-in-depth for the provider-agnostic empty-message shape:
    // a daemon too old to carry the poisoned-error branch reports
    // agent_error.unknown, and without this the manual-retry path would
    // resume the Agent event history the provider just refused.
    task_failure::unresumable_history(error_text)
}

/// Reports whether a failed task qualifies for an automatic retry attempt:
/// an infrastructure-shaped failure_reason, remaining attempt budget, not an
/// automation run, and linked to an issue or chat session. Shared by FailTask's
/// in-transaction retry and the orphan sweeper so both agree on which
/// failures re-run.
pub fn retry_eligible(failure_reason: &str, t: &AgentTaskQueue) -> bool {
    retryable(failure_reason)
        && t.attempt < retry_attempt_ceiling(failure_reason, t.max_attempts)
        && t.automation_run_id.is_none()
        && (t.issue_id.is_some() || t.chat_session_id.is_some())
}

/// Reports whether another not-yet-started task already occupies the single
/// queued/dispatched slot per (issue, agent). Advisory only — no lock — but
/// CreateRetryTask's ON CONFLICT DO NOTHING makes losing the race harmless.
/// Chat / quick-create tasks carry no issue_id so their retries never collide.
pub async fn has_runnable_successor(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    task: &AgentTaskQueue,
) -> anyhow::Result<bool> {
    let Some(issue_id) = task.issue_id else {
        return Ok(false);
    };
    patchbay_db::queries::agent::has_pending_task_for_issue_and_agent(
        executor,
        issue_id,
        task.agent_id,
        None,
    )
    .await
    .map(|v| v.unwrap_or(false))
}

// --- Misc mappers --------------------------------------------------------

/// Total wall-clock from enqueue (user hit send) to terminal state, stored on
/// the assistant chat_message so the UI can render "Replied in 38s". Uses
/// created_at — not started_at — because users experience total wait time
/// including queue + dispatch. None when either bound is unset; negative
/// clamps to zero.
pub fn compute_chat_elapsed_ms(
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
    created_at: chrono::DateTime<chrono::Utc>,
) -> Option<i64> {
    let completed_at = completed_at?;
    let ms = (completed_at - created_at).num_milliseconds();
    Some(ms.max(0))
}

pub fn priority_to_int(p: &str) -> i32 {
    match p {
        "urgent" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

/// Kept for parity with Go's package-level map literal shape; the predicate
/// form above is what call sites use.
#[allow(dead_code)]
static RETRYABLE_REASONS: LazyLock<HashMap<&'static str, bool>> = LazyLock::new(|| {
    [
        ("runtime_offline", true),
        ("runtime_recovery", true),
        ("timeout", true),
        ("codex_semantic_inactivity", true),
        ("agent_error.provider_network", true),
        ("skill_bundle_unavailable", true),
    ]
    .into_iter()
    .collect()
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_for_summary_flattens_and_caps_by_runes() {
        assert_eq!(
            truncate_for_summary("\n hello\tworld \r", 200),
            "hello world"
        );
        // Multibyte runes count as one each ("你好世界再见" = 6 runes).
        let cjk = "你好世界再见";
        assert_eq!(truncate_for_summary(cjk, 4), "你好世界…");
        assert_eq!(truncate_for_summary(cjk, 5), "你好世界再…");
        assert_eq!(truncate_for_summary(cjk, 6), cjk);
        // Byte-length input far beyond rune budget.
        let long = "字".repeat(300);
        let out = truncate_for_summary(&long, 10);
        assert_eq!(out.chars().count(), 11); // 10 + ellipsis
    }

    #[test]
    fn fallback_comment_replaced_wholesale_above_cap() {
        let small = "a real final answer";
        assert_eq!(truncate_fallback_comment_body(small, 100), small);
        let big = format!("{}{}", "narration ", "x".repeat(9000));
        assert_eq!(
            truncate_fallback_comment_body(&big, MAX_SYNTHESIZED_FALLBACK_COMMENT_RUNES),
            OVERSIZED_FALLBACK_COMMENT_NOTICE
        );
    }

    #[test]
    fn trivial_done_multilingual_with_punctuation() {
        for marker in [
            "done",
            "Done.",
            "DONE!",
            "完成",
            "完成。",
            "готово",
            "完了…",
        ] {
            assert!(is_trivial_done_output(marker), "{marker}");
        }
        assert!(!is_trivial_done_output("done: all tests pass"));
        assert!(!is_trivial_done_output(""));
    }

    #[test]
    fn ceiling_widens_only_provider_network_and_never_revives_disabled() {
        assert_eq!(retry_attempt_ceiling("agent_error.provider_network", 2), 3);
        assert_eq!(retry_attempt_ceiling("runtime_offline", 2), 2);
        assert_eq!(retry_attempt_ceiling("agent_error.provider_network", 5), 5);
        // Disabled tasks stay disabled regardless of reason.
        assert_eq!(retry_attempt_ceiling("agent_error.provider_network", 1), 1);
        assert_eq!(retry_attempt_ceiling("anything", 0), 0);
    }

    #[test]
    fn delays_follow_bespoke_schedules() {
        assert_eq!(
            retry_delay_for_attempt("runtime_offline", 1),
            Duration::from_secs(1)
        );
        // provider_network: attempts 0 and 1 immediate, final wait after 2nd failure.
        assert_eq!(
            retry_delay_for_attempt("agent_error.provider_network", 0),
            Duration::ZERO
        );
        assert_eq!(
            retry_delay_for_attempt("agent_error.provider_network", 2),
            Duration::from_secs(5)
        );
        assert_eq!(retry_delay_for_attempt("timeout", 1), Duration::ZERO);
    }

    #[test]
    fn resume_poison_set_and_text_guards() {
        for reason in [
            "iteration_limit",
            "agent_fallback_message",
            "api_invalid_request",
            "codex_semantic_inactivity",
            "agent_error.context_overflow",
            "codex_resume_oversized",
        ] {
            assert!(resume_unsafe_failure(reason, ""), "{reason}");
        }
        assert!(!resume_unsafe_failure("timeout", ""));
        // Raw-text guards fire even with a benign reason.
        assert!(resume_unsafe_failure(
            "agent_error.unknown",
            "API Error: 400 {\"type\":\"invalid_request_error\"}"
        ));
        assert!(resume_unsafe_failure(
            "agent_error.unknown",
            "Could not resolve authentication method for provider"
        ));
        assert!(resume_unsafe_failure(
            "agent_error.unknown",
            "the message at position 37 with role 'assistant' must not be empty"
        ));
        assert!(!resume_unsafe_failure(
            "agent_error.unknown",
            "transient network blip"
        ));
    }

    #[test]
    fn priority_maps_four_levels_plus_default() {
        assert_eq!(priority_to_int("urgent"), 4);
        assert_eq!(priority_to_int("high"), 3);
        assert_eq!(priority_to_int("medium"), 2);
        assert_eq!(priority_to_int("low"), 1);
        assert_eq!(priority_to_int(""), 0);
        assert_eq!(priority_to_int("bogus"), 0);
    }

    #[test]
    fn chat_elapsed_uses_created_not_started_and_clamps_negative() {
        use chrono::Utc;
        let created = Utc::now();
        assert_eq!(
            compute_chat_elapsed_ms(Some(created + chrono::Duration::seconds(38)), created),
            Some(38_000)
        );
        assert_eq!(compute_chat_elapsed_ms(None, created), None);
        assert_eq!(
            compute_chat_elapsed_ms(Some(created - chrono::Duration::seconds(5)), created),
            Some(0)
        );
    }
}
