//! Poisoned-session failure classification.
//!
//! Failure-reason classifiers for tasks whose session is "poisoned" — i.e.
//! resuming the same conversation on a follow-up task would deterministically
//! reproduce the same failure. The server-side GetLastTaskSession query filters
//! these reasons out so the next task starts from a fresh agent session.
//!
//! Symbol map (Go → Rust):
//! - `FailureReason*` constants → [`FAILURE_REASON_*`]
//! - `poisonedOutputMaxLen` → [`POISONED_OUTPUT_MAX_LEN`]
//! - `classifyPoisonedOutput` → [`classify_poisoned_output`]
//! - `classifyPoisonedError` → [`classify_poisoned_error`]
//! - `classifyResumeUnsafeTransport` → [`classify_resume_unsafe_transport`]
//! - `classifyResumeUnsafeTimeout` → [`classify_resume_unsafe_timeout`]
//!
//! The shared taskfailure/agent classifiers (`taskfailure.ContextExhaustedCompletion`,
//! `UnresumableHistory`, `agent.CodexResumeOverflowError`, the codex markers)
//! are ported inline below with their Go doc comments condensed, since the
//! daemon crate does not depend on those server packages (same stand-in
//! pattern as types.rs). Wire values are byte-identical.

use regex::Regex;

/// `FailureReasonIterationLimit` = string(taskfailure.ReasonIterationLimit).
pub(crate) const FAILURE_REASON_ITERATION_LIMIT: &str = "iteration_limit";
/// `FailureReasonAgentFallbackMsg`.
pub(crate) const FAILURE_REASON_AGENT_FALLBACK_MSG: &str = "agent_fallback_message";
/// `FailureReasonAPIInvalidRequest` = string(taskfailure.ReasonAPIInvalidRequest).
pub(crate) const FAILURE_REASON_API_INVALID_REQUEST: &str = "api_invalid_request";
/// `FailureReasonCodexSemanticInactivity`.
pub(crate) const FAILURE_REASON_CODEX_SEMANTIC_INACTIVITY: &str = "codex_semantic_inactivity";
/// `FailureReasonCodexResumeOversized`.
pub(crate) const FAILURE_REASON_CODEX_RESUME_OVERSIZED: &str = "codex_resume_oversized";
/// `string(taskfailure.ReasonAgentContextOverflow)`.
pub(crate) const FAILURE_REASON_AGENT_CONTEXT_OVERFLOW: &str = "agent_error.context_overflow";

/// `poisonedOutputMaxLen`: caps how long an output can be and still be
/// classified as a poisoned fallback. Intentionally errs on the side of NOT
/// classifying — a missed poisoned task gets retried by user action, but a
/// false-positive turns a successful task into a failure and a system comment.
const POISONED_OUTPUT_MAX_LEN: usize = 320;

/// `poisonedMarkers`: substring fingerprints of known agent fallback terminal
/// messages, matched case-insensitively.
const POISONED_MARKERS: &[(&str, &str)] = &[
    (
        "i reached the iteration limit",
        FAILURE_REASON_ITERATION_LIMIT,
    ),
    (
        "put your final update inside the content string",
        FAILURE_REASON_AGENT_FALLBACK_MSG,
    ),
];

// --- taskfailure.ContextExhaustedCompletion (context_exhausted.go:95–120) ---

/// `TerminalReasonPromptTooLong` (context_exhausted.go:47).
#[allow(dead_code)]
pub(crate) const TERMINAL_REASON_PROMPT_TOO_LONG: &str = "prompt_too_long";

/// `contextExhaustedOutputMaxLen`: same rationale and value as
/// poisonedOutputMaxLen — every wording below is a terse one-liner the CLI
/// emits INSTEAD of an answer.
const CONTEXT_EXHAUSTED_OUTPUT_MAX_LEN: usize = 320;

/// `ContextExhaustedCompletion` (context_exhausted.go:95): reports whether an
/// output reported as a SUCCESSFUL final answer is really the provider saying
/// the context window is full. EVERY clause is composite; the bare "Prompt is
/// too long" is deliberately NOT matched (a real agent result could be exactly
/// that sentence).
fn context_exhausted_completion(output: &str) -> bool {
    let trimmed = output.trim();
    if trimmed.is_empty() || trimmed.len() > CONTEXT_EXHAUSTED_OUTPUT_MAX_LEN {
        return false;
    }
    let lowered = trimmed.to_lowercase();
    (lowered.contains("prompt is too long") && lowered.contains("cannot be compacted"))
        || (lowered.contains("conversation too long") && lowered.contains("press esc twice"))
        || (lowered.contains("compaction failed")
            && lowered.contains("reduced below the context limit"))
}

// --- taskfailure.UnresumableHistory (resume.go:38–92) ----------------------

/// `emptyContentRe` (resume.go:80).
fn empty_content_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)must not be empty|must be non-?empty|must have non-?empty|non-?empty content|cannot be empty|should not be empty")
            .expect("valid regex")
    })
}

