//! Terminal task transitions and the auto-retry engine — port of
//! `service/task.go` L3695-5131 (CompleteTask / writeChatCompletionOutcome /
//! FailTask / MaybeRetryFailedTask / RerunIssue / HandleFailedTasks) plus the
//! Slice4b helpers those consume (broadcastChatDone, createAgentComment,
//! quick-create notifications live in `task_notify.rs`; delegated recovery in
//! `task_recovery.rs`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use patchbay_db::dbid::new_v7;
use patchbay_db::models::{Agent, AgentTaskQueue, ChatMessage};
use patchbay_db::queries::agent::{
    cancel_pending_tasks_by_issue_and_agent, complete_agent_task, create_retry_task,
    fail_agent_task, get_agent_task, mark_cancelled_task_session_rollout_missing,
};
use patchbay_db::queries::attachment::{
    bind_chat_attachments_to_message, count_unbound_chat_attachments_for_task,
};
use patchbay_db::queries::chat::{
    clear_chat_session_session_if_matches, release_onboarding_kickoff_from_task,
    task_has_channel_ingested_messages, task_input_is_onboarding_kickoff_only,
    update_chat_session_session,
};
use patchbay_db::queries::comment::get_comment;
use patchbay_db::queries::issue::{get_issue, update_issue_status};

use crate::chat_quick_actions::{split_chat_quick_actions, ChatQuickActionsOrigin};
use crate::issue_status;
use crate::redact;
use crate::task_failure as task_failure_crate;
use crate::task_helpers::{
    compute_chat_elapsed_ms, has_runnable_successor, is_trivial_done_output, retry_attempt_ceiling,
    retry_delay_for_attempt, retry_eligible, truncate_fallback_comment_body,
    MAX_SYNTHESIZED_FALLBACK_COMMENT_RUNES,
};
use crate::task_service::{
    chat_input_owner_id, create_assistant_chat_message_typed, downcast_sqlx, opt_str,
    overlay_value_or_null, sanitize_text_for_postgres, RuntimeMcpOverlayData, TaskService,
    TaskServiceError, ERR_RERUN_INVOKE_NOT_ALLOWED,
};

/// The non-empty English body stored on a no_response assistant row. New
/// clients render a localized message keyed on message_kind='no_response';
/// older clients that ignore message_kind still show this text instead of an
/// empty bubble (PB-4351).
pub const CHAT_NO_RESPONSE_FALLBACK: &str = "The agent finished this turn without a text reply.";

/// Failure reasons the auto-retry path is allowed to act on. Agent-side
/// errors are intentionally excluded — those are real problems the user
/// should see. The one agent_error.* exception is provider_network: a
/// mid-stream provider disconnect is transient infrastructure flakiness
/// (PB-4910). skill_bundle_unavailable: the agent process never started, so
/// every downloaded bundle is already cached on disk (PB-5370).
fn retryable_reason(reason: &str) -> bool {
    matches!(
        reason,
        "runtime_offline"
            | "runtime_recovery"
            | "timeout"
            | "codex_semantic_inactivity"
            | "agent_error.provider_network"
            | "skill_bundle_unavailable"
    )
}

impl TaskService {
    /// Records a missing Codex rollout after server-side cancellation and
    /// clears only the chat pointer that still names the cancelled task's
    /// session. The chat lock preserves the repository-wide lock order.
    pub async fn acknowledge_cancelled_session_rollout_missing(
        &self,
        task_id: Uuid,
    ) -> Result<bool, TaskServiceError> {
        let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;
        lock_chat_session_for_task_write(&mut tx, task_id).await?;
        let Some(task) = get_agent_task(&mut *tx, task_id)
            .await
            .map_err(downcast_sqlx)?
        else {
            tx.commit().await.map_err(TaskServiceError::Sql)?;
            return Ok(false);
        };
        if task.status != "cancelled" {
            tx.commit().await.map_err(TaskServiceError::Sql)?;
            return Ok(false);
        }
        if mark_cancelled_task_session_rollout_missing(&mut *tx, task_id)
            .await
            .map_err(downcast_sqlx)?
            == 0
        {
            tx.commit().await.map_err(TaskServiceError::Sql)?;
            return Ok(false);
        }
        if let (Some(chat_session_id), Some(session_id)) =
            (task.chat_session_id, task.session_id.as_deref())
        {
            if !session_id.is_empty() {
                clear_chat_session_session_if_matches(
                    &mut *tx,
                    chat_session_id,
                    Some(session_id),
                    task.runtime_id,
                )
                .await
                .map_err(|error| {
                    TaskServiceError::Internal(format!(
                        "clear missing-rollout chat session resume pointer: {error}"
                    ))
                })?;
            }
        }
        tx.commit().await.map_err(TaskServiceError::Sql)?;
        Ok(true)
    }

