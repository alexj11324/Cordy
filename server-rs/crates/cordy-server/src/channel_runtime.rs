//! Production lifecycle wiring for chat-channel adapters.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use cordy_channel_engine::lease::{AcquireLeaseParams, LeaseError, LeaseStore, ReleaseLeaseParams};
use cordy_channel_engine::postgres_store::PostgresChannelStore;
use cordy_channel_engine::resolvers::{
    IssueCreator, RouterIssueCreateParams, RouterIssueOutcome, SessionReader, TaskEnqueuer,
};
use cordy_channel_engine::router::{Router as ChannelRouter, RouterConfig};
use cordy_channel_engine::supervisor::{Supervisor, SupervisorConfig};

type ChannelSupervisor = Supervisor<PostgresChannelStore, RuntimeLeaseStore>;

pub struct ChannelRuntime {
    cancel: CancellationToken,
    supervisor: Option<tokio::task::JoinHandle<()>>,
    media_reconciler: Option<tokio::task::JoinHandle<()>>,
    router: Arc<ChannelRouter>,
}

impl ChannelRuntime {
    pub async fn start(
        state: &cordy_handler::HandlerState,
        cfg: &cordy_config::Config,
    ) -> anyhow::Result<Option<Self>> {
        let services = Arc::new(ChannelServices {
            pool: state.pool.clone(),
            issues: state.issues.clone(),
            tasks: state.tasks.clone(),
        });
        let router = ChannelRouter::new(
            services.clone(),
            services.clone(),
            services.clone(),
            RouterConfig::default(),
        );
        router.enable_run_batching(cordy_channel_engine::DEFAULT_CHAT_RUN_BATCH_WINDOW);
        let storage = state
            .attachment_storage
            .clone()
            .map(|inner| Arc::new(ChannelStorage { inner }));
        let registry = Arc::new(cordy_channel::Registry::new());
        let cancel = CancellationToken::new();

        configure_slack(state, cfg, &services, &router, storage.as_ref(), &registry);
        configure_dingtalk(state, cfg, &router, storage.as_ref(), &registry);
        configure_telegram(state, cfg, &router, &registry, &cancel);
        configure_wecom(state, cfg, &router, storage.as_ref(), &registry);
        configure_lark(state, cfg, &router, storage.as_ref(), &registry)?;

        let channel_types = registry.types();
        if channel_types.is_empty() {
            tracing::info!("channel runtime disabled: no adapter secret keys configured");
            return Ok(None);
        }

        let store = Arc::new(PostgresChannelStore::new(state.pool.clone()));
        let lease_store = match RuntimeLeaseStore::from_env(store.clone(), cfg).await {
            Ok(store) => Some(Arc::new(store)),
            Err(error) => {
                tracing::error!(%error, "channel supervisor disabled: lease backend unavailable");
                None
            }
        };
        let supervisor = if let Some(lease_store) = lease_store {
            let inbound_router = router.clone();
            let handler = cordy_channel::InboundHandler::new(move |ctx, message| {
                let router = inbound_router.clone();
                Box::pin(async move {
                    tokio::select! {
                        result = router.handle(message) => result,
                        _ = ctx.cancelled() => Ok(()),
                    }
                })
            });
            let supervisor = ChannelSupervisor::new(
                store,
                lease_store,
                registry,
                handler,
                supervisor_config_from_env(),
                None,
            )?;
            let run_cancel = cancel.clone();
            Some(tokio::spawn(supervisor.run_owned(run_cancel)))
        } else {
            None
        };

        let media_reconciler = storage.map(|storage| {
            let reconciler = cordy_service::channel_media_reconciler::ChannelMediaReconciler {
                pool: state.pool.clone(),
                storage: Some(storage),
                metrics: None,
                delete_timeout: None,
            };
            let run_cancel = cancel.clone();
            tokio::spawn(async move { reconciler.run(run_cancel).await })
        });

        tracing::info!(
            supervisor = supervisor.is_some(),
            media = media_reconciler.is_some(),
            channels = ?channel_types,
            "channel runtime started"
        );
        Ok(Some(Self {
            cancel,
            supervisor,
            media_reconciler,
            router,
        }))
    }

    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(handle) = self.supervisor.take() {
            let _ = handle.await;
        }
        if let Some(handle) = self.media_reconciler.take() {
            let _ = handle.await;
        }
        let drain = CancellationToken::new();
        let deadline = drain.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;
            deadline.cancel();
        });
        if !self.router.drain(&drain).await {
            tracing::warn!("channel router drain deadline reached");
        }
    }
}

