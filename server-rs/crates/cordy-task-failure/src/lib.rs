//! Canonical failure-reason taxonomy for `agent_task_queue.failure_reason`
//! and `chat_message.failure_reason`.
//!
//! Two groups: platform-side values (no `agent_error.` prefix, written by
//! server sweepers / daemon classifiers) and 14 agent-side sub-reasons
//! produced by [`classify`] from raw agent error text. Wire stability: these
//! strings are persisted and surfaced as Prometheus labels.
//!
//! [`Reason`] is an OPEN set on purpose: [`normalize_daemon_reason`] passes
//! through arbitrary legacy values (e.g. the pre-MUL-1949 coarse
//! `"agent_error"`), so a closed enum would be unfaithful.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

/// Marks the sub-reasons that originate inside the agent process as opposed
/// to the platform-side reasons. Classification is a string PREFIX check so
/// any future `agent_error.*` value inherits the grouping automatically.
const AGENT_ERROR_PREFIX: &str = "agent_error.";

/// String-backed enum of canonical failure reasons. Construct only from the
/// associated constants or via `From<&str>`/`From<String>` passthrough (the
/// latter exists for daemon-reported legacy values).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Reason(Cow<'static, str>);

impl Reason {
    pub const QUEUED_EXPIRED: Reason = Reason(Cow::Borrowed("queued_expired"));
    pub const RUNTIME_OFFLINE: Reason = Reason(Cow::Borrowed("runtime_offline"));
    pub const RUNTIME_RECONNECT_TIMEOUT: Reason =
        Reason(Cow::Borrowed("runtime_reconnect_timeout"));
    pub const RUNTIME_RECOVERY: Reason = Reason(Cow::Borrowed("runtime_recovery"));
    pub const TIMEOUT: Reason = Reason(Cow::Borrowed("timeout"));
    pub const ITERATION_LIMIT: Reason = Reason(Cow::Borrowed("iteration_limit"));
    pub const AGENT_BLOCKED: Reason = Reason(Cow::Borrowed("agent_blocked"));
    pub const API_INVALID_REQUEST: Reason = Reason(Cow::Borrowed("api_invalid_request"));
    pub const SKILL_BUNDLE_UNAVAILABLE: Reason = Reason(Cow::Borrowed("skill_bundle_unavailable"));
    pub const RUNTIME_CLI_TIMEOUT: Reason = Reason(Cow::Borrowed("runtime_cli_timeout"));
    pub const INVALID_TASK_IDENTITY: Reason = Reason(Cow::Borrowed("invalid_task_identity"));

