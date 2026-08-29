//! Workspace connector installation management. Secret-bearing writes are
//! gated on a valid per-provider secretbox key and never return stored config.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use patchbay_db::models::ChannelInstallation;
use patchbay_db::queries::{agent, channel, dingtalk};
use patchbay_lark::client::ApiClient as _;
use patchbay_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    error::{error_code_response, error_response},
    state::HandlerState,
};

const BODY_LIMIT: usize = 16 * 1024;

#[derive(Clone, Copy)]
enum Provider {
    DingTalk,
    Lark,
    Slack,
    Telegram,
    WeCom,
    Weixin,
}

impl Provider {
    fn channel_type(self) -> &'static str {
        match self {
            Self::DingTalk => patchbay_dingtalk::TYPE_DINGTALK,
            Self::Lark => patchbay_lark::channel_store::CHANNEL_TYPE_FEISHU,
            Self::Slack => patchbay_slack::TYPE_SLACK,
            Self::Telegram => patchbay_telegram::TYPE_TELEGRAM,
            Self::WeCom => patchbay_wecom::CHANNEL_TYPE_WECOM,
            Self::Weixin => patchbay_weixin::TYPE_WEIXIN,
        }
    }

    fn key_env(self) -> &'static str {
        match self {
            Self::DingTalk => "PATCHBAY_DINGTALK_SECRET_KEY",
            Self::Lark => "PATCHBAY_LARK_SECRET_KEY",
            Self::Slack => "PATCHBAY_SLACK_SECRET_KEY",
            Self::Telegram => "PATCHBAY_TELEGRAM_SECRET_KEY",
            Self::WeCom => "PATCHBAY_WECOM_SECRET_KEY",
            Self::Weixin => "PATCHBAY_WEIXIN_SECRET_KEY",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::DingTalk => "dingtalk",
            Self::Lark => "lark",
            Self::Slack => "slack",
            Self::Telegram => "telegram",
            Self::WeCom => "wecom",
            Self::Weixin => "weixin",
        }
    }

    fn created_event(self) -> &'static str {
        match self {
            Self::DingTalk => patchbay_protocol::EVENT_DINGTALK_INSTALLATION_CREATED,
            Self::Lark => patchbay_protocol::EVENT_LARK_INSTALLATION_CREATED,
            Self::Slack => patchbay_protocol::EVENT_SLACK_INSTALLATION_CREATED,
            Self::Telegram => patchbay_protocol::EVENT_TELEGRAM_INSTALLATION_CREATED,
            Self::WeCom => patchbay_protocol::EVENT_WECOM_INSTALLATION_CREATED,
            Self::Weixin => patchbay_protocol::EVENT_WEIXIN_INSTALLATION_CREATED,
        }
    }

    fn revoked_event(self) -> &'static str {
        match self {
            Self::DingTalk => patchbay_protocol::EVENT_DINGTALK_INSTALLATION_REVOKED,
            Self::Lark => patchbay_protocol::EVENT_LARK_INSTALLATION_REVOKED,
            Self::Slack => patchbay_protocol::EVENT_SLACK_INSTALLATION_REVOKED,
            Self::Telegram => patchbay_protocol::EVENT_TELEGRAM_INSTALLATION_REVOKED,
            Self::WeCom => patchbay_protocol::EVENT_WECOM_INSTALLATION_REVOKED,
            Self::Weixin => patchbay_protocol::EVENT_WEIXIN_INSTALLATION_REVOKED,
        }
    }
}

pub fn member_router() -> Router<HandlerState> {
    Router::new()
        .route(
            "/api/workspaces/{id}/dingtalk/installations",
            get(list_dingtalk),
        )
        .route(
            "/api/workspaces/{id}/dingtalk/group-routes",
            get(list_dingtalk_group_routes),
        )
        .route("/api/workspaces/{id}/lark/installations", get(list_lark))
        .route(
            "/api/workspaces/{id}/lark/install/begin",
            post(begin_lark_install),
        )
        .route(
            "/api/workspaces/{id}/lark/install/{session_id}/status",
            get(lark_install_status),
        )
        .route(
            "/api/workspaces/{id}/lark/installations/{installation_id}",
            delete(revoke_lark),
        )
        .route("/api/workspaces/{id}/slack/installations", get(list_slack))
        .route(
            "/api/workspaces/{id}/telegram/installations",
            get(list_telegram),
        )
        .route("/api/workspaces/{id}/wecom/installations", get(list_wecom))
        .route(
            "/api/workspaces/{id}/weixin/installations",
            get(list_weixin),
        )
        .route(
            "/api/workspaces/{id}/weixin/install/{session_id}/status",
            get(weixin_install_status),
        )
        .route(
            "/api/workspaces/{id}/weixin/install/begin",
            post(begin_weixin_install),
        )
        .route(
            "/api/workspaces/{id}/weixin/installations/{installation_id}",
            delete(revoke_weixin),
        )
}

pub fn admin_router() -> Router<HandlerState> {
    Router::new()
        .route(
            "/api/workspaces/{id}/dingtalk/install/byo",
            post(install_dingtalk),
        )
        .route(
            "/api/workspaces/{id}/dingtalk/installations/{installation_id}",
            delete(revoke_dingtalk),
        )
        .route(
            "/api/workspaces/{id}/dingtalk/group-routes/{route_id}",
            axum::routing::patch(update_dingtalk_group_route),
        )
        .route(
            "/api/workspaces/{id}/slack/install/byo",
            post(install_slack),
        )
        .route(
            "/api/workspaces/{id}/slack/installations/{installation_id}",
            delete(revoke_slack),
        )
        .route(
            "/api/workspaces/{id}/telegram/install",
            post(install_telegram),
        )
        .route(
            "/api/workspaces/{id}/telegram/installations/{installation_id}",
            delete(revoke_telegram),
        )
        .route(
            "/api/workspaces/{id}/wecom/install/byo",
            post(install_wecom),
        )
        .route(
            "/api/workspaces/{id}/wecom/installations/{installation_id}",
            delete(revoke_wecom),
        )
}

fn secret_box(provider: Provider) -> Option<patchbay_util::secretbox::SecretBox> {
    let key = patchbay_util::secretbox::load_key(provider.key_env()).ok()?;
    patchbay_util::secretbox::SecretBox::new(&key).ok()
}

fn user_id(headers: &HeaderMap) -> Result<Uuid, Response> {
    headers
        .get("x-user-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| error_response(StatusCode::UNAUTHORIZED, "user not authenticated"))
}

/// External platform authorization is a formal-account operation. The guest
/// auth middleware marks the authenticated request with this server-owned
/// header; reject it at the backend boundary rather than relying on the
/// integrations page to hide its button.
fn require_formal_user(headers: &HeaderMap) -> Result<(), Response> {
    if headers
        .get("x-guest-user")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
    {
        return Err(error_code_response(
            StatusCode::FORBIDDEN,
            "login_required",
            "log in before connecting an external platform",
        ));
    }
    Ok(())
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace id"))
}

fn public_config(provider: Provider, config: &Value) -> Value {
    match provider {
        Provider::DingTalk => {
            json!({"app_id": config.get("app_id").and_then(Value::as_str).unwrap_or("")})
        }
        Provider::Lark => {
            let mut value = json!({
                "app_id": config.get("app_id").and_then(Value::as_str).unwrap_or(""),
                "bot_open_id": config.get("bot_open_id").and_then(Value::as_str).unwrap_or(""),
                "region": config.get("region").and_then(Value::as_str).unwrap_or("feishu"),
            });
            if let (Some(target), Some(tenant_key)) = (
                value.as_object_mut(),
                config.get("tenant_key").and_then(Value::as_str),
            ) {
                target.insert("tenant_key".into(), Value::String(tenant_key.into()));
            }
            value
        }
        Provider::Slack => {
            let value = patchbay_slack::config::decode_public_config(config);
            json!({"team_id": value.team_id, "bot_user_id": value.bot_user_id})
        }
        Provider::Telegram => {
            let raw = serde_json::to_vec(config).unwrap_or_default();
            let value = patchbay_telegram::decode_public_config(&raw);
            json!({"bot_id": value.bot_id, "bot_username": value.bot_username})
        }
        Provider::WeCom => {
            json!({"bot_id": config.get("bot_id").or_else(|| config.get("app_id")).and_then(Value::as_str).unwrap_or("")})
        }
        Provider::Weixin => {
            let value = patchbay_weixin::config::decode_public_config(config);
            json!({"bot_id": value.bot_id, "ilink_user_id": value.ilink_user_id})
        }
    }
}

fn installation_response(provider: Provider, row: ChannelInstallation) -> Value {
    let mut value = json!({
        "id": row.id,
        "workspace_id": row.workspace_id,
        "agent_id": row.agent_id,
        "installer_user_id": row.installer_user_id,
        "status": row.status,
        "installed_at": crate::timefmt::rfc3339(row.installed_at),
        "created_at": crate::timefmt::rfc3339(row.created_at),
        "updated_at": crate::timefmt::rfc3339(row.updated_at),
    });
    if let (Some(target), Some(fields)) = (
        value.as_object_mut(),
        public_config(provider, &row.config).as_object(),
    ) {
        target.extend(fields.clone());
    }
    value
}

async fn list(state: HandlerState, context: WorkspaceContext, provider: Provider) -> Response {
    if secret_box(provider).is_none() {
        return Json(json!({"installations": [], "configured": false, "install_supported": false}))
            .into_response();
    }
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match channel::list_channel_installations_by_workspace(
        &state.pool,
        workspace_id,
        provider.channel_type(),
    )
    .await
    {
        Ok(rows) => Json(json!({
            "installations": rows.into_iter().map(|row| installation_response(provider, row)).collect::<Vec<_>>(),
            "configured": true,
            "install_supported": true,
        }))
        .into_response(),
        Err(error) => {
            tracing::error!(%error, provider = provider.label(), "list connector installations failed");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list installations",
            )
        }
    }
}

