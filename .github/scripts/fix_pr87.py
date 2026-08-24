#!/usr/bin/env python3
from pathlib import Path
import re


def load(path: str) -> tuple[Path, str]:
    target = Path(path)
    return target, target.read_text()


def save(target: Path, text: str) -> None:
    target.write_text(text)


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return text.replace(old, new, 1)


def replace_between(text: str, start: str, end: str, replacement: str, label: str) -> str:
    start_index = text.find(start)
    if start_index < 0:
        raise SystemExit(f"{label}: start marker not found")
    end_index = text.find(end, start_index)
    if end_index < 0:
        raise SystemExit(f"{label}: end marker not found")
    return text[:start_index] + replacement + text[end_index:]


def rewrite_calls_with_runtime(scope: str) -> str:
    needle = "finish_lark_session("
    output = []
    cursor = 0
    replacements = 0
    while True:
        start = scope.find(needle, cursor)
        if start < 0:
            output.append(scope[cursor:])
            break
        output.append(scope[cursor:start])
        open_paren = start + len("finish_lark_session")
        depth = 0
        quote = None
        escaped = False
        close = None
        for index in range(open_paren, len(scope)):
            char = scope[index]
            if quote is not None:
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == quote:
                    quote = None
                continue
            if char in {'"', "'"}:
                quote = char
                continue
            if char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
                if depth == 0:
                    close = index
                    break
        if close is None:
            raise SystemExit("unterminated finish_lark_session call")
        args = scope[open_paren + 1:close]
        prefix = "runtime.session_redis.as_ref(), "
        if args.startswith("\n"):
            match = re.match(r"\n(\s*)", args)
            indent = match.group(1) if match else "            "
            args = f"\n{indent}runtime.session_redis.as_ref(),{args}"
        else:
            args = prefix + args
        output.append(f"finish_lark_session({args}).await")
        cursor = close + 1
        replacements += 1
    if replacements < 5:
        raise SystemExit(f"expected several finish_lark_session calls, rewrote {replacements}")
    return "".join(output)


# ---------------------------------------------------------------------------
# Bound HTTP drain before owned-runtime shutdown.
# ---------------------------------------------------------------------------
target, text = load("server-rs/crates/cordy-server/src/main.rs")
old = '''    let serve_result = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await;
    // Match Go's shutdown ordering: drain every in-flight HTTP handler before
    // stopping maintenance workers. Channel adapters are producers and must
    // drain while realtime fanout is still accepting their final events.
'''
new = '''    const HTTP_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);
    let graceful_shutdown = CancellationToken::new();
    let graceful_shutdown_signal = graceful_shutdown.clone();
    let server = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        graceful_shutdown_signal.cancelled().await;
    });
    tokio::pin!(server);
    let serve_result = tokio::select! {
        result = &mut server => result,
        () = shutdown_signal() => {
            graceful_shutdown.cancel();
            match tokio::time::timeout(HTTP_DRAIN_TIMEOUT, &mut server).await {
                Ok(result) => result,
                Err(_) => {
                    tracing::warn!(
                        timeout_seconds = HTTP_DRAIN_TIMEOUT.as_secs(),
                        "HTTP handlers exceeded the shutdown drain deadline; continuing owned-runtime shutdown"
                    );
                    Ok(())
                }
            }
        }
    };
    // Match Go's shutdown ordering: give in-flight HTTP handlers a bounded
    // drain window before stopping maintenance workers. Channel adapters are
    // producers and must drain while realtime fanout still accepts final events.
'''
text = replace_once(text, old, new, "HTTP drain block")
save(target, text)


