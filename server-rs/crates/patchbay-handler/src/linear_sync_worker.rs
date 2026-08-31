//! Durable Linear pull/import worker.
//!
//! Linear Webhooks are change notifications, not issue snapshots. This worker
//! claims the durable Inbox with a PostgreSQL lease, fetches the complete
//! remote Issue, and applies only the inbound Project Binding direction.
//! Outbound mutations intentionally do not exist in this phase.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use patchbay_db::models::{
    Issue, LinearConnection, LinearIssueLink, LinearProjectBinding, LinearSyncInbox,
};
use patchbay_db::queries::{issue as issue_q, linear as linear_q};
use patchbay_service::issue_service::{
    ExternalIssueError, ExternalIssuePatch, ExternalSource, IssueCommand, IssueCreateError,
    IssueCreateOpts, IssueCreateParams,
};
use serde_json::{json, Value};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::linear::{LinearRemoteIssue, LinearTokenError, LinearTokenManager};
use crate::state::HandlerState;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const WORKER_COUNT: usize = 4;
const LEASE_SECONDS: i64 = 60;
const MAX_BACKOFF_SECONDS: i64 = 15 * 60;
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
enum SyncError {
    Retry(anyhow::Error),
    Permanent(anyhow::Error),
}

impl SyncError {
    fn retry(error: impl Into<anyhow::Error>) -> Self {
        Self::Retry(error.into())
    }

    fn permanent(error: impl Into<anyhow::Error>) -> Self {
        Self::Permanent(error.into())
    }

    fn message(&self) -> String {
        match self {
            Self::Retry(error) | Self::Permanent(error) => error.to_string(),
        }
    }
}

/// A supervisor-owned Linear Inbox worker. `HandlerState` is cloned into the
/// worker so the domain service, event bus, feature gates, and token manager
/// all remain the same instances used by HTTP routes.
pub struct LinearSyncWorker {
    state: HandlerState,
    notify: Arc<Notify>,
    worker_id: String,
}

impl LinearSyncWorker {
    pub fn new(state: HandlerState, notify: Arc<Notify>) -> Arc<Self> {
        Arc::new(Self {
            state,
            notify,
            worker_id: format!("linear-sync-{}", Uuid::now_v7()),
        })
    }

