//! TaskService core — port of `server/internal/service/task.go` Slices 1–2
//! (lines 1–2186): the service struct, attribution resolution, analytics
//! context cache, metrics capture helpers, and the enqueue family.
//!
//! Later slices add cancel/claim (Slice3) and terminal transitions + retry
//! (Slice4).

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use cordy_analytics as analytics;
use cordy_db::dbid::new_v7;
use cordy_db::models::{Agent, AgentTaskQueue, ChatMessage, ChatSession, Comment, Issue};
use cordy_db::queries::agent::{
    cancel_agent_task, cancel_agent_task_by_user, cancel_agent_task_with_reason,
    cancel_agent_tasks_by_agent, cancel_agent_tasks_by_issue,
    cancel_agent_tasks_by_trigger_comment, cancel_deferred_escalations_for_issue_agent,
    cancel_deferred_escalations_for_task, cancel_queued_agent_task,
    cancel_queued_agent_tasks_for_session, cancel_superseded_deferred_retries_for_runtimes,
    claim_agent_task, claim_chat_finalize_deferred, count_running_tasks, create_agent_task,
    create_deferred_agent_task, create_deferred_channel_issue_task, create_quick_create_task,
    extend_agent_task_prepare_lease, get_agent, get_agent_for_claim_update, get_agent_task,
    list_queued_claim_candidates_by_runtime, list_queued_claim_candidates_by_runtimes,
    mark_agent_task_waiting_local_directory, mark_chat_finalize_deferred,
    promote_deferred_channel_issue_task, promote_due_deferred_tasks_for_runtime,
    promote_due_deferred_tasks_for_runtimes, reclaim_stale_dispatched_task_for_runtime,
    reclaim_stale_dispatched_tasks_for_runtimes, refresh_agent_status_from_tasks,
    requeue_agent_task_after_claim_failure, set_deferred_channel_issue_task_runtime_overlay,
    set_task_delivered_comment_i_ds, start_agent_task,
};
use cordy_db::queries::attachment::detach_attachments_from_user_chat_message_by_task;
use cordy_db::queries::attachment::link_attachments_to_chat_message;
use cordy_db::queries::autopilot::{
    get_active_autopilot_rule_version, get_autopilot, get_autopilot_run,
    get_autopilot_run_by_issue, get_autopilot_trigger,
};
use cordy_db::queries::channel::{
    clear_channel_chat_session_pending_fresh, lock_channel_chat_session_pending_fresh,
};
use cordy_db::queries::chat::{
    adopt_orphan_onboarding_kickoff, advance_cancelled_chat_session_pointer,
    chat_session_has_user_message, create_chat_draft_restore, create_chat_message,
    create_chat_task, create_mika_onboarding_opening, defer_chat_task_for_sealed_pending_media,
    delete_user_chat_message_by_task, get_channel_media_pending_until, get_chat_session,
    get_latest_assistant_chat_message_for_session, has_active_chat_task_for_session,
    has_pending_chat_turn_for_session, link_unowned_channel_chat_messages_to_task,
    lock_chat_session_for_delete, lock_chat_session_for_enqueue,
    lock_chat_session_for_runtime_bind, lock_chat_session_for_task,
    promote_channel_chat_tasks_if_media_ready, reanchor_claimed_direct_chat_input,
    reanchor_next_queued_direct_chat_input, release_onboarding_kickoff_from_task,
    set_chat_task_input_owner_self, task_has_channel_ingested_messages, touch_chat_session,
};
use cordy_db::queries::comment::get_comment_in_workspace;
use cordy_db::queries::daemon_token::{create_daemon_token, delete_expired_daemon_tokens};
use cordy_db::queries::github::get_issue_review_head_sha;
use cordy_db::queries::issue::get_issue;
use cordy_db::queries::runtime::get_agent_runtime;
use cordy_db::queries::task_message::list_task_messages;
use cordy_db::queries::task_token::{create_task_token, delete_task_tokens_by_task};
use cordy_db::queries::workspace::get_workspace_attribution_fail_closed;

use crate::attribution::{
    self, classify_comment, classify_direct, direct_human_run, evidence_chat, owner_fallback,
    rule_owner, trigger_owner, CommentFacts, DirectFacts, EvidenceKind,
    Result_ as AttributionResult,
};
use crate::feature_flags::{composio_mcp_apps_enabled, FlagSource};
use crate::task_helpers::{compute_chat_elapsed_ms, priority_to_int, truncate_for_summary};

/// Cap for the trigger-comment snapshot stored on the task row: enough for a
/// recognisable preview of a one-paragraph comment.
pub const TRIGGER_SUMMARY_MAX_LEN: usize = 200;

/// Maximum DB heartbeat age accepted by every task release path (deferred
/// promotion, stale-dispatch reclaim, fresh claim). Must exceed the 60s DB
/// heartbeat flush interval, one ~15s daemon heartbeat, and the ~30s batch
/// scheduler tick; 150s leaves a 45s buffer above that 105s worst-case age.
pub const RUNTIME_CLAIM_FRESHNESS_SECONDS: f64 = 150.0;

/// Must exceed daemon client.Timeout for /tasks/claim (30s) plus
/// /tasks/{id}/start (30s) plus scheduling slack. Longer pre-start work is
/// protected by [`PREPARE_LEASE_DURATION`] instead of stretching this window.
pub const CLAIM_RESPONSE_RECOVERY_WINDOW: Duration = Duration::from_secs(90);

/// Lease granted to a claimed task that has not started yet.
pub const PREPARE_LEASE_DURATION: Duration = Duration::from_secs(45);

const TASK_ANALYTICS_CONTEXT_CACHE_MAX: usize = 4096;

/// Signals that a run resolved to no precise accountable human and the enqueue
/// is REFUSED rather than started (MUL-4302 §1/§3.5).
#[derive(Debug, thiserror::Error)]
#[error("attribution: no precise accountable human and enqueue refused (fail-closed policy, policy read failed, or no agent owner)")]
pub struct ErrAttributionFailClosed;

/// A fresh enqueue lost the race to a concurrent one (#5914). Benign — a
/// sibling run already covers this target. Returned bare so no upper-layer
/// log or response can leak the constraint name.
#[derive(Debug, thiserror::Error)]
#[error("a pending task for this issue and agent already exists")]
pub struct ErrDuplicatePendingTask;

/// Reports whether err is the pending-task unique-index violation (a
/// concurrent enqueue won the race). Accept both index names while v1 and v2
/// coexist during a rolling deploy.
pub fn is_duplicate_pending_task_err(err: &sqlx::Error) -> bool {
    let Some(db_err) = err.as_database_error() else {
        return false;
    };
    if db_err.code().as_deref() != Some("23505") {
        return false;
    }
    matches!(
        db_err.constraint(),
        Some("idx_one_pending_task_per_issue_agent")
            | Some("idx_one_pending_task_per_issue_agent_v2")
    )
}

/// Extracts the sqlx error from the anyhow error the generated query layer
/// returns, so unique-violation classification keeps working through the
/// wrapper.
pub(crate) fn downcast_sqlx(err: anyhow::Error) -> sqlx::Error {
    err.downcast::<sqlx::Error>()
        .unwrap_or(sqlx::Error::RowNotFound)
}

fn is_duplicate_pending_task_anyhow(err: &anyhow::Error) -> bool {
    err.downcast_ref::<sqlx::Error>()
        .map(is_duplicate_pending_task_err)
        .unwrap_or(false)
}

/// Reports whether err means "the (issue, agent) pending slot was already
/// occupied when we tried to enqueue" — either shape that reaches RerunIssue:
/// the raw unique violation or the normalized sentinel.
pub fn pending_slot_taken_err(err: &TaskServiceError) -> bool {
    match err {
        TaskServiceError::DuplicatePendingTask(_) => true,
        TaskServiceError::Sql(e) => is_duplicate_pending_task_err(e),
        _ => false,
    }
}

/// Error surface shared by the task service paths ported so far.
#[derive(Debug, thiserror::Error)]
pub enum TaskServiceError {
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
    #[error("load agent: {0}")]
    LoadAgent(sqlx::Error),
    #[error("issue has no assignee")]
    NoAssignee,
    #[error("agent is archived")]
    AgentArchived,
    #[error("agent has no runtime")]
    AgentNoRuntime,
    #[error("chat task: agent archived")]
    ChatAgentArchived,
    #[error("chat task: agent has no runtime")]
    ChatAgentNoRuntime,
    #[error("chat task: session archived")]
    ChatSessionArchived,
    #[error("chat session already has a user message")]
    ChatSessionAlreadyStarted,
    #[error("chat quick actions: no assistant turn to regenerate")]
    ChatQuickActionsNoTurn,
    #[error("chat quick actions: llm layer not configured")]
    ChatQuickActionsUnavailable,
    #[error("chat quick actions: refresh target is stale")]
    ChatQuickActionsStale,
    #[error("chat quick actions: session busy")]
    ChatQuickActionsBusy,
    #[error("task is no longer queued")]
    NoLongerQueued(ErrTaskNoLongerQueued),
    #[error("rerun: operator not allowed to invoke target agent")]
    RerunInvokeNotAllowed(ErrRerunInvokeNotAllowed),
    #[error("{0}")]
    Internal(String),
    #[error(transparent)]
    DuplicatePendingTask(#[from] ErrDuplicatePendingTask),
    #[error("{0}: workspace policy unavailable")]
    FailClosedPolicyUnavailable(ErrAttributionFailClosed),
    #[error("{0}: policy read failed: {1}")]
    FailClosedPolicyRead(ErrAttributionFailClosed, String),
    #[error("{0}")]
    FailClosed(ErrAttributionFailClosed),
    #[error("{0}: no agent owner to attribute")]
    FailClosedNoOwner(ErrAttributionFailClosed),
}

/// Seam for building the per-task Composio MCP overlay at enqueue time.
///
/// Contract: `None` means "no overlay for this run". Any overlay value is the
/// exact JSON to store in `agent_task_queue.runtime_mcp_overlay`; connected
/// apps are non-secret metadata stored alongside it. An error is surfaced but
/// treated as best-effort — failed overlay computation must not fail the
/// enqueue.
#[async_trait::async_trait]
pub trait ComposioOverlayBuilder: Send + Sync {
    async fn build_task_overlay(
        &self,
        pool: &PgPool,
        originator_user_id: Uuid,
        agent: &Agent,
    ) -> anyhow::Result<crate::runtime_apps::McpOverlayResult>;
}

/// Wakeup seam used by dispatch to nudge runtimes.
pub trait TaskWakeupNotifier: Send + Sync {
    fn notify_task_available(&self, runtime_id: &str, task_id: &str);
}

/// Quick-create task context stored in `agent_task_queue.context`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct QuickCreateContext {
    #[serde(rename = "type")]
    pub type_: String,
    pub prompt: String,
    pub requester_id: String,
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub priority: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub due_date: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub squad_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_ids: Vec<String>,
    /// Optional parent issue UUID ("Add sub issue" flow); preserved across
    /// the manual→agent mode flip via `--parent <uuid>`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub parent_issue_id: String,
}

/// Marks a task as a quick-create job.
pub const QUICK_CREATE_CONTEXT_TYPE: &str = "quick_create";

/// The optional per-task Composio MCP overlay payload.
#[derive(Debug, Clone, Default)]
pub struct RuntimeMcpOverlayData {
    pub overlay: Option<serde_json::Value>,
    pub connected_apps: Option<serde_json::Value>,
}

// Go-shaped LLM seam lives with the rest of the quick-actions port; re-exported
// here because the TaskService field is wired through this module's namespace.
pub use crate::chat_quick_actions::ChatQuickActionsLlm;

/// The task domain service. Field usage mirrors Go's TaskService; the dead
/// Hub field from Go is omitted (task.go only ever publishes through Bus).
pub struct TaskService {
    pub pool: PgPool,
    pub bus: Arc<cordy_events::Bus>,
    pub analytics: Option<Box<dyn analytics::AnalyticsClient>>,
    pub metrics: Option<Arc<cordy_metrics::BusinessMetrics>>,
    pub wakeup: Option<std::sync::Weak<dyn TaskWakeupNotifier>>,
    /// Server-side toggle router. `None` returns each call site's default.
    pub feature_flags: Option<Box<dyn FlagSource>>,
    /// Optional per-task MCP overlay builder; `None` makes the overlay step a
    /// no-op (deployments without Composio behave exactly as before).
    pub composio: Option<std::sync::Arc<dyn ComposioOverlayBuilder>>,
    /// Optional follow-up suggestion generator; unset disables the feature.
    quick_actions: OnceLock<Arc<dyn ChatQuickActionsLlm>>,

    /// chat session id -> admitted; one suggestion pass per session plus a
    /// process-wide ceiling. Zero values are usable.
    pub(crate) quick_actions_in_flight: Mutex<HashMap<Uuid, ()>>,
    pub(crate) quick_actions_running: AtomicI64,

    /// LRU-ish analytics context cache keyed by task identity columns.
    analytics_context: Mutex<AnalyticsContextCache>,
}

/// Everything the two issue-task INSERT shapes need, resolved by
/// prepare_issue_enqueue.
struct PreparedIssueEnqueue {
    assignee_id: Uuid,
    runtime_id: Uuid,
    originator_user_id: Option<Uuid>,
    accountable_user_id: Option<Uuid>,
    rule_version_id: Option<Uuid>,
    overlay: RuntimeMcpOverlayData,
    attr_source: Option<String>,
    attr_delegated_from: Option<Uuid>,
    attr_evidence_kind: Option<String>,
    attr_evidence_ref: Option<Uuid>,
    trigger_summary: Option<String>,
    head_sha: String,
}

#[derive(Default)]
struct AnalyticsContextCache {
    map: HashMap<String, analytics::TaskContext>,
    order: Vec<String>,
}

impl TaskService {
    pub fn new(pool: PgPool, bus: Arc<cordy_events::Bus>) -> Self {
        Self {
            pool,
            bus,
            analytics: None,
            metrics: None,
            wakeup: None,
            feature_flags: None,
            composio: None,
            quick_actions: OnceLock::new(),
            quick_actions_in_flight: Mutex::new(HashMap::new()),
            quick_actions_running: AtomicI64::new(0),
            analytics_context: Mutex::new(AnalyticsContextCache::default()),
        }
    }

    pub fn install_quick_actions(&self, quick_actions: Arc<dyn ChatQuickActionsLlm>) -> bool {
        self.quick_actions.set(quick_actions).is_ok()
    }

    pub fn quick_actions(&self) -> Option<&Arc<dyn ChatQuickActionsLlm>> {
        self.quick_actions.get()
    }

    // --- Trigger summary ---------------------------------------------------

    /// Fetches the comment content and truncates it for storage on the task
    /// row. Returns `None` when the comment is missing so the column stays
    /// NULL — front-end falls back to a structural label in that case.
    ///
    /// workspaceID scopes the fetch to the task's own workspace: the summary
    /// is later returned in claim / task-history responses, so a foreign
    /// comment UUID must NOT leak another workspace's text even truncated
    /// (MUL-4252).
    pub async fn build_comment_trigger_summary(
        &self,
        workspace_id: Uuid,
        comment_id: Option<Uuid>,
    ) -> anyhow::Result<Option<String>> {
        let Some(comment_id) = comment_id else {
            return Ok(None);
        };
        let Some(comment) = get_comment_in_workspace(&self.pool, comment_id, workspace_id).await?
        else {
            return Ok(None);
        };
        let summary = truncate_for_summary(&comment.content, TRIGGER_SUMMARY_MAX_LEN);
        if summary.is_empty() {
            return Ok(None);
        }
        Ok(Some(summary))
    }

    // --- Attribution ---------------------------------------------------------

    /// Top-of-chain HUMAN user id for a comment-triggered Enqueue* path
    /// (MUL-3869 chain rules). Missing comment / unknown source task / NULL
    /// parent originator → None.
    pub async fn resolve_originator_from_trigger_comment(
        &self,
        workspace_id: Uuid,
        comment_id: Option<Uuid>,
    ) -> anyhow::Result<Option<Uuid>> {
        Ok(self
            .attribution_from_trigger_comment(
                workspace_id,
                comment_id,
                attribution::Source::comment_source(),
            )
            .await
            .user_id)
    }

    /// FULL attribution snapshot for a comment being coalesced into an
    /// already-queued task (MUL-4302). Re-runs the fail-closed gate the fresh
    /// enqueue faced; see [`Self::apply_attribution_fallback`].
    pub async fn attribution_for_merged_comment(
        &self,
        workspace_id: Uuid,
        comment_id: Option<Uuid>,
        is_mention: bool,
        agent: &Agent,
    ) -> Result<AttributionResult, TaskServiceError> {
        let agent_authored_source = if is_mention {
            attribution::Source::delegation()
        } else {
            attribution::Source::comment_source()
        };
        let attr = self
            .attribution_from_trigger_comment(workspace_id, comment_id, agent_authored_source)
            .await;
        self.apply_attribution_fallback(attr, agent).await
    }

    /// Recomputes the Composio MCP overlay + connected-app metadata for
    /// (originatorUserID, agent) when a merge re-stamps a coalesced task's
    /// originator (MUL-4195 review must-fix #1). Fails soft to empty.
    pub async fn build_runtime_mcp_overlay_for_merge(
        &self,
        originator_user_id: Uuid,
        agent: &Agent,
    ) -> (Option<serde_json::Value>, Option<serde_json::Value>) {
        let data = self
            .build_runtime_mcp_overlay(originator_user_id, agent)
            .await;
        (data.overlay, data.connected_apps)
    }

    async fn attribution_from_trigger_comment(
        &self,
        workspace_id: Uuid,
        comment_id: Option<Uuid>,
        agent_authored_source: attribution::Source,
    ) -> AttributionResult {
        let Some(comment_id) = comment_id else {
            return AttributionResult {
                source: Some(attribution::Source::unattributed()),
                ..Default::default()
            };
        };
        let Ok(Some(comment)) =
            get_comment_in_workspace(&self.pool, comment_id, workspace_id).await
        else {
            return AttributionResult {
                source: Some(attribution::Source::unattributed()),
                ..Default::default()
            };
        };
        self.attribution_from_comment(&comment, agent_authored_source)
            .await
    }

    /// Classifies a run from an already-loaded trigger comment so a caller
    /// holding the row does not re-read it.
    async fn attribution_from_comment(
        &self,
        comment: &Comment,
        agent_authored_source: attribution::Source,
    ) -> AttributionResult {
        let mut facts = CommentFacts {
            comment_id: Some(comment.id),
            author_type: comment.author_type.clone(),
            author_id: Some(comment.author_id),
            ..Default::default()
        };
        // For an agent-authored comment, walk source_task_id → parent task →
        // parent.originator_user_id (set by every agent comment-write path
        // since migration 120). NULL/missing source task leaves
        // ParentOriginator unset → ClassifyComment maps to unattributed.
        if comment.author_type == "agent" {
            if let Some(source_task_id) = comment.source_task_id {
                facts.source_task_id = Some(source_task_id);
                if let Ok(Some(parent)) = get_agent_task(&self.pool, source_task_id).await {
                    facts.parent_originator = parent.originator_user_id;
                    facts.parent_accountable = parent.accountable_user_id;
                }
            }
        }
        classify_comment(facts, agent_authored_source)
    }

    /// Top-of-chain human for issue-backed dispatches: comment-triggered runs
    /// keep comment-chain semantics; direct assignment/creation falls back to
    /// the issue's member creator.
    pub async fn resolve_originator_for_issue_task(
        &self,
        issue: &Issue,
        trigger_comment_id: Option<Uuid>,
    ) -> anyhow::Result<Option<Uuid>> {
        Ok(self
            .attribution_for_issue_task(
                issue,
                trigger_comment_id,
                attribution::Source::comment_source(),
                None,
            )
            .await
            .user_id)
    }