fn configure_slack(
    state: &cordy_handler::HandlerState,
    cfg: &cordy_config::Config,
    services: &Arc<ChannelServices>,
    router: &Arc<ChannelRouter>,
    storage: Option<&Arc<ChannelStorage>>,
    registry: &Arc<cordy_channel::Registry>,
) {
    let secret_box = match channel_secret_box("CORDY_SLACK_SECRET_KEY") {
        Ok(Some(secret_box)) => secret_box,
        Ok(None) => {
            tracing::info!("slack channel runtime disabled: CORDY_SLACK_SECRET_KEY not set");
            return;
        }
        Err(error) => {
            tracing::error!(%error, "slack channel runtime disabled: invalid secret key");
            return;
        }
    };
    let decrypt: Arc<cordy_slack::config::Decrypter> =
        Arc::new(move |sealed| secret_box.open(sealed).map_err(anyhow::Error::from));

    let typing = Arc::new(cordy_slack::typing_indicator::TypingIndicatorManager::new(
        state.pool.clone(),
    ));
    // Registration order is observable: clear the processing reaction
    // before the terminal outbound subscriber posts the reply.
    typing.register(&state.bus, Some(decrypt.clone()));

    let replier = Arc::new(cordy_slack::replier::OutboundReplier::new(
        cordy_slack::replier::OutboundReplierConfig {
            pool: state.pool.clone(),
            decrypt: Some(decrypt.clone()),
            app_url: app_url(cfg),
            binding_path: String::new(),
        },
    ));
    let media = storage.map(|storage| {
        Arc::new(cordy_slack::media_ingest::SlackMediaResolver::new(
            Some(decrypt.clone()),
            storage.clone(),
            Arc::new(cordy_channel_engine::resolvers::DbMediaIntentLedger::new(
                state.pool.clone(),
            )),
        )) as Arc<dyn cordy_channel_engine::resolvers::MediaResolver>
    });
    router.register(
        cordy_channel::Type(cordy_slack::TYPE_SLACK.to_string()),
        cordy_slack::resolvers::new_slack_resolver_set(
            state.pool.clone(),
            Some(decrypt.clone()),
            Some(typing),
            media,
            Some(replier),
        ),
    );

    Arc::new(cordy_slack::outbound::Outbound::new(
        state.pool.clone(),
        Some(decrypt.clone()),
    ))
    .register(&state.bus);

    let binding = cordy_slack::binding::BindingTokenService::new(state.pool.clone());
    let slash = Arc::new(cordy_slack::slash_command::SlashCommandProcessor::new(
        cordy_slack::slash_command::SlashCommandConfig {
            pool: state.pool.clone(),
            tasks: services.clone(),
            binding: Some(binding),
            app_url: app_url(cfg),
            binding_path: String::new(),
            respond: None,
        },
    ));
    cordy_slack::channel::register_slack(
        registry,
        cordy_slack::channel::ChannelDeps {
            decrypt: Some(decrypt),
            slash: Some(slash),
        },
    );
}

