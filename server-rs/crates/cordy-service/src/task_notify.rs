//! Task-side broadcast / notification / mapping helpers — port of the
//! service-method half of `service/task.go` L5710-6710 (ReportProgress,
//! agent status, skills loading, chat:done, issue:updated, agent comments,
//! issue maps, quick-create inbox outcomes) plus `agent_ready.go`-free
//! pieces. The terminal/retry methods that consume these live in
//! `task_terminal.rs`.

use serde_json::json;
use uuid::Uuid;

use cordy_db::dbid::new_v7;
use cordy_db::models::{AgentTaskQueue, ChatMessage, Comment, Issue};
use cordy_db::queries::agent::{
    cancel_deferred_escalations_for_issue_agent, link_task_to_issue,
    update_agent_status as update_agent_status_query,
};
use cordy_db::queries::comment::{create_comment, get_thread_root, unresolve_comment};
use cordy_db::queries::inbox::create_inbox_item;
use cordy_db::queries::issue::get_issue;
use cordy_db::queries::issue::get_issue_by_origin;
use cordy_db::queries::skill::{list_agent_skills, list_skill_files};
use cordy_db::queries::workspace::get_workspace;
use cordy_protocol::messages::{ChatDonePayload, TaskProgressPayload};

use crate::builtin_skills::{load_builtin_skills, AgentSkillData};
use crate::issue_status;
use crate::redact;
use crate::skill_bundle::{build_manifest, File as BundleFile, Skill as BundleSkill};
use crate::task_service::{QuickCreateContext, TaskService};

/// Slim claim payload for a skill — port of Go AgentSkillRefData (L5878).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentSkillRefData {
    pub id: String,
    pub source: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub hash: String,
    pub size_bytes: i64,
    pub file_count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<AgentSkillFileRefData>,
}

/// Per-file slim reference — port of Go AgentSkillFileRefData (L5889).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentSkillFileRefData {
    pub path: String,
    pub sha256: String,
    pub size_bytes: i64,
}

/// Ports BuildAgentSkillBundles: every skill gets a stable bundle hash plus a
/// lightweight ref for slim claims.
pub fn build_agent_skill_bundles(
    mut skills: Vec<AgentSkillData>,
) -> (Vec<AgentSkillData>, Vec<AgentSkillRefData>) {
    let mut bundles = Vec::with_capacity(skills.len());
    let mut refs = Vec::with_capacity(skills.len());
    for skill in skills.iter_mut() {
        let source = if !skill.source.is_empty() {
            skill.source.clone()
        } else if skill.id.is_empty() {
            crate::skill_bundle::SOURCE_BUILTIN.to_string()
        } else {
            crate::skill_bundle::SOURCE_WORKSPACE.to_string()
        };
        let id = if skill.id.is_empty() && source == crate::skill_bundle::SOURCE_BUILTIN {
            format!("builtin:{}", skill.name)
        } else {
            skill.id.clone()
        };
        skill.source = source.clone();
        skill.id = id.clone();

        let files: Vec<BundleFile> = skill
            .files
            .iter()
            .map(|f| BundleFile {
                path: f.path.clone(),
                content: f.content.clone(),
            })
            .collect();
        let manifest = build_manifest(BundleSkill {
            id,
            source,
            name: skill.name.clone(),
            description: skill.description.clone(),
            content: skill.content.clone(),
            files,
        });
        skill.hash = manifest.hash.clone();
        skill.size_bytes = manifest.size_bytes;
        let by_path: std::collections::HashMap<&str, &crate::skill_bundle::FileRef> = manifest
            .files
            .iter()
            .map(|f| (f.path.as_str(), f))
            .collect();
        for file in &mut skill.files {
            if let Some(reference) = by_path.get(file.path.as_str()) {
                file.sha256 = reference.sha256.clone();
                file.size_bytes = reference.size_bytes;
            }
        }
        bundles.push(skill.clone());
        refs.push(AgentSkillRefData {
            id: skill.id.clone(),
            source: skill.source.clone(),
            name: skill.name.clone(),
            description: skill.description.clone(),
            hash: manifest.hash,
            size_bytes: manifest.size_bytes,
            file_count: manifest.file_count,
            files: manifest
                .files
                .iter()
                .map(|f| AgentSkillFileRefData {
                    path: f.path.clone(),
                    sha256: f.sha256.clone(),
                    size_bytes: f.size_bytes,
                })
                .collect(),
        });
    }
    (bundles, refs)
}