    /// Full attribution for an issue-backed enqueue. actor_user_id (a direct
    /// member action) wins ahead of any trigger comment or origin (MUL-4302
    /// §4/§5) — a manual rerun may inherit a triggerCommentID for prompt
    /// context but must attribute to the member who clicked rerun.
    pub async fn attribution_for_issue_task(
        &self,
        issue: &Issue,
        trigger_comment_id: Option<Uuid>,
        agent_authored_source: attribution::Source,
        actor_user_id: Option<Uuid>,
    ) -> AttributionResult {
        if let Some(actor) = actor_user_id {
            return classify_direct(DirectFacts {
                issue_id: Some(issue.id),
                actor_user_id: Some(actor),
                ..Default::default()
            });
        }
        if let Some(trigger_comment_id) = trigger_comment_id {
            // workspace-scoped so a foreign comment UUID cannot resolve a
            // human from another tenant (MUL-4252).
            let comment =
                get_comment_in_workspace(&self.pool, trigger_comment_id, issue.workspace_id)
                    .await
                    .ok()
                    .flatten();
            let Some(comment) = comment else {
                return AttributionResult {
                    source: Some(attribution::Source::unattributed()),
                    ..Default::default()
                };
            };
            // A SYSTEM-authored comment (Stage-completion child-done cascade)
            // carries no human and is not part of any delegation chain; skip
            // to the issue's own provenance below instead of degrading to
            // owner_fallback (MUL-4302; Bohan stage-cascade fallback).
            if comment.author_type != "system" {
                return self
                    .attribution_from_comment(&comment, agent_authored_source)
                    .await;
            }
        }
        // Autopilot-origin issues from schedule/webhook triggers: no human
        // authorized the run → originator stays NULL, accountable goes to the
        // human responsible for the firing trigger's effective config —
        // trigger_owner (MUL-4302; Elon must-fix), degrading to rule_owner.
        if issue.origin_type.as_deref() == Some("autopilot") {
            if let Some(origin_id) = issue.origin_id {
                let trigger_id = match get_autopilot_run_by_issue(&self.pool, issue.id).await {
                    Ok(Some(run)) => run.trigger_id,
                    _ => None,
                };
                return trigger_owner_attribution(
                    &self.pool,
                    trigger_id,
                    issue.workspace_id,
                    origin_id,
                    attribution::evidence_issue_assignment(),
                    Some(issue.id),
                )
                .await;
            }
        }
        let mut facts = DirectFacts {
            issue_id: Some(issue.id),
            creator_type: issue.creator_type.clone(),
            creator_id: Some(issue.creator_id),
            ..Default::default()
        };
        // Member-created issues resolve without a DB read. Only origin-linked
        // agent-created issues (quick_create, agent_create) load the origin
        // task to inherit its human (MUL-4305).
        if issue.creator_type != "member"
            && matches!(
                issue.origin_type.as_deref(),
                Some("quick_create") | Some("agent_create")
            )
        {
            if let Some(origin_id) = issue.origin_id {
                facts.origin_type = issue.origin_type.clone().unwrap_or_default();
                facts.origin_task_id = Some(origin_id);
                if let Ok(Some(task)) = get_agent_task(&self.pool, origin_id).await {
                    facts.origin_originator = task.originator_user_id;
                    facts.origin_accountable = task.accountable_user_id;
                }
            }
        }
        classify_direct(facts)
    }

    /// Applies the workspace's degraded-attribution policy to an unattributed
    /// result. PRECISE results pass through untouched (no policy read at all).
    /// For unattributed runs the accountable-never-null guarantee is enforced
    /// fail-closed: policy read failure or fail-closed workspace REFUSES;
    /// otherwise owner_fallback, refusing again when there is no valid owner.
    pub async fn apply_attribution_fallback(
        &self,
        attr: AttributionResult,
        agent: &Agent,
    ) -> Result<AttributionResult, TaskServiceError> {
        if attr.source.as_ref().map(|s| s.as_str()) != Some("unattributed") {
            return Ok(attr);
        }
        let fail_closed = get_workspace_attribution_fail_closed(&self.pool, agent.workspace_id)
            .await
            .map_err(|e| {
                TaskServiceError::FailClosedPolicyRead(ErrAttributionFailClosed, e.to_string())
            })?;
        match fail_closed {
            // Row missing (no workspace) or explicitly fail-closed → refuse.
            None | Some(true) => {
                return Err(TaskServiceError::FailClosed(ErrAttributionFailClosed));
            }
            Some(false) => {}
        }
        let fallback = owner_fallback(attr, agent.owner_id);
        if fallback.source.as_ref().map(|s| s.as_str()) == Some("unattributed") {
            return Err(TaskServiceError::FailClosedNoOwner(
                ErrAttributionFailClosed,
            ));
        }
        Ok(fallback)
    }

    // --- Composio overlay ----------------------------------------------------

    /// Computes the optional per-task Composio MCP overlay. Enqueue paths call
    /// this BEFORE inserting the queued row so the daemon cannot claim a task
    /// during the network round-trip and miss the overlay.
    pub(crate) async fn build_runtime_mcp_overlay(
        &self,
        originator_user_id: Uuid,
        agent: &Agent,
    ) -> RuntimeMcpOverlayData {
        let Some(composio) = &self.composio else {
            return RuntimeMcpOverlayData::default();
        };
        let enabled = match &self.feature_flags {
            Some(f) => composio_mcp_apps_enabled(f.as_ref()),
            None => false,
        };
        if !enabled {
            return RuntimeMcpOverlayData::default();
        }
        match composio
            .build_task_overlay(&self.pool, originator_user_id, agent)
            .await
        {
            Err(err) => {
                tracing::warn!(
                    %originator_user_id,
                    agent_id = %agent.id,
                    error = %err,
                    "runtime mcp overlay: BuildTaskOverlay failed; task will run without composio overlay"
                );
                RuntimeMcpOverlayData::default()
            }
            Ok(result) => {
                if result.mcp_overlay.is_none()
                    || result.mcp_overlay.as_ref() == Some(&serde_json::Value::Null)
                {
                    tracing::debug!(
                        %originator_user_id,
                        agent_id = %agent.id,
                        "runtime mcp overlay: no composio overlay for task"
                    );
                    return RuntimeMcpOverlayData::default();
                }
                RuntimeMcpOverlayData {
                    overlay: result.mcp_overlay,
                    connected_apps: if result.connected_apps.is_empty() {
                        None
                    } else {
                        serde_json::to_value(result.connected_apps).ok()
                    },
                }
            }
        }
    }

    // --- Quick-create context -------------------------------------------------

    /// Parses the quick-create context out of a task's context JSONB. Only
    /// tasks with no issue/chat/autopilot link can be quick-create jobs.
    pub fn parse_quick_create_context(task: &AgentTaskQueue) -> Option<QuickCreateContext> {
        if task.issue_id.is_some()
            || task.chat_session_id.is_some()
            || task.autopilot_run_id.is_some()
        {
            return None;
        }
        let raw = task.context.as_ref()?;
        let qc: QuickCreateContext = serde_json::from_value(raw.clone()).ok()?;
        if qc.type_ != QUICK_CREATE_CONTEXT_TYPE {
            return None;
        }
        Some(qc)
    }

    // --- Analytics context cache ----------------------------------------------

    fn cached_task_analytics_context(
        &self,
        task: &AgentTaskQueue,
    ) -> Option<analytics::TaskContext> {
        let key = task_analytics_context_key(task)?;
        let cache = self
            .analytics_context
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        cache.map.get(&key).cloned()
    }

    fn store_task_analytics_context(&self, task: &AgentTaskQueue, tc: &analytics::TaskContext) {
        if tc.workspace_id.is_empty() {
            return;
        }
        let Some(key) = task_analytics_context_key(task) else {
            return;
        };
        let mut cache = self
            .analytics_context
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !cache.map.contains_key(&key) {
            cache.order.push(key.clone());
            if cache.order.len() > TASK_ANALYTICS_CONTEXT_CACHE_MAX {
                let oldest = cache.order.remove(0);
                cache.map.remove(&oldest);
            }
        }
        cache.map.insert(key, tc.clone());
    }

    /// Derives the (source, runtime_mode, provider) triple driving the task
    /// lifecycle counters.
    pub async fn task_metrics_context(&self, task: &AgentTaskQueue) -> (String, String, String) {
        let tc = self.task_analytics_context(task).await;
        let source: &str = if task.chat_session_id.is_some() {
            "chat"
        } else if task.issue_id.is_some() {
            if tc.source == analytics::SOURCE_AUTOPILOT {
                "autopilot_issue"
            } else {
                "issue"
            }
        } else if task.autopilot_run_id.is_some() {
            "autopilot"
        } else if Self::parse_quick_create_context(task).is_some() {
            "quick_create"
        } else if !tc.source.is_empty() {
            &tc.source
        } else {
            "other"
        };
        (
            source.to_string(),
            tc.runtime_mode.clone(),
            tc.provider.clone(),
        )
    }

    /// Resolves the analytics join/segmentation fields for a task, caching the
    /// result per task identity. Mirrors Go taskAnalyticsContext exactly.
    pub async fn task_analytics_context(&self, task: &AgentTaskQueue) -> analytics::TaskContext {
        if let Some(tc) = self.cached_task_analytics_context(task) {
            return tc;
        }
        let mut tc = analytics::TaskContext {
            agent_id: task.agent_id.to_string(),
            task_id: task.id.to_string(),
            source: analytics::SOURCE_MANUAL.to_string(),
            ..Default::default()
        };
        if let Some(issue_id) = task.issue_id {
            tc.issue_id = issue_id.to_string();
        }
        if let Some(chat_session_id) = task.chat_session_id {
            tc.chat_session_id = chat_session_id.to_string();
            tc.source = analytics::SOURCE_CHAT.to_string();
        }
        if let Some(autopilot_run_id) = task.autopilot_run_id {
            tc.autopilot_run_id = autopilot_run_id.to_string();
            tc.source = analytics::SOURCE_AUTOPILOT.to_string();
        }

        if let Some(runtime_id) = task.runtime_id {
            if let Ok(Some(rt)) = get_agent_runtime(&self.pool, runtime_id).await {
                tc.workspace_id = rt.workspace_id.to_string();
                tc.runtime_mode = rt.runtime_mode.clone();
                tc.provider = rt.provider.clone();
            }
        }
        if tc.workspace_id.is_empty() || tc.runtime_mode.is_empty() {
            if let Ok(Some(agent)) = get_agent(&self.pool, task.agent_id).await {
                if tc.workspace_id.is_empty() {
                    tc.workspace_id = agent.workspace_id.to_string();
                }
                if tc.runtime_mode.is_empty() {
                    tc.runtime_mode = agent.runtime_mode.clone();
                }
            }
        }

        if let Some(issue_id) = task.issue_id {
            if let Ok(Some(issue)) = get_issue(&self.pool, issue_id).await {
                tc.workspace_id = issue.workspace_id.to_string();
                if issue.creator_type == "member" {
                    tc.user_id = issue.creator_id.to_string();
                }
                match issue.origin_type.as_deref() {
                    Some("autopilot") => {
                        tc.source = analytics::SOURCE_AUTOPILOT.to_string();
                        if let Some(origin_id) = issue.origin_id {
                            if let Ok(Some(ap)) = get_autopilot(&self.pool, origin_id).await {
                                if ap.created_by_type == "member" {
                                    tc.user_id = ap.created_by_id.to_string();
                                }
                            }
                        }
                    }
                    Some("quick_create") => {
                        tc.source = analytics::SOURCE_MANUAL.to_string();
                    }
                    _ => {}
                }
            }
        }
        if let Some(chat_session_id) = task.chat_session_id {
            if let Ok(Some(cs)) = get_chat_session(&self.pool, chat_session_id).await {
                tc.workspace_id = cs.workspace_id.to_string();
                tc.user_id = cs.creator_id.to_string();
            }
        }
        if let Some(autopilot_run_id) = task.autopilot_run_id {
            if let Ok(Some(run)) = get_autopilot_run(&self.pool, autopilot_run_id).await {
                if let Ok(Some(ap)) = get_autopilot(&self.pool, run.autopilot_id).await {
                    tc.workspace_id = ap.workspace_id.to_string();
                    if ap.created_by_type == "member" {
                        tc.user_id = ap.created_by_id.to_string();
                    }
                }
            }
        }
        if let Some(qc) = Self::parse_quick_create_context(task) {
            tc.workspace_id = qc.workspace_id;
            tc.user_id = qc.requester_id;
            tc.source = analytics::SOURCE_MANUAL.to_string();
        }
        self.store_task_analytics_context(task, &tc);
        tc
    }

    // --- Metrics capture helpers -----------------------------------------------

    pub async fn capture_task_queued(&self, task: &AgentTaskQueue) {
        if let Some(metrics) = &self.metrics {
            let (source, runtime_mode, _) = self.task_metrics_context(task).await;
            metrics.record_task_enqueued(&source, &runtime_mode);
        }
    }

    pub async fn capture_task_dispatched(&self, task: &AgentTaskQueue) {
        if let Some(metrics) = &self.metrics {
            let (source, runtime_mode, _) = self.task_metrics_context(task).await;
            metrics.record_task_dispatched(
                &task.id.to_string(),
                &source,
                &runtime_mode,
                task_queue_wait_seconds(task),
            );
        }
    }

    pub async fn capture_task_started(&self, task: &AgentTaskQueue) {
        if let Some(metrics) = &self.metrics {
            let (source, runtime_mode, provider) = self.task_metrics_context(task).await;
            metrics.record_task_started(&source, &runtime_mode, &provider);
        }
    }

    pub async fn capture_task_completed(&self, task: &AgentTaskQueue) {
        if let Some(metrics) = &self.metrics {
            let (source, runtime_mode, _) = self.task_metrics_context(task).await;
            metrics.record_task_terminal(
                &task.id.to_string(),
                &source,
                &runtime_mode,
                &task.status,
                task_run_seconds(task),
                task_total_seconds(task),
                task.attempt,
            );
        }
    }

    pub async fn capture_task_failed(&self, task: &AgentTaskQueue) {
        let failure_reason = task_failure_reason(task);
        if let Some(metrics) = &self.metrics {
            let (source, runtime_mode, _) = self.task_metrics_context(task).await;
            metrics.record_task_terminal(
                &task.id.to_string(),
                &source,
                &runtime_mode,
                &task.status,
                task_run_seconds(task),
                task_total_seconds(task),
                task.attempt,
            );
            metrics.record_task_failed(&source, &runtime_mode, &failure_reason);
        }
    }

    /// Terminal-cancelled capture plus eager revocation of any mat_ task
    /// tokens minted for this task (MUL-2600): cancellation is terminal, so
    /// deleting the token closes the window where a compromised process could
    /// keep authenticating until the 24h expiry. Failure is non-fatal.
    pub async fn capture_task_cancelled(&self, task: &AgentTaskQueue) {
        if let Some(metrics) = &self.metrics {
            let (source, runtime_mode, _) = self.task_metrics_context(task).await;
            metrics.record_task_terminal(
                &task.id.to_string(),
                &source,
                &runtime_mode,
                &task.status,
                task_run_seconds(task),
                task_total_seconds(task),
                task.attempt,
            );
        }
        if let Err(err) = delete_task_tokens_by_task(&self.pool, task.id).await {
            tracing::warn!(
                task_id = %task.id,
                error = %err,
                "cancel task: failed to revoke task tokens"
            );
        }
    }

    /// cost_usd_ticks is the provider's own price for this usage in 1e-10 USD,
    /// or 0 when it reported none — the metrics layer prefers it over its rate
    /// table.
    #[allow(clippy::too_many_arguments)]
    pub async fn capture_task_usage(
        &self,
        task: &AgentTaskQueue,
        provider: &str,
        model: &str,
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cache_write_tokens: i64,
        cost_usd_ticks: i64,
    ) {
        let Some(metrics) = &self.metrics else {
            return;
        };
        let (source, runtime_mode, _) = self.task_metrics_context(task).await;
        metrics.record_llm_usage(
            &source,
            &runtime_mode,
            provider,
            model,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            cost_usd_ticks,
        );
    }

    pub async fn capture_queued_expired_tasks(&self, tasks: &[AgentTaskQueue]) {
        let Some(metrics) = &self.metrics else {
            return;
        };
        for task in tasks {
            let (source, runtime_mode, _) = self.task_metrics_context(task).await;
            metrics.record_task_queued_expired(&source, &runtime_mode);
        }
    }

    pub async fn capture_lease_expired_tasks(&self, tasks: &[AgentTaskQueue]) {
        let Some(metrics) = &self.metrics else {
            return;
        };
        for task in tasks {
            let (source, _, _) = self.task_metrics_context(task).await;
            metrics.record_task_lease_expired(&source);
        }
    }

    /// Admits one quick-actions suggestion pass for a session, enforcing both
    /// the one-per-session gate and the process-wide ceiling. Returns false
    /// when the pass is shed.
    pub fn try_admit_quick_actions_pass(&self, session_id: Uuid, ceiling: i64) -> bool {
        let mut in_flight = self
            .quick_actions_in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if in_flight.contains_key(&session_id) {
            return false;
        }
        if self.quick_actions_running.load(Ordering::SeqCst) >= ceiling {
            return false;
        }
        in_flight.insert(session_id, ());
        self.quick_actions_running.fetch_add(1, Ordering::SeqCst);
        true
    }

