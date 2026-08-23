//! Port of `server/internal/daemon/poisoned.go` (lines 1–217).
//!
//! Classifies "poisoned" sessions — resuming the same conversation would
//! deterministically reproduce the same failure — so the server-side
//! GetLastTaskSession query can filter them out and the next task starts
//! fresh. Four flavors: output-side fallback markers / context exhaustion,
//! error-side API invalid_request, timeout-side Codex semantic inactivity,
//! and transport-side Codex resume overflow.
//!
//! Deviations from Go:
//! - `pkg/taskfailure` and `pkg/agent` are not yet dependencies of this
//!   crate, so the four predicates/constants they contribute are ported
//!   locally below with their Go source cited.
//!   S9-integration: swap these for `cordy_task_failure::{…}` /
//!   agent-descriptor equivalents when the dependency lands.
//! - `service.ResumeUnsafeFailure` cross-package contract test
//!   (poisoned_test.go:331–336) is server-side and not portable here.

// S9-integration: consumed by daemon task-failure reporting wiring that lands
// with integration; silence dead-code until then.
#![allow(dead_code)]

use once_cell_free::Lazy;
use regex::Regex;

/// `FailureReasonIterationLimit` (poisoned.go:41) — aliased to the canonical
/// `taskfailure.ReasonIterationLimit` value ("iteration_limit").
pub(crate) const FAILURE_REASON_ITERATION_LIMIT: &str = "iteration_limit";
/// `FailureReasonAgentFallbackMsg` (poisoned.go:42).
pub(crate) const FAILURE_REASON_AGENT_FALLBACK_MSG: &str = "agent_fallback_message";
/// `FailureReasonAPIInvalidRequest` (poisoned.go:43) — canonical
/// `taskfailure.ReasonAPIInvalidRequest` value.
pub(crate) const FAILURE_REASON_API_INVALID_REQUEST: &str = "api_invalid_request";
/// `FailureReasonCodexSemanticInactivity` (poisoned.go:44).
pub(crate) const FAILURE_REASON_CODEX_SEMANTIC_INACTIVITY: &str = "codex_semantic_inactivity";
/// `FailureReasonCodexResumeOversized` (poisoned.go:45).
pub(crate) const FAILURE_REASON_CODEX_RESUME_OVERSIZED: &str = "codex_resume_oversized";

/// `taskfailure.ReasonAgentContextOverflow` string value.
const REASON_AGENT_CONTEXT_OVERFLOW: &str = "agent_error.context_overflow";

/// `poisonedOutputMaxLen` (poisoned.go:57): outputs longer than this are never
/// classified — a real fallback is one short sentence; a long output quoting a
/// marker is a real conclusion (MUL-1630). Byte length, matching Go's `len`.
const POISONED_OUTPUT_MAX_LEN: usize = 320;

/// `agent.CodexSemanticInactivityMarker` (pkg/agent/codex.go:145).
pub(crate) const CODEX_SEMANTIC_INACTIVITY_MARKER: &str = "codex semantic inactivity timeout";
/// `agent.CodexFirstTurnNoProgressMarker` (pkg/agent/codex.go:149).
pub(crate) const CODEX_FIRST_TURN_NO_PROGRESS_MARKER: &str = "codex app-server no progress timeout";

// ---------------------------------------------------------------------------
// Local ports of the pkg/taskfailure / pkg/agent predicates poisoned.go uses.
// ---------------------------------------------------------------------------

/// `taskfailure.ContextExhaustedCompletion`
/// (pkg/taskfailure/context_exhausted.go:95–113): an output reported as a
/// SUCCESSFUL answer that is really the provider's context-exhaustion notice
/// (GH #6402). Every clause is composite; the bare "Prompt is too long"
/// sentence is deliberately not matched. Byte-length cap 320 matches Go.
fn context_exhausted_completion(output: &str) -> bool {
    let trimmed = output.trim();
    if trimmed.is_empty() || trimmed.len() > 320 {
        return false;
    }
    let lowered = trimmed.to_lowercase();
    (lowered.contains("prompt is too long") && lowered.contains("cannot be compacted"))
        || (lowered.contains("conversation too long") && lowered.contains("press esc twice"))
        || (lowered.contains("compaction failed")
            && lowered.contains("reduced below the context limit"))
}