impl TaskService {
    /// Loads an agent's workspace skills with their files for task execution.
    pub async fn load_agent_skills(&self, agent_id: Uuid) -> Vec<AgentSkillData> {
        let Ok(skills) = list_agent_skills(&self.pool, agent_id).await else {
            return Vec::new();
        };
        if skills.is_empty() {
            return Vec::new();
        }
        let mut result = Vec::with_capacity(skills.len());
        for sk in skills {
            let mut data = AgentSkillData {
                id: sk.id.to_string(),
                source: String::new(),
                name: sk.name,
                description: sk.description,
                hash: String::new(),
                size_bytes: 0,
                content: sk.content,
                files: Vec::new(),
            };
            // Best-effort: a file read failure degrades to a content-only skill.
            if let Ok(files) = list_skill_files(&self.pool, sk.id).await {
                data.files = files
                    .into_iter()
                    .map(|f| crate::builtin_skills::AgentSkillFileData {
                        path: f.path,
                        content: f.content,
                        sha256: String::new(),
                        size_bytes: 0,
                    })
                    .collect();
            }
            result.push(data);
        }
        result
    }

    /// Every skill visible to an agent, including built-ins, with stable
    /// bundle hashes and lightweight refs for slim claims.
    pub async fn load_agent_skill_bundles(
        &self,
        agent_id: Uuid,
    ) -> (Vec<AgentSkillData>, Vec<AgentSkillRefData>) {
        let mut skills = self.load_agent_skills(agent_id).await;
        skills.extend(load_builtin_skills());
        build_agent_skill_bundles(skills)
    }

