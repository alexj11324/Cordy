//! Chunk assembler — port of
//! `server/internal/integrations/lark/ws_chunk_assembler.go`.
//!
//! Buffers multi-frame Lark data payloads keyed by message_id and returns
//! the concatenated bytes once every chunk has arrived. Lark splits large
//! event payloads across multiple binary Frames with the headers:
//!
//! - sum        — total number of chunks (>=2 means multi-frame)
//! - seq        — 0-based index of THIS chunk within the message
//! - message_id — common key across the N chunks
//!
//! The SDK reference (larksuite/oapi-sdk-go/v3/ws client combine()) uses a
//! 5-second TTL on partial state — anything older than that is considered
//! abandoned and dropped. Without TTL, a Lark-side packet drop on an
//! intermediate chunk would leak the buffered bytes forever.
//!
//! The assembler is thread-safe: a single instance serves every supervisor
//! task. State lives in-process only — Frame chunks do not arrive across
//! server restarts (Lark re-sends the full event on reconnect), so
//! durability is not required.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct ChunkEntry {
    /// Indexed by seq; None slots = still missing.
    chunks: Vec<Option<Vec<u8>>>,
    /// Count of non-empty slots.
    received: usize,
    deadline: Instant,
}

/// Returns an assembler with the given partial-state TTL. A non-positive ttl
/// falls back to the SDK default (5s).
pub struct ChunkAssembler {
    ttl: Duration,
    buf: Mutex<HashMap<String, ChunkEntry>>,
}

impl ChunkAssembler {
    pub fn new(ttl: Duration) -> Self {
        let ttl = if ttl.is_zero() {
            Duration::from_secs(5)
        } else {
            ttl
        };
        Self {
            ttl,
            buf: Mutex::new(HashMap::new()),
        }
    }

    /// Records a single chunk and returns:
    ///
    /// - `Some(payload)` — every chunk has now arrived; payload is the
    ///   concatenated bytes in seq order and the per-message entry has been
    ///   removed.
    /// - `None`          — partial state; caller should NOT emit yet and
    ///   SHOULD NOT ACK this frame (mirroring SDK behaviour where ACK only
    ///   fires after full assembly so the server can retry the whole event).
    ///
    /// admit rejects malformed inputs (sum<=0, seq<0, seq>=sum) by returning
    /// None and treating the chunk as ignored. In production these conditions
    /// never fire because Lark enforces them server-side, but the function
    /// stays defensive — one malformed header must not corrupt the buffer for
    /// the next event.
    pub fn admit(
        &self,
        message_id: &str,
        sum: usize,
        seq: usize,
        payload: &[u8],
    ) -> Option<Vec<u8>> {
        if message_id.is_empty() || sum == 0 || seq >= sum {
            return None;
        }
        let mut buf = self.buf.lock().unwrap();

        // Lazy GC every admit: cheap (single map walk) and avoids needing a
        // separate sweeper task. Bounded by the live message_id count, which
        // is small (Lark caps in-flight chunked events per connection).
        self.gc_expired_locked(&mut buf);

        if buf
            .get(message_id)
            .is_some_and(|entry| entry.chunks.len() != sum)
        {
            buf.remove(message_id);
            return None;
        }

        let entry = buf.entry(message_id.to_string()).or_insert_with(|| {
            let mut chunks = Vec::with_capacity(sum);
            chunks.resize_with(sum, || None);
            ChunkEntry {
                chunks,
                received: 0,
                deadline: Instant::now() + self.ttl,
            }
        });
        // Duplicate chunk (network retry / out-of-order Lark resend): we
        // silently overwrite. Lark guarantees the payload bytes are stable
        // for a given (message_id, seq), so re-admitting cannot change the
        // final assembled output.
        if entry.chunks[seq].is_none() {
            entry.received += 1;
        }
        entry.chunks[seq] = Some(payload.to_vec());
        // Sliding deadline: every fresh chunk extends the per-message TTL
        // because Lark might pace a multi-frame event across several hundred
        // ms; the static 5s is for "we got chunk 0 and then nothing", not
        // "chunks 0..N-1 arrived steadily over 4.9s".
        entry.deadline = Instant::now() + self.ttl;

        if entry.received < entry.chunks.len() {
            return None;
        }

        let mut out = Vec::with_capacity(
            entry
                .chunks
                .iter()
                .map(|c| c.as_ref().map_or(0, |v| v.len()))
                .sum(),
        );
        for c in &entry.chunks {
            out.extend_from_slice(c.as_deref().unwrap_or(&[]));
        }
        buf.remove(message_id);
        Some(out)
    }

    /// Removes entries whose deadline has passed. Exposed for tests;
    /// production runs it lazily in [`admit`](Self::admit).
    pub fn gc_expired(&self) -> usize {
        let mut buf = self.buf.lock().unwrap();
        self.gc_expired_locked(&mut buf)
    }

