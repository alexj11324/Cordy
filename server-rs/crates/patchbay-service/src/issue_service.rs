//! Issue creation service — full port of `service/issue.go` plus the
//! assignment-trigger predicate from `service/issue_trigger.go` and the
//! refusal notice from `service/runtime_unusable_notice.go`.
//!
//! IssueService is the single service-layer entry point for creating issues:
//! duplicate guard, issue numbering, label/attachment linking, broadcast,
//! analytics, and agent/squad enqueue stay aligned across every create entry
//! (HTTP POST /issues, channel /issue, future MCP/API-key callers). The
//! service stays transport-agnostic — callers pass fully-resolved params.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use patchbay_analytics as analytics;
use patchbay_db::dbid::new_v7;
use patchbay_db::models::{Agent, AgentTaskQueue, Attachment, Issue, IssueLabel};
use patchbay_db::queries::agent::{get_agent, has_pending_task_for_issue_and_agent};
use patchbay_db::queries::attachment::{link_attachments_to_issue, list_attachments_by_issue};
use patchbay_db::queries::issue::{create_issue, create_issue_with_origin, get_issue_in_workspace};
use patchbay_db::queries::issue_label::{attach_label_to_issue_on_create, get_label};
use patchbay_db::queries::issue_status::lock_issue_status_catalog_shared;
use patchbay_db::queries::project::get_project_in_workspace;
use patchbay_db::queries::squad::get_squad_in_workspace;
use patchbay_db::queries::workspace::{get_workspace, increment_issue_counter};

use crate::agent_ready::{agent_readiness, AgentVerdict};
use crate::dispatch_reason::ReasonCode;
use crate::issue_guard::lock_and_find_active_duplicate;
use crate::issue_position::next_top_position;
use crate::issue_status;
use crate::task_notify::issue_to_map_with_category;
use crate::task_service::{opt_str, TaskService};

/// Single service-layer entry point for creating issues. Deliberately does
/// NOT depend on any transport — callers parse their own request payload and
/// pass fully-resolved [`IssueCreateParams`].
pub struct IssueService {
    pub pool: PgPool,
    pub bus: Arc<patchbay_events::Bus>,
    /// PostHog client; nil-safe everywhere (events degrade to metrics-only).
    pub analytics: Option<Box<dyn analytics::AnalyticsClient>>,
    /// Shared business-metrics collector. Unset on self-hosted without the
    /// metrics listener — record_event treats it as "PostHog only".
    pub metrics: Option<std::sync::Arc<patchbay_metrics::BusinessMetrics>>,
    pub task_svc: Arc<TaskService>,
}

impl IssueService {
    pub fn new(pool: PgPool, bus: Arc<patchbay_events::Bus>, task_svc: Arc<TaskService>) -> Self {
        Self {
            pool,
            bus,
            analytics: None,
            metrics: None,
            task_svc,
        }
    }
}

/// Already-validated, already-resolved inputs to [`IssueService::create`].
/// The handler owns parsing; the service stays transport-agnostic.
#[derive(Debug, Clone, Default)]
pub struct IssueCreateParams {
    pub workspace_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub assignee_type: Option<String>,
    pub assignee_id: Option<Uuid>,
    /// "agent" or "member".
    pub creator_type: String,
    pub creator_id: Uuid,
    pub parent_issue_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    pub start_date: Option<chrono::NaiveDate>,
    pub due_date: Option<chrono::NaiveDate>,
    pub origin_type: Option<String>,
    pub origin_id: Option<Uuid>,
    pub attachment_ids: Vec<Uuid>,
    /// Issue-scoped labels attached inside the create transaction so the
    /// issue never commits with a partial or wrong label set. An unknown or
    /// non-issue label id fails the whole create with
    /// [`IssueCreateError::LabelNotFound`].
    pub label_ids: Vec<Uuid>,
    pub allow_duplicate: bool,
    /// Ordered barrier group under the parent (None = unstaged).
    pub stage: Option<i32>,
}

type BroadcastPayloadBuilder =
    dyn Fn(&Issue, &[Attachment], &[IssueLabel]) -> serde_json::Value + Send + Sync;

/// Optional knobs for [`IssueService::create`]. Most callers leave defaults.
#[derive(Default)]
pub struct IssueCreateOpts {
    /// Invoked after the issue row exists and attachments are linked; its
    /// value becomes the EventIssueCreated payload. The HTTP handler injects
    /// its response shape here without forcing this module to depend on the
    /// handler layer. When absent a minimal `{"issue_id": …}` payload fires —
    /// enough for cache invalidation only. The labels argument is the
    /// authoritative snapshot attached in the create transaction.
    pub broadcast_payload: Option<Arc<BroadcastPayloadBuilder>>,
    /// Overrides the broadcast/analytics actor when it differs from the row
    /// creator. Empty falls back to creator_id.
    pub actor_id: String,
    /// Assignee agent (or creator agent for agent-created issues); resolved
    /// by the caller because it depends on transport context.
    pub analytics_agent_id: String,
    /// Client surface tag for the analytics/metrics event.
    pub platform: String,
    /// Creates the automatic assigned-agent task durably deferred. Channel
    /// /issue uses this while detached media resolves; None keeps the
    /// ordinary immediate enqueue path.
    pub assigned_agent_run_fire_at: Option<DateTime<Utc>>,
}

/// Typed failure surface of [`IssueService::create`]. The four sentinel
/// variants are product outcomes callers translate into transport responses
/// (409 / 400); `duplicate` rides the ActiveDuplicate variant so callers can
/// render the conflicting row.
#[derive(Debug, thiserror::Error)]
pub enum IssueCreateError {
    #[error("active duplicate issue exists")]
    ActiveDuplicate { duplicate: Option<Box<Issue>> },
    #[error("parent issue not found in this workspace")]
    ParentIssueNotFound,
    #[error("project not found in this workspace")]
    ProjectNotFound,
    #[error("issue label not found in this workspace")]
    LabelNotFound,
    #[error("issue status is no longer available")]
    StatusUnavailable,
    #[error("issues with work underway require an assignee")]
    ActiveAssigneeRequired,
    #[error("{0}")]
    Internal(String),
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
}

/// Typed return of [`IssueService::create`]: happy path fills issue +
/// attachments (+labels, +assigned task id); the duplicate path arrives as
/// [`IssueCreateError::ActiveDuplicate`].
#[derive(Debug, Default)]
pub struct IssueCreateResult {
    pub issue: Option<Issue>,
    pub attachments: Vec<Attachment>,
    /// Set when Create enqueued the automatic task for an agent assignee,
    /// including a deferred-by-fire-at task.
    pub assigned_task_id: Option<Uuid>,
    /// Authoritative label snapshot attached in the create transaction.
    pub labels: Vec<IssueLabel>,
}

fn ic_err(context: &'static str, e: impl std::fmt::Display) -> IssueCreateError {
    IssueCreateError::Internal(format!("{context}: {e}"))
}

// --- Create -----------------------------------------------------------------

