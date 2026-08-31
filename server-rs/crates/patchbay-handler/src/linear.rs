//! Linear OAuth, project-scoped bindings, and durable sync edges.
//! Network work is kept outside database transactions; inbox/outbox rows are
//! the recovery boundary for retries and partial GraphQL responses.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use patchbay_db::models::{Issue, LinearConnection};
use patchbay_db::queries::{agent, linear as linear_q};
use patchbay_middleware::workspace::WorkspaceContext;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use url::Url;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

type HmacSha256 = Hmac<Sha256>;
const LINEAR_AUTH_URL: &str = "https://linear.app/oauth/authorize";
const LINEAR_TOKEN_URL: &str = "https://api.linear.app/oauth/token";
const LINEAR_GRAPHQL_URL: &str = "https://api.linear.app/graphql";

pub fn member_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/workspaces/{id}/linear", get(get_connection))
        .route("/api/workspaces/{id}/linear/project-bindings", get(list_project_bindings))
        .route(
            "/api/workspaces/{id}/linear/project-bindings/{binding_id}/status-bindings",
            get(list_status_bindings),
        )
        .route("/api/workspaces/{id}/linear/member-bindings", get(list_member_bindings))
        .route("/api/workspaces/{id}/linear/agent-bindings", get(list_agent_bindings))
        .route("/api/workspaces/{id}/linear/conflicts", get(list_conflicts))
        .route(
            "/api/workspaces/{id}/linear/issues/{issue_id}/relations",
            get(list_issue_relations),
        )
}

pub fn admin_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/workspaces/{id}/linear/connect", post(start_oauth))
        .route("/api/workspaces/{id}/linear/project-bindings", post(create_project_binding))
        .route(
            "/api/workspaces/{id}/linear/project-bindings/{binding_id}/status-bindings",
            put(upsert_status_binding),
        )
        .route("/api/workspaces/{id}/linear/project-bindings/{binding_id}/tombstone", post(tombstone_project_binding))
        .route("/api/workspaces/{id}/linear/member-bindings", post(bind_member))
        .route("/api/workspaces/{id}/linear/agent-bindings", post(bind_agent))
        .route(
            "/api/workspaces/{id}/linear/issues/{issue_id}/relations",
            put(upsert_issue_relation),
        )
}

pub fn public_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/linear/oauth/callback", get(oauth_callback))
        .route("/api/webhooks/linear/{connection_id}", post(linear_webhook))
        .route("/api/webhooks/linear/{connection_id}/", post(linear_webhook))
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace id"))
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn seal(state: &HandlerState, plaintext: &str) -> anyhow::Result<String> {
    let secret_box = state.vcs_secret_box.as_ref().ok_or_else(|| anyhow::anyhow!("encrypted secret storage is not configured"))?;
    Ok(STANDARD.encode(secret_box.seal(plaintext.as_bytes())?))
}

fn open(state: &HandlerState, ciphertext: &str) -> anyhow::Result<String> {
    let secret_box = state.vcs_secret_box.as_ref().ok_or_else(|| anyhow::anyhow!("encrypted secret storage is not configured"))?;
    Ok(String::from_utf8(secret_box.open(&STANDARD.decode(ciphertext)?)?)?)
}