    fn gc_expired_locked(&self, buf: &mut HashMap<String, ChunkEntry>) -> usize {
        let now = Instant::now();
        let before = buf.len();
        buf.retain(|_, e| e.deadline > now);
        before - buf.len()
    }

    /// Reports the number of partially-assembled messages currently buffered.
    /// Used by tests; useful for ops dashboards.
    pub fn pending_count(&self) -> usize {
        self.buf.lock().unwrap().len()
    }
}

/// Extracts the chunking metadata from a Frame's headers. Missing or
/// unparseable headers yield (sum=0, seq=0, ""), which the connector reads as
/// "single-frame event" and bypasses the assembler entirely. Lark's docs
/// state sum is omitted (effectively 1) for non-chunked events; SDK's GetInt
/// returns 0 on missing header.
pub fn parse_chunk_headers(f: &crate::ws_frame::Frame) -> (usize, usize, String) {
    let mut sum = 0usize;
    let mut seq = 0usize;
    if let Ok(n) = f
        .header_value(crate::ws_frame::FRAME_HEADER_SUM_KEY)
        .parse::<usize>()
    {
        sum = n;
    }
    if let Ok(n) = f
        .header_value(crate::ws_frame::FRAME_HEADER_SEQ_KEY)
        .parse::<usize>()
    {
        seq = n;
    }
    let message_id = f
        .header_value(crate::ws_frame::FRAME_HEADER_MESSAGE_ID_KEY)
        .to_string();
    (sum, seq, message_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ws_frame::{Frame, FrameHeader};

    #[test]
    fn assembles_in_seq_order_and_clears_state() {
        let a = ChunkAssembler::new(Duration::from_secs(1));
        assert_eq!(a.admit("m", 3, 2, b"c"), None);
        assert_eq!(a.pending_count(), 1);
        assert_eq!(a.admit("m", 3, 0, b"a"), None);
        assert_eq!(a.pending_count(), 1);
        assert_eq!(a.pending_count(), 1);
        let out = a.admit("m", 3, 1, b"b").expect("complete");
        assert_eq!(out, b"abc");
        assert_eq!(a.pending_count(), 0);
    }

    #[test]
    fn duplicate_chunk_does_not_double_count() {
        let a = ChunkAssembler::new(Duration::from_secs(1));
        assert_eq!(a.admit("m", 2, 0, b"x"), None);
        // Network retry of chunk 0: overwrite, still partial.
        assert_eq!(a.admit("m", 2, 0, b"x"), None);
        assert_eq!(a.pending_count(), 1);
        assert_eq!(a.admit("m", 2, 1, b"y"), Some(b"xy".to_vec()));
    }

    #[test]
    fn inconsistent_chunk_count_is_dropped_without_indexing() {
        let a = ChunkAssembler::new(Duration::from_secs(1));
        assert_eq!(a.admit("m", 2, 0, b"a"), None);
        assert_eq!(a.admit("m", 3, 2, b"c"), None);
        assert_eq!(a.pending_count(), 0);
    }

    #[test]
    fn malformed_headers_are_ignored() {
        let a = ChunkAssembler::new(Duration::from_secs(1));
        assert_eq!(a.admit("", 2, 0, b"x"), None);
        assert_eq!(a.admit("m", 0, 0, b"x"), None);
        assert_eq!(a.admit("m", 2, 2, b"x"), None); // seq >= sum
        assert_eq!(a.pending_count(), 0);
    }

    #[test]
    fn expired_partial_state_is_garbage_collected() {
        let a = ChunkAssembler::new(Duration::from_millis(10));
        assert_eq!(a.admit("m", 2, 0, b"x"), None);
        assert_eq!(a.pending_count(), 1);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(a.gc_expired(), 1);
        assert_eq!(a.pending_count(), 0);
    }

    #[test]
    fn parse_chunk_headers_defaults_to_single_frame() {
        let f = Frame::default();
        let (sum, seq, id) = parse_chunk_headers(&f);
        assert_eq!((sum, seq, id.as_str()), (0, 0, ""));

        let f = Frame {
            headers: vec![
                FrameHeader::new("sum", "3"),
                FrameHeader::new("seq", "1"),
                FrameHeader::new("message_id", "m/9"),
            ],
            ..Frame::default()
        };
        let (sum, seq, id) = parse_chunk_headers(&f);
        assert_eq!((sum, seq, id.as_str()), (3, 1, "m/9"));

        // Unparseable values fall back to zero (SDK GetInt behaviour).
        let f = Frame {
            headers: vec![FrameHeader::new("sum", "abc")],
            ..Frame::default()
        };
        let (sum, _, _) = parse_chunk_headers(&f);
        assert_eq!(sum, 0);
    }
}