    /// Releases a previously admitted quick-actions pass.
    pub fn release_quick_actions_pass(&self, session_id: Uuid) {
        let mut in_flight = self
            .quick_actions_in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if in_flight.remove(&session_id).is_some() {
            self.quick_actions_running.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

fn task_analytics_context_key(task: &AgentTaskQueue) -> Option<String> {
    let task_id = task.id;
    Some(format!(
        "{task_id}|{}|{}|{}|{}",
        opt_uuid(&task.runtime_id),
        opt_uuid(&task.issue_id),
        opt_uuid(&task.chat_session_id),
        opt_uuid(&task.autopilot_run_id),
    ))
}

fn opt_uuid(v: &Option<Uuid>) -> String {
    v.map(|u| u.to_string()).unwrap_or_default()
}

// --- Pure helpers ------------------------------------------------------------

pub fn task_queue_wait_seconds(task: &AgentTaskQueue) -> f64 {
    duration_seconds(Some(task.created_at), task.dispatched_at)
}

pub fn task_run_seconds(task: &AgentTaskQueue) -> f64 {
    duration_seconds(task.started_at, task.completed_at)
}

pub fn task_total_seconds(task: &AgentTaskQueue) -> f64 {
    duration_seconds(Some(task.created_at), task.completed_at)
}

pub fn duration_seconds(
    start: Option<chrono::DateTime<chrono::Utc>>,
    end: Option<chrono::DateTime<chrono::Utc>>,
) -> f64 {
    let (Some(start), Some(end)) = (start, end) else {
        return -1.0;
    };
    let seconds = (end - start).num_microseconds().unwrap_or(0) as f64 / 1_000_000.0;
    if seconds < 0.0 {
        return 0.0;
    }
    seconds
}

pub fn task_failure_reason(task: &AgentTaskQueue) -> String {
    match &task.failure_reason {
        Some(r) if !r.is_empty() => r.clone(),
        _ => "agent_error".to_string(),
    }
}

pub fn task_error_type(reason: &str) -> &'static str {
    match reason {
        "runtime_offline" | "runtime_recovery" => "runtime",
        "timeout" | "codex_semantic_inactivity" => "timeout",
        "iteration_limit" | "agent_fallback_message" => "agent_output",
        "cancelled" | "user_cancelled" => "cancelled",
        _ => "agent_error",
    }
}

// --- Free-function attribution helpers (DB-backed) ----------------------------

/// Resolves the rule_owner attribution for an autopilot run from its active
/// rule version snapshot (MUL-4302 §3.4). Shared by both autopilot execution
/// modes. Never errors: attribution must not fail an enqueue.
pub async fn rule_owner_attribution(
    pool: &PgPool,
    workspace_id: Uuid,
    autopilot_id: Uuid,
    evidence_kind: EvidenceKind,
    evidence_ref_id: Option<Uuid>,
) -> AttributionResult {
    let Ok(Some(ver)) = get_active_autopilot_rule_version(pool, workspace_id, autopilot_id).await
    else {
        return rule_owner(None, None, evidence_kind, evidence_ref_id);
    };
    let publisher = if ver.published_by_type == "member" {
        ver.published_by_id
    } else {
        None
    };
    rule_owner(publisher, Some(ver.id), evidence_kind, evidence_ref_id)
}

/// Resolves an autopilot schedule/webhook run to the human currently
/// RESPONSIBLE for the firing trigger's effective config (MUL-4302; Bohan +
/// Elon must-fix). published_by starts at the creator and transfers to whoever
/// later substantively edits it. Degrades to rule_owner when unrecoverable.
pub async fn trigger_owner_attribution(
    pool: &PgPool,
    trigger_id: Option<Uuid>,
    workspace_id: Uuid,
    autopilot_id: Uuid,
    evidence_kind: EvidenceKind,
    evidence_ref_id: Option<Uuid>,
) -> AttributionResult {
    if let Some(trigger_id) = trigger_id {
        if let Ok(Some(trig)) = get_autopilot_trigger(pool, trigger_id).await {
            if trig.published_by_type.as_deref() == Some("member") {
                if let Some(published_by) = trig.published_by_id {
                    return trigger_owner(Some(published_by), evidence_kind, evidence_ref_id);
                }
            }
        }
    }
    rule_owner_attribution(
        pool,
        workspace_id,
        autopilot_id,
        evidence_kind,
        evidence_ref_id,
    )
    .await
}

// --- Event publishing (Go lines ~5932–6127, needed by the enqueue family) ----

impl TaskService {
    /// captureTaskQueued + notifyTaskAvailable: bump the runtime's
    /// empty-claim invalidation version BEFORE the wakeup so a wakeup-driven
    /// claim cannot read a still-current "empty" verdict.
    pub async fn notify_task_enqueued(&self, task: &AgentTaskQueue) {
        self.capture_task_queued(task).await;
        self.notify_runtime_may_have_work(task.runtime_id, Some(&task.id.to_string()));
    }

    /// Best-effort daemon wakeup after a terminal state. The task ID is
    /// deliberately omitted: the completed task is not available; the hint
    /// only means a queued successor may have become claimable.
    pub fn notify_task_finished(&self, task: &AgentTaskQueue) {
        self.notify_runtime_may_have_work(task.runtime_id, None);
    }

    /// Batch form used by bulk terminal transitions; coalesces by runtime.
    pub fn notify_tasks_finished(&self, tasks: &[AgentTaskQueue]) {
        let mut seen = std::collections::HashSet::new();
        for task in tasks {
            let Some(runtime_id) = task.runtime_id else {
                continue;
            };
            if !seen.insert(runtime_id) {
                continue;
            }
            self.notify_runtime_may_have_work(Some(runtime_id), None);
        }
    }

    fn notify_runtime_may_have_work(&self, runtime_id: Option<Uuid>, task_id: Option<&str>) {
        // EmptyClaim cache bump goes here once the Redis-backed cache lands;
        // the wakeup alone still unblocks the daemon's next poll.
        if let Some(wakeup) = self.wakeup.as_ref().and_then(|w| w.upgrade()) {
            if let Some(runtime_id) = runtime_id {
                wakeup.notify_task_available(&runtime_id.to_string(), task_id.unwrap_or(""));
            }
        }
    }

    /// Resolves the workspace for a task event: issue → chat session →
    /// autopilot run → quick-create context. `None` = not found.
    pub async fn resolve_task_workspace_id(&self, task: &AgentTaskQueue) -> Option<String> {
        if let Some(issue_id) = task.issue_id {
            if let Ok(Some(issue)) = get_issue(&self.pool, issue_id).await {
                return Some(issue.workspace_id.to_string());
            }
        }
        if let Some(chat_session_id) = task.chat_session_id {
            if let Ok(Some(cs)) = get_chat_session(&self.pool, chat_session_id).await {
                return Some(cs.workspace_id.to_string());
            }
        }
        if let Some(autopilot_run_id) = task.autopilot_run_id {
            if let Ok(Some(run)) = get_autopilot_run(&self.pool, autopilot_run_id).await {
                if let Ok(Some(ap)) = get_autopilot(&self.pool, run.autopilot_id).await {
                    return Some(ap.workspace_id.to_string());
                }
            }
        }
        Self::parse_quick_create_context(task).map(|qc| qc.workspace_id)
    }

    /// Shared task-lifecycle event contract. Scope hints ride the envelope so
    /// the realtime layer can route without decoding the payload.
    fn task_event(
        &self,
        event_type: &str,
        workspace_id: &str,
        task: &AgentTaskQueue,
        extra: serde_json::Map<String, serde_json::Value>,
    ) -> cordy_events::Event {
        let mut payload = serde_json::Map::new();
        payload.insert("task_id".into(), serde_json::json!(task.id.to_string()));
        payload.insert(
            "agent_id".into(),
            serde_json::json!(task.agent_id.to_string()),
        );
        payload.insert(
            "issue_id".into(),
            serde_json::json!(task.issue_id.map(|u| u.to_string()).unwrap_or_default()),
        );
        payload.insert("status".into(), serde_json::json!(task.status));
        let mut chat_session_id = String::new();
        if let Some(cs) = task.chat_session_id {
            chat_session_id = cs.to_string();
            payload.insert("chat_session_id".into(), serde_json::json!(chat_session_id));
        }
        for (k, v) in extra {
            payload.insert(k, v);
        }
        cordy_events::Event {
            event_type: event_type.to_string(),
            workspace_id: workspace_id.to_string(),
            actor_type: "system".to_string(),
            actor_id: String::new(),
            payload: serde_json::Value::Object(payload),
            task_id: task.id.to_string(),
            chat_session_id,
        }
    }

    async fn publish_task_event(
        &self,
        event_type: &str,
        workspace_id: &str,
        task: &AgentTaskQueue,
        extra: serde_json::Map<String, serde_json::Value>,
    ) {
        if workspace_id.is_empty() {
            return;
        }
        self.bus
            .publish(&self.task_event(event_type, workspace_id, task, extra));
    }

    /// Broadcasts a task event scoped to the task's resolved workspace.
    pub async fn broadcast_task_event(
        &self,
        event_type: &str,
        task: &AgentTaskQueue,
        extra: serde_json::Map<String, serde_json::Value>,
    ) {
        let Some(workspace_id) = self.resolve_task_workspace_id(task).await else {
            return;
        };
        self.publish_task_event(event_type, &workspace_id, task, extra)
            .await;
    }

    /// task:dispatch payload carries the context JSONB plus routing keys.
    pub async fn broadcast_task_dispatch(&self, task: &AgentTaskQueue) {
        let mut payload = match &task.context {
            Some(serde_json::Value::Object(map)) => map.clone(),
            _ => serde_json::Map::new(),
        };
        payload.insert("task_id".into(), serde_json::json!(task.id.to_string()));
        payload.insert(
            "runtime_id".into(),
            serde_json::json!(task.runtime_id.map(|u| u.to_string()).unwrap_or_default()),
        );
        payload.insert(
            "issue_id".into(),
            serde_json::json!(task.issue_id.map(|u| u.to_string()).unwrap_or_default()),
        );
        payload.insert(
            "agent_id".into(),
            serde_json::json!(task.agent_id.to_string()),
        );
        if let Some(cs) = task.chat_session_id {
            // Routing key the chat window uses to writethrough pendingTask →
            // status="running" the moment the daemon claims the task.
            payload.insert("chat_session_id".into(), serde_json::json!(cs.to_string()));
        }
        let Some(workspace_id) = self.resolve_task_workspace_id(task).await else {
            return;
        };
        self.bus.publish(&cordy_events::Event {
            event_type: cordy_protocol::EVENT_TASK_DISPATCH.to_string(),
            workspace_id,
            actor_type: "system".to_string(),
            actor_id: String::new(),
            payload: serde_json::Value::Object(payload),
            task_id: task.id.to_string(),
            chat_session_id: task
                .chat_session_id
                .map(|u| u.to_string())
                .unwrap_or_default(),
        });
    }

    /// Terminal failure context required by channel outbounds; error text is
    /// redacted and omitted while an automatic retry is pending.
    pub async fn broadcast_task_failed_event(
        &self,
        task: &AgentTaskQueue,
        err_msg: &str,
        failure_reason: &str,
        retry_pending: bool,
    ) {
        let mut fields = serde_json::Map::new();
        fields.insert("failure_reason".into(), serde_json::json!(failure_reason));
        fields.insert("retry_pending".into(), serde_json::json!(retry_pending));
        if !err_msg.is_empty() && !retry_pending {
            fields.insert(
                "error".into(),
                serde_json::json!(crate::redact::text(err_msg)),
            );
        }
        self.broadcast_task_event(cordy_protocol::EVENT_TASK_FAILED, task, fields)
            .await;
    }

    // --- Review SHA (TEN-356) -------------------------------------------------

    /// Head SHA of the commit under review for an issue, or "" when none.
    /// Fails soft — any DB error returns "" so a transient github-table hiccup
    /// can never over-dedup a review out of existence.
    pub async fn resolve_issue_review_sha(&self, issue_id: Uuid) -> String {
        match get_issue_review_head_sha(&self.pool, issue_id).await {
            Ok(Some(sha)) => sha,
            Ok(None) => String::new(),
            Err(err) => {
                tracing::warn!(issue_id = %issue_id, error = %err, "resolve issue review sha failed");
                String::new()
            }
        }
    }

    // --- Enqueue family ---------------------------------------------------------

    /// Creates a queued task for an agent-assigned issue.
    pub async fn enqueue_task_for_issue(
        &self,
        issue: &Issue,
        trigger_comment_id: Option<Uuid>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        self.enqueue_issue_task(issue, trigger_comment_id, false, "", None, None, None)
            .await
    }

    /// Persists the assigned task for a media-backed channel /issue turn
    /// without making it claimable yet. fireAt is a crash-safe fallback; the
    /// channel router promotes as soon as the attachment transaction settles.
    pub async fn enqueue_deferred_channel_issue_task(
        &self,
        issue: &Issue,
        fire_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        self.enqueue_issue_task(issue, None, false, "", None, None, Some(fire_at))
            .await
    }

    /// Fills the optional Composio overlay after the issue+task transaction
    /// commits. The conditional update refuses to overwrite a comment merge
    /// that won the post-commit race and already re-attributed the task.
    pub async fn hydrate_deferred_channel_issue_task_overlay(
        &self,
        task: &AgentTaskQueue,
    ) -> Result<(), TaskServiceError> {
        let Some(_composio) = &self.composio else {
            return Ok(());
        };
        let enabled = match &self.feature_flags {
            Some(f) => composio_mcp_apps_enabled(f.as_ref()),
            None => false,
        };
        if !enabled {
            return Ok(());
        }
        let agent = get_agent(&self.pool, task.agent_id)
            .await
            .map_err(|e| TaskServiceError::LoadAgent(downcast_sqlx(e)))?
            .ok_or(TaskServiceError::LoadAgent(sqlx::Error::RowNotFound))?;
        let originator = match task.originator_user_id {
            Some(u) => u,
            None => return Ok(()),
        };
        let overlay = self.build_runtime_mcp_overlay(originator, &agent).await;
        let Some(overlay_value) = overlay.overlay else {
            return Ok(());
        };
        let updated = set_deferred_channel_issue_task_runtime_overlay(
            &self.pool,
            &overlay_value,
            &overlay.connected_apps.unwrap_or(serde_json::Value::Null),
            task.id,
            originator,
        )
        .await
        .map_err(|e| TaskServiceError::Sql(downcast_sqlx(e)))?;
        if updated == 0 {
            tracing::debug!(
                task_id = %task.id,
                "deferred channel issue task overlay skipped: task plan changed"
            );
        }
        Ok(())
    }

    /// Assign/promote variant carrying a handoff note into the run's opening
    /// context (MUL-3375). actorUserID becomes the accountable human
    /// (MUL-4302 §4).
    pub async fn enqueue_task_for_issue_with_handoff(
        &self,
        issue: &Issue,
        handoff_note: &str,
        actor_user_id: Option<Uuid>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        self.enqueue_issue_task(issue, None, false, handoff_note, actor_user_id, None, None)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn enqueue_issue_task(
        &self,
        issue: &Issue,
        trigger_comment_id: Option<Uuid>,
        force_fresh_session: bool,
        handoff_note: &str,
        actor_user_id: Option<Uuid>,
        rerun_of_task_id: Option<Uuid>,
        fire_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        self.enqueue_issue_task_with_comment_plan(
            issue,
            trigger_comment_id,
            vec![],
            force_fresh_session,
            handoff_note,
            actor_user_id,
            rerun_of_task_id,
            fire_at,
        )
        .await
    }

    /// Attribution/guard/metadata phase shared by every issue-task enqueue
    /// shape (pool-backed or tx-scoped). Resolves everything the two INSERT
    /// variants need; performs no writes itself.
    ///
    /// build_overlay gates Composio overlay resolution: the tx-scoped
    /// deferred path keeps that network call out of the caller's transaction
    /// (Go's txService trick carries a nil Composio there) because the task
    /// cannot be claimed while deferred and the overlay hydrates post-commit.
    async fn prepare_issue_enqueue(
        &self,
        issue: &Issue,
        trigger_comment_id: Option<Uuid>,
        actor_user_id: Option<Uuid>,
        build_overlay: bool,
    ) -> Result<PreparedIssueEnqueue, TaskServiceError> {
        let Some(assignee_id) = issue.assignee_id else {
            tracing::error!(issue_id = %issue.id, "task enqueue failed: issue has no assignee");
            return Err(TaskServiceError::NoAssignee);
        };

        let agent = get_agent(&self.pool, assignee_id)
            .await
            .map_err(|e| TaskServiceError::LoadAgent(downcast_sqlx(e)))?
            .ok_or(TaskServiceError::LoadAgent(sqlx::Error::RowNotFound))?;
        if agent.archived_at.is_some() {
            tracing::debug!(issue_id = %issue.id, agent_id = %agent.id, "task enqueue skipped: agent is archived");
            return Err(TaskServiceError::AgentArchived);
        }
        let Some(runtime_id) = agent.runtime_id else {
            tracing::error!(issue_id = %issue.id, "task enqueue failed: agent has no runtime");
            return Err(TaskServiceError::AgentNoRuntime);
        };

        // Issue assignee reacting to an agent-authored comment is
        // comment_source (a delegation special case); member comment or direct
        // assignment is direct_human.
        let attr = self
            .attribution_for_issue_task(
                issue,
                trigger_comment_id,
                attribution::Source::comment_source(),
                actor_user_id,
            )
            .await;
        let attr = self.apply_attribution_fallback(attr, &agent).await.inspect_err(|_e| {
            tracing::warn!(issue_id = %issue.id, agent_id = %assignee_id, "task enqueue refused: attribution fail-closed");
        })?;
        let originator_user_id = attr.user_id;
        let runtime_mcp_overlay = match originator_user_id {
            Some(originator) if build_overlay => {
                self.build_runtime_mcp_overlay(originator, &agent).await
            }
            _ => RuntimeMcpOverlayData::default(),
        };
        let (attr_source, attr_delegated_from, attr_evidence_kind, attr_evidence_ref) =
            attribution_create_params(&attr);
        let trigger_summary = self
            .build_comment_trigger_summary(issue.workspace_id, trigger_comment_id)
            .await
            .unwrap_or(None);
        let head_sha = self.resolve_issue_review_sha(issue.id).await;

        Ok(PreparedIssueEnqueue {
            assignee_id,
            runtime_id,
            originator_user_id,
            accountable_user_id: attr.accountable_user_id,
            rule_version_id: attr.rule_version_id,
            overlay: runtime_mcp_overlay,
            attr_source,
            attr_delegated_from,
            attr_evidence_kind,
            attr_evidence_ref,
            trigger_summary,
            head_sha,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_issue_task_with_comment_plan(
        &self,
        issue: &Issue,
        trigger_comment_id: Option<Uuid>,
        coalesced_comment_ids: Vec<Uuid>,
        force_fresh_session: bool,
        handoff_note: &str,
        actor_user_id: Option<Uuid>,
        rerun_of_task_id: Option<Uuid>,
        fire_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        let prep = self
            .prepare_issue_enqueue(issue, trigger_comment_id, actor_user_id, true)
            .await?;

        let created = if fire_at.is_some() {
            create_deferred_channel_issue_task(
                &self.pool,
                prep.assignee_id,
                prep.runtime_id,
                issue.id,
                priority_to_int(&issue.priority),
                trigger_comment_id.unwrap_or_else(Uuid::nil),
                coalesced_comment_ids,
                prep.trigger_summary.as_deref(),
                Some(force_fresh_session),
                None,
                opt_str(handoff_note),
                Uuid::nil(),
                opt_str(&prep.head_sha),
                prep.originator_user_id.unwrap_or_else(Uuid::nil),
                prep.accountable_user_id.unwrap_or_else(Uuid::nil),
                &overlay_value_or_null(&prep.overlay.overlay),
                &overlay_value_or_null(&prep.overlay.connected_apps),
                prep.attr_source.as_deref(),
                prep.attr_delegated_from.unwrap_or_else(Uuid::nil),
                prep.rule_version_id.unwrap_or_else(Uuid::nil),
                rerun_of_task_id.unwrap_or_else(Uuid::nil),
                prep.attr_evidence_kind.as_deref(),
                prep.attr_evidence_ref.unwrap_or_else(Uuid::nil),
                fire_at,
                new_v7(),
            )
            .await
        } else {
            create_agent_task(
                &self.pool,
                prep.assignee_id,
                prep.runtime_id,
                issue.id,
                priority_to_int(&issue.priority),
                trigger_comment_id.unwrap_or_else(Uuid::nil),
                coalesced_comment_ids,
                prep.trigger_summary.as_deref(),
                Some(force_fresh_session),
                None,
                opt_str(handoff_note),
                Uuid::nil(),
                opt_str(&prep.head_sha),
                prep.originator_user_id.unwrap_or_else(Uuid::nil),
                prep.accountable_user_id.unwrap_or_else(Uuid::nil),
                &overlay_value_or_null(&prep.overlay.overlay),
                &overlay_value_or_null(&prep.overlay.connected_apps),
                prep.attr_source.as_deref(),
                prep.attr_delegated_from.unwrap_or_else(Uuid::nil),
                prep.rule_version_id.unwrap_or_else(Uuid::nil),
                rerun_of_task_id.unwrap_or_else(Uuid::nil),
                prep.attr_evidence_kind.as_deref(),
                prep.attr_evidence_ref.unwrap_or_else(Uuid::nil),
                new_v7(),
            )
            .await
        };
        let task = match created {
            Ok(Some(t)) => t,
            Ok(None) => return Err(TaskServiceError::AgentNoRuntime),
            Err(e) => {
                tracing::error!(issue_id = %issue.id, error = %e, "task enqueue failed");
                return Err(TaskServiceError::Sql(downcast_sqlx(e)));
            }
        };

        tracing::info!(
            task_id = %task.id,
            issue_id = %issue.id,
            agent_id = %prep.assignee_id,
            force_fresh_session,
            "task enqueued"
        );
        if fire_at.is_some() {
            return Ok(task);
        }
        // Order matters: broadcast first, notify daemon second — see Go
        // comment on observe-order correctness.
        self.broadcast_task_event(cordy_protocol::EVENT_TASK_QUEUED, &task, Default::default())
            .await;
        self.notify_task_enqueued(&task).await;
        Ok(task)
    }

    /// Tx-scoped twin used by IssueService::create so a media-gated channel
    /// issue commits atomically with its inert deferred task. Mirrors Go's
    /// `txService := &TaskService{Queries: q}` trick: identical guards and
    /// attribution run against the caller's transaction, while seams stay
    /// dark — the overlay hydrates post-commit (never hold DB locks across a
    /// network call) and deferred tasks return before any broadcast/notify
    /// tail.
    /// Consumed by IssueService::create's media-gated deferred-task path
    /// (issue_service.rs lands next).
    #[allow(dead_code)]
    pub(crate) async fn create_deferred_channel_issue_task_tx(
        &self,
        tx: &mut sqlx::PgConnection,
        issue: &Issue,
        fire_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        let assignee_id = issue.assignee_id.ok_or(TaskServiceError::NoAssignee)?;
        let agent = get_agent(&mut *tx, assignee_id)
            .await
            .map_err(|e| TaskServiceError::LoadAgent(downcast_sqlx(e)))?
            .ok_or(TaskServiceError::LoadAgent(sqlx::Error::RowNotFound))?;
        if agent.archived_at.is_some() {
            return Err(TaskServiceError::AgentArchived);
        }
        let runtime_id = agent.runtime_id.ok_or(TaskServiceError::AgentNoRuntime)?;

        // This path is entered only by a channel `/issue` create and has no
        // trigger comment or actor override. Resolve its direct provenance
        // against the caller's connection so the surrounding transaction
        // never waits for a second pool lease.
        let mut facts = DirectFacts {
            issue_id: Some(issue.id),
            creator_type: issue.creator_type.clone(),
            creator_id: Some(issue.creator_id),
            ..Default::default()
        };
        if issue.creator_type != "member"
            && matches!(
                issue.origin_type.as_deref(),
                Some("quick_create") | Some("agent_create")
            )
        {
            if let Some(origin_id) = issue.origin_id {
                facts.origin_type = issue.origin_type.clone().unwrap_or_default();
                facts.origin_task_id = Some(origin_id);
                if let Ok(Some(task)) = get_agent_task(&mut *tx, origin_id).await {
                    facts.origin_originator = task.originator_user_id;
                    facts.origin_accountable = task.accountable_user_id;
                }
            }
        }
        let mut attr = classify_direct(facts);
        if attr.source.as_ref().map(|source| source.as_str()) == Some("unattributed") {
            let fail_closed = get_workspace_attribution_fail_closed(&mut *tx, agent.workspace_id)
                .await
                .map_err(|e| {
                    TaskServiceError::FailClosedPolicyRead(ErrAttributionFailClosed, e.to_string())
                })?;
            if fail_closed != Some(false) {
                return Err(TaskServiceError::FailClosed(ErrAttributionFailClosed));
            }
            attr = owner_fallback(attr, agent.owner_id);
            if attr.source.as_ref().map(|source| source.as_str()) == Some("unattributed") {
                return Err(TaskServiceError::FailClosedNoOwner(
                    ErrAttributionFailClosed,
                ));
            }
        }
        let (attr_source, attr_delegated_from, attr_evidence_kind, attr_evidence_ref) =
            attribution_create_params(&attr);
        let head_sha = get_issue_review_head_sha(&mut *tx, issue.id)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        let prep = PreparedIssueEnqueue {
            assignee_id,
            runtime_id,
            originator_user_id: attr.user_id,
            accountable_user_id: attr.accountable_user_id,
            rule_version_id: attr.rule_version_id,
            overlay: RuntimeMcpOverlayData::default(),
            attr_source,
            attr_delegated_from,
            attr_evidence_kind,
            attr_evidence_ref,
            trigger_summary: None,
            head_sha,
        };

        let task = create_deferred_channel_issue_task(
            tx,
            prep.assignee_id,
            prep.runtime_id,
            issue.id,
            priority_to_int(&issue.priority),
            Uuid::nil(),
            vec![],
            prep.trigger_summary.as_deref(),
            Some(false),
            None,
            None,
            Uuid::nil(),
            opt_str(&prep.head_sha),
            prep.originator_user_id.unwrap_or_else(Uuid::nil),
            prep.accountable_user_id.unwrap_or_else(Uuid::nil),
            &overlay_value_or_null(&prep.overlay.overlay),
            &overlay_value_or_null(&prep.overlay.connected_apps),
            prep.attr_source.as_deref(),
            prep.attr_delegated_from.unwrap_or_else(Uuid::nil),
            prep.rule_version_id.unwrap_or_else(Uuid::nil),
            Uuid::nil(),
            prep.attr_evidence_kind.as_deref(),
            prep.attr_evidence_ref.unwrap_or_else(Uuid::nil),
            Some(fire_at),
            new_v7(),
        )
        .await
        .map_err(|e| TaskServiceError::Sql(downcast_sqlx(e)))?
        .ok_or(TaskServiceError::AgentNoRuntime)?;
        Ok(task)
    }

    /// Queued task for a mentioned agent on an issue (explicit agent ID).
    pub async fn enqueue_task_for_mention(
        &self,
        issue: &Issue,
        agent_id: Uuid,
        trigger_comment_id: Option<Uuid>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        self.enqueue_mention_task(
            issue,
            agent_id,
            trigger_comment_id,
            vec![],
            false,
            None,
            false,
            "",
            None,
            None,
        )
        .await
    }

    /// Queued task for the agent who authored the direct parent comment a
    /// member replied to.
    pub async fn enqueue_task_for_thread_parent(
        &self,
        issue: &Issue,
        agent_id: Uuid,
        trigger_comment_id: Option<Uuid>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        self.enqueue_mention_task(
            issue,
            agent_id,
            trigger_comment_id,
            vec![],
            false,
            None,
            false,
            "",
            None,
            None,
        )
        .await
    }

    /// Leader-role variant; carries is_leader_task=true plus squad_id so the
    /// daemon injects the squad briefing regardless of trigger path.
    pub async fn enqueue_task_for_squad_leader(
        &self,
        issue: &Issue,
        leader_id: Uuid,
        squad_id: Uuid,
        trigger_comment_id: Option<Uuid>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        self.enqueue_mention_task(
            issue,
            leader_id,
            trigger_comment_id,
            vec![],
            true,
            Some(squad_id),
            false,
            "",
            None,
            None,
        )
        .await
    }

    /// Assign/promote variant carrying a handoff note into the leader run
    /// (MUL-3375).
    pub async fn enqueue_task_for_squad_leader_with_handoff(
        &self,
        issue: &Issue,
        leader_id: Uuid,
        squad_id: Uuid,
        handoff_note: &str,
        actor_user_id: Option<Uuid>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        self.enqueue_mention_task(
            issue,
            leader_id,
            None,
            vec![],
            true,
            Some(squad_id),
            false,
            handoff_note,
            actor_user_id,
            None,
        )
        .await
    }

    /// Shared mention-family implementation. An explicit mention /
    /// thread-parent / squad-leader hop from an agent-authored comment is a
    /// delegation; a member mention is direct_human.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_mention_task(
        &self,
        issue: &Issue,
        agent_id: Uuid,
        trigger_comment_id: Option<Uuid>,
        coalesced_comment_ids: Vec<Uuid>,
        is_leader: bool,
        squad_id: Option<Uuid>,
        force_fresh_session: bool,
        handoff_note: &str,
        actor_user_id: Option<Uuid>,
        rerun_of_task_id: Option<Uuid>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        let agent = get_agent(&self.pool, agent_id)
            .await
            .map_err(|e| TaskServiceError::LoadAgent(downcast_sqlx(e)))?
            .ok_or(TaskServiceError::LoadAgent(sqlx::Error::RowNotFound))?;
        if agent.archived_at.is_some() {
            tracing::debug!(issue_id = %issue.id, agent_id = %agent_id, "mention task enqueue skipped: agent is archived");
            return Err(TaskServiceError::AgentArchived);
        }
        let Some(runtime_id) = agent.runtime_id else {
            tracing::error!(issue_id = %issue.id, agent_id = %agent_id, "mention task enqueue failed: agent has no runtime");
            return Err(TaskServiceError::AgentNoRuntime);
        };

        let attr = self
            .attribution_for_issue_task(
                issue,
                trigger_comment_id,
                attribution::Source::delegation(),
                actor_user_id,
            )
            .await;
        let attr = self.apply_attribution_fallback(attr, &agent).await.inspect_err(|_e| {
            tracing::warn!(issue_id = %issue.id, agent_id = %agent_id, "mention task enqueue refused: attribution fail-closed");
        })?;
        let originator_user_id = attr.user_id;
        let runtime_mcp_overlay = match originator_user_id {
            Some(originator) => self.build_runtime_mcp_overlay(originator, &agent).await,
            None => RuntimeMcpOverlayData::default(),
        };
        let (attr_source, attr_delegated_from, attr_evidence_kind, attr_evidence_ref) =
            attribution_create_params(&attr);
        let trigger_summary = self
            .build_comment_trigger_summary(issue.workspace_id, trigger_comment_id)
            .await
            .unwrap_or(None);
        let head_sha = self.resolve_issue_review_sha(issue.id).await;

        let created = create_agent_task(
            &self.pool,
            agent_id,
            runtime_id,
            issue.id,
            priority_to_int(&issue.priority),
            trigger_comment_id.unwrap_or_else(Uuid::nil),
            coalesced_comment_ids,
            trigger_summary.as_deref(),
            Some(force_fresh_session),
            Some(is_leader),
            opt_str(handoff_note),
            squad_id.unwrap_or_else(Uuid::nil),
            opt_str(&head_sha),
            originator_user_id.unwrap_or_else(Uuid::nil),
            attr.accountable_user_id.unwrap_or_else(Uuid::nil),
            &overlay_value_or_null(&runtime_mcp_overlay.overlay),
            &overlay_value_or_null(&runtime_mcp_overlay.connected_apps),
            attr_source.as_deref(),
            attr_delegated_from.unwrap_or_else(Uuid::nil),
            attr.rule_version_id.unwrap_or_else(Uuid::nil),
            rerun_of_task_id.unwrap_or_else(Uuid::nil),
            attr_evidence_kind.as_deref(),
            attr_evidence_ref.unwrap_or_else(Uuid::nil),
            new_v7(),
        )
        .await;
        let task = match created {
            Ok(Some(t)) => t,
            Ok(None) => return Err(TaskServiceError::AgentNoRuntime),
            Err(e) => {
                // A concurrent enqueue for the same (issue, agent) won the race;
                // benign — log at debug and return the typed sentinel (#5914).
                if is_duplicate_pending_task_anyhow(&e) {
                    tracing::debug!(issue_id = %issue.id, agent_id = %agent_id, "mention task enqueue coalesced: pending task already exists");
                    return Err(TaskServiceError::DuplicatePendingTask(
                        ErrDuplicatePendingTask,
                    ));
                }
                tracing::error!(issue_id = %issue.id, agent_id = %agent_id, error = %e, "mention task enqueue failed");
                return Err(TaskServiceError::Sql(downcast_sqlx(e)));
            }
        };

        tracing::info!(
            task_id = %task.id,
            issue_id = %issue.id,
            agent_id = %agent_id,
            is_leader_task = is_leader,
            "mention task enqueued"
        );
        self.broadcast_task_event(cordy_protocol::EVENT_TASK_QUEUED, &task, Default::default())
            .await;
        self.notify_task_enqueued(&task).await;
        Ok(task)
    }

    /// Inert task that becomes claimable only after promotion flips it from
    /// deferred to queued (fallback assignee escalation).
    pub async fn enqueue_deferred_assignee_fallback(
        &self,
        issue: &Issue,
        agent_id: Uuid,
        squad_id: Option<Uuid>,
        escalation_for_task_id: Uuid,
        trigger_comment_id: Option<Uuid>,
        fire_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        let agent = get_agent(&self.pool, agent_id)
            .await
            .map_err(|e| TaskServiceError::LoadAgent(downcast_sqlx(e)))?
            .ok_or(TaskServiceError::LoadAgent(sqlx::Error::RowNotFound))?;
        if agent.archived_at.is_some() {
            tracing::debug!(issue_id = %issue.id, agent_id = %agent_id, "deferred fallback enqueue skipped: agent is archived");
            return Err(TaskServiceError::AgentArchived);
        }
        let Some(runtime_id) = agent.runtime_id else {
            tracing::error!(issue_id = %issue.id, agent_id = %agent_id, "deferred fallback enqueue failed: agent has no runtime");
            return Err(TaskServiceError::AgentNoRuntime);
        };

        // The fallback assignee reacts to the same trigger comment as the
        // primary routed task; stamping at creation keeps the eventual run off
        // the NULL-source bypass (MUL-4302 §2). Overlay intentionally left for
        // the promotion path.
        let attr = self
            .attribution_for_issue_task(
                issue,
                trigger_comment_id,
                attribution::Source::comment_source(),
                None,
            )
            .await;
        let attr = self.apply_attribution_fallback(attr, &agent).await.inspect_err(|_e| {
            tracing::warn!(issue_id = %issue.id, agent_id = %agent_id, "deferred fallback enqueue refused: attribution fail-closed");
        })?;
        let (attr_source, attr_delegated_from, attr_evidence_kind, attr_evidence_ref) =
            attribution_create_params(&attr);
        let is_leader = squad_id.is_some();
        let trigger_summary = self
            .build_comment_trigger_summary(issue.workspace_id, trigger_comment_id)
            .await
            .unwrap_or(None);
        let task = create_deferred_agent_task(
            &self.pool,
            agent_id,
            runtime_id,
            issue.id,
            agent_id,
            runtime_id,
            issue.id,
            priority_to_int(&issue.priority),
            trigger_comment_id.unwrap_or_else(Uuid::nil),
            trigger_summary.as_deref(),
            Some(is_leader),
            squad_id.unwrap_or_else(Uuid::nil),
            escalation_for_task_id,
            Some(fire_at),
            attr.user_id.unwrap_or_else(Uuid::nil),
            attr.accountable_user_id.unwrap_or_else(Uuid::nil),
            attr_source.as_deref(),
            attr_delegated_from.unwrap_or_else(Uuid::nil),
            attr_evidence_kind.as_deref(),
            attr_evidence_ref.unwrap_or_else(Uuid::nil),
            new_v7(),
        )
        .await
        .map_err(|e| {
            tracing::error!(issue_id = %issue.id, agent_id = %agent_id, error = %e, "deferred fallback enqueue failed");
            TaskServiceError::Sql(downcast_sqlx(e))
        })?
        .ok_or(TaskServiceError::AgentNoRuntime)?;

        tracing::info!(
            task_id = %task.id,
            issue_id = %issue.id,
            agent_id = %agent_id,
            fire_at = %fire_at,
            "deferred fallback task enqueued"
        );
        Ok(task)
    }

    /// Quick-create task: no issue/chat/autopilot link; the prompt lives in
    /// the context JSONB and the agent translates it into `cordy issue create`.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_quick_create_task(
        &self,
        workspace_id: Uuid,
        requester_id: Uuid,
        agent_id: Uuid,
        squad_id: Option<Uuid>,
        prompt: &str,
        priority: &str,
        due_date: &str,
        project_id: Option<Uuid>,
        parent_issue_id: Option<Uuid>,
        attachment_ids: Vec<Uuid>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        let agent = get_agent(&self.pool, agent_id)
            .await
            .map_err(|e| TaskServiceError::LoadAgent(downcast_sqlx(e)))?
            .ok_or(TaskServiceError::LoadAgent(sqlx::Error::RowNotFound))?;
        if agent.archived_at.is_some() {
            return Err(TaskServiceError::AgentArchived);
        }
        let Some(runtime_id) = agent.runtime_id else {
            return Err(TaskServiceError::AgentNoRuntime);
        };

        let mut payload = QuickCreateContext {
            type_: QUICK_CREATE_CONTEXT_TYPE.to_string(),
            prompt: prompt.to_string(),
            requester_id: requester_id.to_string(),
            workspace_id: workspace_id.to_string(),
            priority: priority.to_string(),
            due_date: due_date.to_string(),
            ..Default::default()
        };
        if let Some(project_id) = project_id {
            payload.project_id = project_id.to_string();
        }
        if let Some(squad_id) = squad_id {
            payload.squad_id = squad_id.to_string();
        }
        if let Some(parent_issue_id) = parent_issue_id {
            payload.parent_issue_id = parent_issue_id.to_string();
        }
        if !attachment_ids.is_empty() {
            payload.attachment_ids = attachment_ids.iter().map(|id| id.to_string()).collect();
        }
        let context_json = serde_json::to_value(&payload).map_err(|e| {
            TaskServiceError::Internal(format!("marshal quick-create context: {e}"))
        })?;

        // The requester is the direct_human originator and accountable.
        // Quick-create has NO antecedent row for the evidence pair — the run's
        // whole job is to CREATE the issue — so evidence is intentionally NULL;
        // source is still stamped direct_human (MUL-4302 §2), which is not a
        // NULL-source bypass.
        let attr = direct_human_run(Some(requester_id), EvidenceKind(String::new()), None);
        let attr = self.apply_attribution_fallback(attr, &agent).await?;
        let (attr_source, _, attr_evidence_kind, attr_evidence_ref) =
            attribution_create_params(&attr);
        let runtime_mcp_overlay = self.build_runtime_mcp_overlay(requester_id, &agent).await;
        let task = create_quick_create_task(
            &self.pool,
            agent_id,
            runtime_id,
            priority_to_int("high"),
            &context_json,
            requester_id,
            attr.accountable_user_id.unwrap_or_else(Uuid::nil),
            &overlay_value_or_null(&runtime_mcp_overlay.overlay),
            &overlay_value_or_null(&runtime_mcp_overlay.connected_apps),
            attr_source.as_deref(),
            attr_evidence_kind.as_deref(),
            attr_evidence_ref.unwrap_or_else(Uuid::nil),
            new_v7(),
        )
        .await
        .map_err(|e| TaskServiceError::Internal(format!("create quick-create task: {e}")))?
        .ok_or(TaskServiceError::AgentNoRuntime)?;

        tracing::info!(
            task_id = %task.id,
            agent_id = %agent_id,
            squad_id = %payload.squad_id,
            requester_id = %requester_id,
            workspace_id = %workspace_id,
            project_id = %payload.project_id,
            parent_issue_id = %payload.parent_issue_id,
            "quick-create task enqueued"
        );
        // Kick the daemon WS so the modal does not sit in 'queued' until the
        // next 30s poll tick.
        self.notify_task_enqueued(&task).await;
        Ok(task)
    }

    /// Makes a media-gated /issue task claimable. A missing row means the
    /// deadline sweeper already promoted it (or it was cancelled) — idempotent.
    pub async fn promote_deferred_channel_issue_task(
        &self,
        task_id: Uuid,
    ) -> Result<(), TaskServiceError> {
        let promoted = promote_deferred_channel_issue_task(&self.pool, task_id)
            .await
            .map_err(|e| TaskServiceError::Sql(downcast_sqlx(e)))?;
        let Some(task) = promoted else {
            return Ok(());
        };
        tracing::info!(
            task_id = %task.id,
            issue_id = ?task.issue_id,
            agent_id = %task.agent_id,
            "channel media-ready issue task promoted"
        );
        self.broadcast_task_event(cordy_protocol::EVENT_TASK_QUEUED, &task, Default::default())
            .await;
        self.notify_task_enqueued(&task).await;
        Ok(())
    }

    /// Queues channel tasks as soon as every unexpired media marker in the
    /// session has been cleared. If the process dies first, the normal
    /// deferred-task promoter queues them at their persisted fire_at deadline.
    pub async fn promote_channel_chat_tasks_if_media_ready(
        &self,
        session_id: Uuid,
    ) -> Result<(), TaskServiceError> {
        let tasks = promote_channel_chat_tasks_if_media_ready(&self.pool, session_id)
            .await
            .map_err(|e| {
                TaskServiceError::Internal(format!("promote channel chat tasks after media: {e}"))
            })?;
        for task in tasks {
            tracing::info!(
                task_id = %task.id,
                chat_session_id = %session_id,
                agent_id = %task.agent_id,
                "channel media-ready chat task promoted"
            );
            self.broadcast_task_event(cordy_protocol::EVENT_TASK_QUEUED, &task, Default::default())
                .await;
            self.notify_task_enqueued(&task).await;
        }
        Ok(())
    }

    /// Cancels every active task on the issue, reconciles each affected
    /// agent's status, and broadcasts task:cancelled. Only explicit
    /// issue-lifecycle cleanup paths may call this (DeleteIssue /
    /// BatchDeleteIssues); a plain status flip must NOT route here (MUL-4465).
    pub async fn cancel_tasks_for_issue(&self, issue_id: Uuid) -> Result<(), TaskServiceError> {
        let cancelled = cancel_agent_tasks_by_issue(&self.pool, issue_id)
            .await
            .map_err(|e| TaskServiceError::Sql(downcast_sqlx(e)))?;
        for t in &cancelled {
            self.capture_task_cancelled(t).await;
            self.broadcast_task_event(cordy_protocol::EVENT_TASK_CANCELLED, t, Default::default())
                .await;
        }
        self.notify_tasks_finished(&cancelled);
        Ok(())
    }

    // --- Chat task family -------------------------------------------------------

    /// Creates a task-owned input batch for a chat session. Channel media
    /// defers the task until binding completes or the durable fallback deadline
    /// expires; other chat tasks queue immediately.
    ///
    /// initiatorUserID is the user who actually sent the triggering message —
    /// NOT necessarily chat_session.creator_id (Lark group sessions set the
    /// creator to the installer). Stored so the daemon brief attributes the run
    /// to the right person (MUL-2645).
    pub async fn enqueue_chat_task(
        &self,
        chat_session: &ChatSession,
        initiator_user_id: Option<Uuid>,
        mut force_fresh_session: bool,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        let agent = get_agent(&self.pool, chat_session.agent_id)
            .await
            .map_err(|e| TaskServiceError::LoadAgent(downcast_sqlx(e)))?
            .ok_or(TaskServiceError::LoadAgent(sqlx::Error::RowNotFound))?;
        if agent.archived_at.is_some() {
            return Err(TaskServiceError::ChatAgentArchived);
        }
        let Some(runtime_id) = agent.runtime_id else {
            return Err(TaskServiceError::ChatAgentNoRuntime);
        };

        // The chat sender is the direct_human originator and accountable.
        // Evidence uses the uniform pair (kind=chat, ref=session id); an
        // unresolved sender degrades to unattributed rather than a NULL-source
        // bypass (MUL-4302 §2).
        let attr = direct_human_run(initiator_user_id, evidence_chat(), Some(chat_session.id));
        let attr = self.apply_attribution_fallback(attr, &agent).await.inspect_err(|_e| {
            tracing::warn!(chat_session_id = %chat_session.id, "chat task enqueue refused: attribution fail-closed");
        })?;
        let (attr_source, _, attr_evidence_kind, attr_evidence_ref) =
            attribution_create_params(&attr);
        let runtime_mcp_overlay = match initiator_user_id {
            Some(initiator) => self.build_runtime_mcp_overlay(initiator, &agent).await,
            None => RuntimeMcpOverlayData::default(),
        };

        let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;

        // Refuse to enqueue onto an archived session — the one enqueue path
        // with a delay in front of it (channel runs are debounced by the batch
        // window), so an archive committing inside that window must win. FOR NO
        // KEY UPDATE rather than FOR UPDATE: the channel path cannot afford to
        // block inbound appends. First statement of the tx keeps the
        // chat_session → agent_task_queue lock order deadlock-free.
        let current_session = lock_chat_session_for_enqueue(&mut *tx, chat_session.id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("lock chat session: {e}")))?
            .ok_or(TaskServiceError::Internal(
                "lock chat session: not found".into(),
            ))?;
        if current_session.status != "active" {
            return Err(TaskServiceError::ChatSessionArchived);
        }
        // Lock the binding only after the chat_session lock; the append path
        // touches them in the same order, avoiding an ABBA edge.
        let pending_fresh =
            match lock_channel_chat_session_pending_fresh(&mut *tx, chat_session.id).await {
                Ok(Some(fresh)) => fresh,
                Ok(None) => false,
                Err(e) => {
                    return Err(TaskServiceError::Internal(format!(
                        "lock channel pending fresh: {e}"
                    )))
                }
            };
        if pending_fresh {
            force_fresh_session = true;
        }
        let media_pending_until = get_channel_media_pending_until(&mut *tx, chat_session.id)
            .await
            .map_err(|e| {
                TaskServiceError::Internal(format!("load channel media pending deadline: {e}"))
            })?
            .flatten();

        let initiator = initiator_user_id.unwrap_or_else(Uuid::nil);
        let created = create_chat_task(
            &mut *tx,
            chat_session.agent_id,
            runtime_id,
            2, // medium priority for chat
            chat_session.id,
            initiator,
            media_pending_until,
            initiator,
            attr.accountable_user_id.unwrap_or_else(Uuid::nil),
            Some(force_fresh_session),
            &overlay_value_or_null(&runtime_mcp_overlay.overlay),
            &overlay_value_or_null(&runtime_mcp_overlay.connected_apps),
            attr_source.as_deref(),
            attr_evidence_kind.as_deref(),
            attr_evidence_ref.unwrap_or_else(Uuid::nil),
            new_v7(),
        )
        .await
        .map_err(|e| TaskServiceError::Internal(format!("create chat task: {e}")))?
        .ok_or(TaskServiceError::AgentNoRuntime)?;
        let mut task = set_chat_task_input_owner_self(&mut *tx, created.id)
            .await
            .map_err(|e| {
                TaskServiceError::Internal(format!("set channel chat task input owner: {e}"))
            })?
            .ok_or(TaskServiceError::AgentNoRuntime)?;
        link_unowned_channel_chat_messages_to_task(&mut *tx, task.id, chat_session.id)
            .await
            .map_err(|e| {
                TaskServiceError::Internal(format!("seal channel chat task input: {e}"))
            })?;
        // The deadline read above ran before the seal; under READ COMMITTED a
        // media message committed between the two statements would be sealed
        // into this task without deferring it. Re-derive from the sealed batch.
        match defer_chat_task_for_sealed_pending_media(&mut *tx, task.id).await {
            Ok(Some(corrected)) => task = corrected,
            Ok(None) => {}
            Err(e) => {
                return Err(TaskServiceError::Internal(format!(
                    "defer chat task for sealed pending media: {e}"
                )))
            }
        }
        if pending_fresh {
            clear_channel_chat_session_pending_fresh(&mut *tx, chat_session.id)
                .await
                .map_err(|e| {
                    TaskServiceError::Internal(format!("clear channel pending fresh: {e}"))
                })?;
        }
        tx.commit()
            .await
            .map_err(|e| TaskServiceError::Internal(format!("commit chat task enqueue: {e}")))?;

        if task.status == "deferred" {
            tracing::info!(
                task_id = %task.id,
                chat_session_id = %chat_session.id,
                agent_id = %chat_session.agent_id,
                fire_at = ?task.fire_at,
                "chat task deferred for channel media"
            );
            // Fence the clear-vs-create race: re-check after commit so the task
            // promotes immediately when the marker already cleared. A fence
            // failure must not surface as an enqueue failure — the claim-path
            // promoter re-queues at fire_at regardless.
            if let Err(err) = self
                .promote_channel_chat_tasks_if_media_ready(chat_session.id)
                .await
            {
                tracing::warn!(
                    task_id = %task.id,
                    chat_session_id = %chat_session.id,
                    error = %err,
                    "chat task media-ready fence failed; deferred task falls back to its deadline"
                );
            }
            return Ok(task);
        }

        tracing::info!(
            task_id = %task.id,
            chat_session_id = %chat_session.id,
            agent_id = %chat_session.agent_id,
            "chat task enqueued"
        );
        self.broadcast_task_event(cordy_protocol::EVENT_TASK_QUEUED, &task, Default::default())
            .await;
        self.notify_task_enqueued(&task).await;
        Ok(task)
    }

    /// Atomically persists one web/mobile direct-chat turn: owning task, user
    /// message bound to that task, attachment bindings, and the session touch
    /// all commit together (MUL-4351). The daemon is notified only after
    /// commit. Caller must have gated the session and preflighted the agent;
    /// those checks repeat under the transaction locks because either row may
    /// change before enqueue.
    #[allow(clippy::too_many_arguments)]
    pub async fn send_direct_chat_message(
        &self,
        session: &ChatSession,
        agent: &Agent,
        initiator_user_id: Option<Uuid>,
        content: &str,
        attachment_ids: Vec<Uuid>,
        uploader_type: &str,
        uploader_id: Option<Uuid>,
    ) -> Result<DirectChatSendResult, TaskServiceError> {
        // Overlay before the transaction — network I/O must not hold locks.
        let overlay = match initiator_user_id {
            Some(initiator) => self.build_runtime_mcp_overlay(initiator, agent).await,
            None => RuntimeMcpOverlayData::default(),
        };
        // Full attribution resolved before the tx (policy read + fallback must
        // not run with a transaction open) — same direct_human stamp as
        // EnqueueChatTask (MUL-4302 §2).
        let attr = direct_human_run(initiator_user_id, evidence_chat(), Some(session.id));
        let attr = self.apply_attribution_fallback(attr, agent).await?;
        let (attr_source, _, attr_evidence_kind, attr_evidence_ref) =
            attribution_create_params(&attr);

        let mut out = DirectChatSendResult {
            task: None,
            message: None,
            bound_attachment_ids: vec![],
            queued: false,
        };
        let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;

        // Serialise against a concurrent runtime rebind of the same session
        // (MUL-5163): lock first, then re-read both rows under it.
        lock_chat_session_for_runtime_bind(&mut *tx, session.id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("lock chat session: {e}")))?;
        let current_session = get_chat_session(&mut *tx, session.id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("reload chat session: {e}")))?
            .ok_or(TaskServiceError::Internal(
                "reload chat session: not found".into(),
            ))?;
        if current_session.status != "active" {
            return Err(TaskServiceError::ChatSessionArchived);
        }
        let carrier = get_agent_for_claim_update(&mut *tx, session.agent_id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("reload chat agent: {e}")))?
            .ok_or(TaskServiceError::Internal(
                "reload chat agent: not found".into(),
            ))?;
        if carrier.archived_at.is_some() {
            return Err(TaskServiceError::ChatAgentArchived);
        }
        let Some(carrier_runtime_id) = carrier.runtime_id else {
            return Err(TaskServiceError::ChatAgentNoRuntime);
        };

        // Product queue semantics are positional: this send is a follow-up only
        // when another visible task in the session is ahead of it. Deferred
        // retries count because they resume an older turn first.
        out.queued = has_pending_chat_turn_for_session(&mut *tx, session.id)
            .await
            .map_err(|e| {
                TaskServiceError::Internal(format!("check direct chat queue position: {e}"))
            })?
            .unwrap_or(false);

        let initiator = initiator_user_id.unwrap_or_else(Uuid::nil);
        let created = create_chat_task(
            &mut *tx,
            session.agent_id,
            carrier_runtime_id,
            2, // medium priority; matches EnqueueChatTask
            session.id,
            initiator,
            None,
            attr.user_id.unwrap_or_else(Uuid::nil),
            attr.accountable_user_id.unwrap_or_else(Uuid::nil),
            Some(false),
            &overlay_value_or_null(&overlay.overlay),
            &overlay_value_or_null(&overlay.connected_apps),
            attr_source.as_deref(),
            attr_evidence_kind.as_deref(),
            attr_evidence_ref.unwrap_or_else(Uuid::nil),
            new_v7(),
        )
        .await
        .map_err(|e| TaskServiceError::Internal(format!("create direct chat task: {e}")))?
        .ok_or(TaskServiceError::AgentNoRuntime)?;
        // Claim this task's own input batch before the user message is written.
        let task = set_chat_task_input_owner_self(&mut *tx, created.id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("stamp direct chat input owner: {e}")))?
            .ok_or(TaskServiceError::AgentNoRuntime)?;
        out.task = Some(task.clone());

