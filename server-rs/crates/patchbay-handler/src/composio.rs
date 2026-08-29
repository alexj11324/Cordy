//! Composio connection-management HTTP routes.
//!
//! The OAuth callback is deliberately public and derives identity only from
//! the signed state. The other four routes are authenticated and user-scoped.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use patchbay_composio::service::ComposioConnectionRow;
use patchbay_composio::{
    ClientBuilder, Service, ServiceConfig, ServiceError, Store, UpsertConnectionParams,
};
use patchbay_authorization::{
    Action, AuthorizationContext, AuthorizationRequest, Authorizer, Principal, PrincipalType,
    Resource, ResourceType,
};
use patchbay_service::feature_flags::{composio_mcp_apps_enabled, FlagSource};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::error_response;
use crate::HandlerState;

#[derive(Clone)]
pub struct ComposioState {
    service: Option<Arc<Service>>,
    pool: sqlx::PgPool,
    flags: Option<Arc<dyn FlagSource>>,
}

struct TaskOverlayBuilder {
    service: Arc<Service>,
    authorization: Arc<dyn Authorizer>,
}

#[async_trait::async_trait]
impl patchbay_service::task_service::ComposioOverlayBuilder for TaskOverlayBuilder {
    async fn build_task_overlay(
        &self,
        _pool: &sqlx::PgPool,
        originator_user_id: Uuid,
        agent: &patchbay_db::models::Agent,
    ) -> Result<patchbay_service::runtime_apps::McpOverlayResult> {
        // A shared agent's invocation ACL is not authority to borrow the
        // definition owner's credentials. Broker a session only from the
        // initiating user's own connection set, and make that boundary
        // independently auditable.
        let decision = self
            .authorization
            .authorize(AuthorizationRequest {
                principal: Principal {
                    principal_type: PrincipalType::User,
                    id: Some(originator_user_id),
                },
                action: Action::new(Action::CREDENTIAL_USE),
                resource: Resource {
                    resource_type: ResourceType::new(ResourceType::CREDENTIAL),
                    id: None,
                    workspace_id: agent.workspace_id,
                    owner_id: Some(originator_user_id),
                    attributes: json!({"private": true}),
                },
                context: AuthorizationContext {
                    on_behalf_of_user_id: Some(originator_user_id),
                    via_agent_id: Some(agent.id),
                    ..Default::default()
                },
                delegation_chain: Vec::new(),
            })
            .await?;
        if !decision.is_allowed() {
            return Ok(patchbay_service::runtime_apps::McpOverlayResult::default());
        }
        let result = self
            .service
            .build_task_overlay(
                Some(originator_user_id),
                agent
                    .composio_toolkit_allowlist
                    .as_deref()
                    .unwrap_or_default(),
                patchbay_service::runtime_apps::display_name_for_toolkit_slug,
            )
            .await?;
        let mcp_overlay = if result.mcp_overlay.is_empty() {
            None
        } else {
            Some(serde_json::from_slice(&result.mcp_overlay)?)
        };
        Ok(patchbay_service::runtime_apps::McpOverlayResult {
            mcp_overlay,
            connected_apps: result
                .connected_apps
                .into_iter()
                .map(|app| patchbay_service::runtime_apps::ConnectedApp {
                    provider: app.provider,
                    server_name: app.server_name,
                    toolkit_slug: app.toolkit_slug,
                    toolkit_name: app.toolkit_name,
                })
                .collect(),
        })
    }
}

pub(crate) fn task_overlay_builder(
    service: Arc<Service>,
    authorization: Arc<dyn Authorizer>,
) -> Arc<dyn patchbay_service::task_service::ComposioOverlayBuilder> {
    Arc::new(TaskOverlayBuilder {
        service,
        authorization,
    })
}

impl ComposioState {
    pub fn new(
        service: Option<Arc<Service>>,
        pool: sqlx::PgPool,
        flags: Option<Arc<dyn FlagSource>>,
    ) -> Self {
        Self {
            service,
            pool,
            flags,
        }
    }