macro_rules! list_handler {
    ($name:ident, $provider:expr) => {
        async fn $name(
            State(state): State<HandlerState>,
            Extension(context): Extension<WorkspaceContext>,
        ) -> Response {
            list(state, context, $provider).await
        }
    };
}
list_handler!(list_lark, Provider::Lark);
list_handler!(list_slack, Provider::Slack);
list_handler!(list_telegram, Provider::Telegram);
list_handler!(list_wecom, Provider::WeCom);
list_handler!(list_weixin, Provider::Weixin);

async fn list_dingtalk(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    if secret_box(Provider::DingTalk).is_none() {
        return Json(json!({
            "installations": [],
            "configured": false,
            "install_supported": false,
            "group_routing_supported": false,
        }))
        .into_response();
    }
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let rows = match channel::list_channel_installations_by_workspace(
        &state.pool,
        workspace_id,
        Provider::DingTalk.channel_type(),
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "list DingTalk installations failed");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list installations",
            );
        }
    };
    let bindings = if matches!(context.member.role.as_str(), "owner" | "admin") {
        match dingtalk::list_ding_talk_user_bindings_for_member(
            &state.pool,
            workspace_id,
            context.member.user_id,
        )
        .await
        {
            Ok(rows) => {
                let mut by_installation = HashMap::<Uuid, Vec<String>>::new();
                for row in rows {
                    if let Some(installation_id) = row.installation_id {
                        by_installation
                            .entry(installation_id)
                            .or_default()
                            .push(row.channel_user_id);
                    }
                }
                Some(by_installation)
            }
            Err(error) => {
                tracing::error!(%error, "list DingTalk member bindings failed");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to list dingtalk user bindings",
                );
            }
        }
    } else {
        None
    };
    let installations = rows
        .into_iter()
        .map(|row| dingtalk_installation_response(row, bindings.as_ref()))
        .collect::<Vec<_>>();
    Json(json!({
        "installations": installations,
        "configured": true,
        "install_supported": true,
        "group_routing_supported": true,
    }))
    .into_response()
}

fn dingtalk_installation_response(
    row: ChannelInstallation,
    bindings: Option<&HashMap<Uuid, Vec<String>>>,
) -> Value {
    let installation_id = row.id;
    dingtalk_installation_bindings(
        installation_response(Provider::DingTalk, row),
        installation_id,
        bindings,
    )
}

fn dingtalk_installation_bindings(
    mut value: Value,
    installation_id: Uuid,
    bindings: Option<&HashMap<Uuid, Vec<String>>>,
) -> Value {
    if let (Some(target), Some(bindings)) = (value.as_object_mut(), bindings) {
        target.insert(
            "bound_dingtalk_user_ids".into(),
            json!(bindings.get(&installation_id).cloned().unwrap_or_default()),
        );
    }
    value
}

const WEIXIN_SESSION_PREFIX: &str = "patchbay:{weixin_install_session}:";
const WEIXIN_SESSION_TTL: Duration = Duration::from_secs(5 * 60);
const WEIXIN_SESSION_STORE_TIMEOUT: Duration = Duration::from_millis(250);
const WEIXIN_SESSION_MEMORY_CAP: usize = 1024;

#[derive(Clone, Serialize, Deserialize)]
struct WeixinInstallSession {
    workspace_id: Uuid,
    agent_id: Uuid,
    initiator_id: Uuid,
    qrcode: String,
    base_url: String,
    status: String,
    installation_id: Option<Uuid>,
    error_message: Option<String>,
    expires_at: DateTime<Utc>,
}

#[derive(Clone)]
struct WeixinSessionStore {
    redis: Option<redis::Client>,
}

impl WeixinSessionStore {
    fn from_state(state: &HandlerState) -> Self {
        Self {
            redis: state.rate_limit_client.clone(),
        }
    }

    fn memory() -> &'static Mutex<HashMap<String, WeixinInstallSession>> {
        static SESSIONS: OnceLock<Mutex<HashMap<String, WeixinInstallSession>>> = OnceLock::new();
        SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn key(id: &str) -> String {
        format!("{WEIXIN_SESSION_PREFIX}{id}")
    }

    async fn put(&self, id: &str, value: &WeixinInstallSession) -> Result<(), &'static str> {
        if let Some(client) = &self.redis {
            let key = Self::key(id);
            let payload =
                serde_json::to_string(value).map_err(|_| "failed to store install session")?;
            let ttl = (value.expires_at - Utc::now()).num_seconds().max(1) as u64;
            let operation = async {
                let mut connection = client.get_multiplexed_async_connection().await?;
                redis::cmd("SET")
                    .arg(key)
                    .arg(payload)
                    .arg("EX")
                    .arg(ttl)
                    .query_async::<()>(&mut connection)
                    .await
            };
            return match tokio::time::timeout(WEIXIN_SESSION_STORE_TIMEOUT, operation).await {
                Ok(Ok(())) => Ok(()),
                _ => Err("failed to store install session"),
            };
        }
        let mut sessions = Self::memory().lock().unwrap();
        sessions.retain(|_, session| session.expires_at > Utc::now());
        if sessions.len() >= WEIXIN_SESSION_MEMORY_CAP && !sessions.contains_key(id) {
            return Err("too many install sessions");
        }
        sessions.insert(id.to_string(), value.clone());
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<WeixinInstallSession>, &'static str> {
        if let Some(client) = &self.redis {
            let key = Self::key(id);
            let operation = async {
                let mut connection = client.get_multiplexed_async_connection().await?;
                redis::cmd("GET")
                    .arg(key)
                    .query_async::<Option<String>>(&mut connection)
                    .await
            };
            return match tokio::time::timeout(WEIXIN_SESSION_STORE_TIMEOUT, operation).await {
                Ok(Ok(Some(payload))) => serde_json::from_str::<WeixinInstallSession>(&payload)
                    .map(Some)
                    .map_err(|_| "failed to load install session"),
                Ok(Ok(None)) => Ok(None),
                _ => Err("failed to load install session"),
            };
        }
        let mut sessions = Self::memory().lock().unwrap();
        sessions.retain(|_, session| session.expires_at > Utc::now());
        Ok(sessions.get(id).cloned())
    }
}

async fn begin_weixin_install(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Query(query): Query<AgentQuery>,
) -> Response {
    let Some(box_) = secret_box(Provider::Weixin) else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "weixin integration not configured",
        );
    };
    let (workspace_id, agent_id, actor) =
        match install_context(&state, &context, &headers, &query).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    if !agent_id.is_nil() {
        let target = match agent::get_agent_in_workspace(&state.pool, agent_id, workspace_id).await
        {
            Ok(Some(value)) => value,
            _ => return error_response(StatusCode::NOT_FOUND, "agent not found in this workspace"),
        };
        if !matches!(context.member.role.as_str(), "owner" | "admin")
            && target.owner_id != Some(actor)
        {
            return error_response(StatusCode::FORBIDDEN, "not allowed to manage this agent");
        }
    }
    // Passing the target agent's currently stored local token lets iLink
    // recognize a reconnect instead of returning `binded_redirect` with no
    // usable credentials. A different account still follows the normal scan.
    let installations = match channel::list_channel_installations_by_workspace(
        &state.pool,
        workspace_id,
        patchbay_weixin::TYPE_WEIXIN,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "failed to load existing WeChat installation");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to start WeChat authorization",
            );
        }
    };
    let decrypt = move |sealed: &[u8]| box_.open(sealed).map_err(anyhow::Error::from);
    let local_tokens = installations
        .into_iter()
        .filter(|row| agent_id.is_nil() || row.agent_id == Some(agent_id))
        .filter_map(|row| {
            patchbay_weixin::config::decode_credentials(&row.config, Some(&decrypt))
                .ok()
                .map(|credentials| credentials.bot_token)
        })
        .collect::<Vec<_>>();
    let qr = match patchbay_weixin::api::Client::request_qr_code(&local_tokens).await {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "failed to request WeChat QR code");
            return error_response(
                StatusCode::BAD_GATEWAY,
                "failed to start WeChat authorization",
            );
        }
    };
    let session_id = Uuid::new_v4().to_string();
    let qrcode = qr.qrcode;
    let expires_at = Utc::now() + chrono::Duration::seconds(WEIXIN_SESSION_TTL.as_secs() as i64);
    let session = WeixinInstallSession {
        workspace_id,
        agent_id,
        initiator_id: actor,
        qrcode: qrcode.clone(),
        base_url: patchbay_weixin::api::DEFAULT_BASE_URL.to_string(),
        status: "pending".into(),
        installation_id: None,
        error_message: None,
        expires_at,
    };
    if let Err(message) = WeixinSessionStore::from_state(&state)
        .put(&session_id, &session)
        .await
    {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, message);
    }
    Json(json!({
        "session_id": session_id,
        "qr_code_url": qrcode,
        "expires_in_seconds": WEIXIN_SESSION_TTL.as_secs(),
        "poll_interval_seconds": 2,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct WeixinStatusQuery {
    #[serde(default)]
    verify_code: String,
}

async fn weixin_install_status(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Path((_workspace, session_id)): Path<(String, String)>,
    Query(query): Query<WeixinStatusQuery>,
) -> Response {
    if let Err(response) = require_formal_user(&headers) {
        return response;
    }
    let actor = match user_id(&headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let store = WeixinSessionStore::from_state(&state);
    let mut session = match store.get(&session_id).await {
        Ok(Some(value)) if value.workspace_id == workspace_id && value.initiator_id == actor => {
            value
        }
        Ok(Some(_)) => {
            return error_response(StatusCode::FORBIDDEN, "install session is not yours")
        }
        Ok(None) => return error_response(StatusCode::GONE, "install session expired"),
        Err(message) => return error_response(StatusCode::SERVICE_UNAVAILABLE, message),
    };
    if let Some(id) = session.installation_id {
        return Json(json!({"status": "success", "installation_id": id})).into_response();
    }
    if session.expires_at <= Utc::now() {
        return Json(json!({"status": "expired"})).into_response();
    }
    let status = match patchbay_weixin::api::Client::qr_status(
        &session.base_url,
        &session.qrcode,
        (!query.verify_code.trim().is_empty()).then_some(query.verify_code.trim()),
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "failed to poll WeChat QR status");
            return Json(json!({"status": "pending"})).into_response();
        }
    };
    match status.status.as_str() {
        "wait" => return Json(json!({"status": "pending"})).into_response(),
        "scaned" | "scanned" => return Json(json!({"status": "scanned"})).into_response(),
        "need_verifycode" => return Json(json!({"status": "need_verify_code"})).into_response(),
        "verify_code_blocked" | "expired" => {
            return Json(json!({"status": "expired"})).into_response()
        }
        "scaned_but_redirect" | "scanned_but_redirect" => {
            let allowed = validate_weixin_redirect(&status.redirect_host);
            if let Some(base_url) = allowed {
                session.base_url = base_url;
                let _ = store.put(&session_id, &session).await;
                return Json(json!({"status": "scanned"})).into_response();
            }
            return error_response(
                StatusCode::BAD_GATEWAY,
                "WeChat returned an unsafe redirect host",
            );
        }
        "binded_redirect" => return Json(json!({"status": "already_connected"})).into_response(),
        "confirmed" => {}
        _ => return Json(json!({"status": "pending"})).into_response(),
    }
    if status.bot_token.is_empty()
        || status.ilink_bot_id.is_empty()
        || status.ilink_user_id.is_empty()
    {
        return error_response(
            StatusCode::BAD_GATEWAY,
            "WeChat confirmation was incomplete",
        );
    }
    let Some(box_) = secret_box(Provider::Weixin) else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "weixin integration not configured",
        );
    };
    let sealed = match box_.seal(status.bot_token.as_bytes()) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to encrypt WeChat token",
            )
        }
    };
    let base_url = validate_weixin_redirect(if status.baseurl.is_empty() {
        patchbay_weixin::api::DEFAULT_BASE_URL
    } else {
        &status.baseurl
    })
    .unwrap_or_else(|| patchbay_weixin::api::DEFAULT_BASE_URL.to_string());
    let config = json!({
        "app_id": status.ilink_bot_id,
        "ilink_user_id": status.ilink_user_id,
        "base_url": base_url,
        "bot_token_encrypted": base64::engine::general_purpose::STANDARD.encode(sealed),
    });
    let row = match patchbay_weixin::install::finalize(
        &state.pool,
        &patchbay_weixin::install::InstallParams {
            workspace_id,
            agent_id: session.agent_id,
            installer_id: actor,
            bot_id: status.ilink_bot_id.clone(),
            ilink_user_id: status.ilink_user_id.clone(),
            config,
        },
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "failed to persist WeChat installation");
            if error
                .downcast_ref::<patchbay_weixin::install::InstallError>()
                .is_some()
                || error.to_string().contains("bound to another Patchbay user")
            {
                return error_response(
                    StatusCode::CONFLICT,
                    "this WeChat account is already connected",
                );
            }
            if error.to_string().contains("authorization changed") {
                return error_response(
                    StatusCode::FORBIDDEN,
                    "authorization changed during install",
                );
            }
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to save WeChat connection",
            );
        }
    };
    publish_created(&state, Provider::Weixin, &row, actor);
    session.status = "success".into();
    session.installation_id = Some(row.id);
    let _ = store.put(&session_id, &session).await;
    Json(json!({"status": "success", "installation_id": row.id})).into_response()
}

