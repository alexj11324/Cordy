//! The channel-agnostic runtime that DRIVES the channel adapters defined
//! in `patchbay-channel`.
//!
//! This is the generalized channel engine tracked by PB-3620.
//!
//! It provides:
//!
//! 1. [`lease`] / [`redis_lease_store`] — token-fenced, per-installation
//!    WebSocket leases. Every mutation is a single Lua operation, so
//!    compare + expiry update/delete cannot be interleaved by another
//!    replica; at most one supervisor process globally connects per
//!    installation.
//! 2. [`batcher`] — the per-chat_session run-trigger debouncer that
//!    collapses an inbound burst into ONE agent run.
//! 3. [`issue_command`] / [`fresh_command`] — the shared `/issue` and
//!    `/new` command parsers (cross-platform product behavior).
//! 4. [`provenance`] — the reply-origin check: did this task's input
//!    arrive via a channel (reply goes to IM) or directly from
//!    web/mobile (reply stays in Patchbay, PB-4988)?
//!
//! Supervisor, router, and session state machines use the shared seams defined
//! here ([`lease::LeaseStore`], `patchbay_channel::Registry`,
//! `patchbay_channel::InboundHandler`).

pub mod batcher;
pub mod fresh_command;
pub mod hub;
pub mod ids;
pub mod issue_command;
pub mod lease;
pub mod postgres_store;
pub mod provenance;
pub mod redis_lease_store;
pub mod resolvers;
pub mod router;
pub mod session;
pub mod session_media;
pub mod supervisor;

pub use batcher::{PendingBatcher, DEFAULT_CHAT_RUN_BATCH_WINDOW};
pub use fresh_command::parse_fresh_session_command;
pub use ids::new_node_id;
pub use issue_command::{issue_description_from_command_body, parse_issue_command, IssueCommand};
pub use lease::{AcquireLeaseParams, LeaseError, LeaseStore, ReleaseLeaseParams};
pub use provenance::{task_input_is_channel_ingested, ProvenanceQueries};
