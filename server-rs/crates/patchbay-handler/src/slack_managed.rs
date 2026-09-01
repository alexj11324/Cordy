//! Official hosted Slack installation and Events API ingress.
//!
//! Self-hosted deployments stay on the Multica-style BYO Socket Mode path in
//! `connectors.rs`. This module is enabled only for managed messaging and owns
//! the account-level OAuth state, multi-tenant install, and signed webhook.

use std::time::Duration;

use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use patchbay_middleware::workspace::WorkspaceContext;
use rand::RngCore as _;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use url::Url;
use uuid::Uuid;

use crate::connectors::{self, Provider};
use crate::error::{error_code_response, error_response};
use crate::state::HandlerState;

const OAUTH_STATE_TTL: chrono::Duration = chrono::Duration::minutes(10);
const SLACK_HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const SLACK_INGRESS_ACCEPT_TIMEOUT: Duration = Duration::from_millis(2500);
const SLACK_SIGNATURE_MAX_AGE_SECS: i64 = 5 * 60;
const MANAGED_SLACK_OBSERVER_TOKEN: &str = "managed:slack:webhook:v1";
const SLACK_BOT_SCOPES: &str = "app_mentions:read,channels:history,chat:write,commands,files:read,groups:history,im:history,mpim:history,reactions:write,users:read";

#[derive(Debug, Deserialize)]
pub(crate) struct BeginInstallInput {
    redirect_url: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OAuthCallbackQuery {
    #[serde(default)]
    code: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    error: String,
}

#[derive(Debug, Deserialize)]
struct OAuthAccessResponse {
    ok: bool,
    #[serde(default)]
    error: String,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    app_id: String,
    #[serde(default)]
    bot_user_id: String,
    #[serde(default)]
    team: OAuthTeam,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: i64,
}

#[derive(Debug, Deserialize)]
struct SlackApiResponse {
    ok: bool,
    #[serde(default)]
    error: String,
}

#[derive(Debug, Default, Deserialize)]
struct OAuthTeam {
    #[serde(default)]
    id: String,
}

#[derive(Debug)]
struct ClaimedOAuthState {
    workspace_id: Uuid,
    installer_user_id: Uuid,
    redirect_url: String,
}

#[derive(Debug, Deserialize)]
struct WebhookEnvelope {
    #[serde(rename = "type", default)]
    envelope_type: String,
    #[serde(default)]
    challenge: String,
}

pub fn public_router() -> Router<HandlerState> {
    Router::new()
        .route(
            "/api/integrations/slack/oauth/callback",
            get(oauth_callback),
        )
        .route("/api/integrations/slack/events", post(events_api))
        .route("/api/integrations/slack/commands", post(commands_api))
}

pub(crate) async fn begin_install(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Json(input): Json<BeginInstallInput>,
) -> Response {
    if let Err(response) = connectors::require_setup_writable(&state) {
        return response;
    }
    if state.public_config.messaging.mode != "managed" {
        return error_code_response(
            StatusCode::FORBIDDEN,
            "server_managed_integration",
            "Slack OAuth is available only on the managed gateway",
        );
    }
    if let Err(response) = connectors::require_formal_user(&headers) {
        return response;
    }
    let workspace_id = match connectors::workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let actor = match connectors::user_id(&headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let config = match ManagedSlackConfig::from_state(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let redirect_url =
        match validate_app_redirect(&input.redirect_url, &state.public_config.messaging_bind_url) {
            Some(value) => value,
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "redirect_url must use the configured public app origin",
                );
            }
        };

    let mut raw_state = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw_state);
    let oauth_state = hex::encode(raw_state);
    let state_hash = hash_state(&oauth_state);
    let expires_at = Utc::now() + OAUTH_STATE_TTL;
    let inserted = sqlx::query(
        r#"WITH purged AS (
    DELETE FROM slack_oauth_state WHERE expires_at <= now()
)
INSERT INTO slack_oauth_state (
    state_hash, workspace_id, installer_user_id, redirect_url, expires_at
) VALUES ($1, $2, $3, $4, $5)"#,
    )
    .bind(state_hash)
    .bind(workspace_id)
    .bind(actor)
    .bind(redirect_url.as_str())
    .bind(expires_at)
    .execute(&state.pool)
    .await;
    if let Err(error) = inserted {
        tracing::error!(%error, %workspace_id, "failed to persist Slack OAuth state");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to start Slack authorization",
        );
    }

    let mut authorization_url = match Url::parse("https://slack.com/oauth/v2/authorize") {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to start Slack authorization",
            );
        }
    };
    authorization_url
        .query_pairs_mut()
        .append_pair("client_id", &config.client_id)
        .append_pair("scope", SLACK_BOT_SCOPES)
        .append_pair("redirect_uri", &config.redirect_uri)
        .append_pair("state", &oauth_state);
    Json(json!({"authorization_url": authorization_url.as_str()})).into_response()
}

