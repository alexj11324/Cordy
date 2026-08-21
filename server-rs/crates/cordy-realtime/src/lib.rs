//! Realtime subsystem — port of `server/internal/realtime`.
//!
//! Modules mirror the Go files one-to-one:
//! - [`broadcaster`]: scope constants + producer-facing traits
//! - [`metrics`]: lightweight atomic counters (global singleton `M`)
//! - [`relay_lifecycle`]: managed-relay lifecycle + mirrored dual-write
//!
//! The concrete relays (`RedisRelay`, `ShardedStreamRelay`) and the WS
//! `Hub` land in subsequent port steps.

pub mod broadcaster;
pub mod envelope;
pub mod hub;
pub mod metrics;
pub mod redis_relay;
pub mod relay_lifecycle;
pub mod sharded_stream_relay;
pub mod stream_retention;

pub use broadcaster::{
    Broadcaster, DaemonRuntimeDeliverer, RelayPublisher, SCOPE_CHAT, SCOPE_DAEMON_RUNTIME,
    SCOPE_TASK, SCOPE_USER, SCOPE_WORKSPACE,
};
pub use metrics::{Metrics, M};
pub use relay_lifecycle::{ManagedRelay, MirroredRelay};