    /// Broadcasts a progress update via the event bus.
    pub fn report_progress(
        &self,
        task_id: &str,
        workspace_id: &str,
        summary: &str,
        step: i32,
        total: i32,
    ) {
        let payload = TaskProgressPayload {
            task_id: task_id.to_string(),
            summary: summary.to_string(),
            step,
            total,
        };
        self.bus.publish(&cordy_events::Event {
            event_type: cordy_protocol::EVENT_TASK_PROGRESS.to_string(),
            workspace_id: workspace_id.to_string(),
            actor_type: "system".to_string(),
            actor_id: String::new(),
            payload: serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null),
            task_id: task_id.to_string(),
            chat_session_id: String::new(),
        });
    }

    /// Handler-facing agent status write (Go updateAgentStatus); the HTTP
    /// layer consumes it from S8 onward.
    pub async fn update_agent_status(&self, agent_id: Uuid, status: &str) {
        let Ok(Some(agent)) = update_agent_status_query(&self.pool, agent_id, status).await else {
            return;
        };
        self.publish_agent_status(&agent).await;
    }

    /// For chat tasks: broadcast chat:done AFTER commit. The single assistant
    /// outcome row (message or no_response) and any attachment binding were
    /// already persisted by write_chat_completion_outcome; unread is derived
    /// from the read cursor, so a no_response row counts like a text reply.
    pub(crate) async fn broadcast_chat_done(
        &self,
        task: &AgentTaskQueue,
        msg: Option<&ChatMessage>,
        quick_actions_pending: bool,
    ) {
        let Some(workspace_id) = self.resolve_task_workspace_id(task).await else {
            return;
        };
        let mut payload = ChatDonePayload {
            chat_session_id: task
                .chat_session_id
                .map(|u| u.to_string())
                .unwrap_or_default(),
            task_id: task.id.to_string(),
            message_id: String::new(),
            content: String::new(),
            elapsed_ms: 0,
            created_at: String::new(),
            message_kind: String::new(),
            quick_actions: Vec::new(),
            quick_actions_pending,
        };
        if let Some(msg) = msg {
            payload.message_id = msg.id.to_string();
            payload.content = msg.content.clone();
            payload.message_kind = msg.message_kind.clone();
            payload.quick_actions =
                serde_json::from_value(msg.quick_actions.clone()).unwrap_or_default();
            payload.created_at = cordy_util::rfc3339_nano(msg.created_at);
            payload.elapsed_ms = msg.elapsed_ms.unwrap_or(0);
        }
        self.bus.publish(&cordy_events::Event {
            event_type: cordy_protocol::EVENT_CHAT_DONE.to_string(),
            workspace_id,
            actor_type: "system".to_string(),
            actor_id: String::new(),
            payload: serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null),
            task_id: String::new(),
            chat_session_id: task
                .chat_session_id
                .map(|u| u.to_string())
                .unwrap_or_default(),
        });
    }

    /// Publishes the issue:updated event the frontend's realtime reconcile
    /// relies on to move an issue between status columns / status filters.
    /// `prev_status` is the issue's status before the write so the client can
    /// gate that reconcile on status_changed.
    ///
    /// The `issue` payload is a map (issue_to_map); note this does NOT cover
    /// the full HTTP UpdateIssue side effects: activity-log and inbox
    /// listeners type-assert a handler response and skip maps, so a background
    /// status reset intentionally emits neither (#4648 / PB-3782).
    pub(crate) async fn broadcast_issue_updated(&self, issue: &Issue, prev_status: &str) {
        let prefix = self.get_issue_prefix(issue.workspace_id).await;
        let category = issue_status::effective(&self.pool, issue.workspace_id, &issue.status).await;
        self.bus.publish(&cordy_events::Event {
            event_type: cordy_protocol::EVENT_ISSUE_UPDATED.to_string(),
            workspace_id: issue.workspace_id.to_string(),
            actor_type: "system".to_string(),
            actor_id: String::new(),
            payload: json!({
                "issue": issue_to_map_with_category(issue, &prefix, &category),
                "status_changed": prev_status != issue.status,
                "prev_status": prev_status,
            }),
            task_id: String::new(),
            chat_session_id: String::new(),
        });
    }

    async fn get_issue_prefix(&self, workspace_id: Uuid) -> String {
        match get_workspace(&self.pool, workspace_id).await {
            Ok(Some(ws)) => ws.issue_prefix,
            _ => String::new(),
        }
    }

    /// Creates an agent-authored comment on an issue with the full comment:
    ///created side effects: deferred escalation cancellation, WS broadcast,
    /// and resolved-thread auto-unresolve. Best-effort end to end — every
    /// failure logs and returns without surfacing.
    pub(crate) async fn create_agent_comment(
        &self,
        issue_id: Uuid,
        agent_id: Uuid,
        content: &str,
        comment_type: &str,
        parent_id: Option<Uuid>,
        source_task_id: Option<Uuid>,
    ) {
        if content.is_empty() {
            return;
        }
        // Issue lookup provides the workspace id for broadcasting; failure is
        // silently swallowed in Go too (best-effort synthesis path).
        let Ok(Some(issue)) = get_issue(&self.pool, issue_id).await else {
            return;
        };
        // Resolve the thread root for thread-level side effects without
        // overwriting parent_id. The stored parent_id must remain the exact
        // comment being replied to; recursive thread reads recover the root.
        let root_comment = match parent_id {
            Some(pid) => get_thread_root(&self.pool, pid, issue.workspace_id)
                .await
                .ok()
                .flatten(),
            None => None,
        };
        let Some(created) = create_comment(
            &self.pool,
            issue_id,
            issue.workspace_id,
            "agent",
            agent_id,
            content,
            comment_type,
            parent_id,
            source_task_id,
            None,
            None,
            new_v7(),
        )
        .await
        .ok()
        .flatten() else {
            return;
        };
        let comment = Comment {
            author_id: created.author_id.unwrap_or_else(Uuid::nil),
            author_type: created.author_type.clone(),
            content: created.content.clone(),
            created_at: created.created_at.expect("inserted created_at"),
            id: created.id.expect("inserted id"),
            issue_id: created.issue_id.expect("inserted issue_id"),
            parent_id: created.parent_id,
            quick_action_id: created.quick_action_id,
            resolved_at: created.resolved_at,
            resolved_by_id: created.resolved_by_id,
            resolved_by_type: created.resolved_by_type.clone(),
            revision: created.revision,
            source_task_id: created.source_task_id,
            type_: created.type_.clone(),
            updated_at: created.updated_at.expect("inserted updated_at"),
            via_plugin_id: created.via_plugin_id,
            workspace_id: issue.workspace_id,
        };

        cancel_deferred_escalations_for_issue_agent(&self.pool, issue_id, agent_id)
            .await
            .ok();

        self.bus.publish(&cordy_events::Event {
            event_type: cordy_protocol::EVENT_COMMENT_CREATED.to_string(),
            workspace_id: issue.workspace_id.to_string(),
            actor_type: "agent".to_string(),
            actor_id: agent_id.to_string(),
            payload: json!({
                "comment": {
                    "id": comment.id.to_string(),
                    "issue_id": comment.issue_id.to_string(),
                    "author_type": comment.author_type,
                    "author_id": comment.author_id.to_string(),
                    "content": comment.content,
                    "type": comment.type_,
                    "parent_id": comment.parent_id.map(|p| p.to_string()),
                    "source_task_id": comment.source_task_id.map(|s| s.to_string()),
                    "created_at": comment.created_at
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    "revision": comment.revision,
                },
                "issue_title": issue.title,
                "issue_status": issue.status,
                "issue_revision": created.issue_revision,
            }),
            task_id: String::new(),
            chat_session_id: String::new(),
        });

        self.auto_unresolve_thread_on_reply(
            root_comment.as_ref(),
            &issue.workspace_id.to_string(),
            "agent",
            &agent_id.to_string(),
        )
        .await;
    }

    /// Clears resolved_at on the thread root when a reply lands in a resolved
    /// thread, and broadcasts comment:unresolved. Shared between the user-
    /// facing handler create-comment path and the agent-facing
    /// create_agent_comment path so the resolved-then-replied state can never
    /// desync. Errors are logged — the reply itself already committed, the
    /// desync is recoverable on next read.
    pub async fn auto_unresolve_thread_on_reply(
        &self,
        parent: Option<&Comment>,
        workspace_id: &str,
        actor_type: &str,
        actor_id: &str,
    ) {
        let Some(parent) = parent else { return };
        if parent.resolved_at.is_none() {
            return;
        }
        let updated = match unresolve_comment(&self.pool, parent.id).await {
            Ok(Some(updated)) => updated,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(error = %err, comment_id = %parent.id,
                    "auto-unresolve on reply failed");
                return;
            }
        };
        self.bus.publish(&cordy_events::Event {
            event_type: cordy_protocol::EVENT_COMMENT_UNRESOLVED.to_string(),
            workspace_id: workspace_id.to_string(),
            actor_type: actor_type.to_string(),
            actor_id: actor_id.to_string(),
            payload: json!({
                "comment": {
                    "id": updated.id.to_string(),
                    "issue_id": updated.issue_id.to_string(),
                    "author_type": updated.author_type,
                    "author_id": updated.author_id.to_string(),
                    "content": updated.content,
                    "type": updated.type_,
                    "parent_id": updated.parent_id.map(|p| p.to_string()),
                    "created_at": rfc3339(updated.created_at),
                    "updated_at": rfc3339(updated.updated_at),
                    "resolved_at": updated.resolved_at.map(rfc3339),
                    "resolved_by_type": updated.resolved_by_type.clone(),
                    "resolved_by_id": updated.resolved_by_id.map(|u| u.to_string()),
                    "revision": updated.revision,
                },
            }),
            task_id: String::new(),
            chat_session_id: String::new(),
        });
    }
}