/// `historyMessageLocatorRe` (resume.go:91).
fn history_message_locator_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)role[^a-z0-9]{0,2}assistant|assistant message|message at position|messages\.[0-9]|messages\[[0-9]")
            .expect("valid regex")
    })
}

/// `UnresumableHistory` (resume.go:38): reports whether an agent error means
/// the conversation history itself can no longer be sent to the provider — an
/// empty message baked into the transcript replays the same rejection forever.
/// Deliberately provider-agnostic: both signals are required, which keeps the
/// predicate narrow.
fn unresumable_history(err_text: &str) -> bool {
    if err_text.is_empty() {
        return false;
    }
    empty_content_re().is_match(err_text) && history_message_locator_re().is_match(err_text)
}

// --- agent codex markers (codex.go:145–186) --------------------------------

/// `CodexSemanticInactivityMarker` (codex.go:145).
const CODEX_SEMANTIC_INACTIVITY_MARKER: &str = "codex semantic inactivity timeout";
/// `CodexFirstTurnNoProgressMarker` (codex.go:149).
const CODEX_FIRST_TURN_NO_PROGRESS_MARKER: &str = "codex app-server no progress timeout";
/// `codexResumeMarker` / `codexLineOverflowMarker` (codex.go:160–161).
const CODEX_RESUME_MARKER: &str = "thread/resume failed";
const CODEX_LINE_OVERFLOW_MARKER: &str = "token too long";

/// `CodexResumeOverflowError` (codex.go:179): both markers are required —
/// "thread/resume failed" alone covers ordinary rejections a plain retry
/// handles; "token too long" alone would also match overflows on unrelated
/// RPCs. Codex rollouts are append-only, so the session behind such a failure
/// is unusable for resume until it shrinks, which it never does (MUL-5722).
fn codex_resume_overflow_error(err_text: &str) -> bool {
    if err_text.is_empty() {
        return false;
    }
    let lower = err_text.to_lowercase();
    lower.contains(CODEX_RESUME_MARKER) && lower.contains(CODEX_LINE_OVERFLOW_MARKER)
}

/// `classifyPoisonedOutput` (poisoned.go:110): reports whether output matches a
/// known agent fallback terminal message or the provider's context-exhaustion
/// notice (GH #6402), and returns the failure_reason to persist. Long outputs
/// are never classified: a real fallback is the agent's only utterance for the
/// turn.
pub(crate) fn classify_poisoned_output(output: &str) -> Option<&'static str> {
    if context_exhausted_completion(output) {
        return Some(FAILURE_REASON_AGENT_CONTEXT_OVERFLOW);
    }
    let trimmed = output.trim();
    if trimmed.is_empty() || trimmed.len() > POISONED_OUTPUT_MAX_LEN {
        return None;
    }
    let lowered = trimmed.to_lowercase();
    for (substring, reason) in POISONED_MARKERS {
        if lowered.contains(substring) {
            return Some(reason);
        }
    }
    None
}

/// `classifyPoisonedError` (poisoned.go:167): reports whether an agent error
/// message indicates the LLM API itself rejected the request body — every
/// retry replays the same body and reproduces the same 400, so the session
/// must be excluded from resume lookup. Match shape notes live on each arm.
pub(crate) fn classify_poisoned_error(err_msg: &str) -> Option<&'static str> {
    if err_msg.is_empty() {
        return None;
    }
    let lowered = err_msg.to_lowercase();
    // Kiro/ACP replays oversized images baked into resumed history (GH #5975):
    // requiring BOTH the image-content marker and the dimension phrase keeps
    // this narrow.
    if lowered.contains("image dimensions exceed max allowed size")
        && lowered.contains("image.source.base64.data")
    {
        return Some(FAILURE_REASON_API_INVALID_REQUEST);
    }
    // The canonical Anthropic error shape: "400" alone is too generic and
    // "invalid_request_error" alone could appear in non-poisoning contexts;
    // the combination indicates the conversation history is the problem.
    if lowered.contains("invalid_request_error") && lowered.contains("400") {
        return Some(FAILURE_REASON_API_INVALID_REQUEST);
    }
    // The same defect worded differently by another provider (GH #6066,
    // GH #5760): recognise it by what the provider says is wrong rather than
    // by which provider said it.
    if unresumable_history(err_msg) {
        return Some(FAILURE_REASON_API_INVALID_REQUEST);
    }
    None
}

/// `classifyResumeUnsafeTransport` (poisoned.go:194): a Codex thread/resume
/// response too large to read back means the recorded session must not be
/// resumed again — the thread only grows, so every later resume overflows
/// identically (MUL-5722). Provider-specific on purpose: no other backend
/// replays its entire history through a single line.
pub(crate) fn classify_resume_unsafe_transport(
    provider: &str,
    err_msg: &str,
) -> Option<&'static str> {
    if provider.trim().to_lowercase() != "codex" {
        return None;
    }
    if codex_resume_overflow_error(err_msg) {
        return Some(FAILURE_REASON_CODEX_RESUME_OVERSIZED);
    }
    None
}