fn configure_dingtalk(
    state: &cordy_handler::HandlerState,
    cfg: &cordy_config::Config,
    router: &Arc<ChannelRouter>,
    storage: Option<&Arc<ChannelStorage>>,
    registry: &Arc<cordy_channel::Registry>,
) {
    let secret_box = match channel_secret_box("CORDY_DINGTALK_SECRET_KEY") {
        Ok(Some(secret_box)) => secret_box,
        Ok(None) => {
            tracing::info!("dingtalk channel runtime disabled: CORDY_DINGTALK_SECRET_KEY not set");
            return;
        }
        Err(error) => {
            tracing::error!(%error, "dingtalk channel runtime disabled: invalid secret key");
            return;
        }
    };
    let decrypt: Arc<cordy_dingtalk::config::Decrypter> =
        Arc::new(move |sealed| secret_box.open(sealed).map_err(anyhow::Error::from));
    let client = Arc::new(cordy_dingtalk::client::Client::new(None, ""));
    let binding = Arc::new(cordy_dingtalk::binding::BindingTokenService::new(
        state.pool.clone(),
    ));
    let replier = Arc::new(cordy_dingtalk::replier::OutboundReplier::new(
        cordy_dingtalk::replier::OutboundReplierConfig {
            binding: Some(binding),
            decrypt: Some(decrypt.clone()),
            client: Some(client.clone()),
            app_url: app_url(cfg),
            binding_path: String::new(),
        },
    ));
    let ack = Arc::new(cordy_dingtalk::ack::AckNotifier::new(
        client.clone(),
        Some(decrypt.clone()),
    ));
    let media = storage.map(|storage| {
        Arc::new(cordy_dingtalk::media::MediaResolverImpl::new(
            client.clone(),
            Some(decrypt.clone()),
            storage.clone(),
            Arc::new(cordy_channel_engine::resolvers::DbMediaIntentLedger::new(
                state.pool.clone(),
            )),
        )) as Arc<dyn cordy_channel_engine::resolvers::MediaResolver>
    });
    router.register(
        cordy_dingtalk::channel_type(),
        cordy_dingtalk::resolvers::new_dingtalk_resolver_set(
            state.pool.clone(),
            Some(replier),
            Some(ack),
            media,
        ),
    );

    Arc::new(cordy_dingtalk::outbound::Outbound::new(
        state.pool.clone(),
        Some(decrypt.clone()),
        client.clone(),
    ))
    .register(&state.bus);
    cordy_dingtalk::dingtalk_channel::register_dingtalk(
        registry,
        cordy_dingtalk::dingtalk_channel::ChannelDeps {
            decrypt: Some(decrypt),
            client: Some(client),
        },
    );
}

fn configure_telegram(
    state: &cordy_handler::HandlerState,
    cfg: &cordy_config::Config,
    router: &Arc<ChannelRouter>,
    registry: &Arc<cordy_channel::Registry>,
    cancel: &CancellationToken,
) {
    let secret_box = match channel_secret_box("CORDY_TELEGRAM_SECRET_KEY") {
        Ok(Some(secret_box)) => secret_box,
        Ok(None) => {
            tracing::info!("telegram channel runtime disabled: CORDY_TELEGRAM_SECRET_KEY not set");
            return;
        }
        Err(error) => {
            tracing::error!(%error, "telegram channel runtime disabled: invalid secret key");
            return;
        }
    };
    let decrypt: Arc<cordy_telegram::DecrypterFn> =
        Arc::new(move |sealed| secret_box.open(sealed).map_err(anyhow::Error::from));
    let binding = Arc::new(cordy_telegram::replier::DbBindingMinter::new(
        state.pool.clone(),
    ));
    let replier = Arc::new(cordy_telegram::replier::OutboundReplier::new(
        cordy_telegram::replier::OutboundReplierConfig {
            binding: Some(binding),
            decrypt: Some(decrypt.clone()),
            app_url: app_url(cfg),
            binding_path: String::new(),
            api_base: String::new(),
        },
    ));
    let typing = Arc::new(cordy_telegram::replier::TypingIndicator::new(
        Some(decrypt.clone()),
        String::new(),
    ));
    router.register(
        cordy_channel::Type(cordy_telegram::TYPE_TELEGRAM.to_string()),
        cordy_telegram::resolvers::new_telegram_resolver_set(
            state.pool.clone(),
            Some(replier),
            Some(typing),
        ),
    );

    Arc::new(cordy_telegram::outbound::Outbound::new(
        state.pool.clone(),
        Some(decrypt.clone()),
        String::new(),
        cancel.clone(),
    ))
    .register(&state.bus);
    cordy_telegram::channel::register_telegram(
        registry,
        cordy_telegram::channel::ChannelDeps {
            decrypt: Some(decrypt),
            api_base: String::new(),
        },
    );
}

