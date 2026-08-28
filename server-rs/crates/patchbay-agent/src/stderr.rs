//! Bounded, sanitized child-process diagnostics.

use std::io::{self, Write};
use std::sync::{Arc, LazyLock, Mutex};

use regex::Regex;

pub const DEFAULT_TAIL_BYTES: usize = 2_048;
/// Extra raw bytes retained so a credential split by the output bound can
/// still match a secret pattern before the sanitized tail is truncated.
const REDACTION_WINDOW_BYTES: usize = 4_096;

static AUTH_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)(authorization\s*:\s*)[^\r\n]+")
        .unwrap_or_else(|error| panic!("invalid authorization regex: {error}"))
});
static JSON_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)("(?:token|auth|authorization|api[_-]?key|secret|password)"\s*:\s*)"(?:\\.|[^"\\])*""#)
        .unwrap_or_else(|error| panic!("invalid JSON secret regex: {error}"))
});
static DIAGNOSTIC_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(authorization|auth|api[_-]?key|token|secret|password)(\s*[:=]\s*)([^\s,;]+)")
        .unwrap_or_else(|error| panic!("invalid diagnostic secret regex: {error}"))
});

struct SecretPattern {
    regex: Regex,
    replacement: &'static str,
}

static SECRET_PATTERNS: LazyLock<Vec<SecretPattern>> = LazyLock::new(|| {
    [
        (r"\bAKIA[0-9A-Z]{16}\b", "[REDACTED AWS KEY]"),
        (
            r"(?i)(?:aws_secret_access_key|secret_?access_?key)\s*[=:]\s*[A-Za-z0-9/+=]{40}",
            "[REDACTED AWS SECRET]",
        ),
        (
            r"(?s)-----BEGIN[A-Z\s]*PRIVATE KEY-----.*?-----END[A-Z\s]*PRIVATE KEY-----",
            "[REDACTED PRIVATE KEY]",
        ),
        (
            r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9_]{36,255}\b",
            "[REDACTED GITHUB TOKEN]",
        ),
        (
            r"\bgithub_pat_[A-Za-z0-9_]{20,255}\b",
            "[REDACTED GITHUB TOKEN]",
        ),
        (r"\bsk-[A-Za-z0-9_-]{20,}\b", "[REDACTED API KEY]"),
        (
            r"\bxox[bporase]-[A-Za-z0-9\-]{10,}\b",
            "[REDACTED SLACK TOKEN]",
        ),
        (
            r"\bxapp-[A-Za-z0-9-]{10,}\b",
            "[REDACTED SLACK TOKEN]",
        ),
        (
            r"\bglpat-[A-Za-z0-9_-]{20,}\b",
            "[REDACTED GITLAB TOKEN]",
        ),
        (
            r"\bAIza[0-9A-Za-z_-]{35}([^0-9A-Za-z_-]|$)",
            "[REDACTED GOOGLE API KEY]$1",
        ),
        (
            r"\b(?:sk|rk)_live_[0-9A-Za-z]{16,}\b",
            "[REDACTED STRIPE KEY]",
        ),
        (
            r"\bey[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
            "[REDACTED JWT]",
        ),
        (
            r"(?i)\bBearer\s+[A-Za-z0-9\-._~+/]+=*\b",
            "Bearer [REDACTED]",
        ),
        (
            r"(?i)(?:postgres|mysql|mongodb|redis|amqp)(?:ql)?://[^:\s]+:[^@\s]+@",
            "[REDACTED CONNECTION STRING]@",
        ),
        (
            r"(?i)(?:API_KEY|API_SECRET|SECRET_KEY|SECRET|ACCESS_TOKEN|AUTH_TOKEN|PRIVATE_KEY|DATABASE_URL|DB_PASSWORD|DB_URL|REDIS_URL|PASSWORD|TOKEN)\s*[=:]\s*\S+",
            "[REDACTED CREDENTIAL]",
        ),
    ]
    .into_iter()
    .map(|(pattern, replacement)| SecretPattern {
        regex: Regex::new(pattern)
            .unwrap_or_else(|error| panic!("invalid diagnostic redaction regex: {error}")),
        replacement,
    })
    .collect()
});

static HOME_MASK: LazyLock<Option<String>> = LazyLock::new(detect_home_path);

#[derive(Debug)]
struct State {
    bytes: Vec<u8>,
    total: u64,
}

/// Cloneable sink used by async child-process stderr pumps. It retains only a
/// bounded byte tail; callers sanitize the tail before persistence or logs.
#[derive(Debug, Clone)]
pub struct SharedDiagnosticBuffer {
    max: usize,
    state: Arc<Mutex<State>>,
}

impl SharedDiagnosticBuffer {
    pub fn new(max: usize) -> Self {
        Self {
            max: if max == 0 { DEFAULT_TAIL_BYTES } else { max },
            state: Arc::new(Mutex::new(State {
                bytes: Vec::new(),
                total: 0,
            })),
        }
    }

