//! Pending request stores — port of
//! `runtime_models_redis_store.go` and `runtime_local_skills_redis_store.go`.
//!
//! CLI updates, model-list probes and runtime-local-skill requests share the
//! same pending shape: the frontend creates the request, the daemon claims it
//! on heartbeat (or its WS twin), the daemon reports a terminal result, and
//! the UI polls by request id. Multi-node deploys store lifecycle in Redis;
//! Redis-free single-node boots use the in-memory backends Go installs by
//! default.
//!
//! Redis key formats are preserved byte-for-byte with Go:
//! `patchbay:{runtime_pending}:update:req:{id}` etc. Envelope JSON (`{"r":..}` +
//! private side fields) matches as well.

use chrono::{DateTime, Utc};
use patchbay_redis::RecoveringConnection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::time::Duration;

#[path = "pending_store_memory.rs"]
mod memory;
pub use memory::{
    InMemoryLocalSkillImportStore, InMemoryLocalSkillListStore, InMemoryModelCatalogCache,
    InMemoryModelListStore, InMemoryUpdateStore, LocalSkillImportStoreBackend,
    LocalSkillListStoreBackend, ModelCatalogCacheBackend, ModelListStoreBackend,
    UpdateStoreBackend,
};

// Key namespaces (identical to Go).
pub const UPDATE_KEY_PREFIX: &str = "patchbay:{runtime_pending}:update:req:";
pub const UPDATE_PENDING_PREFIX: &str = "patchbay:{runtime_pending}:update:pending:";
pub const UPDATE_ACTIVE_PREFIX: &str = "patchbay:{runtime_pending}:update:active:";
pub const MODEL_LIST_KEY_PREFIX: &str = "patchbay:{runtime_pending}:model_list:req:";
pub const MODEL_LIST_PENDING_PREFIX: &str = "patchbay:{runtime_pending}:model_list:pending:";
pub const MODEL_CATALOG_KEY_PREFIX: &str = "patchbay:runtime_model_catalog:";
pub const LOCAL_SKILL_LIST_KEY_PREFIX: &str = "patchbay:{runtime_pending}:local_skill:list:";
pub const LOCAL_SKILL_LIST_PENDING_PREFIX: &str =
    "patchbay:{runtime_pending}:local_skill:list:pending:";
pub const LOCAL_SKILL_IMPORT_KEY_PREFIX: &str = "patchbay:{runtime_pending}:local_skill:import:";
pub const LOCAL_SKILL_IMPORT_PENDING_PREFIX: &str =
    "patchbay:{runtime_pending}:local_skill:import:pending:";

fn update_key(id: &str) -> String {
    format!("{UPDATE_KEY_PREFIX}{id}")
}
fn update_pending_key(runtime_id: &str) -> String {
    format!("{UPDATE_PENDING_PREFIX}{runtime_id}")
}
fn update_active_key(runtime_id: &str) -> String {
    format!("{UPDATE_ACTIVE_PREFIX}{runtime_id}")
}
fn model_list_key(id: &str) -> String {
    format!("{MODEL_LIST_KEY_PREFIX}{id}")
}
fn model_list_pending_key(runtime_id: &str) -> String {
    format!("{MODEL_LIST_PENDING_PREFIX}{runtime_id}")
}
fn model_catalog_key(runtime_id: &str) -> String {
    format!("{MODEL_CATALOG_KEY_PREFIX}{runtime_id}")
}
fn local_skill_list_key(id: &str) -> String {
    format!("{LOCAL_SKILL_LIST_KEY_PREFIX}{id}")
}
fn local_skill_list_pending_key(runtime_id: &str) -> String {
    format!("{LOCAL_SKILL_LIST_PENDING_PREFIX}{runtime_id}")
}
fn local_skill_import_key(id: &str) -> String {
    format!("{LOCAL_SKILL_IMPORT_KEY_PREFIX}{id}")
}
fn local_skill_import_pending_key(runtime_id: &str) -> String {
    format!("{LOCAL_SKILL_IMPORT_PENDING_PREFIX}{runtime_id}")
}

// TTLs (Go constants).
const UPDATE_PENDING_TIMEOUT_SECS: i64 = 120;
const UPDATE_RUNNING_TIMEOUT_SECS: i64 = 150;
const UPDATE_STORE_RETENTION_SECS: i64 = 5 * 60;
const MODEL_LIST_PENDING_TIMEOUT_SECS: i64 = 30;
const MODEL_LIST_RUNNING_TIMEOUT_SECS: i64 = 60;
const MODEL_LIST_STORE_RETENTION_SECS: i64 = 2 * 60;
pub const MODEL_CATALOG_REVALIDATE_AFTER_SECS: i64 = 60;
const MODEL_CATALOG_SERVE_WINDOW_SECS: i64 = 24 * 60 * 60;
/// Bounds cache I/O so a half-open Redis connection cannot stall the model picker.
const MODEL_CATALOG_REDIS_TIMEOUT: Duration = Duration::from_millis(250);
const LOCAL_SKILL_PENDING_TIMEOUT_SECS: i64 = 3 * 60;
const LOCAL_SKILL_RUNNING_TIMEOUT_SECS: i64 = 60;
const LOCAL_SKILL_STORE_RETENTION_SECS: i64 = 5 * 60;
const PENDING_REDIS_TIMEOUT: Duration = Duration::from_secs(1);
/// Bounds the cheap HasPending probe on the heartbeat hot path.
pub const HEARTBEAT_HAS_PENDING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
/// Max claims of a single pending queue entry per PopPending call.
const POP_MAX_RETRIES: usize = 5;

/// Go `randomID`: 16 random bytes hex-encoded.
pub fn random_id() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

// Go `claimPendingScript`: atomically ZREMs the pending entry and flips the
// record to running. Either both happen or neither does.
const CLAIM_PENDING_SCRIPT: &str = r#"
local removed = redis.call('ZREM', KEYS[1], ARGV[1])
if removed == 0 then
    return 0
end
redis.call('SET', KEYS[2], ARGV[2], 'EX', tonumber(ARGV[3]))
return 1
"#;

// Go `deleteIfValueScript`.
const DELETE_IF_VALUE_SCRIPT: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then
    return redis.call('DEL', KEYS[1])
end
return 0
"#;

async fn bounded_pending_redis<T, F>(operation: &'static str, future: F) -> anyhow::Result<T>
where
    F: Future<Output = redis::RedisResult<T>>,
{
    bounded_pending_redis_with_timeout(PENDING_REDIS_TIMEOUT, operation, future).await
}

async fn bounded_pending_redis_with_timeout<T, F>(
    timeout: Duration,
    operation: &'static str,
    future: F,
) -> anyhow::Result<T>
where
    F: Future<Output = redis::RedisResult<T>>,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(anyhow::anyhow!(
            "runtime pending Redis {operation} failed: {error}"
        )),
        Err(_) => Err(anyhow::anyhow!(
            "runtime pending Redis {operation} timed out"
        )),
    }
}