/// A status's category when it can be known without a catalog read — i.e. for
/// the 7 built-ins, where key == category.
pub fn built_in_status_category(status: &str) -> &'static str {
    if issue_status::is_built_in(status) {
        // Safe: every built-in key is a 'static literal round-tripped here.
        Box::leak(status.to_string().into_boxed_str())
    } else {
        ""
    }
}

/// Renders an issue row as the map shape the issue:created / issue:updated
/// broadcast payloads carry under their "issue" key. Single source of truth
/// wherever the event is published from outside the HTTP handler. The map
/// must stay key-compatible with handler.IssueResponse — clients type both as
/// a complete Issue and insert straight into the list cache.
pub fn issue_to_map(issue: &Issue, issue_prefix: &str) -> serde_json::Value {
    json!({
        "id": issue.id.to_string(),
        "workspace_id": issue.workspace_id.to_string(),
        "number": issue.number,
        "identifier": crate::plugin_action::issue_identifier(issue_prefix, issue.number),
        "title": issue.title,
        "description": issue.description,
        "status": issue.status,
        // Mirrors handler.IssueResponse.StatusCategory: a built-in status IS
        // its own category; empty for a custom status, which consumers
        // resolve via the catalog. (PB-6243)
        "status_category": built_in_status_category(&issue.status),
        "priority": issue.priority,
        "assignee_type": issue.assignee_type.clone(),
        "assignee_id": issue.assignee_id.map(|u| u.to_string()),
        "creator_type": issue.creator_type,
        "creator_id": issue.creator_id.to_string(),
        "parent_issue_id": issue.parent_issue_id.map(|u| u.to_string()),
        "project_id": issue.project_id.map(|u| u.to_string()),
        "position": issue.position,
        "stage": issue.stage,
        "start_date": date_ptr(issue.start_date),
        "due_date": date_ptr(issue.due_date),
        "created_at": rfc3339(issue.created_at),
        "updated_at": rfc3339(issue.updated_at),
        "last_activity_at": issue.last_activity_at.map(cordy_util::rfc3339_nano),
        "revision": issue.revision,
        "metadata": json_object_or_empty(&issue.metadata),
        "properties": json_object_or_empty(&issue.properties),
    })
}

