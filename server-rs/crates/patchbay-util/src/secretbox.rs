//! Authenticated symmetric encryption for secrets stored at rest —
//! primarily Lark `app_secret` and any future per-tenant secret column
//! that must not appear in plaintext in a DB dump (PB-2671 §4.4).
//!
//! Construction: AES-256-GCM with a per-message 12-byte random nonce
//! prepended to the ciphertext (`nonce || ciphertext || tag`). GCM
//! provides both confidentiality and integrity, so a tampered row
//! decrypts to an error instead of silently garbled plaintext.
//!
//! # Wire compatibility
//!
//! This layout is part of the persisted ciphertext contract: sealing appends
//! ciphertext and tag to a freshly generated 12-byte nonce, and opening splits at the same
//! offset with no associated data. Existing ciphertexts in production
//! databases remain readable; do not change the layout.
//!
//! Key: 32 bytes. Loaded from an env var as base64 ([`load_key`]).
//! Rotation is not supported in this iteration — once we have multiple
//! keys in production we add a single-byte prefix to ciphertext for key
//! id; today every ciphertext is keyed by the one current master key.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use rand::rngs::OsRng;
use rand::RngCore;

/// Required master-key length in bytes (AES-256).
pub const KEY_SIZE: usize = 32;

const NONCE_SIZE: usize = 12;
/// GCM tag overhead (128-bit tag), matching Go's `aead.Overhead()`.
const TAG_SIZE: usize = 16;

/// Errors returned by this module. Message strings mirror the Go
/// implementation where they surface in operator-facing logs.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Returned by [`SecretBox::new`] when the key length is not [`KEY_SIZE`].
    #[error("secretbox: key must be 32 bytes")]
    InvalidKey,

    /// Returned when the input to [`SecretBox::open`] is smaller than
    /// the nonce + GCM tag overhead.
    #[error("secretbox: ciphertext too short")]
    CiphertextTooShort,

    /// GCM authentication failure on [`SecretBox::open`] — the input was
    /// tampered with or was not produced by this construction.
    #[error("secretbox: message authentication failed")]
    AuthFailed,

    /// AEAD failure during [`SecretBox::seal`] (allocation failure).
    #[error("secretbox: gcm: {0}")]
    Gcm(#[source] aes_gcm::aead::Error),

    /// Env var named in the message is unset or empty.
    #[error("secretbox: {0} is not set")]
    EnvNotSet(String),

    /// Env var value is not valid standard base64.
    #[error("secretbox: {0} is not valid base64: {1}")]
    InvalidBase64(String, #[source] base64::DecodeError),

    /// Decoded env var key has the wrong length.
    #[error("secretbox: {0} decodes to {1} bytes, expected {2}")]
    WrongKeyLength(String, usize, usize),
}

/// Encrypts and decrypts byte slices using a fixed master key.
///
/// Cheap to clone; instances are safe for concurrent use after
/// construction. Callers should hold one instance for the process
/// lifetime — constructing it per request needlessly re-derives the AES
/// round keys.
#[derive(Clone)]
pub struct SecretBox {
    aead: Aes256Gcm,
}

// Manual impl: `Aes256Gcm` is not `Debug`, and the derived form would
// risk printing key material anyway.
impl std::fmt::Debug for SecretBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretBox").finish_non_exhaustive()
    }
}

impl SecretBox {
    /// Constructs a [`SecretBox`] bound to the given 32-byte master key.
    pub fn new(key: &[u8]) -> Result<Self, Error> {
        if key.len() != KEY_SIZE {
            return Err(Error::InvalidKey);
        }
        // Length is proven == KEY_SIZE above, so `from_slice` cannot panic.
        Ok(Self {
            aead: Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key)),
        })
    }

    /// Encrypts plaintext and returns `nonce || ciphertext || tag`.
    ///
    /// The nonce is randomly generated per call; callers must NOT cache
    /// or reuse the output as if it were deterministic (e.g. don't index
    /// a secret by its ciphertext — two `seal` calls on the same
    /// plaintext produce different bytes).
    pub fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, Error> {
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes); // exactly NONCE_SIZE bytes
        let ct = self.aead.encrypt(nonce, plaintext).map_err(Error::Gcm)?;
        let mut out = Vec::with_capacity(NONCE_SIZE + ct.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Reverses [`SecretBox::seal`]. Returns [`Error::CiphertextTooShort`]
    /// or [`Error::AuthFailed`] if the input is malformed or tampered.
    pub fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, Error> {
        if sealed.len() < NONCE_SIZE + TAG_SIZE {
            return Err(Error::CiphertextTooShort);
        }
        let (nonce_bytes, ciphertext) = sealed.split_at(NONCE_SIZE);
        let nonce = Nonce::from_slice(nonce_bytes);
        self.aead
            .decrypt(nonce, ciphertext)
            .map_err(|_| Error::AuthFailed)
    }
}