    pub const AGENT_PROVIDER_AUTH_OR_ACCESS: Reason =
        Reason(Cow::Borrowed("agent_error.provider_auth_or_access"));
    pub const AGENT_PROVIDER_QUOTA_LIMIT: Reason =
        Reason(Cow::Borrowed("agent_error.provider_quota_limit"));
    pub const AGENT_PROVIDER_CAPACITY_OR_RATE_LIMIT: Reason =
        Reason(Cow::Borrowed("agent_error.provider_capacity_or_rate_limit"));
    pub const AGENT_PROVIDER_SERVER_ERROR: Reason =
        Reason(Cow::Borrowed("agent_error.provider_server_error"));
    pub const AGENT_PROVIDER_NETWORK: Reason =
        Reason(Cow::Borrowed("agent_error.provider_network"));
    pub const AGENT_PROCESS_FAILURE: Reason = Reason(Cow::Borrowed("agent_error.process_failure"));
    pub const AGENT_EMPTY_OR_UNPARSEABLE_OUTPUT: Reason =
        Reason(Cow::Borrowed("agent_error.empty_or_unparseable_output"));
    pub const AGENT_TIMEOUT: Reason = Reason(Cow::Borrowed("agent_error.agent_timeout"));
    pub const AGENT_CONTEXT_OVERFLOW: Reason =
        Reason(Cow::Borrowed("agent_error.context_overflow"));
    pub const AGENT_MISSING_CONFIG: Reason = Reason(Cow::Borrowed("agent_error.missing_config"));
    pub const AGENT_MODEL_NOT_FOUND_OR_UNAVAILABLE: Reason =
        Reason(Cow::Borrowed("agent_error.model_not_found_or_unavailable"));
    pub const AGENT_RUNTIME_VERSION_UNSUPPORTED: Reason =
        Reason(Cow::Borrowed("agent_error.runtime_version_unsupported"));
    pub const AGENT_RUNTIME_MISSING_EXECUTABLE: Reason =
        Reason(Cow::Borrowed("agent_error.runtime_missing_executable"));
    pub const AGENT_UNKNOWN: Reason = Reason(Cow::Borrowed("agent_error.unknown"));

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True when the reason originates inside the agent process rather than
    /// the platform/scheduler/runtime layer.
    pub fn is_agent_error(&self) -> bool {
        self.0.starts_with(AGENT_ERROR_PREFIX)
    }
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Reason {
    fn from(s: &str) -> Self {
        Reason(Cow::Owned(s.to_string()))
    }
}

impl From<String> for Reason {
    fn from(s: String) -> Self {
        Reason(Cow::Owned(s))
    }
}

/// Canonical reasons in stable order (platform lifecycle order, then
/// agent-side grouped by responsibility area, unknown last) so Prometheus
/// label sets stay deterministic across restarts.
pub fn all_reasons() -> Vec<Reason> {
    vec![
        Reason::QUEUED_EXPIRED,
        Reason::RUNTIME_OFFLINE,
        Reason::RUNTIME_RECONNECT_TIMEOUT,
        Reason::RUNTIME_RECOVERY,
        Reason::TIMEOUT,
        Reason::ITERATION_LIMIT,
        Reason::AGENT_BLOCKED,
        Reason::API_INVALID_REQUEST,
        Reason::SKILL_BUNDLE_UNAVAILABLE,
        Reason::RUNTIME_CLI_TIMEOUT,
        Reason::INVALID_TASK_IDENTITY,
        Reason::AGENT_PROVIDER_AUTH_OR_ACCESS,
        Reason::AGENT_PROVIDER_QUOTA_LIMIT,
        Reason::AGENT_PROVIDER_CAPACITY_OR_RATE_LIMIT,
        Reason::AGENT_PROVIDER_SERVER_ERROR,
        Reason::AGENT_PROVIDER_NETWORK,
        Reason::AGENT_PROCESS_FAILURE,
        Reason::AGENT_EMPTY_OR_UNPARSEABLE_OUTPUT,
        Reason::AGENT_TIMEOUT,
        Reason::AGENT_CONTEXT_OVERFLOW,
        Reason::AGENT_MISSING_CONFIG,
        Reason::AGENT_MODEL_NOT_FOUND_OR_UNAVAILABLE,
        Reason::AGENT_RUNTIME_VERSION_UNSUPPORTED,
        Reason::AGENT_RUNTIME_MISSING_EXECUTABLE,
        Reason::AGENT_UNKNOWN,
    ]
}

// --- HTTP status-code regexes -------------------------------------------
//
// Digit-boundary guards keep bare substrings like "402" inside "402913
// tokens" or "exit status 4030" from misclassifying process/unknown failures
// as provider billing/rate-limit errors. Mirrors the SQL regexes from
// MUL-1949.

static PROVIDER_HTTP_5XX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(^|[^0-9])5[0-9][0-9]([^0-9]|$)"#).unwrap());
static HTTP_AUTH_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(^|[^0-9])(401|403)([^0-9]|$)"#).unwrap());
static HTTP_QUOTA_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(^|[^0-9])402([^0-9]|$)"#).unwrap());
static HTTP_CAPACITY_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(^|[^0-9])(429|529)([^0-9]|$)"#).unwrap());

/// The runtime's wording for "I resolved no LLM provider at all" —
/// structurally a configuration gap, not a rejected credential. Single
/// source of truth shared by classify rule 2 and [`provider_unconfigured`].
const PROVIDER_UNCONFIGURED_PHRASE: &str = "no llm provider configured";

/// Prefix of every failure the OpenCode terminal-signal guard raises. A
/// PREFIX of the whole error, not a phrase inside it, so its presence
/// identifies the failure outright.
const OPENCODE_STREAM_ENDED_PREFIX: &str = "opencode stream ended";

/// Wordings for an overflow reported on the RESPONSE (stop_reason
/// model_context_window_exceeded surfaced verbatim by Claude Code 2.1.x).
/// Each is an unambiguous witness on its own, which lets
/// [`normalize_daemon_reason`] reuse them to upgrade an older daemon's
/// catchall server-side.
const CONTEXT_WINDOW_EXCEEDED_WITNESSES: &[&str] = &[
    "context window limit",
    "model_context_window_exceeded",
    TERMINAL_REASON_PROMPT_TOO_LONG,
];

/// Value Claude Code writes into the stream-json result event's
/// `terminal_reason` when the turn ended because the request no longer fits
/// the model's context window. Structured enum value, not prose.
pub const TERMINAL_REASON_PROMPT_TOO_LONG: &str = "prompt_too_long";

fn contains_any(s: &str, subs: &[&str]) -> bool {
    subs.iter().any(|sub| s.contains(sub))
}

fn contains_all(s: &str, subs: &[&str]) -> bool {
    !subs.is_empty() && subs.iter().all(|sub| s.contains(sub))
}

/// Maps a free-form error string from the agent runtime / CLI to one of the
/// 14 `agent_error.*` sub-reasons. Always returns a valid reason;
/// [`Reason::AGENT_UNKNOWN`] when no rule matches and for empty input.
///
/// The rule order mirrors the SQL CASE expression from MUL-1949 — the SQL is
/// the source of truth, and keeping them in lock-step is required so
/// in-flight rows and historically backfilled rows share one taxonomy.
/// Matching is case-insensitive substring against the lowercased input;
/// more-specific rules come before more-generic ones.
pub fn classify(raw_error: &str) -> Reason {
    let trimmed = raw_error.trim();
    if trimmed.is_empty() {
        return Reason::AGENT_UNKNOWN;
    }
    let lower = trimmed.to_lowercase();

    // 1. Context / token window overflow — before quota so "token limit"
    //    isn't swallowed by the broader "limit" rule.
    if contains_any(
        &lower,
        &[
            "context length",
            "context_length_exceeded",
            "maximum context",
            "prompt is too long",
            "context size has been exceeded",
        ],
    ) || contains_any(&lower, CONTEXT_WINDOW_EXCEEDED_WITNESSES)
        || (lower.contains("token") && lower.contains("limit"))
    {
        return Reason::AGENT_CONTEXT_OVERFLOW;
    }

    // 2. Missing config / API key — before auth: "missing API key" overlaps
    //    with "invalid api key" wording but is structural config, not auth.
    if lower.contains("missing environment variable")
        || (lower.contains("missing") && lower.contains("api_key"))
        || (lower.contains("api key") && lower.contains("required"))
        || lower.contains(PROVIDER_UNCONFIGURED_PHRASE)
        || lower.contains("no provider configured")
    {
        return Reason::AGENT_MISSING_CONFIG;
    }

    // 3. Auth / access. Status codes use a digit boundary so "4030" /
    //    "1401ms" don't land here.
    if HTTP_AUTH_CODE_RE.is_match(&lower)
        || contains_any(
            &lower,
            &[
                "unauthorized",
                "login required",
                "not logged in",
                "please login again",
                "refresh token",
                "invalid api key",
                "access token",
                "subscription access",
                "does not have access",
                "you may not have access",
            ],
        )
    {
        return Reason::AGENT_PROVIDER_AUTH_OR_ACCESS;
    }

    // 4. Quota / billing.
    if HTTP_QUOTA_CODE_RE.is_match(&lower)
        || contains_any(
            &lower,
            &[
                "insufficient_balance",
                "balance is too low",
                "monthly usage limit",
                "usage limit",
                "you've hit your limit",
                // Curly apostrophe variant: providers and copy-pasted error
                // strings sometimes use U+2019 instead of ASCII '.
                "you\u{2019}ve hit your limit",
                "credits",
                "quota",
            ],
        )
    {
        return Reason::AGENT_PROVIDER_QUOTA_LIMIT;
    }

    // 5. Capacity / rate limit.
    if HTTP_CAPACITY_CODE_RE.is_match(&lower)
        || contains_any(
            &lower,
            &["rate limit", "overloaded", "no capacity available"],
        )
    {
        return Reason::AGENT_PROVIDER_CAPACITY_OR_RATE_LIMIT;
    }

    // 6. Provider 5xx / server error — the anchored regex, not plain
    //    substring matches.
    if contains_any(
        &lower,
        &[
            "server had an error",
            "provider returned error",
            "internal error",
            "service unavailable",
            "bad gateway",
        ],
    ) || PROVIDER_HTTP_5XX_RE.is_match(&lower)
    {
        return Reason::AGENT_PROVIDER_SERVER_ERROR;
    }

    // 7. Provider network. Checked before process-failure so the
    //    "... exited with error: exit status N ..." variant still routes
    //    here; "deadline exceeded" covers Go-side context deadlines that
    //    reach the classifier as text.
    if contains_any(
        &lower,
        &[
            "stream disconnected",
            OPENCODE_STREAM_ENDED_PREFIX,
            "connection closed",
            "connection reset",
            "mid-response",
            "error sending request",
            "unable to connect",
            "dial tcp",
            "connection refused",
            "connectionrefused",
            "dns",
            "i/o timeout",
            "deadline exceeded",
            "timeout exceeded while awaiting",
        ],
    ) {
        return Reason::AGENT_PROVIDER_NETWORK;
    }

    // 8. Model not found / unavailable — both substrings present
    //    approximates the SQL `%model%not%found%`.
    if (lower.contains("model") && lower.contains("not found"))
        || contains_any(
            &lower,
            &[
                "unknown model",
                "selected model",
                "http 404",
                "404 page not found",
            ],
        )
    {
        return Reason::AGENT_MODEL_NOT_FOUND_OR_UNAVAILABLE;
    }

    // 9. Empty / unparseable output from the agent CLI itself.
    if contains_any(
        &lower,
        &["returned empty output", "returned no parseable output"],
    ) {
        return Reason::AGENT_EMPTY_OR_UNPARSEABLE_OUTPUT;
    }

    // 10. Agent subprocess hard timeout (per-task wall clock).
    if lower.contains("timed out after") {
        return Reason::AGENT_TIMEOUT;
    }

    // 11. Runner CLI binary missing, or present but not runnable (npm
    //     placeholder stub that only fails at execve).
    if contains_any(&lower, &["executable not found", "exec format error"]) {
        return Reason::AGENT_RUNTIME_MISSING_EXECUTABLE;
    }

    // 12. Runner CLI version too old / incompatible protocol.
    if contains_any(
        &lower,
        &[
            "below the minimum supported version",
            "requires a newer version",
        ],
    ) {
        return Reason::AGENT_RUNTIME_VERSION_UNSUPPORTED;
    }

    // 13. Agent / runner process-level failure — last among specific rules
    //     because "exit status"/"signal" co-occur with more specific
    //     upstream errors that SHOULD win.
    if contains_any(
        &lower,
        &[
            "exit status",
            "signal",
            "panic",
            "sigsegv",
            "process exited",
            "start codex:",
            "pipe has been ended",
            "file already closed",
            "initialize failed",
        ],
    ) {
        return Reason::AGENT_PROCESS_FAILURE;
    }

    Reason::AGENT_UNKNOWN
}

// --- Legacy-daemon upgrade tables ---------------------------------------
//
// Installed daemons upgrade on their own cadence, so a fix that only labels
// correctly on the daemon side reaches nobody until every host updates.
// Each rule below recognises the wire shape an old daemon produces and can
// be deleted once no such daemon is still reporting.

const LEGACY_SKILL_BUNDLE_PREFIX: &str = "resolve skill bundles:";

fn legacy_skill_bundle_reasons(reason: &str) -> bool {
    matches!(
        reason,
        "agent_error.unknown" | "agent_error.provider_network" | "agent_error"
    )
}

fn legacy_context_overflow_reasons(reason: &str) -> bool {
    // Deliberately narrower than the skill-bundle set: a refined reason means
    // the old daemon matched an earlier rule on the same text, which says
    // more about what ended the run than a witness appearing in the blob.
    matches!(reason, "agent_error.unknown" | "agent_error")
}

fn legacy_opencode_stream_ended_reasons(reason: &str) -> bool {
    matches!(
        reason,
        "agent_error.process_failure" | "agent_error.unknown" | "agent_error"
    )
}

fn legacy_openclaw_cli_timeout_reasons(reason: &str) -> bool {
    matches!(
        reason,
        "agent_error.unknown" | "agent_error.provider_network" | "agent_error"
    )
}

const LEGACY_OPENCLAW_CLI_TIMEOUT_WITNESSES: &[&str] =
    &["prepare openclaw config", "deadline exceeded"];

/// Upgrades a failure_reason reported by an older daemon onto the taxonomy
/// this server understands, using the raw error text as the witness.
/// Returns the reason unchanged when nothing applies.
pub fn normalize_daemon_reason(reason: &str, raw_error: &str) -> Reason {
    if legacy_skill_bundle_reasons(reason)
        && raw_error.trim().starts_with(LEGACY_SKILL_BUNDLE_PREFIX)
    {
        return Reason::SKILL_BUNDLE_UNAVAILABLE;
    }
    if legacy_context_overflow_reasons(reason)
        && contains_any(&raw_error.to_lowercase(), CONTEXT_WINDOW_EXCEEDED_WITNESSES)
    {
        return Reason::AGENT_CONTEXT_OVERFLOW;
    }
    if legacy_opencode_stream_ended_reasons(reason)
        && raw_error
            .trim()
            .to_lowercase()
            .starts_with(OPENCODE_STREAM_ENDED_PREFIX)
    {
        return Reason::AGENT_PROVIDER_NETWORK;
    }
    if legacy_openclaw_cli_timeout_reasons(reason)
        && contains_all(
            &raw_error.to_lowercase(),
            LEGACY_OPENCLAW_CLI_TIMEOUT_WITNESSES,
        )
    {
        return Reason::RUNTIME_CLI_TIMEOUT;
    }
    Reason::from(reason)
}

// --- Resume guards -------------------------------------------------------

static EMPTY_CONTENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)must not be empty|must be non-?empty|must have non-?empty|non-?empty content|cannot be empty|should not be empty"#)
        .unwrap()
});
static HISTORY_MESSAGE_LOCATOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)role[^a-z0-9]{0,2}assistant|assistant message|message at position|messages\.[0-9]|messages\[[0-9]"#,
    )
    .unwrap()
});