    pub fn start(self: Arc<Self>, cancel: CancellationToken) -> LinearSyncRuntime {
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { self.run_workers(task_cancel).await });
        LinearSyncRuntime {
            cancel,
            task: Some(task),
        }
    }

    async fn run_workers(self: Arc<Self>, cancel: CancellationToken) {
        let mut workers = tokio::task::JoinSet::new();
        for index in 0..WORKER_COUNT {
            let worker = self.clone();
            let worker_cancel = cancel.child_token();
            workers.spawn(async move { worker.run_loop(worker_cancel, index).await });
        }
        while let Some(result) = workers.join_next().await {
            if let Err(error) = result {
                tracing::error!(%error, "Linear sync worker stopped unexpectedly");
            }
        }
    }

    async fn run_loop(&self, cancel: CancellationToken, index: usize) {
        let worker_id = format!("{}-{index}", self.worker_id);
        loop {
            let processed = tokio::select! {
                _ = cancel.cancelled() => return,
                result = self.process_next(&worker_id) => result,
            };
            match processed {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => tracing::error!(%error, worker_id, "Linear sync worker failed"),
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                () = self.notify.notified() => {},
                () = tokio::time::sleep(POLL_INTERVAL) => {},
            }
        }
    }

    /// Processes at most one Inbox row. This public seam is used by focused
    /// worker tests and by production's supervisor; PostgreSQL remains the
    /// source of truth for claim ownership.
    pub async fn process_next(&self, worker_id: &str) -> anyhow::Result<bool> {
        if !self.state.linear_pull_import_enabled_for_any_workspace() {
            return Ok(false);
        }
        let workspace_filter = self.state.linear_pull_import_workspace_filter();
        let _ = linear_q::dead_letter_exhausted_sync_inbox(
            &self.state.pool,
            workspace_filter.as_deref(),
        )
        .await?;
        let Some(row) = linear_q::claim_sync_inbox(
            &self.state.pool,
            worker_id,
            1,
            LEASE_SECONDS,
            workspace_filter.as_deref(),
        )
        .await?
        .into_iter()
        .next()
        else {
            return Ok(false);
        };

        // Initial imports fetch a complete remote project and can outlive the
        // claim lease. Renew independently of the business operation; the
        // completion/retry SQL still verifies the lease owner and expiry.
        let renew_cancel = CancellationToken::new();
        let renew_task = {
            let pool = self.state.pool.clone();
            let inbox_id = row.id;
            let worker_id = worker_id.to_string();
            let cancel = renew_cancel.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(20));
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = interval.tick() => {
                            match linear_q::renew_claimed_sync_inbox(
                                &pool,
                                inbox_id,
                                &worker_id,
                                LEASE_SECONDS,
                            ).await {
                                Ok(true) => {}
                                Ok(false) => {
                                    tracing::warn!(
                                        inbox_id = %inbox_id,
                                        worker_id,
                                        "Linear Inbox lease renewal lost ownership"
                                    );
                                    return;
                                }
                                Err(error) => tracing::warn!(
                                    inbox_id = %inbox_id,
                                    worker_id,
                                    %error,
                                    "Linear Inbox lease renewal failed"
                                ),
                            }
                        }
                    }
                }
            })
        };
        let result = self.process_row(&row).await;
        renew_cancel.cancel();
        let _ = renew_task.await;

        match result {
            Ok(()) => {
                let owned =
                    linear_q::complete_claimed_sync_inbox(&self.state.pool, row.id, worker_id)
                        .await?;
                if !owned {
                    tracing::warn!(
                        inbox_id = %row.id,
                        worker_id,
                        "Linear Inbox completion lost its lease"
                    );
                }
            }
            Err(error) => {
                let message = error.message();
                let permanent = matches!(error, SyncError::Permanent(_));
                let exhausted = row.attempts >= row.max_attempts;
                if permanent || exhausted {
                    let owned = linear_q::dead_letter_claimed_sync_inbox(
                        &self.state.pool,
                        row.id,
                        worker_id,
                        &message,
                    )
                    .await?;
                    if !owned {
                        tracing::warn!(
                            inbox_id = %row.id,
                            worker_id,
                            "Linear Inbox dead-letter lost its lease"
                        );
                    }
                } else {
                    let available_at = Utc::now() + retry_delay(row.attempts);
                    let owned = linear_q::retry_claimed_sync_inbox(
                        &self.state.pool,
                        row.id,
                        worker_id,
                        available_at,
                        &message,
                    )
                    .await?;
                    if !owned {
                        tracing::warn!(
                            inbox_id = %row.id,
                            worker_id,
                            "Linear Inbox retry lost its lease"
                        );
                    }
                }
                tracing::warn!(
                    inbox_id = %row.id,
                    attempts = row.attempts,
                    permanent,
                    error = %message,
                    "Linear Inbox item failed"
                );
            }
        }
        Ok(true)
    }

    async fn process_row(&self, row: &LinearSyncInbox) -> Result<(), SyncError> {
        let connection =
            linear_q::get_connection_by_id_unscoped(&self.state.pool, row.connection_id)
                .await
                .map_err(SyncError::retry)?
                .ok_or_else(|| {
                    SyncError::permanent(anyhow::anyhow!("Linear connection not found"))
                })?;
        if connection.status != "active" {
            return Err(SyncError::permanent(anyhow::anyhow!(
                "Linear connection is not active"
            )));
        }
        if !self
            .state
            .linear_pull_import_enabled(connection.workspace_id)
        {
            return Err(SyncError::permanent(anyhow::anyhow!(
                "Linear pull/import is not enabled for this workspace"
            )));
        }

        if row.event_type == "linear.initial_import"
            || row.payload.get("kind").and_then(Value::as_str) == Some("initial_import")
        {
            return self.process_initial_import(row, &connection).await;
        }

        let Some(linear_issue_id) = extract_issue_id(&row.payload) else {
            // Organization/project events are valid Webhook deliveries but do
            // not identify an Issue. They are acknowledged and need no retry.
            return Ok(());
        };
        let source_event_id = format!("linear-delivery:{}", row.delivery_id);
        let event_timestamp_ms = extract_event_timestamp_ms(&row.payload);
        let existing_link = linear_q::find_linear_issue_link(
            &self.state.pool,
            connection.workspace_id,
            connection.id,
            &linear_issue_id,
        )
        .await
        .map_err(SyncError::retry)?;
        if is_out_of_order(existing_link.as_ref(), event_timestamp_ms) {
            return Ok(());
        }

        let action = row.payload.get("action").and_then(Value::as_str);
        let is_removed = matches!(action, Some("remove" | "delete"))
            || row.event_type.to_ascii_lowercase().contains("remove")
            || row.event_type.to_ascii_lowercase().contains("delete");
        if is_removed {
            return self
                .apply_remote_removal(
                    &connection,
                    existing_link,
                    &linear_issue_id,
                    &source_event_id,
                    event_timestamp_ms,
                )
                .await;
        }

        let manager = LinearTokenManager::from_state(&self.state)
            .map_err(|error| classify_token_error(error, "create Linear token manager"))?;
        let remote = manager
            .fetch_issue(connection.id, &linear_issue_id)
            .await
            .map_err(|error| classify_token_error(error, "fetch Linear issue"))?;
        let Some(remote) = remote else {
            return self
                .apply_remote_removal(
                    &connection,
                    existing_link,
                    &linear_issue_id,
                    &source_event_id,
                    event_timestamp_ms,
                )
                .await;
        };
        self.apply_remote_issue(
            &connection,
            remote,
            existing_link,
            &source_event_id,
            event_timestamp_ms,
        )
        .await
    }

    async fn process_initial_import(
        &self,
        row: &LinearSyncInbox,
        connection: &LinearConnection,
    ) -> Result<(), SyncError> {
        let binding_id = row
            .payload
            .get("binding_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                SyncError::permanent(anyhow::anyhow!("initial import binding is missing"))
            })?
            .parse::<Uuid>()
            .map_err(|_| {
                SyncError::permanent(anyhow::anyhow!("initial import binding is invalid"))
            })?;
        let binding =
            linear_q::get_project_binding(&self.state.pool, connection.workspace_id, binding_id)
                .await
                .map_err(SyncError::retry)?
                .ok_or_else(|| {
                    SyncError::permanent(anyhow::anyhow!("initial import binding not found"))
                })?;
        if !inbound_enabled(&binding) {
            return Ok(());
        }
        let manager = LinearTokenManager::from_state(&self.state)
            .map_err(|error| classify_token_error(error, "create Linear token manager"))?;
        let issues = manager
            .list_project_issues(connection.id, &binding.linear_project_id)
            .await
            .map_err(|error| classify_token_error(error, "list Linear project issues"))?;
        let source_prefix = row
            .payload
            .get("source_event_id")
            .and_then(Value::as_str)
            .unwrap_or(&row.delivery_id)
            .to_string();
        for remote in issues {
            let issue_id = remote.id.clone();
            let existing_link = linear_q::find_linear_issue_link(
                &self.state.pool,
                connection.workspace_id,
                connection.id,
                &issue_id,
            )
            .await
            .map_err(SyncError::retry)?;
            match self
                .apply_remote_issue(
                    connection,
                    remote,
                    existing_link,
                    &format!("{source_prefix}:{issue_id}"),
                    None,
                )
                .await
            {
                Ok(()) => {}
                Err(SyncError::Permanent(error)) => {
                    // One malformed or unmappable Issue must not abort an
                    // otherwise valid project import. The bad item is
                    // recorded in the Inbox result and the remaining remote
                    // snapshot is still imported.
                    tracing::warn!(%error, issue_id, "skipping permanently invalid Linear import item");
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    async fn apply_remote_issue(
        &self,
        connection: &LinearConnection,
        remote: LinearRemoteIssue,
        existing_link: Option<LinearIssueLink>,
        source_event_id: &str,
        event_timestamp_ms: Option<i64>,
    ) -> Result<(), SyncError> {
        let linear_project_id = remote
            .project
            .as_ref()
            .map(|project| project.id.trim())
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                SyncError::permanent(anyhow::anyhow!(
                    "Linear issue has no project and is outside project bindings"
                ))
            })?;
        let (binding, needs_rebind) = if let Some(link) = existing_link.as_ref() {
            let current_binding = linear_q::get_project_binding(
                &self.state.pool,
                connection.workspace_id,
                link.binding_id,
            )
            .await
            .map_err(SyncError::retry)?;
            if current_binding
                .as_ref()
                .is_some_and(|binding| binding.linear_project_id == linear_project_id)
            {
                (current_binding, false)
            } else {
                let destination = linear_q::get_binding_for_remote_project(
                    &self.state.pool,
                    connection.workspace_id,
                    connection.id,
                    linear_project_id,
                )
                .await
                .map_err(SyncError::retry)?;
                let destination_can_receive =
                    destination.as_ref().map(inbound_enabled).unwrap_or(true);
                if destination_can_receive {
                    (destination, true)
                } else {
                    let _ = linear_q::mark_linear_issue_link_deleted(
                        &self.state.pool,
                        link.id,
                        connection.workspace_id,
                    )
                    .await
                    .map_err(SyncError::retry)?;
                    return Ok(());
                }
            }
        } else {
            (
                linear_q::get_binding_for_remote_project(
                    &self.state.pool,
                    connection.workspace_id,
                    connection.id,
                    linear_project_id,
                )
                .await
                .map_err(SyncError::retry)?,
                false,
            )
        };
        let Some(binding) = binding else {
            // A connected organization can have many projects. Unbound Issues
            // are intentionally ignored rather than imported into a guessed
            // Patchbay Project.
            return Ok(());
        };
        if !inbound_enabled(&binding) {
            return Ok(());
        }
        let remote_uuid = remote
            .id
            .parse::<Uuid>()
            .map_err(|_| SyncError::permanent(anyhow::anyhow!("Linear issue id is not a UUID")))?;
        let mapped_status = map_remote_status(&binding, remote.state.as_ref())?;
        let priority = map_remote_priority(remote.priority)?;
        let due_date = remote
            .due_date
            .as_deref()
            .map(|date| {
                NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| {
                    SyncError::permanent(anyhow::anyhow!("Linear due date is invalid"))
                })
            })
            .transpose()?;
        let remote_updated_at = parse_remote_timestamp(&remote.updated_at)?;
        let event_timestamp_ms = event_timestamp_ms.or_else(|| {
            Some(remote_updated_at.timestamp_millis())
        });
        if is_out_of_order(existing_link.as_ref(), event_timestamp_ms) {
            return Ok(());
        }
        let snapshot = serde_json::to_value(&remote).map_err(|error| {
            SyncError::permanent(anyhow::anyhow!("serialize Linear snapshot: {error}"))
        })?;
        let linked_issue = if let Some(link) = existing_link.as_ref() {
            issue_q::get_issue_in_workspace(
                &self.state.pool,
                link.patchbay_issue_id,
                connection.workspace_id,
            )
            .await
            .map_err(SyncError::retry)?
        } else {
            None
        };
        let mapped_category = patchbay_service::issue_status::effective(
            &self.state.pool,
            connection.workspace_id,
            &mapped_status,
        )
        .await;
        let import_status = if import_status_is_inadmissible(
            &mapped_category,
            linked_issue.as_ref(),
        ) {
            tracing::warn!(
                issue_id = %remote.id,
                mapped_status,
                "parking imported Linear issue until its Patchbay assignments are admissible"
            );
            patchbay_service::issue_status::BACKLOG.to_string()
        } else {
            mapped_status.clone()
        };
        let remote_patch = ExternalIssuePatch {
            title: Some(remote.title.clone()),
            description: Some(remote.description.clone()),
            status: Some(import_status),
            priority: Some(priority.clone()),
            due_date: Some(due_date),
            project_id: Some(Some(binding.patchbay_project_id)),
        };
        let issue = if let Some(link) = existing_link.as_ref() {
            self.state
                .issues
                .apply_external_patch(
                    connection.workspace_id,
                    link.patchbay_issue_id,
                    IssueCommand::ApplyExternalPatch {
                        source: ExternalSource::Linear,
                        source_event_id: source_event_id.to_string(),
                        expected_revision: None,
                        suppress_external_outbox: true,
                        patch: remote_patch.clone(),
                    },
                )
                .await
                .map_err(classify_external_issue_error)?
        } else {
            let existing_issue = issue_q::get_issue_by_origin(
                &self.state.pool,
                connection.workspace_id,
                Some("linear"),
                remote_uuid,
            )
            .await
            .map_err(SyncError::retry)?;
            if let Some(issue) = existing_issue {
                self.state
                    .issues
                    .apply_external_patch(
                        connection.workspace_id,
                        issue.id,
                        IssueCommand::ApplyExternalPatch {
                            source: ExternalSource::Linear,
                            source_event_id: source_event_id.to_string(),
                            expected_revision: None,
                            suppress_external_outbox: true,
                            patch: remote_patch,
                        },
                    )
                    .await
                    .map_err(classify_external_issue_error)?
            } else {
                if remote.title.trim().is_empty() || remote.identifier.trim().is_empty() {
                    return Err(SyncError::permanent(anyhow::anyhow!(
                        "Linear issue identity or title is empty"
                    )));
                }
                let result = self
                    .state
                    .issues
                    .create(
                        IssueCreateParams {
                            workspace_id: connection.workspace_id,
                            title: remote.title.clone(),
                            description: remote.description.clone(),
                            status: remote_patch
                                .status
                                .clone()
                                .unwrap_or_else(|| patchbay_service::issue_status::BACKLOG.to_string()),
                            priority,
                            creator_type: "member".to_string(),
                            creator_id: connection.created_by_id,
                            project_id: Some(binding.patchbay_project_id),
                            due_date,
                            origin_type: Some("linear".to_string()),
                            origin_id: Some(remote_uuid),
                            allow_duplicate: true,
                            ..IssueCreateParams::default()
                        },
                        IssueCreateOpts {
                            actor_id: connection.created_by_id.to_string(),
                            analytics_agent_id: String::new(),
                            platform: "linear".to_string(),
                            ..IssueCreateOpts::default()
                        },
                    )
                    .await;
                match result {
                    Ok(result) => result.issue.ok_or_else(|| {
                        SyncError::retry(anyhow::anyhow!("Linear import created no Patchbay issue"))
                    })?,
                    Err(IssueCreateError::Sql(error)) => {
                        // A concurrent delivery may win the linear-origin
                        // unique index between the pre-check and create. The
                        // committed winner is the recovery record; never
                        // turn that race into a second local Issue.
                        if let Some(issue) = issue_q::get_issue_by_origin(
                            &self.state.pool,
                            connection.workspace_id,
                            Some("linear"),
                            remote_uuid,
                        )
                        .await
                        .map_err(SyncError::retry)?
                        {
                            issue
                        } else {
                            return Err(SyncError::retry(anyhow::anyhow!(error)));
                        }
                    }
                    Err(
                        error @ (IssueCreateError::StatusUnavailable
                        | IssueCreateError::ActiveExecutorRequired
                        | IssueCreateError::ReviewReviewerRequired
                        | IssueCreateError::ParentIssueNotFound
                        | IssueCreateError::ProjectNotFound
                        | IssueCreateError::LabelNotFound),
                    ) => return Err(SyncError::permanent(anyhow::anyhow!(error))),
                    Err(error) => return Err(SyncError::retry(anyhow::anyhow!(error))),
                }
            }
        };

        let mut link = if let Some(link) = existing_link {
            link
        } else if let Some(link) = linear_q::find_linear_issue_link(
            &self.state.pool,
            connection.workspace_id,
            connection.id,
            &remote.id,
        )
        .await
        .map_err(SyncError::retry)?
        {
            link
        } else {
            let created = linear_q::create_linear_issue_link(
                &self.state.pool,
                &linear_q::LinearIssueLinkInput {
                    id: Uuid::now_v7(),
                    workspace_id: connection.workspace_id,
                    binding_id: binding.id,
                    patchbay_issue_id: issue.id,
                    linear_issue_id: &remote.id,
                    linear_identifier: &remote.identifier,
                    last_common_snapshot: &snapshot,
                    remote_updated_at,
                    last_remote_event_at_ms: event_timestamp_ms,
                    last_remote_event_id: Some(source_event_id),
                },
            )
            .await
            .map_err(SyncError::retry)?;
            if let Some(created) = created {
                created
            } else {
                linear_q::find_linear_issue_link(
                    &self.state.pool,
                    connection.workspace_id,
                    connection.id,
                    &remote.id,
                )
                .await
                .map_err(SyncError::retry)?
                .ok_or_else(|| {
                    SyncError::retry(anyhow::anyhow!(
                        "Linear Issue Link insert raced without a visible link"
                    ))
                })?
            }
        };
        if needs_rebind && link.binding_id != binding.id {
            let rebound = linear_q::rebind_linear_issue_link(
                &self.state.pool,
                link.id,
                connection.workspace_id,
                binding.id,
            )
            .await
            .map_err(SyncError::retry)?;
            if !rebound {
                return Err(SyncError::retry(anyhow::anyhow!(
                    "Linear Issue Link disappeared during project rebind"
                )));
            }
            link.binding_id = binding.id;
        }
        let last_event_at_ms = event_timestamp_ms.or(link.last_remote_event_at_ms);
        let last_event_id = event_timestamp_ms
            .map(|_| source_event_id)
            .or(link.last_remote_event_id.as_deref());
        let updated = linear_q::update_linear_issue_link(
            &self.state.pool,
            link.id,
            connection.workspace_id,
            &snapshot,
            remote_updated_at,
            last_event_at_ms,
            last_event_id,
            "active",
        )
        .await
        .map_err(SyncError::retry)?;
        if !updated {
            let current = linear_q::find_linear_issue_link(
                &self.state.pool,
                connection.workspace_id,
                connection.id,
                &remote.id,
            )
            .await
            .map_err(SyncError::retry)?;
            if is_out_of_order(current.as_ref(), event_timestamp_ms) {
                return Ok(());
            }
            return Err(SyncError::retry(anyhow::anyhow!(
                "Linear Issue Link disappeared during update"
            )));
        }
        Ok(())
    }

    async fn apply_remote_removal(
        &self,
        connection: &LinearConnection,
        existing_link: Option<LinearIssueLink>,
        linear_issue_id: &str,
        source_event_id: &str,
        event_timestamp_ms: Option<i64>,
    ) -> Result<(), SyncError> {
        let Some(link) = existing_link.or(linear_q::find_linear_issue_link(
            &self.state.pool,
            connection.workspace_id,
            connection.id,
            linear_issue_id,
        )
        .await
        .map_err(SyncError::retry)?) else {
            return Ok(());
        };
        let Some(binding) = linear_q::get_project_binding(
            &self.state.pool,
            connection.workspace_id,
            link.binding_id,
        )
        .await
        .map_err(SyncError::retry)?
        else {
            return Ok(());
        };
        if !inbound_enabled(&binding) {
            return Ok(());
        }
        let _ = self
            .state
            .issues
            .apply_external_patch(
                connection.workspace_id,
                link.patchbay_issue_id,
                IssueCommand::ApplyExternalPatch {
                    source: ExternalSource::Linear,
                    source_event_id: source_event_id.to_string(),
                    expected_revision: None,
                    suppress_external_outbox: true,
                    patch: ExternalIssuePatch {
                        status: Some("cancelled".to_string()),
                        ..ExternalIssuePatch::default()
                    },
                },
            )
            .await
            .map_err(classify_external_issue_error)?;
        let snapshot = json!({
            "linear_issue_id": linear_issue_id,
            "deleted": true,
        });
        let event_at = event_timestamp_ms.or(link.last_remote_event_at_ms);
        let event_id = event_timestamp_ms
            .map(|_| source_event_id)
            .or(link.last_remote_event_id.as_deref());
        let updated = linear_q::update_linear_issue_link(
            &self.state.pool,
            link.id,
            connection.workspace_id,
            &snapshot,
            link.remote_updated_at,
            event_at,
            event_id,
            "deleted",
        )
        .await
        .map_err(SyncError::retry)?;
        if !updated {
            let current = linear_q::find_linear_issue_link(
                &self.state.pool,
                connection.workspace_id,
                connection.id,
                linear_issue_id,
            )
            .await
            .map_err(SyncError::retry)?;
            if is_out_of_order(current.as_ref(), event_timestamp_ms) {
                return Ok(());
            }
            return Err(SyncError::retry(anyhow::anyhow!(
                "Linear deletion link update lost its row"
            )));
        }
        Ok(())
    }
}