# ---------------------------------------------------------------------------
# Propagate GitHub webhook persistence failures to a retryable HTTP response.
# ---------------------------------------------------------------------------
target, text = load("server-rs/crates/cordy-handler/src/github.rs")
start = "async fn webhook(State(state): State<HandlerState>, headers: HeaderMap, body: Body) -> Response {"
end = "#[derive(Deserialize)]\nstruct PullRequestEvent {"
replacement = r'''async fn webhook(State(state): State<HandlerState>, headers: HeaderMap, body: Body) -> Response {
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
    let event_kind = headers
        .get("x-github-event")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if event_kind == "ping" {
        return Json(json!({"ok": "pong"})).into_response();
    }
    let result = match event_kind {
        "installation" => handle_installation_event(&state, &body).await,
        "pull_request" => handle_pull_request_event(&state, &body).await,
        "check_suite" | "check_run" | "status" => handle_ci_event(&state, &body).await,
        _ => Ok(()),
    };
    if let Err(error) = result {
        tracing::error!(%error, event_kind, "github webhook processing failed");
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "github webhook processing failed",
        );
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
            github::delete_pending_git_hub_installation(&state.pool, event.installation.id)
                .await?;
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

'''
text = replace_between(text, start, end, replacement, "GitHub webhook/installation block")

start = "async fn handle_pull_request_event(state: &HandlerState, body: &[u8]) {"
end = "#[derive(Default)]\nstruct CloseIntentPolicy {"
replacement = r'''async fn handle_pull_request_event(
    state: &HandlerState,
    body: &[u8],
) -> anyhow::Result<()> {
    let Ok(event) = serde_json::from_slice::<PullRequestEvent>(body) else {
        return Ok(());
    };
    if event.installation.id == 0 {
        return Ok(());
    }
    let installations = github::list_git_hub_installations_by_installation_id(
        &state.pool,
        event.installation.id,
    )
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
        .await?
        .ok_or_else(|| anyhow::anyhow!("github pull request upsert returned no row"))?;
        mirror_issue_links(state, &event, &pr, &close_policy).await;
    }
    state.github_snapshots.enqueue(
        event.installation.id,
        owner,
        repo,
        event.pull_request.number,
    );
    Ok(())
}

'''
text = replace_between(text, start, end, replacement, "GitHub pull request handler")

start = "async fn handle_ci_event(state: &HandlerState, body: &[u8]) {"
end = "fn ci_sha(event: &CiEvent) -> &str {"
replacement = r'''async fn handle_ci_event(state: &HandlerState, body: &[u8]) -> anyhow::Result<()> {
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
            numbers = cordy_db::queries::github_snapshot::list_git_hub_pr_numbers_by_head_sha(
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

'''
text = replace_between(text, start, end, replacement, "GitHub CI handler")
save(target, text)


