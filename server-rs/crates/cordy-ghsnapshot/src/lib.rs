//! GitHub PR snapshot refresh: the single GraphQL query, normalization,
//! and the outbound work queue (dedup, single in-flight per PR, bounded
//! concurrency, Retry-After backoff, head-SHA-guarded atomic write).
//!
//! Port of `server/internal/integrations/ghsnapshot` (client.go /
//! snapshot.go / refresh.go). [`manager::Manager`] owns the orchestration
//! half: queue, worker pool, rate-limit pauses, chase backoff, TTL sweep,
//! and the guarded snapshot-write transaction.

pub mod client;
pub mod manager;
pub mod snapshot;

pub use client::{Client, RateLimitError, DEFAULT_API_BASE};
pub use manager::{Address, Manager, OnApplied};
pub use snapshot::{
    fetch_pr_snapshot, normalize_node, normalize_run_status, normalize_status_state, CheckContext,
    PrSnapshot, PR_SNAPSHOT_QUERY,
};
