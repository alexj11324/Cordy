//! User-agent CRUD, lifecycle, skills, labels, task history, and env routes.

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use cordy_db::models::{Agent, AgentInvocationTarget};
use cordy_db::queries::{
    agent, agent_invocation_target, chat, issue_label, runtime, skill, workspace,
};
use cordy_middleware::workspace::WorkspaceContext;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::error_response;
use crate::HandlerState;

const ENV_SENTINEL: &str = "****";

pub fn router() -> Router<HandlerState> {
    Router::new()
        .route("/api/agents", get(list_agents).post(create_agent))
        .route("/api/agents/mika", post(create_mika))
        .route("/api/agents/{id}", get(get_agent).put(update_agent))
        .route("/api/agents/{id}/archive", post(archive_agent))
        .route("/api/agents/{id}/restore", post(restore_agent))
        .route("/api/agents/{id}/cancel-tasks", post(cancel_tasks))
        .route("/api/agents/{id}/tasks", get(list_tasks))
        .route("/api/agents/{id}/env", get(get_env).put(update_env))
        .route(
            "/api/agents/{id}/labels",
            get(list_labels).post(attach_label),
        )
        .route("/api/agents/{id}/labels/{label_id}", delete(detach_label))
        .route("/api/agents/{id}/skills", get(list_skills).put(set_skills))
        .route("/api/agents/{id}/skills/add", post(add_skills))
        .route("/api/agents/{id}/skills/{skill_id}", delete(remove_skill))
        .route(
            "/api/agents/{id}/skills/{skill_id}/enabled",
            put(set_skill_enabled),
        )
        .route(
            "/api/agents/{id}/runtime-skills/enabled",
            put(set_runtime_skill_enabled),
        )
}

fn workspace_id(context: &WorkspaceContext) -> Result<Uuid, Response> {
    Uuid::parse_str(&context.workspace_id)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid workspace id"))
}

fn can_manage(context: &WorkspaceContext, target: &Agent) -> bool {
    matches!(context.member.role.as_str(), "owner" | "admin")
        || target.owner_id == Some(context.member.user_id)
}

fn member_can_view(
    context: &WorkspaceContext,
    target: &Agent,
    targets: &[AgentInvocationTarget],
) -> bool {
    can_manage(context, target)
        || target.permission_mode == "public_to"
            && targets.iter().any(|allowed| {
                allowed.target_type == "workspace"
                    || allowed.target_type == "member"
                        && allowed.target_id == context.member.user_id
            })
}

fn apply_targets(response: &mut Value, targets: &[AgentInvocationTarget]) {
    response["invocation_targets"] = Value::Array(
        targets
            .iter()
            .map(|target| {
                json!({
                    "target_type": target.target_type,
                    "target_id": target.target_id,
                })
            })
            .collect(),
    );
    response["visibility"] = Value::String(
        if response["permission_mode"] == "public_to"
            && targets
                .iter()
                .any(|target| target.target_type == "workspace")
        {
            "workspace"
        } else {
            "private"
        }
        .into(),
    );
}

fn apply_skills<T: serde::Serialize>(response: &mut Value, skills: &T) {
    response["skills"] = serde_json::to_value(skills).unwrap_or_else(|_| json!([]));
}

async fn hydrated_agent_response(
    state: &HandlerState,
    context: &WorkspaceContext,
    headers: &HeaderMap,
    target: Agent,
) -> Result<Value, Response> {
    let (actor_type, _, _) = crate::issue::mutation_actor(state, context, headers).await;
    let is_agent = actor_type == "agent";
    let always_redact = workspace::get_workspace(&state.pool, target.workspace_id)
        .await
        .map_err(|_| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load workspace settings",
            )
        })?
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "workspace not found"))?
        .settings["always_redact_env"]
        .as_bool()
        .unwrap_or(false);
    let reveal_secrets = !is_agent && !always_redact && can_manage(context, &target);
    let reveal_composio = !is_agent && target.owner_id == Some(context.member.user_id);
    let targets = agent_invocation_target::list_agent_invocation_targets(&state.pool, target.id)
        .await
        .map_err(|_| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load agent invocation targets",
            )
        })?;
    let skills = skill::list_agent_skill_summaries(&state.pool, target.id)
        .await
        .map_err(|_| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load agent skills",
            )
        })?;
    let mut response = agent_response(Some(state), target, reveal_secrets, reveal_composio);
    apply_targets(&mut response, &targets);
    apply_skills(&mut response, &skills);
    Ok(response)
}

async fn load_agent(
    state: &HandlerState,
    context: &WorkspaceContext,
    raw: &str,
) -> Result<Agent, Response> {
    let id = Uuid::parse_str(raw)
        .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid agent id"))?;
    let workspace_id = workspace_id(context)?;
    agent::get_agent_in_workspace(&state.pool, id, workspace_id)
        .await
        .map_err(|error| {
            tracing::warn!(%error, %id, "failed to load agent");
            error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load agent")
        })?
        .ok_or_else(|| error_response(StatusCode::NOT_FOUND, "agent not found"))
}

fn manage_or_forbidden(context: &WorkspaceContext, target: &Agent) -> Result<(), Response> {
    can_manage(context, target).then_some(()).ok_or_else(|| {
        error_response(
            StatusCode::FORBIDDEN,
            "only the agent owner or a workspace owner/admin can manage this agent",
        )
    })
}

fn env_map(agent: &Agent) -> BTreeMap<String, String> {
    serde_json::from_value(agent.custom_env.clone()).unwrap_or_default()
}

#[derive(Debug, Default, PartialEq)]
struct EnvAudit {
    added: Vec<String>,
    removed: Vec<String>,
    changed: Vec<String>,
    preserved: Vec<String>,
}

fn merge_agent_env(
    existing: &BTreeMap<String, String>,
    requested: BTreeMap<String, String>,
) -> (BTreeMap<String, String>, EnvAudit) {
    let mut merged = BTreeMap::new();
    let mut audit = EnvAudit::default();
    for (key, value) in requested {
        if value == ENV_SENTINEL {
            if let Some(old) = existing.get(&key) {
                merged.insert(key.clone(), old.clone());
                audit.preserved.push(key);
            }
            continue;
        }
        match existing.get(&key) {
            Some(old) if old == &value => {
                merged.insert(key, value);
            }
            Some(_) => {
                merged.insert(key.clone(), value);
                audit.changed.push(key);
            }
            None => {
                merged.insert(key.clone(), value);
                audit.added.push(key);
            }
        }
    }
    audit.removed.extend(
        existing
            .keys()
            .filter(|key| !merged.contains_key(*key))
            .cloned(),
    );
    (merged, audit)
}

fn mask_gateway_token(mut config: Value) -> Value {
    if let Some(token) = config
        .get_mut("gateway")
        .and_then(Value::as_object_mut)
        .and_then(|gateway| gateway.get_mut("token"))
    {
        if token.as_str().is_some_and(|value| !value.is_empty()) {
            *token = Value::String("***".into());
        }
    }
    config
}

fn preserve_masked_gateway_token(incoming: &mut Value, existing: &Value) {
    let Some(token) = incoming
        .get_mut("gateway")
        .and_then(Value::as_object_mut)
        .and_then(|gateway| gateway.get_mut("token"))
    else {
        return;
    };
    if token.as_str() != Some("***") {
        return;
    }
    if let Some(old) = existing
        .get("gateway")
        .and_then(Value::as_object)
        .and_then(|gateway| gateway.get("token"))
        .filter(|value| value.as_str().is_some_and(|token| !token.is_empty()))
    {
        *token = old.clone();
        return;
    }
    if let Some(gateway) = incoming.get_mut("gateway").and_then(Value::as_object_mut) {
        gateway.remove("token");
    }
}

fn valid_catalog_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn known_thinking_value(provider: &str, value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    if matches!(
        provider,
        "codex" | "dsh" | "opencode" | "kimi" | "reasonix" | "hermes" | "dim"
    ) {
        return valid_catalog_token(value);
    }
    match provider {
        "claude" => matches!(value, "low" | "medium" | "high" | "xhigh" | "max"),
        "codebuddy" => matches!(
            value,
            "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
        ),
        "grok" => matches!(value, "low" | "medium" | "high"),
        "pi" => matches!(
            value,
            "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
        ),
        _ => false,
    }
}

fn known_service_tier(provider: &str, value: &str) -> bool {
    value.is_empty() || provider == "codex" && valid_catalog_token(value)
}

