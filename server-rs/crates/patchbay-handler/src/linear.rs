//! Linear installation foundation.
//!
//! This module deliberately stops at installation and durable Webhook
//! receipt. It does not mutate Issues. Later sync workers consume the Inbox
//! through an explicit domain boundary.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Extension, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use patchbay_db::models::LinearConnection;
use patchbay_db::queries::linear as linear_q;
use patchbay_middleware::workspace::WorkspaceContext;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use url::Url;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

type HmacSha256 = Hmac<Sha256>;

const LINEAR_AUTH_URL: &str = "https://linear.app/oauth/authorize";
const LINEAR_TOKEN_URL: &str = "https://api.linear.app/oauth/token";
const LINEAR_REVOKE_URL: &str = "https://api.linear.app/oauth/revoke";
const LINEAR_GRAPHQL_URL: &str = "https://api.linear.app/graphql";
const LINEAR_OAUTH_SCOPE: &str = "read,write,issues:create,app:assignable";
const WEBHOOK_MAX_AGE_MS: i128 = 60_000;
const TOKEN_REFRESH_SKEW: Duration = Duration::minutes(5);
const MAX_WEBHOOK_BODY_BYTES: usize = 2 * 1024 * 1024;

pub fn member_router() -> Router<HandlerState> {
    Router::new().route("/api/workspaces/{id}/linear", get(get_connection))
}

pub fn admin_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/workspaces/{id}/linear/connect", post(start_oauth))
        .route("/api/workspaces/{id}/linear", delete(disconnect))
}

pub fn public_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/linear/oauth/callback", get(oauth_callback))
        .route("/api/webhooks/linear", post(linear_webhook))
        .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY_BYTES))
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace id"))
}

fn configured_value(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn env_value(name: &str) -> Option<String> {
    configured_value(std::env::var(name).ok().as_deref())
}

fn linear_redirect_uri(state: &HandlerState) -> Option<String> {
    state
        .integrations
        .linear_redirect_uri
        .as_deref()
        .and_then(|value| configured_value(Some(value)))
        .or_else(|| {
            configured_value(Some(&state.public_config.public_url))
                .map(|base| format!("{}/api/linear/oauth/callback", base.trim_end_matches('/')))
        })
}

fn frontend_origin() -> String {
    let Some(raw) = env_value("FRONTEND_ORIGIN") else {
        return "http://localhost:3000".to_string();
    };
    let Ok(mut url) = Url::parse(&raw) else {
        return "http://localhost:3000".to_string();
    };
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return "http://localhost:3000".to_string();
    }
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    url.to_string().trim_end_matches('/').to_string()
}

fn linear_callback_redirect(outcome: &str) -> Response {
    Redirect::temporary(&format!(
        "{}/settings?tab=integrations&linear_{}=1",
        frontend_origin(),
        outcome
    ))
    .into_response()
}

fn build_authorization_url(
    auth_url: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    verifier: &str,
) -> Result<String, url::ParseError> {
    let mut url = Url::parse(auth_url)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    url.query_pairs_mut()
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("state", state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("scope", LINEAR_OAUTH_SCOPE)
        .append_pair("actor", "app");
    Ok(url.to_string())
}

fn connection_json(connection: LinearConnection) -> Value {
    json!({
        "id": connection.id,
        "workspace_id": connection.workspace_id,
        "organization_id": connection.organization_id,
        "organization_name": connection.organization_name,
        "actor_id": connection.actor_id,
        "scopes": connection.scopes,
        "webhook_id": connection.webhook_id,
        "status": connection.status,
        "token_expires_at": connection.token_expires_at,
        "last_success_at": connection.last_success_at,
        "last_error": connection.last_error,
        "created_at": connection.created_at,
        "updated_at": connection.updated_at,
    })
}

fn integration_disabled(state: &HandlerState) -> Option<Response> {
    (!state.linear_integration_enabled).then(|| {
        error_response(
            StatusCode::NOT_FOUND,
            "Linear integration is not configured",
        )
    })
}

async fn get_connection(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    match linear_q::get_connection_for_workspace(&state.pool, workspace_id).await {
        Ok(Some(connection)) => Json(json!({
            "configured": true,
            "connected": connection.status != "revoked",
            "connection": connection_json(connection),
        }))
        .into_response(),
        Ok(None) => Json(json!({
            "configured": true,
            "connected": false,
            "connection": Value::Null,
        }))
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, "Linear connection lookup failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load Linear connection",
            )
        }
    }
}

