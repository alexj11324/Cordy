//! Wire-compatible re-implementation of the Lark/Feishu long-connection
//! binary Frame envelope — port of
//! `server/internal/integrations/lark/ws_frame.go`.
//!
//! The encoded bytes are byte-identical to what the official SDK's
//! github.com/larksuite/oapi-sdk-go/v3/ws Frame produces, which matters
//! because Lark's server rejects frames whose required fields are missing —
//! and the SDK is generated from pbbp2.proto using proto2 + gogo `req`
//! semantics, so SeqID / LogID / Service / Method are emitted unconditionally
//! (even when zero) and the opt string fields (payload_encoding /
//! payload_type / log_id_new) are also emitted unconditionally because gogo's
//! generated code skips the zero-guard for opt strings on this message. Only
//! payload uses the `nil` guard. See the SDK's pbbp2.pb.go
//! MarshalToSizedBuffer for the reference; mismatches against that function
//! silently corrupt the stream because round-tripping against our own
//! unmarshal masks the bug.
//!
//! We hand-roll the protobuf wire codec (varint tags + length-delimited
//! fields) rather than pulling a full prost/protobuf dependency: a 9-field
//! message is bounded, and the golden tests pin the exact byte sequence the
//! SDK would produce for canonical frames — that is the load-bearing
//! compatibility check.

/// Identifies a frame whose Method=Control(0). Control frames carry
/// ping/pong and server-pushed ClientConfig updates; they never carry an
/// inbound event payload.
pub const FRAME_METHOD_CONTROL: i32 = 0;

/// Identifies a frame whose Method=Data(1). Data frames carry the actual
/// event payload (im.message.receive_v1, card interaction, etc.) and require
/// an ACK response.
pub const FRAME_METHOD_DATA: i32 = 1;

/// Header key Lark puts the frame type under; drives per-frame routing.
pub const FRAME_HEADER_TYPE_KEY: &str = "type";
pub const FRAME_HEADER_TYPE_EVENT: &str = "event";
pub const FRAME_HEADER_TYPE_CARD: &str = "card";
pub const FRAME_HEADER_TYPE_PING: &str = "ping";
pub const FRAME_HEADER_TYPE_PONG: &str = "pong";

/// The dedup / chunk key Lark sets on each data frame; reused as-is in the
/// ACK so the server can correlate.
pub const FRAME_HEADER_MESSAGE_ID_KEY: &str = "message_id";

/// Chunking metadata for multi-frame payloads (sum>1 means N chunks indexed
/// by seq). The chunk assembler in [`crate::ws_chunk_assembler`] uses these
/// to reassemble a single JSON payload from multiple Frames before invoking
/// the decoder.
pub const FRAME_HEADER_SUM_KEY: &str = "sum";
pub const FRAME_HEADER_SEQ_KEY: &str = "seq";

/// One (key, value) pair in Frame.headers. Equivalent to the SDK's
/// pbbp2.Header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameHeader {
    pub key: String,
    pub value: String,
}

impl FrameHeader {
    pub fn new(key: &str, value: &str) -> Self {
        Self {
            key: key.to_string(),
            value: value.to_string(),
        }
    }
}

/// Mirrors pbbp2.Frame. Field numbers match the SDK proto so the on-wire
/// bytes are byte-identical to what oapi-sdk-go produces.
///
/// The marshal implementation matches the SDK's gogo-generated code:
/// seq_id, log_id, service, method are emitted unconditionally as proto2
/// required fields; payload_encoding, payload_type, log_id_new are emitted
/// unconditionally with zero-length values when empty (matches the generated
/// code's unconditional copy + length-encoding); payload is emitted only when
/// present. The golden tests pin exact byte sequences.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frame {
    /// proto field 1 (req)
    pub seq_id: u64,
    /// proto field 2 (req)
    pub log_id: u64,
    /// proto field 3 (req)
    pub service: i32,
    /// proto field 4 (req)
    pub method: i32,
    /// proto field 5 (rep)
    pub headers: Vec<FrameHeader>,
    /// proto field 6 (opt)
    pub payload_encoding: String,
    /// proto field 7 (opt)
    pub payload_type: String,
    /// proto field 8 (opt) — None omits the tag entirely; Some emits even
    /// when empty.
    pub payload: Option<Vec<u8>>,
    /// proto field 9 (opt)
    pub log_id_new: String,
}

impl Frame {
    /// Returns the value for the first header with the supplied key, or ""
    /// if absent. Lark uses headers as a flat map, but the SDK's proto schema
    /// is a repeated field — we treat duplicates as "first wins" because
    /// that's what the SDK does in practice.
    pub fn header_value(&self, key: &str) -> &str {
        self.headers
            .iter()
            .find(|h| h.key == key)
            .map(|h| h.value.as_str())
            .unwrap_or("")
    }