pub(crate) async fn revoke_install(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    Path((_workspace, installation_id)): Path<(String, String)>,
) -> Response {
    if state.public_config.messaging.mode != "managed" {
        return connectors::revoke(state, context, headers, installation_id, Provider::Slack).await;
    }
    if let Err(response) = connectors::require_setup_writable(&state) {
        return response;
    }
    if let Err(response) = connectors::require_formal_user(&headers) {
        return response;
    }
    let workspace_id = match connectors::workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let installation_uuid = match Uuid::parse_str(&installation_id) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid installation id"),
    };
    let installation = match patchbay_db::queries::channel::get_channel_installation_in_workspace(
        &state.pool,
        installation_uuid,
        workspace_id,
        patchbay_slack::TYPE_SLACK,
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "installation not found"),
        Err(error) => {
            tracing::error!(%error, %installation_uuid, "failed to load Slack installation for revoke");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to disconnect Slack",
            );
        }
    };
    let cfg: patchbay_slack::config::InstallConfig =
        match serde_json::from_value(installation.config.clone()) {
            Ok(value) => value,
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to disconnect Slack",
                );
            }
        };
    if cfg.transport == "webhook" && installation.status == "active" {
        let managed = match ManagedSlackConfig::from_state(&state) {
            Ok(value) => value,
            Err(response) => return response,
        };
        let box_ = match slack_secret_box() {
            Ok(value) => value,
            Err(_) => return not_configured(),
        };
        let decrypt = move |sealed: &[u8]| box_.open(sealed).map_err(anyhow::Error::from);
        let token =
            match patchbay_slack::config::decrypt_token(&cfg.bot_token_encrypted, Some(&decrypt)) {
                Ok(value) if !value.is_empty() => value,
                _ => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to disconnect Slack",
                    );
                }
            };
        let client = match reqwest::Client::builder()
            .timeout(SLACK_HTTP_TIMEOUT)
            .build()
        {
            Ok(value) => value,
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to disconnect Slack",
                );
            }
        };
        match uninstall_slack_app(&client, &managed, &token).await {
            Ok(value)
                if value.ok
                    || matches!(
                        value.error.as_str(),
                        "token_revoked" | "account_inactive" | "invalid_auth"
                    ) => {}
            Ok(value) => {
                tracing::warn!(error = value.error, %installation_uuid, "Slack uninstall rejected");
                return error_response(StatusCode::BAD_GATEWAY, "Slack uninstall failed");
            }
            Err(error) => {
                tracing::warn!(%error, %installation_uuid, "Slack uninstall request failed");
                return error_response(StatusCode::BAD_GATEWAY, "Slack uninstall failed");
            }
        }
    }
    connectors::revoke(state, context, headers, installation_id, Provider::Slack).await
}

