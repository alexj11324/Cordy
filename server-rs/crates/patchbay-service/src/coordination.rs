//! Durable coordinator handoffs for issue work.
//!
//! The task/event bus remains useful for realtime consumers, but it is not the
//! handoff source of truth. Completion and review-return producers write an
//! outbox row plus its pending assignment in their own business transaction.
//! This worker leases those rows from PostgreSQL, makes a deterministic owner
//! decision, and only then dispatches the next task.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use patchbay_db::dbid::new_v7;
use patchbay_db::models::{ActivityLog, AgentTaskQueue, Issue};
use patchbay_db::queries::{activity, team};
use patchbay_events::{Bus, Event};
use serde_json::{json, Value};
use sqlx::Row;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::issue_status;
use crate::task_notify::{issue_to_map_with_category, rfc3339};
use crate::task_service::{
    TaskService, TaskServiceError, COORDINATION_ASSIGNMENT_ID_CONTEXT_KEY,
    COORDINATION_ISSUE_REVISION_CONTEXT_KEY, COORDINATION_OWNER_GENERATION_CONTEXT_KEY,
    COORDINATION_OWNER_ID_CONTEXT_KEY, COORDINATION_OWNER_TYPE_CONTEXT_KEY,
};

pub const EVENT_TASK_COMPLETED: &str = "task_completed";
pub const EVENT_REVIEW_RETURNED: &str = "review_returned";
pub const ASSIGNMENT_REVIEWER: &str = "reviewer";
pub const ASSIGNMENT_EXECUTOR: &str = "executor";

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const NO_OWNER_RETRY: Duration = Duration::from_secs(30);
const ERROR_RETRY: Duration = Duration::from_secs(10);
const PUBLICATION_ACK_POLL: Duration = Duration::from_millis(50);
const PUBLICATION_ACK_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
struct CoordinationEvent {
    id: Uuid,
    event_key: String,
    workspace_id: Uuid,
    issue_id: Uuid,
    source_task_id: Option<Uuid>,
    event_type: String,
    payload: Value,
    lease_owner: String,
}

#[derive(Debug, Clone)]
struct Assignment {
    id: Uuid,
    role: String,
    status: String,
    owner_type: Option<String>,
    owner_id: Option<Uuid>,
    dispatched_task_id: Option<Uuid>,
    decision: Value,
}

#[derive(Debug, Clone)]
struct ReviewerCandidate {
    id: Uuid,
    name: String,
}

#[derive(Debug, Clone)]
struct DispatchPlan {
    event_id: Uuid,
    assignment_id: Uuid,
    issue: Issue,
    owner_type: String,
    owner_id: Uuid,
    expected_owner_generation: Option<i64>,
    expected_issue_category: String,
    publish_issue_update: bool,
    publish_reviewer_update: bool,
    previous_status: String,
    previous_assignee_type: Option<String>,
    previous_assignee_id: Option<Uuid>,
    previous_reviewer_type: Option<String>,
    previous_reviewer_id: Option<Uuid>,
    handoff_note: Option<String>,
    assignment_activity: Option<ActivityLog>,
}

/// The coordinator owns no in-memory queue. `Notify` only reduces latency;
/// polling and PostgreSQL leases recover rows after a missed signal or restart.
pub struct CoordinatorService {
    pool: sqlx::PgPool,
    tasks: Arc<TaskService>,
    bus: Arc<Bus>,
    notify: Arc<Notify>,
    worker_id: String,
}

