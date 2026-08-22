//! GitHub PR snapshot refresh: the single GraphQL query, normalization,
//! and the outbound work queue (dedup, single in-flight per PR, bounded
//! concurrency, Retry-After backoff, head-SHA-guarded atomic write).
//!
//! Port of `server/internal/integrations/ghsnapshot` (client.go /
//! snapshot.go / refresh.go). The DB-touching half of Manager (row
//! listing + apply transaction) lands with the handler wiring slice; this
//! module carries the full client, snapshot fetch/normalize, and the
//! pure decision logic both halves share.

pub mod client;
pub mod snapshot;

pub use client::{Client, RateLimitError, DEFAULT_API_BASE};
pub use snapshot::{
    fetch_pr_snapshot, normalize_node, normalize_run_status, normalize_status_state, CheckContext,
    PrSnapshot, PR_SNAPSHOT_QUERY,
};