fn random_token(size: usize) -> String {
    let mut bytes = vec![0_u8; size];
    OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn redirect_uri() -> String {
    env_value("PATCHBAY_LINEAR_REDIRECT_URI").unwrap_or_else(|| {
        format!("{}/api/linear/oauth/callback", env_value("PATCHBAY_PUBLIC_URL").unwrap_or_default().trim_end_matches('/'))
    })
}

fn connection_json(connection: LinearConnection) -> Value {
    json!({
        "id": connection.id,
        "workspace_id": connection.workspace_id,
        "organization_id": connection.organization_id,
        "organization_name": connection.organization_name,
        "actor_id": connection.actor_id,
        "scopes": connection.scopes,
        "status": connection.status,
        "token_expires_at": connection.token_expires_at,
        "created_at": connection.created_at,
        "updated_at": connection.updated_at,
    })
}

async fn get_connection(State(state): State<HandlerState>, Extension(context): Extension<WorkspaceContext>) -> Response {
    let workspace_id = match workspace_id(&context) { Ok(id) => id, Err(response) => return response };
    match linear_q::get_connection_for_workspace(&state.pool, workspace_id).await {
        Ok(Some(connection)) => Json(json!({
            "connected": true,
            "connection": connection_json(connection),
            "project_bindings": linear_q::list_project_bindings(&state.pool, workspace_id).await.unwrap_or_default(),
        })).into_response(),
        Ok(None) => Json(json!({"connected": false, "connection": Value::Null, "project_bindings": []})).into_response(),
        Err(error) => { tracing::warn!(%error, "failed to load Linear connection"); error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load Linear connection") }
    }
}

async fn start_oauth(State(state): State<HandlerState>, Extension(context): Extension<WorkspaceContext>) -> Response {
    let Some(client_id) = env_value("PATCHBAY_LINEAR_CLIENT_ID") else { return error_response(StatusCode::NOT_FOUND, "Linear OAuth is not configured") };
    if state.vcs_secret_box.is_none() { return error_response(StatusCode::SERVICE_UNAVAILABLE, "Linear OAuth requires encrypted secret storage") }
    let workspace_id = match workspace_id(&context) { Ok(id) => id, Err(response) => return response };
    let state_token = random_token(32);
    let verifier = random_token(48);
    let verifier_encrypted = match seal(&state, &verifier) { Ok(v) => v, Err(_) => return error_response(StatusCode::SERVICE_UNAVAILABLE, "Linear OAuth secret storage unavailable") };
    let redirect = redirect_uri();
    let expires_at = Utc::now() + Duration::minutes(10);
    let state_hash = sha256_hex(&state_token);
    if let Err(error) = linear_q::insert_oauth_state(&state.pool, &linear_q::OAuthStateInput {
        id: Uuid::now_v7(), state_hash: &state_hash, workspace_id, user_id: context.member.user_id,
        code_verifier_encrypted: &verifier_encrypted, redirect_uri: &redirect, expires_at,
    }).await {
        tracing::warn!(%error, "failed to persist Linear OAuth state");
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to prepare Linear OAuth");
    }
    let mut url = match Url::parse(&env_value("PATCHBAY_LINEAR_AUTH_URL").unwrap_or_else(|| LINEAR_AUTH_URL.into())) {
        Ok(url) => url, Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "invalid Linear OAuth URL"),
    };
    url.query_pairs_mut()
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", &redirect)
        .append_pair("response_type", "code")
        .append_pair("state", &state_token)
        .append_pair("code_challenge", &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())))
        .append_pair("code_challenge_method", "S256")
        .append_pair("scope", "read write");
    Json(json!({"authorization_url": url.to_string(), "state_expires_at": expires_at})).into_response()
}

#[derive(Debug, Deserialize)]
struct OAuthCallbackQuery { code: Option<String>, state: Option<String>, error: Option<String> }

#[derive(Debug, Deserialize)]
struct LinearTokenResponse {
    access_token: String,
    #[serde(default)] refresh_token: String,
    #[serde(default)] expires_in: Option<i64>,
    #[serde(default)] scope: Option<String>,
    #[serde(default)] organization_id: Option<String>,
    #[serde(default)] organization_name: Option<String>,
    #[serde(default)] actor_id: Option<String>,
}

