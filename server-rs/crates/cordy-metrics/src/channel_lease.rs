//! Channel lease metrics — port of `server/internal/metrics/channel_lease.go`.
//!
//! Exposes the ownership signals needed to verify a Redis lease cutover
//! without putting installation IDs into metric labels.

use prometheus::{CounterVec, Gauge, Histogram, Opts};

pub struct ChannelLeaseMetrics {
    pub operations: CounterVec,
    pub active_owners: Gauge,
    pub owners_with_renewal_errors: Gauge,
    pub last_successful_renew: Gauge,
    pub takeover_latency: Histogram,
}

impl ChannelLeaseMetrics {
    pub fn new() -> Self {
        Self {
            operations: CounterVec::new(
                Opts::new(
                    "cordy_channel_lease_operations_total",
                    "Channel lease operations by operation and outcome.",
                ),
                &["operation", "outcome"],
            )
            .expect("valid counter vec"),
            active_owners: Gauge::new(
                "cordy_channel_lease_active_owners",
                "Channel installations currently owned by this process.",
            )
            .expect("valid gauge"),
            owners_with_renewal_errors: Gauge::new(
                "cordy_channel_lease_owners_with_renewal_errors",
                "Channel lease owners whose latest renewal attempt failed.",
            )
            .expect("valid gauge"),
            last_successful_renew: Gauge::new(
                "cordy_channel_lease_last_successful_renewal_timestamp_seconds",
                "Unix timestamp of the most recent successful channel lease renewal.",
            )
            .expect("valid gauge"),
            takeover_latency: Histogram::with_opts(
                prometheus::HistogramOpts::new(
                    "cordy_channel_lease_takeover_latency_seconds",
                    "Time from first observed contention until this process acquires the lease.",
                )
                .buckets(vec![1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 180.0, 300.0]),
            )
            .expect("valid histogram"),
        }
    }

    pub fn record_lease_operation(&self, operation: &str, outcome: &str) {
        self.operations
            .with_label_values(&[operation, outcome])
            .inc();
    }

    pub fn set_active_lease_owners(&self, count: f64) {
        self.active_owners.set(count);
    }

    pub fn set_owners_with_renewal_errors(&self, count: f64) {
        self.owners_with_renewal_errors.set(count);
    }

    /// Records the unix-seconds timestamp of the latest successful renewal.
    pub fn set_last_successful_renewal_at(&self, unix_seconds: i64) {
        self.last_successful_renew.set(unix_seconds as f64);
    }

    pub fn observe_takeover_latency(&self, delay_secs: f64) {
        self.takeover_latency.observe(delay_secs);
    }

    pub fn collectors(&self) -> Vec<Box<dyn prometheus::core::Collector>> {
        vec![
            Box::new(self.operations.clone()),
            Box::new(self.active_owners.clone()),
            Box::new(self.owners_with_renewal_errors.clone()),
            Box::new(self.last_successful_renew.clone()),
            Box::new(self.takeover_latency.clone()),
        ]
    }
}

impl Default for ChannelLeaseMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operations_count_by_operation_and_outcome() {
        let m = ChannelLeaseMetrics::new();
        m.record_lease_operation("acquire", "granted");
        m.record_lease_operation("acquire", "contended");
        m.record_lease_operation("acquire", "granted");
        assert_eq!(
            m.operations
                .with_label_values(&["acquire", "granted"])
                .get(),
            2.0
        );
        assert_eq!(
            m.operations
                .with_label_values(&["acquire", "contended"])
                .get(),
            1.0
        );
    }

    #[test]
    fn gauges_set_directly() {
        let m = ChannelLeaseMetrics::new();
        m.set_active_lease_owners(3.0);
        m.set_owners_with_renewal_errors(1.0);
        m.set_last_successful_renewal_at(1_787_285_000);
        m.observe_takeover_latency(12.0);
        assert_eq!(m.active_owners.get(), 3.0);
        assert_eq!(m.owners_with_renewal_errors.get(), 1.0);
        assert_eq!(m.last_successful_renew.get(), 1_787_285_000.0);
        assert_eq!(m.takeover_latency.get_sample_count(), 1);
    }
}