impl IssueService {
    /// Runs the full issue-creation pipeline atomically end-to-end: custom
    /// status catalog lock + re-resolve, parent/project workspace-boundary
    /// validation (with parent→project back-fill), duplicate guard, counter,
    /// position, insert (with optional origin stamping), in-tx label attach,
    /// optional media-gated deferred assigned task, commit, best-effort
    /// attachment linking, broadcast, analytics, and assign-driven enqueues.
    ///
    /// Validation owned HERE (parent existence, project membership, back-fill)
    /// applies to every entry point; caller-owned validation is limited to
    /// transport-shaped checks (title required, date format, pair sanity).
    pub async fn create(
        &self,
        p: IssueCreateParams,
        opts: IssueCreateOpts,
    ) -> Result<IssueCreateResult, IssueCreateError> {
        let mut tx = self.pool.begin().await.map_err(IssueCreateError::Sql)?;

        // A create landing on a CUSTOM status takes the shared catalog lock
        // AND re-resolves the status inside this transaction: an archive can
        // commit between the caller's pre-flight validation and here, and
        // re-checking under the lock makes the status provably active at
        // write time. Built-ins skip both — the common path is unchanged.
        // (PB-6243)
        let status_category = if !issue_status::is_built_in(&p.status) {
            lock_issue_status_catalog_shared(&mut *tx, p.workspace_id)
                .await
                .map_err(|e| ic_err("lock issue status catalog", e))?;
            issue_status::resolve(&mut *tx, p.workspace_id, &p.status)
                .await
                .map_err(|_| IssueCreateError::StatusUnavailable)?
                .category
        } else {
            p.status.clone()
        };
        if issue_status::requires_assignee(&status_category)
            && (p.assignee_type.is_none() || p.assignee_id.is_none())
        {
            return Err(IssueCreateError::ActiveAssigneeRequired);
        }

        // Resolve and validate parent/project BEFORE the duplicate guard so a
        // forged id is rejected before we touch the issue counter. Both scope
        // by WorkspaceID — there is no path from here to a foreign-workspace
        // row.
        let mut project_id = p.project_id;
        if let Some(parent_id) = p.parent_issue_id.filter(|id| !id.is_nil()) {
            let parent = get_issue_in_workspace(&mut *tx, parent_id, p.workspace_id)
                .await
                .map_err(ic_err_parent)?;
            let Some(parent) = parent else {
                return Err(IssueCreateError::ParentIssueNotFound);
            };
            // Sub-issue inherits its parent's project unless overridden —
            // long-standing HTTP behavior.
            if project_id.is_none() {
                project_id = parent.project_id;
            }
        }
        if let Some(pid) = project_id.filter(|id| !id.is_nil()) {
            // Any lookup failure (missing OR transient) reads as not-found to
            // the caller — matching Go's single error funnel.
            if get_project_in_workspace(&mut *tx, pid, p.workspace_id)
                .await
                .map_err(|_| IssueCreateError::ProjectNotFound)?
                .is_none()
            {
                return Err(IssueCreateError::ProjectNotFound);
            }
        }

        // Validate labels before incrementing the counter so a stale/wrong-
        // scope selection fails the create cheaply; rows echo back as the
        // authoritative snapshot.
        let labels = validate_issue_labels(&mut tx, p.workspace_id, &p.label_ids).await?;

        if let (Some(duplicate), true) = lock_and_find_active_duplicate(
            &mut tx,
            p.workspace_id,
            project_id.filter(|id| !id.is_nil()),
            p.parent_issue_id.filter(|id| !id.is_nil()),
            &p.title,
            p.allow_duplicate,
        )
        .await
        .map_err(|e| ic_err("duplicate guard", e))?
        {
            return Err(IssueCreateError::ActiveDuplicate {
                duplicate: Some(Box::new(duplicate)),
            });
        }

        let issue_number = increment_issue_counter(&mut *tx, p.workspace_id)
            .await
            .map_err(|e| ic_err("increment counter", e))?
            .ok_or_else(|| ic_err_msg("increment counter: no row"))?;

        // New issues sort to the top of their column. Computed after the
        // counter took the workspace row lock so concurrent creates see each
        // other's positions; a concurrent manual reorder does NOT take that
        // lock, so collisions there remain possible and tolerated.
        let new_position = next_top_position(&mut *tx, p.workspace_id, &p.status)
            .await
            .map_err(|e| ic_err("next top position", e))?;

        let issue = if p.origin_type.is_some() {
            create_issue_with_origin(
                &mut *tx,
                p.workspace_id,
                &p.title,
                p.description.as_deref(),
                &p.status,
                &p.priority,
                p.assignee_type.as_deref(),
                p.assignee_id.filter(|id| !id.is_nil()),
                &p.creator_type,
                p.creator_id,
                p.parent_issue_id.filter(|id| !id.is_nil()),
                new_position,
                p.start_date,
                p.due_date,
                issue_number,
                project_id.filter(|id| !id.is_nil()),
                p.origin_type.as_deref(),
                p.origin_id.filter(|id| !id.is_nil()),
                p.stage,
                new_v7(),
            )
            .await
        } else {
            create_issue(
                &mut *tx,
                p.workspace_id,
                &p.title,
                p.description.as_deref(),
                &p.status,
                &p.priority,
                p.assignee_type.as_deref(),
                p.assignee_id.filter(|id| !id.is_nil()),
                &p.creator_type,
                p.creator_id,
                p.parent_issue_id.filter(|id| !id.is_nil()),
                new_position,
                p.start_date,
                p.due_date,
                issue_number,
                project_id.filter(|id| !id.is_nil()),
                p.stage,
                new_v7(),
            )
            .await
        };
        let issue = issue
            .map_err(|e| ic_err("create issue", e))?
            .ok_or_else(|| ic_err_msg("create issue: no row"))?;

        // Labels attach inside the create transaction so issue + labels commit
        // together (the old two-round-trip flow left partial failures
        // mis-categorized). Ids were validated above.
        for label in &labels {
            attach_label_to_issue_on_create(&mut *tx, issue.id, label.id, p.workspace_id)
                .await
                .map_err(|e| ic_err("attach issue label", e))?;
        }

        // The issue must never become visible without its media-gated
        // assigned task: inserting both through the tx makes the unique-index
        // winner deterministic.
        let mut assigned_task: Option<AgentTaskQueue> = None;
        let fire_at = opts.assigned_agent_run_fire_at;
        if let Some(fire_at) = fire_at {
            if Self::should_enqueue_agent_task_with_queries(&mut tx, &issue).await {
                assigned_task = Some(
                    self.task_svc
                        .create_deferred_channel_issue_task_tx(&mut tx, &issue, fire_at)
                        .await
                        .map_err(|e| ic_err("create deferred channel issue task", e))?,
                );
            }
        }

        tx.commit().await.map_err(IssueCreateError::Sql)?;

        let attachments = self.link_attachments(&issue, &p.attachment_ids).await;

        let actor_id = if opts.actor_id.is_empty() {
            issue.creator_id.to_string()
        } else {
            opts.actor_id.clone()
        };

        let mut assigned_task_id: Option<Uuid> = None;
        if fire_at.is_some() {
            if let Some(task) = &assigned_task {
                assigned_task_id = Some(task.id);
                // Overlays are best-effort on every enqueue path: the task is
                // durable and safely deferred, so an integration failure must
                // not turn a committed issue into a retry duplicate.
                if let Err(err) = self
                    .task_svc
                    .hydrate_deferred_channel_issue_task_overlay(task)
                    .await
                {
                    tracing::warn!(
                        issue_id = %issue.id,
                        task_id = %task.id,
                        error = %err,
                        "hydrate deferred channel issue task overlay failed"
                    );
                }
            } else if self.should_enqueue_squad_leader_on_assign(&issue).await {
                // fire-at currently belongs to channel /issue, which always
                // resolves an agent assignee; keep the ordinary squad path for
                // future callers that supply the option with a squad.
                self.enqueue_squad_leader_task(&issue, None, &p.creator_type, &actor_id)
                    .await;
            }
        }

        self.publish_issue_created(
            &issue,
            &attachments,
            &labels,
            &p.creator_type,
            &actor_id,
            &opts,
        );
        self.capture_created_analytics(&issue, &p.creator_type, &actor_id, &opts);
        let mut result = IssueCreateResult {
            issue: Some(issue),
            attachments,
            assigned_task_id,
            labels,
        };
        if fire_at.is_none() {
            result.assigned_task_id = self
                .maybe_enqueue_on_assign(
                    result.issue.as_ref().expect("just created"),
                    &p.creator_type,
                    &actor_id,
                )
                .await;
        }
        Ok(result)
    }
}

