//! WeCom adapter metrics — port of `server/internal/metrics/wecom.go`.
//!
//! The production sink behind the WeCom adapter's Metrics interface. The
//! adapter degrades quietly: dial failures and refused handshakes both return
//! the connection to the Supervisor, which backs off and retries; a blocked
//! ingest queue simply stops draining the socket until the worker catches up.
//!
//! The two connection counters are deliberately separate. A dial or read
//! failure is infrastructure and usually recovers on its own; a handshake the
//! server refuses on its merits is a wrong secret or a deleted bot, and it
//! repeats identically on every backoff until a person fixes the installation.
//! Summed into one number an operator cannot tell "wait" from "rotate the
//! credential".
//!
//! No installation_id label anywhere — it is the same class of unbounded
//! identifier as workspace_id and session_id. Attribution falls to structured
//! logs.

use prometheus::Counter;

#[derive(Clone)]
pub struct WecomMetrics {
    pub connect_failures: Counter,
    pub auth_failures: Counter,
    pub callbacks_queued: Counter,
    pub callback_queue_blocked: Counter,
}

fn counter(name: &str, help: &str) -> Counter {
    Counter::new(name, help).expect("valid counter")
}

impl WecomMetrics {
    pub fn new() -> Self {
        Self {
            connect_failures: counter(
                "cordy_wecom_connect_failures_total",
                "Long-connection attempts that failed for a reason nobody has to act on: the socket never came up, or the server answered the handshake with a code that only means it could not verify the bot (a throttle, a platform-side failure). Excludes credential rejections, which are counted apart.",
            ),
            auth_failures: counter(
                "cordy_wecom_auth_failures_total",
                "Long-connection handshakes the server refused on the credentials themselves (WeCom errcode 40001 / 40013). The bot stays down until somebody fixes the installation.",
            ),
            callbacks_queued: counter(
                "cordy_wecom_inbound_callbacks_total",
                "Inbound callbacks handed to the ingest worker. The baseline every other inbound number is read against.",
            ),
            callback_queue_blocked: counter(
                "cordy_wecom_inbound_queue_blocked_total",
                "Times the read loop had to wait on a full ingest queue. Backpressure by design; a rising rate means the engine is behind and the socket is about to stop being drained.",
            ),
        }
    }

    pub fn record_connect_failure(&self) {
        self.connect_failures.inc();
    }

    pub fn record_auth_failure(&self) {
        self.auth_failures.inc();
    }

    pub fn record_callback_queued(&self) {
        self.callbacks_queued.inc();
    }

    pub fn record_callback_queue_blocked(&self) {
        self.callback_queue_blocked.inc();
    }

    pub fn collectors(&self) -> Vec<Box<dyn prometheus::core::Collector>> {
        vec![
            Box::new(self.connect_failures.clone()),
            Box::new(self.auth_failures.clone()),
            Box::new(self.callbacks_queued.clone()),
            Box::new(self.callback_queue_blocked.clone()),
        ]
    }
}

impl Default for WecomMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn counters_track_each_verdict_separately() {
        let m = WecomMetrics::new();
        m.record_connect_failure();
        m.record_connect_failure();
        m.record_auth_failure();
        m.record_callback_queued();
        m.record_callback_queue_blocked();
        assert_eq!(m.connect_failures.get(), 2.0);
        assert_eq!(m.auth_failures.get(), 1.0);
        assert_eq!(m.callbacks_queued.get(), 1.0);
        assert_eq!(m.callback_queue_blocked.get(), 1.0);
    }

    #[test]
    fn collectors_have_fixed_zero_label_cardinality() {
        let metrics = WecomMetrics::new();
        let families = metrics
            .collectors()
            .into_iter()
            .flat_map(|collector| collector.collect())
            .collect::<Vec<_>>();

        assert_eq!(families.len(), 4);
        assert!(families.iter().all(|family| family
            .get_metric()
            .iter()
            .all(|metric| metric.get_label().is_empty())));
    }

    #[test]
    fn production_registry_registers_every_wecom_counter_once() {
        let registry = crate::Registry::new(crate::RegistryOptions {
            pool: None,
            realtime: None,
            version: "test".to_string(),
            commit: "test".to_string(),
            sampler: None,
        });
        registry.wecom.record_connect_failure();

        let names = registry
            .gatherer
            .gather()
            .into_iter()
            .map(|family| family.name().to_string())
            .filter(|name| name.starts_with("cordy_wecom_"))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names,
            BTreeSet::from([
                "cordy_wecom_auth_failures_total".to_string(),
                "cordy_wecom_connect_failures_total".to_string(),
                "cordy_wecom_inbound_callbacks_total".to_string(),
                "cordy_wecom_inbound_queue_blocked_total".to_string(),
            ])
        );
    }
}
