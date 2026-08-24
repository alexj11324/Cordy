//! Durable PostgreSQL-backed Autopilot webhook delivery worker.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use cordy_db::models::{AutopilotTrigger, WebhookDelivery};
use cordy_db::queries::{autopilot, webhook_delivery};
use cordy_service::autopilot::{AutopilotQuotaExceededError, AutopilotService};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const MAX_ATTEMPTS: i32 = 5;
const CONCURRENCY: usize = 4;
pub const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// The queue and lease live in PostgreSQL. `Notify` is only a latency hint;
/// polling recovers rows after process restarts or missed local notifications.
pub struct WebhookDeliveryWorker {
    pool: sqlx::PgPool,
    autopilots: Arc<AutopilotService>,
    notify: Arc<Notify>,
}

impl WebhookDeliveryWorker {
    pub fn new(
        pool: sqlx::PgPool,
        autopilots: Arc<AutopilotService>,
        notify: Arc<Notify>,
    ) -> Arc<Self> {
        Arc::new(Self {
            pool,
            autopilots,
            notify,
        })
    }

    /// Starts an owned supervisor. Its JoinSet owns all four workers, so
    /// aborting the supervisor cannot leave detached delivery loops behind.
    pub fn start(self: Arc<Self>, cancel: CancellationToken) -> WebhookDeliveryRuntime {
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { self.run_workers(task_cancel).await });
        WebhookDeliveryRuntime {
            cancel,
            task: Some(task),
        }
    }

    async fn run_workers(self: Arc<Self>, cancel: CancellationToken) {
        let mut workers = tokio::task::JoinSet::new();
        for _ in 0..CONCURRENCY {
            let worker = self.clone();
            let worker_cancel = cancel.child_token();
            workers.spawn(async move { worker.run_loop(worker_cancel).await });
        }
        while let Some(result) = workers.join_next().await {
            if let Err(error) = result {
                tracing::error!(%error, "webhook delivery worker stopped unexpectedly");
            }
        }
    }

    async fn run_loop(&self, cancel: CancellationToken) {
        loop {
            let processed = tokio::select! {
                _ = cancel.cancelled() => return,
                result = self.process_next() => result,
            };
            match processed {
                Ok(true) => continue,
                Ok(false) => {}
                Err(error) => tracing::error!(%error, "webhook worker failed to process delivery"),
            }
            tokio::select! {
                _ = cancel.cancelled() => return,
                () = self.notify.notified() => {},
                () = tokio::time::sleep(POLL_INTERVAL) => {},
            }
        }
    }

    pub async fn process_next(&self) -> anyhow::Result<bool> {
        let Some(delivery) = webhook_delivery::claim_queued_webhook_delivery(&self.pool).await?
        else {
            return Ok(false);
        };
        let Some(lease_token) = delivery.lease_token else {
            anyhow::bail!("claimed webhook delivery has no lease token");
        };

        // The ingress persists before returning its authentication response.
        // If that terminal update failed after the insert, the durable row may
        // still be queued. Never let the recovery worker dispatch a payload
        // whose signature was already classified as missing or invalid.
        if matches!(delivery.signature_status.as_str(), "missing" | "invalid") {
            let reason = if delivery.signature_status == "missing" {
                "missing_signature"
            } else {
                "invalid_signature"
            };
            self.complete(
                &delivery,
                lease_token,
                "rejected",
                None,
                Some(reason),
                Some(reason),
            )
            .await?;
            return Ok(true);
        }

        let trigger = match autopilot::get_autopilot_trigger(&self.pool, delivery.trigger_id).await
        {
            Ok(Some(trigger)) => trigger,
            Ok(None) => {
                self.retry_or_fail(&delivery, lease_token, "load trigger: no row")
                    .await?;
                return Ok(true);
            }
            Err(error) => {
                self.retry_or_fail(&delivery, lease_token, &format!("load trigger: {error}"))
                    .await?;
                return Ok(true);
            }
        };
        let autopilot_row = match autopilot::get_autopilot(&self.pool, delivery.autopilot_id).await
        {
            Ok(Some(autopilot)) => autopilot,
            Ok(None) => {
                self.retry_or_fail(&delivery, lease_token, "load autopilot: no row")
                    .await?;
                return Ok(true);
            }
            Err(error) => {
                self.retry_or_fail(&delivery, lease_token, &format!("load autopilot: {error}"))
                    .await?;
                return Ok(true);
            }
        };
        if trigger.autopilot_id != delivery.autopilot_id
            || autopilot_row.workspace_id != delivery.workspace_id
        {
            self.complete(
                &delivery,
                lease_token,
                "failed",
                None,
                Some("delivery ownership mismatch"),
                None,
            )
            .await?;
            return Ok(true);
        }

        let envelope = match normalize_stored_payload(&delivery) {
            Ok(envelope) => envelope,
            Err(error) => {
                self.complete(
                    &delivery,
                    lease_token,
                    "failed",
                    None,
                    Some(&format!("normalize stored body: {error}")),
                    None,
                )
                .await?;
                return Ok(true);
            }
        };
        let admitted = match cordy_db::queries::autopilot::get_autopilot_run_by_webhook_delivery(
            &self.pool,
            delivery.id,
        )
        .await
        {
            Ok(run) => run,
            Err(error) => {
                self.retry_or_fail(
                    &delivery,
                    lease_token,
                    &format!("load admitted run: {error}"),
                )
                .await?;
                return Ok(true);
            }
        };
        if admitted.is_none() {
            let ignored = if !trigger.enabled {
                Some("trigger_disabled")
            } else if autopilot_row.status == "archived" {
                Some("autopilot_archived")
            } else if autopilot_row.status != "active" {
                Some("autopilot_paused")
            } else if !event_allowed(&trigger, &envelope) {
                Some("event_filtered")
            } else {
                None
            };
            if let Some(reason) = ignored {
                self.complete(&delivery, lease_token, "ignored", None, None, Some(reason))
                    .await?;
                return Ok(true);
            }
        }

        match self
            .autopilots
            .dispatch_autopilot_for_webhook_delivery(
                &autopilot_row,
                trigger.id,
                &envelope,
                delivery.id,
            )
            .await
        {
            Ok(run) if run.status == "failed" => {
                self.complete(
                    &delivery,
                    lease_token,
                    "failed",
                    Some(run.id),
                    Some(
                        run.failure_reason
                            .as_deref()
                            .unwrap_or("autopilot run failed"),
                    ),
                    None,
                )
                .await?;
            }
            Ok(run) => {
                if let Err(error) =
                    autopilot::touch_autopilot_trigger_fired_at(&self.pool, trigger.id).await
                {
                    tracing::warn!(
                        %error,
                        delivery_id = %delivery.id,
                        trigger_id = %trigger.id,
                        "webhook worker failed to update trigger last_fired_at"
                    );
                }
                self.complete(
                    &delivery,
                    lease_token,
                    "dispatched",
                    Some(run.id),
                    None,
                    None,
                )
                .await?;
            }
            Err(error)
                if error
                    .downcast_ref::<AutopilotQuotaExceededError>()
                    .is_some() =>
            {
                self.complete(
                    &delivery,
                    lease_token,
                    "ignored",
                    None,
                    Some("autopilot run quota exceeded"),
                    Some("quota_exceeded"),
                )
                .await?;
            }
            Err(error) => {
                self.retry_or_fail(&delivery, lease_token, &error.to_string())
                    .await?;
            }
        }
        Ok(true)
    }

    async fn complete(
        &self,
        delivery: &WebhookDelivery,
        lease_token: Uuid,
        status: &str,
        run_id: Option<Uuid>,
        error: Option<&str>,
        reason_code: Option<&str>,
    ) -> anyhow::Result<()> {
        let _ = webhook_delivery::complete_claimed_webhook_delivery(
            &self.pool,
            delivery.id,
            lease_token,
            status,
            run_id,
            error,
            reason_code,
        )
        .await?;
        Ok(())
    }

    async fn retry_or_fail(
        &self,
        delivery: &WebhookDelivery,
        lease_token: Uuid,
        cause: &str,
    ) -> anyhow::Result<()> {
        if delivery.dispatch_attempts + 1 >= MAX_ATTEMPTS {
            return self
                .complete(delivery, lease_token, "failed", None, Some(cause), None)
                .await;
        }
        let exponent = delivery.dispatch_attempts.clamp(0, 6) as u32;
        let available_at = Utc::now() + chrono::Duration::seconds(1_i64 << exponent);
        let _ = webhook_delivery::retry_claimed_webhook_delivery(
            &self.pool,
            delivery.id,
            lease_token,
            Some(available_at),
            Some(cause),
        )
        .await?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookShutdownOutcome {
    Stopped,
    Panicked,
    TimedOut,
}

/// Production-owned root for the webhook delivery supervisor and its worker
/// JoinSet. Drop is a fail-safe abort; normal shutdown is cooperative/bounded.
pub struct WebhookDeliveryRuntime {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl WebhookDeliveryRuntime {
    pub async fn shutdown(mut self, timeout: Duration) -> WebhookShutdownOutcome {
        self.cancel.cancel();
        let mut task = self
            .task
            .take()
            .expect("webhook delivery runtime always owns a supervisor");
        match tokio::time::timeout(timeout, &mut task).await {
            Ok(Ok(())) => WebhookShutdownOutcome::Stopped,
            Ok(Err(_)) => WebhookShutdownOutcome::Panicked,
            Err(_) => {
                task.abort();
                let _ = task.await;
                WebhookShutdownOutcome::TimedOut
            }
        }
    }
}

impl Drop for WebhookDeliveryRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn normalize_stored_payload(delivery: &WebhookDelivery) -> Result<Value, &'static str> {
    let raw = delivery.raw_body.as_deref().ok_or("empty body")?;
    let raw = raw.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(raw);
    let body: Value = serde_json::from_slice(raw).map_err(|_| "invalid json")?;
    if !matches!(body, Value::Object(_) | Value::Array(_)) {
        return Err("body must be a JSON object or array");
    }
    // `delivery.event` is the normalized provider-aware value computed by
    // ingress (for example `github.pull_request.opened`). It is authoritative
    // during recovery; rebuilding from the raw body alone loses header-derived
    // identity after a crash.
    let event = delivery.event.clone();
    let payload = body
        .as_object()
        .and_then(|object| object.get("eventPayload"))
        .filter(|_| {
            body.get("event")
                .and_then(Value::as_str)
                .is_some_and(|event| !event.is_empty())
        })
        .cloned()
        .unwrap_or(body);
    Ok(json!({
        "event": event,
        "eventPayload": payload,
        "request": {
            "receivedAt": crate::timefmt::rfc3339(delivery.received_at),
            "contentType": delivery.content_type,
        }
    }))
}

#[derive(Debug, Deserialize)]
struct EventFilter {
    event: String,
    #[serde(default)]
    actions: Vec<String>,
}

fn event_allowed(trigger: &AutopilotTrigger, envelope: &Value) -> bool {
    let Some(raw) = trigger.event_filters.as_ref() else {
        return true;
    };
    let Ok(filters) = serde_json::from_value::<Vec<EventFilter>>(raw.clone()) else {
        tracing::warn!(trigger_id = %trigger.id, "webhook trigger has malformed event filters");
        return false;
    };
    if filters.is_empty() {
        return true;
    }
    let event = envelope
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (name, suffix) = split_event(event);
    let payload = envelope.get("eventPayload").unwrap_or(&Value::Null);
    let mut candidates = HashSet::new();
    if !suffix.is_empty() {
        candidates.insert(suffix.to_string());
    }
    if let Some(object) = payload.as_object() {
        for field in ["action", "state", "conclusion", "status"] {
            if let Some(value) = object
                .get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                candidates.insert(value.to_string());
            }
        }
    }
    filters.into_iter().any(|filter| {
        filter.event == name
            && (filter.actions.is_empty()
                || filter
                    .actions
                    .iter()
                    .any(|action| candidates.contains(action)))
    })
}

