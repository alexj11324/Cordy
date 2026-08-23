//! What a callback's aeskey unlocks — port of `media_crypt.go`.
//!
//! A smart-bot message that carries a photo, a file or a video gives the bot
//! two strings: a Tencent COS URL good for five minutes, and the key the
//! bytes behind it are encrypted with. Long-connection mode mints a fresh key
//! per URL, which is the difference from callback mode's one deployment-wide
//! EncodingAESKey — there is nothing to configure and nothing to rotate, but
//! also nothing to fall back on if the key on the frame is unusable.
//!
//! The algorithm is AES-256-CBC with the IV taken from the front of the key
//! itself, and the plaintext PKCS#7-padded to a multiple of 32 bytes
//! (<https://developer.work.weixin.qq.com/document/path/101463>).

use base64::Engine as _;

/// The decoded key length AES-256 requires.
pub const MEDIA_AES_KEY_BYTES: usize = 32;

/// The block the plaintext is padded up to. It is NOT the AES block size, and
/// that gap is the trap: AES works in 16-byte blocks, so a PKCS#7 unpadder
/// written against the cipher rejects any pad longer than 16 — and a file
/// whose length is already a multiple of 32 is padded with a whole 32-byte
/// block. Ordinary payloads land there often enough that such an unpadder
/// looks fine in a smoke test and fails in production.
pub const MEDIA_PAD_BLOCK: usize = 32;

const AES_BLOCK: usize = 16;

/// Turns one downloaded body back into the file the user sent. Every failure
/// is an error rather than a best guess: a wrong answer here is an attachment
/// that opens as garbage, which is worse than an attachment that is honestly
/// missing.
pub fn decrypt_media(aes_key: &str, ciphertext: &[u8]) -> anyhow::Result<Vec<u8>> {
    let key = decode_media_aes_key(aes_key)?;
    if ciphertext.is_empty() {
        anyhow::bail!("wecom: media ciphertext is empty");
    }
    if !ciphertext.len().is_multiple_of(AES_BLOCK) {
        anyhow::bail!(
            "wecom: media ciphertext is {} bytes, not a multiple of the {}-byte AES block",
            ciphertext.len(),
            AES_BLOCK
        );
    }
    // The IV is the front of the key. Reusing key material as an IV is WeCom's
    // choice, not ours; we only have to match it.
    let plain = cbc_decrypt(&key, &key[..AES_BLOCK], ciphertext);
    unpad_media(&plain).map(<[u8]>::to_vec)
}

/// Decodes the base64 the frame carries. Both the padded 44-character form
/// and the unpadded 43-character form appear in WeCom's own surfaces, so both
/// are accepted; anything that does not come out at exactly 32 bytes is
/// refused rather than stretched or truncated into one.
pub fn decode_media_aes_key(raw: &str) -> anyhow::Result<[u8; MEDIA_AES_KEY_BYTES]> {
    let s = raw.trim();
    if s.is_empty() {
        anyhow::bail!("wecom: media aeskey is empty");
    }
    const ENGINES: &[&dyn Base64Decode] = &[&StdB64, &RawStdB64, &UrlB64, &RawUrlB64];
    for enc in ENGINES {
        if let Some(key) = enc.decode_32(s) {
            return Ok(key);
        }
    }
    anyhow::bail!("wecom: media aeskey does not decode to {MEDIA_AES_KEY_BYTES} bytes")
}

trait Base64Decode: Send + Sync {
    fn decode_32(&self, s: &str) -> Option<[u8; MEDIA_AES_KEY_BYTES]>;
}

struct StdB64;
struct RawStdB64;
struct UrlB64;
struct RawUrlB64;

impl Base64Decode for StdB64 {
    fn decode_32(&self, s: &str) -> Option<[u8; 32]> {
        base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .ok()
            .and_then(|v| v.try_into().ok())
    }
}
impl Base64Decode for RawStdB64 {
    fn decode_32(&self, s: &str) -> Option<[u8; 32]> {
        base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(s.as_bytes())
            .ok()
            .and_then(|v| v.try_into().ok())
    }
}
impl Base64Decode for UrlB64 {
    fn decode_32(&self, s: &str) -> Option<[u8; 32]> {
        base64::engine::general_purpose::URL_SAFE
            .decode(s.as_bytes())
            .ok()
            .and_then(|v| v.try_into().ok())
    }
}
impl Base64Decode for RawUrlB64 {
    fn decode_32(&self, s: &str) -> Option<[u8; 32]> {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(s.as_bytes())
            .ok()
            .and_then(|v| v.try_into().ok())
    }
}