fn ic_err_msg(msg: impl std::fmt::Display) -> IssueCreateError {
    IssueCreateError::Internal(msg.to_string())
}

fn ic_err_parent(e: impl std::fmt::Display) -> IssueCreateError {
    // Any parent-load failure (missing OR transient) reads as not-found —
    // matching Go's single error funnel into ErrParentIssueNotFound.
    let _ = e;
    IssueCreateError::ParentIssueNotFound
}

/// Checks every requested label exists in the workspace and is issue-scoped,
/// returning de-duplicated rows so Create echoes an authoritative snapshot
/// without a second query. Mirrors AttachLabelToIssue's workspace +
/// resource_type='issue' guard: an unknown or wrong-scope id surfaces as
/// LabelNotFound instead of a silent no-op insert. Per-issue label counts are
/// small, so a GetLabel per distinct id avoids a new batch query.
async fn validate_issue_labels(
    exec: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    label_ids: &[Uuid],
) -> Result<Vec<IssueLabel>, IssueCreateError> {
    if label_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut seen = std::collections::HashSet::with_capacity(label_ids.len());
    let mut deduped = Vec::with_capacity(label_ids.len());
    for label_id in label_ids {
        if !seen.insert(*label_id) {
            continue;
        }
        let label = get_label(&mut *exec, *label_id, workspace_id)
            .await
            .map_err(ic_err_label_lookup)?
            .ok_or(IssueCreateError::LabelNotFound)?;
        if label.resource_type != "issue" {
            return Err(IssueCreateError::LabelNotFound);
        }
        deduped.push(label);
    }
    Ok(deduped)
}

fn ic_err_label_lookup(e: impl std::fmt::Display) -> IssueCreateError {
    ic_err("get issue label", e)
}

impl IssueService {
    /// Links pre-uploaded attachments to the new issue and re-fetches the
    /// rows so callers build responses without a second query. Errors log and
    /// swallow: linking is best-effort post-commit, and a stale attachment
    /// doesn't justify failing the create.
    async fn link_attachments(&self, issue: &Issue, ids: &[Uuid]) -> Vec<Attachment> {
        if ids.is_empty() {
            return Vec::new();
        }
        if let Err(err) = link_attachments_to_issue(
            &self.pool,
            issue.id,
            issue.workspace_id,
            ids.to_vec(),
            false,
        )
        .await
        {
            tracing::error!(issue_id = %issue.id, error = %err, "failed to link attachments to issue");
            return Vec::new();
        }
        match list_attachments_by_issue(&self.pool, issue.id, issue.workspace_id).await {
            Ok(list) => list,
            Err(err) => {
                tracing::warn!(issue_id = %issue.id, error = %err, "failed to list attachments for new issue");
                Vec::new()
            }
        }
    }