    /// Marks a task completed inside one transaction: status CAS, chat
    /// resume-pointer advance, assistant outcome row. Idempotent under
    /// parallel-terminal races (ErrNoRows → return the existing row).
    #[allow(clippy::too_many_arguments)]
    pub async fn complete_task(
        self: &Arc<Self>,
        task_id: Uuid,
        result: &serde_json::Value,
        session_id: &str,
        work_dir: &str,
        branch_name: &str,
        session_rollout_missing: bool,
        retired_session_id: &str,
        durable_work_dir: &str,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        let mut chat_assistant_msg: Option<ChatMessage> = None;
        let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;
        // chat_session → agent_task_queue is the repo-wide lock order.
        lock_chat_session_for_task_write(&mut tx, task_id).await?;

        let t = complete_agent_task(
            &mut *tx,
            task_id,
            result,
            opt_str(session_id),
            opt_str(work_dir),
            session_rollout_missing,
            opt_str(durable_work_dir),
            opt_str(branch_name),
            opt_str(retired_session_id),
        )
        .await
        .map_err(downcast_sqlx)?;

        let Some(t) = t else {
            // UPDATE … WHERE status='running' matched no rows: another actor
            // already finalized this task. Idempotent success.
            tx.commit().await.map_err(TaskServiceError::Sql)?;
            return self.idempotent_finalized(task_id, "complete task").await;
        };

        if let Some(chat_session_id) = t.chat_session_id {
            // Pin the chat_session's runtime_id alongside the session_id so
            // the next claim can apply the runtime-guard. Both fields move
            // together: no new session id → leave runtime_id untouched (NULL
            // → COALESCE keeps the existing value).
            let session_runtime_id = if !session_id.is_empty() {
                t.runtime_id
            } else {
                None
            };
            update_chat_session_session(
                &mut *tx,
                opt_str(session_id),
                opt_str(work_dir),
                session_runtime_id,
                chat_session_id,
            )
            .await
            .map_err(|e| {
                TaskServiceError::Internal(format!("update chat session resume pointer: {e}"))
            })?;
            // A turn that recovered by abandoning its session still retires it
            // here; runs after the update so a real new id wins first (GH #6066).
            if !retired_session_id.is_empty() {
                clear_chat_session_session_if_matches(
                    &mut *tx,
                    chat_session_id,
                    Some(retired_session_id),
                    t.runtime_id,
                )
                .await
                .map_err(|e| {
                    TaskServiceError::Internal(format!(
                        "clear retired chat session resume pointer: {e}"
                    ))
                })?;
            }

            // Assistant outcome row written in the SAME transaction as the
            // status flip (PB-4351). Failing here rolls everything back so
            // the daemon retries the terminal callback; the status CAS above
            // guarantees a replay can't write a second row.
            chat_assistant_msg = self
                .write_chat_completion_outcome_tx(&mut tx, &t, result)
                .await?;
        }
        tx.commit().await.map_err(TaskServiceError::Sql)?;
        let task = t;

        tracing::info!(task_id = %task.id, issue_id = ?task.issue_id, "task completed");
        self.capture_task_completed(&task).await;

        // Invariant: every completed issue task must have at least one agent
        // comment, so synthesize a fallback from the final output when the
        // agent never posted one during execution.
        if let Some(issue_id) = task.issue_id {
            let suppress_no_action = has_squad_leader_no_action_for_task(&self.pool, &task).await;
            let agent_commented = patchbay_db::queries::comment::has_agent_commented_since(
                &self.pool,
                issue_id,
                task.agent_id,
                task.started_at,
            )
            .await
            .unwrap_or(None)
            .unwrap_or(false);
            if !suppress_no_action && !agent_commented {
                if let Ok(payload) = serde_json::from_value::<
                    patchbay_protocol::messages::TaskCompletedPayload,
                >(result.clone())
                {
                    if !payload.output.is_empty() {
                        // Match the CLI's --content behavior: literal `\n`
                        // sequences decode into real newlines first.
                        let body = patchbay_util::unescape_backslash_escapes(&payload.output);
                        if task.trigger_comment_id.is_some() && is_trivial_done_output(&body) {
                            tracing::warn!(
                                task_id = %task.id,
                                "suppressing trivial comment-trigger fallback output"
                            );
                        } else {
                            // Redact first, then bound (GH #5455): a runaway
                            // raw-stream Output must never reach the thread.
                            let content = truncate_fallback_comment_body(
                                &redact::text(&body),
                                MAX_SYNTHESIZED_FALLBACK_COMMENT_RUNES,
                            );
                            self.create_agent_comment(
                                issue_id,
                                task.agent_id,
                                &content,
                                "comment",
                                task.trigger_comment_id,
                                Some(task.id),
                            )
                            .await;
                        }
                    }
                }
            }
        }

        // Quick-create tasks: push an inbox confirmation to the requester.
        if let Some(qc) = TaskService::parse_quick_create_context(&task) {
            self.notify_quick_create_completed(&task, &qc, result).await;
        }

        // Chat tasks: broadcast chat:done AFTER commit; the pending flag and
        // the resolving pass can never disagree about generation running.
        if task.chat_session_id.is_some() {
            let suggest = crate::task_quick_actions::chat_quick_actions_eligible(
                self,
                &task,
                chat_assistant_msg.as_ref(),
            )
            .await;
            self.broadcast_chat_done(&task, chat_assistant_msg.as_ref(), suggest)
                .await;
            if suggest {
                self.generate_chat_quick_actions_async(
                    task.clone(),
                    ChatQuickActionsOrigin::Automatic,
                );
            }
        }

        self.reconcile_agent_status(task.agent_id).await;
        self.broadcast_task_event(
            patchbay_protocol::EVENT_TASK_COMPLETED,
            &task,
            Default::default(),
        )
        .await;
        Ok(task)
    }

