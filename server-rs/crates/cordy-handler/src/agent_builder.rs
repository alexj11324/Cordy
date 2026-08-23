//! Agent Builder session endpoints.

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, put};
use axum::{Json, Router};
use cordy_db::models::{Agent, AgentRuntime, ChatSession, Member};
use cordy_db::queries::{agent, agent_builder, chat, runtime, workspace};
use cordy_middleware::workspace::WorkspaceContext;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use uuid::Uuid;

use crate::error::error_response;
use crate::state::HandlerState;

const AGENT_BUILDER_INSTRUCTIONS: &str = r#"You are Cordy Agent Builder. Help the user design one practical AI agent through a short conversation.

Your job is to propose and refine configuration, never to create resources yourself. Ask only questions that materially change behavior. Prefer making a reasonable draft immediately, then ask at most two focused questions per turn.

Every response MUST end with exactly one <agent_draft> JSON block using this shape:
<agent_draft>{"name":"","description":"","instructions":"","model":"","skill_ids":[],"permission_scope":"private","member_ids":[]}</agent_draft>

Rules:
- The JSON must be valid, compact JSON on one physical line. Do not wrap it in Markdown fences.
- Escape every line break inside instructions as \n. Never place a literal newline inside a JSON string.
- Preserve good existing draft fields supplied in the user's message unless the user asks to change them.
- name is concise and suitable for a workspace list.
- description is one sentence, at most 200 characters.
- instructions are a complete Markdown system prompt describing role, workflow, output, and constraints.
- model must be empty, preserve current_draft.model, or exactly match an id explicitly listed in AVAILABLE RUNTIME MODELS. Never use a model label as the id.
- When AVAILABLE RUNTIME MODELS is null or empty, preserve current_draft.model and never invent a model id.
- skill_ids may only contain IDs explicitly listed in AVAILABLE WORKSPACE SKILLS.
- permission_scope must be private, workspace, or members. Default to private unless the user explicitly requests sharing.
- member_ids may only contain IDs explicitly listed in AVAILABLE WORKSPACE MEMBERS, and only when permission_scope is members.
- Never request, expose, or place secrets, tokens, passwords, or environment-variable values in the draft.
- Do not claim that the agent has been created. The user must review and confirm the draft in the UI."#;

const MAX_AGENT_BUILDER_DRAFT_BYTES: usize = 256 * 1024;

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route(
            "/api/agent-builder/sessions",
            get(list_sessions).post(create_session),
        )
        .route(
            "/api/agent-builder/sessions/",
            get(list_sessions).post(create_session),
        )
        .route(
            "/api/agent-builder/sessions/{session_id}/runtime",
            patch(switch_runtime),
        )
        .route(
            "/api/agent-builder/sessions/{session_id}/runtime/",
            patch(switch_runtime),
        )
        .route(
            "/api/agent-builder/sessions/{session_id}/draft",
            put(save_draft),
        )
        .route(
            "/api/agent-builder/sessions/{session_id}/draft/",
            put(save_draft),
        )
}

fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