    /// Emits issue:created via the caller-supplied payload builder, falling
    /// back to a minimal `{"issue_id"}` so cache invalidations still fire if
    /// the caller forgot the builder.
    fn publish_issue_created(
        &self,
        issue: &Issue,
        attachments: &[Attachment],
        labels: &[IssueLabel],
        creator_type: &str,
        actor_id: &str,
        opts: &IssueCreateOpts,
    ) {
        let payload = match &opts.broadcast_payload {
            Some(build) => build(issue, attachments, labels),
            None => json!({ "issue_id": issue.id.to_string() }),
        };
        self.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_ISSUE_CREATED.to_string(),
            workspace_id: issue.workspace_id.to_string(),
            actor_type: creator_type.to_string(),
            actor_id: actor_id.to_string(),
            payload,
            task_id: String::new(),
            chat_session_id: String::new(),
        });
    }

    /// Refreshes attachments and the issue projection after a detached
    /// channel media transaction. Creation was broadcast before the remote
    /// download finished, so the attachment event closes the cache gap; the
    /// issue:updated carries the materialized description through a protocol
    /// installed clients already understand.
    pub async fn publish_attachments_changed(&self, issue: &Issue, actor_id: Uuid) {
        let current = match get_issue_in_workspace(&self.pool, issue.id, issue.workspace_id).await {
            Ok(Some(current)) => current,
            Ok(None) | Err(_) => {
                tracing::warn!(issue_id = %issue.id, "failed to load issue after channel media bind");
                self.publish_issue_attachments_changed(issue, actor_id, 0);
                return;
            }
        };
        let workspace = match get_workspace(&self.pool, issue.workspace_id).await {
            Ok(Some(ws)) => ws,
            Ok(None) | Err(_) => {
                tracing::warn!(
                    workspace_id = %issue.workspace_id,
                    "failed to load workspace after channel media bind"
                );
                // Without the workspace there is no matching owner snapshot;
                // keep this auxiliary event unversioned so clients invalidate
                // instead of advancing past a snapshot they never received.
                self.publish_issue_attachments_changed(issue, actor_id, 0);
                return;
            }
        };
        let effective =
            issue_status::effective(&self.pool, current.workspace_id, &current.status).await;
        self.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_ISSUE_UPDATED.to_string(),
            workspace_id: current.workspace_id.to_string(),
            actor_type: "member".to_string(),
            actor_id: actor_id.to_string(),
            payload: json!({
                "issue": issue_to_map_with_category(
                    &current,
                    &workspace.issue_prefix,
                    &effective,
                ),
                "assignee_changed": false,
                "status_changed": false,
                "project_changed": false,
            }),
            task_id: String::new(),
            chat_session_id: String::new(),
        });
        // Auxiliary projection publishes only AFTER the full owner snapshot
        // at this revision — reversed order makes revision-aware clients
        // reject the update payload as an equal-revision duplicate.
        self.publish_issue_attachments_changed(&current, actor_id, current.revision);
    }

    fn publish_issue_attachments_changed(&self, issue: &Issue, actor_id: Uuid, revision: i64) {
        let mut payload = serde_json::Map::new();
        payload.insert("issue_id".into(), json!(issue.id.to_string()));
        if revision > 0 {
            payload.insert("issue_revision".into(), json!(revision));
        }
        self.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_ISSUE_ATTACHMENTS_CHANGED.to_string(),
            workspace_id: issue.workspace_id.to_string(),
            actor_type: "member".to_string(),
            actor_id: actor_id.to_string(),
            payload: serde_json::Value::Object(payload),
            task_id: String::new(),
            chat_session_id: String::new(),
        });
    }

    fn capture_created_analytics(
        &self,
        issue: &Issue,
        creator_type: &str,
        actor_id: &str,
        opts: &IssueCreateOpts,
    ) {
        let (source, task_id, autopilot_run_id) = classify_origin(issue);
        let analytics_actor_id = if creator_type == "agent" {
            format!("agent:{actor_id}")
        } else {
            actor_id.to_string()
        };
        let ev = analytics::issue_created(
            &analytics_actor_id,
            &issue.workspace_id.to_string(),
            &issue.id.to_string(),
            &opts.analytics_agent_id,
            &task_id,
            &autopilot_run_id,
            source,
            &opts.platform,
        );
        patchbay_metrics::business_events::record_event(
            self.analytics.as_deref(),
            self.metrics.as_deref(),
            &ev,
        );
    }

    /// Leaves the refusal on the issue when an assignment cannot be enqueued
    /// because the assignee's CLI cannot run on its machine. Assignment has
    /// no reply anyone reads, so without this the user gets exactly the
    /// silence PB-6164 removes. Best-effort: logged, never returned.
    async fn note_runtime_unusable(&self, issue: &Issue, verdict: &AgentVerdict) {
        let name = get_agent(&self.pool, issue.assignee_id.unwrap_or_else(Uuid::nil))
            .await
            .ok()
            .flatten()
            .map(|a| a.name)
            .unwrap_or_default();
        // author_type='system', author_id=zero UUID (valid 16 bytes; the
        // column is NOT NULL and clients branch on author_type).
        let created = match patchbay_db::queries::comment::create_comment(
            &self.pool,
            issue.id,
            issue.workspace_id,
            "system",
            Uuid::nil(),
            &crate::agent_ready::runtime_unusable_notice(&name, verdict),
            "system",
            None,
            None,
            None,
            None,
            new_v7(),
        )
        .await
        {
            Ok(Some(created)) => created,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    issue_id = %issue.id,
                    "runtime unusable notice: create system comment failed"
                );
                return;
            }
        };
        self.bus.publish(&patchbay_events::Event {
            event_type: patchbay_protocol::EVENT_COMMENT_CREATED.to_string(),
            workspace_id: issue.workspace_id.to_string(),
            actor_type: "system".to_string(),
            actor_id: String::new(),
            payload: json!({
                "comment": {
                    "id": created.id.expect("inserted").to_string(),
                    "issue_id": created.issue_id.expect("inserted").to_string(),
                    "author_type": created.author_type,
                    "author_id": created.author_id.unwrap_or_else(Uuid::nil).to_string(),
                    "content": created.content,
                    "type": created.type_,
                    "revision": created.revision,
                },
                "issue_title": issue.title,
                "issue_revision": created.issue_revision,
            }),
            task_id: String::new(),
            chat_session_id: String::new(),
        });
    }

    /// Assignment-time enqueue decision: backlog parks silently (PB-6243),
    /// blocked runtimes leave a visible refusal notice (PB-6164), ready
    /// agents enqueue immediately, squads route to their leader when ready.
    async fn maybe_enqueue_on_assign(
        &self,
        issue: &Issue,
        creator_type: &str,
        actor_id: &str,
    ) -> Option<Uuid> {
        // Guard-only: the value re-reads from `issue` in each branch below.
        issue.assignee_id?;
        issue.assignee_type.as_deref()?;
        // Backlog is the parking lot: nothing runs from it, so nothing here
        // needs explaining either — a custom status in the backlog category
        // parks the same way. (PB-6243)
        if issue_status::effective(&self.pool, issue.workspace_id, &issue.status).await
            == issue_status::BACKLOG
        {
            return None;
        }
        let (verdict, admitted) = match self.pool.acquire().await {
            Ok(mut conn) => Self::agent_assignee_verdict(&mut conn, issue).await,
            // Mirrors Go's lookup-error path: default (non-unusable) verdict
            // skips the direct enqueue while the squad fallback still runs.
            Err(err) => {
                tracing::warn!(issue_id = %issue.id, error = %err, "enqueue on assign: acquire failed");
                default_verdict()
            }
        };
        if !admitted && verdict.reason == ReasonCode::RuntimeUnusable {
            // Assignment has no response the assigner reads, so the refusal
            // explains itself on the issue instead of vanishing (PB-6164).
            self.note_runtime_unusable(issue, &verdict).await;
        }
        if admitted {
            match self.task_svc.enqueue_task_for_issue(issue, None).await {
                Ok(task) => return Some(task.id),
                Err(err) => {
                    tracing::warn!(
                        issue_id = %issue.id,
                        error = %err,
                        "enqueue agent task on create failed"
                    );
                }
            }
        }
        if self.should_enqueue_squad_leader_on_assign(issue).await {
            self.enqueue_squad_leader_task(issue, None, creator_type, actor_id)
                .await;
        }
        None
    }

    /// True when an issue create should trigger the assigned agent. Runs
    /// INSIDE the create transaction against its snapshot (PB-6243), where
    /// there is nothing to tell anyone yet — unlike the assignment path,
    /// which learns WHY via agent_assignee_verdict.
    async fn should_enqueue_agent_task_with_queries(
        exec: &mut sqlx::PgConnection,
        issue: &Issue,
    ) -> bool {
        if issue_status::effective(&mut *exec, issue.workspace_id, &issue.status).await
            == issue_status::BACKLOG
        {
            return false;
        }
        Self::is_agent_assignee_ready_with_queries(exec, issue).await
    }

    async fn is_agent_assignee_ready_with_queries(
        exec: &mut sqlx::PgConnection,
        issue: &Issue,
    ) -> bool {
        Self::agent_assignee_verdict(exec, issue).await.1
    }

    /// Resolves the issue's agent assignee through the shared readiness
    /// check. Only a BLOCKED verdict stops the enqueue: a merely offline
    /// machine still queues — that work runs when the laptop comes back.
    async fn agent_assignee_verdict(
        exec: &mut sqlx::PgConnection,
        issue: &Issue,
    ) -> (AgentVerdict, bool) {
        let Some(assignee_id) = issue.assignee_id else {
            return default_verdict();
        };
        if issue.assignee_type.as_deref() != Some("agent") {
            return default_verdict();
        }
        let Ok(Some(agent)) = get_agent(&mut *exec, assignee_id).await else {
            return default_verdict();
        };
        let Ok(verdict) = agent_readiness(&mut *exec, &agent).await else {
            return default_verdict();
        };
        let admitted = !verdict.blocked();
        (verdict, admitted)
    }

    async fn should_enqueue_squad_leader_on_assign(&self, issue: &Issue) -> bool {
        if issue_status::effective(&self.pool, issue.workspace_id, &issue.status).await
            == issue_status::BACKLOG
        {
            return false;
        }
        self.is_squad_leader_ready(issue).await
    }

    async fn is_squad_leader_ready(&self, issue: &Issue) -> bool {
        let Some(assignee_id) = issue.assignee_id else {
            return false;
        };
        if issue.assignee_type.as_deref() != Some("squad") {
            return false;
        }
        let Some(squad) = get_squad_in_workspace(&self.pool, assignee_id, issue.workspace_id)
            .await
            .ok()
            .flatten()
        else {
            return false;
        };
        let Some(agent) = get_agent(&self.pool, squad.leader_id).await.ok().flatten() else {
            return false;
        };
        matches!(
            agent_readiness(&self.pool, &agent).await,
            Ok(v) if v.ready()
        )
    }

    /// Squad-leader enqueue with pending-run dedup keyed on the reviewed
    /// HEAD (TEN-356). Best-effort throughout.
    async fn enqueue_squad_leader_task(
        &self,
        issue: &Issue,
        trigger_comment_id: Option<Uuid>,
        _author_type: &str,
        _author_id: &str,
    ) {
        let Some(assignee_id) = issue.assignee_id else {
            return;
        };
        let Some(squad) = get_squad_in_workspace(&self.pool, assignee_id, issue.workspace_id)
            .await
            .ok()
            .flatten()
        else {
            return;
        };
        let head_sha = self.task_svc.resolve_issue_review_sha(issue.id).await;
        match has_pending_task_for_issue_and_agent(
            &self.pool,
            issue.id,
            squad.leader_id,
            opt_str(&head_sha),
        )
        .await
        {
            Ok(Some(false)) => {}
            _ => return,
        }
        if let Err(err) = self
            .task_svc
            .enqueue_task_for_squad_leader(issue, squad.leader_id, squad.id, trigger_comment_id)
            .await
        {
            tracing::warn!(
                issue_id = %issue.id,
                squad_id = %squad.id,
                leader_id = %squad.leader_id,
                error = %err,
                "enqueue squad leader task on create failed"
            );
        }
    }
}