/// issue_to_map with an AUTHORITATIVE status_category resolved through the
/// catalog so a custom status is not emitted blank. Background events go
/// through this; clients bucket by category. (PB-6243)
pub fn issue_to_map_with_category(
    issue: &Issue,
    issue_prefix: &str,
    effective_category: &str,
) -> serde_json::Value {
    let mut m = issue_to_map(issue, issue_prefix);
    m["status_category"] = json!(effective_category);
    m
}

// --- Quick-create inbox outcomes -------------------------------------------

const MAX_QUICK_CREATE_FAILURE_DETAIL_RUNES: usize = 2000;

const QUICK_CREATE_OVERSIZED_FAILURE_DETAIL: &str = "Quick create failed, but the agent's output was too large to show the reason safely. Check the task's execution log for details.";

const INBOX_TYPE_QUICK_CREATE_FAILED: &str = "quick_create_failed";
const INBOX_TYPE_QUICK_CREATE_UNCONFIRMED: &str = "quick_create_unconfirmed";

const QUICK_CREATE_UNCONFIRMED_DETAIL: &str = "Couldn't confirm whether the issue was created. Check your recent issues before retrying — creating it again may produce a duplicate.";

const QUICK_CREATE_NOTIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Extracts a user-facing failure reason from a quick-create task's final
/// output. The quick-create prompt instructs the agent to exit with the CLI
/// error as its only output when `cordy issue create` fails, so this normally
/// carries the real reason. Returns "" when there is no usable output so the
/// caller falls back to a generic message; redaction is applied by the
/// notify path.
fn quick_create_failure_detail(result: &serde_json::Value) -> String {
    use cordy_protocol::messages::TaskCompletedPayload;
    let Ok(payload) = serde_json::from_value::<TaskCompletedPayload>(result.clone()) else {
        return String::new();
    };
    // Same unescape as the comment-fallback path: literal `\n` sequences from
    // agent stdout become real newlines before the reason reaches the user.
    let body = cordy_util::unescape_backslash_escapes(&payload.output)
        .trim()
        .to_string();
    if body.is_empty() {
        return String::new();
    }
    if body.chars().count() > MAX_QUICK_CREATE_FAILURE_DETAIL_RUNES {
        return QUICK_CREATE_OVERSIZED_FAILURE_DETAIL.to_string();
    }
    body
}