fn configure_wecom(
    state: &cordy_handler::HandlerState,
    cfg: &cordy_config::Config,
    router: &Arc<ChannelRouter>,
    storage: Option<&Arc<ChannelStorage>>,
    registry: &Arc<cordy_channel::Registry>,
) {
    let secret_box = match channel_secret_box("CORDY_WECOM_SECRET_KEY") {
        Ok(Some(secret_box)) => secret_box,
        Ok(None) => {
            tracing::info!("wecom channel runtime disabled: CORDY_WECOM_SECRET_KEY not set");
            return;
        }
        Err(error) => {
            tracing::error!(%error, "wecom channel runtime disabled: invalid secret key");
            return;
        }
    };
    let senders = Arc::new(cordy_wecom::senders_registry::SendersRegistry::new());
    let binding = Arc::new(cordy_wecom::replier::DbBindingMinter::new(
        state.pool.clone(),
    ));
    let replier = Arc::new(cordy_wecom::replier::OutboundReplier::new(
        cordy_wecom::replier::OutboundReplierConfig {
            binding: Some(binding),
            senders: senders.clone(),
            app_url: app_url(cfg),
            binding_path: String::new(),
        },
    ));
    let media = storage.and_then(|storage| {
        match cordy_wecom::media_ingest::WecomMediaResolver::new(
            storage.clone(),
            Arc::new(cordy_channel_engine::resolvers::DbMediaIntentLedger::new(
                state.pool.clone(),
            )),
            Some(senders.clone()),
        ) {
            Ok(media) => Some(Arc::new(media)
                as Arc<dyn cordy_channel_engine::resolvers::MediaResolver>),
            Err(error) => {
                tracing::error!(%error, "wecom inbound media disabled: guarded HTTP client unavailable");
                None
            }
        }
    });
    router.register(
        cordy_wecom::type_wecom(),
        cordy_wecom::resolvers::new_wecom_resolver_set(state.pool.clone(), Some(replier), media),
    );

    let mut outbound = cordy_wecom::outbound_media::Outbound::new(state.pool.clone())
        .with_senders(senders.clone())
        .with_app_url(
            std::env::var("WECOM_APP_URL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| app_url(cfg)),
        );
    if let Some(storage) = storage {
        outbound = outbound.with_attachments(storage.clone());
    }
    Arc::new(outbound).register(&state.bus);
    cordy_wecom::wecom_channel::register_wecom(
        registry,
        cordy_wecom::wecom_channel::ChannelDeps {
            credentials: Some(Arc::new(
                cordy_wecom::credentials::SecretboxCredentialsResolver::new(secret_box),
            )),
            senders: Some(senders),
            metrics: None,
            dialer: None,
            ws_url: String::new(),
        },
    );
    if cfg
        .redis
        .url
        .as_deref()
        .is_some_and(|url| !url.trim().is_empty())
    {
        tracing::warn!(
            "wecom outbound uses the lease-holder's in-process WebSocket; run a single server replica until cross-replica outbound routing is configured"
        );
    }
}

fn configure_lark(
    state: &cordy_handler::HandlerState,
    cfg: &cordy_config::Config,
    router: &Arc<ChannelRouter>,
    storage: Option<&Arc<ChannelStorage>>,
    registry: &Arc<cordy_channel::Registry>,
) -> anyhow::Result<()> {
    let secret_box = match channel_secret_box("CORDY_LARK_SECRET_KEY") {
        Ok(Some(secret_box)) => secret_box,
        Ok(None) => {
            tracing::info!("lark channel runtime disabled: CORDY_LARK_SECRET_KEY not set");
            return Ok(());
        }
        Err(error) => return Err(error.context("lark channel runtime: invalid secret key")),
    };
    let store = Arc::new(cordy_lark::channel_store::ChannelStore::new(
        state.pool.clone(),
    ));
    let installations = Arc::new(cordy_lark::installation::InstallationService::new(
        store.as_ref().clone(),
        Arc::new(secret_box),
    ));
    let api: Arc<dyn cordy_lark::client::ApiClient> = Arc::new(
        cordy_lark::http_client::HttpApiClient::new(cordy_lark::http_client::HttpClientConfig {
            base_url: std::env::var("CORDY_LARK_HTTP_BASE_URL").unwrap_or_default(),
            ..Default::default()
        }),
    );
    let endpoint = Arc::new(cordy_lark::ws_endpoint::HttpConnectionTokenFetcher::new(
        cordy_lark::ws_endpoint::HttpConnectionTokenConfig {
            base_url: std::env::var("CORDY_LARK_CALLBACK_BASE_URL").unwrap_or_default(),
            http_client: None,
        },
    ));
    let enricher = Arc::new(cordy_lark::inbound_enricher::InboundEnricher::new(
        api.clone(),
        cordy_lark::inbound_enricher::InboundEnricherConfig::default(),
    ));
    let connector: Arc<dyn cordy_lark::connector::EventConnector> = Arc::new(
        cordy_lark::ws_connector::WsLongConnConnector::new(
            cordy_lark::ws_connector::WsConnectorConfig {
                dialer: Some(Arc::new(cordy_lark::ws_connector::TungsteniteDialer::new())),
                endpoint_fetcher: Some(endpoint),
                frame_decoder: Some(Arc::new(
                    cordy_lark::frame_decoder::LarkJsonFrameDecoder::new(),
                )),
                enricher: Some(enricher),
                credentials_provider: Some(installations.clone()),
                ..Default::default()
            },
        )
        .context("build lark production WebSocket connector")?,
    );

    let binding = Arc::new(cordy_lark::binding_token::BindingTokenService::new(
        store.as_ref().clone(),
    ));
    let replier = cordy_lark::outcome_replier::new_lark_outcome_replier(
        cordy_lark::outcome_replier::OutcomeReplierConfig {
            api_client: Some(api.clone()),
            binding_svc: Some(binding),
            credentials: Some(installations.clone()),
            queries: Some(store.clone()),
            app_url: app_url(cfg),
            binding_path: String::new(),
        },
    );
    let typing = Arc::new(cordy_lark::typing_indicator::TypingIndicatorManager::new(
        api.clone(),
        installations.clone(),
        store.clone(),
    ));
    let patcher = Arc::new(cordy_lark::outbound::LarkPatcher::new(
        state.pool.clone(),
        Some(installations.clone()),
        api.clone(),
        cordy_lark::outbound::PatcherConfig { renderer: None },
    ));
    patcher.set_typing_indicator_manager(Some(typing.clone()));
    patcher.register(&state.bus);

    let media = storage.map(|storage| {
        Arc::new(cordy_lark::media_ingest::FeishuMediaResolver::new(
            api.clone(),
            installations.clone(),
            storage.clone(),
            Arc::new(cordy_channel_engine::resolvers::DbMediaIntentLedger::new(
                state.pool.clone(),
            )),
        )) as Arc<dyn cordy_channel_engine::resolvers::MediaResolver>
    });
    let session = Arc::new(cordy_channel_engine::session::ChatSession::new(
        state.pool.clone(),
        cordy_channel::Type::feishu(),
        cordy_channel_engine::session::SessionTitles {
            group: "Lark group chat".into(),
            direct: "Lark direct message".into(),
            fallback: "Lark chat".into(),
        },
    ));
    router.register(
        cordy_channel::Type::feishu(),
        cordy_lark::resolvers::new_feishu_resolver_set(
            store,
            session,
            Arc::new(cordy_lark::audit::DbAuditLogger::new(state.pool.clone())),
            Some(replier),
            Some(typing),
            media,
        ),
    );
    cordy_lark::channel::register_feishu(
        registry,
        cordy_lark::channel::FeishuChannelDeps {
            connector,
            api,
            credentials: installations,
        },
    );
    tracing::info!("lark production channel runtime wired");
    Ok(())
}

fn channel_secret_box(env_name: &str) -> anyhow::Result<Option<cordy_util::secretbox::SecretBox>> {
    if std::env::var(env_name)
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        return Ok(None);
    }
    let key = cordy_util::secretbox::load_key(env_name)?;
    Ok(Some(cordy_util::secretbox::SecretBox::new(&key)?))
}