/// Reads a base64-encoded 32-byte key from the given env var.
///
/// Returns [`Error::WrongKeyLength`] if the decoded length is not
/// [`KEY_SIZE`]. Empty env values are treated as "not configured" and
/// surface as a clear error rather than silently using a zero key.
pub fn load_key(env_var: &str) -> Result<Vec<u8>, Error> {
    // Go's os.Getenv collapses unset and empty; mirror that.
    let raw = std::env::var(env_var).unwrap_or_default();
    if raw.is_empty() {
        return Err(Error::EnvNotSet(env_var.to_string()));
    }
    let key = STANDARD
        .decode(raw.as_bytes())
        .map_err(|e| Error::InvalidBase64(env_var.to_string(), e))?;
    if key.len() != KEY_SIZE {
        return Err(Error::WrongKeyLength(
            env_var.to_string(),
            key.len(),
            KEY_SIZE,
        ));
    }
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_key() -> Vec<u8> {
        let mut key = vec![0u8; KEY_SIZE];
        OsRng.fill_bytes(&mut key);
        key
    }

    fn must_new_box() -> SecretBox {
        SecretBox::new(&random_key()).expect("New")
    }

    #[test]
    fn round_trip() {
        let b = must_new_box();
        let plaintext = b"lark app_secret 12345";
        let sealed = b.seal(plaintext).expect("Seal");
        let opened = b.open(&sealed).expect("Open");
        assert_eq!(opened, plaintext);
    }

    #[test]
    fn seal_is_non_deterministic() {
        // Same plaintext + same box → different ciphertext on each seal,
        // because the nonce is random. This prevents content-fingerprinting
        // (e.g. confirming that two installations share the same secret).
        let b = must_new_box();
        let plaintext = b"repeat";
        let a = b.seal(plaintext).expect("Seal");
        let c = b.seal(plaintext).expect("Seal");
        assert_ne!(a, c, "expected non-deterministic Seal");
    }

    #[test]
    fn open_rejects_tampered() {
        let b = must_new_box();
        let mut sealed = b.seal(b"important").expect("Seal");
        // Flip a bit in the tag portion (last byte, past the nonce).
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01;
        assert!(
            matches!(b.open(&sealed), Err(Error::AuthFailed)),
            "expected auth failure on tampered ciphertext"
        );
    }

    #[test]
    fn open_rejects_short() {
        let b = must_new_box();
        let err = b.open(b"short").unwrap_err();
        assert!(matches!(err, Error::CiphertextTooShort));
        assert_eq!(err.to_string(), "secretbox: ciphertext too short");
    }

    #[test]
    fn new_rejects_bad_key() {
        let err = SecretBox::new(&[0u8; 16]).unwrap_err();
        assert_eq!(err.to_string(), "secretbox: key must be 32 bytes");
    }

    #[test]
    fn load_key_missing() {
        // Distinct env var names per case: cargo runs tests in parallel
        // threads and process env is global, so sharing one name would race.
        std::env::set_var("TEST_SECRETBOX_KEY_MISSING", "");
        assert!(load_key("TEST_SECRETBOX_KEY_MISSING").is_err());
    }

    #[test]
    fn load_key_bad_base64() {
        std::env::set_var("TEST_SECRETBOX_KEY_B64", "not!base64!");
        assert!(load_key("TEST_SECRETBOX_KEY_B64").is_err());
    }

    #[test]
    fn load_key_wrong_length() {
        let encoded = STANDARD.encode(b"too short");
        std::env::set_var("TEST_SECRETBOX_KEY_LEN", encoded);
        assert!(load_key("TEST_SECRETBOX_KEY_LEN").is_err());
    }

    #[test]
    fn load_key_happy_path() {
        let key = random_key();
        std::env::set_var("TEST_SECRETBOX_KEY_OK", STANDARD.encode(&key));
        let got = load_key("TEST_SECRETBOX_KEY_OK").expect("LoadKey");
        assert_eq!(got, key);
    }

    #[test]
    fn sealed_layout_is_nonce_prefixed_for_go_compat() {
        // Locks the CRITICAL wire contract: sealed output must be exactly
        // nonce(12) || GCM ciphertext || tag(16), where the trailing part
        // decrypts standalone under the leading nonce with no AAD — the
        // exact construction of Go's cipher.AEAD.Seal(nonce, nonce, pt, nil).
        let key = random_key();
        let b = SecretBox::new(&key).expect("New");
        let plaintext = b"lark app_secret 12345";
        let sealed = b.seal(plaintext).expect("Seal");

        assert_eq!(sealed.len(), NONCE_SIZE + plaintext.len() + TAG_SIZE);

        let standalone = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
        let opened = standalone
            .decrypt(
                Nonce::from_slice(&sealed[..NONCE_SIZE]),
                &sealed[NONCE_SIZE..],
            )
            .expect("standalone GCM decrypt of nonce-prefixed payload");
        assert_eq!(opened, plaintext);
    }
}
