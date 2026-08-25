//! Daemon WebSocket collector — port of `server/internal/metrics/daemonws.go`.

use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;

use prometheus::core::{Collector, Desc};
use prometheus::proto::{self, MetricFamily};

use cordy_daemon::hub::Metrics as DaemonWsMetrics;

pub struct DaemonWsCollector {
    metrics: Option<&'static DaemonWsMetrics>,
    descs: Vec<Desc>,
}

fn desc(name: &str, help: &str, variable_labels: &[&str]) -> Desc {
    Desc::new(
        name.to_string(),
        help.to_string(),
        variable_labels.iter().map(|s| s.to_string()).collect(),
        HashMap::new(),
    )
    .expect("valid descriptor")
}

const I_CONNECTS: usize = 0;
const I_DISCONNECTS: usize = 1;
const I_ACTIVE: usize = 2;
const I_SLOW_EVICT: usize = 3;
const I_WAKEUP_PUB: usize = 4;
const I_WAKEUP_PUB_ERR: usize = 5;
const I_WAKEUP_RECV: usize = 6;
const I_WAKEUP_DELIVERED: usize = 7;

impl DaemonWsCollector {
    pub fn new(metrics: &'static DaemonWsMetrics) -> Self {
        let descs = vec![
            desc(
                "cordy_daemonws_connects_total",
                "Total daemon WebSocket connections opened.",
                &[],
            ),
            desc(
                "cordy_daemonws_disconnects_total",
                "Total daemon WebSocket connections closed.",
                &[],
            ),
            desc(
                "cordy_daemonws_active_connections",
                "Current daemon WebSocket connections.",
                &[],
            ),
            desc(
                "cordy_daemonws_slow_evictions_total",
                "Total daemon WebSocket clients evicted for slow consumption.",
                &[],
            ),
            desc(
                "cordy_daemonws_wakeup_published_total",
                "Total daemon wakeups published to the Redis relay.",
                &[],
            ),
            desc(
                "cordy_daemonws_wakeup_publish_errors_total",
                "Total daemon wakeup Redis publish errors.",
                &[],
            ),
            desc(
                "cordy_daemonws_wakeup_received_total",
                "Total daemon wakeups received from the Redis relay.",
                &[],
            ),
            desc(
                "cordy_daemonws_wakeup_delivered_total",
                "Total daemon wakeup local delivery attempts.",
                &["result"],
            ),
        ];
        Self {
            metrics: Some(metrics),
            descs,
        }
    }

    fn collect_values(&self) -> Vec<(usize, f64, Vec<String>)> {
        let Some(m) = self.metrics else {
            return Vec::new();
        };
        let g = |a: &std::sync::atomic::AtomicI64| a.load(Relaxed) as f64;
        vec![
            (I_CONNECTS, g(&m.connects_total), vec![]),
            (I_DISCONNECTS, g(&m.disconnects_total), vec![]),
            (I_ACTIVE, g(&m.active_connections), vec![]),
            (I_SLOW_EVICT, g(&m.slow_evictions_total), vec![]),
            (I_WAKEUP_PUB, g(&m.wakeup_published_total), vec![]),
            (I_WAKEUP_PUB_ERR, g(&m.wakeup_publish_errors), vec![]),
            (I_WAKEUP_RECV, g(&m.wakeup_received_total), vec![]),
            (
                I_WAKEUP_DELIVERED,
                g(&m.wakeup_delivered_hit),
                vec!["hit".into()],
            ),
            (
                I_WAKEUP_DELIVERED,
                g(&m.wakeup_delivered_miss),
                vec!["miss".into()],
            ),
        ]
    }
}

impl Collector for DaemonWsCollector {
    fn desc(&self) -> Vec<&Desc> {
        self.descs.iter().collect()
    }

    fn collect(&self) -> Vec<MetricFamily> {
        self.collect_values()
            .into_iter()
            .map(|(idx, value, labels)| const_metric(&self.descs[idx], value, labels))
            .collect()
    }
}

fn const_metric(desc: &Desc, value: f64, labels: Vec<String>) -> MetricFamily {
    let is_counter = desc.fq_name.ends_with("_total");
    let mut metric = proto::Metric::default();
    if is_counter {
        let mut counter = proto::Counter::default();
        counter.set_value(value);
        metric.set_counter(counter);
    } else {
        let mut gauge = proto::Gauge::default();
        gauge.set_value(value);
        metric.set_gauge(gauge);
    }
    metric.set_label(
        desc.variable_labels
            .iter()
            .zip(labels)
            .map(|(name, val)| {
                let mut lp = proto::LabelPair::default();
                lp.set_name(name.clone());
                lp.set_value(val);
                lp
            })
            .collect(),
    );
    let mut mf = MetricFamily::default();
    mf.set_name(desc.fq_name.clone());
    mf.set_help(desc.help.clone());
    mf.set_field_type(if is_counter {
        proto::MetricType::COUNTER
    } else {
        proto::MetricType::GAUGE
    });
    mf.set_metric(vec![metric]);
    mf
}

#[cfg(test)]
mod tests {
    use super::*;
    use prometheus::proto::MetricType;

    #[test]
    fn daemonws_families_use_counter_and_gauge_types() {
        let collector = DaemonWsCollector::new(&cordy_daemon::hub::M);
        let families = collector.collect();
        let active = families
            .iter()
            .find(|family| family.name() == "cordy_daemonws_active_connections")
            .unwrap();
        assert_eq!(active.get_field_type(), MetricType::GAUGE);
        let connects = families
            .iter()
            .find(|family| family.name() == "cordy_daemonws_connects_total")
            .unwrap();
        assert_eq!(connects.get_field_type(), MetricType::COUNTER);
        let delivered = families
            .iter()
            .find(|family| family.name() == "cordy_daemonws_wakeup_delivered_total")
            .unwrap();
        assert_eq!(delivered.get_field_type(), MetricType::COUNTER);
        assert_eq!(delivered.get_metric().len(), 2);
    }
}
