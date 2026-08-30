//! GitHub App installation management and signed webhook ingress.

use std::time::Duration;

use axum::body::Body;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use patchbay_db::models::{GithubInstallation, GithubPullRequest};
use patchbay_db::queries::{github, member, work_product as work_product_q};
use patchbay_middleware::workspace::WorkspaceContext;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use uuid::Uuid;

use crate::{error::error_response, state::HandlerState};

type HmacSha256 = Hmac<Sha256>;
const MAX_WEBHOOK_BODY: usize = 10 << 20;
const GITHUB_STATE_TTL_SECS: i64 = 10 * 60;
const GITHUB_STATE_REDIS_TIMEOUT: Duration = Duration::from_secs(1);
const GITHUB_STATE_REPLAY_PREFIX: &str = "patchbay:{github_callback_state}:";

pub fn public_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/github/setup", get(setup))
        .route("/api/webhooks/github", post(webhook))
}

pub fn member_router() -> Router<HandlerState> {
    Router::new().route(
        "/api/workspaces/{id}/github/installations",
        get(list_installations),
    )
}

pub fn admin_router() -> Router<HandlerState> {
    Router::new()
        .route("/api/workspaces/{id}/github/connect", get(connect))
        .route(
            "/api/workspaces/{id}/github/installations/{installation_id}",
            delete(remove_installation),
        )
        .route(
            "/api/workspaces/{id}/github/installations/{installation_id}/repositories",
            get(list_repositories),
        )
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_default().trim().to_string()
}

fn configured() -> bool {
    !env("GITHUB_APP_SLUG").is_empty() && !env("GITHUB_WEBHOOK_SECRET").is_empty()
}

fn browse_configured(state: &HandlerState) -> bool {
    state.github_snapshots.client().is_some()
}

fn allowed_return(value: &str) -> bool {
    matches!(value, "github" | "repositories")
}

fn state_token(workspace_id: Uuid, connected_by: Uuid, return_to: &str) -> Option<String> {
    state_token_at(
        workspace_id,
        connected_by,
        return_to,
        Utc::now().timestamp(),
    )
}

fn state_token_at(
    workspace_id: Uuid,
    connected_by: Uuid,
    return_to: &str,
    now: i64,
) -> Option<String> {
    let secret = env("GITHUB_WEBHOOK_SECRET");
    if secret.is_empty() || !allowed_return(return_to) {
        return None;
    }
    let mut nonce = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let expires_at = now.checked_add(GITHUB_STATE_TTL_SECS)?;
    // The public callback trusts only the current, expiring state format. Old
    // formats omitted either the actor or expiry and must fail closed.
    let payload = format!(
        "v3.{workspace_id}.{connected_by}.{return_to}.{expires_at}.{}",
        hex::encode(nonce)
    );
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(payload.as_bytes());
    Some(format!(
        "{payload}.{}",
        hex::encode(mac.finalize().into_bytes())
    ))
}

struct VerifiedState<'a> {
    workspace_id: Uuid,
    return_to: &'a str,
    connected_by: Uuid,
    expires_at: i64,
    nonce: &'a str,
}

fn verify_state(token: &str) -> Option<VerifiedState<'_>> {
    verify_state_at(token, Utc::now().timestamp())
}

fn verify_state_at(token: &str, now: i64) -> Option<VerifiedState<'_>> {
    let secret = env("GITHUB_WEBHOOK_SECRET");
    let parts = token.split('.').collect::<Vec<_>>();
    if secret.is_empty() || parts.len() != 7 || parts[0] != "v3" {
        return None;
    }
    let return_to = parts[3];
    if !allowed_return(return_to) {
        return None;
    }
    let expires_at = parts[4].parse::<i64>().ok()?;
    if expires_at <= now {
        return None;
    }
    let nonce = parts[5];
    if hex::decode(nonce).ok()?.len() != 12 {
        return None;
    }
    let payload = parts[..=5].join(".");
    let signature = hex::decode(parts[6]).ok()?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&signature).ok()?;
    Some(VerifiedState {
        workspace_id: Uuid::parse_str(parts[1]).ok()?,
        return_to,
        connected_by: Uuid::parse_str(parts[2]).ok()?,
        expires_at,
        nonce,
    })
}