/// `classifyResumeUnsafeTimeout` (poisoned.go:212): whether a timeout means the
/// recorded session should not be resumed. Ordinary daemon/backend timeouts are
/// infrastructure-shaped and keep the resume pointer so retries continue the
/// in-flight conversation.
pub(crate) fn classify_resume_unsafe_timeout(
    provider: &str,
    err_msg: &str,
) -> Option<&'static str> {
    if provider.trim().to_lowercase() != "codex" || err_msg.is_empty() {
        return None;
    }
    let lowered = err_msg.to_lowercase();
    if lowered.contains(CODEX_SEMANTIC_INACTIVITY_MARKER)
        || lowered.contains(CODEX_FIRST_TURN_NO_PROGRESS_MARKER)
    {
        return Some(FAILURE_REASON_CODEX_SEMANTIC_INACTIVITY);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of poisoned_test.go / context_exhausted_test.go key cases.

    #[test]
    fn classify_output_iteration_limit_marker() {
        assert_eq!(
            classify_poisoned_output("I reached the iteration limit and stopped"),
            Some(FAILURE_REASON_ITERATION_LIMIT)
        );
    }

    #[test]
    fn classify_output_fallback_marker() {
        assert_eq!(
            classify_poisoned_output("PUT YOUR FINAL UPDATE INSIDE THE CONTENT STRING"),
            Some(FAILURE_REASON_AGENT_FALLBACK_MSG)
        );
    }

    #[test]
    fn classify_output_long_output_quoting_marker_not_classified() {
        // MUL-1630: a code-review reply quoting a marker is a real result.
        let long = format!(
            "reviewing: {} {}",
            "x".repeat(400),
            "i reached the iteration limit"
        );
        assert_eq!(classify_poisoned_output(&long), None);
    }

    #[test]
    fn classify_output_context_exhausted() {
        assert_eq!(
            classify_poisoned_output(
                "Prompt is too long · A single-exchange conversation cannot be compacted; start a new session."
            ),
            Some(FAILURE_REASON_AGENT_CONTEXT_OVERFLOW)
        );
        assert_eq!(
            classify_poisoned_output(
                "Conversation too long. Press esc twice to go up a few messages and try again."
            ),
            Some(FAILURE_REASON_AGENT_CONTEXT_OVERFLOW)
        );
    }

    #[test]
    fn classify_output_bare_prompt_too_long_not_matched() {
        // Deliberately NOT matched — an agent asked "is my prompt too long?"
        // can legitimately produce exactly this sentence.
        assert_eq!(classify_poisoned_output("Prompt is too long"), None);
    }

    #[test]
    fn classify_error_anthropic_shape() {
        let msg = r#"API Error: 400 {"error":{"type":"invalid_request_error"}}"#;
        assert_eq!(
            classify_poisoned_error(msg),
            Some(FAILURE_REASON_API_INVALID_REQUEST)
        );
    }

    #[test]
    fn classify_error_400_alone_too_generic() {
        assert_eq!(classify_poisoned_error("tool returned 400 records"), None);
    }

    #[test]
    fn classify_error_unresumable_history_provider_agnostic() {
        // GH #6066 wording — no "400"/"invalid_request_error" present.
        assert_eq!(
            classify_poisoned_error(
                "Invalid request: the message at position 37 with role 'assistant' must not be empty"
            ),
            Some(FAILURE_REASON_API_INVALID_REQUEST)
        );
        // No locator → not matched.
        assert_eq!(
            classify_poisoned_error("commit message must not be empty"),
            None
        );
        // Locator without emptiness complaint → not matched.
        assert_eq!(classify_poisoned_error("diff touches messages[3]"), None);
    }

    #[test]
    fn resume_unsafe_transport_codex_only() {
        assert_eq!(
            classify_resume_unsafe_transport("codex", "thread/resume failed: token too long"),
            Some(FAILURE_REASON_CODEX_RESUME_OVERSIZED)
        );
        assert_eq!(
            classify_resume_unsafe_transport("claude", "thread/resume failed: token too long"),
            None
        );
    }

    #[test]
    fn resume_unsafe_timeout_markers() {
        assert_eq!(
            classify_resume_unsafe_timeout(
                "codex",
                "stalled: Codex Semantic Inactivity Timeout after 600s"
            ),
            Some(FAILURE_REASON_CODEX_SEMANTIC_INACTIVITY)
        );
        assert_eq!(
            classify_resume_unsafe_timeout("codex", "codex app-server no progress timeout"),
            Some(FAILURE_REASON_CODEX_SEMANTIC_INACTIVITY)
        );
        // Ordinary timeouts keep the resume pointer.
        assert_eq!(
            classify_resume_unsafe_timeout("codex", "context deadline exceeded"),
            None
        );
        assert_eq!(
            classify_resume_unsafe_timeout("claude", "codex semantic inactivity timeout"),
            None
        );
    }
}
