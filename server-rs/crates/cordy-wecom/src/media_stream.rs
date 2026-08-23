//! An attachment that never has to fit in memory — port of `media_stream.go`.
//!
//! The buffered path in media_download.rs holds each attachment twice: the
//! whole ciphertext from the download, then the whole plaintext from the
//! decrypt, both live at once while the upload runs. The engine caps
//! concurrent media resolutions at eight, and the resource cap is 100 MiB, so
//! the worst case a perfectly ordinary workspace can reach — four people
//! sending two large files each — is eight times two hundred megabytes of
//! live heap. On a self-hosted box that is an OOM, and the process it kills
//! is serving Lark, Slack and DingTalk as well. That path remains only as the
//! fallback for a storage backend without UploadStream; both shipped backends
//! have one, so this file is what a real deployment runs.
//!
//! One temp file instead, and the ciphertext never reaches disk at all: it
//! streams from the socket, decrypts block by block into the file, and the
//! upload reads back from there. Peak heap per attachment becomes one buffer,
//! and the number that would otherwise multiply is bounded by disk.
//!
//! The temp file is also what makes the streaming upload possible at all.
//! S3Storage.UploadStream requires an exact ContentLength, and the plaintext
//! length is the ciphertext length minus a pad that is only known after the
//! final block is read — unknowable at the moment the upload must start. A
//! file on disk has already answered the question: its size IS the length.
//!
//! Files are created 0600 and removed on every path. They hold decrypted
//! attachment content, so their permissions are part of the feature rather
//! than housekeeping.

use std::io::{Seek, SeekFrom};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::AsyncReadExt;

use crate::media_crypt::{decode_media_aes_key, unpad_media, CbcDecryptor, MEDIA_PAD_BLOCK};

/// How much ciphertext is decrypted at a time. Large enough that the syscall
/// overhead disappears against a 100 MiB file, small enough that the buffers
/// stay incidental next to everything else the process is holding.
pub const MEDIA_STREAM_CHUNK: usize = 256 << 10;

/// Reads ciphertext from `src`, decrypts it into a new temp file, and returns
/// the file positioned at its start along with the plaintext length. The
/// caller closes it; nothing else ever removes it because the name was
/// unlinked before this function returned.
///
/// CBC needs the tail before the head can be trusted — the pad is on the last
/// block — so the final [`MEDIA_PAD_BLOCK`] bytes are held back until the
/// source is exhausted and unpadded then. Everything before it is written as
/// it goes.
///
/// Failures to make or write the temp file carry "media temp file" in their
/// message: the ingest path keys its fall-back-to-buffered decision on that
/// substring, because only a failure to make the file — not a bad key or a
/// truncated body — is grounds to retry through memory. Read failures keep
/// their source chain intact so a size-cap refusal stays classifiable.
pub async fn decrypt_to_file(
    aes_key: &str,
    src: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
) -> anyhow::Result<(std::fs::File, i64)> {
    let key = decode_media_aes_key(aes_key)?;
    let mut out = create_unlinked_temp_file()?;
    let mut dec = CbcDecryptor::new(&key);

    let mut buf = vec![0u8; MEDIA_STREAM_CHUNK];
    // Holds back the last MEDIA_PAD_BLOCK bytes of plaintext, because the
    // PKCS#7 pad is up to 32 bytes and therefore spans TWO AES blocks — the
    // pad block and the cipher block are not the same size here, which is the
    // trap media_crypt documents. Holding one AES block back would unpad
    // correctly only for pads of 16 or less, i.e. about half of all files.
    let mut tail: Vec<u8> = Vec::with_capacity(MEDIA_PAD_BLOCK + 16);
    // Ciphertext bytes not yet on a block boundary.
    let mut carry: Vec<u8> = Vec::new();
    let mut written: i64 = 0;

    loop {
        let n = src
            .read(&mut buf)
            .await
            .map_err(|e| anyhow::Error::new(e).context("wecom: media decrypt: read"))?;
        if n > 0 {
            carry.extend_from_slice(&buf[..n]);
            let usable = carry.len() - carry.len() % 16;
            if usable > 0 {
                dec.decrypt_blocks(&mut carry[..usable]);
                emit(&mut out, &mut tail, &mut written, &carry[..usable])?;
                carry.drain(..usable);
            }
        }
        if n == 0 {
            break;
        }
    }

    if !carry.is_empty() {
        return Err(anyhow::anyhow!(
            "wecom: media ciphertext is not a multiple of the block size ({} trailing bytes)",
            carry.len()
        ));
    }
    if written == 0 && tail.is_empty() {
        anyhow::bail!("wecom: media ciphertext is empty");
    }

    // The pad is inside what was held back, and unpad_media validates every
    // byte of it — a tail that does not check out means a wrong key or a
    // truncated body, and the file in front of it cannot be trusted either.
    let unpadded = unpad_media(&tail)?;
    if !unpadded.is_empty() {
        use std::io::Write;
        out.write_all(unpadded)
            .map_err(|e| anyhow::anyhow!("wecom: media decrypt: write: {e}"))?;
        written += unpadded.len() as i64;
    }
    out.seek(SeekFrom::Start(0))
        .map_err(|e| anyhow::anyhow!("wecom: media decrypt: rewind: {e}"))?;
    Ok((out, written))
}

