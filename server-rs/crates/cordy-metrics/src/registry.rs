//! Registry — port of `server/internal/metrics/registry.go`.
//!
//! Assembles every collector into one Prometheus registry. The Go version
//! also mounts the Go runtime and process collectors; the Rust port exposes
//! build info plus the domain collectors, and the DB/realtime collectors are
//! optional exactly like their Go counterparts.

use std::sync::Arc;

use prometheus::Opts;

use crate::channel_lease::ChannelLeaseMetrics;
use crate::channel_media::ChannelMediaReconcilerMetrics;
use crate::db::DbCollector;
use crate::http::HttpMetrics;
use crate::realtime::RealtimeCollector;
use crate::wecom::WecomMetrics;

pub struct RegistryOptions {
    pub pool: Option<Arc<sqlx::PgPool>>,
    pub realtime: Option<Arc<cordy_realtime::Metrics>>,
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

        if let Some(pool) = opts.pool {
            let _ = reg.register(Box::new(DbCollector::new(pool)));
        }
        if let Some(realtime_metrics) = opts.realtime {
            let _ = reg.register(Box::new(RealtimeCollector::new(realtime_metrics)));
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
        }
    }
}