async fn oauth_callback(
    State(state): State<HandlerState>,
    Query(query): Query<OAuthCallbackQuery>,
) -> Response {
    if query.state.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "missing Slack OAuth state");
    }
    let claimed = match claim_oauth_state(&state, &query.state).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            return error_code_response(
                StatusCode::BAD_REQUEST,
                "invalid_oauth_state",
                "Slack authorization expired or was already used",
            );
        }
        Err(error) => {
            tracing::error!(%error, "failed to claim Slack OAuth state");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to finish Slack authorization",
            );
        }
    };
    let still_authorized = patchbay_db::queries::member::get_member_by_user_and_workspace(
        &state.pool,
        claimed.installer_user_id,
        claimed.workspace_id,
    )
    .await
    .ok()
    .flatten()
    .is_some_and(|member| can_manage(&member.role));
    if !still_authorized {
        return redirect_slack_error(&claimed.redirect_url, "slack_authorization_changed");
    }
    if !query.error.is_empty() {
        return redirect_slack_error(&claimed.redirect_url, "slack_authorization_denied");
    }
    if query.code.trim().is_empty() {
        return redirect_slack_error(&claimed.redirect_url, "slack_code_missing");
    }
    let config = match ManagedSlackConfig::from_state(&state) {
        Ok(value) => value,
        Err(_) => {
            return redirect_slack_error(&claimed.redirect_url, "slack_not_configured");
        }
    };
    let client = match reqwest::Client::builder()
        .timeout(SLACK_HTTP_TIMEOUT)
        .build()
    {
        Ok(value) => value,
        Err(_) => {
            return redirect_slack_error(&claimed.redirect_url, "slack_exchange_failed");
        }
    };
    let exchanged = client
        .post("https://slack.com/api/oauth.v2.access")
        .basic_auth(&config.client_id, Some(&config.client_secret))
        .form(&[
            ("code", query.code.trim()),
            ("redirect_uri", config.redirect_uri.as_str()),
        ])
        .send()
        .await
        .and_then(reqwest::Response::error_for_status);
    let response = match exchanged {
        Ok(response) => match response.json::<OAuthAccessResponse>().await {
            Ok(value) if value.ok => value,
            Ok(value) => {
                tracing::warn!(error = value.error, "Slack OAuth exchange rejected");
                return redirect_slack_error(&claimed.redirect_url, "slack_exchange_rejected");
            }
            Err(error) => {
                tracing::warn!(%error, "Slack OAuth response decode failed");
                return redirect_slack_error(&claimed.redirect_url, "slack_exchange_failed");
            }
        },
        Err(error) => {
            tracing::warn!(%error, "Slack OAuth exchange failed");
            return redirect_slack_error(&claimed.redirect_url, "slack_exchange_failed");
        }
    };
    if response.access_token.is_empty()
        || response.app_id.is_empty()
        || response.team.id.is_empty()
        || response.bot_user_id.is_empty()
    {
        compensate_failed_install(&client, &config, &response.access_token).await;
        return redirect_slack_error(&claimed.redirect_url, "slack_exchange_incomplete");
    }

    let box_ = match slack_secret_box() {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "managed Slack token encryption unavailable");
            compensate_failed_install(&client, &config, &response.access_token).await;
            return redirect_slack_error(&claimed.redirect_url, "slack_not_configured");
        }
    };
    let bot_token_encrypted = match seal_base64(&box_, response.access_token.as_bytes()) {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "failed to encrypt managed Slack bot token");
            compensate_failed_install(&client, &config, &response.access_token).await;
            return redirect_slack_error(&claimed.redirect_url, "slack_persist_failed");
        }
    };
    let refresh_token_encrypted = if response.refresh_token.is_empty() {
        String::new()
    } else {
        match seal_base64(&box_, response.refresh_token.as_bytes()) {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(%error, "failed to encrypt managed Slack refresh token");
                compensate_failed_install(&client, &config, &response.access_token).await;
                return redirect_slack_error(&claimed.redirect_url, "slack_persist_failed");
            }
        }
    };
    let token_expires_at = (response.expires_in > 0)
        .then(|| Utc::now() + chrono::Duration::seconds(response.expires_in));
    let routing_key =
        patchbay_slack::config::managed_routing_key(&response.app_id, &response.team.id);
    let installation_config = json!({
        "app_id": routing_key,
        "api_app_id": response.app_id,
        "team_id": response.team.id,
        "bot_user_id": response.bot_user_id,
        "bot_token_encrypted": bot_token_encrypted,
        "app_token_encrypted": "",
        "transport": "webhook",
        "refresh_token_encrypted": refresh_token_encrypted,
        "token_expires_at": token_expires_at,
    });
    let persist = match patchbay_slack::install::InstallPersist::from_config(
        claimed.workspace_id,
        Uuid::nil(),
        claimed.installer_user_id,
        installation_config,
    ) {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "failed to compose managed Slack installation");
            compensate_failed_install(&client, &config, &response.access_token).await;
            return redirect_slack_error(&claimed.redirect_url, "slack_persist_failed");
        }
    };
    let installation = patchbay_slack::install::InstallService::new(state.pool.clone())
        .persist_install_with_limit(&persist, connectors::hosted_installation_limit(&state))
        .await;
    let row = match installation {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, workspace_id = %claimed.workspace_id, "managed Slack installation persist failed");
            compensate_failed_install(&client, &config, &response.access_token).await;
            let code = if error
                .to_string()
                .contains("hosted messaging installation limit reached")
            {
                "im_installation_limit_reached"
            } else {
                "slack_persist_failed"
            };
            return redirect_slack_error(&claimed.redirect_url, code);
        }
    };
    if let Err(error) = patchbay_db::queries::channel::upsert_channel_runtime_observation(
        &state.pool,
        row.id,
        MANAGED_SLACK_OBSERVER_TOKEN,
        "starting",
        None,
        None,
    )
    .await
    {
        tracing::warn!(%error, installation_id = %row.id, "failed to initialize managed Slack runtime health");
    }
    connectors::publish_created(&state, Provider::Slack, &row, claimed.installer_user_id);
    redirect_result(&claimed.redirect_url, "slack_connected", "1")
}