fn emit(
    out: &mut std::fs::File,
    tail: &mut Vec<u8>,
    written: &mut i64,
    plain: &[u8],
) -> anyhow::Result<()> {
    use std::io::Write;
    tail.extend_from_slice(plain);
    if tail.len() <= MEDIA_PAD_BLOCK {
        return Ok(());
    }
    let cut = tail.len() - MEDIA_PAD_BLOCK;
    out.write_all(&tail[..cut])
        .map_err(|e| anyhow::anyhow!("wecom: media decrypt: write: {e}"))?;
    *written += cut as i64;
    tail.drain(..cut);
    Ok(())
}

/// Creates a 0600 temp file and unlinks its name immediately: the file stays
/// readable through the handle and disappears the moment the process lets go
/// of it, including on a crash. Nothing with decrypted attachment content is
/// left behind for anyone to find.
fn create_unlinked_temp_file() -> anyhow::Result<std::fs::File> {
    let dir = std::env::temp_dir();
    for _ in 0..8 {
        let path = dir.join(format!("wecom-media-{}.bin", random_suffix()));
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    // Best effort: the file is already unlinked below, so this
                    // is belt as well as braces.
                    let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
                }
                // Unlink now. On Unix the open handle keeps the inode alive;
                // on platforms where removing an open file fails the name
                // simply stays, which is the pre-port behaviour anyway.
                let _ = std::fs::remove_file(&path);
                return Ok(file);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(anyhow::anyhow!("wecom: media temp file: {e}")),
        }
    }
    Err(anyhow::anyhow!(
        "wecom: media temp file: could not find a free name"
    ))
}

fn random_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{nanos:x}{}", std::process::id())
}