async fn run_claim_script(
    conn: &mut RecoveringConnection,
    pending_key: &str,
    record_key: &str,
    id: &str,
    data: &str,
    ttl_secs: i64,
) -> anyhow::Result<bool> {
    let script = redis::Script::new(CLAIM_PENDING_SCRIPT);
    let mut invocation = script.prepare_invoke();
    invocation
        .key(pending_key)
        .key(record_key)
        .arg(id)
        .arg(data)
        .arg(ttl_secs);
    let result: i64 = bounded_pending_redis("claim", invocation.invoke_async(conn)).await?;
    Ok(result == 1)
}

async fn zcard(conn: &mut RecoveringConnection, key: &str) -> anyhow::Result<i64> {
    let mut command = redis::cmd("ZCARD");
    command.arg(key);
    bounded_pending_redis("zcard", command.query_async(conn)).await
}

async fn get_bytes(conn: &mut RecoveringConnection, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
    let mut command = redis::cmd("GET");
    command.arg(key);
    bounded_pending_redis("get", command.query_async(conn)).await
}

async fn zrem(conn: &mut RecoveringConnection, key: &str, member: &str) -> anyhow::Result<()> {
    let mut command = redis::cmd("ZREM");
    command.arg(key).arg(member);
    let _: i64 = bounded_pending_redis("zrem", command.query_async(conn)).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Update store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    #[serde(rename = "pending")]
    #[default]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "timeout")]
    Timeout,
}

impl UpdateStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Timeout => "timeout",
        }
    }
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Timeout)
    }
}

/// Public update-request wire shape (Go `UpdateRequest`; the `-` fields ride
/// the envelope's private keys).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateRequest {
    pub id: String,
    #[serde(rename = "runtime_id")]
    pub runtime_id: String,
    #[serde(skip)]
    pub initiator_user_id: String,
    pub status: UpdateStatus,
    #[serde(rename = "target_version")]
    pub target_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(rename = "created_at")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updated_at")]
    pub updated_at: DateTime<Utc>,
    #[serde(skip)]
    pub run_started_at: Option<DateTime<Utc>>,
}

// Go redisUpdateEnvelope: {"r": public, "s": run_started_at, "u": initiator}.
#[derive(Serialize, Deserialize)]
struct UpdateEnvelope {
    #[serde(rename = "r")]
    public: UpdateRequest,
    #[serde(rename = "s", skip_serializing_if = "Option::is_none")]
    run_started_at: Option<DateTime<Utc>>,
    #[serde(rename = "u", skip_serializing_if = "String::is_empty", default)]
    initiator_user_id: String,
}

fn marshal_update(req: &UpdateRequest) -> anyhow::Result<String> {
    let env = UpdateEnvelope {
        public: req.clone(),
        run_started_at: req.run_started_at,
        initiator_user_id: req.initiator_user_id.clone(),
    };
    Ok(serde_json::to_string(&env)?)
}

fn unmarshal_update(raw: &[u8]) -> anyhow::Result<UpdateRequest> {
    let env: UpdateEnvelope = serde_json::from_slice(raw)?;
    let mut req = env.public;
    req.run_started_at = env.run_started_at;
    req.initiator_user_id = env.initiator_user_id;
    Ok(req)
}

/// Go applyUpdateTimeout: lazily transition aged pending/running rows to
/// timeout. Returns true when the row mutated and must be persisted.
fn apply_update_timeout(req: &mut UpdateRequest, now: DateTime<Utc>) -> bool {
    match req.status {
        UpdateStatus::Pending => {
            if now.signed_duration_since(req.created_at).num_seconds() > UPDATE_PENDING_TIMEOUT_SECS
            {
                req.status = UpdateStatus::Timeout;
                req.error = "daemon did not respond within 120 seconds".into();
                req.updated_at = now;
                return true;
            }
        }
        UpdateStatus::Running => {
            if let Some(started) = req.run_started_at {
                if now.signed_duration_since(started).num_seconds() > UPDATE_RUNNING_TIMEOUT_SECS {
                    req.status = UpdateStatus::Timeout;
                    req.error = "update did not complete within 150 seconds".into();
                    req.updated_at = now;
                    return true;
                }
            }
        }
        _ => {}
    }
    false
}

/// Redis-backed CLI-update store (Go `RedisUpdateStore`). `None` connection is
/// never constructed in production; tests construct disabled variants.
pub struct UpdateStore {
    conn: RecoveringConnection,
}

impl UpdateStore {
    pub fn new(conn: RecoveringConnection) -> Self {
        Self { conn }
    }

    async fn load_request(&self, id: &str) -> anyhow::Result<Option<UpdateRequest>> {
        let raw = get_bytes(&mut self.conn.clone(), &update_key(id)).await?;
        let Some(raw) = raw else { return Ok(None) };
        let mut req = unmarshal_update(&raw)?;
        if apply_update_timeout(&mut req, Utc::now()) {
            self.persist_request(&req).await?;
            self.clear_active_if_matches(&req.runtime_id, &req.id)
                .await?;
            zrem(
                &mut self.conn.clone(),
                &update_pending_key(&req.runtime_id),
                &req.id,
            )
            .await?;
        }
        Ok(Some(req))
    }

    async fn persist_request(&self, req: &UpdateRequest) -> anyhow::Result<()> {
        let data = marshal_update(req)?;
        let mut conn = self.conn.clone();
        let mut command = redis::cmd("SET");
        command
            .arg(update_key(&req.id))
            .arg(data)
            .arg("EX")
            .arg(UPDATE_STORE_RETENTION_SECS);
        let (): () = bounded_pending_redis("persist update", command.query_async(&mut conn))
            .await
            .map_err(|e| anyhow::anyhow!("persist update request: {e}"))?;
        Ok(())
    }

    async fn clear_active_if_matches(&self, runtime_id: &str, id: &str) -> anyhow::Result<()> {
        if runtime_id.is_empty() || id.is_empty() {
            return Ok(());
        }
        let script = redis::Script::new(DELETE_IF_VALUE_SCRIPT);
        let mut invocation = script.prepare_invoke();
        invocation.key(update_active_key(runtime_id)).arg(id);
        let mut conn = self.conn.clone();
        let _: i64 =
            bounded_pending_redis("clear active update", invocation.invoke_async(&mut conn))
                .await
                .map_err(|e| anyhow::anyhow!("clear active update: {e}"))?;
        Ok(())
    }