    /// Encodes the frame to the wire format Lark expects. The returned bytes
    /// are sent verbatim as the WebSocket binary payload.
    ///
    /// Field emission order matches the SDK's MarshalToSizedBuffer reverse
    /// build (fields 1→9 in the final byte stream). DO NOT change which
    /// fields are emitted unconditionally without re-checking the SDK's
    /// generated MarshalToSizedBuffer — it is the authority on wire shape and
    /// divergence is invisible until Lark's server starts dropping frames in
    /// production.
    pub fn marshal(&self) -> Vec<u8> {
        let mut buf: Vec<u8> =
            Vec::with_capacity(64 + self.payload.as_ref().map_or(0, |p| p.len()));

        // Required fields (proto2 req): always emit tag + varint, even when
        // the value is zero. The SDK's generated code unconditionally writes
        // these; a Lark server reading a Frame missing one of them returns a
        // RequiredNotSetError and discards the frame.
        append_tag(&mut buf, 1, WireType::Varint);
        append_varint(&mut buf, self.seq_id);
        append_tag(&mut buf, 2, WireType::Varint);
        append_varint(&mut buf, self.log_id);
        append_tag(&mut buf, 3, WireType::Varint);
        append_varint(&mut buf, self.service as u32 as u64);
        append_tag(&mut buf, 4, WireType::Varint);
        append_varint(&mut buf, self.method as u32 as u64);

        // Repeated headers — emit one length-prefixed entry per FrameHeader.
        // Empty headers list emits nothing, matching the SDK guard.
        for h in &self.headers {
            append_tag(&mut buf, 5, WireType::Bytes);
            append_varint(&mut buf, header_size(h) as u64);
            // Header.key (field 1) and Header.value (field 2) are both
            // proto2 req — emit unconditionally.
            append_tag(&mut buf, 1, WireType::Bytes);
            append_bytes(&mut buf, h.key.as_bytes());
            append_tag(&mut buf, 2, WireType::Bytes);
            append_bytes(&mut buf, h.value.as_bytes());
        }

        // PayloadEncoding (field 6) — gogo's generated code copies the string
        // unconditionally and emits the tag + length prefix even when len==0.
        // Empty string still produces tag + zero-length.
        append_tag(&mut buf, 6, WireType::Bytes);
        append_bytes(&mut buf, self.payload_encoding.as_bytes());

        // PayloadType (field 7) — same unconditional emission.
        append_tag(&mut buf, 7, WireType::Bytes);
        append_bytes(&mut buf, self.payload_type.as_bytes());

        // Payload (field 8) — the SDK uses `if m.Payload != nil`. We mirror
        // that: None omits the tag entirely; Some emits, even if len==0.
        if let Some(payload) = &self.payload {
            append_tag(&mut buf, 8, WireType::Bytes);
            append_bytes(&mut buf, payload);
        }

        // LogIDNew (field 9) — unconditional like payload_encoding/type.
        append_tag(&mut buf, 9, WireType::Bytes);
        append_bytes(&mut buf, self.log_id_new.as_bytes());

        buf
    }
}

fn header_size(h: &FrameHeader) -> usize {
    // Both fields are req and always emitted; their byte cost is
    // tag + length-prefix(len) + len.
    size_tag(1) + size_bytes(h.key.len()) + size_tag(2) + size_bytes(h.value.len())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireType {
    Varint,
    Bytes,
}

impl WireType {
    fn as_u32(self) -> u32 {
        match self {
            WireType::Varint => 0,
            WireType::Bytes => 2,
        }
    }
}

fn append_tag(buf: &mut Vec<u8>, field_number: u32, wire_type: WireType) {
    append_varint(buf, ((field_number << 3) | wire_type.as_u32()) as u64);
}

fn append_varint(buf: &mut Vec<u8>, mut v: u64) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        buf.push(b);
        if v == 0 {
            break;
        }
    }
}

fn append_bytes(buf: &mut Vec<u8>, b: &[u8]) {
    append_varint(buf, b.len() as u64);
    buf.extend_from_slice(b);
}

fn size_tag(field_number: u32) -> usize {
    varint_len(((field_number << 3) | 2) as u64)
}

fn size_bytes(len: usize) -> usize {
    varint_len(len as u64) + len
}

fn varint_len(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

#[derive(Debug)]
struct WireReader<'a> {
    buf: &'a [u8],
}