fn runtime_specific_model(model: &str) -> bool {
    model.contains('/')
        || model.starts_with("claude-")
        || model.starts_with("gpt-")
        || model.starts_with("gemini-")
        || model.starts_with("auto-gemini-")
        || model
            .strip_prefix('o')
            .and_then(|rest| rest.as_bytes().first())
            .is_some_and(u8::is_ascii_digit)
}

fn model_known_incompatible(provider: &str, model: &str) -> bool {
    let model = model.trim();
    if model.is_empty() || !runtime_specific_model(model) {
        return false;
    }
    match provider {
        "claude" => {
            let base = model
                .rsplit_once('[')
                .filter(|(_, suffix)| {
                    suffix.ends_with(']')
                        && suffix[..suffix.len() - 1]
                            .strip_suffix(['k', 'm'])
                            .is_some_and(|digits| {
                                !digits.is_empty()
                                    && !digits.starts_with('0')
                                    && digits.bytes().all(|byte| byte.is_ascii_digit())
                            })
                })
                .map_or(model, |(base, _)| base);
            !matches!(
                base,
                "claude-sonnet-5"
                    | "claude-sonnet-4-6"
                    | "claude-fable-5"
                    | "claude-opus-5"
                    | "claude-opus-4-8"
                    | "claude-opus-4-7"
                    | "claude-haiku-4-5-20251001"
                    | "claude-opus-4-6"
                    | "claude-sonnet-4-5"
            )
        }
        "codex" => !matches!(
            model,
            "gpt-5.6-sol"
                | "gpt-5.6-terra"
                | "gpt-5.6-luna"
                | "gpt-5.5"
                | "gpt-5.4"
                | "gpt-5.4-mini"
                | "gpt-5.3-codex"
                | "gpt-5.2"
        ),
        _ => false,
    }
}

fn normalise_allowlist(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty() && seen.insert(value.clone()))
        .collect()
}

fn system_instructions_for(system_key: Option<&str>, display_name: &str) -> String {
    if system_key == Some(cordy_service::builtin_agents::MIKA_SYSTEM_KEY) {
        cordy_service::builtin_agents::mika_system_instructions(display_name)
    } else {
        String::new()
    }
}

fn agent_response(
    state: Option<&HandlerState>,
    target: Agent,
    reveal_secrets: bool,
    reveal_composio: bool,
) -> Value {
    let env_count = env_map(&target).len();
    let system_instructions = system_instructions_for(target.system_key.as_deref(), &target.name);
    let mut mcp_config = target.mcp_config.clone().unwrap_or_else(|| json!({}));
    let mut mcp_config_redacted = false;
    if !reveal_secrets && has_mcp_config(&mcp_config) {
        mcp_config = json!({});
        mcp_config_redacted = true;
    }
    let composio_toolkit_allowlist = reveal_composio.then(|| {
        target
            .composio_toolkit_allowlist
            .clone()
            .unwrap_or_default()
    });
    let composio_toolkit_allowlist_redacted = !reveal_composio
        && target
            .composio_toolkit_allowlist
            .as_ref()
            .is_some_and(|allowlist| !allowlist.is_empty());
    let avatar_url = target.avatar_url.as_deref().map(|raw| {
        state.map_or_else(
            || raw.to_string(),
            |state| crate::avatar::resolve_url(state, raw),
        )
    });
    json!({
        "id": target.id,
        "workspace_id": target.workspace_id,
        "runtime_id": target.runtime_id.map(|id| id.to_string()).unwrap_or_default(),
        "runtime_bound": target.runtime_id.is_some(),
        "name": target.name,
        "description": target.description,
        "instructions": target.instructions,
        "system_key": target.system_key.unwrap_or_default(),
        "system_instructions": system_instructions,
        "avatar_url": avatar_url,
        "runtime_mode": target.runtime_mode,
        "runtime_config": mask_gateway_token(target.runtime_config),
        "custom_args": target.custom_args,
        "mcp_config": mcp_config,
        "has_custom_env": env_count > 0,
        "custom_env_key_count": env_count,
        "mcp_config_redacted": mcp_config_redacted,
        "visibility": target.visibility,
        "permission_mode": target.permission_mode,
        "invocation_targets": [],
        "status": target.status,
        "max_concurrent_tasks": target.max_concurrent_tasks,
        "model": target.model.unwrap_or_default(),
        "thinking_level": target.thinking_level.unwrap_or_default(),
        "service_tier": target.service_tier.unwrap_or_default(),
        "composio_toolkit_allowlist": composio_toolkit_allowlist,
        "composio_toolkit_allowlist_redacted": composio_toolkit_allowlist_redacted,
        "owner_id": target.owner_id,
        "skills": [],
        "disabled_runtime_skills": target.disabled_runtime_skills,
        "created_at": crate::timefmt::rfc3339(target.created_at),
        "updated_at": crate::timefmt::rfc3339(target.updated_at),
        "archived_at": target.archived_at.map(crate::timefmt::rfc3339),
        "archived_by": target.archived_by,
    })
}

pub(crate) fn agent_event_response(state: &HandlerState, target: &Agent) -> Value {
    agent_response(Some(state), target.clone(), false, false)
}

fn has_mcp_config(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Object(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::String(value) => !value.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

#[derive(Default, Deserialize)]
struct ListParams {
    #[serde(default)]
    include_archived: bool,
}

async fn list_agents(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Query(params): Query<ListParams>,
    headers: HeaderMap,
) -> Response {
    let workspace_id = match workspace_id(&context) {
        Ok(id) => id,
        Err(response) => return response,
    };
    let rows = if params.include_archived {
        agent::list_all_agents(&state.pool, workspace_id).await
    } else {
        agent::list_agents(&state.pool, workspace_id).await
    };
    let rows = match rows {
        Ok(rows) => rows,
        Err(_) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to list agents")
        }
    };
    let (actor_type, _, _) = crate::issue::mutation_actor(&state, &context, &headers).await;
    let is_agent = actor_type == "agent";
    let always_redact = match workspace::get_workspace(&state.pool, workspace_id).await {
        Ok(Some(workspace)) => workspace.settings["always_redact_env"]
            .as_bool()
            .unwrap_or(false),
        _ => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load workspace settings",
            )
        }
    };
    let invocation_targets =
        match agent_invocation_target::list_agent_invocation_targets_by_agent_i_ds(
            &state.pool,
            rows.iter().map(|target| target.id).collect(),
        )
        .await
        {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(%error, %workspace_id, "failed to load agent invocation targets");
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load agent invocation targets",
                );
            }
        };
    let mut targets_by_agent: HashMap<Uuid, Vec<AgentInvocationTarget>> = HashMap::new();
    for target in invocation_targets {
        targets_by_agent
            .entry(target.agent_id)
            .or_default()
            .push(target);
    }
    let skill_rows = match skill::list_agent_skills_by_workspace(&state.pool, workspace_id).await {
        Ok(rows) => rows,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load agent skills",
            )
        }
    };
    let mut skills_by_agent = HashMap::new();
    for row in skill_rows {
        if let Some(agent_id) = row.agent_id {
            skills_by_agent
                .entry(agent_id)
                .or_insert_with(Vec::new)
                .push(row);
        }
    }
    Json(
        rows.into_iter()
            .filter(|target| {
                is_agent
                    || member_can_view(
                        &context,
                        target,
                        targets_by_agent
                            .get(&target.id)
                            .map(Vec::as_slice)
                            .unwrap_or_default(),
                    )
            })
            .map(|target| {
                let target_id = target.id;
                let reveal = !is_agent && !always_redact && can_manage(&context, &target);
                let targets = targets_by_agent
                    .get(&target.id)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let reveal_composio = !is_agent && target.owner_id == Some(context.member.user_id);
                let mut response = agent_response(Some(&state), target, reveal, reveal_composio);
                apply_targets(&mut response, targets);
                if let Some(skills) = skills_by_agent.get(&target_id) {
                    apply_skills(&mut response, skills);
                }
                response
            })
            .collect::<Vec<_>>(),
    )
    .into_response()
}