async fn oauth_callback(State(state): State<HandlerState>, Query(query): Query<OAuthCallbackQuery>) -> Response {
    if let Some(error) = query.error { return error_response(StatusCode::BAD_REQUEST, &format!("Linear OAuth denied: {error}")) }
    let Some(state_token) = query.state.filter(|v| !v.trim().is_empty()) else { return error_response(StatusCode::BAD_REQUEST, "missing OAuth state") };
    let Some(code) = query.code.filter(|v| !v.trim().is_empty()) else { return error_response(StatusCode::BAD_REQUEST, "missing OAuth code") };
    let mut tx = match state.pool.begin().await { Ok(tx) => tx, Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to start OAuth transaction") };
    let oauth_state = match linear_q::consume_oauth_state(&mut *tx, &sha256_hex(&state_token)).await {
        Ok(Some(row)) => row,
        Ok(None) => return error_response(StatusCode::BAD_REQUEST, "OAuth state is invalid or expired"),
        Err(error) => { tracing::warn!(%error, "failed to consume Linear OAuth state"); return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to consume OAuth state") }
    };
    // Consume the one-time state before leaving the database. The token
    // exchange is network I/O and must never hold a PostgreSQL transaction or
    // leave a failed exchange able to replay the same state.
    if tx.commit().await.is_err() {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to consume OAuth state");
    }
    let verifier = match open(&state, &oauth_state.code_verifier_encrypted) { Ok(v) => v, Err(_) => return error_response(StatusCode::SERVICE_UNAVAILABLE, "Linear OAuth secret storage unavailable") };
    let token = match exchange_code(&code, &verifier, &oauth_state.redirect_uri).await {
        Ok(token) => token,
        Err(error) => { tracing::warn!(%error, "Linear OAuth token exchange failed"); return error_response(StatusCode::BAD_GATEWAY, "Linear OAuth token exchange failed") }
    };
    if token.refresh_token.trim().is_empty() { return error_response(StatusCode::BAD_GATEWAY, "Linear OAuth did not return a refresh token") }
    let Some(organization_id) = token.organization_id.or_else(|| env_value("PATCHBAY_LINEAR_ORGANIZATION_ID")) else {
        return error_response(StatusCode::BAD_GATEWAY, "Linear OAuth response did not identify an organization")
    };
    let access = match seal(&state, &token.access_token) { Ok(v) => v, Err(_) => return error_response(StatusCode::SERVICE_UNAVAILABLE, "Linear secret storage unavailable") };
    let refresh = match seal(&state, &token.refresh_token) { Ok(v) => v, Err(_) => return error_response(StatusCode::SERVICE_UNAVAILABLE, "Linear secret storage unavailable") };
    let scopes = token.scope.as_deref().map(|s| json!(s.split_whitespace().collect::<Vec<_>>())).unwrap_or_else(|| json!([]));
    let connection = match linear_q::upsert_connection(&state.pool, &linear_q::LinearConnectionInput {
        id: Uuid::now_v7(), workspace_id: oauth_state.workspace_id, organization_id: &organization_id,
        organization_name: token.organization_name.as_deref(), actor_id: token.actor_id.as_deref(),
        access_token_encrypted: &access, refresh_token_encrypted: &refresh,
        token_expires_at: token.expires_in.map(|v| Utc::now() + Duration::seconds(v)),
        scopes: &scopes, created_by_id: oauth_state.user_id,
    }).await {
        Ok(connection) => connection,
        Err(error) => { tracing::warn!(%error, "failed to save Linear connection"); return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to save Linear connection") }
    };
    Json(json!({"connected": true, "connection": connection_json(connection)})).into_response()
}

async fn exchange_code(code: &str, verifier: &str, redirect: &str) -> anyhow::Result<LinearTokenResponse> {
    let client_id = env_value("PATCHBAY_LINEAR_CLIENT_ID").ok_or_else(|| anyhow::anyhow!("Linear client id missing"))?;
    let client_secret = env_value("PATCHBAY_LINEAR_CLIENT_SECRET").ok_or_else(|| anyhow::anyhow!("Linear client secret missing"))?;
    let response = reqwest::Client::new().post(env_value("PATCHBAY_LINEAR_TOKEN_URL").unwrap_or_else(|| LINEAR_TOKEN_URL.into()))
        .form(&[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", redirect), ("client_id", client_id.as_str()), ("client_secret", client_secret.as_str()), ("code_verifier", verifier)])
        .send().await?;
    let status = response.status();
    let payload: Value = response.json().await?;
    if !status.is_success() { anyhow::bail!("Linear token endpoint returned {status}") }
    Ok(serde_json::from_value(payload)?)
}

#[derive(Debug, Deserialize)]
struct ProjectBindingRequest {
    linear_project_id: String,
    patchbay_project_id: Option<String>,
    default_linear_team_id: Option<String>,
    #[serde(default = "default_sync_mode")] sync_mode: String,
}
fn default_sync_mode() -> String { "two_way".into() }

async fn list_project_bindings(State(state): State<HandlerState>, Extension(context): Extension<WorkspaceContext>) -> Response {
    let workspace_id = match workspace_id(&context) { Ok(id) => id, Err(response) => return response };
    match linear_q::list_project_bindings(&state.pool, workspace_id).await {
        Ok(bindings) => Json(json!({"bindings": bindings})).into_response(),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list project bindings"),
    }
}

async fn list_status_bindings(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, binding_id)): Path<(String, String)>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let Ok(binding_id) = Uuid::parse_str(&binding_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid project binding id");
    };
    match linear_q::get_project_binding(&state.pool, workspace_id, binding_id).await {
        Ok(Some(_)) => match linear_q::list_status_bindings(&state.pool, binding_id).await {
            Ok(bindings) => Json(json!({"bindings": bindings})).into_response(),
            Err(_) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list Linear status bindings",
            ),
        },
        Ok(None) => error_response(StatusCode::NOT_FOUND, "project binding not found"),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to validate project binding",
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusBindingRequest {
    patchbay_status: String,
    linear_status_id: String,
}

async fn upsert_status_binding(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, binding_id)): Path<(String, String)>,
    Json(request): Json<StatusBindingRequest>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let Ok(binding_id) = Uuid::parse_str(&binding_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid project binding id");
    };
    if request.patchbay_status.trim().is_empty() || request.linear_status_id.trim().is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "patchbay_status and linear_status_id are required",
        );
    }
    match linear_q::get_project_binding(&state.pool, workspace_id, binding_id).await {
        Ok(Some(_)) => match linear_q::upsert_status_binding(
            &state.pool,
            &linear_q::StatusBindingInput {
                id: Uuid::now_v7(),
                project_binding_id: binding_id,
                patchbay_status: request.patchbay_status.trim().to_string(),
                linear_status_id: request.linear_status_id.trim().to_string(),
            },
        )
        .await
        {
            Ok(binding) => Json(json!({"binding": binding})).into_response(),
            Err(_) => error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to save Linear status binding",
            ),
        },
        Ok(None) => error_response(StatusCode::NOT_FOUND, "project binding not found"),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to validate project binding",
        ),
    }
}