    /// Go Create: reserve the per-runtime active slot, write the record and
    /// enqueue it on the pending zset.
    pub async fn create(
        &self,
        runtime_id: &str,
        target_version: &str,
        initiator_user_id: &str,
    ) -> anyhow::Result<UpdateRequest> {
        let now = Utc::now();
        let req = UpdateRequest {
            id: random_id(),
            runtime_id: runtime_id.to_string(),
            initiator_user_id: initiator_user_id.to_string(),
            status: UpdateStatus::Pending,
            target_version: target_version.to_string(),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        let data = marshal_update(&req)?;
        let active_key = update_active_key(runtime_id);
        let mut conn = self.conn.clone();
        let mut reserve = redis::cmd("SET");
        reserve
            .arg(&active_key)
            .arg(&req.id)
            .arg("EX")
            .arg(UPDATE_STORE_RETENTION_SECS)
            .arg("NX");
        let ok: Option<String> =
            bounded_pending_redis("reserve active update", reserve.query_async(&mut conn))
                .await
                .map_err(|e| anyhow::anyhow!("reserve active update: {e}"))?;
        if ok.is_none() {
            anyhow::bail!("update already in progress");
        }
        let pending_key = update_pending_key(runtime_id);
        let mut pipe = redis::pipe();
        pipe.cmd("SET")
            .arg(update_key(&req.id))
            .arg(&data)
            .arg("EX")
            .arg(UPDATE_STORE_RETENTION_SECS)
            .ignore()
            .cmd("ZADD")
            .arg(&pending_key)
            .arg(now.timestamp_nanos_opt().unwrap_or_default())
            .arg(&req.id)
            .ignore()
            .cmd("EXPIRE")
            .arg(&pending_key)
            .arg(UPDATE_STORE_RETENTION_SECS * 2)
            .ignore();
        let (): () = bounded_pending_redis("persist update", pipe.query_async(&mut conn))
            .await
            .map_err(|e| anyhow::anyhow!("persist update request: {e}"))?;
        Ok(req)
    }

    pub async fn get(&self, id: &str) -> anyhow::Result<Option<UpdateRequest>> {
        self.load_request(id).await
    }

    pub async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool> {
        let cnt = zcard(&mut self.conn.clone(), &update_pending_key(runtime_id))
            .await
            .map_err(|e| anyhow::anyhow!("zcard pending updates: {e}"))?;
        Ok(cnt > 0)
    }

    /// Claims the oldest pending update for this runtime (Go PopPending).
    pub async fn pop_pending(&self, runtime_id: &str) -> anyhow::Result<Option<UpdateRequest>> {
        let pending_key = update_pending_key(runtime_id);
        let mut conn = self.conn.clone();
        let mut range = redis::cmd("ZRANGE");
        range.arg(&pending_key).arg(0).arg(0);
        let ids: Vec<String> =
            bounded_pending_redis("list pending updates", range.query_async(&mut conn))
                .await
                .map_err(|e| anyhow::anyhow!("zrange pending updates: {e}"))?;
        let Some(id) = ids.into_iter().next() else {
            return Ok(None);
        };
        // The Go loop re-reads the zset each attempt; with one candidate per
        // iteration this collapses to bounded retries over fresh reads.
        for _ in 0..POP_MAX_RETRIES {
            let Some(mut req) = self.load_request(&id).await? else {
                zrem(&mut self.conn.clone(), &pending_key, &id).await?;
                break;
            };
            if req.status != UpdateStatus::Pending {
                zrem(&mut self.conn.clone(), &pending_key, &id).await?;
                break;
            }
            let now = Utc::now();
            req.status = UpdateStatus::Running;
            req.run_started_at = Some(now);
            req.updated_at = now;
            let data = marshal_update(&req)?;
            let won = run_claim_script(
                &mut self.conn.clone(),
                &pending_key,
                &update_key(&id),
                &id,
                &data,
                UPDATE_STORE_RETENTION_SECS,
            )
            .await
            .map_err(|e| anyhow::anyhow!("claim pending update: {e}"))?;
            if won {
                return Ok(Some(req));
            }
        }
        Ok(None)
    }

    pub async fn complete(&self, id: &str, output: &str) -> anyhow::Result<()> {
        let Some(mut req) = self.load_request(id).await? else {
            return Ok(());
        };
        if req.status.is_terminal() {
            return Ok(());
        }
        req.status = UpdateStatus::Completed;
        req.output = output.to_string();
        req.updated_at = Utc::now();
        self.persist_request(&req).await?;
        self.clear_active_if_matches(&req.runtime_id, &req.id)
            .await?;
        zrem(
            &mut self.conn.clone(),
            &update_pending_key(&req.runtime_id),
            &req.id,
        )
        .await?;
        Ok(())
    }

    pub async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()> {
        let Some(mut req) = self.load_request(id).await? else {
            return Ok(());
        };
        if req.status.is_terminal() {
            return Ok(());
        }
        req.status = UpdateStatus::Failed;
        req.error = err_msg.to_string();
        req.updated_at = Utc::now();
        self.persist_request(&req).await?;
        self.clear_active_if_matches(&req.runtime_id, &req.id)
            .await?;
        zrem(
            &mut self.conn.clone(),
            &update_pending_key(&req.runtime_id),
            &req.id,
        )
        .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Model list store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ModelListStatus {
    #[serde(rename = "pending")]
    #[default]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "timeout")]
    Timeout,
}

impl ModelListStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Timeout)
    }
}

