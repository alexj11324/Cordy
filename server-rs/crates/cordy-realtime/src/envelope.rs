//! Relay envelopes and stream key naming — port of the envelope section of
//! `server/internal/realtime/redis_relay.go`.
//!
//! The envelope is what gets serialised into each XADD message. It is opaque
//! to the hub: the relay decodes `payload_json` before fanning out. Field
//! names are byte-for-byte identical to the Go implementation — this is part
//! of the WS protocol compatibility contract.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::broadcaster::{DaemonRuntimeDeliverer, SCOPE_DAEMON_RUNTIME, SCOPE_USER};

/// Stream / registry key naming. Centralised so tests can introspect.
pub fn stream_key(scope_type: &str, scope_id: &str) -> String {
    format!("ws:scope:{scope_type}:{scope_id}:stream")
}

pub fn nodes_key(scope_type: &str, scope_id: &str) -> String {
    format!("ws:scope:{scope_type}:{scope_id}:nodes")
}

pub fn heartbeat_key(node_id: &str) -> String {
    format!("ws:node:{node_id}:heartbeat")
}

pub const HEARTBEAT_TTL_SECS: i64 = 90;
pub const HEARTBEAT_PERIOD_SECS: i64 = 30;
pub const CONSUMER_IDLE_GRACE_SECS: i64 = 10 * 60;
pub const LEGACY_STREAM_SCAN_COUNT: i64 = 128;

/// What we serialise into each XADD message. Serde renames keep the wire
/// format identical to the Go struct's json tags.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    #[serde(rename = "event_id")]
    pub event_id: String,
    #[serde(rename = "event_type")]
    pub event_type: String,
    #[serde(rename = "scope")]
    pub scope: String,
    #[serde(rename = "scope_id")]
    pub scope_id: String,
    #[serde(rename = "workspace_id")]
    pub workspace_id: String,
    #[serde(rename = "actor_id")]
    pub actor_id: String,
    #[serde(rename = "created_at")]
    pub created_at: String,
    #[serde(rename = "node_id")]
    pub node_id: String,
    /// Raw JSON of the original ws frame.
    #[serde(rename = "payload_json")]
    pub payload_json: String,
}

/// Parses the WS frame just enough to lift event_type / actor_id for the
/// envelope. Failures yield empty strings — the envelope still works.
pub fn peek_type_actor(frame: &[u8]) -> (String, String) {
    #[derive(Deserialize, Default)]
    struct Probe {
        #[serde(rename = "type", default)]
        ty: String,
        #[serde(rename = "actor_id", default)]
        actor_id: String,
    }
    serde_json::from_slice::<Probe>(frame)
        .map(|p| (p.ty, p.actor_id))
        .unwrap_or_else(|_| (String::new(), String::new()))
}

/// Inserts the event_id field into an existing JSON object frame without
/// re-encoding unrelated payload bytes beyond one decode/encode round-trip.
/// A frame that is not a JSON object — or already carrying an event_id —
/// passes through untouched.
pub fn inject_event_id(frame: &[u8], event_id: &str) -> Vec<u8> {
    if event_id.is_empty() || frame.is_empty() || frame[0] != b'{' {
        return frame.to_vec();
    }
    let Ok(mut obj) = serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(frame)
    else {
        return frame.to_vec();
    };
    if obj.contains_key("event_id") {
        return frame.to_vec();
    }
    obj.insert(
        "event_id".into(),
        serde_json::Value::String(event_id.to_string()),
    );
    serde_json::to_vec(&obj).unwrap_or_else(|_| frame.to_vec())
}

impl Envelope {
    /// Builds an envelope for a publish. `exclude` (when non-empty) becomes
    /// the WorkspaceID so user-scoped fanout can skip connections already
    /// reached by a workspace broadcast.
    pub fn new(
        node_id: &str,
        scope_type: &str,
        scope_id: &str,
        exclude: &str,
        frame: &[u8],
        id: &str,
    ) -> Self {
        let mut ev = Self {
            event_id: id.to_string(),
            scope: scope_type.to_string(),
            scope_id: scope_id.to_string(),
            node_id: node_id.to_string(),
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true),
            payload_json: String::from_utf8_lossy(frame).to_string(),
            ..Default::default()
        };
        if !exclude.is_empty() {
            ev.workspace_id = exclude.to_string();
        }
        let (t, a) = peek_type_actor(frame);
        if !t.is_empty() {
            ev.event_type = t;
            ev.actor_id = a;
        }
        ev
    }

    /// Ordered field/value pairs for XADD, matching Go's map contents.
    pub fn redis_field_pairs(&self) -> Vec<(&'static str, &str)> {
        vec![
            ("event_id", self.event_id.as_str()),
            ("event_type", self.event_type.as_str()),
            ("scope", self.scope.as_str()),
            ("scope_id", self.scope_id.as_str()),
            ("workspace_id", self.workspace_id.as_str()),
            ("actor_id", self.actor_id.as_str()),
            ("created_at", self.created_at.as_str()),
            ("node_id", self.node_id.as_str()),
            ("payload_json", self.payload_json.as_str()),
        ]
    }

    /// Rebuilds an envelope from decoded XREAD field pairs. Returns None when
    /// payload_json is missing/empty (the Go validity check).
    pub fn from_field_pairs(pairs: &[(String, String)]) -> Option<Self> {
        let get = |name: &str| -> String {
            pairs
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let ev = Self {
            event_id: get("event_id"),
            event_type: get("event_type"),
            scope: get("scope"),
            scope_id: get("scope_id"),
            workspace_id: get("workspace_id"),
            actor_id: get("actor_id"),
            created_at: get("created_at"),
            node_id: get("node_id"),
            payload_json: get("payload_json"),
        };
        if ev.payload_json.is_empty() {
            None
        } else {
            Some(ev)
        }
    }
}