    pub fn push(&self, buffer: &[u8]) {
        if let Ok(mut state) = self.state.lock() {
            state.total = state.total.saturating_add(buffer.len() as u64);
            state.bytes.extend_from_slice(buffer);
            retain_redaction_window(&mut state.bytes, self.max);
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.state.lock().map_or(0, |state| state.total)
    }

    pub fn tail(&self) -> String {
        self.state.lock().map_or_else(
            |_| String::new(),
            |state| bounded_sanitized_tail(&state.bytes, self.max),
        )
    }
}

/// Forwards stderr while retaining only a bounded tail for a task-visible
/// failure diagnostic.
#[derive(Debug)]
pub struct DiagnosticTail<W> {
    inner: W,
    max: usize,
    state: Mutex<State>,
}

impl<W> DiagnosticTail<W> {
    pub fn new(inner: W, max: usize) -> Self {
        Self {
            inner,
            max: if max == 0 { DEFAULT_TAIL_BYTES } else { max },
            state: Mutex::new(State {
                bytes: Vec::new(),
                total: 0,
            }),
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.state.lock().map_or(0, |state| state.total)
    }

    pub fn tail(&self) -> String {
        let Ok(state) = self.state.lock() else {
            return String::new();
        };
        bounded_sanitized_tail(&state.bytes, self.max)
    }
}

impl<W: Write> Write for DiagnosticTail<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.inner.write_all(buffer)?;
        if let Ok(mut state) = self.state.lock() {
            state.total = state.total.saturating_add(buffer.len() as u64);
            state.bytes.extend_from_slice(buffer);
            retain_redaction_window(&mut state.bytes, self.max);
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub fn sanitize_diagnostic(value: &str) -> String {
    let mut clean: String = value
        .chars()
        .filter(|character| *character >= ' ' || matches!(character, '\n' | '\t'))
        .collect();
    for pattern in SECRET_PATTERNS.iter() {
        clean = pattern
            .regex
            .replace_all(&clean, pattern.replacement)
            .into_owned();
    }
    if let Some(home) = HOME_MASK.as_ref() {
        clean = mask_home_path(&clean, home);
    }
    let clean = AUTH_HEADER.replace_all(&clean, "$1[REDACTED]");
    let clean = JSON_SECRET.replace_all(&clean, "$1\"[REDACTED]\"");
    DIAGNOSTIC_SECRET
        .replace_all(&clean, "$1$2[REDACTED]")
        .trim()
        .to_string()
}

fn detect_home_path() -> Option<String> {
    let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok()?;
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        None
    } else {
        Some(home.to_string())
    }
}

fn mask_home_path(value: &str, home: &str) -> String {
    if home.is_empty() {
        value.to_string()
    } else {
        value.replace(home, "[HOME]")
    }
}

fn retain_redaction_window(bytes: &mut Vec<u8>, output_max: usize) {
    let keep = output_max.saturating_add(REDACTION_WINDOW_BYTES);
    let excess = bytes.len().saturating_sub(keep);
    if excess > 0 {
        bytes.drain(..excess);
    }
}

fn bounded_sanitized_tail(bytes: &[u8], output_max: usize) -> String {
    let sanitized = sanitize_diagnostic(String::from_utf8_lossy(bytes).trim());
    take_utf8_suffix(&sanitized, output_max).to_string()
}

fn take_utf8_suffix(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let start = value.len() - max_bytes;
    let start = value
        .char_indices()
        .find(|(index, _)| *index >= start)
        .map(|(index, _)| index)
        .unwrap_or(0);
    &value[start..]
}

pub fn with_stderr(message: &str, label: &str, tail: &str) -> String {
    if tail.is_empty() {
        message.to_string()
    } else {
        format!("{message}; {label} stderr: {tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_all_bytes_and_retains_only_tail() {
        let mut tail = DiagnosticTail::new(Vec::new(), 5);
        assert!(tail.write_all(b"abcdef").is_ok());
        assert_eq!(tail.total_bytes(), 6);
        assert_eq!(tail.tail(), "bcdef");
        assert_eq!(tail.inner, b"abcdef");
    }

    #[test]
    fn diagnostic_removes_controls_and_common_secret_shapes() {
        let value = "\u{1b}Authorization: Bearer abc\n{\"api_key\":\"xyz\"} token=qwerty";
        let clean = sanitize_diagnostic(value);
        assert!(!clean.contains("abc"));
        assert!(!clean.contains("xyz"));
        assert!(!clean.contains("qwerty"));
        assert!(!clean.contains('\u{1b}'));
    }

    #[test]
    fn shared_buffer_is_bounded_across_clones() {
        let buffer = SharedDiagnosticBuffer::new(5);
        buffer.push(b"abc");
        buffer.clone().push(b"def");
        assert_eq!(buffer.total_bytes(), 6);
        assert_eq!(buffer.tail(), "bcdef");
    }

    #[test]
    fn sanitizes_before_truncating_a_split_credential() {
        let secret = format!("token={}", "s".repeat(80));
        let mut tail = DiagnosticTail::new(Vec::new(), 32);
        assert!(tail.write_all(secret.as_bytes()).is_ok());
        let rendered = tail.tail();
        assert!(!rendered.contains('s'), "{rendered}");
        assert!(
            rendered.contains("REDACTED"),
            "truncated tail should still carry the redaction marker: {rendered}"
        );
    }

    #[test]
    fn home_paths_are_masked_without_a_username() {
        assert_eq!(
            mask_home_path("/var/empty/.config/token", "/var/empty"),
            "[HOME]/.config/token"
        );
        assert_eq!(
            mask_home_path("C:\\Users\\svc\\.patchbay", "C:\\Users\\svc"),
            "[HOME]\\.patchbay"
        );
    }
}