    /// Reproduces the Go boot-time gates. Invalid or incomplete configuration
    /// disables the integration while leaving the rest of the server healthy.
    pub fn from_handler(state: &HandlerState) -> Self {
        let flags = state.feature_flags.clone();
        let service = state.composio.clone();
        Self::new(service, state.pool.clone(), flags)
    }

    fn enabled_service(&self) -> Option<&Arc<Service>> {
        let enabled = self.flags.as_deref().is_some_and(composio_mcp_apps_enabled);
        enabled.then_some(())?;
        self.service.as_ref()
    }
}

pub fn public_router() -> Router<ComposioState> {
    Router::new().route("/api/integrations/composio/callback", get(callback))
}

pub fn authenticated_router() -> Router<ComposioState> {
    Router::new()
        .route(
            "/api/integrations/composio/connect/init",
            post(connect_init),
        )
        .route("/api/integrations/composio/toolkits", get(list_toolkits))
        .route(
            "/api/integrations/composio/connections",
            get(list_connections),
        )
        .route(
            "/api/integrations/composio/connections/{id}",
            delete(delete_connection),
        )
}

pub(crate) fn build_service(pool: sqlx::PgPool) -> Result<Service> {
    build_service_from_values(
        pool,
        &env_trimmed("COMPOSIO_API_KEY"),
        composio_state_secret(),
        &callback_base_url(),
        &app_url(),
    )
}

pub(crate) fn build_service_from_config(
    pool: sqlx::PgPool,
    config: &patchbay_config::Config,
) -> Result<Service> {
    let boot = composio_boot_config(config)?;
    build_service_from_values(
        pool,
        &boot.api_key,
        boot.state_secret,
        &boot.callback_base_url,
        &boot.frontend_base_url,
    )
}

struct ComposioBootConfig {
    api_key: String,
    state_secret: Vec<u8>,
    callback_base_url: String,
    frontend_base_url: String,
}

fn composio_boot_config(config: &patchbay_config::Config) -> Result<ComposioBootConfig> {
    let api_key = option_trimmed(config.integrations.composio_api_key.as_deref());
    if api_key.is_empty() {
        anyhow::bail!("COMPOSIO_API_KEY is required");
    }
    let state_secret = composio_state_secret_from_config(config);
    if state_secret.is_empty() {
        anyhow::bail!("COMPOSIO_STATE_SECRET or JWT_SECRET is required");
    }
    let callback_base_url = composio_callback_base_url_from_config(config);
    if callback_base_url.is_empty() {
        anyhow::bail!("Composio callback base URL is required");
    }
    Ok(ComposioBootConfig {
        api_key,
        state_secret,
        callback_base_url,
        frontend_base_url: app_url_from_config(config),
    })
}

fn build_service_from_values(
    pool: sqlx::PgPool,
    api_key: &str,
    secret: Vec<u8>,
    callback_base_url: &str,
    frontend_base_url: &str,
) -> Result<Service> {
    if api_key.is_empty() {
        anyhow::bail!("COMPOSIO_API_KEY is required");
    }
    let client = Arc::new(ClientBuilder::new(api_key).build()?);
    if secret.is_empty() {
        anyhow::bail!("COMPOSIO_STATE_SECRET or JWT_SECRET is required");
    }
    if callback_base_url.is_empty() {
        anyhow::bail!("Composio callback base URL is required");
    }
    Service::new(
        client,
        Arc::new(DbStore { pool }),
        ServiceConfig {
            state_secret: secret,
            callback_base_url: callback_base_url.to_string(),
            frontend_base_url: frontend_base_url.to_string(),
            state_ttl: Duration::ZERO,
            auth_config_ttl: Duration::ZERO,
        },
    )
}

fn composio_state_secret() -> Vec<u8> {
    let explicit = env_trimmed("COMPOSIO_STATE_SECRET");
    if !explicit.is_empty() {
        return explicit.into_bytes();
    }
    let jwt = env_trimmed("JWT_SECRET");
    hashed_jwt_state_secret(&jwt)
}