# ---------------------------------------------------------------------------
# Persist Lark install sessions in shared Redis when Redis is configured.
# ---------------------------------------------------------------------------
target, text = load("server-rs/crates/cordy-handler/src/connectors.rs")
text = replace_once(
    text,
    "use std::time::{Duration, Instant};",
    "use std::time::Duration;",
    "connectors time import",
)
text = replace_once(
    text,
    "use base64::Engine as _;\n",
    "use base64::Engine as _;\nuse chrono::Utc;\n",
    "connectors chrono import",
)
text = replace_once(
    text,
    "use serde::Deserialize;",
    "use serde::{Deserialize, Serialize};",
    "connectors serde import",
)
start = "#[derive(Clone)]\nstruct LarkSession {"
end = "async fn begin_lark_install("
replacement = r'''const LARK_INSTALL_SESSION_TTL_SECS: i64 = 15 * 60;
const LARK_INSTALL_SESSION_REDIS_TIMEOUT: Duration = Duration::from_secs(1);

fn lark_session_key(session_id: &str) -> String {
    format!("mul:channel:lark:install-session:{session_id}")
}

#[derive(Clone, Serialize, Deserialize)]
struct LarkSession {
    workspace_id: Uuid,
    initiator_id: Uuid,
    status: String,
    installation_id: Option<Uuid>,
    error_reason: Option<String>,
    error_message: Option<String>,
    expires_at: i64,
}

struct LarkRegistrationRuntime {
    pool: sqlx::PgPool,
    bus: Arc<cordy_events::Bus>,
    http_base_url: String,
    cancel: CancellationToken,
    session_redis: Option<redis::Client>,
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
    Ok(current.is_some_and(|(role, owner_id)| {
        can_manage_lark_agent(&role, owner_id, actor)
    }))
}

fn lark_sessions() -> &'static Mutex<HashMap<String, LarkSession>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, LarkSession>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn save_lark_session(
    client: Option<&redis::Client>,
    session_id: &str,
    session: &LarkSession,
) -> anyhow::Result<()> {
    if let Some(client) = client {
        let payload = serde_json::to_string(session)?;
        let ttl = (session.expires_at - Utc::now().timestamp())
            .clamp(1, LARK_INSTALL_SESSION_TTL_SECS);
        let mut connection = cordy_redis::RecoveringConnection::new(client.clone());
        let mut command = redis::cmd("SET");
        command
            .arg(lark_session_key(session_id))
            .arg(payload)
            .arg("EX")
            .arg(ttl);
        let result: redis::RedisResult<()> = tokio::time::timeout(
            LARK_INSTALL_SESSION_REDIS_TIMEOUT,
            command.query_async(&mut connection),
        )
        .await
        .map_err(|_| anyhow::anyhow!("lark install session Redis SET timed out"))?;
        result?;
        return Ok(());
    }

    let now = Utc::now().timestamp();
    let mut sessions = lark_sessions()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    sessions.retain(|_, value| value.expires_at > now);
    if sessions.len() >= 1024 && !sessions.contains_key(session_id) {
        anyhow::bail!("too many install sessions");
    }
    sessions.insert(session_id.to_string(), session.clone());
    Ok(())
}

async fn load_lark_session(
    client: Option<&redis::Client>,
    session_id: &str,
) -> anyhow::Result<Option<LarkSession>> {
    if let Some(client) = client {
        let mut connection = cordy_redis::RecoveringConnection::new(client.clone());
        let mut command = redis::cmd("GET");
        command.arg(lark_session_key(session_id));
        let result: redis::RedisResult<Option<Vec<u8>>> = tokio::time::timeout(
            LARK_INSTALL_SESSION_REDIS_TIMEOUT,
            command.query_async(&mut connection),
        )
        .await
        .map_err(|_| anyhow::anyhow!("lark install session Redis GET timed out"))?;
        let Some(raw) = result? else {
            return Ok(None);
        };
        let session: LarkSession = serde_json::from_slice(&raw)?;
        if session.expires_at <= Utc::now().timestamp() {
            delete_lark_session(Some(client), session_id).await?;
            return Ok(None);
        }
        return Ok(Some(session));
    }

    let now = Utc::now().timestamp();
    let mut sessions = lark_sessions()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    sessions.retain(|_, value| value.expires_at > now);
    Ok(sessions.get(session_id).cloned())
}

async fn delete_lark_session(
    client: Option<&redis::Client>,
    session_id: &str,
) -> anyhow::Result<()> {
    if let Some(client) = client {
        let mut connection = cordy_redis::RecoveringConnection::new(client.clone());
        let mut command = redis::cmd("DEL");
        command.arg(lark_session_key(session_id));
        let result: redis::RedisResult<i64> = tokio::time::timeout(
            LARK_INSTALL_SESSION_REDIS_TIMEOUT,
            command.query_async(&mut connection),
        )
        .await
        .map_err(|_| anyhow::anyhow!("lark install session Redis DEL timed out"))?;
        result?;
        return Ok(());
    }
    lark_sessions()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(session_id);
    Ok(())
}

'''
text = replace_between(text, start, end, replacement, "Lark session storage definitions")

old = '''    let session_id = Uuid::new_v4().to_string();
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
'''
new = '''    let session_id = Uuid::new_v4().to_string();
    let expires_in = i64::try_from(begun.expires_in.as_secs())
        .unwrap_or(LARK_INSTALL_SESSION_TTL_SECS)
        .clamp(1, LARK_INSTALL_SESSION_TTL_SECS);
    let session = LarkSession {
        workspace_id,
        initiator_id: actor,
        status: "pending".into(),
        installation_id: None,
        error_reason: None,
        error_message: None,
        expires_at: Utc::now().timestamp() + expires_in,
    };
    if let Err(error) =
        save_lark_session(state.rate_limit_client.as_ref(), &session_id, &session).await
    {
        tracing::error!(%error, "failed to persist Lark install session");
        let status = if error.to_string().contains("too many install sessions") {
            StatusCode::TOO_MANY_REQUESTS
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
        return error_response(status, "failed to persist install session");
    }
'''
text = replace_once(text, old, new, "Lark begin local session block")
text = replace_once(
    text,
    '''        cancel: state.channel_cancel.clone(),
    };''',
    '''        cancel: state.channel_cancel.clone(),
        session_redis: state.rate_limit_client.clone(),
    };''',
    "Lark runtime Redis dependency",
)
text = replace_once(
    text,
    '''        lark_sessions().lock().unwrap().remove(&session_id);
        return error_response(
''',
    '''        let _ = delete_lark_session(state.rate_limit_client.as_ref(), &session_id).await;
        return error_response(
''',
    "Lark spawn failure cleanup",
)