// SQLx 0.8 implements `Executor` for `&mut PgConnection`, not for
// `&mut Transaction`; the explicit deref is required for query calls below.
#[allow(clippy::explicit_auto_deref)]
impl CoordinatorService {
    pub fn new(pool: sqlx::PgPool, tasks: Arc<TaskService>, bus: Arc<Bus>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            tasks,
            bus,
            notify: Arc::new(Notify::new()),
            worker_id: format!("coordinator-{}", new_v7()),
        })
    }

    /// Wake the worker as a best-effort latency hint. The database remains the
    /// authority, so callers do not need to make this call for correctness.
    pub fn notify(&self) {
        self.notify.notify_one();
    }

    pub fn start(self: &Arc<Self>, cancel: CancellationToken) -> CoordinatorRuntime {
        let service = self.clone();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { service.run_loop(task_cancel).await });
        CoordinatorRuntime {
            cancel,
            task: Some(task),
        }
    }

    async fn run_loop(self: Arc<Self>, cancel: CancellationToken) {
        loop {
            let processed = tokio::select! {
                _ = cancel.cancelled() => return,
                result = self.process_next() => result,
            };
            match processed {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => tracing::error!(%error, "coordinator failed to process outbox"),
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                () = self.notify.notified() => {},
                () = tokio::time::sleep(POLL_INTERVAL) => {},
            }
        }
    }

    pub async fn process_next(&self) -> anyhow::Result<bool> {
        let Some(event) = self.claim_next().await? else {
            return Ok(false);
        };
        if let Err(error) = self.process_claimed(event.clone()).await {
            tracing::error!(
                event_id = %event.id,
                event_key = %event.event_key,
                error = %error,
                "coordinator handoff failed; returning it to the outbox"
            );
            if let Err(release_error) = self
                .defer_claimed(&event, &format!("coordinator error: {error}"), ERROR_RETRY)
                .await
            {
                tracing::error!(
                    event_id = %event.id,
                    error = %release_error,
                    "coordinator could not release a failed lease"
                );
            }
        }
        Ok(true)
    }

    async fn claim_next(&self) -> anyhow::Result<Option<CoordinationEvent>> {
        let row = sqlx::query(
            r#"WITH candidate AS (
    SELECT id
    FROM agent_coordination_outbox
    WHERE status IN ('pending', 'processing')
      AND available_at <= now()
      AND (lease_expires_at IS NULL OR lease_expires_at <= now())
    ORDER BY available_at, created_at, id
    FOR UPDATE SKIP LOCKED
    LIMIT 1
)
UPDATE agent_coordination_outbox AS event
SET status = 'processing',
    attempt = event.attempt + 1,
    lease_owner = $1,
    lease_expires_at = now() + interval '5 minutes',
    updated_at = now()
FROM candidate
WHERE event.id = candidate.id
RETURNING event.id, event.event_key, event.workspace_id, event.issue_id,
          event.source_task_id, event.event_type, event.payload, event.lease_owner"#,
        )
        .bind(&self.worker_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(CoordinationEvent {
            id: row.try_get(0)?,
            event_key: row.try_get(1)?,
            workspace_id: row.try_get(2)?,
            issue_id: row.try_get(3)?,
            source_task_id: row.try_get(4)?,
            event_type: row.try_get(5)?,
            payload: row.try_get(6)?,
            lease_owner: row.try_get(7)?,
        }))
    }

    async fn process_claimed(&self, event: CoordinationEvent) -> anyhow::Result<()> {
        let Some(mut plan) = self.prepare_dispatch(&event).await? else {
            return Ok(());
        };

        // The prepare transaction is intentionally short so it never holds a
        // database lock across task creation or event fanout. Re-check the
        // issue after the deferred task exists and immediately before any
        // handoff publication; a newer user update must not receive stale
        // reviewer side effects.
        let task_id = match self.dispatch(&plan).await {
            Ok(task_id) => task_id,
            Err(error) => {
                if let Some(task_id) = self
                    .find_active_task(
                        &plan.issue,
                        &plan.owner_type,
                        plan.owner_id,
                        plan.assignment_id,
                    )
                    .await?
                {
                    tracing::warn!(
                        event_id = %plan.event_id,
                        assignment_id = %plan.assignment_id,
                        task_id = %task_id,
                        error = %error,
                        "coordinator enqueue reported an error but an active task exists"
                    );
                    task_id
                } else {
                    return self
                        .defer_claimed(&event, &format!("dispatch failed: {error}"), ERROR_RETRY)
                        .await;
                }
            }
        };

        let issue_prefix = if plan.publish_issue_update || plan.publish_reviewer_update {
            patchbay_db::queries::workspace::get_workspace(&self.pool, plan.issue.workspace_id)
                .await
                .ok()
                .flatten()
                .map(|workspace| workspace.issue_prefix)
                .unwrap_or_default()
        } else {
            String::new()
        };

        let Some(current_issue) = self
            .revalidate_before_publication(&event, &plan, task_id)
            .await?
        else {
            return Ok(());
        };
        // Carry unrelated edits observed by the revalidation into the event
        // snapshot, so a valid handoff cannot publish an older title or
        // description after the user's update has already committed.
        plan.issue = current_issue;

        if plan.publish_issue_update {
            self.publish_review_handoff(&plan, &issue_prefix);
        }
        if plan.publish_reviewer_update {
            self.publish_reviewer_update(&plan, &issue_prefix);
        }
        if plan.publish_issue_update || plan.publish_reviewer_update {
            self.wait_for_issue_update_publication(&event, &plan)
                .await?;
        }
        if let Some(activity) = &plan.assignment_activity {
            self.publish_assignment_activity(activity);
            self.mark_assignment_activity_published(&event, &plan)
                .await?;
        }

        let promoted = self.mark_dispatched(&event, &plan, task_id).await?;
        if promoted {
            self.tasks.publish_task_queued(task_id).await;
        }
        Ok(())
    }

    async fn prepare_dispatch(
        &self,
        event: &CoordinationEvent,
    ) -> anyhow::Result<Option<DispatchPlan>> {
        let mut tx = self.pool.begin().await?;
        let assignment = sqlx::query(
            r#"SELECT id, role, status, owner_type, owner_id, dispatched_task_id, decision
FROM agent_coordination_assignment
WHERE event_id = $1
ORDER BY created_at, id
LIMIT 1
FOR UPDATE"#,
        )
        .bind(event.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| anyhow::anyhow!("coordination event has no assignment"))?;
        let assignment = Assignment {
            id: assignment.try_get(0)?,
            role: assignment.try_get(1)?,
            status: assignment.try_get(2)?,
            owner_type: assignment.try_get(3)?,
            owner_id: assignment.try_get(4)?,
            dispatched_task_id: assignment.try_get(5)?,
            decision: assignment.try_get(6)?,
        };

        if assignment.dispatched_task_id.is_some()
            || matches!(assignment.status.as_str(), "completed" | "blocked")
        {
            complete_claimed_tx(
                &mut *tx,
                event,
                &assignment,
                assignment.status.as_str(),
                assignment.dispatched_task_id,
                json!({"outcome": "already_finalized"}),
                None,
            )
            .await?;
            tx.commit().await?;
            return Ok(None);
        }

        let issue = sqlx::query_as::<_, Issue>(
            "SELECT * FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
        )
        .bind(event.issue_id)
        .bind(event.workspace_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(issue) = issue else {
            complete_claimed_tx(
                &mut *tx,
                event,
                &assignment,
                "blocked",
                None,
                json!({"outcome": "blocked", "reason": "issue_not_found"}),
                Some("issue not found"),
            )
            .await?;
            tx.commit().await?;
            return Ok(None);
        };
        let current_owner_generation: i64 = sqlx::query_scalar(
            "SELECT assignee_generation FROM issue WHERE id = $1 AND workspace_id = $2",
        )
        .bind(issue.id)
        .bind(issue.workspace_id)
        .fetch_one(&mut *tx)
        .await?;

        if event.event_type == EVENT_TASK_COMPLETED
            && event.payload.get("source_role").and_then(Value::as_str) == Some(ASSIGNMENT_REVIEWER)
        {
            complete_claimed_tx(
                &mut *tx,
                event,
                &assignment,
                "completed",
                None,
                json!({"outcome": "review_task_completed_without_transition"}),
                None,
            )
            .await?;
            tx.commit().await?;
            return Ok(None);
        }

        let category = issue_status::effective(&mut *tx, issue.workspace_id, &issue.status).await;
        let is_task_completion = event.event_type == EVENT_TASK_COMPLETED;
        let is_review_return = event.event_type == EVENT_REVIEW_RETURNED;
        let is_persisted_reviewer_recovery = is_task_completion
            && category == issue_status::IN_REVIEW
            && assignment.role == ASSIGNMENT_REVIEWER
            && assignment.owner_type.as_deref() == Some("agent")
            && assignment.dispatched_task_id.is_none();

        if !is_task_completion && !is_review_return {
            complete_claimed_tx(
                &mut *tx,
                event,
                &assignment,
                "blocked",
                None,
                json!({"outcome": "blocked", "reason": "unknown_event_type"}),
                Some("unknown coordinator event type"),
            )
            .await?;
            tx.commit().await?;
            return Ok(None);
        }

        if is_review_return {
            if let Some(captured_revision) =
                event.payload.get("issue_revision").and_then(Value::as_i64)
            {
                let superseded = sqlx::query_scalar::<_, bool>(
                    r#"SELECT EXISTS (
    SELECT 1
    FROM agent_coordination_outbox newer
    WHERE newer.issue_id = $1
      AND newer.event_type = $2
      AND newer.id <> $3
      AND CASE
          WHEN newer.payload->>'issue_revision' ~ '^[0-9]+$'
          THEN (newer.payload->>'issue_revision')::bigint
      END > $4
)"#,
                )
                .bind(issue.id)
                .bind(EVENT_REVIEW_RETURNED)
                .bind(event.id)
                .bind(captured_revision)
                .fetch_one(&mut *tx)
                .await?;
                if superseded {
                    complete_claimed_tx(
                        &mut *tx,
                        event,
                        &assignment,
                        "completed",
                        None,
                        json!({
                            "outcome": "stale_review_return",
                            "reason": "a newer review return exists",
                            "captured_issue_revision": captured_revision,
                        }),
                        Some("a newer review return exists"),
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(None);
                }
            }
        }

        // A reviewer assignment can remain pending when enqueue fails after
        // the implementation completion transaction committed. If review
        // then returns the issue to in progress before that event is retried,
        // the old completion is stale: do not select another reviewer or move
        // the issue back into review.
        if is_task_completion
            && assignment.role == ASSIGNMENT_REVIEWER
            && assignment.status == "assigned"
            && assignment.dispatched_task_id.is_none()
            && category != issue_status::IN_REVIEW
        {
            complete_claimed_tx(
                &mut *tx,
                event,
                &assignment,
                "completed",
                None,
                json!({
                    "outcome": "stale_completion",
                    "reason": "reviewer handoff became stale after issue left review",
                }),
                Some("reviewer handoff became stale after issue left review"),
            )
            .await?;
            tx.commit().await?;
            return Ok(None);
        }

        // The task captures the implementation owner when it is enqueued.
        // A still-running implementation task must not promote a newer owner
        // after an explicit reassignment (A -> B -> A); the current owner and
        // its monotonic generation are authoritative for the issue. Reviewer
        // tasks carry their selected reviewer in the same context, but their
        // completion is only an assignment acknowledgement and must not be
        // checked against the implementation owner.
        if is_task_completion
            && !is_persisted_reviewer_recovery
            && event.payload.get("source_role").and_then(Value::as_str) != Some(ASSIGNMENT_REVIEWER)
        {
            let captured_owner_type = event
                .payload
                .get("implementation_owner_type")
                .and_then(Value::as_str);
            let captured_owner_id = event
                .payload
                .get("implementation_owner_id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok());
            let captured_owner_generation = event
                .payload
                .get("implementation_owner_generation")
                .and_then(Value::as_i64);
            if let (Some(captured_owner_type), Some(captured_owner_id)) =
                (captured_owner_type, captured_owner_id)
            {
                if issue.assignee_type.as_deref() != Some(captured_owner_type)
                    || issue.assignee_id != Some(captured_owner_id)
                    || captured_owner_generation
                        .is_some_and(|captured| captured != current_owner_generation)
                {
                    complete_claimed_tx(
                        &mut *tx,
                        event,
                        &assignment,
                        "completed",
                        None,
                        json!({
                            "outcome": "stale_completion",
                            "reason": "implementation owner changed after task was assigned",
                            "captured_owner_type": captured_owner_type,
                            "captured_owner_id": captured_owner_id,
                            "captured_owner_generation": captured_owner_generation,
                            "current_owner_type": issue.assignee_type,
                            "current_owner_id": issue.assignee_id,
                            "current_owner_generation": current_owner_generation,
                        }),
                        Some("implementation owner changed after task was assigned"),
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(None);
                }
            }
            if category == issue_status::IN_PROGRESS {
                if let Some(source_task_id) = event.source_task_id {
                    let has_newer_implementation = sqlx::query_scalar::<_, bool>(
                        r#"SELECT EXISTS (
    SELECT 1
    FROM agent_task_queue newer
    JOIN agent_task_queue source ON source.id = $2
    WHERE newer.issue_id = $1
      AND newer.id <> source.id
      AND (
        newer.created_at > source.created_at
        OR (newer.created_at = source.created_at AND newer.id > source.id)
      )
      AND newer.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
      AND COALESCE(newer.context->>'side_chat_parent_task_id', '') = ''
      AND newer.context ? 'coordination_owner_type'
      AND newer.context ? 'coordination_owner_id'
)"#,
                    )
                    .bind(issue.id)
                    .bind(source_task_id)
                    .fetch_one(&mut *tx)
                    .await?;
                    if has_newer_implementation {
                        complete_claimed_tx(
                            &mut *tx,
                            event,
                            &assignment,
                            "completed",
                            None,
                            json!({
                                "outcome": "stale_completion",
                                "reason": "newer implementation task exists",
                                "source_task_id": source_task_id,
                            }),
                            Some("newer implementation task exists"),
                        )
                        .await?;
                        tx.commit().await?;
                        return Ok(None);
                    }
                }
            }
        }

        // A crash after the issue/status transaction but before dispatch leaves
        // the assignment owner recorded and the issue in review. Reuse that
        // decision instead of selecting a second reviewer.
        if is_task_completion
            && category == issue_status::IN_REVIEW
            && assignment.owner_type.as_deref() == Some("agent")
        {
            if let Some(owner_id) = assignment.owner_id {
                if issue.reviewer_type.as_deref() != Some("agent")
                    || issue.reviewer_id != Some(owner_id)
                {
                    complete_claimed_tx(
                        &mut *tx,
                        event,
                        &assignment,
                        "completed",
                        None,
                        json!({"outcome": "stale_assignment"}),
                        Some("issue changed after reviewer selection"),
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(None);
                }
                let team_id = if issue.assignee_type.as_deref() == Some("team") {
                    issue.assignee_id
                } else {
                    None
                };
                let source_agent_id = event
                    .payload
                    .get("agent_id")
                    .and_then(Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok());
                let reviewer_before_recovery_type = issue.reviewer_type.clone();
                let reviewer_before_recovery_id = issue.reviewer_id;
                let persisted_assignment_activity = if assignment
                    .decision
                    .get("assignment_activity_published")
                    .and_then(Value::as_bool)
                    != Some(true)
                {
                    find_assignment_activity(&mut *tx, issue.id, event.id, assignment.id).await?
                } else {
                    None
                };
                let (dispatch_owner_id, issue, assignment_activity, reviewer_replaced) =
                    if reviewer_is_dispatchable(
                        &mut *tx,
                        issue.workspace_id,
                        team_id,
                        source_agent_id,
                        owner_id,
                        assignment.id,
                    )
                    .await?
                    {
                        (owner_id, issue, persisted_assignment_activity, false)
                    } else if let Some(candidate) =
                        select_reviewer(&mut *tx, issue.workspace_id, team_id, source_agent_id)
                            .await?
                    {
                        cancel_unpromoted_reviewer_task(
                            &mut *tx,
                            issue.id,
                            assignment.id,
                            owner_id,
                        )
                        .await?;
                        let updated = sqlx::query_as::<_, Issue>(
                            r#"UPDATE issue SET
    reviewer_type = 'agent',
    reviewer_id = $3,
    revision = revision + 1,
    updated_at = now(),
    last_activity_at = GREATEST(COALESCE(last_activity_at, updated_at), now())
WHERE id = $1 AND workspace_id = $2
RETURNING *"#,
                        )
                        .bind(issue.id)
                        .bind(issue.workspace_id)
                        .bind(candidate.id)
                        .fetch_one(&mut *tx)
                        .await?;
                        let decision = json!({
                            "policy": "reviewer_recovery_replacement",
                            "role": ASSIGNMENT_REVIEWER,
                            "review_publication": "reviewer_replacement",
                            "issue_update_published": false,
                            "assignment_activity_published": false,
                            "candidate_agent_id": candidate.id,
                            "candidate_agent_name": candidate.name,
                            "previous_status": issue.status,
                            "previous_assignee_type": issue.assignee_type,
                            "previous_assignee_id": issue.assignee_id,
                            "previous_reviewer_type": issue.reviewer_type,
                            "previous_reviewer_id": issue.reviewer_id,
                            "source_agent_id": source_agent_id,
                        });
                        record_assignment_decision(
                            &mut *tx,
                            event,
                            &assignment,
                            ASSIGNMENT_REVIEWER,
                            "agent",
                            candidate.id,
                            decision,
                        )
                        .await?;
                        let audit = json!({
                            "event_id": event.id,
                            "assignment_id": assignment.id,
                            "source_task_id": event.source_task_id,
                            "role": ASSIGNMENT_REVIEWER,
                            "owner_type": "agent",
                            "owner_id": candidate.id,
                            "previous_reviewer_id": owner_id,
                            "reason": "persisted reviewer unavailable; selected replacement",
                        });
                        let activity = activity::create_activity(
                            &mut *tx,
                            issue.workspace_id,
                            issue.id,
                            Some("system"),
                            None,
                            "coordinator_assignment",
                            &audit,
                            new_v7(),
                        )
                        .await?;
                        (candidate.id, updated, activity, true)
                    } else {
                        defer_claimed_tx(
                            &mut *tx,
                            event,
                            &assignment,
                            "persisted reviewer unavailable and no replacement reviewer",
                            NO_OWNER_RETRY,
                            Some(("agent", owner_id)),
                        )
                        .await?;
                        tx.commit().await?;
                        return Ok(None);
                    };
                let (
                    previous_status,
                    previous_assignee_type,
                    previous_assignee_id,
                    previous_reviewer_type,
                    previous_reviewer_id,
                    publish_issue_update,
                    publish_reviewer_update,
                ) = if reviewer_replaced {
                    (
                        issue.status.clone(),
                        issue.assignee_type.clone(),
                        issue.assignee_id,
                        reviewer_before_recovery_type,
                        reviewer_before_recovery_id,
                        false,
                        true,
                    )
                } else {
                    let publication_kind = assignment
                        .decision
                        .get("review_publication")
                        .and_then(Value::as_str);
                    let issue_update_published = assignment
                        .decision
                        .get("issue_update_published")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let previous_status = assignment
                        .decision
                        .get("previous_status")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| issue.status.clone());
                    let previous_assignee_type = assignment
                        .decision
                        .get("previous_assignee_type")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| issue.assignee_type.clone());
                    let previous_assignee_id = assignment
                        .decision
                        .get("previous_assignee_id")
                        .and_then(Value::as_str)
                        .and_then(|value| Uuid::parse_str(value).ok())
                        .or(issue.assignee_id);
                    let previous_reviewer_type =
                        if assignment.decision.get("previous_reviewer_type").is_some() {
                            assignment
                                .decision
                                .get("previous_reviewer_type")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        } else {
                            None
                        };
                    let previous_reviewer_id =
                        if assignment.decision.get("previous_reviewer_id").is_some() {
                            assignment
                                .decision
                                .get("previous_reviewer_id")
                                .and_then(Value::as_str)
                                .and_then(|value| Uuid::parse_str(value).ok())
                        } else {
                            None
                        };
                    (
                        previous_status,
                        previous_assignee_type,
                        previous_assignee_id,
                        previous_reviewer_type,
                        previous_reviewer_id,
                        !issue_update_published && publication_kind != Some("reviewer_replacement"),
                        !issue_update_published && publication_kind == Some("reviewer_replacement"),
                    )
                };
                let plan = DispatchPlan {
                    event_id: event.id,
                    assignment_id: assignment.id,
                    issue,
                    owner_type: "agent".to_string(),
                    owner_id: dispatch_owner_id,
                    // Recovery deliberately follows the persisted reviewer
                    // decision even if the implementation owner changed
                    // while the original reviewer task was unavailable.
                    expected_owner_generation: None,
                    expected_issue_category: issue_status::IN_REVIEW.to_string(),
                    publish_issue_update,
                    publish_reviewer_update,
                    previous_status,
                    previous_assignee_type,
                    previous_assignee_id,
                    previous_reviewer_type,
                    previous_reviewer_id,
                    handoff_note: None,
                    assignment_activity,
                };
                tx.commit().await?;
                return Ok(Some(plan));
            }
        }

        if is_task_completion && category != issue_status::IN_PROGRESS {
            complete_claimed_tx(
                &mut *tx,
                event,
                &assignment,
                "completed",
                None,
                json!({"outcome": "ignored", "reason": "issue_not_in_progress"}),
                None,
            )
            .await?;
            tx.commit().await?;
            return Ok(None);
        }
        if is_review_return && category != issue_status::IN_PROGRESS {
            complete_claimed_tx(
                &mut *tx,
                event,
                &assignment,
                "completed",
                None,
                json!({"outcome": "ignored", "reason": "issue_not_in_progress"}),
                None,
            )
            .await?;
            tx.commit().await?;
            return Ok(None);
        }

        let (owner_type, owner_id, publish_issue_update, assignment_activity) =
            if is_task_completion {
                let Some(previous_owner_type) = issue.assignee_type.clone() else {
                    complete_claimed_tx(
                        &mut *tx,
                        event,
                        &assignment,
                        "blocked",
                        None,
                        json!({"outcome": "blocked", "reason": "implementation_owner_missing"}),
                        Some("implementation issue has no owner"),
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(None);
                };
                let Some(previous_owner_id) = issue.assignee_id else {
                    complete_claimed_tx(
                        &mut *tx,
                        event,
                        &assignment,
                        "blocked",
                        None,
                        json!({"outcome": "blocked", "reason": "implementation_owner_missing"}),
                        Some("implementation issue has no owner"),
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(None);
                };
                if !matches!(previous_owner_type.as_str(), "agent" | "team") {
                    complete_claimed_tx(
                        &mut *tx,
                        event,
                        &assignment,
                        "blocked",
                        None,
                        json!({"outcome": "blocked", "reason": "implementation_owner_not_agent"}),
                        Some("implementation owner is not an agent or team"),
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(None);
                }
                let team_id = (previous_owner_type == "team").then_some(previous_owner_id);
                let source_agent_id = event
                    .payload
                    .get("agent_id")
                    .and_then(Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok());
                let Some(candidate) =
                    select_reviewer(&mut *tx, issue.workspace_id, team_id, source_agent_id).await?
                else {
                    defer_claimed_tx(
                        &mut *tx,
                        event,
                        &assignment,
                        "no reviewer with role=reviewer and a bound runtime",
                        NO_OWNER_RETRY,
                        None,
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(None);
                };
                let updated = sqlx::query_as::<_, Issue>(
                    r#"UPDATE issue SET
    status = 'in_review',
    reviewer_type = 'agent',
    reviewer_id = $3,
    revision = revision + 1,
    updated_at = now(),
    last_activity_at = GREATEST(COALESCE(last_activity_at, updated_at), now())
WHERE id = $1 AND workspace_id = $2
RETURNING *"#,
                )
                .bind(issue.id)
                .bind(issue.workspace_id)
                .bind(candidate.id)
                .fetch_one(&mut *tx)
                .await?;
                let decision = json!({
                    "policy": if team_id.is_some() { "team_reviewer_role" } else { "workspace_reviewer_role" },
                    "role": ASSIGNMENT_REVIEWER,
                    "review_publication": "review_handoff",
                    "issue_update_published": false,
                    "assignment_activity_published": false,
                    "candidate_agent_id": candidate.id,
                    "candidate_agent_name": candidate.name,
                    "previous_status": issue.status,
                    "previous_assignee_type": issue.assignee_type,
                    "previous_assignee_id": issue.assignee_id,
                    "previous_reviewer_type": issue.reviewer_type,
                    "previous_reviewer_id": issue.reviewer_id,
                    "source_agent_id": source_agent_id,
                });
                record_assignment_decision(
                    &mut *tx,
                    event,
                    &assignment,
                    ASSIGNMENT_REVIEWER,
                    "agent",
                    candidate.id,
                    decision,
                )
                .await?;
                let audit = json!({
                    "event_id": event.id,
                    "assignment_id": assignment.id,
                    "source_task_id": event.source_task_id,
                    "role": ASSIGNMENT_REVIEWER,
                    "owner_type": "agent",
                    "owner_id": candidate.id,
                    "reason": "implementation task completed",
                });
                let assignment_activity = activity::create_activity(
                    &mut *tx,
                    issue.workspace_id,
                    issue.id,
                    Some("system"),
                    None,
                    "coordinator_assignment",
                    &audit,
                    new_v7(),
                )
                .await?;
                (
                    "agent".to_string(),
                    candidate.id,
                    (updated, issue),
                    assignment_activity,
                )
            } else {
                let target_type = event
                    .payload
                    .get("owner_type")
                    .and_then(Value::as_str)
                    .or(assignment.owner_type.as_deref())
                    .unwrap_or_default()
                    .to_string();
                let target_id = event
                    .payload
                    .get("owner_id")
                    .and_then(Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .or(assignment.owner_id);
                let Some(target_id) = target_id else {
                    complete_claimed_tx(
                        &mut *tx,
                        event,
                        &assignment,
                        "blocked",
                        None,
                        json!({"outcome": "blocked", "reason": "executor_owner_missing"}),
                        Some("review return has no executor owner"),
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(None);
                };
                if issue.assignee_type.as_deref() != Some(target_type.as_str())
                    || issue.assignee_id != Some(target_id)
                    || event
                        .payload
                        .get("owner_generation")
                        .and_then(Value::as_i64)
                        .is_some_and(|captured| captured != current_owner_generation)
                {
                    let captured_owner_generation = event
                        .payload
                        .get("owner_generation")
                        .and_then(Value::as_i64);
                    complete_claimed_tx(
                        &mut *tx,
                        event,
                        &assignment,
                        "completed",
                        None,
                        json!({
                            "outcome": "stale_assignment",
                            "reason": "issue owner changed after review returned work",
                            "captured_owner_generation": captured_owner_generation,
                            "current_owner_generation": current_owner_generation,
                        }),
                        Some("issue owner changed after review returned work"),
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(None);
                }
                if !matches!(target_type.as_str(), "agent" | "team") {
                    complete_claimed_tx(
                        &mut *tx,
                        event,
                        &assignment,
                        "blocked",
                        None,
                        json!({"outcome": "blocked", "reason": "executor_not_agent_or_team"}),
                        Some("review return owner is not an agent or team"),
                    )
                    .await?;
                    tx.commit().await?;
                    return Ok(None);
                }
                match validate_target(&mut *tx, issue.workspace_id, &target_type, target_id).await?
                {
                    TargetVerdict::Ready => {}
                    TargetVerdict::Waitable(reason) => {
                        defer_claimed_tx(
                            &mut *tx,
                            event,
                            &assignment,
                            &reason,
                            NO_OWNER_RETRY,
                            Some((target_type.as_str(), target_id)),
                        )
                        .await?;
                        tx.commit().await?;
                        return Ok(None);
                    }
                    TargetVerdict::Blocked(reason) => {
                        complete_claimed_tx(
                            &mut *tx,
                            event,
                            &assignment,
                            "blocked",
                            None,
                            json!({"outcome": "blocked", "reason": reason}),
                            Some(&reason),
                        )
                        .await?;
                        tx.commit().await?;
                        return Ok(None);
                    }
                }
                let assignment_is_reused = assignment.status == "assigned"
                    && assignment.owner_type.as_deref() == Some(target_type.as_str())
                    && assignment.owner_id == Some(target_id);
                let assignment_activity = if assignment_is_reused {
                    if assignment
                        .decision
                        .get("assignment_activity_published")
                        .and_then(Value::as_bool)
                        != Some(true)
                    {
                        find_assignment_activity(&mut *tx, issue.id, event.id, assignment.id)
                            .await?
                    } else {
                        None
                    }
                } else {
                    let decision = json!({
                        "policy": "review_return_original_owner",
                        "role": ASSIGNMENT_EXECUTOR,
                        "owner_type": target_type,
                        "owner_id": target_id,
                        "assignment_activity_published": false,
                    });
                    record_assignment_decision(
                        &mut *tx,
                        event,
                        &assignment,
                        ASSIGNMENT_EXECUTOR,
                        &target_type,
                        target_id,
                        decision,
                    )
                    .await?;
                    let audit = json!({
                        "event_id": event.id,
                        "assignment_id": assignment.id,
                        "role": ASSIGNMENT_EXECUTOR,
                        "owner_type": target_type,
                        "owner_id": target_id,
                        "reason": "review returned work to implementation",
                    });
                    activity::create_activity(
                        &mut *tx,
                        issue.workspace_id,
                        issue.id,
                        Some("system"),
                        None,
                        "coordinator_assignment",
                        &audit,
                        new_v7(),
                    )
                    .await?
                };
                (
                    target_type,
                    target_id,
                    (issue.clone(), issue),
                    assignment_activity,
                )
            };

        let handoff_note = if is_review_return {
            event
                .payload
                .get("handoff_note")
                .and_then(Value::as_str)
                .filter(|note| !note.trim().is_empty())
                .map(str::to_owned)
        } else {
            None
        };
        let expected_issue_category = if is_task_completion {
            issue_status::IN_REVIEW
        } else {
            issue_status::IN_PROGRESS
        };
        let (issue, previous_issue) = publish_issue_update;
        tx.commit().await?;
        Ok(Some(DispatchPlan {
            event_id: event.id,
            assignment_id: assignment.id,
            issue,
            owner_type,
            owner_id,
            expected_owner_generation: Some(current_owner_generation),
            expected_issue_category: expected_issue_category.to_string(),
            publish_issue_update: event.event_type == EVENT_TASK_COMPLETED,
            publish_reviewer_update: false,
            previous_status: previous_issue.status,
            previous_assignee_type: previous_issue.assignee_type,
            previous_assignee_id: previous_issue.assignee_id,
            previous_reviewer_type: None,
            previous_reviewer_id: None,
            handoff_note,
            assignment_activity,
        }))
    }

    async fn dispatch(&self, plan: &DispatchPlan) -> Result<Uuid, TaskServiceError> {
        if let Some(task_id) = self
            .find_active_task(
                &plan.issue,
                &plan.owner_type,
                plan.owner_id,
                plan.assignment_id,
            )
            .await
            .map_err(|error| TaskServiceError::Internal(error.to_string()))?
        {
            return Ok(task_id);
        }
        let handoff_note = plan
            .handoff_note
            .as_deref()
            .filter(|note| !note.trim().is_empty())
            .unwrap_or_else(|| {
                if plan.expected_issue_category == issue_status::IN_REVIEW {
                    "Coordinator handoff: the implementation task completed. Review the change and either complete the issue or return it to in progress with actionable feedback."
                } else {
                    "Coordinator handoff: review returned this issue to in progress. Resume implementation and address the review feedback."
                }
            });
        let task = if plan.owner_type == "team" {
            let selected =
                team::get_team_in_workspace(&self.pool, plan.owner_id, plan.issue.workspace_id)
                    .await
                    .map_err(|error| TaskServiceError::Internal(error.to_string()))?
                    .ok_or_else(|| TaskServiceError::Internal("executor team not found".into()))?;
            self.tasks
                .enqueue_task_for_team_leader_with_handoff_unpublished(
                    &plan.issue,
                    selected.leader_id,
                    selected.id,
                    handoff_note,
                    None,
                    plan.assignment_id,
                )
                .await?
        } else {
            self.tasks
                .enqueue_task_for_agent_with_handoff_unpublished(
                    &plan.issue,
                    plan.owner_id,
                    handoff_note,
                    None,
                    plan.assignment_id,
                )
                .await?
        };
        Ok(task.id)
    }

    async fn find_active_task(
        &self,
        issue: &Issue,
        owner_type: &str,
        owner_id: Uuid,
        assignment_id: Uuid,
    ) -> anyhow::Result<Option<Uuid>> {
        let agent_id = if owner_type == "team" {
            team::get_team_in_workspace(&self.pool, owner_id, issue.workspace_id)
                .await?
                .map(|team| team.leader_id)
        } else if owner_type == "agent" {
            Some(owner_id)
        } else {
            None
        };
        let Some(agent_id) = agent_id else {
            return Ok(None);
        };
        Ok(sqlx::query_scalar(
            r#"SELECT id FROM agent_task_queue
WHERE issue_id = $1
  AND agent_id = $2
  AND context->>$3 = $4::text
  AND status IN ('queued', 'dispatched', 'running', 'waiting_local_directory', 'deferred')
ORDER BY created_at DESC, id DESC
LIMIT 1"#,
        )
        .bind(issue.id)
        .bind(agent_id)
        .bind(COORDINATION_ASSIGNMENT_ID_CONTEXT_KEY)
        .bind(assignment_id.to_string())
        .fetch_optional(&self.pool)
        .await?)
    }

    async fn revalidate_before_publication(
        &self,
        event: &CoordinationEvent,
        plan: &DispatchPlan,
        task_id: Uuid,
    ) -> anyhow::Result<Option<Issue>> {
        let mut tx = self.pool.begin().await?;
        let assignment = sqlx::query(
            r#"SELECT id, role, status, owner_type, owner_id, dispatched_task_id, decision
FROM agent_coordination_assignment
WHERE id = $1
  AND EXISTS (
      SELECT 1 FROM agent_coordination_outbox
      WHERE id = $2 AND status = 'processing' AND lease_owner = $3
  )
FOR UPDATE"#,
        )
        .bind(plan.assignment_id)
        .bind(event.id)
        .bind(&event.lease_owner)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(assignment) = assignment else {
            return Ok(None);
        };
        let assignment = Assignment {
            id: assignment.try_get(0)?,
            role: assignment.try_get(1)?,
            status: assignment.try_get(2)?,
            owner_type: assignment.try_get(3)?,
            owner_id: assignment.try_get(4)?,
            dispatched_task_id: assignment.try_get(5)?,
            decision: assignment.try_get(6)?,
        };
        let coordinated_task_status: Option<String> = sqlx::query_scalar(
            r#"SELECT status
FROM agent_task_queue
WHERE id = $1
  AND context->>$2 = $3::text
FOR UPDATE"#,
        )
        .bind(task_id)
        .bind(COORDINATION_ASSIGNMENT_ID_CONTEXT_KEY)
        .bind(plan.assignment_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;
        let issue = sqlx::query_as::<_, Issue>(
            "SELECT * FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
        )
        .bind(plan.issue.id)
        .bind(plan.issue.workspace_id)
        .fetch_optional(&mut *tx)
        .await?;
        let issue_is_current = if let Some(issue) = issue.as_ref() {
            issue_matches_dispatch_plan(&mut *tx, issue, plan).await
        } else {
            false
        };
        if issue_is_current {
            tx.commit().await?;
            return Ok(issue);
        }

        let reason = "coordinator handoff became stale before publication";
        if matches!(
            coordinated_task_status.as_deref(),
            Some("deferred") | Some("queued")
        ) {
            sqlx::query(
                r#"UPDATE agent_task_queue
SET status = 'cancelled', completed_at = now(), error = $4
WHERE id = $1
  AND status IN ('deferred', 'queued')
  AND context->>$2 = $3::text"#,
            )
            .bind(task_id)
            .bind(COORDINATION_ASSIGNMENT_ID_CONTEXT_KEY)
            .bind(plan.assignment_id.to_string())
            .bind(reason)
            .execute(&mut *tx)
            .await?;
        }
        complete_claimed_tx(
            &mut *tx,
            event,
            &assignment,
            "completed",
            Some(task_id),
            json!({
                "outcome": "stale_dispatch",
                "reason": reason,
                "task_id": task_id,
                "expected_issue_category": plan.expected_issue_category,
            }),
            Some(reason),
        )
        .await?;
        tx.commit().await?;
        Ok(None)
    }

    async fn mark_dispatched(
        &self,
        event: &CoordinationEvent,
        plan: &DispatchPlan,
        task_id: Uuid,
    ) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let assignment = sqlx::query(
            "SELECT id, role, status, owner_type, owner_id, dispatched_task_id, decision FROM agent_coordination_assignment WHERE id = $1 FOR UPDATE",
        )
        .bind(plan.assignment_id)
        .fetch_one(&mut *tx)
        .await?;
        let assignment = Assignment {
            id: assignment.try_get(0)?,
            role: assignment.try_get(1)?,
            status: assignment.try_get(2)?,
            owner_type: assignment.try_get(3)?,
            owner_id: assignment.try_get(4)?,
            dispatched_task_id: assignment.try_get(5)?,
            decision: assignment.try_get(6)?,
        };
        let coordinated_task_status: Option<String> = sqlx::query_scalar(
            r#"SELECT status
FROM agent_task_queue
WHERE id = $1
  AND context->>$2 = $3::text
FOR UPDATE"#,
        )
        .bind(task_id)
        .bind(COORDINATION_ASSIGNMENT_ID_CONTEXT_KEY)
        .bind(plan.assignment_id.to_string())
        .fetch_optional(&mut *tx)
        .await?;

        let issue = sqlx::query_as::<_, Issue>(
            "SELECT * FROM issue WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
        )
        .bind(plan.issue.id)
        .bind(plan.issue.workspace_id)
        .fetch_optional(&mut *tx)
        .await?;
        let issue_is_current = if let Some(issue) = issue.as_ref() {
            issue_matches_dispatch_plan(&mut *tx, issue, plan).await
        } else {
            false
        };

        if !issue_is_current {
            let reason = "coordinator handoff became stale before task promotion";
            if matches!(
                coordinated_task_status.as_deref(),
                Some("deferred") | Some("queued")
            ) {
                sqlx::query(
                    r#"UPDATE agent_task_queue
SET status = 'cancelled', completed_at = now(), error = $4
WHERE id = $1
  AND status IN ('deferred', 'queued')
  AND context->>$2 = $3::text"#,
                )
                .bind(task_id)
                .bind(COORDINATION_ASSIGNMENT_ID_CONTEXT_KEY)
                .bind(plan.assignment_id.to_string())
                .bind(reason)
                .execute(&mut *tx)
                .await?;
            }
            complete_claimed_tx(
                &mut *tx,
                event,
                &assignment,
                "completed",
                Some(task_id),
                json!({
                    "outcome": "stale_dispatch",
                    "reason": reason,
                    "task_id": task_id,
                    "expected_issue_category": plan.expected_issue_category,
                }),
                Some(reason),
            )
            .await?;
            tx.commit().await?;
            return Ok(false);
        }

        if !coordinated_task_is_promotable(coordinated_task_status.as_deref()) {
            return Err(anyhow::anyhow!(
                "coordinator task is missing or no longer promotable"
            ));
        }

        complete_claimed_tx(
            &mut *tx,
            event,
            &assignment,
            "dispatched",
            Some(task_id),
            json!({"outcome": "dispatched", "task_id": task_id}),
            None,
        )
        .await?;
        if coordinated_task_status.as_deref() == Some("deferred") {
            let promoted = sqlx::query(
                r#"UPDATE agent_task_queue
SET status = 'queued', fire_at = NULL
WHERE id = $1
  AND status = 'deferred'
  AND context->>$2 = $3::text"#,
            )
            .bind(task_id)
            .bind(COORDINATION_ASSIGNMENT_ID_CONTEXT_KEY)
            .bind(plan.assignment_id.to_string())
            .execute(&mut *tx)
            .await?;
            if promoted.rows_affected() != 1 {
                return Err(anyhow::anyhow!(
                    "coordinator task was not promoted after assignment correlation"
                ));
            }
        }
        tx.commit().await?;
        Ok(true)
    }

    async fn defer_claimed(
        &self,
        event: &CoordinationEvent,
        reason: &str,
        retry_after: Duration,
    ) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        let assignment = sqlx::query(
            "SELECT id, role, status, owner_type, owner_id, dispatched_task_id, decision FROM agent_coordination_assignment WHERE event_id = $1 FOR UPDATE",
        )
        .bind(event.id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = assignment {
            let assignment = Assignment {
                id: row.try_get(0)?,
                role: row.try_get(1)?,
                status: row.try_get(2)?,
                owner_type: row.try_get(3)?,
                owner_id: row.try_get(4)?,
                dispatched_task_id: row.try_get(5)?,
                decision: row.try_get(6)?,
            };
            defer_claimed_tx(
                &mut *tx,
                event,
                &assignment,
                reason,
                retry_after,
                assignment.owner_type.as_deref().zip(assignment.owner_id),
            )
            .await?;
        } else {
            let updated = sqlx::query(
                r#"UPDATE agent_coordination_outbox
SET status = 'pending', available_at = now() + $2::interval,
    lease_owner = NULL, lease_expires_at = NULL, last_error = $3,
    updated_at = now()
WHERE id = $1 AND status = 'processing' AND lease_owner = $4"#,
            )
            .bind(event.id)
            .bind(format!("{} seconds", retry_after.as_secs()))
            .bind(reason)
            .bind(&event.lease_owner)
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(anyhow::anyhow!(
                    "coordination event lease is no longer owned"
                ));
            }
        }
        tx.commit().await?;
        Ok(())
    }

    async fn wait_for_issue_update_publication(
        &self,
        event: &CoordinationEvent,
        plan: &DispatchPlan,
    ) -> anyhow::Result<()> {
        let started = Instant::now();
        loop {
            let acknowledged = sqlx::query_scalar::<_, bool>(
                r#"SELECT COALESCE((decision->>$3)::boolean, false)
FROM agent_coordination_assignment
WHERE id = $1 AND event_id = $2"#,
            )
            .bind(plan.assignment_id)
            .bind(event.id)
            .bind("issue_update_published")
            .fetch_optional(&self.pool)
            .await?;
            match acknowledged {
                Some(true) => return Ok(()),
                Some(false) if started.elapsed() < PUBLICATION_ACK_TIMEOUT => {}
                None => {
                    return Err(anyhow::anyhow!(
                        "coordination assignment disappeared while waiting for issue update publication"
                    ));
                }
                Some(false) => {
                    return Err(anyhow::anyhow!(
                        "ordered event side effects did not acknowledge issue update publication"
                    ));
                }
            }
            tokio::time::sleep(PUBLICATION_ACK_POLL).await;
        }
    }

    async fn mark_assignment_activity_published(
        &self,
        event: &CoordinationEvent,
        plan: &DispatchPlan,
    ) -> anyhow::Result<()> {
        let updated = sqlx::query(
            r#"UPDATE agent_coordination_assignment
SET decision = decision || jsonb_build_object('assignment_activity_published', true),
    updated_at = now()
WHERE id = $1
  AND EXISTS (
      SELECT 1 FROM agent_coordination_outbox
      WHERE id = $2 AND status = 'processing' AND lease_owner = $3
  )"#,
        )
        .bind(plan.assignment_id)
        .bind(event.id)
        .bind(&event.lease_owner)
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(anyhow::anyhow!(
                "coordination assignment lease is no longer owned while recording assignment activity publication"
            ));
        }
        Ok(())
    }

    fn publish_assignment_activity(&self, row: &ActivityLog) {
        self.bus.publish(&Event {
            event_type: patchbay_protocol::EVENT_ACTIVITY_CREATED.to_string(),
            workspace_id: row.workspace_id.to_string(),
            actor_type: row.actor_type.clone().unwrap_or_default(),
            actor_id: row.actor_id.map(|id| id.to_string()).unwrap_or_default(),
            payload: json!({
                "issue_id": row.issue_id.map(|id| id.to_string()).unwrap_or_default(),
                "entry": {
                    "type": "activity",
                    "id": row.id,
                    "actor_type": row.actor_type.clone().unwrap_or_default(),
                    "actor_id": row.actor_id.map(|id| id.to_string()).unwrap_or_default(),
                    "action": row.action.clone(),
                    "details": row.details.clone(),
                    "created_at": rfc3339(row.created_at),
                },
            }),
            task_id: row.id.to_string(),
            chat_session_id: String::new(),
        });
    }

    fn publish_review_handoff(&self, plan: &DispatchPlan, prefix: &str) {
        self.bus.publish(&Event {
            event_type: patchbay_protocol::EVENT_ISSUE_UPDATED.to_string(),
            workspace_id: plan.issue.workspace_id.to_string(),
            actor_type: "system".to_string(),
            actor_id: String::new(),
            payload: json!({
                "issue": issue_to_map_with_category(&plan.issue, prefix, issue_status::IN_REVIEW),
                // On the current issue contract the implementation owner stays
                // in assignee_* while the reviewer is recorded separately.
                "assignee_changed": false,
                "status_changed": true,
                "review_handoff": true,
                "coordination_publication": "review_handoff",
                "coordination_event_id": plan.event_id,
                "priority_changed": false,
                "project_changed": false,
                "start_date_changed": false,
                "due_date_changed": false,
                "description_changed": false,
                "title_changed": false,
                "prev_status": plan.previous_status,
                "prev_assignee_type": plan.previous_assignee_type,
                "prev_assignee_id": plan.previous_assignee_id.map(|id| id.to_string()),
            }),
            task_id: plan.event_id.to_string(),
            chat_session_id: String::new(),
        });
    }

    fn publish_reviewer_update(&self, plan: &DispatchPlan, prefix: &str) {
        self.bus.publish(&Event {
            event_type: patchbay_protocol::EVENT_ISSUE_UPDATED.to_string(),
            workspace_id: plan.issue.workspace_id.to_string(),
            actor_type: "system".to_string(),
            actor_id: String::new(),
            payload: json!({
                "issue": issue_to_map_with_category(&plan.issue, prefix, issue_status::IN_REVIEW),
                "assignee_changed": false,
                "status_changed": false,
                "review_handoff": false,
                "reviewer_changed": true,
                "coordination_publication": "reviewer_replacement",
                "coordination_event_id": plan.event_id,
                "priority_changed": false,
                "project_changed": false,
                "start_date_changed": false,
                "due_date_changed": false,
                "description_changed": false,
                "title_changed": false,
                "prev_status": plan.previous_status,
                "prev_assignee_type": plan.previous_assignee_type,
                "prev_assignee_id": plan.previous_assignee_id.map(|id| id.to_string()),
                "prev_reviewer_type": plan.previous_reviewer_type,
                "prev_reviewer_id": plan.previous_reviewer_id.map(|id| id.to_string()),
            }),
            task_id: plan.event_id.to_string(),
            chat_session_id: String::new(),
        });
    }
}

/// Records completion of the ordered side effects for a coordinator-owned
/// issue update. The coordinator waits for this durable acknowledgement before
/// finalizing its outbox row, so a process restart cannot mistake bus enqueue
/// for completed subscriber, notification, and Autopilot work.
pub async fn acknowledge_coordination_publication(
    pool: &sqlx::PgPool,
    event: &Event,
) -> anyhow::Result<()> {
    let Some(event_id) = event
        .payload
        .get("coordination_event_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
    else {
        return Ok(());
    };
    let publication = event
        .payload
        .get("coordination_publication")
        .and_then(Value::as_str)
        .or_else(|| {
            if event.payload.get("review_handoff").and_then(Value::as_bool) == Some(true) {
                Some("review_handoff")
            } else if event
                .payload
                .get("reviewer_changed")
                .and_then(Value::as_bool)
                == Some(true)
            {
                Some("reviewer_replacement")
            } else {
                None
            }
        });
    let Some(decision_key) = (match publication {
        Some("review_handoff") | Some("reviewer_replacement") => Some("issue_update_published"),
        Some("assignment_activity") => Some("assignment_activity_published"),
        _ => None,
    }) else {
        return Ok(());
    };
    sqlx::query(
        r#"UPDATE agent_coordination_assignment
SET decision = decision || jsonb_build_object($2, true), updated_at = now()
WHERE event_id = $1"#,
    )
    .bind(event_id)
    .bind(decision_key)
    .execute(pool)
    .await?;
    Ok(())
}

async fn issue_matches_dispatch_plan(
    executor: &mut sqlx::PgConnection,
    issue: &Issue,
    plan: &DispatchPlan,
) -> bool {
    let category = issue_status::effective(&mut *executor, issue.workspace_id, &issue.status).await;
    let owner_generation_matches = match plan.expected_owner_generation {
        Some(expected) => {
            sqlx::query_scalar::<_, i64>(
                "SELECT assignee_generation FROM issue WHERE id = $1 AND workspace_id = $2",
            )
            .bind(issue.id)
            .bind(issue.workspace_id)
            .fetch_optional(&mut *executor)
            .await
            .ok()
            .flatten()
                == Some(expected)
        }
        None => true,
    };
    owner_generation_matches
        && issue_matches_dispatch_fields(
            category.as_str(),
            issue,
            &plan.expected_issue_category,
            &plan.owner_type,
            plan.owner_id,
        )
}

fn coordinated_task_is_promotable(status: Option<&str>) -> bool {
    matches!(
        status,
        Some("deferred")
            | Some("queued")
            | Some("dispatched")
            | Some("running")
            | Some("waiting_local_directory")
    )
}

fn issue_matches_dispatch_fields(
    category: &str,
    issue: &Issue,
    expected_category: &str,
    owner_type: &str,
    owner_id: Uuid,
) -> bool {
    category == expected_category
        && if expected_category == issue_status::IN_REVIEW {
            owner_type == "agent"
                && issue.reviewer_type.as_deref() == Some("agent")
                && issue.reviewer_id == Some(owner_id)
        } else if expected_category == issue_status::IN_PROGRESS {
            issue.assignee_type.as_deref() == Some(owner_type)
                && issue.assignee_id == Some(owner_id)
        } else {
            false
        }
}

async fn find_assignment_activity(
    executor: &mut sqlx::PgConnection,
    issue_id: Uuid,
    event_id: Uuid,
    assignment_id: Uuid,
) -> anyhow::Result<Option<ActivityLog>> {
    let row = sqlx::query(
        r#"SELECT id, workspace_id, issue_id, actor_type, actor_id, action, details, created_at
FROM activity_log
WHERE issue_id = $1
  AND action = 'coordinator_assignment'
  AND details->>'event_id' = $2::uuid::text
  AND details->>'assignment_id' = $3::uuid::text
ORDER BY created_at DESC, id DESC
LIMIT 1"#,
    )
    .bind(issue_id)
    .bind(event_id)
    .bind(assignment_id)
    .fetch_optional(&mut *executor)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(ActivityLog {
        id: row.try_get(0)?,
        workspace_id: row.try_get(1)?,
        issue_id: row.try_get(2)?,
        actor_type: row.try_get(3)?,
        actor_id: row.try_get(4)?,
        action: row.try_get(5)?,
        details: row.try_get(6)?,
        created_at: row.try_get(7)?,
    }))
}

async fn cancel_unpromoted_reviewer_task(
    executor: &mut sqlx::PgConnection,
    issue_id: Uuid,
    assignment_id: Uuid,
    reviewer_id: Uuid,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"UPDATE agent_task_queue
SET status = 'cancelled', completed_at = now(),
    error = 'reviewer assignment was replaced before dispatch'
WHERE issue_id = $1
  AND agent_id = $2
  AND context->>$3 = $4::text
  AND status IN ('deferred', 'queued')"#,
    )
    .bind(issue_id)
    .bind(reviewer_id)
    .bind(COORDINATION_ASSIGNMENT_ID_CONTEXT_KEY)
    .bind(assignment_id.to_string())
    .execute(&mut *executor)
    .await?;
    Ok(())
}

#[derive(Debug)]
enum TargetVerdict {
    Ready,
    Waitable(String),
    Blocked(String),
}

async fn select_reviewer(
    executor: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    team_id: Option<Uuid>,
    source_agent_id: Option<Uuid>,
) -> anyhow::Result<Option<ReviewerCandidate>> {
    let row = sqlx::query(
        r#"SELECT a.id, a.name
FROM agent a
WHERE a.workspace_id = $1
  AND a.kind = 'user'
  AND a.archived_at IS NULL
  AND a.runtime_id IS NOT NULL
  AND ($3::uuid IS NULL OR a.id <> $3)
  AND EXISTS (
      SELECT 1
      FROM team_member tm
      JOIN team t ON t.id = tm.team_id AND t.workspace_id = a.workspace_id
                  AND t.archived_at IS NULL
      WHERE tm.member_type = 'agent'
        AND tm.member_id = a.id
        AND tm.role = 'reviewer'
        AND ($2::uuid IS NULL OR tm.team_id = $2)
  )
  AND (
      SELECT count(*)
      FROM agent_task_queue q
      WHERE q.agent_id = a.id
        AND q.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory')
  ) + (
      SELECT count(*)
      FROM agent_coordination_assignment reservation
      WHERE reservation.owner_type = 'agent'
        AND reservation.owner_id = a.id
        AND reservation.status = 'assigned'
        AND reservation.dispatched_task_id IS NULL
  ) < a.max_concurrent_tasks
ORDER BY CASE WHEN a.status = 'idle' THEN 0 ELSE 1 END,
         a.updated_at ASC,
         a.id ASC
LIMIT 1
FOR UPDATE SKIP LOCKED"#,
    )
    .bind(workspace_id)
    .bind(team_id)
    .bind(source_agent_id)
    .fetch_optional(&mut *executor)
    .await?;
    row.map(|row| {
        Ok(ReviewerCandidate {
            id: row.try_get(0)?,
            name: row.try_get(1)?,
        })
    })
    .transpose()
}

async fn reviewer_is_dispatchable(
    executor: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    team_id: Option<Uuid>,
    source_agent_id: Option<Uuid>,
    reviewer_id: Uuid,
    assignment_id: Uuid,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        r#"SELECT a.id
FROM agent a
WHERE a.id = $4
  AND a.workspace_id = $1
  AND a.kind = 'user'
  AND a.archived_at IS NULL
  AND a.runtime_id IS NOT NULL
  AND ($3::uuid IS NULL OR a.id <> $3)
  AND EXISTS (
      SELECT 1
      FROM team_member tm
      JOIN team t ON t.id = tm.team_id AND t.workspace_id = a.workspace_id
                  AND t.archived_at IS NULL
      WHERE tm.member_type = 'agent'
        AND tm.member_id = a.id
        AND tm.role = 'reviewer'
        AND ($2::uuid IS NULL OR tm.team_id = $2)
  )
  AND (
      SELECT count(*)
        FROM agent_task_queue q
      WHERE q.agent_id = a.id
        AND q.status IN ('queued', 'dispatched', 'running', 'waiting_local_directory')
        AND COALESCE(q.context->>'coordination_assignment_id', '') <> $5::uuid::text
  ) + (
      SELECT count(*)
      FROM agent_coordination_assignment reservation
      WHERE reservation.owner_type = 'agent'
        AND reservation.owner_id = a.id
        AND reservation.status = 'assigned'
        AND reservation.dispatched_task_id IS NULL
        AND reservation.id <> $5
  ) < a.max_concurrent_tasks
FOR UPDATE"#,
    )
    .bind(workspace_id)
    .bind(team_id)
    .bind(source_agent_id)
    .bind(reviewer_id)
    .bind(assignment_id)
    .fetch_optional(&mut *executor)
    .await?;
    Ok(row.is_some())
}

async fn validate_target(
    executor: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    owner_type: &str,
    owner_id: Uuid,
) -> anyhow::Result<TargetVerdict> {
    match owner_type {
        "agent" => {
            let row = sqlx::query(
                "SELECT archived_at, runtime_id, kind FROM agent WHERE id = $1 AND workspace_id = $2",
            )
            .bind(owner_id)
            .bind(workspace_id)
            .fetch_optional(&mut *executor)
            .await?;
            let Some(row) = row else {
                return Ok(TargetVerdict::Blocked("executor agent not found".into()));
            };
            let archived_at: Option<DateTime<Utc>> = row.try_get(0)?;
            let runtime_id: Option<Uuid> = row.try_get(1)?;
            let kind: String = row.try_get(2)?;
            if archived_at.is_some() || kind != "user" {
                return Ok(TargetVerdict::Blocked(
                    "executor agent is unavailable".into(),
                ));
            }
            if runtime_id.is_none() {
                return Ok(TargetVerdict::Waitable(
                    "executor agent has no bound runtime".into(),
                ));
            }
            Ok(TargetVerdict::Ready)
        }
        "team" => {
            let row = sqlx::query(
                r#"SELECT t.archived_at, a.archived_at, a.runtime_id, a.kind
FROM team t
LEFT JOIN agent a ON a.id = t.leader_id AND a.workspace_id = t.workspace_id
WHERE t.id = $1 AND t.workspace_id = $2"#,
            )
            .bind(owner_id)
            .bind(workspace_id)
            .fetch_optional(&mut *executor)
            .await?;
            let Some(row) = row else {
                return Ok(TargetVerdict::Blocked("executor team not found".into()));
            };
            let team_archived: Option<DateTime<Utc>> = row.try_get(0)?;
            let agent_archived: Option<DateTime<Utc>> = row.try_get(1)?;
            let runtime_id: Option<Uuid> = row.try_get(2)?;
            let kind: Option<String> = row.try_get(3)?;
            if team_archived.is_some()
                || agent_archived.is_some()
                || kind.as_deref() != Some("user")
            {
                return Ok(TargetVerdict::Blocked(
                    "executor team leader is unavailable".into(),
                ));
            }
            if runtime_id.is_none() {
                return Ok(TargetVerdict::Waitable(
                    "executor team leader has no bound runtime".into(),
                ));
            }
            Ok(TargetVerdict::Ready)
        }
        _ => Ok(TargetVerdict::Blocked(
            "executor owner type is not dispatchable".into(),
        )),
    }
}

async fn record_assignment_decision(
    executor: &mut sqlx::PgConnection,
    event: &CoordinationEvent,
    assignment: &Assignment,
    role: &str,
    owner_type: &str,
    owner_id: Uuid,
    decision: Value,
) -> anyhow::Result<()> {
    let updated = sqlx::query(
        r#"UPDATE agent_coordination_assignment
SET role = $2,
    status = 'assigned',
    owner_type = $3,
    owner_id = $4,
    decision = decision || $5::jsonb,
    attempt = attempt + 1,
    assigned_at = COALESCE(assigned_at, now()),
    last_error = NULL,
    updated_at = now()
WHERE id = $1
  AND event_id = $6
  AND EXISTS (
      SELECT 1 FROM agent_coordination_outbox
      WHERE id = $7 AND status = 'processing' AND lease_owner = $8
  )"#,
    )
    .bind(assignment.id)
    .bind(role)
    .bind(owner_type)
    .bind(owner_id)
    .bind(decision)
    .bind(event.id)
    .bind(event.id)
    .bind(&event.lease_owner)
    .execute(&mut *executor)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(anyhow::anyhow!(
            "coordination assignment lease is no longer owned"
        ));
    }
    Ok(())
}

async fn complete_claimed_tx(
    executor: &mut sqlx::PgConnection,
    event: &CoordinationEvent,
    assignment: &Assignment,
    status: &str,
    dispatched_task_id: Option<Uuid>,
    decision: Value,
    last_error: Option<&str>,
) -> anyhow::Result<()> {
    let assignment_update = sqlx::query(
        r#"UPDATE agent_coordination_assignment
SET status = $2,
    dispatched_task_id = COALESCE($3, dispatched_task_id),
    dispatched_at = CASE WHEN $3::uuid IS NULL THEN dispatched_at ELSE COALESCE(dispatched_at, now()) END,
    decision = decision || $4::jsonb,
    last_error = $5,
    updated_at = now()
WHERE id = $1
  AND EXISTS (
      SELECT 1 FROM agent_coordination_outbox
      WHERE id = $6 AND status = 'processing' AND lease_owner = $7
  )"#,
    )
    .bind(assignment.id)
    .bind(status)
    .bind(dispatched_task_id)
    .bind(decision)
    .bind(last_error)
    .bind(event.id)
    .bind(&event.lease_owner)
    .execute(&mut *executor)
    .await?;
    if assignment_update.rows_affected() != 1 {
        return Err(anyhow::anyhow!(
            "coordination assignment lease is no longer owned"
        ));
    }
    let outbox_update = sqlx::query(
        r#"UPDATE agent_coordination_outbox
SET status = 'completed',
    processed_at = now(),
    lease_owner = NULL,
    lease_expires_at = NULL,
    last_error = $2,
    updated_at = now()
WHERE id = $1 AND status = 'processing' AND lease_owner = $3"#,
    )
    .bind(event.id)
    .bind(last_error)
    .bind(&event.lease_owner)
    .execute(&mut *executor)
    .await?;
    if outbox_update.rows_affected() != 1 {
        return Err(anyhow::anyhow!(
            "coordination event lease is no longer owned"
        ));
    }
    Ok(())
}

async fn defer_claimed_tx(
    executor: &mut sqlx::PgConnection,
    event: &CoordinationEvent,
    assignment: &Assignment,
    reason: &str,
    retry_after: Duration,
    owner: Option<(&str, Uuid)>,
) -> anyhow::Result<()> {
    let assignment_status = if assignment.owner_id.is_some() || owner.is_some() {
        "assigned"
    } else {
        "pending"
    };
    if let Some((owner_type, owner_id)) = owner {
        let assignment_update = sqlx::query(
            r#"UPDATE agent_coordination_assignment
SET status = $2, owner_type = COALESCE(owner_type, $3), owner_id = COALESCE(owner_id, $4),
    last_error = $5, updated_at = now()
WHERE id = $1
  AND EXISTS (
      SELECT 1 FROM agent_coordination_outbox
      WHERE id = $6 AND status = 'processing' AND lease_owner = $7
  )"#,
        )
        .bind(assignment.id)
        .bind(assignment_status)
        .bind(owner_type)
        .bind(owner_id)
        .bind(reason)
        .bind(event.id)
        .bind(&event.lease_owner)
        .execute(&mut *executor)
        .await?;
        if assignment_update.rows_affected() != 1 {
            return Err(anyhow::anyhow!(
                "coordination assignment lease is no longer owned"
            ));
        }
    } else {
        let assignment_update = sqlx::query(
            r#"UPDATE agent_coordination_assignment
SET status = $2, last_error = $3, updated_at = now()
WHERE id = $1
  AND EXISTS (
      SELECT 1 FROM agent_coordination_outbox
      WHERE id = $4 AND status = 'processing' AND lease_owner = $5
  )"#,
        )
        .bind(assignment.id)
        .bind(assignment_status)
        .bind(reason)
        .bind(event.id)
        .bind(&event.lease_owner)
        .execute(&mut *executor)
        .await?;
        if assignment_update.rows_affected() != 1 {
            return Err(anyhow::anyhow!(
                "coordination assignment lease is no longer owned"
            ));
        }
    }
    let outbox_update = sqlx::query(
        r#"UPDATE agent_coordination_outbox
SET status = 'pending',
    available_at = now() + $2::interval,
    lease_owner = NULL,
    lease_expires_at = NULL,
    last_error = $3,
    updated_at = now()
WHERE id = $1 AND status = 'processing' AND lease_owner = $4"#,
    )
    .bind(event.id)
    .bind(format!("{} seconds", retry_after.as_secs()))
    .bind(reason)
    .bind(&event.lease_owner)
    .execute(&mut *executor)
    .await?;
    if outbox_update.rows_affected() != 1 {
        return Err(anyhow::anyhow!(
            "coordination event lease is no longer owned"
        ));
    }
    Ok(())
}

/// Writes the task-completed outbox event and its pending reviewer assignment
/// in the same transaction as the task terminal transition.
pub async fn record_task_completed(
    executor: &mut sqlx::PgConnection,
    task: &AgentTaskQueue,
) -> anyhow::Result<()> {
    let Some(issue_id) = task.issue_id else {
        return Ok(());
    };
    let is_side_chat = task
        .context
        .as_ref()
        .and_then(Value::as_object)
        .is_some_and(|context| context.contains_key("side_chat_parent_task_id"));
    if is_side_chat {
        return Ok(());
    }

    let context = task.context.as_ref().and_then(Value::as_object);
    let coordination_assignment_id = context
        .and_then(|context| context.get(COORDINATION_ASSIGNMENT_ID_CONTEXT_KEY))
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    let captured_owner_type = context
        .and_then(|context| context.get(COORDINATION_OWNER_TYPE_CONTEXT_KEY))
        .and_then(Value::as_str);
    let captured_owner_id = context
        .and_then(|context| context.get(COORDINATION_OWNER_ID_CONTEXT_KEY))
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    let captured_owner_generation = context
        .and_then(|context| context.get(COORDINATION_OWNER_GENERATION_CONTEXT_KEY))
        .and_then(Value::as_i64);
    let captured_issue_revision = context
        .and_then(|context| context.get(COORDINATION_ISSUE_REVISION_CONTEXT_KEY))
        .and_then(Value::as_i64);
    if coordination_assignment_id.is_none()
        && (captured_owner_type.is_none() || captured_owner_id.is_none())
    {
        // A plain @mention task is a separate conversation, not the issue's
        // implementation task. Its completion must never move the issue into
        // review or create a reviewer assignment.
        return Ok(());
    }

    let source_role: Option<String> = if let Some(assignment_id) = coordination_assignment_id {
        sqlx::query_scalar("SELECT role FROM agent_coordination_assignment WHERE id = $1")
            .bind(assignment_id)
            .fetch_optional(&mut *executor)
            .await?
    } else {
        sqlx::query_scalar(
            "SELECT role FROM agent_coordination_assignment WHERE dispatched_task_id = $1 ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(task.id)
        .fetch_optional(&mut *executor)
        .await?
    };
    if coordination_assignment_id.is_some() && source_role.is_none() {
        return Err(anyhow::anyhow!(
            "coordinator task completed before its assignment was correlated"
        ));
    }

    let issue = sqlx::query(
        "SELECT workspace_id, assignee_type, assignee_id, assignee_generation FROM issue WHERE id = $1",
    )
    .bind(issue_id)
    .fetch_optional(&mut *executor)
    .await?;
    let Some(issue) = issue else {
        return Ok(());
    };
    let workspace_id: Uuid = issue.try_get(0)?;
    let current_owner_type: Option<String> = issue.try_get(1)?;
    let current_owner_id: Option<Uuid> = issue.try_get(2)?;
    let current_owner_generation: i64 = issue.try_get(3)?;
    if source_role.as_deref() != Some(ASSIGNMENT_REVIEWER) {
        if let (Some(captured_owner_type), Some(captured_owner_id)) =
            (captured_owner_type, captured_owner_id)
        {
            if implementation_completion_is_superseded(
                current_owner_type.as_deref(),
                current_owner_id,
                captured_owner_type,
                captured_owner_id,
                captured_owner_generation,
                current_owner_generation,
            ) {
                tracing::info!(
                    task_id = %task.id,
                    issue_id = %issue_id,
                    captured_owner_type,
                    captured_owner_id = %captured_owner_id,
                    current_owner_type = ?current_owner_type,
                    current_owner_id = ?current_owner_id,
                    captured_owner_generation,
                    current_owner_generation,
                    captured_issue_revision,
                    "ignoring completion from a superseded implementation owner"
                );
                return Ok(());
            }
        }
    }
    let payload = json!({
        "task_id": task.id,
        "issue_id": issue_id,
        "agent_id": task.agent_id,
        "source_role": source_role.unwrap_or_else(|| "implementation".to_string()),
        "implementation_owner_type": captured_owner_type,
        "implementation_owner_id": captured_owner_id,
        "implementation_owner_generation": captured_owner_generation,
        "implementation_issue_revision": captured_issue_revision,
        "coordination_assignment_id": coordination_assignment_id,
    });
    record_event_and_assignment(
        executor,
        &format!("task_completed:{}", task.id),
        workspace_id,
        issue_id,
        Some(task.id),
        EVENT_TASK_COMPLETED,
        payload,
        ASSIGNMENT_REVIEWER,
    )
    .await
}

fn implementation_completion_is_superseded(
    current_owner_type: Option<&str>,
    current_owner_id: Option<Uuid>,
    captured_owner_type: &str,
    captured_owner_id: Uuid,
    captured_owner_generation: Option<i64>,
    current_owner_generation: i64,
) -> bool {
    current_owner_type != Some(captured_owner_type)
        || current_owner_id != Some(captured_owner_id)
        || captured_owner_generation.is_some_and(|captured| current_owner_generation != captured)
}

/// Writes the review-return outbox event and its pending executor assignment
/// in the same transaction as the issue status/owner restoration.
pub async fn record_review_return(
    executor: &mut sqlx::PgConnection,
    issue: &Issue,
    source_task_id: Option<Uuid>,
    handoff_note: Option<&str>,
) -> anyhow::Result<()> {
    let owner_generation: i64 = sqlx::query_scalar(
        "SELECT assignee_generation FROM issue WHERE id = $1 AND workspace_id = $2",
    )
    .bind(issue.id)
    .bind(issue.workspace_id)
    .fetch_one(&mut *executor)
    .await?;
    let payload = json!({
        "issue_id": issue.id,
        "owner_type": issue.assignee_type,
        "owner_id": issue.assignee_id,
        "owner_generation": owner_generation,
        "issue_revision": issue.revision,
        "handoff_note": handoff_note,
    });
    record_event_and_assignment(
        executor,
        &format!("review_returned:{}:{}", issue.id, issue.revision),
        issue.workspace_id,
        issue.id,
        source_task_id,
        EVENT_REVIEW_RETURNED,
        payload,
        ASSIGNMENT_EXECUTOR,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn record_event_and_assignment(
    executor: &mut sqlx::PgConnection,
    event_key: &str,
    workspace_id: Uuid,
    issue_id: Uuid,
    source_task_id: Option<Uuid>,
    event_type: &str,
    payload: Value,
    role: &str,
) -> anyhow::Result<()> {
    let event_id: Uuid = sqlx::query_scalar(
        r#"INSERT INTO agent_coordination_outbox
    (id, event_key, workspace_id, issue_id, source_task_id, event_type, payload)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (event_key) DO UPDATE SET updated_at = agent_coordination_outbox.updated_at
RETURNING id"#,
    )
    .bind(new_v7())
    .bind(event_key)
    .bind(workspace_id)
    .bind(issue_id)
    .bind(source_task_id)
    .bind(event_type)
    .bind(payload)
    .fetch_one(&mut *executor)
    .await?;
    sqlx::query(
        r#"INSERT INTO agent_coordination_assignment
    (id, event_id, workspace_id, issue_id, source_task_id, role)
VALUES ($1, $2, $3, $4, $5, $6)
ON CONFLICT (event_id, role) DO NOTHING"#,
    )
    .bind(new_v7())
    .bind(event_id)
    .bind(workspace_id)
    .bind(issue_id)
    .bind(source_task_id)
    .bind(role)
    .execute(&mut *executor)
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinatorShutdownOutcome {
    Stopped,
    Panicked,
    TimedOut,
}

/// Production-owned root for the coordinator supervisor. PostgreSQL leases
/// make dropping/restarting this runtime safe: an unfinished row becomes
/// claimable after its lease expires.
pub struct CoordinatorRuntime {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl CoordinatorRuntime {
    pub async fn shutdown(mut self, timeout: Duration) -> CoordinatorShutdownOutcome {
        self.cancel.cancel();
        let mut task = self
            .task
            .take()
            .expect("coordinator runtime always owns a supervisor");
        match tokio::time::timeout(timeout, &mut task).await {
            Ok(Ok(())) => CoordinatorShutdownOutcome::Stopped,
            Ok(Err(_)) => CoordinatorShutdownOutcome::Panicked,
            Err(_) => {
                task.abort();
                let _ = task.await;
                CoordinatorShutdownOutcome::TimedOut
            }
        }
    }
}

impl Drop for CoordinatorRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implementation_completion_uses_owner_snapshot_as_stale_fence() {
        let owner_id = Uuid::from_u128(1);
        assert!(!implementation_completion_is_superseded(
            Some("agent"),
            Some(owner_id),
            "agent",
            owner_id,
            Some(0),
            0,
        ));
        assert!(implementation_completion_is_superseded(
            Some("agent"),
            Some(Uuid::from_u128(2)),
            "agent",
            owner_id,
            Some(0),
            0,
        ));
        assert!(implementation_completion_is_superseded(
            Some("agent"),
            Some(owner_id),
            "agent",
            owner_id,
            Some(0),
            1,
        ));
        assert!(implementation_completion_is_superseded(
            Some("agent"),
            Some(owner_id),
            "agent",
            owner_id,
            Some(1),
            0,
        ));
    }

    #[test]
    fn dispatch_revalidation_ignores_unrelated_issue_revision_changes() {
        let owner_id = Uuid::from_u128(1);
        let mut issue = Issue {
            acceptance_criteria: serde_json::json!([]),
            assignee_id: Some(Uuid::from_u128(2)),
            assignee_type: Some("agent".to_string()),
            context_refs: serde_json::json!([]),
            created_at: Utc::now(),
            creator_id: Uuid::from_u128(3),
            creator_type: "member".to_string(),
            description: None,
            due_date: None,
            first_executed_at: None,
            id: Uuid::from_u128(4),
            last_activity_at: None,
            metadata: serde_json::json!({}),
            number: 1,
            origin_id: None,
            origin_type: None,
            parent_issue_id: None,
            position: 0.0,
            priority: "none".to_string(),
            project_id: None,
            properties: serde_json::json!({}),
            revision: 7,
            reviewer_id: Some(owner_id),
            reviewer_type: Some("agent".to_string()),
            stage: None,
            start_date: None,
            status: issue_status::IN_REVIEW.to_string(),
            title: "handoff".to_string(),
            updated_at: Utc::now(),
            workspace_id: Uuid::from_u128(5),
        };
        assert!(issue_matches_dispatch_fields(
            issue_status::IN_REVIEW,
            &issue,
            issue_status::IN_REVIEW,
            "agent",
            owner_id,
        ));

        issue.revision = 8;
        assert!(issue_matches_dispatch_fields(
            issue_status::IN_REVIEW,
            &issue,
            issue_status::IN_REVIEW,
            "agent",
            owner_id,
        ));
    }

    #[test]
    fn only_live_coordinator_tasks_can_be_promoted() {
        for status in [
            "deferred",
            "queued",
            "dispatched",
            "running",
            "waiting_local_directory",
        ] {
            assert!(coordinated_task_is_promotable(Some(status)), "{status}");
        }
        for status in ["completed", "failed", "cancelled"] {
            assert!(!coordinated_task_is_promotable(Some(status)), "{status}");
        }
        assert!(!coordinated_task_is_promotable(None));
    }
}
