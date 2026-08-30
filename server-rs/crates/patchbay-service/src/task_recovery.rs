//! Delegated failure recovery — port of `service/task.go` L5132-5689.
//!
//! When a delegated task reaches a terminal failure with no retry pending,
//! a platform-authored recovery comment on the source coordinator's issue
//! hands control back to the coordinating agent. The comment is a durable
//! outbox: `recover_pending_delegated_failures` replays it after crashes,
//! and bounded automatic attempts end in one visible exhaustion outcome.

use serde_json::{json, Value};
use uuid::Uuid;

use patchbay_db::dbid::new_v7;
use patchbay_db::models::{Agent, AgentTaskQueue, Comment, InboxItem, Issue};
use patchbay_db::queries::agent::{
    acknowledge_exhausted_delegated_failure_recovery, count_delegated_failure_recovery_tasks,
    create_agent_task, get_agent_task, get_agent_task_for_delegated_failure_update,
    has_retry_task_for_parent, has_task_covering_delegated_failure_comment,
    list_pending_delegated_failure_recoveries, merge_delegated_failure_comment_into_pending_task,
    register_planned_comment_for_active_task,
};
use patchbay_db::queries::comment::{
    create_comment, get_delegated_failure_recovery_comment,
    get_delegated_failure_recovery_exhaustion_comment, CreateCommentRow,
};
use patchbay_db::queries::inbox::create_inbox_item;
use patchbay_db::queries::issue::get_issue;
use patchbay_db::queries::member::get_member_by_user_and_workspace;

use crate::attribution::{self, Source};
use crate::issue_status;
use crate::redact;
use crate::task_helpers::{priority_to_int, truncate_for_summary, TRIGGER_SUMMARY_MAX_LEN};
use crate::task_service::{
    downcast_sqlx, issue_task_context, opt_str, overlay_value_or_null, TaskService,
    TaskServiceError, COORDINATION_ISSUE_REVISION_CONTEXT_KEY,
    COORDINATION_OWNER_GENERATION_CONTEXT_KEY, COORDINATION_OWNER_ID_CONTEXT_KEY,
    COORDINATION_OWNER_TYPE_CONTEXT_KEY,
};

pub const DELEGATED_FAILURE_ERROR_SUMMARY_RUNES: usize = 800;
pub const DELEGATED_FAILURE_RECOVERY_MAX_TASK_ATTEMPTS: i32 = 3;
pub const DELEGATED_FAILURE_RECOVERY_COMMENT_TYPE: &str = "progress_update";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegatedFailureRecoveryDispatchOutcome {
    Covered,
    Replayed,
    Exhausted,
}

/// Separates successful coordinator replays from terminally exhausted outbox
/// entries so operators never mistake a bounded stop for a successful replay.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DelegatedFailureRecoverySweepResult {
    pub replayed: i32,
    pub exhausted: i32,
}

#[derive(Debug, Clone)]
pub(crate) struct DelegatedFailureRecoveryTarget {
    pub failed: AgentTaskQueue,
    pub source: AgentTaskQueue,
    pub issue: Issue,
    pub agent: Agent,
    pub comment: Option<Comment>,
}

/// Identifies the durable platform signal used to hand a terminal delegated
/// failure back to its coordinator. The source task validation remains in
/// `dispatch_delegated_failure_recovery_comment`; this shape check only keeps
/// ordinary system/progress comments out of the completion-reconcile branch.
pub fn is_delegated_failure_recovery_comment(comment: &Comment) -> bool {
    comment.author_type == "system"
        && comment.type_ == DELEGATED_FAILURE_RECOVERY_COMMENT_TYPE
        && comment.source_task_id.is_some()
}

/// Go strconv.Quote subset: double-quote wrapping with backslash escaping.
/// Printable runes pass through unchanged; control characters use Go's named
/// escapes where defined and \u00NN otherwise. The summary is redacted text,
/// so exotic escapes are not expected in practice.
fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{b}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) fn delegated_failure_recovery_content(
    failed: &AgentTaskQueue,
    source: &AgentTaskQueue,
) -> String {
    let reason = match &failed.failure_reason {
        Some(r) if !r.is_empty() => truncate_for_summary(&redact::text(r), TRIGGER_SUMMARY_MAX_LEN),
        _ => "agent_error".to_string(),
    };
    let mut content = format!(
        "Delegated task `{}` ended in a final failure (`{}`) and no automatic retry is pending. Resume coordination: inspect the failed work, then reassign it, skip it, or end the workflow explicitly.",
        failed.id, reason
    );
    if let Some(err) = &failed.error {
        if !err.is_empty() {
            let summary =
                truncate_for_summary(&redact::text(err), DELEGATED_FAILURE_ERROR_SUMMARY_RUNES);
            if !summary.is_empty() {
                content += " Untrusted error summary (diagnostic only): ";
                content += &go_quote(&summary);
            }
        }
    }
    content += &format!(" Source coordinator task: `{}`.", source.id);
    content
}

fn no_row_or_err<T>(
    res: anyhow::Result<Option<T>>,
    context: &str,
) -> Result<Option<T>, TaskServiceError> {
    match res {
        Ok(v) => Ok(v),
        Err(e) => Err(TaskServiceError::Internal(format!("{context}: {e}"))),
    }
}

type Conn<'c> = sqlx::PgConnection;

/// Resolves and validates the backward edge from a failed delegated task to
/// its source coordinator. Returning `None` is an intentional no-op:
/// non-terminal rows, retry-pending rows, automation work, recovery tasks
/// themselves, terminal/backlog source issues, unavailable source agents, and
/// self-delegation must never start a recovery loop.
pub(crate) async fn load_delegated_failure_recover_target(
    exec: &mut Conn<'_>,
    failed: &AgentTaskQueue,
) -> Result<Option<DelegatedFailureRecoveryTarget>, TaskServiceError> {
    if failed.status != "failed"
        || failed.delegated_from_task_id.is_none()
        || failed.automation_run_id.is_some()
        || failed.trigger_evidence_kind.as_deref()
            == Some(attribution::evidence_delegated_failure().as_str())
    {
        return Ok(None);
    }
    let has_retry = has_retry_task_for_parent(&mut *exec, failed.id)
        .await
        .map_err(downcast_sqlx)?;
    if has_retry.unwrap_or(false) {
        return Ok(None);
    }
    let Some(delegated_from) = failed.delegated_from_task_id else {
        return Ok(None);
    };
    let Some(source) = no_row_or_err(
        get_agent_task(&mut *exec, delegated_from).await,
        "load source task",
    )?
    else {
        return Ok(None);
    };
    if source.automation_run_id.is_some()
        || source.issue_id.is_none()
        || source.agent_id == failed.agent_id
    {
        return Ok(None);
    }
    let Some(source_issue_id) = source.issue_id else {
        return Ok(None);
    };
    let Some(issue) = no_row_or_err(
        get_issue(&mut *exec, source_issue_id).await,
        "load source issue",
    )?
    else {
        return Ok(None);
    };
    let effective_status =
        issue_status::effective(&mut *exec, issue.workspace_id, &issue.status).await;
    if effective_status == issue_status::DONE
        || effective_status == issue_status::CANCELLED
        || effective_status == issue_status::BACKLOG
    {
        return Ok(None);
    }
    let Some(agent) = no_row_or_err(
        patchbay_db::queries::agent::get_agent(&mut *exec, source.agent_id).await,
        "load source agent",
    )?
    else {
        return Ok(None);
    };
    if agent.archived_at.is_some()
        || agent.runtime_id.is_none()
        || agent.workspace_id != issue.workspace_id
    {
        return Ok(None);
    }
    Ok(Some(DelegatedFailureRecoveryTarget {
        failed: failed.clone(),
        source,
        issue,
        agent,
        comment: None,
    }))
}

pub(crate) fn delegated_failure_recovery_exhaustion_content(
    target: &DelegatedFailureRecoveryTarget,
) -> String {
    format!(
        "Automatic recovery for delegated task `{}` stopped after {} coordinator tasks ended before receiving the recovery signal. No more recovery tasks will be created automatically; resume or dismiss the work manually. Source coordinator task: `{}`.",
        target.failed.id, DELEGATED_FAILURE_RECOVERY_MAX_TASK_ATTEMPTS, target.source.id
    )
}

pub(crate) fn delegated_failure_recovery_attribution(
    target: &DelegatedFailureRecoveryTarget,
) -> (Option<Uuid>, Option<Uuid>) {
    let mut originator = target.failed.originator_user_id;
    let mut accountable = target.failed.accountable_user_id;
    if originator.is_some() {
        accountable = originator;
    }
    if originator.is_none() && accountable.is_none() {
        originator = target.source.originator_user_id;
        accountable = target.source.accountable_user_id;
        if originator.is_some() {
            accountable = originator;
        }
    }
    (originator, accountable)
}

fn delegated_failure_recovery_context(
    target: &DelegatedFailureRecoveryTarget,
    owner_generation: i64,
) -> Value {
    let mut context = serde_json::Map::new();
    if let Some(source_context) = target.source.context.as_ref().and_then(Value::as_object) {
        for key in [
            COORDINATION_OWNER_TYPE_CONTEXT_KEY,
            COORDINATION_OWNER_ID_CONTEXT_KEY,
            COORDINATION_OWNER_GENERATION_CONTEXT_KEY,
            COORDINATION_ISSUE_REVISION_CONTEXT_KEY,
        ] {
            if let Some(value) = source_context.get(key) {
                context.insert(key.to_string(), value.clone());
            }
        }
    }
    let has_owner_identity = context
        .get(COORDINATION_OWNER_TYPE_CONTEXT_KEY)
        .and_then(Value::as_str)
        .is_some()
        && context
            .get(COORDINATION_OWNER_ID_CONTEXT_KEY)
            .and_then(Value::as_str)
            .is_some();
    if has_owner_identity {
        Value::Object(context)
    } else {
        issue_task_context(&target.issue, None, Some(owner_generation))
    }
}