        // Adopt the onboarding kickoff if this session still has an unowned one
        // — the only thing that ever delivers it to a runtime, and it must land
        // before the member's own row so the batch reads "context, then their
        // message" once ordered by created_at.
        adopt_orphan_onboarding_kickoff(&mut *tx, session.id, task.id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("adopt onboarding kickoff: {e}")))?;

        let msg = create_chat_message(
            &mut *tx,
            session.id,
            "user",
            content,
            Some(task.id),
            None,
            None,
            Some(cordy_protocol::CHAT_MESSAGE_KIND_MESSAGE),
            &serde_json::Value::Array(vec![]),
            None,
            None,
            new_v7(),
        )
        .await
        .map_err(|e| TaskServiceError::Internal(format!("create user chat message: {e}")))?
        .ok_or_else(|| TaskServiceError::Internal("create user chat message: no row".into()))?;

        if !attachment_ids.is_empty() {
            out.bound_attachment_ids = link_attachments_to_chat_message(
                &mut *tx,
                msg.id,
                session.id,
                session.workspace_id,
                uploader_type,
                uploader_id.unwrap_or_else(Uuid::nil),
                attachment_ids,
            )
            .await
            .map_err(|e| TaskServiceError::Internal(format!("link chat attachments: {e}")))?;
        }
        out.message = Some(msg);

        touch_chat_session(&mut *tx, session.id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("touch chat session: {e}")))?;
        tx.commit()
            .await
            .map_err(|e| TaskServiceError::Internal(format!("commit direct chat send: {e}")))?;

        tracing::info!(
            task_id = %task.id,
            chat_session_id = %session.id,
            agent_id = %session.agent_id,
            "direct chat task enqueued"
        );
        self.broadcast_task_event(cordy_protocol::EVENT_TASK_QUEUED, &task, Default::default())
            .await;
        self.notify_task_enqueued(&task).await;
        Ok(out)
    }

    /// Writes a Mika conversation's first two rows in one transaction: hidden
    /// kickoff + product-authored opening (MUL-5827). Nothing is enqueued.
    /// "Session still empty" is enforced under the chat-session lock.
    pub async fn open_mika_onboarding_chat(
        &self,
        session: &ChatSession,
        kickoff: &str,
        opening: &str,
    ) -> Result<MikaOnboardingOpenResult, TaskServiceError> {
        let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;
        // Same lock and ORDER as the send path, so an opening racing a first
        // send or a runtime rebind serializes instead of deadlocking.
        lock_chat_session_for_runtime_bind(&mut *tx, session.id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("lock chat session: {e}")))?;
        let current = get_chat_session(&mut *tx, session.id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("reload chat session: {e}")))?
            .ok_or(TaskServiceError::Internal(
                "reload chat session: not found".into(),
            ))?;
        if current.status != "active" {
            return Err(TaskServiceError::ChatSessionArchived);
        }
        if chat_session_has_user_message(&mut *tx, session.id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("check chat session input: {e}")))?
            .unwrap_or(false)
        {
            return Err(TaskServiceError::ChatSessionAlreadyStarted);
        }

        let kickoff_row = create_chat_message(
            &mut *tx,
            session.id,
            "user",
            kickoff,
            None,
            None,
            None,
            Some(cordy_protocol::CHAT_MESSAGE_KIND_ONBOARDING_KICKOFF),
            &serde_json::Value::Array(vec![]),
            None,
            None,
            new_v7(),
        )
        .await
        .map_err(|e| TaskServiceError::Internal(format!("create onboarding kickoff: {e}")))?
        .ok_or_else(|| TaskServiceError::Internal("create onboarding kickoff: no row".into()))?;

        // Ordered after the kickoff — see the query comment for why a shared
        // transaction timestamp is not good enough.
        let opening_row = create_mika_onboarding_opening(
            &mut *tx,
            session.id,
            opening,
            Some(kickoff_row.created_at),
            new_v7(),
        )
        .await
        .map_err(|e| TaskServiceError::Internal(format!("create onboarding opening: {e}")))?
        .ok_or_else(|| TaskServiceError::Internal("create onboarding opening: no row".into()))?;

        touch_chat_session(&mut *tx, session.id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("touch chat session: {e}")))?;
        tx.commit()
            .await
            .map_err(|e| TaskServiceError::Internal(format!("commit mika onboarding open: {e}")))?;

        Ok(MikaOnboardingOpenResult {
            kickoff: kickoff_row,
            opening: opening_row,
        })
    }

    /// Validates a quick-actions refresh target without running generation:
    /// returns the target assistant message id and its turn's task row. The
    /// caller starts the pass via GenerateChatQuickActionsAsync.
    pub async fn regenerate_chat_quick_actions(
        &self,
        chat_session: &ChatSession,
        expected_message_id: Uuid,
    ) -> Result<(Uuid, AgentTaskQueue), TaskServiceError> {
        if self.quick_actions().is_none() {
            return Err(TaskServiceError::ChatQuickActionsUnavailable);
        }
        // Target is the latest assistant turn; only an ordinary message turn
        // can seed suggestions.
        let target = get_latest_assistant_chat_message_for_session(&self.pool, chat_session.id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("load latest assistant turn: {e}")))?
            .ok_or(TaskServiceError::ChatQuickActionsNoTurn)?;
        if target.message_kind != cordy_protocol::CHAT_MESSAGE_KIND_MESSAGE {
            return Err(TaskServiceError::ChatQuickActionsNoTurn);
        }
        let Some(target_task_id) = target.task_id else {
            return Err(TaskServiceError::ChatQuickActionsNoTurn);
        };
        // Refuse unless the client is refreshing the turn that is STILL the
        // latest (MUL-5149).
        if target.id != expected_message_id {
            return Err(TaskServiceError::ChatQuickActionsStale);
        }
        // A running turn is about to replace the latest reply.
        if has_active_chat_task_for_session(&self.pool, chat_session.id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("check active chat task: {e}")))?
            .unwrap_or(false)
        {
            return Err(TaskServiceError::ChatQuickActionsBusy);
        }
        // A refresh creates no task row, so the check above cannot see a
        // generation already running for this session — gate on the in-flight
        // pass registry too.
        if self
            .quick_actions_in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&chat_session.id)
        {
            return Err(TaskServiceError::ChatQuickActionsBusy);
        }

        let task = get_agent_task(&self.pool, target_task_id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("load target turn task: {e}")))?
            .ok_or(TaskServiceError::Internal(
                "load target turn task: not found".into(),
            ))?;

        tracing::info!(
            chat_session_id = %chat_session.id,
            target_task_id = %target_task_id,
            target_message_id = %target.id,
            "chat quick-actions regenerate accepted"
        );
        Ok((target.id, task))
    }
}