fn app_url(cfg: &cordy_config::Config) -> String {
    cfg.urls
        .app_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or(cfg.urls.frontend_origin.as_deref())
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_string()
}

fn supervisor_config_from_env() -> SupervisorConfig {
    SupervisorConfig {
        lease_ttl: super::duration_env("CHANNEL_WS_LEASE_TTL", Duration::from_secs(180), false),
        lease_renew_interval: super::duration_env(
            "CHANNEL_WS_LEASE_RENEW_INTERVAL",
            Duration::from_secs(60),
            false,
        ),
        poll_interval: super::duration_env(
            "CHANNEL_WS_LEASE_POLL_INTERVAL",
            Duration::from_secs(30),
            false,
        ),
        lease_error_retry_interval: super::duration_env(
            "CHANNEL_WS_LEASE_ERROR_RETRY_INTERVAL",
            Duration::from_secs(5),
            false,
        ),
        lease_expiry_safety_margin: super::duration_env(
            "CHANNEL_WS_LEASE_EXPIRY_SAFETY_MARGIN",
            Duration::from_secs(5),
            false,
        ),
        ..Default::default()
    }
}

enum RuntimeLeaseStore {
    Postgres(Arc<PostgresChannelStore>),
    Redis(Box<cordy_channel_engine::redis_lease_store::RedisLeaseStore>),
}