fn retry_delay(attempts: i32) -> chrono::Duration {
    let exponent = attempts.saturating_sub(1).clamp(0, 8) as u32;
    let seconds = (5_i64)
        .saturating_mul(1_i64 << exponent)
        .min(MAX_BACKOFF_SECONDS);
    chrono::Duration::seconds(seconds)
}

fn inbound_enabled(binding: &LinearProjectBinding) -> bool {
    binding.status == "active" && matches!(binding.sync_mode.as_str(), "import" | "two_way")
}

fn import_status_is_inadmissible(category: &str, issue: Option<&Issue>) -> bool {
    let Some(issue) = issue else {
        return patchbay_service::issue_status::requires_executor(category)
            || patchbay_service::issue_status::requires_reviewer(category);
    };
    let has_executor = issue.executor_type.is_some() && issue.executor_id.is_some();
    let has_reviewer = issue.reviewer_type.is_some() && issue.reviewer_id.is_some();
    (patchbay_service::issue_status::requires_executor(category) && !has_executor)
        || (patchbay_service::issue_status::requires_reviewer(category)
            && (!has_reviewer || issue.reviewer_id == issue.executor_id))
}

fn extract_issue_id(payload: &Value) -> Option<String> {
    [
        payload.get("data").and_then(|data| data.get("id")),
        payload.get("issue").and_then(|issue| issue.get("id")),
        payload.get("linearIssueId"),
        payload.get("id"),
    ]
    .into_iter()
    .flatten()
    .and_then(Value::as_str)
    .map(str::trim)
    .filter(|id| !id.is_empty())
    .map(str::to_string)
}