    /// Marks a task failed with auto-retry pre-computed OUTSIDE the
    /// transaction so the retry child commits atomically with the fail
    /// (PB-4351), closing the window where a newer chat task could claim the
    /// idle session ahead of its own retry.
    #[allow(clippy::too_many_arguments)]
    pub async fn fail_task(
        &self,
        task_id: Uuid,
        err_msg: &str,
        session_id: &str,
        work_dir: &str,
        branch_name: &str,
        failure_reason_in: &str,
        session_rollout_missing: bool,
        retired_session_id: &str,
        durable_work_dir: &str,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        // Strip bytes PostgreSQL cannot store before anything reads errMsg,
        // so classifier/transaction/consumers all see the persisted text
        // (GH #7098).
        let err_msg = sanitize_text_for_postgres(err_msg);

        // PB-2946: synthesise a refined reason when none supplied; PB-5370:
        // normalise legacy daemon catchalls either way, before retry
        // pre-compute so the upgraded reason decides eligibility.
        let mut failure_reason = failure_reason_in.to_string();
        if failure_reason.is_empty() {
            failure_reason = task_failure_crate::classify(&err_msg).as_str().to_string();
        }
        failure_reason = task_failure_crate::normalize_daemon_reason(&failure_reason, &err_msg)
            .as_str()
            .to_string();

        // Pre-compute the auto-retry outside the transaction; only retryable
        // failures pay the Composio overlay cost.
        let mut want_retry = false;
        let mut retry_overlay = RuntimeMcpOverlayData::default();
        let mut retry_fire_at: Option<DateTime<Utc>> = None;
        let mut retry_max_attempts: Option<i32> = None;
        if retryable_reason(&failure_reason) {
            if let Ok(Some(parent)) = get_agent_task(&self.pool, task_id).await {
                if retry_eligible(&failure_reason, &parent) {
                    want_retry = true;
                    // Persist the reason-aware effective budget into the child
                    // so the chain self-describes.
                    retry_max_attempts =
                        Some(retry_attempt_ceiling(&failure_reason, parent.max_attempts));
                    // Defer when the reason's schedule calls for backoff; zero
                    // delay leaves fire_at NULL (immediately claimable).
                    let delay = retry_delay_for_attempt(&failure_reason, parent.attempt);
                    if delay > Duration::ZERO {
                        retry_fire_at = Some(chrono::Utc::now() + delay);
                    }
                    match patchbay_db::queries::agent::get_agent(&self.pool, parent.agent_id).await
                    {
                        Err(aerr) => {
                            // Missing overlay is not retry-fatal.
                            tracing::warn!(
                                task_id = %task_id,
                                error = %aerr,
                                "fail task auto-retry: load agent for overlay failed"
                            );
                        }
                        Ok(Some(agent)) => {
                            retry_overlay = self
                                .build_runtime_mcp_overlay(
                                    parent.originator_user_id.unwrap_or_else(Uuid::nil),
                                    &agent,
                                )
                                .await;
                        }
                        Ok(None) => {}
                    }
                }
            } else {
                tracing::warn!(task_id = %task_id, "fail task auto-retry: load parent failed");
            }
        }

        let mut retried: Option<AgentTaskQueue> = None;
        let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;
        lock_chat_session_for_task_write(&mut tx, task_id).await?;

        let t = fail_agent_task(
            &mut *tx,
            task_id,
            Some(&err_msg),
            opt_str(&failure_reason),
            session_rollout_missing,
            opt_str(session_id),
            opt_str(work_dir),
            opt_str(durable_work_dir),
            opt_str(branch_name),
            opt_str(retired_session_id),
        )
        .await
        .map_err(downcast_sqlx)?;
        let Some(t) = t else {
            tx.commit().await.map_err(TaskServiceError::Sql)?;
            return self.idempotent_finalized(task_id, "fail task").await;
        };

        if let Some(chat_session_id) = t.chat_session_id {
            // Keep resume-unsafe sessions observable on the task row but out
            // of the chat-level resume pointer; clear matched exactly so a
            // concurrent turn's newer pointer survives (GH #6066).
            if resume_unsafe_here(&failure_reason, &err_msg) {
                let dead_session = if session_id.is_empty() {
                    t.session_id.as_deref().unwrap_or("")
                } else {
                    session_id
                };
                if !dead_session.is_empty() {
                    clear_chat_session_session_if_matches(
                        &mut *tx,
                        chat_session_id,
                        Some(dead_session),
                        t.runtime_id,
                    )
                    .await
                    .map_err(|e| {
                        TaskServiceError::Internal(format!(
                            "clear poisoned chat session resume pointer: {e}"
                        ))
                    })?;
                }
            }
            // A run-explicitly-retired session goes whatever the terminal
            // status: the replacing retry may well have succeeded.
            if !retired_session_id.is_empty() {
                clear_chat_session_session_if_matches(
                    &mut *tx,
                    chat_session_id,
                    Some(retired_session_id),
                    t.runtime_id,
                )
                .await
                .map_err(|e| {
                    TaskServiceError::Internal(format!(
                        "clear retired chat session resume pointer: {e}"
                    ))
                })?;
            }
            // ResumeUnsafeFailure (not the reason alone) gates re-pinning so an
            // un-upgraded daemon's unknown rows cannot re-pin the cleared session.
            if !resume_unsafe_here(&failure_reason, &err_msg) {
                let session_runtime_id = if !session_id.is_empty() {
                    t.runtime_id
                } else {
                    None
                };
                update_chat_session_session(
                    &mut *tx,
                    opt_str(session_id),
                    opt_str(work_dir),
                    session_runtime_id,
                    chat_session_id,
                )
                .await
                .map_err(|e| {
                    TaskServiceError::Internal(format!("update chat session resume pointer: {e}"))
                })?;
            }
        }

        // Retry child created atomically with the fail. The successor check is
        // an optimisation only — correctness belongs to CreateRetryTask's
        // ON CONFLICT DO NOTHING, which yields instead of raising.
        if want_retry {
            let successor = has_runnable_successor(&mut *tx, &t).await.map_err(|e| {
                TaskServiceError::Internal(format!("check runnable successor: {e}"))
            })?;
            if successor {
                tracing::info!(
                    task_id = %task_id,
                    "fail task auto-retry skipped: a successor is already pending"
                );
            } else {
                match create_retry_task(
                    &mut *tx,
                    task_id,
                    retry_fire_at,
                    retry_max_attempts,
                    &overlay_value_or_null(&retry_overlay.overlay),
                    &overlay_value_or_null(&retry_overlay.connected_apps),
                    new_v7(),
                )
                .await
                {
                    Ok(Some(child)) => retried = Some(child),
                    Ok(None) => {
                        // Workspace torn down mid-flight, or a rerun took the
                        // slot after the unlocked check. This transaction still
                        // owns the parent's failed status — record and move on.
                        tracing::info!(task_id = %task_id, "fail task auto-retry not created: no row written");
                    }
                    Err(cerr) => {
                        return Err(TaskServiceError::Internal(format!(
                            "create retry task: {cerr}"
                        )));
                    }
                }
            }
        }

        // Terminal non-retried chat failure is a visible assistant outcome,
        // persisted while the session lock is held, then the next direct head
        // reanchors past it.
        if let (Some(chat_session_id), false) = (t.chat_session_id, retried.is_some()) {
            // An adopted onboarding kickoff must not stay bound to a task that
            // will never run again (PB-5827); gated on retried==nil because a
            // retry child still reads the root's input binding.
            release_onboarding_kickoff_from_task(&mut *tx, chat_input_owner_id(&t))
                .await
                .map_err(|e| {
                    TaskServiceError::Internal(format!("release onboarding kickoff: {e}"))
                })?;
            create_assistant_chat_message_typed(
                &mut tx,
                chat_session_id,
                &redact::text(&err_msg),
                t.id,
                compute_chat_elapsed_ms(t.completed_at, t.created_at),
                None,
                opt_str(&failure_reason),
            )
            .await
            .map_err(|e| TaskServiceError::Internal(format!("write chat failure outcome: {e}")))?;
        }
        tx.commit().await.map_err(TaskServiceError::Sql)?;
        let task = t;

        tracing::warn!(
            task_id = %task.id,
            issue_id = ?task.issue_id,
            failure_reason = %failure_reason,
            "task failed"
        );
        self.capture_task_failed(&task).await;

        if let Some(retried) = &retried {
            tracing::info!(
                parent_task_id = %task.id,
                child_task_id = %retried.id,
                reason = %failure_reason,
                attempt = retried.attempt,
                max_attempts = retried.max_attempts,
                status = %retried.status,
                "task auto-retry enqueued"
            );
            if retried.status == "queued" {
                self.broadcast_task_event(
                    patchbay_protocol::EVENT_TASK_QUEUED,
                    retried,
                    Default::default(),
                )
                .await;
                self.notify_task_enqueued(retried).await;
            }
        }

        // Delegated tasks hand control back to their coordinator only after
        // the retry decision; recoverable failures stay silent while a child
        // attempt is pending.
        if retried.is_none() {
            if let Err(recovery_err) = self.recover_delegated_task_failure(&task).await {
                tracing::warn!(
                    task_id = %task.id,
                    error = %recovery_err,
                    "delegated task failure recovery failed"
                );
            }
        }

        // Skip the per-failure system comment when an auto-retry is pending.
        if let (Some(issue_id), false) = (task.issue_id, retried.is_some()) {
            if !err_msg.is_empty() {
                self.create_agent_comment(
                    issue_id,
                    task.agent_id,
                    &redact::text(&err_msg),
                    "system",
                    task.trigger_comment_id,
                    Some(task.id),
                )
                .await;
            }
        }

        if retried.is_none() {
            if let Some(qc) = TaskService::parse_quick_create_context(&task) {
                self.notify_quick_create_failed(&task, &qc, &err_msg).await;
            }
        }

        self.reconcile_agent_status(task.agent_id).await;
        self.broadcast_task_failed_event(&task, &err_msg, &failure_reason, retried.is_some())
            .await;
        Ok(task)
    }