impl RuntimeLeaseStore {
    async fn from_env(
        postgres: Arc<PostgresChannelStore>,
        cfg: &cordy_config::Config,
    ) -> anyhow::Result<Self> {
        match std::env::var("CHANNEL_WS_LEASE_BACKEND")
            .unwrap_or_else(|_| "postgres".to_string())
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "" | "postgres" => Ok(Self::Postgres(postgres)),
            "redis" => {
                let redis_url = std::env::var("CHANNEL_WS_LEASE_REDIS_URL")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .or_else(|| cfg.redis.url.clone())
                    .ok_or_else(|| anyhow::anyhow!("Redis lease URL is not configured"))?;
                let client = redis::Client::open(redis_url)?;
                let conn = client.get_connection_manager().await?;
                let namespace = std::env::var("CHANNEL_WS_LEASE_NAMESPACE").unwrap_or_default();
                let store = cordy_channel_engine::redis_lease_store::RedisLeaseStore::new(
                    conn,
                    namespace.trim(),
                )
                .map_err(anyhow::Error::from)?;
                tokio::time::timeout(Duration::from_secs(5), store.ready())
                    .await
                    .map_err(|_| anyhow::anyhow!("Redis lease readiness timed out"))??;
                Ok(Self::Redis(Box::new(store)))
            }
            backend => anyhow::bail!("unsupported CHANNEL_WS_LEASE_BACKEND {backend:?}"),
        }
    }
}

#[async_trait]
impl LeaseStore for RuntimeLeaseStore {
    async fn list_held(&self, ids: &[Uuid]) -> Result<HashSet<String>, LeaseError> {
        match self {
            Self::Postgres(store) => store.list_held(ids).await,
            Self::Redis(store) => store.list_held(ids).await,
        }
    }

    async fn try_acquire(&self, arg: AcquireLeaseParams) -> Result<(), LeaseError> {
        match self {
            Self::Postgres(store) => store.try_acquire(arg).await,
            Self::Redis(store) => store.try_acquire(arg).await,
        }
    }

    async fn renew(&self, arg: AcquireLeaseParams) -> Result<(), LeaseError> {
        match self {
            Self::Postgres(store) => store.renew(arg).await,
            Self::Redis(store) => store.renew(arg).await,
        }
    }

    async fn release(&self, arg: ReleaseLeaseParams) -> Result<(), LeaseError> {
        match self {
            Self::Postgres(store) => store.release(arg).await,
            Self::Redis(store) => store.release(arg).await,
        }
    }
}

struct ChannelStorage {
    inner: Arc<dyn cordy_handler::attachment_storage::AttachmentStorage>,
}