/// Strips the PKCS#7 tail, validating every byte of it. A tail that does not
/// check out means the key was wrong or the body was truncated, and either
/// way the bytes in front of it cannot be trusted to be the file.
pub fn unpad_media(plain: &[u8]) -> anyhow::Result<&[u8]> {
    let n = plain.len();
    if n == 0 {
        anyhow::bail!("wecom: media plaintext is empty");
    }
    let pad = plain[n - 1] as usize;
    if !(1..=MEDIA_PAD_BLOCK).contains(&pad) || pad > n {
        anyhow::bail!("wecom: media padding length {pad} is out of range (1..{MEDIA_PAD_BLOCK})");
    }
    if plain[n - pad..].iter().any(|&b| b as usize != pad) {
        anyhow::bail!("wecom: media padding bytes disagree; wrong key or truncated body");
    }
    Ok(&plain[..n - pad])
}

/// Incremental AES-256-CBC decryptor with the IV taken from the front of the
/// key itself (WeCom's construction).
///
/// Port note: Go drives `cipher.NewCBCDecrypter`; Rust chains the raw block
/// decryptor by hand so both the buffered path here and the streaming path in
/// media_stream.rs share one implementation.
pub(crate) struct CbcDecryptor {
    cipher: aes::Aes256,
    prev: [u8; AES_BLOCK],
}

impl CbcDecryptor {
    /// The IV is the front of the key. Reusing key material as an IV is
    /// WeCom's choice, not ours; we only have to match it.
    pub(crate) fn new(key: &[u8; MEDIA_AES_KEY_BYTES]) -> Self {
        Self::with_iv(key, &key[..AES_BLOCK])
    }

    pub(crate) fn with_iv(key: &[u8; MEDIA_AES_KEY_BYTES], iv: &[u8]) -> Self {
        use aes::cipher::KeyInit;
        Self {
            cipher: aes::Aes256::new_from_slice(key).expect("key length checked"),
            prev: iv[..AES_BLOCK].try_into().expect("16-byte slice"),
        }
    }

    /// Decrypts `buf` in place. `buf.len()` MUST be a multiple of the AES
    /// block size; chaining state carries across calls.
    pub(crate) fn decrypt_blocks(&mut self, buf: &mut [u8]) {
        use aes::cipher::{Array, BlockCipherDecrypt};
        for chunk in buf.chunks_exact_mut(AES_BLOCK) {
            // The next block's chain input is this block's CIPHERTEXT, which
            // is exactly what chunk holds before the ECB pass. Snapshot it
            // while chunk is still the only borrow.
            let mut cipher_block = [0u8; AES_BLOCK];
            cipher_block.copy_from_slice(chunk);
            #[allow(deprecated)] // Array::from_mut_slice: no TryFrom path for &mut slices yet
            let block =
                Array::<u8, <aes::Aes256 as aes::cipher::BlockSizeUser>::BlockSize>::from_mut_slice(
                    chunk,
                );
            <aes::Aes256 as BlockCipherDecrypt>::decrypt_block(&self.cipher, block);
            for j in 0..AES_BLOCK {
                block[j] ^= self.prev[j];
            }
            self.prev = cipher_block;
        }
    }
}

