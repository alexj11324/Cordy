//! Signed connect-state tokens.
//!

use std::time::{SystemTime, UNIX_EPOCH};

use hmac::Mac as _;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Returned when the state token is not the expected "<payload>.<sig>"
/// base64url shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("composio: state malformed")]
pub struct StateMalformedError;

/// Returned when the HMAC signature does not match — the state was tampered
/// with or signed by a different secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("composio: state signature mismatch")]
pub struct StateSignatureError;

/// Returned when the state's exp claim is in the past.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("composio: state expired")]
pub struct StateExpiredError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum StateError {
    #[error("composio: state malformed")]
    Malformed,
    #[error("composio: state signature mismatch")]
    Signature,
    #[error("composio: state expired")]
    Expired,
}

impl From<StateMalformedError> for StateError {
    fn from(_: StateMalformedError) -> Self {
        Self::Malformed
    }
}
impl From<StateSignatureError> for StateError {
    fn from(_: StateSignatureError) -> Self {
        Self::Signature
    }
}
impl From<StateExpiredError> for StateError {
    fn from(_: StateExpiredError) -> Self {
        Self::Expired
    }
}

/// The payload embedded in the signed connect-state. It carries exactly
/// what CompleteCallback needs to attribute the callback to a user and
/// toolkit without a server-side session table — the signature is what
/// makes it trustworthy, the short exp is what bounds replay.
///
/// Field names are single letters (matching the Go json tags) to keep the
/// encoded token compact; they are an internal wire format, never exposed
/// to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateClaims {
    #[serde(rename = "u")]
    pub user_id: String,
    #[serde(rename = "t")]
    pub toolkit_slug: String,
    /// The exact Composio auth_config_id resolved at BeginConnect and used
    /// to create the connect link. Signing it into the state lets
    /// CompleteCallback verify the returned account was created under THIS
    /// toolkit's auth config without re-resolving (which could fail-open).
    /// It is an opaque config handle (ac_…), not a credential.
    #[serde(rename = "a")]
    pub auth_config_id: String,
    #[serde(rename = "e")]
    pub exp: i64,
}

/// Produces a URL-safe "<payload>.<sig>" token: payload is the
/// base64url-encoded JSON claims; sig is the base64url-encoded HMAC-SHA256
/// of the payload under the service secret. We sign the encoded payload
/// (not the raw struct) so verification re-derives the exact bytes that
/// were signed.
pub fn sign_state(secret: &[u8], claims: &StateClaims) -> Result<String, serde_json::Error> {
    let raw = serde_json::to_vec(claims)?;
    use base64::Engine as _;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    let sig = sign_payload(secret, &payload);
    Ok(format!("{payload}.{sig}"))
}

/// Validates the signature and expiry of a token produced by [`sign_state`]
/// and returns the embedded claims. Signature is checked with a
/// constant-time compare before the payload is trusted; expiry is checked
/// against `now`.
pub fn verify_state(
    secret: &[u8],
    token: &str,
    now: SystemTime,
) -> Result<StateClaims, StateError> {
    use base64::Engine as _;
    let Some((payload, sig)) = token.split_once('.') else {
        return Err(StateError::Malformed);
    };
    if payload.is_empty() || sig.is_empty() {
        return Err(StateError::Malformed);
    }
    let expected = sign_payload(secret, payload);
    if !constant_time_eq(sig.as_bytes(), expected.as_bytes()) {
        return Err(StateError::Signature);
    }
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| StateError::Malformed)?;
    let claims: StateClaims = serde_json::from_slice(&raw).map_err(|_| StateError::Malformed)?;
    let unix_now = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if unix_now > claims.exp {
        return Err(StateError::Expired);
    }
    Ok(claims)
}

/// The base64url HMAC-SHA256 of payload under secret.
pub fn sign_payload(secret: &[u8], payload: &str) -> String {
    let mut mac =
        <hmac::Hmac<sha2::Sha256> as hmac::Mac>::new_from_slice(secret).expect("hmac any key len");
    mac.update(payload.as_bytes());
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// (Duration re-export removed: callers name std::time::Duration directly.)

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn claims(exp: i64) -> StateClaims {
        StateClaims {
            user_id: "0198c0de-0000-7000-8000-000000000001".into(),
            toolkit_slug: "github".into(),
            auth_config_id: "ac_123".into(),
            exp,
        }
    }

    #[test]
    fn sign_then_verify_roundtrips_claims() {
        let secret = b"k";
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let c = claims(1_700_000_100);
        let token = sign_state(secret, &c).unwrap();
        let got = verify_state(secret, &token, now).unwrap();
        assert_eq!(got.user_id, c.user_id);
        assert_eq!(got.toolkit_slug, c.toolkit_slug);
        assert_eq!(got.auth_config_id, c.auth_config_id);
        assert_eq!(got.exp, c.exp);
    }

    #[test]
    fn tampered_payload_fails_signature_check() {
        let secret = b"k";
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let token = sign_state(secret, &claims(1_700_000_100)).unwrap();
        // Flip one char of the payload.
        let mut chars: Vec<char> = token.chars().collect();
        let dot = chars.iter().position(|c| *c == '.').unwrap();
        chars[0] = if chars[0] == 'e' { 'f' } else { 'e' };
        let _ = dot;
        assert!(matches!(
            verify_state(secret, &chars.into_iter().collect::<String>(), now),
            Err(StateError::Signature)
        ));
        // Different secret also fails.
        assert!(matches!(
            verify_state(b"other", &token, now),
            Err(StateError::Signature)
        ));
    }

    #[test]
    fn expired_and_malformed_shapes() {
        let secret = b"k";
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_200);
        let token = sign_state(secret, &claims(1_700_000_100)).unwrap();
        assert!(matches!(
            verify_state(secret, &token, now),
            Err(StateError::Expired)
        ));
        // Boundary: now == exp passes (Go checks now > exp).
        assert!(verify_state(
            secret,
            &token,
            UNIX_EPOCH + Duration::from_secs(1_700_000_100)
        )
        .is_ok());
        // "abc.def.ghi" splits at the first dot into non-empty halves, so it
        // fails the SIGNATURE check rather than the shape check.
        for bad in ["", ".", "abc"] {
            assert!(matches!(
                verify_state(secret, bad, now),
                Err(StateError::Malformed)
            ));
        }
        assert!(matches!(
            verify_state(secret, "abc.def.ghi", now),
            Err(StateError::Signature)
        ));
    }
}
