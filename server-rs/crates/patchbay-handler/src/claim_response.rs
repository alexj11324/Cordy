//! Daemon claim payload assembly and helpers
//! (`worktreeClaimBlockReason`, `rerunSourceMatchesTaskScope`,
//! `trailingUserMessages`, capability parsing).
//!
//! Wire contracts are preserved exactly: field names mirror the Go JSON tags;
//! absence vs null vs empty follows the Go omitempty semantics via conditional
//! insertion on top of [`crate::task_json::task_to_map`].

use axum::http::{header, HeaderMap, StatusCode};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use patchbay_db::models::{AgentRuntime, AgentTaskQueue};
use patchbay_db::queries::{
    agent as agent_q, agent as agent_queries, autopilot as autopilot_q, chat as chat_q,
    comment as comment_q, issue as issue_q, project as project_q, team as team_q, user as user_q,
    workspace as workspace_q,
};
use patchbay_protocol::{
    DAEMON_CAPABILITY_COALESCED_COMMENTS_V1, DAEMON_CAPABILITY_LOCAL_WORKTREE_V1,
    DAEMON_CAPABILITY_SKILL_BUNDLES_V1,
};

use crate::claim_comments::{
    build_coalesced_comment_data, comment_data_ids, format_legacy_comment_bundle,
    select_comment_delivery, CoalescedCommentData, MAX_CLAIM_COMMENT_PAYLOAD_BYTES,
};
use crate::daemon::DaemonClaimServices;
use crate::error::error_response;
use crate::team_briefing::build_team_leader_briefing;
use crate::timefmt::rfc3339;

/// Max local-skill import requests claimed in one heartbeat batch.
pub const MAX_LOCAL_SKILL_IMPORT_BATCH: usize = 10;

// ---------------------------------------------------------------------------
// Capability parsing (Go requestHasClientCapability)
// ---------------------------------------------------------------------------

pub fn request_has_client_capability(headers: &HeaderMap, capability: &str) -> bool {
    headers
        .get("X-Client-Capabilities")
        .and_then(|v| v.to_str().ok())
        .map(|raw| raw.split(',').any(|part| part.trim() == capability))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Failure carrier (Go claimBuildFailure)
// ---------------------------------------------------------------------------

pub struct ClaimBuildFailure {
    pub outcome: String,
    pub status: StatusCode,
    pub message: String,
}

impl ClaimBuildFailure {
    fn new(outcome: &'static str, status: StatusCode, message: &str) -> Self {
        Self {
            outcome: outcome.to_string(),
            status,
            message: message.to_string(),
        }
    }

    pub fn to_response(&self) -> axum::response::Response {
        error_response(self.status, &self.message)
    }
}

/// Successful build output: the wire payload plus the exact comment ids that
/// were embedded (the delivery receipt), and whether the task is comment-backed
/// (receipt must be recorded by FinalizeTaskClaim).
pub struct BuiltClaim {
    pub payload: Value,
    pub delivered_comment_ids: Vec<Uuid>,
    pub comment_backed: bool,
}

fn set_if_not_empty(m: &mut Map<String, Value>, key: &str, v: &str) {
    if !v.is_empty() {
        m.insert(key.to_string(), Value::String(v.to_string()));
    }
}

// ---------------------------------------------------------------------------
// Worktree gate (Go worktreeClaimBlockReason)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct LocalDirectoryRef {
    #[serde(default)]
    local_path: String,
    #[serde(default)]
    daemon_id: String,
    #[serde(default, rename = "execution_mode")]
    execution_mode: String,
}

const LOCAL_DIRECTORY_MODE_WORKTREE: &str = "worktree";

/// Returns a user-facing reason when this runtime must not run the task.
/// The decision keys off the CAPABILITY the daemon advertised on this very
/// request, not its version string (PB-5707). Only resources bound to the
/// claiming runtime's own daemon are considered.
fn worktree_claim_block_reason(
    resources: &[Value],
    runtime: &AgentRuntime,
    has_worktree_capability: bool,
) -> String {
    let Some(daemon_id) = runtime.daemon_id.as_deref().filter(|d| !d.is_empty()) else {
        return String::new();
    };
    if has_worktree_capability {
        return String::new();
    }
    for res in resources {
        if res.get("resource_type").and_then(|v| v.as_str()) != Some("local_directory") {
            continue;
        }
        let Ok(r#ref) = serde_json::from_value::<LocalDirectoryRef>(
            res.get("resource_ref").cloned().unwrap_or(Value::Null),
        ) else {
            continue;
        };
        if r#ref.execution_mode != LOCAL_DIRECTORY_MODE_WORKTREE || r#ref.daemon_id != daemon_id {
            continue;
        }
        return format!(
            "This machine's Patchbay runtime does not support parallel (worktree) mode, which {:?} is set to use. \
             Update the Patchbay app on that machine to the latest version, then re-run this task. \
             Refusing to run rather than falling back to editing the directory directly, which is what this mode exists to prevent.",
            r#ref.local_path
        );
    }
    String::new()
}

fn rerun_source_matches_task_scope(task: &AgentTaskQueue, source: &AgentTaskQueue) -> bool {
    if task.agent_id != source.agent_id {
        return false;
    }
    if let Some(issue_id) = task.issue_id {
        return source.issue_id == Some(issue_id);
    }
    if let Some(chat_session_id) = task.chat_session_id {
        return source.chat_session_id == Some(chat_session_id);
    }
    false
}

fn strip_unleased_execution_resume(payload: &mut Map<String, Value>) {
    // A path or provider session is an execution capability, not harmless
    // continuity metadata. Until Directory/session leases exist, a task claim
    // must start fresh even when a stored task, rerun, chat, or Message Bus
    // anchor contains an owner/caller worktree or provider session.
    for key in [
        "work_dir",
        "relative_work_dir",
        "durable_work_dir",
        "relative_durable_work_dir",
        "branch_name",
        "prior_work_dir",
        "prior_session_id",
    ] {
        payload.remove(key);
    }
}

