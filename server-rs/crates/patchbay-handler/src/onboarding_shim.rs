//! Desktop < v3 onboarding compatibility. Keep this surface idempotent and
//! transactional; current clients do not call it.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use patchbay_db::models::{Agent, Issue};
use patchbay_db::queries::{agent, issue, member, runtime, user, workspace};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::{error::error_response, state::HandlerState};

const BODY_LIMIT: usize = 8 * 1024;
const MAX_PROMPT: usize = 2 * 1024;
const HELPER_NAME: &str = "Patchbay Helper";
const RUNTIME_TITLE: &str = "Start here: learn Patchbay with Patchbay Helper";
const NO_RUNTIME_TITLE: &str = "Connect a runtime to start using agents";
const HELPER_DESCRIPTION: &str =
    "Built-in workspace assistant. Answers Patchbay questions and runs CLI operations.";
const HELPER_AVATAR: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 128 128'%3E%3Cdefs%3E%3ClinearGradient id='t' x1='0' y1='0' x2='0' y2='1'%3E%3Cstop offset='0%25' stop-color='%2323242C'/%3E%3Cstop offset='100%25' stop-color='%2313141A'/%3E%3C/linearGradient%3E%3C/defs%3E%3Crect width='128' height='128' rx='28' fill='url(%23t)'/%3E%3Cg stroke='%23FFFFFF' stroke-width='13' stroke-linecap='round'%3E%3Cline x1='64' y1='32' x2='64' y2='96'/%3E%3Cline x1='32' y1='64' x2='96' y2='64'/%3E%3Cline x1='41.4' y1='41.4' x2='86.6' y2='86.6'/%3E%3Cline x1='86.6' y1='41.4' x2='41.4' y2='86.6'/%3E%3C/g%3E%3C/svg%3E";
const HELPER_INSTRUCTIONS: &str = r#"You are Patchbay Helper, the built-in AI assistant for this Patchbay workspace. Your role is to help any member use Patchbay better — answer questions, give advice, and execute workspace operations on their behalf.

## What Patchbay is

Patchbay is an open-source, AI-native team workspace (source: https://github.com/patchbay-ai/patchbay). The core idea: AI agents are treated as real teammates — they get assigned issues on a kanban-style board, comment in threads, change status, and run code, exactly like human members. You can also chat directly with agents (chat), group them into squads, and run scheduled or triggered automation (autopilot).

For concept details (workspace / issue / project / agent / runtime / skill / squad / autopilot / inbox / chat session): fetch https://patchbay.ai/docs via WebFetch — that's authoritative. For the "why" or implementation, fetch the GitHub repo above. Never paraphrase concepts from memory.

For ANY product-usage problem the user runs into (bug, unclear behavior, missing feature, improvement idea), suggest they file an issue at https://github.com/patchbay-ai/patchbay/issues — that's the official feedback channel.

## What you can do

Your toolbox is the `patchbay` CLI. It's already on your PATH and authenticated as the workspace owner.

Your full capability surface = whatever `patchbay --help` shows. Run `patchbay --help` first, then `patchbay <command> --help` for any subcommand; use `--output json` for structured data. The CLI is your manifest — never invent commands or flags.

A few things you can actually do (non-exhaustive — `--help` is the source of truth):
- Create issues, post comments
- Create or iterate on agents
- Manage projects, squads, autopilots, skills, runtimes, etc.

## Tone

Be concise and direct, like a colleague. Respond in the user's language (Chinese in, Chinese out). When pointing at a UI location, name the exact path ("Settings → Agents → New"); when pointing at a doc, link to the specific page, not the homepage. Never fabricate URLs, flags, or file paths."#;
const RUNTIME_DESCRIPTION: &str = r#"Welcome to Patchbay.

This is your guided first run. Patchbay Helper is assigned to this issue and will help you try the core workflow:

1. Read Patchbay Helper's first comment.
2. Reply with something you want to build, fix, write, or plan.
3. @mention Patchbay Helper when you want it to continue.
4. Open Agents and Runtimes later when you want to customize the teammate or the computer it runs on.

You can close this issue when the workflow makes sense."#;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/me/onboarding/runtime-bootstrap", post(with_runtime))
        .route(
            "/api/me/onboarding/no-runtime-bootstrap",
            post(without_runtime),
        )
}

