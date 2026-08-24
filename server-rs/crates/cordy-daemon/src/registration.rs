//! Ordered workspace/runtime registration orchestration.
//!
//! Provider integration supplies one machine-level probe round and derives
//! each workspace's custom-profile payload. The daemon owns API ordering,
//! authoritative state application, dropped-row cleanup, orphan recovery, and
//! membership removal.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::Mutex as AsyncMutex;

use crate::client::{Client, WorkspaceInfo};
use crate::config::Config;
use crate::repocache::Ctx;
use crate::runtime_registry::RuntimeRegistry;

const WORKSPACE_SYNC_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistrationPayload {
    /// Exact `/api/daemon/register` runtime entries. The provider integration
    /// owns version probing and custom-profile launch resolution.
    pub runtimes: Vec<BTreeMap<String, String>>,
    pub failed_profiles: Vec<BTreeMap<String, String>>,
}

#[async_trait::async_trait]
pub trait RuntimeRegistrationRound: Send + Sync + 'static {
    async fn payload_for_workspace(
        &self,
        ctx: Ctx,
        workspace_id: &str,
    ) -> anyhow::Result<RegistrationPayload>;
}

#[async_trait::async_trait]
pub trait RuntimeRegistrationSource: Send + Sync + 'static {
    /// Starts one machine-level probe round. The returned object is reused for
    /// every workspace in this sync so N workspaces never cause N×M CLI probes.
    async fn begin_round(&self, ctx: Ctx) -> anyhow::Result<Arc<dyn RuntimeRegistrationRound>>;
}