async fn list_issue_relations(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, issue_id)): Path<(String, String)>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let Ok(issue_id) = Uuid::parse_str(&issue_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid issue id");
    };
    match linear_q::list_relation_links(&state.pool, workspace_id, issue_id).await {
        Ok(relations) => Json(json!({"relations": relations})).into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list Linear relations",
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelationRequest {
    to_issue_id: String,
    relation_type: String,
    linear_relation_id: Option<String>,
    #[serde(default = "default_active")]
    status: String,
}

fn default_active() -> String {
    "active".into()
}

async fn upsert_issue_relation(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, issue_id)): Path<(String, String)>,
    Json(request): Json<RelationRequest>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let Ok(from_issue_id) = Uuid::parse_str(&issue_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid issue id");
    };
    let Ok(to_issue_id) = Uuid::parse_str(&request.to_issue_id) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid to_issue_id");
    };
    if from_issue_id == to_issue_id
        || !matches!(request.relation_type.as_str(), "parent" | "blocks" | "blocked_by")
        || !matches!(request.status.as_str(), "active" | "conflict" | "tombstone")
    {
        return error_response(StatusCode::BAD_REQUEST, "invalid Linear relation");
    }
    match linear_q::upsert_relation_link(
        &state.pool,
        &linear_q::RelationLinkInput {
            id: Uuid::now_v7(),
            workspace_id,
            from_issue_id,
            to_issue_id,
            linear_relation_id: request.linear_relation_id,
            relation_type: request.relation_type,
            status: request.status,
        },
    )
    .await
    {
        Ok(relation) => Json(json!({"relation": relation})).into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to save Linear relation",
        ),
    }
}