impl<'a> WireReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    fn remaining(&self) -> bool {
        !self.buf.is_empty()
    }

    fn read_varint(&mut self) -> Result<u64, FrameError> {
        let mut result: u64 = 0;
        let mut shift = 0u32;
        loop {
            let Some(&b) = self.buf.first() else {
                return Err(FrameError::Truncated("varint"));
            };
            self.buf = &self.buf[1..];
            if shift >= 64 {
                return Err(FrameError::Overflow);
            }
            result |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
    }

    fn read_tag(&mut self) -> Result<(u32, WireType), FrameError> {
        let tag = self.read_varint()?;
        let num = (tag >> 3) as u32;
        if num == 0 {
            return Err(FrameError::ZeroFieldNumber);
        }
        let wt = tag & 0x7;
        match wt {
            0 => Ok((num, WireType::Varint)),
            2 => Ok((num, WireType::Bytes)),
            _ => Err(FrameError::UnsupportedWireType((tag & 0x7) as u32)),
        }
    }

    fn read_len_delimited(&mut self) -> Result<&'a [u8], FrameError> {
        let len = self.read_varint()? as usize;
        if len > self.buf.len() {
            return Err(FrameError::Truncated("length-delimited"));
        }
        let out = &self.buf[..len];
        self.buf = &self.buf[len..];
        Ok(out)
    }

    /// Skips one field value of the given wire type (proto3 unknown-field
    /// behaviour).
    fn skip_value(&mut self, wire_type: WireType) -> Result<(), FrameError> {
        match wire_type {
            WireType::Varint => {
                self.read_varint()?;
                Ok(())
            }
            WireType::Bytes => {
                self.read_len_delimited()?;
                Ok(())
            }
        }
    }
}

/// Parse failures from [`unmarshal_frame`]. The caller (the WS connector)
/// treats the frame as bad and drops it without tearing down the connection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("ws frame: empty buffer")]
    Empty,
    #[error("ws frame: truncated {0}")]
    Truncated(&'static str),
    #[error("ws frame: invalid field number 0")]
    ZeroFieldNumber,
    #[error("ws frame: unsupported wire type {0}")]
    UnsupportedWireType(u32),
    #[error("ws frame: varint overflow")]
    Overflow,
    #[error("ws frame: field {field} expects {expected}, got wire type {got}")]
    WrongWireType {
        field: u32,
        expected: &'static str,
        got: u32,
    },
}

/// Parses one binary protobuf message into a Frame. Unknown fields are
/// skipped (proto3 behaviour) so server-side schema additions do not break
/// us. Truncated / malformed bytes return an error and the caller (the WS
/// connector) treats the frame as bad and drops it without tearing down the
/// connection.
pub fn unmarshal_frame(b: &[u8]) -> Result<Frame, FrameError> {
    if b.is_empty() {
        return Err(FrameError::Empty);
    }
    let mut f = Frame::default();
    let mut r = WireReader::new(b);
    while r.remaining() {
        let (num, typ) = r.read_tag()?;
        match num {
            1 => {
                // SeqID uint64
                let WireType::Varint = typ else {
                    return Err(wrong_wire(1, "varint", typ));
                };
                f.seq_id = r.read_varint().map_err(|e| e.relabel("seq_id"))?;
            }
            2 => {
                // LogID uint64
                let WireType::Varint = typ else {
                    return Err(wrong_wire(2, "varint", typ));
                };
                f.log_id = r.read_varint().map_err(|e| e.relabel("log_id"))?;
            }
            3 => {
                // Service int32
                let WireType::Varint = typ else {
                    return Err(wrong_wire(3, "varint", typ));
                };
                f.service = r.read_varint().map_err(|e| e.relabel("service"))? as u32 as i32;
            }
            4 => {
                // Method int32
                let WireType::Varint = typ else {
                    return Err(wrong_wire(4, "varint", typ));
                };
                f.method = r.read_varint().map_err(|e| e.relabel("method"))? as u32 as i32;
            }
            5 => {
                // Headers (repeated)
                let WireType::Bytes = typ else {
                    return Err(wrong_wire(5, "bytes", typ));
                };
                let hb = r.read_len_delimited().map_err(|e| e.relabel("header"))?;
                f.headers.push(unmarshal_header(hb)?);
            }
            6 => {
                let WireType::Bytes = typ else {
                    return Err(wrong_wire(6, "bytes", typ));
                };
                let s = r
                    .read_len_delimited()
                    .map_err(|e| e.relabel("payload_encoding"))?;
                f.payload_encoding = String::from_utf8_lossy(s).into_owned();
            }
            7 => {
                let WireType::Bytes = typ else {
                    return Err(wrong_wire(7, "bytes", typ));
                };
                let s = r
                    .read_len_delimited()
                    .map_err(|e| e.relabel("payload_type"))?;
                f.payload_type = String::from_utf8_lossy(s).into_owned();
            }
            8 => {
                let WireType::Bytes = typ else {
                    return Err(wrong_wire(8, "bytes", typ));
                };
                let raw = r.read_len_delimited().map_err(|e| e.relabel("payload"))?;
                // Copy out so the Frame outlives the input buffer.
                f.payload = Some(raw.to_vec());
            }
            9 => {
                let WireType::Bytes = typ else {
                    return Err(wrong_wire(9, "bytes", typ));
                };
                let s = r
                    .read_len_delimited()
                    .map_err(|e| e.relabel("log_id_new"))?;
                f.log_id_new = String::from_utf8_lossy(s).into_owned();
            }
            _ => {
                r.skip_value(typ)
                    .map_err(|e| e.relabel("skip unknown field"))?;
            }
        }
    }
    Ok(f)
}