    /// Auto-retry entry for sweepers/recover-orphans. Mirrors FailTask's
    /// in-tx semantics minus the surrounding fail transaction.
    pub async fn maybe_retry_failed_task(
        &self,
        parent: &AgentTaskQueue,
    ) -> Result<Option<AgentTaskQueue>, TaskServiceError> {
        if parent.status != "failed" {
            return Ok(None);
        }
        let reason = parent.failure_reason.clone().unwrap_or_default();
        if !retryable_reason(&reason) {
            return Ok(None);
        }
        // Reason-aware ceiling so an orphaned provider_network task recovered
        // on its 2nd attempt still gets its deferred 3rd.
        if parent.attempt >= retry_attempt_ceiling(&reason, parent.max_attempts) {
            tracing::info!(
                task_id = %parent.id,
                "task auto-retry skipped: budget exhausted"
            );
            return Ok(None);
        }
        if !retry_eligible(&reason, parent) {
            return Ok(None);
        }

        let mut overlay = RuntimeMcpOverlayData::default();
        match patchbay_db::queries::agent::get_agent(&self.pool, parent.agent_id).await {
            Err(agent_err) => {
                tracing::warn!(
                    parent_task_id = %parent.id,
                    error = %agent_err,
                    "task auto-retry: load agent for overlay failed"
                );
            }
            Ok(Some(agent)) => {
                overlay = self
                    .build_runtime_mcp_overlay(
                        parent.originator_user_id.unwrap_or_else(Uuid::nil),
                        &agent,
                    )
                    .await;
            }
            Ok(None) => {}
        }
        let mut retry_fire_at: Option<DateTime<Utc>> = None;
        let delay = retry_delay_for_attempt(&reason, parent.attempt);
        if delay > Duration::ZERO {
            retry_fire_at = Some(chrono::Utc::now() + delay);
        }
        // Advisory slot check; losing the race is handled by CreateRetryTask
        // yielding, which this caller reads as "no retry".
        match has_runnable_successor(&self.pool, parent).await {
            Ok(true) => {
                tracing::info!(
                    parent_task_id = %parent.id,
                    "task auto-retry skipped: a successor is already pending"
                );
                return Ok(None);
            }
            Ok(false) => {}
            Err(herr) => {
                tracing::warn!(
                    parent_task_id = %parent.id,
                    error = %herr,
                    "task auto-retry: successor check failed; attempting retry anyway"
                );
            }
        }
        match create_retry_task(
            &self.pool,
            parent.id,
            retry_fire_at,
            Some(retry_attempt_ceiling(&reason, parent.max_attempts)),
            &overlay_value_or_null(&overlay.overlay),
            &overlay_value_or_null(&overlay.connected_apps),
            new_v7(),
        )
        .await
        {
            Ok(Some(child)) => {
                tracing::info!(
                    parent_task_id = %parent.id,
                    child_task_id = %child.id,
                    reason = %reason,
                    "task auto-retry enqueued"
                );
                if child.status == "queued" {
                    self.broadcast_task_event(
                        patchbay_protocol::EVENT_TASK_QUEUED,
                        &child,
                        Default::default(),
                    )
                    .await;
                    self.notify_task_enqueued(&child).await;
                }
                Ok(Some(child))
            }
            // Workspace gone or slot taken between check and insert: same
            // contract as FailTask's path — no retry, no error.
            Ok(None) => {
                tracing::info!(parent_task_id = %parent.id, "task auto-retry not created: no row written");
                Ok(None)
            }
            Err(err) => {
                tracing::warn!(parent_task_id = %parent.id, error = %err, "task auto-retry failed");
                Err(TaskServiceError::Internal(format!(
                    "task auto-retry: {err}"
                )))
            }
        }
    }