async fn claim_oauth_state(
    state: &HandlerState,
    raw_state: &str,
) -> anyhow::Result<Option<ClaimedOAuthState>> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, String)>(
        r#"DELETE FROM slack_oauth_state
WHERE state_hash = $1 AND expires_at > now()
RETURNING workspace_id, installer_user_id, redirect_url"#,
    )
    .bind(hash_state(raw_state))
    .fetch_optional(&state.pool)
    .await?;
    Ok(row.map(
        |(workspace_id, installer_user_id, redirect_url)| ClaimedOAuthState {
            workspace_id,
            installer_user_id,
            redirect_url,
        },
    ))
}

async fn events_api(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if state.public_config.messaging.mode != "managed" {
        return error_response(
            StatusCode::NOT_FOUND,
            "managed Slack events are not enabled",
        );
    }
    let Some(signing_secret) = state
        .integrations
        .slack_signing_secret
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "managed Slack events are not configured",
        );
    };
    if !verify_slack_signature(signing_secret, &headers, &body, Utc::now()) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid Slack signature");
    }
    let outer: WebhookEnvelope = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid Slack event"),
    };
    if outer.envelope_type == "url_verification" {
        if outer.challenge.is_empty() {
            return error_response(StatusCode::BAD_REQUEST, "missing Slack challenge");
        }
        return Json(json!({"challenge": outer.challenge})).into_response();
    }
    if outer.envelope_type != "event_callback" {
        return Json(json!({"ok": true})).into_response();
    }
    let envelope: patchbay_slack::inbound::EventsApiEnvelope = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid Slack event"),
    };
    if envelope.api_app_id.is_empty() || envelope.team_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "incomplete Slack event identity");
    }
    let routing_key =
        patchbay_slack::config::managed_routing_key(&envelope.api_app_id, &envelope.team_id);
    let installation = match patchbay_db::queries::channel::get_channel_installation_by_app_id(
        &state.pool,
        patchbay_slack::TYPE_SLACK,
        &routing_key,
    )
    .await
    {
        Ok(Some(value)) if value.status == "active" => value,
        Ok(_) => {
            tracing::warn!(
                team_id = envelope.team_id,
                api_app_id = envelope.api_app_id,
                "Slack event has no active managed installation"
            );
            return Json(json!({"ok": true})).into_response();
        }
        Err(error) => {
            tracing::error!(%error, "failed to resolve managed Slack installation");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Slack event routing unavailable",
            );
        }
    };
    if let Err(error) = patchbay_db::queries::channel::upsert_channel_runtime_observation(
        &state.pool,
        installation.id,
        MANAGED_SLACK_OBSERVER_TOKEN,
        "healthy",
        None,
        None,
    )
    .await
    {
        tracing::warn!(%error, installation_id = %installation.id, "failed to record managed Slack webhook health");
    }
    let event_type = envelope
        .event
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if event_type == "app_uninstalled" {
        let mut transaction = match state.pool.begin().await {
            Ok(value) => value,
            Err(error) => {
                tracing::error!(%error, installation_id = %installation.id, "failed to begin Slack uninstall state update");
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Slack uninstall state unavailable",
                );
            }
        };
        let revoked = patchbay_db::queries::channel::set_channel_installation_status(
            &mut *transaction,
            installation.id,
            "revoked",
        )
        .await;
        match revoked {
            Ok(1) => {}
            Ok(rows_affected) => {
                tracing::error!(%rows_affected, installation_id = %installation.id, "Slack app uninstall updated an unexpected row count");
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Slack uninstall state unavailable",
                );
            }
            Err(error) => {
                tracing::error!(%error, installation_id = %installation.id, "failed to record Slack app uninstall");
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Slack uninstall state unavailable",
                );
            }
        }
        if let Err(error) = patchbay_db::queries::channel::upsert_channel_runtime_observation(
            &mut *transaction,
            installation.id,
            "control:revoked",
            "offline",
            Some("installation_revoked"),
            Some("Slack reported that the app was uninstalled."),
        )
        .await
        {
            tracing::error!(%error, installation_id = %installation.id, "failed to record Slack uninstall health");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Slack uninstall state unavailable",
            );
        }
        if let Err(error) = transaction.commit().await {
            tracing::error!(%error, installation_id = %installation.id, "failed to commit Slack uninstall state");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Slack uninstall state unavailable",
            );
        }
        state.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_SLACK_INSTALLATION_REVOKED.into(),
            workspace_id: installation.workspace_id.to_string(),
            actor_type: "integration".into(),
            actor_id: installation.id.to_string(),
            payload: json!({"id": installation.id}),
            ..Default::default()
        });
        return Json(json!({"ok": true})).into_response();
    }
    let public = patchbay_slack::config::decode_public_config(&installation.config);
    let mention_re = patchbay_slack::inbound::compile_mention_re(&public.bot_user_id);
    let inbound = match event_type {
        "message" => {
            serde_json::from_value::<patchbay_slack::inbound::MessageEvent>(envelope.event.clone())
                .ok()
                .and_then(|event| {
                    patchbay_slack::inbound::inbound_from_message(
                        &envelope,
                        &event,
                        &public.bot_user_id,
                        mention_re.as_ref(),
                    )
                })
        }
        "app_mention" => serde_json::from_value::<patchbay_slack::inbound::AppMentionEvent>(
            envelope.event.clone(),
        )
        .ok()
        .and_then(|event| {
            patchbay_slack::inbound::inbound_from_app_mention(
                &envelope,
                &event,
                &public.bot_user_id,
                mention_re.as_ref(),
            )
        }),
        _ => None,
    };
    let Some(inbound) = inbound else {
        return Json(json!({"ok": true})).into_response();
    };
    let Some(handler) = state.channel_inbound_handler.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Slack event handler unavailable",
        );
    };
    let cancel = state.channel_cancel.child_token();
    match tokio::time::timeout(SLACK_INGRESS_ACCEPT_TIMEOUT, handler.call(cancel, inbound)).await {
        Ok(Ok(())) => Json(json!({"ok": true})).into_response(),
        Ok(Err(error)) => {
            tracing::error!(%error, "managed Slack event acceptance failed");
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "Slack event acceptance unavailable",
            )
        }
        Err(_) => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Slack event acceptance timed out",
        ),
    }
}