async fn consume_state_once(
    client: Option<&redis::Client>,
    verified: &VerifiedState<'_>,
    now: i64,
) -> bool {
    let Some(client) = client else {
        return false;
    };
    let Some(ttl) = verified.expires_at.checked_sub(now).filter(|ttl| *ttl > 0) else {
        return false;
    };
    let key = format!("{GITHUB_STATE_REPLAY_PREFIX}{}", verified.nonce);
    let operation = async {
        let mut connection = client.get_multiplexed_async_connection().await?;
        redis::cmd("SET")
            .arg(key)
            .arg("1")
            .arg("NX")
            .arg("EX")
            .arg(ttl)
            .query_async::<Option<String>>(&mut connection)
            .await
    };
    matches!(
        tokio::time::timeout(GITHUB_STATE_REDIS_TIMEOUT, operation).await,
        Ok(Ok(Some(_)))
    )
}

fn can_manage_github(role: &str) -> bool {
    matches!(role, "owner" | "admin")
}

fn settings_url(return_to: &str) -> String {
    let base = {
        let value = env("FRONTEND_ORIGIN");
        if value.is_empty() {
            "http://localhost:3000".to_string()
        } else {
            value.trim_end_matches('/').to_string()
        }
    };
    format!("{base}/settings?tab={return_to}")
}

#[derive(Deserialize)]
struct ConnectQuery {
    return_to: Option<String>,
}

async fn connect(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(query): Query<ConnectQuery>,
) -> Response {
    let workspace_id = match Uuid::parse_str(&context.workspace_id) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace id"),
    };
    if !configured() {
        return Json(json!({"url": "", "configured": false})).into_response();
    }
    if state.rate_limit_client.is_none() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "github callback state store unavailable",
        );
    }
    let return_to = query.return_to.as_deref().unwrap_or("github").trim();
    if !allowed_return(return_to) {
        return error_response(StatusCode::BAD_REQUEST, "invalid return target");
    }
    let Some(state) = state_token(workspace_id, context.member.user_id, return_to) else {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to sign state");
    };
    let app_slug = env("GITHUB_APP_SLUG");
    let slug = utf8_percent_encode(&app_slug, NON_ALPHANUMERIC);
    let state = utf8_percent_encode(&state, NON_ALPHANUMERIC);
    Json(json!({
        "url": format!("https://github.com/apps/{slug}/installations/new?state={state}"),
        "configured": true
    }))
    .into_response()
}

#[derive(Deserialize)]
struct SetupQuery {
    installation_id: Option<String>,
    state: Option<String>,
}

