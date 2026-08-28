//! Shared primitives for the Patchbay Rust backend.
//!
//! Keep this crate dependency-light: it is linked by everything.

pub mod channel_media;
pub mod branding;
pub mod logging;
pub mod secretbox;

use serde::{Deserialize, Serialize};

/// Selects the workspace's Ring-backed rustls provider before any TLS client
/// builds a `ClientConfig`. This is required when transitive dependencies also
/// enable the AWS-LC provider on rustls.
pub fn install_rustls_crypto_provider() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("a rustls crypto provider was installed before Patchbay startup"))
}

/// Typed ULID wrapper.
///
/// Go side uses `oklog/ulid/v2`; canonical ULIDs are 26-char uppercase
/// Crockford base32 strings. Realtime and daemon production ID generators use
/// [`new_ulid`] so the shared wire representation is kept in one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ulid(#[serde(with = "ulid_string")] pub uuid::Uuid);

/// Generates a canonical uppercase 26-character ULID string.
pub fn new_ulid() -> String {
    ulid::Ulid::new().to_string()
}

/// Formats a UTC timestamp with Go `time.RFC3339Nano` semantics.
///
/// Chrono's nanosecond formatter always emits nine fractional digits. Go
/// emits the same precision but trims every trailing zero, including the
/// entire fractional part for whole-second values.
pub fn rfc3339_nano(timestamp: chrono::DateTime<chrono::Utc>) -> String {
    let rendered = timestamp.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    let Some((prefix, fraction_and_zone)) = rendered.split_once('.') else {
        return rendered;
    };
    let fraction = fraction_and_zone
        .trim_end_matches('Z')
        .trim_end_matches('0');
    if fraction.is_empty() {
        format!("{prefix}Z")
    } else {
        format!("{prefix}.{fraction}Z")
    }
}

mod ulid_string {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &uuid::Uuid, s: S) -> Result<S::Ok, S::Error> {
        let encoded = ulid::Ulid::from_bytes(*v.as_bytes()).to_string();
        s.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<uuid::Uuid, D::Error> {
        let s = String::deserialize(d)?;
        if s.len() != 26 {
            return Err(serde::de::Error::custom(
                "ULID must be exactly 26 characters",
            ));
        }
        if !s
            .as_bytes()
            .first()
            .is_some_and(|byte| matches!(byte, b'0'..=b'7'))
        {
            return Err(serde::de::Error::custom(
                "ULID exceeds the 128-bit canonical range",
            ));
        }
        let value = ulid::Ulid::from_string(&s).map_err(serde::de::Error::custom)?;
        Ok(uuid::Uuid::from_bytes(value.to_bytes()))
    }
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
/// Unescapes the supported backslash escape sequences.
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
    fn ulid_serializes_as_crockford_wire_value() {
        let id = Ulid(uuid::Uuid::now_v7());
        let json = serde_json::to_string(&id).unwrap();
        let wire = json.trim_matches('"');
        assert_eq!(wire.len(), 26);
        assert!(wire.bytes().all(|byte| {
            matches!(
                byte,
                b'0'..=b'9'
                    | b'A'..=b'H'
                    | b'J'..=b'K'
                    | b'M'..=b'N'
                    | b'P'..=b'T'
                    | b'V'..=b'Z'
            )
        }));
        let back: Ulid = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn generated_ulid_is_canonical_wire_value() {
        let wire = new_ulid();
        assert_eq!(wire.len(), 26);
        let parsed = ulid::Ulid::from_string(&wire).unwrap();
        assert_eq!(parsed.to_string(), wire);
    }

    #[test]
    fn rfc3339_nano_matches_go_fractional_precision() {
        let four_digits = chrono::DateTime::parse_from_rfc3339("2026-08-27T23:00:00.123400Z")
            .unwrap()
            .to_utc();
        assert_eq!(rfc3339_nano(four_digits), "2026-08-27T23:00:00.1234Z");

        let nine_digits = chrono::DateTime::parse_from_rfc3339("2026-08-27T23:00:00.123456789Z")
            .unwrap()
            .to_utc();
        assert_eq!(rfc3339_nano(nine_digits), "2026-08-27T23:00:00.123456789Z");

        let whole_second = chrono::DateTime::parse_from_rfc3339("2026-08-27T23:00:00Z")
            .unwrap()
            .to_utc();
        assert_eq!(rfc3339_nano(whole_second), "2026-08-27T23:00:00Z");
    }

    #[test]
    fn ulid_matches_go_crockford_vector() {
        const VECTOR: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let value = ulid::Ulid::from_string(VECTOR).unwrap();
        let id = Ulid(uuid::Uuid::from_bytes(value.to_bytes()));

        assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{VECTOR}\""));
        assert_eq!(
            serde_json::from_str::<Ulid>(&format!("\"{VECTOR}\"")).unwrap(),
            id
        );
    }

    #[test]
    fn ulid_rejects_uuid_hyphenated_wire_value() {
        let uuid_wire = format!("\"{}\"", uuid::Uuid::nil());
        assert!(serde_json::from_str::<Ulid>(&uuid_wire).is_err());
    }

    #[test]
    fn ulid_rejects_overflow_invalid_and_wrong_length_vectors() {
        for wire in [
            "80000000000000000000000000",
            "ZZZZZZZZZZZZZZZZZZZZZZZZZZ",
            "01ARZ3NDEKTSV4RRFFQ69G5FAI",
            "01ARZ3NDEKTSV4RRFFQ69G5FA!",
            "01ARZ3NDEKTSV4RRFFQ69G5FA",
            "01ARZ3NDEKTSV4RRFFQ69G5FAV0",
        ] {
            assert!(
                serde_json::from_str::<Ulid>(&format!("\"{wire}\"")).is_err(),
                "accepted invalid ULID {wire}"
            );
        }

        const MAX: &str = "7ZZZZZZZZZZZZZZZZZZZZZZZZZ";
        let parsed = serde_json::from_str::<Ulid>(&format!("\"{MAX}\"")).unwrap();
        assert_eq!(
            serde_json::to_string(&parsed).unwrap(),
            format!("\"{MAX}\"")
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
