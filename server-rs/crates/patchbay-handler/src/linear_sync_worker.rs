//! Durable Linear pull/import/publish worker.
//!
//! Linear Webhooks are change notifications, not issue snapshots. This worker
//! claims the durable Inbox with a PostgreSQL lease, fetches the complete
//! remote Issue, and applies only the inbound Project Binding direction.
//! Outbound mutations are emitted only from the durable Outbox and are gated
//! independently from inbound import.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use patchbay_db::models::{
    Issue, LinearConnection, LinearIssueLink, LinearProjectBinding, LinearSyncInbox,
    LinearSyncOutbox,
};
use patchbay_db::queries::{
    activity, agent as agent_q, issue as issue_q, linear as linear_q,
    linear_agent as linear_agent_q, workspace as workspace_q,
};
use patchbay_service::issue_service::{
    ExternalIssueError, ExternalIssuePatch, ExternalSource, IssueCommand, IssueCreateError,
    IssueCreateOpts, IssueCreateParams,
};
use patchbay_service::task_helpers::has_runnable_successor;
use patchbay_service::task_service::pending_slot_taken_err;
use serde_json::{json, Value};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::linear::{
    strip_patchbay_issue_marker, LinearIssueCreateInput, LinearIssueUpdateInput, LinearRemoteIssue,
    LinearRemoteLabel, LinearRemoteUser, LinearTokenError, LinearTokenManager,
};
use crate::state::HandlerState;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const WORKER_COUNT: usize = 4;
const LEASE_SECONDS: i64 = 60;
const MAX_BACKOFF_SECONDS: i64 = 15 * 60;
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

fn linear_sync_activity_id(
    connection_id: Uuid,
    issue_id: Uuid,
    source_event_id: &str,
    action: &str,
) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("patchbay:linear:activity:{action}:{connection_id}:{issue_id}:{source_event_id}")
            .as_bytes(),
    )
}

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

#[derive(Debug)]
struct LinearAgentSessionEvent {
    session_id: String,
    linear_issue_id: String,
    action: String,
    prompt_context: Option<String>,
    prompt_body: Option<String>,
    requester_user_id: Option<String>,
}

#[derive(Debug)]
struct LinearAgentSessionTerminalEvent {
    session_id: String,
    status: String,
    body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AgentLabelDecision {
    configured: bool,
    agent_id: Option<Uuid>,
}

fn is_agent_session_event(row: &LinearSyncInbox) -> bool {
    row.event_type
        .to_ascii_lowercase()
        .replace('_', "")
        .contains("agentsession")
        || row.payload.get("agentSession").is_some()
        || row.payload.get("agentSessionEvent").is_some()
        || row.payload.get("data").is_some_and(|data| {
            data.get("agentSession").is_some() || data.get("agentSessionEvent").is_some()
        })
}

fn event_data(payload: &Value) -> &Value {
    payload.get("data").unwrap_or(payload)
}

fn first_string(value: Option<&Value>, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        value
            .and_then(|value| value.get(*field))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn nested_string(value: Option<&Value>, path: &[&str]) -> Option<String> {
    let mut current = value?;
    for field in path {
        current = current.get(*field)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_agent_session_event(payload: &Value) -> Result<LinearAgentSessionEvent, SyncError> {
    let data = event_data(payload);
    let session = data
        .get("agentSession")
        .or_else(|| data.get("agentSessionEvent"))
        .filter(|value| value.is_object());
    let session_id = first_string(session, &["id", "agentSessionId"])
        .or_else(|| first_string(Some(data), &["agentSessionId", "sessionId"]))
        .ok_or_else(|| {
            SyncError::permanent(anyhow::anyhow!(
                "Linear Agent Session event has no session id"
            ))
        })?;
    let linear_issue_id = nested_string(session, &["issue", "id"])
        .or_else(|| first_string(session, &["issueId", "linearIssueId"]))
        .or_else(|| nested_string(Some(data), &["issue", "id"]))
        .or_else(|| first_string(Some(data), &["issueId", "linearIssueId"]))
        .ok_or_else(|| {
            SyncError::permanent(anyhow::anyhow!(
                "Linear Agent Session event has no Issue id"
            ))
        })?;
    let action = first_string(Some(data), &["action"])
        .or_else(|| first_string(session, &["action"]))
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| {
            SyncError::permanent(anyhow::anyhow!("Linear Agent Session event has no action"))
        })?;
    if !matches!(action.as_str(), "created" | "prompted") {
        return Err(SyncError::permanent(anyhow::anyhow!(
            "unsupported Linear Agent Session action: {action}"
        )));
    }
    let prompt_context = first_string(session, &["promptContext", "context"])
        .or_else(|| first_string(Some(data), &["promptContext", "context"]));
    let prompt_body = nested_string(Some(data), &["agentActivity", "body"])
        .or_else(|| nested_string(Some(data), &["agentActivity", "content", "body"]))
        .or_else(|| nested_string(session, &["prompt", "body"]))
        .or_else(|| first_string(session, &["promptBody", "body"]))
        .or_else(|| first_string(Some(data), &["prompt", "promptBody", "body"]));
    let requester_user_id = first_string(session, &["creatorId"])
        .or_else(|| nested_string(session, &["creator", "id"]))
        .or_else(|| first_string(Some(data), &["creatorId"]));
    Ok(LinearAgentSessionEvent {
        session_id,
        linear_issue_id,
        action,
        prompt_context,
        prompt_body,
        requester_user_id,
    })
}

fn parse_agent_session_terminal_event(
    payload: &Value,
) -> Result<LinearAgentSessionTerminalEvent, SyncError> {
    let data = event_data(payload);
    let session = data
        .get("agentSession")
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            SyncError::permanent(anyhow::anyhow!(
                "Linear Agent Session terminal event has no session"
            ))
        })?;
    let session_id = first_string(Some(session), &["id", "agentSessionId"]).ok_or_else(|| {
        SyncError::permanent(anyhow::anyhow!(
            "Linear Agent Session terminal event has no session id"
        ))
    })?;
    let status = first_string(Some(data), &["status", "taskStatus"])
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| {
            SyncError::permanent(anyhow::anyhow!(
                "Linear Agent Session terminal event has no status"
            ))
        })?;
    if !matches!(status.as_str(), "completed" | "failed" | "cancelled") {
        return Err(SyncError::permanent(anyhow::anyhow!(
            "unsupported Linear Agent Session terminal status: {status}"
        )));
    }
    let result_body = data
        .get("result")
        .and_then(|result| {
            result
                .as_str()
                .or_else(|| result.get("output").and_then(Value::as_str))
                .or_else(|| result.get("body").and_then(Value::as_str))
        })
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let error_body = first_string(Some(data), &["error", "failureReason", "message"]);
    let body = match (status.as_str(), result_body, error_body) {
        ("completed", Some(body), _) => body.to_string(),
        ("completed", None, _) => "Patchbay Agent completed the task.".to_string(),
        (_, _, Some(error)) => format!("Patchbay Agent {status}: {error}"),
        (_, _, None) => format!("Patchbay Agent {status} the task."),
    };
    Ok(LinearAgentSessionTerminalEvent {
        session_id,
        status,
        body,
    })
}

fn agent_label_decision(
    binding: &LinearProjectBinding,
    labels: &[LinearRemoteLabel],
) -> Result<AgentLabelDecision, SyncError> {
    let Some(mapping) = binding.agent_label_mapping.as_object() else {
        return Ok(AgentLabelDecision {
            configured: false,
            agent_id: None,
        });
    };
    let Some(_group_id) = mapping
        .get("group_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(AgentLabelDecision {
            configured: false,
            agent_id: None,
        });
    };
    let label_mapping = mapping
        .get("labels")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            SyncError::permanent(anyhow::anyhow!(
                "Linear Agent label mapping has no labels object"
            ))
        })?;
    let mut mapped_agents = Vec::new();
    for (label_id, agent_value) in label_mapping {
        let agent_id = agent_value
            .as_str()
            .ok_or_else(|| {
                SyncError::permanent(anyhow::anyhow!(
                    "Linear Agent label mapping target is not a UUID"
                ))
            })?
            .parse::<Uuid>()
            .map_err(|_| {
                SyncError::permanent(anyhow::anyhow!(
                    "Linear Agent label mapping target is not a UUID"
                ))
            })?;
        if labels.iter().any(|label| label.id == *label_id) {
            mapped_agents.push((label_id.as_str(), agent_id));
        }
    }
    if mapped_agents.len() > 1 {
        return Err(SyncError::permanent(anyhow::anyhow!(
            "Linear Agent Label Group has more than one selected value"
        )));
    }
    if let Some((_, agent_id)) = mapped_agents.into_iter().next() {
        return Ok(AgentLabelDecision {
            configured: true,
            agent_id: Some(agent_id),
        });
    }
    let default_agent_id = mapping
        .get("default_agent_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value.parse::<Uuid>().map_err(|_| {
                SyncError::permanent(anyhow::anyhow!(
                    "Linear default Agent mapping target is not a UUID"
                ))
            })
        })
        .transpose()?;
    Ok(AgentLabelDecision {
        configured: true,
        agent_id: default_agent_id,
    })
}

/// Returns the complete remote label set for an outbound Issue mutation.
/// Labels outside the configured Patchbay Agent group are preserved verbatim;
/// only the integration-owned group value is replaced. `Some(empty)` is
/// intentional when the local executor is cleared, so a stale Agent label is
/// removed instead of being left behind on Linear.
fn agent_label_ids_for_issue(
    binding: &LinearProjectBinding,
    executor_type: Option<&str>,
    executor_id: Option<Uuid>,
    existing_labels: &[LinearRemoteLabel],
) -> Result<Option<Vec<String>>, SyncError> {
    let Some(mapping) = binding.agent_label_mapping.as_object() else {
        return Ok(None);
    };
    let configured_group = mapping
        .get("group_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(_group_id) = configured_group else {
        return Ok(None);
    };
    let label_mapping = mapping
        .get("labels")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            SyncError::permanent(anyhow::anyhow!(
                "Linear Agent label mapping has no labels object"
            ))
        })?;

    let mut owned_label_ids = Vec::with_capacity(label_mapping.len());
    let mut desired_label_id = None;
    for (label_id, agent_value) in label_mapping {
        let agent_id = agent_value
            .as_str()
            .ok_or_else(|| {
                SyncError::permanent(anyhow::anyhow!(
                    "Linear Agent label mapping target is not a UUID"
                ))
            })?
            .parse::<Uuid>()
            .map_err(|_| {
                SyncError::permanent(anyhow::anyhow!(
                    "Linear Agent label mapping target is not a UUID"
                ))
            })?;
        owned_label_ids.push(label_id.as_str());
        if executor_type == Some("agent") && executor_id == Some(agent_id) {
            if desired_label_id.is_some() {
                return Err(SyncError::permanent(anyhow::anyhow!(
                    "Linear Agent label mapping contains duplicate Agent targets"
                )));
            }
            desired_label_id = Some(label_id.as_str());
        }
    }

    if executor_type == Some("agent") && executor_id.is_some() && desired_label_id.is_none() {
        let default_agent_id = mapping
            .get("default_agent_id")
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<Uuid>().ok());
        if default_agent_id != executor_id {
            return Err(SyncError::permanent(anyhow::anyhow!(
                "Patchbay Agent has no label mapping for this Linear binding"
            )));
        }
    }

    let mut label_ids = existing_labels
        .iter()
        .map(|label| label.id.clone())
        .filter(|label_id| !owned_label_ids.iter().any(|owned| *owned == label_id))
        .collect::<Vec<_>>();
    if let Some(label_id) = desired_label_id {
        if !label_ids.iter().any(|existing| existing == label_id) {
            label_ids.push(label_id.to_string());
        }
    }
    Ok(Some(label_ids))
}