async fn start_oauth(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let Some(client_id) = state
        .integrations
        .linear_client_id
        .as_deref()
        .and_then(|value| configured_value(Some(value)))
    else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Linear OAuth is not configured",
        );
    };
    if state
        .integrations
        .linear_client_secret
        .as_deref()
        .and_then(|value| configured_value(Some(value)))
        .is_none()
    {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Linear OAuth is not configured",
        );
    }
    let Some(redirect_uri) = linear_redirect_uri(&state) else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Linear OAuth redirect URI is not configured",
        );
    };
    if state.linear_secret_box.is_none() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Linear encrypted secret storage is not configured",
        );
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };

    let state_token = random_token(32);
    let verifier = random_token(48);
    let verifier_encrypted = match seal(&state, &verifier) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "Linear OAuth verifier encryption failed");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Linear encrypted secret storage is unavailable",
            );
        }
    };
    let expires_at = Utc::now() + Duration::minutes(10);
    let state_hash = sha256_hex(&state_token);
    if let Err(error) = linear_q::insert_oauth_state(
        &state.pool,
        &linear_q::OAuthStateInput {
            id: Uuid::now_v7(),
            state_hash: &state_hash,
            workspace_id,
            user_id: context.member.user_id,
            code_verifier_encrypted: &verifier_encrypted,
            redirect_uri: &redirect_uri,
            expires_at,
        },
    )
    .await
    {
        tracing::warn!(%error, "Linear OAuth state persistence failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to prepare Linear authorization",
        );
    }

    let auth_url = state
        .integrations
        .linear_auth_url
        .as_deref()
        .and_then(|value| configured_value(Some(value)))
        .unwrap_or_else(|| LINEAR_AUTH_URL.to_string());
    let authorization_url = match build_authorization_url(
        &auth_url,
        &client_id,
        &redirect_uri,
        &state_token,
        &verifier,
    ) {
        Ok(url) => url,
        Err(error) => {
            tracing::warn!(%error, "Linear OAuth authorization URL is invalid");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Linear OAuth authorization URL is invalid",
            );
        }
    };
    Json(json!({
        "authorization_url": authorization_url,
        "state_expires_at": expires_at,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct OAuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// The OAuth token response intentionally contains only documented token
/// fields. Installation identity comes from the authenticated GraphQL API,
/// never from undocumented token-response extensions.
#[derive(Debug, Deserialize)]
struct LinearTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphQlResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdentityData {
    viewer: IdentityViewer,
    organization: Option<IdentityOrganization>,
}

#[derive(Debug, Deserialize)]
struct IdentityViewer {
    id: String,
}

#[derive(Debug, Deserialize)]
struct IdentityOrganization {
    id: String,
    name: String,
}

#[derive(Debug)]
struct LinearIdentity {
    actor_id: String,
    organization_id: String,
    organization_name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LinearTokenError {
    #[error("Linear authorization requires reauthorization")]
    InvalidGrant,
    #[error("Linear provider returned an invalid response")]
    InvalidResponse,
    #[error("Linear provider request failed")]
    Provider,
    #[error("Linear integration is not configured")]
    NotConfigured,
    #[error("Linear storage operation failed: {0}")]
    Storage(#[source] anyhow::Error),
    #[error("Linear secret storage operation failed: {0}")]
    Secret(#[source] anyhow::Error),
}

#[derive(Clone)]
pub struct LinearTokenManager {
    pool: PgPool,
    secret_box: patchbay_util::secretbox::SecretBox,
    client: reqwest::Client,
    client_id: String,
    client_secret: String,
    token_url: String,
    revoke_url: String,
    graphql_url: String,
}

impl LinearTokenManager {
    pub fn from_state(state: &HandlerState) -> Result<Self, LinearTokenError> {
        let Some(secret_box) = state.linear_secret_box.clone() else {
            return Err(LinearTokenError::NotConfigured);
        };
        let Some(client_id) = state
            .integrations
            .linear_client_id
            .as_deref()
            .and_then(|value| configured_value(Some(value)))
        else {
            return Err(LinearTokenError::NotConfigured);
        };
        let Some(client_secret) = state
            .integrations
            .linear_client_secret
            .as_deref()
            .and_then(|value| configured_value(Some(value)))
        else {
            return Err(LinearTokenError::NotConfigured);
        };
        Ok(Self {
            pool: state.pool.clone(),
            secret_box,
            client: reqwest::Client::new(),
            client_id,
            client_secret,
            token_url: state
                .integrations
                .linear_token_url
                .as_deref()
                .and_then(|value| configured_value(Some(value)))
                .unwrap_or_else(|| LINEAR_TOKEN_URL.to_string()),
            revoke_url: state
                .integrations
                .linear_revoke_url
                .as_deref()
                .and_then(|value| configured_value(Some(value)))
                .unwrap_or_else(|| LINEAR_REVOKE_URL.to_string()),
            graphql_url: state
                .integrations
                .linear_graphql_url
                .as_deref()
                .and_then(|value| configured_value(Some(value)))
                .unwrap_or_else(|| LINEAR_GRAPHQL_URL.to_string()),
        })
    }

    async fn exchange_authorization_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<LinearTokenResponse, LinearTokenError> {
        self.request_token(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("code_verifier", verifier),
        ])
        .await
    }

    async fn refresh_token(
        &self,
        refresh_token: &str,
    ) -> Result<LinearTokenResponse, LinearTokenError> {
        self.request_token(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
        ])
        .await
    }

    async fn request_token(
        &self,
        form: &[(&str, &str)],
    ) -> Result<LinearTokenResponse, LinearTokenError> {
        let response = self
            .client
            .post(&self.token_url)
            .form(form)
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear token endpoint request failed");
                LinearTokenError::Provider
            })?;
        let status = response.status();
        let payload = response.json::<Value>().await.map_err(|error| {
            tracing::warn!(%error, "Linear token endpoint returned invalid JSON");
            LinearTokenError::InvalidResponse
        })?;
        if !status.is_success() {
            if payload.get("error").and_then(Value::as_str) == Some("invalid_grant") {
                return Err(LinearTokenError::InvalidGrant);
            }
            tracing::warn!(%status, "Linear token endpoint rejected request");
            return Err(LinearTokenError::Provider);
        }
        let token: LinearTokenResponse = serde_json::from_value(payload).map_err(|error| {
            tracing::warn!(%error, "Linear token response shape is invalid");
            LinearTokenError::InvalidResponse
        })?;
        if token.access_token.trim().is_empty()
            || token.refresh_token.as_deref().is_none_or(str::is_empty)
            || token.expires_in.is_none_or(|value| value <= 0)
        {
            return Err(LinearTokenError::InvalidResponse);
        }
        Ok(token)
    }

    async fn discover_identity(
        &self,
        access_token: &str,
    ) -> Result<LinearIdentity, LinearTokenError> {
        let response = self
            .client
            .post(&self.graphql_url)
            .bearer_auth(access_token)
            .json(&json!({
                "query": "query LinearInstallationIdentity { viewer { id } organization { id name } }"
            }))
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear identity request failed");
                LinearTokenError::Provider
            })?;
        let status = response.status();
        let payload = response
            .json::<GraphQlResponse<IdentityData>>()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear identity response is invalid JSON");
                LinearTokenError::InvalidResponse
            })?;
        if !status.is_success() {
            tracing::warn!(%status, "Linear identity request returned an error");
            return Err(LinearTokenError::Provider);
        }
        if let Some(errors) = payload.errors.as_ref().filter(|errors| !errors.is_empty()) {
            let message = errors
                .first()
                .and_then(|error| error.message.as_deref())
                .unwrap_or("unknown GraphQL error");
            tracing::warn!(message, "Linear identity GraphQL request failed");
            return Err(LinearTokenError::Provider);
        }
        let Some(data) = payload.data else {
            return Err(LinearTokenError::InvalidResponse);
        };
        let Some(organization) = data.organization else {
            return Err(LinearTokenError::InvalidResponse);
        };
        if data.viewer.id.trim().is_empty()
            || organization.id.trim().is_empty()
            || organization.name.trim().is_empty()
        {
            return Err(LinearTokenError::InvalidResponse);
        }
        Ok(LinearIdentity {
            actor_id: data.viewer.id,
            organization_id: organization.id,
            organization_name: organization.name,
        })
    }

    /// Returns an access token and refreshes it while holding the connection
    /// row lock. The refresh response must contain the rotated refresh token;
    /// both encrypted values are replaced in one database transaction.
    pub async fn access_token(&self, connection_id: Uuid) -> Result<String, LinearTokenError> {
        let mut transaction = self.pool.begin().await.map_err(storage_sqlx_error)?;
        let Some(connection) =
            linear_q::get_connection_for_update(&mut transaction, connection_id)
                .await
                .map_err(storage_error)?
        else {
            return Err(LinearTokenError::InvalidResponse);
        };
        if connection.status == "revoked" {
            return Err(LinearTokenError::InvalidResponse);
        }
        let access_token = open_secret(&self.secret_box, &connection.access_token_encrypted)?;
        if connection.token_expires_at > Utc::now() + TOKEN_REFRESH_SKEW {
            transaction.commit().await.map_err(storage_sqlx_error)?;
            return Ok(access_token);
        }
        let refresh_token = open_secret(&self.secret_box, &connection.refresh_token_encrypted)?;
        let refreshed = match self.refresh_token(&refresh_token).await {
            Ok(token) => token,
            Err(LinearTokenError::InvalidGrant) => {
                linear_q::mark_reauthorization_required(
                    &mut transaction,
                    connection_id,
                    "invalid_grant",
                )
                .await
                .map_err(storage_error)?;
                transaction.commit().await.map_err(storage_sqlx_error)?;
                return Err(LinearTokenError::InvalidGrant);
            }
            Err(error) => return Err(error),
        };
        let rotated_refresh = refreshed
            .refresh_token
            .as_deref()
            .ok_or(LinearTokenError::InvalidResponse)?;
        let expires_in = refreshed
            .expires_in
            .ok_or(LinearTokenError::InvalidResponse)?;
        let access_encrypted = seal_secret(&self.secret_box, &refreshed.access_token)?;
        let refresh_encrypted = seal_secret(&self.secret_box, rotated_refresh)?;
        let scopes = refreshed
            .scope
            .as_deref()
            .map(parse_scopes)
            .unwrap_or_else(|| connection.scopes.clone());
        linear_q::update_tokens(
            &mut transaction,
            connection_id,
            &access_encrypted,
            &refresh_encrypted,
            Utc::now() + Duration::seconds(expires_in),
            &scopes,
        )
        .await
        .map_err(storage_error)?;
        transaction.commit().await.map_err(storage_sqlx_error)?;
        Ok(refreshed.access_token)
    }

    async fn revoke_connection(
        &self,
        workspace_id: Uuid,
        connection: &LinearConnection,
    ) -> Result<(), LinearTokenError> {
        if connection.status == "revoked" {
            return Ok(());
        }
        let access_token = self.access_token(connection.id).await?;
        let response = self
            .client
            .post(&self.revoke_url)
            .form(&[
                ("token", access_token.as_str()),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await
            .map_err(|error| {
                tracing::warn!(%error, "Linear revoke request failed");
                LinearTokenError::Provider
            })?;
        if !response.status().is_success() {
            tracing::warn!(status = %response.status(), "Linear revoke request rejected");
            return Err(LinearTokenError::Provider);
        }
        let marked = linear_q::mark_revoked(&self.pool, workspace_id, connection.id)
            .await
            .map_err(storage_error)?;
        if !marked {
            return Err(LinearTokenError::InvalidResponse);
        }
        Ok(())
    }
}

fn storage_error(error: anyhow::Error) -> LinearTokenError {
    LinearTokenError::Storage(error)
}

fn storage_sqlx_error(error: sqlx::Error) -> LinearTokenError {
    LinearTokenError::Storage(error.into())
}

fn seal_secret(
    secret_box: &patchbay_util::secretbox::SecretBox,
    plaintext: &str,
) -> Result<String, LinearTokenError> {
    secret_box
        .seal(plaintext.as_bytes())
        .map(|value| STANDARD.encode(value))
        .map_err(|error| LinearTokenError::Secret(anyhow::Error::from(error)))
}

fn open_secret(
    secret_box: &patchbay_util::secretbox::SecretBox,
    ciphertext: &str,
) -> Result<String, LinearTokenError> {
    let decoded = STANDARD
        .decode(ciphertext)
        .map_err(|error| LinearTokenError::Secret(anyhow::Error::from(error)))?;
    let plaintext = secret_box
        .open(&decoded)
        .map_err(|error| LinearTokenError::Secret(anyhow::Error::from(error)))?;
    String::from_utf8(plaintext)
        .map_err(|error| LinearTokenError::Secret(anyhow::Error::from(error)))
}

fn seal(state: &HandlerState, plaintext: &str) -> anyhow::Result<String> {
    let secret_box = state
        .linear_secret_box
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Linear secret storage is not configured"))?;
    seal_secret(secret_box, plaintext).map_err(|error| anyhow::anyhow!(error.to_string()))
}

async fn oauth_callback(
    State(state): State<HandlerState>,
    Query(query): Query<OAuthCallbackQuery>,
) -> Response {
    if !state.linear_integration_enabled {
        return linear_callback_redirect("not_configured");
    }
    let Some(state_token) = query.state.filter(|value| !value.trim().is_empty()) else {
        return linear_callback_redirect("invalid_request");
    };
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "Linear OAuth callback transaction failed");
            return linear_callback_redirect("error");
        }
    };
    let oauth_state =
        match linear_q::consume_oauth_state(&mut transaction, &sha256_hex(&state_token)).await {
            Ok(Some(value)) => value,
            Ok(None) => return linear_callback_redirect("invalid_state"),
            Err(error) => {
                tracing::warn!(%error, "Linear OAuth state lookup failed");
                return linear_callback_redirect("error");
            }
        };
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, "Linear OAuth state commit failed");
        return linear_callback_redirect("error");
    }
    if query.error.is_some() {
        return linear_callback_redirect("denied");
    }
    let Some(code) = query.code.filter(|value| !value.trim().is_empty()) else {
        return linear_callback_redirect("invalid_request");
    };
    let verifier = match open(&state, &oauth_state.code_verifier_encrypted) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "Linear OAuth verifier decryption failed");
            return linear_callback_redirect("error");
        }
    };
    let manager = match LinearTokenManager::from_state(&state) {
        Ok(manager) => manager,
        Err(error) => {
            tracing::warn!(%error, "Linear OAuth configuration is incomplete");
            return linear_callback_redirect("not_configured");
        }
    };
    let token = match manager
        .exchange_authorization_code(&code, &verifier, &oauth_state.redirect_uri)
        .await
    {
        Ok(token) => token,
        Err(error) => {
            tracing::warn!(%error, "Linear OAuth token exchange failed");
            return linear_callback_redirect("error");
        }
    };
    let identity = match manager.discover_identity(&token.access_token).await {
        Ok(identity) => identity,
        Err(error) => {
            tracing::warn!(%error, "Linear installation identity discovery failed");
            return linear_callback_redirect("error");
        }
    };
    let refresh_token = match token.refresh_token.as_deref() {
        Some(value) if !value.trim().is_empty() => value,
        _ => return linear_callback_redirect("error"),
    };
    let expires_in = match token.expires_in {
        Some(value) if value > 0 => value,
        _ => return linear_callback_redirect("error"),
    };
    let access_encrypted = match seal_secret(&manager.secret_box, &token.access_token) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "Linear access token encryption failed");
            return linear_callback_redirect("error");
        }
    };
    let refresh_encrypted = match seal_secret(&manager.secret_box, refresh_token) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "Linear refresh token encryption failed");
            return linear_callback_redirect("error");
        }
    };
    let scopes = token
        .scope
        .as_deref()
        .map(parse_scopes)
        .unwrap_or_else(|| json!([]));
    if let Err(error) = linear_q::upsert_connection(
        &state.pool,
        &linear_q::LinearConnectionInput {
            id: Uuid::now_v7(),
            workspace_id: oauth_state.workspace_id,
            organization_id: &identity.organization_id,
            organization_name: &identity.organization_name,
            actor_id: &identity.actor_id,
            access_token_encrypted: &access_encrypted,
            refresh_token_encrypted: &refresh_encrypted,
            token_expires_at: Utc::now() + Duration::seconds(expires_in),
            scopes: &scopes,
            created_by_id: oauth_state.user_id,
        },
    )
    .await
    {
        tracing::warn!(%error, "Linear connection persistence failed");
        return linear_callback_redirect("error");
    }
    linear_callback_redirect("connected")
}