run_start = text.find("async fn run_lark_registration(")
run_end = text.find("fn lark_poll_protocol_error(", run_start)
if run_start < 0 or run_end < 0:
    raise SystemExit("Lark run registration scope markers not found")
scope = text[run_start:run_end]
scope = rewrite_calls_with_runtime(scope)
text = text[:run_start] + scope + text[run_end:]

start = "fn finish_lark_session("
end = "async fn lark_install_status("
replacement = r'''async fn finish_lark_session(
    session_redis: Option<&redis::Client>,
    session_id: &str,
    installation_id: Option<Uuid>,
    reason: Option<&str>,
    message: Option<&str>,
) {
    let mut session = match load_lark_session(session_redis, session_id).await {
        Ok(Some(session)) => session,
        Ok(None) => return,
        Err(error) => {
            tracing::error!(%error, session_id, "failed to load Lark install session");
            return;
        }
    };
    session.status = if installation_id.is_some() {
        "success".into()
    } else {
        "error".into()
    };
    session.installation_id = installation_id;
    session.error_reason = reason.map(str::to_string);
    session.error_message = message.map(str::to_string);
    if let Err(error) = save_lark_session(session_redis, session_id, &session).await {
        tracing::error!(%error, session_id, "failed to update Lark install session");
    }
}

'''
text = replace_between(text, start, end, replacement, "Lark finish session")

start = "async fn lark_install_status("
end = "async fn list_dingtalk_group_routes("
replacement = r'''async fn lark_install_status(
    State(state): State<HandlerState>,
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
    let session = match load_lark_session(state.rate_limit_client.as_ref(), session_id.trim()).await {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "failed to read Lark install session");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "install session storage unavailable",
            );
        }
    };
    let Some(session) = session.filter(|value| value.workspace_id == workspace_id) else {
        return error_response(StatusCode::NOT_FOUND, "install session not found");
    };
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

'''
text = replace_between(text, start, end, replacement, "Lark status handler")
save(target, text)


# ---------------------------------------------------------------------------
# Emit metrics/analytics side effects for legacy onboarding exactly once.
# ---------------------------------------------------------------------------
target, text = load("server-rs/crates/cordy-handler/src/onboarding_shim.rs")
text = replace_once(
    text,
    "use cordy_db::models::{Agent, Issue};",
    "use chrono::{DateTime, Utc};\nuse cordy_db::models::{Agent, Issue};",
    "onboarding chrono import",
)
start = "async fn complete_user("
end = "async fn publish("
replacement = r'''struct CompletionState {
    first_completion: bool,
    onboarded_at: DateTime<Utc>,
    joined_cloud_waitlist: bool,
}

async fn complete_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> anyhow::Result<CompletionState> {
    let before_onboarded_at = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        r#"SELECT onboarded_at FROM "user" WHERE id = $1 FOR UPDATE"#,
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("user not found"))?;
    let updated = user::mark_user_onboarded(&mut **tx, user_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("user not found"))?;
    if updated.starter_content_state.is_none() {
        user::set_starter_content_state(&mut **tx, user_id, Some("imported")).await?;
    }
    Ok(CompletionState {
        first_completion: before_onboarded_at.is_none(),
        onboarded_at: updated.onboarded_at.unwrap_or_else(Utc::now),
        joined_cloud_waitlist: updated.cloud_waitlist_email.is_some(),
    })
}

fn record_legacy_onboarding_event(
    state: &HandlerState,
    event: cordy_analytics::Event,
) {
    cordy_metrics::business_events::record_event(
        Some(state.analytics.as_ref()),
        state.business_metrics.as_deref(),
        &event,
    );
}

'''
text = replace_between(text, start, end, replacement, "legacy onboarding completion helper")