/// `taskfailure.UnresumableHistory` (pkg/taskfailure/resume.go:38–43): the
/// transcript carries empty content baked in by ANY backend (GH #6066,
/// GH #5760). Both signals required — an emptiness complaint without a
/// history locator is some tool's validation error.
fn unresumable_history(err_text: &str) -> bool {
    /// `emptyContentRe` (resume.go:80).
    static EMPTY_CONTENT_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)must not be empty|must be non-?empty|must have non-?empty|non-?empty content|cannot be empty|should not be empty")
            .expect("static regex")
    });
    /// `historyMessageLocatorRe` (resume.go:91).
    static HISTORY_MESSAGE_LOCATOR_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)role[^a-z0-9]{0,2}assistant|assistant message|message at position|messages\.[0-9]|messages\[[0-9]")
            .expect("static regex")
    });
    if err_text.is_empty() {
        return false;
    }
    EMPTY_CONTENT_RE.is_match(err_text) && HISTORY_MESSAGE_LOCATOR_RE.is_match(err_text)
}

/// `agent.CodexResumeOverflowError` (pkg/agent/codex.go:179–185): the
/// thread/resume response did not fit our stdout line buffer; both markers
/// required (MUL-5722).
fn codex_resume_overflow_error(err_text: &str) -> bool {
    const CODEX_RESUME_MARKER: &str = "thread/resume failed";
    const CODEX_LINE_OVERFLOW_MARKER: &str = "token too long";
    if err_text.is_empty() {
        return false;
    }
    let lower = err_text.to_lowercase();
    lower.contains(CODEX_RESUME_MARKER) && lower.contains(CODEX_LINE_OVERFLOW_MARKER)
}

// ---------------------------------------------------------------------------
// The classifiers themselves (poisoned.go:63–217).
// ---------------------------------------------------------------------------

/// `poisonedMarkers` (poisoned.go:63–69): substring fingerprints of known
/// agent fallback terminal messages → failure_reason. Case-insensitive,
/// substring-based.
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

/// `classifyPoisonedOutput` (poisoned.go:77–105): match a known agent fallback
/// terminal message or the provider's context-window notice; returns
/// `(failure_reason, matched)`. Long outputs are never classified.
pub(crate) fn classify_poisoned_output(output: &str) -> Option<&'static str> {
    // GH #6402: the "context window is full" notice arriving as the run's
    // successful answer. Tool count is intentionally NOT a condition here.
    if context_exhausted_completion(output) {
        return Some(REASON_AGENT_CONTEXT_OVERFLOW);
    }
    let trimmed = output.trim();
    if trimmed.is_empty() || trimmed.len() > POISONED_OUTPUT_MAX_LEN {
        return None;
    }
    let lowered = trimmed.to_lowercase();
    POISONED_MARKERS
        .iter()
        .find(|(substring, _)| lowered.contains(substring))
        .map(|(_, reason)| *reason)
}

/// `classifyPoisonedError` (poisoned.go:131–170): the LLM API rejected the
/// request body itself — every resume replays the same 400.
pub(crate) fn classify_poisoned_error(err_msg: &str) -> Option<&'static str> {
    if err_msg.is_empty() {
        return None;
    }
    let lowered = err_msg.to_lowercase();
    // Kiro/ACP oversized image replayed from resumed history (GH #5975):
    // dimension phrase AND image-content marker required to stay narrow.
    if lowered.contains("image dimensions exceed max allowed size")
        && lowered.contains("image.source.base64.data")
    {
        return Some(FAILURE_REASON_API_INVALID_REQUEST);
    }
    // Canonical Anthropic shape: both markers required — "400" alone is too
    // generic, "invalid_request_error" alone could appear elsewhere.
    if lowered.contains("invalid_request_error") && lowered.contains("400") {
        return Some(FAILURE_REASON_API_INVALID_REQUEST);
    }
    // Same defect worded differently by another provider (GH #6066, #5760).
    if unresumable_history(err_msg) {
        return Some(FAILURE_REASON_API_INVALID_REQUEST);
    }
    None
}

/// `classifyResumeUnsafeTransport` (poisoned.go:193–201): transport-level
/// failures meaning the recorded session must not be resumed again. Today the
/// only case is a Codex resume whose response overflowed the line buffer.
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

/// `classifyResumeUnsafeTimeout` (poisoned.go:207–217): timeouts meaning the
/// recorded session should not be resumed. Deliberately provider-specific —
/// ordinary daemon/backend timeouts are infrastructure-shaped and keep the
/// resume pointer.
pub(crate) fn classify_resume_unsafe_timeout(
    provider: &str,
    err_msg: &str,
) -> Option<&'static str> {
    if provider.trim().to_lowercase() != "codex" || err_msg.is_empty() {
        return None;
    }
    let lowered = err_msg.to_lowercase();
    if lowered.contains(&CODEX_SEMANTIC_INACTIVITY_MARKER.to_lowercase())
        || lowered.contains(&CODEX_FIRST_TURN_NO_PROGRESS_MARKER.to_lowercase())
    {
        return Some(FAILURE_REASON_CODEX_SEMANTIC_INACTIVITY);
    }
    None
}