fn wrong_wire(field: u32, expected: &'static str, got: WireType) -> FrameError {
    FrameError::WrongWireType {
        field,
        expected,
        got: got.as_u32(),
    }
}

impl FrameError {
    fn relabel(self, label: &'static str) -> FrameError {
        match self {
            FrameError::Truncated(_) => FrameError::Truncated(label),
            other => other,
        }
    }
}

fn unmarshal_header(b: &[u8]) -> Result<FrameHeader, FrameError> {
    let mut h = FrameHeader::new("", "");
    let mut r = WireReader::new(b);
    while r.remaining() {
        let (num, typ) = r.read_tag()?;
        match num {
            1 => {
                let WireType::Bytes = typ else {
                    return Err(wrong_wire(1, "bytes", typ));
                };
                let s = r.read_len_delimited()?;
                h.key = String::from_utf8_lossy(s).into_owned();
            }
            2 => {
                let WireType::Bytes = typ else {
                    return Err(wrong_wire(2, "bytes", typ));
                };
                let s = r.read_len_delimited()?;
                h.value = String::from_utf8_lossy(s).into_owned();
            }
            _ => r.skip_value(typ)?,
        }
    }
    Ok(h)
}

/// Builds the client-side keepalive frame. Lark's long connection uses an
/// app-layer ping (binary Frame with type=ping), NOT a WebSocket
/// protocol-level PING — protocol-level pings would be ignored by Lark's
/// server.
pub fn new_ping_frame(service_id: i32) -> Frame {
    Frame {
        method: FRAME_METHOD_CONTROL,
        service: service_id,
        headers: vec![FrameHeader::new(
            FRAME_HEADER_TYPE_KEY,
            FRAME_HEADER_TYPE_PING,
        )],
        ..Frame::default()
    }
}

/// Builds the client-side response to a server-initiated ping. Lark may push
/// ping frames at any cadence; we reply in kind.
pub fn new_pong_frame(service_id: i32) -> Frame {
    Frame {
        method: FRAME_METHOD_CONTROL,
        service: service_id,
        headers: vec![FrameHeader::new(
            FRAME_HEADER_TYPE_KEY,
            FRAME_HEADER_TYPE_PONG,
        )],
        ..Frame::default()
    }
}