async fn get_agent(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(target) => target,
        Err(response) => return response,
    };
    let targets = match agent_invocation_target::list_agent_invocation_targets(
        &state.pool,
        target.id,
    )
    .await
    {
        Ok(targets) => targets,
        Err(error) => {
            tracing::warn!(%error, agent_id = %target.id, "failed to load agent invocation targets");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load agent invocation targets",
            );
        }
    };
    let (actor_type, _, _) = crate::issue::mutation_actor(&state, &context, &headers).await;
    let is_agent = actor_type == "agent";
    if !is_agent && !member_can_view(&context, &target, &targets) {
        return error_response(
            StatusCode::FORBIDDEN,
            "you do not have access to this agent",
        );
    }
    let always_redact = match workspace::get_workspace(&state.pool, target.workspace_id).await {
        Ok(Some(workspace)) => workspace.settings["always_redact_env"]
            .as_bool()
            .unwrap_or(false),
        _ => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load workspace settings",
            )
        }
    };
    let target_id = target.id;
    let reveal = !is_agent && !always_redact && can_manage(&context, &target);
    let reveal_composio = !is_agent && target.owner_id == Some(context.member.user_id);
    let mut response = agent_response(Some(&state), target, reveal, reveal_composio);
    apply_targets(&mut response, &targets);
    match skill::list_agent_skill_summaries(&state.pool, target_id).await {
        Ok(skills) => apply_skills(&mut response, &skills),
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load agent skills",
            )
        }
    }
    Json(response).into_response()
}

#[derive(Default, Deserialize)]
struct AgentWrite {
    name: Option<String>,
    description: Option<String>,
    instructions: Option<String>,
    avatar_url: Option<String>,
    runtime_id: Option<String>,
    runtime_config: Option<Value>,
    custom_env: Option<BTreeMap<String, String>>,
    custom_args: Option<Vec<String>>,
    mcp_config: Option<Value>,
    visibility: Option<String>,
    permission_mode: Option<String>,
    invocation_targets: Option<Vec<InvocationTargetInput>>,
    status: Option<String>,
    max_concurrent_tasks: Option<i32>,
    model: Option<String>,
    thinking_level: Option<String>,
    service_tier: Option<String>,
    composio_toolkit_allowlist: Option<Vec<String>>,
    #[serde(default)]
    template: String,
    #[serde(default)]
    skill_ids: Vec<String>,
}

#[derive(Clone, Deserialize)]
struct InvocationTargetInput {
    target_type: String,
    target_id: Option<String>,
}

fn resolve_permission(
    workspace_id: Uuid,
    permission_mode: Option<&str>,
    visibility: Option<&str>,
    targets: Option<&[InvocationTargetInput]>,
) -> Result<(String, Vec<(String, Uuid)>), Response> {
    let permission_mode = permission_mode.unwrap_or_else(|| {
        if visibility == Some("workspace") {
            "public_to"
        } else {
            "private"
        }
    });
    if let Some(visibility) = visibility {
        if !matches!(visibility, "private" | "workspace") {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "visibility must be private or workspace",
            ));
        }
    }
    if !matches!(permission_mode, "private" | "public_to") {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "permission_mode must be private or public_to",
        ));
    }
    let mut resolved = Vec::new();
    if permission_mode == "public_to" {
        if permission_mode == "public_to" && visibility == Some("workspace") && targets.is_none() {
            resolved.push(("workspace".to_string(), workspace_id));
        } else {
            for target in targets.unwrap_or_default() {
                if !matches!(target.target_type.as_str(), "workspace" | "member" | "team") {
                    return Err(error_response(
                        StatusCode::BAD_REQUEST,
                        "invocation target type must be workspace, member, or team",
                    ));
                }
                let target_id = if target.target_type == "workspace" {
                    match target.target_id.as_deref().filter(|id| !id.is_empty()) {
                        Some(id) if Uuid::parse_str(id).ok() != Some(workspace_id) => {
                            return Err(error_response(
                                StatusCode::BAD_REQUEST,
                                "workspace invocation target must match the current workspace",
                            ));
                        }
                        _ => workspace_id,
                    }
                } else {
                    target
                        .target_id
                        .as_deref()
                        .and_then(|id| Uuid::parse_str(id).ok())
                        .ok_or_else(|| {
                            error_response(
                                StatusCode::BAD_REQUEST,
                                "member/team invocation target requires a valid target_id",
                            )
                        })?
                };
                if !resolved
                    .iter()
                    .any(|(kind, id)| kind == &target.target_type && *id == target_id)
                {
                    resolved.push((target.target_type.clone(), target_id));
                }
            }
        }
        if resolved.is_empty() {
            resolved.push(("workspace".to_string(), workspace_id));
        }
    }
    Ok((permission_mode.to_string(), resolved))
}

fn permission_echo_is_unchanged(
    existing: &Agent,
    request: &AgentWrite,
    existing_targets: &[AgentInvocationTarget],
    workspace_id: Uuid,
) -> bool {
    if let Some(mode) = request.permission_mode.as_deref() {
        if mode != existing.permission_mode {
            return false;
        }
    }
    if let Some(visibility) = request.visibility.as_deref() {
        if visibility != existing.visibility {
            return false;
        }
    }
    if let Some(targets) = request.invocation_targets.as_ref() {
        let Ok((_, mut resolved)) = resolve_permission(
            workspace_id,
            Some(existing.permission_mode.as_str()),
            Some(existing.visibility.as_str()),
            Some(targets),
        ) else {
            return false;
        };
        let mut current = existing_targets
            .iter()
            .map(|target| (target.target_type.clone(), target.target_id))
            .collect::<Vec<_>>();
        current.sort();
        resolved.sort();
        if current != resolved {
            return false;
        }
    }
    true
}

async fn replace_invocation_targets(
    executor: &mut sqlx::PgConnection,
    agent_id: Uuid,
    actor_id: Uuid,
    targets: &[(String, Uuid)],
) -> anyhow::Result<()> {
    agent_invocation_target::delete_agent_invocation_targets(&mut *executor, agent_id).await?;
    for (target_type, target_id) in targets {
        agent_invocation_target::create_agent_invocation_target(
            &mut *executor,
            agent_id,
            target_type,
            *target_id,
            actor_id,
        )
        .await?;
    }
    Ok(())
}

