//! Shared primitives for the Cordy Rust backend.
//!
//! Ported from `server/internal/util` and `server/pkg/...` as the migration
//! progresses. Keep this crate dependency-light: it is linked by everything.

pub mod channel_media;
pub mod secretbox;

use serde::{Deserialize, Serialize};

/// Typed ULID wrapper.
///
/// Go side uses `oklog/ulid/v2`; on the wire ULIDs are 26-character uppercase
/// Crockford base32 strings. Wrapping the Rust ULID type makes that canonical
/// representation the only serde representation instead of accidentally
/// emitting a UUIDv7 string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ulid(pub ulid::Ulid);

impl Ulid {
    /// Generates a canonical ULID for production event and node identifiers.
    pub fn new() -> Self {
        Self(ulid::Ulid::new())
    }

    /// Parses the canonical Go-compatible Crockford base32 representation.
    pub fn from_string(value: &str) -> Result<Self, ulid::DecodeError> {
        ulid::Ulid::from_string(value).map(Self)
    }
}

impl Default for Ulid {
    fn default() -> Self {
        Self(ulid::Ulid::nil())
    }
}

impl std::fmt::Display for Ulid {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Generates a canonical uppercase 26-character ULID string.
pub fn new_ulid() -> String {
    Ulid::new().to_string()
}

/// Domain error type shared across crates.
///
/// Mirrors the error taxonomy emerging from `internal/handler` responses:
/// transport layers map these onto HTTP status codes in one place.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(&'static str),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("invalid request: {0}")]
    Invalid(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Decodes the literal two-character `\n` / `\r` / `\t` / `\\` sequences that
/// agent CLIs emit into their real control characters.
///
/// Port of `util.UnescapeBackslashEscapes` (server/internal/util/text.go).
/// Everything else — including any other backslash pair and a trailing lone
/// backslash — passes through byte-for-byte. Callers that need a literal
/// 4-char sequence intact must bypass this helper entirely (the CLI exposes
/// --content-stdin / --description-stdin for that case).
pub fn unescape_backslash_escapes(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut b = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'n' => {
                    b.push('\n');
                    i += 2;
                    continue;
                }
                b'r' => {
                    b.push('\r');
                    i += 2;
                    continue;
                }
                b't' => {
                    b.push('\t');
                    i += 2;
                    continue;
                }
                b'\\' => {
                    b.push('\\');
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        // Input is &str so it is valid UTF-8; copying byte ranges through
        // char boundaries is safe because ASCII '\\' can never split one.
        let start = i;
        i += 1;
        while i < bytes.len() && (bytes[i] & 0xC0) == 0x80 {
            i += 1;
        }
        b.push_str(&s[start..i]);
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulid_roundtrips_as_string() {
        let id = Ulid::new();
        let json = serde_json::to_string(&id).unwrap();
        let encoded = json.trim_matches('"');
        assert_eq!(encoded.len(), 26);
        assert!(encoded
            .chars()
            .all(|character| character.is_ascii_digit() || ('A'..='Z').contains(&character)));
        let back: Ulid = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn known_ulid_keeps_go_wire_shape() {
        let encoded = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let id = Ulid::from_string(encoded).unwrap();
        assert_eq!(id.to_string(), encoded);
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            format!("\"{encoded}\"")
        );
    }

    #[test]
    fn unescape_backslash_escapes_decodes_the_four_cli_sequences() {
        assert_eq!(unescape_backslash_escapes("a\\nb"), "a\nb");
        assert_eq!(unescape_backslash_escapes("a\\rb\\tc"), "a\rb\tc");
        assert_eq!(unescape_backslash_escapes("\\\\n"), "\\n");
        assert_eq!(unescape_backslash_escapes("trail\\"), "trail\\");
        assert_eq!(unescape_backslash_escapes("plain"), "plain");
        assert_eq!(unescape_backslash_escapes("\\x41"), "\\x41");
    }
}