fn default_verdict() -> (AgentVerdict, bool) {
    (
        AgentVerdict {
            availability: crate::agent_ready::AgentAvailability::Blocked,
            reason: ReasonCode::TargetUnavailable,
            repair: None,
            detail: String::new(),
        },
        false,
    )
}

// --- Assignment-trigger predicate (issue_trigger.go) ------------------------

/// Which kind of issue write would start an agent run; surfaced in previews
/// so the UI can explain each trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnqueueSource {
    /// Creation and assignee changes — the issue is handed to an agent/squad.
    Assign,
    /// Promoting an already-assigned issue out of backlog.
    Status,
}

impl RunEnqueueSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Assign => "assign",
            Self::Status => "status",
        }
    }
}

/// Shared shape of the three optional probe closures.
pub type ProbePredicate<A> = Box<dyn Fn(A) -> bool + Send + Sync>;

/// Request-scoped checks WillEnqueueRun cannot resolve from issue state
/// alone. Write paths pass allow-all probes (their gates ran at the HTTP
/// boundary); preview passes the real gates so it never leaks a private
/// agent's readiness to a member who cannot see it.
pub struct IssueTriggerProbe {
    /// Private-agent gate; None = allow-all.
    pub can_access_agent: Option<ProbePredicate<Agent>>,
    /// Whether promoting out of backlog would be the calling agent
    /// re-triggering its own running task; consulted by the status source.
    pub is_self_loop: Option<ProbePredicate<()>>,
    /// Direct-agent self-claim while the (issue, agent) pair already holds a
    /// non-terminal task: ownership succeeds, duplicate enqueue suppressed.
    /// Cross-issue handoffs to a fresh target remain runnable.
    pub suppress_active_self_assignment: Option<ProbePredicate<Uuid>>,
}

/// One prospective issue write in its post-write shape.
pub struct IssueTriggerInput {
    pub issue: Issue,
    pub prev_status: String,
    pub is_create: bool,
    pub assignee_changed: bool,
    pub status_changed: bool,
}

/// Resolved decision shared by preview and write paths. `agent_id` is who
/// actually runs — assignee for agent issues, squad leader otherwise.
#[derive(Debug, Clone)]
pub struct IssueRunTrigger {
    pub issue_id: Uuid,
    pub agent_id: Uuid,
    pub assignee_type: String,
    pub source: RunEnqueueSource,
}

