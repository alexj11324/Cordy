//! Process-local pending-request backends used when Redis is intentionally
//! unset. Multi-node deploys must keep the Redis implementations so every
//! replica shares one request lifecycle.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
    apply_model_list_timeout, apply_update_timeout, cacheable_model_catalog, random_id,
    skill_timed_out, LocalSkillImportConflict, LocalSkillRequestStatus, ModelCatalogSnapshot,
    ModelEntry, ModelListRequest, ModelListStatus, ModelListStore,
    ModelListStore as RedisModelList, RuntimeLocalMcpServerSummary, RuntimeLocalSkillImportRequest,
    RuntimeLocalSkillListRequest, RuntimeLocalSkillSummary, SessionModeEntry, UpdateRequest,
    UpdateStatus, UpdateStore, LOCAL_SKILL_STORE_RETENTION_SECS, MODEL_CATALOG_SERVE_WINDOW_SECS,
    MODEL_LIST_STORE_RETENTION_SECS, UPDATE_STORE_RETENTION_SECS,
};

#[async_trait]
pub trait UpdateStoreBackend: Send + Sync {
    async fn create(
        &self,
        runtime_id: &str,
        target_version: &str,
        initiator_user_id: &str,
    ) -> anyhow::Result<UpdateRequest>;
    async fn get(&self, id: &str) -> anyhow::Result<Option<UpdateRequest>>;
    async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool>;
    async fn pop_pending(&self, runtime_id: &str) -> anyhow::Result<Option<UpdateRequest>>;
    async fn complete(&self, id: &str, output: &str) -> anyhow::Result<()>;
    async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()>;
}

#[async_trait]
pub trait ModelListStoreBackend: Send + Sync {
    async fn create(&self, runtime_id: &str) -> anyhow::Result<ModelListRequest>;
    async fn get(&self, id: &str) -> anyhow::Result<Option<ModelListRequest>>;
    async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool>;
    async fn pop_pending(&self, runtime_id: &str) -> anyhow::Result<Option<ModelListRequest>>;
    async fn complete(
        &self,
        id: &str,
        models: &[ModelEntry],
        supported: bool,
        session_modes: &[SessionModeEntry],
    ) -> anyhow::Result<()>;
    async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()>;
}

#[async_trait]
pub trait ModelCatalogCacheBackend: Send + Sync {
    async fn get(&self, runtime_id: &str) -> anyhow::Result<Option<ModelCatalogSnapshot>>;
    async fn put(
        &self,
        runtime_id: &str,
        models: &[ModelEntry],
        supported: bool,
        session_modes: &[SessionModeEntry],
    ) -> anyhow::Result<()>;
    async fn invalidate(&self, runtime_id: &str) -> anyhow::Result<()>;
}

#[async_trait]
pub trait LocalSkillListStoreBackend: Send + Sync {
    async fn create(&self, runtime_id: &str) -> anyhow::Result<RuntimeLocalSkillListRequest>;
    async fn get(&self, id: &str) -> anyhow::Result<Option<RuntimeLocalSkillListRequest>>;
    async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool>;
    async fn pop_pending(
        &self,
        runtime_id: &str,
    ) -> anyhow::Result<Option<RuntimeLocalSkillListRequest>>;
    async fn complete(
        &self,
        id: &str,
        skills: &[RuntimeLocalSkillSummary],
        supported: bool,
        mcp_servers: &[RuntimeLocalMcpServerSummary],
        mcp_supported: bool,
    ) -> anyhow::Result<()>;
    async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()>;
}

#[async_trait]
pub trait LocalSkillImportStoreBackend: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    async fn create_import(
        &self,
        runtime_id: &str,
        creator_id: &str,
        skill_key: &str,
        name: Option<String>,
        description: Option<String>,
        action: &str,
        target_skill_id: &str,
        supports_conflict: bool,
    ) -> anyhow::Result<RuntimeLocalSkillImportRequest>;
    async fn get(&self, id: &str) -> anyhow::Result<Option<RuntimeLocalSkillImportRequest>>;
    async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool>;
    async fn pop_pending(
        &self,
        runtime_id: &str,
    ) -> anyhow::Result<Option<RuntimeLocalSkillImportRequest>>;
    async fn pop_pending_batch(
        &self,
        runtime_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<RuntimeLocalSkillImportRequest>>;
    async fn complete(&self, id: &str, skill: Value) -> anyhow::Result<()>;
    async fn conflict(&self, id: &str, info: LocalSkillImportConflict) -> anyhow::Result<()>;
    async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()>;
}

