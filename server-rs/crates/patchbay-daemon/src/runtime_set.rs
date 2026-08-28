//! Canonical, dynamically replaceable daemon runtime identity set.
//!
//! Registration/reconcile owns publication while control transport, HTTP
//! heartbeats, and task claiming subscribe to the same value. This prevents a
//! workspace discovered after startup from being registered locally but never
//! joining the authenticated WebSocket or machine-level claim request.

use tokio::sync::watch;

pub struct RuntimeSet {
    tx: watch::Sender<Vec<String>>,
}

impl RuntimeSet {
    pub fn new() -> Self {
        let (tx, _) = watch::channel(Vec::new());
        Self { tx }
    }

    /// Atomically replaces the complete set. Canonicalization avoids
    /// reconnecting transports for ordering-only or duplicate changes.
    pub fn replace(&self, runtime_ids: impl IntoIterator<Item = String>) {
        let mut ids: Vec<String> = runtime_ids
            .into_iter()
            .filter(|id| !id.is_empty())
            .collect();
        ids.sort();
        ids.dedup();
        if *self.tx.borrow() != ids {
            self.tx.send_replace(ids);
        }
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.tx.borrow().clone()
    }

    pub(crate) fn contains(&self, runtime_id: &str) -> bool {
        self.tx.borrow().iter().any(|id| id == runtime_id)
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Vec<String>> {
        self.tx.subscribe()
    }
}

impl Default for RuntimeSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_is_sorted_deduplicated_and_stable() {
        let set = RuntimeSet::new();
        let mut changes = set.subscribe();
        set.replace(["b".into(), "a".into(), "b".into(), String::new()]);
        assert_eq!(set.snapshot(), vec!["a".to_string(), "b".to_string()]);
        assert!(changes.has_changed().unwrap());
        changes.borrow_and_update();

        set.replace(["b".into(), "a".into()]);
        assert!(!changes.has_changed().unwrap());
    }
}
