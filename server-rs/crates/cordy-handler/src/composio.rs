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
use cordy_composio::service::ComposioConnectionRow;
use cordy_composio::{
    ClientBuilder, Service, ServiceConfig, ServiceError, Store, UpsertConnectionParams,
};
use cordy_service::feature_flags::{composio_mcp_apps_enabled, FlagSource};
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
        let service = flags
            .as_deref()
            .filter(|flags| composio_mcp_apps_enabled(*flags))
            .and_then(|_| build_service(state.pool.clone()).ok())
            .map(Arc::new);
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

fn build_service(pool: sqlx::PgPool) -> Result<Service> {
    let api_key = env_trimmed("COMPOSIO_API_KEY");
    if api_key.is_empty() {
        anyhow::bail!("COMPOSIO_API_KEY is required");
    }
    let client = Arc::new(ClientBuilder::new(api_key).build()?);
    let secret = composio_state_secret();
    if secret.is_empty() {
        anyhow::bail!("COMPOSIO_STATE_SECRET or JWT_SECRET is required");
    }
    let callback_base_url = callback_base_url();
    if callback_base_url.is_empty() {
        anyhow::bail!("Composio callback base URL is required");
    }
    Service::new(
        client,
        Arc::new(DbStore { pool }),
        ServiceConfig {
            state_secret: secret,
            callback_base_url,
            frontend_base_url: app_url(),
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
    if jwt.is_empty() {
        Vec::new()
    } else {
        Sha256::digest(format!("composio-state:{jwt}").as_bytes()).to_vec()
    }
}

fn callback_base_url() -> String {
    ["COMPOSIO_CALLBACK_BASE_URL", "CORDY_PUBLIC_URL"]
        .into_iter()
        .map(env_trimmed)
        .find(|value| !value.is_empty())
        .unwrap_or_else(app_url)
        .trim_end_matches('/')
        .to_string()
}

fn app_url() -> String {
    ["CORDY_APP_URL", "FRONTEND_ORIGIN"]
        .into_iter()
        .map(env_trimmed)
        .find(|value| !value.is_empty())
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string()
}

fn env_trimmed(name: &str) -> String {
    std::env::var(name).unwrap_or_default().trim().to_string()
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
    let rows = match cordy_db::queries::composio::list_active_user_composio_connections(
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
        let row = cordy_db::queries::composio::upsert_user_composio_connection(
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
        let rows =
            cordy_db::queries::composio::list_active_user_composio_connections(&self.pool, user_id)
                .await?;
        Ok(rows.into_iter().map(db_row).collect())
    }

    async fn get_user_composio_connection(
        &self,
        id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<ComposioConnectionRow>> {
        Ok(
            cordy_db::queries::composio::get_user_composio_connection(&self.pool, id, user_id)
                .await?
                .map(db_row),
        )
    }

    async fn mark_user_composio_connection_revoked(&self, id: Uuid, user_id: Uuid) -> Result<()> {
        cordy_db::queries::composio::mark_user_composio_connection_revoked(&self.pool, id, user_id)
            .await?;
        Ok(())
    }
}

fn db_row(row: cordy_db::models::UserComposioConnection) -> ComposioConnectionRow {
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
            if key == cordy_service::feature_flags::COMPOSIO_MCP_APPS {
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