async fn create_agent(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request: AgentWrite = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let name = request.name.as_deref().unwrap_or_default().trim();
    if name.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "name is required");
    }
    if request
        .description
        .as_deref()
        .unwrap_or_default()
        .chars()
        .count()
        > 255
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "description must be 255 characters or fewer",
        );
    }
    let max_concurrent_tasks = request.max_concurrent_tasks.unwrap_or(6);
    if !(1..=50).contains(&max_concurrent_tasks) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "max_concurrent_tasks must be between 1 and 50",
        );
    }
    let runtime_id = match request
        .runtime_id
        .as_deref()
        .and_then(|v| Uuid::parse_str(v).ok())
    {
        Some(id) => id,
        None => return error_response(StatusCode::BAD_REQUEST, "runtime_id is required"),
    };
    let ws = match workspace_id(&context) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (permission_mode, invocation_targets) = match resolve_permission(
        ws,
        request.permission_mode.as_deref(),
        request.visibility.as_deref(),
        request.invocation_targets.as_deref(),
    ) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let legacy_visibility = if permission_mode == "public_to"
        && invocation_targets
            .iter()
            .any(|(target_type, _)| target_type == "workspace")
    {
        "workspace"
    } else {
        "private"
    };
    let rt = match runtime::get_agent_runtime_for_workspace(&state.pool, runtime_id, ws).await {
        Ok(Some(rt)) => rt,
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "runtime not found in this workspace",
            )
        }
    };
    if rt.owner_id.is_none()
        || rt.visibility != "public" && rt.owner_id != Some(context.member.user_id)
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "this runtime is private; only its owner can create agents on it",
        );
    }
    if request
        .model
        .as_deref()
        .is_some_and(|model| model_known_incompatible(&rt.provider, model))
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "model is not valid for this runtime provider",
        );
    }
    if request
        .thinking_level
        .as_deref()
        .is_some_and(|value| !known_thinking_value(&rt.provider, value))
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "thinking_level is not recognised for this runtime",
        );
    }
    if request
        .service_tier
        .as_deref()
        .is_some_and(|value| !known_service_tier(&rt.provider, value))
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "service_tier is not recognised for this runtime",
        );
    }
    let avatar_url = match request.avatar_url.as_deref() {
        Some(raw) => match crate::avatar::accept_url(&state, raw, None).await {
            Ok(value) => Some(value),
            Err(message) => return error_response(StatusCode::FORBIDDEN, message),
        },
        None => None,
    };
    let is_first_agent = sqlx::query_scalar::<_, bool>(
        "SELECT NOT EXISTS (SELECT 1 FROM agent WHERE workspace_id=$1)",
    )
    .bind(ws)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(false);
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to start agent create transaction",
            )
        }
    };
    let skills = match parse_skill_ids(&request.skill_ids) {
        Ok(v) => v,
        Err(r) => return r,
    };
    for id in &skills {
        if !matches!(
            skill::get_skill_in_workspace(&mut *tx, *id, ws).await,
            Ok(Some(_))
        ) {
            return error_response(StatusCode::NOT_FOUND, "skill not found");
        }
    }
    let runtime_config = request.runtime_config.clone().unwrap_or_else(|| json!({}));
    let custom_env = json!(request.custom_env.clone().unwrap_or_default());
    let custom_args = json!(request.custom_args.clone().unwrap_or_default());
    let mcp_config = request.mcp_config.clone().unwrap_or_else(|| json!({}));
    let composio_toolkit_allowlist = normalise_allowlist(
        request
            .composio_toolkit_allowlist
            .clone()
            .unwrap_or_default(),
    );
    let created = agent::create_agent(
        &mut *tx,
        ws,
        name,
        request.description.as_deref().unwrap_or_default().trim(),
        avatar_url.as_deref(),
        &rt.runtime_mode,
        &runtime_config,
        runtime_id,
        legacy_visibility,
        max_concurrent_tasks,
        context.member.user_id,
        request.instructions.as_deref().unwrap_or_default().trim(),
        &custom_env,
        &custom_args,
        &mcp_config,
        request.model.as_deref(),
        request.thinking_level.as_deref(),
        request.service_tier.as_deref(),
        &composio_toolkit_allowlist,
        Some(&permission_mode),
    )
    .await;
    let created = match created {
        Ok(Some(v)) => v,
        Err(error) if crate::workspace::unique_violation(&error) => {
            return error_response(
                StatusCode::CONFLICT,
                &format!("an agent named {name:?} already exists in this workspace"),
            )
        }
        _ => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to create agent"),
    };
    for id in skills {
        if skill::add_agent_skill(&mut *tx, created.id, id)
            .await
            .is_err()
        {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to add agent skill",
            );
        }
    }
    if replace_invocation_targets(
        &mut tx,
        created.id,
        context.member.user_id,
        &invocation_targets,
    )
    .await
    .is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to set agent invocation targets",
        );
    }
    if tx.commit().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to commit agent create",
        );
    }
    if rt.status == "online" {
        state.tasks.reconcile_agent_status(created.id).await;
    }
    let created = match agent::get_agent(&state.pool, created.id).await {
        Ok(Some(created)) => created,
        _ => created,
    };
    publish(&state, "agent:created", &created, context.member.user_id);
    let response = match hydrated_agent_response(&state, &context, &headers, created.clone()).await
    {
        Ok(response) => response,
        Err(response) => return response,
    };
    let analytics = cordy_analytics::events::agent_created(
        &context.member.user_id.to_string(),
        &ws.to_string(),
        &created.id.to_string(),
        &rt.provider,
        &rt.runtime_mode,
        &request.template,
        is_first_agent,
    );
    cordy_metrics::business_events::record_event(
        Some(state.analytics.as_ref()),
        state.business_metrics.as_deref(),
        &analytics,
    );
    (StatusCode::CREATED, Json(response)).into_response()
}

