//! Workspace connector installation management. Secret-bearing writes are
//! gated on a valid per-provider secretbox key and never return stored config.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use base64::Engine as _;
use cordy_db::models::ChannelInstallation;
use cordy_db::queries::{agent, channel, dingtalk};
use cordy_lark::client::ApiClient as _;
use cordy_middleware::workspace::WorkspaceContext;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{error::error_response, state::HandlerState};

const BODY_LIMIT: usize = 16 * 1024;

#[derive(Clone, Copy)]
enum Provider {
    DingTalk,
    Lark,
    Slack,
    Telegram,
    WeCom,
}

impl Provider {
    fn channel_type(self) -> &'static str {
        match self {
            Self::DingTalk => cordy_dingtalk::TYPE_DINGTALK,
            Self::Lark => cordy_lark::channel_store::CHANNEL_TYPE_FEISHU,
            Self::Slack => cordy_slack::TYPE_SLACK,
            Self::Telegram => cordy_telegram::TYPE_TELEGRAM,
            Self::WeCom => cordy_wecom::CHANNEL_TYPE_WECOM,
        }
    }

    fn key_env(self) -> &'static str {
        match self {
            Self::DingTalk => "CORDY_DINGTALK_SECRET_KEY",
            Self::Lark => "CORDY_LARK_SECRET_KEY",
            Self::Slack => "CORDY_SLACK_SECRET_KEY",
            Self::Telegram => "CORDY_TELEGRAM_SECRET_KEY",
            Self::WeCom => "CORDY_WECOM_SECRET_KEY",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::DingTalk => "dingtalk",
            Self::Lark => "lark",
            Self::Slack => "slack",
            Self::Telegram => "telegram",
            Self::WeCom => "wecom",
        }
    }

    fn created_event(self) -> &'static str {
        match self {
            Self::DingTalk => cordy_protocol::EVENT_DINGTALK_INSTALLATION_CREATED,
            Self::Lark => cordy_protocol::EVENT_LARK_INSTALLATION_CREATED,
            Self::Slack => cordy_protocol::EVENT_SLACK_INSTALLATION_CREATED,
            Self::Telegram => cordy_protocol::EVENT_TELEGRAM_INSTALLATION_CREATED,
            Self::WeCom => cordy_protocol::EVENT_WECOM_INSTALLATION_CREATED,
        }
    }

    fn revoked_event(self) -> &'static str {
        match self {
            Self::DingTalk => cordy_protocol::EVENT_DINGTALK_INSTALLATION_REVOKED,
            Self::Lark => cordy_protocol::EVENT_LARK_INSTALLATION_REVOKED,
            Self::Slack => cordy_protocol::EVENT_SLACK_INSTALLATION_REVOKED,
            Self::Telegram => cordy_protocol::EVENT_TELEGRAM_INSTALLATION_REVOKED,
            Self::WeCom => cordy_protocol::EVENT_WECOM_INSTALLATION_REVOKED,
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

fn secret_box(provider: Provider) -> Option<cordy_util::secretbox::SecretBox> {
    let key = cordy_util::secretbox::load_key(provider.key_env()).ok()?;
    cordy_util::secretbox::SecretBox::new(&key).ok()
}

fn user_id(headers: &HeaderMap) -> Result<Uuid, Response> {
    headers
        .get("x-user-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| error_response(StatusCode::UNAUTHORIZED, "user not authenticated"))
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
        Provider::Lark => json!({
            "app_id": config.get("app_id").and_then(Value::as_str).unwrap_or(""),
            "tenant_key": config.get("tenant_key").and_then(Value::as_str),
            "bot_open_id": config.get("bot_open_id").and_then(Value::as_str).unwrap_or(""),
            "region": config.get("region").and_then(Value::as_str).unwrap_or("feishu"),
        }),
        Provider::Slack => {
            let value = cordy_slack::config::decode_public_config(config);
            json!({"team_id": value.team_id, "bot_user_id": value.bot_user_id})
        }
        Provider::Telegram => {
            let raw = serde_json::to_vec(config).unwrap_or_default();
            let value = cordy_telegram::decode_public_config(&raw);
            json!({"bot_id": value.bot_id, "bot_username": value.bot_username})
        }
        Provider::WeCom => {
            json!({"bot_id": config.get("bot_id").or_else(|| config.get("app_id")).and_then(Value::as_str).unwrap_or("")})
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
    match channel::list_channel_installations_by_workspace(&state.pool, workspace_id, provider.channel_type()).await {
        Ok(rows) => Json(json!({
            "installations": rows.into_iter().map(|row| installation_response(provider, row)).collect::<Vec<_>>(),
            "configured": true,
            "install_supported": true,
        })).into_response(),
        Err(error) => {
            tracing::error!(%error, provider = provider.label(), "list connector installations failed");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list installations")
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
list_handler!(list_dingtalk, Provider::DingTalk);
list_handler!(list_lark, Provider::Lark);
list_handler!(list_slack, Provider::Slack);
list_handler!(list_telegram, Provider::Telegram);
list_handler!(list_wecom, Provider::WeCom);

#[derive(Clone)]
struct LarkSession {
    workspace_id: Uuid,
    initiator_id: Uuid,
    status: &'static str,
    installation_id: Option<Uuid>,
    error_reason: Option<String>,
    error_message: Option<String>,
    expires_at: Instant,
}

struct LarkRegistrationRuntime {
    pool: sqlx::PgPool,
    bus: Arc<cordy_events::Bus>,
    http_base_url: String,
    cancel: CancellationToken,
}

fn lark_sessions() -> &'static Mutex<HashMap<String, LarkSession>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, LarkSession>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
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
        "" | "feishu" => cordy_lark::types::Region::Feishu,
        "lark" => cordy_lark::types::Region::Lark,
        _ => return error_response(StatusCode::BAD_REQUEST, "region must be 'feishu' or 'lark'"),
    };
    let target = match agent::get_agent_in_workspace(&state.pool, agent_id, workspace_id).await {
        Ok(Some(value)) => value,
        _ => return error_response(StatusCode::NOT_FOUND, "agent not found in this workspace"),
    };
    if !matches!(context.member.role.as_str(), "owner" | "admin") && target.owner_id != Some(actor)
    {
        return error_response(StatusCode::FORBIDDEN, "not allowed to manage this agent");
    }
    let preset = if target.name.trim().is_empty() {
        "Cordy".into()
    } else {
        format!("{} - Cordy", target.name.trim())
    };
    let client = Arc::new(cordy_lark::registration::RegistrationClient::new(
        cordy_lark::registration::RegistrationConfig {
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
    let mut sessions = lark_sessions().lock().unwrap();
    let now = Instant::now();
    sessions.retain(|_, session| session.expires_at > now);
    if sessions.len() >= 1024 {
        return error_response(StatusCode::TOO_MANY_REQUESTS, "too many install sessions");
    }
    sessions.insert(
        session_id.clone(),
        LarkSession {
            workspace_id,
            initiator_id: actor,
            status: "pending",
            installation_id: None,
            error_reason: None,
            error_message: None,
            expires_at: now + Duration::from_secs(15 * 60),
        },
    );
    drop(sessions);
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
    };
    if !state.channel_tasks.spawn(run_lark_registration(
        runtime,
        client,
        task_session,
        (workspace_id, agent_id, actor),
        region,
        begun.clone(),
    )) {
        lark_sessions().lock().unwrap().remove(&session_id);
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "channel runtime is shutting down",
        );
    }
    Json(
        json!({"session_id": session_id, "qr_code_url": begun.qr_code_url,
        "expires_in_seconds": expires.as_secs(), "poll_interval_seconds": poll_interval}),
    )
    .into_response()
}

async fn run_lark_registration(
    runtime: LarkRegistrationRuntime,
    client: Arc<cordy_lark::registration::RegistrationClient>,
    session_id: String,
    identity: (Uuid, Uuid, Uuid),
    mut region: cordy_lark::types::Region,
    begun: cordy_lark::registration::BeginResult,
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
            finish_lark_session(
                &session_id,
                None,
                Some("expired"),
                Some("install session expired"),
            );
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
                finish_lark_session(
                    &session_id,
                    None,
                    Some("lark_protocol_error"),
                    Some(&format!("{error:#}")),
                );
                return;
            }
            Err(error) => {
                // A short-lived DNS, connect, timeout, or response-body read
                // failure must not invalidate an otherwise live device code.
                // Keep the original Lark deadline and retry on the next tick,
                // matching the Go registration service. Typed protocol errors
                // remain terminal above because another poll cannot repair a
                // malformed or explicitly rejected exchange.
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
            finish_lark_session(&session_id, None, Some(reason), Some(&error.to_string()));
            return;
        }
        if result.status == "slow_down" {
            interval += std::time::Duration::from_secs(5);
            continue;
        }
        if result.client_id.is_empty() {
            continue;
        }
        let api = cordy_lark::http_client::HttpApiClient::new(
            cordy_lark::http_client::HttpClientConfig {
                base_url: runtime.http_base_url.clone(),
                ..Default::default()
            },
        );
        let credentials = cordy_lark::client::InstallationCredentials {
            app_id: result.client_id.clone(),
            app_secret: result.client_secret.clone(),
            tenant_key: String::new(),
            region,
        };
        let bot = match api.get_bot_info(credentials).await {
            Ok(value) => value,
            Err(error) => {
                finish_lark_session(
                    &session_id,
                    None,
                    Some("bot_info_failed"),
                    Some(&format!("{error:#}")),
                );
                return;
            }
        };
        let box_ = match secret_box(Provider::Lark) {
            Some(value) => value,
            None => {
                finish_lark_session(
                    &session_id,
                    None,
                    Some("not_configured"),
                    Some("lark install not configured"),
                );
                return;
            }
        };
        let sealed = match box_.seal(result.client_secret.as_bytes()) {
            Ok(value) => value,
            Err(error) => {
                finish_lark_session(
                    &session_id,
                    None,
                    Some("encryption_failed"),
                    Some(&error.to_string()),
                );
                return;
            }
        };
        let mut tx = match runtime.pool.begin().await {
            Ok(value) => value,
            Err(error) => {
                finish_lark_session(
                    &session_id,
                    None,
                    Some("persist_failed"),
                    Some(&error.to_string()),
                );
                return;
            }
        };
        let persisted = async {
            cordy_lark::channel_store::reclaim_dead_installation_with(
                &mut *tx,
                workspace_id,
                agent_id,
                &result.client_id,
            )
            .await?;
            let installation = cordy_lark::channel_store::upsert_lark_installation_with(
                &mut *tx,
                cordy_lark::params::UpsertInstallationParams {
                    workspace_id,
                    agent_id,
                    app_id: result.client_id,
                    app_secret_encrypted: sealed,
                    bot_open_id: bot.open_id.0,
                    installer_user_id: actor,
                    tenant_key: None,
                    bot_union_id: (!bot.union_id.is_empty()).then_some(bot.union_id),
                    region: region.as_str().into(),
                },
            )
            .await?;
            cordy_lark::channel_store::create_lark_user_binding_with(
                &mut *tx,
                cordy_lark::params::CreateUserBindingParams {
                    workspace_id,
                    cordy_user_id: actor,
                    installation_id: installation.id,
                    channel_user_id: result.open_id.0,
                    union_id: None,
                },
            )
            .await?;
            Ok::<_, anyhow::Error>(installation)
        }
        .await;
        let installation = match persisted {
            Ok(value) if tx.commit().await.is_ok() => value,
            Ok(_) => {
                finish_lark_session(
                    &session_id,
                    None,
                    Some("persist_failed"),
                    Some("failed to commit installation"),
                );
                return;
            }
            Err(error) => {
                finish_lark_session(
                    &session_id,
                    None,
                    Some("persist_failed"),
                    Some(&format!("{error:#}")),
                );
                return;
            }
        };
        runtime.bus.publish(&cordy_events::Event {
            event_type: cordy_protocol::EVENT_LARK_INSTALLATION_CREATED.into(),
            workspace_id: workspace_id.to_string(),
            actor_type: "system".into(),
            payload: json!({"installation_id": installation.id}),
            ..Default::default()
        });
        finish_lark_session(&session_id, Some(installation.id), None, None);
        return;
    }
}

fn lark_poll_protocol_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<cordy_lark::registration::RegistrationError>()
        .is_some()
}

fn finish_lark_session(
    session_id: &str,
    installation_id: Option<Uuid>,
    reason: Option<&str>,
    message: Option<&str>,
) {
    if let Some(session) = lark_sessions().lock().unwrap().get_mut(session_id) {
        session.status = if installation_id.is_some() {
            "success"
        } else {
            "error"
        };
        session.installation_id = installation_id;
        session.error_reason = reason.map(str::to_string);
        session.error_message = message.map(str::to_string);
    }
}

async fn lark_install_status(
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Path((_workspace, session_id)): Path<(String, String)>,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let actor = match user_id(&headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mut sessions = lark_sessions().lock().unwrap();
    let now = Instant::now();
    sessions.retain(|_, session| session.expires_at > now);
    let session = sessions.get(session_id.trim()).cloned();
    drop(sessions);
    let Some(session) = session.filter(|value| value.workspace_id == workspace_id) else {
        return error_response(StatusCode::NOT_FOUND, "install session not found");
    };
    if session.initiator_id != actor && !matches!(context.member.role.as_str(), "owner" | "admin") {
        return error_response(StatusCode::NOT_FOUND, "install session not found");
    }
    Json(
        json!({"status": session.status, "installation_id": session.installation_id,
        "error_reason": session.error_reason, "error_message": session.error_message}),
    )
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
            )
        }
    };
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_DINGTALK_GROUP_ROUTE_UPDATED.into(),
        workspace_id: workspace_id.to_string(),
        actor_type: "user".into(),
        actor_id: actor.to_string(),
        payload: json!({"id": route_id}),
        ..Default::default()
    });
    Json(json!({"id": row.id, "workspace_id": row.workspace_id, "installation_id": row.installation_id,
        "conversation_id": row.conversation_id, "conversation_title": row.conversation_title,
        "agent_id": row.agent_id, "discovered_at": row.discovered_at.map(crate::timefmt::rfc3339),
        "updated_at": row.updated_at.map(crate::timefmt::rfc3339)})).into_response()
}