async fn commands_api(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if state.public_config.messaging.mode != "managed" {
        return error_response(
            StatusCode::NOT_FOUND,
            "managed Slack commands are not enabled",
        );
    }
    let Some(signing_secret) = state
        .integrations
        .slack_signing_secret
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "managed Slack commands are not configured",
        );
    };
    if !verify_slack_signature(signing_secret, &headers, &body, Utc::now()) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid Slack signature");
    }
    let fields = url::form_urlencoded::parse(&body)
        .into_owned()
        .collect::<std::collections::HashMap<String, String>>();
    let command = patchbay_slack::slash_command::SlashCommand {
        command: fields.get("command").cloned().unwrap_or_default(),
        text: fields.get("text").cloned().unwrap_or_default(),
        user_id: fields.get("user_id").cloned().unwrap_or_default(),
        team_id: fields.get("team_id").cloned().unwrap_or_default(),
        api_app_id: fields.get("api_app_id").cloned().unwrap_or_default(),
        channel_id: fields.get("channel_id").cloned().unwrap_or_default(),
        trigger_id: fields.get("trigger_id").cloned().unwrap_or_default(),
        response_url: fields.get("response_url").cloned().unwrap_or_default(),
    };
    match command.command.trim() {
        "/agents" => {
            let Some(inbound) = patchbay_slack::inbound::inbound_from_agents_command(&command)
            else {
                return error_response(StatusCode::BAD_REQUEST, "incomplete Slack command");
            };
            let Some(handler) = state.channel_inbound_handler.clone() else {
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Slack command handler unavailable",
                );
            };
            let cancel = state.channel_cancel.child_token();
            match tokio::time::timeout(SLACK_INGRESS_ACCEPT_TIMEOUT, handler.call(cancel, inbound))
                .await
            {
                Ok(Ok(())) => Json(json!({"response_type": "ephemeral"})).into_response(),
                Ok(Err(error)) => {
                    tracing::error!(%error, "managed Slack /agents acceptance failed");
                    error_response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Slack command acceptance unavailable",
                    )
                }
                Err(_) => error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Slack command acceptance timed out",
                ),
            }
        }
        "/issue" => {
            let Some(processor) = state.slack_slash_processor.clone() else {
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Slack command handler unavailable",
                );
            };
            let cancel = state.channel_cancel.child_token();
            match tokio::time::timeout(
                SLACK_INGRESS_ACCEPT_TIMEOUT,
                processor.response_text(cancel, &command),
            )
            .await
            {
                Ok(text) => Json(json!({
                    "response_type": "ephemeral",
                    "text": text,
                }))
                .into_response(),
                Err(_) => error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Slack command acceptance timed out",
                ),
            }
        }
        _ => error_response(StatusCode::BAD_REQUEST, "unsupported Slack command"),
    }
}