fn extract_event_timestamp_ms(payload: &Value) -> Option<i64> {
    payload
        .get("webhookTimestamp")
        .and_then(Value::as_i64)
        .or_else(|| {
            payload
                .get("webhookTimestamp")
                .and_then(Value::as_str)
                .and_then(|value| value.parse().ok())
        })
}

fn is_out_of_order(link: Option<&LinearIssueLink>, event_timestamp_ms: Option<i64>) -> bool {
    match (
        link.and_then(|link| link.last_remote_event_at_ms),
        event_timestamp_ms,
    ) {
        (Some(previous), Some(current)) => current <= previous,
        _ => false,
    }
}

fn map_remote_priority(priority: i64) -> Result<String, SyncError> {
    let mapped = match priority {
        0 => "none",
        1 => "urgent",
        2 => "high",
        3 => "medium",
        4 => "low",
        _ => {
            return Err(SyncError::permanent(anyhow::anyhow!(
                "Linear issue priority is outside the supported range"
            )))
        }
    };
    Ok(mapped.to_string())
}

fn map_remote_status(
    binding: &LinearProjectBinding,
    state: Option<&crate::linear::LinearRemoteState>,
) -> Result<String, SyncError> {
    let state = state.ok_or_else(|| {
        SyncError::permanent(anyhow::anyhow!("Linear issue has no workflow state"))
    })?;
    if let Some(value) = binding
        .status_mapping
        .as_object()
        .and_then(|mapping| mapping.get(&state.id))
    {
        return value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                SyncError::permanent(anyhow::anyhow!(
                    "Linear status mapping contains an invalid target"
                ))
            });
    }
    let fallback = match state.state_type.as_str() {
        "backlog" => "backlog",
        "unstarted" => "todo",
        "started" => "in_progress",
        "completed" => "done",
        "canceled" | "cancelled" => "cancelled",
        _ => {
            return Err(SyncError::permanent(anyhow::anyhow!(
                "Linear workflow state has no status mapping"
            )))
        }
    };
    Ok(fallback.to_string())
}

