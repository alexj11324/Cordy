//! A process-wide map from installation_id to live wsSender — port of
//! `senders_registry.go`.
//!
//! WecomChannel::connect adds an entry on entry and clears it on exit; the
//! outbound paths look up by installation id to push aibot_send_msg over the
//! same socket the inbound loop owns (aibot has no REST outbound path; every
//! write goes over the WebSocket).
//!
//! Why a registry rather than storing the sender on the channel: the
//! OutboundReplier is created once at boot with the shared engine Router and
//! does not have per-installation Channel handles. When the engine invokes
//! Replier.reply, it passes the resolved installation carrying the
//! installation id, not the Channel. The registry is the seam that lets the
//! boot-time Replier reach the per-installation live connection without
//! threading the Channel through the engine.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use uuid::Uuid;

use crate::ws_sender::WsSender;

/// A goroutine-safe installation_id → wsSender map.
#[derive(Default)]
pub struct SendersRegistry {
    by_key: RwLock<HashMap<String, Arc<WsSender>>>,
}

impl SendersRegistry {
    /// Constructs an empty registry. Boot injects ONE instance into both the
    /// channel deps (writer side) and the outbound subscriber / replier
    /// (reader side).
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, id: Uuid, sender: Arc<WsSender>) {
        self.by_key
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), sender);
    }

    /// Removes this installation's entry, but only if `sender` is still the
    /// one registered under it. A generation that is shutting down must not
    /// evict its own successor: connect installs on entry and clears on exit,
    /// so when a lease flips while the old socket is still draining, the two
    /// overlap and the loser's clear runs after the winner's set. Deleting
    /// unconditionally there leaves the registry empty while a healthy
    /// connection is up, and every outbound push resolves to None — the bot
    /// goes silent with nothing in the log to say why, until the next
    /// reconnect happens to re-register.
    pub fn clear(&self, id: Uuid, sender: &Arc<WsSender>) {
        let mut by_key = self.by_key.write().unwrap_or_else(|e| e.into_inner());
        let key = id.to_string();
        let same = by_key.get(&key).is_some_and(|cur| Arc::ptr_eq(cur, sender));
        if !same {
            return;
        }
        by_key.remove(&key);
    }

    /// Returns the live wsSender for an installation, or None when no
    /// connection is currently held. Callers MUST treat None as "connection
    /// not ready" — the Supervisor may be mid-reconnect after a lease flip.
    pub fn get(&self, id: Uuid) -> Option<Arc<WsSender>> {
        self.by_key
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id.to_string())
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    fn sender() -> Arc<WsSender> {
        struct NoConn;
        #[async_trait]
        impl crate::ws_sender::WsConn for NoConn {
            async fn read_message(&self, _: Option<std::time::Instant>) -> anyhow::Result<Vec<u8>> {
                anyhow::bail!("no reads")
            }
            async fn write_message(
                &self,
                _: String,
                _: Option<std::time::Instant>,
            ) -> anyhow::Result<()> {
                anyhow::bail!("no writes")
            }
            async fn close(&self) {}
        }
        Arc::new(WsSender::new(Arc::new(NoConn)))
    }

    #[test]
    fn set_get_clear_roundtrip() {
        let reg = SendersRegistry::new();
        let id = Uuid::now_v7();
        assert!(reg.get(id).is_none());

        let s1 = sender();
        reg.set(id, s1.clone());
        assert!(Arc::ptr_eq(&reg.get(id).unwrap(), &s1));

        // A different generation's clear does not evict the winner.
        let s2 = sender();
        reg.clear(id, &s2);
        assert!(reg.get(id).is_some());

        reg.clear(id, &s1);
        assert!(reg.get(id).is_none());
    }

    #[test]
    fn keys_are_per_installation() {
        let reg = SendersRegistry::new();
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        let s = sender();
        reg.set(a, s.clone());
        assert!(reg.get(b).is_none());
        assert!(reg.get(a).is_some());
    }
}