/// Rows a transactional direct-chat send persisted, so the handler can
/// broadcast the user message and shape its response without re-reading.
/// Fields are `None` until the transaction fills them in order.
#[derive(Debug, Clone, Default)]
pub struct DirectChatSendResult {
    pub task: Option<AgentTaskQueue>,
    pub message: Option<ChatMessage>,
    pub bound_attachment_ids: Vec<Option<Uuid>>,
    pub queued: bool,
}

/// The two rows that open a Mika conversation (MUL-5827).
#[derive(Debug, Clone)]
pub struct MikaOnboardingOpenResult {
    /// Hidden product context, written WITHOUT a task; the member's first real
    /// send adopts it.
    pub kickoff: ChatMessage,
    /// What the member reads, already final; no task id, nothing to regenerate.
    pub opening: ChatMessage,
}

/// Maps a resolved attribution onto the CreateAgentTask provenance columns.
/// source is always stamped; lineage/evidence only when present.
fn attribution_create_params(
    attr: &AttributionResult,
) -> (Option<String>, Option<Uuid>, Option<String>, Option<Uuid>) {
    (
        attr.source.as_ref().map(|s| s.as_str().to_string()),
        attr.delegated_from_task_id,
        attr.evidence_kind
            .as_ref()
            .filter(|k| !k.as_str().is_empty())
            .map(|k| k.as_str().to_string()),
        attr.evidence_ref_id,
    )
}