async fn revoke(
    state: HandlerState,
    context: WorkspaceContext,
    headers: HeaderMap,
    raw_id: String,
    provider: Provider,
) -> Response {
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
            )
        }
    };
    if matches!(provider, Provider::Lark)
        && !matches!(context.member.role.as_str(), "owner" | "admin")
    {
        let owns_agent = matches!(agent::get_agent_in_workspace(&state.pool, installation.agent_id, workspace_id).await, Ok(Some(value)) if value.owner_id == Some(actor));
        if !owns_agent {
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
    state.bus.publish(&cordy_events::Event {
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
    let workspace_id = workspace_id(context)?;
    let actor = user_id(headers)?;
    let raw = query
        .agent_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "agent_id is required"))?;
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
    state.bus.publish(&cordy_events::Event {
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
    use cordy_dingtalk::byo_install::ByoError;
    use cordy_dingtalk::install::InstallError;

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
            "this DingTalk robot is already connected to a different Cordy workspace — disconnect it there before connecting it here".into(),
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
    let service = cordy_dingtalk::byo_install::ByoInstallService::new(
        state.pool.clone(),
        Arc::new(box_),
        None,
        "",
    );
    match service
        .register_byo(cordy_dingtalk::byo_install::RegisterByoParams {
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
    let service = cordy_wecom::installation::InstallationService::new(state.pool.clone(), box_);
    let cancel = CancellationToken::new();
    match service
        .upsert(
            &cancel,
            &cordy_wecom::installation::InstallationParams {
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
                    )
                }
            };
            publish_created(&state, Provider::WeCom, &row, actor);
            Json(installation_response(Provider::WeCom, row)).into_response()
        }
        Err(error) => {
            let status = if error
                .downcast_ref::<cordy_wecom::installation::InvalidInstallationParams>()
                .is_some()
                || cordy_wecom::credential_probe::is_credentials_rejected(&error)
            {
                StatusCode::BAD_REQUEST
            } else if cordy_wecom::credential_probe::is_credentials_unverifiable(&error) {
                StatusCode::SERVICE_UNAVAILABLE
            } else if cordy_wecom::installation::as_bot_ownership_error(&error).is_some() {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            tracing::warn!(error = %error, "WeCom installation failed");
            error_response(status, "could not connect the WeCom bot")
        }
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
    let bot_id = match cordy_telegram::parse_bot_id(token) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "telegram: bot token must look like 123456:ABC-DEF…",
            )
        }
    };
    let api = cordy_telegram::BotApi::new("", token);
    let me = match api.get_me().await {
        Ok(value) if value.is_bot && !value.username.is_empty() => value,
        Ok(_) => {
            return error_response(StatusCode::BAD_REQUEST, "Telegram rejected this bot token")
        }
        Err(error) => {
            tracing::warn!(%error, "Telegram credential verification failed");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "could not reach Telegram to verify this bot",
            );
        }
    };
    match api.get_webhook_info().await {
        Ok(value) if !value.url.is_empty() => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "this Telegram bot has a webhook configured",
            )
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, "Telegram webhook verification failed");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "could not reach Telegram to verify this bot",
            );
        }
    }
    let sealed = match box_.seal(token.as_bytes()) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to encrypt bot token",
            )
        }
    };
    let config = json!({"app_id": bot_id, "bot_username": me.username, "bot_token_encrypted": base64::engine::general_purpose::STANDARD.encode(sealed)});
    let persist = match cordy_telegram::install::InstallPersist::new(
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
    match cordy_telegram::install::InstallService::new(state.pool.clone())
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
struct TelegramInstallPersistFailure {
    status: StatusCode,
    message: &'static str,
}

fn classify_telegram_install_persist_error(error: &anyhow::Error) -> TelegramInstallPersistFailure {
    use cordy_telegram::install::InstallError;

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
            message: "this Telegram bot is already connected to a different Cordy workspace — disconnect it there before connecting it here",
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
            )
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
            return error_response(StatusCode::BAD_REQUEST, "could not verify the Slack tokens")
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
            return error_response(StatusCode::BAD_REQUEST, "could not verify the Slack tokens")
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
            )
        }
    };
    let sealed_app = match box_.seal(app_token.as_bytes()) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to encrypt Slack token",
            )
        }
    };
    let config = json!({"app_id": app_id, "team_id": auth.team_id, "bot_user_id": auth.user_id, "bot_token_encrypted": base64::engine::general_purpose::STANDARD.encode(sealed_bot), "app_token_encrypted": base64::engine::general_purpose::STANDARD.encode(sealed_app)});
    let persist = match cordy_slack::install::InstallPersist::from_config(
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
    match cordy_slack::install::InstallService::new(state.pool.clone())
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
    use cordy_slack::install::InstallError;

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
            message: "this Slack app is already connected to a different Cordy workspace — disconnect it there before connecting it here",
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
    fn dingtalk_install_errors_preserve_client_and_server_classifications() {
        use cordy_dingtalk::byo_install::ByoError;
        use cordy_dingtalk::install::InstallError;

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
    fn slack_persist_errors_preserve_owner_recovery_path() {
        use cordy_slack::install::InstallError;

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
                "different Cordy workspace",
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
    fn telegram_persist_errors_preserve_owner_recovery_path() {
        use cordy_telegram::install::InstallError;

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
                "different Cordy workspace",
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
    fn lark_poll_errors_retry_only_transport_failures() {
        let protocol = anyhow::Error::new(cordy_lark::registration::RegistrationError {
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
}