fn parse_remote_timestamp(raw: &str) -> Result<DateTime<Utc>, SyncError> {
    DateTime::parse_from_rfc3339(raw)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| SyncError::permanent(anyhow::anyhow!("Linear updatedAt is invalid")))
}

fn classify_token_error(error: LinearTokenError, context: &str) -> SyncError {
    match error {
        LinearTokenError::InvalidResponse => SyncError::permanent(anyhow::anyhow!(
            "{context}: Linear returned an invalid protocol response"
        )),
        other => SyncError::retry(anyhow::anyhow!("{context}: {other}")),
    }
}

fn classify_external_issue_error(error: ExternalIssueError) -> SyncError {
    let permanent = matches!(
        &error,
        ExternalIssueError::MissingSourceEvent
            | ExternalIssueError::ExternalOutboxNotSuppressed
            | ExternalIssueError::NotFound
            | ExternalIssueError::InvalidStatus
            | ExternalIssueError::InvalidPriority
            | ExternalIssueError::ProjectNotFound
            | ExternalIssueError::ActiveExecutorRequired
            | ExternalIssueError::ReviewReviewerRequired
    );
    let message = error.to_string();
    if permanent {
        SyncError::permanent(anyhow::anyhow!(message))
    } else {
        SyncError::retry(anyhow::anyhow!(message))
    }
}