pub(crate) fn opt_str(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

pub(crate) fn overlay_value_or_null(v: &Option<serde_json::Value>) -> serde_json::Value {
    v.clone().unwrap_or(serde_json::Value::Null)
}

/// Port of util.SanitizeTextForPostgres: strips NULs and replaces invalid
/// UTF-8 so a poisoned string cannot roll back the write that carries it.
pub(crate) fn sanitize_text_for_postgres(s: &str) -> String {
    if !s.contains('\0') {
        return s.to_string();
    }
    s.replace('\0', "")
}

// --- Slice 3: cancellation + claim (Go lines ~2187–3694) ----------------------

/// What the caller knows about the client that asked for the cancellation.
#[derive(Debug, Clone, Default)]
pub struct CancelTaskOptions {
    /// True when the caller can recover a prompt through the durable
    /// draft-restore path (#5219). Only such a client may be handed a deferred
    /// outcome. See protocol.APP_CAPABILITY_CHAT_DRAFT_RESTORE_V1.
    pub client_supports_draft_restore: bool,
    /// Turns queue edit/remove into a session-scoped compare-and-set.
    pub queued_only: bool,
    pub expected_chat_session: Uuid,
    pub queue_action: String,
    /// Persisted onto the cancelled row; only for cancellations the USER did
    /// not ask for (server-initiated repairs).
    pub error_message: String,
    pub failure_reason: String,
    /// Distinguishes the issue UI/API cancel action from automatic server
    /// repairs; an explicit user cancellation terminally acknowledges any
    /// delegated-failure recovery signal planned into the task.
    pub user_initiated: bool,
}

#[derive(Debug, Clone)]
pub struct CancelledChatMessageResult {
    pub chat_session_id: String,
    pub message_id: String,
    pub content: String,
    pub restore_to_input: bool,
    /// Rows detached from the deleted user message so they survive the ON
    /// DELETE CASCADE and can re-bind when the restored draft is re-sent.
    pub attachments: Vec<cordy_db::models::Attachment>,
}

#[derive(Debug, Clone)]
pub struct CancelTaskResult {
    pub task: AgentTaskQueue,
    pub cancelled_chat_message: Option<CancelledChatMessageResult>,
}

#[derive(Debug, thiserror::Error)]
#[error("task is no longer queued")]
pub struct ErrTaskNoLongerQueued;

/// RerunIssue refused because the current operator may not invoke the
/// resolved target agent (MUL-4525); the handler maps it to a structured 403.
#[derive(Debug, Clone, Copy)]
pub struct ErrRerunInvokeNotAllowed;

/// Sentinel value of the rerun invoke-not-allowed error.
pub const ERR_RERUN_INVOKE_NOT_ALLOWED: ErrRerunInvokeNotAllowed = ErrRerunInvokeNotAllowed;

/// Parameters for persisting a task-scoped agent token (Go
/// db.CreateTaskTokenParams).
#[derive(Debug, Clone)]
pub struct CreateTaskToken {
    pub token_hash: String,
    pub task_id: Uuid,
    pub agent_id: Uuid,
    pub workspace_id: Uuid,
    pub user_id: Uuid,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Parameters for persisting a Remote MCP daemon token (Go
/// db.CreateDaemonTokenParams).
#[derive(Debug, Clone)]
pub struct CreateDaemonToken {
    pub token_hash: String,
    pub workspace_id: Uuid,
    pub daemon_id: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Each agent id appearing in the cancelled rows once, preserving first-seen
/// order — collapses redundant per-row reconciles to one per agent (D#3319).
fn distinct_agent_ids(cancelled: &[AgentTaskQueue]) -> Vec<Uuid> {
    let mut seen = std::collections::HashSet::new();
    let mut ids = Vec::with_capacity(cancelled.len());
    for t in cancelled {
        if seen.insert(t.agent_id) {
            ids.push(t.agent_id);
        }
    }
    ids
}

/// The id the task's user-message input batch is keyed on:
/// chat_input_task_id when set (auto-retry clones inherit their parent's),
/// falling back to the task's own id for legacy rows.
pub fn chat_input_owner_id(task: &AgentTaskQueue) -> Uuid {
    task.chat_input_task_id.unwrap_or(task.id)
}

/// Websocket-safe agent projection. Status events are workspace-wide, so they
/// must never contain plaintext env/MCP/connected-app configuration.
fn safe_agent_status_payload(agent: &Agent) -> serde_json::Value {
    let env_count = agent.custom_env.as_object().map_or(0, serde_json::Map::len);
    let mut value = serde_json::to_value(agent).unwrap_or_default();
    let Some(map) = value.as_object_mut() else {
        return serde_json::Value::Object(Default::default());
    };
    map.remove("custom_env");
    map.insert("has_custom_env".into(), serde_json::json!(env_count > 0));
    map.insert("custom_env_key_count".into(), serde_json::json!(env_count));

    let has_mcp = map
        .get("mcp_config")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|config| !config.is_empty());
    map.insert("mcp_config".into(), serde_json::json!({}));
    map.insert("mcp_config_redacted".into(), serde_json::json!(has_mcp));

    let has_composio = agent
        .composio_toolkit_allowlist
        .as_ref()
        .is_some_and(|allowlist| !allowlist.is_empty());
    map.remove("composio_toolkit_allowlist");
    map.insert(
        "composio_toolkit_allowlist_redacted".into(),
        serde_json::json!(has_composio),
    );
    if let Some(token) = map
        .get_mut("runtime_config")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|config| config.get_mut("gateway"))
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|gateway| gateway.get_mut("token"))
    {
        if token.as_str().is_some_and(|token| !token.is_empty()) {
            *token = serde_json::Value::String("***".into());
        }
    }
    value
}

impl TaskService {
    /// Refreshes the agent's status from its active tasks and broadcasts
    /// agent:status. Best-effort: errors are swallowed like Go's early return.
    pub async fn reconcile_agent_status(&self, agent_id: Uuid) {
        let Ok(Some(agent)) = refresh_agent_status_from_tasks(&self.pool, agent_id).await else {
            return;
        };
        tracing::debug!(agent_id = %agent_id, status = %agent.status, "agent status reconciled");
        self.publish_agent_status(&agent).await;
    }

    pub(crate) async fn publish_agent_status(&self, agent: &Agent) {
        self.bus.publish(&cordy_events::Event {
            event_type: cordy_protocol::EVENT_AGENT_STATUS.to_string(),
            workspace_id: agent.workspace_id.to_string(),
            actor_type: "system".to_string(),
            actor_id: String::new(),
            payload: serde_json::json!({ "agent": safe_agent_status_payload(agent) }),
            task_id: String::new(),
            chat_session_id: String::new(),
        });
    }

    /// Cancels every active task on an agent and reconciles once after the
    /// loop. Returns the cancelled rows so callers can report counts.
    pub async fn cancel_tasks_for_agent(
        &self,
        agent_id: Uuid,
    ) -> Result<Vec<AgentTaskQueue>, TaskServiceError> {
        let cancelled = cancel_agent_tasks_by_agent(&self.pool, agent_id)
            .await
            .map_err(|e| TaskServiceError::Sql(downcast_sqlx(e)))?;
        for t in &cancelled {
            self.capture_task_cancelled(t).await;
            self.broadcast_task_event(cordy_protocol::EVENT_TASK_CANCELLED, t, Default::default())
                .await;
        }
        // One reconcile: all rows belong to the same agent.
        self.reconcile_agent_status(agent_id).await;
        self.notify_tasks_finished(&cancelled);
        Ok(cancelled)
    }

    /// Cancels active tasks whose planned comment batch contains the given
    /// edited/deleted comment. Must run before deletion clears the trigger FK;
    /// returned rows let the handler re-route surviving input.
    pub async fn cancel_tasks_by_trigger_comment(
        &self,
        comment_id: Uuid,
    ) -> Result<Vec<AgentTaskQueue>, TaskServiceError> {
        let cancelled = cancel_agent_tasks_by_trigger_comment(&self.pool, comment_id)
            .await
            .map_err(|e| TaskServiceError::Sql(downcast_sqlx(e)))?;
        for t in &cancelled {
            self.capture_task_cancelled(t).await;
            self.broadcast_task_event(cordy_protocol::EVENT_TASK_CANCELLED, t, Default::default())
                .await;
        }
        for agent_id in distinct_agent_ids(&cancelled) {
            self.reconcile_agent_status(agent_id).await;
        }
        self.notify_tasks_finished(&cancelled);
        Ok(cancelled)
    }

    /// Reconciles each affected agent and emits task:cancelled for every row.
    /// Callers must invoke AFTER committing so subscribers never observe a
    /// "cancelled" event for a row that might roll back. workspaceID comes from
    /// the caller because the just-committed transaction may have deleted the
    /// row the resolution would read.
    pub async fn broadcast_cancelled_tasks(
        &self,
        workspace_id: &str,
        cancelled: &[AgentTaskQueue],
    ) {
        for t in cancelled {
            self.capture_task_cancelled(t).await;
            self.reconcile_agent_status(t.agent_id).await;
            self.publish_task_event(
                cordy_protocol::EVENT_TASK_CANCELLED,
                workspace_id,
                t,
                Default::default(),
            )
            .await;
        }
        self.notify_tasks_finished(cancelled);
    }

    /// Post-commit queue invalidation for clients.
    pub async fn broadcast_task_queued(&self, task: &AgentTaskQueue) {
        self.broadcast_task_event(cordy_protocol::EVENT_TASK_QUEUED, task, Default::default())
            .await;
    }

    pub async fn capture_cancelled_tasks(&self, cancelled: &[AgentTaskQueue]) {
        for t in cancelled {
            self.capture_task_cancelled(t).await;
        }
    }

    /// Cancels a single task for an automatic server path. Does not
    /// acknowledge delegated-failure recovery inputs.
    pub async fn cancel_task(&self, task_id: Uuid) -> Result<AgentTaskQueue, TaskServiceError> {
        let result = self
            .cancel_task_with_result(
                task_id,
                CancelTaskOptions {
                    client_supports_draft_restore: true,
                    ..Default::default()
                },
            )
            .await?;
        Ok(result.task)
    }

    /// Explicit issue-task cancellation: terminally acknowledges any
    /// delegated-failure recovery signal so the sweeper respects the user's
    /// decision instead of recreating the task.
    pub async fn cancel_task_by_user(
        &self,
        task_id: Uuid,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        let result = self
            .cancel_task_with_result(
                task_id,
                CancelTaskOptions {
                    client_supports_draft_restore: true,
                    user_initiated: true,
                    ..Default::default()
                },
            )
            .await?;
        Ok(result.task)
    }

    /// Cancels a task the SERVER decided to stop, persisting an actionable
    /// reason onto the row. Runs the full cancellation flow because a raw query
    /// bypass leaves the agent pill running and waiters unwoken.
    pub async fn cancel_task_with_reason(
        &self,
        task_id: Uuid,
        error_message: &str,
        failure_reason: &str,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        let result = self
            .cancel_task_with_result(
                task_id,
                CancelTaskOptions {
                    client_supports_draft_restore: true,
                    error_message: error_message.to_string(),
                    failure_reason: failure_reason.to_string(),
                    ..Default::default()
                },
            )
            .await?;
        Ok(result.task)
    }

    /// Cancels a single task and returns any chat-specific cleanup result
    /// needed by user-facing callers.
    pub async fn cancel_task_with_result(
        &self,
        task_id: Uuid,
        mut opts: CancelTaskOptions,
    ) -> Result<CancelTaskResult, TaskServiceError> {
        // A NUL in either field rolls the cancellation back and leaves the task
        // running — the same wedge as GH #7098 on the fail/complete paths.
        opts.error_message = sanitize_text_for_postgres(&opts.error_message);
        opts.failure_reason = sanitize_text_for_postgres(&opts.failure_reason);

        if opts.user_initiated
            && (!opts.error_message.is_empty() || !opts.failure_reason.is_empty())
        {
            return Err(TaskServiceError::Internal(
                "user-initiated cancellation cannot carry a server failure reason".into(),
            ));
        }

        let task;
        let mut cancelled_chat_message = None;

        if opts.queued_only {
            if opts.queue_action != "edit" && opts.queue_action != "remove" {
                return Err(TaskServiceError::Internal(
                    "queue action must be edit or remove".into(),
                ));
            }
            let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;
            lock_chat_session_for_task(&mut *tx, task_id)
                .await
                .map_err(|e| {
                    TaskServiceError::Internal(format!("lock queued chat session: {e}"))
                })?;
            let cancelled = cancel_queued_agent_task(&mut *tx, task_id, opts.expected_chat_session)
                .await
                .map_err(downcast_sqlx)?;
            let Some(cancelled) = cancelled else {
                return Err(TaskServiceError::NoLongerQueued(ErrTaskNoLongerQueued));
            };
            cancelled_chat_message = self
                .settle_queued_chat_input(&mut tx, &cancelled, &opts.queue_action)
                .await?;
            tx.commit().await.map_err(TaskServiceError::Sql)?;
            task = cancelled;
        } else {
            // The status flip and the chat resume-pointer advance commit
            // together; split apart, `cancelled` becomes visible while the
            // pointer still names the previous turn's session.
            let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;
            // chat_session → agent_task_queue is the repo-wide lock order; see
            // Go lockChatSessionForTaskWrite. ErrNoRows = non-chat task or the
            // session was already deleted — nothing to protect either way.
            let _ = lock_chat_session_for_task(&mut *tx, task_id)
                .await
                .map_err(|e| {
                    TaskServiceError::Internal(format!("lock chat session for task write: {e}"))
                })?;
            let cancelled = if opts.user_initiated {
                cancel_agent_task_by_user(&mut *tx, task_id).await
            } else if !opts.error_message.is_empty() || !opts.failure_reason.is_empty() {
                cancel_agent_task_with_reason(
                    &mut *tx,
                    opt_str(&opts.error_message),
                    opt_str(&opts.failure_reason),
                    task_id,
                )
                .await
            } else {
                cancel_agent_task(&mut *tx, task_id).await
            };
            let cancelled = match cancelled {
                Ok(Some(c)) => c,
                Ok(None) => {
                    return Err(TaskServiceError::NoLongerQueued(ErrTaskNoLongerQueued));
                }
                Err(e) => return Err(TaskServiceError::Sql(downcast_sqlx(e))),
            };
            if cancelled.chat_session_id.is_some() {
                advance_cancelled_chat_session_pointer(&mut *tx, cancelled.id)
                    .await
                    .map_err(|e| {
                        TaskServiceError::Internal(format!("advance cancelled chat pointer: {e}"))
                    })?;
            }
            tx.commit().await.map_err(TaskServiceError::Sql)?;
            task = cancelled;
        }

        tracing::info!(task_id = %task.id, issue_id = ?task.issue_id, "task cancelled");
        self.capture_task_cancelled(&task).await;
        if !opts.queued_only {
            cancelled_chat_message = self.finalize_cancelled_chat_message(&task, &opts).await;
        }

        self.reconcile_agent_status(task.agent_id).await;

        self.broadcast_task_event(
            cordy_protocol::EVENT_TASK_CANCELLED,
            &task,
            Default::default(),
        )
        .await;
        self.notify_task_finished(&task);

        Ok(CancelTaskResult {
            task,
            cancelled_chat_message,
        })
    }

    /// Atomically cancels every queued follow-up in a chat session. The
    /// session lock preserves the delete path's session → agent → task order;
    /// the agent lock prevents ClaimTask from promoting a row mid-update.
    pub async fn cancel_queued_chat_tasks(
        &self,
        session_id: Uuid,
        agent_id: Uuid,
    ) -> Result<(), TaskServiceError> {
        let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;
        if lock_chat_session_for_delete(&mut *tx, session_id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("lock chat session: {e}")))?
            .is_none()
        {
            return Ok(());
        }
        get_agent_for_claim_update(&mut *tx, agent_id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("lock chat agent: {e}")))?;
        let tasks = cancel_queued_agent_tasks_for_session(&mut *tx, session_id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("cancel queued chat tasks: {e}")))?;
        for task in &tasks {
            self.settle_queued_chat_input(&mut tx, task, "remove")
                .await?;
        }
        tx.commit().await.map_err(TaskServiceError::Sql)?;

        for task in &tasks {
            tracing::info!(task_id = %task.id, issue_id = ?task.issue_id, "task cancelled");
            self.capture_task_cancelled(task).await;
        }
        if !tasks.is_empty() {
            self.reconcile_agent_status(agent_id).await;
        }
        for task in &tasks {
            self.broadcast_task_event(
                cordy_protocol::EVENT_TASK_CANCELLED,
                task,
                Default::default(),
            )
            .await;
        }
        self.notify_tasks_finished(&tasks);
        Ok(())
    }

    /// Settles a QUEUED chat task's input batch inside the caller's
    /// transaction: channel-ingested batches get a "Stopped." assistant row;
    /// direct-chat batches are deleted (attachments detached first for edit).
    async fn settle_queued_chat_input(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        task: &AgentTaskQueue,
        action: &str,
    ) -> Result<Option<CancelledChatMessageResult>, TaskServiceError> {
        let Some(chat_session_id) = task.chat_session_id else {
            return Ok(None);
        };
        let input_owner_id = chat_input_owner_id(task);
        let channel_ingested = task_has_channel_ingested_messages(&mut **tx, input_owner_id)
            .await
            .map_err(|e| {
                TaskServiceError::Internal(format!("check queued chat channel provenance: {e}"))
            })?
            .unwrap_or(false);
        if channel_ingested {
            create_assistant_chat_message_typed(
                tx,
                chat_session_id,
                "Stopped.",
                task.id,
                compute_chat_elapsed_ms(task.completed_at, task.created_at),
                None,
                None,
            )
            .await?;
            return Ok(None);
        }

        let detached = if action == "edit" {
            detach_attachments_from_user_chat_message_by_task(&mut **tx, input_owner_id)
                .await
                .map_err(|e| {
                    TaskServiceError::Internal(format!(
                        "detach edited queued chat attachments: {e}"
                    ))
                })?
        } else {
            vec![]
        };
        // Release the adopted kickoff together with the delete: leaving it bound
        // to the dead task strands the onboarding context (MUL-5827).
        release_onboarding_kickoff_from_task(&mut **tx, input_owner_id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("release onboarding kickoff: {e}")))?;
        let deleted = delete_user_chat_message_by_task(&mut **tx, input_owner_id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("delete queued chat input: {e}")));
        let deleted = match deleted {
            Ok(Some(d)) => d,
            Ok(None) => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut cancelled = CancelledChatMessageResult {
            chat_session_id: deleted.chat_session_id.to_string(),
            message_id: deleted.id.to_string(),
            content: deleted.content.clone(),
            restore_to_input: false,
            attachments: vec![],
        };
        if action == "remove" {
            return Ok(Some(cancelled));
        }

        let attachment_ids: Vec<Uuid> = detached.iter().map(|a| a.id).collect();
        create_chat_draft_restore(
            &mut **tx,
            deleted.id,
            chat_session_id,
            task.id,
            &deleted.content,
            attachment_ids,
        )
        .await
        .map_err(|e| {
            TaskServiceError::Internal(format!("create queued chat draft restore: {e}"))
        })?;
        cancelled.restore_to_input = true;
        cancelled.attachments = detached;
        Ok(Some(cancelled))
    }

    /// Terminal-path chat settle for a CANCELLED task: empty transcript may be
    /// restorable (or deferred for started tasks behind a draft-restore-capable
    /// client); non-empty gets a "Stopped." assistant row. Errors are logged
    /// and swallowed — the cancellation itself already committed.
    async fn finalize_cancelled_chat_message(
        &self,
        task: &AgentTaskQueue,
        opts: &CancelTaskOptions,
    ) -> Option<CancelledChatMessageResult> {
        let chat_session_id = task.chat_session_id?;
        let mut cancelled = None;
        let result: Result<(), TaskServiceError> = async {
            let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;
            // Same protocol as every other terminal path: session row first.
            let _ = lock_chat_session_for_task(&mut *tx, task.id)
                .await
                .map_err(|e| {
                    TaskServiceError::Internal(format!("lock chat session for task write: {e}"))
                })?;
            let messages = list_task_messages(&mut *tx, task.id).await.map_err(|e| {
                TaskServiceError::Internal(format!("list cancelled chat task messages: {e}"))
            })?;
            let mut restorable = messages.is_empty();
            if restorable {
                // Channel-ingested user messages are the durable record of what
                // the platform sender wrote — no composer to restore into. The
                // gate is the immutable per-message stamp, keyed by the
                // input-batch owner id so retry clones reach the parent's
                // verdict.
                let channel_ingested =
                    task_has_channel_ingested_messages(&mut *tx, chat_input_owner_id(task))
                        .await
                        .map_err(|e| {
                            TaskServiceError::Internal(format!(
                                "check cancelled chat channel provenance: {e}"
                            ))
                        })?
                        .unwrap_or(false);
                restorable = !channel_ingested;
            }
            if restorable && task.started_at.is_some() && opts.client_supports_draft_restore {
                // A started task's daemon learns of the cancellation by polling
                // and may still be flushing its transcript tail, so "empty" is
                // not trustworthy yet. Defer until the daemon acks or the
                // sweeper grace period expires (#5219).
                mark_chat_finalize_deferred(&mut *tx, task.id)
                    .await
                    .map_err(|e| {
                        TaskServiceError::Internal(format!("mark chat finalize deferred: {e}"))
                    })?;
                return Ok(());
            }
            if restorable {
                let input_owner_id = chat_input_owner_id(task);
                // Detach attachments BEFORE deleting the user message — the
                // attachment FK is ON DELETE CASCADE.
                let detached =
                    detach_attachments_from_user_chat_message_by_task(&mut *tx, input_owner_id)
                        .await
                        .map_err(|e| {
                            TaskServiceError::Internal(format!(
                                "detach cancelled chat message attachments: {e}"
                            ))
                        })?;
                release_onboarding_kickoff_from_task(&mut *tx, input_owner_id)
                    .await
                    .map_err(|e| {
                        TaskServiceError::Internal(format!("release onboarding kickoff: {e}"))
                    })?;
                let deleted = delete_user_chat_message_by_task(&mut *tx, input_owner_id)
                    .await
                    .map_err(|e| {
                        TaskServiceError::Internal(format!(
                            "delete empty cancelled chat user message: {e}"
                        ))
                    });
                let Some(deleted) = deleted.ok().flatten() else {
                    return Ok(());
                };
                // Always restorable now: the delete cannot return a kickoff row,
                // so what comes back is always what the member typed (MUL-5827).
                cancelled = Some(CancelledChatMessageResult {
                    chat_session_id: deleted.chat_session_id.to_string(),
                    message_id: deleted.id.to_string(),
                    content: deleted.content,
                    restore_to_input: true,
                    attachments: detached,
                });
                return Ok(());
            }
            create_assistant_chat_message_typed(
                &mut tx,
                chat_session_id,
                "Stopped.",
                task.id,
                compute_chat_elapsed_ms(task.completed_at, task.created_at),
                None,
                None,
            )
            .await?;
            Ok(())
        }
        .await;
        if let Err(err) = result {
            tracing::error!(
                task_id = %task.id,
                error = %err,
                "failed to finalize cancelled chat message"
            );
            return None;
        }
        cancelled
    }

    /// Re-announces an already-cancelled task after a post-terminal delivery
    /// landed on its row. Consumers treat task:cancelled as idempotent cache
    /// invalidation, so a replay is safe.
    pub async fn rebroadcast_cancelled_task(&self, task_id: Uuid) {
        let Ok(Some(task)) = get_agent_task(&self.pool, task_id).await else {
            tracing::warn!(task_id = %task_id, "rebroadcast cancelled task: load failed");
            return;
        };
        if task.status != "cancelled" {
            // A complete/fail callback already announced its own terminal event.
            return;
        }
        self.broadcast_task_event(
            cordy_protocol::EVENT_TASK_CANCELLED,
            &task,
            Default::default(),
        )
        .await;
    }

    /// Settles the empty/non-empty judgment deferred for a started-but-empty
    /// cancelled chat task (#5219). Called from the daemon's cancel-ack and the
    /// sweeper fallback; the marker claim is atomic so concurrent callers
    /// cannot finalize twice. Outcome broadcasts as chat:cancel_finalized.
    pub async fn finalize_deferred_cancelled_chat(&self, task_id: Uuid) {
        let mut payload = cordy_protocol::ChatCancelFinalizedPayload {
            outcome: String::new(),
            chat_session_id: String::new(),
            task_id: String::new(),
            initiator_user_id: String::new(),
            message_id: String::new(),
            content: String::new(),
            message_kind: String::new(),
            created_at: String::new(),
            elapsed_ms: 0,
        };
        let mut settled_task: Option<AgentTaskQueue> = None;
        let mut settled = false;
        let result: Result<(), TaskServiceError> = async {
            let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;
            // Lock the task's chat_session first: chat_draft_restore has no FK
            // (MUL-3515), so without this a concurrent sweep could orphan our
            // restore row holding the user's prompt. Also fixes the global
            // chat_session → agent_task_queue lock order.
            let session_gone = match lock_chat_session_for_task(&mut *tx, task_id).await {
                Ok(_) => false,
                Err(e) => {
                    if is_no_rows(&e) {
                        true
                    } else {
                        return Err(TaskServiceError::Internal(format!(
                            "lock chat session for deferred finalize: {e}"
                        )));
                    }
                }
            };

            // Claim the marker inside the settlement tx: a failed settlement
            // rolls the claim back so the sweeper can retry.
            let claimed = claim_chat_finalize_deferred(&mut *tx, task_id)
                .await
                .map_err(|e| {
                    TaskServiceError::Internal(format!("claim deferred chat finalize: {e}"))
                })?;
            let Some(claimed) = claimed else {
                return Ok(());
            };
            if session_gone || claimed.chat_session_id.is_none() {
                return Ok(());
            }
            let chat_session_id = claimed.chat_session_id.expect("checked above");
            settled = true;
            settled_task = Some(claimed.clone());
            payload.chat_session_id = chat_session_id.to_string();
            payload.task_id = claimed.id.to_string();
            payload.initiator_user_id = claimed
                .initiator_user_id
                .map(|u| u.to_string())
                .unwrap_or_default();

            let messages = list_task_messages(&mut *tx, claimed.id)
                .await
                .map_err(|e| {
                    TaskServiceError::Internal(format!("list cancelled chat task messages: {e}"))
                })?;
            let mut restorable = messages.is_empty();
            if restorable {
                // Same immutable-provenance guard as the sync path; covers
                // markers created by an older replica during a rolling deploy.
                let channel_ingested =
                    task_has_channel_ingested_messages(&mut *tx, chat_input_owner_id(&claimed))
                        .await
                        .map_err(|e| {
                            TaskServiceError::Internal(format!(
                                "check cancelled chat channel provenance: {e}"
                            ))
                        })?
                        .unwrap_or(false);
                restorable = !channel_ingested;
            }
            if restorable {
                let input_owner_id = chat_input_owner_id(&claimed);
                let detached =
                    detach_attachments_from_user_chat_message_by_task(&mut *tx, input_owner_id)
                        .await
                        .map_err(|e| {
                            TaskServiceError::Internal(format!(
                                "detach cancelled chat message attachments: {e}"
                            ))
                        })?;
                release_onboarding_kickoff_from_task(&mut *tx, input_owner_id)
                    .await
                    .map_err(|e| {
                        TaskServiceError::Internal(format!("release onboarding kickoff: {e}"))
                    })?;
                let deleted = delete_user_chat_message_by_task(&mut *tx, input_owner_id)
                    .await
                    .map_err(|e| {
                        TaskServiceError::Internal(format!(
                            "delete empty cancelled chat user message: {e}"
                        ))
                    });
                let Some(deleted) = deleted.ok().flatten() else {
                    payload.outcome = String::new();
                    return Ok(());
                };
                let attachment_ids: Vec<Uuid> = detached.iter().map(|a| a.id).collect();
                create_chat_draft_restore(
                    &mut *tx,
                    deleted.id,
                    chat_session_id,
                    claimed.id,
                    &deleted.content,
                    attachment_ids,
                )
                .await
                .map_err(|e| {
                    TaskServiceError::Internal(format!("create chat draft restore: {e}"))
                })?;
                payload.outcome = cordy_protocol::CHAT_CANCEL_OUTCOME_RESTORED.to_string();
                payload.message_id = deleted.id.to_string();
                return Ok(());
            }
            let row = create_assistant_chat_message_typed(
                &mut tx,
                chat_session_id,
                "Stopped.",
                claimed.id,
                compute_chat_elapsed_ms(claimed.completed_at, claimed.created_at),
                None,
                None,
            )
            .await?;
            payload.outcome = cordy_protocol::CHAT_CANCEL_OUTCOME_STOPPED.to_string();
            payload.message_id = row.id.to_string();
            payload.content = row.content.clone();
            payload.message_kind = row.message_kind.clone();
            payload.created_at = row
                .created_at
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
            payload.elapsed_ms = compute_chat_elapsed_ms(claimed.completed_at, claimed.created_at)
                .unwrap_or_default();
            Ok(())
        }
        .await;
        if let Err(err) = result {
            tracing::error!(task_id = %task_id, error = %err, "failed to finalize deferred cancelled chat");
            return;
        }
        if !settled || payload.outcome.is_empty() {
            return;
        }
        if let Some(task) = settled_task {
            self.broadcast_chat_cancel_finalized(&task, payload).await;
        }
    }

    async fn broadcast_chat_cancel_finalized(
        &self,
        task: &AgentTaskQueue,
        payload: cordy_protocol::ChatCancelFinalizedPayload,
    ) {
        let Some(workspace_id) = self.resolve_task_workspace_id(task).await else {
            return;
        };
        self.bus.publish(&cordy_events::Event {
            event_type: cordy_protocol::EVENT_CHAT_CANCEL_FINALIZED.to_string(),
            workspace_id,
            actor_type: "system".to_string(),
            actor_id: String::new(),
            payload: serde_json::to_value(payload).unwrap_or_default(),
            task_id: task.id.to_string(),
            chat_session_id: task
                .chat_session_id
                .map(|u| u.to_string())
                .unwrap_or_default(),
        });
    }

    // --- Claim family ------------------------------------------------------------

    /// Atomically claims the next queued task for an agent on its current
    /// runtime, respecting max_concurrent_tasks.
    pub async fn claim_task(
        &self,
        agent_id: Uuid,
    ) -> Result<Option<AgentTaskQueue>, TaskServiceError> {
        self.claim_task_scoped(agent_id, None).await
    }

    /// Runtime-scoped claim primitive used by daemon poll paths. Scoping the
    /// SQL claim itself prevents an offline candidate on runtime A from causing
    /// the same agent's task on runtime B to be dispatched then dropped.
    async fn claim_task_scoped(
        &self,
        agent_id: Uuid,
        runtime_id: Option<Uuid>,
    ) -> Result<Option<AgentTaskQueue>, TaskServiceError> {
        let start = std::time::Instant::now();
        let outcome = ClaimOutcome::default();
        let claimed = self.claim_once(agent_id, runtime_id, &outcome).await;
        let total_ms = start.elapsed().as_millis() as i64;
        if total_ms >= 300 {
            tracing::info!(
                agent_id = %agent_id,
                outcome = outcome.get(),
                total_ms,
                "claim_task slow"
            );
        }
        claimed
    }

    async fn claim_once(
        &self,
        agent_id: Uuid,
        runtime_id: Option<Uuid>,
        outcome: &ClaimOutcome,
    ) -> Result<Option<AgentTaskQueue>, TaskServiceError> {
        let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;
        let agent = get_agent_for_claim_update(&mut *tx, agent_id)
            .await
            .map_err(|e| {
                outcome.set("error_get_agent");
                TaskServiceError::Internal(format!("agent not found: {e}"))
            })?
            .ok_or_else(|| {
                outcome.set("error_get_agent");
                TaskServiceError::Internal("agent not found".into())
            })?;
        let claim_runtime_id = runtime_id.or(agent.runtime_id);
        let Some(claim_runtime_id) = claim_runtime_id else {
            outcome.set("no_runtime");
            return Ok(None);
        };

        let running = count_running_tasks(&mut *tx, agent_id)
            .await
            .map_err(|e| {
                outcome.set("error_count_running");
                TaskServiceError::Internal(format!("count running tasks: {e}"))
            })?
            .unwrap_or(0);
        if running >= agent.max_concurrent_tasks as i64 {
            tracing::debug!(
                agent_id = %agent_id,
                running,
                max = agent.max_concurrent_tasks,
                "task claim: no capacity"
            );
            outcome.set("no_capacity");
            return Ok(None);
        }

        let claimed = claim_agent_task(
            &mut *tx,
            PREPARE_LEASE_DURATION.as_secs_f64(),
            agent_id,
            claim_runtime_id,
            RUNTIME_CLAIM_FRESHNESS_SECONDS,
        )
        .await;
        let claimed = match claimed {
            Ok(Some(t)) => t,
            Ok(None) => {
                tracing::debug!(agent_id = %agent_id, "task claim: no tasks available");
                outcome.set("no_tasks");
                return Ok(None);
            }
            Err(e) => {
                outcome.set("error_claim");
                return Err(TaskServiceError::Sql(downcast_sqlx(e)));
            }
        };

        // An idle task-owned direct-chat row may already be visible as the
        // positional queue head; this claim-time reanchor is the compatibility
        // fallback for an older or out-of-order row.
        if claimed.chat_session_id.is_some() && claimed.chat_input_task_id == Some(claimed.id) {
            reanchor_claimed_direct_chat_input(&mut *tx, claimed.dispatched_at, claimed.id)
                .await
                .map_err(|e| {
                    outcome.set("error_reanchor_chat_input");
                    TaskServiceError::Internal(format!("reanchor claimed direct chat input: {e}"))
                })?;
        }

        tx.commit().await.map_err(|e| {
            outcome.set("error_transaction");
            TaskServiceError::Sql(e)
        })?;

        tracing::info!(task_id = %claimed.id, agent_id = %agent_id, "task claimed");
        self.capture_task_dispatched(&claimed).await;
        self.reconcile_agent_status(agent_id).await;
        self.broadcast_task_dispatch(&claimed).await;
        outcome.set("claimed");
        Ok(Some(claimed))
    }

    /// Claims the next runnable task for a runtime while respecting each
    /// agent's max_concurrent_tasks limit, with promote/reclaim/empty-cache
    /// fast paths.
    pub async fn claim_task_for_runtime(
        &self,
        runtime_id: Uuid,
    ) -> Result<Option<AgentTaskQueue>, TaskServiceError> {
        let start = std::time::Instant::now();
        self.promote_due_deferred_tasks_for_runtime(runtime_id)
            .await?;

        // Check before EmptyClaim: a lost claim response moves the task out of
        // `queued`, so the empty-queued cache cannot represent recoverability.
        let stale = reclaim_stale_dispatched_task_for_runtime(
            &self.pool,
            runtime_id,
            PREPARE_LEASE_DURATION.as_secs_f64(),
            CLAIM_RESPONSE_RECOVERY_WINDOW.as_secs_f64(),
            RUNTIME_CLAIM_FRESHNESS_SECONDS,
        )
        .await;
        match stale {
            Ok(Some(stale)) => {
                tracing::info!(
                    task_id = %stale.id,
                    runtime_id = %runtime_id,
                    agent_id = %stale.agent_id,
                    "stale dispatched task reclaimed"
                );
                return Ok(Some(stale));
            }
            Ok(None) => {}
            Err(e) => {
                return Err(TaskServiceError::Internal(format!(
                    "reclaim stale dispatched task: {e}"
                )))
            }
        }

        // EmptyClaim fast path lands here once the Redis-backed cache ports;
        // without it every claim goes through the DB, which is correct.

        let tasks = list_queued_claim_candidates_by_runtime(&self.pool, runtime_id)
            .await
            .map_err(|e| {
                TaskServiceError::Internal(format!("list queued claim candidates: {e}"))
            })?;

        let mut tried_agents = std::collections::HashSet::new();
        for candidate in &tasks {
            if !tried_agents.insert(candidate.agent_id) {
                continue;
            }
            let task = self
                .claim_task_scoped(candidate.agent_id, Some(runtime_id))
                .await?;
            if let Some(task) = task {
                if task.runtime_id == Some(runtime_id) {
                    let total_ms = start.elapsed().as_millis() as i64;
                    if total_ms >= 300 {
                        tracing::info!(runtime_id = %runtime_id, total_ms, "claim_for_runtime slow");
                    }
                    return Ok(Some(task));
                }
            }
        }
        let total_ms = start.elapsed().as_millis() as i64;
        if total_ms >= 300 {
            tracing::info!(runtime_id = %runtime_id, total_ms, "claim_for_runtime slow");
        }
        Ok(None)
    }

    /// Immediately releases an exact dispatched claim whose payload
    /// finalization failed before the HTTP response was written. Not a fresh
    /// enqueue: do not duplicate queued analytics.
    pub async fn requeue_task_after_claim_failure(
        &self,
        task: &AgentTaskQueue,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        let requeued = requeue_agent_task_after_claim_failure(
            &self.pool,
            task.id,
            task.runtime_id.unwrap_or_else(Uuid::nil),
            task.dispatched_at,
        )
        .await
        .map_err(|e| TaskServiceError::Internal(format!("requeue task after claim failure: {e}")))?
        .ok_or(TaskServiceError::AgentNoRuntime)?;
        self.reconcile_agent_status(requeued.agent_id).await;
        self.broadcast_task_event(
            cordy_protocol::EVENT_TASK_QUEUED,
            &requeued,
            Default::default(),
        )
        .await;
        self.notify_runtime_may_have_work(requeued.runtime_id, Some(&requeued.id.to_string()));
        tracing::info!(
            task_id = %requeued.id,
            runtime_id = ?requeued.runtime_id,
            "task requeued after claim finalization failure"
        );
        Ok(requeued)
    }

    /// Machine-level batch counterpart of claim_task_for_runtime (MUL-4257):
    /// claims up to maxTasks across every runtime in one call. Preserves
    /// per-runtime semantics set-ified, with partial-success returns so a late
    /// failure never drops already-dispatched work.
    pub async fn claim_tasks_for_runtimes(
        &self,
        runtime_ids: Vec<Uuid>,
        max_tasks: usize,
    ) -> Result<Vec<AgentTaskQueue>, TaskServiceError> {
        if runtime_ids.is_empty() || max_tasks == 0 {
            return Ok(vec![]);
        }

        // De-dup defensively so bookkeeping stays unambiguous.
        let mut unique_ids = Vec::with_capacity(runtime_ids.len());
        let mut seen = std::collections::HashSet::new();
        let runtime_in_set: std::collections::HashSet<Uuid> = runtime_ids.iter().copied().collect();
        for rid in runtime_ids {
            if seen.insert(rid) {
                unique_ids.push(rid);
            }
        }

        let mut claimed: Vec<AgentTaskQueue> = Vec::with_capacity(max_tasks);

        // 1. Promote due deferred tasks across the whole set (promote-first).
        self.cancel_superseded_deferred_retries(&unique_ids).await;
        let promoted = match promote_due_deferred_tasks_for_runtimes(
            &self.pool,
            unique_ids.clone(),
            RUNTIME_CLAIM_FRESHNESS_SECONDS,
        )
        .await
        {
            Ok(tasks) => tasks,
            Err(e) => {
                if is_duplicate_pending_task_anyhow(&e) {
                    // One contended row must not fail the claim for EVERY
                    // runtime in the batch; promote nothing this tick.
                    tracing::info!("promote deferred tasks (batch): slot taken by a concurrent enqueue, skipping this tick");
                    vec![]
                } else {
                    return Err(TaskServiceError::Internal(format!(
                        "promote deferred tasks: {e}"
                    )));
                }
            }
        };
        for task in promoted {
            tracing::info!(
                task_id = %task.id,
                runtime_id = ?task.runtime_id,
                agent_id = %task.agent_id,
                "deferred fallback task promoted (batch)"
            );
            self.broadcast_task_event(cordy_protocol::EVENT_TASK_QUEUED, &task, Default::default())
                .await;
            self.notify_task_enqueued(&task).await;
        }

        // 2. Reclaim lost-response dispatched tasks across the set.
        let reclaimed = reclaim_stale_dispatched_tasks_for_runtimes(
            &self.pool,
            PREPARE_LEASE_DURATION.as_secs_f64(),
            unique_ids.clone(),
            CLAIM_RESPONSE_RECOVERY_WINDOW.as_secs_f64(),
            RUNTIME_CLAIM_FRESHNESS_SECONDS,
            max_tasks as i32,
        )
        .await
        .map_err(|e| TaskServiceError::Internal(format!("reclaim stale dispatched tasks: {e}")))?;
        for r in reclaimed {
            tracing::info!(
                task_id = %r.id,
                runtime_id = ?r.runtime_id,
                agent_id = %r.agent_id,
                "stale dispatched task reclaimed (batch)"
            );
            claimed.push(r);
        }
        if claimed.len() >= max_tasks {
            claimed.truncate(max_tasks);
            return Ok(claimed);
        }

        // 3-5. Candidate SELECT across the set. (EmptyClaim short-circuit and
        // MarkEmpty land with the Redis-backed cache port.)
        let candidates =
            list_queued_claim_candidates_by_runtimes(&self.pool, unique_ids.clone()).await;
        let candidates = match candidates {
            Ok(c) => c,
            Err(e) => {
                // Partial success: hand back what committed so the handler
                // finalizes and returns it (MUL-4257).
                if !claimed.is_empty() {
                    tracing::error!(error = %e, claimed = claimed.len(), "batch claim: candidate query failed after partial success; returning claimed tasks to avoid loss");
                    return Ok(claimed);
                }
                return Err(TaskServiceError::Internal(format!(
                    "list queued claim candidates: {e}"
                )));
            }
        };

        // 6. Claim per distinct agent through the runtime-scoped helper.
        let mut tried_agents = std::collections::HashSet::new();
        for candidate in &candidates {
            if claimed.len() >= max_tasks {
                break;
            }
            if !tried_agents.insert(candidate.agent_id) {
                continue;
            }
            let task = self
                .claim_task_scoped(candidate.agent_id, candidate.runtime_id)
                .await;
            let task = match task {
                Ok(t) => t,
                Err(e) => {
                    if !claimed.is_empty() {
                        tracing::error!(error = %e, claimed = claimed.len(), "batch claim: claim task failed after partial success; returning claimed tasks to avoid loss");
                        return Ok(claimed);
                    }
                    return Err(e);
                }
            };
            let Some(task) = task else {
                continue;
            };
            // Defensive contract check: the SQL claim is scoped to the
            // candidate runtime; a future query change must not route work to
            // a runtime this daemon does not host.
            if let Some(rt) = task.runtime_id {
                if !runtime_in_set.contains(&rt) {
                    continue;
                }
            }
            claimed.push(task);
        }

        Ok(claimed)
    }

    /// Drops deferred auto-retry rows that an active task already supersedes,
    /// immediately before promotion. Best-effort.
    async fn cancel_superseded_deferred_retries(&self, runtime_ids: &[Uuid]) {
        if runtime_ids.is_empty() {
            return;
        }
        let cancelled =
            cancel_superseded_deferred_retries_for_runtimes(&self.pool, runtime_ids.to_vec()).await;
        let Ok(cancelled) = cancelled else {
            return;
        };
        for task in cancelled {
            tracing::info!(
                task_id = %task.id,
                issue_id = ?task.issue_id,
                agent_id = %task.agent_id,
                "deferred auto-retry cancelled: superseded by an active task"
            );
            self.capture_task_cancelled(&task).await;
            self.reconcile_agent_status(task.agent_id).await;
            self.broadcast_task_event(
                cordy_protocol::EVENT_TASK_CANCELLED,
                &task,
                Default::default(),
            )
            .await;
        }
    }

    pub async fn promote_due_deferred_tasks_for_runtime(
        &self,
        runtime_id: Uuid,
    ) -> Result<(), TaskServiceError> {
        self.cancel_superseded_deferred_retries(&[runtime_id]).await;
        let tasks = promote_due_deferred_tasks_for_runtime(
            &self.pool,
            runtime_id,
            RUNTIME_CLAIM_FRESHNESS_SECONDS,
        )
        .await;
        let tasks = match tasks {
            Ok(t) => t,
            Err(e) => {
                if is_duplicate_pending_task_anyhow(&e) {
                    // The NOT EXISTS fence inside the query cannot see an
                    // enqueue that has not committed yet; one row losing its
                    // slot must not fail the whole claim. Costs one poll
                    // interval, never a stall.
                    tracing::info!(runtime_id = %runtime_id, "promote due deferred tasks: slot taken by a concurrent enqueue, skipping this tick");
                    return Ok(());
                }
                return Err(TaskServiceError::Internal(format!(
                    "promote due deferred tasks: {e}"
                )));
            }
        };
        for task in tasks {
            tracing::info!(
                task_id = %task.id,
                runtime_id = %runtime_id,
                agent_id = %task.agent_id,
                "deferred fallback task promoted"
            );
            self.broadcast_task_event(cordy_protocol::EVENT_TASK_QUEUED, &task, Default::default())
                .await;
            self.notify_task_enqueued(&task).await;
        }
        Ok(())
    }

    /// Atomically persists the task-scoped agent token, an optional
    /// short-lived daemon token used by the Remote MCP broker, and, for a
    /// comment-backed task, the exact comment ids embedded in the response.
    /// The handler must call this only after the full payload has been built
    /// and before writing any response bytes; a failure rolls every write back
    /// so the claim can be safely returned to the queue.
    pub async fn finalize_task_claim(
        &self,
        task: &AgentTaskQueue,
        token: CreateTaskToken,
        daemon_token: Option<CreateDaemonToken>,
        delivered_comment_ids: Vec<Uuid>,
        record_comment_receipt: bool,
    ) -> Result<Vec<Uuid>, TaskServiceError> {
        let mut receipt = task.delivered_comment_ids.clone();
        let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;
        create_task_token(
            &mut *tx,
            &token.token_hash,
            token.task_id,
            token.agent_id,
            token.workspace_id,
            token.user_id,
            token.expires_at,
            new_v7(),
        )
        .await
        .map_err(|e| TaskServiceError::Internal(format!("create task token: {e}")))?;
        if let Some(dt) = &daemon_token {
            // Opportunistic bounded cleanup keeps short-lived per-task daemon
            // credentials from accumulating without adding another sweeper.
            delete_expired_daemon_tokens(&mut *tx).await.map_err(|e| {
                TaskServiceError::Internal(format!("delete expired daemon tokens: {e}"))
            })?;
            create_daemon_token(
                &mut *tx,
                &dt.token_hash,
                dt.workspace_id,
                &dt.daemon_id,
                dt.expires_at,
            )
            .await
            .map_err(|e| {
                TaskServiceError::Internal(format!("create remote MCP daemon token: {e}"))
            })?;
        }
        if record_comment_receipt {
            let persisted = set_task_delivered_comment_i_ds(
                &mut *tx,
                delivered_comment_ids,
                task.id,
                task.runtime_id.unwrap_or_else(Uuid::nil),
                task.dispatched_at,
                task.trigger_comment_id.unwrap_or_else(Uuid::nil),
            )
            .await
            .map_err(|e| TaskServiceError::Internal(format!("set delivered comment ids: {e}")))?;
            if let Some(ids) = persisted.into_iter().next().flatten() {
                receipt = ids;
            }
        }
        tx.commit().await.map_err(TaskServiceError::Sql)?;
        Ok(receipt)
    }
}