async fn setup(State(state): State<HandlerState>, Query(query): Query<SetupQuery>) -> Response {
    let default_url = settings_url("github");
    let Some(raw_state) = query.state.as_deref().filter(|value| !value.is_empty()) else {
        return Redirect::temporary(&format!("{default_url}&github_error=missing_params"))
            .into_response();
    };
    let Some(verified) = verify_state(raw_state) else {
        return Redirect::temporary(&format!("{default_url}&github_error=invalid_state"))
            .into_response();
    };
    let target = settings_url(verified.return_to);
    let Some(installation_id) = query
        .installation_id
        .as_deref()
        .and_then(|value| value.parse::<i64>().ok())
    else {
        let error = if query.installation_id.is_none() {
            "missing_params"
        } else {
            "bad_installation_id"
        };
        return Redirect::temporary(&format!("{target}&github_error={error}")).into_response();
    };
    if !consume_state_once(
        state.rate_limit_client.as_ref(),
        &verified,
        Utc::now().timestamp(),
    )
    .await
    {
        return Redirect::temporary(&format!("{target}&github_error=invalid_state"))
            .into_response();
    }
    let account = match state.github_snapshots.client() {
        Some(client) => client.installation_account(installation_id).await.ok(),
        None => None,
    };
    let (login, account_type, avatar) = account
        .map(|value| (value.login, value.account_type, Some(value.avatar_url)))
        .unwrap_or_else(|| ("unknown".into(), "User".into(), None));
    let workspace_id = verified.workspace_id;
    let connected_by = verified.connected_by;
    let still_authorized =
        member::get_member_by_user_and_workspace(&state.pool, connected_by, workspace_id)
            .await
            .ok()
            .flatten()
            .is_some_and(|membership| can_manage_github(&membership.role));
    if !still_authorized {
        return Redirect::temporary(&format!("{target}&github_error=authorization_changed"))
            .into_response();
    }
    let mut installation = match github::create_git_hub_installation(
        &state.pool,
        workspace_id,
        installation_id,
        &login,
        &account_type,
        avatar.as_deref(),
        Some(connected_by),
    )
    .await
    {
        Ok(Some(value)) => value,
        _ => {
            return Redirect::temporary(&format!("{target}&github_error=persist_failed"))
                .into_response();
        }
    };
    if let Ok(Some(pending)) =
        github::get_pending_git_hub_installation(&state.pool, installation_id).await
    {
        if let Ok(Some(refreshed)) = github::create_git_hub_installation(
            &state.pool,
            workspace_id,
            installation_id,
            &pending.account_login,
            &pending.account_type,
            pending.account_avatar_url.as_deref(),
            Some(connected_by),
        )
        .await
        {
            installation = refreshed;
            let _ = github::delete_pending_git_hub_installation(&state.pool, installation_id).await;
        }
    }
    publish_installation(&state, &installation);
    Redirect::temporary(&format!("{target}&github_connected=1")).into_response()
}

#[derive(Serialize)]
struct InstallationResponse {
    id: Uuid,
    workspace_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    installation_id: Option<i64>,
    account_login: String,
    account_type: String,
    account_avatar_url: Option<String>,
    created_at: String,
}

fn installation_response(row: GithubInstallation, can_manage: bool) -> InstallationResponse {
    InstallationResponse {
        id: row.id,
        workspace_id: row.workspace_id,
        installation_id: can_manage.then_some(row.installation_id),
        account_login: row.account_login,
        account_type: row.account_type,
        account_avatar_url: row.account_avatar_url,
        created_at: crate::timefmt::rfc3339(row.created_at),
    }
}

async fn list_installations(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    let workspace_id = match Uuid::parse_str(&context.workspace_id) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace id"),
    };
    let can_manage = can_manage_github(&context.member.role);
    match github::list_git_hub_installations_by_workspace(&state.pool, workspace_id).await {
        Ok(rows) => Json(json!({
            "installations": rows.into_iter().map(|row| installation_response(row, can_manage)).collect::<Vec<_>>(),
            "configured": configured(),
            "repository_browse_configured": browse_configured(&state),
            "can_manage": can_manage,
        }))
        .into_response(),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list installations"),
    }
}

async fn remove_installation(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, raw_id)): Path<(String, String)>,
) -> Response {
    let workspace_id = match Uuid::parse_str(&context.workspace_id) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace id"),
    };
    let id = match Uuid::parse_str(&raw_id) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid installation id"),
    };
    match github::delete_git_hub_installation(&state.pool, id, workspace_id).await {
        Ok(_) => {
            state.bus.publish(&patchbay_events::Event {
                event_type: patchbay_protocol::EVENT_GITHUB_INSTALLATION_DELETED.into(),
                workspace_id: workspace_id.to_string(),
                actor_type: "system".into(),
                payload: json!({"id": raw_id}),
                ..Default::default()
            });
            StatusCode::NO_CONTENT.into_response()
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to remove installation",
        ),
    }
}

#[derive(Deserialize)]
struct PageQuery {
    page: Option<i32>,
    per_page: Option<i32>,
}