/// Go trailingUserMessages: the run of user messages after the last assistant
/// message — the set the agent has NOT yet replied to. Legacy channel tasks stop
/// at the first unexpired media marker.
fn trailing_user_messages(
    msgs: Vec<patchbay_db::models::ChatMessage>,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<patchbay_db::models::ChatMessage> {
    let mut start = 0usize;
    for i in (0..msgs.len()).rev() {
        if msgs[i].role != "user" {
            start = i + 1;
            break;
        }
    }
    let msgs: Vec<_> = msgs.into_iter().skip(start).collect();
    for i in 0..msgs.len() {
        if let Some(pending) = msgs[i].channel_media_pending_until {
            if pending > now {
                return msgs[..i].to_vec();
            }
        }
    }
    msgs
}

fn disabled_runtime_skills_for(raw: &Value, runtime_id: &str, provider: &str) -> Vec<Value> {
    let empty = vec![];
    let all = raw.as_array().unwrap_or(&empty);
    all.iter()
        .filter(|skill| {
            skill.get("runtime_id").and_then(|v| v.as_str()) == Some(runtime_id)
                && skill.get("provider").and_then(|v| v.as_str()) == Some(provider)
        })
        .cloned()
        .collect()
}

/// Applies project title/description/resources to the payload and returns any
/// github_repo lifts (Go's inline project-resources block).
async fn apply_project_context(
    state: &DaemonClaimServices,
    payload: &mut Map<String, Value>,
    project_id: Uuid,
) -> Vec<Value> {
    if let Ok(Some(proj)) = project_q::get_project(&state.pool, project_id).await {
        payload.insert("project_title".into(), Value::String(proj.title.clone()));
        if let Some(desc) = &proj.description {
            payload.insert("project_description".into(), Value::String(desc.clone()));
        }
    }
    // Local directories and repository URLs are execution capabilities, not
    // project metadata. Phase 1 has no lease-bound Directory/Git credential
    // broker, so a task claim never receives them. This prevents a shared
    // Agent from turning its owner's daemon HOME, checkout, or credential
    // helper into ambient caller authority.
    Vec::new()
}

async fn workspace_repos_or(
    _state: &DaemonClaimServices,
    _workspace_id: Uuid,
    fallback: Value,
) -> Value {
    fallback
}

/// Builds the claim response for one already-claimed task.
///
/// A returned Err means the task must NOT be dispatched; the builder has already
/// cancelled it where the failure semantics require it.
#[allow(clippy::too_many_lines)]
pub(crate) async fn build_claimed_task_response(
    state: &DaemonClaimServices,
    headers: &HeaderMap,
    task: &AgentTaskQueue,
    runtime: &AgentRuntime,
    runtime_id: &str,
    runtime_workspace_id: &str,
) -> Result<BuiltClaim, ClaimBuildFailure> {
    // Base payload from the shared mapper (Go taskToResponse).
    let mut payload = crate::task_json::task_to_map(task, runtime_workspace_id);
    let obj = payload
        .as_object_mut()
        .expect("task_to_map returns an object");
    if let Some(context) = task.context.as_ref().and_then(Value::as_object) {
        for key in [
            "side_chat_parent_task_id",
            "side_chat_root_comment_id",
            "message_bus_parent_task_id",
        ] {
            if let Some(value) = context.get(key).and_then(Value::as_str) {
                set_if_not_empty(obj, key, value);
            }
        }
        if let Some(messages) = context
            .get("message_bus_messages")
            .filter(|value| value.as_array().is_some_and(|items| !items.is_empty()))
        {
            obj.insert("message_bus_messages".into(), messages.clone());
        }
    }

    // Claim-only capability: this server resolves the team-leader role on the
    // wire so the daemon must not re-derive it from the briefing text
    // (PB-5811).
    obj.insert("leader_role_resolved".into(), Value::Bool(true));

    let supports_coalesced_comments =
        request_has_client_capability(headers, DAEMON_CAPABILITY_COALESCED_COMMENTS_V1);
    let use_skill_refs = request_has_client_capability(headers, DAEMON_CAPABILITY_SKILL_BUNDLES_V1);

    // Empty-but-present receipt semantics are handled by the mapper's
    // delivered_comment_ids array.
    let mut delivered_comments: Vec<CoalescedCommentData> = Vec::new();

    // Phase 1 never projects workspace plugin tools, plugin MCP credentials,
    // or connected-app bindings into task claims. Those stores are
    // workspace/owner scoped and have no lease-bound credential broker yet;
    // advertising them here would turn agent.invoke into implicit tool and
    // credential authority.

    // Load the task agent with fresh data (name + skills + env/args/mcp).
    let agent = match agent_q::get_agent(&state.pool, task.agent_id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            // Durable orphan task cannot normally reach here; fail closed.
            tracing::error!(
                task_id = %task.id,
                agent_id = %task.agent_id,
                "daemon claim: task agent no longer exists; refusing dispatch"
            );
            return Err(fail_claimed_task_before_launch(
                state,
                task,
                "Task identity is invalid: the assigned agent no longer exists.",
                patchbay_task_failure::Reason::INVALID_TASK_IDENTITY,
                "error_invalid_task_identity",
                StatusCode::CONFLICT,
                "task agent no longer exists",
            )
            .await);
        }
        Err(e) => {
            tracing::error!(
                error = %e,
                task_id = %task.id,
                agent_id = %task.agent_id,
                "daemon claim: load task agent failed; requeueing claim"
            );
            let _ = state.tasks.requeue_task_after_claim_failure(task).await;
            return Err(ClaimBuildFailure::new(
                "error_load_task_agent",
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to load task agent",
            ));
        }
    };

    // Phase 1 intentionally ignores persisted per-task overlays, including
    // rows queued before this deployment. The legacy Composio overlay embeds
    // a server-wide long-lived API key and was computed before capability
    // lease issuance, so forwarding it would bypass credential.use and lease
    // revocation. A later broker may restore overlays only with a short-lived,
    // lease-bound credential.
    let _ = capability_scoped_task_overlay(task.runtime_mcp_overlay.as_ref());

    let disabled_skills = disabled_runtime_skills_for(
        &agent.disabled_runtime_skills,
        runtime_id,
        &runtime.provider,
    );

    let mut agent_obj = Map::new();
    agent_obj.insert("id".into(), Value::String(agent.id.to_string()));
    agent_obj.insert("name".into(), Value::String(agent.name.clone()));
    agent_obj.insert(
        "instructions".into(),
        Value::String(agent.instructions.clone()),
    );
    set_if_not_empty(
        &mut agent_obj,
        "model",
        agent.model.as_deref().unwrap_or(""),
    );
    set_if_not_empty(
        &mut agent_obj,
        "thinking_level",
        agent.thinking_level.as_deref().unwrap_or(""),
    );
    set_if_not_empty(
        &mut agent_obj,
        "service_tier",
        agent.service_tier.as_deref().unwrap_or(""),
    );
    if !disabled_skills.is_empty() {
        agent_obj.insert(
            "disabled_runtime_skills".into(),
            Value::Array(disabled_skills),
        );
    }

    // System agents carry a product-owned instruction layer shipped with the
    // binary (hot-updatable); workspace notes stay in the row.
    let mut instructions = agent.instructions.clone();
    if agent.system_key.as_deref() == Some(patchbay_service::builtin_agents::MIKA_SYSTEM_KEY) {
        instructions = patchbay_service::builtin_agents::compose_mika_instructions(
            &agent.name,
            &agent.instructions,
        );
    }
    agent_obj.insert("instructions".into(), Value::String(instructions));

    // Skills: slim refs when the daemon advertises skill-bundles-v1, full
    // content otherwise.
    if use_skill_refs {
        let (_, refs) = state.tasks.load_agent_skill_bundles(task.agent_id).await;
        if !refs.is_empty() {
            if let Ok(v) = serde_json::to_value(&refs) {
                agent_obj.insert("skill_refs".into(), v);
            }
        }
    } else {
        let mut skills = state.tasks.load_agent_skills(task.agent_id).await;
        skills.extend(patchbay_service::builtin_skills::load_builtin_skills());
        if !skills.is_empty() {
            if let Ok(v) = serde_json::to_value(&skills) {
                agent_obj.insert("skills".into(), v);
            }
        }
    }

    obj.insert("agent".into(), Value::Object(agent_obj));

    // Identity guard: response agent must agree with the task row.
    let response_agent_id = obj
        .get("agent")
        .and_then(|a| a.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if response_agent_id.is_empty() || response_agent_id != task.agent_id.to_string() {
        tracing::error!(
            task_id = %task.id,
            task_agent_id = %task.agent_id,
            response_agent_id = %response_agent_id,
            "daemon claim: response agent identity mismatch; refusing dispatch"
        );
        return Err(fail_claimed_task_before_launch(
            state,
            task,
            "Task identity is invalid: the task and response agent disagree.",
            patchbay_task_failure::Reason::INVALID_TASK_IDENTITY,
            "error_invalid_task_identity",
            StatusCode::CONFLICT,
            "task response agent identity mismatch",
        )
        .await);
    }

    // The requesting user is the task's on-behalf originator, never the
    // Runtime or Agent owner. Owner profile text here would be both private
    // data disclosure and an instruction-confusion path for shared Agents.
    if let Some(requesting_user_id) = task.originator_user_id {
        if let Ok(Some(requesting_user)) = user_q::get_user(&state.pool, requesting_user_id).await {
            set_if_not_empty(obj, "requesting_user_name", &requesting_user.name);
            if !requesting_user.profile_description.is_empty() {
                obj.insert(
                    "requesting_user_profile_description".into(),
                    Value::String(requesting_user.profile_description.clone()),
                );
            }
        }
    }

    // Stored chat initiator (PB-2645).
    if let Some(initiator_id) = task.initiator_user_id {
        obj.insert("initiator_type".into(), Value::String("member".into()));
        obj.insert(
            "initiator_id".into(),
            Value::String(initiator_id.to_string()),
        );
        if let Ok(Some(u)) = user_q::get_user(&state.pool, initiator_id).await {
            obj.insert("initiator_name".into(), Value::String(u.name.clone()));
            obj.insert("initiator_email".into(), Value::String(u.email.clone()));
        }
    }

    let has_quick_create = task.issue_id.is_none()
        && task.chat_session_id.is_none()
        && task.autopilot_run_id.is_none();

    // ---- Issue-bound tasks -------------------------------------------------
    if let Some(issue_id) = task.issue_id {
        if let Ok(Some(issue)) = issue_q::get_issue(&state.pool, issue_id).await {
            obj.insert(
                "workspace_id".into(),
                Value::String(issue.workspace_id.to_string()),
            );
            obj.insert("thread_name".into(), Value::String(issue.title.clone()));

            // Team-leader briefing injection keyed off is_leader_task +
            // team_id, NOT off the issue assignee (PB-3724 covers the
            // mention path).
            if task.is_leader_task {
                let mut injected = false;
                if let Some(team_id) = task.team_id {
                    if let Ok(Some(team)) =
                        team_q::get_team_in_workspace(&state.pool, team_id, issue.workspace_id)
                            .await
                    {
                        if team.leader_id.to_string() == response_agent_id {
                            let owns_issue_status = issue.assignee_type.as_deref() == Some("team")
                                && issue.assignee_id == Some(team.id);
                            let briefing =
                                build_team_leader_briefing(&state.pool, &team, owns_issue_status)
                                    .await;
                            let current = obj
                                .get("agent")
                                .and_then(|a| a.get("instructions"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let combined = if current.trim().is_empty() {
                                briefing
                            } else {
                                format!("{current}\n\n{briefing}")
                            };
                            if let Some(a) = obj.get_mut("agent").and_then(|v| v.as_object_mut()) {
                                a.insert("instructions".into(), Value::String(combined));
                            }
                            injected = true;
                        }
                    }
                }
                // Every skip leaves a task the daemon must NOT run as a leader:
                // clear the flag so "is_leader_task on the wire ⇔ briefing
                // injected" stays true (PB-5811).
                if !injected {
                    obj.insert("is_leader_task".into(), Value::Bool(false));
                    tracing::warn!(
                        task_id = %task.id,
                        team_id = ?task.team_id,
                        agent_id = %task.agent_id,
                        "team leader briefing not injected; claim delivered as a non-leader task"
                    );
                }
            }

            // Project repos override workspace repos when present.
            let mut project_repos = Vec::new();
            if let Some(project_id) = issue.project_id {
                obj.insert("project_id".into(), Value::String(project_id.to_string()));
                project_repos = apply_project_context(state, obj, project_id).await;
            }
            if !project_repos.is_empty() {
                obj.insert("repos".into(), Value::Array(project_repos));
            } else {
                let repos =
                    workspace_repos_or(state, issue.workspace_id, Value::Array(Vec::new())).await;
                if repos.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                    obj.insert("repos".into(), repos);
                }
            }

            // Comment input: chronological, de-duplicated, budget-bounded.
            let mut planned_comment_ids: Vec<Uuid> = task.coalesced_comment_ids.clone();
            if let Some(trigger) = task.trigger_comment_id {
                planned_comment_ids.push(trigger);
            }
            let loaded_comments = build_coalesced_comment_data(
                &state.pool,
                runtime.workspace_id,
                &planned_comment_ids,
            )
            .await;
            let trigger_comment_id = task
                .trigger_comment_id
                .map(|u| u.to_string())
                .unwrap_or_default();
            let trigger_loaded = loaded_comments.iter().any(|c| c.id == trigger_comment_id);
            if task.trigger_comment_id.is_some() && trigger_loaded {
                delivered_comments = select_comment_delivery(
                    &loaded_comments,
                    &trigger_comment_id,
                    !supports_coalesced_comments,
                    MAX_CLAIM_COMMENT_PAYLOAD_BYTES,
                );
            }

            // The claim advertises only the structured ids actually present in
            // this payload.
            obj.insert("coalesced_comment_ids".into(), json!(Vec::<String>::new()));
            let mut coalesced_out: Vec<Value> = Vec::new();
            let mut coalesced_ids_out: Vec<String> = Vec::new();
            for comment in &delivered_comments {
                if comment.id == trigger_comment_id {
                    obj.insert(
                        "trigger_comment_content".into(),
                        Value::String(comment.content.clone()),
                    );
                    obj.insert(
                        "trigger_thread_id".into(),
                        Value::String(comment.thread_id.clone()),
                    );
                    obj.insert(
                        "trigger_author_type".into(),
                        Value::String(comment.author_type.clone()),
                    );
                    obj.insert(
                        "trigger_author_name".into(),
                        Value::String(comment.author_name.clone()),
                    );
                    continue;
                }
                coalesced_ids_out.push(comment.id.clone());
                coalesced_out.push(comment.to_json());
            }
            if !coalesced_ids_out.is_empty() {
                obj.insert("coalesced_comment_ids".into(), json!(coalesced_ids_out));
            }
            if !coalesced_out.is_empty() {
                obj.insert("coalesced_comments".into(), Value::Array(coalesced_out));
            }

            // Trigger author resolution + catch-up count (workspace-scoped).
            if let Some(trigger_uuid) = task.trigger_comment_id {
                if let Ok(Some(comment)) = comment_q::get_comment_in_workspace(
                    &state.pool,
                    trigger_uuid,
                    runtime.workspace_id,
                )
                .await
                {
                    obj.insert(
                        "trigger_comment_content".into(),
                        Value::String(comment.content.clone()),
                    );
                    let thread_id = comment.parent_id.unwrap_or(comment.id).to_string();
                    obj.insert("trigger_thread_id".into(), Value::String(thread_id));
                    obj.insert(
                        "trigger_author_type".into(),
                        Value::String(comment.author_type.clone()),
                    );
                    obj.insert(
                        "initiator_type".into(),
                        Value::String(comment.author_type.clone()),
                    );
                    obj.insert(
                        "initiator_id".into(),
                        Value::String(comment.author_id.to_string()),
                    );
                    match comment.author_type.as_str() {
                        "agent" => {
                            if let Ok(Some(a)) =
                                agent_queries::get_agent(&state.pool, comment.author_id).await
                            {
                                obj.insert(
                                    "trigger_author_name".into(),
                                    Value::String(a.name.clone()),
                                );
                                obj.insert("initiator_name".into(), Value::String(a.name.clone()));
                            }
                        }
                        "member" => {
                            if let Ok(Some(u)) =
                                user_q::get_user(&state.pool, comment.author_id).await
                            {
                                obj.insert(
                                    "trigger_author_name".into(),
                                    Value::String(u.name.clone()),
                                );
                                obj.insert("initiator_name".into(), Value::String(u.name.clone()));
                                obj.insert(
                                    "initiator_email".into(),
                                    Value::String(u.email.clone()),
                                );
                            }
                        }
                        _ => {}
                    }
                    // Catch-up hint: comments since this agent's last started run.
                    if let Ok(Some(Some(started_at))) =
                        agent_queries::get_last_task_started_at_for_issue_and_agent(
                            &state.pool,
                            task.agent_id,
                            comment.issue_id,
                        )
                        .await
                    {
                        if let Ok(Some(cnt)) = comment_q::count_new_comments_since(
                            &state.pool,
                            comment.issue_id,
                            comment.workspace_id,
                            Some(started_at),
                            trigger_uuid,
                            task.agent_id,
                        )
                        .await
                        {
                            if cnt > 0 {
                                obj.insert("new_comment_count".into(), json!(cnt));
                                obj.insert(
                                    "new_comments_since".into(),
                                    Value::String(rfc3339(started_at)),
                                );
                            }
                        }
                    }
                }
            }

            if !supports_coalesced_comments {
                // Legacy daemons fold every comment into the one trigger field.
                let has_structured = obj.contains_key("coalesced_comments");
                let trigger_empty = obj
                    .get("trigger_comment_content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .is_empty();
                if has_structured || (trigger_empty && !delivered_comments.is_empty()) {
                    let bundle = format_legacy_comment_bundle(&delivered_comments);
                    obj.insert("trigger_comment_content".into(), Value::String(bundle));
                }
                obj.remove("coalesced_comment_ids");
                obj.remove("coalesced_comments");
            } else {
                let trigger_empty = obj
                    .get("trigger_comment_content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .is_empty();
                if trigger_empty && !delivered_comments.is_empty() {
                    obj.insert(
                        "trigger_comment_content".into(),
                        Value::String(
                            "The newest triggering comment is no longer available. Address every earlier comment included below."
                                .to_string(),
                        ),
                    );
                }
            }

            // `/goal` is deliberately explicit and line-oriented. Ordinary
            // issue runs keep their one-turn contract; a member or coordinator
            // can opt a Codex run into app-server autonomous continuation by
            // putting the directive on its own line after the @mention.
            if let Some(goal) = obj
                .get("trigger_comment_content")
                .and_then(Value::as_str)
                .and_then(explicit_goal_objective)
            {
                obj.insert("goal_objective".into(), Value::String(goal));
            }

            // Prior session / workdir resolution.
            if let Some(rerun_of) = task.rerun_of_task_id {
                // Manual retry resumes precisely from the clicked source task.
                match agent_queries::get_agent_task(&state.pool, rerun_of).await {
                    Ok(Some(src)) if rerun_source_matches_task_scope(task, &src) => {
                        if let Some(work_dir) = &src.work_dir {
                            if !work_dir.is_empty() {
                                obj.insert(
                                    "prior_work_dir".into(),
                                    Value::String(work_dir.clone()),
                                );
                            }
                        }
                        let resume_unsafe = patchbay_service::task_helpers::resume_unsafe_failure(
                            src.failure_reason.as_deref().unwrap_or(""),
                            src.error.as_deref().unwrap_or(""),
                        );
                        if !resume_unsafe
                            && src.session_id.is_some()
                            && src.runtime_id == task.runtime_id
                        {
                            set_if_not_empty(
                                obj,
                                "prior_session_id",
                                src.session_id.as_deref().unwrap_or(""),
                            );
                        }
                        if src.session_rollout_missing {
                            obj.insert(
                                "prior_session_resume_unavailable".into(),
                                Value::Bool(true),
                            );
                        }
                    }
                    Ok(Some(_src)) => {
                        tracing::warn!(
                            task_id = %task.id,
                            "daemon claim: rerun source belongs to another agent or scope; starting fresh"
                        );
                        obj.insert("prior_session_resume_unavailable".into(), Value::Bool(true));
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::debug!(error = %e, "claim: rerun source lookup failed");
                    }
                }
            } else if !task.force_fresh_session {
                if let Ok(Some(prior)) = agent_queries::get_last_task_session(
                    &state.pool,
                    task.agent_id,
                    task.issue_id.unwrap_or_else(Uuid::nil),
                )
                .await
                {
                    if prior.session_id.is_some() {
                        if prior.runtime_id == task.runtime_id {
                            set_if_not_empty(
                                obj,
                                "prior_session_id",
                                prior.session_id.as_deref().unwrap_or(""),
                            );
                        }
                        if let Some(work_dir) = prior.work_dir.filter(|w| !w.is_empty()) {
                            obj.insert("prior_work_dir".into(), Value::String(work_dir));
                        }
                    }
                }
                if let Ok(Some(missing)) = agent_queries::get_latest_task_rollout_missing(
                    &state.pool,
                    task.agent_id,
                    task.issue_id.unwrap_or_else(Uuid::nil),
                )
                .await
                {
                    if missing {
                        obj.insert("prior_session_resume_unavailable".into(), Value::Bool(true));
                    }
                }
            }

            // Message Bus turns use the normal issue+Agent continuity lookup
            // above. The deferred promoter waits for every earlier main turn,
            // and the lookup excludes Side Chat sessions, so this resumes the
            // latest state of the main conversation instead of a stale branch.
            // If no resumable session supplied a checkout, retain the anchor
            // task's copied workdir so a provider failure does not discard
            // already-written workspace state.
            if obj.contains_key("message_bus_parent_task_id") && !obj.contains_key("prior_work_dir")
            {
                set_if_not_empty(
                    obj,
                    "prior_work_dir",
                    task.work_dir.as_deref().unwrap_or(""),
                );
            }
        }
    }

    // ---- Chat tasks ---------------------------------------------------------
    if let Some(chat_session_id) = task.chat_session_id {
        if let Ok(Some(cs)) = chat_q::get_chat_session(&state.pool, chat_session_id).await {
            obj.insert(
                "workspace_id".into(),
                Value::String(cs.workspace_id.to_string()),
            );
            obj.insert("chat_session_id".into(), Value::String(cs.id.to_string()));
            obj.insert("thread_name".into(), Value::String(cs.title.clone()));

            // Historical intro sessions carry no opening human message; flag
            // only when the creator hasn't replied yet (PB-4259).
            if cs.is_agent_intro {
                match chat_q::chat_session_has_user_message(&state.pool, cs.id).await {
                    Ok(has_user) => {
                        obj.insert("chat_intro".into(), Value::Bool(!has_user.unwrap_or(false)));
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            chat_session_id = %cs.id,
                            "chat intro gate: has-user-message check failed"
                        );
                    }
                }
            }

            // Channel-backed session flag: read WITHOUT naming a channel
            // (the binding row itself is the answer — PB-4899).
            if let Ok(Some(binding)) =
                patchbay_db::queries::channel::get_channel_chat_session_binding_by_session_any(
                    &state.pool,
                    cs.id,
                )
                .await
            {
                obj.insert(
                    "chat_channel_type".into(),
                    Value::String(binding.channel_type.clone()),
                );
                obj.insert("chat_type".into(), Value::String(binding.chat_type.clone()));
                // File delivery is a deployment fact; the Rust lane carries the
                // same conservative default (false) until adapters declare it.
                if binding.channel_type == patchbay_slack::TYPE_SLACK {
                    obj.insert(
                        "chat_in_thread".into(),
                        Value::Bool(
                            binding.last_thread_id.is_some()
                                && !binding.last_thread_id.as_deref().unwrap_or("").is_empty()
                                && binding.last_thread_id != binding.last_message_id,
                        ),
                    );
                }
            }

            let mut project_repos = Vec::new();
            if let Some(project_id) = cs.project_id {
                if let Ok(Some(project)) = patchbay_db::queries::project::get_project_in_workspace(
                    &state.pool,
                    project_id,
                    cs.workspace_id,
                )
                .await
                {
                    obj.insert("project_id".into(), Value::String(project.id.to_string()));
                    obj.insert("project_title".into(), Value::String(project.title.clone()));
                    if let Some(desc) = &project.description {
                        obj.insert("project_description".into(), Value::String(desc.clone()));
                    }
                    project_repos = apply_project_context(state, obj, project.id).await;
                }
            }
            if !project_repos.is_empty() {
                obj.insert("repos".into(), Value::Array(project_repos));
            } else {
                let repos =
                    workspace_repos_or(state, cs.workspace_id, Value::Array(Vec::new())).await;
                if repos.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                    obj.insert("repos".into(), repos);
                }
            }

            if !task.force_fresh_session {
                if let (Some(session_id), Some(cs_runtime_id)) = (&cs.session_id, cs.runtime_id) {
                    if Some(cs_runtime_id) == task.runtime_id {
                        set_if_not_empty(obj, "prior_session_id", session_id);
                    }
                }
                if let Some(work_dir) = &cs.work_dir {
                    if !work_dir.is_empty() {
                        obj.insert("prior_work_dir".into(), Value::String(work_dir.clone()));
                    }
                }
                // Fallback: most recent chat task session with matching runtime.
                if chat_session_resume_fallback_needed(
                    obj.get("prior_session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                    obj.get("prior_work_dir")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                ) {
                    if let Ok(Some(prior)) =
                        chat_q::get_last_chat_task_session(&state.pool, cs.id).await
                    {
                        if prior.session_id.is_some() {
                            if prior.runtime_id == task.runtime_id
                                && obj
                                    .get("prior_session_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .is_empty()
                            {
                                set_if_not_empty(
                                    obj,
                                    "prior_session_id",
                                    prior.session_id.as_deref().unwrap_or(""),
                                );
                            }
                            if let Some(work_dir) = prior.work_dir.filter(|w| !w.is_empty()) {
                                if obj
                                    .get("prior_work_dir")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .is_empty()
                                {
                                    obj.insert("prior_work_dir".into(), Value::String(work_dir));
                                }
                            }
                        }
                    }
                    if let Ok(Some(missing)) =
                        agent_queries::get_latest_chat_task_rollout_missing(&state.pool, cs.id)
                            .await
                    {
                        if missing {
                            obj.insert(
                                "prior_session_resume_unavailable".into(),
                                Value::Bool(true),
                            );
                        }
                    }
                }
            }

            // User-message input batch (owned or legacy trailing selector).
            let unanswered: Result<Vec<patchbay_db::models::ChatMessage>, _> =
                if let Some(input_task_id) = task.chat_input_task_id {
                    chat_q::list_chat_input_messages(&state.pool, input_task_id).await
                } else {
                    chat_q::list_chat_messages_for_legacy_task(&state.pool, cs.id)
                        .await
                        .map(|msgs| trailing_user_messages(msgs, chrono::Utc::now()))
                };
            let unanswered = match unanswered {
                Ok(msgs) => msgs,
                Err(e) => {
                    // Preserve the just-dispatched task for redelivery rather
                    // than cancelling a valid direct task (PB-4351 review).
                    tracing::error!(
                        error = %e,
                        task_id = %task.id,
                        chat_session_id = %cs.id,
                        "chat claim: load chat input messages failed; preserving task for redelivery"
                    );
                    return Err(ClaimBuildFailure::new(
                        "error_chat_input_load",
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "failed to load chat input",
                    ));
                }
            };

            let mut parts: Vec<String> = Vec::with_capacity(unanswered.len());
            for m in &unanswered {
                if !m.content.trim().is_empty() {
                    parts.push(m.content.clone());
                }
                if let Ok(atts) =
                    patchbay_db::queries::attachment::list_attachments_by_chat_message(
                        &state.pool,
                        m.id,
                        cs.workspace_id,
                    )
                    .await
                {
                    let mut metas = Vec::with_capacity(atts.len());
                    for a in atts {
                        let mut meta = json!({ "id": a.id.to_string(), "filename": a.filename });
                        if !a.content_type.is_empty() {
                            meta["content_type"] = Value::String(a.content_type.clone());
                        }
                        metas.push(meta);
                    }
                    if !metas.is_empty() {
                        let entry = obj
                            .entry("chat_message_attachments")
                            .or_insert_with(|| Value::Array(Vec::new()));
                        if let Value::Array(arr) = entry {
                            arr.append(&mut metas);
                        }
                    }
                }
            }
            let chat_message = parts.join("\n\n");
            if !chat_message.is_empty() {
                obj.insert("chat_message".into(), Value::String(chat_message.clone()));
            }

            // Fail closed on empty task-owned input (PB-4351).
            let chat_intro = obj
                .get("chat_intro")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if task.chat_input_task_id.is_some() && !chat_intro && chat_message.trim().is_empty() {
                tracing::error!(
                    task_id = %task.id,
                    chat_session_id = %cs.id,
                    "chat claim: task-owned direct task has no user input; cancelling"
                );
                if let Err(e) = state.tasks.cancel_task(task.id).await {
                    tracing::error!(error = %e, task_id = %task.id, "chat claim: cancel after empty input failed");
                }
                return Err(ClaimBuildFailure::new(
                    "error_empty_chat_input",
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "chat task has no user input",
                ));
            }

            let thread_name = obj
                .get("thread_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if thread_name.trim().is_empty() && !chat_message.is_empty() {
                obj.insert("thread_name".into(), Value::String(chat_message.clone()));
            }
        }
    }

    // ---- Autopilot run tasks -----------------------------------------------
    if let Some(run_id) = task.autopilot_run_id {
        if let Ok(Some(run)) = autopilot_q::get_autopilot_run(&state.pool, run_id).await {
            obj.insert(
                "autopilot_id".into(),
                Value::String(run.autopilot_id.to_string()),
            );
            obj.insert("autopilot_source".into(), Value::String(run.source.clone()));
            if let Some(tp) = run.trigger_payload {
                obj.insert("autopilot_trigger_payload".into(), tp);
            }
            if let Ok(Some(ap)) = autopilot_q::get_autopilot(&state.pool, run.autopilot_id).await {
                obj.insert("autopilot_title".into(), Value::String(ap.title.clone()));
                obj.insert("thread_name".into(), Value::String(ap.title.clone()));
                if let Some(desc) = &ap.description {
                    obj.insert("autopilot_description".into(), Value::String(desc.clone()));
                }
                let ws_empty = obj
                    .get("workspace_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .is_empty();
                if ws_empty {
                    obj.insert(
                        "workspace_id".into(),
                        Value::String(ap.workspace_id.to_string()),
                    );
                }
                let repos_empty = obj
                    .get("repos")
                    .and_then(|v| v.as_array())
                    .map(|a| a.is_empty())
                    .unwrap_or(true);
                if repos_empty {
                    let repos =
                        workspace_repos_or(state, ap.workspace_id, Value::Array(Vec::new())).await;
                    if repos.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                        obj.insert("repos".into(), repos);
                    }
                }
            }
        }
    }

    // ---- Quick-create tasks --------------------------------------------------
    if has_quick_create {
        if let Some(qc) =
            patchbay_service::task_service::TaskService::parse_quick_create_context(task)
        {
            obj.insert(
                "quick_create_prompt".into(),
                Value::String(qc.prompt.clone()),
            );
            set_if_not_empty(obj, "quick_create_priority", &qc.priority);
            set_if_not_empty(obj, "quick_create_due_date", &qc.due_date);
            if !qc.attachment_ids.is_empty() {
                obj.insert(
                    "quick_create_attachment_ids".into(),
                    json!(qc.attachment_ids),
                );
            }
            obj.insert("thread_name".into(), Value::String(qc.prompt.clone()));
            obj.insert(
                "workspace_id".into(),
                Value::String(qc.workspace_id.clone()),
            );

            let mut project_repos = Vec::new();
            if !qc.project_id.is_empty() {
                if let Ok(project_uuid) = Uuid::parse_str(&qc.project_id) {
                    obj.insert("project_id".into(), Value::String(qc.project_id.clone()));
                    project_repos = apply_project_context(state, obj, project_uuid).await;
                }
            }
            if !project_repos.is_empty() {
                obj.insert("repos".into(), Value::Array(project_repos));
            } else if let Ok(ws_uuid) = Uuid::parse_str(&qc.workspace_id) {
                let repos = workspace_repos_or(state, ws_uuid, Value::Array(Vec::new())).await;
                if repos.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                    obj.insert("repos".into(), repos);
                }
            }

            // Parent-issue resolution ("Add sub issue").
            if !qc.parent_issue_id.is_empty() {
                obj.insert(
                    "parent_issue_id".into(),
                    Value::String(qc.parent_issue_id.clone()),
                );
                if let (Ok(parent_uuid), Ok(ws_uuid)) = (
                    Uuid::parse_str(&qc.parent_issue_id),
                    Uuid::parse_str(&qc.workspace_id),
                ) {
                    if let Ok(Some(parent)) =
                        issue_q::get_issue_in_workspace(&state.pool, parent_uuid, ws_uuid).await
                    {
                        if let Ok(Some(ws)) = workspace_q::get_workspace(&state.pool, ws_uuid).await
                        {
                            obj.insert(
                                "parent_issue_identifier".into(),
                                Value::String(format!("{}-{}", ws.issue_prefix, parent.number)),
                            );
                        }
                    }
                }
            }

            // Team-leader briefing for quick-create teams (no issue yet →
            // never owns parent status on this turn).
            if !qc.team_id.is_empty() {
                if let (Ok(team_uuid), Ok(ws_uuid)) = (
                    Uuid::parse_str(&qc.team_id),
                    Uuid::parse_str(&qc.workspace_id),
                ) {
                    if let Ok(Some(team)) =
                        team_q::get_team_in_workspace(&state.pool, team_uuid, ws_uuid).await
                    {
                        if team.leader_id.to_string() == response_agent_id {
                            let briefing =
                                build_team_leader_briefing(&state.pool, &team, false).await;
                            let current = obj
                                .get("agent")
                                .and_then(|a| a.get("instructions"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let combined = if current.trim().is_empty() {
                                briefing
                            } else {
                                format!("{current}\n\n{briefing}")
                            };
                            if let Some(a) = obj.get_mut("agent").and_then(|v| v.as_object_mut()) {
                                a.insert("instructions".into(), Value::String(combined));
                            }
                            obj.insert("team_id".into(), Value::String(team.id.to_string()));
                            obj.insert("team_name".into(), Value::String(team.name.clone()));
                        }
                    }
                }
            }
        }
    }

    // ---- Workspace isolation check ------------------------------------------
    let resolved_workspace = obj
        .get("workspace_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if resolved_workspace.is_empty() || resolved_workspace != runtime_workspace_id {
        tracing::error!(
            task_id = %task.id,
            runtime_id = %runtime_id,
            runtime_workspace = %runtime_workspace_id,
            resolved_workspace = %resolved_workspace,
            "task claim: workspace isolation check failed, cancelling task"
        );
        if let Err(e) = state.tasks.cancel_task(task.id).await {
            tracing::error!(error = %e, task_id = %task.id, "task claim: cancel after workspace check failed");
        }
        return Err(ClaimBuildFailure::new(
            "error_workspace",
            StatusCode::INTERNAL_SERVER_ERROR,
            "task workspace isolation check failed",
        ));
    }

    // ---- Active sibling runs -------------------------------------------------
    let resolved_ws_uuid = resolved_workspace.parse().unwrap_or_else(|_| Uuid::nil());
    match agent_queries::list_active_sibling_issue_tasks(
        &state.pool,
        task.agent_id,
        task.id,
        resolved_ws_uuid,
    )
    .await
    {
        Ok(siblings) => {
            let runs: Vec<Value> = siblings
                .into_iter()
                .map(|s| {
                    let mut run = json!({
                        "task_id": s.task_id.map(|u| u.to_string()).unwrap_or_default(),
                        "issue_id": s.issue_id.map(|u| u.to_string()).unwrap_or_default(),
                        "issue_identifier": format!("{}-{}", s.issue_prefix, s.issue_number),
                        "issue_title": s.issue_title,
                        "status": s.status,
                        "created_at": s.created_at.map(crate::timefmt::rfc3339).unwrap_or_default(),
                    });
                    if let Some(started) = s.started_at {
                        run["started_at"] = Value::String(crate::timefmt::rfc3339(started));
                    }
                    run
                })
                .collect();
            if !runs.is_empty() {
                obj.insert("active_sibling_runs".into(), Value::Array(runs));
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                task_id = %task.id,
                agent_id = %task.agent_id,
                "task claim: failed to load active sibling runs"
            );
        }
    }

    // ---- Workspace context injection ----------------------------------------
    if let Ok(Some(ws)) = workspace_q::get_workspace(&state.pool, resolved_ws_uuid).await {
        if let Some(ctx) = ws.context.filter(|c| !c.is_empty()) {
            obj.insert("workspace_context".into(), Value::String(ctx));
        }
    } else {
        tracing::warn!(
            task_id = %task.id,
            workspace_id = %resolved_workspace,
            "task claim: failed to load workspace for context injection"
        );
    }

    // ---- Worktree-mode version gate ------------------------------------------
    let resources: Vec<Value> = obj
        .get("project_resources")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let reason = worktree_claim_block_reason(
        &resources,
        runtime,
        request_has_client_capability(headers, DAEMON_CAPABILITY_LOCAL_WORKTREE_V1),
    );
    if !reason.is_empty() {
        tracing::error!(
            task_id = %task.id,
            runtime_id = %runtime_id,
            daemon_id = ?runtime.daemon_id,
            reason = %reason,
            "task claim: runtime too old for worktree mode; cancelling rather than running in place"
        );
        match state
            .tasks
            .cancel_task_with_reason(task.id, &reason, "local_directory_error")
            .await
        {
            Ok(_) => {
                return Err(ClaimBuildFailure::new(
                    "error_worktree_daemon_version",
                    StatusCode::UNPROCESSABLE_ENTITY,
                    &reason,
                ));
            }
            Err(e) => {
                // Cancel did not commit: requeue so the next claim retries the
                // gate, and report a transient 5xx.
                tracing::error!(
                    error = %e,
                    task_id = %task.id,
                    "task claim: cancel after worktree version gate failed; requeueing so the gate can run again"
                );
                let _ = state.tasks.requeue_task_after_claim_failure(task).await;
                return Err(ClaimBuildFailure::new(
                    "error_worktree_gate_cancel",
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "failed to cancel a worktree task blocked by daemon version; task requeued",
                ));
            }
        }
    }

    let comment_backed =
        task.trigger_comment_id.is_some() || !task.coalesced_comment_ids.is_empty();

    // delivered_comment_ids on the wire reflect the builder's receipt.
    if !delivered_comments.is_empty() || comment_backed {
        let ids = comment_data_ids(&delivered_comments);
        obj.insert(
            "delivered_comment_ids".into(),
            Value::Array(ids.iter().map(|u| Value::String(u.to_string())).collect()),
        );
    }

    strip_unleased_execution_resume(obj);

    let _ = header::CACHE_CONTROL; // keep import surface minimal

    Ok(BuiltClaim {
        payload,
        delivered_comment_ids: comment_data_ids(&delivered_comments),
        comment_backed,
    })
}

fn capability_scoped_task_overlay(_persisted: Option<&Value>) -> Option<&Value> {
    None
}

fn chat_session_resume_fallback_needed(prior_session_id: &str, prior_work_dir: &str) -> bool {
    prior_session_id.is_empty() || prior_work_dir.is_empty()
}

fn explicit_goal_objective(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        let mut parts = line.splitn(2, char::is_whitespace);
        let Some(command) = parts.next() else {
            continue;
        };
        if !command.eq_ignore_ascii_case("/goal") {
            continue;
        }
        let objective = parts.next().unwrap_or_default().trim();
        return Some(if objective.is_empty() {
            "Complete every remaining requirement for this issue, verify the result, and report only when the objective is complete or genuinely blocked."
                .to_string()
        } else {
            objective.to_string()
        });
    }
    None
}

/// Go failClaimedTaskBeforeLaunch: settles a durable claim-time rejection before
/// the daemon ever receives the task. If settlement fails, release the exact
/// claim so a later attempt can retry the gate.
async fn fail_claimed_task_before_launch(
    state: &DaemonClaimServices,
    task: &AgentTaskQueue,
    user_message: &str,
    failure_reason: patchbay_task_failure::Reason,
    outcome: &'static str,
    status: StatusCode,
    claim_message: &str,
) -> ClaimBuildFailure {
    let settled = state
        .tasks
        .fail_task(
            task.id,
            user_message,
            "",
            "",
            "",
            failure_reason.as_str(),
            false,
            "",
            "",
        )
        .await;
    match settled {
        Ok(_) => ClaimBuildFailure::new(outcome, status, claim_message),
        Err(e) => {
            tracing::error!(
                error = %e,
                task_id = %task.id,
                outcome = %outcome,
                "task claim: fail rejected task failed; requeueing claim"
            );
            if let Err(requeue_err) = state.tasks.requeue_task_after_claim_failure(task).await {
                tracing::error!(
                    error = %requeue_err,
                    task_id = %task.id,
                    "task claim: requeue after rejected-task settlement failure failed; stale reclaim will recover it"
                );
            }
            let mut failure = ClaimBuildFailure::new(
                outcome,
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to settle a task rejected before launch",
            );
            failure.outcome = format!("{outcome}_settle");
            failure
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        capability_scoped_task_overlay, explicit_goal_objective,
        strip_unleased_execution_resume,
    };
    use serde_json::json;

    #[test]
    fn goal_directive_is_explicit_and_line_oriented() {
        assert_eq!(
            explicit_goal_objective("[@Worker](mention://agent/id)\n\n/goal finish stages 2 and 3"),
            Some("finish stages 2 and 3".to_string())
        );
        assert!(explicit_goal_objective("Please discuss /goal support").is_none());
    }

    #[test]
    fn empty_goal_uses_the_issue_completion_contract() {
        let objective = explicit_goal_objective("/GOAL").expect("goal objective");
        assert!(objective.contains("every remaining requirement"));
        assert!(objective.contains("genuinely blocked"));
    }

    #[test]
    fn legacy_owner_composio_overlay_is_not_delivered_to_a_task() {
        let legacy = json!({
            "mcpServers": {
                "composio": {
                    "type": "http",
                    "url": "https://mcp.example/session",
                    "headers": {"x-api-key": "owner-platform-secret"}
                }
            }
        });
        assert!(capability_scoped_task_overlay(Some(&legacy)).is_none());
    }

    #[test]
    fn stored_worktree_and_provider_session_are_not_delivered_to_a_task() {
        let mut payload = json!({
            "work_dir": "/owner/checkout",
            "relative_work_dir": "task/worktree",
            "durable_work_dir": "/owner/durable",
            "relative_durable_work_dir": "durable",
            "branch_name": "owner/private",
            "prior_work_dir": "/owner/prior",
            "prior_session_id": "owner-provider-session",
            "title": "safe metadata"
        })
        .as_object()
        .cloned()
        .expect("object");
        strip_unleased_execution_resume(&mut payload);
        for denied in [
            "work_dir",
            "relative_work_dir",
            "durable_work_dir",
            "relative_durable_work_dir",
            "branch_name",
            "prior_work_dir",
            "prior_session_id",
        ] {
            assert!(!payload.contains_key(denied));
        }
        assert_eq!(payload.get("title"), Some(&json!("safe metadata")));
    }

}