/// Decrypts `ciphertext` (a multiple of [`AES_BLOCK`]) under AES-256-CBC in
/// one shot. Returns the full-length plaintext; padding is handled by the
/// caller.
pub(crate) fn cbc_decrypt(
    key: &[u8; MEDIA_AES_KEY_BYTES],
    iv: &[u8],
    ciphertext: &[u8],
) -> Vec<u8> {
    let mut dec = CbcDecryptor::with_iv(key, iv);
    let mut out = ciphertext.to_vec();
    dec.decrypt_blocks(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encrypts with AES-256-CBC + 32-byte PKCS#7 pad for test fixtures.
    fn cbc_encrypt_for_test(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
        use aes::cipher::{Array, KeyInit};
        let pad = MEDIA_PAD_BLOCK - (plaintext.len() % MEDIA_PAD_BLOCK);
        let mut padded = plaintext.to_vec();
        padded.extend(std::iter::repeat_n(pad as u8, pad));
        #[allow(deprecated)] // Array::from_slice: no TryFrom<&[u8; N]> impl yet
        let cipher = aes::Aes256::new(Array::from_slice(key));
        // decrypt_media uses key[..16] as the IV; the fixture must match.
        let mut prev = [0u8; AES_BLOCK];
        prev.copy_from_slice(&key[..AES_BLOCK]);
        let mut out = Vec::with_capacity(padded.len());
        for chunk in padded.chunks_exact(AES_BLOCK) {
            let mut block =
                Array::<u8, <aes::Aes256 as aes::cipher::BlockSizeUser>::BlockSize>::try_from(
                    chunk,
                )
                .unwrap();
            for i in 0..AES_BLOCK {
                block[i] ^= prev[i];
            }
            aes::cipher::BlockCipherEncrypt::encrypt_block(&cipher, &mut block);
            out.extend_from_slice(&block);
            prev.copy_from_slice(&block);
        }
        out
    }

    #[test]
    fn debug_single_block_chain() {
        let key = [3u8; 32];
        let plain = [0x42u8];
        let mut padded = plain.to_vec();
        padded.extend(vec![31u8; 31]);
        use aes::cipher::{Array, KeyInit};
        #[allow(deprecated)] // Array::from_slice: no TryFrom<&[u8; N]> impl yet
        let cipher = aes::Aes256::new(Array::from_slice(&key));
        let mut prev = [0u8; 16];
        prev.copy_from_slice(&key[..16]);
        let mut block =
            Array::<u8, <aes::Aes256 as aes::cipher::BlockSizeUser>::BlockSize>::try_from(
                &padded[0..16],
            )
            .unwrap();
        for i in 0..16 {
            block[i] ^= prev[i];
        }
        aes::cipher::BlockCipherEncrypt::encrypt_block(&cipher, &mut block);
        let ct = block;
        // Now decrypt through the real path
        // Second block: p2 XOR c1, encrypt.
        let mut block2 =
            Array::<u8, <aes::Aes256 as aes::cipher::BlockSizeUser>::BlockSize>::try_from(
                &padded[16..32],
            )
            .unwrap();
        for i in 0..16 {
            block2[i] ^= ct[i];
        }
        aes::cipher::BlockCipherEncrypt::encrypt_block(&cipher, &mut block2);
        let mut full = Vec::new();
        full.extend_from_slice(&ct);
        full.extend_from_slice(&block2);
        let mut dec = crate::media_crypt::CbcDecryptor::with_iv(&key, &key[..16]);
        let mut buf = full.clone();
        dec.decrypt_blocks(&mut buf);
        assert_eq!(buf, padded, "raw padded mismatch: {buf:?} vs {:?}", padded);
    }

    #[test]
    fn decrypt_roundtrips_padded_and_block_multiple_lengths() {
        let key = [3u8; 32];
        let enc = base64::engine::general_purpose::STANDARD.encode(key);
        for size in [1usize, 16, 31, 32, 33, 1000] {
            let plain: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let ct = cbc_encrypt_for_test(&key, &plain);
            let got = decrypt_media(&enc, &ct).unwrap_or_else(|e| panic!("size {size}: {e}"));
            assert_eq!(got, plain, "size {size}");
        }
    }

    #[test]
    fn decrypt_rejects_bad_inputs() {
        assert!(decrypt_media("", &[0u8; 16]).is_err());
        let enc = base64::engine::general_purpose::STANDARD.encode([3u8; 32]);
        assert!(decrypt_media(&enc, &[]).is_err(), "empty ciphertext");
        assert!(
            decrypt_media(&enc, &[0u8; 15]).is_err(),
            "not block aligned"
        );
        // Wrong key → padding check fails.
        let other = base64::engine::general_purpose::STANDARD.encode([4u8; 32]);
        let ct = cbc_encrypt_for_test(&[3u8; 32], b"hello");
        assert!(decrypt_media(&other, &ct).is_err());
    }

    #[test]
    fn aes_key_accepts_all_four_base64_forms() {
        let key = [9u8; 32];
        assert_eq!(
            decode_media_aes_key(&base64::engine::general_purpose::STANDARD.encode(key)).unwrap(),
            key
        );
        assert_eq!(
            decode_media_aes_key(&base64::engine::general_purpose::STANDARD_NO_PAD.encode(key))
                .unwrap(),
            key
        );
        assert_eq!(
            decode_media_aes_key(&base64::engine::general_purpose::URL_SAFE.encode(key)).unwrap(),
            key
        );
        assert_eq!(
            decode_media_aes_key(&base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key))
                .unwrap(),
            key
        );
        assert!(decode_media_aes_key("").is_err());
        assert!(decode_media_aes_key("   ").is_err());
        // Decodes but wrong length.
        assert!(
            decode_media_aes_key(&base64::engine::general_purpose::STANDARD.encode([1u8; 16]))
                .is_err()
        );
        // Not base64 at all.
        assert!(decode_media_aes_key("!!!not-base64!!!").is_err());
    }

    #[test]
    fn unpad_validates_every_byte() {
        // The last byte IS the pad length: pad=1 strips one \x01.
        assert_eq!(unpad_media(b"ab\x01").unwrap(), b"ab");
        // Pad of two strips both tail bytes when they agree.
        let mut p = b"ab".to_vec();
        p.extend([2, 2]);
        assert_eq!(unpad_media(&p).unwrap(), b"ab");
        // A whole 32-byte pad block (length already a multiple of 32) is the
        // trap case the mediaPadBlock constant exists for.
        let full = vec![32u8; 32];
        assert_eq!(unpad_media(&full).unwrap(), b"");
        // Pad byte beyond the 32-byte media block.
        let mut bad = vec![0u8; 40];
        bad[39] = 33;
        assert!(unpad_media(&bad).is_err());
        // Disagreeing pad bytes.
        let mut bad = vec![0u8; 34];
        bad[32] = 2;
        bad[33] = 3;
        assert!(unpad_media(&bad).is_err());
        assert!(unpad_media(&[]).is_err());
    }
}