/// Minimal `once_cell::sync::Lazy` stand-in (no once_cell dependency): a
/// std-sync Mutex-free lazy initialized on first deref via OnceLock.
mod once_cell_free {
    use std::sync::OnceLock;

    pub(crate) struct Lazy<T> {
        cell: OnceLock<T>,
        init: fn() -> T,
    }

    impl<T> Lazy<T> {
        pub(crate) const fn new(init: fn() -> T) -> Self {
            Self {
                cell: OnceLock::new(),
                init,
            }
        }
    }

    impl<T> std::ops::Deref for Lazy<T> {
        type Target = T;
        fn deref(&self) -> &T {
            self.cell.get_or_init(self.init)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TestClassifyPoisonedOutput table (poisoned_test.go:12–121).
    #[test]
    fn classify_poisoned_output_table() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            (
                "iteration limit canonical",
                "I reached the iteration limit and couldn't generate a summary.",
                Some(FAILURE_REASON_ITERATION_LIMIT),
            ),
            (
                "iteration limit case insensitive",
                "I REACHED THE ITERATION LIMIT and stopped",
                Some(FAILURE_REASON_ITERATION_LIMIT),
            ),
            (
                "fallback meta message",
                "Put your final update inside the content string. Keep it concise.",
                Some(FAILURE_REASON_AGENT_FALLBACK_MSG),
            ),
            (
                "real conclusion is not poisoned",
                "Fixed the bug in auth.go and pushed PR #42.",
                None,
            ),
            ("empty output", "", None),
            (
                "mentions iteration but not the marker",
                "Each iteration of the loop processes one record.",
                None,
            ),
            (
                "long review quoting both markers is not poisoned",
                "Review for the rerun fix.\n\nDetection markers under consideration:\n- \"I reached the iteration limit and couldn't generate a summary.\"\n- \"Put your final update inside the content string. Keep it concise.\"\n\nThe implementation looks correct: the daemon classifies these as\nfallback output, persists a dedicated failure_reason, and the SQL\nfilter excludes them from the resume lookup. Resume-safe auto-retry\nstill keeps the resume contract, while poisoned sessions are filtered.\nApproving with a follow-up note about the matcher being too permissive\non long outputs.",
                None,
            ),
            (
                "marker buried inside a long agent conclusion",
                &(format!(
                    "{}{}",
                    "All checks passed and the bug is fixed. ".repeat(10),
                    "i reached the iteration limit while debugging earlier."
                )),
                None,
            ),
            (
                "context exhaustion with the provider's full wording",
                "Prompt is too long · the request is ~274931 tokens (limit 200000) but this conversation is only ~1597 tokens — the rest is system prompt, tool definitions, and attachment content. A single-exchange conversation cannot be compacted; reduce attached files/tools or start with less context.",
                Some(REASON_AGENT_CONTEXT_OVERFLOW),
            ),
            (
                "compaction exhausted",
                "Compaction failed · conversation could not be reduced below the context limit",
                Some(REASON_AGENT_CONTEXT_OVERFLOW),
            ),
            (
                "an agent discussing /compact is a real answer",
                "The session is getting long; run /compact before the next batch.",
                None,
            ),
            (
                "bare provider sentence is left to the failure path",
                "Prompt is too long",
                None,
            ),
        ];
        for (name, output, want) in cases {
            assert_eq!(classify_poisoned_output(output), *want, "case {name:?}");
        }
    }