impl IssueService {
    /// The single predicate answering "will this issue write start an agent
    /// run, and for whom" — one source of truth shared by update /
    /// batch-update write paths and the preview endpoint (PB-3375 replaced
    /// four drifting per-site copies).
    ///
    /// Intentionally distinct from the comment trigger: issue writes park on
    /// backlog while comments fire in any status; they share only leaf
    /// readiness checks. The decision must equal the real enqueue conditions
    /// — the status source mirrors the pending-task unique index so preview
    /// never promises a run the write coalesces away, while the assign source
    /// skips that check (creates target fresh issues; reassignment no longer
    /// cancels existing tasks #4963/PB-4113, and the insert simply no-ops on
    /// the shared slot in the rare collision).
    pub async fn will_enqueue_run(
        &self,
        input: IssueTriggerInput,
        probe: IssueTriggerProbe,
    ) -> Option<IssueRunTrigger> {
        let issue = &input.issue;
        let assignee_id = issue.assignee_id?;
        let assignee_type = issue.assignee_type.as_deref()?;

        let can_access = |agent: &Agent| match &probe.can_access_agent {
            Some(f) => f(agent.clone()),
            None => true,
        };

        // Both transition sides normalize to the canonical inherited status:
        // a custom status in the backlog category parks exactly like Backlog,
        // one in the todo category starts exactly like Todo (PB-6243). So
        // the status source requires LEAVING the backlog CATEGORY, not merely
        // changing the key (PB-6463). Built-ins resolve to themselves
        // without a query, keeping workspaces with no custom statuses on the
        // identical path.
        let current_status =
            issue_status::effective(&self.pool, issue.workspace_id, &issue.status).await;
        let prev_status =
            issue_status::effective(&self.pool, issue.workspace_id, &input.prev_status).await;

        let source = if input.is_create || input.assignee_changed {
            // Backlog is the parking lot: assigning into it never starts a run.
            if current_status == issue_status::BACKLOG {
                return None;
            }
            RunEnqueueSource::Assign
        } else if input.status_changed
            && prev_status == issue_status::BACKLOG
            && current_status != issue_status::BACKLOG
            && current_status != issue_status::DONE
            && current_status != issue_status::CANCELLED
        {
            if probe.is_self_loop.as_ref().is_some_and(|f| f(())) {
                return None;
            }
            RunEnqueueSource::Status
        } else {
            return None;
        };

        match assignee_type {
            "agent" => {
                let agent = get_agent(&self.pool, assignee_id).await.ok()??;
                if agent.runtime_id.is_none() || agent.archived_at.is_some() {
                    return None;
                }
                if !can_access(&agent) {
                    return None;
                }
                if source == RunEnqueueSource::Assign
                    && !input.is_create
                    && probe
                        .suppress_active_self_assignment
                        .as_ref()
                        .is_some_and(|f| f(assignee_id))
                {
                    return None;
                }
                if source == RunEnqueueSource::Status
                    && self.has_pending_run(issue.id, assignee_id).await
                {
                    return None;
                }
                Some(IssueRunTrigger {
                    issue_id: issue.id,
                    agent_id: assignee_id,
                    assignee_type: "agent".to_string(),
                    source,
                })
            }
            // Pair-scoped self-assignment suppression intentionally applies
            // only to DIRECT agent ownership: assigning a squad changes the
            // execution context (briefing, roles, member routing), so even a
            // leader acting on its own squad is an intentional group handoff.
            // The status path still uses the leader's pending-task guard.
            "squad" => {
                let squad = get_squad_in_workspace(&self.pool, assignee_id, issue.workspace_id)
                    .await
                    .ok()??;
                let leader = get_agent(&self.pool, squad.leader_id).await.ok()??;
                let verdict = agent_readiness(&self.pool, &leader).await.ok()?;
                if !verdict.ready() {
                    return None;
                }
                if !can_access(&leader) {
                    return None;
                }
                if source == RunEnqueueSource::Status
                    && self.has_pending_run(issue.id, squad.leader_id).await
                {
                    return None;
                }
                Some(IssueRunTrigger {
                    issue_id: issue.id,
                    agent_id: squad.leader_id,
                    assignee_type: "squad".to_string(),
                    source,
                })
            }
            _ => None,
        }
    }

    /// Whether the agent already holds a queued/dispatched task for the issue
    /// — the (issue_id, agent_id) unique-index slot, dedup keyed on the
    /// reviewed HEAD so a pending run against an old HEAD doesn't shadow a
    /// request after HEAD advanced (TEN-356). Errors fail closed so preview
    /// never over-promises a run.
    async fn has_pending_run(&self, issue_id: Uuid, agent_id: Uuid) -> bool {
        let head_sha = self.task_svc.resolve_issue_review_sha(issue_id).await;
        has_pending_task_for_issue_and_agent(&self.pool, issue_id, agent_id, opt_str(&head_sha))
            .await
            .map(|pending| pending.unwrap_or(true))
            .unwrap_or(true)
    }
}

