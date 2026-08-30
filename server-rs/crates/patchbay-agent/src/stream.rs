//! Bounded line-delimited transport reading and shared terminal semantics.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

pub const MAX_LINE_BYTES: usize = 32 * 1024 * 1024;
pub const INITIAL_BUFFER_BYTES: usize = 1024 * 1024;

/// Reads a line-delimited provider transport without Tokio's default small-line
/// assumptions. The limit is checked before converting to UTF-8.
pub struct AgentLineReader<R> {
    reader: R,
    buffer: Vec<u8>,
    max_line_bytes: usize,
}

impl<R: AsyncBufRead + Unpin> AgentLineReader<R> {
    pub fn new(reader: R) -> Self {
        Self::with_limit(reader, MAX_LINE_BYTES)
    }

    pub fn with_limit(reader: R, max_line_bytes: usize) -> Self {
        Self {
            reader,
            buffer: Vec::with_capacity(INITIAL_BUFFER_BYTES.min(max_line_bytes)),
            max_line_bytes,
        }
    }

    pub async fn next_line(&mut self) -> io::Result<Option<String>> {
        self.buffer.clear();
        // `read_until` alone grows without bound before the post-read length
        // check. Wrap this one read in `take(max + 1)` so an unterminated or
        // malicious line can allocate at most one byte beyond the contract.
        let limit = self.max_line_bytes.saturating_add(1) as u64;
        let bytes = (&mut self.reader)
            .take(limit)
            .read_until(b'\n', &mut self.buffer)
            .await?;
        if bytes == 0 {
            return Ok(None);
        }
        if self.buffer.len() > self.max_line_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "agent stream line exceeds {} byte limit",
                    self.max_line_bytes
                ),
            ));
        }
        if self.buffer.last() == Some(&b'\n') {
            self.buffer.pop();
            if self.buffer.last() == Some(&b'\r') {
                self.buffer.pop();
            }
        }
        String::from_utf8(self.buffer.clone())
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssistantTurn {
    pub text: String,
    pub tool_uses: usize,
    pub understood: bool,
}