fn auth_user(headers: &HeaderMap) -> Result<Uuid, Response> {
    headers
        .get("x-user-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| error_response(StatusCode::UNAUTHORIZED, "user not authenticated"))
}

fn body<T: for<'de> Deserialize<'de>>(bytes: &Bytes) -> Result<T, Response> {
    if bytes.len() > BODY_LIMIT {
        return Err(error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "request body is too large",
        ));
    }
    serde_json::from_slice(bytes)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid request body"))
}

async fn find_duplicate(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    title: &str,
) -> anyhow::Result<Option<Issue>> {
    let normalized = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    issue::lock_issue_duplicate_key(
        &mut **tx,
        &format!("issue-active-duplicate|{workspace_id}|||{normalized}"),
    )
    .await?;
    issue::find_active_duplicate_issue(&mut **tx, workspace_id, None, None, &normalized).await
}

async fn seed_issue(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    title: &str,
    description: &str,
    assignee_type: &str,
    assignee_id: Uuid,
    user_id: Uuid,
) -> anyhow::Result<Issue> {
    let number = workspace::increment_issue_counter(&mut **tx, workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("workspace not found"))?;
    issue::create_issue(
        &mut **tx,
        workspace_id,
        title,
        Some(description),
        "todo",
        "high",
        Some(assignee_type),
        Some(assignee_id),
        "member",
        user_id,
        None,
        0.0,
        None,
        None,
        number,
        None,
        None,
        Uuid::now_v7(),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("create issue returned no row"))
}

async fn complete_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> anyhow::Result<(bool, patchbay_db::models::User)> {
    let first = user::claim_first_onboarding(&mut **tx, user_id)
        .await?
        .is_some();
    let user = user::get_user(&mut **tx, user_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("user not found"))?;
    if user.starter_content_state.is_none() {
        user::set_starter_content_state(&mut **tx, user_id, Some("imported")).await?;
    }
    Ok((first, user))
}

fn client_platform(headers: &HeaderMap) -> &str {
    headers
        .get("x-client-platform")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
}

#[allow(clippy::too_many_arguments)]
fn legacy_onboarding_metric_events(
    user_id: Uuid,
    workspace_id: Uuid,
    user: &patchbay_db::models::User,
    first_completion: bool,
    completion_path: &str,
    created_agent_id: Option<Uuid>,
    created_issue_id: Option<Uuid>,
    agent_created_meta: Option<(&str, &str, bool)>,
    platform: &str,
) -> Vec<patchbay_analytics::Event> {
    let mut events = Vec::new();
    if let Some(agent_id) = created_agent_id {
        if let Some((provider, runtime_mode, is_first_agent)) = agent_created_meta {
            events.push(patchbay_analytics::agent_created(
                &user_id.to_string(),
                &workspace_id.to_string(),
                &agent_id.to_string(),
                provider,
                runtime_mode,
                "patchbay_helper",
                is_first_agent,
            ));
        }
    }
    if let Some(issue_id) = created_issue_id {
        events.push(patchbay_analytics::issue_created(
            &user_id.to_string(),
            &workspace_id.to_string(),
            &issue_id.to_string(),
            &created_agent_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            "",
            "",
            patchbay_analytics::SOURCE_ONBOARDING,
            platform,
        ));
    }
    if first_completion {
        let onboarded_at = user
            .onboarded_at
            .map(crate::timefmt::rfc3339)
            .unwrap_or_default();
        events.push(patchbay_analytics::onboarding_completed(
            &user_id.to_string(),
            &workspace_id.to_string(),
            completion_path,
            &onboarded_at,
            user.cloud_waitlist_email.is_some(),
        ));
    }
    events
}

#[allow(clippy::too_many_arguments)]
fn record_legacy_onboarding_side_effects(
    state: &HandlerState,
    user_id: Uuid,
    workspace_id: Uuid,
    user: &patchbay_db::models::User,
    first_completion: bool,
    completion_path: &str,
    created_agent: Option<&Agent>,
    created_issue: Option<&Issue>,
    agent_created_meta: Option<(&str, &str, bool)>,
    platform: &str,
) {
    for event in legacy_onboarding_metric_events(
        user_id,
        workspace_id,
        user,
        first_completion,
        completion_path,
        created_agent.map(|agent| agent.id),
        created_issue.map(|issue| issue.id),
        agent_created_meta,
        platform,
    ) {
        patchbay_metrics::business_events::record_event(
            Some(state.analytics.as_ref()),
            state.business_metrics.as_deref(),
            &event,
        );
    }
}