fn decode_first<T>(body: &[u8]) -> Result<T, ()>
where
    T: for<'de> Deserialize<'de> + Default,
{
    let mut values = serde_json::Deserializer::from_slice(body).into_iter::<Option<T>>();
    values
        .next()
        .ok_or(())?
        .map(|value| value.unwrap_or_default())
        .map_err(|_| ())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CreateSessionRequest {
    #[serde(deserialize_with = "null_default")]
    runtime_id: String,
    #[serde(deserialize_with = "null_default")]
    model: String,
}

#[derive(Debug, Serialize)]
struct CreateSessionResponse {
    session_id: String,
    builder_agent_id: String,
    runtime_id: String,
}

fn can_use_runtime(member: &Member, runtime: &AgentRuntime) -> bool {
    runtime.owner_id.is_some()
        && (runtime.visibility == "public" || runtime.owner_id == Some(member.user_id))
}

async fn resolve_runtime(
    state: &HandlerState,
    context: &WorkspaceContext,
    runtime_id: &str,
    verb: &str,
) -> Result<AgentRuntime, Response> {
    let runtime_uuid = Uuid::parse_str(runtime_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"))?;
    let runtime = runtime::get_agent_runtime_for_workspace(
        &state.pool,
        runtime_uuid,
        context.member.workspace_id,
    )
    .await
    .ok()
    .flatten()
    .ok_or_else(|| error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"))?;
    if !can_use_runtime(&context.member, &runtime) {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "this runtime is private; only its owner can use it",
        ));
    }
    if runtime.status != "online" {
        return Err(error_response(
            StatusCode::CONFLICT,
            &format!("runtime must be online to {verb} an agent builder session"),
        ));
    }
    Ok(runtime)
}

async fn create_session(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    body: Bytes,
) -> Response {
    let request = match decode_first::<CreateSessionRequest>(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let runtime_id = request.runtime_id.trim();
    if runtime_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "runtime_id is required");
    }
    let target_runtime = match resolve_runtime(&state, &context, runtime_id, "start").await {
        Ok(runtime) => runtime,
        Err(response) => return response,
    };

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "failed to start agent builder session transaction");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to start agent builder session",
            );
        }
    };
    match workspace::lock_workspace_for_chat_session_create(
        &mut *transaction,
        context.member.workspace_id,
    )
    .await
    {
        Ok(Some(_)) => {}
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "workspace not found"),
        Err(error) => {
            tracing::warn!(%error, "failed to lock workspace for agent builder");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to lock workspace",
            );
        }
    }

    let flow_id = Uuid::new_v4().to_string();
    let model = request.model.trim();
    let system_key = format!("agent_builder:{flow_id}");
    let builder = match agent::create_agent_builder(
        &mut *transaction,
        context.member.workspace_id,
        &format!(".cordy-agent-builder-{flow_id}"),
        &target_runtime.runtime_mode,
        target_runtime.id,
        context.member.user_id,
        AGENT_BUILDER_INSTRUCTIONS,
        (!model.is_empty()).then_some(model),
        Some(&system_key),
    )
    .await
    {
        Ok(Some(builder)) => builder,
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to prepare agent builder",
            )
        }
    };
    let session = match chat::create_chat_session(
        &mut *transaction,
        context.member.workspace_id,
        builder.id,
        context.member.user_id,
        "Create an agent",
        false,
        Uuid::nil(),
        cordy_db::dbid::new_v7(),
    )
    .await
    {
        Ok(Some(session)) => session,
        Ok(None) | Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create agent builder session",
            )
        }
    };
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, "failed to commit agent builder session");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to commit agent builder session",
        );
    }

    (
        StatusCode::CREATED,
        Json(CreateSessionResponse {
            session_id: session.id.to_string(),
            builder_agent_id: builder.id.to_string(),
            runtime_id: runtime_id.to_string(),
        }),
    )
        .into_response()
}

#[derive(Debug, Serialize)]
struct SessionSummary {
    session_id: String,
    title: String,
    runtime_id: String,
    created_at: String,
    updated_at: String,
    last_message_content: String,
    last_message_role: String,
    last_message_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    draft: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ListSessionsResponse {
    sessions: Vec<SessionSummary>,
}

async fn list_sessions(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
) -> Response {
    match chat::list_agent_builder_sessions_by_creator(
        &state.pool,
        context.member.workspace_id,
        context.member.user_id,
    )
    .await
    {
        Ok(rows) => Json(ListSessionsResponse {
            sessions: rows
                .into_iter()
                .map(|row| SessionSummary {
                    session_id: row.id.map(|id| id.to_string()).unwrap_or_default(),
                    title: row.title,
                    runtime_id: row.runtime_id.map(|id| id.to_string()).unwrap_or_default(),
                    created_at: row
                        .created_at
                        .map(crate::timefmt::rfc3339)
                        .unwrap_or_default(),
                    updated_at: row
                        .updated_at
                        .map(crate::timefmt::rfc3339)
                        .unwrap_or_default(),
                    last_message_content: row.last_message_content,
                    last_message_role: row.last_message_role,
                    last_message_at: row
                        .last_message_at
                        .map(crate::timefmt::rfc3339)
                        .unwrap_or_default(),
                    draft: row.stored_draft,
                })
                .collect(),
        })
        .into_response(),
        Err(error) => {
            tracing::warn!(%error, "failed to list agent builder sessions");
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list agent builder sessions",
            )
        }
    }
}