impl AssistantTurn {
    /// A tool-bearing or unreadable turn clears the previous fallback. A
    /// thinking-only understood turn leaves it intact.
    pub fn resolve_fallback(&self, previous: &str) -> String {
        match (self.tool_uses, self.understood, self.text.is_empty()) {
            (count, _, _) if count > 0 => String::new(),
            (_, false, _) => String::new(),
            (_, true, false) => self.text.clone(),
            (_, true, true) => previous.to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalState {
    pub last_assistant_text: String,
    pub final_result_text: String,
    pub saw_result: bool,
    pub result_is_error: bool,
    pub terminal_reason_error: String,
    pub scan_error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnd {
    Completed,
    DeadlineExceeded,
    Cancelled,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FinalizedStream {
    pub status: String,
    pub output: String,
    pub error: String,
}

/// Applies the provider-neutral fail-closed terminal contract used by
/// stream-json adapters. A clean process exit without a terminal result is a
/// protocol failure, and failed runs never expose partial Agent event history text as
/// final output.
#[allow(clippy::too_many_arguments)]
pub fn finalize_stream(
    provider: &str,
    timeout: Duration,
    run_end: RunEnd,
    write_error: Option<&str>,
    exit_error: Option<&str>,
    session_id: &str,
    state: &TerminalState,
    completion_guard_error: Option<&str>,
) -> FinalizedStream {
    let mut status = "completed".to_string();
    let mut error = String::new();

    if !state.terminal_reason_error.is_empty() {
        status = "failed".to_string();
        error = state.terminal_reason_error.clone();
    } else if state.result_is_error {
        status = "failed".to_string();
        error = if state.final_result_text.is_empty() {
            format!("{provider} returned an error result without details")
        } else {
            state.final_result_text.clone()
        };
    }

    if status == "completed" {
        match run_end {
            RunEnd::DeadlineExceeded => {
                status = "timeout".to_string();
                error = format!("{provider} timed out after {}s", timeout.as_secs_f64());
            }
            RunEnd::Cancelled => {
                status = "aborted".to_string();
                error = "execution cancelled".to_string();
            }
            RunEnd::Completed => {
                if !state.scan_error.is_empty() {
                    status = "failed".to_string();
                    error = format!("{provider} stdout read error: {}", state.scan_error);
                } else if let Some(write_error) = write_error.filter(|_| session_id.is_empty()) {
                    status = "failed".to_string();
                    error = format!("write {provider} input: {write_error}");
                } else if let Some(exit_error) = exit_error {
                    status = "failed".to_string();
                    error = format!("{provider} exited with error: {exit_error}");
                } else if !state.saw_result {
                    status = "failed".to_string();
                    error = format!("{provider} stream ended without terminal result");
                }
            }
        }
    }

    if status == "completed" {
        if let Some(guard) = completion_guard_error.filter(|guard| !guard.is_empty()) {
            status = "failed".to_string();
            error = guard.to_string();
        }
    }
    let output = if status != "completed" {
        String::new()
    } else if !state.final_result_text.is_empty() {
        state.final_result_text.clone()
    } else {
        state.last_assistant_text.clone()
    };
    FinalizedStream {
        status,
        output,
        error,
    }
}

/// Positive-evidence predicate for the fresh-session retry path shared by
/// resumable stream providers. An emitted different session also proves the
/// requested Agent event history was not loaded.
pub fn resume_was_rejected<'a>(
    requested: &str,
    emitted: &str,
    failed: bool,
    texts: impl IntoIterator<Item = &'a str>,
) -> bool {
    if !failed || requested.is_empty() {
        return false;
    }
    const PHRASES: &[&str] = &[
        "invalid conversation id",
        "conversation not found",
        "session not found",
        "no conversation found",
        "no saved session found",
        "已绑定另外",
        "bound to another account",
        "bound to a different account",
    ];
    if texts.into_iter().any(|text| {
        let text = text.to_lowercase();
        PHRASES.iter().any(|phrase| text.contains(phrase))
    }) {
        return true;
    }
    !emitted.is_empty() && emitted != requested
}

#[cfg(test)]
mod tests {
    use tokio::io::BufReader;

    use super::*;

    #[tokio::test]
    async fn reader_accepts_lines_above_the_old_scanner_limit() {
        let line = "x".repeat(11 * 1024 * 1024);
        let input = format!("{line}\n");
        let mut reader = AgentLineReader::new(BufReader::new(input.as_bytes()));
        let result = reader.next_line().await;
        assert!(result.is_ok());
        assert_eq!(
            result.ok().flatten().map(|line| line.len()),
            Some(line.len())
        );
    }

    #[tokio::test]
    async fn reader_fails_closed_above_its_limit() {
        let input = "abcdef\n";
        let mut reader = AgentLineReader::with_limit(BufReader::new(input.as_bytes()), 5);
        let error = reader.next_line().await.err();
        assert_eq!(
            error.map(|error| error.kind()),
            Some(io::ErrorKind::InvalidData)
        );
    }

    #[test]
    fn tool_and_unreadable_turns_clear_stale_fallbacks() {
        let tool = AssistantTurn {
            text: "I will edit".to_string(),
            tool_uses: 1,
            understood: true,
        };
        let unreadable = AssistantTurn {
            understood: false,
            ..AssistantTurn::default()
        };
        assert_eq!(tool.resolve_fallback("old"), "");
        assert_eq!(unreadable.resolve_fallback("old"), "");
    }

    #[test]
    fn success_prefers_terminal_output_then_complete_assistant_fallback() {
        let terminal = TerminalState {
            final_result_text: "final".to_string(),
            last_assistant_text: "fallback".to_string(),
            saw_result: true,
            ..TerminalState::default()
        };
        assert_eq!(
            finalize_stream(
                "agent",
                Duration::ZERO,
                RunEnd::Completed,
                None,
                None,
                "session",
                &terminal,
                None,
            )
            .output,
            "final"
        );
        let fallback = TerminalState {
            final_result_text: String::new(),
            ..terminal
        };
        assert_eq!(
            finalize_stream(
                "agent",
                Duration::ZERO,
                RunEnd::Completed,
                None,
                None,
                "session",
                &fallback,
                None,
            )
            .output,
            "fallback"
        );
    }

    #[test]
    fn missing_terminal_and_failed_result_never_deliver_partial_text() {
        let missing = TerminalState {
            last_assistant_text: "partial".to_string(),
            ..TerminalState::default()
        };
        let finalized = finalize_stream(
            "agent",
            Duration::ZERO,
            RunEnd::Completed,
            None,
            None,
            "session",
            &missing,
            None,
        );
        assert_eq!(finalized.status, "failed");
        assert!(finalized.output.is_empty());

        let failed = TerminalState {
            saw_result: true,
            result_is_error: true,
            final_result_text: "provider failure".to_string(),
            last_assistant_text: "partial".to_string(),
            ..TerminalState::default()
        };
        let finalized = finalize_stream(
            "agent",
            Duration::ZERO,
            RunEnd::Completed,
            None,
            None,
            "session",
            &failed,
            None,
        );
        assert_eq!(finalized.error, "provider failure");
        assert!(finalized.output.is_empty());
    }

    #[test]
    fn resume_rejection_requires_positive_failure_evidence() {
        assert!(!resume_was_rejected(
            "old",
            "",
            false,
            ["No saved session found"]
        ));
        assert!(resume_was_rejected(
            "old",
            "",
            true,
            ["No saved session found with ID redacted"]
        ));
        assert!(resume_was_rejected("old", "fresh", true, [""]));
        assert!(!resume_was_rejected("old", "old", true, ["network error"]));
    }
}