async fn list_repositories(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((_workspace, raw_id)): Path<(String, String)>,
    Query(query): Query<PageQuery>,
) -> Response {
    let workspace_id = match Uuid::parse_str(&context.workspace_id) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace id"),
    };
    let id = match Uuid::parse_str(&raw_id) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid installation id"),
    };
    let page = query.page.unwrap_or(1);
    let per_page = query.per_page.unwrap_or(100);
    if !(1..=100_000).contains(&page) {
        return error_response(StatusCode::BAD_REQUEST, "invalid page");
    }
    if !(1..=100).contains(&per_page) {
        return error_response(StatusCode::BAD_REQUEST, "invalid per_page");
    }
    let row = match github::get_git_hub_installation_by_id(&state.pool, id).await {
        Ok(Some(row)) if row.workspace_id == workspace_id => row,
        _ => return error_response(StatusCode::NOT_FOUND, "github installation not found"),
    };
    let client = match state.github_snapshots.client() {
        Some(value) => value,
        None => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "github repository browsing is not configured",
            );
        }
    };
    match client
        .installation_repositories_once(row.installation_id, page, per_page)
        .await
    {
        Ok(value) => Json(value).into_response(),
        Err(error) => {
            tracing::warn!(%error, "GitHub repository listing failed");
            error_response(
                StatusCode::BAD_GATEWAY,
                "failed to list github repositories",
            )
        }
    }
}

fn publish_installation(state: &HandlerState, row: &GithubInstallation) {
    state.bus.publish(&patchbay_events::Event {
        event_type: patchbay_protocol::EVENT_GITHUB_INSTALLATION_CREATED.into(),
        workspace_id: row.workspace_id.to_string(),
        actor_type: "system".into(),
        payload: json!({"installation": installation_response(row.clone(), false)}),
        ..Default::default()
    });
}

async fn raw_body(body: Body) -> Result<Vec<u8>, Response> {
    let mut stream = body.into_data_stream();
    let mut output = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|_| error_response(StatusCode::BAD_REQUEST, "read body failed"))?;
        if output.len().saturating_add(chunk.len()) > MAX_WEBHOOK_BODY {
            return Err(error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body is too large",
            ));
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

fn webhook_signature_valid(header: Option<&str>, body: &[u8]) -> bool {
    let secret = env("GITHUB_WEBHOOK_SECRET");
    let Some(signature) = header.and_then(|value| value.strip_prefix("sha256=")) else {
        return false;
    };
    let Ok(signature) = hex::decode(signature) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    !secret.is_empty() && mac.verify_slice(&signature).is_ok()
}

async fn webhook(State(state): State<HandlerState>, headers: HeaderMap, body: Body) -> Response {
    if env("GITHUB_WEBHOOK_SECRET").is_empty() {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "github webhooks not configured",
        );
    }
    let body = match raw_body(body).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !webhook_signature_valid(
        headers
            .get("x-hub-signature-256")
            .and_then(|value| value.to_str().ok()),
        &body,
    ) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid signature");
    }
    match headers
        .get("x-github-event")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
    {
        "ping" => return Json(json!({"ok": "pong"})).into_response(),
        "installation" => match handle_installation_event(&state, &body).await {
            Ok(()) => {}
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to persist github webhook",
                )
            }
        },
        "pull_request" => match handle_pull_request_event(&state, &body).await {
            Ok(()) => {}
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to persist github webhook",
                )
            }
        },
        "check_suite" | "check_run" | "status" => match handle_ci_event(&state, &body).await {
            Ok(()) => {}
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to persist github webhook",
                )
            }
        },
        _ => {}
    }
    StatusCode::ACCEPTED.into_response()
}

#[derive(Deserialize)]
struct InstallationEvent {
    action: String,
    installation: InstallationEventBody,
}
#[derive(Deserialize)]
struct InstallationEventBody {
    id: i64,
    account: InstallationAccountBody,
}
#[derive(Deserialize)]
struct InstallationAccountBody {
    #[serde(default)]
    login: String,
    #[serde(default, rename = "type")]
    account_type: String,
    avatar_url: Option<String>,
}