    /// Manual rerun endpoint core. Target resolution: source task's agent
    /// (with leader/squad provenance + trigger inheritance) or the issue's
    /// current assignee. A block fails closed before anything mutates
    /// (PB-4525); the pending-slot clear/enqueue pair retries once against a
    /// concurrent system retry.
    pub async fn rerun_issue(
        &self,
        issue_id: Uuid,
        source_task_id: Option<Uuid>,
        trigger_comment_id_in: Option<Uuid>,
        actor_user_id: Option<Uuid>,
        can_invoke: Option<&(dyn Fn(&Agent) -> bool + Sync)>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        let issue = get_issue(&self.pool, issue_id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("load issue: {e}")))?
            .ok_or_else(|| TaskServiceError::Internal("load issue: not found".into()))?;

        let mut trigger_comment_id = trigger_comment_id_in;
        let agent_id: Uuid;
        let mut is_leader = false;
        let mut squad_id: Option<Uuid> = None;
        let mut coalesced_comment_ids: Vec<Uuid> = Vec::new();
        if let Some(source_task_id) = source_task_id {
            let source_task = get_agent_task(&self.pool, source_task_id)
                .await
                .map_err(|e| TaskServiceError::Internal(format!("load source task: {e}")))?
                .ok_or_else(|| TaskServiceError::Internal("load source task: not found".into()))?;
            if source_task.issue_id != Some(issue_id) {
                return Err(TaskServiceError::Internal(
                    "source task does not belong to this issue".into(),
                ));
            }
            agent_id = source_task.agent_id;
            is_leader = source_task.is_leader_task;
            squad_id = source_task.squad_id;
            // Carry trigger provenance so a per-row rerun stays comment-
            // triggered; only override when the caller passed none.
            if trigger_comment_id.is_none() {
                coalesced_comment_ids = source_task.coalesced_comment_ids.clone();
                if let Some(tc) = source_task.trigger_comment_id {
                    trigger_comment_id = Some(tc);
                } else if !coalesced_comment_ids.is_empty() {
                    let (promoted, remaining) = self
                        .promote_newest_surviving_comment(coalesced_comment_ids.clone())
                        .await
                        .map_err(|e| {
                            TaskServiceError::Internal(format!("repair source comment plan: {e}"))
                        })?;
                    trigger_comment_id = promoted;
                    coalesced_comment_ids = remaining;
                }
            }
        } else {
            match (issue.assignee_type.as_deref(), issue.assignee_id) {
                (Some("agent"), Some(assignee)) => agent_id = assignee,
                (Some("squad"), Some(squad_assignee)) => {
                    let squad = patchbay_db::queries::squad::get_squad(&self.pool, squad_assignee)
                        .await
                        .map_err(|_| {
                            TaskServiceError::Internal(
                                "issue is assigned to a squad but squad not found".into(),
                            )
                        })?
                        .ok_or_else(|| {
                            TaskServiceError::Internal(
                                "issue is assigned to a squad but squad not found".into(),
                            )
                        })?;
                    agent_id = squad.leader_id;
                    is_leader = true;
                    squad_id = Some(squad_assignee);
                }
                _ => {
                    return Err(TaskServiceError::Internal(
                        "issue is not assigned to an agent or squad".into(),
                    ));
                }
            }
        };

        // Re-validate invoke permission on the RESOLVED target before any
        // mutation (PB-4525). A blocked rerun mutates nothing.
        if let Some(can_invoke) = can_invoke {
            let target_agent = patchbay_db::queries::agent::get_agent(&self.pool, agent_id)
                .await
                .map_err(|e| TaskServiceError::Internal(format!("load target agent: {e}")))?
                .ok_or_else(|| TaskServiceError::Internal("load target agent: not found".into()))?;
            if !can_invoke(&target_agent) {
                return Err(TaskServiceError::RerunInvokeNotAllowed(
                    ERR_RERUN_INVOKE_NOT_ALLOWED,
                ));
            }
        }

        let mut cancelled_count = 0usize;
        let mut enqueue_result = self
            .clear_and_enqueue_rerun(
                &issue,
                agent_id,
                trigger_comment_id,
                coalesced_comment_ids.clone(),
                is_leader,
                squad_id,
                actor_user_id,
                source_task_id,
                &mut cancelled_count,
            )
            .await;