fn validate_weixin_redirect(value: &str) -> Option<String> {
    let value = value.trim();
    let normalized = if value.contains("://") {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    let parsed = url::Url::parse(&normalized).ok()?;
    if parsed.scheme() != "https"
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.port().is_some_and(|port| port != 443)
    {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host != "ilinkai.weixin.qq.com" && !host.ends_with(".weixin.qq.com") {
        return None;
    }
    Some(format!(
        "https://{host}{}",
        parsed
            .port()
            .map(|port| format!(":{port}"))
            .unwrap_or_default()
    ))
}

const LARK_SESSION_PREFIX: &str = "patchbay:{lark_install_session}:";
const LARK_SESSION_TTL: Duration = Duration::from_secs(15 * 60);
const LARK_SESSION_REDIS_TIMEOUT: Duration = Duration::from_millis(250);
const LARK_SESSION_MEMORY_CAP: usize = 1024;

#[derive(Clone, Serialize, Deserialize)]
struct LarkSession {
    workspace_id: Uuid,
    initiator_id: Uuid,
    status: String,
    installation_id: Option<Uuid>,
    error_reason: Option<String>,
    error_message: Option<String>,
    expires_at: DateTime<Utc>,
}

#[derive(Clone)]
struct LarkSessionStore {
    redis: Option<redis::Client>,
}

impl LarkSessionStore {
    fn from_state(state: &HandlerState) -> Self {
        Self {
            redis: state.rate_limit_client.clone(),
        }
    }

    fn key(session_id: &str) -> String {
        format!("{LARK_SESSION_PREFIX}{session_id}")
    }

    fn memory() -> &'static Mutex<HashMap<String, LarkSession>> {
        static SESSIONS: OnceLock<Mutex<HashMap<String, LarkSession>>> = OnceLock::new();
        SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn ttl_secs(expires_at: DateTime<Utc>) -> u64 {
        (expires_at - Utc::now()).num_seconds().max(1) as u64
    }

    async fn insert(&self, session_id: &str, session: LarkSession) -> Result<(), &'static str> {
        if self.redis.is_some() {
            return self.persist(session_id, &session).await;
        }
        let mut sessions = Self::memory().lock().unwrap();
        let now = Utc::now();
        sessions.retain(|_, value| value.expires_at > now);
        if sessions.len() >= LARK_SESSION_MEMORY_CAP && !sessions.contains_key(session_id) {
            return Err("too many install sessions");
        }
        sessions.insert(session_id.to_string(), session);
        Ok(())
    }

    async fn persist(&self, session_id: &str, session: &LarkSession) -> Result<(), &'static str> {
        if let Some(client) = &self.redis {
            return self.redis_set(client, session_id, session).await;
        }
        Self::memory()
            .lock()
            .unwrap()
            .insert(session_id.to_string(), session.clone());
        Ok(())
    }

    async fn get(&self, session_id: &str) -> Result<Option<LarkSession>, &'static str> {
        if let Some(client) = &self.redis {
            return self.redis_get(client, session_id).await;
        }
        let mut sessions = Self::memory().lock().unwrap();
        let now = Utc::now();
        sessions.retain(|_, value| value.expires_at > now);
        Ok(sessions.get(session_id).cloned())
    }

    async fn remove(&self, session_id: &str) {
        if let Some(client) = &self.redis {
            let _ = self.redis_del(client, session_id).await;
            return;
        }
        Self::memory().lock().unwrap().remove(session_id);
    }

    async fn finish(
        &self,
        session_id: &str,
        installation_id: Option<Uuid>,
        reason: Option<&str>,
        message: Option<&str>,
    ) {
        let mut session = match self.get(session_id).await {
            Ok(Some(session)) => session,
            _ => return,
        };
        session.status = if installation_id.is_some() {
            "success".into()
        } else {
            "error".into()
        };
        session.installation_id = installation_id;
        session.error_reason = reason.map(str::to_string);
        session.error_message = message.map(str::to_string);
        let _ = self.persist(session_id, &session).await;
    }

    async fn redis_set(
        &self,
        client: &redis::Client,
        session_id: &str,
        session: &LarkSession,
    ) -> Result<(), &'static str> {
        let payload =
            serde_json::to_string(session).map_err(|_| "failed to persist install session")?;
        let key = Self::key(session_id);
        let ttl = Self::ttl_secs(session.expires_at);
        let operation = async {
            let mut connection = client.get_multiplexed_async_connection().await?;
            redis::cmd("SET")
                .arg(key)
                .arg(payload)
                .arg("EX")
                .arg(ttl)
                .query_async::<()>(&mut connection)
                .await
        };
        match tokio::time::timeout(LARK_SESSION_REDIS_TIMEOUT, operation).await {
            Ok(Ok(())) => Ok(()),
            _ => Err("failed to persist install session"),
        }
    }

    async fn redis_get(
        &self,
        client: &redis::Client,
        session_id: &str,
    ) -> Result<Option<LarkSession>, &'static str> {
        let key = Self::key(session_id);
        let operation = async {
            let mut connection = client.get_multiplexed_async_connection().await?;
            redis::cmd("GET")
                .arg(key)
                .query_async::<Option<String>>(&mut connection)
                .await
        };
        match tokio::time::timeout(LARK_SESSION_REDIS_TIMEOUT, operation).await {
            Ok(Ok(Some(payload))) => Ok(serde_json::from_str(&payload)
                .ok()
                .filter(|session: &LarkSession| session.expires_at > Utc::now())),
            Ok(Ok(None)) => Ok(None),
            _ => Err("failed to load install session"),
        }
    }

    async fn redis_del(
        &self,
        client: &redis::Client,
        session_id: &str,
    ) -> Result<(), &'static str> {
        let key = Self::key(session_id);
        let operation = async {
            let mut connection = client.get_multiplexed_async_connection().await?;
            redis::cmd("DEL")
                .arg(key)
                .query_async::<()>(&mut connection)
                .await
        };
        match tokio::time::timeout(LARK_SESSION_REDIS_TIMEOUT, operation).await {
            Ok(Ok(())) => Ok(()),
            _ => Err("failed to persist install session"),
        }
    }
}

struct LarkRegistrationRuntime {
    pool: sqlx::PgPool,
    bus: Arc<patchbay_events::Bus>,
    http_base_url: String,
    cancel: CancellationToken,
    sessions: LarkSessionStore,
}

fn can_manage_lark_agent(role: &str, owner_id: Option<Uuid>, actor: Uuid) -> bool {
    matches!(role, "owner" | "admin") || owner_id == Some(actor)
}