async fn update_agent(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let existing = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = manage_or_forbidden(&context, &existing) {
        return r;
    }
    let raw_request: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let request: AgentWrite = match serde_json::from_value(raw_request.clone()) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if request
        .description
        .as_deref()
        .is_some_and(|description| description.chars().count() > 255)
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "description must be 255 characters or fewer",
        );
    }
    if request
        .max_concurrent_tasks
        .is_some_and(|value| !(1..=50).contains(&value))
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "max_concurrent_tasks must be between 1 and 50",
        );
    }
    if raw_request.get("custom_env").is_some() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "custom_env is no longer accepted on this endpoint; use PUT /api/agents/{id}/env",
        );
    }
    let ws = match workspace_id(&context) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to start agent update transaction",
            )
        }
    };
    let existing = match agent::get_agent_for_update(&mut *tx, existing.id).await {
        Ok(Some(existing)) if existing.workspace_id == ws => existing,
        Ok(Some(_)) | Ok(None) => return error_response(StatusCode::NOT_FOUND, "agent not found"),
        Err(error) => {
            tracing::warn!(%error, "failed to lock agent for update");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load agent");
        }
    };
    if let Err(response) = manage_or_forbidden(&context, &existing) {
        return response;
    }
    let requested_runtime_id = match request.runtime_id.as_deref() {
        Some(value) => match Uuid::parse_str(value) {
            Ok(value) => Some(value),
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"),
        },
        None => existing.runtime_id,
    };
    let target_runtime = match requested_runtime_id {
        Some(runtime_id) => {
            match runtime::get_agent_runtime_for_workspace(&mut *tx, runtime_id, ws).await {
                Ok(Some(runtime)) => Some(runtime),
                _ => return error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"),
            }
        }
        None => None,
    };
    if request.runtime_id.is_some() {
        let Some(target_runtime) = target_runtime.as_ref() else {
            return error_response(StatusCode::BAD_REQUEST, "invalid runtime_id");
        };
        if target_runtime.owner_id.is_none()
            || target_runtime.visibility != "public"
                && target_runtime.owner_id != Some(context.member.user_id)
        {
            return error_response(
                StatusCode::FORBIDDEN,
                "this runtime is private; only its owner can move agents onto it",
            );
        }
    }
    let mut permission_touched = request.permission_mode.is_some()
        || request.visibility.is_some()
        || request.invocation_targets.is_some();
    if permission_touched && existing.owner_id != Some(context.member.user_id) {
        let existing_targets =
            match agent_invocation_target::list_agent_invocation_targets(&mut *tx, existing.id)
                .await
            {
                Ok(targets) => targets,
                Err(_) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to load agent invocation targets",
                    )
                }
            };
        if !permission_echo_is_unchanged(&existing, &request, &existing_targets, ws) {
            return error_response(
                StatusCode::FORBIDDEN,
                "only the agent owner can change access (permission_mode / invocation_targets)",
            );
        }
        permission_touched = false;
    }
    let effective_targets = if request.invocation_targets.is_some() {
        request.invocation_targets.clone()
    } else if permission_touched {
        match agent_invocation_target::list_agent_invocation_targets(&mut *tx, existing.id).await {
            Ok(targets) => Some(
                targets
                    .into_iter()
                    .map(|target| InvocationTargetInput {
                        target_type: target.target_type,
                        target_id: Some(target.target_id.to_string()),
                    })
                    .collect(),
            ),
            Err(_) => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to load agent invocation targets",
                )
            }
        }
    } else {
        None
    };
    let (resolved_permission, resolved_targets) = if permission_touched {
        match resolve_permission(
            ws,
            request.permission_mode.as_deref().or_else(|| {
                request
                    .visibility
                    .is_none()
                    .then_some(existing.permission_mode.as_str())
            }),
            request.visibility.as_deref(),
            effective_targets.as_deref(),
        ) {
            Ok(value) => (Some(value.0), Some(value.1)),
            Err(response) => return response,
        }
    } else {
        (None, None)
    };
    let resolved_visibility = resolved_targets.as_ref().map(|targets| {
        if resolved_permission.as_deref() == Some("public_to")
            && targets
                .iter()
                .any(|(target_type, _)| target_type == "workspace")
        {
            "workspace"
        } else {
            "private"
        }
    });
    if let Some(target_runtime) = target_runtime.as_ref() {
        if request
            .model
            .as_deref()
            .is_some_and(|model| model_known_incompatible(&target_runtime.provider, model))
        {
            return error_response(
                StatusCode::BAD_REQUEST,
                "model is not valid for this runtime provider",
            );
        }
        if request
            .thinking_level
            .as_deref()
            .is_some_and(|value| !known_thinking_value(&target_runtime.provider, value))
        {
            return error_response(
                StatusCode::BAD_REQUEST,
                "thinking_level is not recognised for this runtime",
            );
        }
        if request
            .service_tier
            .as_deref()
            .is_some_and(|value| !known_service_tier(&target_runtime.provider, value))
        {
            return error_response(
                StatusCode::BAD_REQUEST,
                "service_tier is not recognised for this runtime",
            );
        }
        if request.runtime_id.is_some() {
            if request.thinking_level.is_none()
                && existing
                    .thinking_level
                    .as_deref()
                    .is_some_and(|value| !known_thinking_value(&target_runtime.provider, value))
            {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "existing thinking_level is not valid for the new runtime; clear or replace it",
                );
            }
            if request.service_tier.is_none()
                && existing
                    .service_tier
                    .as_deref()
                    .is_some_and(|value| !known_service_tier(&target_runtime.provider, value))
            {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    "existing service_tier is not valid for the new runtime; clear or replace it",
                );
            }
        }
    } else if request.model.is_some()
        || request.thinking_level.is_some()
        || request.service_tier.is_some()
        || request.runtime_id.is_some()
    {
        return error_response(StatusCode::CONFLICT, "agent has no runtime");
    }
    let avatar_url = match request.avatar_url.as_deref() {
        Some(raw) => {
            match crate::avatar::accept_url(&state, raw, existing.avatar_url.as_deref()).await {
                Ok(value) => Some(value),
                Err(message) => return error_response(StatusCode::FORBIDDEN, message),
            }
        }
        None => None,
    };
    let custom_env = existing.custom_env.clone();
    let custom_args = request
        .custom_args
        .as_ref()
        .map(|value| json!(value))
        .unwrap_or_else(|| existing.custom_args.clone());
    let clear_mcp_config = raw_request.get("mcp_config").is_some_and(Value::is_null);
    let mcp_config = request
        .mcp_config
        .clone()
        .or_else(|| existing.mcp_config.clone())
        .unwrap_or_else(|| json!({}));
    let allowlist_touched = raw_request.get("composio_toolkit_allowlist").is_some();
    let is_owner = existing.owner_id == Some(context.member.user_id);
    let clear_allowlist =
        allowlist_touched && is_owner && raw_request["composio_toolkit_allowlist"].is_null();
    let composio_toolkit_allowlist = if allowlist_touched && is_owner {
        request
            .composio_toolkit_allowlist
            .clone()
            .map(normalise_allowlist)
            .unwrap_or_default()
    } else {
        existing
            .composio_toolkit_allowlist
            .clone()
            .unwrap_or_default()
    };
    let mut runtime_config = request
        .runtime_config
        .clone()
        .unwrap_or_else(|| existing.runtime_config.clone());
    preserve_masked_gateway_token(&mut runtime_config, &existing.runtime_config);
    let model = if request.model.is_some() {
        request.model.as_deref()
    } else if request.runtime_id.is_some()
        && target_runtime.as_ref().is_some_and(|target_runtime| {
            existing
                .model
                .as_deref()
                .is_some_and(|model| model_known_incompatible(&target_runtime.provider, model))
        })
    {
        Some("")
    } else {
        None
    };
    let updated = agent::update_agent(
        &mut *tx,
        existing.id,
        request.name.as_deref().map(str::trim),
        request.description.as_deref().map(str::trim),
        avatar_url.as_deref(),
        &runtime_config,
        request.runtime_id.as_ref().and_then(|_| {
            target_runtime
                .as_ref()
                .map(|target_runtime| target_runtime.runtime_mode.as_str())
        }),
        target_runtime
            .as_ref()
            .map(|target_runtime| target_runtime.id),
        resolved_visibility.or(request.visibility.as_deref()),
        resolved_permission.as_deref(),
        request.status.as_deref(),
        request.max_concurrent_tasks,
        request.instructions.as_deref().map(str::trim),
        &custom_env,
        &custom_args,
        &mcp_config,
        model,
        request
            .thinking_level
            .as_deref()
            .filter(|value| !value.is_empty()),
        request
            .service_tier
            .as_deref()
            .filter(|value| !value.is_empty()),
        &composio_toolkit_allowlist,
    )
    .await;
    let mut updated = match updated {
        Ok(Some(v)) => v,
        _ => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update agent"),
    };
    if let Some(targets) = resolved_targets.as_deref() {
        if replace_invocation_targets(&mut tx, updated.id, context.member.user_id, targets)
            .await
            .is_err()
        {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update agent invocation targets",
            );
        }
    }
    if clear_mcp_config {
        updated = match agent::clear_agent_mcp_config(&mut *tx, updated.id).await {
            Ok(Some(updated)) => updated,
            _ => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to clear agent mcp_config",
                )
            }
        };
    }
    if clear_allowlist {
        updated = match agent::clear_agent_composio_toolkit_allowlist(&mut *tx, updated.id).await {
            Ok(Some(updated)) => updated,
            _ => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to clear composio_toolkit_allowlist",
                )
            }
        };
    }
    if request.thinking_level.as_deref() == Some("") {
        updated = match agent::clear_agent_thinking_level(&mut *tx, updated.id).await {
            Ok(Some(updated)) => updated,
            _ => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to clear thinking_level",
                )
            }
        };
    }
    if request.service_tier.as_deref() == Some("") {
        updated = match agent::clear_agent_service_tier(&mut *tx, updated.id).await {
            Ok(Some(updated)) => updated,
            _ => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to clear service_tier",
                )
            }
        };
    }
    if tx.commit().await.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to commit agent update",
        );
    }
    publish(&state, "agent:status", &updated, context.member.user_id);
    match hydrated_agent_response(&state, &context, &headers, updated).await {
        Ok(response) => Json(response).into_response(),
        Err(response) => response,
    }
}

#[derive(Default, Deserialize)]
struct MikaRequest {
    runtime_id: String,
    language: String,
    model: Option<String>,
    #[serde(default)]
    session_title: String,
}

async fn get_or_create_mika_session(
    state: &HandlerState,
    workspace_id: Uuid,
    user_id: Uuid,
    agent_id: Uuid,
    title: &str,
) -> anyhow::Result<cordy_db::models::ChatSession> {
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("mika-session:{workspace_id}:{user_id}"))
        .execute(&mut *tx)
        .await?;
    workspace::lock_workspace_for_chat_session_create(&mut *tx, workspace_id).await?;
    if let Some(existing) = chat::get_oldest_active_chat_session_for_creator_agent(
        &mut *tx,
        workspace_id,
        user_id,
        agent_id,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(existing);
    }
    let created = chat::create_chat_session(
        &mut *tx,
        workspace_id,
        agent_id,
        user_id,
        title.trim(),
        false,
        Uuid::nil(),
        Uuid::now_v7(),
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("chat session insert returned no row"))?;
    tx.commit().await?;
    Ok(created)
}

async fn create_mika(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request: MikaRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let description = match request.language.as_str() {
        "en" => "Your workspace Chief of Staff. Mika turns goals into issues, coordinates agents, and helps build reusable workflows.",
        "zh" => "你的工作区 Chief of Staff。Mika 会把目标转化为任务、协调智能体，并帮你建立可复用的工作流。",
        "ko" => "워크스페이스의 Chief of Staff입니다. Mika가 목표를 태스크로 구체화하고 에이전트를 조율하며 재사용 가능한 워크플로 구성을 돕습니다.",
        "ja" => "ワークスペースの Chief of Staff。Mika は目標をタスクに落とし込み、エージェントを調整し、再利用できるワークフローづくりを支援します。",
        _ => return error_response(StatusCode::BAD_REQUEST, "language must be en, zh, ko, or ja"),
    };
    let ws = match workspace_id(&context) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let runtime_id = match Uuid::parse_str(&request.runtime_id) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"),
    };
    let rt = match runtime::get_agent_runtime_for_workspace(&state.pool, runtime_id, ws).await {
        Ok(Some(v)) => v,
        _ => {
            return error_response(
                StatusCode::BAD_REQUEST,
                "runtime not found in this workspace",
            )
        }
    };
    if rt.owner_id.is_none()
        || rt.visibility != "public" && rt.owner_id != Some(context.member.user_id)
    {
        return error_response(
            StatusCode::FORBIDDEN,
            "you cannot bind an agent to this runtime",
        );
    }
    let mut created_now = false;
    let mut target = match agent::get_agent_by_system_key(&state.pool, ws, Some("mika")).await {
        Ok(Some(existing)) => existing,
        Ok(None) => {
            let mut tx = match state.pool.begin().await {
                Ok(tx) => tx,
                Err(_) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to start agent create transaction",
                    )
                }
            };
            if sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                .bind(format!("mika:{ws}"))
                .execute(&mut *tx)
                .await
                .is_err()
            {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to lock the workspace agent",
                );
            }
            let target = match agent::get_agent_by_system_key(&mut *tx, ws, Some("mika")).await {
                Ok(Some(existing)) => existing,
                Ok(None) => {
                    let created = match agent::create_system_user_agent(
                        &mut *tx,
                        ws,
                        "Mika",
                        description,
                        Some("emoji:🦄"),
                        &rt.runtime_mode,
                        runtime_id,
                        request
                            .model
                            .as_deref()
                            .filter(|model| !model.trim().is_empty()),
                        "workspace",
                        "public_to",
                        3,
                        context.member.user_id,
                        Some("mika"),
                    )
                    .await
                    {
                        Ok(Some(created)) => created,
                        _ => {
                            return error_response(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                "failed to create the workspace agent",
                            )
                        }
                    };
                    if replace_invocation_targets(
                        &mut tx,
                        created.id,
                        context.member.user_id,
                        &[("workspace".to_string(), ws)],
                    )
                    .await
                    .is_err()
                    {
                        return error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "failed to save agent access",
                        );
                    }
                    created_now = true;
                    created
                }
                Err(_) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to look up the workspace agent",
                    )
                }
            };
            if tx.commit().await.is_err() {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to commit agent create",
                );
            }
            target
        }
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to look up the workspace agent",
            )
        }
    };
    if created_now && rt.status == "online" {
        state.tasks.reconcile_agent_status(target.id).await;
        if let Ok(Some(reconciled)) = agent::get_agent(&state.pool, target.id).await {
            target = reconciled;
        }
    }
    let session = match get_or_create_mika_session(
        &state,
        ws,
        context.member.user_id,
        target.id,
        &request.session_title,
    )
    .await
    {
        Ok(session) => session,
        Err(error) => {
            tracing::warn!(%error, agent_id = %target.id, "failed to open Mika conversation");
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to open the Mika conversation",
            );
        }
    };
    if created_now {
        publish(&state, "agent:created", &target, context.member.user_id);
    }
    let mut response = match hydrated_agent_response(&state, &context, &headers, target).await {
        Ok(response) => response,
        Err(response) => return response,
    };
    response["onboarding_session"] = json!(session);
    (
        if created_now {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(response),
    )
        .into_response()
}