text = replace_once(
    text,
    '''    let mut made_agent = None;
    let helper = match agents
''',
    '''    let is_first_agent_in_workspace = agents.is_empty();
    let mut made_agent = None;
    let helper = match agents
''',
    "legacy onboarding first-agent marker",
)
old_completion = '''    if complete_user(&mut tx, user_id).await.is_err() || tx.commit().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to finish onboarding",
        );
    }
'''
new_completion = '''    let completion = match complete_user(&mut tx, user_id).await {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(%error, user_id = %user_id, "failed to complete legacy onboarding");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to finish onboarding",
            );
        }
    };
    if tx.commit().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to finish onboarding",
        );
    }
'''
count = text.count(old_completion)
if count != 2:
    raise SystemExit(f"legacy onboarding completion blocks: expected 2, found {count}")
text = text.replace(old_completion, new_completion, 2)

old = '''    publish(
        &state,
        workspace_id,
        user_id,
        made_agent.as_ref(),
        made_issue.as_ref(),
    )
    .await;
    if let Some(created) = made_issue.as_ref() {
'''
new = '''    publish(
        &state,
        workspace_id,
        user_id,
        made_agent.as_ref(),
        made_issue.as_ref(),
    )
    .await;
    if let Some(created) = made_agent.as_ref() {
        record_legacy_onboarding_event(
            &state,
            cordy_analytics::agent_created(
                &user_id.to_string(),
                &workspace_id.to_string(),
                &created.id.to_string(),
                &runtime.provider,
                &runtime.runtime_mode,
                "legacy_onboarding",
                is_first_agent_in_workspace,
            ),
        );
    }
    if let Some(created) = made_issue.as_ref() {
        record_legacy_onboarding_event(
            &state,
            cordy_analytics::issue_created(
                &user_id.to_string(),
                &workspace_id.to_string(),
                &created.id.to_string(),
                &helper.id.to_string(),
                "",
                "",
                cordy_analytics::SOURCE_ONBOARDING,
                cordy_analytics::PLATFORM_DESKTOP,
            ),
        );
    }
    if completion.first_completion {
        record_legacy_onboarding_event(
            &state,
            cordy_analytics::onboarding_completed(
                &user_id.to_string(),
                &workspace_id.to_string(),
                cordy_analytics::ONBOARDING_PATH_FULL,
                &crate::timefmt::rfc3339(completion.onboarded_at),
                completion.joined_cloud_waitlist,
            ),
        );
    }
    if let Some(created) = made_issue.as_ref() {
'''
text = replace_once(text, old, new, "with-runtime legacy analytics")

old = '''    publish(&state, workspace_id, user_id, None, made_issue.as_ref()).await;
    Json(json!({"workspace_id": workspace_id, "issue_id": onboarding_issue.id})).into_response()
'''
new = '''    publish(&state, workspace_id, user_id, None, made_issue.as_ref()).await;
    if let Some(created) = made_issue.as_ref() {
        record_legacy_onboarding_event(
            &state,
            cordy_analytics::issue_created(
                &user_id.to_string(),
                &workspace_id.to_string(),
                &created.id.to_string(),
                "",
                "",
                "",
                cordy_analytics::SOURCE_ONBOARDING,
                cordy_analytics::PLATFORM_DESKTOP,
            ),
        );
    }
    if completion.first_completion {
        record_legacy_onboarding_event(
            &state,
            cordy_analytics::onboarding_completed(
                &user_id.to_string(),
                &workspace_id.to_string(),
                cordy_analytics::ONBOARDING_PATH_RUNTIME_SKIPPED,
                &crate::timefmt::rfc3339(completion.onboarded_at),
                completion.joined_cloud_waitlist,
            ),
        );
    }
    Json(json!({"workspace_id": workspace_id, "issue_id": onboarding_issue.id})).into_response()
'''
text = replace_once(text, old, new, "without-runtime legacy analytics")
save(target, text)