fn deserialize_present_raw<'de, D>(deserializer: D) -> Result<Option<Box<RawValue>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Box::<RawValue>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SaveDraftRequest {
    #[serde(deserialize_with = "deserialize_present_raw")]
    draft: Option<Box<RawValue>>,
}

async fn load_session(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw_session_id: &str,
) -> Result<ChatSession, Response> {
    let session_id = Uuid::parse_str(raw_session_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid chat session id"))?;
    let session =
        chat::get_chat_session_in_workspace(&state.pool, session_id, context.member.workspace_id)
            .await
            .ok()
            .flatten()
            .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "chat session not found"))?;
    if session.creator_id != context.member.user_id {
        return Err(error_response(
            StatusCode::FORBIDDEN,
            "not your chat session",
        ));
    }
    Ok(session)
}

fn is_builder_carrier(agent: &Agent) -> bool {
    agent.kind == "system"
        && agent
            .system_key
            .as_deref()
            .is_some_and(|key| key.starts_with("agent_builder:"))
}

async fn load_builder_agent(
    state: &HandlerState,
    session: &ChatSession,
) -> Result<Agent, Response> {
    agent::get_agent(&state.pool, session.agent_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load chat agent",
            )
        })
}

async fn save_draft(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(session_id): Path<String>,
    body: Bytes,
) -> Response {
    let request = match decode_first::<SaveDraftRequest>(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let Some(raw_draft) = request.draft else {
        return error_response(StatusCode::BAD_REQUEST, "draft is required");
    };
    if raw_draft.get().len() > MAX_AGENT_BUILDER_DRAFT_BYTES {
        return error_response(StatusCode::PAYLOAD_TOO_LARGE, "draft is too large");
    }
    let draft = match serde_json::from_str::<serde_json::Value>(raw_draft.get()) {
        Ok(draft) => draft,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "draft must be valid JSON"),
    };

    let session = match load_session(&state, &context, &session_id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    let builder = match load_builder_agent(&state, &session).await {
        Ok(agent) => agent,
        Err(response) => return response,
    };
    if !is_builder_carrier(&builder) {
        return error_response(StatusCode::NOT_FOUND, "agent builder session not found");
    }

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "failed to start agent builder draft transaction");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to save agent builder draft",
            );
        }
    };
    let locked = match chat::lock_chat_session_for_draft_write(&mut *transaction, session.id).await
    {
        Ok(Some(locked)) => locked,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "chat session not found"),
        Err(error) => {
            tracing::warn!(%error, "failed to lock chat session for draft write");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to lock chat session",
            );
        }
    };
    if locked.status != "active" {
        return error_response(StatusCode::BAD_REQUEST, "chat session is archived");
    }
    if agent_builder::upsert_agent_builder_draft(
        &mut *transaction,
        locked.id,
        locked.workspace_id,
        &draft,
    )
    .await
    .ok()
    .flatten()
    .is_none()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to save agent builder draft",
        );
    }
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, "failed to commit agent builder draft");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to save agent builder draft",
        );
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SwitchRuntimeRequest {
    #[serde(deserialize_with = "null_default")]
    runtime_id: String,
}

#[derive(Debug, Serialize)]
struct SwitchRuntimeResponse {
    runtime_id: String,
}