fn split_event(event: &str) -> (&str, &str) {
    let mut parts = event.split('.');
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    if matches!(first, "github" | "gitlab" | "bitbucket" | "gitea") {
        let name = second.unwrap_or_default();
        let offset = first.len() + usize::from(second.is_some()) + name.len();
        return (
            name,
            event
                .get(offset + usize::from(offset < event.len())..)
                .unwrap_or_default(),
        );
    }
    let suffix = second
        .and_then(|_| event.get(first.len() + 1..))
        .unwrap_or_default();
    (first, suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_event_preserves_multi_segment_action() {
        assert_eq!(
            split_event("github.workflow_run.completed.success"),
            ("workflow_run", "completed.success")
        );
        assert_eq!(split_event("deploy.finished"), ("deploy", "finished"));
    }

    #[test]
    fn filters_consider_payload_action() {
        let now = Utc::now();
        let trigger = AutopilotTrigger {
            id: Uuid::new_v4(),
            autopilot_id: Uuid::new_v4(),
            kind: "webhook".into(),
            enabled: true,
            cron_expression: None,
            timezone: None,
            next_run_at: None,
            webhook_token: None,
            label: None,
            last_fired_at: None,
            created_at: now,
            updated_at: now,
            provider: "github".into(),
            signing_secret: None,
            event_filters: Some(json!([{"event":"pull_request","actions":["opened"]}])),
            published_by_type: None,
            published_by_id: None,
        };
        assert!(event_allowed(
            &trigger,
            &json!({"event":"github.pull_request","eventPayload":{"action":"opened"}})
        ));
    }
}