fn composio_state_secret_from_config(config: &patchbay_config::Config) -> Vec<u8> {
    let explicit = option_trimmed(config.integrations.composio_state_secret.as_deref());
    if !explicit.is_empty() {
        return explicit.into_bytes();
    }
    hashed_jwt_state_secret(&option_trimmed(config.auth.jwt_secret.as_deref()))
}

fn hashed_jwt_state_secret(jwt: &str) -> Vec<u8> {
    if jwt.is_empty() {
        Vec::new()
    } else {
        Sha256::digest(format!("composio-state:{jwt}").as_bytes()).to_vec()
    }
}

fn callback_base_url() -> String {
    ["COMPOSIO_CALLBACK_BASE_URL", "PATCHBAY_PUBLIC_URL"]
        .into_iter()
        .map(env_trimmed)
        .find(|value| !value.is_empty())
        .unwrap_or_else(app_url)
        .trim_end_matches('/')
        .to_string()
}

fn composio_callback_base_url_from_config(config: &patchbay_config::Config) -> String {
    [
        option_trimmed(config.integrations.composio_callback_base_url.as_deref()),
        option_trimmed(config.urls.public_url.as_deref()),
        app_url_from_config(config),
    ]
    .into_iter()
    .find(|value| !value.is_empty())
    .unwrap_or_default()
    .trim_end_matches('/')
    .to_string()
}

fn app_url() -> String {
    ["PATCHBAY_APP_URL", "FRONTEND_ORIGIN"]
        .into_iter()
        .map(env_trimmed)
        .find(|value| !value.is_empty())
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string()
}

fn app_url_from_config(config: &patchbay_config::Config) -> String {
    [
        option_trimmed(config.urls.app_url.as_deref()),
        option_trimmed(config.urls.frontend_origin.as_deref()),
    ]
    .into_iter()
    .find(|value| !value.is_empty())
    .unwrap_or_default()
    .trim_end_matches('/')
    .to_string()
}

fn env_trimmed(name: &str) -> String {
    std::env::var(name).unwrap_or_default().trim().to_string()
}

fn option_trimmed(value: Option<&str>) -> String {
    value.unwrap_or("").trim().to_string()
}

#[derive(Debug, Deserialize)]
struct ConnectInitRequest {
    #[serde(default)]
    toolkit_slug: String,
}

async fn connect_init(
    State(state): State<ComposioState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(service) = state.enabled_service() else {
        return unavailable();
    };
    let user_id = match authenticated_user_id(&headers) {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let request: ConnectInitRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request.toolkit_slug.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "toolkit_slug is required");
    }
    match service.begin_connect(user_id, &request.toolkit_slug).await {
        Ok(redirect_url) => Json(json!({ "redirect_url": redirect_url })).into_response(),
        Err(error) if is_service_error(&error, ServiceError::ToolkitNotSupported) => {
            error_response(StatusCode::BAD_REQUEST, "toolkit not supported")
        }
        Err(error) => {
            tracing::warn!(%error, "failed to start composio connect");
            error_response(StatusCode::BAD_GATEWAY, "failed to start composio connect")
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct CallbackQuery {
    #[serde(default)]
    state: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    connected_account_id: String,
}

async fn callback(
    State(state): State<ComposioState>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let Some(service) = state.enabled_service() else {
        return unavailable();
    };
    let (slug, success) = match service
        .complete_callback(&query.state, &query.status, &query.connected_account_id)
        .await
    {
        Ok(slug) => (slug, true),
        Err(error) => (error.toolkit_slug().unwrap_or_default().to_string(), false),
    };
    found_redirect(&service.callback_redirect(&slug, success))
}

async fn list_connections(State(state): State<ComposioState>, headers: HeaderMap) -> Response {
    if state.enabled_service().is_none() {
        return unavailable();
    }
    let user_id = match authenticated_user_id(&headers) {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let rows = match patchbay_db::queries::composio::list_active_user_composio_connections(
        &state.pool,
        user_id,
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, "failed to list composio connections");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list composio connections",
            );
        }
    };
    Json(
        rows.into_iter()
            .map(|row| ConnectionResponse {
                id: row.id.to_string(),
                toolkit_slug: row.toolkit_slug,
                status: row.status,
                connected_at: crate::timefmt::rfc3339(row.connected_at),
                last_used_at: row.last_used_at.map(crate::timefmt::rfc3339),
            })
            .collect::<Vec<_>>(),
    )
    .into_response()
}