async fn switch_runtime(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(session_id): Path<String>,
    body: Bytes,
) -> Response {
    let request = match decode_first::<SwitchRuntimeRequest>(&body) {
        Ok(request) => request,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let runtime_id = request.runtime_id.trim();
    if runtime_id.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "runtime_id is required");
    }
    let session = match load_session(&state, &context, &session_id).await {
        Ok(session) => session,
        Err(response) => return response,
    };
    if session.status != "active" {
        return error_response(StatusCode::BAD_REQUEST, "chat session is archived");
    }
    let builder = match load_builder_agent(&state, &session).await {
        Ok(agent) => agent,
        Err(response) => return response,
    };
    if !is_builder_carrier(&builder) {
        return error_response(StatusCode::NOT_FOUND, "agent builder session not found");
    }
    let target_runtime = match resolve_runtime(&state, &context, runtime_id, "switch").await {
        Ok(runtime) => runtime,
        Err(response) => return response,
    };

    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => {
            tracing::warn!(%error, "failed to start agent builder runtime transaction");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to switch agent builder runtime",
            );
        }
    };
    match chat::lock_chat_session_for_runtime_bind(&mut *transaction, session.id).await {
        Ok(Some(_)) => {}
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "chat session not found"),
        Err(error) => {
            tracing::warn!(%error, "failed to lock chat session for runtime switch");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to lock chat session",
            );
        }
    }
    match chat::get_pending_chat_task(&mut *transaction, session.id).await {
        Ok(Some(_)) => {
            return error_response(
                StatusCode::CONFLICT,
                "stop the current reply before switching runtime",
            )
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(%error, "failed to check pending builder task");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to check pending builder task",
            );
        }
    }
    let updated = match agent::rebind_agent_builder_runtime(
        &mut *transaction,
        target_runtime.id,
        &target_runtime.runtime_mode,
        None,
        builder.id,
    )
    .await
    {
        Ok(Some(updated)) => updated,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, "agent builder session not found")
        }
        Err(error) => {
            tracing::warn!(%error, "failed to rebind agent builder runtime");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to switch agent builder runtime",
            );
        }
    };
    if let Err(error) = transaction.commit().await {
        tracing::warn!(%error, "failed to commit agent builder runtime switch");
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to commit agent builder runtime switch",
        );
    }
    Json(SwitchRuntimeResponse {
        runtime_id: updated
            .runtime_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn member(user_id: Uuid) -> Member {
        Member {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            user_id,
            role: "member".into(),
            created_at: Utc::now(),
        }
    }

    fn runtime(owner_id: Option<Uuid>, visibility: &str) -> AgentRuntime {
        AgentRuntime {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            daemon_id: None,
            name: "runtime".into(),
            runtime_mode: "local".into(),
            provider: "codex".into(),
            status: "online".into(),
            device_info: String::new(),
            metadata: serde_json::json!({}),
            last_seen_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            owner_id,
            legacy_daemon_id: None,
            visibility: visibility.into(),
            profile_id: None,
            custom_name: None,
        }
    }

    #[test]
    fn runtime_access_matches_private_owner_contract() {
        let user_id = Uuid::new_v4();
        let caller = member(user_id);
        assert!(can_use_runtime(&caller, &runtime(Some(user_id), "private")));
        assert!(can_use_runtime(
            &caller,
            &runtime(Some(Uuid::new_v4()), "public")
        ));
        assert!(!can_use_runtime(
            &caller,
            &runtime(Some(Uuid::new_v4()), "private")
        ));
        assert!(!can_use_runtime(&caller, &runtime(None, "public")));
    }

    #[test]
    fn draft_decoder_distinguishes_missing_from_json_null_and_preserves_size() {
        let missing = decode_first::<SaveDraftRequest>(br#"{}"#).unwrap();
        assert!(missing.draft.is_none());

        let null = decode_first::<SaveDraftRequest>(br#"{"draft": null}"#).unwrap();
        let raw = null.draft.unwrap();
        assert_eq!(raw.get(), "null");

        let spaced = decode_first::<SaveDraftRequest>(br#"{"draft": { "name": "A" }}"#).unwrap();
        assert_eq!(spaced.draft.unwrap().get(), r#"{ "name": "A" }"#);
    }

    #[test]
    fn session_summary_omits_absent_draft_but_preserves_json_null() {
        let base = SessionSummary {
            session_id: String::new(),
            title: String::new(),
            runtime_id: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            last_message_content: String::new(),
            last_message_role: String::new(),
            last_message_at: String::new(),
            draft: None,
        };
        let absent = serde_json::to_value(&base).unwrap();
        assert!(absent.get("draft").is_none());

        let with_null = SessionSummary {
            draft: Some(serde_json::Value::Null),
            ..base
        };
        assert!(serde_json::to_value(with_null).unwrap()["draft"].is_null());
    }

    #[test]
    fn builder_prompt_keeps_model_and_secret_constraints() {
        for requirement in [
            "AVAILABLE RUNTIME MODELS",
            "Never use a model label as the id",
            "Never request, expose, or place secrets",
            "Do not claim that the agent has been created",
        ] {
            assert!(AGENT_BUILDER_INSTRUCTIONS.contains(requirement));
        }
    }
}