async fn create_project_binding(State(state): State<HandlerState>, Extension(context): Extension<WorkspaceContext>, Json(request): Json<ProjectBindingRequest>) -> Response {
    let workspace_id = match workspace_id(&context) { Ok(id) => id, Err(response) => return response };
    if request.linear_project_id.trim().is_empty() || !matches!(request.sync_mode.as_str(), "two_way" | "pull_only" | "push_only") {
        return error_response(StatusCode::BAD_REQUEST, "invalid Linear project binding");
    }
    let patchbay_project_id = match request.patchbay_project_id.as_deref() {
        Some(raw) => match Uuid::parse_str(raw) {
            Ok(id) => match patchbay_db::queries::project::get_project_in_workspace(&state.pool, id, workspace_id).await {
                Ok(Some(_)) => Some(id), Ok(None) => return error_response(StatusCode::BAD_REQUEST, "project not found in this workspace"), Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to validate project"),
            },
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid patchbay_project_id"),
        },
        None => None,
    };
    let Some(connection) = linear_q::get_connection_for_workspace(&state.pool, workspace_id).await.ok().flatten() else { return error_response(StatusCode::CONFLICT, "connect Linear before binding projects") };
    match linear_q::upsert_project_binding(&state.pool, &linear_q::ProjectBindingInput {
        id: Uuid::now_v7(), workspace_id, connection_id: connection.id, patchbay_project_id,
        linear_project_id: request.linear_project_id.trim().into(), default_linear_team_id: request.default_linear_team_id, sync_mode: request.sync_mode,
    }).await {
        Ok(binding) => (StatusCode::CREATED, Json(json!({"binding": binding}))).into_response(),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to save project binding"),
    }
}

async fn tombstone_project_binding(State(state): State<HandlerState>, Extension(context): Extension<WorkspaceContext>, Path((_workspace, binding_id)): Path<(String, String)>) -> Response {
    let workspace_id = match workspace_id(&context) { Ok(id) => id, Err(response) => return response };
    let Ok(binding_id) = Uuid::parse_str(&binding_id) else { return error_response(StatusCode::BAD_REQUEST, "invalid binding id") };
    match sqlx::query("UPDATE linear_project_binding SET status = 'tombstone', updated_at = now() WHERE id = $1 AND workspace_id = $2 RETURNING id").bind(binding_id).bind(workspace_id).fetch_optional(&state.pool).await {
        Ok(Some(_)) => Json(json!({"status": "tombstone"})).into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "project binding not found"),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update project binding"),
    }
}

async fn list_member_bindings(State(state): State<HandlerState>, Extension(context): Extension<WorkspaceContext>) -> Response {
    let workspace_id = match workspace_id(&context) { Ok(id) => id, Err(response) => return response };
    match sqlx::query_as::<_, patchbay_db::models::LinearMemberBinding>("SELECT * FROM linear_member_binding WHERE workspace_id = $1 ORDER BY normalized_email, id").bind(workspace_id).fetch_all(&state.pool).await {
        Ok(bindings) => Json(json!({"bindings": bindings})).into_response(),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list member bindings"),
    }
}

async fn list_agent_bindings(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    match linear_q::list_agent_bindings(&state.pool, workspace_id).await {
        Ok(bindings) => Json(json!({"bindings": bindings})).into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list Linear agent bindings",
        ),
    }
}

async fn list_conflicts(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    match linear_q::list_open_conflicts(&state.pool, workspace_id, 100).await {
        Ok(conflicts) => Json(json!({"conflicts": conflicts})).into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list Linear conflicts",
        ),
    }
}

