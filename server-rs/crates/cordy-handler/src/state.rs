//! Shared handler state — the Rust analogue of the Go `Handler` struct's
//! DB/redis wiring. Domain services are added per-slice as routes land.

use std::sync::Arc;

use cordy_auth::pat_cache::PatCache;
use cordy_realtime::hub::Hub;
use cordy_service::issue_service::IssueService;
use cordy_service::plugin::PluginService;
use cordy_service::plugin_token::CallbackTokens;
use cordy_service::task_service::TaskService;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachmentDownloadMode {
    Auto,
    CloudFront,
    Presign,
    Proxy,
}

#[derive(Clone)]
pub struct AttachmentDownloadSettings {
    pub mode: AttachmentDownloadMode,
    pub public_url: String,
    pub ttl: std::time::Duration,
    pub cloudfront_signer: Option<Arc<crate::cloudfront::CloudFrontSigner>>,
}

impl Default for AttachmentDownloadSettings {
    fn default() -> Self {
        Self {
            mode: AttachmentDownloadMode::Auto,
            public_url: String::new(),
            ttl: std::time::Duration::from_secs(30 * 60),
            cloudfront_signer: None,
        }
    }
}

impl AttachmentDownloadSettings {
    pub async fn from_config(config: &cordy_config::Config) -> anyhow::Result<Self> {
        let mode = match config
            .storage
            .attachment_download_mode
            .as_deref()
            .unwrap_or("auto")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "" | "auto" => AttachmentDownloadMode::Auto,
            "cloudfront" => AttachmentDownloadMode::CloudFront,
            "presign" => AttachmentDownloadMode::Presign,
            "proxy" => AttachmentDownloadMode::Proxy,
            _ => anyhow::bail!(
                "ATTACHMENT_DOWNLOAD_MODE must be auto, cloudfront, presign, or proxy"
            ),
        };
        let ttl = config
            .storage
            .attachment_download_url_ttl
            .as_deref()
            .map(parse_attachment_ttl)
            .transpose()?
            .unwrap_or_else(|| std::time::Duration::from_secs(30 * 60));
        let cloudfront_signer = crate::cloudfront::CloudFrontSigner::from_config(config)
            .await?
            .map(Arc::new);
        anyhow::ensure!(
            mode != AttachmentDownloadMode::CloudFront || cloudfront_signer.is_some(),
            "ATTACHMENT_DOWNLOAD_MODE=cloudfront requires a CloudFront signing key"
        );
        Ok(Self {
            mode,
            public_url: config
                .urls
                .public_url
                .as_deref()
                .unwrap_or_default()
                .trim()
                .trim_end_matches('/')
                .to_string(),
            ttl,
            cloudfront_signer,
        })
    }
}

fn parse_attachment_ttl(raw: &str) -> anyhow::Result<std::time::Duration> {
    let raw = raw.trim();
    anyhow::ensure!(
        !raw.is_empty(),
        "ATTACHMENT_DOWNLOAD_URL_TTL cannot be empty"
    );
    let split = raw
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(raw.len());
    let amount = raw[..split].parse::<u64>()?;
    anyhow::ensure!(amount > 0, "ATTACHMENT_DOWNLOAD_URL_TTL must be positive");
    let multiplier = match &raw[split..] {
        "" | "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        _ => anyhow::bail!("ATTACHMENT_DOWNLOAD_URL_TTL must use s, m, h, or d"),
    };
    Ok(std::time::Duration::from_secs(
        amount
            .checked_mul(multiplier)
            .ok_or_else(|| anyhow::anyhow!("ATTACHMENT_DOWNLOAD_URL_TTL is too large"))?,
    ))
}

struct DaemonTaskWakeup {
    hub: Arc<cordy_daemon::hub::DaemonHub>,
}

impl cordy_service::task_service::TaskWakeupNotifier for DaemonTaskWakeup {
    fn notify_task_available(&self, runtime_id: &str, task_id: &str) {
        self.hub.notify_task_available(runtime_id, task_id);
    }
}