/// Maps the issue's origin columns into analytics source labels. Unknown
/// origin falls back to manual with a warning — analytics drift beats
/// dropping the event entirely.
fn classify_origin(issue: &Issue) -> (&'static str, String, String) {
    let Some(origin_type) = issue.origin_type.as_deref() else {
        return (analytics::SOURCE_MANUAL, String::new(), String::new());
    };
    let origin_id = issue.origin_id.unwrap_or_else(Uuid::nil).to_string();
    match origin_type {
        // Both link back to the agent_task_queue row that created the issue
        // (agent_create is the ordinary `issue create` path, PB-4305);
        // surface that task id under the manual source label.
        "quick_create" | "agent_create" => (analytics::SOURCE_MANUAL, origin_id, String::new()),
        "autopilot" => (analytics::SOURCE_AUTOPILOT, String::new(), origin_id),
        other => {
            tracing::warn!(
                origin_type = other,
                issue_id = %issue.id,
                "analytics: unknown issue origin type"
            );
            (analytics::SOURCE_MANUAL, String::new(), String::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use sqlx::postgres::PgPoolOptions;

    async fn required_pool() -> PgPool {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required for issue creation transaction contracts");
        PgPoolOptions::new()
            .max_connections(8)
            .connect(&url)
            .await
            .expect("connect contract PostgreSQL")
    }

    async fn tagged_pool(application_name: &str) -> PgPool {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL is required for issue creation transaction contracts");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .expect("connect tagged contract PostgreSQL");
        sqlx::query("SELECT set_config('application_name', $1, false)")
            .bind(application_name)
            .execute(&pool)
            .await
            .expect("tag contract PostgreSQL connection");
        pool
    }

    async fn workspace(pool: &PgPool) -> Uuid {
        let slug = format!("issue-create-contract-{}", Uuid::now_v7().simple());
        sqlx::query_scalar(
            "INSERT INTO workspace (name, slug) VALUES ('issue create contract', $1) RETURNING id",
        )
        .bind(slug)
        .fetch_one(pool)
        .await
        .expect("create workspace")
    }

    fn service(pool: &PgPool) -> Arc<IssueService> {
        let bus = Arc::new(patchbay_events::Bus::new());
        let tasks = Arc::new(TaskService::new(pool.clone(), bus.clone()));
        Arc::new(IssueService::new(pool.clone(), bus, tasks))
    }

    fn params(workspace_id: Uuid, title: &str, status: &str) -> IssueCreateParams {
        let creator_id = Uuid::now_v7();
        IssueCreateParams {
            workspace_id,
            title: title.into(),
            status: status.into(),
            priority: "none".into(),
            assignee_type: Some("member".into()),
            assignee_id: Some(creator_id),
            creator_type: "member".into(),
            creator_id,
            ..IssueCreateParams::default()
        }
    }

    async fn create(
        service: &IssueService,
        params: IssueCreateParams,
    ) -> Result<Issue, IssueCreateError> {
        service
            .create(params, IssueCreateOpts::default())
            .await
            .map(|result| result.issue.expect("created issue"))
    }

    async fn cleanup(pool: &PgPool, workspace_id: Uuid) {
        sqlx::query("DELETE FROM issue WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(pool)
            .await
            .expect("delete issues");
        sqlx::query("DELETE FROM issue_status WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(pool)
            .await
            .expect("delete issue statuses");
        sqlx::query("DELETE FROM project WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(pool)
            .await
            .expect("delete projects");
        sqlx::query("DELETE FROM workspace WHERE id = $1")
            .bind(workspace_id)
            .execute(pool)
            .await
            .expect("delete workspace");
    }

    async fn wait_for_duplicate_lock(pool: &PgPool, workspace_id: Uuid, title: &str) {
        let key = format!(
            "issue-active-duplicate|{workspace_id}|||{}",
            crate::issue_guard::normalize_title(title)
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let mut probe = pool.begin().await.expect("duplicate lock probe");
                let available: bool = sqlx::query_scalar(
                    "SELECT pg_try_advisory_xact_lock(hashtextextended($1::text, 0))",
                )
                .bind(&key)
                .fetch_one(&mut *probe)
                .await
                .expect("probe duplicate lock");
                probe
                    .rollback()
                    .await
                    .expect("release duplicate lock probe");
                if !available {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("production create did not acquire the duplicate lock");
    }

    async fn wait_for_tagged_advisory_wait(pool: &PgPool, application_name: &str) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM pg_stat_activity \
                     WHERE application_name = $1 AND state = 'active' \
                       AND wait_event_type = 'Lock' AND wait_event = 'advisory')",
                )
                .bind(application_name)
                .fetch_one(pool)
                .await
                .expect("observe duplicate advisory lock wait");
                if waiting {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("second production create did not wait for the duplicate lock");
    }

    #[tokio::test]
    async fn production_create_enforces_duplicate_identity_and_column_top_order() {
        let pool = required_pool().await;
        let workspace_id = workspace(&pool).await;
        crate::issue_status::ensure(&pool, workspace_id)
            .await
            .expect("seed statuses");
        let service = service(&pool);

        let mut unassigned_active = params(workspace_id, "Owner required", "in_progress");
        unassigned_active.assignee_type = None;
        unassigned_active.assignee_id = None;
        assert!(matches!(
            create(&service, unassigned_active).await,
            Err(IssueCreateError::ActiveAssigneeRequired)
        ));

        let first = create(&service, params(workspace_id, "First", "todo"))
            .await
            .expect("first issue");
        let second = create(&service, params(workspace_id, "Second", "todo"))
            .await
            .expect("second issue");
        assert_eq!(first.position, -1.0);
        assert_eq!(second.position, -2.0);

        sqlx::query("UPDATE issue SET position = -50 WHERE id = $1")
            .bind(first.id)
            .execute(&pool)
            .await
            .expect("simulate drag reorder");
        let next = create(&service, params(workspace_id, "After drag", "todo"))
            .await
            .expect("issue after drag");
        assert_eq!(next.position, -51.0);

        let mut unassigned_todo = params(workspace_id, "Parked without owner", "todo");
        unassigned_todo.assignee_type = None;
        unassigned_todo.assignee_id = None;
        create(&service, unassigned_todo)
            .await
            .expect("todo may remain unassigned");

        let original = create(
            &service,
            params(workspace_id, "  Duplicate\u{00a0}Title  ", "in_progress"),
        )
        .await
        .expect("original issue");
        let duplicate = create(
            &service,
            params(workspace_id, "duplicate\u{2003}title", "in_progress"),
        )
        .await
        .expect_err("normalized active duplicate must be rejected");
        match duplicate {
            IssueCreateError::ActiveDuplicate {
                duplicate: Some(found),
            } => {
                assert_eq!(found.id, original.id);
                assert_eq!(found.title, original.title);
                assert_eq!(found.status, "in_progress");
            }
            other => panic!("unexpected duplicate result: {other:?}"),
        }

        let mut allowed = params(workspace_id, "duplicate title", "in_progress");
        allowed.allow_duplicate = true;
        let allowed = create(&service, allowed)
            .await
            .expect("explicit duplicate override");
        assert_ne!(allowed.id, original.id);

        sqlx::query("UPDATE issue SET status = 'done' WHERE id = ANY($1)")
            .bind(vec![original.id, allowed.id])
            .execute(&pool)
            .await
            .expect("close duplicates");
        create(
            &service,
            params(workspace_id, "duplicate title", "in_progress"),
        )
        .await
        .expect("closed effective statuses do not block");

        patchbay_db::queries::issue_status::create_issue_status_entry(
            &pool,
            workspace_id,
            "human_review",
            "Human Review",
            "",
            "in_progress",
            "#8b5cf6",
        )
        .await
        .expect("create custom status")
        .expect("custom status row");
        let custom = create(
            &service,
            params(workspace_id, "Custom active duplicate", "human_review"),
        )
        .await
        .expect("custom status issue");
        match create(
            &service,
            params(workspace_id, "custom active duplicate", "human_review"),
        )
        .await
        .expect_err("custom active category must block")
        {
            IssueCreateError::ActiveDuplicate {
                duplicate: Some(found),
            } => assert_eq!(found.id, custom.id),
            other => panic!("unexpected custom duplicate result: {other:?}"),
        }

        // Duplicate identity is workspace-scoped and applies across every
        // active status, not just equal status keys. A custom status inherits
        // the same active category semantics as its built-in category.
        let active_identity = create(
            &service,
            params(workspace_id, "Cross status identity", "todo"),
        )
        .await
        .expect("cross-status issue");
        match create(
            &service,
            params(workspace_id, "cross status identity", "in_progress"),
        )
        .await
        .expect_err("active duplicate must span active status columns")
        {
            IssueCreateError::ActiveDuplicate {
                duplicate: Some(found),
            } => {
                assert_eq!(found.id, active_identity.id)
            }
            other => panic!("unexpected cross-status duplicate result: {other:?}"),
        }

        patchbay_db::queries::issue_status::create_issue_status_entry(
            &pool,
            workspace_id,
            "completed_custom",
            "Completed Custom",
            "",
            "done",
            "#8b82f6",
        )
        .await
        .expect("create closed custom status")
        .expect("closed custom status row");
        create(
            &service,
            params(workspace_id, "Closed custom identity", "completed_custom"),
        )
        .await
        .expect("custom done issue");
        create(
            &service,
            params(workspace_id, "closed custom identity", "done"),
        )
        .await
        .expect("done category does not block duplicate");

        let prior_done_top: f64 = sqlx::query_scalar(
            "SELECT COALESCE(MIN(position), 0) FROM issue \
             WHERE workspace_id = $1 AND status = 'done'",
        )
        .bind(workspace_id)
        .fetch_one(&pool)
        .await
        .expect("load current done column top");
        let done = create(&service, params(workspace_id, "Done column", "done"))
            .await
            .expect("next issue in done column");
        assert_eq!(done.position, prior_done_top - 1.0);

        let project_id: Uuid = sqlx::query_scalar(
            "INSERT INTO project (workspace_id, title) VALUES ($1, 'Scoped') RETURNING id",
        )
        .bind(workspace_id)
        .fetch_one(&pool)
        .await
        .expect("create project");
        let mut project_issue = params(workspace_id, "Scoped identity", "todo");
        project_issue.project_id = Some(project_id);
        create(&service, project_issue)
            .await
            .expect("project-scoped issue");
        create(&service, params(workspace_id, "Scoped identity", "todo"))
            .await
            .expect("root and project identities are independent");

        let parent = create(&service, params(workspace_id, "Parent", "todo"))
            .await
            .expect("parent issue");
        let mut child = params(workspace_id, "Child identity", "todo");
        child.parent_issue_id = Some(parent.id);
        create(&service, child).await.expect("parent-scoped issue");
        create(&service, params(workspace_id, "Child identity", "todo"))
            .await
            .expect("root and child identities are independent");

        let other_workspace = workspace(&pool).await;
        let other = create(
            &service,
            params(other_workspace, "duplicate title", "in_progress"),
        )
        .await
        .expect("duplicate identity is workspace scoped");
        assert_eq!(other.position, -1.0);

        cleanup(&pool, other_workspace).await;
        cleanup(&pool, workspace_id).await;
    }

    #[tokio::test]
    async fn production_create_advisory_lock_serializes_same_identity() {
        let pool = required_pool().await;
        let workspace_id = workspace(&pool).await;
        let issue_service = service(&pool);

        // Hold the row updated by increment_issue_counter. The first create
        // reaches it only after acquiring the duplicate advisory lock; the
        // second create must therefore wait at the duplicate lock. If the
        // production advisory lock is removed, both pass the lookup before
        // this row is released and both insert, making this test fail.
        let mut blocker = pool.begin().await.expect("workspace row blocker");
        sqlx::query("SELECT id FROM workspace WHERE id = $1 FOR UPDATE")
            .bind(workspace_id)
            .fetch_one(&mut *blocker)
            .await
            .expect("lock workspace counter row");

        let first_service = issue_service.clone();
        let first = tokio::spawn(async move {
            let mut first_params = params(workspace_id, "Concurrent identity", "todo");
            // The duplicate override still has to take the transaction-scoped
            // advisory lock; otherwise a normal create can pass its lookup
            // while this transaction is blocked on the counter row.
            first_params.allow_duplicate = true;
            create(&first_service, first_params).await
        });
        wait_for_duplicate_lock(&pool, workspace_id, "Concurrent identity").await;
        let waiter_name = format!("patchbay-duplicate-contract-{}", Uuid::now_v7());
        let waiter_pool = tagged_pool(&waiter_name).await;
        let second_service = service(&waiter_pool);
        let second = tokio::spawn(async move {
            create(
                &second_service,
                params(workspace_id, "  concurrent   IDENTITY ", "todo"),
            )
            .await
        });
        wait_for_tagged_advisory_wait(&pool, &waiter_name).await;
        blocker.commit().await.expect("release workspace counter");

        let results = [
            first.await.expect("first task"),
            second.await.expect("second task"),
        ];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(IssueCreateError::ActiveDuplicate { .. })))
                .count(),
            1
        );
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM issue WHERE workspace_id = $1 AND lower(btrim(regexp_replace(title, '[[:space:]]+', ' ', 'g'))) = 'concurrent identity'",
        )
        .bind(workspace_id)
        .fetch_one(&pool)
        .await
        .expect("count concurrent issues");
        assert_eq!(count, 1);

        cleanup(&pool, workspace_id).await;
    }

    #[tokio::test]
    async fn recent_autopilot_guard_preserves_scope_window_and_active_semantics() {
        let pool = required_pool().await;
        let workspace_id = workspace(&pool).await;
        let agent_id: Uuid = sqlx::query_scalar(
            "INSERT INTO agent (workspace_id, name, runtime_mode) VALUES ($1, 'contract agent', 'local') RETURNING id",
        )
        .bind(workspace_id)
        .fetch_one(&pool)
        .await
        .expect("create agent");
        let autopilot_id: Uuid = sqlx::query_scalar(
            "INSERT INTO autopilot (workspace_id, title, assignee_type, assignee_id, execution_mode, created_by_type, created_by_id) VALUES ($1, 'contract autopilot', 'agent', $2, 'create_issue', 'member', $3) RETURNING id",
        )
        .bind(workspace_id)
        .bind(agent_id)
        .bind(Uuid::now_v7())
        .fetch_one(&pool)
        .await
        .expect("create autopilot");
        let issue_id: Uuid = sqlx::query_scalar(
            "INSERT INTO issue (workspace_id, title, status, priority, creator_type, creator_id, number, position, origin_type, origin_id) VALUES ($1, '  Recurring\tWork  ', 'todo', 'none', 'agent', $2, 1, -1, 'autopilot', $3) RETURNING id",
        )
        .bind(workspace_id)
        .bind(agent_id)
        .bind(autopilot_id)
        .fetch_one(&pool)
        .await
        .expect("create autopilot issue");
        sqlx::query(
            "INSERT INTO autopilot_run (autopilot_id, source, status, issue_id) VALUES ($1, 'manual', 'running', $2)",
        )
        .bind(autopilot_id)
        .bind(issue_id)
        .execute(&pool)
        .await
        .expect("create active run");

        let mut tx = pool.begin().await.expect("guard transaction");
        let (duplicate, found) = crate::issue_guard::lock_and_find_recent_autopilot_duplicate(
            &mut tx,
            workspace_id,
            Some(autopilot_id),
            None,
            "recurring work",
            chrono::Duration::hours(1),
        )
        .await
        .expect("recent guard");
        assert!(found);
        assert_eq!(duplicate.expect("recent duplicate").id, issue_id);
        tx.rollback().await.expect("release guard lock");

        for (autopilot, title, window) in [
            (None, "recurring work", chrono::Duration::hours(1)),
            (Some(autopilot_id), "   ", chrono::Duration::hours(1)),
            (
                Some(autopilot_id),
                "recurring work",
                chrono::Duration::zero(),
            ),
        ] {
            let mut tx = pool.begin().await.expect("no-op transaction");
            let (_, found) = crate::issue_guard::lock_and_find_recent_autopilot_duplicate(
                &mut tx,
                workspace_id,
                autopilot,
                None,
                title,
                window,
            )
            .await
            .expect("recent guard no-op");
            assert!(!found);
            tx.rollback().await.expect("release no-op transaction");
        }

        sqlx::query("UPDATE issue SET created_at = now() - interval '2 hours' WHERE id = $1")
            .bind(issue_id)
            .execute(&pool)
            .await
            .expect("age issue");
        let mut tx = pool.begin().await.expect("expired transaction");
        let (_, found) = crate::issue_guard::lock_and_find_recent_autopilot_duplicate(
            &mut tx,
            workspace_id,
            Some(autopilot_id),
            None,
            "recurring work",
            chrono::Duration::hours(1),
        )
        .await
        .expect("expired recent guard");
        assert!(!found);
        tx.rollback().await.expect("release expired guard lock");

        sqlx::query("DELETE FROM autopilot_run WHERE autopilot_id = $1")
            .bind(autopilot_id)
            .execute(&pool)
            .await
            .expect("delete autopilot runs");
        sqlx::query("DELETE FROM issue WHERE workspace_id = $1")
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("delete autopilot issues");
        sqlx::query("DELETE FROM autopilot WHERE id = $1")
            .bind(autopilot_id)
            .execute(&pool)
            .await
            .expect("delete autopilot");
        sqlx::query("DELETE FROM agent WHERE id = $1")
            .bind(agent_id)
            .execute(&pool)
            .await
            .expect("delete agent");
        sqlx::query("DELETE FROM workspace WHERE id = $1")
            .bind(workspace_id)
            .execute(&pool)
            .await
            .expect("delete workspace");
    }
}
