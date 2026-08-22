//! Process-identity and id-coercion helpers.
//!
//! Port of `newNodeID` / `uuidString` from
//! `server/internal/integrations/channel/engine/supervisor.go`.

/// Returns a 16-byte hex random string unique to this process. Stored in
/// `channel_installation.ws_lease_token`; matching tokens on a subsequent
/// acquire are treated as renewals (same owner).
///
/// Port note: Go falls back to a timestamp-derived token when crypto/rand
/// fails rather than panicking on boot. Rust's `rand` thread RNG cannot
/// fail, so the fallback branch is unreachable; kept for documentation.
pub fn new_node_id() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}