        if matches!(&enqueue_result, Err(e) if crate::task_service::pending_slot_taken_err(e)) {
            // The clear and this enqueue are separate commits; a concurrent
            // FailTask retry can take the slot between them. Clear once more —
            // bounded to a single extra attempt (#5914 lineage).
            tracing::info!(
                issue_id = %issue_id,
                "issue rerun: pending slot taken concurrently, reclaiming"
            );
            enqueue_result = self
                .clear_and_enqueue_rerun(
                    &issue,
                    agent_id,
                    trigger_comment_id,
                    coalesced_comment_ids,
                    is_leader,
                    squad_id,
                    actor_user_id,
                    source_task_id,
                    &mut cancelled_count,
                )
                .await;
        }
        let task = enqueue_result?;
        tracing::info!(
            task_id = %task.id,
            issue_id = %issue_id,
            cancelled_pending = cancelled_count,
            "issue rerun enqueued"
        );
        Ok(task)
    }

    #[allow(clippy::too_many_arguments)]
    async fn clear_and_enqueue_rerun(
        &self,
        issue: &patchbay_db::models::Issue,
        agent_id: Uuid,
        trigger_comment_id: Option<Uuid>,
        coalesced_comment_ids: Vec<Uuid>,
        is_leader: bool,
        squad_id: Option<Uuid>,
        actor_user_id: Option<Uuid>,
        source_task_id: Option<Uuid>,
        cancelled_count: &mut usize,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        // Clear only not-yet-executing rows; running/waiting_local_directory
        // are deliberately left alone — interrupting an in-flight run is what
        // CancelTask is for.
        match cancel_pending_tasks_by_issue_and_agent(&self.pool, issue.id, agent_id).await {
            Ok(cancelled) => {
                *cancelled_count += cancelled.len();
                for t in &cancelled {
                    self.capture_task_cancelled(t).await;
                    self.reconcile_agent_status(t.agent_id).await;
                    self.broadcast_task_event(
                        patchbay_protocol::EVENT_TASK_CANCELLED,
                        t,
                        Default::default(),
                    )
                    .await;
                }
            }
            Err(cerr) => {
                tracing::warn!(error = %cerr, "rerun: cancel pending tasks failed");
            }
        }
        // A manual rerun is a NEW direct_human trigger attributed to the
        // rerunning member (PB-4302 §5); rerun_of rides the insert so the
        // queued event never sees NULL lineage.
        self.enqueue_rerun_task(
            issue,
            agent_id,
            trigger_comment_id,
            coalesced_comment_ids,
            is_leader,
            squad_id,
            actor_user_id,
            source_task_id,
        )
        .await
    }

    /// Repairs a manual rerun whose original trigger was deleted: promote the
    /// newest surviving comment to trigger so the enqueue recomputes
    /// originator and connected-app capabilities from a real comment.
    pub(crate) async fn promote_newest_surviving_comment(
        &self,
        ids: Vec<Uuid>,
    ) -> Result<(Option<Uuid>, Vec<Uuid>), TaskServiceError> {
        struct SurvivingComment {
            id: Uuid,
            created_at: chrono::DateTime<chrono::Utc>,
        }
        let mut survivors: Vec<SurvivingComment> = Vec::with_capacity(ids.len());
        let mut seen = std::collections::HashSet::new();
        for id in ids {
            if seen.insert(id) {
                match get_comment(&self.pool, id).await {
                    Ok(Some(comment)) => survivors.push(SurvivingComment {
                        id: comment.id,
                        created_at: comment.created_at,
                    }),
                    // Deleted comments just drop out of the plan.
                    Ok(None) => {}
                    Err(e) => return Err(TaskServiceError::Internal(e.to_string())),
                }
            }
        }
        if survivors.is_empty() {
            return Ok((None, Vec::new()));
        }
        let mut newest = 0;
        for i in 1..survivors.len() {
            if survivors[i].created_at > survivors[newest].created_at
                || (survivors[i].created_at == survivors[newest].created_at
                    && survivors[i].id.to_string() > survivors[newest].id.to_string())
            {
                newest = i;
            }
        }
        let remaining: Vec<Uuid> = survivors
            .iter()
            .enumerate()
            .filter(|(idx, _)| *idx != newest)
            .map(|(_, c)| c.id)
            .collect();
        Ok((Some(survivors[newest].id), remaining))
    }

    /// Enqueues a fresh task for the given agent on the issue: assignee-driven
    /// path when the target IS the single-agent assignee (keeps assignee
    /// bookkeeping in sync), mention path otherwise. force_fresh_session is
    /// pinned true on every rerun row — rollback-safe legacy signal for old
    /// claim handlers (PB-4869).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn enqueue_rerun_task(
        &self,
        issue: &patchbay_db::models::Issue,
        agent_id: Uuid,
        trigger_comment_id: Option<Uuid>,
        coalesced_comment_ids: Vec<Uuid>,
        is_leader: bool,
        squad_id: Option<Uuid>,
        actor_user_id: Option<Uuid>,
        rerun_of_task_id: Option<Uuid>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        if issue.assignee_type.as_deref() == Some("agent")
            && issue.assignee_id.is_some()
            && issue.assignee_id == Some(agent_id)
        {
            return self
                .enqueue_issue_task_with_comment_plan(
                    issue,
                    trigger_comment_id,
                    coalesced_comment_ids,
                    true,
                    "",
                    actor_user_id,
                    rerun_of_task_id,
                    None,
                )
                .await;
        }
        self.enqueue_mention_task(
            issue,
            agent_id,
            trigger_comment_id,
            coalesced_comment_ids,
            is_leader,
            squad_id,
            true,
            "",
            actor_user_id,
            rerun_of_task_id,
        )
        .await
    }

    /// Post-failure side effects for a batch of freshly-failed tasks: auto-
    /// retry first (so issues don't flap todo→in_progress within a tick),
    /// delegated recovery, metrics, task:failed broadcast, stuck-issue reset,
    /// agent reconciliation, daemon wakeups. Every surface-the-failure caller
    /// funnels through here.
    pub async fn handle_failed_tasks(&self, tasks: &[AgentTaskQueue]) -> usize {
        if tasks.is_empty() {
            return 0;
        }

        let mut affected_agents: HashMap<Uuid, ()> = HashMap::new();
        let mut processed_issues: std::collections::HashSet<Uuid> = Default::default();
        let mut retried_issues: std::collections::HashSet<Uuid> = Default::default();
        let mut retried_count = 0usize;

        for t in tasks {
            let mut retry_pending = false;
            if let Ok(Some(child)) = self.maybe_retry_failed_task(t).await {
                if child.id != Uuid::nil() {
                    retry_pending = true;
                    retried_count += 1;
                    if let Some(issue_id) = t.issue_id {
                        retried_issues.insert(issue_id);
                    }
                }
            }
            if !retry_pending {
                if let Err(err) = self.recover_delegated_task_failure(t).await {
                    tracing::warn!(
                        task_id = %t.id,
                        error = %err,
                        "handle failed tasks: delegated failure recovery failed"
                    );
                }
            }

            let failure_reason = t
                .failure_reason
                .clone()
                .filter(|r| !r.is_empty())
                .unwrap_or_else(|| "agent_error".to_string());
            self.capture_task_failed(t).await;

            if let Some(issue_id) = t.issue_id {
                if let Ok(Some(issue)) = get_issue(&self.pool, issue_id).await {
                    // Reset stuck in_progress issues only when nothing else is
                    // active and no retry was just enqueued. in_review/blocked
                    // excluded — a human owns those then (PB-6243).
                    let effective =
                        issue_status::effective(&self.pool, issue.workspace_id, &issue.status)
                            .await;
                    if effective == "in_progress"
                        && !processed_issues.contains(&issue_id)
                        && !retried_issues.contains(&issue_id)
                    {
                        processed_issues.insert(issue_id);
                        match patchbay_db::queries::agent::has_active_task_for_issue(
                            &self.pool, issue_id,
                        )
                        .await
                        {
                            Ok(Some(false)) => {
                                match update_issue_status(
                                    &self.pool,
                                    issue_id,
                                    "todo",
                                    issue.workspace_id,
                                )
                                .await
                                {
                                    Ok(Some(updated_issue)) => {
                                        // Direct reset bypasses the HTTP handler that
                                        // normally emits issue:updated (#4648 / PB-3782).
                                        self.broadcast_issue_updated(&updated_issue, &issue.status)
                                            .await;
                                    }
                                    Ok(None) => {}
                                    Err(update_err) => {
                                        tracing::warn!(
                                            issue_id = %issue_id,
                                            error = %update_err,
                                            "handle failed tasks: reset stuck issue failed"
                                        );
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(check_err) => {
                                tracing::warn!(
                                    issue_id = %issue_id,
                                    error = %check_err,
                                    "handle failed tasks: active check failed"
                                );
                            }
                        }
                    }
                }
            }

            // publishTaskFailedEvent equivalent: the shared broadcast resolves
            // the workspace through the same waterfall.
            let err_text = t.error.clone().unwrap_or_default();
            self.broadcast_task_failed_event(t, &err_text, &failure_reason, retry_pending)
                .await;

            affected_agents.insert(t.agent_id, ());
        }

        for agent_id in affected_agents.keys() {
            self.reconcile_agent_status(*agent_id).await;
        }
        self.notify_tasks_finished(tasks).await;
        retried_count
    }
}

impl TaskService {
    /// Writes the assistant chat_message outcome for a completed chat task
    /// inside the caller's completion transaction, returning the row written
    /// (None when none is written).
    ///
    /// Direct tasks: non-empty output → ordinary assistant message; empty →
    /// visible no_response carrying the English fallback body. Never
    /// auto-retries: an empty output is a legitimate tool-only terminal.
    ///
    /// Channel/legacy tasks: empty output writes NO row so chat:done carries
    /// empty content and the channel outbound silently drops it — the
    /// no_response fallback body must never be pushed to Slack/Lark
    /// (PB-4351). Attachments force a row regardless.
    async fn write_chat_completion_outcome_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        task: &AgentTaskQueue,
        result: &serde_json::Value,
    ) -> Result<Option<ChatMessage>, TaskServiceError> {
        use patchbay_protocol::messages::TaskCompletedPayload;

        // result is the daemon request re-marshalled by the handler, always
        // valid JSON; an empty Output is the only case this branch cares about.
        let payload = serde_json::from_value::<TaskCompletedPayload>(result.clone()).unwrap_or(
            TaskCompletedPayload {
                task_id: String::new(),
                pr_url: String::new(),
                output: String::new(),
            },
        );
        let body_full = patchbay_util::unescape_backslash_escapes(&payload.output);
        // Strip the reserved in-band footer from EVERY completion; parsed
        // suggestions are DISCARDED — the server-side pass supersedes them.
        let (body, _) = split_chat_quick_actions(&body_full);
        let is_empty = body.trim().is_empty();

        // Completion-boundary observation (PB-4899). Strictly non-blocking.
        observe_chat_output_local_path(self, task, &body);

        // Unclaimed attachments make an empty-text turn a real response —
        // NOT a no_response — and need a row to hang on. Count + bind run on
        // qtx so message creation and binding are one atomic outcome.
        let ws_uuid = resolve_ws_uuid(self, task).await;
        let mut pending_attachments: i64 = 0;
        if let Some(ws) = ws_uuid {
            pending_attachments = count_unbound_chat_attachments_for_task(&mut **tx, ws, task.id)
                .await
                .map_err(|e| TaskServiceError::Internal(format!("count chat attachments: {e}")))?
                .unwrap_or(0);
        }

        // Channel/legacy empty completion with nothing to show: emit no
        // assistant row at all.
        if is_empty && pending_attachments == 0 {
            match task.chat_input_task_id {
                None => return Ok(None), // legacy task
                Some(input_owner) => {
                    let channel_ingested =
                        task_has_channel_ingested_messages(&mut **tx, input_owner)
                            .await
                            .map_err(|e| {
                                TaskServiceError::Internal(format!(
                                    "check chat completion channel provenance: {e}"
                                ))
                            })?
                            .unwrap_or(false);
                    if channel_ingested {
                        return Ok(None); // channel task
                    }
                }
            }
        }

        let chat_session_id = task.chat_session_id.expect("guarded by caller");
        let elapsed_ms = compute_chat_elapsed_ms(task.completed_at, task.created_at);
        let (content, message_kind): (String, Option<&str>) = if !is_empty {
            let content = redact::text(&body);
            // Deploy-window case: a kickoff task enqueued by the previous
            // server can be claimed here; its reply IS that member's opening
            // and must keep the starter cards (PB-5827). Keyed on the input
            // batch owner so an auto-retry clone reaches the same verdict.
            let opening_only =
                task_input_is_onboarding_kickoff_only(&mut **tx, chat_input_owner_id(task))
                    .await
                    .map_err(|e| {
                        TaskServiceError::Internal(format!("check onboarding kickoff input: {e}"))
                    })?
                    .flatten()
                    .unwrap_or(false);
            (
                content,
                Some(if opening_only {
                    patchbay_protocol::CHAT_MESSAGE_KIND_ONBOARDING_OPENING
                } else {
                    patchbay_protocol::CHAT_MESSAGE_KIND_MESSAGE
                }),
            )
        } else if pending_attachments > 0 {
            // Image/file-only reply: a real 'message' outcome with empty text.
            (
                String::new(),
                Some(patchbay_protocol::CHAT_MESSAGE_KIND_MESSAGE),
            )
        } else {
            // Direct task, empty output, no attachments: explicit no_response.
            (
                CHAT_NO_RESPONSE_FALLBACK.to_string(),
                Some(patchbay_protocol::CHAT_MESSAGE_KIND_NO_RESPONSE),
            )
        };
        let row = create_assistant_chat_message_typed(
            tx,
            chat_session_id,
            &content,
            task.id,
            elapsed_ms,
            message_kind,
            None,
        )
        .await?;

        if pending_attachments > 0 {
            if let Some(ws) = ws_uuid {
                let bound = bind_chat_attachments_to_message(&mut **tx, row.id, ws, task.id)
                    .await
                    .map_err(|e| {
                        TaskServiceError::Internal(format!("bind chat attachments: {e}"))
                    })?;
                if !bound.is_empty() {
                    tracing::info!(
                        task_id = %task.id,
                        count = bound.len(),
                        "bound chat attachments to assistant reply"
                    );
                }
            }
        }
        Ok(Some(row))
    }

    /// ErrNoRows on the terminal UPDATE means another actor finalized first.
    /// Surface the existing row as an idempotent success, or a lookup failure.
    async fn idempotent_finalized(
        &self,
        task_id: Uuid,
        op: &str,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        match get_agent_task(&self.pool, task_id).await {
            Ok(Some(existing)) => {
                tracing::info!(op = op, task_id = %task_id, current_status = %existing.status, "already finalized");
                Ok(existing)
            }
            Ok(None) => {
                tracing::warn!(op = op, task_id = %task_id, "finalized: task not found");
                Err(TaskServiceError::Internal(format!("{op}: task not found")))
            }
            Err(lookup_err) => {
                tracing::warn!(op = op, task_id = %task_id, error = %lookup_err, "terminal update failed");
                Err(TaskServiceError::Internal(format!("{op}: {lookup_err}")))
            }
        }
    }
}

/// Records a metric when a chat reply references a runtime-local path
/// (PB-4899). Lexical only: file:// URLs and the recorded work_dir prefix.
/// No path, body text, or fragment may reach the metric or the log.
fn observe_chat_output_local_path(svc: &TaskService, task: &AgentTaskQueue, body: &str) {
    let Some(metrics) = &svc.metrics else { return };
    if body.trim().is_empty() {
        return;
    }
    let kind = if body.to_lowercase().contains("file://") {
        "file_url"
    } else {
        match &task.work_dir {
            Some(wd) if !wd.is_empty() && body.contains(wd.as_str()) => "workdir_path",
            _ => return,
        }
    };
    metrics.record_chat_output_local_path(kind);
    tracing::warn!(task_id = %task.id, kind = kind, "chat reply references a runtime-local path");
}

/// Go lockChatSessionForTaskWrite: session → task lock order. ErrNoRows is a
/// non-chat task or an already-deleted session — tolerated.
async fn lock_chat_session_for_task_write(
    exec: &mut sqlx::PgConnection,
    task_id: Uuid,
) -> Result<(), TaskServiceError> {
    let _ = patchbay_db::queries::chat::lock_chat_session_for_task(&mut *exec, task_id)
        .await
        .map_err(|e| {
            TaskServiceError::Internal(format!("lock chat session for task write: {e}"))
        })?;
    Ok(())
}

async fn has_squad_leader_no_action_for_task(pool: &sqlx::PgPool, task: &AgentTaskQueue) -> bool {
    let Some(issue_id) = task.issue_id else {
        return false;
    };
    match patchbay_db::queries::activity::has_squad_leader_no_action_evaluation_for_task(
        pool,
        issue_id,
        task.agent_id,
        &task.id.to_string(),
    )
    .await
    {
        Ok(v) => v.unwrap_or(false),
        Err(err) => {
            tracing::warn!(
                task_id = %task.id,
                error = %err,
                "checking squad leader no_action evaluation failed"
            );
            false
        }
    }
}

async fn resolve_ws_uuid(svc: &TaskService, task: &AgentTaskQueue) -> Option<Uuid> {
    svc.resolve_task_workspace_id(task)
        .await
        .and_then(|ws| Uuid::parse_str(&ws).ok())
}

/// ResumeUnsafeFailure(failureReason, errMsg): the two-signal check so an
/// un-upgraded daemon's coarse rows are caught by the error text too.
fn resume_unsafe_here(failure_reason: &str, err_msg: &str) -> bool {
    crate::task_helpers::resume_unsafe_failure(failure_reason, err_msg)
}
