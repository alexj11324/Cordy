//! Registry — port of `server/internal/metrics/registry.go`.
//!
//! Assembles every collector into one Prometheus registry. The Go version
//! also mounts the Go runtime and process collectors; the Rust port exposes
//! the native Linux process collector plus build and domain collectors, and
//! the DB/realtime collectors are optional exactly like their Go counterparts.

use std::sync::Arc;

use prometheus::Opts;

use crate::channel_lease::ChannelLeaseMetrics;
use crate::channel_media::ChannelMediaReconcilerMetrics;
use crate::db::DbCollector;
use crate::http::HttpMetrics;
use crate::lark_backfill::LarkBackfillMetrics;
use crate::realtime::RealtimeCollector;
use crate::wecom::WecomMetrics;

pub struct RegistryOptions {
    pub pool: Option<Arc<sqlx::PgPool>>,
    pub realtime: Option<&'static cordy_realtime::Metrics>,
    pub daemonws: Option<&'static cordy_daemon::hub::Metrics>,
    pub version: String,
    pub commit: String,
    /// When `Some`, opts the registry into the scrape-time SQL sampler
    /// (MUL-2947). Intentionally separate from `pool` so existing callers
    /// cannot accidentally start hitting the database on every /metrics
    /// scrape.
    pub sampler: Option<crate::sampler::BusinessSamplerOptions>,
}

pub struct Registry {
    pub gatherer: prometheus::Registry,
    pub http: Arc<HttpMetrics>,
    pub business: Arc<crate::business::BusinessMetrics>,
    pub channel_media: Arc<ChannelMediaReconcilerMetrics>,
    pub channel_lease: Arc<ChannelLeaseMetrics>,
    pub wecom: Arc<WecomMetrics>,
    pub lark_backfill: Arc<LarkBackfillMetrics>,
}

fn default_label(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

impl Registry {
    pub fn new(opts: RegistryOptions) -> Self {
        let reg = prometheus::Registry::new();

        let build_info = prometheus::GaugeVec::new(
            Opts::new(
                "cordy_build_info",
                "Build information for the Cordy server binary.",
            ),
            &["version", "commit"],
        )
        .expect("valid gauge vec");
        build_info
            .with_label_values(&[
                &default_label(&opts.version, "dev"),
                &default_label(&opts.commit, "unknown"),
            ])
            .set(1.0);
        let _ = reg.register(Box::new(build_info));

        #[cfg(target_os = "linux")]
        let _ = reg.register(Box::new(
            prometheus::process_collector::ProcessCollector::for_self(),
        ));

        let http = Arc::new(HttpMetrics::new());
        for c in http.collectors() {
            let _ = reg.register(c);
        }

        let business = Arc::new(crate::business::BusinessMetrics::new());
        business.register_all(&reg);

        let channel_media = Arc::new(ChannelMediaReconcilerMetrics::new());
        for c in channel_media.collectors() {
            let _ = reg.register(c);
        }

        let channel_lease = Arc::new(ChannelLeaseMetrics::new());
        for c in channel_lease.collectors() {
            let _ = reg.register(c);
        }

        let wecom = Arc::new(WecomMetrics::new());
        for c in wecom.collectors() {
            let _ = reg.register(c);
        }

        let lark_backfill = Arc::new(LarkBackfillMetrics::new());
        for c in lark_backfill.collectors() {
            let _ = reg.register(c);
        }

        if let Some(pool) = opts.pool {
            let _ = reg.register(Box::new(DbCollector::new(pool)));
        }
        if let Some(realtime_metrics) = opts.realtime {
            let _ = reg.register(Box::new(RealtimeCollector::new(realtime_metrics)));
        }
        if let Some(daemonws_metrics) = opts.daemonws {
            let _ = reg.register(Box::new(crate::daemonws::DaemonWsCollector::new(
                daemonws_metrics,
            )));
        }

        if let Some(sampler_opts) = opts.sampler {
            if let Some(sampler) = crate::sampler::BusinessSamplerCollector::new(sampler_opts) {
                for c in sampler.collectors() {
                    let _ = reg.register(c);
                }
            }
        }

        Self {
            gatherer: reg,
            http,
            business,
            channel_media,
            channel_lease,
            wecom,
            lark_backfill,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn production_registry_exposes_native_process_diagnostics() {
        let registry = Registry::new(RegistryOptions {
            pool: None,
            realtime: None,
            daemonws: None,
            version: "test".to_string(),
            commit: "test".to_string(),
            sampler: None,
        });
        let names: Vec<_> = registry
            .gatherer
            .gather()
            .into_iter()
            .map(|family| family.name().to_string())
            .collect();

        for expected in [
            "process_resident_memory_bytes",
            "process_virtual_memory_bytes",
            "process_threads",
            "process_open_fds",
        ] {
            assert!(names.iter().any(|name| name == expected), "{expected}");
        }
    }
}