async fn publish(
    state: &HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    created_agent: Option<&Agent>,
    created_issue: Option<&Issue>,
) {
    let issue_payload = if let Some(issue) = created_issue {
        Some(json!({
            "issue": crate::issue::issue_response_projection(state, issue).await,
        }))
    } else {
        None
    };
    for (event_type, payload) in [
        created_agent.map(|value| {
            (
                patchbay_protocol::EVENT_AGENT_CREATED,
                json!({"agent": value}),
            )
        }),
        issue_payload.map(|value| (patchbay_protocol::EVENT_ISSUE_CREATED, value)),
    ]
    .into_iter()
    .flatten()
    {
        state.bus.publish(&patchbay_events::Event {
            event_type: event_type.into(),
            workspace_id: workspace_id.to_string(),
            actor_type: "member".into(),
            actor_id: user_id.to_string(),
            payload,
            ..Default::default()
        });
    }
}

#[derive(Deserialize)]
struct RuntimeInput {
    #[serde(default)]
    workspace_id: String,
    #[serde(default)]
    runtime_id: String,
    #[serde(default)]
    starter_prompt: String,
}

async fn with_runtime(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    let user_id = match auth_user(&headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mut input: RuntimeInput = match body(&bytes) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if input.workspace_id.is_empty() || input.runtime_id.is_empty() {
        let field = if input.workspace_id.is_empty() {
            "workspace_id"
        } else {
            "runtime_id"
        };
        return error_response(StatusCode::BAD_REQUEST, &format!("{field} is required"));
    }
    input.starter_prompt = input.starter_prompt.trim().to_string();
    if input.starter_prompt.chars().count() > MAX_PROMPT {
        return error_response(
            StatusCode::BAD_REQUEST,
            &format!("starter_prompt exceeds {MAX_PROMPT} characters"),
        );
    }
    let workspace_id = match Uuid::parse_str(&input.workspace_id) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace_id"),
    };
    let runtime_id = match Uuid::parse_str(&input.runtime_id) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"),
    };
    let mut tx = match state.pool.begin().await {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to start onboarding",
            );
        }
    };
    let membership =
        match member::get_member_by_user_and_workspace(&mut *tx, user_id, workspace_id).await {
            Ok(Some(value)) => value,
            _ => return error_response(StatusCode::FORBIDDEN, "not a member of this workspace"),
        };
    let runtime =
        match runtime::get_agent_runtime_for_workspace(&mut *tx, runtime_id, workspace_id).await {
            Ok(Some(value)) => value,
            _ => return error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"),
        };
    if runtime.visibility == "private" && runtime.owner_id != Some(membership.user_id) {
        return error_response(
            StatusCode::FORBIDDEN,
            "this runtime is private; only its owner can create agents on it",
        );
    }
    let agents = match agent::list_agents(&mut *tx, workspace_id).await {
        Ok(value) => value,
        Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list agents");
        }
    };
    let is_first_agent = agents.is_empty();
    let mut made_agent = None;
    let helper = match agents
        .into_iter()
        .find(|value| value.name == HELPER_NAME && value.visibility == "workspace")
    {
        Some(value) => value,
        None => match agent::create_agent(
            &mut *tx,
            workspace_id,
            HELPER_NAME,
            HELPER_DESCRIPTION,
            Some(HELPER_AVATAR),
            &runtime.runtime_mode,
            &json!({}),
            runtime.id,
            "workspace",
            6,
            user_id,
            HELPER_INSTRUCTIONS,
            &json!({}),
            &json!([]),
            &json!(null),
            None,
            None,
            None,
            &[],
            None,
        )
        .await
        {
            Ok(Some(value)) => {
                made_agent = Some(value.clone());
                value
            }
            _ => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create onboarding assistant",
                );
            }
        },
    };
    let mut made_issue = None;
    let onboarding_issue = match find_duplicate(&mut tx, workspace_id, RUNTIME_TITLE).await {
        Ok(Some(value)) => value,
        Ok(None) => {
            let description = if input.starter_prompt.is_empty() {
                RUNTIME_DESCRIPTION
            } else {
                &input.starter_prompt
            };
            match seed_issue(
                &mut tx,
                workspace_id,
                RUNTIME_TITLE,
                description,
                "agent",
                helper.id,
                user_id,
            )
            .await
            {
                Ok(value) => {
                    made_issue = Some(value.clone());
                    value
                }
                Err(_) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to create onboarding issue",
                    );
                }
            }
        }
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create onboarding issue",
            );
        }
    };
    let (first_completion, user) = match complete_user(&mut tx, user_id).await {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to finish onboarding",
            )
        }
    };
    if tx.commit().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to finish onboarding",
        );
    }
    record_legacy_onboarding_side_effects(
        &state,
        user_id,
        workspace_id,
        &user,
        first_completion,
        patchbay_analytics::ONBOARDING_PATH_FULL,
        made_agent.as_ref(),
        made_issue.as_ref(),
        made_agent.as_ref().map(|_| {
            (
                runtime.provider.as_str(),
                runtime.runtime_mode.as_str(),
                is_first_agent,
            )
        }),
        client_platform(&headers),
    );
    publish(
        &state,
        workspace_id,
        user_id,
        made_agent.as_ref(),
        made_issue.as_ref(),
    )
    .await;
    if let Some(created) = made_issue.as_ref() {
        match patchbay_service::agent_ready::agent_readiness(&state.pool, &helper).await {
            Ok(verdict) if !verdict.blocked() => {
                if let Err(error) = state.tasks.enqueue_task_for_issue(created, None).await {
                    tracing::warn!(%error, issue_id = %created.id, "legacy onboarding enqueue failed");
                }
            }
            Ok(verdict) => {
                tracing::warn!(
                    issue_id = %created.id,
                    agent_id = %helper.id,
                    reason = %verdict.reason,
                    "legacy onboarding enqueue skipped because the runtime is unusable"
                );
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    issue_id = %created.id,
                    agent_id = %helper.id,
                    "legacy onboarding readiness check failed"
                );
            }
        }
    }
    Json(json!({"workspace_id": workspace_id, "agent_id": helper.id, "issue_id": onboarding_issue.id})).into_response()
}