async fn lark_finalize_authorized(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    agent_id: Uuid,
    actor: Uuid,
) -> anyhow::Result<bool> {
    if agent_id.is_nil() {
        return sqlx::query_scalar::<_, bool>(
            r#"SELECT EXISTS (
                SELECT 1 FROM member
                WHERE workspace_id = $1
                  AND user_id = $2
                  AND role IN ('owner', 'admin')
            )"#,
        )
        .bind(workspace_id)
        .bind(actor)
        .fetch_one(&mut **tx)
        .await
        .map_err(anyhow::Error::from);
    }
    let current = sqlx::query_as::<_, (String, Option<Uuid>)>(
        r#"SELECT m.role, a.owner_id
FROM member m
JOIN agent a ON a.id = $3 AND a.workspace_id = m.workspace_id AND a.kind = 'user'
WHERE m.workspace_id = $1 AND m.user_id = $2
FOR SHARE OF m, a"#,
    )
    .bind(workspace_id)
    .bind(actor)
    .bind(agent_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(current.is_some_and(|(role, owner_id)| can_manage_lark_agent(&role, owner_id, actor)))
}

async fn begin_lark_install(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Query(query): Query<AgentQuery>,
) -> Response {
    if secret_box(Provider::Lark).is_none() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "lark install not configured",
        );
    }
    let (workspace_id, agent_id, actor) =
        match install_context(&state, &context, &headers, &query).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    let region = match query
        .region
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "" | "feishu" => patchbay_lark::types::Region::Feishu,
        "lark" => patchbay_lark::types::Region::Lark,
        _ => return error_response(StatusCode::BAD_REQUEST, "region must be 'feishu' or 'lark'"),
    };
    let preset = if agent_id.is_nil() {
        "Patchbay".to_string()
    } else {
        let target = match agent::get_agent_in_workspace(&state.pool, agent_id, workspace_id).await
        {
            Ok(Some(value)) => value,
            _ => return error_response(StatusCode::NOT_FOUND, "agent not found in this workspace"),
        };
        if !matches!(context.member.role.as_str(), "owner" | "admin")
            && target.owner_id != Some(actor)
        {
            return error_response(StatusCode::FORBIDDEN, "not allowed to manage this agent");
        }
        if target.name.trim().is_empty() {
            "Patchbay".to_string()
        } else {
            format!("{} - Patchbay", target.name.trim())
        }
    };
    let client = Arc::new(patchbay_lark::registration::RegistrationClient::new(
        patchbay_lark::registration::RegistrationConfig {
            domain: state
                .integrations
                .lark_registration_domain
                .clone()
                .unwrap_or_default(),
            lark_domain: state
                .integrations
                .lark_registration_lark_domain
                .clone()
                .unwrap_or_default(),
            ..Default::default()
        },
    ));
    let begun = match client.begin(&preset, region).await {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, "failed to begin Lark registration");
            return error_response(StatusCode::BAD_GATEWAY, "failed to start install");
        }
    };
    let session_id = Uuid::new_v4().to_string();
    let sessions = LarkSessionStore::from_state(&state);
    if let Err(message) = sessions
        .insert(
            &session_id,
            LarkSession {
                workspace_id,
                initiator_id: actor,
                status: "pending".into(),
                installation_id: None,
                error_reason: None,
                error_message: None,
                expires_at: Utc::now()
                    + chrono::Duration::seconds(LARK_SESSION_TTL.as_secs() as i64),
            },
        )
        .await
    {
        let status = if message == "too many install sessions" {
            StatusCode::TOO_MANY_REQUESTS
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
        return error_response(status, message);
    }
    let task_session = session_id.clone();
    let poll_interval = begun.interval.as_secs().max(1);
    let expires = begun.expires_in;
    let runtime = LarkRegistrationRuntime {
        pool: state.pool.clone(),
        bus: state.bus.clone(),
        http_base_url: state
            .integrations
            .lark_http_base_url
            .clone()
            .unwrap_or_default(),
        cancel: state.channel_cancel.clone(),
        sessions: sessions.clone(),
    };
    if !state.channel_tasks.spawn(run_lark_registration(
        runtime,
        client,
        task_session,
        (workspace_id, agent_id, actor),
        region,
        begun.clone(),
    )) {
        sessions.remove(&session_id).await;
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "channel runtime is shutting down",
        );
    }
    Json(json!({
        "session_id": session_id,
        "qr_code_url": begun.qr_code_url,
        "expires_in_seconds": expires.as_secs(),
        "poll_interval_seconds": poll_interval
    }))
    .into_response()
}

async fn run_lark_registration(
    runtime: LarkRegistrationRuntime,
    client: Arc<patchbay_lark::registration::RegistrationClient>,
    session_id: String,
    identity: (Uuid, Uuid, Uuid),
    mut region: patchbay_lark::types::Region,
    begun: patchbay_lark::registration::BeginResult,
) {
    let (workspace_id, agent_id, actor) = identity;
    let deadline = tokio::time::Instant::now() + begun.expires_in;
    let mut domain = begun.domain;
    let mut interval = begun.interval.max(std::time::Duration::from_secs(1));
    loop {
        if runtime.cancel.is_cancelled() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            runtime
                .sessions
                .finish(
                    &session_id,
                    None,
                    Some("expired"),
                    Some("install session expired"),
                )
                .await;
            return;
        }
        tokio::select! {
            _ = runtime.cancel.cancelled() => return,
            _ = tokio::time::sleep(interval) => {}
        }
        let poll = tokio::select! {
            _ = runtime.cancel.cancelled() => return,
            result = client.poll(&domain, &begun.device_code) => result,
        };
        let result = match poll {
            Ok(value) => value,
            Err(error) if lark_poll_protocol_error(&error) => {
                runtime
                    .sessions
                    .finish(
                        &session_id,
                        None,
                        Some("lark_protocol_error"),
                        Some(&format!("{error:#}")),
                    )
                    .await;
                return;
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %session_id,
                    %error,
                    "Lark registration transport error; retrying"
                );
                continue;
            }
        };
        if !result.switched_domain.is_empty() {
            domain = result.switched_domain;
            if let Some(value) = result.switched_region {
                region = value;
            }
            continue;
        }
        if let Some(error) = result.err {
            let reason = match error.code.as_str() {
                "access_denied" => "access_denied",
                "expired_token" => "expired",
                _ => "registration_failed",
            };
            runtime
                .sessions
                .finish(&session_id, None, Some(reason), Some(&error.to_string()))
                .await;
            return;
        }
        if result.status == "slow_down" {
            interval += std::time::Duration::from_secs(5);
            continue;
        }
        if result.client_id.is_empty() {
            continue;
        }
        let api = patchbay_lark::http_client::HttpApiClient::new(
            patchbay_lark::http_client::HttpClientConfig {
                base_url: runtime.http_base_url.clone(),
                ..Default::default()
            },
        );
        let credentials = patchbay_lark::client::InstallationCredentials {
            app_id: result.client_id.clone(),
            app_secret: result.client_secret.clone(),
            tenant_key: String::new(),
            region,
        };
        let bot = match api.get_bot_info(credentials).await {
            Ok(value) => value,
            Err(error) => {
                runtime
                    .sessions
                    .finish(
                        &session_id,
                        None,
                        Some("bot_info_failed"),
                        Some(&format!("{error:#}")),
                    )
                    .await;
                return;
            }
        };
        let box_ = match secret_box(Provider::Lark) {
            Some(value) => value,
            None => {
                runtime
                    .sessions
                    .finish(
                        &session_id,
                        None,
                        Some("not_configured"),
                        Some("lark install not configured"),
                    )
                    .await;
                return;
            }
        };
        let sealed = match box_.seal(result.client_secret.as_bytes()) {
            Ok(value) => value,
            Err(error) => {
                runtime
                    .sessions
                    .finish(
                        &session_id,
                        None,
                        Some("encryption_failed"),
                        Some(&error.to_string()),
                    )
                    .await;
                return;
            }
        };
        let mut tx = match runtime.pool.begin().await {
            Ok(value) => value,
            Err(error) => {
                runtime
                    .sessions
                    .finish(
                        &session_id,
                        None,
                        Some("internal_error"),
                        Some(&error.to_string()),
                    )
                    .await;
                return;
            }
        };
        let authorized =
            match lark_finalize_authorized(&mut tx, workspace_id, agent_id, actor).await {
                Ok(authorized) => authorized,
                Err(error) => {
                    let _ = tx.rollback().await;
                    runtime
                        .sessions
                        .finish(
                            &session_id,
                            None,
                            Some("internal_error"),
                            Some(&error.to_string()),
                        )
                        .await;
                    return;
                }
            };
        if !authorized {
            let _ = tx.rollback().await;
            runtime
                .sessions
                .finish(
                    &session_id,
                    None,
                    Some("authorization_revoked"),
                    Some("workspace membership or agent-management permission changed"),
                )
                .await;
            return;
        }
        let app_id = result.client_id.clone();
        if let Err(error) = patchbay_lark::channel_store::reclaim_dead_installation_with(
            &mut *tx,
            workspace_id,
            agent_id,
            &app_id,
        )
        .await
        {
            let _ = tx.rollback().await;
            runtime
                .sessions
                .finish(
                    &session_id,
                    None,
                    Some("internal_error"),
                    Some(&format!("{error:#}")),
                )
                .await;
            return;
        }
        let installation = match patchbay_lark::channel_store::upsert_lark_installation_with(
            &mut *tx,
            patchbay_lark::params::UpsertInstallationParams {
                workspace_id,
                agent_id,
                app_id: app_id.clone(),
                app_secret_encrypted: sealed,
                bot_open_id: bot.open_id.0,
                installer_user_id: actor,
                tenant_key: None,
                bot_union_id: (!bot.union_id.is_empty()).then_some(bot.union_id),
                region: region.as_str().into(),
            },
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                let is_conflict = patchbay_lark::channel_store::is_unique_violation(&error);
                let _ = tx.rollback().await;
                if is_conflict {
                    let message =
                        lark_live_owner_conflict_message(&runtime.pool, workspace_id, &app_id)
                            .await;
                    runtime
                        .sessions
                        .finish(
                            &session_id,
                            None,
                            Some("installation_conflict"),
                            Some(&message),
                        )
                        .await;
                } else {
                    runtime
                        .sessions
                        .finish(
                            &session_id,
                            None,
                            Some("internal_error"),
                            Some(&format!("{error:#}")),
                        )
                        .await;
                }
                return;
            }
        };
        if let Err(error) = patchbay_lark::channel_store::create_lark_user_binding_with(
            &mut *tx,
            patchbay_lark::params::CreateUserBindingParams {
                workspace_id,
                patchbay_user_id: actor,
                installation_id: installation.id,
                channel_user_id: result.open_id.0,
                union_id: None,
            },
        )
        .await
        {
            let _ = tx.rollback().await;
            runtime
                .sessions
                .finish(
                    &session_id,
                    None,
                    Some("installer_bind_failed"),
                    Some(&format!("{error:#}")),
                )
                .await;
            return;
        }
        if let Err(error) = tx.commit().await {
            runtime
                .sessions
                .finish(
                    &session_id,
                    None,
                    Some("internal_error"),
                    Some(&format!("{error:#}")),
                )
                .await;
            return;
        }
        runtime.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_LARK_INSTALLATION_CREATED.into(),
            workspace_id: workspace_id.to_string(),
            actor_type: "system".into(),
            payload: json!({"installation_id": installation.id}),
            ..Default::default()
        });
        runtime
            .sessions
            .finish(&session_id, Some(installation.id), None, None)
            .await;
        return;
    }
}