/// Production-owned root for the Linear worker supervisor and JoinSet.
pub struct LinearSyncRuntime {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl LinearSyncRuntime {
    pub async fn shutdown(mut self, timeout: Duration) -> LinearSyncShutdownOutcome {
        self.cancel.cancel();
        let mut task = self
            .task
            .take()
            .expect("Linear sync runtime always owns a supervisor");
        match tokio::time::timeout(timeout, &mut task).await {
            Ok(Ok(())) => LinearSyncShutdownOutcome::Stopped,
            Ok(Err(_)) => LinearSyncShutdownOutcome::Panicked,
            Err(_) => {
                task.abort();
                let _ = task.await;
                LinearSyncShutdownOutcome::TimedOut
            }
        }
    }
}

impl Drop for LinearSyncRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinearSyncShutdownOutcome {
    Stopped,
    Panicked,
    TimedOut,
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use patchbay_db::models::{LinearIssueLink, LinearProjectBinding};
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        extract_event_timestamp_ms, extract_issue_id, inbound_enabled, is_out_of_order,
        map_remote_priority, map_remote_status, parse_remote_timestamp, retry_delay,
    };
    use crate::linear::LinearRemoteState;

    fn binding(status_mapping: serde_json::Value) -> LinearProjectBinding {
        LinearProjectBinding {
            activated_at: None,
            agent_label_mapping: json!({}),
            connection_id: Uuid::now_v7(),
            patchbay_project_id: Uuid::now_v7(),
            created_at: Utc::now(),
            created_by_id: Uuid::now_v7(),
            id: Uuid::now_v7(),
            initial_source_of_truth: Some("linear".to_string()),
            linear_project_id: "linear-project".to_string(),
            linear_team_id: Some("linear-team".to_string()),
            paused_at: None,
            status: "active".to_string(),
            status_mapping,
            sync_mode: "import".to_string(),
            updated_at: Utc::now(),
            workspace_id: Uuid::now_v7(),
        }
    }

