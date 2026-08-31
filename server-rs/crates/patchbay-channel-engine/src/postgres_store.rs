//! PostgreSQL-backed installation discovery and channel lease CAS.
//!
//! This is the production rollback/self-host backend used by the Go server:
//! installation discovery spans every channel type, credential rotation is
//! detected from an opaque config fingerprint, and lease ownership is fenced
//! by the token predicates in the `channel_installation` update queries.

use std::collections::HashSet;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::lease::{AcquireLeaseParams, LeaseError, LeaseStore, ReleaseLeaseParams};
use crate::supervisor::{Installation, InstallationStore};

#[derive(Clone)]
pub struct PostgresChannelStore {
    pool: PgPool,
}

impl PostgresChannelStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Mirrors a lease owned by the external Redis backend into the durable
    /// installation row for the public health endpoint.
    pub async fn mirror_lease(
        &self,
        id: Uuid,
        token: &str,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> anyhow::Result<()> {
        patchbay_db::queries::channel::mirror_channel_ws_lease(
            &self.pool,
            id,
            token,
            expires_at,
        )
        .await
        .map(|_| ())
    }

    pub async fn clear_mirrored_lease(&self, id: Uuid, token: &str) -> anyhow::Result<()> {
        patchbay_db::queries::channel::clear_mirrored_channel_ws_lease(&self.pool, id, token)
            .await
            .map(|_| ())
    }
}

fn row_fingerprint(channel_type: &str, config: &serde_json::Value) -> String {
    let mut hash = Sha256::new();
    hash.update(channel_type.as_bytes());
    hash.update([0]);
    // JSONB values returned by sqlx have a stable semantic representation;
    // credential rotation only needs inequality, never a reversible encoding.
    hash.update(serde_json::to_vec(config).unwrap_or_default());
    hex::encode(hash.finalize())
}

#[async_trait]
impl InstallationStore for PostgresChannelStore {
    async fn list_active_installations(&self) -> anyhow::Result<Vec<Installation>> {
        let rows = patchbay_db::queries::channel::list_all_active_channel_installations(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| Installation {
                id: row.id,
                channel_type: patchbay_channel::Type(row.channel_type.clone()),
                fingerprint: row_fingerprint(&row.channel_type, &row.config),
                config: row.config,
            })
            .collect())
    }
}

#[async_trait]
impl LeaseStore for PostgresChannelStore {
    async fn list_held(&self, _ids: &[Uuid]) -> Result<HashSet<String>, LeaseError> {
        // The SQL CAS remains authoritative. Returning no hints makes each
        // candidate attempt that CAS once per supervisor poll.
        Ok(HashSet::new())
    }

    async fn try_acquire(&self, arg: AcquireLeaseParams) -> Result<(), LeaseError> {
        acquire_or_renew(&self.pool, arg).await
    }

    async fn renew(&self, arg: AcquireLeaseParams) -> Result<(), LeaseError> {
        acquire_or_renew(&self.pool, arg).await
    }

    async fn release(&self, arg: ReleaseLeaseParams) -> Result<(), LeaseError> {
        patchbay_db::queries::channel::release_channel_ws_lease(
            &self.pool,
            arg.id,
            Some(&arg.token),
        )
        .await
        .map(|_| ())
        .map_err(LeaseError::Backend)
    }
}

async fn acquire_or_renew(pool: &PgPool, arg: AcquireLeaseParams) -> Result<(), LeaseError> {
    let acquired = patchbay_db::queries::channel::acquire_channel_ws_lease(
        pool,
        Some(&arg.token),
        Some(arg.expires_at),
        arg.id,
    )
    .await
    .map_err(LeaseError::Backend)?;
    if acquired.is_none() {
        return Err(LeaseError::NotAcquired);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::row_fingerprint;
    use serde_json::json;

    #[test]
    fn fingerprint_changes_with_type_or_credentials_and_ignores_object_order() {
        let first = row_fingerprint("slack", &json!({"app_id":"A", "token":"one"}));
        let reordered = row_fingerprint("slack", &json!({"token":"one", "app_id":"A"}));
        let rotated = row_fingerprint("slack", &json!({"app_id":"A", "token":"two"}));
        let other_type = row_fingerprint("feishu", &json!({"app_id":"A", "token":"one"}));

        assert_eq!(first, reordered);
        assert_ne!(first, rotated);
        assert_ne!(first, other_type);
    }
}
