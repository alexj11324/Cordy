//! Product telemetry shipping to an external analytics backend (PostHog) —
//!
//! Design (from the Go package):
//! - [`AnalyticsClient::capture`] is non-blocking. Request handlers must never
//!   wait on analytics network I/O: events enqueue into a bounded channel and
//!   a background worker flushes in batches.
//! - When the queue is full events are dropped (and counted). A broken
//!   analytics backend must never degrade the product.
//! - With no API key configured the package runs a no-op client, keeping
//!   local dev and self-hosted instances friction-free.

pub mod client;
pub mod events;
pub mod posthog;

pub use client::{new_from_env, AnalyticsClient, Event, NoopClient};
pub use events::*;