/// One model entry in a completed catalog (Go `ModelEntry`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelEntry {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub label: String,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub provider: String,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub default: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ModelThinking>,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        rename = "service_tiers",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub service_tiers: Vec<ModelServiceTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelServiceTier {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub id: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub name: String,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelThinking {
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        rename = "supported_levels"
    )]
    pub supported_levels: Vec<ThinkingLevel>,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        rename = "default_level",
        skip_serializing_if = "String::is_empty"
    )]
    pub default_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThinkingLevel {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub value: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub label: String,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub description: String,
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionModeEntry {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub value: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub label: String,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub kind: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelListRequest {
    pub id: String,
    #[serde(rename = "runtime_id")]
    pub runtime_id: String,
    pub status: ModelListStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelEntry>,
    #[serde(default)]
    pub supported: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(rename = "created_at")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updated_at")]
    pub updated_at: DateTime<Utc>,
    #[serde(skip)]
    pub run_started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cached: bool,
    #[serde(default, rename = "cached_at", skip_serializing_if = "Option::is_none")]
    pub cached_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session_modes: Vec<SessionModeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCatalogSnapshot {
    #[serde(rename = "runtime_id")]
    pub runtime_id: String,
    pub models: Vec<ModelEntry>,
    pub supported: bool,
    #[serde(rename = "stored_at")]
    pub stored_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session_modes: Vec<SessionModeEntry>,
}

pub(crate) fn cacheable_model_catalog(models: &[ModelEntry], supported: bool) -> bool {
    supported && !models.is_empty()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelCatalogCacheAction {
    Store,
    Drop,
    Keep,
}

pub(crate) fn model_catalog_cache_action(
    models: &[ModelEntry],
    supported: bool,
    fallback: bool,
) -> ModelCatalogCacheAction {
    if fallback {
        ModelCatalogCacheAction::Keep
    } else if cacheable_model_catalog(models, supported) {
        ModelCatalogCacheAction::Store
    } else {
        ModelCatalogCacheAction::Drop
    }
}

pub struct ModelCatalogCache {
    conn: RecoveringConnection,
}

async fn bounded_model_catalog_redis<T, F>(timeout: Duration, operation: F) -> anyhow::Result<T>
where
    F: Future<Output = redis::RedisResult<T>>,
{
    match tokio::time::timeout(timeout, operation).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(anyhow::anyhow!(
            "model catalog Redis operation failed: {error}"
        )),
        Err(_) => Err(anyhow::anyhow!("model catalog Redis operation timed out")),
    }
}

impl ModelCatalogCache {
    pub fn new(conn: RecoveringConnection) -> Self {
        Self { conn }
    }

    pub async fn get(&self, runtime_id: &str) -> anyhow::Result<Option<ModelCatalogSnapshot>> {
        if runtime_id.is_empty() {
            return Ok(None);
        }
        let key = model_catalog_key(runtime_id);
        let mut conn = self.conn.clone();
        let mut get = redis::cmd("GET");
        get.arg(&key);
        let raw: Option<Vec<u8>> =
            bounded_model_catalog_redis(MODEL_CATALOG_REDIS_TIMEOUT, get.query_async(&mut conn))
                .await
                .map_err(|error| anyhow::anyhow!("get model catalog: {error}"))?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        let snapshot: ModelCatalogSnapshot = match serde_json::from_slice(&raw) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let mut conn = self.conn.clone();
                let mut del = redis::cmd("DEL");
                del.arg(&key);
                let _: anyhow::Result<i64> = bounded_model_catalog_redis(
                    MODEL_CATALOG_REDIS_TIMEOUT,
                    del.query_async(&mut conn),
                )
                .await;
                return Err(anyhow::anyhow!("decode model catalog: {error}"));
            }
        };
        if Utc::now()
            .signed_duration_since(snapshot.stored_at)
            .num_seconds()
            > MODEL_CATALOG_SERVE_WINDOW_SECS
        {
            return Ok(None);
        }
        Ok(Some(snapshot))
    }

    pub async fn put(
        &self,
        runtime_id: &str,
        models: &[ModelEntry],
        supported: bool,
        session_modes: &[SessionModeEntry],
    ) -> anyhow::Result<()> {
        if runtime_id.is_empty() || !cacheable_model_catalog(models, supported) {
            return Ok(());
        }
        let snapshot = ModelCatalogSnapshot {
            runtime_id: runtime_id.to_string(),
            models: models.to_vec(),
            supported,
            stored_at: Utc::now(),
            session_modes: session_modes.to_vec(),
        };
        let data = serde_json::to_string(&snapshot)
            .map_err(|error| anyhow::anyhow!("marshal model catalog: {error}"))?;
        let mut conn = self.conn.clone();
        let mut set = redis::cmd("SET");
        set.arg(model_catalog_key(runtime_id))
            .arg(data)
            .arg("EX")
            .arg(MODEL_CATALOG_SERVE_WINDOW_SECS);
        let (): () =
            bounded_model_catalog_redis(MODEL_CATALOG_REDIS_TIMEOUT, set.query_async(&mut conn))
                .await
                .map_err(|error| anyhow::anyhow!("persist model catalog: {error}"))?;
        Ok(())
    }

    pub async fn invalidate(&self, runtime_id: &str) -> anyhow::Result<()> {
        if runtime_id.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.clone();
        let mut del = redis::cmd("DEL");
        del.arg(model_catalog_key(runtime_id));
        let _: i64 =
            bounded_model_catalog_redis(MODEL_CATALOG_REDIS_TIMEOUT, del.query_async(&mut conn))
                .await
                .map_err(|error| anyhow::anyhow!("invalidate model catalog: {error}"))?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct ModelListEnvelope {
    #[serde(rename = "r")]
    public: ModelListRequest,
    #[serde(rename = "s", skip_serializing_if = "Option::is_none")]
    run_started_at: Option<DateTime<Utc>>,
}

fn marshal_model_list(req: &ModelListRequest) -> anyhow::Result<String> {
    Ok(serde_json::to_string(&ModelListEnvelope {
        public: req.clone(),
        run_started_at: req.run_started_at,
    })?)
}

fn unmarshal_model_list(raw: &[u8]) -> anyhow::Result<ModelListRequest> {
    let env: ModelListEnvelope = serde_json::from_slice(raw)?;
    let mut req = env.public;
    req.run_started_at = env.run_started_at;
    Ok(req)
}

fn apply_model_list_timeout(req: &mut ModelListRequest, now: DateTime<Utc>) -> bool {
    match req.status {
        ModelListStatus::Pending => {
            if now.signed_duration_since(req.created_at).num_seconds()
                > MODEL_LIST_PENDING_TIMEOUT_SECS
            {
                req.status = ModelListStatus::Timeout;
                req.error = "daemon did not respond within 30 seconds".into();
                req.updated_at = now;
                true
            } else {
                false
            }
        }
        ModelListStatus::Running => {
            if let Some(started) = req.run_started_at {
                if now.signed_duration_since(started).num_seconds()
                    > MODEL_LIST_RUNNING_TIMEOUT_SECS
                {
                    req.status = ModelListStatus::Timeout;
                    req.error = "daemon did not finish within 60 seconds".into();
                    req.updated_at = now;
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

pub struct ModelListStore {
    conn: RecoveringConnection,
}

impl ModelListStore {
    pub fn new(conn: RecoveringConnection) -> Self {
        Self { conn }
    }

    async fn load_request(&self, id: &str) -> anyhow::Result<Option<ModelListRequest>> {
        let raw = get_bytes(&mut self.conn.clone(), &model_list_key(id)).await?;
        let Some(raw) = raw else { return Ok(None) };
        let mut req = unmarshal_model_list(&raw)?;
        if apply_model_list_timeout(&mut req, Utc::now()) {
            self.persist_request(&req).await?;
            zrem(
                &mut self.conn.clone(),
                &model_list_pending_key(&req.runtime_id),
                &req.id,
            )
            .await?;
        }
        Ok(Some(req))
    }

    async fn persist_request(&self, req: &ModelListRequest) -> anyhow::Result<()> {
        let data = marshal_model_list(req)?;
        let mut conn = self.conn.clone();
        let mut command = redis::cmd("SET");
        command
            .arg(model_list_key(&req.id))
            .arg(data)
            .arg("EX")
            .arg(MODEL_LIST_STORE_RETENTION_SECS);
        let (): () = bounded_pending_redis("persist model list", command.query_async(&mut conn))
            .await
            .map_err(|e| anyhow::anyhow!("persist model list request: {e}"))?;
        Ok(())
    }

    pub async fn create(&self, runtime_id: &str) -> anyhow::Result<ModelListRequest> {
        let now = Utc::now();
        let req = ModelListRequest {
            id: random_id(),
            runtime_id: runtime_id.to_string(),
            status: ModelListStatus::Pending,
            supported: true,
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        let data = marshal_model_list(&req)?;
        let mut conn = self.conn.clone();
        let pending_key = model_list_pending_key(runtime_id);
        let mut pipe = redis::pipe();
        pipe.cmd("SET")
            .arg(model_list_key(&req.id))
            .arg(&data)
            .arg("EX")
            .arg(MODEL_LIST_STORE_RETENTION_SECS)
            .ignore()
            .cmd("ZADD")
            .arg(&pending_key)
            .arg(now.timestamp_nanos_opt().unwrap_or_default())
            .arg(&req.id)
            .ignore()
            .cmd("EXPIRE")
            .arg(&pending_key)
            .arg(MODEL_LIST_STORE_RETENTION_SECS * 2)
            .ignore();
        let (): () = bounded_pending_redis("persist model list", pipe.query_async(&mut conn))
            .await
            .map_err(|e| anyhow::anyhow!("persist model list request: {e}"))?;
        Ok(req)
    }

    pub async fn get(&self, id: &str) -> anyhow::Result<Option<ModelListRequest>> {
        self.load_request(id).await
    }

    pub async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool> {
        Ok(zcard(&mut self.conn.clone(), &model_list_pending_key(runtime_id)).await? > 0)
    }

    pub async fn pop_pending(&self, runtime_id: &str) -> anyhow::Result<Option<ModelListRequest>> {
        let pending_key = model_list_pending_key(runtime_id);
        for _ in 0..POP_MAX_RETRIES {
            let mut conn = self.conn.clone();
            let mut range = redis::cmd("ZRANGE");
            range.arg(&pending_key).arg(0).arg(0);
            let ids: Vec<String> =
                bounded_pending_redis("list pending model requests", range.query_async(&mut conn))
                    .await
                    .map_err(|e| anyhow::anyhow!("zrange pending: {e}"))?;
            let Some(id) = ids.into_iter().next() else {
                return Ok(None);
            };
            let Some(mut req) = self.load_request(&id).await? else {
                // Record expired but the zset still references it — drop and retry.
                zrem(&mut self.conn.clone(), &pending_key, &id).await?;
                continue;
            };
            if req.status != ModelListStatus::Pending {
                // Timeout fired inside load_request or another node picked it up.
                zrem(&mut self.conn.clone(), &pending_key, &id).await?;
                continue;
            }
            let now = Utc::now();
            req.status = ModelListStatus::Running;
            req.run_started_at = Some(now);
            req.updated_at = now;
            let data = marshal_model_list(&req)?;
            let won = run_claim_script(
                &mut self.conn.clone(),
                &pending_key,
                &model_list_key(&id),
                &id,
                &data,
                MODEL_LIST_STORE_RETENTION_SECS,
            )
            .await
            .map_err(|e| anyhow::anyhow!("claim pending: {e}"))?;
            if won {
                return Ok(Some(req));
            }
        }
        Ok(None)
    }

    pub async fn complete(
        &self,
        id: &str,
        models: &[ModelEntry],
        supported: bool,
        session_modes: &[SessionModeEntry],
    ) -> anyhow::Result<()> {
        let Some(mut req) = self.load_request(id).await? else {
            return Ok(());
        };
        req.status = ModelListStatus::Completed;
        req.models = models.to_vec();
        req.supported = supported;
        req.session_modes = session_modes.to_vec();
        req.updated_at = Utc::now();
        self.persist_request(&req).await
    }

    pub async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()> {
        let Some(mut req) = self.load_request(id).await? else {
            return Ok(());
        };
        req.status = ModelListStatus::Failed;
        req.error = err_msg.to_string();
        req.updated_at = Utc::now();
        self.persist_request(&req).await
    }
}

// ---------------------------------------------------------------------------
// Local-skill stores (list + import)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LocalSkillRequestStatus {
    #[serde(rename = "pending")]
    #[default]
    Pending,
    #[serde(rename = "running")]
    Running,
    #[serde(rename = "completed")]
    Completed,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "conflict")]
    Conflict,
    #[serde(rename = "timeout")]
    Timeout,
}

impl LocalSkillRequestStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Timeout | Self::Conflict
        )
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Conflict => "conflict",
            Self::Timeout => "timeout",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeLocalSkillSummary {
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(rename = "source_path")]
    pub source_path: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub plugin: String,
    #[serde(default)]
    pub can_disable: bool,
    #[serde(default)]
    pub file_count: i32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuntimeLocalMcpServerSummary {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub transport: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeLocalSkillListRequest {
    pub id: String,
    #[serde(rename = "runtime_id")]
    pub runtime_id: String,
    pub status: LocalSkillRequestStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<RuntimeLocalSkillSummary>,
    #[serde(default)]
    pub supported: bool,
    #[serde(default, rename = "mcp_servers", skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<RuntimeLocalMcpServerSummary>,
    #[serde(default, rename = "mcp_supported")]
    pub mcp_supported: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(rename = "created_at")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updated_at")]
    pub updated_at: DateTime<Utc>,
    #[serde(skip)]
    pub run_started_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuntimeLocalSkillImportRequest {
    pub id: String,
    #[serde(rename = "runtime_id")]
    pub runtime_id: String,
    #[serde(rename = "skill_key")]
    pub skill_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, rename = "action", skip_serializing_if = "String::is_empty")]
    pub action: String,
    #[serde(
        default,
        rename = "target_skill_id",
        skip_serializing_if = "String::is_empty"
    )]
    pub target_skill_id: String,
    #[serde(
        default,
        rename = "supports_conflict",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub supports_conflict: bool,
    pub status: LocalSkillRequestStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<LocalSkillImportConflict>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(rename = "created_at")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updated_at")]
    pub updated_at: DateTime<Utc>,
    #[serde(skip)]
    pub run_started_at: Option<DateTime<Utc>>,
    #[serde(skip)]
    pub creator_id: String,
}

/// Structured same-name conflict metadata (Go `LocalSkillImportConflict`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalSkillImportConflict {
    #[serde(rename = "existing_skill_id")]
    pub existing_skill_id: String,
    #[serde(
        rename = "existing_created_by",
        skip_serializing_if = "String::is_empty"
    )]
    pub existing_created_by: String,
    #[serde(rename = "can_overwrite", default)]
    pub can_overwrite: bool,
}

