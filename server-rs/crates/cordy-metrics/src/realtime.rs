//! Realtime collector — port of `server/internal/metrics/realtime.go`.
//!
//! Exposes the atomic counters of [`cordy_realtime::Metrics`] as Prometheus
//! const metrics gathered at scrape time. Metric names and label sets match
//! the Go collector byte-for-byte so dashboards survive the cutover.

use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;

use prometheus::core::{Collector, Desc};
use prometheus::proto::{self, MetricFamily};

use cordy_realtime::Metrics as RealtimeMetrics;

pub struct RealtimeCollector {
    metrics: Option<Arc<RealtimeMetrics>>,
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

// Index into descs; keep in one place so the collect path stays readable.
const I_CONNECTS: usize = 0;
const I_DISCONNECTS: usize = 1;
const I_ACTIVE: usize = 2;
const I_SLOW_EVICT: usize = 3;
const I_SENT: usize = 4;
const I_DROPPED: usize = 5;
const I_TOO_LARGE: usize = 6;
const I_REDIS_CONNECTED: usize = 7;
const I_XADD: usize = 8;
const I_XADD_ERR: usize = 9;
const I_XREAD: usize = 10;
const I_XREAD_ERR: usize = 11;
const I_ACK: usize = 12;
const I_MIRROR_ERR: usize = 13; // labeled "target"
const I_MIRROR_DIVERGED: usize = 14;
const I_TRIMMED: usize = 15;
const I_MISSING: usize = 16;
const I_RETENTION_ERR: usize = 17;
const I_NO_TTL: usize = 18;
const I_USED_MEM: usize = 19;
const I_MAX_MEM: usize = 20;
const I_EVICTED: usize = 21;
const I_STREAM_ENTRIES: usize = 22; // labeled "stream"
const I_STREAM_MEMORY: usize = 23; // labeled "stream"
const I_STREAM_PTTL: usize = 24; // labeled "stream"

impl RealtimeCollector {
    pub fn new(metrics: Arc<RealtimeMetrics>) -> Self {
        let plain = |name: &str, help: &str| desc(name, help, &[]);
        let descs = vec![
            plain("cordy_realtime_connects_total", "Total realtime WebSocket connections opened."),
            plain("cordy_realtime_disconnects_total", "Total realtime WebSocket connections closed."),
            plain("cordy_realtime_active_connections", "Current realtime WebSocket connections."),
            plain("cordy_realtime_slow_evictions_total", "Total realtime clients evicted for slow consumption."),
            plain("cordy_realtime_messages_sent_total", "Total realtime messages sent."),
            plain("cordy_realtime_messages_dropped_total", "Total realtime messages dropped."),
            plain("cordy_realtime_inbound_too_large_total", "Total realtime connections closed for exceeding the inbound message size limit."),
            plain("cordy_realtime_redis_connected", "Whether the realtime Redis relay is connected."),
            plain("cordy_realtime_redis_xadd_total", "Total Redis XADD operations by the realtime relay."),
            plain("cordy_realtime_redis_xadd_errors_total", "Total Redis XADD errors by the realtime relay."),
            plain("cordy_realtime_redis_xread_total", "Total Redis XREAD operations by the realtime relay."),
            plain("cordy_realtime_redis_xread_errors_total", "Total Redis XREAD errors by the realtime relay."),
            plain("cordy_realtime_redis_ack_total", "Total Redis stream acknowledgements by the realtime relay."),
            desc("cordy_realtime_redis_mirror_errors_total", "Total Redis mirror write errors by the realtime relay.", &["target"]),
            plain("cordy_realtime_redis_mirror_divergence_total", "Total Redis mirror divergence events by the realtime relay."),
            plain("cordy_realtime_redis_stream_trimmed_entries_total", "Total Redis Stream entries removed by retention maintenance."),
            plain("cordy_realtime_redis_stream_missing_total", "Total observed relay stream disappearance transitions, including eviction and expiry."),
            plain("cordy_realtime_redis_retention_errors_total", "Total Redis relay retention maintenance errors."),
            plain("cordy_realtime_redis_streams_without_ttl", "Current number of observed relay streams missing an expiry while TTL protection is enabled."),
            plain("cordy_realtime_redis_used_memory_bytes", "Redis used_memory sampled by the realtime relay."),
            plain("cordy_realtime_redis_maxmemory_bytes", "Redis maxmemory sampled by the realtime relay; zero means unlimited."),
            plain("cordy_realtime_redis_evicted_keys", "Redis instance evicted_keys sampled by the realtime relay."),
            desc("cordy_realtime_redis_stream_entries", "Current entry count of a sampled relay stream.", &["stream"]),
            desc("cordy_realtime_redis_stream_memory_bytes", "Sampled memory usage of a relay stream.", &["stream"]),
            desc("cordy_realtime_redis_stream_pttl_milliseconds", "Remaining relay stream TTL in milliseconds; -1 means no TTL and -2 means missing.", &["stream"]),
        ];
        Self {
            metrics: Some(metrics),
            descs,
        }
    }