    fn link(last_remote_event_at_ms: Option<i64>) -> LinearIssueLink {
        LinearIssueLink {
            binding_id: Uuid::now_v7(),
            created_at: Utc::now(),
            id: Uuid::now_v7(),
            last_common_snapshot: json!({}),
            last_remote_event_at_ms,
            last_remote_event_id: None,
            linear_identifier: "COR-1".to_string(),
            linear_issue_id: Uuid::now_v7().to_string(),
            patchbay_issue_id: Uuid::now_v7(),
            remote_updated_at: None,
            sync_status: "active".to_string(),
            updated_at: Utc::now(),
            workspace_id: Uuid::now_v7(),
        }
    }

    #[test]
    fn retry_delay_is_exponential_and_bounded() {
        let delays = (1..=10)
            .map(|attempt| retry_delay(attempt).num_seconds())
            .collect::<Vec<_>>();
        assert_eq!(delays, vec![5, 10, 20, 40, 80, 160, 320, 640, 900, 900]);
    }

    #[test]
    fn webhook_issue_and_timestamp_extractors_accept_documented_shapes() {
        let payload = json!({
            "data": {"id": "issue-from-data"},
            "webhookTimestamp": "1700000000123"
        });
        assert_eq!(
            extract_issue_id(&payload).as_deref(),
            Some("issue-from-data")
        );
        assert_eq!(
            extract_event_timestamp_ms(&payload),
            Some(1_700_000_000_123)
        );
        assert_eq!(
            extract_issue_id(&json!({"id": "  direct "})).as_deref(),
            Some("direct")
        );
        assert_eq!(extract_issue_id(&json!({"data": {}})), None);
    }