#[derive(Serialize, Deserialize)]
struct LocalSkillImportEnvelope {
    #[serde(rename = "r")]
    public: RuntimeLocalSkillImportRequest,
    #[serde(rename = "c", skip_serializing_if = "String::is_empty", default)]
    creator_id: String,
    #[serde(rename = "s", skip_serializing_if = "Option::is_none")]
    run_started_at: Option<DateTime<Utc>>,
}

fn marshal_skill_list(req: &RuntimeLocalSkillListRequest) -> anyhow::Result<String> {
    Ok(serde_json::to_string(req)?)
}

fn unmarshal_skill_list(raw: &[u8]) -> anyhow::Result<RuntimeLocalSkillListRequest> {
    Ok(serde_json::from_slice(raw)?)
}

fn marshal_skill_import(req: &RuntimeLocalSkillImportRequest) -> anyhow::Result<String> {
    Ok(serde_json::to_string(&LocalSkillImportEnvelope {
        public: req.clone(),
        creator_id: req.creator_id.clone(),
        run_started_at: req.run_started_at,
    })?)
}

fn unmarshal_skill_import(raw: &[u8]) -> anyhow::Result<RuntimeLocalSkillImportRequest> {
    let env: LocalSkillImportEnvelope = serde_json::from_slice(raw)?;
    let mut req = env.public;
    req.creator_id = env.creator_id;
    req.run_started_at = env.run_started_at;
    Ok(req)
}

/// Shared lazy-timeout application for both local-skill request kinds.
fn skill_timed_out(
    status: &LocalSkillRequestStatus,
    created_at: DateTime<Utc>,
    run_started_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    match status {
        LocalSkillRequestStatus::Pending => {
            now.signed_duration_since(created_at).num_seconds() > LOCAL_SKILL_PENDING_TIMEOUT_SECS
        }
        LocalSkillRequestStatus::Running => run_started_at
            .map(|s| now.signed_duration_since(s).num_seconds() > LOCAL_SKILL_RUNNING_TIMEOUT_SECS)
            .unwrap_or(false),
        _ => false,
    }
}

pub struct LocalSkillListStore {
    conn: RecoveringConnection,
}