/// The generated CreateCommentRow returns nullable identity columns; the
/// recovery inserts always provide them, so unwrap into the full Comment
/// shape the events carry.
fn comment_from_row(row: CreateCommentRow, workspace_id: Uuid) -> Comment {
    Comment {
        author_id: row.author_id.unwrap_or_else(Uuid::nil),
        author_type: row.author_type,
        content: row.content,
        created_at: row.created_at.expect("inserted comment created_at"),
        id: row.id.expect("inserted comment id"),
        issue_id: row.issue_id.expect("inserted comment issue_id"),
        parent_id: row.parent_id,
        quick_action_id: row.quick_action_id,
        resolved_at: row.resolved_at,
        resolved_by_id: row.resolved_by_id,
        resolved_by_type: row.resolved_by_type,
        revision: row.revision,
        source_task_id: row.source_task_id,
        type_: row.type_,
        updated_at: row.updated_at.expect("inserted comment updated_at"),
        via_plugin_id: row.via_plugin_id,
        workspace_id,
    }
}

/// System-authored comment payload sub-map, field-for-field with the Go
/// recovery broadcasts (which carry no `revision` key).
fn comment_payload(comment: &Comment) -> serde_json::Value {
    json!({
        "id": comment.id.to_string(),
        "issue_id": comment.issue_id.to_string(),
        "author_type": comment.author_type,
        "author_id": comment.author_id.to_string(),
        "content": comment.content,
        "type": comment.type_,
        "parent_id": comment.parent_id.map(|p| p.to_string()),
        "source_task_id": comment.source_task_id.map(|s| s.to_string()),
        "created_at": comment
            .created_at
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    })
}

impl TaskService {
    /// Creates one durable recovery signal per failed task. The failed row
    /// lock serializes FailTask and sweeper callers; the comment itself is
    /// platform-authored so it does not create subscriber or
    /// mention-notification side effects.
    pub(crate) async fn ensure_delegated_failure_recovery_comment(
        &self,
        failed_id: Uuid,
    ) -> Result<(Option<DelegatedFailureRecoveryTarget>, bool), TaskServiceError> {
        let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;
        let locked = get_agent_task_for_delegated_failure_update(&mut *tx, failed_id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("lock failed task: {e}")))?
            .ok_or_else(|| TaskServiceError::Internal("lock failed task: not found".into()))?;
        let mut target = match load_delegated_failure_recover_target(&mut tx, &locked).await? {
            Some(t) => t,
            None => {
                tx.commit().await.map_err(TaskServiceError::Sql)?;
                return Ok((None, false));
            }
        };

        let existing = get_delegated_failure_recovery_comment(
            &mut *tx,
            target.issue.id,
            target.issue.workspace_id,
            locked.id,
        )
        .await
        .map_err(|e| TaskServiceError::Internal(format!("find recovery comment: {e}")))?;
        let mut created = false;
        match existing {
            Some(comment) => target.comment = Some(comment),
            None => {
                let row = create_comment(
                    &mut *tx,
                    target.issue.id,
                    target.issue.workspace_id,
                    "system",
                    Uuid::nil(),
                    &delegated_failure_recovery_content(&target.failed, &target.source),
                    DELEGATED_FAILURE_RECOVERY_COMMENT_TYPE,
                    None,
                    Some(locked.id),
                    None,
                    None,
                    new_v7(),
                )
                .await
                .map_err(|e| TaskServiceError::Internal(format!("create recovery comment: {e}")))?
                .expect("recovery comment insert returns row");
                target.comment = Some(comment_from_row(row, target.issue.workspace_id));
                created = true;
            }
        }
        tx.commit().await.map_err(TaskServiceError::Sql)?;