fn lock_map<'a, T>(
    mutex: &'a Mutex<HashMap<String, T>>,
    name: &'static str,
) -> anyhow::Result<MutexGuard<'a, HashMap<String, T>>> {
    mutex
        .lock()
        .map_err(|_| anyhow::anyhow!("{name} store lock poisoned"))
}

fn retain_fresh<T>(
    requests: &mut HashMap<String, T>,
    now: DateTime<Utc>,
    retention_secs: i64,
    created_at: impl Fn(&T) -> DateTime<Utc>,
) {
    requests.retain(|_, request| {
        now.signed_duration_since(created_at(request)).num_seconds() <= retention_secs
    });
}

fn oldest_id_by_created<T>(
    requests: &HashMap<String, T>,
    runtime_id: &str,
    runtime_of: impl Fn(&T) -> &str,
    is_pending: impl Fn(&T) -> bool,
    created_at: impl Fn(&T) -> DateTime<Utc>,
    id_of: impl Fn(&T) -> &str,
) -> Option<String> {
    requests
        .values()
        .filter(|request| runtime_of(request) == runtime_id && is_pending(request))
        .min_by(|left, right| created_at(left).cmp(&created_at(right)))
        .map(|request| id_of(request).to_string())
}

fn apply_skill_list_timeout(request: &mut RuntimeLocalSkillListRequest, now: DateTime<Utc>) {
    let was_running = request.status == LocalSkillRequestStatus::Running;
    if skill_timed_out(
        &request.status,
        request.created_at,
        request.run_started_at,
        now,
    ) {
        request.status = LocalSkillRequestStatus::Timeout;
        request.error = if was_running {
            "daemon did not finish within 60 seconds".into()
        } else {
            "daemon did not respond within 3 minutes".into()
        };
        request.updated_at = now;
    }
}

fn apply_skill_import_timeout(request: &mut RuntimeLocalSkillImportRequest, now: DateTime<Utc>) {
    let was_running = request.status == LocalSkillRequestStatus::Running;
    if skill_timed_out(
        &request.status,
        request.created_at,
        request.run_started_at,
        now,
    ) {
        request.status = LocalSkillRequestStatus::Timeout;
        request.error = if was_running {
            "daemon did not finish within 60 seconds".into()
        } else {
            "daemon did not respond within 3 minutes".into()
        };
        request.updated_at = now;
    }
}

/// Single-node update lifecycle used when Redis is intentionally unset.
#[derive(Default)]
pub struct InMemoryUpdateStore {
    requests: Mutex<HashMap<String, UpdateRequest>>,
}