fn lark_poll_protocol_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<patchbay_lark::registration::RegistrationError>()
        .is_some()
}

async fn lark_live_owner_conflict_message(
    pool: &sqlx::PgPool,
    requesting_workspace_id: Uuid,
    app_id: &str,
) -> String {
    let owner = patchbay_lark::channel_store::ChannelStore::new(pool.clone())
        .installation_owner_by_app_id(app_id)
        .await
        .ok()
        .flatten();
    lark_owner_conflict_message(requesting_workspace_id, owner.as_ref())
}

fn lark_owner_conflict_message(
    requesting_workspace_id: Uuid,
    owner: Option<&patchbay_db::queries::channel::GetChannelInstallationOwnerByAppIDRow>,
) -> String {
    match owner {
        Some(owner) if owner.workspace_id != Some(requesting_workspace_id) => {
            "This Feishu app is already connected to a different Patchbay workspace. Disconnect it there before connecting it here."
        }
        Some(owner) if owner.agent_archived_at.is_some() => {
            "This Feishu app is connected to an archived agent in this workspace. Restore that agent, or disconnect its bot, before connecting it here."
        }
        Some(_) => {
            "This Feishu app is already connected to another agent in this workspace. Disconnect it there first, then connect it here."
        }
        None => {
            "This Feishu app is already connected to another agent. Disconnect it there first, then connect it here."
        }
    }
    .into()
}

async fn lark_install_status(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Path((_workspace, session_id)): Path<(String, String)>,
) -> Response {
    if let Err(response) = require_formal_user(&headers) {
        return response;
    }
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let actor = match user_id(&headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let session = match LarkSessionStore::from_state(&state)
        .get(session_id.trim())
        .await
    {
        Ok(Some(session)) => session,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "install session not found"),
        Err(message) => return error_response(StatusCode::SERVICE_UNAVAILABLE, message),
    };
    if session.workspace_id != workspace_id {
        return error_response(StatusCode::NOT_FOUND, "install session not found");
    }
    if session.initiator_id != actor && !matches!(context.member.role.as_str(), "owner" | "admin") {
        return error_response(StatusCode::NOT_FOUND, "install session not found");
    }
    Json(json!({
        "status": session.status,
        "installation_id": session.installation_id,
        "error_reason": session.error_reason,
        "error_message": session.error_message
    }))
    .into_response()
}

async fn list_dingtalk_group_routes(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    if secret_box(Provider::DingTalk).is_none() {
        return Json(json!({"routes": []})).into_response();
    }
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    match dingtalk::list_ding_talk_group_routes_by_workspace(&state.pool, workspace_id).await {
        Ok(rows) => Json(json!({"routes": rows.into_iter().map(|row| json!({
            "id": row.id, "workspace_id": row.workspace_id, "installation_id": row.installation_id,
            "conversation_id": row.conversation_id, "conversation_title": row.conversation_title,
            "agent_id": row.agent_id, "discovered_at": crate::timefmt::rfc3339(row.discovered_at),
            "updated_at": crate::timefmt::rfc3339(row.updated_at),
        })).collect::<Vec<_>>() }))
        .into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list dingtalk group routes",
        ),
    }
}

#[derive(Deserialize)]
struct GroupRouteBody {
    agent_id: String,
}

async fn update_dingtalk_group_route(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Path((_workspace, raw_route)): Path<(String, String)>,
    bytes: axum::body::Bytes,
) -> Response {
    if let Err(response) = require_formal_user(&headers) {
        return response;
    }
    if secret_box(Provider::DingTalk).is_none() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "dingtalk integration not configured",
        );
    }
    let actor = match user_id(&headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let route_id = match Uuid::parse_str(&raw_route) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid route id"),
    };
    let input: GroupRouteBody = match decode_body(&bytes) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let agent_id = match Uuid::parse_str(input.agent_id.trim()) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid agent_id"),
    };
    let target = match agent::get_agent(&state.pool, agent_id).await {
        Ok(Some(value)) if value.workspace_id == workspace_id => value,
        Ok(_) => return error_response(StatusCode::NOT_FOUND, "agent not found in this workspace"),
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load agent"),
    };
    if target.kind != "user" {
        return error_response(
            StatusCode::BAD_REQUEST,
            "only user agents can handle a DingTalk group",
        );
    }
    if target.archived_at.is_some() {
        return error_response(
            StatusCode::CONFLICT,
            "an archived agent cannot handle a DingTalk group",
        );
    }
    let row = match dingtalk::reassign_ding_talk_group_route(
        &state.pool,
        workspace_id,
        agent_id,
        route_id,
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "dingtalk group route not found"),
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update dingtalk group route",
            );
        }
    };
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::EVENT_DINGTALK_GROUP_ROUTE_UPDATED.into(),
        workspace_id: workspace_id.to_string(),
        actor_type: "user".into(),
        actor_id: actor.to_string(),
        payload: json!({"id": route_id}),
        ..Default::default()
    });
    Json(json!({
        "id": row.id,
        "workspace_id": row.workspace_id,
        "installation_id": row.installation_id,
        "conversation_id": row.conversation_id,
        "conversation_title": row.conversation_title,
        "agent_id": row.agent_id,
        "discovered_at": row.discovered_at.map(crate::timefmt::rfc3339),
        "updated_at": row.updated_at.map(crate::timefmt::rfc3339)
    }))
    .into_response()
}

async fn revoke(
    state: HandlerState,
    context: WorkspaceContext,
    headers: HeaderMap,
    raw_id: String,
    provider: Provider,
) -> Response {
    if let Err(response) = require_formal_user(&headers) {
        return response;
    }
    if secret_box(provider).is_none() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "integration not configured",
        );
    }
    let actor = match user_id(&headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let id = match Uuid::parse_str(&raw_id) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid installation id"),
    };
    let installation = match channel::get_channel_installation_in_workspace(
        &state.pool,
        id,
        workspace_id,
        provider.channel_type(),
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "installation not found"),
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load installation",
            );
        }
    };
    if matches!(provider, Provider::Lark | Provider::Weixin)
        && !matches!(context.member.role.as_str(), "owner" | "admin")
    {
        let can_manage = match installation.agent_id {
            None => false,
            Some(agent_id) => matches!(
                agent::get_agent_in_workspace(&state.pool, agent_id, workspace_id).await,
                Ok(Some(value)) if value.owner_id == Some(actor)
            ),
        };
        if !can_manage {
            return error_response(StatusCode::FORBIDDEN, "not allowed to manage this agent");
        }
    }
    if channel::set_channel_installation_status(&state.pool, id, "revoked")
        .await
        .is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to revoke installation",
        );
    }
    state.bus.publish(&patchbay_events::Event {
        event_type: provider.revoked_event().into(),
        workspace_id: workspace_id.to_string(),
        actor_type: "user".into(),
        actor_id: actor.to_string(),
        payload: json!({"id": id}),
        ..Default::default()
    });
    StatusCode::NO_CONTENT.into_response()
}

macro_rules! revoke_handler {
    ($name:ident, $provider:expr) => {
        async fn $name(
            State(state): State<HandlerState>,
            Extension(context): Extension<WorkspaceContext>,
            headers: HeaderMap,
            Path((_workspace, id)): Path<(String, String)>,
        ) -> Response {
            revoke(state, context, headers, id, $provider).await
        }
    };
}
revoke_handler!(revoke_dingtalk, Provider::DingTalk);
revoke_handler!(revoke_lark, Provider::Lark);
revoke_handler!(revoke_slack, Provider::Slack);
revoke_handler!(revoke_telegram, Provider::Telegram);
revoke_handler!(revoke_wecom, Provider::WeCom);
revoke_handler!(revoke_weixin, Provider::Weixin);

#[derive(Deserialize)]
struct AgentQuery {
    agent_id: Option<String>,
    region: Option<String>,
}

async fn install_context(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    query: &AgentQuery,
) -> Result<(Uuid, Uuid, Uuid), Response> {
    require_formal_user(headers)?;
    let workspace_id = workspace_id(context)?;
    let actor = user_id(headers)?;
    let Some(raw) = query
        .agent_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        if !matches!(context.member.role.as_str(), "owner" | "admin") {
            return Err(error_response(
                StatusCode::FORBIDDEN,
                "only workspace admins can connect a platform without selecting an Agent",
            ));
        }
        return Ok((workspace_id, Uuid::nil(), actor));
    };
    let agent_id = Uuid::parse_str(raw)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid agent_id"))?;
    if !matches!(
        agent::get_agent_in_workspace(&state.pool, agent_id, workspace_id).await,
        Ok(Some(_))
    ) {
        return Err(error_response(
            StatusCode::NOT_FOUND,
            "agent not found in this workspace",
        ));
    }
    Ok((workspace_id, agent_id, actor))
}

fn decode_body<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, Response> {
    if bytes.len() > BODY_LIMIT {
        return Err(error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request body is too large",
        ));
    }
    serde_json::from_slice(bytes)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid request body"))
}

fn publish_created(
    state: &HandlerState,
    provider: Provider,
    row: &ChannelInstallation,
    actor: Uuid,
) {
    state.bus.publish(&patchbay_events::Event {
        event_type: provider.created_event().into(),
        workspace_id: row.workspace_id.to_string(),
        actor_type: "user".into(),
        actor_id: actor.to_string(),
        payload: json!({"id": row.id}),
        ..Default::default()
    });
}