async fn handle_installation_event(state: &HandlerState, body: &[u8]) -> anyhow::Result<()> {
    let Ok(event) = serde_json::from_slice::<InstallationEvent>(body) else {
        return Ok(());
    };
    match event.action.as_str() {
        "deleted" | "suspend" => {
            let rows = github::delete_git_hub_installation_by_installation_id(
                &state.pool,
                event.installation.id,
            )
            .await?;
            github::delete_pending_git_hub_installation(&state.pool, event.installation.id).await?;
            for row in rows {
                if let (Some(id), Some(workspace_id)) = (row.id, row.workspace_id) {
                    state.bus.publish(&patchbay_events::Event {
                        event_type: patchbay_protocol::EVENT_GITHUB_INSTALLATION_DELETED.into(),
                        workspace_id: workspace_id.to_string(),
                        actor_type: "system".into(),
                        payload: json!({"id": id}),
                        ..Default::default()
                    });
                }
            }
        }
        "created" | "new_permissions_accepted" | "unsuspend"
            if !event.installation.account.login.trim().is_empty() =>
        {
            let account_type = if event.installation.account.account_type.is_empty() {
                "User"
            } else {
                &event.installation.account.account_type
            };
            let rows = github::list_git_hub_installations_by_installation_id(
                &state.pool,
                event.installation.id,
            )
            .await?;
            if rows.is_empty() {
                github::upsert_pending_git_hub_installation(
                    &state.pool,
                    event.installation.id,
                    &event.installation.account.login,
                    account_type,
                    event.installation.account.avatar_url.as_deref(),
                )
                .await?;
            } else {
                let refreshed = github::update_git_hub_installation_account_by_installation_id(
                    &state.pool,
                    event.installation.id,
                    &event.installation.account.login,
                    account_type,
                    event.installation.account.avatar_url.as_deref(),
                )
                .await?;
                github::delete_pending_git_hub_installation(&state.pool, event.installation.id)
                    .await?;
                for row in &refreshed {
                    publish_installation(state, row);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Deserialize)]
struct PullRequestEvent {
    #[serde(default)]
    action: String,
    pull_request: PullRequestBody,
    repository: RepositoryBody,
    installation: EventInstallation,
    changes: Option<PullRequestChanges>,
}
#[derive(Deserialize)]
struct PullRequestChanges {
    base: Option<BaseChange>,
}
#[derive(Deserialize)]
struct BaseChange {
    #[serde(rename = "ref")]
    reference: Option<RefChange>,
}
#[derive(Deserialize)]
struct RefChange {
    #[serde(default)]
    from: String,
}
#[derive(Deserialize)]
struct EventInstallation {
    id: i64,
}
#[derive(Deserialize)]
struct RepositoryBody {
    name: String,
    owner: LoginBody,
}
#[derive(Deserialize)]
struct LoginBody {
    login: String,
}
#[derive(Deserialize)]
struct PullRequestBody {
    number: i32,
    html_url: String,
    title: String,
    state: String,
    draft: bool,
    merged: bool,
    merged_at: Option<String>,
    closed_at: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    mergeable_state: Option<String>,
    additions: i32,
    deletions: i32,
    changed_files: i32,
    head: HeadBody,
    #[serde(default)]
    user: Option<UserBody>,
}
#[derive(Deserialize)]
struct HeadBody {
    #[serde(rename = "ref")]
    branch: String,
    sha: String,
}
#[derive(Deserialize)]
struct UserBody {
    login: String,
    avatar_url: String,
}

fn timestamp(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.to_utc())
}

async fn handle_pull_request_event(state: &HandlerState, body: &[u8]) -> anyhow::Result<()> {
    let Ok(event) = serde_json::from_slice::<PullRequestEvent>(body) else {
        return Ok(());
    };
    if event.installation.id == 0 {
        return Ok(());
    }
    let installations =
        github::list_git_hub_installations_by_installation_id(&state.pool, event.installation.id)
            .await?;
    if installations.is_empty() {
        return Ok(());
    }
    let owner = event.repository.owner.login.to_lowercase();
    let repo = event.repository.name.to_lowercase();
    let pr_state = if event.pull_request.merged {
        "merged"
    } else if event.pull_request.state == "closed" {
        "closed"
    } else if event.pull_request.draft {
        "draft"
    } else {
        "open"
    };
    for installation in installations {
        let base_changed = event
            .changes
            .as_ref()
            .and_then(|changes| changes.base.as_ref())
            .and_then(|base| base.reference.as_ref())
            .is_some_and(|reference| !reference.from.is_empty());
        let clear_mergeable =
            matches!(event.action.as_str(), "opened" | "synchronize" | "reopened")
                || (event.action == "edited" && base_changed);
        let pr = github::upsert_git_hub_pull_request(
            &state.pool,
            installation.workspace_id,
            event.installation.id,
            &owner,
            &repo,
            event.pull_request.number,
            &event.pull_request.title,
            pr_state,
            &event.pull_request.html_url,
            Some(timestamp(event.pull_request.created_at.as_deref()).unwrap_or_else(Utc::now)),
            Some(timestamp(event.pull_request.updated_at.as_deref()).unwrap_or_else(Utc::now)),
            &event.pull_request.head.sha,
            event.pull_request.additions,
            event.pull_request.deletions,
            event.pull_request.changed_files,
            Some(&event.pull_request.head.branch),
            event
                .pull_request
                .user
                .as_ref()
                .map(|user| user.login.as_str())
                .filter(|value| !value.is_empty()),
            event
                .pull_request
                .user
                .as_ref()
                .map(|user| user.avatar_url.as_str())
                .filter(|value| !value.is_empty()),
            timestamp(event.pull_request.merged_at.as_deref()),
            timestamp(event.pull_request.closed_at.as_deref()),
            event.pull_request.mergeable_state.as_deref(),
            Some(clear_mergeable),
        )
        .await?;
        if let Some(pr) = pr {
            let product = match work_product_q::upsert_work_product(
                &state.pool,
                pr.workspace_id,
                "pull_request",
                "github",
                &work_product_q::external_identity_for_github(&owner, &repo, pr.pr_number),
                Some(&pr.html_url),
                Some("github_pull_request"),
                Some(pr.id),
            )
            .await
            {
                Ok(product) => product,
                Err(error) => {
                    tracing::warn!(%error, pr_id = %pr.id, "github: upsert work product failed");
                    continue;
                }
            };
            let issue_ids = match work_product_q::list_issue_ids_for_work_product(
                &state.pool,
                pr.workspace_id,
                product.id,
            )
            .await
            {
                Ok(issue_ids) => issue_ids,
                Err(error) => {
                    tracing::warn!(%error, pr_id = %pr.id, "github: list work product relations failed");
                    continue;
                }
            };
            if matches!(pr.state.as_str(), "merged" | "closed") {
                for issue_id in &issue_ids {
                    let Ok(Some(issue)) = patchbay_db::queries::issue::get_issue_in_workspace(
                        &state.pool,
                        *issue_id,
                        pr.workspace_id,
                    )
                    .await
                    else {
                        continue;
                    };
                    crate::vcs_webhook::maybe_complete_issue(state, issue).await;
                }
            }
            publish_pr(
                state,
                &pr,
                issue_ids
                    .into_iter()
                    .map(|issue_id| issue_id.to_string())
                    .collect(),
            );
        }
    }
    state.github_snapshots.enqueue(
        event.installation.id,
        owner,
        repo,
        event.pull_request.number,
    );
    Ok(())
}

fn publish_pr(state: &HandlerState, pr: &GithubPullRequest, linked_issue_ids: Vec<String>) {
    state.bus.publish(&patchbay_events::Event { event_type: patchbay_protocol::EVENT_PULL_REQUEST_UPDATED.into(), workspace_id: pr.workspace_id.to_string(), actor_type: "system".into(), payload: json!({"pull_request": crate::issue_pull_request::github_model_response(pr.clone(), state.github_snapshots.enabled()), "linked_issue_ids": linked_issue_ids}), ..Default::default() });
}

#[derive(Deserialize)]
struct CiEvent {
    installation: EventInstallation,
    repository: RepositoryBody,
    #[serde(default)]
    sha: String,
    #[serde(default)]
    check_suite: CiSuite,
    #[serde(default)]
    check_run: CiRun,
}
#[derive(Default, Deserialize)]
struct CiSuite {
    #[serde(default)]
    head_sha: String,
    #[serde(default)]
    pull_requests: Vec<CiPr>,
}
#[derive(Default, Deserialize)]
struct CiRun {
    #[serde(default)]
    pull_requests: Vec<CiPr>,
    #[serde(default)]
    check_suite: CiSuite,
}
#[derive(Deserialize)]
struct CiPr {
    number: i32,
}

async fn handle_ci_event(state: &HandlerState, body: &[u8]) -> anyhow::Result<()> {
    let Ok(event) = serde_json::from_slice::<CiEvent>(body) else {
        return Ok(());
    };
    let owner = event.repository.owner.login.to_lowercase();
    let repo = event.repository.name.to_lowercase();
    if event.installation.id == 0 || repo.is_empty() {
        return Ok(());
    }
    let mut numbers = event
        .check_suite
        .pull_requests
        .iter()
        .chain(&event.check_run.pull_requests)
        .map(|value| value.number)
        .collect::<Vec<_>>();
    numbers.sort_unstable();
    numbers.dedup();
    if numbers.is_empty() {
        let sha = ci_sha(&event).to_string();
        if !sha.is_empty() {
            numbers = patchbay_db::queries::github_snapshot::list_git_hub_pr_numbers_by_head_sha(
                &state.pool,
                event.installation.id,
                &owner,
                &repo,
                &sha,
            )
            .await?;
        }
    }
    for number in numbers {
        state
            .github_snapshots
            .enqueue(event.installation.id, owner.clone(), repo.clone(), number);
    }
    Ok(())
}

fn ci_sha(event: &CiEvent) -> &str {
    if !event.sha.is_empty() {
        &event.sha
    } else if !event.check_suite.head_sha.is_empty() {
        &event.check_suite.head_sha
    } else {
        &event.check_run.check_suite.head_sha
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt;

    fn signed_state(payload: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(b"test-secret").unwrap();
        mac.update(payload.as_bytes());
        format!("{payload}.{}", hex::encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn state_and_webhook_signatures_are_tamper_evident() {
        std::env::set_var("GITHUB_WEBHOOK_SECRET", "test-secret");
        let workspace = Uuid::new_v4();
        let user = Uuid::new_v4();
        let now = 1_700_000_000;
        let token = state_token_at(workspace, user, "repositories", now).unwrap();
        let verified = verify_state_at(&token, now).unwrap();
        assert_eq!(verified.workspace_id, workspace);
        assert_eq!(verified.return_to, "repositories");
        assert_eq!(verified.connected_by, user);
        assert_eq!(verified.expires_at, now + GITHUB_STATE_TTL_SECS);
        assert!(verify_state_at(&format!("{token}x"), now).is_none());
        let mut mac = HmacSha256::new_from_slice(b"test-secret").unwrap();
        mac.update(b"{}");
        let header = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert!(webhook_signature_valid(Some(&header), b"{}"));
        assert!(!webhook_signature_valid(Some(&header), b"{ }"));
    }

    #[test]
    fn callback_identity_cannot_be_replaced_without_invalidating_state() {
        std::env::set_var("GITHUB_WEBHOOK_SECRET", "test-secret");
        let workspace = Uuid::new_v4();
        let user = Uuid::new_v4();
        let now = 1_700_000_000;
        let token = state_token_at(workspace, user, "github", now).unwrap();
        let forged = token.replacen(&user.to_string(), &Uuid::new_v4().to_string(), 1);
        assert!(verify_state_at(&forged, now).is_none());
    }

    #[test]
    fn callback_state_expires_and_legacy_shapes_fail_closed() {
        std::env::set_var("GITHUB_WEBHOOK_SECRET", "test-secret");
        let workspace = Uuid::new_v4();
        let user = Uuid::new_v4();
        let now = 1_700_000_000;
        let token = state_token_at(workspace, user, "github", now).unwrap();
        assert!(verify_state_at(&token, now + GITHUB_STATE_TTL_SECS - 1).is_some());
        assert!(verify_state_at(&token, now + GITHUB_STATE_TTL_SECS).is_none());

        for payload in [
            format!("{workspace}.nonce"),
            format!("{workspace}.github.nonce"),
            format!("v2.{workspace}.{user}.github.nonce"),
        ] {
            assert!(verify_state_at(&signed_state(&payload), now).is_none());
        }
    }

    #[tokio::test]
    async fn callback_state_consumption_fails_closed_without_shared_store() {
        std::env::set_var("GITHUB_WEBHOOK_SECRET", "test-secret");
        let now = 1_700_000_000;
        let token = state_token_at(Uuid::new_v4(), Uuid::new_v4(), "github", now).unwrap();
        let verified = verify_state_at(&token, now).unwrap();
        assert!(!consume_state_once(None, &verified, now).await);
    }

    #[test]
    fn github_callback_requires_current_admin_role() {
        assert!(can_manage_github("owner"));
        assert!(can_manage_github("admin"));
        assert!(!can_manage_github("member"));
        assert!(!can_manage_github("guest"));
    }

    #[test]
    fn status_webhook_preserves_top_level_sha_for_pr_lookup() {
        let event: CiEvent = serde_json::from_value(json!({
            "installation": {"id": 42},
            "repository": {"name": "patchbay", "owner": {"login": "patchbay-ai"}},
            "sha": "abc123"
        }))
        .unwrap();
        assert_eq!(ci_sha(&event), "abc123");
    }

    #[test]
    fn pull_request_payload_accepts_deleted_author() {
        let event: PullRequestEvent = serde_json::from_value(json!({
            "action": "closed",
            "installation": {"id": 42},
            "repository": {"name": "patchbay", "owner": {"login": "patchbay-ai"}},
            "pull_request": {
                "number": 1, "html_url": "https://example.test/pr/1", "title": "CORD-1",
                "state": "closed", "draft": false, "merged": false,
                "merged_at": null, "closed_at": null, "created_at": null, "updated_at": null,
                "mergeable_state": null, "additions": 0, "deletions": 0, "changed_files": 0,
                "head": {"ref": "cord-1", "sha": "abc"}, "user": null
            }
        }))
        .unwrap();
        assert!(event.pull_request.user.is_none());
    }

    #[tokio::test]
    async fn setup_callback_is_outside_user_authentication() {
        let response = crate::build_router(None, None)
            .oneshot(
                Request::get("/api/github/setup")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert!(response.status().is_redirection());
    }

    fn signed_webhook_body(body: &[u8]) -> String {
        std::env::set_var("GITHUB_WEBHOOK_SECRET", "test-secret");
        let mut mac = HmacSha256::new_from_slice(b"test-secret").unwrap();
        mac.update(body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    }

    #[tokio::test]
    async fn webhook_persistence_failure_is_not_acked() {
        let body = br#"{"action":"deleted","installation":{"id":1,"account":{"login":"x","type":"User"}}}"#;
        let signature = signed_webhook_body(body);
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let response = crate::build_router(Some(pool), None)
            .oneshot(
                Request::post("/api/webhooks/github")
                    .header("x-hub-signature-256", signature)
                    .header("x-github-event", "installation")
                    .body(Body::from(body.as_slice()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn invalid_webhook_json_is_still_acked() {
        let body = b"not-json";
        let signature = signed_webhook_body(body);
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap();
        let response = crate::build_router(Some(pool), None)
            .oneshot(
                Request::post("/api/webhooks/github")
                    .header("x-hub-signature-256", signature)
                    .header("x-github-event", "installation")
                    .body(Body::from(body.as_slice()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn repository_browse_follows_live_github_client() {
        let state = crate::HandlerState::new(
            sqlx::PgPool::connect_lazy("postgres://invalid/invalid").unwrap(),
            patchbay_auth::pat_cache::PatCache::disabled(),
            None,
        );
        assert!(!browse_configured(&state));
    }
}