async fn archive_agent(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = manage_or_forbidden(&context, &target) {
        return r;
    }
    if target.archived_at.is_some() {
        return error_response(StatusCode::CONFLICT, "agent is already archived");
    }
    if target.system_key.as_deref().is_some_and(|v| !v.is_empty()) {
        return error_response(
            StatusCode::BAD_REQUEST,
            "this agent is built into Cordy and cannot be archived",
        );
    }
    let archived = match agent::archive_agent(&state.pool, target.id, context.member.user_id).await
    {
        Ok(Some(v)) => v,
        _ => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to archive agent"),
    };
    if state.tasks.cancel_tasks_for_agent(target.id).await.is_err() {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to cancel tasks");
    }
    publish(&state, "agent:archived", &archived, context.member.user_id);
    match hydrated_agent_response(&state, &context, &headers, archived).await {
        Ok(response) => Json(response).into_response(),
        Err(response) => response,
    }
}

async fn restore_agent(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = manage_or_forbidden(&context, &target) {
        return r;
    }
    if target.archived_at.is_none() {
        return error_response(StatusCode::CONFLICT, "agent is not archived");
    }
    let restored = match agent::restore_agent(&state.pool, target.id).await {
        Ok(Some(v)) => v,
        _ => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to restore agent"),
    };
    publish(&state, "agent:restored", &restored, context.member.user_id);
    match hydrated_agent_response(&state, &context, &headers, restored).await {
        Ok(response) => Json(response).into_response(),
        Err(response) => response,
    }
}

async fn cancel_tasks(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = manage_or_forbidden(&context, &target) {
        return r;
    }
    match state.tasks.cancel_tasks_for_agent(target.id).await {
        Ok(rows) => Json(json!({"cancelled": rows.len()})).into_response(),
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to cancel tasks"),
    }
}

async fn list_tasks(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (actor_type, _, _) = crate::issue::mutation_actor(&state, &context, &headers).await;
    let targets = match agent_invocation_target::list_agent_invocation_targets(
        &state.pool,
        target.id,
    )
    .await
    {
        Ok(targets) => targets,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load agent invocation targets",
            )
        }
    };
    if actor_type != "agent" && !member_can_view(&context, &target, &targets) {
        return error_response(
            StatusCode::FORBIDDEN,
            "you do not have access to this agent",
        );
    }
    match agent::list_agent_tasks(&state.pool, target.id).await {
        Ok(rows) => Json(crate::issue::task_maps(&state, &rows, &context.workspace_id).await)
            .into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list agent tasks",
        ),
    }
}

async fn get_env(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (actor_type, _, _) = crate::issue::mutation_actor(&state, &context, &headers).await;
    if actor_type == "agent" {
        return error_response(
            StatusCode::FORBIDDEN,
            "agents may not access env management endpoints",
        );
    }
    if let Err(r) = manage_or_forbidden(&context, &target) {
        return r;
    }
    let env = env_map(&target);
    if audit_env(
        &state,
        &target,
        context.member.user_id,
        "agent_env_revealed",
        json!({
            "agent_id": target.id,
            "agent_name": target.name,
            "revealed_keys": env.keys().collect::<Vec<_>>(),
            "key_count": env.len()
        }),
    )
    .await
    .is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "audit log write failed; refusing to serve env without a recorded reveal",
        );
    }
    Json(json!({"agent_id": target.id, "custom_env": env})).into_response()
}

#[derive(Default, Deserialize)]
struct EnvRequest {
    custom_env: Option<BTreeMap<String, String>>,
}
async fn update_env(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (actor_type, _, _) = crate::issue::mutation_actor(&state, &context, &headers).await;
    if actor_type == "agent" {
        return error_response(
            StatusCode::FORBIDDEN,
            "agents may not access env management endpoints",
        );
    }
    if let Err(r) = manage_or_forbidden(&context, &target) {
        return r;
    }
    let request: EnvRequest = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    let existing = env_map(&target);
    let (incoming, audit) = merge_agent_env(&existing, request.custom_env.unwrap_or_default());
    let mut tx = match state.pool.begin().await {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update env"),
    };
    let updated = match agent::update_agent_custom_env(&mut *tx, target.id, &json!(incoming)).await
    {
        Ok(Some(v)) => v,
        _ => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update env"),
    };
    if audit_env_on(
        &mut *tx,
        &target,
        context.member.user_id,
        "agent_env_updated",
        json!({
            "agent_id": target.id,
            "agent_name": target.name,
            "added_keys": audit.added,
            "removed_keys": audit.removed,
            "changed_keys": audit.changed,
            "preserved_keys": audit.preserved,
        }),
    )
    .await
    .is_err()
        || tx.commit().await.is_err()
    {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to update env");
    }
    publish(&state, "agent:status", &updated, context.member.user_id);
    Json(json!({"agent_id": target.id, "custom_env": env_map(&updated)})).into_response()
}

async fn list_labels(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    labels_response(&state, &target).await
}

#[derive(Deserialize)]
struct LabelRequest {
    label_id: String,
}
async fn attach_label(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    Json(request): Json<LabelRequest>,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = manage_or_forbidden(&context, &target) {
        return r;
    }
    let label_id = match Uuid::parse_str(&request.label_id) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid label_id"),
    };
    match issue_label::attach_label_to_agent(&state.pool, target.id, label_id, target.workspace_id)
        .await
    {
        Ok(0) => error_response(StatusCode::NOT_FOUND, "label not found"),
        Ok(_) => {
            publish_label_updated(
                &state,
                target.workspace_id,
                context.member.user_id,
                json!({"label_id": label_id, "resource_type":"agent"}),
            );
            labels_response(&state, &target).await
        }
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to attach label"),
    }
}
async fn detach_label(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((id, label_id)): Path<(String, String)>,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = manage_or_forbidden(&context, &target) {
        return r;
    }
    let label_id = match Uuid::parse_str(&label_id) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid label id"),
    };
    match issue_label::detach_label_from_agent(
        &state.pool,
        target.id,
        label_id,
        target.workspace_id,
    )
    .await
    {
        Ok(_) => {
            publish_label_updated(
                &state,
                target.workspace_id,
                context.member.user_id,
                json!({"label_id": label_id, "resource_type":"agent"}),
            );
            labels_response(&state, &target).await
        }
        Err(_) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to detach label"),
    }
}