impl LocalSkillListStore {
    pub fn new(conn: RecoveringConnection) -> Self {
        Self { conn }
    }

    async fn load_request(&self, id: &str) -> anyhow::Result<Option<RuntimeLocalSkillListRequest>> {
        let raw = get_bytes(&mut self.conn.clone(), &local_skill_list_key(id)).await?;
        let Some(raw) = raw else { return Ok(None) };
        let mut req = unmarshal_skill_list(&raw)?;
        let was_running = req.status == LocalSkillRequestStatus::Running;
        if skill_timed_out(&req.status, req.created_at, req.run_started_at, Utc::now()) {
            req.status = LocalSkillRequestStatus::Timeout;
            req.error = if was_running {
                "daemon did not finish within 60 seconds".into()
            } else {
                "daemon did not respond within 3 minutes".into()
            };
            req.updated_at = Utc::now();
            self.persist_request(&req).await?;
            zrem(
                &mut self.conn.clone(),
                &local_skill_list_pending_key(&req.runtime_id),
                &req.id,
            )
            .await?;
        }
        Ok(Some(req))
    }

    async fn persist_request(&self, req: &RuntimeLocalSkillListRequest) -> anyhow::Result<()> {
        let data = marshal_skill_list(req)?;
        let mut conn = self.conn.clone();
        let mut command = redis::cmd("SET");
        command
            .arg(local_skill_list_key(&req.id))
            .arg(data)
            .arg("EX")
            .arg(LOCAL_SKILL_STORE_RETENTION_SECS);
        let (): () =
            bounded_pending_redis("persist local skill list", command.query_async(&mut conn))
                .await
                .map_err(|e| anyhow::anyhow!("persist list request: {e}"))?;
        Ok(())
    }

    pub async fn create(&self, runtime_id: &str) -> anyhow::Result<RuntimeLocalSkillListRequest> {
        let now = Utc::now();
        let req = RuntimeLocalSkillListRequest {
            id: random_id(),
            runtime_id: runtime_id.to_string(),
            status: LocalSkillRequestStatus::Pending,
            supported: true,
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        let data = marshal_skill_list(&req)?;
        let mut conn = self.conn.clone();
        let pending_key = local_skill_list_pending_key(runtime_id);
        let mut pipe = redis::pipe();
        pipe.cmd("SET")
            .arg(local_skill_list_key(&req.id))
            .arg(&data)
            .arg("EX")
            .arg(LOCAL_SKILL_STORE_RETENTION_SECS)
            .ignore()
            .cmd("ZADD")
            .arg(&pending_key)
            .arg(now.timestamp_nanos_opt().unwrap_or_default())
            .arg(&req.id)
            .ignore()
            .cmd("EXPIRE")
            .arg(&pending_key)
            .arg(LOCAL_SKILL_STORE_RETENTION_SECS * 2)
            .ignore();
        let (): () = bounded_pending_redis("persist local skill list", pipe.query_async(&mut conn))
            .await
            .map_err(|e| anyhow::anyhow!("persist list request: {e}"))?;
        Ok(req)
    }

    pub async fn get(&self, id: &str) -> anyhow::Result<Option<RuntimeLocalSkillListRequest>> {
        self.load_request(id).await
    }

    pub async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool> {
        Ok(zcard(
            &mut self.conn.clone(),
            &local_skill_list_pending_key(runtime_id),
        )
        .await?
            > 0)
    }

    pub async fn pop_pending(
        &self,
        runtime_id: &str,
    ) -> anyhow::Result<Option<RuntimeLocalSkillListRequest>> {
        let pending_key = local_skill_list_pending_key(runtime_id);
        for _ in 0..POP_MAX_RETRIES {
            let mut conn = self.conn.clone();
            let mut range = redis::cmd("ZRANGE");
            range.arg(&pending_key).arg(0).arg(0);
            let ids: Vec<String> = bounded_pending_redis(
                "list pending local skill requests",
                range.query_async(&mut conn),
            )
            .await
            .map_err(|e| anyhow::anyhow!("zrange pending: {e}"))?;
            let Some(id) = ids.into_iter().next() else {
                return Ok(None);
            };
            let Some(mut req) = load_local_skill_list(&mut self.conn.clone(), &id).await? else {
                // Record expired but the zset still references it — drop and retry.
                zrem(&mut self.conn.clone(), &pending_key, &id).await?;
                continue;
            };
            if req.status != LocalSkillRequestStatus::Pending {
                // Timeout fired inside load_request or another node picked it up.
                zrem(&mut self.conn.clone(), &pending_key, &id).await?;
                continue;
            }
            let now = Utc::now();
            req.status = LocalSkillRequestStatus::Running;
            req.run_started_at = Some(now);
            req.updated_at = now;
            let data = marshal_skill_list(&req)?;
            let won = run_claim_script(
                &mut self.conn.clone(),
                &pending_key,
                &local_skill_list_key(&id),
                &id,
                &data,
                LOCAL_SKILL_STORE_RETENTION_SECS,
            )
            .await
            .map_err(|e| anyhow::anyhow!("claim pending: {e}"))?;
            if won {
                return Ok(Some(req));
            }
        }
        Ok(None)
    }

    pub async fn complete(
        &self,
        id: &str,
        skills: &[RuntimeLocalSkillSummary],
        supported: bool,
        mcp_servers: &[RuntimeLocalMcpServerSummary],
        mcp_supported: bool,
    ) -> anyhow::Result<()> {
        let Some(mut req) = self.load_request(id).await? else {
            return Ok(());
        };
        req.status = LocalSkillRequestStatus::Completed;
        req.skills = skills.to_vec();
        req.supported = supported;
        req.mcp_servers = mcp_servers.to_vec();
        req.mcp_supported = mcp_supported;
        req.updated_at = Utc::now();
        self.persist_request(&req).await
    }

    pub async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()> {
        let Some(mut req) = self.load_request(id).await? else {
            return Ok(());
        };
        req.status = LocalSkillRequestStatus::Failed;
        req.error = err_msg.to_string();
        req.updated_at = Utc::now();
        self.persist_request(&req).await
    }
}

async fn load_local_skill_list(
    conn: &mut RecoveringConnection,
    id: &str,
) -> anyhow::Result<Option<RuntimeLocalSkillListRequest>> {
    match get_bytes(conn, &local_skill_list_key(id)).await? {
        Some(raw) => Ok(Some(unmarshal_skill_list(&raw)?)),
        None => Ok(None),
    }
}

pub struct LocalSkillImportStore {
    conn: RecoveringConnection,
}

impl LocalSkillImportStore {
    pub fn new(conn: RecoveringConnection) -> Self {
        Self { conn }
    }

    async fn load_request(
        &self,
        id: &str,
    ) -> anyhow::Result<Option<RuntimeLocalSkillImportRequest>> {
        let raw = get_bytes(&mut self.conn.clone(), &local_skill_import_key(id)).await?;
        let Some(raw) = raw else { return Ok(None) };
        let mut req = unmarshal_skill_import(&raw)?;
        let was_running = req.status == LocalSkillRequestStatus::Running;
        if skill_timed_out(&req.status, req.created_at, req.run_started_at, Utc::now()) {
            req.status = LocalSkillRequestStatus::Timeout;
            req.error = if was_running {
                "daemon did not finish within 60 seconds".into()
            } else {
                "daemon did not respond within 3 minutes".into()
            };
            req.updated_at = Utc::now();
            self.persist_request(&req).await?;
            zrem(
                &mut self.conn.clone(),
                &local_skill_import_pending_key(&req.runtime_id),
                &req.id,
            )
            .await?;
        }
        Ok(Some(req))
    }