#[derive(Deserialize)]
struct DingTalkBody {
    #[serde(alias = "app_key")]
    client_id: String,
    #[serde(alias = "app_secret")]
    client_secret: String,
}

fn dingtalk_install_error(error: &anyhow::Error) -> (StatusCode, String) {
    use patchbay_dingtalk::byo_install::ByoError;
    use patchbay_dingtalk::install::InstallError;

    match error.downcast_ref::<ByoError>() {
        Some(ByoError::InvalidAppKey | ByoError::InvalidAppSecret) => {
            (StatusCode::BAD_REQUEST, error.to_string())
        }
        Some(ByoError::CredentialValidation(_)) => (
            StatusCode::BAD_REQUEST,
            "could not verify the DingTalk credentials — check the AppKey (client id) and AppSecret (client secret), and that the robot is a Stream-mode robot in your organization".into(),
        ),
        Some(ByoError::Install(InstallError::RobotOwnedBySameWorkspace)) => (
            StatusCode::CONFLICT,
            "this DingTalk robot is already connected to another agent in this workspace — disconnect it there first, then connect it here".into(),
        ),
        Some(ByoError::Install(InstallError::RobotOwnedByArchivedAgent)) => (
            StatusCode::CONFLICT,
            "this DingTalk robot is connected to an archived agent in this workspace — restore that agent, or disconnect its robot, before connecting it here".into(),
        ),
        Some(ByoError::Install(InstallError::RobotOwnedByAnotherWorkspace)) => (
            StatusCode::CONFLICT,
            "this DingTalk robot is already connected to a different Patchbay workspace — disconnect it there before connecting it here".into(),
        ),
        Some(ByoError::Install(InstallError::InstallationNotFound)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not connect the DingTalk robot".into(),
        ),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not connect the DingTalk robot".into(),
        ),
    }
}

async fn install_dingtalk(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Query(query): Query<AgentQuery>,
    bytes: axum::body::Bytes,
) -> Response {
    let Some(box_) = secret_box(Provider::DingTalk) else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "dingtalk integration not enabled",
        );
    };
    let (workspace_id, agent_id, actor) =
        match install_context(&state, &context, &headers, &query).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    let input: DingTalkBody = match decode_body(&bytes) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let service = patchbay_dingtalk::byo_install::ByoInstallService::new(
        state.pool.clone(),
        Arc::new(box_),
        None,
        "",
    );
    match service
        .register_byo(patchbay_dingtalk::byo_install::RegisterByoParams {
            workspace_id,
            agent_id,
            initiator_id: actor,
            app_key: input.client_id,
            app_secret: input.client_secret,
        })
        .await
    {
        Ok(row) => {
            publish_created(&state, Provider::DingTalk, &row, actor);
            Json(installation_response(Provider::DingTalk, row)).into_response()
        }
        Err(error) => {
            tracing::warn!(error = %error, "DingTalk installation rejected");
            let (status, message) = dingtalk_install_error(&error);
            error_response(status, &message)
        }
    }
}

#[derive(Deserialize)]
struct WeComBody {
    bot_id: String,
    secret: String,
    #[serde(default)]
    bot_name: String,
}

async fn install_wecom(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Query(query): Query<AgentQuery>,
    bytes: axum::body::Bytes,
) -> Response {
    let Some(box_) = secret_box(Provider::WeCom) else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "wecom integration not enabled",
        );
    };
    let (workspace_id, agent_id, actor) =
        match install_context(&state, &context, &headers, &query).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    let input: WeComBody = match decode_body(&bytes) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let service = patchbay_wecom::installation::InstallationService::new(state.pool.clone(), box_);
    let cancel = CancellationToken::new();
    match service
        .upsert(
            &cancel,
            &patchbay_wecom::installation::InstallationParams {
                workspace_id,
                agent_id,
                installer_user_id: actor,
                bot_id: input.bot_id.trim().into(),
                secret: input.secret.trim().into(),
                bot_display_name: input.bot_name.trim().into(),
            },
        )
        .await
    {
        Ok(inst) => {
            let row = match channel::get_channel_installation_in_workspace(
                &state.pool,
                inst.id,
                workspace_id,
                Provider::WeCom.channel_type(),
            )
            .await
            {
                Ok(Some(row)) => row,
                _ => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to load installation",
                    );
                }
            };
            publish_created(&state, Provider::WeCom, &row, actor);
            Json(installation_response(Provider::WeCom, row)).into_response()
        }
        Err(error) => {
            let failure = classify_wecom_install_error(&error);
            match failure.log {
                WecomInstallLog::None => {}
                WecomInstallLog::Warn => tracing::warn!(
                    %error,
                    %workspace_id,
                    %agent_id,
                    "WeCom installation could not verify the bot"
                ),
                WecomInstallLog::Error => tracing::error!(
                    %error,
                    %workspace_id,
                    %agent_id,
                    "WeCom installation failed"
                ),
            }
            error_code_response(failure.status, failure.code, failure.message)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WecomInstallLog {
    None,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WecomInstallFailure {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    log: WecomInstallLog,
}

fn classify_wecom_install_error(error: &anyhow::Error) -> WecomInstallFailure {
    use patchbay_wecom::installation::BotOwnershipError;

    let (status, code, message, log) =
        match patchbay_wecom::installation::as_bot_ownership_error(error) {
            Some(BotOwnershipError::SameWorkspace) => (
                StatusCode::CONFLICT,
                "wecom_bot_owned_by_same_workspace",
                "this bot is already connected to another agent in this workspace — disconnect it there first, then connect it here",
                WecomInstallLog::None,
            ),
            Some(BotOwnershipError::ArchivedAgent) => (
                StatusCode::CONFLICT,
                "wecom_bot_owned_by_archived_agent",
                "this bot is connected to an archived agent in this workspace — restore that agent, or disconnect its bot, before connecting it here",
                WecomInstallLog::None,
            ),
            Some(BotOwnershipError::AnotherWorkspace) => (
                StatusCode::CONFLICT,
                "wecom_bot_owned_by_another_workspace",
                "this bot is already connected to a different Patchbay workspace — disconnect it there before connecting it here",
                WecomInstallLog::None,
            ),
            None if error.chain().any(|cause| {
                cause
                    .downcast_ref::<patchbay_wecom::installation::InvalidInstallationParams>()
                    .is_some()
            }) => (
                StatusCode::BAD_REQUEST,
                "wecom_install_rejected",
                "could not connect the WeCom bot — check the Bot ID and secret from the WeCom admin console, and that the bot is a smart bot with the long connection enabled",
                WecomInstallLog::None,
            ),
            None if patchbay_wecom::credential_probe::is_credentials_rejected(error) => (
                StatusCode::BAD_REQUEST,
                "wecom_credentials_rejected",
                "WeCom rejected this Bot ID and secret — check both on the WeCom admin console, and that the bot is a smart bot with the long connection enabled",
                WecomInstallLog::None,
            ),
            None if patchbay_wecom::credential_probe::is_credentials_unverifiable(error) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "wecom_credentials_unverifiable",
                "could not reach WeCom to verify this bot — the credentials were not changed; try again in a moment",
                WecomInstallLog::Warn,
            ),
            None => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "wecom_install_failed",
                "could not save this bot — something went wrong on our side. Your credentials were not changed; please try again, and contact support if it keeps failing",
                WecomInstallLog::Error,
            ),
        };
    WecomInstallFailure {
        status,
        code,
        message,
        log,
    }
}

#[derive(Deserialize)]
struct TelegramBody {
    bot_token: String,
}