#[derive(Debug, Serialize)]
struct ConnectionResponse {
    id: String,
    toolkit_slug: String,
    status: String,
    connected_at: String,
    last_used_at: Option<String>,
}

async fn list_toolkits(State(state): State<ComposioState>, headers: HeaderMap) -> Response {
    let Some(service) = state.enabled_service() else {
        return unavailable();
    };
    if let Err(response) = authenticated_user_id(&headers) {
        return response;
    }
    match service.list_toolkits().await {
        Ok(toolkits) => Json(toolkits).into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to list composio toolkits");
            error_response(StatusCode::BAD_GATEWAY, "failed to list composio toolkits")
        }
    }
}

async fn delete_connection(
    State(state): State<ComposioState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(service) = state.enabled_service() else {
        return unavailable();
    };
    let user_id = match authenticated_user_id(&headers) {
        Ok(user_id) => user_id,
        Err(response) => return response,
    };
    let connection_id = match Uuid::parse_str(&id) {
        Ok(id) => id,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid connection id"),
    };
    match service.disconnect(user_id, connection_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) if is_service_error(&error, ServiceError::ConnectionNotFound) => {
            error_response(StatusCode::NOT_FOUND, "composio connection not found")
        }
        Err(error) => {
            tracing::warn!(%error, "failed to disconnect composio connection");
            error_response(
                StatusCode::BAD_GATEWAY,
                "failed to disconnect composio connection",
            )
        }
    }
}

fn is_service_error(error: &anyhow::Error, expected: ServiceError) -> bool {
    error
        .downcast_ref::<ServiceError>()
        .is_some_and(|actual| *actual == expected)
}

fn authenticated_user_id(headers: &HeaderMap) -> std::result::Result<Uuid, Response> {
    let Some(raw) = headers
        .get("x-user-id")
        .and_then(|value| value.to_str().ok())
    else {
        return Err(error_response(
            StatusCode::UNAUTHORIZED,
            "user not authenticated",
        ));
    };
    if raw.is_empty() {
        return Err(error_response(
            StatusCode::UNAUTHORIZED,
            "user not authenticated",
        ));
    }
    Uuid::parse_str(raw).map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid user id"))
}

fn unavailable() -> Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "composio integration not configured",
    )
}

fn found_redirect(location: &str) -> Response {
    match HeaderValue::from_str(location) {
        Ok(location) => (StatusCode::FOUND, [(header::LOCATION, location)]).into_response(),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "invalid redirect URL"),
    }
}

struct DbStore {
    pool: sqlx::PgPool,
}

#[async_trait::async_trait]
impl Store for DbStore {
    async fn upsert_user_composio_connection(&self, p: UpsertConnectionParams) -> Result<()> {
        let row = patchbay_db::queries::composio::upsert_user_composio_connection(
            &self.pool,
            p.user_id,
            &p.toolkit_slug,
            &p.auth_config_id,
            &p.connected_account_id,
            &p.composio_user_id,
        )
        .await?;
        if row.is_none() {
            anyhow::bail!("upsert composio connection returned no row");
        }
        Ok(())
    }