fn open(state: &HandlerState, ciphertext: &str) -> anyhow::Result<String> {
    let secret_box = state
        .linear_secret_box
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Linear secret storage is not configured"))?;
    let decoded = STANDARD.decode(ciphertext)?;
    Ok(String::from_utf8(secret_box.open(&decoded)?)?)
}

async fn disconnect(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let connection = match linear_q::get_connection_for_workspace(&state.pool, workspace_id).await {
        Ok(Some(connection)) => connection,
        Ok(None) => return StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::warn!(%error, "Linear disconnect lookup failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load Linear connection",
            );
        }
    };
    if connection.status == "revoked" {
        return StatusCode::NO_CONTENT.into_response();
    }
    let manager = match LinearTokenManager::from_state(&state) {
        Ok(manager) => manager,
        Err(error) => {
            tracing::warn!(%error, "Linear disconnect configuration is incomplete");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Linear OAuth is not configured",
            );
        }
    };
    match manager.revoke_connection(workspace_id, &connection).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(LinearTokenError::InvalidGrant) => error_response(
            StatusCode::CONFLICT,
            "Linear authorization requires reauthorization before disconnect",
        ),
        Err(LinearTokenError::Provider) => {
            error_response(StatusCode::BAD_GATEWAY, "Linear revoke request failed")
        }
        Err(error) => {
            tracing::warn!(%error, "Linear disconnect failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to disconnect Linear",
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct LinearWebhookEnvelope {
    #[serde(rename = "organizationId")]
    organization_id: Option<String>,
    #[serde(rename = "webhookId")]
    webhook_id: Option<String>,
    #[serde(rename = "webhookTimestamp")]
    webhook_timestamp: Option<i64>,
    #[serde(rename = "type")]
    event_type: Option<String>,
}

#[derive(Debug, PartialEq)]
struct VerifiedWebhook {
    organization_id: String,
    webhook_id: String,
    delivery_id: String,
    event_type: String,
    payload: Value,
}

#[derive(Debug, PartialEq, Eq)]
enum WebhookValidationError {
    MissingSecret,
    MissingSignature,
    InvalidSignature,
    InvalidPayload,
    MissingOrganization,
    MissingWebhook,
    MissingTimestamp,
    ExpiredTimestamp,
    InvalidHeaderTimestamp,
    TimestampMismatch,
    MissingDelivery,
}

fn validate_webhook(
    secret: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
    now_ms: i64,
) -> Result<VerifiedWebhook, WebhookValidationError> {
    let secret = secret
        .and_then(|value| configured_value(Some(value)))
        .ok_or(WebhookValidationError::MissingSecret)?;
    let signature = header_value(headers, "linear-signature")
        .ok_or(WebhookValidationError::MissingSignature)?;
    if !verify_signature(&secret, &signature, body) {
        return Err(WebhookValidationError::InvalidSignature);
    }
    let payload = serde_json::from_slice::<Value>(body)
        .map_err(|_| WebhookValidationError::InvalidPayload)?;
    let envelope = serde_json::from_value::<LinearWebhookEnvelope>(payload.clone())
        .map_err(|_| WebhookValidationError::InvalidPayload)?;
    let organization_id = envelope
        .organization_id
        .as_deref()
        .and_then(|value| configured_value(Some(value)))
        .ok_or(WebhookValidationError::MissingOrganization)?;
    let webhook_id = envelope
        .webhook_id
        .as_deref()
        .and_then(|value| configured_value(Some(value)))
        .ok_or(WebhookValidationError::MissingWebhook)?;
    let webhook_timestamp = envelope
        .webhook_timestamp
        .ok_or(WebhookValidationError::MissingTimestamp)?;
    if !timestamp_is_fresh(webhook_timestamp, now_ms) {
        return Err(WebhookValidationError::ExpiredTimestamp);
    }
    if let Some(header_timestamp) = header_value(headers, "linear-timestamp") {
        let header_timestamp = header_timestamp
            .parse::<i64>()
            .map_err(|_| WebhookValidationError::InvalidHeaderTimestamp)?;
        if header_timestamp != webhook_timestamp {
            return Err(WebhookValidationError::TimestampMismatch);
        }
    }
    let delivery_id =
        header_value(headers, "linear-delivery").ok_or(WebhookValidationError::MissingDelivery)?;
    let event_type = header_value(headers, "linear-event")
        .or_else(|| configured_value(envelope.event_type.as_deref()))
        .unwrap_or_else(|| "unknown".to_string());
    Ok(VerifiedWebhook {
        organization_id,
        webhook_id,
        delivery_id,
        event_type,
        payload,
    })
}

fn webhook_validation_response(error: WebhookValidationError) -> Response {
    let (status, message) = match error {
        WebhookValidationError::MissingSecret => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Linear Webhook secret is not configured",
        ),
        WebhookValidationError::MissingSignature => {
            (StatusCode::UNAUTHORIZED, "missing Linear signature")
        }
        WebhookValidationError::InvalidSignature => {
            (StatusCode::UNAUTHORIZED, "invalid Linear signature")
        }
        WebhookValidationError::InvalidPayload => {
            (StatusCode::BAD_REQUEST, "invalid Linear Webhook payload")
        }
        WebhookValidationError::MissingOrganization => (
            StatusCode::BAD_REQUEST,
            "Linear Webhook organizationId is required",
        ),
        WebhookValidationError::MissingWebhook => (
            StatusCode::BAD_REQUEST,
            "Linear Webhook webhookId is required",
        ),
        WebhookValidationError::MissingTimestamp => (
            StatusCode::BAD_REQUEST,
            "Linear Webhook webhookTimestamp is required",
        ),
        WebhookValidationError::ExpiredTimestamp => (
            StatusCode::BAD_REQUEST,
            "Linear Webhook timestamp is expired",
        ),
        WebhookValidationError::InvalidHeaderTimestamp => {
            (StatusCode::BAD_REQUEST, "invalid Linear timestamp")
        }
        WebhookValidationError::TimestampMismatch => {
            (StatusCode::BAD_REQUEST, "Linear timestamps do not match")
        }
        WebhookValidationError::MissingDelivery => {
            (StatusCode::BAD_REQUEST, "missing Linear delivery id")
        }
    };
    error_response(status, message)
}

async fn linear_webhook(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = integration_disabled(&state) {
        return response;
    }
    let verified = match validate_webhook(
        state.integrations.linear_webhook_secret.as_deref(),
        &headers,
        &body,
        current_time_millis(),
    ) {
        Ok(verified) => verified,
        Err(error) => return webhook_validation_response(error),
    };

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "Linear Webhook transaction failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Linear Webhook persistence unavailable",
            );
        }
    };
    let candidates = match linear_q::find_connections_for_webhook(
        &mut transaction,
        &verified.organization_id,
        &verified.webhook_id,
    )
    .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(%error, "Linear Webhook installation lookup failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Linear Webhook persistence unavailable",
            );
        }
    };
    if candidates.len() != 1 {
        return error_response(StatusCode::NOT_FOUND, "unknown Linear Webhook installation");
    }
    let connection = &candidates[0];
    if connection.webhook_id.is_none() {
        match linear_q::bind_webhook(&mut transaction, connection.id, &verified.webhook_id).await {
            Ok(true) => {}
            Ok(false) => {
                return error_response(StatusCode::CONFLICT, "Linear Webhook installation changed")
            }
            Err(error) => {
                tracing::warn!(%error, "Linear Webhook identity binding failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Linear Webhook persistence unavailable",
                );
            }
        }
    }
    let inserted = match linear_q::insert_sync_inbox(
        &mut transaction,
        Uuid::now_v7(),
        connection.id,
        &verified.delivery_id,
        &verified.event_type,
        &verified.payload,
    )
    .await
    {
        Ok(inserted) => inserted,
        Err(error) => {
            tracing::warn!(%error, "Linear Webhook Inbox insert failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Linear Webhook persistence unavailable",
            );
        }
    };
    if let Err(error) = linear_q::mark_webhook_accepted(&mut transaction, connection.id).await {
        tracing::warn!(%error, "Linear Webhook health update failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Linear Webhook persistence unavailable",
        );
    }
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, "Linear Webhook transaction commit failed");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Linear Webhook persistence unavailable",
        );
    }
    (
        StatusCode::OK,
        Json(json!({ "accepted": true, "duplicate": !inserted })),
    )
        .into_response()
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| configured_value(Some(value)))
}