async fn install_telegram(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Query(query): Query<AgentQuery>,
    bytes: axum::body::Bytes,
) -> Response {
    let Some(box_) = secret_box(Provider::Telegram) else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "telegram integration not enabled",
        );
    };
    let (workspace_id, agent_id, actor) =
        match install_context(&state, &context, &headers, &query).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    let input: TelegramBody = match decode_body(&bytes) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let token = input.bot_token.trim();
    let bot_id = match patchbay_telegram::parse_bot_id(token) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "telegram: bot token must look like 123456:ABC-DEF…",
            );
        }
    };
    let api = patchbay_telegram::BotApi::new("", token);
    let me = match api.get_me().await {
        Ok(value) if value.is_bot && !value.username.is_empty() => value,
        Ok(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Telegram rejected this bot token — generate a current token in @BotFather and try again",
            );
        }
        Err(error) => {
            let failure = classify_telegram_verification_error(&error);
            tracing::warn!(%error, "Telegram credential verification failed");
            return error_response(failure.status, failure.message);
        }
    };
    match api.get_webhook_info().await {
        Ok(value) if !value.url.is_empty() => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "this Telegram bot has a webhook configured — remove the webhook before connecting it with long polling",
            );
        }
        Ok(_) => {}
        Err(error) => {
            let failure = classify_telegram_verification_error(&error);
            tracing::warn!(%error, "Telegram webhook verification failed");
            return error_response(failure.status, failure.message);
        }
    }
    let sealed = match box_.seal(token.as_bytes()) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to encrypt bot token",
            );
        }
    };
    let config = json!({
        "app_id": bot_id,
        "bot_username": me.username,
        "bot_token_encrypted": base64::engine::general_purpose::STANDARD.encode(sealed)
    });
    let persist = match patchbay_telegram::install::InstallPersist::new(
        workspace_id,
        agent_id,
        actor,
        bot_id,
        config,
    ) {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "failed to compose Telegram installation");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to save Telegram installation",
            );
        }
    };
    match patchbay_telegram::install::InstallService::new(state.pool.clone())
        .persist_install(&persist)
        .await
    {
        Ok(row) => {
            publish_created(&state, Provider::Telegram, &row, actor);
            Json(installation_response(Provider::Telegram, row)).into_response()
        }
        Err(error) => {
            let failure = classify_telegram_install_persist_error(&error);
            if failure.status == StatusCode::INTERNAL_SERVER_ERROR {
                tracing::error!(%error, %workspace_id, %agent_id, "Telegram installation persist failed");
            }
            error_response(failure.status, failure.message)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TelegramVerificationFailure {
    status: StatusCode,
    message: &'static str,
}

fn classify_telegram_verification_error(error: &anyhow::Error) -> TelegramVerificationFailure {
    let rejected = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<patchbay_telegram::api::ApiError>())
        .is_some_and(|api| api.code == StatusCode::UNAUTHORIZED.as_u16());
    if rejected {
        TelegramVerificationFailure {
            status: StatusCode::BAD_REQUEST,
            message: "Telegram rejected this bot token — generate a current token in @BotFather and try again",
        }
    } else {
        TelegramVerificationFailure {
            status: StatusCode::SERVICE_UNAVAILABLE,
            message: "could not reach Telegram to verify this bot — check the server network or proxy and try again; the token was not saved",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TelegramInstallPersistFailure {
    status: StatusCode,
    message: &'static str,
}

fn classify_telegram_install_persist_error(error: &anyhow::Error) -> TelegramInstallPersistFailure {
    use patchbay_telegram::install::InstallError;

    match error
        .chain()
        .find_map(|cause| cause.downcast_ref::<InstallError>())
    {
        Some(InstallError::BotOwnedBySameWorkspace) => TelegramInstallPersistFailure {
            status: StatusCode::CONFLICT,
            message: "this Telegram bot is already connected to another agent in this workspace — disconnect it there first, then connect it here",
        },
        Some(InstallError::BotOwnedByArchivedAgent) => TelegramInstallPersistFailure {
            status: StatusCode::CONFLICT,
            message: "this Telegram bot is connected to an archived agent in this workspace — restore that agent, or disconnect its bot, before connecting it here",
        },
        Some(InstallError::BotOwnedByAnotherWorkspace) => TelegramInstallPersistFailure {
            status: StatusCode::CONFLICT,
            message: "this Telegram bot is already connected to a different Patchbay workspace — disconnect it there before connecting it here",
        },
        None => TelegramInstallPersistFailure {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "could not save this Telegram bot — something went wrong on the server; the token was not saved",
        },
    }
}

#[derive(Deserialize)]
struct SlackBody {
    bot_token: String,
    app_token: String,
}

#[derive(Deserialize)]
struct SlackEnvelope {
    ok: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    team_id: String,
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    bot_id: String,
    #[serde(default)]
    bot: SlackBot,
}
#[derive(Default, Deserialize)]
struct SlackBot {
    #[serde(default)]
    app_id: String,
}

async fn slack_call(
    client: &reqwest::Client,
    method: &str,
    token: &str,
    params: &[(&str, &str)],
) -> anyhow::Result<SlackEnvelope> {
    let response = client
        .post(format!("https://slack.com/api/{method}"))
        .bearer_auth(token)
        .form(params)
        .send()
        .await?;
    let value: SlackEnvelope = response.json().await?;
    if !value.ok {
        anyhow::bail!("Slack rejected {method}: {}", value.error);
    }
    Ok(value)
}

async fn install_slack(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Query(query): Query<AgentQuery>,
    bytes: axum::body::Bytes,
) -> Response {
    let Some(box_) = secret_box(Provider::Slack) else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "slack integration not enabled",
        );
    };
    let (workspace_id, agent_id, actor) =
        match install_context(&state, &context, &headers, &query).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    let input: SlackBody = match decode_body(&bytes) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let bot_token = input.bot_token.trim();
    let app_token = input.app_token.trim();
    if !bot_token.starts_with("xoxb-") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "slack: bot token must start with xoxb-",
        );
    }
    let parts = app_token.splitn(5, '-').collect::<Vec<_>>();
    let Some(app_id) = parts
        .get(2)
        .copied()
        .filter(|value| app_token.starts_with("xapp-") && value.starts_with('A'))
    else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "slack: app-level token must start with xapp- and embed an app id",
        );
    };
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to initialize Slack verification",
            );
        }
    };
    let auth = match slack_call(&client, "auth.test", bot_token, &[]).await {
        Ok(value)
            if !value.team_id.is_empty()
                && !value.user_id.is_empty()
                && !value.bot_id.is_empty() =>
        {
            value
        }
        Ok(_) | Err(_) => {
            return error_response(StatusCode::BAD_REQUEST, "could not verify the Slack tokens");
        }
    };
    let bot = match slack_call(
        &client,
        "bots.info",
        bot_token,
        &[("bot", auth.bot_id.as_str())],
    )
    .await
    {
        Ok(value) => value,
        Err(_) => {
            return error_response(StatusCode::BAD_REQUEST, "could not verify the Slack tokens");
        }
    };
    if bot.bot.app_id != app_id {
        return error_response(
            StatusCode::BAD_REQUEST,
            "slack: the bot token and app-level token are from different Slack apps",
        );
    }
    if slack_call(&client, "apps.connections.open", app_token, &[])
        .await
        .is_err()
    {
        return error_response(StatusCode::BAD_REQUEST, "could not verify the Slack tokens");
    }
    let sealed_bot = match box_.seal(bot_token.as_bytes()) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to encrypt Slack token",
            );
        }
    };
    let sealed_app = match box_.seal(app_token.as_bytes()) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to encrypt Slack token",
            );
        }
    };
    let config = json!({
        "app_id": app_id,
        "team_id": auth.team_id,
        "bot_user_id": auth.user_id,
        "bot_token_encrypted": base64::engine::general_purpose::STANDARD.encode(sealed_bot),
        "app_token_encrypted": base64::engine::general_purpose::STANDARD.encode(sealed_app)
    });
    let persist = match patchbay_slack::install::InstallPersist::from_config(
        workspace_id,
        agent_id,
        actor,
        config,
    ) {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "failed to compose Slack installation");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to save Slack installation",
            );
        }
    };
    match patchbay_slack::install::InstallService::new(state.pool.clone())
        .persist_install(&persist)
        .await
    {
        Ok(row) => {
            publish_created(&state, Provider::Slack, &row, actor);
            Json(installation_response(Provider::Slack, row)).into_response()
        }
        Err(error) => {
            let failure = classify_slack_install_persist_error(&error);
            if failure.status == StatusCode::INTERNAL_SERVER_ERROR {
                tracing::error!(%error, %workspace_id, %agent_id, "Slack installation persist failed");
            }
            error_response(failure.status, failure.message)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SlackInstallPersistFailure {
    status: StatusCode,
    message: &'static str,
}

fn classify_slack_install_persist_error(error: &anyhow::Error) -> SlackInstallPersistFailure {
    use patchbay_slack::install::InstallError;

    let ownership = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<InstallError>());
    match ownership {
        Some(InstallError::TeamOwnedBySameWorkspace) => SlackInstallPersistFailure {
            status: StatusCode::CONFLICT,
            message: "this Slack app is already connected to another agent in this workspace — disconnect it there first, then connect it here",
        },
        Some(InstallError::TeamOwnedByArchivedAgent) => SlackInstallPersistFailure {
            status: StatusCode::CONFLICT,
            message: "this Slack app is connected to an archived agent in this workspace — restore that agent, or disconnect its bot, before connecting it here",
        },
        Some(InstallError::TeamOwnedByAnotherWorkspace) => SlackInstallPersistFailure {
            status: StatusCode::CONFLICT,
            message: "this Slack app is already connected to a different Patchbay workspace — disconnect it there before connecting it here",
        },
        Some(InstallError::InstallationNotFound) | None => SlackInstallPersistFailure {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "failed to save Slack installation",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[test]
    fn public_projections_never_include_credentials() {
        for provider in [
            Provider::DingTalk,
            Provider::Lark,
            Provider::Slack,
            Provider::Telegram,
            Provider::WeCom,
        ] {
            let value = public_config(
                provider,
                &json!({"app_id":"A1", "bot_id":"B1", "app_secret_encrypted":"cipher", "bot_token_encrypted":"cipher", "app_token_encrypted":"cipher", "secret_encrypted":"cipher"}),
            );
            let encoded = value.to_string();
            assert!(!encoded.contains("encrypted"));
            assert!(!encoded.contains("cipher"));
        }
    }

    #[test]
    fn lark_public_projection_omits_absent_tenant_key() {
        let without_tenant = public_config(
            Provider::Lark,
            &json!({"app_id":"cli_1", "bot_open_id":"ou_1", "region":"lark"}),
        );
        assert!(!without_tenant
            .as_object()
            .unwrap()
            .contains_key("tenant_key"));

        let with_tenant = public_config(
            Provider::Lark,
            &json!({"app_id":"cli_1", "tenant_key":"tenant-1"}),
        );
        assert_eq!(with_tenant["tenant_key"], "tenant-1");
    }

    #[test]
    fn dingtalk_list_projection_scopes_member_bindings() {
        let installation_id = Uuid::new_v4();
        let mut bindings = HashMap::new();
        bindings.insert(installation_id, vec!["staff-1001".into()]);

        let admin = dingtalk_installation_bindings(
            json!({"id": installation_id}),
            installation_id,
            Some(&bindings),
        );
        assert_eq!(admin["bound_dingtalk_user_ids"], json!(["staff-1001"]));

        let member =
            dingtalk_installation_bindings(json!({"id": installation_id}), installation_id, None);
        assert!(member.get("bound_dingtalk_user_ids").is_none());
    }

    #[test]
    fn body_limit_is_enforced_before_deserialization() {
        assert!(decode_body::<TelegramBody>(&vec![b'x'; BODY_LIMIT + 1]).is_err());
    }

    #[test]
    fn dingtalk_body_accepts_established_client_field_names() {
        let parsed = decode_body::<DingTalkBody>(
            br#"{"client_id":"ding-key","client_secret":"ding-secret"}"#,
        )
        .unwrap();
        assert_eq!(parsed.client_id, "ding-key");
        assert_eq!(parsed.client_secret, "ding-secret");

        let legacy =
            decode_body::<DingTalkBody>(br#"{"app_key":"old-key","app_secret":"old-secret"}"#)
                .unwrap();
        assert_eq!(legacy.client_id, "old-key");
        assert_eq!(legacy.client_secret, "old-secret");
    }

    #[test]
    fn lark_finalize_uses_current_management_authority() {
        let actor = Uuid::now_v7();
        let other = Uuid::now_v7();

        assert!(can_manage_lark_agent("owner", Some(other), actor));
        assert!(can_manage_lark_agent("admin", Some(other), actor));
        assert!(can_manage_lark_agent("member", Some(actor), actor));
        assert!(!can_manage_lark_agent("member", Some(other), actor));
        assert!(!can_manage_lark_agent("member", None, actor));
    }

    #[test]
    fn dingtalk_install_errors_preserve_client_and_server_classifications() {
        use patchbay_dingtalk::byo_install::ByoError;
        use patchbay_dingtalk::install::InstallError;

        let conflict =
            anyhow::Error::new(ByoError::Install(InstallError::RobotOwnedBySameWorkspace));
        assert_eq!(dingtalk_install_error(&conflict).0, StatusCode::CONFLICT);

        let credentials = anyhow::Error::new(ByoError::CredentialValidation("denied".into()));
        assert_eq!(
            dingtalk_install_error(&credentials).0,
            StatusCode::BAD_REQUEST
        );

        let internal = anyhow::anyhow!("encrypt failed");
        assert_eq!(
            dingtalk_install_error(&internal).0,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn lark_install_conflicts_name_the_live_owner_scope() {
        use chrono::Utc;
        use patchbay_db::queries::channel::GetChannelInstallationOwnerByAppIDRow;

        let workspace_id = Uuid::new_v4();
        let other_workspace_id = Uuid::new_v4();
        let mut owner = GetChannelInstallationOwnerByAppIDRow {
            workspace_id: Some(other_workspace_id),
            agent_id: Some(Uuid::new_v4()),
            agent_archived_at: None,
        };
        assert!(lark_owner_conflict_message(workspace_id, Some(&owner))
            .contains("different Patchbay workspace"));

        owner.workspace_id = Some(workspace_id);
        assert!(lark_owner_conflict_message(workspace_id, Some(&owner))
            .contains("another agent in this workspace"));

        owner.agent_archived_at = Some(Utc::now());
        assert!(lark_owner_conflict_message(workspace_id, Some(&owner)).contains("archived agent"));

        assert!(lark_owner_conflict_message(workspace_id, None).contains("another agent"));
    }

    #[test]
    fn wecom_install_errors_preserve_recovery_semantics() {
        use patchbay_wecom::credential_probe::CredentialError;
        use patchbay_wecom::installation::{BotOwnershipError, InvalidInstallationParams};

        let cases = [
            (
                anyhow::Error::new(BotOwnershipError::SameWorkspace),
                StatusCode::CONFLICT,
                "wecom_bot_owned_by_same_workspace",
                WecomInstallLog::None,
            ),
            (
                anyhow::Error::new(BotOwnershipError::ArchivedAgent),
                StatusCode::CONFLICT,
                "wecom_bot_owned_by_archived_agent",
                WecomInstallLog::None,
            ),
            (
                anyhow::Error::new(BotOwnershipError::AnotherWorkspace),
                StatusCode::CONFLICT,
                "wecom_bot_owned_by_another_workspace",
                WecomInstallLog::None,
            ),
            (
                anyhow::Error::new(InvalidInstallationParams("secret")),
                StatusCode::BAD_REQUEST,
                "wecom_install_rejected",
                WecomInstallLog::None,
            ),
            (
                anyhow::Error::new(CredentialError::Rejected {
                    code: 40_001,
                    msg: "invalid secret".into(),
                }),
                StatusCode::BAD_REQUEST,
                "wecom_credentials_rejected",
                WecomInstallLog::None,
            ),
            (
                anyhow::Error::new(CredentialError::Unverifiable("timeout".into())),
                StatusCode::SERVICE_UNAVAILABLE,
                "wecom_credentials_unverifiable",
                WecomInstallLog::Warn,
            ),
            (
                anyhow::anyhow!("database unavailable"),
                StatusCode::INTERNAL_SERVER_ERROR,
                "wecom_install_failed",
                WecomInstallLog::Error,
            ),
        ];

        for (error, status, code, log) in cases {
            let failure = classify_wecom_install_error(&error);
            assert_eq!(failure.status, status);
            assert_eq!(failure.code, code);
            assert_eq!(failure.log, log);
            assert!(!failure.message.is_empty());
        }
    }

    #[test]
    fn telegram_verification_distinguishes_rejection_from_no_verdict() {
        let rejected = anyhow::Error::new(patchbay_telegram::api::ApiError {
            code: StatusCode::UNAUTHORIZED.as_u16(),
            description: "Unauthorized".into(),
            retry_after: 0,
        });
        let rejected = classify_telegram_verification_error(&rejected);
        assert_eq!(rejected.status, StatusCode::BAD_REQUEST);
        assert!(rejected.message.contains("@BotFather"));

        let unavailable = classify_telegram_verification_error(&anyhow::anyhow!(
            "telegram: getMe request failed"
        ));
        assert_eq!(unavailable.status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(unavailable.message.contains("token was not saved"));

        let rate_limited = anyhow::Error::new(patchbay_telegram::api::ApiError {
            code: StatusCode::TOO_MANY_REQUESTS.as_u16(),
            description: "Too Many Requests".into(),
            retry_after: 1,
        });
        assert_eq!(
            classify_telegram_verification_error(&rate_limited).status,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn telegram_persist_errors_preserve_owner_recovery_path() {
        use patchbay_telegram::install::InstallError;

        let cases = [
            (
                InstallError::BotOwnedBySameWorkspace,
                "another agent in this workspace",
            ),
            (
                InstallError::BotOwnedByArchivedAgent,
                "archived agent in this workspace",
            ),
            (
                InstallError::BotOwnedByAnotherWorkspace,
                "different Patchbay workspace",
            ),
        ];
        for (error, recovery_scope) in cases {
            let failure = classify_telegram_install_persist_error(&anyhow::Error::new(error));
            assert_eq!(failure.status, StatusCode::CONFLICT);
            assert!(failure.message.contains(recovery_scope));
        }

        let internal = classify_telegram_install_persist_error(&anyhow::anyhow!("database down"));
        assert_eq!(internal.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(internal.message.contains("token was not saved"));
    }

    #[test]
    fn slack_persist_errors_preserve_owner_recovery_path() {
        use patchbay_slack::install::InstallError;

        let cases = [
            (
                InstallError::TeamOwnedBySameWorkspace,
                "another agent in this workspace",
            ),
            (
                InstallError::TeamOwnedByArchivedAgent,
                "archived agent in this workspace",
            ),
            (
                InstallError::TeamOwnedByAnotherWorkspace,
                "different Patchbay workspace",
            ),
        ];
        for (error, recovery_scope) in cases {
            let failure = classify_slack_install_persist_error(&anyhow::Error::new(error));
            assert_eq!(failure.status, StatusCode::CONFLICT);
            assert!(failure.message.contains(recovery_scope));
        }

        let internal = classify_slack_install_persist_error(&anyhow::anyhow!("database down"));
        assert_eq!(internal.status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn lark_poll_errors_retry_only_transport_failures() {
        let protocol = anyhow::Error::new(patchbay_lark::registration::RegistrationError {
            code: "http_502".into(),
            description: "invalid response".into(),
        });
        assert!(lark_poll_protocol_error(&protocol));

        let transport = anyhow::anyhow!("registration: http do: connection reset");
        assert!(!lark_poll_protocol_error(&transport));
    }

    #[tokio::test]
    async fn installation_routes_require_user_authentication() {
        let workspace_id = Uuid::new_v4();
        let response = crate::build_router(None, None)
            .oneshot(
                Request::get(format!(
                    "/api/workspaces/{workspace_id}/telegram/installations"
                ))
                .body(Body::empty())
                .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn guest_installation_requests_are_rejected_at_the_handler_boundary() {
        let mut headers = HeaderMap::new();
        headers.insert("x-guest-user", "true".parse().expect("header value"));
        let response = require_formal_user(&headers).expect_err("guest must be rejected");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn lark_install_session_redis_key_is_hash_tagged() {
        let session_id = "11111111-1111-1111-1111-111111111111";
        assert_eq!(
            LarkSessionStore::key(session_id),
            format!("patchbay:{{lark_install_session}}:{session_id}")
        );
    }

    #[test]
    fn weixin_redirects_are_https_and_tencent_scoped() {
        assert_eq!(
            validate_weixin_redirect("ilinkai.weixin.qq.com"),
            Some("https://ilinkai.weixin.qq.com".into())
        );
        assert_eq!(
            validate_weixin_redirect("https://sh.ilink.weixin.qq.com/path?ignored=1"),
            Some("https://sh.ilink.weixin.qq.com".into())
        );
        assert!(validate_weixin_redirect("http://ilinkai.weixin.qq.com").is_none());
        assert!(validate_weixin_redirect("https://ilinkai.weixin.qq.com:8443").is_none());
        assert!(validate_weixin_redirect("https://weixin.qq.com.evil.test").is_none());
    }

    #[tokio::test]
    async fn memory_lark_sessions_round_trip_and_finish() {
        let store = LarkSessionStore { redis: None };
        let session_id = Uuid::new_v4().to_string();
        let workspace_id = Uuid::new_v4();
        let initiator_id = Uuid::new_v4();
        store
            .insert(
                &session_id,
                LarkSession {
                    workspace_id,
                    initiator_id,
                    status: "pending".into(),
                    installation_id: None,
                    error_reason: None,
                    error_message: None,
                    expires_at: chrono::Utc::now() + chrono::Duration::minutes(15),
                },
            )
            .await
            .expect("insert");
        let loaded = store.get(&session_id).await.expect("get").expect("present");
        assert_eq!(loaded.workspace_id, workspace_id);
        assert_eq!(loaded.status, "pending");
        let installation_id = Uuid::new_v4();
        store
            .finish(&session_id, Some(installation_id), None, None)
            .await;
        let finished = store.get(&session_id).await.expect("get").expect("present");
        assert_eq!(finished.status, "success");
        assert_eq!(finished.installation_id, Some(installation_id));
        store.remove(&session_id).await;
        assert!(store.get(&session_id).await.expect("get").is_none());
    }
}