async fn labels_response(state: &HandlerState, target: &Agent) -> Response {
    match issue_label::list_labels_by_agent(&state.pool, target.id, target.workspace_id).await {
        Ok(labels) => Json(json!({"labels": labels})).into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list agent labels",
        ),
    }
}

fn publish_label_updated(state: &HandlerState, workspace_id: Uuid, actor_id: Uuid, payload: Value) {
    state.bus.publish(&cordy_events::Event {
        event_type: "label:updated".into(),
        workspace_id: workspace_id.to_string(),
        actor_type: "member".into(),
        actor_id: actor_id.to_string(),
        payload,
        ..Default::default()
    });
}

async fn list_skills(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    skills_response(&state, target.id).await
}
#[derive(Default, Deserialize)]
struct SkillsRequest {
    #[serde(default)]
    skill_ids: Vec<String>,
}
async fn set_skills(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    Json(req): Json<SkillsRequest>,
) -> Response {
    mutate_skills(state, context, id, req.skill_ids, true).await
}
async fn add_skills(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    Json(req): Json<SkillsRequest>,
) -> Response {
    mutate_skills(state, context, id, req.skill_ids, false).await
}
async fn mutate_skills(
    state: HandlerState,
    context: WorkspaceContext,
    id: String,
    ids: Vec<String>,
    replace: bool,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = manage_or_forbidden(&context, &target) {
        return r;
    }
    let ids = match parse_skill_ids(&ids) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut tx = match state.pool.begin().await {
        Ok(v) => v,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to start transaction",
            )
        }
    };
    for id in &ids {
        if !matches!(
            skill::get_skill_in_workspace(&mut *tx, *id, target.workspace_id).await,
            Ok(Some(_))
        ) {
            return error_response(StatusCode::NOT_FOUND, "skill not found");
        }
    }
    if replace
        && skill::remove_all_agent_skills(&mut *tx, target.id)
            .await
            .is_err()
    {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to clear agent skills",
        );
    }
    for id in ids {
        if skill::add_agent_skill(&mut *tx, target.id, id)
            .await
            .is_err()
        {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to add agent skill",
            );
        }
    }
    if tx.commit().await.is_err() {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to commit");
    }
    updated_skills_response(&state, &target, context.member.user_id).await
}
async fn set_skill_enabled(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((id, skill_id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = manage_or_forbidden(&context, &target) {
        return r;
    }
    let skill_id = match Uuid::parse_str(&skill_id) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid skill_id"),
    };
    #[derive(Deserialize)]
    struct R {
        enabled: Option<bool>,
    }
    let req: R = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "enabled is required"),
    };
    let Some(enabled) = req.enabled else {
        return error_response(StatusCode::BAD_REQUEST, "enabled is required");
    };
    match skill::set_agent_skill_enabled(&state.pool, target.id, skill_id, enabled).await {
        Ok(0) => error_response(StatusCode::NOT_FOUND, "agent skill not found"),
        Ok(_) => updated_skills_response(&state, &target, context.member.user_id).await,
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to update agent skill",
        ),
    }
}
async fn remove_skill(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path((id, skill_id)): Path<(String, String)>,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = manage_or_forbidden(&context, &target) {
        return r;
    }
    let skill_id = match Uuid::parse_str(&skill_id) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid skill_id"),
    };
    match skill::remove_agent_skill(&state.pool, target.id, skill_id).await {
        Ok(_) => updated_skills_response(&state, &target, context.member.user_id).await,
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to remove agent skill",
        ),
    }
}

#[derive(Deserialize)]
struct RuntimeSkillRequest {
    runtime_id: String,
    root: String,
    key: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    plugin: String,
    enabled: Option<bool>,
}
async fn set_runtime_skill_enabled(
    State(state): State<HandlerState>,
    Extension(context): Extension<WorkspaceContext>,
    Path(id): Path<String>,
    Json(req): Json<RuntimeSkillRequest>,
) -> Response {
    let target = match load_agent(&state, &context, &id).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = manage_or_forbidden(&context, &target) {
        return r;
    }
    let runtime_id = match Uuid::parse_str(&req.runtime_id) {
        Ok(v) => v,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "invalid runtime_id"),
    };
    let Some(enabled) = req.enabled else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "runtime_id, root, key, and enabled are required",
        );
    };
    if target.runtime_id != Some(runtime_id) {
        return error_response(
            StatusCode::CONFLICT,
            "agent is no longer assigned to this runtime",
        );
    }
    let root = req.root.trim();
    let key = req.key.trim().replace('\\', "/");
    let mut plugin = req.plugin.trim().to_string();
    if key.is_empty()
        || key.len() > 512
        || key.starts_with('/')
        || key.split('/').any(|p| p == "..")
        || req.name.trim().len() > 512
    {
        return error_response(StatusCode::BAD_REQUEST, "invalid runtime skill identity");
    }
    if !matches!(root, "provider" | "universal" | "plugin") || root == "plugin" && plugin.is_empty()
    {
        return error_response(StatusCode::BAD_REQUEST, "invalid runtime skill identity");
    }
    if root != "plugin" {
        plugin.clear();
    }
    let rt = match runtime::get_agent_runtime(&state.pool, runtime_id).await {
        Ok(Some(runtime)) if runtime.workspace_id == target.workspace_id => runtime,
        _ => return error_response(StatusCode::NOT_FOUND, "runtime not found"),
    };
    if rt.runtime_mode != "local" || !matches!(rt.provider.as_str(), "codex" | "claude") {
        return error_response(
            StatusCode::BAD_REQUEST,
            "runtime skill controls are only supported for codex and claude",
        );
    }
    if root == "plugin" && rt.provider != "claude" {
        return error_response(StatusCode::BAD_REQUEST, "invalid runtime skill identity");
    }
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(_) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to begin transaction",
            )
        }
    };
    let locked = match agent::get_agent_for_update(&mut *tx, target.id).await {
        Ok(Some(agent)) => agent,
        _ => return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to load agent"),
    };
    if locked.runtime_id != Some(runtime_id) {
        return error_response(
            StatusCode::CONFLICT,
            "agent is no longer assigned to this runtime",
        );
    }
    let mut values = locked
        .disabled_runtime_skills
        .as_array()
        .cloned()
        .unwrap_or_default();
    let identity = |v: &Value| {
        v["runtime_id"] == req.runtime_id
            && v["provider"] == rt.provider
            && v["root"] == root
            && v["key"] == key
            && v["plugin"] == plugin
    };
    if enabled {
        values.retain(|v| !identity(v));
    } else if !values.iter().any(identity) {
        values.push(json!({"runtime_id":req.runtime_id,"provider":rt.provider,"root":root,"key":key,"name":req.name.trim(),"plugin":plugin}));
    }
    let updated = match agent::update_agent_disabled_runtime_skills(
        &mut *tx,
        target.id,
        &Value::Array(values),
    )
    .await
    {
        Ok(Some(agent)) => agent,
        _ => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to update runtime skill",
            )
        }
    };
    if tx.commit().await.is_err() {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "failed to commit");
    }
    publish(&state, "agent:status", &updated, context.member.user_id);
    StatusCode::NO_CONTENT.into_response()
}