#[derive(Debug, Deserialize)]
struct MemberBindingRequest { linear_user_id: String, email: String, #[serde(default = "default_true")] active: bool, #[serde(default = "default_human")] kind: String }
fn default_true() -> bool { true }
fn default_human() -> String { "human".into() }
fn normalize_email(email: &str) -> String { email.trim().to_ascii_lowercase() }

async fn bind_member(State(state): State<HandlerState>, Extension(context): Extension<WorkspaceContext>, Json(request): Json<MemberBindingRequest>) -> Response {
    let workspace_id = match workspace_id(&context) { Ok(id) => id, Err(response) => return response };
    let email = normalize_email(&request.email);
    if request.linear_user_id.trim().is_empty() || email.is_empty() { return error_response(StatusCode::BAD_REQUEST, "linear_user_id and email are required") }
    if !request.active || request.kind != "human" { return Json(json!({"bound": false, "status": "unbound", "diagnostic": "only active Linear humans can be bound"})).into_response() }
    let rows = match sqlx::query(r#"SELECT m.id FROM member m JOIN "user" u ON u.id = m.user_id WHERE m.workspace_id = $1 AND NOT u.is_guest AND lower(trim(u.email)) = $2"#).bind(workspace_id).bind(&email).fetch_all(&state.pool).await {
        Ok(rows) => rows, Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to match member email"),
    };
    if rows.len() != 1 { return Json(json!({"bound": false, "status": "diagnostic", "diagnostic": if rows.is_empty() { "no unique Patchbay member matched this email" } else { "multiple Patchbay members matched this email" }})).into_response() }
    if linear_q::get_connection_for_workspace(&state.pool, workspace_id).await.ok().flatten().is_none() { return error_response(StatusCode::CONFLICT, "connect Linear before binding members") }
    let member_id: Uuid = match sqlx::Row::try_get(&rows[0], 0) { Ok(id) => id, Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "invalid member row") };
    match sqlx::query_as::<_, patchbay_db::models::LinearMemberBinding>(r#"INSERT INTO linear_member_binding (id,workspace_id,member_id,linear_user_id,normalized_email,status) VALUES ($1,$2,$3,$4,$5,'active') ON CONFLICT (workspace_id,linear_user_id) DO UPDATE SET member_id = EXCLUDED.member_id, normalized_email = EXCLUDED.normalized_email, status = 'active', diagnostic = NULL, updated_at = now() RETURNING *"#).bind(Uuid::now_v7()).bind(workspace_id).bind(member_id).bind(request.linear_user_id.trim()).bind(&email).fetch_one(&state.pool).await {
        Ok(binding) => Json(json!({"bound": true, "binding": binding})).into_response(),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to save member binding"),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentBindingRequest {
    agent_id: String,
    linear_label_group_id: String,
    linear_label_id: String,
    label_name: String,
}

async fn bind_agent(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Json(request): Json<AgentBindingRequest>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let Ok(agent_id) = Uuid::parse_str(request.agent_id.trim()) else {
        return error_response(StatusCode::BAD_REQUEST, "invalid agent_id");
    };
    if request.linear_label_group_id.trim().is_empty()
        || request.linear_label_id.trim().is_empty()
        || request.label_name.trim().is_empty()
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "linear label group, label id, and label name are required",
        );
    }
    if linear_q::get_connection_for_workspace(&state.pool, workspace_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return error_response(StatusCode::CONFLICT, "connect Linear before binding agents");
    }
    match agent::get_agent_in_workspace(&state.pool, agent_id, workspace_id).await {
        Ok(Some(target)) if target.archived_at.is_none() => {}
        Ok(Some(_)) => return error_response(StatusCode::BAD_REQUEST, "agent is archived"),
        Ok(None) => return error_response(StatusCode::BAD_REQUEST, "agent not found in this workspace"),
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to validate agent"),
    }
    match linear_q::upsert_agent_binding(
        &state.pool,
        &linear_q::AgentBindingInput {
            id: Uuid::now_v7(),
            workspace_id,
            agent_id,
            linear_label_group_id: request.linear_label_group_id.trim().to_string(),
            linear_label_id: request.linear_label_id.trim().to_string(),
            label_name: request.label_name.trim().to_string(),
        },
    )
    .await
    {
        Ok(binding) => Json(json!({"binding": binding})).into_response(),
        Err(_) => error_response(
            StatusCode::CONFLICT,
            "Linear label is already bound to another Patchbay agent",
        ),
    }
}