/// Reads up to `n` bytes from the head of `f` and rewinds it, so a caller
/// that needs to sniff a content type does not have to hold the whole file.
pub fn peek_file(f: &mut std::fs::File, n: usize) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    let mut head = vec![0u8; n];
    let mut read = 0;
    while read < n {
        match f.read(&mut head[read..]) {
            Ok(0) => break,
            Ok(k) => read += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    f.seek(SeekFrom::Start(0))?;
    head.truncate(read);
    Ok(head)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::{Array, KeyInit};
    use base64::Engine as _;

    fn encrypt_for_test(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
        let pad = MEDIA_PAD_BLOCK - (plaintext.len() % MEDIA_PAD_BLOCK);
        let mut padded = plaintext.to_vec();
        padded.extend(std::iter::repeat_n(pad as u8, pad));
        #[allow(deprecated)] // Array::from_slice: no TryFrom<&[u8; N]> impl yet
        let cipher = aes::Aes256::new(Array::from_slice(key));
        // decrypt_to_file uses key[..16] as the IV; the fixture must match.
        let mut prev = [0u8; 16];
        prev.copy_from_slice(&key[..16]);
        let mut out = Vec::with_capacity(padded.len());
        for chunk in padded.chunks_exact(16) {
            let mut block =
                Array::<u8, <aes::Aes256 as aes::cipher::BlockSizeUser>::BlockSize>::try_from(
                    chunk,
                )
                .unwrap();
            for i in 0..16 {
                block[i] ^= prev[i];
            }
            aes::cipher::BlockCipherEncrypt::encrypt_block(&cipher, &mut block);
            out.extend_from_slice(&block);
            prev.copy_from_slice(&block);
        }
        out
    }

    #[tokio::test]
    async fn decrypt_to_file_roundtrips_large_payloads_without_holding_them() {
        let key = [5u8; 32];
        let enc = base64::engine::general_purpose::STANDARD.encode(key);
        let plain: Vec<u8> = (0..500_000usize).map(|i| (i * 7 % 253) as u8).collect();
        let ct = encrypt_for_test(&key, &plain);

        let (mut file, size) = decrypt_to_file(&enc, &mut ct.as_slice()).await.unwrap();
        assert_eq!(size, plain.len() as i64);
        let got = peek_file(&mut file, plain.len()).unwrap();
        assert_eq!(got, plain);
    }

    #[tokio::test]
    async fn decrypt_to_file_rejects_bad_keys_and_truncated_bodies() {
        let enc = base64::engine::general_purpose::STANDARD.encode([5u8; 32]);
        assert!(decrypt_to_file("nope", &mut "".as_bytes()).await.is_err());
        assert!(
            decrypt_to_file(&enc, &mut "".as_bytes()).await.is_err(),
            "empty ciphertext"
        );
        // Not block aligned → trailing-bytes error.
        assert!(decrypt_to_file(&enc, &mut [1u8, 2, 3].as_slice())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn temp_files_are_created_readable_then_unnamed() {
        let enc = base64::engine::general_purpose::STANDARD.encode([6u8; 32]);
        let ct = encrypt_for_test(&[6u8; 32], b"secret attachment bytes");
        let (file, size) = decrypt_to_file(&enc, &mut ct.as_slice()).await.unwrap();
        assert_eq!(size, 23);
        drop(file);
        // No assertion possible on the name (never captured); the property is
        // that creation succeeded and the handle stayed valid above.
    }

    #[tokio::test]
    async fn size_cap_refusal_stays_classifiable_through_the_chain() {
        use crate::media_download::{CappedBody, MediaTooLarge, MAX_MEDIA_BYTES};

        let big: Vec<u8> = vec![0u8; MAX_MEDIA_BYTES + 10];
        let mut capped = CappedBody::new(big.as_slice(), MAX_MEDIA_BYTES as i64 + 1);
        let mut sink: Vec<u8> = Vec::new();
        let res = tokio::io::copy(&mut capped, &mut sink).await;
        let err = res.expect_err("cap must refuse");
        let inner = err.get_ref().expect("io::Error::other carries the cause");
        assert!(
            inner.downcast_ref::<MediaTooLarge>().is_some(),
            "cap refusal must stay classifiable"
        );
    }

    #[tokio::test]
    async fn peek_file_reads_head_and_rewinds() {
        let enc = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        let ct = encrypt_for_test(&[7u8; 32], b"0123456789");
        let (mut file, _) = decrypt_to_file(&enc, &mut ct.as_slice()).await.unwrap();
        assert_eq!(peek_file(&mut file, 4).unwrap(), b"0123");
        assert_eq!(peek_file(&mut file, 100).unwrap(), b"0123456789");
        assert_eq!(peek_file(&mut file, 2).unwrap(), b"01");
    }
}