    fn collect_values(&self) -> Vec<(usize, f64, Vec<String>)> {
        let mut out = Vec::new();
        let Some(m) = &self.metrics else {
            return out;
        };
        let g = |a: &std::sync::atomic::AtomicI64| a.load(Relaxed) as f64;
        out.push((I_CONNECTS, g(&m.connects_total), vec![]));
        out.push((I_DISCONNECTS, g(&m.disconnects_total), vec![]));
        out.push((I_ACTIVE, g(&m.active_connections), vec![]));
        out.push((I_SLOW_EVICT, g(&m.slow_evictions_total), vec![]));
        out.push((I_SENT, g(&m.messages_sent_total), vec![]));
        out.push((I_DROPPED, g(&m.messages_dropped_total), vec![]));
        out.push((I_TOO_LARGE, g(&m.inbound_too_large_total), vec![]));
        out.push((
            I_REDIS_CONNECTED,
            m.redis_connected.load(std::sync::atomic::Ordering::Relaxed) as i32 as f64,
            vec![],
        ));
        out.push((I_XADD, g(&m.redis_xadd_total), vec![]));
        out.push((I_XADD_ERR, g(&m.redis_xadd_errors), vec![]));
        out.push((I_XREAD, g(&m.redis_xread_total), vec![]));
        out.push((I_XREAD_ERR, g(&m.redis_xread_errors), vec![]));
        out.push((I_ACK, g(&m.redis_ack_total), vec![]));
        out.push((
            I_MIRROR_ERR,
            g(&m.redis_mirror_primary_errors),
            vec!["primary".into()],
        ));
        out.push((
            I_MIRROR_ERR,
            g(&m.redis_mirror_secondary_errors),
            vec!["secondary".into()],
        ));
        out.push((
            I_MIRROR_DIVERGED,
            g(&m.redis_mirror_divergence_total),
            vec![],
        ));
        out.push((I_TRIMMED, g(&m.redis_relay_stream_trimmed_total), vec![]));
        out.push((I_MISSING, g(&m.redis_relay_stream_missing_total), vec![]));
        out.push((I_RETENTION_ERR, g(&m.redis_relay_retention_errors), vec![]));
        out.push((I_NO_TTL, g(&m.redis_relay_streams_without_ttl), vec![]));
        out.push((I_USED_MEM, g(&m.redis_used_memory_bytes), vec![]));
        out.push((I_MAX_MEM, g(&m.redis_max_memory_bytes), vec![]));
        out.push((I_EVICTED, g(&m.redis_evicted_keys), vec![]));
        for (stream, obs) in m.redis_stream_observations() {
            out.push((I_STREAM_ENTRIES, obs.entries as f64, vec![stream.clone()]));
            out.push((
                I_STREAM_MEMORY,
                obs.memory_bytes as f64,
                vec![stream.clone()],
            ));
            out.push((I_STREAM_PTTL, obs.pttl_millis as f64, vec![stream]));
        }
        out
    }
}

impl Collector for RealtimeCollector {
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

/// Builds one gauge/counter const-metric family. Counters and gauges share
/// the wire shape here; type metadata rides on the family name convention.
fn const_metric(desc: &Desc, value: f64, labels: Vec<String>) -> MetricFamily {
    let mut gauge = proto::Gauge::default();
    gauge.set_value(value);
    let mut metric = proto::Metric::default();
    metric.set_gauge(gauge);
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
    mf.set_metric(vec![metric]);
    mf
}
