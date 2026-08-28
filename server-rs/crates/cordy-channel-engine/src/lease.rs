//! Token-fenced, per-installation WebSocket lease types and the store
//! seam.
//!
//! Lease vocabulary shared with the Redis implementation in
//! `redis_lease_store`. The interfaces live here
//! so supervisor/router/session modules share one dependency-free spot.

use std::collections::HashSet;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;
use uuid::Uuid;

/// Fences the WS supervisor lease for an installation. The store performs
/// the CAS: grant when the row is unleased, expired, or already held by
/// `token` (renewal); otherwise report it held elsewhere via
/// [`LeaseError::NotAcquired`].
#[derive(Debug, Clone)]
pub struct AcquireLeaseParams {
    /// The channel_installation id being leased.
    pub id: Uuid,
    /// The caller's fencing token (a per-process random node id).
    pub token: String,
    /// Absolute expiry mirror persisted by DB-backed stores; Redis-backed
    /// leases derive it from `ttl`.
    pub expires_at: DateTime<Utc>,
    /// How long the grant is valid.
    pub ttl: chrono::Duration,
}

/// Releases a WS supervisor lease the caller still holds. The store must
/// fence on `token` so a stale release from a rotation predecessor cannot
/// clobber a successor's freshly acquired lease.
#[derive(Debug, Clone)]
pub struct ReleaseLeaseParams {
    pub id: Uuid,
    pub token: String,
}

/// Error taxonomy for lease stores.
///
/// Port note: Go returns the sentinel `ErrLeaseNotAcquired` via
/// `errors.Is`; Rust models it as a typed error variant so callers match
/// instead of string-comparing.
#[derive(Debug, Error)]
pub enum LeaseError {
    /// The CAS predicate did not match — another replica (or an
    /// in-process predecessor mid-rotation) holds a live lease. The
    /// Supervisor treats it as "not ours yet, retry later", distinct from
    /// a transport error. Stores wrap their backend's no-rows signal into
    /// this. Message mirrors Go's sentinel text.
    #[error("engine: ws lease held elsewhere")]
    NotAcquired,

    /// A transport/backend failure (Redis down, pool closed, …).
    #[error(transparent)]
    Backend(#[from] anyhow::Error),
}

/// Owns the token-fenced, per-installation WebSocket leases.
#[async_trait]
pub trait LeaseStore: Send + Sync {
    /// Returns the IDs that currently have any live owner. It is a sweep
    /// optimization only; [`try_acquire`](Self::try_acquire) remains the
    /// authority.
    async fn list_held(&self, ids: &[Uuid]) -> Result<HashSet<String>, LeaseError>;

    /// Grants when the lease is absent or already carries the same token
    /// (safe retry after an uncertain response); otherwise it returns
    /// [`LeaseError::NotAcquired`].
    async fn try_acquire(&self, arg: AcquireLeaseParams) -> Result<(), LeaseError>;

    /// Extends only a lease whose current value equals `token`.
    /// [`LeaseError::NotAcquired`] means ownership has been lost.
    async fn renew(&self, arg: AcquireLeaseParams) -> Result<(), LeaseError>;

    /// Deletes only a lease whose current value equals `token`. A stale
    /// token is an intentional fenced no-op (`Ok(())`).
    async fn release(&self, arg: ReleaseLeaseParams) -> Result<(), LeaseError>;
}