struct ManagedSlackConfig {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
}

async fn uninstall_slack_app(
    client: &reqwest::Client,
    config: &ManagedSlackConfig,
    access_token: &str,
) -> anyhow::Result<SlackApiResponse> {
    Ok(client
        .post("https://slack.com/api/apps.uninstall")
        .bearer_auth(access_token)
        .form(&[
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<SlackApiResponse>()
        .await?)
}

async fn compensate_failed_install(
    client: &reqwest::Client,
    config: &ManagedSlackConfig,
    access_token: &str,
) {
    if access_token.trim().is_empty() {
        return;
    }
    match uninstall_slack_app(client, config, access_token).await {
        Ok(value)
            if value.ok
                || matches!(
                    value.error.as_str(),
                    "token_revoked" | "account_inactive" | "invalid_auth"
                ) => {}
        Ok(value) => {
            tracing::error!(
                error = value.error,
                "Slack failed-install compensation rejected"
            );
        }
        Err(error) => {
            tracing::error!(%error, "Slack failed-install compensation request failed");
        }
    }
}

impl ManagedSlackConfig {
    fn from_state(state: &HandlerState) -> Result<Self, Response> {
        let required = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let Some(client_id) = required(&state.integrations.slack_client_id) else {
            return Err(not_configured());
        };
        let Some(client_secret) = required(&state.integrations.slack_client_secret) else {
            return Err(not_configured());
        };
        let Some(redirect_uri) = required(&state.integrations.slack_oauth_redirect_url) else {
            return Err(not_configured());
        };
        if required(&state.integrations.slack_signing_secret).is_none() {
            return Err(not_configured());
        }
        let valid_redirect = Url::parse(&redirect_uri).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.query().is_none()
                && url.fragment().is_none()
        });
        if !valid_redirect {
            return Err(not_configured());
        }
        if slack_secret_box().is_err() {
            return Err(not_configured());
        }
        Ok(Self {
            client_id,
            client_secret,
            redirect_uri,
        })
    }
}

pub(crate) fn configured(state: &HandlerState) -> bool {
    state.public_config.messaging.mode == "managed" && ManagedSlackConfig::from_state(state).is_ok()
}

fn not_configured() -> Response {
    error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "managed Slack OAuth is not configured",
    )
}

fn slack_secret_box() -> anyhow::Result<patchbay_util::secretbox::SecretBox> {
    let key = patchbay_util::secretbox::load_key("PATCHBAY_SLACK_SECRET_KEY")?;
    patchbay_util::secretbox::SecretBox::new(&key).map_err(anyhow::Error::from)
}

fn seal_base64(
    box_: &patchbay_util::secretbox::SecretBox,
    plaintext: &[u8],
) -> anyhow::Result<String> {
    Ok(base64::engine::general_purpose::STANDARD.encode(box_.seal(plaintext)?))
}