/// Lowercase provider phrase [`auth_method_unresolved`] matches. Single
/// source of truth — kept in sync with the GetLastTaskSession /
/// GetLastChatTaskSession resume queries, which apply the same guard
/// server-side.
const AUTH_METHOD_UNRESOLVED_PHRASE: &str = "could not resolve authentication method";

/// Reports whether an agent error means the conversation history itself can
/// no longer be sent to the provider: some message baked into the transcript
/// carries empty content, so every resume replays the same rejection.
///
/// Deliberately provider-agnostic and deliberately narrow: BOTH signals are
/// required (an emptiness complaint AND a message locator), so a tool
/// reporting "commit message must not be empty" does not match. Erring
/// toward NOT matching is safe — a miss leaves today's behaviour, while a
/// false positive discards a healthy session pointer.
pub fn unresumable_history(err_text: &str) -> bool {
    if err_text.is_empty() {
        return false;
    }
    EMPTY_CONTENT_RE.is_match(err_text) && HISTORY_MESSAGE_LOCATOR_RE.is_match(err_text)
}

/// Reports whether an agent error is the provider SDK refusing to resolve
/// its own credentials. On a RESUMED session this is deterministic rather
/// than transient, so the retry must start fresh. Keyed on the exact text
/// rather than the reason because classify leaves this shape as
/// agent_error.unknown (resume-safe).
pub fn auth_method_unresolved(err_text: &str) -> bool {
    !err_text.is_empty()
        && err_text
            .to_lowercase()
            .contains(AUTH_METHOD_UNRESOLVED_PHRASE)
}