async fn linear_webhook(State(state): State<HandlerState>, Path(connection_id): Path<String>, headers: HeaderMap, body: Bytes) -> Response {
    let Ok(connection_id) = Uuid::parse_str(&connection_id) else { return error_response(StatusCode::BAD_REQUEST, "invalid Linear connection id") };
    let Some(secret) = env_value("PATCHBAY_LINEAR_WEBHOOK_SECRET") else { return error_response(StatusCode::SERVICE_UNAVAILABLE, "Linear webhook secret is not configured") };
    let signature = headers.get("linear-signature").or_else(|| headers.get("x-linear-signature")).and_then(|v| v.to_str().ok()).unwrap_or_default();
    if let Some(timestamp) = headers.get("linear-timestamp").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<i64>().ok()) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        if (now - timestamp).abs() > 300 { return error_response(StatusCode::UNAUTHORIZED, "stale Linear webhook") }
    }
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key");
    mac.update(&body);
    let expected = mac.finalize().into_bytes();
    let supplied = signature.strip_prefix("sha256=").unwrap_or(signature);
    let supplied = hex::decode(supplied).or_else(|_| STANDARD.decode(supplied)).unwrap_or_default();
    if supplied.len() != expected.len() || !constant_time_eq(&supplied, &expected) { return error_response(StatusCode::UNAUTHORIZED, "invalid Linear webhook signature") }
    let payload: Value = match serde_json::from_slice(&body) { Ok(value) => value, Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid Linear webhook body") };
    let delivery_id = headers.get("linear-delivery").or_else(|| headers.get("x-linear-delivery")).and_then(|v| v.to_str().ok()).filter(|v| !v.trim().is_empty()).map(str::to_string).unwrap_or_else(|| sha256_hex(std::str::from_utf8(&body).unwrap_or_default()));
    let event_type = headers.get("linear-event").or_else(|| headers.get("x-linear-event")).and_then(|v| v.to_str().ok()).unwrap_or_else(|| payload.get("type").and_then(Value::as_str).unwrap_or("unknown"));
    let exists = sqlx::query_scalar::<_, Uuid>("SELECT id FROM linear_connection WHERE id = $1 AND status = 'active'").bind(connection_id).fetch_optional(&state.pool).await;
    let Some(connection_id) = (match exists {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load Linear connection"),
    }) else {
        return error_response(StatusCode::NOT_FOUND, "Linear connection not found");
    };
    match linear_q::insert_sync_inbox(&state.pool, Uuid::now_v7(), connection_id, &delivery_id, event_type, &payload).await {
        Ok(inserted) => (StatusCode::ACCEPTED, Json(json!({"accepted": true, "duplicate": !inserted}))).into_response(),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to persist Linear webhook"),
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = (left.len() ^ right.len()) as u8;
    for i in 0..left.len().max(right.len()) { diff |= left.get(i).copied().unwrap_or_default() ^ right.get(i).copied().unwrap_or_default(); }
    diff == 0
}

pub async fn enqueue_issue_outbox_tx(executor: &mut PgConnection, previous: &Issue, updated: &Issue, correlation_id: Uuid) -> anyhow::Result<()> {
    let row = sqlx::query(r#"SELECT c.id FROM linear_connection c JOIN linear_issue_link l ON l.workspace_id = c.workspace_id AND l.issue_id = $1 AND l.status = 'active' WHERE c.workspace_id = $2 AND c.status = 'active'"#).bind(updated.id).bind(updated.workspace_id).fetch_optional(&mut *executor).await?;
    let Some(row) = row else { return Ok(()) };
    let connection_id: Uuid = sqlx::Row::try_get(&row, 0)?;
    let managed_description = patchbay_service::linear_sync::merge_managed_description(
        updated.description.as_deref().unwrap_or_default(),
        &patchbay_service::linear_sync::managed_description_block(
            &updated.acceptance_criteria,
            updated
                .metadata
                .get("orchestration_summary")
                .and_then(Value::as_str),
        ),
    );
    let payload = json!({
        "issue": updated,
        "previous": previous,
        "revision": updated.revision,
        "managed_description": managed_description,
    });
    linear_q::enqueue_sync_outbox(&mut *executor, &linear_q::SyncOutboxInput { id: Uuid::now_v7(), workspace_id: updated.workspace_id, connection_id, issue_id: Some(updated.id), correlation_id, operation: "issue.update", payload: &payload }).await?;
    Ok(())
}

pub async fn graphql_request(access_token: &str, query: &str, variables: Value) -> anyhow::Result<Value> {
    let response = reqwest::Client::new().post(env_value("PATCHBAY_LINEAR_GRAPHQL_URL").unwrap_or_else(|| LINEAR_GRAPHQL_URL.into())).bearer_auth(access_token).json(&json!({"query": query, "variables": variables})).send().await?;
    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() || body.get("errors").and_then(Value::as_array).is_some_and(|errors| !errors.is_empty()) { anyhow::bail!("Linear GraphQL request failed") }
    let data = body.get("data").cloned().unwrap_or(Value::Null);
    if data.is_null() { anyhow::bail!("Linear GraphQL returned no data") }
    Ok(data)
}