struct ExistingRemoteIssueInput<'a> {
    connection: &'a LinearConnection,
    binding: LinearProjectBinding,
    link: LinearIssueLink,
    remote: LinearRemoteIssue,
    remote_snapshot: Value,
    source_event_id: &'a str,
    event_timestamp_ms: Option<i64>,
    remote_updated_at: DateTime<Utc>,
    updated_from: Option<&'a Value>,
    agent_decision: AgentLabelDecision,
    destination_project_id: Option<Uuid>,
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

    /// Processes at most one row from each enabled queue. This public seam is
    /// used by focused worker tests and by production's supervisor; PostgreSQL
    /// remains the source of truth for claim ownership.
    pub async fn process_next(&self, worker_id: &str) -> anyhow::Result<bool> {
        let pull_enabled = self.state.linear_pull_import_enabled_for_any_workspace();
        let push_enabled = self.state.linear_push_enabled_for_any_workspace();
        let agent_bridge_enabled = self.state.linear_agent_bridge_enabled_for_any_workspace();
        if !pull_enabled && !push_enabled && !agent_bridge_enabled {
            return Ok(false);
        }
        let mut processed = false;

        if pull_enabled {
            let workspace_filter = self.state.linear_pull_import_workspace_filter();
            let _ = linear_q::dead_letter_exhausted_sync_inbox(
                &self.state.pool,
                workspace_filter.as_deref(),
            )
            .await?;
            if let Some(row) = linear_q::claim_sync_inbox(
                &self.state.pool,
                worker_id,
                1,
                LEASE_SECONDS,
                workspace_filter.as_deref(),
                true,
            )
            .await?
            .into_iter()
            .next()
            {
                self.finish_inbox_row(row, worker_id).await?;
                processed = true;
            }
        }

        if agent_bridge_enabled {
            let workspace_filter = self.state.linear_agent_bridge_workspace_filter();
            let _ = linear_q::dead_letter_exhausted_sync_inbox(
                &self.state.pool,
                workspace_filter.as_deref(),
            )
            .await?;
            if let Some(row) = linear_q::claim_sync_inbox(
                &self.state.pool,
                worker_id,
                1,
                LEASE_SECONDS,
                workspace_filter.as_deref(),
                false,
            )
            .await?
            .into_iter()
            .next()
            {
                self.finish_inbox_row(row, worker_id).await?;
                processed = true;
            }
        }

        if push_enabled {
            let workspace_filter = self.state.linear_push_workspace_filter();
            let _ = linear_q::dead_letter_exhausted_sync_outbox(
                &self.state.pool,
                workspace_filter.as_deref(),
            )
            .await?;
            if let Some(row) = linear_q::claim_sync_outbox(
                &self.state.pool,
                worker_id,
                1,
                LEASE_SECONDS,
                workspace_filter.as_deref(),
            )
            .await?
            .into_iter()
            .next()
            {
                self.finish_outbox_row(row, worker_id).await?;
                processed = true;
            }
        }

        Ok(processed)
    }

    async fn finish_inbox_row(&self, row: LinearSyncInbox, worker_id: &str) -> anyhow::Result<()> {
        let renew_cancel = CancellationToken::new();
        let lease_lost = CancellationToken::new();
        let renew_task = {
            let pool = self.state.pool.clone();
            let inbox_id = row.id;
            let worker_id = worker_id.to_string();
            let cancel = renew_cancel.clone();
            let lost = lease_lost.clone();
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
                                    lost.cancel();
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
        let result = tokio::select! {
            result = self.process_row(&row, worker_id) => result,
            _ = lease_lost.cancelled() => Err(SyncError::retry(anyhow::anyhow!(
                "Linear Inbox processing lost its lease"
            ))),
        };
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
        Ok(())
    }

    async fn finish_outbox_row(
        &self,
        row: LinearSyncOutbox,
        worker_id: &str,
    ) -> anyhow::Result<()> {
        let renew_cancel = CancellationToken::new();
        let renew_task = {
            let pool = self.state.pool.clone();
            let outbox_id = row.id;
            let worker_id = worker_id.to_string();
            let cancel = renew_cancel.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(20));
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = interval.tick() => {
                            match linear_q::renew_claimed_sync_outbox(
                                &pool,
                                outbox_id,
                                &worker_id,
                                LEASE_SECONDS,
                            ).await {
                                Ok(true) => {}
                                Ok(false) => {
                                    tracing::warn!(
                                        outbox_id = %outbox_id,
                                        worker_id,
                                        "Linear Outbox lease renewal lost ownership"
                                    );
                                    return;
                                }
                                Err(error) => tracing::warn!(
                                    outbox_id = %outbox_id,
                                    worker_id,
                                    %error,
                                    "Linear Outbox lease renewal failed"
                                ),
                            }
                        }
                    }
                }
            })
        };
        let result = self.process_outbox_row(&row, worker_id).await;
        renew_cancel.cancel();
        let _ = renew_task.await;

        match result {
            Ok(()) => {
                // Successful rows are completed inside the same transaction
                // that writes the Issue Link. The worker method only logs a
                // lost lease here if a future implementation returns before
                // that atomic completion.
            }
            Err(error) => {
                let message = error.message();
                let permanent = matches!(error, SyncError::Permanent(_));
                let exhausted = row.attempts >= row.max_attempts;
                if permanent || exhausted {
                    let owned = linear_q::dead_letter_claimed_sync_outbox(
                        &self.state.pool,
                        row.id,
                        worker_id,
                        &message,
                    )
                    .await?;
                    if !owned {
                        tracing::warn!(
                            outbox_id = %row.id,
                            worker_id,
                            "Linear Outbox dead-letter lost its lease"
                        );
                    }
                } else {
                    let available_at = Utc::now() + retry_delay(row.attempts);
                    let owned = linear_q::retry_claimed_sync_outbox(
                        &self.state.pool,
                        row.id,
                        worker_id,
                        available_at,
                        &message,
                    )
                    .await?;
                    if !owned {
                        tracing::warn!(
                            outbox_id = %row.id,
                            worker_id,
                            "Linear Outbox retry lost its lease"
                        );
                    }
                }
                tracing::warn!(
                    outbox_id = %row.id,
                    attempts = row.attempts,
                    permanent,
                    error = %message,
                    "Linear Outbox item failed"
                );
            }
        }
        Ok(())
    }

    async fn process_outbox_row(
        &self,
        row: &LinearSyncOutbox,
        worker_id: &str,
    ) -> Result<(), SyncError> {
        if !self.state.linear_push_enabled(row.workspace_id) {
            return Err(SyncError::retry(anyhow::anyhow!(
                "Linear push is not enabled for this workspace"
            )));
        }
        if !matches!(row.event_type.as_str(), "issue_created" | "issue_updated") {
            return Err(SyncError::permanent(anyhow::anyhow!(
                "unsupported Linear Outbox event type"
            )));
        }

        let binding =
            linear_q::get_project_binding(&self.state.pool, row.workspace_id, row.binding_id)
                .await
                .map_err(SyncError::retry)?
                .ok_or_else(|| SyncError::permanent(anyhow::anyhow!("Linear binding not found")))?;
        if binding.status != "active"
            || !matches!(binding.sync_mode.as_str(), "publish" | "two_way")
            || binding.linear_team_id.is_none()
        {
            return Err(SyncError::retry(anyhow::anyhow!(
                "Linear binding is not currently publishable"
            )));
        }

        let connection =
            linear_q::get_connection_by_id_unscoped(&self.state.pool, binding.connection_id)
                .await
                .map_err(SyncError::retry)?
                .ok_or_else(|| {
                    SyncError::permanent(anyhow::anyhow!("Linear connection not found"))
                })?;
        if connection.workspace_id != row.workspace_id {
            return Err(SyncError::permanent(anyhow::anyhow!(
                "Linear Outbox workspace does not match its binding"
            )));
        }
        if connection.status != "active" {
            return Err(if connection.status == "reauthorization_required" {
                SyncError::retry(anyhow::anyhow!(
                    "Linear connection requires reauthorization"
                ))
            } else {
                SyncError::permanent(anyhow::anyhow!("Linear connection is not active"))
            });
        }

        let mut issue =
            issue_q::get_issue_in_workspace(&self.state.pool, row.issue_id, row.workspace_id)
                .await
                .map_err(SyncError::retry)?
                .ok_or_else(|| SyncError::permanent(anyhow::anyhow!("Patchbay Issue not found")))?;
        let existing_link = linear_q::get_linear_issue_link_by_patchbay_issue(
            &self.state.pool,
            row.workspace_id,
            issue.id,
        )
        .await
        .map_err(SyncError::retry)?;
        if let Some(link) = existing_link.as_ref() {
            if link.binding_id != binding.id {
                return Err(SyncError::permanent(anyhow::anyhow!(
                    "Patchbay Issue is already linked to another Linear binding"
                )));
            }
            if link.sync_status == "conflict" {
                return Err(SyncError::permanent(anyhow::anyhow!(
                    "Linear Issue Link is awaiting conflict resolution"
                )));
            }
        }

        let manager = LinearTokenManager::from_state(&self.state)
            .map_err(|error| classify_token_error(error, "create Linear token manager"))?;
        let current_remote = if let Some(link) = existing_link.as_ref() {
            let remote = manager
                .fetch_issue(connection.id, &link.linear_issue_id)
                .await
                .map_err(|error| classify_token_error(error, "fetch Linear Issue before push"))?
                .ok_or_else(|| {
                    SyncError::permanent(anyhow::anyhow!(
                        "remote Linear Issue no longer exists; relink is required"
                    ))
                })?;
            let remote_updated_at = parse_remote_timestamp(&remote.updated_at)?;
            let remote_status = map_remote_status(&binding, remote.state.as_ref())?;
            let remote_priority = map_remote_priority(remote.priority)?;
            let remote_owner_id = self
                .remote_owner_id(&connection, remote.assignee.as_ref())
                .await?;
            let remote_snapshot =
                remote_sync_snapshot(&remote, &remote_status, &remote_priority, remote_owner_id);
            let base_snapshot = normalized_base_snapshot(&link.last_common_snapshot, &binding)?;
            let local_snapshot = local_sync_snapshot(&issue);
            let merge = merge_sync_snapshots(&base_snapshot, &local_snapshot, &remote_snapshot);
            if !merge.conflicts.is_empty() {
                let source_event_id = format!("linear-outbox:{}", row.id);
                let mut transaction = self.state.pool.begin().await.map_err(SyncError::retry)?;
                for conflict in &merge.conflicts {
                    linear_q::create_linear_sync_conflict(
                        &mut *transaction,
                        &linear_q::LinearSyncConflictInput {
                            id: Uuid::now_v7(),
                            workspace_id: row.workspace_id,
                            binding_id: binding.id,
                            link_id: link.id,
                            patchbay_issue_id: issue.id,
                            linear_issue_id: &remote.id,
                            field: &conflict.field,
                            base_value: &conflict.base_value,
                            local_value: &conflict.local_value,
                            remote_value: &conflict.remote_value,
                            source_event_id: &source_event_id,
                            source_event_at_ms: None,
                        },
                    )
                    .await
                    .map_err(SyncError::retry)?;
                }
                let updated = linear_q::set_linear_issue_link_state(
                    &mut *transaction,
                    link.id,
                    row.workspace_id,
                    &base_snapshot,
                    Some(remote_updated_at),
                    link.last_remote_event_at_ms,
                    link.last_remote_event_id.as_deref(),
                    "conflict",
                )
                .await
                .map_err(SyncError::retry)?;
                if !updated {
                    return Err(SyncError::retry(anyhow::anyhow!(
                        "Linear Issue Link disappeared while recording push conflict"
                    )));
                }
                transaction.commit().await.map_err(SyncError::retry)?;
                return Err(SyncError::permanent(anyhow::anyhow!(
                    "remote Linear Issue changed before push; conflict recorded"
                )));
            }
            if merge.remote_changed {
                self.state
                    .issues
                    .apply_external_patch(
                        row.workspace_id,
                        issue.id,
                        IssueCommand::ApplyExternalPatch {
                            source: ExternalSource::Linear,
                            source_event_id: format!("linear-outbox:{}", row.id),
                            expected_revision: Some(issue.revision),
                            suppress_external_outbox: true,
                            patch: external_patch_from_snapshot(&merge.merged)?,
                        },
                    )
                    .await
                    .map_err(|error| {
                        classify_external_error(error, "merge Linear Issue before push")
                    })?;
                issue = issue_q::get_issue_in_workspace(
                    &self.state.pool,
                    row.issue_id,
                    row.workspace_id,
                )
                .await
                .map_err(SyncError::retry)?
                .ok_or_else(|| {
                    SyncError::retry(anyhow::anyhow!(
                        "Patchbay Issue disappeared after Linear merge"
                    ))
                })?;
            }
            Some(remote)
        } else {
            None
        };

        let priority = map_local_priority(&issue.priority)?;
        let state_id = map_local_status(&binding, &issue.status);
        let due_date = issue
            .due_date
            .map(|date| date.format("%Y-%m-%d").to_string());
        let linear_owner_id = match (issue.owner_type.as_deref(), issue.owner_id) {
            (None, None) => None,
            (Some("member"), Some(owner_id)) => Some(
                linear_q::get_linear_member_binding(
                    &self.state.pool,
                    row.workspace_id,
                    connection.id,
                    owner_id,
                )
                .await
                .map_err(SyncError::retry)?
                .ok_or_else(|| {
                    SyncError::permanent(anyhow::anyhow!(
                        "Patchbay human owner has no Linear member mapping"
                    ))
                })?
                .linear_user_id,
            ),
            _ => {
                return Err(SyncError::permanent(anyhow::anyhow!(
                    "Patchbay Issue owner is not a supported human member"
                )))
            }
        };
        let update_assignee = Some(linear_owner_id.as_deref());
        let desired_delegate_id = if self.state.linear_agent_bridge_enabled(row.workspace_id)
            && issue.executor_type.as_deref() == Some("agent")
            && issue.executor_id.is_some()
        {
            Some(connection.actor_id.as_str())
        } else {
            None
        };
        let update_delegate = if desired_delegate_id.is_some() {
            Some(desired_delegate_id)
        } else if current_remote.as_ref().is_some_and(|remote| {
            remote
                .delegate
                .as_ref()
                .is_some_and(|delegate| delegate.id == connection.actor_id)
        }) {
            // Remove only Patchbay's own delegate. A user may have selected a
            // different Agent, which this integration must leave untouched.
            Some(None)
        } else {
            None
        };
        let agent_label_ids = if self.state.linear_agent_bridge_enabled(row.workspace_id) {
            agent_label_ids_for_issue(
                &binding,
                issue.executor_type.as_deref(),
                issue.executor_id,
                current_remote
                    .as_ref()
                    .map(|remote| remote.labels.nodes.as_slice())
                    .unwrap_or(&[]),
            )?
        } else {
            None
        };
        let remote = if let Some(remote) = current_remote {
            manager
                .update_issue(&LinearIssueUpdateInput {
                    connection_id: connection.id,
                    linear_issue_id: &remote.id,
                    patchbay_issue_id: issue.id,
                    title: &issue.title,
                    description: issue.description.as_deref(),
                    priority,
                    state_id: state_id.as_deref(),
                    due_date: due_date.as_deref(),
                    assignee_id: update_assignee,
                    delegate_id: update_delegate,
                    label_ids: agent_label_ids.as_deref(),
                })
                .await
                .map_err(|error| classify_token_error(error, "update Linear Issue"))?
        } else if row.attempts > 1 {
            if let Some(remote) = manager
                .find_issue_by_marker(connection.id, &binding.linear_project_id, issue.id)
                .await
                .map_err(|error| classify_token_error(error, "reconcile Linear Issue create"))?
            {
                remote
            } else {
                let team_id = binding.linear_team_id.as_deref().ok_or_else(|| {
                    SyncError::permanent(anyhow::anyhow!(
                        "Linear binding has no team for Issue creation"
                    ))
                })?;
                manager
                    .create_issue(&LinearIssueCreateInput {
                        connection_id: connection.id,
                        team_id,
                        project_id: &binding.linear_project_id,
                        issue_id: issue.id,
                        title: &issue.title,
                        description: issue.description.as_deref(),
                        priority,
                        state_id: state_id.as_deref(),
                        due_date: due_date.as_deref(),
                        assignee_id: linear_owner_id.as_deref(),
                        delegate_id: desired_delegate_id,
                        label_ids: agent_label_ids.as_deref(),
                    })
                    .await
                    .map_err(|error| classify_token_error(error, "create Linear Issue"))?
            }
        } else {
            let team_id = binding.linear_team_id.as_deref().ok_or_else(|| {
                SyncError::permanent(anyhow::anyhow!(
                    "Linear binding has no team for Issue creation"
                ))
            })?;
            manager
                .create_issue(&LinearIssueCreateInput {
                    connection_id: connection.id,
                    team_id,
                    project_id: &binding.linear_project_id,
                    issue_id: issue.id,
                    title: &issue.title,
                    description: issue.description.as_deref(),
                    priority,
                    state_id: state_id.as_deref(),
                    due_date: due_date.as_deref(),
                    assignee_id: linear_owner_id.as_deref(),
                    delegate_id: desired_delegate_id,
                    label_ids: agent_label_ids.as_deref(),
                })
                .await
                .map_err(|error| classify_token_error(error, "create Linear Issue"))?
        };
        if remote.identifier.trim().is_empty() || remote.id.trim().is_empty() {
            return Err(SyncError::permanent(anyhow::anyhow!(
                "Linear mutation returned incomplete Issue identity"
            )));
        }
        if remote
            .project
            .as_ref()
            .is_some_and(|project| project.id != binding.linear_project_id)
        {
            return Err(SyncError::permanent(anyhow::anyhow!(
                "Linear mutation returned an Issue from another Project"
            )));
        }
        let remote_updated_at = parse_remote_timestamp(&remote.updated_at)?;
        let remote_status = map_remote_status(&binding, remote.state.as_ref())?;
        let remote_priority = map_remote_priority(remote.priority)?;
        let remote_owner_id = self
            .remote_owner_id(&connection, remote.assignee.as_ref())
            .await?;
        let snapshot =
            remote_sync_snapshot(&remote, &remote_status, &remote_priority, remote_owner_id);

        // The provider mutation is followed by one local transaction. If the
        // commit fails, the Outbox row remains pending and the next attempt
        // searches the stable marker before issuing another mutation.
        let mut transaction = self.state.pool.begin().await.map_err(SyncError::retry)?;
        let link = if let Some(link) = existing_link {
            link
        } else {
            let created = linear_q::create_linear_issue_link(
                &mut *transaction,
                &linear_q::LinearIssueLinkInput {
                    id: Uuid::now_v7(),
                    workspace_id: row.workspace_id,
                    binding_id: binding.id,
                    patchbay_issue_id: issue.id,
                    linear_issue_id: &remote.id,
                    linear_identifier: &remote.identifier,
                    last_common_snapshot: &snapshot,
                    remote_updated_at: Some(remote_updated_at),
                    last_remote_event_at_ms: None,
                    last_remote_event_id: None,
                },
            )
            .await
            .map_err(SyncError::retry)?;
            if let Some(created) = created {
                created
            } else {
                linear_q::get_linear_issue_link_by_patchbay_issue(
                    &mut *transaction,
                    row.workspace_id,
                    issue.id,
                )
                .await
                .map_err(SyncError::retry)?
                .filter(|link| link.binding_id == binding.id)
                .ok_or_else(|| {
                    SyncError::retry(anyhow::anyhow!(
                        "Linear Issue Link insert raced without a visible binding"
                    ))
                })?
            }
        };
        let last_event_at_ms = link.last_remote_event_at_ms;
        let last_event_id = link.last_remote_event_id.as_deref();
        let updated = linear_q::update_linear_issue_link(
            &mut *transaction,
            &linear_q::LinearIssueLinkUpdate {
                link_id: link.id,
                workspace_id: row.workspace_id,
                last_common_snapshot: &snapshot,
                remote_updated_at: Some(remote_updated_at),
                last_remote_event_at_ms: last_event_at_ms,
                last_remote_event_id: last_event_id,
                sync_status: "active",
            },
        )
        .await
        .map_err(SyncError::retry)?;
        if !updated {
            return Err(SyncError::retry(anyhow::anyhow!(
                "Linear Issue Link disappeared during push"
            )));
        }
        if self.state.linear_agent_bridge_enabled(row.workspace_id) {
            if let Some(url) = self.patchbay_issue_url(&issue).await? {
                manager
                    .create_or_update_attachment(
                        connection.id,
                        &remote.id,
                        "Open in Patchbay",
                        &format!("Agent · {}", issue.status),
                        &url,
                        json!({
                            "patchbay_issue_id": issue.id.to_string(),
                            "status": issue.status,
                        }),
                    )
                    .await
                    .map_err(|error| {
                        classify_token_error(error, "publish Linear Agent attachment")
                    })?;
            }
        }
        let completed =
            linear_q::complete_claimed_sync_outbox(&mut *transaction, row.id, worker_id)
                .await
                .map_err(SyncError::retry)?;
        if !completed {
            return Err(SyncError::retry(anyhow::anyhow!(
                "Linear Outbox completion lost its lease"
            )));
        }
        transaction.commit().await.map_err(SyncError::retry)?;
        self.resume_agent_sessions_awaiting_issue_link(&connection, &remote.id)
            .await?;
        Ok(())
    }

    async fn process_row(&self, row: &LinearSyncInbox, worker_id: &str) -> Result<(), SyncError> {
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
        if is_agent_session_event(row) {
            if !self
                .state
                .linear_agent_bridge_enabled(connection.workspace_id)
            {
                return Ok(());
            }
            if row.event_type == "linear.agentSession.terminal"
                || row.payload.get("linearAgentSessionTerminal").is_some()
            {
                return self
                    .process_agent_session_terminal(row, &connection, worker_id)
                    .await;
            }
            return self
                .process_agent_session_event(row, &connection, worker_id)
                .await;
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
        let updated_from = extract_updated_from(&row.payload);
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
            updated_from,
        )
        .await?;
        self.resume_agent_sessions_awaiting_issue_link(&connection, &linear_issue_id)
            .await
    }

    async fn process_agent_session_event(
        &self,
        row: &LinearSyncInbox,
        connection: &LinearConnection,
        worker_id: &str,
    ) -> Result<(), SyncError> {
        let event = parse_agent_session_event(&row.payload)?;
        let event_timestamp_ms = extract_event_timestamp_ms(&row.payload);
        let source_event_id = format!("linear-agent-delivery:{}", row.delivery_id);
        let mut link = linear_q::find_linear_issue_link(
            &self.state.pool,
            connection.workspace_id,
            connection.id,
            &event.linear_issue_id,
        )
        .await
        .map_err(SyncError::retry)?;

        // A delegated Issue can arrive before the ordinary Issue webhook. Use
        // the same full-snapshot bootstrap as Project Sync, then persist an
        // explicit waiting state if the binding is not import-capable.
        if link.is_none() {
            let manager = LinearTokenManager::from_state(&self.state)
                .map_err(|error| classify_token_error(error, "create Linear token manager"))?;
            if let Some(remote) = manager
                .fetch_issue(connection.id, &event.linear_issue_id)
                .await
                .map_err(|error| classify_token_error(error, "fetch Linear Agent Issue"))?
            {
                self.apply_remote_issue(
                    connection,
                    remote,
                    None,
                    &source_event_id,
                    event_timestamp_ms,
                    None,
                )
                .await?;
                link = linear_q::find_linear_issue_link(
                    &self.state.pool,
                    connection.workspace_id,
                    connection.id,
                    &event.linear_issue_id,
                )
                .await
                .map_err(SyncError::retry)?;
            }
        }

        let Some(link) = link else {
            linear_agent_q::upsert_linear_agent_session(
                &self.state.pool,
                Uuid::now_v7(),
                connection.workspace_id,
                connection.id,
                &event.session_id,
                &event.linear_issue_id,
                None,
                None,
                None,
                &event.action,
                "awaiting_issue_link",
                event.prompt_context.as_deref(),
                event.prompt_body.as_deref(),
                event.requester_user_id.as_deref(),
                &row.delivery_id,
                event_timestamp_ms,
            )
            .await
            .map_err(SyncError::retry)?;
            return Ok(());
        };

        let issue = issue_q::get_issue_in_workspace(
            &self.state.pool,
            link.patchbay_issue_id,
            connection.workspace_id,
        )
        .await
        .map_err(SyncError::retry)?
        .ok_or_else(|| SyncError::permanent(anyhow::anyhow!("Patchbay Issue not found")))?;
        let Some(agent_id) = (issue.executor_type.as_deref() == Some("agent"))
            .then_some(issue.executor_id)
            .flatten()
        else {
            linear_agent_q::upsert_linear_agent_session(
                &self.state.pool,
                Uuid::now_v7(),
                connection.workspace_id,
                connection.id,
                &event.session_id,
                &event.linear_issue_id,
                Some(issue.id),
                None,
                None,
                &event.action,
                "agent_selection_required",
                event.prompt_context.as_deref(),
                event.prompt_body.as_deref(),
                event.requester_user_id.as_deref(),
                &row.delivery_id,
                event_timestamp_ms,
            )
            .await
            .map_err(SyncError::retry)?;
            return Ok(());
        };
        let agent =
            agent_q::get_agent_in_workspace(&self.state.pool, agent_id, connection.workspace_id)
                .await
                .map_err(SyncError::retry)?;
        if agent
            .as_ref()
            .map(|agent| agent.archived_at.is_some())
            .unwrap_or(true)
        {
            linear_agent_q::upsert_linear_agent_session(
                &self.state.pool,
                Uuid::now_v7(),
                connection.workspace_id,
                connection.id,
                &event.session_id,
                &event.linear_issue_id,
                Some(issue.id),
                Some(agent_id),
                None,
                &event.action,
                "agent_selection_required",
                event.prompt_context.as_deref(),
                event.prompt_body.as_deref(),
                event.requester_user_id.as_deref(),
                &row.delivery_id,
                event_timestamp_ms,
            )
            .await
            .map_err(SyncError::retry)?;
            return Ok(());
        }
        let requester_user_id = self
            .patchbay_requester_id(connection, event.requester_user_id.as_deref())
            .await?;

        let existing_session = linear_agent_q::get_linear_agent_session(
            &self.state.pool,
            connection.workspace_id,
            connection.id,
            &event.session_id,
        )
        .await
        .map_err(SyncError::retry)?;
        if existing_session.as_ref().is_some_and(|session| {
            matches!(
                session.status.as_str(),
                "completed" | "failed" | "cancelled"
            )
        }) {
            return Ok(());
        }
        if existing_session.as_ref().is_some_and(|session| {
            session.last_event_id != row.delivery_id
                && matches!(
                    (event_timestamp_ms, session.last_event_at_ms),
                    (Some(incoming), Some(previous)) if incoming <= previous
                )
        }) {
            return Ok(());
        }
        let same_delivery_with_task = existing_session.as_ref().is_some_and(|session| {
            session.last_event_id == row.delivery_id && session.task_id.is_some()
        });
        if same_delivery_with_task
            && existing_session
                .as_ref()
                .is_some_and(|session| matches!(session.status.as_str(), "queued" | "prompted"))
        {
            return Ok(());
        }

        // Atomically reserve this non-terminal delivery before creating or
        // continuing a task. Terminal Inbox work observes `dispatching` and
        // retries, so it cannot publish a final response between this guard
        // and the durable task correlation below.
        let Some(claimed_session) = linear_agent_q::claim_linear_agent_session_dispatch(
            &self.state.pool,
            Uuid::now_v7(),
            connection.workspace_id,
            connection.id,
            &event.session_id,
            &event.linear_issue_id,
            issue.id,
            agent_id,
            &event.action,
            event.prompt_context.as_deref(),
            event.prompt_body.as_deref(),
            event.requester_user_id.as_deref(),
            &row.delivery_id,
            event_timestamp_ms,
            worker_id,
        )
        .await
        .map_err(SyncError::retry)?
        else {
            let current = linear_agent_q::get_linear_agent_session(
                &self.state.pool,
                connection.workspace_id,
                connection.id,
                &event.session_id,
            )
            .await
            .map_err(SyncError::retry)?;
            if current.as_ref().is_some_and(|session| {
                matches!(
                    session.status.as_str(),
                    "completed" | "failed" | "cancelled"
                )
            }) {
                return Ok(());
            }
            return Err(SyncError::retry(anyhow::anyhow!(
                "Linear Agent Session has another dispatch in progress"
            )));
        };

        let mut task = if let Some(task_id) = claimed_session.task_id {
            agent_q::get_agent_task_in_workspace(&self.state.pool, task_id, connection.workspace_id)
                .await
                .map_err(SyncError::retry)?
                .filter(|task| task.agent_id == agent_id)
        } else {
            None
        };
        let session_marker = format!("linear-agent-session:{}", event.session_id);
        if task.is_none() {
            if let Some(task_id) = agent_q::find_task_id_by_issue_agent_session_marker(
                &self.state.pool,
                issue.id,
                agent_id,
                &session_marker,
            )
            .await
            .map_err(SyncError::retry)?
            {
                task = agent_q::get_agent_task_in_workspace(
                    &self.state.pool,
                    task_id,
                    connection.workspace_id,
                )
                .await
                .map_err(SyncError::retry)?;
            }
        }
        if task.is_none() {
            task = agent_q::list_active_tasks_by_issue(&self.state.pool, issue.id)
                .await
                .map_err(SyncError::retry)?
                .into_iter()
                .find(|candidate| candidate.agent_id == agent_id);
        }

        let mut continuation_task_id = None;
        let event_context = event
            .prompt_body
            .as_deref()
            .or(event.prompt_context.as_deref())
            .map(str::trim)
            .filter(|context| !context.is_empty());
        let should_continue_existing =
            event.action == "prompted" || (event.action == "created" && event_context.is_some());
        if should_continue_existing && !same_delivery_with_task {
            if let Some(parent_task) = task.as_ref() {
                let handoff = event_context.unwrap_or("Linear Agent Session prompt");
                let idempotency_key = format!(
                    "linear-agent-session:{}:{}",
                    event.session_id, row.delivery_id
                );
                let receipt = self
                    .state
                    .tasks
                    .continue_agent_thread(
                        parent_task.id,
                        handoff,
                        &idempotency_key,
                        requester_user_id,
                    )
                    .await
                    .map_err(|error| {
                        SyncError::retry(anyhow::anyhow!(
                            "continue Linear Agent Session task: {error}"
                        ))
                    })?;
                continuation_task_id = Some(receipt.continuation_task_id);
            }
        }
        if task.is_none() && continuation_task_id.is_none() {
            let mut agent_issue = issue.clone();
            agent_issue.executor_type = Some("agent".to_string());
            agent_issue.executor_id = Some(agent_id);
            let handoff = match event_context {
                Some(context) => format!("{session_marker}\n\n{context}"),
                None => session_marker.clone(),
            };
            let enqueue_result = self
                .state
                .tasks
                .enqueue_task_for_issue_with_handoff(
                    &agent_issue,
                    &handoff,
                    Some(requester_user_id),
                )
                .await;
            task = match enqueue_result {
                Ok(task) => Some(task),
                Err(error) if pending_slot_taken_err(&error) => {
                    let recovered = if let Some(task_id) =
                        agent_q::find_task_id_by_issue_agent_session_marker(
                            &self.state.pool,
                            issue.id,
                            agent_id,
                            &session_marker,
                        )
                        .await
                        .map_err(SyncError::retry)?
                    {
                        agent_q::get_agent_task_in_workspace(
                            &self.state.pool,
                            task_id,
                            connection.workspace_id,
                        )
                        .await
                        .map_err(SyncError::retry)?
                    } else {
                        None
                    };
                    recovered.or(
                        agent_q::list_active_tasks_by_issue(&self.state.pool, issue.id)
                            .await
                            .map_err(SyncError::retry)?
                            .into_iter()
                            .find(|candidate| candidate.agent_id == agent_id),
                    )
                }
                Err(error) => {
                    return Err(SyncError::retry(anyhow::anyhow!(
                        "enqueue Linear Agent Session task: {error}"
                    )))
                }
            };
        }
        let task_id = continuation_task_id
            .or_else(|| task.as_ref().map(|task| task.id))
            .ok_or_else(|| {
                SyncError::retry(anyhow::anyhow!(
                    "Linear Agent Session task was not visible after enqueue"
                ))
            })?;
        let session_status = if event.action == "prompted" {
            "prompted"
        } else {
            "queued"
        };
        let correlated = linear_agent_q::correlate_linear_agent_session_dispatch(
            &self.state.pool,
            connection.workspace_id,
            connection.id,
            &event.session_id,
            &row.delivery_id,
            task_id,
            worker_id,
        )
        .await
        .map_err(SyncError::retry)?;
        if !correlated {
            return Err(SyncError::retry(anyhow::anyhow!(
                "Linear Agent Session dispatch lost its reservation"
            )));
        }

        // A very fast task can reach a terminal state before the session row
        // becomes visible to the task terminal hook. Recover that race after
        // correlation using the same idempotent Inbox delivery key.
        if let Some(terminal_task) =
            agent_q::get_agent_task_in_workspace(&self.state.pool, task_id, connection.workspace_id)
                .await
                .map_err(SyncError::retry)?
                .filter(|task| matches!(task.status.as_str(), "completed" | "failed" | "cancelled"))
        {
            let retry_pending = terminal_task.status == "failed"
                && has_runnable_successor(&self.state.pool, &terminal_task)
                    .await
                    .map_err(SyncError::retry)?;
            if !retry_pending {
                let mut transaction = self.state.pool.begin().await.map_err(SyncError::retry)?;
                linear_agent_q::enqueue_linear_agent_terminal_event(
                    &mut transaction,
                    terminal_task.id,
                    &format!(
                        "linear-agent-terminal:{}:{}",
                        terminal_task.id, terminal_task.status
                    ),
                    &json!({
                        "action": "terminal",
                        "linearAgentSessionTerminal": true,
                        "status": terminal_task.status,
                        "result": terminal_task.result,
                        "error": terminal_task.error,
                        "failureReason": terminal_task.failure_reason,
                        "taskId": terminal_task.id,
                    }),
                )
                .await
                .map_err(SyncError::retry)?;
                transaction.commit().await.map_err(SyncError::retry)?;
                self.notify.notify_waiters();
            }
        }

        let manager = LinearTokenManager::from_state(&self.state)
            .map_err(|error| classify_token_error(error, "create Linear token manager"))?;
        if let Some(url) = self.patchbay_issue_url(&issue).await? {
            manager
                .update_agent_session_external_url(connection.id, &event.session_id, &url)
                .await
                .map_err(|error| classify_token_error(error, "update Linear Agent Session URL"))?;
        }
        let activity = if event.action == "prompted" {
            "Linear prompt accepted and queued for the selected Agent."
        } else {
            "Linear Agent Session accepted and queued for the selected Agent."
        };
        manager
            .create_agent_activity(
                connection.id,
                &event.session_id,
                json!({"type": "thought", "body": activity}),
            )
            .await
            .map_err(|error| classify_token_error(error, "acknowledge Linear Agent Session"))?;
        let released = linear_agent_q::upsert_linear_agent_session(
            &self.state.pool,
            Uuid::now_v7(),
            connection.workspace_id,
            connection.id,
            &event.session_id,
            &event.linear_issue_id,
            Some(issue.id),
            Some(agent_id),
            Some(task_id),
            &event.action,
            session_status,
            event.prompt_context.as_deref(),
            event.prompt_body.as_deref(),
            event.requester_user_id.as_deref(),
            &row.delivery_id,
            event_timestamp_ms,
        )
        .await
        .map_err(SyncError::retry)?;
        if released.is_none() {
            return Err(SyncError::retry(anyhow::anyhow!(
                "Linear Agent Session dispatch could not release its reservation"
            )));
        }
        Ok(())
    }

    async fn process_agent_session_terminal(
        &self,
        row: &LinearSyncInbox,
        connection: &LinearConnection,
        worker_id: &str,
    ) -> Result<(), SyncError> {
        let event = parse_agent_session_terminal_event(&row.payload)?;
        let existing = linear_agent_q::get_linear_agent_session(
            &self.state.pool,
            connection.workspace_id,
            connection.id,
            &event.session_id,
        )
        .await
        .map_err(SyncError::retry)?
        .ok_or_else(|| {
            SyncError::permanent(anyhow::anyhow!(
                "Linear Agent Session terminal event has no local correlation"
            ))
        })?;
        if existing.status == event.status && existing.last_event_id == row.delivery_id {
            return Ok(());
        }
        if existing.status.starts_with("dispatching") {
            return Err(SyncError::retry(anyhow::anyhow!(
                "Linear Agent Session dispatch is still being correlated"
            )));
        }
        let event_timestamp_ms = extract_event_timestamp_ms(&row.payload);
        if existing.last_event_id != row.delivery_id
            && matches!(
                (event_timestamp_ms, existing.last_event_at_ms),
                (Some(incoming), Some(previous)) if incoming <= previous
            )
        {
            return Ok(());
        }

        let claimed = linear_agent_q::claim_linear_agent_session_terminal(
            &self.state.pool,
            connection.workspace_id,
            connection.id,
            &event.session_id,
            &row.delivery_id,
            event_timestamp_ms,
            worker_id,
        )
        .await
        .map_err(SyncError::retry)?;
        if !claimed {
            let current = linear_agent_q::get_linear_agent_session(
                &self.state.pool,
                connection.workspace_id,
                connection.id,
                &event.session_id,
            )
            .await
            .map_err(SyncError::retry)?;
            if current.as_ref().is_some_and(|session| {
                matches!(
                    session.status.as_str(),
                    "completed" | "failed" | "cancelled"
                )
            }) {
                return Ok(());
            }
            return Err(SyncError::retry(anyhow::anyhow!(
                "Linear Agent Session has another dispatch in progress"
            )));
        }

        let manager = LinearTokenManager::from_state(&self.state)
            .map_err(|error| classify_token_error(error, "create Linear token manager"))?;
        manager
            .create_agent_activity(
                connection.id,
                &event.session_id,
                json!({"type": "response", "body": event.body}),
            )
            .await
            .map_err(|error| classify_token_error(error, "publish Linear Agent Session result"))?;

        let updated = linear_agent_q::mark_linear_agent_session_terminal(
            &self.state.pool,
            connection.workspace_id,
            connection.id,
            &event.session_id,
            &event.status,
            &row.delivery_id,
            event_timestamp_ms,
        )
        .await
        .map_err(SyncError::retry)?;
        if !updated {
            tracing::warn!(
                connection_id = %connection.id,
                session_id = %event.session_id,
                "Linear Agent Session correlation disappeared after terminal result"
            );
        }
        Ok(())
    }

    async fn patchbay_requester_id(
        &self,
        connection: &LinearConnection,
        linear_user_id: Option<&str>,
    ) -> Result<Uuid, SyncError> {
        if let Some(linear_user_id) = linear_user_id.filter(|id| !id.trim().is_empty()) {
            if let Some(binding) = linear_q::get_linear_member_binding_by_linear_user(
                &self.state.pool,
                connection.workspace_id,
                connection.id,
                linear_user_id,
            )
            .await
            .map_err(SyncError::retry)?
            {
                return Ok(binding.patchbay_user_id);
            }
            tracing::warn!(
                workspace_id = %connection.workspace_id,
                connection_id = %connection.id,
                linear_user_id,
                "Linear Agent Session creator has no member mapping; falling back to installation creator"
            );
        }
        Ok(connection.created_by_id)
    }

    async fn resume_waiting_agent_sessions(
        &self,
        connection: &LinearConnection,
        issue: &patchbay_db::models::Issue,
        agent_id: Uuid,
        source_event_id: &str,
    ) -> Result<(), SyncError> {
        let sessions = linear_agent_q::list_waiting_linear_agent_sessions(
            &self.state.pool,
            connection.workspace_id,
            issue.id,
        )
        .await
        .map_err(SyncError::retry)?;
        if sessions.is_empty() {
            return Ok(());
        }

        let mut transaction = self.state.pool.begin().await.map_err(SyncError::retry)?;
        for session in sessions {
            let delivery_id = format!(
                "linear-agent-selection-retry:{}:{}:{}",
                session.linear_session_id, agent_id, source_event_id
            );
            linear_agent_q::enqueue_linear_agent_session_retry(
                &mut transaction,
                connection.id,
                &delivery_id,
                &session,
                Some(agent_id),
            )
            .await
            .map_err(SyncError::retry)?;
        }
        transaction.commit().await.map_err(SyncError::retry)?;
        self.notify.notify_waiters();
        Ok(())
    }

    async fn resume_agent_sessions_awaiting_issue_link(
        &self,
        connection: &LinearConnection,
        linear_issue_id: &str,
    ) -> Result<(), SyncError> {
        if linear_q::find_linear_issue_link(
            &self.state.pool,
            connection.workspace_id,
            connection.id,
            linear_issue_id,
        )
        .await
        .map_err(SyncError::retry)?
        .is_none()
        {
            return Ok(());
        }
        let sessions = linear_agent_q::list_linear_agent_sessions_awaiting_issue_link(
            &self.state.pool,
            connection.workspace_id,
            connection.id,
            linear_issue_id,
        )
        .await
        .map_err(SyncError::retry)?;
        if sessions.is_empty() {
            return Ok(());
        }

        let mut transaction = self.state.pool.begin().await.map_err(SyncError::retry)?;
        for session in sessions {
            let delivery_id = format!(
                "linear-agent-issue-link-retry:{}",
                session.linear_session_id
            );
            linear_agent_q::enqueue_linear_agent_session_retry(
                &mut transaction,
                connection.id,
                &delivery_id,
                &session,
                None,
            )
            .await
            .map_err(SyncError::retry)?;
        }
        transaction.commit().await.map_err(SyncError::retry)?;
        self.notify.notify_waiters();
        Ok(())
    }

    async fn patchbay_issue_url(
        &self,
        issue: &patchbay_db::models::Issue,
    ) -> Result<Option<String>, SyncError> {
        let base = self.state.public_config.frontend_app_url.trim();
        if base.is_empty() {
            return Ok(None);
        }
        let workspace = workspace_q::get_workspace(&self.state.pool, issue.workspace_id)
            .await
            .map_err(SyncError::retry)?
            .ok_or_else(|| SyncError::permanent(anyhow::anyhow!("Patchbay Workspace not found")))?;
        let identifier = if workspace.issue_prefix.trim().is_empty() {
            issue.number.to_string()
        } else {
            format!("{}-{}", workspace.issue_prefix, issue.number)
        };
        Ok(Some(format!(
            "{}/{}/issues/{identifier}",
            base.trim_end_matches('/'),
            workspace.slug.trim_matches('/')
        )))
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
        updated_from: Option<&Value>,
    ) -> Result<(), SyncError> {
        let was_unlinked = existing_link.is_none();
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
        if binding.linear_project_id != linear_project_id {
            return Err(SyncError::permanent(anyhow::anyhow!(
                "Linear issue project does not match its binding"
            )));
        }
        let agent_decision = self
            .agent_label_decision_for_issue(connection, &binding, &remote)
            .await?;
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
        let event_timestamp_ms = event_timestamp_ms.or(Some(remote_updated_at.timestamp_millis()));
        if is_out_of_order(existing_link.as_ref(), event_timestamp_ms) {
            return Ok(());
        }
        let remote_owner_id = self
            .remote_owner_id(connection, remote.assignee.as_ref())
            .await?;
        let snapshot = remote_sync_snapshot(&remote, &mapped_status, &priority, remote_owner_id);
        if let Some(link) = existing_link {
            let destination_project_id = (needs_rebind && link.binding_id != binding.id)
                .then_some(binding.patchbay_project_id);
            return self
                .apply_existing_remote_issue(ExistingRemoteIssueInput {
                    connection,
                    binding,
                    link,
                    remote,
                    remote_snapshot: snapshot,
                    source_event_id,
                    event_timestamp_ms,
                    remote_updated_at,
                    updated_from,
                    agent_decision,
                    destination_project_id,
                })
                .await;
        }
        let mapped_category = patchbay_service::issue_status::effective(
            &self.state.pool,
            connection.workspace_id,
            &mapped_status,
        )
        .await;
        let import_status =
            if import_status_is_inadmissible(&mapped_category, None, agent_decision.agent_id) {
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
            description: Some(strip_patchbay_issue_marker(remote.description.as_deref())),
            status: Some(import_status.clone()),
            priority: Some(priority.clone()),
            due_date: Some(due_date),
            project_id: Some(Some(binding.patchbay_project_id)),
            owner_type: Some(remote_owner_id.map(|_| "member".to_string())),
            owner_id: Some(remote_owner_id),
            executor_type: Some(agent_decision.agent_id.map(|_| "agent".to_string())),
            executor_id: Some(agent_decision.agent_id),
        };
        let issue = {
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
                            patch: remote_patch.clone(),
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
                            description: strip_patchbay_issue_marker(remote.description.as_deref()),
                            status: import_status,
                            priority,
                            creator_type: "member".to_string(),
                            creator_id: connection.created_by_id,
                            project_id: Some(binding.patchbay_project_id),
                            due_date,
                            owner_type: remote_owner_id.map(|_| "member".to_string()),
                            owner_id: remote_owner_id,
                            executor_type: agent_decision.agent_id.map(|_| "agent".to_string()),
                            executor_id: agent_decision.agent_id,
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

        let link = if let Some(link) = linear_q::find_linear_issue_link(
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
                    remote_updated_at: Some(remote_updated_at),
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
        let last_event_at_ms = event_timestamp_ms.or(link.last_remote_event_at_ms);
        let last_event_id = event_timestamp_ms
            .map(|_| source_event_id)
            .or(link.last_remote_event_id.as_deref());
        let updated = linear_q::update_linear_issue_link(
            &self.state.pool,
            &linear_q::LinearIssueLinkUpdate {
                link_id: link.id,
                workspace_id: connection.workspace_id,
                last_common_snapshot: &snapshot,
                remote_updated_at: Some(remote_updated_at),
                last_remote_event_at_ms: last_event_at_ms,
                last_remote_event_id: last_event_id,
                sync_status: "active",
            },
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
        activity::create_activity(
            &self.state.pool,
            connection.workspace_id,
            issue.id,
            Some("system"),
            None,
            "linear_sync_applied",
            &json!({
                "source": "linear",
                "source_event_id": source_event_id,
                "connection_id": connection.id,
                "binding_id": binding.id,
                "linear_issue_id": remote.id,
                "linear_identifier": remote.identifier,
                "remote_updated_at": remote.updated_at,
                "event_timestamp_ms": event_timestamp_ms,
                "imported": was_unlinked,
            }),
            linear_sync_activity_id(connection.id, issue.id, source_event_id, "applied"),
        )
        .await
        .map_err(SyncError::retry)?;
        Ok(())
    }

    async fn remote_owner_id(
        &self,
        connection: &LinearConnection,
        assignee: Option<&LinearRemoteUser>,
    ) -> Result<Option<Uuid>, SyncError> {
        let Some(assignee) = assignee else {
            return Ok(None);
        };
        let binding = linear_q::get_linear_member_binding_by_linear_user(
            &self.state.pool,
            connection.workspace_id,
            connection.id,
            &assignee.id,
        )
        .await
        .map_err(SyncError::retry)?;
        binding
            .map(|binding| Some(binding.patchbay_user_id))
            .ok_or_else(|| {
                SyncError::permanent(anyhow::anyhow!(
                    "Linear human assignee has no Patchbay member mapping"
                ))
            })
    }

    async fn agent_label_decision_for_issue(
        &self,
        connection: &LinearConnection,
        binding: &LinearProjectBinding,
        remote: &LinearRemoteIssue,
    ) -> Result<AgentLabelDecision, SyncError> {
        if !self
            .state
            .linear_agent_bridge_enabled(connection.workspace_id)
        {
            return Ok(AgentLabelDecision {
                configured: false,
                agent_id: None,
            });
        }
        let decision = agent_label_decision(binding, &remote.labels.nodes)?;
        if let Some(agent_id) = decision.agent_id {
            let agent = agent_q::get_agent_in_workspace(
                &self.state.pool,
                agent_id,
                connection.workspace_id,
            )
            .await
            .map_err(SyncError::retry)?;
            if agent
                .as_ref()
                .map(|agent| agent.archived_at.is_some())
                .unwrap_or(true)
            {
                tracing::warn!(
                    workspace_id = %connection.workspace_id,
                    connection_id = %connection.id,
                    agent_id = %agent_id,
                    "Linear Agent label maps to an unavailable Patchbay Agent; treating it as unassigned"
                );
                return Ok(AgentLabelDecision {
                    configured: true,
                    agent_id: None,
                });
            }
        }
        Ok(decision)
    }

    async fn apply_existing_remote_issue(
        &self,
        input: ExistingRemoteIssueInput<'_>,
    ) -> Result<(), SyncError> {
        let ExistingRemoteIssueInput {
            connection,
            binding,
            link,
            remote,
            remote_snapshot,
            source_event_id,
            event_timestamp_ms,
            remote_updated_at,
            updated_from,
            agent_decision,
            destination_project_id,
        } = input;
        if event_timestamp_ms.is_none()
            && link
                .remote_updated_at
                .is_some_and(|previous| remote_updated_at <= previous)
        {
            return Ok(());
        }
        let issue = issue_q::get_issue_in_workspace(
            &self.state.pool,
            link.patchbay_issue_id,
            connection.workspace_id,
        )
        .await
        .map_err(SyncError::retry)?
        .ok_or_else(|| SyncError::permanent(anyhow::anyhow!("Patchbay Issue not found")))?;
        let base_snapshot = normalized_base_snapshot(&link.last_common_snapshot, &binding)?;
        let local_snapshot = local_sync_snapshot(&issue);
        let merge = merge_sync_snapshots_with_updated_from(
            &base_snapshot,
            &local_snapshot,
            &remote_snapshot,
            updated_from,
        );
        let last_event_at_ms = event_timestamp_ms.or(link.last_remote_event_at_ms);
        let last_event_id = event_timestamp_ms
            .map(|_| source_event_id)
            .or(link.last_remote_event_id.as_deref());
        let executor_changed = agent_decision.configured
            && (issue.executor_type.as_deref() != agent_decision.agent_id.map(|_| "agent")
                || issue.executor_id != agent_decision.agent_id);

        let mut transaction = self.state.pool.begin().await.map_err(SyncError::retry)?;
        let (applied, agent_selection_required) = if merge.remote_changed
            || destination_project_id.is_some()
            || (merge.conflicts.is_empty() && executor_changed)
        {
            let mut patch = if merge.remote_changed {
                external_patch_from_snapshot(&merge.merged)?
            } else {
                ExternalIssuePatch::default()
            };
            patch.project_id = destination_project_id.map(Some);
            if merge.conflicts.is_empty() && executor_changed {
                patch.executor_type = Some(agent_decision.agent_id.map(|_| "agent".to_string()));
                patch.executor_id = Some(agent_decision.agent_id);
            }
            let mut fallback_patch = patch.clone();
            fallback_patch.executor_type = None;
            fallback_patch.executor_id = None;
            let apply_result = self
                .state
                .issues
                .apply_external_patch_in_transaction(
                    &mut transaction,
                    connection.workspace_id,
                    issue.id,
                    IssueCommand::ApplyExternalPatch {
                        source: ExternalSource::Linear,
                        source_event_id: source_event_id.to_string(),
                        expected_revision: Some(issue.revision),
                        suppress_external_outbox: true,
                        patch,
                    },
                )
                .await;
            match apply_result {
                Ok(applied) => (Some(applied), false),
                Err(ExternalIssueError::ActiveExecutorRequired) if executor_changed => {
                    let applied = if merge.remote_changed || destination_project_id.is_some() {
                        Some(
                            self.state
                                .issues
                                .apply_external_patch_in_transaction(
                                    &mut transaction,
                                    connection.workspace_id,
                                    issue.id,
                                    IssueCommand::ApplyExternalPatch {
                                        source: ExternalSource::Linear,
                                        source_event_id: source_event_id.to_string(),
                                        expected_revision: Some(issue.revision),
                                        suppress_external_outbox: true,
                                        patch: fallback_patch,
                                    },
                                )
                                .await
                                .map_err(|error| {
                                    classify_external_error(error, "apply merged Linear Issue")
                                })?,
                        )
                    } else {
                        None
                    };
                    tracing::warn!(
                        issue_id = %issue.id,
                        "Linear Agent label requires an executor compatible with the current Issue status"
                    );
                    (applied, true)
                }
                Err(error) => {
                    return Err(classify_external_error(error, "apply merged Linear Issue"));
                }
            }
        } else {
            (None, false)
        };
        if destination_project_id.is_some() {
            let rebound = linear_q::rebind_linear_issue_link(
                &mut *transaction,
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
        }

        if !merge.conflicts.is_empty() {
            for conflict in &merge.conflicts {
                linear_q::create_linear_sync_conflict(
                    &mut *transaction,
                    &linear_q::LinearSyncConflictInput {
                        id: Uuid::now_v7(),
                        workspace_id: connection.workspace_id,
                        binding_id: binding.id,
                        link_id: link.id,
                        patchbay_issue_id: issue.id,
                        linear_issue_id: &remote.id,
                        field: &conflict.field,
                        base_value: &conflict.base_value,
                        local_value: &conflict.local_value,
                        remote_value: &conflict.remote_value,
                        source_event_id,
                        source_event_at_ms: event_timestamp_ms,
                    },
                )
                .await
                .map_err(SyncError::retry)?;
            }
            let updated = linear_q::update_linear_issue_link(
                &mut *transaction,
                &linear_q::LinearIssueLinkUpdate {
                    link_id: link.id,
                    workspace_id: connection.workspace_id,
                    last_common_snapshot: &merge.common,
                    remote_updated_at: Some(remote_updated_at),
                    last_remote_event_at_ms: last_event_at_ms,
                    last_remote_event_id: last_event_id,
                    sync_status: "conflict",
                },
            )
            .await
            .map_err(SyncError::retry)?;
            if !updated {
                return Err(SyncError::retry(anyhow::anyhow!(
                    "Linear Issue Link disappeared while recording conflict"
                )));
            }
            transaction.commit().await.map_err(SyncError::retry)?;
            if let Some(applied) = &applied {
                self.state
                    .issues
                    .publish_external_issue_apply(applied)
                    .await;
            }
            return Ok(());
        }

        let updated = linear_q::update_linear_issue_link(
            &mut *transaction,
            &linear_q::LinearIssueLinkUpdate {
                link_id: link.id,
                workspace_id: connection.workspace_id,
                last_common_snapshot: &merge.common,
                remote_updated_at: Some(remote_updated_at),
                last_remote_event_at_ms: last_event_at_ms,
                last_remote_event_id: last_event_id,
                sync_status: if agent_selection_required {
                    "agent_selection_required"
                } else {
                    "active"
                },
            },
        )
        .await
        .map_err(SyncError::retry)?;
        if !updated {
            return Err(SyncError::retry(anyhow::anyhow!(
                "Linear Issue Link disappeared after merge"
            )));
        }
        transaction.commit().await.map_err(SyncError::retry)?;
        if let Some(applied) = &applied {
            self.state
                .issues
                .publish_external_issue_apply(applied)
                .await;
        }
        if agent_selection_required {
            return Ok(());
        }
        if let Some(agent_id) = agent_decision.agent_id {
            self.resume_waiting_agent_sessions(connection, &issue, agent_id, source_event_id)
                .await?;
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
            &linear_q::LinearIssueLinkUpdate {
                link_id: link.id,
                workspace_id: connection.workspace_id,
                last_common_snapshot: &snapshot,
                remote_updated_at: link.remote_updated_at,
                last_remote_event_at_ms: event_at,
                last_remote_event_id: event_id,
                sync_status: "deleted",
            },
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
        activity::create_activity(
            &self.state.pool,
            connection.workspace_id,
            link.patchbay_issue_id,
            Some("system"),
            None,
            "linear_sync_removed",
            &json!({
                "source": "linear",
                "source_event_id": source_event_id,
                "connection_id": connection.id,
                "binding_id": link.binding_id,
                "linear_issue_id": linear_issue_id,
                "event_timestamp_ms": event_timestamp_ms,
            }),
            linear_sync_activity_id(
                connection.id,
                link.patchbay_issue_id,
                source_event_id,
                "removed",
            ),
        )
        .await
        .map_err(SyncError::retry)?;
        Ok(())
    }
}

const SHARED_SYNC_FIELDS: [&str; 6] = [
    "title",
    "description",
    "priority",
    "status",
    "due_date",
    "owner_id",
];

#[derive(Debug)]
struct SyncConflictValue {
    field: String,
    base_value: Value,
    local_value: Value,
    remote_value: Value,
}

#[derive(Debug)]
struct SyncMergePlan {
    common: Value,
    conflicts: Vec<SyncConflictValue>,
    merged: Value,
    remote_changed: bool,
}

fn local_sync_snapshot(issue: &patchbay_db::models::Issue) -> Value {
    json!({
        "title": issue.title,
        "description": issue.description,
        "priority": issue.priority,
        "status": issue.status,
        "due_date": issue.due_date.map(|date| date.format("%Y-%m-%d").to_string()),
        "owner_id": (issue.owner_type.as_deref() == Some("member"))
            .then(|| issue.owner_id.map(|id| id.to_string()))
            .flatten(),
    })
}

fn remote_sync_snapshot(
    remote: &LinearRemoteIssue,
    status: &str,
    priority: &str,
    owner_id: Option<Uuid>,
) -> Value {
    json!({
        "title": remote.title,
        "description": strip_patchbay_issue_marker(remote.description.as_deref()),
        "priority": priority,
        "status": status,
        "due_date": remote.due_date,
        "owner_id": owner_id.map(|id| id.to_string()),
    })
}

fn normalized_base_snapshot(
    snapshot: &Value,
    binding: &LinearProjectBinding,
) -> Result<Value, SyncError> {
    let object = snapshot.as_object().ok_or_else(|| {
        SyncError::permanent(anyhow::anyhow!(
            "Linear Issue Link common snapshot is not an object"
        ))
    })?;
    let field = |name: &str| object.get(name).cloned().unwrap_or(Value::Null);
    let priority = match object.get("priority") {
        Some(Value::String(value)) => Value::String(value.clone()),
        Some(Value::Number(value)) => {
            Value::String(map_remote_priority(value.as_i64().ok_or_else(|| {
                SyncError::permanent(anyhow::anyhow!(
                    "Linear Issue Link priority snapshot is invalid"
                ))
            })?)?)
        }
        Some(Value::Null) | None => Value::Null,
        Some(_) => {
            return Err(SyncError::permanent(anyhow::anyhow!(
                "Linear Issue Link priority snapshot is invalid"
            )))
        }
    };
    let status = if let Some(value) = object.get("status").and_then(Value::as_str) {
        Value::String(value.to_string())
    } else if let Some(state) = object.get("state").and_then(Value::as_object) {
        let state_id = state.get("id").and_then(Value::as_str);
        let state_type = state.get("type").and_then(Value::as_str);
        match (state_id, state_type) {
            (Some(state_id), _) => binding
                .status_mapping
                .as_object()
                .and_then(|mapping| mapping.get(state_id))
                .and_then(Value::as_str)
                .map(|value| Value::String(value.to_string()))
                .unwrap_or(Value::Null),
            (None, Some(state_type)) => Value::String(
                match state_type {
                    "backlog" => "backlog",
                    "unstarted" => "todo",
                    "started" => "in_progress",
                    "completed" => "done",
                    "canceled" | "cancelled" => "cancelled",
                    _ => "",
                }
                .to_string(),
            ),
            (None, None) => Value::Null,
        }
    } else {
        Value::Null
    };
    let description = object
        .get("description")
        .map(|value| match value {
            Value::String(value) => strip_patchbay_issue_marker(Some(value))
                .map(Value::String)
                .unwrap_or(Value::Null),
            Value::Null => Value::Null,
            _ => Value::Null,
        })
        .unwrap_or(Value::Null);
    let due_date = object
        .get("due_date")
        .or_else(|| object.get("dueDate"))
        .cloned()
        .unwrap_or(Value::Null);
    let owner_id = object
        .get("owner_id")
        .or_else(|| object.get("assignee").and_then(|value| value.get("id")))
        .cloned()
        .unwrap_or(Value::Null);
    Ok(json!({
        "title": field("title"),
        "description": description,
        "priority": priority,
        "status": status,
        "due_date": due_date,
        "owner_id": owner_id,
    }))
}

fn merge_sync_snapshots(base: &Value, local: &Value, remote: &Value) -> SyncMergePlan {
    merge_sync_snapshots_with_updated_from(base, local, remote, None)
}

fn merge_sync_snapshots_with_updated_from(
    base: &Value,
    local: &Value,
    remote: &Value,
    updated_from: Option<&Value>,
) -> SyncMergePlan {
    let mut common = serde_json::Map::new();
    let mut merged = serde_json::Map::new();
    let mut conflicts = Vec::new();
    let mut remote_changed = false;
    for field in SHARED_SYNC_FIELDS {
        let base_value = base.get(field).cloned().unwrap_or(Value::Null);
        let local_value = local.get(field).cloned().unwrap_or(Value::Null);
        let remote_value = remote.get(field).cloned().unwrap_or(Value::Null);
        // Linear's updatedFrom contains the previous values of the properties
        // changed by this event. The full Issue fetch remains authoritative for
        // the current values, while updatedFrom prevents a mapped/no-op value
        // from being mistaken for an unchanged remote field.
        let remote_field_changed =
            remote_value != base_value || updated_from_changed_field(updated_from, field);
        if local_value == remote_value {
            common.insert(field.to_string(), local_value.clone());
            merged.insert(field.to_string(), local_value);
        } else if local_value == base_value && remote_field_changed {
            remote_changed = true;
            common.insert(field.to_string(), remote_value.clone());
            merged.insert(field.to_string(), remote_value);
        } else if remote_value == base_value && !remote_field_changed {
            merged.insert(field.to_string(), local_value.clone());
            common.insert(field.to_string(), base_value);
        } else {
            remote_changed = true;
            // Keep both plans complete. Conflicting fields stay local in the
            // applied Issue and retain the previous common value as their
            // merge base while independent remote changes can proceed.
            merged.insert(field.to_string(), local_value.clone());
            common.insert(field.to_string(), base_value.clone());
            conflicts.push(SyncConflictValue {
                field: field.to_string(),
                base_value,
                local_value,
                remote_value,
            });
        }
    }
    SyncMergePlan {
        common: Value::Object(common),
        conflicts,
        merged: Value::Object(merged),
        remote_changed,
    }
}

fn required_snapshot_string(snapshot: &Value, field: &str) -> Result<String, SyncError> {
    snapshot
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| SyncError::permanent(anyhow::anyhow!("merged Linear {field} is invalid")))
}

fn external_patch_from_snapshot(snapshot: &Value) -> Result<ExternalIssuePatch, SyncError> {
    let description = match snapshot.get("description") {
        Some(Value::String(value)) => Some(Some(value.clone())),
        Some(Value::Null) | None => Some(None),
        Some(_) => {
            return Err(SyncError::permanent(anyhow::anyhow!(
                "merged Linear description is invalid"
            )))
        }
    };
    let due_date = match snapshot.get("due_date") {
        Some(Value::String(value)) if !value.trim().is_empty() => {
            Some(Some(NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(
                |_| SyncError::permanent(anyhow::anyhow!("merged Linear due date is invalid")),
            )?))
        }
        Some(Value::Null) | None => Some(None),
        Some(_) => {
            return Err(SyncError::permanent(anyhow::anyhow!(
                "merged Linear due date is invalid"
            )))
        }
    };
    let owner_id = match snapshot.get("owner_id") {
        Some(Value::String(value)) if !value.trim().is_empty() => {
            Some(Some(value.parse::<Uuid>().map_err(|_| {
                SyncError::permanent(anyhow::anyhow!("merged Linear owner is invalid"))
            })?))
        }
        Some(Value::Null) | None => Some(None),
        Some(_) => {
            return Err(SyncError::permanent(anyhow::anyhow!(
                "merged Linear owner is invalid"
            )))
        }
    };
    let owner_type = owner_id
        .as_ref()
        .map(|value| value.as_ref().map(|_| "member".to_string()));
    Ok(ExternalIssuePatch {
        title: Some(required_snapshot_string(snapshot, "title")?),
        description,
        status: Some(required_snapshot_string(snapshot, "status")?),
        priority: Some(required_snapshot_string(snapshot, "priority")?),
        due_date,
        project_id: None,
        owner_type,
        owner_id,
        executor_type: None,
        executor_id: None,
    })
}

fn classify_external_error(error: ExternalIssueError, context: &str) -> SyncError {
    match error {
        error @ (ExternalIssueError::InvalidStatus
        | ExternalIssueError::InvalidPriority
        | ExternalIssueError::InvalidOwner
        | ExternalIssueError::InvalidExecutor
        | ExternalIssueError::ActiveExecutorRequired
        | ExternalIssueError::ReviewReviewerRequired
        | ExternalIssueError::ProjectNotFound
        | ExternalIssueError::NotFound) => {
            SyncError::permanent(anyhow::anyhow!("{context}: {error}"))
        }
        ExternalIssueError::RevisionConflict { .. } => {
            SyncError::retry(anyhow::anyhow!("{context}: {error}"))
        }
        other => SyncError::retry(anyhow::anyhow!("{context}: {other}")),
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

fn import_status_is_inadmissible(
    category: &str,
    issue: Option<&Issue>,
    incoming_agent_id: Option<Uuid>,
) -> bool {
    let has_executor = incoming_agent_id.is_some()
        || issue.is_some_and(|issue| issue.executor_type.is_some() && issue.executor_id.is_some());
    let has_reviewer =
        issue.is_some_and(|issue| issue.reviewer_type.is_some() && issue.reviewer_id.is_some());
    let same_reviewer_and_executor = issue
        .is_some_and(|issue| issue.reviewer_id.is_some() && issue.reviewer_id == issue.executor_id);
    (patchbay_service::issue_status::requires_executor(category) && !has_executor)
        || (patchbay_service::issue_status::requires_reviewer(category)
            && (!has_reviewer || same_reviewer_and_executor))
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
    .find_map(Value::as_str)
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

fn extract_updated_from(payload: &Value) -> Option<&Value> {
    payload
        .get("updatedFrom")
        .filter(|value| value.as_object().is_some())
}

fn updated_from_changed_field(updated_from: Option<&Value>, field: &str) -> bool {
    let Some(updated_from) = updated_from.and_then(Value::as_object) else {
        return false;
    };
    let aliases: &[&str] = match field {
        "title" => &["title"],
        "description" => &["description"],
        "priority" => &["priority"],
        "status" => &["state", "status"],
        "due_date" => &["dueDate", "due_date"],
        "owner_id" => &["assignee", "assigneeId", "owner_id", "ownerId"],
        _ => &[],
    };
    aliases
        .iter()
        .any(|alias| updated_from.contains_key(*alias))
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

fn map_local_priority(priority: &str) -> Result<i64, SyncError> {
    match priority {
        "none" => Ok(0),
        "urgent" => Ok(1),
        "high" => Ok(2),
        "medium" => Ok(3),
        "low" => Ok(4),
        other => Err(SyncError::permanent(anyhow::anyhow!(
            "Patchbay Issue priority is unsupported: {other}"
        ))),
    }
}

fn map_local_status(binding: &LinearProjectBinding, status: &str) -> Option<String> {
    binding.status_mapping.as_object().and_then(|mapping| {
        mapping
            .iter()
            .find_map(|(linear_state_id, patchbay_status)| {
                (patchbay_status.as_str() == Some(status)).then(|| linear_state_id.clone())
            })
    })
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
        LinearTokenError::MutationRejected(message) => {
            SyncError::permanent(anyhow::anyhow!("{context}: {message}"))
        }
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
        agent_label_decision, agent_label_ids_for_issue, extract_event_timestamp_ms,
        extract_issue_id, extract_updated_from, import_status_is_inadmissible, inbound_enabled,
        is_out_of_order, map_local_priority, map_local_status, map_remote_priority,
        map_remote_status, merge_sync_snapshots, merge_sync_snapshots_with_updated_from,
        parse_agent_session_event, parse_agent_session_terminal_event, parse_remote_timestamp,
        remote_sync_snapshot, retry_delay,
    };
    use crate::linear::{LinearRemoteIssue, LinearRemoteLabel, LinearRemoteState};

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
        assert_eq!(
            extract_updated_from(&json!({"updatedFrom": {"title": "old"}}))
                .and_then(|value| value.get("title")),
            Some(&json!("old"))
        );
        assert_eq!(extract_updated_from(&json!({"updatedFrom": null})), None);
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
    fn local_priority_and_status_mapping_are_explicit() {
        assert_eq!(map_local_priority("none").unwrap(), 0);
        assert_eq!(map_local_priority("urgent").unwrap(), 1);
        assert_eq!(map_local_priority("low").unwrap(), 4);
        assert!(map_local_priority("provider_added_priority").is_err());

        let binding = binding(json!({
            "linear-started": "in_progress",
            "linear-done": "done"
        }));
        assert_eq!(
            map_local_status(&binding, "in_progress").as_deref(),
            Some("linear-started")
        );
        assert_eq!(map_local_status(&binding, "todo"), None);
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
    fn delegated_import_keeps_executor_required_mapped_status() {
        assert!(import_status_is_inadmissible("in_progress", None, None));
        assert!(!import_status_is_inadmissible(
            "in_progress",
            None,
            Some(Uuid::now_v7())
        ));
        assert!(import_status_is_inadmissible(
            "in_review",
            None,
            Some(Uuid::now_v7())
        ));
    }

    #[test]
    fn remote_timestamp_requires_rfc3339() {
        assert!(parse_remote_timestamp("2026-08-31T12:00:00Z").is_ok());
        assert!(parse_remote_timestamp("not-a-timestamp").is_err());
    }

    #[test]
    fn agent_label_mapping_collects_all_labels_and_ignores_unrelated_order() {
        let agent_id = Uuid::now_v7();
        let mut binding = binding(json!({}));
        binding.agent_label_mapping = json!({
            "group_id": "agent-group",
            "labels": {"agent-backend": agent_id.to_string()}
        });
        let labels = vec![
            LinearRemoteLabel {
                id: "bug".to_string(),
            },
            LinearRemoteLabel {
                id: "agent-backend".to_string(),
            },
        ];
        assert_eq!(
            agent_label_decision(&binding, &labels).unwrap(),
            super::AgentLabelDecision {
                configured: true,
                agent_id: Some(agent_id),
            }
        );

        assert_eq!(
            agent_label_decision(
                &binding,
                &[LinearRemoteLabel {
                    id: "bug".to_string(),
                }]
            )
            .unwrap(),
            super::AgentLabelDecision {
                configured: true,
                agent_id: None,
            }
        );
    }

    #[test]
    fn agent_label_mapping_rejects_multiple_selected_values() {
        let first = Uuid::now_v7();
        let second = Uuid::now_v7();
        let mut binding = binding(json!({}));
        binding.agent_label_mapping = json!({
            "group_id": "agent-group",
            "labels": {
                "agent-backend": first.to_string(),
                "agent-frontend": second.to_string()
            }
        });
        let error = agent_label_decision(
            &binding,
            &[
                LinearRemoteLabel {
                    id: "agent-backend".to_string(),
                },
                LinearRemoteLabel {
                    id: "agent-frontend".to_string(),
                },
            ],
        )
        .unwrap_err();
        assert!(error.message().contains("more than one selected"));
    }

    #[test]
    fn outbound_agent_labels_preserve_unrelated_labels_and_clear_stale_value() {
        let agent_id = Uuid::now_v7();
        let mut binding = binding(json!({}));
        binding.agent_label_mapping = json!({
            "group_id": "agent-group",
            "labels": {"agent-backend": agent_id.to_string()}
        });
        let existing = vec![
            LinearRemoteLabel {
                id: "bug".to_string(),
            },
            LinearRemoteLabel {
                id: "agent-backend".to_string(),
            },
        ];
        assert_eq!(
            agent_label_ids_for_issue(&binding, Some("agent"), Some(agent_id), &existing).unwrap(),
            Some(vec!["bug".to_string(), "agent-backend".to_string()])
        );
        assert_eq!(
            agent_label_ids_for_issue(&binding, None, None, &existing).unwrap(),
            Some(vec!["bug".to_string()])
        );

        let unmapped_agent = Uuid::now_v7();
        assert!(agent_label_ids_for_issue(
            &binding,
            Some("agent"),
            Some(unmapped_agent),
            &existing,
        )
        .unwrap_err()
        .message()
        .contains("no label mapping"));

        binding.agent_label_mapping["default_agent_id"] = Value::String(unmapped_agent.to_string());
        assert_eq!(
            agent_label_ids_for_issue(&binding, Some("agent"), Some(unmapped_agent), &existing,)
                .unwrap(),
            Some(vec!["bug".to_string()])
        );

        binding.agent_label_mapping = json!({});
        assert_eq!(
            agent_label_ids_for_issue(&binding, Some("agent"), Some(agent_id), &existing).unwrap(),
            None
        );
    }

    #[test]
    fn agent_session_event_parser_accepts_created_and_prompted_shapes() {
        let event = parse_agent_session_event(&json!({
            "action": "prompted",
            "agentSession": {
                "id": "session-1",
                "issue": {"id": "issue-1"},
                "promptContext": "continue the implementation",
                "creatorId": "018f0d7f-3b4f-7b1a-8c4e-7baf8ecbda40"
            },
            "agentActivity": {"content": {"body": "Please continue"}}
        }))
        .unwrap();
        assert_eq!(event.session_id, "session-1");
        assert_eq!(event.linear_issue_id, "issue-1");
        assert_eq!(event.action, "prompted");
        assert_eq!(event.prompt_body.as_deref(), Some("Please continue"));
        assert_eq!(
            event.requester_user_id,
            Some("018f0d7f-3b4f-7b1a-8c4e-7baf8ecbda40".to_string())
        );

        let retry = parse_agent_session_event(&json!({
            "action": "prompted",
            "agentSession": {
                "id": "session-1",
                "issue": {"id": "issue-1"},
                "promptContext": "original context",
                "promptBody": "original prompt body",
                "creatorId": "linear-user-1"
            }
        }))
        .unwrap();
        assert_eq!(retry.prompt_body.as_deref(), Some("original prompt body"));
        assert_eq!(retry.requester_user_id.as_deref(), Some("linear-user-1"));
    }

    #[test]
    fn agent_session_terminal_parser_keeps_result_or_failure_context() {
        let completed = parse_agent_session_terminal_event(&json!({
            "status": "completed",
            "agentSession": {"id": "session-1"},
            "result": {"output": "Implemented the change"}
        }))
        .unwrap();
        assert_eq!(completed.session_id, "session-1");
        assert_eq!(completed.status, "completed");
        assert_eq!(completed.body, "Implemented the change");

        let failed = parse_agent_session_terminal_event(&json!({
            "status": "failed",
            "agentSession": {"id": "session-1"},
            "error": "provider unavailable"
        }))
        .unwrap();
        assert_eq!(failed.body, "Patchbay Agent failed: provider unavailable");
    }

    #[test]
    fn post_mutation_snapshot_uses_canonical_shared_values() {
        let remote: LinearRemoteIssue = serde_json::from_value(json!({
            "id": "linear-issue",
            "identifier": "LIN-1",
            "title": "Canonical snapshot",
            "description": "Description\n\n<!-- patchbay:issue:00000000-0000-0000-0000-000000000000 -->",
            "priority": 2,
            "state": {
                "id": "unmapped-linear-state",
                "name": "Started",
                "type": "started"
            },
            "dueDate": null,
            "project": {"id": "linear-project"},
            "updatedAt": "2026-08-31T12:00:00Z",
            "team": {"id": "linear-team"},
            "assignee": {"id": "linear-user"},
            "labels": {
                "nodes": [],
                "pageInfo": {"hasNextPage": false, "endCursor": null}
            }
        }))
        .expect("remote issue fixture should deserialize");
        let patchbay_owner_id = Uuid::now_v7();

        let snapshot =
            remote_sync_snapshot(&remote, "in_progress", "high", Some(patchbay_owner_id));

        assert_eq!(snapshot["status"], "in_progress");
        assert_eq!(snapshot["priority"], "high");
        assert_eq!(snapshot["owner_id"], patchbay_owner_id.to_string());
        assert_eq!(snapshot["description"], "Description");
        assert!(snapshot.get("state").is_none());
        assert!(snapshot.get("assignee").is_none());
    }

    #[test]
    fn three_way_merge_keeps_disjoint_changes_and_advances_common_fields() {
        let base = json!({
            "title": "Base",
            "description": "Base description",
            "priority": "medium",
            "status": "todo",
            "due_date": null,
            "owner_id": null
        });
        let local = json!({
            "title": "Local title",
            "description": "Base description",
            "priority": "medium",
            "status": "todo",
            "due_date": null,
            "owner_id": null
        });
        let remote = json!({
            "title": "Base",
            "description": "Remote description",
            "priority": "medium",
            "status": "todo",
            "due_date": null,
            "owner_id": null
        });

        let plan = merge_sync_snapshots(&base, &local, &remote);
        assert!(plan.conflicts.is_empty());
        assert_eq!(plan.merged["title"], "Local title");
        assert_eq!(plan.merged["description"], "Remote description");
        assert_eq!(plan.common["title"], "Base");
        assert_eq!(plan.common["description"], "Remote description");
        assert!(plan.remote_changed);
    }

    #[test]
    fn three_way_merge_records_same_field_conflicts_without_overwriting() {
        let base = json!({
            "title": "Base",
            "description": null,
            "priority": "medium",
            "status": "todo",
            "due_date": null,
            "owner_id": null
        });
        let local = json!({
            "title": "Local",
            "description": null,
            "priority": "medium",
            "status": "todo",
            "due_date": null,
            "owner_id": null
        });
        let remote = json!({
            "title": "Remote",
            "description": null,
            "priority": "medium",
            "status": "todo",
            "due_date": null,
            "owner_id": null
        });

        let plan = merge_sync_snapshots(&base, &local, &remote);
        assert_eq!(plan.conflicts.len(), 1);
        assert_eq!(plan.conflicts[0].field, "title");
        assert_eq!(plan.conflicts[0].base_value, "Base");
        assert_eq!(plan.conflicts[0].local_value, "Local");
        assert_eq!(plan.conflicts[0].remote_value, "Remote");
        assert_eq!(plan.merged["title"], "Local");
        assert_eq!(plan.common["title"], "Base");
    }

    #[test]
    fn three_way_merge_keeps_nonconflicting_remote_changes_with_a_conflict() {
        let base = json!({
            "title": "Base",
            "description": null,
            "priority": "medium",
            "status": "todo",
            "due_date": null,
            "owner_id": null
        });
        let local = json!({
            "title": "Local",
            "description": null,
            "priority": "medium",
            "status": "todo",
            "due_date": null,
            "owner_id": null
        });
        let remote = json!({
            "title": "Remote",
            "description": "Remote description",
            "priority": "medium",
            "status": "todo",
            "due_date": null,
            "owner_id": null
        });

        let plan = merge_sync_snapshots(&base, &local, &remote);
        assert_eq!(plan.conflicts.len(), 1);
        assert_eq!(plan.merged["title"], "Local");
        assert_eq!(plan.merged["description"], "Remote description");
        assert_eq!(plan.common["title"], "Base");
        assert_eq!(plan.common["description"], "Remote description");
    }

    #[test]
    fn updated_from_marks_a_remote_change_even_when_normalization_matches_base() {
        let base = json!({
            "title": "Base",
            "description": null,
            "priority": "medium",
            "status": "todo",
            "due_date": null,
            "owner_id": null
        });
        let local = json!({
            "title": "Local",
            "description": null,
            "priority": "medium",
            "status": "todo",
            "due_date": null,
            "owner_id": null
        });
        let remote = json!({
            "title": "Base",
            "description": null,
            "priority": "medium",
            "status": "todo",
            "due_date": null,
            "owner_id": null
        });

        let plan = merge_sync_snapshots_with_updated_from(
            &base,
            &local,
            &remote,
            Some(&json!({"title": "Base"})),
        );
        assert_eq!(plan.conflicts.len(), 1);
        assert_eq!(plan.conflicts[0].field, "title");
    }
}