#[derive(Deserialize)]
struct NoRuntimeInput {
    #[serde(default)]
    workspace_id: String,
}

fn no_runtime_copy(language: Option<&str>) -> &'static str {
    if language.is_some_and(|value| value.starts_with("zh")) {
        r#"欢迎来到 Patchbay。

智能体需要先连上运行时才能执行工作。运行时还没准备好时，你也可以先把 Patchbay 当作轻量项目管理工具体验起来。

## 先体验项目管理功能

运行时安装前，你可以先做这些事：

1. 为当前工作创建一个项目。
2. 新建几个 issue，并在 backlog、todo、in_progress、done 之间流转。
3. 给 issue 加优先级、标签、评论和订阅。
4. 用收件箱追踪分配给你的事项和 @mention。

这样你先熟悉项目管理层。连上运行时后，智能体会直接在这些 issue 上开始工作。

## 安装第一个 Agent 运行时

完整文档：https://patchbay.ai/docs/install-agent-runtime

中文用户建议先装 Kimi CLI：

1. 在 macOS / Linux 终端安装 Kimi CLI：
   curl -LsSf https://code.kimi.com/install.sh | bash
   Windows PowerShell：
   Invoke-RestMethod https://code.kimi.com/install.ps1 | Invoke-Expression
2. 确认终端能找到 Kimi：
   kimi --version
3. 在你想让 Kimi 工作的项目目录里启动一次：
   kimi
4. 首次启动后输入 /login，按提示完成 Kimi Code 或 API key 配置。
5. 重启 Patchbay 守护进程：
   patchbay daemon restart
   如果你用桌面端，重启 app 即可。
6. 回到 Runtimes 页面刷新。你应该能看到一个在线的 Kimi 运行时。
7. 用这个运行时创建第一个智能体，再把一个 issue 分配给它，并把状态切到 todo。

Kimi CLI 官方文档：https://moonshotai.github.io/kimi-cli/zh/guides/getting-started.html

运行时连上后，你就可以创建 Patchbay Helper，开始一次有智能体参与的上手引导。"#
    } else {
        r#"Welcome to Patchbay.

Agents need a runtime before they can execute work. You can still use Patchbay as a lightweight project-management workspace while you install one.

## Try Patchbay first

Before the runtime is ready, you can:

1. Create a project for your current work.
2. Create a few issues and move them across backlog, todo, in_progress, and done.
3. Add priorities, labels, comments, and subscriptions.
4. Use Inbox to track assignments and mentions.

That gives you the project-management layer first. Once a runtime is connected, agents can start working from the same issues.

## Install your first agent runtime

Full guide: https://patchbay.ai/docs/install-agent-runtime

For English users, the fastest first path is Codex:

1. Make sure Node.js is installed.
2. Install Codex:
   npm i -g @openai/codex
3. Sign in:
   codex
4. Confirm your terminal can find it:
   which codex
   codex --version
5. Restart the Patchbay daemon:
   patchbay daemon restart
   If you use the desktop app, restarting the app is enough.
6. Return to Runtimes and refresh. You should see a Codex runtime online.
7. Create your first agent from that runtime, then assign an issue to the agent and set status to todo.

Codex reference: https://developers.openai.com/codex/cli

When the runtime is connected, you can create Patchbay Helper for a guided first run."#
    }
}

