//! Channel media reconciler metrics — port of
//! `server/internal/metrics/channel_media.go`.
//!
//! Observes the media intent-ledger reconciler: how many unreferenced objects
//! it deletes, how many rows it clears because a durable attachment reference
//! exists, how many storage deletes fail (and go to backoff), and the current
//! ledger backlog.

use prometheus::{Counter, Gauge};

pub struct ChannelMediaReconcilerMetrics {
    pub objects_deleted: Counter,
    pub rows_referenced: Counter,
    pub delete_failures: Counter,
    pub backlog: Gauge,
    pub tombstones: Gauge,
    /// Counts an invariant violation, not routine work: a tombstone pass found
    /// an attachment reading the object it was about to re-delete. The object
    /// is kept; a non-zero value means the per-message key or the bind's state
    /// guard stopped holding and needs investigation.
    pub tombstone_referenced: Counter,
}

impl ChannelMediaReconcilerMetrics {
    pub fn new() -> Self {
        Self {
            objects_deleted: Counter::new(
                "cordy_channel_media_reconciler_objects_deleted_total",
                "Unreferenced media objects deleted by the reconciler.",
            )
            .expect("valid counter"),
            rows_referenced: Counter::new(
                "cordy_channel_media_reconciler_rows_referenced_total",
                "Ledger rows cleared because a durable attachment references the object.",
            )
            .expect("valid counter"),
            delete_failures: Counter::new(
                "cordy_channel_media_reconciler_delete_failures_total",
                "Object-storage deletes that failed and were scheduled for retry.",
            )
            .expect("valid counter"),
            backlog: Gauge::new(
                "cordy_channel_media_pending_objects",
                "Live intent rows awaiting bind or reclaim (excludes tombstones).",
            )
            .expect("valid gauge"),
            tombstones: Gauge::new(
                "cordy_channel_media_tombstoned_objects",
                "Deleted objects still tombstoned for scheduled re-deletion.",
            )
            .expect("valid gauge"),
            tombstone_referenced: Counter::new(
                "cordy_channel_media_reconciler_tombstone_referenced_total",
                "Tombstone passes that found an attachment referencing the object (invariant violation; the object is kept).",
            )
            .expect("valid counter"),
        }
    }

    pub fn collectors(&self) -> Vec<Box<dyn prometheus::core::Collector>> {
        vec![
            Box::new(self.objects_deleted.clone()),
            Box::new(self.rows_referenced.clone()),
            Box::new(self.delete_failures.clone()),
            Box::new(self.backlog.clone()),
            Box::new(self.tombstones.clone()),
            Box::new(self.tombstone_referenced.clone()),
        ]
    }
}

impl Default for ChannelMediaReconcilerMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_increment_and_gauges_set() {
        let m = ChannelMediaReconcilerMetrics::new();
        m.objects_deleted.inc();
        m.rows_referenced.inc();
        m.delete_failures.inc();
        m.backlog.set(7.0);
        m.tombstones.set(2.0);
        assert_eq!(m.objects_deleted.get(), 1.0);
        assert_eq!(m.rows_referenced.get(), 1.0);
        assert_eq!(m.delete_failures.get(), 1.0);
        assert_eq!(m.backlog.get(), 7.0);
        assert_eq!(m.tombstones.get(), 2.0);
        assert_eq!(m.tombstone_referenced.get(), 0.0);
    }
}