    /// TestClassifyPoisonedError table (poisoned_test.go:123–260).
    #[test]
    fn classify_poisoned_error_table() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            (
                "claude could not process image",
                r#"API Error: 400 {"type":"error","error":{"type":"invalid_request_error","message":"Could not process image"},"request_id":"req_011CarVEtBLj95zD7i8xardY"}"#,
                Some(FAILURE_REASON_API_INVALID_REQUEST),
            ),
            (
                "prompt too long is also poisoning",
                r#"API Error: 400 {"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 213000 tokens > 200000 maximum"}}"#,
                Some(FAILURE_REASON_API_INVALID_REQUEST),
            ),
            (
                "case insensitive",
                r#"api error: 400 {"type":"INVALID_REQUEST_ERROR"}"#,
                Some(FAILURE_REASON_API_INVALID_REQUEST),
            ),
            (
                "429 rate limit is transient",
                r#"API Error: 429 {"type":"error","error":{"type":"rate_limit_error","message":"Number of request tokens has exceeded your per-minute rate limit"}}"#,
                None,
            ),
            (
                "5xx overloaded is transient",
                r#"API Error: 529 {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
                None,
            ),
            (
                "401 auth error",
                r#"API Error: 401 {"type":"error","error":{"type":"authentication_error","message":"invalid api key"}}"#,
                None,
            ),
            (
                "tool 400 without invalid_request_error",
                r#"agent tool returned status 400: not found"#,
                None,
            ),
            ("empty error message", "", None),
            (
                "unrelated execution error",
                "claude execution timeout after 10m",
                None,
            ),
            (
                "gh6066 empty assistant message in history",
                "Invalid request: the message at position 37 with role 'assistant' must not be empty",
                Some(FAILURE_REASON_API_INVALID_REQUEST),
            ),
            (
                "gh5760 kimi empty assistant message",
                "kimi provider error: provider.api_error: 400 the message at position 43 with role 'assistant' must not be empty",
                Some(FAILURE_REASON_API_INVALID_REQUEST),
            ),
            (
                "tool validation emptiness is not poisoning",
                "validation error: field must not be empty",
                None,
            ),
            (
                "kiro oversized history image",
                r#"kiro session/prompt failed: session/prompt: Internal error (code=-32603, data=Encountered an error in the response stream: messages.14.content.0.image.source.base64.data: At least one of the image dimensions exceed max allowed size: 8000 pixels)"#,
                Some(FAILURE_REASON_API_INVALID_REQUEST),
            ),
            (
                "plain kiro internal error is not poisoning",
                r#"kiro session/prompt failed: session/prompt: Internal error (code=-32603, data=Kiro failed to generate a response)"#,
                None,
            ),
            (
                "dimension phrase without image-content marker",
                r#"some tool reported: image dimensions exceed max allowed size: 8000 pixels"#,
                None,
            ),
        ];
        for (name, err_msg, want) in cases {
            assert_eq!(classify_poisoned_error(err_msg), *want, "case {name:?}");
        }
    }

    /// TestClassifyResumeUnsafeTransport table (poisoned_test.go:262–324).
    #[test]
    fn classify_resume_unsafe_transport_table() {
        let overflow_err =
            "codex thread/resume failed: codex process exited: bufio.Scanner: token too long";
        let cases: &[(&str, &str, &str, Option<&str>)] = &[
            (
                "codex resume overflow",
                "codex",
                overflow_err,
                Some(FAILURE_REASON_CODEX_RESUME_OVERSIZED),
            ),
            (
                "overflow outside a resume stays resumable",
                "codex",
                "codex thread/start failed: codex process exited: bufio.Scanner: token too long",
                None,
            ),
            (
                "plain resume failure stays resumable",
                "codex",
                "codex thread/resume failed: thread not found",
                None,
            ),
            (
                "other provider same text is not classified",
                "claude",
                overflow_err,
                None,
            ),
            ("empty error", "codex", "", None),
        ];
        for (name, provider, err_msg, want) in cases {
            assert_eq!(
                classify_resume_unsafe_transport(provider, err_msg),
                *want,
                "case {name:?}"
            );
        }
    }

    /// TestClassifyResumeUnsafeTimeout table (poisoned_test.go:338–391).
    #[test]
    fn classify_resume_unsafe_timeout_table() {
        let cases: &[(&str, &str, &str, Option<&str>)] = &[
            (
                "codex semantic inactivity",
                "codex",
                &(CODEX_SEMANTIC_INACTIVITY_MARKER.to_string()
                    + " after 10m0s without agent progress (last activity: tool-result:exec_command)"),
                Some(FAILURE_REASON_CODEX_SEMANTIC_INACTIVITY),
            ),
            (
                "codex first turn no progress",
                "codex",
                &(CODEX_FIRST_TURN_NO_PROGRESS_MARKER.to_string()
                    + " after 30s: received turn start but no item, turn/completed, or error event"),
                Some(FAILURE_REASON_CODEX_SEMANTIC_INACTIVITY),
            ),
            (
                "codex ordinary timeout remains resumable",
                "codex",
                "codex timed out after 30m0s",
                None,
            ),
            (
                "other provider same text is not classified",
                "claude",
                &(CODEX_SEMANTIC_INACTIVITY_MARKER.to_string()
                    + " after 10m0s without agent progress"),
                None,
            ),
            ("empty error", "codex", "", None),
        ];
        for (name, provider, err_msg, want) in cases {
            assert_eq!(
                classify_resume_unsafe_timeout(provider, err_msg),
                *want,
                "case {name:?}"
            );
        }
    }
}