/// Reports whether an agent error is the runtime refusing to start because
/// it resolved no provider whatsoever. Strictly "the runtime resolved no
/// provider" — an expired key, a 401, or a mistyped provider must NOT be
/// widened into this.
pub fn provider_unconfigured(err_text: &str) -> bool {
    !err_text.is_empty()
        && err_text
            .to_lowercase()
            .contains(PROVIDER_UNCONFIGURED_PHRASE)
}

// --- Context-exhausted success detection ---------------------------------

/// Caps how long a reported-successful output can be and still be re-read as
/// a context-exhaustion notice: every wording below is a terse one-liner the
/// CLI emits INSTEAD of an answer. Same value as poisonedOutputMaxLen in
/// internal/daemon.
const CONTEXT_EXHAUSTED_OUTPUT_MAX_LEN: usize = 320;

/// Reports whether an output the agent runtime reported as a SUCCESSFUL final
/// answer is really the provider saying the session's context window is full.
/// Text-side counterpart to [`TERMINAL_REASON_PROMPT_TOO_LONG`], for backends
/// without the structured field and for daemons too old to carry it.
///
/// EVERY clause is composite and pinned to full distinctive wordings from the
/// Claude Code binary. The CLI's bare "Prompt is too long" is deliberately
/// NOT matched: it is a common English sentence an agent can legitimately
/// produce, and this predicate only ever sees output a caller believed was a
/// success — a match costs a real task and a healthy session.
pub fn context_exhausted_completion(output: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_reasons_order_is_stable_and_complete() {
        let reasons = all_reasons();
        assert_eq!(reasons.len(), 25);
        assert_eq!(reasons[0], Reason::QUEUED_EXPIRED);
        assert_eq!(reasons.last(), Some(&Reason::AGENT_UNKNOWN));
        // Platform block first, then agent block.
        assert!(reasons[..11].iter().all(|r| !r.is_agent_error()));
        assert!(reasons[11..].iter().all(|r| r.is_agent_error()));
    }

    #[test]
    fn wire_values_are_byte_stable() {
        assert_eq!(
            Reason::AGENT_PROVIDER_NETWORK.as_str(),
            "agent_error.provider_network"
        );
        assert_eq!(Reason::RUNTIME_CLI_TIMEOUT.as_str(), "runtime_cli_timeout");
    }

    #[test]
    fn classify_empty_lands_in_unknown() {
        assert_eq!(classify(""), Reason::AGENT_UNKNOWN);
        assert_eq!(classify("   \n "), Reason::AGENT_UNKNOWN);
    }

    #[test]
    fn classify_rule_order_specific_before_generic() {
        // "token limit" must beat the quota bucket's "limit".
        assert_eq!(
            classify("Token limit reached"),
            Reason::AGENT_CONTEXT_OVERFLOW
        );
        // Rate-limited crash beats process_failure's "exit status".
        assert_eq!(
            classify("exit status 1: 429 rate limit exceeded"),
            Reason::AGENT_PROVIDER_CAPACITY_OR_RATE_LIMIT
        );
        // Stream cut beats "signal" in process_failure.
        assert_eq!(
            classify("opencode stream ended: terminal signal, exit status 0"),
            Reason::AGENT_PROVIDER_NETWORK
        );
    }

    #[test]
    fn classify_each_bucket() {
        assert_eq!(
            classify("prompt is too long: 200000 tokens > 128000 maximum"),
            Reason::AGENT_CONTEXT_OVERFLOW
        );
        assert_eq!(
            classify("Missing environment variable ANTHROPIC_API_KEY"),
            Reason::AGENT_MISSING_CONFIG
        );
        assert_eq!(
            classify("No LLM provider configured."),
            Reason::AGENT_MISSING_CONFIG
        );
        assert_eq!(
            classify("Unauthorized: invalid api key"),
            Reason::AGENT_PROVIDER_AUTH_OR_ACCESS
        );
        assert_eq!(
            classify("HTTP 401 Forbidden"),
            Reason::AGENT_PROVIDER_AUTH_OR_ACCESS
        );
        assert_eq!(
            classify("insufficient_balance: credits exhausted"),
            Reason::AGENT_PROVIDER_QUOTA_LIMIT
        );
        assert_eq!(
            classify("429 rate limited, slow down"),
            Reason::AGENT_PROVIDER_CAPACITY_OR_RATE_LIMIT
        );
        assert_eq!(
            classify("500 Internal Server Error"),
            Reason::AGENT_PROVIDER_SERVER_ERROR
        );
        assert_eq!(
            classify("API Error: Connection closed mid-response."),
            Reason::AGENT_PROVIDER_NETWORK
        );
        assert_eq!(
            classify("dial tcp 1.2.3.4:443: connection refused"),
            Reason::AGENT_PROVIDER_NETWORK
        );
        assert_eq!(
            classify("context deadline exceeded"),
            Reason::AGENT_PROVIDER_NETWORK
        );
        assert_eq!(
            classify("connection reset by peer"),
            Reason::AGENT_PROVIDER_NETWORK
        );
        assert_eq!(
            classify("model gpt-9 was not found"),
            Reason::AGENT_MODEL_NOT_FOUND_OR_UNAVAILABLE
        );
        assert_eq!(
            classify("claude: returned empty output"),
            Reason::AGENT_EMPTY_OR_UNPARSEABLE_OUTPUT
        );
        assert_eq!(
            classify("Task timed out after 2h0m0s"),
            Reason::AGENT_TIMEOUT
        );
        assert_eq!(
            classify("exec: executable not found in $PATH"),
            Reason::AGENT_RUNTIME_MISSING_EXECUTABLE
        );
        assert_eq!(
            classify("panic: runtime error"),
            Reason::AGENT_PROCESS_FAILURE
        );
        assert_eq!(classify("something entirely novel"), Reason::AGENT_UNKNOWN);
    }

    #[test]
    fn digit_boundary_guards_reject_embedded_codes() {
        // "402" inside a longer number must NOT hit quota.
        assert_ne!(
            classify("402913 tokens processed"),
            Reason::AGENT_PROVIDER_QUOTA_LIMIT
        );
        // "403" inside "4030" must NOT hit auth.
        assert_ne!(
            classify("exit status 4030"),
            Reason::AGENT_PROVIDER_AUTH_OR_ACCESS
        );
        // "1500ms" must NOT hit 5xx.
        assert_ne!(
            classify("took 1500ms then died"),
            Reason::AGENT_PROVIDER_SERVER_ERROR
        );
        // But standalone codes do match.
        assert_eq!(
            classify("request failed with 402"),
            Reason::AGENT_PROVIDER_QUOTA_LIMIT
        );
        assert_eq!(
            classify("got 529 from upstream"),
            Reason::AGENT_PROVIDER_CAPACITY_OR_RATE_LIMIT
        );
    }

    #[test]
    fn curly_apostrophe_quota_variant_matches() {
        assert_eq!(
            classify("you\u{2019}ve hit your limit"),
            Reason::AGENT_PROVIDER_QUOTA_LIMIT
        );
    }

    #[test]
    fn normalize_upgrades_legacy_daemon_shapes() {
        assert_eq!(
            normalize_daemon_reason(
                "agent_error.unknown",
                "resolve skill bundles: download failed"
            ),
            Reason::SKILL_BUNDLE_UNAVAILABLE
        );
        assert_eq!(
            normalize_daemon_reason(
                "agent_error.unknown",
                "API Error: The model has reached its context window limit."
            ),
            Reason::AGENT_CONTEXT_OVERFLOW
        );
        assert_eq!(
            normalize_daemon_reason(
                "agent_error.process_failure",
                "opencode stream ended: step left open at EOF"
            ),
            Reason::AGENT_PROVIDER_NETWORK
        );
        assert_eq!(
            normalize_daemon_reason(
                "agent_error.provider_network",
                "prepare openclaw config: context deadline exceeded"
            ),
            Reason::RUNTIME_CLI_TIMEOUT
        );
    }

    #[test]
    fn normalize_passthrough_preserves_unknown_legacy_values() {
        assert_eq!(
            normalize_daemon_reason("agent_error", "whatever"),
            Reason::from("agent_error")
        );
        assert_eq!(
            normalize_daemon_reason("runtime_offline", "unrelated text"),
            Reason::RUNTIME_OFFLINE
        );
    }

    #[test]
    fn normalize_refined_overflow_reason_is_not_upgraded_by_witness() {
        // legacyContextOverflowReasons deliberately excludes refined buckets:
        // process_failure on a crash marker says more than a witness in the
        // same blob.
        assert_eq!(
            normalize_daemon_reason(
                "agent_error.process_failure",
                "crashed; context window limit mentioned somewhere"
            ),
            Reason::from("agent_error.process_failure")
        );
    }

    #[test]
    fn unresumable_history_requires_both_signals() {
        assert!(unresumable_history(
            "Invalid request: the message at position 37 with role 'assistant' must not be empty"
        ));
        assert!(unresumable_history(
            "messages[43].content: content must not be empty"
        ));
        // Emptiness complaint without a locator → other field, not transcript.
        assert!(!unresumable_history("commit message must not be empty"));
        // Locator without an emptiness complaint.
        assert!(!unresumable_history("messages[3]: invalid role value"));
        assert!(!unresumable_history(""));
    }

    #[test]
    fn auth_method_unresolved_exact_phrase_only() {
        assert!(auth_method_unresolved(
            "session/resume failed: Could not resolve authentication method"
        ));
        // Expired/revoked credentials are NOT cured by a fresh session and
        // must not match.
        assert!(!auth_method_unresolved("authentication token expired"));
        assert!(!auth_method_unresolved(""));
    }

    #[test]
    fn provider_unconfigured_substring_not_equality() {
        assert!(provider_unconfigured(
            r#"hermes session/new failed: {"details":"No LLM provider configured."}"#
        ));
        assert!(!provider_unconfigured("provider misconfigured: bad key"));
        assert!(!provider_unconfigured(""));
    }

    #[test]
    fn context_exhausted_composite_clauses_only() {
        assert!(context_exhausted_completion(
            "Prompt is too long · A single-exchange conversation cannot be compacted; start a new session."
        ));
        assert!(context_exhausted_completion(
            "Conversation too long. Press esc twice to go up a few messages and try again."
        ));
        assert!(context_exhausted_completion(
            "Compaction failed · conversation could not be reduced below the context limit"
        ));
        // Bare sentence alone must NOT match — an agent asked "is my prompt
        // too long?" can legitimately answer exactly that.
        assert!(!context_exhausted_completion("Prompt is too long"));
        // Over the size cap → never re-read as exhaustion.
        let big = format!("Prompt is too long · {}", "x".repeat(400));
        assert!(!context_exhausted_completion(&big));
        assert!(!context_exhausted_completion(""));
    }

    #[test]
    fn reason_open_set_roundtrip() {
        let legacy = Reason::from("agent_error");
        assert_eq!(legacy.as_str(), "agent_error");
        assert!(!legacy.is_agent_error()); // coarse value lacks the dot suffix
        assert!(Reason::AGENT_UNKNOWN.is_agent_error());
    }
}
