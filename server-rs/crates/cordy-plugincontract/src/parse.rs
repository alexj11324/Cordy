//! Strict manifest decoding — port of Go's `ParseManifest`. Unknown fields
//! fail instead of being ignored so a typo cannot silently weaken what the
//! administrator approved.

use serde::Deserialize;

use crate::types::{Manifest, MAX_MANIFEST_SIZE};
use crate::validate::Error;

/// Decodes a strict v1 manifest and returns the canonical bytes an installation
/// stores as its consented snapshot.
pub fn parse_manifest(raw: &[u8]) -> Result<(Manifest, Vec<u8>), Error> {
    if raw.is_empty() {
        return Err(Error::new("plugin manifest is empty"));
    }
    if raw.len() > MAX_MANIFEST_SIZE {
        return Err(Error::new(format!(
            "plugin manifest exceeds {MAX_MANIFEST_SIZE} bytes"
        )));
    }

    let mut deserializer = serde_json::Deserializer::from_slice(raw);
    let manifest = Manifest::deserialize(&mut deserializer)
        .map_err(|e| Error::new(format!("decode plugin manifest: {e}")))?;
    // Refuses both a second JSON value and malformed trailing data, matching
    // Go's rejectTrailingJSON.
    deserializer
        .end()
        .map_err(|e| Error::new(format!("decode trailing plugin manifest data: {e}")))?;
    manifest.validate()?;

    let canonical = serde_json::to_vec(&manifest)
        .map_err(|e| Error::new(format!("canonicalize plugin manifest: {e}")))?;
    Ok((manifest, canonical))
}