    async fn list_active_user_composio_connections(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<ComposioConnectionRow>> {
        let rows = patchbay_db::queries::composio::list_active_user_composio_connections(
            &self.pool, user_id,
        )
        .await?;
        Ok(rows.into_iter().map(db_row).collect())
    }

    async fn get_user_composio_connection(
        &self,
        id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<ComposioConnectionRow>> {
        Ok(
            patchbay_db::queries::composio::get_user_composio_connection(&self.pool, id, user_id)
                .await?
                .map(db_row),
        )
    }

    async fn mark_user_composio_connection_revoked(&self, id: Uuid, user_id: Uuid) -> Result<()> {
        patchbay_db::queries::composio::mark_user_composio_connection_revoked(
            &self.pool, id, user_id,
        )
        .await?;
        Ok(())
    }
}

fn db_row(row: patchbay_db::models::UserComposioConnection) -> ComposioConnectionRow {
    ComposioConnectionRow {
        id: row.id,
        toolkit_slug: row.toolkit_slug,
        status: row.status,
        connected_account_id: row.connected_account_id,
        connected_at_unix: row.connected_at.timestamp(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    struct FixedFlags(bool);

    impl FlagSource for FixedFlags {
        fn is_enabled(&self, key: &str, default: bool) -> bool {
            if key == patchbay_service::feature_flags::COMPOSIO_MCP_APPS {
                self.0
            } else {
                default
            }
        }
    }

    fn lazy_pool() -> sqlx::PgPool {
        sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap()
    }

    fn disabled_state(flag: bool) -> ComposioState {
        ComposioState::new(None, lazy_pool(), Some(Arc::new(FixedFlags(flag))))
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn every_route_is_503_when_service_or_flag_is_disabled() {
        for flag in [false, true] {
            let state = disabled_state(flag);
            let private = authenticated_router().with_state(state.clone());
            for (method, uri) in [
                ("POST", "/api/integrations/composio/connect/init"),
                ("GET", "/api/integrations/composio/toolkits"),
                ("GET", "/api/integrations/composio/connections"),
                (
                    "DELETE",
                    "/api/integrations/composio/connections/00000000-0000-0000-0000-000000000001",
                ),
            ] {
                let response = private
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method)
                            .uri(uri)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
                assert_eq!(
                    body_json(response).await["error"],
                    "composio integration not configured"
                );
            }

            let response = public_router()
                .with_state(state)
                .oneshot(
                    Request::get("/api/integrations/composio/callback")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        }
    }

    #[test]
    fn state_secret_fallback_is_domain_separated() {
        let jwt = "jwt-secret";
        let expected = Sha256::digest(format!("composio-state:{jwt}").as_bytes()).to_vec();
        assert_ne!(expected, Sha256::digest(jwt.as_bytes()).to_vec());
        assert_eq!(hashed_jwt_state_secret(jwt), expected);
    }

    #[test]
    fn loaded_config_builds_composio_without_process_env() {
        let mut config = patchbay_config::Config::default();
        config.integrations.composio_api_key = Some(" toml-api-key ".into());
        config.integrations.composio_callback_base_url = Some("https://api.example/ ".into());
        config.integrations.composio_state_secret = Some(" toml-state-secret ".into());
        config.urls.app_url = Some("https://app.example/".into());
        let boot = composio_boot_config(&config).unwrap();
        assert_eq!(boot.api_key, "toml-api-key");
        assert_eq!(boot.callback_base_url, "https://api.example");
        assert_eq!(boot.frontend_base_url, "https://app.example");
        assert_eq!(boot.state_secret, b"toml-state-secret");

        config.integrations.composio_state_secret = None;
        config.auth.jwt_secret = Some(" jwt-from-toml ".into());
        let boot = composio_boot_config(&config).unwrap();
        assert_eq!(boot.state_secret, hashed_jwt_state_secret("jwt-from-toml"));

        config.integrations.composio_api_key = None;
        assert!(composio_boot_config(&config).is_err());
    }

    #[tokio::test]
    async fn loaded_config_composio_service_constructs() {
        let mut config = patchbay_config::Config::default();
        config.integrations.composio_api_key = Some("toml-api-key".into());
        config.integrations.composio_callback_base_url = Some("https://api.example".into());
        config.integrations.composio_state_secret = Some("toml-state-secret".into());
        build_service_from_config(lazy_pool(), &config).unwrap();
    }

    #[test]
    fn found_redirect_is_a_302() {
        let response = found_redirect("/settings?tab=integrations&connected=notion");
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response.headers()[header::LOCATION],
            "/settings?tab=integrations&connected=notion"
        );
    }
}
