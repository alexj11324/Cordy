//! GitHub App installation management and signed webhook ingress.

use std::collections::{HashMap, HashSet};

use axum::body::Body;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use cordy_db::models::{GithubInstallation, GithubPullRequest};
use cordy_db::queries::github;
use cordy_middleware::workspace::WorkspaceContext;
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
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

fn browse_configured() -> bool {
    !env("GITHUB_APP_ID").is_empty() && !env("GITHUB_APP_PRIVATE_KEY").is_empty()
}

fn allowed_return(value: &str) -> bool {
    matches!(value, "github" | "repositories")
}

fn state_token(workspace_id: Uuid, connected_by: Uuid, return_to: &str) -> Option<String> {
    let secret = env("GITHUB_WEBHOOK_SECRET");
    if secret.is_empty() || !allowed_return(return_to) {
        return None;
    }
    let mut nonce = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce);
    // The callback is public, so the connecting identity must travel inside
    // the authenticated state rather than in a client-controlled header.
    // The version marker also lets us continue accepting legacy state tokens
    // (which intentionally yield no connected_by attribution).
    let payload = format!(
        "v2.{workspace_id}.{connected_by}.{return_to}.{}",
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
    connected_by: Option<Uuid>,
}

fn verify_state(token: &str) -> Option<VerifiedState<'_>> {
    let secret = env("GITHUB_WEBHOOK_SECRET");
    let parts = token.split('.').collect::<Vec<_>>();
    if secret.is_empty() || !matches!(parts.len(), 3 | 4 | 6) {
        return None;
    }
    let (workspace_index, return_to, nonce_index, connected_by) = if parts.len() == 6 {
        if parts[0] != "v2" {
            return None;
        }
        (1, parts[3], 4, Some(Uuid::parse_str(parts[2]).ok()?))
    } else if parts.len() == 4 {
        (0, parts[1], 2, None)
    } else {
        (0, "github", 1, None)
    };
    if !allowed_return(return_to) {
        return None;
    }
    let payload = parts[..=nonce_index].join(".");
    let signature = hex::decode(parts[nonce_index + 1]).ok()?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&signature).ok()?;
    Some(VerifiedState {
        workspace_id: Uuid::parse_str(parts[workspace_index]).ok()?,
        return_to,
        connected_by,
    })
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
    let account = match cordy_ghsnapshot::Client::new_from_env() {
        Ok(Some(client)) => client.installation_account(installation_id).await.ok(),
        _ => None,
    };
    let (login, account_type, avatar) = account
        .map(|value| (value.login, value.account_type, Some(value.avatar_url)))
        .unwrap_or_else(|| ("unknown".into(), "User".into(), None));
    let workspace_id = verified.workspace_id;
    let connected_by = verified.connected_by;
    let mut installation = match github::create_git_hub_installation(
        &state.pool,
        workspace_id,
        installation_id,
        &login,
        &account_type,
        avatar.as_deref(),
        connected_by,
    )
    .await
    {
        Ok(Some(value)) => value,
        _ => {
            return Redirect::temporary(&format!("{target}&github_error=persist_failed"))
                .into_response()
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
            connected_by,
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
    let can_manage = matches!(context.member.role.as_str(), "owner" | "admin");
    match github::list_git_hub_installations_by_workspace(&state.pool, workspace_id).await {
        Ok(rows) => Json(json!({
            "installations": rows.into_iter().map(|row| installation_response(row, can_manage)).collect::<Vec<_>>(),
            "configured": configured(),
            "repository_browse_configured": browse_configured(),
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
            state.bus.publish(&cordy_events::Event {
                event_type: cordy_protocol::EVENT_GITHUB_INSTALLATION_DELETED.into(),
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
    let client = match cordy_ghsnapshot::Client::new_from_env() {
        Ok(Some(value)) => value,
        _ => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "github repository browsing is not configured",
            )
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
    state.bus.publish(&cordy_events::Event {
        event_type: cordy_protocol::EVENT_GITHUB_INSTALLATION_CREATED.into(),
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
        "installation" => handle_installation_event(&state, &body).await,
        "pull_request" => handle_pull_request_event(&state, &body).await,
        "check_suite" | "check_run" | "status" => handle_ci_event(&state, &body).await,
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

async fn handle_installation_event(state: &HandlerState, body: &[u8]) {
    let Ok(event) = serde_json::from_slice::<InstallationEvent>(body) else {
        return;
    };
    match event.action.as_str() {
        "deleted" | "suspend" => {
            if let Ok(rows) = github::delete_git_hub_installation_by_installation_id(
                &state.pool,
                event.installation.id,
            )
            .await
            {
                let _ =
                    github::delete_pending_git_hub_installation(&state.pool, event.installation.id)
                        .await;
                for row in rows {
                    if let (Some(id), Some(workspace_id)) = (row.id, row.workspace_id) {
                        state.bus.publish(&cordy_events::Event {
                            event_type: cordy_protocol::EVENT_GITHUB_INSTALLATION_DELETED.into(),
                            workspace_id: workspace_id.to_string(),
                            actor_type: "system".into(),
                            payload: json!({"id": id}),
                            ..Default::default()
                        });
                    }
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
            .await
            .unwrap_or_default();
            if rows.is_empty() {
                let _ = github::upsert_pending_git_hub_installation(
                    &state.pool,
                    event.installation.id,
                    &event.installation.account.login,
                    account_type,
                    event.installation.account.avatar_url.as_deref(),
                )
                .await;
            } else if let Ok(refreshed) =
                github::update_git_hub_installation_account_by_installation_id(
                    &state.pool,
                    event.installation.id,
                    &event.installation.account.login,
                    account_type,
                    event.installation.account.avatar_url.as_deref(),
                )
                .await
            {
                let _ =
                    github::delete_pending_git_hub_installation(&state.pool, event.installation.id)
                        .await;
                for row in &refreshed {
                    publish_installation(state, row);
                }
            }
        }
        _ => {}
    }
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
    #[serde(default, deserialize_with = "null_string")]
    body: String,
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

fn null_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

async fn handle_pull_request_event(state: &HandlerState, body: &[u8]) {
    let Ok(event) = serde_json::from_slice::<PullRequestEvent>(body) else {
        return;
    };
    if event.installation.id == 0 {
        return;
    }
    let installations = match github::list_git_hub_installations_by_installation_id(
        &state.pool,
        event.installation.id,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, installation_id = event.installation.id, "github: lookup installation failed");
            return;
        }
    };
    if installations.is_empty() {
        return;
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
    let close_policy = close_intent_policy(state, &installations, &event).await;
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
        if let Ok(Some(pr)) = github::upsert_git_hub_pull_request(
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
        .await
        {
            mirror_issue_links(state, &event, &pr, &close_policy).await;
        }
    }
    state.github_snapshots.enqueue(
        event.installation.id,
        owner,
        repo,
        event.pull_request.number,
    );
}

#[derive(Default)]
struct CloseIntentPolicy {
    unrestricted: bool,
    owners: HashMap<String, Uuid>,
}

impl CloseIntentPolicy {
    fn permits(&self, identifier: &str, workspace_id: Uuid) -> bool {
        self.unrestricted || self.owners.get(identifier) == Some(&workspace_id)
    }
}

fn auto_link_enabled(settings: &serde_json::Value) -> Result<bool, ()> {
    let Some(object) = settings.as_object() else {
        return if settings.is_null() {
            Ok(true)
        } else {
            Err(())
        };
    };
    if matches!(
        object.get("github_enabled"),
        Some(serde_json::Value::Bool(false))
    ) {
        return Ok(false);
    }
    match object.get("github_auto_link_prs_enabled") {
        None => Ok(true),
        Some(serde_json::Value::Bool(value)) => Ok(*value),
        Some(_) => Err(()),
    }
}

async fn close_intent_policy(
    state: &HandlerState,
    installations: &[GithubInstallation],
    event: &PullRequestEvent,
) -> CloseIntentPolicy {
    if installations.len() < 2 {
        return CloseIntentPolicy {
            unrestricted: true,
            ..Default::default()
        };
    }
    let closing = crate::vcs_webhook::extract_closing_identifiers([
        event.pull_request.title.as_str(),
        event.pull_request.body.as_str(),
    ]);
    if closing.is_empty() {
        return CloseIntentPolicy::default();
    }
    let mut resolvers: HashMap<String, Vec<Uuid>> = HashMap::new();
    for installation in installations {
        let workspace = match cordy_db::queries::workspace::get_workspace(
            &state.pool,
            installation.workspace_id,
        )
        .await
        {
            Ok(Some(workspace)) => workspace,
            _ => return CloseIntentPolicy::default(),
        };
        match auto_link_enabled(&workspace.settings) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(()) => return CloseIntentPolicy::default(),
        }
        for identifier in &closing {
            if crate::vcs_webhook::lookup_issue(
                state,
                installation.workspace_id,
                &workspace.issue_prefix,
                identifier,
            )
            .await
            .is_some()
            {
                resolvers
                    .entry(identifier.clone())
                    .or_default()
                    .push(installation.workspace_id);
            }
        }
    }
    CloseIntentPolicy {
        unrestricted: false,
        owners: resolvers
            .into_iter()
            .filter_map(|(identifier, workspaces)| {
                (workspaces.len() == 1).then_some((identifier, workspaces[0]))
            })
            .collect(),
    }
}

async fn mirror_issue_links(
    state: &HandlerState,
    event: &PullRequestEvent,
    pull_request: &GithubPullRequest,
    close_policy: &CloseIntentPolicy,
) {
    let workspace =
        match cordy_db::queries::workspace::get_workspace(&state.pool, pull_request.workspace_id)
            .await
        {
            Ok(Some(workspace)) => workspace,
            _ => {
                publish_pr(state, pull_request, Vec::new());
                return;
            }
        };
    if !auto_link_enabled(&workspace.settings).unwrap_or(true) {
        publish_pr(state, pull_request, Vec::new());
        return;
    }
    let identifiers = crate::vcs_webhook::extract_identifiers([
        event.pull_request.title.as_str(),
        event.pull_request.body.as_str(),
        event.pull_request.head.branch.as_str(),
    ]);
    let closing: HashSet<_> = crate::vcs_webhook::extract_closing_identifiers([
        event.pull_request.title.as_str(),
        event.pull_request.body.as_str(),
    ])
    .into_iter()
    .collect();
    let qualifying: HashSet<_> = crate::vcs_webhook::extract_identifiers([
        event.pull_request.title.as_str(),
        event.pull_request.head.branch.as_str(),
    ])
    .into_iter()
    .chain(closing.iter().cloned())
    .collect();
    let preserve =
        event.action != "closed" && matches!(pull_request.state.as_str(), "merged" | "closed");
    let mut linked_ids = Vec::new();
    let mut reevaluate = Vec::new();
    for identifier in identifiers {
        let Some(issue) = crate::vcs_webhook::lookup_issue(
            state,
            pull_request.workspace_id,
            &workspace.issue_prefix,
            &identifier,
        )
        .await
        else {
            continue;
        };
        let close_intent = closing.contains(&identifier)
            && close_policy.permits(&identifier, pull_request.workspace_id)
            && !preserve;
        match github::link_issue_to_pull_request(
            &state.pool,
            issue.id,
            pull_request.id,
            close_intent,
            Some("system"),
            None,
            !qualifying.contains(&identifier),
            preserve,
            preserve,
            true,
        )
        .await
        {
            Ok(_) => {
                linked_ids.push(issue.id.to_string());
                reevaluate.push(issue);
            }
            Err(error) => tracing::warn!(%error, "github: link failed"),
        }
    }
    if matches!(pull_request.state.as_str(), "merged" | "closed") {
        for issue in reevaluate {
            crate::vcs_webhook::maybe_complete_issue(state, issue).await;
        }
    }
    publish_pr(state, pull_request, linked_ids);
}

fn publish_pr(state: &HandlerState, pr: &GithubPullRequest, linked_issue_ids: Vec<String>) {
    state.bus.publish(&cordy_events::Event { event_type: cordy_protocol::EVENT_PULL_REQUEST_UPDATED.into(), workspace_id: pr.workspace_id.to_string(), actor_type: "system".into(), payload: json!({"pull_request": crate::issue_pull_request::github_model_response(pr.clone(), state.github_snapshots.enabled()), "linked_issue_ids": linked_issue_ids}), ..Default::default() });
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

async fn handle_ci_event(state: &HandlerState, body: &[u8]) {
    let Ok(event) = serde_json::from_slice::<CiEvent>(body) else {
        return;
    };
    let owner = event.repository.owner.login.to_lowercase();
    let repo = event.repository.name.to_lowercase();
    if event.installation.id == 0 || repo.is_empty() {
        return;
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
            numbers = cordy_db::queries::github_snapshot::list_git_hub_pr_numbers_by_head_sha(
                &state.pool,
                event.installation.id,
                &owner,
                &repo,
                &sha,
            )
            .await
            .unwrap_or_default();
        }
    }
    for number in numbers {
        state
            .github_snapshots
            .enqueue(event.installation.id, owner.clone(), repo.clone(), number);
    }
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
    #[test]
    fn state_and_webhook_signatures_are_tamper_evident() {
        std::env::set_var("GITHUB_WEBHOOK_SECRET", "test-secret");
        let workspace = Uuid::new_v4();
        let user = Uuid::new_v4();
        let token = state_token(workspace, user, "repositories").unwrap();
        let verified = verify_state(&token).unwrap();
        assert_eq!(verified.workspace_id, workspace);
        assert_eq!(verified.return_to, "repositories");
        assert_eq!(verified.connected_by, Some(user));
        assert!(verify_state(&format!("{token}x")).is_none());
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
        let token = state_token(workspace, user, "github").unwrap();
        let forged = token.replacen(&user.to_string(), &Uuid::new_v4().to_string(), 1);
        assert!(verify_state(&forged).is_none());
    }

    #[test]
    fn status_webhook_preserves_top_level_sha_for_pr_lookup() {
        let event: CiEvent = serde_json::from_value(json!({
            "installation": {"id": 42},
            "repository": {"name": "cordy", "owner": {"login": "cordy-ai"}},
            "sha": "abc123"
        }))
        .unwrap();
        assert_eq!(ci_sha(&event), "abc123");
    }

    #[test]
    fn pull_request_payload_accepts_null_body_and_deleted_author() {
        let event: PullRequestEvent = serde_json::from_value(json!({
            "action": "closed",
            "installation": {"id": 42},
            "repository": {"name": "cordy", "owner": {"login": "cordy-ai"}},
            "pull_request": {
                "number": 1, "html_url": "https://example.test/pr/1", "title": "CORD-1",
                "body": null, "state": "closed", "draft": false, "merged": false,
                "merged_at": null, "closed_at": null, "created_at": null, "updated_at": null,
                "mergeable_state": null, "additions": 0, "deletions": 0, "changed_files": 0,
                "head": {"ref": "cord-1", "sha": "abc"}, "user": null
            }
        }))
        .unwrap();
        assert!(event.pull_request.body.is_empty());
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
}
