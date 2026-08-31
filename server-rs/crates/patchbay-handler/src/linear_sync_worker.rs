//! Durable Linear inbox/outbox worker.
//!
//! Webhook ingress and local issue writes only append rows to PostgreSQL. This
//! worker is the network boundary that drains those rows after a commit, so a
//! process restart cannot strand a connected workspace with an unprocessed
//! delivery. The worker is deliberately small and provider-neutral: GraphQL
//! errors are retried through the durable queue and local updates never make a
//! second network call while holding a transaction.

use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::{DateTime, Utc};
use patchbay_db::models::{LinearConnection, LinearSyncInbox, LinearSyncOutbox};
use patchbay_db::queries::{activity, issue as issue_q, linear as linear_q};
use patchbay_util::secretbox::SecretBox;
use serde_json::{json, Map, Value};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const BATCH_SIZE: i64 = 50;
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub struct LinearSyncWorker {
    pool: sqlx::PgPool,
    secret_box: Option<SecretBox>,
}

pub struct LinearSyncRuntime {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl LinearSyncWorker {
    pub fn new(pool: sqlx::PgPool, secret_box: Option<SecretBox>) -> Arc<Self> {
        Arc::new(Self { pool, secret_box })
    }

    pub fn start(self: Arc<Self>, cancel: CancellationToken) -> LinearSyncRuntime {
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { self.run(task_cancel).await });
        LinearSyncRuntime {
            cancel,
            task: Some(task),
        }
    }

