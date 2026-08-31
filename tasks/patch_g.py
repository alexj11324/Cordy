#!/usr/bin/env python3
"""automation.rs sub-slice E: public dispatch entry points."""
import sys

P = "server-rs/crates/patchbay-service/src/automation.rs"
s = open(P).read()

BODY = '''

// --- Public dispatch entry points -------------------------------------------

use patchbay_db::queries::agent::{get_automation_task_by_run, list_tasks_by_issue};
use patchbay_db::queries::automation::{
    get_automation_run_by_trigger_and_planned, get_automation_run_by_webhook_delivery,
};

impl AutomationService {
    /// Schedule/webhook/api entry point: no member actor (rule_owner
    /// attribution), no per-run reason-code surface for a human, and no
    /// webhook delivery id — durable deliveries admit through
    /// admit_automation_webhook_delivery instead.
    pub async fn dispatch_automation_public(
        &self,
        automation: &Automation,
        trigger_id: Uuid,
        source: &str,
        payload: &serde_json::Value,
    ) -> anyhow::Result<AutomationRun> {
        let key = format!("{source}:{}", new_request_idempotency_key());
        let outcome = self
            .dispatch_automation(
                automation,
                trigger_id,
                source,
                payload,
                None,
                Uuid::nil(),
                None,
                &key,
            )
            .await?;
        Ok(outcome.run)
    }

    /// "Run now" for a member: a direct human action attributed direct_human
    /// to the clicker across both execution modes (PB-4302 §4). A nil actor
    /// behaves exactly like source="manual" automation dispatch.
    pub async fn dispatch_automation_manual(
        &self,
        automation: &Automation,
        trigger_id: Uuid,
        payload: &serde_json::Value,
        actor_user_id: Option<Uuid>,
    ) -> anyhow::Result<DispatchOutcome> {
        self.dispatch_automation_manual_with_key(
            automation,
            trigger_id,
            payload,
            actor_user_id,
            &new_request_idempotency_key(),
        )
        .await
    }

    /// Preserves a caller-supplied request key so retrying the same HTTP
    /// request cannot reserve or execute twice. The manual path is the one
    /// surface that shows a per-run outcome code to a human, hence the typed
    /// reason code in the outcome.
    pub async fn dispatch_automation_manual_with_key(
        &self,
        automation: &Automation,
        trigger_id: Uuid,
        payload: &serde_json::Value,
        actor_user_id: Option<Uuid>,
        idempotency_key: &str,
    ) -> anyhow::Result<DispatchOutcome> {
        let key = format!("manual:{}:{idempotency_key}", automation.id);
        self.dispatch_automation(
            automation,
            trigger_id,
            "manual",
            payload,
            None,
            Uuid::nil(),
            actor_user_id,
            &key,
        )
        .await
    }

    /// Creates or reuses the idempotent run for a durable webhook delivery
    /// WITHOUT executing downstream issue/task side effects; HTTP keeps its
    /// 200 accepted/skipped + run_id contract while the database-backed worker
    /// still owns recoverable dispatch.
    pub async fn admit_automation_webhook_delivery(
        &self,
        automation: &Automation,
        trigger_id: Uuid,
        payload: &serde_json::Value,
        delivery_id: Uuid,
    ) -> anyhow::Result<AutomationRun> {
        if delivery_id.is_nil() {
            anyhow::bail!("admit webhook delivery: delivery_id is required");
        }

        if let Some(existing) =
            get_automation_run_by_webhook_delivery(&self.pool, delivery_id).await?
        {
            return Ok(existing);
        }

        // Webhook admission has no member actor → automation principal
        // (rule_owner); the per-run reason code is not surfaced to a human
        // here, so it is dropped.
        if let Some((reason, _code)) = self.should_skip_dispatch(automation, None).await {
            let skipped = self
                .record_skipped_run(
                    automation,
                    trigger_id,
                    "webhook",
                    payload,
                    None,
                    delivery_id,
                    &reason,
                    None,
                )
                .await;
            return match skipped {
                Ok(run) => Ok(run),
                // A concurrent replica may have admitted the same delivery
                // while we evaluated the gate — reuse its run instead of
                // surfacing our loser error.
                Err(err) => match self.recover_concurrent_webhook_admission(delivery_id).await? {
                    Some(run) => Ok(run),
                    None => Err(anyhow::anyhow!(
                        "admit webhook delivery: create skipped run: {err}"
                    )),
                },
            };
        }

        let initial_status = if automation.execution_mode == "run_only" {
            "running"
        } else {
            "issue_created"
        };
        let params = CreateAutomationRunParams {
            automation_id: automation.id,
            trigger_id,
            source: "webhook".to_string(),
            status: initial_status.to_string(),
            trigger_payload: payload.clone(),
            team_id: Self::team_attribution(automation).unwrap_or_else(Uuid::nil),
            planned_at: None,
            webhook_delivery_id: delivery_id,
            reason_code: None,
        };
        match self
            .create_run_with_quota(
                automation.workspace_id,
                "webhook",
                &format!("webhook:{delivery_id}"),
                &params,
            )
            .await
        {
            Ok((run, _)) => {
                self.capture_automation_run_started(automation, &run, "webhook").await;
                Ok(run)
            }
            // Another replica may have claimed this durable delivery after our
            // admission lookup — the unique delivery/run index picks one winner
            // and the loser reuses that run. Go gates recovery on a typed 23505
            // cause; reloading unconditionally is equivalent because it only
            // ever reuses an actually-existing row.
            Err(err) => match self.recover_concurrent_webhook_admission(delivery_id).await? {
                Some(run) => Ok(run),
                None => Err(anyhow::anyhow!("admit webhook delivery: create run: {err}")),
            },
        }
    }

    /// Reloads the winning concurrent run after an admission collision.
    /// Ok(None) means no winner materialized and the caller should propagate
    /// its original cause.
    async fn recover_concurrent_webhook_admission(
        &self,
        delivery_id: Uuid,
    ) -> anyhow::Result<Option<AutomationRun>> {
        get_automation_run_by_webhook_delivery(&self.pool, delivery_id)
            .await
            .map_err(|e| {
                anyhow::anyhow!("admit webhook delivery: reload concurrent run: {e}")
            })
    }

    /// Durable webhook worker entry point. webhook_delivery_id is persisted on
    /// the run under a partial unique index, so reclaiming a queued delivery
    /// after a crash reuses the original run instead of creating a second
    /// issue or task.
    pub async fn dispatch_automation_for_webhook_delivery(
        &self,
        automation: &Automation,
        trigger_id: Uuid,
        payload: &serde_json::Value,
        delivery_id: Uuid,
    ) -> anyhow::Result<AutomationRun> {
        let mut run = self
            .admit_automation_webhook_delivery(automation, trigger_id, payload, delivery_id)
            .await?;
        if is_run_complete(&run) {
            if automation.execution_mode == "create_issue" && run.issue_id.is_some() {
                self.ensure_webhook_create_issue_task(automation, &run).await?;
            }
            return Ok(run);
        }

        // A run_only task may have committed immediately before the process
        // died while linking task_id back to the run. Repair that linkage and
        // wake the daemon; otherwise continue the same partial run below.
        if automation.execution_mode == "run_only" && run.task_id.is_none() {
            if let Some(repaired) = self
                .repair_automation_run_task_link(&run)
                .await
                .map_err(|e| anyhow::anyhow!("dispatch for webhook delivery: {e}"))?
            {
                return Ok(repaired);
            }
        }
        // Worker dispatch has no member actor; the reason code is dropped.
        let outcome = self
            .dispatch_automation_run(automation, trigger_id, "webhook", &mut run, None)
            .await?;
        Ok(outcome.run)
    }

    /// Repairs the create_issue crash window after the issue/run transaction
    /// commits but before the ordinary task enqueue does. Any existing issue
    /// task proves ownership already moved downstream; otherwise enqueue via
    /// exactly the executor path used by the original dispatch.
    async fn ensure_webhook_create_issue_task(
        &self,
        automation: &Automation,
        run: &AutomationRun,
    ) -> anyhow::Result<()> {
        let issue_id = run.issue_id.expect("guarded by caller");
        let tasks = list_tasks_by_issue(&self.pool, issue_id)
            .await
            .map_err(|e| {
                anyhow::anyhow!("dispatch for webhook delivery: inspect issue tasks: {e}")
            })?;
        if !tasks.is_empty() {
            return Ok(());
        }
        let issue = get_issue(&self.pool, issue_id)
            .await
            .map_err(|e| {
                anyhow::anyhow!("dispatch for webhook delivery: load linked issue: {e}")
            })?
            .ok_or_else(|| {
                anyhow::anyhow!("dispatch for webhook delivery: linked issue missing")
            })?;
        let effective =
            crate::issue_status::effective(&self.pool, issue.workspace_id, &issue.status).await;
        if effective != "todo" && effective != "in_progress" {
            return Ok(());
        }
        if automation.executor_type == "team" {
            let (leader, _) = self.resolve_leader(automation).await.map_err(|e| {
                anyhow::anyhow!("dispatch for webhook delivery: resolve team leader: {e}")
            })?;
            self.task_svc
                .enqueue_task_for_team_leader(&issue, leader.id, automation.executor_id, None)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("dispatch for webhook delivery: repair team task: {e}")
                })?;
            return Ok(());
        }
        self.task_svc
            .enqueue_task_for_issue(&issue, None)
            .await
            .map_err(|e| anyhow::anyhow!("dispatch for webhook delivery: repair issue task: {e}"))?;
        Ok(())
    }

    /// Closes the run_only crash window where task creation committed but
    /// automation_run.task_id did not. Finding any task proves downstream
    /// ownership moved: active work is re-woken, terminal work replays through
    /// the normal finalizer instead of being duplicated.
    /// Returns the repaired run when a linked task exists.
    async fn repair_automation_run_task_link(
        &self,
        run: &AutomationRun,
    ) -> anyhow::Result<Option<AutomationRun>> {
        let Some(task) = get_automation_task_by_run(&self.pool, run.id).await? else {
            return Ok(None);
        };
        let mut updated = update_automation_run_running(&self.pool, run.id, task.id)
            .await
            .map_err(|e| anyhow::anyhow!("repair task linkage: {e}"))?
            .ok_or_else(|| anyhow::anyhow!("repair task linkage: no row"))?;
        match task.status.as_str() {
            "completed" | "failed" | "cancelled" => {
                self.sync_run_from_task(&task).await;
                updated = get_automation_run(&self.pool, run.id)
                    .await
                    .map_err(|e| anyhow::anyhow!("reload terminal repaired run: {e}"))?
                    .ok_or_else(|| {
                        anyhow::anyhow!("reload terminal repaired run: no row")
                    })?;
            }
            _ => {
                self.task_svc.notify_task_enqueued(&task).await;
            }
        }
        Ok(Some(updated))
    }

    /// Scheduler bridge for one cron occurrence. Idempotent per
    /// (trigger_id, planned_at) via the partial unique index: complete runs
    /// short-circuit, partial runs are recovered (or repaired for run_only)
    /// before a fresh dispatch claims the slot. plannedAt is always a real
    /// timestamp from the scheduler, so Go's zero-time guard has no Rust
    /// counterpart.
    pub async fn dispatch_automation_for_plan(
        &self,
        automation: &Automation,
        trigger_id: Uuid,
        source: &str,
        payload: &serde_json::Value,
        planned_at: DateTime<Utc>,
    ) -> anyhow::Result<AutomationRun> {
        if trigger_id.is_nil() {
            anyhow::bail!("dispatch for plan: trigger_id is required");
        }

        // Fast path: a prior attempt already created a run for this exact
        // occurrence. The unique index would also reject a duplicate INSERT,
        // but looking up front lets us short-circuit on a complete run and
        // gives us a chance to recover a partial run before retrying.
        match get_automation_run_by_trigger_and_planned(
            &self.pool,
            trigger_id,
            Some(planned_at),
        )
        .await
        {
            Ok(Some(existing)) => {
                if is_run_complete(&existing) {
                    // Hand the complete run back so the job records SUCCESS in
                    // sys_cron_executions without duplicating any side effect.
                    return Ok(existing);
                }
                if automation.execution_mode == "run_only" && existing.task_id.is_none() {
                    if let Some(repaired) = self
                        .repair_automation_run_task_link(&existing)
                        .await
                        .map_err(|e| anyhow::anyhow!("dispatch for plan: {e}"))?
                    {
                        return Ok(repaired);
                    }
                }
                // Partial-state run from a crashed attempt. Mark it failed
                // (with a recovery reason) and release its partial-unique slot
                // so the fresh dispatch below can create a new row.
                tracing::warn!(
                    run_id = %existing.id,
                    trigger_id = %trigger_id,
                    planned_at = %crate::task_notify::rfc3339_nano(planned_at),
                    status = %existing.status,
                    issue_set = existing.issue_id.is_some(),
                    task_set = existing.task_id.is_some(),
                    "automation dispatch for plan: recovering partial run"
                );
                let recovered = self
                    .recover_partial_automation_run(existing.id)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("dispatch for plan: recover partial run: {e}")
                    })?;
                if !recovered {
                    anyhow::bail!(
                        "dispatch for plan: partial run changed concurrently; retry"
                    );
                }
                // Fall through to a fresh dispatch below.
            }
            // No prior attempt for this occurrence.
            Ok(None) => {}
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "dispatch for plan: lookup existing run: {e}"
                ));
            }
        }

        // Scheduled dispatch has no member actor → rule_owner attribution, and
        // no human surface for a per-run reason code.
        let key = format!(
            "schedule:{}:{}",
            trigger_id,
            crate::task_notify::rfc3339_nano(planned_at)
        );
        let outcome = self
            .dispatch_automation(
                automation,
                trigger_id,
                source,
                payload,
                Some(planned_at),
                Uuid::nil(),
                None,
                &key,
            )
            .await?;
        Ok(outcome.run)
    }
}
'''

s = s + BODY
open(P, "w").write(s)
print("patch G ok")