impl TaskService {
    /// Writes a success inbox notification to the requester pointing at the
    /// issue the agent just created. The issue is stamped with
    /// origin_type=quick_create + origin_id=<task_id>, so the lookup is
    /// deterministic — robust against parallel issues from the same agent.
    pub(crate) async fn notify_quick_create_completed(
        &self,
        task: &AgentTaskQueue,
        qc: &QuickCreateContext,
        result: &serde_json::Value,
    ) {
        let Ok(requester_id) = Uuid::parse_str(&qc.requester_id) else {
            tracing::warn!(task_id = %task.id, "quick-create completion: invalid requester id");
            return;
        };
        let Ok(workspace_id) = Uuid::parse_str(&qc.workspace_id) else {
            tracing::warn!(task_id = %task.id, "quick-create completion: invalid workspace id");
            return;
        };
        let issue = match get_issue_by_origin(
            &self.pool,
            workspace_id,
            Some("quick_create"),
            task.id,
        )
        .await
        {
            Err(err) => {
                // The lookup itself failed, not a confirmed "no issue": the
                // agent may well have created it, so a failure inbox would
                // misreport. But nothing retries this reconciliation — write
                // a neutral terminal notification instead.
                tracing::error!(
                    task_id = %task.id,
                    error = %err,
                    "quick-create completion: issue lookup failed, writing unconfirmed inbox"
                );
                self.notify_quick_create_unconfirmed(task, qc).await;
                return;
            }
            Ok(None) => {
                // No issue created — the CLI create call must have failed
                // (most often the active-duplicate guard). Prefer the CLI
                // error text over a generic string (#5885).
                let detail = quick_create_failure_detail(result);
                tracing::warn!(
                    task_id = %task.id,
                    has_detail = !detail.is_empty(),
                    "quick-create completion: no issue found, writing failure inbox"
                );
                self.notify_quick_create_failed(task, qc, &detail).await;
                return;
            }
            Ok(Some(issue)) => issue,
        };

        // Link the new issue back to this task so subsequent reads render it
        // as a normal direct issue task instead of the "Creating issue" label.
        // Best-effort: a write failure doesn't block the inbox notification.
        if let Err(err) = link_task_to_issue(&self.pool, task.id, issue.id).await {
            tracing::warn!(
                task_id = %task.id,
                issue_id = %issue.id,
                error = %err,
                "quick-create completion: link task→issue failed"
            );
        }

        // Requester subscription happens at issue-creation time in the shared
        // delegated-subscriber rule, NOT here (PB-5483 keeps one owner).
        let prefix = self.get_issue_prefix(workspace_id).await;
        let identifier = format!("{}-{}", prefix, issue.number);
        let details = json!({
            "task_id": task.id.to_string(),
            "agent_id": task.agent_id.to_string(),
            "issue_id": issue.id.to_string(),
            "identifier": identifier,
            "original_prompt": qc.prompt,
        });
        let item = match create_inbox_item(
            &self.pool,
            workspace_id,
            "member",
            requester_id,
            "quick_create_done",
            "info",
            Some(issue.id),
            &issue.title,
            None,
            Some("agent"),
            task.agent_id,
            &details,
            new_v7(),
        )
        .await
        {
            Ok(Some(item)) => item,
            Ok(None) => return,
            Err(err) => {
                tracing::error!(
                    task_id = %task.id,
                    error = %err,
                    "quick-create completion: inbox write failed"
                );
                return;
            }
        };
        self.publish_quick_create_inbox(
            &item,
            &qc.workspace_id,
            &task.agent_id.to_string(),
            &issue.status,
        );
    }

    /// Failure inbox carrying the original prompt + agent ID so the frontend
    /// can render an "Edit as advanced form" entry pre-filling the legacy
    /// modal. Only for KNOWN failures; unverifiable outcomes use unconfirmed.
    pub(crate) async fn notify_quick_create_failed(
        &self,
        task: &AgentTaskQueue,
        qc: &QuickCreateContext,
        err_msg: &str,
    ) {
        let err_msg = if err_msg.is_empty() {
            "Quick create did not finish successfully"
        } else {
            err_msg
        };
        self.write_quick_create_outcome_inbox(
            task,
            qc,
            INBOX_TYPE_QUICK_CREATE_FAILED,
            "Quick create failed",
            err_msg,
        )
        .await;
    }

    /// NEUTRAL terminal notification for a run whose outcome could not be
    /// verified. It uses its own inbox type rather than reusing
    /// quick_create_failed: every client renders the failed type with a
    /// "Failed:" prefix, which would assert a failure we never observed.
    pub(crate) async fn notify_quick_create_unconfirmed(
        &self,
        task: &AgentTaskQueue,
        qc: &QuickCreateContext,
    ) {
        self.write_quick_create_outcome_inbox(
            task,
            qc,
            INBOX_TYPE_QUICK_CREATE_UNCONFIRMED,
            "Quick create needs a check",
            QUICK_CREATE_UNCONFIRMED_DETAIL,
        )
        .await;
    }