impl cordy_slack::media_ingest::MediaStorage for ChannelStorage {
    fn upload(
        &self,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        let key = key.to_string();
        let content_type = content_type.to_string();
        let filename = filename.to_string();
        Box::pin(async move {
            self.inner
                .upload(&key, data, &content_type, &filename)
                .await
                .map(|_| ())
        })
    }

    fn object_url(&self, key: &str) -> String {
        self.inner.object_url(key)
    }
}

impl cordy_dingtalk::media::MediaStorage for ChannelStorage {
    fn upload(
        &self,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        let key = key.to_string();
        let content_type = content_type.to_string();
        let filename = filename.to_string();
        Box::pin(async move {
            self.inner
                .upload(&key, data, &content_type, &filename)
                .await
                .map(|_| ())
        })
    }

    fn object_url(&self, key: &str) -> String {
        self.inner.object_url(key)
    }
}

#[async_trait]
impl cordy_wecom::media_ingest::MediaStorage for ChannelStorage {
    async fn upload(
        &self,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> anyhow::Result<String> {
        self.inner.upload(key, data, content_type, filename).await
    }

    fn object_url(&self, key: &str) -> String {
        self.inner.object_url(key)
    }
}

impl cordy_lark::media_ingest::MediaStorage for ChannelStorage {
    fn upload(
        &self,
        key: &str,
        data: Vec<u8>,
        content_type: &str,
        filename: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + '_>> {
        let key = key.to_string();
        let content_type = content_type.to_string();
        let filename = filename.to_string();
        Box::pin(async move {
            self.inner
                .upload(&key, data, &content_type, &filename)
                .await
                .map(|_| ())
        })
    }

    fn object_url(&self, key: &str) -> String {
        self.inner.object_url(key)
    }
}

#[async_trait]
impl cordy_wecom::outbound_media::MediaObjectStore for ChannelStorage {
    fn key_from_url(&self, raw_url: &str) -> String {
        self.inner.key_from_url(raw_url).unwrap_or_default()
    }

    async fn get_object(&self, key: &str, max_bytes: usize) -> anyhow::Result<Vec<u8>> {
        use http_body_util::BodyExt as _;

        if key.is_empty() {
            anyhow::bail!("attachment URL is not owned by configured storage");
        }
        let range = format!("bytes=0-{max_bytes}");
        let object = self.inner.get(key, Some(&range)).await?;
        let bytes = object.body.collect().await?.to_bytes();
        if bytes.len() > max_bytes {
            anyhow::bail!("attachment exceeds WeCom upload limit");
        }
        Ok(bytes.to_vec())
    }
}

#[async_trait]
impl cordy_service::channel_media_reconciler::MediaObjectDeleter for ChannelStorage {
    async fn delete_object(&self, key: &str) -> anyhow::Result<()> {
        self.inner.delete(key).await
    }
}

struct ChannelServices {
    pool: sqlx::PgPool,
    issues: Arc<cordy_service::issue_service::IssueService>,
    tasks: Arc<cordy_service::task_service::TaskService>,
}

#[async_trait]
impl IssueCreator for ChannelServices {
    async fn create_issue_for_router(
        &self,
        p: RouterIssueCreateParams,
    ) -> anyhow::Result<RouterIssueOutcome> {
        use cordy_service::issue_service::{IssueCreateError, IssueCreateOpts, IssueCreateParams};
        let result = self
            .issues
            .create(
                IssueCreateParams {
                    workspace_id: p.workspace_id,
                    title: p.title,
                    description: (!p.description.is_empty()).then_some(p.description),
                    status: "todo".to_string(),
                    priority: "none".to_string(),
                    assignee_type: Some("agent".to_string()),
                    assignee_id: Some(p.assignee_agent_id),
                    creator_type: "member".to_string(),
                    creator_id: p.creator_user_id,
                    origin_type: (!p.origin_type.is_empty()).then_some(p.origin_type),
                    origin_id: Some(p.origin_session_id),
                    ..Default::default()
                },
                IssueCreateOpts {
                    assigned_agent_run_fire_at: p.assigned_run_fire_at,
                    actor_id: p.creator_user_id.to_string(),
                    platform: "channel".to_string(),
                    ..Default::default()
                },
            )
            .await;
        match result {
            Ok(result) => {
                let issue = result
                    .issue
                    .ok_or_else(|| anyhow::anyhow!("channel issue create returned no issue"))?;
                Ok(RouterIssueOutcome {
                    issue_id: Some(issue.id),
                    issue_number: issue.number,
                    issue_title: issue.title,
                    assigned_task_id: result.assigned_task_id,
                    ..Default::default()
                })
            }
            Err(IssueCreateError::ActiveDuplicate { duplicate }) => Ok(RouterIssueOutcome {
                duplicate_issue_id: duplicate.map(|issue| issue.id),
                ..Default::default()
            }),
            Err(error) => Err(anyhow::Error::new(error)),
        }
    }

