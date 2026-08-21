//! Prometheus metrics — port of `server/internal/metrics`.
//!
//! Layout mirrors the Go package:
//! - business half: labels, pricing, task lifecycle / LLM counters, PR3
//!   funnel / community / commercial counters, and the PostHog↔Prometheus
//!   pairing bridge (`record_event`).
//! - infrastructure half: HTTP middleware instrumentation, DB pool gauges,
//!   realtime/daemonws collectors, channel lease & media reconciler counters,
//!   WeCom adapter counters, the scrape-time SQL sampler, the METRICS_ADDR
//!   config, the standalone /metrics server, and the registry that assembles
//!   everything.

pub mod business;
pub mod business_events;
pub mod channel_lease;
pub mod channel_media;
pub mod config;
pub mod db;
pub mod http;
pub mod labels;
pub mod labels_pr3;
pub mod pricing;
pub mod realtime;
pub mod registry;
pub mod sampler;
pub mod server;
pub mod wecom;

pub use business::BusinessMetrics;
pub use channel_lease::ChannelLeaseMetrics;
pub use channel_media::ChannelMediaReconcilerMetrics;
pub use config::{is_loopback_addr, Config};
pub use http::HttpMetrics;
pub use registry::{Registry, RegistryOptions};