fn parse_skill_ids(values: &[String]) -> Result<Vec<Uuid>, Response> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for raw in values {
        let id = Uuid::parse_str(raw)
            .map_err(|_| error_response(StatusCode::BAD_REQUEST, "invalid skill_ids"))?;
        if seen.insert(id) {
            out.push(id);
        }
    }
    Ok(out)
}
async fn skills_response(state: &HandlerState, agent_id: Uuid) -> Response {
    match skill::list_agent_skill_summaries(&state.pool, agent_id).await {
        Ok(v) => Json(v).into_response(),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list agent skills",
        ),
    }
}
async fn updated_skills_response(state: &HandlerState, target: &Agent, actor_id: Uuid) -> Response {
    match skill::list_agent_skill_summaries(&state.pool, target.id).await {
        Ok(skills) => {
            state.bus.publish(&cordy_events::Event {
                event_type: "agent:status".into(),
                workspace_id: target.workspace_id.to_string(),
                actor_type: "member".into(),
                actor_id: actor_id.to_string(),
                payload: json!({"agent_id":target.id,"skills":skills}),
                ..Default::default()
            });
            Json(skills).into_response()
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to list agent skills",
        ),
    }
}
fn publish(state: &HandlerState, event_type: &str, target: &Agent, actor_id: Uuid) {
    state.bus.publish(&cordy_events::Event {
        event_type: event_type.into(),
        workspace_id: target.workspace_id.to_string(),
        actor_type: "member".into(),
        actor_id: actor_id.to_string(),
        payload: json!({"agent":agent_response(Some(state),target.clone(),false,false)}),
        ..Default::default()
    });
}
async fn audit_env(
    state: &HandlerState,
    target: &Agent,
    actor_id: Uuid,
    action: &str,
    details: Value,
) -> anyhow::Result<()> {
    audit_env_on(&state.pool, target, actor_id, action, details).await
}
async fn audit_env_on<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    target: &Agent,
    actor_id: Uuid,
    action: &str,
    details: Value,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO activity_log (id,workspace_id,issue_id,actor_type,actor_id,action,details) VALUES ($1,$2,NULL,'member',$3,$4,$5)").bind(Uuid::now_v7()).bind(target.workspace_id).bind(actor_id).bind(action).bind(details).execute(executor).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_fixture() -> Agent {
        Agent {
            id: Uuid::now_v7(),
            workspace_id: Uuid::now_v7(),
            name: "Agent".into(),
            avatar_url: None,
            runtime_mode: "local".into(),
            runtime_config: json!({"gateway":{"token":"gateway-secret"}}),
            visibility: "private".into(),
            status: "idle".into(),
            max_concurrent_tasks: 1,
            owner_id: Some(Uuid::now_v7()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            description: String::new(),
            runtime_id: None,
            instructions: String::new(),
            archived_at: None,
            archived_by: None,
            custom_env: json!({"TOKEN":"env-secret"}),
            custom_args: json!([]),
            mcp_config: Some(json!({"server":{"token":"mcp-secret"}})),
            model: None,
            thinking_level: None,
            composio_toolkit_allowlist: Some(vec!["github".into()]),
            permission_mode: "private".into(),
            kind: "user".into(),
            system_key: None,
            disabled_runtime_skills: json!([]),
            service_tier: None,
        }
    }

    #[test]
    fn gateway_tokens_are_masked() {
        assert_eq!(
            mask_gateway_token(json!({"gateway":{"token":"secret"}}))["gateway"]["token"],
            "***"
        );
    }

    #[test]
    fn agent_write_preserves_status_updates() {
        let request: AgentWrite = serde_json::from_value(json!({ "status": "offline" })).unwrap();
        assert_eq!(request.status.as_deref(), Some("offline"));
    }

    #[test]
    fn agent_actor_projection_redacts_every_secret_surface() {
        let response = agent_response(None, agent_fixture(), false, false);
        assert_eq!(response["runtime_config"]["gateway"]["token"], "***");
        assert_eq!(response["mcp_config"], json!({}));
        assert_eq!(response["mcp_config_redacted"], true);
        assert!(response["composio_toolkit_allowlist"].is_null());
        assert_eq!(response["composio_toolkit_allowlist_redacted"], true);
        assert!(response.get("custom_env").is_none());
        assert_eq!(response["custom_env_key_count"], 1);
        assert_eq!(response["runtime_id"], "");
    }

    #[test]
    fn agent_actor_projection_redacts_non_object_mcp_configuration() {
        for mcp_config in [json!("secret"), json!(["secret"])] {
            let mut agent = agent_fixture();
            agent.mcp_config = Some(mcp_config);
            let response = agent_response(None, agent, false, false);
            assert_eq!(response["mcp_config"], json!({}));
            assert_eq!(response["mcp_config_redacted"], true);
        }
    }
    #[test]
    fn skill_ids_are_validated_and_deduplicated() {
        let id = Uuid::new_v4();
        assert_eq!(
            parse_skill_ids(&[id.to_string(), id.to_string()]).unwrap(),
            vec![id]
        );
        assert!(parse_skill_ids(&["bad".into()]).is_err());
    }

    #[test]
    fn legacy_workspace_visibility_becomes_workspace_allowlist() {
        let workspace_id = Uuid::new_v4();
        let resolved = match resolve_permission(workspace_id, None, Some("workspace"), None) {
            Ok(resolved) => resolved,
            Err(_) => panic!("legacy workspace permission should be valid"),
        };
        assert_eq!(resolved.0, "public_to");
        assert_eq!(resolved.1, vec![("workspace".to_string(), workspace_id)]);
    }

    #[test]
    fn public_permission_without_targets_defaults_to_workspace() {
        let workspace_id = Uuid::new_v4();
        assert_eq!(
            resolve_permission(workspace_id, Some("public_to"), None, None).unwrap(),
            ("public_to".into(), vec![("workspace".into(), workspace_id)])
        );
    }

    #[test]
    fn cross_workspace_target_is_rejected() {
        let workspace_id = Uuid::new_v4();
        let targets = [InvocationTargetInput {
            target_type: "workspace".into(),
            target_id: Some(Uuid::new_v4().to_string()),
        }];
        assert!(resolve_permission(workspace_id, Some("public_to"), None, Some(&targets)).is_err());
    }

    #[test]
    fn mika_response_carries_product_instructions() {
        let instructions = system_instructions_for(Some("mika"), "Mika");
        assert!(instructions.contains("You are Mika"));
        assert!(system_instructions_for(None, "Ordinary agent").is_empty());
    }

    #[test]
    fn workspace_target_may_omit_target_id_and_team_is_supported() {
        let workspace_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();
        let targets = [
            InvocationTargetInput {
                target_type: "workspace".into(),
                target_id: None,
            },
            InvocationTargetInput {
                target_type: "team".into(),
                target_id: Some(team_id.to_string()),
            },
        ];
        assert_eq!(
            resolve_permission(workspace_id, Some("public_to"), None, Some(&targets))
                .unwrap()
                .1,
            vec![("workspace".into(), workspace_id), ("team".into(), team_id)]
        );
    }

    #[test]
    fn masked_gateway_token_round_trip_preserves_secret() {
        let mut incoming = json!({"gateway":{"token":"***"}});
        preserve_masked_gateway_token(&mut incoming, &json!({"gateway":{"token":"secret"}}));
        assert_eq!(incoming["gateway"]["token"], "secret");
    }

    #[test]
    fn orphaned_gateway_token_mask_is_dropped() {
        let mut incoming = json!({"gateway":{"token":"***","url":"wss://example"}});
        preserve_masked_gateway_token(&mut incoming, &json!({"gateway":{}}));
        assert!(incoming["gateway"].get("token").is_none());
        assert_eq!(incoming["gateway"]["url"], "wss://example");
    }

    #[test]
    fn invalid_legacy_visibility_is_rejected() {
        let workspace_id = Uuid::new_v4();
        assert!(resolve_permission(workspace_id, None, Some("workpace"), None).is_err());
        assert!(resolve_permission(workspace_id, None, Some("private"), None).is_ok());
    }

    #[test]
    fn env_merge_drops_unknown_sentinel_and_reports_key_diff_without_values() {
        let existing = BTreeMap::from([
            ("KEEP".into(), "old".into()),
            ("CHANGE".into(), "before".into()),
            ("REMOVE".into(), "gone".into()),
        ]);
        let requested = BTreeMap::from([
            ("KEEP".into(), ENV_SENTINEL.into()),
            ("CHANGE".into(), "after".into()),
            ("ADD".into(), "new".into()),
            ("UNKNOWN".into(), ENV_SENTINEL.into()),
        ]);
        let (merged, audit) = merge_agent_env(&existing, requested);
        assert_eq!(merged.get("KEEP").map(String::as_str), Some("old"));
        assert!(!merged.contains_key("UNKNOWN"));
        assert_eq!(audit.added, vec!["ADD"]);
        assert_eq!(audit.changed, vec!["CHANGE"]);
        assert_eq!(audit.preserved, vec!["KEEP"]);
        assert_eq!(audit.removed, vec!["REMOVE"]);
    }

    #[test]
    fn runtime_native_validation_rejects_foreign_values() {
        assert!(model_known_incompatible("codex", "claude-opus-5"));
        assert!(model_known_incompatible("claude", "gpt-5.6-sol"));
        assert!(model_known_incompatible("codex", "gpt-99"));
        assert!(!model_known_incompatible("claude", "claude-opus-5[1m]"));
        assert!(!model_known_incompatible("codex", "private-lab-model"));
        assert!(known_thinking_value("codex", "ultra"));
        assert!(!known_thinking_value("grok", "xhigh"));
        assert!(known_service_tier("codex", "priority"));
        assert!(!known_service_tier("claude", "priority"));
    }
}
