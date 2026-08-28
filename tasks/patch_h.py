#!/usr/bin/env python3
"""⑧-a: extract prepare phase from enqueue_issue_task_with_comment_plan and
add the tx-scoped deferred-channel variant for IssueService::create."""

P = "/Users/alexjiang/Desktop/vibe/Patchbay/server-rs/crates/patchbay-service/src/task_service.rs"
s = open(P).read()

START = "    /// Shared implementation behind EnqueueTaskForIssue and the manual rerun"
END = "\n\n    /// Queued task for a mentioned agent on an issue (explicit agent ID)."

start_idx = s.find(START)
end_idx = s.find(END, start_idx)
assert start_idx != -1 and end_idx != -1, "anchors missing"

NEW = '''    /// Attribution/guard/metadata phase shared by every issue-task enqueue
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
        self.broadcast_task_event(patchbay_protocol::EVENT_TASK_QUEUED, &task, Default::default())
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
    pub(crate) async fn create_deferred_channel_issue_task_tx(
        &self,
        tx: &mut sqlx::PgConnection,
        issue: &Issue,
        fire_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<AgentTaskQueue, TaskServiceError> {
        let prep = self.prepare_issue_enqueue(issue, None, None, false).await?;

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
    }'''

s = s[:start_idx] + NEW + s[end_idx:]

# Struct definition placed just above the impl block containing these methods.
STRUCT_ANCHOR = "#[derive(Default)]\nstruct AnalyticsContextCache {"
assert STRUCT_ANCHOR in s, "struct anchor missing"
s = s.replace(
    STRUCT_ANCHOR,
    """/// Everything the two issue-task INSERT shapes need, resolved by
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
struct AnalyticsContextCache {""",
)

open(P, "w").write(s)
print("patch H ok")