    async fn persist_request(&self, req: &RuntimeLocalSkillImportRequest) -> anyhow::Result<()> {
        let data = marshal_skill_import(req)?;
        let mut conn = self.conn.clone();
        let mut command = redis::cmd("SET");
        command
            .arg(local_skill_import_key(&req.id))
            .arg(data)
            .arg("EX")
            .arg(LOCAL_SKILL_STORE_RETENTION_SECS);
        let (): () =
            bounded_pending_redis("persist local skill import", command.query_async(&mut conn))
                .await
                .map_err(|e| anyhow::anyhow!("persist import request: {e}"))?;
        Ok(())
    }

    /// Go Create(LocalSkillImportRequestInput).
    #[allow(clippy::too_many_arguments)]
    pub async fn create_import(
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
        let req = RuntimeLocalSkillImportRequest {
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
        let data = marshal_skill_import(&req)?;
        let mut conn = self.conn.clone();
        let pending_key = local_skill_import_pending_key(runtime_id);
        let mut pipe = redis::pipe();
        pipe.cmd("SET")
            .arg(local_skill_import_key(&req.id))
            .arg(&data)
            .arg("EX")
            .arg(LOCAL_SKILL_STORE_RETENTION_SECS)
            .ignore()
            .cmd("ZADD")
            .arg(&pending_key)
            .arg(now.timestamp_nanos_opt().unwrap_or_default())
            .arg(&req.id)
            .ignore()
            .cmd("EXPIRE")
            .arg(&pending_key)
            .arg(LOCAL_SKILL_STORE_RETENTION_SECS * 2)
            .ignore();
        let (): () =
            bounded_pending_redis("persist local skill import", pipe.query_async(&mut conn))
                .await
                .map_err(|e| anyhow::anyhow!("persist import request: {e}"))?;
        Ok(req)
    }

    pub async fn get(&self, id: &str) -> anyhow::Result<Option<RuntimeLocalSkillImportRequest>> {
        self.load_request(id).await
    }

    pub async fn has_pending(&self, runtime_id: &str) -> anyhow::Result<bool> {
        Ok(zcard(
            &mut self.conn.clone(),
            &local_skill_import_pending_key(runtime_id),
        )
        .await?
            > 0)
    }

    pub async fn pop_pending(
        &self,
        runtime_id: &str,
    ) -> anyhow::Result<Option<RuntimeLocalSkillImportRequest>> {
        let pending_key = local_skill_import_pending_key(runtime_id);
        for _ in 0..POP_MAX_RETRIES {
            let mut conn = self.conn.clone();
            let mut range = redis::cmd("ZRANGE");
            range.arg(&pending_key).arg(0).arg(0);
            let ids: Vec<String> = bounded_pending_redis(
                "list pending local skill imports",
                range.query_async(&mut conn),
            )
            .await
            .map_err(|e| anyhow::anyhow!("zrange pending: {e}"))?;
            let Some(id) = ids.into_iter().next() else {
                return Ok(None);
            };
            let Some(mut req) = load_local_skill_import(&mut self.conn.clone(), &id).await? else {
                // Record expired but the zset still references it — drop and retry.
                zrem(&mut self.conn.clone(), &pending_key, &id).await?;
                continue;
            };
            if req.status != LocalSkillRequestStatus::Pending {
                // Timeout fired inside load_request or another node picked it up.
                zrem(&mut self.conn.clone(), &pending_key, &id).await?;
                continue;
            }
            let now = Utc::now();
            req.status = LocalSkillRequestStatus::Running;
            req.run_started_at = Some(now);
            req.updated_at = now;
            let data = marshal_skill_import(&req)?;
            let won = run_claim_script(
                &mut self.conn.clone(),
                &pending_key,
                &local_skill_import_key(&id),
                &id,
                &data,
                LOCAL_SKILL_STORE_RETENTION_SECS,
            )
            .await
            .map_err(|e| anyhow::anyhow!("claim pending: {e}"))?;
            if won {
                return Ok(Some(req));
            }
        }
        Ok(None)
    }

    /// Go PopPendingBatch: claim up to `limit` candidates atomically; partial
    /// failures keep whatever was claimed because those records are already
    /// running in the store.
    pub async fn pop_pending_batch(
        &self,
        runtime_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<RuntimeLocalSkillImportRequest>> {
        let pending_key = local_skill_import_pending_key(runtime_id);
        let mut conn = self.conn.clone();
        let mut range = redis::cmd("ZRANGE");
        range
            .arg(&pending_key)
            .arg(0)
            .arg(limit.saturating_sub(1) as i64);
        let ids: Vec<String> = bounded_pending_redis(
            "list pending local skill import batch",
            range.query_async(&mut conn),
        )
        .await
        .map_err(|e| anyhow::anyhow!("zrange pending batch: {e}"))?;
        let mut out = Vec::new();
        for id in ids {
            let Some(mut req) = load_local_skill_import(&mut self.conn.clone(), &id).await? else {
                zrem(&mut self.conn.clone(), &pending_key, &id).await?;
                continue;
            };
            if req.status != LocalSkillRequestStatus::Pending {
                zrem(&mut self.conn.clone(), &pending_key, &id).await?;
                continue;
            }
            let now = Utc::now();
            req.status = LocalSkillRequestStatus::Running;
            req.run_started_at = Some(now);
            req.updated_at = now;
            let data = marshal_skill_import(&req)?;
            let claimed = run_claim_script(
                &mut self.conn.clone(),
                &pending_key,
                &local_skill_import_key(&id),
                &id,
                &data,
                LOCAL_SKILL_STORE_RETENTION_SECS,
            )
            .await
            .map_err(|e| anyhow::anyhow!("claim pending batch: {e}"))?;
            if claimed {
                out.push(req);
            }
        }
        Ok(out)
    }

    pub async fn complete(&self, id: &str, skill: Value) -> anyhow::Result<()> {
        let Some(mut req) = self.load_request(id).await? else {
            return Ok(());
        };
        req.status = LocalSkillRequestStatus::Completed;
        req.skill = Some(skill);
        req.updated_at = Utc::now();
        self.persist_request(&req).await
    }

    pub async fn conflict(&self, id: &str, info: LocalSkillImportConflict) -> anyhow::Result<()> {
        let Some(mut req) = self.load_request(id).await? else {
            return Ok(());
        };
        req.status = LocalSkillRequestStatus::Conflict;
        req.conflict = Some(info);
        req.updated_at = Utc::now();
        self.persist_request(&req).await
    }

    pub async fn fail(&self, id: &str, err_msg: &str) -> anyhow::Result<()> {
        let Some(mut req) = self.load_request(id).await? else {
            return Ok(());
        };
        req.status = LocalSkillRequestStatus::Failed;
        req.error = err_msg.to_string();
        req.updated_at = Utc::now();
        self.persist_request(&req).await
    }
}

async fn load_local_skill_import(
    conn: &mut RecoveringConnection,
    id: &str,
) -> anyhow::Result<Option<RuntimeLocalSkillImportRequest>> {
    match get_bytes(conn, &local_skill_import_key(id)).await? {
        Some(raw) => Ok(Some(unmarshal_skill_import(&raw)?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn model_catalog_redis_operation_has_a_hard_deadline() {
        let error = bounded_model_catalog_redis(
            Duration::from_millis(1),
            std::future::pending::<redis::RedisResult<()>>(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "model catalog Redis operation timed out");
    }

    #[tokio::test]
    async fn in_memory_stores_preserve_pending_request_lifecycles() {
        let updates = InMemoryUpdateStore::new();
        let update = updates.create("runtime-1", "v2", "user-1").await.unwrap();
        assert!(updates.has_pending("runtime-1").await.unwrap());
        assert_eq!(
            updates
                .pop_pending("runtime-1")
                .await
                .unwrap()
                .unwrap()
                .status,
            UpdateStatus::Running
        );
        updates.complete(&update.id, "updated").await.unwrap();
        let update = updates.get(&update.id).await.unwrap().unwrap();
        assert_eq!(update.status, UpdateStatus::Completed);
        assert_eq!(update.output, "updated");

        let models = InMemoryModelListStore::new();
        let model_request = models.create("runtime-1").await.unwrap();
        assert_eq!(
            models
                .pop_pending("runtime-1")
                .await
                .unwrap()
                .unwrap()
                .status,
            ModelListStatus::Running
        );
        let entries = vec![ModelEntry {
            id: "model-1".into(),
            label: "Model 1".into(),
            ..Default::default()
        }];
        models
            .complete(&model_request.id, &entries, true, &[])
            .await
            .unwrap();
        let model_request = models.get(&model_request.id).await.unwrap().unwrap();
        assert_eq!(model_request.status, ModelListStatus::Completed);
        assert_eq!(model_request.models.len(), 1);
        assert!(model_request.session_modes.is_empty());

        let session_modes = vec![SessionModeEntry {
            value: "auto".into(),
            label: "Approve for me".into(),
            kind: "auto_review".into(),
        }];
        models
            .complete(&model_request.id, &entries, true, &session_modes)
            .await
            .unwrap();
        let model_request = models.get(&model_request.id).await.unwrap().unwrap();
        assert_eq!(model_request.session_modes.len(), 1);
        assert_eq!(model_request.session_modes[0].value, "auto");

        let catalog = InMemoryModelCatalogCache::new();
        catalog.put("runtime-1", &entries, true, &[]).await.unwrap();
        assert_eq!(
            catalog
                .get("runtime-1")
                .await
                .unwrap()
                .unwrap()
                .models
                .len(),
            1
        );

        let skill_lists = InMemoryLocalSkillListStore::new();
        let list_request = skill_lists.create("runtime-1").await.unwrap();
        assert_eq!(
            skill_lists
                .pop_pending("runtime-1")
                .await
                .unwrap()
                .unwrap()
                .status,
            LocalSkillRequestStatus::Running
        );
        skill_lists
            .complete(&list_request.id, &[], true, &[], true)
            .await
            .unwrap();
        assert_eq!(
            skill_lists
                .get(&list_request.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            LocalSkillRequestStatus::Completed
        );

        let imports = InMemoryLocalSkillImportStore::new();
        let import_request = imports
            .create_import(
                "runtime-1",
                "user-1",
                "review",
                Some("Review".into()),
                None,
                "create",
                "",
                true,
            )
            .await
            .unwrap();
        assert_eq!(
            imports
                .pop_pending("runtime-1")
                .await
                .unwrap()
                .unwrap()
                .status,
            LocalSkillRequestStatus::Running
        );
        imports
            .complete(&import_request.id, serde_json::json!({ "id": "skill-1" }))
            .await
            .unwrap();
        let import_request = imports.get(&import_request.id).await.unwrap().unwrap();
        assert_eq!(import_request.status, LocalSkillRequestStatus::Completed);
        assert_eq!(import_request.skill.unwrap()["id"], "skill-1");
    }

    #[tokio::test]
    async fn pending_redis_operation_has_a_hard_deadline() {
        let error = bounded_pending_redis_with_timeout(
            Duration::from_millis(1),
            "update",
            std::future::pending::<redis::RedisResult<()>>(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "runtime pending Redis update timed out");
    }

    #[test]
    fn local_skill_list_uses_the_direct_go_wire_shape() {
        let request = RuntimeLocalSkillListRequest {
            id: "request-1".into(),
            runtime_id: "runtime-1".into(),
            status: LocalSkillRequestStatus::Pending,
            supported: true,
            ..Default::default()
        };
        let encoded = marshal_skill_list(&request).unwrap();
        let value: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["id"], "request-1");
        assert_eq!(value["runtime_id"], "runtime-1");
        assert!(value.get("r").is_none());
        assert!(value.get("s").is_none());
    }

    #[test]
    fn local_skill_summary_preserves_extended_runtime_fields() {
        let summary: RuntimeLocalSkillSummary = serde_json::from_value(serde_json::json!({
            "key": "review",
            "name": "Review",
            "source_path": "/skills/review",
            "provider": "codex",
            "plugin": "quality",
            "can_disable": true,
            "file_count": 3
        }))
        .unwrap();
        assert_eq!(summary.plugin, "quality");
        assert!(summary.can_disable);
        assert_eq!(summary.file_count, 3);
    }

    #[test]
    fn model_catalog_cache_action_preserves_last_known_good_semantics() {
        let models = vec![ModelEntry {
            id: "model-1".into(),
            label: "Model 1".into(),
            ..Default::default()
        }];
        assert_eq!(
            model_catalog_cache_action(&models, true, false),
            ModelCatalogCacheAction::Store
        );
        assert_eq!(
            model_catalog_cache_action(&[], true, false),
            ModelCatalogCacheAction::Drop
        );
        assert_eq!(
            model_catalog_cache_action(&models, false, false),
            ModelCatalogCacheAction::Drop
        );
        assert_eq!(
            model_catalog_cache_action(&models, true, true),
            ModelCatalogCacheAction::Keep
        );
    }

    #[test]
    fn cached_model_list_wire_marks_only_cache_hits() {
        let stored_at = Utc::now();
        let cached = ModelListRequest {
            id: "synthetic".into(),
            runtime_id: "runtime-1".into(),
            status: ModelListStatus::Completed,
            supported: true,
            created_at: stored_at,
            updated_at: stored_at,
            cached: true,
            cached_at: Some(stored_at),
            ..Default::default()
        };
        let cached_json = serde_json::to_value(&cached).unwrap();
        assert_eq!(cached_json["cached"], true);
        assert_eq!(cached_json["cached_at"], serde_json::json!(stored_at));

        let live = ModelListRequest {
            id: "live".into(),
            runtime_id: "runtime-1".into(),
            status: ModelListStatus::Pending,
            created_at: stored_at,
            updated_at: stored_at,
            ..Default::default()
        };
        let live_json = serde_json::to_value(&live).unwrap();
        assert!(live_json.get("cached").is_none());
        assert!(live_json.get("cached_at").is_none());
    }
}