impl InMemoryUpdateStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl UpdateStoreBackend for InMemoryUpdateStore {
    async fn create(
        &self,
        runtime_id: &str,
        target_version: &str,
        initiator_user_id: &str,
    ) -> anyhow::Result<UpdateRequest> {
        let now = Utc::now();
        let mut requests = lock_map(&self.requests, "update")?;
        retain_fresh(&mut requests, now, UPDATE_STORE_RETENTION_SECS, |request| {
            request.created_at
        });
        if requests.values().any(|request| {
            request.runtime_id == runtime_id
                && matches!(
                    request.status,
                    UpdateStatus::Pending | UpdateStatus::Running
                )
        }) {
            anyhow::bail!("update already in progress");
        }
        let request = UpdateRequest {
            id: random_id(),
            runtime_id: runtime_id.to_string(),
            initiator_user_id: initiator_user_id.to_string(),
            status: UpdateStatus::Pending,
            target_version: target_version.to_string(),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        requests.insert(request.id.clone(), request.clone());
        Ok(request)
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<UpdateRequest>> {
        let mut requests = lock_map(&self.requests, "update")?;
        let Some(request) = requests.get_mut(id) else {
            return Ok(None);
        };
        apply_update_timeout(request, Utc::now());
        Ok(Some(request.clone()))
    }

    async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool> {
        let now = Utc::now();
        let mut requests = lock_map(&self.requests, "update")?;
        for request in requests.values_mut() {
            apply_update_timeout(request, now);
        }
        Ok(requests.values().any(|request| {
            request.runtime_id == runtime_id && request.status == UpdateStatus::Pending
        }))
    }

    async fn pop_pending(&self, runtime_id: &str) -> anyhow::Result<Option<UpdateRequest>> {
        let now = Utc::now();
        let mut requests = lock_map(&self.requests, "update")?;
        for request in requests.values_mut() {
            apply_update_timeout(request, now);
        }
        let Some(oldest_id) = oldest_id_by_created(
            &requests,
            runtime_id,
            |request| &request.runtime_id,
            |request| request.status == UpdateStatus::Pending,
            |request| request.created_at,
            |request| &request.id,
        ) else {
            return Ok(None);
        };
        let Some(request) = requests.get_mut(&oldest_id) else {
            return Ok(None);
        };
        request.status = UpdateStatus::Running;
        request.run_started_at = Some(now);
        request.updated_at = now;
        Ok(Some(request.clone()))
    }

    async fn complete(&self, id: &str, output: &str) -> anyhow::Result<()> {
        if let Some(request) = lock_map(&self.requests, "update")?.get_mut(id) {
            if !request.status.is_terminal() {
                request.status = UpdateStatus::Completed;
                request.output = output.to_string();
                request.updated_at = Utc::now();
            }
        }
        Ok(())
    }

    async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()> {
        if let Some(request) = lock_map(&self.requests, "update")?.get_mut(id) {
            if !request.status.is_terminal() {
                request.status = UpdateStatus::Failed;
                request.error = err_msg.to_string();
                request.updated_at = Utc::now();
            }
        }
        Ok(())
    }
}

#[async_trait]
impl UpdateStoreBackend for UpdateStore {
    async fn create(
        &self,
        runtime_id: &str,
        target_version: &str,
        initiator_user_id: &str,
    ) -> anyhow::Result<UpdateRequest> {
        UpdateStore::create(self, runtime_id, target_version, initiator_user_id).await
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<UpdateRequest>> {
        UpdateStore::get(self, id).await
    }

    async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool> {
        UpdateStore::has_pending(self, runtime_id).await
    }

    async fn pop_pending(&self, runtime_id: &str) -> anyhow::Result<Option<UpdateRequest>> {
        UpdateStore::pop_pending(self, runtime_id).await
    }

    async fn complete(&self, id: &str, output: &str) -> anyhow::Result<()> {
        UpdateStore::complete(self, id, output).await
    }

    async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()> {
        UpdateStore::fail(self, id, err_msg).await
    }
}

#[derive(Default)]
pub struct InMemoryModelListStore {
    requests: Mutex<HashMap<String, ModelListRequest>>,
}

impl InMemoryModelListStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ModelListStoreBackend for InMemoryModelListStore {
    async fn create(&self, runtime_id: &str) -> anyhow::Result<ModelListRequest> {
        let now = Utc::now();
        let mut requests = lock_map(&self.requests, "model list")?;
        retain_fresh(
            &mut requests,
            now,
            MODEL_LIST_STORE_RETENTION_SECS,
            |request| request.created_at,
        );
        let request = ModelListRequest {
            id: random_id(),
            runtime_id: runtime_id.to_string(),
            status: ModelListStatus::Pending,
            supported: true,
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        requests.insert(request.id.clone(), request.clone());
        Ok(request)
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<ModelListRequest>> {
        let mut requests = lock_map(&self.requests, "model list")?;
        let Some(request) = requests.get_mut(id) else {
            return Ok(None);
        };
        apply_model_list_timeout(request, Utc::now());
        Ok(Some(request.clone()))
    }

    async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool> {
        let now = Utc::now();
        let mut requests = lock_map(&self.requests, "model list")?;
        for request in requests.values_mut() {
            apply_model_list_timeout(request, now);
        }
        Ok(requests.values().any(|request| {
            request.runtime_id == runtime_id && request.status == ModelListStatus::Pending
        }))
    }

    async fn pop_pending(&self, runtime_id: &str) -> anyhow::Result<Option<ModelListRequest>> {
        let now = Utc::now();
        let mut requests = lock_map(&self.requests, "model list")?;
        for request in requests.values_mut() {
            apply_model_list_timeout(request, now);
        }
        let Some(oldest_id) = oldest_id_by_created(
            &requests,
            runtime_id,
            |request| &request.runtime_id,
            |request| request.status == ModelListStatus::Pending,
            |request| request.created_at,
            |request| &request.id,
        ) else {
            return Ok(None);
        };
        let Some(request) = requests.get_mut(&oldest_id) else {
            return Ok(None);
        };
        request.status = ModelListStatus::Running;
        request.run_started_at = Some(now);
        request.updated_at = now;
        Ok(Some(request.clone()))
    }

    async fn complete(
        &self,
        id: &str,
        models: &[ModelEntry],
        supported: bool,
        session_modes: &[SessionModeEntry],
    ) -> anyhow::Result<()> {
        if let Some(request) = lock_map(&self.requests, "model list")?.get_mut(id) {
            request.status = ModelListStatus::Completed;
            request.models = models.to_vec();
            request.supported = supported;
            request.session_modes = session_modes.to_vec();
            request.updated_at = Utc::now();
        }
        Ok(())
    }

    async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()> {
        if let Some(request) = lock_map(&self.requests, "model list")?.get_mut(id) {
            request.status = ModelListStatus::Failed;
            request.error = err_msg.to_string();
            request.updated_at = Utc::now();
        }
        Ok(())
    }
}

#[async_trait]
impl ModelListStoreBackend for RedisModelList {
    async fn create(&self, runtime_id: &str) -> anyhow::Result<ModelListRequest> {
        ModelListStore::create(self, runtime_id).await
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<ModelListRequest>> {
        ModelListStore::get(self, id).await
    }

    async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool> {
        ModelListStore::has_pending(self, runtime_id).await
    }

    async fn pop_pending(&self, runtime_id: &str) -> anyhow::Result<Option<ModelListRequest>> {
        ModelListStore::pop_pending(self, runtime_id).await
    }

    async fn complete(
        &self,
        id: &str,
        models: &[ModelEntry],
        supported: bool,
        session_modes: &[SessionModeEntry],
    ) -> anyhow::Result<()> {
        ModelListStore::complete(self, id, models, supported, session_modes).await
    }

    async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()> {
        ModelListStore::fail(self, id, err_msg).await
    }
}

#[derive(Default)]
pub struct InMemoryModelCatalogCache {
    entries: Mutex<HashMap<String, ModelCatalogSnapshot>>,
}

impl InMemoryModelCatalogCache {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ModelCatalogCacheBackend for InMemoryModelCatalogCache {
    async fn get(&self, runtime_id: &str) -> anyhow::Result<Option<ModelCatalogSnapshot>> {
        if runtime_id.is_empty() {
            return Ok(None);
        }
        let mut entries = lock_map(&self.entries, "model catalog")?;
        let Some(snapshot) = entries.get(runtime_id).cloned() else {
            return Ok(None);
        };
        if Utc::now()
            .signed_duration_since(snapshot.stored_at)
            .num_seconds()
            > MODEL_CATALOG_SERVE_WINDOW_SECS
        {
            entries.remove(runtime_id);
            return Ok(None);
        }
        Ok(Some(snapshot))
    }

    async fn put(
        &self,
        runtime_id: &str,
        models: &[ModelEntry],
        supported: bool,
        session_modes: &[SessionModeEntry],
    ) -> anyhow::Result<()> {
        if runtime_id.is_empty() || !cacheable_model_catalog(models, supported) {
            return Ok(());
        }
        let now = Utc::now();
        let mut entries = lock_map(&self.entries, "model catalog")?;
        retain_fresh(
            &mut entries,
            now,
            MODEL_CATALOG_SERVE_WINDOW_SECS,
            |snapshot| snapshot.stored_at,
        );
        entries.insert(
            runtime_id.to_string(),
            ModelCatalogSnapshot {
                runtime_id: runtime_id.to_string(),
                models: models.to_vec(),
                supported,
                stored_at: now,
                session_modes: session_modes.to_vec(),
            },
        );
        Ok(())
    }

    async fn invalidate(&self, runtime_id: &str) -> anyhow::Result<()> {
        if runtime_id.is_empty() {
            return Ok(());
        }
        lock_map(&self.entries, "model catalog")?.remove(runtime_id);
        Ok(())
    }
}

#[async_trait]
impl ModelCatalogCacheBackend for super::ModelCatalogCache {
    async fn get(&self, runtime_id: &str) -> anyhow::Result<Option<ModelCatalogSnapshot>> {
        super::ModelCatalogCache::get(self, runtime_id).await
    }

    async fn put(
        &self,
        runtime_id: &str,
        models: &[ModelEntry],
        supported: bool,
        session_modes: &[SessionModeEntry],
    ) -> anyhow::Result<()> {
        super::ModelCatalogCache::put(self, runtime_id, models, supported, session_modes).await
    }

    async fn invalidate(&self, runtime_id: &str) -> anyhow::Result<()> {
        super::ModelCatalogCache::invalidate(self, runtime_id).await
    }
}

#[derive(Default)]
pub struct InMemoryLocalSkillListStore {
    requests: Mutex<HashMap<String, RuntimeLocalSkillListRequest>>,
}

impl InMemoryLocalSkillListStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl LocalSkillListStoreBackend for InMemoryLocalSkillListStore {
    async fn create(&self, runtime_id: &str) -> anyhow::Result<RuntimeLocalSkillListRequest> {
        let now = Utc::now();
        let mut requests = lock_map(&self.requests, "local skill list")?;
        retain_fresh(
            &mut requests,
            now,
            LOCAL_SKILL_STORE_RETENTION_SECS,
            |request| request.created_at,
        );
        let request = RuntimeLocalSkillListRequest {
            id: random_id(),
            runtime_id: runtime_id.to_string(),
            status: LocalSkillRequestStatus::Pending,
            supported: true,
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        requests.insert(request.id.clone(), request.clone());
        Ok(request)
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<RuntimeLocalSkillListRequest>> {
        let mut requests = lock_map(&self.requests, "local skill list")?;
        let Some(request) = requests.get_mut(id) else {
            return Ok(None);
        };
        apply_skill_list_timeout(request, Utc::now());
        Ok(Some(request.clone()))
    }

    async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool> {
        let now = Utc::now();
        let mut requests = lock_map(&self.requests, "local skill list")?;
        for request in requests.values_mut() {
            apply_skill_list_timeout(request, now);
        }
        Ok(requests.values().any(|request| {
            request.runtime_id == runtime_id && request.status == LocalSkillRequestStatus::Pending
        }))
    }

    async fn pop_pending(
        &self,
        runtime_id: &str,
    ) -> anyhow::Result<Option<RuntimeLocalSkillListRequest>> {
        let now = Utc::now();
        let mut requests = lock_map(&self.requests, "local skill list")?;
        for request in requests.values_mut() {
            apply_skill_list_timeout(request, now);
        }
        let Some(oldest_id) = oldest_id_by_created(
            &requests,
            runtime_id,
            |request| &request.runtime_id,
            |request| request.status == LocalSkillRequestStatus::Pending,
            |request| request.created_at,
            |request| &request.id,
        ) else {
            return Ok(None);
        };
        let Some(request) = requests.get_mut(&oldest_id) else {
            return Ok(None);
        };
        request.status = LocalSkillRequestStatus::Running;
        request.run_started_at = Some(now);
        request.updated_at = now;
        Ok(Some(request.clone()))
    }

    async fn complete(
        &self,
        id: &str,
        skills: &[RuntimeLocalSkillSummary],
        supported: bool,
        mcp_servers: &[RuntimeLocalMcpServerSummary],
        mcp_supported: bool,
    ) -> anyhow::Result<()> {
        if let Some(request) = lock_map(&self.requests, "local skill list")?.get_mut(id) {
            request.status = LocalSkillRequestStatus::Completed;
            request.skills = skills.to_vec();
            request.supported = supported;
            request.mcp_servers = mcp_servers.to_vec();
            request.mcp_supported = mcp_supported;
            request.updated_at = Utc::now();
        }
        Ok(())
    }

    async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()> {
        if let Some(request) = lock_map(&self.requests, "local skill list")?.get_mut(id) {
            request.status = LocalSkillRequestStatus::Failed;
            request.error = err_msg.to_string();
            request.updated_at = Utc::now();
        }
        Ok(())
    }
}

#[async_trait]
impl LocalSkillListStoreBackend for super::LocalSkillListStore {
    async fn create(&self, runtime_id: &str) -> anyhow::Result<RuntimeLocalSkillListRequest> {
        super::LocalSkillListStore::create(self, runtime_id).await
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<RuntimeLocalSkillListRequest>> {
        super::LocalSkillListStore::get(self, id).await
    }

    async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool> {
        super::LocalSkillListStore::has_pending(self, runtime_id).await
    }

    async fn pop_pending(
        &self,
        runtime_id: &str,
    ) -> anyhow::Result<Option<RuntimeLocalSkillListRequest>> {
        super::LocalSkillListStore::pop_pending(self, runtime_id).await
    }

    async fn complete(
        &self,
        id: &str,
        skills: &[RuntimeLocalSkillSummary],
        supported: bool,
        mcp_servers: &[RuntimeLocalMcpServerSummary],
        mcp_supported: bool,
    ) -> anyhow::Result<()> {
        super::LocalSkillListStore::complete(
            self,
            id,
            skills,
            supported,
            mcp_servers,
            mcp_supported,
        )
        .await
    }

    async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()> {
        super::LocalSkillListStore::fail(self, id, err_msg).await
    }
}

#[derive(Default)]
pub struct InMemoryLocalSkillImportStore {
    requests: Mutex<HashMap<String, RuntimeLocalSkillImportRequest>>,
}

impl InMemoryLocalSkillImportStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl LocalSkillImportStoreBackend for InMemoryLocalSkillImportStore {
    async fn create_import(
        &self,
        runtime_id: &str,
        creator_id: &str,
        skill_key: &str,
        name: Option<String>,
        description: Option<String>,
        action: &str,
        target_skill_id: &str,
        supports_conflict: bool,
    ) -> anyhow::Result<RuntimeLocalSkillImportRequest> {
        let now = Utc::now();
        let mut requests = lock_map(&self.requests, "local skill import")?;
        retain_fresh(
            &mut requests,
            now,
            LOCAL_SKILL_STORE_RETENTION_SECS,
            |request| request.created_at,
        );
        let request = RuntimeLocalSkillImportRequest {
            id: random_id(),
            runtime_id: runtime_id.to_string(),
            skill_key: skill_key.to_string(),
            name,
            description,
            action: action.to_string(),
            target_skill_id: target_skill_id.to_string(),
            supports_conflict,
            status: LocalSkillRequestStatus::Pending,
            created_at: now,
            updated_at: now,
            creator_id: creator_id.to_string(),
            ..Default::default()
        };
        requests.insert(request.id.clone(), request.clone());
        Ok(request)
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<RuntimeLocalSkillImportRequest>> {
        let mut requests = lock_map(&self.requests, "local skill import")?;
        let Some(request) = requests.get_mut(id) else {
            return Ok(None);
        };
        apply_skill_import_timeout(request, Utc::now());
        Ok(Some(request.clone()))
    }

    async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool> {
        let now = Utc::now();
        let mut requests = lock_map(&self.requests, "local skill import")?;
        for request in requests.values_mut() {
            apply_skill_import_timeout(request, now);
        }
        Ok(requests.values().any(|request| {
            request.runtime_id == runtime_id && request.status == LocalSkillRequestStatus::Pending
        }))
    }

    async fn pop_pending(
        &self,
        runtime_id: &str,
    ) -> anyhow::Result<Option<RuntimeLocalSkillImportRequest>> {
        Ok(self
            .pop_pending_batch(runtime_id, 1)
            .await?
            .into_iter()
            .next())
    }

    async fn pop_pending_batch(
        &self,
        runtime_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<RuntimeLocalSkillImportRequest>> {
        let now = Utc::now();
        let mut requests = lock_map(&self.requests, "local skill import")?;
        for request in requests.values_mut() {
            apply_skill_import_timeout(request, now);
        }
        let mut ids = requests
            .values()
            .filter(|request| {
                request.runtime_id == runtime_id
                    && request.status == LocalSkillRequestStatus::Pending
            })
            .map(|request| (request.created_at, request.id.clone()))
            .collect::<Vec<_>>();
        ids.sort_by_key(|left| left.0);
        let mut claimed = Vec::new();
        for (_, id) in ids.into_iter().take(limit) {
            let Some(request) = requests.get_mut(&id) else {
                continue;
            };
            request.status = LocalSkillRequestStatus::Running;
            request.run_started_at = Some(now);
            request.updated_at = now;
            claimed.push(request.clone());
        }
        Ok(claimed)
    }

    async fn complete(&self, id: &str, skill: Value) -> anyhow::Result<()> {
        if let Some(request) = lock_map(&self.requests, "local skill import")?.get_mut(id) {
            request.status = LocalSkillRequestStatus::Completed;
            request.skill = Some(skill);
            request.updated_at = Utc::now();
        }
        Ok(())
    }

    async fn conflict(&self, id: &str, info: LocalSkillImportConflict) -> anyhow::Result<()> {
        if let Some(request) = lock_map(&self.requests, "local skill import")?.get_mut(id) {
            request.status = LocalSkillRequestStatus::Conflict;
            request.conflict = Some(info);
            request.updated_at = Utc::now();
        }
        Ok(())
    }

    async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()> {
        if let Some(request) = lock_map(&self.requests, "local skill import")?.get_mut(id) {
            request.status = LocalSkillRequestStatus::Failed;
            request.error = err_msg.to_string();
            request.updated_at = Utc::now();
        }
        Ok(())
    }
}

#[async_trait]
impl LocalSkillImportStoreBackend for super::LocalSkillImportStore {
    async fn create_import(
        &self,
        runtime_id: &str,
        creator_id: &str,
        skill_key: &str,
        name: Option<String>,
        description: Option<String>,
        action: &str,
        target_skill_id: &str,
        supports_conflict: bool,
    ) -> anyhow::Result<RuntimeLocalSkillImportRequest> {
        super::LocalSkillImportStore::create_import(
            self,
            runtime_id,
            creator_id,
            skill_key,
            name,
            description,
            action,
            target_skill_id,
            supports_conflict,
        )
        .await
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<RuntimeLocalSkillImportRequest>> {
        super::LocalSkillImportStore::get(self, id).await
    }

    async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool> {
        super::LocalSkillImportStore::has_pending(self, runtime_id).await
    }

    async fn pop_pending(
        &self,
        runtime_id: &str,
    ) -> anyhow::Result<Option<RuntimeLocalSkillImportRequest>> {
        super::LocalSkillImportStore::pop_pending(self, runtime_id).await
    }

    async fn pop_pending_batch(
        &self,
        runtime_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<RuntimeLocalSkillImportRequest>> {
        super::LocalSkillImportStore::pop_pending_batch(self, runtime_id, limit).await
    }

    async fn complete(&self, id: &str, skill: Value) -> anyhow::Result<()> {
        super::LocalSkillImportStore::complete(self, id, skill).await
    }

    async fn conflict(&self, id: &str, info: LocalSkillImportConflict) -> anyhow::Result<()> {
        super::LocalSkillImportStore::conflict(self, id, info).await
    }

    async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()> {
        super::LocalSkillImportStore::fail(self, id, err_msg).await
    }
}