    #[test]
    fn older_or_equal_webhook_events_are_ignored() {
        let link = link(Some(200));
        assert!(is_out_of_order(Some(&link), Some(199)));
        assert!(is_out_of_order(Some(&link), Some(200)));
        assert!(!is_out_of_order(Some(&link), Some(201)));
        assert!(!is_out_of_order(None, Some(200)));
    }

    #[test]
    fn remote_priority_and_status_mapping_are_explicit() {
        assert_eq!(map_remote_priority(0).unwrap(), "none");
        assert_eq!(map_remote_priority(4).unwrap(), "low");
        assert!(map_remote_priority(5).is_err());

        let fallback_binding = binding(json!({}));
        let started = LinearRemoteState {
            id: "started-state".to_string(),
            name: "In Progress".to_string(),
            state_type: "started".to_string(),
        };
        assert_eq!(
            map_remote_status(&fallback_binding, Some(&started)).unwrap(),
            "in_progress"
        );

        let mapped_binding = binding(json!({"started-state": "todo"}));
        assert_eq!(
            map_remote_status(&mapped_binding, Some(&started)).unwrap(),
            "todo"
        );
    }

    #[test]
    fn inbound_mode_excludes_publish_paused_and_tombstone_bindings() {
        let mut binding = binding(json!({}));
        assert!(inbound_enabled(&binding));
        binding.sync_mode = "publish".to_string();
        assert!(!inbound_enabled(&binding));
        binding.sync_mode = "import".to_string();
        binding.status = "paused".to_string();
        assert!(!inbound_enabled(&binding));
        binding.status = "tombstone".to_string();
        assert!(!inbound_enabled(&binding));
    }

    #[test]
    fn remote_timestamp_requires_rfc3339() {
        assert!(parse_remote_timestamp("2026-08-31T12:00:00Z").is_ok());
        assert!(parse_remote_timestamp("not-a-timestamp").is_err());
    }
}