pub struct RuntimeRegistrationService<S: RuntimeRegistrationSource> {
    config: Arc<Config>,
    client: Arc<Client>,
    source: Arc<S>,
    serial: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl<S: RuntimeRegistrationSource> RuntimeRegistrationService<S> {
    pub fn new(config: Arc<Config>, client: Arc<Client>, source: Arc<S>) -> Self {
        Self {
            config,
            client,
            source,
            serial: Mutex::new(HashMap::new()),
        }
    }

    /// One workspace membership/reconciliation round. Startup calls this with
    /// `reconcile_profiles=false`; WebSocket reconnect calls it with `true` so
    /// profile changes missed while disconnected are folded into registration.
    pub async fn sync_once(
        &self,
        ctx: Ctx,
        registry: &RuntimeRegistry,
        reconcile_profiles: bool,
    ) -> anyhow::Result<()> {
        let workspaces =
            tokio::time::timeout(WORKSPACE_SYNC_TIMEOUT, self.client.list_workspaces(&ctx))
                .await
                .map_err(|_| anyhow::anyhow!("list workspaces timed out"))??;
        let api_ids: BTreeSet<String> = workspaces
            .iter()
            .map(|workspace| workspace.id.clone())
            .collect();
        let tracked: BTreeSet<String> = registry.workspace_ids().into_iter().collect();
        let needs_registration: Vec<WorkspaceInfo> = workspaces
            .into_iter()
            .filter(|workspace| {
                reconcile_profiles
                    || !tracked.contains(&workspace.id)
                    || registry.workspace_needs_runtime_recovery(&workspace.id)
            })
            .collect();

        let mut registered = 0usize;
        if !needs_registration.is_empty() {
            let round = self.source.begin_round(ctx.child()).await?;
            for workspace in &needs_registration {
                let recover_orphans = !tracked.contains(&workspace.id)
                    || registry.workspace_needs_runtime_recovery(&workspace.id);
                match self
                    .register_workspace(
                        ctx.child(),
                        registry,
                        workspace,
                        Arc::clone(&round),
                        recover_orphans,
                    )
                    .await
                {
                    Ok(()) => registered += 1,
                    Err(error) => tracing::warn!(
                        workspace_id = %workspace.id,
                        workspace_name = %workspace.name,
                        %error,
                        "failed to register workspace runtimes"
                    ),
                }
            }
        }

        for workspace_id in tracked.difference(&api_ids) {
            registry.remove_workspace(workspace_id);
            tracing::info!(%workspace_id, "stopped watching workspace");
        }

        if registry.runtime_ids().is_empty() && registered == 0 && !api_ids.is_empty() {
            anyhow::bail!(
                "failed to register runtimes for any of the {} workspace(s)",
                api_ids.len()
            );
        }
        Ok(())
    }

    /// Runtime-gone recovery prunes the stale identity before re-registering
    /// its workspace with a fresh probe round. Orphan recovery is required for
    /// the new response because the previous process/runtime row disappeared.
    pub async fn recover_runtime_gone(
        &self,
        ctx: Ctx,
        registry: &RuntimeRegistry,
        runtime_id: &str,
    ) -> anyhow::Result<()> {
        let workspace_id = registry
            .remove_runtime(runtime_id)
            .ok_or_else(|| anyhow::anyhow!("runtime {runtime_id} is not registered"))?;
        let workspace = registry
            .workspace(&workspace_id)
            .ok_or_else(|| anyhow::anyhow!("workspace {workspace_id} is no longer tracked"))?;
        let round = self.source.begin_round(ctx.child()).await?;
        self.register_workspace(
            ctx,
            registry,
            &WorkspaceInfo {
                id: workspace.id,
                name: workspace.name,
            },
            round,
            true,
        )
        .await
    }

    async fn register_workspace(
        &self,
        ctx: Ctx,
        registry: &RuntimeRegistry,
        workspace: &WorkspaceInfo,
        round: Arc<dyn RuntimeRegistrationRound>,
        recover_orphans: bool,
    ) -> anyhow::Result<()> {
        let serial = self.workspace_lock(&workspace.id);
        let _guard = serial.lock().await;
        let payload = round
            .payload_for_workspace(ctx.child(), &workspace.id)
            .await?;
        anyhow::ensure!(
            !payload.runtimes.is_empty() || !payload.failed_profiles.is_empty(),
            "no runtimes to register for workspace {}",
            workspace.id
        );
        let response = self
            .client
            .register(
                &ctx,
                json!({
                    "workspace_id": &workspace.id,
                    "daemon_id": &self.config.daemon_id,
                    "legacy_daemon_ids": &self.config.legacy_daemon_ids,
                    "device_name": &self.config.device_name,
                    "cli_version": &self.config.cli_version,
                    "launched_by": &self.config.launched_by,
                    "runtimes": &payload.runtimes,
                    "failed_profiles": &payload.failed_profiles,
                }),
            )
            .await?;
        anyhow::ensure!(
            !response.runtimes.is_empty() || !payload.failed_profiles.is_empty(),
            "register runtimes returned an empty response for workspace {}",
            workspace.id
        );
        let runtime_ids: Vec<String> = response
            .runtimes
            .iter()
            .map(|runtime| runtime.id.clone())
            .collect();
        let delta = registry.apply_registration(
            workspace.id.clone(),
            workspace.name.clone(),
            response.runtimes,
        )?;
        if !delta.dropped.is_empty() {
            self.client
                .deregister(&ctx, &delta.dropped, HashMap::new())
                .await?;
        }
        if recover_orphans {
            for runtime_id in runtime_ids {
                if let Err(error) = self.client.recover_orphans(&ctx, &runtime_id).await {
                    tracing::warn!(%runtime_id, %error, "recover-orphans failed");
                }
            }
        }
        Ok(())
    }

    fn workspace_lock(&self, workspace_id: &str) -> Arc<AsyncMutex<()>> {
        let mut serial = self.serial.lock().unwrap();
        Arc::clone(
            serial
                .entry(workspace_id.to_string())
                .or_insert_with(|| Arc::new(AsyncMutex::new(()))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn registration_payload_preserves_provider_owned_wire_fields() {
        let payload = RegistrationPayload {
            runtimes: vec![BTreeMap::from([
                ("type".to_string(), "codex".to_string()),
                ("version".to_string(), "1.2.3".to_string()),
                ("profile_id".to_string(), "profile-1".to_string()),
            ])],
            failed_profiles: vec![BTreeMap::from([(
                "profile_id".to_string(),
                "profile-2".to_string(),
            )])],
        };

        let wire = json!({
            "runtimes": payload.runtimes,
            "failed_profiles": payload.failed_profiles,
        });
        assert_eq!(wire["runtimes"][0]["type"], Value::String("codex".into()));
        assert_eq!(
            wire["failed_profiles"][0]["profile_id"],
            Value::String("profile-2".into())
        );
    }
}