    async fn publish_attachments_changed(&self, issue_id: Uuid, actor_id: Uuid) {
        if let Ok(Some(issue)) = cordy_db::queries::issue::get_issue(&self.pool, issue_id).await {
            self.issues
                .publish_attachments_changed(&issue, actor_id)
                .await;
        }
    }
}

#[async_trait]
impl TaskEnqueuer for ChannelServices {
    async fn enqueue_chat_task(
        &self,
        session_id: Uuid,
        initiator_user_id: Uuid,
        force_fresh_session: bool,
    ) -> anyhow::Result<Uuid> {
        let session = cordy_db::queries::chat::get_chat_session(&self.pool, session_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("chat session not found"))?;
        self.tasks
            .enqueue_chat_task(&session, Some(initiator_user_id), force_fresh_session)
            .await
            .map(|task| task.id)
            .map_err(anyhow::Error::from)
    }

    async fn promote_channel_chat_tasks_if_media_ready(
        &self,
        session_id: Uuid,
    ) -> anyhow::Result<()> {
        self.tasks
            .promote_channel_chat_tasks_if_media_ready(session_id)
            .await
            .map_err(anyhow::Error::from)
    }

    async fn promote_deferred_channel_issue_task(&self, task_id: Uuid) -> anyhow::Result<()> {
        self.tasks
            .promote_deferred_channel_issue_task(task_id)
            .await
            .map_err(anyhow::Error::from)
    }
}

#[async_trait]
impl SessionReader for ChannelServices {
    async fn get_chat_session_title(&self, id: Uuid) -> anyhow::Result<String> {
        Ok(cordy_db::queries::chat::get_chat_session(&self.pool, id)
            .await?
            .map(|session| session.title)
            .unwrap_or_default())
    }

    async fn get_workspace_issue_prefix(&self, id: Uuid) -> anyhow::Result<String> {
        Ok(cordy_db::queries::workspace::get_workspace(&self.pool, id)
            .await?
            .map(|workspace| workspace.issue_prefix)
            .unwrap_or_default())
    }
}

#[async_trait]
impl cordy_slack::slash_command::QuickCreateEnqueuer for ChannelServices {
    async fn enqueue_quick_create_task(
        &self,
        req: cordy_slack::slash_command::QuickCreateRequest,
    ) -> anyhow::Result<()> {
        self.tasks
            .enqueue_quick_create_task(
                req.workspace_id,
                req.requester_id,
                req.agent_id,
                req.squad_id,
                &req.prompt,
                &req.priority,
                &req.due_date,
                req.project_id,
                req.parent_issue_id,
                req.attachment_ids,
            )
            .await
            .map(|_| ())
            .map_err(anyhow::Error::from)
    }
}

#[cfg(test)]
mod tests {
    use super::app_url;

    #[test]
    fn app_url_prefers_explicit_app_host_and_trims_slash() {
        let mut cfg = cordy_config::Config::default();
        cfg.urls.frontend_origin = Some("https://frontend.example/".into());
        assert_eq!(app_url(&cfg), "https://frontend.example");
        cfg.urls.app_url = Some("https://app.example///".into());
        assert_eq!(app_url(&cfg), "https://app.example");
    }
}