/// The hub-side fanout surface `deliver_envelope` needs. Implemented by the
/// WS hub once ported; lets relays dispatch without a concrete Hub type.
#[async_trait]
pub trait HubFanout: Send + Sync {
    async fn fanout_all_dedup(&self, frame: &[u8], exclude_workspace: &str, event_id: &str);
    async fn fanout_user(
        &self,
        user_id: &str,
        frame: &[u8],
        exclude_workspace: &str,
        event_id: &str,
    );
    async fn broadcast_to_scope_dedup(
        &self,
        scope_type: &str,
        scope_id: &str,
        frame: &[u8],
        event_id: &str,
    );
}

/// Routes a decoded envelope to its consumers: daemon-runtime frames go to
/// the deliverer, everything else fans out through the hub with dedup.
pub async fn deliver_envelope(
    hub: Arc<dyn HubFanout>,
    daemon_runtime: Option<Arc<dyn DaemonRuntimeDeliverer>>,
    ev: Envelope,
) {
    if ev.payload_json.is_empty() {
        return;
    }
    let frame = inject_event_id(ev.payload_json.as_bytes(), &ev.event_id);
    match ev.scope.as_str() {
        SCOPE_DAEMON_RUNTIME => {
            if let Some(dr) = daemon_runtime {
                dr.deliver_daemon_runtime(&ev.scope_id, &frame, &ev.event_id);
            }
        }
        "global" => hub.fanout_all_dedup(&frame, "", &ev.event_id).await,
        SCOPE_USER => {
            hub.fanout_user(&ev.scope_id, &frame, &ev.workspace_id, &ev.event_id)
                .await;
        }
        other => {
            hub.broadcast_to_scope_dedup(other, &ev.scope_id, &frame, &ev.event_id)
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_serialises_with_go_field_names() {
        let ev = Envelope {
            event_id: "01ABC".into(),
            event_type: "issue:created".into(),
            scope: "workspace".into(),
            scope_id: "ws-1".into(),
            workspace_id: "ws-1".into(),
            actor_id: "agent-7".into(),
            created_at: "2026-08-20T00:00:00Z".into(),
            node_id: "node-1".into(),
            payload_json: r#"{"type":"issue:created"}"#.into(),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["event_id"], "01ABC");
        assert_eq!(json["event_type"], "issue:created");
        assert_eq!(json["scope"], "workspace");
        assert_eq!(json["scope_id"], "ws-1");
        assert_eq!(json["workspace_id"], "ws-1");
        assert_eq!(json["actor_id"], "agent-7");
        assert_eq!(json["created_at"], "2026-08-20T00:00:00Z");
        assert_eq!(json["node_id"], "node-1");
        assert_eq!(json["payload_json"], r#"{"type":"issue:created"}"#);

        // Roundtrip preserves every field.
        let back: Envelope = serde_json::from_value(json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn new_envelope_lifts_type_actor_and_exclude() {
        let frame = br#"{"type":"task:update","actor_id":"agent-9","x":1}"#;
        let ev = Envelope::new("node-1", "task", "t-1", "ws-2", frame, "evt-1");

        assert_eq!(ev.event_type, "task:update");
        assert_eq!(ev.actor_id, "agent-9");
        assert_eq!(ev.workspace_id, "ws-2", "exclude becomes workspace_id");
        assert_eq!(ev.payload_json, String::from_utf8_lossy(frame));
        assert!(!ev.created_at.is_empty());
    }

    #[test]
    fn new_envelope_without_exclude_leaves_workspace_empty() {
        let ev = Envelope::new("n", "workspace", "ws-1", "", b"{}", "e");
        assert_eq!(ev.workspace_id, "");
    }

    #[test]
    fn peek_type_actor_tolerates_garbage() {
        assert_eq!(peek_type_actor(b"not json"), (String::new(), String::new()));
        assert_eq!(
            peek_type_actor(br#"{"type":"t"}"#),
            ("t".to_string(), String::new())
        );
    }

    #[test]
    fn inject_event_id_rules() {
        // Non-object frames pass through untouched.
        assert_eq!(inject_event_id(b"[1,2]", "e"), b"[1,2]");
        assert_eq!(inject_event_id(b"", "e"), b"");
        // Empty event id passes through.
        assert_eq!(inject_event_id(b"{}", ""), b"{}");
        // Existing event_id wins.
        let existing = br#"{"event_id":"orig"}"#;
        assert_eq!(inject_event_id(existing, "new"), existing.to_vec());
        // Insertion adds exactly the requested id.
        let out = inject_event_id(br#"{"type":"t"}"#, "evt-42");
        let obj: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(obj["event_id"], "evt-42");
        assert_eq!(obj["type"], "t");
    }

    #[test]
    fn field_pair_roundtrip() {
        let ev = Envelope::new("n", "user", "u-1", "", br#"{"type":"ping"}"#, "e-1");
        let pairs = ev.redis_field_pairs();
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let back = Envelope::from_field_pairs(&owned).expect("valid envelope");
        assert_eq!(back, ev);

        // Missing payload_json invalidates the entry (Go's validity check).
        let empty: Vec<(String, String)> = vec![];
        assert!(Envelope::from_field_pairs(&empty).is_none());
    }

    #[test]
    fn key_formats_match_go() {
        assert_eq!(
            stream_key("workspace", "ws-1"),
            "ws:scope:workspace:ws-1:stream"
        );
        assert_eq!(nodes_key("user", "u-1"), "ws:scope:user:u-1:nodes");
        assert_eq!(heartbeat_key("node-1"), "ws:node:node-1:heartbeat");
    }
}
