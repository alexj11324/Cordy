//! Shared lease-generation fencing for channel runtime resources.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, RwLock};

use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// The exact lease-owner generation attached to one channel build.
///
/// The supervisor backs this with the connector's run token. Lease loss,
/// credential rotation, and shutdown therefore revoke connect and outbound
/// access at the same instant. Adapters may also attach the generation to
/// secondary handles (WebSocket senders, media uploaders) without learning
/// anything about Redis or PostgreSQL leases.
pub struct LeaseGeneration {
    token: String,
    epoch: uuid::Uuid,
    cancel: CancellationToken,
}

impl std::fmt::Debug for LeaseGeneration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LeaseGeneration")
            .field("epoch", &self.epoch)
            .field("active", &self.is_active())
            .finish_non_exhaustive()
    }
}

impl LeaseGeneration {
    pub fn new(token: impl Into<String>, cancel: CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            token: token.into(),
            epoch: uuid::Uuid::now_v7(),
            cancel,
        })
    }

    /// A generation for direct adapter construction outside the supervisor.
    pub fn standalone() -> Arc<Self> {
        Self::new(
            format!("standalone-{}", uuid::Uuid::now_v7()),
            CancellationToken::new(),
        )
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Time-ordered, non-secret incarnation used for cross-process routing
    /// advertisements where a draining predecessor must sort before its
    /// successor. Lease CAS still uses [`token`](Self::token).
    pub fn epoch(&self) -> uuid::Uuid {
        self.epoch
    }

    pub fn is_active(&self) -> bool {
        !self.cancel.is_cancelled()
    }

    pub fn ensure_active(&self) -> Result<(), GenerationExpired> {
        if self.is_active() {
            Ok(())
        } else {
            Err(GenerationExpired)
        }
    }

    pub fn revoke(&self) {
        self.cancel.cancel();
    }

    pub async fn cancelled(&self) {
        self.cancel.cancelled().await;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("channel lease generation is no longer active")]
pub struct GenerationExpired;

struct Entry<V> {
    value: Arc<V>,
    generation: Arc<LeaseGeneration>,
}

/// Generation-aware storage for adapter-owned sender and media handles.
///
/// A stale generation can neither resolve nor clear its successor. Replacing
/// an entry from another generation revokes the predecessor immediately.
pub struct GenerationRegistry<K, V> {
    entries: RwLock<HashMap<K, Entry<V>>>,
}

impl<K, V> Default for GenerationRegistry<K, V> {
    fn default() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
        }
    }
}

impl<K, V> GenerationRegistry<K, V>
where
    K: Eq + Hash + Clone,
{
    pub fn insert(&self, key: K, value: Arc<V>, generation: Arc<LeaseGeneration>) {
        let mut entries = self
            .entries
            .write()
            .unwrap_or_else(|error| error.into_inner());
        let predecessor = entries.insert(key.clone(), Entry { value, generation });
        if let Some(predecessor) = predecessor {
            let same_generation = entries
                .get(&key)
                .is_some_and(|entry| Arc::ptr_eq(&entry.generation, &predecessor.generation));
            if !same_generation {
                predecessor.generation.revoke();
            }
        }
    }

    pub fn remove(&self, key: &K, generation: &Arc<LeaseGeneration>) -> bool {
        let mut entries = self
            .entries
            .write()
            .unwrap_or_else(|error| error.into_inner());
        if !entries
            .get(key)
            .is_some_and(|entry| Arc::ptr_eq(&entry.generation, generation))
        {
            return false;
        }
        entries.remove(key);
        true
    }

    pub fn get(&self, key: &K) -> Option<GenerationHandle<V>> {
        self.entries
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .get(key)
            .filter(|entry| entry.generation.is_active())
            .map(|entry| GenerationHandle {
                value: entry.value.clone(),
                generation: entry.generation.clone(),
            })
    }

    pub fn routing_snapshot(&self) -> Vec<(K, String)> {
        self.entries
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .filter(|(_, entry)| entry.generation.is_active())
            .map(|(key, entry)| (key.clone(), entry.generation.epoch().to_string()))
            .collect()
    }
}

pub struct GenerationHandle<V> {
    value: Arc<V>,
    generation: Arc<LeaseGeneration>,
}

impl<V> GenerationHandle<V> {
    pub fn value(&self) -> &Arc<V> {
        &self.value
    }

    pub fn generation(&self) -> &Arc<LeaseGeneration> {
        &self.generation
    }

    pub fn ensure_active(&self) -> Result<(), GenerationExpired> {
        self.generation.ensure_active()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_revokes_predecessor_and_stale_remove_cannot_win() {
        let registry = GenerationRegistry::default();
        let old = LeaseGeneration::new("old", CancellationToken::new());
        let new = LeaseGeneration::new("new", CancellationToken::new());
        registry.insert("installation", Arc::new(1), old.clone());
        registry.insert("installation", Arc::new(2), new.clone());

        assert!(!old.is_active());
        assert_eq!(**registry.get(&"installation").unwrap().value(), 2);
        assert!(!registry.remove(&"installation", &old));
        assert!(registry.remove(&"installation", &new));
    }
}