async fn without_runtime(
    State(state): State<HandlerState>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    let user_id = match auth_user(&headers) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let input: NoRuntimeInput = match body(&bytes) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if input.workspace_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "workspace_id is required");
    }
    let workspace_id = match Uuid::parse_str(&input.workspace_id) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid workspace_id"),
    };
    let mut tx = match state.pool.begin().await {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to start onboarding",
            );
        }
    };
    if !matches!(
        member::get_member_by_user_and_workspace(&mut *tx, user_id, workspace_id).await,
        Ok(Some(_))
    ) {
        return error_response(StatusCode::FORBIDDEN, "not a member of this workspace");
    }
    let before = match user::get_user(&mut *tx, user_id).await {
        Ok(Some(value)) => value,
        _ => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load user"),
    };
    let mut made_issue = None;
    let onboarding_issue = match find_duplicate(&mut tx, workspace_id, NO_RUNTIME_TITLE).await {
        Ok(Some(value)) => value,
        Ok(None) => match seed_issue(
            &mut tx,
            workspace_id,
            NO_RUNTIME_TITLE,
            no_runtime_copy(before.language.as_deref()),
            "member",
            user_id,
            user_id,
        )
        .await
        {
            Ok(value) => {
                made_issue = Some(value.clone());
                value
            }
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to create onboarding issue",
                );
            }
        },
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create onboarding issue",
            );
        }
    };
    let (first_completion, user) = match complete_user(&mut tx, user_id).await {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to finish onboarding",
            )
        }
    };
    if tx.commit().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to finish onboarding",
        );
    }
    record_legacy_onboarding_side_effects(
        &state,
        user_id,
        workspace_id,
        &user,
        first_completion,
        patchbay_analytics::ONBOARDING_PATH_RUNTIME_SKIPPED,
        None,
        made_issue.as_ref(),
        None,
        client_platform(&headers),
    );
    publish(&state, workspace_id, user_id, None, made_issue.as_ref()).await;
    Json(json!({"workspace_id": workspace_id, "issue_id": onboarding_issue.id})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_prompt_limit_and_language_prefix_match_legacy_contract() {
        assert_eq!("你好".chars().count(), 2);
        assert!(no_runtime_copy(Some("zh-Hans")).starts_with("欢迎"));
        assert!(no_runtime_copy(Some("en")).starts_with("Welcome"));
    }

    #[test]
    fn first_completion_emits_established_analytics_side_effects() {
        let user_id = Uuid::now_v7();
        let workspace_id = Uuid::now_v7();
        let now = chrono::Utc::now();
        let user = patchbay_db::models::User {
            id: user_id,
            name: "Alex".into(),
            email: "alex@example.com".into(),
            is_guest: false,
            avatar_url: None,
            created_at: now,
            updated_at: now,
            onboarded_at: Some(now),
            onboarding_questionnaire: serde_json::json!({}),
            cloud_waitlist_email: None,
            cloud_waitlist_reason: None,
            starter_content_state: Some("imported".into()),
            language: None,
            profile_description: String::new(),
            timezone: None,
        };
        let agent_id = Uuid::now_v7();
        let issue_id = Uuid::now_v7();
        let events = legacy_onboarding_metric_events(
            user_id,
            workspace_id,
            &user,
            true,
            patchbay_analytics::ONBOARDING_PATH_FULL,
            Some(agent_id),
            Some(issue_id),
            Some(("cursor", "cloud", true)),
            "desktop",
        );
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].name, patchbay_analytics::EVENT_AGENT_CREATED);
        assert_eq!(events[1].name, patchbay_analytics::EVENT_ISSUE_CREATED);
        assert_eq!(
            events[2].name,
            patchbay_analytics::EVENT_ONBOARDING_COMPLETED
        );
        let repeat = legacy_onboarding_metric_events(
            user_id,
            workspace_id,
            &user,
            false,
            patchbay_analytics::ONBOARDING_PATH_FULL,
            None,
            None,
            None,
            "desktop",
        );
        assert!(repeat.is_empty());
    }
}