/// Builds the ACK response for an inbound data frame. Per the SDK, the ACK
/// reuses the inbound frame's Headers verbatim (so the server can correlate
/// by message_id) and writes a JSON-encoded Response struct as the Payload.
///
/// `code_ok` is true on successful dispatch (Response.code=200); false
/// surfaces 500 to the server (it will retry the event). The payload shape
/// mirrors the SDK's NewResponseByCode JSON: null headers and null data,
/// which is what the server expects to receive.
pub fn new_ack_frame(inbound: &Frame, code_ok: bool) -> Frame {
    let code = if code_ok { 200 } else { 500 };
    let payload = format!(r#"{{"code":{code},"headers":null,"data":null}}"#);
    Frame {
        seq_id: inbound.seq_id,
        log_id: inbound.log_id,
        method: inbound.method,
        service: inbound.service,
        headers: inbound.headers.clone(),
        payload_encoding: inbound.payload_encoding.clone(),
        payload_type: inbound.payload_type.clone(),
        payload: Some(payload.into_bytes()),
        log_id_new: inbound.log_id_new.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden bytes produced by the official SDK's pbbp2.Frame
    /// MarshalToSizedBuffer for a canonical ping frame. This is the
    /// load-bearing compatibility check: Lark's server must accept exactly
    /// these bytes.
    #[test]
    fn ping_frame_matches_sdk_golden_bytes() {
        let f = new_ping_frame(9);
        let got = f.marshal();
        // field1(seq=0) field2(log=0) field3(service=9) field4(method=0)
        // field5(header{type:ping}) field6("") field7("") field9("")
        let want: Vec<u8> = [
            0x08, 0x00, // seq_id = 0
            0x10, 0x00, // log_id = 0
            0x18, 0x09, // service = 9
            0x20, 0x00, // method = 0
            // header entry: tag 5 bytes, len, key "type", value "ping"
            0x2a, 0x0c, 0x0a, 0x04, b't', b'y', b'p', b'e', 0x12, 0x04, b'p', b'i', b'n', b'g',
            0x32, 0x00, // payload_encoding = ""
            0x3a, 0x00, // payload_type = ""
            0x4a, 0x00, // log_id_new = ""
        ]
        .to_vec();
        assert_eq!(got, want);
    }

    #[test]
    fn ack_frame_reuses_headers_and_encodes_code() {
        let inbound = Frame {
            seq_id: 1,
            log_id: 2,
            service: 9,
            method: FRAME_METHOD_DATA,
            headers: vec![
                FrameHeader::new("message_id", "m/1"),
                FrameHeader::new("sum", "1"),
            ],
            payload: Some(br#"{"event":{}}"#.to_vec()),
            ..Frame::default()
        };
        let ack = new_ack_frame(&inbound, true);
        assert_eq!(ack.method, FRAME_METHOD_DATA);
        assert_eq!(ack.service, 9);
        assert_eq!(ack.headers, inbound.headers);
        let payload = ack.payload.as_deref().unwrap();
        assert_eq!(
            std::str::from_utf8(payload).unwrap(),
            r#"{"code":200,"headers":null,"data":null}"#
        );

        let nack = new_ack_frame(&inbound, false);
        assert_eq!(
            std::str::from_utf8(nack.payload.as_deref().unwrap()).unwrap(),
            r#"{"code":500,"headers":null,"data":null}"#
        );
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let f = Frame {
            seq_id: 42,
            log_id: 7,
            service: -1, // negative int32 encodes as 10-byte varint
            method: FRAME_METHOD_DATA,
            headers: vec![
                FrameHeader::new("message_id", "m/abc"),
                FrameHeader::new("sum", "3"),
                FrameHeader::new("seq", "2"),
            ],
            payload_encoding: "json".into(),
            payload_type: "event".into(),
            payload: Some(vec![1, 2, 3, 255]),
            log_id_new: "log-new".into(),
        };
        let decoded = unmarshal_frame(&f.marshal()).unwrap();
        assert_eq!(decoded, f);
    }

    #[test]
    fn empty_payload_is_some_vs_none_on_the_wire() {
        // None payload omits field 8 entirely.
        let none = Frame {
            service: 1,
            ..Frame::default()
        };
        let raw = none.marshal();
        assert!(!raw.contains(&0x42), "tag 8 (0x42<<3|2) must be absent");
        assert_eq!(unmarshal_frame(&raw).unwrap().payload, None);

        // Some(empty) still emits the tag with a zero length.
        let some_empty = Frame {
            service: 1,
            payload: Some(Vec::new()),
            ..Frame::default()
        };
        let decoded = unmarshal_frame(&some_empty.marshal()).unwrap();
        assert_eq!(decoded.payload, Some(Vec::new()));
    }

    #[test]
    fn unmarshal_skips_unknown_fields() {
        let mut base = new_ping_frame(9).marshal();
        // Append an unknown field 15, wire type 2, value "zz".
        append_tag(&mut base, 15, WireType::Bytes);
        append_bytes(&mut base, b"zz");
        let f = unmarshal_frame(&base).unwrap();
        assert_eq!(f.service, 9);
        assert_eq!(f.header_value("type"), "ping");
    }

    #[test]
    fn unmarshal_rejects_truncated_and_empty_input() {
        assert_eq!(unmarshal_frame(&[]).unwrap_err(), FrameError::Empty);
        // Tag says 10-byte string but only 2 bytes follow.
        let bad = [0x2a, 0x0a, 0x01, 0x02];
        assert!(unmarshal_frame(&bad).is_err());
    }

    #[test]
    fn first_header_wins_for_duplicate_keys() {
        let f = Frame {
            headers: vec![
                FrameHeader::new("type", "ping"),
                FrameHeader::new("type", "pong"),
            ],
            ..Frame::default()
        };
        assert_eq!(f.header_value("type"), "ping");
        assert_eq!(f.header_value("missing"), "");
    }
}