fn verify_signature(secret: &str, signature: &str, body: &[u8]) -> bool {
    let Ok(signature) = hex::decode(signature) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&signature).is_ok()
}

fn timestamp_is_fresh(timestamp_ms: i64, now_ms: i64) -> bool {
    (i128::from(now_ms) - i128::from(timestamp_ms)).abs() <= WEBHOOK_MAX_AGE_MS
}

fn current_time_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

fn random_token(size: usize) -> String {
    let mut bytes = vec![0_u8; size];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn parse_scopes(scope: &str) -> Value {
    Value::Array(
        scope
            .split(|character: char| character == ',' || character.is_ascii_whitespace())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| Value::String(value.to_string()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn signed_headers(secret: &str, body: &[u8], timestamp: Option<i64>) -> HeaderMap {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let mut headers = HeaderMap::new();
        headers.insert(
            "linear-signature",
            HeaderValue::from_str(&hex::encode(mac.finalize().into_bytes())).unwrap(),
        );
        headers.insert("linear-delivery", HeaderValue::from_static("delivery-1"));
        if let Some(timestamp) = timestamp {
            headers.insert(
                "linear-timestamp",
                HeaderValue::from_str(&timestamp.to_string()).unwrap(),
            );
        }
        headers
    }

    #[test]
    fn official_token_fixture_does_not_require_installation_fields() {
        let token: LinearTokenResponse = serde_json::from_value(json!({
            "access_token": "access",
            "refresh_token": "refresh",
            "expires_in": 86399,
            "scope": "read write"
        }))
        .expect("documented token response should deserialize");
        assert_eq!(token.access_token, "access");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh"));
    }

    #[test]
    fn scopes_accept_documented_space_format_and_authorization_comma_format() {
        assert_eq!(parse_scopes("read write"), json!(["read", "write"]));
        assert_eq!(
            parse_scopes("read,write,issues:create,app:assignable"),
            json!(["read", "write", "issues:create", "app:assignable"])
        );
    }

    #[test]
    fn authorization_url_uses_pkce_app_actor_and_comma_scopes() {
        let authorization_url = build_authorization_url(
            "https://linear.example/oauth/authorize?existing=1",
            "client-1",
            "https://api.example/api/linear/oauth/callback",
            "state-1",
            "verifier-1",
        )
        .unwrap();
        let url = Url::parse(&authorization_url).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("existing").map(String::as_str), Some("1"));
        assert_eq!(query.get("client_id").map(String::as_str), Some("client-1"));
        assert_eq!(
            query.get("redirect_uri").map(String::as_str),
            Some("https://api.example/api/linear/oauth/callback")
        );
        assert_eq!(query.get("actor").map(String::as_str), Some("app"));
        assert_eq!(
            query.get("scope").map(String::as_str),
            Some("read,write,issues:create,app:assignable")
        );
        assert_eq!(
            query.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        let expected_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(b"verifier-1"));
        assert_eq!(
            query.get("code_challenge").map(String::as_str),
            Some(expected_challenge.as_str())
        );
    }

    #[test]
    fn webhook_freshness_uses_milliseconds_and_requires_the_sixty_second_window() {
        assert!(timestamp_is_fresh(
            1_700_000_000_000,
            1_700_000_000_000 + 60_000
        ));
        assert!(!timestamp_is_fresh(
            1_700_000_000_000,
            1_700_000_000_000 + 60_001
        ));
    }

    #[test]
    fn webhook_signature_is_over_the_raw_body() {
        let secret = "webhook-secret";
        let body = br#"{"organizationId":"org"}"#;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let signature = hex::encode(mac.finalize().into_bytes());
        assert!(verify_signature(secret, &signature, body));
        assert!(!verify_signature(
            secret,
            &signature,
            br#"{"organizationId":"other"}"#
        ));
    }

    #[test]
    fn valid_webhook_requires_and_preserves_millisecond_timestamp() {
        let secret = "webhook-secret";
        let timestamp = 1_700_000_000_000;
        let body = br#"{"organizationId":"org-1","webhookId":"webhook-1","webhookTimestamp":1700000000000,"type":"Issue"}"#;
        let headers = signed_headers(secret, body, Some(timestamp));
        let webhook = validate_webhook(Some(secret), &headers, body, timestamp + 1).unwrap();
        assert_eq!(webhook.organization_id, "org-1");
        assert_eq!(webhook.webhook_id, "webhook-1");
        assert_eq!(webhook.delivery_id, "delivery-1");
        assert_eq!(webhook.event_type, "Issue");
    }

    #[test]
    fn webhook_without_timestamp_is_rejected_even_with_a_valid_signature() {
        let secret = "webhook-secret";
        let body = br#"{"organizationId":"org-1","webhookId":"webhook-1"}"#;
        let headers = signed_headers(secret, body, None);
        assert_eq!(
            validate_webhook(Some(secret), &headers, body, 1_700_000_000_000),
            Err(WebhookValidationError::MissingTimestamp)
        );
    }

    #[test]
    fn expired_webhook_is_rejected_after_sixty_seconds() {
        let secret = "webhook-secret";
        let timestamp = 1_700_000_000_000;
        let body = br#"{"organizationId":"org-1","webhookId":"webhook-1","webhookTimestamp":1700000000000}"#;
        let headers = signed_headers(secret, body, Some(timestamp));
        assert_eq!(
            validate_webhook(Some(secret), &headers, body, timestamp + 60_001),
            Err(WebhookValidationError::ExpiredTimestamp)
        );
    }

    #[test]
    fn webhook_timestamp_header_must_match_the_signed_body() {
        let secret = "webhook-secret";
        let timestamp = 1_700_000_000_000;
        let body = br#"{"organizationId":"org-1","webhookId":"webhook-1","webhookTimestamp":1700000000000}"#;
        let headers = signed_headers(secret, body, Some(timestamp + 1));
        assert_eq!(
            validate_webhook(Some(secret), &headers, body, timestamp),
            Err(WebhookValidationError::TimestampMismatch)
        );
    }
}