fn hash_state(raw: &str) -> Vec<u8> {
    Sha256::digest(raw.as_bytes()).to_vec()
}

fn can_manage(role: &str) -> bool {
    matches!(role, "owner" | "admin")
}

fn validate_app_redirect(raw: &str, public_app_origin: &str) -> Option<String> {
    let public = Url::parse(public_app_origin.trim()).ok()?;
    let redirect = if raw.trim().starts_with('/') {
        public.join(raw.trim()).ok()?
    } else {
        Url::parse(raw.trim()).ok()?
    };
    if redirect.scheme() != "https"
        || redirect.scheme() != public.scheme()
        || redirect.host_str() != public.host_str()
        || redirect.port_or_known_default() != public.port_or_known_default()
        || !redirect.username().is_empty()
        || redirect.password().is_some()
        || redirect.fragment().is_some()
    {
        return None;
    }
    Some(redirect.to_string())
}

fn redirect_result(base: &str, key: &str, value: &str) -> Response {
    let Ok(mut url) = Url::parse(base) else {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "invalid Slack authorization return URL",
        );
    };
    url.query_pairs_mut().append_pair(key, value);
    (
        StatusCode::SEE_OTHER,
        [(header::LOCATION, url.as_str().to_string())],
    )
        .into_response()
}

fn redirect_slack_error(base: &str, code: &str) -> Response {
    redirect_result(base, "slack_error", code)
}

fn verify_slack_signature(
    signing_secret: &str,
    headers: &HeaderMap,
    body: &[u8],
    now: DateTime<Utc>,
) -> bool {
    let Some(timestamp) = headers
        .get("x-slack-request-timestamp")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
    else {
        return false;
    };
    if (now.timestamp() - timestamp).abs() > SLACK_SIGNATURE_MAX_AGE_SECS {
        return false;
    }
    let Some(signature) = headers
        .get("x-slack-signature")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("v0="))
        .and_then(|value| hex::decode(value).ok())
    else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(signing_secret.as_bytes()) else {
        return false;
    };
    mac.update(format!("v0:{timestamp}:").as_bytes());
    mac.update(body);
    mac.verify_slice(&signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_must_stay_on_public_app_origin() {
        let origin = "https://patchbay.aspectlylabs.com";
        assert_eq!(
            validate_app_redirect("/acme/settings?tab=integrations", origin).as_deref(),
            Some("https://patchbay.aspectlylabs.com/acme/settings?tab=integrations")
        );
        assert_eq!(
            validate_app_redirect(
                "https://patchbay.aspectlylabs.com/acme/settings?tab=integrations",
                origin,
            )
            .as_deref(),
            Some("https://patchbay.aspectlylabs.com/acme/settings?tab=integrations")
        );
        assert!(validate_app_redirect("http://localhost:3000/settings", origin).is_none());
        assert!(validate_app_redirect("https://evil.example/settings", origin).is_none());
    }

    #[test]
    fn slack_signature_is_body_bound_and_replay_bounded() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let body = br#"{"type":"event_callback"}"#;
        let timestamp = now.timestamp();
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(format!("v0:{timestamp}:").as_bytes());
        mac.update(body);
        let signature = format!("v0={}", hex::encode(mac.finalize().into_bytes()));
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-slack-request-timestamp",
            timestamp.to_string().parse().unwrap(),
        );
        headers.insert("x-slack-signature", signature.parse().unwrap());
        assert!(verify_slack_signature("secret", &headers, body, now));
        assert!(!verify_slack_signature("secret", &headers, b"{}", now));
        assert!(!verify_slack_signature(
            "secret",
            &headers,
            body,
            now + chrono::Duration::minutes(6),
        ));
    }

    #[test]
    fn managed_routing_key_is_tenant_specific() {
        assert_ne!(
            patchbay_slack::config::managed_routing_key("A1", "T1"),
            patchbay_slack::config::managed_routing_key("A1", "T2"),
        );
    }

    #[test]
    fn callback_requires_current_workspace_manager_role() {
        assert!(can_manage("owner"));
        assert!(can_manage("admin"));
        assert!(!can_manage("member"));
        assert!(!can_manage("guest"));
    }

    #[test]
    fn managed_oauth_requests_slash_command_scope() {
        assert!(SLACK_BOT_SCOPES.split(',').any(|scope| scope == "commands"));
    }
}
