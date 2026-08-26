//! Shared primitives for the Cordy Rust backend.
//!
//! Ported from `server/internal/util` and `server/pkg/...` as the migration
//! progresses. Keep this crate dependency-light: it is linked by everything.

pub mod channel_media;
pub mod mentions;
pub mod logging;
pub mod json;
pub mod secretbox;
pub mod self_exec;
pub mod text;

pub use text::unescape_backslash_escapes;

use serde::{Deserialize, Serialize};

/// Typed ULID wrapper.
///
/// Go side uses `oklog/ulid/v2`; on the wire ULIDs are 26-char uppercase
/// Crockford base32 strings. Serde serializes as that string to keep API
/// contracts byte-identical (see migration plan §二 hard constraints).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ulid(#[serde(with = "ulid_string")] pub uuid::Uuid);

mod ulid_string {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &uuid::Uuid, s: S) -> Result<S::Ok, S::Error> {
        // TODO(S2): switch to Crockford base32 via the `ulid` crate once ids
        // are generated natively; UUIDv7 hyphenated form is accepted by the
        // frontend today but must be re-audited before cutover.
        s.serialize_str(&v.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<uuid::Uuid, D::Error> {
        let s = String::deserialize(d)?;
        uuid::Uuid::parse_str(&s).map_err(serde::de::Error::custom)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulid_roundtrips_as_string() {
        let id = Ulid(uuid::Uuid::now_v7());
        let json = serde_json::to_string(&id).unwrap();
        assert!(json.starts_with('"'));
        let back: Ulid = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }
}