    async fn run(&self, cancel: CancellationToken) {
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = ticker.tick() => {
                    if let Err(error) = self.process_once().await {
                        tracing::warn!(%error, "Linear sync worker poll failed");
                    }
                }
            }
        }
    }

    /// Drain one bounded batch from every active connection. Exposed for
    /// focused integration tests and for an operator-triggered maintenance
    /// pass; production normally calls it from the polling runtime.
    pub async fn process_once(&self) -> anyhow::Result<usize> {
        let connections = linear_q::list_active_connections(&self.pool).await?;
        let mut processed = 0;
        for connection in connections {
            let token = match self.access_token(&connection) {
                Ok(token) => token,
                Err(error) => {
                    tracing::warn!(connection_id = %connection.id, %error, "Linear sync connection secret unavailable");
                    continue;
                }
            };
            let outbox = linear_q::list_pending_outbox(
                &self.pool,
                connection.workspace_id,
                BATCH_SIZE,
            )
            .await?;
            for item in outbox {
                if item.connection_id != connection.id {
                    continue;
                }
                match self.process_outbox(&connection, &token, &item).await {
                    Ok(()) => processed += 1,
                    Err(error) => {
                        let delay = retry_delay(item.attempts);
                        tracing::warn!(outbox_id = %item.id, %error, delay_seconds = delay, "Linear outbox delivery failed");
                        let _ = linear_q::mark_outbox_failed(
                            &self.pool,
                            item.id,
                            &redact_error(&error),
                            delay,
                        )
                        .await;
                    }
                }
            }
            let inbox = linear_q::list_pending_inbox(&self.pool, connection.id, BATCH_SIZE).await?;
            for item in inbox {
                match self.process_inbox(&connection, &token, &item).await {
                    Ok(()) => processed += 1,
                    Err(error) => {
                        tracing::warn!(inbox_id = %item.id, %error, "Linear inbox delivery failed");
                        let _ = linear_q::mark_inbox_failed(
                            &self.pool,
                            item.id,
                            &redact_error(&error),
                        )
                        .await;
                    }
                }
            }
        }
        Ok(processed)
    }

    fn access_token(&self, connection: &LinearConnection) -> anyhow::Result<String> {
        let secret_box = self
            .secret_box
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("encrypted Linear secret storage is not configured"))?;
        let ciphertext = STANDARD.decode(&connection.access_token_encrypted)?;
        Ok(String::from_utf8(secret_box.open(&ciphertext)?)?)
    }

    async fn process_outbox(
        &self,
        connection: &LinearConnection,
        access_token: &str,
        item: &LinearSyncOutbox,
    ) -> anyhow::Result<()> {
        let Some(issue_id) = item.issue_id else {
            linear_q::mark_outbox_sent(&self.pool, item.id).await?;
            return Ok(());
        };
        let Some(link) = linear_q::get_issue_link(&self.pool, connection.workspace_id, issue_id).await? else {
            // The link may have been tombstoned between the issue update and
            // this worker pass. It is no longer actionable, so acknowledge the
            // row instead of retrying forever.
            linear_q::mark_outbox_sent(&self.pool, item.id).await?;
            return Ok(());
        };
        if link.status != "active" {
            linear_q::mark_outbox_sent(&self.pool, item.id).await?;
            return Ok(());
        }
        let issue = item
            .payload
            .get("issue")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Linear outbox payload has no issue"))?;
        let mut input = Map::new();
        if let Some(title) = issue.get("title").and_then(Value::as_str) {
            input.insert("title".into(), json!(title));
        }
        if let Some(description) = item.payload.get("managed_description").and_then(Value::as_str) {
            input.insert("description".into(), json!(description));
        } else if let Some(description) = issue.get("description").and_then(Value::as_str) {
            input.insert("description".into(), json!(description));
        }
        if let Some(priority) = issue.get("priority").and_then(Value::as_str) {
            input.insert("priority".into(), json!(linear_priority(priority)));
        }
        if input.is_empty() {
            linear_q::mark_outbox_sent(&self.pool, item.id).await?;
            return Ok(());
        }
        let data = crate::linear::graphql_request(
            access_token,
            r#"mutation PatchbayIssueUpdate($id: ID!, $input: IssueUpdateInput!) {
                issueUpdate(id: $id, input: $input) {
                    success
                    issue { id identifier updatedAt title description priority }
                }
            }"#,
            json!({"id": link.linear_issue_id, "input": input}),
        )
        .await?;
        let update = data
            .get("issueUpdate")
            .ok_or_else(|| anyhow::anyhow!("Linear issueUpdate response is missing"))?;
        anyhow::ensure!(
            update.get("success").and_then(Value::as_bool).unwrap_or(false),
            "Linear issueUpdate was not successful"
        );
        let remote = update
            .get("issue")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Linear issueUpdate returned no issue"))?;
        let remote_updated_at = parse_remote_updated_at(&remote);
        linear_q::mark_issue_link_pushed(
            &self.pool,
            link.id,
            remote_updated_at,
            &remote,
        )
        .await?;
        linear_q::mark_outbox_sent(&self.pool, item.id).await?;
        Ok(())
    }

    async fn process_inbox(
        &self,
        connection: &LinearConnection,
        access_token: &str,
        item: &LinearSyncInbox,
    ) -> anyhow::Result<()> {
        let remote = item
            .payload
            .get("data")
            .filter(|value| value.is_object())
            .unwrap_or(&item.payload);
        let Some(linear_issue_id) = remote.get("id").and_then(Value::as_str) else {
            linear_q::mark_inbox_processed(&self.pool, item.id).await?;
            return Ok(());
        };
        let mut link = linear_q::get_issue_link_by_linear_id(
            &self.pool,
            connection.workspace_id,
            linear_issue_id,
        )
        .await?;
        if link.is_none() {
            link = self.maybe_create_link(connection, remote).await?;
        }
        let Some(link) = link else {
            // The remote issue is outside the explicitly bound project scope.
            // Acknowledge it without importing or deleting either side.
            linear_q::mark_inbox_processed(&self.pool, item.id).await?;
            return Ok(());
        };
        self.apply_remote_issue(&link, remote, item.delivery_id.as_str()).await?;
        linear_q::mark_inbox_processed(&self.pool, item.id).await?;
        let _ = access_token; // reserved for relation/label hydration passes
        Ok(())
    }

    async fn maybe_create_link(
        &self,
        connection: &LinearConnection,
        remote: &Value,
    ) -> anyhow::Result<Option<patchbay_db::models::LinearIssueLink>> {
        let Some(local_id) = remote
            .get("patchbayIssueId")
            .or_else(|| remote.get("patchbay_issue_id"))
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
        else {
            return Ok(None);
        };
        let Some(linear_project_id) = remote
            .get("project")
            .and_then(|project| project.get("id"))
            .and_then(Value::as_str)
        else {
            return Ok(None);
        };
        let Some(binding) = linear_q::get_project_binding_by_linear_id(
            &self.pool,
            connection.workspace_id,
            linear_project_id,
        )
        .await?
        else {
            return Ok(None);
        };
        if binding.sync_mode == "push_only" {
            return Ok(None);
        }
        let link = linear_q::upsert_issue_link(
            &self.pool,
            &linear_q::IssueLinkInput {
                id: Uuid::now_v7(),
                workspace_id: connection.workspace_id,
                issue_id: local_id,
                linear_issue_id: remote
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                linear_identifier: remote
                    .get("identifier")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                project_binding_id: binding.id,
                remote_updated_at: parse_remote_updated_at(remote),
                remote_snapshot: remote.clone(),
            },
        )
        .await?;
        Ok(Some(link))
    }

    async fn apply_remote_issue(
        &self,
        link: &patchbay_db::models::LinearIssueLink,
        remote: &Value,
        delivery_id: &str,
    ) -> anyhow::Result<()> {
        let _current = issue_q::get_issue_in_workspace(&self.pool, link.issue_id, link.workspace_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("linked Patchbay issue no longer exists"))?;
        let title = remote.get("title").and_then(Value::as_str).map(str::to_string);
        let description = remote
            .get("description")
            .and_then(Value::as_str)
            .map(strip_managed_description);
        let priority = remote
            .get("priority")
            .and_then(Value::as_i64)
            .map(patchbay_priority)
            .or_else(|| remote.get("priority").and_then(Value::as_str).map(str::to_string));
        // An unknown/ambiguous Linear person must not erase a known local
        // owner. Only an explicit null assignee clears the owner; an object
        // that cannot be resolved by the immutable member binding is left
        // unbound and visible for an administrator to diagnose.
        let owner_update = match remote.get("assignee") {
            Some(Value::Null) => Some(None),
            Some(assignee) => match assignee.get("id").and_then(Value::as_str) {
                Some(assignee_id) => linear_q::resolve_patchbay_user_for_linear_user(
                    &self.pool,
                    link.workspace_id,
                    assignee_id,
                )
                .await?
                .map(Some),
                None => None,
            },
            None => None,
        };
        let label_id = remote
            .get("labels")
            .and_then(|labels| labels.get("nodes"))
            .and_then(Value::as_array)
            .and_then(|labels| labels.iter().find_map(|label| label.get("id").and_then(Value::as_str)))
            .map(str::to_string);
        let label_agent_id = if let Some(label_id) = label_id.as_deref() {
            linear_q::resolve_agent_for_linear_label(&self.pool, link.workspace_id, label_id)
                .await?
        } else {
            None
        };
        let status_id = remote
            .get("state")
            .and_then(|state| state.get("id"))
            .and_then(Value::as_str);
        let status = if let Some(status_id) = status_id {
            linear_q::list_status_bindings(&self.pool, link.project_binding_id)
                .await?
                .into_iter()
                .find(|binding| binding.linear_status_id == status_id)
                .map(|binding| binding.patchbay_status)
        } else {
            None
        };

        let owner_touched = owner_update.is_some();
        let owner_id = owner_update.flatten();
        let executor_touched = label_agent_id.is_some();
        let changed = title.is_some()
            || description.is_some()
            || priority.is_some()
            || owner_touched
            || executor_touched
            || status.is_some();
        let mut tx = self.pool.begin().await?;
        let updated = if changed {
            sqlx::query_as::<_, patchbay_db::models::Issue>(
                r#"UPDATE issue SET
                   title = COALESCE($3, title),
                   description = COALESCE($4, description),
                   priority = COALESCE($5, priority),
                   status = CASE WHEN $6 THEN COALESCE($7, status) ELSE status END,
                   owner_type = CASE WHEN $8 THEN CASE WHEN $9::uuid IS NULL THEN NULL ELSE 'member' END ELSE owner_type END,
                   owner_id = CASE WHEN $8 THEN $9 ELSE owner_id END,
                   executor_type = CASE WHEN $10 THEN 'agent' ELSE executor_type END,
                   executor_id = CASE WHEN $10 THEN $11 ELSE executor_id END,
                   revision = revision + 1, updated_at = now()
                   WHERE id = $1 AND workspace_id = $2
                   RETURNING *"#,
            )
            .bind(link.issue_id)
            .bind(link.workspace_id)
            .bind(title.as_deref())
            .bind(description.as_deref())
            .bind(priority.as_deref())
            .bind(status.is_some())
            .bind(status.as_deref())
            .bind(owner_touched)
            .bind(owner_id)
            .bind(executor_touched)
            .bind(label_agent_id)
            .fetch_optional(&mut *tx)
            .await?
        } else {
            None
        };
        if let Some(updated) = updated.as_ref() {
            let details = json!({
                "source": "linear_webhook",
                "delivery_id": delivery_id,
                "linear_issue_id": link.linear_issue_id,
                "changed": [
                    title.as_ref().map(|_| "title"),
                    description.as_ref().map(|_| "description"),
                    priority.as_ref().map(|_| "priority"),
                    owner_touched.then_some("owner"),
                    executor_touched.then_some("executor"),
                    status.as_ref().map(|_| "status"),
                ],
            });
            activity::create_activity(
                &mut *tx,
                updated.workspace_id,
                updated.id,
                Some("system"),
                None,
                "linear_issue_pulled",
                &details,
                Uuid::new_v5(&Uuid::NAMESPACE_URL, delivery_id.as_bytes()),
            )
            .await?;
        }
        linear_q::mark_issue_link_pulled(
            &mut *tx,
            link.id,
            parse_remote_updated_at(remote),
            remote,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

impl LinearSyncRuntime {
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        let Some(mut task) = self.task.take() else { return };
        if tokio::time::timeout(DEFAULT_SHUTDOWN_TIMEOUT, &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

fn retry_delay(attempts: i32) -> i64 {
    2_i64.saturating_pow(attempts.clamp(0, 8) as u32).clamp(5, 900)
}

fn redact_error(error: &anyhow::Error) -> String {
    let text = error.to_string();
    if text.len() > 500 { text[..500].to_string() } else { text }
}

fn parse_remote_updated_at(value: &Value) -> Option<DateTime<Utc>> {
    value
        .get("updatedAt")
        .and_then(Value::as_str)
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn linear_priority(priority: &str) -> i64 {
    match priority {
        "urgent" => 1,
        "high" => 2,
        "medium" => 3,
        "low" => 4,
        _ => 0,
    }
}

fn patchbay_priority(priority: i64) -> String {
    match priority {
        1 => "urgent",
        2 => "high",
        3 => "medium",
        4 => "low",
        _ => "none",
    }
    .to_string()
}

/// Removes only the stable Patchbay-managed block before storing a Linear
/// description locally. The surrounding human-authored text is preserved;
/// malformed markers are treated as ordinary text and left untouched.
fn strip_managed_description(description: &str) -> String {
    let Some(start) = description.find(patchbay_service::linear_sync::MANAGED_BLOCK_START) else {
        return description.to_string();
    };
    let Some(end_offset) = description[start..]
        .find(patchbay_service::linear_sync::MANAGED_BLOCK_END)
    else {
        return description.to_string();
    };
    let end = start + end_offset + patchbay_service::linear_sync::MANAGED_BLOCK_END.len();
    let mut human = String::with_capacity(description.len());
    human.push_str(&description[..start]);
    human.push_str(&description[end..]);
    human.trim().to_string()
}