/// Cell carrying the claim outcome label for the slow-log path.
#[derive(Default)]
struct ClaimOutcome(std::sync::Mutex<String>);

impl ClaimOutcome {
    fn set(&self, v: &str) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = v.to_string();
    }
    fn get(&self) -> String {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn is_no_rows(err: &anyhow::Error) -> bool {
    err.downcast_ref::<sqlx::Error>()
        .map(|e| matches!(e, sqlx::Error::RowNotFound))
        .unwrap_or(false)
}

/// Writes an assistant outcome and reanchors the newly-visible queued direct
/// head in the caller's transaction — the transcript-order boundary. Callers
/// MUST observe the settling task outside the visible-head status set first.
pub(crate) async fn create_assistant_chat_message_typed(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    chat_session_id: Uuid,
    content: &str,
    task_id: Uuid,
    elapsed_ms: Option<i64>,
    message_kind: Option<&str>,
    failure_reason: Option<&str>,
) -> Result<ChatMessage, TaskServiceError> {
    let row = create_chat_message(
        &mut **tx,
        chat_session_id,
        "assistant",
        content,
        Some(task_id),
        failure_reason,
        elapsed_ms,
        Some(message_kind.unwrap_or(cordy_protocol::CHAT_MESSAGE_KIND_MESSAGE)),
        &serde_json::Value::Array(vec![]),
        None,
        None,
        new_v7(),
    )
    .await
    .map_err(|e| TaskServiceError::Internal(format!("create assistant chat message: {e}")))?
    .ok_or_else(|| TaskServiceError::Internal("create assistant chat message: no row".into()))?;
    reanchor_next_queued_direct_chat_input(&mut **tx, chat_session_id, Some(row.created_at))
        .await
        .map_err(|e| {
            TaskServiceError::Internal(format!("reanchor next queued direct chat input: {e}"))
        })?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// Slice4a — task.go 3579-3694 (start / escalation-cancel / prepare-lease /
// waiting_local_directory). CompleteTask and beyond live in task_terminal.rs.
// ---------------------------------------------------------------------------

impl TaskService {
    /// StartTask transitions a dispatched task to running
    /// (task.go 3579-3601). Issue status is NOT changed here — the agent
    /// manages it via the CLI.
    pub async fn start_task(&self, task_id: Uuid) -> Result<AgentTaskQueue, TaskServiceError> {
        let task = start_agent_task(&self.pool, task_id)
            .await
            .map_err(downcast_sqlx)
            .map_err(|e| TaskServiceError::Internal(format!("start task: {e}")))?
            .ok_or_else(|| TaskServiceError::Internal("start task: no row written".into()))?;
        self.cancel_deferred_escalations_for_task(task.id).await;

        tracing::info!(task_id = %task.id, issue_id = ?task.issue_id, "task started");
        self.capture_task_started(&task).await;
        // A local-directory waiter was reconciled out of the persisted working
        // status while parked. Restore working as soon as it enters running;
        // the normal dispatched -> running path is already working, so this is
        // intentionally idempotent there.
        self.reconcile_agent_status(task.agent_id).await;
        self.broadcast_task_event(
            cordy_protocol::EVENT_TASK_RUNNING,
            &task,
            Default::default(),
        )
        .await;
        Ok(task)
    }

    /// cancelDeferredEscalationsForTask cancels the deferred fallback
    /// (escalation) tasks waiting behind a task that just started — the
    /// primary acknowledged the work. Best-effort.
    async fn cancel_deferred_escalations_for_task(&self, primary_task_id: Uuid) {
        let Ok(cancelled) = cancel_deferred_escalations_for_task(&self.pool, primary_task_id).await
        else {
            return;
        };
        for task in cancelled {
            tracing::info!(
                task_id = %task.id,
                primary_task_id = %primary_task_id,
                reason = "primary_acknowledged",
                "deferred fallback task cancelled"
            );
        }
    }

    /// CancelDeferredEscalationsForIssueAgent (task.go 3618-3638).
    pub async fn cancel_deferred_escalations_for_issue_agent(
        &self,
        issue_id: Uuid,
        agent_id: Uuid,
    ) {
        match cancel_deferred_escalations_for_issue_agent(&self.pool, issue_id, agent_id).await {
            Ok(cancelled) => {
                for task in cancelled {
                    tracing::info!(
                        task_id = ?task.id,
                        issue_id = %issue_id,
                        agent_id = %agent_id,
                        reason = "agent_comment_acknowledged",
                        "deferred fallback task cancelled"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(%issue_id, %agent_id, error = %e, "cancel deferred escalations for issue agent failed")
            }
        }
    }

    /// ExtendTaskPrepareLease keeps a claimed-but-not-started task protected
    /// while the daemon resolves cached inputs and prepares the execution
    /// environment (task.go 3642-3652).
    pub async fn extend_task_prepare_lease(
        &self,
        task_id: Uuid,
        runtime_id: Uuid,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        extend_agent_task_prepare_lease(
            &self.pool,
            task_id,
            runtime_id,
            PREPARE_LEASE_DURATION.as_secs_f64(),
        )
        .await
        .map_err(downcast_sqlx)
        .map_err(|e| TaskServiceError::Internal(format!("extend task prepare lease: {e}")))?
        .ok_or_else(|| TaskServiceError::Internal("extend task prepare lease: no row".to_string()))
    }

    /// MarkTaskWaitingLocalDirectory parks a dispatched task in the
    /// waiting_local_directory state while the daemon waits for another
    /// in-flight task to release the project_resource path lock
    /// (task.go 3660-3683).
    pub async fn mark_task_waiting_local_directory(
        &self,
        task_id: Uuid,
        reason: &str,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        let reason = reason.trim();
        let task = mark_agent_task_waiting_local_directory(
            &self.pool,
            task_id,
            (!reason.is_empty()).then_some(reason),
            PREPARE_LEASE_DURATION.as_secs_f64(),
        )
        .await
        .map_err(downcast_sqlx)
        .map_err(|e| TaskServiceError::Internal(format!("mark task waiting_local_directory: {e}")))?
        .ok_or_else(|| {
            TaskServiceError::Internal("mark task waiting_local_directory: no row".into())
        })?;

        tracing::info!(task_id = %task.id, issue_id = ?task.issue_id, reason, "task waiting_local_directory");
        // waiting_local_directory is owned/queued work, not executing work. The
        // claim path marked the agent working while the row was dispatched, so
        // reconcile immediately when it parks instead of leaving that persisted
        // status stale until a terminal transition.
        self.reconcile_agent_status(task.agent_id).await;
        self.broadcast_task_event(
            cordy_protocol::EVENT_TASK_WAITING_LOCAL_DIRECTORY,
            &task,
            Default::default(),
        )
        .await;
        Ok(task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_fixture() -> AgentTaskQueue {
        let id = Uuid::nil();
        AgentTaskQueue {
            id,
            agent_id: id,
            accountable_user_id: None,
            attempt: 1,
            autopilot_run_id: None,
            branch_name: None,
            chat_finalize_deferred_at: None,
            chat_input_task_id: None,
            chat_session_id: None,
            coalesced_comment_ids: vec![],
            completed_at: None,
            context: None,
            created_at: chrono::DateTime::parse_from_rfc3339("2026-08-20T12:00:00Z")
                .expect("valid ts")
                .with_timezone(&chrono::Utc),
            delegated_from_task_id: None,
            delivered_comment_ids: vec![],
            dispatched_at: None,
            durable_work_dir: None,
            error: None,
            escalation_for_task_id: None,
            failure_reason: None,
            fire_at: None,
            force_fresh_session: false,
            handoff_note: None,
            initiator_user_id: None,
            is_leader_task: false,
            issue_id: None,
            max_attempts: 3,
            originator_source: None,
            originator_user_id: None,
            parent_task_id: None,
            prepare_lease_expires_at: None,
            priority: 0,
            quick_actions_disabled: false,
            regenerate_quick_actions_for: None,
            rerun_of_task_id: None,
            result: None,
            retired_session_id: None,
            retry_of_task_id: None,
            rule_version_id: None,
            runtime_connected_apps: None,
            runtime_id: None,
            runtime_mcp_overlay: None,
            session_id: None,
            session_rollout_missing: false,
            squad_id: None,
            started_at: None,
            status: "queued".to_string(),
            trigger_comment_id: None,
            trigger_evidence_kind: None,
            trigger_evidence_ref_id: None,
            trigger_summary: None,
            wait_reason: None,
            work_dir: None,
        }
    }

    #[test]
    fn duration_seconds_matches_go_semantics() {
        // Either side missing → -1 (caller skips the observation).
        assert_eq!(duration_seconds(None, None), -1.0);
        let start = chrono::DateTime::parse_from_rfc3339("2026-08-20T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let end = start + chrono::Duration::seconds(42);
        assert_eq!(duration_seconds(Some(start), Some(end)), 42.0);
        // Inverted interval clamps to 0, never negative.
        assert_eq!(duration_seconds(Some(end), Some(start)), 0.0);
    }

    #[test]
    fn task_failure_reason_defaults_to_agent_error() {
        let mut task = task_fixture();
        assert_eq!(task_failure_reason(&task), "agent_error");
        task.failure_reason = Some("".to_string());
        assert_eq!(
            task_failure_reason(&task),
            "agent_error",
            "empty string degrades too"
        );
        task.failure_reason = Some("provider_auth".to_string());
        assert_eq!(task_failure_reason(&task), "provider_auth");
    }

    #[test]
    fn task_error_type_buckets_match_go() {
        assert_eq!(task_error_type("runtime_offline"), "runtime");
        assert_eq!(task_error_type("runtime_recovery"), "runtime");
        assert_eq!(task_error_type("timeout"), "timeout");
        assert_eq!(task_error_type("codex_semantic_inactivity"), "timeout");
        assert_eq!(task_error_type("iteration_limit"), "agent_output");
        assert_eq!(task_error_type("agent_fallback_message"), "agent_output");
        assert_eq!(task_error_type("cancelled"), "cancelled");
        assert_eq!(task_error_type("user_cancelled"), "cancelled");
        assert_eq!(task_error_type("exotic"), "agent_error");
    }

    #[test]
    fn quick_create_context_parses_only_for_linked_free_tasks() {
        let mut task = task_fixture();
        // Any link disqualifies before parsing.
        task.issue_id = Some(Uuid::nil());
        task.context = Some(serde_json::json!({"type": "quick_create"}));
        assert!(TaskService::parse_quick_create_context(&task).is_none());
        task.issue_id = None;
        // Wrong type marker is rejected.
        task.context = Some(serde_json::json!({"type": "other"}));
        assert!(TaskService::parse_quick_create_context(&task).is_none());
        // Full shape parses with optional fields.
        task.context = Some(serde_json::json!({
            "type": "quick_create",
            "prompt": "fix the flaky test",
            "requester_id": "u1",
            "workspace_id": "w1",
            "priority": "high",
            "attachment_ids": ["a1"]
        }));
        let qc = TaskService::parse_quick_create_context(&task).expect("parses");
        assert_eq!(qc.prompt, "fix the flaky test");
        assert_eq!(qc.priority, "high");
        assert_eq!(qc.attachment_ids, vec!["a1".to_string()]);
        assert_eq!(qc.parent_issue_id, "");
    }

    #[test]
    fn analytics_context_key_joins_identity_columns() {
        let mut task = task_fixture();
        let id = Uuid::now_v7();
        task.id = id;
        task.runtime_id = Some(id);
        let key = task_analytics_context_key(&task).expect("non-empty");
        let parts: Vec<&str> = key.split('|').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0], id.to_string());
        assert_eq!(parts[1], id.to_string());
        assert_eq!(parts[2], "");
    }

    #[tokio::test]
    async fn metrics_context_prefers_link_over_cache_source() {
        // No DB wired: runtime/agent lookups fail and fall through to defaults.
        let pool =
            sqlx::PgPool::connect_lazy("postgres://invalid.invalid/nope").expect("lazy pool");
        let bus = std::sync::Arc::new(cordy_events::Bus::new());
        let svc = TaskService::new(pool, bus);
        let mut task = task_fixture();
        task.chat_session_id = Some(Uuid::now_v7());
        let (source, _, _) = svc.task_metrics_context(&task).await;
        assert_eq!(source, "chat");

        let mut task = task_fixture();
        task.issue_id = Some(Uuid::now_v7());
        let (source, _, _) = svc.task_metrics_context(&task).await;
        assert_eq!(
            source, "issue",
            "no autopilot context without a DB → plain issue"
        );

        let mut task = task_fixture();
        task.autopilot_run_id = Some(Uuid::now_v7());
        let (source, _, _) = svc.task_metrics_context(&task).await;
        assert_eq!(source, "autopilot");

        let task = task_fixture();
        let (source, _, _) = svc.task_metrics_context(&task).await;
        assert_eq!(
            source, "manual",
            "no links and no DB: TaskContext starts at source=manual, which the default branch keeps"
        );
    }

    #[tokio::test]
    async fn quick_actions_pass_admits_once_per_session_and_releases() {
        let pool =
            sqlx::PgPool::connect_lazy("postgres://invalid.invalid/nope").expect("lazy pool");
        let bus = std::sync::Arc::new(cordy_events::Bus::new());
        let svc = TaskService::new(pool, bus);
        let session = Uuid::now_v7();

        assert!(svc.try_admit_quick_actions_pass(session, 4));
        assert!(
            !svc.try_admit_quick_actions_pass(session, 4),
            "second concurrent pass for the same session is shed"
        );
        // A different session admits while the first is in flight.
        let other = Uuid::now_v7();
        assert!(svc.try_admit_quick_actions_pass(other, 4));
        // Ceiling reached.
        let third = Uuid::now_v7();
        assert!(!svc.try_admit_quick_actions_pass(third, 2));

        svc.release_quick_actions_pass(session);
        assert!(svc.try_admit_quick_actions_pass(third, 2));
    }
}
