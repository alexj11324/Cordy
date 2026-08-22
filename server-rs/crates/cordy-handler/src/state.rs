//! Shared handler state — the Rust analogue of the Go `Handler` struct's
//! DB/redis wiring. Domain services are added per-slice as routes land.

use std::sync::Arc;

use cordy_auth::pat_cache::PatCache;
use cordy_realtime::hub::Hub;
use cordy_service::plugin::PluginService;
use cordy_service::plugin_token::CallbackTokens;
use cordy_service::task_service::TaskService;

/// Handler-layer state shared by all axum extractors.
#[derive(Clone)]
pub struct HandlerState {
    pub pool: sqlx::PgPool,
    pub pat_cache: PatCache,
    /// Realtime WS hub (cordy-realtime). `None` only in tests.
    pub hub: Option<Arc<Hub>>,
    /// Event bus (Go h.Bus) for workspace-scoped WS fanout.
    pub bus: Arc<cordy_events::Bus>,
    /// Task domain service (Go h.TaskService).
    pub tasks: Arc<TaskService>,
    /// Plugin service (Go h.PluginService).
    pub plugins: Arc<PluginService>,
    /// Hook callback token store; None disables callback tokens (fail-closed).
    pub callbacks: Option<Arc<CallbackTokens>>,
    /// Absolute base URL used in hook callback_url; empty omits the field.
    pub callback_base_url: String,
}

impl HandlerState {
    pub fn new(pool: sqlx::PgPool, pat_cache: PatCache, hub: Option<Arc<Hub>>) -> Self {
        let bus = Arc::new(cordy_events::Bus::new());
        let tasks = Arc::new(TaskService::new(pool.clone(), bus.clone()));
        let plugins = Arc::new(PluginService::with_pool(pool.clone()));
        Self {
            pool,
            pat_cache,
            hub,
            bus,
            tasks,
            plugins,
            callbacks: Some(Arc::new(CallbackTokens::new())),
            callback_base_url: String::new(),
        }
    }
}