/// Handler-layer state shared by all axum extractors.
#[derive(Clone)]
pub struct HandlerState {
    pub pool: sqlx::PgPool,
    pub pat_cache: PatCache,
    /// Realtime WS hub (cordy-realtime). `None` only in tests.
    pub hub: Option<Arc<Hub>>,
    /// Event bus (Go h.Bus) for workspace-scoped WS fanout.
    pub bus: Arc<cordy_events::Bus>,
    /// Feature flag source. `None` fails closed for rollout-gated writes.
    pub feature_flags: Option<Arc<dyn cordy_service::feature_flags::FlagSource>>,
    /// Task domain service (Go h.TaskService).
    pub tasks: Arc<TaskService>,
    /// Issue domain service (Go h.IssueService).
    pub issues: Arc<IssueService>,
    /// Plugin service (Go h.PluginService).
    pub plugins: Arc<PluginService>,
    /// Hook callback token store; None disables callback tokens (fail-closed).
    pub callbacks: Option<Arc<CallbackTokens>>,
    /// Absolute base URL used in hook callback_url; empty omits the field.
    pub callback_base_url: String,
    /// Redis-backed pending request stores (update / model list / local
    /// skills). `None` matches Go's nil-store path: every probe reports an
    /// empty queue and report endpoints answer 404, which daemons treat as a
    /// dropped one-shot report.
    pub update_store: Option<Arc<crate::pending_store::UpdateStore>>,
    pub model_list_store: Option<Arc<crate::pending_store::ModelListStore>>,
    pub local_skill_list_store: Option<Arc<crate::pending_store::LocalSkillListStore>>,
    pub local_skill_import_store: Option<Arc<crate::pending_store::LocalSkillImportStore>>,
    /// Daemon WebSocket hub (cordy-daemon). `None` only in tests — the WS
    /// endpoint reports 503 and daemons fall back to HTTP polling.
    pub daemon_hub: Option<Arc<cordy_daemon::hub::DaemonHub>>,
    /// Shared attachment/object storage and its one download URL policy.
    /// #70 extends this seam; it must not create a second policy or signer.
    pub attachment_storage: Option<Arc<dyn crate::attachment_storage::AttachmentStorage>>,
    pub attachment_download: AttachmentDownloadSettings,
    /// Keeps the weak notifier installed in `TaskService` alive.
    _task_wakeup: Arc<dyn cordy_service::task_service::TaskWakeupNotifier>,
}

impl HandlerState {
    pub fn new(pool: sqlx::PgPool, pat_cache: PatCache, hub: Option<Arc<Hub>>) -> Self {
        let bus = Arc::new(cordy_events::Bus::new());
        let daemon_hub = Arc::new(cordy_daemon::hub::DaemonHub::new());
        let task_wakeup: Arc<dyn cordy_service::task_service::TaskWakeupNotifier> =
            Arc::new(DaemonTaskWakeup {
                hub: daemon_hub.clone(),
            });
        let mut task_service = TaskService::new(pool.clone(), bus.clone());
        task_service.wakeup = Some(Arc::downgrade(&task_wakeup));
        let tasks = Arc::new(task_service);
        let issues = Arc::new(IssueService::new(pool.clone(), bus.clone(), tasks.clone()));
        let plugins = Arc::new(PluginService::with_pool(pool.clone()));
        Self {
            pool,
            pat_cache,
            hub,
            bus,
            feature_flags: None,
            tasks,
            issues,
            plugins,
            callbacks: Some(Arc::new(CallbackTokens::new())),
            callback_base_url: String::new(),
            update_store: None,
            model_list_store: None,
            local_skill_list_store: None,
            local_skill_import_store: None,
            daemon_hub: Some(daemon_hub),
            attachment_storage: None,
            attachment_download: AttachmentDownloadSettings::default(),
            _task_wakeup: task_wakeup,
        }
    }

    pub fn with_attachment_storage(
        mut self,
        storage: Arc<dyn crate::attachment_storage::AttachmentStorage>,
        download: AttachmentDownloadSettings,
    ) -> Self {
        self.attachment_storage = Some(storage);
        self.attachment_download = download;
        self
    }

    /// Builds the pending-request stores from a Redis client (Go
    /// NewRedis{Update,ModelList,LocalSkill*}Store wiring). Callers without
    /// Redis keep `None` fields — the disabled path degrades exactly like Go's
    /// nil-store behavior.
    pub async fn with_redis(mut self, client: redis::Client) -> Result<Self, redis::RedisError> {
        let conn = client.get_connection_manager().await?;
        self.update_store = Some(Arc::new(crate::pending_store::UpdateStore::new(
            conn.clone(),
        )));
        self.model_list_store = Some(Arc::new(crate::pending_store::ModelListStore::new(
            conn.clone(),
        )));
        self.local_skill_list_store = Some(Arc::new(
            crate::pending_store::LocalSkillListStore::new(conn.clone()),
        ));
        self.local_skill_import_store = Some(Arc::new(
            crate::pending_store::LocalSkillImportStore::new(conn),
        ));
        Ok(self)
    }
}