    /// Shared inbox row behind both non-success outcomes. Callers own the
    /// wording and the row type. Bounds the detached write so a wedged pool
    /// cannot pin the completion path (the task already committed).
    async fn write_quick_create_outcome_inbox(
        &self,
        task: &AgentTaskQueue,
        qc: &QuickCreateContext,
        inbox_type: &str,
        title: &str,
        err_msg: &str,
    ) {
        let Ok(requester_id) = Uuid::parse_str(&qc.requester_id) else {
            return;
        };
        let Ok(workspace_id) = Uuid::parse_str(&qc.workspace_id) else {
            return;
        };
        let details = json!({
            "task_id": task.id.to_string(),
            "agent_id": task.agent_id.to_string(),
            "original_prompt": qc.prompt,
            "error": redact::text(err_msg),
        });
        let item = tokio::time::timeout(
            QUICK_CREATE_NOTIFY_TIMEOUT,
            create_inbox_item(
                &self.pool,
                workspace_id,
                "member",
                requester_id,
                inbox_type,
                "action_required",
                None,
                title,
                Some(&redact::text(err_msg)),
                Some("agent"),
                task.agent_id,
                &details,
                new_v7(),
            ),
        )
        .await;
        let item = match item {
            Ok(Ok(Some(item))) => item,
            Ok(Ok(None)) => return,
            Ok(Err(err)) => {
                tracing::error!(
                    task_id = %task.id,
                    error = %err,
                    "quick-create failure: inbox write failed"
                );
                return;
            }
            Err(_) => {
                tracing::error!(
                    task_id = %task.id,
                    "quick-create failure: inbox write timed out"
                );
                return;
            }
        };
        self.publish_quick_create_inbox(&item, &qc.workspace_id, &task.agent_id.to_string(), "");
    }

    /// Emits the WS event so the requester's inbox list updates immediately.
    /// Mirrors the payload shape used by the other inbox listeners.
    pub(crate) fn publish_quick_create_inbox(
        &self,
        item: &cordy_db::models::InboxItem,
        workspace_id: &str,
        agent_id: &str,
        issue_status: &str,
    ) {
        let resp = json!({
            "id": item.id.to_string(),
            "workspace_id": item.workspace_id.to_string(),
            "recipient_type": item.recipient_type,
            "recipient_id": item.recipient_id.to_string(),
            "type": item.type_,
            "severity": item.severity,
            "issue_id": item.issue_id.map(|i| i.to_string()),
            "title": item.title,
            "body": item.body.clone(),
            "read": item.read,
            "archived": item.archived,
            "created_at": rfc3339(item.created_at),
            "actor_type": item.actor_type.clone(),
            "actor_id": item.actor_id.map(|a| a.to_string()),
            "details": item.details.clone(),
            "issue_status": issue_status,
        });
        self.bus.publish(&cordy_events::Event {
            event_type: cordy_protocol::EVENT_INBOX_NEW.to_string(),
            workspace_id: workspace_id.to_string(),
            actor_type: "agent".to_string(),
            actor_id: agent_id.to_string(),
            payload: json!({ "item": resp }),
            task_id: String::new(),
            chat_session_id: String::new(),
        });
    }
}

/// Simple map for broadcasting agent status updates (Go agentToMap L6686).
pub fn agent_to_map(a: &cordy_db::models::Agent) -> serde_json::Value {
    json!({
        "id": a.id.to_string(),
        "workspace_id": a.workspace_id.to_string(),
        "runtime_id": a.runtime_id.map(|u| u.to_string()).unwrap_or_default(),
        "name": a.name,
        "description": a.description,
        "avatar_url": a.avatar_url.clone(),
        "runtime_mode": a.runtime_mode,
        "runtime_config": a.runtime_config,
        "visibility": a.visibility,
        "status": a.status,
        "max_concurrent_tasks": a.max_concurrent_tasks,
        "owner_id": a.owner_id.map(|u| u.to_string()),
        "skills": [],
        "created_at": rfc3339(a.created_at),
        "updated_at": rfc3339(a.updated_at),
        "archived_at": a.archived_at.map(rfc3339),
        "archived_by": a.archived_by.map(|u| u.to_string()),
    })
}

// --- Format helpers ---------------------------------------------------------

/// Go time.RFC3339 (seconds precision, Z suffix for UTC instants).
pub(crate) fn rfc3339(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Go util.DateToPtr equivalent (DateOnly rendering, null when absent).
fn date_ptr(d: Option<chrono::NaiveDate>) -> Option<String> {
    d.map(|d| d.format("%Y-%m-%d").to_string())
}

/// Go util.JSONObjectOrEmpty: object passthrough, everything else becomes {}.
fn json_object_or_empty(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(_) => v.clone(),
        _ => serde_json::Value::Object(Default::default()),
    }
}