        if created {
            if let Some(comment) = &target.comment {
                self.bus.publish(&patchbay_events::Event {
                    event_type: patchbay_protocol::EVENT_COMMENT_CREATED.to_string(),
                    workspace_id: target.issue.workspace_id.to_string(),
                    actor_type: "system".to_string(),
                    actor_id: String::new(),
                    payload: json!({
                        "comment": comment_payload(comment),
                        "issue_title": target.issue.title,
                        "issue_status": target.issue.status,
                    }),
                    task_id: String::new(),
                    chat_session_id: String::new(),
                });
            }
        }
        Ok((Some(target), created))
    }

    /// Atomically settles the recovery outbox after its bounded automatic
    /// attempts, creates one visible system explanation, and notifies the
    /// responsible human. The bool reports whether this caller created that
    /// terminal outcome. Updating the newest attempt first serializes
    /// concurrent sweepers; the second caller then observes the explanation
    /// written by the first and does not report another exhaustion.
    pub(crate) async fn exhaust_delegated_failure_recovery(
        &self,
        target: &DelegatedFailureRecoveryTarget,
    ) -> Result<bool, TaskServiceError> {
        let exhausted_comment: Comment;
        let mut exhausted_inbox: Option<InboxItem> = None;
        let mut created = false;

        let mut tx = self.pool.begin().await.map_err(TaskServiceError::Sql)?;
        acknowledge_exhausted_delegated_failure_recovery(
            &mut *tx,
            target.comment.as_ref().expect("recovery comment").id,
            target.failed.id,
            DELEGATED_FAILURE_RECOVERY_MAX_TASK_ATTEMPTS,
        )
        .await
        .map_err(|e| {
            TaskServiceError::Internal(format!(
                "acknowledge exhausted delegated failure recovery: {e}"
            ))
        })?;

        let existing = get_delegated_failure_recovery_exhaustion_comment(
            &mut *tx,
            target.issue.id,
            target.issue.workspace_id,
            target.failed.id,
        )
        .await
        .map_err(|e| {
            TaskServiceError::Internal(format!("find delegated failure exhaustion comment: {e}"))
        })?;
        match existing {
            Some(comment) => exhausted_comment = comment,
            None => {
                let row = create_comment(
                    &mut *tx,
                    target.issue.id,
                    target.issue.workspace_id,
                    "system",
                    Uuid::nil(),
                    &delegated_failure_recovery_exhaustion_content(target),
                    "system",
                    None,
                    Some(target.failed.id),
                    None,
                    None,
                    new_v7(),
                )
                .await
                .map_err(|e| {
                    TaskServiceError::Internal(format!(
                        "create delegated failure exhaustion comment: {e}"
                    ))
                })?
                .expect("exhaustion comment insert returns row");
                exhausted_comment = comment_from_row(row, target.issue.workspace_id);
                created = true;

                // Exhaustion deliberately does not @mention the coordinator
                // agent: doing so would enqueue a fourth recovery run and
                // defeat the attempt bound. Instead, create one durable
                // action-required inbox item for the human who originated (or
                // is accountable for) the delegated work.
                let (_, recipient) = delegated_failure_recovery_attribution(target);
                if let Some(recipient) = recipient {
                    match get_member_by_user_and_workspace(
                        &mut *tx,
                        recipient,
                        target.issue.workspace_id,
                    )
                    .await
                    {
                        Ok(Some(_)) => {
                            let details = json!({
                                "failed_task_id": target.failed.id.to_string(),
                                "source_task_id": target.source.id.to_string(),
                                "coordinator_agent_id": target.agent.id.to_string(),
                                "max_attempts": DELEGATED_FAILURE_RECOVERY_MAX_TASK_ATTEMPTS,
                            });
                            let inbox = create_inbox_item(
                                &mut *tx,
                                target.issue.workspace_id,
                                "member",
                                recipient,
                                "task_failed",
                                "action_required",
                                Some(target.issue.id),
                                &target.issue.title,
                                Some(exhausted_comment.content.as_str()),
                                Some("system"),
                                Uuid::nil(),
                                &details,
                                new_v7(),
                            )
                            .await
                            .map_err(|e| {
                                TaskServiceError::Internal(format!(
                                    "create delegated failure exhaustion inbox item: {e}"
                                ))
                            })?
                            .expect("exhaustion inbox insert returns row");
                            exhausted_inbox = Some(inbox);
                        }
                        // Recipient left the workspace: keep the comment,
                        // skip the inbox write (Go returns nil mid-tx).
                        Ok(None) => {}
                        Err(e) => {
                            return Err(TaskServiceError::Internal(format!(
                                "validate delegated failure exhaustion recipient: {e}"
                            )));
                        }
                    }
                }
            }
        }
        tx.commit().await.map_err(TaskServiceError::Sql)?;

        if created {
            self.bus.publish(&patchbay_events::Event {
                event_type: patchbay_protocol::EVENT_COMMENT_CREATED.to_string(),
                workspace_id: target.issue.workspace_id.to_string(),
                actor_type: "system".to_string(),
                actor_id: String::new(),
                payload: json!({
                    "comment": comment_payload(&exhausted_comment),
                    "issue_title": target.issue.title,
                    "issue_status": target.issue.status,
                }),
                task_id: String::new(),
                chat_session_id: String::new(),
            });
        }
        if let Some(item) = &exhausted_inbox {
            self.bus.publish(&patchbay_events::Event {
                event_type: patchbay_protocol::EVENT_INBOX_NEW.to_string(),
                workspace_id: target.issue.workspace_id.to_string(),
                actor_type: "system".to_string(),
                actor_id: String::new(),
                payload: json!({
                    "item": {
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
                        "created_at": item
                            .created_at
                            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                        "actor_type": item.actor_type.clone(),
                        "actor_id": item.actor_id.map(|a| a.to_string()),
                        "details": item.details.clone(),
                        "issue_status": target.issue.status,
                    }
                }),
                task_id: String::new(),
                chat_session_id: String::new(),
            });
        }
        Ok(created)
    }

    /// Routes a recovery comment to the source coordinator without relying on
    /// generic mention parsing. A pre-claim task absorbs it; otherwise a
    /// dedicated queued successor is created. The only state that blocks both
    /// writes is a dispatched task (its claim is already built and the
    /// pending-task uniqueness slot is still held), so that narrow race
    /// records the comment as planned-but-undelivered and lets completion
    /// reconciliation schedule the follow-up. The three-pass loop closes
    /// state changes around those writes.
    pub(crate) async fn dispatch_delegated_failure_recovery(
        &self,
        target: &DelegatedFailureRecoveryTarget,
        exclude_task_id: Option<Uuid>,
    ) -> Result<DelegatedFailureRecoveryDispatchOutcome, TaskServiceError> {
        const MAX_ATTEMPTS: usize = 3;
        let comment_id = target.comment.as_ref().expect("recovery comment").id;
        for _attempt in 0..MAX_ATTEMPTS {
            let covered = has_task_covering_delegated_failure_comment(
                &self.pool,
                target.issue.id,
                target.agent.id,
                comment_id,
                exclude_task_id.unwrap_or_else(Uuid::nil),
            )
            .await
            .map_err(|e| TaskServiceError::Internal(format!("check recovery coverage: {e}")))?;
            if covered.unwrap_or(false) {
                return Ok(DelegatedFailureRecoveryDispatchOutcome::Covered);
            }

            let recovery_tasks =
                count_delegated_failure_recovery_tasks(&self.pool, target.failed.id)
                    .await
                    .map_err(|e| {
                        TaskServiceError::Internal(format!(
                            "count delegated failure recovery tasks: {e}"
                        ))
                    })?;
            if recovery_tasks.unwrap_or(0)
                >= i64::from(DELEGATED_FAILURE_RECOVERY_MAX_TASK_ATTEMPTS)
            {
                let exhausted = self.exhaust_delegated_failure_recovery(target).await?;
                if exhausted {
                    return Ok(DelegatedFailureRecoveryDispatchOutcome::Exhausted);
                }
                return Ok(DelegatedFailureRecoveryDispatchOutcome::Covered);
            }

            let trigger_summary = self
                .build_comment_trigger_summary(target.issue.workspace_id, Some(comment_id))
                .await
                .map_err(|e| TaskServiceError::Internal(e.to_string()))?;
            match merge_delegated_failure_comment_into_pending_task(
                &self.pool,
                comment_id,
                trigger_summary.as_deref(),
                target.issue.id,
                target.agent.id,
            )
            .await
            {
                Ok(Some(merged)) => {
                    tracing::info!(
                        failed_task_id = %target.failed.id,
                        coordinator_task_id = %merged.id,
                        "delegated failure recovery merged into pending coordinator task"
                    );
                    return Ok(DelegatedFailureRecoveryDispatchOutcome::Replayed);
                }
                // No pending task absorbed it — fall through to creating one.
                Ok(None) => {}
                Err(e) => {
                    return Err(TaskServiceError::Internal(format!(
                        "merge recovery into pending task: {e}"
                    )));
                }
            }

            let (originator, accountable) = delegated_failure_recovery_attribution(target);
            let mut source = Source::delegation();
            if originator.is_none() && accountable.is_none() {
                source = Source::unattributed();
            }
            let rule_version_id = target
                .failed
                .rule_version_id
                .or(target.source.rule_version_id);
            let overlay = self
                .build_runtime_mcp_overlay(originator.unwrap_or_else(Uuid::nil), &target.agent)
                .await;
            let head_sha = self.resolve_issue_review_sha(target.issue.id).await;
            let owner_generation: i64 = sqlx::query_scalar(
                "SELECT assignee_generation FROM issue WHERE id = $1 AND workspace_id = $2",
            )
            .bind(target.issue.id)
            .bind(target.issue.workspace_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|error| {
                TaskServiceError::Internal(format!(
                    "load delegated recovery owner generation: {error}"
                ))
            })?;
            let context = delegated_failure_recovery_context(target, owner_generation);
            let created = create_agent_task(
                &self.pool,
                target.agent.id,
                target.agent.runtime_id.unwrap_or_else(Uuid::nil),
                target.issue.id,
                priority_to_int(&target.issue.priority),
                comment_id,
                Vec::new(),
                trigger_summary.as_deref(),
                None,
                Some(target.source.is_leader_task),
                None,
                target.source.team_id.unwrap_or_else(Uuid::nil),
                opt_str(&head_sha),
                originator.unwrap_or_else(Uuid::nil),
                accountable.unwrap_or_else(Uuid::nil),
                &overlay_value_or_null(&overlay.overlay),
                &overlay_value_or_null(&overlay.connected_apps),
                Some(source.as_str()),
                target.failed.id,
                rule_version_id.unwrap_or_else(Uuid::nil),
                Uuid::nil(),
                Some(attribution::evidence_delegated_failure().as_str()),
                target.failed.id,
                new_v7(),
                &context,
                "queued",
            )
            .await;
            match created {
                Ok(Some(task)) => {
                    tracing::info!(
                        failed_task_id = %target.failed.id,
                        source_task_id = %target.source.id,
                        recovery_task_id = %task.id,
                        coordinator_agent_id = %target.agent.id,
                        "delegated failure recovery task enqueued"
                    );
                    self.broadcast_task_event(
                        patchbay_protocol::EVENT_TASK_QUEUED,
                        &task,
                        Default::default(),
                    )
                    .await;
                    self.notify_task_enqueued(&task).await;
                    return Ok(DelegatedFailureRecoveryDispatchOutcome::Replayed);
                }
                Ok(None) => {}
                Err(e) => {
                    let sqlx_err = downcast_sqlx(e);
                    if !crate::task_service::is_duplicate_pending_task_err(&sqlx_err) {
                        return Err(TaskServiceError::Sql(sqlx_err));
                    }
                }
            }

            // A dispatched task still owns the unique queued/dispatched slot,
            // but its claim payload is immutable. Register the comment as
            // undelivered so its completion reconciliation creates the
            // successor. Running tasks do not own that slot, so they took the
            // durable queued-successor path above.
            match register_planned_comment_for_active_task(
                &self.pool,
                comment_id,
                target.issue.id,
                target.agent.id,
                opt_str(&head_sha),
            )
            .await
            {
                Ok(Some(active)) => {
                    tracing::info!(
                        failed_task_id = %target.failed.id,
                        coordinator_task_id = %active.id.unwrap_or_default(),
                        "delegated failure recovery registered behind dispatched coordinator task"
                    );
                    return Ok(DelegatedFailureRecoveryDispatchOutcome::Replayed);
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(TaskServiceError::Internal(format!(
                        "register recovery on dispatched task: {e}"
                    )));
                }
            }
        }
        Err(TaskServiceError::Internal(
            "delegate failure recovery could not acquire coordinator task slot".into(),
        ))
    }

    /// The shared post-terminal hook for FailTask and HandleFailedTasks.
    /// Returns whether the failure was an eligible delegated terminal; the
    /// production terminal paths deliberately retain their legacy raw-error
    /// notice alongside the richer coordinator recovery signal.
    pub(crate) async fn recover_delegated_task_failure(
        &self,
        failed: &AgentTaskQueue,
    ) -> Result<bool, TaskServiceError> {
        let (target, _) = self
            .ensure_delegated_failure_recovery_comment(failed.id)
            .await?;
        let Some(target) = target else {
            return Ok(false);
        };
        self.dispatch_delegated_failure_recovery(&target, None)
            .await?;
        Ok(true)
    }

    /// Replays the durable recovery outbox. The platform recovery comment is
    /// the obligation; it is complete only while a task that carries it can
    /// still execute, or after a task records it in delivered_comment_ids.
    /// This lets a later sweeper repair a process crash or transient database
    /// error between comment creation and coordinator dispatch without
    /// producing duplicate runnable tasks.
    pub async fn recover_pending_delegated_failures(
        &self,
        max_per_tick: i32,
    ) -> Result<DelegatedFailureRecoverySweepResult, TaskServiceError> {
        let mut result = DelegatedFailureRecoverySweepResult::default();
        if max_per_tick <= 0 {
            return Ok(result);
        }
        let pending = list_pending_delegated_failure_recoveries(&self.pool, max_per_tick)
            .await
            .map_err(|e| {
                TaskServiceError::Internal(format!(
                    "list pending delegated failure recoveries: {e}"
                ))
            })?;

        let mut errs: Vec<String> = Vec::new();
        for comment in pending {
            match self
                .dispatch_delegated_failure_recovery_comment_inner(&comment, None)
                .await
            {
                Ok(outcome) => match outcome {
                    DelegatedFailureRecoveryDispatchOutcome::Replayed => result.replayed += 1,
                    DelegatedFailureRecoveryDispatchOutcome::Exhausted => result.exhausted += 1,
                    DelegatedFailureRecoveryDispatchOutcome::Covered => {}
                },
                Err(recovery_err) => {
                    errs.push(format!(
                        "dispatch recovery comment {}: {recovery_err}",
                        comment.id
                    ));
                }
            }
        }
        if errs.is_empty() {
            Ok(result)
        } else {
            // errors.Join equivalent: every failure surfaces together.
            Err(TaskServiceError::Internal(errs.join("\n")))
        }
    }

    /// Used by completion reconciliation when a recovery signal arrived after
    /// a coordinator task was claimed. The completed task is excluded from the
    /// coverage check because the comment was planned but not delivered to it;
    /// routing then merges/enqueues exactly one follow-up.
    pub async fn dispatch_delegated_failure_recovery_comment(
        &self,
        comment: &Comment,
        completed_task_id: Option<Uuid>,
    ) -> Result<(), TaskServiceError> {
        self.dispatch_delegated_failure_recovery_comment_inner(comment, completed_task_id)
            .await?;
        Ok(())
    }

    async fn dispatch_delegated_failure_recovery_comment_inner(
        &self,
        comment: &Comment,
        completed_task_id: Option<Uuid>,
    ) -> Result<DelegatedFailureRecoveryDispatchOutcome, TaskServiceError> {
        if !is_delegated_failure_recovery_comment(comment) {
            return Ok(DelegatedFailureRecoveryDispatchOutcome::Covered);
        }
        let Some(source_task_id) = comment.source_task_id else {
            return Ok(DelegatedFailureRecoveryDispatchOutcome::Covered);
        };
        let failed = get_agent_task(&self.pool, source_task_id)
            .await
            .map_err(|e| TaskServiceError::Internal(format!("load failed recovery source: {e}")))?
            .ok_or_else(|| {
                TaskServiceError::Internal("load failed recovery source: not found".into())
            })?;
        let mut conn = self.pool.acquire().await.map_err(TaskServiceError::Sql)?;
        let Some(mut target) = load_delegated_failure_recover_target(&mut conn, &failed).await?
        else {
            return Ok(DelegatedFailureRecoveryDispatchOutcome::Covered);
        };
        if target.issue.id != comment.issue_id || target.issue.workspace_id != comment.workspace_id
        {
            return Err(TaskServiceError::Internal(
                "delegated failure recovery comment scope mismatch".into(),
            ));
        }
        target.comment = Some(comment.clone());
        self.dispatch_delegated_failure_recovery(&target, completed_task_id)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use chrono::Utc;
    use patchbay_db::queries::{agent, comment};
    use patchbay_events::{Bus, Event};
    use sqlx::PgPool;
    use tokio::sync::Barrier;

    // Recovery scans are workspace-wide.  Use a transaction-scoped advisory
    // lock so the handler and service contract tests (which run in separate
    // test binaries) cannot consume one another's pending rows.
    const CONTRACT_DB_LOCK_KEY: i64 = 0x434f_5244_595f_5357;

    struct ContractDbLock {
        _transaction: sqlx::Transaction<'static, sqlx::Postgres>,
    }

    impl ContractDbLock {
        async fn acquire(pool: &PgPool) -> anyhow::Result<Self> {
            let mut transaction = pool.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(CONTRACT_DB_LOCK_KEY)
                .execute(&mut *transaction)
                .await?;
            Ok(Self {
                _transaction: transaction,
            })
        }
    }

    async fn cleanup_workspace(
        pool: &PgPool,
        workspace_id: Uuid,
        user_id: Uuid,
    ) -> anyhow::Result<()> {
        // Recovery fixtures contain tasks, comments, inbox rows, agents, and
        // runtimes. Reuse the production dependency order so teardown is
        // complete even when a test fails halfway through an assertion.
        let mut tx = pool.begin().await?;
        let task_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM agent_task_queue WHERE agent_id IN (SELECT id FROM agent WHERE workspace_id = $1) \
             OR issue_id IN (SELECT id FROM issue WHERE workspace_id = $1) \
             OR runtime_id IN (SELECT id FROM agent_runtime WHERE workspace_id = $1)",
        )
        .bind(workspace_id)
        .fetch_all(&mut *tx)
        .await?;
        if !task_ids.is_empty() {
            patchbay_db::queries::workspace_delete::detach_task_batch_references(
                &mut *tx,
                task_ids.clone(),
            )
            .await?;
            patchbay_db::queries::workspace_delete::delete_task_batch(&mut *tx, task_ids).await?;
        }
        patchbay_db::queries::workspace_delete::delete_workspace_leaf_data(&mut *tx, workspace_id)
            .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_automation_runs(
            &mut *tx,
            workspace_id,
        )
        .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_automation_quota_reservations(
            &mut *tx,
            workspace_id,
        )
        .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_automation_quota_periods(
            &mut *tx,
            workspace_id,
        )
        .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_chat_messages(
            &mut *tx,
            workspace_id,
        )
        .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_communication_roots(
            &mut *tx,
            workspace_id,
        )
        .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_comments(&mut *tx, workspace_id)
            .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_issue_roots(
            &mut *tx,
            workspace_id,
        )
        .await?;
        patchbay_db::queries::issue_status::delete_issue_status_entries_for_workspace(
            &mut *tx,
            workspace_id,
        )
        .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_automation_children(
            &mut *tx,
            workspace_id,
        )
        .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_automations(&mut *tx, workspace_id)
            .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_pull_requests(
            &mut *tx,
            workspace_id,
        )
        .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_connections(
            &mut *tx,
            workspace_id,
        )
        .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_teams_and_skills(
            &mut *tx,
            workspace_id,
        )
        .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_plugin_data(
            &mut *tx,
            workspace_id,
        )
        .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_agents(&mut *tx, workspace_id)
            .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_runtimes_and_projects(
            &mut *tx,
            workspace_id,
        )
        .await?;
        patchbay_db::queries::workspace_delete::delete_workspace_administration(
            &mut *tx,
            workspace_id,
        )
        .await?;
        patchbay_db::queries::workspace::delete_workspace(&mut *tx, workspace_id).await?;
        tx.commit().await?;
        sqlx::query("DELETE FROM \"user\" WHERE id = $1")
            .bind(user_id)
            .execute(pool)
            .await?;
        Ok(())
    }

    struct SetupCleanupGuard {
        pool: PgPool,
        workspace_id: Uuid,
        user_id: Uuid,
        armed: bool,
    }

    impl SetupCleanupGuard {
        fn new(pool: PgPool, workspace_id: Uuid) -> Self {
            Self {
                pool,
                workspace_id,
                user_id: Uuid::nil(),
                armed: true,
            }
        }

        fn disarm(&mut self) {
            self.armed = false;
        }
    }

    impl Drop for SetupCleanupGuard {
        fn drop(&mut self) {
            if !self.armed {
                return;
            }
            let pool = self.pool.clone();
            let workspace_id = self.workspace_id;
            let user_id = self.user_id;
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = cleanup_workspace(&pool, workspace_id, user_id).await;
                });
            }
        }
    }

    struct RecoveryRows {
        pool: PgPool,
        workspace_id: Uuid,
        user_id: Uuid,
        _lock: ContractDbLock,
        runtime_id: Uuid,
        coordinator_id: Uuid,
        worker_id: Uuid,
        source_issue_id: Uuid,
        worker_issue_id: Uuid,
        source_task_id: Uuid,
    }

    impl RecoveryRows {
        async fn required() -> anyhow::Result<Self> {
            let url = std::env::var("DATABASE_URL")
                .expect("DATABASE_URL is required for delegated recovery contracts");
            let pool = PgPool::connect(&url).await?;
            let lock = ContractDbLock::acquire(&pool).await?;
            let workspace_id = new_v7();
            sqlx::query("INSERT INTO workspace (id, name, slug) VALUES ($1, $2, $3)")
                .bind(workspace_id)
                .bind("Rust delegated recovery contract")
                .bind(format!("rust-delegated-recovery-{workspace_id}"))
                .execute(&pool)
                .await?;
            let mut setup_cleanup = SetupCleanupGuard::new(pool.clone(), workspace_id);
            let suffix = workspace_id.simple().to_string();
            let user_id: Uuid = sqlx::query_scalar(
                "INSERT INTO \"user\" (name, email) VALUES ($1, $2) RETURNING id",
            )
            .bind("delegated recovery contract user")
            .bind(format!("delegated-recovery-{suffix}@example.test"))
            .fetch_one(&pool)
            .await?;
            setup_cleanup.user_id = user_id;
            sqlx::query(
                "INSERT INTO member (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
            )
            .bind(workspace_id)
            .bind(user_id)
            .execute(&pool)
            .await?;

            let runtime_id = new_v7();
            sqlx::query(
                "INSERT INTO agent_runtime (id, workspace_id, daemon_id, name, runtime_mode, provider, status, last_seen_at) \
                 VALUES ($1, $2, $3, $4, 'local', $5, 'online', now())",
            )
            .bind(runtime_id)
            .bind(workspace_id)
            .bind(format!("delegated-recovery-{runtime_id}"))
            .bind("Delegated recovery runtime")
            .bind(format!("delegated-recovery-{runtime_id}"))
            .execute(&pool)
            .await?;

            let coordinator_id =
                Self::agent(&pool, workspace_id, user_id, runtime_id, "coordinator").await?;
            let worker_id = Self::agent(&pool, workspace_id, user_id, runtime_id, "worker").await?;
            let source_issue_id =
                Self::issue(&pool, workspace_id, user_id, coordinator_id, 1, None).await?;
            let worker_issue_id = Self::issue(
                &pool,
                workspace_id,
                user_id,
                worker_id,
                2,
                Some(source_issue_id),
            )
            .await?;
            let source_task_id = Self::task(
                &pool,
                coordinator_id,
                runtime_id,
                source_issue_id,
                "completed",
                1,
                1,
                Some(Utc::now()),
                None,
                None,
                None,
                None,
                None,
            )
            .await?;
            // A real delegated source carries the human attribution copied by
            // the worker. Keeping it on the fixture exercises the exhaustion
            // inbox recipient path instead of silently taking the
            // unattributed branch.
            sqlx::query(
                "UPDATE agent_task_queue SET originator_user_id = $2, accountable_user_id = $2, originator_source = 'direct_human' WHERE id = $1",
            )
            .bind(source_task_id)
            .bind(user_id)
            .execute(&pool)
                .await?;

            setup_cleanup.disarm();

            Ok(Self {
                pool,
                workspace_id,
                user_id,
                _lock: lock,
                runtime_id,
                coordinator_id,
                worker_id,
                source_issue_id,
                worker_issue_id,
                source_task_id,
            })
        }

        async fn agent(
            pool: &PgPool,
            workspace_id: Uuid,
            owner_id: Uuid,
            runtime_id: Uuid,
            suffix: &str,
        ) -> anyhow::Result<Uuid> {
            let id = new_v7();
            sqlx::query(
                "INSERT INTO agent (id, workspace_id, name, runtime_mode, status, max_concurrent_tasks, owner_id, runtime_id) \
                 VALUES ($1, $2, $3, 'local', 'idle', 4, $4, $5)",
            )
            .bind(id)
            .bind(workspace_id)
            .bind(format!("Delegated {suffix} agent"))
            .bind(owner_id)
            .bind(runtime_id)
            .execute(pool)
            .await?;
            Ok(id)
        }

        async fn issue(
            pool: &PgPool,
            workspace_id: Uuid,
            creator_id: Uuid,
            assignee_id: Uuid,
            number: i32,
            parent_issue_id: Option<Uuid>,
        ) -> anyhow::Result<Uuid> {
            let id = new_v7();
            sqlx::query(
                "INSERT INTO issue (id, workspace_id, title, status, priority, creator_type, creator_id, assignee_type, assignee_id, parent_issue_id, number, position) \
                 VALUES ($1, $2, $3, 'in_progress', 'medium', 'member', $4, 'agent', $5, $6, $7, 0)",
            )
            .bind(id)
            .bind(workspace_id)
            .bind(format!("Delegated recovery issue {number}"))
            .bind(creator_id)
            .bind(assignee_id)
            .bind(parent_issue_id)
            .bind(number)
            .execute(pool)
            .await?;
            Ok(id)
        }

        #[allow(clippy::too_many_arguments)]
        async fn task(
            pool: &PgPool,
            agent_id: Uuid,
            runtime_id: Uuid,
            issue_id: Uuid,
            status: &str,
            attempt: i32,
            max_attempts: i32,
            completed_at: Option<chrono::DateTime<Utc>>,
            error: Option<&str>,
            failure_reason: Option<&str>,
            delegated_from_task_id: Option<Uuid>,
            trigger_evidence_kind: Option<&str>,
            trigger_evidence_ref_id: Option<Uuid>,
        ) -> anyhow::Result<Uuid> {
            let id = new_v7();
            sqlx::query(
                "INSERT INTO agent_task_queue (id, agent_id, runtime_id, issue_id, status, priority, attempt, max_attempts, completed_at, error, failure_reason, originator_user_id, accountable_user_id, originator_source, delegated_from_task_id, trigger_evidence_kind, trigger_evidence_ref_id, delivered_comment_ids) \
                 VALUES ($1, $2, $3, $4, $5, 0, $6, $7, $8, $9, $10, NULL, NULL, NULL, $11, $12, $13, '{}'::uuid[])",
            )
            .bind(id)
            .bind(agent_id)
            .bind(runtime_id)
            .bind(issue_id)
            .bind(status)
            .bind(attempt)
            .bind(max_attempts)
            .bind(completed_at)
            .bind(error)
            .bind(failure_reason)
            .bind(delegated_from_task_id)
            .bind(trigger_evidence_kind)
            .bind(trigger_evidence_ref_id)
            .execute(pool)
            .await?;
            Ok(id)
        }

        async fn worker_task(
            &self,
            status: &str,
            evidence_kind: &str,
            attempt: i32,
            max_attempts: i32,
            failure_reason: Option<&str>,
            error: Option<&str>,
        ) -> anyhow::Result<Uuid> {
            Self::task(
                &self.pool,
                self.worker_id,
                self.runtime_id,
                self.worker_issue_id,
                status,
                attempt,
                max_attempts,
                (status == "failed").then_some(Utc::now()),
                error,
                failure_reason,
                Some(self.source_task_id),
                Some(evidence_kind),
                None,
            )
            .await
        }

        async fn coordinator_task(&self, status: &str) -> anyhow::Result<Uuid> {
            let task_id = Self::task(
                &self.pool,
                self.coordinator_id,
                self.runtime_id,
                self.source_issue_id,
                status,
                1,
                1,
                (status == "failed").then_some(Utc::now()),
                None,
                None,
                None,
                Some("issue_assignment"),
                Some(self.source_issue_id),
            )
            .await?;
            if status == "dispatched" {
                sqlx::query("UPDATE agent_task_queue SET dispatched_at = now() WHERE id = $1")
                    .bind(task_id)
                    .execute(&self.pool)
                    .await?;
            } else if status == "running" {
                sqlx::query("UPDATE agent_task_queue SET dispatched_at = now(), started_at = now() WHERE id = $1")
                    .bind(task_id)
                    .execute(&self.pool)
                    .await?;
            }
            Ok(task_id)
        }

        async fn failed_task(&self, id: Uuid) -> anyhow::Result<AgentTaskQueue> {
            agent::get_agent_task(&self.pool, id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("failed task {id} missing"))
        }

        async fn recovery_comment(&self, failed_id: Uuid) -> anyhow::Result<Comment> {
            comment::get_delegated_failure_recovery_comment(
                &self.pool,
                self.source_issue_id,
                self.workspace_id,
                failed_id,
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("recovery comment for {failed_id} missing"))
        }

        async fn recovery_count(&self, failed_id: Uuid) -> anyhow::Result<i64> {
            Ok(sqlx::query_scalar(
                "SELECT count(*) FROM agent_task_queue WHERE trigger_evidence_kind = 'delegated_failure' AND trigger_evidence_ref_id = $1",
            )
            .bind(failed_id)
            .fetch_one(&self.pool)
            .await?)
        }

        async fn automation_run(&self) -> anyhow::Result<Uuid> {
            let automation_id: Uuid = sqlx::query_scalar(
                "INSERT INTO automation (workspace_id, title, assignee_type, assignee_id, execution_mode, created_by_type, created_by_id) \
                 VALUES ($1, 'delegated recovery contract', 'agent', $2, 'create_issue', 'member', $3) RETURNING id",
            )
            .bind(self.workspace_id)
            .bind(self.coordinator_id)
            .bind(self.user_id)
            .fetch_one(&self.pool)
            .await?;
            Ok(sqlx::query_scalar(
                "INSERT INTO automation_run (automation_id, source, status, issue_id) \
                 VALUES ($1, 'manual', 'running', $2) RETURNING id",
            )
            .bind(automation_id)
            .bind(self.source_issue_id)
            .fetch_one(&self.pool)
            .await?)
        }

        fn service(&self) -> (TaskService, Arc<Bus>, Arc<Mutex<Vec<Event>>>) {
            let bus = Arc::new(Bus::new());
            let events = Arc::new(Mutex::new(Vec::new()));
            let captured = events.clone();
            bus.subscribe_all(move |event| {
                captured
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(event.clone());
            });
            (
                TaskService::new(self.pool.clone(), bus.clone()),
                bus,
                events,
            )
        }

        fn service_arc(&self) -> (Arc<TaskService>, Arc<Bus>, Arc<Mutex<Vec<Event>>>) {
            let bus = Arc::new(Bus::new());
            let events = Arc::new(Mutex::new(Vec::new()));
            let captured = events.clone();
            bus.subscribe_all(move |event| {
                captured
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(event.clone());
            });
            (
                Arc::new(TaskService::new(self.pool.clone(), bus.clone())),
                bus,
                events,
            )
        }

        async fn cleanup(&self) -> anyhow::Result<()> {
            cleanup_workspace(&self.pool, self.workspace_id, self.user_id).await
        }
    }

    impl Drop for RecoveryRows {
        fn drop(&mut self) {
            let pool = self.pool.clone();
            let workspace_id = self.workspace_id;
            let user_id = self.user_id;
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = cleanup_workspace(&pool, workspace_id, user_id).await;
                });
            }
        }
    }

    #[tokio::test]
    async fn final_delegated_failure_creates_one_redacted_recovery_signal() {
        let rows = RecoveryRows::required().await.expect(
            "DATABASE_URL and migrated PostgreSQL are required for delegated recovery contract",
        );
        let result = async {
            let secret = format!("sk-{}", "a".repeat(24));
            let failed_id = rows
                .worker_task(
                    "failed",
                    "comment",
                    1,
                    1,
                    Some("agent_error.process_failure"),
                    Some(&format!("worker exited with {secret}")),
                )
                .await?;
            let failed = rows.failed_task(failed_id).await?;
            let (svc, _bus, events) = rows.service();
            anyhow::ensure!(svc.handle_failed_tasks(std::slice::from_ref(&failed)).await == 0, "final delegated failure unexpectedly retried");

            let recovery = rows.recovery_comment(failed_id).await?;
            anyhow::ensure!(recovery.author_type == "system", "recovery author = {}", recovery.author_type);
            anyhow::ensure!(recovery.type_ == DELEGATED_FAILURE_RECOVERY_COMMENT_TYPE, "recovery type = {}", recovery.type_);
            anyhow::ensure!(!recovery.content.contains(&secret), "recovery comment leaked the raw API key");
            anyhow::ensure!(recovery.content.contains("[REDACTED API KEY]"), "recovery comment did not redact the API key");
            anyhow::ensure!(recovery.content.contains(&failed_id.to_string()), "recovery comment omitted failed task id");
            anyhow::ensure!(recovery.content.contains(&rows.source_task_id.to_string()), "recovery comment omitted source task id");

            let recovery_row: (
                Uuid,
                Uuid,
                Uuid,
                Option<Uuid>,
                Option<Uuid>,
                Option<String>,
                serde_json::Value,
            ) = sqlx::query_as(
                "SELECT id, agent_id, issue_id, delegated_from_task_id, trigger_evidence_ref_id, trigger_evidence_kind, context FROM agent_task_queue WHERE trigger_evidence_kind = 'delegated_failure' AND trigger_evidence_ref_id = $1",
            )
            .bind(failed_id)
            .fetch_one(&rows.pool)
            .await?;
            anyhow::ensure!(recovery_row.1 == rows.coordinator_id && recovery_row.2 == rows.source_issue_id, "recovery target = {}/{}", recovery_row.1, recovery_row.2);
            anyhow::ensure!(recovery_row.3 == Some(failed_id) && recovery_row.4 == Some(failed_id) && recovery_row.5.as_deref() == Some("delegated_failure"), "recovery lineage is incomplete: {recovery_row:?}");
            anyhow::ensure!(
                recovery_row.6["coordination_owner_type"].as_str() == Some("agent")
                    && recovery_row.6["coordination_owner_id"]
                        .as_str()
                        .and_then(|value| Uuid::parse_str(value).ok())
                        == Some(rows.coordinator_id)
                    && recovery_row.6["coordination_owner_generation"].as_i64() == Some(0),
                "recovery task lost implementation owner context: {}",
                recovery_row.6
            );
            anyhow::ensure!(rows.recovery_count(failed_id).await? == 1, "recovery task count is not one");

            {
                let captured = events.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                anyhow::ensure!(captured.iter().filter(|event| event.event_type == patchbay_protocol::EVENT_COMMENT_CREATED).count() == 1, "comment-created event count mismatch");
                anyhow::ensure!(captured.iter().filter(|event| event.event_type == patchbay_protocol::EVENT_TASK_QUEUED).count() == 1, "task-queued event count mismatch");
            }

            let failed_again = rows.failed_task(failed_id).await?;
            let failed_race = failed_again.clone();
            let (race_svc, _race_bus, _race_events) = rows.service();
            let barrier = Arc::new(Barrier::new(3));
            let left_barrier = barrier.clone();
            let left = tokio::spawn(async move {
                left_barrier.wait().await;
                svc.handle_failed_tasks(&[failed_again]).await
            });
            let right_barrier = barrier.clone();
            let right = tokio::spawn(async move {
                right_barrier.wait().await;
                race_svc.handle_failed_tasks(&[failed_race]).await
            });
            barrier.wait().await;
            let _ = left.await?;
            let _ = right.await?;
            anyhow::ensure!(rows.recovery_count(failed_id).await? == 1, "replaying terminal failure created a duplicate recovery task");
            let comment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM comment WHERE issue_id = $1 AND type = 'progress_update' AND source_task_id = $2")
                .bind(rows.source_issue_id)
                .bind(failed_id)
                .fetch_one(&rows.pool)
                .await?;
            anyhow::ensure!(comment_count == 1, "replaying terminal failure created {comment_count} recovery comments");
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("final delegated failure contract failed");
        cleanup.expect("delegated recovery fixture cleanup failed");
    }

    #[tokio::test]
    async fn committed_recovery_comment_is_replayed_once_by_bounded_sweeper() {
        let rows = RecoveryRows::required().await.expect(
            "DATABASE_URL and migrated PostgreSQL are required for recovery outbox contract",
        );
        let result = async {
            let failed_id = rows
                .worker_task(
                    "failed",
                    "comment",
                    1,
                    1,
                    Some("provider_auth"),
                    Some("worker exited"),
                )
                .await?;
            let (svc, _bus, _events) = rows.service();
            let (target, created) = svc
                .ensure_delegated_failure_recovery_comment(failed_id)
                .await?;
            anyhow::ensure!(
                target.is_some() && created,
                "durable recovery comment was not created"
            );
            anyhow::ensure!(
                rows.recovery_count(failed_id).await? == 0,
                "comment creation unexpectedly enqueued a task"
            );
            let first_comment = rows.recovery_comment(failed_id).await?;

            let second_failed_id = rows
                .worker_task(
                    "failed",
                    "comment",
                    1,
                    1,
                    Some("provider_auth"),
                    Some("second worker exited"),
                )
                .await?;
            let (second_target, second_created) = svc
                .ensure_delegated_failure_recovery_comment(second_failed_id)
                .await?;
            anyhow::ensure!(
                second_target.is_some() && second_created,
                "second durable recovery comment was not created"
            );
            let second_comment = rows.recovery_comment(second_failed_id).await?;

            let zero = svc.recover_pending_delegated_failures(0).await?;
            anyhow::ensure!(
                zero == DelegatedFailureRecoverySweepResult::default(),
                "zero-sized replay mutated outbox: {zero:?}"
            );
            let replayed = svc.recover_pending_delegated_failures(1).await?;
            anyhow::ensure!(
                replayed.replayed == 1 && replayed.exhausted == 0,
                "outbox replay = {replayed:?}, want one replay"
            );
            anyhow::ensure!(
                rows.recovery_count(failed_id).await? == 1,
                "outbox replay did not create one coordinator task"
            );
            let first_recovery_task_id: Uuid = sqlx::query_scalar(
                "SELECT id FROM agent_task_queue WHERE trigger_evidence_kind = 'delegated_failure' AND trigger_evidence_ref_id = $1",
            )
            .bind(failed_id)
            .fetch_one(&rows.pool)
            .await?;
            anyhow::ensure!(
                rows.recovery_count(second_failed_id).await? == 0,
                "positive limit replayed more than one row"
            );
            let second = svc.recover_pending_delegated_failures(1).await?;
            anyhow::ensure!(
                second.replayed == 1 && second.exhausted == 0,
                "second bounded replay = {second:?}, want one replay"
            );
            anyhow::ensure!(
                rows.recovery_count(second_failed_id).await? == 0,
                "second bounded replay created a parallel coordinator task"
            );
            let merged = rows.failed_task(first_recovery_task_id).await?;
            anyhow::ensure!(
                merged.status == "queued",
                "second bounded replay changed the pending coordinator status to {}",
                merged.status
            );
            anyhow::ensure!(
                merged.trigger_comment_id == Some(second_comment.id)
                    && merged.coalesced_comment_ids.contains(&first_comment.id),
                "pending coordinator did not carry both recovery comments: trigger={:?} coalesced={:?}",
                merged.trigger_comment_id,
                merged.coalesced_comment_ids
            );
            let pending_coordinators: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM agent_task_queue WHERE issue_id = $1 AND agent_id = $2 AND status IN ('queued', 'dispatched')",
            )
            .bind(rows.source_issue_id)
            .bind(rows.coordinator_id)
            .fetch_one(&rows.pool)
            .await?;
            anyhow::ensure!(
                pending_coordinators == 1,
                "bounded replay left {pending_coordinators} parallel coordinator tasks"
            );
            let third = svc.recover_pending_delegated_failures(1).await?;
            anyhow::ensure!(
                third == DelegatedFailureRecoverySweepResult::default(),
                "third replay was not idempotent: {third:?}"
            );

            let pending = agent::list_pending_delegated_failure_recoveries(&rows.pool, 1).await?;
            anyhow::ensure!(
                pending.is_empty(),
                "covered recovery comment remained pending: {} rows",
                pending.len()
            );
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("delegated recovery outbox contract failed");
        cleanup.expect("delegated recovery outbox fixture cleanup failed");
    }

    #[tokio::test]
    async fn pending_coordinator_merges_multiple_recovery_signals_without_parallel_tasks() {
        let rows = RecoveryRows::required()
            .await
            .expect("DATABASE_URL and migrated PostgreSQL are required for pending merge contract");
        let result = async {
            let coordinator = rows.coordinator_task("queued").await?;
            let (svc, _bus, _events) = rows.service();
            let first_id = rows
                .worker_task("failed", "comment", 1, 1, Some("provider_auth"), Some("first worker exited"))
                .await?;
            let first = rows.failed_task(first_id).await?;
            svc.handle_failed_tasks(&[first]).await;
            let first_comment = rows.recovery_comment(first_id).await?;

            let second_id = rows
                .worker_task("failed", "comment", 1, 1, Some("provider_auth"), Some("second worker exited"))
                .await?;
            let second = rows.failed_task(second_id).await?;
            svc.handle_failed_tasks(&[second]).await;
            let second_comment = rows.recovery_comment(second_id).await?;

            let (trigger, coalesced, status): (Option<Uuid>, Vec<Uuid>, String) = sqlx::query_as(
                "SELECT trigger_comment_id, coalesced_comment_ids, status FROM agent_task_queue WHERE id = $1",
            )
            .bind(coordinator)
            .fetch_one(&rows.pool)
            .await?;
            anyhow::ensure!(trigger == Some(second_comment.id) && coalesced.contains(&first_comment.id), "pending coordinator lost recovery lineage: trigger={trigger:?} coalesced={coalesced:?}");
            anyhow::ensure!(status == "queued", "pending coordinator status changed to {status}");
            anyhow::ensure!(rows.recovery_count(first_id).await? == 0 && rows.recovery_count(second_id).await? == 0, "pending coordinator received standalone recovery tasks");
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("pending coordinator merge contract failed");
        cleanup.expect("pending merge fixture cleanup failed");
    }

    #[tokio::test]
    async fn dispatched_coordinator_plans_signal_and_completion_replays_successor() {
        let rows = RecoveryRows::required().await.expect(
            "DATABASE_URL and migrated PostgreSQL are required for dispatched recovery contract",
        );
        let result = async {
            let coordinator = rows.coordinator_task("dispatched").await?;
            let failed_id = rows
                .worker_task(
                    "failed",
                    "comment",
                    1,
                    1,
                    Some("provider_auth"),
                    Some("worker exited"),
                )
                .await?;
            let failed = rows.failed_task(failed_id).await?;
            let (svc, _bus, events) = rows.service_arc();
            svc.handle_failed_tasks(&[failed]).await;
            let recovery = rows.recovery_comment(failed_id).await?;
            let planned: Vec<Uuid> = sqlx::query_scalar(
                "SELECT coalesced_comment_ids FROM agent_task_queue WHERE id = $1",
            )
            .bind(coordinator)
            .fetch_one(&rows.pool)
            .await?;
            anyhow::ensure!(
                planned.contains(&recovery.id),
                "dispatched coordinator did not record planned recovery comment"
            );
            anyhow::ensure!(
                rows.recovery_count(failed_id).await? == 0,
                "dispatched coordinator received a premature successor"
            );

            let running = svc.start_task(coordinator).await?;
            anyhow::ensure!(
                running.status == "running",
                "coordinator start status = {}",
                running.status
            );
            let completed = svc
                .complete_task(
                    coordinator,
                    &serde_json::json!({"output": "coordinator completed recovery"}),
                    "",
                    "",
                    "",
                    false,
                    "",
                    "",
                )
                .await?;
            anyhow::ensure!(
                completed.status == "completed",
                "coordinator completion status = {}",
                completed.status
            );
            svc.dispatch_delegated_failure_recovery_comment(&recovery, Some(coordinator))
                .await?;
            anyhow::ensure!(
                rows.recovery_count(failed_id).await? == 1,
                "completion reconciliation did not create one successor"
            );
            let coordinator_id = coordinator.to_string();
            let workspace_id = rows.workspace_id.to_string();
            {
                let captured = events
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                anyhow::ensure!(
                    captured.iter().any(|event| event.event_type
                        == patchbay_protocol::EVENT_TASK_RUNNING
                        && event.task_id == coordinator_id),
                    "production start path did not publish task-running"
                );
                anyhow::ensure!(
                    captured.iter().any(|event| event.event_type
                        == patchbay_protocol::EVENT_TASK_COMPLETED
                        && event.task_id == coordinator_id),
                    "production completion path did not publish task-completed"
                );
                anyhow::ensure!(
                    captured.iter().any(|event| event.event_type
                        == patchbay_protocol::EVENT_AGENT_STATUS
                        && event.workspace_id == workspace_id),
                    "production terminal path did not reconcile agent status"
                );
            }
            let (agent_status, issue_status): (String, String) = sqlx::query_as(
                "SELECT a.status, i.status FROM agent a JOIN issue i ON i.id = $2 WHERE a.id = $1",
            )
            .bind(rows.coordinator_id)
            .bind(rows.source_issue_id)
            .fetch_one(&rows.pool)
            .await?;
            anyhow::ensure!(
                agent_status == "idle",
                "coordinator agent terminal status = {agent_status}"
            );
            anyhow::ensure!(
                issue_status == "in_progress",
                "terminal completion changed source issue status to {issue_status}"
            );
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("dispatched coordinator recovery contract failed");
        cleanup.expect("dispatched recovery fixture cleanup failed");
    }

    #[tokio::test]
    async fn running_coordinator_gets_independent_successor() {
        let rows = RecoveryRows::required().await.expect(
            "DATABASE_URL and migrated PostgreSQL are required for running recovery contract",
        );
        let result = async {
            let coordinator = rows.coordinator_task("running").await?;
            let failed_id = rows
                .worker_task(
                    "failed",
                    "comment",
                    1,
                    1,
                    Some("provider_auth"),
                    Some("worker exited"),
                )
                .await?;
            let failed = rows.failed_task(failed_id).await?;
            let (svc, _bus, _events) = rows.service();
            svc.handle_failed_tasks(&[failed]).await;
            anyhow::ensure!(
                rows.recovery_count(failed_id).await? == 1,
                "running coordinator did not get a successor"
            );
            let covered: bool = sqlx::query_scalar(
                "SELECT $2::uuid = ANY(coalesced_comment_ids) FROM agent_task_queue WHERE id = $1",
            )
            .bind(coordinator)
            .bind(rows.recovery_comment(failed_id).await?.id)
            .fetch_one(&rows.pool)
            .await?;
            anyhow::ensure!(
                !covered,
                "running coordinator incorrectly claimed the recovery comment"
            );
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("running coordinator recovery contract failed");
        cleanup.expect("running recovery fixture cleanup failed");
    }

    #[tokio::test]
    async fn user_cancel_acknowledges_recovery_signal_without_replay() {
        let rows = RecoveryRows::required()
            .await
            .expect("DATABASE_URL and migrated PostgreSQL are required for user-cancel contract");
        let result = async {
            let failed_id = rows
                .worker_task("failed", "comment", 1, 1, Some("provider_auth"), Some("worker exited"))
                .await?;
            let failed = rows.failed_task(failed_id).await?;
            let (svc, _bus, _events) = rows.service();
            anyhow::ensure!(svc.handle_failed_tasks(&[failed]).await == 0, "final delegated failure unexpectedly retried");
            let recovery = rows.recovery_comment(failed_id).await?;
            let recovery_task_id: Uuid = sqlx::query_scalar(
                "SELECT id FROM agent_task_queue WHERE trigger_evidence_kind = 'delegated_failure' AND trigger_evidence_ref_id = $1",
            )
            .bind(failed_id)
            .fetch_one(&rows.pool)
            .await?;

            let cancelled = svc.cancel_task_by_user(recovery_task_id).await?;
            anyhow::ensure!(cancelled.status == "cancelled", "user cancellation status = {}", cancelled.status);
            let acknowledged: bool = sqlx::query_scalar(
                "SELECT $2::uuid = ANY(delivered_comment_ids) FROM agent_task_queue WHERE id = $1",
            )
            .bind(recovery_task_id)
            .bind(recovery.id)
            .fetch_one(&rows.pool)
            .await?;
            anyhow::ensure!(acknowledged, "user cancellation did not acknowledge the recovery signal");

            let sweep = svc.recover_pending_delegated_failures(100).await?;
            anyhow::ensure!(sweep == DelegatedFailureRecoverySweepResult::default(), "user-cancelled recovery was replayed: {sweep:?}");
            anyhow::ensure!(rows.recovery_count(failed_id).await? == 1, "user cancellation created a second recovery task");
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("user-cancel recovery contract failed");
        cleanup.expect("user-cancel recovery fixture cleanup failed");
    }

    #[tokio::test]
    async fn manual_rerun_keeps_cancelled_recovery_signal_replayable() {
        let rows = RecoveryRows::required()
            .await
            .expect("DATABASE_URL and migrated PostgreSQL are required for manual-rerun contract");
        let result = async {
            let failed_id = rows
                .worker_task("failed", "comment", 1, 1, Some("provider_auth"), Some("worker exited"))
                .await?;
            let failed = rows.failed_task(failed_id).await?;
            let (svc, _bus, _events) = rows.service();
            svc.handle_failed_tasks(&[failed]).await;
            let recovery = rows.recovery_comment(failed_id).await?;
            let first_recovery_task_id: Uuid = sqlx::query_scalar(
                "SELECT id FROM agent_task_queue WHERE trigger_evidence_kind = 'delegated_failure' AND trigger_evidence_ref_id = $1",
            )
            .bind(failed_id)
            .fetch_one(&rows.pool)
            .await?;

            // Rerun the historical coordinator task as a human action. The
            // pending recovery row is cancelled by the rerun slot clear, but
            // must not receive a delivery receipt: the durable outbox remains
            // replayable if the new manual task did not carry the signal.
            let rerun = svc
                .rerun_issue(
                    rows.source_issue_id,
                    Some(rows.source_task_id),
                    None,
                    Some(rows.user_id),
                    None,
                )
                .await?;
            let (status, acknowledged): (String, bool) = sqlx::query_as(
                "SELECT status, $2::uuid = ANY(delivered_comment_ids) FROM agent_task_queue WHERE id = $1",
            )
            .bind(first_recovery_task_id)
            .bind(recovery.id)
            .fetch_one(&rows.pool)
            .await?;
            anyhow::ensure!(status == "cancelled" && !acknowledged, "manual rerun cancelled recovery = status {status}, acknowledged {acknowledged}");

            let replay = svc.recover_pending_delegated_failures(100).await?;
            anyhow::ensure!(replay.replayed == 1 && replay.exhausted == 0, "manual rerun recovery sweep = {replay:?}, want one replay");
            let rerun_trigger: Option<Uuid> = sqlx::query_scalar(
                "SELECT trigger_comment_id FROM agent_task_queue WHERE id = $1",
            )
            .bind(rerun.id)
            .fetch_one(&rows.pool)
            .await?;
            anyhow::ensure!(rerun_trigger == Some(recovery.id), "manual rerun trigger = {rerun_trigger:?}, want recovery comment");
            anyhow::ensure!(rows.recovery_count(failed_id).await? == 1, "manual rerun created a duplicate dedicated recovery task");
            anyhow::ensure!(svc.recover_pending_delegated_failures(100).await? == DelegatedFailureRecoverySweepResult::default(), "manual rerun replay was not idempotent");
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("manual-rerun recovery contract failed");
        cleanup.expect("manual-rerun recovery fixture cleanup failed");
    }

    #[tokio::test]
    async fn recovery_attempts_exhaust_once_and_never_self_recurse() {
        let rows = RecoveryRows::required()
            .await
            .expect("DATABASE_URL and migrated PostgreSQL are required for exhaustion contract");
        let result = async {
            let failed_id = rows
                .worker_task("failed", "comment", 1, 1, Some("provider_auth"), Some("worker exited"))
                .await?;
            let (svc, _bus, events) = rows.service();
            let (target, created) = svc.ensure_delegated_failure_recovery_comment(failed_id).await?;
            anyhow::ensure!(target.is_some() && created, "initial recovery signal missing");
            let first = svc.recover_pending_delegated_failures(100).await?;
            anyhow::ensure!(first.replayed == 1, "initial recovery replay = {first:?}");

            for attempt in 1..=DELEGATED_FAILURE_RECOVERY_MAX_TASK_ATTEMPTS {
                let current: Uuid = sqlx::query_scalar(
                    "SELECT id FROM agent_task_queue WHERE trigger_evidence_kind = 'delegated_failure' AND trigger_evidence_ref_id = $1 AND status = 'queued' ORDER BY created_at DESC, id DESC LIMIT 1",
                )
                .bind(failed_id)
                .fetch_one(&rows.pool)
                .await?;
                sqlx::query(
                    "UPDATE agent_task_queue SET status = 'failed', completed_at = now(), error = 'task expired in queue', failure_reason = 'queued_expired', delivered_comment_ids = '{}'::uuid[] WHERE id = $1",
                )
                .bind(current)
                .execute(&rows.pool)
                .await?;
                let replay = svc.recover_pending_delegated_failures(100).await?;
                if attempt < DELEGATED_FAILURE_RECOVERY_MAX_TASK_ATTEMPTS {
                    anyhow::ensure!(replay.replayed == 1 && replay.exhausted == 0, "recovery attempt {attempt} = {replay:?}, want replay");
                } else {
                    anyhow::ensure!(replay.replayed == 0 && replay.exhausted == 1, "final recovery attempt = {replay:?}, want exhaustion");
                }
            }
            anyhow::ensure!(rows.recovery_count(failed_id).await? == i64::from(DELEGATED_FAILURE_RECOVERY_MAX_TASK_ATTEMPTS), "recovery task count exceeded bound");
            let exhaustion: (i64, String) = sqlx::query_as(
                "SELECT count(*), COALESCE(max(content), '') FROM comment WHERE issue_id = $1 AND author_type = 'system' AND type = 'system' AND source_task_id = $2",
            )
            .bind(rows.source_issue_id)
            .bind(failed_id)
            .fetch_one(&rows.pool)
            .await?;
            anyhow::ensure!(exhaustion.0 == 1 && exhaustion.1.contains("stopped after 3"), "exhaustion comment = {exhaustion:?}");
            let inbox: (i64, String, String) = sqlx::query_as(
                "SELECT count(*), COALESCE(max(severity), ''), COALESCE(max(body), '') FROM inbox_item WHERE workspace_id = $1 AND recipient_id = $2 AND issue_id = $3 AND type = 'task_failed'",
            )
            .bind(rows.workspace_id)
            .bind(rows.user_id)
            .bind(rows.source_issue_id)
            .fetch_one(&rows.pool)
            .await?;
            anyhow::ensure!(inbox.0 == 1 && inbox.1 == "action_required" && inbox.2 == exhaustion.1, "exhaustion inbox = {inbox:?}");
            let after = svc.recover_pending_delegated_failures(100).await?;
            anyhow::ensure!(after == DelegatedFailureRecoverySweepResult::default(), "post-exhaustion sweep was not idempotent: {after:?}");

            let recursive_id = rows
                .worker_task("failed", attribution::evidence_delegated_failure().as_str(), 1, 1, Some("agent_error"), Some("recovery task failed"))
                .await?;
            let recursive = rows.recovery_comment(recursive_id).await;
            anyhow::ensure!(recursive.is_err(), "recovery task recursively created a recovery comment");
            let captured = events.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            anyhow::ensure!(captured.iter().any(|event| event.event_type == patchbay_protocol::EVENT_INBOX_NEW), "exhaustion inbox event missing");
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("delegated recovery exhaustion contract failed");
        cleanup.expect("exhaustion fixture cleanup failed");
    }

    #[tokio::test]
    async fn retry_pending_and_invalid_source_shapes_fail_closed() {
        let rows = RecoveryRows::required().await.expect(
            "DATABASE_URL and migrated PostgreSQL are required for recovery guard contract",
        );
        let result = async {
            let (svc, _bus, _events) = rows.service();
            let retry_failed = rows
                .worker_task(
                    "failed",
                    "comment",
                    1,
                    1,
                    Some("provider_auth"),
                    Some("worker exited"),
                )
                .await?;
            let retry_id = RecoveryRows::task(
                &rows.pool,
                rows.worker_id,
                rows.runtime_id,
                rows.worker_issue_id,
                "queued",
                1,
                1,
                None,
                None,
                None,
                Some(retry_failed),
                Some("retry"),
                Some(retry_failed),
            )
            .await?;
            // `has_retry_task_for_parent` intentionally follows
            // parent_task_id, not the delegated edge. Mark this row as the
            // ordinary retry child so the recovery guard proves the exact
            // production predicate.
            sqlx::query("UPDATE agent_task_queue SET parent_task_id = $2 WHERE id = $1")
                .bind(retry_id)
                .bind(retry_failed)
                .execute(&rows.pool)
                .await?;
            anyhow::ensure!(
                svc.ensure_delegated_failure_recovery_comment(retry_failed)
                    .await?
                    .0
                    .is_none(),
                "retry-pending failure woke coordinator"
            );
            sqlx::query("DELETE FROM agent_task_queue WHERE id = $1")
                .bind(retry_id)
                .execute(&rows.pool)
                .await?;

            let backlog_failed = rows
                .worker_task(
                    "failed",
                    "comment",
                    1,
                    1,
                    Some("provider_auth"),
                    Some("worker exited"),
                )
                .await?;
            sqlx::query("UPDATE issue SET status = 'backlog' WHERE id = $1")
                .bind(rows.source_issue_id)
                .execute(&rows.pool)
                .await?;
            anyhow::ensure!(
                svc.ensure_delegated_failure_recovery_comment(backlog_failed)
                    .await?
                    .0
                    .is_none(),
                "backlog source issue woke coordinator"
            );
            sqlx::query("UPDATE issue SET status = 'in_progress' WHERE id = $1")
                .bind(rows.source_issue_id)
                .execute(&rows.pool)
                .await?;

            let unbound_failed = rows
                .worker_task(
                    "failed",
                    "comment",
                    1,
                    1,
                    Some("provider_auth"),
                    Some("worker exited"),
                )
                .await?;
            sqlx::query("UPDATE agent SET runtime_id = NULL WHERE id = $1")
                .bind(rows.coordinator_id)
                .execute(&rows.pool)
                .await?;
            anyhow::ensure!(
                svc.ensure_delegated_failure_recovery_comment(unbound_failed)
                    .await?
                    .0
                    .is_none(),
                "unbound source agent woke coordinator"
            );
            sqlx::query("UPDATE agent SET runtime_id = $2 WHERE id = $1")
                .bind(rows.coordinator_id)
                .bind(rows.runtime_id)
                .execute(&rows.pool)
                .await?;

            let failed_automation = rows
                .worker_task(
                    "failed",
                    "comment",
                    1,
                    1,
                    Some("provider_auth"),
                    Some("worker exited"),
                )
                .await?;
            let automation_run_id = rows.automation_run().await?;
            sqlx::query("UPDATE agent_task_queue SET automation_run_id = $2 WHERE id = $1")
                .bind(failed_automation)
                .bind(automation_run_id)
                .execute(&rows.pool)
                .await?;
            anyhow::ensure!(
                svc.ensure_delegated_failure_recovery_comment(failed_automation)
                    .await?
                    .0
                    .is_none(),
                "automation failed task woke coordinator"
            );

            sqlx::query("UPDATE agent_task_queue SET automation_run_id = $2 WHERE id = $1")
                .bind(rows.source_task_id)
                .bind(automation_run_id)
                .execute(&rows.pool)
                .await?;
            let source_automation_failed = rows
                .worker_task(
                    "failed",
                    "comment",
                    1,
                    1,
                    Some("provider_auth"),
                    Some("worker exited"),
                )
                .await?;
            anyhow::ensure!(
                svc.ensure_delegated_failure_recovery_comment(source_automation_failed)
                    .await?
                    .0
                    .is_none(),
                "automation source task woke coordinator"
            );

            let recursive_id = rows
                .worker_task(
                    "failed",
                    attribution::evidence_delegated_failure().as_str(),
                    1,
                    1,
                    Some("agent_error"),
                    Some("recovery task failed"),
                )
                .await?;
            let recursive = svc
                .ensure_delegated_failure_recovery_comment(recursive_id)
                .await?;
            anyhow::ensure!(
                recursive.0.is_none(),
                "recovery task shape was not rejected"
            );
            Ok::<(), anyhow::Error>(())
        }
        .await;
        let cleanup = rows.cleanup().await;
        result.expect("delegated recovery guard contract failed");
        cleanup.expect("guard fixture cleanup failed");
    }
}
